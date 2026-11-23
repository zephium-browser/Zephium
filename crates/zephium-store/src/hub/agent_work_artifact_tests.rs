//! SQLite fault fixtures exercise persisted facts, not successful execution.
//! The application suite separately supplies real controller terminal owners.
use super::tests::{
    clean_runs, initial, insert_settled, journal, open, open_hub, simulate_process_exit,
    work_test_guard, Fault, FAULT,
};
use super::*;
use zephium_agentic::AgentWorkDebt;

const EMPTY_ARCHIVE: &[u8] = br#"{"version":1,"id":[1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1],"profile":"00000000000000000000000001","key":[0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,1,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,1],"schema":1,"observation":1,"generation":1,"captured_millis":1,"fields":[],"sources":[]}"#;
const EMPTY_DIGEST: [u8; 32] = [
    0x45, 0xc4, 0xfa, 0x1f, 0x7d, 0xec, 0xe9, 0xb3, 0x4e, 0xdd, 0xbf, 0x73, 0x6a, 0x08, 0xf0, 0xab,
    0x1c, 0x2f, 0xf2, 0x7a, 0x4e, 0x19, 0xfc, 0x17, 0x95, 0x75, 0xaa, 0x3c, 0x77, 0x66, 0x77, 0x90,
];

#[test]
fn historical_evidence_requires_exact_profile_source_and_verified_archive_bytes() {
    use sha2::{Digest, Sha256};
    use zephium_core::work::{artifact::WorkEvidenceLink, WorkError};
    let _process = work_test_guard();
    let (_directory, mut hub, owner) = open();
    let (running, succeeded, original) = fixture(&mut hub, owner);
    let source = r#""sources":[{"id":1,"origin":"https://fixture.invalid/","role":"paragraph","field":1,"context":[1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1],"context_generation":1,"navigation_epoch":1,"frame":1,"frame_generation":1,"invocation":1,"snapshot":1,"reference":1,"browser_derived":true,"content":{"kind":"text","value":"Historical source quotation"}}]"#;
    let body = std::str::from_utf8(EMPTY_ARCHIVE)
        .unwrap()
        .replace("\"sources\":[]", source);
    let descriptor = AgentWorkArtifactDescriptor::decode(
        original.descriptor.id(),
        1_u128.into(),
        running.key(),
        Sha256::digest(body.as_bytes()).into(),
        body.len() as u32,
    )
    .unwrap();
    AgentWorkArchivedExtraction::decode(descriptor, body.as_bytes()).unwrap();
    compare_and_set_records(
        journal(&mut hub),
        Some(running),
        succeeded,
        None,
        Some(ArtifactStorageValue {
            descriptor,
            body: body.as_bytes(),
        }),
    )
    .unwrap();
    let link = WorkEvidenceLink {
        extraction_id: u128::from_be_bytes(descriptor.id()).into(),
        source_id: 1,
    };
    let preview = read_work_evidence(journal(&mut hub), 1_u128.into(), link.clone()).unwrap();
    assert_eq!(preview.text, "Historical source quotation");
    assert!(!preview.truncated);
    assert_eq!(preview.source_bytes, preview.text.len().to_string());
    assert!(!format!("{preview:?}").contains("quotation"));
    assert!(matches!(
        read_work_evidence(journal(&mut hub), 2_u128.into(), link.clone()),
        Err(WorkError::NotFound)
    ));
    let mut missing = link.clone();
    missing.source_id = 2;
    assert!(matches!(
        read_work_evidence(journal(&mut hub), 1_u128.into(), missing),
        Err(WorkError::NotFound)
    ));
    // Corrupt persisted bytes are not promoted to source content.
    journal(&mut hub)
        .execute_batch("DROP TRIGGER agent_work_artifact_immutable")
        .unwrap();
    let tampered = body.replace("Historical", "Tampered!!");
    journal(&mut hub)
        .execute(
            "UPDATE agent_work_artifacts SET body = ?1",
            [tampered.as_bytes()],
        )
        .unwrap();
    assert!(matches!(
        read_work_evidence(journal(&mut hub), 1_u128.into(), link),
        Err(WorkError::Invalid)
    ));
}

