use super::*;

impl Place {
    pub(super) fn navigation(&mut self, doc: &Doc, footer_pass: bool) {
        let nodes = doc.nodes();
        let first = nodes.iter().enumerate().position(|(i, n)| {
            n.role == Role::Landmark(Some(Landmark::Navigation))
                && visible(nodes, i)
                && !ancestors(nodes, i)
                    .any(|a| nodes[a].role == Role::Landmark(Some(Landmark::Complementary)))
        });
        let mut owners = BTreeMap::new();
        let mut group_labels = BTreeMap::new();
        let mut menu_labels = BTreeMap::new();
        for (i, n) in nodes.iter().enumerate() {
            if !visible(nodes, i) {
                continue;
            }
            if matches!(n.role, Role::Heading(_) | Role::Button | Role::Text)
                && n.parent.is_some()
                && ancestors(nodes, i)
                    .any(|a| nodes[a].role == Role::Landmark(Some(Landmark::Contentinfo)))
                && !ancestors(nodes, i).any(|a| nodes[a].role == Role::Link)
            {
                if let Some(p) = n.parent {
                    group_labels.insert(p, clip(&n.text, TITLE_BYTES));
                }
            }
            if matches!(n.role, Role::Button | Role::Heading(_))
                && !n.accessible_name().is_empty()
                && !ancestors(nodes, i).any(|a| nodes[a].role == Role::Link)
            {
                if let Some(p) = n
                    .parent
                    .filter(|&p| matches!(nodes[p].role, Role::Group | Role::ListItem))
                {
                    menu_labels.insert(p, n.accessible_name());
                }
            }
            let Some(url) = n.destination.as_ref() else {
                continue;
            };
            if !footer_pass {
                self.notes.links_seen += 1;
            }
            let nav = ancestors(nodes, i).any(|a| {
                Some(a) == first || nodes[a].role == Role::Landmark(Some(Landmark::Banner))
            });
            let footer = ancestors(nodes, i)
                .any(|a| nodes[a].role == Role::Landmark(Some(Landmark::Contentinfo)));
            if footer != footer_pass || !nav && !footer || self.profile(url) {
                continue;
            }
            let parent = ancestors(nodes, i).skip(1).find_map(|a| {
                owners
                    .get(&a)
                    .copied()
                    .filter(|_| n.parent != Some(a) || nodes[a].role == Role::ListItem)
            });
            if nav && !footer && parent.is_none() {
                if let Some((item, label)) = ancestors(nodes, i)
                    .take_while(|&a| !matches!(nodes[a].role, Role::Landmark(_)))
                    .find_map(|a| menu_labels.get(&a).map(|s| (a, *s)))
                {
                    if let Some(section) = self.entry_for(url, label, Source::PrimaryNav) {
                        if let Some(index) = self.section(section) {
                            owners.insert(item, index);
                            if let Some(child) =
                                self.entry_for(url, n.accessible_name(), Source::MenuGroup)
                            {
                                self.sections[index].child(child);
                            }
                        }
                    }
                    continue;
                }
            }
            let source = if footer {
                Source::Footer
            } else if parent.is_some() {
                Source::MenuGroup
            } else {
                Source::PrimaryNav
            };
            let Some(mut entry) = self.entry_for(url, n.accessible_name(), source) else {
                continue;
            };
            if footer {
                entry.group = ancestors(nodes, i).find_map(|a| group_labels.get(&a).cloned());
            }
            if let Some(index) = parent {
                let section: &mut Entry = &mut self.sections[index];
                section.child(entry);
                continue;
            }
            if let Some(index) = self.section(entry) {
                if nav {
                    if let Some(item) = ancestors(nodes, i)
                        .skip(1)
                        .take_while(|&a| !matches!(nodes[a].role, Role::Landmark(_)))
                        .find(|&a| matches!(nodes[a].role, Role::ListItem | Role::Group))
                    {
                        owners.insert(item, index);
                    }
                }
            }
        }
    }
    pub(super) fn section_page(&mut self, index: usize, doc: &Doc) {
        self.notes.section_pages += 1;
        self.notes.section_page_truncated |= doc.truncated();
        if challenge(doc)
            || SiteKey::from_url(doc.page()) != SiteKey::from_url(self.sections[index].key.as_url())
        {
            return;
        }
        let Ok(key) = PageKey::from_url(doc.page()) else {
            return;
        };
        self.sections[index].key = key.clone();
        self.sections[index].external = key.as_url().host_str() != self.entry.as_url().host_str();
        let nodes = doc.nodes();
        let mut heading = None;
        let mut releases = 0;
        let dated = matches!(
            self.sections[index].role,
            Some(SectionRole::Changelog | SectionRole::Releases | SectionRole::News)
        );
        let mut headings = Vec::new();
        for (i, n) in nodes.iter().enumerate() {
            if !visible(nodes, i) {
                continue;
            }
            if ancestors(nodes, i).any(|a| {
                matches!(
                    nodes[a].role,
                    Role::Landmark(Some(Landmark::Banner | Landmark::Contentinfo))
                )
            }) {
                continue;
            }
            let local = ancestors(nodes, i).find(|&a| {
                matches!(
                    nodes[a].role,
                    Role::Landmark(Some(Landmark::Navigation | Landmark::Complementary))
                )
            });
            if matches!(n.role, Role::Heading(_)) {
                if n.text.trim().is_empty() {
                    continue;
                }
                if let Some(group) = local {
                    heading = Some((group, clip(&n.text, TITLE_BYTES)));
                    continue;
                }
                let version = n.text.split_whitespace().any(|w| {
                    w.trim_start_matches('v')
                        .starts_with(|c: char| c.is_ascii_digit())
                        && (w.contains(['.', '-']) || w.trim_matches(',').len() == 4)
                });
                if dated && (!version || releases >= RELEASE_HEADINGS) {
                    continue;
                }
                let locator = n
                    .element_id
                    .as_deref()
                    .and_then(|s| LocatorText::new(s).ok())
                    .map(Locator::Element)
                    .or_else(|| (dated && version).then(|| quote(&n.text)).flatten());
                if let Some(locator) = locator {
                    let mut e = self.sections[index].leaf(Source::Heading, &n.text);
                    e.locator = locator;
                    if headings.len() < MAX_CHILDREN {
                        headings.push(e);
                    } else {
                        self.sections[index].more += 1;
                    }
                    releases += 1;
                }
            } else if let Some((url, group)) = n.destination.as_ref().zip(local) {
                // The section can have its own delegated documentation host.
                let child = self
                    .entry_for(url, n.accessible_name(), Source::SectionSidebar)
                    .or_else(|| {
                        if SiteKey::from_url(url)? != SiteKey::from_url(doc.page())? {
                            return None;
                        }
                        let mut e =
                            self.sections[index].leaf(Source::SectionSidebar, n.accessible_name());
                        e.key = PageKey::from_url(url).ok()?;
                        e.role = section_role(&e.label, url);
                        Some(e)
                    });
                if let Some(mut e) = child {
                    e.group = heading
                        .as_ref()
                        .filter(|(g, _)| *g == group)
                        .map(|(_, s)| s.clone());
                    self.sections[index].child(e);
                }
            }
        }
        for e in headings {
            self.sections[index].child(e);
        }
    }
}
