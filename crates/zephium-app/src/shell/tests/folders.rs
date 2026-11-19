use super::*;
use crate::{FolderCommand, SpaceCommand};
use zephium_core::item::{FolderColor, ItemKind};

fn browsed() -> (Shell, Arc<FakeEngine>, Screen, Arc<FakeStore>) {
    let store = Arc::new(FakeStore::default());
    let (mut shell, engine, screen) = setup_with(store.clone());
    shell.handle(Command::Bootstrap);
    (shell, engine, screen, store)
}

/// Applied, with native work still to come counting as applied.
fn op(shell: &mut Shell, command: Command) -> OperationDisposition {
    let mut done = shell.handle_operation(command);
    if done.outcome == OperationOutcome::Deferred {
        done.outcome = OperationOutcome::Applied;
    }
    done
}

fn folder_op(shell: &mut Shell, command: FolderCommand) -> OperationDisposition {
    op(shell, Command::Folder(Box::new(command)))
}

fn space(shell: &Shell) -> SpaceId {
    shell.windows.focused().unwrap().space
}

fn today(shell: &Shell) -> Placement {
    Placement::Space {
        space: space(shell),
        section: SpaceSection::Today,
    }
}

/// Opens `pages` loaded tabs after the first one, which shows `first`.
fn tabs(shell: &mut Shell, screen: &Screen, pages: &[&str]) -> Vec<ItemId> {
    let mut ids = vec![active_id(screen)];
    navigate_and_commit(shell, ids[0], pages[0]);
    for page in &pages[1..] {
        shell.handle(Command::Open);
        let id = active_id(screen);
        navigate_and_commit(shell, id, page);
        ids.push(id);
    }
    ids
}

fn group(shell: &mut Shell, name: &str, tabs: Vec<ItemId>) -> ItemId {
    let made = folder_op(
        shell,
        FolderCommand::Create {
            name: name.into(),
            color: None,
            tabs,
            parent: None,
            before: None,
        },
    );
    assert_eq!(made.outcome, OperationOutcome::Applied, "{made:?}");
    *shell
        .items
        .roots(today(shell))
        .iter()
        .find(|id| {
            matches!(&shell.items.get(**id).unwrap().kind,
                ItemKind::Folder { name: made, .. } if made == name)
        })
        .expect("folder")
}

fn folder_view(state: &ItemsState, id: ItemId) -> SidebarNodeKindView {
    state
        .nodes
        .iter()
        .find(|node| node.id == id.to_string())
        .map(|node| node.kind.clone())
        .expect("projected folder")
}

#[test]
fn tabs_grouped_into_a_folder_take_the_first_ones_place_in_sidebar_order() {
    let (mut shell, _engine, screen, store) = browsed();
    let ids = tabs(
        &mut shell,
        &screen,
        &["a.example", "b.example", "c.example"],
    );
    let folder = group(&mut shell, "Research", vec![ids[2], ids[1]]);

    assert_eq!(shell.items.roots(today(&shell)), [ids[0], folder]);
    assert_eq!(shell.items.children(folder), [ids[1], ids[2]]);
    let state = last(&screen);
    assert_eq!(
        folder_view(&state, folder),
        SidebarNodeKindView::Folder {
            name: "Research".into(),
            color: None,
            collapsed: false,
        }
    );
    let child = state
        .nodes
        .iter()
        .find(|node| node.id == ids[1].to_string())
        .unwrap();
    assert_eq!(child.parent_id, Some(folder.to_string()));

    shell.persist();
    let saved = store.saved.lock().unwrap().clone().unwrap();
    let kept = saved.items.iter().find(|item| item.id == folder).unwrap();
    assert_eq!(kept.kind, PersistedKind::folder("Research"));
    assert_eq!(
        saved
            .items
            .iter()
            .filter(|item| item.parent == Some(folder))
            .count(),
        2
    );
}

