use super::*;
use crate::map_bench::corpus::{fixture, Kind};
use crate::map_bench::engine::{Depth, Entry, Expectation};
use crate::map_bench::report;
use url::Url;
use zephium_public_fetch::{PublicBody, StatusClass, StatusRefusal, Timings};

const TEXT: &str = "private search secret";
const PROBE_CLOCK: RunClock = RunClock {
    engine: EngineKind::Probe,
    now: 0,
};
const SESSION_CLOCK: RunClock = RunClock {
    engine: EngineKind::Session,
    now: 4321,
};
const BODY: &[u8] = b"private third party content";

fn repository() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..")
}

fn query(id: &str, fields: &str) -> CorpusQuery {
    fixture(&format!("[[query]]\nid='{id}'\ntext='{TEXT}'\n{fields}"))
}

fn place(id: &str) -> CorpusQuery {
    query(
        id,
        "kind='place'\nsite='example.com'\nwant=['example.com/docs']\nslice='founder'",
    )
}

fn corpus(queries: Vec<CorpusQuery>) -> Corpus {
    Corpus {
        queries,
        hashes: vec![store::digest(b"corpus")],
    }
}

fn request(path: &str, expect: Expectation) -> FetchRequest {
    FetchRequest {
        url: Url::parse(&format!("https://example.com{path}")).unwrap(),
        expect,
        max_bytes: 1024,
        timeout: Duration::from_secs(3),
    }
}

#[derive(Default)]
struct FakeSource {
    calls: u64,
    sessions: u64,
    wire: u64,
    decoded: u64,
    denied: u64,
    pending: Option<Completed>,
}

impl Source for FakeSource {
    fn start(&mut self) {
        self.sessions += 1;
    }

    fn submit(&mut self, token: usize, request: FetchRequest, start: Duration) {
        self.calls += 1;
        self.wire += 42;
        let result = if request.url.path() == "/denied" {
            self.denied += 1;
            Err(FetchRefusal::Reader(ReadRefusal::Status(
                StatusRefusal::Denied,
            )))
        } else {
            self.decoded += BODY.len() as u64;
            Ok(PublicBody {
                final_url: request.url,
                hops: 0,
                status: StatusClass::NonAuthoritative,
                media_type: Some("text/plain".into()),
                charset: Some("utf-8".into()),
                bytes: BODY.to_vec(),
                truncated: true,
                wire_bytes: 42,
                timings: Timings {
                    queued: Duration::from_millis(1),
                    connect: None,
                    first_byte: Duration::from_millis(2),
                    total: Duration::from_millis(3),
                },
            })
        };
        let total = Duration::from_millis(3);
        assert!(self
            .pending
            .replace(Completed {
                token,
                result,
                transfer: Transfer {
                    wire_bytes: Some(42),
                    start,
                    total,
                    end: start + total
                },
            })
            .is_none());
    }

    fn next(&mut self) -> Option<Completed> {
        self.pending.take()
    }

    fn counts(&self) -> Measure {
        Measure {
            requests: self.calls,
            wire_bytes: self.wire,
            decoded_bytes: self.decoded,
            refusals: BTreeMap::from([(
                ReadRefusal::Status(StatusRefusal::Denied).to_string(),
                self.denied,
            )]),
            ..Measure::default()
        }
    }
}

struct Script<'a> {
    path: &'a str,
    expect: Expectation,
    repeated: bool,
}

impl Engine for Script<'_> {
    fn run(&mut self, query: &CorpusQuery, io: &mut dyn BenchIo) -> Outcome {
        let mut outcome = Outcome::empty(query.label.kind);
        let result = io.fetch(request(self.path, self.expect));
        if self.repeated {
            let mut repeated = request(self.path, self.expect);
            repeated.timeout = Duration::from_millis(1);
            repeated.max_bytes = 1;
            repeated.url.set_fragment(Some("does-not-reach-server"));
            assert_eq!(io.fetch(repeated), result);
        }
        if let Ok(body) = result {
            assert_eq!(body.bytes, BODY);
            assert_eq!(body.status, StatusClass::NonAuthoritative);
            assert_eq!(body.charset.as_deref(), Some("utf-8"));
            assert_eq!(body.media_type.as_deref(), Some("text/plain"));
            assert_eq!(body.timings.total, Duration::from_millis(3));
            assert!(body.truncated);
            outcome.entries.push(Entry {
                focus: false,
                url: body.final_url,
                depth: Depth::One,
                source: "test_source".into(),
                tags: BTreeSet::new(),
            });
        }
        outcome.milestones.full = Some(io.elapsed());
        outcome
    }
}

