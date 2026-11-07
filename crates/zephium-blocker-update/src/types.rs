use std::collections::HashSet;
#[cfg(feature = "tuf")]
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use thiserror::Error;
#[cfg(feature = "tuf")]
use url::Url;
use zephium_blocker::PolicyCatalog;
#[cfg(feature = "tuf")]
use zephium_blocker::Sha256;

/// Hard maximum for a shipped trusted root.
pub(crate) const HARD_MAX_ROOT_BYTES: usize = 256 * 1024;
pub(crate) const HARD_MAX_TARGETS_METADATA_BYTES: u64 = 1024 * 1024;
pub(crate) const HARD_MAX_TIMESTAMP_METADATA_BYTES: u64 = 256 * 1024;
pub(crate) const HARD_MAX_SNAPSHOT_METADATA_BYTES: u64 = 512 * 1024;
pub(crate) const HARD_MAX_KNOWN_TIME_BYTES: u64 = 4 * 1024;
/// Hard maximum for one catalog manifest.
pub(crate) const HARD_MAX_MANIFEST_BYTES: usize = 128 * 1024;
/// Hard maximum number of package sources.
pub(crate) const HARD_MAX_SOURCES: usize = 32;
/// Hard maximum for one source.
pub(crate) const HARD_MAX_SOURCE_BYTES: u64 = 16 * 1024 * 1024;
/// Hard maximum for all source bytes.
pub(crate) const HARD_MAX_TOTAL_SOURCE_BYTES: u64 = 32 * 1024 * 1024;
pub(crate) const HARD_MAX_CACHE_OBJECTS: usize = 1024;
pub(crate) const HARD_MAX_TUF_OBJECTS: usize = 5;

/// Bounded resource and network settings for one update attempt.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct UpdateLimits {
    /// Maximum trusted-root or downloaded root metadata bytes.
    pub max_root_bytes: u64,
    /// Maximum top-level targets metadata bytes.
    pub max_targets_metadata_bytes: u64,
    /// Maximum timestamp metadata bytes.
    pub max_timestamp_metadata_bytes: u64,
    /// Maximum snapshot metadata bytes.
    pub max_snapshot_metadata_bytes: u64,
    /// Maximum sequential root rotations accepted in one refresh.
    pub max_root_updates: u64,
    /// Maximum catalog-manifest bytes.
    pub max_manifest_bytes: usize,
    /// Maximum source count.
    pub max_sources: usize,
    /// Maximum bytes in one source.
    pub max_source_bytes: u64,
    /// Maximum bytes across all sources.
    pub max_total_source_bytes: u64,
    /// Complete HTTP request deadline.
    pub request_timeout: Duration,
    /// HTTP connect deadline.
    pub connect_timeout: Duration,
}

impl Default for UpdateLimits {
    fn default() -> Self {
        Self {
            max_root_bytes: 256 * 1024,
            max_targets_metadata_bytes: 512 * 1024,
            max_timestamp_metadata_bytes: 64 * 1024,
            max_snapshot_metadata_bytes: 128 * 1024,
            max_root_updates: 32,
            max_manifest_bytes: HARD_MAX_MANIFEST_BYTES,
            max_sources: HARD_MAX_SOURCES,
            max_source_bytes: HARD_MAX_SOURCE_BYTES,
            max_total_source_bytes: HARD_MAX_TOTAL_SOURCE_BYTES,
            request_timeout: Duration::from_secs(30),
            connect_timeout: Duration::from_secs(10),
        }
    }
}