#[test]
fn a_folder_is_renamed_coloured_and_folded_and_keeps_that_across_a_restart() {
    let (mut shell, _engine, screen, store) = browsed();
    let ids = tabs(&mut shell, &screen, &["a.example", "b.example"]);
    let folder = group(&mut shell, "F", vec![ids[1]]);
    for (command, outcome) in [
        (
            FolderCommand::Rename {
                id: folder,
                name: "  Trips ".into(),
            },
            OperationOutcome::Applied,
        ),
        (
            FolderCommand::Rename {
                id: folder,
                name: "Trips".into(),
            },
            OperationOutcome::NoOp,
        ),
        (
            FolderCommand::SetColor {
                id: folder,
                color: Some(FolderColor::Lilac),
            },
            OperationOutcome::Applied,
        ),
        (
            FolderCommand::SetCollapsed {
                id: folder,
                collapsed: true,
            },
            OperationOutcome::Applied,
        ),
    ] {
        assert_eq!(folder_op(&mut shell, command).outcome, outcome);
    }
    assert_eq!(
        folder_view(&last(&screen), folder),
        SidebarNodeKindView::Folder {
            name: "Trips".into(),
            color: Some(FolderColor::Lilac),
            collapsed: true,
        }
    );

    shell.persist();
    let saved = store.saved.lock().unwrap().clone().unwrap();
    let restarted = Arc::new(FakeStore::default());
    *restarted.saved.lock().unwrap() = Some(saved);
    let (mut again, _engine, screen) = setup_with(restarted);
    again.handle(Command::Bootstrap);
    assert_eq!(
        folder_view(&last(&screen), folder),
        SidebarNodeKindView::Folder {
            name: "Trips".into(),
            color: Some(FolderColor::Lilac),
            collapsed: true,
        }
    );
    // Restored folder tabs are asleep until opened.
    let child = again.items.children(folder)[0];
    assert!(!again.items.tab(child).unwrap().has_view());
    let view = last(&screen)
        .tabs
        .into_iter()
        .find(|tab| tab.id == child.to_string())
        .unwrap();
    assert!(view.asleep);
}

#[test]
fn closing_a_folder_tab_keeps_it_asleep_once_then_takes_it_out() {
    let (mut shell, _engine, screen, _store) = browsed();
    let ids = tabs(
        &mut shell,
        &screen,
        &["a.example", "b.example", "c.example", "d.example"],
    );
    let folder = group(&mut shell, "Keep", vec![ids[1], ids[2], ids[3]]);
    let closed_before = shell.recently_closed.len();

    // The one in front is d, inside the folder. Closing it keeps it, asleep
    // where it stood, and hands the window to a tab that is still awake.
    assert_eq!(active_id(&screen), ids[3]);
    assert_eq!(
        op(&mut shell, Command::Close(ids[3])).outcome,
        OperationOutcome::Applied
    );
    let held = shell.items.children(folder).to_vec();
    assert_eq!(held.len(), 3);
    assert_eq!(&held[..2], &ids[1..3]);
    let kept = shell.items.tab(held[2]).unwrap();
    assert_eq!(kept.url.as_ref().unwrap().host_str(), Some("d.example"));
    assert!(!kept.has_view());
    assert_eq!(shell.recently_closed.len(), closed_before);
    let next = active_id(&screen);
    assert!(shell.items.tab(next).unwrap().has_view());
    assert_ne!(next, held[2]);

    // Closing it again, asleep, takes it out of the folder for good.
    assert_eq!(
        op(&mut shell, Command::Close(held[2])).outcome,
        OperationOutcome::Applied
    );
    assert_eq!(shell.items.children(folder), &held[..2]);
    assert_eq!(shell.recently_closed.len(), closed_before + 1);

    // Closing the folder's tabs ends every page and keeps every tab.
    assert_eq!(
        folder_op(&mut shell, FolderCommand::CloseTabs(folder)).outcome,
        OperationOutcome::Applied
    );
    let held = shell.items.children(folder).to_vec();
    assert_eq!(held.len(), 2);
    assert!(held
        .iter()
        .all(|id| !shell.items.tab(*id).unwrap().has_view()));
    assert_eq!(active_id(&screen), ids[0]);
    assert_eq!(
        folder_op(&mut shell, FolderCommand::CloseTabs(folder)).outcome,
        OperationOutcome::NoOp
    );
}