fn init(root: &Path, corpus: &Corpus) {
    init_engine(root, corpus, EngineKind::Probe);
}

fn init_engine(root: &Path, corpus: &Corpus, engine: EngineKind) {
    let _lock = store::record_lock(root, true).unwrap();
    write_json(
        &root.join(MANIFEST),
        &RunManifest::new(corpus, &repository(), ReaderLimits::default(), engine).unwrap(),
    )
    .unwrap();
}

#[test]
fn fake_recording_is_lossless_idempotent_and_can_be_replayed() {
    let root = tempfile::tempdir().unwrap();
    let corpus = corpus(vec![place("a"), place("b"), place("c")]);
    init(root.path(), &corpus);
    let mut source = FakeSource::default();
    let mut script = Script {
        path: "/docs",
        expect: Expectation::Text,
        repeated: true,
    };
    let selected = pending(root.path(), &corpus.queries, 2).unwrap();
    record_queries(
        root.path(),
        selected,
        &mut script,
        &mut source,
        Duration::ZERO,
        PROBE_CLOCK,
    )
    .unwrap();

    let remaining = pending(root.path(), &corpus.queries, 10).unwrap();
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].label.id, "c");
    assert_eq!((source.calls, source.sessions), (2, 2));
    let first = query_dir(root.path(), "a", false).unwrap();
    let captured = store::read_json::<QueryResult>(&first.join(OUTCOME)).unwrap();
    let measure = store::read_json::<Measure>(&first.join(MEASURE)).unwrap();
    assert_eq!(
        (measure.requests, measure.wire_bytes, measure.decoded_bytes),
        (1, 42, BODY.len() as u64)
    );
    assert_eq!(captured.activity.site_requests, 2);
    assert!(serde_json::from_str::<Depth>("0").is_err());
    assert!(serde_json::from_str::<Depth>("3").is_err());
    assert_eq!(serde_json::to_string(&Depth::One).unwrap(), "1");

    let journal = first.join("requests.jsonl");
    let original = fs::read(&journal).unwrap();
    let json: serde_json::Value = serde_json::from_slice(&original).unwrap();
    assert_eq!(json["key"]["method"], "GET");
    assert_eq!(json["key"]["url"], "https://example.com/docs");
    assert_eq!(json["result_class"], "body");
    assert_eq!(
        json["body"]["final_url_hash"],
        store::digest(b"https://example.com/docs")
    );

    replay(
        root.path(),
        &corpus,
        vec![&corpus.queries[0]],
        &mut script,
        EngineKind::Probe,
    )
    .unwrap();
    let (first_replay, _) = latest_replay(root.path()).unwrap().unwrap();
    let replayed =
        store::read_json::<QueryResult>(&first_replay.join("queries/a/outcome.json")).unwrap();
    assert_eq!(captured, replayed);
    let cost: ReplayMeasure =
        store::read_json(&first_replay.join("queries/a/measure.json")).unwrap();
    assert!(cost.wall_replayed && !cost.wall.is_zero());
    assert_eq!(cost.cpu_ns.is_some(), platform::sample().is_some());
    assert_eq!(cost.peak_rss_delta_bytes.is_some(), cost.cpu_ns.is_some());

    for (path, expect) in [
        ("/changed", Expectation::Text),
        ("/docs", Expectation::Html),
    ] {
        let mut changed = Script {
            path,
            expect,
            repeated: false,
        };
        let replayed = replay_query(
            root.path(),
            &corpus.queries[0],
            &mut changed,
            PROBE_CLOCK,
            0,
            None,
        )
        .unwrap();
        assert_eq!(replayed.score.verdict, Verdict::NotRecorded);
        assert_eq!(replayed.activity.not_recorded, 1);
        assert_eq!(replayed.activity.refusals["NotRecorded"], 1);
    }
    let unrecorded = replay_query(
        root.path(),
        &corpus.queries[2],
        &mut crate::map_bench::probe::ProbeEngine,
        PROBE_CLOCK,
        2,
        None,
    )
    .unwrap();
    assert_eq!(unrecorded.score.verdict, Verdict::NotRecorded);
    assert_eq!(unrecorded.score.reason, Reason::RecordingMissing);
    assert_eq!((source.calls, source.sessions), (2, 2));
    assert_eq!(fs::read(journal).unwrap(), original);
    let incomplete = root
        .path()
        .join("replay")
        .join(format!("{:039}", u128::MAX));
    fs::create_dir(incomplete).unwrap();
    assert_eq!(latest_replay(root.path()).unwrap().unwrap().0, first_replay);

    let hash = store::digest(BODY);
    let shared = root.path().join("bodies").join(&hash);
    assert_eq!(fs::read_dir(shared.parent().unwrap()).unwrap().count(), 1);
    fs::write(&shared, vec![b'x'; BODY.len()]).unwrap();
    for id in ["a", "b"] {
        let query_dir = query_dir(root.path(), id, false).unwrap();
        assert_eq!(
            fs::read(query_dir.join("bodies").join(&hash)).unwrap(),
            vec![b'x'; BODY.len()]
        );
        let stored = Journal::load(&query_dir).unwrap();
        let matched = stored
            .find(
                &request("/docs", Expectation::Text),
                |_| true,
                MatchPolicy::Probe,
            )
            .unwrap();
        assert_eq!(
            stored.result(matched.index, &query_dir).unwrap_err().class,
            "body_digest_mismatch"
        );
    }
}

