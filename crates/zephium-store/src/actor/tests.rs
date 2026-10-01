use super::*;
use rusqlite::{params, Connection};
use zephium_core::blocker::{BlockerConfig, BlockerConfigRevision, ProfileBlockerConfig};
use zephium_core::ids::{ItemId, PagePermissionGrantId, SpaceId, UserscriptId};
use zephium_core::item::{Placement, SpaceSection};
use zephium_core::permissions::{
    PageOrigin, PagePermissionCatalogRevision, PagePermissionChange, PagePermissionGrantRevision,
    PagePermissionKind, PagePermissionPatch, RememberedPagePermission,
};
use zephium_core::profiles::ProfileKind;
use zephium_core::session::{
    PersistedClosedTab, PersistedItem, PersistedKind, PersistedProfile, PersistedSpace,
};
use zephium_core::split::{Axis, Pane};
use zephium_core::userscripts::{
    UserscriptCatalogMutation, UserscriptCatalogRevision, UserscriptRevision,
};

fn tab(id: u128, space: SpaceId, url: &str) -> PersistedItem {
    PersistedItem {
        id: ItemId::from(id),
        parent: None,
        placement: Placement::Space {
            space,
            section: SpaceSection::Today,
        },
        kind: PersistedKind::Tab {
            url: url.into(),
            title: "T".into(),
            zoom: 1.0,
        },
    }
}

fn rgba() -> Vec<u8> {
    vec![0x7f; zephium_core::icon::RGBA32_BYTES]
}

fn test_store_with_sender(tx: SyncSender<Cmd>) -> SqliteStore {
    let (_exit, exited) = mpsc::sync_channel(1);
    SqliteStore {
        resource_admission: Arc::new(AtomicUsize::new(0)),
        #[cfg(feature = "work-execution")]
        work_admission: OnceLock::new(),
        tx,
        latest_session: Arc::new(Mutex::new(None)),
        pending_visits: Arc::new(Mutex::new(PendingVisits::new())),
        pending_settings: Arc::new(Mutex::new(PendingSettings::default())),
        userscript_mutation_admission: Arc::new(Mutex::new(UserscriptMutationAdmission::default())),
        page_permission_mutation_admission: Arc::new(Mutex::new(
            PagePermissionMutationAdmission::default(),
        )),
        agent_audit_delivery_admission: OnceLock::new(),
        lifecycle: Mutex::new(ActorLifecycle {
            join: None,
            exited,
            terminal_admitted: false,
        }),
        shutdown_clean: AtomicBool::new(false),
    }
}

#[cfg(feature = "work-execution")]
#[test]
fn work_journal_mailbox_is_lazy_bounded_and_refusal_never_calls_completion() {
    use zephium_agentic::{
        AgentWorkArtifactRequest, AgentWorkIncarnation, AgentWorkJournalError,
        AgentWorkJournalPort, AgentWorkJournalRequest, AgentWorkRecord, AGENT_WORK_RECORD_BYTES,
    };
    let (tx, rx) = mpsc::sync_channel(8);
    let store = test_store_with_sender(tx);
    assert!(store.work_admission.get().is_none());
    let owner = AgentWorkIncarnation::generate();
    // Persisted fixture data only; no terminal/native authority is created.
    let mut bytes = [0; AGENT_WORK_RECORD_BYTES];
    bytes[0] = 1;
    bytes[1] = 1;
    bytes[2] = 63;
    bytes[15] = 1;
    bytes[16..32].copy_from_slice(&owner.bytes());
    bytes[47] = 1;
    bytes[63] = 1;
    let request = AgentWorkArtifactRequest::Read {
        owner,
        record: AgentWorkRecord::decode(bytes).unwrap(),
        profile: 1_u128.into(),
    };
    for index in 0..4 {
        let admitted = if index % 2 == 0 {
            store.dispatch(
                AgentWorkJournalRequest::Claim,
                Box::new(|_| panic!("not pumped")),
            )
        } else {
            store.artifact(request.clone(), Box::new(|_| panic!("not pumped")))
        };
        assert!(admitted.is_ok());
    }
    assert_eq!(
        store.artifact(request.clone(), Box::new(|_| panic!("refused callback"))),
        Err(AgentWorkJournalError::Capacity)
    );
    assert_eq!(
        store.dispatch(
            AgentWorkJournalRequest::Claim,
            Box::new(|_| panic!("refused callback"))
        ),
        Err(AgentWorkJournalError::Capacity)
    );
    drop(rx.recv().unwrap());
    assert!(store
        .dispatch(
            AgentWorkJournalRequest::Claim,
            Box::new(|_| panic!("not pumped"))
        )
        .is_ok());
    drop(rx);
    assert_eq!(
        store.work_admission.get().unwrap().load(Ordering::Acquire),
        0
    );
    assert_eq!(
        store.dispatch(
            AgentWorkJournalRequest::Claim,
            Box::new(|_| panic!("refused callback"))
        ),
        Err(AgentWorkJournalError::Shutdown)
    );
    assert_eq!(
        store.artifact(request, Box::new(|_| panic!("refused callback"))),
        Err(AgentWorkJournalError::Shutdown)
    );
}

#[cfg(all(
    feature = "work-execution",
    any(target_os = "macos", target_os = "linux")
))]
#[test]
fn work_journal_callback_loss_and_panic_do_not_reclaim_process_identity_or_kill_store() {
    let _process = crate::hub::work_test_guard();
    use zephium_agentic::{AgentWorkJournalPort, AgentWorkJournalReply, AgentWorkJournalRequest};
    let directory = tempfile::tempdir().unwrap();
    let store = SqliteStore::open(directory.path()).unwrap();
    // A lost acknowledgement keeps the Store's exact incarnation and lock.
    store
        .dispatch(AgentWorkJournalRequest::Claim, Box::new(|_| {}))
        .unwrap();
    store
        .dispatch(
            AgentWorkJournalRequest::Claim,
            Box::new(|_| panic!("fixture callback panic")),
        )
        .unwrap();
    let (tx, rx) = mpsc::sync_channel(1);
    store
        .dispatch(
            AgentWorkJournalRequest::Claim,
            Box::new(move |result| {
                let _ = tx.send(result);
            }),
        )
        .unwrap();
    let AgentWorkJournalReply::Claimed { owner, records } =
        rx.recv_timeout(Duration::from_secs(5)).unwrap().unwrap()
    else {
        panic!()
    };
    assert!(records.is_empty());
    let (tx, rx) = mpsc::sync_channel(1);
    store
        .dispatch(
            AgentWorkJournalRequest::Claim,
            Box::new(move |result| {
                let _ = tx.send(result);
            }),
        )
        .unwrap();
    assert!(
        matches!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), Ok(AgentWorkJournalReply::Claimed { owner: again, .. }) if again == owner)
    );
    assert_eq!(
        store.shutdown_until(Instant::now() + Duration::from_secs(2)),
        zephium_core::ports::store::StoreShutdownOutcome::Clean
    );
}

fn create_profile_file(dir: &Path, profile: ProfileId) -> std::path::PathBuf {
    let path = dir.join(format!("profile-{profile}.sqlite"));
    let mut conn = Connection::open(&path).unwrap();
    migrations::apply(&mut conn, migrations::PROFILE).unwrap();
    conn.execute(
        "INSERT INTO history(url, title, visited_at)
         VALUES ('https://recoverable.example/', 'Recoverable', 1)",
        [],
    )
    .unwrap();
    path
}

fn artifact_bytes(path: &Path) -> Vec<(String, Vec<u8>)> {
    ["", "-wal", "-shm"]
        .into_iter()
        .filter_map(|suffix| {
            let artifact = if suffix.is_empty() {
                path.to_path_buf()
            } else {
                let mut artifact = path.as_os_str().to_owned();
                artifact.push(suffix);
                std::path::PathBuf::from(artifact)
            };
            std::fs::read(artifact)
                .ok()
                .map(|bytes| (suffix.to_owned(), bytes))
        })
        .collect()
}

fn sample() -> SessionState {
    let profile = ProfileId::from(1);
    let space = SpaceId::from(2);
    let folder = ItemId::from(20);
    SessionState {
        profiles: vec![PersistedProfile {
            id: profile,
            name: "Personal".into(),
            kind: ProfileKind::Default,
        }],
        spaces: vec![PersistedSpace {
            id: space,
            profile,
            name: "Space".into(),
        }],
        items: vec![
            PersistedItem {
                id: folder,
                parent: None,
                placement: Placement::Space {
                    space,
                    section: SpaceSection::Pinned,
                },
                kind: PersistedKind::Folder {
                    name: "Work".into(),
                },
            },
            PersistedItem {
                id: ItemId::from(21),
                parent: Some(folder),
                placement: Placement::Space {
                    space,
                    section: SpaceSection::Pinned,
                },
                kind: PersistedKind::Tab {
                    url: "https://docs.rs/".into(),
                    title: "Docs".into(),
                    zoom: 1.5,
                },
            },
            tab(10, space, "https://example.com/"),
            tab(11, space, "https://github.com/"),
        ],
        active_space: Some(space),
        active_item: Some(ItemId::from(11)),
        splits: Some(Pane::Branch {
            axis: Axis::Row,
            ratio: 0.4,
            a: Box::new(Pane::Leaf(ItemId::from(10))),
            b: Box::new(Pane::Leaf(ItemId::from(11))),
        }),
        recently_closed: Vec::new(),
    }
}

fn two_profile_sample() -> SessionState {
    let mut state = sample();
    state.profiles.push(PersistedProfile {
        id: ProfileId::from(3),
        name: "Work".into(),
        kind: ProfileKind::Named,
    });
    state.spaces.push(PersistedSpace {
        id: SpaceId::from(4),
        profile: ProfileId::from(3),
        name: "Work".into(),
    });
    state
}

fn loaded(store: &impl Store) -> SessionState {
    match store.load_session() {
        SessionLoad::Loaded { state, .. } => state,
        other => panic!("expected loaded session, got {other:?}"),
    }
}

fn default_blocker_configs(state: &SessionState) -> Vec<ProfileBlockerConfig> {
    let mut configs: Vec<_> = state
        .profiles
        .iter()
        .map(|profile| ProfileBlockerConfig {
            profile: profile.id,
            revision: BlockerConfigRevision::INITIAL,
            config: BlockerConfig::default(),
        })
        .collect();
    configs.sort_unstable_by_key(|config| config.profile.to_string());
    configs
}

fn loaded_session(state: SessionState) -> SessionLoad {
    SessionLoad::Loaded {
        blocker_configs: default_blocker_configs(&state),
        state,
    }
}

fn userscript_source(name: &str) -> Arc<str> {
    format!(
        "// ==UserScript==\n// @name {name}\n// @match https://example.com/*\n// ==/UserScript==\n"
    )
    .into()
}

fn load_userscripts(store: &impl Store, profile: ProfileId) -> UserscriptCatalogLoadOutcome {
    let (reply, outcome) = mpsc::sync_channel(1);
    assert!(store.load_userscript_catalog(
        profile,
        Box::new(move |result| {
            let _ = reply.send(result);
        }),
    ));
    outcome.recv_timeout(STORE_RPC_TIMEOUT).unwrap()
}

fn mutate_userscripts(
    store: &impl Store,
    profile: ProfileId,
    expected: UserscriptCatalogRevision,
    mutation: UserscriptCatalogMutation,
) -> UserscriptCatalogMutationOutcome {
    let (reply, outcome) = mpsc::sync_channel(1);
    assert!(store.mutate_userscript_catalog(
        profile,
        expected,
        mutation,
        Box::new(move |result| {
            let _ = reply.send(result);
        }),
    ));
    outcome.recv_timeout(STORE_RPC_TIMEOUT).unwrap()
}

fn page_origin(value: &str) -> PageOrigin {
    PageOrigin::parse_exact(value).unwrap()
}

fn page_patch(changes: Vec<PagePermissionChange>) -> PagePermissionPatch {
    PagePermissionPatch::new(changes).unwrap()
}

fn load_page_permissions(
    store: &impl Store,
    profile: ProfileId,
) -> PagePermissionCatalogLoadOutcome {
    let (reply, outcome) = mpsc::sync_channel(1);
    assert!(store.load_page_permission_catalog(
        profile,
        Box::new(move |result| {
            let _ = reply.send(result);
        }),
    ));
    outcome.recv_timeout(STORE_RPC_TIMEOUT).unwrap()
}

fn mutate_page_permissions(
    store: &impl Store,
    profile: ProfileId,
    expected: PagePermissionCatalogRevision,
    patch: PagePermissionPatch,
) -> PagePermissionCatalogMutationOutcome {
    let (reply, outcome) = mpsc::sync_channel(1);
    assert!(store.mutate_page_permission_catalog(
        profile,
        expected,
        patch,
        Box::new(move |result| {
            let _ = reply.send(result);
        }),
    ));
    outcome.recv_timeout(STORE_RPC_TIMEOUT).unwrap()
}

#[test]
fn userscript_catalog_is_source_authoritative_durable_and_revision_checked() {
    let dir = tempfile::tempdir().unwrap();
    let profile = ProfileId::from(1);
    let id = UserscriptId::from(7);
    let catalog_revision_before_reopen;
    let script_revision_before_reopen;

    {
        let store = SqliteStore::open(dir.path()).unwrap();
        store.save_session(sample());
        assert!(store.flush());

        let UserscriptCatalogLoadOutcome::Loaded(initial) = load_userscripts(&store, profile)
        else {
            panic!("new profile did not expose an exact empty userscript catalog");
        };
        assert_eq!(initial.revision(), UserscriptCatalogRevision::INITIAL);
        assert!(initial.scripts().is_empty());

        assert_eq!(
            mutate_userscripts(
                &store,
                profile,
                initial.revision(),
                UserscriptCatalogMutation::Install {
                    id,
                    enabled: true,
                    source: "not a userscript".into(),
                },
            ),
            UserscriptCatalogMutationOutcome::Invalid
        );

        let installed = mutate_userscripts(
            &store,
            profile,
            initial.revision(),
            UserscriptCatalogMutation::Install {
                id,
                enabled: true,
                source: userscript_source("Installed"),
            },
        );
        let UserscriptCatalogMutationOutcome::Applied(installed) = installed else {
            panic!("valid install was not applied");
        };
        let installed_script = installed.script.unwrap();
        assert_eq!(installed.catalog_revision.get(), 2);
        assert_eq!(installed_script.revision, UserscriptRevision::INITIAL);
        assert_eq!(installed_script.metadata.name.as_ref(), "Installed");
        assert!(!installed_script.compatibility.executable);

        assert_eq!(
            mutate_userscripts(
                &store,
                profile,
                UserscriptCatalogRevision::INITIAL,
                UserscriptCatalogMutation::Delete {
                    id,
                    expected: UserscriptRevision::INITIAL,
                },
            ),
            UserscriptCatalogMutationOutcome::Conflict {
                current: installed.catalog_revision,
            }
        );
        assert_eq!(
            mutate_userscripts(
                &store,
                profile,
                installed.catalog_revision,
                UserscriptCatalogMutation::UpdateSource {
                    id,
                    expected: UserscriptRevision::new(2).unwrap(),
                    source: userscript_source("Stale"),
                },
            ),
            UserscriptCatalogMutationOutcome::Invalid
        );

        let updated = mutate_userscripts(
            &store,
            profile,
            installed.catalog_revision,
            UserscriptCatalogMutation::UpdateSource {
                id,
                expected: UserscriptRevision::INITIAL,
                source: userscript_source("Updated"),
            },
        );
        let UserscriptCatalogMutationOutcome::Applied(updated) = updated else {
            panic!("valid source update was not applied");
        };
        let updated_script = updated.script.unwrap();
        assert_eq!(updated.catalog_revision.get(), 3);
        assert_eq!(updated_script.revision.get(), 2);
        assert_eq!(updated_script.metadata.name.as_ref(), "Updated");

        let unchanged = mutate_userscripts(
            &store,
            profile,
            updated.catalog_revision,
            UserscriptCatalogMutation::SetEnabled {
                id,
                expected: updated_script.revision,
                enabled: true,
            },
        );
        let UserscriptCatalogMutationOutcome::Applied(unchanged) = unchanged else {
            panic!("idempotent toggle was not acknowledged");
        };
        assert_eq!(unchanged.catalog_revision, updated.catalog_revision);
        assert_eq!(unchanged.script.unwrap().revision, updated_script.revision);

        let disabled = mutate_userscripts(
            &store,
            profile,
            updated.catalog_revision,
            UserscriptCatalogMutation::SetEnabled {
                id,
                expected: updated_script.revision,
                enabled: false,
            },
        );
        let UserscriptCatalogMutationOutcome::Applied(disabled) = disabled else {
            panic!("valid toggle was not applied");
        };
        let disabled_script = disabled.script.unwrap();
        assert_eq!(disabled.catalog_revision.get(), 4);
        assert_eq!(disabled_script.revision.get(), 3);
        assert!(!disabled_script.enabled);
        catalog_revision_before_reopen = disabled.catalog_revision;
        script_revision_before_reopen = disabled_script.revision;
        assert!(store.flush());
        assert_eq!(
            store.shutdown_until(Instant::now() + STORE_RPC_TIMEOUT),
            StoreShutdownOutcome::Clean
        );
    }

    let store = SqliteStore::open(dir.path()).unwrap();
    let UserscriptCatalogLoadOutcome::Loaded(reopened) = load_userscripts(&store, profile) else {
        panic!("durable catalog could not be reopened");
    };
    assert_eq!(reopened.revision(), catalog_revision_before_reopen);
    assert_eq!(reopened.scripts().len(), 1);
    let reopened_script = &reopened.scripts()[0];
    assert_eq!(reopened_script.id, id);
    assert_eq!(reopened_script.revision, script_revision_before_reopen);
    assert_eq!(reopened_script.metadata.name.as_ref(), "Updated");
    assert!(!reopened_script.enabled);

    let deleted = mutate_userscripts(
        &store,
        profile,
        reopened.revision(),
        UserscriptCatalogMutation::Delete {
            id,
            expected: reopened_script.revision,
        },
    );
    let UserscriptCatalogMutationOutcome::Applied(deleted) = deleted else {
        panic!("valid deletion was not applied");
    };
    assert!(deleted.script.is_none());
    let UserscriptCatalogLoadOutcome::Loaded(empty) = load_userscripts(&store, profile) else {
        panic!("deleted catalog could not be loaded");
    };
    assert_eq!(empty.revision(), deleted.catalog_revision);
    assert!(empty.scripts().is_empty());
}

