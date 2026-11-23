use super::*;
use zephium_app::home::{HomePage, ResultsDocument};
use zephium_core::home::{PageRow, Search};
use zephium_core::ids::{ItemId, SpaceId};
use zephium_core::ports::engine::PageHtml;
use zephium_map_driver::wire::{MapAlso, MapAlsoKind, MapEntry, MapNetwork, MapNotes, MapPlace};

fn place(seq: u32, slot: u8, title: &str) -> MapChange {
    MapChange::Place {
        seq,
        slot,
        place: MapPlace {
            title: title.into(),
            site: "Acme".into(),
            host: "acme.example".into(),
            sections: vec![entry(
                "aaaaaaaaaaaaaaaa",
                vec![entry("bbbbbbbbbbbbbbbb", Vec::new())],
            )],
            focus: None,
            social: vec![MapNetwork::GitHub],
            notes: MapNotes::default(),
        },
        stale: false,
        confidence: wire::MapConfidence::High,
        complete: true,
    }
}

fn entry(id: &str, children: Vec<MapEntry>) -> MapEntry {
    MapEntry {
        id: id.into(),
        tier: wire::MapTier::Primary,
        label: "Pricing".into(),
        role: None,
        group: None,
        off_site: false,
        menu_only: false,
        open: false,
        host: None,
        path: "/pricing".into(),
        read: None,
        children,
        more: false,
    }
}

fn first_layer(seq: u32, also: usize) -> MapChange {
    MapChange::FirstLayer {
        seq,
        official: None,
        social: Vec::new(),
        organic: 3,
        also: (0..also)
            .map(|index| MapAlso {
                title: format!("Also {index}"),
                host: "other.example".into(),
                kind: MapAlsoKind::Page,
            })
            .collect(),
    }
}

fn end(seq: u32) -> MapChange {
    MapChange::End {
        seq,
        outcome: wire::MapOutcome::Done,
        elapsed_ms: 10,
    }
}

#[test]
fn a_map_keeps_the_newest_change_of_each_kind_in_the_order_sent() {
    let mut folded = Folded::default();
    assert!(folded.fold(&first_layer(1, 0)));
    assert!(folded.fold(&place(2, 0, "Acme")));
    assert!(folded.fold(&place(3, 1, "Other")));
    assert!(folded.fold(&place(4, 0, "Acme again")));
    assert!(
        !folded.fold(&place(4, 0, "a repeat")),
        "a sequence is kept once"
    );
    assert!(
        !folded.fold(&first_layer(2, 1)),
        "an older change never wins"
    );
    assert!(!folded.ended());
    assert!(folded.fold(&end(5)));
    assert!(folded.ended());
    let seqs: Vec<u32> = folded.changes().iter().map(MapChange::seq).collect();
    assert_eq!(seqs, vec![1, 3, 4, 5]);
    assert_eq!(folded.seq(), Some(5));
    assert!(matches!(
        &folded.changes()[2],
        MapChange::Place { place, .. } if place.title == "Acme again"
    ));
    assert!(folded.fold(&place(9, 7, "no such slot")));
    assert_eq!(
        folded.changes().len(),
        4,
        "a slot the map has not is never kept"
    );
}

