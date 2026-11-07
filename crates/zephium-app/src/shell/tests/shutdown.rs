use super::*;

struct OrderedShutdownBlocker {
    order: Arc<Mutex<Vec<&'static str>>>,
    panic_on_shutdown: bool,
}

impl BlockerCompiler for OrderedShutdownBlocker {
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
        self.order.lock().unwrap().push("blocker");
        assert!(!self.panic_on_shutdown, "injected blocker shutdown panic");
        BlockerShutdownOutcome::Clean
    }
}

impl BlockerCatalog for OrderedShutdownBlocker {
    fn maintain(&self) -> BlockerCatalogSnapshot {
        test_catalog_snapshot()
    }

    fn request_refresh(&self) -> BlockerCatalogRefreshDispatch {
        BlockerCatalogRefreshDispatch::Busy
    }
}

#[test]
fn shutdown_snapshots_flushes_and_seals_the_actor_once() {
    let store = Arc::new(FakeStore::default());
    let (mut shell, _engine, screen) = setup_with(store.clone());
    shell.handle(Command::Bootstrap);
    let id = active_id(&screen);
    navigate_and_commit(&mut shell, id, "example.com");
    // Title updates are normally not session writes. The shutdown-owned
    // snapshot must nevertheless capture the actor's latest truth.
    shell.handle(Command::Engine(EngineEvent::TitleChanged {
        id,
        title: "Final title".into(),
    }));

    let (ack, done) = sync_channel(1);
    shell.handle(Command::Shutdown {
        deadline: test_shutdown_deadline(),
        ack,
    });
    assert_eq!(done.recv().unwrap(), ShutdownOutcome::Clean);

    let events = store.events.lock().unwrap().clone();
    assert!(events.ends_with(&["save", "flush", "flush"]), "{events:?}");
    assert_eq!(events.iter().filter(|event| **event == "flush").count(), 2);
    let saved = store.saved.lock().unwrap().clone().unwrap();
    let tab = saved.items.iter().find(|item| item.id == id).unwrap();
    let PersistedKind::Tab { title, .. } = &tab.kind else {
        panic!("active item must remain a tab");
    };
    assert_eq!(title, "Final title");

    // Late UI/engine/timer work cannot mutate state after the barrier.
    let tabs = last(&screen).tabs.len();
    shell.handle(Command::Open);
    assert_eq!(last(&screen).tabs.len(), tabs);

    // A repeated close request receives the same completion without a
    // second snapshot or flush.
    let before = store.events.lock().unwrap().clone();
    let (ack, done) = sync_channel(1);
    shell.handle(Command::Shutdown {
        deadline: test_shutdown_deadline(),
        ack,
    });
    assert_eq!(done.recv().unwrap(), ShutdownOutcome::Clean);
    assert_eq!(*store.events.lock().unwrap(), before);
}

fn assert_terminal_store_callback_cannot_reenter_store(
    case: &str,
    outcome: impl FnOnce(ProfileId) -> BlockerConfigUpdateOutcome,
) {
    let store = super::blocker::opted_out_store();
    store
        .hold_blocker_updates
        .store(true, std::sync::atomic::Ordering::Release);
    let (mut shell, _engine, _screen, operations) = setup_with_operation_log(store.clone());
    shell.handle(Command::Bootstrap);
    let profile = shell.windows.focused().unwrap().profile;
    let operation_id = format!("shutdown-late-store-result-{case}");
    shell.handle(Command::Operation {
        operation_id: operation_id.clone(),
        command: Box::new(Command::SetFocusedContentBlockerEnabled(true)),
    });
    assert_eq!(store.held_blocker_updates.lock().unwrap().len(), 1);
    *store.blocker_update_on_shutdown.lock().unwrap() = Some(outcome(profile));

    let (ack, done) = sync_channel(1);
    shell.handle(Command::Shutdown {
        deadline: test_shutdown_deadline(),
        ack,
    });

    assert_eq!(done.recv().unwrap(), ShutdownOutcome::Clean);
    assert_eq!(
        store
            .blocker_load_calls
            .load(std::sync::atomic::Ordering::Acquire),
        0,
        "a terminally late indeterminate mutation must not start reconciliation after Store shutdown"
    );
    assert_eq!(
        store
            .blocker_load_after_shutdown_calls
            .load(std::sync::atomic::Ordering::Acquire),
        0
    );
    assert_eq!(
        operations.lock().unwrap().as_slice(),
        &[OperationDisposition {
            operation_id,
            outcome: OperationOutcome::Deferred,
            reason: OperationReason::StoreOutcomeUnknown,
        }]
    );
}

