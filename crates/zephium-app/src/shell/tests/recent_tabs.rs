use super::*;
use zephium_ipc::TabSwitcherAction;

fn loaded(shell: &mut Shell, screen: &Screen, name: &str) -> ItemId {
    shell.handle(Command::Open);
    let id = active_id(screen);
    shell.handle(Command::Navigate {
        id,
        input: format!("https://{name}.test/"),
    });
    id
}

#[test]
fn recent_order_tracks_activation_close_and_keeps_sleeping_tabs() {
    let (mut shell, _, screen) = setup();
    shell.handle(Command::Bootstrap);
    let first = loaded(&mut shell, &screen, "one");
    let second = loaded(&mut shell, &screen, "two");
    let third = loaded(&mut shell, &screen, "three");
    assert_eq!(shell.recent_tab_order(), vec![third, second, first]);
    shell.handle(Command::Activate(first));
    assert_eq!(shell.recent_tab_order(), vec![first, third, second]);
    shell.handle(Command::Close(third));
    assert_eq!(shell.recent_tab_order(), vec![first, second]);
    shell.residency.dormant_sent.push(second);
    assert_eq!(shell.recent_tab_order(), vec![first, second]);
    shell.handle(Command::BeginTabSwitcher {
        session: 1,
        reverse: false,
        keyboard: true,
    });
    shell.handle(Command::TabSwitcher {
        session: 1,
        action: TabSwitcherAction::Commit,
    });
    assert_eq!(active_id(&screen), second);
}

#[test]
fn quick_taps_toggle_and_cancellation_never_activates_selection() {
    let (mut shell, _, screen) = setup();
    shell.handle(Command::Bootstrap);
    let first = loaded(&mut shell, &screen, "one");
    let second = loaded(&mut shell, &screen, "two");
    shell.handle(Command::BeginTabSwitcher {
        session: 1,
        reverse: false,
        keyboard: true,
    });
    assert_eq!(active_id(&screen), second);
    shell.handle(Command::TabSwitcher {
        session: 1,
        action: TabSwitcherAction::Commit,
    });
    assert_eq!(active_id(&screen), first);
    shell.handle(Command::BeginTabSwitcher {
        session: 2,
        reverse: false,
        keyboard: true,
    });
    shell.handle(Command::TabSwitcher {
        session: 1,
        action: TabSwitcherAction::Commit,
    });
    shell.handle(Command::RevealTabSwitcher { session: 2 });
    shell.handle(Command::TabSwitcher {
        session: 2,
        action: TabSwitcherAction::Cancel,
    });
    assert_eq!(active_id(&screen), first);
    shell.handle(Command::BeginTabSwitcher {
        session: 3,
        reverse: false,
        keyboard: true,
    });
    shell.handle(Command::TabSwitcher {
        session: 3,
        action: TabSwitcherAction::Commit,
    });
    assert_eq!(active_id(&screen), second);
}

#[test]
fn reverse_close_and_malformed_selection_preserve_the_frozen_order() {
    let (mut shell, _, screen) = setup();
    shell.handle(Command::Bootstrap);
    let first = loaded(&mut shell, &screen, "one");
    let second = loaded(&mut shell, &screen, "two");
    let third = loaded(&mut shell, &screen, "three");
    shell.handle(Command::BeginTabSwitcher {
        session: 4,
        reverse: true,
        keyboard: true,
    });
    shell.handle(Command::TabSwitcher {
        session: 4,
        action: TabSwitcherAction::Select { index: u32::MAX },
    });
    shell.handle(Command::TabSwitcher {
        session: 4,
        action: TabSwitcherAction::Close,
    });
    assert!(shell.items.tab(first).is_none());
    assert_eq!(active_id(&screen), third);
    shell.handle(Command::TabSwitcher {
        session: 4,
        action: TabSwitcherAction::Commit,
    });
    assert_eq!(active_id(&screen), second);
}

