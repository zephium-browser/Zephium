use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use url::Url;
use zephium_public_fetch::{Expect, PublicBody, ReadRefusal, StatusClass, StatusRefusal};

use super::corpus::{CorpusQuery, Group, Kind};
use super::{Result, MAX_URL_BYTES};

pub(super) const MAX_REQUESTS: usize = 64;
pub(super) const MAX_SOURCE_BYTES: usize = 80;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum EngineKind {
    Probe,
    Session,
}

impl EngineKind {
    pub(super) fn legacy() -> Self {
        Self::Probe
    }
}

pub(super) trait Engine {
    fn run(&mut self, query: &CorpusQuery, io: &mut dyn BenchIo) -> Outcome;
}

pub(super) trait BenchIo {
    fn fetch(&mut self, request: FetchRequest) -> FetchResult {
        self.fetch_checked(request, &mut |_, _| true)
    }
    fn fetch_checked(
        &mut self,
        request: FetchRequest,
        admit: &mut LandingAdmission<'_>,
    ) -> FetchResult;
    fn start_fetch(&mut self, request: FetchRequest) -> usize;
    fn next_fetch(&mut self) -> Option<(usize, FetchResult)>;
    fn results_page(&mut self, synthetic: Option<(&Url, Vec<u8>)>) -> ResultsPage;
    fn remember(&mut self, index: usize, memo: &[u8]);
    fn now(&self) -> u64;
    fn session_id(&self) -> u64;
    // The clock advances at recorded I/O boundaries so an identical replay sees the same time.
    fn elapsed(&self) -> Duration;
}

pub(super) type LandingAdmission<'a> = dyn FnMut(&mut dyn BenchIo, &Url) -> bool + 'a;

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Expectation {
    Html,
    Xml,
    Text,
}

impl From<Expectation> for Expect {
    fn from(expectation: Expectation) -> Self {
        match expectation {
            Expectation::Html => Self::Html,
            Expectation::Xml => Self::Xml,
            Expectation::Text => Self::Text,
        }
    }
}

#[derive(Clone)]
pub(super) struct FetchRequest {
    pub(super) url: Url,
    pub(super) expect: Expectation,
    pub(super) max_bytes: usize,
    pub(super) timeout: Duration,
}

impl FetchRequest {
    pub(super) fn valid(&self) -> bool {
        self.url.as_str().len() <= MAX_URL_BYTES
            && (1..=zephium_public_fetch::MAX_DECODED_BYTES).contains(&self.max_bytes)
            && !self.timeout.is_zero()
            && self.timeout <= zephium_public_fetch::MAX_READ_DURATION
    }
}

pub(super) type FetchResult = std::result::Result<PublicBody, FetchRefusal>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum FetchRefusal {
    Reader(ReadRefusal),
    NotRecorded,
    Interrupted,
    Limit,
    Storage,
    RobotsDenied,
}

impl FetchRefusal {
    pub(super) fn blocks_reader(self) -> bool {
        matches!(
            self,
            Self::Reader(ReadRefusal::Status(
                StatusRefusal::Denied | StatusRefusal::RateLimited | StatusRefusal::Challenge
            ))
        )
    }
}

impl fmt::Display for FetchRefusal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Reader(refusal) => refusal.fmt(formatter),
            Self::NotRecorded => formatter.write_str("NotRecorded"),
            Self::Interrupted => formatter.write_str("Interrupted"),
            Self::Limit => formatter.write_str("Limit"),
            Self::Storage => formatter.write_str("Storage"),
            Self::RobotsDenied => formatter.write_str("RobotsDenied"),
        }
    }
}

impl Serialize for FetchRefusal {
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for FetchRefusal {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        use serde::de::Error;
        let text = String::deserialize(deserializer)?;
        match text.as_str() {
            "NotRecorded" => Ok(Self::NotRecorded),
            "Interrupted" => Ok(Self::Interrupted),
            "Limit" => Ok(Self::Limit),
            "Storage" => Ok(Self::Storage),
            "RobotsDenied" => Ok(Self::RobotsDenied),
            _ => ReadRefusal::ALL
                .into_iter()
                .find(|refusal| refusal.to_string() == text)
                .map(Self::Reader)
                .ok_or_else(|| D::Error::custom("unknown refusal class")),
        }
    }
}

