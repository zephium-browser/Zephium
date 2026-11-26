pub(super) use super::loopback_support::{browser_settings, WorkflowResult};
use super::{
    loopback_support::{seeded_blocker, NoChrome},
    ProbeFailure,
};
use std::{
    io::Write as _,
    sync::{
        atomic::{AtomicBool, AtomicU8, Ordering},
        mpsc, Arc, Mutex,
    },
    time::{Duration, Instant},
};
use zephium_core::{
    ids::{ItemId, ProfileId, SpaceId},
    ports::store::Store,
    profiles::ProfileKind,
    session::{PersistedProfile, PersistedSpace, SessionState},
};
use zephium_work_composition::NativeWorkComposition;

pub(super) fn run_loopback_site() -> Result<(), ProbeFailure> {
    let data = tempfile::Builder::new()
        .prefix("zephium-windows-work-")
        .tempdir()
        .map_err(|_| ProbeFailure::Runtime)?;
    let store = Arc::new(super::windows_work_storage_fixture::open_store(
        data.path(),
    )?);
    let profile = ProfileId::generate();
    let space = SpaceId::generate();
    store.save_session(SessionState {
        profiles: vec![PersistedProfile {
            id: profile,
            name: "Windows Work qualification".into(),
            kind: ProfileKind::Default,
        }],
        spaces: vec![PersistedSpace {
            id: space,
            profile,
            name: "Loopback replicas".into(),
            look: Default::default(),
        }],
        active_space: Some(space),
        ..SessionState::default()
    });
    if !store.flush() {
        return Err(ProbeFailure::Runtime);
    }
    let blocker = seeded_blocker(data.path())?;
    let keys = (0..4)
        .map(|_| zephium_agentic::load_probe_openai_credential())
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| ProbeFailure::Keychain)?;
    let relay = Arc::new(Mutex::new(None::<zephium_app::CallbackHandle>));
    let events = relay.clone();
    let selected_tab = ItemId::generate();
    let selected_store_ready = Arc::new(AtomicU8::new(0));
    let selected_store_events = selected_store_ready.clone();
    let (send, receive) = mpsc::sync_channel(1);
    let fixture_failed = Arc::new(AtomicBool::new(false));
    let owner_store = store.clone();
    let timeout = Duration::from_secs(840);
    zephium_engine::run_windows_work_application_with_input_probe(
        profile, if super::work_site::needs_human_presentation() { zephium_engine::MacosWorkProbeInput::Human } else { zephium_engine::MacosWorkProbeInput::LifecycleOnly }, timeout + Duration::from_secs(30),
        move |event| {
            match &event {
                zephium_core::ports::engine::EngineEvent::UrlChanged { id, url }
                    if *id == selected_tab && url == "about:blank" => {
                        let _ = selected_store_events.compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire);
                    }
                zephium_core::ports::engine::EngineEvent::ViewCreationFailed { id }
                    if *id == selected_tab => selected_store_events.store(2, Ordering::Release),
                _ => {}
            }
            if let Ok(relay) = events.lock() { if let Some(handle) = relay.as_ref() { handle.dispatch(zephium_app::Command::Engine(event)); } }
        },
        move |engine| {
            let shell = zephium_app::spawn_suspended(engine.clone(), owner_store.clone(), blocker,
                Box::new(|_| {}), Arc::new(NoChrome), Box::new(|_| {})).map_err(|_| "loopback_shell")?;
            *relay.lock().map_err(|_| "loopback_events")? = Some(shell.callback_handle());
            super::work_site::install_presence(engine.clone());
            let selected_engine = engine.clone();
            let composition = NativeWorkComposition::new(engine, owner_store);
            if !shell.admit_startup() || !shell.dispatch(zephium_app::Command::Bootstrap) { return Err("loopback_startup"); }
            let worker_handle = shell.clone();
            let worker_failed = fixture_failed.clone();
            let worker = std::thread::Builder::new().name("windows-work-qualification".into()).stack_size(16 * 1024 * 1024)
                .spawn(move || {
                    let result = tokio::runtime::Builder::new_current_thread().enable_all().build().map_err(|_| "loopback_runtime")
                        .and_then(|runtime| runtime.block_on(async {
                            tokio::time::timeout(timeout, async {
                                // The product root gives the composition its public route; the
                                // fixture profile has no proxy of its own, so the system's is used.
                                let routes = zephium_agentic::public_asset::RouteSource::new(&tokio::runtime::Handle::current());
                                routes.set_app_proxy(None);
                                composition.set_public_routes(routes.clone());
                                routes.settled().await;
                                let binding = loop {
                                    let selected = worker_handle.work_profile_binding();
                                    let answer = loop {
                                        if let Some(answer) = selected.try_recv() { break answer; }
                                        tokio::time::sleep(Duration::from_millis(10)).await;
                                    };
                                    match answer {
                                        zephium_app::AgentWorkProfileReadiness::Ready(binding) if binding.profile() == profile => break binding,
                                        zephium_app::AgentWorkProfileReadiness::PolicyPending(_) |
                                        zephium_app::AgentWorkProfileReadiness::PolicyMissing |
                                        zephium_app::AgentWorkProfileReadiness::ProfileMissing => tokio::time::sleep(Duration::from_millis(10)).await,
                                        _ => return Err("loopback_profile"),
                                    }
                                };
                                // Product startup constructs an ordinary Browse tab. Establish the
                                // fresh selected native store through that same policy-gated path:
                                // missing profile binding is unknown, never proof of no session.
                                use zephium_core::ports::engine::Engine as _;
                                if !selected_engine.create_view(
                                    selected_tab,
                                    zephium_core::ports::engine::Partition::Persistent(profile),
                                    "about:blank",
                                    zephium_core::geometry::Rect::new(0.0, 0.0, 1.0, 1.0),
                                ) {
                                    return Err("loopback_selected_store_admission");
                                }
                                let selected_deadline = Instant::now() + Duration::from_secs(30);
                                loop {
                                    match selected_store_ready.load(Ordering::Acquire) {
                                        1 => break,
                                        2 => return Err("loopback_selected_store_construction"),
                                        _ if Instant::now() >= selected_deadline => return Err("loopback_selected_store_deadline"),
                                        _ => tokio::time::sleep(Duration::from_millis(10)).await,
                                    }
                                }
                                // Retire the initializer before querying presence. The host's FIFO
                                // dispatch orders physical close ahead of that query; no permanent
                                // Browse controller masks zero-owner generation rollover.
                                if selected_engine.close(selected_tab)
                                    != zephium_core::ports::engine::NativeDispatch::Scheduled
                                {
                                    return Err("loopback_selected_store_close_admission");
                                }
                                let _ = writeln!(std::io::stdout().lock(), "windows-work-probe: selected_store_native_empty_fixture_initialized=true initializer_close_admitted=true; content=redacted");
                                super::work_site::workflow(&worker_handle, &composition, profile, binding, keys).await
                            }).await.map_err(|_| "loopback_deadline")?
                        }));
                    worker_failed.store(match &result { Ok(workflow) => workflow.failure.is_some(), Err(_) => true }, Ordering::Release);
                    let _ = send.send(result);
                }).map_err(|_| "loopback_worker")?;
            let mut worker = Some(worker);
            let mut shutdown = None;
            Ok(Box::new(move |native_failed| {
                let finished = worker.as_ref().is_some_and(|join| join.is_finished());
                if finished && worker.take().is_none_or(|join| join.join().is_err()) { return Some(Err("loopback_worker_panic")); }
                if (native_failed || finished) && shutdown.is_none() {
                    let request = shell.shutdown_with_deadline(Instant::now() + Duration::from_secs(8));
                    shutdown = std::thread::Builder::new().name("windows-work-shutdown".into()).spawn(move || request.recv_until_deadline()).ok();
                    if shutdown.is_none() { return Some(Err("loopback_shutdown_thread")); }
                }
                if shutdown.as_ref().is_some_and(|join| join.is_finished()) {
                    let (clean, shutdown_fact) = match shutdown.take().map(|join| join.join()) {
                        Some(Ok(Ok(zephium_app::ShutdownOutcome::Clean))) => (true, "clean"),
                        Some(Ok(Ok(zephium_app::ShutdownOutcome::Unclean))) => (false, "unclean"),
                        Some(Ok(Ok(zephium_app::ShutdownOutcome::RetryableFailure))) => (false, "retryable_failure"),
                        Some(Ok(Err(std::sync::mpsc::RecvTimeoutError::Timeout))) => (false, "timeout"),
                        Some(Ok(Err(std::sync::mpsc::RecvTimeoutError::Disconnected))) => (false, "disconnected"),
                        Some(Err(_)) => (false, "join_panic"),
                        None => (false, "join_missing"),
                    };
                    let _ = writeln!(std::io::stdout().lock(),
                        "windows-work-probe: shell_shutdown={shutdown_fact} native_failed={native_failed}; content=redacted");
                    return Some(if !clean || native_failed { Err("loopback_shutdown") } else if fixture_failed.load(Ordering::Acquire) { Err("loopback_workflow_failed") } else { Ok(()) });
                }
                None
            }))
        }
    ).map_err(|reason| {
        let _ = writeln!(std::io::stderr().lock(), "windows-work-probe: host_failure={reason}; content=redacted");
        ProbeFailure::Runtime
    })?;
    let result = receive
        .recv_timeout(Duration::from_secs(1))
        .map_err(|_| ProbeFailure::Runtime)?
        .map_err(|reason| {
            let _ = writeln!(
                std::io::stderr().lock(),
                "windows-work-probe: workflow_failure={reason}; content=redacted"
            );
            ProbeFailure::Runtime
        })?;
    if result.failure.is_some() || result.state.work.profile != profile {
        return Err(ProbeFailure::Verification);
    }
    if store.shutdown_until(Instant::now() + Duration::from_secs(5))
        != zephium_core::ports::store::StoreShutdownOutcome::Clean
    {
        return Err(ProbeFailure::Runtime);
    }
    Ok(())
}
