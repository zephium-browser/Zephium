use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use zephium_public_fetch::{PublicReader, ReadRefusal, ReadRequest, ReadSession, ReaderLimits};

use super::corpus::{Corpus, CorpusQuery, QueryLabel, MAX_CORPUS_FILES, MAX_QUERIES};
use super::engine::{
    run_catching, Activity, BenchIo, Engine, EngineFailure, FetchRefusal, FetchRequest,
    FetchResult, Outcome, ResultsPage, MAX_REQUESTS,
};
use super::score::{self, Reason, Score, Verdict};
use super::store::{self, optional_json, query_dir, write_json, Journal, ResultsSlot};
use super::{platform, BenchError, Result};

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
    fn new(corpus: &Corpus, repository: &Path, limits: ReaderLimits) -> Result<Self> {
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
            corpus_sha256: corpus.hashes.clone(),
            reader_limits: limits.into(),
            queries: corpus
                .queries
                .iter()
                .map(|query| query.label.clone())
                .collect(),
        })
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

trait Source {
    fn start(&mut self);
    fn fetch(&mut self, request: FetchRequest) -> FetchResult;
    fn counts(&self) -> Measure;
    fn wire_bytes(&self) -> u64;
}

struct PublicSource {
    reader: PublicReader,
    session: Option<ReadSession>,
    runtime: tokio::runtime::Runtime,
}

impl PublicSource {
    fn new() -> Result<Self> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| BenchError::io("reader_runtime", &error))?;
        Ok(Self {
            reader: PublicReader::new(ReaderLimits::default()),
            session: None,
            runtime,
        })
    }
}

impl Source for PublicSource {
    fn start(&mut self) {
        self.session = Some(self.reader.session());
    }

    fn fetch(&mut self, request: FetchRequest) -> FetchResult {
        let session = self.session.as_ref().ok_or(FetchRefusal::Storage)?;
        self.runtime
            .block_on(session.read(ReadRequest {
                url: request.url,
                expect: request.expect.into(),
                max_bytes: request.max_bytes,
                deadline: Instant::now() + request.timeout,
            }))
            .map_err(FetchRefusal::Reader)
    }

    fn wire_bytes(&self) -> u64 {
        self.reader.counters().wire_bytes
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
    journal: Journal,
    live: Option<Live<'a>>,
    elapsed: Duration,
    activity: Activity,
    failure: Option<Reason>,
}

struct Live<'a> {
    root: &'a Path,
    source: &'a mut dyn Source,
    started: Instant,
    prior_elapsed: Duration,
}

impl QueryIo<'_> {
    fn fetch_once(&mut self, request: FetchRequest) -> Result<FetchResult> {
        if let Some((result, elapsed)) = self.journal.lookup(&request, self.query)? {
            self.elapsed = self.elapsed.max(elapsed);
            return Ok(result);
        }
        let Some(live) = &mut self.live else {
            return Ok(Err(FetchRefusal::NotRecorded));
        };
        let elapsed = || live.prior_elapsed.saturating_add(live.started.elapsed());
        // A killed process cannot tell whether a reserved request reached the server.
        self.journal.reserve(&request, self.query, elapsed())?;
        let before = live.source.wire_bytes();
        let started = Instant::now();
        let result = live.source.fetch(request);
        let total = started.elapsed();
        let wire = live.source.wire_bytes().saturating_sub(before);
        self.elapsed = elapsed();
        self.journal
            .finish(live.root, self.query, &result, wire, total, self.elapsed)?;
        Ok(result)
    }
}

impl BenchIo for QueryIo<'_> {
    fn fetch(&mut self, request: FetchRequest) -> FetchResult {
        let result = if self.failure.is_some() {
            Err(FetchRefusal::Storage)
        } else if self.activity.site_requests >= MAX_REQUESTS as u64 || !request.valid() {
            self.failure = Some(Reason::RequestLimit);
            Err(FetchRefusal::Limit)
        } else {
            self.fetch_once(request).unwrap_or_else(|error| {
                Err(if error.class == "request_limit" {
                    self.failure = Some(Reason::RequestLimit);
                    FetchRefusal::Limit
                } else {
                    self.failure = Some(Reason::ArtifactError);
                    FetchRefusal::Storage
                })
            })
        };
        self.activity.called(&result);
        result
    }

    fn results_page(&mut self, _: &CorpusQuery) -> ResultsPage {
        if self.live.is_some() {
            return ResultsPage::Unavailable;
        }
        match ResultsSlot::load(self.query) {
            Ok((page, elapsed)) => {
                self.elapsed = self.elapsed.max(elapsed);
                page
            }
            Err(_) => {
                self.failure = Some(Reason::ArtifactError);
                ResultsPage::Unavailable
            }
        }
    }

    fn elapsed(&self) -> Duration {
        self.elapsed
    }
}

