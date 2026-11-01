use super::*;
use zephium_core::profiles::{ProfileAvatar, ProfileAvatarStyle};

fn browsed() -> (Shell, Arc<FakeEngine>, Screen, Arc<FakeStore>) {
    let store = Arc::new(FakeStore::default());
    let (mut shell, engine, screen) = setup_with(store.clone());
    shell.handle(Command::Bootstrap);
    (shell, engine, screen, store)
}

fn window_profile(shell: &Shell) -> ProfileId {
    shell.windows.focused().unwrap().profile
}

fn symbol(color: u8, symbol: u8) -> ProfileAvatar {
    ProfileAvatar {
        style: ProfileAvatarStyle::Symbol,
        color,
        symbol,
        revision: 0,
    }
}

fn create(shell: &mut Shell, name: &str) -> ProfileId {
    let before: Vec<_> = shell.profiles.iter().map(|profile| profile.id).collect();
    let created = shell.handle_operation(Command::CreateProfile {
        name: name.into(),
        avatar: symbol(2, 5),
        photo: None,
        switch: false,
    });
    assert_eq!(created.outcome, OperationOutcome::Applied);
    shell
        .profiles
        .iter()
        .map(|profile| profile.id)
        .find(|id| !before.contains(id))
        .expect("a new profile")
}

#[test]
fn a_created_profile_is_named_registered_and_drawn() {
    let (mut shell, _engine, _screen, store) = browsed();
    let work = create(&mut shell, "  Work \n");

    let profile = shell.profiles.get(work).unwrap();
    assert_eq!(profile.name, "Work");
    assert_eq!(profile.kind, ProfileKind::Named);
    assert!(shell.spaces.first_for(work).is_some());
    assert!(shell.blocker.profiles.contains_key(&work));
    let saved = store.saved.lock().unwrap().clone().unwrap();
    assert!(saved.profiles.iter().any(|persisted| persisted.id == work));
    assert!(saved.spaces.iter().any(|space| space.profile == work));
    assert_eq!(
        store
            .avatars
            .lock()
            .unwrap()
            .last()
            .map(|(id, avatar, _)| (*id, *avatar)),
        Some((
            work,
            ProfileAvatar {
                revision: 1,
                ..symbol(2, 5)
            }
        ))
    );
    let view = shell.profiles_view().unwrap();
    assert_eq!(view.profiles.len(), 2);
    assert_eq!(
        view.current.as_deref(),
        Some(&*window_profile(&shell).to_string())
    );
    assert_eq!(view.profiles[1].name, "Work");
    assert_eq!(view.profiles[1].avatar.symbol, 5);

    for (name, avatar, photo) in [
        (" ", symbol(0, 0), None),
        ("Bad colour", symbol(10, 0), None),
        ("Bad symbol", symbol(0, 16), None),
        (
            "Photo without bytes",
            ProfileAvatar {
                style: ProfileAvatarStyle::Photo,
                ..symbol(0, 0)
            },
            None,
        ),
        (
            "Bytes without photo",
            symbol(0, 0),
            Some(Arc::new(vec![1; 8])),
        ),
    ] {
        let refused = shell.handle_operation(Command::CreateProfile {
            name: name.into(),
            avatar,
            photo,
            switch: true,
        });
        assert_eq!(refused.outcome, OperationOutcome::Rejected, "{name}");
    }
    assert_eq!(shell.profiles.iter().count(), 2);
}

#[test]
fn switching_shows_the_other_profile_and_returns_to_where_it_left() {
    let (mut shell, engine, screen, store) = browsed();
    let personal = window_profile(&shell);
    let first = active_id(&screen);
    navigate_and_commit(&mut shell, first, "personal.example");
    let work = create(&mut shell, "Work");

    let switched = shell.handle_operation(Command::SwitchProfile(work));
    assert_ne!(switched.outcome, OperationOutcome::Rejected);
    assert_eq!(window_profile(&shell), work);
    let opened = active_id(&screen);
    assert_ne!(opened, first);
    assert_eq!(shell.profile_of_item(opened), Some(work));
    assert!(matches!(
        shell.partition_of(opened),
        Partition::Persistent(owner) if owner == work
    ));
    assert!(engine.calls().contains(&format!("close {first}")));
    assert_eq!(last(&screen).profile.unwrap().id, work.to_string());
    shell.persist();
    let saved = store.saved.lock().unwrap().clone().unwrap();
    let active_space = saved.active_space.unwrap();
    assert!(saved
        .spaces
        .iter()
        .any(|space| space.id == active_space && space.profile == work));

    let again = shell.handle_operation(Command::SwitchProfile(work));
    assert_eq!(again.outcome, OperationOutcome::NoOp);

    shell.handle_operation(Command::SwitchProfile(personal));
    assert_eq!(window_profile(&shell), personal);
    assert_eq!(active_id(&screen), first);
    assert!(shell.filtered_session_for_profile_deletion(work).is_some());
}

