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