#[test]
fn a_deleted_folder_comes_back_whole_and_asleep_from_reopen_closed_tab() {
    let (mut shell, _engine, screen, _store) = browsed();
    let ids = tabs(
        &mut shell,
        &screen,
        &["a.example", "b.example", "c.example"],
    );
    let outer = group(&mut shell, "Outer", vec![ids[1]]);
    let made = folder_op(
        &mut shell,
        FolderCommand::Create {
            name: "Inner".into(),
            color: Some(FolderColor::Sky),
            tabs: vec![],
            parent: Some(outer),
            before: None,
        },
    );
    assert_eq!(made.outcome, OperationOutcome::Applied);
    let inner = shell.items.children(outer)[1];
    assert_eq!(
        folder_op(
            &mut shell,
            FolderCommand::Move {
                id: ids[2],
                parent: Some(inner),
                before: None,
            },
        )
        .outcome,
        OperationOutcome::Applied
    );

    assert_eq!(
        folder_op(&mut shell, FolderCommand::Delete(outer)).outcome,
        OperationOutcome::Applied
    );
    assert_eq!(shell.items.roots(today(&shell)), [ids[0]]);
    assert!(shell.items.get(ids[1]).is_none());
    assert_eq!(shell.recently_closed.len(), 2);

    shell.handle(Command::Run("tab.reopen".into()));
    let roots = shell.items.roots(today(&shell)).to_vec();
    assert_eq!(roots.len(), 2);
    let back = roots[1];
    assert!(matches!(
        &shell.items.get(back).unwrap().kind,
        ItemKind::Folder { name, .. } if name == "Outer"
    ));
    let held = shell.items.children(back).to_vec();
    assert_eq!(held.len(), 2);
    let tab = shell.items.tab(held[0]).unwrap();
    assert_eq!(tab.url.as_ref().unwrap().host_str(), Some("b.example"));
    assert!(!tab.has_view());
    assert!(matches!(
        &shell.items.get(held[1]).unwrap().kind,
        ItemKind::Folder { name, color: Some(FolderColor::Sky), .. } if name == "Inner"
    ));
    assert_eq!(shell.items.subtree_tabs(held[1]).len(), 1);
    // Its tabs leave Recently Closed with it, so they cannot come back twice.
    assert!(shell.recently_closed.is_empty());
    assert_eq!(active_id(&screen), ids[0]);
}

#[test]
fn ungrouping_and_removing_from_a_folder_leave_tabs_open_beside_it() {
    let (mut shell, _engine, screen, _store) = browsed();
    let ids = tabs(
        &mut shell,
        &screen,
        &["a.example", "b.example", "c.example", "d.example"],
    );
    let folder = group(&mut shell, "F", vec![ids[1], ids[2]]);
    assert_eq!(
        folder_op(&mut shell, FolderCommand::RemoveFromFolder(ids[1])).outcome,
        OperationOutcome::Applied
    );
    assert_eq!(
        shell.items.roots(today(&shell)),
        [ids[0], folder, ids[1], ids[3]]
    );
    assert_eq!(
        folder_op(&mut shell, FolderCommand::Ungroup(folder)).outcome,
        OperationOutcome::Applied
    );
    assert_eq!(
        shell.items.roots(today(&shell)),
        [ids[0], ids[2], ids[1], ids[3]]
    );
    assert!(shell.items.get(folder).is_none());
    assert!(shell.items.tab(ids[2]).unwrap().has_view());
}

