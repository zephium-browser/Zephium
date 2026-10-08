use super::*;
use zephium_core::blocker::ContentPolicyGeneration;
use zephium_core::ids::{ItemId, ProfileId};
use zephium_core::ports::engine::EngineEvent;

use crate::shell::tests::{FakeChrome, FakeEngine, FakeStore, ImmediateAllowAllCompiler};

#[cfg(feature = "work-execution")]
#[test]
fn retained_work_wake_uses_owned_full_or_shutdown_drain_but_rejects_closed_queue() {
    let queue = CommandQueue::new();
    let owner = Handle::new(queue.clone());
    let callback = owner.callback_handle();
    assert!(callback.wake_retained_work());
    assert!(matches!(queue.try_recv(), Some(Command::WorkWake)));
    while queue.try_push(Command::Open).is_ok() {}
    // WorkWake owns an existing critical-lifecycle slot beyond normal FIFO
    // saturation and coalesces; it cannot consume unbounded extra capacity.
    assert!(queue.try_push(Command::WorkWake).is_ok());
    assert!(callback.wake_retained_work());
    let _shutdown =
        owner.shutdown_with_deadline(std::time::Instant::now() + std::time::Duration::from_secs(1));
    assert!(matches!(
        queue.try_push(Command::WorkWake),
        Err(TryPushError::Sealed(_))
    ));
    assert!(callback.wake_retained_work());
    drop(queue.close_and_drain());
    assert!(!callback.wake_retained_work());
    drop(owner);
    drop(queue);
    assert!(!callback.wake_retained_work());
}

#[cfg(feature = "agentic-browser")]
#[derive(Default)]
struct AgentLifecycleProbe {
    shutdown_calls: std::sync::atomic::AtomicUsize,
    dropped_without_shutdown: std::sync::atomic::AtomicBool,
}

#[cfg(feature = "agentic-browser")]
struct ProbeAgentLifecycle(Arc<AgentLifecycleProbe>);

#[cfg(feature = "agentic-browser")]
impl Drop for ProbeAgentLifecycle {
    fn drop(&mut self) {
        if self
            .0
            .shutdown_calls
            .load(std::sync::atomic::Ordering::Acquire)
            == 0
        {
            self.0
                .dropped_without_shutdown
                .store(true, std::sync::atomic::Ordering::Release);
        }
    }
}

#[cfg(feature = "agentic-browser")]
impl zephium_agentic::AgentBrowserLifecycle for ProbeAgentLifecycle {
    fn shutdown_until(
        self: Box<Self>,
        _deadline: std::time::Instant,
    ) -> zephium_agentic::AgentBrowserShutdownOutcome {
        self.0
            .shutdown_calls
            .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
        zephium_agentic::AgentBrowserShutdownOutcome::Unclean
    }
}

#[cfg(feature = "agentic-browser")]
fn agent_lifecycle_probe() -> (AgentLifecycle, Arc<AgentLifecycleProbe>) {
    let probe = Arc::new(AgentLifecycleProbe::default());
    (Box::new(ProbeAgentLifecycle(Arc::clone(&probe))), probe)
}

