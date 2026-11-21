use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tokio::task::{Id as TaskId, JoinSet};
use url::Url;
use zephium_public_fetch::{
    PublicReader, ReadRefusal, ReadRequest, ReadSession, ReaderLimits, RouteSource,
};

use super::corpus::{Corpus, CorpusQuery, QueryLabel, MAX_CORPUS_FILES, MAX_QUERIES};
use super::engine::{
    redact_thread_panics, run_catching, Activity, BenchIo, Engine, EngineFailure, EngineKind,
    FetchRefusal, FetchRequest, FetchResult, LandingAdmission, Outcome, ResultsPage, MAX_REQUESTS,
};
use super::score::{self, Reason, Score, Verdict};
use super::store::{
    self, query_dir, write_json, Journal, MatchPolicy, RecordedMatch, ResultsSlot, Transfer,
};
use super::{platform, BenchError, Options, Result};

const SCHEMA: u32 = 1;
pub(super) const BETWEEN_QUERIES: Duration = Duration::from_millis(500);
pub(super) const MANIFEST: &str = "run.json";
pub(super) const OUTCOME: &str = "outcome.json";
pub(super) const MEASURE: &str = "measure.json";
pub(super) const REPLAY: &str = "replay.json";

#[derive(Debug, Deserialize, Serialize)]
pub(super) struct RunManifest {
    schema: u32,
    tool_version: String,
    git_commit: String,
    host_os: String,
    host_arch: String,
    start_unix_ns: u128,
    #[serde(default = "EngineKind::legacy")]
    pub(super) engine: EngineKind,
    pub(super) corpus_sha256: Vec<String>,
    reader_limits: ReaderConfig,
    pub(super) queries: Vec<QueryLabel>,
}

#[derive(Debug, Deserialize, Eq, PartialEq, Serialize)]
struct ReaderConfig {
    max_concurrent: usize,
    max_per_host: usize,
    connect_timeout: Duration,
    first_byte_timeout: Duration,
    max_duration: Duration,
    max_decoded_bytes: usize,
    max_wire_bytes: u64,
    max_header_bytes: usize,
    max_redirects: u8,
    max_location_bytes: usize,
}

impl From<ReaderLimits> for ReaderConfig {
    fn from(limits: ReaderLimits) -> Self {
        Self {
            max_concurrent: limits.max_concurrent,
            max_per_host: limits.max_per_host,
            connect_timeout: limits.connect_timeout,
            first_byte_timeout: limits.first_byte_timeout,
            max_duration: limits.max_duration,
            max_decoded_bytes: limits.max_decoded_bytes,
            max_wire_bytes: limits.max_wire_bytes,
            max_header_bytes: limits.max_header_bytes,
            max_redirects: zephium_public_fetch::MAX_REDIRECTS,
            max_location_bytes: zephium_public_fetch::MAX_LOCATION_BYTES,
        }
    }
}

fn now() -> Result<u128> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|time| time.as_nanos())
        .map_err(|_| BenchError::new("invalid_system_clock"))
}

