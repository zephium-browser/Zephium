ted_at: 1_700_000_000,
        },
    ];
    assert_eq!(hub.import_history(profile, &visits), Some(2));
    assert_eq!(hub.import_history(profile, &visits), Some(0));
    let page = hub.history_page(profile, "kept", None, None, 10);
    assert_eq!(
        page.iter()
            .map(|visit| visit.visited_at)
            .collect::<Vec<_>>(),
        vec![1_700_000_500, 1_700_000_000]
    );
    assert_eq!(hub.import_history(ProfileId::from(99), &visits), None);
}

#[test]
fn checked_history_pages_distinguish_queue_refusal_timeout_and_storage_errors_from_eof() {
    let profile = ProfileId::from(1);
    let (tx, _rx) = mpsc::sync_channel(0);
    let store = test_store_with_sender(tx);
    assert_eq!(
        store.history_page_checked(profile, "", None, None, 10),
        Err(HistoryPageError::NotAdmitted)
    );
    let (tx, _rx) = mpsc::sync_channel(1);
    let store = test_store_with_sender(tx);
    assert_eq!(
        store.history_page_checked(profile, "", None, None, 10),
        Err(HistoryPageError::TimedOut)
    );
    let dir = tempfile::tempdir().unwrap();
    let mut hub = Hub::open(dir.path().to_path_buf()).unwrap();
    hub.save(&sample()).unwrap();
    assert_eq!(
        hub.history_page_checked(profile, "", None, None, 10),
        Ok(Vec::new())
    );
    Connection::open(dir.path().join(format!("profile-{profile}.sqlite")))
        .unwrap()
        .execute_batch("DROP TABLE history")
        .unwrap();
    assert_eq!(
        hub.history_page_checked(profile, "", None, None, 10),
        Err(HistoryPageError::Failed)
    );
}

#[test]
fn checked_history_pages_skip_disallowed_rows_without_shortening_export_pages() {
    let dir = tempfile::tempdir().unwrap();
    let profile = ProfileId::from(1);
    let mut hub = Hub::open(dir.path().to_path_buf()).unwrap();
    hub.save(&sample()).unwrap();
    let visits: Vec<_> = (1..=405)
        .map(|id| zephium_core::ports::store::ImportedVisit {
            url: format!("https://visit{id}.example/"),
            title: "Kept".into(),
            visited_at: id,
        })
        .collect();
    assert_eq!(hub.import_history(profile, &visits), Some(405));
    let conn = Connection::open(dir.path().join(format!("profile-{profile}.sqlite"))).unwrap();
    conn.execute(
        "UPDATE history SET url = 'javascript:invalid' WHERE id BETWEEN 200 AND 203 OR id = 405",
        [],
    )
    .unwrap();
    for query in ["", "kept"] {
        let mut cursor = None;
        let mut exported = Vec::new();
        for expected_size in [200, 200, 0] {
            let page = hub
                .history_page_checked(profile, query, None, cursor, 200)
                .unwrap();
            assert_eq!(page.len(), expected_size, "{query}");
            assert!(page.iter().all(|row| navigation::is_allowed_str(&row.url)));
            cursor = page.last().map(|row| row.id).or(cursor);
            exported.extend(page.into_iter().map(|row| row.id));
        }
        let expected: Vec<_> = (1..=404)
            .rev()
            .filter(|id| !(200..=203).contains(id))
            .collect();
        assert_eq!(exported, expected, "{query}");
    }
}

