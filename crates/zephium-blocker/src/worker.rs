use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TrySendError};
use std::sync::{Arc, Mutex, MutexGuard, TryLockError, Weak};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::Sha256;
use thiserror::Error;
use zephium_core::blocker::{
    BlockerConfig, ContentPolicyGeneration, ContentRuleCoverage, ContentRuleDigest, ContentRules,
    NetworkAttribution, NetworkDecision as CoreDecision, NetworkPolicyDiagnostics,
    NetworkRequest as CoreRequest, NetworkRequestPolicy, NetworkResourceType,
};
use zephium_core::ids::ProfileId;
use zephium_core::ports::blocker::{
    BlockerCompileFailure, BlockerCompileOutcome, BlockerCompiler, BlockerDispatch,
    BlockerRetirementDispatch, BlockerShutdownOutcome,
};

use crate::cache::{
    CacheKey, CompiledArtifactCacheConfig, LoadedArtifact, PersistentArtifactCache,
};
#[cfg(feature = "runtime")]
use crate::rules::CachedRuntimeRules;
#[cfg(feature = "runtime-exact")]
use crate::rules::SourceDocument;
use crate::{
    CompileError, CompileTarget, CompiledRules, Compiler, FilterSource, NetworkAction,
    NetworkRequest, RequestMethod, ResourceType, SourceFormat, SourceId,
};

const ALLOW_ALL_DIGEST_DOMAIN: &[u8] = b"zephium-explicit-allow-all-v1";
const SHUTDOWN_POLL_INTERVAL: Duration = Duration::from_millis(1);

/// Reusable, immutable filter source supplied by an already-authenticated
/// source-package layer.
///
/// This type performs no download, signature verification, or update. Those
/// are separate supply-chain responsibilities and must finish before a source
/// reaches this boundary.
#[derive(Clone, Debug)]
pub struct PolicySource {
    id: SourceId,
    format: SourceFormat,
    contents: Arc<str>,
    trusted: bool,
}

impl PolicySource {
    /// Creates an immutable source. The compiler applies all byte/rule limits
    /// before publishing an artifact.
    pub fn new(id: SourceId, format: SourceFormat, contents: Arc<str>) -> Self {
        Self {
            id,
            format,
            contents,
            trusted: false,
        }
    }

    /// Lets this source's `trusted-*` scriptlet rules run.
    #[must_use]
    pub const fn trusting_scriptlets(mut self) -> Self {
        self.trusted = true;
        self
    }

    /// Returns the immutable source byte length without copying it.
    pub fn byte_len(&self) -> usize {
        self.contents.len()
    }

    fn instantiate(&self) -> FilterSource {
        let source = FilterSource::new(
            self.id.clone(),
            self.format,
            self.contents.as_ref().to_owned(),
        );
        if self.trusted {
            source.trusting_scriptlets()
        } else {
            source
        }
    }
}

/// One bounded, immutable source catalog shared by every profile.
///
/// The initial implementation intentionally has no mutable remote updater.
/// A new authenticated package should create a new catalog/revision rather
/// than modifying memory observed by native request callbacks.
#[derive(Clone, Debug, Default)]
pub struct StaticPolicyCatalog {
    sources: Arc<[PolicySource]>,
}

type DeferredCatalogLoader =
    Box<dyn FnMut() -> Result<StaticPolicyCatalog, BlockerCompileFailure> + Send>;

enum PolicyCatalogSource {
    Eager(StaticPolicyCatalog),
    Deferred {
        loader: DeferredCatalogLoader,
        retain_success: bool,
    },
}

/// Authenticated catalog material which can defer source I/O until a compiled
/// cache miss actually requires parsing.
///
/// Clones share one exact source lease. Deferred source I/O is serialized and
/// failures are never memoized, so an authenticated on-disk repair remains
/// observable without permitting duplicate concurrent loads. Loaders may
/// retain successful material, or deliberately reload it after each compiled
/// cache miss so large release-seed source strings are not held indefinitely.
/// A persistent-cache hit drops the lease without reading source bodies.
#[derive(Clone)]
pub struct PolicyCatalog {
    manifest_sha256: Option<[u8; 32]>,
    source: Arc<Mutex<PolicyCatalogSource>>,
}

impl std::fmt::Debug for PolicyCatalog {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PolicyCatalog")
            .field("authenticated", &self.manifest_sha256.is_some())
            .finish_non_exhaustive()
    }
}

impl PolicyCatalog {
    /// Wraps in-memory source material without an external package identity.
    pub fn eager(catalog: StaticPolicyCatalog) -> Self {
        Self {
            manifest_sha256: None,
            source: Arc::new(Mutex::new(PolicyCatalogSource::Eager(catalog))),
        }
    }

    /// Wraps source material authenticated by one exact canonical manifest.
    pub fn authenticated(manifest_sha256: [u8; 32], catalog: StaticPolicyCatalog) -> Self {
        Self {
            manifest_sha256: Some(manifest_sha256),
            source: Arc::new(Mutex::new(PolicyCatalogSource::Eager(catalog))),
        }
    }

    /// Defers exact source loading until the persistent compiled cache misses.
    ///
    /// The loader can be called again after an error because authenticated
    /// package storage may be repaired in the same process. Successful source
    /// material is retained and shared across catalog clones.
    pub fn deferred(
        manifest_sha256: [u8; 32],
        loader: impl FnMut() -> Result<StaticPolicyCatalog, BlockerCompileFailure> + Send + 'static,
    ) -> Self {
        Self {
            manifest_sha256: Some(manifest_sha256),
            source: Arc::new(Mutex::new(PolicyCatalogSource::Deferred {
                loader: Box::new(loader),
                retain_success: true,
            })),
        }
    }

    /// Defers source loading without retaining inflated source material.
    ///
    /// This is intended for immutable release-embedded assets whose loader is
    /// cheap, bounded, and independently authenticated on every call. A
    /// persistent artifact-cache hit still skips the loader entirely; after a
    /// cache miss, raw source strings are dropped once compilation completes.
    pub fn deferred_reloadable(
        manifest_sha256: [u8; 32],
        loader: impl FnMut() -> Result<StaticPolicyCatalog, BlockerCompileFailure> + Send + 'static,
    ) -> Self {
        Self {
            manifest_sha256: Some(manifest_sha256),
            source: Arc::new(Mutex::new(PolicyCatalogSource::Deferred {
                loader: Box::new(loader),
                retain_success: false,
            })),
        }
    }

    fn cache_key(
        &self,
        target: CompileTarget,
        limits: crate::CompileLimits,
    ) -> Result<CacheKey, BlockerCompileFailure> {
        if let Some(manifest_sha256) = self.manifest_sha256 {
            return Ok(CacheKey::for_authenticated_catalog(
                target,
                limits,
                manifest_sha256,
            ));
        }
        let source = self
            .source
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let PolicyCatalogSource::Eager(catalog) = &*source else {
            return Err(BlockerCompileFailure::Internal);
        };
        Ok(catalog.cache_key(target, limits))
    }

    fn load(&self) -> Result<StaticPolicyCatalog, BlockerCompileFailure> {
        let mut source = self
            .source
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match &mut *source {
            PolicyCatalogSource::Eager(catalog) => Ok(catalog.clone()),
            PolicyCatalogSource::Deferred {
                loader,
                retain_success,
            } => {
                let catalog = loader()?;
                if *retain_success {
                    *source = PolicyCatalogSource::Eager(catalog.clone());
                }
                Ok(catalog)
            }
        }
    }
}

impl StaticPolicyCatalog {
    /// Validates and owns already-authenticated source material.
    ///
    /// Catalog admission happens before the worker can clone source strings,
    /// so an updater cannot queue oversized deep copies for every profile.
    /// Zephium's own rules compile with every catalog, so admission keeps a
    /// source slot, their bytes, lines and rules free for them and refuses
    /// their ID.
    pub fn new(sources: Vec<PolicySource>) -> Result<Self, CatalogError> {
        let limits = crate::CompileLimits::default();
        if sources.len() >= limits.max_sources() {
            return Err(CatalogError::TooManySources);
        }
        let mut total_bytes = crate::builtin::byte_len();
        let (mut total_lines, mut total_rules) = crate::builtin::line_counts();
        let mut ids = HashSet::with_capacity(sources.len());
        for source in &sources {
            if source.id.as_str() == crate::builtin::ID {
                return Err(CatalogError::ReservedSource);
            }
            if !ids.insert(&source.id) {
                return Err(CatalogError::DuplicateSource);
            }
            if source.byte_len() > limits.max_source_bytes() {
                return Err(CatalogError::SourceTooLarge);
            }
            total_bytes = total_bytes
                .checked_add(source.byte_len())
                .ok_or(CatalogError::TotalBytesExceeded)?;
            if total_bytes > limits.max_total_source_bytes() {
                return Err(CatalogError::TotalBytesExceeded);
            }
            let (lines, rules) = crate::compiler::line_counts(source.format, &source.contents);
            total_lines = total_lines.saturating_add(lines);
            if total_lines > limits.max_physical_lines() {
                return Err(CatalogError::TooManyLines);
            }
            total_rules = total_rules.saturating_add(rules);
            if total_rules > limits.max_rules() {
                return Err(CatalogError::TooManyRules);
            }
        }
        Ok(Self {
            sources: sources.into(),
        })
    }

    /// Creates the production-safe no-source catalog. Disabled profiles still
    /// receive an explicit allow-all generation; enabled compilation fails
    /// rather than pretending an empty policy provides protection.
    pub fn empty() -> Self {
        Self::default()
    }

    fn instantiate(&self) -> Vec<FilterSource> {
        self.sources.iter().map(PolicySource::instantiate).collect()
    }

    fn is_empty(&self) -> bool {
        self.sources.is_empty()
    }

    fn cache_key(&self, target: CompileTarget, limits: crate::CompileLimits) -> CacheKey {
        CacheKey::for_catalog(
            target,
            limits,
            self.sources.iter().map(|source| {
                let format = match source.format {
                    SourceFormat::Standard => 0,
                    SourceFormat::Hosts => 1,
                };
                // Trust changes what compiles, so it is part of the key.
                let trust = if source.trusted { 2 } else { 0 };
                (source.id.as_str(), format | trust, source.contents.as_ref())
            }),
        )
    }
}

/// An authenticated source catalog exceeded process resource invariants.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum CatalogError {
    /// More sources were supplied than the compiler can admit.
    #[error("blocker catalog has too many sources")]
    TooManySources,
    /// Source identifiers were not unique.
    #[error("blocker catalog contains a duplicate source identifier")]
    DuplicateSource,
    /// The sources' physical lines exceeded the compiler ceiling.
    #[error("blocker catalog has too many lines")]
    TooManyLines,
    /// The sources' candidate rules exceeded the compiler ceiling.
    #[error("blocker catalog has too many rules")]
    TooManyRules,
    /// A source used the identifier reserved for Zephium's own rules.
    #[error("blocker catalog source uses a reserved identifier")]
    ReservedSource,
    /// One source exceeded the immutable per-source byte ceiling.
    #[error("blocker catalog source exceeds its byte limit")]
    SourceTooLarge,
    /// Aggregate source bytes overflowed or exceeded the process ceiling.
    #[error("blocker catalog exceeds its aggregate byte limit")]
    TotalBytesExceeded,
}

struct CompileJob {
    profile: ProfileId,
    generation: ContentPolicyGeneration,
    config: BlockerConfig,
    done: Box<dyn FnOnce(BlockerCompileOutcome) + Send>,
}

struct ProfileRetirement {
    profile: ProfileId,
    generation: ContentPolicyGeneration,
    done: Box<dyn FnOnce() + Send>,
}

struct CatalogReplacement {
    catalog: StaticPolicyCatalog,
    done: Box<dyn FnOnce() + Send>,
}

struct CatalogPreparation {
    identity: [u8; 32],
    catalog: PolicyCatalog,
    done: Box<dyn FnOnce(Result<Arc<ContentRules>, BlockerCompileFailure>) + Send>,
}

struct CatalogActivation {
    identity: [u8; 32],
    done: Box<dyn FnOnce(bool) + Send>,
}

enum WorkerCommand {
    Compile(CompileJob),
    Retire(ProfileRetirement),
    ReplaceCatalog(CatalogReplacement),
    PrepareCatalog(CatalogPreparation),
    ActivateCatalog(CatalogActivation),
    DiscardPreparedCatalog(CatalogActivation),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ProfileAdmission {
    /// The compile is queued or its artifact is being resolved.
    Compiling { generation: ContentPolicyGeneration },
    /// Callback delivery has crossed its linearization point.
    Delivering { generation: ContentPolicyGeneration },
    /// No new compile may enter until the ordered worker marker runs.
    Retiring { generation: ContentPolicyGeneration },
}

impl ProfileAdmission {
    fn generation(self) -> ContentPolicyGeneration {
        match self {
            Self::Compiling { generation }
            | Self::Delivering { generation }
            | Self::Retiring { generation } => generation,
        }
    }
}

#[derive(Default)]
struct AdmissionState {
    sealed: bool,
    profiles: HashMap<ProfileId, ProfileAdmission>,
    catalog_transition_pending: bool,
}

struct SharedAdmission {
    state: Mutex<AdmissionState>,
}

impl SharedAdmission {
    fn compile_active(&self, profile: ProfileId, generation: ContentPolicyGeneration) -> bool {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .profiles
            .get(&profile)
            == Some(&ProfileAdmission::Compiling { generation })
    }

    fn begin_delivery(&self, profile: ProfileId, generation: ContentPolicyGeneration) -> bool {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let Some(admission) = state.profiles.get_mut(&profile) else {
            return false;
        };
        if *admission != (ProfileAdmission::Compiling { generation }) {
            return false;
        }
        *admission = ProfileAdmission::Delivering { generation };
        true
    }

    fn finish_delivery(&self, profile: ProfileId, generation: ContentPolicyGeneration) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if state.profiles.get(&profile) == Some(&ProfileAdmission::Delivering { generation }) {
            state.profiles.remove(&profile);
        }
    }
}

/// Bounded asynchronous compiler and one-artifact recovery cache.
///
/// One worker serializes expensive parsing, coalesces all profiles onto the
/// same immutable `Arc<ContentRules>`, and never performs I/O in a native
/// request callback. Durable declarative bytes are held weakly after delivery,
/// so their lifetime follows only the exact app/engine/native compilation
/// cohort; the worker retains their authenticated reload recipe. A missing or
/// failed persistent cache instead keeps the successful artifact strong. The
/// queue is bounded by the maximum profile cohort.
pub struct WorkerBlocker {
    // Taking and dropping the sole sender is the worker's shutdown signal.
    // This lets the idle worker block indefinitely instead of polling.
    queue: Mutex<Option<SyncSender<WorkerCommand>>>,
    admission: Arc<SharedAdmission>,
    worker: Mutex<Option<JoinHandle<()>>>,
    worker_done: Mutex<Receiver<()>>,
    shutdown_serial: Mutex<()>,
    completed: AtomicBool,
}

impl WorkerBlocker {
    /// Starts the compiler worker for the current native backend.
    pub fn start(catalog: StaticPolicyCatalog) -> std::io::Result<Arc<Self>> {
        Self::start_inner(PolicyCatalog::eager(catalog), None)
    }

    /// Starts the compiler with an optional, private persistent artifact cache.
    ///
    /// Cache admission, locking, reads, validation, writes, and cleanup occur
    /// only on the compiler worker. An unsafe or unavailable cache silently
    /// degrades to the same bounded source compilation used by [`Self::start`].
    pub fn start_with_cache(
        catalog: StaticPolicyCatalog,
        cache: CompiledArtifactCacheConfig,
    ) -> std::io::Result<Arc<Self>> {
        Self::start_inner(PolicyCatalog::eager(catalog), Some(cache))
    }

    /// Starts the compiler with authenticated eager or deferred catalog
    /// material and a private persistent artifact cache.
    pub fn start_with_policy_catalog(
        catalog: PolicyCatalog,
        cache: CompiledArtifactCacheConfig,
    ) -> std::io::Result<Arc<Self>> {
        Self::start_inner(catalog, Some(cache))
    }

