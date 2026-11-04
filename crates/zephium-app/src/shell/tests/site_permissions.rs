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
            scheme: None,
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
            scheme: None,
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
            scheme: None,
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
                scheme: None,
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

mod downloads {
    use super::*;
    use zephium_core::permissions::DownloadPermissionPrompt;
    use zephium_ipc::{PageRequestView, SitePermissionKindView};

    fn prompt(
        request: u32,
        site: &str,
        allowed: bool,
        unclicked: bool,
    ) -> DownloadPermissionPrompt {
        DownloadPermissionPrompt {
            request,
            site: origin(site),
            filename: "report.pdf".into(),
            allowed,
            unclicked,
        }
    }

    fn shown(screen: &Screen, id: ItemId) -> Option<PageRequestView> {
        last(screen)
            .tabs
            .into_iter()
            .find(|tab| tab.id == id.to_string())
            .and_then(|tab| tab.page_request)
    }

    fn answers(engine: &FakeEngine) -> Vec<String> {
        engine
            .calls()
            .into_iter()
            .filter(|call| call.starts_with("download-permission "))
            .collect()
    }

    fn answer(shell: &mut Shell, id: ItemId, decision: crate::PageRequestDecision) {
        shell.handle(Command::AnswerPageRequest { id, decision });
    }

    #[test]
    fn allowing_a_site_keeps_it_and_lets_the_download_continue() {
        let store = Arc::new(FakeStore::default());
        let (mut shell, engine, screen, _operations, _queue) = ready(store.clone());
        let profile = shell.windows.focused().unwrap().profile;
        let id = active_id(&screen);
        navigate_and_commit(&mut shell, id, "https://files.example/list");
        shell.handle(Command::Engine(EngineEvent::DownloadPermissionRequested {
            id,
            prompt: prompt(3, "https://files.example", false, false),
        }));
        assert_eq!(
            shown(&screen, id),
            Some(PageRequestView::Download {
                request: 3,
                site: "files.example".into(),
                filename: "report.pdf".into(),
                allowed: false,
                unclicked: false,
            })
        );
        assert!(
            answers(&engine).is_empty(),
            "nothing continues before the answer"
        );

        answer(&mut shell, id, crate::PageRequestDecision::Allow);
        assert_eq!(shown(&screen, id), None);
        assert_eq!(
            answers(&engine),
            vec![format!("download-permission {id} 3 allow")]
        );
        assert_eq!(
            store.download_sites.lock().unwrap().get(&profile).cloned(),
            Some(vec![(origin("https://files.example"), true)])
        );
        assert_eq!(
            engine.download_sites.lock().unwrap().last().cloned(),
            Some((profile, vec![("https://files.example".to_owned(), true)]))
        );
    }

    #[test]
    fn not_now_refuses_this_download_only() {
        let store = Arc::new(FakeStore::default());
        let (mut shell, engine, screen, _operations, _queue) = ready(store.clone());
        let profile = shell.windows.focused().unwrap().profile;
        let id = active_id(&screen);
        navigate_and_commit(&mut shell, id, "https://files.example/list");
        shell.handle(Command::Engine(EngineEvent::DownloadPermissionRequested {
            id,
            prompt: prompt(4, "https://files.example", false, false),
        }));
        answer(&mut shell, id, crate::PageRequestDecision::Dismiss);
        assert_eq!(
            answers(&engine),
            vec![format!("download-permission {id} 4 refuse")]
        );
        assert!(store
            .download_sites
            .lock()
            .unwrap()
            .get(&profile)
            .is_none_or(Vec::is_empty));
    }

    #[test]
    fn an_unclicked_download_on_an_allowed_site_is_let_through_once() {
        let store = Arc::new(FakeStore::default());
        let profile_sites = vec![(origin("https://files.example"), true)];
        let (mut shell, engine, screen, _operations, _queue) = ready(store.clone());
        let profile = shell.windows.focused().unwrap().profile;
        store
            .download_sites
            .lock()
            .unwrap()
            .insert(profile, profile_sites.clone());
        let id = active_id(&screen);
        navigate_and_commit(&mut shell, id, "https://files.example/list");
        shell.handle(Command::Engine(EngineEvent::DownloadPermissionRequested {
            id,
            prompt: prompt(5, "https://files.example", true, true),
        }));
        answer(&mut shell, id, crate::PageRequestDecision::Allow);
        assert_eq!(
            answers(&engine),
            vec![format!("download-permission {id} 5 allow")]
        );
        assert_eq!(
            store.download_sites.lock().unwrap().get(&profile).cloned(),
            Some(profile_sites),
            "a one-time confirmation changes nothing kept"
        );
    }