#[test]
fn userscript_mutation_admission_is_independently_bounded_and_exact() {
    let (tx, rx) = mpsc::sync_channel(8);
    let store = test_store_with_sender(tx);
    let profile = ProfileId::from(1);

    for id in 1..=MAX_PENDING_USERSCRIPT_MUTATIONS {
        assert!(store.mutate_userscript_catalog(
            profile,
            UserscriptCatalogRevision::INITIAL,
            UserscriptCatalogMutation::Install {
                id: UserscriptId::from(id as u128),
                enabled: true,
                source: userscript_source("Queued"),
            },
            Box::new(|_| {}),
        ));
    }
    assert!(!store.mutate_userscript_catalog(
        profile,
        UserscriptCatalogRevision::INITIAL,
        UserscriptCatalogMutation::Install {
            id: UserscriptId::from(99),
            enabled: true,
            source: userscript_source("Refused"),
        },
        Box::new(|_| {}),
    ));

    drop(rx.recv_timeout(STORE_RPC_TIMEOUT).unwrap());
    assert!(store.mutate_userscript_catalog(
        profile,
        UserscriptCatalogRevision::INITIAL,
        UserscriptCatalogMutation::Install {
            id: UserscriptId::from(100),
            enabled: true,
            source: userscript_source("Admitted after release"),
        },
        Box::new(|_| {}),
    ));

    drop(rx);
    let admission = store
        .userscript_mutation_admission
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    assert_eq!(admission.count, 0);
    assert_eq!(admission.source_bytes, 0);
}

#[test]
fn oversized_userscript_source_is_refused_without_callback_transfer() {
    let (tx, _rx) = mpsc::sync_channel(1);
    let store = test_store_with_sender(tx);
    let (reply, outcome) = mpsc::sync_channel(1);
    assert!(!store.mutate_userscript_catalog(
        ProfileId::from(1),
        UserscriptCatalogRevision::INITIAL,
        UserscriptCatalogMutation::Install {
            id: UserscriptId::from(1),
            enabled: true,
            source: "x"
                .repeat(zephium_core::ports::engine::MAX_USER_SCRIPT_BYTES + 1)
                .into(),
        },
        Box::new(move |result| {
            let _ = reply.send(result);
        }),
    ));
    assert!(matches!(
        outcome.recv_timeout(Duration::from_millis(20)),
        Err(mpsc::RecvTimeoutError::Disconnected)
    ));
}

#[test]
fn page_permission_catalog_is_atomic_revision_checked_noop_stable_and_durable() {
    let dir = tempfile::tempdir().unwrap();
    let profile = ProfileId::from(1);
    let camera_id = PagePermissionGrantId::from(41);
    let microphone_id = PagePermissionGrantId::from(42);
    let origin = page_origin("https://permissions.example");
    let final_revision;

    {
        let store = SqliteStore::open(dir.path()).unwrap();
        store.save_session(sample());
        assert!(store.flush());
        let PagePermissionCatalogLoadOutcome::Loaded(initial) =
            load_page_permissions(&store, profile)
        else {
            panic!("new profile did not expose an exact empty permission catalog");
        };
        assert_eq!(initial.revision(), PagePermissionCatalogRevision::INITIAL);
        assert!(initial.grants().is_empty());

        let created = mutate_page_permissions(
            &store,
            profile,
            initial.revision(),
            page_patch(vec![
                PagePermissionChange::Create {
                    id: camera_id,
                    origin: origin.clone(),
                    kind: PagePermissionKind::Camera,
                    decision: RememberedPagePermission::Allow,
                },
                PagePermissionChange::Create {
                    id: microphone_id,
                    origin: origin.clone(),
                    kind: PagePermissionKind::Microphone,
                    decision: RememberedPagePermission::Allow,
                },
            ]),
        );
        let PagePermissionCatalogMutationOutcome::Applied(created) = created else {
            panic!("atomic camera/microphone create was not applied");
        };
        assert_eq!(created.catalog_revision.get(), 2);
        assert_eq!(created.results.as_slice().len(), 2);
        assert!(created.results.as_slice().iter().all(|result| {
            result
                .grant
                .as_ref()
                .is_some_and(|grant| grant.revision == PagePermissionGrantRevision::INITIAL)
        }));

        assert_eq!(
            mutate_page_permissions(
                &store,
                profile,
                PagePermissionCatalogRevision::INITIAL,
                page_patch(vec![PagePermissionChange::Delete {
                    id: camera_id,
                    expected: PagePermissionGrantRevision::INITIAL,
                }]),
            ),
            PagePermissionCatalogMutationOutcome::Conflict {
                current: created.catalog_revision,
            }
        );

        let no_op = mutate_page_permissions(
            &store,
            profile,
            created.catalog_revision,
            page_patch(vec![PagePermissionChange::Update {
                id: camera_id,
                expected: PagePermissionGrantRevision::INITIAL,
                decision: RememberedPagePermission::Allow,
            }]),
        );
        let PagePermissionCatalogMutationOutcome::Applied(no_op) = no_op else {
            panic!("idempotent remembered decision was not acknowledged");
        };
        assert_eq!(no_op.catalog_revision, created.catalog_revision);
        assert_eq!(
            no_op.results.as_slice()[0].grant.as_ref().unwrap().revision,
            PagePermissionGrantRevision::INITIAL
        );

        let updated = mutate_page_permissions(
            &store,
            profile,
            created.catalog_revision,
            page_patch(vec![
                PagePermissionChange::Update {
                    id: camera_id,
                    expected: PagePermissionGrantRevision::INITIAL,
                    decision: RememberedPagePermission::Deny,
                },
                PagePermissionChange::Update {
                    id: microphone_id,
                    expected: PagePermissionGrantRevision::INITIAL,
                    decision: RememberedPagePermission::Deny,
                },
            ]),
        );
        let PagePermissionCatalogMutationOutcome::Applied(updated) = updated else {
            panic!("atomic camera/microphone update was not applied");
        };
        assert_eq!(updated.catalog_revision.get(), 3);
        assert!(updated.results.as_slice().iter().all(|result| {
            result.grant.as_ref().is_some_and(|grant| {
                grant.revision.get() == 2 && grant.decision == RememberedPagePermission::Deny
            })
        }));
        final_revision = updated.catalog_revision;
        assert_eq!(
            store.shutdown_until(Instant::now() + STORE_RPC_TIMEOUT),
            StoreShutdownOutcome::Clean
        );
    }

    let store = SqliteStore::open(dir.path()).unwrap();
    let PagePermissionCatalogLoadOutcome::Loaded(reopened) = load_page_permissions(&store, profile)
    else {
        panic!("durable page-permission catalog could not be reopened");
    };
    assert_eq!(reopened.revision(), final_revision);
    assert_eq!(reopened.grants().len(), 2);
    assert!(reopened.grants().iter().all(|grant| {
        grant.revision.get() == 2 && grant.decision == RememberedPagePermission::Deny
    }));
}

#[test]
fn page_permission_authority_replacement_is_patch_order_independent() {
    for create_first in [true, false] {
        let store = SqliteStore::in_memory().unwrap();
        let profile = ProfileId::from(1);
        let old_id = PagePermissionGrantId::from(51);
        let new_id = PagePermissionGrantId::from(52);
        let origin = page_origin("https://replace.example");
        store.save_session(sample());
        assert!(store.flush());

        let created = mutate_page_permissions(
            &store,
            profile,
            PagePermissionCatalogRevision::INITIAL,
            page_patch(vec![PagePermissionChange::Create {
                id: old_id,
                origin: origin.clone(),
                kind: PagePermissionKind::Notifications,
                decision: RememberedPagePermission::Allow,
            }]),
        );
        let PagePermissionCatalogMutationOutcome::Applied(created) = created else {
            panic!("replacement fixture was not created");
        };
        let create = PagePermissionChange::Create {
            id: new_id,
            origin: origin.clone(),
            kind: PagePermissionKind::Notifications,
            decision: RememberedPagePermission::Deny,
        };
        let delete = PagePermissionChange::Delete {
            id: old_id,
            expected: PagePermissionGrantRevision::INITIAL,
        };
        let changes = if create_first {
            vec![create, delete]
        } else {
            vec![delete, create]
        };
        let replaced = mutate_page_permissions(
            &store,
            profile,
            created.catalog_revision,
            page_patch(changes),
        );
        let PagePermissionCatalogMutationOutcome::Applied(replaced) = replaced else {
            panic!("authority replacement failed for create_first={create_first}");
        };
        let expected_result_ids = if create_first {
            vec![new_id, old_id]
        } else {
            vec![old_id, new_id]
        };
        assert_eq!(
            replaced
                .results
                .as_slice()
                .iter()
                .map(|result| result.id)
                .collect::<Vec<_>>(),
            expected_result_ids,
            "response order drifted from patch order"
        );
        let PagePermissionCatalogLoadOutcome::Loaded(catalog) =
            load_page_permissions(&store, profile)
        else {
            panic!("replaced authority could not be loaded");
        };
        assert_eq!(catalog.grants().len(), 1);
        assert_eq!(catalog.grants()[0].id, new_id);
        assert_eq!(catalog.grants()[0].origin, origin);
        assert_eq!(catalog.grants()[0].decision, RememberedPagePermission::Deny);
    }
}

#[test]
fn page_permission_commit_ambiguity_requires_exact_load_reconciliation() {
    let profile = ProfileId::from(1);
    let id = PagePermissionGrantId::from(61);
    let mut hub = Hub::in_memory().unwrap();
    hub.save(&sample()).unwrap();
    hub.make_next_page_permission_commit_ambiguous();
    let store = SqliteStore::spawn(hub).unwrap();

    assert_eq!(
        mutate_page_permissions(
            &store,
            profile,
            PagePermissionCatalogRevision::INITIAL,
            page_patch(vec![PagePermissionChange::Create {
                id,
                origin: page_origin("https://ambiguous.example"),
                kind: PagePermissionKind::Geolocation,
                decision: RememberedPagePermission::Deny,
            }]),
        ),
        PagePermissionCatalogMutationOutcome::OutcomeUnknown
    );
    let PagePermissionCatalogLoadOutcome::Loaded(reconciled) =
        load_page_permissions(&store, profile)
    else {
        panic!("ambiguous commit could not be reconciled");
    };
    assert_eq!(reconciled.revision().get(), 2);
    assert_eq!(reconciled.grants().len(), 1);
    assert_eq!(reconciled.grants()[0].id, id);
}

#[test]
fn page_permission_store_admission_is_independently_bounded_and_exact() {
    let (tx, rx) = mpsc::sync_channel(MAX_PENDING_PAGE_PERMISSION_MUTATIONS + 1);
    let store = test_store_with_sender(tx);
    let profile = ProfileId::from(1);

    for id in 1..=MAX_PENDING_PAGE_PERMISSION_MUTATIONS {
        assert!(store.mutate_page_permission_catalog(
            profile,
            PagePermissionCatalogRevision::INITIAL,
            page_patch(vec![PagePermissionChange::Delete {
                id: PagePermissionGrantId::from(id as u128),
                expected: PagePermissionGrantRevision::INITIAL,
            }]),
            Box::new(|_| {}),
        ));
    }
    assert!(!store.mutate_page_permission_catalog(
        profile,
        PagePermissionCatalogRevision::INITIAL,
        page_patch(vec![PagePermissionChange::Delete {
            id: PagePermissionGrantId::from(99),
            expected: PagePermissionGrantRevision::INITIAL,
        }]),
        Box::new(|_| {}),
    ));

    drop(rx.recv_timeout(STORE_RPC_TIMEOUT).unwrap());
    assert!(store.mutate_page_permission_catalog(
        profile,
        PagePermissionCatalogRevision::INITIAL,
        page_patch(vec![PagePermissionChange::Delete {
            id: PagePermissionGrantId::from(100),
            expected: PagePermissionGrantRevision::INITIAL,
        }]),
        Box::new(|_| {}),
    ));
    drop(rx);
    assert_eq!(
        store
            .page_permission_mutation_admission
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .count,
        0
    );
}

#[test]
fn page_permission_unknown_ephemeral_terminal_and_full_queue_paths_fail_closed() {
    let dir = tempfile::tempdir().unwrap();
    let store = SqliteStore::open(dir.path()).unwrap();
    store.save_session(sample());
    assert!(store.flush());
    let ephemeral = ProfileId::from(700);
    assert_eq!(
        load_page_permissions(&store, ephemeral),
        PagePermissionCatalogLoadOutcome::NotRegistered
    );
    assert_eq!(
        mutate_page_permissions(
            &store,
            ephemeral,
            PagePermissionCatalogRevision::INITIAL,
            page_patch(vec![PagePermissionChange::Create {
                id: PagePermissionGrantId::from(701),
                origin: page_origin("https://private.example"),
                kind: PagePermissionKind::ClipboardRead,
                decision: RememberedPagePermission::Allow,
            }]),
        ),
        PagePermissionCatalogMutationOutcome::NotRegistered
    );
    assert!(!dir
        .path()
        .join(format!("profile-{ephemeral}.sqlite"))
        .exists());

    store.lifecycle.lock().unwrap().terminal_admitted = true;
    let completions = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let callback_completions = completions.clone();
    assert!(!store.load_page_permission_catalog(
        ProfileId::from(1),
        Box::new(move |_| {
            callback_completions.fetch_add(1, Ordering::Relaxed);
        }),
    ));
    assert_eq!(completions.load(Ordering::Relaxed), 0);
    store.lifecycle.lock().unwrap().terminal_admitted = false;

    let (tx, _rx) = mpsc::sync_channel(0);
    let full = std::mem::ManuallyDrop::new(test_store_with_sender(tx));
    let callback_completions = completions.clone();
    assert!(!full.load_page_permission_catalog(
        ProfileId::from(1),
        Box::new(move |_| {
            callback_completions.fetch_add(1, Ordering::Relaxed);
        }),
    ));
    assert_eq!(completions.load(Ordering::Relaxed), 0);
}

#[test]
fn roundtrip_tree_folders_focus_and_splits() {
    let store = SqliteStore::in_memory().unwrap();
    assert_eq!(store.load_session(), SessionLoad::Absent);
    let session = sample();
    store.save_session(session.clone());
    assert_eq!(loaded(&store), session);
}

#[test]
fn browser_owned_tab_roundtrips_as_typed_page_without_a_url() {
    let store = SqliteStore::in_memory().unwrap();
    let mut session = sample();
    let id = ItemId::from(72);
    session.items.push(PersistedItem {
        id,
        parent: None,
        placement: Placement::Space {
            space: session.spaces[0].id,
            section: SpaceSection::Today,
        },
        kind: PersistedKind::BrowserTab {
            page: zephium_core::item::BrowserOwnedTab::Extensions,
        },
    });
    session.active_item = Some(id);
    session.splits = None;
    store.save_session(session.clone());
    assert_eq!(loaded(&store), session);
}

#[test]
fn recently_closed_tabs_roundtrip_in_the_authoritative_bounded_snapshot() {
    let store = SqliteStore::in_memory().unwrap();
    let mut session = sample();
    session.recently_closed.push(PersistedClosedTab {
        profile: session.profiles[0].id,
        space: session.spaces[0].id,
        url: "https://closed.example/path".into(),
        title: "Closed tab".into(),
        zoom: 1.25,
        session_id: Some(zephium_core::ids::ClosedSessionId::from(42)),
        closed_at_ms: Some(1_700_000_000_123),
    });
    store.save_session(session.clone());
    assert_eq!(loaded(&store), session);
}

#[test]
fn blocker_preferences_load_with_the_authoritative_session_and_default_enabled() {
    let store = SqliteStore::in_memory().unwrap();
    let session = two_profile_sample();
    store.save_session(session.clone());

    assert_eq!(store.load_session(), loaded_session(session));
}

