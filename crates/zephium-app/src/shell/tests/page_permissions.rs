use super::*;

use zephium_core::permissions::{
    PageOrigin, PagePermissionCatalog, PagePermissionCatalogRevision, PagePermissionGrant,
    PagePermissionGrantRevision, PagePermissionKind, PagePermissionRequest,
    PagePermissionRequestId, PagePermissionRequestKind, PagePermissionRequestSettlement,
    RememberedPagePermission,
};

fn request(id: u64, kind: PagePermissionRequestKind) -> PagePermissionRequest {
    PagePermissionRequest {
        id: PagePermissionRequestId::new(id).unwrap(),
        origin: PageOrigin::parse_exact("https://media.example").unwrap(),
        kind,
    }
}

fn ready_shell(
    store: Arc<FakeStore>,
) -> (
    Shell,
    Arc<FakeEngine>,
    Screen,
    CommandQueue,
    ProfileId,
    ItemId,
) {
    let (mut shell, engine, screen) = setup_with(store);
    shell.page_permissions.remember_enabled = true;
    shell.handle(Command::Bootstrap);
    shell.handle(Command::SetWindowFocused(true));
    let item = active_id(&screen);
    navigate_and_commit(&mut shell, item, "https://media.example/call");
    let profile = shell.windows.focused().unwrap().profile;
    let queue = CommandQueue::new();
    shell.attach_queue(queue.clone());
    (shell, engine, screen, queue, profile, item)
}

fn empty_catalog() -> PagePermissionCatalog {
    PagePermissionCatalog::new(PagePermissionCatalogRevision::INITIAL, Vec::new()).unwrap()
}

#[test]
fn capture_observation_and_stop_are_bound_to_the_native_document() {
    use zephium_core::ports::engine::{CaptureDeviceState, MediaCaptureState};
    let (mut shell, engine, _screen, _queue, _profile, item) =
        ready_shell(Arc::new(FakeStore::default()));
    let navigation = shell.presentation.presented_navigations[&item].0;
    let state = MediaCaptureState {
        camera: CaptureDeviceState::Active,
        microphone: CaptureDeviceState::Muted,
    };
    assert!(shell.items.tab(item).unwrap().capture.is_none());
    shell.handle(Command::Engine(EngineEvent::MediaCaptureChanged {
        id: item,
        navigation,
        state,
    }));
    assert_eq!(
        shell.items.tab(item).unwrap().capture,
        Some((navigation, state))
    );
    let disposition = shell.operation_stop_media_capture(item, navigation);
    assert_eq!(disposition.outcome, OperationOutcome::Deferred);
    assert_eq!(
        *engine.media_capture_stops.lock().unwrap(),
        vec![(item, navigation)]
    );
    // Admission is not proof that capture stopped. Only native state clears it.
    assert!(shell.items.tab(item).unwrap().capture.is_some());
    shell.handle(Command::Engine(EngineEvent::MediaCaptureChanged {
        id: item,
        navigation,
        state: MediaCaptureState::default(),
    }));
    assert!(shell.items.tab(item).unwrap().capture.is_none());
}

#[test]
fn old_document_capture_cannot_reappear_or_stop_the_replacement() {
    use zephium_core::ports::engine::{CaptureDeviceState, MediaCaptureState};
    let (mut shell, engine, _screen, _queue, _profile, item) =
        ready_shell(Arc::new(FakeStore::default()));
    let old = shell.presentation.presented_navigations[&item].0;
    let state = MediaCaptureState {
        camera: CaptureDeviceState::Active,
        microphone: CaptureDeviceState::None,
    };
    shell.handle(Command::Engine(EngineEvent::MediaCaptureChanged {
        id: item,
        navigation: old,
        state,
    }));
    navigate_and_commit(&mut shell, item, "https://replacement.example/");
    let replacement = shell.presentation.presented_navigations[&item].0;
    // Native sampling after definitive commit, rather than optimistic clearing.
    shell.handle(Command::Engine(EngineEvent::MediaCaptureChanged {
        id: item,
        navigation: replacement,
        state: MediaCaptureState::default(),
    }));
    shell.handle(Command::Engine(EngineEvent::MediaCaptureChanged {
        id: item,
        navigation: old,
        state,
    }));
    assert!(shell.items.tab(item).unwrap().capture.is_none());
    assert_eq!(
        shell.operation_stop_media_capture(item, old).reason,
        OperationReason::InvalidScope
    );
    assert!(engine.media_capture_stops.lock().unwrap().is_empty());
}

