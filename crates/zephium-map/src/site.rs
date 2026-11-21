use crate::{
    ancestors,
    doc::{Landmark, Role},
    key::{KeyError, LocatorText, MAX_URL_BYTES},
    linked,
    query::SECTION_WORDS,
    results::ProfileBasis,
    results::{
        clip, site_profile, Link, Profile, CHALLENGE, MAX_SITELINKS, MAX_SOCIAL, TITLE_BYTES,
    },
    robots::{Robots, RobotsOutcome, MAX_SITEMAPS},
    session::ReadState,
    visible, Doc, Locator, PageKey, PlaceKey, SectionRole, SiteKey,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use url::Url;
mod navigation;
mod sitemap;
pub use sitemap::{Sitemap, XmlError, MAX_SITEMAP_URLS, MAX_XML_BYTES, MAX_XML_DEPTH};

pub const ENTRY_ID_BYTES: usize = 16;
pub const MAX_PRIMARY_SECTIONS: usize = 12;
pub const MAX_SECONDARY_SECTIONS: usize = 20;
pub const MAX_SECTIONS: usize = MAX_PRIMARY_SECTIONS + MAX_SECONDARY_SECTIONS;
pub const MAX_PLACE_CHILDREN: usize = 36;
pub const MAX_CHILDREN: usize = 12;
pub const MAX_SECTION_PAGES: usize = 6;
pub const MAX_LLMS_BYTES: usize = 128 * 1024;
const SHELL_TEXT_BYTES: usize = 80;
const SHELL_LINKS: u32 = 2;
const RELEASE_HEADINGS: usize = 2;

#[derive(Clone, Copy, PartialOrd, Ord, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum Source {
    PrimaryNav,
    MenuGroup,
    Footer,
    EngineSitelink,
    EngineResult,
    SectionSidebar,
    Heading,
    Llms,
    Sitemap,
    LinkedData,
    LinkCluster,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Tier {
    Primary,
    Secondary,
}
impl Source {
    fn tier(self) -> Tier {
        match self {
            Self::PrimaryNav | Self::MenuGroup | Self::Llms => Tier::Primary,
            _ => Tier::Secondary,
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct EntryId(String);
impl EntryId {
    fn new(parent: Option<&EntryId>, key: &PageKey, locator: &Locator) -> Self {
        // FNV-1a over length-delimited discovery fields, independent of Rust's Hash implementation.
        let mut hash = 0xcbf29ce484222325u64;
        let matched = key.page_match();
        let q = locator.quote.as_ref();
        for part in [
            parent.map(|p| p.0.as_str()),
            Some(matched.as_str()),
            locator.element.as_ref().map(LocatorText::as_str),
            q.and_then(|q| q.prefix.as_ref()).map(LocatorText::as_str),
            q.map(|q| q.start.as_str()),
            q.and_then(|q| q.end.as_ref()).map(LocatorText::as_str),
            q.and_then(|q| q.suffix.as_ref()).map(LocatorText::as_str),
        ] {
            let bytes = part.unwrap_or_default().as_bytes();
            for b in (bytes.len() as u32)
                .to_be_bytes()
                .into_iter()
                .chain(bytes.iter().copied())
            {
                hash = (hash ^ u64::from(b)).wrapping_mul(0x100000001b3);
            }
        }
        Self(format!("{hash:016x}"))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
    pub(crate) fn valid(&self) -> bool {
        self.0.len() == ENTRY_ID_BYTES
            && self
                .0
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    }
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    pub id: EntryId,
    pub tier: Tier,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub read: Option<ReadState>,
    pub key: PageKey,
    pub locator: Locator,
    pub label: String,
    pub role: Option<SectionRole>,
    pub sources: Vec<Source>,
    pub off_site: bool,
    pub group: Option<String>,
    pub children: Vec<Entry>,
    pub more: u32,
}
#[derive(Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlaceNotes {
    pub shell: bool,
    pub wall: bool,
    pub robots_disallowed: bool,
    pub truncated: bool,
}
#[derive(Default, Clone, Debug, PartialEq, Eq)]
pub struct PlaceStats {
    pub robots_truncated: bool,
    pub entry_truncated: bool,
    pub sections_truncated: bool,
    pub section_page_truncated: bool,
    pub llms_truncated: bool,
    pub sitemap_truncated: bool,
    pub sitelinks_truncated: bool,
    pub linked_data_truncated: bool,
    pub links_seen: u32,
    pub sitemap_urls: u32,
    pub section_pages: u32,
}
impl PlaceStats {
    pub(crate) fn truncated(&self) -> bool {
        self.robots_truncated
            || self.entry_truncated
            || self.sections_truncated
            || self.section_page_truncated
            || self.llms_truncated
            || self.sitemap_truncated
            || self.sitelinks_truncated
            || self.linked_data_truncated
    }
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Place {
    pub key: PlaceKey,
    pub site: SiteKey,
    pub title: String,
    pub entry: PageKey,
    pub sections: Vec<Entry>,
    pub focus: Option<EntryId>,
    pub social: Vec<Profile>,
    pub notes: PlaceNotes,
}

#[derive(Default)]
pub struct SiteInputs<'a> {
    pub result_section: Option<&'a Link>,
    pub sitelinks: &'a [Link],
    pub robots: Option<&'a Robots>,
    pub llms: Option<&'a str>,
    pub sitemaps: BTreeMap<PageKey, &'a Sitemap>,
    pub pages: BTreeMap<PageKey, &'a Doc>,
}

pub const MAX_WALL_TEXT_BYTES: usize = 2048;
pub(crate) fn challenge(doc: &Doc) -> bool {
    let hit = |s: &str| {
        let lower = s.to_ascii_lowercase();
        CHALLENGE.iter().any(|p| lower.contains(p))
    };
    let prompt = |s: &str| {
        let lower = s
            .trim()
            .trim_end_matches(['.', '!', '?'])
            .to_ascii_lowercase();
        let lower = lower.strip_prefix("please ").unwrap_or(&lower);
        CHALLENGE.iter().any(|&phrase| {
            lower == phrase
                || phrase != "captcha"
                    && lower
                        .strip_prefix(phrase)
                        .is_some_and(|tail| tail.starts_with(" to "))
        })
    };
    let mut size = 0usize;
    let nodes = doc.nodes();
    let mut challenge = hit(&doc.head().title);
    for (i, n) in nodes.iter().enumerate().filter(|(i, _)| visible(nodes, *i)) {
        size = size.saturating_add(n.text.len());
        if size > MAX_WALL_TEXT_BYTES
            || matches!(n.role, Role::Paragraph | Role::CodeBlock | Role::Document)
                && !n.text.trim().is_empty()
                && !prompt(&n.text)
                && ancestors(nodes, i).any(|a| {
                    matches!(
                        nodes[a].role,
                        Role::Document
                            | Role::Landmark {
                                landmark: Some(Landmark::Main)
                            }
                    )
                })
        {
            return false;
        }
        challenge |= hit(&n.text);
    }
    challenge
}

fn normal(s: &str) -> String {
    s.split(|c: char| !c.is_alphanumeric())
        .filter(|s| !s.is_empty())
        .map(str::to_lowercase)
        .collect::<Vec<_>>()
        .join(" ")
}
pub(crate) fn section_role(name: &str, url: &Url) -> Option<SectionRole> {
    for value in [
        name,
        url.path()
            .trim_matches('/')
            .rsplit('/')
            .next()
            .unwrap_or_default(),
        url.host_str()
            .and_then(|h| h.split('.').next())
            .unwrap_or_default(),
    ] {
        let label = normal(value);
        if let Some((_, role)) = SECTION_WORDS.iter().find(|(w, _)| {
            label.match_indices(w).any(|(i, _)| {
                (i == 0 || label.as_bytes()[i - 1] == b' ')
                    && label.as_bytes().get(i + w.len()).is_none_or(|b| *b == b' ')
            })
        }) {
            return Some(*role);
        }
    }
    (url.path() == "/" && name.eq_ignore_ascii_case("home")).then_some(SectionRole::Home)
}
pub(crate) fn external_role(role: Option<SectionRole>) -> bool {
    matches!(
        role,
        Some(
            SectionRole::Documentation
                | SectionRole::Status
                | SectionRole::Changelog
                | SectionRole::Releases
                | SectionRole::Blog
                | SectionRole::Community
        )
    )
}
pub(crate) struct Focus {
    pub role: SectionRole,
    pub modifiers: Vec<String>,
    pub fresh: bool,
}
impl Focus {
    fn rank(&self, e: &Entry) -> Option<(bool, bool, bool)> {
        let exact = e.role == Some(self.role);
        if !(exact
            || self.fresh
                && matches!(
                    e.role,
                    Some(
                        SectionRole::Changelog
                            | SectionRole::Releases
                            | SectionRole::News
                            | SectionRole::Blog
                    )
                ))
        {
            return None;
        }
        let url = e.key.as_url();
        let evidence = [
            e.label.as_str(),
            url.path(),
            url.host_str().unwrap_or_default(),
        ];
        if !self.modifiers.iter().all(|modifier| {
            evidence.iter().any(|s| {
                s.split(|c: char| !c.is_alphanumeric())
                    .any(|w| w.eq_ignore_ascii_case(modifier))
            })
        }) {
            return None;
        }
        let (path, label) = e.focus_rank(self.role);
        Some((!exact, path, label))
    }
}
struct Build<'a> {
    place: Place,
    index: std::collections::HashMap<(crate::PageMatch, Locator), usize>,
    requested: Option<&'a Focus>,
    stats: PlaceStats,
}
impl std::ops::Deref for Build<'_> {
    type Target = Place;
    fn deref(&self) -> &Place {
        &self.place
    }
}
impl std::ops::DerefMut for Build<'_> {
    fn deref_mut(&mut self) -> &mut Place {
        &mut self.place
    }
}
impl Build<'_> {
    fn add_results(&mut self, links: &[Link], result: Option<&Link>) {
        self.stats.sitelinks_truncated = links.len() > MAX_SITELINKS;
        for l in links.iter().take(MAX_SITELINKS) {
            if let Some(e) = self.entry_for(&l.url, &l.title, Source::EngineSitelink) {
                self.section(e);
            }
        }
        if let Some(link) = result {
            if let Some(entry) = self.entry_for(&link.url, &link.title, Source::EngineResult) {
                self.section(entry);
            }
        }
    }
    fn entry_for(&self, url: &Url, label: &str, source: Source) -> Option<Entry> {
        self.entry_for_site(url, label, source, &self.site)
    }
    fn entry_for_site(
        &self,
        url: &Url,
        label: &str,
        source: Source,
        scope: &SiteKey,
    ) -> Option<Entry> {
        let key = PageKey::from_url(url).ok()?;
        let site = SiteKey::from_url(url)?;
        let label = clip(&crate::clean_text(label.trim()), TITLE_BYTES);
        if label.is_empty() {
            return None;
        }
        let role = section_role(&label, url);
        let off_site = site != self.site;
        if &site != scope
            && !(matches!(
                source,
                Source::PrimaryNav | Source::MenuGroup | Source::Footer
            ) && external_role(role))
        {
            return None;
        }
        let locator = element_locator(url).ok()?;
        Some(Entry {
            id: EntryId::new(None, &key, &locator),
            read: None,
            tier: source.tier(),
            key,
            label,
            role,
            off_site,
            sources: [source].into(),
            locator,
            group: None,
            children: Vec::new(),
            more: 0,
        })
    }
    fn section(&mut self, entry: Entry) -> usize {
        let destination = (entry.key.page_match(), entry.locator.clone());
        if let Some(&i) = self.index.get(&destination) {
            self.sections[i].add_sources(entry.sources);
            return i;
        }
        let index = self.sections.len();
        self.index.insert(destination, index);
        self.sections.push(entry);
        index
    }
    fn child(&mut self, index: usize, entry: Entry) {
        let requested = self.requested;
        self.sections[index].child(entry, requested);
    }
    fn finish_sections(&mut self) {
        let requested = self.requested;
        let cluster_primary = !self
            .sections
            .iter()
            .any(|e| e.tier == Tier::Primary && !e.utility());
        let globals: BTreeSet<_> = self
            .sections
            .iter()
            .map(|e| (e.key.page_match(), e.locator.fragment()))
            .chain(std::iter::once((self.entry.page_match(), None)))
            .collect();
        let first_signup = self
            .sections
            .iter()
            .find(|e| e.role == Some(SectionRole::SignUp))
            .map(|e| e.id.clone());
        let demoted = |e: &Entry| {
            e.utility()
                || e.role == Some(SectionRole::SignUp) && Some(&e.id) != first_signup.as_ref()
        };
        for e in &mut self.sections {
            if e.sources.contains(&Source::LinkCluster) && cluster_primary {
                e.tier = Tier::Primary;
            }
            if demoted(e) {
                e.tier = Tier::Secondary;
            }
            e.children.retain(|c| {
                c.sources.contains(&Source::Heading)
                    || !globals.contains(&(c.key.page_match(), c.locator.fragment()))
            });
        }
        self.sections.sort_by_cached_key(|e| (e.tier, demoted(e)));
        let mut focus = None;
        if let Some(request) = requested {
            focus = self
                .sections
                .iter()
                .flat_map(|e| std::iter::once((e, e)).chain(e.children.iter().map(move |c| (e, c))))
                .filter_map(|(p, e)| Some((request.rank(e)?, p, e)))
                .min_by_key(|(rank, p, e)| (*rank, e.tier, p.id != e.id))
                .map(|(_, parent, entry)| (parent.id.clone(), entry.id.clone()));
        }
        let mut sections = std::mem::take(&mut self.sections);
        for tier in [Tier::Primary, Tier::Secondary] {
            let cap = match tier {
                Tier::Primary => MAX_PRIMARY_SECTIONS,
                Tier::Secondary => MAX_SECONDARY_SECTIONS,
            };
            let priority = |entry: &Entry| {
                (
                    focus
                        .as_ref()
                        .is_some_and(|(parent, _)| parent == &entry.id),
                    entry.role.is_some(),
                )
            };
            let mut kept = Vec::new();
            for e in sections.extract_if(.., |e| e.tier == tier) {
                if kept.len() == cap {
                    self.stats.sections_truncated = true;
                    let rank = priority(&e);
                    let replace = kept
                        .iter()
                        .enumerate()
                        .rev()
                        .min_by_key(|(_, entry)| priority(entry))
                        .filter(|(_, entry)| priority(entry) < rank);
                    let Some((index, _)) = replace else { continue };
                    kept.remove(index);
                }
                kept.push(e);
            }
            self.sections.extend(kept);
        }
        self.focus = focus.map(|(_, id)| id);
        let total: usize = self.sections.iter().map(|e| e.children.len()).sum();
        for _ in MAX_PLACE_CHILDREN..total {
            if let Some(section) = self
                .place
                .sections
                .iter_mut()
                .max_by_key(|e| e.children.len())
            {
                let Some(index) = section
                    .children
                    .iter()
                    .rposition(|c| Some(&c.id) != self.place.focus.as_ref())
                else {
                    break;
                };
                section.children.remove(index);
                section.more = section.more.saturating_add(1);
            }
        }
    }
    fn profile(&mut self, url: &Url, basis: ProfileBasis) -> bool {
        let Some(network) = site_profile(url) else {
            return false;
        };
        if let Some(p) = self
            .social
            .iter_mut()
            .find(|p| crate::key::same_page(&p.url, url))
        {
            if basis == ProfileBasis::SameAs {
                p.basis = basis;
            }
            return true;
        }
        if self.social.len() < MAX_SOCIAL && PageKey::from_url(url).is_ok() {
            self.social.push(Profile {
                network,
                basis,
                url: url.clone(),
            });
        }
        true
    }
    fn linked_data(&mut self, doc: &Doc) {
        linked::objects(doc.head(), |v| {
            if linked::typed(v, "Organization") {
                let links = &v["sameAs"];
                for s in links
                    .as_array()
                    .map(Vec::as_slice)
                    .unwrap_or(std::slice::from_ref(links))
                    .iter()
                    .filter_map(|v| v.as_str())
                {
                    if let Ok(url) = Url::parse(s) {
                        self.profile(&url, ProfileBasis::SameAs);
                    }
                }
            }
            if linked::typed(v, "WebSite") && self.title == self.site.as_str() {
                if let Some(name) = v["name"].as_str() {
                    self.title = clip(&crate::clean_text(name), TITLE_BYTES);
                }
            }
            let mut add = |label: Option<&str>, path: Option<&str>| {
                if let Some((name, url)) = label.zip(path.and_then(|s| doc.page().join(s).ok())) {
                    if let Some(e) = self.entry_for(&url, name, Source::LinkedData) {
                        self.section(e);
                    }
                }
            };
            if linked::typed(v, "SiteNavigationElement") {
                add(v["name"].as_str(), v["url"].as_str());
                if let Some((names, urls)) = v["name"].as_array().zip(v["url"].as_array()) {
                    for (n, u) in names.iter().zip(urls) {
                        add(n.as_str(), u.as_str());
                    }
                }
            } else if linked::typed(v, "BreadcrumbList") {
                if let Some(items) = v["itemListElement"].as_array() {
                    for item in items {
                        add(
                            item.get("name")
                                .or_else(|| item["item"].get("name"))
                                .and_then(|i| i.as_str()),
                            item["item"]
                                .as_str()
                                .or_else(|| item["item"]["@id"].as_str()),
                        );
                    }
                }
            }
        });
    }
    fn llms(&mut self, text: &str) {
        if text.len() > MAX_LLMS_BYTES {
            self.stats.llms_truncated = true;
            return;
        }
        if self.title == self.site.as_str() {
            if let Some(title) = text
                .lines()
                .map(str::trim)
                .find_map(|l| l.strip_prefix("# "))
                .filter(|s| !s.trim().is_empty())
            {
                self.title = clip(&crate::clean_text(title.trim()), TITLE_BYTES);
            }
        }
        let mut labels = BTreeMap::new();
        for (i, e) in self.sections.iter().enumerate() {
            labels.entry(normal(&e.label)).or_insert(i);
        }
        let mut heading = None;
        let mut index = None;
        for line in text.lines().map(str::trim) {
            if let Some(label) = line.strip_prefix("## ") {
                heading = Some(label.trim());
                index = labels.get(&normal(label)).copied();
                continue;
            }
            let Some(label) = heading else { continue };
            let Some((name, tail)) = line
                .trim_start_matches(['-', '*', ' '])
                .strip_prefix('[')
                .and_then(|s| s.split_once("]("))
            else {
                continue;
            };
            let Some(path) = tail
                .split_once(')')
                .map(|(s, _)| s)
                .filter(|s| s.len() <= MAX_URL_BYTES)
            else {
                continue;
            };
            let Ok(url) = self.entry.as_url().join(path) else {
                continue;
            };
            let Some(child) = self.entry_for(&url, name, Source::Llms) else {
                continue;
            };
            let parent = match index {
                Some(i) => i,
                None => {
                    let Some(entry) = self.entry_for(&url, label, Source::Llms) else {
                        continue;
                    };
                    let i = self.section(entry);
                    labels.entry(normal(label)).or_insert(i);
                    index = Some(i);
                    i
                }
            };
            self.sections[parent].add_sources([Source::Llms]);
            self.child(parent, child);
        }
    }

    fn sitemap(&mut self, map: &Sitemap) {
        self.stats.sitemap_truncated |= map.truncated;
        self.stats.sitemap_urls = self.stats.sitemap_urls.saturating_add(map.count);
        for (prefix, group) in map.groups() {
            let Some(first) = group.pages.first() else {
                continue;
            };
            let Some(seg) = first.as_url().path_segments().and_then(|mut s| s.next()) else {
                continue;
            };
            let existing = self
                .sections
                .iter()
                .position(|e| e.key.page_match() == *prefix);
            if existing.is_none()
                && group.count == 1
                && first.as_url().path().trim_matches('/').contains('/')
            {
                continue;
            }
            let index = if let Some(i) = existing {
                i
            } else {
                let Some(entry) = self.entry_for(first.as_url(), seg, Source::Sitemap) else {
                    continue;
                };
                self.section(entry)
            };
            self.sections[index].add_sources([Source::Sitemap]);
            for key in &group.pages {
                if self.sections[index].key.page_match() == key.page_match() {
                    continue;
                }
                let label = key
                    .as_url()
                    .path()
                    .trim_end_matches('/')
                    .rsplit('/')
                    .next()
                    .unwrap_or_default()
                    .replace('-', " ");
                if let Some(child) = self.entry_for(key.as_url(), &label, Source::Sitemap) {
                    self.child(index, child);
                }
            }
            self.sections[index].more += group.count.saturating_sub(group.pages.len() as u32);
        }
    }
}
impl Place {
    pub(crate) fn check_ids(&self) -> Result<(), KeyError> {
        let mut seen = BTreeSet::new();
        if self
            .sections
            .iter()
            .flat_map(|e| std::iter::once(e).chain(&e.children))
            .all(|e| e.id.valid() && seen.insert(&e.id))
        {
            Ok(())
        } else {
            Err(KeyError::Identity)
        }
    }
    pub(crate) fn keep_discovery(&mut self, previous: &Place) {
        for section in &mut self.sections {
            if section.sources.first() != Some(&Source::Sitemap) {
                continue;
            }
            // A sitemap cluster can acquire a shallower representative on the second read.
            if let Some(old) = previous.sections.iter().find(|old| {
                old.sources.first() == Some(&Source::Sitemap)
                    && old.label == section.label
                    && old.key.as_url().host_str() == section.key.as_url().host_str()
                    && old.id != section.id
            }) {
                if self.focus.as_ref() == Some(&section.id) {
                    self.focus = Some(old.id.clone());
                }
                section.id = old.id.clone();
                for child in &mut section.children {
                    let id = EntryId::new(Some(&section.id), &child.key, &child.locator);
                    if self.focus.as_ref() == Some(&child.id) {
                        self.focus = Some(id.clone());
                    }
                    child.id = id;
                }
            }
        }
    }
    fn focuses(&self, entry: &Entry) -> bool {
        self.focus.as_ref().is_some_and(|focus| {
            entry.id == *focus || entry.children.iter().any(|child| child.id == *focus)
        })
    }
    pub(crate) fn read_priority(&self, entry: &Entry) -> (bool, bool) {
        (!self.focuses(entry), entry.role.is_none())
    }
    pub(crate) fn fetch_sections(&self) -> Vec<&Entry> {
        let mut entries: Vec<_> = self
            .sections
            .iter()
            .filter(|e| {
                if e.sources.contains(&Source::EngineResult) {
                    return true;
                }
                e.key.page_match() != self.entry.page_match()
                    && !e
                        .children
                        .iter()
                        .any(|c| c.sources.first() == Some(&Source::MenuGroup))
                    && matches!(
                        e.role,
                        Some(
                            SectionRole::News
                                | SectionRole::Documentation
                                | SectionRole::Api
                                | SectionRole::Reference
                                | SectionRole::Pricing
                                | SectionRole::Product
                                | SectionRole::Changelog
                                | SectionRole::Releases
                                | SectionRole::Download
                                | SectionRole::Install
                        )
                    )
            })
            .collect();
        entries.sort_by_key(|e| self.read_priority(e));
        entries
    }
}
fn element_locator(url: &Url) -> Result<Locator, KeyError> {
    let Some(fragment) = url
        .fragment()
        .filter(|s| !s.is_empty() && !crate::key::route(s))
    else {
        return Ok(Locator::default());
    };
    let id = decode_fragment(fragment);
    if crate::clean_text(&id) != id {
        return Ok(Locator::default());
    }
    Ok(Locator {
        element: Some(LocatorText::new(&id)?),
        ..Default::default()
    })
}