impl UpdateLimits {
    #[cfg(feature = "tuf")]
    fn validate(self) -> Result<Self, ConfigError> {
        let bounded = self.max_root_bytes > 0
            && self.max_root_bytes <= HARD_MAX_ROOT_BYTES as u64
            && self.max_targets_metadata_bytes > 0
            && self.max_targets_metadata_bytes <= HARD_MAX_TARGETS_METADATA_BYTES
            && self.max_timestamp_metadata_bytes > 0
            && self.max_timestamp_metadata_bytes <= HARD_MAX_TIMESTAMP_METADATA_BYTES
            && self.max_snapshot_metadata_bytes > 0
            && self.max_snapshot_metadata_bytes <= HARD_MAX_SNAPSHOT_METADATA_BYTES
            && (1..=64).contains(&self.max_root_updates)
            && self.max_manifest_bytes > 0
            && self.max_manifest_bytes <= HARD_MAX_MANIFEST_BYTES
            && self.max_sources > 0
            && self.max_sources <= HARD_MAX_SOURCES
            && self.max_source_bytes > 0
            && self.max_source_bytes <= HARD_MAX_SOURCE_BYTES
            && self.max_total_source_bytes >= self.max_source_bytes
            && self.max_total_source_bytes <= HARD_MAX_TOTAL_SOURCE_BYTES
            && self.connect_timeout > Duration::ZERO
            && self.request_timeout >= self.connect_timeout
            && self.request_timeout <= Duration::from_secs(60);
        if !bounded {
            return Err(ConfigError::InvalidLimits);
        }
        Ok(self)
    }
}

/// Immutable configuration for one authenticated repository.
///
/// There is intentionally no default endpoint or root. Release engineering
/// must inject both together after provisioning production keys.
#[derive(Clone, Debug)]
#[cfg(feature = "tuf")]
pub struct RepositoryConfig {
    pub(crate) trusted_root: Arc<[u8]>,
    pub(crate) metadata_base_url: Url,
    pub(crate) targets_base_url: Url,
    pub(crate) storage_dir: PathBuf,
    pub(crate) limits: UpdateLimits,
    pub(crate) license_policy: LicensePolicy,
    pub(crate) repository_identity: [u8; 32],
}

#[cfg(feature = "tuf")]
impl RepositoryConfig {
    /// Creates a fixed-origin HTTPS configuration.
    pub fn new(
        repository_id: RepositoryId,
        trusted_root: impl Into<Arc<[u8]>>,
        metadata_base_url: &str,
        targets_base_url: &str,
        storage_dir: PathBuf,
        license_policy: LicensePolicy,
    ) -> Result<Self, ConfigError> {
        Self::with_limits(
            repository_id,
            trusted_root,
            metadata_base_url,
            targets_base_url,
            storage_dir,
            license_policy,
            UpdateLimits::default(),
        )
    }

    /// Creates a fixed-origin HTTPS configuration with stricter custom limits.
    pub fn with_limits(
        repository_id: RepositoryId,
        trusted_root: impl Into<Arc<[u8]>>,
        metadata_base_url: &str,
        targets_base_url: &str,
        storage_dir: PathBuf,
        license_policy: LicensePolicy,
        limits: UpdateLimits,
    ) -> Result<Self, ConfigError> {
        let trusted_root = trusted_root.into();
        if trusted_root.is_empty() || trusted_root.len() > HARD_MAX_ROOT_BYTES {
            return Err(ConfigError::InvalidTrustedRoot);
        }
        let metadata_base_url = parse_https_base(metadata_base_url)?;
        let targets_base_url = parse_https_base(targets_base_url)?;
        validate_storage_path(&storage_dir)?;
        let repository_identity =
            repository_identity(&repository_id, &metadata_base_url, &targets_base_url);
        Ok(Self {
            trusted_root,
            metadata_base_url,
            targets_base_url,
            storage_dir,
            limits: limits.validate()?,
            license_policy,
            repository_identity,
        })
    }

    /// Returns the private application-data directory used by this repository.
    pub fn storage_dir(&self) -> &Path {
        &self.storage_dir
    }

    pub(crate) fn package_admission_policy_sha256(&self) -> [u8; 32] {
        let mut hasher = Sha256::new();
        // v2 binds the project-owned single-segment target-name grammar.
        // Durable TUF state admitted under the former Tough-only grammar must
        // be reauthenticated before it can cross the compiler boundary.
        hasher.update(b"zephium:blocker-package-admission:v2\0");
        for value in [
            self.limits.max_manifest_bytes as u64,
            self.limits.max_sources as u64,
            self.limits.max_source_bytes,
            self.limits.max_total_source_bytes,
        ] {
            hasher.update(value.to_be_bytes());
        }
        let mut licenses = self
            .license_policy
            .accepted
            .iter()
            .map(|expression| expression.as_bytes())
            .collect::<Vec<_>>();
        licenses.sort_unstable();
        hasher.update((licenses.len() as u64).to_be_bytes());
        for license in licenses {
            hasher.update((license.len() as u64).to_be_bytes());
            hasher.update(license);
        }
        hasher.finalize()
    }

