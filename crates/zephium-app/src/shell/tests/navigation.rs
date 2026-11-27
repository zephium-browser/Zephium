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
        request: shown_serial(&shell, id),
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
        request: shown_serial(&shell, id),
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
            initiator: zephium_core::navigation::AppLinkSource::Page,
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
        initiator: zephium_core::navigation::AppLinkSource::Page,
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
        request: shown_serial(&shell, id),
        decision: crate::PageRequestDecision::AlwaysAllow,
    });
    assert_eq!(request(&screen), None);
    assert_eq!(opened(&engine), 1);

    shell.handle(Command::Engine(EngineEvent::ExternalAppRequested {
        id,
        url: "zoommtg://zoom.us/join?confno=2".into(),
        app: Some("zoom.us".into()),
        user_gesture: true,
        initiator: zephium_core::navigation::AppLinkSource::Page,
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
            initiator: zephium_core::navigation::AppLinkSource::Page,
        }));
        assert_eq!(request(&screen).is_some(), shown);
        shell.handle(Command::AnswerPageRequest {
            id,
            request: shown_serial(&shell, id),
            decision: crate::PageRequestDecision::Dismiss,
        });
    }

    shell.handle(Command::Engine(EngineEvent::ExternalAppRequested {
        id,
        url: "slack://open".into(),
        app: Some("Slack".into()),
        user_gesture: true,
        initiator: zephium_core::navigation::AppLinkSource::Page,
    }));
    shell.handle(Command::AnswerPageRequest {
        id,
        request: shown_serial(&shell, id),
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
        request: shown_serial(&shell, id),
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
        request: shown_serial(&shell, id),
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

#[test]
fn a_tab_opened_at_a_refused_address_shows_why_over_its_blank_page() {
    use zephium_core::ports::engine::NavigationBlockReason;
    use zephium_ipc::TabBlockReason;
    let (mut shell, engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    shell.handle(Command::OpenUrl {
        input: "https://scam.example/win".into(),
        new_tab: true,
    });
    let id = active_id(&screen);
    assert!(engine
        .calls()
        .iter()
        .any(|call| call.starts_with(&format!("create {id}"))));
    // The engine names the refusal before it builds the view, then builds
    // it on a blank page.
    shell.handle(Command::Engine(EngineEvent::NavigationBlocked {
        id,
        url: "https://scam.example/win".into(),
        reason: NavigationBlockReason::Badware,
        request: None,
        can_continue: true,
    }));
    commit_url(&mut shell, id, "about:blank");
    present_committed(&mut shell, id, "about:blank");
    let shown = last(&screen)
        .tabs
        .into_iter()
        .find(|tab| tab.id == id.to_string())
        .unwrap();
    let failure = shown.failure.expect("the refusal is explained");
    assert_eq!(failure.blocked, Some(TabBlockReason::Badware));
    assert_eq!(failure.url, "https://scam.example/win");
    assert!(failure.covers_page);
    assert_eq!(shown.title, "scam.example");
    assert_eq!(shown.url, None);
    assert!(!shown.loading);
}

#[test]
fn an_address_opened_in_a_new_tab_that_redirects_into_a_refused_host_shows_why() {
    use zephium_core::ports::engine::NavigationBlockReason;
    let blocked = |id| {
        Command::Engine(EngineEvent::NavigationBlocked {
            id,
            url: "https://scam.example/win".into(),
            reason: NavigationBlockReason::Badware,
            request: None,
            can_continue: true,
        })
    };
    let failure = |screen: &Screen, id: ItemId| {
        last(screen)
            .tabs
            .into_iter()
            .find(|tab| tab.id == id.to_string())
            .unwrap()
            .failure
    };
    let (mut shell, _engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    shell.handle(Command::OpenUrl {
        input: "https://fine.example/".into(),
        new_tab: true,
    });
    let id = active_id(&screen);
    shell.handle(blocked(id));
    let shown = failure(&screen, id).expect("the redirect's refusal is explained");
    assert_eq!(shown.url, "https://scam.example/win");
    assert!(shown.can_continue);

    // Once the person asks for another page, the first load's refusal is stale.
    let (mut shell, _engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    shell.handle(Command::OpenUrl {
        input: "https://fine.example/".into(),
        new_tab: true,
    });
    let id = active_id(&screen);
    shell.handle(Command::Navigate {
        id,
        input: "other.example".into(),
    });
    shell.handle(blocked(id));
    assert_eq!(failure(&screen, id), None);
}

#[test]
fn a_refused_page_covers_the_tab_until_the_person_leaves_or_continues() {
    use zephium_core::ports::engine::NavigationBlockReason;
    use zephium_ipc::{TabBlockReason, TabFailureReason};
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
    shell.handle(Command::Navigate {
        id,
        input: "news.example".into(),
    });
    commit_url(&mut shell, id, "https://news.example/");
    present_committed(&mut shell, id, "https://news.example/");
    shell.handle(Command::Engine(EngineEvent::FaviconPixels {
        id,
        page_url: "https://news.example/".into(),
        rgba: vec![9; zephium_core::icon::RGBA32_BYTES],
    }));
    assert!(tab(&screen).icon.is_some());
    let page_title = tab(&screen).title;
    let block_for = |shell: &mut Shell, url: &str, reason| {
        shell.handle(Command::Engine(EngineEvent::NavigationBlocked {
            id,
            url: url.into(),
            reason,
            request: None,
            can_continue: true,
        }))
    };
    let block = |shell: &mut Shell| {
        block_for(
            shell,
            "https://scam.example/win",
            NavigationBlockReason::Badware,
        )
    };

    // A link on the page leads to a refused site: the page stays under the
    // explanation, which the tab is titled for, and nothing is loading.
    shell.handle(Command::Engine(EngineEvent::LoadingChanged {
        id,
        loading: true,
    }));
    block(&mut shell);
    let shown = tab(&screen);
    let failure = shown.failure.expect("the refusal is explained");
    assert_eq!(failure.reason, TabFailureReason::Blocked);
    assert_eq!(failure.blocked, Some(TabBlockReason::Badware));
    assert_eq!(failure.url, "https://scam.example/win");
    assert!(failure.covers_page);
    assert_eq!(shown.title, "scam.example");
    assert!(!shown.loading);
    assert_eq!(shown.icon, None, "the blocked page wears no site's icon");
    shell.handle(Command::LeaveBlockedPage(id));
    let shown = tab(&screen);
    assert_eq!(shown.failure, None);
    assert_eq!(shown.url.as_deref(), Some("https://news.example/"));
    assert_eq!(shown.title, page_title);
    assert!(shown.icon.is_some(), "the page's icon returns with it");

    // A second refusal in the same session, for the lists' own reason, is
    // explained the same way.
    block_for(
        &mut shell,
        "https://ads.example/",
        NavigationBlockReason::FilterLists,
    );
    let shown = tab(&screen);
    let failure = shown.failure.expect("the second refusal is explained");
    assert_eq!(failure.blocked, Some(TabBlockReason::FilterLists));
    assert_eq!(failure.url, "https://ads.example/");
    assert_eq!(shown.title, "ads.example");
    shell.handle(Command::LeaveBlockedPage(id));

    // Continue lifts only the refusal the person was shown.
    block(&mut shell);
    let allowed = |engine: &FakeEngine| {
        engine
            .calls()
            .iter()
            .filter(|call| call.starts_with("allow-blocked"))
            .count()
    };
    shell.handle(Command::ContinuePastBlock {
        id,
        url: "https://other.example/".into(),
    });
    assert_eq!(allowed(&engine), 0, "a refusal the person never saw stays");
    shell.handle(Command::ContinuePastBlock {
        id,
        url: "https://scam.example/win".into(),
    });
    assert!(engine
        .calls()
        .contains(&format!("allow-blocked {id} https://scam.example/win")));
    commit_url(&mut shell, id, "https://scam.example/win");
    assert_eq!(tab(&screen).failure, None);
}

#[test]
fn going_back_from_a_restored_tab_s_refused_page_leaves_no_address_behind() {
    use zephium_core::ports::engine::NavigationBlockReason;
    let (mut shell, _engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    let id = active_id(&screen);
    // A restored tab holds its last address, and its view is refused it.
    commit_url(&mut shell, id, "https://scam.example/win");
    shell.handle(Command::Engine(EngineEvent::NavigationBlocked {
        id,
        url: "https://scam.example/win".into(),
        reason: NavigationBlockReason::Badware,
        request: None,
        can_continue: true,
    }));
    shell.handle(Command::LeaveBlockedPage(id));
    let shown = last(&screen)
        .tabs
        .into_iter()
        .find(|tab| tab.id == id.to_string())
        .unwrap();
    assert_eq!(shown.failure, None);
    assert_eq!(shown.url, None);
    assert_eq!(shown.title, "New Tab");
}

#[test]
fn a_refusal_the_engine_itself_enforces_offers_no_way_past() {
    use zephium_core::ports::engine::NavigationBlockReason;
    let (mut shell, engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    let id = active_id(&screen);
    navigate_and_commit(&mut shell, id, "news.example");
    // WebKit's content rules refused the page: allowing the host cannot open it.
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
