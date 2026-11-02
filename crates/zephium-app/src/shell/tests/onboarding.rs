use super::*;

fn onboarding(store: &FakeStore) -> Option<String> {
    store.app_setting("onboarding")
}

fn kept(shell: &Shell) -> Vec<(ItemId, String)> {
    let space = shell.windows.focused().unwrap().space;
    shell
        .items
        .roots(Placement::Favorites { space })
        .iter()
        .map(|id| {
            let url = shell.items.tab(*id).unwrap().url.as_ref().unwrap();
            (*id, url.to_string())
        })
        .collect()
}

#[test]
fn a_fresh_install_opens_onboarding_until_it_is_finished() {
    let store = Arc::new(FakeStore::default());
    assert!(crate::onboarding_due(store.as_ref()));
    assert_eq!(onboarding(&store).as_deref(), Some("pending"));

    // Quitting part way through brings it back, even once the shell has
    // written a session behind it.
    let (mut shell, _engine, _screen) = setup_with(store.clone());
    shell.handle(Command::Bootstrap);
    shell.handle(Command::Persist);
    assert!(store.saved.lock().unwrap().is_some());
    assert!(crate::onboarding_due(store.as_ref()));

    assert!(crate::finish_onboarding(store.as_ref()));
    assert!(!crate::onboarding_due(store.as_ref()));
    // Answered is answered, even if the session is later lost.
    *store.saved.lock().unwrap() = None;
    assert!(!crate::onboarding_due(store.as_ref()));
}

#[test]
fn an_install_from_before_onboarding_never_sees_it_and_is_asked_once() {
    let store = Arc::new(FakeStore::default());
    let (mut shell, _engine, _screen) = setup_with(store.clone());
    shell.handle(Command::Bootstrap);
    shell.handle(Command::Persist);
    assert!(store.saved.lock().unwrap().is_some());
    assert_eq!(onboarding(&store), None);

    assert!(!crate::onboarding_due(store.as_ref()));
    assert_eq!(onboarding(&store).as_deref(), Some("done"));
    let reads = store
        .load_session_calls
        .load(std::sync::atomic::Ordering::Acquire);
    assert!(!crate::onboarding_due(store.as_ref()));
    assert_eq!(
        store
            .load_session_calls
            .load(std::sync::atomic::Ordering::Acquire),
        reads
    );
}

#[test]
fn storage_that_needs_recovery_never_begins_onboarding_or_records_an_answer() {
    let failed = FakeStore::default();
    *failed.load_failed.lock().unwrap() = true;
    assert!(!crate::onboarding_due(&failed));
    assert_eq!(onboarding(&failed), None);

    let recovering = FakeStore::default();
    *recovering.recovery_reason.lock().unwrap() = Some("schema".into());
    assert!(!crate::onboarding_due(&recovering));
    assert_eq!(onboarding(&recovering), None);
}

#[test]
fn bootstrap_lands_on_the_browser_whatever_onboarding_has_recorded() {
    let store = Arc::new(FakeStore::default());
    assert!(store.set_app_setting("onboarding".into(), "pending".into()));
    let (mut shell, _engine, _screen) = setup_with(store);
    shell.handle(Command::Bootstrap);
    assert_eq!(shell.active_browser_page(), None);
}

#[test]
fn a_catalog_site_is_kept_unloaded_with_its_mark_and_only_once() {
    let store = Arc::new(FakeStore::default());
    let (mut shell, _engine, screen) = setup_with(store.clone());
    shell.handle(Command::Bootstrap);
    let active = active_id(&screen);

    let first = shell.handle_operation(Command::KeepSite("slack".into()));
    assert!(matches!(
        first.outcome,
        OperationOutcome::Applied | OperationOutcome::Deferred
    ));
    let sites = kept(&shell);
    assert_eq!(sites.len(), 1);
    assert_eq!(sites[0].1, "https://app.slack.com/client");
    let tab = shell.items.tab(sites[0].0).unwrap();
    assert_eq!(tab.title, "Slack");
    assert!(!tab.has_view(), "keeping a site must not load it");
    assert_eq!(
        active_id(&screen),
        active,
        "keeping a site never focuses it"
    );
    assert!(last(&screen)
        .nodes
        .iter()
        .any(|node| node.id == sites[0].0.to_string()));

    let icons = store.icons.lock().unwrap().clone();
    assert_eq!(icons.len(), 1);
    assert_eq!(icons[0].0, "https://app.slack.com");
    assert_eq!(icons[0].1.len(), zephium_core::icon::RGBA32_BYTES);

    let again = shell.handle_operation(Command::KeepSite("slack".into()));
    assert_eq!(again.outcome, OperationOutcome::NoOp);
    assert_eq!(kept(&shell).len(), 1);

    for unknown in ["", "javascript:alert(1)", "https://evil.example/", "SLACK"] {
        let refused = shell.handle_operation(Command::KeepSite(unknown.into()));
        assert_eq!(refused.outcome, OperationOutcome::Rejected);
    }
    assert_eq!(kept(&shell).len(), 1);
}