#[test]
fn terminal_store_callbacks_cannot_reenter_store_after_shutdown() {
    assert_terminal_store_callback_cannot_reenter_store("unknown", |_| {
        BlockerConfigUpdateOutcome::OutcomeUnknown
    });
    assert_terminal_store_callback_cannot_reenter_store("malformed-updated", |profile| {
        BlockerConfigUpdateOutcome::Updated(ProfileBlockerConfig {
            profile,
            revision: BlockerConfigRevision::INITIAL,
            config: BlockerConfig { enabled: true },
        })
    });
    assert_terminal_store_callback_cannot_reenter_store("malformed-conflict", |profile| {
        BlockerConfigUpdateOutcome::Conflict(ProfileBlockerConfig {
            profile,
            revision: BlockerConfigRevision::INITIAL,
            config: BlockerConfig { enabled: true },
        })
    });
}

#[test]
fn pre_terminal_store_result_reconciles_before_store_shutdown() {
    let store = super::blocker::opted_out_store();
    store
        .hold_blocker_updates
        .store(true, std::sync::atomic::Ordering::Release);
    let (mut shell, _engine, _screen, operations) = setup_with_operation_log(store.clone());
    shell.handle(Command::Bootstrap);
    shell.handle(Command::Operation {
        operation_id: "shutdown-pre-store-result".into(),
        command: Box::new(Command::SetFocusedContentBlockerEnabled(true)),
    });
    store.complete_blocker_update(BlockerConfigUpdateOutcome::OutcomeUnknown);

    let (ack, done) = sync_channel(1);
    shell.handle(Command::Shutdown {
        deadline: test_shutdown_deadline(),
        ack,
    });

    assert_eq!(done.recv().unwrap(), ShutdownOutcome::Clean);
    assert_eq!(
        store
            .blocker_load_calls
            .load(std::sync::atomic::Ordering::Acquire),
        1,
        "a result published before Store's terminal barrier should still reconcile"
    );
    assert_eq!(
        store
            .blocker_load_after_shutdown_calls
            .load(std::sync::atomic::Ordering::Acquire),
        0
    );
    assert_eq!(
        operations.lock().unwrap().as_slice(),
        &[OperationDisposition {
            operation_id: "shutdown-pre-store-result".into(),
            outcome: OperationOutcome::Deferred,
            reason: OperationReason::StoreOutcomeUnknown,
        }]
    );
}

#[test]
fn shutdown_deadline_includes_time_spent_waiting_before_actor_processing() {
    let store = Arc::new(FakeStore::default());
    let (mut shell, _engine, _screen) = setup_with(store.clone());
    shell.handle(Command::Bootstrap);
    store.events.lock().unwrap().clear();
    let (ack, done) = sync_channel(1);

    shell.handle(Command::Shutdown {
        deadline: std::time::Instant::now() - std::time::Duration::from_millis(1),
        ack,
    });

    assert_eq!(done.recv().unwrap(), ShutdownOutcome::RetryableFailure);
    assert!(shell.shutdown_result.is_none());
    assert!(store.events.lock().unwrap().is_empty());

    let (ack, done) = sync_channel(1);
    shell.handle(Command::Shutdown {
        deadline: test_shutdown_deadline(),
        ack,
    });
    assert_eq!(done.recv().unwrap(), ShutdownOutcome::Clean);
}