#[test]
fn switching_from_a_private_window_leaves_private_first() {
    let (mut shell, _engine, _screen, _store) = browsed();
    let personal = window_profile(&shell);
    let work = create(&mut shell, "Work");
    shell.handle(Command::Run("window.newPrivate".into()));
    assert!(shell.private_shown());
    assert!(shell.profiles_view().unwrap().private);
    assert_eq!(
        shell.profiles_view().unwrap().current,
        Some(personal.to_string())
    );

    shell.handle_operation(Command::SwitchProfile(work));
    assert!(!shell.private_shown());
    assert_eq!(window_profile(&shell), work);
    assert!(shell
        .profiles_view()
        .unwrap()
        .profiles
        .iter()
        .all(|entry| entry.kind != zephium_ipc::ProfileKindView::Incognito));
}

#[test]
fn any_profile_can_be_renamed_and_redrawn_with_a_photo() {
    let (mut shell, _engine, _screen, store) = browsed();
    let work = create(&mut shell, "Work");

    let renamed = shell.handle_operation(Command::RenameProfile {
        profile: work,
        name: "Studio".into(),
    });
    assert_eq!(renamed.outcome, OperationOutcome::Applied);
    assert_eq!(shell.profiles.get(work).unwrap().name, "Studio");
    let cleared = shell.handle_operation(Command::RenameProfile {
        profile: work,
        name: "\u{200f} ".into(),
    });
    assert_eq!(cleared.outcome, OperationOutcome::Rejected);

    let photo = Arc::new(vec![7_u8; 2048]);
    let drawn = shell.handle_operation(Command::SetProfileAvatar {
        profile: work,
        avatar: ProfileAvatar {
            style: ProfileAvatarStyle::Photo,
            ..symbol(0, 0)
        },
        photo: Some(photo.clone()),
    });
    assert_eq!(drawn.outcome, OperationOutcome::Applied);
    let avatar = shell.profile_avatar(work);
    assert_eq!(avatar.style, ProfileAvatarStyle::Photo);
    assert_eq!(avatar.revision, 2);
    let (reply, served) = std::sync::mpsc::sync_channel(1);
    shell.handle(Command::ProfileAvatarPhoto {
        profile: work,
        reply,
    });
    assert_eq!(served.recv().unwrap().as_ref(), Some(&*photo));

    let same = shell.handle_operation(Command::SetProfileAvatar {
        profile: work,
        avatar: symbol(4, 1),
        photo: None,
    });
    assert_eq!(same.outcome, OperationOutcome::Applied);
    let unchanged = shell.handle_operation(Command::SetProfileAvatar {
        profile: work,
        avatar: symbol(4, 1),
        photo: None,
    });
    assert_eq!(unchanged.outcome, OperationOutcome::NoOp);
    let (reply, served) = std::sync::mpsc::sync_channel(1);
    shell.handle(Command::ProfileAvatarPhoto {
        profile: work,
        reply,
    });
    assert_eq!(served.recv().unwrap(), None);
    assert_eq!(store.avatars.lock().unwrap().len(), 3);
}

#[test]
fn creating_from_the_sidebar_also_switches_to_the_new_profile() {
    let (mut shell, _engine, screen, _store) = browsed();
    let personal = window_profile(&shell);
    let created = shell.handle_operation(Command::CreateProfile {
        name: "School".into(),
        avatar: symbol(3, 9),
        photo: None,
        switch: true,
    });
    assert_ne!(created.outcome, OperationOutcome::Rejected);
    let school = window_profile(&shell);
    assert_ne!(school, personal);
    assert_eq!(shell.profiles.get(school).unwrap().name, "School");
    assert_eq!(shell.profile_of_item(active_id(&screen)), Some(school));
    assert_eq!(
        shell.profiles_view().unwrap().current,
        Some(school.to_string())
    );
}

#[test]
fn next_and_previous_profile_cycle_the_roster() {
    let (mut shell, _engine, _screen, _store) = browsed();
    let first = window_profile(&shell);
    shell.handle(Command::Run("profile.next".into()));
    assert_eq!(window_profile(&shell), first);
    let second = create(&mut shell, "Studio");
    shell.handle(Command::Run("profile.next".into()));
    assert_eq!(window_profile(&shell), second);
    shell.handle(Command::Run("profile.next".into()));
    assert_eq!(window_profile(&shell), first);
    shell.handle(Command::Run("profile.previous".into()));
    assert_eq!(window_profile(&shell), second);
}