#[test]
fn every_page_a_map_offers_is_resolved_once_and_outlives_its_driver() {
    let mut targets = Targets::default();
    let lookup = |slot: u8, target: &MapTarget| -> Option<tauri::Url> {
        let path = match target {
            MapTarget::Entry { id } if id == "bbbbbbbbbbbbbbbb" => return None,
            MapTarget::Entry { id } => format!("entry/{id}"),
            MapTarget::Site => "site".into(),
            MapTarget::Social { .. } => "social".into(),
            MapTarget::Also { index } => format!("also/{index}"),
        };
        tauri::Url::parse(&format!("https://slot{slot}.example/{path}")).ok()
    };
    targets.resolve(&place(1, 0, "Acme"), lookup);
    targets.resolve(&first_layer(2, 2), lookup);
    let entry = MapTarget::Entry {
        id: "aaaaaaaaaaaaaaaa".into(),
    };
    assert_eq!(
        targets.get(0, &entry).unwrap().as_str(),
        "https://slot0.example/entry/aaaaaaaaaaaaaaaa"
    );
    assert_eq!(targets.get(1, &entry), None, "a page belongs to its slot");
    assert_eq!(
        targets.get(
            0,
            &MapTarget::Entry {
                id: "bbbbbbbbbbbbbbbb".into()
            }
        ),
        None,
        "a page the map offers no address for stays closed"
    );
    assert!(targets.get(0, &MapTarget::Site).is_some());
    assert!(targets
        .get(
            0,
            &MapTarget::Social {
                network: MapNetwork::GitHub
            }
        )
        .is_some());
    assert!(targets.get(1, &MapTarget::Also { index: 1 }).is_some());
    assert_eq!(targets.get(0, &MapTarget::Also { index: 2 }), None);

    targets.resolve(
        &MapChange::PlaceFailed {
            seq: 3,
            slot: 0,
            failure: wire::MapFailure::Unavailable,
        },
        lookup,
    );
    assert_eq!(targets.get(0, &entry), None);
    assert!(targets.get(0, &MapTarget::Also { index: 0 }).is_some());
    assert_eq!(entry_of(&entry), "aaaaaaaaaaaaaaaa");
    assert!(entry_of(&MapTarget::Social {
        network: MapNetwork::GitHub
    })
    .starts_with("social-"));
}

fn records(title: &str, host: &str, text: &str) -> HomeRecords {
    let profile = ProfileId::generate();
    let space = SpaceId::generate();
    let tab = ItemId::generate();
    HomeRecords {
        profile,
        space,
        searches: vec![Search {
            id: SearchId(0x1f),
            text: text.into(),
            at_ms: 1_700_000_000_000,
            kind: SearchKind::Place,
            engine: SearchEngine::default(),
            profile,
            space,
            map: Some(MapId(7)),
            results: Some(tab),
            results_url: Some("https://duckduckgo.com/?q=acme".into()),
            pages: Vec::new(),
        }],
        pages: vec![vec![HomePage {
            tab: Some(tab),
            row: Some(PageRow {
                slot: 0,
                entry: "aaaaaaaaaaaaaaaa".into(),
            }),
            open: true,
            title: title.into(),
            host: host.into(),
        }]],
    }
}

fn idle_host() -> Arc<Host> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    Host::new(
        Arc::new(|_| true),
        Arc::new(|_, _| {}),
        runtime.handle().clone(),
        None,
    )
}

#[test]
fn a_snapshot_names_no_address_whatever_the_pages_call_themselves() {
    let host = idle_host();
    let hostile = [
        (
            "Visit https://evil.example/login now",
            "WWW.Acme.Example.",
            "acme",
        ),
        (
            "<script>x</script>\u{202e}Title\n\twith\u{0}breaks",
            "evil.example/path",
            "acme\nhttp://x",
        ),
        ("www.phish.example", "a b", "q=1 ://"),
    ];
    for (title, host_name, text) in hostile {
        let shot = snapshot(&host, records(title, host_name, text));
        let json = serde_json::to_string(&shot).unwrap();
        let page = &shot.searches[0].pages[0];
        assert!(!page.title.contains('\n') && !page.title.contains('\u{0}'));
        assert!(!json.contains("duckduckgo"), "{json}");
        if text.contains("://") || text.contains("http") {
            continue;
        }
        assert!(!json.contains("http"), "{json}");
        assert!(!json.contains("://"), "{json}");
    }
    let shot = snapshot(&host, records("Acme pricing", "WWW.Acme.Example.", "acme"));
    assert_eq!(shot.searches[0].pages[0].host, "acme.example");
    assert_eq!(shot.searches[0].pages[0].title, "Acme pricing");
    assert!(shot.searches[0].results_open);
    assert_eq!(shot.searches[0].map, None, "a map the host no longer has");
    assert_eq!(shot.searches[0].id, "000000000000001f");
}

