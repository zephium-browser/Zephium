//! Production coordination for authenticated filter packages and compilation.
//!
//! The service owns no profile policy and performs no native work. It joins
//! the updater's durable package activation barrier to the compiler's ordered
//! catalog-replacement barrier, then exposes a small nonblocking maintenance
//! port to the authoritative application actor.

#![deny(unsafe_code)]
#![deny(missing_docs)]

mod release_seed;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, TryLockError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[cfg(any(feature = "tuf", test))]
use zephium_blocker::PolicyCatalog;
use zephium_blocker::{
    CatalogPreparationOutcome, CatalogReplacementDispatch, CompiledArtifactCacheConfig,
    StaticPolicyCatalog, WorkerBlocker,
};
#[cfg(feature = "tuf")]
use zephium_blocker_update::RepositoryConfig;
use zephium_blocker_update::{
    ActivatedCatalog, CandidateCommitDispatch, CandidateCommitOutcome, CandidateRejectDispatch,
    CandidateRejectOutcome, CandidateRejectionReason, CandidateRepairDispatch,
    CandidateRepairOutcome, CatalogAvailability, CatalogIdentity, CatalogUpdateWorker, FailureKind,
    RefreshAdmission, ShutdownOutcome as UpdateShutdownOutcome, StatusSnapshot, UnavailableReason,
    UpdateStatus,
};
use zephium_core::blocker::{BlockerConfig, ContentPolicyGeneration};
use zephium_core::ids::ProfileId;
use zephium_core::ports::blocker::{
    BlockerCatalog, BlockerCatalogFailure, BlockerCatalogPhase, BlockerCatalogProvenance,
    BlockerCatalogRefreshDispatch, BlockerCatalogSnapshot, BlockerCatalogUnavailable,
    BlockerCompileFailure, BlockerCompileOutcome, BlockerCompiler, BlockerDispatch,
    BlockerRetirementDispatch, BlockerShutdownOutcome,
};

pub use release_seed::{
    EmbeddedReleaseAsset, ReleaseCatalogSeed, ReleaseSeedAssetManifest, ReleaseSeedCompression,
    ReleaseSeedError, ReleaseSeedManifest,
};
pub use zephium_blocker_update::{LicensePolicy, UpdateLimits};

const NORMAL_REFRESH_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);
const EXPIRY_REFRESH_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);
const MIN_EXPIRY_REFRESH_INTERVAL: Duration = Duration::from_secs(15 * 60);
const STALE_REFRESH_INTERVAL: Duration = Duration::from_secs(60 * 60);
const EXPIRY_REFRESH_JITTER_WINDOW: Duration = Duration::from_secs(2 * 60 * 60);
const FAILURE_BACKOFF: [Duration; 4] = [
    Duration::from_secs(15 * 60),
    Duration::from_secs(60 * 60),
    Duration::from_secs(6 * 60 * 60),
    Duration::from_secs(24 * 60 * 60),
];
const EXPIRY_REFRESH_LEAD: Duration = Duration::from_secs(24 * 60 * 60);
const SHUTDOWN_POLL_INTERVAL: Duration = Duration::from_millis(1);

/// Compiler plus optional authenticated catalog updater.
///
/// Expensive work remains on the two dedicated bounded workers. Calls through
/// [`BlockerCatalog`] perform only small lock-protected state transitions and
/// nonblocking queue admission.
/// Narrow native preflight port. The service never receives a WebView,
/// navigation capability, or a native handle.
pub type NativeRuleValidator = Arc<
    dyn Fn(
            Arc<zephium_core::blocker::ContentRules>,
            zephium_core::ports::engine::ContentRuleValidationCompletion,
        ) + Send
        + Sync,
>;

