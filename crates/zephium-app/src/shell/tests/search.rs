use super::*;

fn search_sink() -> (Arc<Mutex<Vec<SearchResults>>>, EmitFn) {
    let seen: Arc<Mutex<Vec<SearchResults>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = seen.clone();
    let emit: EmitFn = Box::new(move |p| {
        if let Projection::Search(r) = p {
            sink.lock().unwrap().push(r);
        }
    });
    (seen, emit)
}

#[test]
fn search_ranks_tabs_primary_action_commands_and_history() {
    let (seen, emit) = search_sink();
    let store = Arc::new(FakeStore {
        history: vec![zephium_core::ports::store::HistoryHit {
            url: "https://blog.example.com/".into(),
            title: "Example Blog".into(),
            last_visit: 1,
        }],
        ..Default::default()
    });
    let mut shell = Shell::new(
        Arc::new(FakeEngine::default()),
        store,
        Arc::new(FakeChrome),
        emit,
    );
    shell.handle(Command::SetWindowSize(Size::new(1200.0, 800.0)));
    shell.handle(Command::Bootstrap);
    let id = shell.windows.focused().and_then(|w| w.active).unwrap();
    shell.handle(Command::Navigate {
        id,
        input: "example.com".into(),
    });
    shell.handle(Command::Engine(EngineEvent::TitleChanged {
        id,
        title: "Example Site".into(),
    }));

    shell.handle(Command::Search("example".into()));
    let last = seen.lock().unwrap().last().unwrap().clone();
    assert_eq!(last.query, "example");
    // Sections hold a fixed order, and what the user typed is always the row
    // Enter runs. A prefix-matching tab title used to outrank it, so typing
    // two letters could switch tabs when the user meant to search.
    let kinds: Vec<&str> = last.results.iter().map(|r| r.kind.as_str()).collect();
    assert_eq!(kinds, ["search", "tab", "history"]);
    assert!(matches!(
        &last.results[0].action,
        SearchAction::OpenUrl { .. }
    ));
    assert!(matches!(
        &last.results[1].action,
        SearchAction::ActivateTab { id: tab } if *tab == id.to_string()
    ));
    // Page-derived image delivery stays absent until a sandboxed broker
    // exists; search projections must not recreate the old protocol URL.
    assert!(last.results[1].icon.is_none());

    shell.handle(Command::Search("reload".into()));
    let last = seen.lock().unwrap().last().unwrap().clone();
    assert!(last.results.iter().any(|r| r.kind == "command"
        && matches!(&r.action, SearchAction::RunCommand { id } if id == "nav.reload")));

    shell.handle(Command::Search("example.com".into()));
    let last = seen.lock().unwrap().last().unwrap().clone();
    assert!(last.results.iter().any(|r| r.kind == "url"));

    shell.handle(Command::Search("".into()));
    let last = seen.lock().unwrap().last().unwrap().clone();
    assert!(last.results.iter().all(|r| r.kind == "tab"));
}

#[test]
fn stale_history_reply_cannot_replace_a_newer_launcher_query() {
    let (seen, emit) = search_sink();
    let mut shell = Shell::new(
        Arc::new(FakeEngine::default()),
        Arc::new(FakeStore::default()),
        Arc::new(FakeChrome),
        emit,
    );
    shell.handle(Command::SetWindowSize(Size::new(1200.0, 800.0)));
    shell.handle(Command::Bootstrap);
    shell.store_reads = Some(StoreReadQueue::new());
    let profile = shell.windows.focused().unwrap().profile;

    shell.handle(Command::Search("old".into()));
    let old_generation = shell.search.pending.as_ref().unwrap().generation;
    shell.handle(Command::Search("new".into()));
    let new_generation = shell.search.pending.as_ref().unwrap().generation;
    shell.handle(Command::StoreRead(StoreReadResult::History {
        generation: old_generation,
        profile,
        query: "old".into(),
        hits: vec![zephium_core::ports::store::HistoryHit {
            url: "https://old.example/".into(),
            title: "Old".into(),
            last_visit: 1,
        }],
    }));
    assert_eq!(
        shell
            .search
            .pending
            .as_ref()
            .map(|pending| pending.generation),
        Some(new_generation)
    );
    assert_eq!(seen.lock().unwrap().last().unwrap().query, "new");

    shell.handle(Command::StoreRead(StoreReadResult::History {
        generation: new_generation,
        profile,
        query: "new".into(),
        hits: vec![zephium_core::ports::store::HistoryHit {
            url: "https://new.example/".into(),
            title: "New".into(),
            last_visit: 2,
        }],
    }));
    let last = seen.lock().unwrap().last().unwrap().clone();
    assert_eq!(last.query, "new");
    assert!(last.results.iter().any(|result| {
        matches!(&result.action, SearchAction::OpenUrl { url } if url == "https://new.example/")
    }));
    assert!(last.results.iter().all(|result| {
        !matches!(&result.action, SearchAction::OpenUrl { url } if url == "https://old.example/")
    }));
}