impl RunManifest {
    fn new(
        corpus: &Corpus,
        repository: &Path,
        limits: ReaderLimits,
        engine: EngineKind,
    ) -> Result<Self> {
        let git = Command::new("git")
            .args(["rev-parse", "--verify", "HEAD"])
            .current_dir(repository)
            .stderr(Stdio::null())
            .output()
            .map_err(|error| BenchError::io("read_git_revision", &error))?;
        if !git.status.success() {
            return Err(BenchError::new("read_git_revision"));
        }
        let git_commit = std::str::from_utf8(&git.stdout)
            .map_err(|_| BenchError::new("invalid_git_revision"))?
            .trim();
        if !matches!(git_commit.len(), 40 | 64)
            || !git_commit.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(BenchError::new("invalid_git_revision"));
        }
        Ok(Self {
            schema: SCHEMA,
            tool_version: env!("CARGO_PKG_VERSION").into(),
            git_commit: git_commit.into(),
            host_os: std::env::consts::OS.into(),
            host_arch: std::env::consts::ARCH.into(),
            start_unix_ns: now()?,
            engine,
            corpus_sha256: corpus.hashes.clone(),
            reader_limits: limits.into(),
            queries: corpus
                .queries
                .iter()
                .map(|query| query.label.clone())
                .collect(),
        })
    }

    fn clock(&self) -> RunClock {
        RunClock {
            engine: self.engine,
            now: u64::try_from(self.start_unix_ns / Duration::from_secs(1).as_nanos())
                .unwrap_or(u64::MAX),
        }
    }

    pub(super) fn open(root: &Path) -> Result<Self> {
        let manifest: Self = store::read_json(&root.join(MANIFEST))?;
        if manifest.schema != SCHEMA
            || manifest.corpus_sha256.is_empty()
            || manifest.corpus_sha256.len() > MAX_CORPUS_FILES
            || !manifest
                .corpus_sha256
                .iter()
                .all(|hash| store::valid_digest(hash))
        {
            return Err(BenchError::new("invalid_run_manifest"));
        }
        validate_labels(&manifest.queries)?;
        Ok(manifest)
    }
}

pub(super) fn validate_labels(queries: &[QueryLabel]) -> Result<()> {
    let mut seen = BTreeSet::new();
    if queries.is_empty()
        || queries.len() > MAX_QUERIES
        || !queries
            .iter()
            .all(|query| query.valid() && seen.insert(&query.id))
    {
        return Err(BenchError::new("invalid_query_index"));
    }
    Ok(())
}

#[derive(Deserialize, Serialize)]
pub(super) struct ReplayManifest {
    pub(super) started_unix_ns: u128,
    pub(super) corpus_sha256: Vec<String>,
    pub(super) queries: Vec<QueryLabel>,
}

#[derive(Debug, Default, Deserialize, PartialEq, Serialize)]
pub(super) struct Measure {
    pub(super) wall: Duration,
    pub(super) cpu_ns: Option<u64>,
    pub(super) peak_rss_delta_bytes: Option<u64>,
    pub(super) requests: u64,
    pub(super) wire_bytes: u64,
    pub(super) decoded_bytes: u64,
    pub(super) refusals: BTreeMap<String, u64>,
}

#[derive(Deserialize, Serialize)]
pub(super) struct ReplayMeasure {
    pub(super) wall: Duration,
    pub(super) wall_replayed: bool,
    pub(super) cpu_ns: Option<u64>,
    pub(super) peak_rss_delta_bytes: Option<u64>,
}

fn process_delta(before: Option<platform::ProcessSample>) -> (Option<u64>, Option<u64>) {
    let Some((before, after)) = before.and_then(|before| Some((before, platform::sample()?)))
    else {
        return (None, None);
    };
    (
        after.cpu_ns.checked_sub(before.cpu_ns),
        after.peak_rss_bytes.checked_sub(before.peak_rss_bytes),
    )
}

#[derive(Debug, Deserialize, PartialEq, Serialize)]
pub(super) struct QueryResult {
    pub(super) outcome: Option<Outcome>,
    pub(super) score: Score,
    pub(super) activity: Activity,
}

impl QueryResult {
    pub(super) fn unavailable(verdict: Verdict, reason: Reason, activity: Activity) -> Self {
        Self {
            outcome: None,
            score: Score::unavailable(verdict, reason),
            activity,
        }
    }
}

#[derive(Clone, Copy)]
struct RunClock {
    engine: EngineKind,
    now: u64,
}

struct Completed {
    token: usize,
    result: FetchResult,
    transfer: Transfer,
}

trait Source {
    fn start(&mut self);
    fn submit(&mut self, token: usize, request: FetchRequest, start: Duration);
    fn next(&mut self) -> Option<Completed>;
    fn counts(&self) -> Measure;
}

struct PublicSource {
    reader: PublicReader,
    session: Arc<ReadSession>,
    runtime: tokio::runtime::Runtime,
    reads: JoinSet<Completed>,
    tasks: HashMap<TaskId, (usize, Instant, Duration)>,
}

