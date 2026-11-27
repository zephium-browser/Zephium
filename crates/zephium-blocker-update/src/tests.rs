use std::fs;
use std::num::NonZeroU64;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use aws_lc_rs::rand::SystemRandom;
use jiff::Timestamp;
use tempfile::TempDir;
use tough::editor::signed::PathExists;
use tough::editor::RepositoryEditor;
use tough::key_source::{KeySource, LocalKeySource};
use tough::schema::{Role, Root, Signature, Signed};
use url::Url;
use zephium_blocker::Sha256;
use zephium_blocker::{
    CatalogPreparationOutcome, CatalogReplacementDispatch, StaticPolicyCatalog, WorkerBlocker,
};
use zephium_core::ports::blocker::{BlockerCompiler, BlockerShutdownOutcome};

use crate::manifest::{
    CatalogManifest, LicenseMetadata, ManifestError, ManifestSource, ManifestSourceFormat,
    CATALOG_MANIFEST_TARGET, CATALOG_MANIFEST_VERSION,
};
use crate::repository::{
    verify_candidate_repair_at, verify_repository_at, VerificationError, VerifiedCatalog,
    VerifiedCatalogPayload,
};
use crate::storage::{CatalogStore, FaultPoint, StoreError};
use crate::types::{
    ActivatedCatalog, CatalogAvailability, CatalogIdentity, GcBudget, LicensePolicy,
    RefreshAdmission, RepositoryConfig, ShutdownOutcome, UnavailableReason, UpdateLimits,
    UpdateStatus, WaitForStatus, HARD_MAX_CACHE_OBJECTS,
};
use crate::CatalogUpdateWorker;

const TEST_NOW: u64 = 2_000_000_000;
const VALID_TUF_EXPIRY: i64 = 4_000_000_000;
const EXPIRED_TUF_EXPIRY: i64 = 1_500_000_000;
const SOURCE_NAME: &str = "filters.txt";
const SOURCE_CONTENTS: &str = "||ads.example^\n";

#[derive(Clone, Copy, Debug)]
enum CachedObjectCorruption {
    Missing,
    Short,
    SameLength,
    Long,
}

struct RepositoryFixture {
    _directory: TempDir,
    trusted_root: Arc<[u8]>,
    metadata_url: Url,
    metadata_path: PathBuf,
    targets_url: Url,
    targets_path: PathBuf,
}

impl RepositoryFixture {
    fn config(&self, cache: &Path, limits: UpdateLimits, licenses: &[&str]) -> RepositoryConfig {
        RepositoryConfig::for_filesystem_tests(
            Arc::clone(&self.trusted_root),
            self.metadata_url.clone(),
            self.targets_url.clone(),
            cache.to_path_buf(),
            LicensePolicy::new(licenses.iter().copied()).unwrap(),
            limits,
        )
    }
}

#[tokio::test]
async fn signed_repository_yields_only_a_fully_verified_catalog() {
    let fixture = build_repository(1, SOURCE_CONTENTS, "MIT", VALID_TUF_EXPIRY).await;
    let cache = tempfile::tempdir().unwrap();
    let config = fixture.config(
        &cache.path().canonicalize().unwrap().join("cache"),
        UpdateLimits::default(),
        &["MIT"],
    );
    let mut store = CatalogStore::open(&config).unwrap();
    let datastore = store.prepare_tuf_datastore_at(TEST_NOW).unwrap();

    let verified = verify_repository_at(&config, &datastore, &store, TEST_NOW)
        .await
        .unwrap();

    assert_eq!(verified.identity().revision, 1);
    assert_eq!(
        verified.identity().source_bytes,
        SOURCE_CONTENTS.len() as u64
    );
}

#[tokio::test]
async fn unchanged_signed_targets_are_reused_from_the_verified_local_cas() {
    let fixture = build_repository(1, SOURCE_CONTENTS, "MIT", VALID_TUF_EXPIRY).await;
    let cache = tempfile::tempdir().unwrap();
    let config = fixture.config(
        &cache.path().canonicalize().unwrap().join("cache"),
        UpdateLimits::default(),
        &["MIT"],
    );
    let mut store = CatalogStore::open(&config).unwrap();
    let prepared = store.prepare_tuf_datastore_at(TEST_NOW).unwrap();
    let verified = verify_repository_at(&config, &prepared, &store, TEST_NOW)
        .await
        .unwrap();
    let expected = verified.identity().clone();
    stage_and_commit_verified(&mut store, verified).unwrap();

    // Metadata remains available and authenticated, but the origin's target
    // bodies are deliberately absent. Exact signed digests must resolve from
    // the private CAS without a second list-body transfer.
    for entry in fs::read_dir(&fixture.targets_path).unwrap() {
        let entry = entry.unwrap();
        if entry.file_type().unwrap().is_file() {
            fs::remove_file(entry.path()).unwrap();
        }
    }
    let prepared = store.prepare_tuf_datastore_at(TEST_NOW).unwrap();
    let reused = verify_repository_at(&config, &prepared, &store, TEST_NOW)
        .await
        .unwrap();
    assert_eq!(reused.identity(), &expected);
}

#[tokio::test]
async fn authenticated_refresh_repairs_current_manifest_and_source_corruption() {
    let fixture = build_repository(1, SOURCE_CONTENTS, "MIT", VALID_TUF_EXPIRY).await;
    let cache = tempfile::tempdir().unwrap();
    let config = fixture.config(
        &cache.path().canonicalize().unwrap().join("cache"),
        UpdateLimits::default(),
        &["MIT"],
    );
    let mut store = CatalogStore::open(&config).unwrap();
    let prepared = store.prepare_tuf_datastore_at(TEST_NOW).unwrap();
    let verified = verify_repository_at(&config, &prepared, &store, TEST_NOW)
        .await
        .unwrap();
    let identity = verified.identity().clone();
    stage_and_commit_verified(&mut store, verified).unwrap();

    let manifest_bytes = manifest(1, SOURCE_CONTENTS, "MIT")
        .encode_canonical()
        .unwrap();
    let manifest_path = config
        .storage_dir()
        .join("objects")
        .join(hex_sha256(&manifest_bytes));
    let source_path = config
        .storage_dir()
        .join("objects")
        .join(hex_sha256(SOURCE_CONTENTS.as_bytes()));
    for corruption in [
        CachedObjectCorruption::Missing,
        CachedObjectCorruption::Short,
        CachedObjectCorruption::SameLength,
        CachedObjectCorruption::Long,
    ] {
        corrupt_cached_object(&manifest_path, &manifest_bytes, corruption);
        corrupt_cached_object(&source_path, SOURCE_CONTENTS.as_bytes(), corruption);

        let prepared = store.prepare_tuf_datastore_at(TEST_NOW).unwrap();
        let repaired = verify_repository_at(&config, &prepared, &store, TEST_NOW)
            .await
            .unwrap();
        assert_eq!(repaired.identity(), &identity);
        assert!(matches!(
            &repaired.payload,
            VerifiedCatalogPayload::Unchanged { .. }
        ));
        stage_and_commit_verified(&mut store, repaired).unwrap();
        assert_eq!(fs::read(&manifest_path).unwrap(), manifest_bytes);
        assert_eq!(fs::read(&source_path).unwrap(), SOURCE_CONTENTS.as_bytes());
    }
}

#[tokio::test]
async fn authenticated_new_candidate_repairs_manifest_and_source_corruption() {
    for corruption in [
        CachedObjectCorruption::Missing,
        CachedObjectCorruption::Short,
        CachedObjectCorruption::SameLength,
        CachedObjectCorruption::Long,
    ] {
        let current = build_repository(1, "one\n", "MIT", VALID_TUF_EXPIRY).await;
        let next = build_repository(2, "two\n", "MIT", VALID_TUF_EXPIRY).await;
        let cache = tempfile::tempdir().unwrap();
        let config = current.config(
            &cache.path().canonicalize().unwrap().join("cache"),
            UpdateLimits::default(),
            &["MIT"],
        );
        let mut store = CatalogStore::open(&config).unwrap();
        let prepared = store.prepare_tuf_datastore_at(TEST_NOW).unwrap();
        let verified = verify_repository_at(&config, &prepared, &store, TEST_NOW)
            .await
            .unwrap();
        stage_and_commit_verified(&mut store, verified).unwrap();

        replace_directory_contents(&next.metadata_path, &current.metadata_path);
        replace_directory_contents(&next.targets_path, &current.targets_path);
        let manifest_bytes = manifest(2, "two\n", "MIT").encode_canonical().unwrap();
        let manifest_path = config
            .storage_dir()
            .join("objects")
            .join(hex_sha256(&manifest_bytes));
        let source_path = config
            .storage_dir()
            .join("objects")
            .join(hex_sha256(b"two\n"));
        corrupt_cached_object(&manifest_path, &manifest_bytes, corruption);
        corrupt_cached_object(&source_path, b"two\n", corruption);

        let prepared = store.prepare_tuf_datastore_at(TEST_NOW).unwrap();
        let candidate = verify_repository_at(&config, &prepared, &store, TEST_NOW)
            .await
            .unwrap();
        assert_eq!(candidate.identity().revision, 2);
        assert!(matches!(
            &candidate.payload,
            VerifiedCatalogPayload::Candidate { .. }
        ));
        assert!(matches!(
            store.stage_candidate(candidate).unwrap(),
            crate::storage::CandidateStageOutcome::Candidate(_)
        ));
        assert_eq!(fs::read(&manifest_path).unwrap(), manifest_bytes);
        assert_eq!(fs::read(&source_path).unwrap(), b"two\n");
    }
}

#[tokio::test]
async fn recovered_candidate_repairs_authenticated_cas_and_retries_same_deferred_catalog() {
    for corruption in [
        CachedObjectCorruption::Missing,
        CachedObjectCorruption::Short,
        CachedObjectCorruption::SameLength,
        CachedObjectCorruption::Long,
    ] {
        let fixture = build_repository(1, SOURCE_CONTENTS, "MIT", VALID_TUF_EXPIRY).await;
        let cache = tempfile::tempdir().unwrap();
        let config = fixture.config(
            &cache.path().canonicalize().unwrap().join("cache"),
            UpdateLimits::default(),
            &["MIT"],
        );
        let mut store = CatalogStore::open(&config).unwrap();
        let prepared = store.prepare_tuf_datastore_at(TEST_NOW).unwrap();
        let verified = verify_repository_at(&config, &prepared, &store, TEST_NOW)
            .await
            .unwrap();
        let identity = verified.identity().clone();
        assert!(matches!(
            store.stage_candidate(verified).unwrap(),
            crate::storage::CandidateStageOutcome::Candidate(_)
        ));
        drop(store);

        let source_path = config
            .storage_dir()
            .join("objects")
            .join(hex_sha256(SOURCE_CONTENTS.as_bytes()));
        corrupt_cached_object(&source_path, SOURCE_CONTENTS.as_bytes(), corruption);

        let mut recovered_store = CatalogStore::open(&config).unwrap();
        let recovered = recovered_store.take_pending_candidate().unwrap();
        assert_eq!(recovered.identity, identity);
        let compiler = WorkerBlocker::start(StaticPolicyCatalog::empty()).unwrap();
        assert_eq!(
            prepare_policy_catalog(&compiler, &recovered),
            CatalogPreparationOutcome::Failed(
                zephium_blocker::BlockerCompileFailure::SourceUnavailable
            )
        );

        let generation = recovered_store.generation();
        let prepared = recovered_store.prepare_tuf_datastore_at(TEST_NOW).unwrap();
        let repair =
            verify_candidate_repair_at(&config, &prepared, &recovered_store, &identity, TEST_NOW)
                .await
                .unwrap();
        assert!(matches!(
            &repair.payload,
            VerifiedCatalogPayload::CandidateRepair {
                identity: repaired,
                ..
            } if *repaired == identity
        ));
        recovered_store.repair_candidate(&identity, repair).unwrap();

        assert_eq!(recovered_store.generation(), generation + 1);
        assert_eq!(
            recovered_store.classify_identity(&identity),
            Ok(crate::storage::CatalogIdentityDisposition::Candidate)
        );
        assert!(recovered_store.current_identity().is_none());
        assert_eq!(fs::read(&source_path).unwrap(), SOURCE_CONTENTS.as_bytes());
        assert_eq!(
            prepare_policy_catalog(&compiler, &recovered),
            CatalogPreparationOutcome::Prepared
        );
        assert_eq!(
            compiler.shutdown_until(std::time::Instant::now() + std::time::Duration::from_secs(2)),
            BlockerShutdownOutcome::Clean
        );
    }
}

