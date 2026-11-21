use crate::{
    ancestors,
    doc::{Landmark, Role},
    key::{KeyError, LocatorText, MAX_URL_BYTES},
    linked,
    query::SECTION_WORDS,
    results::{
        clip, site_profile, Link, Profile, CHALLENGE, MAX_SITELINKS, MAX_SOCIAL, TITLE_BYTES,
    },
    robots::{Robots, RobotsOutcome, MAX_SITEMAPS},
    visible, Doc, Locator, PageKey, SectionRole, SiteKey,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use url::Url;
mod navigation;
mod sitemap;
pub use sitemap::{Sitemap, XmlError, MAX_SITEMAP_URLS, MAX_XML_BYTES, MAX_XML_DEPTH};

pub const MAX_SECTIONS: usize = 32;
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
    SectionSidebar,
    Heading,
    Llms,
    Sitemap,
    LinkedData,
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    pub key: PageKey,
    pub locator: Locator,
    pub label: String,
    pub role: Option<SectionRole>,
    pub source: Source,
    pub sources: BTreeSet<Source>,
    pub external: bool,
    pub group: Option<String>,
    pub children: Vec<Entry>,
    pub more: u32,
}
#[derive(Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlaceNotes {
    pub shell: bool,
    pub wall: bool,
    pub robots_disallowed: bool,
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
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Place {
    pub site: SiteKey,
    pub title: String,
    pub entry: PageKey,
    pub sections: Vec<Entry>,
    pub social: Vec<Profile>,
    pub notes: PlaceNotes,
}

#[derive(Default)]
pub struct SiteInputs<'a> {
    pub sitelinks: &'a [Link],
    pub robots: Option<&'a Robots>,
    pub llms: Option<&'a str>,
    pub sitemaps: BTreeMap<PageKey, &'a Sitemap>,
    pub pages: BTreeMap<PageKey, &'a Doc>,
}

pub(crate) fn challenge(doc: &Doc) -> bool {
    let hit = |s: &str| {
        let lower = s.to_ascii_lowercase();
        CHALLENGE.iter().any(|p| lower.contains(p))
    };
    hit(&doc.head().title)
        || doc
            .nodes()
            .iter()
            .enumerate()
            .any(|(i, n)| visible(doc.nodes(), i) && hit(&n.text))
}

