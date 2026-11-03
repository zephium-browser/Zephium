use super::*;

#[test]
fn native_profile_process_group_ceiling_counts_distinct_profiles_only() {
    let profiles: Vec<_> = (1..=MAX_NATIVE_PROFILE_PROCESS_GROUPS)
        .map(|id| ProfileId::from(id as u128))
        .collect();
    let existing = profiles[0];
    let new = ProfileId::from(10_000);

    assert!(profile_process_group_capacity_allows(
        profiles.iter().copied().chain([existing, existing]),
        existing,
    ));
    assert!(!profile_process_group_capacity_allows(
        profiles.iter().copied().chain([profiles[0]]),
        new,
    ));
    assert!(profile_process_group_capacity_allows(
        profiles[..MAX_NATIVE_PROFILE_PROCESS_GROUPS - 1]
            .iter()
            .copied(),
        new,
    ));
}

#[test]
fn profile_id_never_crosses_durable_and_ephemeral_storage_classes() {
    let durable_first = ProfileId::from(31);
    let ephemeral_first = ProfileId::from(32);
    let mut bindings = HashMap::new();

    assert!(bind_profile_persistence_class(
        &mut bindings,
        Partition::Default(durable_first)
    ));
    assert!(bind_profile_persistence_class(
        &mut bindings,
        Partition::Persistent(durable_first)
    ));
    assert!(!bind_profile_persistence_class(
        &mut bindings,
        Partition::Ephemeral(durable_first)
    ));
    assert!(bind_profile_persistence_class(
        &mut bindings,
        Partition::Default(durable_first)
    ));

    assert!(bind_profile_persistence_class(
        &mut bindings,
        Partition::Ephemeral(ephemeral_first)
    ));
    assert!(!bind_profile_persistence_class(
        &mut bindings,
        Partition::Default(ephemeral_first)
    ));
    assert!(!bind_profile_persistence_class(
        &mut bindings,
        Partition::Persistent(ephemeral_first)
    ));
    assert!(bind_profile_persistence_class(
        &mut bindings,
        Partition::Ephemeral(ephemeral_first)
    ));
}

#[test]
fn profile_scoped_values_reuse_within_profile_and_isolate_profiles() {
    let first = ProfileId::from(41);
    let second = ProfileId::from(42);
    let mut values = HashMap::new();
    let mut next = 100usize;

    let first_value = profile_scoped_value(&mut values, first, || {
        next += 1;
        Ok::<_, ()>(next)
    })
    .unwrap();
    let first_again = profile_scoped_value(&mut values, first, || {
        next += 1;
        Ok::<_, ()>(next)
    })
    .unwrap();
    let second_value = profile_scoped_value(&mut values, second, || {
        next += 1;
        Ok::<_, ()>(next)
    })
    .unwrap();

    assert_eq!(first_value, first_again);
    assert_ne!(first_value, second_value);
    assert_eq!(
        next, 102,
        "the same profile must not invoke the factory twice"
    );
    assert!(profile_value_is_isolated(
        &values,
        first,
        &first_value,
        |left, right| left == right,
    ));

    values.insert(second, first_value);
    assert!(!profile_value_is_isolated(
        &values,
        first,
        &first_value,
        |left, right| left == right,
    ));
}

#[test]
fn profile_persistence_binding_is_bounded_without_evicting_old_proof() {
    let mut bindings = HashMap::new();
    for value in 0..MAX_PROFILE_PERSISTENCE_BINDINGS {
        assert!(bind_profile_persistence_class(
            &mut bindings,
            Partition::Persistent(ProfileId::from(value as u128 + 1))
        ));
    }
    assert!(!bind_profile_persistence_class(
        &mut bindings,
        Partition::Persistent(ProfileId::from(
            MAX_PROFILE_PERSISTENCE_BINDINGS as u128 + 1
        ))
    ));
    assert!(bind_profile_persistence_class(
        &mut bindings,
        Partition::Default(ProfileId::from(1))
    ));
    assert!(!bind_profile_persistence_class(
        &mut bindings,
        Partition::Ephemeral(ProfileId::from(1))
    ));
}

