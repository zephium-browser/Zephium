use super::*;
pub const MIN_CLUSTER_LINKS: usize = 4;
pub const MAX_CLUSTER_LABEL_CHARS: usize = 48;

impl Build<'_> {
    pub(super) fn navigation(&mut self, doc: &Doc, footer_pass: bool) {
        let nodes = doc.nodes();
        let first = nodes.iter().enumerate().position(|(i, n)| {
            n.role
                == Role::Landmark {
                    landmark: Some(Landmark::Navigation),
                }
                && visible(nodes, i)
                && !ancestors(nodes, i).any(|a| {
                    matches!(
                        nodes[a].role,
                        Role::Landmark {
                            landmark: Some(Landmark::Complementary | Landmark::Contentinfo),
                        }
                    )
                })
        });
        let mut owners = BTreeMap::new();
        let mut group_labels = BTreeMap::new();
        let mut menu_labels = BTreeMap::new();
        for (i, n) in nodes.iter().enumerate() {
            if !visible(nodes, i) {
                continue;
            }
            if matches!(
                n.role,
                Role::Heading { level: _ } | Role::Button | Role::Text
            ) && n.parent_index().is_some()
                && ancestors(nodes, i).any(|a| {
                    nodes[a].role
                        == Role::Landmark {
                            landmark: Some(Landmark::Contentinfo),
                        }
                })
                && !ancestors(nodes, i).any(|a| nodes[a].role == Role::Link)
            {
                if let Some(p) = n.parent_index() {
                    group_labels.insert(p, clip(&n.text, TITLE_BYTES));
                }
            }
            if matches!(n.role, Role::Button | Role::Heading { level: _ })
                && !n.accessible_name().is_empty()
                && !ancestors(nodes, i).any(|a| nodes[a].role == Role::Link)
            {
                if let Some(p) = n
                    .parent_index()
                    .filter(|&p| matches!(nodes[p].role, Role::Group | Role::ListItem))
                {
                    menu_labels.insert(p, n.accessible_name());
                }
            }
            let Some(url) = n.destination.as_ref() else {
                continue;
            };
            if !footer_pass {
                self.stats.links_seen += 1;
            }
            let nav = ancestors(nodes, i).any(|a| {
                Some(a) == first
                    || nodes[a].role
                        == Role::Landmark {
                            landmark: Some(Landmark::Banner),
                        }
            });
            let footer = ancestors(nodes, i).any(|a| {
                nodes[a].role
                    == Role::Landmark {
                        landmark: Some(Landmark::Contentinfo),
                    }
            });
            if footer != footer_pass
                || !nav && !footer
                || SiteKey::from_url(url).as_ref() != Some(&self.site)
                    && self.profile(url, ProfileBasis::Navigation)
            {
                continue;
            }
            let parent = ancestors(nodes, i).skip(1).find_map(|a| {
                owners
                    .get(&a)
                    .copied()
                    .filter(|_| n.parent_index() != Some(a) || nodes[a].role == Role::ListItem)
            });
            if nav && !footer && parent.is_none() {
                if let Some((item, label)) = ancestors(nodes, i)
                    .take_while(|&a| !matches!(nodes[a].role, Role::Landmark { landmark: _ }))
                    .find_map(|a| menu_labels.get(&a).map(|s| (a, *s)))
                {
                    if let Some(section) = self.entry_for(url, label, Source::PrimaryNav) {
                        let index = self.section(section);
                        owners.insert(item, index);
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
                self.child(index, entry);
                continue;
            }
            let index = self.section(entry);
            if nav {
                if let Some(item) = ancestors(nodes, i)
                    .skip(1)
                    .take_while(|&a| !matches!(nodes[a].role, Role::Landmark { landmark: _ }))
                    .find(|&a| matches!(nodes[a].role, Role::ListItem | Role::Group))
                {
                    owners.insert(item, index);
                }
            }
        }
    }
    pub(super) fn link_clusters(&mut self, doc: &Doc) {
        let nodes = doc.nodes();
        if nodes.iter().enumerate().any(|(i, n)| {
            visible(nodes, i)
                && matches!(
                    n.role,
                    Role::Landmark {
                        landmark: Some(Landmark::Navigation | Landmark::Banner),
                    }
                )
                && !ancestors(nodes, i).any(|a| {
                    matches!(
                        nodes[a].role,
                        Role::Landmark {
                            landmark: Some(Landmark::Contentinfo),
                        }
                    )
                })
        }) {
            return;
        }
        let mut clusters: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
        let mut prose = BTreeSet::new();
        for (i, n) in nodes.iter().enumerate() {
            if !visible(nodes, i) {
                continue;
            }
            if let Some(url) = &n.destination {
                if n.accessible_name().is_empty()
                    || n.accessible_name().chars().count() > MAX_CLUSTER_LABEL_CHARS
                    || SiteKey::from_url(url).as_ref() != Some(&self.site)
                {
                    continue;
                }
                let mut parents = ancestors(nodes, i)
                    .skip(1)
                    .filter(|&p| nodes[p].role != Role::Text);
                if let Some(mut owner) = parents.next() {
                    if nodes[owner].role == Role::Paragraph
                        && nodes[owner].text.trim() == n.text.trim()
                    {
                        let Some(parent) = parents.next() else {
                            continue;
                        };
                        owner = parent;
                    }
                    if matches!(
                        nodes[owner].role,
                        Role::Cell | Role::CellHeader | Role::ListItem
                    ) {
                        let Some(container) = parents.next() else {
                            continue;
                        };
                        owner = container;
                    }
                    clusters.entry(owner).or_default().push(i);
                }
            } else if matches!(n.role, Role::Paragraph | Role::Text)
                && n.text.chars().count() > MAX_CLUSTER_LABEL_CHARS
                && !ancestors(nodes, i).any(|p| nodes[p].role == Role::Link)
            {
                prose.insert(i);
            }
        }
        let mut groups = Vec::new();
        for (owner, links) in clusters {
            if links.len() < MIN_CLUSTER_LINKS {
                continue;
            }
            let first = links[0];
            let last = links[links.len() - 1];
            if prose.range(first..=last).next().is_some() {
                continue;
            }
            if nodes[owner].role == Role::Paragraph {
                let labels: usize = links
                    .iter()
                    .map(|&i| {
                        nodes[i]
                            .text
                            .chars()
                            .filter(|c| c.is_alphanumeric())
                            .count()
                    })
                    .sum();
                if nodes[owner]
                    .text
                    .chars()
                    .filter(|c| c.is_alphanumeric())
                    .count()
                    > labels
                {
                    continue;
                }
                prose.remove(&owner);
            }
            groups.push((first, last, links));
        }
        groups.sort_by_key(|g| g.0);
        let top = groups
            .iter()
            .find(|g| prose.first().is_none_or(|p| g.1 < *p));
        let bottom = groups.last();
        for g in top
            .into_iter()
            .chain(bottom.filter(|g| top.is_none_or(|t| t.0 != g.0)))
        {
            for &i in &g.2 {
                let n = &nodes[i];
                if let Some(e) = n
                    .destination
                    .as_ref()
                    .and_then(|u| self.entry_for(u, n.accessible_name(), Source::LinkCluster))
                {
                    self.section(e);
                }
            }
        }
    }
    pub(super) fn section_page(&mut self, index: usize, doc: &Doc) {
        self.stats.section_pages += 1;
        self.stats.section_page_truncated |= doc.truncated();
        if challenge(doc)
            || SiteKey::from_url(doc.page()) != SiteKey::from_url(self.sections[index].key.as_url())
        {
            return;
        }
        let Ok(key) = PageKey::from_url(doc.page()) else {
            return;
        };
        if !crate::key::same_page(doc.page(), self.sections[index].key.as_url())
            || doc.page().fragment().is_some()
        {
            self.sections[index].locator = element_locator(doc.page()).unwrap_or_default();
        }
        self.sections[index].key = key.clone();
        self.sections[index].off_site =
            SiteKey::from_url(key.as_url()).as_ref() != Some(&self.site);
        let nodes = doc.nodes();
        let globals: BTreeSet<_> = self.sections.iter().map(|e| e.key.page_match()).collect();
        let root = self.entry.page_match();
        let local_group = |i| {
            ancestors(nodes, i).find(|&a| {
                matches!(
                    nodes[a].role,
                    Role::Landmark {
                        landmark: Some(Landmark::Navigation | Landmark::Complementary)
                    }
                )
            })
        };
        let mut sidebars = BTreeMap::<usize, (usize, usize)>::new();
        for (i, n) in nodes.iter().enumerate() {
            if let Some((url, group)) = n
                .destination
                .as_ref()
                .zip(local_group(i))
                .filter(|_| visible(nodes, i))
            {
                let counts = sidebars.entry(group).or_default();
                counts.0 += 1;
                counts.1 += usize::from(
                    PageKey::from_url(url).is_ok_and(|k| globals.contains(&k.page_match())),
                );
            }
        }
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
                    Role::Landmark {
                        landmark: Some(Landmark::Banner | Landmark::Contentinfo)
                    }
                )
            }) {
                continue;
            }
            let local = local_group(i);
            if local
                .and_then(|g| sidebars.get(&g))
                .is_some_and(|(all, global)| global * 2 > *all)
            {
                continue;
            }
            if matches!(n.role, Role::Heading { level: _ }) {
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
                if headings.len() >= MAX_CHILDREN {
                    if n.element_id.is_some() || dated {
                        self.sections[index].more += 1;
                    }
                    continue;
                }
                let locator = if dated || n.element_id.is_some() {
                    Locator::for_text(doc, i, &n.text, n.selector_id())
                } else {
                    None
                };
                if let Some(locator) = locator {
                    let mut e = self.sections[index].leaf(Source::Heading, &n.text);
                    e.locator = locator;
                    headings.push(e);
                    releases += 1;
                }
            } else if let Some((url, group)) = n.destination.as_ref().zip(local) {
                // The section can have its own delegated documentation host.
                let child = self
                    .entry_for(url, n.accessible_name(), Source::SectionSidebar)
                    .or_else(|| {
                        let scope = SiteKey::from_url(doc.page())?;
                        self.entry_for_site(
                            url,
                            n.accessible_name(),
                            Source::SectionSidebar,
                            &scope,
                        )
                    });
                if let Some(mut e) = child.filter(|e| {
                    let page = e.key.page_match();
                    page != root && !globals.contains(&page)
                }) {
                    e.group = heading
                        .as_ref()
                        .filter(|(g, _)| *g == group)
                        .map(|(_, s)| s.clone());
                    self.child(index, e);
                }
            }
        }
        for e in headings {
            self.child(index, e);
        }
    }
}