    #[cfg(test)]
    pub(crate) fn for_filesystem_tests(
        trusted_root: Arc<[u8]>,
        metadata_base_url: Url,
        targets_base_url: Url,
        storage_dir: PathBuf,
        license_policy: LicensePolicy,
        limits: UpdateLimits,
    ) -> Self {
        let repository_id = RepositoryId::new("zephium-test-filter-repository-v1")
            .expect("valid test repository ID");
        let repository_identity =
            repository_identity(&repository_id, &metadata_base_url, &targets_base_url);
        Self {
            trusted_root,
            metadata_base_url,
            targets_base_url,
            storage_dir,
            limits,
            license_policy,
            repository_identity,
        }
    }
}

/// Stable identifier for one TUF trust domain and repository epoch.
///
/// Keep this unchanged across ordinary sequential root rotation. Endpoint
/// migration or emergency trust re-anchoring requires a new identifier and a
/// separate cache namespace.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
#[cfg(feature = "tuf")]
pub struct RepositoryId(Box<str>);

#[cfg(feature = "tuf")]
impl RepositoryId {
    /// Creates a lowercase ASCII identifier of at most 64 bytes.
    pub fn new(value: impl Into<Box<str>>) -> Result<Self, ConfigError> {
        let value = value.into();
        let valid = !value.is_empty()
            && value.len() <= 64
            && value.bytes().enumerate().all(|(index, byte)| {
                byte.is_ascii_lowercase()
                    || byte.is_ascii_digit()
                    || (byte == b'-' && index > 0 && index + 1 < value.len())
            });
        if !valid {
            return Err(ConfigError::InvalidRepositoryId);
        }
        Ok(Self(value))
    }

    fn as_str(&self) -> &str {
        &self.0
    }
}

/// Exact shipped policy for licenses accepted into a redistributed catalog.
///
/// Expressions are compared byte-for-byte. A package cannot broaden an
/// approved expression by substituting an equivalent-looking SPDX formula.
#[derive(Clone, Debug)]
pub struct LicensePolicy {
    accepted: Arc<HashSet<Box<str>>>,
}

impl LicensePolicy {
    /// Creates a bounded exact-expression allowlist.
    pub fn new(
        expressions: impl IntoIterator<Item = impl Into<Box<str>>>,
    ) -> Result<Self, ConfigError> {
        let mut accepted = HashSet::new();
        for expression in expressions {
            let expression = expression.into();
            if expression.is_empty()
                || expression.len() > 256
                || expression.chars().any(char::is_control)
                || !accepted.insert(expression)
            {
                return Err(ConfigError::InvalidLicensePolicy);
            }
            if accepted.len() > 32 {
                return Err(ConfigError::InvalidLicensePolicy);
            }
        }
        if accepted.is_empty() {
            return Err(ConfigError::InvalidLicensePolicy);
        }
        Ok(Self {
            accepted: Arc::new(accepted),
        })
    }

    pub(crate) fn accepts(&self, expression: &str) -> bool {
        self.accepted.contains(expression)
    }
}

#[cfg(feature = "tuf")]
fn parse_https_base(raw: &str) -> Result<Url, ConfigError> {
    if raw.is_empty()
        || raw.len() > 2048
        || raw.contains('\\')
        || raw.contains('%')
        || raw
            .chars()
            .any(|character| character.is_control() || character.is_whitespace())
    {
        return Err(ConfigError::InvalidRepositoryOrigin);
    }
    let url = Url::parse(raw).map_err(|_| ConfigError::InvalidRepositoryOrigin)?;
    let path = url.path();
    let interior_path = path
        .strip_prefix('/')
        .and_then(|path| path.strip_suffix('/'))
        .unwrap_or_default();
    let clean_path =
        path == "/" || (!interior_path.is_empty() && !interior_path.split('/').any(str::is_empty));
    let clean = url.scheme() == "https"
        && url.host_str().is_some()
        && url.username().is_empty()
        && url.password().is_none()
        && url.query().is_none()
        && url.fragment().is_none()
        && url.path().ends_with('/')
        && url.as_str() == raw
        && clean_path;
    if !clean {
        return Err(ConfigError::InvalidRepositoryOrigin);
    }
    Ok(url)
}

