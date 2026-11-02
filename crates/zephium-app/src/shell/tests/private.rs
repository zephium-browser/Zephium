use super::*;

fn browsed() -> (Shell, Arc<FakeEngine>, Screen, Arc<FakeStore>) {
    let store = Arc::new(FakeStore::default());
    let (mut shell, engine, screen) = setup_with(store.clone());
    shell.handle(Command::Bootstrap);
    (shell, engine, screen, store)
}

fn profile(shell: &Shell) -> ProfileId {
    shell.windows.focused().unwrap().profile
}

#[test]
fn a_private_window_runs_in_its_own_ephemeral_profile() {
    let (mut shell, _engine, screen, _store) = browsed();
    shell.handle(Command::Open);
    let regular = active_id(&screen);
    let regular_profile = profile(&shell);

    shell.handle(Command::Run("window.newPrivate".into()));

    let private = active_id(&screen);
    let private_profile = profile(&shell);
    assert_ne!(private, regular);
    assert_ne!(private_profile, regular_profile);
    assert!(shell.incognito_profile(private_profile));
    assert!(matches!(
        shell.partition_of(private),
        Partition::Ephemeral(owner) if owner == private_profile
    ));
}

#[test]
fn private_pages_leave_no_history_and_the_session_names_the_regular_tabs() {
    let (mut shell, _engine, screen, store) = browsed();
    shell.handle(Command::Open);
    let regular = active_id(&screen);
    navigate_and_commit(&mut shell, regular, "regular.example");
    shell.handle(Command::Run("window.newPrivate".into()));
    let private = active_id(&screen);

    navigate_and_commit(&mut shell, private, "secret.example");
    shell.persist();

    assert!(store
        .recorded_visits
        .lock()
        .unwrap()
        .iter()
        .all(|visit| !visit.url.contains("secret.example")));
    let saved = store.saved.lock().unwrap().clone().expect("a session save");
    assert_eq!(saved.active_item, Some(regular));
    assert!(saved.items.iter().all(|item| item.id != private));
    assert!(saved
        .profiles
        .iter()
        .all(|persisted| persisted.id != profile(&shell)));
}

#[test]
fn closing_the_last_private_tab_erases_the_partition_and_returns() {
    let (mut shell, engine, screen, _store) = browsed();
    shell.handle(Command::Open);
    let regular = active_id(&screen);
    let regular_profile = profile(&shell);
    shell.handle(Command::Run("window.newPrivate".into()));
    let private_profile = profile(&shell);

    shell.handle(Command::Close(active_id(&screen)));

    assert_eq!(profile(&shell), regular_profile);
    assert_eq!(active_id(&screen), regular);
    assert_eq!(engine.erasure_requests(), vec![private_profile]);
    assert!(shell.profiles.get(private_profile).is_none());
}

#[test]
fn the_shortcut_inside_private_opens_another_private_tab() {
    let (mut shell, _engine, screen, _store) = browsed();
    shell.handle(Command::Run("window.newPrivate".into()));
    let first = active_id(&screen);
    let private_profile = profile(&shell);

    shell.handle(Command::Run("window.newPrivate".into()));

    assert_ne!(active_id(&screen), first);
    assert_eq!(profile(&shell), private_profile);
}

#[test]
fn private_tabs_survive_a_visit_to_the_regular_ones() {
    let (mut shell, engine, screen, _store) = browsed();
    shell.handle(Command::Open);
    let regular = active_id(&screen);
    shell.handle(Command::Run("window.newPrivate".into()));
    let private = active_id(&screen);

    shell.open_external(vec!["https://example.com/".into()]);
    assert_ne!(active_id(&screen), private);
    assert_ne!(active_id(&screen), regular);
    shell.handle(Command::Run("window.newPrivate".into()));

    assert_eq!(active_id(&screen), private);
    assert!(engine.erasure_requests().is_empty());
}

#[test]
fn another_applications_link_opens_with_the_regular_tabs() {
    let (mut shell, _engine, _screen, _store) = browsed();
    let regular_profile = profile(&shell);
    shell.handle(Command::Run("window.newPrivate".into()));

    shell.open_external(vec!["https://example.com/".into()]);

    assert_eq!(profile(&shell), regular_profile);
}