    fn start_inner(
        catalog: PolicyCatalog,
        cache: Option<CompiledArtifactCacheConfig>,
    ) -> std::io::Result<Arc<Self>> {
        // At most one compile and one ordered retirement marker can exist for
        // each admitted profile. The final slot is reserved for the single
        // authenticated-catalog replacement. This bound makes control work
        // non-starvable without an unbounded side channel.
        let capacity = zephium_core::session::MAX_SESSION_PROFILES
            .saturating_mul(2)
            .saturating_add(1);
        let (queue, receiver) = mpsc::sync_channel(capacity);
        let (done_tx, done_rx) = mpsc::sync_channel(1);
        let admission = Arc::new(SharedAdmission {
            state: Mutex::new(AdmissionState::default()),
        });
        let worker_admission = admission.clone();
        let worker = thread::Builder::new()
            .name("zephium-blocker".into())
            .spawn(move || {
                run_worker(receiver, worker_admission, catalog, cache);
                let _ = done_tx.send(());
            })?;
        Ok(Arc::new(Self {
            queue: Mutex::new(Some(queue)),
            admission,
            worker: Mutex::new(Some(worker)),
            worker_done: Mutex::new(done_rx),
            shutdown_serial: Mutex::new(()),
            completed: AtomicBool::new(false),
        }))
    }

    /// Orders one immutable, already-authenticated catalog replacement after
    /// every compile accepted before this call.
    ///
    /// Only one replacement may be pending. `Scheduled` transfers ownership
    /// of `done`; the worker invokes it exactly once after the new catalog is
    /// authoritative and every older compile callback has returned. Callers
    /// may then request recompilation without a race against an old catalog.
    pub fn replace_catalog(
        &self,
        catalog: StaticPolicyCatalog,
        done: Box<dyn FnOnce() + Send>,
    ) -> CatalogReplacementDispatch {
        let mut state = self
            .admission
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if state.sealed {
            return CatalogReplacementDispatch::Terminal;
        }
        if state.catalog_transition_pending {
            return CatalogReplacementDispatch::Rejected;
        }
        state.catalog_transition_pending = true;
        let command = WorkerCommand::ReplaceCatalog(CatalogReplacement { catalog, done });
        let queue = self
            .queue
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let result = match queue.as_ref() {
            Some(queue) => queue.try_send(command),
            None => Err(TrySendError::Disconnected(command)),
        };
        drop(queue);
        match result {
            Ok(()) => CatalogReplacementDispatch::Scheduled,
            Err(TrySendError::Full(_)) => {
                state.catalog_transition_pending = false;
                CatalogReplacementDispatch::Rejected
            }
            Err(TrySendError::Disconnected(_)) => {
                state.catalog_transition_pending = false;
                CatalogReplacementDispatch::Terminal
            }
        }
    }

    /// Compiles and persists a candidate without displacing the active
    /// catalog. Completion is ordered after all previously accepted work.
    pub fn prepare_catalog(
        &self,
        identity: [u8; 32],
        catalog: PolicyCatalog,
        done: Box<dyn FnOnce(CatalogPreparationOutcome) + Send>,
    ) -> CatalogReplacementDispatch {
        self.prepare_catalog_rules(
            identity,
            catalog,
            Box::new(move |result| {
                done(match result {
                    Ok(_) => CatalogPreparationOutcome::Prepared,
                    Err(failure) => CatalogPreparationOutcome::Failed(failure),
                });
            }),
        )
    }

    /// Prepares the exact candidate and transfers bounded artifact ownership
    /// for native preflight, without changing the current source authority.
    pub fn prepare_catalog_rules(
        &self,
        identity: [u8; 32],
        catalog: PolicyCatalog,
        done: Box<dyn FnOnce(Result<Arc<ContentRules>, BlockerCompileFailure>) + Send>,
    ) -> CatalogReplacementDispatch {
        let mut state = self
            .admission
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if state.sealed {
            return CatalogReplacementDispatch::Terminal;
        }
        if state.catalog_transition_pending {
            return CatalogReplacementDispatch::Rejected;
        }
        state.catalog_transition_pending = true;
        let command = WorkerCommand::PrepareCatalog(CatalogPreparation {
            identity,
            catalog,
            done,
        });
        let queue = self
            .queue
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let result = match queue.as_ref() {
            Some(queue) => queue.try_send(command),
            None => Err(TrySendError::Disconnected(command)),
        };
        match result {
            Ok(()) => CatalogReplacementDispatch::Scheduled,
            Err(TrySendError::Full(_)) => {
                state.catalog_transition_pending = false;
                CatalogReplacementDispatch::Rejected
            }
            Err(TrySendError::Disconnected(_)) => {
                state.catalog_transition_pending = false;
                CatalogReplacementDispatch::Terminal
            }
        }
    }

    /// Atomically swaps one exact successfully prepared catalog.
    pub fn activate_prepared_catalog(
        &self,
        identity: [u8; 32],
        done: Box<dyn FnOnce(bool) + Send>,
    ) -> CatalogReplacementDispatch {
        let mut state = self
            .admission
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if state.sealed {
            return CatalogReplacementDispatch::Terminal;
        }
        if state.catalog_transition_pending {
            return CatalogReplacementDispatch::Rejected;
        }
        state.catalog_transition_pending = true;
        let command = WorkerCommand::ActivateCatalog(CatalogActivation { identity, done });
        let queue = self
            .queue
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let result = match queue.as_ref() {
            Some(queue) => queue.try_send(command),
            None => Err(TrySendError::Disconnected(command)),
        };
        match result {
            Ok(()) => CatalogReplacementDispatch::Scheduled,
            Err(TrySendError::Full(_)) => {
                state.catalog_transition_pending = false;
                CatalogReplacementDispatch::Rejected
            }
            Err(TrySendError::Disconnected(_)) => {
                state.catalog_transition_pending = false;
                CatalogReplacementDispatch::Terminal
            }
        }
    }

    /// Drops one exact prepared candidate without changing the active catalog.
    ///
    /// This is the rollback half of the prepare/commit handshake. Callers must
    /// invoke it whenever durable promotion fails after successful compiler
    /// preparation so a full candidate artifact is not retained indefinitely.
    pub fn discard_prepared_catalog(
        &self,
        identity: [u8; 32],
        done: Box<dyn FnOnce(bool) + Send>,
    ) -> CatalogReplacementDispatch {
        let mut state = self
            .admission
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if state.sealed {
            return CatalogReplacementDispatch::Terminal;
        }
        if state.catalog_transition_pending {
            return CatalogReplacementDispatch::Rejected;
        }
        state.catalog_transition_pending = true;
        let command = WorkerCommand::DiscardPreparedCatalog(CatalogActivation { identity, done });
        let queue = self
            .queue
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let result = match queue.as_ref() {
            Some(queue) => queue.try_send(command),
            None => Err(TrySendError::Disconnected(command)),
        };
        match result {
            Ok(()) => CatalogReplacementDispatch::Scheduled,
            Err(TrySendError::Full(_)) => {
                state.catalog_transition_pending = false;
                CatalogReplacementDispatch::Rejected
            }
            Err(TrySendError::Disconnected(_)) => {
                state.catalog_transition_pending = false;
                CatalogReplacementDispatch::Terminal
            }
        }
    }
}

/// Result of compiling a catalog candidate without activating it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CatalogPreparationOutcome {
    /// The exact catalog has a validated recoverable artifact.
    ///
    /// A healthy persistent cache permits declarative bytes to be released
    /// after preparation; an unavailable cache instead retains the successful
    /// in-memory artifact strongly. Activation revalidates this proof before
    /// changing compiler authority.
    Prepared,
    /// Compilation failed while the prior catalog remained authoritative.
    Failed(BlockerCompileFailure),
}

/// Admission result for one ordered authenticated-catalog replacement.
#[must_use]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CatalogReplacementDispatch {
    /// The worker owns the catalog and exactly-once completion.
    Scheduled,
    /// Another replacement is pending or the bounded control slot was not
    /// available. The caller retains responsibility for retry/coalescing.
    Rejected,
    /// Compiler shutdown sealed all future work.
    Terminal,
}

impl BlockerCompiler for WorkerBlocker {
    fn prepare_site_preferences(
        &self,
        preferences: &zephium_core::blocker::BlockerSitePreferences,
    ) -> Option<Arc<zephium_core::blocker::PreparedBlockerSites>> {
        crate::prepare_site_preferences(preferences)
    }
    fn validate_personal_selector(&self, selector: &str) -> Option<String> {
        crate::validate_personal_selector(selector)
    }
    fn compile(
        &self,
        profile: ProfileId,
        generation: ContentPolicyGeneration,
        config: BlockerConfig,
        done: Box<dyn FnOnce(BlockerCompileOutcome) + Send>,
    ) -> BlockerDispatch {
        let mut state = self
            .admission
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if state.sealed {
            return BlockerDispatch::Terminal;
        }
        if state.profiles.contains_key(&profile)
            || state.profiles.len() >= zephium_core::session::MAX_SESSION_PROFILES
        {
            return BlockerDispatch::Rejected;
        }
        state
            .profiles
            .insert(profile, ProfileAdmission::Compiling { generation });
        let job = WorkerCommand::Compile(CompileJob {
            profile,
            generation,
            config,
            done,
        });
        let queue = self
            .queue
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let result = match queue.as_ref() {
            Some(queue) => queue.try_send(job),
            None => Err(TrySendError::Disconnected(job)),
        };
        match result {
            Ok(()) => BlockerDispatch::Scheduled,
            Err(TrySendError::Full(_)) => {
                if state.profiles.get(&profile) == Some(&ProfileAdmission::Compiling { generation })
                {
                    state.profiles.remove(&profile);
                }
                BlockerDispatch::Rejected
            }
            Err(TrySendError::Disconnected(_)) => {
                state.sealed = true;
                if state.profiles.get(&profile) == Some(&ProfileAdmission::Compiling { generation })
                {
                    state.profiles.remove(&profile);
                }
                BlockerDispatch::Terminal
            }
        }
    }

    fn retire_profile(
        &self,
        profile: ProfileId,
        done: Box<dyn FnOnce() + Send>,
    ) -> BlockerRetirementDispatch {
        let mut state = self
            .admission
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if state.sealed {
            return BlockerRetirementDispatch::Terminal;
        }
        let Some(previous) = state.profiles.get(&profile).copied() else {
            drop(state);
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(done));
            return BlockerRetirementDispatch::Quiesced;
        };
        if matches!(previous, ProfileAdmission::Retiring { .. }) {
            return BlockerRetirementDispatch::AlreadyScheduled;
        }
        let generation = previous.generation();
        state
            .profiles
            .insert(profile, ProfileAdmission::Retiring { generation });
        let command = WorkerCommand::Retire(ProfileRetirement {
            profile,
            generation,
            done,
        });
        let queue = self
            .queue
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let result = match queue.as_ref() {
            Some(queue) => queue.try_send(command),
            None => Err(TrySendError::Disconnected(command)),
        };
        match result {
            Ok(()) => BlockerRetirementDispatch::Scheduled,
            Err(TrySendError::Full(_)) => {
                if state.profiles.get(&profile) == Some(&ProfileAdmission::Retiring { generation })
                {
                    state.profiles.insert(profile, previous);
                }
                BlockerRetirementDispatch::Rejected
            }
            Err(TrySendError::Disconnected(_)) => {
                state.sealed = true;
                BlockerRetirementDispatch::Terminal
            }
        }
    }

    fn shutdown_until(&self, deadline: Instant) -> BlockerShutdownOutcome {
        let shutdown_deadline = deadline;
        if self.completed.load(Ordering::Acquire) {
            return BlockerShutdownOutcome::Clean;
        }
        let Some(_serial) = lock_until(&self.shutdown_serial, shutdown_deadline) else {
            return if self.completed.load(Ordering::Acquire) {
                BlockerShutdownOutcome::Clean
            } else {
                BlockerShutdownOutcome::Unclean
            };
        };
        if self.completed.load(Ordering::Acquire) {
            return BlockerShutdownOutcome::Clean;
        }
        let Some(mut admission) = lock_until(&self.admission.state, shutdown_deadline) else {
            return BlockerShutdownOutcome::Unclean;
        };
        admission.sealed = true;
        drop(admission);
        // `compile` and shutdown acquire admission before the queue, so once
        // the sealed state is published no sender can race this take. The
        // receiver drains the bounded accepted cohort and then observes
        // disconnection without an idle timer.
        let Some(mut queue) = lock_until(&self.queue, shutdown_deadline) else {
            return BlockerShutdownOutcome::Unclean;
        };
        queue.take();
        drop(queue);

        let Some(mut worker_slot) = lock_until(&self.worker, shutdown_deadline) else {
            return BlockerShutdownOutcome::Unclean;
        };
        let Some(worker) = worker_slot.take() else {
            return BlockerShutdownOutcome::Unclean;
        };
        drop(worker_slot);
        let Some(worker_done) = lock_until(&self.worker_done, shutdown_deadline) else {
            drop(worker);
            return BlockerShutdownOutcome::Unclean;
        };
        let exited = worker_exit_proven_until(&worker_done, &worker, shutdown_deadline);
        drop(worker_done);
        if !exited || !worker.is_finished() {
            drop(worker);
            return BlockerShutdownOutcome::Unclean;
        }
        if worker.join().is_err() {
            return BlockerShutdownOutcome::Unclean;
        }
        self.completed.store(true, Ordering::Release);
        BlockerShutdownOutcome::Clean
    }
}

impl Drop for WorkerBlocker {
    fn drop(&mut self) {
        match self.admission.state.try_lock() {
            Ok(mut state) => state.sealed = true,
            Err(TryLockError::Poisoned(poisoned)) => poisoned.into_inner().sealed = true,
            Err(TryLockError::WouldBlock) => {}
        }
        self.queue
            .get_mut()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
    }
}

fn lock_until<T>(mutex: &Mutex<T>, deadline: Instant) -> Option<MutexGuard<'_, T>> {
    loop {
        if Instant::now() >= deadline {
            return None;
        }
        match mutex.try_lock() {
            Ok(guard) => return Some(guard),
            Err(TryLockError::Poisoned(poisoned)) => return Some(poisoned.into_inner()),
            Err(TryLockError::WouldBlock) => {}
        }
        let remaining = deadline.checked_duration_since(Instant::now())?;
        thread::park_timeout(remaining.min(SHUTDOWN_POLL_INTERVAL));
    }
}

fn worker_exit_proven_until(
    done: &Receiver<()>,
    worker: &JoinHandle<()>,
    deadline: Instant,
) -> bool {
    if Instant::now() >= deadline {
        return false;
    }
    if worker.is_finished() {
        return true;
    }
    let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
        return false;
    };
    match done.recv_timeout(remaining) {
        Ok(()) | Err(RecvTimeoutError::Disconnected) => {}
        Err(RecvTimeoutError::Timeout) => return false,
    }
    loop {
        if Instant::now() >= deadline {
            return false;
        }
        if worker.is_finished() {
            return true;
        }
        let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
            return false;
        };
        thread::park_timeout(remaining.min(SHUTDOWN_POLL_INTERVAL));
    }
}