impl PublicSource {
    fn new() -> Result<Self> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .on_thread_start(redact_thread_panics)
            .enable_all()
            .build()
            .map_err(|error| BenchError::io("reader_runtime", &error))?;
        let routes = RouteSource::new(runtime.handle());
        routes.set_app_proxy(None);
        runtime.block_on(routes.settled());
        let reader = PublicReader::new(ReaderLimits::default(), routes);
        let session = Arc::new(reader.session());
        Ok(Self {
            reader,
            session,
            runtime,
            reads: JoinSet::new(),
            tasks: HashMap::new(),
        })
    }
}

impl Source for PublicSource {
    fn start(&mut self) {
        self.session = Arc::new(self.reader.session());
    }

    fn submit(&mut self, token: usize, request: FetchRequest, start: Duration) {
        let session = Arc::clone(&self.session);
        let started = Instant::now();
        let task = self.reads.spawn_on(
            async move {
                let result = session
                    .read(ReadRequest {
                        url: request.url,
                        expect: request.expect.into(),
                        max_bytes: request.max_bytes,
                        deadline: started + request.timeout,
                    })
                    .await
                    .map_err(FetchRefusal::Reader);
                let total = started.elapsed();
                let wire_bytes = result.as_ref().ok().map(|body| body.wire_bytes);
                Completed {
                    token,
                    result,
                    transfer: Transfer {
                        wire_bytes,
                        start,
                        total,
                        end: start.saturating_add(total),
                    },
                }
            },
            self.runtime.handle(),
        );
        self.tasks.insert(task.id(), (token, started, start));
    }

    fn next(&mut self) -> Option<Completed> {
        match self.runtime.block_on(self.reads.join_next_with_id())? {
            Ok((id, completed)) => {
                self.tasks.remove(&id);
                Some(completed)
            }
            Err(error) => {
                let (token, started, start) = self.tasks.remove(&error.id())?;
                if error.is_panic() {
                    std::mem::forget(error.into_panic());
                }
                let total = started.elapsed();
                Some(Completed {
                    token,
                    result: Err(FetchRefusal::Storage),
                    transfer: Transfer {
                        wire_bytes: None,
                        start,
                        total,
                        end: start.saturating_add(total),
                    },
                })
            }
        }
    }

    fn counts(&self) -> Measure {
        let counts = self.reader.counters();
        Measure {
            requests: counts.reads,
            wire_bytes: counts.wire_bytes,
            decoded_bytes: counts.decoded_bytes,
            refusals: ReadRefusal::ALL
                .into_iter()
                .map(|class| (class.to_string(), counts.refused(class)))
                .collect(),
            ..Measure::default()
        }
    }
}

impl Measure {
    fn include_prior_wire(&mut self, prior: u64) -> Result<()> {
        self.wire_bytes = self
            .wire_bytes
            .checked_add(prior)
            .ok_or(BenchError::new("wire_bytes_overflow"))?;
        Ok(())
    }

    fn delta(self, before: &Self) -> Self {
        Self {
            requests: self.requests.saturating_sub(before.requests),
            wire_bytes: self.wire_bytes.saturating_sub(before.wire_bytes),
            decoded_bytes: self.decoded_bytes.saturating_sub(before.decoded_bytes),
            refusals: self
                .refusals
                .into_iter()
                .filter_map(|(class, count)| {
                    let delta =
                        count.saturating_sub(before.refusals.get(&class).copied().unwrap_or(0));
                    (delta > 0).then_some((class, delta))
                })
                .collect(),
            ..Self::default()
        }
    }
}

struct QueryIo<'a> {
    query: &'a Path,
    memo_directory: Option<&'a Path>,
    journal: Journal,
    live: Option<Live<'a>>,
    clock: RunClock,
    query_index: usize,
    elapsed: Duration,
    activity: Activity,
    failure: Option<Reason>,
    submitted: usize,
    claimed: BTreeSet<usize>,
    active: BTreeMap<usize, (usize, Option<u64>)>,
    ready: BTreeMap<(Option<usize>, Duration, usize), Ready>,
}

