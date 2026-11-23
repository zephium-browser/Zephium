use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use url::Url;
use zephium_map::{
    compare,
    query::{self, SectionRole},
    results::{self, FirstLayer, Profile},
    robots::Robots,
    session::{self, ReadState},
    site, PageKey, PlaceKey, SiteKey,
};

pub const MAX_TEXT_BYTES: usize = 300;
pub use site::MAX_PATH_CHARS;
pub const MAX_SECTIONS: usize = site::MAX_SECTIONS;
pub const MAX_CHILDREN: usize = 12;
pub const MAX_SOCIAL: usize = 10;
pub const MAX_ALSO: usize = 8;
pub use site::MAX_HOST_BYTES;
pub const MAX_ROWS: usize = 2 * MAX_SECTIONS;
pub const ID_BYTES: usize = 16;

macro_rules! mirror {
    ($name:ident from $engine:path { $($variant:ident),+ $(,)? }) => {
        #[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
        #[cfg_attr(feature = "ipc-types", derive(specta::Type))]
        #[serde(rename_all = "snake_case")]
        pub enum $name {
            $($variant),+
        }
        impl From<$engine> for $name {
            fn from(value: $engine) -> Self {
                match value {
                    $(<$engine>::$variant => Self::$variant),+
                }
            }
        }
    };
}

mirror!(Role from SectionRole {
    News, Product, Features, Solutions, Enterprise, Customers, Community, Security, Legal,
    Contact, SignUp, Learn, Examples, Playground, Integrations, Home, Pricing, Documentation,
    Api, Reference, Changelog, Releases, Status, Download, Install, Login, Blog, About, Careers,
    Support,
});
mirror!(Network from results::Network {
    Twitter, GitHub, YouTube, LinkedIn, Discord, Reddit, Mastodon, Bluesky, Instagram, Facebook,
});
mirror!(Tier from site::Tier { Primary, Secondary, Action });
mirror!(Confidence from query::Confidence { Low, High });

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "ipc-types", derive(specta::Type))]
#[serde(rename_all = "snake_case")]
pub enum StartRefusal {
    NotAMap,
    UnsupportedEngine,
    Invalid,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "ipc-types", derive(specta::Type))]
#[serde(rename_all = "snake_case")]
pub enum OpenRefusal {
    NoPlace,
    UnknownEntry,
    NotOpenable,
    TooManyWaiting,
    Cancelled,
}

impl From<session::OpenRefusal> for OpenRefusal {
    fn from(refusal: session::OpenRefusal) -> Self {
        match refusal {
            session::OpenRefusal::UnknownEntry => Self::UnknownEntry,
            session::OpenRefusal::NotOpenable => Self::NotOpenable,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "ipc-types", derive(specta::Type))]
#[serde(rename_all = "snake_case")]
pub enum Failure {
    Cancelled,
    Blocked,
    Challenge,
    Consent,
    NoResults,
    Unreadable,
    Unavailable,
    Timeout,
    Limit,
    Missing,
}

impl From<session::Failure> for Failure {
    fn from(failure: session::Failure) -> Self {
        use session::Failure as F;
        match failure {
            F::Cancelled => Self::Cancelled,
            F::Missing | F::Client => Self::Missing,
            F::Server | F::Unavailable => Self::Unavailable,
            F::Unreadable | F::ContentType => Self::Unreadable,
            F::Timeout => Self::Timeout,
            F::External | F::Robots => Self::Blocked,
            F::Challenge => Self::Challenge,
            F::Consent => Self::Consent,
            F::NoResults => Self::NoResults,
            F::Limit => Self::Limit,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "ipc-types", derive(specta::Type))]
#[serde(rename_all = "snake_case")]
pub enum Read {
    Queued,
    Read,
    Refused,
    Skipped,
}

impl From<ReadState> for Read {
    fn from(state: ReadState) -> Self {
        match state {
            ReadState::Queued => Self::Queued,
            ReadState::Read => Self::Read,
            ReadState::Refused { .. } => Self::Refused,
            ReadState::Skipped { .. } => Self::Skipped,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "ipc-types", derive(specta::Type))]
pub struct Official {
    pub title: String,
    pub site: String,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "ipc-types", derive(specta::Type))]
#[serde(rename_all = "snake_case")]
pub enum AlsoKind {
    Page,
    Discussion,
    Video,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "ipc-types", derive(specta::Type))]
pub struct Also {
    pub title: String,
    pub host: String,
    pub kind: AlsoKind,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "ipc-types", derive(specta::Type))]