    #[test]
    fn the_card_goes_when_the_engine_gives_up_and_a_sign_in_refuses_it() {
        let store = Arc::new(FakeStore::default());
        let (mut shell, engine, screen, _operations, _queue) = ready(store);
        let id = active_id(&screen);
        navigate_and_commit(&mut shell, id, "https://files.example/list");
        shell.handle(Command::Engine(EngineEvent::DownloadPermissionRequested {
            id,
            prompt: prompt(6, "https://files.example", false, false),
        }));
        shell.handle(Command::Engine(EngineEvent::DownloadPermissionEnded {
            id,
            request: 5,
        }));
        assert!(
            matches!(shown(&screen, id), Some(PageRequestView::Download { .. })),
            "a stale end leaves the card"
        );
        shell.handle(Command::Engine(EngineEvent::DownloadPermissionEnded {
            id,
            request: 6,
        }));
        assert_eq!(shown(&screen, id), None);
        assert!(answers(&engine).is_empty());

        shell.handle(Command::Engine(EngineEvent::DownloadPermissionRequested {
            id,
            prompt: prompt(7, "https://files.example", false, false),
        }));
        shell.handle(Command::Engine(EngineEvent::HttpAuthRequested {
            id,
            prompt: zephium_core::http_auth::HttpAuthPrompt {
                request: 1,
                host: "files.example".into(),
                insecure: false,
                proxy: false,
                retry: false,
            },
        }));
        assert!(matches!(
            shown(&screen, id),
            Some(PageRequestView::HttpAuth { .. })
        ));
        assert_eq!(
            answers(&engine),
            vec![format!("download-permission {id} 7 refuse")]
        );

        shell.handle(Command::Engine(EngineEvent::DownloadPermissionRequested {
            id,
            prompt: prompt(8, "https://files.example", false, false),
        }));
        assert!(
            matches!(shown(&screen, id), Some(PageRequestView::HttpAuth { .. })),
            "a waiting sign-in is never hidden"
        );
        assert_eq!(
            answers(&engine).last().cloned(),
            Some(format!("download-permission {id} 8 refuse"))
        );
    }

    #[test]
    fn site_settings_list_and_change_download_decisions() {
        let store = Arc::new(FakeStore::default());
        let (mut shell, engine, _screen, operations, _queue) = ready(store.clone());
        let profile = shell.windows.focused().unwrap().profile;
        let set = |shell: &mut Shell, site: &str, decision: Option<RememberedPagePermission>| {
            shell.handle(Command::Operation {
                operation_id: site.into(),
                command: Box::new(Command::SetSitePermission {
                    origin: origin(site),
                    kind: SitePermissionKind::Downloads,
                    decision,
                    scheme: None,
                }),
            });
        };
        set(
            &mut shell,
            "https://files.example",
            Some(RememberedPagePermission::Allow),
        );
        set(
            &mut shell,
            "https://ads.example",
            Some(RememberedPagePermission::Deny),
        );
        let view = query(&mut shell, None);
        assert!(view.kinds.contains(&SitePermissionKindView::Downloads));
        let downloads: Vec<_> = view
            .entries
            .iter()
            .filter(|entry| entry.kind == SitePermissionKindView::Downloads)
            .map(|entry| (entry.origin.clone(), entry.decision))
            .collect();
        assert_eq!(
            downloads,
            vec![
                (
                    "https://ads.example".to_owned(),
                    zephium_ipc::SitePermissionDecisionView::Block
                ),
                (
                    "https://files.example".to_owned(),
                    zephium_ipc::SitePermissionDecisionView::Allow
                ),
            ]
        );

        set(
            &mut shell,
            "https://files.example",
            Some(RememberedPagePermission::Deny),
        );
        set(&mut shell, "https://ads.example", None);
        let outcomes: Vec<_> = operations
            .lock()
            .unwrap()
            .iter()
            .map(|op| op.outcome)
            .collect();
        assert_eq!(outcomes, vec![OperationOutcome::Applied; 4]);
        assert_eq!(
            store.download_sites.lock().unwrap().get(&profile).cloned(),
            Some(vec![(origin("https://files.example"), false)])
        );
        assert_eq!(
            engine.download_sites.lock().unwrap().last().cloned(),
            Some((profile, vec![("https://files.example".to_owned(), false)]))
        );
    }

    #[test]
    fn a_private_profile_keeps_an_allowed_site_in_memory_only() {
        let store = Arc::new(FakeStore::default());
        let (mut shell, engine, screen, _operations, _queue) = ready(store.clone());
        let profile = make_private(&mut shell);
        let id = active_id(&screen);
        navigate_and_commit(&mut shell, id, "https://files.example/list");
        shell.handle(Command::Engine(EngineEvent::DownloadPermissionRequested {
            id,
            prompt: prompt(9, "https://files.example", false, false),
        }));
        answer(&mut shell, id, crate::PageRequestDecision::Allow);
        assert_eq!(
            answers(&engine),
            vec![format!("download-permission {id} 9 allow")]
        );
        assert!(store.download_sites.lock().unwrap().get(&profile).is_none());
        assert_eq!(
            engine.download_sites.lock().unwrap().last().cloned(),
            Some((profile, vec![("https://files.example".to_owned(), true)]))
        );
    }
}