enum Ready {
    Recorded(RecordedMatch),
    Refused(FetchRefusal),
}

struct Live<'a> {
    root: &'a Path,
    source: &'a mut dyn Source,
    started: Instant,
    prior_elapsed: Duration,
}

impl Live<'_> {
    fn elapsed(&self) -> Duration {
        self.prior_elapsed.saturating_add(self.started.elapsed())
    }
}

impl<'a> QueryIo<'a> {
    fn new(query: &'a Path, journal: Journal, clock: RunClock, live: Option<Live<'a>>) -> Self {
        Self {
            query,
            memo_directory: live.as_ref().map(|_| query),
            journal,
            clock,
            live,
            query_index: 0,
            elapsed: Duration::ZERO,
            activity: Activity::default(),
            failure: None,
            submitted: 0,
            claimed: BTreeSet::new(),
            active: BTreeMap::new(),
            ready: BTreeMap::new(),
        }
    }

    fn refuse(&mut self, error: BenchError) -> FetchRefusal {
        if error.class == "request_limit" {
            self.failure = Some(Reason::RequestLimit);
            FetchRefusal::Limit
        } else {
            self.failure = Some(Reason::ArtifactError);
            FetchRefusal::Storage
        }
    }

    fn enqueue(&mut self, token: usize, request: FetchRequest) -> Result<()> {
        let probe = self.clock.engine == EngineKind::Probe;
        let policy = if probe {
            MatchPolicy::Probe
        } else if self.live.is_some() {
            MatchPolicy::Exact
        } else {
            MatchPolicy::Replay
        };
        if let Some(matched) = self.journal.find(
            &request,
            |index| probe || !self.claimed.contains(&index),
            policy,
        ) {
            self.claimed.insert(matched.index);
            self.ready.insert(
                (Some(matched.order), matched.end, token),
                Ready::Recorded(matched),
            );
        } else if let Some(live) = &mut self.live {
            let index = self.journal.reserve(&request, self.query, live.elapsed())?;
            self.claimed.insert(index);
            // The sequential probe can attribute refusals' reader-wide byte deltas.
            let wire_before = probe.then(|| live.source.counts().wire_bytes);
            live.source.submit(token, request, live.elapsed());
            self.active.insert(token, (index, wire_before));
        } else {
            self.ready.insert(
                (None, self.elapsed, token),
                Ready::Refused(FetchRefusal::NotRecorded),
            );
        }
        Ok(())
    }

    fn complete_live(
        &mut self,
        admit: Option<&mut LandingAdmission<'_>>,
    ) -> Option<(usize, FetchResult)> {
        let live = self.live.as_mut()?;
        if self.active.is_empty() {
            return None;
        }
        let Some(mut completed) = live.source.next() else {
            self.active.clear();
            self.failure = Some(Reason::ArtifactError);
            return None;
        };
        let Some((index, wire_before)) = self.active.remove(&completed.token) else {
            self.active.clear();
            self.failure = Some(Reason::ArtifactError);
            return None;
        };
        if let Some(before) = wire_before {
            completed.transfer.wire_bytes =
                Some(live.source.counts().wire_bytes.saturating_sub(before));
        }
        if completed.result == Err(FetchRefusal::Storage) {
            self.failure = Some(Reason::ArtifactError);
        }
        self.elapsed = self.elapsed.max(completed.transfer.end);
        let root = live.root;
        let mut denied_origin = None;
        if let (Ok(body), Some(admit)) = (&completed.result, admit) {
            if !admit(self, &body.final_url) {
                denied_origin = Url::parse(&body.final_url.origin().ascii_serialization()).ok();
                completed.result = Err(FetchRefusal::RobotsDenied);
            }
        }
        let result = self
            .journal
            .finish(
                root,
                self.query,
                index,
                &completed.result,
                completed.transfer,
                denied_origin,
            )
            .map_or_else(|error| Err(self.refuse(error)), |()| completed.result);
        Some((completed.token, result))
    }

