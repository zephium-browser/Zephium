use super::*;

#[test]
fn final_discard_veto_preserves_view_and_replays_latest_foreground_navigation() {
    let (mut shell, engine, screen) = setup();
    shell.residency.warm_view_limit = 1;
    shell.residency.live_view_pressure_limit = 1;
    shell.handle(Command::Bootstrap);
    let candidate = active_id(&screen);
    navigate_and_commit(&mut shell, candidate, "retained.example");
    shell.items.set_title(candidate, "Retained document".into());
    let resident = shell.residency.resident_since[&candidate];
    shell.handle(Command::Open);
    let active = active_id(&screen);
    navigate_and_commit(&mut shell, active, "foreground.example");
    let (_, probe) = first_probing_discard(&shell);
    shell.handle(Command::Engine(EngineEvent::DiscardSafety {
        id: candidate,
        probe,
        can_discard: true,
    }));
    assert_eq!(shell.residency.resident_since[&candidate], resident);
    shell.handle(Command::Activate(candidate));
    shell.handle(Command::Navigate {
        id: candidate,
        input: "latest.example".into(),
    });
    let profile = shell.profile_of_item(candidate).unwrap();
    shell.handle(Command::Engine(EngineEvent::ViewDiscardRefused {
        id: candidate,
        profile,
        probe: DiscardProbeId(probe.0 + 1),
    }));
    assert!(shell.residency.discard_probes.contains_key(&candidate));
    shell.handle(Command::Engine(EngineEvent::ViewDiscardRefused {
        id: candidate,
        profile,
        probe,
    }));
    assert!(shell.items.tab(candidate).unwrap().has_view());
    assert_eq!(
        shell.items.tab(candidate).unwrap().title,
        "Retained document"
    );
    assert_eq!(shell.residency.resident_since[&candidate], resident);
    assert!(!shell.residency.discard_probes.contains_key(&candidate));
    assert!(engine
        .calls()
        .iter()
        .any(|call| call.starts_with(&format!("cancel-discard {candidate} "))));
    assert!(engine
        .calls()
        .iter()
        .any(|call| call.starts_with(&format!("navigate {candidate} https://latest.example/"))));
    assert_eq!(
        engine
            .calls()
            .iter()
            .filter(|call| call.starts_with(&format!("create {candidate} ")))
            .count(),
        1
    );
}

#[test]
fn rejected_discard_admission_clears_closing_without_losing_residency() {
    let (mut shell, engine, screen) = setup();
    shell.residency.warm_view_limit = 1;
    shell.residency.live_view_pressure_limit = 1;
    shell.handle(Command::Bootstrap);
    let candidate = active_id(&screen);
    navigate_and_commit(&mut shell, candidate, "admission.example");
    let resident = shell.residency.resident_since[&candidate];
    shell.handle(Command::Open);
    let active = active_id(&screen);
    navigate_and_commit(&mut shell, active, "active.example");
    let (_, probe) = first_probing_discard(&shell);
    engine
        .reject_discard_dispatch
        .store(true, std::sync::atomic::Ordering::Release);
    shell.handle(Command::Engine(EngineEvent::DiscardSafety {
        id: candidate,
        probe,
        can_discard: true,
    }));
    assert!(shell.items.tab(candidate).unwrap().has_view());
    assert_eq!(shell.residency.resident_since[&candidate], resident);
    assert!(shell.residency.discard_probes.is_empty());
    assert!(shell
        .residency
        .discard_protected_until
        .contains_key(&candidate));
}

#[test]
fn explicit_close_during_final_discard_check_still_closes_and_forgets_native_state() {
    let (mut shell, engine, screen) = setup();
    shell.residency.warm_view_limit = 1;
    shell.residency.live_view_pressure_limit = 1;
    shell.handle(Command::Bootstrap);
    let candidate = active_id(&screen);
    navigate_and_commit(&mut shell, candidate, "close-race.example");
    shell.handle(Command::Open);
    let active = active_id(&screen);
    navigate_and_commit(&mut shell, active, "active.example");
    let (_, probe) = first_probing_discard(&shell);
    shell.handle(Command::Engine(EngineEvent::DiscardSafety {
        id: candidate,
        probe,
        can_discard: true,
    }));
    let profile = shell.profile_of_item(candidate).unwrap();
    shell.handle(Command::Close(candidate));
    assert!(shell.items.tab(candidate).is_none());
    assert!(!shell.residency.discard_probes.contains_key(&candidate));
    assert!(engine
        .calls()
        .iter()
        .any(|call| call == &format!("close {candidate}")));
    assert!(engine
        .calls()
        .iter()
        .any(|call| call == &format!("forget-discarded {profile} {candidate}")));
    shell.handle(Command::Engine(EngineEvent::ViewDiscardRefused {
        id: candidate,
        profile,
        probe,
    }));
    assert!(shell.items.tab(candidate).is_none());
}

#[test]
fn renderer_death_supersedes_final_discard_and_cannot_strand_recovery() {
    let (mut shell, engine, screen) = setup();
    shell.residency.warm_view_limit = 1;
    shell.residency.live_view_pressure_limit = 1;
    shell.handle(Command::Bootstrap);
    let candidate = active_id(&screen);
    navigate_and_commit(&mut shell, candidate, "crash-race.example");
    shell.handle(Command::Open);
    let active = active_id(&screen);
    navigate_and_commit(&mut shell, active, "active.example");
    let (_, probe) = first_probing_discard(&shell);
    shell.handle(Command::Engine(EngineEvent::DiscardSafety {
        id: candidate,
        probe,
        can_discard: true,
    }));
    shell.handle(Command::Activate(candidate));
    shell.handle(Command::Engine(EngineEvent::Crashed { id: candidate }));
    assert!(!shell.residency.discard_probes.contains_key(&candidate));
    assert!(shell.items.tab(candidate).unwrap().has_view());
    shell.handle(Command::Reload(candidate));
    assert!(engine
        .calls()
        .iter()
        .any(|call| call == &format!("reload {candidate}")));
}