#[test]
fn interrupted_reservation_is_never_retried_and_partial_run_resumes() {
    let root = tempfile::tempdir().unwrap();
    let query = place("a");
    let path = query_dir(root.path(), "a", true).unwrap();
    let mut journal = Journal::default();
    journal
        .reserve(
            &request("/docs", Expectation::Text),
            &path,
            Duration::from_millis(9),
        )
        .unwrap();

    let mut script = Script {
        path: "/docs",
        expect: Expectation::Text,
        repeated: false,
    };
    let mut source = FakeSource::default();
    assert_eq!(pending(root.path(), [&query], 1).unwrap().len(), 1);
    record_queries(
        root.path(),
        vec![&query],
        &mut script,
        &mut source,
        Duration::ZERO,
        PROBE_CLOCK,
    )
    .unwrap();

    assert_eq!(source.calls, 0);
    assert_eq!(source.sessions, 1);
    assert!(pending(root.path(), [&query], 1).unwrap().is_empty());
    let output = store::read_json::<QueryResult>(&path.join(OUTCOME)).unwrap();
    assert_eq!(output.score.verdict, Verdict::NotRecorded);
    assert_eq!(output.score.reason, Reason::RequestInterrupted);
    assert_eq!(
        output.outcome.unwrap().milestones.full,
        Some(Duration::from_millis(9))
    );
    let measure = store::read_json::<Measure>(&path.join(MEASURE)).unwrap();
    assert_eq!(measure.cpu_ns, None);
    assert_eq!(measure.refusals["Interrupted"], 1);
}