#[test]
fn capture_does_not_disappear_while_navigation_is_provisional_or_failed() {
    use zephium_core::ports::engine::{CaptureDeviceState, MediaCaptureState};
    let (mut shell, engine, _screen, _queue, _profile, item) =
        ready_shell(Arc::new(FakeStore::default()));
    let navigation = shell.presentation.presented_navigations[&item].0;
    let state = MediaCaptureState {
        camera: CaptureDeviceState::Active,
        microphone: CaptureDeviceState::None,
    };
    shell.handle(Command::Engine(EngineEvent::MediaCaptureChanged {
        id: item,
        navigation,
        state,
    }));
    shell.handle(Command::Navigate {
        id: item,
        input: "https://next.example/".into(),
    });
    shell.handle(Command::Engine(EngineEvent::LoadingChanged {
        id: item,
        loading: true,
    }));
    assert_eq!(
        shell.items.tab(item).unwrap().capture,
        Some((navigation, state))
    );
    shell.handle(Command::Engine(EngineEvent::NavigationFailed {
        id: item,
        request: *engine.navigation_requests.lock().unwrap().last().unwrap(),
    }));
    assert_eq!(
        shell.items.tab(item).unwrap().capture,
        Some((navigation, state))
    );
    assert_eq!(
        shell.operation_stop_media_capture(item, navigation).outcome,
        OperationOutcome::Deferred
    );
    shell.handle(Command::Engine(EngineEvent::MediaCaptureChanged {
        id: item,
        navigation,
        state: MediaCaptureState::default(),
    }));
    assert!(shell.items.tab(item).unwrap().capture.is_none());
}

#[test]
fn os_focus_loss_cancels_consent_without_hiding_or_suspending_content() {
    let store = Arc::new(FakeStore::default());
    let (mut shell, engine, _screen, _queue, profile, item) = ready_shell(store);
    shell.page_permissions.remember_enabled = false;
    let first = request(101, PagePermissionRequestKind::CameraAndMicrophone);
    shell.handle(Command::Engine(EngineEvent::PermissionRequested {
        id: item,
        profile,
        request: first.clone(),
    }));
    assert!(shell.page_permissions.is_visible());

    shell.handle(Command::SetWindowFocused(false));
    assert!(shell.window_visible);
    assert!(shell.items.tab(item).unwrap().has_view());
    assert!(!shell.page_permissions.has_pending());
    shell.handle(Command::Operation {
        operation_id: "stale-after-blur".into(),
        command: Box::new(Command::RespondToPagePermissionPrompt {
            profile,
            item,
            request: first.id,
            decision: PagePermissionPromptDecision::AllowOnce,
        }),
    });
    assert_eq!(
        engine.page_permission_settlements(),
        vec![(
            profile,
            item,
            first.id,
            PagePermissionRequestSettlement::Deny,
        )]
    );
}