#[tokio::test]
async fn candidate_repair_crash_boundaries_preserve_candidate_and_high_water() {
    for fault in [
        FaultPoint::AfterObjects,
        FaultPoint::AfterJournal,
        FaultPoint::AfterStateReplace,
    ] {
        let fixture = build_repository(1, SOURCE_CONTENTS, "MIT", VALID_TUF_EXPIRY).await;
        let cache = tempfile::tempdir().unwrap();
        let config = fixture.config(
            &cache.path().canonicalize().unwrap().join("cache"),
            UpdateLimits::default(),
            &["MIT"],
        );
        let mut store = CatalogStore::open(&config).unwrap();
        let prepared = store.prepare_tuf_datastore_at(TEST_NOW).unwrap();
        let verified = verify_repository_at(&config, &prepared, &store, TEST_NOW)
            .await
            .unwrap();
        let identity = verified.identity().clone();
        assert!(matches!(
            store.stage_candidate(verified).unwrap(),
            crate::storage::CandidateStageOutcome::Candidate(_)
        ));
        let source_path = config
            .storage_dir()
            .join("objects")
            .join(hex_sha256(SOURCE_CONTENTS.as_bytes()));
        corrupt_cached_object(
            &source_path,
            SOURCE_CONTENTS.as_bytes(),
            CachedObjectCorruption::SameLength,
        );

        let prepared = store.prepare_tuf_datastore_at(TEST_NOW).unwrap();
        let repair = verify_candidate_repair_at(&config, &prepared, &store, &identity, TEST_NOW)
            .await
            .unwrap();
        assert!(matches!(
            store.repair_candidate_with_fault(&identity, repair, fault),
            Err(StoreError::InjectedFault)
        ));
        drop(store);

        let mut recovered = CatalogStore::open(&config).unwrap();
        assert!(recovered.current_identity().is_none());
        assert_eq!(
            recovered.classify_identity(&identity),
            Ok(crate::storage::CatalogIdentityDisposition::Candidate)
        );
        assert_eq!(
            recovered
                .take_pending_candidate()
                .map(|candidate| candidate.identity),
            Some(identity)
        );
        assert_eq!(fs::read(&source_path).unwrap(), SOURCE_CONTENTS.as_bytes());
    }
}

#[test]
fn strictly_newer_candidate_supersession_is_atomic_at_every_crash_boundary() {
    for fault in [
        FaultPoint::AfterObjects,
        FaultPoint::AfterJournal,
        FaultPoint::AfterStateReplace,
    ] {
        let directory = tempfile::tempdir().unwrap();
        let config = storage_config(&directory);
        let mut store = CatalogStore::open(&config).unwrap();
        activate_test_catalog(&mut store, verified_catalog(1, "previous\n")).unwrap();
        activate_test_catalog(&mut store, verified_catalog(2, "current\n")).unwrap();
        let _ = store.take_pending_activation();

        let mut old = verified_catalog(3, "old-candidate\n");
        prepare_test_verified(&mut store, &mut old);
        let old_identity = old.identity().clone();
        assert!(matches!(
            store.stage_candidate(old).unwrap(),
            crate::storage::CandidateStageOutcome::Candidate(_)
        ));

        let mut newer = verified_catalog(4, "new-candidate\n");
        prepare_test_verified(&mut store, &mut newer);
        let newer_identity = newer.identity().clone();
        assert!(matches!(
            store.repair_candidate_with_fault(&old_identity, newer, fault),
            Err(StoreError::InjectedFault)
        ));
        drop(store);

        let mut recovered = CatalogStore::open(&config).unwrap();
        assert_eq!(recovered.current_identity().unwrap().revision, 2);
        assert_eq!(
            recovered
                .load_previous()
                .unwrap()
                .unwrap()
                .identity
                .revision,
            1
        );
        let expected = if fault == FaultPoint::AfterObjects {
            old_identity.clone()
        } else {
            newer_identity.clone()
        };
        assert_eq!(
            recovered
                .take_pending_candidate()
                .map(|candidate| candidate.identity),
            Some(expected.clone())
        );
        assert_eq!(
            recovered.classify_identity(&expected),
            Ok(crate::storage::CatalogIdentityDisposition::Candidate)
        );
    }
}

#[test]
fn candidate_supersession_rejects_lower_and_equal_revision_equivocation() {
    let directory = tempfile::tempdir().unwrap();
    let config = storage_config(&directory);
    let mut store = CatalogStore::open(&config).unwrap();
    activate_test_catalog(&mut store, verified_catalog(1, "current\n")).unwrap();
    let _ = store.take_pending_activation();

    let mut old = verified_catalog(2, "candidate\n");
    prepare_test_verified(&mut store, &mut old);
    let old_identity = old.identity().clone();
    assert!(matches!(
        store.stage_candidate(old).unwrap(),
        crate::storage::CandidateStageOutcome::Candidate(_)
    ));
    let generation = store.generation();

    let mut lower = verified_catalog(1, "lower\n");
    lower.repository_identity = store.repository_identity();
    assert!(matches!(
        store.repair_candidate(&old_identity, lower),
        Err(StoreError::Rollback)
    ));

    let mut equivocation = verified_catalog(2, "different-candidate\n");
    equivocation.repository_identity = store.repository_identity();
    assert!(matches!(
        store.repair_candidate(&old_identity, equivocation),
        Err(StoreError::Equivocation)
    ));

    assert_eq!(store.generation(), generation);
    assert_eq!(
        store.classify_identity(&old_identity),
        Ok(crate::storage::CatalogIdentityDisposition::Candidate)
    );
}

#[cfg(unix)]
#[tokio::test]
async fn authenticated_repair_never_replaces_an_unsafe_cache_identity() {
    use std::os::unix::fs::symlink;

    let fixture = build_repository(1, SOURCE_CONTENTS, "MIT", VALID_TUF_EXPIRY).await;
    let cache = tempfile::tempdir().unwrap();
    let config = fixture.config(
        &cache.path().canonicalize().unwrap().join("cache"),
        UpdateLimits::default(),
        &["MIT"],
    );
    let mut store = CatalogStore::open(&config).unwrap();
    let manifest_bytes = manifest(1, SOURCE_CONTENTS, "MIT")
        .encode_canonical()
        .unwrap();
    let manifest_path = config
        .storage_dir()
        .join("objects")
        .join(hex_sha256(&manifest_bytes));
    let outside = cache.path().join("outside");
    write_private_test_file(&outside, b"outside");
    symlink(&outside, &manifest_path).unwrap();

    let prepared = store.prepare_tuf_datastore_at(TEST_NOW).unwrap();
    assert!(matches!(
        verify_repository_at(&config, &prepared, &store, TEST_NOW).await,
        Err(VerificationError::Storage)
    ));
    assert!(manifest_path.is_symlink());
    assert_eq!(fs::read(outside).unwrap(), b"outside");
}

#[tokio::test]
async fn worker_delivers_each_candidate_once_and_commits_only_after_authorization() {
    let current_time = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let fixture =
        build_repository_at(1, SOURCE_CONTENTS, "MIT", VALID_TUF_EXPIRY, current_time).await;
    let cache = tempfile::tempdir().unwrap();
    let config = fixture.config(
        &cache.path().canonicalize().unwrap().join("cache"),
        UpdateLimits::default(),
        &["MIT"],
    );
    let worker = CatalogUpdateWorker::start(config.clone());
    let operation = match worker.request_refresh() {
        RefreshAdmission::Accepted(operation) => operation,
        other => panic!("refresh was not accepted: {other:?}"),
    };
    let (mut observed, current, mut candidate) = worker.observe_and_take_catalogs();
    assert!(current.is_none());
    assert!(matches!(
        observed.status,
        UpdateStatus::Refreshing {
            operation: observed,
            ..
        } if observed == operation
    ));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let candidate = loop {
        if let Some(candidate) = candidate.take() {
            break candidate;
        }
        assert!(
            !matches!(
                observed.status,
                UpdateStatus::Failed {
                    operation: failed,
                    ..
                } if failed == operation
            ),
            "refresh failed before publishing its candidate: {observed:?}"
        );
        assert!(
            !matches!(observed.status, UpdateStatus::Shutdown),
            "worker shut down before publishing its candidate"
        );
        let remaining = deadline
            .checked_duration_since(std::time::Instant::now())
            .expect("worker completion deadline elapsed");
        match worker.wait_for_change(observed.revision, remaining) {
            WaitForStatus::Changed(snapshot) => {
                let (next, current, next_candidate) = worker.observe_and_take_catalogs();
                assert!(current.is_none());
                assert!(next.revision >= snapshot.revision);
                observed = next;
                candidate = next_candidate;
            }
            WaitForStatus::TimedOut => panic!("worker completion timed out"),
        }
    };
    assert_eq!(candidate.identity.revision, 1);
    let (_, current, duplicate) = worker.observe_and_take_catalogs();
    assert!(current.is_none());
    assert!(duplicate.is_none());

    let identity = candidate.identity;
    let (done_tx, done_rx) = std::sync::mpsc::sync_channel(1);
    assert_eq!(
        worker.commit_candidate(
            identity.clone(),
            Box::new(move |outcome| done_tx.send(outcome).unwrap()),
        ),
        crate::CandidateCommitDispatch::Scheduled
    );
    assert_eq!(
        done_rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .unwrap(),
        crate::CandidateCommitOutcome::Committed
    );
    let ready = worker.status();
    assert!(matches!(
        ready,
        crate::StatusSnapshot {
            last_refresh_attempt_unix: Some(_),
            status: UpdateStatus::Ready(CatalogAvailability::Fresh(ref current)),
            ..
        } if *current == identity
    ));
    assert_eq!(
        worker.shutdown(std::time::Duration::from_secs(2)),
        ShutdownOutcome::Complete
    );
}

#[tokio::test]
async fn worker_refresh_repairs_exact_current_material_after_restart() {
    let current_time = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let fixture =
        build_repository_at(1, SOURCE_CONTENTS, "MIT", VALID_TUF_EXPIRY, current_time).await;
    let manifest_bytes = manifest_at(1, SOURCE_CONTENTS, "MIT", current_time)
        .encode_canonical()
        .unwrap();

    for (expected_material, material_digest) in [
        (
            manifest_bytes.as_slice(),
            hex_sha256(manifest_bytes.as_slice()),
        ),
        (
            SOURCE_CONTENTS.as_bytes(),
            hex_sha256(SOURCE_CONTENTS.as_bytes()),
        ),
    ] {
        for corruption in [
            CachedObjectCorruption::Missing,
            CachedObjectCorruption::Short,
            CachedObjectCorruption::SameLength,
            CachedObjectCorruption::Long,
        ] {
            let cache = tempfile::tempdir().unwrap();
            let config = fixture.config(
                &cache.path().canonicalize().unwrap().join("cache"),
                UpdateLimits::default(),
                &["MIT"],
            );
            let staging = CatalogUpdateWorker::start(config.clone());
            let operation = match staging.request_refresh() {
                RefreshAdmission::Accepted(operation) => operation,
                other => panic!("initial refresh was not accepted: {other:?}"),
            };
            let candidate = wait_for_candidate(&staging, operation);
            let identity = candidate.identity.clone();
            let (commit_tx, commit_rx) = std::sync::mpsc::sync_channel(1);
            assert_eq!(
                staging.commit_candidate(
                    identity.clone(),
                    Box::new(move |outcome| commit_tx.send(outcome).unwrap()),
                ),
                crate::CandidateCommitDispatch::Scheduled
            );
            assert_eq!(
                commit_rx
                    .recv_timeout(std::time::Duration::from_secs(10))
                    .unwrap(),
                crate::CandidateCommitOutcome::Committed
            );
            assert_eq!(
                staging.shutdown(std::time::Duration::from_secs(2)),
                ShutdownOutcome::Complete
            );

            let material_path = config.storage_dir().join("objects").join(&material_digest);
            corrupt_cached_object(&material_path, expected_material, corruption);
            let worker = CatalogUpdateWorker::start(config);
            let (_, current, candidate) = worker.observe_and_take_catalogs();
            assert!(candidate.is_none());
            let current = current.expect("durable current catalog must recover lazily");
            assert_eq!(current.identity, identity);
            let compiler = WorkerBlocker::start(StaticPolicyCatalog::empty()).unwrap();
            assert_eq!(
                prepare_policy_catalog(&compiler, &current),
                CatalogPreparationOutcome::Failed(
                    zephium_blocker::BlockerCompileFailure::SourceUnavailable
                )
            );

            let repair_operation = match worker.request_refresh() {
                RefreshAdmission::Accepted(operation) => operation,
                other => panic!("current repair refresh was not accepted: {other:?}"),
            };
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            loop {
                let snapshot = worker.status();
                match snapshot.status {
                    UpdateStatus::Ready(ref availability)
                        if identity_for_test(availability) == &identity =>
                    {
                        break;
                    }
                    UpdateStatus::Failed {
                        operation, failure, ..
                    } if operation == repair_operation => {
                        panic!("current repair failed: {failure:?}");
                    }
                    UpdateStatus::Shutdown => panic!("worker shut down during current repair"),
                    _ => {}
                }
                let remaining = deadline
                    .checked_duration_since(std::time::Instant::now())
                    .expect("current repair deadline elapsed");
                assert!(matches!(
                    worker.wait_for_change(snapshot.revision, remaining),
                    WaitForStatus::Changed(_)
                ));
            }

            assert_eq!(fs::read(&material_path).unwrap(), expected_material);
            assert_eq!(
                prepare_policy_catalog(&compiler, &current),
                CatalogPreparationOutcome::Prepared
            );
            assert_eq!(
                compiler
                    .shutdown_until(std::time::Instant::now() + std::time::Duration::from_secs(2)),
                BlockerShutdownOutcome::Clean
            );
            assert_eq!(
                worker.shutdown(std::time::Duration::from_secs(2)),
                ShutdownOutcome::Complete
            );
        }
    }
}