#[test]
fn engine_panic_and_limits_checkpoint_query_failures_and_continue() {
    struct Unreliable;

    impl Engine for Unreliable {
        fn run(&mut self, query: &CorpusQuery, io: &mut dyn BenchIo) -> Outcome {
            match query.label.id.as_str() {
                "panic" => panic!("https://example.com/panic-with-private-data"),
                "limit" => {
                    for index in 0..MAX_REQUESTS {
                        assert!(io
                            .fetch(request(&format!("/req-{index}"), Expectation::Text))
                            .is_ok());
                    }
                    assert_eq!(
                        io.fetch(request("/exceeds-limit", Expectation::Text)),
                        Err(FetchRefusal::Limit)
                    );
                }
                "engine-error" => {
                    let mut outcome = Outcome::empty(Kind::Place);
                    outcome.failure = Some(EngineFailure::SessionFailed);
                    return outcome;
                }
                "invalid-source" => {
                    let mut outcome = Outcome::empty(Kind::Place);
                    outcome.entries.push(Entry {
                        focus: false,
                        url: Url::parse("https://example.com/").unwrap(),
                        depth: Depth::One,
                        source: "x".repeat(crate::map_bench::engine::MAX_SOURCE_BYTES + 1),
                        tags: BTreeSet::new(),
                    });
                    return outcome;
                }
                "artifact" => {
                    let mut outcome = Outcome::empty(Kind::Place);
                    outcome
                        .stats
                        .insert("x".repeat(store::MAX_ARTIFACT_BYTES), 0);
                    return outcome;
                }
                _ => {}
            }
            Outcome::empty(query.label.kind)
        }
    }

    let root = tempfile::tempdir().unwrap();
    let corpus = corpus(vec![
        place("panic"),
        place("limit"),
        place("artifact"),
        place("engine-error"),
        place("invalid-source"),
        query("instant", "kind='instant'"),
    ]);
    init(root.path(), &corpus);
    let mut source = FakeSource::default();
    record_queries(
        root.path(),
        corpus.queries.iter().collect(),
        &mut Unreliable,
        &mut source,
        Duration::ZERO,
        PROBE_CLOCK,
    )
    .unwrap();
    assert_eq!(source.calls, MAX_REQUESTS as u64);
    for (id, verdict, reason) in [
        ("panic", Verdict::Fail, Reason::EnginePanic),
        ("limit", Verdict::Fail, Reason::RequestLimit),
        ("artifact", Verdict::Fail, Reason::InvalidOutcome),
        ("engine-error", Verdict::Fail, Reason::EngineSessionFailed),
        ("invalid-source", Verdict::Fail, Reason::InvalidOutcome),
        ("instant", Verdict::StayedOutOk, Reason::StayedOut),
    ] {
        let path = query_dir(root.path(), id, false).unwrap();
        let result = store::read_json::<QueryResult>(&path.join(OUTCOME)).unwrap();
        assert_eq!(
            (result.score.verdict, result.score.reason),
            (verdict, reason)
        );
    }
}

#[test]
fn invalid_request_cap_timeout_and_url_are_refused() {
    let root = tempfile::tempdir().unwrap();
    let base = request("/private", Expectation::Html);
    for invalid in [
        FetchRequest {
            max_bytes: 0,
            ..base.clone()
        },
        FetchRequest {
            max_bytes: zephium_public_fetch::MAX_DECODED_BYTES + 1,
            ..base.clone()
        },
        FetchRequest {
            timeout: Duration::ZERO,
            ..base.clone()
        },
        FetchRequest {
            timeout: zephium_public_fetch::MAX_READ_DURATION + Duration::from_nanos(1),
            ..base.clone()
        },
        request(
            &format!("/{}", "x".repeat(crate::map_bench::MAX_URL_BYTES)),
            Expectation::Text,
        ),
    ] {
        let mut io = QueryIo::new(root.path(), Journal::default(), PROBE_CLOCK, None);
        assert_eq!(io.fetch(invalid), Err(FetchRefusal::Limit));
        assert_eq!(io.failure, Some(Reason::RequestLimit));
    }
}