#[test]
fn artifact_migration_limits_and_profile_erasure_match_the_closed_contract() {
    let source = include_str!("../migrations.rs");
    assert!(source.contains(&format!(
        "length(body) BETWEEN 1 AND {}",
        MAX_AGENT_WORK_ARTIFACT_BYTES
    )));
    assert!(source.contains(&format!(
        "length(NEW.body) > {}",
        MAX_AGENT_WORK_ARTIFACT_TOTAL_BYTES
    )));
    assert!(source.contains("profile_id TEXT NOT NULL REFERENCES profiles(id) ON DELETE CASCADE"));
    assert!(source.contains("WHEN NEW.result_profile IS NOT OLD.result_profile"));
}

fn register(hub: &mut Hub) {
    use zephium_core::{
        profiles::ProfileKind,
        session::{PersistedProfile, PersistedSpace, SessionState},
    };
    hub.save(&SessionState {
        profiles: vec![PersistedProfile {
            id: 1_u128.into(),
            name: "Fixture".into(),
            kind: ProfileKind::Default,
        }],
        spaces: vec![PersistedSpace {
            id: 2_u128.into(),
            profile: 1_u128.into(),
            name: "Fixture".into(),
            look: Default::default(),
        }],
        items: vec![],
        active_space: Some(2_u128.into()),
        active_item: None,
        splits: None,
        recently_closed: vec![],
    })
    .unwrap();
}

fn fixture(
    hub: &mut Hub,
    owner: AgentWorkIncarnation,
) -> (
    AgentWorkRecord,
    AgentWorkRecord,
    ArtifactStorageValue<'static>,
) {
    register(hub);
    let admitted = initial(owner, 1);
    compare_and_set_records(journal(hub), None, admitted, Some(1_u128.into()), None).unwrap();
    let running = admitted.transition(AgentWorkDisposition::Running).unwrap();
    compare_and_set_records(journal(hub), Some(admitted), running, None, None).unwrap();
    // Raw persisted success is fixture data, never a core publication owner.
    let mut bytes = *running.as_bytes();
    bytes[1] = 6;
    bytes[2] = 0;
    bytes[15] = 3;
    let succeeded = AgentWorkRecord::decode(bytes).unwrap();
    let descriptor = AgentWorkArtifactDescriptor::decode(
        [1; 16],
        1_u128.into(),
        running.key(),
        EMPTY_DIGEST,
        EMPTY_ARCHIVE.len() as u32,
    )
    .unwrap();
    AgentWorkArchivedExtraction::decode(descriptor, EMPTY_ARCHIVE).unwrap();
    (
        running,
        succeeded,
        ArtifactStorageValue {
            descriptor,
            body: EMPTY_ARCHIVE,
        },
    )
}

#[test]
fn partial_result_or_terminal_writes_rollback_together_and_commit_loss_reconciles_once() {
    for fault in [
        Fault::BeforeWrite,
        Fault::AfterArtifactWrite,
        Fault::AfterWrite,
        Fault::AfterCommit,
    ] {
        let _process = work_test_guard();
        let (_directory, mut hub, owner) = open();
        let (running, succeeded, artifact) = fixture(&mut hub, owner);
        FAULT.with(|slot| slot.set(Some(fault)));
        assert_eq!(
            compare_and_set_records(
                journal(&mut hub),
                Some(running),
                succeeded,
                None,
                Some(artifact)
            ),
            Err(Error::Uncertain)
        );
        let committed = fault == Fault::AfterCommit;
        assert_eq!(
            read(journal(&mut hub), running.key()).unwrap(),
            Some(if committed { succeeded } else { running })
        );
        assert_eq!(
            read_artifact(journal(&mut hub), running.key(), 1_u128.into())
                .unwrap()
                .is_some(),
            committed
        );
        compare_and_set_records(
            journal(&mut hub),
            Some(running),
            succeeded,
            None,
            Some(artifact),
        )
        .unwrap();
        compare_and_set_records(
            journal(&mut hub),
            Some(running),
            succeeded,
            None,
            Some(artifact),
        )
        .unwrap();
        assert_eq!(
            journal(&mut hub)
                .query_row("SELECT count(*) FROM agent_work_artifacts", [], |row| row
                    .get::<_, i64>(
                    0
                ))
                .unwrap(),
            1
        );
        assert_eq!(
            read(journal(&mut hub), running.key()).unwrap(),
            Some(succeeded)
        );
        drop(hub);
    }
}