#[test]
fn visible_unfocused_window_denies_requests_until_os_focus_returns() {
    let store = Arc::new(FakeStore::default());
    let (mut shell, engine, _screen, _queue, profile, item) = ready_shell(store.clone());
    shell.page_permissions.remember_enabled = false;
    shell.handle(Command::SetWindowFocused(false));
    let first = request(
        102,
        PagePermissionRequestKind::Single(PagePermissionKind::Camera),
    );
    shell.handle(Command::Engine(EngineEvent::PermissionRequested {
        id: item,
        profile,
        request: first.clone(),
    }));
    assert!(shell.window_visible);
    assert!(!shell.page_permissions.has_pending());
    assert!(store.page_permission_load_calls.lock().unwrap().is_empty());
    assert_eq!(
        engine.page_permission_settlements(),
        vec![(
            profile,
            item,
            first.id,
            PagePermissionRequestSettlement::Deny,
        )]
    );

    shell.handle(Command::SetWindowFocused(true));
    shell.handle(Command::Engine(EngineEvent::PermissionRequested {
        id: item,
        profile,
        request: request(
            103,
            PagePermissionRequestKind::Single(PagePermissionKind::Microphone),
        ),
    }));
    assert!(shell.page_permissions.is_visible());
}

fn applied_mutation_outcome() -> PagePermissionCatalogMutationOutcome {
    let application = empty_catalog()
        .apply_patch(
            &zephium_core::permissions::PagePermissionPatch::new(vec![
                zephium_core::permissions::PagePermissionChange::Create {
                    id: zephium_core::ids::PagePermissionGrantId::from(999),
                    origin: PageOrigin::parse_exact("https://irrelevant.example").unwrap(),
                    kind: PagePermissionKind::Camera,
                    decision: RememberedPagePermission::Allow,
                },
            ])
            .unwrap(),
        )
        .unwrap();
    let (catalog, results, _) = application.into_parts();
    PagePermissionCatalogMutationOutcome::Applied(
        zephium_core::ports::store::PagePermissionCatalogMutationApplied {
            catalog_revision: catalog.revision(),
            results,
        },
    )
}

#[test]
fn foreground_request_loads_on_demand_and_allow_once_never_mutates_store() {
    let store = Arc::new(FakeStore::default());
    store
        .page_permission_load_outcomes
        .lock()
        .unwrap()
        .push_back(PagePermissionCatalogLoadOutcome::Loaded(empty_catalog()));
    let (mut shell, engine, _screen, queue, profile, item) = ready_shell(store.clone());
    let request = request(
        11,
        PagePermissionRequestKind::Single(PagePermissionKind::Camera),
    );

    shell.handle(Command::Engine(EngineEvent::PermissionRequested {
        id: item,
        profile,
        request: request.clone(),
    }));
    assert!(engine.page_permission_settlements().is_empty());
    shell.handle(queue.recv().unwrap());
    assert_eq!(
        shell
            .page_permissions
            .visible()
            .map(|(_, _, request, processing)| (request.id, processing)),
        Some((request.id, false))
    );

    shell.handle(Command::Operation {
        operation_id: "allow-once".into(),
        command: Box::new(Command::RespondToPagePermissionPrompt {
            profile,
            item,
            request: request.id,
            decision: PagePermissionPromptDecision::AllowOnce,
        }),
    });

    assert!(shell.page_permissions.visible().is_none());
    assert!(store
        .page_permission_mutation_calls
        .lock()
        .unwrap()
        .is_empty());
    assert_eq!(
        engine.page_permission_settlements(),
        vec![(
            profile,
            item,
            request.id,
            PagePermissionRequestSettlement::Allow,
        )]
    );
}

