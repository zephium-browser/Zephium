file,
                    name: "Foreign".into(),
                    look: Default::default(),
                },
            ],
            items: vec![
                tab(local, local_space, "local.example"),
                tab(sibling, sibling_space, "sibling.example"),
                tab(foreign, foreign_space, "foreign.example"),
            ],
            active_space: Some(local_space),
            active_item: Some(foreign),
            splits: Some(Pane::Branch {
                axis: Axis::Row,
                ratio: 0.5,
                a: Box::new(Pane::Leaf(local)),
                b: Box::new(Pane::Leaf(sibling)),
            }),
            recently_closed: Vec::new(),
        })),
        ..Default::default()
    });

    store
        .settings
        .lock()
        .unwrap()
        .insert("tabs.startup".into(), "continue".into());
    let (mut shell, engine, screen) = setup_with(store);
    shell.handle(Command::Bootstrap);

    let win = shell.windows.focused().unwrap();
    assert_eq!(win.profile, local_profile);
    assert_eq!(win.space, local_space);
    assert_eq!(win.active, Some(local));
    assert!(win.splits.is_none());
    assert_eq!(active_id(&screen), local);
    assert_eq!(engine.last_layout(), vec![local.to_string()]);
    let calls = engine.calls();
    assert!(calls
        .iter()
        .any(|call| call.starts_with(&format!("create {local} "))));
    assert!(!calls
        .iter()
        .any(|call| call.starts_with(&format!("create {sibling} "))));
    assert!(!calls
        .iter()
        .any(|call| call.starts_with(&format!("create {foreign} "))));
}

#[test]
fn restart_discards_oversized_split_before_creating_views() {
    let store = Arc::new(FakeStore::default());
    store
        .settings
        .lock()
        .unwrap()
        .insert("tabs.startup".into(), "continue".into());
    let (mut shell, _engine, screen) = setup_with(store.clone());
    shell.handle(Command::Bootstrap);
    let mut ids = vec![active_id(&screen)];
    navigate_and_commit(&mut shell, ids[0], "pane0.example");
    for n in 1..=MAX_VISIBLE_PANES {
        shell.handle(Command::Open);
        let id = active_id(&screen);
        navigate_and_commit(&mut shell, id, &format!("pane{n}.example"));
        ids.push(id);
    }

    let mut state = store.saved.lock().unwrap().clone().unwrap();
    state.active_item = Some(ids[0]);
    state.splits = Some(
        ids[1..]
            .iter()
            .fold(Pane::Leaf(ids[0]), |tree, id| Pane::Branch {
                axis: Axis::Row,
                ratio: 0.5,
                a: Box::new(tree),
                b: Box::new(Pane::Leaf(*id)),
            }),
    );
    *store.saved.lock().unwrap() = Some(state);

    let (mut restored, engine, _screen) = setup_with(store);
    restored.handle(Command::Bootstrap);
    assert_eq!(engine.last_layout(), vec![ids[0].to_string()]);
    assert!(restored.windows.focused().unwrap().splits.is_none());
    assert_eq!(
        engine
            .calls()
            .iter()
            .filter(|call| call.starts_with("create "))
            .count(),
        1,
        "oversized restored panes must not trigger a controller storm"
    );
}

