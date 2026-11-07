use super::*;

fn insert_inactive_profile_tab(
    shell: &mut Shell,
    profile: ProfileId,
    seed: u128,
) -> (SpaceId, ItemId) {
    let space = shell
        .spaces
        .iter()
        .find(|space| space.profile == profile)
        .expect("inactive profile owns its test space")
        .id;
    let item = ItemId::from(seed);
    assert!(shell.items.insert_tab(
        item,
        Placement::Space {
            space,
            section: SpaceSection::Today,
        },
    ));
    (space, item)
}

#[test]
fn profile_deletion_completes_once_only_after_native_and_store_phases() {
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
    let (mut shell, engine, _screen, operations) =
        setup_with_operation_log_and_failure(store.clone(), Box::new(|_| {}));
    shell.handle(Command::Bootstrap);
    store.events.lock().unwrap().clear();
    let profile = add_inactive_named_profile(&mut shell, 20_000);
    engine.push_erasure_outcomes([ProfileDataErasureOutcome::Verified]);

    shell.handle(delete_operation("delete-1", profile));

    let completions = operations.lock().unwrap().clone();
    assert_eq!(completions.len(), 1);
    assert_eq!(completions[0].operation_id, "delete-1");
    assert_eq!(completions[0].outcome, OperationOutcome::Applied);
    assert_eq!(
        completions[0].reason,
        OperationReason::ProfileDeletionCompleted
    );
    assert!(shell.profiles.get(profile).is_none());
    assert!(store.pending_deletions.lock().unwrap().is_empty());
    let authorized = store.authorized_sessions.lock().unwrap();
    assert_eq!(authorized.len(), 1);
    assert!(authorized[0]
        .1
        .profiles
        .iter()
        .all(|candidate| candidate.id != profile));
    assert_eq!(
        session::canonicalize(authorized[0].1.clone()),
        authorized[0].1
    );
    assert_eq!(
        store.events.lock().unwrap().as_slice(),
        ["authorize-delete", "finalize-delete"]
    );
    assert_eq!(engine.erasure_requests(), vec![profile]);
}

#[test]
fn profile_with_a_native_view_obligation_is_rejected_before_authorization() {
    let store = Arc::new(FakeStore::default());
    let (mut shell, _engine, _screen, operations) =
        setup_with_operation_log_and_failure(store.clone(), Box::new(|_| {}));
    shell.handle(Command::Bootstrap);
    let profile = add_inactive_named_profile(&mut shell, 20_050);
    let (_space, item) = insert_inactive_profile_tab(&mut shell, profile, 20_052);
    let effects = shell
        .items
        .navigate(item, "https://view-obligation.example/");
    assert!(matches!(effects.as_slice(), [Effect::CreateView { id, .. }] if *id == item));
    assert!(shell.items.tab(item).is_some_and(TabState::has_view));

    shell.handle(delete_operation("delete-with-live-view", profile));

    assert!(!shell.profile_deletion.states.contains_key(&profile));
    assert!(store.authorized_sessions.lock().unwrap().is_empty());
    let completions = operations.lock().unwrap();
    assert_eq!(completions.len(), 1);
    assert_eq!(completions[0].outcome, OperationOutcome::Rejected);
    assert_eq!(
        completions[0].reason,
        OperationReason::ProfileDeletionPolicyRejected
    );
}