#[cfg(feature = "tuf")]
fn repository_identity(
    repository_id: &RepositoryId,
    metadata_base_url: &Url,
    targets_base_url: &Url,
) -> [u8; 32] {
    let mut digest = Sha256::new();
    digest.update(b"zephium-filter-repository-v1\0");
    digest.update(repository_id.as_str().as_bytes());
    digest.update([0]);
    digest.update(metadata_base_url.as_str().as_bytes());
    digest.update([0]);
    digest.update(targets_base_url.as_str().as_bytes());
    digest.finalize()
}

#[cfg(feature = "tuf")]
fn validate_storage_path(path: &Path) -> Result<(), ConfigError> {
    if !path.is_absolute() || path.file_name().is_none() {
        return Err(ConfigError::InvalidStoragePath);
    }
    Ok(())
}

/// Configuration rejected before any filesystem or network work.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum ConfigError {
    /// The explicit repository trust-domain identifier is malformed.
    #[error("invalid repository trust-domain identifier")]
    InvalidRepositoryId,
    /// The trusted root is empty or exceeds its hard bound.
    #[error("invalid trusted TUF root")]
    InvalidTrustedRoot,
    /// A repository URL is not a clean fixed HTTPS base URL.
    #[error("invalid fixed HTTPS repository origin")]
    InvalidRepositoryOrigin,
    /// The private storage path is not absolute.
    #[error("invalid updater storage path")]
    InvalidStoragePath,
    /// One or more resource limits are empty, inconsistent, or above a hard cap.
    #[error("invalid updater limits")]
    InvalidLimits,
    /// The shipped exact license allowlist is empty, duplicated, or unbounded.
    #[error("invalid package license policy")]
    InvalidLicensePolicy,
}

/// Stable reason the updater cannot operate on this build or filesystem.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UnavailableReason {
    /// Crash-durable atomic activation is not implemented for this operating system.
    DurableActivationUnsupported,
    /// The durable clock high-water record is corrupt or outside the admitted range.
    ClockUnsafe,
    /// The private cache could not establish its durability invariants.
    StorageUnavailable,
}

/// Stable failure category suitable for status and telemetry-free diagnostics.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureKind {
    /// Fixed-origin transport failed or rejected a response.
    Transport,
    /// TUF metadata failed authentication, rollback, size, or expiry checks.
    Metadata,
    /// The wall clock moved backward or outside the updater's admitted range.
    Clock,
    /// The package manifest was absent, malformed, non-canonical, or expired.
    Manifest,
    /// A target was absent, oversized, corrupt, or not UTF-8.
    Target,
    /// Required license and redistribution metadata was invalid.
    License,
    /// Package revision rollback or equivocation was detected.
    Rollback,
    /// Durable storage admission, verification, or activation failed.
    Storage,
    /// The verified sources could not form a bounded compiler catalog.
    Catalog,
    /// The worker stopped due to an internal invariant failure.
    Internal,
}

impl FailureKind {
    /// Whether an exact candidate repair may consume a later user-armed
    /// process-local retry.
    ///
    /// Rollback/authority contradictions, unsafe durable storage, and
    /// internal invariant failures remain fail-closed.
    pub const fn candidate_repair_retryable(self) -> bool {
        !matches!(self, Self::Rollback | Self::Storage | Self::Internal)
    }

    /// Whether failed current-package material may wait for an explicit
    /// authenticated refresh instead of sealing enabled-policy admission.
    ///
    /// Unsafe storage identity, signed-authority contradiction, and internal
    /// invariant failures can never authorize replacement or retry.
    pub const fn source_material_repair_retryable(self) -> bool {
        !matches!(self, Self::Rollback | Self::Storage | Self::Internal)
    }
}

/// Immutable identity of one activated package.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CatalogIdentity {
    /// Strictly monotonic package revision.
    pub revision: u64,
    /// SHA-256 of the exact canonical manifest bytes.
    pub manifest_sha256: [u8; 32],
    /// Package creation time as Unix seconds.
    pub created_unix: u64,
    /// Package expiry time as Unix seconds.
    pub expires_unix: u64,
    /// Number of immutable sources.
    pub source_count: u32,
    /// Total UTF-8 source bytes.
    pub source_bytes: u64,
}