#[tokio::test]
async fn automatic_candidate_repair_preserves_originating_refresh_operation() {
    let current_time = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let fixture =
        build_repository_at(1, SOURCE_CONTENTS, "MIT", VALID_TUF_EXPIRY, current_time).await;
    let cache = tempfile::tempdir().unwrap();
    let config = fixture.config(
        &cache.path().canonicalize().unwrap().join("cache"),
        UpdateLimits::default(),
        &["MIT"],
    );
    let worker = CatalogUpdateWorker::start(config.clone());
    let refresh_operation = match worker.request_refresh() {
        RefreshAdmission::Accepted(operation) => operation,
        other => panic!("refresh was not accepted: {other:?}"),
    };
    let candidate = wait_for_candidate(&worker, refresh_operation);
    let identity = candidate.identity.clone();
    assert_eq!(
        worker.retry_candidate(identity.clone(), Box::new(|_| {})),
        crate::CandidateRepairDispatch::Rejected
    );
    let compiler = WorkerBlocker::start(StaticPolicyCatalog::empty()).unwrap();

    let (repair_tx, repair_rx) = std::sync::mpsc::sync_channel(1);
    assert_eq!(
        worker.repair_candidate(
            identity.clone(),
            Box::new(move |outcome| repair_tx.send(outcome).unwrap()),
        ),
        crate::CandidateRepairDispatch::Scheduled {
            operation: refresh_operation
        }
    );
    assert!(matches!(
        repair_rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .unwrap(),
        crate::CandidateRepairOutcome::Repaired
    ));
    assert!(matches!(
        worker.status().status,
        UpdateStatus::Refreshing { operation, .. } if operation == refresh_operation
    ));
    assert_eq!(
        prepare_policy_catalog(&compiler, &candidate),
        CatalogPreparationOutcome::Prepared
    );

    let (commit_tx, commit_rx) = std::sync::mpsc::sync_channel(1);
    assert_eq!(
        worker.commit_candidate(
            identity.clone(),
            Box::new(move |outcome| commit_tx.send(outcome).unwrap()),
        ),
        crate::CandidateCommitDispatch::Scheduled
    );
    assert_eq!(
        commit_rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .unwrap(),
        crate::CandidateCommitOutcome::Committed
    );
    assert!(matches!(
        worker.status().status,
        UpdateStatus::Ready(CatalogAvailability::Fresh(ref current))
            if *current == identity
    ));
    assert_eq!(
        compiler.shutdown_until(std::time::Instant::now() + std::time::Duration::from_secs(2)),
        BlockerShutdownOutcome::Clean
    );
    assert_eq!(
        worker.shutdown(std::time::Duration::from_secs(2)),
        ShutdownOutcome::Complete
    );
}

#[tokio::test]
async fn worker_repairs_one_exact_candidate_once_and_compiler_retries_in_process() {
    let current_time = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let fixture =
        build_repository_at(1, SOURCE_CONTENTS, "MIT", VALID_TUF_EXPIRY, current_time).await;
    let cache = tempfile::tempdir().unwrap();
    let config = fixture.config(
        &cache.path().canonicalize().unwrap().join("cache"),
        UpdateLimits::default(),
        &["MIT"],
    );
    let staging_worker = CatalogUpdateWorker::start(config.clone());
    let operation = match staging_worker.request_refresh() {
        RefreshAdmission::Accepted(operation) => operation,
        other => panic!("refresh was not accepted: {other:?}"),
    };
    let staged = wait_for_candidate(&staging_worker, operation);
    let identity = staged.identity;
    assert_eq!(
        staging_worker.shutdown(std::time::Duration::from_secs(2)),
        ShutdownOutcome::Complete
    );
    let source_path = config
        .storage_dir()
        .join("objects")
        .join(hex_sha256(SOURCE_CONTENTS.as_bytes()));
    corrupt_cached_object(
        &source_path,
        SOURCE_CONTENTS.as_bytes(),
        CachedObjectCorruption::SameLength,
    );
    let worker = CatalogUpdateWorker::start(config);
    let (_, current, candidate) = worker.observe_and_take_catalogs();
    assert!(current.is_none());
    let candidate = candidate.expect("durable candidate must recover");
    assert_eq!(candidate.identity, identity);

    let compiler = WorkerBlocker::start(StaticPolicyCatalog::empty()).unwrap();
    assert_eq!(
        prepare_policy_catalog(&compiler, &candidate),
        CatalogPreparationOutcome::Failed(
            zephium_blocker::BlockerCompileFailure::SourceUnavailable
        )
    );

    let (repair_tx, repair_rx) = std::sync::mpsc::sync_channel(1);
    assert!(matches!(
        worker.repair_candidate(
            identity.clone(),
            Box::new(move |outcome| repair_tx.send(outcome).unwrap()),
        ),
        crate::CandidateRepairDispatch::Scheduled { .. }
    ));
    assert!(matches!(
        repair_rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .unwrap(),
        crate::CandidateRepairOutcome::Repaired
    ));
    assert_eq!(fs::read(&source_path).unwrap(), SOURCE_CONTENTS.as_bytes());
    assert_eq!(
        prepare_policy_catalog(&compiler, &candidate),
        CatalogPreparationOutcome::Prepared
    );

    assert_eq!(
        worker.repair_candidate(identity.clone(), Box::new(|_| {})),
        crate::CandidateRepairDispatch::Rejected
    );
    let (commit_tx, commit_rx) = std::sync::mpsc::sync_channel(1);
    assert_eq!(
        worker.commit_candidate(
            identity,
            Box::new(move |outcome| commit_tx.send(outcome).unwrap()),
        ),
        crate::CandidateCommitDispatch::Scheduled
    );
    assert_eq!(
        commit_rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .unwrap(),
        crate::CandidateCommitOutcome::Committed
    );
    assert_eq!(
        compiler.shutdown_until(std::time::Instant::now() + std::time::Duration::from_secs(2)),
        BlockerShutdownOutcome::Clean
    );
    assert_eq!(
        worker.shutdown(std::time::Duration::from_secs(2)),
        ShutdownOutcome::Complete
    );
}

#[tokio::test]
async fn recovered_corrupt_candidate_is_superseded_by_authenticated_newer_revision() {
    let current_time = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let old =
        build_repository_at(1, "old-candidate\n", "MIT", VALID_TUF_EXPIRY, current_time).await;
    let newer =
        build_repository_at(2, "new-candidate\n", "MIT", VALID_TUF_EXPIRY, current_time).await;
    let cache = tempfile::tempdir().unwrap();
    let config = old.config(
        &cache.path().canonicalize().unwrap().join("cache"),
        UpdateLimits::default(),
        &["MIT"],
    );
    let staging_worker = CatalogUpdateWorker::start(config.clone());
    let operation = match staging_worker.request_refresh() {
        RefreshAdmission::Accepted(operation) => operation,
        other => panic!("refresh was not accepted: {other:?}"),
    };
    let old_identity = wait_for_candidate(&staging_worker, operation).identity;
    assert_eq!(
        staging_worker.shutdown(std::time::Duration::from_secs(2)),
        ShutdownOutcome::Complete
    );

    let old_source_path = config
        .storage_dir()
        .join("objects")
        .join(hex_sha256(b"old-candidate\n"));
    corrupt_cached_object(
        &old_source_path,
        b"old-candidate\n",
        CachedObjectCorruption::SameLength,
    );
    let worker = CatalogUpdateWorker::start(config.clone());
    let (_, current, recovered) = worker.observe_and_take_catalogs();
    assert!(current.is_none());
    let recovered = recovered.expect("durable candidate must recover");
    assert_eq!(recovered.identity, old_identity);
    let compiler = WorkerBlocker::start(StaticPolicyCatalog::empty()).unwrap();
    assert_eq!(
        prepare_policy_catalog(&compiler, &recovered),
        CatalogPreparationOutcome::Failed(
            zephium_blocker::BlockerCompileFailure::SourceUnavailable
        )
    );

    for entry in fs::read_dir(&old.targets_path).unwrap() {
        let entry = entry.unwrap();
        if entry.file_type().unwrap().is_file() {
            fs::remove_file(entry.path()).unwrap();
        }
    }
    let mut failed_operation = None;
    for explicit in [false, true] {
        let (done_tx, done_rx) = std::sync::mpsc::sync_channel(1);
        let dispatch = if explicit {
            worker.retry_candidate(
                old_identity.clone(),
                Box::new(move |outcome| done_tx.send(outcome).unwrap()),
            )
        } else {
            worker.repair_candidate(
                old_identity.clone(),
                Box::new(move |outcome| done_tx.send(outcome).unwrap()),
            )
        };
        let operation = match dispatch {
            crate::CandidateRepairDispatch::Scheduled { operation } => operation,
            other => panic!("candidate repair was not admitted: {other:?}"),
        };
        if let Some(previous) = failed_operation {
            assert!(operation > previous);
        }
        failed_operation = Some(operation);
        assert!(matches!(
            done_rx
                .recv_timeout(std::time::Duration::from_secs(10))
                .unwrap(),
            crate::CandidateRepairOutcome::Failed(crate::FailureKind::Target)
        ));
    }
    replace_directory_contents(&newer.metadata_path, &old.metadata_path);
    replace_directory_contents(&newer.targets_path, &old.targets_path);

    let (repair_tx, repair_rx) = std::sync::mpsc::sync_channel(1);
    let repair_operation = match worker.retry_candidate(
        old_identity.clone(),
        Box::new(move |outcome| repair_tx.send(outcome).unwrap()),
    ) {
        crate::CandidateRepairDispatch::Scheduled { operation } => operation,
        other => panic!("candidate supersession was not admitted: {other:?}"),
    };
    assert!(repair_operation > failed_operation.unwrap());
    let superseding = match repair_rx
        .recv_timeout(std::time::Duration::from_secs(10))
        .unwrap()
    {
        crate::CandidateRepairOutcome::Superseded(candidate) => candidate,
        other => panic!("expected authenticated supersession, got {other:?}"),
    };
    assert_eq!(superseding.identity.revision, 2);
    assert!(matches!(
        worker.status().status,
        UpdateStatus::Refreshing { operation, .. } if operation == repair_operation
    ));
    let (_, current, published) = worker.observe_and_take_catalogs();
    assert!(current.is_none());
    assert_eq!(
        published
            .as_ref()
            .map(|candidate| candidate.identity.clone()),
        Some(superseding.identity.clone())
    );
    let (new_repair_tx, new_repair_rx) = std::sync::mpsc::sync_channel(1);
    assert_eq!(
        worker.repair_candidate(
            superseding.identity.clone(),
            Box::new(move |outcome| new_repair_tx.send(outcome).unwrap()),
        ),
        crate::CandidateRepairDispatch::Scheduled {
            operation: repair_operation
        }
    );
    assert!(matches!(
        new_repair_rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .unwrap(),
        crate::CandidateRepairOutcome::Repaired
    ));
    assert_eq!(
        prepare_policy_catalog(&compiler, &superseding),
        CatalogPreparationOutcome::Prepared
    );

    let identity = superseding.identity;
    let (commit_tx, commit_rx) = std::sync::mpsc::sync_channel(1);
    assert_eq!(
        worker.commit_candidate(
            identity.clone(),
            Box::new(move |outcome| commit_tx.send(outcome).unwrap()),
        ),
        crate::CandidateCommitDispatch::Scheduled
    );
    assert_eq!(
        commit_rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .unwrap(),
        crate::CandidateCommitOutcome::Committed
    );
    assert!(matches!(
        worker.status().status,
        UpdateStatus::Ready(CatalogAvailability::Fresh(ref current))
            if *current == identity
    ));
    assert_eq!(
        compiler.shutdown_until(std::time::Instant::now() + std::time::Duration::from_secs(2)),
        BlockerShutdownOutcome::Clean
    );
    assert_eq!(
        worker.shutdown(std::time::Duration::from_secs(2)),
        ShutdownOutcome::Complete
    );
}

