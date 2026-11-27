use std::collections::HashSet;
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

pub use crate::storage_io::StoreError;
use crate::storage_io::*;
use serde::{Deserialize, Serialize};
use zephium_blocker::Sha256;
use zephium_blocker::{BlockerCompileFailure, PolicyCatalog, StaticPolicyCatalog};

use crate::manifest::{decode_sha256, CatalogManifest};
use crate::repository::{VerifiedCatalog, VerifiedCatalogPayload};
use crate::types::{
    ActivatedCatalog, CatalogIdentity, GcBudget, RepositoryConfig, HARD_MAX_CACHE_OBJECTS,
    HARD_MAX_KNOWN_TIME_BYTES, HARD_MAX_TUF_OBJECTS,
};

const STATE_SCHEMA_VERSION: u32 = 4;
const JOURNAL_SCHEMA_VERSION: u32 = 1;
const CHECKPOINT_SCHEMA_VERSION: u32 = 1;
const CLOCK_SCHEMA_VERSION: u32 = 1;
const REFRESH_HINT_SCHEMA_VERSION: u32 = 1;
const MAX_STATE_BYTES: u64 = 64 * 1024;
const MAX_JOURNAL_BYTES: u64 = 64 * 1024;
const MAX_CLOCK_BYTES: u64 = 1024;
const MAX_REFRESH_HINT_BYTES: u64 = 1024;
const MAX_ROOT_ENTRIES: usize = 14;
const MAX_RECOVERY_JOURNALS: usize = 1024;
const MAX_TUF_FILES: usize = HARD_MAX_TUF_OBJECTS;
const MAX_TUF_STAGE_ENTRIES: usize = MAX_TUF_FILES * 2;
const MAX_CLOCK_UNIX: u64 = 7_258_118_400;
const DURABILITY_PROBE_BYTES: &[u8] = b"zephium-blocker-cache-durability-v1";
const TUF_FILE_NAMES: [&str; MAX_TUF_FILES] = [
    "root.json",
    "timestamp.json",
    "snapshot.json",
    "targets.json",
    "latest_known_time.json",
];

pub(crate) struct CatalogStore {
    _lock: crate::cache_lock::CacheLock,
    root: PathBuf,
    objects: PathBuf,
    journals: PathBuf,
    tuf_stage: PathBuf,
    config: RepositoryConfig,
    state: StoreState,
    clock_high_water: u64,
    last_refresh_attempt_unix: Option<u64>,
    pending_activation: Option<ActivatedCatalog>,
    pending_candidate: Option<ActivatedCatalog>,
    activation_sealed: bool,
}

impl CatalogStore {
    pub(crate) fn open(config: &RepositoryConfig) -> Result<Self, StoreError> {
        let root = config.storage_dir.clone();
        create_private_directory(&root)?;
        let cache_lock = crate::cache_lock::CacheLock::acquire(&root.join("lock-v1"))
            .ok_or(StoreError::LockUnavailable)?;
        let objects = create_child_directory(&root, "objects")?;
        let journals = create_child_directory(&root, "activation-journals")?;
        let tuf_stage = create_child_directory(&root, "tuf-stage")?;
        validate_root_namespace(&root)?;
        reset_tuf_stage(&tuf_stage, config)?;
        validate_object_namespace(&objects, config, CasNamespaceBounds::recovery(config)?)?;
        durability_probe(&root)?;
        let clock_high_water = read_clock_high_water(&root.join("clock-high-water.json"))?;
        let last_refresh_attempt_unix =
            read_refresh_attempt_hint(&root.join("last-refresh-attempt.json"), clock_high_water)?;

        let state_path = root.join("state.json");
        let (mut state, mut state_bytes) = read_state(&state_path)?;
        if state.generation > 0 && clock_high_water == 0 {
            return Err(StoreError::ClockInvalid);
        }
        validate_repository_identity(&state, config)?;
        let recovered = recover_journal_chain(
            &root,
            &objects,
            &journals,
            config,
            &mut state,
            &mut state_bytes,
        )?;
        validate_repository_identity(&state, config)?;
        validate_committed_tuf(&objects, state.tuf.as_ref(), config)?;
        let pending_activation = match (recovered, state.current.as_ref()) {
            (Some(catalog), _) => Some(catalog),
            (None, Some(stored)) => Some(load_package(&objects, stored, config)?),
            (None, None) => None,
        };
        let pending_candidate = state
            .candidate
            .as_ref()
            .map(|stored| load_package(&objects, stored, config))
            .transpose()?;

        // Only after exact CAS recovery succeeds may this process checkpoint
        // the state and retire prior-process journal records.
        write_checkpoint(&root, &state, &state_bytes)?;
        retire_checkpointed_journals(&journals, state.generation)?;

        let store = Self {
            _lock: cache_lock,
            root,
            objects,
            journals,
            tuf_stage,
            config: config.clone(),
            state,
            clock_high_water,
            last_refresh_attempt_unix,
            pending_activation,
            pending_candidate,
            activation_sealed: false,
        };
        store.enforce_steady_cas_namespace()?;
        Ok(store)
    }