#[test]
fn newtab_search_cannot_execute_after_its_tab_has_navigated() {
    let (seen, emit) = search_sink();
    let mut shell = Shell::new(
        Arc::new(FakeEngine::default()),
        Arc::new(FakeStore::default()),
        Arc::new(FakeChrome),
        emit,
    );
    shell.handle(Command::SetWindowSize(Size::new(1200.0, 800.0)));
    shell.handle(Command::Bootstrap);
    let window = shell.windows.focused().unwrap();
    let id = window.active.unwrap();
    let context = zephium_ipc::SearchContext {
        window_id: window.id.to_string(),
        profile_id: window.profile.to_string(),
        space_id: window.space.to_string(),
        session_id: format!("newtab:{id}:1"),
        request_id: "request-1".into(),
    };
    shell.handle(Command::SearchScoped {
        query: "example.com".into(),
        context: Box::new(context.clone()),
    });
    let action = seen.lock().unwrap().last().unwrap().results[0]
        .action
        .clone();
    shell.handle(Command::Navigate {
        id,
        input: "other.example".into(),
    });
    let disposition = shell.operation_run_search_action(context, action, false);
    assert_eq!(disposition.outcome, OperationOutcome::Rejected);
}

#[test]
fn local_scopes_never_offer_a_web_search() {
    let (seen, emit) = search_sink();
    let mut shell = Shell::new(
        Arc::new(FakeEngine::default()),
        Arc::new(FakeStore::default()),
        Arc::new(FakeChrome),
        emit,
    );
    shell.handle(Command::Bootstrap);
    for query in [
        "@notes private",
        "@tabs private",
        "@history private",
        "> reload",
    ] {
        shell.handle(Command::Search(query.into()));
        assert!(seen
            .lock()
            .unwrap()
            .last()
            .unwrap()
            .results
            .iter()
            .all(|result| result.kind != "search" && result.kind != "url"));
    }
}

#[test]
fn empty_local_scope_stays_pending_until_its_providers_finish() {
    let (seen, emit) = search_sink();
    let mut shell = Shell::new(
        Arc::new(FakeEngine::default()),
        Arc::new(FakeStore::default()),
        Arc::new(FakeChrome),
        emit,
    );
    shell.handle(Command::Bootstrap);
    let window = shell.windows.focused().unwrap();
    let context = zephium_ipc::SearchContext {
        window_id: window.id.to_string(),
        profile_id: window.profile.to_string(),
        space_id: window.space.to_string(),
        session_id: "panel-session".into(),
        request_id: "first".into(),
    };
    shell.handle(Command::SearchScoped {
        query: "@notes first".into(),
        context: Box::new(context.clone()),
    });
    assert!(seen.lock().unwrap().last().unwrap().pending);
    let next = zephium_ipc::SearchContext {
        request_id: "second".into(),
        ..context.clone()
    };
    shell.handle(Command::SearchScoped {
        query: "@notes second".into(),
        context: Box::new(next.clone()),
    });
    shell.search_supplementary_finished(context, "@notes first".into());
    assert!(seen.lock().unwrap().last().unwrap().pending);
    shell.search_supplementary_finished(next, "@notes second".into());
    let seen = seen.lock().unwrap();
    let result = seen.last().unwrap();
    assert!(!result.pending);
    assert!(result.results.is_empty());
    assert_eq!(result.query, "@notes second");
}

