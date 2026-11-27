use super::*;
use crate::SpaceCommand;
use zephium_core::spaces::{SpaceIcon, SpaceLook, SpaceTheme};

fn browsed() -> (Shell, Arc<FakeEngine>, Screen, Arc<FakeStore>) {
    let store = Arc::new(FakeStore::default());
    let (mut shell, engine, screen) = setup_with(store.clone());
    shell.handle(Command::Bootstrap);
    (shell, engine, screen, store)
}

fn space_op(shell: &mut Shell, command: SpaceCommand) -> OperationDisposition {
    shell.handle_operation(Command::Space(Box::new(command)))
}

fn current(shell: &Shell) -> SpaceId {
    shell.windows.focused().unwrap().space
}

fn studio() -> SpaceLook {
    SpaceLook {
        icon: Some(SpaceIcon::Emoji("🎨".into())),
        color: 5,
        theme: Some(SpaceTheme::Preset("nord".into())),
    }
}

fn create(shell: &mut Shell, name: &str, look: SpaceLook) -> SpaceId {
    let created = space_op(
        shell,
        SpaceCommand::Create {
            name: name.into(),
            look,
        },
    );
    assert_ne!(created.outcome, OperationOutcome::Rejected);
    current(shell)
}

#[test]
fn a_new_space_is_drawn_switched_to_and_opens_a_tab_of_its_own() {
    let (mut shell, _engine, screen, _store) = browsed();
    let home = current(&shell);
    let first = active_id(&screen);
    navigate_and_commit(&mut shell, first, "home.example");

    let made = create(&mut shell, "  Studio  ", studio());
    assert_ne!(made, home);
    let state = last(&screen);
    assert_eq!(state.active_space_id, Some(made.to_string()));
    let opened = active_id(&screen);
    assert_ne!(opened, first);
    assert!(matches!(
        shell.items.get(opened).unwrap().placement,
        Placement::Space { space, section: SpaceSection::Today } if space == made
    ));
    let views: Vec<_> = state
        .spaces
        .iter()
        .map(|view| (view.name.as_str(), view.tabs, view.color))
        .collect();
    assert_eq!(views, vec![("Space", 1, 0), ("Studio", 1, 5)]);
    assert_eq!(
        state.spaces[1].icon,
        Some(zephium_ipc::SpaceIconView::Emoji {
            text: "🎨".into()
        })
    );
    assert_eq!(
        state.spaces[1].theme,
        Some(zephium_ipc::SpaceThemeView::Preset { id: "nord".into() })
    );

    let blank = space_op(
        &mut shell,
        SpaceCommand::Create {
            name: "   ".into(),
            look: SpaceLook::default(),
        },
    );
    assert_ne!(blank.outcome, OperationOutcome::Rejected);
    assert_eq!(shell.spaces.get(current(&shell)).unwrap().name, "Space 3");
    let bad = space_op(
        &mut shell,
        SpaceCommand::Create {
            name: "Bad".into(),
            look: SpaceLook {
                color: 99,
                ..SpaceLook::default()
            },
        },
    );
    assert_eq!(bad.outcome, OperationOutcome::Rejected);
}

