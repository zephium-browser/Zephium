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
        bookmark_folder: None,
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
            bookmark_folder: None,
            switch: true,
        });
        assert_eq!(refused.outcome, OperationOutcome::Rejected, "{name}");
    }
    assert_eq!(shell.profiles.iter().count(), 2);
}

#[test]
fn profile_creation_validates_and_persists_the_shared_bookmark_folder() {
    let (mut shell, _engine, _screen, store) = browsed();
    let queue = crate::actor::CommandQueue::new();
    shell.self_queue = Some(queue.clone());
    let current = window_profile(&shell);
    store.bookmark_folders.lock().unwrap().insert(
        current,
        vec![zephium_core::bookmarks::BookmarkFolder {
            id: 41,
            title: "Shared favorites".into(),
        }],
    );
    let before = shell.profiles.iter().count();
    shell.handle(Command::Operation {
        operation_id: "unknown-bookmark-folder".into(),
        command: Box::new(Command::CreateProfile {
            name: "Unknown folder".into(),
            avatar: symbol(2, 5),
            photo: None,
            bookmark_folder: Some(42),
            switch: false,
        }),
    });
    assert_eq!(shell.profiles.iter().count(), before);
    let completion = queue.try_recv().expect("async folder validation");
    assert!(matches!(
        completion,
        Command::ProfileBookmarkFolderValidated { .. }
    ));
    shell.handle(completion);
    assert_eq!(shell.profiles.iter().count(), before);

    shell.handle(Command::Operation {
        operation_id: "known-bookmark-folder".into(),
        command: Box::new(Command::CreateProfile {
            name: "Shared".into(),
            avatar: symbol(2, 5),
            photo: None,
            bookmark_folder: Some(41),
            switch: false,
        }),
    });
    assert_eq!(shell.profiles.iter().count(), before);
    shell.handle(queue.try_recv().expect("async folder validation"));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
    let mut published = None;
    while published.is_none() && std::time::Instant::now() < deadline {
        if let Some(command @ Command::ProfileCreationPublished { .. }) = queue.try_recv() {
            published = Some(command);
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    shell.handle(published.expect("async profile publication result"));
    let profile = shell
        .profiles
        .iter()
        .find(|profile| profile.name == "Shared")
        .unwrap();
    assert_eq!(shell.profiles.bookmark_folder(profile.id), Some(41));
    assert_eq!(
        store
            .saved
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .shared_bookmark_folders,
        vec![zephium_core::session::PersistedBookmarkFolder {
            profile: profile.id,
            folder: 41,
        }]
    );
}

#[test]
fn profile_creation_rechecks_its_source_after_async_bookmark_folder_read() {
    let (mut shell, _engine, _screen, store) = browsed();
    let queue = crate::actor::CommandQueue::new();
    shell.self_queue = Some(queue.clone());
    let source = window_profile(&shell);
    store.bookmark_folders.lock().unwrap().insert(
        source,
        vec![zephium_core::bookmarks::BookmarkFolder {
            id: 41,
            title: "Shared favorites".into(),
        }],
    );

    shell.handle(Command::Operation {
        operation_id: "stale-bookmark-folder-scope".into(),
        command: Box::new(Command::CreateProfile {
            name: "Stale".into(),
            avatar: symbol(2, 5),
            photo: None,
            bookmark_folder: Some(41),
            switch: false,
        }),
    });
    let completion = queue.try_recv().expect("async folder validation");
    shell.self_queue = None;
    let other = create(&mut shell, "Other");
    assert_ne!(other, source);
    assert_ne!(
        shell.operation_switch_profile(other).outcome,
        OperationOutcome::Rejected
    );

    shell.handle(completion);
    assert!(!shell.profiles.iter().any(|profile| profile.name == "Stale"));
}

#[test]
fn unavailable_folder_lookup_is_not_reported_as_an_invalid_folder_id() {
    let store = Arc::new(FakeStore::default());
    let (mut shell, _engine, _screen, operations) = setup_with_operation_log(store);
    shell.handle(Command::Bootstrap);
    let queue = crate::actor::CommandQueue::new();
    shell.self_queue = Some(queue.clone());
    let before = shell.profiles.iter().count();
    shell.handle(Command::Operation {
        operation_id: "unavailable-folder-list".into(),
        command: Box::new(Command::CreateProfile {
            name: "Shared".into(),
            avatar: symbol(2, 5),
            photo: None,
            bookmark_folder: Some(41),
            switch: false,
        }),
    });
    shell.handle(queue.try_recv().expect("async folder validation"));
    let completions = operations.lock().unwrap();
    assert_eq!(completions.len(), 1);
    assert_eq!(
        completions[0].reason,
        OperationReason::StoreAdmissionRejected
    );
    assert_eq!(shell.profiles.iter().count(), before);
}

#[test]
fn profile_creation_rejects_when_checked_session_commit_fails() {
    let (mut shell, _engine, _screen, store) = browsed();
    *store.checked_save_result.lock().unwrap() = Some(false);
    let before = shell.profiles.iter().count();
    let result = shell.handle_operation(Command::CreateProfile {
        name: "Uncommitted".into(),
        avatar: symbol(2, 5),
        photo: None,
        bookmark_folder: None,
        switch: false,
    });
    assert_eq!(result.outcome, OperationOutcome::Rejected);
    assert_eq!(result.reason, OperationReason::StoreAdmissionRejected);
    assert_eq!(shell.profiles.iter().count(), before);
}

#[test]
fn pending_profile_publication_does_not_block_other_shell_commands() {
    let store = Arc::new(FakeStore::default());
    let (mut shell, _engine, _screen, operations) = setup_with_operation_log(store.clone());
    shell.handle(Command::Bootstrap);
    let queue = crate::actor::CommandQueue::new();
    shell.self_queue = Some(queue.clone());
    let (started_tx, started_rx) = std::sync::mpsc::sync_channel(1);
    let (release_tx, release_rx) = std::sync::mpsc::sync_channel(1);
    *store.checked_save_started.lock().unwrap() = Some(started_tx);
    *store.checked_save_release.lock().unwrap() = Some(release_rx);

    shell.handle(Command::Operation {
        operation_id: "async-profile-create".into(),
        command: Box::new(Command::CreateProfile {
            name: "Async".into(),
            avatar: symbol(2, 5),
            photo: None,
            bookmark_folder: None,
            switch: false,
        }),
    });
    started_rx
        .recv_timeout(std::time::Duration::from_secs(1))
        .expect("checked publication started on its worker");
    assert_eq!(operations.lock().unwrap().len(), 0);
    assert_eq!(shell.pending_profile_creations.len(), 1);

    let renamed = shell.handle_operation(Command::RenameFocusedProfile("Still responsive".into()));
    assert_eq!(renamed.outcome, OperationOutcome::Applied);
    assert!(shell
        .profiles
        .iter()
        .any(|profile| profile.name == "Still responsive"));

    release_tx.send(()).unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
    let mut published = None;
    while published.is_none() && std::time::Instant::now() < deadline {
        if let Some(command @ Command::ProfileCreationPublished { .. }) = queue.try_recv() {
            published = Some(command);
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    shell.handle(published.expect("profile publication callback"));
    assert_eq!(shell.pending_profile_creations.len(), 0);
    assert_eq!(operations.lock().unwrap().len(), 1);
    assert_eq!(
        operations.lock().unwrap()[0].outcome,
        OperationOutcome::Applied
    );
}

#[test]
fn shutdown_does_not_reject_a_profile_whose_publication_is_already_in_flight() {
    let store = Arc::new(FakeStore::default());
    let (mut shell, _engine, _screen, operations) = setup_with_operation_log(store.clone());
    shell.handle(Command::Bootstrap);
    let queue = crate::actor::CommandQueue::new();
    shell.self_queue = Some(queue);
    let (started_tx, started_rx) = std::sync::mpsc::sync_channel(1);
    let (release_tx, release_rx) = std::sync::mpsc::sync_channel(1);
    *store.checked_save_started.lock().unwrap() = Some(started_tx);
    *store.checked_save_release.lock().unwrap() = Some(release_rx);

    shell.handle(Command::Operation {
        operation_id: "shutdown-profile-create".into(),
        command: Box::new(Command::CreateProfile {
            name: "Finishing".into(),
            avatar: symbol(2, 5),
            photo: None,
            bookmark_folder: None,
            switch: false,
        }),
    });
    started_rx
        .recv_timeout(std::time::Duration::from_secs(1))
        .expect("checked publication started");
    release_tx.send(()).unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
    while !store.saved.lock().unwrap().as_ref().is_some_and(|session| {
        session
            .profiles
            .iter()
            .any(|profile| profile.name == "Finishing")
    }) && std::time::Instant::now() < deadline
    {
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    assert!(store.saved.lock().unwrap().as_ref().is_some_and(|session| {
        session
            .profiles
            .iter()
            .any(|profile| profile.name == "Finishing")
    }));

    let (ack, result) = std::sync::mpsc::sync_channel(1);
    shell.handle(Command::Shutdown {
        deadline: std::time::Instant::now() + std::time::Duration::from_secs(1),
        ack,
    });
    assert!(result.recv().is_ok());
    assert!(operations.lock().unwrap().is_empty());
}

#[test]
fn shutdown_completes_pending_folder_profile_creation_and_ignores_late_result() {
    let store = Arc::new(FakeStore::default());
    let (mut shell, _engine, _screen, operations) = setup_with_operation_log(store.clone());
    shell.handle(Command::Bootstrap);
    let queue = crate::actor::CommandQueue::new();
    shell.self_queue = Some(queue.clone());
    let before = shell.profiles.iter().count();
    shell.handle(Command::Operation {
        operation_id: "pending-profile-create".into(),
        command: Box::new(Command::CreateProfile {
            name: "Pending".into(),
            avatar: symbol(2, 5),
            photo: None,
            bookmark_folder: Some(41),
            switch: false,
        }),
    });
    assert_eq!(shell.pending_profile_creations.len(), 1);
    let (ack, result) = std::sync::mpsc::sync_channel(1);
    shell.handle(Command::Shutdown {
        deadline: std::time::Instant::now() + std::time::Duration::from_secs(1),
        ack,
    });
    assert!(result.recv().is_ok());
    let completions = operations.lock().unwrap();
    assert_eq!(completions.len(), 1);
    assert_eq!(completions[0].operation_id, "pending-profile-create");
    assert_eq!(completions[0].outcome, OperationOutcome::Rejected);
    drop(completions);

    let late = queue.try_recv().expect("store validation result");
    shell.handle(late);
    assert_eq!(shell.profiles.iter().count(), before);
    assert_eq!(operations.lock().unwrap().len(), 1);
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
fn a_profile_tombstone_clears_only_that_profiles_destination_entries() {
    let destination = "https://start.example/";
    let (mut shell, _, screen, _) = browsed();
    shell.handle(Command::SetAppSetting {
        key: "tabs.new-page".into(),
        value: destination.into(),
    });
    shell.handle(Command::Open);
    let personal_tab = active_id(&screen);
    commit_url(&mut shell, personal_tab, destination);
    present_committed(&mut shell, personal_tab, destination);
    let personal = window_profile(&shell);
    let work = create(&mut shell, "Work");
    shell.handle_operation(Command::SwitchProfile(work));
    let work_tab = active_id(&screen);
    assert_eq!(shell.profile_of_item(work_tab), Some(work));
    commit_url(&mut shell, work_tab, destination);
    present_committed(&mut shell, work_tab, destination);
    shell.handle_operation(Command::SwitchProfile(personal));
    assert_eq!(window_profile(&shell), personal);
    assert!(shell.new_tab_destinations.contains_key(&personal_tab));
    assert!(shell.new_tab_destinations.contains_key(&work_tab));

    shell.apply_profile_tombstone(work);

    assert!(shell.items.tab(work_tab).is_none());
    assert!(!shell.new_tab_destinations.contains_key(&work_tab));
    assert_eq!(
        shell
            .new_tab_destinations
            .get(&personal_tab)
            .map(url::Url::as_str),
        Some(destination)
    );
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
        bookmark_folder: None,
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
    let third = create(&mut shell, "School");
    shell.handle(Command::Run("profile.next".into()));
    assert_eq!(window_profile(&shell), second);
    shell.handle(Command::Run("profile.next".into()));
    assert_eq!(window_profile(&shell), third);
    shell.handle(Command::Run("profile.next".into()));
    assert_eq!(window_profile(&shell), first);
    shell.handle(Command::Run("profile.previous".into()));
    assert_eq!(window_profile(&shell), third);
    shell.handle(Command::Run("profile.previous".into()));
    assert_eq!(window_profile(&shell), second);
    shell.handle(Command::Run("profile.previous".into()));
    assert_eq!(window_profile(&shell), first);
}

#[test]
fn profile_steps_accumulate_while_browser_return_is_pending() {
    let (mut shell, _engine, chrome, screen) = setup_with_async_chrome();
    shell.handle(Command::Bootstrap);
    let first = window_profile(&shell);
    let second = create(&mut shell, "Studio");
    let third = create(&mut shell, "School");
    let page = active_id(&screen);
    shell.handle(Command::Navigate {
        id: page,
        input: "example.com".into(),
    });
    shell.handle(Command::Engine(EngineEvent::UrlChanged {
        id: page,
        url: "https://example.com/".into(),
    }));
    present_committed(&mut shell, page, "https://example.com/");
    shell.handle(Command::ShowBrowserPage(Some(crate::BrowserPage::Settings)));

    for target in [second, third] {
        let result = shell.handle_operation(Command::Run("profile.next".into()));
        assert_eq!(result.outcome, OperationOutcome::Deferred);
        assert_eq!(window_profile(&shell), first);
        assert!(matches!(
            shell.browser_after_return.as_deref(),
            Some(Command::SwitchProfile(queued)) if *queued == target
        ));
    }

    let (revision, _) = chrome.complete_browser_return(true);
    shell.browser_chrome_restored(revision, true);
    assert_eq!(window_profile(&shell), third);
    assert_eq!(shell.active_browser_page(), None);
    assert!(shell.browser_after_return.is_none());
}

#[test]
fn profile_steps_require_a_focused_window() {
    let (mut shell, _engine, _screen) = setup();
    for forward in [true, false] {
        let result = shell.operation_step_profile(forward);
        assert_eq!(result.outcome, OperationOutcome::Rejected);
        assert_eq!(result.reason, OperationReason::NoFocusedWindow);
    }
}

#[test]
fn profile_steps_reject_an_unrostered_current_profile() {
    let (mut shell, _engine, _screen, _store) = browsed();
    let current = window_profile(&shell);
    shell.profiles.remove(current).unwrap();
    for forward in [true, false] {
        let result = shell.operation_step_profile(forward);
        assert_eq!(result.outcome, OperationOutcome::Rejected);
        assert_eq!(result.reason, OperationReason::InvalidScope);
        assert_eq!(window_profile(&shell), current);
    }
}

#[test]
fn profile_steps_leave_private_without_cycling_through_incognito() {
    let (mut shell, _engine, _screen, _store) = browsed();
    let first = window_profile(&shell);
    let second = create(&mut shell, "Studio");
    shell.handle(Command::Run("window.newPrivate".into()));
    assert!(shell.private_shown());
    shell.handle(Command::Run("profile.next".into()));
    assert!(!shell.private_shown());
    assert_eq!(window_profile(&shell), second);
    shell.handle(Command::Run("profile.previous".into()));
    assert_eq!(window_profile(&shell), first);
}
