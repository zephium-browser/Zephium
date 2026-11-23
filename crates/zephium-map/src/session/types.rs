use super::*;
use crate::compare::Row;
use crate::doc::MAX_CHARSET_LABEL_BYTES;
use crate::query::QueryKind;

pub const MAX_FETCHES: usize = 24;
pub const MAX_HOST_FETCHES: usize = 12;
pub const MAX_CONCURRENT: usize = 4;
pub const MAX_HOST_CONCURRENT: usize = 2;
pub const MAX_TOTAL_BYTES: u64 = 6 * 1024 * 1024;
pub const MAX_HTML_BYTES: usize = DocLimits::SITE_PAGE.input_bytes as usize;
pub const MAX_TEXT_BYTES: usize = crate::site::MAX_LLMS_BYTES;
pub const MAX_XML_BYTES: usize = 512 * 1024;
pub const MAX_SITEMAP_READS: usize = 2;
pub const SITEMAP_SECTION_THRESHOLD: usize = 8;
pub const MAX_ELAPSED: Duration = Duration::from_secs(15);
pub const LAST_START: Duration = Duration::from_secs(10);
pub const FETCH_TIMEOUT: Duration = Duration::from_secs(4);
pub const MAX_MEDIA_TYPE_BYTES: usize = 256;

#[derive(Clone)]
pub struct Config {
    /// The caller assigns a distinct namespace to each live search and reuses it for replay.
    pub session_id: u64,
    pub engine: SearchEngine,
    pub custom_template: String,
    pub now: u64,
    pub fetches: u32,
    pub host_fetches: u32,
    pub concurrent: u32,
    pub host_concurrent: u32,
    pub total_bytes: u64,
    pub html_bytes: u32,
    pub xml_bytes: u32,
    pub text_bytes: u32,
    pub section_pages: u32,
    pub result_pages: u32,
    pub elapsed: Duration,
    pub last_start: Duration,
    pub timeout: Duration,
}
impl Default for Config {
    fn default() -> Self {
        Self {
            session_id: 0,
            engine: SearchEngine::default(),
            custom_template: String::new(),
            now: 0,
            fetches: MAX_FETCHES as u32,
            host_fetches: MAX_HOST_FETCHES as u32,
            concurrent: MAX_CONCURRENT as u32,
            host_concurrent: MAX_HOST_CONCURRENT as u32,
            total_bytes: MAX_TOTAL_BYTES,
            html_bytes: MAX_HTML_BYTES as u32,
            xml_bytes: MAX_XML_BYTES as u32,
            text_bytes: MAX_TEXT_BYTES as u32,
            section_pages: MAX_SECTION_PAGES as u32,
            result_pages: MAX_RESULT_PAGES as u32,
            elapsed: MAX_ELAPSED,
            last_start: LAST_START,
            timeout: FETCH_TIMEOUT,
        }
    }
}
impl Config {
    pub(super) fn tighten(&mut self) {
        self.fetches = self.fetches.min(MAX_FETCHES as u32);
        self.host_fetches = self.host_fetches.min(MAX_HOST_FETCHES as u32);
        self.concurrent = self.concurrent.min(MAX_CONCURRENT as u32);
        self.host_concurrent = self.host_concurrent.min(MAX_HOST_CONCURRENT as u32);
        self.total_bytes = self.total_bytes.min(MAX_TOTAL_BYTES);
        self.html_bytes = self.html_bytes.min(MAX_HTML_BYTES as u32);
        self.xml_bytes = self.xml_bytes.min(MAX_XML_BYTES as u32);
        self.text_bytes = self.text_bytes.min(MAX_TEXT_BYTES as u32);
        self.section_pages = self.section_pages.min(MAX_SECTION_PAGES as u32);
        self.result_pages = self.result_pages.min(MAX_RESULT_PAGES as u32);
        self.elapsed = self.elapsed.min(MAX_ELAPSED);
        self.last_start = self.last_start.min(LAST_START);
        self.timeout = self.timeout.min(FETCH_TIMEOUT);
    }
    pub(super) fn bytes(&self, expect: Expect) -> u32 {
        match expect {
            Expect::Html => self.html_bytes,
            Expect::Xml => self.xml_bytes,
            Expect::Text => self.text_bytes,
        }
    }
}