#[test]
fn switching_returns_to_the_tab_each_space_left_and_drops_an_untouched_new_tab() {
    let (mut shell, _engine, screen, _store) = browsed();
    let home = current(&shell);
    let first = active_id(&screen);
    navigate_and_commit(&mut shell, first, "home.example");
    shell.handle(Command::Open);
    let second = active_id(&screen);
    navigate_and_commit(&mut shell, second, "two.example");
    shell.handle(Command::Activate(first));

    let studio = create(&mut shell, "Studio", SpaceLook::default());
    let untouched = active_id(&screen);
    let back = space_op(&mut shell, SpaceCommand::Switch(home));
    assert_ne!(back.outcome, OperationOutcome::Rejected);
    assert_eq!(current(&shell), home);
    assert_eq!(active_id(&screen), first);
    assert!(
        shell.items.get(untouched).is_none(),
        "a blank tab is not kept"
    );

    // Into the now-empty space: a fresh tab, never the profile's other tabs.
    space_op(&mut shell, SpaceCommand::Switch(studio));
    let fresh = active_id(&screen);
    assert!(shell.item_in_scope(fresh, shell.windows.focused().unwrap().profile, studio));
    navigate_and_commit(&mut shell, fresh, "studio.example");
    space_op(&mut shell, SpaceCommand::Switch(home));
    space_op(&mut shell, SpaceCommand::Switch(studio));
    assert_eq!(active_id(&screen), fresh);
    let again = space_op(&mut shell, SpaceCommand::Switch(studio));
    assert_eq!(again.outcome, OperationOutcome::NoOp);
    assert!(shell.items.tab(first).is_some() && shell.items.tab(second).is_some());
}

#[test]
fn switching_spaces_from_work_keeps_the_window_in_work() {
    let (mut shell, _engine, screen, _store) = browsed();
    let home = current(&shell);
    navigate_and_commit(&mut shell, active_id(&screen), "home.example");
    let studio = create(&mut shell, "Studio", SpaceLook::default());
    shell.handle_operation(Command::ShowBrowserPage(Some(crate::BrowserPage::Work)));
    assert_eq!(shell.active_browser_page(), Some(crate::BrowserPage::Work));

    let back = space_op(&mut shell, SpaceCommand::Switch(home));
    assert_ne!(back.outcome, OperationOutcome::Rejected);
    assert_eq!(current(&shell), home);
    assert_eq!(shell.active_browser_page(), Some(crate::BrowserPage::Work));

    let step = space_op(&mut shell, SpaceCommand::Step(1));
    assert_ne!(step.outcome, OperationOutcome::Rejected);
    assert_eq!(current(&shell), studio);
    assert_eq!(shell.active_browser_page(), Some(crate::BrowserPage::Work));
}

#[test]
fn spaces_are_edited_reordered_and_kept_across_a_restart() {
    let (mut shell, _engine, screen, store) = browsed();
    let home = current(&shell);
    navigate_and_commit(&mut shell, active_id(&screen), "home.example");
    let studio = create(&mut shell, "Studio", SpaceLook::default());
    navigate_and_commit(&mut shell, active_id(&screen), "studio.example");

    let edited = space_op(
        &mut shell,
        SpaceCommand::Update {
            id: studio,
            name: "Atelier".into(),
            look: studio_look(),
        },
    );
    assert_eq!(edited.outcome, OperationOutcome::Applied);
    let same = space_op(
        &mut shell,
        SpaceCommand::Update {
            id: studio,
            name: "Atelier".into(),
            look: studio_look(),
        },
    );
    assert_eq!(same.outcome, OperationOutcome::NoOp);
    for (name, look) in [
        ("", SpaceLook::default()),
        (
            "Theme",
            SpaceLook {
                theme: Some(SpaceTheme::Preset("missing".into())),
                ..SpaceLook::default()
            },
        ),
    ] {
        let refused = space_op(
            &mut shell,
            SpaceCommand::Update {
                id: studio,
                name: name.into(),
                look,
            },
        );
        assert_eq!(refused.outcome, OperationOutcome::Rejected, "{name}");
    }
    let moved = space_op(
        &mut shell,
        SpaceCommand::Reorder {
            id: studio,
            before: Some(home),
        },
    );
    assert_eq!(moved.outcome, OperationOutcome::Applied);
    let names: Vec<_> = last(&screen).spaces.into_iter().map(|s| s.name).collect();
    assert_eq!(names, ["Atelier", "Space"]);

    shell.persist();
    let saved = store.saved.lock().unwrap().clone().unwrap();
    assert_eq!(saved.spaces[0].id, studio);
    assert_eq!(saved.spaces[0].look, studio_look());
    assert_eq!(saved.active_space, Some(studio));

    let restarted = Arc::new(FakeStore::default());
    *restarted.saved.lock().unwrap() = Some(saved);
    let (mut shell, _engine, screen) = setup_with(restarted);
    shell.handle(Command::Bootstrap);
    let state = last(&screen);
    assert_eq!(state.active_space_id, Some(studio.to_string()));
    assert_eq!(state.spaces[0].name, "Atelier");
    assert_eq!(state.spaces[1].tabs, 1);
}

