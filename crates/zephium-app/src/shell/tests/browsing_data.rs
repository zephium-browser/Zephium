use super::history::{drain_reads, visit};
use super::*;

use zephium_core::browsing_data::{
    EngineDataCall, EngineDataFailure, EngineDataKinds, EngineDataReply, EngineSite,
};
use zephium_core::downloads::DownloadCall;
use zephium_ipc::{
    BrowsingDataCall, BrowsingDataError, BrowsingDataRange as Range, BrowsingDataResponse,
};

type Answer = Arc<Mutex<Option<BrowsingDataResponse>>>;

fn dispatch(shell: &mut Shell, profile: ProfileId, call: BrowsingDataCall) -> Answer {
    let slot: Answer = Arc::new(Mutex::new(None));
    let sink = slot.clone();
    shell.handle(Command::BrowsingDataCall {
        expected_profile: profile,
        call: Box::new(call),
        done: crate::api::BrowsingDataCompletion::new(move |response| {
            *sink.lock().unwrap() = Some(response);
        }),
    });
    slot
}

fn answered(answer: &Answer) -> BrowsingDataResponse {
    answer
        .lock()
        .unwrap()
        .take()
        .expect("every browsing data call is answered")
}

fn clear(range: Range, [history, cookies, cache, downloads]: [bool; 4]) -> BrowsingDataCall {
    BrowsingDataCall::Clear {
        range,
        history,
        cookies,
        cache,
        downloads,
    }
}

fn host(name: &str, cookies: bool, storage: bool) -> EngineSite {
    EngineSite {
        name: name.into(),
        cookies,
        storage,
    }
}

fn set_reply(engine: &FakeEngine, reply: EngineDataReply) {
    *engine.data_reply.lock().unwrap() = Some(reply);
}

fn browsed(
    store: Arc<FakeStore>,
) -> (
    Shell,
    Arc<FakeEngine>,
    Screen,
    crate::store_reads::StoreReadQueue,
) {
    let (mut shell, engine, screen) = setup_with(store);
    shell.handle(Command::Bootstrap);
    let queue = crate::store_reads::StoreReadQueue::new();
    shell.store_reads = Some(queue.clone());
    (shell, engine, screen, queue)
}

fn focused_profile(shell: &Shell) -> ProfileId {
    shell.windows.focused().unwrap().profile
}

#[test]
fn cookies_and_cache_reach_the_engine_for_the_focused_profile_and_range() {
    let store = Arc::new(FakeStore::default());
    let (mut shell, engine, _screen, _queue) = browsed(store);
    let profile = focused_profile(&shell);
    let before = chrono::Utc::now().timestamp();

    let answer = dispatch(
        &mut shell,
        profile,
        clear(Range::FourWeeks, [false, true, true, false]),
    );

    assert_eq!(answered(&answer), BrowsingDataResponse::Cleared);
    let after = chrono::Utc::now().timestamp();
    let fake = &engine;
    let calls = fake.data_calls.lock().unwrap().clone();
    assert_eq!(calls.len(), 1);
    let (partition, EngineDataCall::Clear { kinds, since }) = calls[0].clone() else {
        panic!("a clear asks the engine to clear");
    };
    assert_eq!(partition.profile(), profile);
    assert!(!matches!(partition, Partition::Ephemeral(_)));
    assert_eq!(
        kinds,
        EngineDataKinds {
            history: false,
            site_data: true,
            cache: true
        }
    );
    let window = 28 * 24 * 3600;
    assert!(since.is_some_and(|since| (before - window..=after - window).contains(&since)));
    assert!(fake.download_calls.lock().unwrap().is_empty());
}