fn decode_fragment(s: &str) -> String {
    url::form_urlencoded::parse(
        format!("id={}", s.replace('+', "%2B").replace('&', "%26")).as_bytes(),
    )
    .next()
    .map(|(_, v)| v.into_owned())
    .unwrap_or_default()
}
pub(crate) fn quote(text: &str) -> Option<Locator> {
    let words = crate::flatten(text);
    let mut opening = clip(&words, crate::key::MAX_LOCATOR_BYTES);
    if opening.len() < words.len() && !words[opening.len()..].starts_with(' ') {
        if let Some(end) = opening.rfind(' ') {
            opening.truncate(end);
        }
    }
    let tail = words[opening.len()..].trim();
    let end = if tail.is_empty() {
        None
    } else {
        let at = tail.ceil_char_boundary(tail.len().saturating_sub(crate::key::MAX_LOCATOR_BYTES));
        let tail = &tail[at..];
        let phrase = if at == 0 {
            tail
        } else {
            tail.split_once(' ').map_or(tail, |(_, t)| t)
        };
        LocatorText::new(phrase).ok()
    };
    Some(Locator {
        quote: Some(crate::key::TextQuote {
            prefix: None,
            suffix: None,
            start: LocatorText::new(&opening).ok()?,
            end,
        }),
        ..Default::default()
    })
}
impl Entry {
    fn utility(&self) -> bool {
        let label = self.label.to_lowercase();
        self.role == Some(SectionRole::Legal)
            || ["skip to", "language", "cookie", "select region"]
                .iter()
                .any(|s| label.contains(s))
    }
    fn leaf(&self, source: Source, label: &str) -> Entry {
        Entry {
            id: self.id.clone(),
            read: None,
            tier: source.tier(),
            key: self.key.clone(),
            locator: Locator::default(),
            label: clip(label, TITLE_BYTES),
            role: None,
            off_site: self.off_site,
            sources: [source].into(),
            group: None,
            children: Vec::new(),
            more: 0,
        }
    }
    fn add_sources(&mut self, sources: impl IntoIterator<Item = Source>) {
        for source in sources {
            self.tier = self.tier.min(source.tier());
            if !self.sources.contains(&source) {
                self.sources.push(source);
            }
        }
    }
    fn focus_rank(&self, role: SectionRole) -> (bool, bool) {
        let exact = |s: &str| {
            SECTION_WORDS
                .iter()
                .any(|&(word, r)| r == role && s.eq_ignore_ascii_case(word))
        };
        (
            !exact(
                self.key
                    .as_url()
                    .path()
                    .trim_matches('/')
                    .rsplit('/')
                    .next()
                    .unwrap_or(""),
            ),
            !exact(&self.label),
        )
    }
    fn child(&mut self, mut entry: Entry, requested: Option<&Focus>) {
        if let Some(existing) = self
            .children
            .iter_mut()
            .find(|e| e.key.page_match() == entry.key.page_match() && e.locator == entry.locator)
        {
            existing.add_sources(entry.sources);
            return;
        }
        if self.children.len() == MAX_CHILDREN
            && requested.is_some_and(|request| {
                request.rank(&entry).is_some_and(|rank| {
                    self.children
                        .iter()
                        .filter_map(|c| request.rank(c))
                        .all(|existing| rank < existing)
                })
            })
        {
            self.children.pop();
            self.more += 1;
        }
        if self.children.len() < MAX_CHILDREN {
            entry.id = EntryId::new(Some(&self.id), &entry.key, &entry.locator);
            self.children.push(entry);
        } else {
            self.more += 1;
        }
    }
}