#[test]
fn folder_requests_outside_the_focused_space_or_its_rules_are_refused() {
    let (mut shell, _engine, screen, _store) = browsed();
    let ids = tabs(&mut shell, &screen, &["a.example", "b.example"]);
    let folder = group(&mut shell, "F", vec![ids[1]]);
    let unknown = ItemId::from(987_654);
    let refused = [
        FolderCommand::Rename {
            id: unknown,
            name: "x".into(),
        },
        // A tab is not a folder.
        FolderCommand::Rename {
            id: ids[0],
            name: "x".into(),
        },
        FolderCommand::SetCollapsed {
            id: ids[0],
            collapsed: true,
        },
        FolderCommand::Move {
            id: folder,
            parent: Some(folder),
            before: None,
        },
        FolderCommand::Move {
            id: ids[0],
            parent: Some(ids[1]),
            before: None,
        },
        FolderCommand::Move {
            id: ids[0],
            parent: Some(folder),
            before: Some(ids[0]),
        },
        FolderCommand::Move {
            id: unknown,
            parent: None,
            before: None,
        },
        FolderCommand::Create {
            name: "x".into(),
            color: None,
            tabs: vec![unknown],
            parent: None,
            before: None,
        },
        FolderCommand::Create {
            name: "x".into(),
            color: None,
            tabs: vec![],
            parent: Some(ids[0]),
            before: None,
        },
        FolderCommand::RemoveFromFolder(ids[0]),
        FolderCommand::Ungroup(ids[0]),
        FolderCommand::Delete(unknown),
        FolderCommand::CloseTabs(ids[0]),
    ];
    for command in refused {
        let label = format!("{command:?}");
        assert_eq!(
            folder_op(&mut shell, command).outcome,
            OperationOutcome::Rejected,
            "{label}"
        );
    }
    assert_eq!(shell.items.roots(today(&shell)), [ids[0], folder]);
    assert_eq!(shell.items.children(folder), [ids[1]]);

    // Another space's folder is out of reach from this one.
    let created = shell.handle_operation(Command::Space(Box::new(SpaceCommand::Create {
        name: "Other".into(),
        look: Default::default(),
    })));
    assert_ne!(created.outcome, OperationOutcome::Rejected);
    assert_eq!(
        folder_op(
            &mut shell,
            FolderCommand::Rename {
                id: folder,
                name: "x".into(),
            },
        )
        .outcome,
        OperationOutcome::Rejected
    );
}

#[test]
fn nesting_stops_at_the_depth_limit_by_standing_beside_the_deepest_folder() {
    let (mut shell, _engine, screen, _store) = browsed();
    let ids = tabs(&mut shell, &screen, &["a.example"]);
    let mut parent = None;
    let mut chain = Vec::new();
    for depth in 0..4 {
        let made = folder_op(
            &mut shell,
            FolderCommand::Create {
                name: format!("L{depth}"),
                color: None,
                tabs: vec![],
                parent,
                before: None,
            },
        );
        assert_eq!(made.outcome, OperationOutcome::Applied);
        if depth == 3 {
            // The fourth asked for inside the third stands beside it instead.
            let beside = shell.items.children(chain[1]);
            assert_eq!(beside.len(), 2);
            assert_eq!(beside[1], chain[2]);
            continue;
        }
        let id = match parent {
            Some(parent) => *shell.items.children(parent).last().unwrap(),
            None => *shell.items.roots(today(&shell)).last().unwrap(),
        };
        if depth < 3 {
            chain.push(id);
            parent = Some(id);
        }
    }
    assert_eq!(
        folder_op(
            &mut shell,
            FolderCommand::Move {
                id: chain[0],
                parent: Some(chain[2]),
                before: None,
            },
        )
        .outcome,
        OperationOutcome::Rejected
    );
    assert_eq!(shell.items.roots(today(&shell)), [ids[0], chain[0]]);
}

#[test]
fn a_folder_moves_to_another_space_with_its_tabs_and_the_window_stays() {
    let (mut shell, _engine, screen, _store) = browsed();
    let home = space(&shell);
    let ids = tabs(&mut shell, &screen, &["a.example", "b.example"]);
    let folder = group(&mut shell, "Move me", vec![ids[1]]);
    let created = shell.handle_operation(Command::Space(Box::new(SpaceCommand::Create {
        name: "Other".into(),
        look: Default::default(),
    })));
    assert_ne!(created.outcome, OperationOutcome::Rejected);
    let other = space(&shell);
    let switched = shell.handle_operation(Command::Space(Box::new(SpaceCommand::Switch(home))));
    assert_ne!(switched.outcome, OperationOutcome::Rejected);
    assert_ne!(
        op(&mut shell, Command::Activate(ids[1])).outcome,
        OperationOutcome::Rejected
    );
    assert_eq!(active_id(&screen), ids[1]);

    assert_eq!(
        folder_op(
            &mut shell,
            FolderCommand::MoveToSpace {
                id: folder,
                space: other,
            },
        )
        .outcome,
        OperationOutcome::Applied
    );
    let moved = shell.items.get(ids[1]).unwrap();
    assert_eq!(
        moved.placement,
        Placement::Space {
            space: other,
            section: SpaceSection::Today
        }
    );
    assert_eq!(moved.parent, Some(folder));
    assert_eq!(space(&shell), home);
    assert_eq!(active_id(&screen), ids[0]);
}