fn normal(s: &str) -> String {
    s.split(|c: char| !c.is_alphanumeric())
        .filter(|s| !s.is_empty())
        .map(str::to_lowercase)
        .collect::<Vec<_>>()
        .join(" ")
}
fn section_role(name: &str, url: &Url) -> Option<SectionRole> {
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
impl Place {
    fn entry_for(&self, url: &Url, label: &str, source: Source) -> Option<Entry> {
        let key = PageKey::from_url(url).ok()?;
        let site = SiteKey::from_url(url)?;
        let label = clip(label.trim(), TITLE_BYTES);
        if label.is_empty() {
            return None;
        }
        let role = section_role(&label, url);
        let external = url.host_str() != self.entry.as_url().host_str();
        if site != self.site
            && !(matches!(
                source,
                Source::PrimaryNav | Source::MenuGroup | Source::Footer
            ) && external_role(role))
        {
            return None;
        }
        let locator = if let Some(fragment) = url.fragment().filter(|s| !s.is_empty()) {
            Locator::Element(LocatorText::new(&decode_fragment(fragment)).ok()?)
        } else {
            Locator::None
        };
        Some(Entry {
            key,
            label,
            role,
            external,
            source,
            sources: [source].into(),
            locator,
            group: None,
            children: Vec::new(),
            more: 0,
        })
    }
    fn section(&mut self, entry: Entry) -> Option<usize> {
        if let Some(i) = self
            .sections
            .iter()
            .position(|e| e.key == entry.key && e.locator == entry.locator)
        {
            self.sections[i].sources.extend(entry.sources);
            return Some(i);
        }
        let duplicate_signup = entry.role == Some(SectionRole::SignUp)
            && self
                .sections
                .iter()
                .any(|e| e.role == Some(SectionRole::SignUp));
        let utility = entry.utility() || duplicate_signup;
        let mut first_signup = true;
        let count = self
            .sections
            .iter()
            .filter(|e| {
                let duplicate =
                    e.role == Some(SectionRole::SignUp) && !std::mem::take(&mut first_signup);
                (e.utility() || duplicate) == utility
            })
            .count();
        if count >= MAX_SECTIONS {
            self.notes.sections_truncated = true;
            return None;
        }
        self.sections.push(entry);
        Some(self.sections.len() - 1)
    }
    fn profile(&mut self, url: &Url) -> bool {
        let Some(network) = site_profile(url) else {
            return false;
        };
        if self.social.len() < MAX_SOCIAL
            && !self.social.iter().any(|p| p.url == *url)
            && PageKey::from_url(url).is_ok()
        {
            self.social.push(Profile {
                network,
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
                        self.profile(&url);
                    }
                }
            }
            if linked::typed(v, "WebSite") && self.title == self.site.as_str() {
                if let Some(name) = v["name"].as_str() {
                    self.title = clip(name, TITLE_BYTES);
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
            self.notes.llms_truncated = true;
            return;
        }
        if self.title == self.site.as_str() {
            if let Some(title) = text
                .lines()
                .map(str::trim)
                .find_map(|l| l.strip_prefix("# "))
                .filter(|s| !s.trim().is_empty())
            {
                self.title = clip(title.trim(), TITLE_BYTES);
            }
        }
        let mut heading = None;
        let mut index = None;
        for line in text.lines().map(str::trim) {
            if let Some(label) = line.strip_prefix("## ") {
                heading = Some(label.trim());
                index = self
                    .sections
                    .iter()
                    .position(|e| normal(&e.label) == normal(label));
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
                    let Some(i) = self.section(entry) else {
                        continue;
                    };
                    index = Some(i);
                    i
                }
            };
            self.sections[parent].sources.insert(Source::Llms);
            self.sections[parent].child(child);
        }
    }

    fn sitemap(&mut self, map: &Sitemap) {
        self.notes.sitemap_truncated |= map.truncated;
        self.notes.sitemap_urls = self.notes.sitemap_urls.saturating_add(map.count);
        for (prefix, group) in &map.groups {
            let Some(first) = group.pages.first() else {
                continue;
            };
            let Some(seg) = first.as_url().path_segments().and_then(|mut s| s.next()) else {
                continue;
            };
            let existing = self
                .sections
                .iter()
                .position(|e| e.key.as_str().trim_end_matches('/') == prefix);
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
                let Some(i) = self.section(entry) else {
                    continue;
                };
                i
            };
            self.sections[index].sources.insert(Source::Sitemap);
            for key in &group.pages {
                if self.sections[index].key == *key {
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
                    self.sections[index].child(child);
                }
            }
            self.sections[index].more += group.count.saturating_sub(group.pages.len() as u32);
        }
    }
    pub fn fetch_sections(&self, fresh: bool) -> Vec<&Entry> {
        let mut entries: Vec<_> = self
            .sections
            .iter()
            .filter(|e| {
                e.key != self.entry
                    && !e.children.iter().any(|c| c.source == Source::MenuGroup)
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
        if fresh {
            entries.sort_by_key(|e| {
                !matches!(
                    e.role,
                    Some(SectionRole::News | SectionRole::Changelog | SectionRole::Releases)
                )
            });
        }
        entries
    }
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
    Some(Locator::TextQuote {
        start: LocatorText::new(&opening).ok()?,
        end: None,
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
            key: self.key.clone(),
            locator: Locator::None,
            label: clip(label, TITLE_BYTES),
            role: None,
            external: self.external,
            source,
            sources: [source].into(),
            group: None,
            children: Vec::new(),
            more: 0,
        }
    }
    fn child(&mut self, entry: Entry) {
        if let Some(existing) = self
            .children
            .iter_mut()
            .find(|e| e.key == entry.key && e.locator == entry.locator)
        {
            existing.sources.extend(entry.sources);
            return;
        }
        if self.children.len() < MAX_CHILDREN {
            self.children.push(entry);
        } else {
            self.more += 1;
        }
    }
}

pub fn read_site(doc: &Doc, inputs: &SiteInputs<'_>) -> Result<Place, KeyError> {
    let site = SiteKey::from_url(doc.page()).ok_or(KeyError::Url)?;
    let title = [
        doc.head().site_name.as_str(),
        doc.head().title.as_str(),
        site.as_str(),
    ]
    .into_iter()
    .find(|s| !s.trim().is_empty())
    .unwrap_or_default();
    let mut place = Place {
        title: clip(title, TITLE_BYTES),
        site,
        entry: PageKey::from_url(doc.page())?,
        sections: Vec::new(),
        social: Vec::new(),
        notes: PlaceNotes::default(),
    };
    place.notes.wall = challenge(doc);
    place.notes.entry_truncated = doc.body_truncated();
    place.notes.linked_data_truncated = doc.head().truncated;
    if !place.notes.wall {
        place.navigation(doc, false);
        place.navigation(doc, true);
        let nodes = doc.nodes();
        place.notes.shell = place.notes.links_seen <= SHELL_LINKS
            && nodes.iter().map(|n| n.text.len()).sum::<usize>() <= SHELL_TEXT_BYTES
            && nodes.iter().enumerate().any(|(i, n)| {
                matches!(n.element_id.as_deref(), Some("root" | "app" | "__next"))
                    && n.text.is_empty()
                    && nodes.get(i + 1).is_none_or(|child| child.parent != Some(i))
            });
    } else {
        place.title = place.site.as_str().to_owned();
    }
    place.notes.sitelinks_truncated = inputs.sitelinks.len() > MAX_SITELINKS;
    for l in inputs.sitelinks.iter().take(MAX_SITELINKS) {
        if let Some(e) = place.entry_for(&l.url, &l.title, Source::EngineSitelink) {
            place.section(e);
        }
    }
    if !place.notes.wall {
        place.linked_data(doc);
    }
    if !place.notes.wall && doc.page().path() != "/" {
        let heading = doc
            .nodes()
            .iter()
            .find(|n| matches!(n.role, Role::Heading(Some(1))))
            .map(|n| n.text.as_str())
            .filter(|s| !s.is_empty())
            .unwrap_or(&doc.head().title);
        if let Some(entry) = place
            .entry_for(doc.page(), heading, Source::Heading)
            .filter(|e| e.role.is_some())
        {
            if let Some(i) = place.section(entry) {
                place.section_page(i, doc);
            }
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
    place.notes.sitemap_truncated |= inputs.sitemaps.len() > MAX_SITEMAPS;
    for (key, page) in inputs.pages.iter().take(MAX_SECTION_PAGES) {
        if let Some(i) = place.sections.iter().position(|e| e.key == *key) {
            let (mut preferred, others): (Vec<_>, Vec<_>) =
                std::mem::take(&mut place.sections[i].children)
                    .into_iter()
                    .partition(|e| e.source == Source::MenuGroup);
            place.sections[i].children.append(&mut preferred);
            place.section_page(i, page);
            for c in others {
                place.sections[i].child(c);
            }
        }
    }
    place.notes.section_page_truncated |= inputs.pages.len() > MAX_SECTION_PAGES;
    if let Some(robots) = inputs.robots {
        place.notes.robots_truncated = robots.outcome == RobotsOutcome::Limit;
        place.notes.robots_disallowed = place.fetch_sections(false).iter().any(|e| {
            e.key.as_url().origin() == doc.page().origin()
                && !robots.allows(&e.key.as_url()[url::Position::BeforePath..])
        });
    }
    let first_signup = place
        .sections
        .iter()
        .find(|e| e.role == Some(SectionRole::SignUp))
        .map(|e| (e.key.clone(), e.locator.clone()));
    place.sections.sort_by_cached_key(|e| {
        let duplicate = e.role == Some(SectionRole::SignUp)
            && first_signup
                .as_ref()
                .is_some_and(|(k, l)| *k != e.key || *l != e.locator);
        (
            duplicate || e.utility(),
            e.source != Source::PrimaryNav,
            e.source != Source::Footer,
        )
    });
    place.notes.sections_truncated |= place.sections.len() > MAX_SECTIONS;
    place.sections.truncate(MAX_SECTIONS);
    Ok(place)
}