pub(crate) fn with_results(
    place: Place,
    links: &[Link],
    hint: Option<&Link>,
    requested: Option<&Focus>,
) -> Result<Place, KeyError> {
    let mut build = Build {
        index: place
            .sections
            .iter()
            .enumerate()
            .map(|(i, e)| ((e.key.page_match(), e.locator.clone()), i))
            .collect(),
        requested,
        place,
        stats: PlaceStats::default(),
    };
    for (link, source) in links
        .iter()
        .take(MAX_SITELINKS)
        .map(|link| (link, Source::EngineSitelink))
        .chain(hint.map(|link| (link, Source::EngineResult)))
    {
        if let Some(entry) = build.entry_for(&link.url, &link.title, source) {
            if let Some(existing) = build
                .sections
                .iter_mut()
                .find(|existing| existing.id == entry.id)
            {
                existing.add_sources(entry.sources);
            } else {
                build.section(entry);
            }
        }
    }
    build.stats.sitelinks_truncated = links.len() > MAX_SITELINKS;
    build.finish_sections();
    build.notes.truncated |= build.stats.truncated();
    build.check_ids()?;
    Ok(build.place)
}
pub fn read_site(doc: &Doc, inputs: &SiteInputs<'_>) -> Result<Place, KeyError> {
    Ok(read_site_with_stats(doc, inputs)?.0)
}
pub fn read_site_with_stats(
    doc: &Doc,
    inputs: &SiteInputs<'_>,
) -> Result<(Place, PlaceStats), KeyError> {
    read_site_for(doc, inputs, None)
}
pub(crate) fn read_site_for(
    doc: &Doc,
    inputs: &SiteInputs<'_>,
    requested: Option<&Focus>,
) -> Result<(Place, PlaceStats), KeyError> {
    let key = PlaceKey::from_url(doc.page()).ok_or(KeyError::Url)?;
    let site = SiteKey::from_url(doc.page()).ok_or(KeyError::Url)?;
    let title = [
        doc.head().site_name.as_str(),
        doc.head().title.as_str(),
        site.as_str(),
    ]
    .into_iter()
    .find(|s| !s.trim().is_empty())
    .unwrap_or_default();
    let mut place = Build {
        index: Default::default(),
        requested,
        stats: PlaceStats::default(),
        place: Place {
            title: clip(title, TITLE_BYTES),
            site,
            entry: key.root(doc.page())?,
            key,
            sections: Vec::new(),
            focus: None,
            social: Vec::new(),
            notes: PlaceNotes::default(),
        },
    };
    place.notes.wall = challenge(doc);
    place.stats.entry_truncated = doc.body_truncated();
    place.stats.linked_data_truncated = doc.head().truncated;
    if !place.notes.wall {
        place.navigation(doc, false);
        place.navigation(doc, true);
        place.link_clusters(doc);
        let nodes = doc.nodes();
        place.notes.shell = place.stats.links_seen <= SHELL_LINKS
            && nodes.iter().map(|n| n.text.len()).sum::<usize>() <= SHELL_TEXT_BYTES
            && nodes.iter().enumerate().any(|(i, n)| {
                matches!(n.element_id.as_deref(), Some("root" | "app" | "__next"))
                    && n.text.is_empty()
                    && nodes
                        .get(i + 1)
                        .is_none_or(|child| child.parent_index() != Some(i))
            });
    } else {
        place.title = place.site.as_str().to_owned();
    }
    place.add_results(inputs.sitelinks, inputs.result_section);
    if !place.notes.wall {
        place.linked_data(doc);
    }
    if !place.notes.wall && doc.page().path() != "/" {
        let heading = doc
            .nodes()
            .iter()
            .find(|n| matches!(n.role, Role::Heading { level: Some(1) }))
            .map(|n| n.text.as_str())
            .filter(|s| !s.is_empty())
            .unwrap_or(&doc.head().title);
        if let Some(entry) = place
            .entry_for(doc.page(), heading, Source::Heading)
            .filter(|e| e.role.is_some())
        {
            let i = place.section(entry);
            place.section_page(i, doc);
        }
    }
    // Infer parents first, then insert their children in source priority order.
    if let Some(text) = inputs.llms {
        place.llms(text);
    }
    let mut combined = Sitemap::default();
    for map in inputs.sitemaps.values().take(MAX_SITEMAPS) {
        combined.merge(map);
    }
    place.sitemap(&combined);
    place.stats.sitemap_truncated |= inputs.sitemaps.len() > MAX_SITEMAPS;
    let page_sections: Vec<_> = inputs
        .pages
        .iter()
        .take(MAX_SECTION_PAGES)
        .flat_map(|(key, page)| {
            let target = key.page_match();
            place
                .sections
                .iter()
                .enumerate()
                .filter_map(move |(index, e)| {
                    (e.key.page_match() == target).then_some((index, *page))
                })
        })
        .collect();
    for (i, page) in page_sections {
        let (mut preferred, others): (Vec<_>, Vec<_>) =
            std::mem::take(&mut place.sections[i].children)
                .into_iter()
                .partition(|e| e.sources.first() == Some(&Source::MenuGroup));
        place.sections[i].children.append(&mut preferred);
        place.section_page(i, page);
        for c in others {
            place.child(i, c);
        }
    }
    place.stats.section_page_truncated |= inputs.pages.len() > MAX_SECTION_PAGES;
    if let Some(robots) = inputs.robots {
        place.stats.robots_truncated = robots.truncated || robots.outcome == RobotsOutcome::Limit;
        place.notes.robots_disallowed = place.fetch_sections().iter().any(|e| {
            e.key.as_url().origin() == doc.page().origin()
                && !robots.allows(&e.key.as_url()[url::Position::BeforePath..])
        });
    }
    place.finish_sections();
    place.check_ids()?;
    place.notes.truncated = place.stats.truncated() || place.sections.iter().any(|e| e.more > 0);
    Ok((place.place, place.stats))
}