#[test]
fn incognito_prompts_once_without_reading_or_writing_durable_policy() {
    let store = Arc::new(FakeStore::default());
    let (mut shell, engine, _screen, _queue, profile, item) = ready_shell(store.clone());
    let mut ephemeral = shell.profiles.remove(profile).unwrap();
    ephemeral.kind = ProfileKind::Incognito;
    assert!(shell.profiles.insert(ephemeral));
    let request = request(
        15,
        PagePermissionRequestKind::Single(PagePermissionKind::Camera),
    );

    shell.handle(Command::Engine(EngineEvent::PermissionRequested {
        id: item,
        profile,
        request: request.clone(),
    }));

    assert!(store.page_permission_load_calls.lock().unwrap().is_empty());
    assert!(shell.page_permissions.is_visible());
    assert!(!shell.page_permissions.visible_rememberable());
    shell.handle(Command::Operation {
        operation_id: "forged-incognito-remember".into(),
        command: Box::new(Command::RespondToPagePermissionPrompt {
            profile,
            item,
            request: request.id,
            decision: PagePermissionPromptDecision::AlwaysAllow,
        }),
    });
    assert!(shell.page_permissions.is_visible());
    assert!(engine.page_permission_settlements().is_empty());
    shell.handle(Command::Operation {
        operation_id: "incognito-once".into(),
        command: Box::new(Command::RespondToPagePermissionPrompt {
            profile,
            item,
            request: request.id,
            decision: PagePermissionPromptDecision::AllowOnce,
        }),
    });
    assert!(store
        .page_permission_mutation_calls
        .lock()
        .unwrap()
        .is_empty());
    assert_eq!(
        engine.page_permission_settlements(),
        vec![(
            profile,
            item,
            request.id,
            PagePermissionRequestSettlement::Allow,
        )]
    );
}

#[test]
fn release_profiles_decide_once_without_reading_or_writing_durable_policy() {
    let store = Arc::new(FakeStore::default());
    let (mut shell, engine, _screen, _queue, profile, item) = ready_shell(store.clone());
    shell.page_permissions.remember_enabled = false;
    let request = request(16, PagePermissionRequestKind::CameraAndMicrophone);

    shell.handle(Command::Engine(EngineEvent::PermissionRequested {
        id: item,
        profile,
        request: request.clone(),
    }));

    assert!(store.page_permission_load_calls.lock().unwrap().is_empty());
    assert!(shell.page_permissions.is_visible());
    assert!(!shell.page_permissions.visible_rememberable());
    shell.handle(Command::Operation {
        operation_id: "forged-remember".into(),
        command: Box::new(Command::RespondToPagePermissionPrompt {
            profile,
            item,
            request: request.id,
            decision: PagePermissionPromptDecision::AlwaysAllow,
        }),
    });
    assert!(engine.page_permission_settlements().is_empty());
    shell.handle(Command::Operation {
        operation_id: "allow-once".into(),
        command: Box::new(Command::RespondToPagePermissionPrompt {
            profile,
            item,
            request: request.id,
            decision: PagePermissionPromptDecision::AllowOnce,
        }),
    });
    assert!(store
        .page_permission_mutation_calls
        .lock()
        .unwrap()
        .is_empty());
    assert_eq!(
        engine.page_permission_settlements(),
        vec![(
            profile,
            item,
            request.id,
            PagePermissionRequestSettlement::Allow,
        )]
    );
}

#[test]
fn remembered_allow_is_reobserved_before_native_allow() {
    let store = Arc::new(FakeStore::default());
    store
        .page_permission_load_outcomes
        .lock()
        .unwrap()
        .push_back(PagePermissionCatalogLoadOutcome::Loaded(empty_catalog()));
    store
        .page_permission_mutation_outcomes
        .lock()
        .unwrap()
        .push_back(applied_mutation_outcome());
    let (mut shell, engine, _screen, queue, profile, item) = ready_shell(store.clone());
    let request = request(12, PagePermissionRequestKind::CameraAndMicrophone);
    shell.handle(Command::Engine(EngineEvent::PermissionRequested {
        id: item,
        profile,
        request: request.clone(),
    }));
    shell.handle(queue.recv().unwrap());

    shell.handle(Command::Operation {
        operation_id: "always-allow".into(),
        command: Box::new(Command::RespondToPagePermissionPrompt {
            profile,
            item,
            request: request.id,
            decision: PagePermissionPromptDecision::AlwaysAllow,
        }),
    });
    assert!(engine.page_permission_settlements().is_empty());
    let mutation = store.page_permission_mutation_calls.lock().unwrap()[0]
        .2
        .clone();
    assert_eq!(mutation.changes().len(), 2);
    let grants = mutation
        .changes()
        .iter()
        .map(|change| match change {
            zephium_core::permissions::PagePermissionChange::Create {
                id,
                origin,
                kind,
                decision,
            } => PagePermissionGrant {
                id: *id,
                revision: PagePermissionGrantRevision::INITIAL,
                origin: origin.clone(),
                kind: *kind,
                decision: *decision,
            },
            _ => panic!("empty catalog must create both grants"),
        })
        .collect();
    store
        .page_permission_load_outcomes
        .lock()
        .unwrap()
        .push_back(PagePermissionCatalogLoadOutcome::Loaded(
            PagePermissionCatalog::new(PagePermissionCatalogRevision::new(2).unwrap(), grants)
                .unwrap(),
        ));

    shell.handle(queue.recv().unwrap());
    assert!(engine.page_permission_settlements().is_empty());
    shell.handle(queue.recv().unwrap());
    assert_eq!(
        engine.page_permission_settlements(),
        vec![(
            profile,
            item,
            request.id,
            PagePermissionRequestSettlement::Allow,
        )]
    );
}

