use super::*;

struct ActorExitBlocker {
    order: Arc<Mutex<Vec<&'static str>>>,
    maintain_calls: Arc<std::sync::atomic::AtomicUsize>,
    shutdown_calls: Arc<std::sync::atomic::AtomicUsize>,
    panic_on_maintain: bool,
}

impl BlockerCompiler for ActorExitBlocker {
    fn compile(
        &self,
        profile: ProfileId,
        generation: ContentPolicyGeneration,
        config: BlockerConfig,
        done: Box<dyn FnOnce(BlockerCompileOutcome) + Send>,
    ) -> BlockerDispatch {
        ImmediateAllowAllCompiler.compile(profile, generation, config, done)
    }

    fn retire_profile(
        &self,
        profile: ProfileId,
        done: Box<dyn FnOnce() + Send>,
    ) -> BlockerRetirementDispatch {
        ImmediateAllowAllCompiler.retire_profile(profile, done)
    }

    fn shutdown_until(&self, _deadline: std::time::Instant) -> BlockerShutdownOutcome {
        self.shutdown_calls
            .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
        self.order.lock().unwrap().push("blocker");
        BlockerShutdownOutcome::Clean
    }
}

impl BlockerCatalog for ActorExitBlocker {
    fn maintain(&self) -> BlockerCatalogSnapshot {
        self.maintain_calls
            .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
        assert!(
            !self.panic_on_maintain,
            "injected initial blocker-catalog panic"
        );
        test_catalog_snapshot()
    }

    fn request_refresh(&self) -> BlockerCatalogRefreshDispatch {
        BlockerCatalogRefreshDispatch::Busy
    }
}

#[test]
fn actor_panic_terminalizes_pending_and_later_shutdown_requests() {
    let store = Arc::new(FakeStore::default());
    let order = Arc::new(Mutex::new(Vec::new()));
    *store.barrier_order.lock().unwrap() = Some(Arc::clone(&order));
    *store.shutdown_outcome.lock().unwrap() = Some(StoreShutdownOutcome::Clean);
    store
        .panic_on_load
        .store(true, std::sync::atomic::Ordering::Release);
    let engine = Arc::new(FakeEngine::default());
    *engine.shutdown_order.lock().unwrap() = Some(Arc::clone(&order));
    let blocker_shutdown_calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let blocker_maintain_calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let blocker = Arc::new(ActorExitBlocker {
        order: Arc::clone(&order),
        maintain_calls: blocker_maintain_calls,
        shutdown_calls: Arc::clone(&blocker_shutdown_calls),
        panic_on_maintain: false,
    });
    let (terminal_tx, terminal_rx) = sync_channel(1);
    let handle = spawn(
        engine.clone(),
        store.clone(),
        blocker,
        Box::new(move |failure| {
            let _ = terminal_tx.send(failure);
        }),
        Arc::new(FakeChrome),
        Box::new(|_| {}),
    )
    .expect("spawn test shell");

    assert!(handle.dispatch(Command::Bootstrap));
    let pending = handle.shutdown();
    assert_eq!(
        pending
            .recv_timeout(std::time::Duration::from_secs(2))
            .unwrap(),
        ShutdownOutcome::Unclean
    );
    assert_eq!(handle.shutdown().recv().unwrap(), ShutdownOutcome::Unclean);
    assert_eq!(
        terminal_rx.recv().unwrap(),
        ShellTerminalFailure::ActorExitedUnexpectedly
    );
    assert_eq!(
        store
            .shutdown_calls
            .load(std::sync::atomic::Ordering::Acquire),
        1
    );
    assert_eq!(
        engine
            .shutdown_calls
            .load(std::sync::atomic::Ordering::Acquire),
        1
    );
    assert_eq!(
        blocker_shutdown_calls.load(std::sync::atomic::Ordering::Acquire),
        1
    );
    assert_eq!(
        order.lock().unwrap().as_slice(),
        &["store-final", "engine", "blocker"]
    );
}

#[test]
fn profile_creation_rejection_projection_panic_does_not_skip_shutdown_barriers() {
    let store = Arc::new(FakeStore::default());
    let engine = Arc::new(FakeEngine::default());
    let mut shell = Shell::new(
        engine,
        store.clone(),
        Arc::new(FakeChrome),
        Box::new(|projection| {
            if matches!(projection, Projection::OperationProcessed(_)) {
                panic!("injected rejection projection panic");
            }
        }),
    );
    shell
        .pending_profile_creations
        .insert(1, Some("pending-operation".into()));
    let (ack, result) = sync_channel(1);

    shell.shutdown_until(test_shutdown_deadline(), ack);

    assert_eq!(result.recv().unwrap(), ShutdownOutcome::Unclean);
    assert_eq!(
        store
            .shutdown_calls
            .load(std::sync::atomic::Ordering::Acquire),
        1
    );
}