#[test]
fn blocker_preference_update_is_durable_monotonic_compare_and_swap() {
    let store = SqliteStore::in_memory().unwrap();
    let session = sample();
    let profile = session.profiles[0].id;
    store.save_session(session.clone());
    assert!(store.flush());

    let (updated_tx, updated_rx) = mpsc::channel();
    assert!(store.update_profile_blocker_config(
        profile,
        BlockerConfigRevision::INITIAL,
        BlockerConfig { enabled: true },
        Box::new(move |outcome| {
            updated_tx.send(outcome).unwrap();
        }),
    ));
    let revision = BlockerConfigRevision::INITIAL.next().unwrap();
    let updated = ProfileBlockerConfig {
        profile,
        revision,
        config: BlockerConfig { enabled: true },
    };
    assert_eq!(
        updated_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
        BlockerConfigUpdateOutcome::Updated(updated)
    );
    assert!(
        updated_rx.try_recv().is_err(),
        "one admitted update completed more than once"
    );
    assert_eq!(
        store.load_session(),
        SessionLoad::Loaded {
            state: session,
            blocker_configs: vec![updated],
        }
    );

    let (conflict_tx, conflict_rx) = mpsc::channel();
    assert!(store.update_profile_blocker_config(
        profile,
        BlockerConfigRevision::INITIAL,
        BlockerConfig { enabled: false },
        Box::new(move |outcome| {
            conflict_tx.send(outcome).unwrap();
        }),
    ));
    assert_eq!(
        conflict_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
        BlockerConfigUpdateOutcome::Conflict(updated)
    );
}

#[test]
fn blocker_preference_update_survives_a_clean_process_boundary() {
    let dir = tempfile::tempdir().unwrap();
    let session = sample();
    let profile = session.profiles[0].id;
    let revision = BlockerConfigRevision::INITIAL.next().unwrap();
    {
        let store = SqliteStore::open(dir.path()).unwrap();
        store.save_session(session.clone());
        assert!(store.flush());
        let (done, completed) = mpsc::channel();
        assert!(store.update_profile_blocker_config(
            profile,
            BlockerConfigRevision::INITIAL,
            BlockerConfig { enabled: false },
            Box::new(move |outcome| {
                done.send(outcome).unwrap();
            }),
        ));
        assert!(matches!(
            completed.recv_timeout(Duration::from_secs(1)).unwrap(),
            BlockerConfigUpdateOutcome::Updated(_)
        ));
        assert_eq!(
            store.shutdown_until(Instant::now() + Duration::from_secs(2)),
            StoreShutdownOutcome::Clean
        );
    }

    let store = SqliteStore::open(dir.path()).unwrap();
    assert_eq!(
        store.load_session(),
        SessionLoad::Loaded {
            state: session,
            blocker_configs: vec![ProfileBlockerConfig {
                profile,
                revision,
                config: BlockerConfig { enabled: false },
            }],
        }
    );
}

#[test]
fn blocker_preference_update_rejects_unknown_profiles_and_terminal_admission() {
    let store = SqliteStore::in_memory().unwrap();
    store.save_session(sample());
    assert!(store.flush());

    let (unknown_tx, unknown_rx) = mpsc::channel();
    assert!(store.update_profile_blocker_config(
        ProfileId::from(999),
        BlockerConfigRevision::INITIAL,
        BlockerConfig { enabled: true },
        Box::new(move |outcome| {
            unknown_tx.send(outcome).unwrap();
        }),
    ));
    assert_eq!(
        unknown_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
        BlockerConfigUpdateOutcome::NotRegistered
    );

    store.lifecycle.lock().unwrap().terminal_admitted = true;
    let completions = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let callback_completions = completions.clone();
    assert!(!store.update_profile_blocker_config(
        ProfileId::from(1),
        BlockerConfigRevision::INITIAL,
        BlockerConfig { enabled: true },
        Box::new(move |_| {
            callback_completions.fetch_add(1, Ordering::Relaxed);
        }),
    ));
    thread::sleep(Duration::from_millis(10));
    assert_eq!(completions.load(Ordering::Relaxed), 0);
    store.lifecycle.lock().unwrap().terminal_admitted = false;
}

#[test]
fn blocker_preference_reconciliation_reads_one_exact_authoritative_row() {
    let store = SqliteStore::in_memory().unwrap();
    let session = sample();
    let profile = session.profiles[0].id;
    store.save_session(session);
    assert!(store.flush());

    let (loaded_tx, loaded_rx) = mpsc::channel();
    assert!(store.load_profile_blocker_config(
        profile,
        Box::new(move |outcome| loaded_tx.send(outcome).unwrap()),
    ));
    assert_eq!(
        loaded_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
        BlockerConfigLoadOutcome::Loaded(ProfileBlockerConfig {
            profile,
            revision: BlockerConfigRevision::INITIAL,
            config: BlockerConfig::default(),
        })
    );
    assert!(loaded_rx.try_recv().is_err());

    let (unknown_tx, unknown_rx) = mpsc::channel();
    assert!(store.load_profile_blocker_config(
        ProfileId::from(999),
        Box::new(move |outcome| unknown_tx.send(outcome).unwrap()),
    ));
    assert_eq!(
        unknown_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
        BlockerConfigLoadOutcome::NotRegistered
    );
}

#[test]
fn blocker_preference_reconciliation_rejects_terminal_and_full_queue_admission() {
    let store = SqliteStore::in_memory().unwrap();
    store.lifecycle.lock().unwrap().terminal_admitted = true;
    let completions = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let callback_completions = completions.clone();
    assert!(!store.load_profile_blocker_config(
        ProfileId::from(1),
        Box::new(move |_| {
            callback_completions.fetch_add(1, Ordering::Relaxed);
        }),
    ));
    assert_eq!(completions.load(Ordering::Relaxed), 0);

    let (tx, _rx) = mpsc::sync_channel(0);
    let full_store = std::mem::ManuallyDrop::new(test_store_with_sender(tx));
    let callback_completions = completions.clone();
    assert!(!full_store.load_profile_blocker_config(
        ProfileId::from(1),
        Box::new(move |_| {
            callback_completions.fetch_add(1, Ordering::Relaxed);
        }),
    ));
    assert_eq!(completions.load(Ordering::Relaxed), 0);
}

#[test]
fn blocker_preference_update_reports_queue_non_admission_without_a_callback() {
    let (tx, _rx) = mpsc::sync_channel(0);
    let store = std::mem::ManuallyDrop::new(test_store_with_sender(tx));
    let completions = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let callback_completions = completions.clone();

    assert!(!store.update_profile_blocker_config(
        ProfileId::from(1),
        BlockerConfigRevision::INITIAL,
        BlockerConfig { enabled: true },
        Box::new(move |_| {
            callback_completions.fetch_add(1, Ordering::Relaxed);
        }),
    ));
    assert_eq!(completions.load(Ordering::Relaxed), 0);
}

#[test]
fn session_commit_preserves_survivors_and_defaults_only_new_profiles() {
    let mut hub = Hub::in_memory().unwrap();
    let first = sample();
    let first_profile = first.profiles[0].id;
    hub.save(&first).unwrap();
    let updated = hub
        .update_profile_blocker_config(
            first_profile,
            BlockerConfigRevision::INITIAL,
            BlockerConfig { enabled: false },
        )
        .unwrap();
    let BlockerConfigUpdateOutcome::Updated(updated) = updated else {
        panic!("blocker preference update did not settle")
    };

    let second = two_profile_sample();
    hub.save(&second).unwrap();
    assert_eq!(
        hub.profile_blocker_configs().unwrap(),
        vec![
            updated,
            ProfileBlockerConfig {
                profile: ProfileId::from(3),
                revision: BlockerConfigRevision::INITIAL,
                config: BlockerConfig::default(),
            },
        ]
    );
}

#[test]
fn empty_tab_session_still_persists_profile_identity() {
    let store = SqliteStore::in_memory().unwrap();
    let mut session = sample();
    session.items.clear();
    session.active_item = None;
    session.splits = None;

    store.save_session(session.clone());
    assert_eq!(store.load_session(), loaded_session(session));
}

#[test]
fn adapter_rejects_incognito_even_if_a_caller_bypasses_core_snapshot() {
    let mut hub = Hub::in_memory().unwrap();
    let mut private = sample();
    private.profiles[0].kind = ProfileKind::Incognito;

    assert!(hub.save(&private).is_err());
    assert!(hub.load().unwrap().is_none());
}

#[test]
fn actor_boundary_retains_last_good_session_after_oversized_or_private_input() {
    let store = SqliteStore::in_memory().unwrap();
    let good = sample();
    store.save_session(good.clone());
    assert_eq!(store.load_session(), loaded_session(good.clone()));

    let mut private = good.clone();
    private.profiles[0].kind = ProfileKind::Incognito;
    store.save_session(private);
    let mut oversized = good.clone();
    let PersistedKind::Tab { title, .. } = &mut oversized.items[2].kind else {
        panic!("sample item changed kind")
    };
    *title = "x".repeat(zephium_core::item::MAX_PAGE_TITLE_CHARS * 4 + 1);
    store.save_session(oversized);

    assert_eq!(store.load_session(), loaded_session(good));
}

#[test]
fn actor_boundary_rejects_recursive_or_nonfinite_programmatic_state() {
    let store = SqliteStore::in_memory().unwrap();
    let good = sample();
    store.save_session(good.clone());
    assert_eq!(store.load_session(), loaded_session(good.clone()));

    let mut too_deep = good.clone();
    let mut split = Pane::Leaf(ItemId::from(10));
    for _ in 0..=MAX_SPLIT_DEPTH {
        split = Pane::Branch {
            axis: Axis::Row,
            ratio: 0.5,
            a: Box::new(split),
            b: Box::new(Pane::Leaf(ItemId::from(11))),
        };
    }
    too_deep.splits = Some(split);
    store.save_session(too_deep);

    let mut nonfinite = good.clone();
    let PersistedKind::Tab { zoom, .. } = &mut nonfinite.items[2].kind else {
        panic!("sample item changed kind")
    };
    *zoom = f64::NAN;
    store.save_session(nonfinite);

    assert_eq!(store.load_session(), loaded_session(good));
}

#[test]
fn debounce_coalesces_latest_wins() {
    let store = SqliteStore::in_memory().unwrap();
    let mut second = sample();
    second.active_item = Some(ItemId::from(10));
    store.save_session(sample());
    store.save_session(second.clone());
    // load flushes the pending write, so it must observe the LAST save
    assert_eq!(loaded(&store), second);
}

#[test]
fn explicit_flush_is_an_ordered_durability_barrier() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(SqliteStore::open(dir.path()).unwrap());
    let mut latest = sample();
    latest.active_item = Some(ItemId::from(10));
    store.save_session(sample());
    store.save_session(latest.clone());

    assert!(store.flush());
    // Observe through independent connections so this assertion cannot be
    // satisfied by the actor's in-memory pending snapshot.
    let mut observer = Hub::open(dir.path().to_path_buf()).unwrap();
    assert_eq!(observer.load().unwrap(), Some(latest));
    assert!(store.flush(), "an empty repeated barrier is idempotent");
}

#[test]
fn terminal_shutdown_flushes_drops_sqlite_and_joins_the_actor() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(SqliteStore::open(dir.path()).unwrap());
    let latest = sample();
    store.save_session(latest.clone());

    assert_eq!(
        store.shutdown_until(Instant::now() + Duration::from_secs(2)),
        StoreShutdownOutcome::Clean
    );
    assert!(store.shutdown_clean.load(Ordering::Acquire));
    assert!(store.lifecycle.lock().unwrap().join.is_none());
    assert_eq!(
        store.shutdown_until(Instant::now() + Duration::from_secs(2)),
        StoreShutdownOutcome::Clean,
        "a proven terminal shutdown is idempotent"
    );

    let mut observer = Hub::open(dir.path().to_path_buf()).unwrap();
    assert_eq!(observer.load().unwrap(), Some(latest));
}

#[test]
fn flush_deadline_bounds_actor_queue_admission() {
    let (tx, _rx) = mpsc::sync_channel(0);
    let store = std::mem::ManuallyDrop::new(test_store_with_sender(tx));
    let started = Instant::now();

    assert!(!store.flush_until(started + Duration::from_millis(20)));
    assert!(started.elapsed() < Duration::from_millis(250));
}

#[test]
fn shutdown_deadline_bounds_actor_queue_admission_without_claiming_terminal_state() {
    let (tx, _rx) = mpsc::sync_channel(0);
    let store = std::mem::ManuallyDrop::new(test_store_with_sender(tx));
    let started = Instant::now();

    assert_eq!(
        store.shutdown_until(started + Duration::from_millis(20)),
        StoreShutdownOutcome::RetryableFailure
    );
    assert!(!store.lifecycle.lock().unwrap().terminal_admitted);
    assert!(!store.shutdown_clean.load(Ordering::Acquire));
    assert!(started.elapsed() < Duration::from_millis(250));
}

#[test]
fn save_rejects_noncanonical_state_instead_of_reducing_it() {
    let mut hub = Hub::in_memory().unwrap();
    let good = sample();
    hub.save(&good).unwrap();
    let mut invalid = good.clone();
    invalid.active_item = Some(ItemId::from(999_999));

    assert!(hub.save(&invalid).is_err());
    assert_eq!(hub.load().unwrap(), Some(good));
}

#[test]
fn non_save_traffic_cannot_starve_pending_session() {
    let dir = tempfile::tempdir().unwrap();
    let store = SqliteStore::open(dir.path()).unwrap();
    store.save_session(sample());

    let start = Instant::now();
    while start.elapsed() < MAX_PENDING_AGE + Duration::from_millis(250) {
        assert!(store.set_app_setting("pulse".into(), "1".into()));
        // The synchronous read proves the actor consumed the non-save
        // command, continuously exercising its receive loop.
        assert_eq!(store.app_setting("pulse").as_deref(), Some("1"));
        thread::sleep(Duration::from_millis(100));
    }

    let meta = Connection::open(dir.path().join("meta.sqlite")).unwrap();
    let count: i64 = meta
        .query_row("SELECT COUNT(*) FROM profiles", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 1, "pending session exceeded its maximum age");
}

#[test]
fn reopen_from_disk_survives_process_boundary() {
    let dir = tempfile::tempdir().unwrap();
    let session = sample();
    {
        let store = SqliteStore::open(dir.path()).unwrap();
        store.save_session(session.clone());
        // Process shutdown uses this explicit, deadline-bounded barrier;
        // dropping the last sender only triggers a detached best-effort
        // attempt and is deliberately not a synchronous durability API.
        assert!(store.flush());
    }
    let store = SqliteStore::open(dir.path()).unwrap();
    assert_eq!(loaded(&store), session);
    assert!(dir.path().join("meta.sqlite").exists());
    // Session truth is one atomic meta transaction; a profile database is
    // created lazily only when history/favicon data is first written.
    assert!(!dir
        .path()
        .join(format!("profile-{}.sqlite", ProfileId::from(1)))
        .exists());
}

#[test]
fn first_visit_lands_even_inside_the_save_debounce() {
    let store = SqliteStore::in_memory().unwrap();
    let profile = ProfileId::from(1);
    // save is still pending (debounced) when the visit arrives
    store.save_session(sample());
    store.record_visit(profile, "https://news.ycombinator.com/".into(), "HN".into());
    let hits = store.search_history(profile, "news", 10);
    assert_eq!(hits.len(), 1, "visit must not race the registry flush");
}

#[test]
fn failed_save_uses_capped_exponential_backoff_and_keeps_latest() {
    let started = Instant::now();
    let mut pending = PendingSession::new(sample(), started);
    assert_eq!(pending.deadline(), started + DEBOUNCE);

    for delay in [1, 2, 4, 8, 16, 30, 30] {
        pending.failed(started);
        assert_eq!(pending.deadline(), started + Duration::from_secs(delay));
    }

    let mut latest = sample();
    latest.active_item = None;
    let mailbox = Mutex::new(Some(latest.clone()));
    let retry_at = pending.retry_at;
    let mut slot = Some(pending);
    absorb_latest_session(&mailbox, &mut slot);
    let pending = slot.unwrap();
    assert_eq!(pending.state, latest);
    assert_eq!(pending.retry_at, retry_at, "new snapshots retain backoff");
}

#[test]
fn visit_requeue_preserves_concurrent_newer_value() {
    let profile = ProfileId::from(1);
    let same = (profile, "https://same.example/".to_owned());
    let other = (profile, "https://other.example/".to_owned());
    let mailbox = Mutex::new(PendingVisits::from([(same.clone(), "new".into())]));
    requeue_visits(
        &mailbox,
        PendingVisits::from([
            (same.clone(), "old".into()),
            (other.clone(), "other".into()),
        ]),
    );
    let mailbox = mailbox.lock().unwrap();
    assert_eq!(mailbox.get(&same).map(String::as_str), Some("new"));
    assert_eq!(mailbox.get(&other).map(String::as_str), Some("other"));
}

#[test]
fn failed_history_write_is_requeued_and_makes_flush_fail() {
    let profile = ProfileId::from(1);
    let mut hub = Hub::in_memory().unwrap();
    hub.save(&sample()).unwrap();
    hub.fail_history_writes(profile);
    let store = SqliteStore::spawn(hub).unwrap();
    store.record_visit(profile, "https://example.com/".into(), "Example".into());

    assert!(!store.flush());
    let mailbox = store.pending_visits.lock().unwrap();
    assert_eq!(
        mailbox
            .get(&(profile, "https://example.com/".into()))
            .map(String::as_str),
        Some("Example")
    );
}

#[test]
fn failed_setting_write_is_requeued_and_makes_flush_fail() {
    let mut hub = Hub::in_memory().unwrap();
    hub.fail_setting_writes();
    let store = SqliteStore::spawn(hub).unwrap();

    assert!(store.set_app_setting("keymap".into(), "custom".into()));
    assert!(!store.flush());
    let mailbox = store.pending_settings.lock().unwrap();
    assert_eq!(
        mailbox.pending.get("keymap").map(String::as_str),
        Some("custom")
    );
}