#[test]
fn promised_result_cannot_be_downgraded_or_replaced_by_conflicting_publication() {
    let _process = work_test_guard();
    let (_directory, mut hub, owner) = open();
    let (running, succeeded, artifact) = fixture(&mut hub, owner);
    assert_eq!(
        compare_and_set_records(journal(&mut hub), Some(running), succeeded, None, None),
        Err(Error::Transition)
    );
    compare_and_set_records(
        journal(&mut hub),
        Some(running),
        succeeded,
        None,
        Some(artifact),
    )
    .unwrap();
    let changed = ArtifactStorageValue {
        descriptor: AgentWorkArtifactDescriptor::decode(
            [2; 16],
            1_u128.into(),
            running.key(),
            EMPTY_DIGEST,
            EMPTY_ARCHIVE.len() as u32,
        )
        .unwrap(),
        ..artifact
    };
    assert_eq!(
        compare_and_set_records(
            journal(&mut hub),
            Some(running),
            succeeded,
            None,
            Some(changed)
        ),
        Err(Error::Conflict)
    );
    assert_eq!(
        compare_and_set_records(journal(&mut hub), Some(running), succeeded, None, None),
        Err(Error::Transition)
    );
    assert!(journal(&mut hub)
        .execute(
            "UPDATE agent_work_artifacts SET body = body WHERE run_key = ?1",
            [running.key().as_slice()]
        )
        .is_err());
    assert!(journal(&mut hub)
        .execute(
            "UPDATE agent_work_runs SET result_profile = NULL WHERE run_key = ?1",
            [running.key().as_slice()]
        )
        .is_err());
    drop(hub);
}

#[test]
fn restart_reads_archived_data_with_new_fence_but_never_reopens_the_terminal() {
    let _process = work_test_guard();
    let (directory, mut hub, owner) = open();
    let (running, succeeded, artifact) = fixture(&mut hub, owner);
    compare_and_set_records(
        journal(&mut hub),
        Some(running),
        succeeded,
        None,
        Some(artifact),
    )
    .unwrap();
    drop(hub);
    simulate_process_exit();
    let mut hub = open_hub(directory.path()).unwrap();
    let Reply::Claimed {
        owner: fresh,
        records,
    } = hub.agent_work(Request::Claim).unwrap()
    else {
        panic!()
    };
    assert_ne!(fresh, owner);
    assert_eq!(records, [succeeded]);
    assert!(matches!(
        hub.agent_work_artifact(AgentWorkArtifactRequest::Read {
            owner,
            record: succeeded,
            profile: 1_u128.into()
        }),
        Err(Error::Fenced)
    ));
    assert!(matches!(
        hub.agent_work_artifact(AgentWorkArtifactRequest::Read {
            owner: fresh,
            record: running,
            profile: 1_u128.into()
        }),
        Err(Error::Conflict)
    ));
    assert!(matches!(
        hub.agent_work_artifact(AgentWorkArtifactRequest::Read {
            owner: fresh,
            record: succeeded,
            profile: 99_u128.into()
        }),
        Err(Error::Fenced)
    ));
    let AgentWorkArtifactReply::Read(Some(result)) = hub
        .agent_work_artifact(AgentWorkArtifactRequest::Read {
            owner: fresh,
            record: succeeded,
            profile: 1_u128.into(),
        })
        .unwrap()
    else {
        panic!()
    };
    assert_eq!(result.descriptor(), artifact.descriptor);
    assert_eq!(
        result.trust(),
        zephium_agentic::SemanticExtractionTrust::ModelMapped
    );
    assert_eq!(
        read(journal(&mut hub), running.key()).unwrap(),
        Some(succeeded)
    );
    drop(hub);
}

#[test]
fn profile_erasure_removes_private_body_without_rewriting_terminal_or_restoring_intent() {
    let _process = work_test_guard();
    let (_directory, mut hub, owner) = open();
    let (running, succeeded, artifact) = fixture(&mut hub, owner);
    compare_and_set_records(
        journal(&mut hub),
        Some(running),
        succeeded,
        None,
        Some(artifact),
    )
    .unwrap();
    // Persisted fixture cleanup supplies the protected intent on Windows;
    // legacy metadata retains its original FK. Neither is native erasure proof.
    #[cfg(windows)]
    hub.authorize_windows_work_artifact_deletion(1_u128.into())
        .unwrap();
    hub.meta
        .execute(
            "DELETE FROM profiles WHERE id = ?1",
            [ProfileId::from(1).to_string()],
        )
        .unwrap();
    hub.registry.remove(&1_u128.into());
    #[cfg(windows)]
    hub.purge_windows_work_artifacts(1_u128.into()).unwrap();
    assert!(
        read_artifact(journal(&mut hub), running.key(), 1_u128.into())
            .unwrap()
            .is_none()
    );
    assert_eq!(
        read(journal(&mut hub), running.key()).unwrap(),
        Some(succeeded)
    );
    assert_eq!(
        result_profile(journal(&mut hub), running.key()).unwrap(),
        Some(1_u128.into())
    );
    assert!(matches!(
        hub.agent_work_artifact(AgentWorkArtifactRequest::Read {
            owner,
            record: succeeded,
            profile: 1_u128.into()
        }),
        Err(Error::Fenced)
    ));
    drop(hub);
}