fn execute(engine: &mut dyn Engine, query: &CorpusQuery, io: &mut QueryIo<'_>) -> QueryResult {
    let outcome = run_catching(engine, query, io);
    let activity = std::mem::take(&mut io.activity);
    let Ok(outcome) = outcome else {
        return QueryResult::unavailable(Verdict::Fail, Reason::EnginePanic, activity);
    };
    let failure = io.failure.or(match outcome.failure {
        Some(EngineFailure::InvalidInput) => Some(Reason::EngineInvalidInput),
        Some(EngineFailure::SessionFailed) => Some(Reason::EngineSessionFailed),
        None if !outcome.valid() => Some(Reason::InvalidOutcome),
        None => None,
    });
    if let Some(reason) = failure {
        return QueryResult::unavailable(Verdict::Fail, reason, activity);
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
    let mut io = QueryIo {
        query: &path,
        journal,
        elapsed: Duration::ZERO,
        activity: Activity::default(),
        failure: None,
        live: Some(Live {
            root,
            source,
            started,
            prior_elapsed,
        }),
    };
    let output = execute(engine, query, &mut io);
    let wall = prior_elapsed.saturating_add(started.elapsed());
    let process_after = (!resumed).then(platform::sample).flatten();
    let mut measure = source.counts().delta(&before);
    measure.wall = wall;
    if let (Some(before), Some(after)) = (process_before, process_after) {
        measure.cpu_ns = Some(after.cpu_ns.saturating_sub(before.cpu_ns));
        measure.peak_rss_delta_bytes =
            Some(after.peak_rss_bytes.saturating_sub(before.peak_rss_bytes));
    }
    measure.requests += prior_reads;
    measure.wire_bytes += prior_wire;
    measure.decoded_bytes += prior_decoded;
    for (class, count) in prior_refusals {
        *measure.refusals.entry(class).or_default() += count;
    }
    if let Some(count) = output.activity.refusals.get("Limit") {
        measure.refusals.insert("Limit".into(), *count);
    }
    write_json(&path.join(MEASURE), &measure)?;
    save_outcome(&path.join(OUTCOME), output)
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
        if !store::exists(&path.join(OUTCOME))? {
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
    resume: bool,
    limit: usize,
    engine: &mut dyn Engine,
) -> Result<()> {
    store::create_parents(root)?;
    let path = root.join(MANIFEST);
    if !store::exists(&path)? {
        store::ensure_empty_run(root)?;
    }
    let _lock = store::record_lock(root, true)?;
    let mut source = PublicSource::new()?;
    let limits = source.reader.limits();
    if store::exists(&path)? {
        if !resume {
            return Err(BenchError::new("run_exists_use_resume"));
        }
        let recorded = RunManifest::open(root)?;
        if recorded.corpus_sha256 != corpus.hashes {
            return Err(BenchError::new("resume_corpus_mismatch"));
        }
        if recorded.reader_limits != ReaderConfig::from(limits)
            || recorded.host_os != std::env::consts::OS
            || recorded.host_arch != std::env::consts::ARCH
        {
            return Err(BenchError::new("resume_reader_or_host_mismatch"));
        }
    } else {
        store::ensure_empty_run(root)?;
        write_json(&path, &RunManifest::new(corpus, repository, limits)?)?;
    }
    record_queries(
        root,
        pending(root, selected, limit)?,
        engine,
        &mut source,
        BETWEEN_QUERIES,
    )
}

fn record_queries(
    root: &Path,
    queries: Vec<&CorpusQuery>,
    engine: &mut dyn Engine,
    source: &mut dyn Source,
    pause: Duration,
) -> Result<()> {
    let mut failed = false;
    for (index, query) in queries.into_iter().enumerate() {
        if index > 0 {
            std::thread::sleep(pause);
        }
        if let Err(error) = record_query(root, query, engine, source) {
            failed = true;
            eprintln!("map bench: {}", error.for_query(&query.label.id));
            let checkpoint = (|| {
                let path = query_dir(root, &query.label.id, true)?;
                save_outcome(
                    &path.join(OUTCOME),
                    QueryResult::unavailable(
                        Verdict::Fail,
                        Reason::ArtifactError,
                        Activity::default(),
                    ),
                )
            })();
            if let Err(error) = checkpoint {
                eprintln!("map bench: {}", error.for_query(&query.label.id));
            }
        }
    }
    if failed {
        Err(BenchError::new("record_artifact_errors"))
    } else {
        Ok(())
    }
}

fn replay_query(root: &Path, query: &CorpusQuery, engine: &mut dyn Engine) -> Result<QueryResult> {
    let path = query_dir(root, &query.label.id, false)?;
    let mut io = QueryIo {
        query: &path,
        journal: Journal::load(&path)?,
        live: None,
        elapsed: Duration::ZERO,
        activity: Activity::default(),
        failure: None,
    };
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
) -> Result<()> {
    let _lock = store::read_lock(root)?;
    RunManifest::open(root)?;
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
        let result = replay_query(root, query, engine).unwrap_or_else(|error| {
            eprintln!("map bench: {}", error.for_query(&query.label.id));
            QueryResult::unavailable(Verdict::Fail, Reason::ArtifactError, Activity::default())
        });
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
    let mut latest = None::<(PathBuf, ReplayManifest)>;
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
                .is_some_and(|(_, replay)| timestamp <= replay.started_unix_ns)
        {
            continue;
        }
        let path = entry.path();
        if !store::directory(&path, false)? {
            continue;
        }
        if let Some(manifest) = optional_json::<ReplayManifest>(&path.join(REPLAY))? {
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
            latest = Some((path, manifest));
        }
    }
    Ok(latest)
}

#[cfg(test)]
mod tests;