#[test]
fn all_time_clears_without_a_date() {
    let store = Arc::new(FakeStore::default());
    let (mut shell, engine, _screen, _queue) = browsed(store);
    let profile = focused_profile(&shell);

    let answer = dispatch(
        &mut shell,
        profile,
        clear(Range::Everything, [false, true, false, true]),
    );

    assert_eq!(answered(&answer), BrowsingDataResponse::Cleared);
    let fake = &engine;
    assert!(matches!(
        fake.data_calls.lock().unwrap()[0].1,
        EngineDataCall::Clear { since: None, .. }
    ));
    assert!(matches!(
        fake.download_calls.lock().unwrap()[0].1,
        DownloadCall::Clear
    ));
}

#[test]
fn a_ranged_clear_dates_the_download_list_too() {
    let store = Arc::new(FakeStore::default());
    let (mut shell, engine, _screen, _queue) = browsed(store);
    let profile = focused_profile(&shell);

    let answer = dispatch(
        &mut shell,
        profile,
        clear(Range::Day, [false, false, false, true]),
    );

    assert_eq!(answered(&answer), BrowsingDataResponse::Cleared);
    let fake = &engine;
    assert!(fake.data_calls.lock().unwrap().is_empty());
    let calls = fake.download_calls.lock().unwrap().clone();
    let (partition, DownloadCall::ClearSince { since }) = calls[0].clone() else {
        panic!("a ranged clear forgets only the downloads inside it");
    };
    assert_eq!(partition.profile(), profile);
    assert!(since > chrono::Utc::now().timestamp() - 24 * 3600 - 5);
}

#[test]
fn history_cookies_and_downloads_answer_once_all_have() {
    let store = Arc::new(FakeStore::default());
    let (mut shell, _engine, screen, queue) = browsed(store.clone());
    visit(&mut shell, &screen, "visited.example");
    let profile = focused_profile(&shell);

    let answer = dispatch(
        &mut shell,
        profile,
        clear(Range::Everything, [true, true, true, true]),
    );
    assert!(
        answer.lock().unwrap().is_none(),
        "the store has not answered, so the clear is not done"
    );
    drain_reads(&mut shell, store, queue);

    assert_eq!(answered(&answer), BrowsingDataResponse::Cleared);
}

#[test]
fn a_part_that_fails_fails_the_clear() {
    let store = Arc::new(FakeStore::default());
    let (mut shell, engine, _screen, _queue) = browsed(store);
    let profile = focused_profile(&shell);
    set_reply(&engine, EngineDataReply::Failed(EngineDataFailure::Failed));

    let answer = dispatch(
        &mut shell,
        profile,
        clear(Range::Hour, [false, true, false, true]),
    );

    assert_eq!(
        answered(&answer),
        BrowsingDataResponse::Error {
            error: BrowsingDataError::Failed
        }
    );
}

#[test]
fn a_clear_of_nothing_is_refused() {
    let store = Arc::new(FakeStore::default());
    let (mut shell, engine, _screen, _queue) = browsed(store);
    let profile = focused_profile(&shell);

    let answer = dispatch(
        &mut shell,
        profile,
        clear(Range::Hour, [false, false, false, false]),
    );

    assert_eq!(
        answered(&answer),
        BrowsingDataResponse::Error {
            error: BrowsingDataError::Invalid
        }
    );
    assert!(engine.data_calls.lock().unwrap().is_empty());
}

#[test]
fn another_profile_is_never_addressed() {
    let store = Arc::new(FakeStore::default());
    let (mut shell, engine, _screen, _queue) = browsed(store.clone());
    let other = ProfileId::from(9_001);
    assert_ne!(other, focused_profile(&shell));

    for call in [
        clear(Range::Everything, [true, true, true, true]),
        BrowsingDataCall::Sites,
        BrowsingDataCall::ForgetSite {
            site: "example.com".into(),
        },
        BrowsingDataCall::ForgetPage {
            origin: "https://example.com".into(),
        },
    ] {
        let answer = dispatch(&mut shell, other, call);
        assert_eq!(
            answered(&answer),
            BrowsingDataResponse::Error {
                error: BrowsingDataError::Invalid
            }
        );
    }
    let fake = &engine;
    assert!(fake.data_calls.lock().unwrap().is_empty());
    assert!(fake.download_calls.lock().unwrap().is_empty());
    assert!(store.backup_clears.lock().unwrap().is_empty());
}