#[test]
fn impossible_store_authorization_outcomes_are_terminal_invariants() {
    for (offset, outcome) in [
        ProfileDeletionAuthorizeOutcome::NotRegistered,
        ProfileDeletionAuthorizeOutcome::SessionConflict,
        ProfileDeletionAuthorizeOutcome::InvalidSession,
    ]
    .into_iter()
    .enumerate()
    {
        let store = Arc::new(FakeStore::default());
        store.authorize_outcomes.lock().unwrap().push_back(outcome);
        let failures = Arc::new(Mutex::new(Vec::new()));
        let (mut shell, engine, _screen, operations) =
            setup_with_operation_log_and_failure(store.clone(), {
                let failures = Arc::clone(&failures);
                Box::new(move |failure| failures.lock().unwrap().push(failure))
            });
        shell.handle(Command::Bootstrap);
        let profile = add_inactive_named_profile(&mut shell, 20_300 + offset as u128 * 10);

        shell.handle(delete_operation("delete-store-conflict", profile));

        assert_eq!(
            failures.lock().unwrap().as_slice(),
            &[ShellTerminalFailure::ProfileDeletionInvariant],
            "{outcome:?}"
        );
        assert!(shell.profiles.get(profile).is_some(), "{outcome:?}");
        assert!(
            store.pending_deletions.lock().unwrap().is_empty(),
            "{outcome:?}"
        );
        assert!(!engine
            .calls()
            .iter()
            .any(|call| call == &format!("erase-profile {profile}")));
        assert!(operations.lock().unwrap().is_empty(), "{outcome:?}");
    }
}

#[test]
fn untracked_profile_deletion_command_is_inert() {
    let store = Arc::new(FakeStore::default());
    let (mut shell, engine, _screen) = setup_with(store.clone());
    shell.handle(Command::Bootstrap);
    let profile = add_inactive_named_profile(&mut shell, 20_500);

    shell.handle(Command::DeleteProfile(profile));

    assert!(shell.profiles.get(profile).is_some());
    assert!(store.authorized_sessions.lock().unwrap().is_empty());
    assert!(!engine
        .calls()
        .iter()
        .any(|call| call == &format!("erase-profile {profile}")));
}

#[test]
fn profile_deletion_retry_emits_no_intermediate_or_duplicate_completion() {
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
    let (mut shell, engine, _screen, operations) = setup_with_operation_log(store);
    shell.handle(Command::Bootstrap);
    let profile = add_inactive_named_profile(&mut shell, 21_000);
    engine.push_erasure_outcomes([
        ProfileDataErasureOutcome::TimedOut,
        ProfileDataErasureOutcome::Verified,
    ]);

    shell.handle(delete_operation("delete-retry", profile));
    assert!(operations.lock().unwrap().is_empty());
    assert!(shell.profiles.get(profile).is_none());
    let generation = shell
        .profile_deletion
        .states
        .get(&profile)
        .unwrap()
        .retry_generation;
    shell.handle(Command::ProfileDeletionRetry {
        profile,
        generation,
    });

    let completions = operations.lock().unwrap().clone();
    assert_eq!(completions.len(), 1);
    assert_eq!(completions[0].operation_id, "delete-retry");
    assert_eq!(completions[0].outcome, OperationOutcome::Applied);
}

#[test]
fn duplicate_profile_deletion_is_rejected_without_stealing_original_id() {
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
    let (mut shell, engine, _screen, operations) = setup_with_operation_log(store);
    shell.handle(Command::Bootstrap);
    let profile = add_inactive_named_profile(&mut shell, 22_000);
    engine
        .hold_erasures
        .store(true, std::sync::atomic::Ordering::Release);

    shell.handle(delete_operation("delete-original", profile));
    shell.handle(delete_operation("delete-duplicate", profile));
    {
        let completions = operations.lock().unwrap();
        assert_eq!(completions.len(), 1);
        assert_eq!(completions[0].operation_id, "delete-duplicate");
        assert_eq!(completions[0].outcome, OperationOutcome::Rejected);
        assert_eq!(
            completions[0].reason,
            OperationReason::ProfileDeletionInProgress
        );
    }

    engine.complete_held_erasure(ProfileDataErasureOutcome::Verified);
    shell.handle(Command::ProfileDeletionReady(profile));
    let completions = operations.lock().unwrap().clone();
    assert_eq!(completions.len(), 2);
    assert_eq!(
        completions
            .iter()
            .filter(|completion| completion.operation_id == "delete-original")
            .count(),
        1
    );
}