#[test]
fn loading_background_documents_are_not_requested_to_suspend() {
    let (mut shell, _, screen) = setup();
    shell.handle(Command::Bootstrap);
    let candidate = active_id(&screen);
    navigate_and_commit(&mut shell, candidate, "long-load.example");
    shell.handle(Command::Open);
    shell.items.set_loading(candidate, true);
    shell.residency.dormant_min = std::time::Duration::ZERO;
    shell.handle(Command::Tick);
    assert!(!shell.residency.dormant_sent.contains(&candidate));
    shell.items.set_loading(candidate, false);
    shell.handle(Command::Tick);
    assert!(shell.residency.dormant_sent.contains(&candidate));
}

#[test]
fn disabling_sleep_cancels_a_pending_close_without_reloading_hidden_tabs() {
    let (mut shell, engine, screen) = setup();
    shell.residency.warm_view_limit = 1;
    shell.residency.live_view_pressure_limit = 1;
    shell.handle(Command::Bootstrap);
    let candidate = active_id(&screen);
    navigate_and_commit(&mut shell, candidate, "hidden-context.example");
    shell.handle(Command::Open);
    let active = active_id(&screen);
    navigate_and_commit(&mut shell, active, "active.example");
    let (_, probe) = first_probing_discard(&shell);
    shell.handle(Command::Engine(EngineEvent::DiscardSafety {
        id: candidate,
        probe,
        can_discard: true,
    }));
    shell.handle(Command::SetAppSetting {
        key: "performance.sleep".into(),
        value: "false".into(),
    });
    assert!(engine
        .calls()
        .iter()
        .any(|call| call == &format!("cancel-discard {candidate} {}", probe.0)));
    // If physical retirement already won, its acknowledgment stays truthful;
    // changing policy does not eagerly reconstruct an unseen page.
    shell.handle(Command::Engine(EngineEvent::ViewDiscarded {
        id: candidate,
        profile: shell.profile_of_item(candidate).unwrap(),
        probe,
    }));
    assert!(!shell.items.tab(candidate).unwrap().has_view());
    assert_eq!(
        engine
            .calls()
            .iter()
            .filter(|call| call.starts_with(&format!("create {candidate} ")))
            .count(),
        1
    );
    let profile = shell.profile_of_item(candidate).unwrap();
    shell.handle(Command::Close(candidate));
    assert!(engine
        .calls()
        .iter()
        .any(|call| call == &format!("forget-discarded {profile} {candidate}")));
}

#[test]
fn leaving_a_visible_split_starts_idle_grace_and_minimization_keeps_discard_protection() {
    use zephium_core::ports::engine::MemoryPressure;
    let (mut shell, _, screen) = setup();
    shell.handle(Command::Bootstrap);
    let first = active_id(&screen);
    navigate_and_commit(&mut shell, first, "read-first.example");
    shell.handle(Command::Open);
    let second = active_id(&screen);
    navigate_and_commit(&mut shell, second, "read-second.example");
    shell.handle(Command::SplitWith {
        other: first,
        axis: Axis::Row,
    });
    shell.residency.warm_view_limit = 1;
    // A freshly booted machine (a hosted Windows runner) cannot represent a
    // moment before its own uptime.
    let Some(old) = std::time::Instant::now().checked_sub(std::time::Duration::from_secs(30 * 60))
    else {
        return;
    };
    for id in [first, second] {
        shell.residency.last_focus.insert(id, old);
        shell.residency.resident_since.insert(id, old);
        shell.residency.inactive_since.insert(id, old);
    }

    shell.handle(Command::Unsplit);
    assert!(
        shell.residency.discard_probes.is_empty(),
        "a long foreground read does not consume the background grace"
    );
    assert!(shell.residency.dormant_sent.is_empty());
    assert_eq!(
        shell.residency.last_focus[&first], old,
        "hiding is not a focus event"
    );
    assert!(shell.residency.inactive_since[&first] > old);

    shell.handle(Command::SetWindowVisible(false));
    assert!(shell.discard_protected_leaves().contains(&second));
    assert!(shell.residency.inactive_since[&second] > old);
    assert!(
        shell.residency.dormant_sent.is_empty(),
        "minimization starts its own dormancy grace"
    );
    shell.handle(Command::SetMemoryPressure(MemoryPressure::Critical));
    assert!(shell.residency.discard_probes.contains_key(&first));
    assert!(!shell.residency.discard_probes.contains_key(&second));
}

