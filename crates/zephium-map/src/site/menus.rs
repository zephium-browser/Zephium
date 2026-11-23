use super::*;
use crate::Node;

const MIN_MENU_ITEMS: usize = 2;

#[derive(Default)]
pub(super) struct Menus {
    members: BTreeSet<EntryId>,
    groups: BTreeMap<usize, Menu>,
    order: BTreeMap<EntryId, usize>,
}

#[derive(Clone, Copy, Default, PartialEq)]
enum Region {
    #[default]
    Other,
    Navigation,
    Footer,
    Excluded,
}

struct Menu {
    label: String,
    page: Option<Entry>,
    items: Vec<Entry>,
    trigger: Option<EntryId>,
    source: Source,
}

pub(super) fn link_label(n: &Node) -> &str {
    let prefix = n.text.get(..n.link_prefix_bytes).filter(|s| {
        !s.is_empty()
            && n.text[s.len()..].starts_with(char::is_whitespace)
            && s.chars().any(char::is_alphabetic)
            && !matches!(normal(s).as_str(), "new" | "beta")
    });
    [Some(n.name.trim()), Some(n.text.trim()), prefix]
        .into_iter()
        .flatten()
        .filter(|s| !s.is_empty())
        .min_by_key(|s| s.chars().count())
        .unwrap_or_default()
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
        let mut direct = BTreeSet::new();
        let mut first_nav = None;
        let clusters: BTreeSet<_> = build
            .sections
            .iter()
            .filter(|e| e.sources.contains(&Source::LinkCluster))
            .map(|e| e.id.clone())
            .collect();
        let mut following = vec![None; nodes.len()];
        let mut sibling = vec![None; nodes.len()];
        for (i, n) in nodes.iter().enumerate().rev() {
            if let Some(p) = n.parent_index() {
                following[i] = sibling[p].replace(i);
            }
        }
        let mut regions = Vec::with_capacity(nodes.len());
        for (i, n) in nodes.iter().enumerate() {
            let inherited = n.parent_index().map(|p| regions[p]).unwrap_or_default();
            let region = match (inherited, n.role) {
                (Region::Excluded, _)
                | (
                    _,
                    Role::Landmark {
                        landmark: Some(Landmark::Main | Landmark::Complementary),
                    },
                ) => Region::Excluded,
                (Region::Footer, _)
                | (
                    _,
                    Role::Landmark {
                        landmark: Some(Landmark::Contentinfo),
                    },
                ) => Region::Footer,
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
            let label = n.accessible_name().trim();
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
            let scope = match n.role {
                Role::Group | Role::List if !label.is_empty() => Some(i),
                Role::Button if doc.menu_links.contains_key(&i) => Some(i),
                Role::Button | Role::Heading { .. } => parent
                    .filter(|&p| matches!(nodes[p].role, Role::Group | Role::ListItem))
                    .or_else(|| {
                        following[i]
                            .filter(|&next| matches!(nodes[next].role, Role::Group | Role::List))
                    }),
                Role::Link => {
                    parent.filter(|&p| nodes[p].role == Role::ListItem && !scopes.contains_key(&p))
                }
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
                .and_then(|u| build.entry_for(u, label, Source::PrimaryNav))
                .filter(|e| {
                    !card::self_link(e, &build.entry, Some(doc.page())) && !card::action(e)
                });
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
                trigger,
                source: if nav {
                    Source::PrimaryNav
                } else {
                    Source::LinkCluster
                },
                items: Vec::new(),
            });
            if let Some(links) = doc.menu_links.get(&i) {
                if let Some(menu) = menus.groups.get_mut(&owner) {
                    for (url, label) in links {
                        if let Some(mut e) =
                            build.entry_for(url, label, Source::MenuGroup).filter(|e| {
                                !(card::action(e)
                                    || card::placeholder(label)
                                    || card::self_link(e, &build.entry, Some(doc.page()))
                                    || e.off_site && site_profile(url).is_some())
                            })
                        {
                            e.menu_item = true;
                            menu.items.push(e);
                        }
                    }
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
        for (i, n) in nodes.iter().enumerate() {
            let region = regions[i];
            let nav = region == Region::Navigation;
            let Some(url) = &n.destination else { continue };
            if region == Region::Excluded
                || region == Region::Other && clusters.is_empty()
                || !visible(nodes, i)
            {
                continue;
            }
            let label = link_label(n);
            if SiteKey::from_url(url).as_ref() != Some(&build.site) && site_profile(url).is_some() {
                continue;
            }
            let Some(mut entry) = build.entry_for(url, label, Source::MenuGroup) else {
                continue;
            };
            if n.marks.has(crate::doc::Marks::ACTION)
                || card::action(&entry)
                || card::self_link(&entry, &build.entry, Some(doc.page()))
                || card::placeholder(label)
            {
                continue;
            }
            if region == Region::Footer {
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
            if !nav && !clusters.contains(&entry.id) {
                continue;
            }
            menus.order.entry(entry.id.clone()).or_insert(i);
            let label = normal(label);
            let owner = ancestors(nodes, i)
                .filter_map(|a| scopes.get(&a).copied().flatten())
                .last();
            let all = group_page(&label).and_then(|g| named.get(g)).copied();
            if let Some(owner) = all {
                if let Some(menu) = menus.groups.get_mut(&owner) {
                    menu.page.get_or_insert(entry);
                }
            } else if let Some(owner) = owner.filter(|&o| o != i) {
                if let Some(menu) = menus.groups.get_mut(&owner) {
                    let group = normal(&menu.label);
                    if label == group
                        || label == "overview"
                            && entry
                                .key
                                .as_url()
                                .path_segments()
                                .and_then(|mut p| p.next_back())
                                .is_some_and(|s| normal(s) == group)
                    {
                        menu.page.get_or_insert(entry);
                        continue;
                    }
                }
                if seen.insert((owner, entry.id.clone())) {
                    entry.menu_item = true;
                    if let Some(menu) = menus.groups.get_mut(&owner) {
                        menu.items.push(entry);
                    }
                }
            } else {
                direct.insert(entry.id);
            }
        }
        let empty: BTreeSet<_> = menus
            .groups
            .iter()
            .filter(|(_, m)| m.items.is_empty())
            .map(|(&i, _)| i)
            .collect();
        for (owner, mut entry) in footer_links {
            if !empty.contains(&owner) {
                continue;
            }
            let Some(menu) = menus.groups.get_mut(&owner) else {
                continue;
            };
            if !direct.contains(&entry.id) && seen.insert((owner, entry.id.clone())) {
                entry.menu_item = true;
                entry.sources = vec![Source::Footer, Source::MenuGroup];
                menu.items.push(entry);
            }
        }
        let mut unique = BTreeMap::new();
        let mut by_label = BTreeMap::new();
        let mut joined = BTreeMap::<usize, Menu>::new();
        for (i, menu) in menus.groups {
            let identity = menu
                .page
                .as_ref()
                .map(|p| p.id.clone())
                .unwrap_or_else(|| EntryId::menu(&build.entry, &menu.label));
            let key = normal(&menu.label);
            let owner = *by_label
                .entry(key)
                .or_insert_with(|| *unique.entry(identity).or_insert(i));
            if let Some(kept) = joined.get_mut(&owner) {
                if kept.page.is_none() {
                    kept.page = menu.page;
                }
                kept.items.extend(menu.items);
            } else {
                joined.insert(owner, menu);
            }
        }
        menus.groups = joined;
        for menu in menus.groups.values_mut() {
            let mut distinct = BTreeSet::new();
            menu.items.retain(|e| {
                distinct.insert(e.id.clone())
                    && !direct.contains(&e.id)
                    && menu.page.as_ref().is_none_or(|p| p.id != e.id)
            });
            if menu.items.len() >= MIN_MENU_ITEMS {
                menus
                    .members
                    .extend(menu.items.iter().map(|e| e.id.clone()));
            }
        }
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
}

impl Menus {
    pub(super) fn apply(mut self, build: &mut Build<'_>) {
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
        let mut rows = Vec::new();
        let mut taken: BTreeSet<_> = self
            .groups
            .values()
            .filter(|m| !m.items.is_empty())
            .filter_map(|m| m.page.as_ref().map(|e| e.id.clone()))
            .collect();
        for (position, menu) in self.groups {
            if menu.items.is_empty() || menu.items.len() < MIN_MENU_ITEMS && menu.page.is_none() {
                // A lone link underneath a toggle keeps its own name and destination.
                if menu.page.is_none() {
                    if let Some(mut item) = menu.items.into_iter().next() {
                        if let Some((_, e)) = old.get_mut(&prior_id(&item.id)) {
                            e.label = item.label;
                            e.role = item.role;
                        } else {
                            item.menu_item = false;
                            item.add_sources([menu.source]);
                            rows.push((position, item));
                        }
                    }
                }
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
                if let Some((_, e)) = old.remove(&prior_id(&original)) {
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
                    item.key = e.key;
                    item.off_site = e.off_site;
                    item.locator = e.locator;
                    item.add_sources(e.sources);
                }
                let prior = extra
                    .iter()
                    .find(|c| c.key == item.key && c.locator == item.locator)
                    .map(|c| c.id.clone());
                let discovered = taken
                    .insert(original.clone())
                    .then(|| prior.unwrap_or_else(|| prior_id(&original)));
                parent.insert_child(item, build.requested, discovered);
            }
            for mut child in extra {
                let route = EntryId::new(None, &child.key, &child.locator);
                if self.members.contains(&route) {
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
                .filter(|_| e.tier != Tier::Action)
            {
                let group = &mut rows[i].1;
                // The old discovery pass can use a menu label on its first linked item.
                let children = std::mem::take(&mut e.children);
                for child in children {
                    group.child(child, build.requested);
                }
                if group.menu_only
                    && !self.members.contains(&e.id)
                    && !e.menu_only
                    && e.navigation_row()
                    && normal(e.key.as_url().path().trim_matches('/')) == normal(&e.label)
                {
                    group.key = e.key;
                    group.id = e.id;
                    group.locator = e.locator;
                    group.menu_only = false;
                    group.off_site = e.off_site;
                }
                continue;
            }
            // Menu destinations can also occur in result, footer or index sections.
            e.children
                .retain(|c| !taken.contains(&EntryId::new(None, &c.key, &c.locator)));
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