fn run_worker(
    receiver: Receiver<WorkerCommand>,
    admission: Arc<SharedAdmission>,
    catalog: PolicyCatalog,
    cache: Option<CompiledArtifactCacheConfig>,
) {
    let compiler = Compiler::default();
    let allow_all = ContentRules::allow_all(allow_all_digest());
    let mut persistent = cache
        .as_ref()
        .and_then(|cache| PersistentArtifactCache::open(cache).ok());
    let mut artifacts = ArtifactCache::new(catalog);
    let mut prepared: Option<([u8; 32], ArtifactCache)> = None;

    while let Ok(command) = receiver.recv() {
        match command {
            WorkerCommand::Compile(job) => {
                complete_job(
                    job,
                    &admission,
                    compiler,
                    &allow_all,
                    &mut artifacts,
                    &mut persistent,
                );
            }
            WorkerCommand::Retire(retirement) => {
                let should_complete = {
                    let mut state = admission
                        .state
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    if state.profiles.get(&retirement.profile)
                        == Some(&ProfileAdmission::Retiring {
                            generation: retirement.generation,
                        })
                    {
                        state.profiles.remove(&retirement.profile);
                        true
                    } else {
                        false
                    }
                };
                if should_complete {
                    // Admission reopens before completion, but this single
                    // worker cannot deliver a newly queued compile callback
                    // until `done` returns.
                    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        (retirement.done)();
                    }));
                }
            }
            WorkerCommand::ReplaceCatalog(replacement) => {
                artifacts.replace_catalog(PolicyCatalog::eager(replacement.catalog));
                prepared = None;
                {
                    let mut state = admission
                        .state
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    state.catalog_transition_pending = false;
                }
                let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    (replacement.done)();
                }));
            }
            WorkerCommand::PrepareCatalog(preparation) => {
                prepared = None;
                let mut candidate = ArtifactCache::new(preparation.catalog);
                let outcome = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    crate::priority::in_background(|| candidate.resolve(compiler, &mut persistent))
                })) {
                    Ok(BlockerCompileOutcome::Compiled(rules)) => {
                        prepared = Some((preparation.identity, candidate));
                        Ok(rules)
                    }
                    Ok(BlockerCompileOutcome::Failed(failure)) => Err(failure),
                    Err(_) => Err(BlockerCompileFailure::Internal),
                };
                admission
                    .state
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .catalog_transition_pending = false;
                let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    (preparation.done)(outcome);
                }));
            }
            WorkerCommand::ActivateCatalog(activation) => {
                let success = match prepared.take() {
                    Some((identity, mut candidate)) if identity == activation.identity => {
                        // Preparation may deliberately retain only a weak
                        // declarative handle after proving the persistent
                        // artifact. Revalidate recoverability at the activation
                        // linearization point so cache/source damage between
                        // prepare and commit cannot publish an unusable
                        // compiler authority.
                        let recoverable =
                            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                candidate.resolve(compiler, &mut persistent)
                            }));
                        if matches!(recoverable, Ok(BlockerCompileOutcome::Compiled(_))) {
                            artifacts = candidate;
                            true
                        } else {
                            prepared = Some((identity, candidate));
                            false
                        }
                    }
                    Some(candidate) => {
                        // A stale or contradictory activation must not destroy
                        // the one exact prepared candidate. Its caller receives
                        // false and may retry only with the matching identity.
                        prepared = Some(candidate);
                        false
                    }
                    None => false,
                };
                admission
                    .state
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .catalog_transition_pending = false;
                let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    (activation.done)(success);
                }));
            }
            WorkerCommand::DiscardPreparedCatalog(discard) => {
                let discarded = match prepared.take() {
                    Some((identity, _)) if identity == discard.identity => true,
                    Some(candidate) => {
                        prepared = Some(candidate);
                        false
                    }
                    None => false,
                };
                admission
                    .state
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .catalog_transition_pending = false;
                let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    (discard.done)(discarded);
                }));
            }
        }
    }
}

#[derive(Clone)]
enum CachedArtifact {
    Strong(Arc<ContentRules>),
    Declarative(Weak<ContentRules>),
    Failed(BlockerCompileFailure),
}

impl CachedArtifact {
    fn compiled(rules: &Arc<ContentRules>, durably_cached: bool) -> Self {
        if durably_cached
            && matches!(
                rules.payload(),
                zephium_core::blocker::ContentRulesPayload::Declarative { .. }
            )
        {
            Self::Declarative(Arc::downgrade(rules))
        } else {
            // Runtime policies are also retained by the native synchronous
            // matcher. Keeping the compiler's strong handle avoids rebuilding
            // that matcher merely because the small ContentRules envelope
            // itself was released. A declarative artifact stays strong here
            // only when no durable recovery copy was proven.
            Self::Strong(rules.clone())
        }
    }

    fn outcome(&self) -> Option<BlockerCompileOutcome> {
        match self {
            Self::Strong(rules) => Some(BlockerCompileOutcome::Compiled(rules.clone())),
            Self::Declarative(rules) => rules.upgrade().map(BlockerCompileOutcome::Compiled),
            Self::Failed(failure) => Some(BlockerCompileOutcome::Failed(*failure)),
        }
    }

    fn retains_reload_recipe(&self) -> bool {
        matches!(self, Self::Declarative(_))
    }
}

struct ArtifactCache {
    catalog: Option<PolicyCatalog>,
    artifact: Option<CachedArtifact>,
    #[cfg(test)]
    compilation_attempts: usize,
    #[cfg(test)]
    persistent_hits: usize,
}

impl ArtifactCache {
    fn new(catalog: PolicyCatalog) -> Self {
        Self {
            catalog: Some(catalog),
            artifact: None,
            #[cfg(test)]
            compilation_attempts: 0,
            #[cfg(test)]
            persistent_hits: 0,
        }
    }

    fn replace_catalog(&mut self, catalog: PolicyCatalog) {
        self.catalog = Some(catalog);
        self.artifact = None;
        #[cfg(test)]
        {
            self.compilation_attempts = 0;
            self.persistent_hits = 0;
        }
    }

    fn resolve(
        &mut self,
        compiler: Compiler,
        persistent: &mut Option<PersistentArtifactCache>,
    ) -> BlockerCompileOutcome {
        if let Some(cached) = &self.artifact {
            if let Some(outcome) = cached.outcome() {
                return outcome;
            }
            // The declarative bytes outlived neither an application delivery
            // nor native compilation. Recover from the checksummed persistent
            // artifact, or from the retained authenticated source recipe if
            // that cache has since become unavailable.
            self.artifact = None;
        }
        let Some(catalog) = self.catalog.as_ref() else {
            let failure = CachedArtifact::Failed(BlockerCompileFailure::Internal);
            let outcome = BlockerCompileOutcome::Failed(BlockerCompileFailure::Internal);
            self.artifact = Some(failure);
            return outcome;
        };
        let target = native_target();
        let cache_key = match catalog.cache_key(target, compiler.limits()) {
            Ok(cache_key) => cache_key,
            Err(failure) => {
                let outcome = BlockerCompileOutcome::Failed(failure);
                self.artifact = Some(CachedArtifact::Failed(failure));
                return outcome;
            }
        };
        if let Some(cache) = persistent.as_ref() {
            match cache.load(cache_key, target, compiler.limits()) {
                Ok(Some(loaded)) => {
                    if let Some(rules) = adapt_loaded_rules(loaded) {
                        #[cfg(test)]
                        {
                            self.persistent_hits += 1;
                        }
                        let artifact = CachedArtifact::compiled(&rules, true);
                        let retain_catalog = artifact.retains_reload_recipe();
                        let outcome = BlockerCompileOutcome::Compiled(rules);
                        self.artifact = Some(artifact);
                        if !retain_catalog {
                            self.catalog = None;
                        }
                        return outcome;
                    }
                    *persistent = None;
                }
                Ok(None) => {}
                Err(_) => *persistent = None,
            }
        }
        #[cfg(test)]
        {
            self.compilation_attempts += 1;
        }
        let catalog = match self
            .catalog
            .as_ref()
            .ok_or(BlockerCompileFailure::Internal)
            .and_then(PolicyCatalog::load)
        {
            Ok(catalog) => catalog,
            // Source storage is independently repairable. Retain the reusable
            // loader and do not turn a transient or corrected authenticated
            // storage failure into a process-lifetime cache entry.
            Err(failure) => return BlockerCompileOutcome::Failed(failure),
        };
        let failure = if catalog.is_empty() {
            BlockerCompileFailure::SourceUnavailable
        } else {
            match compiler.compile(target, with_own_rules(catalog.instantiate(), target)) {
                Ok(compiled) => {
                    let persistent_copy = compiled.clone();
                    match adapt_rules(compiled) {
                        Ok(rules) => {
                            let navigation = compile_navigation(&catalog, compiler);
                            let rules = with_navigation(rules, navigation.clone());
                            let durably_cached = match persistent.as_ref() {
                                Some(cache)
                                    if cache
                                        .store(
                                            cache_key,
                                            &persistent_copy,
                                            &rules,
                                            navigation.as_ref(),
                                        )
                                        .is_ok() =>
                                {
                                    true
                                }
                                Some(_) => {
                                    *persistent = None;
                                    false
                                }
                                None => false,
                            };
                            let artifact = CachedArtifact::compiled(&rules, durably_cached);
                            let retain_catalog = artifact.retains_reload_recipe();
                            self.artifact = Some(artifact);
                            if !retain_catalog {
                                self.catalog = None;
                            }
                            return BlockerCompileOutcome::Compiled(rules);
                        }
                        Err(failure) => failure,
                    }
                }
                Err(error) => map_compile_error(&error),
            }
        };
        self.artifact = Some(CachedArtifact::Failed(failure));
        self.catalog = None;
        BlockerCompileOutcome::Failed(failure)
    }
}

fn complete_job(
    job: CompileJob,
    admission: &SharedAdmission,
    compiler: Compiler,
    allow_all: &Arc<ContentRules>,
    artifacts: &mut ArtifactCache,
    persistent: &mut Option<PersistentArtifactCache>,
) {
    if !admission.compile_active(job.profile, job.generation) {
        return;
    }
    let outcome = if !job.config.enabled {
        BlockerCompileOutcome::Compiled(allow_all.clone())
    } else {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            artifacts.resolve(compiler, persistent)
        }))
        .unwrap_or(BlockerCompileOutcome::Failed(
            BlockerCompileFailure::Internal,
        ))
    };
    if admission.begin_delivery(job.profile, job.generation) {
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            (job.done)(outcome);
        }));
        admission.finish_delivery(job.profile, job.generation);
    }
}

/// uBlock Origin's list of scam and malware sites, which alone judges each
/// page and frame as it starts loading: WebKit gets none of its wildcard
/// rules, and a refused page is named as a scam or malware site.
#[cfg(feature = "runtime")]
const NAVIGATION_SOURCE: &str = "ublock-badware";

/// The badware list alone, compiled for the runtime matcher.
#[cfg(feature = "runtime")]
fn compile_navigation(catalog: &StaticPolicyCatalog, compiler: Compiler) -> Option<CompiledRules> {
    #[allow(unused_mut)]
    let mut sources: Vec<_> = catalog
        .instantiate()
        .into_iter()
        .filter(|source| source.id().as_str() == NAVIGATION_SOURCE)
        .collect();
    #[cfg(feature = "qa-test-hosts")]
    sources.push(FilterSource::new(
        crate::SourceId::new("zephium-qa-badware").expect("a valid source ID"),
        crate::SourceFormat::Standard,
        crate::builtin::QA_BADWARE_RULES.to_owned(),
    ));
    if sources.is_empty() {
        return None;
    }
    compiler
        .compile(CompileTarget::Runtime, sources)
        .map_err(|error| eprintln!("content blocker: navigation check unavailable: {error}"))
        .ok()
}

#[cfg(not(feature = "runtime"))]
fn compile_navigation(_: &StaticPolicyCatalog, _: Compiler) -> Option<CompiledRules> {
    None
}

#[cfg(feature = "runtime")]
fn with_navigation(
    rules: Arc<ContentRules>,
    navigation: Option<CompiledRules>,
) -> Arc<ContentRules> {
    match navigation {
        Some(navigation) if rules.enabled() => {
            rules.with_navigation(Arc::new(RuntimePolicy::compiled(navigation)))
        }
        _ => rules,
    }
}

#[cfg(not(feature = "runtime"))]
fn with_navigation(rules: Arc<ContentRules>, _: Option<CompiledRules>) -> Arc<ContentRules> {
    rules
}

fn with_own_rules(mut sources: Vec<FilterSource>, target: CompileTarget) -> Vec<FilterSource> {
    if crate::builtin::applies_to(target) {
        sources.push(crate::builtin::source());
    }
    sources
}

#[cfg(target_os = "windows")]
const fn native_target() -> CompileTarget {
    CompileTarget::Runtime
}

#[cfg(not(target_os = "windows"))]
const fn native_target() -> CompileTarget {
    CompileTarget::WebKit
}

pub(crate) fn adapt_rules(
    rules: CompiledRules,
) -> Result<Arc<ContentRules>, BlockerCompileFailure> {
    let cosmetics = rules.cosmetics().cloned();
    let report = rules.report();
    let (
        platform_omitted_rules,
        platform_approximated_rules,
        platform_resource_approximated_rules,
        platform_source_kind_approximated_rules,
        platform_attribution_approximated_rules,
    ) = match rules.target() {
        CompileTarget::Runtime => (
            report.runtime_omitted_rules(),
            report.runtime_approximated_rules(),
            report.runtime_resource_approximated_rules(),
            report.runtime_source_kind_approximated_rules(),
            0,
        ),
        CompileTarget::WebKit => {
            let omitted = report
                .webkit()
                .map_or(0, |webkit| webkit.omitted_input_rules());
            let approximated = report
                .webkit()
                .map_or(0, |webkit| webkit.approximated_input_rules());
            let resource_approximated = report
                .webkit()
                .map_or(0, |webkit| webkit.resource_approximated_input_rules());
            let attribution_approximated = report
                .webkit()
                .map_or(0, |webkit| webkit.attribution_approximated_input_rules());
            (
                omitted,
                approximated,
                resource_approximated,
                0,
                attribution_approximated,
            )
        }
    };
    let coverage = ContentRuleCoverage {
        source_rules: u64::try_from(report.candidate_rules())
            .map_err(|_| BlockerCompileFailure::ResourceLimit)?,
        accepted_rules: u64::try_from(report.accepted_rules())
            .map_err(|_| BlockerCompileFailure::ResourceLimit)?,
        rejected_rules: u64::try_from(report.rejected_rules())
            .map_err(|_| BlockerCompileFailure::ResourceLimit)?,
        platform_omitted_rules: u64::try_from(platform_omitted_rules)
            .map_err(|_| BlockerCompileFailure::ResourceLimit)?,
        platform_approximated_rules: u64::try_from(platform_approximated_rules)
            .map_err(|_| BlockerCompileFailure::ResourceLimit)?,
        platform_resource_approximated_rules: u64::try_from(platform_resource_approximated_rules)
            .map_err(|_| BlockerCompileFailure::ResourceLimit)?,
        platform_source_kind_approximated_rules: u64::try_from(
            platform_source_kind_approximated_rules,
        )
        .map_err(|_| BlockerCompileFailure::ResourceLimit)?,
        platform_attribution_approximated_rules: u64::try_from(
            platform_attribution_approximated_rules,
        )
        .map_err(|_| BlockerCompileFailure::ResourceLimit)?,
        blocking_rule_entries: u64::try_from(report.native_blocking_rule_entries())
            .map_err(|_| BlockerCompileFailure::ResourceLimit)?,
    };
    let result = match rules.target() {
        CompileTarget::Runtime => {
            if let Err(error) = rules.verify_official_canary() {
                eprintln!("content blocker: lists refused: {error}");
                return Err(BlockerCompileFailure::InvalidSource);
            }
            let digest = ContentRuleDigest::from_bytes(*rules.digest().as_bytes());
            ContentRules::runtime(digest, coverage, Arc::new(RuntimePolicy::compiled(rules)))
                .ok_or(BlockerCompileFailure::Internal)
        }
        CompileTarget::WebKit => {
            let webkit = rules.webkit().ok_or(BlockerCompileFailure::Internal)?;
            let digest = ContentRuleDigest::from_bytes(*rules.digest().as_bytes());
            let content_rules = ContentRules::declarative(
                digest,
                coverage,
                zephium_core::blocker::DeclarativeRuleFormat::WebKitContentBlockerV1,
                webkit.encoded(),
            )
            .ok_or(BlockerCompileFailure::ResourceLimit)?;
            let zephium_core::blocker::ContentRulesPayload::Declarative {
                artifact_digest, ..
            } = content_rules.payload()
            else {
                return Err(BlockerCompileFailure::Internal);
            };
            if artifact_digest.as_bytes() != webkit.digest().as_bytes() {
                return Err(BlockerCompileFailure::Internal);
            }
            Ok(content_rules)
        }
    }?;
    Ok(attach_cosmetics(result, cosmetics))
}

fn attach_cosmetics(
    rules: Arc<ContentRules>,
    cosmetics: Option<crate::cosmetics::PreparedCosmetics>,
) -> Arc<ContentRules> {
    match cosmetics {
        Some(cosmetics) => rules.with_cosmetics(cosmetics.policy),
        None => rules,
    }
}

