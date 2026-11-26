use crate::doc::{Doc, Landmark, Marks, Node, Role};
use crate::flatten;
use crate::key::{PageKey, SiteKey, MAX_URL_BYTES};
use crate::query::{Confidence, QueryRead};
use destination::{Drop, Rules};
use url::Url;
use zephium_core::search::SearchEngine;

mod destination;
mod entity;
pub(crate) use entity::site_profile;

pub const MAX_RESULTS: usize = 16;
pub const MAX_SITELINKS: usize = 8;
pub const MAX_SOCIAL: usize = 10;
pub const MAX_VIDEO: usize = 8;
pub const MAX_IMAGES: usize = 12;
pub const TITLE_BYTES: usize = 300;
pub const SNIPPET_BYTES: usize = 1024;
const MIN_SNIPPET_CHARS: usize = 24;
const PAID_CONTEXT_WORDS: usize = 8;
const LABELS_BEFORE: usize = 3;
const LABELS_AFTER: usize = 4;
const ADDRESS_WORDS: usize = 10;

#[derive(Clone)]
pub struct Link {
    pub title: String,
    pub url: Url,
}
pub struct Image {
    pub title: String,
    pub source: Url,
    pub destination: Url,
}
pub struct Organic {
    pub key: PageKey,
    pub url: Url,
    pub title: String,
    pub snippet: Option<String>,
    pub rank: u32,
    pub official: Option<Official>,
    pub discussion: bool,
    pub sitelinks: Vec<Link>,
}
#[derive(Default)]
pub struct DropCounts {
    pub generated: u32,
    pub paid: u32,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Network {
    Twitter,
    GitHub,
    YouTube,
    LinkedIn,
    Discord,
    Reddit,
    Mastodon,
    Bluesky,
    Instagram,
    Facebook,
}
#[derive(Clone, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum ProfileBasis {
    SameAs,
    Navigation,
    Results,
}
#[derive(Clone, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Profile {
    pub basis: ProfileBasis,
    pub network: Network,
    pub url: Url,
}
pub struct Featured {
    pub text: String,
    pub cites: PageKey,
}
pub struct Official {
    pub site: SiteKey,
    pub confidence: Confidence,
    pub evidence: Evidence,
}
pub struct Evidence {
    pub domain_match: bool,
    pub sitelinks: bool,
    pub panel_website: bool,
}
#[derive(Default)]
pub struct FirstLayer {
    pub featured: Option<Box<Featured>>,
    pub widget: bool,
    pub organic: Vec<Organic>,
    pub social: Vec<Profile>,
    pub video: Vec<Link>,
    pub images: Vec<Image>,
    pub dropped: DropCounts,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wall {
    Challenge,
    Consent,
    NoResults,
    Unreadable,
}
pub enum ResultsRead {
    Wall { wall: Wall },
    FirstLayer { layer: FirstLayer },
}

const AD_LABELS: &[&str] = &[
    "ad",
    "ads",
    "sponsored",
    "sponsored result",
    "sponsored results",
    "promoted",
    "anzeige",
    "annonce",
    "publicité",
    "anuncio",
    "patrocinado",
    "gesponsert",
    "anúncio",
    "anúncios",
    "patrocinada",
    "sponsorisé",
];
/// A page that asks whether a person is there before any results.
pub(crate) const CHALLENGE: &[&str] = &[
    "unusual traffic",
    "not a robot",
    "are you a robot",
    "captcha",
    "verify you are human",
    "verify you're human",
    "confirm you are human",
    "confirm you're human",
    "confirm you're a human",
    "bots use duckduckgo too",
    "solve the challenge",
    "automated queries",
    "checking your browser",
    "just a moment",
    "press and hold",
];
/// A page that asks for a cookie or privacy choice before any results.
const CONSENT: &[&str] = &[
    "before you continue",
    "accept all",
    "reject all",
    "we use cookies",
    "cookie",
    "consent",
];
/// An engine's own words for a query that found nothing.
const NO_RESULTS: &[&str] = &[
    "did not match any documents",
    "no results found",
    "no results for",
    "there are no results",
    "no results were found",
    "returned no results",
    "not many great matches",
];

fn past_site(text: &str, url: &Url) -> Option<String> {
    let host = url.host_str().unwrap_or_default();
    let host = host.trim_start_matches("www.");
    let trimmed = text.trim();
    let lower = trimmed.to_ascii_lowercase();
    let bare = lower
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .trim_start_matches("www.");
    let Some(rest) = bare.strip_prefix(host) else {
        return Some(trimmed.to_owned());
    };
    if !rest.is_empty() && !rest.starts_with([' ', '/', '›', '>']) {
        return Some(trimmed.to_owned());
    }
    // The same words in their own case: the address is ASCII, so its byte
    // length carries over.
    let rest = &trimmed[trimmed.len() - rest.len()..];
    let words: Vec<&str> = rest.split_whitespace().collect();
    let mut at = 0;
    while at < words.len() {
        match words[at] {
            "›" | ">" | "/" => at += 2,
            word if word.starts_with('/') => at += 1,
            _ => break,
        }
    }
    let title = words.get(at..).unwrap_or_default().join(" ");
    (!title.is_empty()).then_some(title)
}

fn past_address(text: &str) -> Option<String> {
    let words: Vec<&str> = text.split(' ').collect();
    let prefix = &words[..words.len().min(ADDRESS_WORDS)];
    let last_crumb = prefix.iter().rposition(|word| *word == "›");
    let mut end = None;
    let mut crumb = false;
    for (at, word) in prefix.iter().enumerate() {
        if address_like(word) || *word == "›" || crumb {
            end = Some(at + 1);
            crumb = *word == "›";
        } else if end.is_some() && last_crumb.is_none_or(|last| at > last) {
            break;
        }
    }
    let rest = words[end?..].join(" ");
    (!rest.trim().is_empty()).then(|| rest.trim().to_owned())
}

fn ad_label(text: &str) -> bool {
    let text = flatten(text).to_lowercase();
    AD_LABELS.contains(&text.trim_matches(|c: char| c == '·' || c == ':' || c.is_whitespace()))
}

/// A displayed address or breadcrumb rather than prose.
fn address_like(text: &str) -> bool {
    let text = text.trim();
    text.starts_with("http://")
        || text.starts_with("https://")
        || text.contains(" › ")
        || (!text.contains(' ') && text.contains('.'))
}

pub(crate) fn clip(text: &str, bytes: usize) -> String {
    if text.len() <= bytes {
        return text.to_owned();
    }
    let mut end = bytes;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].trim_end().to_owned()
}