#[test]
fn result_retention_pressure_never_evicts_and_corrupt_bodies_fail_closed() {
    let _process = work_test_guard();
    let (_directory, mut hub, owner) = open();
    let (running, succeeded, artifact) = fixture(&mut hub, owner);
    for key in 2..=129_u16 {
        let record = initial(owner, key);
        compare_and_set_records(journal(&mut hub), None, record, None, None).unwrap();
        journal(&mut hub).execute("INSERT INTO agent_work_artifacts(run_key, profile_id, artifact_id, digest, body) VALUES (?1, ?2, ?3, ?4, zeroblob(?5))",
            params![record.key().as_slice(), ProfileId::from(1).to_string(), (key as u128).to_be_bytes().as_slice(), [0_u8;32].as_slice(), MAX_AGENT_WORK_ARTIFACT_BYTES]).unwrap();
    }
    assert_eq!(
        compare_and_set_records(
            journal(&mut hub),
            Some(running),
            succeeded,
            None,
            Some(artifact)
        ),
        Err(Error::Capacity)
    );
    assert_eq!(
        read(journal(&mut hub), running.key()).unwrap(),
        Some(running)
    );
    assert!(matches!(
        read_artifact(journal(&mut hub), initial(owner, 2).key(), 1_u128.into()),
        Err(Error::Uncertain)
    ));
    assert_eq!(
        journal(&mut hub)
            .query_row("SELECT count(*) FROM agent_work_artifacts", [], |row| row
                .get::<_, i64>(
                0
            ))
            .unwrap(),
        128
    );
    drop(hub);
}

