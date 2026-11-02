use super::*;

use zephium_core::permissions::{PageOrigin, RememberedPagePermission, SitePermissionKind};

fn origin(value: &str) -> PageOrigin {
    PageOrigin::parse_exact(value).unwrap()
}

fn ready(store: Arc<FakeStore>) -> (Shell, Arc<FakeEngine>, Screen, OperationLog, CommandQueue) {
    let (mut shell, engine, screen, operations) = setup_with_operation_log(store);
    shell.handle(Command::Bootstrap);
    shell.handle(Command::SetWindowFocused(true));
    let queue = CommandQueue::new();
    shell.attach_queue(queue.clone());
    (shell, engine, screen, operations, queue)
}

fn make_private(shell: &mut Shell) -> ProfileId {
    let profile = shell.windows.focused().unwrap().profile;
    let mut private = shell.profiles.remove(profile).unwrap();
    private.kind = ProfileKind::Incognito;
    assert!(shell.profiles.insert(private));
    profile
}

fn query(shell: &mut Shell, origin: Option<PageOrigin>) -> zephium_ipc::SitePermissionsView {
    let (reply, receiver) = std::sync::mpsc::sync_channel(1);
    shell.handle(Command::SitePermissions { origin, reply });
    receiver.try_recv().unwrap().unwrap()
}

#[test]
fn a_private_profile_lists_nothing_and_reads_nothing() {
    let store = Arc::new(FakeStore::default());
    let (mut shell, _engine, _screen, operations, _queue) = ready(store.clone());
    make_private(&mut shell);
    let view = query(&mut shell, None);
    assert!(view.private && view.entries.is_empty());
    assert!(store.page_permission_load_calls.lock().unwrap().is_empty());

    shell.handle(Command::Operation {
        operation_id: "private-change".into(),
        command: Box::new(Command::SetSitePermission {
            origin: origin("https://meet.example"),
            kind: SitePermissionKind::Camera,
            decision: Some(RememberedPagePermission::Allow),
        }),
    });
    assert_eq!(
        operations.lock().unwrap()[0].outcome,
        OperationOutcome::Rejected
    );
    assert!(store
        .page_permission_mutation_calls
        .lock()
        .unwrap()
        .is_empty());
}

#[test]
fn a_kind_this_platform_does_not_keep_is_refused() {
    let store = Arc::new(FakeStore::default());
    let (mut shell, _engine, _screen, operations, _queue) = ready(store.clone());
    shell.handle(Command::Operation {
        operation_id: "midi".into(),
        command: Box::new(Command::SetSitePermission {
            origin: origin("https://synth.example"),
            kind: SitePermissionKind::Midi,
            decision: Some(RememberedPagePermission::Allow),
        }),
    });
    // macOS keeps no MIDI decisions; Windows hands it to the engine.
    if cfg!(target_os = "windows") {
        assert!(operations.lock().unwrap().is_empty());
    } else {
        assert_eq!(
            operations.lock().unwrap()[0].outcome,
            OperationOutcome::Rejected
        );
    }
    assert!(store.page_permission_load_calls.lock().unwrap().is_empty());
}

#[test]
fn allowing_pop_ups_from_a_blocked_card_is_saved_and_reaches_the_engine() {
    let store = Arc::new(FakeStore::default());
    let (mut shell, engine, screen, operations, _queue) = ready(store.clone());
    let profile = shell.windows.focused().unwrap().profile;
    let id = active_id(&screen);
    navigate_and_commit(&mut shell, id, "https://mail.example/inbox");
    shell.handle(Command::Engine(EngineEvent::PageOpenBlocked {
        id,
        url: None,
    }));
    shell.handle(Command::Operation {
        operation_id: "always".into(),
        command: Box::new(Command::AnswerPageRequest {
            id,
            decision: crate::PageRequestDecision::AlwaysAllow,
        }),
    });
    assert_eq!(
        operations.lock().unwrap()[0].outcome,
        OperationOutcome::Applied
    );
    assert_eq!(
        store.popup_sites.lock().unwrap().get(&profile).cloned(),
        Some(vec![origin("https://mail.example")])
    );
    assert_eq!(
        engine.popup_sites.lock().unwrap().last().cloned(),
        Some((profile, vec!["https://mail.example".to_owned()]))
    );

    shell.handle(Command::Operation {
        operation_id: "block".into(),
        command: Box::new(Command::SetSitePermission {
            origin: origin("https://mail.example"),
            kind: SitePermissionKind::Popups,
            decision: Some(RememberedPagePermission::Deny),
        }),
    });
    assert_eq!(
        operations.lock().unwrap()[1].outcome,
        OperationOutcome::Applied
    );
    assert_eq!(
        store.popup_sites.lock().unwrap().get(&profile).cloned(),
        Some(Vec::new())
    );
    assert_eq!(
        engine.popup_sites.lock().unwrap().last().cloned(),
        Some((profile, Vec::new()))
    );
}