#[test]
fn a_space_starts_without_an_icon_and_an_icon_can_be_taken_away() {
    let (mut shell, _engine, screen, store) = browsed();
    assert_eq!(last(&screen).spaces[0].icon, None);
    let studio = create(&mut shell, "Studio", studio_look());
    assert!(last(&screen).spaces[1].icon.is_some());
    let bare = SpaceLook {
        icon: None,
        ..studio_look()
    };
    let cleared = space_op(
        &mut shell,
        SpaceCommand::Update {
            id: studio,
            name: "Studio".into(),
            look: bare.clone(),
        },
    );
    assert_eq!(cleared.outcome, OperationOutcome::Applied);
    let state = last(&screen);
    assert_eq!(state.spaces[1].icon, None);
    assert_eq!(state.spaces[1].color, 2);
    shell.persist();
    let saved = store.saved.lock().unwrap().clone().unwrap();
    assert_eq!(saved.spaces[1].look, bare);
}

fn studio_look() -> SpaceLook {
    SpaceLook {
        icon: Some(SpaceIcon::Symbol(7)),
        color: 2,
        theme: Some(SpaceTheme::Preset("dracula".into())),
    }
}

#[test]
fn deleting_a_space_closes_its_tabs_and_never_the_last_one() {
    let (mut shell, engine, screen, _store) = browsed();
    let home = current(&shell);
    let kept = active_id(&screen);
    navigate_and_commit(&mut shell, kept, "home.example");
    let studio = create(&mut shell, "Studio", SpaceLook::default());
    let a = active_id(&screen);
    navigate_and_commit(&mut shell, a, "a.example");
    shell.handle(Command::Open);
    let b = active_id(&screen);
    navigate_and_commit(&mut shell, b, "b.example");
    shell.handle(Command::Close(b));
    assert!(shell
        .recently_closed
        .iter()
        .any(|entry| entry.space == studio));
    shell.handle(Command::Open);
    let c = active_id(&screen);
    navigate_and_commit(&mut shell, c, "c.example");
    assert_eq!(last(&screen).spaces[1].tabs, 2);

    let deleted = space_op(&mut shell, SpaceCommand::Delete(studio));
    assert_ne!(deleted.outcome, OperationOutcome::Rejected);
    assert!(shell.spaces.get(studio).is_none());
    assert_eq!(current(&shell), home);
    assert_eq!(active_id(&screen), kept);
    assert!(shell.items.get(a).is_none() && shell.items.get(c).is_none());
    assert!(engine.calls().contains(&format!("close {a}")));
    assert!(!shell
        .recently_closed
        .iter()
        .any(|entry| entry.space == studio));
    assert_eq!(last(&screen).spaces.len(), 1);

    let last_one = space_op(&mut shell, SpaceCommand::Delete(home));
    assert_eq!(last_one.outcome, OperationOutcome::Rejected);
    assert!(shell.items.tab(kept).is_some());
}