/// Compiler and bounded source-update coordinator shared by all profiles.
pub struct ManagedBlocker {
    compiler: Arc<WorkerBlocker>,
    native_validation: Option<NativeRuleValidator>,
    updater: Mutex<Option<CatalogUpdateWorker>>,
    supply: SupplyMode,
    state: Mutex<ServiceState>,
    source_material_signal: Arc<SourceMaterialSignal>,
    transition_completion: Arc<Mutex<Option<CatalogTransitionCompletion>>>,
    maintenance_serial: Mutex<()>,
    shutdown_serial: Mutex<()>,
    shutdown_result: Mutex<Option<BlockerShutdownOutcome>>,
    sealed: AtomicBool,
    /// Present while a downloaded list compiles. A release build aborts on a
    /// panic, so a marker left at launch names a list that crashed the
    /// browser; it is refused instead of compiled again on every start.
    compile_marker: Option<std::path::PathBuf>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SupplyMode {
    Unconfigured,
    ReleaseSeed,
    #[cfg(feature = "tuf")]
    TufRepository,
    #[cfg(feature = "official-https")]
    OfficialHttps {
        fallback_manifest: [u8; 32],
    },
}

struct ServiceState {
    snapshot: BlockerCatalogSnapshot,
    observed_update: Option<StatusSnapshot>,
    installed: Option<CatalogIdentity>,
    candidate: Option<CatalogIdentity>,
    pending: Option<ActivatedCatalog>,
    transition: Option<CatalogTransition>,
    failure_streak: usize,
    next_automatic_refresh_unix: Option<u64>,
    refresh_jitter: Duration,
    terminal_failure: Option<BlockerCatalogFailure>,
    enabled_policy_terminal: bool,
    repaired_candidate: Option<CatalogIdentity>,
    source_material_epoch: u64,
    source_material_repair: Option<SourceMaterialRepair>,
    automatic_source_material_repair: Option<CatalogIdentity>,
    native_retry: Option<(CatalogIdentity, u8, Instant)>,
    official_verification: Option<u64>,
}

#[derive(Clone)]
enum SourceMaterialRepair {
    Requested(CatalogIdentity),
    InFlight {
        identity: CatalogIdentity,
        operation: u64,
    },
    RetryPending(CatalogIdentity),
}

/// Names the downloaded list compiling right now, inside the source cache.
const COMPILE_MARKER: &str = "compiling";

fn hex_digest(digest: &[u8; 32]) -> String {
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

struct SourceMaterialSignal {
    sealed: AtomicBool,
    pending: Mutex<Option<CatalogIdentity>>,
}

#[derive(Clone)]
enum CatalogTransition {
    Preparing(CatalogIdentity),
    ReadyToRepair(CatalogIdentity),
    Repairing(CatalogIdentity),
    RepairRetryPending(CatalogIdentity, FailureKind),
    ReadyToCommit(CatalogIdentity),
    Committing(CatalogIdentity),
    ReadyToActivate(CatalogIdentity),
    Activating(CatalogIdentity),
    ReadyToReject(CatalogIdentity, FailureKind, CandidateRejectionReason),
    Rejecting(CatalogIdentity, FailureKind),
    ReadyToDiscard(CatalogIdentity, Option<BlockerCatalogFailure>),
    Discarding(CatalogIdentity, Option<BlockerCatalogFailure>),
}

enum CatalogTransitionCompletion {
    Prepared(CatalogIdentity, CatalogPreparationOutcome),
    NativeUnavailable(CatalogIdentity),
    Repaired(CatalogIdentity, CandidateRepairOutcome),
    Committed(CatalogIdentity, CandidateCommitOutcome),
    Activated(CatalogIdentity, bool),
    Rejected(CatalogIdentity, CandidateRejectOutcome),
    Discarded(CatalogIdentity, bool),
}

impl SourceMaterialRepair {
    fn source_identity(&self) -> &CatalogIdentity {
        match self {
            Self::Requested(identity)
            | Self::InFlight { identity, .. }
            | Self::RetryPending(identity) => identity,
        }
    }
}

impl SourceMaterialSignal {
    fn new() -> Self {
        Self {
            sealed: AtomicBool::new(false),
            pending: Mutex::new(None),
        }
    }

    fn report(&self, identity: CatalogIdentity) {
        if self.sealed.load(Ordering::Acquire) {
            return;
        }
        let mut pending = self
            .pending
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if self.sealed.load(Ordering::Acquire) {
            return;
        }
        match pending.as_ref() {
            Some(current) if current.revision >= identity.revision => {}
            _ => *pending = Some(identity),
        }
    }

    fn take(&self) -> Option<CatalogIdentity> {
        self.pending
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
    }

    fn seal(&self) {
        self.sealed.store(true, Ordering::Release);
        match self.pending.try_lock() {
            Ok(mut pending) => {
                let _ = pending.take();
            }
            Err(TryLockError::Poisoned(poisoned)) => {
                let _ = poisoned.into_inner().take();
            }
            Err(TryLockError::WouldBlock) => {}
        }
    }
}

impl ManagedBlocker {
    /// Starts a cache-backed compiler without a provisioned update repository.
    ///
    /// Disabled profiles still receive an explicit allow-all generation.
    /// Enabling protection reports source-unavailable and never labels an
    /// empty catalog as active protection.
    pub fn unconfigured(artifact_cache: CompiledArtifactCacheConfig) -> std::io::Result<Arc<Self>> {
        let compiler =
            WorkerBlocker::start_with_cache(StaticPolicyCatalog::empty(), artifact_cache)?;
        Ok(Arc::new(Self {
            compiler,
            native_validation: None,
            updater: Mutex::new(None),
            supply: SupplyMode::Unconfigured,
            state: Mutex::new(ServiceState {
                snapshot: BlockerCatalogSnapshot::not_configured(),
                observed_update: None,
                installed: None,
                candidate: None,
                pending: None,
                transition: None,
                failure_streak: 0,
                next_automatic_refresh_unix: None,
                refresh_jitter: Duration::ZERO,
                terminal_failure: None,
                enabled_policy_terminal: false,
                repaired_candidate: None,
                source_material_epoch: 0,
                source_material_repair: None,
                automatic_source_material_repair: None,
                native_retry: None,
                official_verification: None,
            }),
            source_material_signal: Arc::new(SourceMaterialSignal::new()),
            transition_completion: Arc::new(Mutex::new(None)),
            maintenance_serial: Mutex::new(()),
            shutdown_serial: Mutex::new(()),
            shutdown_result: Mutex::new(None),
            sealed: AtomicBool::new(false),
            compile_marker: None,
        }))
    }

    /// Starts with an immutable catalog embedded in the signed application
    /// release.
    ///
    /// The source package is immediately authoritative for compilation, while
    /// profile protection remains disabled until the user's persisted
    /// preference enables it. Network refresh is explicitly unsupported in
    /// this mode; a later TUF integration can supersede it without making
    /// first-run filtering depend on network availability.
    pub fn with_release_seed(
        seed: ReleaseCatalogSeed,
        artifact_cache: CompiledArtifactCacheConfig,
    ) -> std::io::Result<Arc<Self>> {
        let identity = seed.identity;
        let compiler = WorkerBlocker::start_with_policy_catalog(seed.catalog, artifact_cache)?;
        let mut state = ServiceState {
            snapshot: BlockerCatalogSnapshot::not_configured(),
            observed_update: None,
            installed: Some(identity),
            candidate: None,
            pending: None,
            transition: None,
            failure_streak: 0,
            next_automatic_refresh_unix: None,
            refresh_jitter: Duration::ZERO,
            terminal_failure: None,
            enabled_policy_terminal: false,
            repaired_candidate: None,
            source_material_epoch: 0,
            source_material_repair: None,
            automatic_source_material_repair: None,
            native_retry: None,
            official_verification: None,
        };
        state.publish_release_seed(now_unix());
        Ok(Arc::new(Self {
            compiler,
            native_validation: None,
            updater: Mutex::new(None),
            supply: SupplyMode::ReleaseSeed,
            state: Mutex::new(state),
            source_material_signal: Arc::new(SourceMaterialSignal::new()),
            transition_completion: Arc::new(Mutex::new(None)),
            maintenance_serial: Mutex::new(()),
            shutdown_serial: Mutex::new(()),
            shutdown_result: Mutex::new(None),
            sealed: AtomicBool::new(false),
            compile_marker: None,
        }))
    }

    /// Starts official HTTPS updates with a bundled offline fallback and an
    /// exact native validation barrier before durable source activation.
    #[cfg(feature = "official-https")]
    pub fn with_official_updates(
        seed: ReleaseCatalogSeed,
        artifact_cache: CompiledArtifactCacheConfig,
        source_cache: std::path::PathBuf,
        validate: NativeRuleValidator,
    ) -> std::io::Result<Arc<Self>> {
        let fallback = ActivatedCatalog {
            identity: seed.identity.clone(),
            catalog: seed.catalog.clone(),
        };
        let fallback_manifest = fallback.identity.manifest_sha256;
        let compile_marker = source_cache.join(COMPILE_MARKER);
        let crashed_compile = std::fs::read_to_string(&compile_marker).ok();
        let _ = std::fs::remove_file(&compile_marker);
        let updater = match CatalogUpdateWorker::start_official(source_cache, fallback) {
            Ok(updater) => updater,
            Err(_) => {
                let fallback = Self::with_release_seed(seed, artifact_cache)?;
                {
                    let mut state = fallback.lock_state();
                    state.fail(BlockerCatalogFailure::Storage, false);
                    state.publish_release_seed(now_unix());
                }
                return Ok(fallback);
            }
        };
        let (initial_status, initial, candidate) = updater.observe_and_take_catalogs();
        let initial =
            initial.expect("official worker always supplies its selected offline current");
        let compiler =
            match WorkerBlocker::start_with_policy_catalog(initial.catalog, artifact_cache) {
                Ok(compiler) => compiler,
                Err(error) => {
                    let _ = updater.shutdown(Duration::from_secs(1));
                    return Err(error);
                }
            };
        let mut state = ServiceState {
            snapshot: BlockerCatalogSnapshot::not_configured(),
            observed_update: None,
            installed: Some(initial.identity),
            candidate: None,
            pending: None,
            transition: None,
            failure_streak: 0,
            next_automatic_refresh_unix: None,
            refresh_jitter: Duration::ZERO,
            terminal_failure: None,
            enabled_policy_terminal: false,
            repaired_candidate: None,
            source_material_epoch: 0,
            source_material_repair: None,
            automatic_source_material_repair: None,
            native_retry: None,
            official_verification: None,
        };
        if let Some(candidate) = candidate {
            if crashed_compile.as_deref().map(str::trim)
                == Some(hex_digest(&candidate.identity.manifest_sha256).as_str())
            {
                eprintln!(
                    "blocker: a downloaded list stopped the browser while compiling; it is refused"
                );
                state.candidate = Some(candidate.identity.clone());
                state.transition = Some(CatalogTransition::ReadyToReject(
                    candidate.identity,
                    FailureKind::Catalog,
                    CandidateRejectionReason::CompilerPolicy,
                ));
            } else {
                state.admit_activation(candidate, now_unix());
            }
        }
        state.recompute_schedule(&initial_status, now_unix());
        state.publish_with_supply(
            &initial_status,
            Some((
                fallback_manifest,
                updater.official_freshness().is_none_or(|(_, due)| due),
            )),
        );
        Ok(Arc::new(Self {
            compiler,
            native_validation: Some(validate),
            updater: Mutex::new(Some(updater)),
            supply: SupplyMode::OfficialHttps { fallback_manifest },
            state: Mutex::new(state),
            source_material_signal: Arc::new(SourceMaterialSignal::new()),
            transition_completion: Arc::new(Mutex::new(None)),
            maintenance_serial: Mutex::new(()),
            shutdown_serial: Mutex::new(()),
            shutdown_result: Mutex::new(None),
            sealed: AtomicBool::new(false),
            compile_marker: Some(compile_marker),
        }))
    }

    /// Starts the authenticated updater, recovers its durable current package,
    /// and makes that exact catalog authoritative before returning.
    ///
    /// This low-level repository-only constructor exists for updater
    /// verification. A future desktop integration must preserve the embedded
    /// release seed as its offline baseline and admit only a monotonic,
    /// provenance-carrying transition to repository authority.
    #[cfg(feature = "tuf")]
    pub fn with_repository(
        repository: RepositoryConfig,
        artifact_cache: CompiledArtifactCacheConfig,
    ) -> std::io::Result<Arc<Self>> {
        let updater = CatalogUpdateWorker::start(repository);
        let (initial_status, initial, candidate) = updater.observe_and_take_catalogs();
        let catalog = initial.as_ref().map_or_else(
            || PolicyCatalog::eager(StaticPolicyCatalog::empty()),
            |value| value.catalog.clone(),
        );
        let compiler = match WorkerBlocker::start_with_policy_catalog(catalog, artifact_cache) {
            Ok(compiler) => compiler,
            Err(error) => {
                let _ = updater.shutdown(Duration::from_secs(1));
                return Err(error);
            }
        };
        let installed = initial.map(|value| value.identity);
        let now = now_unix();
        let mut state = ServiceState {
            snapshot: BlockerCatalogSnapshot::not_configured(),
            observed_update: Some(initial_status.clone()),
            installed,
            candidate: None,
            pending: None,
            transition: None,
            failure_streak: 0,
            next_automatic_refresh_unix: None,
            refresh_jitter: refresh_jitter(),
            terminal_failure: None,
            enabled_policy_terminal: false,
            repaired_candidate: None,
            source_material_epoch: 0,
            source_material_repair: None,
            automatic_source_material_repair: None,
            native_retry: None,
            official_verification: None,
        };
        if let Some(candidate) = candidate {
            state.admit_activation(candidate, now);
        }
        state.recompute_schedule(&initial_status, now);
        state.publish(&initial_status);
        Ok(Arc::new(Self {
            compiler,
            native_validation: None,
            updater: Mutex::new(Some(updater)),
            supply: SupplyMode::TufRepository,
            state: Mutex::new(state),
            source_material_signal: Arc::new(SourceMaterialSignal::new()),
            transition_completion: Arc::new(Mutex::new(None)),
            maintenance_serial: Mutex::new(()),
            shutdown_serial: Mutex::new(()),
            shutdown_result: Mutex::new(None),
            sealed: AtomicBool::new(false),
            compile_marker: None,
        }))
    }

    fn lock_state(&self) -> MutexGuard<'_, ServiceState> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn lock_updater(&self) -> MutexGuard<'_, Option<CatalogUpdateWorker>> {
        self.updater
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn drive_locked(&self, automatic_refresh: bool) -> BlockerCatalogSnapshot {
        if self.sealed.load(Ordering::Acquire) {
            let mut state = self.lock_state();
            state.publish_shutdown();
            return state.snapshot;
        }

        let (mut update, candidate) = {
            let updater = self.lock_updater();
            match updater.as_ref() {
                Some(updater) => {
                    let (status, unexpected_current, candidate) =
                        updater.observe_and_take_catalogs();
                    if unexpected_current.is_some() {
                        let mut state = self.lock_state();
                        state.fail(BlockerCatalogFailure::Internal, true);
                    }
                    (Some(status), candidate)
                }
                None => (None, None),
            }
        };

        let completion = self
            .transition_completion
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        let source_failure = self.source_material_signal.take();
        {
            let mut state = self.lock_state();
            if let Some(candidate) = candidate {
                state.admit_activation(candidate, now_unix());
            }
            if let Some(completion) = completion {
                state.apply_transition_completion(completion);
            }
            if let Some(identity) = source_failure {
                state.admit_source_material_failure(identity);
            }
            state.expire_retry_candidate(now_unix());
        }

        self.drive_catalog_transition();

        update = self
            .lock_updater()
            .as_ref()
            .map(CatalogUpdateWorker::status)
            .or(update);
        {
            let mut state = self.lock_state();
            if let Some(update) = &update {
                state.observe_update(update, now_unix());
            }
        }

        self.drive_source_material_repair(update.as_ref());
        update = self
            .lock_updater()
            .as_ref()
            .map(CatalogUpdateWorker::status)
            .or(update);
        {
            let mut state = self.lock_state();
            if let Some(update) = &update {
                state.observe_update(update, now_unix());
            }
        }

        if automatic_refresh {
            self.drive_automatic_refresh(update.as_ref());
        }

        let freshness = self
            .lock_updater()
            .as_ref()
            .and_then(CatalogUpdateWorker::official_freshness);
        let mut state = self.lock_state();
        if let Some(update) = &update {
            state.observe_official_verification(update, freshness);
            #[cfg(feature = "official-https")]
            if let SupplyMode::OfficialHttps { fallback_manifest } = self.supply {
                state.publish_with_supply(
                    update,
                    Some((fallback_manifest, freshness.is_none_or(|(_, due)| due))),
                );
                return state.snapshot;
            }
            let _ = freshness;
            state.publish(update);
        } else if self.supply == SupplyMode::ReleaseSeed {
            state.publish_release_seed(now_unix());
        } else if self.sealed.load(Ordering::Acquire) {
            state.publish_shutdown();
        }
        state.snapshot
    }

    fn drive_catalog_transition(&self) {
        enum Action {
            Prepare(ActivatedCatalog),
            Repair(CatalogIdentity),
            Commit(CatalogIdentity),
            Activate(CatalogIdentity),
            Reject(CatalogIdentity, FailureKind, CandidateRejectionReason),
            Discard(CatalogIdentity),
        }
        let action = {
            let state = self.lock_state();
            if state.terminal_failure.is_some() {
                None
            } else if let Some(transition) = &state.transition {
                match transition {
                    CatalogTransition::ReadyToRepair(identity) => {
                        Some(Action::Repair(identity.clone()))
                    }
                    CatalogTransition::ReadyToCommit(identity) => {
                        Some(Action::Commit(identity.clone()))
                    }
                    CatalogTransition::ReadyToActivate(identity) => {
                        Some(Action::Activate(identity.clone()))
                    }
                    CatalogTransition::ReadyToReject(identity, failure, reason) => {
                        Some(Action::Reject(identity.clone(), *failure, *reason))
                    }
                    CatalogTransition::ReadyToDiscard(identity, _) => {
                        Some(Action::Discard(identity.clone()))
                    }
                    CatalogTransition::Preparing(_)
                    | CatalogTransition::Repairing(_)
                    | CatalogTransition::RepairRetryPending(_, _)
                    | CatalogTransition::Committing(_)
                    | CatalogTransition::Activating(_)
                    | CatalogTransition::Rejecting(_, _)
                    | CatalogTransition::Discarding(_, _) => None,
                }
            } else {
                state
                    .pending
                    .clone()
                    .filter(|_| {
                        state
                            .native_retry
                            .as_ref()
                            .is_none_or(|(_, _, deadline)| Instant::now() >= *deadline)
                    })
                    .map(Action::Prepare)
            }
        };
        let Some(action) = action else {
            return;
        };
        match action {
            Action::Prepare(candidate) => {
                let identity = candidate.identity.clone();
                let callback_identity = identity.clone();
                let marker = self.compile_marker.clone();
                if let Some(marker) = marker.as_ref() {
                    let _ = std::fs::write(marker, hex_digest(&identity.manifest_sha256));
                }
                let completion = Arc::clone(&self.transition_completion);
                let native_validation = self.native_validation.clone();
                let dispatch = self.compiler.prepare_catalog_rules(
                    identity.manifest_sha256,
                    candidate.catalog,
                    Box::new(move |outcome| {
                        if let Some(marker) = marker.as_ref() {
                            let _ = std::fs::remove_file(marker);
                        }
                        let publish=move |value| {completion.lock().unwrap_or_else(|p|p.into_inner()).replace(value);};
                        match outcome {
                            Err(failure)=>publish(CatalogTransitionCompletion::Prepared(callback_identity,CatalogPreparationOutcome::Failed(failure))),
                            Ok(rules)=>if let Some(validate)=native_validation {
                                validate(rules,zephium_core::ports::engine::ContentRuleValidationCompletion::new(move |outcome| {
                                    use zephium_core::ports::engine::ContentRuleValidationOutcome;
                                    publish(match outcome {
                                        ContentRuleValidationOutcome::Valid=>CatalogTransitionCompletion::Prepared(callback_identity,CatalogPreparationOutcome::Prepared),
                                        ContentRuleValidationOutcome::Unavailable=>CatalogTransitionCompletion::NativeUnavailable(callback_identity),
                                        ContentRuleValidationOutcome::Rejected(_)=>CatalogTransitionCompletion::Prepared(callback_identity,CatalogPreparationOutcome::Failed(BlockerCompileFailure::InvalidSource)),
                                    });
                                }));
                            } else {publish(CatalogTransitionCompletion::Prepared(callback_identity,CatalogPreparationOutcome::Prepared));},
                        }
                    }),
                );
                let mut state = self.lock_state();
                match dispatch {
                    CatalogReplacementDispatch::Scheduled
                        if state
                            .pending
                            .as_ref()
                            .is_some_and(|pending| pending.identity == identity)
                            && state.transition.is_none() =>
                    {
                        state.transition = Some(CatalogTransition::Preparing(identity));
                    }
                    CatalogReplacementDispatch::Scheduled
                    | CatalogReplacementDispatch::Terminal => {
                        state.fail(BlockerCatalogFailure::Internal, true);
                    }
                    CatalogReplacementDispatch::Rejected => {}
                }
            }
            Action::Repair(identity) => {
                let _ = self.schedule_candidate_repair(identity, false);
            }
            Action::Commit(identity) => {
                let callback_identity = identity.clone();
                let completion = Arc::clone(&self.transition_completion);
                let dispatch = self.lock_updater().as_ref().map_or(
                    CandidateCommitDispatch::Terminal,
                    |updater| {
                        updater.commit_candidate(
                            identity.clone(),
                            Box::new(move |outcome| {
                                let mut slot = completion
                                    .lock()
                                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                                let _ = slot.replace(CatalogTransitionCompletion::Committed(
                                    callback_identity,
                                    outcome,
                                ));
                            }),
                        )
                    },
                );
                let mut state = self.lock_state();
                match dispatch {
                    CandidateCommitDispatch::Scheduled
                        if matches!(
                            state.transition,
                            Some(CatalogTransition::ReadyToCommit(ref expected))
                                if *expected == identity
                        ) =>
                    {
                        state.transition = Some(CatalogTransition::Committing(identity));
                    }
                    CandidateCommitDispatch::Scheduled | CandidateCommitDispatch::Terminal => {
                        if matches!(dispatch, CandidateCommitDispatch::Terminal)
                            && state.candidate.as_ref() == Some(&identity)
                        {
                            state.transition = Some(CatalogTransition::ReadyToDiscard(
                                identity,
                                Some(BlockerCatalogFailure::Internal),
                            ));
                        } else {
                            state.fail(BlockerCatalogFailure::Internal, true);
                        }
                    }
                    CandidateCommitDispatch::Rejected => {}
                }
            }
            Action::Activate(identity) => {
                let callback_identity = identity.clone();
                let completion = Arc::clone(&self.transition_completion);
                let dispatch = self.compiler.activate_prepared_catalog(
                    identity.manifest_sha256,
                    Box::new(move |success| {
                        let mut slot = completion
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner());
                        let _ = slot.replace(CatalogTransitionCompletion::Activated(
                            callback_identity,
                            success,
                        ));
                    }),
                );
                let mut state = self.lock_state();
                match dispatch {
                    CatalogReplacementDispatch::Scheduled
                        if matches!(
                            state.transition,
                            Some(CatalogTransition::ReadyToActivate(ref expected))
                                if *expected == identity
                        ) =>
                    {
                        state.transition = Some(CatalogTransition::Activating(identity));
                    }
                    CatalogReplacementDispatch::Scheduled
                    | CatalogReplacementDispatch::Terminal => {
                        state.fail(BlockerCatalogFailure::Internal, true);
                    }
                    CatalogReplacementDispatch::Rejected => {}
                }
            }
            Action::Reject(identity, failure, reason) => {
                let callback_identity = identity.clone();
                let completion = Arc::clone(&self.transition_completion);
                let dispatch = self.lock_updater().as_ref().map_or(
                    CandidateRejectDispatch::Terminal,
                    |updater| {
                        updater.reject_candidate(
                            identity.clone(),
                            failure,
                            reason,
                            Box::new(move |outcome| {
                                let mut slot = completion
                                    .lock()
                                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                                let _ = slot.replace(CatalogTransitionCompletion::Rejected(
                                    callback_identity,
                                    outcome,
                                ));
                            }),
                        )
                    },
                );
                let mut state = self.lock_state();
                match dispatch {
                    CandidateRejectDispatch::Scheduled
                        if matches!(
                            state.transition,
                            Some(CatalogTransition::ReadyToReject(
                                ref expected,
                                expected_failure,
                                _
                            )) if *expected == identity && expected_failure == failure
                        ) =>
                    {
                        state.transition = Some(CatalogTransition::Rejecting(identity, failure));
                    }
                    CandidateRejectDispatch::Scheduled | CandidateRejectDispatch::Terminal => {
                        state.fail(BlockerCatalogFailure::Internal, true);
                    }
                    CandidateRejectDispatch::Rejected => {}
                }
            }
            Action::Discard(identity) => {
                let callback_identity = identity.clone();
                let completion = Arc::clone(&self.transition_completion);
                let dispatch = self.compiler.discard_prepared_catalog(
                    identity.manifest_sha256,
                    Box::new(move |discarded| {
                        let mut slot = completion
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner());
                        let _ = slot.replace(CatalogTransitionCompletion::Discarded(
                            callback_identity,
                            discarded,
                        ));
                    }),
                );
                let mut state = self.lock_state();
                match dispatch {
                    CatalogReplacementDispatch::Scheduled
                        if matches!(
                            state.transition,
                            Some(CatalogTransition::ReadyToDiscard(ref expected, _))
                                if *expected == identity
                        ) =>
                    {
                        let terminal = match state.transition.take() {
                            Some(CatalogTransition::ReadyToDiscard(_, terminal)) => terminal,
                            _ => None,
                        };
                        state.transition = Some(CatalogTransition::Discarding(identity, terminal));
                    }
                    CatalogReplacementDispatch::Scheduled
                    | CatalogReplacementDispatch::Terminal => {
                        state.fail(BlockerCatalogFailure::Internal, true);
                    }
                    CatalogReplacementDispatch::Rejected => {}
                }
            }
        }
    }

    fn schedule_candidate_repair(
        &self,
        identity: CatalogIdentity,
        explicit_retry: bool,
    ) -> CandidateRepairDispatch {
        let callback_identity = identity.clone();
        let completion = Arc::clone(&self.transition_completion);
        let dispatch =
            self.lock_updater()
                .as_ref()
                .map_or(CandidateRepairDispatch::Terminal, |updater| {
                    let done = Box::new(move |outcome| {
                        let mut slot = completion
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner());
                        let _ = slot.replace(CatalogTransitionCompletion::Repaired(
                            callback_identity,
                            outcome,
                        ));
                    });
                    if explicit_retry {
                        updater.retry_candidate(identity.clone(), done)
                    } else {
                        updater.repair_candidate(identity.clone(), done)
                    }
                });
        let mut state = self.lock_state();
        let expected = if explicit_retry {
            matches!(
                state.transition,
                Some(CatalogTransition::RepairRetryPending(ref pending, _))
                    if *pending == identity
            )
        } else {
            matches!(
                state.transition,
                Some(CatalogTransition::ReadyToRepair(ref pending))
                    if *pending == identity
            ) && state.repaired_candidate.as_ref() != Some(&identity)
        };
        match dispatch {
            CandidateRepairDispatch::Scheduled { .. } if expected => {
                state.transition = Some(CatalogTransition::Repairing(identity));
            }
            CandidateRepairDispatch::Rejected => {}
            CandidateRepairDispatch::LimitReached if explicit_retry && expected => {
                let failure = match state.transition.take() {
                    Some(CatalogTransition::RepairRetryPending(_, failure)) => failure,
                    _ => FailureKind::Internal,
                };
                state.pending = None;
                let seals_enabled_policy = state.installed.is_none();
                state.fail(map_failure(failure), seals_enabled_policy);
            }
            CandidateRepairDispatch::Scheduled { .. }
            | CandidateRepairDispatch::LimitReached
            | CandidateRepairDispatch::Terminal => {
                state.pending = None;
                state.fail(BlockerCatalogFailure::Internal, true);
            }
        }
        dispatch
    }

