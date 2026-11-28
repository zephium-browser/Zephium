t (mut shell, _engine, chrome, screen) = setup_with_async_chrome();
    shell.handle(Command::Bootstrap);
    let page = active_id(&screen);
    shell.handle(Command::Navigate {
        id: page,
        input: "chat.example".into(),
    });
    shell.handle(Command::Engine(EngineEvent::UrlChanged {
        id: page,
        url: "https://chat.example/".into(),
    }));
    present_committed(&mut shell, page, "https://chat.example/");
    shell.handle_operation(Command::ShowBrowserPage(Some(crate::BrowserPage::Settings)));

    assert_eq!(
        shell.handle_operation(Command::Open).outcome,
        OperationOutcome::Deferred
    );
    shell.handle(Command::Engine(EngineEvent::TitleChanged {
        id: page,
        title: "(3) Chat".into(),
    }));
    let (revision, _) = chrome.complete_browser_return(true);
    shell.browser_chrome_restored(revision, true);

    assert_ne!(active_id(&screen), page);
    assert_eq!(shell.active_browser_page(), None);
}

fn open_page(shell: &mut Shell, screen: &Screen, url: &str) -> ItemId {
    shell.handle(Command::Run("tab.new".into()));
    let id = active_id(screen);
    shell.handle(Command::Navigate {
        id,
        input: url.into(),
    });
    shell.handle(Command::Engine(EngineEvent::UrlChanged {
        id,
        url: url.into(),
    }));
    shell.handle(Command::Engine(EngineEvent::LoadingChanged {
        id,
        loading: false,
    }));
    id
}