#[test]
fn initial_blocker_catalog_panic_keeps_full_shell_under_terminal_cleanup() {
    let store = Arc::new(FakeStore::default());
    let order = Arc::new(Mutex::new(Vec::new()));
    *store.barrier_order.lock().unwrap() = Some(Arc::clone(&order));
    *store.shutdown_outcome.lock().unwrap() = Some(StoreShutdownOutcome::Clean);
    let engine = Arc::new(FakeEngine::default());
    *engine.shutdown_order.lock().unwrap() = Some(Arc::clone(&order));
    let blocker_shutdown_calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let blocker_maintain_calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let blocker = Arc::new(ActorExitBlocker {
        order: Arc::clone(&order),
        maintain_calls: blocker_maintain_calls,
        shutdown_calls: Arc::clone(&blocker_shutdown_calls),
        panic_on_maintain: true,
    });
    let callback_calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let callback_calls_for_actor = Arc::clone(&callback_calls);
    let (terminal_tx, terminal_rx) = sync_channel(1);

    let handle = spawn(
        engine.clone(),
        store.clone(),
        blocker,
        Box::new(move |failure| {
            callback_calls_for_actor.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
            let _ = terminal_tx.send(failure);
        }),
        Arc::new(FakeChrome),
        Box::new(|_| {}),
    )
    .expect("worker construction and ownership handoff must succeed");

    assert_eq!(
        terminal_rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("guarded construction panic must reach the terminal callback"),
        ShellTerminalFailure::ActorExitedUnexpectedly
    );
    assert_eq!(handle.shutdown().recv().unwrap(), ShutdownOutcome::Unclean);
    assert_eq!(callback_calls.load(std::sync::atomic::Ordering::Acquire), 1);
    assert!(terminal_rx.try_recv().is_err());
    assert_eq!(
        store
            .shutdown_calls
            .load(std::sync::atomic::Ordering::Acquire),
        1
    );
    assert_eq!(
        engine
            .shutdown_calls
            .load(std::sync::atomic::Ordering::Acquire),
        1
    );
    assert_eq!(
        blocker_shutdown_calls.load(std::sync::atomic::Ordering::Acquire),
        1
    );
    assert_eq!(
        order.lock().unwrap().as_slice(),
        &["store-final", "engine", "blocker"]
    );
}

#[test]
fn suspended_actor_enters_no_external_port_before_composition_admission() {
    let store = Arc::new(FakeStore::default());
    *store.shutdown_outcome.lock().unwrap() = Some(StoreShutdownOutcome::Clean);
    let engine = Arc::new(FakeEngine::default());
    let order = Arc::new(Mutex::new(Vec::new()));
    *store.barrier_order.lock().unwrap() = Some(Arc::clone(&order));
    *engine.shutdown_order.lock().unwrap() = Some(Arc::clone(&order));
    let maintain_calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let blocker_shutdown_calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let blocker = Arc::new(ActorExitBlocker {
        order: Arc::clone(&order),
        maintain_calls: Arc::clone(&maintain_calls),
        shutdown_calls: Arc::clone(&blocker_shutdown_calls),
        panic_on_maintain: true,
    });
    let published = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let callback_observed_publication = Arc::clone(&published);
    let callback_order = Arc::clone(&order);
    let (terminal_tx, terminal_rx) = sync_channel(1);

    let handle = crate::spawn_suspended(
        engine.clone(),
        store.clone(),
        blocker,
        Box::new(move |failure| {
            assert!(
                callback_observed_publication.load(std::sync::atomic::Ordering::Acquire),
                "terminal callback must not run before desktop publishes ownership"
            );
            callback_order.lock().unwrap().push("terminal");
            let _ = terminal_tx.send(failure);
        }),
        Arc::new(FakeChrome),
        Box::new(|_| {}),
    )
    .expect("suspended Shell construction");

    assert!(handle.wait_until_startup_suspended(
        std::time::Instant::now() + std::time::Duration::from_secs(2)
    ));
    assert_eq!(maintain_calls.load(std::sync::atomic::Ordering::Acquire), 0);
    assert!(order.lock().unwrap().is_empty());

    // Models desktop publishing the Handle and clearing all temporary owners.
    published.store(true, std::sync::atomic::Ordering::Release);
    assert!(handle.admit_startup());
    assert!(!handle.admit_startup(), "admission is exactly once");
    assert_eq!(
        terminal_rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("guarded startup panic reaches the published owner"),
        ShellTerminalFailure::ActorExitedUnexpectedly
    );
    assert_eq!(handle.shutdown().recv().unwrap(), ShutdownOutcome::Unclean);

    assert_eq!(maintain_calls.load(std::sync::atomic::Ordering::Acquire), 1);
    assert_eq!(
        store
            .shutdown_calls
            .load(std::sync::atomic::Ordering::Acquire),
        1
    );
    assert_eq!(
        engine
            .shutdown_calls
            .load(std::sync::atomic::Ordering::Acquire),
        1
    );
    assert_eq!(
        blocker_shutdown_calls.load(std::sync::atomic::Ordering::Acquire),
        1
    );
    assert_eq!(
        order.lock().unwrap().as_slice(),
        &["terminal", "store-final", "engine", "blocker"]
    );
}