    fn next_fetch_with(
        &mut self,
        admit: Option<&mut LandingAdmission<'_>>,
    ) -> Option<(usize, FetchResult)> {
        let (token, result) = if let Some(((_, end, token), ready)) = self.ready.pop_first() {
            self.elapsed = self.elapsed.max(end);
            let result = match ready {
                Ready::Recorded(matched) => match self.journal.result(matched.index, self.query) {
                    Ok(mut result) => {
                        if let Some(cap) = matched.relaxed_cap {
                            if let Ok(body) = &mut result {
                                body.bytes.truncate(cap);
                            }
                            self.activity.matched_relaxed += 1;
                        }
                        if let Some(admit) = admit {
                            if let Ok(body) = &result {
                                if !admit(self, &body.final_url) {
                                    result = Err(FetchRefusal::RobotsDenied);
                                }
                            } else if let (Err(FetchRefusal::RobotsDenied), Some(origin)) = (
                                &result,
                                self.journal.robots_denied_origin(matched.index).cloned(),
                            ) {
                                admit(self, &origin);
                            }
                        }
                        result
                    }
                    Err(error) => Err(self.refuse(error)),
                },
                Ready::Refused(class) => Err(class),
            };
            (token, result)
        } else {
            self.complete_live(admit)?
        };
        self.activity.called(&result);
        Some((token, result))
    }

    fn load_results(&mut self, synthetic: Option<(&Url, Vec<u8>)>) -> Result<ResultsPage> {
        let (page, elapsed) = ResultsSlot::load(self.query)?;
        if let (ResultsPage::Unavailable, Some(live), Some((url, bytes))) =
            (&page, &self.live, synthetic)
        {
            // A resumed query keeps the original results clock if it had one.
            self.elapsed = live.started.elapsed();
            ResultsSlot::save(live.root, self.query, url, &bytes, self.elapsed)?;
            return Ok(ResultsPage::Available {
                url: url.clone(),
                bytes,
                synthetic: true,
            });
        }
        self.elapsed = self.elapsed.max(elapsed);
        Ok(page)
    }
}

impl BenchIo for QueryIo<'_> {
    fn fetch(&mut self, request: FetchRequest) -> FetchResult {
        self.start_fetch(request);
        self.next_fetch()
            .map_or(Err(FetchRefusal::Storage), |(_, result)| result)
    }

    fn fetch_checked(
        &mut self,
        request: FetchRequest,
        admit: &mut LandingAdmission<'_>,
    ) -> FetchResult {
        self.start_fetch(request);
        self.next_fetch_with(Some(admit))
            .map_or(Err(FetchRefusal::Storage), |(_, result)| result)
    }

    fn start_fetch(&mut self, request: FetchRequest) -> usize {
        let token = self.submitted;
        self.submitted = self.submitted.saturating_add(1);
        let refused = if self.failure.is_some() {
            Some(FetchRefusal::Storage)
        } else if token >= MAX_REQUESTS || !request.valid() {
            self.failure = Some(Reason::RequestLimit);
            Some(FetchRefusal::Limit)
        } else {
            self.enqueue(token, request)
                .err()
                .map(|error| self.refuse(error))
        };
        if let Some(class) = refused {
            self.ready
                .insert((None, self.elapsed, token), Ready::Refused(class));
        }
        token
    }

    fn next_fetch(&mut self) -> Option<(usize, FetchResult)> {
        self.next_fetch_with(None)
    }

    fn results_page(&mut self, synthetic: Option<(&Url, Vec<u8>)>) -> ResultsPage {
        if self.live.is_some() && self.clock.engine == EngineKind::Probe {
            return ResultsPage::Unavailable;
        }
        self.load_results(synthetic).unwrap_or_else(|error| {
            self.refuse(error);
            ResultsPage::Unavailable
        })
    }

    fn remember(&mut self, index: usize, memo: &[u8]) {
        if let Some(directory) = self.memo_directory {
            let name = if index == 0 {
                "memo.bin".into()
            } else {
                format!("memo-{}.bin", index + 1)
            };
            if let Err(error) = store::write_blob(&directory.join(name), memo) {
                self.refuse(error);
            }
        }
    }

    fn now(&self) -> u64 {
        self.clock.now
    }

    fn session_id(&self) -> u64 {
        self.query_index as u64
    }

    fn elapsed(&self) -> Duration {
        self.elapsed
    }
}

