use super::*;
use crate::PageMatch;

pub const MAX_PRIMARY_LABEL_CHARS: usize = 48;
pub const MAX_HOST_BYTES: usize = 253;
pub const MAX_PATH_CHARS: usize = 128;
const SENTENCE_WORDS: usize = 7;
const MIN_DIRECTORY_ITEMS: usize = 3;

fn locale(segment: &str) -> bool {
    let (language, region) = segment
        .split_once('-')
        .map_or((segment, None), |(l, r)| (l, Some(r)));
    language.len() == 2
        && language.bytes().all(|c| c.is_ascii_alphabetic())
        && region.is_none_or(|r| r.len() == 2 && r.bytes().all(|c| c.is_ascii_alphabetic()))
}

impl Entry {
    pub fn card_page(&self) -> PageMatch {
        localized_key(&self.key)
    }
}

fn localized_key(key: &PageKey) -> PageMatch {
    let mut url = key.as_url().clone();
    if let Some((first, rest)) = url.path().trim_matches('/').split_once('/') {
        if locale(first) {
            let path = format!("/{rest}");
            url.set_path(&path);
        }
    }
    PageKey::from_url(&url).map_or_else(|_| key.page_match(), |k| k.page_match())
}

pub(super) fn placeholder(label: &str) -> bool {
    matches!(
        normal(label).as_str(),
        "" | "other"
            | "more"
            | "optional"
            | "menu"
            | "main navigation menu"
            | "open sidebar"
            | "learn more"
    )
}

pub(super) fn sentence(label: &str) -> bool {
    label.chars().count() > MAX_PRIMARY_LABEL_CHARS
        || label.contains(". ")
        || label.split_whitespace().count() >= SENTENCE_WORDS
}