    pub(crate) fn prepare_tuf_datastore(&mut self) -> Result<PreparedTufDatastore, StoreError> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| StoreError::ClockInvalid)?
            .as_secs();
        self.prepare_tuf_datastore_at(now)
    }

    pub(crate) fn admit_startup_clock(&mut self) -> Result<(), StoreError> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| StoreError::ClockInvalid)?
            .as_secs();
        self.admit_clock(now)
    }

    pub(crate) fn current_identity(&self) -> Option<CatalogIdentity> {
        self.state
            .current
            .as_ref()
            .map(|current| current.identity.clone())
    }

    pub(crate) fn classify_identity(
        &self,
        identity: &CatalogIdentity,
    ) -> Result<CatalogIdentityDisposition, StoreError> {
        let Some(highest) = &self.state.highest else {
            return Ok(CatalogIdentityDisposition::New);
        };
        if identity.revision < highest.revision {
            return Err(StoreError::Rollback);
        }
        if identity.revision > highest.revision {
            return Ok(CatalogIdentityDisposition::New);
        }
        if identity != highest {
            return Err(StoreError::Equivocation);
        }
        if self
            .state
            .current
            .as_ref()
            .is_some_and(|current| current.identity == *identity)
        {
            return Ok(CatalogIdentityDisposition::Current);
        }
        if self
            .state
            .candidate
            .as_ref()
            .is_some_and(|candidate| candidate.identity == *identity)
        {
            return Ok(CatalogIdentityDisposition::Candidate);
        }
        match self.state.rejection {
            Some(StoredRejection::Compiler { policy_fingerprint })
                if policy_fingerprint != zephium_blocker::compiler_policy_fingerprint() =>
            {
                Ok(CatalogIdentityDisposition::New)
            }
            Some(_) => Ok(CatalogIdentityDisposition::Rejected),
            None => Err(StoreError::StateCorrupt),
        }
    }

    pub(crate) fn take_pending_activation(&mut self) -> Option<ActivatedCatalog> {
        self.pending_activation.take()
    }

    pub(crate) fn take_pending_candidate(&mut self) -> Option<ActivatedCatalog> {
        self.pending_candidate.take()
    }

    /// Reads one exact content-addressed object after the same bounded
    /// no-follow identity and digest checks used during activation.
    ///
    /// Missing content is a normal cache miss. Any other failure means the
    /// durable namespace is not safe to use for this refresh.
    pub(crate) fn load_cached_object(
        &self,
        digest: &[u8; 32],
        max_bytes: u64,
    ) -> Result<Option<Vec<u8>>, StoreError> {
        match read_object(&self.objects, digest, max_bytes) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(StoreError::NotFound) => Ok(None),
            Err(error) => Err(error),
        }
    }

    pub(crate) fn validate_cached_object(
        &self,
        digest: &[u8; 32],
        exact_bytes: u64,
    ) -> Result<CachedObjectState, StoreError> {
        let path = self.objects.join(hex_digest(digest));
        let mut file = match crate::file_identity::open_verified_regular(&path) {
            Some(file) => file,
            None => match fs::symlink_metadata(&path) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    return Ok(CachedObjectState::Missing);
                }
                _ => return Err(StoreError::UnsafePath),
            },
        };
        let metadata = file.metadata().map_err(|_| StoreError::Io)?;
        if !metadata.is_file() || metadata.len() != exact_bytes {
            return Ok(CachedObjectState::Corrupt);
        }
        let mut hasher = Sha256::new();
        let mut observed = 0u64;
        let mut buffer = [0u8; 64 * 1024];
        loop {
            let read = file.read(&mut buffer).map_err(|_| StoreError::Io)?;
            if read == 0 {
                break;
            }
            observed = observed
                .checked_add(read as u64)
                .ok_or(StoreError::ObjectCorrupt)?;
            if observed > exact_bytes {
                return Ok(CachedObjectState::Corrupt);
            }
            hasher.update(&buffer[..read]);
        }
        if observed != exact_bytes || hasher.finalize().as_slice() != digest {
            return Ok(CachedObjectState::Corrupt);
        }
        Ok(CachedObjectState::Present)
    }

    pub(crate) fn last_refresh_attempt_unix(&self) -> Option<u64> {
        self.last_refresh_attempt_unix
    }

    #[allow(dead_code)]
    pub(crate) fn load_previous(&self) -> Result<Option<ActivatedCatalog>, StoreError> {
        self.state
            .previous
            .as_ref()
            .map(|stored| load_package(&self.objects, stored, &self.config))
            .transpose()
    }

    pub(crate) fn discard_tuf_stage(&self) -> Result<(), StoreError> {
        reset_tuf_stage(&self.tuf_stage, &self.config)
    }

    pub(crate) fn stage_candidate(
        &mut self,
        verified: VerifiedCatalog,
    ) -> Result<CandidateStageOutcome, StoreError> {
        self.stage_candidate_inner(verified, FaultPoint::None)
    }

    fn stage_candidate_inner(
        &mut self,
        verified: VerifiedCatalog,
        fault: FaultPoint,
    ) -> Result<CandidateStageOutcome, StoreError> {
        if self.activation_sealed {
            return Err(StoreError::ActivationSealed);
        }
        if self.state.candidate.is_some() {
            return Err(StoreError::CandidatePending);
        }
        if verified.repository_identity != self.config.repository_identity {
            return Err(StoreError::RepositoryMismatch);
        }
        self.admit_clock(verified.verified_at_unix)?;
        let identity = verified.identity().clone();
        let disposition = self.classify_identity(&identity)?;
        let payload_matches_state = matches!(
            (&verified.payload, disposition),
            (
                VerifiedCatalogPayload::Candidate { .. },
                CatalogIdentityDisposition::New
            ) | (
                VerifiedCatalogPayload::Unchanged { .. },
                CatalogIdentityDisposition::Current
            ) | (
                VerifiedCatalogPayload::RejectedUnchanged(_),
                CatalogIdentityDisposition::Rejected
            )
        );
        if !payload_matches_state {
            return Err(StoreError::StateCorrupt);
        }

        let tuf = ingest_tuf_stage(
            &self.tuf_stage,
            &self.objects,
            &self.config,
            verified.tuf_descriptor_sha256,
        )?;
        let manifest_bytes = verified
            .manifest
            .encode_canonical()
            .map_err(|_| StoreError::InvalidPackage)?;
        replace_authenticated_object(&self.objects, &identity.manifest_sha256, &manifest_bytes)?;
        if let VerifiedCatalogPayload::Candidate {
            source_contents, ..
        } = &verified.payload
        {
            for (descriptor, contents) in verified.manifest.sources.iter().zip(source_contents) {
                let digest =
                    decode_sha256(&descriptor.sha256).map_err(|_| StoreError::InvalidPackage)?;
                replace_authenticated_object(&self.objects, &digest, contents.as_bytes())?;
            }
        }
        if let VerifiedCatalogPayload::Unchanged {
            repaired_objects, ..
        } = &verified.payload
        {
            for (digest, bytes) in repaired_objects {
                replace_authenticated_object(&self.objects, digest, bytes)?;
            }
        }
        fault.fail_if(FaultPoint::AfterObjects)?;

        let candidate =
            matches!(&verified.payload, VerifiedCatalogPayload::Candidate { .. }).then(|| {
                StoredPackage {
                    identity: identity.clone(),
                    admission_policy_sha256: self.config.package_admission_policy_sha256(),
                }
            });
        let rejection = matches!(
            &verified.payload,
            VerifiedCatalogPayload::RejectedUnchanged(_)
        )
        .then_some(self.state.rejection)
        .flatten();
        let mut current = self.state.current.clone();
        if matches!(&verified.payload, VerifiedCatalogPayload::Unchanged { .. }) {
            let current = current.as_mut().ok_or(StoreError::StateCorrupt)?;
            if current.identity != identity {
                return Err(StoreError::StateCorrupt);
            }
            current.admission_policy_sha256 = self.config.package_admission_policy_sha256();
        }
        let next_state = StoreState {
            schema_version: STATE_SCHEMA_VERSION,
            generation: self
                .state
                .generation
                .checked_add(1)
                .ok_or(StoreError::StateCorrupt)?,
            repository_identity: Some(self.config.repository_identity),
            highest: Some(identity.clone()),
            current,
            previous: self.state.previous.clone(),
            candidate,
            rejection,
            tuf: Some(tuf),
        };
        self.persist_state(next_state, &identity, fault)?;
        let _ = self.discard_tuf_stage();
        let outcome = match verified.payload {
            VerifiedCatalogPayload::Candidate { activated, .. } => {
                self.pending_candidate = Some(activated.clone());
                CandidateStageOutcome::Candidate(activated)
            }
            VerifiedCatalogPayload::Unchanged { .. } => {
                self.pending_candidate = None;
                CandidateStageOutcome::Unchanged(identity)
            }
            VerifiedCatalogPayload::RejectedUnchanged(_) => {
                self.pending_candidate = None;
                CandidateStageOutcome::Rejected
            }
            VerifiedCatalogPayload::CandidateRepair { .. } => {
                return Err(StoreError::StateCorrupt);
            }
        };
        Ok(outcome)
    }

    pub(crate) fn repair_candidate(
        &mut self,
        identity: &CatalogIdentity,
        verified: VerifiedCatalog,
    ) -> Result<CandidateRepairStoreOutcome, StoreError> {
        self.repair_candidate_inner(identity, verified, FaultPoint::None)
    }

    fn repair_candidate_inner(
        &mut self,
        identity: &CatalogIdentity,
        verified: VerifiedCatalog,
        fault: FaultPoint,
    ) -> Result<CandidateRepairStoreOutcome, StoreError> {
        if self.activation_sealed {
            return Err(StoreError::ActivationSealed);
        }
        if verified.repository_identity != self.config.repository_identity {
            return Err(StoreError::RepositoryMismatch);
        }
        self.admit_clock(verified.verified_at_unix)?;
        if self.state.highest.as_ref() != Some(identity)
            || self
                .state
                .candidate
                .as_ref()
                .is_none_or(|candidate| &candidate.identity != identity)
        {
            return Err(StoreError::CandidateMismatch);
        }
        let verified_identity = verified.identity().clone();
        let disposition = self.classify_identity(&verified_identity)?;
        let (source_contents, superseding) = match (&verified.payload, disposition) {
            (
                VerifiedCatalogPayload::CandidateRepair {
                    identity: repaired_identity,
                    source_contents,
                },
                CatalogIdentityDisposition::Candidate,
            ) if repaired_identity == identity && verified_identity == *identity => {
                (source_contents, None)
            }
            (
                VerifiedCatalogPayload::Candidate {
                    activated,
                    source_contents,
                },
                CatalogIdentityDisposition::New,
            ) if verified_identity.revision > identity.revision
                && activated.identity == verified_identity =>
            {
                (source_contents, Some(activated.clone()))
            }
            _ => return Err(StoreError::CandidateMismatch),
        };
        if source_contents.len() != verified.manifest.sources.len() {
            return Err(StoreError::InvalidPackage);
        }

        let tuf = ingest_tuf_stage(
            &self.tuf_stage,
            &self.objects,
            &self.config,
            verified.tuf_descriptor_sha256,
        )?;
        let manifest_bytes = verified
            .manifest
            .encode_canonical()
            .map_err(|_| StoreError::InvalidPackage)?;
        replace_authenticated_object(
            &self.objects,
            &verified_identity.manifest_sha256,
            &manifest_bytes,
        )?;
        for (descriptor, contents) in verified.manifest.sources.iter().zip(source_contents) {
            let digest =
                decode_sha256(&descriptor.sha256).map_err(|_| StoreError::InvalidPackage)?;
            if contents.len() as u64 != descriptor.length {
                return Err(StoreError::InvalidPackage);
            }
            replace_authenticated_object(&self.objects, &digest, contents.as_bytes())?;
        }
        fault.fail_if(FaultPoint::AfterObjects)?;

        let mut next_state = self.state.clone();
        next_state.generation = next_state
            .generation
            .checked_add(1)
            .ok_or(StoreError::StateCorrupt)?;
        next_state.tuf = Some(tuf);
        if superseding.is_some() {
            next_state.highest = Some(verified_identity.clone());
            next_state.candidate = Some(StoredPackage {
                identity: verified_identity.clone(),
                admission_policy_sha256: self.config.package_admission_policy_sha256(),
            });
            next_state.rejection = None;
        } else {
            let candidate = next_state
                .candidate
                .as_mut()
                .ok_or(StoreError::StateCorrupt)?;
            if candidate.identity != *identity {
                return Err(StoreError::StateCorrupt);
            }
            candidate.admission_policy_sha256 = self.config.package_admission_policy_sha256();
        }
        // Exact repair changes authenticated cache material, TUF state, and
        // the candidate's admission-policy proof. Supersession changes
        // candidate and high-water together; current and previous authority
        // remain untouched.
        self.persist_state(next_state, &verified_identity, fault)?;
        let _ = self.discard_tuf_stage();
        match superseding {
            Some(candidate) => {
                self.pending_candidate = Some(candidate.clone());
                Ok(CandidateRepairStoreOutcome::Superseded(candidate))
            }
            None => Ok(CandidateRepairStoreOutcome::Repaired),
        }
    }

    pub(crate) fn commit_candidate(
        &mut self,
        identity: &CatalogIdentity,
    ) -> Result<CatalogIdentity, StoreError> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| StoreError::ClockInvalid)?
            .as_secs();
        self.commit_candidate_at(identity, now)
    }

    fn commit_candidate_at(
        &mut self,
        identity: &CatalogIdentity,
        now_unix: u64,
    ) -> Result<CatalogIdentity, StoreError> {
        self.commit_candidate_at_inner(identity, now_unix, FaultPoint::None)
    }

    fn commit_candidate_at_inner(
        &mut self,
        identity: &CatalogIdentity,
        now_unix: u64,
        fault: FaultPoint,
    ) -> Result<CatalogIdentity, StoreError> {
        if self.activation_sealed {
            return Err(StoreError::ActivationSealed);
        }
        self.admit_clock(now_unix)?;
        if identity.expires_unix <= now_unix {
            return Err(StoreError::CandidateExpired);
        }
        let candidate = self
            .state
            .candidate
            .as_ref()
            .filter(|candidate| &candidate.identity == identity)
            .cloned()
            .ok_or(StoreError::CandidateMismatch)?;
        let next_state = StoreState {
            schema_version: STATE_SCHEMA_VERSION,
            generation: self
                .state
                .generation
                .checked_add(1)
                .ok_or(StoreError::StateCorrupt)?,
            repository_identity: Some(self.config.repository_identity),
            highest: self.state.highest.clone(),
            current: Some(candidate),
            previous: self.state.current.clone(),
            candidate: None,
            rejection: None,
            tuf: self.state.tuf.clone(),
        };
        self.persist_state(next_state, identity, fault)?;
        self.pending_candidate = None;
        Ok(identity.clone())
    }

    pub(crate) fn reject_candidate(
        &mut self,
        identity: &CatalogIdentity,
        rejection: CandidateRejection,
    ) -> Result<(), StoreError> {
        self.reject_candidate_inner(identity, rejection, FaultPoint::None)
    }

    fn reject_candidate_inner(
        &mut self,
        identity: &CatalogIdentity,
        rejection: CandidateRejection,
        fault: FaultPoint,
    ) -> Result<(), StoreError> {
        if self.activation_sealed {
            return Err(StoreError::ActivationSealed);
        }
        let candidate = self
            .state
            .candidate
            .as_ref()
            .filter(|candidate| &candidate.identity == identity)
            .ok_or(StoreError::CandidateMismatch)?;
        if self.state.highest.as_ref() != Some(&candidate.identity) {
            return Err(StoreError::StateCorrupt);
        }
        let next_state = StoreState {
            schema_version: STATE_SCHEMA_VERSION,
            generation: self
                .state
                .generation
                .checked_add(1)
                .ok_or(StoreError::StateCorrupt)?,
            repository_identity: Some(self.config.repository_identity),
            highest: self.state.highest.clone(),
            current: self.state.current.clone(),
            previous: self.state.previous.clone(),
            candidate: None,
            rejection: Some(match rejection {
                CandidateRejection::Expired => StoredRejection::Expired,
                CandidateRejection::Compiler => StoredRejection::Compiler {
                    policy_fingerprint: zephium_blocker::compiler_policy_fingerprint(),
                },
            }),
            tuf: self.state.tuf.clone(),
        };
        self.persist_state(next_state, identity, fault)?;
        self.pending_candidate = None;
        Ok(())
    }

    fn persist_state(
        &mut self,
        next_state: StoreState,
        identity: &CatalogIdentity,
        fault: FaultPoint,
    ) -> Result<(), StoreError> {
        let previous_bytes = encode_canonical(&self.state)?;
        let next_bytes = encode_canonical(&next_state)?;
        let previous_sha256: [u8; 32] = Sha256::digest(&previous_bytes);
        let next_sha256: [u8; 32] = Sha256::digest(&next_bytes);
        let journal = ActivationJournal {
            schema_version: JOURNAL_SCHEMA_VERSION,
            generation: next_state.generation,
            previous_state_sha256: previous_sha256,
            next_state_sha256: next_sha256,
            revision: identity.revision,
            manifest_sha256: identity.manifest_sha256,
            next_state: next_state.clone(),
        };
        let journal_path = self.journals.join(format!(
            "{:020}-{}.json",
            next_state.generation,
            hex_digest(&identity.manifest_sha256)
        ));
        // Once a journal write begins, any error may mean it reached durable
        // storage. Seal this process until exact startup recovery resolves it.
        self.activation_sealed = true;
        atomic_write(&journal_path, &encode_canonical(&journal)?)?;
        fault.fail_if(FaultPoint::AfterJournal)?;

        atomic_write(&self.root.join("state.json"), &next_bytes)?;
        fault.fail_if(FaultPoint::AfterStateReplace)?;
        let persisted = read_bounded_regular(&self.root.join("state.json"), MAX_STATE_BYTES)?;
        if persisted != next_bytes || Sha256::digest(&persisted).as_slice() != next_sha256 {
            return Err(StoreError::StateCorrupt);
        }
        let parsed: StoreState = decode_canonical(&persisted)?;
        if parsed != next_state {
            return Err(StoreError::StateCorrupt);
        }
        write_checkpoint(&self.root, &next_state, &next_bytes)?;
        retire_checkpointed_journals(&self.journals, next_state.generation)?;

        self.state = next_state;
        self.enforce_steady_cas_namespace()?;
        self.activation_sealed = false;
        Ok(())
    }

    fn enforce_steady_cas_namespace(&self) -> Result<(), StoreError> {
        let recovery = CasNamespaceBounds::recovery(&self.config)?;
        let collection = self.collect_garbage_inner(GcBudget {
            max_entries_scanned: recovery.max_entries,
            max_objects_removed: recovery.max_entries,
            max_bytes_removed: recovery.max_bytes,
        })?;
        debug_assert!(collection.report.objects_removed <= recovery.max_entries);
        let bounds = if collection.references_complete {
            CasNamespaceBounds::steady(&self.config)?
        } else {
            recovery
        };
        validate_object_namespace(&self.objects, &self.config, bounds)
    }

    #[cfg(test)]
    pub(crate) fn collect_garbage(
        &self,
        budget: GcBudget,
    ) -> Result<GarbageCollectionReport, StoreError> {
        self.collect_garbage_inner(budget)
            .map(|collection| collection.report)
    }

    fn collect_garbage_inner(&self, budget: GcBudget) -> Result<GarbageCollectionPass, StoreError> {
        let mut protected = HashSet::new();
        let mut references_complete = true;
        if let Some(current) = &self.state.current {
            references_complete &=
                collect_package_digests(&self.objects, current, &self.config, &mut protected)?;
        }
        if let Some(previous) = &self.state.previous {
            references_complete &=
                collect_package_digests(&self.objects, previous, &self.config, &mut protected)?;
        }
        if let Some(candidate) = &self.state.candidate {
            references_complete &=
                collect_package_digests(&self.objects, candidate, &self.config, &mut protected)?;
        }
        if let Some(tuf) = &self.state.tuf {
            protected.extend(tuf.files.iter().map(|file| file.sha256));
        }
        if !references_complete {
            return Ok(GarbageCollectionPass {
                report: GarbageCollectionReport::default(),
                references_complete: false,
            });
        }

        let mut report = GarbageCollectionReport::default();
        let entries = fs::read_dir(&self.objects).map_err(|_| StoreError::Io)?;
        for entry in entries.take(budget.max_entries_scanned) {
            report.entries_scanned += 1;
            let entry = entry.map_err(|_| StoreError::Io)?;
            let name = match entry.file_name().to_str() {
                Some(name) => name.to_owned(),
                None => return Err(StoreError::ObjectCorrupt),
            };
            let (digest, stage) = if let Some(digest) = parse_digest_filename(&name) {
                (digest, false)
            } else if let Some(digest) = parse_object_stage_filename(&name) {
                (digest, true)
            } else {
                return Err(StoreError::ObjectCorrupt);
            };
            if (!stage && protected.contains(&digest))
                || report.objects_removed >= budget.max_objects_removed
            {
                continue;
            }
            let file = crate::file_identity::open_verified_regular(&entry.path())
                .ok_or(StoreError::UnsafePath)?;
            let metadata = file.metadata().map_err(|_| StoreError::Io)?;
            drop(file);
            let next_bytes = report.bytes_removed.saturating_add(metadata.len());
            if next_bytes > budget.max_bytes_removed {
                continue;
            }
            fs::remove_file(entry.path()).map_err(|_| StoreError::Io)?;
            report.objects_removed += 1;
            report.bytes_removed = next_bytes;
        }
        if report.objects_removed > 0 {
            sync_directory(&self.objects)?;
        }
        Ok(GarbageCollectionPass {
            report,
            references_complete: true,
        })
    }

    #[cfg(test)]
    pub(crate) fn stage_candidate_with_fault(
        &mut self,
        verified: VerifiedCatalog,
        fault: FaultPoint,
    ) -> Result<CandidateStageOutcome, StoreError> {
        self.stage_candidate_inner(verified, fault)
    }

    #[cfg(test)]
    pub(crate) fn commit_candidate_at_for_test(
        &mut self,
        identity: &CatalogIdentity,
        now_unix: u64,
    ) -> Result<CatalogIdentity, StoreError> {
        self.commit_candidate_at(identity, now_unix)
    }

    #[cfg(test)]
    pub(crate) fn commit_candidate_with_fault(
        &mut self,
        identity: &CatalogIdentity,
        now_unix: u64,
        fault: FaultPoint,
    ) -> Result<CatalogIdentity, StoreError> {
        self.commit_candidate_at_inner(identity, now_unix, fault)
    }

    #[cfg(test)]
    pub(crate) fn reject_candidate_with_fault(
        &mut self,
        identity: &CatalogIdentity,
        rejection: CandidateRejection,
        fault: FaultPoint,
    ) -> Result<(), StoreError> {
        self.reject_candidate_inner(identity, rejection, fault)
    }

    #[cfg(test)]
    pub(crate) fn repair_candidate_with_fault(
        &mut self,
        identity: &CatalogIdentity,
        verified: VerifiedCatalog,
        fault: FaultPoint,
    ) -> Result<CandidateRepairStoreOutcome, StoreError> {
        self.repair_candidate_inner(identity, verified, fault)
    }

    pub(crate) fn prepare_tuf_datastore_at(
        &mut self,
        now_unix: u64,
    ) -> Result<PreparedTufDatastore, StoreError> {
        self.admit_clock(now_unix)?;
        self.record_refresh_attempt(now_unix)?;
        reset_tuf_stage(&self.tuf_stage, &self.config)?;
        if let Some(tuf) = &self.state.tuf {
            materialize_tuf_snapshot(&self.objects, &self.tuf_stage, tuf, &self.config)?;
        }
        let trusted_root = match &self.state.tuf {
            Some(tuf) => {
                let root = tuf.files.first().ok_or(StoreError::StateCorrupt)?;
                if root.name != "root.json" {
                    return Err(StoreError::StateCorrupt);
                }
                Arc::<[u8]>::from(read_object(&self.objects, &root.sha256, root.length)?)
            }
            None => Arc::clone(&self.config.trusted_root),
        };
        Ok(PreparedTufDatastore {
            path: self.tuf_stage.clone(),
            trusted_root,
            repository_identity: self.config.repository_identity,
            attempted_unix: now_unix,
        })
    }

    #[cfg(test)]
    pub(crate) fn repository_identity(&self) -> [u8; 32] {
        self.config.repository_identity
    }

    #[cfg(test)]
    pub(crate) fn generation(&self) -> u64 {
        self.state.generation
    }

    #[cfg(test)]
    pub(crate) fn staged_tuf_seal(&self) -> [u8; 32] {
        seal_tuf_stage(&self.tuf_stage, &self.config).expect("test TUF stage must be complete")
    }

    fn admit_clock(&mut self, now_unix: u64) -> Result<(), StoreError> {
        if now_unix > MAX_CLOCK_UNIX || self.clock_high_water > MAX_CLOCK_UNIX {
            return Err(StoreError::ClockInvalid);
        }
        if now_unix < self.clock_high_water {
            return Err(StoreError::ClockRollback);
        }
        if now_unix == self.clock_high_water {
            return Ok(());
        }
        let high_water = ClockHighWater {
            schema_version: CLOCK_SCHEMA_VERSION,
            observed_unix: now_unix,
        };
        atomic_write(
            &self.root.join("clock-high-water.json"),
            &encode_canonical(&high_water)?,
        )?;
        self.clock_high_water = now_unix;
        Ok(())
    }

    fn record_refresh_attempt(&mut self, now_unix: u64) -> Result<(), StoreError> {
        if now_unix > self.clock_high_water || now_unix > MAX_CLOCK_UNIX {
            return Err(StoreError::ClockInvalid);
        }
        let hint = RefreshAttemptHint {
            schema_version: REFRESH_HINT_SCHEMA_VERSION,
            attempted_unix: now_unix,
        };
        atomic_write(
            &self.root.join("last-refresh-attempt.json"),
            &encode_canonical(&hint)?,
        )?;
        self.last_refresh_attempt_unix = Some(now_unix);
        Ok(())
    }
}