#[test]
fn a_tab_moves_to_another_space_and_the_space_it_left_keeps_going() {
    let (mut shell, _engine, screen, _store) = browsed();
    let home = current(&shell);
    let first = active_id(&screen);
    navigate_and_commit(&mut shell, first, "one.example");
    shell.handle(Command::Open);
    let second = active_id(&screen);
    navigate_and_commit(&mut shell, second, "two.example");
    let studio = create(&mut shell, "Studio", SpaceLook::default());
    space_op(&mut shell, SpaceCommand::Switch(home));
    assert_eq!(active_id(&screen), second);

    let moved = space_op(
        &mut shell,
        SpaceCommand::MoveTab {
            tab: second,
            space: studio,
        },
    );
    assert_ne!(moved.outcome, OperationOutcome::Rejected);
    assert_eq!(current(&shell), home);
    assert_eq!(active_id(&screen), first);
    assert!(matches!(
        shell.items.get(second).unwrap().placement,
        Placement::Space { space, .. } if space == studio
    ));
    space_op(&mut shell, SpaceCommand::Switch(studio));
    assert_eq!(active_id(&screen), second, "it is in front where it went");

    shell.handle(Command::SetTabEssential {
        id: second,
        essential: true,
        before: None,
    });
    assert!(matches!(
        shell.items.get(second).unwrap().placement,
        Placement::Favorites { space } if space == studio
    ));
    // An Essential moves as an Essential, to the other space's.
    let favorite = space_op(
        &mut shell,
        SpaceCommand::MoveTab {
            tab: second,
            space: home,
        },
    );
    assert_ne!(favorite.outcome, OperationOutcome::Rejected);
    assert!(matches!(
        shell.items.get(second).unwrap().placement,
        Placement::Favorites { space } if space == home
    ));
}

#[test]
fn keys_and_steps_walk_the_spaces_in_order() {
    let (mut shell, _engine, screen, _store) = browsed();
    let home = current(&shell);
    navigate_and_commit(&mut shell, active_id(&screen), "home.example");
    let second = create(&mut shell, "Two", SpaceLook::default());
    navigate_and_commit(&mut shell, active_id(&screen), "two.example");

    shell.handle(Command::Run("space.select.1".into()));
    assert_eq!(current(&shell), home);
    shell.handle(Command::Run("space.select.2".into()));
    assert_eq!(current(&shell), second);
    let beyond = shell.handle_operation(Command::Run("space.select.3".into()));
    assert_eq!(beyond.outcome, OperationOutcome::NoOp);
    let bogus = shell.handle_operation(Command::Run("space.select.0".into()));
    assert_eq!(bogus.outcome, OperationOutcome::Rejected);

    let end = space_op(&mut shell, SpaceCommand::Step(1));
    assert_eq!(end.outcome, OperationOutcome::NoOp);
    shell.handle(Command::Run("space.previous".into()));
    assert_eq!(current(&shell), home);
    shell.handle(Command::Run("space.next".into()));
    assert_eq!(current(&shell), second);
}

#[test]
fn private_browsing_keeps_one_space_and_profiles_return_to_theirs() {
    let (mut shell, _engine, screen, _store) = browsed();
    let personal = shell.windows.focused().unwrap().profile;
    navigate_and_commit(&mut shell, active_id(&screen), "home.example");
    let studio = create(&mut shell, "Studio", SpaceLook::default());
    navigate_and_commit(&mut shell, active_id(&screen), "studio.example");

    shell.handle(Command::Run("window.newPrivate".into()));
    assert!(shell.private_shown());
    let refused = space_op(
        &mut shell,
        SpaceCommand::Create {
            name: "Secret".into(),
            look: SpaceLook::default(),
        },
    );
    assert_eq!(refused.outcome, OperationOutcome::Rejected);
    assert_eq!(last(&screen).spaces.len(), 1);
    shell.handle(Command::Run("window.closePrivate".into()));
    assert_eq!(current(&shell), studio);

    let created = shell.handle_operation(Command::CreateProfile {
        name: "Work".into(),
        avatar: zephium_core::profiles::ProfileAvatar::initial(ProfileId::from(9)),
        photo: None,
        switch: true,
    });
    assert_ne!(created.outcome, OperationOutcome::Rejected);
    assert_ne!(shell.windows.focused().unwrap().profile, personal);
    shell.handle(Command::SwitchProfile(personal));
    assert_eq!(current(&shell), studio);
}

