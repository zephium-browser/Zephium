use super::*;

#[test]
fn engine_events_fold_into_projection() {
    let (mut shell, engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    let id = active_id(&screen);
    shell.handle(Command::Navigate {
        id,
        input: "example.com".into(),
    });

    shell.handle(Command::Engine(EngineEvent::TitleChanged {
        id,
        title: "Example".into(),
    }));
    shell.handle(Command::Engine(EngineEvent::LoadingChanged {
        id,
        loading: false,
    }));

    let key = id.to_string();
    let tab = last(&screen)
        .tabs
        .into_iter()
        .find(|t| t.id == key)
        .unwrap();
    assert_eq!(tab.title, "Example");
    assert!(!tab.loading);
    assert_eq!(engine.warm_spare_calls(), 1);
}

#[test]
fn unknown_or_retired_loading_event_cannot_warm_a_renderer() {
    let (mut shell, engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    let active = active_id(&screen);
    let unknown = ItemId::from(81_001);

    shell.handle(Command::Engine(EngineEvent::LoadingChanged {
        id: unknown,
        loading: false,
    }));
    assert_eq!(engine.warm_spare_calls(), 0);

    shell.items.view_creation_failed(active);
    shell.handle(Command::Engine(EngineEvent::LoadingChanged {
        id: active,
        loading: false,
    }));
    assert_eq!(engine.warm_spare_calls(), 0);
}

#[test]
fn runtime_restart_requirement_is_sticky_deduplicated_and_replayed_on_bootstrap() {
    let statuses = Arc::new(Mutex::new(Vec::new()));
    let sink = statuses.clone();
    let mut shell = Shell::new(
        Arc::new(FakeEngine::default()),
        Arc::new(FakeStore::default()),
        Arc::new(FakeChrome),
        Box::new(move |projection| {
            if let Projection::RuntimeStatus(status) = projection {
                sink.lock().unwrap().push(status.restart_required);
            }
        }),
    );
    shell.handle(Command::SetWindowSize(Size::new(1200.0, 800.0)));
    shell.handle(Command::Bootstrap);
    shell.handle(Command::Engine(EngineEvent::RuntimeRestartRequired));
    shell.handle(Command::Engine(EngineEvent::RuntimeRestartRequired));
    shell.handle(Command::Bootstrap);

    assert_eq!(*statuses.lock().unwrap(), vec![false, true, true]);
    assert!(shell.runtime_restart_required);
}

#[test]
fn user_content_failure_is_projected_and_only_a_newer_success_clears_it() {
    let statuses = Arc::new(Mutex::new(Vec::new()));
    let sink = statuses.clone();
    let mut shell = Shell::new(
        Arc::new(FakeEngine::default()),
        Arc::new(FakeStore::default()),
        Arc::new(FakeChrome),
        Box::new(move |projection| {
            if let Projection::RuntimeStatus(status) = projection {
                sink.lock()
                    .unwrap()
                    .push(status.user_content_degraded_scope_count);
            }
        }),
    );
    shell.handle(Command::SetWindowSize(Size::new(1200.0, 800.0)));
    shell.handle(Command::Bootstrap);
    let profile = shell.profiles.iter().next().unwrap().id;
    let first = zephium_core::ports::engine::UserContentGeneration::new(1).unwrap();
    let second = zephium_core::ports::engine::UserContentGeneration::new(2).unwrap();
    let failure = zephium_core::ports::engine::UserContentSettlement::Unavailable {
        failure: zephium_core::ports::engine::UserContentApplyFailure::NativeInstallation,
    };

    shell.handle(Command::Engine(EngineEvent::UserContentSettled {
        scope: ContentScope::Profile(ProfileId::from(u128::MAX)),
        requested: first,
        settlement: failure.clone(),
    }));
    shell.handle(Command::Engine(EngineEvent::UserContentSettled {
        scope: ContentScope::Profile(profile),
        requested: first,
        settlement: failure.clone(),
    }));
    shell.handle(Command::Engine(EngineEvent::UserContentSettled {
        scope: ContentScope::Profile(profile),
        requested: first,
        settlement: failure,
    }));
    shell.handle(Command::Engine(EngineEvent::UserContentSettled {
        scope: ContentScope::Profile(profile),
        requested: second,
        settlement: zephium_core::ports::engine::UserContentSettlement::Applied {
            generation: second,
        },
    }));

    assert_eq!(*statuses.lock().unwrap(), vec![0, 1, 0]);
}

#[test]
fn bootstrap_projects_every_sanitized_runtime_security_advisory() {
    let engine = Arc::new(FakeEngine::default());
    let mut advisories = zephium_core::runtime_security::RuntimeSecurityAdvisories::new();
    advisories.insert(
        zephium_core::runtime_security::RuntimeSecurityAdvisory::update_recommended(
            zephium_core::runtime_security::RuntimeSecurityUpdateTarget::OperatingSystem,
        ),
    );
    advisories.insert(zephium_core::runtime_security::RuntimeSecurityAdvisory::review_overdue());
    *engine.runtime_security_advisories.lock().unwrap() = advisories;
    let statuses = Arc::new(Mutex::new(Vec::new()));
    let sink = statuses.clone();
    let mut shell = Shell::new(
        engine,
        Arc::new(FakeStore::default()),
        Arc::new(FakeChrome),
        Box::new(move |projection| {
            if let Projection::RuntimeStatus(status) = projection {
                sink.lock().unwrap().push(status);
            }
        }),
    );
    shell.handle(Command::SetWindowSize(Size::new(1200.0, 800.0)));
    shell.handle(Command::Bootstrap);

    assert_eq!(
        statuses.lock().unwrap().as_slice(),
        &[RuntimeStatus {
            restart_required: false,
            session_set_aside: false,
            user_content_degraded_scope_count: 0,
            security_advisories: vec![
                RuntimeSecurityAdvisory {
                    kind: RuntimeSecurityAdvisoryKind::ReviewOverdue,
                    update_target: RuntimeSecurityUpdateTarget::Zephium,
                },
                RuntimeSecurityAdvisory {
                    kind: RuntimeSecurityAdvisoryKind::UpdateRecommended,
                    update_target: RuntimeSecurityUpdateTarget::OperatingSystem,
                },
            ],
        }]
    );
}

#[test]
fn maintenance_reconciles_a_runtime_event_lost_before_shell_admission() {
    let engine = Arc::new(FakeEngine::default());
    let statuses = Arc::new(Mutex::new(Vec::new()));
    let sink = statuses.clone();
    let mut shell = Shell::new(
        engine.clone(),
        Arc::new(FakeStore::default()),
        Arc::new(FakeChrome),
        Box::new(move |projection| {
            if let Projection::RuntimeStatus(status) = projection {
                sink.lock().unwrap().push(status.restart_required);
            }
        }),
    );
    shell.handle(Command::SetWindowSize(Size::new(1200.0, 800.0)));
    shell.handle(Command::Bootstrap);
    engine
        .runtime_restart_required
        .store(true, std::sync::atomic::Ordering::Release);

    shell.handle(Command::Tick);
    shell.handle(Command::Tick);

    assert_eq!(*statuses.lock().unwrap(), vec![false, true]);
}

#[test]
fn page_permission_completion_is_explicitly_denied_before_and_after_bootstrap() {
    use zephium_core::permissions::{
        PageOrigin, PagePermissionKind, PagePermissionRequest, PagePermissionRequestId,
        PagePermissionRequestKind, PagePermissionRequestSettlement,
    };

    let (mut shell, engine, _screen) = setup();
    let id = ItemId::from(90_001);
    let profile = ProfileId::from(90_002);
    let event = |request| EngineEvent::PermissionRequested {
        id,
        profile,
        request: PagePermissionRequest {
            id: PagePermissionRequestId::new(request).unwrap(),
            origin: PageOrigin::parse_exact("https://permissions.example").unwrap(),
            kind: PagePermissionRequestKind::Single(PagePermissionKind::Camera),
        },
    };

    shell.handle(Command::Engine(event(1)));
    shell.handle(Command::Bootstrap);
    shell.handle(Command::Engine(event(2)));

    assert_eq!(
        engine.page_permission_settlements(),
        vec![
            (
                profile,
                id,
                PagePermissionRequestId::new(1).unwrap(),
                PagePermissionRequestSettlement::Deny,
            ),
            (
                profile,
                id,
                PagePermissionRequestId::new(2).unwrap(),
                PagePermissionRequestSettlement::Deny,
            ),
        ]
    );
}