#[test]
fn a_private_window_has_nothing_to_clear_and_reaches_no_store() {
    let store = Arc::new(FakeStore::default());
    let (mut shell, engine, _screen, _queue) = browsed(store.clone());
    shell.handle(Command::Run("window.newPrivate".into()));
    let private = focused_profile(&shell);
    assert!(shell.incognito_profile(private));

    for call in [
        clear(Range::Everything, [true, true, true, true]),
        BrowsingDataCall::Sites,
        BrowsingDataCall::ForgetSite {
            site: "example.com".into(),
        },
        BrowsingDataCall::ForgetPage {
            origin: "https://example.com".into(),
        },
    ] {
        let answer = dispatch(&mut shell, private, call);
        assert_eq!(
            answered(&answer),
            BrowsingDataResponse::Error {
                error: BrowsingDataError::Private
            }
        );
    }
    let fake = &engine;
    assert!(fake.data_calls.lock().unwrap().is_empty());
    assert!(fake.download_calls.lock().unwrap().is_empty());
    assert!(store.backup_clears.lock().unwrap().is_empty());
}

#[test]
fn the_site_list_groups_hosts_under_their_registrable_domain() {
    let store = Arc::new(FakeStore::default());
    let (mut shell, engine, _screen, _queue) = browsed(store);
    let profile = focused_profile(&shell);
    set_reply(
        &engine,
        EngineDataReply::Sites {
            sites: vec![
                host("www.example.co.uk", true, false),
                host("example.co.uk", false, true),
                host("accounts.example.org", true, true),
                host("localhost", false, true),
                host("127.0.0.1", true, false),
            ],
            partial: true,
        },
    );

    let answer = dispatch(&mut shell, profile, BrowsingDataCall::Sites);

    let BrowsingDataResponse::Sites { sites, partial } = answered(&answer) else {
        panic!("the list answers with sites");
    };
    assert!(partial);
    let names: Vec<_> = sites.iter().map(|site| site.site.as_str()).collect();
    assert_eq!(
        names,
        ["127.0.0.1", "example.co.uk", "example.org", "localhost"]
    );
    let merged = sites
        .iter()
        .find(|site| site.site == "example.co.uk")
        .unwrap();
    assert!(merged.cookies && merged.storage);
}

#[test]
fn forgetting_a_listed_site_asks_only_for_that_site() {
    let store = Arc::new(FakeStore::default());
    let (mut shell, engine, _screen, _queue) = browsed(store);
    let profile = focused_profile(&shell);

    let answer = dispatch(
        &mut shell,
        profile,
        BrowsingDataCall::ForgetSite {
            site: "example.com".into(),
        },
    );

    assert_eq!(answered(&answer), BrowsingDataResponse::Cleared);
    let calls = engine.data_calls.lock().unwrap().clone();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].0.profile(), profile);
    assert_eq!(
        calls[0].1,
        EngineDataCall::ForgetSite {
            site: "example.com".into(),
            origin: None,
        }
    );
}

#[test]
fn forgetting_a_page_reaches_its_whole_site_and_keeps_the_origin() {
    let store = Arc::new(FakeStore::default());
    let (mut shell, engine, _screen, _queue) = browsed(store);
    let profile = focused_profile(&shell);

    for (origin, site) in [
        ("https://news.example.co.uk", "example.co.uk"),
        ("http://localhost:3000", "localhost"),
        ("https://127.0.0.1:8443", "127.0.0.1"),
    ] {
        let answer = dispatch(
            &mut shell,
            profile,
            BrowsingDataCall::ForgetPage {
                origin: origin.into(),
            },
        );
        assert_eq!(answered(&answer), BrowsingDataResponse::Cleared, "{origin}");
        let (_, call) = engine.data_calls.lock().unwrap().last().cloned().unwrap();
        assert_eq!(
            call,
            EngineDataCall::ForgetSite {
                site: site.into(),
                origin: Some(origin.into()),
            }
        );
    }
}