#[test]
fn an_idle_store_expires_its_backup_without_a_command_or_session_save() {
    let dir = tempfile::tempdir().unwrap();
    let state = SessionState::default();
    Hub::open(dir.path().to_path_buf())
        .unwrap()
        .save(&state)
        .unwrap();
    let meta = Connection::open(dir.path().join("meta.sqlite")).unwrap();
    meta.execute(
        "INSERT INTO session_snapshot_backup VALUES (1, ?1, 1, ?2)",
        params![
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs() as i64
                - 30 * 24 * 3600
                + 2,
            serde_json::to_vec(&state).unwrap()
        ],
    )
    .unwrap();
    let store = SqliteStore::open(dir.path()).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let remaining: i64 = meta
            .query_row("SELECT count(*) FROM session_snapshot_backup", [], |r| {
                r.get(0)
            })
            .unwrap();
        if remaining == 0 {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "idle expiry requires no mailbox command"
        );
        thread::sleep(Duration::from_millis(10));
    }
    drop(store);
}

#[test]
fn session_backup_disposal_reports_committed_failure_and_retries() {
    let dir = tempfile::tempdir().unwrap();
    let store = SqliteStore::open(dir.path()).unwrap();
    let profile = ProfileId::from(1);
    let state = SessionState {
        profiles: vec![PersistedProfile {
            id: profile,
            name: "P".into(),
            kind: ProfileKind::Default,
        }],
        ..Default::default()
    };
    store.save_session(state.clone());
    let (tx, rx) = mpsc::channel();
    assert!(store.discard_session_backup(
        profile,
        Box::new(move |ok| {
            tx.send(ok).unwrap();
        })
    ));
    assert!(
        rx.recv_timeout(REPLY_TIMEOUT).unwrap(),
        "first-run session is published before disposal"
    );
    let meta = Connection::open(dir.path().join("meta.sqlite")).unwrap();
    meta.execute(
        "INSERT INTO session_snapshot_backup VALUES (1, ?1, 1, ?2)",
        params![
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs() as i64,
            serde_json::to_vec(&state).unwrap()
        ],
    )
    .unwrap();
    meta.execute_batch("CREATE TRIGGER fail_backup_delete BEFORE DELETE ON session_snapshot_backup BEGIN SELECT RAISE(ABORT, 'fixture write failure'); END;").unwrap();
    let (tx, rx) = mpsc::channel();
    assert!(store.discard_session_backup(
        profile,
        Box::new(move |ok| {
            tx.send(ok).unwrap();
        })
    ));
    assert!(!rx.recv_timeout(REPLY_TIMEOUT).unwrap());
    meta.execute_batch("DROP TRIGGER fail_backup_delete")
        .unwrap();
    let (tx, rx) = mpsc::channel();
    assert!(store.discard_session_backup(
        profile,
        Box::new(move |ok| {
            tx.send(ok).unwrap();
        })
    ));
    assert!(rx.recv_timeout(REPLY_TIMEOUT).unwrap());
    assert_eq!(
        meta.query_row("SELECT count(*) FROM session_snapshot_backup", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
}

#[test]
fn panicking_session_backup_completion_does_not_stop_store() {
    let store = SqliteStore::in_memory().unwrap();
    let state = sample();
    let profile = state.profiles[0].id;
    store.save_session(state);
    assert!(store.discard_session_backup(profile, Box::new(|_| panic!("fixture completion panic"))));
    assert!(matches!(store.load_session(), SessionLoad::Loaded { .. }));
}

#[test]
fn checked_history_forget_drains_failed_pending_visits_before_erasure() {
    let dir = tempfile::tempdir().unwrap();
    let mut hub = Hub::open(dir.path().to_path_buf()).unwrap();
    let state = sample();
    let profile = state.profiles[0].id;
    hub.save(&state).unwrap();
    hub.history_page(profile, "", None, None, 10);
    let observer = Connection::open(dir.path().join(format!("profile-{profile}.sqlite"))).unwrap();
    observer.execute_batch("CREATE TRIGGER fail_visit BEFORE INSERT ON history BEGIN SELECT RAISE(ABORT, 'fixture write failure'); END;").unwrap();
    let store = SqliteStore::spawn(hub).unwrap();
    let urls = vec!["https://forget.example/".to_owned()];
    store.record_visit(profile, urls[0].clone(), "Example".into());
    assert_eq!(store.forget_history_urls_checked(profile, &urls), None);
    observer.execute_batch("DROP TRIGGER fail_visit").unwrap();
    assert_eq!(store.forget_history_urls_checked(profile, &urls), Some(1));
    assert!(store.flush_until(Instant::now() + REPLY_TIMEOUT));
    assert!(store.history_page(profile, "", None, None, 10).is_empty());
}

#[test]
fn extension_profile_deletion_erases_registry_temporary_and_owned_staging() {
    let dir = tempfile::tempdir().unwrap();
    let mut hub = Hub::open(dir.path().to_path_buf()).unwrap();
    let state = sample();
    let profile = state.profiles[0].id;
    hub.save(&state).unwrap();
    let root = dir.path().join("webext");
    let registries = root.join("profiles");
    std::fs::create_dir_all(&registries).unwrap();
    let registry = registries.join(format!("{profile}.json"));
    let temporary = registry.with_extension("json.tmp");
    std::fs::write(&registry, br#"{"extensions":[]}"#).unwrap();
    std::fs::write(&temporary, b"private pending access choices").unwrap();
    let staged = root.join("staging").join(profile.to_string());
    std::fs::create_dir_all(&staged).unwrap();
    std::fs::write(staged.join("private-source"), b"private staged package").unwrap();
    let other = registries.join(format!("{}.json", ProfileId::from(999)));
    std::fs::write(&other, b"keep other profile").unwrap();
    assert_eq!(
        hub.authorize_profile_deletion(profile, &SessionState::default())
            .unwrap(),
        ProfileDeletionAuthorizeOutcome::Authorized
    );
    assert!(hub
        .finalize_profile_deletion_requiring_restart_confirmation(profile)
        .unwrap());
    for path in [&registry, &temporary, &staged] {
        assert!(
            !path.exists(),
            "deleted profile retained {}",
            path.display()
        );
    }
    assert_eq!(std::fs::read(other).unwrap(), b"keep other profile");
    drop(hub);
    let mut reopened = Hub::open_for_new_process(dir.path().to_path_buf()).unwrap();
    assert_eq!(reopened.pending_profile_deletions().unwrap().len(), 1);
    assert!(reopened
        .finalize_profile_deletion_requiring_restart_confirmation(profile)
        .unwrap());
    assert!(reopened.pending_profile_deletions().unwrap().is_empty());
    assert!(!registry.exists());
}

#[test]
fn extension_profile_deletion_revokes_publication_without_waiting_for_ticket_drop() {
    let dir = tempfile::tempdir().unwrap();
    let mut hub = Hub::open(dir.path().to_path_buf()).unwrap();
    let state = sample();
    let profile = state.profiles[0].id;
    hub.save(&state).unwrap();
    let root = dir.path().join("webext");
    std::fs::create_dir_all(&root).unwrap();
    let root = std::fs::canonicalize(root).unwrap();
    let (ticket, unrelated) = {
        let mut io = zephium_core::extensions::REGISTRY_IO.lock().unwrap();
        (
            io.register(root.clone(), profile).unwrap(),
            io.register(root.clone(), ProfileId::from(999)).unwrap(),
        )
    };
    assert_eq!(
        hub.authorize_profile_deletion(profile, &SessionState::default())
            .unwrap(),
        ProfileDeletionAuthorizeOutcome::Authorized
    );
    assert!(hub.finalize_profile_deletion(profile).unwrap());
    let _io = zephium_core::extensions::REGISTRY_IO.lock().unwrap();
    assert!(!ticket.allows(&root, profile));
    assert!(unrelated.allows(&root, ProfileId::from(999)));
}

#[test]
fn extension_profile_deletion_does_not_wait_for_package_publication() {
    let dir = tempfile::tempdir().unwrap();
    let mut hub = Hub::open(dir.path().to_path_buf()).unwrap();
    let state = sample();
    let profile = state.profiles[0].id;
    hub.save(&state).unwrap();
    let publication = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(dir.path().join("extension-publication.lock"))
        .unwrap();
    publication.lock().unwrap();
    assert_eq!(
        hub.authorize_profile_deletion(profile, &SessionState::default())
            .unwrap(),
        ProfileDeletionAuthorizeOutcome::Authorized
    );
    assert!(hub.finalize_profile_deletion(profile).unwrap());
}

#[test]
fn extension_cleanup_failure_keeps_the_deletion_retryable() {
    let dir = tempfile::tempdir().unwrap();
    let mut hub = Hub::open(dir.path().to_path_buf()).unwrap();
    let state = sample();
    let profile = state.profiles[0].id;
    hub.save(&state).unwrap();
    hub.authorize_profile_deletion(profile, &SessionState::default())
        .unwrap();
    let root = dir.path().join("webext");
    std::fs::create_dir(&root).unwrap();
    std::fs::write(root.join("staging"), b"invalid namespace").unwrap();
    assert!(hub.finalize_profile_deletion(profile).is_err());
    assert_eq!(hub.pending_profile_deletions().unwrap().len(), 1);
    std::fs::remove_file(root.join("staging")).unwrap();
    assert!(hub.finalize_profile_deletion(profile).unwrap());
}

#[test]
fn a_sites_origins_come_from_its_history_and_icons_with_their_ports() {
    let mut hub = Hub::in_memory().unwrap();
    hub.save(&sample()).unwrap();
    let profile = ProfileId::from(1);
    for url in [
        "https://www.example.com/a",
        "http://example.com:8080/b?c",
        "https://www.example.com/again",
        "https://notexample.com/",
        "https://example.com.evil.test/",
        "http://127.0.0.1:3000/",
    ] {
        hub.record_visit(profile, url, "Page");
    }
    hub.save_favicon(profile, "https://cdn.example.com", None, &rgba());
    hub.save_favicon(profile, "https://shop.example.com:8443/cart", None, &rgba());

    let (origins, complete) = hub.site_origins(profile, "example.com");
    assert!(complete);
    assert_eq!(origins.len(), 4, "{origins:?}");
    for origin in [
        "https://www.example.com",
        "http://example.com:8080",
        "https://cdn.example.com",
        "https://shop.example.com:8443",
    ] {
        assert!(origins.iter().any(|o| o == origin), "{origin}: {origins:?}");
    }
    assert_eq!(
        hub.site_origins(profile, "127.0.0.1"),
        (vec!["http://127.0.0.1:3000".to_owned()], true)
    );
    // Nothing can be read for a name that is no site or a profile not held.
    assert_eq!(
        hub.site_origins(profile, "https://example.com"),
        (Vec::new(), false)
    );
    assert_eq!(
        hub.site_origins(ProfileId::from(9), "example.com"),
        (Vec::new(), false)
    );
}

#[test]
fn a_sites_origins_stop_at_their_cap() {
    let mut hub = Hub::in_memory().unwrap();
    hub.save(&sample()).unwrap();
    let profile = ProfileId::from(1);
    let visits: Vec<(String, String)> = (0..zephium_core::browsing_data::MAX_SITE_ORIGINS + 20)
        .map(|n| (format!("https://s{n}.example.com/"), "Page".to_owned()))
        .collect();
    for (url, title) in &visits {
        hub.record_visit(profile, url, title);
    }
    let (origins, complete) = hub.site_origins(profile, "example.com");
    assert!(!complete, "stopped at the cap, so some may be left out");
    assert_eq!(origins.len(), zephium_core::browsing_data::MAX_SITE_ORIGINS);
    // Newest visits first.
    assert_eq!(
        origins[0],
        format!(
            "https://s{}.example.com",
            zephium_core::browsing_data::MAX_SITE_ORIGINS + 19
        )
    );
}