/// A catalog and the authenticated identity that produced it.
#[derive(Clone, Debug)]
pub struct ActivatedCatalog {
    /// Authenticated package identity.
    pub identity: CatalogIdentity,
    /// Immutable catalog accepted by the blocker compiler boundary.
    pub catalog: PolicyCatalog,
}

/// Freshness of a retained authenticated package.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CatalogAvailability {
    /// The package has not reached its signed expiry.
    Fresh(CatalogIdentity),
    /// The package is expired but retained as degraded last-known-good policy.
    ///
    /// Stale policy remains usable so an unavailable update service does not
    /// silently disable filtering. Integrators must expose the degraded state.
    Stale(CatalogIdentity),
}

impl CatalogAvailability {
    #[cfg(feature = "tuf")]
    pub(crate) fn from_identity(identity: CatalogIdentity, now_unix: u64) -> Self {
        if identity.expires_unix > now_unix {
            Self::Fresh(identity)
        } else {
            Self::Stale(identity)
        }
    }
}

/// Stable updater state.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum UpdateStatus {
    /// The updater cannot safely operate on this platform or storage.
    Unavailable(UnavailableReason),
    /// Ready, with no accepted package yet.
    Idle,
    /// Ready with an authenticated current package.
    Ready(CatalogAvailability),
    /// One bounded refresh is in progress.
    Refreshing {
        /// Monotonic in-process operation identifier.
        operation: u64,
        /// Current package retained while refreshing.
        current: Option<CatalogAvailability>,
    },
    /// The last refresh failed; an earlier package remains active when present.
    Failed {
        /// Failed operation identifier.
        operation: u64,
        /// Stable failure category.
        failure: FailureKind,
        /// Earlier authenticated package, if any.
        current: Option<CatalogAvailability>,
    },
    /// Admission is sealed and the worker is exiting or exited.
    Shutdown,
}

/// Revision-tagged status used for event-driven observation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StatusSnapshot {
    /// Strictly increasing in-process status revision.
    pub revision: u64,
    /// Last durably admitted network refresh attempt, if known.
    ///
    /// This is a scheduling hint, not authenticated package state. Corrupt or
    /// absent hints are reported as `None` so startup scheduling treats the
    /// updater as due rather than suppressing refresh forever.
    pub last_refresh_attempt_unix: Option<u64>,
    /// State at this revision.
    pub status: UpdateStatus,
}

/// Result of a deadline-bounded status wait.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WaitForStatus {
    /// A revision newer than the caller's observation is available.
    Changed(StatusSnapshot),
    /// The deadline elapsed without a status transition.
    TimedOut,
}

/// Result of nonblocking refresh admission.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RefreshAdmission {
    /// The operation was admitted to the sole worker.
    Accepted(u64),
    /// Another refresh is already admitted or running.
    Busy,
    /// The updater is explicitly unavailable.
    Unavailable(UnavailableReason),
    /// Shutdown has sealed command admission.
    Shutdown,
}

/// Nonblocking admission for committing one compiler-prepared candidate.
#[must_use]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CandidateCommitDispatch {
    /// The updater worker owns the exact commit and completion callback.
    Scheduled,
    /// Another commit is pending or the bounded worker slot was unavailable.
    Rejected,
    /// Updater admission is sealed.
    Terminal,
}

/// Exact terminal result of a candidate commit.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CandidateCommitOutcome {
    /// The prepared candidate crossed the durable current-package barrier.
    Committed,
    /// Durable activation failed without authorizing compiler activation.
    Failed(FailureKind),
}

/// Nonblocking admission for repairing one exact durable candidate.
///
/// An exact repair changes only authenticated cache material. A repository
/// that has advanced may instead atomically supersede the candidate and
/// signed high-water while preserving current authority.
#[must_use]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CandidateRepairDispatch {
    /// The updater worker owns the exact repair and completion callback.
    Scheduled {
        /// Exact process-local operation retained through activation.
        operation: u64,
    },
    /// Another candidate transition is pending or the bounded worker slot was
    /// unavailable; no retry budget was consumed.
    Rejected,
    /// The hard per-candidate process retry budget was consumed.
    LimitReached,
    /// Updater admission is sealed.
    Terminal,
}