#[test]
fn uncertain_profile_deletion_rpcs_reconcile_from_the_journal() {
    let store = Arc::new(FakeStore::default());
    store
        .authorize_outcomes
        .lock()
        .unwrap()
        .push_back(ProfileDeletionAuthorizeOutcome::OutcomeUnknown);
    store
        .authorize_unknown_commits
        .store(true, std::sync::atomic::Ordering::Release);
    store
        .finalize_outcomes
        .lock()
        .unwrap()
        .push_back(ProfileDeletionFinalizeOutcome::OutcomeUnknown);
    store
        .finalize_unknown_completes
        .store(true, std::sync::atomic::Ordering::Release);
    let (mut shell, engine, _screen, operations) = setup_with_operation_log(store.clone());
    shell.handle(Command::Bootstrap);
    let profile = add_inactive_named_profile(&mut shell, 23_000);
    engine.push_erasure_outcomes([ProfileDataErasureOutcome::Verified]);

    shell.handle(delete_operation("delete-uncertain", profile));

    let completions = operations.lock().unwrap().clone();
    assert_eq!(completions.len(), 1);
    assert_eq!(completions[0].operation_id, "delete-uncertain");
    assert_eq!(completions[0].outcome, OperationOutcome::Applied);
    assert!(store.pending_deletions.lock().unwrap().is_empty());
}

#[test]
fn uncertain_profile_deletion_reauthorization_rebuilds_the_survivor_snapshot() {
    let store = Arc::new(FakeStore::default());
    store.authorize_outcomes.lock().unwrap().extend([
        ProfileDeletionAuthorizeOutcome::OutcomeUnknown,
        ProfileDeletionAuthorizeOutcome::Authorized,
    ]);
    store
        .finalize_outcomes
        .lock()
        .unwrap()
        .push_back(ProfileDeletionFinalizeOutcome::Completed);
    let (mut shell, engine, screen, operations) =
        setup_with_operation_log_and_failure(store.clone(), Box::new(|_| {}));
    shell.handle(Command::Bootstrap);
    let survivor = active_id(&screen);
    let profile = add_inactive_named_profile(&mut shell, 23_500);
    // Fail the ordered journal read after the first durability-ambiguous
    // authorization. This leaves a real actor-order window in which newer
    // survivor mutations can be accepted before a safe retry.
    store
        .pending_load_failures
        .store(1, std::sync::atomic::Ordering::Release);

    shell.handle(delete_operation("delete-rebuild", profile));
    assert!(operations.lock().unwrap().is_empty());
    navigate_and_commit(&mut shell, survivor, "new-survivor.example");
    engine.push_erasure_outcomes([ProfileDataErasureOutcome::Verified]);
    let generation = shell
        .profile_deletion
        .states
        .get(&profile)
        .unwrap()
        .retry_generation;
    shell.handle(Command::ProfileDeletionRetry {
        profile,
        generation,
    });
    // A stale timer/callback cannot complete the already-removed state a
    // second time.
    shell.handle(Command::ProfileDeletionRetry {
        profile,
        generation,
    });
    shell.handle(Command::ProfileDeletionReady(profile));

    let authorized = store.authorized_sessions.lock().unwrap();
    assert_eq!(authorized.len(), 2);
    let retried = &authorized[1].1;
    assert!(retried
        .profiles
        .iter()
        .all(|candidate| candidate.id != profile));
    assert!(retried.items.iter().any(|item| {
        item.id == survivor
            && matches!(
                &item.kind,
                PersistedKind::Tab { url, .. } if url == "https://new-survivor.example/"
            )
    }));
    let completions = operations.lock().unwrap();
    assert_eq!(
        completions
            .iter()
            .filter(|completion| completion.operation_id == "delete-rebuild")
            .count(),
        1
    );
    assert_eq!(completions[0].outcome, OperationOutcome::Applied);
}