fn version_badge(label: &str) -> bool {
    let Some(version) = label.strip_prefix('v') else {
        return false;
    };
    version.contains('.')
        && version
            .split('.')
            .all(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
}

fn path_name(url: &Url) -> Option<String> {
    let slug = url.path_segments()?.rfind(|s| !s.is_empty())?;
    let slug = super::decode_fragment(slug);
    let name = slug
        .trim_end_matches(".html")
        .trim_end_matches(".md")
        .replace(['-', '_'], " ");
    (!name.is_empty()
        && !sentence(&name)
        && !placeholder(&name)
        && name.chars().any(char::is_alphabetic)
        && name.chars().all(|c| c.is_alphanumeric() || c == ' '))
    .then(|| capitalized(&name))
    .filter(|s| !sentence(s))
}

pub(super) fn capitalized(name: &str) -> String {
    let mut chars = name.chars();
    match chars.next() {
        Some(c) => c.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

pub(super) fn action(entry: &Entry) -> bool {
    let url = entry.key.as_url();
    let path = url.path().trim_matches('/').to_ascii_lowercase();
    let last = path.rsplit('/').next().unwrap_or_default();
    let label = normal(&entry.label);
    if matches!(
        last,
        "login"
            | "signin"
            | "sign-in"
            | "sign_in"
            | "register"
            | "signup"
            | "sign-up"
            | "sign_up"
            | "contact-sales"
            | "request-demo"
            | "book-demo"
    ) || path.trim_end_matches(".md").ends_with("contact/sales")
        || last.ends_with("enterprise-sales")
        || last.ends_with(".dmg")
        || last.ends_with(".msi")
        || last.ends_with(".exe")
    {
        return true;
    }
    if path.starts_with("downloads/")
        && label.contains("download")
        && label.split_whitespace().next_back() == Some(last)
    {
        return true;
    }
    if matches!(last, "download" | "downloads" | "download.html") {
        return false;
    }
    entry.tier == Tier::Action
        || (last == "refer" && label.starts_with("refer "))
        || (matches!(last, "shell" | "app") && label.starts_with("launch "))
        || (last == "get-started" && label == "get started" && !path.contains('/'))
        || matches!(
            label.as_str(),
            "sign in"
                | "log in or sign up"
                | "log in"
                | "login"
                | "sign up"
                | "contact sales"
                | "request a demo"
                | "get a demo"
                | "start free trial"
                | "book a demo"
                | "get started download"
                | "new chat"
                | "donate"
                | "under attack"
        )
}

pub(super) fn self_link(entry: &Entry, root: &PageKey, landing: Option<&Url>) -> bool {
    if entry.locator != Locator::default() {
        return false;
    }
    let url = entry.key.as_url();
    if localized_key(&entry.key) == root.page_match()
        || landing.is_some_and(|u| crate::key::same_page(u, url))
    {
        return true;
    }
    let host = |u: &Url| u.domain().map(|h| h.trim_start_matches("www.").to_owned());
    if host(url) != host(root.as_url()) || url.query().is_some() {
        return false;
    }
    let named_home = normal(&entry.label)
        .split_whitespace()
        .any(|w| matches!(w, "home" | "homepage"));
    let brand = host(url).is_some_and(|h| {
        h.split('.')
            .next()
            .is_some_and(|s| s.eq_ignore_ascii_case(&entry.label))
    });
    match url.path().trim_matches('/') {
        "index.html" | "homepage" => true,
        "home" => brand || named_home && !entry.label.eq_ignore_ascii_case("home"),
        "product" => named_home,
        path if locale(path) => named_home,
        _ => false,
    }
}

impl Build<'_> {
    fn link_name(&self, entry: &Entry) -> Option<&LinkName> {
        self.names.get(&entry.id).or_else(|| {
            self.names
                .get(&EntryId::new(None, &entry.key, &entry.locator))
        })
    }
    fn apply_link_name(&self, entry: &mut Entry) {
        let is_action = action(entry);
        if let Some(name) = self.link_name(entry) {
            if !name.text.is_empty() {
                entry.label.clone_from(&name.text);
                if entry.off_site
                    && normal(&entry.label).starts_with("open on ")
                    && entry.role.is_some()
                {
                    if let Some(name) = path_name(entry.key.as_url()) {
                        entry.path_name = true;
                        entry.label = name;
                    }
                }
            }
            if name.action {
                entry.tier = Tier::Action;
            }
        }
        if is_action {
            entry.tier = Tier::Action;
        }
    }

    pub(super) fn edit_card(&mut self, landing: Option<&Url>) {
        let mut candidates = Vec::new();
        let entries = std::mem::take(&mut self.sections);
        let locale_paths: BTreeSet<_> = entries
            .iter()
            .flat_map(|e| std::iter::once(e).chain(&e.children))
            .filter_map(|e| {
                let u = e.key.as_url();
                let path = u.path().trim_matches('/');
                (locale(path)
                    && u.domain().map(|h| h.trim_start_matches("www."))
                        == Some(self.key.host.as_str())
                    && !e.label.eq_ignore_ascii_case(path))
                .then_some(path.to_owned())
            })
            .collect();
        let home = |e: &Entry| {
            self_link(e, &self.entry, landing)
                || (locale_paths.len() > 1
                    && e.locator == Locator::default()
                    && e.key.as_url().query().is_none()
                    && e.key
                        .as_url()
                        .domain()
                        .map(|h| h.trim_start_matches("www."))
                        == Some(self.key.host.as_str())
                    && locale_paths.contains(e.key.as_url().path().trim_matches('/')))
        };
        let originals: std::collections::HashMap<_, _> = entries
            .iter()
            .map(|e| ((e.key.page_match(), e.locator.clone()), e.id.clone()))
            .collect();
        for mut entry in entries {
            self.apply_link_name(&mut entry);
            for child in &mut entry.children {
                self.apply_link_name(child);
            }
            let remove = !entry.menu_only && (home(&entry) || placeholder(&entry.label));
            let keep_members = !placeholder(&entry.label) || !entry.sources.contains(&Source::Llms);
            for mut child in entry
                .children
                .extract_if(.., |c| remove || c.tier == Tier::Action)
            {
                if (keep_members || child.tier == Tier::Action)
                    && !placeholder(&child.label)
                    && !home(&child)
                {
                    if let Some(id) =
                        originals.get(&(child.key.page_match(), child.locator.clone()))
                    {
                        child.id = id.clone();
                    }
                    candidates.push(child);
                }
            }
            if !remove {
                candidates.push(entry);
            }
        }
        // Prefer the site's navigation over a second representation in a list or result.
        candidates.sort_by_key(|e| (!e.navigation_row(), e.tier));
        let roots: BTreeSet<_> = candidates
            .iter()
            .filter(|e| {
                (e.key.as_url().path() == "/" || self.link_name(e).is_some_and(|n| n.host_root))
                    && e.locator == Locator::default()
            })
            .map(|e| e.key.as_url().host_str().map(str::to_owned))
            .collect();
        let mut destinations = std::collections::HashMap::new();
        let mut labels = BTreeMap::new();
        let mut editions = BTreeMap::new();
        for mut e in candidates {
            if action(&e) {
                e.tier = Tier::Action;
            } else if e.tier == Tier::Primary
                && ((e.key.as_url().host_str() != self.entry.as_url().host_str()
                    && roots.contains(&e.key.as_url().host_str().map(str::to_owned))
                    && e.key.as_url().path().trim_matches('/').contains('/'))
                    || e.key.as_url().path().contains("/news/") && e.label.ends_with('!'))
            {
                e.tier = Tier::Secondary;
            } else if e.tier == Tier::Primary && (sentence(&e.label) || version_badge(&e.label)) {
                if let Some(name) = path_name(e.key.as_url()) {
                    e.path_name = true;
                    e.label = name;
                } else {
                    e.tier = Tier::Secondary;
                }
            }
            let destination = (
                localized_key(&e.key),
                e.locator.clone(),
                e.menu_only.then(|| e.id.clone()),
            );
            let label = crate::flatten(&e.label).to_lowercase();
            let edition = self
                .link_name(&e)
                .filter(|n| n.language_host)
                .and_then(|_| e.key.as_url().domain()?.split_once('.'))
                .map(|(_, host)| {
                    (
                        host.to_owned(),
                        e.key.as_url()[url::Position::BeforePath..].to_owned(),
                        e.locator.fragment(),
                    )
                });
            if edition.is_some() {
                e.tier = Tier::Secondary;
            }
            let existing = destinations
                .get(&destination)
                .or_else(|| edition.as_ref().and_then(|key| editions.get(key)))
                .or_else(|| labels.get(&label))
                .copied();
            if let Some(i) = existing {
                let kept: &mut Entry = &mut self.place.sections[i];
                let tier = kept.tier;
                kept.add_sources(e.sources.iter().copied());
                kept.tier = if tier == Tier::Action || e.tier == Tier::Action {
                    Tier::Action
                } else {
                    tier.min(e.tier)
                };
                let children = std::mem::take(&mut e.children);
                if destination
                    != (
                        localized_key(&kept.key),
                        kept.locator.clone(),
                        kept.menu_only.then(|| kept.id.clone()),
                    )
                {
                    kept.child(e, self.requested);
                }
                for child in children {
                    kept.child(child, self.requested);
                }
                destinations.insert(destination, i);
                labels.insert(label, i);
                if let Some(key) = edition {
                    editions.insert(key, i);
                }
            } else {
                let index = self.sections.len();
                destinations.insert(destination, index);
                labels.insert(label, index);
                if let Some(key) = edition {
                    editions.insert(key, index);
                }
                self.sections.push(e);
            }
        }
        self.prefer_observed_directories();
    }
    pub(super) fn fill_from_footer(&mut self, saw_nav: bool) {
        if saw_nav
            && self
                .sections
                .iter()
                .filter(|e| e.tier == Tier::Primary && e.navigation_row() && !e.utility())
                .count()
                < MIN_NAV_ROWS
        {
            for e in &mut self.sections {
                if e.tier == Tier::Secondary
                    && e.sources.contains(&Source::Footer)
                    && !e.utility()
                    && !sentence(&e.label)
                    && e.role.is_some()
                    && section_role("", e.key.as_url()) == e.role
                {
                    e.tier = Tier::Primary;
                }
            }
        }
    }
    fn prefer_observed_directories(&mut self) {
        let mut children = BTreeMap::<_, Vec<usize>>::new();
        for (i, e) in self.sections.iter().enumerate().filter(|(_, e)| {
            e.tier == Tier::Primary
                && !e
                    .children
                    .iter()
                    .any(|c| c.sources.contains(&Source::MenuGroup))
        }) {
            let url = e.key.as_url();
            if let Some((parent, _)) = url.path().trim_end_matches('/').rsplit_once('/') {
                if !parent.is_empty() {
                    children
                        .entry((url.origin().ascii_serialization(), parent.to_owned()))
                        .or_default()
                        .push(i);
                }
            }
        }
        let parents: BTreeMap<_, _> = self
            .sections
            .iter()
            .enumerate()
            .filter(|(_, e)| {
                e.tier != Tier::Action
                    && e.locator == Locator::default()
                    && e.key.as_url().query().is_none()
            })
            .map(|(i, e)| {
                (
                    (
                        e.key.as_url().origin().ascii_serialization(),
                        e.key.as_url().path().trim_end_matches('/').to_owned(),
                    ),
                    i,
                )
            })
            .collect();
        for (path, indices) in children {
            let Some(&i) = parents.get(&path) else {
                continue;
            };
            let parent = &mut self.sections[i];
            let release_listing = matches!(
                parent.role,
                Some(SectionRole::Changelog | SectionRole::Releases)
            );
            if release_listing
                || (path.1.trim_matches('/').contains('/') && indices.len() >= MIN_DIRECTORY_ITEMS)
            {
                if let Some(name) = path_name(parent.key.as_url()) {
                    if parent.label.starts_with(char::is_numeric) || sentence(&parent.label) {
                        parent.path_name = true;
                        parent.label = name;
                    }
                    parent.tier = Tier::Primary;
                    for i in indices {
                        let child = &mut self.sections[i];
                        if !release_listing || !child.sources.contains(&Source::Footer) {
                            child.tier = Tier::Secondary;
                        }
                    }
                }
            }
        }
    }
}

pub(super) fn child_self(child: &Entry, parent: &Entry) -> bool {
    !parent.menu_only
        && child.locator == parent.locator
        && localized_key(&child.key) == localized_key(&parent.key)
}

fn scheme_like(segment: &str) -> bool {
    segment.strip_suffix(':').is_some_and(|scheme| {
        scheme.starts_with(|c: char| c.is_ascii_alphabetic())
            && scheme
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
    })
}

pub fn display_path(url: &Url) -> String {
    let mut out = String::new();
    for segment in url.path().split('/').filter(|s| !s.is_empty()) {
        if scheme_like(segment) {
            break;
        }
        out.push('/');
        out.push_str(segment);
    }
    if out.is_empty() {
        out.push('/');
    }
    match out.char_indices().nth(MAX_PATH_CHARS) {
        Some((cut, _)) => out[..cut].to_owned(),
        None => out,
    }
}

pub(super) fn same_display(left: &Entry, right: &Entry) -> bool {
    if left.key.as_url().domain() != right.key.as_url().domain() {
        return false;
    }
    let name = |e: &Entry| {
        let label = crate::flatten(&e.label).to_lowercase();
        if label.contains("://")
            || label
                .split_whitespace()
                .any(|w| w.starts_with("http:") || w.starts_with("https:") || w.starts_with("www."))
        {
            display_path(e.key.as_url()).to_lowercase()
        } else {
            label
        }
    };
    name(left) == name(right) && display_path(left.key.as_url()) == display_path(right.key.as_url())
}

pub fn display_hostname(url: &Url) -> Option<&str> {
    let host = url.domain()?.trim_end_matches('.');
    (!host.is_empty()
        && host.len() <= MAX_HOST_BYTES
        && host
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-')))
    .then_some(host)
}

impl Entry {
    pub fn navigation_row(&self) -> bool {
        self.sources.iter().any(|s| {
            matches!(
                s,
                Source::PrimaryNav | Source::MenuGroup | Source::LinkCluster
            )
        })
    }
}

impl Build<'_> {
    pub(super) fn merge_supplements(&mut self) {
        let sparse = self
            .sections
            .iter()
            .filter(|e| e.tier == Tier::Primary && e.navigation_row() && !e.utility())
            .count()
            < MIN_NAV_ROWS;
        let mut roles = BTreeMap::new();
        let mut names = BTreeMap::new();
        let mut docs = None;
        let mut merged = BTreeMap::new();
        for (i, e) in self
            .sections
            .iter()
            .enumerate()
            .filter(|(_, e)| e.tier == Tier::Primary && e.navigation_row() && !e.utility())
        {
            let label = normal(&e.label);
            if matches!(label.as_str(), "docs" | "documentation") {
                if let Some(parent) = docs {
                    names.entry(label).or_insert(parent);
                    merged.insert(i, parent);
                    continue;
                } else {
                    docs = Some(i);
                }
            }
            names.entry(label).or_insert(i);
            if let Some(role) = e.role {
                roles.entry(role).or_insert(i);
            }
        }
        let headings = &self.llms_headings;
        for (i, e) in self.place.sections.iter_mut().enumerate() {
            if e.navigation_row()
                || e.tier == Tier::Action
                || e.utility()
                || !e
                    .sources
                    .iter()
                    .any(|s| matches!(s, Source::Llms | Source::Sitemap))
            {
                continue;
            }
            let heading = headings.get(&e.id);
            if let Some(parent) = heading.and_then(|name| names.get(name)) {
                merged.insert(i, *parent);
                continue;
            }
            let role = heading.map_or(e.role, |name| label_role(name));
            if let Some(role) = role {
                if let Some(&parent) = roles.get(&role) {
                    merged.insert(i, parent);
                } else if !sentence(&e.label)
                    && (e.tier == Tier::Primary
                        || e.sources.first() == Some(&Source::Sitemap)
                            && SECTION_WORDS
                                .iter()
                                .any(|(name, r)| *r == role && e.label.eq_ignore_ascii_case(name)))
                {
                    e.tier = Tier::Primary;
                    roles.insert(role, i);
                }
            } else if !sparse {
                e.tier = Tier::Secondary;
            }
        }
        if merged.is_empty() {
            return;
        }
        let mut rows: Vec<_> = std::mem::take(&mut self.sections)
            .into_iter()
            .map(Some)
            .collect();
        for (index, owner) in merged {
            let Some(mut entry) = rows[index].take() else {
                continue;
            };
            let Some(kept) = &mut rows[owner] else {
                continue;
            };
            let mut children = std::mem::take(&mut entry.children);
            kept.add_sources(entry.sources.iter().copied());
            let prior = entry.id.clone();
            if let Some(i) = children.iter().position(|c| child_self(c, &entry)) {
                kept.insert_child(children.remove(i), self.requested, Some(prior));
            } else if !entry.menu_only && !child_self(&entry, kept) {
                kept.insert_child(entry, self.requested, Some(prior));
            }
            for child in children {
                kept.child(child, self.requested);
            }
        }
        self.sections = rows.into_iter().flatten().collect();
    }
}
