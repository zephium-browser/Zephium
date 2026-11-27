//! Filter-package validation and optional authenticated acquisition.
//!
//! This crate deliberately has no browser, profile, page, or UI dependency.
//! Canonical package validation is always available for application-bundled
//! catalogs. The `tuf` feature adds one application-shipped trust root, fixed
//! HTTPS repository origins, durable state, and the bounded update worker.

#![deny(unsafe_code)]
#![deny(missing_docs)]

mod manifest;
#[cfg(feature = "official-https")]
mod official;
#[cfg(feature = "official-https")]
mod official_store;
mod types;

#[cfg(any(feature = "tuf", feature = "official-https"))]
mod cache_lock;
#[cfg(any(feature = "tuf", feature = "official-https"))]
mod file_identity;
#[cfg(feature = "tuf")]
mod repository;
#[cfg(feature = "tuf")]
mod storage;
#[cfg(any(feature = "tuf", feature = "official-https"))]
mod storage_io;
#[cfg(feature = "tuf")]
mod transport;
#[cfg(feature = "tuf")]
mod worker;

#[cfg(all(test, feature = "tuf"))]
mod tests;

pub use manifest::{
    CatalogManifest, LicenseMetadata, ManifestError, ManifestSource, ManifestSourceFormat,
    CATALOG_MANIFEST_TARGET, CATALOG_MANIFEST_VERSION,
};
#[cfg(feature = "tuf")]
pub use storage::GarbageCollectionReport;
#[cfg(any(feature = "tuf", feature = "official-https"))]
pub use storage_io::StoreError;
pub use types::{
    ActivatedCatalog, CandidateCommitDispatch, CandidateCommitOutcome, CandidateRejectDispatch,
    CandidateRejectOutcome, CandidateRejectionReason, CandidateRepairDispatch,
    CandidateRepairOutcome, CatalogAvailability, CatalogIdentity, ConfigError, FailureKind,
    GcBudget, LicensePolicy, RefreshAdmission, ShutdownOutcome, StatusSnapshot, UnavailableReason,
    UpdateLimits, UpdateStatus, WaitForStatus,
};
#[cfg(feature = "tuf")]
pub use types::{RepositoryConfig, RepositoryId};
#[cfg(any(feature = "tuf", feature = "official-https"))]
mod catalog_worker;
#[cfg(any(feature = "tuf", feature = "official-https"))]
pub use catalog_worker::CatalogUpdateWorker;

#[cfg(not(any(feature = "tuf", feature = "official-https")))]
mod disabled_worker {
    use std::time::Duration;

    use crate::types::{
        ActivatedCatalog, CandidateCommitDispatch, CandidateCommitOutcome, CandidateRejectDispatch,
        CandidateRejectOutcome, CandidateRejectionReason, CandidateRepairDispatch,
        CandidateRepairOutcome, CatalogIdentity, RefreshAdmission, ShutdownOutcome, StatusSnapshot,
        WaitForStatus,
    };

    /// Uninhabited update-worker marker for builds without the `tuf` feature.
    ///
    /// `Option<CatalogUpdateWorker>` is therefore provably always `None`.
    /// Keeping the type available lets the bundle coordinator share one
    /// audited layout without linking repository, network, TLS, or worker
    /// code into the product.
    pub enum CatalogUpdateWorker {}

    impl CatalogUpdateWorker {
        /// Unreachable without an enabled source worker.
        pub fn next_refresh_unix(&self) -> Option<u64> {
            match *self {}
        }
        /// Unreachable without an enabled source worker.
        pub fn set_change_waker(&self, _wake: std::sync::Arc<dyn Fn() + Send + Sync>) {
            match *self {}
        }
        /// Unreachable without an enabled source worker.
        pub fn official_freshness(&self) -> Option<(u64, bool)> {
            match *self {}
        }
        /// Unreachable without the feature that can construct this type.
        pub fn request_refresh(&self) -> RefreshAdmission {
            match *self {}
        }

        /// Unreachable without the feature that can construct this type.
        pub fn status(&self) -> StatusSnapshot {
            match *self {}
        }

        /// Unreachable without the feature that can construct this type.
        pub fn observe_and_take_catalogs(
            &self,
        ) -> (
            StatusSnapshot,
            Option<ActivatedCatalog>,
            Option<ActivatedCatalog>,
        ) {
            match *self {}
        }

        /// Unreachable without the feature that can construct this type.
        pub fn commit_candidate(
            &self,
            _identity: CatalogIdentity,
            _done: Box<dyn FnOnce(CandidateCommitOutcome) + Send>,
        ) -> CandidateCommitDispatch {
            match *self {}
        }

        /// Unreachable without the feature that can construct this type.
        pub fn repair_candidate(
            &self,
            _identity: CatalogIdentity,
            _done: Box<dyn FnOnce(CandidateRepairOutcome) + Send>,
        ) -> CandidateRepairDispatch {
            match *self {}
        }

        /// Unreachable without the feature that can construct this type.
        pub fn retry_candidate(
            &self,
            _identity: CatalogIdentity,
            _done: Box<dyn FnOnce(CandidateRepairOutcome) + Send>,
        ) -> CandidateRepairDispatch {
            match *self {}
        }

        /// Unreachable without the feature that can construct this type.
        pub fn reject_candidate(
            &self,
            _identity: CatalogIdentity,
            _failure: crate::FailureKind,
            _reason: CandidateRejectionReason,
            _done: Box<dyn FnOnce(CandidateRejectOutcome) + Send>,
        ) -> CandidateRejectDispatch {
            match *self {}
        }

        /// Unreachable without the feature that can construct this type.
        pub fn wait_for_change(&self, _after_revision: u64, _deadline: Duration) -> WaitForStatus {
            match *self {}
        }

        /// Unreachable without the feature that can construct this type.
        pub fn shutdown(self, _timeout: Duration) -> ShutdownOutcome {
            match self {}
        }
    }
}

#[cfg(not(any(feature = "tuf", feature = "official-https")))]
pub use disabled_worker::CatalogUpdateWorker;