fn execute(engine: &mut dyn Engine, query: &CorpusQuery, io: &mut QueryIo<'_>) -> QueryResult {
    let outcome = run_catching(engine, query, io);
    while io.next_fetch().is_some() {}
    let activity = std::mem::take(&mut io.activity);
    let Ok(outcome) = outcome else {
        return QueryResult::unavailable(Verdict::Fail, Reason::EnginePanic, activity);
    };
    let failure = io.failure.or(match outcome.failure {
        Some(EngineFailure::InvalidInput) => Some(Reason::EngineInvalidInput),
        Some(EngineFailure::SessionFailed) if outcome.no_official_site => {
            (!outcome.valid()).then_some(Reason::InvalidOutcome)
        }
        Some(EngineFailure::SessionFailed) => Some(Reason::EngineSessionFailed),
        Some(_) => Some(Reason::EngineFault),
        None if !outcome.valid() => Some(Reason::InvalidOutcome),
        None => None,
    });
    if let Some(reason) = failure {
        return QueryResult {
            outcome: outcome.valid().then_some(outcome),
            score: Score::unavailable(Verdict::Fail, reason),
            activity,
        };
    }
    QueryResult {
        score: score::score(query, &outcome, &activity),
        outcome: Some(outcome),
        activity,
    }
}

fn save_outcome(path: &Path, output: QueryResult) -> Result<()> {
    match write_json(path, &output) {
        Ok(()) => Ok(()),
        Err(error) if matches!(error.class, "artifact_size_limit" | "serialize_artifact") => {
            let failure =
                QueryResult::unavailable(Verdict::Fail, Reason::InvalidOutcome, output.activity);
            write_json(path, &failure)
        }
        Err(error) => Err(error),
    }
}

fn record_query(
    root: &Path,
    query: &CorpusQuery,
    engine: &mut dyn Engine,
    source: &mut dyn Source,
    clock: RunClock,
    publish_outcome: impl FnOnce(&Path, QueryResult) -> Result<()>,
) -> Result<()> {
    let path = query_dir(root, &query.label.id, false)?;
    let resumed = store::directory(&path, false)?;
    query_dir(root, &query.label.id, true)?;
    let journal = Journal::load(&path)?;
    journal.initialize(&path)?;
    ResultsSlot::initialize(&path)?;

    let prior_elapsed = journal.elapsed();
    let (prior_reads, prior_wire, prior_decoded) = journal.prior_transfer();
    let mut prior_refusals = BTreeMap::<String, u64>::new();
    for refusal in journal.prior_refusals() {
        *prior_refusals.entry(refusal.to_string()).or_default() += 1;
    }
    let before = source.counts();
    let process_before = (!resumed).then(platform::sample).flatten();
    let started = Instant::now();
    source.start();
    let mut io = QueryIo::new(
        &path,
        journal,
        clock,
        Some(Live {
            root,
            source,
            started,
            prior_elapsed,
        }),
    );
    io.query_index = query.index;
    let output = execute(engine, query, &mut io);
    let wall = prior_elapsed.saturating_add(started.elapsed());
    let cost = process_delta(process_before);
    let mut measure = source.counts().delta(&before);
    measure.wall = wall;
    (measure.cpu_ns, measure.peak_rss_delta_bytes) = cost;
    measure.requests += prior_reads;
    measure.include_prior_wire(prior_wire)?;
    measure.decoded_bytes += prior_decoded;
    for (class, count) in prior_refusals {
        *measure.refusals.entry(class).or_default() += count;
    }
    if let Some(count) = output.activity.refusals.get("Limit") {
        measure.refusals.insert("Limit".into(), *count);
    }
    let robots_denied = FetchRefusal::RobotsDenied.to_string();
    if let Some(count) = output.activity.refusals.get(&robots_denied) {
        measure.refusals.insert(robots_denied, *count);
    }
    let outcome = path.join(OUTCOME);
    if store::exists(&outcome)? {
        fs::remove_file(&outcome).map_err(|error| BenchError::io("invalidate_outcome", &error))?;
    }
    write_json(&path.join(MEASURE), &measure)?;
    publish_outcome(&outcome, output)
}

