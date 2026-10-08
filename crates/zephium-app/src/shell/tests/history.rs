use super::*;

use zephium_ipc::{HistoryCall, HistoryError, HistoryRange, HistoryResponse};

type Answer = Arc<Mutex<Option<HistoryResponse>>>;

fn dispatch(shell: &mut Shell, profile: ProfileId, call: HistoryCall) -> Answer {
    let slot: Answer = Arc::new(Mutex::new(None));
    let sink = slot.clone();
    shell.handle(Command::HistoryCall {
        expected_profile: profile,
        call: Box::new(call),
        done: crate::api::HistoryCompletion::new(move |response| {
            *sink.lock().unwrap() = Some(response);
        }),
    });
    slot
}

fn taken(answer: &Answer) -> HistoryResponse {
    answer
        .lock()
        .unwrap()
        .take()
        .expect("every history call is answered")
}

fn page(query: &str, limit: u16) -> HistoryCall {
    scoped(query, limit, HistoryRange::Everything)
}

fn scoped(query: &str, limit: u16, range: HistoryRange) -> HistoryCall {
    HistoryCall::Page {
        query: query.into(),
        range,
        before: None,
        limit,
    }
}

/// Drives the reader thread the way the actor does and hands its reply back.
fn drain_reads(
    shell: &mut Shell,
    store: Arc<FakeStore>,
    queue: crate::store_reads::StoreReadQueue,
) {
    let reader_store: SharedStore = store;
    let reader_queue = queue.clone();
    let (tx, rx) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        crate::store_reads::run_for_test(reader_store, reader_queue, tx);
    });
    let result = rx
        .recv_timeout(std::time::Duration::from_secs(5))
        .expect("the reader must answer a requested history call");
    queue.stop();
    worker.join().unwrap();
    shell.handle(Command::StoreRead(result));
}

/// Visits are deduplicated per tab within a second, so each address gets its
/// own tab rather than racing that rule.
fn visit(shell: &mut Shell, screen: &Screen, host: &str) -> ItemId {
    shell.handle(Command::Open);
    let id = active_id(screen);
    navigate_and_commit(shell, id, host);
    id
}

fn browsed(store: Arc<FakeStore>) -> (Shell, Screen, crate::store_reads::StoreReadQueue) {
    let (mut shell, _engine, screen) = setup_with(store);
    shell.handle(Command::Bootstrap);
    let queue = crate::store_reads::StoreReadQueue::new();
    shell.store_reads = Some(queue.clone());
    (shell, screen, queue)
}

#[test]
fn a_history_page_carries_icon_references_for_addresses_chrome_already_holds() {
    let store = Arc::new(FakeStore::default());
    let (mut shell, screen, queue) = browsed(store.clone());
    let id = visit(&mut shell, &screen, "visited.example");
    shell.handle(Command::Engine(EngineEvent::FaviconPixels {
        id,
        page_url: "https://visited.example/".into(),
        rgba: vec![13; zephium_core::icon::RGBA32_BYTES],
    }));
    let profile = shell.windows.focused().unwrap().profile;

    let answer = dispatch(&mut shell, profile, page("", 50));
    drain_reads(&mut shell, store, queue);

    let HistoryResponse::Page { visits, next } = taken(&answer) else {
        panic!("the reader answers with a page")
    };
    assert_eq!(visits.len(), 1);
    assert_eq!(visits[0].url, "https://visited.example/");
    assert_eq!(
        visits[0].icon.as_ref().map(|icon| icon.origin.as_str()),
        Some("https://visited.example")
    );
    assert!(next.is_none(), "a short page is the end of the list");
}

#[test]
fn a_full_page_offers_a_cursor_and_a_short_one_ends_the_list() {
    let store = Arc::new(FakeStore::default());
    let (mut shell, screen, queue) = browsed(store.clone());
    for index in 0..3 {
        visit(&mut shell, &screen, &format!("page-{index}.example"));
    }
    let profile = shell.windows.focused().unwrap().profile;

    let answer = dispatch(&mut shell, profile, page("", 2));
    drain_reads(&mut shell, store, queue);

    let HistoryResponse::Page { visits, next } = taken(&answer) else {
        panic!("the reader answers with a page")
    };
    assert_eq!(visits.len(), 2);
    assert!(next.is_some(), "a full page may have more behind it");
}

#[test]
fn history_never_answers_a_profile_other_than_the_focused_one() {
    let store = Arc::new(FakeStore::default());
    let (mut shell, screen, _queue) = browsed(store);
    visit(&mut shell, &screen, "private.example");

    let answer = dispatch(&mut shell, ProfileId::from(9_999), page("", 50));

    assert!(matches!(
        taken(&answer),
        HistoryResponse::Error {
            error: HistoryError::Invalid
        }
    ));
}