#[derive(Clone, Copy, Default)]
struct Context {
    end: usize,
    hidden: bool,
    chrome: bool,
    main: bool,
    panel: bool,
    paid: bool,
    generated: bool,
    featured: bool,
    widget: bool,
    mixed: bool,
}
struct Page<'a> {
    nodes: &'a [Node],
    context: Vec<Context>,
    has_main: bool,
}
struct Title {
    at: usize,
    text: String,
    destination: Result<Url, Drop>,
    heading: Option<usize>,
}

fn words(node: &Node) -> &str {
    if node.text.trim().is_empty() {
        &node.name
    } else {
        &node.text
    }
}
fn prose(role: Role) -> bool {
    matches!(role, Role::Document | Role::Paragraph | Role::ListItem)
}
fn container(role: Role) -> bool {
    prose(role) || role == Role::Group
}

impl<'a> Page<'a> {
    fn new(nodes: &'a [Node]) -> Self {
        let mut context: Vec<Context> = Vec::with_capacity(nodes.len());
        let mut has_main = false;
        for (at, n) in nodes.iter().enumerate() {
            let mut state = n.parent_index().map(|p| context[p]).unwrap_or_default();
            state.end = at + 1;
            state.generated |= n.marks.has(Marks::GENERATED)
                || Marks::from_label(&n.name) == Some(Marks::GENERATED);
            state.hidden |= n.hidden || state.generated;
            state.featured =
                n.marks.has(Marks::FEATURED) || Marks::from_label(&n.name) == Some(Marks::FEATURED);
            state.widget =
                n.marks.has(Marks::WIDGET) || Marks::from_label(&n.name) == Some(Marks::WIDGET);
            state.paid |= n.marks.has(Marks::SPONSORED) || ad_label(&n.name);
            state.main |= n.role
                == Role::Landmark {
                    landmark: Some(Landmark::Main),
                };
            state.panel |= n.role
                == Role::Landmark {
                    landmark: Some(Landmark::Complementary),
                }
                || n.role
                    == Role::Landmark {
                        landmark: Some(Landmark::Region),
                    }
                    && ["knowledge panel", "knowledge graph"]
                        .iter()
                        .any(|s| n.name.eq_ignore_ascii_case(s));
            state.chrome |= matches!(
                n.role,
                Role::Landmark {
                    landmark: Some(
                        Landmark::Navigation
                            | Landmark::Contentinfo
                            | Landmark::Banner
                            | Landmark::Complementary
                    )
                }
            );
            has_main |= state.main && !state.hidden;
            context.push(state);
        }
        let mut targets = vec![0usize; nodes.len()];
        for at in (0..nodes.len()).rev() {
            targets[at] += usize::from(
                !context[at].hidden && matches!(nodes[at].role, Role::Link | Role::Image),
            );
            if let Some(p) = nodes[at].parent_index() {
                context[p].end = context[p].end.max(context[at].end);
                targets[p] += targets[at];
            }
        }
        for (at, node) in nodes.iter().enumerate() {
            if !context[at].hidden
                && matches!(
                    node.role,
                    Role::Heading { level: _ } | Role::Text | Role::Paragraph
                )
            {
                let mark = Marks::from_label(words(node));
                if matches!(
                    mark,
                    Some(Marks::GENERATED | Marks::FEATURED | Marks::WIDGET)
                ) {
                    let card =
                        std::iter::successors(node.parent_index(), |&p| nodes[p].parent_index())
                            .take_while(|&p| {
                                !matches!(
                                    nodes[p].role,
                                    Role::Landmark {
                                        landmark: Some(Landmark::Main)
                                    } | Role::List
                                )
                            })
                            .find(|&p| {
                                nodes[p].parent_index().is_some()
                                    && (container(nodes[p].role)
                                        || nodes[p].role
                                            == Role::Landmark {
                                                landmark: Some(Landmark::Region),
                                            })
                            })
                            .unwrap_or(at);
                    match mark {
                        Some(Marks::GENERATED) => context[card].generated = true,
                        Some(Marks::FEATURED) => context[card].featured = true,
                        Some(Marks::WIDGET) => context[card].widget = true,
                        _ => {}
                    }
                }
            }
            if !context[at].hidden && ad_label(words(node)) {
                let card = std::iter::successors(node.parent_index(), |&p| nodes[p].parent_index())
                    .take_while(|&p| {
                        !matches!(nodes[p].role, Role::Landmark { landmark: _ } | Role::List)
                    })
                    .find(|&p| {
                        nodes[p].parent_index().is_some()
                            && container(nodes[p].role)
                            && targets[p] > 0
                    });
                if let Some(p) = card {
                    context[p].paid = true;
                }
            }
        }
        for (at, node) in nodes.iter().enumerate() {
            if node.parent_index().is_some_and(|p| context[p].generated) {
                context[at].generated = true;
            }
            context[at].hidden |= context[at].generated;
            context[at].widget |= node.parent_index().is_some_and(|p| context[p].widget);
            if node.parent_index().is_some_and(|p| context[p].paid) {
                context[at].paid = true;
            }
        }
        for (i, n) in nodes.iter().enumerate().rev() {
            if let Some(p) = n.parent_index() {
                context[p].mixed |= context[i].mixed || context[i].generated || context[i].widget;
            }
        }
        Self {
            nodes,
            context,
            has_main,
        }
    }