#[test]
fn terminal_barrier_cancels_suspended_startup_without_external_admission() {
    let store = Arc::new(FakeStore::default());
    *store.shutdown_outcome.lock().unwrap() = Some(StoreShutdownOutcome::Clean);
    let engine = Arc::new(FakeEngine::default());
    let order = Arc::new(Mutex::new(Vec::new()));
    *store.barrier_order.lock().unwrap() = Some(Arc::clone(&order));
    *engine.shutdown_order.lock().unwrap() = Some(Arc::clone(&order));
    let maintain_calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let blocker_shutdown_calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let blocker = Arc::new(ActorExitBlocker {
        order: Arc::clone(&order),
        maintain_calls: Arc::clone(&maintain_calls),
        shutdown_calls: Arc::clone(&blocker_shutdown_calls),
        panic_on_maintain: false,
    });
    let terminal_calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let terminal_calls_for_actor = Arc::clone(&terminal_calls);
    let handle = crate::spawn_suspended(
        engine.clone(),
        store.clone(),
        blocker,
        Box::new(move |_| {
            terminal_calls_for_actor.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
        }),
        Arc::new(FakeChrome),
        Box::new(|_| {}),
    )
    .expect("suspended Shell construction");

    assert!(handle.wait_until_startup_suspended(
        std::time::Instant::now() + std::time::Duration::from_secs(2)
    ));
    assert_eq!(handle.shutdown().recv().unwrap(), ShutdownOutcome::Clean);
    assert_eq!(maintain_calls.load(std::sync::atomic::Ordering::Acquire), 0);
    assert_eq!(terminal_calls.load(std::sync::atomic::Ordering::Acquire), 0);
    assert_eq!(
        store
            .shutdown_calls
            .load(std::sync::atomic::Ordering::Acquire),
        1
    );
    assert_eq!(
        engine
            .shutdown_calls
            .load(std::sync::atomic::Ordering::Acquire),
        1
    );
    assert_eq!(
        blocker_shutdown_calls.load(std::sync::atomic::Ordering::Acquire),
        1
    );
    assert_eq!(
        order.lock().unwrap().as_slice(),
        &["store-preflight", "store-final", "engine", "blocker",]
    );
}

#[test]
fn spawned_actor_processes_dispatched_commands() {
    let (tx, rx) = std::sync::mpsc::channel();
    let handle = spawn(
        Arc::new(FakeEngine::default()),
        Arc::new(FakeStore::default()),
        Arc::new(ImmediateAllowAllCompiler),
        Box::new(|_| {}),
        Arc::new(FakeChrome),
        Box::new(move |s| {
            let _ = tx.send(s);
        }),
    )
    .expect("spawn test shell");
    handle.dispatch(Command::SetWindowSize(Size::new(1200.0, 800.0)));
    handle.dispatch(Command::Bootstrap);
    let projection = std::iter::from_fn(|| rx.recv_timeout(std::time::Duration::from_secs(2)).ok())
        .find(|p| matches!(p, Projection::Items(_)))
        .expect("bootstrap must project an items snapshot");
    let Projection::Items(s) = projection else {
        unreachable!()
    };
    assert!(s.active.is_some());
}