#[test]
fn store_read_quiescence_failure_keeps_shutdown_retryable() {
    let store = Arc::new(FakeStore::default());
    store
        .history_delay_ms
        .store(200, std::sync::atomic::Ordering::Release);
    let reads = StoreReadQueue::new();
    let callback_owner = crate::actor::Handle::new(CommandQueue::new());
    let reader = {
        let store = store.clone();
        let reads = reads.clone();
        let callback = callback_owner.callback_handle();
        std::thread::spawn(move || crate::store_reads::run(store, reads, callback))
    };
    assert!(reads.request_history(
        1,
        ProfileId::from(7),
        "blocked real reader".into(),
        crate::store_reads::Sources::HISTORY
    ));
    let started_deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while !store
        .history_started
        .load(std::sync::atomic::Ordering::Acquire)
        && std::time::Instant::now() < started_deadline
    {
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    assert!(store
        .history_started
        .load(std::sync::atomic::Ordering::Acquire));

    let mut shell = Shell::with_store_reads(
        ShellPorts::new(
            Arc::new(FakeEngine::default()),
            store,
            Arc::new(ImmediateAllowAllCompiler),
            Box::new(|_| {}),
            Arc::new(FakeChrome),
            Box::new(|_| {}),
        ),
        reads.clone(),
        true,
    );

    let (ack, done) = sync_channel(1);
    shell.handle(Command::Shutdown {
        deadline: std::time::Instant::now() + std::time::Duration::from_millis(1),
        ack,
    });
    assert_eq!(done.recv().unwrap(), ShutdownOutcome::RetryableFailure);

    assert!(reads.quiesce_until(std::time::Instant::now() + std::time::Duration::from_secs(2)));
    let (ack, done) = sync_channel(1);
    shell.handle(Command::Shutdown {
        deadline: test_shutdown_deadline(),
        ack,
    });
    assert_eq!(done.recv().unwrap(), ShutdownOutcome::Clean);
    reader
        .join()
        .expect("real Store reader exits after shutdown");
    drop(callback_owner);
}

#[test]
fn failed_shutdown_barrier_keeps_state_live_for_retry() {
    let store = Arc::new(FakeStore::default());
    *store.flush_result.lock().unwrap() = Some(false);
    let (mut shell, _engine, screen) = setup_with(store.clone());
    shell.handle(Command::Bootstrap);

    let (ack, done) = sync_channel(1);
    shell.handle(Command::Shutdown {
        deadline: test_shutdown_deadline(),
        ack,
    });
    assert_eq!(done.recv().unwrap(), ShutdownOutcome::RetryableFailure);

    let before = last(&screen).tabs.len();
    shell.handle(Command::Open);
    assert_eq!(last(&screen).tabs.len(), before + 1);

    *store.flush_result.lock().unwrap() = Some(true);
    let (ack, done) = sync_channel(1);
    shell.handle(Command::Shutdown {
        deadline: test_shutdown_deadline(),
        ack,
    });
    assert_eq!(done.recv().unwrap(), ShutdownOutcome::Clean);
}

#[test]
fn uncertain_store_termination_still_initiates_native_teardown() {
    let store = Arc::new(FakeStore::default());
    *store.shutdown_outcome.lock().unwrap() = Some(StoreShutdownOutcome::Unclean);
    let (mut shell, engine, _screen) = setup_with(store);
    shell.handle(Command::Bootstrap);

    let (ack, done) = sync_channel(1);
    shell.handle(Command::Shutdown {
        deadline: test_shutdown_deadline(),
        ack,
    });

    assert_eq!(done.recv().unwrap(), ShutdownOutcome::Unclean);
    assert_eq!(
        engine
            .shutdown_calls
            .load(std::sync::atomic::Ordering::Acquire),
        1
    );
}

#[cfg(feature = "agentic-browser")]
#[test]
fn clean_agent_lifecycle_proof_precedes_every_terminal_owner() {
    let order = Arc::new(Mutex::new(Vec::new()));
    let store = Arc::new(FakeStore::default());
    *store.shutdown_order.lock().unwrap() = Some(Arc::clone(&order));
    let engine = Arc::new(FakeEngine::default());
    *engine.shutdown_order.lock().unwrap() = Some(Arc::clone(&order));
    let blocker = Arc::new(OrderedShutdownBlocker {
        order: Arc::clone(&order),
        panic_on_shutdown: false,
    });
    let (agent_lifecycle, agent_state) = agent_lifecycle_with_clean(true);
    *agent_state.shutdown_order.lock().unwrap() = Some(Arc::clone(&order));
    let deadline = test_shutdown_deadline();
    let mut shell = Shell::new_with_agent_lifecycle(
        engine,
        store,
        blocker,
        agent_lifecycle,
        Arc::new(FakeChrome),
        Box::new(|_| {}),
    );

    let (ack, done) = sync_channel(1);
    shell.handle(Command::Shutdown { deadline, ack });

    assert_eq!(done.recv().unwrap(), ShutdownOutcome::Clean);
    assert_eq!(
        order.lock().unwrap().as_slice(),
        &["agent", "store", "engine", "blocker"]
    );
    assert_eq!(
        agent_state.deadlines.lock().unwrap().as_slice(),
        &[deadline]
    );
    assert_eq!(
        agent_state
            .shutdown_calls
            .load(std::sync::atomic::Ordering::Acquire),
        1
    );
    assert!(!agent_state
        .dropped_without_shutdown
        .load(std::sync::atomic::Ordering::Acquire));
}

#[cfg(feature = "agentic-browser")]
#[test]
fn unclean_agent_lifecycle_is_terminal_but_does_not_skip_cleanup() {
    let store = Arc::new(FakeStore::default());
    let engine = Arc::new(FakeEngine::default());
    let (agent_lifecycle, agent_state) = agent_lifecycle_with_clean(false);
    let mut shell = Shell::new_with_agent_lifecycle(
        engine.clone(),
        store.clone(),
        Arc::new(ImmediateAllowAllCompiler),
        agent_lifecycle,
        Arc::new(FakeChrome),
        Box::new(|_| {}),
    );

    let (ack, done) = sync_channel(1);
    shell.handle(Command::Shutdown {
        deadline: test_shutdown_deadline(),
        ack,
    });

    assert_eq!(done.recv().unwrap(), ShutdownOutcome::Unclean);
    assert_eq!(
        agent_state
            .shutdown_calls
            .load(std::sync::atomic::Ordering::Acquire),
        1
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
}

#[cfg(feature = "agentic-browser")]
#[test]
fn retryable_preflight_preserves_agent_lifecycle_for_one_later_consumption() {
    let store = Arc::new(FakeStore::default());
    *store.flush_result.lock().unwrap() = Some(false);
    let (agent_lifecycle, agent_state) = agent_lifecycle_with_clean(true);
    let mut shell = Shell::new_with_agent_lifecycle(
        Arc::new(FakeEngine::default()),
        store.clone(),
        Arc::new(ImmediateAllowAllCompiler),
        agent_lifecycle,
        Arc::new(FakeChrome),
        Box::new(|_| {}),
    );

    let (ack, first) = sync_channel(1);
    shell.handle(Command::Shutdown {
        deadline: test_shutdown_deadline(),
        ack,
    });
    assert_eq!(first.recv().unwrap(), ShutdownOutcome::RetryableFailure);
    assert_eq!(
        agent_state
            .shutdown_calls
            .load(std::sync::atomic::Ordering::Acquire),
        0
    );

    *store.flush_result.lock().unwrap() = Some(true);
    let (ack, second) = sync_channel(1);
    shell.handle(Command::Shutdown {
        deadline: test_shutdown_deadline(),
        ack,
    });
    assert_eq!(second.recv().unwrap(), ShutdownOutcome::Clean);
    assert_eq!(
        agent_state
            .shutdown_calls
            .load(std::sync::atomic::Ordering::Acquire),
        1
    );
}

#[cfg(feature = "agentic-browser")]
#[test]
fn panicking_agent_lifecycle_is_contained_and_later_barriers_run() {
    let order = Arc::new(Mutex::new(Vec::new()));
    let store = Arc::new(FakeStore::default());
    *store.shutdown_order.lock().unwrap() = Some(Arc::clone(&order));
    let engine = Arc::new(FakeEngine::default());
    *engine.shutdown_order.lock().unwrap() = Some(Arc::clone(&order));
    let blocker = Arc::new(OrderedShutdownBlocker {
        order: Arc::clone(&order),
        panic_on_shutdown: false,
    });
    let (agent_lifecycle, agent_state) = agent_lifecycle_with_clean(true);
    *agent_state.shutdown_order.lock().unwrap() = Some(Arc::clone(&order));
    agent_state
        .panic_on_shutdown
        .store(true, std::sync::atomic::Ordering::Release);
    let mut shell = Shell::new_with_agent_lifecycle(
        engine,
        store,
        blocker,
        agent_lifecycle,
        Arc::new(FakeChrome),
        Box::new(|_| {}),
    );

    let (ack, done) = sync_channel(1);
    shell.handle(Command::Shutdown {
        deadline: test_shutdown_deadline(),
        ack,
    });

    assert_eq!(done.recv().unwrap(), ShutdownOutcome::Unclean);
    assert_eq!(
        order.lock().unwrap().as_slice(),
        &["agent", "store", "engine", "blocker"]
    );
}

#[test]
fn every_panicking_terminal_barrier_runs_and_acknowledges_unclean_once() {
    let order = Arc::new(Mutex::new(Vec::new()));
    let store = Arc::new(FakeStore::default());
    *store.barrier_order.lock().unwrap() = Some(Arc::clone(&order));
    let engine = Arc::new(FakeEngine::default());
    *engine.shutdown_order.lock().unwrap() = Some(Arc::clone(&order));
    let blocker = Arc::new(OrderedShutdownBlocker {
        order: Arc::clone(&order),
        panic_on_shutdown: true,
    });
    let mut shell = Shell::new_with_failure(
        engine.clone(),
        store.clone(),
        blocker,
        Box::new(|_| {}),
        Arc::new(FakeChrome),
        Box::new(|_| {}),
    );
    shell.handle(Command::Bootstrap);
    order.lock().unwrap().clear();

    store
        .panic_on_save
        .store(true, std::sync::atomic::Ordering::Release);
    store
        .panic_on_flush
        .store(true, std::sync::atomic::Ordering::Release);
    store
        .panic_on_shutdown
        .store(true, std::sync::atomic::Ordering::Release);
    engine
        .panic_on_shutdown
        .store(true, std::sync::atomic::Ordering::Release);

    let (ack, done) = sync_channel(1);
    shell.handle(Command::Shutdown {
        deadline: test_shutdown_deadline(),
        ack,
    });

    assert_eq!(done.recv().unwrap(), ShutdownOutcome::Unclean);
    assert!(
        done.try_recv().is_err(),
        "shutdown ack must resolve exactly once"
    );
    assert_eq!(shell.shutdown_result, Some(ShutdownOutcome::Unclean));
    assert_eq!(
        order.lock().unwrap().as_slice(),
        &[
            "persist",
            "store-preflight",
            "store-final",
            "engine",
            "blocker",
        ]
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
}

#[test]
fn failed_shutdown_folds_racing_native_failure_before_resuming() {
    let store = Arc::new(FakeStore::default());
    *store.flush_result.lock().unwrap() = Some(false);
    let (mut shell, engine, screen) = setup_with(store);
    shell.handle(Command::Bootstrap);
    let id = active_id(&screen);
    navigate_and_commit(&mut shell, id, "example.com");

    let queue = CommandQueue::new();
    shell.self_queue = Some(queue.clone());
    let handle = Handle::new(queue.clone());
    let (ack, done) = sync_channel(1);
    queue
        .try_push(Command::Shutdown {
            deadline: test_shutdown_deadline(),
            ack,
        })
        .ok()
        .unwrap();
    let shutdown = queue.try_recv().unwrap();
    assert!(!handle.dispatch(Command::Engine(EngineEvent::Crashed { id })));

    shell.handle(shutdown);
    assert_eq!(done.recv().unwrap(), ShutdownOutcome::RetryableFailure);
    assert!(!engine
        .calls()
        .iter()
        .any(|call| call == &format!("close {id}")));
    assert_eq!(
        engine
            .calls()
            .iter()
            .filter(|call| call.starts_with(&format!("create {id} ")))
            .count(),
        2
    );
    assert!(handle.dispatch(Command::Open));
}

#[test]
fn native_shutdown_timeout_is_bounded_and_terminal() {
    let (mut shell, engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    engine
        .skip_shutdown_callback
        .store(true, std::sync::atomic::Ordering::Release);

    let before = last(&screen).tabs.len();
    let started = std::time::Instant::now();
    let (ack, done) = sync_channel(1);
    shell.handle(Command::Shutdown {
        deadline: test_shutdown_deadline(),
        ack,
    });
    assert_eq!(done.recv().unwrap(), ShutdownOutcome::Unclean);
    assert!(started.elapsed() < std::time::Duration::from_secs(1));

    // Native teardown may have happened even without its callback. The
    // shell must never resume and issue operations into a partial engine.
    shell.handle(Command::Open);
    assert_eq!(last(&screen).tabs.len(), before);
}

#[test]
fn native_cleanup_rejection_is_terminal_and_sticky() {
    let (mut shell, engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    *engine.shutdown_result.lock().unwrap() = Some(false);

    let before = last(&screen).tabs.len();
    let (ack, done) = sync_channel(1);
    shell.handle(Command::Shutdown {
        deadline: test_shutdown_deadline(),
        ack,
    });
    assert_eq!(done.recv().unwrap(), ShutdownOutcome::Unclean);

    shell.handle(Command::Open);
    assert_eq!(last(&screen).tabs.len(), before);

    let (ack, done) = sync_channel(1);
    shell.handle(Command::Shutdown {
        deadline: test_shutdown_deadline(),
        ack,
    });
    assert_eq!(done.recv().unwrap(), ShutdownOutcome::Unclean);
}