    fn drive_source_material_repair(&self, update: Option<&StatusSnapshot>) {
        let requested = {
            let mut state = self.lock_state();
            let Some(SourceMaterialRepair::Requested(identity)) =
                state.source_material_repair.clone()
            else {
                return;
            };
            if state.terminal_failure.is_some()
                || state.installed.as_ref() != Some(&identity)
                || state.candidate.is_some()
                || state.pending.is_some()
                || state.transition.is_some()
            {
                return;
            }
            match update.map(|update| &update.status) {
                Some(UpdateStatus::Refreshing { operation, current })
                    if current.as_ref().map(identity_for_availability) == Some(&identity) =>
                {
                    state.automatic_source_material_repair = Some(identity.clone());
                    state.source_material_repair = Some(SourceMaterialRepair::InFlight {
                        identity,
                        operation: *operation,
                    });
                    return;
                }
                Some(UpdateStatus::Ready(current))
                    if identity_for_availability(current) == &identity => {}
                Some(UpdateStatus::Failed { current, .. })
                    if current.as_ref().map(identity_for_availability) == Some(&identity) => {}
                Some(
                    UpdateStatus::Unavailable(_)
                    | UpdateStatus::Idle
                    | UpdateStatus::Ready(_)
                    | UpdateStatus::Refreshing { .. }
                    | UpdateStatus::Failed { .. }
                    | UpdateStatus::Shutdown,
                )
                | None => {
                    state.fail(BlockerCatalogFailure::Internal, true);
                    return;
                }
            }
            identity
        };

        let admission = self.lock_updater().as_ref().map_or(
            RefreshAdmission::Shutdown,
            CatalogUpdateWorker::request_refresh,
        );
        let mut state = self.lock_state();
        if !matches!(
            state.source_material_repair,
            Some(SourceMaterialRepair::Requested(ref identity)) if *identity == requested
        ) {
            if matches!(admission, RefreshAdmission::Accepted(_)) {
                state.fail(BlockerCatalogFailure::Internal, true);
            }
            return;
        }
        match admission {
            RefreshAdmission::Accepted(operation) => {
                state.automatic_source_material_repair = Some(requested.clone());
                state.source_material_repair = Some(SourceMaterialRepair::InFlight {
                    identity: requested,
                    operation,
                });
            }
            RefreshAdmission::Busy => {}
            RefreshAdmission::Unavailable(_) | RefreshAdmission::Shutdown => {
                state.fail(BlockerCatalogFailure::Internal, true);
            }
        }
    }

    fn drive_automatic_refresh(&self, update: Option<&StatusSnapshot>) {
        let Some(update) = update else {
            return;
        };
        let now = now_unix();
        let persisted_due = self
            .lock_updater()
            .as_ref()
            .and_then(CatalogUpdateWorker::next_refresh_unix);
        let due = {
            let state = self.lock_state();
            state.terminal_failure.is_none()
                && state.pending.is_none()
                && state.transition.is_none()
                && !matches!(
                    state.source_material_repair,
                    Some(SourceMaterialRepair::RetryPending(_))
                )
                && persisted_due
                    .or(state.next_automatic_refresh_unix)
                    .is_some_and(|deadline| deadline <= now)
                && !matches!(
                    update.status,
                    UpdateStatus::Unavailable(_)
                        | UpdateStatus::Refreshing { .. }
                        | UpdateStatus::Shutdown
                )
        };
        if !due {
            return;
        }
        let admission = {
            let updater = self.lock_updater();
            updater.as_ref().map(CatalogUpdateWorker::request_refresh)
        };
        let mut state = self.lock_state();
        match admission {
            Some(RefreshAdmission::Accepted(_)) | Some(RefreshAdmission::Busy) => {
                state.next_automatic_refresh_unix = None;
            }
            Some(RefreshAdmission::Unavailable(_)) | Some(RefreshAdmission::Shutdown) | None => {}
        }
    }
}

impl ServiceState {
    fn candidate_identity(&self) -> Option<&CatalogIdentity> {
        self.candidate.as_ref()
    }

    fn transition_identity(&self) -> Option<&CatalogIdentity> {
        match self.transition.as_ref() {
            Some(
                CatalogTransition::Preparing(identity)
                | CatalogTransition::ReadyToRepair(identity)
                | CatalogTransition::Repairing(identity)
                | CatalogTransition::RepairRetryPending(identity, _)
                | CatalogTransition::ReadyToCommit(identity)
                | CatalogTransition::Committing(identity)
                | CatalogTransition::ReadyToActivate(identity)
                | CatalogTransition::Activating(identity)
                | CatalogTransition::ReadyToReject(identity, _, _)
                | CatalogTransition::Rejecting(identity, _)
                | CatalogTransition::ReadyToDiscard(identity, _)
                | CatalogTransition::Discarding(identity, _),
            ) => Some(identity),
            None => None,
        }
    }

    fn apply_transition_completion(&mut self, completion: CatalogTransitionCompletion) {
        let transition = self.transition.take();
        match (transition, completion) {
            (
                Some(CatalogTransition::Preparing(expected)),
                CatalogTransitionCompletion::NativeUnavailable(completed),
            ) if expected == completed => {
                let attempts = self
                    .native_retry
                    .as_ref()
                    .filter(|(identity, _, _)| identity == &completed)
                    .map_or(0, |(_, attempts, _)| *attempts);
                if attempts < 3 {
                    self.native_retry = Some((
                        completed,
                        attempts + 1,
                        Instant::now() + Duration::from_secs(30),
                    ));
                    self.transition = None;
                } else {
                    self.native_retry = None;
                    self.pending = None;
                    self.transition = Some(CatalogTransition::ReadyToReject(
                        completed,
                        FailureKind::Catalog,
                        CandidateRejectionReason::CompilerPolicy,
                    ));
                }
            }
            (
                Some(CatalogTransition::Preparing(expected)),
                CatalogTransitionCompletion::Prepared(completed, outcome),
            ) if expected == completed => match outcome {
                CatalogPreparationOutcome::Prepared => {
                    self.native_retry = None;
                    self.pending = None;
                    self.transition = Some(CatalogTransition::ReadyToCommit(completed));
                }
                CatalogPreparationOutcome::Failed(failure) => match failure {
                    zephium_core::blocker::BlockerCompileFailure::SourceUnavailable => {
                        if self.repaired_candidate.as_ref() == Some(&completed) {
                            self.pending = None;
                            let seals_enabled_policy = self.installed.is_none();
                            self.fail(BlockerCatalogFailure::Storage, seals_enabled_policy);
                        } else {
                            self.transition = Some(CatalogTransition::ReadyToRepair(completed));
                        }
                    }
                    zephium_core::blocker::BlockerCompileFailure::InvalidSource
                    | zephium_core::blocker::BlockerCompileFailure::ResourceLimit => {
                        self.pending = None;
                        self.transition = Some(CatalogTransition::ReadyToReject(
                            completed,
                            FailureKind::Catalog,
                            CandidateRejectionReason::CompilerPolicy,
                        ));
                    }
                    zephium_core::blocker::BlockerCompileFailure::Internal => {
                        self.pending = None;
                        self.fail(BlockerCatalogFailure::Internal, true);
                    }
                },
            },
            (
                Some(CatalogTransition::Repairing(expected)),
                CatalogTransitionCompletion::Repaired(completed, outcome),
            ) if expected == completed => match outcome {
                CandidateRepairOutcome::Repaired
                    if self
                        .pending
                        .as_ref()
                        .is_some_and(|pending| pending.identity == completed) =>
                {
                    self.repaired_candidate = Some(completed);
                }
                CandidateRepairOutcome::Repaired => {
                    self.pending = None;
                    self.fail(BlockerCatalogFailure::Internal, true);
                }
                CandidateRepairOutcome::Superseded(superseding)
                    if superseding.identity.revision > completed.revision =>
                {
                    let superseding_identity = superseding.identity.clone();
                    let now = now_unix();
                    self.admit_activation(superseding, now);
                    if self.candidate.as_ref() != Some(&superseding_identity) {
                        self.pending = None;
                        self.fail(BlockerCatalogFailure::Internal, true);
                    } else if superseding_identity.expires_unix <= now {
                        self.pending = None;
                        self.transition = Some(CatalogTransition::ReadyToReject(
                            superseding_identity,
                            FailureKind::Manifest,
                            CandidateRejectionReason::Expired,
                        ));
                    } else if !self
                        .pending
                        .as_ref()
                        .is_some_and(|pending| pending.identity == superseding_identity)
                    {
                        self.pending = None;
                        self.fail(BlockerCatalogFailure::Internal, true);
                    }
                }
                CandidateRepairOutcome::Superseded(_) => {
                    self.pending = None;
                    self.fail(BlockerCatalogFailure::Internal, true);
                }
                CandidateRepairOutcome::Failed(failure) => {
                    if failure.candidate_repair_retryable()
                        && self
                            .pending
                            .as_ref()
                            .is_some_and(|pending| pending.identity == completed)
                    {
                        self.transition =
                            Some(CatalogTransition::RepairRetryPending(completed, failure));
                    } else {
                        self.pending = None;
                        let seals_enabled_policy = self.installed.is_none()
                            || matches!(failure, FailureKind::Rollback | FailureKind::Internal);
                        self.fail(map_failure(failure), seals_enabled_policy);
                    }
                }
            },
            (
                Some(CatalogTransition::Committing(expected)),
                CatalogTransitionCompletion::Committed(completed, outcome),
            ) if expected == completed => match outcome {
                CandidateCommitOutcome::Committed => {
                    self.transition = Some(CatalogTransition::ReadyToActivate(completed));
                }
                CandidateCommitOutcome::Failed(failure) => {
                    let terminal = (failure != FailureKind::Manifest).then(|| map_failure(failure));
                    self.transition = Some(CatalogTransition::ReadyToDiscard(completed, terminal));
                }
            },
            (
                Some(CatalogTransition::Activating(expected)),
                CatalogTransitionCompletion::Activated(completed, success),
            ) if expected == completed => {
                if success {
                    self.installed = Some(completed.clone());
                    if self.candidate.as_ref() == Some(&completed) {
                        self.candidate = None;
                    }
                } else {
                    self.fail(BlockerCatalogFailure::Internal, true);
                }
            }
            (
                Some(CatalogTransition::Rejecting(expected, expected_failure)),
                CatalogTransitionCompletion::Rejected(completed, outcome),
            ) if expected == completed => {
                match outcome {
                    CandidateRejectOutcome::Rejected => {
                        if self.candidate.as_ref() == Some(&completed) {
                            self.candidate = None;
                        }
                    }
                    CandidateRejectOutcome::Failed(failure) => {
                        self.fail(
                            map_failure(failure),
                            matches!(failure, FailureKind::Rollback | FailureKind::Internal),
                        );
                    }
                }
                if expected_failure == FailureKind::Internal {
                    self.fail(BlockerCatalogFailure::Internal, true);
                }
            }
            (
                Some(CatalogTransition::Discarding(expected, terminal)),
                CatalogTransitionCompletion::Discarded(completed, discarded),
            ) if expected == completed => {
                if discarded {
                    if terminal.is_none() && self.candidate.as_ref() == Some(&completed) {
                        self.candidate = None;
                    }
                    if let Some(failure) = terminal {
                        self.fail(
                            failure,
                            matches!(
                                failure,
                                BlockerCatalogFailure::Rollback | BlockerCatalogFailure::Internal
                            ),
                        );
                    }
                } else {
                    self.fail(BlockerCatalogFailure::Internal, true);
                }
            }
            (
                transition,
                CatalogTransitionCompletion::Prepared(
                    completed,
                    CatalogPreparationOutcome::Prepared,
                ),
            ) if self.candidate.as_ref() == Some(&completed) => {
                let _ = transition;
                self.transition = Some(CatalogTransition::ReadyToDiscard(
                    completed,
                    Some(BlockerCatalogFailure::Internal),
                ));
            }
            (transition, _) => {
                self.transition = transition;
                self.fail(BlockerCatalogFailure::Internal, true);
            }
        }
    }

    fn fail(&mut self, failure: BlockerCatalogFailure, seals_enabled_policy: bool) {
        self.terminal_failure = Some(failure);
        self.enabled_policy_terminal |= seals_enabled_policy;
        if seals_enabled_policy {
            self.source_material_repair = None;
        }
    }

