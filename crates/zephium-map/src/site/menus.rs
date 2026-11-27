use super::*;
use crate::Node;

const MIN_MENU_ITEMS: usize = 2;

mod editions;
use editions::Editions;
#[cfg(test)]
mod tests;

#[derive(Default)]
pub(super) struct Menus {
    actions: Vec<Entry>,
    members: BTreeSet<EntryId>,
    groups: BTreeMap<usize, Menu>,
    order: BTreeMap<EntryId, usize>,
    direct: BTreeMap<(crate::PageMatch, Option<String>), (usize, Entry)>,
    excluded: BTreeSet<EntryId>,
    evidence: Evidence,
}

#[derive(Default)]
pub(super) struct Evidence {
    direct: BTreeMap<EntryId, (crate::PageMatch, Option<String>)>,
    pages: BTreeMap<String, Option<EntryId>>,
    languages: BTreeSet<String>,
    editions: std::collections::HashSet<(crate::PageMatch, Locator)>,
}

impl Evidence {
    pub(super) fn measure(&self, place: &Place, stats: &mut PlaceStats) {
        let primary: BTreeSet<_> = place
            .sections
            .iter()
            .filter(|e| e.tier == Tier::Primary)
            .flat_map(|e| [e.id.clone(), EntryId::new(None, &e.key, &e.locator)])
            .collect();
        let primary_routes: BTreeSet<_> = place
            .sections
            .iter()
            .filter(|e| e.tier == Tier::Primary && !e.menu_only)
            .map(Entry::card_route)
            .collect();
        stats.missing_header_links = self
            .direct
            .iter()
            .filter(|(id, route)| !primary.contains(id) && !primary_routes.contains(route))
            .count();
        for e in &place.sections {
            let label = normal(&e.label);
            let language_group = self.languages.contains(&label) && !e.children.is_empty()
                || e.children.len() >= MIN_MENU_ITEMS
                    && e.children
                        .iter()
                        .all(|c| self.editions.contains(&(c.card_page(), c.locator.clone())));
            stats.language_groups += usize::from(language_group);
            if !e.menu_only
                && (e.children.iter().any(|c| c.card_route() == e.card_route())
                    || !self.direct.contains_key(&e.id)
                        && self.pages.get(&label).is_some_and(|page| {
                            page.as_ref().is_none_or(|id| {
                                *id != e.id && *id != EntryId::new(None, &e.key, &e.locator)
                            })
                        }))
            {
                stats.invalid_group_pages += 1;
            }
        }
    }
}

#[derive(Clone, Copy, Default, PartialEq)]
enum Region {
    #[default]
    Other,
    Navigation,
    Footer,
    Excluded,
}

struct ObservedLink<'a> {
    url: &'a Url,
    label: &'a str,
    action: bool,
    entry: Option<Entry>,
}

impl<'a> ObservedLink<'a> {
    fn new(build: &Build<'_>, url: &'a Url, label: &'a str, marked: bool) -> Self {
        let entry = build.entry_for(url, label, Source::MenuGroup);
        let action = marked
            || entry
                .as_ref()
                .map_or_else(|| card::action_target(url, label), card::action);
        Self {
            url,
            label,
            action,
            entry,
        }
    }

    fn route(&self, build: &Build<'_>) -> Option<(crate::PageMatch, Option<String>)> {
        if let Some(entry) = &self.entry {
            return Some(entry.card_route());
        }
        let key = PageKey::from_url(self.url).ok()?;
        let page = card::card_page(&key, build.localized.contains(&key.page_match()));
        Some((page, element_locator(self.url).ok()?.fragment()))
    }
}

struct Menu {
    label: String,
    page: Option<Entry>,
    index: Option<Entry>,
    items: Vec<Entry>,
    trigger: Option<EntryId>,
    source: Source,
    editions: Editions,
}

impl Menu {
    fn add_item(&mut self, mut item: Entry) {
        let label = normal(&item.label);
        let group = normal(&self.label);
        if label == group
            || group_page(&label) == Some(group.as_str())
            || matches!(
                label.as_str(),
                "see all" | "view all" | "browse all" | "explore all"
            )
            || label == "overview"
                && item
                    .key
                    .as_url()
                    .path_segments()
                    .and_then(|mut s| s.next_back())
                    .is_some_and(|s| normal(s) == group)
        {
            self.index.get_or_insert(item);
        } else {
            item.menu_item = true;
            self.items.push(item);
        }
    }
}

