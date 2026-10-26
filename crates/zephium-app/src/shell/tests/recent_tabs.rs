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