#[tokio::test]
async fn failed_worker_repair_retains_exact_candidate_and_enforces_process_budget() {
    let current_time = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let fixture =
        build_repository_at(1, SOURCE_CONTENTS, "MIT", VALID_TUF_EXPIRY, current_time).await;
    let cache = tempfile::tempdir().unwrap();
    let config = fixture.config(
        &cache.path().canonicalize().unwrap().join("cache"),
        UpdateLimits::default(),
        &["MIT"],
    );
    let staging_worker = CatalogUpdateWorker::start(config.clone());
    let operation = match staging_worker.request_refresh() {
        RefreshAdmission::Accepted(operation) => operation,
        other => panic!("refresh was not accepted: {other:?}"),
    };
    let identity = wait_for_candidate(&staging_worker, operation).identity;
    assert_eq!(
        staging_worker.shutdown(std::time::Duration::from_secs(2)),
        ShutdownOutcome::Complete
    );

    let source_path = config
        .storage_dir()
        .join("objects")
        .join(hex_sha256(SOURCE_CONTENTS.as_bytes()));
    corrupt_cached_object(
        &source_path,
        SOURCE_CONTENTS.as_bytes(),
        CachedObjectCorruption::Missing,
    );
    for entry in fs::read_dir(&fixture.targets_path).unwrap() {
        let entry = entry.unwrap();
        if entry.file_type().unwrap().is_file() {
            fs::remove_file(entry.path()).unwrap();
        }
    }

    let worker = CatalogUpdateWorker::start(config.clone());
    let (_, current, candidate) = worker.observe_and_take_catalogs();
    assert!(current.is_none());
    assert_eq!(
        candidate.as_ref().map(|candidate| &candidate.identity),
        Some(&identity)
    );
    let (done_tx, done_rx) = std::sync::mpsc::sync_channel(1);
    let mut last_operation = match worker.repair_candidate(
        identity.clone(),
        Box::new(move |outcome| done_tx.send(outcome).unwrap()),
    ) {
        crate::CandidateRepairDispatch::Scheduled { operation } => operation,
        other => panic!("first repair was not admitted: {other:?}"),
    };
    assert!(matches!(
        done_rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .unwrap(),
        crate::CandidateRepairOutcome::Failed(crate::FailureKind::Target)
    ));
    assert!(matches!(
        worker.status().status,
        UpdateStatus::Failed { operation, .. } if operation == last_operation
    ));
    for _ in 0..2 {
        let (done_tx, done_rx) = std::sync::mpsc::sync_channel(1);
        let operation = match worker.retry_candidate(
            identity.clone(),
            Box::new(move |outcome| done_tx.send(outcome).unwrap()),
        ) {
            crate::CandidateRepairDispatch::Scheduled { operation } => operation,
            other => panic!("explicit repair retry was not admitted: {other:?}"),
        };
        assert!(operation > last_operation);
        last_operation = operation;
        assert!(matches!(
            done_rx
                .recv_timeout(std::time::Duration::from_secs(10))
                .unwrap(),
            crate::CandidateRepairOutcome::Failed(crate::FailureKind::Target)
        ));
        assert!(matches!(
            worker.status().status,
            UpdateStatus::Failed { operation, .. } if operation == last_operation
        ));
    }
    assert_eq!(
        worker.retry_candidate(identity.clone(), Box::new(|_| {})),
        crate::CandidateRepairDispatch::LimitReached
    );
    assert!(matches!(
        worker.status().status,
        UpdateStatus::Failed {
            failure: crate::FailureKind::Target,
            ..
        }
    ));
    assert_eq!(
        worker.shutdown(std::time::Duration::from_secs(2)),
        ShutdownOutcome::Complete
    );

    let recovered = CatalogUpdateWorker::start(config);
    let (_, current, candidate) = recovered.observe_and_take_catalogs();
    assert!(current.is_none());
    assert_eq!(
        candidate.map(|candidate| candidate.identity),
        Some(identity.clone())
    );
    let (done_tx, done_rx) = std::sync::mpsc::sync_channel(1);
    assert!(matches!(
        recovered.repair_candidate(
            identity,
            Box::new(move |outcome| done_tx.send(outcome).unwrap()),
        ),
        crate::CandidateRepairDispatch::Scheduled { .. }
    ));
    assert!(matches!(
        done_rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .unwrap(),
        crate::CandidateRepairOutcome::Failed(crate::FailureKind::Target)
    ));
    assert_eq!(
        recovered.shutdown(std::time::Duration::from_secs(2)),
        ShutdownOutcome::Complete
    );
}

#[tokio::test]
async fn nonretryable_worker_repair_failure_consumes_explicit_retry_arm() {
    let current_time = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let fixture =
        build_repository_at(1, SOURCE_CONTENTS, "MIT", VALID_TUF_EXPIRY, current_time).await;
    let target_backup =
        build_repository_at(1, SOURCE_CONTENTS, "MIT", VALID_TUF_EXPIRY, current_time).await;
    let cache = tempfile::tempdir().unwrap();
    let config = fixture.config(
        &cache.path().canonicalize().unwrap().join("cache"),
        UpdateLimits::default(),
        &["MIT"],
    );
    let worker = CatalogUpdateWorker::start(config.clone());
    let refresh_operation = match worker.request_refresh() {
        RefreshAdmission::Accepted(operation) => operation,
        other => panic!("refresh was not accepted: {other:?}"),
    };
    let identity = wait_for_candidate(&worker, refresh_operation).identity;

    for entry in fs::read_dir(&fixture.targets_path).unwrap() {
        let entry = entry.unwrap();
        if entry.file_type().unwrap().is_file() {
            fs::remove_file(entry.path()).unwrap();
        }
    }
    let (first_tx, first_rx) = std::sync::mpsc::sync_channel(1);
    assert!(matches!(
        worker.repair_candidate(
            identity.clone(),
            Box::new(move |outcome| first_tx.send(outcome).unwrap()),
        ),
        crate::CandidateRepairDispatch::Scheduled { .. }
    ));
    assert!(matches!(
        first_rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .unwrap(),
        crate::CandidateRepairOutcome::Failed(crate::FailureKind::Target)
    ));

    replace_directory_contents(&target_backup.targets_path, &fixture.targets_path);
    let source_path = config
        .storage_dir()
        .join("objects")
        .join(hex_sha256(SOURCE_CONTENTS.as_bytes()));
    fs::remove_file(&source_path).unwrap();
    fs::create_dir(&source_path).unwrap();
    let (retry_tx, retry_rx) = std::sync::mpsc::sync_channel(1);
    assert!(matches!(
        worker.retry_candidate(
            identity.clone(),
            Box::new(move |outcome| retry_tx.send(outcome).unwrap()),
        ),
        crate::CandidateRepairDispatch::Scheduled { .. }
    ));
    assert!(matches!(
        retry_rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .unwrap(),
        crate::CandidateRepairOutcome::Failed(crate::FailureKind::Storage)
    ));
    assert_eq!(
        worker.retry_candidate(identity, Box::new(|_| {})),
        crate::CandidateRepairDispatch::Rejected
    );
    assert_eq!(
        worker.shutdown(std::time::Duration::from_secs(2)),
        ShutdownOutcome::Complete
    );
}

#[tokio::test]
async fn safe_expiration_enforcement_rejects_expired_tuf_metadata() {
    let fixture = build_repository(1, SOURCE_CONTENTS, "MIT", EXPIRED_TUF_EXPIRY).await;
    let cache = tempfile::tempdir().unwrap();
    let config = fixture.config(
        &cache.path().canonicalize().unwrap().join("cache"),
        UpdateLimits::default(),
        &["MIT"],
    );
    let mut store = CatalogStore::open(&config).unwrap();
    let datastore = store.prepare_tuf_datastore_at(TEST_NOW).unwrap();

    assert_eq!(
        verify_repository_at(&config, &datastore, &store, TEST_NOW)
            .await
            .unwrap_err(),
        VerificationError::Metadata
    );
}

#[tokio::test]
async fn corrupt_signed_target_is_never_promoted_to_utf8_or_catalog_state() {
    let fixture = build_repository(1, SOURCE_CONTENTS, "MIT", VALID_TUF_EXPIRY).await;
    let target = fs::read_dir(&fixture.targets_path)
        .unwrap()
        .map(Result::unwrap)
        .find(|entry| entry.file_name().to_string_lossy().ends_with(SOURCE_NAME))
        .unwrap()
        .path();
    fs::write(target, b"||evil.example^\n").unwrap();
    let cache = tempfile::tempdir().unwrap();
    let config = fixture.config(
        &cache.path().canonicalize().unwrap().join("cache"),
        UpdateLimits::default(),
        &["MIT"],
    );
    let mut store = CatalogStore::open(&config).unwrap();
    let datastore = store.prepare_tuf_datastore_at(TEST_NOW).unwrap();

    assert_eq!(
        verify_repository_at(&config, &datastore, &store, TEST_NOW)
            .await
            .unwrap_err(),
        VerificationError::Target
    );
}

#[tokio::test]
async fn failed_verification_discards_staged_metadata_and_persists_attempt_hint() {
    let current_time = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let fixture =
        build_repository_at(1, SOURCE_CONTENTS, "MIT", VALID_TUF_EXPIRY, current_time).await;
    let target = fs::read_dir(&fixture.targets_path)
        .unwrap()
        .map(Result::unwrap)
        .find(|entry| entry.file_name().to_string_lossy().ends_with(SOURCE_NAME))
        .unwrap()
        .path();
    fs::write(target, b"corrupt-after-signing\n").unwrap();
    let cache = tempfile::tempdir().unwrap();
    let config = fixture.config(
        &cache.path().canonicalize().unwrap().join("cache"),
        UpdateLimits::default(),
        &["MIT"],
    );
    let worker = CatalogUpdateWorker::start(config.clone());
    let operation = match worker.request_refresh() {
        RefreshAdmission::Accepted(operation) => operation,
        other => panic!("refresh was not accepted: {other:?}"),
    };
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let mut revision = worker.status().revision;
    let failed = loop {
        let remaining = deadline
            .checked_duration_since(std::time::Instant::now())
            .expect("failed refresh deadline elapsed");
        match worker.wait_for_change(revision, remaining) {
            WaitForStatus::Changed(snapshot)
                if matches!(
                    snapshot.status,
                    UpdateStatus::Failed {
                        operation: observed,
                        ..
                    } if observed == operation
                ) =>
            {
                break snapshot;
            }
            WaitForStatus::Changed(snapshot) => revision = snapshot.revision,
            WaitForStatus::TimedOut => panic!("failed refresh timed out"),
        }
    };
    let attempted = failed
        .last_refresh_attempt_unix
        .expect("network admission must publish its durable hint");
    let (_, current, candidate) = worker.observe_and_take_catalogs();
    assert!(current.is_none());
    assert!(candidate.is_none());
    assert_eq!(
        fs::read_dir(config.storage_dir().join("tuf-stage"))
            .unwrap()
            .count(),
        0
    );
    assert_eq!(
        worker.shutdown(std::time::Duration::from_secs(2)),
        ShutdownOutcome::Complete
    );

    let store = CatalogStore::open(&config).unwrap();
    assert!(store.current_identity().is_none());
    assert_eq!(store.last_refresh_attempt_unix(), Some(attempted));
}

#[tokio::test]
async fn staged_tuf_metadata_cannot_change_between_verification_and_activation() {
    let fixture = build_repository(1, SOURCE_CONTENTS, "MIT", VALID_TUF_EXPIRY).await;
    let cache = tempfile::tempdir().unwrap();
    let config = fixture.config(
        &cache.path().canonicalize().unwrap().join("cache"),
        UpdateLimits::default(),
        &["MIT"],
    );
    let mut store = CatalogStore::open(&config).unwrap();
    let prepared = store.prepare_tuf_datastore_at(TEST_NOW).unwrap();
    let verified = verify_repository_at(&config, &prepared, &store, TEST_NOW)
        .await
        .unwrap();
    fs::write(prepared.path.join("root.json"), b"post-verification swap").unwrap();

    assert!(matches!(
        store.stage_candidate(verified),
        Err(StoreError::StagedMetadataChanged)
    ));
    assert!(store.current_identity().is_none());
}

#[tokio::test]
async fn manifest_source_size_is_enforced_before_source_body_admission() {
    let fixture = build_repository(1, SOURCE_CONTENTS, "MIT", VALID_TUF_EXPIRY).await;
    let cache = tempfile::tempdir().unwrap();
    let limits = UpdateLimits {
        max_source_bytes: 8,
        max_total_source_bytes: 8,
        ..UpdateLimits::default()
    };
    let config = fixture.config(
        &cache.path().canonicalize().unwrap().join("cache"),
        limits,
        &["MIT"],
    );
    let mut store = CatalogStore::open(&config).unwrap();
    let datastore = store.prepare_tuf_datastore_at(TEST_NOW).unwrap();

    assert_eq!(
        verify_repository_at(&config, &datastore, &store, TEST_NOW)
            .await
            .unwrap_err(),
        VerificationError::Manifest
    );
}

#[tokio::test]
async fn license_expression_requires_exact_shipped_policy_admission() {
    let fixture = build_repository(1, SOURCE_CONTENTS, "Apache-2.0", VALID_TUF_EXPIRY).await;
    let cache = tempfile::tempdir().unwrap();
    let config = fixture.config(
        &cache.path().canonicalize().unwrap().join("cache"),
        UpdateLimits::default(),
        &["MIT"],
    );
    let mut store = CatalogStore::open(&config).unwrap();
    let datastore = store.prepare_tuf_datastore_at(TEST_NOW).unwrap();

    assert_eq!(
        verify_repository_at(&config, &datastore, &store, TEST_NOW)
            .await
            .unwrap_err(),
        VerificationError::License
    );
}

