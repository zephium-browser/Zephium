use super::*;

struct ScriptedSource {
    respond: fn(&FetchRequest) -> FetchResult,
    pending: Vec<Completed>,
    calls: Vec<FetchRequest>,
    elapsed: Duration,
    reversed: bool,
    delay_html: Duration,
}

impl ScriptedSource {
    fn new(respond: fn(&FetchRequest) -> FetchResult) -> Self {
        Self {
            respond,
            pending: Vec::new(),
            calls: Vec::new(),
            elapsed: Duration::ZERO,
            reversed: false,
            delay_html: Duration::ZERO,
        }
    }
}

impl Source for ScriptedSource {
    fn start(&mut self) {
        assert!(self.pending.is_empty());
        self.elapsed = Duration::ZERO;
    }

    fn submit(&mut self, token: usize, request: FetchRequest, start: Duration) {
        let result = (self.respond)(&request);
        let delay = if request.expect == Expectation::Html {
            self.delay_html
        } else {
            Duration::ZERO
        };
        self.calls.push(request);
        let end = if self.reversed {
            start
        } else {
            self.elapsed.max(start) + Duration::from_millis(1) + delay
        };
        let total = end - start;
        self.pending.push(Completed {
            token,
            transfer: Transfer {
                wire_bytes: result.as_ref().ok().map(|body| body.wire_bytes),
                start,
                total,
                end,
            },
            result,
        });
    }

    fn next(&mut self) -> Option<Completed> {
        let order = |token| [3, 1, 2][token];
        let (index, _) = self.pending.iter().enumerate().min_by_key(|(_, item)| {
            if self.reversed {
                (Duration::ZERO, order(item.token))
            } else {
                (item.transfer.end, item.token)
            }
        })?;
        let mut completed = self.pending.swap_remove(index);
        if self.reversed {
            let ready = self
                .pending
                .iter()
                .map(|item| item.transfer.start)
                .fold(completed.transfer.start.max(self.elapsed), Duration::max);
            completed.transfer.end = ready + Duration::from_millis(1);
            completed.transfer.total = completed.transfer.end - completed.transfer.start;
        }
        self.elapsed = completed.transfer.end;
        Some(completed)
    }

    fn counts(&self) -> Measure {
        Measure {
            requests: self.calls.len() as u64,
            ..Measure::default()
        }
    }
}

fn scripted_body(request: &FetchRequest, media: &str, text: &str) -> FetchResult {
    Ok(PublicBody {
        final_url: request.url.clone(),
        hops: 0,
        status: StatusClass::Ok,
        media_type: Some(media.into()),
        charset: Some("utf-8".into()),
        bytes: text.as_bytes().to_vec(),
        truncated: false,
        wire_bytes: text.len() as u64,
        timings: Timings::default(),
    })
}

fn refuse(class: StatusRefusal) -> FetchResult {
    Err(FetchRefusal::Reader(ReadRefusal::Status(class)))
}

#[test]
fn probe_does_not_persist_robots_denied_redirect_bodies() {
    const DENIED_BODY: &str = "denied homepage body";
    let root = tempfile::tempdir().unwrap();
    let query = place("redirected");
    let mut source = ScriptedSource::new(|request| match request.url.path() {
        "/robots.txt" => scripted_body(
            request,
            "text/plain",
            if request.url.host_str() == Some("docs.example.com") {
                "User-agent: *\nDisallow: /private"
            } else {
                "User-agent: *\nAllow: /"
            },
        )
        .map(|mut body| {
            body.wire_bytes = 0;
            body
        }),
        "/" => scripted_body(request, "text/html", DENIED_BODY).map(|mut body| {
            body.final_url = Url::parse("https://docs.example.com/private").unwrap();
            body
        }),
        _ => refuse(StatusRefusal::Client),
    });
    let mut engine = crate::map_bench::probe::ProbeEngine;
    record_query(
        root.path(),
        &query,
        &mut engine,
        &mut source,
        PROBE_CLOCK,
        save_outcome,
    )
    .unwrap();
    let digest = store::digest(DENIED_BODY.as_bytes());
    let path = query_dir(root.path(), &query.label.id, false).unwrap();
    assert!(!root.path().join("bodies").join(&digest).exists());
    assert!(!path.join("bodies").join(digest).exists());
    let recorded: QueryResult = store::read_json(&path.join(OUTCOME)).unwrap();
    assert_eq!(recorded.score.reason, Reason::RobotsDenied);
    assert_eq!(recorded.activity.site_requests, 3);
    let replay = replay_query(root.path(), &query, &mut engine, PROBE_CLOCK, 0, None).unwrap();
    assert_eq!(recorded, replay);
}