#[test]
fn late_providers_extend_their_own_section_without_reordering_the_list() {
    let (seen, emit) = search_sink();
    let store = Arc::new(FakeStore {
        history: vec![zephium_core::ports::store::HistoryHit {
            url: "https://blog.example.com/".into(),
            title: "Example Blog".into(),
            last_visit: 1,
        }],
        ..Default::default()
    });
    let mut shell = Shell::new(
        Arc::new(FakeEngine::default()),
        store,
        Arc::new(FakeChrome),
        emit,
    );
    shell.handle(Command::SetWindowSize(Size::new(1200.0, 800.0)));
    shell.handle(Command::Bootstrap);
    let id = shell.windows.focused().and_then(|w| w.active).unwrap();
    shell.handle(Command::Navigate {
        id,
        input: "example.com".into(),
    });
    shell.handle(Command::Engine(EngineEvent::TitleChanged {
        id,
        title: "Example Site".into(),
    }));
    let window = shell.windows.focused().unwrap();
    let context = zephium_ipc::SearchContext {
        window_id: window.id.to_string(),
        profile_id: window.profile.to_string(),
        space_id: window.space.to_string(),
        session_id: "panel-session".into(),
        request_id: "first".into(),
    };
    shell.handle(Command::SearchScoped {
        query: "example".into(),
        context: Box::new(context.clone()),
    });
    let settled: Vec<String> = seen
        .lock()
        .unwrap()
        .last()
        .unwrap()
        .results
        .iter()
        .map(|result| result.kind.clone())
        .collect();

    // Notes and online suggestions arrive well after the local set. Each must
    // land in its own section; nothing already on screen may move relative to
    // anything else, because the user is reading and aiming at those rows.
    shell.handle(Command::SearchAdditional {
        context: Box::new(context.clone()),
        query: "example".into(),
        results: vec![SearchResult {
            kind: "note".into(),
            title: "Example note".into(),
            detail: "Note".into(),
            icon: None,
            action: SearchAction::OpenNote {
                id: "01ARZ3NDEKTSV4RRFFQ69G5FAV".into(),
            },
        }],
    });
    shell.handle(Command::SearchAdditional {
        context: Box::new(context),
        query: "example".into(),
        results: vec![SearchResult {
            kind: "suggestion".into(),
            title: "example domain".into(),
            detail: "DuckDuckGo".into(),
            icon: None,
            action: SearchAction::OpenUrl {
                url: "https://duckduckgo.com/?q=example+domain".into(),
            },
        }],
    });
    let merged: Vec<String> = seen
        .lock()
        .unwrap()
        .last()
        .unwrap()
        .results
        .iter()
        .map(|result| result.kind.clone())
        .collect();
    assert_eq!(merged, ["search", "suggestion", "tab", "history", "note"]);
    // Every row that was already visible kept both its identity and its
    // relative position.
    let retained: Vec<&String> = merged
        .iter()
        .filter(|kind| settled.contains(kind))
        .collect();
    assert_eq!(retained, settled.iter().collect::<Vec<_>>());
}

#[test]
fn one_talkative_provider_cannot_starve_the_other_sections() {
    let (seen, emit) = search_sink();
    let mut shell = Shell::new(
        Arc::new(FakeEngine::default()),
        Arc::new(FakeStore::default()),
        Arc::new(FakeChrome),
        emit,
    );
    shell.handle(Command::SetWindowSize(Size::new(1200.0, 800.0)));
    shell.handle(Command::Bootstrap);
    let window = shell.windows.focused().unwrap();
    let context = zephium_ipc::SearchContext {
        window_id: window.id.to_string(),
        profile_id: window.profile.to_string(),
        space_id: window.space.to_string(),
        session_id: "panel-session".into(),
        request_id: "first".into(),
    };
    shell.handle(Command::SearchScoped {
        query: "example".into(),
        context: Box::new(context.clone()),
    });
    // Six admissible notes against a note capacity of two.
    shell.handle(Command::SearchAdditional {
        context: Box::new(context),
        query: "example".into(),
        results: (0..6)
            .map(|index| SearchResult {
                kind: "note".into(),
                title: format!("Example note {index}"),
                detail: "Note".into(),
                icon: None,
                action: SearchAction::OpenNote {
                    id: format!("01ARZ3NDEKTSV4RRFFQ69G5FA{index}"),
                },
            })
            .collect(),
    });
    let seen = seen.lock().unwrap();
    let last = seen.last().unwrap();
    assert_eq!(
        last.results.iter().filter(|r| r.kind == "note").count(),
        zephium_core::search::kind_capacity("note")
    );
    // The typed action survives a flood of late results.
    assert_eq!(last.results[0].kind, "search");
    assert!(last.results.len() <= zephium_core::search::MAX_RESULTS);
}

#[test]
fn new_tab_offers_places_to_go_and_leaves_commands_to_the_launcher() {
    let (seen, emit) = search_sink();
    let mut shell = Shell::new(
        Arc::new(FakeEngine::default()),
        Arc::new(FakeStore::default()),
        Arc::new(FakeChrome),
        emit,
    );
    shell.handle(Command::SetWindowSize(Size::new(1200.0, 800.0)));
    shell.handle(Command::Bootstrap);
    let window = shell.windows.focused().unwrap();
    let id = window.active.unwrap();
    let newtab = zephium_ipc::SearchContext {
        window_id: window.id.to_string(),
        profile_id: window.profile.to_string(),
        space_id: window.space.to_string(),
        session_id: format!("newtab:{id}:1"),
        request_id: "request-1".into(),
    };
    shell.handle(Command::SearchScoped {
        query: "reload".into(),
        context: Box::new(newtab.clone()),
    });
    let last = seen.lock().unwrap().last().unwrap().clone();
    assert!(
        last.results.iter().all(|result| result.kind != "command"),
        "the field is an address bar, not a command palette: {:?}",
        last.results.iter().map(|r| &r.kind).collect::<Vec<_>>()
    );
    // The typed action is still offered, so the field always has a default.
    assert_eq!(
        last.results.first().map(|result| result.kind.as_str()),
        Some("search")
    );

    // The launcher is the surface built for commands and keeps them.
    let panel = zephium_ipc::SearchContext {
        session_id: "panel-session".into(),
        ..newtab
    };
    shell.handle(Command::SearchScoped {
        query: "reload".into(),
        context: Box::new(panel),
    });
    let last = seen.lock().unwrap().last().unwrap().clone();
    assert!(last.results.iter().any(|result| result.kind == "command"
        && matches!(&result.action, SearchAction::RunCommand { id } if id == "nav.reload")));
}