#[test]
fn delayed_authorization_proof_reschedules_newer_survivor_durability() {
    let store = Arc::new(FakeStore::default());
    store
        .authorize_outcomes
        .lock()
        .unwrap()
        .push_back(ProfileDeletionAuthorizeOutcome::OutcomeUnknown);
    store
        .authorize_unknown_commits
        .store(true, std::sync::atomic::Ordering::Release);
    store
        .finalize_outcomes
        .lock()
        .unwrap()
        .push_back(ProfileDeletionFinalizeOutcome::Completed);
    let (mut shell, engine, screen, operations) = setup_with_operation_log(store.clone());
    shell.handle(Command::Bootstrap);
    let survivor = active_id(&screen);
    let profile = add_inactive_named_profile(&mut shell, 23_750);
    store
        .pending_load_failures
        .store(1, std::sync::atomic::Ordering::Release);

    shell.handle(delete_operation("delete-post-barrier", profile));
    navigate_and_commit(&mut shell, survivor, "post-barrier.example");
    engine.push_erasure_outcomes([ProfileDataErasureOutcome::Verified]);
    let generation = shell
        .profile_deletion
        .states
        .get(&profile)
        .unwrap()
        .retry_generation;
    shell.handle(Command::ProfileDeletionRetry {
        profile,
        generation,
    });

    let persisted = store.saved.lock().unwrap().clone().unwrap();
    assert!(persisted
        .profiles
        .iter()
        .all(|candidate| candidate.id != profile));
    assert!(persisted.items.iter().any(|item| {
        item.id == survivor
            && matches!(
                &item.kind,
                PersistedKind::Tab { url, .. } if url == "https://post-barrier.example/"
            )
    }));
    let completions = operations.lock().unwrap();
    assert_eq!(
        completions
            .iter()
            .filter(|completion| completion.operation_id == "delete-post-barrier")
            .count(),
        1
    );
    assert_eq!(completions[0].outcome, OperationOutcome::Applied);
}

#[test]
fn restart_resumes_journaled_native_erasure_before_creating_views() {
    let store = Arc::new(FakeStore::default());
    store
        .settings
        .lock()
        .unwrap()
        .insert("tabs.startup".into(), "continue".into());
    store
        .authorize_outcomes
        .lock()
        .unwrap()
        .push_back(ProfileDeletionAuthorizeOutcome::Authorized);
    let (mut first, first_engine, screen, first_operations) =
        setup_with_operation_log(store.clone());
    first.handle(Command::Bootstrap);
    let active = active_id(&screen);
    navigate_and_commit(&mut first, active, "survivor.example");
    let profile = add_inactive_named_profile(&mut first, 24_000);
    first_engine.push_erasure_outcomes([ProfileDataErasureOutcome::Failed]);
    first.handle(delete_operation("delete-before-crash", profile));
    assert!(first_operations.lock().unwrap().is_empty());
    assert_eq!(store.pending_deletions.lock().unwrap().len(), 1);
    drop(first);

    store
        .finalize_outcomes
        .lock()
        .unwrap()
        .push_back(ProfileDeletionFinalizeOutcome::Completed);
    let (mut restarted, engine, _screen) = setup_with(store.clone());
    engine.push_erasure_outcomes([ProfileDataErasureOutcome::Verified]);
    restarted.handle(Command::Bootstrap);

    let calls = engine.calls();
    let erase = calls
        .iter()
        .position(|call| call == &format!("erase-profile {profile}"))
        .unwrap();
    let first_create = calls
        .iter()
        .position(|call| call.starts_with("create "))
        .unwrap();
    assert!(erase < first_create);
    assert!(store.pending_deletions.lock().unwrap().is_empty());
    assert!(restarted.profile_deletion.states.is_empty());
}