fn spawn_with_test_workers(
    spawner: impl FnMut(&'static str, WorkerTask) -> std::io::Result<WorkerThread>,
) -> Result<Handle, SpawnFailure> {
    spawn_with_worker_spawner(
        ShellHandoff {
            engine: Arc::new(FakeEngine::default()),
            store: Arc::new(FakeStore::default()),
            blocker: Arc::new(ImmediateAllowAllCompiler),
            agent_lifecycle: NoAgentLifecycle,
            terminal_failure: Box::new(|_| {}),
            chrome: Arc::new(FakeChrome),
            emit: Box::new(|_| {}),
        },
        spawner,
    )
}

#[cfg(feature = "agentic-browser")]
fn spawn_agentic_with_test_workers(
    agent_lifecycle: AgentLifecycle,
    spawner: impl FnMut(&'static str, WorkerTask) -> std::io::Result<WorkerThread>,
) -> Result<Handle, AgenticSpawnFailure> {
    spawn_agentic_with_worker_spawner(
        ShellHandoff {
            engine: Arc::new(FakeEngine::default()),
            store: Arc::new(FakeStore::default()),
            blocker: Arc::new(ImmediateAllowAllCompiler),
            agent_lifecycle: PendingAgentBrowserLifecycle(agent_lifecycle),
            terminal_failure: Box::new(|_| {}),
            chrome: Arc::new(FakeChrome),
            emit: Box::new(|_| {}),
        },
        spawner,
    )
}

fn test_shutdown_deadline() -> std::time::Instant {
    std::time::Instant::now() + END_TO_END_SHUTDOWN_TIMEOUT
}

#[test]
fn every_worker_spawn_refusal_is_reported_after_bounded_cleanup() {
    for target in ["zephium-store-reader", "zephium-shell", "zephium-timer"] {
        let failure = match spawn_with_test_workers(|name, task| {
            if name == target {
                drop(task);
                Err(std::io::Error::other("injected worker refusal"))
            } else {
                spawn_worker(name, task)
            }
        }) {
            Ok(_) => panic!("the selected worker spawn must be refused"),
            Err(failure) => failure,
        };
        assert!(failure.worker_cleanup_proven(), "{target} cleanup");
        assert!(matches!(
            (target, failure.error()),
            ("zephium-store-reader", SpawnError::StoreReader(_))
                | ("zephium-shell", SpawnError::Actor(_))
                | ("zephium-timer", SpawnError::Timer(_))
        ));
    }
}

#[test]
fn disconnected_actor_handoff_is_reported_after_bounded_cleanup() {
    let failure = match spawn_with_test_workers(|name, task| {
        if name == "zephium-shell" {
            // Drop the real waiter (and therefore the one-shot receiver), but
            // return a successfully-created worker to exercise handoff
            // recovery rather than the actor-spawn refusal path.
            drop(task);
            spawn_worker(name, Box::new(|| {}))
        } else {
            spawn_worker(name, task)
        }
    }) {
        Ok(_) => panic!("the disconnected one-shot receiver must refuse Shell"),
        Err(failure) => failure,
    };

    assert!(failure.worker_cleanup_proven());
    assert!(matches!(failure.error(), SpawnError::ActorHandoff(_)));
}

#[cfg(feature = "agentic-browser")]
#[test]
fn every_agentic_worker_refusal_returns_the_agent_lifecycle_losslessly() {
    for target in ["zephium-store-reader", "zephium-shell", "zephium-timer"] {
        let (agent_lifecycle, agent_probe) = agent_lifecycle_probe();
        let failure = match spawn_agentic_with_test_workers(agent_lifecycle, |name, task| {
            if name == target {
                drop(task);
                Err(std::io::Error::other("injected agentic worker refusal"))
            } else {
                spawn_worker(name, task)
            }
        }) {
            Ok(_) => panic!("the selected agentic worker spawn must be refused"),
            Err(failure) => failure,
        };
        assert!(failure.worker_cleanup_proven(), "{target} cleanup");
        assert_eq!(
            agent_probe
                .shutdown_calls
                .load(std::sync::atomic::Ordering::Acquire),
            0
        );
        assert!(!agent_probe
            .dropped_without_shutdown
            .load(std::sync::atomic::Ordering::Acquire));

        let (error, agent_lifecycle) = failure.into_parts();
        assert!(matches!(
            (target, error),
            ("zephium-store-reader", SpawnError::StoreReader(_))
                | ("zephium-shell", SpawnError::Actor(_))
                | ("zephium-timer", SpawnError::Timer(_))
        ));
        assert!(matches!(
            agent_lifecycle.shutdown_until(test_shutdown_deadline()),
            zephium_agentic::AgentBrowserShutdownOutcome::Unclean
        ));
        assert_eq!(
            agent_probe
                .shutdown_calls
                .load(std::sync::atomic::Ordering::Acquire),
            1
        );
    }
}

#[cfg(feature = "agentic-browser")]
#[test]
fn disconnected_agentic_handoff_returns_the_agent_lifecycle_losslessly() {
    let (agent_lifecycle, agent_probe) = agent_lifecycle_probe();
    let failure = match spawn_agentic_with_test_workers(agent_lifecycle, |name, task| {
        if name == "zephium-shell" {
            drop(task);
            spawn_worker(name, Box::new(|| {}))
        } else {
            spawn_worker(name, task)
        }
    }) {
        Ok(_) => panic!("the disconnected agentic handoff must be refused"),
        Err(failure) => failure,
    };

    assert!(failure.worker_cleanup_proven());
    assert!(matches!(failure.error(), SpawnError::ActorHandoff(_)));
    assert_eq!(
        agent_probe
            .shutdown_calls
            .load(std::sync::atomic::Ordering::Acquire),
        0
    );
    let (error, agent_lifecycle) = failure.into_parts();
    assert!(matches!(error, SpawnError::ActorHandoff(_)));
    assert!(matches!(
        agent_lifecycle.shutdown_until(test_shutdown_deadline()),
        zephium_agentic::AgentBrowserShutdownOutcome::Unclean
    ));
    assert_eq!(
        agent_probe
            .shutdown_calls
            .load(std::sync::atomic::Ordering::Acquire),
        1
    );
}

#[cfg(feature = "agentic-browser")]
#[test]
fn last_handle_exit_consumes_the_agent_lifecycle_once() {
    let (agent_lifecycle, agent_probe) = agent_lifecycle_probe();
    let handle = spawn_agentic_with_test_workers(agent_lifecycle, spawn_worker)
        .expect("spawn agentic test shell");
    drop(handle);

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while agent_probe
        .shutdown_calls
        .load(std::sync::atomic::Ordering::Acquire)
        == 0
        && std::time::Instant::now() < deadline
    {
        std::thread::yield_now();
    }
    assert_eq!(
        agent_probe
            .shutdown_calls
            .load(std::sync::atomic::Ordering::Acquire),
        1
    );
    assert!(!agent_probe
        .dropped_without_shutdown
        .load(std::sync::atomic::Ordering::Acquire));
}

#[cfg(feature = "agentic-browser")]
#[test]
fn public_suspended_agentic_spawn_transfers_and_consumes_the_agent_lifecycle() {
    let (agent_lifecycle, agent_probe) = agent_lifecycle_probe();
    let handle = spawn_agentic_suspended(
        Arc::new(FakeEngine::default()),
        Arc::new(FakeStore::default()),
        Arc::new(ImmediateAllowAllCompiler),
        agent_lifecycle,
        Box::new(|_| {}),
        Arc::new(FakeChrome),
        Box::new(|_| {}),
    )
    .expect("spawn suspended agentic Shell");

    assert_eq!(
        handle.shutdown().recv().unwrap(),
        ShutdownOutcome::Unclean,
        "the fake agent lifecycle deliberately cannot mint a clean proof"
    );
    assert_eq!(
        agent_probe
            .shutdown_calls
            .load(std::sync::atomic::Ordering::Acquire),
        1
    );
}

#[test]
fn divider_release_is_a_tracked_operation() {
    assert!(tracked_operation_command(&Command::DividerRelease {
        x: None,
        y: None,
    }));
}

#[test]
fn extension_action_is_a_tracked_operation() {
    assert!(tracked_operation_command(&Command::InvokeExtensionAction {
        runtime: zephium_core::extensions::ExtensionRuntimeInstance::new(
            ProfileId::from(29),
            zephium_core::ids::ExtensionInstallId::from(31),
            zephium_core::extensions::ExtensionRuntimeGeneration::INITIAL,
        ),
        revision: zephium_core::extensions::ExtensionActionRevision::INITIAL,
        anchor: zephium_core::extensions::ExtensionPopupAnchor::new(
            zephium_core::geometry::Rect::new(8.0, 12.0, 28.0, 28.0),
        )
        .unwrap(),
    }));
}

#[test]
fn exact_content_policy_retry_is_a_tracked_operation() {
    assert!(tracked_operation_command(&Command::RetryContentPolicy {
        profile: ProfileId::from(17),
        failed_generation: ContentPolicyGeneration::new(9).unwrap(),
    }));
    assert!(tracked_operation_command(
        &Command::RetryFocusedContentPolicy {
            failed_generation: ContentPolicyGeneration::new(9).unwrap(),
        }
    ));
    assert!(tracked_operation_command(
        &Command::SetFocusedContentBlockerEnabled(true)
    ));
    assert!(tracked_operation_command(
        &Command::RefreshContentBlockerSources
    ));
}

#[test]
fn content_policy_status_query_is_ordered_and_fails_boundedly_when_sealed() {
    let profile = ProfileId::from(23);
    let queue = CommandQueue::new();
    let handle = Handle::new(queue.clone());

    let request = handle.content_policy_status(profile);
    let Command::ContentPolicyStatus {
        profile: requested,
        reply,
    } = queue.try_recv().expect("query is admitted")
    else {
        panic!("unexpected queued command");
    };
    assert_eq!(requested, profile);
    reply
        .send(ContentPolicyStatusQueryOutcome::UnknownProfile)
        .unwrap();
    assert_eq!(
        request.recv_timeout(std::time::Duration::from_millis(10)),
        ContentPolicyStatusQueryOutcome::UnknownProfile
    );

    let _shutdown = handle.shutdown();
    let rejected = handle.content_policy_status(profile);
    assert_eq!(
        rejected.recv_timeout(std::time::Duration::from_millis(10)),
        ContentPolicyStatusQueryOutcome::Unavailable
    );
}

#[cfg(feature = "work-execution")]
#[test]
fn work_profile_query_is_selector_free_bounded_and_closed_on_shutdown() {
    let queue = CommandQueue::new();
    let handle = Handle::new(queue.clone());
    let request = handle.work_profile_binding();
    assert_eq!(request.try_recv(), None);
    let Command::WorkProfileBinding { reply } = queue.try_recv().unwrap() else {
        panic!("wrong query");
    };
    reply
        .send(crate::AgentWorkProfileReadiness::ProfileMissing)
        .unwrap();
    assert_eq!(
        request.try_recv(),
        Some(crate::AgentWorkProfileReadiness::ProfileMissing)
    );
    let _shutdown = handle.shutdown();
    assert_eq!(
        handle.work_profile_binding().try_recv(),
        Some(crate::AgentWorkProfileReadiness::Unavailable)
    );
}

#[test]
fn focused_content_policy_query_has_no_profile_selector_and_fails_with_revision_zero() {
    let queue = CommandQueue::new();
    let handle = Handle::new(queue.clone());

    let request = handle.focused_content_policy_status();
    let Command::FocusedContentPolicyStatus { reply } =
        queue.try_recv().expect("focused query is admitted")
    else {
        panic!("unexpected queued command");
    };
    let mut status = BlockerStatusView::unavailable();
    status.projection_revision = "0000000000000000000000000000002a".into();
    reply.send(status.clone()).unwrap();
    assert_eq!(
        request.recv_timeout(std::time::Duration::from_millis(10)),
        status
    );

    let _shutdown = handle.shutdown();
    let rejected = handle.focused_content_policy_status();
    assert_eq!(
        rejected.recv_timeout(std::time::Duration::from_millis(10)),
        BlockerStatusView::unavailable()
    );
}

#[test]
fn failed_shutdown_replays_only_late_critical_callbacks() {
    let id = ItemId::from(7);
    let queue = CommandQueue::new();
    let handle = Handle::new(queue.clone());
    let _done = handle.shutdown();

    assert!(!handle.dispatch(Command::Engine(EngineEvent::Crashed { id })));
    assert!(!handle.dispatch(Command::Engine(EngineEvent::UrlChanged {
        id,
        url: "https://old.example/".into(),
    })));
    assert!(!handle.dispatch(Command::Engine(EngineEvent::UrlChanged {
        id,
        url: "https://latest.example/".into(),
    })));
    assert!(!handle.dispatch(Command::Open));
    assert!(matches!(queue.try_recv(), Some(Command::Shutdown { .. })));
    assert!(queue.try_recv().is_none());

    let recovered = queue.reopen_after_failed_shutdown();
    assert_eq!(recovered.len(), 2);
    assert!(matches!(
        &recovered[0],
        Command::Engine(EngineEvent::Crashed { id: recovered }) if *recovered == id
    ));
    assert!(matches!(
        &recovered[1],
        Command::Engine(EngineEvent::UrlChanged { id: recovered, url })
            if *recovered == id && url == "https://latest.example/"
    ));
    assert!(
        queue.try_recv().is_none(),
        "ordinary late work stays rejected"
    );
    assert!(handle.dispatch(Command::Open));
}

#[test]
fn last_public_handle_closes_actor_queue_and_wakes_ticker() {
    let queue = CommandQueue::new();
    let first = Handle::new(queue.clone());
    let last = first.clone();
    drop(first);

    let completion = last.shutdown();
    drop(last);

    assert!(matches!(
        queue.try_push(Command::Tick),
        Err(TryPushError::Closed(_))
    ));
    assert!(!queue.wait_for_tick(std::time::Duration::ZERO));
    let pending = queue.recv().expect("accepted shutdown remains ordered");
    finish_unprocessed_command(pending, ShutdownOutcome::Clean);
    assert_eq!(completion.recv().unwrap(), ShutdownOutcome::Clean);
    assert!(queue.recv().is_none());
}

#[test]
fn handle_count_overflow_seals_instead_of_aborting_or_underflowing() {
    let queue = CommandQueue::new();
    let handle = Handle::new(queue.clone());
    queue.inner.state.lock().unwrap().handles = usize::MAX;

    let uncounted = handle.clone();

    assert!(!uncounted.counted);
    assert!(matches!(
        queue.try_push(Command::Tick),
        Err(TryPushError::Closed(_))
    ));
    drop(uncounted);
    drop(handle);
}

#[test]
fn unexpected_handle_release_seals_without_panicking() {
    let queue = CommandQueue::new();

    let _ = queue.release_handle();

    assert!(matches!(
        queue.try_push(Command::Tick),
        Err(TryPushError::Closed(_))
    ));
    assert!(!queue.wait_for_tick(std::time::Duration::ZERO));
}

#[test]
fn terminal_cancellation_can_overtake_unconsumed_startup_admission() {
    let gate = ActorStartupGate::new();

    assert!(gate.admit());
    assert!(gate.cancel());
    assert!(!gate.wait_for_admission());
    assert!(!gate.admit());
}

#[test]
fn compatibility_startup_commit_is_irrevocable() {
    let gate = ActorStartupGate::new();

    assert!(gate.admit());
    assert!(gate.commit_admission());
    assert!(!gate.cancel());
    assert!(gate.wait_for_admission());
}

#[test]
fn dependency_callback_handle_is_weak_and_never_keeps_actor_open() {
    let queue = CommandQueue::new();
    let handle = Handle::new(queue.clone());
    let callback = handle.callback_handle();
    assert!(callback.dispatch(Command::Tick));
    assert!(queue.try_recv().is_some());

    drop(handle);
    assert!(!callback.dispatch(Command::Tick));
    drop(queue);
    assert!(!callback.dispatch(Command::Tick));
}

#[test]
fn actor_exit_guard_makes_a_pending_shutdown_terminal() {
    let queue = CommandQueue::new();
    let (ack, completion) = sync_channel(1);
    queue
        .try_push(Command::Shutdown {
            deadline: test_shutdown_deadline(),
            ack,
        })
        .ok()
        .unwrap();

    {
        let _guard = ActorExitGuard(queue.clone());
    }

    assert_eq!(completion.recv().unwrap(), ShutdownOutcome::Unclean);
    assert!(matches!(
        queue.try_push(Command::Tick),
        Err(TryPushError::Closed(_))
    ));
}

#[test]
fn shutdown_on_a_permanently_closed_actor_is_terminal() {
    let queue = CommandQueue::new();
    let handle = Handle::new(queue.clone());
    let drained = queue.close_and_drain();
    assert!(drained.is_empty());

    assert_eq!(handle.shutdown().recv().unwrap(), ShutdownOutcome::Unclean);
}

#[test]
fn composition_shutdown_preserves_an_earlier_deadline_and_clamps_a_later_one() {
    let earlier = std::time::Instant::now() + std::time::Duration::from_millis(5);
    let queue = CommandQueue::new();
    let handle = Handle::new(queue);
    let request = handle.shutdown_with_deadline(earlier);
    assert_eq!(request.deadline(), earlier);
    drop(request);
    drop(handle);

    let too_late = std::time::Instant::now() + std::time::Duration::from_secs(1);
    let queue = CommandQueue::new();
    let handle = Handle::new(queue);
    let request = handle.shutdown_with_deadline(too_late);
    assert!(request.deadline() < too_late);
}

#[test]
fn browser_pages_cross_the_tracked_operation_admission_boundary() {
    let queue = CommandQueue::new();
    let handle = Handle::new(queue.clone());
    for (index, page) in [
        Some(crate::BrowserPage::Settings),
        Some(crate::BrowserPage::History),
        Some(crate::BrowserPage::Downloads),
        Some(crate::BrowserPage::Work),
        None,
    ]
    .into_iter()
    .enumerate()
    {
        let operation_id = format!("{index:016x}");
        assert!(handle.dispatch_operation(operation_id.clone(), Command::ShowBrowserPage(page)));
        let Command::Operation {
            operation_id: admitted,
            command,
        } = queue.try_recv().expect("admitted browser request")
        else {
            panic!("expected tracked operation");
        };
        assert_eq!(admitted, operation_id);
        assert!(matches!(*command,Command::ShowBrowserPage(actual) if actual == page));
    }
    assert!(!handle.dispatch_operation(
        "0000000000000004".into(),
        Command::BrowserChromeRestored {
            revision: 1,
            applied: true
        }
    ));
    assert!(queue.try_recv().is_none());
}

#[test]
fn essentials_cross_the_tracked_operation_admission_boundary() {
    let queue = CommandQueue::new();
    let handle = Handle::new(queue.clone());
    let id = ItemId::from(41_u128);
    let before = Some(ItemId::from(42_u128));
    for essential in [true, false] {
        assert!(handle.dispatch_operation(
            "0000000000000001".into(),
            Command::SetTabEssential {
                id,
                essential,
                before
            }
        ));
        let Command::Operation { command, .. } = queue.try_recv().expect("admitted move") else {
            panic!("expected tracked operation");
        };
        assert!(
            matches!(*command,Command::SetTabEssential {id: actual,essential: actual_essential,before: actual_before} if actual == id && actual_essential == essential && actual_before == before)
        );
    }
}

#[test]
fn a_brief_os_focus_loss_is_not_coalesced_away_by_focus_return() {
    let queue = CommandQueue::new();
    assert!(queue.try_push(Command::SetWindowFocused(false)).is_ok());
    assert!(queue.try_push(Command::SetWindowFocused(true)).is_ok());
    assert!(matches!(
        queue.try_recv(),
        Some(Command::SetWindowFocused(false))
    ));
    assert!(matches!(
        queue.try_recv(),
        Some(Command::SetWindowFocused(true))
    ));
}

#[test]
fn scoped_launcher_actions_cross_the_real_operation_admission_boundary() {
    let queue = CommandQueue::new();
    let handle = Handle::new(queue.clone());
    let context = zephium_ipc::SearchContext {
        window_id: "window".into(),
        session_id: "0000000000000001".into(),
        request_id: "request-1".into(),
        profile_id: "p".into(),
        space_id: "s".into(),
    };
    let action = zephium_ipc::SearchAction::OpenUrl {
        url: "https://example.com/".into(),
    };
    assert!(handle.dispatch_operation(
        "0000000000000001".into(),
        Command::RunSearchAction {
            context: Box::new(context.clone()),
            action: action.clone(),
            background: false,
        }
    ));
    let Command::Operation { command, .. } = queue.try_recv().unwrap() else {
        panic!("tracked command required");
    };
    assert!(
        matches!(*command,Command::RunSearchAction{context:actual,action:actual_action,..} if *actual==context&&actual_action==action)
    );
    assert!(!handle.dispatch_operation(
        "0000000000000002".into(),
        Command::CancelSearch {
            session_id: context.session_id
        }
    ));
}