#[test]
fn each_private_session_gets_a_new_identity() {
    let (mut shell, engine, screen, _store) = browsed();
    shell.handle(Command::Run("window.newPrivate".into()));
    let first = profile(&shell);
    shell.handle(Command::Close(active_id(&screen)));
    shell.handle(Command::Run("window.newPrivate".into()));

    assert_ne!(profile(&shell), first);
    assert_eq!(engine.erasure_requests(), vec![first]);
}

fn op(shell: &mut Shell, id: &str) -> OperationOutcome {
    shell.handle_operation(Command::Run(id.into())).outcome
}

#[test]
fn going_back_sets_the_private_window_aside_with_its_tabs() {
    let (mut shell, engine, screen, _store) = browsed();
    shell.handle(Command::Open);
    let regular = active_id(&screen);
    let regular_profile = profile(&shell);
    shell.handle(Command::Run("window.newPrivate".into()));
    let private_profile = profile(&shell);
    shell.handle(Command::Run("tab.new".into()));
    let private = active_id(&screen);

    shell.handle(Command::Run("window.showRegular".into()));

    assert!(!shell.private_shown());
    assert_eq!(profile(&shell), regular_profile);
    assert_eq!(active_id(&screen), regular);
    assert!(engine.erasure_requests().is_empty());
    assert!(shell.profiles.get(private_profile).is_some());
    let roster = shell.profiles_view().unwrap();
    assert!(!roster.private);
    assert_eq!(roster.private_tabs, Some(2));
    assert_eq!(roster.current, Some(regular_profile.to_string()));

    shell.handle(Command::Run("window.showPrivate".into()));

    assert!(shell.private_shown());
    assert_eq!(profile(&shell), private_profile);
    assert_eq!(active_id(&screen), private);
    assert!(engine.erasure_requests().is_empty());
}

#[test]
fn the_roster_names_the_private_window_from_the_moment_it_exists() {
    let (mut shell, _engine, _screen, _store) = browsed();
    let regular_profile = profile(&shell);
    assert_eq!(shell.profiles_view().unwrap().private_tabs, None);

    shell.handle(Command::Run("window.newPrivate".into()));
    let roster = shell.profiles_view().unwrap();
    assert!(roster.private);
    assert_eq!(roster.private_tabs, Some(1));
    assert_eq!(roster.current, Some(regular_profile.to_string()));
    assert_eq!(shell.private_projected, (true, Some(1)));

    shell.handle(Command::Run("tab.new".into()));
    assert_eq!(shell.private_projected, (true, Some(2)));

    shell.handle(Command::Run("window.showRegular".into()));
    assert_eq!(shell.private_projected, (false, Some(2)));

    shell.handle(Command::Run("window.closePrivate".into()));
    assert_eq!(shell.private_projected, (false, None));
    assert_eq!(shell.profiles_view().unwrap().private_tabs, None);
}

#[test]
fn closing_still_erases_a_private_window_that_was_set_aside() {
    let (mut shell, engine, screen, _store) = browsed();
    shell.handle(Command::Open);
    let regular = active_id(&screen);
    let regular_profile = profile(&shell);
    shell.handle(Command::Run("window.newPrivate".into()));
    let private_profile = profile(&shell);
    shell.handle(Command::Run("window.showRegular".into()));

    shell.handle(Command::Run("window.closePrivate".into()));

    assert_eq!(engine.erasure_requests(), vec![private_profile]);
    assert!(shell.profiles.get(private_profile).is_none());
    assert!(shell.private.is_none());
    assert_eq!(profile(&shell), regular_profile);
    assert_eq!(active_id(&screen), regular);
}