#[test]
fn new_tab_leaves_the_switcher_for_a_new_tab_in_its_window() {
    let (mut shell, _, screen) = setup();
    shell.handle(Command::Bootstrap);
    let first = loaded(&mut shell, &screen, "one");
    let second = loaded(&mut shell, &screen, "two");
    shell.handle(Command::BeginTabSwitcher {
        session: 5,
        reverse: false,
        keyboard: false,
    });
    shell.handle(Command::TabSwitcher {
        session: 5,
        action: TabSwitcherAction::NewTab,
    });
    let opened = active_id(&screen);
    assert!(opened != first && opened != second);
    shell.handle(Command::TabSwitcher {
        session: 5,
        action: TabSwitcherAction::Commit,
    });
    assert_eq!(active_id(&screen), opened);
}

#[test]
fn changing_space_cancels_selection_and_keeps_each_spaces_history() {
    let (mut shell, _, screen) = setup();
    shell.handle(Command::Bootstrap);
    let first_space = shell.windows.focused().unwrap().space;
    let first = loaded(&mut shell, &screen, "one");
    let second = loaded(&mut shell, &screen, "two");
    shell.handle(Command::BeginTabSwitcher {
        session: 9,
        reverse: false,
        keyboard: true,
    });
    shell.handle(Command::Space(Box::new(crate::SpaceCommand::Create {
        name: "Other".into(),
        look: Default::default(),
    })));
    let other = loaded(&mut shell, &screen, "other");
    shell.handle(Command::TabSwitcher {
        session: 9,
        action: TabSwitcherAction::Commit,
    });
    assert_eq!(active_id(&screen), other);
    assert_eq!(shell.recent_tab_order(), vec![other]);
    shell.handle(Command::Space(Box::new(crate::SpaceCommand::Switch(
        first_space,
    ))));
    assert_eq!(shell.recent_tab_order(), vec![second, first]);
}

#[test]
fn preview_reads_refuse_unknown_tabs_revisions_and_other_windows() {
    let (mut shell, _, screen) = setup();
    shell.handle(Command::Bootstrap);
    let tab = loaded(&mut shell, &screen, "one");
    let window = shell.windows.focused().unwrap().id;
    assert!(shell.read_tab_preview(window, tab, 1).is_none());
    assert!(shell.read_tab_preview(window + 1, tab, 1).is_none());
    assert!(shell
        .read_tab_preview(window, ItemId::from(999), 1)
        .is_none());
}

#[test]
fn essentials_and_pinned_tabs_participate_in_recency_without_sidebar_order() {
    let (mut shell, _, screen) = setup();
    shell.handle(Command::Bootstrap);
    let essential = loaded(&mut shell, &screen, "essential");
    let pinned = loaded(&mut shell, &screen, "pinned");
    let today = loaded(&mut shell, &screen, "today");
    let space = shell.windows.focused().unwrap().space;
    assert!(shell
        .items
        .move_tab_to_root(essential, Placement::Favorites { space }, None));
    assert!(shell.items.move_tab_to_root(
        pinned,
        Placement::Space {
            space,
            section: SpaceSection::Pinned
        },
        None
    ));
    assert_eq!(shell.recent_tab_order(), vec![today, pinned, essential]);
    shell.handle(Command::Activate(essential));
    assert_eq!(shell.recent_tab_order(), vec![essential, today, pinned]);
}