/// Asleep tabs at the end of the space until only `leave` items fit.
fn fill(shell: &mut Shell, leave: usize) -> Vec<ItemId> {
    let place = today(shell);
    let page = url::Url::parse("https://filler.example/").unwrap();
    let mut filler = Vec::new();
    while shell.items.room() > leave {
        let id = ItemId::generate();
        assert!(shell
            .items
            .insert_unloaded_tab_at(id, place, None, None, page.clone(), "Filler"));
        filler.push(id);
    }
    filler
}

fn hosts(shell: &Shell, host: &str) -> usize {
    shell
        .items
        .roots(today(shell))
        .iter()
        .flat_map(|root| {
            if shell.items.is_folder(*root) {
                shell.items.subtree_tabs(*root)
            } else {
                vec![*root]
            }
        })
        .filter(|id| {
            shell
                .items
                .tab(*id)
                .and_then(|tab| tab.url.as_ref())
                .is_some_and(|url| url.host_str() == Some(host))
        })
        .count()
}

fn named_folder(shell: &Shell, name: &str) -> Option<ItemId> {
    shell.items.roots(today(shell)).iter().copied().find(|id| {
        matches!(&shell.items.get(*id).unwrap().kind,
            ItemKind::Folder { name: made, .. } if made == name)
    })
}

#[test]
fn deleting_the_folder_around_the_lone_new_tab_leaves_that_tab_in_its_place() {
    let (mut shell, _engine, screen, _store) = browsed();
    let lone = active_id(&screen);
    assert!(shell.is_untouched_new_tab(lone));
    let folder = group(&mut shell, "Only", vec![lone]);
    assert_eq!(shell.items.children(folder), [lone]);

    assert_eq!(
        folder_op(&mut shell, FolderCommand::Delete(folder)).outcome,
        OperationOutcome::Applied
    );
    assert!(shell.items.get(folder).is_none());
    assert_eq!(shell.items.roots(today(&shell)), [lone]);
    assert_eq!(shell.items.get(lone).unwrap().parent, None);
    assert_eq!(active_id(&screen), lone);
}

#[test]
fn an_empty_deleted_folder_comes_back_from_reopen_closed_tab() {
    let (mut shell, _engine, screen, _store) = browsed();
    let ids = tabs(&mut shell, &screen, &["a.example"]);
    let folder = group(&mut shell, "Empty", vec![]);
    assert_eq!(
        folder_op(&mut shell, FolderCommand::Delete(folder)).outcome,
        OperationOutcome::Applied
    );
    assert_eq!(shell.items.roots(today(&shell)), [ids[0]]);

    shell.handle(Command::Run("tab.reopen".into()));
    let back = named_folder(&shell, "Empty").expect("folder back");
    assert!(shell.items.children(back).is_empty());
    assert_eq!(active_id(&screen), ids[0]);
}

#[test]
fn a_tab_closed_after_a_folder_was_deleted_reopens_before_it() {
    let (mut shell, _engine, screen, _store) = browsed();
    let ids = tabs(
        &mut shell,
        &screen,
        &["a.example", "b.example", "c.example"],
    );
    let folder = group(&mut shell, "Older", vec![ids[1]]);
    assert_eq!(
        folder_op(&mut shell, FolderCommand::Delete(folder)).outcome,
        OperationOutcome::Applied
    );
    assert_eq!(
        op(&mut shell, Command::Close(ids[2])).outcome,
        OperationOutcome::Applied
    );

    shell.handle(Command::Run("tab.reopen".into()));
    assert_eq!(hosts(&shell, "c.example"), 1);
    assert!(named_folder(&shell, "Older").is_none());
    shell.handle(Command::Run("tab.reopen".into()));
    let back = named_folder(&shell, "Older").expect("folder back");
    assert_eq!(shell.items.children(back).len(), 1);
    assert_eq!(hosts(&shell, "b.example"), 1);
}