#[test]
fn idle_today_tabs_close_into_history_and_tabs_in_use_stay() {
    let (mut shell, _engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    let old = open_page(&mut shell, &screen, "https://old.example/");
    let kept_by_engine = open_page(&mut shell, &screen, "https://form.example/");
    let recent = open_page(&mut shell, &screen, "https://recent.example/");
    let front = open_page(&mut shell, &screen, "https://front.example/");
    let now = crate::shell::tabs::unix_millis().unwrap();
    assert!(shell.items.tab(front).unwrap().last_active_ms.unwrap() >= now - 1_000);
    let eight_days_ago = now - 8 * 24 * 60 * 60 * 1000;
    for id in [old, kept_by_engine, front] {
        shell.items.set_last_active(id, eight_days_ago);
    }
    shell.items.set_last_active(recent, now - 60 * 60 * 1000);

    shell.handle(Command::Tick);
    assert!(shell.items.tab(old).is_some(), "never is the default");

    shell.handle(Command::SetAppSetting {
        key: "tabs.archive".into(),
        value: "7d".into(),
    });
    // A page the engine refused to discard holds state a reload would lose.
    shell.residency.discard_protected_until.insert(
        kept_by_engine,
        std::time::Instant::now() + std::time::Duration::from_secs(60),
    );
    shell.handle(Command::Tick);
    assert!(shell.items.tab(old).is_none());
    assert_eq!(
        shell.recently_closed.last().map(|tab| tab.url.as_str()),
        Some("https://old.example/")
    );
    for id in [kept_by_engine, recent, front] {
        assert!(shell.items.tab(id).is_some());
    }
    assert!(shell.items.tab(front).unwrap().last_active_ms.unwrap() >= now);
    assert_eq!(active_id(&screen), front);
}

#[test]
fn a_tab_without_a_recorded_time_starts_its_clock_when_first_seen() {
    let store = Arc::new(FakeStore::default());
    store
        .settings
        .lock()
        .unwrap()
        .insert("tabs.archive".into(), "12h".into());
    let (mut shell, _engine, screen) = setup_with(store);
    shell.handle(Command::Bootstrap);
    open_page(&mut shell, &screen, "https://front.example/");
    let space = shell.windows.focused().unwrap().space;
    let legacy = ItemId::generate();
    assert!(shell.items.insert_unloaded_tab(
        legacy,
        Placement::Space {
            space,
            section: SpaceSection::Today,
        },
        url::Url::parse("https://legacy.example/").unwrap(),
        "Legacy",
    ));
    assert_eq!(shell.items.tab(legacy).unwrap().last_active_ms, None);
    shell.handle(Command::Tick);
    let stamped = shell.items.tab(legacy).unwrap().last_active_ms;
    assert!(stamped.is_some_and(|ms| ms + 1_000 >= crate::shell::tabs::unix_millis().unwrap()));
}

#[test]
fn links_from_other_apps_follow_new_tabs_unless_placed_on_their_own() {
    let store = Arc::new(FakeStore::default());
    store
        .settings
        .lock()
        .unwrap()
        .insert("tabs.new-position".into(), "after-current".into());
    let (mut shell, _engine, screen) = setup_with(store);
    shell.handle(Command::Bootstrap);
    let first = open_page(&mut shell, &screen, "https://first.example/");
    open_page(&mut shell, &screen, "https://second.example/");
    let today = |screen: &Screen| {
        last(screen)
            .nodes
            .iter()
            .filter(|node| node.section == SidebarSectionView::Today && node.parent_id.is_none())
            .filter_map(|node| ItemId::parse(&node.id))
            .collect::<Vec<_>>()
    };
    let follows = |screen: &Screen, opened: ItemId| {
        let order = today(screen);
        let at = order.iter().position(|id| *id == first).unwrap();
        order.get(at + 1) == Some(&opened)
    };

    // By default a handed-over link lands where a new tab would.
    shell.handle(Command::Activate(first));
    shell.handle(Command::OpenExternal(
        vec!["https://beside.example/".into()],
    ));
    assert!(follows(&screen, active_id(&screen)));

    shell.handle(Command::SetAppSetting {
        key: "tabs.external-position".into(),
        value: "end".into(),
    });
    shell.handle(Command::Activate(first));
    shell.handle(Command::OpenExternal(vec!["https://end.example/".into()]));
    let end = active_id(&screen);
    assert_eq!(today(&screen).last(), Some(&end));

    shell.handle(Command::SetAppSetting {
        key: "tabs.new-position".into(),
        value: "end".into(),
    });
    shell.handle(Command::SetAppSetting {
        key: "tabs.external-position".into(),
        value: "after-current".into(),
    });
    shell.handle(Command::Activate(first));
    shell.handle(Command::OpenExternal(vec!["https://next.example/".into()]));
    assert!(follows(&screen, active_id(&screen)));
}

#[test]
fn new_tab_destination_opens_fresh_tabs_without_redirecting_explicit_urls() {
    let (mut shell, engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    shell.handle(Command::SetAppSetting {
        key: "tabs.new-page".into(),
        value: "https://start.example/".into(),
    });
    shell.handle(Command::Open);
    let first = active_id(&screen);
    assert!(engine
        .calls()
        .iter()
        .any(|call| call == &format!("create {first} https://start.example/ [default]")));
    navigate_and_commit(&mut shell, first, "https://start.example/");
    shell.handle(Command::Run("tab.new".into()));
    let second = active_id(&screen);
    assert_ne!(first, second);
    assert!(engine
        .calls()
        .iter()
        .any(|call| call == &format!("create {second} https://start.example/ [default]")));

    let before = engine.calls().len();
    shell.handle(Command::OpenUrl {
        input: "https://other.example/".into(),
        new_tab: true,
    });
    let explicit = active_id(&screen);
    assert!(engine.calls()[before..]
        .iter()
        .all(|call| !call.contains("start.example")));
    assert!(engine.calls()[before..]
        .iter()
        .any(|call| call == &format!("create {explicit} https://other.example/ [default]")));

    shell.handle(Command::SetAppSetting {
        key: "tabs.new-page".into(),
        value: "".into(),
    });
    shell.handle(Command::Open);
    assert!(shell.is_untouched_new_tab(active_id(&screen)));
}

#[test]
fn private_new_tabs_keep_the_builtin_page_with_a_custom_destination() {
    let (mut shell, engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    shell.handle(Command::SetAppSetting {
        key: "tabs.new-page".into(),
        value: "https://start.example/".into(),
    });
    shell.handle(Command::Run("window.newPrivate".into()));
    assert!(shell.incognito_profile(shell.windows.focused().unwrap().profile));
    shell.handle(Command::Open);
    assert!(shell.is_untouched_new_tab(active_id(&screen)));
    assert!(engine
        .calls()
        .iter()
        .all(|call| !call.contains("start.example")));
}

const NEW_TAB_DESTINATION: &str = "https://start.example/";

fn custom_new_tab(destination: &str, first_url: &str) -> (Shell, Arc<FakeEngine>, Screen) {
    let store = Arc::new(FakeStore::default());
    store
        .settings
        .lock()
        .unwrap()
        .insert("tabs.new-page".into(), destination.into());
    let (mut shell, engine, screen) = setup_with(store);
    shell.handle(Command::Bootstrap);
    let id = active_id(&screen);
    commit_url(&mut shell, id, first_url);
    present_committed(&mut shell, id, first_url);
    (shell, engine, screen)
}

#[test]
fn closing_the_lone_destination_tab_is_a_no_op_even_after_reload() {
    let (mut shell, engine, screen) = custom_new_tab(NEW_TAB_DESTINATION, NEW_TAB_DESTINATION);
    let id = active_id(&screen);
    shell.handle(Command::Reload(id));
    commit_url(&mut shell, id, NEW_TAB_DESTINATION);

    let before = engine.calls();
    let closed = shell.handle_operation(Command::Close(id));

    assert_eq!(closed.outcome, OperationOutcome::NoOp);
    assert_eq!(active_id(&screen), id);
    assert_eq!(engine.calls(), before);
    assert!(shell.recently_closed.is_empty());
}

#[test]
fn closing_a_destination_tab_with_another_tab_open_skips_recently_closed() {
    let (mut shell, engine, screen) = custom_new_tab(NEW_TAB_DESTINATION, NEW_TAB_DESTINATION);
    let first = active_id(&screen);
    shell.handle(Command::Open);
    let stand_in = active_id(&screen);
    assert_ne!(stand_in, first);
    commit_url(&mut shell, stand_in, NEW_TAB_DESTINATION);
    present_committed(&mut shell, stand_in, NEW_TAB_DESTINATION);
    assert!(shell.is_new_tab_stand_in(stand_in));

    shell.handle(Command::Close(stand_in));

    assert!(shell.items.tab(stand_in).is_none());
    assert_eq!(active_id(&screen), first);
    assert!(engine.calls().contains(&format!("close {stand_in}")));
    assert!(shell.recently_closed.is_empty());
}

#[test]
fn a_destination_redirect_stands_in_until_a_later_commit_changes_url() {
    let redirected = "https://www.start.example/";
    let (mut shell, engine, screen) = custom_new_tab(NEW_TAB_DESTINATION, redirected);
    let id = active_id(&screen);
    let before = engine.calls();
    assert!(before.contains(&format!("create {id} {NEW_TAB_DESTINATION} [default]")));
    assert_eq!(last(&screen).tabs.len(), 1);
    assert_eq!(
        shell.new_tab_destinations.get(&id).map(url::Url::as_str),
        Some(redirected)
    );

    let closed = shell.handle_operation(Command::Close(id));
    assert_eq!(closed.outcome, OperationOutcome::NoOp);
    assert_eq!(active_id(&screen), id);
    assert_eq!(engine.calls(), before);
    assert!(shell.recently_closed.is_empty());

    let elsewhere = "https://elsewhere.example/";
    commit_url(&mut shell, id, elsewhere);
    shell.handle(Command::Close(id));

    assert_ne!(active_id(&screen), id);
    assert!(shell.items.tab(id).is_none());
    assert!(engine.calls().contains(&format!("close {id}")));
    assert_eq!(shell.recently_closed.len(), 1);
    assert_eq!(shell.recently_closed[0].url, elsewhere);
}

#[test]
fn an_https_upgraded_destination_stands_in_until_a_later_commit() {
    let destination = "http://start.example/";
    let upgraded = navigation::https_upgrade(&url::Url::parse(destination).unwrap()).unwrap();
    let (mut shell, engine, screen) = custom_new_tab(destination, upgraded.as_str());
    let id = active_id(&screen);
    assert!(engine
        .calls()
        .contains(&format!("create {id} {upgraded} [default]")));
    assert_eq!(shell.new_tab_destinations.get(&id), Some(&upgraded));
    assert!(shell.is_new_tab_stand_in(id));

    commit_url(&mut shell, id, "https://elsewhere.example/");

    assert!(!shell.new_tab_destinations.contains_key(&id));
    assert!(!shell.is_new_tab_stand_in(id));
}

#[test]
fn leaving_a_space_keeps_the_destination_tab_and_returning_focuses_it() {
    let (mut shell, engine, screen) = custom_new_tab(NEW_TAB_DESTINATION, NEW_TAB_DESTINATION);
    let home = shell.windows.focused().unwrap().space;
    let stand_in = active_id(&screen);
    shell.handle(Command::Space(Box::new(SpaceCommand::Create {
        name: "Studio".into(),
        look: Default::default(),
    })));
    let studio = shell.windows.focused().unwrap().space;
    assert_ne!(studio, home);
    let studio_tab = active_id(&screen);
    commit_url(&mut shell, studio_tab, NEW_TAB_DESTINATION);
    present_committed(&mut shell, studio_tab, NEW_TAB_DESTINATION);
    assert_eq!(shell.regular_tabs(home), [stand_in]);
    assert!(!engine.calls().contains(&format!("close {stand_in}")));

    shell.handle(Command::Space(Box::new(SpaceCommand::Switch(home))));

    assert_eq!(shell.windows.focused().unwrap().space, home);
    assert_eq!(active_id(&screen), stand_in);
    assert_eq!(shell.regular_tabs(studio), [studio_tab]);
    assert!(shell.recently_closed.is_empty());
}

#[test]
fn reopening_a_tab_keeps_the_destination_tab_beside_it() {
    let (mut shell, engine, screen) = custom_new_tab(NEW_TAB_DESTINATION, NEW_TAB_DESTINATION);
    let first = active_id(&screen);
    navigate_and_commit(&mut shell, first, "https://closed.example/");
    shell.handle(Command::Close(first));
    let stand_in = active_id(&screen);
    assert_ne!(stand_in, first);
    commit_url(&mut shell, stand_in, NEW_TAB_DESTINATION);
    present_committed(&mut shell, stand_in, NEW_TAB_DESTINATION);

    shell.handle(Command::Run("tab.reopen".into()));

    let restored = active_id(&screen);
    assert_ne!(restored, stand_in);
    let space = shell.windows.focused().unwrap().space;
    let tabs = shell.regular_tabs(space);
    assert_eq!(tabs.len(), 2);
    assert!(tabs.contains(&stand_in) && tabs.contains(&restored));
    assert_eq!(
        shell
            .items
            .tab(restored)
            .and_then(|tab| tab.url.as_ref())
            .map(url::Url::as_str),
        Some("https://closed.example/")
    );
    assert!(!engine.calls().contains(&format!("close {stand_in}")));
    assert!(shell.recently_closed.is_empty());
}

#[test]
fn a_destination_tab_that_navigated_away_can_close_after_returning() {
    let (mut shell, engine, screen) = custom_new_tab(NEW_TAB_DESTINATION, NEW_TAB_DESTINATION);
    let used = active_id(&screen);
    commit_url(&mut shell, used, "https://elsewhere.example/");
    commit_url(&mut shell, used, NEW_TAB_DESTINATION);

    shell.handle(Command::Close(used));

    assert_ne!(active_id(&screen), used);
    assert!(shell.items.tab(used).is_none());
    assert!(engine.calls().contains(&format!("close {used}")));
    assert_eq!(shell.recently_closed.len(), 1);
    assert_eq!(shell.recently_closed[0].url, NEW_TAB_DESTINATION);
}