#[test]
fn recorded_results_page_slot_replays_bytes_and_time() {
    let root = tempfile::tempdir().unwrap();
    let path = query_dir(root.path(), "q", true).unwrap();
    let bodies = path.join("bodies");
    fs::create_dir(&bodies).unwrap();
    let hash = store::digest(BODY);
    fs::write(bodies.join(&hash), BODY).unwrap();
    let url = Url::parse("https://example.com/search?private-query").unwrap();
    let elapsed = Duration::from_millis(99);
    write_json(
        &path.join("results-page.json"),
        &ResultsSlot::Available {
            url: url.clone(),
            sha256: hash,
            decoded_bytes: BODY.len(),
            elapsed_at_return: elapsed,
            synthetic: false,
        },
    )
    .unwrap();

    let mut io = QueryIo::new(&path, Journal::default(), PROBE_CLOCK, None);
    let ResultsPage::Available {
        url: recorded_url,
        bytes,
        synthetic: false,
    } = io.results_page(None)
    else {
        panic!("recorded results page unavailable");
    };
    assert_eq!(recorded_url, url);
    assert_eq!(bytes, BODY);
    assert_eq!(io.elapsed(), elapsed);
    assert_eq!(io.activity.site_requests, 0);
}

#[test]
fn output_path_stays_outside_checkout_and_run_writer_is_exclusive() {
    let repo = repository().canonicalize().unwrap();
    let path = repo.join("target").join("map-bench-would-be-recording");
    assert_eq!(
        store::external_path(&path, &repo).unwrap_err().class,
        "path_inside_repository"
    );
    let outside = tempfile::tempdir().unwrap();
    let file = outside.path().join("new").join("child");
    assert_eq!(
        store::external_path(&file, &repo).unwrap(),
        outside.path().canonicalize().unwrap().join("new/child")
    );

    let first = store::record_lock(outside.path(), true).unwrap();
    assert_eq!(
        store::record_lock(outside.path(), true).unwrap_err().class,
        "recording_in_use"
    );
    assert_eq!(
        store::read_lock(outside.path()).unwrap_err().class,
        "recording_in_use"
    );
    drop(first);
    assert!(store::read_lock(outside.path()).is_ok());
}

#[cfg(target_os = "macos")]
#[test]
fn symlink_paths_cannot_point_into_checkout_or_redirect_artifacts() {
    let repo = repository().canonicalize().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let alias = outside.path().join("alias");
    std::os::unix::fs::symlink(&repo, &alias).unwrap();
    assert_eq!(
        store::external_path(&alias.join("new"), &repo)
            .unwrap_err()
            .class,
        "path_inside_repository"
    );
    let fake_queries = outside.path().join("queries");
    std::os::unix::fs::symlink(&repo, &fake_queries).unwrap();
    assert_eq!(
        query_dir(outside.path(), "q", true).unwrap_err().class,
        "artifact_symlink"
    );
}