#[test]
fn best_effort_actor_calls_remain_bounded_at_a_full_queue() {
    let (tx, _rx) = mpsc::sync_channel(0);
    let store = std::mem::ManuallyDrop::new(test_store_with_sender(tx));
    let start = Instant::now();

    assert_eq!(store.app_setting("key"), None);
    assert!(store.set_app_setting("key".into(), "value".into()));
    assert!(store
        .search_history(ProfileId::from(1), "example", 10)
        .is_empty());
    assert_eq!(
        store.favicon_age(ProfileId::from(1), "https://example.com"),
        None
    );
    store.save_favicon(
        ProfileId::from(1),
        "https://example.com".into(),
        Some(zephium_core::icon::RGBA32_MIME.into()),
        rgba(),
    );
    assert_eq!(
        store.favicon_bytes(ProfileId::from(1), "https://example.com"),
        None
    );
    assert_eq!(
        store.favicon_raster_with_age(ProfileId::from(1), "https://example.com"),
        None
    );
    assert!(start.elapsed() < Duration::from_secs(1));
}

#[test]
fn setting_admission_reports_disconnected_actor() {
    let (tx, rx) = mpsc::sync_channel(1);
    drop(rx);
    let store = test_store_with_sender(tx);

    assert!(!store.set_app_setting("key".into(), "value".into()));
}

#[test]
fn visits_index_into_fts_and_unknown_profiles_are_ignored() {
    let mut hub = Hub::in_memory().unwrap();
    hub.save(&sample()).unwrap();
    let known = ProfileId::from(1);
    let unknown = ProfileId::from(99);

    hub.record_visit(known, "https://news.ycombinator.com/", "Hacker News");
    hub.record_visit(unknown, "https://example.com/", "Nope");

    assert_eq!(hub.history_count(known), 1);
    assert_eq!(hub.history_matches(known, "hacker"), 1);
    assert_eq!(hub.history_count(unknown), 0);
}

#[test]
fn history_search_prefix_dedupes_and_ranks_recent() {
    let mut hub = Hub::in_memory().unwrap();
    hub.save(&sample()).unwrap();
    let profile = ProfileId::from(1);
    hub.record_visit(profile, "https://news.ycombinator.com/", "Hacker News");
    hub.record_visit(profile, "https://news.ycombinator.com/", "Hacker News");
    hub.record_visit(profile, "https://example.com/", "Example");

    let hits = hub.search_history(profile, "hack", 10);
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].url, "https://news.ycombinator.com/");

    assert!(hub.search_history(profile, "zzz", 10).is_empty());
    assert!(hub.search_history(profile, "  ", 10).is_empty());
    assert!(hub
        .search_history(ProfileId::from(99), "hack", 10)
        .is_empty());
    // FTS5 syntax in user input must not error
    assert!(hub
        .search_history(profile, "\"unbalanced OR (", 10)
        .is_empty());
}

#[test]
fn history_adapter_bounds_inputs_outputs_and_sanitizes_titles() {
    let mut hub = Hub::in_memory().unwrap();
    hub.save(&sample()).unwrap();
    let profile = ProfileId::from(1);
    hub.record_visits((0..150).map(|index| {
        (
            profile,
            format!("https://example.com/{index}"),
            "\u{202e}Match\n".to_owned(),
        )
    }))
    .unwrap();
    hub.record_visit(profile, "file:///etc/passwd", "Match");

    let hits = hub.search_history(profile, "match", u32::MAX);
    assert_eq!(hits.len(), MAX_HISTORY_RESULTS as usize);
    assert!(hits.iter().all(|hit| hit.title == "Match"));
    assert_eq!(hub.history_count(profile), 150);
    assert!(hub
        .search_history(profile, &"x".repeat(MAX_HISTORY_QUERY_BYTES + 1), 10)
        .is_empty());
}

#[test]
fn favicons_roundtrip_with_age() {
    let mut hub = Hub::in_memory().unwrap();
    hub.save(&sample()).unwrap();
    let profile = ProfileId::from(1);
    let origin = "https://example.com";
    let bytes = rgba();

    assert_eq!(hub.favicon_age(profile, origin), None);
    // Caller metadata is not trusted; storage derives the MIME from the
    // fixed-shape raster bytes.
    hub.save_favicon(profile, origin, Some("text/html"), &bytes);
    assert!(hub.favicon_age(profile, origin).unwrap() < 5);
    let (ct, stored) = hub.favicon_bytes(profile, origin).unwrap();
    assert_eq!(ct.as_deref(), Some(zephium_core::icon::RGBA32_MIME));
    assert_eq!(stored, bytes);
    let (raster, age) = hub.favicon_raster_with_age(profile, origin).unwrap();
    assert_eq!(raster, bytes);
    assert!(age < 5);

    hub.save_favicon(profile, "https://example.com/path", None, &rgba());
    hub.save_favicon(profile, "https://invalid.example", None, &[1, 2, 3]);
    assert_eq!(hub.favicon_bytes(profile, "https://example.com/path"), None);
    assert_eq!(hub.favicon_bytes(profile, "https://invalid.example"), None);

    assert_eq!(hub.favicon_bytes(ProfileId::from(99), origin), None);
    assert_eq!(
        hub.favicon_raster_with_age(ProfileId::from(99), origin),
        None
    );
}

#[test]
fn favicon_raster_batch_is_single_request_bounded_and_exact() {
    let mut hub = Hub::in_memory().unwrap();
    hub.save(&sample()).unwrap();
    let profile = ProfileId::from(1);
    let first = "https://one.example";
    let second = "https://two.example";
    let first_rgba = vec![11; zephium_core::icon::RGBA32_BYTES];
    let second_rgba = vec![19; zephium_core::icon::RGBA32_BYTES];
    hub.save_favicon(profile, first, None, &first_rgba);
    hub.save_favicon(profile, second, None, &second_rgba);
    let store = SqliteStore::spawn(hub).unwrap();

    assert_eq!(
        store.favicon_rasters(profile, &[first.to_owned(), second.to_owned()]),
        vec![
            (first.to_owned(), first_rgba),
            (second.to_owned(), second_rgba)
        ]
    );
    assert!(store
        .favicon_rasters(profile, &[first.to_owned(), first.to_owned()])
        .is_empty());
    assert!(store
        .favicon_rasters(
            profile,
            &vec!["https://missing.example".to_owned(); MAX_FAVICON_BATCH_ORIGINS + 1],
        )
        .is_empty());
}

