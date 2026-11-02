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
        blocked_by_system: false,
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
    shell.handle(Command::Bootstrap);
    shell.handle(Command::SetWindowFocused(true));
    let item = active_id(&screen);
    navigate_and_commit(&mut shell, item, "https://media.example/call");
    let profile = shell.windows.focused().unwrap().profile;
    let queue = CommandQueue::new();
    shell.attach_queue(queue.clone());
    (shell, engine, screen, queue, profile, item)
}

fn document(shell: &Shell, item: ItemId) -> NavigationPresentationId {
    shell.presentation.presented_navigations[&item].0
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
    store
        .page_permission_load_outcomes
        .lock()
        .unwrap()
        .push_back(PagePermissionCatalogLoadOutcome::Loaded(empty_catalog()));
    let (mut shell, engine, _screen, queue, profile, item) = ready_shell(store);
    let first = request(101, PagePermissionRequestKind::CameraAndMicrophone);
    shell.handle(Command::Engine(EngineEvent::PermissionRequested {
        id: item,
        profile,
        navigation: document(&shell, item),
        request: first.clone(),
    }));
    shell.handle(queue.recv().unwrap());
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
    let (mut shell, engine, _screen, queue, profile, item) = ready_shell(store.clone());
    shell.handle(Command::SetWindowFocused(false));
    let first = request(
        102,
        PagePermissionRequestKind::Single(PagePermissionKind::Camera),
    );
    shell.handle(Command::Engine(EngineEvent::PermissionRequested {
        id: item,
        profile,
        navigation: document(&shell, item),
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
    store
        .page_permission_load_outcomes
        .lock()
        .unwrap()
        .push_back(PagePermissionCatalogLoadOutcome::Loaded(empty_catalog()));
    shell.handle(Command::Engine(EngineEvent::PermissionRequested {
        id: item,
        profile,
        navigation: document(&shell, item),
        request: request(
            103,
            PagePermissionRequestKind::Single(PagePermissionKind::Microphone),
        ),
    }));
    shell.handle(queue.recv().unwrap());
    assert!(shell.page_permissions.is_visible());
}

pub(super) fn applied_mutation_outcome() -> PagePermissionCatalogMutationOutcome {
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
        navigation: document(&shell, item),
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
        navigation: document(&shell, item),
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
fn remembered_allow_settles_without_a_prompt() {
    let store = Arc::new(FakeStore::default());
    let origin = PageOrigin::parse_exact("https://media.example").unwrap();
    let grants = [PagePermissionKind::Camera, PagePermissionKind::Microphone]
        .into_iter()
        .zip(30..)
        .map(|(kind, id)| PagePermissionGrant {
            id: zephium_core::ids::PagePermissionGrantId::from(id),
            revision: PagePermissionGrantRevision::INITIAL,
            origin: origin.clone(),
            kind,
            decision: RememberedPagePermission::Allow,
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
    let (mut shell, engine, _screen, queue, profile, item) = ready_shell(store.clone());
    let request = request(19, PagePermissionRequestKind::CameraAndMicrophone);

    shell.handle(Command::Engine(EngineEvent::PermissionRequested {
        id: item,
        profile,
        navigation: document(&shell, item),
        request: request.clone(),
    }));
    shell.handle(queue.recv().unwrap());

    assert!(!shell.page_permissions.has_pending());
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
fn a_remembered_allow_for_one_capability_still_asks_for_the_pair() {
    let store = Arc::new(FakeStore::default());
    store
        .page_permission_load_outcomes
        .lock()
        .unwrap()
        .push_back(PagePermissionCatalogLoadOutcome::Loaded(
            PagePermissionCatalog::new(
                PagePermissionCatalogRevision::new(2).unwrap(),
                vec![PagePermissionGrant {
                    id: zephium_core::ids::PagePermissionGrantId::from(40),
                    revision: PagePermissionGrantRevision::INITIAL,
                    origin: PageOrigin::parse_exact("https://media.example").unwrap(),
                    kind: PagePermissionKind::Camera,
                    decision: RememberedPagePermission::Allow,
                }],
            )
            .unwrap(),
        ));
    let (mut shell, engine, _screen, queue, profile, item) = ready_shell(store);
    let request = request(20, PagePermissionRequestKind::CameraAndMicrophone);

    shell.handle(Command::Engine(EngineEvent::PermissionRequested {
        id: item,
        profile,
        navigation: document(&shell, item),
        request: request.clone(),
    }));
    shell.handle(queue.recv().unwrap());

    assert!(shell.page_permissions.is_visible());
    assert!(shell.page_permissions.visible_rememberable());
    assert!(engine.page_permission_settlements().is_empty());
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
        navigation: document(&shell, item),
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
        navigation: document(&shell, item),
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
        navigation: document(&shell, item),
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
        navigation: document(&shell, item),
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
        navigation: document(&shell, item),
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
        navigation: document(&shell, background),
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

#[test]
fn location_asks_through_the_same_prompt_and_honours_a_remembered_block() {
    let store = Arc::new(FakeStore::default());
    store.page_permission_load_outcomes.lock().unwrap().extend([
        PagePermissionCatalogLoadOutcome::Loaded(empty_catalog()),
        PagePermissionCatalogLoadOutcome::Loaded(
            PagePermissionCatalog::new(
                PagePermissionCatalogRevision::new(2).unwrap(),
                vec![PagePermissionGrant {
                    id: zephium_core::ids::PagePermissionGrantId::from(50),
                    revision: PagePermissionGrantRevision::INITIAL,
                    origin: PageOrigin::parse_exact("https://media.example").unwrap(),
                    kind: PagePermissionKind::Geolocation,
                    decision: RememberedPagePermission::Deny,
                }],
            )
            .unwrap(),
        ),
    ]);
    let (mut shell, engine, _screen, queue, profile, item) = ready_shell(store);
    let location = PagePermissionRequestKind::Single(PagePermissionKind::Geolocation);
    let first = request(21, location);
    shell.handle(Command::Engine(EngineEvent::PermissionRequested {
        id: item,
        profile,
        navigation: document(&shell, item),
        request: first.clone(),
    }));
    shell.handle(queue.recv().unwrap());
    assert!(shell.page_permissions.visible_rememberable());
    shell.handle(Command::Operation {
        operation_id: "location-once".into(),
        command: Box::new(Command::RespondToPagePermissionPrompt {
            profile,
            item,
            request: first.id,
            decision: PagePermissionPromptDecision::AllowOnce,
        }),
    });

    let second = request(22, location);
    shell.handle(Command::Engine(EngineEvent::PermissionRequested {
        id: item,
        profile,
        navigation: document(&shell, item),
        request: second.clone(),
    }));
    shell.handle(queue.recv().unwrap());
    assert!(!shell.page_permissions.has_pending());
    assert_eq!(
        engine.page_permission_settlements(),
        vec![
            (
                profile,
                item,
                first.id,
                PagePermissionRequestSettlement::Allow
            ),
            (
                profile,
                item,
                second.id,
                PagePermissionRequestSettlement::Deny
            ),
        ]
    );
}

fn ask(shell: &mut Shell, profile: ProfileId, item: ItemId, request: &PagePermissionRequest) {
    let navigation = document(shell, item);
    shell.handle(Command::Engine(EngineEvent::PermissionRequested {
        id: item,
        profile,
        navigation,
        request: request.clone(),
    }));
}

fn answer(
    shell: &mut Shell,
    profile: ProfileId,
    item: ItemId,
    request: &PagePermissionRequest,
    decision: PagePermissionPromptDecision,
) {
    shell.handle(Command::Operation {
        operation_id: format!("answer-{}", request.id.get()),
        command: Box::new(Command::RespondToPagePermissionPrompt {
            profile,
            item,
            request: request.id,
            decision,
        }),
    });
}

fn make_private(shell: &mut Shell, profile: ProfileId) {
    let mut ephemeral = shell.profiles.remove(profile).unwrap();
    ephemeral.kind = ProfileKind::Incognito;
    assert!(shell.profiles.insert(ephemeral));
}

fn camera(id: u64) -> PagePermissionRequest {
    request(
        id,
        PagePermissionRequestKind::Single(PagePermissionKind::Camera),
    )
}

fn settled(engine: &FakeEngine) -> Vec<(u64, PagePermissionRequestSettlement)> {
    engine
        .page_permission_settlements()
        .into_iter()
        .map(|(_, _, request, settlement)| (request.get(), settlement))
        .collect()
}

#[test]
fn allow_this_time_covers_later_requests_from_the_same_document() {
    use PagePermissionRequestSettlement::Allow;
    let store = Arc::new(FakeStore::default());
    store.page_permission_load_outcomes.lock().unwrap().extend([
        PagePermissionCatalogLoadOutcome::Loaded(empty_catalog()),
        PagePermissionCatalogLoadOutcome::Loaded(empty_catalog()),
        PagePermissionCatalogLoadOutcome::Loaded(empty_catalog()),
    ]);
    let (mut shell, engine, _screen, queue, profile, item) = ready_shell(store.clone());
    let first = request(201, PagePermissionRequestKind::CameraAndMicrophone);
    ask(&mut shell, profile, item, &first);
    shell.handle(queue.recv().unwrap());
    answer(
        &mut shell,
        profile,
        item,
        &first,
        PagePermissionPromptDecision::AllowOnce,
    );

    // The page asks again for one of the pair, then for the pair, after
    // changing its address in place.
    ask(&mut shell, profile, item, &camera(202));
    shell.handle(queue.recv().unwrap());
    shell.handle(Command::Engine(EngineEvent::UrlChanged {
        id: item,
        url: "https://media.example/call?joined".into(),
    }));
    ask(
        &mut shell,
        profile,
        item,
        &request(203, PagePermissionRequestKind::CameraAndMicrophone),
    );
    shell.handle(queue.recv().unwrap());

    assert!(!shell.page_permissions.has_pending());
    assert_eq!(settled(&engine), [(201, Allow), (202, Allow), (203, Allow)]);
    assert!(store
        .page_permission_mutation_calls
        .lock()
        .unwrap()
        .is_empty());
}

#[test]
fn the_asking_document_keeps_its_prompt_until_another_one_starts() {
    let store = Arc::new(FakeStore::default());
    store.page_permission_load_outcomes.lock().unwrap().extend([
        PagePermissionCatalogLoadOutcome::Loaded(empty_catalog()),
        PagePermissionCatalogLoadOutcome::Loaded(empty_catalog()),
    ]);
    let (mut shell, engine, _screen, queue, profile, item) = ready_shell(store);
    let first = camera(211);
    ask(&mut shell, profile, item, &first);
    shell.handle(queue.recv().unwrap());
    let navigation = document(&shell, item);
    shell.handle(Command::Engine(EngineEvent::UrlChanged {
        id: item,
        url: "https://media.example/call#lobby".into(),
    }));
    shell.handle(Command::Engine(EngineEvent::PresentationReady {
        id: item,
        navigation,
        url: "https://media.example/call".into(),
    }));
    assert!(shell.page_permissions.is_visible());
    answer(
        &mut shell,
        profile,
        item,
        &first,
        PagePermissionPromptDecision::AllowOnce,
    );

    // The tab starts loading another document: the engine has already
    // refused what that page was asking, and the prompt goes too.
    let second = request(
        212,
        PagePermissionRequestKind::Single(PagePermissionKind::Microphone),
    );
    ask(&mut shell, profile, item, &second);
    shell.handle(queue.recv().unwrap());
    assert!(shell.page_permissions.is_visible());
    shell.handle(Command::Engine(EngineEvent::LoadingChanged {
        id: item,
        loading: true,
    }));
    assert!(!shell.page_permissions.has_pending());
    assert_eq!(
        settled(&engine),
        [
            (211, PagePermissionRequestSettlement::Allow),
            (212, PagePermissionRequestSettlement::Deny),
        ]
    );
}

#[test]
fn a_one_time_answer_stays_with_its_tab_origin_and_document() {
    let store = Arc::new(FakeStore::default());
    let (mut shell, engine, screen, _queue, profile, item) = ready_shell(store.clone());
    make_private(&mut shell, profile);
    ask(&mut shell, profile, item, &camera(221));
    answer(
        &mut shell,
        profile,
        item,
        &camera(221),
        PagePermissionPromptDecision::AllowOnce,
    );

    // A frame from another origin in the same document.
    let other = PagePermissionRequest {
        origin: PageOrigin::parse_exact("https://embed.example").unwrap(),
        ..camera(222)
    };
    ask(&mut shell, profile, item, &other);
    assert!(shell.page_permissions.is_visible());
    answer(
        &mut shell,
        profile,
        item,
        &other,
        PagePermissionPromptDecision::Dismiss,
    );

    // The same site in another tab.
    shell.handle(Command::Open);
    let second = active_id(&screen);
    assert_ne!(second, item);
    navigate_and_commit(&mut shell, second, "https://media.example/call");
    ask(&mut shell, profile, second, &camera(223));
    assert!(shell.page_permissions.is_visible());
    answer(
        &mut shell,
        profile,
        second,
        &camera(223),
        PagePermissionPromptDecision::Dismiss,
    );

    // The first tab after it loads another document of the same site.
    shell.handle(Command::Activate(item));
    navigate_and_commit(&mut shell, item, "https://media.example/call");
    ask(&mut shell, profile, item, &camera(224));
    assert!(shell.page_permissions.is_visible());

    assert!(store.page_permission_load_calls.lock().unwrap().is_empty());
    assert_eq!(
        settled(&engine),
        [
            (221, PagePermissionRequestSettlement::Allow),
            (222, PagePermissionRequestSettlement::Deny),
            (223, PagePermissionRequestSettlement::Deny),
        ]
    );
}

#[test]
fn dont_allow_holds_for_the_document_but_a_dismissal_asks_again() {
    use PagePermissionRequestSettlement::Deny;
    let (mut shell, engine, _screen, _queue, profile, item) =
        ready_shell(Arc::new(FakeStore::default()));
    make_private(&mut shell, profile);
    ask(&mut shell, profile, item, &camera(231));
    answer(
        &mut shell,
        profile,
        item,
        &camera(231),
        PagePermissionPromptDecision::Dismiss,
    );
    ask(&mut shell, profile, item, &camera(232));
    assert!(shell.page_permissions.is_visible());
    answer(
        &mut shell,
        profile,
        item,
        &camera(232),
        PagePermissionPromptDecision::DenyOnce,
    );
    // A denied capability also refuses the pair it belongs to.
    ask(
        &mut shell,
        profile,
        item,
        &request(233, PagePermissionRequestKind::CameraAndMicrophone),
    );

    assert!(!shell.page_permissions.has_pending());
    assert_eq!(settled(&engine), [(231, Deny), (232, Deny), (233, Deny)]);
}

#[test]
fn a_saved_decision_outranks_a_one_time_answer() {
    let store = Arc::new(FakeStore::default());
    store.page_permission_load_outcomes.lock().unwrap().extend([
        PagePermissionCatalogLoadOutcome::Loaded(empty_catalog()),
        PagePermissionCatalogLoadOutcome::Loaded(
            PagePermissionCatalog::new(
                PagePermissionCatalogRevision::new(2).unwrap(),
                vec![PagePermissionGrant {
                    id: zephium_core::ids::PagePermissionGrantId::from(60),
                    revision: PagePermissionGrantRevision::INITIAL,
                    origin: PageOrigin::parse_exact("https://media.example").unwrap(),
                    kind: PagePermissionKind::Camera,
                    decision: RememberedPagePermission::Allow,
                }],
            )
            .unwrap(),
        ),
    ]);
    let (mut shell, engine, _screen, queue, profile, item) = ready_shell(store);
    ask(&mut shell, profile, item, &camera(241));
    shell.handle(queue.recv().unwrap());
    answer(
        &mut shell,
        profile,
        item,
        &camera(241),
        PagePermissionPromptDecision::DenyOnce,
    );
    ask(&mut shell, profile, item, &camera(242));
    shell.handle(queue.recv().unwrap());

    assert_eq!(
        settled(&engine),
        [
            (241, PagePermissionRequestSettlement::Deny),
            (242, PagePermissionRequestSettlement::Allow),
        ]
    );
}

#[test]
fn changing_the_site_setting_forgets_a_one_time_answer() {
    let store = Arc::new(FakeStore::default());
    store.page_permission_load_outcomes.lock().unwrap().extend([
        PagePermissionCatalogLoadOutcome::Loaded(empty_catalog()),
        PagePermissionCatalogLoadOutcome::Loaded(empty_catalog()),
        PagePermissionCatalogLoadOutcome::Loaded(empty_catalog()),
    ]);
    let (mut shell, _engine, _screen, queue, profile, item) = ready_shell(store);
    ask(&mut shell, profile, item, &camera(251));
    shell.handle(queue.recv().unwrap());
    answer(
        &mut shell,
        profile,
        item,
        &camera(251),
        PagePermissionPromptDecision::AllowOnce,
    );
    shell.handle(Command::Operation {
        operation_id: "reset-camera".into(),
        command: Box::new(Command::SetSitePermission {
            origin: PageOrigin::parse_exact("https://media.example").unwrap(),
            kind: zephium_core::permissions::SitePermissionKind::Camera,
            decision: None,
        }),
    });
    while let Some(command) = queue.try_recv() {
        shell.handle(command);
    }
    ask(&mut shell, profile, item, &camera(252));
    shell.handle(queue.recv().unwrap());
    assert!(shell.page_permissions.is_visible());
}

#[test]
fn ending_the_private_session_forgets_its_one_time_answers() {
    let (mut shell, engine, screen, _queue, _profile, _item) =
        ready_shell(Arc::new(FakeStore::default()));
    shell.handle(Command::Run("window.newPrivate".into()));
    let profile = shell.windows.focused().unwrap().profile;
    assert!(shell.incognito_profile(profile));
    let private = active_id(&screen);
    navigate_and_commit(&mut shell, private, "https://media.example/call");
    ask(&mut shell, profile, private, &camera(261));
    answer(
        &mut shell,
        profile,
        private,
        &camera(261),
        PagePermissionPromptDecision::AllowOnce,
    );
    assert_eq!(shell.page_permissions.remembered_documents(), 1);

    shell.handle(Command::Run("window.closePrivate".into()));
    assert_eq!(shell.page_permissions.remembered_documents(), 0);
    assert_eq!(
        settled(&engine),
        [(261, PagePermissionRequestSettlement::Allow)]
    );
}

fn watch_prompts(shell: &mut Shell) -> Arc<Mutex<Vec<Option<bool>>>> {
    let shown = Arc::new(Mutex::new(Vec::new()));
    let sink = shown.clone();
    let previous = std::mem::replace(&mut shell.emit, Box::new(|_| {}));
    shell.emit = Box::new(move |projection| {
        if let Projection::PagePermissionPrompt(view) = &projection {
            sink.lock()
                .unwrap()
                .push(view.prompt.as_ref().map(|prompt| prompt.in_sidebar));
        }
        previous(projection);
    });
    shown
}

fn page_on_screen(engine: &FakeEngine) -> bool {
    engine
        .layout_regions
        .lock()
        .unwrap()
        .last()
        .copied()
        .flatten()
        .is_some()
}

#[test]
fn a_prompt_in_the_sidebar_leaves_the_page_on_screen() {
    let (mut shell, engine, _screen, _queue, profile, item) =
        ready_shell(Arc::new(FakeStore::default()));
    make_private(&mut shell, profile);
    let shown = watch_prompts(&mut shell);
    let last = || *shown.lock().unwrap().last().unwrap();

    // Compact or hidden sidebar: a sheet over the window, page off screen.
    ask(&mut shell, profile, item, &camera(271));
    assert_eq!(last(), Some(false));
    assert!(!page_on_screen(&engine));

    shell.handle(Command::SetPagePermissionCardHost(true));
    assert_eq!(last(), Some(true));
    assert!(page_on_screen(&engine));

    shell.handle(Command::SetPagePermissionCardHost(false));
    assert_eq!(last(), Some(false));
    assert!(!page_on_screen(&engine));

    shell.handle(Command::SetPagePermissionCardHost(true));
    answer(
        &mut shell,
        profile,
        item,
        &camera(271),
        PagePermissionPromptDecision::AllowOnce,
    );
    assert_eq!(last(), None);
    assert!(page_on_screen(&engine));
    assert_eq!(
        settled(&engine),
        [(271, PagePermissionRequestSettlement::Allow)]
    );
}

#[test]
fn opening_a_browser_page_dismisses_the_prompt_in_the_sidebar() {
    let (mut shell, engine, _screen, _queue, profile, item) =
        ready_shell(Arc::new(FakeStore::default()));
    make_private(&mut shell, profile);
    shell.handle(Command::SetPagePermissionCardHost(true));
    ask(&mut shell, profile, item, &camera(281));
    assert!(shell.page_permissions.is_visible());

    let _ = shell.handle_operation(Command::ShowBrowserPage(Some(crate::BrowserPage::Settings)));

    assert!(!shell.page_permissions.has_pending());
    assert_eq!(
        settled(&engine),
        [(281, PagePermissionRequestSettlement::Deny)]
    );
}

#[test]
fn a_location_request_macos_refuses_tells_chrome_so() {
    let (mut shell, _engine, _screen, _queue, profile, item) =
        ready_shell(Arc::new(FakeStore::default()));
    make_private(&mut shell, profile);
    let shown = Arc::new(Mutex::new(Vec::new()));
    let sink = shown.clone();
    shell.emit = Box::new(move |projection| {
        if let Projection::PagePermissionPrompt(view) = projection {
            sink.lock().unwrap().push(view.prompt);
        }
    });
    let location = PagePermissionRequest {
        blocked_by_system: true,
        ..request(
            301,
            PagePermissionRequestKind::Single(PagePermissionKind::Geolocation),
        )
    };
    ask(&mut shell, profile, item, &location);
    let prompt = shown.lock().unwrap().last().cloned().flatten().unwrap();
    assert_eq!(prompt.kinds, [PagePermissionKindView::Geolocation]);
    assert!(prompt.blocked_by_system);
}