pub(crate) fn sitemap_priority(url: &Url, entry: &Url) -> (bool, bool, bool) {
    let path = url.path().to_ascii_lowercase();
    let parts: Vec<_> = path.split(|c: char| !c.is_ascii_alphanumeric()).collect();
    let locale = entry
        .path_segments()
        .and_then(|mut p| p.next())
        .filter(|s| s.len() == 2)
        .unwrap_or("en");
    let other_locale = parts
        .iter()
        .any(|s| s.len() == 2 && s.bytes().all(|b| b.is_ascii_alphabetic()) && *s != locale);
    (
        other_locale,
        parts.iter().any(|s| {
            matches!(
                *s,
                "post"
                    | "posts"
                    | "product"
                    | "products"
                    | "image"
                    | "images"
                    | "video"
                    | "videos"
                    | "tag"
                    | "tags"
            )
        }),
        !parts
            .iter()
            .any(|s| matches!(*s, "page" | "pages" | "docs")),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn asked_section_survives_tier_and_menu_limits() {
        let links = (0..MAX_PRIMARY_SECTIONS + 1)
            .map(|i| format!("<li><a href='/s{i}'>Item {i}</a></li>"))
            .collect::<String>();
        let children = (0..MAX_CHILDREN + 1)
            .map(|i| format!("<li><a href='/c{i}'>Child {i}</a></li>"))
            .collect::<String>();
        let root = Url::parse("https://workshop.example").unwrap();
        let top = "<li><a href='/pricing'>Pricing</a></li>".to_owned();
        let nested = format!("<li><a href='/products'>Product</a><ul>{children}{top}</ul></li>");
        let request = Focus {
            role: SectionRole::Pricing,
            modifiers: Vec::new(),
            fresh: false,
        };
        for menu in [top, nested] {
            let html = format!("<nav><ul>{links}{menu}</ul></nav>");
            let doc =
                Doc::from_html(&root, html.as_bytes(), None, crate::DocLimits::SITE_PAGE).unwrap();
            let (place, _) = read_site_for(&doc, &SiteInputs::default(), Some(&request)).unwrap();
            assert_eq!(place.sections.len(), MAX_PRIMARY_SECTIONS);
            let focus = place
                .sections
                .iter()
                .flat_map(|e| std::iter::once(e).chain(&e.children))
                .find(|e| Some(&e.id) == place.focus.as_ref())
                .unwrap();
            assert_eq!(focus.key.as_url().path(), "/pricing");
        }
    }

    #[test]
    fn a_link_fragment_changed_by_cleaning_names_no_element() {
        let locator = |raw: &str| element_locator(&Url::parse(raw).unwrap()).unwrap();
        assert!(locator("https://workshop.example/a#a%E2%80%8Bb") == Locator::default());
        assert_eq!(
            locator("https://workshop.example/a#ab")
                .element
                .as_ref()
                .map(LocatorText::as_str),
            Some("ab")
        );
    }
}

#[cfg(test)]
mod review_tests {
    use super::*;

    #[test]
    fn result_sitelink_merges_with_the_discovered_id_after_a_redirect() {
        let base = Url::parse("https://example.com/").unwrap();
        let doc = |url: &Url, html: &str| {
            Doc::from_html(url, html.as_bytes(), None, crate::DocLimits::SITE_PAGE).unwrap()
        };
        let home = doc(&base, "<nav><a href='/docs'>Documentation</a></nav>");
        let original = base.join("/docs").unwrap();
        let landing = doc(&base.join("/guide").unwrap(), "<p>Guide</p>");
        let place = read_site(
            &home,
            &SiteInputs {
                pages: [(PageKey::from_url(&original).unwrap(), &landing)].into(),
                ..SiteInputs::default()
            },
        )
        .unwrap();
        let discovered = place.sections[0].id.clone();
        let merged = with_results(
            place,
            &[Link {
                url: original,
                title: "Docs".into(),
            }],
            None,
            None,
        )
        .unwrap();
        assert_eq!(merged.sections.len(), 1);
        assert!(merged.sections[0].id == discovered);
        assert_eq!(merged.sections[0].key.as_url().path(), "/guide");
        assert!(merged.sections[0].sources.contains(&Source::EngineSitelink));
    }
}