#[test]
fn only_a_sure_place_or_a_comparison_starts_a_map() {
    assert_eq!(map_kind("stripe"), Ok(MapKind::Place));
    assert_eq!(map_kind("figma versus sketch"), Ok(MapKind::Compare));
    for words in ["how", "or stripe", "", &"x".repeat(5000)] {
        assert_eq!(map_kind(words), Err(MapRefusal::NotAMap), "{words:?}");
    }
    let host = idle_host();
    let start = MapStart {
        map: MapId(1),
        search: SearchId(1),
        window: 1,
        profile: ProfileId::generate(),
        private: false,
        engine: SearchEngine::default(),
        query: "stripe".into(),
    };
    assert_eq!(
        HostPort(host).start(start),
        Err(MapRefusal::Unavailable),
        "nothing to read pages through"
    );
}

struct Offline;
impl zephium_map_driver::Fetcher for Offline {
    fn fetch(
        &self,
        _: zephium_map_driver::FetchRequest,
    ) -> Reading<Result<zephium_map::session::FetchResult, Failure>> {
        Box::pin(async { Err(Failure::Unavailable) })
    }
}

#[test]
fn a_full_window_refuses_a_map_until_one_has_finished() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .unwrap();
    let never: Dispatch = Arc::new(|_| false);
    let mut maps = HashMap::new();
    for id in 1..=MAX_LIVE_MAPS_PER_WINDOW as u64 {
        assert!(Host::make_room(&mut maps, 1));
        let io = Io {
            results: Arc::new(source(never.clone(), RESULTS_READ_TIMEOUT)),
            fetch: Arc::new(Offline),
            memo: Arc::new(MemoryMemo::new()),
            clock: Arc::new(SystemClock::new()),
        };
        let request = MapRequest {
            query: "stripe".into(),
            engine: SearchEngine::DuckDuckGo,
            session_id: id,
            profile: "p".into(),
            private: false,
        };
        let (driver, _changes) = Driver::start(request, io, runtime.handle()).unwrap();
        maps.insert(
            MapId(id),
            LiveMap {
                search: SearchId(id),
                window: 1,
                profile: ProfileId::generate(),
                driver: Some(driver),
                folded: Folded::default(),
                targets: Targets::default(),
            },
        );
    }
    assert!(!Host::make_room(&mut maps, 1), "four maps still running");
    assert!(Host::make_room(&mut maps, 2), "another window has room");
    maps.get_mut(&MapId(2)).unwrap().folded.fold(&end(1));
    maps.get_mut(&MapId(3)).unwrap().folded.fold(&end(1));
    assert!(Host::make_room(&mut maps, 1));
    assert!(
        maps[&MapId(2)].driver.is_none(),
        "the oldest finished map gives way"
    );
    assert!(maps[&MapId(3)].driver.is_some());
    assert_eq!(maps.len(), MAX_LIVE_MAPS_PER_WINDOW, "its view stays");
}

type Answers = Arc<Mutex<Vec<Result<ResultsDocument, ResultsRefusal>>>>;

/// A shell that answers each read with the next canned answer and records
/// the commands it was sent.
fn shell(answers: Answers, sent: Arc<Mutex<Vec<&'static str>>>) -> Dispatch {
    let unanswered = Arc::new(Mutex::new(Vec::new()));
    Arc::new(move |command| {
        let Command::Home(command) = command else {
            return false;
        };
        let (name, done) = match *command {
            HomeCommand::ReadResults { done, .. } => ("results", Some(done)),
            HomeCommand::ReadAgain { done, .. } => ("again", Some(done)),
            HomeCommand::ReadAbandoned { .. } => ("abandoned", None),
            _ => ("other", None),
        };
        sent.lock().unwrap().push(name);
        if let Some(done) = done {
            let mut answers = answers.lock().unwrap();
            if answers.is_empty() {
                unanswered.lock().unwrap().push(done);
            } else {
                done.finish(answers.remove(0));
            }
        }
        true
    })
}

