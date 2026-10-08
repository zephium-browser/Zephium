use super::*;
use zephium_core::time::{FocusRecord, HourTally, Place, Tally, TimeQuery, MAX_PENDING_TALLIES};

fn tally(hour: i64, spent_ms: i64) -> Vec<HourTally> {
    vec![HourTally {
        hour,
        place: Place::Site("example.test".to_owned()),
        tally: Tally { spent_ms, opens: 1 },
    }]
}

fn focus(started_ms: i64, focused_ms: i64) -> FocusRecord {
    FocusRecord {
        started_ms,
        ended_ms: started_ms + focused_ms,
        focused_ms,
        rounds: 1,
        completed: true,
    }
}

// Match the existing storage harness deadline; parallel SQLite fixture
// setup can outlast the product's short interactive barrier budget.
fn durable(store: &SqliteStore) -> bool {
    store.flush_until(Instant::now() + REPLY_TIMEOUT)
}

fn fixture() -> (tempfile::TempDir, SqliteStore, Connection) {
    let dir = tempfile::tempdir().unwrap();
    let mut hub = Hub::open(dir.path().to_path_buf()).unwrap();
    hub.save(&sample()).unwrap();
    hub.record_time(ProfileId::from(1), &[], 0).unwrap();
    hub.immediate_time_lock_failures_for_test(ProfileId::from(1));
    let store = SqliteStore::spawn(hub).unwrap();
    let conn = Connection::open(
        dir.path()
            .join(format!("profile-{}.sqlite", ProfileId::from(1))),
    )
    .unwrap();
    (dir, store, conn)
}

fn fail_inserts(conn: &Connection, table: &str) {
    if table == "time_spent" {
        conn.execute_batch("BEGIN IMMEDIATE").unwrap();
        return;
    }
    conn.execute_batch(&format!(
        "CREATE TRIGGER injected_activity_failure BEFORE INSERT ON {table}
         BEGIN SELECT RAISE(ABORT, 'fixture write failure'); END;"
    ))
    .unwrap();
}

fn recover(conn: &Connection) {
    if !conn.is_autocommit() {
        conn.execute_batch("ROLLBACK").unwrap();
        return;
    }
    conn.execute_batch("DROP TRIGGER injected_activity_failure;")
        .unwrap();
}

fn total(conn: &Connection) -> i64 {
    conn.query_row(
        "SELECT coalesce(sum(spent_ms), 0) FROM time_spent",
        [],
        |row| row.get(0),
    )
    .unwrap()
}

#[test]
fn activity_failed_batch_is_retained_and_flush_reports_failure_until_commit() {
    let (_dir, store, observer) = fixture();
    let profile = ProfileId::from(1);
    fail_inserts(&observer, "time_spent");
    assert!(store.record_time(profile, tally(100, 1_000), 0));
    assert!(!durable(&store));
    assert_eq!(store.activity_admission.lock().unwrap().commands, 1);
    assert!(store.record_time(profile, tally(100, 2_000), 0));
    assert!(!durable(&store));
    assert_eq!(store.activity_admission.lock().unwrap().commands, 2);
    assert_eq!(total(&observer), 0);

    let (done, result) = mpsc::channel();
    assert!(store.time_report(
        profile,
        TimeQuery {
            from_hour: 100,
            bucket_hours: 1,
            buckets: 1,
            site: None,
        },
        Box::new(move |report| {
            done.send(report).unwrap();
        })
    ));
    assert!(result.recv_timeout(REPLY_TIMEOUT).unwrap().is_none());
    recover(&observer);
    assert!(durable(&store));
    assert_eq!(total(&observer), 3_000);
    assert_eq!(store.activity_admission.lock().unwrap().commands, 0);
    assert!(durable(&store));
    assert_eq!(
        total(&observer),
        3_000,
        "successful batches must not replay"
    );
}

#[test]
fn activity_fifo_preserves_write_clear_write_order_after_failure() {
    let (_dir, store, observer) = fixture();
    let profile = ProfileId::from(1);
    fail_inserts(&observer, "time_spent");
    assert!(store.record_time(profile, tally(100, 1_000), 0));
    assert!(!durable(&store));
    assert!(store.clear_time(profile, None));
    assert!(store.record_time(profile, tally(100, 2_000), 0));
    assert!(!durable(&store));
    recover(&observer);
    assert!(durable(&store));
    assert_eq!(
        total(&observer),
        2_000,
        "older retry must stay before the clear"
    );
}

