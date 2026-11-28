//! A search engine's results page read as its results, with no model: each
//! title link's real destination, its title and the engine's own snippet,
//! in the engine's order. Ads, the engine's own links and repeats are left
//! out; a page that shows no result says what stood in its way instead.

use base64::Engine as _;
use zephium_core::search::SearchEngine;
use zephium_core::work::search::{
    WorkSearchHit, WorkSearchPage, WorkSearchWall, BROWSER_SEARCH_SNIPPET_BYTES,
};

use crate::*;

/// Results a page read keeps: enough to choose from, few enough to read.
pub const SEARCH_PAGE_RESULTS: usize = 8;
const TITLE_BYTES: usize = 300;
/// Below this a paragraph is a label or a date, not a snippet.
const MIN_SNIPPET_CHARS: usize = 24;

/// Words that mark a result as paid, as engines print them beside it.
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
];
/// A page that asks whether a person is there before any results.
const CHALLENGE: &[&str] = &[
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

/// What a results page showed, and the nodes a read of it cites: each
/// result's title link and snippet, or the node that stood in the way.
pub struct SearchPageRead {
    /// The results, or the wall.
    pub page: WorkSearchPage,
    /// One row of nodes per result, or the one node of the wall.
    pub cited: Vec<Vec<SemanticReferenceId>>,
}

/// Reads `observation`, a whole look at `engine`'s results page. `own` is
/// a custom engine's host, whose links are the engine's own.
pub fn read_search_page(
    engine: SearchEngine,
    own: Option<&str>,
    observation: &SemanticObservation,
) -> SearchPageRead {
    let Some(frame) = observation.frames().first() else {
        return wall(WorkSearchWall::Unreadable, None);
    };
    let page = Page::new(frame.nodes());
    let rules = Rules { engine, own };
    let titles = page.titles(&rules);
    let mut hits: Vec<WorkSearchHit> = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    let mut cited = Vec::new();
    for (position, title) in titles.iter().enumerate() {
        if hits.len() >= SEARCH_PAGE_RESULTS {
            break;
        }
        let end = titles
            .get(position + 1)
            .map_or(page.nodes.len(), |t| t.link);
        let start = position.checked_sub(1).map_or(0, |at| titles[at].link + 1);
        let others: Vec<usize> = titles
            .iter()
            .filter(|other| other.link != title.link)
            .map(|other| other.link)
            .collect();
        if page.paid(title, start, end, &others) {
            continue;
        }
        let Some(url) = rules.destination(page.nodes[title.link].link_destination()) else {
            continue;
        };
        let key = canonical(&url);
        if seen.contains(&key) {
            continue;
        }
        seen.push(key);
        let snippet = page.snippet(title, end, &others);
        let mut nodes = vec![page.nodes[title.link].reference()];
        if let Some((at, _)) = &snippet {
            nodes.push(page.nodes[*at].reference());
        }
        cited.push(nodes);
        hits.push(WorkSearchHit {
            title: title.text.clone(),
            url,
            snippet: snippet.map(|(_, text)| text),
        });
    }
    if !hits.is_empty() {
        return SearchPageRead {
            page: WorkSearchPage::Results(hits),
            cited,
        };
    }
    let found = |words: &[&str]| page.saying(words);
    if let Some(at) = found(CHALLENGE) {
        return wall(WorkSearchWall::Challenge, Some(page.nodes[at].reference()));
    }
    if let Some(at) = found(NO_RESULTS) {
        return wall(WorkSearchWall::NoResults, Some(page.nodes[at].reference()));
    }
    if let Some(at) = found(CONSENT) {
        return wall(WorkSearchWall::Consent, Some(page.nodes[at].reference()));
    }
    wall(WorkSearchWall::Unreadable, page.first_text())
}

fn wall(wall: WorkSearchWall, node: Option<SemanticReferenceId>) -> SearchPageRead {
    SearchPageRead {
        page: WorkSearchPage::Wall(wall),
        cited: node.map(|node| vec![vec![node]]).unwrap_or_default(),
    }
}

/// One result's title: its link, and the heading or words that name it.
struct Title {
    link: usize,
    /// Nodes inside the link, which never stand as its snippet.
    inside: Vec<usize>,
    text: String,
}

struct Rules<'a> {
    engine: SearchEngine,
    own: Option<&'a str>,
}