#[test]
fn never_focused_fresh_background_view_respects_idle_grace_until_real_pressure() {
    use zephium_core::ports::engine::MemoryPressure;
    let (mut shell, engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    let active = active_id(&screen);
    navigate_and_commit(&mut shell, active, "foreground.example");
    shell.residency.warm_view_limit = 1;
    shell.residency.live_view_pressure_limit = LIVE_VIEW_PRESSURE_LIMIT;

    let result = shell.operation_open_url_background("fresh-background.example".into());
    assert_eq!(result.outcome, OperationOutcome::Deferred);
    let background = shell
        .items
        .view_ids()
        .into_iter()
        .find(|id| *id != active)
        .unwrap();
    navigate_and_commit(&mut shell, background, "fresh-background.example");
    shell.handle(Command::Tick);

    assert_eq!(active_id(&screen), active);
    assert!(
        !shell.residency.last_focus.contains_key(&background),
        "creation is not focus"
    );
    assert!(
        shell.residency.discard_probes.is_empty(),
        "the fresh background document has its full idle grace"
    );
    assert!(shell.residency.dormant_sent.is_empty());
    assert!(!engine
        .calls()
        .iter()
        .any(|call| call.starts_with(&format!("probe-discard {background} "))));

    shell.handle(Command::SetMemoryPressure(MemoryPressure::Critical));
    assert!(
        shell.residency.discard_probes.contains_key(&background),
        "real pressure intentionally bypasses idle grace"
    );
}

#[test]
fn rejected_view_creation_rolls_back_the_live_view_synchronously() {
    let (mut shell, engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    let id = active_id(&screen);
    engine
        .reject_create_dispatch
        .store(true, std::sync::atomic::Ordering::Release);

    shell.handle(Command::Navigate {
        id,
        input: "example.com".into(),
    });

    let tab = shell.items.tab(id).unwrap();
    assert!(!tab.has_view());
    assert!(tab.url.is_none());
    assert!(!tab.loading);
    assert_eq!(tab.title, "example.com");
    let failure = tab.failure.as_deref().unwrap();
    assert_eq!(failure.url.as_str(), "https://example.com/");
}

#[test]
fn failed_native_view_can_be_retried() {
    let (mut shell, engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    let id = active_id(&screen);
    shell.handle(Command::Navigate {
        id,
        input: "example.com".into(),
    });
    assert!(shell.items.tab(id).unwrap().has_view());

    shell.handle(Command::Engine(EngineEvent::ViewCreationFailed { id }));
    let tab = shell.items.tab(id).unwrap();
    assert!(!tab.has_view());
    assert!(!tab.loading);
    assert_eq!(tab.title, "example.com");
    assert!(tab.failure.is_some());

    shell.handle(Command::Navigate {
        id,
        input: "example.com".into(),
    });
    assert!(shell.items.tab(id).unwrap().failure.is_none());
    assert_eq!(
        engine
            .calls()
            .iter()
            .filter(|call| call.starts_with(&format!("create {id} ")))
            .count(),
        2
    );
}

#[test]
fn background_views_are_not_discarded_without_safety_signals() {
    let (mut shell, engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    let mut ids = vec![active_id(&screen)];
    shell.handle(Command::Navigate {
        id: ids[0],
        input: "site0.com".into(),
    });
    for n in 1..15 {
        shell.handle(Command::Open);
        let id = active_id(&screen);
        shell.handle(Command::Navigate {
            id,
            input: format!("site{n}.com"),
        });
        ids.push(id);
    }
    // 15 tabs but none idle: everything stays warm
    assert!(ids
        .iter()
        .all(|id| shell.items.tab(*id).unwrap().has_view()));

    shell.handle(Command::Tick);
    assert!(ids
        .iter()
        .all(|id| shell.items.tab(*id).unwrap().has_view()));
    assert!(!engine.calls().iter().any(|call| call.starts_with("close ")));
}

#[test]
fn crashed_page_recreates_once_then_reports() {
    let (mut shell, engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    let id = active_id(&screen);
    navigate_and_commit(&mut shell, id, "example.com");

    // First crash: the engine already removed the dead native generation;
    // the focused tab is recreated once without an id-only cleanup close.
    shell.handle(Command::Engine(EngineEvent::Crashed { id }));
    assert!(!engine.calls().iter().any(|c| c == &format!("close {id}")));
    assert_eq!(
        engine
            .calls()
            .iter()
            .filter(|call| call.starts_with(&format!("create {id} ")))
            .count(),
        2
    );

    // Second crash right after: no renderer-spawn loop, the tab reports it.
    shell.handle(Command::Engine(EngineEvent::Crashed { id }));
    assert_eq!(
        engine
            .calls()
            .iter()
            .filter(|call| call.starts_with(&format!("create {id} ")))
            .count(),
        2
    );
    let tab = last(&screen)
        .tabs
        .into_iter()
        .find(|t| t.id == id.to_string())
        .unwrap();
    assert_eq!(
        tab.failure.map(|failure| failure.reason),
        Some(zephium_ipc::TabFailureReason::Crashed)
    );
    assert!(!tab.loading);
}

#[test]
fn crash_presentation_never_overwrites_the_durable_document_title() {
    let store = Arc::new(FakeStore::default());
    let (mut shell, _engine, screen) = setup_with(store.clone());
    shell.handle(Command::Bootstrap);
    let id = active_id(&screen);
    navigate_and_commit(&mut shell, id, "durable-title.example");
    shell.handle(Command::Engine(EngineEvent::TitleChanged {
        id,
        title: "Last real title".into(),
    }));
    let profile = shell.windows.focused().unwrap().profile;

    shell.handle(Command::Engine(EngineEvent::ProfileProcessExited {
        profile,
        ids: vec![id],
    }));
    let projected = last(&screen)
        .tabs
        .into_iter()
        .find(|tab| tab.id == id.to_string())
        .unwrap();
    assert_eq!(projected.title, "Last real title");
    assert_eq!(
        projected.failure.map(|failure| failure.reason),
        Some(zephium_ipc::TabFailureReason::Crashed)
    );

    let saved = store.saved.lock().unwrap().clone().unwrap();
    let durable_title = saved.items.into_iter().find_map(|item| {
        (item.id == id)
            .then_some(item.kind)
            .and_then(|kind| match kind {
                PersistedKind::Tab { title, .. } => Some(title),
                PersistedKind::Folder { .. } => None,
                PersistedKind::BrowserTab { .. } => None,
            })
    });
    assert_eq!(durable_title.as_deref(), Some("Last real title"));
}

#[test]
fn hidden_renderer_crash_does_not_close_a_future_generation_or_relaunch() {
    let store = Arc::new(FakeStore::default());
    let (mut shell, engine, screen) = setup_with(store.clone());
    shell.handle(Command::Bootstrap);
    let hidden = active_id(&screen);
    shell.handle(Command::Navigate {
        id: hidden,
        input: "hidden.example".into(),
    });
    shell.handle(Command::Open);
    let active = active_id(&screen);
    shell.handle(Command::Navigate {
        id: active,
        input: "active.example".into(),
    });
    store.events.lock().unwrap().clear();
    let creates_before = engine
        .calls()
        .iter()
        .filter(|call| call.starts_with(&format!("create {hidden} ")))
        .count();

    shell.handle(Command::Engine(EngineEvent::Crashed { id: hidden }));
    assert!(!engine
        .calls()
        .iter()
        .any(|call| call == &format!("close {hidden}")));
    assert_eq!(
        engine
            .calls()
            .iter()
            .filter(|call| call.starts_with(&format!("create {hidden} ")))
            .count(),
        creates_before
    );
    assert!(store.events.lock().unwrap().is_empty());
    assert!(!shell.items.tab(hidden).unwrap().has_view());
}

#[test]
fn profile_process_exit_recreates_focused_view_only_once() {
    let (mut shell, engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    let hidden = active_id(&screen);
    navigate_and_commit(&mut shell, hidden, "example.com");
    shell.handle(Command::Open);
    let visible = active_id(&screen);
    navigate_and_commit(&mut shell, visible, "github.com");
    let profile = shell.profile_of_item(visible).unwrap();

    shell.handle(Command::Engine(EngineEvent::ProfileProcessExited {
        profile,
        ids: vec![hidden, visible],
    }));
    assert!(!shell.items.tab(hidden).unwrap().has_view());
    assert!(shell.items.tab(visible).unwrap().has_view());
    assert_eq!(
        engine
            .calls()
            .iter()
            .filter(|call| call.starts_with(&format!("create {visible} ")))
            .count(),
        2
    );

    // A second failure in the burst does not enter a rebuild loop.
    shell.handle(Command::Engine(EngineEvent::ProfileProcessExited {
        profile,
        ids: vec![visible],
    }));
    assert!(!shell.items.tab(visible).unwrap().has_view());
    assert_eq!(
        engine
            .calls()
            .iter()
            .filter(|call| call.starts_with(&format!("create {visible} ")))
            .count(),
        2
    );
}

#[test]
fn profile_process_exit_defers_hidden_recovery_until_activation() {
    let (mut shell, engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    let hidden = active_id(&screen);
    navigate_and_commit(&mut shell, hidden, "hidden.example");
    shell.handle(Command::Open);
    let active = active_id(&screen);
    navigate_and_commit(&mut shell, active, "active.example");
    let profile = shell.profile_of_item(active).unwrap();
    let before = engine
        .calls()
        .iter()
        .filter(|call| call.starts_with("create "))
        .count();

    shell.handle(Command::Engine(EngineEvent::ProfileProcessExited {
        profile,
        ids: vec![hidden, active],
    }));
    let after_failure = engine
        .calls()
        .iter()
        .filter(|call| call.starts_with("create "))
        .count();
    assert_eq!(after_failure, before + 1, "only the focused tab recovers");
    assert!(!shell.items.tab(hidden).unwrap().has_view());

    shell.handle(Command::Activate(hidden));
    assert!(shell.items.tab(hidden).unwrap().has_view());
    let after_activation = engine
        .calls()
        .iter()
        .filter(|call| call.starts_with("create "))
        .count();
    assert_eq!(after_activation, after_failure + 1);
}

#[test]
fn profile_process_exit_recreates_every_visible_split_leaf() {
    let (mut shell, engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    let first = active_id(&screen);
    navigate_and_commit(&mut shell, first, "first.example");
    shell.handle(Command::Open);
    let second = active_id(&screen);
    navigate_and_commit(&mut shell, second, "second.example");
    shell.handle(Command::SplitWith {
        other: first,
        axis: Axis::Row,
    });
    let profile = shell.profile_of_item(second).unwrap();

    shell.handle(Command::Engine(EngineEvent::ProfileProcessExited {
        profile,
        ids: vec![first, second],
    }));

    assert!(shell.items.tab(first).unwrap().has_view());
    assert!(shell.items.tab(second).unwrap().has_view());
    assert_eq!(engine.last_layout().len(), 2);
    for id in [first, second] {
        assert_eq!(
            engine
                .calls()
                .iter()
                .filter(|call| call.starts_with(&format!("create {id} ")))
                .count(),
            2
        );
    }
}

#[test]
fn multi_leaf_process_recovery_accounts_for_the_whole_optimistic_batch() {
    let (mut shell, engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    let first = active_id(&screen);
    navigate_and_commit(&mut shell, first, "recovery0.example");
    let mut ids = vec![first];
    for index in 1..MAX_VISIBLE_PANES {
        shell.handle(Command::Open);
        let id = active_id(&screen);
        navigate_and_commit(&mut shell, id, &format!("recovery{index}.example"));
        ids.push(id);
    }
    let tree = ids[1..]
        .iter()
        .fold(Pane::Leaf(first), |tree, id| Pane::Branch {
            axis: Axis::Row,
            ratio: 0.5,
            a: Box::new(tree),
            b: Box::new(Pane::Leaf(*id)),
        });
    shell.windows.focused_mut().unwrap().splits = Some(tree);
    let profile = shell.profile_of_item(first).unwrap();
    let creates_before = engine
        .calls()
        .iter()
        .filter(|call| call.starts_with("create "))
        .count();

    shell.handle(Command::Engine(EngineEvent::ProfileProcessExited {
        profile,
        ids: ids.clone(),
    }));

    assert_eq!(shell.items.view_ids().len(), MAX_VISIBLE_PANES);
    assert!(shell.items.view_ids().len() <= LIVE_VIEW_ABSOLUTE_LIMIT);
    assert!(ids
        .iter()
        .all(|id| shell.items.tab(*id).is_some_and(TabState::has_view)));
    assert_eq!(engine.last_layout().len(), MAX_VISIBLE_PANES);
    assert_eq!(
        engine
            .calls()
            .iter()
            .filter(|call| call.starts_with("create "))
            .count(),
        creates_before + MAX_VISIBLE_PANES,
        "later optimistic leaves must not make the first recovery create appear over budget"
    );
}

#[test]
fn repeated_crash_leaf_temporarily_collapses_to_a_live_split_sibling() {
    let (mut shell, engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    let first = active_id(&screen);
    navigate_and_commit(&mut shell, first, "first.example");
    shell.handle(Command::Open);
    let second = active_id(&screen);
    navigate_and_commit(&mut shell, second, "second.example");
    shell.handle(Command::SplitWith {
        other: first,
        axis: Axis::Row,
    });
    let profile = shell.profile_of_item(second).unwrap();
    shell
        .crash
        .crashes
        .insert(second, std::time::Instant::now());

    shell.handle(Command::Engine(EngineEvent::ProfileProcessExited {
        profile,
        ids: vec![first, second],
    }));

    assert_eq!(shell.windows.focused().unwrap().active, Some(first));
    assert!(shell.items.tab(first).unwrap().has_view());
    assert!(!shell.items.tab(second).unwrap().has_view());
    assert_eq!(engine.last_layout(), vec![first.to_string()]);
    assert!(shell.windows.focused().unwrap().splits.is_some());
}

#[test]
fn hidden_idle_views_go_dormant_and_wake_on_show() {
    let (mut shell, engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    let first = active_id(&screen);
    navigate_and_commit(&mut shell, first, "example.com");
    shell.handle(Command::Open);
    let second = active_id(&screen);
    navigate_and_commit(&mut shell, second, "github.com");

    // nothing is idle yet: no dormancy requested
    assert!(!engine.calls().iter().any(|c| c.starts_with("dormant")));

    // once idle, the hidden view suspends; the shown one does not
    shell.residency.dormant_min = std::time::Duration::ZERO;
    shell.handle(Command::Tick);
    assert!(engine
        .calls()
        .iter()
        .any(|c| c == &format!("dormant {first}")));

    // refocusing moves dormancy to the other tab
    shell.handle(Command::Activate(first));
    assert!(engine
        .calls()
        .iter()
        .any(|c| c == &format!("dormant {second}")));
    let calls = engine.calls();
    let layout = calls
        .iter()
        .rposition(|c| c.starts_with("layout@"))
        .unwrap();
    let dormant = calls
        .iter()
        .rposition(|c| c == &format!("dormant {second}"))
        .unwrap();
    assert!(layout < dormant, "a view must be hidden before suspension");
}

#[test]
fn idle_hidden_pages_beyond_the_warm_set_sleep_only_after_acknowledgement() {
    let (mut shell, _engine, screen) = setup();
    shell.residency.warm_view_limit = 2;
    shell.residency.live_view_pressure_limit = LIVE_VIEW_PRESSURE_LIMIT;
    shell.residency.discard_idle_min = std::time::Duration::ZERO;
    shell.handle(Command::Bootstrap);
    let first = active_id(&screen);
    navigate_and_commit(&mut shell, first, "budget0.example");
    for n in 1..6 {
        shell.handle(Command::Open);
        let id = active_id(&screen);
        navigate_and_commit(&mut shell, id, &format!("budget{n}.example"));
    }
    assert_eq!(shell.items.view_ids().len(), 6);

    // The foreground page plus the warm set stay resident; every older idle
    // page sleeps, though the live count is below the pressure watermark.
    while shell.items.view_ids().len() > shell.residency.warm_view_limit + 1 {
        let (id, probe) = first_probing_discard(&shell);
        acknowledge_safe_discard(&mut shell, id, probe);
    }
    assert!(shell.residency.discard_probes.is_empty());

    assert_eq!(shell.items.view_ids().len(), 3);
    assert_eq!(
        shell
            .items
            .view_ids()
            .into_iter()
            .filter(|id| shell.discard_protected_leaves().contains(id))
            .count(),
        1
    );
}

#[test]
fn unsafe_page_is_exempt_from_budget_and_reprobe_is_cooled_down() {
    let (mut shell, engine, screen) = setup();
    shell.residency.warm_view_limit = 1;
    shell.residency.live_view_pressure_limit = 1;
    shell.residency.discard_idle_min = std::time::Duration::ZERO;
    shell.handle(Command::Bootstrap);
    let protected = active_id(&screen);
    navigate_and_commit(&mut shell, protected, "dirty.example");
    shell.handle(Command::Open);
    let active = active_id(&screen);
    navigate_and_commit(&mut shell, active, "active.example");
    let (id, probe) = first_probing_discard(&shell);
    assert_eq!(id, protected);

    shell.handle(Command::Engine(EngineEvent::DiscardSafety {
        id,
        probe,
        can_discard: false,
    }));
    shell.handle(Command::Tick);

    assert_eq!(shell.items.view_ids().len(), 2);
    assert!(shell
        .residency
        .discard_protected_until
        .contains_key(&protected));
    assert_eq!(
        engine
            .calls()
            .iter()
            .filter(|call| call.starts_with(&format!("probe-discard {protected} ")))
            .count(),
        1
    );
}

#[test]
fn missing_probe_callback_times_out_fail_closed_with_bounded_state() {
    let (mut shell, _engine, screen) = setup();
    shell.residency.warm_view_limit = 1;
    shell.residency.live_view_pressure_limit = 1;
    shell.residency.discard_idle_min = std::time::Duration::ZERO;
    shell.handle(Command::Bootstrap);
    let candidate = active_id(&screen);
    navigate_and_commit(&mut shell, candidate, "timeout.example");
    shell.handle(Command::Open);
    let active = active_id(&screen);
    navigate_and_commit(&mut shell, active, "active.example");
    let (id, probe) = first_probing_discard(&shell);

    shell.handle(Command::DiscardProbeTimeout { id, probe });

    assert!(shell.items.tab(candidate).unwrap().has_view());
    assert!(shell.residency.discard_probes.is_empty());
    assert!(shell
        .residency
        .discard_protected_until
        .contains_key(&candidate));
}

#[test]
fn common_tab_counts_probe_before_and_never_exceed_the_resident_view_ceiling() {
    for count in [1_usize, 10, 24, 25, 32, 33, 50, 100] {
        let (mut shell, engine, screen) = setup();
        shell.handle(Command::Bootstrap);
        let first = active_id(&screen);
        navigate_and_commit(&mut shell, first, "count0.example");
        for index in 1..count {
            shell.handle(Command::Open);
            let id = active_id(&screen);
            shell.handle(Command::Navigate {
                id,
                input: format!("count{index}.example"),
            });
            if shell.items.tab(id).is_some_and(TabState::has_view) {
                shell.handle(Command::Engine(EngineEvent::UrlChanged {
                    id,
                    url: format!("https://count{index}.example/"),
                }));
            }
        }
        assert_eq!(last(&screen).tabs.len(), count, "tabs remain in the model");
        assert_eq!(
            shell.items.view_ids().len(),
            count.min(LIVE_VIEW_ABSOLUTE_LIMIT)
        );
        assert_eq!(
            engine
                .calls()
                .iter()
                .filter(|call| call.starts_with("create "))
                .count(),
            count.min(LIVE_VIEW_ABSOLUTE_LIMIT),
            "native creates must stop at the logical ceiling"
        );
        let expected_probes = if count.min(LIVE_VIEW_ABSOLUTE_LIMIT) > LIVE_VIEW_PRESSURE_LIMIT {
            MAX_CONCURRENT_DISCARD_PROBES
        } else {
            0
        };
        assert_eq!(
            shell.residency.discard_probes.len(),
            expected_probes,
            "pressure probing must start before absolute admission is exhausted"
        );
    }
}

#[test]
fn resident_ceiling_defers_foreground_create_and_expires_without_changing_saved_title() {
    let (mut shell, _engine, screen, operations) =
        setup_with_operation_log(Arc::new(FakeStore::default()));
    shell.handle(Command::Bootstrap);
    let first = active_id(&screen);
    navigate_and_commit(&mut shell, first, "ceiling0.example");
    for index in 1..LIVE_VIEW_ABSOLUTE_LIMIT {
        shell.handle(Command::Open);
        let id = active_id(&screen);
        navigate_and_commit(&mut shell, id, &format!("ceiling{index}.example"));
    }
    shell.handle(Command::Open);
    let excess = active_id(&screen);
    let original_title = shell.items.tab(excess).unwrap().title.clone();
    shell.handle(Command::Operation {
        operation_id: "resident-limit".into(),
        command: Box::new(Command::Navigate {
            id: excess,
            input: "excess.example".into(),
        }),
    });

    assert_eq!(last(&screen).tabs.len(), LIVE_VIEW_ABSOLUTE_LIMIT + 1);
    assert!(!shell.items.tab(excess).unwrap().has_view());
    assert_eq!(shell.items.view_ids().len(), LIVE_VIEW_ABSOLUTE_LIMIT);
    assert_eq!(
        operations.lock().unwrap().last().unwrap().outcome,
        OperationOutcome::Deferred
    );
    assert_eq!(
        operations.lock().unwrap().last().unwrap().reason,
        OperationReason::NativeWorkPending
    );
    assert_eq!(shell.items.tab(excess).unwrap().title, original_title);
    assert!(matches!(
        shell.capacity_presentation(excess),
        Some(zephium_ipc::TabAvailability::WaitingForCapacity { .. })
    ));
    shell
        .residency
        .capacity_requests
        .front_mut()
        .unwrap()
        .deadline = std::time::Instant::now();
    shell.handle(Command::ViewCapacityRetry(excess));
    assert!(shell.residency.capacity_requests.is_empty());
    assert!(matches!(
        shell.capacity_presentation(excess),
        Some(zephium_ipc::TabAvailability::BlockedByCapacity { .. })
    ));
    assert_eq!(shell.items.tab(excess).unwrap().title, original_title);
}

#[test]
fn foreground_capacity_waits_for_physical_close_and_replays_only_latest_intent() {
    let (mut shell, engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    let first = active_id(&screen);
    navigate_and_commit(&mut shell, first, "resident0.example");
    for index in 1..LIVE_VIEW_ABSOLUTE_LIMIT {
        shell.handle(Command::Open);
        let id = active_id(&screen);
        navigate_and_commit(&mut shell, id, &format!("resident{index}.example"));
    }
    shell.handle(Command::Open);
    let excess = active_id(&screen);
    for input in ["old-intent.example", "latest-intent.example"] {
        shell.handle(Command::Navigate {
            id: excess,
            input: input.into(),
        });
    }
    assert_eq!(shell.residency.capacity_requests.len(), 1);
    let (candidate, probe) = first_probing_discard(&shell);
    shell.handle(Command::Engine(EngineEvent::DiscardSafety {
        id: candidate,
        probe,
        can_discard: true,
    }));
    assert!(
        !shell.items.tab(excess).unwrap().has_view(),
        "safe probe alone cannot release a slot"
    );
    shell.handle(Command::Engine(EngineEvent::ViewDiscarded {
        id: candidate,
        profile: shell.profile_of_item(candidate).unwrap(),
        probe,
    }));
    assert!(shell.items.tab(excess).unwrap().has_view());
    assert!(shell.residency.capacity_requests.is_empty());
    assert_eq!(shell.items.view_ids().len(), LIVE_VIEW_ABSOLUTE_LIMIT);
    let creates: Vec<_> = engine
        .calls()
        .into_iter()
        .filter(|call| call.starts_with(&format!("create {excess} ")))
        .collect();
    assert_eq!(creates.len(), 1);
    assert!(creates[0].contains("https://latest-intent.example/"));
}

#[test]
fn disabled_sleep_is_honored_until_critical_memory_pressure() {
    use zephium_core::ports::engine::MemoryPressure;
    let (mut shell, _engine, screen) = setup();
    shell.handle(Command::SetAppSetting {
        key: "performance.sleep".into(),
        value: "false".into(),
    });
    shell.handle(Command::Bootstrap);
    let first = active_id(&screen);
    navigate_and_commit(&mut shell, first, "resident0.example");
    for index in 1..LIVE_VIEW_ABSOLUTE_LIMIT {
        shell.handle(Command::Open);
        let id = active_id(&screen);
        navigate_and_commit(&mut shell, id, &format!("resident{index}.example"));
    }
    shell.handle(Command::Open);
    let excess = active_id(&screen);
    shell.handle(Command::Navigate {
        id: excess,
        input: "waiting.example".into(),
    });
    assert_eq!(shell.residency.capacity_requests.len(), 1);
    assert!(
        shell.residency.discard_probes.is_empty(),
        "a waiting page does not override the user's choice to keep tabs awake"
    );
    shell.handle(Command::SetMemoryPressure(MemoryPressure::Critical));
    // The least recently used pages are probed first, in a bounded batch.
    let Some(PendingDiscardProbe::Probing { probe, .. }) =
        shell.residency.discard_probes.get(&first).cloned()
    else {
        panic!("critical pressure must probe the least recently used page");
    };
    acknowledge_safe_discard(&mut shell, first, probe);
    assert!(shell.items.tab(excess).unwrap().has_view());
    assert!(shell.residency.capacity_requests.is_empty());
}

#[test]
fn heavy_hidden_pages_sleep_early_but_the_last_hidden_page_stays_instant() {
    let (mut shell, _engine, screen) = setup();
    shell.residency.discard_idle_min = std::time::Duration::from_secs(60 * 60);
    shell.residency.heavy_page = (100, std::time::Duration::ZERO);
    shell.handle(Command::Bootstrap);
    let heavy = active_id(&screen);
    navigate_and_commit(&mut shell, heavy, "heavy.example");
    let mut ids = vec![heavy];
    for host in ["light.example", "recent-heavy.example", "active.example"] {
        shell.handle(Command::Open);
        let id = active_id(&screen);
        navigate_and_commit(&mut shell, id, host);
        ids.push(id);
    }
    let (light, recent) = (ids[1], ids[2]);
    let profile = shell.profile_of_item(heavy).unwrap();
    for (id, bytes) in [(heavy, 200), (light, 50), (recent, 200)] {
        shell.handle(Command::Engine(EngineEvent::PageMemory {
            id,
            profile,
            bytes,
        }));
    }
    shell.handle(Command::Tick);
    assert!(
        shell.residency.discard_probes.contains_key(&heavy),
        "a heavy page sleeps early even inside the warm set"
    );
    assert!(!shell.residency.discard_probes.contains_key(&light));
    assert!(
        !shell.residency.discard_probes.contains_key(&recent),
        "the most recent hidden page stays for instant switching"
    );
}

#[test]
fn memory_warning_uses_a_short_grace_instead_of_discarding_at_once() {
    use zephium_core::ports::engine::MemoryPressure;
    let (mut shell, _engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    let first = active_id(&screen);
    navigate_and_commit(&mut shell, first, "warning0.example");
    for n in 1..6 {
        shell.handle(Command::Open);
        let id = active_id(&screen);
        navigate_and_commit(&mut shell, id, &format!("warning{n}.example"));
    }
    shell.handle(Command::SetMemoryPressure(MemoryPressure::Warning));
    assert!(
        shell.residency.discard_probes.is_empty(),
        "pages left seconds ago stay resident under a warning"
    );
    let Some(stale) = std::time::Instant::now().checked_sub(std::time::Duration::from_secs(3 * 60))
    else {
        return;
    };
    for times in [
        &mut shell.residency.last_focus,
        &mut shell.residency.resident_since,
        &mut shell.residency.inactive_since,
    ] {
        if let Some(time) = times.get_mut(&first) {
            *time = stale;
        }
    }
    shell.handle(Command::Tick);
    let (candidate, _) = first_probing_discard(&shell);
    assert_eq!(candidate, first);
    assert_eq!(shell.residency.discard_probes.len(), 1);
}

#[test]
fn performance_preferences_load_durably_and_pressure_restores_preferred_targets() {
    use zephium_core::ports::engine::MemoryPressure;
    let store = Arc::new(FakeStore::default());
    assert!(store.set_app_setting("performance.memory".into(), "keep-ready".into()));
    assert!(store.set_app_setting("performance.after".into(), "30".into()));
    let (mut shell, _, _) = setup_with(store.clone());
    assert_eq!(shell.residency.warm_view_limit, 10);
    assert_eq!(
        shell.residency.discard_idle_min,
        std::time::Duration::from_secs(30 * 60)
    );
    // Lossless suspension never waits longer than its own short grace.
    assert_eq!(
        shell.residency.dormant_min,
        std::time::Duration::from_secs(5 * 60)
    );
    shell.handle(Command::SetMemoryPressure(MemoryPressure::Critical));
    assert_eq!(shell.residency.warm_view_limit, 0);
    shell.handle(Command::SetAppSetting {
        key: "performance.memory".into(),
        value: "save-memory".into(),
    });
    assert_eq!(shell.residency.warm_view_limit, 0);
    shell.handle(Command::SetMemoryPressure(MemoryPressure::Normal));
    assert_eq!(shell.residency.warm_view_limit, 2);
    let (restarted, _, _) = setup_with(store);
    assert_eq!(restarted.residency.warm_view_limit, 2);
}

#[test]
fn awake_site_exceptions_cover_subdomains_and_disabling_sleep_cancels_probes() {
    let (mut shell, engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    let protected = active_id(&screen);
    navigate_and_commit(&mut shell, protected, "mail.kept.example");
    shell.handle(Command::SetAppSetting {
        key: "performance.exceptions".into(),
        value: "kept.example".into(),
    });
    shell.handle(Command::Open);
    let candidate = active_id(&screen);
    navigate_and_commit(&mut shell, candidate, "notkept.example");
    shell.handle(Command::Open);
    let active = active_id(&screen);
    navigate_and_commit(&mut shell, active, "active.example");
    shell.residency.warm_view_limit = 1;
    shell.residency.live_view_pressure_limit = 1;
    shell.residency.dormant_min = std::time::Duration::ZERO;
    shell.handle(Command::Tick);
    let (id, probe) = first_probing_discard(&shell);
    assert_eq!(id, candidate);
    assert!(!engine
        .calls()
        .iter()
        .any(|call| call.starts_with(&format!("probe-discard {protected} "))));
    shell.handle(Command::SetAppSetting {
        key: "performance.sleep".into(),
        value: "false".into(),
    });
    assert!(shell.residency.discard_probes.is_empty());
    assert!(shell.residency.dormant_sent.is_empty());
    shell.handle(Command::Engine(EngineEvent::DiscardSafety {
        id,
        probe,
        can_discard: true,
    }));
    assert!(shell.items.tab(candidate).unwrap().has_view());
}

#[test]
fn optimistic_multi_leaf_batch_admits_available_slots_in_effect_order() {
    let (mut shell, engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    let first = active_id(&screen);
    navigate_and_commit(&mut shell, first, "baseline0.example");
    for index in 1..(LIVE_VIEW_ABSOLUTE_LIMIT - 1) {
        shell.handle(Command::Open);
        let id = active_id(&screen);
        navigate_and_commit(&mut shell, id, &format!("baseline{index}.example"));
    }
    assert_eq!(shell.items.view_ids().len(), LIVE_VIEW_ABSOLUTE_LIMIT - 1);

    let space = shell.windows.focused().unwrap().space;
    let mut effects = Vec::new();
    let mut leaves = Vec::new();
    for index in 0..MAX_VISIBLE_PANES {
        let id = ItemId::from(10_000 + index as u128);
        assert!(shell.items.insert_tab(
            id,
            Placement::Space {
                space,
                section: SpaceSection::Today,
            },
        ));
        effects.extend(shell.items.navigate(id, &format!("batch{index}.example")));
        leaves.push(id);
    }
    assert_eq!(
        shell.items.view_ids().len(),
        LIVE_VIEW_ABSOLUTE_LIMIT - 1 + MAX_VISIBLE_PANES,
        "all batch effects have optimistically set their logical bits"
    );

    let native = shell.apply(effects);

    assert!(native.scheduled);
    assert!(native.rejected);
    assert!(shell.items.tab(leaves[0]).unwrap().has_view());
    assert!(leaves[1..]
        .iter()
        .all(|id| !shell.items.tab(*id).unwrap().has_view()));
    assert_eq!(shell.items.view_ids().len(), LIVE_VIEW_ABSOLUTE_LIMIT);
    assert_eq!(
        engine
            .calls()
            .iter()
            .filter(|call| call.starts_with("create "))
            .count(),
        LIVE_VIEW_ABSOLUTE_LIMIT,
        "the first batch leaf uses the one free slot; later leaves are rolled back"
    );
}

#[test]
fn stale_or_navigation_cancelled_discard_probe_cannot_close_a_view() {
    let (mut shell, engine, screen) = setup();
    shell.residency.warm_view_limit = 1;
    shell.residency.live_view_pressure_limit = 1;
    shell.residency.discard_idle_min = std::time::Duration::ZERO;
    shell.handle(Command::Bootstrap);
    let candidate = active_id(&screen);
    navigate_and_commit(&mut shell, candidate, "old.example");
    shell.handle(Command::Open);
    let active = active_id(&screen);
    navigate_and_commit(&mut shell, active, "active.example");
    let (_, stale) = first_probing_discard(&shell);

    navigate_and_commit(&mut shell, candidate, "new.example");
    shell.handle(Command::Engine(EngineEvent::DiscardSafety {
        id: candidate,
        probe: stale,
        can_discard: true,
    }));
    shell.handle(Command::Engine(EngineEvent::ViewDiscarded {
        id: candidate,
        profile: shell.profile_of_item(candidate).unwrap(),
        probe: stale,
    }));

    assert!(shell.items.tab(candidate).unwrap().has_view());
    assert_eq!(
        shell
            .items
            .tab(candidate)
            .unwrap()
            .url
            .as_ref()
            .unwrap()
            .host_str(),
        Some("new.example")
    );
    assert!(!engine
        .calls()
        .iter()
        .any(|call| call.starts_with(&format!("discard {candidate} "))));
}

#[test]
fn every_currently_visible_split_leaf_is_outside_the_discard_candidate_set() {
    let (mut shell, engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    let first = active_id(&screen);
    navigate_and_commit(&mut shell, first, "left.example");
    shell.handle(Command::Open);
    let second = active_id(&screen);
    navigate_and_commit(&mut shell, second, "right.example");
    shell.handle(Command::SplitWith {
        other: first,
        axis: Axis::Row,
    });
    shell.residency.warm_view_limit = 0;
    shell.residency.live_view_pressure_limit = 0;
    shell.residency.discard_idle_min = std::time::Duration::ZERO;
    shell.handle(Command::Tick);

    assert!(shell.items.tab(first).unwrap().has_view());
    assert!(shell.items.tab(second).unwrap().has_view());
    assert!(!engine
        .calls()
        .iter()
        .any(|call| call.starts_with("probe-discard ")));
}

#[test]
fn acknowledged_discard_preserves_url_and_activation_recreates_lazily() {
    let (mut shell, engine, screen) = setup();
    shell.residency.warm_view_limit = 1;
    shell.residency.live_view_pressure_limit = 1;
    shell.residency.discard_idle_min = std::time::Duration::ZERO;
    shell.handle(Command::Bootstrap);
    let sleeping = active_id(&screen);
    navigate_and_commit(&mut shell, sleeping, "sleeping.example/path");
    shell.handle(Command::Open);
    let active = active_id(&screen);
    navigate_and_commit(&mut shell, active, "active.example");
    let (_, probe) = first_probing_discard(&shell);
    acknowledge_safe_discard(&mut shell, sleeping, probe);

    assert!(!shell.items.tab(sleeping).unwrap().has_view());
    assert_eq!(
        shell
            .items
            .tab(sleeping)
            .unwrap()
            .url
            .as_ref()
            .unwrap()
            .as_str(),
        "https://sleeping.example/path"
    );
    shell.handle(Command::Activate(sleeping));
    assert!(shell.items.tab(sleeping).unwrap().has_view());
    assert_eq!(
        engine
            .calls()
            .iter()
            .filter(|call| call.starts_with(&format!("create {sleeping} ")))
            .count(),
        2
    );
}

#[test]
fn minimized_window_hides_before_dormancy_and_wakes_focused_view() {
    let (mut shell, engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    let first = active_id(&screen);
    navigate_and_commit(&mut shell, first, "first.example");
    shell.handle(Command::Open);
    let second = active_id(&screen);
    navigate_and_commit(&mut shell, second, "second.example");
    shell.residency.dormant_min = std::time::Duration::ZERO;

    shell.handle(Command::SetWindowVisible(false));
    assert!(engine.last_layout().is_empty());
    let calls = engine.calls();
    let hidden = calls
        .iter()
        .rposition(|call| call.starts_with("layout@") && call.ends_with(' '))
        .unwrap();
    let dormant = calls
        .iter()
        .rposition(|call| call.starts_with("dormant "))
        .unwrap();
    assert!(hidden < dormant, "controllers must hide before suspension");
    let latest_dormant = &calls[dormant];
    assert!(latest_dormant.contains(&first.to_string()));
    assert!(latest_dormant.contains(&second.to_string()));

    shell.handle(Command::SetWindowVisible(true));
    assert_eq!(engine.last_layout(), vec![second.to_string()]);
    assert_eq!(
        engine
            .calls()
            .iter()
            .rev()
            .find(|call| call.starts_with("dormant "))
            .unwrap(),
        &format!("dormant {first}")
    );
}

#[test]
fn split_created_views_are_not_unsafely_discarded() {
    let (mut shell, _engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    let first = active_id(&screen);
    shell.handle(Command::Navigate {
        id: first,
        input: "left.com".into(),
    });
    shell.handle(Command::Open);
    let second = active_id(&screen);
    shell.handle(Command::Navigate {
        id: second,
        input: "right.com".into(),
    });
    shell.handle(Command::Activate(first));
    // splitting creates second's view without focusing it
    shell.handle(Command::SplitWith {
        other: second,
        axis: Axis::Row,
    });
    shell.handle(Command::Unsplit);
    // Even under tab pressure the never-focused split view may contain
    // unsaved renderer state, so it remains resident.
    for n in 0..13 {
        shell.handle(Command::Open);
        let id = active_id(&screen);
        shell.handle(Command::Navigate {
            id,
            input: format!("warm{n}.com"),
        });
    }
    assert!(shell.items.tab(second).unwrap().has_view());
}