pub(super) fn pending<'a>(
    root: &Path,
    queries: impl IntoIterator<Item = &'a CorpusQuery>,
    limit: usize,
) -> Result<Vec<&'a CorpusQuery>> {
    let mut pending = Vec::new();
    for query in queries {
        if pending.len() >= limit {
            break;
        }
        let path = query_dir(root, &query.label.id, false)?;
        if !store::exists(&path.join(OUTCOME))? || !store::exists(&path.join(MEASURE))? {
            pending.push(query);
        }
    }
    Ok(pending)
}

pub(super) fn record(
    repository: &Path,
    root: &Path,
    corpus: &Corpus,
    selected: Vec<&CorpusQuery>,
    options: &Options,
    engine: &mut dyn Engine,
) -> Result<()> {
    let (resume, limit, engine_kind) = (options.resume, options.limit, options.engine);
    store::create_parents(root)?;
    let path = root.join(MANIFEST);
    if !store::exists(&path)? {
        store::ensure_empty_run(root)?;
    }
    let _lock = store::record_lock(root, true)?;
    let mut source = PublicSource::new()?;
    let limits = source.reader.limits();
    let manifest = if store::exists(&path)? {
        if !resume {
            return Err(BenchError::new("run_exists_use_resume"));
        }
        let recorded = RunManifest::open(root)?;
        if recorded.engine != engine_kind {
            return Err(BenchError::new("resume_engine_mismatch"));
        }
        if recorded.corpus_sha256 != corpus.hashes {
            return Err(BenchError::new("resume_corpus_mismatch"));
        }
        if recorded.reader_limits != ReaderConfig::from(limits)
            || recorded.host_os != std::env::consts::OS
            || recorded.host_arch != std::env::consts::ARCH
        {
            return Err(BenchError::new("resume_reader_or_host_mismatch"));
        }
        recorded
    } else {
        store::ensure_empty_run(root)?;
        let manifest = RunManifest::new(corpus, repository, limits, engine_kind)?;
        write_json(&path, &manifest)?;
        manifest
    };
    record_queries(
        root,
        pending(root, selected, limit)?,
        engine,
        &mut source,
        BETWEEN_QUERIES,
        manifest.clock(),
    )
}

fn record_queries(
    root: &Path,
    queries: Vec<&CorpusQuery>,
    engine: &mut dyn Engine,
    source: &mut dyn Source,
    pause: Duration,
    clock: RunClock,
) -> Result<()> {
    let mut failed = false;
    for (index, query) in queries.into_iter().enumerate() {
        if index > 0 {
            std::thread::sleep(pause);
        }
        if let Err(error) = record_query(root, query, engine, source, clock, save_outcome) {
            failed = true;
            eprintln!("map bench: {}", error.for_query(&query.label.id));
        }
    }
    if failed {
        Err(BenchError::new("record_artifact_errors"))
    } else {
        Ok(())
    }
}

fn replay_query(
    root: &Path,
    query: &CorpusQuery,
    engine: &mut dyn Engine,
    clock: RunClock,
    index: usize,
    destination: Option<&Path>,
) -> Result<QueryResult> {
    let path = query_dir(root, &query.label.id, false)?;
    let mut io = QueryIo::new(&path, Journal::load(&path)?, clock, None);
    io.query_index = index;
    io.memo_directory = destination;
    let result = execute(engine, query, &mut io);
    if !store::directory(&path, false)? {
        return Ok(QueryResult::unavailable(
            Verdict::NotRecorded,
            Reason::RecordingMissing,
            result.activity,
        ));
    }
    Ok(result)
}

