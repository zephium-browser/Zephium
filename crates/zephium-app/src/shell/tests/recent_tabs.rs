use super::*;
use crate::api::WorkPaneTarget;

fn stack(shell: &Shell) -> Vec<ItemId> {
    shell.residency.recent.most_recent_first().collect()
}

fn open(shell: &mut Shell, screen: &Screen) -> ItemId {
    shell.handle(Command::Open);
    active_id(screen)
}

#[test]
fn selecting_a_tab_puts_it_on_top() {
    let (mut shell, _engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    let a = active_id(&screen);
    let b = open(&mut shell, &screen);
    let c = open(&mut shell, &screen);

    shell.handle(Command::Activate(a));

    assert_eq!(stack(&shell), vec![a, c, b]);
}

#[test]
fn reselecting_the_selected_tab_keeps_one_entry_on_top() {
    let (mut shell, _engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    let a = active_id(&screen);
    let web = open(&mut shell, &screen);
    navigate_and_commit(&mut shell, web, "reselect.example");
    // Put another tab above the selected one, as the crash replacement did
    // before it pushed, so the re-activation has something to undo.
    shell.residency.recent.push(a);

    let again = shell.handle_operation(Command::Activate(web));
    assert_eq!(again.outcome, OperationOutcome::NoOp);
    assert_eq!(stack(&shell), vec![web, a]);

    shell.handle_operation(Command::ShowBrowserPage(Some(
        crate::BrowserPage::Extensions,
    )));
    let extensions = active_id(&screen);
    shell.residency.recent.push(a);
    let again = shell.handle_operation(Command::Activate(extensions));
    assert_eq!(again.outcome, OperationOutcome::NoOp);
    assert_eq!(stack(&shell), vec![extensions, a, web]);
}

#[test]
fn closing_a_tab_removes_it() {
    let (mut shell, _engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    let a = active_id(&screen);
    let b = open(&mut shell, &screen);
    let c = open(&mut shell, &screen);

    shell.handle(Command::Close(b));
    assert_eq!(stack(&shell), vec![c, a]);

    shell.handle(Command::Close(c));
    let successor = active_id(&screen);
    assert_eq!(stack(&shell), vec![successor]);
    assert_eq!(successor, a);
}

#[test]
fn closing_in_another_space_removes_it() {
    let (mut shell, _engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    let a = active_id(&screen);
    let other = SpaceId::from(9101);
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
    let b = open(&mut shell, &screen);
    assert_eq!(stack(&shell), vec![b, a]);

    shell.close_in_any_space(a);

    assert!(shell.items.get(a).is_none());
    assert_eq!(stack(&shell), vec![b]);
}

#[test]
fn native_child_close_removes_it() {
    let (mut shell, _engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    let source = active_id(&screen);
    navigate_and_commit(&mut shell, source, "opener.example");
    let child = ItemId::generate();
    shell.handle(Command::Engine(EngineEvent::NativeTabOpened {
        id: source,
        child,
        foreground: true,
        adoption: zephium_core::ports::engine::NativeTabAdoption::new(|_| {}),
    }));
    commit_url(&mut shell, child, "https://opener.example/popup");
    shell.handle(Command::Engine(presentation_pending(
        child,
        NavigationPresentationId::from_raw(91),
        "https://opener.example/popup",
    )));
    assert_eq!(stack(&shell), vec![child, source]);

    shell.handle(Command::Engine(EngineEvent::NativeTabCloseRequested {
        id: child,
    }));

    assert!(shell.items.get(child).is_none());
    assert_eq!(stack(&shell), vec![source]);
}

#[test]
fn profile_deletion_removes_every_tab_of_the_profile() {
    let store = Arc::new(FakeStore::default());
    store
        .authorize_outcomes
        .lock()
        .unwrap()
        .push_back(ProfileDeletionAuthorizeOutcome::Authorized);
    store
        .finalize_outcomes
        .lock()
        .unwrap()
        .push_back(ProfileDeletionFinalizeOutcome::Completed);
    let (mut shell, _engine, screen) = setup_with(store);
    shell.handle(Command::Bootstrap);
    let personal = shell.windows.focused().unwrap().profile;
    let kept = active_id(&screen);
    shell.handle_operation(Command::CreateProfile {
        name: "Work".into(),
        avatar: zephium_core::profiles::ProfileAvatar {
            style: zephium_core::profiles::ProfileAvatarStyle::Symbol,
            color: 2,
            symbol: 5,
            revision: 0,
        },
        photo: None,
        switch: false,
    });
    let work = shell
        .profiles
        .iter()
        .map(|profile| profile.id)
        .find(|id| *id != personal)
        .unwrap();
    shell.handle_operation(Command::SwitchProfile(work));
    let first = active_id(&screen);
    let second = open(&mut shell, &screen);
    shell.handle_operation(Command::SwitchProfile(personal));
    assert_eq!(stack(&shell), vec![kept, second, first]);

    shell.handle(delete_operation("delete-recent", work));

    assert!(shell.profiles.get(work).is_none());
    assert_eq!(stack(&shell), vec![kept]);
}

#[test]
fn ending_a_private_session_removes_its_tabs() {
    let (mut shell, _engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    let regular = open(&mut shell, &screen);
    shell.handle(Command::Run("window.newPrivate".into()));
    let first = active_id(&screen);
    let second = open(&mut shell, &screen);
    assert!(stack(&shell).starts_with(&[second, first, regular]));

    shell.handle(Command::Run("window.closePrivate".into()));

    let after = stack(&shell);
    assert!(!after.contains(&first) && !after.contains(&second));
    assert_eq!(after.first(), Some(&regular));
}

#[test]
fn crash_replacement_goes_on_top() {
    let (mut shell, _engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    let first = active_id(&screen);
    navigate_and_commit(&mut shell, first, "first.example");
    let second = open(&mut shell, &screen);
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
    assert_eq!(stack(&shell), vec![second, first]);

    // The selected leaf crashes again inside the retry window.
    shell.handle(Command::Engine(EngineEvent::ProfileProcessExited {
        profile,
        ids: vec![second],
    }));

    assert_eq!(shell.windows.focused().unwrap().active, Some(first));
    assert_eq!(stack(&shell), vec![first, second]);
}

#[test]
fn split_partner_and_work_pane_do_not_push() {
    let (mut shell, _engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    let a = active_id(&screen);
    let b = open(&mut shell, &screen);
    let c = open(&mut shell, &screen);

    shell.handle(Command::SplitWith {
        other: a,
        axis: Axis::Row,
    });
    assert_eq!(stack(&shell), vec![c, b, a]);

    shell.handle(Command::ShowBrowserPage(Some(crate::BrowserPage::Work)));
    let shown = shell.operation_work_pane_show(
        WorkPaneTarget::Tab(b),
        Rect::new(300.0, 120.0, 720.0, 560.0),
    );
    assert_eq!(shown.outcome, OperationOutcome::Deferred);
    assert_eq!(stack(&shell), vec![c, b, a]);
}

#[test]
fn activating_a_split_member_pushes_only_that_member() {
    let (mut shell, engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    let a = active_id(&screen);
    navigate_and_commit(&mut shell, a, "left.example");
    let b = open(&mut shell, &screen);
    navigate_and_commit(&mut shell, b, "right.example");
    shell.handle(Command::SplitWith {
        other: a,
        axis: Axis::Row,
    });
    let x = open(&mut shell, &screen);
    assert_eq!(stack(&shell), vec![x, b, a]);

    shell.handle(Command::Activate(a));

    assert_eq!(engine.last_layout().len(), 2, "the split is back on screen");
    assert_eq!(stack(&shell), vec![a, x, b]);
}

#[test]
fn browser_page_does_not_push_and_deferred_activate_does() {
    let (mut shell, _engine, chrome, screen) = setup_with_async_chrome();
    shell.handle(Command::Bootstrap);
    let a = active_id(&screen);
    let b = open(&mut shell, &screen);

    shell.handle_operation(Command::ShowBrowserPage(Some(crate::BrowserPage::Work)));
    assert_eq!(shell.active_browser_page(), Some(crate::BrowserPage::Work));
    assert_eq!(stack(&shell), vec![b, a]);

    shell.handle_operation(Command::Activate(a));
    assert_eq!(stack(&shell), vec![b, a], "nothing moves before the return");
    let (revision, _) = chrome.complete_browser_return(true);
    shell.browser_chrome_restored(revision, true);

    assert_eq!(active_id(&screen), a);
    assert_eq!(stack(&shell), vec![a, b]);
}

#[test]
fn extensions_tab_rollback_restores_stack() {
    let (mut shell, engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    let a = active_id(&screen);
    let b = open(&mut shell, &screen);
    shell.handle(Command::Activate(a));
    let before = stack(&shell);
    let tabs_before = shell.items.view_ids();
    engine
        .reject_native_dispatch
        .store(true, std::sync::atomic::Ordering::Release);

    let opened = shell.handle_operation(Command::ShowBrowserPage(Some(
        crate::BrowserPage::Extensions,
    )));

    assert_eq!(opened.outcome, OperationOutcome::NativeAdmissionFailed);
    assert_eq!(shell.items.view_ids(), tabs_before);
    assert_eq!(stack(&shell), before);
    assert_eq!(before, vec![a, b]);
}

#[test]
fn show_browser_page_falls_back_to_most_recent_web_tab() {
    let (mut shell, _engine, chrome, screen) = setup_with_async_chrome();
    shell.handle(Command::Bootstrap);
    let _first = active_id(&screen);
    let middle = open(&mut shell, &screen);
    let _last = open(&mut shell, &screen);
    shell.handle(Command::Activate(middle));
    shell.handle_operation(Command::ShowBrowserPage(Some(
        crate::BrowserPage::Extensions,
    )));
    let manager = active_id(&screen);
    assert_eq!(stack(&shell).first(), Some(&manager));

    shell.handle_operation(Command::ShowBrowserPage(None));
    let (revision, _) = chrome.complete_browser_return(true);
    shell.browser_chrome_restored(revision, true);

    assert_eq!(active_id(&screen), middle);
}

// After closing a tab, "Previously active tab": the stack first, Essentials
// included; then, without Essentials, the newest `last_focus`, then the
// newest id.

fn prefer_recent(shell: &mut Shell) {
    shell.handle(Command::SetAppSetting {
        key: "tabs.after-close".into(),
        value: "recent".into(),
    });
}

fn restart(store: Arc<FakeStore>) -> (Shell, Arc<FakeEngine>, Screen) {
    store
        .settings
        .lock()
        .unwrap()
        .insert("tabs.after-close".into(), "recent".into());
    let (mut shell, engine, screen) = setup_with(store);
    shell.handle(Command::Bootstrap);
    (shell, engine, screen)
}

fn make_essential(shell: &mut Shell, id: ItemId) {
    shell.handle(Command::SetTabEssential {
        id,
        essential: true,
        before: None,
    });
    assert!(matches!(
        shell.items.get(id).unwrap().placement,
        Placement::Favorites { .. }
    ));
}

/// A new space, switched to, with the tab it opens in front.
fn new_space(shell: &mut Shell, screen: &Screen) -> (SpaceId, ItemId) {
    let created = shell.handle_operation(Command::Space(Box::new(crate::SpaceCommand::Create {
        name: "Other".into(),
        look: Default::default(),
    })));
    assert_ne!(created.outcome, OperationOutcome::Rejected);
    (shell.windows.focused().unwrap().space, active_id(screen))
}

fn today_in(space: SpaceId) -> Placement {
    Placement::Space {
        space,
        section: SpaceSection::Today,
    }
}

fn active(screen: &Screen) -> Option<ItemId> {
    last(screen)
        .active
        .and_then(|active| ItemId::parse(&active))
}

#[test]
fn after_close_recent_follows_activation_not_split_join() {
    let (mut shell, _engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    prefer_recent(&mut shell);
    let x = active_id(&screen);
    let a = open(&mut shell, &screen);
    navigate_and_commit(&mut shell, a, "left.example");
    let b = open(&mut shell, &screen);
    navigate_and_commit(&mut shell, b, "right.example");
    shell.handle(Command::Activate(x));
    shell.handle(Command::Activate(a));
    shell.handle(Command::SplitWith {
        other: b,
        axis: Axis::Row,
    });

    shell.handle(Command::Close(a));

    assert_eq!(
        active_id(&screen),
        x,
        "joining a split is not an activation"
    );
}

#[test]
fn after_close_recent_ignores_work_pane_tab() {
    let (mut shell, _engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    prefer_recent(&mut shell);
    let x = active_id(&screen);
    navigate_and_commit(&mut shell, x, "x.example");
    let shown = open(&mut shell, &screen);
    navigate_and_commit(&mut shell, shown, "shown.example");
    let y = open(&mut shell, &screen);
    navigate_and_commit(&mut shell, y, "y.example");
    shell.handle(Command::Activate(x));
    shell.handle(Command::ShowBrowserPage(Some(crate::BrowserPage::Work)));
    let pane = shell.operation_work_pane_show(
        WorkPaneTarget::Tab(shown),
        Rect::new(300.0, 120.0, 720.0, 560.0),
    );
    assert_eq!(pane.outcome, OperationOutcome::Deferred);
    shell.handle(Command::ShowBrowserPage(None));
    assert_eq!(active_id(&screen), x);

    shell.handle(Command::Close(x));

    assert_eq!(active_id(&screen), y);
}

#[test]
fn after_close_recent_skips_other_spaces_and_moved_tab() {
    let (mut shell, _engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    prefer_recent(&mut shell);
    let home = shell.windows.focused().unwrap().space;
    let a = active_id(&screen);
    navigate_and_commit(&mut shell, a, "a.example");
    let b = open(&mut shell, &screen);
    navigate_and_commit(&mut shell, b, "b.example");
    let (other, elsewhere) = new_space(&mut shell, &screen);
    navigate_and_commit(&mut shell, elsewhere, "elsewhere.example");
    shell.handle_operation(Command::Space(Box::new(crate::SpaceCommand::Switch(home))));
    assert_eq!(active_id(&screen), b);
    assert_eq!(stack(&shell)[1], elsewhere, "the other space's tab is next");

    shell.handle(Command::Close(b));
    assert_eq!(active_id(&screen), a);

    let c = open(&mut shell, &screen);
    navigate_and_commit(&mut shell, c, "c.example");
    let moved = shell.handle_operation(Command::Space(Box::new(crate::SpaceCommand::MoveTab {
        tab: c,
        space: other,
    })));
    assert_ne!(moved.outcome, OperationOutcome::Rejected);
    assert_eq!(active_id(&screen), a);
    assert_eq!(
        stack(&shell)[..2],
        [a, c],
        "the moved tab stays on the stack"
    );
}

#[test]
fn after_close_recent_without_any_activated_candidate() {
    let store = Arc::new(FakeStore::default());
    let (mut shell, _engine, screen) = setup_with(store.clone());
    shell.handle(Command::Bootstrap);
    let first = active_id(&screen);
    navigate_and_commit(&mut shell, first, "first.example");
    let second = open(&mut shell, &screen);
    navigate_and_commit(&mut shell, second, "second.example");
    let third = open(&mut shell, &screen);
    navigate_and_commit(&mut shell, third, "third.example");
    shell.handle(Command::Activate(first));
    shell.persist();

    let (mut shell, _engine, screen) = restart(store);
    assert_eq!(active_id(&screen), first);
    assert_eq!(stack(&shell), vec![first]);
    shell.handle(Command::Close(first));

    assert_eq!(active(&screen), Some(second.max(third)), "the newest id");
}

#[test]
fn after_close_recent_restored_split_lands_on_other_half() {
    let store = Arc::new(FakeStore::default());
    let (mut shell, _engine, screen) = setup_with(store.clone());
    shell.handle(Command::Bootstrap);
    let a = active_id(&screen);
    navigate_and_commit(&mut shell, a, "left.example");
    let b = open(&mut shell, &screen);
    navigate_and_commit(&mut shell, b, "right.example");
    let _newer = open(&mut shell, &screen);
    shell.handle(Command::Activate(a));
    shell.handle(Command::SplitWith {
        other: b,
        axis: Axis::Row,
    });
    shell.persist();

    let (mut shell, engine, screen) = restart(store);
    assert_eq!(engine.last_layout().len(), 2);
    assert_eq!(active_id(&screen), a);
    shell.handle(Command::Close(a));

    assert_eq!(active_id(&screen), b);
}

#[test]
fn after_close_recent_never_picks_a_closed_essential() {
    let (mut shell, _engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    prefer_recent(&mut shell);
    let profile = shell.windows.focused().unwrap().profile;
    let essential = open(&mut shell, &screen);
    navigate_and_commit(&mut shell, essential, "essential.example");
    make_essential(&mut shell, essential);
    shell.handle(Command::Close(essential));
    let slot = shell.items.roots(Placement::Favorites { profile })[0];
    assert_ne!(slot, essential, "the Essential is kept under a new id");

    let (space, fresh) = new_space(&mut shell, &screen);
    let background = ItemId::from(5);
    assert!(shell.items.insert_tab(background, today_in(space)));
    assert!(slot > background);
    shell.handle(Command::Close(fresh));

    assert_eq!(active_id(&screen), background);
    assert!(!shell.items.tab(slot).unwrap().has_view());
}

#[test]
fn after_close_recent_with_only_essentials_left() {
    let store = Arc::new(FakeStore::default());
    let (mut shell, _engine, screen) = setup_with(store.clone());
    shell.handle(Command::Bootstrap);
    let a = active_id(&screen);
    navigate_and_commit(&mut shell, a, "a.example");
    let essential = open(&mut shell, &screen);
    navigate_and_commit(&mut shell, essential, "essential.example");
    make_essential(&mut shell, essential);
    shell.handle(Command::Activate(a));
    shell.persist();

    let (mut shell, _engine, screen) = restart(store);
    assert_eq!(active_id(&screen), a);
    assert!(!shell.items.tab(essential).unwrap().has_view());
    shell.handle(Command::Close(a));

    assert_eq!(active(&screen), None);
    assert_eq!(shell.windows.focused().unwrap().active, None);
    assert!(!shell.items.tab(essential).unwrap().has_view());
}

#[test]
fn after_close_recent_fallback_prefers_touched_tabs() {
    let (mut shell, _engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    prefer_recent(&mut shell);
    let (space, fresh) = new_space(&mut shell, &screen);
    navigate_and_commit(&mut shell, fresh, "fresh.example");
    let shown = ItemId::from(10);
    assert!(shell.items.insert_tab(shown, today_in(space)));
    navigate_and_commit(&mut shell, shown, "shown.example");
    shell.handle(Command::ShowBrowserPage(Some(crate::BrowserPage::Work)));
    let pane = shell.operation_work_pane_show(
        WorkPaneTarget::Tab(shown),
        Rect::new(300.0, 120.0, 720.0, 560.0),
    );
    assert_eq!(pane.outcome, OperationOutcome::Deferred);
    shell.handle(Command::ShowBrowserPage(None));
    let newer = ItemId::from(u128::MAX - 1);
    assert!(shell.items.insert_tab(newer, today_in(space)));

    shell.handle(Command::Close(fresh));

    assert_eq!(active_id(&screen), shown);
}

#[test]
fn after_close_recent_fallback_skips_touched_essential() {
    let (mut shell, _engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    prefer_recent(&mut shell);
    let profile = shell.windows.focused().unwrap().profile;
    let (space, fresh) = new_space(&mut shell, &screen);
    navigate_and_commit(&mut shell, fresh, "fresh.example");
    let essential = ItemId::from(u128::MAX - 1);
    assert!(shell
        .items
        .insert_tab(essential, Placement::Favorites { profile }));
    navigate_and_commit(&mut shell, essential, "essential.example");
    let background = ItemId::from(5);
    assert!(shell.items.insert_tab(background, today_in(space)));
    shell.handle(Command::SplitWith {
        other: essential,
        axis: Axis::Row,
    });
    assert!(shell.residency.last_focus.contains_key(&essential));

    shell.handle(Command::Close(fresh));

    assert_eq!(active_id(&screen), background);
}

#[test]
fn after_close_recent_reaches_an_activated_essential() {
    let (mut shell, _engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    prefer_recent(&mut shell);
    let essential = open(&mut shell, &screen);
    navigate_and_commit(&mut shell, essential, "essential.example");
    make_essential(&mut shell, essential);
    let b = open(&mut shell, &screen);

    shell.handle(Command::Close(b));

    assert_eq!(active_id(&screen), essential);
}

#[test]
fn after_close_recent_after_rejected_extensions_tab() {
    let (mut shell, engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    prefer_recent(&mut shell);
    let a = active_id(&screen);
    let b = open(&mut shell, &screen);
    shell.handle_operation(Command::ShowBrowserPage(Some(
        crate::BrowserPage::Extensions,
    )));
    let extensions = active_id(&screen);
    shell.handle(Command::Activate(a));
    shell.handle(Command::Activate(b));
    engine
        .reject_native_dispatch
        .store(true, std::sync::atomic::Ordering::Release);
    let reopened = shell.handle_operation(Command::ShowBrowserPage(Some(
        crate::BrowserPage::Extensions,
    )));
    assert_eq!(reopened.outcome, OperationOutcome::NativeAdmissionFailed);
    engine
        .reject_native_dispatch
        .store(false, std::sync::atomic::Ordering::Release);
    assert_eq!(active_id(&screen), b);
    assert_eq!(stack(&shell), vec![b, a, extensions]);

    shell.handle(Command::Close(b));

    assert_eq!(active_id(&screen), a);
}

#[test]
fn after_close_recent_reaches_favorites_and_pinned() {
    let (mut shell, _engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    prefer_recent(&mut shell);
    let profile = shell.windows.focused().unwrap().profile;
    let space = shell.windows.focused().unwrap().space;
    let pinned = Placement::Space {
        space,
        section: SpaceSection::Pinned,
    };
    let favorite = ItemId::generate();
    assert!(shell
        .items
        .insert_tab(favorite, Placement::Favorites { profile }));
    let folder = ItemId::generate();
    assert!(shell.items.insert(zephium_core::item::Item {
        id: folder,
        parent: None,
        placement: pinned,
        kind: ItemKind::Folder {
            name: "Folder".into()
        },
    }));
    let child = ItemId::generate();
    assert!(shell.items.insert_tab(child, pinned));
    let tab = shell.items.tab(child).cloned().unwrap();
    let _ = shell.items.remove(child);
    assert!(shell.items.insert(zephium_core::item::Item {
        id: child,
        parent: Some(folder),
        placement: pinned,
        kind: ItemKind::Tab(tab),
    }));

    shell.handle(Command::Activate(child));
    shell.handle(Command::Activate(favorite));
    let b = open(&mut shell, &screen);
    shell.handle(Command::Close(b));
    assert_eq!(active_id(&screen), favorite);

    shell.handle(Command::Activate(child));
    let c = open(&mut shell, &screen);
    shell.handle(Command::Close(c));
    assert_eq!(active_id(&screen), child);
}
