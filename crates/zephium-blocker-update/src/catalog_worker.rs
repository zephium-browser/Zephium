//! Shared coordination surface; each supply mode retains its own trust rules.
use crate::types::*;
use std::time::Duration;

/// Bounded source worker. Official HTTPS mode trusts the fixed publisher's TLS
/// endpoint; optional TUF mode verifies the separately provisioned repository.
pub struct CatalogUpdateWorker(Worker);
enum Worker {
    #[cfg(feature = "tuf")]
    Tuf(crate::worker::CatalogUpdateWorker),
    #[cfg(feature = "official-https")]
    Official(crate::official::OfficialWorker),
}
impl CatalogUpdateWorker {
    /// Starts the optional provisioned signed repository worker.
    #[cfg(feature = "tuf")]
    pub fn start(config: RepositoryConfig) -> Self {
        Self(Worker::Tuf(crate::worker::CatalogUpdateWorker::start(
            config,
        )))
    }
    /// Starts the fixed official HTTPS worker with a validated release fallback.
    /// No request is made until refresh is admitted by the coordinator.
    #[cfg(feature = "official-https")]
    pub fn start_official(
        storage: std::path::PathBuf,
        fallback: ActivatedCatalog,
    ) -> std::io::Result<Self> {
        crate::official::OfficialWorker::start(storage, fallback)
            .map(|worker| Self(Worker::Official(worker)))
    }
    /// Persisted due time for official HTTPS mode; signed repositories retain
    /// their existing coordinator-owned signed-expiry scheduling.
    pub fn next_refresh_unix(&self) -> Option<u64> {
        match &self.0 {
            #[cfg(feature = "tuf")]
            Worker::Tuf(_) => None,
            #[cfg(feature = "official-https")]
            Worker::Official(worker) => Some(worker.next_refresh_unix()),
        }
    }
    /// Successful-source verification sequence and advisory refresh need for
    /// official HTTPS mode. This never represents signed metadata freshness.
    pub fn official_freshness(&self) -> Option<(u64, bool)> {
        match &self.0 {
            #[cfg(feature = "tuf")]
            Worker::Tuf(_) => None,
            #[cfg(feature = "official-https")]
            Worker::Official(worker) => Some(worker.freshness()),
        }
    }
    /// Attempts one bounded refresh, coalescing existing candidate work.
    pub fn request_refresh(&self) -> RefreshAdmission {
        match &self.0 {
            #[cfg(feature = "tuf")]
            Worker::Tuf(worker) => worker.request_refresh(),
            #[cfg(feature = "official-https")]
            Worker::Official(worker) => worker.request_refresh(),
        }
    }
    /// Current revisioned worker status without filesystem I/O.
    pub fn status(&self) -> StatusSnapshot {
        match &self.0 {
            #[cfg(feature = "tuf")]
            Worker::Tuf(worker) => worker.status(),
            #[cfg(feature = "official-https")]
            Worker::Official(worker) => worker.status(),
        }
    }
    /// Observes status and transfers each initial/candidate catalog only once.
    pub fn observe_and_take_catalogs(
        &self,
    ) -> (
        StatusSnapshot,
        Option<ActivatedCatalog>,
        Option<ActivatedCatalog>,
    ) {
        match &self.0 {
            #[cfg(feature = "tuf")]
            Worker::Tuf(worker) => worker.observe_and_take_catalogs(),
            #[cfg(feature = "official-https")]
            Worker::Official(worker) => worker.observe_and_take_catalogs(),
        }
    }
    /// Durably promotes the exact candidate after compiler and native checks.
    pub fn commit_candidate(
        &self,
        identity: CatalogIdentity,
        done: Box<dyn FnOnce(CandidateCommitOutcome) + Send>,
    ) -> CandidateCommitDispatch {
        match &self.0 {
            #[cfg(feature = "tuf")]
            Worker::Tuf(worker) => worker.commit_candidate(identity, done),
            #[cfg(feature = "official-https")]
            Worker::Official(worker) => worker.commit_candidate(identity, done),
        }
    }
    /// Repairs one exact candidate without permitting a stale commit.
    pub fn repair_candidate(
        &self,
        identity: CatalogIdentity,
        done: Box<dyn FnOnce(CandidateRepairOutcome) + Send>,
    ) -> CandidateRepairDispatch {
        match &self.0 {
            #[cfg(feature = "tuf")]
            Worker::Tuf(worker) => worker.repair_candidate(identity, done),
            #[cfg(feature = "official-https")]
            Worker::Official(worker) => worker.repair_candidate(identity, done, false),
        }
    }
    /// Consumes one explicit bounded retry for an exact candidate.
    pub fn retry_candidate(
        &self,
        identity: CatalogIdentity,
        done: Box<dyn FnOnce(CandidateRepairOutcome) + Send>,
    ) -> CandidateRepairDispatch {
        match &self.0 {
            #[cfg(feature = "tuf")]
            Worker::Tuf(worker) => worker.retry_candidate(identity, done),
            #[cfg(feature = "official-https")]
            Worker::Official(worker) => worker.repair_candidate(identity, done, true),
        }
    }
    /// Rejects one candidate while preserving previously working sources.
    pub fn reject_candidate(
        &self,
        identity: CatalogIdentity,
        failure: FailureKind,
        reason: CandidateRejectionReason,
        done: Box<dyn FnOnce(CandidateRejectOutcome) + Send>,
    ) -> CandidateRejectDispatch {
        match &self.0 {
            #[cfg(feature = "tuf")]
            Worker::Tuf(worker) => worker.reject_candidate(identity, failure, reason, done),
            #[cfg(feature = "official-https")]
            Worker::Official(worker) => worker.reject_candidate(identity, failure, reason, done),
        }
    }
    /// Calls `wake` after each status the worker publishes. The signed
    /// repository worker is not provisioned and keeps the routine check.
    pub fn set_change_waker(&self, wake: std::sync::Arc<dyn Fn() + Send + Sync>) {
        match &self.0 {
            #[cfg(feature = "tuf")]
            Worker::Tuf(_) => drop(wake),
            #[cfg(feature = "official-https")]
            Worker::Official(worker) => worker.set_change_waker(wake),
        }
    }
    /// Waits for a newer status revision, without periodic background polling.
    pub fn wait_for_change(&self, revision: u64, timeout: Duration) -> WaitForStatus {
        match &self.0 {
            #[cfg(feature = "tuf")]
            Worker::Tuf(worker) => worker.wait_for_change(revision, timeout),
            #[cfg(feature = "official-https")]
            Worker::Official(worker) => worker.wait_for_change(revision, timeout),
        }
    }
    /// Cancels transport, seals admission and joins within a finite deadline.
    pub fn shutdown(self, timeout: Duration) -> ShutdownOutcome {
        match self.0 {
            #[cfg(feature = "tuf")]
            Worker::Tuf(worker) => worker.shutdown(timeout),
            #[cfg(feature = "official-https")]
            Worker::Official(worker) => worker.shutdown(timeout),
        }
    }
}
