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
        url: None,
        replaced_page: false,
    }));
    assert_eq!(failure(&screen), None, "a page's own load names no address");

    shell.handle(Command::Navigate {
        id,
        input: "unreachable.example".into(),
    });
    shell.handle(Command::Engine(EngineEvent::NavigationFailureReported {
        id,
        reason: NavigationFailureReason::HostNotFound,
        url: None,
        replaced_page: false,
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
fn a_page_replaced_by_an_error_document_is_covered_until_a_page_commits() {
    use zephium_core::ports::engine::NavigationFailureReason;
    let (mut shell, engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    let id = active_id(&screen);
    shell.handle(Command::Navigate {
        id,
        input: "example.com".into(),
    });
    commit_url(&mut shell, id, "https://example.com/");
    present_committed(&mut shell, id, "https://example.com/");
    assert_eq!(engine.last_layout(), [id.to_string()]);
    let tab = |screen: &Screen| {
        last(screen)
            .tabs
            .into_iter()
            .find(|tab| tab.id == id.to_string())
            .unwrap()
    };

    // A link the page followed failed, and the engine replaced the page.
    shell.handle(Command::Engine(EngineEvent::NavigationFailureReported {
        id,
        reason: NavigationFailureReason::Unreachable,
        url: Some("https://example.com/down".into()),
        replaced_page: true,
    }));
    let failure = tab(&screen)
        .failure
        .expect("the failure stands in for the page");
    assert!(failure.covers_page);
    assert_eq!(failure.url, "https://example.com/down");
    assert!(
        engine.last_layout().is_empty(),
        "the view keeps its history off the stage"
    );

    // Retrying keeps it covered while the address loads.
    shell.handle(Command::Navigate {
        id,
        input: "https://example.com/down".into(),
    });
    assert!(tab(&screen)
        .failure
        .is_some_and(|failure| failure.covers_page));
    assert!(engine.last_layout().is_empty());

    // Back, or any other document, commits: the view returns.
    commit_url(&mut shell, id, "https://example.com/");
    assert_eq!(tab(&screen).failure, None);
    assert_eq!(engine.last_layout(), [id.to_string()]);
}

#[test]
fn a_page_kept_in_place_after_a_failure_is_never_covered() {
    use zephium_core::ports::engine::NavigationFailureReason;
    let (mut shell, engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    let id = active_id(&screen);
    shell.handle(Command::Navigate {
        id,
        input: "example.com".into(),
    });
    commit_url(&mut shell, id, "https://example.com/");
    present_committed(&mut shell, id, "https://example.com/");
    shell.handle(Command::Navigate {
        id,
        input: "unreachable.example".into(),
    });
    shell.handle(Command::Engine(EngineEvent::NavigationFailureReported {
        id,
        reason: NavigationFailureReason::HostNotFound,
        url: None,
        replaced_page: false,
    }));
    let failure = last(&screen)
        .tabs
        .into_iter()
        .find(|tab| tab.id == id.to_string())
        .and_then(|tab| tab.failure)
        .unwrap();
    assert!(!failure.covers_page);
    assert_eq!(engine.last_layout(), [id.to_string()]);
}

#[test]
fn a_page_that_stops_responding_in_front_asks_to_wait_or_reload() {
    use zephium_ipc::PageRequestView;
    let (mut shell, engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    let behind = active_id(&screen);
    commit_url(&mut shell, behind, "https://behind.example/");
    shell.handle(Command::Open);
    let id = active_id(&screen);
    shell.handle(Command::Navigate {
        id,
        input: "example.com".into(),
    });
    commit_url(&mut shell, id, "https://example.com/");
    present_committed(&mut shell, id, "https://example.com/");
    let request = |screen: &Screen, id: ItemId| {
        last(screen)
            .tabs
            .into_iter()
            .find(|tab| tab.id == id.to_string())
            .and_then(|tab| tab.page_request)
    };
    let hung = |shell: &mut Shell, id: ItemId, responding: bool| {
        shell.handle(Command::Engine(EngineEvent::PageResponsiveness {
            id,
            responding,
        }));
    };

    hung(&mut shell, behind, false);
    assert_eq!(request(&screen, behind), None, "only the tab in front asks");

    hung(&mut shell, id, false);
    assert_eq!(request(&screen, id), Some(PageRequestView::Unresponsive));

    // The page answers again: the question goes.
    hung(&mut shell, id, true);
    assert_eq!(request(&screen, id), None);

    // Waiting holds the question back while the page stays hung.
    hung(&mut shell, id, false);
    shell.handle(Command::AnswerPageRequest {
        id,
        decision: crate::PageRequestDecision::Dismiss,
    });
    assert_eq!(request(&screen, id), None);
    hung(&mut shell, id, false);
    assert_eq!(request(&screen, id), None, "the person chose to wait");
    shell.crash.unresponsive_waits.insert(
        id,
        std::time::Instant::now() - super::super::page_requests::UNRESPONSIVE_WAIT,
    );
    hung(&mut shell, id, false);
    assert_eq!(request(&screen, id), Some(PageRequestView::Unresponsive));

    // Reload ends the hung view; the crash path brings the page back.
    shell.handle(Command::AnswerPageRequest {
        id,
        decision: crate::PageRequestDecision::Allow,
    });
    assert_eq!(request(&screen, id), None);
    assert!(engine.calls().contains(&format!("end-unresponsive {id}")));
    assert!(shell.crash.unresponsive_waits.is_empty());
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
            user_gesture: true,
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
        user_gesture: true,
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
        user_gesture: true,
    }));
    assert_eq!(
        request(&screen),
        None,
        "the site was allowed for this kind of link"
    );
    assert_eq!(opened(&engine), 2);

    for (app, user_gesture, shown) in [(None, false, false), (None, true, true)] {
        shell.handle(Command::Engine(EngineEvent::ExternalAppRequested {
            id,
            url: "slack://open".into(),
            app,
            user_gesture,
        }));
        assert_eq!(request(&screen).is_some(), shown);
        shell.handle(Command::AnswerPageRequest {
            id,
            decision: crate::PageRequestDecision::Dismiss,
        });
    }

    shell.handle(Command::Engine(EngineEvent::ExternalAppRequested {
        id,
        url: "slack://open".into(),
        app: Some("Slack".into()),
        user_gesture: true,
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
            host: Some("accounts.example.com".into()),
            site: Some("example.com".into()),
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

#[test]
fn a_sign_in_prompt_waits_on_its_tab_and_answers_once() {
    use zephium_core::http_auth::{HttpAuthPrompt, HttpCredentials};
    use zephium_ipc::PageRequestView;
    let (mut shell, engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    let id = active_id(&screen);
    let shown = |screen: &Screen| {
        last(screen)
            .tabs
            .into_iter()
            .find(|tab| tab.id == id.to_string())
            .and_then(|tab| tab.page_request)
    };
    let answers = |engine: &FakeEngine| {
        engine
            .calls()
            .into_iter()
            .filter(|call| call.starts_with("http-auth "))
            .collect::<Vec<_>>()
    };
    let ask = |shell: &mut Shell, request: u32| {
        shell.handle(Command::Engine(EngineEvent::HttpAuthRequested {
            id,
            prompt: HttpAuthPrompt {
                request,
                host: "router.lan".into(),
                insecure: true,
                proxy: false,
                retry: false,
            },
        }));
    };
    let credentials = || HttpCredentials::new("admin".into(), "hunter2".into());

    ask(&mut shell, 4);
    assert_eq!(
        shown(&screen),
        Some(PageRequestView::HttpAuth {
            request: 4,
            host: "router.lan".into(),
            insecure: true,
            proxy: false,
            retry: false,
        })
    );
    shell.handle(Command::Engine(EngineEvent::PageOpenBlocked {
        id,
        url: None,
    }));
    assert!(
        matches!(shown(&screen), Some(PageRequestView::HttpAuth { .. })),
        "a refused pop-up never hides a waiting sign-in"
    );

    shell.handle(Command::AnswerHttpAuth {
        id,
        request: 3,
        credentials: credentials(),
    });
    assert!(answers(&engine).is_empty(), "a stale answer goes nowhere");
    shell.handle(Command::AnswerHttpAuth {
        id,
        request: 4,
        credentials: credentials(),
    });
    assert_eq!(shown(&screen), None);
    assert_eq!(
        answers(&engine),
        [format!("http-auth {id} 4 sign-in as admin")]
    );
    shell.handle(Command::AnswerHttpAuth {
        id,
        request: 4,
        credentials: credentials(),
    });
    assert_eq!(answers(&engine).len(), 1, "a prompt is answered once");

    ask(&mut shell, 5);
    shell.handle(Command::Engine(EngineEvent::HttpAuthEnded {
        id,
        request: 4,
    }));
    assert!(
        shown(&screen).is_some(),
        "only the ended prompt is taken down"
    );
    shell.handle(Command::Engine(EngineEvent::HttpAuthEnded {
        id,
        request: 5,
    }));
    assert_eq!(shown(&screen), None);

    ask(&mut shell, 6);
    shell.handle(Command::AnswerPageRequest {
        id,
        decision: crate::PageRequestDecision::Allow,
    });
    assert_eq!(shown(&screen), None);
    assert_eq!(
        answers(&engine).last().map(String::as_str),
        Some(format!("http-auth {id} 6 cancel").as_str()),
        "the generic answer can only cancel a sign-in"
    );

    ask(&mut shell, 7);
    shell.handle(Command::Engine(EngineEvent::Crashed { id }));
    assert!(!matches!(
        shown(&screen),
        Some(PageRequestView::HttpAuth { .. })
    ));
}

#[test]
fn only_a_local_hosts_refused_certificate_has_a_way_past() {
    use zephium_core::certificates::CertificateProblem;
    use zephium_core::ports::engine::NavigationFailureReason;
    use zephium_ipc::{TabCertificateProblem, TabFailureReason};
    let (mut shell, engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    let id = active_id(&screen);
    let tab = |screen: &Screen| {
        last(screen)
            .tabs
            .into_iter()
            .find(|tab| tab.id == id.to_string())
            .unwrap()
    };
    let allowed = |engine: &FakeEngine| {
        engine
            .calls()
            .into_iter()
            .filter(|call| call.starts_with("allow-certificate "))
            .collect::<Vec<_>>()
    };
    let refuse = |shell: &mut Shell, input: &str, refused: &str| {
        shell.handle(Command::Navigate {
            id,
            input: input.into(),
        });
        shell.handle(Command::Engine(EngineEvent::NavigationFailureReported {
            id,
            reason: NavigationFailureReason::Certificate(CertificateProblem::Untrusted),
            url: Some(refused.into()),
            replaced_page: false,
        }));
    };

    refuse(&mut shell, "example.com", "https://example.com/");
    let failure = tab(&screen).failure.expect("the refusal is explained");
    assert_eq!(failure.reason, TabFailureReason::Insecure);
    assert_eq!(failure.certificate, Some(TabCertificateProblem::Untrusted));
    assert!(!failure.can_continue, "a public name never has a way past");
    shell.handle(Command::ContinuePastCertificate(id));
    assert!(allowed(&engine).is_empty());

    // The refused address, after the redirect to https, is the one shown.
    refuse(&mut shell, "http://nas.local", "https://nas.local:5001/");
    let failure = tab(&screen).failure.unwrap();
    assert_eq!(failure.url, "https://nas.local:5001/");
    assert!(failure.can_continue);
    shell.handle(Command::ContinuePastCertificate(id));
    assert_eq!(
        allowed(&engine),
        [format!("allow-certificate {id} https://nas.local:5001/")]
    );
    commit_url(&mut shell, id, "https://nas.local:5001/");
    present_committed(&mut shell, id, "https://nas.local:5001/");
    let shown = tab(&screen);
    assert_eq!(shown.failure, None);
    assert!(shown.certificate_exception, "the page never looks secure");

    commit_url(&mut shell, id, "https://nas.local/");
    present_committed(&mut shell, id, "https://nas.local/");
    assert!(
        !tab(&screen).certificate_exception,
        "another port is another origin"
    );
}