#[test]
fn a_private_tab_cannot_always_allow_pop_ups() {
    let store = Arc::new(FakeStore::default());
    let (mut shell, engine, screen, operations, _queue) = ready(store.clone());
    let id = active_id(&screen);
    navigate_and_commit(&mut shell, id, "https://mail.example/inbox");
    let profile = make_private(&mut shell);
    shell.handle(Command::Engine(EngineEvent::PageOpenBlocked {
        id,
        url: None,
    }));
    let shown = last(&screen)
        .tabs
        .into_iter()
        .find(|tab| tab.id == id.to_string())
        .and_then(|tab| tab.page_request);
    assert_eq!(
        shown,
        Some(zephium_ipc::PageRequestView::Popup {
            host: None,
            site: None
        })
    );
    shell.handle(Command::Operation {
        operation_id: "always".into(),
        command: Box::new(Command::AnswerPageRequest {
            id,
            decision: crate::PageRequestDecision::AlwaysAllow,
        }),
    });
    assert_eq!(
        operations.lock().unwrap()[0].outcome,
        OperationOutcome::Rejected
    );
    assert!(store.popup_sites.lock().unwrap().get(&profile).is_none());
    assert!(engine
        .popup_sites
        .lock()
        .unwrap()
        .iter()
        .all(|(owner, sites)| *owner != profile || sites.is_empty()));
}

/// macOS keeps these decisions in the page-permission catalog; Windows asks
/// the engine.
#[cfg(not(target_os = "windows"))]
mod catalog {
    use super::*;
    use zephium_core::permissions::{
        PagePermissionCatalog, PagePermissionCatalogRevision, PagePermissionChange,
        PagePermissionGrant, PagePermissionGrantRevision, PagePermissionKind,
    };
    use zephium_ipc::{SitePermissionDecisionView, SitePermissionKindView};

    fn catalog(
        grants: Vec<(u128, &str, PagePermissionKind, RememberedPagePermission)>,
    ) -> PagePermissionCatalog {
        PagePermissionCatalog::new(
            PagePermissionCatalogRevision::new(4).unwrap(),
            grants
                .into_iter()
                .map(|(id, site, kind, decision)| PagePermissionGrant {
                    id: zephium_core::ids::PagePermissionGrantId::from(id),
                    revision: PagePermissionGrantRevision::INITIAL,
                    origin: origin(site),
                    kind,
                    decision,
                })
                .collect(),
        )
        .unwrap()
    }

    #[test]
    fn site_settings_lists_saved_decisions_and_allowed_pop_ups() {
        let store = Arc::new(FakeStore::default());
        store.page_permission_load_outcomes.lock().unwrap().extend([
            PagePermissionCatalogLoadOutcome::Loaded(catalog(vec![
                (
                    1,
                    "https://meet.example",
                    PagePermissionKind::Camera,
                    RememberedPagePermission::Allow,
                ),
                (
                    2,
                    "https://maps.example",
                    PagePermissionKind::Geolocation,
                    RememberedPagePermission::Deny,
                ),
            ])),
            PagePermissionCatalogLoadOutcome::Loaded(catalog(vec![(
                1,
                "https://meet.example",
                PagePermissionKind::Camera,
                RememberedPagePermission::Allow,
            )])),
        ]);
        let (mut shell, _engine, _screen, _operations, _queue) = ready(store.clone());
        let profile = shell.windows.focused().unwrap().profile;
        store
            .popup_sites
            .lock()
            .unwrap()
            .insert(profile, vec![origin("https://meet.example")]);
        shell.popup_sites.clear();

        let all = query(&mut shell, None);
        assert!(!all.private && all.complete);
        assert_eq!(
            all.entries
                .iter()
                .map(|entry| (entry.origin.as_str(), entry.kind, entry.decision))
                .collect::<Vec<_>>(),
            [
                (
                    "https://meet.example",
                    SitePermissionKindView::Camera,
                    SitePermissionDecisionView::Allow
                ),
                (
                    "https://maps.example",
                    SitePermissionKindView::Geolocation,
                    SitePermissionDecisionView::Block
                ),
                (
                    "https://meet.example",
                    SitePermissionKindView::Popups,
                    SitePermissionDecisionView::Allow
                ),
            ]
        );
        let one = query(&mut shell, Some(origin("https://meet.example")));
        assert_eq!(one.entries.len(), 2);
    }

    #[test]
    fn changing_a_decision_writes_one_exact_catalog_change() {
        let store = Arc::new(FakeStore::default());
        store
            .page_permission_load_outcomes
            .lock()
            .unwrap()
            .push_back(PagePermissionCatalogLoadOutcome::Loaded(catalog(vec![(
                1,
                "https://meet.example",
                PagePermissionKind::Camera,
                RememberedPagePermission::Allow,
            )])));
        store
            .page_permission_mutation_outcomes
            .lock()
            .unwrap()
            .push_back(super::page_permissions::applied_mutation_outcome());
        let (mut shell, _engine, _screen, operations, queue) = ready(store.clone());
        shell.handle(Command::Operation {
            operation_id: "block-camera".into(),
            command: Box::new(Command::SetSitePermission {
                origin: origin("https://meet.example"),
                kind: SitePermissionKind::Camera,
                decision: Some(RememberedPagePermission::Deny),
            }),
        });
        assert!(operations.lock().unwrap().is_empty());
        shell.handle(queue.recv().unwrap());
        let calls = store.page_permission_mutation_calls.lock().unwrap().clone();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].1, PagePermissionCatalogRevision::new(4).unwrap());
        assert!(matches!(
            calls[0].2.changes(),
            [PagePermissionChange::Update {
                decision: RememberedPagePermission::Deny,
                ..
            }]
        ));
        shell.handle(queue.recv().unwrap());
        assert_eq!(
            operations
                .lock()
                .unwrap()
                .last()
                .map(|op| (op.operation_id.as_str(), op.outcome)),
            Some(("block-camera", OperationOutcome::Applied))
        );
    }
}