#[test]
fn a_tab_reopened_on_its_own_does_not_come_back_twice_with_its_folder() {
    let (mut shell, _engine, screen, _store) = browsed();
    let ids = tabs(
        &mut shell,
        &screen,
        &["a.example", "b.example", "c.example"],
    );
    let folder = group(&mut shell, "Pair", vec![ids[1], ids[2]]);
    assert_eq!(
        folder_op(&mut shell, FolderCommand::Delete(folder)).outcome,
        OperationOutcome::Applied
    );
    let record = shell
        .recently_closed
        .iter()
        .find(|entry| entry.url.contains("b.example"))
        .and_then(|entry| entry.session_id)
        .unwrap();
    assert!(matches!(
        shell.operation_reopen_closed_tab(Some(record)).outcome,
        OperationOutcome::Applied | OperationOutcome::Deferred
    ));
    assert_eq!(hosts(&shell, "b.example"), 1);

    shell.handle(Command::Run("tab.reopen".into()));
    let back = named_folder(&shell, "Pair").expect("folder back");
    let held = shell.items.children(back).to_vec();
    assert_eq!(held.len(), 1);
    let tab = shell.items.tab(held[0]).unwrap();
    assert_eq!(tab.url.as_ref().unwrap().host_str(), Some("c.example"));
    assert_eq!(hosts(&shell, "b.example"), 1);
    assert!(shell.recently_closed.is_empty());
}

#[test]
fn a_deleted_folder_waits_while_the_session_has_no_room_for_it() {
    let (mut shell, _engine, screen, _store) = browsed();
    let ids = tabs(
        &mut shell,
        &screen,
        &["a.example", "b.example", "c.example"],
    );
    let folder = group(&mut shell, "Big", vec![ids[1], ids[2]]);
    assert_eq!(
        folder_op(&mut shell, FolderCommand::Delete(folder)).outcome,
        OperationOutcome::Applied
    );
    // The folder and both tabs need three items.
    let filler = fill(&mut shell, 2);

    shell.handle(Command::Run("tab.reopen".into()));
    assert!(named_folder(&shell, "Big").is_none());
    assert_eq!(hosts(&shell, "c.example"), 1);

    for id in filler {
        let _ = shell.items.remove(id);
    }
    shell.handle(Command::Run("tab.reopen".into()));
    let back = named_folder(&shell, "Big").expect("folder back");
    assert_eq!(shell.items.children(back).len(), 1);
    assert_eq!(hosts(&shell, "b.example"), 1);
    assert_eq!(hosts(&shell, "c.example"), 1);
}

#[test]
fn moving_the_front_folder_away_hands_the_window_to_the_tab_after_it() {
    let (mut shell, _engine, screen, _store) = browsed();
    let home = space(&shell);
    let ids = tabs(
        &mut shell,
        &screen,
        &[
            "a.example",
            "b.example",
            "c.example",
            "d.example",
            "e.example",
        ],
    );
    let folder = group(&mut shell, "Away", vec![ids[1], ids[2]]);
    let created = shell.handle_operation(Command::Space(Box::new(SpaceCommand::Create {
        name: "Other".into(),
        look: Default::default(),
    })));
    assert_ne!(created.outcome, OperationOutcome::Rejected);
    let other = space(&shell);
    let switched = shell.handle_operation(Command::Space(Box::new(SpaceCommand::Switch(home))));
    assert_ne!(switched.outcome, OperationOutcome::Rejected);
    assert_ne!(
        op(&mut shell, Command::Activate(ids[2])).outcome,
        OperationOutcome::Rejected
    );

    assert_eq!(
        folder_op(
            &mut shell,
            FolderCommand::MoveToSpace {
                id: folder,
                space: other,
            },
        )
        .outcome,
        OperationOutcome::Applied
    );
    assert_eq!(active_id(&screen), ids[3]);
}

#[test]
fn closing_the_front_folder_tab_of_a_full_session_takes_it_out() {
    let (mut shell, _engine, screen, _store) = browsed();
    let ids = tabs(&mut shell, &screen, &["a.example", "b.example"]);
    let folder = group(&mut shell, "Full", vec![ids[1]]);
    assert_eq!(active_id(&screen), ids[1]);
    let _ = fill(&mut shell, 0);
    assert!(shell.items.is_full());
    let closed_before = shell.recently_closed.len();

    assert_eq!(
        op(&mut shell, Command::Close(ids[1])).outcome,
        OperationOutcome::Applied
    );
    assert!(shell.items.children(folder).is_empty());
    assert_eq!(shell.recently_closed.len(), closed_before + 1);
    assert_ne!(active_id(&screen), ids[1]);
}