fn site(request: &FetchRequest) -> FetchResult {
    match request.url.path() {
        "/robots.txt" => {
            scripted_body(request, "text/plain", "User-agent: *\nDisallow: /private\n")
        }
        "/" => scripted_body(
            request,
            "text/html",
            r#"<title>Acme</title>
            <header><nav><a href="/docs">Documentation</a><a href="/pricing">Pricing</a></nav></header>
            <footer><a href="/about">About</a></footer>"#,
        ),
        "/llms.txt" => scripted_body(
            request,
            "text/markdown",
            "# Acme\n## Resources\n- [Careers](/careers): Join the team\n",
        ),
        "/sitemap.xml" => scripted_body(
            request,
            "application/xml",
            r#"<urlset><url><loc>https://acme.com/contact</loc></url></urlset>"#,
        ),
        "/docs" => scripted_body(
            request,
            "text/html",
            r#"<aside><h2>Getting started</h2><ul><li><a href="/setup">Setup</a></li></ul></aside>"#,
        ),
        "/pricing" => scripted_body(
            request,
            "text/html",
            r#"<main><h1>Pricing</h1><h2 id="tiers">Plan tiers</h2></main>"#,
        ),
        _ => panic!("unexpected scripted request"),
    }
}

fn session_query(id: &str, text: &str, fields: &str) -> CorpusQuery {
    fixture(&format!("[[query]]\nid='{id}'\ntext='{text}'\n{fields}"))
}

const ACME: &str = "kind='place'\nsite='acme.com'\nwant=['acme.com/setup']\ngroup=['video']";

fn run_session(root: &Path, query: &CorpusQuery, source: &mut ScriptedSource) -> QueryResult {
    let mut engine = crate::map_bench::session::SessionEngine;
    record_query(
        root,
        query,
        &mut engine,
        source,
        SESSION_CLOCK,
        save_outcome,
    )
    .unwrap();
    let path = query_dir(root, &query.label.id, false).unwrap();
    let captured = store::read_json::<QueryResult>(&path.join(OUTCOME)).unwrap();
    let destination = tempfile::tempdir().unwrap();
    let replayed = replay_query(
        root,
        query,
        &mut engine,
        SESSION_CLOCK,
        query.index,
        Some(destination.path()),
    )
    .unwrap();
    for name in ["memo.bin", "memo-2.bin"] {
        let recorded = path.join(name);
        if recorded.is_file() {
            assert_eq!(
                fs::read(&recorded).unwrap(),
                fs::read(destination.path().join(name)).unwrap()
            );
        }
    }
    assert!(
        captured == replayed,
        "record and replay differed for {}",
        query.label.id
    );
    captured
}

#[test]
fn session_builds_and_replays_a_two_level_place() {
    let root = tempfile::tempdir().unwrap();
    let query = session_query("site", "Acme docs", ACME);
    let mut source = ScriptedSource::new(site);
    let result = run_session(root.path(), &query, &mut source);
    assert_eq!(result.score.verdict, Verdict::Pass);
    assert_eq!(result.score.missing_groups, None);
    assert_eq!(result.score.destination.unwrap().depth, 2);
    let outcome = result.outcome.unwrap();
    assert!(outcome.synthetic_results);
    assert_eq!(outcome.milestones.first_layer, None);
    assert!(outcome.milestones.first_place.unwrap() < outcome.milestones.full.unwrap());
    for (path, depth, source) in [
        ("/docs", Depth::One, "primary_nav"),
        ("/about", Depth::One, "footer"),
        ("/setup", Depth::Two, "section_sidebar"),
        ("/pricing", Depth::Two, "heading"),
    ] {
        assert!(
            outcome.entries.iter().any(|entry| {
                entry.url.path() == path && entry.depth == depth && entry.tags.contains(source)
            }),
            "{path} {depth:?} {source}"
        );
    }
    for (phase, requests) in [("Entry", 1), ("Llms", 1), ("Sitemap", 1), ("Section", 2)] {
        assert_eq!(outcome.stats[&format!("phase.{phase}.started")], requests);
    }
    let path = query_dir(root.path(), "site", false).unwrap();
    let memo =
        store::read_bytes(&path.join("memo.bin"), zephium_map::session::MAX_MEMO_BYTES).unwrap();
    assert_eq!(outcome.stats["memo.0.bytes"], memo.len() as u64);
    let site = zephium_map::PlaceKey::from_url(&Url::parse("https://acme.com/").unwrap()).unwrap();
    assert_eq!(
        zephium_map::session::SiteMemo::decode(&memo, &site)
            .unwrap()
            .read_at,
        SESSION_CLOCK.now
    );
    let ResultsPage::Available {
        bytes,
        synthetic: true,
        ..
    } = ResultsSlot::load(&path).unwrap().0
    else {
        panic!("missing synthetic slot");
    };
    assert_eq!(
        std::str::from_utf8(&bytes).unwrap(),
        "<main><article><h2><a href=\"https://acme.com/\">Acme</a></h2></article></main>"
    );
}