#[test]
fn forgetting_an_address_reports_what_it_removed() {
    let store = Arc::new(FakeStore::default());
    let (mut shell, screen, queue) = browsed(store.clone());
    visit(&mut shell, &screen, "forget.example");
    visit(&mut shell, &screen, "keep.example");
    let profile = shell.windows.focused().unwrap().profile;

    let answer = dispatch(
        &mut shell,
        profile,
        HistoryCall::Forget {
            urls: vec!["https://forget.example/".into()],
        },
    );
    drain_reads(&mut shell, store.clone(), queue);

    assert!(matches!(
        taken(&answer),
        HistoryResponse::Removed { count: 1 }
    ));
    assert_eq!(store.history_page(profile, "", None, None, 50).len(), 1);
}

#[test]
fn clearing_everything_empties_the_list() {
    let store = Arc::new(FakeStore::default());
    let (mut shell, screen, queue) = browsed(store.clone());
    visit(&mut shell, &screen, "one.example");
    visit(&mut shell, &screen, "two.example");
    let profile = shell.windows.focused().unwrap().profile;

    let answer = dispatch(
        &mut shell,
        profile,
        HistoryCall::Clear {
            range: HistoryRange::Everything,
        },
    );
    drain_reads(&mut shell, store.clone(), queue);

    assert!(matches!(
        taken(&answer),
        HistoryResponse::Removed { count: 2 }
    ));
    assert!(store.history_page(profile, "", None, None, 50).is_empty());
}

#[test]
fn a_title_published_after_the_url_commits_reaches_recorded_history() {
    let store = Arc::new(FakeStore::default());
    let (mut shell, screen, _queue) = browsed(store.clone());
    let id = visit(&mut shell, &screen, "article.example");
    let profile = shell.windows.focused().unwrap().profile;
    // The visit was recorded when the URL committed, before the document had a
    // title of its own.
    assert_eq!(
        store.history_page(profile, "", None, None, 1)[0].title,
        "article.example"
    );

    shell.handle(Command::Engine(EngineEvent::TitleChanged {
        id,
        title: "The Real Headline".into(),
    }));

    assert_eq!(
        store.history_page(profile, "", None, None, 1)[0].title,
        "The Real Headline"
    );
}

#[test]
fn a_shutting_down_shell_answers_history_calls_it_can_no_longer_serve() {
    let store = Arc::new(FakeStore::default());
    let (mut shell, screen, _queue) = browsed(store);
    visit(&mut shell, &screen, "pending.example");
    let profile = shell.windows.focused().unwrap().profile;

    let answer = dispatch(&mut shell, profile, page("", 50));
    assert!(
        answer.lock().unwrap().is_none(),
        "the reader has not replied"
    );
    shell.fail_pending_history_calls();

    assert!(matches!(
        taken(&answer),
        HistoryResponse::Error {
            error: HistoryError::Unavailable
        }
    ));
}

#[test]
fn opening_a_history_row_navigates_in_place_and_leaves_the_library() {
    let store = Arc::new(FakeStore::default());
    let (mut shell, screen, _queue) = browsed(store);
    let id = active_id(&screen);
    shell.handle(Command::ShowBrowserPage(Some(crate::BrowserPage::History)));
    assert!(shell.active_browser_page().is_some());

    shell.handle(Command::OpenUrl {
        input: "https://example.com/article".into(),
        new_tab: false,
    });

    // Navigation from a browser page returns from it first and replays itself,
    // so the row lands in the tab the reader was already looking at.
    assert!(shell.browser_return.is_some() || shell.active_browser_page().is_none());
    assert_eq!(active_id(&screen), id);
}

#[test]
fn opening_a_history_row_in_a_new_tab_leaves_the_current_one_alone() {
    let store = Arc::new(FakeStore::default());
    let (mut shell, screen, _queue) = browsed(store);
    let first = visit(&mut shell, &screen, "kept.example");

    shell.handle(Command::OpenUrl {
        input: "https://example.com/article".into(),
        new_tab: true,
    });

    assert_ne!(active_id(&screen), first);
    assert_eq!(
        shell
            .items
            .tab(first)
            .and_then(|tab| tab.url.as_ref())
            .map(ToString::to_string),
        Some("https://kept.example/".to_owned())
    );
}

