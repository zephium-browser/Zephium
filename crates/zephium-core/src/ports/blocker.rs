use std::sync::Arc;
use std::time::Instant;

use crate::blocker::{BlockerConfig, ContentPolicyGeneration, ContentRules};
use crate::ids::ProfileId;

pub use crate::blocker::BlockerCompileFailure;

#[must_use]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlockerDispatch {
    Scheduled,
    Rejected,
    /// Enabled source-policy admission is terminal for this process.
    ///
    /// Disabled allow-all compilation remains admissible; this must not be
    /// widened into global compiler terminality by consumers.
    EnabledPolicyTerminal,
    /// The compiler is process-terminal and retry cannot restore admission.
    Terminal,
}

/// Admission result for an exact profile-compilation retirement barrier.
#[must_use]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlockerRetirementDispatch {
    /// No compile callback was outstanding and `done` ran before return.
    Quiesced,
    /// The compiler owns `done` and will run it after every older compile for
    /// this profile has either returned from its callback or been suppressed.
    Scheduled,
    /// An earlier retirement barrier already owns the profile. `done` was not
    /// accepted; admission will reopen when that barrier finishes.
    AlreadyScheduled,
    /// The bounded control queue could not admit the barrier. `done` was not
    /// accepted and consumers must continue rejecting stale generations.
    Rejected,
    /// Compiler shutdown is authoritative. `done` was not accepted.
    Terminal,
}

#[derive(Clone, Debug)]
pub enum BlockerCompileOutcome {
    Compiled(Arc<ContentRules>),
    Failed(BlockerCompileFailure),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlockerShutdownOutcome {
    Clean,
    Unclean,
}

/// Stable reason the authenticated catalog service cannot run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlockerCatalogUnavailable {
    /// This build has no provisioned trust root, repository, or approved list
    /// license policy.
    NotConfigured,
    /// The platform cannot prove the durable activation contract.
    DurableActivationUnsupported,
    /// The private catalog/cache namespace could not be admitted safely.
    StorageUnavailable,
    /// The durable monotonic clock state is corrupt or contradicted by the
    /// current wall clock.
    ClockUnsafe,
}

/// Stable, non-page-derived category for the most recent refresh failure.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlockerCatalogFailure {
    Transport,
    Metadata,
    Clock,
    Manifest,
    Target,
    License,
    Rollback,
    Storage,
    Catalog,
    Internal,
}

/// Current state of the authenticated source package.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlockerCatalogPhase {
    Unavailable(BlockerCatalogUnavailable),
    Idle,
    Fresh,
    Stale,
    Refreshing,
    Failed(BlockerCatalogFailure),
    Shutdown,
}

/// Authority which admitted one immutable filter package.
///
/// Release-bundled material is authenticated by the signed application
/// artifact. Repository material is authenticated by the separately
/// provisioned TUF trust domain. Keeping the distinction at the policy port
/// prevents a bundled fallback from being mislabeled as an online update.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlockerCatalogProvenance {
    ReleaseBundle,
    TufRepository,
    /// Source bytes obtained from fixed official HTTPS publisher endpoints;
    /// this is transport/server trust, not publisher signatures.
    OfficialHttps,
}

