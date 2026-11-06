use super::*;

#[test]
fn split_shows_both_panes_close_collapses() {
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
    let panes = engine.last_layout();
    assert_eq!(panes.len(), 2);
    assert!(panes.contains(&first.to_string()) && panes.contains(&second.to_string()));

    // closing one pane collapses the split onto the other
    shell.handle(Command::Close(first));
    assert_eq!(engine.last_layout(), vec![second.to_string()]);
}

#[test]
fn split_layout_enforces_native_renderer_ceiling() {
    let (mut shell, engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    let anchor = active_id(&screen);
    shell.handle(Command::Navigate {
        id: anchor,
        input: "anchor.example".into(),
    });

    for n in 0..MAX_VISIBLE_PANES {
        shell.handle(Command::Open);
        let other = active_id(&screen);
        shell.handle(Command::Navigate {
            id: other,
            input: format!("pane{n}.example"),
        });
        shell.handle(Command::Activate(anchor));
        shell.handle(Command::SplitWith {
            other,
            axis: Axis::Row,
        });
    }

    assert_eq!(engine.last_layout().len(), MAX_VISIBLE_PANES);
    assert_eq!(
        shell
            .windows
            .focused()
            .and_then(|window| window.splits.as_ref())
            .map(|tree| tree.tabs().len()),
        Some(MAX_VISIBLE_PANES)
    );
}

#[test]
fn split_group_survives_tab_switches() {
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

    // a fresh tab shows alone without dissolving the group
    shell.handle(Command::Open);
    let third = active_id(&screen);
    shell.handle(Command::Navigate {
        id: third,
        input: "wikipedia.org".into(),
    });
    assert_eq!(engine.last_layout(), vec![third.to_string()]);

    // returning to a member brings the whole group back
    shell.handle(Command::Activate(first));
    let panes = engine.last_layout();
    assert_eq!(panes.len(), 2);
    assert!(panes.contains(&first.to_string()) && panes.contains(&second.to_string()));
}

#[test]
fn items_projection_tracks_retained_split_group_in_native_pane_order() {
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

    assert_eq!(
        last(&screen).split_group.unwrap().members,
        vec![second.to_string(), first.to_string()]
    );

    // The retained Arc-style group remains represented while a non-member is
    // the one native pane currently visible.
    shell.handle(Command::Open);
    let third = active_id(&screen);
    navigate_and_commit(&mut shell, third, "third.example");
    assert_eq!(engine.last_layout(), vec![third.to_string()]);
    assert_eq!(
        last(&screen).split_group.unwrap().members,
        vec![second.to_string(), first.to_string()]
    );

    // Extending the group preserves the native tree's left/top-to-right/bottom
    // traversal order instead of re-sorting members by the flat sidebar list.
    shell.handle(Command::Activate(first));
    shell.handle(Command::SplitWith {
        other: third,
        axis: Axis::Col,
    });
    assert_eq!(
        last(&screen).split_group.unwrap().members,
        vec![second.to_string(), first.to_string(), third.to_string()]
    );
}

#[test]
fn projected_split_group_clears_after_unsplit_or_single_leaf_collapse() {
    let (mut shell, _engine, screen) = setup();
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
    assert!(last(&screen).split_group.is_some());
    shell.handle(Command::Unsplit);
    assert!(last(&screen).split_group.is_none());

    shell.handle(Command::SplitWith {
        other: first,
        axis: Axis::Row,
    });
    assert!(last(&screen).split_group.is_some());
    shell.handle(Command::Close(first));
    assert_eq!(active_id(&screen), second);
    assert!(last(&screen).split_group.is_none());
}

#[test]
fn split_projection_includes_pinned_members_and_their_authoritative_rows() {
    let (mut shell, _engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    let active = active_id(&screen);
    let window = shell.windows.focused().unwrap().id;
    let space = shell.windows.focused().unwrap().space;
    let pinned = ItemId::from(91_001);
    assert!(shell.items.insert_tab(
        pinned,
        Placement::Space {
            space,
            section: SpaceSection::Pinned,
        },
    ));
    shell.windows.get_mut(window).unwrap().splits = Some(Pane::Branch {
        axis: Axis::Row,
        ratio: 0.5,
        a: Box::new(Pane::Leaf(active)),
        b: Box::new(Pane::Leaf(pinned)),
    });

    shell.project_items();

    let state = last(&screen);
    assert_eq!(
        state.split_group,
        Some(SplitGroupView {
            members: vec![active.to_string(), pinned.to_string()],
        })
    );
    assert!(state.tabs.iter().any(|tab| tab.id == pinned.to_string()));
    assert!(state.nodes.iter().any(|node| {
        node.kind
            == (SidebarNodeKindView::Tab {
                tab_id: pinned.to_string(),
            })
            && node.section == SidebarSectionView::Pinned
    }));
}

#[test]
fn asynchronous_split_leaf_creation_failure_collapses_and_cannot_resurrect() {
    let store = Arc::new(FakeStore::default());
    let (mut shell, engine, screen) = setup_with(store.clone());
    shell.handle(Command::Bootstrap);
    let first = active_id(&screen);
    navigate_and_commit(&mut shell, first, "first.example");
    shell.handle(Command::Open);
    let failed = active_id(&screen);
    navigate_and_commit(&mut shell, failed, "failed.example");
    shell.handle(Command::SplitWith {
        other: first,
        axis: Axis::Row,
    });
    assert_eq!(engine.last_layout().len(), 2);

    shell.handle(Command::Engine(EngineEvent::ViewCreationFailed {
        id: failed,
    }));

    let window = shell.windows.focused().unwrap();
    assert_eq!(window.active, Some(first));
    assert!(window
        .splits
        .as_ref()
        .is_none_or(|tree| !tree.contains(failed)));
    assert_eq!(engine.last_layout(), vec![first.to_string()]);
    let SessionLoad::Loaded { state: saved, .. } = store.load_session() else {
        panic!("failed-leaf collapse must be durable");
    };
    assert!(saved
        .splits
        .as_ref()
        .is_none_or(|tree| !tree.contains(failed)));

    // Retrying and activating the failed tab is an explicit single-tab
    // transition; it must not revive the topology that owned its failed
    // native construction.
    shell.handle(Command::Navigate {
        id: failed,
        input: "failed.example".into(),
    });
    shell.handle(Command::Activate(failed));
    assert_eq!(engine.last_layout(), vec![failed.to_string()]);
    assert!(shell
        .windows
        .focused()
        .unwrap()
        .splits
        .as_ref()
        .is_none_or(|tree| !tree.contains(failed)));
}

#[test]
fn synchronous_create_refusal_never_commits_split_or_drop_topology() {
    let (mut shell, engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    let first = active_id(&screen);
    navigate_and_commit(&mut shell, first, "first.example");
    shell.handle(Command::Open);
    let failed = active_id(&screen);
    navigate_and_commit(&mut shell, failed, "failed.example");
    shell.items.view_creation_failed(failed);
    shell.handle(Command::Activate(first));
    engine
        .reject_create_dispatch
        .store(true, std::sync::atomic::Ordering::Release);

    let split = shell.operation_split(failed, Axis::Row);
    assert_eq!(split.outcome, OperationOutcome::NativeAdmissionFailed);
    assert!(shell.windows.focused().unwrap().splits.is_none());
    assert_eq!(engine.last_layout(), vec![first.to_string()]);

    let drop = shell.apply_drop(first, failed, Edge::Right);
    assert_eq!(drop.outcome, OperationOutcome::NativeAdmissionFailed);
    assert!(shell.windows.focused().unwrap().splits.is_none());
    assert_eq!(engine.last_layout(), vec![first.to_string()]);
}

#[test]
fn divider_drag_updates_ratio_and_projects_strips() {
    let store = Arc::new(FakeStore::default());
    let engine = Arc::new(FakeEngine::default());
    let strips: Arc<Mutex<Vec<DividerView>>> = Arc::new(Mutex::new(Vec::new()));
    let screen: Screen = Arc::new(Mutex::new(ItemsState {
        projection_revision: String::new(),
        profile: None,
        spaces: Vec::new(),
        active_space_id: None,
        nodes: Vec::new(),
        tabs: Vec::new(),
        active: None,
        split_group: None,
    }));
    let (sink, strip_sink) = (screen.clone(), strips.clone());
    let mut shell = Shell::new(
        engine,
        store.clone(),
        Arc::new(FakeChrome),
        Box::new(move |p| match p {
            Projection::Layout(l) => *strip_sink.lock().unwrap() = l.dividers,
            p => apply_projection(&mut sink.lock().unwrap(), p),
        }),
    );
    shell.handle(Command::SetWindowSize(Size::new(1200.0, 800.0)));
    shell.handle(Command::Bootstrap);
    let first = active_id(&screen);
    navigate_and_commit(&mut shell, first, "example.com");
    shell.handle(Command::Open);
    let second = active_id(&screen);
    navigate_and_commit(&mut shell, second, "github.com");
    shell.handle(Command::SplitWith {
        other: first,
        axis: Axis::Row,
    });

    let before = strips.lock().unwrap().clone();
    assert_eq!(before.len(), 1);
    assert!(before[0].vertical);

    let (cx, cy) = (before[0].x + before[0].width / 2.0, before[0].y + 10.0);
    shell.handle(Command::DividerGrab { x: cx, y: cy });
    shell.handle(Command::SetWindowSize(Size::new(1600.0, 900.0)));
    assert!(shell.divider.is_some(), "resize preserves pointer capture");
    let resized = strips.lock().unwrap().clone();
    assert_eq!(resized.len(), 1);
    let (resized_x, resized_y) = (resized[0].x + resized[0].width / 2.0, resized[0].y + 10.0);
    shell.handle(Command::DividerRelease {
        x: Some(resized_x - 100.0),
        y: Some(resized_y),
    });

    let after = strips.lock().unwrap().clone();
    assert_eq!(after.len(), 1);
    assert!(
        after[0].x < resized[0].x - 50.0,
        "the captured path is resolved against resized geometry"
    );

    let SessionLoad::Loaded { state: saved, .. } = store.load_session() else {
        panic!("release persists the split")
    };
    let Some(Pane::Branch { ratio, .. }) = saved.splits else {
        panic!("split persisted");
    };
    assert!(ratio < 0.5);
}

#[test]
fn topology_collapse_revokes_a_captured_divider_before_its_path_can_alias() {
    let (mut shell, _engine, screen) = setup();
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
    shell.handle(Command::Open);
    let third = active_id(&screen);
    navigate_and_commit(&mut shell, third, "third.example");
    shell.handle(Command::Activate(first));
    shell.handle(Command::SplitWith {
        other: third,
        axis: Axis::Row,
    });

    // The outer root is [second | (first | third)]. Removing `second`
    // promotes the nested branch to root, where the old empty path would
    // otherwise authorize a different divider.
    let win = shell.windows.focused().unwrap();
    let tree = shell.pane_tree().unwrap();
    let content = layout::compute(win.size, win.mode, win.metrics, true)
        .content
        .unwrap();
    let local = Rect::new(0.0, 0.0, content.width, content.height);
    let outer = split::divider_at_path(&tree, local, win.metrics.gap, &[]).unwrap();
    shell.handle(Command::DividerGrab {
        x: content.x + outer.strip.x + outer.strip.width / 2.0,
        y: content.y + outer.strip.y + 10.0,
    });
    assert!(shell.divider.is_some());

    shell.handle(Command::Close(second));
    assert!(shell.divider.is_none(), "topology mutation revokes capture");
    shell.handle(Command::DividerRelease {
        x: Some(content.x + content.width * 0.8),
        y: Some(content.y + 10.0),
    });

    let Pane::Branch { ratio, a, b, .. } = shell.pane_tree().unwrap() else {
        panic!("the promoted first/third split remains live");
    };
    assert_eq!(ratio, 0.5, "stale root path did not mutate the new root");
    assert_eq!(a.tabs(), vec![first]);
    assert_eq!(b.tabs(), vec![third]);
}

#[test]
fn right_sidebar_split_coordinates_follow_the_page_and_side_changes_revoke_capture() {
    let (mut shell, _engine, screen) = setup();
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
    shell.handle(Command::SetAppSetting {
        key: "sidebar.position".into(),
        value: "right".into(),
    });
    let win = shell.windows.focused().unwrap();
    let tree = shell.pane_tree().unwrap();
    let content = layout::compute(win.size, win.mode, win.metrics, true)
        .content
        .unwrap();
    assert_eq!(content.x, 8.0);
    let local = Rect::new(0.0, 0.0, content.width, content.height);
    let (target, region) = split::layout(&tree, local, win.metrics.gap)[0];
    let drop = shell
        .resolve_drop(
            content.x + region.x + 1.0,
            content.y + region.y + region.height / 2.0,
        )
        .unwrap();
    assert_eq!(drop.tab, target);
    assert_eq!(drop.edge, Edge::Left);
    let divider = split::divider_at_path(&tree, local, win.metrics.gap, &[]).unwrap();
    shell.handle(Command::DividerGrab {
        x: content.x + divider.strip.x + divider.strip.width / 2.0,
        y: content.y + divider.strip.y + 10.0,
    });
    assert!(shell.divider.is_some());
    shell.handle(Command::SetAppSetting {
        key: "sidebar.position".into(),
        value: "left".into(),
    });
    assert!(shell.divider.is_none());
    shell.handle(Command::DividerRelease {
        x: Some(content.x + content.width * 0.8),
        y: Some(content.y + 10.0),
    });
    let Pane::Branch { ratio, .. } = shell.pane_tree().unwrap() else {
        panic!("the split remains live");
    };
    assert_eq!(ratio, 0.5);
}

#[test]
fn native_divider_event_changes_ratio_only_and_rejects_non_finite_state() {
    let store = Arc::new(FakeStore::default());
    let (mut shell, _engine, screen) = setup_with(store.clone());
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
    let window = shell.windows.focused().unwrap().id;
    let mut changed = shell.windows.focused().unwrap().splits.clone().unwrap();
    changed.set_ratio(&[], 0.7);
    shell.handle(Command::Engine(EngineEvent::SplitChanged {
        window,
        tree: changed,
    }));
    let Pane::Branch { ratio, .. } = shell.windows.focused().unwrap().splits.as_ref().unwrap()
    else {
        panic!("split remains a branch");
    };
    assert_eq!(*ratio, 0.7);
    assert_eq!(
        store
            .saved
            .lock()
            .unwrap()
            .as_ref()
            .and_then(|state| state.splits.as_ref())
            .and_then(|tree| match tree {
                Pane::Branch { ratio, .. } => Some(*ratio),
                Pane::Leaf(_) => None,
            }),
        Some(0.7)
    );

    let mut invalid = shell.windows.focused().unwrap().splits.clone().unwrap();
    let Pane::Branch { ratio, .. } = &mut invalid else {
        unreachable!()
    };
    *ratio = f64::NAN;
    shell.handle(Command::Engine(EngineEvent::SplitChanged {
        window,
        tree: invalid,
    }));
    let Pane::Branch { ratio, .. } = shell.windows.focused().unwrap().splits.as_ref().unwrap()
    else {
        unreachable!()
    };
    assert_eq!(*ratio, 0.7);
}

#[test]
fn native_split_update_cannot_ratify_a_cross_space_tree() {
    let (mut shell, engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    let local = active_id(&screen);
    shell.handle(Command::Navigate {
        id: local,
        input: "local.example".into(),
    });
    let (window, profile) = shell
        .windows
        .focused()
        .map(|win| (win.id, win.profile))
        .unwrap();
    let foreign_space = SpaceId::from(9200);
    let foreign = ItemId::from(9201);
    assert!(shell.spaces.insert(Space {
        id: foreign_space,
        profile,
        name: "Foreign".into(),
        look: Default::default(),
    }));
    assert!(shell.items.insert_tab(
        foreign,
        Placement::Space {
            space: foreign_space,
            section: SpaceSection::Today,
        },
    ));

    // Model a corrupted/stale native topology already present in memory.
    // A ratio callback with matching topology must still fail scope
    // validation rather than blessing and persisting the foreign leaf.
    let invalid = Pane::Branch {
        axis: Axis::Row,
        ratio: 0.5,
        a: Box::new(Pane::Leaf(local)),
        b: Box::new(Pane::Leaf(foreign)),
    };
    shell.windows.get_mut(window).unwrap().splits = Some(invalid.clone());
    let mut candidate = invalid;
    candidate.set_ratio(&[], 0.7);
    shell.handle(Command::Engine(EngineEvent::SplitChanged {
        window,
        tree: candidate,
    }));

    let Pane::Branch { ratio, .. } = shell
        .windows
        .get(window)
        .and_then(|win| win.splits.as_ref())
        .unwrap()
    else {
        panic!("test corruption remains unchanged")
    };
    assert_eq!(*ratio, 0.5);
    assert_eq!(engine.last_layout(), vec![local.to_string()]);
}

#[test]
fn a_tab_leaving_a_split_keeps_the_rest_paired() {
    let (mut shell, _engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    let first = active_id(&screen);
    navigate_and_commit(&mut shell, first, "first.example");
    shell.handle(Command::Open);
    let second = active_id(&screen);
    navigate_and_commit(&mut shell, second, "second.example");
    shell.handle(Command::Open);
    let third = active_id(&screen);
    navigate_and_commit(&mut shell, third, "third.example");

    shell.handle(Command::SplitWith {
        other: first,
        axis: Axis::Row,
    });
    shell.handle(Command::SplitWith {
        other: second,
        axis: Axis::Row,
    });
    assert_eq!(last(&screen).split_group.unwrap().members.len(), 3);

    let left = shell.handle_operation(Command::LeaveSplit(first));
    assert_eq!(left.outcome, OperationOutcome::Deferred);
    let members = last(&screen).split_group.unwrap().members;
    assert!(!members.contains(&first.to_string()));
    assert_eq!(members.len(), 2);

    let again = shell.handle_operation(Command::LeaveSplit(first));
    assert_eq!(again.outcome, OperationOutcome::NoOp);

    // Two become one: no split remains.
    shell.handle_operation(Command::LeaveSplit(second));
    assert!(last(&screen).split_group.is_none());
    assert_eq!(active_id(&screen), third);
}