pub(super) fn link_label(n: &Node) -> &str {
    let prefix = n.text.get(..n.link_prefix_bytes).filter(|s| {
        !s.is_empty()
            && n.text[s.len()..].starts_with(char::is_whitespace)
            && s.chars().any(char::is_alphabetic)
            && !matches!(normal(s).as_str(), "new" | "beta")
    });
    choose_link_label(&n.name, &n.text, prefix)
}

fn choose_link_label<'a>(name: &'a str, text: &'a str, prefix: Option<&'a str>) -> &'a str {
    [Some(name.trim()), Some(text.trim()), prefix]
        .into_iter()
        .flatten()
        .filter(|s| !s.is_empty())
        .min_by_key(|s| s.chars().count())
        .unwrap_or_default()
}

pub(super) fn skip_node(n: &Node, url: &Url, label: &str, landing: &Url) -> bool {
    card::skip_link(url, &[n.accessible_name(), label], landing)
}

fn group_page(label: &str) -> Option<&str> {
    ["see all ", "view all ", "browse all ", "explore all "]
        .into_iter()
        .find_map(|prefix| label.strip_prefix(prefix))
}

impl Menus {
    pub(super) fn read(build: &Build<'_>, doc: &Doc) -> Self {
        let nodes = doc.nodes();
        let mut menus = Self::default();
        let mut scopes = BTreeMap::new();
        let mut named = BTreeMap::new();
        let mut footer = BTreeMap::new();
        let mut first_nav = None;
        let alternates: BTreeSet<_> = doc
            .head()
            .alternates
            .iter()
            .filter(|a| !a.language.is_empty())
            .filter_map(|a| {
                Some((
                    PageKey::from_url(&a.url).ok()?.page_match(),
                    a.url.fragment().map(str::to_owned),
                ))
            })
            .collect();
        let mut standalone = Vec::new();
        let mut actions: BTreeSet<_> = build
            .sections
            .iter()
            .flat_map(|e| std::iter::once(e).chain(&e.children))
            .filter(|e| card::action(e) || build.names.get(&e.id).is_some_and(|name| name.action))
            .map(Entry::card_route)
            .collect();
        let clusters: BTreeSet<_> = build
            .sections
            .iter()
            .filter(|e| e.sources.contains(&Source::LinkCluster))
            .map(|e| e.id.clone())
            .collect();
        let mut following = vec![None; nodes.len()];
        let mut sibling = vec![None; nodes.len()];
        let mut peer_buttons = vec![0; nodes.len()];
        for (i, n) in nodes.iter().enumerate().rev() {
            if let Some(p) = n.parent_index() {
                following[i] = sibling[p].replace(i);
                peer_buttons[p] += usize::from(n.role == Role::Button && visible(nodes, i));
            }
        }
        let self_link = |e: &Entry| card::self_link(e, &build.entry, Some(doc.page()));
        let profile = |e: &Entry| e.off_site && site_profile(e.key.as_url()).is_some();
        let mut targets = Vec::new();
        let mut regions = Vec::with_capacity(nodes.len());
        for (i, n) in nodes.iter().enumerate() {
            let inherited = n.parent_index().map(|p| regions[p]).unwrap_or_default();
            let region = match (inherited, n.role) {
                (Region::Excluded, _) => Region::Excluded,
                (
                    _,
                    Role::Landmark {
                        landmark: Some(Landmark::Contentinfo),
                    },
                ) => Region::Footer,
                (_, role) if role.outside_menu() => Region::Excluded,
                (Region::Footer, _) => Region::Footer,
                (
                    _,
                    Role::Landmark {
                        landmark: Some(Landmark::Banner),
                    },
                ) => Region::Navigation,
                (
                    _,
                    Role::Landmark {
                        landmark: Some(Landmark::Navigation),
                    },
                ) if visible(nodes, i) && *first_nav.get_or_insert(i) == i => Region::Navigation,
                _ => inherited,
            };
            regions.push(region);
            if region == Region::Excluded
                || region == Region::Other && (clusters.is_empty() || n.role != Role::Link)
                || !visible(nodes, i)
            {
                continue;
            }
            let nav = region == Region::Navigation;
            let parent = n.parent_index();
            let label = if n.role == Role::Text {
                n.text.trim()
            } else {
                n.accessible_name().trim()
            };
            let control_action = if let Some(url) = &n.destination {
                let short = link_label(n);
                let link =
                    ObservedLink::new(build, url, short, n.marks.has(crate::doc::Marks::ACTION));
                if skip_node(n, url, short, doc.page()) {
                    menus.excluded.extend(link.entry.map(|e| e.id));
                    continue;
                }
                let action = link.action;
                targets.push((i, None, link));
                action
            } else {
                false
            };
            if region == Region::Footer {
                if matches!(n.role, Role::Text | Role::Heading { .. } | Role::Button)
                    && !n.text.trim().is_empty()
                    && !ancestors(nodes, i).any(|a| nodes[a].role == Role::Link)
                {
                    if let Some(p) = parent {
                        footer.insert(p, normal(&n.text));
                    }
                }
                continue;
            }
            let paired_panel = |control: usize| {
                let p = nodes[control].parent_index()?;
                let panel = following[control]?;
                (matches!(nodes[p].role, Role::Group | Role::ListItem)
                    && sibling[p] == Some(control)
                    && following[panel].is_none()
                    && matches!(
                        nodes[panel].role,
                        Role::List
                            | Role::Group
                            | Role::Landmark {
                                landmark: Some(Landmark::Navigation)
                            }
                    ))
                .then_some(panel)
            };
            let scope = match n.role {
                Role::Group | Role::List if !label.is_empty() => Some(i),
                Role::Button | Role::Link if doc.menu_links.contains_key(&i) => Some(i),
                Role::Button | Role::Heading { .. } => parent
                    .filter(|&p| {
                        matches!(nodes[p].role, Role::Group | Role::ListItem)
                            && (n.role != Role::Button || peer_buttons[p] == 1)
                    })
                    .or_else(|| {
                        following[i]
                            .filter(|&next| matches!(nodes[next].role, Role::Group | Role::List))
                    }),
                Role::Link => parent
                    .filter(|&p| nodes[p].role == Role::ListItem && !scopes.contains_key(&p))
                    .or_else(|| paired_panel(i)),
                Role::Text if nav => paired_panel(i).or_else(|| {
                    parent
                        .filter(|&p| {
                            nodes[p].role == Role::Group
                                && sibling[p] == Some(i)
                                && following[i].is_none()
                        })
                        .and_then(paired_panel)
                }),
                _ => None,
            };
            let Some(scope) = scope else { continue };
            if normal(label)
                .split_whitespace()
                .any(|w| w == "menu" || w == "navigation")
                || card::placeholder(label)
                || card::sentence(label)
                || ancestors(nodes, i)
                    .skip(1)
                    .any(|a| nodes[a].role == Role::Link)
            {
                continue;
            }
            let key = normal(label);
            let page = n
                .destination
                .as_ref()
                .and_then(|u| build.entry_for(u, label, Source::PrimaryNav));
            let panel = doc.menu_links.get(&i);
            let action = control_action
                || page
                    .as_ref()
                    .is_some_and(|e| card::action(e) || actions.contains(&e.card_route()));

            if panel.is_none()
                && (action
                    || page.as_ref().is_some_and(&self_link)
                        && n.destination
                            .as_ref()
                            .is_some_and(|u| u.fragment().is_none()))
            {
                continue;
            }
            let page = page.filter(|e| !action && !self_link(e) && !profile(e));
            let trigger = page
                .as_ref()
                .filter(|e| e.key.page_match() == build.entry.page_match())
                .map(|e| e.id.clone());
            let page = page.filter(|_| trigger.is_none());
            let owner = if page.is_none() {
                *named.entry(key).or_insert(i)
            } else {
                i
            };
            menus.groups.entry(owner).or_insert_with(|| Menu {
                label: clip(&crate::clean_text(label), TITLE_BYTES),
                page,
                index: None,
                trigger,
                source: if nav {
                    Source::PrimaryNav
                } else {
                    Source::LinkCluster
                },
                items: Vec::new(),
                editions: Editions::default(),
            });
            if let Some(links) = panel {
                for link in links {
                    let label = choose_link_label(&link.name, &link.text, None);
                    if card::skip_link(&link.url, &[&link.name, label], doc.page()) {
                        continue;
                    }
                    targets.push((
                        i,
                        Some(owner),
                        ObservedLink::new(build, &link.url, label, link.action),
                    ));
                }
            }
            scopes
                .entry(scope)
                .and_modify(|previous| {
                    if *previous != Some(owner) {
                        *previous = None;
                    }
                })
                .or_insert(Some(owner));
        }
        let mut footer_links = Vec::new();
        let mut seen = BTreeSet::new();
        let mut flat_scopes = vec![None; nodes.len()];
        let mut flat_headings = BTreeMap::new();
        for (i, n) in nodes.iter().enumerate() {
            let region = regions[i];
            let nav = region == Region::Navigation;
            if nav {
                if let Some(p) = n.parent_index() {
                    if matches!(n.role, Role::Text | Role::ListItem)
                        && (sibling[p] == Some(i) || flat_headings.contains_key(&p))
                        && !n.text.is_empty()
                        && visible(nodes, i)
                    {
                        if let Some(&owner) = named.get(&normal(&n.text)) {
                            flat_headings.insert(p, owner);
                        }
                    }
                    flat_scopes[i] = flat_headings.get(&p).copied().or(flat_scopes[p]);
                }
            }
        }
        for (_, _, link) in &targets {
            if link.action {
                actions.extend(link.route(build));
            }
        }
        for (i, panel_owner, link) in targets {
            let region = regions[i];
            let nav = region == Region::Navigation;
            let owner = panel_owner.or_else(|| {
                ancestors(nodes, i)
                    .filter_map(|a| scopes.get(&a).copied().flatten())
                    .last()
                    .filter(|&o| o != i)
            });
            let action = link.action
                || link
                    .route(build)
                    .is_some_and(|route| actions.contains(&route));
            let url = link.url;
            let label = link.label;
            if !action {
                if let Some(menu) = owner.and_then(|o| menus.groups.get_mut(&o)) {
                    menu.editions
                        .observe(url, &build.localized, doc.page(), &alternates);
                }
            }
            let Some(mut entry) = link.entry else {
                continue;
            };
            if self_link(&entry) || card::placeholder(label) || profile(&entry) {
                continue;
            }
            menus.excluded.remove(&entry.id);
            if action {
                if panel_owner.is_some() && link.action {
                    entry.tier = Tier::Action;
                    menus.actions.push(entry);
                }
                continue;
            }
            if region == Region::Footer {
                if entry.footer_utility() {
                    continue;
                }
                let group = ancestors(nodes, i).find_map(|a| footer.get(&a));
                if let Some(owner) = group.and_then(|g| {
                    named.get(g).or_else(|| {
                        g.split_once(" and ")
                            .and_then(|(first, _)| named.get(first))
                    })
                }) {
                    footer_links.push((*owner, entry));
                }
                continue;
            }
            if panel_owner.is_none() && !nav && !clusters.contains(&entry.id) {
                continue;
            }
            if panel_owner.is_none() {
                menus.order.entry(entry.id.clone()).or_insert(i);
            }
            if let Some(owner) = owner {
                if seen.insert((owner, entry.id.clone())) {
                    if let Some(menu) = menus.groups.get_mut(&owner) {
                        menu.add_item(entry);
                    }
                }
            } else if let Some(menu) = group_page(&normal(label))
                .and_then(|g| named.get(g))
                .and_then(|o| menus.groups.get_mut(o))
            {
                menu.index.get_or_insert(entry);
            } else if nav {
                standalone.push((i, entry));
            }
        }
        let mut by_label = BTreeMap::new();
        let mut joined = BTreeMap::<usize, Menu>::new();
        let mut owners = BTreeMap::new();
        for (i, menu) in menus.groups {
            let key = normal(&menu.label);
            let owner = *by_label.entry(key).or_insert(i);
            owners.insert(i, owner);
            if let Some(kept) = joined.get_mut(&owner) {
                if kept.page.is_none() {
                    kept.page = menu.page;
                }
                if kept.index.is_none() {
                    kept.index = menu.index;
                }
                kept.items.extend(menu.items);
                kept.editions.merge(menu.editions);
            } else {
                joined.insert(owner, menu);
            }
        }
        menus.groups = joined;
        let header_items: BTreeSet<_> = menus
            .groups
            .iter()
            .flat_map(|(&i, menu)| menu.items.iter().map(move |e| (i, e.card_route())))
            .collect();
        let empty: BTreeSet<_> = menus
            .groups
            .iter()
            .filter(|(_, m)| m.editions.targets.is_empty())
            .map(|(&i, _)| i)
            .collect();
        for (owner, mut entry) in footer_links {
            if let Some(menu) = owners
                .get(&owner)
                .filter(|o| empty.contains(o))
                .and_then(|o| menus.groups.get_mut(o))
            {
                menu.editions.observe(
                    entry.key.as_url(),
                    &build.localized,
                    doc.page(),
                    &alternates,
                );
                entry.sources = vec![Source::Footer, Source::MenuGroup];
                menu.add_item(entry);
            }
        }
        for (i, entry) in standalone {
            if flat_scopes[i]
                .map(|owner| {
                    ancestors(nodes, owner)
                        .filter_map(|a| scopes.get(&a).copied().flatten())
                        .last()
                        .unwrap_or(owner)
                })
                .and_then(|owner| owners.get(&owner))
                .is_some_and(|owner| header_items.contains(&(*owner, entry.card_route())))
            {
                continue;
            }
            let control = owners
                .get(&i)
                .and_then(|o| menus.groups.get(o))
                .is_some_and(|m| !m.items.is_empty() || m.editions.is_picker());
            if !control {
                menus.direct.entry(entry.card_route()).or_insert((i, entry));
            }
        }
        menus.groups.retain(|_, m| {
            if m.editions.is_picker() {
                menus.evidence.languages.insert(normal(&m.label));
                menus
                    .evidence
                    .editions
                    .extend(m.items.iter().map(|e| (e.card_page(), e.locator.clone())));
                menus
                    .excluded
                    .extend(m.items.iter().chain(&m.page).map(|e| e.id.clone()));
                false
            } else {
                true
            }
        });
        let landings: BTreeMap<_, _> = build
            .sections
            .iter()
            .filter(|e| e.id != EntryId::new(None, &e.key, &e.locator))
            .map(|e| (&e.id, e.card_route()))
            .collect();
        let route = |e: &Entry| {
            landings
                .get(&e.id)
                .cloned()
                .unwrap_or_else(|| e.card_route())
        };
        let mut claimed = BTreeMap::new();
        let mut ambiguous = BTreeSet::new();
        for (owner, menu) in menus.groups.iter().filter(|(_, m)| !m.items.is_empty()) {
            for page in menu.page.iter().chain(&menu.index) {
                let destination = route(page);
                if claimed
                    .insert(destination.clone(), owner)
                    .is_some_and(|previous| previous != owner)
                {
                    ambiguous.insert(destination);
                }
            }
        }
        for menu in menus.groups.values_mut() {
            let items: BTreeSet<_> = menu.items.iter().map(&route).collect();
            menu.page = menu
                .page
                .take()
                .into_iter()
                .chain(menu.index.take())
                .find(|p| {
                    let target = route(p);
                    !actions.contains(&target)
                        && !actions.contains(&p.card_route())
                        && !menus.direct.contains_key(&p.card_route())
                        && !ambiguous.contains(&target)
                        && !items.contains(&target)
                });
            let mut distinct = BTreeSet::new();
            menu.items.retain(|e| {
                distinct.insert(e.card_route()) && !menus.direct.contains_key(&e.card_route())
            });
            if !menu.items.is_empty() {
                menus.evidence.pages.insert(
                    normal(&menu.label),
                    menu.page.as_ref().map(|e| e.id.clone()),
                );
                menus
                    .members
                    .extend(menu.items.iter().map(|e| e.id.clone()));
            }
        }
        let triggers: BTreeSet<_> = menus
            .groups
            .values()
            .filter(|m| !m.items.is_empty())
            .filter_map(|m| m.trigger.as_ref())
            .collect();
        menus.direct.retain(|_, (_, e)| {
            !(e.utility()
                || card::sentence(&e.label)
                || build.link_is_edition(e)
                || menus.excluded.contains(&e.id)
                || triggers.contains(&e.id))
        });
        menus.evidence.direct.extend(
            menus
                .direct
                .values()
                .map(|(_, e)| (e.id.clone(), e.card_route())),
        );
        for e in build
            .sections
            .iter()
            .flat_map(|e| std::iter::once(e).chain(&e.children))
        {
            if menus
                .members
                .contains(&EntryId::new(None, &e.key, &e.locator))
            {
                menus.members.insert(e.id.clone());
            }
        }
        menus
    }