    fn ancestors(&self, at: usize) -> impl Iterator<Item = usize> + '_ {
        std::iter::successors(self.nodes[at].parent_index(), |&p| {
            self.nodes[p].parent_index()
        })
    }
    fn eligible(&self, at: usize) -> bool {
        let c = self.context[at];
        !c.hidden && !c.chrome && !c.widget && (!self.has_main || c.main)
    }
    fn titles(&self, rules: &Rules) -> Vec<Title> {
        let mut titles = Vec::new();
        for (at, n) in self.nodes.iter().enumerate() {
            if n.role != Role::Link
                || !self.eligible(at)
                || self.context[at].mixed
                || self.ancestors(at).any(|p| self.nodes[p].role == Role::Link)
            {
                continue;
            }
            let Some(source) = &n.destination else {
                continue;
            };
            let heading = (at + 1..self.context[at].end)
                .find(|&i| {
                    !self.context[i].hidden
                        && matches!(self.nodes[i].role, Role::Heading { level: _ })
                })
                .or_else(|| {
                    self.ancestors(at)
                        .find(|&p| matches!(self.nodes[p].role, Role::Heading { level: _ }))
                });
            let destination = rules.destination(source);
            let text = if let Some(h) = heading {
                let own = words(n);
                let head = words(&self.nodes[h]);
                // Sibling address and title links can share a heading.
                if h < at && !own.is_empty() {
                    Some(flatten(own))
                } else if !head.is_empty() {
                    Some(flatten(head))
                } else {
                    Some(flatten(own))
                }
            } else if rules.engine == SearchEngine::Brave {
                past_address(&flatten(words(n)))
            } else {
                None
            };
            let Some(text) = text
                .and_then(|text| past_site(&text, destination.as_ref().unwrap_or(source)))
                .filter(|s| !s.is_empty())
            else {
                continue;
            };
            titles.push(Title {
                at,
                text,
                destination,
                heading,
            });
        }
        titles
    }

    fn nested_sitelink(&self, primary: &Title, next: &Title) -> bool {
        let card = self
            .ancestors(primary.at)
            .take_while(|&p| {
                !matches!(
                    self.nodes[p].role,
                    Role::Landmark { landmark: _ } | Role::List
                )
            })
            .find(|&p| {
                self.nodes[p].parent_index().is_some()
                    && (matches!(self.nodes[p].role, Role::ListItem | Role::Document)
                        || self.nodes[p].role == Role::Group && self.context[p].end > next.at)
            });
        let Some(card) = card.filter(|&p| next.at < self.context[p].end) else {
            return false;
        };
        let nested_list = self
            .ancestors(next.at)
            .take_while(|&p| p > card)
            .any(|p| matches!(self.nodes[p].role, Role::List | Role::Table) && p > primary.at);
        let level = |title: &Title| {
            title.heading.and_then(|i| match self.nodes[i].role {
                Role::Heading { level: n } => n,
                _ => None,
            })
        };
        let lower_heading = level(primary)
            .zip(level(next))
            .is_some_and(|(first, second)| second > first);
        nested_list
            || lower_heading
                && (self.nodes[card].role != Role::Group
                    || primary.heading.is_some_and(|h| {
                        self.nodes[h].parent_index() == Some(card)
                            || self.nodes[primary.at].parent_index() == Some(card)
                    }))
    }

    fn primary_titles(&self, rules: &Rules) -> Vec<Title> {
        let mut primary = Vec::new();
        let mut site = None;
        for title in self.titles(rules) {
            let destination_site = title.destination.as_ref().ok().and_then(SiteKey::from_url);
            if site.is_some()
                && site == destination_site
                && primary
                    .last()
                    .is_some_and(|p| self.nested_sitelink(p, &title))
            {
                continue;
            }
            site = destination_site;
            primary.push(title);
        }
        primary
    }

    fn scope(&self, title: &Title, prev: Option<usize>, next: usize) -> Option<usize> {
        self.ancestors(title.at)
            .take_while(|&p| {
                !matches!(
                    self.nodes[p].role,
                    Role::Landmark { landmark: _ } | Role::List
                ) && prev.is_none_or(|v| v < p)
                    && self.context[p].end <= next
            })
            .filter(|&p| container(self.nodes[p].role) && self.nodes[p].parent_index().is_some())
            .last()
    }

    fn paid(
        &self,
        title: &Title,
        scope: Option<usize>,
        start: usize,
        end: usize,
        next_prefix: Option<usize>,
    ) -> bool {
        if self.context[title.at].paid || matches!(title.destination, Err(Drop::Paid)) {
            return true;
        }
        let card = scope.filter(|&p| matches!(self.nodes[p].role, Role::Document | Role::ListItem));
        let start = card.unwrap_or(start);
        let end = card.map_or(end, |p| self.context[p].end);
        let label = |i: usize| {
            !self.context[i].hidden && !self.context[i].paid && ad_label(words(&self.nodes[i]))
        };
        if let Some(p) = scope {
            if (p..self.context[p].end).any(label) {
                return true;
            }
        }
        let inside_end = self.context[title.at].end;
        if (start..title.at)
            .rev()
            .filter(|&i| !words(&self.nodes[i]).is_empty())
            .take(LABELS_BEFORE)
            .any(label)
            || (title.at + 1..inside_end).any(label)
            || (inside_end..end)
                .filter(|&i| !words(&self.nodes[i]).is_empty())
                .take(LABELS_AFTER)
                .any(|i| next_prefix.is_none_or(|start| i < start) && label(i))
        {
            return true;
        }
        self.ancestors(title.at)
            .take_while(|&p| scope.is_some_and(|s| p >= s))
            .filter(|&p| prose(self.nodes[p].role))
            .any(|p| {
                let mut rest = flatten(&self.nodes[p].text).replace(&title.text, " ");
                for i in p + 1..self.context[p].end {
                    if self.nodes[i].role == Role::Paragraph && !self.nodes[i].text.is_empty() {
                        rest = rest.replace(&self.nodes[i].text, " ");
                    }
                }
                let mut words = rest.split_whitespace();
                words.clone().take(PAID_CONTEXT_WORDS + 1).count() <= PAID_CONTEXT_WORDS
                    && words.any(ad_label)
            })
    }

    fn snippet(&self, title: &Title, scope: Option<usize>, end: usize) -> Option<(usize, String)> {
        let start = scope.unwrap_or(title.at);
        let end = scope
            .filter(|&p| matches!(self.nodes[p].role, Role::Document | Role::ListItem))
            .map_or(end, |p| self.context[p].end);
        let mut best = None;
        for i in start..end {
            let n = &self.nodes[i];
            if !self.eligible(i)
                || self.context[i].paid
                || self.context[i].mixed
                || !prose(n.role)
                || (i < title.at && self.context[i].end <= title.at)
                || self.context[i].end > end
                || (title.at..self.context[title.at].end).contains(&i)
            {
                continue;
            }
            let text = flatten(&n.text);
            let after = if i < title.at && self.context[i].end > title.at {
                text.split_once(&title.text)
                    .map_or(text.as_str(), |(_, rest)| rest)
            } else {
                &text
            };
            let text = after
                .split_whitespace()
                .skip_while(|s| address_like(s))
                .collect::<Vec<_>>()
                .join(" ");
            if text.chars().count() >= MIN_SNIPPET_CHARS
                && !address_like(&text)
                && best
                    .as_ref()
                    .is_none_or(|(_, s): &(usize, String)| text.len() >= s.len())
            {
                best = Some((i, text));
            }
        }
        best.map(|(at, s)| (at, clip(&s, SNIPPET_BYTES)))
    }
}

