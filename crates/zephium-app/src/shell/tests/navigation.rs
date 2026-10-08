use super::*;

#[test]
fn navigate_creates_and_shows_active_tab() {
    let (mut shell, engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    let id = active_id(&screen);

    shell.handle(Command::Navigate {
        id,
        input: "example.com".into(),
    });

    assert!(engine
        .calls()
        .contains(&format!("create {id} https://example.com/ [default]")));
    assert_eq!(engine.last_layout(), vec![id.to_string()]);

    let active = id.to_string();
    let tab = last(&screen)
        .tabs
        .into_iter()
        .find(|t| t.id == active)
        .unwrap();
    assert_eq!(tab.url, None, "an intent is not a committed page");
    assert!(!tab.loading, "native load events own the loading state");

    shell.handle(Command::Engine(EngineEvent::UrlChanged {
        id,
        url: "https://example.com/".into(),
    }));
    present_committed(&mut shell, id, "https://example.com/");
    let tab = last(&screen)
        .tabs
        .into_iter()
        .find(|t| t.id == active)
        .unwrap();
    assert_eq!(tab.url.as_deref(), Some("https://example.com/"));
}

#[test]
fn rejected_navigation_never_replaces_the_displayed_or_persisted_url() {
    let store = Arc::new(FakeStore::default());
    let (mut shell, engine, screen) = setup_with(store.clone());
    shell.handle(Command::Bootstrap);
    let id = active_id(&screen);
    navigate_and_commit(&mut shell, id, "a.example");

    engine
        .reject_navigation_dispatch
        .store(true, std::sync::atomic::Ordering::Release);
    shell.handle(Command::Navigate {
        id,
        input: "b.example".into(),
    });

    let tab = last(&screen)
        .tabs
        .into_iter()
        .find(|tab| tab.id == id.to_string())
        .unwrap();
    assert_eq!(tab.url.as_deref(), Some("https://a.example/"));
    let saved = store.saved.lock().unwrap().clone().unwrap();
    let PersistedKind::Tab { url, .. } =
        &saved.items.iter().find(|item| item.id == id).unwrap().kind
    else {
        panic!("committed tab must remain persisted")
    };
    assert_eq!(url, "https://a.example/");
    assert!(!engine
        .calls()
        .iter()
        .any(|call| call.contains("https://b.example/")));
}

#[test]
fn native_navigation_failure_is_correlated_and_never_commits_intent() {
    let (mut shell, engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    let id = active_id(&screen);
    navigate_and_commit(&mut shell, id, "a.example");

    shell.handle(Command::Navigate {
        id,
        input: "b.example".into(),
    });
    let old_request = engine.last_navigation_request();
    shell.handle(Command::Navigate {
        id,
        input: "c.example".into(),
    });
    let current_request = engine.last_navigation_request();

    shell.handle(Command::Engine(EngineEvent::NavigationFailed {
        id,
        request: old_request,
    }));
    shell.handle(Command::Engine(EngineEvent::NavigationFailed {
        id,
        request: current_request,
    }));
    let tab = last(&screen)
        .tabs
        .into_iter()
        .find(|tab| tab.id == id.to_string())
        .unwrap();
    assert_eq!(tab.url.as_deref(), Some("https://a.example/"));
}

#[test]
fn invalid_committed_url_events_have_no_state_or_history_side_effects() {
    let store = Arc::new(FakeStore::default());
    let (mut shell, _engine, screen) = setup_with(store.clone());
    shell.handle(Command::Bootstrap);
    let id = active_id(&screen);
    navigate_and_commit(&mut shell, id, "a.example");
    store.visits.lock().unwrap().clear();

    shell.handle(Command::Navigate {
        id,
        input: "b.example".into(),
    });
    shell.handle(Command::Engine(EngineEvent::UrlChanged {
        id,
        url: "file:///etc/passwd".into(),
    }));

    let tab = last(&screen)
        .tabs
        .into_iter()
        .find(|tab| tab.id == id.to_string())
        .unwrap();
    assert_eq!(tab.url.as_deref(), Some("https://a.example/"));
    assert!(store.visits.lock().unwrap().is_empty());

    shell.handle(Command::Engine(EngineEvent::UrlChanged {
        id,
        url: "https://b.example/".into(),
    }));
    let tab = last(&screen)
        .tabs
        .into_iter()
        .find(|tab| tab.id == id.to_string())
        .unwrap();
    assert_eq!(tab.url.as_deref(), Some("https://b.example/"));
}

#[test]
fn a_failed_address_is_explained_until_the_next_attempt() {
    use zephium_core::ports::engine::NavigationFailureReason;
    use zephium_ipc::TabFailureReason;
    let (mut shell, _engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    let id = active_id(&screen);
    let failure = |screen: &Screen| {
        last(screen)
            .tabs
            .into_iter()
            .find(|tab| tab.id == id.to_string())
            .and_then(|tab| tab.failure)
    };

    shell.handle(Command::Engine(EngineEvent::NavigationFailureReported {
        id,
        reason: NavigationFailureReason::Offline,
    }));
    assert_eq!(failure(&screen), None, "a page's own load names no address");

    shell.handle(Command::Navigate {
        id,
        input: "unreachable.example".into(),
    });
    shell.handle(Command::Engine(EngineEvent::NavigationFailureReported {
        id,
        reason: NavigationFailureReason::HostNotFound,
    }));
    let shown = failure(&screen).expect("the failed address is explained");
    assert_eq!(shown.url, "https://unreachable.example/");
    assert_eq!(shown.reason, TabFailureReason::HostNotFound);

    shell.handle(Command::Navigate {
        id,
        input: "example.com".into(),
    });
    assert_eq!(
        failure(&screen),
        None,
        "a new attempt clears the explanation"
    );
}

#[test]
fn a_link_for_another_app_waits_for_the_person() {
    use zephium_ipc::PageRequestView;
    let (mut shell, engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    let id = active_id(&screen);
    shell.handle(Command::Navigate {
        id,
        input: "zoom.us/j/1".into(),
    });
    shell.handle(Command::Engine(EngineEvent::UrlChanged {
        id,
        url: "https://zoom.us/j/1".into(),
    }));
    present_committed(&mut shell, id, "https://zoom.us/j/1");
    let request = |screen: &Screen| {
        last(screen)
            .tabs
            .into_iter()
            .find(|tab| tab.id == id.to_string())
            .and_then(|tab| tab.page_request)
    };
    let opened = |engine: &FakeEngine| {
        engine
            .calls()
            .into_iter()
            .filter(|call| call.starts_with("open-app "))
            .count()
    };

    for url in ["javascript:alert(1)", "file:///etc/passwd", "ms-msdt:/id x"] {
        shell.handle(Command::Engine(EngineEvent::ExternalAppRequested {
            id,
            url: url.into(),
            app: Some("Anything".into()),
        }));
    }
    assert_eq!(
        request(&screen),
        None,
        "only application links are asked about"
    );

    shell.handle(Command::Engine(EngineEvent::ExternalAppRequested {
        id,
        url: "zoommtg://zoom.us/join?confno=1".into(),
        app: Some("zoom.us".into()),
    }));
    assert_eq!(
        request(&screen),
        Some(PageRequestView::ExternalApp {
            site: Some("zoom.us".into()),
            scheme: "zoommtg".into(),
            app: Some("zoom.us".into()),
        })
    );
    assert_eq!(
        opened(&engine),
        0,
        "nothing opens before the person answers"
    );

    shell.handle(Command::AnswerPageRequest {
        id,
        decision: crate::PageRequestDecision::AlwaysAllow,
    });
    assert_eq!(request(&screen), None);
    assert_eq!(opened(&engine), 1);

    shell.handle(Command::Engine(EngineEvent::ExternalAppRequested {
        id,
        url: "zoommtg://zoom.us/join?confno=2".into(),
        app: Some("zoom.us".into()),
    }));
    assert_eq!(
        request(&screen),
        None,
        "the site was allowed for this kind of link"
    );
    assert_eq!(opened(&engine), 2);

    shell.handle(Command::Engine(EngineEvent::ExternalAppRequested {
        id,
        url: "slack://open".into(),
        app: Some("Slack".into()),
    }));
    shell.handle(Command::AnswerPageRequest {
        id,
        decision: crate::PageRequestDecision::Dismiss,
    });
    assert_eq!(request(&screen), None);
    assert_eq!(opened(&engine), 2, "a dismissed link never opens");
}

#[test]
fn a_blocked_new_tab_can_be_opened_from_its_notice() {
    use zephium_ipc::PageRequestView;
    let (mut shell, _engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    let id = active_id(&screen);
    shell.handle(Command::Navigate {
        id,
        input: "example.com".into(),
    });
    shell.handle(Command::Engine(EngineEvent::UrlChanged {
        id,
        url: "https://example.com/".into(),
    }));
    present_committed(&mut shell, id, "https://example.com/");
    let tabs_before = last(&screen).tabs.len();

    shell.handle(Command::Engine(EngineEvent::PageOpenBlocked {
        id,
        url: Some("https://accounts.example.com/login".into()),
    }));
    let shown = last(&screen)
        .tabs
        .into_iter()
        .find(|tab| tab.id == id.to_string())
        .and_then(|tab| tab.page_request);
    assert_eq!(
        shown,
        Some(PageRequestView::Popup {
            host: Some("accounts.example.com".into())
        })
    );

    shell.handle(Command::AnswerPageRequest {
        id,
        decision: crate::PageRequestDecision::Allow,
    });
    assert_eq!(last(&screen).tabs.len(), tabs_before + 1);
}

#[test]
fn an_unchanged_tab_keeps_its_revision_across_snapshots() {
    let (mut shell, _engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    let first = active_id(&screen);
    shell.handle(Command::Open);
    let second = active_id(&screen);
    let revision_of = |screen: &Screen, id: ItemId| {
        last(screen)
            .tabs
            .into_iter()
            .find(|tab| tab.id == id.to_string())
            .unwrap()
            .projection_revision
    };
    let before = revision_of(&screen, first);
    shell.handle(Command::Activate(first));
    shell.handle(Command::Activate(second));
    assert_eq!(
        revision_of(&screen, first),
        before,
        "switching tabs changed nothing about it"
    );

    shell.handle(Command::Engine(EngineEvent::TitleChanged {
        id: first,
        title: "Renamed".into(),
    }));
    shell.project_items();
    assert!(
        revision_of(&screen, first) > before,
        "a changed tab gets a newer revision"
    );
}