#[test]
fn app_settings_roundtrip() {
    let store = SqliteStore::in_memory().unwrap();
    assert_eq!(store.app_setting("keymap"), None);
    assert!(store.set_app_setting("keymap".into(), r#"{"tab.new":"CmdOrCtrl+N"}"#.into()));
    assert_eq!(
        store.app_setting("keymap").as_deref(),
        Some(r#"{"tab.new":"CmdOrCtrl+N"}"#)
    );

    assert!(!store.set_app_setting(String::new(), "ignored".into()));
    assert!(!store.set_app_setting("oversized".into(), "x".repeat(MAX_SETTING_VALUE_BYTES + 1)));
    assert_eq!(store.app_setting(""), None);
    assert_eq!(store.app_setting("oversized"), None);
    assert_eq!(
        store.app_setting(&"k".repeat(MAX_SETTING_KEY_BYTES + 1)),
        None
    );
}

#[test]
fn app_setting_cardinality_is_bounded_but_existing_keys_remain_updatable() {
    let store = SqliteStore::in_memory().unwrap();
    for index in 0..hub::MAX_APP_SETTINGS {
        let key = format!("setting-{index}");
        assert!(store.set_app_setting(key.clone(), "initial".into()));
        assert_eq!(store.app_setting(&key).as_deref(), Some("initial"));
    }

    assert!(!store.set_app_setting("setting-overflow".into(), "rejected".into()));
    assert_eq!(store.app_setting("setting-overflow"), None);
    assert!(store.flush());

    assert!(store.set_app_setting("setting-0".into(), "updated".into()));
    assert_eq!(store.app_setting("setting-0").as_deref(), Some("updated"));
    assert!(store.flush());
}

#[test]
fn durable_setting_keys_initialize_the_actor_admission_registry() {
    let mut hub = Hub::in_memory().unwrap();
    for index in 0..hub::MAX_APP_SETTINGS {
        assert!(hub
            .set_app_setting(&format!("setting-{index}"), "initial")
            .unwrap());
    }
    let store = SqliteStore::spawn(hub).unwrap();

    assert!(!store.set_app_setting("overflow".into(), "rejected".into()));
    assert!(store.set_app_setting("setting-0".into(), "updated".into()));
    assert!(store.flush());
    assert_eq!(store.app_setting("setting-0").as_deref(), Some("updated"));
}

#[test]
fn impossible_post_admission_setting_rejection_fails_the_barrier_and_requeues() {
    let dir = tempfile::tempdir().unwrap();
    let store = SqliteStore::open(dir.path()).unwrap();
    // Model an external same-user writer diverging after the actor loaded
    // its authoritative bounded key registry.
    let mut external = Connection::open(dir.path().join("meta.sqlite")).unwrap();
    let tx = external.transaction().unwrap();
    {
        let mut insert = tx
            .prepare("INSERT INTO settings(key, value) VALUES (?1, 'external')")
            .unwrap();
        for index in 0..hub::MAX_APP_SETTINGS {
            insert.execute([format!("external-{index}")]).unwrap();
        }
    }
    tx.commit().unwrap();
    drop(external);

    assert!(store.set_app_setting("accepted-before-divergence".into(), "value".into()));
    assert!(
        !store.flush(),
        "quota rejection was acknowledged as durable"
    );
    assert_eq!(
        store
            .pending_settings
            .lock()
            .unwrap()
            .pending
            .get("accepted-before-divergence")
            .map(String::as_str),
        Some("value")
    );
}

#[test]
fn compatibility_reader_rejects_lossy_limits_without_purging_source() {
    let dir = tempfile::tempdir().unwrap();
    let profile = ProfileId::from(1);
    let mut meta = Connection::open(dir.path().join("meta.sqlite")).unwrap();
    migrations::apply(&mut meta, migrations::META).unwrap();
    meta.execute(
        "INSERT INTO profiles(id, name, kind, position) VALUES (?1, 'Personal', 'default', 0)",
        [profile.to_string()],
    )
    .unwrap();
    meta.execute(
        "INSERT INTO state(id, last_profile) VALUES (1, ?1)",
        [profile.to_string()],
    )
    .unwrap();
    drop(meta);

    let profile_path = dir.path().join(format!("profile-{profile}.sqlite"));
    let mut conn = Connection::open(&profile_path).unwrap();
    migrations::apply(&mut conn, migrations::PROFILE).unwrap();
    let tx = conn.transaction().unwrap();
    {
        let mut insert = tx
            .prepare("INSERT INTO spaces(id, name, position) VALUES (?1, ?2, ?3)")
            .unwrap();
        for index in 0..(zephium_core::session::MAX_SESSION_SPACES + 100) {
            insert
                .execute(rusqlite::params![
                    SpaceId::from(1_000 + index as u128).to_string(),
                    format!("Space {index}"),
                    index as i64
                ])
                .unwrap();
        }
        insert
            .execute(rusqlite::params![
                SpaceId::from(999_999).to_string(),
                "x".repeat(hub::MAX_NAME_BYTES + 1),
                -1_i64
            ])
            .unwrap();
    }
    {
        let mut insert = tx
            .prepare(
                "INSERT INTO items(
                     id, parent_id, space_id, section, position, kind,
                     name, url, title, zoom
                 ) VALUES (?1, ?2, NULL, 'favorites', ?3, ?4, ?5, ?6, ?7, 1)",
            )
            .unwrap();
        let mut parent: Option<String> = None;
        for depth in 0..=80_u128 {
            let id = ItemId::from(10_000 + depth).to_string();
            insert
                .execute(rusqlite::params![
                    id,
                    parent,
                    depth as i64,
                    "folder",
                    format!("Folder {depth}"),
                    Option::<String>::None,
                    Option::<String>::None
                ])
                .unwrap();
            parent = Some(ItemId::from(10_000 + depth).to_string());
        }
        for index in 0..(zephium_core::session::MAX_SESSION_ITEMS + 100) {
            insert
                .execute(rusqlite::params![
                    ItemId::from(20_000 + index as u128).to_string(),
                    Option::<String>::None,
                    1000 + index as i64,
                    "tab",
                    Option::<String>::None,
                    format!("https://example.com/{index}"),
                    "Title"
                ])
                .unwrap();
        }
        insert
            .execute(rusqlite::params![
                ItemId::from(999_999).to_string(),
                Option::<String>::None,
                -1_i64,
                "tab",
                Option::<String>::None,
                "x".repeat(hub::MAX_URL_BYTES + 1),
                "Oversized"
            ])
            .unwrap();
    }
    tx.execute(
        "INSERT INTO focus(id, active_space, active_item, splits)
         VALUES (1, NULL, NULL, CAST(zeroblob(?1) AS TEXT))",
        [hub::MAX_SPLIT_JSON_BYTES as i64 + 1],
    )
    .unwrap();
    tx.commit().unwrap();
    drop(conn);

    let mut hub = Hub::open(dir.path().to_path_buf()).unwrap();
    assert!(hub.load().is_err());
    drop(hub);
    let conn = Connection::open(profile_path).unwrap();
    let spaces: i64 = conn
        .query_row("SELECT count(*) FROM spaces", [], |row| row.get(0))
        .unwrap();
    let items: i64 = conn
        .query_row("SELECT count(*) FROM items", [], |row| row.get(0))
        .unwrap();
    let focus: i64 = conn
        .query_row("SELECT count(*) FROM focus", [], |row| row.get(0))
        .unwrap();
    assert!(spaces > zephium_core::session::MAX_SESSION_SPACES as i64);
    assert!(items > zephium_core::session::MAX_SESSION_ITEMS as i64);
    assert_eq!(focus, 1);
}

#[test]
fn compatibility_semantic_corruption_is_not_canonicalized_over_source() {
    let dir = tempfile::tempdir().unwrap();
    let profile = ProfileId::from(1);
    let mut meta = Connection::open(dir.path().join("meta.sqlite")).unwrap();
    migrations::apply(&mut meta, migrations::META).unwrap();
    meta.execute(
        "INSERT INTO profiles(id, name, kind, position) VALUES (?1, 'Personal', 'default', 0)",
        [profile.to_string()],
    )
    .unwrap();
    meta.execute(
        "INSERT INTO state(id, last_profile) VALUES (1, ?1)",
        [profile.to_string()],
    )
    .unwrap();
    drop(meta);

    let profile_path = dir.path().join(format!("profile-{profile}.sqlite"));
    let mut conn = Connection::open(&profile_path).unwrap();
    migrations::apply(&mut conn, migrations::PROFILE).unwrap();
    conn.execute(
        "INSERT INTO items(
             id, parent_id, space_id, section, position, kind, name, url, title, zoom
         ) VALUES (?1, NULL, NULL, 'favorites', 0, 'tab', NULL, 'file:///etc/passwd', 'Local', 1)",
        [ItemId::from(9).to_string()],
    )
    .unwrap();
    drop(conn);

    assert!(SqliteStore::open(dir.path()).is_err());
    let conn = Connection::open(profile_path).unwrap();
    let rows: i64 = conn
        .query_row("SELECT count(*) FROM items", [], |row| row.get(0))
        .unwrap();
    assert_eq!(rows, 1, "failed recovery preflight modified source rows");
}

#[test]
fn valid_multi_profile_compatibility_state_loads_in_canonical_container_order() {
    let dir = tempfile::tempdir().unwrap();
    let first = ProfileId::from(1);
    let second = ProfileId::from(2);
    let first_space = SpaceId::from(11);
    let second_space = SpaceId::from(12);
    let first_favorite = ItemId::from(21);
    let second_favorite = ItemId::from(22);
    let first_today = ItemId::from(31);
    let second_today = ItemId::from(32);

    let mut meta = Connection::open(dir.path().join("meta.sqlite")).unwrap();
    migrations::apply(&mut meta, migrations::META).unwrap();
    for (position, profile, name, kind) in [
        (0_i64, first, "First", "default"),
        (1_i64, second, "Second", "named"),
    ] {
        meta.execute(
            "INSERT INTO profiles(id, name, kind, position) VALUES (?1, ?2, ?3, ?4)",
            params![profile.to_string(), name, kind, position],
        )
        .unwrap();
    }
    meta.execute(
        "INSERT INTO state(id, last_profile) VALUES (1, ?1)",
        [second.to_string()],
    )
    .unwrap();
    drop(meta);

    for (profile, space, favorite, today, focused) in [
        (first, first_space, first_favorite, first_today, false),
        (second, second_space, second_favorite, second_today, true),
    ] {
        let path = dir.path().join(format!("profile-{profile}.sqlite"));
        let mut conn = Connection::open(path).unwrap();
        migrations::apply(&mut conn, migrations::PROFILE).unwrap();
        conn.execute(
            "INSERT INTO spaces(id, name, position) VALUES (?1, 'Space', 0)",
            [space.to_string()],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO items(
                 id, parent_id, space_id, section, position, kind, name, url, title, zoom
             ) VALUES (?1, NULL, NULL, 'favorites', 0, 'tab', NULL, ?2, 'Favorite', 1)",
            params![
                favorite.to_string(),
                format!("https://favorite-{profile}.example/")
            ],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO items(
                 id, parent_id, space_id, section, position, kind, name, url, title, zoom
             ) VALUES (?1, NULL, ?2, 'today', 0, 'tab', NULL, ?3, 'Today', 1)",
            params![
                today.to_string(),
                space.to_string(),
                format!("https://today-{profile}.example/")
            ],
        )
        .unwrap();
        let (active_space, active_item, splits) = if focused {
            (
                Some(space.to_string()),
                Some(today.to_string()),
                Some(format!(r#"{{"leaf":"{today}"}}"#)),
            )
        } else {
            (None, None, None)
        };
        conn.execute(
            "INSERT INTO focus(id, active_space, active_item, splits) VALUES (1, ?1, ?2, ?3)",
            params![active_space, active_item, splits],
        )
        .unwrap();
    }

    let mut hub = Hub::open(dir.path().to_path_buf()).unwrap();
    let state = hub.load().unwrap().unwrap();
    assert_eq!(
        state.items.iter().map(|item| item.id).collect::<Vec<_>>(),
        vec![first_favorite, second_favorite, first_today, second_today]
    );
    assert_eq!(state.active_space, Some(second_space));
    assert_eq!(state.active_item, Some(second_today));
    assert_eq!(state.splits, Some(Pane::Leaf(second_today)));
}

#[test]
fn malformed_registry_rows_cannot_crowd_out_and_delete_a_valid_profile() {
    let dir = tempfile::tempdir().unwrap();
    drop(Hub::open(dir.path().to_path_buf()).unwrap());
    let valid = ProfileId::from(500);
    let profile_path = create_profile_file(dir.path(), valid);
    let meta = Connection::open(dir.path().join("meta.sqlite")).unwrap();
    for position in 0..MAX_SESSION_PROFILES {
        meta.execute(
            "INSERT INTO profiles(id, name, kind, position)
             VALUES (?1, 'Malformed', 'default', ?2)",
            params![format!("malformed-{position:02}"), position as i64],
        )
        .unwrap();
    }
    meta.execute(
        "INSERT INTO profiles(id, name, kind, position)
         VALUES (?1, 'Valid', 'default', ?2)",
        params![valid.to_string(), MAX_SESSION_PROFILES as i64],
    )
    .unwrap();
    drop(meta);

    assert!(Hub::open(dir.path().to_path_buf()).is_err());
    assert!(
        profile_path.exists(),
        "failed open deleted recoverable data"
    );
}

#[test]
fn registry_rejects_invalid_and_duplicate_id_aliases_without_cleanup() {
    for alias in [None, Some(ProfileId::from(42).to_string().to_lowercase())] {
        let dir = tempfile::tempdir().unwrap();
        drop(Hub::open(dir.path().to_path_buf()).unwrap());
        let profile = ProfileId::from(42);
        let profile_path = create_profile_file(dir.path(), profile);
        let meta = Connection::open(dir.path().join("meta.sqlite")).unwrap();
        meta.execute(
            "INSERT INTO profiles(id, name, kind, position)
             VALUES (?1, 'Profile', 'default', 0)",
            [alias.as_deref().unwrap_or("not-a-profile-id")],
        )
        .unwrap();
        if alias.is_some() {
            meta.execute(
                "INSERT INTO profiles(id, name, kind, position)
                 VALUES (?1, 'Duplicate', 'named', 1)",
                [profile.to_string()],
            )
            .unwrap();
        }
        drop(meta);

        assert!(Hub::open(dir.path().to_path_buf()).is_err());
        assert!(profile_path.exists());
    }
}

#[test]
fn compatibility_profile_filtering_fails_instead_of_persisting_a_subset() {
    let dir = tempfile::tempdir().unwrap();
    drop(Hub::open(dir.path().to_path_buf()).unwrap());
    let profile = ProfileId::from(1);
    let profile_path = create_profile_file(dir.path(), profile);
    let meta = Connection::open(dir.path().join("meta.sqlite")).unwrap();
    meta.execute(
        "INSERT INTO profiles(id, name, kind, position)
         VALUES (?1, ?2, 'default', 0)",
        params![profile.to_string(), "x".repeat(hub::MAX_NAME_BYTES + 1)],
    )
    .unwrap();
    drop(meta);

    assert!(SqliteStore::open(dir.path()).is_err());
    assert!(
        profile_path.exists(),
        "failed preflight deleted profile data"
    );
}

#[test]
fn corrupt_unsupported_and_oversized_snapshots_enter_recovery_without_file_cleanup() {
    for corruption in ["corrupt", "unsupported", "oversized"] {
        let dir = tempfile::tempdir().unwrap();
        let profile = ProfileId::from(1);
        let stale = ProfileId::from(900);
        let registered_path;
        {
            let mut hub = Hub::open(dir.path().to_path_buf()).unwrap();
            hub.save(&sample()).unwrap();
            hub.record_visit(profile, "https://example.com/", "Example");
            registered_path = dir.path().join(format!("profile-{profile}.sqlite"));
        }
        let stale_path = create_profile_file(dir.path(), stale);
        let meta = Connection::open(dir.path().join("meta.sqlite")).unwrap();
        match corruption {
            "corrupt" => {
                meta.execute("UPDATE session_snapshot SET data = '{' WHERE id = 1", [])
                    .unwrap();
            }
            "unsupported" => {
                meta.execute(
                    "UPDATE session_snapshot SET schema_version = 999 WHERE id = 1",
                    [],
                )
                .unwrap();
            }
            "oversized" => {
                meta.execute(
                    "UPDATE session_snapshot
                     SET data = CAST(zeroblob(?1) AS TEXT) WHERE id = 1",
                    [hub::MAX_SESSION_SNAPSHOT_BYTES as i64 + 1],
                )
                .unwrap();
            }
            _ => unreachable!(),
        }
        drop(meta);

        let store = SqliteStore::open(dir.path()).unwrap();
        assert!(matches!(
            store.load_session(),
            SessionLoad::RecoveryRequired { .. }
        ));
        assert!(registered_path.exists(), "registered profile was deleted");
        assert!(
            stale_path.exists(),
            "stale profile was deleted on failed open"
        );
    }
}

#[test]
fn semantic_session_corruption_is_quarantined_exactly_and_store_becomes_read_only() {
    let dir = tempfile::tempdir().unwrap();
    {
        let mut hub = Hub::open(dir.path().to_path_buf()).unwrap();
        hub.save(&sample()).unwrap();
    }
    let mut invalid = sample();
    invalid.active_item = Some(ItemId::from(999_999));
    let original = serde_json::to_string(&invalid).unwrap();
    let meta = Connection::open(dir.path().join("meta.sqlite")).unwrap();
    meta.execute(
        "UPDATE session_snapshot SET data = ?1 WHERE id = 1",
        [&original],
    )
    .unwrap();
    drop(meta);

    let store = Arc::new(SqliteStore::open(dir.path()).unwrap());
    let SessionLoad::RecoveryRequired { reason } = store.load_session() else {
        panic!("semantic corruption did not enter explicit recovery mode")
    };
    assert!(reason.contains("canonical"), "{reason}");
    assert_eq!(
        load_page_permissions(store.as_ref(), ProfileId::from(1)),
        PagePermissionCatalogLoadOutcome::Failed
    );
    assert_eq!(
        mutate_page_permissions(
            store.as_ref(),
            ProfileId::from(1),
            PagePermissionCatalogRevision::INITIAL,
            page_patch(vec![PagePermissionChange::Create {
                id: PagePermissionGrantId::from(811),
                origin: page_origin("https://recovery-must-not-write.example"),
                kind: PagePermissionKind::Camera,
                decision: RememberedPagePermission::Allow,
            }]),
        ),
        PagePermissionCatalogMutationOutcome::Failed
    );
    assert!(store.set_app_setting("must-not-write".into(), "value".into()));
    store.save_session(sample());
    assert!(!store.flush());

    let meta = Connection::open(dir.path().join("meta.sqlite")).unwrap();
    let quarantined: Vec<u8> = meta
        .query_row(
            "SELECT data FROM session_recovery WHERE id = 1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let authoritative: String = meta
        .query_row(
            "SELECT data FROM session_snapshot WHERE id = 1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let setting_count: i64 = meta
        .query_row(
            "SELECT count(*) FROM settings WHERE key = 'must-not-write'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(quarantined, original.as_bytes());
    assert_eq!(authoritative, original);
    assert_eq!(setting_count, 0);
}

#[test]
fn registered_future_profile_schema_is_preserved_and_explicitly_degraded() {
    let dir = tempfile::tempdir().unwrap();
    let state = two_profile_sample();
    let healthy = ProfileId::from(1);
    let degraded = ProfileId::from(3);
    {
        let mut hub = Hub::open(dir.path().to_path_buf()).unwrap();
        hub.save(&state).unwrap();
        hub.record_visit(healthy, "https://healthy.example/", "Healthy");
        hub.record_visit(degraded, "https://preserved.example/", "Preserved");
    }
    let degraded_path = dir.path().join(format!("profile-{degraded}.sqlite"));
    let degraded_db = Connection::open(&degraded_path).unwrap();
    degraded_db
        .pragma_update(None, "user_version", 10_000)
        .unwrap();
    degraded_db
        .execute_batch("PRAGMA wal_checkpoint(TRUNCATE); PRAGMA journal_mode=DELETE;")
        .unwrap();
    drop(degraded_db);
    let preserved = artifact_bytes(&degraded_path);
    let orphan = ProfileId::from(2);
    let orphan_path = dir.path().join(format!("profile-{orphan}.sqlite"));
    std::fs::write(&orphan_path, b"not a database").unwrap();

    let store = Arc::new(SqliteStore::open(dir.path()).unwrap());
    assert_eq!(
        store.load_session(),
        SessionLoad::LoadedWithDegradedProfiles {
            state: state.clone(),
            profiles: vec![degraded],
            blocker_configs: default_blocker_configs(&state),
        }
    );
    assert_eq!(
        load_page_permissions(store.as_ref(), degraded),
        PagePermissionCatalogLoadOutcome::DegradedProfile
    );
    assert_eq!(
        mutate_page_permissions(
            store.as_ref(),
            degraded,
            PagePermissionCatalogRevision::INITIAL,
            page_patch(vec![PagePermissionChange::Create {
                id: PagePermissionGrantId::from(812),
                origin: page_origin("https://degraded-must-not-write.example"),
                kind: PagePermissionKind::Microphone,
                decision: RememberedPagePermission::Deny,
            }]),
        ),
        PagePermissionCatalogMutationOutcome::DegradedProfile
    );
    // Degraded operations are terminal no-ops: they neither reopen the
    // source nor poison the ordered flush barrier with infinite retries.
    store.record_visit(
        degraded,
        "https://must-not-land.example/".into(),
        "Ignored".into(),
    );
    store.save_favicon(
        degraded,
        "https://must-not-land.example".into(),
        None,
        rgba(),
    );
    assert!(store.flush());
    assert!(store.search_history(degraded, "preserved", 10).is_empty());
    assert_eq!(
        store.favicon_age(degraded, "https://must-not-land.example"),
        None
    );
    assert_eq!(artifact_bytes(&degraded_path), preserved);

    // Another profile in the same exact authoritative session remains
    // fully usable.
    store.record_visit(
        healthy,
        "https://still-usable.example/".into(),
        "Still Usable".into(),
    );
    assert!(store.flush());
    assert_eq!(store.search_history(healthy, "usable", 10).len(), 1);
    assert_eq!(std::fs::read(orphan_path).unwrap(), b"not a database");

    // Exact journal authorization may delete the preserved degraded file;
    // nothing else may rewrite or unlink it.
    let mut filtered = state;
    filtered.profiles.retain(|profile| profile.id != degraded);
    filtered.spaces.retain(|space| space.profile != degraded);
    assert_eq!(
        store.authorize_profile_deletion(
            degraded,
            filtered,
            Instant::now() + Duration::from_secs(1),
        ),
        ProfileDeletionAuthorizeOutcome::Authorized
    );
    assert_eq!(
        store.finalize_profile_deletion(degraded, Instant::now() + Duration::from_secs(1),),
        ProfileDeletionFinalizeOutcome::Completed
    );
    assert!(!degraded_path.exists());
}

#[test]
fn registered_schema_corruption_is_preserved_without_blocking_the_session() {
    let dir = tempfile::tempdir().unwrap();
    let state = two_profile_sample();
    let degraded = ProfileId::from(3);
    {
        let mut hub = Hub::open(dir.path().to_path_buf()).unwrap();
        hub.save(&state).unwrap();
        hub.record_visit(degraded, "https://preserved.example/", "Preserved");
    }
    let path = dir.path().join(format!("profile-{degraded}.sqlite"));
    let degraded_db = Connection::open(&path).unwrap();
    degraded_db
        .execute_batch(
            "CREATE TRIGGER unexpected_history_trigger
             AFTER INSERT ON history BEGIN SELECT 1; END;",
        )
        .unwrap();
    degraded_db
        .execute_batch("PRAGMA wal_checkpoint(TRUNCATE); PRAGMA journal_mode=DELETE;")
        .unwrap();
    drop(degraded_db);
    let preserved = artifact_bytes(&path);

    let store = SqliteStore::open(dir.path()).unwrap();
    let blocker_configs = default_blocker_configs(&state);
    assert_eq!(
        store.load_session(),
        SessionLoad::LoadedWithDegradedProfiles {
            state,
            profiles: vec![degraded],
            blocker_configs,
        }
    );
    assert_eq!(artifact_bytes(&path), preserved);
}

#[test]
fn unknown_meta_schema_is_rejected_before_a_writable_sqlite_open() {
    let dir = tempfile::tempdir().unwrap();
    drop(Hub::open(dir.path().to_path_buf()).unwrap());
    let path = dir.path().join("meta.sqlite");
    let meta = Connection::open(&path).unwrap();
    meta.execute_batch(
        "CREATE VIEW unexpected_meta_view AS SELECT id FROM profiles;
         PRAGMA wal_checkpoint(TRUNCATE);
         PRAGMA journal_mode=DELETE;",
    )
    .unwrap();
    drop(meta);
    let preserved = artifact_bytes(&path);

    let error = match Hub::open(dir.path().to_path_buf()) {
        Ok(_) => panic!("unknown authoritative schema was accepted"),
        Err(error) => error,
    };
    assert!(error.to_string().contains("sqlite_schema"), "{error}");
    assert_eq!(
        artifact_bytes(&path),
        preserved,
        "failed validation modified authoritative storage"
    );
}

#[test]
fn registered_hard_link_violation_still_fails_startup_globally() {
    let dir = tempfile::tempdir().unwrap();
    let state = two_profile_sample();
    let first = ProfileId::from(1);
    let second = ProfileId::from(3);
    {
        let mut hub = Hub::open(dir.path().to_path_buf()).unwrap();
        hub.save(&state).unwrap();
        hub.record_visit(first, "https://first.example/", "First");
        hub.record_visit(second, "https://second.example/", "Second");
    }
    let first_path = dir.path().join(format!("profile-{first}.sqlite"));
    let second_path = dir.path().join(format!("profile-{second}.sqlite"));
    std::fs::remove_file(&second_path).unwrap();
    std::fs::hard_link(&first_path, &second_path).unwrap();

    assert!(SqliteStore::open(dir.path()).is_err());
}

#[test]
fn authoritative_snapshot_must_exactly_match_validated_registry_before_purge() {
    let dir = tempfile::tempdir().unwrap();
    let snapshot_profile = ProfileId::from(1);
    let registry_profile = ProfileId::from(2);
    let profile_path;
    {
        let mut hub = Hub::open(dir.path().to_path_buf()).unwrap();
        hub.save(&sample()).unwrap();
        hub.record_visit(snapshot_profile, "https://example.com/", "Example");
        profile_path = dir
            .path()
            .join(format!("profile-{snapshot_profile}.sqlite"));
    }
    let meta = Connection::open(dir.path().join("meta.sqlite")).unwrap();
    // Model out-of-band registry corruption, not a legal application update.
    // The site-preference FK correctly prevents this mutation in normal use.
    meta.pragma_update(None, "foreign_keys", false).unwrap();
    meta.execute(
        "UPDATE profiles SET id = ?1 WHERE id = ?2",
        params![registry_profile.to_string(), snapshot_profile.to_string()],
    )
    .unwrap();
    drop(meta);

    let store = SqliteStore::open(dir.path()).unwrap();
    assert!(matches!(
        store.load_session(),
        SessionLoad::RecoveryRequired { .. }
    ));
    assert!(
        profile_path.exists(),
        "registry mismatch authorized destructive reconciliation"
    );
}

#[test]
fn blocker_site_preferences_survive_session_saves_restart_and_stale_writes() {
    use zephium_core::blocker::{
        BlockerSite, BlockerSitePreferences, PersonalHide, SitePreferenceChange,
    };
    fn load(store: &SqliteStore, profile: ProfileId) -> Arc<BlockerSitePreferences> {
        let (send, receive) = mpsc::channel();
        assert!(store.load_profile_blocker_sites(
            profile,
            Box::new(move |outcome| send.send(outcome).unwrap())
        ));
        let BlockerSiteLoadOutcome::Loaded(value) =
            receive.recv_timeout(Duration::from_secs(5)).unwrap()
        else {
            panic!("site preferences must load");
        };
        value
    }
    fn update(
        store: &SqliteStore,
        profile: ProfileId,
        expected: u64,
        next: Arc<BlockerSitePreferences>,
    ) -> BlockerSiteUpdateOutcome {
        let (send, receive) = mpsc::channel();
        assert!(store.update_profile_blocker_sites(
            profile,
            expected,
            next,
            Box::new(move |outcome| send.send(outcome).unwrap())
        ));
        receive.recv_timeout(Duration::from_secs(5)).unwrap()
    }
    let dir = tempfile::tempdir().unwrap();
    let profile = ProfileId::from(1);
    let site = BlockerSite::from_url("https://example.com/").unwrap();
    let expected;
    {
        let store = SqliteStore::open(dir.path()).unwrap();
        store.save_session(sample());
        // The narrow load must make an already-admitted new-profile session
        // visible without requiring an unrelated UI operation to flush it.
        let initial = load(&store, profile);
        let hidden = Arc::new(
            initial
                .changed(SitePreferenceChange::AddHide(PersonalHide {
                    id: 0,
                    site: site.clone(),
                    selector: ".banner".into(),
                    label: "Banner".into(),
                    enabled: true,
                }))
                .unwrap(),
        );
        assert!(matches!(
            update(&store, profile, initial.revision(), hidden.clone()),
            BlockerSiteUpdateOutcome::Updated(_)
        ));
        expected = Arc::new(
            hidden
                .changed(SitePreferenceChange::Pause {
                    site: site.clone(),
                    paused: true,
                })
                .unwrap(),
        );
        assert!(matches!(
            update(&store, profile, hidden.revision(), expected.clone()),
            BlockerSiteUpdateOutcome::Updated(_)
        ));
        assert_eq!(
            update(&store, profile, initial.revision(), hidden),
            BlockerSiteUpdateOutcome::Conflict(expected.clone())
        );
        store.save_session(sample());
        assert!(store.flush());
        assert_eq!(load(&store, profile), expected);

        let private = Arc::new(
            BlockerSitePreferences::default()
                .changed(SitePreferenceChange::Pause {
                    site: BlockerSite::from_url("https://private-only.invalid/").unwrap(),
                    paused: true,
                })
                .unwrap(),
        );
        assert_eq!(
            update(&store, ProfileId::from(999), 1, private),
            BlockerSiteUpdateOutcome::NotRegistered
        );
        assert_eq!(
            store.shutdown_until(Instant::now() + Duration::from_secs(5)),
            StoreShutdownOutcome::Clean
        );
    }
    let store = SqliteStore::open(dir.path()).unwrap();
    assert_eq!(load(&store, profile), expected);
    assert!(load(&store, profile).paused(&site));
    assert_eq!(
        store.shutdown_until(Instant::now() + Duration::from_secs(5)),
        StoreShutdownOutcome::Clean
    );
    assert!(!std::fs::read(dir.path().join("meta.sqlite"))
        .unwrap()
        .windows(b"private-only.invalid".len())
        .any(|w| w == b"private-only.invalid"));
}

#[test]
fn authoritative_blocker_cohort_corruption_enters_read_only_recovery() {
    for corruption in ["missing", "extra", "malformed"] {
        let dir = tempfile::tempdir().unwrap();
        let original = sample();
        {
            let mut hub = Hub::open(dir.path().to_path_buf()).unwrap();
            hub.save(&original).unwrap();
        }
        let meta = Connection::open(dir.path().join("meta.sqlite")).unwrap();
        let original_snapshot: String = meta
            .query_row(
                "SELECT data FROM session_snapshot WHERE id = 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        match corruption {
            "missing" => {
                meta.execute(
                    "DELETE FROM profile_blocker_settings WHERE profile_id = ?1",
                    [ProfileId::from(1).to_string()],
                )
                .unwrap();
            }
            "extra" => {
                meta.execute(
                    "INSERT INTO profile_blocker_settings(profile_id, revision, enabled)
                     VALUES (?1, 1, 0)",
                    [ProfileId::from(999).to_string()],
                )
                .unwrap();
            }
            "malformed" => {
                meta.execute(
                    "UPDATE profile_blocker_settings
                     SET profile_id = 'ZZZZZZZZZZZZZZZZZZZZZZZZZZ'",
                    [],
                )
                .unwrap();
            }
            _ => unreachable!(),
        }
        drop(meta);

        let store = SqliteStore::open(dir.path()).unwrap();
        let SessionLoad::RecoveryRequired { reason } = store.load_session() else {
            panic!("{corruption} blocker cohort was accepted")
        };
        assert!(reason.contains("blocker"), "{reason}");
        drop(store);

        let meta = Connection::open(dir.path().join("meta.sqlite")).unwrap();
        let retained_snapshot: String = meta
            .query_row(
                "SELECT data FROM session_snapshot WHERE id = 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            retained_snapshot, original_snapshot,
            "{corruption} blocker corruption rewrote the authoritative session"
        );
    }
}

#[test]
fn session_commit_never_silently_repairs_a_diverged_blocker_cohort() {
    let dir = tempfile::tempdir().unwrap();
    let original = sample();
    let mut hub = Hub::open(dir.path().to_path_buf()).unwrap();
    hub.save(&original).unwrap();
    let external = Connection::open(dir.path().join("meta.sqlite")).unwrap();
    external
        .execute(
            "DELETE FROM profile_blocker_settings WHERE profile_id = ?1",
            [ProfileId::from(1).to_string()],
        )
        .unwrap();
    drop(external);

    let mut updated = original.clone();
    updated.active_item = Some(ItemId::from(10));
    assert!(hub.save(&updated).is_err());
    assert_eq!(hub.load().unwrap(), Some(original));
}

#[test]
fn maximum_valid_session_fits_snapshot_budget() {
    let profile = ProfileId::from(1);
    let space = SpaceId::from(2);
    let prefix = "https://example.com/";
    let url = format!("{prefix}{}", "a".repeat(hub::MAX_URL_BYTES - prefix.len()));
    let items = (0..zephium_core::session::MAX_SESSION_ITEMS)
        .map(|index| PersistedItem {
            id: ItemId::from(100 + index as u128),
            parent: None,
            placement: Placement::Space {
                space,
                section: SpaceSection::Today,
            },
            kind: PersistedKind::Tab {
                url: url.clone(),
                title: "\\".repeat(zephium_core::item::MAX_PAGE_TITLE_CHARS),
                zoom: 1.0,
            },
        })
        .collect();
    let state = zephium_core::session::canonicalize(SessionState {
        profiles: vec![PersistedProfile {
            id: profile,
            name: "Personal".into(),
            kind: ProfileKind::Default,
        }],
        spaces: vec![PersistedSpace {
            id: space,
            profile,
            name: "Space".into(),
        }],
        items,
        active_space: Some(space),
        active_item: Some(ItemId::from(100)),
        splits: None,
        recently_closed: Vec::new(),
    });
    let encoded = serde_json::to_vec(&state).unwrap();
    assert!(
        encoded.len() <= hub::MAX_SESSION_SNAPSHOT_BYTES,
        "valid maximum session serialized to {} bytes",
        encoded.len()
    );
}

#[test]
fn generic_session_save_cannot_implicitly_authorize_profile_erasure() {
    let mut hub = Hub::in_memory().unwrap();
    let original = sample();
    hub.save(&original).unwrap();

    let error = hub.save(&SessionState::default()).unwrap_err().to_string();
    assert!(error.contains("explicit deletion authorization"), "{error}");
    assert_eq!(hub.load().unwrap(), Some(original));
    assert!(hub.pending_profile_deletions().unwrap().is_empty());
}

#[test]
fn deletion_authorization_atomically_publishes_filtered_session_and_journal() {
    let mut hub = Hub::in_memory().unwrap();
    let profile = ProfileId::from(1);
    hub.save(&sample()).unwrap();

    assert_eq!(
        hub.authorize_profile_deletion(profile, &SessionState::default())
            .unwrap(),
        ProfileDeletionAuthorizeOutcome::Authorized
    );
    assert_eq!(hub.load().unwrap(), Some(SessionState::default()));
    assert!(
        hub.profile_blocker_configs().unwrap().is_empty(),
        "deletion authorization retained a profile preference"
    );
    assert_eq!(
        hub.pending_profile_deletions().unwrap(),
        vec![zephium_core::ports::store::PendingProfileDeletion {
            profile,
            native_erasure_verified: false,
        }]
    );

    // A crash-resume retry observes the exact durable authorization and
    // never creates a second journal row.
    assert_eq!(
        hub.authorize_profile_deletion(profile, &SessionState::default())
            .unwrap(),
        ProfileDeletionAuthorizeOutcome::AlreadyAuthorized
    );
    assert_eq!(hub.pending_profile_deletions().unwrap().len(), 1);
}

#[test]
fn deletion_authorization_rejects_non_exact_registry_transitions() {
    let mut hub = Hub::in_memory().unwrap();
    let original = two_profile_sample();
    hub.save(&original).unwrap();

    assert_eq!(
        hub.authorize_profile_deletion(ProfileId::from(1), &SessionState::default())
            .unwrap(),
        ProfileDeletionAuthorizeOutcome::SessionConflict
    );
    assert_eq!(hub.load().unwrap(), Some(original));
    assert!(hub.pending_profile_deletions().unwrap().is_empty());
}

#[test]
fn deletion_authorization_deadline_reports_definite_non_admission() {
    let store = SqliteStore::in_memory().unwrap();
    store.save_session(sample());
    assert!(store.flush());

    assert_eq!(
        store.authorize_profile_deletion(
            ProfileId::from(1),
            SessionState::default(),
            Instant::now(),
        ),
        ProfileDeletionAuthorizeOutcome::NotAdmitted
    );
    assert_eq!(loaded(&store), sample());
    assert_eq!(
        store.pending_profile_deletions(),
        ProfileDeletionLoad::Loaded(Vec::new())
    );
}

#[test]
fn ambiguous_committed_deletion_reloads_durable_truth_before_pending_snapshot_retry() {
    let profile = ProfileId::from(1);
    let mut hub = Hub::in_memory().unwrap();
    hub.save(&sample()).unwrap();
    hub.fail_next_profile_deletion_commit_as_ambiguous();
    let store = SqliteStore::spawn(hub).unwrap();
    // Leave the pre-deletion snapshot in the actor's debounce slot. If the
    // committed journal is interpreted through stale in-memory registry
    // state, this snapshot retries forever and journal reconciliation fails.
    store.save_session(sample());

    assert_eq!(
        store.authorize_profile_deletion(
            profile,
            SessionState::default(),
            Instant::now() + Duration::from_secs(1),
        ),
        ProfileDeletionAuthorizeOutcome::Authorized
    );
    assert_eq!(
        store.pending_profile_deletions(),
        ProfileDeletionLoad::Loaded(vec![zephium_core::ports::store::PendingProfileDeletion {
            profile,
            native_erasure_verified: false,
        },])
    );
    assert!(
        store.flush(),
        "superseded pre-barrier snapshot was retained"
    );
    assert_eq!(loaded(&store), SessionState::default());
}

#[test]
fn removed_profile_database_waits_for_native_proof_then_purges_sidecars() {
    let dir = tempfile::tempdir().unwrap();
    let profile = ProfileId::from(1);
    let path = dir.path().join(format!("profile-{profile}.sqlite"));
    let mut hub = Hub::open(dir.path().to_path_buf()).unwrap();
    hub.save(&sample()).unwrap();
    hub.record_visit(profile, "https://example.com/", "Example");
    hub.save_favicon(profile, "https://example.com", None, &rgba());
    assert!(path.exists());
    let notes = dir.path().join("notes").join(profile.to_string());
    std::fs::create_dir_all(notes.join("Notes")).unwrap();
    std::fs::write(notes.join("Notes/Plans.md"), "# Plans").unwrap();
    std::fs::write(notes.join("index.sqlite"), "index").unwrap();
    let outside = dir.path().join("outside.md");
    std::fs::write(&outside, "kept").unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(&outside, notes.join("Notes/Link.md")).unwrap();

    assert_eq!(
        hub.authorize_profile_deletion(profile, &SessionState::default())
            .unwrap(),
        ProfileDeletionAuthorizeOutcome::Authorized
    );
    assert!(path.exists());
    assert!(notes.exists());
    assert_eq!(
        hub.pending_profile_deletions().unwrap(),
        vec![zephium_core::ports::store::PendingProfileDeletion {
            profile,
            native_erasure_verified: false,
        }]
    );
    assert!(hub.finalize_profile_deletion(profile).unwrap());
    assert!(!path.exists());
    assert!(!std::path::PathBuf::from(format!("{}-wal", path.display())).exists());
    assert!(!std::path::PathBuf::from(format!("{}-shm", path.display())).exists());
    assert!(!notes.exists());
    assert_eq!(
        std::fs::read_dir(dir.path().join("notes")).unwrap().count(),
        0
    );
    assert_eq!(std::fs::read_to_string(&outside).unwrap(), "kept");
    assert!(hub.pending_profile_deletions().unwrap().is_empty());
}

#[test]
fn windows_style_local_deletion_keeps_authorization_until_restart_confirms_absence() {
    let dir = tempfile::tempdir().unwrap();
    let profile = ProfileId::from(1);
    let path = dir.path().join(format!("profile-{profile}.sqlite"));
    {
        let mut hub = Hub::open(dir.path().to_path_buf()).unwrap();
        hub.save(&sample()).unwrap();
        hub.record_visit(profile, "https://private.example/", "Private");
        assert_eq!(
            hub.authorize_profile_deletion(profile, &SessionState::default())
                .unwrap(),
            ProfileDeletionAuthorizeOutcome::Authorized
        );

        assert!(hub
            .finalize_profile_deletion_requiring_restart_confirmation(profile)
            .unwrap());
        assert!(!path.exists());
        // Completion is visible to the current shell, but the internal
        // authorization deliberately remains durable on disk.
        assert!(hub.pending_profile_deletions().unwrap().is_empty());
        assert_eq!(
            hub.completed_profile_deletion_tombstones().unwrap(),
            vec![profile]
        );
    }

    // Reopening storage inside the same process is not a restart and must
    // not retire the completed tombstone.
    let hub = Hub::open(dir.path().to_path_buf()).unwrap();
    assert!(hub.pending_profile_deletions().unwrap().is_empty());
    assert_eq!(
        hub.completed_profile_deletion_tombstones().unwrap(),
        vec![profile]
    );
    drop(hub);

    // A new process generation occurs after filesystem recovery. Only
    // this observation is allowed to retire the Windows tombstone.
    let hub = Hub::open_for_new_process(dir.path().to_path_buf()).unwrap();
    assert!(hub.pending_profile_deletions().unwrap().is_empty());
    assert!(hub
        .completed_profile_deletion_tombstones()
        .unwrap()
        .is_empty());
}

#[test]
fn restart_never_reaps_a_completed_tombstone_without_authoritative_session() {
    let dir = tempfile::tempdir().unwrap();
    let profile = ProfileId::from(1);
    {
        let mut hub = Hub::open(dir.path().to_path_buf()).unwrap();
        hub.save(&sample()).unwrap();
        hub.record_visit(profile, "https://private.example/", "Private");
        assert_eq!(
            hub.authorize_profile_deletion(profile, &SessionState::default())
                .unwrap(),
            ProfileDeletionAuthorizeOutcome::Authorized
        );
        assert!(hub
            .finalize_profile_deletion_requiring_restart_confirmation(profile)
            .unwrap());
    }

    // Model meta corruption that removes the authoritative survivor
    // snapshot. Restart must fail closed before absence verification can
    // retire the only remaining deletion authorization.
    let meta_path = dir.path().join("meta.sqlite");
    let meta = rusqlite::Connection::open(&meta_path).unwrap();
    assert_eq!(meta.execute("DELETE FROM session_snapshot", []).unwrap(), 1);
    drop(meta);
    assert!(Hub::open_for_new_process(dir.path().to_path_buf()).is_err());

    let meta = rusqlite::Connection::open(meta_path).unwrap();
    let retained: i64 = meta
        .query_row("SELECT count(*) FROM profile_deletion_journal", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(retained, 1);
}

#[test]
fn restart_reopens_local_cleanup_if_a_completed_artifact_is_observed() {
    let dir = tempfile::tempdir().unwrap();
    let profile = ProfileId::from(1);
    let path = dir.path().join(format!("profile-{profile}.sqlite"));
    let wal_path = std::path::PathBuf::from(format!("{}-wal", path.display()));
    {
        let mut hub = Hub::open(dir.path().to_path_buf()).unwrap();
        hub.save(&sample()).unwrap();
        hub.record_visit(profile, "https://private.example/", "Private");
        assert_eq!(
            hub.authorize_profile_deletion(profile, &SessionState::default())
                .unwrap(),
            ProfileDeletionAuthorizeOutcome::Authorized
        );
        assert!(hub
            .finalize_profile_deletion_requiring_restart_confirmation(profile)
            .unwrap());
    }

    // Model an unlink that was acknowledged before a power cut but whose
    // namespace update did not survive recovery.
    std::fs::write(&wal_path, b"resurrected private WAL bytes").unwrap();
    {
        let mut hub = Hub::open_for_new_process(dir.path().to_path_buf()).unwrap();
        let pending = hub.pending_profile_deletions().unwrap();
        assert_eq!(pending.len(), 1);
        assert!(pending[0].native_erasure_verified);
        assert!(hub
            .completed_profile_deletion_tombstones()
            .unwrap()
            .is_empty());

        // Native proof is preserved; only the local authorized artifact
        // is retried and tombstoned for another restart observation.
        assert!(hub
            .finalize_profile_deletion_requiring_restart_confirmation(profile)
            .unwrap());
        assert!(!wal_path.exists());
        assert!(hub.pending_profile_deletions().unwrap().is_empty());
        assert_eq!(
            hub.completed_profile_deletion_tombstones().unwrap(),
            vec![profile]
        );
    }

    let hub = Hub::open_for_new_process(dir.path().to_path_buf()).unwrap();
    assert!(hub.pending_profile_deletions().unwrap().is_empty());
    assert!(hub
        .completed_profile_deletion_tombstones()
        .unwrap()
        .is_empty());
}

#[test]
fn crash_after_unlink_but_before_completion_marker_keeps_authorization_pending() {
    let dir = tempfile::tempdir().unwrap();
    let profile = ProfileId::from(1);
    let path = dir.path().join(format!("profile-{profile}.sqlite"));
    {
        let mut hub = Hub::open(dir.path().to_path_buf()).unwrap();
        hub.save(&sample()).unwrap();
        hub.record_visit(profile, "https://private.example/", "Private");
        assert_eq!(
            hub.authorize_profile_deletion(profile, &SessionState::default())
                .unwrap(),
            ProfileDeletionAuthorizeOutcome::Authorized
        );
        hub.fail_next_profile_deletion_after_local_purge();
        assert!(hub
            .finalize_profile_deletion_requiring_restart_confirmation(profile)
            .is_err());
        assert!(!path.exists());
        let pending = hub.pending_profile_deletions().unwrap();
        assert_eq!(pending.len(), 1);
        assert!(pending[0].native_erasure_verified);
        assert!(hub
            .completed_profile_deletion_tombstones()
            .unwrap()
            .is_empty());
    }

    let mut hub = Hub::open_for_new_process(dir.path().to_path_buf()).unwrap();
    let pending = hub.pending_profile_deletions().unwrap();
    assert_eq!(pending.len(), 1);
    assert!(pending[0].native_erasure_verified);
    assert!(hub.finalize_profile_deletion(profile).unwrap());
    assert!(hub.pending_profile_deletions().unwrap().is_empty());
}

#[test]
fn profile_deletion_waits_across_restart_for_native_erasure_proof() {
    let dir = tempfile::tempdir().unwrap();
    let profile = ProfileId::from(1);
    let path = dir.path().join(format!("profile-{profile}.sqlite"));
    {
        let mut hub = Hub::open(dir.path().to_path_buf()).unwrap();
        hub.save(&sample()).unwrap();
        hub.record_visit(profile, "https://private.example/", "Private");
        assert!(path.exists());

        // Model process death after the authoritative registry/session
        // transaction but before the engine has verified native erasure.
        assert_eq!(
            hub.authorize_profile_deletion(profile, &SessionState::default())
                .unwrap(),
            ProfileDeletionAuthorizeOutcome::Authorized
        );
        assert!(path.exists());
        assert_eq!(hub.pending_profile_deletions().unwrap().len(), 1);
    }

    let mut hub = Hub::open(dir.path().to_path_buf()).unwrap();
    assert!(path.exists(), "startup must not infer native erasure");
    let pending = hub.pending_profile_deletions().unwrap();
    assert_eq!(pending.len(), 1);
    assert!(!pending[0].native_erasure_verified);
    assert!(hub.finalize_profile_deletion(profile).unwrap());
    assert!(!path.exists());
    assert!(hub.pending_profile_deletions().unwrap().is_empty());
}

#[test]
fn native_proof_is_durable_when_local_profile_purge_must_retry() {
    let dir = tempfile::tempdir().unwrap();
    let profile = ProfileId::from(1);
    let path = dir.path().join(format!("profile-{profile}.sqlite"));
    {
        let mut hub = Hub::open(dir.path().to_path_buf()).unwrap();
        hub.save(&sample()).unwrap();
        hub.record_visit(profile, "https://private.example/", "Private");
        assert_eq!(
            hub.authorize_profile_deletion(profile, &SessionState::default())
                .unwrap(),
            ProfileDeletionAuthorizeOutcome::Authorized
        );

        // Replace the now-closed exact file with a non-file so the local
        // purge fails after committing native proof.
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();
        assert!(hub.finalize_profile_deletion(profile).is_err());
        let pending = hub.pending_profile_deletions().unwrap();
        assert_eq!(pending.len(), 1);
        assert!(pending[0].native_erasure_verified);
    }

    let mut hub = Hub::open(dir.path().to_path_buf()).unwrap();
    let pending = hub.pending_profile_deletions().unwrap();
    assert_eq!(pending.len(), 1);
    assert!(pending[0].native_erasure_verified);
    std::fs::remove_dir(&path).unwrap();
    assert!(hub.finalize_profile_deletion(profile).unwrap());
    assert!(hub.pending_profile_deletions().unwrap().is_empty());
}

#[test]
fn profile_deletion_rejects_a_hard_link_to_another_profile_database() {
    let dir = tempfile::tempdir().unwrap();
    let deleted = ProfileId::from(1);
    let survivor = ProfileId::from(3);
    let deleted_path = dir.path().join(format!("profile-{deleted}.sqlite"));
    let survivor_path = dir.path().join(format!("profile-{survivor}.sqlite"));
    let mut hub = Hub::open(dir.path().to_path_buf()).unwrap();
    hub.save(&two_profile_sample()).unwrap();
    hub.record_visit(deleted, "https://deleted.example/", "Deleted");
    hub.record_visit(survivor, "https://survivor.example/", "Survivor");
    let filtered = SessionState {
        profiles: vec![PersistedProfile {
            id: survivor,
            name: "Work".into(),
            kind: ProfileKind::Named,
        }],
        spaces: vec![PersistedSpace {
            id: SpaceId::from(4),
            profile: survivor,
            name: "Work".into(),
        }],
        items: Vec::new(),
        active_space: Some(SpaceId::from(4)),
        active_item: None,
        splits: None,
        recently_closed: Vec::new(),
    };
    assert_eq!(
        hub.authorize_profile_deletion(deleted, &filtered).unwrap(),
        ProfileDeletionAuthorizeOutcome::Authorized
    );

    std::fs::remove_file(&deleted_path).unwrap();
    std::fs::hard_link(&survivor_path, &deleted_path).unwrap();
    assert!(hub.finalize_profile_deletion(deleted).is_err());
    assert_eq!(
        hub.search_history(survivor, "survivor", 10).len(),
        1,
        "foreign profile data was scrubbed through a hard link"
    );
    let pending = hub.pending_profile_deletions().unwrap();
    assert_eq!(pending.len(), 1);
    assert!(pending[0].native_erasure_verified);

    std::fs::remove_file(&deleted_path).unwrap();
    assert!(hub.finalize_profile_deletion(deleted).unwrap());
}

#[test]
fn actor_exposes_exact_profile_deletion_phases() {
    use zephium_core::ports::store::{
        PendingProfileDeletion, ProfileDeletionAuthorizeOutcome, ProfileDeletionFinalizeOutcome,
        ProfileDeletionLoad,
    };

    let store = SqliteStore::in_memory().unwrap();
    store.save_session(sample());
    assert!(store.flush());
    assert_eq!(
        store.authorize_profile_deletion(
            ProfileId::from(1),
            SessionState::default(),
            Instant::now() + Duration::from_secs(1),
        ),
        ProfileDeletionAuthorizeOutcome::Authorized
    );
    assert_eq!(
        store.pending_profile_deletions(),
        ProfileDeletionLoad::Loaded(vec![PendingProfileDeletion {
            profile: ProfileId::from(1),
            native_erasure_verified: false,
        }])
    );
    assert_eq!(
        store.finalize_profile_deletion(
            ProfileId::from(1),
            Instant::now() + Duration::from_secs(1),
        ),
        ProfileDeletionFinalizeOutcome::Completed
    );
    assert_eq!(
        store.pending_profile_deletions(),
        ProfileDeletionLoad::Loaded(Vec::new())
    );
}

#[test]
fn unauthorized_profile_finalization_never_deletes_an_orphan() {
    use zephium_core::ports::store::ProfileDeletionFinalizeOutcome;

    let dir = tempfile::tempdir().unwrap();
    let store = SqliteStore::open(dir.path()).unwrap();
    let profile = ProfileId::from(999);
    let orphan = dir.path().join(format!("profile-{profile}.sqlite"));
    std::fs::write(&orphan, b"unclaimed").unwrap();
    assert_eq!(
        store.finalize_profile_deletion(profile, Instant::now() + Duration::from_secs(1),),
        ProfileDeletionFinalizeOutcome::NotAuthorized
    );
    assert_eq!(std::fs::read(orphan).unwrap(), b"unclaimed");
}

#[cfg(unix)]
#[test]
fn unregistered_profile_shaped_symlink_is_ignored_and_never_followed() {
    use std::os::unix::fs::symlink;

    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("unrelated.sqlite");
    std::fs::write(&target, b"must remain untouched").unwrap();
    let profile = ProfileId::from(1);
    symlink(
        &target,
        dir.path().join(format!("profile-{profile}.sqlite")),
    )
    .unwrap();

    assert!(Hub::open(dir.path().to_path_buf()).is_ok());
    assert_eq!(std::fs::read(target).unwrap(), b"must remain untouched");
}

#[test]
fn startup_ignores_unregistered_profile_file_fanout_without_opening_files() {
    let dir = tempfile::tempdir().unwrap();
    drop(Hub::open(dir.path().to_path_buf()).unwrap());
    for value in 0..=256 {
        let profile = ProfileId::from(value as u128 + 1);
        std::fs::write(
            dir.path().join(format!("profile-{profile}.sqlite")),
            b"not opened",
        )
        .unwrap();
    }

    assert!(Hub::open(dir.path().to_path_buf()).is_ok());
}

#[test]
fn new_profile_never_claims_a_preexisting_orphan_database() {
    let dir = tempfile::tempdir().unwrap();
    let profile = ProfileId::from(1);
    let orphan = dir.path().join(format!("profile-{profile}.sqlite"));
    std::fs::write(&orphan, b"unclaimed").unwrap();
    let mut hub = Hub::open(dir.path().to_path_buf()).unwrap();

    assert!(hub.save(&sample()).is_err());
    assert_eq!(std::fs::read(orphan).unwrap(), b"unclaimed");
    let meta = Connection::open(dir.path().join("meta.sqlite")).unwrap();
    let profiles: i64 = meta
        .query_row("SELECT count(*) FROM profiles", [], |row| row.get(0))
        .unwrap();
    assert_eq!(profiles, 0);
}

#[test]
fn legacy_single_file_imports_once() {
    let dir = tempfile::tempdir().unwrap();
    let legacy_path = dir.path().join("default.sqlite");
    {
        let conn = Connection::open(&legacy_path).unwrap();
        conn.execute_batch(
            "CREATE TABLE session (id INTEGER PRIMARY KEY CHECK (id = 1), data TEXT NOT NULL);
             CREATE TABLE history (
                 id INTEGER PRIMARY KEY AUTOINCREMENT,
                 url TEXT NOT NULL,
                 title TEXT NOT NULL,
                 visited_at INTEGER NOT NULL
             );
             PRAGMA user_version = 1;",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO session(id, data) VALUES (1, ?1)",
            [
                r#"{"tabs":[{"url":"https://example.com/","title":"Example"},
                 {"url":"https://github.com/","title":"GitHub"}],"active":1}"#,
            ],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO history(url, title, visited_at) VALUES ('https://example.com/', 'Example', 1)",
            [],
        )
        .unwrap();
    }

    let store = SqliteStore::open(dir.path()).unwrap();
    let session = loaded(&store);
    assert_eq!(session.profiles.len(), 1);
    assert_eq!(session.items.len(), 2);
    assert!(session.active_item.is_some());
    // The source copy is removed only after both the authoritative state
    // and history marker have committed.
    assert!(!legacy_path.exists());
    assert!(!dir.path().join("default.sqlite.bak").exists());
    drop(store);

    let mut hub = Hub::open(dir.path().to_path_buf()).unwrap();
    assert_eq!(hub.history_count(session.profiles[0].id), 1);
    assert_eq!(hub.history_matches(session.profiles[0].id, "example"), 1);
}

#[test]
fn legacy_import_resumes_without_duplicating_committed_history() {
    let dir = tempfile::tempdir().unwrap();
    let profile = ProfileId::from(1);
    {
        let mut hub = Hub::open(dir.path().to_path_buf()).unwrap();
        hub.save(&sample()).unwrap();
        hub.record_visit(profile, "https://existing.example/", "Existing");
    }
    let profile_path = dir.path().join(format!("profile-{profile}.sqlite"));
    let profile_db = Connection::open(profile_path).unwrap();
    profile_db
        .execute(
            "INSERT INTO settings(key, value) VALUES (?1, '1')
             ON CONFLICT(key) DO UPDATE SET value = '1'",
            [hub::LEGACY_HISTORY_MARKER],
        )
        .unwrap();
    drop(profile_db);
    let meta = Connection::open(dir.path().join("meta.sqlite")).unwrap();
    meta.execute(
        "INSERT INTO settings(key, value) VALUES (?1, 'started')
         ON CONFLICT(key) DO UPDATE SET value = 'started'",
        [hub::LEGACY_IMPORT_STATE_KEY],
    )
    .unwrap();
    drop(meta);

    let legacy_path = dir.path().join("default.sqlite");
    let legacy = Connection::open(&legacy_path).unwrap();
    legacy
        .execute_batch(
            "CREATE TABLE session (id INTEGER PRIMARY KEY, data TEXT NOT NULL);
             CREATE TABLE history (
                 id INTEGER PRIMARY KEY, url TEXT, title TEXT, visited_at INTEGER
             );",
        )
        .unwrap();
    legacy
        .execute(
            "INSERT INTO session(id, data) VALUES (1, ?1)",
            [r#"{"tabs":[{"url":"https://legacy.example/","title":"Legacy"}],"active":0}"#],
        )
        .unwrap();
    legacy
        .execute(
            "INSERT INTO history VALUES (1, 'https://legacy.example/', 'Legacy', 1)",
            [],
        )
        .unwrap();
    drop(legacy);

    let mut hub = Hub::open(dir.path().to_path_buf()).unwrap();
    assert_eq!(hub.history_count(profile), 1);
    assert_eq!(
        hub.app_setting(hub::LEGACY_IMPORT_STATE_KEY).as_deref(),
        Some("complete")
    );
    assert!(!legacy_path.exists());
}

fn agent_audit_ledger() -> zephium_agentic::AgentAuditLedger {
    use zephium_agentic::{
        AgentAccountScope, AgentAuditEventId, AgentDelegationSpec, AgentDelegationTopology,
        AgentEffectScope, AgentPlanNodeAuthority, AgentPlanNodeId, AgentPlanNodeScope,
        AgentPolicyInstant, AgentRunBudget, AgentRunManifest, AgentRunManifestId, AgentRunScope,
        AgentRunSupervisor, AgentSupervisorId, ContextRunId, SemanticEffectClass, SemanticOrigin,
        SemanticSensitivity,
    };

    let profile = ProfileId::from(1);
    let origin =
        SemanticOrigin::parse("https://audit.example.test/private?secret=hidden").expect("origin");
    let effects = AgentEffectScope::try_new(&[SemanticEffectClass::Read]).expect("effects");
    let node = AgentPlanNodeId::generate();
    let manifest = AgentRunManifest::try_new(
        AgentRunManifestId::generate(),
        ContextRunId::generate(),
        AgentRunScope::try_new(
            vec![profile],
            vec![AgentAccountScope::Anonymous],
            vec![origin.clone()],
            SemanticSensitivity::Public,
            effects,
            Vec::new(),
        )
        .expect("scope"),
        AgentRunBudget::try_new(10, 1_000, 1_000, 1).expect("budget"),
        AgentPolicyInstant::from_millis(100),
        AgentPolicyInstant::from_millis(10_000),
        vec![AgentPlanNodeScope::new(
            node,
            AgentPlanNodeAuthority::try_new(
                vec![profile],
                vec![AgentAccountScope::Anonymous],
                vec![origin],
                SemanticSensitivity::Public,
                effects,
            )
            .expect("authority"),
            AgentRunBudget::try_new(10, 1_000, 1_000, 1).expect("node budget"),
            AgentPolicyInstant::from_millis(9_000),
        )],
    )
    .expect("manifest");
    let topology =
        AgentDelegationTopology::try_new(&manifest, vec![AgentDelegationSpec::new(node, None)])
            .expect("topology");
    let supervisor =
        AgentRunSupervisor::new(AgentSupervisorId::new(1).expect("supervisor"), topology);
    let mut ledger =
        zephium_agentic::AgentAuditLedger::try_new(&manifest, &supervisor).expect("ledger");
    ledger
        .record_current(
            &supervisor,
            node,
            AgentAuditEventId::new(1).expect("event"),
            AgentPolicyInstant::from_millis(100),
        )
        .expect("record");
    ledger
}

#[test]
fn agent_audit_actor_commits_and_reconciles_the_exact_in_flight_delivery() {
    use zephium_agentic::{
        AgentAuditDeliveryId, AgentAuditDeliveryOutcome, AgentAuditDispatch, AgentAuditPort,
    };

    let store = SqliteStore::in_memory().expect("store");
    let mut ledger = agent_audit_ledger();
    let delivery = ledger
        .begin_delivery(AgentAuditDeliveryId::new(1).expect("delivery"), 16)
        .expect("delivery");
    let proof = delivery.proof();
    let (first_tx, first_rx) = mpsc::sync_channel(1);
    assert_eq!(
        store.append(
            delivery,
            Box::new(move |settlement| first_tx.send(settlement).expect("first receiver")),
        ),
        AgentAuditDispatch::Accepted(proof)
    );
    let first = first_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("first settlement");
    assert_eq!(first.outcome(), AgentAuditDeliveryOutcome::Committed);

    // Model a lost shell handoff: retain the ledger, replay only the exact
    // delivery, and require the Store to acknowledge without a second row.
    let replay = ledger
        .current_delivery()
        .expect("current delivery")
        .expect("in flight");
    let (replay_tx, replay_rx) = mpsc::sync_channel(1);
    assert_eq!(
        store.append(
            replay,
            Box::new(move |settlement| replay_tx.send(settlement).expect("replay receiver")),
        ),
        AgentAuditDispatch::Accepted(proof)
    );
    let replayed = replay_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("replay settlement");
    assert_eq!(replayed, first);
    ledger.settle_delivery(replayed).expect("settle ledger");
    assert_eq!(ledger.status().committed(), 1);
    assert_eq!(
        store.shutdown_until(Instant::now() + Duration::from_secs(2)),
        StoreShutdownOutcome::Clean
    );
}

#[test]
fn agent_audit_actor_admission_is_independently_bounded_and_releases_commands() {
    use zephium_agentic::{
        AgentAuditDeliveryId, AgentAuditDeliveryOutcome, AgentAuditDispatch, AgentAuditPort,
        AgentAuditSinkFailure,
    };

    let (tx, rx) = mpsc::sync_channel(MAX_PENDING_AGENT_AUDIT_DELIVERIES);
    let store = test_store_with_sender(tx);
    let mut ledger = agent_audit_ledger();
    let first = ledger
        .begin_delivery(AgentAuditDeliveryId::new(1).expect("delivery"), 16)
        .expect("delivery");
    let proof = first.proof();
    for delivery in std::iter::once(first).chain(
        (1..MAX_PENDING_AGENT_AUDIT_DELIVERIES)
            .map(|_| ledger.current_delivery().unwrap().unwrap()),
    ) {
        assert_eq!(
            store.append(delivery, Box::new(|_| {})),
            AgentAuditDispatch::Accepted(proof)
        );
    }
    let refused = store.append(
        ledger.current_delivery().unwrap().unwrap(),
        Box::new(|_| panic!("capacity refusal transferred callback")),
    );
    assert_eq!(
        refused,
        AgentAuditDispatch::Refused(proof.settle(AgentAuditDeliveryOutcome::Refused(
            AgentAuditSinkFailure::Capacity,
        )))
    );
    struct PanicOnDrop;
    impl Drop for PanicOnDrop {
        fn drop(&mut self) {
            panic!("integration callback destructor");
        }
    }
    let callback_capture = PanicOnDrop;
    let discard = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        store.append(
            ledger.current_delivery().unwrap().unwrap(),
            Box::new(move |_| {
                let _retained_until_drop = &callback_capture;
            }),
        )
    }))
    .expect("capacity refusal contains callback destructor panic");
    assert_eq!(
        discard,
        AgentAuditDispatch::Refused(proof.settle(AgentAuditDeliveryOutcome::Refused(
            AgentAuditSinkFailure::Capacity,
        )))
    );
    drop(rx);
    assert_eq!(
        store
            .agent_audit_delivery_admission
            .get()
            .expect("admission")
            .load(Ordering::Acquire),
        0
    );
}

#[test]
fn agent_audit_completion_panic_is_contained_by_the_actor() {
    use zephium_agentic::{AgentAuditDeliveryId, AgentAuditDispatch, AgentAuditPort};

    let store = SqliteStore::in_memory().expect("store");
    let mut ledger = agent_audit_ledger();
    let delivery = ledger
        .begin_delivery(AgentAuditDeliveryId::new(1).expect("delivery"), 16)
        .expect("delivery");
    let proof = delivery.proof();
    assert_eq!(
        store.append(delivery, Box::new(|_| panic!("integration callback"))),
        AgentAuditDispatch::Accepted(proof)
    );

    let replay = ledger
        .current_delivery()
        .expect("current delivery")
        .expect("in flight");
    let (settled_tx, settled_rx) = mpsc::sync_channel(1);
    assert_eq!(
        store.append(
            replay,
            Box::new(move |settlement| settled_tx.send(settlement).expect("receiver")),
        ),
        AgentAuditDispatch::Accepted(proof)
    );
    let settlement = settled_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("actor survived callback panic");
    ledger.settle_delivery(settlement).expect("settle replay");
    assert_eq!(
        store.shutdown_until(Instant::now() + Duration::from_secs(2)),
        StoreShutdownOutcome::Clean
    );
}

#[test]
fn history_search_ranks_by_frecency_and_never_starves_on_one_busy_address() {
    let mut hub = Hub::in_memory().unwrap();
    hub.save(&sample()).unwrap();
    let profile = ProfileId::from(1);
    // One address visited far more often than the scan cap used to allow.
    // Capping candidates before the group-by let this address consume every
    // slot, hiding every other match for the same term.
    for _ in 0..600 {
        hub.record_visit(profile, "https://example.com/busy", "Example Busy");
    }
    hub.record_visit(profile, "https://example.com/quiet", "Example Quiet");
    hub.record_visit(profile, "https://example.org/other", "Example Other");

    let hits = hub.search_history(profile, "example", 10);
    let urls: Vec<&str> = hits.iter().map(|hit| hit.url.as_str()).collect();
    assert!(
        urls.contains(&"https://example.com/quiet") && urls.contains(&"https://example.org/other"),
        "a busy address must not hide the rest: {urls:?}"
    );
    // Frecency, not recency: the daily destination outranks the page opened
    // once, even though that page was visited more recently.
    assert_eq!(hits[0].url, "https://example.com/busy");
}

#[test]
fn history_pages_every_visit_newest_first_without_gaps_or_repeats() {
    let mut hub = Hub::in_memory().unwrap();
    hub.save(&sample()).unwrap();
    let profile = ProfileId::from(1);
    for index in 0..25 {
        hub.record_visit(profile, &format!("https://example.com/{index}"), "Example");
    }

    let mut seen = Vec::new();
    let mut cursor = None;
    loop {
        let page = hub.history_page(profile, "", None, cursor, 10);
        if page.is_empty() {
            break;
        }
        cursor = page.last().map(|visit| visit.id);
        seen.extend(page);
    }

    assert_eq!(seen.len(), 25);
    // Visits recorded in the same second still page exactly, because the cursor
    // is the row id rather than the timestamp.
    assert!(seen.windows(2).all(|pair| pair[0].id > pair[1].id));
    assert_eq!(seen[0].url, "https://example.com/24");
}

#[test]
fn downloads_are_profile_scoped_and_recover_interrupted_native_ownership() {
    use zephium_core::downloads::*;
    use zephium_core::ids::DownloadId;
    let mut hub = Hub::in_memory().unwrap();
    hub.save(&sample()).unwrap();
    let profile = ProfileId::from(1);
    let id = DownloadId::generate();
    let session = DownloadId::generate();
    let mut record = DownloadRecord {
        id,
        session,
        revision: 1,
        created_at: 1,
        filename: "fixture.txt".into(),
        source: "https://example.com".into(),
        source_is_context: false,
        state: DownloadState::Pending,
        received: 0,
        total: None,
        error: None,
        destination: None,
        staging: None,
        staging_identity: None,
        identity: None,
        writer: None,
        writer_released: false,
    };
    assert!(matches!(
        hub.download_call(profile, DownloadStoreCall::Save(Box::new(record.clone()))),
        DownloadStoreReply::Saved
    ));
    assert!(matches!(
        hub.download_call(profile, DownloadStoreCall::Save(Box::new(record.clone()))),
        DownloadStoreReply::Saved
    ));
    record.filename = "changed.txt".into();
    assert!(matches!(
        hub.download_call(profile, DownloadStoreCall::Save(Box::new(record.clone()))),
        DownloadStoreReply::Error(DownloadError::Invalid)
    ));
    assert!(matches!(
        hub.download_call(ProfileId::from(999), DownloadStoreCall::Get(id)),
        DownloadStoreReply::Error(DownloadError::Storage)
    ));
    assert!(matches!(
        hub.download_call(profile, DownloadStoreCall::Forget(id)),
        DownloadStoreReply::Error(DownloadError::Invalid)
    ));
    let DownloadStoreReply::Page(page) = hub.download_call(
        profile,
        DownloadStoreCall::List {
            before: None,
            limit: 10,
            session: DownloadId::generate(),
            active: Vec::new(),
        },
    ) else {
        panic!("download page")
    };
    assert_eq!(page.len(), 1);
    assert_eq!(page[0].state, DownloadState::Interrupted);
    record.revision = 2;
    assert!(matches!(
        hub.download_call(profile, DownloadStoreCall::Save(Box::new(record))),
        DownloadStoreReply::Error(DownloadError::Invalid)
    ));
    assert!(matches!(
        hub.download_call(profile, DownloadStoreCall::Forget(id)),
        DownloadStoreReply::Saved
    ));
}

#[test]
fn download_preferences_are_validated_and_persist_only_in_registered_profiles() {
    use zephium_core::downloads::*;
    let mut hub = Hub::in_memory().unwrap();
    hub.save(&sample()).unwrap();
    let profile = ProfileId::from(1);
    let value = DownloadPreferences {
        ask_destination: false,
        directory: Some(
            std::env::temp_dir()
                .join("download-fixtures")
                .to_string_lossy()
                .into_owned(),
        ),
        directory_identity: Some("0000000000000001:0000000000000002".into()),
    };
    assert!(matches!(
        hub.download_call(profile, DownloadStoreCall::SetPreferences(
            DownloadPreferenceChange::AskDestination(false)
        )), DownloadStoreReply::Preferences(saved) if !saved.ask_destination
    ));
    assert!(matches!(
        hub.download_call(profile, DownloadStoreCall::SetPreferences(
            DownloadPreferenceChange::Directory {
                path: value.directory.clone().unwrap(), identity: value.directory_identity.clone().unwrap(),
            }
        )), DownloadStoreReply::Preferences(saved) if saved == value
    ));
    assert!(
        matches!(hub.download_call(profile, DownloadStoreCall::Preferences), DownloadStoreReply::Preferences(saved) if saved == value)
    );
    assert!(matches!(
        hub.download_call(
            ProfileId::from(999),
            DownloadStoreCall::SetPreferences(DownloadPreferenceChange::AskDestination(true))
        ),
        DownloadStoreReply::Error(DownloadError::Storage)
    ));
    assert!(matches!(
        hub.download_call(
            profile,
            DownloadStoreCall::SetPreferences(DownloadPreferenceChange::Directory {
                path: "relative".into(),
                identity: "0000000000000001:0000000000000002".into()
            })
        ),
        DownloadStoreReply::Error(DownloadError::Invalid)
    ));
}

#[test]
fn history_page_narrows_by_query_and_keeps_repeat_visits() {
    let mut hub = Hub::in_memory().unwrap();
    hub.save(&sample()).unwrap();
    let profile = ProfileId::from(1);
    hub.record_visit(profile, "https://example.com/docs", "Documentation");
    hub.record_visit(profile, "https://example.com/docs", "Documentation");
    hub.record_visit(profile, "https://other.example/news", "Headlines");

    let page = hub.history_page(profile, "documentation", None, None, 10);
    assert_eq!(
        page.len(),
        2,
        "a history list shows each visit, not each page"
    );
    assert!(page
        .iter()
        .all(|visit| visit.url == "https://example.com/docs"));
    assert!(hub
        .history_page(profile, "documentation", None, None, 0)
        .is_empty());
}

#[test]
fn forgetting_an_address_removes_it_from_search_and_the_byte_ledger() {
    let mut hub = Hub::in_memory().unwrap();
    hub.save(&sample()).unwrap();
    let profile = ProfileId::from(1);
    hub.record_visit(profile, "https://forget.example/page", "Forgettable");
    hub.record_visit(profile, "https://forget.example/page", "Forgettable");
    hub.record_visit(profile, "https://keep.example/page", "Keepsake");
    let before = hub.history_bytes(profile);

    assert_eq!(
        hub.forget_history_urls(profile, &["https://forget.example/page".to_owned()]),
        2
    );

    assert!(hub.search_history(profile, "forgettable", 10).is_empty());
    assert_eq!(hub.search_history(profile, "keepsake", 10).len(), 1);
    assert!(hub.history_bytes(profile) < before);
}

#[test]
fn clearing_a_range_leaves_older_visits_and_drops_recorded_searches() {
    let mut hub = Hub::in_memory().unwrap();
    hub.save(&sample()).unwrap();
    let profile = ProfileId::from(1);
    hub.record_visit(profile, "https://old.example/page", "Ancient");
    hub.backdate_history(profile, 7 * 24 * 3600);
    hub.record_visit(profile, "https://new.example/page", "Recent");
    hub.record_search(
        profile,
        "recent query",
        "https://duckduckgo.com/?q=recent+query",
    );

    let cleared = hub.clear_history(
        profile,
        Some(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs() as i64
                - 3600,
        ),
    );

    assert_eq!(cleared, 1);
    assert_eq!(hub.search_history(profile, "ancient", 10).len(), 1);
    assert!(hub.search_history(profile, "recent", 10).is_empty());
    assert!(hub.search_queries(profile, "recent").is_empty());
}

#[test]
fn a_title_published_after_the_url_commits_replaces_the_placeholder() {
    let mut hub = Hub::in_memory().unwrap();
    hub.save(&sample()).unwrap();
    let profile = ProfileId::from(1);
    // The shell records a visit the moment a URL commits, which is before the
    // document has a title of its own.
    hub.record_visit(profile, "https://example.com/article", "example.com");

    hub.amend_visit_title(profile, "https://example.com/article", "The Real Headline");

    let page = hub.history_page(profile, "", None, None, 10);
    assert_eq!(page[0].title, "The Real Headline");
    assert_eq!(hub.search_history(profile, "headline", 10).len(), 1);

    hub.backdate_history(profile, 300);
    hub.amend_visit_title(profile, "https://example.com/article", "Too Late");
    assert_eq!(
        hub.history_page(profile, "", None, None, 10)[0].title,
        "The Real Headline"
    );
}

#[test]
fn a_scoped_history_page_reaches_back_only_as_far_as_its_range() {
    let mut hub = Hub::in_memory().unwrap();
    hub.save(&sample()).unwrap();
    let profile = ProfileId::from(1);
    hub.record_visit(profile, "https://old.example/page", "Ancient");
    hub.backdate_history(profile, 3 * 24 * 3600);
    hub.record_visit(profile, "https://new.example/page", "Recent");
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;

    let everything = hub.history_page(profile, "", None, None, 10);
    let last_hour = hub.history_page(profile, "", Some(now - 3600), None, 10);

    assert_eq!(everything.len(), 2);
    assert_eq!(last_hour.len(), 1, "a scoped page must not reach past it");
    assert_eq!(last_hour[0].url, "https://new.example/page");

    // The scope applies to a narrowed list too, not just the unfiltered one.
    assert!(hub
        .history_page(profile, "ancient", Some(now - 3600), None, 10)
        .is_empty());
    assert_eq!(
        hub.history_page(profile, "ancient", None, None, 10).len(),
        1
    );
}

#[test]
fn saving_an_icon_clears_rows_that_can_never_be_read_back() {
    let mut hub = Hub::in_memory().unwrap();
    hub.save(&sample()).unwrap();
    let profile = ProfileId::from(1);
    hub.save_favicon(profile, "https://keep.example", None, &rgba());
    // A row from before the fixed-raster format: readable queries reject it,
    // so it is invisible but still occupies a retention slot.
    hub.seed_malformed_favicon(profile, "https://legacy.example", 5430);
    assert_eq!(hub.favicon_rows(profile), 2);

    hub.save_favicon(profile, "https://later.example", None, &rgba());

    assert_eq!(hub.favicon_rows(profile), 2, "the unreadable row is gone");
    assert!(hub
        .favicon_raster_with_age(profile, "https://keep.example")
        .is_some());
    assert!(hub
        .favicon_raster_with_age(profile, "https://legacy.example")
        .is_none());
}

#[test]
fn blocker_statistics_roundtrip_and_clear_with_browsing_data() {
    let mut hub = Hub::in_memory().unwrap();
    let session = sample();
    let profile = session.profiles[0].id;
    hub.save(&session).unwrap();
    let mut statistics = zephium_core::blocker::BlockerStatistics::default();
    statistics.record(700_000, 12);
    statistics.record(700_001, 9);
    hub.save_blocker_statistics(profile, &statistics).unwrap();
    assert_eq!(hub.load_blocker_statistics(profile).unwrap(), statistics);
    hub.clear_history(profile, None);
    assert_eq!(
        hub.load_blocker_statistics(profile).unwrap(),
        Default::default()
    );
    assert!(hub
        .save_blocker_statistics(ProfileId::from(999), &statistics)
        .is_err());
}