pub(crate) enum CandidateStageOutcome {
    Candidate(ActivatedCatalog),
    Unchanged(CatalogIdentity),
    Rejected,
}

pub(crate) enum CandidateRepairStoreOutcome {
    Repaired,
    Superseded(ActivatedCatalog),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CatalogIdentityDisposition {
    New,
    Current,
    Candidate,
    Rejected,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CachedObjectState {
    Present,
    Missing,
    Corrupt,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CandidateRejection {
    Expired,
    Compiler,
}

pub(crate) struct PreparedTufDatastore {
    pub(crate) path: PathBuf,
    pub(crate) trusted_root: Arc<[u8]>,
    pub(crate) repository_identity: [u8; 32],
    pub(crate) attempted_unix: u64,
}

/// Bounded cache-collection result.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct GarbageCollectionReport {
    /// Directory entries inspected.
    pub entries_scanned: usize,
    /// Unreferenced objects removed.
    pub objects_removed: usize,
    /// Unreferenced bytes removed.
    pub bytes_removed: u64,
}

struct GarbageCollectionPass {
    report: GarbageCollectionReport,
    references_complete: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct StoreState {
    schema_version: u32,
    generation: u64,
    repository_identity: Option<[u8; 32]>,
    highest: Option<CatalogIdentity>,
    current: Option<StoredPackage>,
    previous: Option<StoredPackage>,
    candidate: Option<StoredPackage>,
    rejection: Option<StoredRejection>,
    tuf: Option<StoredTufSnapshot>,
}

impl Default for StoreState {
    fn default() -> Self {
        Self {
            schema_version: STATE_SCHEMA_VERSION,
            generation: 0,
            repository_identity: None,
            highest: None,
            current: None,
            previous: None,
            candidate: None,
            rejection: None,
            tuf: None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct StoredPackage {
    identity: CatalogIdentity,
    admission_policy_sha256: [u8; 32],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
enum StoredRejection {
    Expired,
    Compiler { policy_fingerprint: [u8; 32] },
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct StoredTufSnapshot {
    files: Vec<StoredTufFile>,
    aggregate_bytes: u64,
    descriptor_sha256: [u8; 32],
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct StoredTufFile {
    name: String,
    length: u64,
    sha256: [u8; 32],
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ClockHighWater {
    schema_version: u32,
    observed_unix: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct RefreshAttemptHint {
    schema_version: u32,
    attempted_unix: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ActivationJournal {
    schema_version: u32,
    generation: u64,
    previous_state_sha256: [u8; 32],
    next_state_sha256: [u8; 32],
    revision: u64,
    manifest_sha256: [u8; 32],
    next_state: StoreState,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct RecoveryCheckpoint {
    schema_version: u32,
    generation: u64,
    state_sha256: [u8; 32],
}

fn read_state(path: &Path) -> Result<(StoreState, Vec<u8>), StoreError> {
    match read_bounded_regular(path, MAX_STATE_BYTES) {
        Ok(bytes) => {
            let state: StoreState = decode_canonical(&bytes)?;
            validate_state(&state)?;
            Ok((state, bytes))
        }
        Err(StoreError::NotFound) => {
            let state = StoreState::default();
            let bytes = encode_canonical(&state)?;
            Ok((state, bytes))
        }
        Err(error) => Err(error),
    }
}

fn validate_state(state: &StoreState) -> Result<(), StoreError> {
    if state.schema_version != STATE_SCHEMA_VERSION {
        return Err(StoreError::StateCorrupt);
    }
    if state.generation == 0 {
        if state.repository_identity.is_some()
            || state.highest.is_some()
            || state.current.is_some()
            || state.previous.is_some()
            || state.candidate.is_some()
            || state.rejection.is_some()
            || state.tuf.is_some()
        {
            return Err(StoreError::StateCorrupt);
        }
        return Ok(());
    }
    let tuf = state.tuf.as_ref().ok_or(StoreError::StateCorrupt)?;
    let highest = state.highest.as_ref().ok_or(StoreError::StateCorrupt)?;
    if state.repository_identity.is_none() || validate_tuf_descriptor(tuf).is_err() {
        return Err(StoreError::StateCorrupt);
    }
    if state
        .candidate
        .as_ref()
        .is_some_and(|candidate| candidate.identity != *highest)
    {
        return Err(StoreError::StateCorrupt);
    }
    let high_water_has_package = state
        .candidate
        .as_ref()
        .or(state.current.as_ref())
        .is_some_and(|package| package.identity == *highest);
    if high_water_has_package == state.rejection.is_some() {
        return Err(StoreError::StateCorrupt);
    }
    if state.current.as_ref().is_some_and(|current| {
        current.identity.revision > highest.revision
            || (current.identity.revision == highest.revision && current.identity != *highest)
    }) {
        return Err(StoreError::StateCorrupt);
    }
    if state.candidate.as_ref().is_some_and(|candidate| {
        state
            .current
            .as_ref()
            .is_some_and(|current| candidate.identity.revision <= current.identity.revision)
    }) {
        return Err(StoreError::StateCorrupt);
    }
    if state.previous.as_ref().is_some_and(|previous| {
        state
            .current
            .as_ref()
            .is_none_or(|current| previous.identity.revision >= current.identity.revision)
    }) {
        return Err(StoreError::StateCorrupt);
    }
    Ok(())
}

fn validate_repository_identity(
    state: &StoreState,
    config: &RepositoryConfig,
) -> Result<(), StoreError> {
    if state
        .repository_identity
        .is_some_and(|identity| identity != config.repository_identity)
    {
        return Err(StoreError::RepositoryMismatch);
    }
    Ok(())
}

fn recover_journal_chain(
    root: &Path,
    objects: &Path,
    journals_dir: &Path,
    config: &RepositoryConfig,
    state: &mut StoreState,
    state_bytes: &mut Vec<u8>,
) -> Result<Option<ActivatedCatalog>, StoreError> {
    let mut state_sha256: [u8; 32] = Sha256::digest(&*state_bytes);
    let checkpoint = read_checkpoint(&root.join("recovery-checkpoint.json"))?;
    if checkpoint.generation > state.generation
        || (checkpoint.generation == state.generation && checkpoint.state_sha256 != state_sha256)
    {
        return Err(StoreError::JournalAmbiguous);
    }

    let mut journals = Vec::new();
    let mut entries_seen = 0usize;
    let mut removed_stage = false;
    for entry in fs::read_dir(journals_dir).map_err(|_| StoreError::Io)? {
        entries_seen = entries_seen
            .checked_add(1)
            .ok_or(StoreError::JournalAmbiguous)?;
        if entries_seen > MAX_RECOVERY_JOURNALS {
            return Err(StoreError::JournalAmbiguous);
        }
        let entry = entry.map_err(|_| StoreError::Io)?;
        let name = entry
            .file_name()
            .to_str()
            .ok_or(StoreError::JournalAmbiguous)?
            .to_owned();
        if parse_journal_stage_filename(&name).is_some() {
            let file = crate::file_identity::open_verified_regular(&entry.path())
                .ok_or(StoreError::JournalAmbiguous)?;
            drop(file);
            fs::remove_file(entry.path()).map_err(|_| StoreError::Io)?;
            removed_stage = true;
            continue;
        }
        let (name_generation, name_digest) =
            parse_journal_filename(&name).ok_or(StoreError::JournalAmbiguous)?;
        let metadata = entry
            .path()
            .symlink_metadata()
            .map_err(|_| StoreError::Io)?;
        if !metadata.is_file()
            || metadata.file_type().is_symlink()
            || metadata.len() > MAX_JOURNAL_BYTES
        {
            return Err(StoreError::JournalAmbiguous);
        }
        let bytes = read_bounded_regular(&entry.path(), MAX_JOURNAL_BYTES)?;
        let journal: ActivationJournal = decode_canonical(&bytes)?;
        let encoded_next = encode_canonical(&journal.next_state)?;
        if journal.schema_version != JOURNAL_SCHEMA_VERSION
            || journal.next_state.generation != journal.generation
            || journal.next_state.highest.as_ref().is_none_or(|highest| {
                highest.revision != journal.revision
                    || highest.manifest_sha256 != journal.manifest_sha256
            })
            || journal.generation != name_generation
            || journal.manifest_sha256 != name_digest
            || Sha256::digest(&encoded_next).as_slice() != journal.next_state_sha256
            || validate_state(&journal.next_state).is_err()
        {
            return Err(StoreError::JournalAmbiguous);
        }
        validate_repository_identity(&journal.next_state, config)?;
        if journal.generation > checkpoint.generation {
            journals.push(journal);
        }
    }
    if removed_stage {
        sync_directory(journals_dir)?;
    }
    journals.sort_by_key(|journal| journal.generation);

    let mut expected_generation = checkpoint.generation;
    let mut expected_sha = checkpoint.state_sha256;
    let mut matched_state = checkpoint.generation == state.generation;
    let mut prepared = None;
    for (index, journal) in journals.iter().enumerate() {
        if journal.generation
            != expected_generation
                .checked_add(1)
                .ok_or(StoreError::JournalAmbiguous)?
            || journal.previous_state_sha256 != expected_sha
        {
            return Err(StoreError::JournalAmbiguous);
        }
        expected_generation = journal.generation;
        expected_sha = journal.next_state_sha256;
        if journal.generation == state.generation {
            if journal.next_state_sha256 != state_sha256
                || state.highest.as_ref().is_none_or(|highest| {
                    highest.revision != journal.revision
                        || highest.manifest_sha256 != journal.manifest_sha256
                })
            {
                return Err(StoreError::JournalAmbiguous);
            }
            matched_state = true;
        } else if journal.generation > state.generation {
            // At most one journal can be prepared but not reflected in state:
            // the single worker cannot start the next activation first.
            if journal.generation
                != state
                    .generation
                    .checked_add(1)
                    .ok_or(StoreError::JournalAmbiguous)?
                || journal.previous_state_sha256 != state_sha256
                || index + 1 != journals.len()
            {
                return Err(StoreError::JournalAmbiguous);
            }
            prepared = Some(journal);
        }
    }
    if !matched_state {
        return Err(StoreError::JournalAmbiguous);
    }
    let Some(prepared) = prepared else {
        return Ok(None);
    };

    // Objects are made durable before the journal. If state replacement was
    // lost or reverted, finish the exact journal-authorized activation rather
    // than forgetting its higher revision.
    let package_at_high_water = prepared
        .next_state
        .candidate
        .as_ref()
        .or(prepared.next_state.current.as_ref())
        .filter(|package| prepared.next_state.highest.as_ref() == Some(&package.identity));
    if let Some(package) = package_at_high_water {
        let _ = load_validated_manifest(objects, package, config)?;
    }
    validate_committed_tuf(objects, prepared.next_state.tuf.as_ref(), config)?;
    let next_bytes = encode_canonical(&prepared.next_state)?;
    atomic_write(&root.join("state.json"), &next_bytes)?;
    let persisted = read_bounded_regular(&root.join("state.json"), MAX_STATE_BYTES)?;
    if persisted != next_bytes
        || Sha256::digest(&persisted).as_slice() != prepared.next_state_sha256
    {
        return Err(StoreError::JournalAmbiguous);
    }
    *state = prepared.next_state.clone();
    *state_bytes = next_bytes;
    state_sha256 = prepared.next_state_sha256;
    let recovered_sha256: [u8; 32] = Sha256::digest(&*state_bytes);
    if state_sha256 != recovered_sha256 {
        return Err(StoreError::JournalAmbiguous);
    }
    state
        .current
        .as_ref()
        .map(|current| load_package(objects, current, config))
        .transpose()
}

fn read_checkpoint(path: &Path) -> Result<RecoveryCheckpoint, StoreError> {
    match read_bounded_regular(path, MAX_JOURNAL_BYTES) {
        Ok(bytes) => {
            let checkpoint: RecoveryCheckpoint = decode_canonical(&bytes)?;
            if checkpoint.schema_version != CHECKPOINT_SCHEMA_VERSION {
                return Err(StoreError::JournalAmbiguous);
            }
            Ok(checkpoint)
        }
        Err(StoreError::NotFound) => {
            let state = StoreState::default();
            let bytes = encode_canonical(&state)?;
            Ok(RecoveryCheckpoint {
                schema_version: CHECKPOINT_SCHEMA_VERSION,
                generation: 0,
                state_sha256: Sha256::digest(bytes),
            })
        }
        Err(error) => Err(error),
    }
}

fn write_checkpoint(root: &Path, state: &StoreState, state_bytes: &[u8]) -> Result<(), StoreError> {
    let checkpoint = RecoveryCheckpoint {
        schema_version: CHECKPOINT_SCHEMA_VERSION,
        generation: state.generation,
        state_sha256: Sha256::digest(state_bytes),
    };
    atomic_write(
        &root.join("recovery-checkpoint.json"),
        &encode_canonical(&checkpoint)?,
    )
}

fn retire_checkpointed_journals(
    journals_dir: &Path,
    checkpoint_generation: u64,
) -> Result<(), StoreError> {
    let mut removed = false;
    let mut entries_seen = 0usize;
    for entry in fs::read_dir(journals_dir).map_err(|_| StoreError::Io)? {
        entries_seen = entries_seen
            .checked_add(1)
            .ok_or(StoreError::JournalAmbiguous)?;
        if entries_seen > MAX_RECOVERY_JOURNALS {
            return Err(StoreError::JournalAmbiguous);
        }
        let entry = entry.map_err(|_| StoreError::Io)?;
        let name = entry
            .file_name()
            .to_str()
            .ok_or(StoreError::JournalAmbiguous)?
            .to_owned();
        let (name_generation, name_digest) =
            parse_journal_filename(&name).ok_or(StoreError::JournalAmbiguous)?;
        let bytes = read_bounded_regular(&entry.path(), MAX_JOURNAL_BYTES)?;
        let journal: ActivationJournal = decode_canonical(&bytes)?;
        if journal.generation != name_generation || journal.manifest_sha256 != name_digest {
            return Err(StoreError::JournalAmbiguous);
        }
        if journal.generation <= checkpoint_generation {
            fs::remove_file(entry.path()).map_err(|_| StoreError::Io)?;
            removed = true;
        }
    }
    if removed {
        sync_directory(journals_dir)?;
    }
    Ok(())
}

fn load_package(
    objects: &Path,
    stored: &StoredPackage,
    config: &RepositoryConfig,
) -> Result<ActivatedCatalog, StoreError> {
    match load_validated_manifest(objects, stored, config) {
        Ok(_) => {}
        Err(StoreError::NotFound | StoreError::ObjectCorrupt)
            if stored.admission_policy_sha256 == config.package_admission_policy_sha256() => {}
        Err(StoreError::NotFound | StoreError::ObjectCorrupt) => {
            return Err(StoreError::InvalidPackage);
        }
        Err(error) => return Err(error),
    }
    // State holds the authenticated manifest identity and the exact package
    // admission-policy fingerprint. A missing or corrupt regular CAS object
    // under that same policy is recoverable source material: keep the package
    // deferred so compilation can request one authenticated refresh. Unsafe
    // filesystem identities, changed admission policy, and semantically
    // invalid signed packages still make startup unavailable.
    let objects = objects.to_path_buf();
    let stored_for_load = stored.clone();
    let config = config.clone();
    let manifest_sha256 = stored.identity.manifest_sha256;
    Ok(ActivatedCatalog {
        identity: stored.identity.clone(),
        catalog: PolicyCatalog::deferred(manifest_sha256, move || {
            load_static_catalog(&objects, &stored_for_load, &config)
                .map_err(map_deferred_catalog_error)
        }),
    })
}

fn load_static_catalog(
    objects: &Path,
    stored: &StoredPackage,
    config: &RepositoryConfig,
) -> Result<StaticPolicyCatalog, StoreError> {
    let manifest = load_validated_manifest(objects, stored, config)?;
    let mut source_contents = Vec::with_capacity(manifest.sources.len());
    let mut total = 0u64;
    for source in &manifest.sources {
        let digest = decode_sha256(&source.sha256).map_err(|_| StoreError::InvalidPackage)?;
        let bytes = read_deferred_object(objects, &digest, source.length)?;
        if bytes.len() as u64 != source.length {
            return Err(StoreError::InvalidPackage);
        }
        total = total
            .checked_add(source.length)
            .ok_or(StoreError::InvalidPackage)?;
        let contents = String::from_utf8(bytes).map_err(|_| StoreError::InvalidPackage)?;
        source_contents.push(Arc::<str>::from(contents));
    }
    if total != stored.identity.source_bytes {
        return Err(StoreError::InvalidPackage);
    }
    manifest
        .build_verified_catalog(&source_contents)
        .map_err(|_| StoreError::InvalidPackage)
}

fn load_validated_manifest(
    objects: &Path,
    stored: &StoredPackage,
    config: &RepositoryConfig,
) -> Result<CatalogManifest, StoreError> {
    let manifest_bytes = read_deferred_object(
        objects,
        &stored.identity.manifest_sha256,
        config.limits.max_manifest_bytes as u64,
    )?;
    let manifest =
        CatalogManifest::parse_cached(&manifest_bytes, config.limits, &config.license_policy)
            .map_err(|_| StoreError::InvalidPackage)?;
    if manifest.revision != stored.identity.revision
        || manifest.created_unix != stored.identity.created_unix
        || manifest.expires_unix != stored.identity.expires_unix
        || manifest.sources.len() as u32 != stored.identity.source_count
        || manifest
            .sources
            .iter()
            .try_fold(0u64, |total, source| total.checked_add(source.length))
            != Some(stored.identity.source_bytes)
    {
        return Err(StoreError::InvalidPackage);
    }
    Ok(manifest)
}

fn map_deferred_catalog_error(error: StoreError) -> BlockerCompileFailure {
    match error {
        // Unsafe identities enter the same bounded authenticated-repair
        // coordination path, but the store will classify them as terminal
        // Storage and will never replace them.
        StoreError::NotFound
        | StoreError::Io
        | StoreError::ObjectCorrupt
        | StoreError::UnsafePath => BlockerCompileFailure::SourceUnavailable,
        StoreError::InvalidPackage => BlockerCompileFailure::InvalidSource,
        _ => BlockerCompileFailure::Internal,
    }
}

fn read_deferred_object(
    directory: &Path,
    digest: &[u8; 32],
    exact_bytes: u64,
) -> Result<Vec<u8>, StoreError> {
    match read_object(directory, digest, exact_bytes) {
        Err(StoreError::UnsafePath)
            if crate::file_identity::open_verified_regular(&directory.join(hex_digest(digest)))
                .is_some() =>
        {
            // A verified regular object over its signed length is corrupt
            // cache material, not a compiler-invalid signed package. Unsafe
            // identities (symlinks, links, directories, replacement races)
            // still remain fail-closed as `UnsafePath`.
            Err(StoreError::ObjectCorrupt)
        }
        result => result,
    }
}

fn collect_package_digests(
    objects: &Path,
    stored: &StoredPackage,
    config: &RepositoryConfig,
    output: &mut HashSet<[u8; 32]>,
) -> Result<bool, StoreError> {
    output.insert(stored.identity.manifest_sha256);
    let manifest = match load_validated_manifest(objects, stored, config) {
        Ok(manifest) => manifest,
        Err(StoreError::NotFound | StoreError::ObjectCorrupt)
            if stored.admission_policy_sha256 == config.package_admission_policy_sha256() =>
        {
            return Ok(false);
        }
        Err(error) => return Err(error),
    };
    for source in manifest.sources {
        output.insert(decode_sha256(&source.sha256).map_err(|_| StoreError::InvalidPackage)?);
    }
    Ok(true)
}

fn write_object(directory: &Path, digest: &[u8; 32], bytes: &[u8]) -> Result<(), StoreError> {
    if Sha256::digest(bytes).as_slice() != digest {
        return Err(StoreError::InvalidPackage);
    }
    let path = directory.join(hex_digest(digest));
    match read_object(directory, digest, bytes.len() as u64) {
        Ok(existing) if existing == bytes => return Ok(()),
        Ok(_) => return Err(StoreError::ObjectCorrupt),
        Err(StoreError::NotFound) => {}
        Err(error) => return Err(error),
    }
    atomic_write(&path, bytes)?;
    let persisted = read_object(directory, digest, bytes.len() as u64)?;
    if persisted != bytes {
        return Err(StoreError::ObjectCorrupt);
    }
    Ok(())
}

// Callers reach this only after the same refresh has authenticated the exact
// digest and length through the sealed TUF datastore. The final digest check
// is repeated here, while an unsafe filesystem identity remains terminal.
fn replace_authenticated_object(
    directory: &Path,
    digest: &[u8; 32],
    bytes: &[u8],
) -> Result<(), StoreError> {
    if Sha256::digest(bytes).as_slice() != digest {
        return Err(StoreError::InvalidPackage);
    }
    let path = directory.join(hex_digest(digest));
    match read_object(directory, digest, bytes.len() as u64) {
        Ok(existing) if existing == bytes => return Ok(()),
        Ok(_) => return Err(StoreError::ObjectCorrupt),
        Err(StoreError::NotFound) => return write_object(directory, digest, bytes),
        Err(StoreError::ObjectCorrupt | StoreError::UnsafePath) => {
            if crate::file_identity::open_verified_regular(&path).is_none() {
                return Err(StoreError::UnsafePath);
            }
        }
        Err(error) => return Err(error),
    }
    atomic_write(&path, bytes)?;
    let persisted = read_object(directory, digest, bytes.len() as u64)?;
    if persisted != bytes {
        return Err(StoreError::ObjectCorrupt);
    }
    Ok(())
}

fn read_object(directory: &Path, digest: &[u8; 32], max_bytes: u64) -> Result<Vec<u8>, StoreError> {
    let bytes = read_bounded_regular(&directory.join(hex_digest(digest)), max_bytes)?;
    if Sha256::digest(&bytes).as_slice() != digest {
        return Err(StoreError::ObjectCorrupt);
    }
    Ok(bytes)
}

fn durability_probe(root: &Path) -> Result<(), StoreError> {
    let path = root.join("durability-v1");
    match read_bounded_regular(&path, 128) {
        Ok(bytes) if bytes == DURABILITY_PROBE_BYTES => Ok(()),
        Ok(_) => Err(StoreError::DurabilityPrimitiveUnavailable),
        Err(StoreError::NotFound) => {
            atomic_write(&path, DURABILITY_PROBE_BYTES)?;
            let bytes = read_bounded_regular(&path, 128)?;
            if bytes != DURABILITY_PROBE_BYTES {
                return Err(StoreError::DurabilityPrimitiveUnavailable);
            }
            Ok(())
        }
        Err(error) => Err(error),
    }
}

fn read_clock_high_water(path: &Path) -> Result<u64, StoreError> {
    match read_bounded_regular(path, MAX_CLOCK_BYTES) {
        Ok(bytes) => {
            let high_water: ClockHighWater =
                decode_canonical(&bytes).map_err(|_| StoreError::ClockInvalid)?;
            if high_water.schema_version != CLOCK_SCHEMA_VERSION
                || high_water.observed_unix > MAX_CLOCK_UNIX
            {
                return Err(StoreError::ClockInvalid);
            }
            Ok(high_water.observed_unix)
        }
        Err(StoreError::NotFound) => Ok(0),
        Err(error) => Err(error),
    }
}

fn read_refresh_attempt_hint(
    path: &Path,
    clock_high_water: u64,
) -> Result<Option<u64>, StoreError> {
    let file = match crate::file_identity::open_verified_regular(path) {
        Some(file) => file,
        None => {
            return match fs::symlink_metadata(path) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
                _ => Err(StoreError::UnsafePath),
            };
        }
    };
    let metadata = file.metadata().map_err(|_| StoreError::Io)?;
    drop(file);
    if metadata.len() > MAX_REFRESH_HINT_BYTES {
        return Ok(None);
    }
    let bytes = read_bounded_regular(path, MAX_REFRESH_HINT_BYTES)?;
    let Ok(hint) = serde_json::from_slice::<RefreshAttemptHint>(&bytes) else {
        return Ok(None);
    };
    let canonical = encode_canonical(&hint)?;
    if canonical != bytes
        || hint.schema_version != REFRESH_HINT_SCHEMA_VERSION
        || hint.attempted_unix > clock_high_water
        || hint.attempted_unix > MAX_CLOCK_UNIX
    {
        return Ok(None);
    }
    Ok(Some(hint.attempted_unix))
}

fn validate_root_namespace(path: &Path) -> Result<(), StoreError> {
    validate_private_directory(path)?;
    let mut entries = 0usize;
    let mut removed_stage = false;
    for entry in fs::read_dir(path).map_err(|_| StoreError::Io)? {
        entries = entries.checked_add(1).ok_or(StoreError::UnsafePath)?;
        if entries > MAX_ROOT_ENTRIES {
            return Err(StoreError::UnsafePath);
        }
        let entry = entry.map_err(|_| StoreError::Io)?;
        let name = entry
            .file_name()
            .to_str()
            .ok_or(StoreError::UnsafePath)?
            .to_owned();
        match name.as_str() {
            "objects" | "activation-journals" | "tuf-stage" => {
                validate_private_directory(&entry.path())?;
            }
            "lock-v1" => {
                let file = validate_bounded_regular(&entry.path(), 0)?;
                if file.metadata().map_err(|_| StoreError::Io)?.len() != 0 {
                    return Err(StoreError::UnsafePath);
                }
            }
            "durability-v1" => {
                validate_bounded_regular(&entry.path(), 128)?;
            }
            "clock-high-water.json" => {
                validate_bounded_regular(&entry.path(), MAX_CLOCK_BYTES)?;
            }
            "last-refresh-attempt.json" => {
                let file = crate::file_identity::open_verified_regular(&entry.path())
                    .ok_or(StoreError::UnsafePath)?;
                drop(file);
            }
            "state.json" => {
                validate_bounded_regular(&entry.path(), MAX_STATE_BYTES)?;
            }
            "recovery-checkpoint.json" => {
                validate_bounded_regular(&entry.path(), MAX_JOURNAL_BYTES)?;
            }
            ".durability-v1.stage"
            | ".clock-high-water.json.stage"
            | ".last-refresh-attempt.json.stage"
            | ".state.json.stage"
            | ".recovery-checkpoint.json.stage" => {
                remove_verified_regular(&entry.path())?;
                removed_stage = true;
            }
            _ => return Err(StoreError::UnsafePath),
        }
    }
    if removed_stage {
        sync_directory(path)?;
    }
    Ok(())
}

#[derive(Clone, Copy)]
struct CasNamespaceBounds {
    max_entries: usize,
    max_bytes: u64,
}

impl CasNamespaceBounds {
    fn steady(config: &RepositoryConfig) -> Result<Self, StoreError> {
        Ok(Self {
            max_entries: HARD_MAX_CACHE_OBJECTS,
            max_bytes: steady_cas_byte_limit(config)?,
        })
    }

    fn recovery(config: &RepositoryConfig) -> Result<Self, StoreError> {
        let package_bytes = package_cas_byte_limit(config)?;
        let tuf_bytes = tuf_cas_byte_limit(config)?;
        let max_object_bytes = max_cas_object_bytes(config);
        Ok(Self {
            max_entries: HARD_MAX_CACHE_OBJECTS
                .checked_add(config.limits.max_sources)
                .and_then(|entries| entries.checked_add(1 + MAX_TUF_FILES + 1))
                .ok_or(StoreError::StateCorrupt)?,
            max_bytes: steady_cas_byte_limit(config)?
                .checked_add(package_bytes)
                .and_then(|bytes| bytes.checked_add(tuf_bytes))
                .and_then(|bytes| bytes.checked_add(max_object_bytes))
                .ok_or(StoreError::StateCorrupt)?,
        })
    }
}

fn package_cas_byte_limit(config: &RepositoryConfig) -> Result<u64, StoreError> {
    let source_slots = config
        .limits
        .max_source_bytes
        .checked_mul(
            u64::try_from(config.limits.max_sources).map_err(|_| StoreError::StateCorrupt)?,
        )
        .ok_or(StoreError::StateCorrupt)?;
    config
        .limits
        .max_total_source_bytes
        .min(source_slots)
        .checked_add(
            u64::try_from(config.limits.max_manifest_bytes)
                .map_err(|_| StoreError::StateCorrupt)?,
        )
        .ok_or(StoreError::StateCorrupt)
}

fn tuf_cas_byte_limit(config: &RepositoryConfig) -> Result<u64, StoreError> {
    config
        .limits
        .max_root_bytes
        .checked_add(config.limits.max_targets_metadata_bytes)
        .and_then(|bytes| bytes.checked_add(config.limits.max_timestamp_metadata_bytes))
        .and_then(|bytes| bytes.checked_add(config.limits.max_snapshot_metadata_bytes))
        .and_then(|bytes| bytes.checked_add(HARD_MAX_KNOWN_TIME_BYTES))
        .ok_or(StoreError::StateCorrupt)
}

pub(crate) fn steady_cas_byte_limit(config: &RepositoryConfig) -> Result<u64, StoreError> {
    let package_bytes = package_cas_byte_limit(config)?;
    let tuf_bytes = tuf_cas_byte_limit(config)?;
    package_bytes
        .checked_mul(3)
        .and_then(|bytes| bytes.checked_add(tuf_bytes))
        .ok_or(StoreError::StateCorrupt)
}

#[cfg(test)]
pub(crate) fn recovery_cas_byte_limit(config: &RepositoryConfig) -> Result<u64, StoreError> {
    CasNamespaceBounds::recovery(config).map(|bounds| bounds.max_bytes)
}

#[cfg(test)]
pub(crate) fn recovery_cas_entry_limit(config: &RepositoryConfig) -> Result<usize, StoreError> {
    CasNamespaceBounds::recovery(config).map(|bounds| bounds.max_entries)
}

fn max_cas_object_bytes(config: &RepositoryConfig) -> u64 {
    config
        .limits
        .max_source_bytes
        .max(config.limits.max_manifest_bytes as u64)
        .max(config.limits.max_root_bytes)
        .max(config.limits.max_targets_metadata_bytes)
        .max(config.limits.max_timestamp_metadata_bytes)
        .max(config.limits.max_snapshot_metadata_bytes)
        .max(HARD_MAX_KNOWN_TIME_BYTES)
}

fn validate_object_namespace(
    path: &Path,
    config: &RepositoryConfig,
    bounds: CasNamespaceBounds,
) -> Result<(), StoreError> {
    validate_private_directory(path)?;
    let max_object_bytes = max_cas_object_bytes(config);
    let mut entries = 0usize;
    let mut aggregate_bytes = 0u64;
    let mut removed_stage = false;
    for entry in fs::read_dir(path).map_err(|_| StoreError::Io)? {
        entries = entries.checked_add(1).ok_or(StoreError::ObjectCorrupt)?;
        if entries > bounds.max_entries {
            return Err(StoreError::ObjectCorrupt);
        }
        let entry = entry.map_err(|_| StoreError::Io)?;
        let name = entry
            .file_name()
            .to_str()
            .ok_or(StoreError::ObjectCorrupt)?
            .to_owned();
        let is_object = parse_digest_filename(&name).is_some();
        let is_stage = parse_object_stage_filename(&name).is_some();
        if !is_object && !is_stage {
            return Err(StoreError::ObjectCorrupt);
        }
        let file = validate_bounded_regular(&entry.path(), max_object_bytes)?;
        let bytes = file.metadata().map_err(|_| StoreError::Io)?.len();
        drop(file);
        aggregate_bytes = aggregate_bytes
            .checked_add(bytes)
            .ok_or(StoreError::ObjectCorrupt)?;
        if aggregate_bytes > bounds.max_bytes {
            return Err(StoreError::ObjectCorrupt);
        }
        if is_stage {
            remove_verified_regular(&entry.path())?;
            removed_stage = true;
        }
    }
    if removed_stage {
        sync_directory(path)?;
    }
    Ok(())
}

fn reset_tuf_stage(path: &Path, config: &RepositoryConfig) -> Result<(), StoreError> {
    validate_private_directory(path)?;
    let mut entries = 0usize;
    let mut removed = false;
    for entry in fs::read_dir(path).map_err(|_| StoreError::Io)? {
        entries = entries.checked_add(1).ok_or(StoreError::UnsafePath)?;
        if entries > MAX_TUF_STAGE_ENTRIES {
            return Err(StoreError::UnsafePath);
        }
        let entry = entry.map_err(|_| StoreError::Io)?;
        let name = entry
            .file_name()
            .to_str()
            .ok_or(StoreError::UnsafePath)?
            .to_owned();
        let canonical_name = name
            .strip_prefix('.')
            .and_then(|name| name.strip_suffix(".stage"))
            .unwrap_or(&name);
        let _ = tuf_file_max_bytes(canonical_name, config).ok_or(StoreError::UnsafePath)?;
        harden_private_file_permissions(&entry.path())?;
        remove_verified_regular(&entry.path())?;
        removed = true;
    }
    if removed {
        sync_directory(path)?;
    }
    Ok(())
}

fn materialize_tuf_snapshot(
    objects: &Path,
    stage: &Path,
    snapshot: &StoredTufSnapshot,
    config: &RepositoryConfig,
) -> Result<(), StoreError> {
    validate_tuf_descriptor(snapshot)?;
    for descriptor in &snapshot.files {
        let max_bytes =
            tuf_file_max_bytes(&descriptor.name, config).ok_or(StoreError::StateCorrupt)?;
        if descriptor.length == 0 || descriptor.length > max_bytes {
            return Err(StoreError::StateCorrupt);
        }
        let bytes = read_object(objects, &descriptor.sha256, descriptor.length)?;
        if bytes.len() as u64 != descriptor.length {
            return Err(StoreError::ObjectCorrupt);
        }
        atomic_write(&stage.join(&descriptor.name), &bytes)?;
    }
    validate_complete_tuf_stage(stage, config, false)?;
    sync_directory(stage)
}

fn ingest_tuf_stage(
    stage: &Path,
    objects: &Path,
    config: &RepositoryConfig,
    expected_descriptor_sha256: [u8; 32],
) -> Result<StoredTufSnapshot, StoreError> {
    let files = validate_complete_tuf_stage(stage, config, true)?;
    let mut descriptors = Vec::with_capacity(MAX_TUF_FILES);
    let mut aggregate_bytes = 0u64;
    for (name, bytes) in &files {
        aggregate_bytes = aggregate_bytes
            .checked_add(bytes.len() as u64)
            .ok_or(StoreError::StateCorrupt)?;
        let sha256: [u8; 32] = Sha256::digest(bytes);
        descriptors.push(StoredTufFile {
            name: (*name).to_owned(),
            length: bytes.len() as u64,
            sha256,
        });
    }
    let descriptor_sha256 = tuf_descriptor_sha256(&descriptors)?;
    let snapshot = StoredTufSnapshot {
        files: descriptors,
        aggregate_bytes,
        descriptor_sha256,
    };
    validate_tuf_descriptor(&snapshot)?;
    if snapshot.descriptor_sha256 != expected_descriptor_sha256 {
        return Err(StoreError::StagedMetadataChanged);
    }
    for (descriptor, (_, bytes)) in snapshot.files.iter().zip(files) {
        replace_authenticated_object(objects, &descriptor.sha256, &bytes)?;
    }
    Ok(snapshot)
}

pub(crate) fn seal_tuf_stage(
    stage: &Path,
    config: &RepositoryConfig,
) -> Result<[u8; 32], StoreError> {
    let files = validate_complete_tuf_stage(stage, config, false)?;
    let mut descriptors = Vec::with_capacity(MAX_TUF_FILES);
    for (name, bytes) in files {
        descriptors.push(StoredTufFile {
            name: name.to_owned(),
            length: bytes.len() as u64,
            sha256: Sha256::digest(bytes),
        });
    }
    tuf_descriptor_sha256(&descriptors)
}

fn validate_complete_tuf_stage(
    stage: &Path,
    config: &RepositoryConfig,
    synchronize: bool,
) -> Result<Vec<(&'static str, Vec<u8>)>, StoreError> {
    validate_private_directory(stage)?;
    let mut entries = HashSet::new();
    for entry in fs::read_dir(stage).map_err(|_| StoreError::Io)? {
        if entries.len() >= MAX_TUF_FILES {
            return Err(StoreError::UnsafePath);
        }
        let entry = entry.map_err(|_| StoreError::Io)?;
        let name = entry
            .file_name()
            .to_str()
            .ok_or(StoreError::UnsafePath)?
            .to_owned();
        let max_bytes = tuf_file_max_bytes(&name, config).ok_or(StoreError::UnsafePath)?;
        if !entries.insert(name) {
            return Err(StoreError::UnsafePath);
        }
        harden_private_file_permissions(&entry.path())?;
        let file = validate_bounded_regular(&entry.path(), max_bytes)?;
        let metadata = file.metadata().map_err(|_| StoreError::Io)?;
        if metadata.len() == 0 {
            return Err(StoreError::UnsafePath);
        }
        if synchronize {
            #[cfg(target_os = "windows")]
            let file = crate::file_identity::open_verified_staged_for_sync(
                &entry.path(),
                &file,
                max_bytes,
            )
            .ok_or(StoreError::UnsafePath)?;
            // Windows FlushFileBuffers requires GENERIC_WRITE, obtained only for
            // this owned TUF stage after exact identity and size revalidation.
            file.sync_all().map_err(|_| StoreError::Io)?;
        }
    }
    if entries.len() != MAX_TUF_FILES || !TUF_FILE_NAMES.iter().all(|name| entries.contains(*name))
    {
        return Err(StoreError::UnsafePath);
    }
    if synchronize {
        sync_directory(stage)?;
    }
    TUF_FILE_NAMES
        .iter()
        .map(|name| {
            let max_bytes = tuf_file_max_bytes(name, config).ok_or(StoreError::UnsafePath)?;
            read_bounded_regular(&stage.join(name), max_bytes).map(|bytes| (*name, bytes))
        })
        .collect()
}

fn validate_tuf_descriptor(snapshot: &StoredTufSnapshot) -> Result<(), StoreError> {
    if snapshot.files.len() != MAX_TUF_FILES {
        return Err(StoreError::StateCorrupt);
    }
    let mut aggregate_bytes = 0u64;
    for (descriptor, expected_name) in snapshot.files.iter().zip(TUF_FILE_NAMES) {
        if descriptor.name != expected_name || descriptor.length == 0 {
            return Err(StoreError::StateCorrupt);
        }
        aggregate_bytes = aggregate_bytes
            .checked_add(descriptor.length)
            .ok_or(StoreError::StateCorrupt)?;
    }
    if aggregate_bytes != snapshot.aggregate_bytes
        || tuf_descriptor_sha256(&snapshot.files)? != snapshot.descriptor_sha256
    {
        return Err(StoreError::StateCorrupt);
    }
    Ok(())
}

fn validate_committed_tuf(
    objects: &Path,
    snapshot: Option<&StoredTufSnapshot>,
    config: &RepositoryConfig,
) -> Result<(), StoreError> {
    let Some(snapshot) = snapshot else {
        return Ok(());
    };
    validate_tuf_descriptor(snapshot)?;
    for descriptor in &snapshot.files {
        let max_bytes =
            tuf_file_max_bytes(&descriptor.name, config).ok_or(StoreError::StateCorrupt)?;
        if descriptor.length > max_bytes {
            return Err(StoreError::StateCorrupt);
        }
        let bytes = read_object(objects, &descriptor.sha256, descriptor.length)?;
        if bytes.len() as u64 != descriptor.length {
            return Err(StoreError::ObjectCorrupt);
        }
    }
    Ok(())
}

fn tuf_descriptor_sha256(files: &[StoredTufFile]) -> Result<[u8; 32], StoreError> {
    let bytes = serde_json::to_vec(files).map_err(|_| StoreError::StateCorrupt)?;
    Ok(Sha256::digest(bytes))
}

fn tuf_file_max_bytes(name: &str, config: &RepositoryConfig) -> Option<u64> {
    match name {
        "root.json" => Some(config.limits.max_root_bytes),
        "timestamp.json" => Some(config.limits.max_timestamp_metadata_bytes),
        "snapshot.json" => Some(config.limits.max_snapshot_metadata_bytes),
        "targets.json" => Some(config.limits.max_targets_metadata_bytes),
        "latest_known_time.json" => Some(HARD_MAX_KNOWN_TIME_BYTES),
        _ => None,
    }
}

fn validate_bounded_regular(path: &Path, max_bytes: u64) -> Result<File, StoreError> {
    let file = crate::file_identity::open_verified_regular(path).ok_or(StoreError::UnsafePath)?;
    let metadata = file.metadata().map_err(|_| StoreError::Io)?;
    if !metadata.is_file() || metadata.len() > max_bytes {
        return Err(StoreError::UnsafePath);
    }
    Ok(file)
}

#[cfg(unix)]
fn harden_private_file_permissions(path: &Path) -> Result<(), StoreError> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    let metadata = fs::symlink_metadata(path).map_err(|_| StoreError::Io)?;
    if !metadata.is_file()
        || metadata.file_type().is_symlink()
        || metadata.nlink() != 1
        || metadata.uid() != rustix::process::geteuid().as_raw()
    {
        return Err(StoreError::UnsafePath);
    }
    if metadata.mode() & 0o077 != 0 {
        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).map_err(|_| StoreError::Io)?;
    }
    Ok(())
}

#[cfg(not(unix))]
fn harden_private_file_permissions(path: &Path) -> Result<(), StoreError> {
    let metadata = fs::symlink_metadata(path).map_err(|_| StoreError::Io)?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(StoreError::UnsafePath);
    }
    Ok(())
}

fn encode_canonical<T: Serialize>(value: &T) -> Result<Vec<u8>, StoreError> {
    serde_json::to_vec(value).map_err(|_| StoreError::StateCorrupt)
}

fn decode_canonical<T>(bytes: &[u8]) -> Result<T, StoreError>
where
    T: for<'de> Deserialize<'de> + Serialize,
{
    let value: T = serde_json::from_slice(bytes).map_err(|_| StoreError::StateCorrupt)?;
    if encode_canonical(&value)? != bytes {
        return Err(StoreError::StateCorrupt);
    }
    Ok(value)
}

fn hex_digest(digest: &[u8; 32]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(64);
    for byte in digest {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }
    output
}

fn parse_digest_filename(value: &str) -> Option<[u8; 32]> {
    crate::manifest::decode_sha256(value).ok()
}

fn parse_object_stage_filename(value: &str) -> Option<[u8; 32]> {
    value
        .strip_prefix('.')
        .and_then(|value| value.strip_suffix(".stage"))
        .and_then(parse_digest_filename)
}

fn parse_journal_filename(value: &str) -> Option<(u64, [u8; 32])> {
    let value = value.strip_suffix(".json")?;
    let (generation, digest) = value.split_once('-')?;
    if generation.len() != 20 || !generation.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let generation = generation.parse().ok()?;
    let digest = parse_digest_filename(digest)?;
    Some((generation, digest))
}

fn parse_journal_stage_filename(value: &str) -> Option<(u64, [u8; 32])> {
    value
        .strip_prefix('.')
        .and_then(|value| value.strip_suffix(".stage"))
        .and_then(parse_journal_filename)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum FaultPoint {
    None,
    AfterObjects,
    AfterJournal,
    AfterStateReplace,
}

impl FaultPoint {
    fn fail_if(self, point: Self) -> Result<(), StoreError> {
        if self == point {
            return Err(StoreError::InjectedFault);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_state_rejects_unknown_or_noncanonical_input() {
        let state = StoreState::default();
        let bytes = encode_canonical(&state).unwrap();
        assert_eq!(decode_canonical::<StoreState>(&bytes).unwrap(), state);

        let spaced = serde_json::to_string_pretty(&state).unwrap();
        assert_eq!(
            decode_canonical::<StoreState>(spaced.as_bytes()),
            Err(StoreError::StateCorrupt)
        );
    }

    #[test]
    fn digest_filenames_are_exact_lowercase_hex() {
        let digest = [0xabu8; 32];
        let encoded = hex_digest(&digest);
        assert_eq!(encoded.len(), 64);
        assert_eq!(parse_digest_filename(&encoded), Some(digest));
        assert_eq!(parse_digest_filename(&encoded.to_uppercase()), None);
    }

    #[test]
    fn authenticated_repair_replaces_short_same_and_long_regular_corruption() {
        let root = tempfile::tempdir().unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
        }
        let objects = root.path().canonicalize().unwrap().join("objects");
        create_private_directory(&objects).unwrap();
        let expected = b"correct";
        let digest: [u8; 32] = Sha256::digest(expected);
        let path = objects.join(hex_digest(&digest));

        for corrupt in [
            b"x".as_slice(),
            b"wrong!!".as_slice(),
            b"definitely-longer".as_slice(),
        ] {
            atomic_write(&path, corrupt).unwrap();
            replace_authenticated_object(&objects, &digest, expected).unwrap();
            assert_eq!(
                read_object(&objects, &digest, expected.len() as u64).unwrap(),
                expected
            );
        }
    }
}