/// Exact terminal result of one authenticated candidate-cache repair.
#[derive(Clone, Debug)]
pub enum CandidateRepairOutcome {
    /// Every candidate object was authenticated and durably replaced.
    Repaired,
    /// A fully authenticated newer package atomically replaced the older
    /// durable candidate and advanced signed high-water authority.
    ///
    /// The exact immutable catalog travels with completion so a consumer does
    /// not depend on separately sampling the worker's publication queue.
    Superseded(ActivatedCatalog),
    /// Repair failed without changing package authority.
    Failed(FailureKind),
}

/// Nonblocking admission for durably abandoning one rejected candidate.
#[must_use]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CandidateRejectDispatch {
    /// The updater worker owns the exact rejection and completion callback.
    Scheduled,
    /// Another candidate transition is pending or the worker slot was unavailable.
    Rejected,
    /// Updater admission is sealed.
    Terminal,
}

/// Exact terminal result of a durable candidate rejection.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CandidateRejectOutcome {
    /// Candidate authority was removed while its signed high-water remained.
    Rejected,
    /// Durable rejection failed and updater state remains authoritative.
    Failed(FailureKind),
}

/// Durable reason one exact candidate is being abandoned.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CandidateRejectionReason {
    /// Deterministic rejection by the current compiler-policy fingerprint.
    CompilerPolicy,
    /// Candidate validity ended before durable activation.
    Expired,
}

/// Result of deadline-bounded worker shutdown.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ShutdownOutcome {
    /// The worker completed and joined before the deadline.
    Complete,
    /// No worker existed because this platform was unavailable.
    Unavailable,
    /// The worker did not finish before the caller's deadline and was detached.
    TimedOut,
}

/// Work bound for one opportunistic cache collection pass.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GcBudget {
    /// Maximum directory entries inspected.
    pub max_entries_scanned: usize,
    /// Maximum objects unlinked.
    pub max_objects_removed: usize,
    /// Maximum total bytes unlinked.
    pub max_bytes_removed: u64,
}

impl Default for GcBudget {
    fn default() -> Self {
        Self {
            // One pass observes the complete admitted CAS namespace. A
            // smaller prefix can permanently starve entries because
            // read_dir has no stable ordering or cursor.
            max_entries_scanned: HARD_MAX_CACHE_OBJECTS,
            // One maximum package is 32 sources plus its manifest and five
            // authenticated TUF datastore objects.
            max_objects_removed: HARD_MAX_SOURCES + 1 + HARD_MAX_TUF_OBJECTS,
            // This covers the hard source-total limit, maximum manifest, and
            // every maximum-sized TUF object admitted by UpdateLimits.
            max_bytes_removed: HARD_MAX_TOTAL_SOURCE_BYTES
                + HARD_MAX_MANIFEST_BYTES as u64
                + HARD_MAX_ROOT_BYTES as u64
                + HARD_MAX_TARGETS_METADATA_BYTES
                + HARD_MAX_TIMESTAMP_METADATA_BYTES
                + HARD_MAX_SNAPSHOT_METADATA_BYTES
                + HARD_MAX_KNOWN_TIME_BYTES,
        }
    }
}

#[cfg(all(test, feature = "tuf"))]
mod tests {
    use super::*;

    #[test]
    fn repository_id_is_a_bounded_stable_namespace() {
        assert!(RepositoryId::new("zephium-filter-lists-v1").is_ok());
        for invalid in [
            "",
            "-leading",
            "trailing-",
            "Uppercase",
            "contains_underscore",
            "contains space",
        ] {
            assert_eq!(
                RepositoryId::new(invalid),
                Err(ConfigError::InvalidRepositoryId)
            );
        }
        assert_eq!(
            RepositoryId::new("a".repeat(65)),
            Err(ConfigError::InvalidRepositoryId)
        );
    }