#[test]
fn a_space_names_up_to_three_tab_icons_for_the_switcher() {
    let (mut shell, _engine, screen, _store) = browsed();
    navigate_and_commit(&mut shell, active_id(&screen), "home.example");
    for _ in 0..4 {
        shell.handle(Command::Open);
    }
    let state = last(&screen);
    assert_eq!(state.spaces[0].tabs, 5);
    assert_eq!(state.spaces[0].previews.len(), 3);
}

#[test]
fn the_tab_arrows_walk_the_sidebar_and_wrap() {
    let (mut shell, _engine, screen, _store) = browsed();
    let first = active_id(&screen);
    navigate_and_commit(&mut shell, first, "one.example");
    shell.handle(Command::Open);
    let second = active_id(&screen);
    assert_ne!(first, second);
    shell.handle(Command::Run("tab.below".into()));
    assert_eq!(active_id(&screen), first);
    shell.handle(Command::Run("tab.above".into()));
    assert_eq!(active_id(&screen), second);
}

fn essentials_of(shell: &Shell, space: SpaceId) -> Vec<ItemId> {
    shell.items.roots(Placement::Favorites { space }).to_vec()
}

/// The Essentials the sidebar is being sent, as tab ids.
fn essentials_shown(screen: &Screen) -> Vec<String> {
    last(screen)
        .nodes
        .iter()
        .filter(|node| node.section == zephium_ipc::SidebarSectionView::Favorites)
        .filter_map(|node| match &node.kind {
            zephium_ipc::SidebarNodeKindView::Tab { tab_id } => Some(tab_id.clone()),
            _ => None,
        })
        .collect()
}

/// A page in the space on screen, made an Essential of it.
fn keep_page(shell: &mut Shell, screen: &Screen, address: &str) -> ItemId {
    shell.handle(Command::Open);
    let id = active_id(screen);
    navigate_and_commit(shell, id, address);
    let kept = shell.handle_operation(Command::SetTabEssential {
        id,
        essential: true,
        before: None,
    });
    assert_ne!(kept.outcome, OperationOutcome::Rejected);
    id
}

#[test]
fn each_space_shows_only_its_own_essentials() {
    let (mut shell, _engine, screen, _store) = browsed();
    let home = current(&shell);
    navigate_and_commit(&mut shell, active_id(&screen), "home.example");
    let mail = keep_page(&mut shell, &screen, "mail.example");
    assert_eq!(essentials_of(&shell, home), [mail]);
    assert_eq!(essentials_shown(&screen), [mail.to_string()]);

    let studio = create(&mut shell, "Studio", SpaceLook::default());
    assert!(
        essentials_shown(&screen).is_empty(),
        "a new space starts without any"
    );
    assert!(essentials_of(&shell, studio).is_empty());
    let board = keep_page(&mut shell, &screen, "board.example");
    assert_eq!(essentials_of(&shell, studio), [board]);
    assert_eq!(essentials_shown(&screen), [board.to_string()]);
    assert_eq!(essentials_of(&shell, home), [mail], "home keeps its own");

    space_op(&mut shell, SpaceCommand::Switch(home));
    assert_eq!(essentials_shown(&screen), [mail.to_string()]);
    space_op(&mut shell, SpaceCommand::Switch(studio));
    assert_eq!(essentials_shown(&screen), [board.to_string()]);
}

#[test]
fn another_spaces_essential_is_out_of_reach_until_its_space_is_shown() {
    let (mut shell, _engine, screen, _store) = browsed();
    let home = current(&shell);
    navigate_and_commit(&mut shell, active_id(&screen), "home.example");
    let mail = keep_page(&mut shell, &screen, "mail.example");
    create(&mut shell, "Studio", SpaceLook::default());
    let before = active_id(&screen);

    shell.handle(Command::Activate(mail));

    assert_eq!(active_id(&screen), before, "it is not this space's");
    assert!(!shell.item_in_focused_scope(mail));
    space_op(&mut shell, SpaceCommand::Switch(home));
    assert_eq!(current(&shell), home);
    assert!(shell.item_in_focused_scope(mail));
}