impl Rules<'_> {
    /// The engine's own host, whose links are navigation, not results.
    fn own_host(&self, host: &str) -> bool {
        let host = host.trim_end_matches('.').to_ascii_lowercase();
        let on = |site: &str| host == site || host.ends_with(&format!(".{site}"));
        // Google's search host in any country; its other services can be results.
        let google = host
            .trim_start_matches("www.")
            .strip_prefix("google.")
            .is_some_and(|tld| tld.split('.').all(|part| part.len() <= 3))
            || on("gstatic.com");
        let ads = on("googleadservices.com") || on("doubleclick.net");
        ads || match self.engine {
            SearchEngine::Google => google,
            SearchEngine::Bing => on("bing.com") || on("bing.net"),
            SearchEngine::DuckDuckGo => on("duckduckgo.com") || on("duck.com"),
            SearchEngine::Brave => on("search.brave.com"),
            SearchEngine::Kagi => on("kagi.com"),
            SearchEngine::Custom => self.own.is_some_and(on),
        }
    }

    /// The destination a result link leads to: the engine's redirect
    /// unwrapped, and nothing when it stays on the engine or is not http(s).
    fn destination(&self, target: Option<&ContextNavigationTarget>) -> Option<String> {
        let mut url = target?.as_url().clone();
        for _ in 0..2 {
            if !matches!(url.scheme(), "http" | "https") {
                return None;
            }
            let host = url.host_str()?.to_owned();
            if !self.own_host(&host) {
                url.set_fragment(None);
                return (url.as_str().len() <= 2048 && url.username().is_empty())
                    .then(|| url.into());
            }
            url = unwrap_redirect(&url)?;
        }
        None
    }
}

/// A result's words without the address it leads to in front: its host,
/// with or without `www.`, and a breadcrumb of its path. None when the words
/// are only that address.
fn past_site(text: &str, url: &str) -> Option<String> {
    let Some(host) = url::Url::parse(url)
        .ok()
        .and_then(|url| url.host_str().map(str::to_ascii_lowercase))
    else {
        return Some(text.to_owned());
    };
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

/// The page a tracking redirect stands for: Google's `/url?q=`, Bing's
/// `/ck/a?u=a1<base64>`, DuckDuckGo's `/l/?uddg=` and any engine's plain
/// `url=` parameter.
fn unwrap_redirect(url: &url::Url) -> Option<url::Url> {
    let path = url.path();
    let value = |keys: &[&str]| {
        url.query_pairs()
            .find(|(key, _)| keys.contains(&key.as_ref()))
            .map(|(_, value)| value.into_owned())
    };
    let raw = if path == "/ck/a" {
        let encoded = value(&["u"])?;
        let encoded = encoded.strip_prefix("a1")?;
        let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(encoded.trim_end_matches('='))
            .ok()?;
        String::from_utf8(bytes).ok()?
    } else if path == "/url" {
        value(&["q", "url"])?
    } else if path.starts_with("/l") {
        value(&["uddg"])?
    } else if path.contains("aclk") || path.contains("y.js") {
        // An ad's click: never followed.
        return None;
    } else {
        value(&["url", "u", "dest", "target"])?
    };
    let target = url::Url::parse(&raw).ok()?;
    matches!(target.scheme(), "http" | "https").then_some(target)
}

/// A URL as the same page: no fragment, host without `www.`, no trailing
/// slash and no campaign parameters.
fn canonical(url: &str) -> String {
    let Ok(mut parsed) = url::Url::parse(url) else {
        return url.to_owned();
    };
    parsed.set_fragment(None);
    let kept: Vec<(String, String)> = parsed
        .query_pairs()
        .filter(|(key, _)| !key.starts_with("utm_") && key != "ref" && key != "fbclid")
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect();
    parsed.set_query(None);
    if !kept.is_empty() {
        parsed.query_pairs_mut().extend_pairs(kept);
    }
    let host = parsed
        .host_str()
        .unwrap_or_default()
        .trim_start_matches("www.")
        .to_ascii_lowercase();
    format!(
        "{host}{}{}",
        parsed.path().trim_end_matches('/'),
        parsed.query().map(|q| format!("?{q}")).unwrap_or_default()
    )
}

struct Page<'a> {
    nodes: &'a [SemanticNode],
    /// Inside the page's header, navigation, footer or sidebar.
    chrome: Vec<bool>,
    /// Inside the page's main landmark.
    main: Vec<bool>,
}