fn document(html: &str) -> Result<ResultsDocument, ResultsRefusal> {
    Ok(ResultsDocument {
        url: tauri::Url::parse("https://duckduckgo.com/?q=acme").unwrap(),
        html: PageHtml(html.into()),
        truncated: false,
    })
}

fn source(dispatch: Dispatch, timeout: Duration) -> ShellResults {
    ShellResults {
        dispatch,
        map: MapId(3),
        engine: SearchEngine::DuckDuckGo,
        query: "acme".into(),
        timeout,
        interval: Duration::from_millis(1),
    }
}

fn run<T>(future: impl std::future::Future<Output = T>) -> T {
    tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap()
        .block_on(future)
}

#[test]
fn a_results_page_is_read_again_until_it_has_drawn_its_results() {
    let drawn = "<main><article><h2><a href='https://acme.com/'>Acme</a></h2></article></main>";
    let answers: Answers = Arc::new(Mutex::new(vec![
        document("<main></main>"),
        document("<main><script>draw()</script></main>"),
        document(drawn),
    ]));
    let sent = Arc::new(Mutex::new(Vec::new()));
    let results = source(shell(answers, sent.clone()), RESULTS_READ_TIMEOUT);
    let url = tauri::Url::parse("https://duckduckgo.com/?q=acme").unwrap();
    let page = run(results.load(url.clone())).unwrap();
    assert_eq!(page.html, drawn);
    assert_eq!(
        *sent.lock().unwrap(),
        vec!["results", "again", "again", "abandoned"]
    );

    // A page that says it has nothing, or asks for consent, is final.
    let answers: Answers = Arc::new(Mutex::new(vec![document(
        "<main><p>No results found for acme.</p></main>",
    )]));
    let sent = Arc::new(Mutex::new(Vec::new()));
    let results = source(shell(answers, sent.clone()), RESULTS_READ_TIMEOUT);
    assert!(run(results.load(url.clone())).is_ok());
    assert_eq!(*sent.lock().unwrap(), vec!["results", "abandoned"]);

    let answers: Answers = Arc::new(Mutex::new(vec![Err(ResultsRefusal::Redirected)]));
    let results = source(shell(answers, Arc::default()), RESULTS_READ_TIMEOUT);
    assert_eq!(run(results.load(url.clone())).err(), Some(Failure::Consent));

    let answers: Answers = Arc::new(Mutex::new(vec![Err(ResultsRefusal::Unknown)]));
    let results = source(shell(answers, Arc::default()), RESULTS_READ_TIMEOUT);
    assert_eq!(run(results.load(url)).err(), Some(Failure::Unavailable));
}

#[test]
fn an_unanswered_read_times_out_and_tells_the_shell() {
    let sent = Arc::new(Mutex::new(Vec::new()));
    let results = source(
        shell(Arc::default(), sent.clone()),
        Duration::from_millis(20),
    );
    let url = tauri::Url::parse("https://duckduckgo.com/?q=acme").unwrap();
    assert_eq!(run(results.load(url)).err(), Some(Failure::Timeout));
    assert_eq!(*sent.lock().unwrap(), vec!["results", "abandoned"]);

    let refused: Dispatch = Arc::new(|_| false);
    let results = source(refused, RESULTS_READ_TIMEOUT);
    let url = tauri::Url::parse("https://duckduckgo.com/?q=acme").unwrap();
    assert_eq!(run(results.load(url)).err(), Some(Failure::Unavailable));
}

#[test]
fn malformed_ids_never_name_a_search() {
    for raw in [
        "",
        "1f",
        "000000000000001F",
        "0000000000000000",
        "000000000000001f0",
        "zzzzzzzzzzzzzzzz",
    ] {
        assert_eq!(search_id(raw), None, "{raw:?}");
    }
    assert_eq!(search_id("000000000000001f"), Some(SearchId(0x1f)));
    for host in ["", "a b", "evil.example/path", "x:y", &"a".repeat(300)] {
        assert_eq!(shown_host(host), "", "{host:?}");
    }
}