pub fn read_results(
    engine: SearchEngine,
    own_host: Option<&str>,
    query: &QueryRead,
    doc: &Doc,
) -> ResultsRead {
    read_results_with_stats(engine, own_host, query, doc, &mut ResultStats::default())
}

#[derive(Default, Clone, Debug, PartialEq, Eq)]
pub struct ResultStats {
    pub own: u32,
    pub repeat: u32,
}
pub fn read_results_with_stats(
    engine: SearchEngine,
    own_host: Option<&str>,
    query: &QueryRead,
    doc: &Doc,
    counts: &mut ResultStats,
) -> ResultsRead {
    let own = own_host.unwrap_or_else(|| doc.page().host_str().unwrap_or_default());
    if own.len() > MAX_URL_BYTES || own.is_empty() {
        return ResultsRead::Wall {
            wall: Wall::Unreadable,
        };
    }
    if doc.body_truncated() {
        return ResultsRead::Wall {
            wall: Wall::Unreadable,
        };
    }
    let normalized_host = own.trim_end_matches('.').to_ascii_lowercase();
    let rules = Rules {
        engine,
        own: &normalized_host,
        page_host: doc.page().host_str().unwrap_or_default(),
    };
    let page = Page::new(doc.nodes());
    let titles = page.primary_titles(&rules);
    let scopes: Vec<_> = titles
        .iter()
        .enumerate()
        .map(|(pos, t)| {
            let previous = pos.checked_sub(1).map(|p| titles[p].at);
            let next = titles.get(pos + 1).map_or(page.nodes.len(), |t| t.at);
            page.scope(t, previous, next)
        })
        .collect();
    let mut layer = FirstLayer::default();
    let mut first_title = None;
    let mut seen = std::collections::BTreeSet::new();
    let mut excluded = vec![false; page.nodes.len()];
    for (pos, title) in titles.iter().enumerate() {
        let start = pos.checked_sub(1).map_or(0, |p| {
            scopes[p].map_or(titles[p].at + 1, |i| page.context[i].end)
        });
        let end = titles
            .get(pos + 1)
            .map_or(page.nodes.len(), |n| scopes[pos + 1].unwrap_or(n.at));
        let scope = scopes[pos];
        let next_prefix = titles.get(pos + 1).and_then(|next| {
            let next_start = scopes[pos + 1]
                .filter(|&p| matches!(page.nodes[p].role, Role::Document | Role::ListItem))
                .or_else(|| scope.map(|i| page.context[i].end))
                .unwrap_or(title.at + 1);
            (next_start..next.at)
                .rev()
                .filter(|&i| !words(&page.nodes[i]).is_empty())
                .take(LABELS_BEFORE)
                .last()
        });
        if page.paid(title, scope, start, end, next_prefix) {
            layer.dropped.paid += 1;
            excluded[scope.unwrap_or(title.at)..scope.map_or(end, |p| page.context[p].end)]
                .fill(true);
            continue;
        }
        let url = match &title.destination {
            Ok(url) => url,
            Err(Drop::Own) => {
                counts.own = counts.own.saturating_add(1);
                continue;
            }
            Err(_) => continue,
        };
        let Ok(key) = PageKey::from_url(url) else {
            continue;
        };
        if !seen.insert(key.page_match()) {
            counts.repeat = counts.repeat.saturating_add(1);
            continue;
        }
        if layer.organic.len() >= MAX_RESULTS {
            continue;
        }
        let sitelinks = page.sitelinks(title, scope, &rules);
        let snippet = page.snippet(title, scope, end).map(|(_, text)| text);
        first_title.get_or_insert(title.at);
        layer.organic.push(Organic {
            key,
            url: url.clone(),
            title: clip(&title.text, TITLE_BYTES),
            snippet,
            rank: layer.organic.len() as u32 + 1,
            official: None,
            discussion: false,
            sitelinks,
        });
    }
    page.groups(&mut layer, &rules, first_title.unwrap_or(page.nodes.len()));
    page.enrich(&mut layer, &rules, query, &excluded);
    if layer.organic.is_empty() {
        for (phrases, wall) in [
            (CHALLENGE, Wall::Challenge),
            (NO_RESULTS, Wall::NoResults),
            (CONSENT, Wall::Consent),
        ] {
            if std::iter::once(doc.head().title.as_str())
                .chain(
                    page.nodes
                        .iter()
                        .enumerate()
                        .filter(|(i, _)| !page.context[*i].hidden)
                        .map(|(_, n)| words(n)),
                )
                .any(|s| {
                    let s = flatten(s).to_lowercase();
                    phrases.iter().any(|p| s.contains(p))
                })
            {
                return ResultsRead::Wall { wall };
            }
        }
        if layer.images.is_empty() && layer.video.is_empty() && !layer.widget {
            return ResultsRead::Wall {
                wall: Wall::Unreadable,
            };
        }
    }
    ResultsRead::FirstLayer { layer }
}