#[test]
fn restart_with_native_proof_skips_engine_and_finishes_local_purge() {
    let store = Arc::new(FakeStore::default());
    let default_profile = ProfileId::from(25_000);
    let default_space = SpaceId::from(25_001);
    *store.saved.lock().unwrap() = Some(SessionState {
        profiles: vec![PersistedProfile {
            id: default_profile,
            name: "Personal".into(),
            kind: ProfileKind::Default,
        }],
        spaces: vec![PersistedSpace {
            id: default_space,
            profile: default_profile,
            name: "Space".into(),
            look: Default::default(),
        }],
        active_space: Some(default_space),
        ..SessionState::default()
    });
    let removed = ProfileId::from(25_002);
    store
        .pending_deletions
        .lock()
        .unwrap()
        .push(PendingProfileDeletion {
            profile: removed,
            native_erasure_verified: true,
        });
    store
        .finalize_outcomes
        .lock()
        .unwrap()
        .push_back(ProfileDeletionFinalizeOutcome::Completed);
    let (mut shell, engine, _screen) = setup_with(store.clone());

    shell.handle(Command::Bootstrap);

    assert!(!engine
        .calls()
        .iter()
        .any(|call| call == &format!("erase-profile {removed}")));
    assert!(store.pending_deletions.lock().unwrap().is_empty());
}

#[test]
fn recovered_finalize_not_authorized_is_a_terminal_invariant() {
    let store = Arc::new(FakeStore::default());
    let default_profile = ProfileId::from(25_300);
    let default_space = SpaceId::from(25_301);
    *store.saved.lock().unwrap() = Some(SessionState {
        profiles: vec![PersistedProfile {
            id: default_profile,
            name: "Personal".into(),
            kind: ProfileKind::Default,
        }],
        spaces: vec![PersistedSpace {
            id: default_space,
            profile: default_profile,
            name: "Space".into(),
            look: Default::default(),
        }],
        active_space: Some(default_space),
        ..SessionState::default()
    });
    let removed = ProfileId::from(25_302);
    store
        .pending_deletions
        .lock()
        .unwrap()
        .push(PendingProfileDeletion {
            profile: removed,
            native_erasure_verified: true,
        });
    store
        .finalize_outcomes
        .lock()
        .unwrap()
        .push_back(ProfileDeletionFinalizeOutcome::NotAuthorized);
    let failures = Arc::new(Mutex::new(Vec::new()));
    let (mut shell, engine, _screen, _operations) =
        setup_with_operation_log_and_failure(store.clone(), {
            let failures = Arc::clone(&failures);
            Box::new(move |failure| failures.lock().unwrap().push(failure))
        });

    shell.handle(Command::Bootstrap);

    assert_eq!(
        failures.lock().unwrap().as_slice(),
        &[ShellTerminalFailure::ProfileDeletionInvariant]
    );
    assert!(matches!(
        shell.profile_deletion.states[&removed].phase,
        ProfileDeletionPhase::FailedClosed
    ));
    assert!(store
        .pending_deletions
        .lock()
        .unwrap()
        .iter()
        .any(|deletion| deletion.profile == removed));
    assert!(!engine
        .calls()
        .iter()
        .any(|call| call.starts_with("create ")));
}