mod app_links {
    use super::*;
    use zephium_ipc::{PageRequestView, SitePermissionKindView};

    fn shown(screen: &Screen, id: ItemId) -> Option<PageRequestView> {
        last(screen)
            .tabs
            .into_iter()
            .find(|tab| tab.id == id.to_string())
            .and_then(|tab| tab.page_request)
    }

    fn opened(engine: &FakeEngine) -> usize {
        engine
            .calls()
            .iter()
            .filter(|call| call.starts_with("open-app "))
            .count()
    }

    fn ask(shell: &mut Shell, id: ItemId, url: &str, initiator: Option<&str>) {
        shell.handle(Command::Engine(EngineEvent::ExternalAppRequested {
            id,
            url: url.into(),
            app: Some("Meetings".into()),
            user_gesture: true,
            initiator: initiator.map(origin),
        }));
    }

    #[test]
    fn always_allow_is_kept_per_site_and_kind_of_link() {
        let store = Arc::new(FakeStore::default());
        let (mut shell, engine, screen, _operations, _queue) = ready(store.clone());
        let profile = shell.windows.focused().unwrap().profile;
        let id = active_id(&screen);
        navigate_and_commit(&mut shell, id, "https://meet.example/room");
        ask(&mut shell, id, "zoommtg://zoom.us/join?confno=1", None);
        shell.handle(Command::AnswerPageRequest {
            id,
            decision: crate::PageRequestDecision::AlwaysAllow,
        });
        assert_eq!(
            store.app_link_sites.lock().unwrap().get(&profile).cloned(),
            Some(vec![(origin("https://meet.example"), "zoommtg".to_owned())])
        );
        let before = opened(&engine);
        ask(&mut shell, id, "zoommtg://zoom.us/join?confno=2", None);
        assert_eq!(shown(&screen, id), None);
        assert_eq!(opened(&engine), before + 1, "the kept kind opens at once");
        ask(&mut shell, id, "msteams:/l/meetup-join/1", None);
        assert!(
            matches!(
                shown(&screen, id),
                Some(PageRequestView::ExternalApp { .. })
            ),
            "another kind of link still asks"
        );

        let view = query(&mut shell, None);
        let kept: Vec<_> = view
            .entries
            .iter()
            .filter(|entry| entry.kind == SitePermissionKindView::AppLinks)
            .map(|entry| (entry.origin.clone(), entry.scheme.clone()))
            .collect();
        assert_eq!(
            kept,
            vec![(
                "https://meet.example".to_owned(),
                Some("zoommtg".to_owned())
            )]
        );
        shell.handle(Command::Operation {
            operation_id: "forget".into(),
            command: Box::new(Command::SetSitePermission {
                origin: origin("https://meet.example"),
                kind: SitePermissionKind::AppLinks,
                decision: None,
                scheme: Some("zoommtg".into()),
            }),
        });
        assert_eq!(
            store.app_link_sites.lock().unwrap().get(&profile).cloned(),
            Some(Vec::new())
        );
    }

    #[test]
    fn a_frame_from_another_site_is_named_and_kept_apart_from_the_tab() {
        let store = Arc::new(FakeStore::default());
        let (mut shell, _engine, screen, _operations, _queue) = ready(store.clone());
        let profile = shell.windows.focused().unwrap().profile;
        let id = active_id(&screen);
        navigate_and_commit(&mut shell, id, "https://news.example/story");
        ask(
            &mut shell,
            id,
            "zoommtg://zoom.us/join",
            Some("https://widget.example"),
        );
        assert_eq!(
            shown(&screen, id),
            Some(PageRequestView::ExternalApp {
                site: Some("widget.example".into()),
                scheme: "zoommtg".into(),
                app: Some("Meetings".into()),
            })
        );
        shell.handle(Command::AnswerPageRequest {
            id,
            decision: crate::PageRequestDecision::AlwaysAllow,
        });
        assert_eq!(
            store.app_link_sites.lock().unwrap().get(&profile).cloned(),
            Some(vec![(
                origin("https://widget.example"),
                "zoommtg".to_owned()
            )])
        );
    }

    #[test]
    fn a_private_profile_keeps_always_allow_in_memory_only() {
        let store = Arc::new(FakeStore::default());
        let (mut shell, engine, screen, _operations, _queue) = ready(store.clone());
        let profile = make_private(&mut shell);
        let id = active_id(&screen);
        navigate_and_commit(&mut shell, id, "https://meet.example/room");
        ask(&mut shell, id, "zoommtg://zoom.us/join?confno=1", None);
        shell.handle(Command::AnswerPageRequest {
            id,
            decision: crate::PageRequestDecision::AlwaysAllow,
        });
        assert!(store.app_link_sites.lock().unwrap().get(&profile).is_none());
        let before = opened(&engine);
        ask(&mut shell, id, "zoommtg://zoom.us/join?confno=2", None);
        assert_eq!(opened(&engine), before + 1);
    }
}