#[test]
fn concurrent_completions_are_saved_and_replay_in_finish_order() {
    let root = tempfile::tempdir().unwrap();
    let path = query_dir(root.path(), "order", true).unwrap();
    let mut source =
        ScriptedSource::new(|request| scripted_body(request, "text/html", "<main>data</main>"));
    source.reversed = true;
    let mut live = QueryIo::new(
        &path,
        Journal::default(),
        SESSION_CLOCK,
        Some(Live {
            root: root.path(),
            source: &mut source,
            started: Instant::now(),
            prior_elapsed: Duration::ZERO,
        }),
    );
    let requests = ["/a", "/b", "/c"].map(|path| request(path, Expectation::Html));
    for request in &requests {
        live.start_fetch(request.clone());
    }
    let mut expected = Vec::new();
    for token in [1, 2, 0] {
        let (actual, result) = live.next_fetch().unwrap();
        assert_eq!(actual, token);
        assert!(result.is_ok());
        expected.push((actual, result, live.elapsed()));
    }
    assert!(live.next_fetch().is_none());
    let journal = Journal::load(&path).unwrap();
    let mut replay = QueryIo::new(&path, journal, SESSION_CLOCK, None);
    for request in requests {
        replay.start_fetch(request);
    }
    let missing = replay.start_fetch(request("/not-recorded", Expectation::Html));
    assert_eq!(
        replay.next_fetch(),
        Some((missing, Err(FetchRefusal::NotRecorded)))
    );
    assert_eq!(replay.elapsed(), Duration::ZERO);
    for (token, result, end) in expected {
        assert_eq!(replay.next_fetch(), Some((token, result)));
        assert_eq!(replay.elapsed(), end);
    }
    assert!(replay.next_fetch().is_none());
}

#[test]
fn session_verdicts_cover_no_results_no_official_site_and_staying_out() {
    let root = tempfile::tempdir().unwrap();
    let mut source = ScriptedSource::new(|request| {
        assert_eq!(request.url.path(), "/robots.txt");
        scripted_body(request, "text/plain", "User-agent: *\nDisallow:\n")
    });
    for (id, text, fields, expected) in [
        (
            "missing",
            "how to reset timer",
            "kind='passage'\nanswer='button'",
            (Verdict::NotMeasured, Reason::NeedsResultsPage),
        ),
        (
            "unmatched",
            "Odd Brand docs",
            ACME,
            (Verdict::NoOfficialSite, Reason::NoOfficialSite),
        ),
        (
            "instant",
            "2 + 2",
            "kind='instant'\ngroup=['featured']",
            (Verdict::StayedOutOk, Reason::StayedOut),
        ),
        (
            "list",
            "best laptops",
            "kind='list'\ngroup=['discussion']",
            (Verdict::StayedOutOk, Reason::StayedOut),
        ),
    ] {
        let query = session_query(id, text, fields);
        let result = run_session(root.path(), &query, &mut source);
        assert_eq!(
            (result.score.verdict, result.score.reason),
            expected,
            "{id}"
        );
        assert_eq!(result.activity.site_requests, 0, "{id}");
    }
    assert!(source.calls.is_empty());
    let query = session_query(
        "compare",
        "Acme vs Odd Brand",
        "kind='compare'\nsites=['acme.com','another.com']",
    );
    let mut source = ScriptedSource::new(|request| {
        if request.url.path() == "/robots.txt" {
            scripted_body(request, "text/plain", "User-agent: *\nDisallow:\n")
        } else {
            refuse(StatusRefusal::Client)
        }
    });
    let result = run_session(root.path(), &query, &mut source);
    assert_eq!(result.score.verdict, Verdict::NoOfficialSite);
    let outcome = result.outcome.unwrap();
    assert_eq!(outcome.stats["official_sites"], 1);
    assert_eq!(outcome.stats["anonymous_results_wanted"], 1);
    assert_eq!(
        outcome.stats["phase.AnonymousResults.refused.Unavailable"],
        1
    );
}