pub(super) fn replay(
    root: &Path,
    corpus: &Corpus,
    queries: Vec<&CorpusQuery>,
    engine: &mut dyn Engine,
    engine_kind: EngineKind,
) -> Result<()> {
    let _lock = store::read_lock(root)?;
    let recorded = RunManifest::open(root)?;
    if recorded.engine != engine_kind {
        return Err(BenchError::new("replay_engine_mismatch"));
    }
    let parent = root.join("replay");
    store::directory(&parent, true)?;
    let started_unix_ns = now()?;
    let destination = parent.join(format!("{started_unix_ns:039}"));
    let mut builder = fs::DirBuilder::new();
    platform::private_directory(&mut builder);
    builder
        .create(&destination)
        .map_err(|error| BenchError::io("create_replay", &error))?;
    for query in &queries {
        let path = query_dir(&destination, &query.label.id, true)?;
        let index = recorded
            .queries
            .iter()
            .position(|label| label.id == query.label.id)
            .unwrap_or(query.index);
        let before = platform::sample();
        let started = Instant::now();
        let result = replay_query(root, query, engine, recorded.clock(), index, Some(&path))
            .unwrap_or_else(|error| {
                eprintln!("map bench: {}", error.for_query(&query.label.id));
                QueryResult::unavailable(Verdict::Fail, Reason::ArtifactError, Activity::default())
            });
        let wall = started.elapsed();
        let (cpu_ns, peak_rss_delta_bytes) = process_delta(before);
        write_json(
            &path.join(MEASURE),
            &ReplayMeasure {
                wall,
                wall_replayed: true,
                cpu_ns,
                peak_rss_delta_bytes,
            },
        )?;
        save_outcome(&path.join(OUTCOME), result)?;
    }
    let manifest = ReplayManifest {
        started_unix_ns,
        corpus_sha256: corpus.hashes.clone(),
        queries: queries
            .into_iter()
            .map(|query| query.label.clone())
            .collect(),
    };
    write_json(&destination.join(REPLAY), &manifest)
}

pub(super) fn latest_replay(root: &Path) -> Result<Option<(PathBuf, ReplayManifest)>> {
    let parent = root.join("replay");
    if !store::directory(&parent, false)? {
        return Ok(None);
    }
    let mut latest = None::<(u128, Result<PathBuf>)>;
    for entry in store::children(&parent)? {
        let entry = entry.map_err(|error| BenchError::io("list_replay", &error))?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let Ok(timestamp) = name.parse::<u128>() else {
            continue;
        };
        if name != format!("{timestamp:039}")
            || latest
                .as_ref()
                .is_some_and(|(newest, _)| timestamp <= *newest)
        {
            continue;
        }
        let path = entry.path();
        let published = store::directory(&path, false).and_then(|directory| {
            if directory {
                store::exists(&path.join(REPLAY))
            } else {
                Ok(false)
            }
        });
        match published {
            Ok(false) => {}
            Ok(true) => latest = Some((timestamp, Ok(path))),
            Err(error) => latest = Some((timestamp, Err(error))),
        }
    }
    let Some((timestamp, path)) = latest else {
        return Ok(None);
    };
    let path = path?;
    let manifest: ReplayManifest = store::read_json(&path.join(REPLAY))?;
    if manifest.started_unix_ns != timestamp
        || manifest.corpus_sha256.is_empty()
        || manifest.corpus_sha256.len() > MAX_CORPUS_FILES
        || !manifest
            .corpus_sha256
            .iter()
            .all(|hash| store::valid_digest(hash))
    {
        return Err(BenchError::new("invalid_replay_manifest"));
    }
    validate_labels(&manifest.queries)?;
    Ok(Some((path, manifest)))
}

#[cfg(test)]
mod tests;