#[test]
fn failed_profile_erasure_stays_tombstoned_and_is_retryable() {
    use std::sync::mpsc;

    let profile = ProfileId::from(44);
    let mut tombstones = HashSet::new();
    let mut attempts = HashMap::new();

    let (first_tx, first_rx) = mpsc::channel();
    let first = crate::erasure::Completion::start(
        Box::new(move |outcome| {
            first_tx.send(outcome).unwrap();
        }),
        Arc::new(AtomicBool::new(true)),
    );
    assert!(admit_profile_erasure(
        &mut tombstones,
        &mut attempts,
        profile,
        &first
    ));
    assert!(tombstones.contains(&profile));

    let (duplicate_tx, duplicate_rx) = mpsc::channel();
    let duplicate = crate::erasure::Completion::start(
        Box::new(move |outcome| {
            duplicate_tx.send(outcome).unwrap();
        }),
        Arc::new(AtomicBool::new(true)),
    );
    assert!(!admit_profile_erasure(
        &mut tombstones,
        &mut attempts,
        profile,
        &duplicate
    ));
    assert_eq!(
        duplicate_rx
            .recv_timeout(std::time::Duration::from_millis(100))
            .unwrap(),
        zephium_core::ports::engine::ProfileDataErasureOutcome::Failed
    );

    first.finish(zephium_core::ports::engine::ProfileDataErasureOutcome::Failed);
    assert_eq!(
        first_rx
            .recv_timeout(std::time::Duration::from_millis(100))
            .unwrap(),
        zephium_core::ports::engine::ProfileDataErasureOutcome::Failed
    );
    assert!(tombstones.contains(&profile));

    let retry =
        crate::erasure::Completion::start(Box::new(|_| {}), Arc::new(AtomicBool::new(true)));
    assert!(admit_profile_erasure(
        &mut tombstones,
        &mut attempts,
        profile,
        &retry
    ));
    assert!(tombstones.contains(&profile));
    retry.finish(zephium_core::ports::engine::ProfileDataErasureOutcome::Verified);
}

#[test]
fn caller_timeout_before_host_dispatch_still_tombstones_and_blocks_retry() {
    use std::sync::mpsc;

    let profile = ProfileId::from(45);
    let mut tombstones = HashSet::new();
    let mut attempts = HashMap::new();
    let (tx, rx) = mpsc::channel();
    let delayed = crate::erasure::Completion::start(
        Box::new(move |outcome| {
            tx.send(outcome).unwrap();
        }),
        Arc::new(AtomicBool::new(true)),
    );

    delayed.report_unsettled(zephium_core::ports::engine::ProfileDataErasureOutcome::TimedOut);
    assert_eq!(
        rx.recv_timeout(std::time::Duration::from_millis(100))
            .unwrap(),
        zephium_core::ports::engine::ProfileDataErasureOutcome::TimedOut
    );
    assert!(admit_profile_erasure(
        &mut tombstones,
        &mut attempts,
        profile,
        &delayed
    ));
    assert!(tombstones.contains(&profile));

    let retry =
        crate::erasure::Completion::start(Box::new(|_| {}), Arc::new(AtomicBool::new(true)));
    assert!(!admit_profile_erasure(
        &mut tombstones,
        &mut attempts,
        profile,
        &retry
    ));

    delayed.finish(zephium_core::ports::engine::ProfileDataErasureOutcome::Verified);
    let settled_retry =
        crate::erasure::Completion::start(Box::new(|_| {}), Arc::new(AtomicBool::new(true)));
    assert!(admit_profile_erasure(
        &mut tombstones,
        &mut attempts,
        profile,
        &settled_retry
    ));
    settled_retry.finish(zephium_core::ports::engine::ProfileDataErasureOutcome::Verified);
}

#[test]
fn host_erasure_proof_maps_reject_new_profiles_at_profile_bound() {
    use std::sync::mpsc;

    let mut tombstones = HashSet::new();
    let mut attempts = HashMap::new();
    for value in 0..zephium_core::session::MAX_SESSION_PROFILES {
        let profile = ProfileId::from(value as u128 + 1);
        tombstones.insert(profile);
        attempts.insert(profile, Arc::new(AtomicBool::new(false)));
    }
    let (tx, rx) = mpsc::channel();
    let completion = crate::erasure::Completion::start(
        Box::new(move |outcome| tx.send(outcome).unwrap()),
        Arc::new(AtomicBool::new(true)),
    );

    assert!(!admit_profile_erasure(
        &mut tombstones,
        &mut attempts,
        ProfileId::from(100_000),
        &completion,
    ));
    assert_eq!(
        rx.recv_timeout(std::time::Duration::from_millis(100))
            .unwrap(),
        zephium_core::ports::engine::ProfileDataErasureOutcome::Failed
    );
    assert_eq!(
        tombstones.len(),
        zephium_core::session::MAX_SESSION_PROFILES
    );
    assert_eq!(attempts.len(), zephium_core::session::MAX_SESSION_PROFILES);
}