    fn admit_source_material_failure(&mut self, identity: CatalogIdentity) {
        if self.terminal_failure.is_some()
            || self.enabled_policy_terminal
            || self.installed.as_ref() != Some(&identity)
            || self
                .source_material_repair
                .as_ref()
                .is_some_and(|repair| repair.source_identity() == &identity)
        {
            return;
        }
        self.source_material_repair = Some(
            if self.automatic_source_material_repair.as_ref() == Some(&identity) {
                SourceMaterialRepair::RetryPending(identity)
            } else {
                SourceMaterialRepair::Requested(identity)
            },
        );
    }

    fn observe_source_material_repair(&mut self, update: &StatusSnapshot) {
        let Some(SourceMaterialRepair::InFlight {
            identity,
            operation,
        }) = self.source_material_repair.clone()
        else {
            return;
        };
        match &update.status {
            UpdateStatus::Refreshing {
                operation: observed,
                current,
            } if *observed == operation
                && current.as_ref().map(identity_for_availability) == Some(&identity) => {}
            UpdateStatus::Ready(current)
                if identity_for_availability(current) == &identity
                    && self.installed.as_ref() == Some(&identity) =>
            {
                let Some(epoch) = self.source_material_epoch.checked_add(1) else {
                    self.source_material_repair = None;
                    self.fail(BlockerCatalogFailure::Internal, true);
                    return;
                };
                self.source_material_epoch = epoch;
                self.source_material_repair = None;
            }
            UpdateStatus::Failed {
                operation: observed,
                failure,
                current,
            } if *observed == operation
                && current.as_ref().map(identity_for_availability) == Some(&identity) =>
            {
                if failure.source_material_repair_retryable() {
                    self.source_material_repair =
                        Some(SourceMaterialRepair::RetryPending(identity));
                } else {
                    self.source_material_repair = None;
                    self.fail(map_failure(*failure), true);
                }
            }
            UpdateStatus::Ready(_)
                if self.candidate.is_some()
                    || self
                        .installed
                        .as_ref()
                        .is_some_and(|installed| installed.revision > identity.revision) =>
            {
                self.source_material_repair = None;
            }
            UpdateStatus::Unavailable(_)
            | UpdateStatus::Idle
            | UpdateStatus::Refreshing { .. }
            | UpdateStatus::Failed { .. }
            | UpdateStatus::Ready(_)
            | UpdateStatus::Shutdown => {
                self.source_material_repair = None;
                self.fail(BlockerCatalogFailure::Internal, true);
            }
        }
    }

    fn expire_retry_candidate(&mut self, now: u64) {
        let Some(CatalogTransition::RepairRetryPending(identity, _)) = self.transition.as_ref()
        else {
            return;
        };
        if identity.expires_unix > now || self.candidate.as_ref() != Some(identity) {
            return;
        }
        let identity = identity.clone();
        self.pending = None;
        self.transition = Some(CatalogTransition::ReadyToReject(
            identity,
            FailureKind::Manifest,
            CandidateRejectionReason::Expired,
        ));
    }

    fn admit_activation(&mut self, activation: ActivatedCatalog, now: u64) {
        if self
            .source_material_repair
            .as_ref()
            .is_some_and(|repair| activation.identity.revision > repair.source_identity().revision)
        {
            self.source_material_repair = None;
        }
        let supersedes_repair = self.transition.as_ref().is_some_and(|transition| {
            matches!(
                transition,
                CatalogTransition::Repairing(repairing)
                    if activation.identity.revision > repairing.revision
            )
        });
        let newest = self
            .candidate
            .as_ref()
            .or_else(|| self.pending.as_ref().map(|value| &value.identity))
            .or_else(|| self.transition_identity())
            .or(self.installed.as_ref());
        if let Some(current) = newest {
            if activation.identity.revision < current.revision
                || (activation.identity.revision == current.revision
                    && activation.identity != *current)
            {
                self.fail(BlockerCatalogFailure::Rollback, true);
                return;
            }
            if activation.identity == *current {
                return;
            }
        }
        if self.repaired_candidate.as_ref() != Some(&activation.identity) {
            self.repaired_candidate = None;
        }
        self.native_retry = None;
        self.candidate = Some(activation.identity.clone());
        if activation.identity.expires_unix <= now {
            self.pending = None;
            if !supersedes_repair {
                self.transition = Some(CatalogTransition::ReadyToReject(
                    activation.identity,
                    FailureKind::Manifest,
                    CandidateRejectionReason::Expired,
                ));
            }
        } else {
            self.pending = Some(activation);
        }
    }

    fn observe_update(&mut self, update: &StatusSnapshot, now: u64) {
        self.observe_source_material_repair(update);
        if self
            .observed_update
            .as_ref()
            .is_some_and(|current| current.revision == update.revision)
        {
            return;
        }
        let previous = self.observed_update.replace(update.clone());
        match &update.status {
            UpdateStatus::Failed { operation, .. } => {
                let is_new_failure = !previous.as_ref().is_some_and(|previous| {
                    matches!(
                        previous.status,
                        UpdateStatus::Failed {
                            operation: previous_operation,
                            ..
                        } if previous_operation == *operation
                    )
                });
                if is_new_failure {
                    self.failure_streak = self.failure_streak.saturating_add(1);
                }
                let index = self
                    .failure_streak
                    .saturating_sub(1)
                    .min(FAILURE_BACKOFF.len() - 1);
                self.next_automatic_refresh_unix = checked_deadline(now, FAILURE_BACKOFF[index]);
            }
            UpdateStatus::Ready(availability) => {
                self.failure_streak = 0;
                self.next_automatic_refresh_unix = ready_refresh_deadline(
                    availability,
                    update.last_refresh_attempt_unix,
                    now,
                    self.refresh_jitter,
                );
            }
            UpdateStatus::Idle => {
                self.next_automatic_refresh_unix = update
                    .last_refresh_attempt_unix
                    .and_then(|attempt| checked_deadline(attempt, FAILURE_BACKOFF[0]))
                    .or(Some(now));
            }
            UpdateStatus::Refreshing { .. } => {
                self.next_automatic_refresh_unix = None;
            }
            UpdateStatus::Unavailable(_) | UpdateStatus::Shutdown => {
                self.next_automatic_refresh_unix = None;
            }
        }
    }

    #[cfg(any(feature = "tuf", feature = "official-https"))]
    fn recompute_schedule(&mut self, update: &StatusSnapshot, now: u64) {
        self.observed_update = None;
        self.observe_update(update, now);
    }

    fn observe_official_verification(
        &mut self,
        update: &StatusSnapshot,
        freshness: Option<(u64, bool)>,
    ) {
        let Some((verification, _)) = freshness else {
            return;
        };
        let previous = self.official_verification.replace(verification);
        let current = match &update.status {
            UpdateStatus::Ready(value) => Some(identity_for_availability(value)),
            _ => None,
        };
        if previous.is_some_and(|previous| verification > previous)
            && current == self.installed.as_ref()
            && current.is_some_and(|current| {
                self.snapshot.package_manifest_sha256 == Some(current.manifest_sha256)
                    && self.snapshot.installed_manifest_sha256 == Some(current.manifest_sha256)
            })
        {
            if let Some(epoch) = self.source_material_epoch.checked_add(1) {
                self.source_material_epoch = epoch;
            } else {
                self.fail(BlockerCatalogFailure::Internal, true);
            }
        }
    }

    fn publish(&mut self, update: &StatusSnapshot) {
        self.publish_with_supply(update, None);
    }
    fn publish_with_supply(&mut self, update: &StatusSnapshot, official: Option<([u8; 32], bool)>) {
        let activation_pending = self.candidate.is_some();
        let mut next = snapshot_from_update(
            update,
            self.installed.as_ref(),
            self.candidate_identity(),
            activation_pending,
        );
        next.repair_retry_pending = matches!(
            self.transition,
            Some(CatalogTransition::RepairRetryPending(_, _))
        );
        next.source_material_epoch = self.source_material_epoch;
        next.source_material_repair_pending = matches!(
            self.source_material_repair,
            Some(SourceMaterialRepair::Requested(_) | SourceMaterialRepair::InFlight { .. })
        );
        next.source_material_repair_retry_pending = matches!(
            self.source_material_repair,
            Some(SourceMaterialRepair::RetryPending(_))
        );
        if let Some(failure) = self.terminal_failure {
            next.phase = BlockerCatalogPhase::Failed(failure);
        } else if matches!(next.phase, BlockerCatalogPhase::Failed(_))
            && next.refresh_operation.is_some()
            && self.transition.as_ref().is_some_and(|transition| {
                !matches!(transition, CatalogTransition::RepairRetryPending(_, _))
            })
        {
            // A failed updater operation can still own exact asynchronous
            // compiler cleanup. Keep polling semantics live until that
            // transition settles; only explicit-wait repair state is idle.
            next.phase = BlockerCatalogPhase::Refreshing;
        }
        next.enabled_policy_terminal = self.enabled_policy_terminal;
        if let Some((fallback, due)) = official {
            let provenance = |digest: Option<[u8; 32]>| {
                digest.map(|digest| {
                    if digest == fallback {
                        BlockerCatalogProvenance::ReleaseBundle
                    } else {
                        BlockerCatalogProvenance::OfficialHttps
                    }
                })
            };
            next.package_provenance = provenance(next.package_manifest_sha256);
            next.installed_provenance = provenance(next.installed_manifest_sha256);
            next.candidate_provenance = provenance(next.candidate_manifest_sha256);
            let available = match &update.status {
                UpdateStatus::Ready(value) => Some(value),
                UpdateStatus::Refreshing { current, .. } | UpdateStatus::Failed { current, .. } => {
                    current.as_ref()
                }
                _ => None,
            };
            let stale =
                available.is_some_and(|value| matches!(value, CatalogAvailability::Stale(_)));
            next.package_stale = next.package_revision.map(|_| {
                stale && next.package_provenance != Some(BlockerCatalogProvenance::ReleaseBundle)
            });
            next.source_refresh_due = next.package_revision.is_some() && due;
            if next.package_provenance == Some(BlockerCatalogProvenance::ReleaseBundle) {
                next.source_refresh_due |= self.snapshot.source_refresh_due
                    || next
                        .package_expires_unix
                        .is_some_and(|expiry| expiry <= now_unix());
            }
            if matches!(
                next.phase,
                BlockerCatalogPhase::Fresh | BlockerCatalogPhase::Stale
            ) {
                next.phase = if next.package_stale == Some(true) {
                    BlockerCatalogPhase::Stale
                } else {
                    BlockerCatalogPhase::Fresh
                };
            }
        }
        self.replace_snapshot(next);
    }

    fn publish_release_seed(&mut self, now: u64) {
        let Some(identity) = self.installed.as_ref() else {
            self.fail(BlockerCatalogFailure::Internal, true);
            return;
        };
        // Source currency is advisory, but it must still be monotonic for one
        // immutable bundled identity. A wall-clock rollback cannot make an
        // already-observed refresh recommendation disappear.
        let source_refresh_due = self.snapshot.source_refresh_due || identity.expires_unix <= now;
        let mut next = BlockerCatalogSnapshot {
            revision: 1,
            phase: BlockerCatalogPhase::Fresh,
            enabled_policy_terminal: self.enabled_policy_terminal,
            package_revision: Some(identity.revision),
            package_manifest_sha256: Some(identity.manifest_sha256),
            package_provenance: Some(BlockerCatalogProvenance::ReleaseBundle),
            package_created_unix: Some(identity.created_unix),
            package_expires_unix: Some(identity.expires_unix),
            package_stale: Some(false),
            source_refresh_due,
            source_count: Some(identity.source_count),
            source_bytes: Some(identity.source_bytes),
            candidate_revision: None,
            candidate_manifest_sha256: None,
            candidate_provenance: None,
            candidate_created_unix: None,
            candidate_expires_unix: None,
            candidate_source_count: None,
            candidate_source_bytes: None,
            installed_revision: Some(identity.revision),
            installed_manifest_sha256: Some(identity.manifest_sha256),
            installed_provenance: Some(BlockerCatalogProvenance::ReleaseBundle),
            refresh_supported: false,
            source_material_epoch: self.source_material_epoch,
            source_material_repair_pending: false,
            source_material_repair_retry_pending: false,
            activation_pending: false,
            repair_retry_pending: false,
            last_refresh_attempt_unix: None,
            refresh_operation: None,
        };
        if let Some(failure) = self.terminal_failure {
            next.phase = BlockerCatalogPhase::Failed(failure);
        }
        self.replace_snapshot(next);
    }

    fn publish_shutdown(&mut self) {
        self.enabled_policy_terminal = true;
        let mut next = self.snapshot;
        next.phase = BlockerCatalogPhase::Shutdown;
        next.enabled_policy_terminal = true;
        next.candidate_revision = None;
        next.candidate_manifest_sha256 = None;
        next.candidate_provenance = None;
        next.candidate_created_unix = None;
        next.candidate_expires_unix = None;
        next.candidate_source_count = None;
        next.candidate_source_bytes = None;
        next.activation_pending = false;
        next.repair_retry_pending = false;
        next.source_material_repair_pending = false;
        next.source_material_repair_retry_pending = false;
        next.refresh_operation = None;
        self.replace_snapshot(next);
    }

    fn replace_snapshot(&mut self, mut next: BlockerCatalogSnapshot) {
        next.revision = self.snapshot.revision;
        if next == self.snapshot {
            return;
        }
        let Some(revision) = self.snapshot.revision.checked_add(1) else {
            self.snapshot.revision = u64::MAX;
            self.snapshot.phase = BlockerCatalogPhase::Shutdown;
            self.snapshot.enabled_policy_terminal = true;
            self.snapshot.candidate_revision = None;
            self.snapshot.candidate_manifest_sha256 = None;
            self.snapshot.candidate_created_unix = None;
            self.snapshot.candidate_expires_unix = None;
            self.snapshot.candidate_source_count = None;
            self.snapshot.candidate_source_bytes = None;
            self.snapshot.activation_pending = false;
            self.snapshot.repair_retry_pending = false;
            self.snapshot.source_material_repair_pending = false;
            self.snapshot.source_material_repair_retry_pending = false;
            self.snapshot.refresh_operation = None;
            self.fail(BlockerCatalogFailure::Internal, true);
            return;
        };
        next.revision = revision;
        self.snapshot = next;
    }
}

impl BlockerCatalog for ManagedBlocker {
    fn maintain(&self) -> BlockerCatalogSnapshot {
        let _serial = self
            .maintenance_serial
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        self.drive_locked(true)
    }