#[test]
fn retryable_store_authorization_refusals_retain_quarantine() {
    for (offset, first) in [
        ProfileDeletionAuthorizeOutcome::NotAdmitted,
        ProfileDeletionAuthorizeOutcome::Failed,
    ]
    .into_iter()
    .enumerate()
    {
        let store = Arc::new(FakeStore::default());
        store
            .authorize_outcomes
            .lock()
            .unwrap()
            .extend([first, ProfileDeletionAuthorizeOutcome::Authorized]);
        store
            .finalize_outcomes
            .lock()
            .unwrap()
            .push_back(ProfileDeletionFinalizeOutcome::Completed);
        let (mut shell, engine, _screen, operations) =
            setup_with_operation_log_and_failure(store.clone(), Box::new(|_| {}));
        shell.handle(Command::Bootstrap);
        let profile = add_inactive_named_profile(&mut shell, 25_400 + offset as u128 * 10);
        engine.push_erasure_outcomes([ProfileDataErasureOutcome::Verified]);

        shell.handle(delete_operation("retry-store-authorization", profile));

        assert!(shell.profile_deletion_quarantines(profile));
        assert!(shell.profiles.get(profile).is_some());
        assert!(store.pending_deletions.lock().unwrap().is_empty());
        assert!(operations.lock().unwrap().is_empty());
        let generation = shell.profile_deletion.states[&profile].retry_generation;

        shell.handle(Command::ProfileDeletionRetry {
            profile,
            generation,
        });

        assert!(!shell.profile_deletion.states.contains_key(&profile));
        assert!(shell.profiles.get(profile).is_none());
        assert!(store.pending_deletions.lock().unwrap().is_empty());
        assert_eq!(operations.lock().unwrap().len(), 1);
    }
}

#[test]
fn profile_deletion_policy_rejects_default_active_private_and_last_profile() {
    let store = Arc::new(FakeStore::default());
    let (mut shell, _engine, _screen, operations) = setup_with_operation_log(store);
    shell.handle(Command::Bootstrap);
    let default = shell.windows.focused().unwrap().profile;

    shell.handle(delete_operation("delete-default", default));
    let private = ProfileId::from(26_000);
    assert!(shell.profiles.insert(Profile {
        id: private,
        name: "Private".into(),
        kind: ProfileKind::Incognito,
    }));
    shell.handle(delete_operation("delete-private", private));

    let completions = operations.lock().unwrap().clone();
    assert_eq!(completions.len(), 2);
    assert!(completions.iter().all(|completion| {
        completion.outcome == OperationOutcome::Rejected
            && completion.reason == OperationReason::ProfileDeletionPolicyRejected
    }));

    let (mut last_only, _engine, _screen) = setup();
    last_only.bootstrapped = true;
    let only = ProfileId::from(26_100);
    assert!(last_only.profiles.insert(Profile {
        id: only,
        name: "Only".into(),
        kind: ProfileKind::Named,
    }));
    assert!(last_only
        .filtered_session_for_profile_deletion(only)
        .is_none());
}

#[derive(Default)]
struct DelayedDeletionNotes {
    release: Mutex<Option<Box<dyn FnOnce() + Send>>>,
}
impl zephium_core::ports::notes::Notes for DelayedDeletionNotes {
    fn call(
        &self,
        _: ProfileId,
        _: zephium_core::notes::NoteCall,
        done: zephium_core::notes::NoteDone,
    ) {
        done(zephium_core::notes::NoteResponse::Done);
    }
    fn release(&self, _: ProfileId, done: Box<dyn FnOnce() + Send>) -> bool {
        *self.release.lock().unwrap() = Some(done);
        true
    }
}

#[test]
fn data_safety_deletion_waits_for_notes_drain_before_native_erasure_and_finalization() {
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
    let (mut shell, engine, _screen, operations) =
        setup_with_operation_log_and_failure(store.clone(), Box::new(|_| {}));
    let notes = Arc::new(DelayedDeletionNotes::default());
    shell.handle(Command::AttachNotes(crate::api::NotesAttachment(
        notes.clone(),
    )));
    shell.handle(Command::Bootstrap);
    store.events.lock().unwrap().clear();
    let profile = add_inactive_named_profile(&mut shell, 89_100);
    engine.push_erasure_outcomes([ProfileDataErasureOutcome::Verified]);
    shell.handle(delete_operation("wait-notes", profile));
    assert!(engine.erasure_requests().is_empty());
    assert!(operations.lock().unwrap().is_empty());
    assert_eq!(
        store.events.lock().unwrap().as_slice(),
        ["authorize-delete"]
    );
    assert!(!store.pending_deletions.lock().unwrap().is_empty());
    notes.release.lock().unwrap().take().unwrap()();
    shell.handle(Command::ProfileDeletionReady(profile));
    assert_eq!(engine.erasure_requests(), vec![profile]);
    assert_eq!(
        operations.lock().unwrap()[0].outcome,
        OperationOutcome::Applied
    );
    assert!(store.pending_deletions.lock().unwrap().is_empty());
}