fn adapt_loaded_rules(loaded: LoadedArtifact) -> Option<Arc<ContentRules>> {
    match loaded {
        #[cfg(feature = "runtime")]
        LoadedArtifact::Runtime {
            digest,
            coverage,
            rules,
            cosmetics,
            navigation,
        } => ContentRules::runtime(digest, coverage, Arc::new(RuntimePolicy::persistent(rules)))
            .map(|rules| attach_loaded_navigation(attach_cosmetics(rules, cosmetics), navigation)),
        #[cfg(feature = "webkit")]
        LoadedArtifact::WebKit {
            digest,
            coverage,
            artifact_digest,
            encoded,
            cosmetics,
            #[cfg(feature = "runtime")]
            navigation,
        } => {
            let rules = ContentRules::declarative_with_digest(
                digest,
                coverage,
                zephium_core::blocker::DeclarativeRuleFormat::WebKitContentBlockerV1,
                artifact_digest,
                encoded,
            )?;
            let rules = attach_cosmetics(rules, cosmetics);
            #[cfg(feature = "runtime")]
            let rules = attach_loaded_navigation(rules, navigation);
            Some(rules)
        }
    }
}

#[cfg(feature = "runtime")]
fn attach_loaded_navigation(
    rules: Arc<ContentRules>,
    navigation: Option<Box<CachedRuntimeRules>>,
) -> Arc<ContentRules> {
    match navigation {
        Some(navigation) => rules.with_navigation(Arc::new(RuntimePolicy::persistent(navigation))),
        None => rules,
    }
}

enum RuntimePolicyArtifact {
    Compiled(CompiledRules),
    #[cfg(feature = "runtime")]
    Persistent(Box<CachedRuntimeRules>),
}

#[derive(Default)]
struct RuntimePolicyCounters {
    total_decisions: AtomicU64,
    candidate_budget_exhausted: AtomicU64,
    matcher_unavailable: AtomicU64,
    matcher_unprepared: AtomicU64,
    attribution_unavailable: AtomicU64,
    evaluation_errors: AtomicU64,
}

impl RuntimePolicyCounters {
    fn increment(counter: &AtomicU64, success: Ordering) {
        // Never let a long-running browser turn diagnostics into a wrapping
        // value. The ordinary decision path needs only one relaxed atomic
        // update. Exceptional updates publish after the total update so an
        // acquiring snapshot cannot observe a classified decision without
        // also observing its membership in `total_decisions`.
        let _ = counter.fetch_update(success, Ordering::Relaxed, |value| value.checked_add(1));
    }

    fn record_total(&self) {
        Self::increment(&self.total_decisions, Ordering::Relaxed);
    }

    fn record_attribution_unavailable(&self) {
        Self::increment(&self.attribution_unavailable, Ordering::Release);
    }

    fn record_error(&self, error: crate::MatchError) {
        let counter = match error {
            crate::MatchError::CandidateBudgetExhausted => &self.candidate_budget_exhausted,
            crate::MatchError::MatcherUnavailable => &self.matcher_unavailable,
            crate::MatchError::MatcherUnprepared => &self.matcher_unprepared,
            crate::MatchError::WrongArtifactTarget
            | crate::MatchError::RequestUrlTooLong { .. }
            | crate::MatchError::SourceUrlTooLong { .. }
            | crate::MatchError::InvalidRequest
            | crate::MatchError::ExactAttributionUnsupported
            | crate::MatchError::UnexpectedMutation => &self.evaluation_errors,
        };
        Self::increment(counter, Ordering::Release);
    }

    fn snapshot(&self) -> NetworkPolicyDiagnostics {
        // Exceptional counters are acquired first. Every writer records total
        // before releasing its exceptional class, so the final total load
        // includes every classified decision observed by this snapshot.
        NetworkPolicyDiagnostics {
            candidate_budget_exhausted: self.candidate_budget_exhausted.load(Ordering::Acquire),
            matcher_unavailable: self.matcher_unavailable.load(Ordering::Acquire),
            matcher_unprepared: self.matcher_unprepared.load(Ordering::Acquire),
            attribution_unavailable: self.attribution_unavailable.load(Ordering::Acquire),
            evaluation_errors: self.evaluation_errors.load(Ordering::Acquire),
            total_decisions: self.total_decisions.load(Ordering::Acquire),
        }
    }
}

struct RuntimePolicy {
    artifact: RuntimePolicyArtifact,
    counters: RuntimePolicyCounters,
    #[cfg(feature = "runtime-exact")]
    sources: Mutex<RecentSources>,
}

/// Documents that recently sent requests, newest first. Every view of a
/// profile shares one policy, so a few entries cover the pages loading at once.
#[cfg(feature = "runtime-exact")]
#[derive(Default)]
struct RecentSources(Vec<(Box<str>, Arc<SourceDocument>)>);

#[cfg(feature = "runtime-exact")]
impl RecentSources {
    const CAPACITY: usize = 8;
    /// Longer document URLs are parsed per request rather than held.
    const MAX_URL_BYTES: usize = 2048;

    fn get(&mut self, url: &str) -> Option<Arc<SourceDocument>> {
        let index = self.0.iter().position(|(held, _)| **held == *url)?;
        let entry = self.0.remove(index);
        let source = Arc::clone(&entry.1);
        self.0.insert(0, entry);
        Some(source)
    }

    fn insert(&mut self, url: &str, source: &Arc<SourceDocument>) {
        if url.len() > Self::MAX_URL_BYTES {
            return;
        }
        self.0.truncate(Self::CAPACITY - 1);
        self.0.insert(0, (url.into(), Arc::clone(source)));
    }
}

impl RuntimePolicy {
    fn compiled(rules: CompiledRules) -> Self {
        Self::new(RuntimePolicyArtifact::Compiled(rules))
    }

    #[cfg(feature = "runtime")]
    fn persistent(rules: Box<CachedRuntimeRules>) -> Self {
        Self::new(RuntimePolicyArtifact::Persistent(rules))
    }

    fn new(artifact: RuntimePolicyArtifact) -> Self {
        Self {
            artifact,
            counters: RuntimePolicyCounters::default(),
            #[cfg(feature = "runtime-exact")]
            sources: Mutex::default(),
        }
    }

    #[cfg(feature = "runtime-exact")]
    fn evaluate_from(
        &self,
        url: &str,
        source_url: &str,
        resource_type: ResourceType,
        method: RequestMethod,
    ) -> Result<crate::NetworkDecision, crate::MatchError> {
        let source = self.source_document(source_url)?;
        match &self.artifact {
            RuntimePolicyArtifact::Compiled(rules) => {
                rules.evaluate_from(url, &source, resource_type, method)
            }
            #[cfg(feature = "runtime")]
            RuntimePolicyArtifact::Persistent(rules) => {
                rules.evaluate_from(url, &source, resource_type, method)
            }
        }
    }

    #[cfg(feature = "runtime-exact")]
    fn source_document(&self, url: &str) -> Result<Arc<SourceDocument>, crate::MatchError> {
        // Decisions run on the UI thread: a contended cache parses instead of
        // waiting.
        let mut recent = match self.sources.try_lock() {
            Ok(recent) => Some(recent),
            Err(TryLockError::Poisoned(poisoned)) => Some(poisoned.into_inner()),
            Err(TryLockError::WouldBlock) => None,
        };
        if let Some(source) = recent.as_mut().and_then(|recent| recent.get(url)) {
            return Ok(source);
        }
        let source = Arc::new(match &self.artifact {
            RuntimePolicyArtifact::Compiled(rules) => rules.source_document(url),
            #[cfg(feature = "runtime")]
            RuntimePolicyArtifact::Persistent(rules) => rules.source_document(url),
        }?);
        if let Some(recent) = recent.as_mut() {
            recent.insert(url, &source);
        }
        Ok(source)
    }

    fn evaluate_source_independent(
        &self,
        request: NetworkRequest<'_>,
    ) -> Result<crate::NetworkDecision, crate::MatchError> {
        match &self.artifact {
            RuntimePolicyArtifact::Compiled(rules) => rules.evaluate_source_independent(request),
            #[cfg(feature = "runtime")]
            RuntimePolicyArtifact::Persistent(rules) => rules.evaluate_source_independent(request),
        }
    }
}

impl NetworkRequestPolicy for RuntimePolicy {
    fn decide(&self, request: &CoreRequest<'_>) -> CoreDecision {
        self.counters.record_total();
        let attribution = request.attribution();
        let resource_type = map_resource_type(request.resource_type());
        let method = map_method(request.method());
        let decision = match attribution {
            NetworkAttribution::Exact => {
                #[cfg(feature = "runtime-exact")]
                {
                    let Some(source_url) = request.source_url() else {
                        self.counters.record_attribution_unavailable();
                        return CoreDecision::Allow;
                    };
                    self.evaluate_from(request.url(), source_url, resource_type, method)
                }
                #[cfg(not(feature = "runtime-exact"))]
                {
                    // The shipped WebView2 callback has no exact initiating-frame
                    // URL. The resolver is used for document cosmetics only;
                    // it does not grant request attribution. Treat an unexpected
                    // exact request as a capability mismatch and fail open.
                    self.counters.record_attribution_unavailable();
                    return CoreDecision::Allow;
                }
            }
            // The main document stands in for the frame WebView2 cannot
            // name: `$domain` and party options are judged against it.
            #[cfg(feature = "runtime-exact")]
            NetworkAttribution::TopLevelApproximation => {
                let Some(top_level_url) = request.source_url() else {
                    self.counters.record_attribution_unavailable();
                    return CoreDecision::Allow;
                };
                self.evaluate_from(request.url(), top_level_url, resource_type, method)
            }
            #[cfg(not(feature = "runtime-exact"))]
            NetworkAttribution::TopLevelApproximation => self.evaluate_source_independent(
                NetworkRequest::source_independent(request.url(), resource_type, method),
            ),
            NetworkAttribution::SourceIndependent => self.evaluate_source_independent(
                NetworkRequest::source_independent(request.url(), resource_type, method),
            ),
            NetworkAttribution::Unavailable => {
                self.counters.record_attribution_unavailable();
                return CoreDecision::Allow;
            }
        };
        match decision {
            Ok(decision) if decision.action() == NetworkAction::Block => CoreDecision::Block,
            Ok(_) => CoreDecision::Allow,
            Err(error) => {
                self.counters.record_error(error);
                CoreDecision::Allow
            }
        }
    }

    fn diagnostics(&self) -> NetworkPolicyDiagnostics {
        self.counters.snapshot()
    }
}

const fn map_resource_type(resource: NetworkResourceType) -> ResourceType {
    match resource {
        NetworkResourceType::Document => ResourceType::Document,
        NetworkResourceType::Subdocument => ResourceType::Subdocument,
        NetworkResourceType::Stylesheet => ResourceType::Stylesheet,
        NetworkResourceType::Image => ResourceType::Image,
        NetworkResourceType::Media => ResourceType::Media,
        NetworkResourceType::Font => ResourceType::Font,
        NetworkResourceType::Script => ResourceType::Script,
        NetworkResourceType::XmlHttpRequest => ResourceType::XmlHttpRequest,
        NetworkResourceType::Fetch => ResourceType::Fetch,
        NetworkResourceType::WebSocket => ResourceType::WebSocket,
        NetworkResourceType::Ping => ResourceType::Beacon,
        NetworkResourceType::Other => ResourceType::Other,
    }
}

fn map_method(method: &str) -> RequestMethod {
    if method.eq_ignore_ascii_case("GET") {
        RequestMethod::Get
    } else if method.eq_ignore_ascii_case("HEAD") {
        RequestMethod::Head
    } else if method.eq_ignore_ascii_case("POST") {
        RequestMethod::Post
    } else if method.eq_ignore_ascii_case("PUT") {
        RequestMethod::Put
    } else if method.eq_ignore_ascii_case("DELETE") {
        RequestMethod::Delete
    } else if method.eq_ignore_ascii_case("OPTIONS") {
        RequestMethod::Options
    } else if method.eq_ignore_ascii_case("PATCH") {
        RequestMethod::Patch
    } else if method.eq_ignore_ascii_case("CONNECT") {
        RequestMethod::Connect
    } else {
        RequestMethod::Other
    }
}

fn allow_all_digest() -> ContentRuleDigest {
    ContentRuleDigest::from_bytes(Sha256::digest(ALLOW_ALL_DIGEST_DOMAIN))
}