#[test]
fn session_handles_robots_denial_and_render_unavailability() {
    let root = tempfile::tempdir().unwrap();
    let mut denied = ScriptedSource::new(|request| {
        assert_eq!(request.url.path(), "/robots.txt");
        scripted_body(request, "text/plain", "User-agent: *\nDisallow: /\n")
    });
    let query = session_query("denied", "Acme docs", ACME);
    let result = run_session(root.path(), &query, &mut denied);
    assert_eq!(
        (result.score.verdict, result.score.reason),
        (Verdict::Blocked, Reason::RobotsDenied)
    );
    assert_eq!(denied.calls.len(), 1);
    assert_eq!(
        result.outcome.unwrap().stats["phase.Entry.refused.Robots"],
        1
    );

    let mut shell = ScriptedSource::new(|request| match request.url.path() {
        "/robots.txt" => scripted_body(request, "text/plain", "User-agent: *\nDisallow:\n"),
        "/" => scripted_body(
            request,
            "text/html",
            "<title>Acme</title><div id='__next'></div><script>mount()</script>",
        ),
        _ => refuse(StatusRefusal::Client),
    });
    let query = session_query("shell", "Acme docs", ACME);
    let result = run_session(root.path(), &query, &mut shell);
    let stats = result.outcome.unwrap().stats;
    assert_eq!(stats["shells"], 1);
    assert_eq!(stats["render_wanted"], 1);
    assert_eq!(stats["phase.Render.refused.Unavailable"], 1);
    assert_eq!(stats["place.0.shell"], 1);
}

#[test]
fn passage_reports_answer_wrong_empty_and_unreadable() {
    let root = tempfile::tempdir().unwrap();
    for (id, url, answer, expected) in [
        (
            "right",
            "answer",
            "side button",
            (true, false, false, false),
        ),
        ("wrong", "answer", "top lever", (false, true, false, false)),
        ("empty", "empty", "side button", (false, false, true, false)),
        (
            "bad",
            "unreadable",
            "side button",
            (false, false, false, true),
        ),
        (
            "wall",
            "challenge",
            "side button",
            (false, false, false, true),
        ),
    ] {
        let mut source = ScriptedSource::new(|request| match request.url.path() {
            "/robots.txt" => scripted_body(request, "text/plain", "User-agent: *\nDisallow:\n"),
            "/answer" => scripted_body(
                request,
                "text/html",
                "<main><p>Reset the timer using the side button.</p></main>",
            ),
            "/empty" => scripted_body(
                request,
                "text/html",
                "<main><p>Geese migrate in autumn.</p></main>",
            ),
            "/challenge" => scripted_body(request, "text/html", "<title>Just a moment</title>"),
            _ => refuse(StatusRefusal::Server),
        });
        let query = session_query(id, "how to reset timer", &format!(
            "kind='passage'\nanswer='{answer}'\nfrom=['example.com']\npages=['https://example.com/{url}']"
        ));
        let result = run_session(root.path(), &query, &mut source);
        let score = result.score.passage.unwrap();
        assert_eq!(
            (
                score.answer_from_listed_source,
                score.wrong_answer_shown,
                score.nothing_returned,
                score.page_could_not_be_read
            ),
            expected,
            "{id}"
        );
    }
}