impl Page<'_> {
    fn groups(&self, layer: &mut FirstLayer, rules: &Rules, first: usize) {
        for (i, n) in self.nodes.iter().enumerate() {
            let c = self.context[i];
            if c.generated && !n.parent_index().is_some_and(|p| self.context[p].generated) {
                layer.dropped.generated += 1;
            }
            if c.hidden || c.paid {
                continue;
            }
            layer.widget |= i < first && c.widget;
            if c.featured && layer.featured.is_none() {
                let mut citation = None;
                let mut body = None;
                for j in i..c.end {
                    let node = &self.nodes[j];
                    if self.context[j].hidden || self.context[j].paid {
                        continue;
                    }
                    if node.role == Role::Paragraph
                        && !self.context[j].mixed
                        && !node.text.trim().is_empty()
                    {
                        body.get_or_insert(&node.text);
                    }
                    if let Some(url) = node
                        .destination
                        .as_ref()
                        .and_then(|u| rules.destination(u).ok())
                        .and_then(|u| PageKey::from_url(&u).ok())
                    {
                        citation = citation.or_else(|| {
                            layer
                                .organic
                                .iter()
                                .any(|r| r.key.page_match() == url.page_match())
                                .then_some(url)
                        });
                    }
                }
                layer.featured = citation
                    .zip(body)
                    .map(|(cites, text)| Featured {
                        cites,
                        text: clip(text, SNIPPET_BYTES),
                    })
                    .map(Box::new);
            }
        }
        for r in &mut layer.organic {
            let host = r.url.host_str().unwrap_or_default();
            r.discussion = [
                "reddit.com",
                "news.ycombinator.com",
                "stackexchange.com",
                "stackoverflow.com",
                "superuser.com",
                "serverfault.com",
                "askubuntu.com",
                "mathoverflow.net",
                "producthunt.com",
                "g2.com",
                "trustpilot.com",
                "quora.com",
            ]
            .iter()
            .any(|d| destination::on(host, d))
                || host
                    .split('.')
                    .next()
                    .is_some_and(|s| matches!(s, "forum" | "forums" | "discuss" | "community"));
        }
    }
}

#[cfg(test)]
#[test]
fn address_prefix_does_not_consume_address_like_title_words() {
    assert_eq!(
        past_address("example.net › docs New Node.js Guide").as_deref(),
        Some("New Node.js Guide")
    );
}

#[cfg(test)]
#[test]
fn address_prefix_consumes_multiword_breadcrumb_segments() {
    assert_eq!(
        past_address("example.net › getting started › install Install Guide").as_deref(),
        Some("Install Guide")
    );
}