impl<'a> Page<'a> {
    fn new(nodes: &'a [SemanticNode]) -> Self {
        let mut chrome = vec![false; nodes.len()];
        let mut main = vec![false; nodes.len()];
        for (index, node) in nodes.iter().enumerate() {
            let parent = node.parent().map(usize::from).filter(|at| *at < index);
            let (inherited_chrome, inherited_main) =
                parent.map_or((false, false), |at| (chrome[at], main[at]));
            let kind = (node.role() == SemanticRole::Landmark)
                .then(|| node.landmark_kind())
                .flatten();
            chrome[index] = inherited_chrome
                || matches!(
                    kind,
                    Some(
                        SemanticLandmarkKind::Navigation
                            | SemanticLandmarkKind::Banner
                            | SemanticLandmarkKind::Contentinfo
                            | SemanticLandmarkKind::Complementary
                    )
                );
            main[index] = inherited_main || kind == Some(SemanticLandmarkKind::Main);
        }
        Self {
            nodes,
            chrome,
            main,
        }
    }

    fn ancestors(&self, index: usize) -> impl Iterator<Item = usize> + '_ {
        std::iter::successors(self.parent(index), move |at| self.parent(*at))
    }

    fn parent(&self, index: usize) -> Option<usize> {
        self.nodes[index]
            .parent()
            .map(usize::from)
            .filter(|at| *at < index)
    }

    fn within(&self, index: usize, ancestor: usize) -> bool {
        self.ancestors(index).any(|at| at == ancestor)
    }

    fn words(&self, index: usize) -> Option<String> {
        let node = &self.nodes[index];
        let text = node
            .text()
            .or_else(|| node.name())
            .map(SemanticText::as_str)?;
        let text = flatten(text);
        (!text.is_empty()).then_some(text)
    }

    /// Each result's title link, in page order: a link out of the engine
    /// that a heading names (inside it or around it), or on Brave, whose
    /// result links hold their title as text.
    fn titles(&self, rules: &Rules<'_>) -> Vec<Title> {
        let has_main = (0..self.nodes.len()).any(|at| self.main[at]);
        let mut titles: Vec<Title> = Vec::new();
        for (index, node) in self.nodes.iter().enumerate() {
            if node.role() != SemanticRole::Link
                || self.chrome[index]
                || (has_main && !self.main[index])
                || rules.destination(node.link_destination()).is_none()
                || titles.iter().any(|title| self.within(index, title.link))
            {
                continue;
            }
            let inside: Vec<usize> = (index + 1..self.nodes.len())
                .take_while(|at| self.within(*at, index))
                .collect();
            let heading = inside
                .iter()
                .copied()
                .find(|at| self.nodes[*at].role() == SemanticRole::Heading)
                .or_else(|| {
                    self.ancestors(index)
                        .find(|at| self.nodes[*at].role() == SemanticRole::Heading)
                });
            let text = match heading {
                Some(at) => self.words(at).or_else(|| self.words(index)),
                // Brave's result link holds its site, address and title as
                // one name: the title follows the address.
                None if rules.engine == SearchEngine::Brave => {
                    self.words(index).and_then(|name| past_address(&name))
                }
                None => None,
            };
            let Some(text) = text else { continue };
            // A result's address ("allegro.pl", "tokio.rs › docs") names its
            // site, not its page: alone it is the address link, and the title
            // link comes after it; read with the title, it is left off.
            let Some(text) = rules
                .destination(node.link_destination())
                .map_or(Some(text.clone()), |url| past_site(&text, &url))
            else {
                continue;
            };
            titles.push(Title {
                link: index,
                inside,
                text: clip(&text, TITLE_BYTES),
            });
        }
        titles
    }

    /// The result an engine marks as paid: a label beside it in its own
    /// container, before its title, or in a region named for ads.
    fn paid(&self, title: &Title, start: usize, end: usize, others: &[usize]) -> bool {
        let label = |at: usize| self.words(at).is_some_and(|text| ad_label(&text));
        if self.ancestors(title.link).any(|at| {
            self.nodes[at]
                .name()
                .is_some_and(|name| ad_label(name.as_str()))
        }) {
            return true;
        }
        // A label in the result's own container reads among its few words
        // besides the title and the snippet: its address, its date.
        let beside = self.container(title, others).is_some_and(|at| {
            let Some(text) = self.nodes[at].text() else {
                return false;
            };
            let mut rest = flatten(text.as_str()).replace(&title.text, " ");
            for prose in
                (at + 1..end).filter(|node| self.nodes[*node].role() == SemanticRole::Paragraph)
            {
                if let Some(words) = self.words(prose) {
                    rest = rest.replace(&words, " ");
                }
            }
            let words: Vec<&str> = rest.split_whitespace().collect();
            words.len() <= 8 && words.iter().any(|word| ad_label(word))
        });
        // Labels print just above the title or within the result's first lines.
        beside
            || (start..title.link).rev().take(3).any(label)
            || (title.link + 1..end)
                .filter(|at| !title.inside.contains(at))
                .take(4)
                .any(label)
            || title.inside.iter().copied().any(label)
    }

    /// The prose containers (list item, article) that hold this result and
    /// no other, nearest first.
    fn containers(&self, title: &Title, others: &[usize]) -> Vec<usize> {
        self.ancestors(title.link)
            .filter(|at| {
                matches!(
                    self.nodes[*at].role(),
                    SemanticRole::Paragraph | SemanticRole::ListItem | SemanticRole::Document
                )
            })
            .take_while(|at| !others.iter().any(|other| self.within(*other, *at)))
            .collect()
    }

    fn container(&self, title: &Title, others: &[usize]) -> Option<usize> {
        self.containers(title, others).into_iter().next()
    }

    /// The engine's snippet for a result: the longest prose after its title
    /// and before the next, or its own container's words past the title,
    /// never an address or a breadcrumb. `others` are the other titles.
    fn snippet(&self, title: &Title, end: usize, others: &[usize]) -> Option<(usize, String)> {
        let prose = |at: &usize| {
            matches!(
                self.nodes[*at].role(),
                SemanticRole::Paragraph | SemanticRole::ListItem | SemanticRole::Document
            )
        };
        // A result's list item or article holds its words when no prose
        // node of its own does.
        self.containers(title, others)
            .into_iter()
            .chain((title.link + 1..end).filter(|at| {
                !title.inside.contains(at) && !others.iter().any(|other| self.within(*other, *at))
            }))
            .filter(prose)
            .filter_map(|at| Some((at, flatten(self.nodes[at].text()?.as_str()))))
            .map(|(at, text)| {
                let after = match text.find(&title.text) {
                    Some(start) => text[start + title.text.len()..].to_owned(),
                    None => text,
                };
                let words: Vec<&str> = after
                    .split(' ')
                    .skip_while(|word| word.is_empty() || address_like(word))
                    .collect();
                (at, words.join(" "))
            })
            .filter(|(_, text)| text.chars().count() >= MIN_SNIPPET_CHARS && !address_like(text))
            .max_by_key(|(_, text)| text.len())
            .map(|(at, text)| (at, clip(&text, BROWSER_SEARCH_SNIPPET_BYTES)))
    }

    /// The first node whose words hold one of `phrases`.
    fn saying(&self, phrases: &[&str]) -> Option<usize> {
        (0..self.nodes.len()).find(|at| {
            self.words(*at).is_some_and(|text| {
                let text = text.to_lowercase();
                phrases.iter().any(|phrase| text.contains(phrase))
            })
        })
    }

    fn first_text(&self) -> Option<SemanticReferenceId> {
        (0..self.nodes.len())
            .find(|at| self.words(*at).is_some())
            .map(|at| self.nodes[at].reference())
    }
}

/// The words after a displayed address and its breadcrumb path near the
/// start of `text`; none when no address leads it.
fn past_address(text: &str) -> Option<String> {
    let words: Vec<&str> = text.split(' ').collect();
    let mut end = None;
    let mut crumb = false;
    for (at, word) in words.iter().enumerate().take(10) {
        if address_like(word) || *word == "›" || crumb {
            end = Some(at + 1);
            crumb = *word == "›";
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

/// One line of text: control characters and runs of space become one space.
fn flatten(text: &str) -> String {
    text.split(|c: char| c.is_whitespace() || c.is_control())
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

fn clip(text: &str, bytes: usize) -> String {
    if text.len() <= bytes {
        return text.to_owned();
    }
    let mut end = bytes;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].trim_end().to_owned()
}

#[cfg(test)]
#[path = "results_tests.rs"]
mod tests;
