use super::*;

use zephium_core::extension_data::{
    ExtensionDataCall, ExtensionDataChange, ExtensionDataInterest, ExtensionDataReply,
    HistoryReply, HistoryRequest,
};
use zephium_core::ports::engine::WebExtensionDataRequest;

use super::history::visit;

fn browsed() -> (
    Shell,
    Arc<FakeEngine>,
    Screen,
    Arc<FakeStore>,
    crate::store_reads::StoreReadQueue,
) {
    let store = Arc::new(FakeStore::default());
    let (mut shell, engine, screen) = setup_with(store.clone());
    shell.handle(Command::Bootstrap);
    let queue = crate::store_reads::StoreReadQueue::new();
    shell.store_reads = Some(queue.clone());
    (shell, engine, screen, store, queue)
}

/// Runs the reader until it has nothing more to deliver.
fn drain(shell: &mut Shell, store: Arc<FakeStore>, queue: crate::store_reads::StoreReadQueue) {
    let reader_store: SharedStore = store;
    let reader_queue = queue.clone();
    let (tx, rx) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        crate::store_reads::run_for_test(reader_store, reader_queue, tx);
    });
    while let Ok(result) = rx.recv_timeout(std::time::Duration::from_millis(300)) {
        shell.handle(Command::StoreRead(result));
    }
    queue.stop();
    worker.join().unwrap();
}

fn ask(shell: &mut Shell, profile: ProfileId, request: u64, call: ExtensionDataCall) {
    shell.handle(Command::Engine(EngineEvent::WebExtensionDataRequested(
        Box::new(WebExtensionDataRequest {
            profile,
            request,
            call,
        }),
    )));
}

fn listen(shell: &mut Shell, profile: ProfileId, interest: ExtensionDataInterest) {
    shell.handle(Command::Engine(EngineEvent::WebExtensionDataInterest {
        profile,
        interest,
    }));
}

#[test]
fn requests_the_profile_cannot_answer_are_refused_at_once() {
    let (mut shell, engine, _, _, _) = browsed();
    let profile = shell.windows.focused().unwrap().profile;
    ask(
        &mut shell,
        ProfileId::from(9_999),
        1,
        ExtensionDataCall::History(HistoryRequest::RemoveAll),
    );
    ask(
        &mut shell,
        profile,
        2,
        ExtensionDataCall::History(HistoryRequest::RemoveRange { start: 2, end: 1 }),
    );
    assert_eq!(
        *engine.extension_data_answers.lock().unwrap(),
        vec![
            (ProfileId::from(9_999), 1, ExtensionDataReply::Unavailable),
            (profile, 2, ExtensionDataReply::Unavailable),
        ]
    );
}

#[test]
fn a_store_request_is_answered_from_the_reader() {
    let (mut shell, engine, screen, store, queue) = browsed();
    visit(&mut shell, &screen, "read.example");
    let profile = shell.windows.focused().unwrap().profile;
    ask(
        &mut shell,
        profile,
        7,
        ExtensionDataCall::History(HistoryRequest::Search {
            text: "read".into(),
            start: 0,
            end: i64::MAX,
            limit: 10,
        }),
    );
    drain(&mut shell, store, queue);
    let answers = engine.extension_data_answers.lock().unwrap();
    let [(answered, 7, ExtensionDataReply::History(HistoryReply::Entries(entries)))] =
        answers.as_slice()
    else {
        panic!("one answer with entries: {answers:?}");
    };
    assert_eq!(*answered, profile);
    assert_eq!(entries[0].url, "https://read.example/");
}

#[test]
fn visits_are_described_only_while_an_extension_listens() {
    let (mut shell, engine, screen, store, queue) = browsed();
    let profile = shell.windows.focused().unwrap().profile;
    visit(&mut shell, &screen, "unheard.example");
    listen(
        &mut shell,
        profile,
        ExtensionDataInterest {
            history: true,
            ..Default::default()
        },
    );
    visit(&mut shell, &screen, "heard.example");
    drain(&mut shell, store, queue);
    let changes = engine.extension_data_changes.lock().unwrap();
    let urls: Vec<&str> = changes
        .iter()
        .map(|(_, change)| match change {
            ExtensionDataChange::Visited(entry) => entry.url.as_str(),
            other => panic!("only visits: {other:?}"),
        })
        .collect();
    assert_eq!(urls, ["https://heard.example/"]);
}

#[test]
fn closed_tabs_list_newest_first_and_reopen_in_the_focused_window() {
    let (mut shell, engine, screen, _, _) = browsed();
    let profile = shell.windows.focused().unwrap().profile;
    listen(
        &mut shell,
        profile,
        ExtensionDataInterest {
            sessions: true,
            ..Default::default()
        },
    );
    for host in ["first.example", "second.example"] {
        let tab = visit(&mut shell, &screen, host);
        shell.handle(Command::Close(tab));
    }
    assert_eq!(engine.extension_data_changes.lock().unwrap().len(), 2);

    ask(
        &mut shell,
        profile,
        1,
        ExtensionDataCall::RecentlyClosed { limit: 1 },
    );
    ask(
        &mut shell,
        profile,
        2,
        ExtensionDataCall::Restore { session: None },
    );
    ask(
        &mut shell,
        profile,
        3,
        ExtensionDataCall::Restore {
            session: Some("not-a-session".into()),
        },
    );
    let answers = engine.extension_data_answers.lock().unwrap();
    let ExtensionDataReply::Closed(closed) = &answers[0].2 else {
        panic!("closed tabs: {answers:?}");
    };
    assert_eq!(closed.len(), 1);
    assert_eq!(closed[0].url, "https://second.example/");
    let ExtensionDataReply::Restored(restored) = &answers[1].2 else {
        panic!("restored: {answers:?}");
    };
    assert_eq!(restored.session, closed[0].session);
    assert_eq!(answers[2].2, ExtensionDataReply::Unavailable);
    assert_eq!(shell.recently_closed.len(), 1);
    assert!(engine
        .extension_data_changes
        .lock()
        .unwrap()
        .iter()
        .all(|(_, change)| *change == ExtensionDataChange::Sessions));
}
