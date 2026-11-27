use super::*;

#[test]
fn url_churn_costs_one_snapshot_that_reaches_the_store_within_two_seconds() {
    let store = Arc::new(FakeStore::default());
    let (tx, rx) = std::sync::mpsc::channel();
    let handle = spawn(
        Arc::new(FakeEngine::default()),
        store.clone(),
        Arc::new(ImmediateAllowAllCompiler),
        Box::new(|_| {}),
        Arc::new(FakeChrome),
        Box::new(move |projection| {
            let _ = tx.send(projection);
        }),
    )
    .expect("spawn test shell");
    assert!(handle.dispatch(Command::SetWindowSize(Size::new(1200.0, 800.0))));
    assert!(handle.dispatch(Command::Bootstrap));
    let state = std::iter::from_fn(|| rx.recv_timeout(std::time::Duration::from_secs(2)).ok())
        .find_map(|projection| match projection {
            Projection::Items(state) => Some(state),
            _ => None,
        })
        .expect("bootstrap snapshot");
    let id = ItemId::parse(state.active.as_deref().unwrap()).unwrap();
    // First-run bootstrap durably registers the chrome-visible profile before
    // profile-scoped services can observe it. This test measures only the
    // subsequent URL-churn cadence.
    let first_run_deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while !store.events.lock().unwrap().contains(&"save") {
        assert!(
            std::time::Instant::now() < first_run_deadline,
            "first-run profile registration did not settle"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    store.events.lock().unwrap().clear();

    let started = std::time::Instant::now();
    for value in 0..100 {
        assert!(handle.dispatch(Command::Engine(EngineEvent::UrlChanged {
            id,
            url: format!("https://example.test/{value}"),
        })));
    }
    std::thread::sleep(PERSIST_DEBOUNCE + std::time::Duration::from_millis(150));
    assert!(
        !store.events.lock().unwrap().contains(&"save"),
        "a committed address waits out its delay before the snapshot"
    );
    let deadline = started + std::time::Duration::from_secs(2);
    while !store.events.lock().unwrap().contains(&"save") {
        assert!(
            std::time::Instant::now() < deadline,
            "a committed address must reach the store within about two seconds"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    std::thread::sleep(PERSIST_DEBOUNCE);
    assert_eq!(
        store
            .events
            .lock()
            .unwrap()
            .iter()
            .filter(|event| **event == "save")
            .count(),
        1,
        "one hundred address changes must construct one snapshot"
    );
    let saved = store.saved.lock().unwrap().clone().unwrap();
    assert!(saved.items.iter().any(|item| {
        item.id == id
            && matches!(
                &item.kind,
                PersistedKind::Tab { url, .. }
                    if url == "https://example.test/99"
            )
    }));
}

#[test]
fn a_committed_address_arms_one_deadline_and_structure_preempts_it() {
    let (mut shell, _engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    let queue = CommandQueue::new();
    shell.self_queue = Some(queue.clone());
    let id = active_id(&screen);
    let deadline = || {
        queue
            .inner
            .timer_state
            .lock()
            .unwrap()
            .persist_deadline
            .unwrap()
    };

    let started = std::time::Instant::now();
    shell.handle(Command::Engine(EngineEvent::UrlChanged {
        id,
        url: "https://churn.example/0".into(),
    }));
    let armed = deadline();
    assert!(armed >= started + URL_CHECKPOINT_DELAY);
    assert!(armed <= std::time::Instant::now() + URL_CHECKPOINT_DELAY);
    for value in 1..100 {
        shell.handle(Command::Engine(EngineEvent::UrlChanged {
            id,
            url: format!("https://churn.example/{value}"),
        }));
    }
    assert_eq!(deadline(), armed, "later changes ride the armed snapshot");

    let structural_started = std::time::Instant::now();
    shell.schedule_persist();
    let structural = deadline();
    assert!(structural < armed);
    assert!(structural <= structural_started + PERSIST_MAX_AGE);
}