pub struct Entry {
    pub id: String,
    pub tier: Tier,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<Role>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    pub off_site: bool,
    pub open: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub read: Option<Read>,
    pub children: Vec<Entry>,
    pub more: bool,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "ipc-types", derive(specta::Type))]
pub struct Notes {
    pub shell: bool,
    pub wall: bool,
    pub robots_disallowed: bool,
    pub truncated: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "ipc-types", derive(specta::Type))]
pub struct Place {
    pub title: String,
    pub site: String,
    pub host: String,
    pub sections: Vec<Entry>,
    pub focus: Option<String>,
    pub social: Vec<Network>,
    pub notes: Notes,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "ipc-types", derive(specta::Type))]
pub struct Row {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<Role>,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub left: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub right: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "ipc-types", derive(specta::Type))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Outcome {
    Done,
    Cancelled,
    Failed { failure: Failure },
}

impl From<session::EndOutcome> for Outcome {
    fn from(outcome: session::EndOutcome) -> Self {
        match outcome {
            session::EndOutcome::Done => Self::Done,
            session::EndOutcome::Cancelled => Self::Cancelled,
            session::EndOutcome::Failed { failure } => Self::Failed {
                failure: failure.into(),
            },
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "ipc-types", derive(specta::Type))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum OpenResult {
    Read,
    Refused { failure: Failure },
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "ipc-types", derive(specta::Type))]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Change {
    FirstLayer {
        seq: u32,
        official: Option<Official>,
        social: Vec<Network>,
        organic: u8,
        also: Vec<Also>,
    },
    Place {
        seq: u32,
        slot: u8,
        place: Place,
        stale: bool,
        confidence: Confidence,
        complete: bool,
    },
    PlaceFailed {
        seq: u32,
        slot: u8,
        failure: Failure,
    },
    Compare {
        seq: u32,
        rows: Vec<Row>,
    },
    End {
        seq: u32,
        outcome: Outcome,
        elapsed_ms: u32,
    },
    Opened {
        seq: u32,
        slot: u8,
        id: String,
        result: OpenResult,
    },
}

impl Change {
    pub fn seq(&self) -> u32 {
        match self {
            Self::FirstLayer { seq, .. }
            | Self::Place { seq, .. }
            | Self::PlaceFailed { seq, .. }
            | Self::Compare { seq, .. }
            | Self::End { seq, .. }
            | Self::Opened { seq, .. } => *seq,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "ipc-types", derive(specta::Type))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Target {
    Entry { id: String },
    Site,
    Social { network: Network },
    Also { index: u8 },
}

#[derive(Default)]
pub(crate) struct Targets {
    entries: BTreeMap<String, Url>,
    site: Option<Url>,
    social: BTreeMap<Network, Url>,
}

impl Targets {
    pub(crate) fn resolve(&self, target: &Target) -> Option<Url> {
        let url = match target {
            Target::Entry { id } => self.entries.get(id),
            Target::Site => self.site.as_ref(),
            Target::Social { network } => self.social.get(network),
            Target::Also { .. } => None,
        }?;
        (url.scheme() == "https").then(|| url.clone())
    }

    pub(crate) fn has(&self, id: &str) -> bool {
        self.entries.contains_key(id)
    }
}

pub(crate) fn text(raw: &str) -> String {
    let mut out = String::new();
    let mut space = false;
    for c in zephium_map::clean_text(raw).chars() {
        if c.is_whitespace() || c.is_control() {
            space = !out.is_empty();
            continue;
        }
        if out.len() + c.len_utf8() + usize::from(space) > MAX_TEXT_BYTES {
            break;
        }
        if std::mem::take(&mut space) {
            out.push(' ');
        }
        out.push(c);
    }
    out
}

fn address_shaped(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    lower.contains("://")
        || lower.split(' ').any(|word| {
            ["http:", "https:", "www."]
                .iter()
                .any(|prefix| word.starts_with(prefix))
        })
}

fn shown(raw: &str) -> Option<String> {
    let text = text(raw);
    (!text.is_empty() && !address_shaped(&text)).then_some(text)
}

fn valid_id(id: &str) -> bool {
    id.len() == ID_BYTES && id.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

pub(crate) use site::display_path as path;

fn address(entry: &site::Entry) -> Url {
    let mut url = entry.key.as_url().clone();
    if url.fragment().is_none() {
        if let Some(fragment) = entry.locator.fragment() {
            url.set_fragment(fragment.strip_prefix('#'));
        }
    }
    url
}

fn hostname(url: &Url) -> Option<String> {
    site::display_hostname(url).map(str::to_owned)
}

fn networks(profiles: &[Profile], social: &mut BTreeMap<Network, Url>) -> Vec<Network> {
    let mut out = Vec::new();
    for profile in profiles {
        let network = Network::from(profile.network);
        if out.len() >= MAX_SOCIAL {
            break;
        }
        if !out.contains(&network) {
            out.push(network);
            social.insert(network, profile.url.clone());
        }
    }
    out
}

fn entry(
    source: &site::Entry,
    child: bool,
    place_host: Option<&str>,
    open: bool,
    seen: &mut BTreeSet<String>,
    targets: &mut BTreeMap<String, Url>,
) -> Option<Entry> {
    let id = source.id.as_str();
    if !valid_id(id) || seen.contains(id) {
        return None;
    }
    let path = path(source.key.as_url());
    let label = text(&source.label);
    let label = if label.is_empty() {
        return None;
    } else if !address_shaped(&label) {
        label
    } else if path != "/" {
        path.clone()
    } else {
        return None;
    };
    seen.insert(id.to_owned());
    targets.insert(id.to_owned(), address(source));
    let mut children = Vec::new();
    let mut more = source.more > 0;
    if child {
        more |= !source.children.is_empty();
    } else {
        let mut rest = source.children.iter();
        for next in rest.by_ref() {
            if let Some(projected) = entry(next, true, place_host, false, seen, targets) {
                children.push(projected);
                if children.len() == MAX_CHILDREN {
                    break;
                }
            }
        }
        more |= rest.next().is_some();
    }
    Some(Entry {
        id: id.to_owned(),
        tier: source.tier.into(),
        label,
        role: source.role.map(Role::from),
        group: source.group.as_deref().and_then(shown),
        off_site: source.off_site,
        open,
        host: hostname(source.key.as_url()).filter(|host| Some(host.as_str()) != place_host),
        path,
        read: source.read.map(Read::from),
        children,
        more,
    })
}

pub(crate) fn place(
    source: &site::Place,
    robots: Option<&BTreeMap<String, Robots>>,
) -> (Place, Targets) {
    let mut targets = Targets {
        site: Some(source.entry.as_url().clone()),
        ..Targets::default()
    };
    let mut seen = BTreeSet::new();
    let mut ordered: Vec<_> = source.sections.iter().collect();
    ordered.sort_by_key(|e| e.tier);
    let mut sections = Vec::new();
    let host = hostname(source.entry.as_url());
    let mut actions = 0;
    for section in ordered {
        if section.tier == site::Tier::Action && actions == site::MAX_ACTIONS {
            continue;
        }
        if sections.len() == MAX_SECTIONS {
            break;
        }
        let open = robots.is_some_and(|rules| source.entry_openable(&section.id, rules));
        if let Some(projected) = entry(
            section,
            false,
            host.as_deref(),
            open,
            &mut seen,
            &mut targets.entries,
        ) {
            actions += usize::from(projected.tier == Tier::Action);
            sections.push(projected);
        }
    }
    let site = text(source.site.as_str());
    let place = Place {
        title: shown(&source.title).unwrap_or_else(|| site.clone()),
        host: text(&source.key.host),
        site,
        sections,
        focus: source
            .focus
            .as_ref()
            .map(|id| id.as_str())
            .filter(|id| seen.contains(*id))
            .map(str::to_owned),
        social: networks(&source.social, &mut targets.social),
        notes: Notes {
            shell: source.notes.shell,
            wall: source.notes.wall,
            robots_disallowed: source.notes.robots_disallowed,
            truncated: source.notes.truncated,
        },
    };
    (place, targets)
}

pub(crate) struct Layer {
    pub official: Option<Official>,
    pub social: Vec<Network>,
    pub organic: u8,
    pub also: Vec<Also>,
    pub targets: Vec<Url>,
}

pub(crate) fn first_layer(layer: &FirstLayer, places: &[(&PlaceKey, SiteKey)]) -> Layer {
    let official = layer.organic.iter().find_map(|hit| {
        let site = text(hit.official.as_ref()?.site.as_str());
        Some(Official {
            title: shown(&hit.title).unwrap_or_else(|| site.clone()),
            site,
        })
    });
    let mut social_targets = BTreeMap::new();
    let social = networks(&layer.social, &mut social_targets);
    let organic = u8::try_from(layer.organic.len()).unwrap_or(u8::MAX);
    let mut seen: BTreeSet<_> = social_targets
        .values()
        .filter_map(|url| Some(PageKey::from_url(url).ok()?.page_match()))
        .collect();
    let (also, targets) = layer
        .organic
        .iter()
        .filter(|hit| hit.official.is_none())
        .map(|hit| {
            (
                &hit.url,
                &hit.title,
                if hit.discussion {
                    AlsoKind::Discussion
                } else {
                    AlsoKind::Page
                },
            )
        })
        .chain(
            layer
                .video
                .iter()
                .map(|hit| (&hit.url, &hit.title, AlsoKind::Video)),
        )
        .filter_map(|(url, title, kind)| {
            let host = hostname(url)?;
            let key = PageKey::from_url(url).ok()?.page_match();
            if places.iter().any(|(place, site)| {
                if place.tenant.is_some() {
                    PlaceKey::from_url(url).as_ref() == Some(*place)
                } else {
                    SiteKey::from_url(url).as_ref() == Some(site)
                }
            }) || !seen.insert(key)
            {
                return None;
            }
            let title = shown(title)
                .filter(|text| {
                    !text.split_whitespace().any(|word| {
                        word.starts_with('/')
                            || word
                                .split_once('/')
                                .is_some_and(|(head, _)| head.contains('.'))
                    })
                })
                .unwrap_or_else(|| host.clone());
            Some((Also { title, host, kind }, url.clone()))
        })
        .take(MAX_ALSO)
        .unzip();
    Layer {
        official,
        social,
        organic,
        also,
        targets,
    }
}

pub(crate) fn also_target(urls: &[Url], index: u8) -> Option<Url> {
    let url = urls.get(usize::from(index))?;
    (url.scheme() == "https").then(|| url.clone())
}

pub(crate) fn rows(source: &[compare::Row], left: &Targets, right: &Targets) -> Vec<Row> {
    source
        .iter()
        .filter_map(|row| {
            let side = |id: &Option<site::EntryId>, targets: &Targets| {
                id.as_ref()
                    .map(|id| id.as_str())
                    .filter(|id| valid_id(id) && targets.has(id))
                    .map(str::to_owned)
            };
            let (l, r) = (side(&row.left, left), side(&row.right, right));
            if l.is_none() && r.is_none() {
                return None;
            }
            Some(Row {
                role: row.role.map(Role::from),
                label: shown(&row.label)?,
                left: l,
                right: r,
            })
        })
        .take(MAX_ROWS)
        .collect()
}

#[cfg(test)]
mod tests;