#[test]
fn a_deleted_folder_holds_only_the_tabs_recently_closed_still_has() {
    let (mut shell, _engine, screen, _store) = browsed();
    let limit = zephium_core::session::MAX_RECENTLY_CLOSED_TABS;
    let pages: Vec<String> = (0..=limit + 2).map(|n| format!("p{n}.example")).collect();
    let pages: Vec<&str> = pages.iter().map(String::as_str).collect();
    let ids = tabs(&mut shell, &screen, &pages);
    let folder = group(&mut shell, "Large", ids[1..].to_vec());
    assert_eq!(shell.items.children(folder).len(), limit + 2);

    assert_eq!(
        folder_op(&mut shell, FolderCommand::Delete(folder)).outcome,
        OperationOutcome::Applied
    );
    // The first two records were pushed out by the folder's own later ones.
    assert_eq!(shell.held_deleted_tabs(), limit);
    shell.handle(Command::Run("tab.reopen".into()));
    let back = named_folder(&shell, "Large").expect("folder back");
    assert_eq!(shell.items.children(back).len(), limit);
    assert_eq!(hosts(&shell, "p1.example"), 0);
    assert_eq!(hosts(&shell, &format!("p{}.example", limit + 2)), 1);
    assert_eq!(shell.held_deleted_tabs(), 0);
}

#[test]
fn closing_the_front_essential_of_a_full_session_keeps_it_asleep() {
    let (mut shell, _engine, screen, _store) = browsed();
    let ids = tabs(&mut shell, &screen, &["a.example", "kept.example"]);
    let essential = ids[1];
    assert!(matches!(
        shell
            .handle_operation(Command::SetTabEssential {
                id: essential,
                essential: true,
                before: None,
            })
            .outcome,
        OperationOutcome::Applied | OperationOutcome::Deferred
    ));
    assert_eq!(active_id(&screen), essential);
    let _ = fill(&mut shell, 0);

    assert_eq!(
        op(&mut shell, Command::Close(essential)).outcome,
        OperationOutcome::Applied
    );
    let favorites = Placement::Favorites {
        space: space(&shell),
    };
    let kept = shell.items.roots(favorites).to_vec();
    assert_eq!(kept.len(), 1);
    let tab = shell.items.tab(kept[0]).unwrap();
    assert_eq!(tab.url.as_ref().unwrap().host_str(), Some("kept.example"));
    assert!(!tab.has_view());
}

#[test]
fn a_folder_tab_whose_page_cannot_be_loaded_back_goes_to_recently_closed() {
    let (mut shell, _engine, screen, _store) = browsed();
    let ids = tabs(
        &mut shell,
        &screen,
        &["a.example", "b.example", "c.example"],
    );
    let folder = group(&mut shell, "Pages", vec![ids[1], ids[2]]);
    let page = url::Url::parse("chrome-extension://aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa/options.html")
        .unwrap();
    shell.items.set_committed_url(ids[2], page);
    let closed_before = shell.recently_closed.len();

    assert_eq!(
        op(&mut shell, Command::Close(ids[2])).outcome,
        OperationOutcome::Applied
    );
    assert_eq!(shell.items.children(folder), [ids[1]]);
    assert_eq!(shell.recently_closed.len(), closed_before + 1);
}

#[test]
fn a_deleted_folder_reopens_before_a_record_from_an_older_build() {
    let (mut shell, _engine, screen, _store) = browsed();
    let ids = tabs(&mut shell, &screen, &["a.example"]);
    let window = shell.windows.focused().unwrap();
    let (profile, home) = (window.profile, window.space);
    // Read back from an older snapshot: no id and no close time.
    shell
        .recently_closed
        .push(zephium_core::session::PersistedClosedTab {
            profile,
            space: home,
            url: "https://old.example/".into(),
            title: "Old".into(),
            zoom: 1.0,
            session_id: None,
            closed_at_ms: None,
            position: None,
        });
    let folder = group(&mut shell, "Empty", vec![]);
    assert_eq!(
        folder_op(&mut shell, FolderCommand::Delete(folder)).outcome,
        OperationOutcome::Applied
    );

    shell.handle(Command::Run("tab.reopen".into()));
    assert!(named_folder(&shell, "Empty").is_some());
    assert_eq!(hosts(&shell, "old.example"), 0);
    assert_eq!(shell.items.roots(today(&shell))[0], ids[0]);
}
