use super::*;

/// Counts the sidebar snapshots and tab deltas chrome is sent.
fn counted_shell() -> (Shell, Arc<Mutex<(usize, usize)>>) {
    let counts = Arc::new(Mutex::new((0, 0)));
    let sink = counts.clone();
    let mut shell = Shell::new(
        Arc::new(FakeEngine::default()),
        Arc::new(FakeStore::default()),
        Arc::new(FakeChrome),
        Box::new(move |projection| match projection {
            Projection::Items(_) => sink.lock().unwrap().0 += 1,
            Projection::Tab(_) => sink.lock().unwrap().1 += 1,
            _ => {}
        }),
    );
    shell.handle(Command::SetWindowSize(Size::new(1200.0, 800.0)));
    shell.handle(Command::Bootstrap);
    (shell, counts)
}

#[test]
fn a_page_repeating_its_title_costs_chrome_nothing() {
    let (mut shell, counts) = counted_shell();
    let id = shell.windows.focused().unwrap().active.unwrap();
    let title = |shell: &mut Shell, title: &str| {
        shell.handle(Command::Engine(EngineEvent::TitleChanged {
            id,
            title: title.into(),
        }))
    };
    title(&mut shell, "Inbox (3)");
    let after_first = counts.lock().unwrap().1;
    title(&mut shell, "Inbox (3)");
    title(&mut shell, "Inbox (3)");
    assert_eq!(counts.lock().unwrap().1, after_first);
    title(&mut shell, "Inbox (4)");
    assert_eq!(counts.lock().unwrap().1, after_first + 1);
}

#[test]
fn a_committed_action_that_changes_no_row_sends_no_snapshot_but_a_reload_gets_one() {
    let (mut shell, counts) = counted_shell();
    let id = shell.windows.focused().unwrap().active.unwrap();
    let before = counts.lock().unwrap().0;
    // Focusing the tab that is already focused commits, and changes nothing.
    shell.handle(Command::Activate(id));
    shell.handle(Command::Activate(id));
    assert_eq!(counts.lock().unwrap().0, before);
    // The chrome document reloaded: it asks again and is sent everything.
    shell.handle(Command::Bootstrap);
    assert_eq!(counts.lock().unwrap().0, before + 1);
    shell.handle(Command::Open);
    assert_eq!(counts.lock().unwrap().0, before + 2);
}

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
        navigation: zephium_core::ports::engine::NavigationPresentationId::from_raw(1),
        request: PagePermissionRequest {
            id: PagePermissionRequestId::new(request).unwrap(),
            origin: PageOrigin::parse_exact("https://permissions.example").unwrap(),
            kind: PagePermissionRequestKind::Single(PagePermissionKind::Camera),
            blocked_by_system: false,
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