#[test]
fn the_frame_listens_for_every_home_event_by_name() {
    let events = crate::frame_sources::SRC_SHARED_IPC_NATIVE_EVENTS_TS;
    for name in [EVENT_HOME_MAP_CHANGED, EVENT_HOME_CHANGED, EVENT_HOME_SHOW] {
        assert!(events.contains(&format!("\"{name}\"")), "{name}");
    }
}

#[test]
fn only_the_main_window_may_ask_for_home() {
    assert!(caller_allowed(HOME_POLICY, MAIN_LABEL));
    for label in [
        crate::overlay::PANEL_LABEL,
        "mini-1",
        "01M83E1GK2ZKG7ZHYG7HSW1VC0",
        "content",
        "",
    ] {
        assert!(!caller_allowed(HOME_POLICY, label), "{label:?}");
    }
    let source = include_str!("../home.rs");
    let commands: Vec<&str> = source.split("#[tauri::command]").skip(1).collect();
    assert_eq!(commands.len(), 6);
    for command in commands {
        let body = &command[..command.find("\n}\n").unwrap()];
        assert!(
            body.contains("authorize(&caller, HOME_POLICY,") || body.contains("home_intent("),
            "{body}"
        );
    }
    assert!(source.contains("if !authorize(caller, HOME_POLICY, name) {"));
    assert_eq!(source.matches("CallerPolicy::").count(), 1);
}

#[test]
fn a_malformed_request_names_no_search_and_no_entry() {
    let search = "000000000000001f";
    let entry = "0123456789abcdef";
    let named = Some(SearchId(0x1f));
    let at = |id: &str| MapTarget::Entry { id: id.into() };
    assert_eq!(target_request(search, 0, &at(entry)), named);
    assert_eq!(target_request(search, 1, &MapTarget::Site), named);
    assert_eq!(
        target_request(
            search,
            0,
            &MapTarget::Also {
                index: (wire::MAX_ALSO - 1) as u8
            }
        ),
        named
    );
    let long_search = format!("{search}0");
    let long_entry = format!("{entry}0");
    for (search, slot, target) in [
        (long_search.as_str(), 0, at(entry)),
        ("000000000000001F", 0, at(entry)),
        ("0000000000000000", 0, at(entry)),
        (search, SLOTS, at(entry)),
        (search, u8::MAX, MapTarget::Site),
        (search, 0, at(&long_entry)),
        (search, 0, at("0123456789ABCDEF")),
        (search, 0, at("0123456789abcdeg")),
        (search, 0, at("")),
        (
            search,
            0,
            MapTarget::Also {
                index: wire::MAX_ALSO as u8,
            },
        ),
    ] {
        assert_eq!(target_request(search, slot, &target), None, "{target:?}");
    }

    assert_eq!(inside_request(search, 1, entry), Ok(SearchId(0x1f)));
    for (search, slot, entry, refusal) in [
        (
            long_search.as_str(),
            0,
            entry,
            HomeInsideRefusalV1::SearchUnknown,
        ),
        (
            "not a search id!",
            0,
            entry,
            HomeInsideRefusalV1::SearchUnknown,
        ),
        (search, SLOTS, entry, HomeInsideRefusalV1::UnknownEntry),
        (
            search,
            0,
            long_entry.as_str(),
            HomeInsideRefusalV1::UnknownEntry,
        ),
        (
            search,
            0,
            "0123456789ABCDEF",
            HomeInsideRefusalV1::UnknownEntry,
        ),
    ] {
        assert_eq!(inside_request(search, slot, entry), Err(refusal));
    }
}

#[test]
fn a_private_search_asks_the_driver_for_a_private_map() {
    for private in [false, true] {
        let request = map_request(&MapStart {
            map: MapId(7),
            search: SearchId(9),
            window: 1,
            profile: ProfileId::generate(),
            private,
            engine: SearchEngine::default(),
            query: "coffee near shibuya".into(),
        });
        assert_eq!(request.private, private);
        assert_eq!(request.session_id, 7);
    }
}