#[test]
fn slow_history_sqlite_read_never_blocks_shell_coordination() {
    let store = Arc::new(FakeStore::default());
    store
        .history_delay_ms
        .store(500, std::sync::atomic::Ordering::Release);
    let (tx, rx) = std::sync::mpsc::channel();
    let handle = spawn(
        Arc::new(FakeEngine::default()),
        store,
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
    assert!(
        std::iter::from_fn(|| rx.recv_timeout(std::time::Duration::from_secs(2)).ok())
            .any(|projection| matches!(projection, Projection::Items(_)))
    );

    let started = std::time::Instant::now();
    assert!(handle.dispatch(Command::Search("slow".into())));
    assert!(handle.dispatch(Command::Open));
    let opened = std::iter::from_fn(|| rx.recv_timeout(std::time::Duration::from_millis(250)).ok())
        .find_map(|projection| match projection {
            Projection::Items(items) if items.tabs.len() == 2 => Some(items),
            _ => None,
        });
    assert!(
        opened.is_some(),
        "shell actor stalled behind SQLite history"
    );
    assert!(started.elapsed() < std::time::Duration::from_millis(400));
}

#[test]
fn tracked_operation_has_exact_admission_and_actor_disposition_id() {
    let (tx, rx) = std::sync::mpsc::channel();
    let handle = spawn(
        Arc::new(FakeEngine::default()),
        Arc::new(FakeStore::default()),
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
    assert!(handle.dispatch_operation("operation-7".into(), Command::Open));

    let completion = std::iter::from_fn(|| rx.recv_timeout(std::time::Duration::from_secs(2)).ok())
        .find_map(|projection| match projection {
            Projection::OperationProcessed(completion) => Some(completion),
            _ => None,
        })
        .expect("admitted operation must complete in actor order");
    assert_eq!(completion.operation_id, "operation-7");
    assert_eq!(completion.outcome, OperationOutcome::Deferred);
    assert_eq!(completion.reason, OperationReason::NativeWorkPending);
    assert!(!handle.dispatch_operation(String::new(), Command::Open));
    assert!(!handle.dispatch_operation(
        "nested".into(),
        Command::Operation {
            operation_id: "inner".into(),
            command: Box::new(Command::Open),
        },
    ));
}

#[test]
fn spawned_shutdown_is_ordered_behind_prior_commands() {
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
    handle.dispatch(Command::SetWindowSize(Size::new(1200.0, 800.0)));
    handle.dispatch(Command::Bootstrap);
    let state = std::iter::from_fn(|| rx.recv_timeout(std::time::Duration::from_secs(2)).ok())
        .find_map(|projection| match projection {
            Projection::Items(state) => Some(state),
            _ => None,
        })
        .expect("bootstrap snapshot");
    let id = ItemId::parse(state.active.as_deref().unwrap()).unwrap();

    handle.dispatch(Command::Navigate {
        id,
        input: "example.com".into(),
    });
    handle.dispatch(Command::Engine(EngineEvent::UrlChanged {
        id,
        url: "https://example.com/".into(),
    }));
    handle.dispatch(Command::Engine(EngineEvent::TitleChanged {
        id,
        title: "Queued before close".into(),
    }));
    let completion = handle.shutdown();
    let workers = completion.workers.clone();
    assert_eq!(
        completion
            .recv_timeout(std::time::Duration::from_secs(2))
            .unwrap(),
        ShutdownOutcome::Clean
    );
    assert!(workers.actor_and_timer_stopped());

    let saved = store.saved.lock().unwrap().clone().unwrap();
    let tab = saved.items.iter().find(|item| item.id == id).unwrap();
    let PersistedKind::Tab { title, .. } = &tab.kind else {
        panic!("active item must remain a tab");
    };
    assert_eq!(title, "Queued before close");
    assert_eq!(
        store
            .events
            .lock()
            .unwrap()
            .iter()
            .filter(|event| **event == "flush")
            .count(),
        2
    );
}

#[test]
fn dropping_last_handle_does_not_cancel_an_accepted_shutdown_barrier() {
    let handle = spawn(
        Arc::new(FakeEngine::default()),
        Arc::new(FakeStore::default()),
        Arc::new(ImmediateAllowAllCompiler),
        Box::new(|_| {}),
        Arc::new(FakeChrome),
        Box::new(|_| {}),
    )
    .expect("spawn test shell");
    assert!(handle.dispatch(Command::Bootstrap));
    let completion = handle.shutdown();
    drop(handle);

    assert_eq!(
        completion
            .recv_timeout(std::time::Duration::from_secs(2))
            .unwrap(),
        ShutdownOutcome::Clean
    );
}