pub(super) enum ResultsPage {
    Unavailable,
    Available {
        url: Url,
        bytes: Vec<u8>,
        synthetic: bool,
    },
}

impl ResultsPage {
    pub(super) fn available(&self) -> bool {
        match self {
            Self::Unavailable => false,
            Self::Available { url, bytes, .. } => url.has_host() && !bytes.is_empty(),
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(try_from = "u8", into = "u8")]
pub(super) enum Depth {
    One = 1,
    Two = 2,
}

impl From<Depth> for u8 {
    fn from(depth: Depth) -> Self {
        depth as u8
    }
}

impl TryFrom<u8> for Depth {
    type Error = &'static str;

    fn try_from(depth: u8) -> std::result::Result<Self, Self::Error> {
        match depth {
            1 => Ok(Self::One),
            2 => Ok(Self::Two),
            _ => Err("entry depth must be one or two"),
        }
    }
}

#[derive(Debug, Deserialize, PartialEq, Serialize)]
pub(super) struct Entry {
    #[serde(with = "url_text")]
    pub(super) url: Url,
    pub(super) depth: Depth,
    #[serde(default)]
    pub(super) focus: bool,
    pub(super) source: String,
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub(super) tags: BTreeSet<String>,
}

#[derive(Debug, Deserialize, PartialEq, Serialize)]
pub(super) struct Passage {
    pub(super) text: String,
    #[serde(with = "url_text")]
    pub(super) source: Url,
}

pub(super) mod url_text {
    use serde::{Deserialize, Deserializer, Serializer};
    use url::Url;

    pub(in crate::map_bench) fn serialize<S: Serializer>(
        url: &Url,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(url.as_str())
    }

    pub(in crate::map_bench) fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Url, D::Error> {
        use serde::de::Error;
        let value = String::deserialize(deserializer)?;
        if value.len() > super::MAX_URL_BYTES {
            return Err(D::Error::custom("URL limit"));
        }
        Url::parse(&value).map_err(|_| D::Error::custom("invalid URL"))
    }
}

#[derive(Debug, Default, Deserialize, PartialEq, Serialize)]
pub(super) struct Milestones {
    pub(super) first_layer: Option<Duration>,
    pub(super) first_place: Option<Duration>,
    pub(super) full: Option<Duration>,
}

#[derive(Debug, Deserialize, PartialEq, Serialize)]
pub(super) struct Outcome {
    pub(super) kind: Kind,
    pub(super) failure: Option<EngineFailure>,
    pub(super) entries: Vec<Entry>,
    pub(super) passages: Vec<Passage>,
    pub(super) groups: BTreeSet<Group>,
    pub(super) milestones: Milestones,
    pub(super) stats: BTreeMap<String, u64>,
    pub(super) probe: Vec<ProbeSite>,
    #[serde(default)]
    pub(super) synthetic_results: bool,
    #[serde(default)]
    pub(super) no_official_site: bool,
    #[serde(default)]
    pub(super) needs_results_page: bool,
}

impl Outcome {
    pub(super) fn empty(kind: Kind) -> Self {
        Self {
            kind,
            failure: None,
            entries: Vec::new(),
            passages: Vec::new(),
            groups: BTreeSet::new(),
            milestones: Milestones::default(),
            stats: BTreeMap::new(),
            probe: Vec::new(),
            synthetic_results: false,
            no_official_site: false,
            needs_results_page: false,
        }
    }

    pub(super) fn valid(&self) -> bool {
        self.entries.iter().all(|entry| {
            entry.url.as_str().len() <= MAX_URL_BYTES
                && !entry.source.is_empty()
                && entry.source.len() <= MAX_SOURCE_BYTES
        }) && self
            .passages
            .iter()
            .all(|passage| passage.source.as_str().len() <= MAX_URL_BYTES)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum EngineFailure {
    InvalidInput,
    SessionFailed,
    AfterTerminal,
    MissingTerminal,
    Concurrency,
    UnexpectedCompletion,
    UnexpectedPlace,
    StayOutFetch,
    MemoLimit,
}

#[derive(Debug, Deserialize, PartialEq, Serialize)]
pub(super) struct ProbeSite {
    pub(super) robots: ProbeFetch,
    pub(super) homepage: ProbeFetch,
    pub(super) www_retry: Option<ProbeFetch>,
    pub(super) llms: ProbeFetch,
    pub(super) sitemap: ProbeFetch,
    pub(super) homepage_final_host: Option<String>,
    pub(super) robots_stop: Option<RobotsStop>,
}

impl ProbeSite {
    pub(super) fn blocked(&self) -> bool {
        let home = self.www_retry.as_ref().unwrap_or(&self.homepage);
        self.robots_stop
            .is_some_and(|stop| stop != RobotsStop::Interrupted)
            || (!home.answered()
                && [&self.homepage, home].into_iter().any(
                    |fetch| matches!(fetch, ProbeFetch::Refused { class } if class.blocks_reader()),
                ))
    }
}

#[derive(Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "status")]
pub(super) enum ProbeFetch {
    #[default]
    Skipped,
    Answered {
        http_status: u16,
        decoded_bytes: usize,
        wire_bytes: u64,
    },
    Refused {
        class: FetchRefusal,
    },
}

impl ProbeFetch {
    pub(super) fn answered(&self) -> bool {
        matches!(self, Self::Answered { .. })
    }
}

impl From<&FetchResult> for ProbeFetch {
    fn from(result: &FetchResult) -> Self {
        match result {
            Ok(body) => Self::Answered {
                http_status: match body.status {
                    StatusClass::Ok => 200,
                    StatusClass::NonAuthoritative => 203,
                },
                decoded_bytes: body.bytes.len(),
                wire_bytes: body.wire_bytes,
            },
            Err(class) => Self::Refused { class: *class },
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum RobotsStop {
    BlocksAll,
    Server,
    Timeout,
    Interrupted,
}

#[derive(Clone, Default, Debug, Deserialize, PartialEq, Serialize)]
pub(super) struct Activity {
    pub(super) site_requests: u64,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub(super) matched_relaxed: u64,
    pub(super) not_recorded: u64,
    pub(super) interrupted: u64,
    pub(super) refusals: BTreeMap<String, u64>,
}

fn is_zero(value: &u64) -> bool {
    *value == 0
}

impl Activity {
    pub(super) fn called(&mut self, result: &FetchResult) {
        self.site_requests = self.site_requests.saturating_add(1);
        if let Err(class) = result {
            match class {
                FetchRefusal::NotRecorded => self.not_recorded += 1,
                FetchRefusal::Interrupted => self.interrupted += 1,
                _ => {}
            }
            *self.refusals.entry(class.to_string()).or_default() += 1;
        }
    }

    pub(super) fn blocked(&self) -> bool {
        ReadRefusal::ALL
            .into_iter()
            .map(FetchRefusal::Reader)
            .any(|class| class.blocks_reader() && self.refusals.contains_key(&class.to_string()))
    }
}

thread_local! {
    static REDACT_PANICS: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

pub(super) fn redact_thread_panics() {
    REDACT_PANICS.set(true);
}

pub(super) fn run_catching(
    engine: &mut dyn Engine,
    query: &CorpusQuery,
    io: &mut dyn BenchIo,
) -> Result<Outcome> {
    use std::panic::{catch_unwind, set_hook, take_hook, AssertUnwindSafe};
    use std::sync::Once;

    static INSTALL_HOOK: Once = Once::new();
    INSTALL_HOOK.call_once(|| {
        let original = take_hook();
        set_hook(Box::new(move |info| {
            if !REDACT_PANICS
                .try_with(std::cell::Cell::get)
                .unwrap_or(false)
            {
                original(info);
            }
        }));
    });
    let previous = REDACT_PANICS.replace(true);
    let outcome = catch_unwind(AssertUnwindSafe(|| engine.run(query, io)));
    REDACT_PANICS.set(previous);
    outcome.map_err(|payload| {
        // A custom panic payload can panic again when dropped.
        std::mem::forget(payload);
        super::BenchError::new("engine_panic")
    })
}