/// Small, immutable catalog status consumed by the authoritative shell actor.
///
/// It contains package metadata only. Repository URLs, source names, rules,
/// native errors, and browsing decisions never cross this boundary.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BlockerCatalogSnapshot {
    /// Strictly increasing process-local service revision.
    pub revision: u64,
    pub phase: BlockerCatalogPhase,
    /// Enabled source-policy admission is permanently sealed for this process.
    ///
    /// This is monotonic and independent from transient updater failures.
    /// Disabled allow-all compilation remains available when this is true.
    pub enabled_policy_terminal: bool,
    /// Current authenticated package revision, including while stale, failed,
    /// or refreshing.
    pub package_revision: Option<u64>,
    /// SHA-256 of the exact canonical manifest for the current package.
    pub package_manifest_sha256: Option<[u8; 32]>,
    pub package_provenance: Option<BlockerCatalogProvenance>,
    pub package_created_unix: Option<u64>,
    pub package_expires_unix: Option<u64>,
    /// Whether the authenticated package authority itself is stale.
    ///
    /// This is meaningful for refreshable repository packages. A signed
    /// application release remains valid authority for its embedded package,
    /// independently from the upstream lists' recommended update cadence.
    pub package_stale: Option<bool>,
    /// The source publisher's recommended refresh time has elapsed.
    ///
    /// This is advisory for release-bundled packages and must not downgrade
    /// otherwise healthy protection. Repository mode can act on it through
    /// an authenticated refresh.
    pub source_refresh_due: bool,
    pub source_count: Option<u32>,
    pub source_bytes: Option<u64>,
    /// Authenticated package which has not yet become durable current
    /// authority. Candidate identity remains distinct so an initial package
    /// cannot be mistaken for current before its compiler preparation and
    /// durable commit barriers complete.
    pub candidate_revision: Option<u64>,
    pub candidate_manifest_sha256: Option<[u8; 32]>,
    pub candidate_provenance: Option<BlockerCatalogProvenance>,
    pub candidate_created_unix: Option<u64>,
    pub candidate_expires_unix: Option<u64>,
    pub candidate_source_count: Option<u32>,
    pub candidate_source_bytes: Option<u64>,
    /// Package revision already made authoritative inside the compiler.
    pub installed_revision: Option<u64>,
    /// SHA-256 of the exact canonical manifest already authoritative inside
    /// the compiler.
    pub installed_manifest_sha256: Option<[u8; 32]>,
    pub installed_provenance: Option<BlockerCatalogProvenance>,
    /// Whether this exact supply mode can admit an authenticated network
    /// refresh. Release-only builds keep filtering available while exposing
    /// refresh as unsupported rather than pretending to be unconfigured.
    pub refresh_supported: bool,
    /// Process-local generation of authenticated source material for the
    /// installed package.
    ///
    /// This advances only after the updater has reauthenticated and repaired
    /// the exact installed package without changing its signed identity.
    pub source_material_epoch: u64,
    /// Exact installed-package material is crossing its bounded authenticated
    /// repair barrier.
    pub source_material_repair_pending: bool,
    /// Exact installed-package material remains unusable after its bounded
    /// automatic repair admission and requires an explicit source refresh.
    pub source_material_repair_retry_pending: bool,
    /// A newer authenticated package is waiting for or crossing the compiler
    /// replacement barrier.
    pub activation_pending: bool,
    /// Candidate repair is idle until an explicit user request consumes one
    /// of the bounded retry admissions.
    pub repair_retry_pending: bool,
    /// Non-authoritative scheduling hint recorded before network work begins.
    pub last_refresh_attempt_unix: Option<u64>,
    /// Exact updater operation while refreshing or for the retained failure.
    pub refresh_operation: Option<u64>,
}

impl BlockerCatalogSnapshot {
    pub const fn not_configured() -> Self {
        Self {
            revision: 1,
            phase: BlockerCatalogPhase::Unavailable(BlockerCatalogUnavailable::NotConfigured),
            enabled_policy_terminal: false,
            package_revision: None,
            package_manifest_sha256: None,
            package_provenance: None,
            package_created_unix: None,
            package_expires_unix: None,
            package_stale: None,
            source_refresh_due: false,
            source_count: None,
            source_bytes: None,
            candidate_revision: None,
            candidate_manifest_sha256: None,
            candidate_provenance: None,
            candidate_created_unix: None,
            candidate_expires_unix: None,
            candidate_source_count: None,
            candidate_source_bytes: None,
            installed_revision: None,
            installed_manifest_sha256: None,
            installed_provenance: None,
            refresh_supported: false,
            source_material_epoch: 0,
            source_material_repair_pending: false,
            source_material_repair_retry_pending: false,
            activation_pending: false,
            repair_retry_pending: false,
            last_refresh_attempt_unix: None,
            refresh_operation: None,
        }
    }
}