#[test]
#[cfg(target_os = "windows")]
fn a_tab_is_captured_while_in_front_never_after_the_switch_hides_it() {
    let (mut shell, engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    let first = loaded(&mut shell, &screen, "one");
    commit_url(&mut shell, first, "https://one.test/");
    let second = loaded(&mut shell, &screen, "two");
    commit_url(&mut shell, second, "https://two.test/");
    shell.tab_previews = Default::default();
    let queue = CommandQueue::new();
    shell.self_queue = Some(queue.clone());
    let window = shell.windows.focused().unwrap().id;
    engine.calls.lock().unwrap().clear();
    let captures = |tab: ItemId| {
        engine
            .calls()
            .iter()
            .filter(|call| **call == format!("capture-preview {tab}"))
            .count()
    };

    // A tab behind the one in front finishing its load is not on screen.
    shell.handle(Command::Engine(EngineEvent::LoadingChanged {
        id: first,
        loading: false,
    }));
    while let Some(command) = queue.try_recv() {
        shell.handle(command);
    }
    assert_eq!(captures(first), 0);
    shell.handle(Command::Engine(EngineEvent::LoadingChanged {
        id: second,
        loading: false,
    }));
    while let Some(command) = queue.try_recv() {
        shell.handle(command);
    }
    assert_eq!(captures(second), 1);
    let done = engine.tab_preview_completions.lock().unwrap().remove(0);
    done(Some(vec![2]));
    while let Some(command) = queue.try_recv() {
        shell.handle(command);
    }
    assert_eq!(shell.read_tab_preview(window, second, 1), Some(vec![2]));

    // Leaving it keeps what was taken in front; the tab come to is taken.
    shell.handle(Command::Activate(first));
    while let Some(command) = queue.try_recv() {
        shell.handle(command);
    }
    assert_eq!(captures(second), 1);
    assert_eq!(captures(first), 1);
    let done = engine.tab_preview_completions.lock().unwrap().remove(0);
    done(None);
    while let Some(command) = queue.try_recv() {
        shell.handle(command);
    }
    assert_eq!(shell.read_tab_preview(window, second, 1), Some(vec![2]));
}

#[test]
#[cfg(not(target_os = "windows"))]
fn outgoing_preview_is_requested_before_the_switch_layout_and_cached_only_on_completion() {
    let (mut shell, engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    let first = loaded(&mut shell, &screen, "one");
    let second = loaded(&mut shell, &screen, "two");
    commit_url(&mut shell, second, "https://two.test/");
    shell.tab_previews = Default::default();
    let queue = CommandQueue::new();
    shell.self_queue = Some(queue.clone());
    let window = shell.windows.focused().unwrap().id;
    engine.calls.lock().unwrap().clear();

    shell.handle(Command::Activate(first));

    let calls = engine.calls();
    let capture = calls
        .iter()
        .position(|call| call == &format!("capture-preview {second}"))
        .unwrap();
    let layout = calls
        .iter()
        .position(|call| call.starts_with("layout@"))
        .unwrap();
    assert!(capture < layout, "{calls:?}");
    assert_eq!(active_id(&screen), first);
    assert!(shell.read_tab_preview(window, second, 1).is_none());
    let done = engine.tab_preview_completions.lock().unwrap().remove(0);
    done(Some(vec![1, 2, 3]));
    while let Some(command) = queue.try_recv() {
        shell.handle(command);
    }
    assert_eq!(
        shell.read_tab_preview(window, second, 1),
        Some(vec![1, 2, 3])
    );

    shell.handle(Command::Activate(second));
    shell.handle(Command::Activate(first));
    assert_eq!(
        engine
            .calls()
            .iter()
            .filter(|call| **call == format!("capture-preview {second}"))
            .count(),
        1
    );
}

#[test]
fn rapid_switches_keep_one_capture_in_flight_and_refuse_stale_completion() {
    let (mut shell, engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    let first = loaded(&mut shell, &screen, "one");
    commit_url(&mut shell, first, "https://one.test/");
    let second = loaded(&mut shell, &screen, "two");
    commit_url(&mut shell, second, "https://two.test/");
    shell.tab_previews = Default::default();
    shell.self_queue = Some(CommandQueue::new());
    let window = shell.windows.focused().unwrap().id;
    engine.calls.lock().unwrap().clear();

    shell.handle(Command::Activate(first));
    shell.handle(Command::Activate(second));
    shell.handle(Command::CaptureTabPreview);
    assert_eq!(engine.tab_preview_completions.lock().unwrap().len(), 1);
    shell.handle(Command::TabPreviewReady {
        request: 99,
        bytes: Some(vec![9]),
    });
    assert_eq!(engine.tab_preview_completions.lock().unwrap().len(), 1);
    assert!(shell.read_tab_preview(window, second, 99).is_none());
    commit_url(&mut shell, second, "https://changed.test/");
    shell.handle(Command::TabPreviewReady {
        request: 1,
        bytes: Some(vec![1]),
    });
    assert!(shell.read_tab_preview(window, second, 1).is_none());
    assert_eq!(engine.tab_preview_completions.lock().unwrap().len(), 2);
    assert_eq!(
        engine
            .calls()
            .iter()
            .filter(|call| call.starts_with("capture-preview"))
            .count(),
        2
    );
}

#[test]
fn a_card_acts_on_its_own_tab_whatever_the_selection_has_become() {
    let (mut shell, _, screen) = setup();
    shell.handle(Command::Bootstrap);
    let first = loaded(&mut shell, &screen, "one");
    let second = loaded(&mut shell, &screen, "two");
    let third = loaded(&mut shell, &screen, "three");
    shell.handle(Command::BeginTabSwitcher {
        session: 9,
        reverse: false,
        keyboard: false,
    });
    for action in [
        TabSwitcherAction::Select { index: 1 },
        TabSwitcherAction::Select { index: 2 },
        TabSwitcherAction::CloseTab {
            tab_id: second.to_string(),
        },
        TabSwitcherAction::CloseTab {
            tab_id: "not-a-tab".into(),
        },
    ] {
        shell.handle(Command::TabSwitcher { session: 9, action });
    }
    assert!(shell.items.tab(second).is_none());
    assert!(shell.items.tab(first).is_some());
    assert_eq!(active_id(&screen), third);
    shell.handle(Command::TabSwitcher {
        session: 9,
        action: TabSwitcherAction::Activate {
            tab_id: first.to_string(),
        },
    });
    assert_eq!(active_id(&screen), first);
}

#[test]
fn from_a_blank_tab_the_first_step_is_the_latest_page() {
    let (mut shell, _, screen) = setup();
    shell.handle(Command::Bootstrap);
    let first = loaded(&mut shell, &screen, "one");
    let second = loaded(&mut shell, &screen, "two");
    shell.handle(Command::Open);
    let blank = active_id(&screen);
    assert_eq!(shell.recent_tab_order(), vec![second, first]);
    shell.handle(Command::BeginTabSwitcher {
        session: 3,
        reverse: false,
        keyboard: true,
    });
    shell.handle(Command::TabSwitcher {
        session: 3,
        action: TabSwitcherAction::Commit,
    });
    assert_eq!(active_id(&screen), second);
    shell.handle(Command::Activate(blank));
    shell.handle(Command::BeginTabSwitcher {
        session: 4,
        reverse: true,
        keyboard: true,
    });
    shell.handle(Command::TabSwitcher {
        session: 4,
        action: TabSwitcherAction::Commit,
    });
    assert_eq!(active_id(&screen), first);
}

#[test]
fn browser_pages_are_switchable_and_an_empty_switcher_still_offers_a_new_tab() {
    let (mut shell, _, screen) = setup();
    shell.handle(Command::Bootstrap);
    shell.handle_operation(Command::ShowBrowserPage(Some(
        crate::BrowserPage::Extensions,
    )));
    let extensions = active_id(&screen);
    shell.handle(Command::Open);
    assert_eq!(shell.recent_tab_order(), vec![extensions]);
    shell.handle(Command::Close(extensions));
    assert!(shell.recent_tab_order().is_empty());
    shell.handle(Command::BeginTabSwitcher {
        session: 6,
        reverse: false,
        keyboard: false,
    });
    let blank = active_id(&screen);
    shell.handle(Command::TabSwitcher {
        session: 6,
        action: TabSwitcherAction::NewTab,
    });
    assert_ne!(active_id(&screen), blank);
}