#[test]
fn bootstrap_is_idempotent_across_chrome_reloads() {
    let (mut shell, engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    let first = active_id(&screen);
    shell.handle(Command::Navigate {
        id: first,
        input: "example.com".into(),
    });

    shell.handle(Command::SetSidebarHidden(true));
    shell.handle(Command::PeekSidebar {
        peek: true,
        slide: false,
    });
    let window = shell.windows.focused().unwrap().id;
    // chrome webview reloaded (dev HMR): same window, same stage, state kept
    shell.handle(Command::Bootstrap);
    assert_eq!(
        engine
            .calls()
            .into_iter()
            .filter(|call| call.starts_with("raise@"))
            .collect::<Vec<_>>(),
        [
            format!("raise@{window} false"),
            format!("raise@{window} true"),
            format!("raise@{window} false")
        ]
    );
    assert_eq!(active_id(&screen), first);
    let windows: std::collections::HashSet<String> = engine
        .calls()
        .iter()
        .filter_map(|c| c.split(' ').next().map(String::from))
        .filter(|c| c.starts_with("layout@"))
        .collect();
    assert_eq!(windows.len(), 1, "one window, one stage: {windows:?}");
    assert_eq!(last(&screen).tabs.len(), 1);
}

#[test]
fn exhausted_space_aggregate_refuses_first_run_without_panicking() {
    let (mut shell, _engine, _screen) = setup();
    let profile = ProfileId::from(26_200);
    assert!(shell.profiles.insert(Profile {
        id: profile,
        name: "Personal".into(),
        kind: ProfileKind::Default,
    }));
    for index in 0..zephium_core::session::MAX_SESSION_SPACES {
        assert!(shell.spaces.insert(Space {
            id: SpaceId::from(27_000 + index as u128),
            profile,
            name: "Full".into(),
            look: Default::default(),
        }));
    }

    assert!(shell.create_default_space().is_none());
}

#[test]
fn failed_pending_profile_deletion_load_prevents_bootstrap_views_and_native_erasure() {
    let store = Arc::new(FakeStore::default());
    store
        .pending_load_failures
        .store(1, std::sync::atomic::Ordering::Release);
    let (mut shell, engine, screen) = setup_with(store);

    shell.handle(Command::Bootstrap);

    assert!(!shell.bootstrapped);
    assert!(shell.windows.focused().is_none());
    assert!(last(&screen).tabs.is_empty());
    let calls = engine.calls();
    assert!(!calls.iter().any(|call| call.starts_with("create ")));
    assert!(!calls.iter().any(|call| call.starts_with("erase-profile ")));
}

#[test]
fn failed_session_load_never_bootstraps_or_overwrites_storage() {
    let store = Arc::new(FakeStore::default());
    *store.load_failed.lock().unwrap() = true;
    let (mut shell, engine, screen) = setup_with(store.clone());

    shell.handle(Command::Bootstrap);
    shell.persist();

    assert!(!shell.bootstrapped);
    assert!(shell.windows.focused().is_none());
    assert!(last(&screen).tabs.is_empty());
    assert!(!store.events.lock().unwrap().contains(&"save"));
    assert!(engine.calls().is_empty());
}

#[test]
fn an_unrestorable_session_is_set_aside_and_the_browser_opens_with_its_profile() {
    let store = Arc::new(FakeStore::default());
    *store.recovery_reason.lock().unwrap() = Some("corrupt authoritative session snapshot".into());
    let profile = ProfileId::from(32_000);
    *store.set_aside.lock().unwrap() = Some(SessionState {
        profiles: vec![PersistedProfile {
            id: profile,
            name: "Personal".into(),
            kind: ProfileKind::Default,
        }],
        ..SessionState::default()
    });
    let (mut shell, _engine, _screen) = setup_with(store);

    shell.handle(Command::Bootstrap);

    assert!(shell.bootstrapped);
    assert!(shell.session_set_aside);
    assert_eq!(shell.windows.focused().unwrap().profile, profile);
}

#[test]
fn recovery_required_never_bootstraps_or_overwrites_storage_when_it_cannot_be_set_aside() {
    let store = Arc::new(FakeStore::default());
    *store.recovery_reason.lock().unwrap() = Some("snapshot is not canonical".into());
    let (mut shell, engine, screen) = setup_with(store.clone());

    shell.handle(Command::Bootstrap);
    shell.persist();

    assert!(!shell.bootstrapped);
    assert!(shell.windows.focused().is_none());
    assert!(last(&screen).tabs.is_empty());
    assert!(!store.events.lock().unwrap().contains(&"save"));
    assert!(engine.calls().is_empty());
}

#[test]
fn degraded_ancillary_profiles_are_recorded_without_blocking_exact_session_bootstrap() {
    let store = Arc::new(FakeStore::default());
    let profile = ProfileId::from(31_000);
    let space = SpaceId::from(31_001);
    *store.saved.lock().unwrap() = Some(SessionState {
        profiles: vec![PersistedProfile {
            id: profile,
            name: "Personal".into(),
            kind: ProfileKind::Default,
        }],
        spaces: vec![PersistedSpace {
            id: space,
            profile,
            name: "Space".into(),
            look: Default::default(),
        }],
        active_space: Some(space),
        ..SessionState::default()
    });
    store.degraded_profiles.lock().unwrap().push(profile);
    let (mut shell, _engine, _screen) = setup_with(store);

    shell.handle(Command::Bootstrap);

    assert!(shell.bootstrapped);
    assert_eq!(
        shell.degraded_storage_profiles,
        std::collections::HashSet::from([profile])
    );
    assert!(shell.profiles.get(profile).is_some());
}

#[test]
fn forged_degraded_profile_report_cannot_bootstrap_an_unrelated_session() {
    let store = Arc::new(FakeStore::default());
    let profile = ProfileId::from(32_000);
    let space = SpaceId::from(32_001);
    *store.saved.lock().unwrap() = Some(SessionState {
        profiles: vec![PersistedProfile {
            id: profile,
            name: "Personal".into(),
            kind: ProfileKind::Default,
        }],
        spaces: vec![PersistedSpace {
            id: space,
            profile,
            name: "Space".into(),
            look: Default::default(),
        }],
        active_space: Some(space),
        ..SessionState::default()
    });
    store
        .degraded_profiles
        .lock()
        .unwrap()
        .push(ProfileId::from(99_999));
    let (mut shell, engine, _screen) = setup_with(store);

    shell.handle(Command::Bootstrap);

    assert!(!shell.bootstrapped);
    assert!(shell.degraded_storage_profiles.is_empty());
    assert!(engine.calls().is_empty());
}

#[test]
fn persisted_new_tab_destination_is_loaded_at_startup_and_invalid_values_stay_blank() {
    for destination in [
        "https://start.example/",
        "file:///etc/hosts",
        "not an address",
    ] {
        let store = Arc::new(FakeStore::default());
        store
            .settings
            .lock()
            .unwrap()
            .insert("tabs.new-page".into(), destination.into());
        let (mut shell, engine, screen) = setup_with(store);
        shell.handle(Command::Bootstrap);
        let id = active_id(&screen);
        if destination.starts_with("https:") {
            assert!(engine
                .calls()
                .iter()
                .any(|call| call == &format!("create {id} {destination} [default]")));
        } else {
            assert!(shell.is_untouched_new_tab(id));
            assert!(engine
                .calls()
                .iter()
                .all(|call| !call.contains(destination)));
        }
    }
}

#[test]
fn startup_reuses_the_restored_active_tab_at_the_destination() {
    let destination = "https://start.example/";
    let store = Arc::new(FakeStore::default());
    let (mut shell, _, screen) = setup_with(store.clone());
    shell.handle(Command::Bootstrap);
    let first = active_id(&screen);
    navigate_and_commit(&mut shell, first, destination);
    store
        .settings
        .lock()
        .unwrap()
        .insert("tabs.new-page".into(), destination.into());

    for _ in 0..2 {
        let (mut restarted, engine, screen) = setup_with(store.clone());
        restarted.handle(Command::Bootstrap);
        assert_eq!(active_id(&screen), first);
        assert_eq!(last(&screen).tabs.len(), 1);
        assert!(restarted.is_new_tab_stand_in(first));
        assert!(engine
            .calls()
            .contains(&format!("create {first} {destination} [default]")));
        restarted.persist();
    }
}

#[test]
fn startup_reuses_the_restored_destination_after_a_www_redirect() {
    let destination = "https://start.example/";
    let redirected = "https://www.start.example/";
    let store = Arc::new(FakeStore::default());
    store.settings.lock().unwrap().extend([
        ("tabs.startup".to_owned(), "new-tab".to_owned()),
        ("tabs.new-page".to_owned(), destination.to_owned()),
    ]);
    let (mut shell, _, screen) = setup_with(store.clone());
    shell.handle(Command::Bootstrap);
    let first = active_id(&screen);
    commit_url(&mut shell, first, redirected);
    shell.persist();
    drop(shell);

    for _ in 0..2 {
        let (mut restarted, engine, screen) = setup_with(store.clone());
        restarted.handle(Command::Bootstrap);
        assert_eq!(active_id(&screen), first);
        assert_eq!(last(&screen).tabs.len(), 1);
        let before = engine.calls();
        assert!(before.contains(&format!("create {first} {redirected} [default]")));

        let closed = restarted.handle_operation(Command::Close(first));
        assert_eq!(closed.outcome, OperationOutcome::NoOp);
        assert_eq!(active_id(&screen), first);
        assert_eq!(engine.calls(), before);
        restarted.persist();
    }
}

#[test]
fn closing_the_lone_restored_destination_tab_is_a_no_op() {
    let destination = "https://start.example/";
    let store = Arc::new(FakeStore::default());
    store.settings.lock().unwrap().extend([
        ("tabs.startup".to_owned(), "continue".to_owned()),
        ("tabs.new-page".to_owned(), destination.to_owned()),
    ]);
    let (mut shell, _, screen) = setup_with(store.clone());
    shell.handle(Command::Bootstrap);
    let first = active_id(&screen);
    commit_url(&mut shell, first, destination);
    shell.persist();
    drop(shell);

    let (mut restarted, engine, screen) = setup_with(store);
    restarted.handle(Command::Bootstrap);
    assert_eq!(active_id(&screen), first);
    assert_eq!(last(&screen).tabs.len(), 1);
    let before = engine.calls();

    let closed = restarted.handle_operation(Command::Close(first));

    assert_eq!(closed.outcome, OperationOutcome::NoOp);
    assert_eq!(active_id(&screen), first);
    assert_eq!(engine.calls(), before);
    assert!(restarted.recently_closed.is_empty());
}

#[test]
fn startup_opens_the_destination_when_the_restored_active_tab_is_elsewhere() {
    let destination = "https://start.example/";
    let elsewhere = "https://elsewhere.example/";
    let store = Arc::new(FakeStore::default());
    let (mut shell, _, screen) = setup_with(store.clone());
    shell.handle(Command::Bootstrap);
    let previous = active_id(&screen);
    navigate_and_commit(&mut shell, previous, elsewhere);
    store
        .settings
        .lock()
        .unwrap()
        .insert("tabs.new-page".into(), destination.into());

    let (mut restarted, engine, screen) = setup_with(store);
    restarted.handle(Command::Bootstrap);

    let fresh = active_id(&screen);
    assert_ne!(fresh, previous);
    let state = last(&screen);
    assert_eq!(state.tabs.len(), 2);
    assert!(state
        .tabs
        .iter()
        .any(|tab| tab.id == previous.to_string() && tab.url.as_deref() == Some(elsewhere)));
    assert!(engine
        .calls()
        .contains(&format!("create {fresh} {destination} [default]")));
}
