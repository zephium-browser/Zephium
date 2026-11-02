use super::*;

#[test]
fn automatic_pip_follows_tab_selection_and_respects_split_visibility_and_preference() {
    let (mut shell, engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    let first = active_id(&screen);
    shell.handle(Command::Open);
    let second = active_id(&screen);
    engine.automatic_pip.lock().unwrap().clear();
    shell.handle(Command::Activate(first));
    assert_eq!(
        *engine.automatic_pip.lock().unwrap(),
        vec![(Some(second), true), (Some(first), false)]
    );
    engine.automatic_pip.lock().unwrap().clear();
    shell.windows.focused_mut().unwrap().splits = Some(Pane::Branch {
        axis: zephium_core::split::Axis::Row,
        ratio: 0.5,
        a: Box::new(Pane::leaf(first)),
        b: Box::new(Pane::leaf(second)),
    });
    shell.focus_tab(second);
    assert_eq!(
        *engine.automatic_pip.lock().unwrap(),
        vec![(Some(second), false)]
    );
    shell.operation_set_app_setting("tabs.automatic-pip".into(), "false".into());
    assert_eq!(
        engine.automatic_pip.lock().unwrap().last(),
        Some(&(None, false))
    );
    engine.automatic_pip.lock().unwrap().clear();
    shell.windows.focused_mut().unwrap().splits = None;
    shell.focus_tab(first);
    assert_eq!(
        *engine.automatic_pip.lock().unwrap(),
        vec![(Some(first), false)]
    );
}

#[test]
fn switching_tabs_shows_only_active() {
    let (mut shell, engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    let first = active_id(&screen);
    shell.handle(Command::Navigate {
        id: first,
        input: "example.com".into(),
    });
    shell.handle(Command::Open);
    let second = active_id(&screen);
    shell.handle(Command::Navigate {
        id: second,
        input: "github.com".into(),
    });
    assert_eq!(engine.last_layout(), vec![second.to_string()]);

    shell.handle(Command::Activate(first));
    assert_eq!(engine.last_layout(), vec![first.to_string()]);
}

#[test]
fn recently_closed_tab_restores_with_a_fresh_identity_and_durable_metadata() {
    let store = Arc::new(FakeStore::default());
    let (mut shell, engine, screen) = setup_with(store.clone());
    shell.handle(Command::Bootstrap);
    let closed = active_id(&screen);
    let closed_profile = shell.profile_of_item(closed).unwrap();
    store.zoom_saved.lock().unwrap().insert(
        closed_profile,
        vec![zephium_core::zoom::SavedLevel {
            site: "restore.example".into(),
            percent: 125,
            set_at: 1,
        }],
    );
    navigate_and_commit(&mut shell, closed, "restore.example/path");
    shell.items.set_title(closed, "Restorable title".into());
    shell.handle(Command::Close(closed));
    assert_eq!(shell.recently_closed.len(), 1);
    let profile = shell.windows.focused().unwrap().profile;
    let (restored, native) = shell
        .restore_recently_closed_tab(profile, None)
        .expect("recent tab must restore in its original focused space");
    assert_ne!(restored, closed);
    assert!(!native.rejected);
    assert!(shell.recently_closed.is_empty());
    let tab = shell.items.tab(restored).unwrap();
    assert_eq!(
        tab.url.as_ref().map(url::Url::as_str),
        Some("https://restore.example/path")
    );
    assert_eq!(tab.title, "Restorable title");
    // The page comes back at what its site shows, not what the tab held.
    assert_eq!(engine.last_zoom_request().0, restored);
    assert_eq!(engine.last_zoom_request().1, 1.25);
    assert!(engine.calls().iter().any(|call| {
        call == &format!("create {restored} https://restore.example/path [default]")
    }));
    assert!(store
        .saved
        .lock()
        .unwrap()
        .as_ref()
        .is_some_and(|session| session.recently_closed.is_empty()));
}

#[test]
fn legacy_closed_tab_without_identity_still_reopens() {
    let (mut shell, _, screen) = setup();
    shell.handle(Command::Bootstrap);
    let tab = active_id(&screen);
    navigate_and_commit(&mut shell, tab, "legacy.example");
    shell.handle(Command::Close(tab));
    let profile = shell.windows.focused().unwrap().profile;
    shell.recently_closed[0].session_id = None;
    shell.recently_closed[0].closed_at_ms = None;

    assert!(shell.restore_recently_closed_tab(profile, None).is_some());
}

#[test]
fn forged_runtime_ids_cannot_cross_the_focused_space_or_profile() {
    let (mut shell, engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    let local = active_id(&screen);
    shell.handle(Command::Navigate {
        id: local,
        input: "local.example".into(),
    });
    let (profile, space, window) = shell
        .windows
        .focused()
        .map(|win| (win.profile, win.space, win.id))
        .unwrap();

    let sibling_space = SpaceId::from(9001);
    assert!(shell.spaces.insert(Space {
        id: sibling_space,
        profile,
        name: "Sibling".into(),
        look: Default::default(),
    }));
    let foreign_profile = ProfileId::from(9002);
    let foreign_space = SpaceId::from(9003);
    assert!(shell.profiles.insert(Profile {
        id: foreign_profile,
        name: "Foreign".into(),
        kind: ProfileKind::Named,
    }));
    assert!(shell.spaces.insert(Space {
        id: foreign_space,
        profile: foreign_profile,
        name: "Foreign".into(),
        look: Default::default(),
    }));

    let sibling = ItemId::from(9004);
    let foreign = ItemId::from(9005);
    assert!(shell.items.insert_tab(
        sibling,
        Placement::Space {
            space: sibling_space,
            section: SpaceSection::Today,
        },
    ));
    assert!(shell.items.insert_tab(
        foreign,
        Placement::Space {
            space: foreign_space,
            section: SpaceSection::Today,
        },
    ));

    let focused = shell.windows.get(window).unwrap();
    let region = layout::compute(focused.size, focused.mode, focused.metrics, true)
        .content
        .unwrap();
    let (drop_x, drop_y) = (region.x + 1.0, region.y + region.height / 2.0);
    assert!(shell.resolve_drop(drop_x, drop_y).is_some());

    for (attacker, attacker_space) in [(sibling, sibling_space), (foreign, foreign_space)] {
        let calls_before = engine.calls();
        let roots_before = shell
            .items
            .roots(Placement::Space {
                space: attacker_space,
                section: SpaceSection::Today,
            })
            .len();

        shell.handle(Command::Activate(attacker));
        shell.handle(Command::Navigate {
            id: attacker,
            input: "attacker.example".into(),
        });
        shell.handle(Command::Reload(attacker));
        shell.handle(Command::SplitWith {
            other: attacker,
            axis: Axis::Row,
        });
        shell.handle(Command::DropTab {
            id: attacker,
            x: drop_x,
            y: drop_y,
        });
        shell.handle(Command::Engine(EngineEvent::NewWindowRequested {
            id: attacker,
            url: "https://popup.example/".into(),
        }));
        shell.handle(Command::Close(attacker));

        assert_eq!(shell.windows.focused().unwrap().active, Some(local));
        assert!(shell.windows.focused().unwrap().splits.is_none());
        assert!(shell.items.get(attacker).is_some());
        assert!(shell.items.tab(attacker).unwrap().url.is_none());
        assert_eq!(
            shell
                .items
                .roots(Placement::Space {
                    space: attacker_space,
                    section: SpaceSection::Today,
                })
                .len(),
            roots_before,
            "a foreign popup source must not create a tab in its space"
        );
        assert_eq!(engine.calls(), calls_before);
    }

    assert_eq!(active_id(&screen), local);
    assert_eq!(shell.windows.focused().unwrap().space, space);
    assert_eq!(shell.windows.focused().unwrap().profile, profile);
}

#[test]
fn linked_tab_shows_alone_and_split_group_survives() {
    let (mut shell, engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    let first = active_id(&screen);
    shell.handle(Command::Navigate {
        id: first,
        input: "example.com".into(),
    });
    shell.handle(Command::Open);
    let second = active_id(&screen);
    shell.handle(Command::Navigate {
        id: second,
        input: "github.com".into(),
    });
    shell.handle(Command::SplitWith {
        other: first,
        axis: Axis::Row,
    });
    assert_eq!(engine.last_layout().len(), 2);

    // page JS opens a link: the popup shows alone
    shell.handle(Command::Engine(EngineEvent::NewWindowRequested {
        id: second,
        url: "https://wikipedia.org/".into(),
    }));
    assert_eq!(engine.last_layout().len(), 1);

    // returning to a member restores the whole group
    shell.handle(Command::Activate(first));
    assert_eq!(engine.last_layout().len(), 2);
}

#[test]
fn dragging_over_a_pane_edge_previews_the_split_and_dropping_clears_it() {
    let (mut shell, engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    let first = active_id(&screen);
    navigate_and_commit(&mut shell, first, "example.com");
    shell.handle(Command::Open);
    let second = active_id(&screen);
    navigate_and_commit(&mut shell, second, "github.com");
    shell.handle(Command::Activate(first));

    let win = shell.windows.focused().unwrap();
    let gap = win.metrics.gap;
    let region = layout::compute(win.size, win.mode, win.metrics, true)
        .content
        .unwrap();
    let (x, y) = (region.x + 2.0, region.y + region.height / 2.0);
    shell.handle(Command::DragOver {
        point: Some((x, y)),
    });
    let preview = engine.previews().pop().flatten().expect("a preview");
    assert_eq!(preview.tab, first);
    // The new pane takes the left half and the page moves into the right.
    let half = (region.width - gap) / 2.0;
    assert_eq!(preview.zone, Rect::new(0.0, 0.0, half, region.height));
    assert_eq!(preview.shift, (half + gap, 0.0));

    shell.handle(Command::DragOver { point: None });
    assert_eq!(engine.previews().pop(), Some(None));

    shell.handle(Command::DropTab { id: second, x, y });
    assert_eq!(engine.previews().pop(), Some(None));
    assert_eq!(engine.last_layout().len(), 2);
}

#[test]
fn url_focus_emits_ui_command() {
    let engine = Arc::new(FakeEngine::default());
    let seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = seen.clone();
    let mut shell = Shell::new(
        engine,
        Arc::new(FakeStore::default()),
        Arc::new(FakeChrome),
        Box::new(move |p| {
            if let Projection::UiCommand(id) = p {
                sink.lock().unwrap().push(id);
            }
        }),
    );
    shell.handle(Command::Run("url.focus".into()));
    assert_eq!(seen.lock().unwrap().as_slice(), ["url.focus"]);
}

#[test]
fn open_url_lands_in_a_new_tab() {
    let (mut shell, engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    let first = active_id(&screen);
    shell.handle(Command::OpenUrl {
        input: "github.com".into(),
        new_tab: true,
    });
    let second = active_id(&screen);
    assert_ne!(first, second);
    assert!(engine
        .calls()
        .iter()
        .any(|c| c == &format!("create {second} https://github.com/ [default]")));
}

#[test]
fn open_url_at_item_limit_never_navigates_the_active_tab() {
    let (mut shell, engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    let active = active_id(&screen);
    navigate_and_commit(&mut shell, active, "kept.example");
    let space = shell.windows.focused().unwrap().space;
    for value in 0..(zephium_core::session::MAX_SESSION_ITEMS - 1) {
        assert!(shell.items.insert_tab(
            ItemId::from(100_000 + value as u128),
            Placement::Space {
                space,
                section: SpaceSection::Today,
            },
        ));
    }
    let calls_before = engine.calls().len();

    let completion = shell.handle_operation(Command::OpenUrl {
        input: "must-not-replace.example".into(),
        new_tab: true,
    });

    assert_eq!(completion.outcome, OperationOutcome::Rejected);
    assert_eq!(completion.reason, OperationReason::ItemLimitReached);
    assert_eq!(shell.windows.focused().unwrap().active, Some(active));
    assert_eq!(
        shell
            .items
            .tab(active)
            .and_then(|tab| tab.url.as_ref().map(url::Url::as_str)),
        Some("https://kept.example/")
    );
    assert_eq!(engine.calls().len(), calls_before);
}

#[test]
fn native_tab_adoption_preserves_background_focus_and_does_not_replay_navigation() {
    for foreground in [false, true] {
        let (mut shell, engine, screen) = setup();
        shell.handle(Command::Bootstrap);
        let source = active_id(&screen);
        navigate_and_commit(&mut shell, source, "example.com");
        let child = ItemId::generate();
        let (send, receive) = std::sync::mpsc::channel();
        let before = engine.calls().len();
        shell.handle(Command::Engine(EngineEvent::NativeTabOpened {
            id: source,
            child,
            foreground,
            adoption: zephium_core::ports::engine::NativeTabAdoption::new(move |ok| {
                send.send(ok).unwrap()
            }),
        }));
        assert!(receive.recv().unwrap());
        assert_eq!(active_id(&screen), source);
        let tab = shell.items.tab(child).unwrap();
        assert!(tab.has_view());
        assert!(
            tab.url.is_none(),
            "construction cannot fabricate a committed URL"
        );
        assert!(!engine.calls()[before..]
            .iter()
            .any(|call| call.starts_with("create ") || call.starts_with("navigate ")));
        shell.handle(Command::Engine(EngineEvent::LinkedDownloadStarted {
            id: child,
        }));
        assert!(shell.items.get(child).is_none());
        assert_eq!(active_id(&screen), source);
    }
}

#[test]
fn native_tab_adoption_rejects_an_unknown_source_and_cleans_up_the_lease() {
    let (mut shell, _, screen) = setup();
    shell.handle(Command::Bootstrap);
    let current = active_id(&screen);
    let child = ItemId::generate();
    let (send, receive) = std::sync::mpsc::channel();
    shell.handle(Command::Engine(EngineEvent::NativeTabOpened {
        id: ItemId::generate(),
        child,
        foreground: true,
        adoption: zephium_core::ports::engine::NativeTabAdoption::new(move |ok| {
            send.send(ok).unwrap()
        }),
    }));
    assert!(!receive.recv().unwrap());
    assert!(shell.items.get(child).is_none());
    assert_eq!(active_id(&screen), current);
}

#[test]
fn download_cleanup_does_not_close_a_native_tab_that_has_committed_a_document() {
    let (mut shell, _, screen) = setup();
    shell.handle(Command::Bootstrap);
    let source = active_id(&screen);
    navigate_and_commit(&mut shell, source, "example.com");
    let child = ItemId::generate();
    shell.handle(Command::Engine(EngineEvent::NativeTabOpened {
        id: source,
        child,
        foreground: true,
        adoption: zephium_core::ports::engine::NativeTabAdoption::new(|_| {}),
    }));
    commit_url(&mut shell, child, "https://example.com/other");
    shell.handle(Command::Engine(presentation_pending(
        child,
        NavigationPresentationId::from_raw(77),
        "https://example.com/other",
    )));
    shell.handle(Command::Engine(EngineEvent::LinkedDownloadStarted {
        id: child,
    }));
    assert!(shell.items.get(child).is_some());
    assert_eq!(active_id(&screen), child);
}

#[test]
fn native_foreground_selection_waits_for_presentation_and_respects_newer_user_selection() {
    for (foreground, switch_away) in [(true, false), (false, false), (true, true)] {
        let (mut shell, _, screen) = setup();
        shell.handle(Command::Bootstrap);
        let source = active_id(&screen);
        navigate_and_commit(&mut shell, source, "example.com");
        let child = ItemId::generate();
        shell.handle(Command::Engine(EngineEvent::NativeTabOpened {
            id: source,
            child,
            foreground,
            adoption: zephium_core::ports::engine::NativeTabAdoption::new(|_| {}),
        }));
        assert_eq!(active_id(&screen), source);
        if switch_away {
            shell.handle(Command::Open);
            shell.handle(Command::Activate(source));
        }
        commit_url(&mut shell, child, "https://example.com/child");
        assert_eq!(
            active_id(&screen),
            source,
            "URL alone does not select a tab"
        );
        shell.handle(Command::Engine(presentation_pending(
            child,
            NavigationPresentationId::from_raw(78),
            "https://example.com/child",
        )));
        assert_eq!(
            active_id(&screen),
            if foreground && !switch_away {
                child
            } else {
                source
            }
        );
    }
}

#[test]
fn native_close_request_only_closes_an_owned_child() {
    let (mut shell, _, screen) = setup();
    shell.handle(Command::Bootstrap);
    let source = active_id(&screen);
    shell.handle(Command::Engine(EngineEvent::NativeTabCloseRequested {
        id: source,
    }));
    assert!(shell.items.get(source).is_some());
    let child = ItemId::generate();
    shell.handle(Command::Engine(EngineEvent::NativeTabOpened {
        id: source,
        child,
        foreground: true,
        adoption: zephium_core::ports::engine::NativeTabAdoption::new(|_| {}),
    }));
    shell.handle(Command::Engine(EngineEvent::NativeTabCloseRequested {
        id: child,
    }));
    assert!(shell.items.get(child).is_none());
    assert_eq!(active_id(&screen), source);
}

#[test]
fn native_download_cleanup_survives_a_space_switch() {
    let (mut shell, _, screen) = setup();
    shell.handle(Command::Bootstrap);
    let source = active_id(&screen);
    let child = ItemId::generate();
    shell.handle(Command::Engine(EngineEvent::NativeTabOpened {
        id: source,
        child,
        foreground: true,
        adoption: zephium_core::ports::engine::NativeTabAdoption::new(|_| {}),
    }));
    let other = SpaceId::from(9001);
    let profile = shell.windows.focused().unwrap().profile;
    assert!(shell.spaces.insert(Space {
        id: other,
        profile,
        name: "Other".into(),
        look: Default::default(),
    }));
    let window = shell.windows.focused_mut().unwrap();
    window.space = other;
    window.active = None;
    shell.handle(Command::Open);
    let selected = active_id(&screen);
    shell.handle(Command::Engine(EngineEvent::LinkedDownloadStarted {
        id: child,
    }));
    assert!(shell.items.get(child).is_none());
    assert!(shell.items.get(source).is_some());
    assert_eq!(active_id(&screen), selected);
}

#[test]
fn closing_an_essential_ends_its_page_and_keeps_it_in_place() {
    let (mut shell, engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    let space = shell.windows.focused().unwrap().space;
    let site = |url: &str| crate::ImportedSite {
        url: url.into(),
        title: url.into(),
    };
    let destination = shell.import_destination().unwrap();
    shell.handle(Command::Import {
        destination,
        work: Box::new(crate::ImportWork::Essentials(vec![
            site("https://mail.example/"),
            site("https://board.example/"),
        ])),
        done: crate::ImportCompletion::new(|_| {}),
    });
    let kept = |shell: &Shell| shell.items.roots(Placement::Favorites { space }).to_vec();
    let mail = kept(&shell)[0];
    shell.handle(Command::Activate(mail));
    assert!(shell.items.tab(mail).unwrap().has_view());
    navigate_and_commit(&mut shell, mail, "mail.example/inbox");

    shell.handle(Command::Close(mail));
    let after = kept(&shell);
    assert_eq!(after.len(), 2);
    let reopened = after[0];
    assert_ne!(reopened, mail, "a closed view's id is never reused");
    let tab = shell.items.tab(reopened).unwrap();
    assert!(!tab.has_view());
    assert_eq!(
        tab.url.as_ref().unwrap().as_str(),
        "https://mail.example/inbox"
    );
    assert!(shell.recently_closed.is_empty());
    // Focus moves on by the usual close policy, never back onto the ended page.
    assert_ne!(active_id(&screen), reopened);
    assert!(engine
        .last_layout()
        .iter()
        .all(|id| *id != reopened.to_string()));
}

fn today_of(shell: &Shell, space: SpaceId) -> Vec<ItemId> {
    shell
        .items
        .roots(Placement::Space {
            space,
            section: SpaceSection::Today,
        })
        .to_vec()
}

#[test]
fn a_background_tab_opens_right_after_its_source_and_takes_no_focus() {
    let (mut shell, engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    let first = active_id(&screen);
    shell.handle(Command::Open);
    let second = active_id(&screen);
    shell.handle(Command::Open);
    let third = active_id(&screen);
    shell.handle(Command::Activate(first));
    shell.windows.focused_mut().unwrap().splits = Some(Pane::Branch {
        axis: Axis::Row,
        ratio: 0.5,
        a: Box::new(Pane::leaf(first)),
        b: Box::new(Pane::leaf(second)),
    });
    let (profile, space) = shell
        .windows
        .focused()
        .map(|win| (win.profile, win.space))
        .unwrap();
    let layout = engine.last_layout();
    let panes = shell
        .windows
        .focused()
        .unwrap()
        .splits
        .clone()
        .unwrap()
        .tabs();

    let opened = shell
        .open_background_tab(profile, space, Some(first), "https://example.test/a")
        .expect("a background tab");
    let again = shell
        .open_background_tab(profile, space, Some(third), "https://example.test/b")
        .expect("a background tab at the end");

    assert_eq!(
        today_of(&shell, space),
        [first, opened, second, third, again]
    );
    let window = shell.windows.focused().unwrap();
    assert_eq!(window.active, Some(first));
    assert_eq!(window.splits.clone().unwrap().tabs(), panes);
    assert_eq!(active_id(&screen), first);
    assert_eq!(engine.last_layout(), layout);
    assert!(engine
        .calls()
        .iter()
        .any(|call| call == &format!("create {opened} https://example.test/a [default]")));
}

#[test]
fn a_background_tab_without_a_source_in_today_goes_to_the_end() {
    let (mut shell, _engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    let first = active_id(&screen);
    shell.handle(Command::Open);
    let second = active_id(&screen);
    let (profile, space) = shell
        .windows
        .focused()
        .map(|win| (win.profile, win.space))
        .unwrap();
    assert!(shell.items.move_tab_to_root(
        first,
        Placement::Space {
            space,
            section: SpaceSection::Pinned,
        },
        None,
    ));

    for after in [Some(first), Some(ItemId::from(9_999)), None] {
        let opened = shell
            .open_background_tab(profile, space, after, "https://example.test/")
            .expect("a background tab");
        assert_eq!(today_of(&shell, space).last(), Some(&opened));
    }
    assert_eq!(shell.windows.focused().unwrap().active, Some(second));
}

#[test]
fn a_background_tab_is_refused_outside_the_persons_scope_or_for_other_addresses() {
    let (mut shell, engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    let first = active_id(&screen);
    let (profile, space) = shell
        .windows
        .focused()
        .map(|win| (win.profile, win.space))
        .unwrap();
    let foreign_profile = ProfileId::from(9_002);
    let foreign_space = SpaceId::from(9_003);
    assert!(shell.profiles.insert(Profile {
        id: foreign_profile,
        name: "Foreign".into(),
        kind: ProfileKind::Named,
    }));
    assert!(shell.spaces.insert(Space {
        id: foreign_space,
        profile: foreign_profile,
        name: "Foreign".into(),
        look: Default::default(),
    }));
    let tabs = shell.items.view_ids().len();
    let calls = engine.calls();
    let open = |shell: &mut Shell, profile, space, url: &str| {
        shell.open_background_tab(profile, space, Some(first), url)
    };

    for url in [
        "javascript:alert(1)",
        "about:blank",
        "file:///etc/passwd",
        "ftp://example.test/",
        "https://user:secret@example.test/",
        "example.test",
        "two words",
        "",
    ] {
        assert_eq!(open(&mut shell, profile, space, url), None, "{url}");
    }
    assert_eq!(
        open(&mut shell, profile, foreign_space, "https://example.test/"),
        None
    );
    assert_eq!(
        open(
            &mut shell,
            profile,
            SpaceId::from(9_999),
            "https://example.test/"
        ),
        None
    );
    assert_eq!(
        open(
            &mut shell,
            foreign_profile,
            foreign_space,
            "https://example.test/"
        ),
        None,
        "only the profile in front opens tabs"
    );
    assert_eq!(shell.items.view_ids().len(), tabs);
    assert_eq!(engine.calls(), calls);

    let mut private = shell.profiles.remove(profile).unwrap();
    private.kind = ProfileKind::Incognito;
    assert!(shell.profiles.insert(private));
    assert_eq!(
        open(&mut shell, profile, space, "https://example.test/"),
        None
    );
    assert_eq!(engine.calls(), calls);
}

#[test]
fn a_background_tab_is_refused_at_the_item_limit() {
    let (mut shell, _engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    let first = active_id(&screen);
    let (profile, space) = shell
        .windows
        .focused()
        .map(|win| (win.profile, win.space))
        .unwrap();
    let today = Placement::Space {
        space,
        section: SpaceSection::Today,
    };
    while shell.items.insert_tab(ItemId::generate(), today) {}
    let roots = today_of(&shell, space);

    assert_eq!(
        shell.open_background_tab(profile, space, Some(first), "https://example.test/"),
        None
    );
    assert_eq!(today_of(&shell, space), roots);
}

#[test]
fn a_background_tab_command_answers_through_its_completion() {
    let (mut shell, _engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    let first = active_id(&screen);
    let (profile, space) = shell
        .windows
        .focused()
        .map(|win| (win.profile, win.space))
        .unwrap();
    let answers = Arc::new(Mutex::new(Vec::new()));
    for url in ["https://example.test/", "javascript:alert(1)"] {
        let sink = answers.clone();
        shell.handle(Command::OpenBackgroundTab {
            profile,
            space,
            after: Some(first),
            url: url.into(),
            focus: false,
            done: crate::TabCompletion::new(move |tab| sink.lock().unwrap().push(tab)),
        });
    }

    let answers = answers.lock().unwrap().clone();
    assert_eq!(answers.len(), 2);
    assert_eq!(answers[0], today_of(&shell, space).get(1).copied());
    assert!(answers[0].is_some());
    assert_eq!(answers[1], None);
}

#[test]
fn a_background_tab_opens_through_the_handle() {
    let (handle, profile, space, active) = started(Arc::new(FakeStore::default()));

    let request =
        handle.open_background_tab(profile, space, Some(active), "https://example.test/".into());
    fn runs_on_any_thread<T: Send>(_: &T) {}
    runs_on_any_thread(&request);
    let opened = block_on(request).expect("a background tab");

    let tabs = handle.window_tabs(profile).recv().unwrap();
    assert_eq!(
        tabs.iter().map(|tab| tab.id).collect::<Vec<_>>(),
        [active, opened]
    );
    for url in ["javascript:alert(1)", "file:///etc/passwd"] {
        assert_eq!(
            block_on(handle.open_background_tab(profile, space, None, url.into())),
            None
        );
    }
    assert_eq!(handle.window_tabs(profile).recv().unwrap().len(), 2);
}

#[test]
fn a_tab_opened_in_front_is_shown_after_its_source() {
    let (mut shell, _engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    let first = active_id(&screen);
    shell.handle(Command::Open);
    let second = active_id(&screen);
    shell.handle(Command::Activate(first));
    let (profile, space) = shell
        .windows
        .focused()
        .map(|win| (win.profile, win.space))
        .unwrap();
    let opened = shell
        .open_page_tab(profile, space, Some(first), "https://example.test/", true)
        .expect("a tab in front");
    assert_eq!(today_of(&shell, space), [first, opened, second]);
    assert_eq!(shell.windows.focused().unwrap().active, Some(opened));
}

fn focused_scope(shell: &Shell) -> (ProfileId, SpaceId) {
    let window = shell.windows.focused().unwrap();
    (window.profile, window.space)
}

/// Opens a loaded Today tab and leaves it in front.
fn open_page(shell: &mut Shell, screen: &Screen, address: &str) -> ItemId {
    shell.handle(Command::Open);
    let id = active_id(screen);
    navigate_and_commit(shell, id, address);
    id
}

fn make_essential(shell: &mut Shell, id: ItemId) {
    shell.handle(Command::SetTabEssential {
        id,
        essential: true,
        before: None,
    });
}

fn pin(shell: &mut Shell, id: ItemId) {
    let (_, space) = focused_scope(shell);
    assert!(shell.items.move_tab_to_root(
        id,
        Placement::Space {
            space,
            section: SpaceSection::Pinned,
        },
        None,
    ));
}

#[test]
fn closing_the_last_open_tab_opens_a_new_tab_in_the_same_space() {
    let (mut shell, _engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    let (_, space) = focused_scope(&shell);
    let only = active_id(&screen);
    navigate_and_commit(&mut shell, only, "only.example");

    shell.handle(Command::Close(only));

    let fresh = active_id(&screen);
    assert_ne!(fresh, only);
    assert_eq!(today_of(&shell, space), vec![fresh]);
    assert!(shell.items.tab(fresh).unwrap().url.is_none());
    assert_eq!(shell.windows.focused().unwrap().space, space);
}

#[test]
fn closing_never_hands_the_window_to_an_essential_or_a_pinned_tab() {
    for preference in ["next", "previous", "recent"] {
        let (mut shell, _engine, screen) = setup();
        shell.handle(Command::Bootstrap);
        shell.operation_set_app_setting("tabs.after-close".into(), preference.into());
        let (_, space) = focused_scope(&shell);
        let first = active_id(&screen);
        navigate_and_commit(&mut shell, first, "first.example");
        let essential = open_page(&mut shell, &screen, "essential.example");
        make_essential(&mut shell, essential);
        let pinned = open_page(&mut shell, &screen, "pinned.example");
        pin(&mut shell, pinned);
        let last = open_page(&mut shell, &screen, "last.example");

        shell.handle(Command::Close(last));
        assert_eq!(active_id(&screen), first, "{preference}");
        shell.handle(Command::Close(first));

        let fresh = active_id(&screen);
        assert_ne!(fresh, essential, "{preference}");
        assert_ne!(fresh, pinned, "{preference}");
        assert_eq!(today_of(&shell, space), vec![fresh], "{preference}");
        assert!(
            shell.items.tab(fresh).unwrap().url.is_none(),
            "{preference}"
        );
        assert!(shell.items.tab(pinned).is_some());
    }
}

#[test]
fn previously_active_mode_does_not_loop_back_through_the_essentials() {
    let (mut shell, _engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    shell.operation_set_app_setting("tabs.after-close".into(), "recent".into());
    let (_, space) = focused_scope(&shell);
    let home = active_id(&screen);
    navigate_and_commit(&mut shell, home, "home.example");
    let essential = open_page(&mut shell, &screen, "mail.example");
    make_essential(&mut shell, essential);
    let a = open_page(&mut shell, &screen, "a.example");
    let b = open_page(&mut shell, &screen, "b.example");
    shell.handle(Command::Activate(essential));
    shell.handle(Command::Activate(b));
    let essentials = shell.items.roots(Placement::Favorites { space }).to_vec();

    for closing in [b, a, home] {
        shell.handle(Command::Close(closing));
        let front = active_id(&screen);
        assert!(!essentials.contains(&front));
        assert!(today_of(&shell, space).contains(&front));
    }
    let remaining = today_of(&shell, space);
    assert_eq!(remaining.len(), 1);
    assert!(shell.items.tab(remaining[0]).unwrap().url.is_none());
    assert_eq!(shell.items.roots(Placement::Favorites { space }).len(), 1);
}

#[test]
fn closing_the_front_essential_or_pinned_tab_follows_the_open_tabs() {
    let (mut shell, _engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    let (_, space) = focused_scope(&shell);
    let first = active_id(&screen);
    navigate_and_commit(&mut shell, first, "first.example");
    let second = open_page(&mut shell, &screen, "second.example");
    let essential = open_page(&mut shell, &screen, "essential.example");
    make_essential(&mut shell, essential);
    let pinned = open_page(&mut shell, &screen, "pinned.example");
    pin(&mut shell, pinned);

    shell.handle(Command::Activate(essential));
    shell.handle(Command::Close(essential));
    assert_eq!(
        active_id(&screen),
        first,
        "the first open tab, not the next essential"
    );

    shell.handle(Command::Activate(pinned));
    shell.handle(Command::Close(pinned));
    assert_eq!(active_id(&screen), first);
    assert_eq!(today_of(&shell, space), vec![first, second]);
}

#[test]
fn closing_the_front_essential_with_no_open_tab_opens_a_new_tab_and_keeps_the_essential() {
    let (mut shell, _engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    let (_, space) = focused_scope(&shell);
    let blank = active_id(&screen);
    navigate_and_commit(&mut shell, blank, "mail.example/inbox");
    make_essential(&mut shell, blank);
    assert!(today_of(&shell, space).is_empty());

    shell.handle(Command::Close(blank));

    let fresh = active_id(&screen);
    assert_eq!(today_of(&shell, space), vec![fresh]);
    assert!(shell.items.tab(fresh).unwrap().url.is_none());
    let kept = shell.items.roots(Placement::Favorites { space }).to_vec();
    assert_eq!(kept.len(), 1);
    assert!(!shell.items.tab(kept[0]).unwrap().has_view());
}

#[test]
fn closing_a_tab_behind_the_front_one_opens_nothing() {
    let (mut shell, _engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    let (_, space) = focused_scope(&shell);
    let essential = active_id(&screen);
    navigate_and_commit(&mut shell, essential, "essential.example");
    make_essential(&mut shell, essential);
    let behind = open_page(&mut shell, &screen, "behind.example");
    shell.handle(Command::Activate(essential));

    shell.handle(Command::Close(behind));

    assert_eq!(active_id(&screen), essential);
    assert!(today_of(&shell, space).is_empty());
}

#[test]
fn closing_the_only_new_tab_changes_nothing() {
    let (mut shell, engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    let (_, space) = focused_scope(&shell);
    let only = active_id(&screen);
    engine.calls.lock().unwrap().clear();

    shell.handle(Command::Close(only));
    let disposition = shell.handle_operation(Command::Close(only));

    assert_eq!(disposition.outcome, OperationOutcome::NoOp);
    assert_eq!(active_id(&screen), only);
    assert_eq!(today_of(&shell, space), vec![only]);
    assert!(engine.calls().iter().all(|call| !call.starts_with("close")));
    let keyboard = shell.handle_operation(Command::Run("tab.close".into()));
    assert_eq!(keyboard.outcome, OperationOutcome::NoOp);
    assert_eq!(today_of(&shell, space), vec![only]);
}

#[test]
fn the_only_tab_whose_first_page_failed_closes_into_a_new_tab() {
    let (mut shell, _engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    let (_, space) = focused_scope(&shell);
    let failed = active_id(&screen);
    shell.handle(Command::Navigate {
        id: failed,
        input: "localhost:9".into(),
    });
    shell.handle(Command::Engine(EngineEvent::ViewCreationFailed {
        id: failed,
    }));
    assert!(shell.items.tab(failed).unwrap().failure.is_some());

    let disposition = shell.handle_operation(Command::Close(failed));

    assert_ne!(disposition.outcome, OperationOutcome::NoOp);
    let replacement = active_id(&screen);
    assert_ne!(replacement, failed);
    assert_eq!(today_of(&shell, space), vec![replacement]);
}

#[test]
fn the_only_new_tab_is_kept_beside_essentials_but_a_typed_one_is_replaced() {
    let (mut shell, _engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    let (_, space) = focused_scope(&shell);
    let essential = active_id(&screen);
    navigate_and_commit(&mut shell, essential, "essential.example");
    make_essential(&mut shell, essential);
    shell.handle(Command::Open);
    let blank = active_id(&screen);

    shell.handle(Command::Close(blank));
    assert_eq!(today_of(&shell, space), vec![blank]);
    assert_eq!(active_id(&screen), blank);

    shell.handle(Command::Navigate {
        id: blank,
        input: "typed.example".into(),
    });
    shell.handle(Command::Close(blank));
    let fresh = active_id(&screen);
    assert_ne!(fresh, blank);
    assert_eq!(today_of(&shell, space), vec![fresh]);
    assert!(shell.items.tab(fresh).unwrap().url.is_none());
}

#[test]
fn moving_the_front_tab_out_of_its_space_leaves_a_new_tab_behind() {
    let (mut shell, _engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    let (_, home) = focused_scope(&shell);
    let only = active_id(&screen);
    navigate_and_commit(&mut shell, only, "only.example");
    shell.handle(Command::Space(Box::new(crate::SpaceCommand::Create {
        name: "Other".into(),
        look: Default::default(),
    })));
    let other = focused_scope(&shell).1;
    shell.handle(Command::Space(Box::new(crate::SpaceCommand::Switch(home))));
    assert_eq!(active_id(&screen), only);

    shell.handle(Command::Space(Box::new(crate::SpaceCommand::MoveTab {
        tab: only,
        space: other,
    })));

    let fresh = active_id(&screen);
    assert_ne!(fresh, only);
    assert_eq!(focused_scope(&shell).1, home);
    assert_eq!(today_of(&shell, home), vec![fresh]);
    assert!(shell.items.tab(fresh).unwrap().url.is_none());
}

#[test]
fn closing_the_last_private_tab_still_ends_the_private_session() {
    let (mut shell, engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    let regular = active_id(&screen);
    let regular_profile = focused_scope(&shell).0;
    shell.handle(Command::Run("window.newPrivate".into()));
    let private_profile = focused_scope(&shell).0;
    assert!(shell.incognito_profile(private_profile));
    let lone = active_id(&screen);
    assert!(shell.items.tab(lone).unwrap().url.is_none());

    let closed = shell.handle_operation(Command::Close(lone));

    assert_ne!(closed.outcome, OperationOutcome::NoOp);
    assert_eq!(focused_scope(&shell).0, regular_profile);
    assert_eq!(active_id(&screen), regular);
    assert_eq!(engine.erasure_requests(), vec![private_profile]);
}

#[test]
fn closing_one_private_tab_hands_over_to_another() {
    let (mut shell, _engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    shell.handle(Command::Run("window.newPrivate".into()));
    let first = active_id(&screen);
    shell.handle(Command::Open);
    let second = active_id(&screen);

    shell.handle(Command::Close(second));

    assert_eq!(active_id(&screen), first);
    assert!(shell.incognito_profile(focused_scope(&shell).0));
}