#[test]
fn conflict_allows_only_when_exact_reload_observes_the_requested_policy() {
    let store = Arc::new(FakeStore::default());
    let origin = PageOrigin::parse_exact("https://media.example").unwrap();
    store.page_permission_load_outcomes.lock().unwrap().extend([
        PagePermissionCatalogLoadOutcome::Loaded(empty_catalog()),
        PagePermissionCatalogLoadOutcome::Loaded(
            PagePermissionCatalog::new(
                PagePermissionCatalogRevision::new(2).unwrap(),
                vec![PagePermissionGrant {
                    id: zephium_core::ids::PagePermissionGrantId::from(20),
                    revision: PagePermissionGrantRevision::INITIAL,
                    origin,
                    kind: PagePermissionKind::Camera,
                    decision: RememberedPagePermission::Allow,
                }],
            )
            .unwrap(),
        ),
    ]);
    store
        .page_permission_mutation_outcomes
        .lock()
        .unwrap()
        .push_back(PagePermissionCatalogMutationOutcome::Conflict {
            current: PagePermissionCatalogRevision::new(2).unwrap(),
        });
    let (mut shell, engine, _screen, queue, profile, item) = ready_shell(store);
    let request = request(
        16,
        PagePermissionRequestKind::Single(PagePermissionKind::Camera),
    );
    shell.handle(Command::Engine(EngineEvent::PermissionRequested {
        id: item,
        profile,
        request: request.clone(),
    }));
    shell.handle(queue.recv().unwrap());
    shell.handle(Command::Operation {
        operation_id: "conflict-observed".into(),
        command: Box::new(Command::RespondToPagePermissionPrompt {
            profile,
            item,
            request: request.id,
            decision: PagePermissionPromptDecision::AlwaysAllow,
        }),
    });
    shell.handle(queue.recv().unwrap());
    assert!(engine.page_permission_settlements().is_empty());
    shell.handle(queue.recv().unwrap());

    assert_eq!(
        engine.page_permission_settlements(),
        vec![(
            profile,
            item,
            request.id,
            PagePermissionRequestSettlement::Allow,
        )]
    );
    assert!(!shell.page_permissions.failed_until_restart());
}

#[test]
fn applied_mutation_with_contradictory_reload_fails_closed() {
    let store = Arc::new(FakeStore::default());
    store.page_permission_load_outcomes.lock().unwrap().extend([
        PagePermissionCatalogLoadOutcome::Loaded(empty_catalog()),
        PagePermissionCatalogLoadOutcome::Loaded(empty_catalog()),
    ]);
    store
        .page_permission_mutation_outcomes
        .lock()
        .unwrap()
        .push_back(applied_mutation_outcome());
    let (mut shell, engine, _screen, queue, profile, item) = ready_shell(store);
    let request = request(
        17,
        PagePermissionRequestKind::Single(PagePermissionKind::Microphone),
    );
    shell.handle(Command::Engine(EngineEvent::PermissionRequested {
        id: item,
        profile,
        request: request.clone(),
    }));
    shell.handle(queue.recv().unwrap());
    shell.handle(Command::Operation {
        operation_id: "contradictory-applied".into(),
        command: Box::new(Command::RespondToPagePermissionPrompt {
            profile,
            item,
            request: request.id,
            decision: PagePermissionPromptDecision::AlwaysAllow,
        }),
    });
    shell.handle(queue.recv().unwrap());
    shell.handle(queue.recv().unwrap());

    assert_eq!(
        engine.page_permission_settlements(),
        vec![(
            profile,
            item,
            request.id,
            PagePermissionRequestSettlement::Deny,
        )]
    );
    assert!(shell.page_permissions.failed_until_restart());
}