#[tokio::test]
async fn tuf_datastore_rejects_metadata_version_rollback() {
    let newer = build_repository(2, "new\n", "MIT", VALID_TUF_EXPIRY).await;
    let older = build_repository(1, "old\n", "MIT", VALID_TUF_EXPIRY).await;
    let cache = tempfile::tempdir().unwrap();
    let cache_path = cache.path().canonicalize().unwrap().join("cache");
    let newer_config = newer.config(&cache_path, UpdateLimits::default(), &["MIT"]);
    let older_config = older.config(&cache_path, UpdateLimits::default(), &["MIT"]);
    let mut store = CatalogStore::open(&newer_config).unwrap();
    let datastore = store.prepare_tuf_datastore_at(TEST_NOW).unwrap();
    let verified = verify_repository_at(&newer_config, &datastore, &store, TEST_NOW)
        .await
        .unwrap();
    stage_and_commit_verified(&mut store, verified).unwrap();

    let datastore = store.prepare_tuf_datastore_at(TEST_NOW).unwrap();
    assert_eq!(
        verify_repository_at(&older_config, &datastore, &store, TEST_NOW)
            .await
            .unwrap_err(),
        VerificationError::Metadata
    );
}

#[tokio::test]
async fn committed_root_survives_shipped_root_update_and_downgrade() {
    let original = build_repository(1, "one\n", "MIT", VALID_TUF_EXPIRY).await;
    let rotated_root = rotate_fixture_root(2).await;
    let rotated =
        build_repository_with_root(2, "two\n", "MIT", VALID_TUF_EXPIRY, &rotated_root).await;
    let cache = tempfile::tempdir().unwrap();
    let cache_path = cache.path().canonicalize().unwrap().join("cache");
    let original_config = original.config(&cache_path, UpdateLimits::default(), &["MIT"]);
    let mut store = CatalogStore::open(&original_config).unwrap();
    let prepared = store.prepare_tuf_datastore_at(TEST_NOW).unwrap();
    let verified = verify_repository_at(&original_config, &prepared, &store, TEST_NOW)
        .await
        .unwrap();
    stage_and_commit_verified(&mut store, verified).unwrap();

    replace_directory_contents(&rotated.metadata_path, &original.metadata_path);
    replace_directory_contents(&rotated.targets_path, &original.targets_path);
    let prepared = store.prepare_tuf_datastore_at(TEST_NOW).unwrap();
    assert_eq!(root_version(&prepared.trusted_root), 1);
    let verified = verify_repository_at(&original_config, &prepared, &store, TEST_NOW)
        .await
        .unwrap();
    assert_eq!(verified.identity().revision, 2);
    stage_and_commit_verified(&mut store, verified).unwrap();
    drop(store);

    let mut shipped_newer = original_config.clone();
    shipped_newer.trusted_root = Arc::clone(&rotated_root);
    let mut reopened = CatalogStore::open(&shipped_newer).unwrap();
    let prepared = reopened.prepare_tuf_datastore_at(TEST_NOW).unwrap();
    assert_eq!(root_version(&prepared.trusted_root), 2);
    drop(reopened);

    // An older application-shipped bootstrap cannot roll the committed TUF
    // root back. Only the committed root is authoritative after first use.
    let mut downgraded = CatalogStore::open(&original_config).unwrap();
    let prepared = downgraded.prepare_tuf_datastore_at(TEST_NOW).unwrap();
    assert_eq!(root_version(&prepared.trusted_root), 2);
    drop(downgraded);

    let mut unrelated_epoch = original_config.clone();
    unrelated_epoch.repository_identity = [0x5a; 32];
    assert!(matches!(
        CatalogStore::open(&unrelated_epoch),
        Err(StoreError::RepositoryMismatch)
    ));
}

#[test]
fn package_expiry_and_provenance_query_are_rejected() {
    let policy = LicensePolicy::new(["MIT"]).unwrap();
    let expired = manifest(1, SOURCE_CONTENTS, "MIT");
    let expired_bytes = expired.encode_canonical().unwrap();
    assert_eq!(
        CatalogManifest::parse_canonical(
            &expired_bytes,
            expired.expires_unix,
            UpdateLimits::default(),
            &policy,
        ),
        Err(ManifestError::Expired)
    );

    let mut queried = manifest(1, SOURCE_CONTENTS, "MIT");
    queried.sources[0].license.source_url = "https://lists.example/source?mutable=tracking".into();
    let queried_bytes = queried.encode_canonical().unwrap();
    assert_eq!(
        CatalogManifest::parse_canonical(
            &queried_bytes,
            TEST_NOW,
            UpdateLimits::default(),
            &policy,
        ),
        Err(ManifestError::License)
    );
}

#[test]
fn post_journal_stage_failure_recovers_candidate_without_promoting_current() {
    let directory = tempfile::tempdir().unwrap();
    let config = storage_config(&directory);
    let mut store = CatalogStore::open(&config).unwrap();
    activate_test_catalog(&mut store, verified_catalog(1, "one\n")).unwrap();
    let _ = store.take_pending_activation();

    assert!(matches!(
        activate_test_catalog_with_fault(
            &mut store,
            verified_catalog(2, "two\n"),
            FaultPoint::AfterJournal
        ),
        Err(StoreError::InjectedFault)
    ));
    assert!(matches!(
        activate_test_catalog(&mut store, verified_catalog(3, "three\n")),
        Err(StoreError::ActivationSealed)
    ));
    drop(store);

    let mut recovered = CatalogStore::open(&config).unwrap();
    assert_eq!(recovered.current_identity().unwrap().revision, 1);
    assert_eq!(
        recovered
            .take_pending_candidate()
            .unwrap()
            .identity
            .revision,
        2
    );
    let mut superseding = verified_catalog(3, "three\n");
    superseding.repository_identity = recovered.repository_identity();
    assert!(matches!(
        recovered.stage_candidate(superseding),
        Err(StoreError::CandidatePending)
    ));
}

#[test]
fn exact_cache_lock_excludes_another_process_and_releases_on_drop() {
    let directory = tempfile::tempdir().unwrap();
    let config = storage_config(&directory);
    let store = CatalogStore::open(&config).unwrap();
    let child = std::env::current_exe().unwrap();
    let status = Command::new(child)
        .arg("--exact")
        .arg("tests::cache_lock_child_observes_exclusion")
        .arg("--nocapture")
        .env(
            "ZEPHIUM_BLOCKER_LOCK_TEST_CACHE",
            config.storage_dir().as_os_str(),
        )
        .status()
        .unwrap();
    assert!(status.success());
    drop(store);

    drop(CatalogStore::open(&config).unwrap());
}

#[test]
fn cache_lock_child_observes_exclusion() {
    let Some(cache) = std::env::var_os("ZEPHIUM_BLOCKER_LOCK_TEST_CACHE") else {
        return;
    };
    let config = storage_config_for_cache(PathBuf::from(cache));
    assert!(matches!(
        CatalogStore::open(&config),
        Err(StoreError::LockUnavailable)
    ));
}

#[test]
fn durable_clock_rejects_rollback_and_out_of_range_time() {
    let directory = tempfile::tempdir().unwrap();
    let config = storage_config(&directory);
    let mut store = CatalogStore::open(&config).unwrap();
    store.prepare_tuf_datastore_at(TEST_NOW).unwrap();
    assert!(matches!(
        store.prepare_tuf_datastore_at(TEST_NOW - 1),
        Err(StoreError::ClockRollback)
    ));
    assert!(matches!(
        store.prepare_tuf_datastore_at(7_258_118_401),
        Err(StoreError::ClockInvalid)
    ));
}

#[test]
fn startup_rejects_wall_clock_rollback_before_exposing_cached_policy() {
    let directory = tempfile::tempdir().unwrap();
    let config = storage_config(&directory);
    let mut store = CatalogStore::open(&config).unwrap();
    let future = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
        + 60;
    store.prepare_tuf_datastore_at(future).unwrap();
    drop(store);

    let worker = CatalogUpdateWorker::start(config);
    assert!(matches!(
        worker.status().status,
        UpdateStatus::Unavailable(UnavailableReason::ClockUnsafe)
    ));
    assert_eq!(
        worker.shutdown(std::time::Duration::from_millis(1)),
        ShutdownOutcome::Unavailable
    );
}

#[test]
fn corrupt_clock_is_stably_unavailable_and_corrupt_hint_is_due_now() {
    let directory = tempfile::tempdir().unwrap();
    let config = storage_config(&directory);
    {
        let mut store = CatalogStore::open(&config).unwrap();
        store.prepare_tuf_datastore_at(TEST_NOW).unwrap();
    }
    fs::write(
        config.storage_dir().join("last-refresh-attempt.json"),
        b"not canonical json",
    )
    .unwrap();
    let store = CatalogStore::open(&config).unwrap();
    assert_eq!(store.last_refresh_attempt_unix(), None);
    drop(store);

    fs::write(
        config.storage_dir().join("clock-high-water.json"),
        b"not canonical json",
    )
    .unwrap();
    let worker = CatalogUpdateWorker::start(config);
    assert!(matches!(
        worker.status().status,
        UpdateStatus::Unavailable(UnavailableReason::ClockUnsafe)
    ));
    assert_eq!(
        worker.shutdown(std::time::Duration::from_millis(1)),
        ShutdownOutcome::Unavailable
    );
}

#[test]
fn committed_state_requires_its_durable_clock_high_water() {
    let directory = tempfile::tempdir().unwrap();
    let config = storage_config(&directory);
    let mut store = CatalogStore::open(&config).unwrap();
    activate_test_catalog(&mut store, verified_catalog(1, "one\n")).unwrap();
    drop(store);
    fs::remove_file(config.storage_dir().join("clock-high-water.json")).unwrap();

    let worker = CatalogUpdateWorker::start(config);
    assert!(matches!(
        worker.status().status,
        UpdateStatus::Unavailable(UnavailableReason::ClockUnsafe)
    ));
    assert_eq!(
        worker.shutdown(std::time::Duration::from_millis(1)),
        ShutdownOutcome::Unavailable
    );
}

#[test]
fn every_candidate_stage_boundary_has_deterministic_recovery() {
    for fault in [
        FaultPoint::AfterObjects,
        FaultPoint::AfterJournal,
        FaultPoint::AfterStateReplace,
    ] {
        let directory = tempfile::tempdir().unwrap();
        let config = storage_config(&directory);
        let mut store = CatalogStore::open(&config).unwrap();
        activate_test_catalog(&mut store, verified_catalog(1, "one\n")).unwrap();
        let _ = store.take_pending_activation();
        assert!(matches!(
            activate_test_catalog_with_fault(&mut store, verified_catalog(2, "two\n"), fault),
            Err(StoreError::InjectedFault)
        ));
        drop(store);

        let mut recovered = CatalogStore::open(&config).unwrap();
        let expected_tuf_revision = match fault {
            FaultPoint::AfterObjects => 1,
            FaultPoint::AfterJournal | FaultPoint::AfterStateReplace => 2,
            FaultPoint::None => unreachable!(),
        };
        assert_eq!(recovered.current_identity().unwrap().revision, 1);
        let candidate = recovered
            .take_pending_candidate()
            .map(|candidate| candidate.identity.revision);
        assert_eq!(candidate, (fault != FaultPoint::AfterObjects).then_some(2));
        let prepared = recovered.prepare_tuf_datastore_at(TEST_NOW).unwrap();
        assert_eq!(
            prepared.trusted_root.as_ref(),
            format!("test-tuf-{expected_tuf_revision}-root.json").as_bytes()
        );
    }
}

#[test]
fn every_candidate_commit_boundary_has_deterministic_recovery() {
    for fault in [FaultPoint::AfterJournal, FaultPoint::AfterStateReplace] {
        let directory = tempfile::tempdir().unwrap();
        let config = storage_config(&directory);
        let mut store = CatalogStore::open(&config).unwrap();
        activate_test_catalog(&mut store, verified_catalog(1, "one\n")).unwrap();

        let mut verified = verified_catalog(2, "two\n");
        prepare_test_verified(&mut store, &mut verified);
        let identity = verified.identity().clone();
        assert!(matches!(
            store.stage_candidate(verified).unwrap(),
            crate::storage::CandidateStageOutcome::Candidate(_)
        ));
        assert!(matches!(
            store.commit_candidate_with_fault(&identity, TEST_NOW, fault),
            Err(StoreError::InjectedFault)
        ));
        drop(store);

        let mut recovered = CatalogStore::open(&config).unwrap();
        assert_eq!(recovered.current_identity().unwrap().revision, 2);
        assert!(recovered.take_pending_candidate().is_none());
        assert_eq!(
            recovered
                .take_pending_activation()
                .unwrap()
                .identity
                .revision,
            2
        );
    }
}

