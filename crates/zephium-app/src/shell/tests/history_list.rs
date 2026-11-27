use super::*;
use zephium_core::ports::engine::{HistoryEntries, HistoryEntry};

fn listing(shell: &mut Shell, id: ItemId) -> std::sync::mpsc::Receiver<HistoryEntries> {
    let (send, receive) = std::sync::mpsc::channel();
    shell.handle(Command::HistoryList {
        id,
        done: crate::HistoryListCompletion::new(move |entries| {
            let _ = send.send(entries);
        }),
    });
    receive
}

fn page_with_history(shell: &mut Shell, screen: &Screen, back: bool, forward: bool) -> ItemId {
    shell.handle(Command::Bootstrap);
    let id = active_id(screen);
    navigate_and_commit(shell, id, "history.example");
    shell.handle(Command::Engine(EngineEvent::NavState {
        id,
        can_go_back: back,
        can_go_forward: forward,
    }));
    id
}

fn entry(offset: i32, url: &str) -> HistoryEntry {
    HistoryEntry {
        offset,
        title: format!("Title {offset}"),
        url: url.into(),
    }
}

#[test]
fn a_menu_waits_for_the_engine_and_gets_the_list_for_its_own_request() {
    let (mut shell, engine, screen) = setup();
    let id = page_with_history(&mut shell, &screen, true, true);

    let first = listing(&mut shell, id);
    let second = listing(&mut shell, id);
    let calls: Vec<String> = engine
        .calls()
        .into_iter()
        .filter(|call| call.starts_with("history-list"))
        .collect();
    assert_eq!(calls.len(), 2);
    assert!(first.try_recv().is_err());

    let list = HistoryEntries {
        back: vec![entry(-1, "https://a.example/")],
        forward: vec![entry(1, "https://c.example/")],
    };
    // An answer from another tab, or for a request nobody made, settles nothing.
    shell.handle(Command::Engine(EngineEvent::HistoryEntries {
        id: ItemId::generate(),
        request: HistoryRequestId(1),
        entries: list.clone(),
    }));
    shell.handle(Command::Engine(EngineEvent::HistoryEntries {
        id,
        request: HistoryRequestId(99),
        entries: list.clone(),
    }));
    assert!(first.try_recv().is_err() && second.try_recv().is_err());

    shell.handle(Command::Engine(EngineEvent::HistoryEntries {
        id,
        request: HistoryRequestId(2),
        entries: list.clone(),
    }));
    assert_eq!(second.recv().unwrap(), list);
    assert!(first.try_recv().is_err());

    shell.handle(Command::Engine(EngineEvent::HistoryEntries {
        id,
        request: HistoryRequestId(1),
        entries: HistoryEntries::default(),
    }));
    assert_eq!(first.recv().unwrap(), HistoryEntries::default());
}

#[test]
fn a_tab_with_nothing_to_go_to_or_a_refusing_engine_answers_empty_at_once() {
    let (mut shell, engine, screen) = setup();
    let id = page_with_history(&mut shell, &screen, false, false);
    assert_eq!(
        listing(&mut shell, id).recv().unwrap(),
        HistoryEntries::default()
    );

    shell.handle(Command::Engine(EngineEvent::NavState {
        id,
        can_go_back: true,
        can_go_forward: false,
    }));
    engine
        .reject_native_dispatch
        .store(true, std::sync::atomic::Ordering::Release);
    assert_eq!(
        listing(&mut shell, id).recv().unwrap(),
        HistoryEntries::default()
    );
    assert_eq!(
        listing(&mut shell, ItemId::generate()).recv().unwrap(),
        HistoryEntries::default()
    );
    assert!(engine
        .calls()
        .iter()
        .all(|call| !call.starts_with("history-list") || call.contains(&id.to_string())));
}

#[test]
fn menus_beyond_the_bound_and_a_shutdown_are_answered_empty() {
    let (mut shell, _engine, screen) = setup();
    let id = page_with_history(&mut shell, &screen, true, false);
    let waiting: Vec<_> = (0..5).map(|_| listing(&mut shell, id)).collect();
    // Room was made for the fifth by answering the oldest.
    assert_eq!(waiting[0].recv().unwrap(), HistoryEntries::default());
    assert!(waiting[1..].iter().all(|menu| menu.try_recv().is_err()));

    shell.fail_pending_history_lists();
    for menu in &waiting[1..] {
        assert_eq!(menu.recv().unwrap(), HistoryEntries::default());
    }
}

#[test]
fn going_to_an_entry_asks_the_engine_for_that_offset_and_address() {
    let (mut shell, engine, screen) = setup();
    let id = page_with_history(&mut shell, &screen, true, true);

    let back = shell.handle_operation(Command::GoToHistoryEntry {
        id,
        offset: -3,
        url: "https://old.example/".into(),
    });
    assert_eq!(back.outcome, OperationOutcome::Deferred);
    let forward = shell.handle_operation(Command::GoToHistoryEntry {
        id,
        offset: 2,
        url: "https://next.example/".into(),
    });
    assert_eq!(forward.outcome, OperationOutcome::Deferred);
    let calls = engine.calls();
    assert!(calls.contains(&format!("history-go {id} -3 https://old.example/")));
    assert!(calls.contains(&format!("history-go {id} 2 https://next.example/")));
}

#[test]
fn an_entry_that_is_not_reachable_or_not_a_web_address_is_refused_before_the_engine() {
    let (mut shell, engine, screen) = setup();
    let id = page_with_history(&mut shell, &screen, true, false);
    let refuse = |shell: &mut Shell, id, offset, url: &str| {
        shell
            .handle_operation(Command::GoToHistoryEntry {
                id,
                offset,
                url: url.into(),
            })
            .outcome
    };
    // Nothing is forward of this page, and no step is zero or beyond the list.
    assert_eq!(
        refuse(&mut shell, id, 1, "https://a.example/"),
        OperationOutcome::NoOp
    );
    assert_eq!(
        refuse(&mut shell, id, 0, "https://a.example/"),
        OperationOutcome::NoOp
    );
    assert_eq!(
        refuse(&mut shell, id, -51, "https://a.example/"),
        OperationOutcome::NoOp
    );
    assert_eq!(
        refuse(&mut shell, id, i32::MIN, "https://a.example/"),
        OperationOutcome::NoOp
    );
    assert_eq!(
        refuse(&mut shell, id, -1, "javascript:alert(1)"),
        OperationOutcome::NoOp
    );
    assert_eq!(
        refuse(&mut shell, ItemId::generate(), -1, "https://a.example/"),
        OperationOutcome::Rejected
    );
    assert!(engine
        .calls()
        .iter()
        .all(|call| !call.starts_with("history-go")));
}