    fn request_refresh(&self) -> BlockerCatalogRefreshDispatch {
        let _serial = self
            .maintenance_serial
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let snapshot = self.drive_locked(false);
        if self.sealed.load(Ordering::Acquire) || self.lock_state().terminal_failure.is_some() {
            return BlockerCatalogRefreshDispatch::Terminal;
        }
        if self.supply == SupplyMode::ReleaseSeed {
            return BlockerCatalogRefreshDispatch::Unsupported;
        }
        let repair_retry = {
            let state = self.lock_state();
            match state.transition.as_ref() {
                Some(CatalogTransition::RepairRetryPending(identity, _)) => Some(identity.clone()),
                _ => None,
            }
        };
        if let Some(identity) = repair_retry {
            return match self.schedule_candidate_repair(identity, true) {
                CandidateRepairDispatch::Scheduled { operation } => {
                    // Publish the exact repair operation before returning
                    // acceptance. It remains refreshing through compiler
                    // preparation, durable commit, and exact activation.
                    let _ = self.drive_locked(false);
                    BlockerCatalogRefreshDispatch::Accepted { operation }
                }
                CandidateRepairDispatch::Rejected => BlockerCatalogRefreshDispatch::Busy,
                CandidateRepairDispatch::LimitReached => {
                    BlockerCatalogRefreshDispatch::RetryLimitReached
                }
                CandidateRepairDispatch::Terminal => BlockerCatalogRefreshDispatch::Terminal,
            };
        }
        let source_material_retry = {
            let state = self.lock_state();
            match state.source_material_repair.as_ref() {
                Some(SourceMaterialRepair::RetryPending(identity)) => Some(identity.clone()),
                _ => None,
            }
        };
        if let Some(identity) = source_material_retry {
            let admission = self.lock_updater().as_ref().map_or(
                RefreshAdmission::Shutdown,
                CatalogUpdateWorker::request_refresh,
            );
            return match admission {
                RefreshAdmission::Accepted(operation) => {
                    let mut state = self.lock_state();
                    if matches!(
                        state.source_material_repair,
                        Some(SourceMaterialRepair::RetryPending(ref pending))
                            if *pending == identity
                    ) {
                        state.source_material_repair = Some(SourceMaterialRepair::InFlight {
                            identity,
                            operation,
                        });
                        drop(state);
                        let _ = self.drive_locked(false);
                        BlockerCatalogRefreshDispatch::Accepted { operation }
                    } else {
                        state.fail(BlockerCatalogFailure::Internal, true);
                        BlockerCatalogRefreshDispatch::Terminal
                    }
                }
                RefreshAdmission::Busy => BlockerCatalogRefreshDispatch::Busy,
                RefreshAdmission::Unavailable(reason) => {
                    BlockerCatalogRefreshDispatch::Unavailable(map_unavailable(reason))
                }
                RefreshAdmission::Shutdown => BlockerCatalogRefreshDispatch::Terminal,
            };
        }
        if snapshot.activation_pending {
            return BlockerCatalogRefreshDispatch::Busy;
        }
        let admission = {
            let updater = self.lock_updater();
            let Some(updater) = updater.as_ref() else {
                return BlockerCatalogRefreshDispatch::Unavailable(
                    BlockerCatalogUnavailable::NotConfigured,
                );
            };
            updater.request_refresh()
        };
        match admission {
            RefreshAdmission::Accepted(operation) => {
                // Publish the updater's Refreshing transition in this same
                // serialized admission call; privileged status never needs
                // to wait for a later heartbeat to learn that work started.
                let _ = self.drive_locked(false);
                BlockerCatalogRefreshDispatch::Accepted { operation }
            }
            RefreshAdmission::Busy => BlockerCatalogRefreshDispatch::Busy,
            RefreshAdmission::Unavailable(reason) => {
                BlockerCatalogRefreshDispatch::Unavailable(map_unavailable(reason))
            }
            RefreshAdmission::Shutdown => BlockerCatalogRefreshDispatch::Terminal,
        }
    }
}

impl BlockerCompiler for ManagedBlocker {
    fn prepare_site_preferences(
        &self,
        preferences: &zephium_core::blocker::BlockerSitePreferences,
    ) -> Option<Arc<zephium_core::blocker::PreparedBlockerSites>> {
        zephium_blocker::prepare_site_preferences(preferences)
    }
    fn validate_personal_selector(&self, selector: &str) -> Option<String> {
        zephium_blocker::validate_personal_selector(selector)
    }
    fn compile(
        &self,
        profile: ProfileId,
        generation: ContentPolicyGeneration,
        config: BlockerConfig,
        done: Box<dyn FnOnce(BlockerCompileOutcome) + Send>,
    ) -> BlockerDispatch {
        if self.sealed.load(Ordering::Acquire) {
            BlockerDispatch::Terminal
        } else if config.enabled && self.lock_state().enabled_policy_terminal {
            BlockerDispatch::EnabledPolicyTerminal
        } else {
            let installed = if config.enabled {
                self.lock_state().installed.clone()
            } else {
                None
            };
            let signal = Arc::clone(&self.source_material_signal);
            self.compiler.compile(
                profile,
                generation,
                config,
                Box::new(move |outcome| {
                    if matches!(
                        &outcome,
                        BlockerCompileOutcome::Failed(BlockerCompileFailure::SourceUnavailable)
                    ) {
                        if let Some(identity) = installed {
                            signal.report(identity);
                        }
                    }
                    done(outcome);
                }),
            )
        }
    }

    fn retire_profile(
        &self,
        profile: ProfileId,
        done: Box<dyn FnOnce() + Send>,
    ) -> BlockerRetirementDispatch {
        if self.sealed.load(Ordering::Acquire) {
            BlockerRetirementDispatch::Terminal
        } else {
            self.compiler.retire_profile(profile, done)
        }
    }

    fn shutdown_until(&self, deadline: Instant) -> BlockerShutdownOutcome {
        let Some(_serial) = lock_until(&self.shutdown_serial, deadline) else {
            return BlockerShutdownOutcome::Unclean;
        };
        let Some(result) = lock_until(&self.shutdown_result, deadline) else {
            return BlockerShutdownOutcome::Unclean;
        };
        if let Some(outcome) = *result {
            return outcome;
        }
        drop(result);
        let Some(_maintenance) = lock_until(&self.maintenance_serial, deadline) else {
            return BlockerShutdownOutcome::Unclean;
        };
        self.sealed.store(true, Ordering::Release);
        self.source_material_signal.seal();

        let (updater, updater_lock_failed) = match lock_until(&self.updater, deadline) {
            Some(mut updater) => (updater.take(), false),
            None => (None, true),
        };
        let had_updater = updater.is_some();
        let updater_slot = updater.map(|updater| Arc::new(Mutex::new(Some(updater))));
        let updater_thread = updater_slot.as_ref().and_then(|slot| {
            let slot = Arc::clone(slot);
            thread::Builder::new()
                .name("zephium-blocker-update-shutdown".into())
                .spawn(move || {
                    let updater = slot
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .take();
                    updater.map(|updater| {
                        updater.shutdown(deadline.saturating_duration_since(Instant::now()))
                    })
                })
                .ok()
        });
        let compiler_outcome = self.compiler.shutdown_until(deadline);
        let updater_outcome = match updater_thread {
            Some(thread) => join_until(thread, deadline).flatten(),
            None => updater_slot.and_then(|slot| {
                let mut slot = lock_until(&slot, deadline)?;
                let updater = slot.take()?;
                drop(slot);
                Some(updater.shutdown(deadline.saturating_duration_since(Instant::now())))
            }),
        };
        let outcome = classify_shutdown(
            compiler_outcome,
            had_updater || updater_lock_failed,
            updater_outcome,
        );
        let Some(mut result) = lock_until(&self.shutdown_result, deadline) else {
            return BlockerShutdownOutcome::Unclean;
        };
        *result = Some(outcome);
        outcome
    }
}

fn classify_shutdown(
    compiler: BlockerShutdownOutcome,
    had_updater: bool,
    updater: Option<UpdateShutdownOutcome>,
) -> BlockerShutdownOutcome {
    let updater_clean = if had_updater {
        matches!(
            updater,
            Some(UpdateShutdownOutcome::Complete | UpdateShutdownOutcome::Unavailable)
        )
    } else {
        true
    };
    if compiler == BlockerShutdownOutcome::Clean && updater_clean {
        BlockerShutdownOutcome::Clean
    } else {
        BlockerShutdownOutcome::Unclean
    }
}

fn lock_until<T>(mutex: &Mutex<T>, deadline: Instant) -> Option<MutexGuard<'_, T>> {
    loop {
        match mutex.try_lock() {
            Ok(guard) => return Some(guard),
            Err(TryLockError::Poisoned(poisoned)) => return Some(poisoned.into_inner()),
            Err(TryLockError::WouldBlock) => {}
        }
        let remaining = deadline.checked_duration_since(Instant::now())?;
        thread::park_timeout(remaining.min(SHUTDOWN_POLL_INTERVAL));
    }
}

fn join_until<T>(thread: JoinHandle<T>, deadline: Instant) -> Option<T> {
    loop {
        if thread.is_finished() {
            return thread.join().ok();
        }
        let remaining = deadline.checked_duration_since(Instant::now())?;
        thread::park_timeout(remaining.min(SHUTDOWN_POLL_INTERVAL));
    }
}

fn snapshot_from_update(
    update: &StatusSnapshot,
    installed: Option<&CatalogIdentity>,
    candidate: Option<&CatalogIdentity>,
    activation_pending: bool,
) -> BlockerCatalogSnapshot {
    snapshot_from_update_at(update, installed, candidate, activation_pending, now_unix())
}

fn snapshot_from_update_at(
    update: &StatusSnapshot,
    installed: Option<&CatalogIdentity>,
    candidate: Option<&CatalogIdentity>,
    activation_pending: bool,
    now: u64,
) -> BlockerCatalogSnapshot {
    let (phase, current, package_stale, operation) = match &update.status {
        UpdateStatus::Unavailable(reason) => (
            BlockerCatalogPhase::Unavailable(map_unavailable(*reason)),
            installed,
            installed.map(|identity| identity.expires_unix <= now),
            None,
        ),
        UpdateStatus::Idle => (BlockerCatalogPhase::Idle, None, None, None),
        UpdateStatus::Ready(availability) => (
            phase_for_availability(availability, now),
            Some(identity_for_availability(availability)),
            Some(availability_is_stale(availability, now)),
            None,
        ),
        UpdateStatus::Refreshing { operation, current } => (
            BlockerCatalogPhase::Refreshing,
            current.as_ref().map(identity_for_availability),
            current
                .as_ref()
                .map(|value| availability_is_stale(value, now)),
            Some(*operation),
        ),
        UpdateStatus::Failed {
            operation,
            failure,
            current,
        } => (
            BlockerCatalogPhase::Failed(map_failure(*failure)),
            current.as_ref().map(identity_for_availability),
            current
                .as_ref()
                .map(|value| availability_is_stale(value, now)),
            Some(*operation),
        ),
        UpdateStatus::Shutdown => (
            BlockerCatalogPhase::Shutdown,
            installed,
            installed.map(|identity| identity.expires_unix <= now),
            None,
        ),
    };
    BlockerCatalogSnapshot {
        revision: 1,
        phase,
        enabled_policy_terminal: false,
        package_revision: current.map(|identity| identity.revision),
        package_manifest_sha256: current.map(|identity| identity.manifest_sha256),
        package_provenance: current.map(|_| BlockerCatalogProvenance::TufRepository),
        package_created_unix: current.map(|identity| identity.created_unix),
        package_expires_unix: current.map(|identity| identity.expires_unix),
        package_stale,
        source_refresh_due: current.is_some_and(|identity| identity.expires_unix <= now),
        source_count: current.map(|identity| identity.source_count),
        source_bytes: current.map(|identity| identity.source_bytes),
        candidate_revision: candidate.map(|identity| identity.revision),
        candidate_manifest_sha256: candidate.map(|identity| identity.manifest_sha256),
        candidate_provenance: candidate.map(|_| BlockerCatalogProvenance::TufRepository),
        candidate_created_unix: candidate.map(|identity| identity.created_unix),
        candidate_expires_unix: candidate.map(|identity| identity.expires_unix),
        candidate_source_count: candidate.map(|identity| identity.source_count),
        candidate_source_bytes: candidate.map(|identity| identity.source_bytes),
        installed_revision: installed.map(|identity| identity.revision),
        installed_manifest_sha256: installed.map(|identity| identity.manifest_sha256),
        installed_provenance: installed.map(|_| BlockerCatalogProvenance::TufRepository),
        refresh_supported: true,
        source_material_epoch: 0,
        source_material_repair_pending: false,
        source_material_repair_retry_pending: false,
        activation_pending,
        repair_retry_pending: false,
        last_refresh_attempt_unix: update.last_refresh_attempt_unix,
        refresh_operation: operation,
    }
}

fn phase_for_availability(availability: &CatalogAvailability, now: u64) -> BlockerCatalogPhase {
    if availability_is_stale(availability, now) {
        BlockerCatalogPhase::Stale
    } else {
        BlockerCatalogPhase::Fresh
    }
}

fn availability_is_stale(availability: &CatalogAvailability, now: u64) -> bool {
    matches!(availability, CatalogAvailability::Stale(_))
        || identity_for_availability(availability).expires_unix <= now
}

fn identity_for_availability(availability: &CatalogAvailability) -> &CatalogIdentity {
    match availability {
        CatalogAvailability::Fresh(identity) | CatalogAvailability::Stale(identity) => identity,
    }
}

fn map_unavailable(reason: UnavailableReason) -> BlockerCatalogUnavailable {
    match reason {
        UnavailableReason::DurableActivationUnsupported => {
            BlockerCatalogUnavailable::DurableActivationUnsupported
        }
        UnavailableReason::StorageUnavailable => BlockerCatalogUnavailable::StorageUnavailable,
        UnavailableReason::ClockUnsafe => BlockerCatalogUnavailable::ClockUnsafe,
    }
}

fn map_failure(failure: FailureKind) -> BlockerCatalogFailure {
    match failure {
        FailureKind::Transport => BlockerCatalogFailure::Transport,
        FailureKind::Metadata => BlockerCatalogFailure::Metadata,
        FailureKind::Clock => BlockerCatalogFailure::Clock,
        FailureKind::Manifest => BlockerCatalogFailure::Manifest,
        FailureKind::Target => BlockerCatalogFailure::Target,
        FailureKind::License => BlockerCatalogFailure::License,
        FailureKind::Rollback => BlockerCatalogFailure::Rollback,
        FailureKind::Storage => BlockerCatalogFailure::Storage,
        FailureKind::Catalog => BlockerCatalogFailure::Catalog,
        FailureKind::Internal => BlockerCatalogFailure::Internal,
    }
}