#[test]
fn a_private_window_set_aside_stays_out_of_history_and_the_session() {
    let (mut shell, _engine, screen, store) = browsed();
    shell.handle(Command::Open);
    let regular = active_id(&screen);
    navigate_and_commit(&mut shell, regular, "regular.example");
    shell.handle(Command::Run("window.newPrivate".into()));
    let private = active_id(&screen);
    let private_profile = profile(&shell);
    navigate_and_commit(&mut shell, private, "secret.example");

    shell.handle(Command::Run("window.showRegular".into()));
    shell.persist();

    assert!(store
        .recorded_visits
        .lock()
        .unwrap()
        .iter()
        .all(|visit| !visit.url.contains("secret.example")));
    let saved = store.saved.lock().unwrap().clone().expect("a session save");
    assert_eq!(saved.active_item, Some(regular));
    assert!(saved.items.iter().all(|item| item.id != private));
    assert!(saved
        .profiles
        .iter()
        .all(|persisted| persisted.id != private_profile));
    assert_eq!(
        shell.persisted_scope().1,
        Some(regular),
        "the durable session names the regular tab whichever side shows"
    );
}

#[test]
fn the_toggle_starts_the_private_window_then_alternates_between_the_sides() {
    let (mut shell, engine, screen, _store) = browsed();
    shell.handle(Command::Open);
    let regular = active_id(&screen);
    let regular_profile = profile(&shell);

    assert_eq!(
        op(&mut shell, "window.togglePrivate"),
        OperationOutcome::Deferred
    );
    assert!(shell.private_shown());
    let private = active_id(&screen);
    let private_profile = profile(&shell);
    assert!(shell.incognito_profile(private_profile));

    shell.handle(Command::Run("window.togglePrivate".into()));
    assert!(!shell.private_shown());
    assert_eq!(profile(&shell), regular_profile);
    assert_eq!(active_id(&screen), regular);

    shell.handle(Command::Run("window.togglePrivate".into()));
    assert!(shell.private_shown());
    assert_eq!(profile(&shell), private_profile);
    assert_eq!(active_id(&screen), private);
    assert!(engine.erasure_requests().is_empty());
}

#[test]
fn naming_a_side_that_is_already_shown_or_absent_changes_nothing() {
    let (mut shell, _engine, screen, _store) = browsed();
    let regular = active_id(&screen);
    assert_eq!(op(&mut shell, "window.showRegular"), OperationOutcome::NoOp);
    assert_eq!(op(&mut shell, "window.showPrivate"), OperationOutcome::NoOp);
    assert_eq!(active_id(&screen), regular);

    shell.handle(Command::Run("window.newPrivate".into()));
    assert_eq!(op(&mut shell, "window.showPrivate"), OperationOutcome::NoOp);
    shell.handle(Command::Run("window.showRegular".into()));
    assert_eq!(op(&mut shell, "window.showRegular"), OperationOutcome::NoOp);
    assert!(shell.private.is_some());
}

#[test]
fn the_shortcut_while_set_aside_reveals_the_window_instead_of_starting_another() {
    let (mut shell, engine, screen, _store) = browsed();
    shell.handle(Command::Run("window.newPrivate".into()));
    let private = active_id(&screen);
    let private_profile = profile(&shell);
    shell.handle(Command::Run("window.showRegular".into()));

    shell.handle(Command::Run("window.newPrivate".into()));

    assert!(shell.private_shown());
    assert_eq!(profile(&shell), private_profile);
    assert_eq!(active_id(&screen), private);
    assert!(engine.erasure_requests().is_empty());
}

#[test]
fn a_tab_of_the_window_set_aside_cannot_be_closed_from_the_regular_side() {
    let (mut shell, engine, screen, _store) = browsed();
    shell.handle(Command::Run("window.newPrivate".into()));
    let private = active_id(&screen);
    shell.handle(Command::Run("window.showRegular".into()));

    shell.handle(Command::Close(private));

    assert!(shell.private.is_some());
    assert_eq!(shell.profiles_view().unwrap().private_tabs, Some(1));
    assert!(engine.erasure_requests().is_empty());
}

#[test]
fn going_back_from_a_page_in_front_returns_to_browse_first() {
    let (mut shell, _engine, screen, _store) = browsed();
    let regular = active_id(&screen);
    shell.handle(Command::Run("window.newPrivate".into()));
    shell.handle_operation(Command::ShowBrowserPage(Some(crate::BrowserPage::Settings)));
    assert_eq!(
        shell.active_browser_page(),
        Some(crate::BrowserPage::Settings)
    );

    shell.handle(Command::Run("window.showRegular".into()));

    assert_eq!(shell.active_browser_page(), None);
    assert!(!shell.private_shown());
    assert_eq!(active_id(&screen), regular);
}