#[test]
fn activity_failed_clear_cannot_erase_a_later_accepted_write() {
    let (_dir, store, observer) = fixture();
    let profile = ProfileId::from(1);
    assert!(store.record_time(profile, tally(100, 1_000), 0));
    assert!(durable(&store));
    observer.execute_batch("BEGIN IMMEDIATE").unwrap();
    assert!(store.clear_time(profile, Some(100)));
    assert!(!durable(&store));
    assert!(store.record_time(profile, tally(100, 2_000), 0));
    assert!(!durable(&store));
    assert_eq!(total(&observer), 1_000);
    recover(&observer);
    assert!(durable(&store));
    assert_eq!(total(&observer), 2_000);
}

#[test]
fn activity_focus_failure_does_not_hold_shutdown() {
    let (dir, store, _observer) = fixture();
    let meta = Connection::open(dir.path().join("meta.sqlite")).unwrap();
    fail_inserts(&meta, "focus_sessions");
    assert!(store.record_focus(focus(1_000, 3_000), 1));
    assert!(!durable(&store));
    let (done, result) = mpsc::channel();
    assert!(store.focus_days(
        1,
        1,
        Box::new(move |days| {
            done.send(days).unwrap();
        })
    ));
    assert!(result.recv_timeout(REPLY_TIMEOUT).unwrap().is_none());
    // A record that keeps failing cannot outlive the process. Holding the
    // exit unclean for it would only postpone updates on every quit.
    assert_eq!(
        store.shutdown_until(Instant::now() + REPLY_TIMEOUT),
        StoreShutdownOutcome::Clean
    );
    assert_eq!(store.activity_admission.lock().unwrap().commands, 0);
    assert_eq!(
        meta.query_row("SELECT count(*) FROM focus_sessions", [], |row| row
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
}

#[test]
fn activity_waits_for_failed_initial_profile_registration() {
    let dir = tempfile::tempdir().unwrap();
    let store = SqliteStore::open(dir.path()).unwrap();
    let meta = Connection::open(dir.path().join("meta.sqlite")).unwrap();
    fail_inserts(&meta, "session_snapshot");
    store.save_session(sample());
    assert!(store.record_time(ProfileId::from(1), tally(100, 1_000), 0));
    assert!(!durable(&store));
    assert_eq!(store.activity_admission.lock().unwrap().commands, 1);
    recover(&meta);
    assert!(durable(&store));
    let observer = Connection::open(
        dir.path()
            .join(format!("profile-{}.sqlite", ProfileId::from(1))),
    )
    .unwrap();
    assert_eq!(total(&observer), 1_000);
}

#[test]
fn activity_authorized_profile_deletion_discards_retained_writes_without_resurrection() {
    let (_dir, store, observer) = fixture();
    let profile = ProfileId::from(1);
    fail_inserts(&observer, "time_spent");
    assert!(store.record_time(profile, tally(100, 1_000), 0));
    assert!(!durable(&store));
    assert_eq!(
        store.authorize_profile_deletion(
            profile,
            SessionState::default(),
            Instant::now() + REPLY_TIMEOUT
        ),
        ProfileDeletionAuthorizeOutcome::Authorized
    );
    assert_eq!(store.activity_admission.lock().unwrap().commands, 0);
    recover(&observer);
    assert!(store.record_time(profile, tally(100, 2_000), 0));
    assert!(store.clear_time(profile, None));
    assert!(durable(&store));
    assert_eq!(total(&observer), 0);
    assert_eq!(store.activity_admission.lock().unwrap().commands, 0);
}

#[test]
fn activity_unregistered_private_profile_never_creates_a_database() {
    let dir = tempfile::tempdir().unwrap();
    let store = SqliteStore::open(dir.path()).unwrap();
    let private = ProfileId::from(999);
    assert!(store.record_time(private, tally(100, 1_000), 0));
    assert!(store.clear_time(private, None));
    assert!(durable(&store));
    assert!(!dir
        .path()
        .join(format!("profile-{private}.sqlite"))
        .exists());
    assert_eq!(store.activity_admission.lock().unwrap().commands, 0);
}

#[test]
fn activity_admission_stays_charged_across_actor_dequeue_and_releases_on_refusal() {
    let (tx, rx) = mpsc::sync_channel(256);
    let store = test_store_with_sender(tx);
    for _ in 0..super::super::activity::MAX_PENDING_ACTIVITY_WRITES_PER_SCOPE {
        assert!(store.record_focus(focus(1_000, 3_000), 1));
    }
    assert!(!store.record_focus(focus(1_000, 3_000), 1));
    let retained = rx.recv().unwrap();
    assert!(
        !store.record_focus(focus(1_000, 3_000), 1),
        "dequeue cannot free a retry slot"
    );
    drop(retained);
    assert!(store.record_focus(focus(1_000, 3_000), 1));
    drop(rx);
    assert_eq!(store.activity_admission.lock().unwrap().commands, 0);
    assert!(!store.record_focus(focus(1_000, 3_000), 1));
    assert_eq!(store.activity_admission.lock().unwrap().commands, 0);
}

#[test]
fn activity_admission_bounds_retained_vector_capacity_and_does_not_wait_for_lifecycle() {
    let (tx, rx) = mpsc::sync_channel(256);
    let store = test_store_with_sender(tx);
    let tallies = Vec::with_capacity(MAX_PENDING_TALLIES + 1);
    assert!(!store.record_time(ProfileId::from(1), tallies, 0));
    let mut tallies = Vec::with_capacity(MAX_PENDING_TALLIES);
    tallies.extend(tally(100, 1_000));
    assert!(store.record_time(ProfileId::from(1), tallies, 0));
    assert!(!store.record_time(ProfileId::from(1), tally(100, 1_000), 0));
    drop(rx.recv().unwrap());
    assert!(store.record_time(ProfileId::from(1), tally(100, 1_000), 0));
    let _shutdown = store.lifecycle.write().unwrap();
    assert!(!store.record_focus(focus(1_000, 3_000), 1));
}

#[test]
fn activity_idle_retry_commits_without_another_actor_command() {
    let (_dir, store, observer) = fixture();
    fail_inserts(&observer, "time_spent");
    assert!(store.record_time(ProfileId::from(1), tally(100, 1_000), 0));
    assert!(!durable(&store));
    recover(&observer);
    let deadline = Instant::now() + REPLY_TIMEOUT;
    while total(&observer) != 1_000 && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(total(&observer), 1_000);
    assert_eq!(store.activity_admission.lock().unwrap().commands, 0);
}

#[test]
fn activity_deletion_unblocks_survivor_fifo_without_unrelated_traffic() {
    let (_dir, store, observer) = fixture();
    store.save_session(two_profile_sample());
    assert!(durable(&store));
    fail_inserts(&observer, "time_spent");
    assert!(store.record_time(ProfileId::from(1), tally(100, 1_000), 0));
    assert!(!durable(&store));
    assert!(store.record_time(ProfileId::from(3), tally(100, 2_000), 0));
    let survivors = SessionState {
        profiles: vec![PersistedProfile {
            id: ProfileId::from(3),
            name: "Work".into(),
            kind: ProfileKind::Named,
        }],
        spaces: vec![PersistedSpace {
            id: SpaceId::from(4),
            profile: ProfileId::from(3),
            name: "Work".into(),
        }],
        active_space: Some(SpaceId::from(4)),
        ..SessionState::default()
    };
    assert_eq!(
        store.authorize_profile_deletion(
            ProfileId::from(1),
            survivors,
            Instant::now() + REPLY_TIMEOUT
        ),
        ProfileDeletionAuthorizeOutcome::Authorized
    );
    let deadline = Instant::now() + REPLY_TIMEOUT;
    while store.activity_admission.lock().unwrap().commands != 0 && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(store.activity_admission.lock().unwrap().commands, 0);
    let (done, result) = mpsc::channel();
    assert!(store.time_report(
        ProfileId::from(3),
        TimeQuery {
            from_hour: 100,
            bucket_hours: 1,
            buckets: 1,
            site: None,
        },
        Box::new(move |report| {
            done.send(report).unwrap();
        })
    ));
    assert_eq!(
        result.recv_timeout(REPLY_TIMEOUT).unwrap().unwrap().buckets[0].browse_ms,
        2_000
    );
}

#[test]
fn activity_checked_history_clear_cannot_mask_failed_time_clear_or_history_delete() {
    let (_dir, store, observer) = fixture();
    let profile = ProfileId::from(1);
    store.record_visit(
        profile,
        "https://history.example/".into(),
        "History fixture".into(),
    );
    assert!(store.record_time(profile, tally(100, 1_000), 0));
    assert!(durable(&store));
    observer
        .execute_batch(
            "CREATE TRIGGER injected_activity_failure BEFORE DELETE ON time_spent
         BEGIN SELECT RAISE(ABORT, 'fixture time clear failure'); END;",
        )
        .unwrap();
    assert!(store.clear_time(profile, None));
    assert_eq!(store.clear_history_checked(profile, None), None);
    assert_eq!(
        observer
            .query_row("SELECT count(*) FROM history", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        1
    );
    recover(&observer);
    assert_eq!(store.clear_history_checked(profile, None), Some(1));
    assert_eq!(total(&observer), 0);
    assert_eq!(store.clear_history_checked(profile, None), Some(0));

    store.record_visit(
        profile,
        "https://history.example/".into(),
        "History fixture".into(),
    );
    assert!(durable(&store));
    observer
        .execute_batch(
            "CREATE TRIGGER injected_activity_failure BEFORE DELETE ON history
         BEGIN SELECT RAISE(ABORT, 'fixture history clear failure'); END;",
        )
        .unwrap();
    assert_eq!(store.clear_history_checked(profile, None), None);
    assert_eq!(
        observer
            .query_row("SELECT count(*) FROM history", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        1
    );
    recover(&observer);
    assert_eq!(store.clear_history_checked(profile, None), Some(1));

    // A previously accepted visit cannot reappear after a successful clear.
    fail_inserts(&observer, "history");
    store.record_visit(
        profile,
        "https://pending.example/".into(),
        "Pending fixture".into(),
    );
    assert_eq!(store.clear_history_checked(profile, None), None);
    recover(&observer);
    assert_eq!(store.clear_history_checked(profile, None), Some(1));
    assert!(durable(&store));
    assert_eq!(
        observer
            .query_row("SELECT count(*) FROM history", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
}

#[test]
fn activity_ambiguous_commit_keeps_the_batch_id_and_blocks_newer_receipt_pruning() {
    let dir = tempfile::tempdir().unwrap();
    let profile = ProfileId::from(1);
    let mut hub = Hub::open(dir.path().to_path_buf()).unwrap();
    hub.save(&sample()).unwrap();
    for _ in 0..hub::MAX_TIME_BATCH_RECEIPTS {
        hub.record_time(profile, &tally(100, 1), 0).unwrap();
    }
    hub.fail_next_time_commit_as_ambiguous();
    let store = SqliteStore::spawn(hub).unwrap();
    let observer = Connection::open(dir.path().join(format!("profile-{profile}.sqlite"))).unwrap();
    assert!(store.record_time(profile, tally(100, 1_000), 0));
    assert!(store.record_time(profile, tally(100, 2_000), 0));
    let (done, result) = mpsc::channel();
    assert!(store.time_report(
        profile,
        TimeQuery {
            from_hour: 100,
            bucket_hours: 1,
            buckets: 1,
            site: None,
        },
        Box::new(move |report| {
            done.send(report).unwrap();
        })
    ));
    assert!(result.recv_timeout(REPLY_TIMEOUT).unwrap().is_none());
    assert_eq!(store.activity_admission.lock().unwrap().commands, 2);
    assert_eq!(
        total(&observer),
        hub::MAX_TIME_BATCH_RECEIPTS as i64 + 1_000,
        "the negative acknowledgement committed once, and later activity is blocked"
    );
    assert_eq!(
        observer
            .query_row("SELECT count(*) FROM time_batch_receipts", [], |row| row
                .get::<_, usize>(
                0
            ))
            .unwrap(),
        hub::MAX_TIME_BATCH_RECEIPTS
    );
    assert!(durable(&store));
    assert_eq!(
        total(&observer),
        hub::MAX_TIME_BATCH_RECEIPTS as i64 + 3_000,
        "the original batch must reconcile its retained receipt rather than add twice"
    );
    assert_eq!(store.activity_admission.lock().unwrap().commands, 0);
    assert!(durable(&store));
    assert_eq!(
        total(&observer),
        hub::MAX_TIME_BATCH_RECEIPTS as i64 + 3_000
    );
}

#[test]
fn activity_permanent_profile_failure_does_not_block_other_profiles_or_focus() {
    let (_dir, store, observer) = fixture();
    store.save_session(two_profile_sample());
    assert!(durable(&store));
    observer.execute_batch("CREATE TRIGGER permanent_time_failure BEFORE INSERT ON time_spent BEGIN SELECT RAISE(ABORT, 'permanent fixture'); END;").unwrap();
    assert!(store.record_time(ProfileId::from(1), tally(100, 1_000), 0));
    let (done, result) = mpsc::channel();
    assert!(store.time_report(
        ProfileId::from(1),
        TimeQuery {
            from_hour: 100,
            bucket_hours: 1,
            buckets: 1,
            site: None
        },
        Box::new(move |value| {
            done.send(value).unwrap();
        })
    ));
    assert!(result.recv_timeout(REPLY_TIMEOUT).unwrap().is_none());
    for _ in 0..100 {
        assert!(!store.record_time(ProfileId::from(1), tally(100, 1_000), 0));
    }
    assert!(store.record_time(ProfileId::from(3), tally(100, 2_000), 0));
    assert!(store.record_focus(focus(1_000, 3_000), 1));
    let (done, result) = mpsc::channel();
    assert!(store.time_report(
        ProfileId::from(3),
        TimeQuery {
            from_hour: 100,
            bucket_hours: 1,
            buckets: 1,
            site: None
        },
        Box::new(move |value| {
            done.send(value).unwrap();
        })
    ));
    assert_eq!(
        result.recv_timeout(REPLY_TIMEOUT).unwrap().unwrap().buckets[0].browse_ms,
        2_000
    );
    let (done, result) = mpsc::channel();
    assert!(store.focus_days(
        1,
        1,
        Box::new(move |value| {
            done.send(value).unwrap();
        })
    ));
    assert_eq!(
        result.recv_timeout(REPLY_TIMEOUT).unwrap().unwrap()[0].focused_ms,
        3_000
    );
    assert_eq!(
        store.clear_history_checked(ProfileId::from(3), None),
        Some(0)
    );
    assert_eq!(store.activity_admission.lock().unwrap().commands, 1);
    // Repair alone cannot cause background/report retries of a permanent
    // failure; an intentional clear explicitly retries the quarantined lane.
    observer
        .execute_batch("DROP TRIGGER permanent_time_failure")
        .unwrap();
    let (done, result) = mpsc::channel();
    assert!(store.time_report(
        ProfileId::from(1),
        TimeQuery {
            from_hour: 100,
            bucket_hours: 1,
            buckets: 1,
            site: None
        },
        Box::new(move |value| {
            done.send(value).unwrap();
        })
    ));
    assert!(result.recv_timeout(REPLY_TIMEOUT).unwrap().is_none());
    assert_eq!(total(&observer), 0);
    assert_eq!(
        store.clear_history_checked(ProfileId::from(1), None),
        Some(0)
    );
    assert_eq!(total(&observer), 1_000);
}

#[test]
fn activity_admission_waits_for_brief_counter_contention() {
    let (tx, rx) = mpsc::sync_channel(4);
    let store = Arc::new(test_store_with_sender(tx));
    let guard = store.activity_admission.lock().unwrap();
    let producer = store.clone();
    let (done, result) = mpsc::channel();
    let worker = thread::spawn(move || {
        done.send(producer.record_focus(focus(1_000, 3_000), 1))
            .unwrap();
    });
    assert!(result.recv_timeout(Duration::from_millis(20)).is_err());
    drop(guard);
    assert!(result.recv_timeout(REPLY_TIMEOUT).unwrap());
    worker.join().unwrap();
    drop(rx.recv().unwrap());
    assert_eq!(store.activity_admission.lock().unwrap().commands, 0);
}

#[test]
fn activity_profile_clear_is_not_held_by_a_foreign_failed_visit() {
    let (_dir, store, observer) = fixture();
    store.save_session(two_profile_sample());
    assert!(durable(&store));
    store.record_visit(
        ProfileId::from(3),
        "https://healthy.test/".into(),
        "Healthy".into(),
    );
    assert!(durable(&store));
    fail_inserts(&observer, "history");
    store.record_visit(
        ProfileId::from(1),
        "https://broken.test/".into(),
        "Pending".into(),
    );
    assert!(!durable(&store));
    assert_eq!(
        store.clear_history_checked(ProfileId::from(3), None),
        Some(1)
    );
    recover(&observer);
    assert!(durable(&store));
    assert_eq!(
        store
            .history_page(ProfileId::from(1), "", None, None, 50)
            .len(),
        1
    );
    assert!(store
        .history_page(ProfileId::from(3), "", None, None, 50)
        .is_empty());
}

#[test]
fn activity_clearing_all_time_replaces_a_write_that_keeps_failing() {
    let (_dir, store, observer) = fixture();
    let profile = ProfileId::from(1);
    assert!(store.record_time(profile, tally(100, 500), 0));
    assert!(durable(&store));
    observer
        .execute_batch(
            "CREATE TRIGGER injected_tally_failure BEFORE INSERT ON time_spent
         BEGIN SELECT RAISE(ABORT, 'fixture tally failure'); END;",
        )
        .unwrap();
    assert!(store.record_time(profile, tally(101, 1_000), 0));
    assert!(!durable(&store));

    // The stuck tally is exactly what the clear erases, so the clear is never
    // the write a quarantine refuses, and it settles the profile again.
    assert!(store.clear_time(profile, None));
    assert!(durable(&store));
    assert_eq!(total(&observer), 0);
}