fn ready_refresh_deadline(
    availability: &CatalogAvailability,
    last_attempt: Option<u64>,
    now: u64,
    jitter: Duration,
) -> Option<u64> {
    let identity = identity_for_availability(availability);
    let last_attempt_or_now = last_attempt.unwrap_or(now);
    let periodic = checked_deadline(last_attempt_or_now, NORMAL_REFRESH_INTERVAL)?;
    let expiry_lead = identity
        .expires_unix
        .saturating_sub(EXPIRY_REFRESH_LEAD.as_secs());
    let jittered_expiry_lead =
        expiry_lead.saturating_add(jitter.min(EXPIRY_REFRESH_JITTER_WINDOW).as_secs());

    if now < jittered_expiry_lead {
        return Some(periodic.min(jittered_expiry_lead));
    }

    if last_attempt.is_none_or(|attempt| attempt < expiry_lead) {
        return Some(now);
    }

    if identity.expires_unix <= now {
        return checked_deadline(last_attempt_or_now, STALE_REFRESH_INTERVAL);
    }

    let remaining = identity.expires_unix - now;
    let interval = Duration::from_secs((remaining / 2).clamp(
        MIN_EXPIRY_REFRESH_INTERVAL.as_secs(),
        EXPIRY_REFRESH_INTERVAL.as_secs(),
    ));
    checked_deadline(last_attempt_or_now, interval)
}

fn checked_deadline(base: u64, interval: Duration) -> Option<u64> {
    base.checked_add(interval.as_secs())
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(u64::MAX, |duration| duration.as_secs())
}