/// Everything between the field and SQLite, with no doubles: a real profile
/// database, the real read queue, and the real worker loop. The pieces each
/// had coverage; the seam between them had none, and a history row has to
/// cross all of it.
#[test]
fn history_reaches_the_field_through_the_real_store_and_read_queue() {
    let directory = tempfile::tempdir().unwrap();
    let store = Arc::new(zephium_store::SqliteStore::open(directory.path()).unwrap());
    let (seen, emit) = search_sink();
    let mut shell = Shell::new(
        Arc::new(FakeEngine::default()),
        store.clone(),
        Arc::new(FakeChrome),
        emit,
    );
    shell.handle(Command::SetWindowSize(Size::new(1200.0, 800.0)));
    shell.handle(Command::Bootstrap);
    let profile = shell.windows.focused().unwrap().profile;
    store.record_visit(
        profile,
        "https://www.notion.so/workspace".into(),
        "Notion".into(),
    );
    // A loaded test machine can outlast the default barrier.
    assert!(store.flush_until(std::time::Instant::now() + std::time::Duration::from_secs(30)));

    let queue = crate::store_reads::StoreReadQueue::new();
    shell.store_reads = Some(queue.clone());
    // The scoped command is what both shipping surfaces send; `Search` is the
    // older unscoped entry point.
    let window = shell.windows.focused().unwrap();
    let id = window.active.unwrap();
    let context = zephium_ipc::SearchContext {
        window_id: window.id.to_string(),
        profile_id: window.profile.to_string(),
        space_id: window.space.to_string(),
        session_id: format!("newtab:{id}:1"),
        request_id: "request-1".into(),
    };
    shell.handle(Command::SearchScoped {
        query: "notion".into(),
        context: Box::new(context),
    });

    // Drive the worker exactly as the actor's reader thread does, then hand
    // its reply back the way the command queue would.
    let reader_store: SharedStore = store.clone();
    let reader_queue = queue.clone();
    let (tx, rx) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        crate::store_reads::run_for_test(reader_store, reader_queue, tx);
    });
    let result = rx
        .recv_timeout(std::time::Duration::from_secs(5))
        .expect("the reader must answer a requested history search");
    queue.stop();
    worker.join().unwrap();
    shell.handle(Command::StoreRead(result));

    let last = seen.lock().unwrap().last().unwrap().clone();
    assert!(
        last.results.iter().any(|result| result.kind == "history"
            && matches!(&result.action, SearchAction::OpenUrl { url }
                if url == "https://www.notion.so/workspace")),
        "a visited address must be offered back: {:?}",
        last.results
            .iter()
            .map(|result| (&result.kind, &result.title))
            .collect::<Vec<_>>()
    );
}

/// Recorded searches and visited pages share the History section. Ranked
/// together under one cap, a browser that has been used for a while fills
/// every slot with its own past queries and stops offering pages entirely —
/// which reads as history search being broken.
#[test]
fn recorded_searches_cannot_crowd_visited_pages_out_of_the_list() {
    let (seen, emit) = search_sink();
    let store = Arc::new(FakeStore {
        history: (0..3)
            .map(|n| zephium_core::ports::store::HistoryHit {
                url: format!("https://duckduckgo.com/?q=note+taking+{n}"),
                title: format!("note taking {n}"),
                last_visit: 10 - n,
            })
            .chain((0..3).map(|n| zephium_core::ports::store::HistoryHit {
                url: format!("https://notes{n}.example.com/"),
                title: format!("Notes {n}"),
                last_visit: 5 - n,
            }))
            .collect(),
        ..Default::default()
    });
    let mut shell = Shell::new(
        Arc::new(FakeEngine::default()),
        store,
        Arc::new(FakeChrome),
        emit,
    );
    shell.handle(Command::SetWindowSize(Size::new(1200.0, 800.0)));
    shell.handle(Command::Bootstrap);
    shell.handle(Command::Search("note".into()));

    let last = seen.lock().unwrap().last().unwrap().clone();
    let visited = last
        .results
        .iter()
        .filter(|result| result.kind == "history")
        .count();
    assert!(
        visited >= 2,
        "a page the user actually visited must still be offered: {:?}",
        last.results
            .iter()
            .map(|result| (&result.kind, &result.title))
            .collect::<Vec<_>>()
    );
}