#[test]
fn every_candidate_rejection_boundary_recovers_rejected_high_water() {
    for fault in [FaultPoint::AfterJournal, FaultPoint::AfterStateReplace] {
        let directory = tempfile::tempdir().unwrap();
        let config = storage_config(&directory);
        let mut store = CatalogStore::open(&config).unwrap();
        activate_test_catalog(&mut store, verified_catalog(1, "one\n")).unwrap();
        let _ = store.take_pending_activation();

        let mut verified = verified_catalog(2, "rejected\n");
        prepare_test_verified(&mut store, &mut verified);
        let identity = verified.identity().clone();
        assert!(matches!(
            store.stage_candidate(verified).unwrap(),
            crate::storage::CandidateStageOutcome::Candidate(_)
        ));
        assert!(matches!(
            store.reject_candidate_with_fault(
                &identity,
                crate::storage::CandidateRejection::Compiler,
                fault,
            ),
            Err(StoreError::InjectedFault)
        ));
        drop(store);

        let mut recovered = CatalogStore::open(&config).unwrap();
        assert_eq!(recovered.current_identity().unwrap().revision, 1);
        assert!(recovered.take_pending_candidate().is_none());
        assert_eq!(
            recovered.classify_identity(&identity),
            Ok(crate::storage::CatalogIdentityDisposition::Rejected)
        );
        let prepared = recovered.prepare_tuf_datastore_at(TEST_NOW).unwrap();
        assert_eq!(prepared.trusted_root.as_ref(), b"test-tuf-2-root.json");
    }
}

#[test]
fn commit_time_expiry_rejection_collects_candidate_objects() {
    let directory = tempfile::tempdir().unwrap();
    let config = storage_config(&directory);
    let mut store = CatalogStore::open(&config).unwrap();
    activate_test_catalog(&mut store, verified_catalog(1, "one\n")).unwrap();
    let baseline_entries = object_entry_count(&config);

    let mut verified = verified_catalog(2, "expired-before-commit\n");
    prepare_test_verified(&mut store, &mut verified);
    let identity = verified.identity().clone();
    assert!(matches!(
        store.stage_candidate(verified).unwrap(),
        crate::storage::CandidateStageOutcome::Candidate(_)
    ));
    assert!(matches!(
        store.commit_candidate_at_for_test(&identity, identity.expires_unix),
        Err(StoreError::CandidateExpired)
    ));
    store
        .reject_candidate(&identity, crate::storage::CandidateRejection::Expired)
        .unwrap();
    assert_eq!(object_entry_count(&config), baseline_entries);
    assert_eq!(
        store.classify_identity(&identity),
        Ok(crate::storage::CatalogIdentityDisposition::Rejected)
    );
}

#[test]
fn unchanged_package_advances_tuf_state_without_displacing_previous_package() {
    let directory = tempfile::tempdir().unwrap();
    let config = storage_config(&directory);
    let mut store = CatalogStore::open(&config).unwrap();
    activate_test_catalog(&mut store, verified_catalog(1, "one\n")).unwrap();
    activate_test_catalog(&mut store, verified_catalog(2, "two\n")).unwrap();
    let generation = store.generation();

    let mut unchanged = verified_catalog(2, "two\n");
    unchanged.repository_identity = store.repository_identity();
    unchanged.payload = VerifiedCatalogPayload::Unchanged {
        identity: unchanged.identity().clone(),
        repaired_objects: Vec::new(),
    };
    prepare_test_tuf_stage(&mut store, 99);
    unchanged.tuf_descriptor_sha256 = store.staged_tuf_seal();
    stage_and_commit_verified(&mut store, unchanged).unwrap();
    assert_eq!(store.generation(), generation + 1);
    assert_eq!(store.current_identity().unwrap().revision, 2);
    assert_eq!(store.load_previous().unwrap().unwrap().identity.revision, 1);
    let prepared = store.prepare_tuf_datastore_at(TEST_NOW).unwrap();
    assert_eq!(prepared.trusted_root.as_ref(), b"test-tuf-99-root.json");
}

#[test]
fn multiple_same_process_activations_checkpoint_without_journal_growth() {
    let directory = tempfile::tempdir().unwrap();
    let config = storage_config(&directory);
    let mut store = CatalogStore::open(&config).unwrap();
    for revision in 1..=3 {
        activate_test_catalog(
            &mut store,
            verified_catalog(revision, &format!("revision-{revision}\n")),
        )
        .unwrap();
        let _ = store.take_pending_activation();
        assert_eq!(
            fs::read_dir(config.storage_dir().join("activation-journals"))
                .unwrap()
                .count(),
            0
        );
    }
    drop(store);

    let mut recovered = CatalogStore::open(&config).unwrap();
    assert_eq!(recovered.current_identity().unwrap().revision, 3);
    assert_eq!(
        recovered
            .load_previous()
            .unwrap()
            .unwrap()
            .identity
            .revision,
        2
    );
    assert_eq!(
        recovered
            .take_pending_activation()
            .unwrap()
            .identity
            .revision,
        3
    );
    assert!(recovered.take_pending_activation().is_none());
}

#[test]
fn cache_collection_is_bounded_and_never_removes_current_or_previous() {
    let directory = tempfile::tempdir().unwrap();
    let config = storage_config(&directory);
    let mut store = CatalogStore::open(&config).unwrap();
    activate_test_catalog(&mut store, verified_catalog(1, "one\n")).unwrap();
    activate_test_catalog(&mut store, verified_catalog(2, "two\n")).unwrap();
    assert!(matches!(
        activate_test_catalog_with_fault(
            &mut store,
            verified_catalog(3, "orphan\n"),
            FaultPoint::AfterObjects
        ),
        Err(StoreError::InjectedFault)
    ));

    let report = store
        .collect_garbage(GcBudget {
            max_entries_scanned: 64,
            max_objects_removed: 1,
            max_bytes_removed: 1024 * 1024,
        })
        .unwrap();
    assert_eq!(report.objects_removed, 1);
    assert_eq!(store.current_identity().unwrap().revision, 2);
    assert_eq!(store.load_previous().unwrap().unwrap().identity.revision, 1);
}

#[test]
fn background_gc_prevents_repeated_pre_journal_crashes_from_accumulating() {
    let directory = tempfile::tempdir().unwrap();
    let config = storage_config(&directory);
    let mut store = CatalogStore::open(&config).unwrap();
    activate_test_catalog(&mut store, verified_catalog(1, "one\n")).unwrap();
    drop(store);
    let baseline = object_entry_count(&config);

    for revision in 2..=12 {
        let mut store = CatalogStore::open(&config).unwrap();
        assert!(matches!(
            activate_test_catalog_with_fault(
                &mut store,
                verified_catalog(revision, &format!("orphan-{revision}\n")),
                FaultPoint::AfterObjects
            ),
            Err(StoreError::InjectedFault)
        ));
        drop(store);

        let recovered = CatalogStore::open(&config).unwrap();
        assert_eq!(recovered.current_identity().unwrap().revision, 1);
        recovered.collect_garbage(GcBudget::default()).unwrap();
        drop(recovered);
        assert_eq!(object_entry_count(&config), baseline);
    }
}

#[test]
fn near_entry_cap_after_objects_crash_reopens_and_converges() {
    let directory = tempfile::tempdir().unwrap();
    let config = storage_config(&directory);
    let mut store = CatalogStore::open(&config).unwrap();
    activate_test_catalog(&mut store, verified_catalog(1, "one\n")).unwrap();
    activate_test_catalog(&mut store, verified_catalog(2, "two\n")).unwrap();
    store.collect_garbage(GcBudget::default()).unwrap();
    let protected_entries = object_entry_count(&config);

    let objects = config.storage_dir().join("objects");
    for index in 0..(HARD_MAX_CACHE_OBJECTS - protected_entries) {
        let digest = hex_sha256(format!("orphan-object-{index}").as_bytes());
        write_private_test_file(&objects.join(digest), b"");
    }
    assert_eq!(object_entry_count(&config), HARD_MAX_CACHE_OBJECTS);
    assert!(matches!(
        activate_test_catalog_with_fault(
            &mut store,
            verified_catalog(3, "pre-journal-orphan\n"),
            FaultPoint::AfterObjects
        ),
        Err(StoreError::InjectedFault)
    ));
    assert!(object_entry_count(&config) > HARD_MAX_CACHE_OBJECTS);
    assert!(
        object_entry_count(&config) <= crate::storage::recovery_cas_entry_limit(&config).unwrap()
    );
    drop(store);

    let reopened = CatalogStore::open(&config).unwrap();
    assert_eq!(reopened.current_identity().unwrap().revision, 2);
    assert_eq!(
        reopened.load_previous().unwrap().unwrap().identity.revision,
        1
    );
    assert_eq!(object_entry_count(&config), protected_entries);
}

#[test]
fn near_byte_cap_after_objects_crash_reopens_below_steady_ceiling() {
    let directory = tempfile::tempdir().unwrap();
    let config = small_cas_limit_config(&directory);
    let mut store = CatalogStore::open(&config).unwrap();
    activate_test_catalog(&mut store, verified_catalog(1, "one\n")).unwrap();
    let baseline_bytes = object_namespace_bytes(&config);
    let steady_limit = crate::storage::steady_cas_byte_limit(&config).unwrap();
    let recovery_limit = crate::storage::recovery_cas_byte_limit(&config).unwrap();
    let mut remaining = steady_limit - baseline_bytes;
    let objects = config.storage_dir().join("objects");
    let mut index = 0u64;
    while remaining > 0 {
        let bytes = remaining.min(config.limits.max_source_bytes);
        let digest = hex_sha256(format!("near-byte-cap-{index}").as_bytes());
        write_private_sparse_test_file(&objects.join(digest), bytes);
        remaining -= bytes;
        index += 1;
    }
    assert_eq!(object_namespace_bytes(&config), steady_limit);

    assert!(matches!(
        activate_test_catalog_with_fault(
            &mut store,
            verified_catalog(2, "pre-journal-byte-orphan\n"),
            FaultPoint::AfterObjects
        ),
        Err(StoreError::InjectedFault)
    ));
    assert!(object_namespace_bytes(&config) > steady_limit);
    assert!(object_namespace_bytes(&config) <= recovery_limit);
    drop(store);

    let recovered = CatalogStore::open(&config).unwrap();
    assert_eq!(recovered.current_identity().unwrap().revision, 1);
    assert_eq!(object_namespace_bytes(&config), baseline_bytes);
}

#[test]
fn incomplete_manifest_keeps_recovery_capacity_until_references_are_restored() {
    let directory = tempfile::tempdir().unwrap();
    let config = small_cas_limit_config(&directory);
    let mut store = CatalogStore::open(&config).unwrap();
    let verified = verified_catalog(1, "one\n");
    let manifest_bytes = verified.manifest.encode_canonical().unwrap();
    let manifest_path = config
        .storage_dir()
        .join("objects")
        .join(hex_sha256(&manifest_bytes));
    activate_test_catalog(&mut store, verified).unwrap();
    drop(store);

    fs::remove_file(&manifest_path).unwrap();
    let steady_limit = crate::storage::steady_cas_byte_limit(&config).unwrap();
    let recovery_limit = crate::storage::recovery_cas_byte_limit(&config).unwrap();
    let objects = config.storage_dir().join("objects");
    let mut remaining = steady_limit + 1 - object_namespace_bytes(&config);
    let mut index = 0u64;
    while remaining > 0 {
        let bytes = remaining.min(config.limits.max_source_bytes);
        let digest = hex_sha256(format!("degraded-recovery-orphan-{index}").as_bytes());
        write_private_sparse_test_file(&objects.join(digest), bytes);
        remaining -= bytes;
        index += 1;
    }
    assert!(object_namespace_bytes(&config) > steady_limit);
    assert!(object_namespace_bytes(&config) <= recovery_limit);

    let store = CatalogStore::open(&config).unwrap();
    assert!(object_namespace_bytes(&config) > steady_limit);
    assert_eq!(
        store.collect_garbage(GcBudget::default()).unwrap(),
        crate::storage::GarbageCollectionReport::default()
    );

    write_private_test_file(&manifest_path, &manifest_bytes);
    assert!(
        store
            .collect_garbage(GcBudget {
                max_entries_scanned: HARD_MAX_CACHE_OBJECTS,
                max_objects_removed: HARD_MAX_CACHE_OBJECTS,
                max_bytes_removed: recovery_limit,
            })
            .unwrap()
            .objects_removed
            > 0
    );
    assert!(object_namespace_bytes(&config) <= steady_limit);
}

#[test]
fn recovery_byte_ceiling_rejects_an_oversized_namespace() {
    let directory = tempfile::tempdir().unwrap();
    let config = small_cas_limit_config(&directory);
    drop(CatalogStore::open(&config).unwrap());
    let objects = config.storage_dir().join("objects");
    let mut remaining = crate::storage::recovery_cas_byte_limit(&config).unwrap() + 1;
    let mut index = 0u64;
    while remaining > 0 {
        let bytes = remaining.min(config.limits.max_source_bytes);
        let digest = hex_sha256(format!("over-recovery-byte-cap-{index}").as_bytes());
        write_private_sparse_test_file(&objects.join(digest), bytes);
        remaining -= bytes;
        index += 1;
    }

    assert!(matches!(
        CatalogStore::open(&config),
        Err(StoreError::ObjectCorrupt)
    ));
}

