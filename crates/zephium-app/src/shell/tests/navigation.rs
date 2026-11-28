  shell.handle(Command::Engine(EngineEvent::NavigationBlocked {
        id,
        url: "https://ads.example/".into(),
        reason: NavigationBlockReason::FilterLists,
        request: None,
        can_continue: false,
    }));
    let failure = last(&screen)
        .tabs
        .into_iter()
        .find(|tab| tab.id == id.to_string())
        .unwrap()
        .failure
        .expect("the refusal is explained");
    assert!(!failure.can_continue);
    shell.handle(Command::ContinuePastBlock {
        id,
        url: "https://ads.example/".into(),
    });
    assert!(!engine
        .calls()
        .iter()
        .any(|call| call.starts_with("allow-blocked")));
}

#[test]
fn a_refusal_of_what_the_person_no_longer_asks_for_is_dropped() {
    use zephium_core::ports::engine::NavigationBlockReason;
    let (mut shell, _engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    let id = active_id(&screen);
    shell.handle(Command::Navigate {
        id,
        input: "news.example".into(),
    });
    let old = shell.items.pending_navigation_request(id).unwrap();
    // The person has since asked for another page.
    shell.handle(Command::Navigate {
        id,
        input: "fine.example".into(),
    });
    let failure = |screen: &Screen| {
        last(screen)
            .tabs
            .into_iter()
            .find(|tab| tab.id == id.to_string())
            .unwrap()
            .failure
    };
    // A redirect of the earlier request, and a load the old page began.
    for (url, request) in [
        ("https://scam.example/", Some(old)),
        ("https://scam.example/", None),
    ] {
        shell.handle(Command::Engine(EngineEvent::NavigationBlocked {
            id,
            url: url.into(),
            reason: NavigationBlockReason::Badware,
            request,
            can_continue: true,
        }));
        assert_eq!(failure(&screen), None, "{request:?}");
    }
    // A refusal of the page now asked for is shown.
    shell.handle(Command::Engine(EngineEvent::NavigationBlocked {
        id,
        url: "https://fine.example/".into(),
        reason: NavigationBlockReason::Badware,
        request: None,
        can_continue: true,
    }));
    assert!(failure(&screen).is_some());
}

#[test]
fn a_typed_address_that_redirects_into_a_refused_host_shows_why() {
    use zephium_core::ports::engine::NavigationBlockReason;
    let (mut shell, _engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    let id = active_id(&screen);
    shell.handle(Command::Navigate {
        id,
        input: "fine.example".into(),
    });
    let typed = shell.items.pending_navigation_request(id).unwrap();
    shell.handle(Command::Engine(EngineEvent::NavigationBlocked {
        id,
        url: "https://scam.example/win".into(),
        reason: NavigationBlockReason::Badware,
        request: Some(typed),
        can_continue: true,
    }));
    let failure = last(&screen)
        .tabs
        .into_iter()
        .find(|tab| tab.id == id.to_string())
        .unwrap()
        .failure
        .unwrap();
    assert_eq!(failure.url, "https://scam.example/win");
    assert!(failure.can_continue);
}

#[test]
fn a_first_view_closed_with_its_https_try_loads_over_http_in_a_new_view() {
    use zephium_core::ports::engine::NavigationFailureReason;
    let (mut shell, engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    let id = active_id(&screen);
    let created = |engine: &FakeEngine| -> Vec<String> {
        engine
            .calls()
            .into_iter()
            .filter_map(|call| {
                call.strip_prefix(&format!("create {id} "))
                    .and_then(|rest| rest.split(' ').next())
                    .map(str::to_owned)
            })
            .collect()
    };
    let failure = |screen: &Screen| {
        last(screen)
            .tabs
            .into_iter()
            .find(|tab| tab.id == id.to_string())
            .and_then(|tab| tab.failure)
    };
    shell.handle(Command::Navigate {
        id,
        input: "http://plain.example/".into(),
    });
    let before = created(&engine).len();
    // The engine closes a new view whose first load fails before commit.
    shell.handle(Command::Engine(EngineEvent::NavigationFailureReported {
        id,
        reason: NavigationFailureReason::Unreachable,
        url: Some("https://plain.example/".into()),
        replaced_page: false,
    }));
    shell.handle(Command::Engine(EngineEvent::ViewCreationFailed { id }));
    assert_eq!(failure(&screen), None, "the failed try never shows");
    assert_eq!(
        created(&engine)[before..],
        ["http://plain.example/".to_owned()],
        "the page opens over http in a new view"
    );

    shell.handle(Command::Engine(EngineEvent::ViewCreationFailed { id }));
    assert!(
        failure(&screen).is_some(),
        "an http load that fails as well is shown"
    );
}

#[test]
fn an_http_page_is_tried_over_https_first_and_falls_back_once() {
    use zephium_core::ports::engine::NavigationFailureReason;
    let (mut shell, engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    let id = active_id(&screen);
    let loads = |engine: &FakeEngine| -> Vec<String> {
        engine
            .calls()
            .into_iter()
            .filter_map(|call| {
                call.strip_prefix(&format!("navigate {id} "))
                    .or_else(|| call.strip_prefix(&format!("create {id} ")))
                    .and_then(|rest| rest.split(' ').next())
                    .map(str::to_owned)
            })
            .collect()
    };
    let failure = |screen: &Screen| {
        last(screen)
            .tabs
            .into_iter()
            .find(|tab| tab.id == id.to_string())
            .and_then(|tab| tab.failure)
    };
    shell.handle(Command::Navigate {
        id,
        input: "http://plain.example/news".into(),
    });
    assert_eq!(
        loads(&engine).last().map(String::as_str),
        Some("https://plain.example/news")
    );
    shell.handle(Command::Engine(EngineEvent::NavigationFailureReported {
        id,
        reason: NavigationFailureReason::Unreachable,
        url: Some("https://plain.example/news".into()),
        replaced_page: true,
    }));
    assert_eq!(failure(&screen), None, "the failed try never shows");
    assert_eq!(
        loads(&engine).last().map(String::as_str),
        Some("http://plain.example/news"),
        "the page loads over http as asked"
    );

    shell.handle(Command::Navigate {
        id,
        input: "http://plain.example/other".into(),
    });
    assert_eq!(
        loads(&engine).last().map(String::as_str),
        Some("http://plain.example/other"),
        "a host that failed is not tried again"
    );
    shell.handle(Command::Navigate {
        id,
        input: "http://router.lan/".into(),
    });
    assert_eq!(
        loads(&engine).last().map(String::as_str),
        Some("http://router.lan/"),
        "the person's own network stays as asked"
    );
}

#[test]
fn an_https_try_that_redirected_to_another_host_is_over_when_it_commits() {
    use zephium_core::ports::engine::NavigationFailureReason;
    let (mut shell, engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    let id = active_id(&screen);
    navigate_and_commit(&mut shell, id, "a.example");
    shell.handle(Command::Navigate {
        id,
        input: "http://example.com/".into(),
    });
    assert!(engine
        .calls()
        .iter()
        .any(|call| call.starts_with(&format!("navigate {id} https://example.com/"))));
    commit_url(&mut shell, id, "https://www.example.com/");
    // A link on that page then fails on its own.
    let before = engine.calls().len();
    shell.handle(Command::Engine(EngineEvent::NavigationFailureReported {
        id,
        reason: NavigationFailureReason::Unreachable,
        url: Some("https://www.example.com/next".into()),
        replaced_page: true,
    }));
    assert!(
        !engine.calls()[before..]
            .iter()
            .any(|call| call.contains("http://example.com/")),
        "a later failure is not taken for the try"
    );
    let tab = last(&screen)
        .tabs
        .into_iter()
        .find(|tab| tab.id == id.to_string())
        .unwrap();
    assert!(tab.failure.is_some(), "and it shows");
}

#[test]
fn an_https_try_whose_http_load_cannot_be_sent_shows_its_failure() {
    use zephium_core::ports::engine::NavigationFailureReason;
    let (mut shell, engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    let id = active_id(&screen);
    navigate_and_commit(&mut shell, id, "a.example");
    shell.handle(Command::Navigate {
        id,
        input: "http://plain.example/".into(),
    });
    engine
        .reject_navigation_dispatch
        .store(true, std::sync::atomic::Ordering::Release);
    shell.handle(Command::Engine(EngineEvent::NavigationFailureReported {
        id,
        reason: NavigationFailureReason::Unreachable,
        url: None,
        replaced_page: false,
    }));
    let tab = last(&screen)
        .tabs
        .into_iter()
        .find(|tab| tab.id == id.to_string())
        .unwrap();
    assert!(tab.failure.is_some());
}

#[test]
fn native_url_refusal_without_a_browser_intent_projects_a_dismiss_only_notice() {
    let (mut shell, engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    let id = active_id(&screen);
    navigate_and_commit(&mut shell, id, "shown.example");
    assert!(shell.items.pending_navigation_request(id).is_none());
    shell.handle(Command::Engine(EngineEvent::NativeNavigationRefused {
        id,
        embedded: false,
    }));
    let tab = last(&screen)
        .tabs
        .into_iter()
        .find(|tab| tab.id == id.to_string())
        .unwrap();
    assert_eq!(
        tab.page_request,
        Some(zephium_ipc::PageRequestView::NavigationRefused { embedded: false })
    );
    assert_eq!(tab.url.as_deref(), Some("https://shown.example/"));
    assert!(tab.failure.is_none(), "the current document did not fail");
    let calls = engine.calls();
    let disposition = shell.operation_answer_page_request(
        id,
        tab.page_request_serial.unwrap(),
        crate::PageRequestDecision::Allow,
    );
    assert_eq!(disposition.outcome, OperationOutcome::Applied);
    assert_eq!(
        engine.calls(),
        calls,
        "even a forged allow answer cannot replay an unreadable URL"
    );
    assert!(shell.items.tab(id).unwrap().page_request.is_none());
}

#[test]
fn native_url_policy_refusal_never_triggers_an_https_to_http_fallback() {
    let (mut shell, engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    let id = active_id(&screen);
    navigate_and_commit(&mut shell, id, "shown.example");
    shell.handle(Command::Navigate {
        id,
        input: "http://plain.example/news".into(),
    });
    let calls = engine.calls();
    assert!(calls
        .iter()
        .any(|call| call.contains("https://plain.example/news")));
    shell.handle(Command::Engine(EngineEvent::NativeNavigationRefused {
        id,
        embedded: false,
    }));
    assert_eq!(
        engine.calls(),
        calls,
        "policy refusal grants no transport downgrade"
    );
    let tab = last(&screen)
        .tabs
        .into_iter()
        .find(|tab| tab.id == id.to_string())
        .unwrap();
    assert_eq!(
        tab.page_request,
        Some(zephium_ipc::PageRequestView::NavigationRefused { embedded: false })
    );
    assert!(tab.failure.is_none());
    let request = engine.last_navigation_request();
    // A policy cancellation may be followed by native failure/cancellation
    // callbacks. It must not resurrect fallback authority; only the exact
    // native request completion may settle the browser's pending intent.
    shell.handle(Command::Engine(EngineEvent::NavigationFailureReported {
        id,
        reason: zephium_core::ports::engine::NavigationFailureReason::Other,
        url: None,
        replaced_page: false,
    }));
    shell.handle(Command::Engine(EngineEvent::LoadingChanged {
        id,
        loading: false,
    }));
    shell.handle(Command::Engine(EngineEvent::NavigationFailed {
        id,
        request,
    }));
    assert!(shell.items.pending_navigation_request(id).is_none());
    assert!(!shell.items.tab(id).unwrap().loading);
    assert!(
        !engine
            .calls()
            .iter()
            .any(|call| call.contains("http://plain.example/news")),
        "late cancellation cannot authorize HTTP"
    );
    shell.handle(Command::Navigate {
        id,
        input: "http://plain.example/news".into(),
    });
    let loads: Vec<_> = engine
        .calls()
        .into_iter()
        .filter(|call| {
            call.starts_with(&format!("navigate {id} "))
                || call.starts_with(&format!("create {id} "))
        })
        .collect();
    assert!(
        loads.last().unwrap().contains("https://plain.example/news"),
        "policy refusal does not mark the host HTTPS-incompatible"
    );
}

#[test]
fn embedded_url_refusal_is_dismiss_only_and_preserves_top_level_https_authority() {
    let (mut shell, engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    let id = active_id(&screen);
    navigate_and_commit(&mut shell, id, "shown.example");
    shell.handle(Command::Navigate {
        id,
        input: "http://plain.example/news".into(),
    });
    let request = shell.items.pending_navigation_request(id).unwrap();
    let calls = engine.calls();
    shell.handle(Command::Engine(EngineEvent::NativeNavigationRefused {
        id,
        embedded: true,
    }));
    assert_eq!(shell.items.pending_navigation_request(id), Some(request));
    let tab = last(&screen)
        .tabs
        .into_iter()
        .find(|tab| tab.id == id.to_string())
        .unwrap();
    assert_eq!(
        tab.page_request,
        Some(zephium_ipc::PageRequestView::NavigationRefused { embedded: true })
    );
    shell.operation_answer_page_request(
        id,
        tab.page_request_serial.unwrap(),
        crate::PageRequestDecision::AlwaysAllow,
    );
    assert_eq!(
        engine.calls(),
        calls,
        "an embedded refusal carries no replay or grant authority"
    );
    assert_eq!(shell.items.pending_navigation_request(id), Some(request));
    // A later real transport failure still owns the original HTTPS attempt.
    shell.handle(Command::Engine(EngineEvent::NavigationFailureReported {
        id,
        reason: zephium_core::ports::engine::NavigationFailureReason::Unreachable,
        url: Some("https://plain.example/news".into()),
        replaced_page: false,
    }));
    assert!(
        engine
            .calls()
            .iter()
            .any(|call| call.contains("http://plain.example/news")),
        "child refusal did not revoke the independent top-level upgrade attempt"
    );
}