#[cfg(feature = "tuf")]
fn refresh_jitter() -> Duration {
    use std::hash::{DefaultHasher, Hash, Hasher};

    let mut hasher = DefaultHasher::new();
    std::process::id().hash(&mut hasher);
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .subsec_nanos()
        .hash(&mut hasher);
    Duration::from_secs(hasher.finish() % (EXPIRY_REFRESH_JITTER_WINDOW.as_secs() + 1))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cache() -> (tempfile::TempDir, CompiledArtifactCacheConfig) {
        let root = tempfile::tempdir().unwrap();
        let config = CompiledArtifactCacheConfig::new(root.path().join("compiled")).unwrap();
        (root, config)
    }

    #[test]
    fn unconfigured_service_is_explicit_and_compiler_still_shuts_down() {
        let (_cache_root, cache) = cache();
        let service = ManagedBlocker::unconfigured(cache).unwrap();
        assert_eq!(service.maintain(), BlockerCatalogSnapshot::not_configured());
        assert_eq!(
            service.request_refresh(),
            BlockerCatalogRefreshDispatch::Unavailable(BlockerCatalogUnavailable::NotConfigured)
        );
        assert_eq!(
            service.shutdown_until(Instant::now() + Duration::from_secs(2)),
            BlockerShutdownOutcome::Clean
        );
    }

    #[test]
    fn terminal_authority_dominates_stale_explicit_retry_state() {
        let (_cache_root, cache) = cache();
        let service = ManagedBlocker::unconfigured(cache).unwrap();
        let mut candidate = identity(5, 5);
        candidate.expires_unix = u64::MAX;
        {
            let mut state = service.lock_state();
            state.candidate = Some(candidate.clone());
            state.pending = Some(ActivatedCatalog {
                identity: candidate.clone(),
                catalog: PolicyCatalog::eager(StaticPolicyCatalog::empty()),
            });
            state.transition = Some(CatalogTransition::RepairRetryPending(
                candidate,
                FailureKind::Transport,
            ));
            state.terminal_failure = Some(BlockerCatalogFailure::Internal);
        }

        assert_eq!(
            service.request_refresh(),
            BlockerCatalogRefreshDispatch::Terminal
        );
        assert_eq!(
            service.shutdown_until(Instant::now() + Duration::from_secs(2)),
            BlockerShutdownOutcome::Clean
        );
    }

    #[test]
    fn enabled_policy_terminal_is_typed_without_terminalizing_disabled_allow_all() {
        let (_cache_root, cache) = cache();
        let service = ManagedBlocker::unconfigured(cache).unwrap();
        service.lock_state().enabled_policy_terminal = true;
        let profile = ProfileId::from(1);
        let enabled_generation = ContentPolicyGeneration::new(1).unwrap();
        assert_eq!(
            service.compile(
                profile,
                enabled_generation,
                BlockerConfig { enabled: true },
                Box::new(|_| panic!("terminal enabled policy must not own completion")),
            ),
            BlockerDispatch::EnabledPolicyTerminal
        );

        let (done_tx, done_rx) = std::sync::mpsc::sync_channel(1);
        assert_eq!(
            service.compile(
                profile,
                ContentPolicyGeneration::new(2).unwrap(),
                BlockerConfig { enabled: false },
                Box::new(move |outcome| done_tx.send(outcome).unwrap()),
            ),
            BlockerDispatch::Scheduled
        );
        assert!(matches!(
            done_rx.recv_timeout(Duration::from_secs(2)).unwrap(),
            BlockerCompileOutcome::Compiled(_)
        ));
        assert_eq!(
            service.shutdown_until(Instant::now() + Duration::from_secs(2)),
            BlockerShutdownOutcome::Clean
        );
    }

    #[test]
    fn shutdown_never_claims_clean_when_an_owned_updater_lacks_exit_proof() {
        assert_eq!(
            classify_shutdown(BlockerShutdownOutcome::Clean, true, None),
            BlockerShutdownOutcome::Unclean
        );
        assert_eq!(
            classify_shutdown(
                BlockerShutdownOutcome::Clean,
                true,
                Some(UpdateShutdownOutcome::TimedOut),
            ),
            BlockerShutdownOutcome::Unclean
        );
        assert_eq!(
            classify_shutdown(
                BlockerShutdownOutcome::Clean,
                true,
                Some(UpdateShutdownOutcome::Complete),
            ),
            BlockerShutdownOutcome::Clean
        );
    }

    #[test]
    fn shutdown_helper_requires_exact_thread_exit_before_deadline() {
        let (ack_tx, ack_rx) = std::sync::mpsc::sync_channel(1);
        let (release_tx, release_rx) = std::sync::mpsc::sync_channel(1);
        let (exited_tx, exited_rx) = std::sync::mpsc::sync_channel(1);
        let helper = thread::spawn(move || {
            ack_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            exited_tx.send(()).unwrap();
            UpdateShutdownOutcome::Complete
        });
        ack_rx.recv_timeout(Duration::from_secs(1)).unwrap();

        assert_eq!(
            join_until(helper, Instant::now() + Duration::from_millis(25)),
            None
        );
        release_tx.send(()).unwrap();
        exited_rx.recv_timeout(Duration::from_secs(1)).unwrap();
    }

    #[test]
    fn shutdown_serialization_obeys_the_callers_absolute_deadline() {
        let (_cache_root, cache) = cache();
        let service = ManagedBlocker::unconfigured(cache).unwrap();
        let held = service.shutdown_serial.lock().unwrap();
        let started = Instant::now();
        assert_eq!(
            service.shutdown_until(started + Duration::from_millis(25)),
            BlockerShutdownOutcome::Unclean
        );
        assert!(started.elapsed() < Duration::from_millis(250));
        drop(held);
        assert_eq!(
            service.shutdown_until(Instant::now() + Duration::from_secs(2)),
            BlockerShutdownOutcome::Clean
        );
    }

    #[test]
    fn concurrent_and_repeated_service_shutdown_share_exact_exit_proof() {
        let (_cache_root, cache) = cache();
        let service = ManagedBlocker::unconfigured(cache).unwrap();
        let barrier = Arc::new(std::sync::Barrier::new(3));
        let shutdowns = (0..2)
            .map(|_| {
                let service = Arc::clone(&service);
                let barrier = Arc::clone(&barrier);
                thread::spawn(move || {
                    barrier.wait();
                    service.shutdown_until(Instant::now() + Duration::from_secs(2))
                })
            })
            .collect::<Vec<_>>();
        barrier.wait();
        for shutdown in shutdowns {
            assert_eq!(shutdown.join().unwrap(), BlockerShutdownOutcome::Clean);
        }
        assert_eq!(
            service.shutdown_until(Instant::now()),
            BlockerShutdownOutcome::Clean
        );
    }

    #[test]
    fn refresh_deadline_is_due_when_the_expiry_window_was_never_observed() {
        let availability = CatalogAvailability::Fresh(CatalogIdentity {
            revision: 1,
            manifest_sha256: [1; 32],
            created_unix: 10,
            expires_unix: 10_000,
            source_count: 1,
            source_bytes: 10,
        });
        assert_eq!(
            ready_refresh_deadline(&availability, None, 100, Duration::ZERO),
            Some(100)
        );
    }

    #[test]
    fn successful_refresh_inside_expiry_window_has_a_bounded_minimum_interval() {
        let now = 100_000;
        let availability = CatalogAvailability::Fresh(CatalogIdentity {
            revision: 1,
            manifest_sha256: [1; 32],
            created_unix: 10,
            expires_unix: now + 12 * 60 * 60,
            source_count: 1,
            source_bytes: 10,
        });
        assert_eq!(
            ready_refresh_deadline(&availability, Some(now), now, Duration::ZERO),
            Some(now + EXPIRY_REFRESH_INTERVAL.as_secs())
        );
    }

    #[test]
    fn successful_refresh_in_the_final_hour_never_becomes_a_heartbeat_loop() {
        let expires = 100_000;
        let now = expires - 30 * 60;
        let availability = CatalogAvailability::Fresh(CatalogIdentity {
            revision: 1,
            manifest_sha256: [1; 32],
            created_unix: 10,
            expires_unix: expires,
            source_count: 1,
            source_bytes: 10,
        });
        assert_eq!(
            ready_refresh_deadline(&availability, Some(now), now, Duration::ZERO),
            Some(now + MIN_EXPIRY_REFRESH_INTERVAL.as_secs())
        );
    }

    #[test]
    fn expiry_lead_is_locally_jittered_within_its_fixed_window() {
        let expires = 200_000;
        let lead = expires - EXPIRY_REFRESH_LEAD.as_secs();
        let availability = CatalogAvailability::Fresh(CatalogIdentity {
            revision: 1,
            manifest_sha256: [1; 32],
            created_unix: 10,
            expires_unix: expires,
            source_count: 1,
            source_bytes: 10,
        });
        assert_eq!(
            ready_refresh_deadline(
                &availability,
                Some(lead),
                lead,
                EXPIRY_REFRESH_JITTER_WINDOW,
            ),
            Some(lead + EXPIRY_REFRESH_JITTER_WINDOW.as_secs())
        );
    }

    fn identity(revision: u64, digest: u8) -> CatalogIdentity {
        CatalogIdentity {
            revision,
            manifest_sha256: [digest; 32],
            created_unix: 10,
            expires_unix: 100_000,
            source_count: 1,
            source_bytes: 10,
        }
    }

    fn state_with_installed(installed: CatalogIdentity) -> ServiceState {
        ServiceState {
            snapshot: BlockerCatalogSnapshot::not_configured(),
            observed_update: None,
            installed: Some(installed),
            candidate: None,
            pending: None,
            transition: None,
            failure_streak: 0,
            next_automatic_refresh_unix: None,
            refresh_jitter: Duration::ZERO,
            terminal_failure: None,
            enabled_policy_terminal: false,
            repaired_candidate: None,
            source_material_epoch: 0,
            source_material_repair: None,
            automatic_source_material_repair: None,
            native_retry: None,
            official_verification: None,
        }
    }

    #[test]
    fn bundled_source_refresh_due_never_invalidates_release_authority() {
        let current = identity(4, 4);
        let mut state = state_with_installed(current.clone());
        state.publish_release_seed(current.expires_unix);

        assert_eq!(state.snapshot.phase, BlockerCatalogPhase::Fresh);
        assert_eq!(state.snapshot.package_stale, Some(false));
        assert!(state.snapshot.source_refresh_due);
        assert_eq!(state.snapshot.package_revision, Some(current.revision));
        assert_eq!(state.snapshot.installed_revision, Some(current.revision));
        assert!(!state.snapshot.refresh_supported);

        state.publish_release_seed(current.expires_unix - 1);
        assert!(state.snapshot.source_refresh_due);
        assert_eq!(state.snapshot.phase, BlockerCatalogPhase::Fresh);
    }

    fn refreshing_status(current: Option<CatalogIdentity>) -> StatusSnapshot {
        StatusSnapshot {
            revision: 9,
            last_refresh_attempt_unix: Some(8),
            status: UpdateStatus::Refreshing {
                operation: 10,
                current: current.map(CatalogAvailability::Fresh),
            },
        }
    }

    fn ready_status(current: CatalogIdentity) -> StatusSnapshot {
        StatusSnapshot {
            revision: 10,
            last_refresh_attempt_unix: Some(9),
            status: UpdateStatus::Ready(CatalogAvailability::Fresh(current)),
        }
    }

    fn failed_status(
        current: CatalogIdentity,
        operation: u64,
        failure: FailureKind,
    ) -> StatusSnapshot {
        StatusSnapshot {
            revision: 10,
            last_refresh_attempt_unix: Some(9),
            status: UpdateStatus::Failed {
                operation,
                failure,
                current: Some(CatalogAvailability::Fresh(current)),
            },
        }
    }

    #[test]
    fn current_source_failures_coalesce_by_exact_newest_identity_and_seal() {
        let signal = SourceMaterialSignal::new();
        let old = identity(4, 4);
        let current = identity(5, 5);
        signal.report(current.clone());
        signal.report(old);
        signal.report(current.clone());
        assert_eq!(signal.take(), Some(current.clone()));
        assert_eq!(signal.take(), None);

        signal.report(current);
        signal.seal();
        signal.report(identity(6, 6));
        assert_eq!(signal.take(), None);
    }

    #[test]
    fn later_same_identity_failure_requires_explicit_repair_after_one_automatic_attempt() {
        let current = identity(4, 4);
        let mut state = state_with_installed(current.clone());
        state.admit_source_material_failure(current.clone());
        assert!(matches!(
            state.source_material_repair,
            Some(SourceMaterialRepair::Requested(ref identity)) if *identity == current
        ));

        state.automatic_source_material_repair = Some(current.clone());
        state.source_material_repair = None;
        state.admit_source_material_failure(current.clone());
        assert!(matches!(
            state.source_material_repair,
            Some(SourceMaterialRepair::RetryPending(ref identity)) if *identity == current
        ));
    }

    #[test]
    fn exact_current_repair_advances_material_epoch_once() {
        let current = identity(4, 4);
        let mut state = state_with_installed(current.clone());
        state.source_material_repair = Some(SourceMaterialRepair::InFlight {
            identity: current.clone(),
            operation: 17,
        });

        let ready = ready_status(current);
        state.observe_source_material_repair(&ready);
        assert_eq!(state.source_material_epoch, 1);
        assert!(state.source_material_repair.is_none());

        state.observe_source_material_repair(&ready);
        assert_eq!(state.source_material_epoch, 1);
    }

    #[test]
    fn transport_waits_for_manual_current_repair_but_unsafe_storage_seals() {
        let current = identity(4, 4);
        let mut transport = state_with_installed(current.clone());
        transport.source_material_repair = Some(SourceMaterialRepair::InFlight {
            identity: current.clone(),
            operation: 17,
        });
        transport.observe_source_material_repair(&failed_status(
            current.clone(),
            17,
            FailureKind::Transport,
        ));
        assert!(matches!(
            transport.source_material_repair,
            Some(SourceMaterialRepair::RetryPending(ref identity)) if *identity == current
        ));
        assert_eq!(transport.source_material_epoch, 0);
        assert_eq!(transport.terminal_failure, None);
        let update = failed_status(current.clone(), 17, FailureKind::Transport);
        transport.publish(&update);
        assert!(!transport.snapshot.source_material_repair_pending);
        assert!(transport.snapshot.source_material_repair_retry_pending);

        let mut storage = state_with_installed(current.clone());
        storage.source_material_repair = Some(SourceMaterialRepair::InFlight {
            identity: current.clone(),
            operation: 18,
        });
        storage.observe_source_material_repair(&failed_status(
            current.clone(),
            18,
            FailureKind::Storage,
        ));
        assert_eq!(
            storage.terminal_failure,
            Some(BlockerCatalogFailure::Storage)
        );
        assert!(storage.enabled_policy_terminal);
        assert!(storage.source_material_repair.is_none());
        storage.publish(&failed_status(current, 18, FailureKind::Storage));
        assert!(!storage.snapshot.source_material_repair_pending);
        assert!(!storage.snapshot.source_material_repair_retry_pending);
    }

    #[test]
    fn newer_catalog_supersedes_current_material_repair_without_minting_an_epoch() {
        let current = identity(4, 4);
        let newer = identity(5, 5);
        let mut state = state_with_installed(current.clone());
        state.automatic_source_material_repair = Some(current.clone());
        state.source_material_repair = Some(SourceMaterialRepair::InFlight {
            identity: current,
            operation: 17,
        });
        state.admit_activation(
            ActivatedCatalog {
                identity: newer.clone(),
                catalog: PolicyCatalog::eager(StaticPolicyCatalog::empty()),
            },
            1,
        );

        assert!(state.source_material_repair.is_none());
        assert_eq!(state.source_material_epoch, 0);
        assert_eq!(state.candidate, Some(newer));
    }

    fn assert_candidate(snapshot: BlockerCatalogSnapshot, expected: Option<&CatalogIdentity>) {
        assert_eq!(
            snapshot.candidate_revision,
            expected.map(|identity| identity.revision)
        );
        assert_eq!(
            snapshot.candidate_manifest_sha256,
            expected.map(|identity| identity.manifest_sha256)
        );
        assert_eq!(
            snapshot.candidate_created_unix,
            expected.map(|identity| identity.created_unix)
        );
        assert_eq!(
            snapshot.candidate_expires_unix,
            expected.map(|identity| identity.expires_unix)
        );
        assert_eq!(
            snapshot.candidate_source_count,
            expected.map(|identity| identity.source_count)
        );
        assert_eq!(
            snapshot.candidate_source_bytes,
            expected.map(|identity| identity.source_bytes)
        );
    }

    #[test]
    fn exact_snapshot_separates_initial_candidate_from_absent_current() {
        let candidate = identity(1, 1);
        let snapshot = snapshot_from_update(&refreshing_status(None), None, Some(&candidate), true);

        assert_eq!(snapshot.package_revision, None);
        assert_eq!(snapshot.package_manifest_sha256, None);
        assert_eq!(snapshot.installed_revision, None);
        assert_eq!(snapshot.installed_manifest_sha256, None);
        assert_candidate(snapshot, Some(&candidate));
        assert!(snapshot.activation_pending);
    }

    #[test]
    fn exact_snapshot_keeps_old_current_installed_and_new_candidate_distinct() {
        let current = identity(4, 4);
        let candidate = identity(5, 5);
        let snapshot = snapshot_from_update(
            &refreshing_status(Some(current.clone())),
            Some(&current),
            Some(&candidate),
            true,
        );

        assert_eq!(snapshot.package_revision, Some(current.revision));
        assert_eq!(
            snapshot.package_manifest_sha256,
            Some(current.manifest_sha256)
        );
        assert_eq!(snapshot.installed_revision, Some(current.revision));
        assert_eq!(
            snapshot.installed_manifest_sha256,
            Some(current.manifest_sha256)
        );
        assert_candidate(snapshot, Some(&candidate));
    }

    #[test]
    fn post_commit_pre_activation_snapshot_reports_new_current_and_candidate() {
        let installed = identity(4, 4);
        let candidate = identity(5, 5);
        let snapshot = snapshot_from_update(
            &ready_status(candidate.clone()),
            Some(&installed),
            Some(&candidate),
            true,
        );

        assert_eq!(snapshot.package_revision, Some(candidate.revision));
        assert_eq!(
            snapshot.package_manifest_sha256,
            Some(candidate.manifest_sha256)
        );
        assert_eq!(snapshot.installed_revision, Some(installed.revision));
        assert_eq!(
            snapshot.installed_manifest_sha256,
            Some(installed.manifest_sha256)
        );
        assert_candidate(snapshot, Some(&candidate));
        assert!(snapshot.activation_pending);
    }

    #[test]
    fn transient_catalog_failure_does_not_terminalize_enabled_installed_policy() {
        let installed = identity(4, 4);
        let mut state = state_with_installed(installed.clone());
        state.fail(BlockerCatalogFailure::Storage, false);
        state.publish(&ready_status(installed.clone()));

        assert_eq!(
            state.snapshot.phase,
            BlockerCatalogPhase::Failed(BlockerCatalogFailure::Storage)
        );
        assert!(!state.snapshot.enabled_policy_terminal);
        assert_eq!(state.snapshot.package_revision, Some(installed.revision));
        assert_eq!(state.snapshot.installed_revision, Some(installed.revision));

        state.fail(BlockerCatalogFailure::Internal, true);
        state.publish(&ready_status(installed));
        assert!(state.snapshot.enabled_policy_terminal);
    }

    #[test]
    fn shutdown_projection_revokes_candidate_and_refresh_authority() {
        let installed = identity(4, 4);
        let candidate = identity(5, 5);
        let mut state = state_with_installed(installed.clone());
        state.candidate = Some(candidate.clone());
        state.publish(&StatusSnapshot {
            revision: 10,
            last_refresh_attempt_unix: Some(9),
            status: UpdateStatus::Refreshing {
                operation: 11,
                current: Some(CatalogAvailability::Fresh(installed)),
            },
        });
        assert_candidate(state.snapshot, Some(&candidate));
        assert_eq!(state.snapshot.refresh_operation, Some(11));

        state.publish_shutdown();

        assert_eq!(state.snapshot.phase, BlockerCatalogPhase::Shutdown);
        assert_candidate(state.snapshot, None);
        assert!(!state.snapshot.activation_pending);
        assert_eq!(state.snapshot.refresh_operation, None);
        assert!(state.snapshot.enabled_policy_terminal);
    }

    #[test]
    fn failed_update_projects_progress_until_async_cleanup_settles() {
        let installed = identity(4, 4);
        let candidate = identity(5, 5);
        let failed = StatusSnapshot {
            revision: 10,
            last_refresh_attempt_unix: Some(9),
            status: UpdateStatus::Failed {
                operation: 11,
                failure: FailureKind::Storage,
                current: Some(CatalogAvailability::Fresh(installed.clone())),
            },
        };
        let mut state = state_with_installed(installed);
        state.candidate = Some(candidate.clone());
        state.transition = Some(CatalogTransition::Discarding(candidate.clone(), None));
        state.publish(&failed);
        assert_eq!(state.snapshot.phase, BlockerCatalogPhase::Refreshing);
        assert_eq!(state.snapshot.refresh_operation, Some(11));
        assert!(state.snapshot.activation_pending);
        assert!(!state.snapshot.repair_retry_pending);

        state.transition = Some(CatalogTransition::RepairRetryPending(
            candidate,
            FailureKind::Transport,
        ));
        state.publish(&failed);
        assert_eq!(
            state.snapshot.phase,
            BlockerCatalogPhase::Failed(BlockerCatalogFailure::Storage)
        );
        assert_eq!(state.snapshot.refresh_operation, Some(11));
        assert!(state.snapshot.activation_pending);
        assert!(state.snapshot.repair_retry_pending);
    }

    #[test]
    fn source_storage_failure_enters_one_exact_authenticated_repair_path() {
        let installed = identity(4, 4);
        let candidate = identity(5, 5);
        let mut state = state_with_installed(installed);
        state.candidate = Some(candidate.clone());
        state.pending = Some(ActivatedCatalog {
            identity: candidate.clone(),
            catalog: PolicyCatalog::eager(StaticPolicyCatalog::empty()),
        });
        state.transition = Some(CatalogTransition::Preparing(candidate.clone()));

        state.apply_transition_completion(CatalogTransitionCompletion::Prepared(
            candidate.clone(),
            CatalogPreparationOutcome::Failed(
                zephium_core::blocker::BlockerCompileFailure::SourceUnavailable,
            ),
        ));

        assert_eq!(state.candidate, Some(candidate.clone()));
        assert!(matches!(
            state.transition,
            Some(CatalogTransition::ReadyToRepair(ref expected))
                if *expected == candidate
        ));
        assert!(state.pending.is_some());
        assert!(state.terminal_failure.is_none());
        assert!(!state.enabled_policy_terminal);
    }

    #[test]
    fn successful_repair_retries_and_repeated_local_failure_is_never_durably_rejected() {
        let installed = identity(4, 4);
        let candidate = identity(5, 5);
        let mut state = state_with_installed(installed);
        state.candidate = Some(candidate.clone());
        state.pending = Some(ActivatedCatalog {
            identity: candidate.clone(),
            catalog: PolicyCatalog::eager(StaticPolicyCatalog::empty()),
        });
        state.transition = Some(CatalogTransition::Repairing(candidate.clone()));

        state.apply_transition_completion(CatalogTransitionCompletion::Repaired(
            candidate.clone(),
            CandidateRepairOutcome::Repaired,
        ));
        assert!(state.transition.is_none());
        assert!(state.pending.is_some());
        assert!(state.terminal_failure.is_none());

        state.transition = Some(CatalogTransition::Preparing(candidate.clone()));
        state.apply_transition_completion(CatalogTransitionCompletion::Prepared(
            candidate.clone(),
            CatalogPreparationOutcome::Failed(
                zephium_core::blocker::BlockerCompileFailure::SourceUnavailable,
            ),
        ));
        assert_eq!(state.candidate, Some(candidate));
        assert!(state.transition.is_none());
        assert!(state.pending.is_none());
        assert_eq!(state.terminal_failure, Some(BlockerCatalogFailure::Storage));
        assert!(!state.enabled_policy_terminal);
    }

    #[test]
    fn retryable_repair_failure_waits_for_explicit_retry_without_dropping_authority() {
        for failure in [
            FailureKind::Transport,
            FailureKind::Metadata,
            FailureKind::Target,
        ] {
            let installed = identity(4, 4);
            let candidate = identity(5, 5);
            let mut state = state_with_installed(installed.clone());
            state.candidate = Some(candidate.clone());
            state.pending = Some(ActivatedCatalog {
                identity: candidate.clone(),
                catalog: PolicyCatalog::eager(StaticPolicyCatalog::empty()),
            });
            state.transition = Some(CatalogTransition::Repairing(candidate.clone()));

            state.apply_transition_completion(CatalogTransitionCompletion::Repaired(
                candidate.clone(),
                CandidateRepairOutcome::Failed(failure),
            ));
            assert_eq!(state.installed, Some(installed));
            assert_eq!(state.candidate, Some(candidate.clone()));
            assert!(state
                .pending
                .as_ref()
                .is_some_and(|pending| pending.identity == candidate));
            assert!(matches!(
                state.transition,
                Some(CatalogTransition::RepairRetryPending(
                    ref pending,
                    pending_failure
                )) if *pending == candidate && pending_failure == failure
            ));
            assert!(state.terminal_failure.is_none());
            assert!(!state.enabled_policy_terminal);
        }
    }

    #[test]
    fn expired_retry_candidate_is_rejected_without_spending_repair_admission() {
        let installed = identity(4, 4);
        let mut candidate = identity(5, 5);
        candidate.expires_unix = 100;
        let mut state = state_with_installed(installed.clone());
        state.candidate = Some(candidate.clone());
        state.pending = Some(ActivatedCatalog {
            identity: candidate.clone(),
            catalog: PolicyCatalog::eager(StaticPolicyCatalog::empty()),
        });
        state.transition = Some(CatalogTransition::RepairRetryPending(
            candidate.clone(),
            FailureKind::Transport,
        ));

        state.expire_retry_candidate(100);

        assert_eq!(state.installed, Some(installed));
        assert_eq!(state.candidate, Some(candidate.clone()));
        assert!(state.pending.is_none());
        assert!(matches!(
            state.transition,
            Some(CatalogTransition::ReadyToReject(
                ref expired,
                FailureKind::Manifest,
                CandidateRejectionReason::Expired,
            )) if *expired == candidate
        ));
        assert!(state.terminal_failure.is_none());
    }

    #[test]
    fn unsafe_or_contradictory_repair_failure_stops_retry_and_preserves_current() {
        for (failure, expected_terminal) in [
            (FailureKind::Storage, false),
            (FailureKind::Rollback, true),
            (FailureKind::Internal, true),
        ] {
            let installed = identity(4, 4);
            let candidate = identity(5, 5);
            let mut state = state_with_installed(installed.clone());
            state.candidate = Some(candidate.clone());
            state.pending = Some(ActivatedCatalog {
                identity: candidate.clone(),
                catalog: PolicyCatalog::eager(StaticPolicyCatalog::empty()),
            });
            state.transition = Some(CatalogTransition::Repairing(candidate.clone()));

            state.apply_transition_completion(CatalogTransitionCompletion::Repaired(
                candidate.clone(),
                CandidateRepairOutcome::Failed(failure),
            ));
            assert_eq!(state.installed, Some(installed));
            assert_eq!(state.candidate, Some(candidate));
            assert!(state.pending.is_none());
            assert!(state.transition.is_none());
            assert_eq!(state.terminal_failure, Some(map_failure(failure)));
            assert_eq!(state.enabled_policy_terminal, expected_terminal);
        }
    }

    #[test]
    fn supersession_completion_carries_candidate_across_pre_observation_race() {
        let installed = identity(4, 4);
        let mut old = identity(5, 5);
        old.expires_unix = u64::MAX;
        let mut newer = identity(6, 6);
        newer.expires_unix = u64::MAX;
        let mut state = state_with_installed(installed.clone());
        state.candidate = Some(old.clone());
        state.pending = Some(ActivatedCatalog {
            identity: old.clone(),
            catalog: PolicyCatalog::eager(StaticPolicyCatalog::empty()),
        });
        state.transition = Some(CatalogTransition::Repairing(old.clone()));

        // Model the service's candidate sample occurring immediately before
        // worker publication. Completion owns the exact immutable catalog, so
        // it cannot depend on a second queue observation in this drive.
        state.apply_transition_completion(CatalogTransitionCompletion::Repaired(
            old,
            CandidateRepairOutcome::Superseded(ActivatedCatalog {
                identity: newer.clone(),
                catalog: PolicyCatalog::eager(StaticPolicyCatalog::empty()),
            }),
        ));

        assert_eq!(state.installed, Some(installed));
        assert_eq!(state.candidate, Some(newer.clone()));
        assert!(state
            .pending
            .as_ref()
            .is_some_and(|pending| pending.identity == newer));
        assert!(state.transition.is_none());
        assert!(state.terminal_failure.is_none());
    }

    #[test]
    fn deterministic_invalid_candidate_enters_exact_rejection_path() {
        let installed = identity(4, 4);
        let candidate = identity(5, 5);
        let mut state = state_with_installed(installed);
        state.candidate = Some(candidate.clone());
        state.transition = Some(CatalogTransition::Preparing(candidate.clone()));

        state.apply_transition_completion(CatalogTransitionCompletion::Prepared(
            candidate.clone(),
            CatalogPreparationOutcome::Failed(
                zephium_core::blocker::BlockerCompileFailure::InvalidSource,
            ),
        ));

        assert!(matches!(
            state.transition,
            Some(CatalogTransition::ReadyToReject(
                ref expected,
                FailureKind::Catalog,
                CandidateRejectionReason::CompilerPolicy,
            )) if *expected == candidate
        ));
        assert_eq!(state.candidate, Some(candidate));
        assert!(state.terminal_failure.is_none());
    }

    #[test]
    fn successful_candidate_rejection_preserves_installed_authority() {
        let installed = identity(4, 4);
        let candidate = identity(5, 5);
        let mut state = state_with_installed(installed.clone());
        state.candidate = Some(candidate.clone());
        state.transition = Some(CatalogTransition::Rejecting(
            candidate.clone(),
            FailureKind::Catalog,
        ));

        state.apply_transition_completion(CatalogTransitionCompletion::Rejected(
            candidate,
            CandidateRejectOutcome::Rejected,
        ));

        assert_eq!(state.installed, Some(installed));
        assert!(state.candidate.is_none());
        assert!(state.transition.is_none());
        assert!(state.terminal_failure.is_none());
    }

    #[test]
    fn failed_candidate_rejection_keeps_candidate_authority_visible() {
        let installed = identity(4, 4);
        let candidate = identity(5, 5);
        let mut state = state_with_installed(installed.clone());
        state.candidate = Some(candidate.clone());
        state.transition = Some(CatalogTransition::Rejecting(
            candidate.clone(),
            FailureKind::Catalog,
        ));

        state.apply_transition_completion(CatalogTransitionCompletion::Rejected(
            candidate.clone(),
            CandidateRejectOutcome::Failed(FailureKind::Storage),
        ));

        assert_eq!(state.installed, Some(installed));
        assert_eq!(state.candidate, Some(candidate));
        assert!(state.transition.is_none());
        assert_eq!(state.terminal_failure, Some(BlockerCatalogFailure::Storage));
    }

    #[test]
    fn expired_commit_discards_prepared_candidate_then_clears_authority() {
        let installed = identity(4, 4);
        let candidate = identity(5, 5);
        let mut state = state_with_installed(installed.clone());
        state.candidate = Some(candidate.clone());
        state.transition = Some(CatalogTransition::Committing(candidate.clone()));

        state.apply_transition_completion(CatalogTransitionCompletion::Committed(
            candidate.clone(),
            CandidateCommitOutcome::Failed(FailureKind::Manifest),
        ));
        assert!(matches!(
            state.transition,
            Some(CatalogTransition::ReadyToDiscard(ref expected, None))
                if *expected == candidate
        ));

        state.transition = Some(CatalogTransition::Discarding(candidate.clone(), None));
        state.apply_transition_completion(CatalogTransitionCompletion::Discarded(candidate, true));
        assert_eq!(state.installed, Some(installed));
        assert!(state.candidate.is_none());
        assert!(state.transition.is_none());
        assert!(state.terminal_failure.is_none());
    }

    #[test]
    fn activation_failure_keeps_exact_durable_candidate_visible() {
        let installed = identity(4, 4);
        let candidate = identity(5, 5);
        let mut state = state_with_installed(installed.clone());
        state.candidate = Some(candidate.clone());
        state.transition = Some(CatalogTransition::Activating(candidate.clone()));

        state.apply_transition_completion(CatalogTransitionCompletion::Activated(
            candidate.clone(),
            false,
        ));
        assert_eq!(state.installed, Some(installed));
        assert_eq!(state.candidate, Some(candidate));
        assert!(state.transition.is_none());
        assert_eq!(
            state.terminal_failure,
            Some(BlockerCatalogFailure::Internal)
        );
    }

    #[test]
    fn unexpected_exact_prepared_completion_is_discarded_before_terminalization() {
        let installed = identity(4, 4);
        let candidate = identity(5, 5);
        let mut state = state_with_installed(installed);
        state.candidate = Some(candidate.clone());
        state.transition = Some(CatalogTransition::ReadyToCommit(candidate.clone()));

        state.apply_transition_completion(CatalogTransitionCompletion::Prepared(
            candidate.clone(),
            CatalogPreparationOutcome::Prepared,
        ));

        assert!(matches!(
            state.transition,
            Some(CatalogTransition::ReadyToDiscard(
                ref expected,
                Some(BlockerCatalogFailure::Internal),
            )) if *expected == candidate
        ));
        assert!(state.terminal_failure.is_none());
    }

    #[test]
    fn terminal_commit_dispatch_routes_prepared_candidate_through_discard() {
        let (_cache_root, cache) = cache();
        let service = ManagedBlocker::unconfigured(cache).unwrap();
        let candidate = identity(5, 5);
        {
            let mut state = service.lock_state();
            state.candidate = Some(candidate.clone());
            state.transition = Some(CatalogTransition::ReadyToCommit(candidate.clone()));
        }

        service.drive_catalog_transition();

        let state = service.lock_state();
        assert!(matches!(
            state.transition,
            Some(CatalogTransition::ReadyToDiscard(
                ref expected,
                Some(BlockerCatalogFailure::Internal),
            )) if *expected == candidate
        ));
        assert!(state.terminal_failure.is_none());
        drop(state);
        assert_eq!(
            service.shutdown_until(Instant::now() + Duration::from_secs(2)),
            BlockerShutdownOutcome::Clean
        );
    }

    #[test]
    fn activation_is_strictly_monotonic_and_equivocation_is_terminal() {
        let mut state = state_with_installed(identity(4, 4));
        state.admit_activation(
            ActivatedCatalog {
                identity: identity(5, 5),
                catalog: PolicyCatalog::eager(StaticPolicyCatalog::empty()),
            },
            100,
        );
        assert_eq!(
            state.pending.as_ref().map(|value| value.identity.clone()),
            Some(identity(5, 5))
        );
        assert!(state.terminal_failure.is_none());

        state.admit_activation(
            ActivatedCatalog {
                identity: identity(5, 6),
                catalog: PolicyCatalog::eager(StaticPolicyCatalog::empty()),
            },
            100,
        );
        assert_eq!(
            state.terminal_failure,
            Some(BlockerCatalogFailure::Rollback)
        );
    }

    #[test]
    fn catalog_transition_completion_must_match_the_exact_in_flight_identity() {
        let mut state = state_with_installed(identity(4, 4));
        state.transition = Some(CatalogTransition::Preparing(identity(5, 5)));
        state.apply_transition_completion(CatalogTransitionCompletion::Prepared(
            identity(5, 6),
            CatalogPreparationOutcome::Prepared,
        ));
        assert_eq!(
            state.terminal_failure,
            Some(BlockerCatalogFailure::Internal)
        );
        assert_eq!(state.installed, Some(identity(4, 4)));
    }

    #[test]
    fn updater_unavailability_does_not_erase_an_authenticated_installed_identity() {
        let installed = identity(4, 4);
        let snapshot = snapshot_from_update(
            &StatusSnapshot {
                revision: 9,
                last_refresh_attempt_unix: Some(8),
                status: UpdateStatus::Unavailable(UnavailableReason::StorageUnavailable),
            },
            Some(&installed),
            None,
            false,
        );
        assert_eq!(snapshot.package_revision, Some(4));
        assert_eq!(snapshot.installed_revision, Some(4));
        assert_eq!(snapshot.source_count, Some(1));
        assert_eq!(snapshot.source_bytes, Some(10));
        assert_eq!(
            snapshot.phase,
            BlockerCatalogPhase::Unavailable(BlockerCatalogUnavailable::StorageUnavailable)
        );
    }

    #[test]
    fn service_recomputes_expiry_even_while_the_updater_is_refreshing() {
        let current = identity(4, 4);
        let snapshot = snapshot_from_update_at(
            &StatusSnapshot {
                revision: 9,
                last_refresh_attempt_unix: Some(8),
                status: UpdateStatus::Refreshing {
                    operation: 10,
                    current: Some(CatalogAvailability::Fresh(current.clone())),
                },
            },
            Some(&current),
            None,
            false,
            current.expires_unix,
        );
        assert_eq!(snapshot.phase, BlockerCatalogPhase::Refreshing);
        assert_eq!(snapshot.package_stale, Some(true));
        assert!(snapshot.source_refresh_due);
    }

    #[test]
    fn forward_clock_jump_relabels_a_ready_snapshot_stale_without_updater_transition() {
        let current = identity(4, 4);
        let update = StatusSnapshot {
            revision: 9,
            last_refresh_attempt_unix: Some(8),
            status: UpdateStatus::Ready(CatalogAvailability::Fresh(current.clone())),
        };
        let before = snapshot_from_update_at(
            &update,
            Some(&current),
            None,
            false,
            current.expires_unix - 1,
        );
        let after =
            snapshot_from_update_at(&update, Some(&current), None, false, current.expires_unix);
        assert_eq!(before.phase, BlockerCatalogPhase::Fresh);
        assert_eq!(before.package_stale, Some(false));
        assert_eq!(after.phase, BlockerCatalogPhase::Stale);
        assert_eq!(after.package_stale, Some(true));
        assert!(!before.source_refresh_due);
        assert!(after.source_refresh_due);
    }
    #[test]
    fn native_preflight_owns_candidate_completion_and_queue_refusal_is_bounded() {
        use zephium_blocker::{PolicySource, SourceFormat, SourceId};
        use zephium_core::ports::engine::{
            ContentRuleValidationCompletion, ContentRuleValidationOutcome,
        };
        let root = tempfile::tempdir_in(std::env::current_dir().unwrap()).unwrap();
        let mut current = identity(1, 1);
        current.created_unix = now_unix();
        current.expires_unix = now_unix() + 86400;
        let catalog = |hash| {
            PolicyCatalog::authenticated(
                [hash; 32],
                StaticPolicyCatalog::new(vec![PolicySource::new(
                    SourceId::new("test").unwrap(),
                    SourceFormat::Standard,
                    Arc::from("||ads.example.invalid^\n##.ad"),
                )])
                .unwrap(),
            )
        };
        let mut service = ManagedBlocker::with_release_seed(
            ReleaseCatalogSeed {
                identity: current.clone(),
                catalog: catalog(1),
            },
            CompiledArtifactCacheConfig::new(root.path().join("compiled")).unwrap(),
        )
        .unwrap();
        let (tx, rx) = std::sync::mpsc::sync_channel::<ContentRuleValidationCompletion>(2);
        Arc::get_mut(&mut service).unwrap().native_validation =
            Some(Arc::new(move |rules, done| {
                assert!(!matches!(
                    rules.payload(),
                    zephium_core::blocker::ContentRulesPayload::AllowAll
                ));
                tx.send(done).unwrap();
            }));
        let mut candidate = current.clone();
        candidate.revision = 2;
        candidate.manifest_sha256 = [2; 32];
        service.lock_state().admit_activation(
            ActivatedCatalog {
                identity: candidate.clone(),
                catalog: catalog(2),
            },
            now_unix(),
        );
        service.drive_locked(false);
        let completion = rx.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(matches!(
            service.lock_state().transition,
            Some(CatalogTransition::Preparing(_))
        ));
        assert_eq!(service.lock_state().installed.as_ref(), Some(&current));
        completion.finish(ContentRuleValidationOutcome::Unavailable);
        service.drive_locked(false);
        assert_eq!(service.lock_state().native_retry.as_ref().unwrap().1, 1);
        service.drive_locked(false);
        assert!(rx.try_recv().is_err());
        service.lock_state().native_retry.as_mut().unwrap().2 = Instant::now();
        service.drive_locked(false);
        rx.recv_timeout(Duration::from_secs(2))
            .unwrap()
            .finish(ContentRuleValidationOutcome::Valid);
        let completion = service
            .transition_completion
            .lock()
            .unwrap()
            .take()
            .unwrap();
        service.lock_state().apply_transition_completion(completion);
        assert!(matches!(
            service.lock_state().transition,
            Some(CatalogTransition::ReadyToCommit(_))
        ));
        assert_eq!(service.lock_state().installed.as_ref(), Some(&current));
        assert_eq!(
            service.shutdown_until(Instant::now() + Duration::from_secs(2)),
            BlockerShutdownOutcome::Clean
        );
    }

    #[test]
    fn official_verification_can_refresh_unchanged_bytes_without_extending_tuf_expiry() {
        let current = identity(1, 1);
        let mut state = state_with_installed(current.clone());
        let old = StatusSnapshot {
            revision: 1,
            last_refresh_attempt_unix: Some(10),
            status: UpdateStatus::Ready(CatalogAvailability::Stale(current.clone())),
        };
        state.observe_official_verification(&old, Some((1, true)));
        state.publish_with_supply(&old, Some(([0; 32], true)));
        assert_eq!(state.snapshot.package_stale, Some(true));
        let checked = StatusSnapshot {
            revision: 2,
            last_refresh_attempt_unix: Some(20),
            status: UpdateStatus::Ready(CatalogAvailability::Fresh(current.clone())),
        };
        state.observe_official_verification(&checked, Some((2, false)));
        state.publish_with_supply(&checked, Some(([0; 32], false)));
        assert_eq!(state.snapshot.source_material_epoch, 1);
        assert_eq!(state.snapshot.package_stale, Some(false));
        assert!(!state.snapshot.source_refresh_due);
        assert_eq!(
            state.snapshot.package_provenance,
            Some(BlockerCatalogProvenance::OfficialHttps)
        );
        assert_eq!(
            state.snapshot.package_expires_unix,
            Some(current.expires_unix)
        );
        let signed = snapshot_from_update(&checked, Some(&current), None, false);
        assert_eq!(signed.package_stale, Some(true));
    }
}