#[test]
fn actual_corpus_synthetic_entity_findings() {
    let root = tempfile::tempdir().unwrap();
    let evidence = repository().join("../../zephium-audit/impl/search-map");
    let paths = [
        evidence.join("corpus-v0.toml"),
        evidence.join("corpus-v1.toml"),
    ];
    let Some(corpus) = crate::map_bench::corpus::load_external_fixture(&paths).unwrap() else {
        return;
    };
    let mut source = ScriptedSource::new(|_| refuse(StatusRefusal::Client));
    let mut missing = Vec::new();
    for query in corpus
        .queries
        .iter()
        .filter(|q| matches!(q.label.kind, Kind::Place | Kind::Compare))
    {
        let result = run_session(root.path(), query, &mut source);
        if result.score.verdict == Verdict::NoOfficialSite {
            missing.push(query.label.id.as_str());
        } else {
            assert_ne!(
                result.score.reason,
                Reason::EngineSessionFailed,
                "{}",
                query.label.id
            );
        }
    }
    eprintln!("no_official_site IDs: {missing:?}");
}

#[test]
fn synthetic_session_report_matches_golden() {
    let root = tempfile::tempdir().unwrap();
    let corpus = corpus(vec![
        session_query("site", "Acme docs", ACME),
        session_query(
            "missing",
            "secret unanswered search",
            "kind='passage'\nanswer='private answer'",
        ),
    ]);
    init_engine(root.path(), &corpus, EngineKind::Session);
    let mut source = ScriptedSource::new(site);
    for query in &corpus.queries {
        let mut result = run_session(root.path(), query, &mut source);
        let outcome = result.outcome.as_mut().unwrap();
        let is_place = query.label.kind == Kind::Place;
        outcome.milestones.first_place = is_place.then(|| Duration::from_nanos(5));
        outcome.milestones.full = is_place.then(|| Duration::from_nanos(10));
        if is_place {
            outcome.stats.insert("memo.0.bytes".into(), 256);
            outcome.stats.insert("budget.Fetches".into(), 1);
        }
        outcome
            .stats
            .insert("budget.https://private.example/".into(), 42);
        let path = query_dir(root.path(), &query.label.id, false).unwrap();
        save_outcome(&path.join(OUTCOME), result).unwrap();
        write_json(&path.join(MEASURE), &Measure::default()).unwrap();
    }
    report::write(root.path(), None).unwrap();
    let markdown = fs::read_to_string(root.path().join("summary.md")).unwrap();
    assert!(!markdown.contains("https://"));
    assert!(!markdown.contains("Acme docs"));
    assert!(!markdown.contains("secret unanswered search"));
    let golden = r"# Search map benchmark

This run used synthetic results pages. Group checks and first-layer latency were not measured for those pages.

## Recording

| Scope | pass | partial | fail | stayed_out_ok | stayed_out_broken | blocked | not_recorded | not_measured | no_official_site |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| Total | 1 | 0 | 0 | 0 | 0 | 0 | 0 | 1 | 0 |
| passage | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 1 | 0 |
| place | 1 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 |
| Slice (unsliced) | 1 | 0 | 0 | 0 | 0 | 0 | 0 | 1 | 0 |

| Metric | Samples | p50 | p90 | Max |
| --- | ---: | ---: | ---: | ---: |
| children_per_place | 1 | 2 | 2 | 2 |
| cpu_ns | 0 | — | — | — |
| decoded_bytes | 2 | 0 | 0 | 0 |
| first_place_ns | 1 | 5 | 5 | 5 |
| full_ns | 1 | 10 | 10 | 10 |
| memo_bytes | 1 | 256 | — | 256 |
| peak_rss_delta_bytes | 0 | — | — | — |
| primary_sections_per_place | 1 | 3 | 3 | 3 |
| requests | 2 | 0 | 0 | 0 |
| secondary_sections_per_place | 1 | 2 | 2 | 2 |
| sections_per_place | 1 | 5 | 5 | 5 |
| wall_ns | 2 | 0 | 0 | 0 |
| wire_bytes | 2 | 0 | 0 | 0 |

| Refusal class | Count |
| --- | ---: |
| — | 0 |

Places with at least 3 sections — 1 of 1 place and compare sites.

| Session activity | Count |
| --- | ---: |
| Shells | 0 |
| Walls | 0 |
| Robots denials | 0 |
| Renders wanted | 0 |
| Anonymous results reads wanted | 0 |

| Source | Places |
| --- | ---: |
| navigation | 1 |
| footer | 1 |
| llms_txt | 1 |
| sitemap | 1 |
| linked_data | 0 |
| section_pages | 1 |

| Budget hit | Queries |
| --- | ---: |
| Fetches | 1 |

| Requests per query by phase | Samples | p50 | p90 | Max |
| --- | ---: | ---: | ---: | ---: |
| Results | 1 | 1 | 1 | 1 |
| AnonymousResults | 1 | 0 | 0 | 0 |
| Robots | 1 | 1 | 1 | 1 |
| Entry | 1 | 1 | 1 | 1 |
| Render | 1 | 0 | 0 | 0 |
| Section | 1 | 2 | 2 | 2 |
| Llms | 1 | 1 | 1 | 1 |
| Sitemap | 1 | 1 | 1 | 1 |
| Passage | 1 | 0 | 0 | 0 |

| Passage queries with listed pages | Count |
| --- | ---: |
| answered_from_listed_source | 0 |
| answered_wrong | 0 |
| nothing_returned | 0 |
| page_could_not_be_read | 0 |

Destinations scored as matched_locale — 0.

| Section query | Within two levels | Focus right |
| --- | --- | --- |
| `site` | true | false |

| Query ID | Verdict | Reason |
| --- | --- | --- |
| `missing` | not_measured | needs_results_page |

";
    assert_eq!(markdown, golden);
}