#[test]
fn a_kept_site_belongs_to_the_space_it_was_kept_in() {
    let (mut shell, _engine, _screen) = setup();
    shell.handle(Command::Bootstrap);
    let home = shell.windows.focused().unwrap().space;
    shell.handle_operation(Command::KeepSite("slack".into()));
    assert_eq!(kept(&shell).len(), 1);

    shell.handle_operation(Command::Space(Box::new(crate::SpaceCommand::Create {
        name: "Studio".into(),
        look: Default::default(),
    })));
    assert_ne!(shell.windows.focused().unwrap().space, home);
    assert!(kept(&shell).is_empty());

    // The same site may be kept again here, and letting it go here leaves
    // the other space's alone.
    let here = shell.handle_operation(Command::KeepSite("slack".into()));
    assert_ne!(here.outcome, OperationOutcome::NoOp);
    assert_eq!(kept(&shell).len(), 1);
    shell.handle_operation(Command::ReleaseSite("slack".into()));
    assert!(kept(&shell).is_empty());
    assert_eq!(
        shell
            .items
            .roots(Placement::Favorites { space: home })
            .len(),
        1
    );
}

#[test]
fn a_kept_site_is_let_go_whole_where_closing_would_keep_it() {
    let (mut shell, _engine, _screen) = setup();
    shell.handle(Command::Bootstrap);
    shell.handle_operation(Command::KeepSite("slack".into()));
    shell.handle_operation(Command::KeepSite("notion".into()));
    assert_eq!(kept(&shell).len(), 2);

    let released = shell.handle_operation(Command::ReleaseSite("slack".into()));
    assert!(matches!(
        released.outcome,
        OperationOutcome::Applied | OperationOutcome::Deferred
    ));
    let sites = kept(&shell);
    assert_eq!(sites.len(), 1);
    assert_eq!(sites[0].1, "https://www.notion.so/");

    let again = shell.handle_operation(Command::ReleaseSite("slack".into()));
    assert_eq!(again.outcome, OperationOutcome::NoOp);
    let unknown = shell.handle_operation(Command::ReleaseSite("SLACK".into()));
    assert_eq!(unknown.outcome, OperationOutcome::Rejected);
    assert_eq!(kept(&shell).len(), 1);
}

#[test]
fn every_catalog_site_is_a_valid_page_with_a_full_mark() {
    for site in crate::shell::kept_sites::KEPT_SITES {
        let url = url::Url::parse(site.url).unwrap();
        assert!(zephium_core::navigation::is_allowed(&url), "{}", site.id);
        assert_eq!(url.scheme(), "https", "{}", site.id);
        assert!(
            site.mark.chunks_exact(4).any(|pixel| pixel[3] > 0),
            "{} has an empty mark",
            site.id
        );
    }
}

/// Chrome draws the catalog and names sites by id; native owns each address.
/// Both lists must describe the same sites at the same addresses.
#[test]
fn chrome_and_native_catalogs_agree() {
    let source = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../frame/src/features/onboarding/lib/catalog.ts"
    ))
    .unwrap();
    let chrome_ids: Vec<&str> = source
        .lines()
        .filter_map(|line| line.strip_prefix("  { id: \""))
        .filter_map(|rest| rest.split('"').next())
        .collect();
    let native_ids: Vec<&str> = crate::shell::kept_sites::KEPT_SITES
        .iter()
        .map(|site| site.id)
        .collect();
    assert_eq!(chrome_ids, native_ids);
    for site in crate::shell::kept_sites::KEPT_SITES {
        let key = if site.id.contains('-') {
            format!("  \"{}\": \"{}\",", site.id, site.url)
        } else {
            format!("  {}: \"{}\",", site.id, site.url)
        };
        assert!(
            source.contains(&key),
            "chrome has another address for {}",
            site.id
        );
    }
}