#[test]
fn combined_request_denies_when_either_durable_capability_is_denied() {
    let store = Arc::new(FakeStore::default());
    let origin = PageOrigin::parse_exact("https://media.example").unwrap();
    let catalog = PagePermissionCatalog::new(
        PagePermissionCatalogRevision::new(3).unwrap(),
        vec![PagePermissionGrant {
            id: zephium_core::ids::PagePermissionGrantId::from(1),
            revision: PagePermissionGrantRevision::INITIAL,
            origin,
            kind: PagePermissionKind::Camera,
            decision: RememberedPagePermission::Deny,
        }],
    )
    .unwrap();
    store
        .page_permission_load_outcomes
        .lock()
        .unwrap()
        .push_back(PagePermissionCatalogLoadOutcome::Loaded(catalog));
    let (mut shell, engine, _screen, queue, profile, item) = ready_shell(store);
    let request = request(13, PagePermissionRequestKind::CameraAndMicrophone);

    shell.handle(Command::Engine(EngineEvent::PermissionRequested {
        id: item,
        profile,
        request: request.clone(),
    }));
    shell.handle(queue.recv().unwrap());

    assert!(shell.page_permissions.visible().is_none());
    assert_eq!(
        engine.page_permission_settlements(),
        vec![(
            profile,
            item,
            request.id,
            PagePermissionRequestSettlement::Deny,
        )]
    );
}

#[test]
fn changing_active_tab_cancels_visible_request_without_resurrecting_a_view() {
    let store = Arc::new(FakeStore::default());
    store
        .page_permission_load_outcomes
        .lock()
        .unwrap()
        .push_back(PagePermissionCatalogLoadOutcome::Loaded(empty_catalog()));
    let (mut shell, engine, _screen, queue, profile, item) = ready_shell(store);
    let request = request(
        14,
        PagePermissionRequestKind::Single(PagePermissionKind::Microphone),
    );
    shell.handle(Command::Engine(EngineEvent::PermissionRequested {
        id: item,
        profile,
        request: request.clone(),
    }));
    shell.handle(queue.recv().unwrap());
    assert!(shell.page_permissions.is_visible());

    shell.handle(Command::Open);

    assert!(!shell.page_permissions.has_pending());
    assert_eq!(
        engine.page_permission_settlements(),
        vec![(
            profile,
            item,
            request.id,
            PagePermissionRequestSettlement::Deny,
        )]
    );
}

#[test]
fn background_request_denies_without_store_or_view_work() {
    let store = Arc::new(FakeStore::default());
    let (mut shell, engine, _screen, _queue, profile, background) = ready_shell(store.clone());
    shell.handle(Command::Open);
    let request = request(
        18,
        PagePermissionRequestKind::Single(PagePermissionKind::Camera),
    );

    shell.handle(Command::Engine(EngineEvent::PermissionRequested {
        id: background,
        profile,
        request: request.clone(),
    }));

    assert!(store.page_permission_load_calls.lock().unwrap().is_empty());
    assert!(!shell.page_permissions.has_pending());
    assert_eq!(
        engine.page_permission_settlements(),
        vec![(
            profile,
            background,
            request.id,
            PagePermissionRequestSettlement::Deny,
        )]
    );
}