    pub(super) fn take_evidence(&mut self) -> Evidence {
        std::mem::take(&mut self.evidence)
    }
    pub(super) fn take_actions(&mut self) -> Vec<Entry> {
        std::mem::take(&mut self.actions)
    }
}

impl Menus {
    pub(super) fn apply(mut self, build: &mut Build<'_>) {
        let mut direct_routes = BTreeSet::new();
        let member_routes: BTreeSet<_> = self
            .groups
            .values()
            .flat_map(|m| m.items.iter().map(Entry::card_route))
            .collect();
        build.sections.retain(|e| {
            !self
                .excluded
                .contains(&EntryId::new(None, &e.key, &e.locator))
        });
        for e in &mut build.sections {
            e.children.retain(|c| {
                !self
                    .excluded
                    .contains(&EntryId::new(None, &c.key, &c.locator))
            });
        }
        let available: BTreeMap<_, _> = build
            .sections
            .iter()
            .enumerate()
            .map(|(i, e)| (e.card_route(), i))
            .collect();
        let discovered: BTreeMap<_, _> = build
            .sections
            .iter()
            .enumerate()
            .map(|(i, e)| (e.id.clone(), i))
            .collect();
        let mut named_rows: BTreeSet<_> = build.sections.iter().map(|e| normal(&e.label)).collect();
        for (route, (position, mut entry)) in self.direct {
            if let Some(i) = discovered
                .get(&entry.id)
                .or_else(|| available.get(&route))
                .copied()
            {
                let e = &mut build.sections[i];
                if e.tier != Tier::Action {
                    e.add_sources([Source::PrimaryNav]);
                    if !e.path_name {
                        e.label = entry.label;
                        e.role = entry.role;
                    }
                    direct_routes.insert(route);
                }
                continue;
            }
            if named_rows.insert(normal(&entry.label)) {
                entry.sources = vec![Source::PrimaryNav];
                entry.tier = Tier::Primary;
                self.order.insert(entry.id.clone(), position);
                direct_routes.insert(route);
                build.sections.push(entry);
            }
        }
        let mut old: BTreeMap<_, _> = std::mem::take(&mut build.sections)
            .into_iter()
            .enumerate()
            .map(|(position, e)| (e.id.clone(), (position, e)))
            .collect();
        let routes: BTreeMap<_, _> = old
            .iter()
            .map(|(id, (_, e))| (EntryId::new(None, &e.key, &e.locator), id.clone()))
            .collect();
        let prior_id = |id: &EntryId| routes.get(id).cloned().unwrap_or_else(|| id.clone());
        let localized: BTreeMap<_, _> = old
            .values()
            .map(|(_, e)| (e.card_route(), e.id.clone()))
            .collect();
        let mut rows = Vec::new();
        let mut taken: BTreeSet<_> = self
            .groups
            .values()
            .filter(|m| !m.items.is_empty())
            .filter_map(|m| m.page.as_ref().map(|e| e.id.clone()))
            .collect();
        for (position, menu) in self.groups {
            if menu.items.is_empty() {
                continue;
            }
            if let Some(id) = &menu.trigger {
                old.remove(&prior_id(id));
            }
            let Some(mut parent) = menu.page.or_else(|| {
                let mut e = build.entry_for(build.entry.as_url(), &menu.label, menu.source)?;
                e.id = EntryId::menu(&build.entry, &menu.label);
                e.menu_only = true;
                Some(e)
            }) else {
                continue;
            };
            let mut extra = Vec::new();
            let mut prior_more = 0;
            if let Some((_, existing)) = old.remove(&prior_id(&parent.id)) {
                parent = existing;
                extra = std::mem::take(&mut parent.children);
                prior_more = std::mem::take(&mut parent.more);
            }
            parent.label = menu.label;
            parent.tier = Tier::Primary;
            parent.path_name = false;
            parent.role = label_role(&parent.label);
            parent.add_sources([menu.source]);
            self.order.insert(parent.id.clone(), position);
            let mut items = menu.items;
            if items.len() > MAX_CHILDREN {
                let mut ranked: Vec<_> = items.iter().collect();
                ranked.sort_by_key(|e| {
                    (
                        build.requested.and_then(|r| r.rank(e)).is_none(),
                        e.role.is_none(),
                    )
                });
                let selected: BTreeSet<_> = ranked
                    .into_iter()
                    .take(MAX_CHILDREN)
                    .map(|e| e.id.clone())
                    .collect();
                parent.more += (items.len() - MAX_CHILDREN) as u32;
                items.retain(|e| {
                    if selected.contains(&e.id) {
                        true
                    } else {
                        old.remove(&prior_id(&e.id));
                        false
                    }
                });
            }
            for mut item in items {
                let original = item.id.clone();
                let prior = routes
                    .get(&original)
                    .or_else(|| localized.get(&item.card_route()))
                    .cloned()
                    .unwrap_or_else(|| original.clone());
                if let Some((_, e)) = old.remove(&prior) {
                    if build
                        .llms_headings
                        .get(&e.id)
                        .is_some_and(|h| *h == normal(&parent.label))
                    {
                        extra.extend(
                            e.children
                                .into_iter()
                                .filter(|c| c.sources.contains(&Source::Llms)),
                        );
                    }
                    if prior == original || routes.contains_key(&original) {
                        item.key = e.key;
                        item.localized = e.localized;
                        item.off_site = e.off_site;
                        item.locator = e.locator;
                    }
                    item.add_sources(e.sources);
                }
                let existing = extra
                    .iter()
                    .find(|c| c.card_route() == item.card_route())
                    .map(|c| c.id.clone());
                let discovered = taken
                    .insert(prior.clone())
                    .then(|| existing.unwrap_or(prior));
                parent.insert_child(item, build.requested, discovered);
            }
            for mut child in extra {
                let route = EntryId::new(None, &child.key, &child.locator);
                if self.members.contains(&route)
                    || direct_routes.contains(&child.card_route())
                    || child.sources.first() == Some(&Source::Footer)
                {
                    continue;
                }
                let previous = prior_id(&route);
                let secondary = child.sources.contains(&Source::Llms)
                    && old
                        .get(&previous)
                        .is_some_and(|(_, e)| e.tier == Tier::Secondary && !e.navigation_row());
                let discovered = if secondary {
                    old.remove(&previous).map(|(_, e)| {
                        child.add_sources(e.sources);
                        e.id
                    })
                } else {
                    None
                };
                parent.insert_child(child, build.requested, discovered);
            }
            parent.more = parent.more.max(prior_more);
            rows.push((position, parent));
        }
        let labels: BTreeMap<_, _> = rows
            .iter()
            .enumerate()
            .map(|(i, (_, e))| (normal(&e.label), i))
            .collect();
        let mut original: Vec<_> = old.into_values().collect();
        original.sort_by_key(|(pos, _)| *pos);
        for (_, mut e) in original {
            if let Some(&i) = labels
                .get(&normal(&e.label))
                .filter(|_| e.tier != Tier::Action && !direct_routes.contains(&e.card_route()))
            {
                let group = &mut rows[i].1;
                if build
                    .llms_headings
                    .get(&e.id)
                    .is_some_and(|name| *name == normal(&group.label))
                {
                    for child in e.children.into_iter().filter(|c| {
                        c.sources.contains(&Source::Llms)
                            && !direct_routes.contains(&c.card_route())
                    }) {
                        group.child(child, build.requested);
                    }
                }
                continue;
            }
            if member_routes.contains(&e.card_route()) {
                continue;
            }
            // Menu destinations can also occur in result, footer or index sections.
            e.children.retain(|c| {
                !taken.contains(&EntryId::new(None, &c.key, &c.locator))
                    && !direct_routes.contains(&c.card_route())
            });
            e.menu_item |= self.members.contains(&e.id);
            let order = self
                .order
                .get(&e.id)
                .or_else(|| self.order.get(&EntryId::new(None, &e.key, &e.locator)))
                .copied()
                .unwrap_or(usize::MAX);
            rows.push((order, e));
        }
        rows.sort_by_key(|(i, _)| *i);
        build.sections = rows.into_iter().map(|(_, e)| e).collect();
    }
}