#[test]
fn the_focused_profile_takes_the_name_it_is_given() {
    let store = Arc::new(FakeStore::default());
    let (mut shell, _engine, screen) = setup_with(store.clone());
    shell.handle(Command::Bootstrap);

    let named = shell.handle_operation(Command::RenameFocusedProfile("  Alex Rivera ".into()));
    assert_eq!(named.outcome, OperationOutcome::Applied);
    assert_eq!(last(&screen).profile.unwrap().name, "Alex Rivera");

    let cleared = shell.handle_operation(Command::RenameFocusedProfile("   ".into()));
    assert_eq!(cleared.outcome, OperationOutcome::Rejected);
    assert_eq!(last(&screen).profile.unwrap().name, "Alex Rivera");

    shell.persist();
    let saved = store.saved.lock().unwrap().clone().unwrap();
    assert!(saved
        .profiles
        .iter()
        .any(|profile| profile.name == "Alex Rivera"));
}

#[test]
fn essentials_from_another_browser_join_the_kept_sites_once_each_unloaded() {
    let (mut shell, _engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    let active = active_id(&screen);
    shell.handle_operation(Command::KeepSite("slack".into()));

    let site = |url: &str, title: &str| crate::ImportedSite {
        url: url.into(),
        title: title.into(),
    };
    let (send, receive) = std::sync::mpsc::channel();
    let destination = shell.import_destination().unwrap();
    shell.handle(Command::Import {
        destination,
        work: Box::new(crate::ImportWork::Essentials(vec![
            site("https://mail.example/", "Mail"),
            site("https://mail.example/", "Mail again"),
            site("https://app.slack.com/client", "Slack"),
            site("javascript:alert(1)", "Script"),
            site("https://board.example/", "Board"),
        ])),
        done: crate::ImportCompletion::new(move |added| {
            let _ = send.send(added);
        }),
    });
    assert_eq!(receive.recv().unwrap(), Some(2));
    let sites = kept(&shell);
    assert_eq!(
        sites
            .iter()
            .map(|(_, url)| url.as_str())
            .collect::<Vec<_>>(),
        vec![
            "https://app.slack.com/client",
            "https://mail.example/",
            "https://board.example/"
        ]
    );
    assert!(sites
        .iter()
        .all(|(id, _)| !shell.items.tab(*id).unwrap().has_view()));
    assert_eq!(active_id(&screen), active);
}

#[test]
fn data_safety_import_retains_its_admitted_profile_across_focus_changes() {
    let (mut shell, _engine, _screen) = setup();
    shell.handle(Command::Bootstrap);
    let (send, receive) = std::sync::mpsc::channel();
    shell.handle(Command::ImportDestination {
        done: crate::ImportDestinationCompletion::new(move |profile| {
            send.send(profile).unwrap();
        }),
    });
    let admitted = receive.recv().unwrap().unwrap();
    let other = add_inactive_named_profile(&mut shell, 89_000);
    for (index, focused) in [admitted.profile, other].into_iter().enumerate() {
        shell.windows.focused_mut().unwrap().profile = focused;
        shell.handle(Command::Import {
            destination: admitted,
            work: Box::new(crate::ImportWork::Essentials(vec![crate::ImportedSite {
                url: format!("https://kind{index}.example/"),
                title: "Imported".into(),
            }])),
            done: crate::ImportCompletion::new(|added| assert_eq!(added, Some(1))),
        });
    }
    assert_eq!(
        shell
            .items
            .roots(Placement::Favorites {
                space: admitted.space
            })
            .len(),
        2
    );
    assert!(shell
        .items
        .roots(Placement::Favorites {
            space: shell
                .spaces
                .iter()
                .find(|space| space.profile == other)
                .unwrap()
                .id
        })
        .is_empty());
}

#[test]
fn import_rejects_private_and_removed_destinations_without_retargeting() {
    let (mut shell, _engine, _screen) = setup();
    shell.handle(Command::Bootstrap);
    let destination = add_inactive_named_profile(&mut shell, 89_200);
    let destination = crate::ImportDestination {
        profile: destination,
        space: shell
            .spaces
            .iter()
            .find(|space| space.profile == destination)
            .unwrap()
            .id,
    };
    shell.profiles.remove(destination.profile);
    let (send, receive) = std::sync::mpsc::channel();
    shell.handle(Command::Import {
        destination,
        work: Box::new(crate::ImportWork::Essentials(vec![])),
        done: crate::ImportCompletion::new(move |added| {
            send.send(added).unwrap();
        }),
    });
    assert_eq!(receive.recv().unwrap(), None);
    shell.handle(Command::Run("window.newPrivate".into()));
    assert_eq!(shell.import_destination(), None);
}

#[test]
fn all_queued_import_kinds_and_icons_keep_one_destination_after_focus_moves() {
    use zephium_core::bookmarks::{BookmarkReply, BookmarkRequest};
    use zephium_core::ports::store::ImportedVisit;
    let directory = tempfile::tempdir().unwrap();
    let store = Arc::new(zephium_store::SqliteStore::open(directory.path()).unwrap());
    let mut shell = Shell::new(
        Arc::new(FakeEngine::default()),
        store.clone(),
        Arc::new(FakeChrome),
        Box::new(|_| {}),
    );
    shell.handle(Command::SetWindowSize(Size::new(1200.0, 800.0)));
    shell.handle(Command::Bootstrap);
    let destination = shell.import_destination().unwrap();
    let other = add_inactive_named_profile(&mut shell, 89_400);
    shell.persist();
    assert!(store.flush_until(std::time::Instant::now() + std::time::Duration::from_secs(30)));
    let queue = crate::store_reads::StoreReadQueue::new();
    shell.store_reads = Some(queue.clone());
    let work = [
        crate::ImportWork::Bookmarks {
            folder: "Imported".into(),
            nodes: vec![zephium_core::bookmarks::ImportNode::Link {
                title: "Imported".into(),
                url: "https://bookmark.example/".into(),
            }],
        },
        crate::ImportWork::History(vec![ImportedVisit {
            url: "https://import.example/".into(),
            title: "Imported".into(),
            visited_at: 1,
        }]),
        crate::ImportWork::Icons(vec![(
            "https://import.example".into(),
            vec![0x7f; zephium_core::icon::RGBA32_BYTES],
        )]),
    ];
    // Source reads may finish after focus changes between any pair of kinds.
    for (index, work) in work.into_iter().enumerate() {
        shell.windows.focused_mut().unwrap().profile = if index == 0 {
            destination.profile
        } else {
            other
        };
        shell.handle(Command::Import {
            destination,
            work: Box::new(work),
            done: crate::ImportCompletion::new(|added| assert!(added.is_some())),
        });
    }
    let (send, receive) = std::sync::mpsc::channel();
    let worker_queue = queue.clone();
    let worker_store: SharedStore = store.clone();
    let worker = std::thread::spawn(move || {
        crate::store_reads::run_for_test(worker_store, worker_queue, send)
    });
    let results: Vec<_> = (0..3)
        .map(|_| {
            receive
                .recv_timeout(std::time::Duration::from_secs(10))
                .unwrap()
        })
        .collect();
    queue.stop();
    worker.join().unwrap();
    for result in results {
        shell.handle(Command::StoreRead(result));
    }
    assert!(
        matches!(store.bookmarks(destination.profile, BookmarkRequest::All), BookmarkReply::Nodes(nodes) if !nodes.is_empty())
    );
    assert!(
        matches!(store.bookmarks(other, BookmarkRequest::All), BookmarkReply::Nodes(nodes) if nodes.is_empty())
    );
    assert_eq!(
        store
            .history_page_checked(destination.profile, "", None, None, 10)
            .unwrap()
            .len(),
        1
    );
    assert!(store
        .history_page_checked(other, "", None, None, 10)
        .unwrap()
        .is_empty());
    assert!(store
        .favicon_raster_with_age(destination.profile, "https://import.example")
        .is_some());
    assert!(store
        .favicon_raster_with_age(other, "https://import.example")
        .is_none());
}