#[test]
fn compare_keeps_both_last_places_and_two_memos() {
    let root = tempfile::tempdir().unwrap();
    let query = session_query(
        "both",
        "Acme vs Example docs",
        "kind='compare'\nsites=['acme.com','example.com']\nwant=['example.com/manual']",
    );
    let mut source = ScriptedSource::new(|request| match request.url.path() {
        "/robots.txt" => scripted_body(request, "text/plain", "User-agent: *\nDisallow:\n"),
        "/" if request.url.host_str() == Some("acme.com") => {
            scripted_body(request, "text/html", "<nav><a href='/docs'>Docs</a></nav>")
        }
        "/" => scripted_body(
            request,
            "text/html",
            "<nav><a href='/manual'>Docs</a></nav>",
        ),
        _ => refuse(StatusRefusal::Client),
    });
    let output = run_session(root.path(), &query, &mut source);
    assert_eq!(output.score.verdict, Verdict::Pass);
    let outcome = output.outcome.unwrap();
    assert_eq!(outcome.stats["official_sites"], 2);
    assert_eq!(outcome.stats["place.0.sections"], 1);
    assert_eq!(outcome.stats["place.1.sections"], 1);
    assert_eq!(outcome.entries[0].url.host_str(), Some("acme.com"));
    assert_eq!(outcome.entries[1].url.host_str(), Some("example.com"));
    let directory = query_dir(root.path(), "both", false).unwrap();
    assert!(directory.join("memo.bin").is_file());
    assert!(directory.join("memo-2.bin").is_file());
}

#[test]
fn replay_refuses_a_different_engine_before_writing_a_run() {
    let root = tempfile::tempdir().unwrap();
    let corpus = corpus(vec![session_query("site", "Acme docs", ACME)]);
    init_engine(root.path(), &corpus, EngineKind::Session);
    let error = replay(
        root.path(),
        &corpus,
        vec![&corpus.queries[0]],
        &mut crate::map_bench::probe::ProbeEngine,
        EngineKind::Probe,
    )
    .unwrap_err();
    assert_eq!(error.class, "replay_engine_mismatch");
    assert!(!root.path().join("replay").exists());
}

fn save_row(root: &Path, path: &Path, request: &FetchRequest, result: FetchResult) {
    let mut journal = Journal::load(path).unwrap();
    let start = Duration::from_secs(10);
    let total = Duration::from_millis(10);
    let index = journal.reserve(request, path, start).unwrap();
    let transfer = Transfer {
        start,
        total,
        end: start + total,
        wire_bytes: result.as_ref().ok().map(|body| body.wire_bytes),
    };
    journal
        .finish(root, path, index, &result, transfer, None)
        .unwrap();
}

fn replay_row(path: &Path, request: &FetchRequest, cap: usize, timeout_ms: u64) -> FetchResult {
    let mut io = QueryIo::new(path, Journal::load(path).unwrap(), SESSION_CLOCK, None);
    let mut request = request.clone();
    let exact = cap == request.max_bytes && Duration::from_millis(timeout_ms) == request.timeout;
    request.max_bytes = cap;
    request.timeout = Duration::from_millis(timeout_ms);
    let result = io.fetch(request);
    let missing = result == Err(FetchRefusal::NotRecorded);
    assert_eq!(io.activity.not_recorded, u64::from(missing));
    assert_eq!(io.activity.matched_relaxed, u64::from(!exact && !missing));
    result
}

