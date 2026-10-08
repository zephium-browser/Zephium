use super::*;

fn request(profile: ProfileId, id: u64, action: ExtensionBrowserRequestAction) -> EngineEvent {
    EngineEvent::ExtensionBrowserRequested {
        request: ExtensionBrowserRequest::new(
            profile,
            ExtensionBrowserRequestId::new(id).unwrap(),
            action,
        )
        .unwrap(),
    }
}

fn activate_extensions(shell: &mut Shell, profile: ProfileId) {
    let mut profiles = zephium_core::extensions::ExtensionActiveProfiles::EMPTY;
    assert!(profiles.try_insert(profile));
    assert!(shell
        .extension_browser_surfaces
        .replace_active_profiles(profiles));
    assert!(shell.sync_extension_browser_surfaces().native.scheduled);
}

#[test]
fn auth_cleanup_closes_only_its_unchanged_committed_document() {
    let (mut shell, engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    let tab = active_id(&screen);
    let profile = shell.windows.focused().unwrap().profile;
    let provider = "https://provider.example/challenge";
    navigate_and_commit(&mut shell, tab, provider);
    activate_extensions(&mut shell, profile);
    let navigation = shell.presentation.presented_navigations[&tab].0;
    shell.handle(Command::Engine(request(
        profile,
        101,
        ExtensionBrowserRequestAction::CloseTabIfUnchanged {
            tab,
            navigation,
            url: Arc::from(provider),
            cleanup: zephium_core::extensions::AuthTabCleanupPermit::default(),
        },
    )));
    assert!(shell.items.tab(tab).is_none());
    assert!(matches!(
        engine.extension_browser_settlements().last().unwrap().2,
        ExtensionBrowserRequestSettlement::Applied(ExtensionBrowserRequestResult::Complete)
    ));
}

#[test]
fn auth_cleanup_refuses_reused_or_navigating_tabs_when_the_close_is_processed() {
    let (mut shell, engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    let tab = active_id(&screen);
    let profile = shell.windows.focused().unwrap().profile;
    let provider = "https://provider.example/challenge";
    navigate_and_commit(&mut shell, tab, provider);
    activate_extensions(&mut shell, profile);
    let original = shell.presentation.presented_navigations[&tab].0;
    let stale = ExtensionBrowserRequestAction::CloseTabIfUnchanged {
        tab,
        navigation: original,
        url: Arc::from(provider),
        cleanup: zephium_core::extensions::AuthTabCleanupPermit::default(),
    };
    shell.handle(Command::Navigate {
        id: tab,
        input: "https://reading.example/article".into(),
    });
    shell.handle(Command::Engine(request(profile, 102, stale.clone())));
    assert!(shell.items.tab(tab).is_some());
    assert!(matches!(
        engine.extension_browser_settlements().last().unwrap().2,
        ExtensionBrowserRequestSettlement::Rejected(ExtensionBrowserRequestRejection::InvalidScope)
    ));
    // Even the same URL in a newer document is not the old sign-in document.
    navigate_and_commit(&mut shell, tab, provider);
    shell.handle(Command::Engine(request(profile, 103, stale)));
    assert!(shell.items.tab(tab).is_some());
    assert!(matches!(
        engine.extension_browser_settlements().last().unwrap().2,
        ExtensionBrowserRequestSettlement::Rejected(ExtensionBrowserRequestRejection::InvalidScope)
    ));
}

#[test]
fn auth_cleanup_revoked_after_queueing_cannot_close_the_old_url_snapshot() {
    let (mut shell, engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    let tab = active_id(&screen);
    let profile = shell.windows.focused().unwrap().profile;
    let provider = "https://provider.example/login";
    navigate_and_commit(&mut shell, tab, provider);
    activate_extensions(&mut shell, profile);
    let navigation = shell.presentation.presented_navigations[&tab].0;
    let cleanup = zephium_core::extensions::AuthTabCleanupPermit::default();
    let queued = request(
        profile,
        106,
        ExtensionBrowserRequestAction::CloseTabIfUnchanged {
            tab,
            navigation,
            url: Arc::from(provider),
            cleanup: cleanup.clone(),
        },
    );
    // Native link activation relinquishes ownership before a newer URL
    // projection reaches the actor; the old URL/epoch alone must not close it.
    cleanup.revoke();
    shell.handle(Command::Engine(queued));
    assert!(shell.items.tab(tab).is_some());
    assert!(matches!(
        engine.extension_browser_settlements().last().unwrap().2,
        ExtensionBrowserRequestSettlement::Rejected(ExtensionBrowserRequestRejection::InvalidScope)
    ));
}

#[test]
fn expired_creation_cleanup_refuses_reserved_or_pending_navigation() {
    let (mut shell, engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    let tab = active_id(&screen);
    let profile = shell.windows.focused().unwrap().profile;
    activate_extensions(&mut shell, profile);
    assert!(shell.items.reserve_deferred_navigation(tab).is_some());
    shell.handle(Command::Engine(request(
        profile,
        104,
        ExtensionBrowserRequestAction::CloseTabIfPristine { tab },
    )));
    assert!(shell.items.tab(tab).is_some());
    assert!(matches!(
        engine.extension_browser_settlements().last().unwrap().2,
        ExtensionBrowserRequestSettlement::Rejected(ExtensionBrowserRequestRejection::InvalidScope)
    ));
    shell.handle(Command::Navigate {
        id: tab,
        input: "https://reading.example/".into(),
    });
    shell.handle(Command::Engine(request(
        profile,
        105,
        ExtensionBrowserRequestAction::CloseTabIfPristine { tab },
    )));
    assert!(shell.items.tab(tab).is_some());
}

#[test]
fn authenticated_browser_mutations_follow_shell_scope_and_settle_exactly_once() {
    let (mut shell, engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    let original = active_id(&screen);
    let window = shell.windows.focused().unwrap().id;
    let profile = shell.windows.focused().unwrap().profile;
    activate_extensions(&mut shell, profile);

    shell.handle(Command::Engine(request(
        profile,
        1,
        ExtensionBrowserRequestAction::CreateTab {
            window: Some(window),
            url: Some(Arc::from("https://created.example/")),
            active: true,
        },
    )));

    let settlements = engine.extension_browser_settlements();
    let (
        _,
        _,
        ExtensionBrowserRequestSettlement::Applied(ExtensionBrowserRequestResult::CreatedTab(
            created,
        )),
    ) = settlements[0]
    else {
        panic!("create request was not applied: {settlements:?}")
    };
    assert_ne!(created, original);
    assert_eq!(shell.windows.focused().unwrap().active, Some(created));
    #[cfg(target_os = "macos")]
    {
        let (first_url, first_intent) = engine.extension_browser_first_urls()[0].clone().unwrap();
        assert_eq!(first_url.as_ref(), "https://created.example/");
        assert!(
            !engine
                .calls()
                .iter()
                .any(|call| call.starts_with(&format!("create {created} "))),
            "the requested URL must wait for the native tabs.create reply"
        );
        shell.handle(Command::Engine(EngineEvent::ExtensionCreatedTabReplied {
            profile,
            request: ExtensionBrowserRequestId::new(1).unwrap(),
            tab: created,
            url: Arc::from("https://created.example/"),
            intent: first_intent,
        }));
        assert!(engine.calls().iter().any(|call| {
            call == &format!("create {created} https://created.example/ [default]")
        }));
        shell.handle(Command::Engine(EngineEvent::ExtensionCreatedTabReplied {
            profile,
            request: ExtensionBrowserRequestId::new(1).unwrap(),
            tab: created,
            url: Arc::from("https://created.example/"),
            intent: first_intent,
        }));
        assert_eq!(
            engine
                .calls()
                .iter()
                .filter(
                    |call| call == &&format!("create {created} https://created.example/ [default]")
                )
                .count(),
            1,
            "duplicate native settlement cannot start the first load twice"
        );
    }
    #[cfg(not(target_os = "macos"))]
    {
        assert!(engine.extension_browser_first_urls()[0].is_none());
        assert!(engine.calls().iter().any(|call| {
            call == &format!("create {created} https://created.example/ [default]")
        }));
    }

    shell.handle(Command::Engine(request(
        profile,
        2,
        ExtensionBrowserRequestAction::ActivateTab { tab: original },
    )));
    assert_eq!(shell.windows.focused().unwrap().active, Some(original));

    shell.handle(Command::Engine(request(
        profile,
        3,
        ExtensionBrowserRequestAction::LoadTabUrl {
            tab: original,
            url: Arc::from("https://updated.example/"),
        },
    )));
    assert!(engine
        .calls()
        .iter()
        .any(|call| { call == &format!("create {original} https://updated.example/ [default]") }));

    shell.handle(Command::Engine(request(
        profile,
        4,
        ExtensionBrowserRequestAction::CloseTab { tab: created },
    )));
    assert!(shell.items.tab(created).is_none());

    shell.handle(Command::Engine(request(
        ProfileId::from(99_999),
        5,
        ExtensionBrowserRequestAction::ActivateTab { tab: original },
    )));

    let settlements = engine.extension_browser_settlements();
    assert_eq!(settlements.len(), 5);
    assert!(settlements[..4].iter().all(|(_, _, settlement)| matches!(
        settlement,
        ExtensionBrowserRequestSettlement::Applied(_)
    )));
    assert_eq!(
        settlements[4].2,
        ExtensionBrowserRequestSettlement::Rejected(
            ExtensionBrowserRequestRejection::InvalidContext
        )
    );
}

#[test]
#[cfg(target_os = "macos")]
fn delayed_create_reply_cannot_override_a_newer_failed_navigation_or_closed_tab() {
    let (mut shell, engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    let window = shell.windows.focused().unwrap().id;
    let profile = shell.windows.focused().unwrap().profile;
    activate_extensions(&mut shell, profile);

    shell.handle(Command::Engine(request(
        profile,
        41,
        ExtensionBrowserRequestAction::CreateTab {
            window: Some(window),
            url: Some(Arc::from("https://first.example/")),
            active: true,
        },
    )));
    let (
        _,
        _,
        ExtensionBrowserRequestSettlement::Applied(ExtensionBrowserRequestResult::CreatedTab(tab)),
    ) = engine.extension_browser_settlements()[0]
    else {
        panic!("first logical tab was not created");
    };
    let intent = engine.extension_browser_first_urls()[0].as_ref().unwrap().1;
    shell.handle(Command::Engine(EngineEvent::ExtensionCreatedTabReplied {
        profile: ProfileId::from(999),
        request: ExtensionBrowserRequestId::new(41).unwrap(),
        tab,
        url: Arc::from("https://first.example/"),
        intent,
    }));
    assert_eq!(shell.items.pending_navigation_request(tab), Some(intent));

    shell.handle(Command::Navigate {
        id: tab,
        input: "https://newer.example/".into(),
    });
    shell.handle(Command::Engine(EngineEvent::ViewCreationFailed { id: tab }));
    assert!(shell.items.pending_navigation_request(tab).is_none());
    shell.handle(Command::Engine(EngineEvent::ExtensionCreatedTabReplied {
        profile,
        request: ExtensionBrowserRequestId::new(41).unwrap(),
        tab,
        url: Arc::from("https://first.example/"),
        intent,
    }));
    assert!(!engine
        .calls()
        .iter()
        .any(|call| call == &format!("create {tab} https://first.example/ [default]")));

    shell.handle(Command::Engine(request(
        profile,
        42,
        ExtensionBrowserRequestAction::CreateTab {
            window: Some(window),
            url: Some(Arc::from("https://closed.example/")),
            active: true,
        },
    )));
    let (
        _,
        _,
        ExtensionBrowserRequestSettlement::Applied(ExtensionBrowserRequestResult::CreatedTab(
            closed,
        )),
    ) = engine.extension_browser_settlements()[1]
    else {
        panic!("second logical tab was not created");
    };
    let closed_intent = engine.extension_browser_first_urls()[1].as_ref().unwrap().1;
    shell.handle(Command::Close(closed));
    shell.handle(Command::Engine(EngineEvent::ExtensionCreatedTabReplied {
        profile,
        request: ExtensionBrowserRequestId::new(42).unwrap(),
        tab: closed,
        url: Arc::from("https://closed.example/"),
        intent: closed_intent,
    }));
    assert!(shell.items.tab(closed).is_none());
    assert!(!engine
        .calls()
        .iter()
        .any(|call| call == &format!("create {closed} https://closed.example/ [default]")));
    let _ = screen;
}

#[test]
fn prebootstrap_browser_request_is_explicitly_rejected() {
    let (mut shell, engine, _) = setup();
    shell.handle(Command::Engine(request(
        ProfileId::from(1),
        1,
        ExtensionBrowserRequestAction::CreateTab {
            window: None,
            url: None,
            active: true,
        },
    )));

    assert_eq!(
        engine.extension_browser_settlements(),
        vec![(
            ProfileId::from(1),
            ExtensionBrowserRequestId::new(1).unwrap(),
            ExtensionBrowserRequestSettlement::Rejected(
                ExtensionBrowserRequestRejection::InvalidContext
            ),
        )]
    );
}

#[test]
fn extension_page_authority_is_limited_to_the_active_profile() {
    let (mut shell, engine, _) = setup();
    shell.handle(Command::Bootstrap);
    let profile = shell.windows.focused().unwrap().profile;
    activate_extensions(&mut shell, profile);

    shell.handle(Command::Engine(request(
        profile,
        1,
        ExtensionBrowserRequestAction::OpenExtensionPage,
    )));
    shell.handle(Command::Engine(request(
        ProfileId::from(99_999),
        2,
        ExtensionBrowserRequestAction::OpenExtensionPage,
    )));

    let settlements = engine.extension_browser_settlements();
    let ExtensionBrowserRequestSettlement::Applied(
        ExtensionBrowserRequestResult::ExtensionPageAuthorized { tab, window },
    ) = settlements[0].2
    else {
        panic!("extension page was not admitted")
    };
    assert_eq!(window, shell.windows.focused().unwrap().id);
    let state = shell.items.tab(tab).unwrap();
    assert_eq!(
        state.content,
        zephium_core::item::TabContent::ExtensionOwned
    );
    assert!(state.url.is_none());
    assert!(state.has_view());
    assert_eq!(
        settlements[1].2,
        ExtensionBrowserRequestSettlement::Rejected(
            ExtensionBrowserRequestRejection::InvalidContext,
        )
    );
}

#[test]
fn extension_navigation_controls_require_a_resident_tab_and_never_wake_a_discard() {
    let (mut shell, engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    let tab = active_id(&screen);
    navigate_and_commit(&mut shell, tab, "extension-controls.example");
    let profile = shell.windows.focused().unwrap().profile;
    activate_extensions(&mut shell, profile);
    shell.handle(Command::Engine(EngineEvent::NavState {
        id: tab,
        can_go_back: true,
        can_go_forward: true,
    }));

    for (id, action) in [
        (1, ExtensionBrowserRequestAction::ReloadTab { tab }),
        (2, ExtensionBrowserRequestAction::GoBack { tab }),
        (3, ExtensionBrowserRequestAction::GoForward { tab }),
    ] {
        shell.handle(Command::Engine(request(profile, id, action)));
    }

    let calls = engine.calls();
    assert!(calls.iter().any(|call| call == &format!("reload {tab}")));
    assert!(calls.iter().any(|call| call == &format!("back {tab}")));
    assert!(calls.iter().any(|call| call == &format!("forward {tab}")));
    assert!(engine.extension_browser_settlements()[..3]
        .iter()
        .all(|(_, _, settlement)| matches!(
            settlement,
            ExtensionBrowserRequestSettlement::Applied(ExtensionBrowserRequestResult::Complete)
        )));

    assert!(shell.items.mark_view_discarded(tab));
    let native_calls_before = engine.calls().len();
    shell.handle(Command::Engine(request(
        profile,
        4,
        ExtensionBrowserRequestAction::ReloadTab { tab },
    )));

    assert_eq!(engine.calls().len(), native_calls_before);
    assert_eq!(
        engine.extension_browser_settlements()[3].2,
        ExtensionBrowserRequestSettlement::Rejected(ExtensionBrowserRequestRejection::TabDiscarded)
    );
    assert!(!shell.items.tab(tab).unwrap().has_view());
}

#[test]
fn extension_history_unavailable_is_not_reported_as_applied() {
    let (mut shell, engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    let tab = active_id(&screen);
    navigate_and_commit(&mut shell, tab, "extension-history.example");
    let profile = shell.windows.focused().unwrap().profile;
    activate_extensions(&mut shell, profile);

    shell.handle(Command::Engine(request(
        profile,
        1,
        ExtensionBrowserRequestAction::GoBack { tab },
    )));

    assert_eq!(
        engine.extension_browser_settlements()[0].2,
        ExtensionBrowserRequestSettlement::Rejected(
            ExtensionBrowserRequestRejection::InvalidRequest
        )
    );
    assert!(!engine
        .calls()
        .iter()
        .any(|call| call == &format!("back {tab}")));
}

#[test]
fn extension_requests_reach_tabs_in_spaces_no_window_shows() {
    use super::extension_browser_surface::{activate_profile, install_profile};
    let (mut shell, engine, _) = setup();
    let profile = ProfileId::from(53_000);
    let (shown, other) = (SpaceId::from(53_001), SpaceId::from(53_002));
    install_profile(&mut shell, profile, &[shown, other]);
    shell
        .windows
        .create(WindowKind::Main, profile, shown, Size::new(1200.0, 800.0));
    activate_profile(&mut shell, profile);
    shell.bootstrapped = true;
    let visible = ItemId::from(53_003);
    let hidden = ItemId::from(53_004);
    for (tab, space) in [(visible, shown), (hidden, other)] {
        assert!(shell.items.insert_tab(
            tab,
            Placement::Space {
                space,
                section: SpaceSection::Today,
            },
        ));
    }
    shell.windows.focused_mut().unwrap().active = Some(visible);

    shell.handle(Command::Engine(request(
        profile,
        1,
        ExtensionBrowserRequestAction::LoadTabUrl {
            tab: hidden,
            url: Arc::from("https://elsewhere.example/"),
        },
    )));
    shell.handle(Command::Engine(request(
        profile,
        2,
        ExtensionBrowserRequestAction::CloseTab { tab: hidden },
    )));

    let settlements = engine.extension_browser_settlements();
    assert!(
        settlements.iter().all(|(_, _, settlement)| matches!(
            settlement,
            ExtensionBrowserRequestSettlement::Applied(_)
        )),
        "{settlements:?}"
    );
    assert!(shell.items.tab(hidden).is_none());
    assert_eq!(shell.windows.focused().unwrap().active, Some(visible));
}

#[test]
fn extensions_cannot_query_or_mutate_work_owned_borrowed_or_agent_tabs() {
    for owned in [false, true] {
        let (mut shell, engine, screen) = setup();
        shell.handle(Command::Bootstrap);
        let tab = active_id(&screen);
        let window = shell.windows.focused().unwrap();
        let profile = window.profile;
        shell.work_pane = Some(crate::shell::work_pane::WorkPane {
            window: window.id,
            tab,
            rect: Rect::new(200.0, 100.0, 600.0, 500.0),
            generation: 1,
            owned,
        });
        activate_extensions(&mut shell, profile);
        let surfaces = engine.extension_browser_surfaces();
        let window = &surfaces.last().unwrap().windows()[0];
        assert!(window.tabs().iter().all(|candidate| candidate.id() != tab));
        assert_eq!(window.active(), None);
        // Agent-owned contexts never enter Items. Even a guessed ID cannot
        // turn an owned context into an extension-addressable Browse tab.
        let agent = ItemId::from(u128::MAX);
        assert!(shell.items.get(agent).is_none());
        for (index, action) in [
            ExtensionBrowserRequestAction::CloseTab { tab },
            ExtensionBrowserRequestAction::LoadTabUrl {
                tab,
                url: Arc::from("https://attacker.invalid/"),
            },
            ExtensionBrowserRequestAction::ActivateTab { tab },
            ExtensionBrowserRequestAction::ReloadTab { tab },
            ExtensionBrowserRequestAction::GoBack { tab },
            ExtensionBrowserRequestAction::GoForward { tab },
            ExtensionBrowserRequestAction::CloseTab { tab: agent },
        ]
        .into_iter()
        .enumerate()
        {
            shell.handle(Command::Engine(request(profile, index as u64 + 1, action)));
            assert!(matches!(
                engine.extension_browser_settlements().last().unwrap().2,
                ExtensionBrowserRequestSettlement::Rejected(
                    ExtensionBrowserRequestRejection::InvalidScope
                )
            ));
            assert!(shell.items.tab(tab).is_some());
        }
        shell.work_pane = None;
        assert!(shell.sync_extension_browser_surfaces().native.scheduled);
        assert!(engine
            .extension_browser_surfaces()
            .last()
            .unwrap()
            .windows()[0]
            .tabs()
            .iter()
            .any(|candidate| candidate.id() == tab));
    }
}