    #[test]
    fn package_admission_fingerprint_is_canonical_and_policy_exact() {
        let config = |licenses: &[&str], limits: UpdateLimits| RepositoryConfig {
            trusted_root: Arc::from(&b"root"[..]),
            metadata_base_url: Url::parse("https://metadata.example.invalid/").unwrap(),
            targets_base_url: Url::parse("https://targets.example.invalid/").unwrap(),
            storage_dir: PathBuf::from("/not-used"),
            limits,
            license_policy: LicensePolicy::new(licenses.iter().copied()).unwrap(),
            repository_identity: [7; 32],
        };
        let first = config(&["MIT", "Apache-2.0"], UpdateLimits::default());
        let reordered = config(&["Apache-2.0", "MIT"], UpdateLimits::default());
        assert_eq!(
            first.package_admission_policy_sha256(),
            reordered.package_admission_policy_sha256()
        );

        let narrower_license = config(&["MIT"], UpdateLimits::default());
        assert_ne!(
            first.package_admission_policy_sha256(),
            narrower_license.package_admission_policy_sha256()
        );
        let mut narrower_limits = UpdateLimits::default();
        narrower_limits.max_sources -= 1;
        let narrower_limits = config(&["MIT", "Apache-2.0"], narrower_limits);
        assert_ne!(
            first.package_admission_policy_sha256(),
            narrower_limits.package_admission_policy_sha256()
        );
    }

    #[test]
    fn candidate_repair_retryability_is_exhaustive_and_fail_closed() {
        for retryable in [
            FailureKind::Transport,
            FailureKind::Metadata,
            FailureKind::Clock,
            FailureKind::Manifest,
            FailureKind::Target,
            FailureKind::License,
            FailureKind::Catalog,
        ] {
            assert!(retryable.candidate_repair_retryable());
        }
        for terminal in [
            FailureKind::Rollback,
            FailureKind::Storage,
            FailureKind::Internal,
        ] {
            assert!(!terminal.candidate_repair_retryable());
        }
    }

    #[test]
    fn source_material_repair_retryability_is_exhaustive_and_fail_closed() {
        for retryable in [
            FailureKind::Transport,
            FailureKind::Metadata,
            FailureKind::Clock,
            FailureKind::Manifest,
            FailureKind::Target,
            FailureKind::License,
            FailureKind::Catalog,
        ] {
            assert!(retryable.source_material_repair_retryable());
        }
        for terminal in [
            FailureKind::Rollback,
            FailureKind::Storage,
            FailureKind::Internal,
        ] {
            assert!(!terminal.source_material_repair_retryable());
        }
    }

    #[test]
    fn default_gc_can_retire_one_maximal_package_and_tuf_snapshot() {
        let budget = GcBudget::default();
        assert_eq!(budget.max_entries_scanned, HARD_MAX_CACHE_OBJECTS);
        assert!(budget.max_objects_removed >= HARD_MAX_SOURCES + 1 + HARD_MAX_TUF_OBJECTS);
        assert!(
            budget.max_bytes_removed
                >= HARD_MAX_TOTAL_SOURCE_BYTES
                    + HARD_MAX_MANIFEST_BYTES as u64
                    + HARD_MAX_ROOT_BYTES as u64
                    + HARD_MAX_TARGETS_METADATA_BYTES
                    + HARD_MAX_TIMESTAMP_METADATA_BYTES
                    + HARD_MAX_SNAPSHOT_METADATA_BYTES
                    + HARD_MAX_KNOWN_TIME_BYTES
        );
    }

    #[test]
    fn repository_bases_require_exact_canonical_https_boundaries() {
        for valid in [
            "https://updates.example/",
            "https://updates.example/metadata/",
            "https://updates.example:8443/metadata/",
        ] {
            assert_eq!(parse_https_base(valid).unwrap().as_str(), valid);
        }

        for invalid in [
            "http://updates.example/metadata/",
            "https://updates.example/metadata",
            "https://updates.example/metadata//",
            "https://updates.example//metadata/",
            "https://updates.example/metadata/../private/",
            "https://updates.example/metadata/%2e%2e/private/",
            "https://updates.example/metadata\\private/",
            "https://updates.example/metadata/?token=x",
            "https://updates.example/metadata/#fragment",
            "https://user@updates.example/metadata/",
            "https://UPDATES.example/metadata/",
            "https://updates.example:443/metadata/",
        ] {
            assert_eq!(
                parse_https_base(invalid),
                Err(ConfigError::InvalidRepositoryOrigin),
                "{invalid}"
            );
        }
    }
}
