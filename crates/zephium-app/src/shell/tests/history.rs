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
pub(super) fn drain_reads(
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
pub(super) fn visit(shell: &mut Shell, screen: &Screen, host: &str) -> ItemId {
    shell.handle(Command::Open);
    let id = active_id(screen);
    navigate_and_commit(shell, id, host);
    id
}

pub(super) fn browsed(
    store: Arc<FakeStore>,
) -> (Shell, Screen, crate::store_reads::StoreReadQueue) {
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

    for call in [
        page("", 50),
        HistoryCall::Forget {
            urls: vec!["https://private.example/".into()],
        },
        HistoryCall::Clear {
            range: HistoryRange::Everything,
        },
    ] {
        let answer = dispatch(&mut shell, ProfileId::from(9_999), call);
        assert!(matches!(
            taken(&answer),
            HistoryResponse::Error {
                error: HistoryError::Invalid
            }
        ));
    }
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

fn current_space(shell: &Shell) -> SpaceId {
    shell.windows.focused().unwrap().space
}

fn new_space(shell: &mut Shell, name: &str) -> SpaceId {
    shell.handle(Command::Space(Box::new(crate::SpaceCommand::Create {
        name: name.into(),
        look: Default::default(),
    })));
    current_space(shell)
}

fn today(shell: &Shell, space: SpaceId) -> Vec<ItemId> {
    shell
        .items
        .roots(Placement::Space {
            space,
            section: SpaceSection::Today,
        })
        .to_vec()
}

fn url_of(shell: &Shell, id: ItemId) -> String {
    shell
        .items
        .tab(id)
        .unwrap()
        .url
        .as_ref()
        .unwrap()
        .to_string()
}

#[test]
fn reopening_returns_a_tab_to_the_space_it_was_closed_in() {
    let store = Arc::new(FakeStore::default());
    let (mut shell, screen, _queue) = browsed(store);
    let home = current_space(&shell);
    let blank = active_id(&screen);
    let kept = visit(&mut shell, &screen, "kept.example");
    let closed = visit(&mut shell, &screen, "closed.example");
    shell.handle(Command::Close(closed));
    let studio = new_space(&mut shell, "Studio");
    visit(&mut shell, &screen, "studio.example");
    assert_ne!(studio, home);

    shell.handle(Command::Run("tab.reopen".into()));

    assert_eq!(current_space(&shell), home);
    let restored = active_id(&screen);
    assert_eq!(url_of(&shell, restored), "https://closed.example/");
    assert_eq!(today(&shell, home), vec![blank, kept, restored]);
    assert!(shell.recently_closed.is_empty());
}

#[test]
fn reopening_into_an_empty_space_leaves_no_stray_new_tab() {
    let store = Arc::new(FakeStore::default());
    let (mut shell, screen, _queue) = browsed(store);
    let home = current_space(&shell);
    let only = active_id(&screen);
    navigate_and_commit(&mut shell, only, "only.example");
    shell.handle(Command::Close(only));
    let stand_in = active_id(&screen);
    assert!(shell.items.tab(stand_in).unwrap().url.is_none());
    // Leaving an untouched new tab behind closes it: home is empty now.
    let studio = new_space(&mut shell, "Studio");
    assert_eq!(current_space(&shell), studio);
    assert!(today(&shell, home).is_empty());

    shell.handle(Command::Run("tab.reopen".into()));

    assert_eq!(current_space(&shell), home);
    let restored = active_id(&screen);
    assert_eq!(today(&shell, home), vec![restored]);
    assert_eq!(url_of(&shell, restored), "https://only.example/");
    assert!(
        today(&shell, studio).is_empty(),
        "the untouched new tab left behind is not kept"
    );
}

#[test]
fn a_tab_comes_back_to_its_old_place_among_the_open_tabs() {
    let store = Arc::new(FakeStore::default());
    let (mut shell, screen, _queue) = browsed(store);
    let home = current_space(&shell);
    let first = visit(&mut shell, &screen, "first.example");
    let middle = visit(&mut shell, &screen, "middle.example");
    let last = visit(&mut shell, &screen, "last.example");
    shell.handle(Command::Close(middle));

    shell.handle(Command::Run("tab.reopen".into()));

    let restored = active_id(&screen);
    let open = today(&shell, home);
    let at = |id| open.iter().position(|tab| *tab == id).unwrap();
    assert_eq!(url_of(&shell, restored), "https://middle.example/");
    assert!(at(first) < at(restored) && at(restored) < at(last));
}

#[test]
fn a_place_that_is_gone_puts_the_tab_at_the_end() {
    let store = Arc::new(FakeStore::default());
    let (mut shell, screen, _queue) = browsed(store);
    let home = current_space(&shell);
    visit(&mut shell, &screen, "first.example");
    let second = visit(&mut shell, &screen, "second.example");
    let third = visit(&mut shell, &screen, "third.example");
    shell.handle(Command::Close(third));
    shell.handle(Command::Close(second));
    let at_end = today(&shell, home).len();
    shell
        .recently_closed
        .iter_mut()
        .for_each(|entry| entry.position = Some(40));

    shell.handle(Command::Run("tab.reopen".into()));
    shell.handle(Command::Run("tab.reopen".into()));

    let open = today(&shell, home);
    assert_eq!(open.len(), at_end + 2);
    assert_eq!(
        url_of(&shell, open[open.len() - 1]),
        "https://third.example/"
    );
}

#[test]
fn a_space_that_is_gone_reopens_in_the_current_one() {
    let store = Arc::new(FakeStore::default());
    let (mut shell, screen, _queue) = browsed(store);
    let home = current_space(&shell);
    visit(&mut shell, &screen, "first.example");
    let closed = visit(&mut shell, &screen, "closed.example");
    shell.handle(Command::Close(closed));
    shell.recently_closed[0].space = SpaceId::from(9_999);

    shell.handle(Command::Run("tab.reopen".into()));

    assert_eq!(current_space(&shell), home);
    assert_eq!(
        url_of(&shell, active_id(&screen)),
        "https://closed.example/"
    );
}

#[test]
fn a_closed_tab_can_be_reopened_by_name_out_of_order() {
    let store = Arc::new(FakeStore::default());
    let (mut shell, screen, _queue) = browsed(store);
    visit(&mut shell, &screen, "keep.example");
    let older = visit(&mut shell, &screen, "older.example");
    let newer = visit(&mut shell, &screen, "newer.example");
    shell.handle(Command::Close(older));
    shell.handle(Command::Close(newer));
    let name = shell.recently_closed[0].session_id.unwrap();

    let disposition = shell.operation_run_command(&format!("tab.reopen={name}"));

    assert_ne!(disposition.outcome, OperationOutcome::NoOp);
    assert_eq!(url_of(&shell, active_id(&screen)), "https://older.example/");
    assert_eq!(shell.recently_closed.len(), 1);
    assert_eq!(
        shell.operation_run_command("tab.reopen=not-an-id").outcome,
        OperationOutcome::Rejected
    );
}

#[test]
fn closing_the_only_tab_and_reopening_leaves_just_that_tab() {
    let store = Arc::new(FakeStore::default());
    let (mut shell, screen, _queue) = browsed(store);
    let home = current_space(&shell);
    let only = active_id(&screen);
    navigate_and_commit(&mut shell, only, "only.example");
    shell.handle(Command::Close(only));
    assert_eq!(today(&shell, home).len(), 1);

    shell.handle(Command::Run("tab.reopen".into()));

    let open = today(&shell, home);
    assert_eq!(open.len(), 1);
    assert_eq!(url_of(&shell, open[0]), "https://only.example/");
    assert_eq!(active_id(&screen), open[0]);
}

#[test]
fn tabs_closed_in_the_private_window_are_never_recorded_or_listed() {
    let store = Arc::new(FakeStore::default());
    let (mut shell, screen, _queue) = browsed(store);
    visit(&mut shell, &screen, "regular.example");
    shell.handle(Command::Run("window.newPrivate".into()));
    let private_profile = shell.windows.focused().unwrap().profile;
    let first = active_id(&screen);
    navigate_and_commit(&mut shell, first, "secret.example");
    shell.handle(Command::Open);
    let second = active_id(&screen);
    navigate_and_commit(&mut shell, second, "hidden.example");

    shell.handle(Command::Close(second));

    assert!(shell.recently_closed.is_empty());
    let answer = dispatch(&mut shell, private_profile, HistoryCall::ClosedTabs);
    let HistoryResponse::ClosedTabs { tabs } = taken(&answer) else {
        panic!("closed tabs answer with their list")
    };
    assert!(tabs.is_empty());
    assert_eq!(
        shell.operation_run_command("tab.reopen").outcome,
        OperationOutcome::NoOp
    );
}

#[test]
fn the_closed_tabs_list_is_newest_first_per_profile_and_reopens_by_id() {
    let store = Arc::new(FakeStore::default());
    let (mut shell, screen, _queue) = browsed(store);
    let profile = shell.windows.focused().unwrap().profile;
    visit(&mut shell, &screen, "keep.example");
    let older = visit(&mut shell, &screen, "older.example");
    let newer = visit(&mut shell, &screen, "newer.example");
    shell.handle(Command::Close(older));
    shell.handle(Command::Close(newer));
    let mut foreign = shell.recently_closed[0].clone();
    foreign.profile = ProfileId::from(9_001);
    foreign.session_id = Some(zephium_core::ids::ClosedSessionId::generate());
    shell.recently_closed.push(foreign);

    let answer = dispatch(&mut shell, profile, HistoryCall::ClosedTabs);
    let HistoryResponse::ClosedTabs { tabs } = taken(&answer) else {
        panic!("closed tabs answer with their list")
    };
    let urls: Vec<_> = tabs.iter().map(|tab| tab.url.as_str()).collect();
    assert_eq!(
        urls,
        vec!["https://newer.example/", "https://older.example/"]
    );

    let reopened = dispatch(
        &mut shell,
        profile,
        HistoryCall::ReopenClosed {
            id: tabs[1].id.clone(),
        },
    );
    assert_eq!(taken(&reopened), HistoryResponse::Reopened);
    assert_eq!(url_of(&shell, active_id(&screen)), "https://older.example/");

    let stale = dispatch(
        &mut shell,
        profile,
        HistoryCall::ReopenClosed {
            id: tabs[1].id.clone(),
        },
    );
    assert!(matches!(taken(&stale), HistoryResponse::Error { .. }));
    let foreign_id = shell
        .recently_closed
        .last()
        .unwrap()
        .session_id
        .unwrap()
        .to_string();
    let foreign_reopen = dispatch(
        &mut shell,
        profile,
        HistoryCall::ReopenClosed { id: foreign_id },
    );
    assert!(matches!(
        taken(&foreign_reopen),
        HistoryResponse::Error { .. }
    ));
    assert!(!HistoryCall::ReopenClosed { id: "x".into() }.validate());
}

#[test]
fn closed_tab_list_answers_even_when_storage_is_degraded() {
    let store = Arc::new(FakeStore::default());
    let (mut shell, screen, _queue) = browsed(store);
    let profile = shell.windows.focused().unwrap().profile;
    visit(&mut shell, &screen, "keep.example");
    let closed = visit(&mut shell, &screen, "closed.example");
    shell.handle(Command::Close(closed));
    shell.degraded_storage_profiles.insert(profile);

    let answer = dispatch(&mut shell, profile, HistoryCall::ClosedTabs);

    assert!(matches!(
        taken(&answer),
        HistoryResponse::ClosedTabs { tabs } if tabs.len() == 1
    ));
}

#[test]
fn the_menus_hear_of_closed_tabs_only_when_the_list_changes() {
    let log: Arc<Mutex<Vec<Vec<zephium_ipc::ClosedTabView>>>> = Arc::default();
    let sink = log.clone();
    let screen: Screen = Arc::new(Mutex::new(empty_items()));
    let items = screen.clone();
    let mut shell = Shell::new_with_failure(
        Arc::new(FakeEngine::default()),
        Arc::new(FakeStore::default()),
        Arc::new(ImmediateAllowAllCompiler),
        Box::new(|_| {}),
        Arc::new(FakeChrome),
        Box::new(move |projection| {
            if let Projection::RecentlyClosed(tabs) = &projection {
                sink.lock().unwrap().push(tabs.clone());
            }
            apply_projection(&mut items.lock().unwrap(), projection);
        }),
    );
    shell.handle(Command::SetWindowSize(Size::new(1200.0, 800.0)));
    shell.handle(Command::Bootstrap);
    visit(&mut shell, &screen, "keep.example");
    assert!(log.lock().unwrap().is_empty());

    let first = visit(&mut shell, &screen, "first.example");
    shell.handle(Command::Close(first));
    let second = visit(&mut shell, &screen, "second.example");
    shell.handle(Command::Close(second));
    shell.handle(Command::Open);
    shell.handle(Command::Open);

    let sent = log.lock().unwrap().clone();
    assert_eq!(sent.len(), 2, "unrelated changes say nothing");
    let titles: Vec<_> = sent[1].iter().map(|tab| tab.url.as_str()).collect();
    assert_eq!(
        titles,
        vec!["https://second.example/", "https://first.example/"]
    );
    assert!(sent[1].iter().all(|tab| tab.icon.is_none()));

    shell.handle(Command::Run("tab.reopen".into()));
    let sent = log.lock().unwrap().clone();
    assert_eq!(sent.len(), 3);
    assert_eq!(sent[2].len(), 1);
}

fn empty_items() -> ItemsState {
    ItemsState {
        projection_revision: String::new(),
        profile: None,
        spaces: Vec::new(),
        active_space_id: None,
        nodes: Vec::new(),
        tabs: Vec::new(),
        active: None,
        split_group: None,
    }
}