/// Nonblocking admission result for a user-requested package refresh.
#[must_use]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlockerCatalogRefreshDispatch {
    Accepted {
        operation: u64,
    },
    Busy,
    /// The exact candidate consumed its bounded process retry budget.
    ///
    /// Catalog refresh admission stops, but this is not global compiler
    /// terminality: retained enabled policy and disabled allow-all remain
    /// usable.
    RetryLimitReached,
    /// The current authenticated supply is immutable for this build.
    Unsupported,
    Unavailable(BlockerCatalogUnavailable),
    Terminal,
}

/// Authenticated catalog maintenance remains separate from compilation and
/// per-profile policy authority.
///
/// Both methods must be nonblocking. `maintain` may perform only bounded
/// in-memory coordination and nonblocking worker admission; filesystem,
/// network, parsing, and native work belong to their dedicated workers.
pub trait BlockerCatalog {
    fn maintain(&self) -> BlockerCatalogSnapshot;

    fn request_refresh(&self) -> BlockerCatalogRefreshDispatch;

    /// Registers `wake`, called from the update worker after each status it
    /// publishes, such as a finished download, so the caller can `maintain`
    /// at once instead of at its next routine check. It must only queue work.
    fn set_change_waker(&self, _wake: Arc<dyn Fn() + Send + Sync>) {}
}

/// Bounded asynchronous compiler for browser-maintained filter material.
///
/// Implementations own their worker and source catalog. They must invoke an
/// accepted callback exactly once unless the profile was explicitly retired;
/// callbacks may run on a worker and therefore must never call native UI APIs
/// directly.
pub trait BlockerCompiler {
    fn prepare_site_preferences(
        &self,
        preferences: &crate::blocker::BlockerSitePreferences,
    ) -> Option<Arc<crate::blocker::PreparedBlockerSites>> {
        if !preferences.hides().is_empty() {
            return None;
        }
        crate::blocker::PreparedBlockerSites::new(
            preferences.revision(),
            preferences
                .paused_sites()
                .map(|site| (site.clone(), true, Arc::from(""))),
        )
    }
    /// Bounded, I/O-free validation for an explicit personal hide. Returning
    /// None refuses syntax that cannot be represented as static CSS.
    fn validate_personal_selector(&self, _selector: &str) -> Option<String> {
        None
    }
    fn compile(
        &self,
        profile: ProfileId,
        generation: ContentPolicyGeneration,
        config: BlockerConfig,
        done: Box<dyn FnOnce(BlockerCompileOutcome) + Send>,
    ) -> BlockerDispatch;

    /// Retires the currently admitted lifecycle of one profile identity.
    ///
    /// A scheduled barrier is ordered after the profile's previously accepted
    /// compile. Before invoking `done`, the implementation must forget the
    /// retired lifecycle so arbitrary create/delete churn cannot accumulate
    /// process-lifetime tombstones. New work may then be admitted, but its
    /// callback must remain ordered after `done` returns. A callback which
    /// began before retirement may finish before `done`; consumers must
    /// therefore generation-check every result.
    ///
    /// Implementations must not hold an internal admission lock while invoking
    /// either a compile callback or `done`.
    fn retire_profile(
        &self,
        profile: ProfileId,
        done: Box<dyn FnOnce() + Send>,
    ) -> BlockerRetirementDispatch;

    /// Seals admission and proves the worker exited within the caller-owned
    /// process shutdown deadline.
    fn shutdown_until(&self, deadline: Instant) -> BlockerShutdownOutcome;
}

/// Composition-boundary port implemented by the managed production service.
///
/// The two parent traits remain separate so catalog supply never acquires
/// profile/native authority and the compiler never acquires network access.
pub trait ContentBlocker: BlockerCompiler + BlockerCatalog {}

impl<T> ContentBlocker for T where T: BlockerCompiler + BlockerCatalog + ?Sized {}
