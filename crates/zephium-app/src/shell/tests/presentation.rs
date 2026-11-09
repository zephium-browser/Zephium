use super::*;

#[test]
fn committed_url_projection_precedes_immediate_exact_presentation() {
    let (mut shell, engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    let id = active_id(&screen);
    shell.handle(Command::Navigate {
        id,
        input: "https://example.test/".into(),
    });
    let navigation = NavigationPresentationId::from_raw(41);
    let queue = CommandQueue::new();
    shell.self_queue = Some(queue.clone());
    assert!(shell.items.tab(id).is_some_and(TabState::has_view));
    shell.items.set_loading(id, true);
    commit_url(&mut shell, id, "https://example.test/");
    assert_eq!(
        last(&screen)
            .tabs
            .into_iter()
            .find(|tab| tab.id == id.to_string())
            .and_then(|tab| tab.url),
        None,
        "the ordinary URL fact must retain the real New Tab projection"
    );

    shell.handle(Command::Engine(presentation_pending(
        id,
        navigation,
        "https://example.test/",
    )));
    assert_eq!(
        last(&screen)
            .tabs
            .into_iter()
            .find(|tab| tab.id == id.to_string())
            .and_then(|tab| tab.url),
        Some("https://example.test/".into())
    );
    assert!(engine
        .calls()
        .iter()
        .any(|call| call == &format!("present {id} 41")));

    // Finished is an idempotent re-drive, not a second reveal or a
    // first-paint prerequisite.
    shell.handle(Command::Engine(presentation_ready(
        id,
        navigation,
        "https://example.test/",
    )));

    assert_eq!(
        engine
            .calls()
            .iter()
            .filter(|call| *call == &format!("present {id} 41"))
            .count(),
        1
    );
    assert!(queue
        .inner
        .timer_state
        .lock()
        .unwrap()
        .presentation_deadlines
        .is_empty());
}

#[test]
fn raw_presentation_waits_for_exact_privileged_chrome_callback() {
    let (mut shell, engine, chrome, screen) = setup_with_async_chrome();
    let queue = CommandQueue::new();
    shell.self_queue = Some(queue.clone());
    shell.handle(Command::Bootstrap);
    while let Some(command) = queue.try_recv() {
        assert!(matches!(command, Command::BlockerReady(_)));
        shell.handle(command);
    }
    let id = active_id(&screen);
    shell.handle(Command::Navigate {
        id,
        input: "https://verified.example/".into(),
    });
    assert!(shell.items.tab(id).is_some_and(TabState::has_view));
    let layout_calls_before_commit = engine
        .calls()
        .iter()
        .filter(|call| call.starts_with("layout@"))
        .count();
    commit_url(&mut shell, id, "https://verified.example/");
    assert_eq!(
        shell
            .items
            .tab(id)
            .and_then(|tab| tab.url.as_ref())
            .map(url::Url::as_str),
        Some("https://verified.example/")
    );
    let navigation = NavigationPresentationId::from_raw(4201);

    shell.handle(Command::Engine(presentation_pending(
        id,
        navigation,
        "https://verified.example/",
    )));

    assert_eq!(
        engine
            .calls()
            .iter()
            .filter(|call| call.starts_with("layout@"))
            .count(),
        layout_calls_before_commit,
        "the real New Tab frame remains allocated until exact chrome verification"
    );
    assert!(!engine
        .calls()
        .iter()
        .any(|call| call == &format!("present {id} 4201")));
    let presentation = chrome.presentations().pop().unwrap();
    assert_eq!(presentation.id, id);
    assert_eq!(presentation.url, "https://verified.example/");
    assert_eq!(presentation.active, Some(id));
    assert_eq!(
        presentation.tab.url.as_deref(),
        Some("https://verified.example/")
    );
    assert!(queue
        .inner
        .timer_state
        .lock()
        .unwrap()
        .presentation_deadlines
        .contains_key(&id));

    chrome.complete_next(true);
    let callback = queue
        .try_recv()
        .expect("callback must enter the actor queue");
    assert!(matches!(
        &callback,
        Command::ChromePresentationApplied {
            id: observed,
            navigation: observed_navigation,
            url,
            active: Some(observed_active),
            projection_revision: _,
            applied: true,
        } if *observed == id
            && *observed_navigation == navigation
            && url == "https://verified.example/"
            && *observed_active == id
    ));
    shell.handle(callback);

    assert_eq!(
        engine
            .calls()
            .iter()
            .filter(|call| call.starts_with("layout@"))
            .count(),
        layout_calls_before_commit + 1
    );
    assert!(engine
        .calls()
        .iter()
        .any(|call| call == &format!("present {id} 4201")));
    assert!(!shell.presentation.pending_presentations.contains_key(&id));
    assert!(!queue
        .inner
        .timer_state
        .lock()
        .unwrap()
        .presentation_deadlines
        .contains_key(&id));
}

#[test]
fn newer_same_tab_projection_invalidates_a_queued_chrome_success_callback() {
    let (mut shell, engine, chrome, screen) = setup_with_async_chrome();
    let queue = CommandQueue::new();
    shell.self_queue = Some(queue.clone());
    shell.handle(Command::Bootstrap);
    let id = active_id(&screen);
    shell.handle(Command::Navigate {
        id,
        input: "https://ordered.example/".into(),
    });
    commit_url(&mut shell, id, "https://ordered.example/");
    let navigation = NavigationPresentationId::from_raw(4_211);
    shell.handle(Command::Engine(presentation_pending(
        id,
        navigation,
        "https://ordered.example/",
    )));
    let first = chrome.presentations().into_iter().next().unwrap();
    let hard_deadline = shell.presentation.pending_presentations[&id].hard_deadline;

    // The native eval reports success, but its callback has not yet
    // reached the actor. Model every generic projection source that may
    // run in that interval; each remains New-Tab-masked yet advances this
    // item's exact emitted revision.
    chrome.complete_next(true);
    shell.handle(Command::Engine(EngineEvent::NavState {
        id,
        can_go_back: true,
        can_go_forward: false,
    }));
    shell.handle(Command::Engine(EngineEvent::LoadingChanged {
        id,
        loading: true,
    }));
    shell.handle(Command::Engine(EngineEvent::TitleChanged {
        id,
        title: "New document title".into(),
    }));
    shell.project_items();
    let latest_revision = shell
        .presentation
        .last_tab_projection_revision
        .borrow()
        .get(&id)
        .cloned()
        .unwrap();
    assert!(latest_revision > first.tab.projection_revision);
    let masked = last(&screen)
        .tabs
        .into_iter()
        .find(|tab| tab.id == id.to_string())
        .unwrap();
    assert_eq!(masked.url, None);
    assert_eq!(masked.title, "New Tab");

    let stale_success = queue.try_recv().unwrap();
    shell.handle(stale_success);
    assert!(!engine
        .calls()
        .iter()
        .any(|call| call == &format!("present {id} 4211")));
    assert!(shell.presentation.pending_presentations.contains_key(&id));

    // The existing exact timer reprojects the newest authoritative tab.
    shell.on_presentation_fallback(id, navigation, hard_deadline);
    let retry = chrome.presentations().into_iter().next().unwrap();
    assert!(retry.tab.projection_revision > latest_revision);
    assert_eq!(retry.tab.title, "New document title");
    chrome.complete_next(true);
    shell.handle(queue.try_recv().unwrap());

    assert!(engine
        .calls()
        .iter()
        .any(|call| call == &format!("present {id} 4211")));
    assert!(!shell.presentation.pending_presentations.contains_key(&id));
}

#[test]
fn overlapping_privileged_callbacks_cannot_acknowledge_the_newer_document() {
    let (mut shell, engine, chrome, screen) = setup_with_async_chrome();
    let queue = CommandQueue::new();
    shell.self_queue = Some(queue.clone());
    shell.handle(Command::Bootstrap);
    let id = active_id(&screen);
    shell.handle(Command::Navigate {
        id,
        input: "https://first.example/".into(),
    });
    assert!(shell.items.tab(id).is_some_and(TabState::has_view));
    let retired = NavigationPresentationId::from_raw(4202);
    let current = NavigationPresentationId::from_raw(4203);

    commit_url(&mut shell, id, "https://first.example/");
    shell.handle(Command::Engine(presentation_pending(
        id,
        retired,
        "https://first.example/",
    )));
    commit_url(&mut shell, id, "https://second.example/");
    shell.handle(Command::Engine(presentation_pending(
        id,
        current,
        "https://second.example/",
    )));
    assert_eq!(chrome.presentations().len(), 2);
    let revisions = chrome
        .presentations()
        .into_iter()
        .map(|presentation| presentation.tab.projection_revision)
        .collect::<Vec<_>>();
    assert!(revisions[0] < revisions[1]);

    let first = chrome.complete_next(true);
    assert_eq!(first.navigation, retired);
    shell.handle(queue.try_recv().unwrap());
    assert!(!engine
        .calls()
        .iter()
        .any(|call| call == &format!("present {id} 4202")));
    assert_eq!(
        shell.presentation.pending_presentations[&id].navigation,
        current
    );

    let second = chrome.complete_next(true);
    assert_eq!(second.navigation, current);
    shell.handle(queue.try_recv().unwrap());
    assert!(engine
        .calls()
        .iter()
        .any(|call| call == &format!("present {id} 4203")));
}

#[test]
fn lost_privileged_callback_keeps_one_exact_retry_and_never_timeout_reveals() {
    let (mut shell, engine, chrome, screen) = setup_with_async_chrome();
    let queue = CommandQueue::new();
    shell.self_queue = Some(queue.clone());
    shell.handle(Command::Bootstrap);
    let id = active_id(&screen);
    shell.handle(Command::Navigate {
        id,
        input: "https://lost-callback.example/".into(),
    });
    assert!(shell.items.tab(id).is_some_and(TabState::has_view));
    let navigation = NavigationPresentationId::from_raw(4204);
    commit_url(&mut shell, id, "https://lost-callback.example/");
    shell.handle(Command::Engine(presentation_pending(
        id,
        navigation,
        "https://lost-callback.example/",
    )));
    let first = shell.presentation.pending_presentations[&id].clone();
    assert!(first.chrome_request_in_flight);

    shell.on_presentation_fallback(id, navigation, first.hard_deadline);
    assert_eq!(chrome.presentations().len(), 2);
    assert_eq!(
        shell.presentation.pending_presentations[&id].admission_rejections,
        1
    );
    assert!(!engine
        .calls()
        .iter()
        .any(|call| call == &format!("present {id} 4204")));

    let expired = std::time::Instant::now();
    shell
        .presentation
        .pending_presentations
        .get_mut(&id)
        .unwrap()
        .hard_deadline = expired;
    shell.on_presentation_fallback(id, navigation, expired);
    assert!(!shell.items.tab(id).unwrap().has_view());
    assert!(engine
        .calls()
        .iter()
        .any(|call| call == &format!("close {id}")));
    assert!(!engine
        .calls()
        .iter()
        .any(|call| call == &format!("present {id} 4204")));
}

#[test]
fn cross_origin_commit_neutralizes_prior_title_before_exact_presentation_ack() {
    let (mut shell, engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    let id = active_id(&screen);
    navigate_and_commit(&mut shell, id, "https://trusted.example/account");
    shell.handle(Command::Engine(EngineEvent::TitleChanged {
        id,
        title: "Trusted Account".into(),
    }));
    assert_eq!(shell.items.tab(id).unwrap().title, "Trusted Account");

    shell.handle(Command::Engine(EngineEvent::UrlChanged {
        id,
        url: "https://redirected.example/login".into(),
    }));
    let tab = shell.items.tab(id).unwrap();
    assert_eq!(
        tab.url.as_ref().map(url::Url::as_str),
        Some("https://redirected.example/login")
    );
    assert_eq!(tab.title, "redirected.example");

    let navigation = NavigationPresentationId::from_raw(410);
    shell.handle(Command::Engine(presentation_pending(
        id,
        navigation,
        "https://redirected.example/login",
    )));
    shell.handle(Command::Engine(presentation_ready(
        id,
        navigation,
        "https://redirected.example/login",
    )));
    assert!(engine
        .calls()
        .iter()
        .any(|call| call == &format!("present {id} 410")));
    assert_eq!(shell.items.tab(id).unwrap().title, "redirected.example");

    shell.handle(Command::Engine(EngineEvent::TitleChanged {
        id,
        title: "Redirected Login".into(),
    }));
    assert_eq!(shell.items.tab(id).unwrap().title, "Redirected Login");
}

#[test]
fn same_origin_history_url_observation_preserves_current_document_title() {
    let (mut shell, _engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    let id = active_id(&screen);
    navigate_and_commit(&mut shell, id, "https://same.example/first");
    shell.handle(Command::Engine(EngineEvent::TitleChanged {
        id,
        title: "Same document title".into(),
    }));

    shell.handle(Command::Engine(EngineEvent::UrlChanged {
        id,
        url: "https://same.example/second#state".into(),
    }));
    assert_eq!(shell.items.tab(id).unwrap().title, "Same document title");
}

#[test]
fn loading_state_never_delays_exact_committed_url_presentation() {
    let (mut shell, engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    let id = active_id(&screen);
    shell.handle(Command::Navigate {
        id,
        input: "https://example.test/".into(),
    });
    shell.items.set_loading(id, true);
    let navigation = NavigationPresentationId::from_raw(42);
    let queue = CommandQueue::new();
    shell.self_queue = Some(queue.clone());
    commit_url(&mut shell, id, "https://example.test/");

    shell.handle(Command::Engine(presentation_pending(
        id,
        navigation,
        "https://example.test/",
    )));

    assert!(engine
        .calls()
        .iter()
        .any(|call| call == &format!("present {id} 42")));
    assert!(!shell.presentation.pending_presentations.contains_key(&id));
    assert!(queue
        .inner
        .timer_state
        .lock()
        .unwrap()
        .presentation_deadlines
        .is_empty());
}

#[test]
fn presentation_hard_limit_retires_hidden_content_instead_of_timeout_revealing() {
    let (mut shell, engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    let id = active_id(&screen);
    shell.handle(Command::Navigate {
        id,
        input: "https://example.test/".into(),
    });
    shell.items.set_loading(id, true);
    let navigation = NavigationPresentationId::from_raw(43);
    let queue = CommandQueue::new();
    shell.self_queue = Some(queue.clone());
    commit_url(&mut shell, id, "https://example.test/");
    let hard_deadline = std::time::Instant::now()
        .checked_sub(std::time::Duration::from_millis(1))
        .unwrap_or_else(std::time::Instant::now);
    shell.presentation.pending_presentations.insert(
        id,
        PendingPresentation {
            navigation,
            url: "https://example.test/".into(),
            hard_deadline,
            admission_rejections: 0,
            chrome_applied: true,
            chrome_request_in_flight: false,
        },
    );

    shell.on_presentation_fallback(id, navigation, hard_deadline);

    assert!(!engine
        .calls()
        .iter()
        .any(|call| call == &format!("present {id} 43")));
    assert!(engine
        .calls()
        .iter()
        .any(|call| call == &format!("close {id}")));
    assert!(!shell.items.tab(id).unwrap().has_view());
    assert!(!shell.presentation.pending_presentations.contains_key(&id));
    assert!(!queue
        .inner
        .timer_state
        .lock()
        .unwrap()
        .presentation_deadlines
        .contains_key(&id));
}

#[test]
fn ready_presentation_rejection_retains_and_retries_the_exact_obligation() {
    let (mut shell, engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    let id = active_id(&screen);
    shell.handle(Command::Navigate {
        id,
        input: "https://example.test/".into(),
    });
    let navigation = NavigationPresentationId::from_raw(431);
    let queue = CommandQueue::new();
    shell.self_queue = Some(queue.clone());
    engine
        .reject_native_dispatch
        .store(true, std::sync::atomic::Ordering::Release);
    commit_url(&mut shell, id, "https://example.test/");

    // Model bounded event coalescing where Ready replaces Pending. The
    // exact reveal obligation must still be materialized before dispatch.
    shell.handle(Command::Engine(presentation_ready(
        id,
        navigation,
        "https://example.test/",
    )));

    let pending = shell.presentation.pending_presentations[&id].clone();
    assert_eq!(pending.navigation, navigation);
    assert_eq!(pending.admission_rejections, 1);
    assert_eq!(
        queue
            .inner
            .timer_state
            .lock()
            .unwrap()
            .presentation_deadlines[&id]
            .navigation,
        navigation
    );

    engine
        .reject_native_dispatch
        .store(false, std::sync::atomic::Ordering::Release);
    queue.cancel_presentation(id);
    shell.on_presentation_fallback(id, navigation, pending.hard_deadline);

    assert!(!shell.presentation.pending_presentations.contains_key(&id));
    assert_eq!(
        engine
            .calls()
            .iter()
            .filter(|call| *call == &format!("present {id} 431"))
            .count(),
        1,
        "a rejected first layout must not attempt native reveal"
    );
}

#[test]
fn fallback_presentation_rejection_retains_and_retries_the_exact_obligation() {
    let (mut shell, engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    let id = active_id(&screen);
    shell.handle(Command::Navigate {
        id,
        input: "https://example.test/".into(),
    });
    shell.items.set_loading(id, false);
    let navigation = NavigationPresentationId::from_raw(432);
    let queue = CommandQueue::new();
    shell.self_queue = Some(queue.clone());
    commit_url(&mut shell, id, "https://example.test/");
    engine
        .reject_native_dispatch
        .store(true, std::sync::atomic::Ordering::Release);
    shell.handle(Command::Engine(presentation_pending(
        id,
        navigation,
        "https://example.test/",
    )));
    let hard_deadline = shell.presentation.pending_presentations[&id].hard_deadline;

    assert_eq!(
        shell.presentation.pending_presentations[&id].admission_rejections,
        1
    );
    assert_eq!(
        queue
            .inner
            .timer_state
            .lock()
            .unwrap()
            .presentation_deadlines[&id]
            .navigation,
        navigation
    );

    engine
        .reject_native_dispatch
        .store(false, std::sync::atomic::Ordering::Release);
    queue.cancel_presentation(id);
    shell.on_presentation_fallback(id, navigation, hard_deadline);

    assert!(!shell.presentation.pending_presentations.contains_key(&id));
    assert!(!queue
        .inner
        .timer_state
        .lock()
        .unwrap()
        .presentation_deadlines
        .contains_key(&id));
}

#[test]
fn permanently_rejected_presentation_retires_the_exact_hidden_view() {
    let (mut shell, engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    let id = active_id(&screen);
    shell.handle(Command::Navigate {
        id,
        input: "https://example.test/".into(),
    });
    shell.items.set_loading(id, false);
    let navigation = NavigationPresentationId::from_raw(433);
    let queue = CommandQueue::new();
    shell.self_queue = Some(queue.clone());
    commit_url(&mut shell, id, "https://example.test/");
    engine
        .reject_native_dispatch
        .store(true, std::sync::atomic::Ordering::Release);
    shell.handle(Command::Engine(presentation_pending(
        id,
        navigation,
        "https://example.test/",
    )));
    let hard_deadline = shell.presentation.pending_presentations[&id].hard_deadline;

    assert_eq!(
        shell.presentation.pending_presentations[&id].admission_rejections,
        1
    );
    for rejection in 2..=MAX_PRESENTATION_ADMISSION_REJECTIONS {
        // Model each coalesced timer wake entering the actor. There can be
        // only one timer and one shell obligation for this item.
        queue.cancel_presentation(id);
        shell.on_presentation_fallback(id, navigation, hard_deadline);
        if rejection < MAX_PRESENTATION_ADMISSION_REJECTIONS {
            assert_eq!(
                shell.presentation.pending_presentations[&id].admission_rejections,
                rejection
            );
            assert_eq!(
                queue
                    .inner
                    .timer_state
                    .lock()
                    .unwrap()
                    .presentation_deadlines
                    .len(),
                1
            );
        }
    }

    assert!(!shell.presentation.pending_presentations.contains_key(&id));
    assert!(!shell.items.tab(id).unwrap().has_view());
    assert!(!queue
        .inner
        .timer_state
        .lock()
        .unwrap()
        .presentation_deadlines
        .contains_key(&id));
    assert!(engine
        .calls()
        .iter()
        .any(|call| call == &format!("close {id}")));
}

#[test]
fn unsupported_acknowledgement_is_an_exact_view_lifecycle_failure() {
    let (mut shell, engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    let id = active_id(&screen);
    shell.handle(Command::Navigate {
        id,
        input: "https://example.test/".into(),
    });
    let navigation = NavigationPresentationId::from_raw(434);
    let queue = CommandQueue::new();
    shell.self_queue = Some(queue.clone());
    engine
        .unsupported_presentation
        .store(true, std::sync::atomic::Ordering::Release);
    commit_url(&mut shell, id, "https://example.test/");

    shell.handle(Command::Engine(presentation_ready(
        id,
        navigation,
        "https://example.test/",
    )));

    assert!(!shell.presentation.pending_presentations.contains_key(&id));
    assert!(!shell.items.tab(id).unwrap().has_view());
    assert!(engine
        .calls()
        .iter()
        .any(|call| call == &format!("close {id}")));
}

#[test]
fn terminal_shutdown_does_not_rearm_a_rejected_presentation_wake() {
    let (mut shell, engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    let id = active_id(&screen);
    shell.handle(Command::Navigate {
        id,
        input: "https://example.test/".into(),
    });
    let navigation = NavigationPresentationId::from_raw(435);
    let queue = CommandQueue::new();
    shell.self_queue = Some(queue.clone());
    commit_url(&mut shell, id, "https://example.test/");
    engine
        .reject_native_dispatch
        .store(true, std::sync::atomic::Ordering::Release);
    shell.handle(Command::Engine(presentation_pending(
        id,
        navigation,
        "https://example.test/",
    )));
    let pending = shell.presentation.pending_presentations[&id].clone();

    let (barrier_ack, _barrier_done) = sync_channel(1);
    queue
        .try_push(Command::Shutdown {
            deadline: test_shutdown_deadline(),
            ack: barrier_ack,
        })
        .unwrap_or_else(|_| panic!("shutdown barrier must enter its reserved slot"));
    assert!(matches!(queue.try_recv(), Some(Command::Shutdown { .. })));

    // Model wait_for_timer removing the entry before the corresponding
    // command discovers that the actor is sealed behind the barrier.
    queue.cancel_presentation(id);
    assert!(matches!(
        queue.try_push(Command::PresentationFallback {
            id,
            navigation,
            hard_deadline: pending.hard_deadline,
        }),
        Err(TryPushError::Sealed(_))
    ));
    assert!(!queue
        .inner
        .timer_state
        .lock()
        .unwrap()
        .presentation_deadlines
        .contains_key(&id));

    let (ack, done) = sync_channel(1);
    shell.handle(Command::Shutdown {
        deadline: test_shutdown_deadline(),
        ack,
    });
    assert_eq!(done.recv().unwrap(), ShutdownOutcome::Clean);
    assert!(!queue
        .inner
        .timer_state
        .lock()
        .unwrap()
        .presentation_deadlines
        .contains_key(&id));
}

#[test]
fn escaped_stale_presentation_fallback_cannot_rearm_over_a_new_navigation() {
    let (mut shell, engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    let id = active_id(&screen);
    shell.handle(Command::Navigate {
        id,
        input: "https://example.test/".into(),
    });
    shell.items.set_loading(id, true);
    let retired = NavigationPresentationId::from_raw(44);
    let current = NavigationPresentationId::from_raw(45);
    let queue = CommandQueue::new();
    shell.self_queue = Some(queue.clone());
    engine
        .reject_native_dispatch
        .store(true, std::sync::atomic::Ordering::Release);

    commit_url(&mut shell, id, "https://first.example/");
    shell.handle(Command::Engine(presentation_pending(
        id,
        retired,
        "https://first.example/",
    )));
    let retired_hard = shell.presentation.pending_presentations[&id].hard_deadline;
    commit_url(&mut shell, id, "https://second.example/");
    shell.handle(Command::Engine(presentation_pending(
        id,
        current,
        "https://second.example/",
    )));
    let current_pending = shell.presentation.pending_presentations[&id].clone();
    let calls_before_stale_wake = engine.calls().len();

    shell.on_presentation_fallback(id, retired, retired_hard);

    assert_eq!(
        shell.presentation.pending_presentations[&id],
        current_pending
    );
    assert_eq!(
        queue
            .inner
            .timer_state
            .lock()
            .unwrap()
            .presentation_deadlines[&id]
            .navigation,
        current
    );
    assert_eq!(engine.calls().len(), calls_before_stale_wake);
}

#[test]
fn same_document_url_refresh_preserves_presentation_and_rejects_stale_facts() {
    let (mut shell, engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    let id = active_id(&screen);
    let original = "https://chromewebstore.google.com/category/extensions";
    let listing =
        "https://chromewebstore.google.com/detail/vimium/dbepggeogbaibhgnhhndojpepiihcmeb";
    shell.handle(Command::Navigate {
        id,
        input: original.into(),
    });
    let navigation = NavigationPresentationId::from_raw(420);
    commit_url(&mut shell, id, original);
    shell.handle(Command::Engine(presentation_pending(
        id, navigation, original,
    )));
    let presents = engine
        .calls()
        .iter()
        .filter(|call| call.starts_with("present "))
        .count();
    commit_url(&mut shell, id, listing);
    // URL attribution alone does not manufacture presentation evidence.
    assert_eq!(
        shell.presentation.presented_navigations.get(&id).unwrap().1,
        original
    );
    shell.handle(Command::Engine(presentation_pending(
        id, navigation, listing,
    )));
    assert_eq!(
        shell.presentation.presented_navigations.get(&id),
        Some(&(navigation, listing.into()))
    );
    assert!(!shell.presentation.pending_presentations.contains_key(&id));
    assert_eq!(
        engine
            .calls()
            .iter()
            .filter(|call| call.starts_with("present "))
            .count(),
        presents
    );
    shell.handle(Command::Engine(presentation_pending(
        id, navigation, original,
    )));
    assert_eq!(
        shell.presentation.presented_navigations.get(&id).unwrap().1,
        listing
    );
}