fn map_compile_error(error: &CompileError) -> BlockerCompileFailure {
    match error {
        CompileError::Cosmetics(crate::CosmeticError::ResourceLimit) => {
            BlockerCompileFailure::ResourceLimit
        }
        CompileError::Cosmetics(_) => BlockerCompileFailure::InvalidSource,
        CompileError::TargetUnavailable => BlockerCompileFailure::Internal,
        CompileError::NoSources => BlockerCompileFailure::SourceUnavailable,
        CompileError::NoUsableRules
        | CompileError::NoNativeBlockingRules
        | CompileError::DuplicateSource { .. }
        | CompileError::UpstreamInvariant
        | CompileError::AdmissionInvariant => BlockerCompileFailure::InvalidSource,
        CompileError::TooManySources { .. }
        | CompileError::SourceTooLarge { .. }
        | CompileError::TotalSourceBytesOverflow
        | CompileError::TotalSourceBytesExceeded { .. }
        | CompileError::LineTooLong { .. }
        | CompileError::RuleCountOverflow
        | CompileError::TooManyRules { .. }
        | CompileError::TooManyPhysicalLines { .. }
        | CompileError::TooManyWebKitRules { .. }
        | CompileError::WebKitJsonTooLarge { .. } => BlockerCompileFailure::ResourceLimit,
        #[cfg(feature = "runtime")]
        CompileError::RuntimeRegexBudgetExceeded
        | CompileError::RuntimePatternBytesOverflow
        | CompileError::RuntimePatternBytesBudgetExceeded
        | CompileError::RuntimeRegexBucketBudgetExceeded
        | CompileError::RuntimeRegexBucketBytesBudgetExceeded => {
            BlockerCompileFailure::ResourceLimit
        }
        #[cfg(feature = "runtime")]
        CompileError::RuntimeRegexCompilationFailed => BlockerCompileFailure::InvalidSource,
        #[cfg(feature = "runtime")]
        CompileError::RuntimePreparationInvariant => BlockerCompileFailure::Internal,
        #[cfg(feature = "webkit")]
        CompileError::SerializeWebKit(_) => BlockerCompileFailure::Internal,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(all(feature = "qa-test-hosts", feature = "runtime-exact"))]
    #[test]
    fn qa_builds_refuse_the_badware_test_host() {
        use zephium_core::blocker::{
            NetworkDecision, NetworkRequest as CoreRequest, NetworkRequestSourceKind,
            NetworkResourceType,
        };
        let catalog = StaticPolicyCatalog::new(vec![PolicySource::new(
            SourceId::new("easylist").unwrap(),
            SourceFormat::Standard,
            Arc::from("||ads.example^"),
        )])
        .unwrap();
        let navigation = RuntimePolicy::compiled(
            compile_navigation(&catalog, Compiler::default()).expect("the QA host"),
        );
        let url = "https://blocked-badware.test/";
        let request = CoreRequest::exact(
            url,
            url,
            "GET",
            NetworkResourceType::Document,
            NetworkRequestSourceKind::Document,
        )
        .unwrap();
        assert_eq!(navigation.decide(&request), NetworkDecision::Block);
    }

    #[cfg(all(
        feature = "runtime-exact",
        feature = "webkit",
        not(target_os = "windows")
    ))]
    #[test]
    fn webkit_rules_carry_a_badware_navigation_check() {
        use zephium_core::blocker::{
            NetworkDecision, NetworkRequest as CoreRequest, NetworkRequestSourceKind,
            NetworkResourceType,
        };
        let source = |id: &str, rules: &str| {
            PolicySource::new(
                SourceId::new(id).unwrap(),
                SourceFormat::Standard,
                Arc::from(rules),
            )
        };
        let catalog = |sources| StaticPolicyCatalog::new(sources).unwrap();
        let compile = |catalog: &StaticPolicyCatalog| {
            let compiler = Compiler::default();
            let compiled = compiler
                .compile(CompileTarget::WebKit, catalog.instantiate())
                .unwrap();
            with_navigation(
                adapt_rules(compiled).unwrap(),
                compile_navigation(catalog, compiler),
            )
        };
        let page = |url| {
            CoreRequest::exact(
                url,
                url,
                "GET",
                NetworkResourceType::Document,
                NetworkRequestSourceKind::Document,
            )
            .unwrap()
        };
        let with_badware = compile(&catalog(vec![
            source("easylist", "||ads.example^"),
            source(
                "ublock-badware",
                "||scam.example^$doc\n||cdn.example/*/alarm.mp3",
            ),
        ]));
        let navigation = with_badware
            .navigation_policy()
            .expect("a navigation check");
        assert_eq!(
            navigation.decide(&page("https://scam.example/")),
            NetworkDecision::Block
        );
        // Only the badware list judges navigations.
        assert_eq!(
            navigation.decide(&page("https://ads.example/")),
            NetworkDecision::Allow
        );
        let without = compile(&catalog(vec![source("easylist", "||ads.example^")]));
        // Protection QA builds always carry their test host.
        assert_eq!(
            without.navigation_policy().is_some(),
            cfg!(feature = "qa-test-hosts")
        );
    }

    fn compiler_with_post_ack_exit_gate() -> (Arc<WorkerBlocker>, SyncSender<()>, Receiver<()>) {
        let (queue, receiver) = mpsc::sync_channel::<WorkerCommand>(1);
        let (done_tx, done_rx) = mpsc::sync_channel(1);
        let (release_tx, release_rx) = mpsc::sync_channel(1);
        let (exited_tx, exited_rx) = mpsc::sync_channel(1);
        let worker = thread::spawn(move || {
            while receiver.recv().is_ok() {}
            let _ = done_tx.send(());
            release_rx.recv().unwrap();
            exited_tx.send(()).unwrap();
        });
        (
            Arc::new(WorkerBlocker {
                queue: Mutex::new(Some(queue)),
                admission: Arc::new(SharedAdmission {
                    state: Mutex::new(AdmissionState::default()),
                }),
                worker: Mutex::new(Some(worker)),
                worker_done: Mutex::new(done_rx),
                shutdown_serial: Mutex::new(()),
                completed: AtomicBool::new(false),
            }),
            release_tx,
            exited_rx,
        )
    }

    #[test]
    fn worker_exit_requires_thread_termination_after_done_acknowledgement() {
        let (done_tx, done_rx) = mpsc::sync_channel(1);
        let (release_tx, release_rx) = mpsc::sync_channel(1);
        let worker = thread::spawn(move || {
            done_tx.send(()).unwrap();
            release_rx.recv().unwrap();
        });

        assert!(!worker_exit_proven_until(
            &done_rx,
            &worker,
            Instant::now() + Duration::from_millis(25),
        ));
        assert!(!worker.is_finished());
        release_tx.send(()).unwrap();
        worker.join().unwrap();
    }

    #[test]
    fn compiler_shutdown_detaches_a_worker_parked_after_acknowledgement() {
        let (worker, release, exited) = compiler_with_post_ack_exit_gate();

        assert_eq!(
            worker.shutdown_until(Instant::now() + Duration::from_millis(25)),
            BlockerShutdownOutcome::Unclean
        );
        assert_eq!(
            worker.shutdown_until(Instant::now() + Duration::from_secs(1)),
            BlockerShutdownOutcome::Unclean
        );
        release.send(()).unwrap();
        exited.recv_timeout(Duration::from_secs(1)).unwrap();
    }

    #[test]
    fn worker_exit_proof_joins_a_normally_finished_worker() {
        let (done_tx, done_rx) = mpsc::sync_channel(1);
        let worker = thread::spawn(move || {
            done_tx.send(()).unwrap();
        });

        assert!(worker_exit_proven_until(
            &done_rx,
            &worker,
            Instant::now() + Duration::from_secs(1),
        ));
        assert!(worker.is_finished());
        worker.join().unwrap();
    }

    #[test]
    fn expired_exit_deadline_never_waits_for_an_active_worker() {
        let (done_tx, done_rx) = mpsc::sync_channel(1);
        let (release_tx, release_rx) = mpsc::sync_channel(1);
        let worker = thread::spawn(move || {
            release_rx.recv().unwrap();
            done_tx.send(()).unwrap();
        });

        assert!(!worker_exit_proven_until(&done_rx, &worker, Instant::now(),));
        release_tx.send(()).unwrap();
        worker.join().unwrap();
    }

    #[test]
    fn expired_compiler_shutdown_deadline_returns_without_taking_worker_ownership() {
        let (worker, release, exited) = compiler_with_post_ack_exit_gate();

        assert_eq!(
            worker.shutdown_until(Instant::now()),
            BlockerShutdownOutcome::Unclean
        );
        release.send(()).unwrap();
        drop(worker);
        exited.recv_timeout(Duration::from_secs(1)).unwrap();
    }

    #[test]
    fn shutdown_serial_lock_acquisition_obeys_its_absolute_deadline() {
        let lock = Arc::new(Mutex::new(()));
        let held = lock.lock().unwrap();
        let waiter_lock = Arc::clone(&lock);
        let waiter = thread::spawn(move || {
            lock_until(&waiter_lock, Instant::now() + Duration::from_millis(25)).is_none()
        });

        assert!(waiter.join().unwrap());
        drop(held);
    }

    #[test]
    fn concurrent_and_repeated_shutdown_share_one_exact_exit_proof() {
        let worker = WorkerBlocker::start(StaticPolicyCatalog::empty()).unwrap();
        let barrier = Arc::new(std::sync::Barrier::new(3));
        let shutdowns = (0..2)
            .map(|_| {
                let worker = Arc::clone(&worker);
                let barrier = Arc::clone(&barrier);
                thread::spawn(move || {
                    barrier.wait();
                    worker.shutdown_until(Instant::now() + Duration::from_secs(2))
                })
            })
            .collect::<Vec<_>>();
        barrier.wait();
        for shutdown in shutdowns {
            assert_eq!(shutdown.join().unwrap(), BlockerShutdownOutcome::Clean);
        }
        assert_eq!(
            worker.shutdown_until(Instant::now()),
            BlockerShutdownOutcome::Clean
        );
    }

    fn profile() -> ProfileId {
        ProfileId::generate()
    }

    fn generation(value: u64) -> ContentPolicyGeneration {
        ContentPolicyGeneration::new(value).unwrap()
    }

    #[test]
    fn deferred_catalog_clones_serialize_and_memoize_one_successful_load() {
        let load_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let loader_count = load_count.clone();
        let catalog = PolicyCatalog::deferred([6; 32], move || {
            loader_count.fetch_add(1, Ordering::Relaxed);
            std::thread::sleep(Duration::from_millis(10));
            Ok(StaticPolicyCatalog::empty())
        });
        let threads = (0..8)
            .map(|_| {
                let catalog = catalog.clone();
                std::thread::spawn(move || catalog.load().unwrap())
            })
            .collect::<Vec<_>>();
        for thread in threads {
            assert!(thread.join().unwrap().is_empty());
        }
        assert_eq!(load_count.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn reloadable_deferred_catalog_does_not_retain_successful_source_material() {
        let load_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let loader_count = load_count.clone();
        let catalog = PolicyCatalog::deferred_reloadable([7; 32], move || {
            loader_count.fetch_add(1, Ordering::Relaxed);
            Ok(StaticPolicyCatalog::empty())
        });

        assert!(catalog.load().unwrap().is_empty());
        assert!(catalog.clone().load().unwrap().is_empty());
        assert_eq!(load_count.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn disabled_profiles_share_explicit_allow_all() {
        let worker = WorkerBlocker::start(StaticPolicyCatalog::empty()).unwrap();
        let (tx, rx) = mpsc::sync_channel(2);
        for value in 1..=2 {
            let tx = tx.clone();
            assert_eq!(
                worker.compile(
                    profile(),
                    generation(value),
                    BlockerConfig { enabled: false },
                    Box::new(move |outcome| tx.send(outcome).unwrap()),
                ),
                BlockerDispatch::Scheduled
            );
        }
        let BlockerCompileOutcome::Compiled(first) = rx.recv().unwrap() else {
            panic!("explicit allow-all did not compile")
        };
        let BlockerCompileOutcome::Compiled(second) = rx.recv().unwrap() else {
            panic!("explicit allow-all did not compile")
        };
        assert!(Arc::ptr_eq(&first, &second));
        assert!(!first.enabled());
        assert_eq!(
            worker.shutdown_until(Instant::now() + Duration::from_secs(2)),
            BlockerShutdownOutcome::Clean
        );
    }

    #[test]
    fn panicking_completion_does_not_kill_worker_or_shutdown() {
        let worker = WorkerBlocker::start(StaticPolicyCatalog::empty()).unwrap();
        assert_eq!(
            worker.compile(
                profile(),
                generation(1),
                BlockerConfig { enabled: false },
                Box::new(|_| panic!("test completion panic")),
            ),
            BlockerDispatch::Scheduled
        );

        let (tx, rx) = mpsc::sync_channel(1);
        assert_eq!(
            worker.compile(
                profile(),
                generation(2),
                BlockerConfig { enabled: false },
                Box::new(move |outcome| tx.send(outcome).unwrap()),
            ),
            BlockerDispatch::Scheduled
        );
        assert!(matches!(
            rx.recv_timeout(Duration::from_secs(2)).unwrap(),
            BlockerCompileOutcome::Compiled(_)
        ));
        assert_eq!(
            worker.shutdown_until(Instant::now() + Duration::from_secs(2)),
            BlockerShutdownOutcome::Clean
        );
    }

    #[test]
    fn enabled_empty_catalog_fails_instead_of_claiming_protection() {
        let worker = WorkerBlocker::start(StaticPolicyCatalog::empty()).unwrap();
        let (tx, rx) = mpsc::sync_channel(1);
        assert_eq!(
            worker.compile(
                profile(),
                generation(1),
                BlockerConfig { enabled: true },
                Box::new(move |outcome| tx.send(outcome).unwrap()),
            ),
            BlockerDispatch::Scheduled
        );
        assert!(matches!(
            rx.recv().unwrap(),
            BlockerCompileOutcome::Failed(BlockerCompileFailure::SourceUnavailable)
        ));
        assert_eq!(
            worker.shutdown_until(Instant::now() + Duration::from_secs(2)),
            BlockerShutdownOutcome::Clean
        );
    }

    #[test]
    fn catalog_replacement_is_ordered_after_prior_compile_completion() {
        let worker = WorkerBlocker::start(StaticPolicyCatalog::empty()).unwrap();
        let (order_tx, order_rx) = mpsc::sync_channel(2);
        let compile_tx = order_tx.clone();
        assert_eq!(
            worker.compile(
                profile(),
                generation(1),
                BlockerConfig { enabled: false },
                Box::new(move |_| compile_tx.send("compile").unwrap()),
            ),
            BlockerDispatch::Scheduled
        );
        assert_eq!(
            worker.replace_catalog(
                StaticPolicyCatalog::empty(),
                Box::new(move || order_tx.send("catalog").unwrap()),
            ),
            CatalogReplacementDispatch::Scheduled
        );
        assert_eq!(
            order_rx.recv_timeout(Duration::from_secs(2)).unwrap(),
            "compile"
        );
        assert_eq!(
            order_rx.recv_timeout(Duration::from_secs(2)).unwrap(),
            "catalog"
        );
        assert_eq!(
            worker.shutdown_until(Instant::now() + Duration::from_secs(2)),
            BlockerShutdownOutcome::Clean
        );
    }

    #[test]
    fn only_one_catalog_replacement_can_own_the_reserved_control_slot() {
        let worker = WorkerBlocker::start(StaticPolicyCatalog::empty()).unwrap();
        let (entered_tx, entered_rx) = mpsc::sync_channel(1);
        let (release_tx, release_rx) = mpsc::sync_channel(1);
        assert_eq!(
            worker.compile(
                profile(),
                generation(1),
                BlockerConfig { enabled: false },
                Box::new(move |_| {
                    entered_tx.send(()).unwrap();
                    release_rx.recv().unwrap();
                }),
            ),
            BlockerDispatch::Scheduled
        );
        entered_rx.recv_timeout(Duration::from_secs(2)).unwrap();

        let (replaced_tx, replaced_rx) = mpsc::sync_channel(1);
        assert_eq!(
            worker.replace_catalog(
                StaticPolicyCatalog::empty(),
                Box::new(move || replaced_tx.send(()).unwrap()),
            ),
            CatalogReplacementDispatch::Scheduled
        );
        assert_eq!(
            worker.replace_catalog(StaticPolicyCatalog::empty(), Box::new(|| {})),
            CatalogReplacementDispatch::Rejected
        );
        release_tx.send(()).unwrap();
        replaced_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        assert_eq!(
            worker.shutdown_until(Instant::now() + Duration::from_secs(2)),
            BlockerShutdownOutcome::Clean
        );
    }

    #[cfg(any(
        all(target_os = "windows", feature = "runtime"),
        all(not(target_os = "windows"), feature = "webkit")
    ))]
    #[test]
    fn replacement_invalidates_the_old_artifact_cache_before_completion() {
        let worker = WorkerBlocker::start(StaticPolicyCatalog::empty()).unwrap();
        let (first_tx, first_rx) = mpsc::sync_channel(1);
        assert_eq!(
            worker.compile(
                profile(),
                generation(1),
                BlockerConfig { enabled: true },
                Box::new(move |outcome| first_tx.send(outcome).unwrap()),
            ),
            BlockerDispatch::Scheduled
        );
        assert!(matches!(
            first_rx.recv_timeout(Duration::from_secs(2)).unwrap(),
            BlockerCompileOutcome::Failed(BlockerCompileFailure::SourceUnavailable)
        ));

        let catalog = StaticPolicyCatalog::new(vec![PolicySource::new(
            SourceId::new("maintained").unwrap(),
            SourceFormat::Standard,
            Arc::from("||ads.zephium.invalid^$script"),
        )])
        .unwrap();
        let (replaced_tx, replaced_rx) = mpsc::sync_channel(1);
        assert_eq!(
            worker.replace_catalog(catalog, Box::new(move || replaced_tx.send(()).unwrap())),
            CatalogReplacementDispatch::Scheduled
        );
        replaced_rx.recv_timeout(Duration::from_secs(2)).unwrap();

        let (second_tx, second_rx) = mpsc::sync_channel(1);
        assert_eq!(
            worker.compile(
                profile(),
                generation(2),
                BlockerConfig { enabled: true },
                Box::new(move |outcome| second_tx.send(outcome).unwrap()),
            ),
            BlockerDispatch::Scheduled
        );
        assert!(matches!(
            second_rx.recv_timeout(Duration::from_secs(2)).unwrap(),
            BlockerCompileOutcome::Compiled(rules) if rules.enabled()
        ));
        assert_eq!(
            worker.shutdown_until(Instant::now() + Duration::from_secs(2)),
            BlockerShutdownOutcome::Clean
        );
    }

    #[test]
    fn retirement_barrier_suppresses_queued_work_then_reopens_the_identity() {
        let worker = WorkerBlocker::start(StaticPolicyCatalog::empty()).unwrap();
        let blocker = profile();
        let (entered_tx, entered_rx) = mpsc::sync_channel(1);
        let (release_tx, release_rx) = mpsc::sync_channel(1);
        assert_eq!(
            worker.compile(
                blocker,
                generation(1),
                BlockerConfig { enabled: false },
                Box::new(move |_| {
                    entered_tx.send(()).unwrap();
                    release_rx.recv().unwrap();
                }),
            ),
            BlockerDispatch::Scheduled
        );
        entered_rx.recv_timeout(Duration::from_secs(2)).unwrap();

        let retired = profile();
        let (stale_tx, stale_rx) = mpsc::sync_channel(1);
        assert_eq!(
            worker.compile(
                retired,
                generation(2),
                BlockerConfig { enabled: false },
                Box::new(move |_| stale_tx.send(()).unwrap()),
            ),
            BlockerDispatch::Scheduled
        );
        let (retired_tx, retired_rx) = mpsc::sync_channel(1);
        assert_eq!(
            worker.retire_profile(retired, Box::new(move || retired_tx.send(()).unwrap()),),
            BlockerRetirementDispatch::Scheduled
        );
        assert_eq!(
            worker.retire_profile(retired, Box::new(|| {})),
            BlockerRetirementDispatch::AlreadyScheduled
        );
        assert_eq!(
            worker.compile(
                retired,
                generation(3),
                BlockerConfig { enabled: false },
                Box::new(|_| {}),
            ),
            BlockerDispatch::Rejected
        );
        release_tx.send(()).unwrap();
        retired_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(matches!(
            stale_rx.recv_timeout(Duration::from_millis(50)),
            Err(mpsc::RecvTimeoutError::Disconnected)
        ));

        let (reused_tx, reused_rx) = mpsc::sync_channel(1);
        assert_eq!(
            worker.compile(
                retired,
                generation(4),
                BlockerConfig { enabled: false },
                Box::new(move |_| reused_tx.send(()).unwrap()),
            ),
            BlockerDispatch::Scheduled
        );
        reused_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        assert_eq!(
            worker.shutdown_until(Instant::now() + Duration::from_secs(2)),
            BlockerShutdownOutcome::Clean
        );
    }

    #[test]
    fn retirement_completion_waits_for_a_callback_already_in_progress() {
        let worker = WorkerBlocker::start(StaticPolicyCatalog::empty()).unwrap();
        let profile = profile();
        let (entered_tx, entered_rx) = mpsc::sync_channel(1);
        let (release_tx, release_rx) = mpsc::sync_channel(1);
        assert_eq!(
            worker.compile(
                profile,
                generation(1),
                BlockerConfig { enabled: false },
                Box::new(move |_| {
                    entered_tx.send(()).unwrap();
                    release_rx.recv().unwrap();
                }),
            ),
            BlockerDispatch::Scheduled
        );
        entered_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        assert_eq!(
            worker.compile(
                profile,
                generation(2),
                BlockerConfig { enabled: false },
                Box::new(|_| {}),
            ),
            BlockerDispatch::Rejected
        );

        let (retired_tx, retired_rx) = mpsc::sync_channel(1);
        assert_eq!(
            worker.retire_profile(profile, Box::new(move || retired_tx.send(()).unwrap()),),
            BlockerRetirementDispatch::Scheduled
        );
        assert_eq!(
            retired_rx.recv_timeout(Duration::from_millis(50)),
            Err(mpsc::RecvTimeoutError::Timeout)
        );
        release_tx.send(()).unwrap();
        retired_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        assert_eq!(
            worker.shutdown_until(Instant::now() + Duration::from_secs(2)),
            BlockerShutdownOutcome::Clean
        );
    }

    #[test]
    fn sequential_profile_churn_does_not_accumulate_tombstones_or_terminalize() {
        let worker = WorkerBlocker::start(StaticPolicyCatalog::empty()).unwrap();
        let churn = zephium_core::session::MAX_SESSION_PROFILES * 2 + 17;
        for index in 0..churn {
            let profile = ProfileId::from(10_000 + index as u128);
            let (retired_tx, retired_rx) = mpsc::sync_channel(1);
            assert_eq!(
                worker.compile(
                    profile,
                    generation(index as u64 + 1),
                    BlockerConfig { enabled: false },
                    Box::new(|_| {}),
                ),
                BlockerDispatch::Scheduled
            );
            assert!(matches!(
                worker.retire_profile(profile, Box::new(move || retired_tx.send(()).unwrap()),),
                BlockerRetirementDispatch::Scheduled | BlockerRetirementDispatch::Quiesced
            ));
            retired_rx.recv_timeout(Duration::from_secs(2)).unwrap();
            assert!(worker.admission.state.lock().unwrap().profiles.is_empty());
        }

        let (tx, rx) = mpsc::sync_channel(1);
        assert_eq!(
            worker.compile(
                profile(),
                generation(churn as u64 + 1),
                BlockerConfig { enabled: false },
                Box::new(move |_| tx.send(()).unwrap()),
            ),
            BlockerDispatch::Scheduled
        );
        rx.recv_timeout(Duration::from_secs(2)).unwrap();
        assert_eq!(
            worker.shutdown_until(Instant::now() + Duration::from_secs(2)),
            BlockerShutdownOutcome::Clean
        );
    }

    #[test]
    fn profile_admission_is_bounded_even_when_the_control_queue_has_capacity() {
        let worker = WorkerBlocker::start(StaticPolicyCatalog::empty()).unwrap();
        let (entered_tx, entered_rx) = mpsc::sync_channel(1);
        let (release_tx, release_rx) = mpsc::sync_channel(1);
        assert_eq!(
            worker.compile(
                ProfileId::from(20_000),
                generation(1),
                BlockerConfig { enabled: false },
                Box::new(move |_| {
                    entered_tx.send(()).unwrap();
                    release_rx.recv().unwrap();
                }),
            ),
            BlockerDispatch::Scheduled
        );
        entered_rx.recv_timeout(Duration::from_secs(2)).unwrap();

        for index in 1..zephium_core::session::MAX_SESSION_PROFILES {
            assert_eq!(
                worker.compile(
                    ProfileId::from(20_000 + index as u128),
                    generation(index as u64 + 1),
                    BlockerConfig { enabled: false },
                    Box::new(|_| {}),
                ),
                BlockerDispatch::Scheduled
            );
        }
        assert_eq!(
            worker.compile(
                ProfileId::from(99_999),
                generation(50_000),
                BlockerConfig { enabled: false },
                Box::new(|_| {}),
            ),
            BlockerDispatch::Rejected
        );
        assert_eq!(
            worker.admission.state.lock().unwrap().profiles.len(),
            zephium_core::session::MAX_SESSION_PROFILES
        );

        release_tx.send(()).unwrap();
        assert_eq!(
            worker.shutdown_until(Instant::now() + Duration::from_secs(2)),
            BlockerShutdownOutcome::Clean
        );
    }

    #[test]
    fn clean_shutdown_makes_future_dispatch_explicitly_terminal() {
        let worker = WorkerBlocker::start(StaticPolicyCatalog::empty()).unwrap();
        assert_eq!(
            worker.shutdown_until(Instant::now() + Duration::from_secs(2)),
            BlockerShutdownOutcome::Clean
        );
        assert_eq!(
            worker.compile(
                profile(),
                generation(1),
                BlockerConfig { enabled: false },
                Box::new(|_| panic!("terminal dispatch cannot accept a callback")),
            ),
            BlockerDispatch::Terminal
        );
    }

    #[test]
    fn shutdown_drains_every_accepted_callback_before_becoming_terminal() {
        let worker = WorkerBlocker::start(StaticPolicyCatalog::empty()).unwrap();
        let (tx, rx) = mpsc::sync_channel(1);
        assert_eq!(
            worker.compile(
                profile(),
                generation(1),
                BlockerConfig { enabled: false },
                Box::new(move |outcome| tx.send(outcome).unwrap()),
            ),
            BlockerDispatch::Scheduled
        );
        assert_eq!(
            worker.shutdown_until(Instant::now() + Duration::from_secs(2)),
            BlockerShutdownOutcome::Clean
        );
        assert!(matches!(
            rx.recv_timeout(Duration::from_secs(1)).unwrap(),
            BlockerCompileOutcome::Compiled(_)
        ));
    }

    #[test]
    fn idle_worker_has_no_periodic_wakeup_path() {
        let source = include_str!("worker.rs");
        let worker_loop = source
            .split_once("fn run_worker(")
            .expect("worker loop disappeared")
            .1
            .split_once("fn complete_job(")
            .expect("worker loop boundary disappeared")
            .0;
        assert!(worker_loop.contains("receiver.recv()"));
        assert!(!worker_loop.contains("recv_timeout"));
        assert!(!worker_loop.contains("sleep"));
    }

    #[test]
    fn catalog_rejects_duplicate_ids_before_worker_cloning() {
        let source = |contents: &str| {
            PolicySource::new(
                SourceId::new("same").unwrap(),
                SourceFormat::Standard,
                Arc::from(contents),
            )
        };
        assert!(matches!(
            StaticPolicyCatalog::new(vec![source("a"), source("b")]),
            Err(CatalogError::DuplicateSource)
        ));
    }

    fn numbered_sources(count: usize) -> Vec<PolicySource> {
        (0..count)
            .map(|n| {
                PolicySource::new(
                    SourceId::new(format!("list{n}")).unwrap(),
                    SourceFormat::Standard,
                    Arc::from(format!("||ads{n}.zephium.invalid^")),
                )
            })
            .collect()
    }

    #[test]
    fn catalog_keeps_a_source_slot_for_zephium_s_own_rules() {
        let limit = crate::CompileLimits::default().max_sources();
        assert!(matches!(
            StaticPolicyCatalog::new(numbered_sources(limit)),
            Err(CatalogError::TooManySources)
        ));
        StaticPolicyCatalog::new(numbered_sources(limit - 1)).unwrap();
    }

    #[cfg(any(
        all(target_os = "windows", feature = "runtime"),
        all(not(target_os = "windows"), feature = "webkit")
    ))]
    #[test]
    fn a_catalog_one_below_the_source_limit_compiles_with_zephium_s_own_rules() {
        let limit = crate::CompileLimits::default().max_sources();
        let catalog = StaticPolicyCatalog::new(numbered_sources(limit - 1)).unwrap();
        let mut cache = ArtifactCache::new(PolicyCatalog::eager(catalog));
        let mut persistent = None;
        assert!(matches!(
            cache.resolve(Compiler::default(), &mut persistent),
            BlockerCompileOutcome::Compiled(_)
        ));
    }

    #[test]
    fn catalog_keeps_room_for_zephium_s_own_rule_bytes() {
        let limits = crate::CompileLimits::default();
        let source = |id: &str, len: usize| {
            PolicySource::new(
                SourceId::new(id).unwrap(),
                SourceFormat::Standard,
                Arc::from("!".repeat(len)),
            )
        };
        let half = limits.max_total_source_bytes() / 2;
        assert!(half <= limits.max_source_bytes());
        assert!(matches!(
            StaticPolicyCatalog::new(vec![source("a", half), source("b", half)]),
            Err(CatalogError::TotalBytesExceeded)
        ));
        StaticPolicyCatalog::new(vec![
            source("a", half),
            source("b", half - crate::builtin::byte_len()),
        ])
        .unwrap();
    }

    #[cfg(feature = "runtime-exact")]
    #[test]
    fn the_exact_runtime_target_applies_zephium_s_own_site_exceptions_per_page() {
        let rules = Compiler::default()
            .compile(
                CompileTarget::Runtime,
                with_own_rules(
                    vec![FilterSource::new(
                        SourceId::new("list").unwrap(),
                        SourceFormat::Standard,
                        "||pagead2.googlesyndication.com^".to_owned(),
                    )],
                    CompileTarget::Runtime,
                ),
            )
            .unwrap();
        let on = |page| {
            rules
                .evaluate(NetworkRequest::new(
                    "https://pagead2.googlesyndication.com/pagead/js/adsbygoogle.js",
                    page,
                    ResourceType::Script,
                    RequestMethod::Get,
                ))
                .unwrap()
                .action()
        };
        assert_eq!(
            on("https://www.allrecipes.com/recipe/1/"),
            NetworkAction::Allow
        );
        assert_eq!(on("https://news.example/story"), NetworkAction::Block);
    }

    #[cfg(all(feature = "runtime", not(feature = "runtime-exact")))]
    #[test]
    fn the_runtime_target_leaves_out_zephium_s_own_site_exceptions() {
        let compile = |sources| {
            Compiler::default()
                .compile(CompileTarget::Runtime, sources)
                .unwrap()
        };
        let blocklist = || {
            FilterSource::new(
                SourceId::new("list").unwrap(),
                SourceFormat::Standard,
                "||pagead2.googlesyndication.com^".to_owned(),
            )
        };
        let loader = NetworkRequest::source_independent(
            "https://pagead2.googlesyndication.com/pagead/js/adsbygoogle.js",
            ResourceType::Script,
            RequestMethod::Get,
        );
        let action =
            |rules: &CompiledRules| rules.evaluate_source_independent(loader).unwrap().action();
        let rules = compile(with_own_rules(vec![blocklist()], CompileTarget::Runtime));
        assert_eq!(action(&rules), NetworkAction::Block);
        // Without the page, a site-scoped exception would allow the loader on
        // every site.
        let scoped = compile(vec![blocklist(), crate::builtin::source()]);
        assert_ne!(action(&scoped), NetworkAction::Block);
    }

    #[test]
    fn catalog_keeps_room_for_zephium_s_own_lines_and_rules() {
        let limits = crate::CompileLimits::default();
        let (own_lines, own_rules) = crate::builtin::line_counts();
        let source = |lines: Vec<&str>| {
            vec![PolicySource::new(
                SourceId::new("list").unwrap(),
                SourceFormat::Standard,
                Arc::from(lines.join("\n")),
            )]
        };
        let comments = |count: usize| source(vec!["!"; count]);
        assert!(matches!(
            StaticPolicyCatalog::new(comments(limits.max_physical_lines() - own_lines + 1)),
            Err(CatalogError::TooManyLines)
        ));
        StaticPolicyCatalog::new(comments(limits.max_physical_lines() - own_lines)).unwrap();
        let rules = |count: usize| source(vec!["||ads.zephium.invalid^"; count]);
        assert!(matches!(
            StaticPolicyCatalog::new(rules(limits.max_rules() - own_rules + 1)),
            Err(CatalogError::TooManyRules)
        ));
        StaticPolicyCatalog::new(rules(limits.max_rules() - own_rules)).unwrap();
    }

    #[test]
    fn catalog_refuses_the_id_of_zephium_s_own_rules() {
        let source = PolicySource::new(
            SourceId::new(crate::builtin::ID).unwrap(),
            SourceFormat::Standard,
            Arc::from("||ads.zephium.invalid^"),
        );
        assert!(matches!(
            StaticPolicyCatalog::new(vec![source]),
            Err(CatalogError::ReservedSource)
        ));
    }

    #[cfg(feature = "runtime")]
    #[test]
    fn runtime_artifact_preserves_typed_native_coverage_dimensions() {
        let rules = Compiler::default()
            .compile(
                CompileTarget::Runtime,
                vec![FilterSource::new(
                    SourceId::new("test").unwrap(),
                    SourceFormat::Standard,
                    concat!(
                        "||script.example^$script\n",
                        "||image.example^$image\n",
                        "||generic.example^\n",
                    )
                    .to_owned(),
                )],
            )
            .unwrap();
        let artifact = adapt_rules(rules).unwrap();
        let coverage = artifact.coverage();

        assert_eq!(coverage.platform_omitted_rules, 0);
        assert_eq!(coverage.platform_approximated_rules, 3);
        assert_eq!(coverage.platform_resource_approximated_rules, 1);
        assert_eq!(coverage.platform_source_kind_approximated_rules, 3);
        assert_eq!(coverage.platform_attribution_approximated_rules, 0);
    }

    #[cfg(feature = "webkit")]
    #[test]
    fn webkit_artifact_preserves_typed_coverage_dimensions() {
        let rules = Compiler::default()
            .compile(
                CompileTarget::WebKit,
                vec![FilterSource::new(
                    SourceId::new("test").unwrap(),
                    SourceFormat::Standard,
                    concat!(
                        "||tracker.example^$script,domain=publisher.example\n",
                        "||other.example^$other\n",
                        "||object.example^$object,domain=publisher.example\n",
                    )
                    .to_owned(),
                )],
            )
            .unwrap();
        let artifact = adapt_rules(rules).unwrap();
        let coverage = artifact.coverage();

        assert_eq!(coverage.platform_omitted_rules, 0);
        assert_eq!(coverage.platform_approximated_rules, 3);
        assert_eq!(coverage.platform_resource_approximated_rules, 2);
        assert_eq!(coverage.platform_source_kind_approximated_rules, 0);
        assert_eq!(coverage.platform_attribution_approximated_rules, 2);
    }

    #[cfg(feature = "runtime")]
    #[test]
    fn runtime_policy_preserves_source_independent_fail_open_semantics() {
        let rules = Compiler::default()
            .compile(
                CompileTarget::Runtime,
                vec![FilterSource::new(
                    SourceId::new("test").unwrap(),
                    SourceFormat::Standard,
                    concat!(
                        "||tracker.example^\n",
                        "@@||tracker.example^$domain=publisher.example\n",
                        "||generic.example^\n",
                    )
                    .to_owned(),
                )],
            )
            .unwrap();
        let policy = RuntimePolicy::compiled(rules);
        let source_kind = zephium_core::blocker::NetworkRequestSourceKind::Document;

        let potentially_excepted = CoreRequest::source_independent(
            "https://tracker.example/ad.js",
            "GET",
            NetworkResourceType::Script,
            source_kind,
        )
        .unwrap();
        assert_eq!(policy.decide(&potentially_excepted), CoreDecision::Allow);

        let generic = CoreRequest::source_independent(
            "https://generic.example/ad.js",
            "GET",
            NetworkResourceType::Script,
            source_kind,
        )
        .unwrap();
        assert_eq!(policy.decide(&generic), CoreDecision::Block);

        let unavailable = CoreRequest::unavailable(
            "https://generic.example/ad.js",
            "GET",
            NetworkResourceType::Script,
            source_kind,
        )
        .unwrap();
        assert_eq!(policy.decide(&unavailable), CoreDecision::Allow);
        assert_eq!(
            policy.diagnostics(),
            NetworkPolicyDiagnostics {
                total_decisions: 3,
                attribution_unavailable: 1,
                ..NetworkPolicyDiagnostics::default()
            }
        );
    }

    #[test]
    fn runtime_diagnostics_classify_fail_open_causes_without_wrapping() {
        let counters = RuntimePolicyCounters::default();
        for error in [
            crate::MatchError::CandidateBudgetExhausted,
            crate::MatchError::MatcherUnavailable,
            crate::MatchError::MatcherUnprepared,
            crate::MatchError::InvalidRequest,
        ] {
            counters.record_total();
            counters.record_error(error);
        }
        counters.record_total();
        counters.record_attribution_unavailable();

        assert_eq!(
            counters.snapshot(),
            NetworkPolicyDiagnostics {
                total_decisions: 5,
                candidate_budget_exhausted: 1,
                matcher_unavailable: 1,
                matcher_unprepared: 1,
                attribution_unavailable: 1,
                evaluation_errors: 1,
            }
        );
        counters.total_decisions.store(u64::MAX, Ordering::SeqCst);
        counters.record_total();
        assert_eq!(
            counters.snapshot().total_decisions,
            u64::MAX,
            "a long-lived matcher diagnostic must saturate instead of wrapping"
        );
    }

    #[cfg(feature = "runtime-exact")]
    #[test]
    fn optional_runtime_exact_policy_honors_source_domains() {
        let rules = Compiler::default()
            .compile(
                CompileTarget::Runtime,
                vec![FilterSource::new(
                    SourceId::new("test").unwrap(),
                    SourceFormat::Standard,
                    concat!(
                        "||baseline.example^\n",
                        "||tracker.example^$script,domain=publisher.example\n",
                    )
                    .to_owned(),
                )],
            )
            .unwrap();
        let policy = RuntimePolicy::compiled(rules);
        let source_kind = zephium_core::blocker::NetworkRequestSourceKind::Document;
        let request = |source_url| {
            CoreRequest::exact(
                "https://tracker.example/ad.js",
                source_url,
                "GET",
                NetworkResourceType::Script,
                source_kind,
            )
            .unwrap()
        };

        assert_eq!(
            policy.decide(&request("https://publisher.example/")),
            CoreDecision::Block
        );
        assert_eq!(
            policy.decide(&request("https://unrelated.example/")),
            CoreDecision::Allow
        );
    }

    #[cfg(feature = "runtime-exact")]
    #[test]
    fn top_level_attribution_applies_domain_party_and_scoped_exception_rules() {
        let rules = Compiler::default()
            .compile(
                CompileTarget::Runtime,
                vec![FilterSource::new(
                    SourceId::new("test").unwrap(),
                    SourceFormat::Standard,
                    concat!(
                        "||tracker.example^$script,domain=publisher.example
",
                        "||cdn.example^$third-party
",
                        "||ads.example/loader.js
",
                        "@@||ads.example/loader.js$domain=friendly.example
",
                    )
                    .to_owned(),
                )],
            )
            .unwrap();
        let policy = RuntimePolicy::compiled(rules);
        let decide = |url, top| {
            policy.decide(
                &CoreRequest::top_level_approximation(
                    url,
                    top,
                    "GET",
                    NetworkResourceType::Script,
                    zephium_core::blocker::NetworkRequestSourceKind::Document,
                )
                .unwrap(),
            )
        };
        let unattributed = |url| {
            policy.decide(
                &CoreRequest::source_independent(
                    url,
                    "GET",
                    NetworkResourceType::Script,
                    zephium_core::blocker::NetworkRequestSourceKind::Document,
                )
                .unwrap(),
            )
        };

        let tracker = "https://tracker.example/t.js";
        assert_eq!(
            decide(tracker, "https://publisher.example/a"),
            CoreDecision::Block
        );
        assert_eq!(
            decide(tracker, "https://unrelated.example/"),
            CoreDecision::Allow
        );
        assert_eq!(unattributed(tracker), CoreDecision::Allow);

        let cdn = "https://cdn.example/lib.js";
        assert_eq!(
            decide(cdn, "https://publisher.example/"),
            CoreDecision::Block
        );
        assert_eq!(decide(cdn, "https://www.cdn.example/"), CoreDecision::Allow);
        assert_eq!(unattributed(cdn), CoreDecision::Allow);

        // Unattributed, a block that one site excepts fails open everywhere.
        let loader = "https://ads.example/loader.js";
        assert_eq!(
            decide(loader, "https://publisher.example/"),
            CoreDecision::Block
        );
        assert_eq!(
            decide(loader, "https://friendly.example/"),
            CoreDecision::Allow
        );
        assert_eq!(unattributed(loader), CoreDecision::Allow);
    }

    #[cfg(feature = "runtime-exact")]
    #[test]
    fn a_document_parsed_once_decides_like_its_url() {
        let rules = Compiler::default()
            .compile(
                CompileTarget::Runtime,
                vec![FilterSource::new(
                    SourceId::new("test").unwrap(),
                    SourceFormat::Standard,
                    [
                        "||cdn.example^$third-party",
                        "||widget.example^$~third-party",
                        "||tracker.example^$domain=publisher.example|b.co.uk",
                        "||ads.example/loader.js",
                        "@@||ads.example/loader.js$domain=friendly.example",
                        "||pages.github.io^$third-party",
                        "||xn--bcher-kva.example^$domain=xn--bcher-kva.example",
                    ]
                    .join("\n"),
                )],
            )
            .unwrap();
        let urls = [
            "https://cdn.example/lib.js",
            "https://static.cdn.example/lib.js",
            "https://widget.example/w.js",
            "https://tracker.example/t.js",
            "https://ads.example/loader.js",
            "https://a.pages.github.io/x.js",
            "https://xn--bcher-kva.example/i.js",
        ];
        let documents = [
            "https://publisher.example/a?b=c#d",
            "https://WWW.CDN.EXAMPLE:8443/",
            "https://widget.example/",
            "https://friendly.example/",
            "https://a.b.co.uk/",
            "https://a.pages.github.io/",
            "https://b.pages.github.io/",
            "https://bücher.example/",
            "http://127.0.0.1:8080/",
        ];
        for document in documents {
            let source = rules.source_document(document).unwrap();
            for url in urls {
                assert_eq!(
                    rules.evaluate_from(url, &source, ResourceType::Script, RequestMethod::Get),
                    rules.evaluate(NetworkRequest::new(
                        url,
                        document,
                        ResourceType::Script,
                        RequestMethod::Get,
                    )),
                    "{url} from {document}"
                );
            }
        }
    }

    #[cfg(feature = "runtime-exact")]
    #[test]
    fn a_source_document_too_long_or_unparsable_is_refused() {
        let rules = Compiler::default()
            .compile(
                CompileTarget::Runtime,
                vec![FilterSource::new(
                    SourceId::new("test").unwrap(),
                    SourceFormat::Standard,
                    "||ads.example^".to_owned(),
                )],
            )
            .unwrap();
        let limit = crate::CompileLimits::default().max_source_url_bytes();
        let long = format!("https://a.example/{}", "x".repeat(limit));
        assert!(matches!(
            rules.source_document(&long),
            Err(crate::MatchError::SourceUrlTooLong { .. })
        ));
        assert!(matches!(
            rules.source_document("not a url"),
            Err(crate::MatchError::InvalidRequest)
        ));
    }

    #[cfg(feature = "runtime-exact")]
    #[test]
    fn recent_documents_are_parsed_once_and_bounded() {
        let rules = Compiler::default()
            .compile(
                CompileTarget::Runtime,
                vec![FilterSource::new(
                    SourceId::new("test").unwrap(),
                    SourceFormat::Standard,
                    "||ads.example^".to_owned(),
                )],
            )
            .unwrap();
        let policy = RuntimePolicy::compiled(rules);
        let first = policy.source_document("https://a.example/").unwrap();
        assert!(Arc::ptr_eq(
            &first,
            &policy.source_document("https://a.example/").unwrap()
        ));
        for index in 0..RecentSources::CAPACITY {
            policy
                .source_document(&format!("https://{index}.example/"))
                .unwrap();
        }
        assert!(!Arc::ptr_eq(
            &first,
            &policy.source_document("https://a.example/").unwrap()
        ));
        let long = format!(
            "https://a.example/{}",
            "x".repeat(RecentSources::MAX_URL_BYTES)
        );
        let once = policy.source_document(&long).unwrap();
        assert!(!Arc::ptr_eq(&once, &policy.source_document(&long).unwrap()));
        assert_eq!(
            policy.sources.lock().unwrap().0.len(),
            RecentSources::CAPACITY
        );
    }

    #[cfg(all(feature = "runtime", not(feature = "runtime-exact")))]
    #[test]
    fn runtime_only_policy_keeps_top_level_requests_source_independent() {
        let rules = Compiler::default()
            .compile(
                CompileTarget::Runtime,
                vec![FilterSource::new(
                    SourceId::new("test").unwrap(),
                    SourceFormat::Standard,
                    concat!(
                        "||generic.example^
",
                        "||tracker.example^$script,domain=publisher.example
",
                    )
                    .to_owned(),
                )],
            )
            .unwrap();
        let policy = RuntimePolicy::compiled(rules);
        let decide = |url| {
            policy.decide(
                &CoreRequest::top_level_approximation(
                    url,
                    "https://publisher.example/",
                    "GET",
                    NetworkResourceType::Script,
                    zephium_core::blocker::NetworkRequestSourceKind::Document,
                )
                .unwrap(),
            )
        };
        assert_eq!(decide("https://generic.example/a.js"), CoreDecision::Block);
        assert_eq!(decide("https://tracker.example/t.js"), CoreDecision::Allow);
        assert_eq!(policy.diagnostics().attribution_unavailable, 0);
    }

    #[cfg(all(feature = "runtime", not(feature = "runtime-exact")))]
    #[test]
    fn shipped_runtime_only_policy_fails_open_on_unexpected_exact_attribution() {
        let rules = Compiler::default()
            .compile(
                CompileTarget::Runtime,
                vec![FilterSource::new(
                    SourceId::new("test").unwrap(),
                    SourceFormat::Standard,
                    "||tracker.example^\n".to_owned(),
                )],
            )
            .unwrap();
        let request = CoreRequest::exact(
            "https://tracker.example/ad.js",
            "https://publisher.example/",
            "GET",
            NetworkResourceType::Script,
            zephium_core::blocker::NetworkRequestSourceKind::Document,
        )
        .unwrap();

        assert_eq!(
            RuntimePolicy::compiled(rules).decide(&request),
            CoreDecision::Allow
        );
    }

    #[cfg(any(
        all(target_os = "windows", feature = "runtime"),
        all(not(target_os = "windows"), feature = "webkit")
    ))]
    #[test]
    // Protection QA builds add a rule of their own, so no catalog fails alone.
    #[cfg(not(feature = "qa-test-hosts"))]
    fn deterministic_compile_failure_is_cached_once() {
        let catalog = StaticPolicyCatalog::new(vec![PolicySource::new(
            SourceId::new("invalid").unwrap(),
            SourceFormat::Standard,
            Arc::from("example.com##.ad"),
        )])
        .unwrap();
        let mut cache = ArtifactCache::new(PolicyCatalog::eager(catalog));
        let mut persistent = None;
        for _ in 0..64 {
            assert!(matches!(
                cache.resolve(Compiler::default(), &mut persistent),
                BlockerCompileOutcome::Failed(BlockerCompileFailure::InvalidSource)
            ));
        }
        assert_eq!(cache.compilation_attempts, 1);
        assert!(cache.catalog.is_none());
    }

    #[cfg(any(
        all(target_os = "windows", feature = "runtime"),
        all(not(target_os = "windows"), feature = "webkit")
    ))]
    #[test]
    fn authenticated_source_repair_is_visible_to_same_process_catalog() {
        let root = tempfile::tempdir().unwrap();
        let source_path = root.path().join("current-source");
        let load_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let loader_count = load_count.clone();
        let loader_path = source_path.clone();
        let catalog = PolicyCatalog::deferred([7; 32], move || {
            loader_count.fetch_add(1, Ordering::Relaxed);
            let contents = std::fs::read_to_string(&loader_path)
                .map_err(|_| BlockerCompileFailure::SourceUnavailable)?;
            if contents != "||ads.zephium.invalid^$script" {
                return Err(BlockerCompileFailure::InvalidSource);
            }
            StaticPolicyCatalog::new(vec![PolicySource::new(
                SourceId::new("maintained").unwrap(),
                SourceFormat::Standard,
                Arc::from(contents),
            )])
            .map_err(|_| BlockerCompileFailure::InvalidSource)
        });
        let mut cache = ArtifactCache::new(catalog);
        let mut persistent = None;

        assert!(matches!(
            cache.resolve(Compiler::default(), &mut persistent),
            BlockerCompileOutcome::Failed(BlockerCompileFailure::SourceUnavailable)
        ));
        std::fs::write(&source_path, "corrupt authenticated source").unwrap();
        assert!(matches!(
            cache.resolve(Compiler::default(), &mut persistent),
            BlockerCompileOutcome::Failed(BlockerCompileFailure::InvalidSource)
        ));
        std::fs::write(&source_path, "||ads.zephium.invalid^$script").unwrap();
        assert!(matches!(
            cache.resolve(Compiler::default(), &mut persistent),
            BlockerCompileOutcome::Compiled(_)
        ));
        assert_eq!(load_count.load(Ordering::Relaxed), 3);
        assert!(cache.catalog.is_none());

        std::fs::remove_file(source_path).unwrap();
        assert!(matches!(
            cache.resolve(Compiler::default(), &mut persistent),
            BlockerCompileOutcome::Compiled(_)
        ));
        assert_eq!(load_count.load(Ordering::Relaxed), 3);
    }

    #[cfg(any(
        all(target_os = "windows", feature = "runtime"),
        all(not(target_os = "windows"), feature = "webkit")
    ))]
    #[test]
    fn persistent_cache_warm_hit_skips_source_parsing_and_retains_only_required_recovery() {
        let root = crate::cache::private_test_root();
        let config = CompiledArtifactCacheConfig::new(root.path()).unwrap();
        let catalog = StaticPolicyCatalog::new(vec![PolicySource::new(
            SourceId::new("maintained").unwrap(),
            SourceFormat::Standard,
            Arc::from("||ads.zephium.invalid^"),
        )])
        .unwrap();

        let mut cold = ArtifactCache::new(PolicyCatalog::eager(catalog.clone()));
        let mut cold_persistent = Some(PersistentArtifactCache::open(&config).unwrap());
        assert!(matches!(
            cold.resolve(Compiler::default(), &mut cold_persistent),
            BlockerCompileOutcome::Compiled(_)
        ));
        assert_eq!(cold.compilation_attempts, 1);
        assert_eq!(cold.persistent_hits, 0);
        if native_target() == CompileTarget::WebKit {
            assert!(cold.catalog.is_some());
            assert!(matches!(
                cold.artifact,
                Some(CachedArtifact::Declarative(_))
            ));
        } else {
            assert!(cold.catalog.is_none());
            assert!(matches!(cold.artifact, Some(CachedArtifact::Strong(_))));
        }
        drop(cold);
        drop(cold_persistent);

        let mut warm = ArtifactCache::new(PolicyCatalog::eager(catalog));
        let mut warm_persistent = Some(PersistentArtifactCache::open(&config).unwrap());
        assert!(matches!(
            warm.resolve(Compiler::default(), &mut warm_persistent),
            BlockerCompileOutcome::Compiled(_)
        ));
        assert_eq!(warm.compilation_attempts, 0);
        assert_eq!(warm.persistent_hits, 1);
        if native_target() == CompileTarget::WebKit {
            assert!(warm.catalog.is_some());
            assert!(matches!(
                warm.artifact,
                Some(CachedArtifact::Declarative(_))
            ));
        } else {
            assert!(warm.catalog.is_none());
            assert!(matches!(warm.artifact, Some(CachedArtifact::Strong(_))));
        }
    }

    #[cfg(all(unix, feature = "webkit"))]
    #[test]
    fn unsafe_persistent_cache_degrades_to_bounded_source_compilation() {
        use std::os::unix::fs::PermissionsExt;

        let root = tempfile::tempdir_in(std::env::current_dir().unwrap()).unwrap();
        std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
        let config = CompiledArtifactCacheConfig::new(root.path()).unwrap();
        let catalog = StaticPolicyCatalog::new(vec![PolicySource::new(
            SourceId::new("maintained").unwrap(),
            SourceFormat::Standard,
            Arc::from("||ads.zephium.invalid^$script"),
        )])
        .unwrap();

        let mut cache = ArtifactCache::new(PolicyCatalog::eager(catalog));
        let mut persistent = PersistentArtifactCache::open(&config).ok();
        assert!(persistent.is_none());
        assert!(matches!(
            cache.resolve(Compiler::default(), &mut persistent),
            BlockerCompileOutcome::Compiled(_)
        ));
        assert_eq!(cache.compilation_attempts, 1);
        assert_eq!(cache.persistent_hits, 0);
        assert!(cache.catalog.is_none());
        assert!(matches!(cache.artifact, Some(CachedArtifact::Strong(_))));
        assert!(!root.path().join("current.bin").exists());
    }

    #[cfg(all(not(target_os = "windows"), feature = "webkit"))]
    #[test]
    fn durable_declarative_artifact_lives_only_while_a_consumer_owns_it() {
        let root = tempfile::tempdir_in(std::env::current_dir().unwrap()).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        let config = CompiledArtifactCacheConfig::new(root.path()).unwrap();
        let catalog = StaticPolicyCatalog::new(vec![PolicySource::new(
            SourceId::new("maintained").unwrap(),
            SourceFormat::Standard,
            Arc::from("||ads.zephium.invalid^$script"),
        )])
        .unwrap();
        let mut cache = ArtifactCache::new(PolicyCatalog::eager(catalog));
        let mut persistent = PersistentArtifactCache::open(&config).ok();

        let first = match cache.resolve(Compiler::default(), &mut persistent) {
            BlockerCompileOutcome::Compiled(rules) => rules,
            BlockerCompileOutcome::Failed(failure) => {
                panic!("unexpected compilation failure: {failure:?}")
            }
        };
        let digest = first.digest();
        let released = Arc::downgrade(&first);
        assert_eq!(cache.compilation_attempts, 1);
        assert_eq!(cache.persistent_hits, 0);

        // A second profile in the same delivery/native-compilation cohort
        // upgrades the exact allocation without disk I/O or recompilation.
        let cohort = match cache.resolve(Compiler::default(), &mut persistent) {
            BlockerCompileOutcome::Compiled(rules) => rules,
            BlockerCompileOutcome::Failed(failure) => {
                panic!("unexpected cohort failure: {failure:?}")
            }
        };
        assert!(Arc::ptr_eq(&first, &cohort));
        assert_eq!(cache.compilation_attempts, 1);
        assert_eq!(cache.persistent_hits, 0);

        drop(first);
        drop(cohort);
        assert!(released.upgrade().is_none());
        assert!(matches!(
            cache.artifact,
            Some(CachedArtifact::Declarative(_))
        ));

        let recovered = match cache.resolve(Compiler::default(), &mut persistent) {
            BlockerCompileOutcome::Compiled(rules) => rules,
            BlockerCompileOutcome::Failed(failure) => {
                panic!("unexpected cache recovery failure: {failure:?}")
            }
        };
        assert_eq!(recovered.digest(), digest);
        assert_eq!(cache.compilation_attempts, 1);
        assert_eq!(cache.persistent_hits, 1);
    }

    #[cfg(all(not(target_os = "windows"), feature = "webkit"))]
    #[test]
    fn missing_persistent_cache_keeps_declarative_fallback_strong() {
        let catalog = StaticPolicyCatalog::new(vec![PolicySource::new(
            SourceId::new("maintained").unwrap(),
            SourceFormat::Standard,
            Arc::from("||ads.zephium.invalid^$script"),
        )])
        .unwrap();
        let mut cache = ArtifactCache::new(PolicyCatalog::eager(catalog));
        let mut persistent = None;

        let delivered = match cache.resolve(Compiler::default(), &mut persistent) {
            BlockerCompileOutcome::Compiled(rules) => rules,
            BlockerCompileOutcome::Failed(failure) => {
                panic!("unexpected compilation failure: {failure:?}")
            }
        };
        let retained = Arc::downgrade(&delivered);
        drop(delivered);

        assert!(retained.upgrade().is_some());
        assert!(cache.catalog.is_none());
        assert!(matches!(cache.artifact, Some(CachedArtifact::Strong(_))));
        assert!(matches!(
            cache.resolve(Compiler::default(), &mut persistent),
            BlockerCompileOutcome::Compiled(_)
        ));
        assert_eq!(cache.compilation_attempts, 1);
    }

    #[cfg(all(not(target_os = "windows"), feature = "webkit"))]
    #[test]
    fn persistent_store_failure_keeps_declarative_fallback_strong() {
        let root = tempfile::tempdir_in(std::env::current_dir().unwrap()).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        let config = CompiledArtifactCacheConfig::new(root.path()).unwrap();
        let catalog = StaticPolicyCatalog::new(vec![PolicySource::new(
            SourceId::new("maintained").unwrap(),
            SourceFormat::Standard,
            Arc::from("||ads.zephium.invalid^$script"),
        )])
        .unwrap();
        let mut cache = ArtifactCache::new(PolicyCatalog::eager(catalog));
        let mut persistent = PersistentArtifactCache::open(&config).ok();
        std::fs::create_dir(root.path().join("stage.bin")).unwrap();

        let delivered = match cache.resolve(Compiler::default(), &mut persistent) {
            BlockerCompileOutcome::Compiled(rules) => rules,
            BlockerCompileOutcome::Failed(failure) => {
                panic!("unexpected compilation failure: {failure:?}")
            }
        };
        let retained = Arc::downgrade(&delivered);
        drop(delivered);

        assert!(persistent.is_none());
        assert!(retained.upgrade().is_some());
        assert!(cache.catalog.is_none());
        assert!(matches!(cache.artifact, Some(CachedArtifact::Strong(_))));
        assert!(matches!(
            cache.resolve(Compiler::default(), &mut persistent),
            BlockerCompileOutcome::Compiled(_)
        ));
        assert_eq!(cache.compilation_attempts, 1);
    }

    #[cfg(all(not(target_os = "windows"), feature = "webkit"))]
    #[test]
    fn corrupt_durable_declarative_artifact_recompiles_from_retained_recipe() {
        let root = tempfile::tempdir_in(std::env::current_dir().unwrap()).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        let config = CompiledArtifactCacheConfig::new(root.path()).unwrap();
        let catalog = StaticPolicyCatalog::new(vec![PolicySource::new(
            SourceId::new("maintained").unwrap(),
            SourceFormat::Standard,
            Arc::from("||ads.zephium.invalid^$script"),
        )])
        .unwrap();
        let mut cache = ArtifactCache::new(PolicyCatalog::eager(catalog));
        let mut persistent = PersistentArtifactCache::open(&config).ok();

        let first = match cache.resolve(Compiler::default(), &mut persistent) {
            BlockerCompileOutcome::Compiled(rules) => rules,
            BlockerCompileOutcome::Failed(failure) => {
                panic!("unexpected compilation failure: {failure:?}")
            }
        };
        let digest = first.digest();
        drop(first);
        std::fs::write(root.path().join("current.bin"), b"corrupt").unwrap();

        let recovered = match cache.resolve(Compiler::default(), &mut persistent) {
            BlockerCompileOutcome::Compiled(rules) => rules,
            BlockerCompileOutcome::Failed(failure) => {
                panic!("unexpected recompilation failure: {failure:?}")
            }
        };
        assert_eq!(recovered.digest(), digest);
        assert_eq!(cache.compilation_attempts, 2);
        assert_eq!(cache.persistent_hits, 0);
        assert!(persistent.is_some());
        assert!(cache.catalog.is_some());
        assert!(matches!(
            cache.artifact,
            Some(CachedArtifact::Declarative(_))
        ));
    }

    #[cfg(all(not(target_os = "windows"), feature = "webkit"))]
    #[test]
    fn prepared_candidate_reloads_durable_artifact_after_activation() {
        let root = tempfile::tempdir_in(std::env::current_dir().unwrap()).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        let config = CompiledArtifactCacheConfig::new(root.path()).unwrap();
        let worker = WorkerBlocker::start_with_policy_catalog(
            PolicyCatalog::eager(StaticPolicyCatalog::empty()),
            config,
        )
        .unwrap();
        let source = StaticPolicyCatalog::new(vec![PolicySource::new(
            SourceId::new("candidate").unwrap(),
            SourceFormat::Standard,
            Arc::from("||ads.zephium.invalid^$script\n##.candidate-ad"),
        )])
        .unwrap();
        let loads = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let loader_loads = loads.clone();
        let candidate = PolicyCatalog::deferred_reloadable([11; 32], move || {
            if loader_loads.fetch_add(1, Ordering::Relaxed) == 0 {
                Ok(source.clone())
            } else {
                Err(BlockerCompileFailure::SourceUnavailable)
            }
        });

        let identity = [12; 32];
        let (prepared_tx, prepared_rx) = mpsc::sync_channel(1);
        assert_eq!(
            worker.prepare_catalog_rules(
                identity,
                candidate,
                Box::new(move |outcome| prepared_tx.send(outcome).unwrap()),
            ),
            CatalogReplacementDispatch::Scheduled
        );
        let preflight = prepared_rx
            .recv_timeout(Duration::from_secs(2))
            .unwrap()
            .unwrap();
        assert!(
            matches!(preflight.payload(), zephium_core::blocker::ContentRulesPayload::Declarative { encoded, .. } if !encoded.is_empty())
        );
        assert!(preflight.cosmetics().is_some());
        drop(preflight);

        assert_eq!(loads.load(Ordering::Relaxed), 1);

        let (stale_tx, stale_rx) = mpsc::sync_channel(1);
        assert_eq!(
            worker.activate_prepared_catalog(
                [13; 32],
                Box::new(move |activated| stale_tx.send(activated).unwrap()),
            ),
            CatalogReplacementDispatch::Scheduled
        );
        assert!(!stale_rx.recv_timeout(Duration::from_secs(2)).unwrap());

        let (activated_tx, activated_rx) = mpsc::sync_channel(1);
        assert_eq!(
            worker.activate_prepared_catalog(
                identity,
                Box::new(move |activated| activated_tx.send(activated).unwrap()),
            ),
            CatalogReplacementDispatch::Scheduled
        );
        assert!(activated_rx.recv_timeout(Duration::from_secs(2)).unwrap());

        let (compiled_tx, compiled_rx) = mpsc::sync_channel(1);
        assert_eq!(
            worker.compile(
                profile(),
                generation(1),
                BlockerConfig { enabled: true },
                Box::new(move |outcome| compiled_tx.send(outcome).unwrap()),
            ),
            BlockerDispatch::Scheduled
        );
        assert!(matches!(
            compiled_rx.recv_timeout(Duration::from_secs(2)).unwrap(),
            BlockerCompileOutcome::Compiled(_)
        ));
        // The candidate's sole in-memory artifact was released after prepare;
        // activation recovered the exact checksummed persistent copy rather
        // than consulting source material a second time.
        assert_eq!(loads.load(Ordering::Relaxed), 1);
        assert_eq!(
            worker.shutdown_until(Instant::now() + Duration::from_secs(2)),
            BlockerShutdownOutcome::Clean
        );
    }

    #[cfg(all(not(target_os = "windows"), feature = "webkit"))]
    #[test]
    fn activation_revalidates_and_preserves_a_temporarily_unrecoverable_candidate() {
        let root = tempfile::tempdir_in(std::env::current_dir().unwrap()).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        let config = CompiledArtifactCacheConfig::new(root.path()).unwrap();
        let worker = WorkerBlocker::start_with_policy_catalog(
            PolicyCatalog::eager(StaticPolicyCatalog::empty()),
            config,
        )
        .unwrap();
        let source = StaticPolicyCatalog::new(vec![PolicySource::new(
            SourceId::new("candidate").unwrap(),
            SourceFormat::Standard,
            Arc::from("||ads.zephium.invalid^$script"),
        )])
        .unwrap();
        let source_available = Arc::new(AtomicBool::new(true));
        let loader_available = source_available.clone();
        let candidate = PolicyCatalog::deferred_reloadable([14; 32], move || {
            if loader_available.load(Ordering::Acquire) {
                Ok(source.clone())
            } else {
                Err(BlockerCompileFailure::SourceUnavailable)
            }
        });
        let identity = [15; 32];
        let (prepared_tx, prepared_rx) = mpsc::sync_channel(1);
        assert_eq!(
            worker.prepare_catalog(
                identity,
                candidate,
                Box::new(move |outcome| prepared_tx.send(outcome).unwrap()),
            ),
            CatalogReplacementDispatch::Scheduled
        );
        assert_eq!(
            prepared_rx.recv_timeout(Duration::from_secs(2)).unwrap(),
            CatalogPreparationOutcome::Prepared
        );

        // Invalidate the only persistent copy after prepare and make the exact
        // source recipe transiently unavailable. Activation must fail without
        // displacing either active authority or the prepared candidate.
        std::fs::write(root.path().join("current.bin"), b"corrupt").unwrap();
        source_available.store(false, Ordering::Release);
        let (failed_tx, failed_rx) = mpsc::sync_channel(1);
        assert_eq!(
            worker.activate_prepared_catalog(
                identity,
                Box::new(move |activated| failed_tx.send(activated).unwrap()),
            ),
            CatalogReplacementDispatch::Scheduled
        );
        assert!(!failed_rx.recv_timeout(Duration::from_secs(2)).unwrap());

        source_available.store(true, Ordering::Release);
        let (activated_tx, activated_rx) = mpsc::sync_channel(1);
        assert_eq!(
            worker.activate_prepared_catalog(
                identity,
                Box::new(move |activated| activated_tx.send(activated).unwrap()),
            ),
            CatalogReplacementDispatch::Scheduled
        );
        assert!(activated_rx.recv_timeout(Duration::from_secs(2)).unwrap());

        let (compiled_tx, compiled_rx) = mpsc::sync_channel(1);
        assert_eq!(
            worker.compile(
                profile(),
                generation(1),
                BlockerConfig { enabled: true },
                Box::new(move |outcome| compiled_tx.send(outcome).unwrap()),
            ),
            BlockerDispatch::Scheduled
        );
        assert!(matches!(
            compiled_rx.recv_timeout(Duration::from_secs(2)).unwrap(),
            BlockerCompileOutcome::Compiled(_)
        ));
        assert_eq!(
            worker.shutdown_until(Instant::now() + Duration::from_secs(2)),
            BlockerShutdownOutcome::Clean
        );
    }

    #[cfg(feature = "runtime")]
    #[test]
    fn an_official_list_that_stops_blocking_is_refused() {
        let rules = Compiler::default()
            .compile(
                CompileTarget::Runtime,
                vec![FilterSource::new(
                    SourceId::new("easylist").unwrap(),
                    SourceFormat::Standard,
                    "||unrelated.example^".to_owned(),
                )],
            )
            .unwrap();
        assert_eq!(
            adapt_rules(rules).err(),
            Some(BlockerCompileFailure::InvalidSource)
        );
    }
}