#[test]
fn a_site_or_page_that_is_not_one_host_is_refused_before_the_engine() {
    let store = Arc::new(FakeStore::default());
    let (mut shell, engine, _screen, _queue) = browsed(store);
    let profile = focused_profile(&shell);

    let refused = [
        BrowsingDataCall::ForgetSite {
            site: "https://example.com".into(),
        },
        BrowsingDataCall::ForgetSite {
            site: "example.com/path".into(),
        },
        BrowsingDataCall::ForgetSite {
            site: "*.example.com".into(),
        },
        BrowsingDataCall::ForgetSite { site: "".into() },
        BrowsingDataCall::ForgetPage {
            origin: "https://example.com/path".into(),
        },
        BrowsingDataCall::ForgetPage {
            origin: "javascript:alert(1)".into(),
        },
        BrowsingDataCall::ForgetPage {
            origin: "about:blank".into(),
        },
    ];
    for call in refused {
        let answer = dispatch(&mut shell, profile, call.clone());
        assert_eq!(
            answered(&answer),
            BrowsingDataResponse::Error {
                error: BrowsingDataError::Invalid
            },
            "{call:?}"
        );
    }
    assert!(engine.data_calls.lock().unwrap().is_empty());
}

#[test]
fn a_four_week_history_clear_reaches_the_store_with_its_date() {
    let store = Arc::new(FakeStore::default());
    let (mut shell, _engine, screen, queue) = browsed(store.clone());
    visit(&mut shell, &screen, "visited.example");
    let profile = focused_profile(&shell);
    let before = chrono::Utc::now().timestamp();

    let answer = dispatch(
        &mut shell,
        profile,
        clear(Range::FourWeeks, [true, false, false, false]),
    );
    drain_reads(&mut shell, store.clone(), queue);

    assert_eq!(answered(&answer), BrowsingDataResponse::Cleared);
    let after = chrono::Utc::now().timestamp();
    let window = 28 * 24 * 3600;
    let clears = store.chat_clears.lock().unwrap().clone();
    let [(cleared, Some(since))] = clears[..] else {
        panic!("one dated clear reaches the store: {clears:?}");
    };
    assert_eq!(cleared, profile);
    assert!((before - window - 1..=after - window + 1).contains(&since));
}

#[test]
fn clearing_history_takes_the_closed_tabs_of_the_range_out_of_recently_closed() {
    let store = Arc::new(FakeStore::default());
    let (mut shell, _engine, screen, queue) = browsed(store.clone());
    let profile = focused_profile(&shell);
    visit(&mut shell, &screen, "keep.example");
    let old = visit(&mut shell, &screen, "old.example");
    let recent = visit(&mut shell, &screen, "recent.example");
    shell.handle(Command::Close(old));
    shell.handle(Command::Close(recent));
    let now_ms = u64::try_from(chrono::Utc::now().timestamp_millis()).unwrap();
    for entry in &mut shell.recently_closed {
        entry.closed_at_ms = Some(if entry.url.contains("old.example") {
            now_ms - 3 * 3600 * 1000
        } else {
            now_ms
        });
    }
    let mut foreign = shell.recently_closed[0].clone();
    foreign.profile = ProfileId::from(9_001);
    shell.recently_closed.push(foreign);

    let answer = dispatch(
        &mut shell,
        profile,
        clear(Range::Hour, [true, false, false, false]),
    );
    drain_reads(&mut shell, store.clone(), queue.clone());
    assert_eq!(answered(&answer), BrowsingDataResponse::Cleared);

    let urls: Vec<_> = shell
        .recently_closed
        .iter()
        .map(|entry| (entry.profile == profile, entry.url.clone()))
        .collect();
    assert_eq!(
        urls,
        [
            (true, "https://old.example/".to_owned()),
            (false, "https://old.example/".to_owned())
        ]
    );

    let queue = crate::store_reads::StoreReadQueue::new();
    shell.store_reads = Some(queue.clone());
    let answer = dispatch(
        &mut shell,
        profile,
        clear(Range::Everything, [true, false, false, false]),
    );
    drain_reads(&mut shell, store, queue);
    assert_eq!(answered(&answer), BrowsingDataResponse::Cleared);
    assert!(shell
        .recently_closed
        .iter()
        .all(|entry| entry.profile != profile));
    assert_eq!(shell.recently_closed.len(), 1, "another profile's stays");
}