#[test]
fn replay_checks_body_length_truncation_and_completion_time() {
    let root = tempfile::tempdir().unwrap();
    let mut req = request("/robots.txt", Expectation::Text);
    req.max_bytes = 128 * 1024;
    for truncated in [false, true] {
        let path = query_dir(
            root.path(),
            if truncated { "partial" } else { "full" },
            true,
        )
        .unwrap();
        let mut body = scripted_body(&req, "text/plain", "abcd").unwrap();
        body.timings.total = Duration::from_millis(8);
        body.truncated = truncated;
        if truncated {
            body.bytes.resize(req.max_bytes, b'x');
            body.wire_bytes = req.max_bytes as u64;
        }
        save_row(root.path(), &path, &req, Ok(body.clone()));
        let missing = Err(FetchRefusal::NotRecorded);
        assert_eq!(replay_row(&path, &req, req.max_bytes, 9), missing);
        assert_eq!(replay_row(&path, &req, req.max_bytes, 10), Ok(body.clone()));
        if truncated {
            assert_eq!(replay_row(&path, &req, 512 * 1024, 3000), missing);
            body.bytes.truncate(3);
            assert_eq!(replay_row(&path, &req, 3, 10), Ok(body));
        } else {
            assert_eq!(replay_row(&path, &req, 3, 3000), missing);
            assert_eq!(replay_row(&path, &req, 4, 10), Ok(body.clone()));
            assert_eq!(replay_row(&path, &req, 512 * 1024, 3000), Ok(body));
        }
    }
}

#[test]
fn replay_uses_reader_time_for_journals_without_a_request_start() {
    let root = tempfile::tempdir().unwrap();
    let req = request("/docs", Expectation::Text);
    let path = query_dir(root.path(), "legacy", true).unwrap();
    let mut body = scripted_body(&req, "text/plain", "abcd").unwrap();
    body.timings.total = Duration::from_millis(8);
    save_row(root.path(), &path, &req, Ok(body.clone()));
    let journal = path.join("requests.jsonl");
    let mut row: serde_json::Value = serde_json::from_slice(&fs::read(&journal).unwrap()).unwrap();
    row.as_object_mut().unwrap().remove("elapsed_at_start");
    fs::write(journal, format!("{row}\n")).unwrap();
    assert_eq!(
        replay_row(&path, &req, 4, 7),
        Err(FetchRefusal::NotRecorded)
    );
    assert_eq!(replay_row(&path, &req, 4, 8), Ok(body));
}

#[test]
fn replay_refusals_require_evidence_for_the_requested_limits() {
    use zephium_public_fetch::SizeLimit;
    let root = tempfile::tempdir().unwrap();
    let req = request("/docs", Expectation::Html);
    for (index, class) in ReadRefusal::ALL
        .into_iter()
        .map(FetchRefusal::Reader)
        .chain([
            FetchRefusal::NotRecorded,
            FetchRefusal::Interrupted,
            FetchRefusal::Limit,
            FetchRefusal::Storage,
        ])
        .enumerate()
    {
        let path = query_dir(root.path(), &format!("case-{index}"), true).unwrap();
        save_row(root.path(), &path, &req, Err(class));
        for cap in [req.max_bytes - 1, req.max_bytes, req.max_bytes + 1] {
            for timeout in [2999, 3000, 3001] {
                let compatible = match class {
                    FetchRefusal::Reader(ReadRefusal::TooLarge(SizeLimit::Headers)) => true,
                    FetchRefusal::Reader(
                        ReadRefusal::TooLarge(SizeLimit::Wire) | ReadRefusal::Encoding,
                    ) => cap >= req.max_bytes,
                    FetchRefusal::Reader(ReadRefusal::Timeout(_)) => timeout <= 3000,
                    FetchRefusal::Reader(_) => true,
                    _ => cap == req.max_bytes && timeout == 3000,
                };
                let expected = if compatible {
                    class
                } else {
                    FetchRefusal::NotRecorded
                };
                assert_eq!(
                    replay_row(&path, &req, cap, timeout),
                    Err(expected),
                    "{class:?}"
                );
            }
        }
        if matches!(
            class,
            FetchRefusal::Reader(ReadRefusal::TooLarge(_) | ReadRefusal::Encoding)
        ) {
            for (cap, timeout, answered) in [
                (req.max_bytes, 9, false),
                (req.max_bytes, 10, true),
                (req.max_bytes * 2, 9, false),
                (req.max_bytes * 2, 10, true),
                (
                    1,
                    10,
                    class == FetchRefusal::Reader(ReadRefusal::TooLarge(SizeLimit::Headers)),
                ),
            ] {
                let expected = if answered {
                    class
                } else {
                    FetchRefusal::NotRecorded
                };
                assert_eq!(
                    replay_row(&path, &req, cap, timeout),
                    Err(expected),
                    "{class:?} {cap} {timeout}"
                );
            }
        }
    }
}