#[test]
fn missing_notes_drain_proof_expires_and_retries_without_erasing_data() {
    for stale_first in [true, false] {
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
        let (mut shell, engine, _screen, operations) =
            setup_with_operation_log_and_failure(store.clone(), Box::new(|_| {}));
        let notes = Arc::new(DelayedDeletionNotes::default());
        shell.handle(Command::AttachNotes(crate::api::NotesAttachment(
            notes.clone(),
        )));
        shell.handle(Command::Bootstrap);
        let profile = add_inactive_named_profile(&mut shell, 89_200);
        engine.push_erasure_outcomes([ProfileDataErasureOutcome::Verified]);
        shell.handle(delete_operation("expired-notes", profile));
        let stale = notes.release.lock().unwrap().take().unwrap();
        if let ProfileDeletionPhase::NotesInFlight { deadline, .. } = &mut shell
            .profile_deletion
            .states
            .get_mut(&profile)
            .unwrap()
            .phase
        {
            *deadline = std::time::Instant::now();
        } else {
            panic!("deletion must wait for notes drain");
        }
        let generation = shell.profile_deletion.states[&profile].retry_generation;
        shell.handle(Command::ProfileDeletionRetry {
            profile,
            generation,
        });
        // No drain proof leaves deletion pending, with both durable and native data intact.
        assert!(operations.lock().unwrap().is_empty());
        assert!(engine.erasure_requests().is_empty());
        assert!(!store.pending_deletions.lock().unwrap().is_empty());
        let generation = shell.profile_deletion.states[&profile].retry_generation;
        shell.handle(Command::ProfileDeletionRetry {
            profile,
            generation,
        });
        let fresh = notes
            .release
            .lock()
            .unwrap()
            .take()
            .expect("expired drain retries");
        if stale_first {
            stale();
            shell.handle(Command::ProfileDeletionReady(profile));
            assert!(engine.erasure_requests().is_empty());
            fresh();
        } else {
            fresh();
            stale();
        }
        shell.handle(Command::ProfileDeletionReady(profile));
        assert_eq!(engine.erasure_requests(), vec![profile]);
        assert_eq!(operations.lock().unwrap().len(), 1);
    }
}

#[test]
fn restart_with_verified_native_erasure_still_requires_current_notes_release() {
    let store = Arc::new(FakeStore::default());
    let (mut initial, _, _) = setup_with(store.clone());
    initial.handle(Command::Bootstrap);
    initial.persist();
    drop(initial);
    let removed = ProfileId::from(89_300);
    store
        .pending_deletions
        .lock()
        .unwrap()
        .push(PendingProfileDeletion {
            profile: removed,
            native_erasure_verified: true,
        });
    store
        .finalize_outcomes
        .lock()
        .unwrap()
        .push_back(ProfileDeletionFinalizeOutcome::Completed);
    let (mut shell, engine, _screen) = setup_with(store.clone());
    let notes = Arc::new(DelayedDeletionNotes::default());
    shell.handle(Command::AttachNotes(crate::api::NotesAttachment(
        notes.clone(),
    )));
    shell.handle(Command::Bootstrap);
    assert!(!store.pending_deletions.lock().unwrap().is_empty());
    assert!(!store.events.lock().unwrap().contains(&"finalize-delete"));
    notes.release.lock().unwrap().take().unwrap()();
    shell.handle(Command::ProfileDeletionReady(removed));
    assert!(engine.erasure_requests().is_empty());
    assert!(store.pending_deletions.lock().unwrap().is_empty());
}