#[test]
fn maximum_byte_orphan_churn_converges_for_more_than_thirty_passes() {
    let directory = tempfile::tempdir().unwrap();
    let config = small_cas_limit_config(&directory);
    let mut store = CatalogStore::open(&config).unwrap();
    activate_test_catalog(&mut store, verified_catalog(1, "one\n")).unwrap();
    let baseline_bytes = object_namespace_bytes(&config);
    let objects = config.storage_dir().join("objects");

    for pass in 0..40 {
        for (part, bytes) in [
            config.limits.max_source_bytes,
            config.limits.max_source_bytes,
            16 * 1024,
        ]
        .into_iter()
        .enumerate()
        {
            let digest = hex_sha256(format!("byte-churn-{pass}-{part}").as_bytes());
            write_private_sparse_test_file(&objects.join(digest), bytes);
        }
        let report = store.collect_garbage(GcBudget::default()).unwrap();
        assert_eq!(report.objects_removed, 3);
        assert_eq!(object_namespace_bytes(&config), baseline_bytes);
    }
    drop(store);

    let recovered = CatalogStore::open(&config).unwrap();
    assert_eq!(recovered.current_identity().unwrap().revision, 1);
    assert_eq!(object_namespace_bytes(&config), baseline_bytes);
}

#[test]
fn more_than_thirty_package_revisions_remain_bounded_and_reopenable() {
    let directory = tempfile::tempdir().unwrap();
    let config = storage_config(&directory);
    let mut store = CatalogStore::open(&config).unwrap();

    for revision in 1..=40 {
        activate_test_catalog(
            &mut store,
            verified_catalog(revision, &format!("revision-{revision}\n")),
        )
        .unwrap();
        store.collect_garbage(GcBudget::default()).unwrap();
        assert!(object_entry_count(&config) < 64);
    }
    drop(store);

    let recovered = CatalogStore::open(&config).unwrap();
    assert_eq!(recovered.current_identity().unwrap().revision, 40);
    assert_eq!(
        recovered
            .load_previous()
            .unwrap()
            .unwrap()
            .identity
            .revision,
        39
    );
    assert!(object_entry_count(&config) < 64);
}

#[test]
fn unchanged_and_rejected_refresh_churn_collects_obsolete_metadata() {
    let directory = tempfile::tempdir().unwrap();
    let config = storage_config(&directory);
    let mut store = CatalogStore::open(&config).unwrap();
    activate_test_catalog(&mut store, verified_catalog(1, "one\n")).unwrap();
    store.collect_garbage(GcBudget::default()).unwrap();
    let baseline = object_entry_count(&config);
    let current = store.current_identity().unwrap();

    for marker in 2..=40 {
        let mut unchanged = verified_catalog(1, "one\n");
        unchanged.repository_identity = store.repository_identity();
        unchanged.payload = VerifiedCatalogPayload::Unchanged {
            identity: current.clone(),
            repaired_objects: Vec::new(),
        };
        prepare_test_tuf_stage(&mut store, marker);
        unchanged.tuf_descriptor_sha256 = store.staged_tuf_seal();
        assert!(matches!(
            store.stage_candidate(unchanged).unwrap(),
            crate::storage::CandidateStageOutcome::Unchanged(identity)
                if identity == current
        ));
        assert_eq!(object_entry_count(&config), baseline);
    }

    let mut rejected = verified_catalog(2, "rejected\n");
    prepare_test_verified(&mut store, &mut rejected);
    let rejected_identity = rejected.identity().clone();
    assert!(matches!(
        store.stage_candidate(rejected).unwrap(),
        crate::storage::CandidateStageOutcome::Candidate(_)
    ));
    store
        .reject_candidate(
            &rejected_identity,
            crate::storage::CandidateRejection::Compiler,
        )
        .unwrap();
    store.collect_garbage(GcBudget::default()).unwrap();
    assert_eq!(object_entry_count(&config), baseline);

    for marker in 41..=80 {
        let mut unchanged_rejection = verified_catalog(2, "rejected\n");
        unchanged_rejection.repository_identity = store.repository_identity();
        unchanged_rejection.payload =
            VerifiedCatalogPayload::RejectedUnchanged(rejected_identity.clone());
        prepare_test_tuf_stage(&mut store, marker);
        unchanged_rejection.tuf_descriptor_sha256 = store.staged_tuf_seal();
        assert!(matches!(
            store.stage_candidate(unchanged_rejection).unwrap(),
            crate::storage::CandidateStageOutcome::Rejected
        ));
        assert_eq!(object_entry_count(&config), baseline);
    }
    drop(store);

    let recovered = CatalogStore::open(&config).unwrap();
    assert_eq!(recovered.current_identity(), Some(current));
    assert_eq!(
        recovered.classify_identity(&rejected_identity),
        Ok(crate::storage::CatalogIdentityDisposition::Rejected)
    );
}

#[test]
fn tuf_sized_cas_objects_are_admitted_when_source_limits_are_smaller() {
    let directory = tempfile::tempdir().unwrap();
    let mut config = storage_config(&directory);
    config.limits.max_source_bytes = 8;
    config.limits.max_total_source_bytes = 8;
    drop(CatalogStore::open(&config).unwrap());

    let bytes = vec![b'm'; 256 * 1024];
    let digest = hex_sha256(&bytes);
    write_private_test_file(&config.storage_dir().join("objects").join(&digest), &bytes);
    let store = CatalogStore::open(&config).unwrap();
    store.collect_garbage(GcBudget::default()).unwrap();
    drop(store);
    assert!(!config.storage_dir().join("objects").join(digest).exists());
}

#[test]
fn corrupt_cas_object_is_detected_without_eager_startup_loading() {
    let directory = tempfile::tempdir().unwrap();
    let config = storage_config(&directory);
    let mut store = CatalogStore::open(&config).unwrap();
    let verified = verified_catalog(1, "one\n");
    let source_digest = verified.manifest.sources[0].sha256.clone();
    activate_test_catalog(&mut store, verified).unwrap();
    drop(store);

    fs::write(
        config.storage_dir().join("objects").join(&source_digest),
        b"bad\n",
    )
    .unwrap();
    let store = CatalogStore::open(&config).unwrap();
    let digest = crate::manifest::decode_sha256(&source_digest).unwrap();
    assert_eq!(
        store.validate_cached_object(&digest, 4),
        Ok(crate::storage::CachedObjectState::Corrupt)
    );
}

#[test]
fn garbage_collection_pauses_while_a_retained_manifest_is_repairable() {
    for corruption in [
        CachedObjectCorruption::Missing,
        CachedObjectCorruption::Short,
        CachedObjectCorruption::SameLength,
        CachedObjectCorruption::Long,
    ] {
        let directory = tempfile::tempdir().unwrap();
        let config = storage_config(&directory);
        let mut store = CatalogStore::open(&config).unwrap();
        let verified = verified_catalog(1, "one\n");
        let manifest_bytes = verified.manifest.encode_canonical().unwrap();
        let manifest_path = config
            .storage_dir()
            .join("objects")
            .join(hex_sha256(&manifest_bytes));
        activate_test_catalog(&mut store, verified).unwrap();
        let orphan = b"unreferenced";
        let orphan_path = config
            .storage_dir()
            .join("objects")
            .join(hex_sha256(orphan));
        write_private_test_file(&orphan_path, orphan);
        drop(store);

        corrupt_cached_object(&manifest_path, &manifest_bytes, corruption);
        let store = CatalogStore::open(&config).unwrap();
        assert_eq!(
            store.collect_garbage(GcBudget::default()).unwrap(),
            crate::storage::GarbageCollectionReport::default()
        );
        assert!(orphan_path.exists());

        write_private_test_file(&manifest_path, &manifest_bytes);
        let report = store.collect_garbage(GcBudget::default()).unwrap();
        assert_eq!(report.objects_removed, 1);
        assert!(!orphan_path.exists());
    }
}

#[test]
fn degraded_manifest_requires_the_exact_durable_admission_policy() {
    let directory = tempfile::tempdir().unwrap();
    let config = storage_config(&directory);
    let mut store = CatalogStore::open(&config).unwrap();
    let verified = verified_catalog(1, "one\n");
    let manifest_bytes = verified.manifest.encode_canonical().unwrap();
    let manifest_path = config
        .storage_dir()
        .join("objects")
        .join(hex_sha256(&manifest_bytes));
    activate_test_catalog(&mut store, verified).unwrap();
    drop(store);

    let mut broadened = config.clone();
    broadened.license_policy = LicensePolicy::new(["MIT", "Apache-2.0"]).unwrap();
    fs::remove_file(&manifest_path).unwrap();
    assert!(matches!(
        CatalogStore::open(&broadened),
        Err(StoreError::InvalidPackage)
    ));

    write_private_test_file(&manifest_path, &manifest_bytes);
    let mut store = CatalogStore::open(&broadened).unwrap();
    activate_test_catalog(&mut store, verified_catalog(1, "one\n")).unwrap();
    drop(store);

    fs::remove_file(&manifest_path).unwrap();
    assert!(CatalogStore::open(&broadened).is_ok());
}

#[cfg(unix)]
#[test]
fn unsafe_current_manifest_identity_keeps_updater_unavailable() {
    use std::os::unix::fs::symlink;

    let directory = tempfile::tempdir().unwrap();
    let config = storage_config(&directory);
    let mut store = CatalogStore::open(&config).unwrap();
    let verified = verified_catalog(1, "one\n");
    let manifest_bytes = verified.manifest.encode_canonical().unwrap();
    let manifest_path = config
        .storage_dir()
        .join("objects")
        .join(hex_sha256(&manifest_bytes));
    activate_test_catalog(&mut store, verified).unwrap();
    drop(store);

    fs::remove_file(&manifest_path).unwrap();
    let outside = directory.path().join("outside");
    write_private_test_file(&outside, &manifest_bytes);
    symlink(&outside, &manifest_path).unwrap();

    let worker = CatalogUpdateWorker::start(config);
    assert!(matches!(
        worker.status().status,
        UpdateStatus::Unavailable(UnavailableReason::StorageUnavailable)
    ));
    assert_eq!(
        worker.shutdown(std::time::Duration::ZERO),
        ShutdownOutcome::Unavailable
    );
}

#[cfg(unix)]
#[test]
fn hard_linked_cache_object_is_rejected() {
    let directory = tempfile::tempdir().unwrap();
    let config = storage_config(&directory);
    let mut store = CatalogStore::open(&config).unwrap();
    let verified = verified_catalog(1, "one\n");
    let source_digest = verified.manifest.sources[0].sha256.clone();
    activate_test_catalog(&mut store, verified).unwrap();
    drop(store);
    let object = config.storage_dir().join("objects").join(source_digest);
    fs::hard_link(&object, config.storage_dir().join("alias")).unwrap();

    assert!(matches!(
        CatalogStore::open(&config),
        Err(StoreError::UnsafePath)
    ));
}

#[test]
fn stale_exact_stage_files_are_removed_before_recovery() {
    let directory = tempfile::tempdir().unwrap();
    let config = storage_config(&directory);
    drop(CatalogStore::open(&config).unwrap());

    let root = config.storage_dir();
    let digest = "a".repeat(64);
    let root_stage = root.join(".state.json.stage");
    let object_stage = root.join("objects").join(format!(".{digest}.stage"));
    let journal_stage = root
        .join("activation-journals")
        .join(format!(".{:020}-{digest}.json.stage", 1));
    for path in [&root_stage, &object_stage, &journal_stage] {
        write_private_test_file(path, b"interrupted write");
    }

    drop(CatalogStore::open(&config).unwrap());
    assert!(!root_stage.exists());
    assert!(!object_stage.exists());
    assert!(!journal_stage.exists());
}

#[test]
fn unknown_entries_in_owned_namespaces_fail_closed() {
    for relative in [
        PathBuf::from("unexpected"),
        PathBuf::from("objects").join("unexpected"),
        PathBuf::from("tuf-stage").join("unexpected"),
        PathBuf::from("activation-journals").join("unexpected"),
    ] {
        let directory = tempfile::tempdir().unwrap();
        let config = storage_config(&directory);
        drop(CatalogStore::open(&config).unwrap());
        write_private_test_file(&config.storage_dir().join(&relative), b"unexpected");

        assert!(CatalogStore::open(&config).is_err(), "{relative:?}");
    }
}

#[test]
fn journal_recovery_has_a_hard_directory_entry_bound() {
    let directory = tempfile::tempdir().unwrap();
    let config = storage_config(&directory);
    drop(CatalogStore::open(&config).unwrap());
    let journals = config.storage_dir().join("activation-journals");
    let digest = "b".repeat(64);
    for generation in 1..=1025 {
        write_private_test_file(
            &journals.join(format!(".{generation:020}-{digest}.json.stage")),
            b"interrupted write",
        );
    }

    assert!(matches!(
        CatalogStore::open(&config),
        Err(StoreError::JournalAmbiguous)
    ));
}

fn write_private_test_file(path: &Path, bytes: &[u8]) {
    fs::write(path, bytes).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
    }
}