#[test]
#[cfg(windows)]
fn protected_deletion_hooks_preserve_tombstone_on_purge_and_restart_failure() {
    use zephium_core::ports::store::ProfileDeletionAuthorizeOutcome;
    let _process = work_test_guard();
    let (directory, mut hub, owner) = open();
    let profile: ProfileId = 1_u128.into();
    let (running, succeeded, artifact) = fixture(&mut hub, owner);
    compare_and_set_records(
        journal(&mut hub),
        Some(running),
        succeeded,
        None,
        Some(artifact),
    )
    .unwrap();
    assert_eq!(
        hub.authorize_profile_deletion(profile, &zephium_core::session::SessionState::default())
            .unwrap(),
        ProfileDeletionAuthorizeOutcome::Authorized
    );
    // The actual finalization hook must preserve its ordinary-meta obligation
    // when the separately protected transaction cannot write.
    journal(&mut hub)
        .execute_batch("PRAGMA query_only=ON")
        .unwrap();
    assert!(hub.finalize_profile_deletion(profile).is_err());
    assert_eq!(hub.pending_profile_deletions().unwrap().len(), 1);
    assert!(read_artifact(journal(&mut hub), running.key(), profile)
        .unwrap()
        .is_some());
    journal(&mut hub)
        .execute_batch("PRAGMA query_only=OFF")
        .unwrap();
    assert!(hub.finalize_profile_deletion(profile).unwrap());
    assert_eq!(
        hub.completed_profile_deletion_tombstones().unwrap(),
        vec![profile]
    );
    assert!(read_artifact(journal(&mut hub), running.key(), profile)
        .unwrap()
        .is_none());
    assert_eq!(
        read(journal(&mut hub), running.key()).unwrap(),
        Some(succeeded)
    );
    let selector = work_test_storage(directory.path());
    drop(hub);
    // A different restart generation cannot discard the final tombstone while
    // the original native journal lease is still unavailable for protected purge.
    assert!(Hub::open_prepared(
        directory.path().into(),
        ProfileId::generate(),
        Some(selector.clone())
    )
    .is_err());
    let metadata = Connection::open_with_flags(
        directory.path().join("meta.sqlite"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    assert_eq!(
        metadata
            .query_row(
                "SELECT count(*) FROM profile_deletion_journal WHERE profile_id = ?1",
                [profile.to_string()],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        1
    );
    drop(metadata);
    simulate_process_exit();
    let mut reopened = Hub::open_prepared(
        directory.path().into(),
        ProfileId::generate(),
        Some(selector),
    )
    .unwrap();
    // Restart exposes the obligation again; only a finalize retires it.
    assert_eq!(
        reopened.completed_profile_deletion_tombstones().unwrap(),
        vec![profile]
    );
    assert_eq!(reopened.pending_profile_deletions().unwrap().len(), 1);
    assert!(reopened.finalize_profile_deletion(profile).unwrap());
    assert!(reopened
        .completed_profile_deletion_tombstones()
        .unwrap()
        .is_empty());
    assert!(reopened.pending_profile_deletions().unwrap().is_empty());
    assert!(
        read_artifact(journal(&mut reopened), running.key(), profile)
            .unwrap()
            .is_none()
    );
    assert_eq!(
        read(journal(&mut reopened), running.key()).unwrap(),
        Some(succeeded)
    );
    drop(reopened);
}

#[test]
fn full_result_budget_prunes_old_results_with_their_runs_but_keeps_cited_ones() {
    let _process = work_test_guard();
    let (_directory, mut hub, owner) = open();
    let (running, succeeded, artifact) = fixture(&mut hub, owner);
    let transaction = journal(&mut hub).transaction().unwrap();
    for key in 2..=129_u16 {
        let record = insert_settled(
            &transaction,
            owner,
            key,
            AgentWorkDisposition::Succeeded,
            AgentWorkDebt::NONE,
            Some(i64::from(key)),
        );
        transaction.execute("INSERT INTO agent_work_artifacts(run_key, profile_id, artifact_id, digest, body) VALUES (?1, ?2, ?3, ?4, zeroblob(?5))",
            params![record.key().as_slice(), ProfileId::from(1).to_string(), u128::from(key).to_be_bytes().as_slice(), [0_u8; 32].as_slice(), MAX_AGENT_WORK_ARTIFACT_BYTES]).unwrap();
    }
    transaction.commit().unwrap();
    // The oldest result is still cited by a Work execution on the canvas.
    let cited = ulid::Ulid(2).to_string();
    let profile = hub.profile_conn(1_u128.into()).unwrap();
    profile.execute("INSERT INTO works(id, schema_version, revision, status, objective, created_unix_ms, updated_unix_ms, lifecycle, objective_revision, context_revision, objective_author) VALUES ('00000000000000000000000001', 2, 1, 'draft', 'Fixture', 1, 1, 'active', 1, 1, 'user')", []).unwrap();
    profile.execute("INSERT INTO work_plans(work_id, revision, plan_id, author, basis_revision) VALUES ('00000000000000000000000001', 2, '00000000000000000000000002', 'user', 1)", []).unwrap();
    profile.execute("INSERT INTO work_executions(work_id, execution_id, plan_revision, owner_session, approved_unix_ms, expires_unix_ms, body) VALUES ('00000000000000000000000001', '00000000000000000000000003', 2, '00000000000000000000000004', 1, 2, ?1)",
        [format!(r#"{{"artifacts":[{{"evidence":[{{"extraction_id":"{cited}","source_id":1}}]}}]}}"#)]).unwrap();

    assert_eq!(
        artifact_bytes(journal(&mut hub)).unwrap(),
        MAX_AGENT_WORK_ARTIFACT_TOTAL_BYTES
    );
    hub.prune_agent_work(artifact.body.len(), AGENT_WORK_PRUNE_BATCH);
    let kept = clean_runs(&mut hub);
    assert_eq!(kept.len(), 127);
    assert!(kept.contains(&initial(owner, 2).key()));
    assert!(!kept.contains(&initial(owner, 3).key()));
    assert_eq!(
        artifact_bytes(journal(&mut hub)).unwrap(),
        MAX_AGENT_WORK_ARTIFACT_TOTAL_BYTES - MAX_AGENT_WORK_ARTIFACT_BYTES
    );
    compare_and_set_records(
        journal(&mut hub),
        Some(running),
        succeeded,
        None,
        Some(artifact),
    )
    .unwrap();
    let cited_kept: bool = journal(&mut hub)
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM agent_work_artifacts WHERE run_key = ?1)",
            [initial(owner, 2).key().as_slice()],
            |row| row.get(0),
        )
        .unwrap();
    assert!(cited_kept);
    drop(hub);
}