#[test]
fn attempted_webview2_construction_prevents_empty_maps_from_proving_absence() {
    assert!(browser_group_absence_is_proven(false, false, false));
    assert!(!browser_group_absence_is_proven(false, false, true));
    assert!(!browser_group_absence_is_proven(true, false, false));
    assert!(!browser_group_absence_is_proven(false, true, false));
}

#[test]
fn recovery_requires_a_present_exact_signalled_browser_process() {
    assert!(exact_browser_process_exit_proves_recovery(
        Some((41, true)),
        41,
        41,
        true,
    ));
    assert!(!exact_browser_process_exit_proves_recovery(
        None, 41, 41, true,
    ));
    assert!(!exact_browser_process_exit_proves_recovery(
        Some((41, false)),
        41,
        41,
        true,
    ));
    assert!(!exact_browser_process_exit_proves_recovery(
        Some((42, true)),
        41,
        41,
        true,
    ));
    assert!(!exact_browser_process_exit_proves_recovery(
        Some((41, true)),
        41,
        42,
        true,
    ));
    assert!(!exact_browser_process_exit_proves_recovery(
        Some((41, true)),
        41,
        41,
        false,
    ));
}

#[test]
fn successful_windows_profile_erasure_restores_the_shutdown_map_invariant() {
    assert_eq!(
        transferred_erasure_exit_settlement(41, 41, true, true, false),
        TransferredErasureExitSettlement::Proven
    );
    assert!(
        !windows_profile_provenance_presence_is_consistent(false, false, true, false),
        "an observer without its original retained HANDLE must block shutdown"
    );
    assert!(
        windows_profile_provenance_presence_is_consistent(false, false, false, false),
        "the exact exit callback releases the retained process and observer together"
    );

    assert_eq!(
        transferred_erasure_exit_settlement(41, 41, true, false, true),
        TransferredErasureExitSettlement::Invalid
    );
    assert_eq!(
        transferred_erasure_exit_settlement(41, 41, false, true, false),
        TransferredErasureExitSettlement::Stale
    );
    assert_eq!(
        transferred_erasure_exit_settlement(41, 41, true, false, false),
        TransferredErasureExitSettlement::Pending
    );
}

#[test]
fn windows_erasure_shutdown_requires_the_original_retained_process_pair() {
    assert!(windows_erasure_provenance_is_consistent(
        false,
        false,
        true,
        Some(41),
        Some((41, false)),
    ));
    for (environment, version, tombstone, process, observer) in [
        (false, false, true, None, Some((41, false))),
        (false, false, true, Some(41), None),
        (false, false, true, Some(42), Some((41, false))),
        (false, false, true, Some(41), Some((41, true))),
        (false, false, false, Some(41), Some((41, false))),
        (true, false, true, Some(41), Some((41, false))),
        (false, true, true, Some(41), Some((41, false))),
    ] {
        assert!(!windows_erasure_provenance_is_consistent(
            environment,
            version,
            tombstone,
            process,
            observer,
        ));
    }
    // Pending erasure is a distinct owned state, not a general exemption for
    // an orphaned observer or an incomplete ordinary profile environment.
    assert!(!windows_profile_provenance_presence_is_consistent(
        false, false, true, false
    ));
    assert!(!windows_profile_provenance_presence_is_consistent(
        false, true, true, false
    ));
}