#[test]
fn site_data_erasure_disposes_the_focused_profiles_session_backup() {
    let store = Arc::new(FakeStore::default());
    let (mut shell, _engine, _screen, _queue) = browsed(store.clone());
    let profile = focused_profile(&shell);
    for call in [
        clear(Range::Hour, [false, true, false, false]),
        BrowsingDataCall::ForgetSite {
            site: "example.test".into(),
        },
        BrowsingDataCall::ForgetPage {
            origin: "https://example.test".into(),
        },
    ] {
        assert_eq!(
            answered(&dispatch(&mut shell, profile, call)),
            BrowsingDataResponse::Cleared
        );
    }
    assert_eq!(*store.backup_clears.lock().unwrap(), vec![profile; 3]);
    for flags in [[false, false, true, false], [false, false, false, true]] {
        assert_eq!(
            answered(&dispatch(
                &mut shell,
                profile,
                clear(Range::Everything, flags)
            )),
            BrowsingDataResponse::Cleared
        );
    }
    assert_eq!(*store.backup_clears.lock().unwrap(), vec![profile; 3]);
}

#[test]
fn site_data_clear_reports_backup_write_failure_and_queue_refusal() {
    let store = Arc::new(FakeStore::default());
    let (mut shell, _engine, _screen, _queue) = browsed(store.clone());
    let profile = focused_profile(&shell);
    for refused in [false, true] {
        *store.backup_clear_result.lock().unwrap() = Some(false);
        store
            .reject_backup_clears
            .store(refused, std::sync::atomic::Ordering::Relaxed);
        for call in [
            clear(Range::Everything, [false, true, false, false]),
            BrowsingDataCall::ForgetSite {
                site: "example.test".into(),
            },
            BrowsingDataCall::ForgetPage {
                origin: "https://example.test".into(),
            },
        ] {
            assert_eq!(
                answered(&dispatch(&mut shell, profile, call)),
                BrowsingDataResponse::Error {
                    error: BrowsingDataError::Failed
                }
            );
        }
    }
}

#[test]
fn site_data_clear_waits_for_backup_disposal_and_preserves_engine_failures() {
    let store = Arc::new(FakeStore::default());
    *store.held_backup_clears.lock().unwrap() = Some(Vec::new());
    let (mut shell, engine, _screen, _queue) = browsed(store.clone());
    let profile = focused_profile(&shell);
    for call in [
        clear(Range::Everything, [false, true, false, false]),
        BrowsingDataCall::ForgetPage {
            origin: "https://example.test".into(),
        },
    ] {
        let answer = dispatch(&mut shell, profile, call);
        assert!(answer.lock().unwrap().is_none());
        let done = store
            .held_backup_clears
            .lock()
            .unwrap()
            .as_mut()
            .unwrap()
            .pop()
            .unwrap();
        done(true);
        assert_eq!(answered(&answer), BrowsingDataResponse::Cleared);
    }
    *store.held_backup_clears.lock().unwrap() = None;
    set_reply(
        &engine,
        EngineDataReply::Failed(EngineDataFailure::Unavailable),
    );
    let answer = dispatch(
        &mut shell,
        profile,
        BrowsingDataCall::ForgetSite {
            site: "example.test".into(),
        },
    );
    assert_eq!(
        answered(&answer),
        BrowsingDataResponse::Error {
            error: BrowsingDataError::Unavailable
        }
    );
}