fn corrupt_cached_object(path: &Path, expected: &[u8], corruption: CachedObjectCorruption) {
    match corruption {
        CachedObjectCorruption::Missing => match fs::remove_file(path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => panic!("failed to remove cached object: {error}"),
        },
        CachedObjectCorruption::Short => {
            assert!(expected.len() > 1);
            write_private_test_file(path, &vec![0xa5; expected.len() - 1]);
        }
        CachedObjectCorruption::SameLength => {
            write_private_test_file(path, &vec![0xa5; expected.len()]);
        }
        CachedObjectCorruption::Long => {
            write_private_test_file(path, &vec![0xa5; expected.len() + 1]);
        }
    }
}

fn prepare_policy_catalog(
    compiler: &WorkerBlocker,
    candidate: &ActivatedCatalog,
) -> CatalogPreparationOutcome {
    let (done_tx, done_rx) = std::sync::mpsc::sync_channel(1);
    assert_eq!(
        compiler.prepare_catalog(
            candidate.identity.manifest_sha256,
            candidate.catalog.clone(),
            Box::new(move |outcome| done_tx.send(outcome).unwrap()),
        ),
        CatalogReplacementDispatch::Scheduled
    );
    done_rx
        .recv_timeout(std::time::Duration::from_secs(10))
        .unwrap()
}

fn identity_for_test(availability: &CatalogAvailability) -> &CatalogIdentity {
    match availability {
        CatalogAvailability::Fresh(identity) | CatalogAvailability::Stale(identity) => identity,
    }
}

fn wait_for_candidate(worker: &CatalogUpdateWorker, operation: u64) -> ActivatedCatalog {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        let (snapshot, current, candidate) = worker.observe_and_take_catalogs();
        assert!(current.is_none());
        if let Some(candidate) = candidate {
            return candidate;
        }
        assert!(
            !matches!(
                snapshot.status,
                UpdateStatus::Failed {
                    operation: failed,
                    ..
                } if failed == operation
            ),
            "refresh failed before publishing its candidate: {snapshot:?}"
        );
        assert!(!matches!(snapshot.status, UpdateStatus::Shutdown));
        let remaining = deadline
            .checked_duration_since(std::time::Instant::now())
            .expect("worker candidate deadline elapsed");
        assert!(matches!(
            worker.wait_for_change(snapshot.revision, remaining),
            WaitForStatus::Changed(_)
        ));
    }
}

fn activate_test_catalog(
    store: &mut CatalogStore,
    mut verified: VerifiedCatalog,
) -> Result<(), StoreError> {
    prepare_test_verified(store, &mut verified);
    stage_and_commit_verified(store, verified)
}

fn stage_and_commit_verified(
    store: &mut CatalogStore,
    verified: VerifiedCatalog,
) -> Result<(), StoreError> {
    let identity = verified.identity().clone();
    match store.stage_candidate(verified)? {
        crate::storage::CandidateStageOutcome::Candidate(_) => store
            .commit_candidate_at_for_test(&identity, TEST_NOW)
            .map(|_| ()),
        crate::storage::CandidateStageOutcome::Unchanged(_) => Ok(()),
        crate::storage::CandidateStageOutcome::Rejected => Err(StoreError::CandidateRejected),
    }
}

fn activate_test_catalog_with_fault(
    store: &mut CatalogStore,
    mut verified: VerifiedCatalog,
    fault: FaultPoint,
) -> Result<(), StoreError> {
    prepare_test_verified(store, &mut verified);
    store
        .stage_candidate_with_fault(verified, fault)
        .map(|_| ())
}

fn prepare_test_verified(store: &mut CatalogStore, verified: &mut VerifiedCatalog) {
    verified.repository_identity = store.repository_identity();
    let identity = verified.identity().clone();
    if store.classify_identity(&identity).unwrap()
        == crate::storage::CatalogIdentityDisposition::Current
    {
        verified.payload = VerifiedCatalogPayload::Unchanged {
            identity,
            repaired_objects: Vec::new(),
        };
    }
    prepare_test_tuf_stage(store, verified.identity().revision);
    verified.tuf_descriptor_sha256 = store.staged_tuf_seal();
}

fn prepare_test_tuf_stage(store: &mut CatalogStore, marker: u64) {
    let stage = store.prepare_tuf_datastore_at(TEST_NOW).unwrap();
    for name in [
        "root.json",
        "timestamp.json",
        "snapshot.json",
        "targets.json",
        "latest_known_time.json",
    ] {
        write_private_test_file(
            &stage.path.join(name),
            format!("test-tuf-{marker}-{name}").as_bytes(),
        );
    }
}

fn storage_config(directory: &TempDir) -> RepositoryConfig {
    storage_config_for_cache(directory.path().canonicalize().unwrap().join("cache"))
}

fn small_cas_limit_config(directory: &TempDir) -> RepositoryConfig {
    let mut config = storage_config(directory);
    config.limits.max_root_bytes = 4 * 1024;
    config.limits.max_targets_metadata_bytes = 4 * 1024;
    config.limits.max_timestamp_metadata_bytes = 4 * 1024;
    config.limits.max_snapshot_metadata_bytes = 4 * 1024;
    config.limits.max_manifest_bytes = 4 * 1024;
    config.limits.max_sources = 2;
    config.limits.max_source_bytes = 64 * 1024;
    config.limits.max_total_source_bytes = 128 * 1024;
    config
}

fn storage_config_for_cache(cache: PathBuf) -> RepositoryConfig {
    let base = Url::from_directory_path(cache.parent().unwrap()).unwrap();
    RepositoryConfig::for_filesystem_tests(
        Arc::<[u8]>::from(&b"test-root"[..]),
        base.clone(),
        base,
        cache,
        LicensePolicy::new(["MIT"]).unwrap(),
        UpdateLimits::default(),
    )
}

fn object_entry_count(config: &RepositoryConfig) -> usize {
    fs::read_dir(config.storage_dir().join("objects"))
        .unwrap()
        .count()
}

fn object_namespace_bytes(config: &RepositoryConfig) -> u64 {
    fs::read_dir(config.storage_dir().join("objects"))
        .unwrap()
        .map(|entry| entry.unwrap().metadata().unwrap().len())
        .sum()
}

fn write_private_sparse_test_file(path: &Path, bytes: u64) {
    let file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .unwrap();
    file.set_len(bytes).unwrap();
    drop(file);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
    }
}

fn verified_catalog(revision: u64, contents: &str) -> VerifiedCatalog {
    let manifest = manifest(revision, contents, "MIT");
    let manifest_bytes = manifest.encode_canonical().unwrap();
    let source_contents = vec![Arc::<str>::from(contents)];
    let catalog = manifest.build_verified_catalog(&source_contents).unwrap();
    VerifiedCatalog {
        repository_identity: [0; 32],
        tuf_descriptor_sha256: [0; 32],
        verified_at_unix: TEST_NOW,
        payload: VerifiedCatalogPayload::Candidate {
            activated: ActivatedCatalog {
                identity: CatalogIdentity {
                    revision,
                    manifest_sha256: Sha256::digest(&manifest_bytes),
                    created_unix: manifest.created_unix,
                    expires_unix: manifest.expires_unix,
                    source_count: 1,
                    source_bytes: contents.len() as u64,
                },
                catalog: zephium_blocker::PolicyCatalog::authenticated(
                    Sha256::digest(&manifest_bytes),
                    catalog,
                ),
            },
            source_contents,
        },
        manifest,
    }
}

fn manifest(revision: u64, contents: &str, license: &str) -> CatalogManifest {
    manifest_at(revision, contents, license, TEST_NOW)
}

fn manifest_at(revision: u64, contents: &str, license: &str, package_now: u64) -> CatalogManifest {
    CatalogManifest {
        schema_version: CATALOG_MANIFEST_VERSION,
        revision,
        created_unix: package_now - 60,
        // Production admission requires at least 24 hours of remaining
        // validity. Give asynchronous worker tests scheduling headroom instead
        // of creating a package that becomes invalid one second after signing.
        expires_unix: package_now + 7 * 24 * 60 * 60,
        sources: vec![ManifestSource {
            id: "test-list".into(),
            format: ManifestSourceFormat::Standard,
            target: SOURCE_NAME.into(),
            length: contents.len() as u64,
            sha256: hex_sha256(contents.as_bytes()),
            license: LicenseMetadata {
                license_expression: license.into(),
                attribution: "Test-only list".into(),
                redistribution: "Test-only redistribution permission".into(),
                source_url: "https://lists.example/source".into(),
            },
        }],
    }
}

async fn build_repository(
    revision: u64,
    contents: &str,
    license: &str,
    metadata_expiry: i64,
) -> RepositoryFixture {
    build_repository_at(revision, contents, license, metadata_expiry, TEST_NOW).await
}

async fn build_repository_at(
    revision: u64,
    contents: &str,
    license: &str,
    metadata_expiry: i64,
    package_now: u64,
) -> RepositoryFixture {
    let trusted_root = Arc::from(fs::read(fixture_path("root.json")).unwrap());
    build_repository_with_root_at(
        revision,
        contents,
        license,
        metadata_expiry,
        package_now,
        &trusted_root,
    )
    .await
}

async fn build_repository_with_root(
    revision: u64,
    contents: &str,
    license: &str,
    metadata_expiry: i64,
    trusted_root: &Arc<[u8]>,
) -> RepositoryFixture {
    build_repository_with_root_at(
        revision,
        contents,
        license,
        metadata_expiry,
        TEST_NOW,
        trusted_root,
    )
    .await
}

async fn build_repository_with_root_at(
    revision: u64,
    contents: &str,
    license: &str,
    metadata_expiry: i64,
    package_now: u64,
    trusted_root: &Arc<[u8]>,
) -> RepositoryFixture {
    let directory = tempfile::tempdir().unwrap();
    let inputs = directory.path().join("inputs");
    let metadata = directory.path().join("metadata");
    let targets = directory.path().join("targets");
    fs::create_dir(&inputs).unwrap();
    let manifest = manifest_at(revision, contents, license, package_now);
    fs::write(
        inputs.join(CATALOG_MANIFEST_TARGET),
        manifest.encode_canonical().unwrap(),
    )
    .unwrap();
    fs::write(inputs.join(SOURCE_NAME), contents).unwrap();

    let root_path = directory.path().join("root.json");
    fs::write(&root_path, trusted_root).unwrap();
    let key_path = fixture_path("test-key.pem");
    let version = NonZeroU64::new(revision).unwrap();
    let expiry = Timestamp::from_second(metadata_expiry).unwrap();
    let mut editor = RepositoryEditor::new(&root_path).await.unwrap();
    editor
        .targets_version(version)
        .unwrap()
        .targets_expires(expiry)
        .unwrap()
        .snapshot_version(version)
        .snapshot_expires(expiry)
        .timestamp_version(version)
        .timestamp_expires(expiry)
        .add_target_paths(vec![
            inputs.join(CATALOG_MANIFEST_TARGET),
            inputs.join(SOURCE_NAME),
        ])
        .await
        .unwrap();
    let keys: Vec<Box<dyn KeySource>> = vec![Box::new(LocalKeySource { path: key_path })];
    let signed = editor.sign(&keys).await.unwrap();
    signed.write(&metadata).await.unwrap();
    signed
        .copy_targets(&inputs, &targets, PathExists::Fail)
        .await
        .unwrap();

    RepositoryFixture {
        trusted_root: Arc::clone(trusted_root),
        metadata_url: Url::from_directory_path(&metadata).unwrap(),
        metadata_path: metadata,
        targets_url: Url::from_directory_path(&targets).unwrap(),
        targets_path: targets,
        _directory: directory,
    }
}

async fn rotate_fixture_root(version: u64) -> Arc<[u8]> {
    let root_bytes = fs::read(fixture_path("root.json")).unwrap();
    let mut signed: Signed<Root> = serde_json::from_slice(&root_bytes).unwrap();
    let keyid = signed.signatures.first().unwrap().keyid.clone();
    signed.signed.version = NonZeroU64::new(version).unwrap();
    let canonical = signed.signed.canonical_form().unwrap();
    let key_source = LocalKeySource {
        path: fixture_path("test-key.pem"),
    };
    let signer = key_source.as_sign().await.unwrap();
    let signature = signer.sign(&canonical, &SystemRandom::new()).await.unwrap();
    signed.signatures = vec![Signature {
        keyid,
        sig: signature.into(),
    }];
    let mut rotated = serde_json::to_vec_pretty(&signed).unwrap();
    rotated.push(b'\n');
    Arc::from(rotated)
}

fn root_version(bytes: &[u8]) -> u64 {
    serde_json::from_slice::<Signed<Root>>(bytes)
        .unwrap()
        .signed
        .version
        .get()
}

fn replace_directory_contents(source: &Path, destination: &Path) {
    fs::remove_dir_all(destination).unwrap();
    fs::create_dir(destination).unwrap();
    for entry in fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        let metadata = entry.metadata().unwrap();
        assert!(metadata.is_file());
        fs::copy(entry.path(), destination.join(entry.file_name())).unwrap();
    }
}

fn fixture_path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(name)
}

fn hex_sha256(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}