#[derive(Clone, Copy, PartialOrd, Ord, PartialEq, Eq, Debug)]
pub struct Id {
    pub session: u64,
    pub sequence: u32,
}
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Expect {
    Html,
    Xml,
    Text,
}
#[derive(
    serde::Serialize, serde::Deserialize, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug,
)]
pub enum Failure {
    Cancelled,
    Missing,
    Client,
    Server,
    Unreadable,
    Unavailable,
    Timeout,
    External,
    Robots,
    Challenge,
    Consent,
    NoResults,
    Limit,
    ContentType,
}
impl From<Wall> for Failure {
    fn from(w: Wall) -> Self {
        match w {
            Wall::Challenge => Self::Challenge,
            Wall::Consent => Self::Consent,
            Wall::NoResults => Self::NoResults,
            Wall::Unreadable => Self::Unreadable,
        }
    }
}
#[derive(Clone, Copy, PartialOrd, Ord, PartialEq, Eq, Debug)]
pub enum Phase {
    Results,
    AnonymousResults,
    Robots,
    Entry,
    Render,
    Section,
    Llms,
    Sitemap,
    Passage,
}
#[derive(
    serde::Serialize, serde::Deserialize, Clone, Copy, PartialOrd, Ord, PartialEq, Eq, Debug,
)]
pub enum Budget {
    Fetches,
    HostFetches,
    Concurrent,
    HostConcurrent,
    TotalBytes,
    HtmlBytes,
    XmlBytes,
    TextBytes,
    SectionPages,
    ResultPages,
    Elapsed,
    LastStart,
    Timeout,
}
impl Expect {
    pub(super) fn budget(self) -> Budget {
        match self {
            Self::Html => Budget::HtmlBytes,
            Self::Xml => Budget::XmlBytes,
            Self::Text => Budget::TextBytes,
        }
    }
}
#[derive(Default, Clone, Debug, PartialEq, Eq)]
pub struct Counts {
    pub started: u32,
    pub completed: u32,
    pub refused: BTreeMap<Failure, u32>,
    pub bytes: u64,
}
#[derive(Default, Clone, Debug, PartialEq, Eq)]
pub struct Stats {
    pub results: crate::results::ResultStats,
    pub places: BTreeMap<u8, site::PlaceStats>,
    pub phases: BTreeMap<Phase, Counts>,
    pub sources: BTreeSet<Source>,
    pub robots: BTreeMap<RobotsOutcome, u32>,
    pub shells: u32,
    pub walls: u32,
    pub budgets: BTreeSet<Budget>,
    pub cancelled: bool,
    pub fresh_memos: u32,
    pub stale_memos: u32,
    pub rejected_memos: u32,
}
#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReadState {
    Queued,
    Read,
    Refused { failure: Failure },
    Skipped { budget: Budget },
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EndOutcome {
    Done,
    Cancelled,
    Failed { failure: Failure },
}
pub enum MapEvent {
    Kind {
        kind: QueryKind,
    },
    FirstLayer {
        layer: FirstLayer,
    },
    Place {
        slot: u8,
        place: Place,
        stale: bool,
        confidence: Confidence,
        complete: bool,
    },
    PlaceFailed {
        slot: u8,
        failure: Failure,
    },
    Passages {
        passages: Vec<Passage>,
    },
    Compare {
        rows: Vec<Row>,
    },
    End {
        outcome: EndOutcome,
        stats: Stats,
    },
}
pub enum Effect {
    LoadResults {
        id: Id,
        url: Url,
    },
    Fetch {
        id: Id,
        url: Url,
        expect: Expect,
        max_bytes: u32,
        timeout: Duration,
    },
    Render {
        id: Id,
        url: Url,
    },
    ReadResultsAnonymously {
        id: Id,
        query: String,
    },
    LookupSite {
        id: Id,
        site: PlaceKey,
    },
    RememberSite {
        id: Id,
        site: PlaceKey,
        memo: Vec<u8>,
    },
    Emit {
        id: Id,
        event: MapEvent,
    },
}
impl Effect {
    pub fn id(&self) -> Id {
        match self {
            Self::LoadResults { id, .. }
            | Self::Fetch { id, .. }
            | Self::Render { id, .. }
            | Self::ReadResultsAnonymously { id, .. }
            | Self::LookupSite { id, .. }
            | Self::RememberSite { id, .. }
            | Self::Emit { id, .. } => *id,
        }
    }
}
pub struct FetchResult {
    pub final_url: Url,
    pub content_type: String,
    pub charset: Option<String>,
    pub bytes: Vec<u8>,
    pub truncated: bool,
}
impl FetchResult {
    pub(super) fn media(&self, expect: Expect) -> bool {
        if self.content_type.len() > MAX_MEDIA_TYPE_BYTES
            || self
                .charset
                .as_ref()
                .is_some_and(|c| c.len() > MAX_CHARSET_LABEL_BYTES)
        {
            return false;
        }
        let media = self
            .content_type
            .split(';')
            .next()
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase();
        match expect {
            Expect::Html => matches!(media.as_str(), "text/html" | "application/xhtml+xml"),
            Expect::Text => matches!(media.as_str(), "text/plain" | "text/markdown"),
            Expect::Xml => {
                matches!(media.as_str(), "text/xml" | "application/xml")
                    || media.split_once('/').is_some_and(|(kind, subtype)| {
                        let token = |value: &str| {
                            !value.is_empty()
                                && value.bytes().all(|b| {
                                    b.is_ascii_alphanumeric() || b"!#$%&'*+-.^`_|~".contains(&b)
                                })
                        };
                        token(kind)
                            && token(subtype)
                            && subtype
                                .strip_suffix("+xml")
                                .is_some_and(|base| !base.is_empty())
                    })
            }
        }
    }
}
pub enum Input {
    Results {
        id: Id,
        result: Result<Doc, Failure>,
    },
    Fetch {
        id: Id,
        result: Result<FetchResult, Failure>,
    },
    Render {
        id: Id,
        result: Result<Doc, Failure>,
    },
    Cache {
        id: Id,
        memo: Option<Vec<u8>>,
    },
    Elapsed {
        time: Duration,
    },
    Cancel,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn configuration_cannot_raise_any_hard_ceiling() {
        let mut cfg = Config {
            concurrent: u32::MAX,
            host_concurrent: u32::MAX,
            fetches: u32::MAX,
            host_fetches: u32::MAX,
            total_bytes: u64::MAX,
            html_bytes: u32::MAX,
            xml_bytes: u32::MAX,
            text_bytes: u32::MAX,
            section_pages: u32::MAX,
            result_pages: u32::MAX,
            elapsed: Duration::MAX,
            last_start: Duration::MAX,
            timeout: Duration::MAX,
            ..Default::default()
        };
        cfg.tighten();
        assert_eq!(cfg.total_bytes, MAX_TOTAL_BYTES);
        assert_eq!(
            (
                cfg.concurrent,
                cfg.host_concurrent,
                cfg.fetches,
                cfg.host_fetches
            ),
            (
                MAX_CONCURRENT as u32,
                MAX_HOST_CONCURRENT as u32,
                MAX_FETCHES as u32,
                MAX_HOST_FETCHES as u32
            )
        );
        assert_eq!(
            (cfg.html_bytes, cfg.xml_bytes, cfg.text_bytes),
            (
                MAX_HTML_BYTES as u32,
                MAX_XML_BYTES as u32,
                MAX_TEXT_BYTES as u32
            )
        );
        assert_eq!(
            (cfg.section_pages, cfg.result_pages),
            (MAX_SECTION_PAGES as u32, MAX_RESULT_PAGES as u32)
        );
        assert_eq!(
            (cfg.elapsed, cfg.last_start, cfg.timeout),
            (MAX_ELAPSED, LAST_START, FETCH_TIMEOUT)
        );
    }
}