#[test]
fn replay_prefers_an_unclaimed_exact_row_then_uses_the_first_compatible_row() {
    let root = tempfile::tempdir().unwrap();
    let path = query_dir(root.path(), "priority", true).unwrap();
    let req = request("/docs", Expectation::Html);
    let mut other = req.clone();
    other.max_bytes += 1;
    save_row(root.path(), &path, &other, refuse(StatusRefusal::Denied));
    save_row(root.path(), &path, &req, refuse(StatusRefusal::Challenge));
    let mut io = QueryIo::new(&path, Journal::load(&path).unwrap(), SESSION_CLOCK, None);
    assert_eq!(io.fetch(req.clone()), refuse(StatusRefusal::Challenge));
    assert_eq!(io.activity.matched_relaxed, 0);
    assert_eq!(io.fetch(req.clone()), refuse(StatusRefusal::Denied));
    assert_eq!(io.activity.matched_relaxed, 1);
    assert_eq!(io.fetch(req), Err(FetchRefusal::NotRecorded));
}

#[test]
fn replay_response_refusal_requires_time_to_observe_it() {
    let root = tempfile::tempdir().unwrap();
    let path = query_dir(root.path(), "refusal", true).unwrap();
    let req = request("/docs", Expectation::Html);
    save_row(root.path(), &path, &req, refuse(StatusRefusal::Denied));
    assert_eq!(
        replay_row(&path, &req, req.max_bytes, 9),
        Err(FetchRefusal::NotRecorded)
    );
    assert_eq!(
        replay_row(&path, &req, req.max_bytes, 10),
        refuse(StatusRefusal::Denied)
    );
}

#[test]
fn completion_order_fixture_accepts_late_submissions() {
    let mut source = ScriptedSource::new(|_| refuse(StatusRefusal::Denied));
    source.reversed = true;
    for (token, start) in [8, 10, 12].into_iter().enumerate() {
        source.submit(
            token,
            request("/docs", Expectation::Html),
            Duration::from_secs(start),
        );
    }
    for token in [1, 2, 0] {
        let result = source.next().unwrap();
        assert_eq!(result.token, token);
        assert!(result.transfer.end >= result.transfer.start);
    }
}

#[test]
fn passage_page_counts_only_after_session_acceptance() {
    let root = tempfile::tempdir().unwrap();
    for (id, delay, expected) in [
        ("in-time", Duration::ZERO, 1),
        ("late", zephium_map::session::MAX_ELAPSED, 0),
    ] {
        let query = session_query(id, "how to reset timer", "kind='passage'\nanswer='side button'\nfrom=['example.com']\npages=['https://example.com/empty']");
        let path = query_dir(root.path(), &query.label.id, true).unwrap();
        let mut source = ScriptedSource::new(|request| {
            if request.url.path() == "/robots.txt" {
                scripted_body(request, "text/plain", "User-agent: *\n")
            } else {
                scripted_body(request, "text/html", "<p>Geese migrate in autumn.</p>")
            }
        });
        source.delay_html = delay;
        let mut io = QueryIo::new(
            &path,
            Journal::default(),
            SESSION_CLOCK,
            Some(Live {
                root: root.path(),
                source: &mut source,
                started: Instant::now(),
                prior_elapsed: Duration::ZERO,
            }),
        );
        let output = execute(
            &mut crate::map_bench::session::SessionEngine,
            &query,
            &mut io,
        );
        let outcome = output.outcome.unwrap();
        assert!(outcome.passages.is_empty());
        assert_eq!(
            outcome
                .stats
                .get("passage_pages_read")
                .copied()
                .unwrap_or(0),
            expected
        );
    }
}