#[test]
fn reopening_restores_the_tab_that_was_closed_last() {
    let store = Arc::new(FakeStore::default());
    let (mut shell, screen, _queue) = browsed(store);
    let first = visit(&mut shell, &screen, "first.example");
    let second = visit(&mut shell, &screen, "second.example");

    shell.handle(Command::Close(second));
    shell.handle(Command::Run("tab.reopen".into()));

    let restored = active_id(&screen);
    assert_ne!(restored, first);
    assert_ne!(
        restored, second,
        "a restored tab is a new item, not a revival"
    );
    assert_eq!(
        shell
            .items
            .tab(restored)
            .and_then(|tab| tab.url.as_ref())
            .map(ToString::to_string),
        Some("https://second.example/".to_owned())
    );
}

#[test]
fn reopening_with_nothing_closed_is_a_no_op_rather_than_a_failure() {
    let store = Arc::new(FakeStore::default());
    let (mut shell, screen, _queue) = browsed(store);
    let only = visit(&mut shell, &screen, "only.example");

    let disposition = shell.operation_run_command("tab.reopen");

    assert_eq!(disposition.outcome, OperationOutcome::NoOp);
    assert_eq!(active_id(&screen), only);
}

#[test]
fn degraded_storage_reports_itself_rather_than_looking_empty() {
    let store = Arc::new(FakeStore::default());
    let (mut shell, screen, _queue) = browsed(store);
    visit(&mut shell, &screen, "recorded.example");
    let profile = shell.windows.focused().unwrap().profile;
    shell.degraded_storage_profiles.insert(profile);

    let answer = dispatch(&mut shell, profile, page("", 50));

    assert!(
        matches!(
            taken(&answer),
            HistoryResponse::Error {
                error: HistoryError::Unavailable
            }
        ),
        "an unusable store must not present itself as an empty history"
    );
}

#[test]
fn the_range_the_reader_picks_narrows_the_list_it_is_looking_at() {
    let store = Arc::new(FakeStore::default());
    let (mut shell, screen, queue) = browsed(store.clone());
    visit(&mut shell, &screen, "recent.example");
    let profile = shell.windows.focused().unwrap().profile;

    // Every recorded visit is older than the window.
    let answer = dispatch(&mut shell, profile, scoped("", 50, HistoryRange::Hour));
    drain_reads(&mut shell, store, queue);

    let HistoryResponse::Page { visits, .. } = taken(&answer) else {
        panic!("the reader answers with a page")
    };
    assert!(
        visits.is_empty(),
        "a scoped request must reach the store's range bound: {visits:?}"
    );
}

#[test]
fn a_failed_history_clear_completes_with_an_error_once() {
    let store = Arc::new(FakeStore::default());
    let (mut shell, screen, queue) = browsed(store);
    visit(&mut shell, &screen, "kept.example");
    let profile = shell.windows.focused().unwrap().profile;
    let answer = dispatch(
        &mut shell,
        profile,
        HistoryCall::Clear {
            range: HistoryRange::Everything,
        },
    );
    assert!(
        answer.lock().unwrap().is_none(),
        "clear must reach the pending reader before failure settlement"
    );
    let token = 1; // This fresh shell owns its first history-surface request.
    shell.handle(Command::StoreRead(StoreReadResult::HistorySurfaceFailed {
        token,
        profile,
    }));
    assert!(matches!(
        taken(&answer),
        HistoryResponse::Error {
            error: HistoryError::Unavailable
        }
    ));
    shell.handle(Command::StoreRead(StoreReadResult::HistorySurfaceFailed {
        token,
        profile,
    }));
    assert!(answer.lock().unwrap().is_none());
    queue.stop();
}

#[test]
fn a_refused_time_clear_preserves_blocker_counters_and_skips_the_reader() {
    let store = Arc::new(FakeStore::default());
    let (mut shell, screen, queue) = browsed(store.clone());
    visit(&mut shell, &screen, "kept.example");
    let profile = shell.windows.focused().unwrap().profile;
    let writes = store
        .statistics_writes
        .load(std::sync::atomic::Ordering::Acquire);
    store
        .reject_time_clears
        .store(true, std::sync::atomic::Ordering::Release);
    let answer = dispatch(
        &mut shell,
        profile,
        HistoryCall::Clear {
            range: HistoryRange::Everything,
        },
    );
    assert!(matches!(
        taken(&answer),
        HistoryResponse::Error {
            error: HistoryError::Unavailable
        }
    ));
    assert_eq!(
        store
            .statistics_writes
            .load(std::sync::atomic::Ordering::Acquire),
        writes
    );
    assert_eq!(queue.pending_surface_calls_for_test(), 0);
}
