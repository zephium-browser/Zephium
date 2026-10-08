use super::*;

#[test]
fn run_commands_drive_tabs_zoom_and_engine() {
    let (mut shell, engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    let first = active_id(&screen);
    shell.handle(Command::Navigate {
        id: first,
        input: "example.com".into(),
    });

    shell.handle(Command::Run("tab.new".into()));
    let second = active_id(&screen);
    assert_ne!(first, second);

    shell.handle(Command::Run("tab.next".into()));
    assert_eq!(active_id(&screen), first);
    shell.handle(Command::Run("tab.previous".into()));
    assert_eq!(active_id(&screen), second);

    shell.handle(Command::Run("tab.close".into()));
    assert_eq!(active_id(&screen), first);

    shell.handle(Command::Run("zoom.in".into()));
    assert!(engine
        .calls()
        .iter()
        .any(|c| c == &format!("zoom {first} 1.1")));
    shell.handle(Command::Run("zoom.reset".into()));
    assert!(engine
        .calls()
        .iter()
        .any(|c| c == &format!("zoom {first} 1")));
}

#[test]
fn setting_operation_reports_store_admission_without_claiming_durability() {
    let store = Arc::new(FakeStore::default());
    let (mut shell, _engine, _screen) = setup_with(store.clone());
    shell.handle(Command::Bootstrap);

    store
        .reject_settings
        .store(true, std::sync::atomic::Ordering::Release);
    let rejected = shell.handle_operation(Command::SetAppSetting {
        key: "appearance".into(),
        value: "dark".into(),
    });
    assert_eq!(rejected.outcome, OperationOutcome::Rejected);
    assert_eq!(rejected.reason, OperationReason::StoreAdmissionRejected);

    store
        .reject_settings
        .store(false, std::sync::atomic::Ordering::Release);
    let accepted = shell.handle_operation(Command::SetAppSetting {
        key: "appearance".into(),
        value: "light".into(),
    });
    assert_eq!(accepted.outcome, OperationOutcome::Deferred);
    assert_eq!(accepted.reason, OperationReason::StoreWorkPending);
}

#[test]
fn operation_outcomes_reject_invalid_scope_and_report_native_admission() {
    let (mut shell, engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    let id = active_id(&screen);
    navigate_and_commit(&mut shell, id, "admission.example");

    let invalid = shell.handle_operation(Command::Reload(ItemId::from(u128::MAX)));
    assert_eq!(invalid.outcome, OperationOutcome::Rejected);
    assert_eq!(invalid.reason, OperationReason::InvalidScope);

    engine
        .reject_native_dispatch
        .store(true, std::sync::atomic::Ordering::Release);
    let rejected = shell.handle_operation(Command::Reload(id));
    assert_eq!(rejected.outcome, OperationOutcome::NativeAdmissionFailed);
    assert_eq!(rejected.reason, OperationReason::NativeDispatchRejected);

    engine
        .reject_native_dispatch
        .store(false, std::sync::atomic::Ordering::Release);
    let scheduled = shell.handle_operation(Command::Reload(id));
    assert_eq!(scheduled.outcome, OperationOutcome::Deferred);
    assert_eq!(scheduled.reason, OperationReason::NativeWorkPending);
}

#[test]
fn history_operations_distinguish_noop_from_scheduled_dispatch() {
    let (mut shell, _engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    let id = active_id(&screen);
    navigate_and_commit(&mut shell, id, "history.example");

    let unavailable = shell.handle_operation(Command::GoBack(id));
    assert_eq!(unavailable.outcome, OperationOutcome::NoOp);
    assert_eq!(unavailable.reason, OperationReason::HistoryUnavailable);

    shell.handle(Command::Engine(EngineEvent::NavState {
        id,
        can_go_back: true,
        can_go_forward: false,
    }));
    let back = shell.handle_operation(Command::GoBack(id));
    assert_eq!(back.outcome, OperationOutcome::Deferred);
    assert_eq!(back.reason, OperationReason::NativeWorkPending);
    let forward = shell.handle_operation(Command::GoForward(id));
    assert_eq!(forward.outcome, OperationOutcome::NoOp);
    assert_eq!(forward.reason, OperationReason::HistoryUnavailable);
}

#[test]
fn navigation_waits_for_exact_inflight_discard_instead_of_claiming_success() {
    let (mut shell, _engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    let id = active_id(&screen);
    navigate_and_commit(&mut shell, id, "before-discard.example");
    shell.residency.discard_probes.insert(
        id,
        PendingDiscardProbe::Closing {
            probe: DiscardProbeId(77),
            recreate: false,
            deferred_navigation: None,
            reload_on_refusal: false,
        },
    );

    let completion = shell.handle_operation(Command::Navigate {
        id,
        input: "after-discard.example".into(),
    });
    assert_eq!(completion.outcome, OperationOutcome::Deferred);
    assert_eq!(completion.reason, OperationReason::DiscardCompletionPending);
    assert!(matches!(
        shell.residency.discard_probes.get(&id),
        Some(PendingDiscardProbe::Closing {
            recreate: true,
            deferred_navigation: Some(input),
            ..
        }) if input == "https://after-discard.example/"
    ));
}

#[test]
fn layout_and_zoom_report_rejection_and_roll_back_unapplied_zoom() {
    let (mut shell, engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    let first = active_id(&screen);
    navigate_and_commit(&mut shell, first, "layout-left.example");
    let opened = shell.operation_open();
    assert_eq!(opened.outcome, OperationOutcome::Deferred);
    let second = active_id(&screen);
    navigate_and_commit(&mut shell, second, "layout-right.example");

    let split = shell.handle_operation(Command::SplitWith {
        other: first,
        axis: Axis::Row,
    });
    assert_eq!(split.outcome, OperationOutcome::Deferred);

    let original_zoom = shell.items.tab(second).unwrap().zoom;
    engine
        .reject_native_dispatch
        .store(true, std::sync::atomic::Ordering::Release);
    let zoom = shell.handle_operation(Command::Run("zoom.in".into()));
    assert_eq!(zoom.outcome, OperationOutcome::NativeAdmissionFailed);
    assert_eq!(shell.items.tab(second).unwrap().zoom, original_zoom);

    let unsplit = shell.handle_operation(Command::Unsplit);
    assert_eq!(unsplit.outcome, OperationOutcome::NativeAdmissionFailed);
    assert!(shell.windows.focused().unwrap().splits.is_none());
}

#[test]
fn browser_page_hides_native_content_and_restores_exact_tab() {
    let page = crate::BrowserPage::Work;
    let (mut shell, engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    let id = active_id(&screen);
    navigate_and_commit(&mut shell, id, "settings-return.example");
    let window = shell.windows.focused().unwrap().id;
    let opened = shell.handle_operation(Command::ShowBrowserPage(Some(page)));
    assert_eq!(opened.outcome, OperationOutcome::Deferred);
    assert_eq!(shell.active_browser_page(), Some(page));
    assert_eq!(
        engine
            .calls()
            .iter()
            .rev()
            .find(|call| call.starts_with("layout@")),
        Some(&format!("layout@{window} "))
    );
    assert_eq!(active_id(&screen), id);
    assert!(shell.locate_divider(300.0, 200.0).is_none());
    shell.handle(Command::Bootstrap);
    assert_eq!(shell.active_browser_page(), Some(page));
    shell.handle_operation(Command::ShowBrowserPage(None));
    assert_eq!(shell.active_browser_page(), None);
    assert_eq!(
        engine
            .calls()
            .iter()
            .rev()
            .find(|call| call.starts_with("layout@")),
        Some(&format!("layout@{window} {id}"))
    );
    assert_eq!(active_id(&screen), id);
}

#[test]
fn settings_is_a_window_page_and_extensions_is_a_deduplicated_tab() {
    use zephium_core::item::{BrowserOwnedTab, TabContent};

    let (mut shell, engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    let web = active_id(&screen);
    navigate_and_commit(&mut shell, web, "browser-owned-return.example");
    let window = shell.windows.focused().unwrap().id;

    assert_eq!(
        shell
            .handle_operation(Command::ShowBrowserPage(Some(crate::BrowserPage::Settings)))
            .outcome,
        OperationOutcome::Deferred
    );
    assert_eq!(active_id(&screen), web);
    assert_eq!(shell.items_snapshot().unwrap().tabs.len(), 1);
    assert_eq!(
        shell.active_browser_page(),
        Some(crate::BrowserPage::Settings)
    );
    assert_eq!(
        engine
            .calls()
            .iter()
            .rev()
            .find(|call| call.starts_with("layout@")),
        Some(&format!("layout@{window} "))
    );
    shell.handle_operation(Command::ShowBrowserPage(Some(crate::BrowserPage::Settings)));
    assert_eq!(active_id(&screen), web);
    shell.handle_operation(Command::ShowBrowserPage(None));
    assert_eq!(shell.active_browser_page(), None);

    shell.handle_operation(Command::ShowBrowserPage(Some(
        crate::BrowserPage::Extensions,
    )));
    let extensions = active_id(&screen);
    assert_ne!(extensions, web);
    assert_eq!(
        shell.items.tab(extensions).unwrap().content,
        TabContent::BrowserOwned(BrowserOwnedTab::Extensions)
    );
    assert!(shell.items.tab(extensions).unwrap().url.is_none());
    shell.handle_operation(Command::ShowBrowserPage(Some(
        crate::BrowserPage::Extensions,
    )));
    assert_eq!(active_id(&screen), extensions);
    shell.handle_operation(Command::ShowBrowserPage(Some(crate::BrowserPage::Settings)));
    assert_eq!(active_id(&screen), extensions);
    assert_eq!(
        shell.active_browser_page(),
        Some(crate::BrowserPage::Settings)
    );
    shell.handle_operation(Command::ShowBrowserPage(None));
    assert_eq!(active_id(&screen), extensions);
    assert_eq!(
        shell.active_browser_page(),
        Some(crate::BrowserPage::Extensions)
    );
    shell.handle_operation(Command::Activate(web));
    assert_eq!(active_id(&screen), web);
    assert_eq!(shell.active_browser_page(), None);
    assert_eq!(
        engine
            .calls()
            .iter()
            .rev()
            .find(|call| call.starts_with("layout@")),
        Some(&format!("layout@{window} {web}"))
    );
}

#[test]
fn trusted_extension_page_closure_removes_only_matching_typed_marker() {
    use zephium_core::item::TabContent;

    let (mut shell, _engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    let web = active_id(&screen);
    let (profile, space) = shell
        .windows
        .focused()
        .map(|window| (window.profile, window.space))
        .unwrap();
    let guest = ItemId::from(91);
    assert!(shell.items.insert_extension_tab(
        guest,
        Placement::Space {
            space,
            section: SpaceSection::Today
        },
    ));
    assert!(shell.items.adopt_extension_view(guest));
    let effects = shell.focus_tab(guest);
    let _ = shell.commit(effects);
    assert_eq!(active_id(&screen), guest);
    assert_eq!(
        shell.items.tab(guest).unwrap().content,
        TabContent::ExtensionOwned
    );
    shell.handle(Command::Engine(EngineEvent::ExtensionPageChanged {
        profile: ProfileId::from(900),
        id: guest,
        title: "Wrong profile".into(),
        loading: true,
        can_go_back: true,
        can_go_forward: true,
    }));
    assert_eq!(shell.items.tab(guest).unwrap().title, "Extension");
    shell.handle(Command::Engine(EngineEvent::ExtensionPageChanged {
        profile,
        id: guest,
        title: "\u{202e}Options".into(),
        loading: true,
        can_go_back: true,
        can_go_forward: false,
    }));
    let changed = shell.items.tab(guest).unwrap();
    assert_eq!(changed.title, "Options");
    assert!(changed.loading);
    assert!(changed.can_go_back);
    assert!(!changed.can_go_forward);
    assert!(changed.url.is_none());
    shell.handle(Command::Engine(EngineEvent::ExtensionPageClosed {
        profile: ProfileId::from(900),
        id: guest,
    }));
    assert!(shell.items.tab(guest).is_some());
    shell.handle(Command::Engine(EngineEvent::ExtensionPageClosed {
        profile,
        id: web,
    }));
    assert!(shell.items.tab(web).is_some());
    shell.handle(Command::Engine(EngineEvent::ExtensionPageClosed {
        profile,
        id: guest,
    }));
    assert!(shell.items.tab(guest).is_none());
    assert_eq!(active_id(&screen), web);

    let other_profile = ProfileId::from(901);
    let other_space = SpaceId::from(902);
    assert!(shell.profiles.insert(zephium_core::profiles::Profile {
        id: other_profile,
        name: "Other".into(),
        kind: ProfileKind::Named,
    }));
    assert!(shell.spaces.insert(zephium_core::spaces::Space {
        id: other_space,
        profile: other_profile,
        name: "Other".into(),
    }));
    let inactive_guest = ItemId::from(903);
    assert!(shell.items.insert_extension_tab(
        inactive_guest,
        Placement::Space {
            space: other_space,
            section: SpaceSection::Today
        },
    ));
    shell.handle(Command::Engine(EngineEvent::ExtensionPageClosed {
        profile: other_profile,
        id: inactive_guest,
    }));
    assert!(shell.items.tab(inactive_guest).is_none());
    assert_eq!(active_id(&screen), web);
}

#[test]
fn extension_guest_activation_from_extensions_waits_for_chrome_restore_barrier() {
    let (mut shell, engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    shell.handle_operation(Command::ShowBrowserPage(Some(
        crate::BrowserPage::Extensions,
    )));
    let extensions = active_id(&screen);
    let (space, window) = shell
        .windows
        .focused()
        .map(|window| (window.space, window.id))
        .unwrap();
    let guest = ItemId::from(904);
    assert!(shell.items.insert_extension_tab(
        guest,
        Placement::Space {
            space,
            section: SpaceSection::Today
        },
    ));
    let _ = shell.commit(Vec::new());
    assert_eq!(active_id(&screen), extensions);
    assert_eq!(
        shell.active_browser_page(),
        Some(crate::BrowserPage::Extensions)
    );
    assert!(!shell.items.tab(guest).unwrap().has_view());
    assert_eq!(
        engine
            .calls()
            .iter()
            .rev()
            .find(|call| call.starts_with("layout@")),
        Some(&format!("layout@{window} ")),
    );
    // Native settlement reserves the exact guest id before foregrounding it.
    assert!(shell.items.adopt_extension_view(guest));
    let before = shell.browser_return_revision;
    let disposition = shell.operation_activate(guest);
    assert_eq!(disposition.outcome, OperationOutcome::Deferred);
    assert_eq!(shell.browser_return_revision, before + 1);
    assert_ne!(guest, extensions);
    assert_eq!(active_id(&screen), guest);
    assert_eq!(shell.active_browser_page(), None);
    assert_eq!(
        engine
            .calls()
            .iter()
            .rev()
            .find(|call| call.starts_with("layout@")),
        Some(&format!("layout@{window} {guest}")),
    );
}

#[test]
fn native_guest_title_arriving_during_chrome_restore_keeps_exact_activation() {
    let (mut shell, engine, chrome, screen) = setup_with_async_chrome();
    shell.handle(Command::Bootstrap);
    shell.handle_operation(Command::ShowBrowserPage(Some(
        crate::BrowserPage::Extensions,
    )));
    let extensions = active_id(&screen);
    let (profile, space, window) = shell
        .windows
        .focused()
        .map(|window| (window.profile, window.space, window.id))
        .unwrap();
    let guest = ItemId::from(906);
    assert!(shell.items.insert_extension_tab(
        guest,
        Placement::Space {
            space,
            section: SpaceSection::Today,
        },
    ));
    let _ = shell.commit(Vec::new());
    assert!(shell.items.adopt_extension_view(guest));
    assert_eq!(
        shell.operation_activate(guest).outcome,
        OperationOutcome::Deferred
    );
    assert_eq!(active_id(&screen), extensions);
    assert!(shell.browser_return.is_some());

    shell.handle(Command::Engine(EngineEvent::ExtensionPageChanged {
        profile,
        id: guest,
        title: "Vimium".into(),
        loading: false,
        can_go_back: false,
        can_go_forward: false,
    }));
    assert_eq!(shell.items.tab(guest).unwrap().title, "Vimium");
    let (revision, snapshot) = chrome.complete_browser_return(true);
    assert_eq!(
        snapshot.active.as_deref(),
        Some(extensions.to_string().as_str())
    );
    assert!(snapshot.tabs.iter().any(|tab| {
        tab.id == guest.to_string()
            && tab.content == zephium_ipc::TabContentView::ExtensionOwned
            && tab.title == "Extension"
            && tab.url.is_none()
    }));
    shell.browser_chrome_restored(revision, true);
    assert!(shell.browser_return.is_none());
    assert_eq!(active_id(&screen), guest);
    assert_eq!(shell.active_browser_page(), None);
    assert_eq!(
        engine
            .calls()
            .iter()
            .rev()
            .find(|call| call.starts_with("layout@")),
        Some(&format!("layout@{window} {guest}")),
    );
}

#[test]
fn closed_guest_during_chrome_restore_cannot_activate_a_missing_marker() {
    let (mut shell, _engine, chrome, screen) = setup_with_async_chrome();
    shell.handle(Command::Bootstrap);
    shell.handle_operation(Command::ShowBrowserPage(Some(
        crate::BrowserPage::Extensions,
    )));
    let extensions = active_id(&screen);
    let (profile, space) = shell
        .windows
        .focused()
        .map(|window| (window.profile, window.space))
        .unwrap();
    let guest = ItemId::from(907);
    assert!(shell.items.insert_extension_tab(
        guest,
        Placement::Space {
            space,
            section: SpaceSection::Today,
        },
    ));
    let _ = shell.commit(Vec::new());
    assert!(shell.items.adopt_extension_view(guest));
    assert_eq!(
        shell.operation_activate(guest).outcome,
        OperationOutcome::Deferred
    );
    shell.close_extension_owned_marker(profile, guest);
    assert!(shell.items.tab(guest).is_none());
    let (revision, _) = chrome.complete_browser_return(true);
    shell.browser_chrome_restored(revision, true);
    assert_eq!(active_id(&screen), extensions);
    assert!(shell.browser_after_return.is_none());
    assert_eq!(
        shell.active_browser_page(),
        Some(crate::BrowserPage::Extensions)
    );
}

#[test]
fn address_submission_on_extension_guest_opens_an_ordinary_tab() {
    let (mut shell, _engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    let space = shell.windows.focused().unwrap().space;
    let guest = ItemId::from(905);
    assert!(shell.items.insert_extension_tab(
        guest,
        Placement::Space {
            space,
            section: SpaceSection::Today
        },
    ));
    assert!(shell.items.adopt_extension_view(guest));
    let effects = shell.focus_tab(guest);
    let _ = shell.commit(effects);
    let disposition = shell.operation_navigate(guest, "https://example.test/".into());
    assert_eq!(disposition.outcome, OperationOutcome::Deferred);
    let ordinary = active_id(&screen);
    assert_ne!(ordinary, guest);
    assert_eq!(
        shell.items.tab(ordinary).unwrap().content,
        zephium_core::item::TabContent::Web
    );
    assert_eq!(
        shell.items.tab(guest).unwrap().content,
        zephium_core::item::TabContent::ExtensionOwned
    );
    assert!(shell.items.tab(guest).unwrap().url.is_none());
}

#[test]
fn new_web_tab_from_settings_waits_for_chrome_restore_barrier() {
    let (mut shell, _engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    shell.handle_operation(Command::ShowBrowserPage(Some(crate::BrowserPage::Settings)));
    let prior_web = active_id(&screen);
    assert_eq!(shell.items_snapshot().unwrap().tabs.len(), 1);
    let before = shell.browser_return_revision;
    let disposition = shell.operation_open_url("https://example.test/".into(), true);
    assert_eq!(disposition.outcome, OperationOutcome::Deferred);
    assert_eq!(shell.browser_return_revision, before + 1);
    let web = active_id(&screen);
    assert_ne!(web, prior_web);
    assert_eq!(shell.active_browser_page(), None);
    assert_eq!(
        shell.items.tab(web).unwrap().content,
        zephium_core::item::TabContent::Web
    );
    assert!(shell.items.tab(prior_web).is_some());
}

#[test]
fn disabled_work_rejects_native_entry_without_changing_browse() {
    let store = Arc::new(FakeStore::default());
    let (mut shell, engine, screen) = setup_with(store.clone());
    shell.handle(Command::Bootstrap);
    let tab = active_id(&screen);
    navigate_and_commit(&mut shell, tab, "work-disabled.example");
    store
        .work_disabled
        .store(true, std::sync::atomic::Ordering::Release);
    let before = engine.calls();
    let refused = shell.handle_operation(Command::ShowBrowserPage(Some(crate::BrowserPage::Work)));
    assert_eq!(refused.outcome, OperationOutcome::Rejected);
    assert_eq!(shell.active_browser_page(), None);
    assert_eq!(active_id(&screen), tab);
    assert_eq!(engine.calls(), before);
    let settings =
        shell.handle_operation(Command::ShowBrowserPage(Some(crate::BrowserPage::Settings)));
    assert_eq!(settings.outcome, OperationOutcome::Deferred);
}

#[test]
fn browser_page_return_accepts_a_retained_single_leaf_split() {
    let (mut shell, _, screen) = setup();
    shell.handle(Command::Bootstrap);
    let id = active_id(&screen);
    navigate_and_commit(&mut shell, id, "settings-return.example");
    // Removing one side of a split can retain its remaining leaf. The
    // projection correctly represents that as no visible split group.
    shell.windows.focused_mut().unwrap().splits = Some(zephium_core::split::Pane::leaf(id));
    shell.handle_operation(Command::ShowBrowserPage(Some(crate::BrowserPage::Settings)));
    shell.handle_operation(Command::ShowBrowserPage(None));
    assert_eq!(shell.active_browser_page(), None);
    assert_eq!(active_id(&screen), id);
}

#[test]
fn browser_page_rejected_native_layout_retains_previous_destination() {
    let (mut shell, engine, _) = setup();
    shell.handle(Command::Bootstrap);
    shell.handle_operation(Command::ShowBrowserPage(Some(crate::BrowserPage::History)));
    engine
        .reject_native_dispatch
        .store(true, std::sync::atomic::Ordering::Release);
    let rejected =
        shell.handle_operation(Command::ShowBrowserPage(Some(crate::BrowserPage::Settings)));
    assert_eq!(rejected.outcome, OperationOutcome::NativeAdmissionFailed);
    assert_eq!(
        shell.active_browser_page(),
        Some(crate::BrowserPage::History)
    );
    let rejected_close = shell.handle_operation(Command::ShowBrowserPage(None));
    assert_eq!(rejected_close.outcome, OperationOutcome::Deferred);
    assert_eq!(
        shell.active_browser_page(),
        Some(crate::BrowserPage::History)
    );
}

#[test]
fn browser_page_tab_activation_restores_browsing_without_navigation() {
    let (mut shell, _, screen) = setup();
    shell.handle(Command::Bootstrap);
    let id = active_id(&screen);
    navigate_and_commit(&mut shell, id, "tab-return.example");
    shell.handle_operation(Command::ShowBrowserPage(Some(crate::BrowserPage::Settings)));
    shell.handle_operation(Command::Activate(id));
    assert_eq!(shell.active_browser_page(), None);
    assert_eq!(active_id(&screen), id);
    assert_eq!(
        shell
            .items
            .tab(id)
            .unwrap()
            .url
            .as_ref()
            .map(url::Url::as_str),
        Some("https://tab-return.example/")
    );
}

#[test]
fn work_citation_navigation_restores_browse_and_preserves_existing_tab() {
    for reject in [false, true] {
        let (mut shell, engine, screen) = setup();
        shell.handle(Command::Bootstrap);
        let original = active_id(&screen);
        navigate_and_commit(&mut shell, original, "original.example");
        shell.handle_operation(Command::ShowBrowserPage(Some(crate::BrowserPage::Work)));
        let before = shell.items_snapshot().unwrap().tabs.len();
        engine
            .reject_native_dispatch
            .store(reject, std::sync::atomic::Ordering::Release);
        let result = shell.handle_operation(Command::OpenUrl {
            input: "https://github.com/sveltejs/svelte/issues/18096".into(),
            new_tab: true,
        });
        assert_eq!(result.outcome, OperationOutcome::Deferred);
        assert_eq!(
            shell
                .items
                .tab(original)
                .unwrap()
                .url
                .as_ref()
                .map(url::Url::as_str),
            Some("https://original.example/")
        );
        if reject {
            assert_eq!(shell.active_browser_page(), Some(crate::BrowserPage::Work));
            assert_eq!(shell.items_snapshot().unwrap().tabs.len(), before);
            assert_eq!(active_id(&screen), original);
        } else {
            assert_eq!(shell.active_browser_page(), None);
            assert_eq!(shell.items_snapshot().unwrap().tabs.len(), before + 1);
            let opened = active_id(&screen);
            assert_ne!(opened, original);
            assert!(engine.calls().iter().any(|call| call
                == &format!(
                    "create {opened} https://github.com/sveltejs/svelte/issues/18096 [default]"
                )));
        }
    }
}

#[test]
fn essential_move_preserves_the_active_native_tab_and_can_be_reversed() {
    let (mut shell, engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    let id = active_id(&screen);
    navigate_and_commit(&mut shell, id, "essential.example");
    let profile = shell.windows.focused().unwrap().profile;
    let original_url = shell.items.tab(id).unwrap().url.clone();
    let before = engine.calls().len();
    let result = shell.handle_operation(Command::SetTabEssential {
        id,
        essential: true,
        before: None,
    });
    assert!(matches!(
        result.outcome,
        OperationOutcome::Applied | OperationOutcome::Deferred
    ));
    assert_eq!(
        shell.items.get(id).unwrap().placement,
        Placement::Favorites { profile }
    );
    assert_eq!(shell.items.tab(id).unwrap().url, original_url);
    assert!(shell.items.tab(id).unwrap().has_view());
    assert_eq!(active_id(&screen), id);
    assert!(!engine.calls()[before..]
        .iter()
        .any(|call| call.starts_with("close ") || call.starts_with("create ")));
    shell.handle_operation(Command::SetTabEssential {
        id,
        essential: false,
        before: None,
    });
    assert!(matches!(
        shell.items.get(id).unwrap().placement,
        Placement::Space {
            section: SpaceSection::Today,
            ..
        }
    ));
}

#[test]
fn essential_move_rejects_foreign_and_invalid_drop_targets() {
    let (mut shell, _, screen) = setup();
    shell.handle(Command::Bootstrap);
    let id = active_id(&screen);
    let placement = shell.items.get(id).unwrap().placement;
    let denied = shell.handle_operation(Command::SetTabEssential {
        id,
        essential: true,
        before: Some(ItemId::from(u128::MAX)),
    });
    assert_eq!(denied.outcome, OperationOutcome::Rejected);
    assert_eq!(shell.items.get(id).unwrap().placement, placement);
    let denied = shell.handle_operation(Command::SetTabEssential {
        id: ItemId::from(u128::MAX),
        essential: true,
        before: None,
    });
    assert_eq!(denied.outcome, OperationOutcome::Rejected);
    let foreign_profile = add_inactive_named_profile(&mut shell, 900);
    let foreign_tab = ItemId::from(902_u128);
    assert!(shell.items.insert_tab(
        foreign_tab,
        Placement::Favorites {
            profile: foreign_profile
        }
    ));
    let denied = shell.handle_operation(Command::SetTabEssential {
        id: foreign_tab,
        essential: true,
        before: None,
    });
    assert_eq!(denied.reason, OperationReason::InvalidScope);
    assert_eq!(
        shell.items.get(foreign_tab).unwrap().placement,
        Placement::Favorites {
            profile: foreign_profile
        }
    );
}

#[test]
fn essential_move_survives_session_restore() {
    let store = Arc::new(FakeStore::default());
    let (mut shell, _, screen) = setup_with(store.clone());
    shell.handle(Command::Bootstrap);
    let id = active_id(&screen);
    navigate_and_commit(&mut shell, id, "saved-essential.example");
    let profile = shell.windows.focused().unwrap().profile;
    shell.handle_operation(Command::SetTabEssential {
        id,
        essential: true,
        before: None,
    });
    shell.persist();
    let (mut restored, _, _) = setup_with(store);
    restored.handle(Command::Bootstrap);
    assert_eq!(
        restored.items.get(id).unwrap().placement,
        Placement::Favorites { profile }
    );
    assert_eq!(
        restored
            .items
            .tab(id)
            .unwrap()
            .url
            .as_ref()
            .map(url::Url::as_str),
        Some("https://saved-essential.example/")
    );
}

#[test]
fn scoped_launcher_actions_reject_stale_requests_and_unoffered_actions() {
    let (mut shell, _, screen) = setup();
    shell.handle(Command::Bootstrap);
    let id = active_id(&screen);
    let window = shell.windows.focused().unwrap();
    let context = zephium_ipc::SearchContext {
        window_id: window.id.to_string(),
        session_id: "0000000000000001".into(),
        request_id: "one".into(),
        profile_id: window.profile.to_string(),
        space_id: window.space.to_string(),
    };
    shell.handle(Command::SearchScoped {
        query: String::new(),
        context: Box::new(context.clone()),
    });
    assert!(shell.search.results.iter().any(|result| matches!(&result.action,SearchAction::ActivateTab{id:found} if *found==id.to_string())));
    let forged = shell.handle_operation(Command::RunSearchAction {
        context: Box::new(context.clone()),
        action: SearchAction::OpenUrl {
            url: "https://not-in-results.example/".into(),
        },
        background: false,
    });
    assert_eq!(forged.reason, OperationReason::InvalidScope);
    shell.handle(Command::CancelSearch {
        session_id: context.session_id.clone(),
    });
    let stale = shell.handle_operation(Command::RunSearchAction {
        context: Box::new(context),
        action: SearchAction::ActivateTab { id: id.to_string() },
        background: false,
    });
    assert_eq!(stale.reason, OperationReason::InvalidScope);
}

#[test]
fn a_background_launcher_open_keeps_the_current_tab_and_the_search() {
    let (mut shell, _, screen) = setup();
    shell.handle(Command::Bootstrap);
    let active = active_id(&screen);
    let window = shell.windows.focused().unwrap();
    let context = zephium_ipc::SearchContext {
        window_id: window.id.to_string(),
        session_id: "0000000000000001".into(),
        request_id: "one".into(),
        profile_id: window.profile.to_string(),
        space_id: window.space.to_string(),
    };
    shell.handle(Command::SearchScoped {
        query: "example.com".into(),
        context: Box::new(context.clone()),
    });
    let action = shell
        .search
        .results
        .iter()
        .find(|result| matches!(result.action, SearchAction::OpenUrl { .. }))
        .map(|result| result.action.clone())
        .expect("the typed address is offered");
    let before = shell.items.view_ids().len();
    for _ in 0..2 {
        let opened = shell.handle_operation(Command::RunSearchAction {
            context: Box::new(context.clone()),
            action: action.clone(),
            background: true,
        });
        // Deferred: the new view settles once native has created it.
        assert!(matches!(
            opened.outcome,
            OperationOutcome::Applied | OperationOutcome::Deferred
        ));
    }
    assert_eq!(shell.windows.focused().unwrap().active, Some(active));
    assert_eq!(shell.items.view_ids().len(), before + 2);
    // The launcher is still showing these rows, so they must still admit.
    assert_eq!(shell.search.context.as_ref(), Some(&context));
}

#[test]
fn launcher_includes_essentials_and_folder_tabs_from_the_current_scope() {
    let (mut shell, _, screen) = setup();
    shell.handle(Command::Bootstrap);
    let id = active_id(&screen);
    shell.handle_operation(Command::SetTabEssential {
        id,
        essential: true,
        before: None,
    });
    shell.handle(Command::Search(String::new()));
    assert!(shell.search.results.iter().any(|result| matches!(&result.action,SearchAction::ActivateTab{id:found} if *found==id.to_string())));
}

#[test]
fn sidebar_shape_slides_precede_layout() {
    let (mut shell, engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    let id = active_id(&screen);
    navigate_and_commit(&mut shell, id, "travel.example");
    let window = shell.windows.focused().unwrap().id;
    let motion = |engine: &FakeEngine| {
        engine
            .calls()
            .into_iter()
            .filter(|call| call.starts_with("motion@"))
            .collect::<Vec<_>>()
    };

    shell.handle(Command::SetSidebarWidth(300.0, false));
    assert!(
        motion(&engine).is_empty(),
        "an ordinary nonanimated width change does not request a shape journey"
    );

    shell.handle(Command::SetSidebarWidth(56.0, true));
    assert_eq!(motion(&engine), [format!("motion@{window} Slide")]);
    // The hint precedes the layout it belongs to.
    let calls = engine.calls();
    let hinted = calls
        .iter()
        .rposition(|call| call.starts_with("motion@"))
        .unwrap();
    let laid = calls
        .iter()
        .rposition(|call| call.starts_with("layout@"))
        .unwrap();
    assert!(hinted < laid);
}

#[test]
fn sidebar_resize_guide_does_not_mutate_width_or_layout() {
    let (mut shell, engine, _) = setup();
    shell.handle(Command::Bootstrap);
    let width = shell.windows.focused().unwrap().metrics.sidebar_width;
    let layouts = engine
        .calls()
        .iter()
        .filter(|call| call.starts_with("layout@"))
        .count();
    shell.handle(Command::SidebarResizeGuide(Some(310.0)));
    shell.handle(Command::SidebarResizeGuide(None));
    assert_eq!(
        shell.windows.focused().unwrap().metrics.sidebar_width,
        width
    );
    assert_eq!(
        engine
            .calls()
            .iter()
            .filter(|call| call.starts_with("layout@"))
            .count(),
        layouts
    );
    assert!(engine
        .calls()
        .iter()
        .any(|call| call.starts_with("resize-guide@") && call.ends_with("None")));
}

#[test]
fn returning_from_a_browser_page_brings_the_content_back_into_view() {
    let (mut shell, engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    let id = active_id(&screen);
    navigate_and_commit(&mut shell, id, "arrive.example");
    let window = shell.windows.focused().unwrap().id;
    shell.handle_operation(Command::ShowBrowserPage(Some(crate::BrowserPage::Settings)));
    assert!(
        !engine
            .calls()
            .iter()
            .any(|call| call.starts_with("motion@")),
        "leaving for a browser page hides the content at once"
    );
    shell.handle_operation(Command::ShowBrowserPage(None));
    let calls = engine.calls();
    let arrive = calls
        .iter()
        .rposition(|call| call == &format!("motion@{window} Arrive"))
        .expect("return hints an arrival");
    let shown = calls
        .iter()
        .rposition(|call| call == &format!("layout@{window} {id}"))
        .unwrap();
    assert!(arrive < shown);
}

#[test]
fn browser_settings_return_to_the_last_browsing_tab_after_chrome_ack() {
    let (mut shell, _engine, chrome, screen) = setup_with_async_chrome();
    shell.handle(Command::Bootstrap);
    let oldest = active_id(&screen);
    shell.handle(Command::Open);
    let recent = active_id(&screen);
    assert_ne!(recent, oldest);
    shell.handle_operation(Command::ShowBrowserPage(Some(crate::BrowserPage::Settings)));
    shell.handle_operation(Command::ShowBrowserPage(Some(
        crate::BrowserPage::Extensions,
    )));
    let manager = active_id(&screen);
    shell.handle_operation(Command::ShowBrowserPage(None));
    assert_eq!(active_id(&screen), manager);
    let (revision, _) = chrome.complete_browser_return(true);
    shell.browser_chrome_restored(revision, true);
    assert_eq!(active_id(&screen), recent);
}

#[test]
fn keyboard_selection_walks_the_sidebar_from_favorites_through_pinned_folders() {
    let profile = ProfileId::from(51_001);
    let space = SpaceId::from(52_001);
    let favorite = ItemId::from(53_001);
    let pinned_folder = ItemId::from(53_002);
    let pinned_child = ItemId::from(53_003);
    let today_first = ItemId::from(53_004);
    let today_last = ItemId::from(53_005);
    let item = |id, parent, placement, kind| PersistedItem {
        id,
        parent,
        placement,
        kind,
    };
    let page = |host: &str| PersistedKind::Tab {
        url: format!("https://{host}/"),
        title: host.into(),
        zoom: 1.0,
    };
    let pinned = Placement::Space {
        space,
        section: SpaceSection::Pinned,
    };
    let today = Placement::Space {
        space,
        section: SpaceSection::Today,
    };
    let store = Arc::new(FakeStore {
        saved: Mutex::new(Some(SessionState {
            profiles: vec![PersistedProfile {
                id: profile,
                name: "Personal".into(),
                kind: ProfileKind::Default,
            }],
            spaces: vec![PersistedSpace {
                id: space,
                profile,
                name: "Main".into(),
            }],
            items: vec![
                item(
                    favorite,
                    None,
                    Placement::Favorites { profile },
                    page("favorite.example"),
                ),
                item(
                    pinned_folder,
                    None,
                    pinned,
                    PersistedKind::Folder {
                        name: "Work".into(),
                    },
                ),
                item(
                    pinned_child,
                    Some(pinned_folder),
                    pinned,
                    page("pinned.example"),
                ),
                item(today_first, None, today, page("first.example")),
                item(today_last, None, today, page("last.example")),
            ],
            active_space: Some(space),
            active_item: Some(today_first),
            splits: None,
            recently_closed: Vec::new(),
        })),
        ..Default::default()
    });
    let (mut shell, _engine, screen) = setup_with(store);
    shell.handle(Command::Bootstrap);

    shell.handle(Command::Run("tab.select.1".into()));
    assert_eq!(active_id(&screen), favorite);
    shell.handle(Command::Run("tab.select.2".into()));
    assert_eq!(active_id(&screen), pinned_child);
    shell.handle(Command::Run("tab.select.last".into()));
    assert_eq!(active_id(&screen), today_last);

    // Cycling continues from a pinned tab instead of stopping there.
    shell.handle(Command::Run("tab.select.2".into()));
    shell.handle(Command::Run("tab.next".into()));
    assert_eq!(active_id(&screen), today_first);
    shell.handle(Command::Run("tab.previous".into()));
    shell.handle(Command::Run("tab.previous".into()));
    assert_eq!(active_id(&screen), favorite);

    // A position past the end, or outside the registry, changes nothing.
    let beyond = shell.handle_operation(Command::Run("tab.select.8".into()));
    assert_eq!(beyond.outcome, OperationOutcome::NoOp);
    let bogus = shell.handle_operation(Command::Run("tab.select.0".into()));
    assert_eq!(bogus.outcome, OperationOutcome::Rejected);
    assert_eq!(active_id(&screen), favorite);
}

#[test]
fn links_from_other_apps_wait_for_the_session_then_open_in_new_tabs() {
    let (mut shell, engine, screen) = setup();
    shell.handle(Command::OpenExternal(vec![
        "https://early.example/".into(),
        "javascript:alert(1)".into(),
    ]));
    let loaded = |host: &str| {
        engine.calls().iter().any(|call| {
            (call.starts_with("create ") || call.starts_with("navigate ")) && call.contains(host)
        })
    };
    assert!(!loaded("early.example"));

    shell.handle(Command::Bootstrap);
    assert!(loaded("early.example"));
    assert!(!engine
        .calls()
        .iter()
        .any(|call| call.contains("javascript")));
    let early = active_id(&screen);

    shell.handle(Command::OpenExternal(vec!["https://late.example/".into()]));
    assert!(loaded("late.example"));
    assert_ne!(active_id(&screen), early);
}

#[test]
fn find_follows_the_page_in_front_and_answers_only_for_it() {
    let engine = Arc::new(FakeEngine::default());
    let results: Arc<Mutex<Vec<zephium_ipc::FindResultView>>> = Arc::default();
    let screen: Screen = Arc::new(Mutex::new(ItemsState {
        projection_revision: String::new(),
        profile: None,
        spaces: Vec::new(),
        active_space_id: None,
        nodes: Vec::new(),
        tabs: Vec::new(),
        active: None,
        split_group: None,
    }));
    let (sink, view) = (results.clone(), screen.clone());
    let mut shell = Shell::new_with_failure(
        engine.clone(),
        Arc::new(FakeStore::default()),
        Arc::new(ImmediateAllowAllCompiler),
        Box::new(|_| {}),
        Arc::new(FakeChrome),
        Box::new(move |projection| match projection {
            Projection::FindResult(result) => sink.lock().unwrap().push(result),
            other => apply_projection(&mut view.lock().unwrap(), other),
        }),
    );
    shell.handle(Command::SetWindowSize(Size::new(1200.0, 800.0)));
    shell.handle(Command::Bootstrap);
    let first = active_id(&screen);
    shell.handle(Command::Run("tab.new".into()));
    let second = active_id(&screen);
    let request = |query: &str, forward| {
        Some(zephium_core::ports::engine::FindRequest {
            query: query.into(),
            forward,
        })
    };

    shell.handle(Command::Find(request("apple", true)));
    shell.handle(Command::Find(request("apple", false)));
    let result = |id, matches| {
        Command::Engine(EngineEvent::FindResult {
            id,
            query: "apple".into(),
            matches,
            active: Some(1),
        })
    };
    shell.handle(result(second, 3));

    // Switching pages ends the search where it was.
    shell.handle(Command::Activate(first));
    shell.handle(Command::Find(request("pear", true)));
    shell.handle(result(second, 9));
    shell.handle(Command::Find(None));

    let calls = engine.calls();
    let finds: Vec<&String> = calls
        .iter()
        .filter(|call| call.starts_with("find "))
        .collect();
    assert_eq!(
        finds,
        [
            &format!("find {second} apple next"),
            &format!("find {second} apple previous"),
            &format!("find {second} end"),
            &format!("find {first} pear next"),
            &format!("find {first} end"),
        ]
    );
    assert_eq!(
        results.lock().unwrap().as_slice(),
        [zephium_ipc::FindResultView {
            query: "apple".into(),
            matches: 3,
            active: Some(1),
        }]
    );
}

#[test]
fn tab_preferences_place_new_tabs_and_choose_who_follows_a_close() {
    let store = Arc::new(FakeStore::default());
    store
        .settings
        .lock()
        .unwrap()
        .insert("tabs.new-position".into(), "after-current".into());
    let (mut shell, _engine, screen) = setup_with(store);
    shell.handle(Command::Bootstrap);
    let first = active_id(&screen);
    shell.handle(Command::Run("tab.new".into()));
    let end = active_id(&screen);
    shell.handle(Command::Activate(first));
    shell.handle(Command::Run("tab.new".into()));
    let middle = active_id(&screen);
    let order = |screen: &Screen| {
        last(screen)
            .nodes
            .iter()
            .filter(|node| node.section == SidebarSectionView::Today && node.parent_id.is_none())
            .filter_map(|node| ItemId::parse(&node.id))
            .collect::<Vec<_>>()
    };
    assert_eq!(order(&screen), vec![first, middle, end]);

    // Previous: closing the middle tab lands on the one above it.
    shell.handle(Command::SetAppSetting {
        key: "tabs.after-close".into(),
        value: "previous".into(),
    });
    shell.handle(Command::Close(middle));
    assert_eq!(active_id(&screen), first);

    // Recent: closing goes back to the tab used before, wherever it is.
    shell.handle(Command::SetAppSetting {
        key: "tabs.after-close".into(),
        value: "recent".into(),
    });
    shell.handle(Command::Activate(end));
    shell.handle(Command::Run("tab.new".into()));
    let newest = active_id(&screen);
    shell.handle(Command::Activate(first));
    shell.handle(Command::Activate(newest));
    shell.handle(Command::Close(newest));
    assert_eq!(active_id(&screen), first);
}

#[test]
fn opening_an_address_already_open_goes_to_its_tab_unless_turned_off() {
    let (mut shell, _engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    let first = active_id(&screen);
    shell.handle(Command::Navigate {
        id: first,
        input: "https://docs.example/".into(),
    });
    shell.handle(Command::Engine(EngineEvent::UrlChanged {
        id: first,
        url: "https://docs.example/".into(),
    }));
    shell.handle(Command::Run("tab.new".into()));
    let blank = active_id(&screen);

    shell.handle(Command::OpenUrl {
        input: "https://docs.example/".into(),
        new_tab: true,
    });
    assert_eq!(active_id(&screen), first);

    shell.handle(Command::SetAppSetting {
        key: "tabs.switch-to-open".into(),
        value: "false".into(),
    });
    shell.handle(Command::OpenUrl {
        input: "https://docs.example/".into(),
        new_tab: true,
    });
    let opened = active_id(&screen);
    assert_ne!(opened, first);
    assert_ne!(opened, blank);
}

fn open_order(screen: &Screen) -> Vec<ItemId> {
    last(screen)
        .nodes
        .iter()
        .filter(|node| node.section == SidebarSectionView::Today && node.parent_id.is_none())
        .filter_map(|node| ItemId::parse(&node.id))
        .collect()
}

#[test]
fn duplicating_a_tab_opens_its_page_right_after_it() {
    let (mut shell, engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    let first = active_id(&screen);
    navigate_and_commit(&mut shell, first, "https://docs.example/");
    shell.handle(Command::Run("tab.new".into()));
    let second = active_id(&screen);

    let duplicated = shell.handle_operation(Command::TabAction {
        id: first,
        action: crate::TabAction::Duplicate,
    });
    assert_eq!(duplicated.outcome, OperationOutcome::Deferred);
    let copy = active_id(&screen);
    assert_eq!(open_order(&screen), vec![first, copy, second]);
    assert!(engine
        .calls()
        .iter()
        .any(|line| line.contains(&copy.to_string()) && line.contains("docs.example")));

    // A tab with no page has nothing to duplicate.
    let blank = shell.handle_operation(Command::TabAction {
        id: second,
        action: crate::TabAction::Duplicate,
    });
    assert_eq!(blank.outcome, OperationOutcome::NoOp);
}

#[test]
fn closing_other_tabs_or_those_below_keeps_the_chosen_one_in_front() {
    let (mut shell, _engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    let first = active_id(&screen);
    shell.handle(Command::Run("tab.new".into()));
    let second = active_id(&screen);
    shell.handle(Command::Run("tab.new".into()));
    let third = active_id(&screen);
    shell.handle(Command::Run("tab.new".into()));
    let fourth = active_id(&screen);
    assert_eq!(open_order(&screen), vec![first, second, third, fourth]);

    shell.handle(Command::TabAction {
        id: second,
        action: crate::TabAction::CloseBelow,
    });
    assert_eq!(open_order(&screen), vec![first, second]);
    assert_eq!(active_id(&screen), second);

    let nothing_below = shell.handle_operation(Command::TabAction {
        id: second,
        action: crate::TabAction::CloseBelow,
    });
    assert_eq!(nothing_below.outcome, OperationOutcome::NoOp);

    shell.handle(Command::Activate(first));
    shell.handle(Command::TabAction {
        id: second,
        action: crate::TabAction::CloseOthers,
    });
    assert_eq!(open_order(&screen), vec![second]);
    assert_eq!(active_id(&screen), second);
    let _ = third;
}

#[test]
fn a_new_tab_opened_from_settings_fills_the_window_with_the_new_tab() {
    let (mut shell, engine, chrome, screen) = setup_with_async_chrome();
    shell.handle(Command::Bootstrap);
    let page = active_id(&screen);
    shell.handle(Command::Navigate {
        id: page,
        input: "example.com".into(),
    });
    shell.handle(Command::Engine(EngineEvent::UrlChanged {
        id: page,
        url: "https://example.com/".into(),
    }));
    present_committed(&mut shell, page, "https://example.com/");
    shell.handle_operation(Command::ShowBrowserPage(Some(crate::BrowserPage::Settings)));
    assert_eq!(
        shell.active_browser_page(),
        Some(crate::BrowserPage::Settings)
    );

    assert_eq!(
        shell.handle_operation(Command::Open).outcome,
        OperationOutcome::Deferred
    );
    let (revision, _) = chrome.complete_browser_return(true);
    shell.browser_chrome_restored(revision, true);

    let opened = active_id(&screen);
    assert_ne!(opened, page);
    assert_eq!(shell.active_browser_page(), None);
    let opened_view = last(&screen)
        .tabs
        .into_iter()
        .find(|tab| tab.id == opened.to_string())
        .unwrap();
    assert!(
        !opened_view.loading && opened_view.url.is_none(),
        "the frame shows its new tab only for an idle tab without a page: {opened_view:?}"
    );
    let frame = chrome.last_frame().expect("chrome was placed");
    assert!(
        frame.fill_width,
        "the frame draws the new tab across the window: {frame:?}"
    );
    let last_layout = engine
        .calls()
        .into_iter()
        .rev()
        .find(|call| call.starts_with("layout@"))
        .unwrap();
    assert!(!last_layout.contains(&page.to_string()), "{last_layout}");
}