#[test]
fn report_matches_golden_and_hides_private_inputs() {
    let root = tempfile::tempdir().unwrap();
    let corpus = corpus(vec![place("blocked"), query("answer", "kind='instant'")]);
    init(root.path(), &corpus);
    record_query(
        root.path(),
        &corpus.queries[0],
        &mut Script {
            path: "/denied",
            expect: Expectation::Text,
            repeated: false,
        },
        &mut FakeSource::default(),
        PROBE_CLOCK,
        save_outcome,
    )
    .unwrap();
    let first = query_dir(root.path(), "blocked", false).unwrap();
    let mut result = store::read_json::<QueryResult>(&first.join(OUTCOME)).unwrap();
    let outcome = result.outcome.as_mut().unwrap();
    outcome.milestones.full = Some(Duration::from_nanos(8));
    outcome
        .stats
        .insert("https://private.example/path".into(), 10);
    save_outcome(&first.join(OUTCOME), result).unwrap();
    write_json(
        &first.join(MEASURE),
        &Measure {
            wall: Duration::from_nanos(10),
            cpu_ns: Some(4),
            peak_rss_delta_bytes: Some(16),
            requests: 1,
            wire_bytes: 42,
            decoded_bytes: 0,
            refusals: BTreeMap::from([
                ("Status(Denied)".into(), 1),
                ("https://private.example/refusal".into(), 42),
            ]),
        },
    )
    .unwrap();

    let input = &corpus.queries[1];
    let second = query_dir(root.path(), "answer", true).unwrap();
    let mut outcome = Outcome::empty(Kind::Instant);
    outcome.milestones.full = Some(Duration::from_nanos(2));
    let activity = Activity::default();
    let score = score::score(input, &outcome, &activity);
    save_outcome(
        &second.join(OUTCOME),
        QueryResult {
            outcome: Some(outcome),
            score,
            activity,
        },
    )
    .unwrap();
    write_json(
        &second.join(MEASURE),
        &Measure {
            wall: Duration::from_nanos(3),
            cpu_ns: Some(1),
            peak_rss_delta_bytes: Some(0),
            ..Measure::default()
        },
    )
    .unwrap();

    report::write(root.path(), None).unwrap();
    let markdown = fs::read_to_string(root.path().join("summary.md")).unwrap();
    let golden = r"# Search map benchmark

## Recording

| Scope | pass | partial | fail | stayed_out_ok | stayed_out_broken | blocked | not_recorded | not_measured | no_official_site |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| Total | 0 | 0 | 0 | 1 | 0 | 1 | 0 | 0 | 0 |
| instant | 0 | 0 | 0 | 1 | 0 | 0 | 0 | 0 | 0 |
| place | 0 | 0 | 0 | 0 | 0 | 1 | 0 | 0 | 0 |
| Slice (unsliced) | 0 | 0 | 0 | 1 | 0 | 0 | 0 | 0 | 0 |
| Slice founder | 0 | 0 | 0 | 0 | 0 | 1 | 0 | 0 | 0 |

| Metric | Samples | p50 | p90 | Max |
| --- | ---: | ---: | ---: | ---: |
| cpu_ns | 2 | 1 | — | 4 |
| decoded_bytes | 2 | 0 | 0 | 0 |
| first_layer_ns | 0 | — | — | — |
| first_place_ns | 0 | — | — | — |
| full_ns | 2 | 2 | 8 | 8 |
| peak_rss_delta_bytes | 2 | 0 | — | 16 |
| requests | 2 | 0 | 1 | 1 |
| wall_ns | 2 | 3 | 10 | 10 |
| wire_bytes | 2 | 0 | 42 | 42 |

| Refusal class | Count |
| --- | ---: |
| Status(Denied) | 1 |

| Probe | Sites |
| --- | ---: |
| Total | 0 |
| Robots answered | 0 |
| Homepage answered | 0 |
| llms.txt answered | 0 |
| Sitemap answered | 0 |
| Blocked | 0 |
| robots_blocks_all | 0 |

Destinations scored as matched_locale — 0.

| Query ID | Verdict | Reason |
| --- | --- | --- |
| `blocked` | blocked | reader_blocked |

";
    assert_eq!(markdown, golden);
    assert!(!markdown.contains("https://") && !markdown.contains(TEXT));
    let json: serde_json::Value = store::read_json(&root.path().join("report.json")).unwrap();
    assert_eq!(json["recording"]["totals"]["blocked"], 1);
    assert!(json["latest_replay"].is_null());
    assert!(report::write(root.path(), Some(&root.path().join("run.json"))).is_err());

    replay(
        root.path(),
        &corpus,
        vec![&corpus.queries[0]],
        &mut Script {
            path: "/missing",
            expect: Expectation::Text,
            repeated: false,
        },
        EngineKind::Probe,
    )
    .unwrap();
    let custom = root.path().join("custom.md");
    report::write(root.path(), Some(&custom)).unwrap();
    let markdown = fs::read_to_string(custom).unwrap();
    assert!(markdown.contains("## Latest replay\n\nUnrecorded requests: 1"));
    assert!(markdown.contains("| replay_cpu_ns |"));
    assert!(markdown.contains("| replay_peak_rss_delta_bytes |"));
    assert!(markdown.contains("| replay_wall_ns |"));
    assert!(!markdown.contains("https://") && !markdown.contains(TEXT));
}

mod session;

#[test]
fn resume_retries_a_checkpoint_without_a_measurement() {
    let root = tempfile::tempdir().unwrap();
    let query = place("unfinished");
    let path = query_dir(root.path(), &query.label.id, true).unwrap();
    save_outcome(
        &path.join(OUTCOME),
        QueryResult::unavailable(Verdict::Fail, Reason::ArtifactError, Activity::default()),
    )
    .unwrap();
    assert_eq!(pending(root.path(), [&query], 1).unwrap().len(), 1);
    write_json(&path.join(MEASURE), &Measure::default()).unwrap();
    assert!(pending(root.path(), [&query], 1).unwrap().is_empty());
}