#[test]
fn closing_an_essential_leaves_the_other_spaces_alone() {
    let (mut shell, _engine, screen, _store) = browsed();
    let home = current(&shell);
    navigate_and_commit(&mut shell, active_id(&screen), "home.example");
    let mail = keep_page(&mut shell, &screen, "mail.example");
    let studio = create(&mut shell, "Studio", SpaceLook::default());
    let board = keep_page(&mut shell, &screen, "board.example");

    shell.handle(Command::Close(board));

    let after = essentials_of(&shell, studio);
    assert_eq!(after.len(), 1, "closing ends the page, not the Essential");
    assert!(!shell.items.tab(after[0]).unwrap().has_view());
    assert_eq!(essentials_of(&shell, home), [mail]);
    assert!(shell.items.tab(mail).unwrap().has_view());
}

#[test]
fn deleting_a_space_takes_its_essentials_and_their_pages_with_it() {
    let (mut shell, engine, screen, _store) = browsed();
    let home = current(&shell);
    navigate_and_commit(&mut shell, active_id(&screen), "home.example");
    let mail = keep_page(&mut shell, &screen, "mail.example");
    let studio = create(&mut shell, "Studio", SpaceLook::default());
    let board = keep_page(&mut shell, &screen, "board.example");
    assert!(shell.items.insert_unloaded_tab(
        ItemId::from(7_001),
        Placement::Favorites { space: studio },
        url::Url::parse("https://idle.example/").unwrap(),
        "Idle",
    ));

    let deleted = space_op(&mut shell, SpaceCommand::Delete(studio));

    assert_ne!(deleted.outcome, OperationOutcome::Rejected);
    assert!(shell.items.get(board).is_none());
    assert!(shell.items.get(ItemId::from(7_001)).is_none());
    assert!(engine.calls().contains(&format!("close {board}")));
    assert!(essentials_of(&shell, studio).is_empty());
    assert_eq!(essentials_of(&shell, home), [mail]);
    assert_eq!(essentials_shown(&screen), [mail.to_string()]);
}

#[test]
fn essentials_stay_with_their_space_across_a_restart() {
    let (mut shell, _engine, screen, store) = browsed();
    let home = current(&shell);
    navigate_and_commit(&mut shell, active_id(&screen), "home.example");
    let mail = keep_page(&mut shell, &screen, "mail.example");
    let studio = create(&mut shell, "Studio", SpaceLook::default());
    let board = keep_page(&mut shell, &screen, "board.example");
    shell.persist();

    let (mut restored, _engine, _screen) = setup_with(store);
    restored.handle(Command::Bootstrap);

    assert_eq!(essentials_of(&restored, home), [mail]);
    assert_eq!(essentials_of(&restored, studio), [board]);
}

#[test]
fn imported_essentials_keep_the_admitted_space_after_switching() {
    let (mut shell, _engine, _screen, _store) = browsed();
    let home = current(&shell);
    let studio = create(&mut shell, "Studio", SpaceLook::default());
    let site = |url: &str| crate::ImportedSite {
        url: url.into(),
        title: url.into(),
    };

    let destination = shell.import_destination().unwrap();
    space_op(&mut shell, SpaceCommand::Switch(home));
    shell.handle(Command::Import {
        destination,
        work: Box::new(crate::ImportWork::Essentials(vec![
            site("https://mail.example/"),
            site("https://board.example/"),
        ])),
        done: crate::ImportCompletion::new(|_| {}),
    });

    assert_eq!(essentials_of(&shell, studio).len(), 2);
    assert!(essentials_of(&shell, home).is_empty());
}