#[test]
fn transferred_erasure_exit_waits_for_the_original_handle_and_shutdown_owner() {
    use TransferredErasureExitSettlement::*;
    assert_eq!(
        transferred_erasure_handle_settlement(Proven, Some((41, true)), 41, false),
        Proven
    );
    assert_eq!(
        transferred_erasure_handle_settlement(Proven, Some((41, false)), 41, false),
        Pending
    );
    assert_eq!(
        transferred_erasure_handle_settlement(Proven, Some((42, true)), 41, false),
        Invalid
    );
    assert_eq!(
        transferred_erasure_handle_settlement(Proven, None, 41, false),
        Invalid
    );
    // The global worker already owns the original HANDLE/proof after drainage;
    // a queued late callback cannot invent missing provenance or take it back.
    assert_eq!(
        transferred_erasure_handle_settlement(Proven, None, 41, true),
        Pending
    );
    for observed in [Pending, Stale, Invalid] {
        assert_eq!(
            transferred_erasure_handle_settlement(observed, Some((41, true)), 41, false),
            observed
        );
    }
}

#[test]
fn delayed_erasure_handle_signal_releases_capacity_without_another_exit_event() {
    let profiles: Vec<_> = (0..MAX_NATIVE_PROFILE_PROCESS_GROUPS)
        .map(|_| ProfileId::generate())
        .collect();
    let erased = profiles[0];
    let requested = ProfileId::generate();
    let mut retained: HashSet<_> = profiles.into_iter().collect();
    assert!(!profile_process_group_capacity_allows(
        retained.iter().copied(),
        requested
    ));

    // The Environment5 proof already arrived; the original HANDLE has not
    // signalled yet. Admission must continue counting that exact group.
    assert!(!windows_erasure_pair_is_reapable(
        false,
        false,
        true,
        Some((41, false)),
        Some((41, false, true)),
    ));
    assert!(!profile_process_group_capacity_allows(
        retained.iter().copied(),
        requested
    ));

    // Without another event, the next admission may release only the exact
    // tombstoned pair after the original handle signals.
    if windows_erasure_pair_is_reapable(
        false,
        false,
        true,
        Some((41, true)),
        Some((41, false, true)),
    ) {
        retained.remove(&erased);
    }
    assert!(profile_process_group_capacity_allows(
        retained.iter().copied(),
        requested
    ));
    for (environment, version, tombstone, process, observer) in [
        (
            false,
            false,
            true,
            Some((41, true)),
            Some((41, false, false)),
        ),
        (false, false, true, Some((41, true)), Some((41, true, true))),
        (
            false,
            false,
            true,
            Some((42, true)),
            Some((41, false, true)),
        ),
        (false, false, true, None, Some((41, false, true))),
        (false, false, true, Some((41, true)), None),
        (
            false,
            false,
            false,
            Some((41, true)),
            Some((41, false, true)),
        ),
        (true, false, true, Some((41, true)), Some((41, false, true))),
        (false, true, true, Some((41, true)), Some((41, false, true))),
    ] {
        assert!(!windows_erasure_pair_is_reapable(
            environment,
            version,
            tombstone,
            process,
            observer
        ));
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
#[test]
fn linux_manager_release_is_exact_attempt_and_terminal_only() {
    let completed = Arc::new(AtomicBool::new(true));
    let replacement = Arc::new(AtomicBool::new(false));

    assert!(!linux_erasure_release_matches(Some(&completed), &completed));
    completed.store(false, Ordering::Release);
    assert!(linux_erasure_release_matches(Some(&completed), &completed));
    assert!(!linux_erasure_release_matches(
        Some(&replacement),
        &completed
    ));
    assert!(!linux_erasure_release_matches(None, &completed));
}

#[test]
fn macos_store_release_is_exact_attempt_and_terminal_only() {
    let completed = Arc::new(AtomicBool::new(true));
    let replacement = Arc::new(AtomicBool::new(false));

    assert!(!macos_erasure_release_matches(Some(&completed), &completed));
    completed.store(false, Ordering::Release);
    assert!(macos_erasure_release_matches(Some(&completed), &completed));
    assert!(!macos_erasure_release_matches(
        Some(&replacement),
        &completed
    ));
    assert!(!macos_erasure_release_matches(None, &completed));
}

#[cfg(target_os = "windows")]
#[test]
fn a_reused_environment_whose_browser_exited_expects_a_new_one() {
    assert!(bootstrap_starts_browser(false, false));
    assert!(bootstrap_starts_browser(true, false));
    assert!(!bootstrap_starts_browser(true, true));
}