#[test]
fn failed_outcome_publication_leaves_the_resumed_query_pending() {
    let root = tempfile::tempdir().unwrap();
    let query = place("unfinished");
    let path = query_dir(root.path(), &query.label.id, true).unwrap();
    save_outcome(
        &path.join(OUTCOME),
        QueryResult::unavailable(Verdict::Fail, Reason::ArtifactError, Activity::default()),
    )
    .unwrap();
    assert_eq!(pending(root.path(), [&query], 1).unwrap().len(), 1);
    let mut engine = Script {
        path: "/denied",
        expect: Expectation::Text,
        repeated: false,
    };
    let mut source = FakeSource::default();
    let error = record_query(
        root.path(),
        &query,
        &mut engine,
        &mut source,
        PROBE_CLOCK,
        |_, _| {
            let measure = store::read_json::<Measure>(&path.join(MEASURE)).unwrap();
            assert_eq!(measure.requests, 1);
            Err(BenchError::new("publish_checkpoint"))
        },
    )
    .unwrap_err();
    assert_eq!(error.class, "publish_checkpoint");
    assert_eq!(pending(root.path(), [&query], 1).unwrap().len(), 1);
    record_query(
        root.path(),
        &query,
        &mut engine,
        &mut source,
        PROBE_CLOCK,
        save_outcome,
    )
    .unwrap();
    assert!(pending(root.path(), [&query], 1).unwrap().is_empty());
}

#[test]
fn invalid_outcomes_cannot_claim_the_no_site_exception() {
    struct Invalid;
    impl Engine for Invalid {
        fn run(&mut self, _: &CorpusQuery, _: &mut dyn BenchIo) -> Outcome {
            let mut outcome = Outcome::empty(Kind::Place);
            outcome.failure = Some(EngineFailure::SessionFailed);
            outcome.no_official_site = true;
            outcome.entries.push(Entry {
                focus: false,
                url: Url::parse("https://example.com/").unwrap(),
                depth: Depth::One,
                source: "x".repeat(crate::map_bench::engine::MAX_SOURCE_BYTES + 1),
                tags: BTreeSet::new(),
            });
            outcome
        }
    }
    let root = tempfile::tempdir().unwrap();
    let mut io = QueryIo::new(root.path(), Journal::default(), SESSION_CLOCK, None);
    let result = execute(&mut Invalid, &place("invalid"), &mut io);
    assert!(result.outcome.is_none());
    assert_eq!(result.score.reason, Reason::InvalidOutcome);
}

#[test]
fn latest_replay_reads_only_the_newest_published_manifest() {
    let root = tempfile::tempdir().unwrap();
    let directory = |stamp: u128| root.path().join("replay").join(format!("{stamp:039}"));
    for stamp in [1, 2, 3] {
        fs::create_dir_all(directory(stamp)).unwrap();
    }
    fs::write(directory(1).join(REPLAY), b"invalid").unwrap();
    write_json(
        &directory(2).join(REPLAY),
        &ReplayManifest {
            started_unix_ns: 2,
            corpus_sha256: vec![store::digest(b"corpus")],
            queries: vec![place("example").label],
        },
    )
    .unwrap();
    assert_eq!(latest_replay(root.path()).unwrap().unwrap().0, directory(2));
    fs::write(directory(3).join(REPLAY), b"invalid").unwrap();
    assert!(latest_replay(root.path()).is_err());
}

#[test]
fn recording_refuses_a_wire_total_above_u64() {
    let mut measure = Measure {
        wire_bytes: 1,
        ..Measure::default()
    };
    measure.include_prior_wire(u64::MAX - 1).unwrap();
    assert_eq!(measure.wire_bytes, u64::MAX);
    assert_eq!(
        measure.include_prior_wire(1).unwrap_err().class,
        "wire_bytes_overflow"
    );
}
