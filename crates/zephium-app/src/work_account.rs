//! Actor-approved service identity and independently collected document evidence.
//!
//! Browser storage, cookies, semantic page text and qualification attestations
//! cannot enroll or identify an account. This module deliberately supplies no
//! service detector: a trusted actor must supply both the service's stable account
//! identity and a collector that can independently establish the current account.

use std::cell::Cell;
use zephium_agent_controller::{AgentWorkAccountSource, AgentWorkFailure};
use zephium_agentic::{
    AgentAccountId, AgentAccountScope, AgentContextAccountBinding, ContextIdentity, ContextJoin,
    ContextKind, FrameId, SemanticOrigin,
};

use crate::AgentWorkProfileBinding;

/// Approval of one service account in the actor-selected browser session.
/// This is intent, not evidence of login. `account` must be the stable identity
/// of the explicitly enrolled service account (or its established host mapping),
/// never a new random ID standing in for an unknown signed-in account.
#[derive(Clone)]
pub struct AgentWorkAccountEnrollment {
    profile: AgentWorkProfileBinding,
    origin: SemanticOrigin,
    account: AgentAccountId,
}

impl AgentWorkAccountEnrollment {
    pub fn new(
        profile: AgentWorkProfileBinding,
        origin: SemanticOrigin,
        account: AgentAccountId,
    ) -> Self {
        Self {
            profile,
            origin,
            account,
        }
    }

    pub fn profile(&self) -> AgentWorkProfileBinding {
        self.profile
    }
    pub fn origin(&self) -> &SemanticOrigin {
        &self.origin
    }
    pub fn account(&self) -> AgentAccountId {
        self.account
    }

    /// Transfers the independent host collector to this single-run source.
    /// No account sample is minted by enrollment, binding or polling.
    pub fn with_collector(
        self,
        collector: Box<dyn AgentWorkAccountCollector>,
    ) -> AgentWorkEnrolledAccount {
        AgentWorkEnrolledAccount {
            enrollment: self,
            collector,
            identity: Cell::new(None),
            previous: Cell::new(None),
            refusal: Cell::new(None),
        }
    }
}

impl std::fmt::Debug for AgentWorkAccountEnrollment {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("AgentWorkAccountEnrollment([actor-approved, redacted])")
    }
}

/// Original service identity facts, independently bound to an exact native
/// document. Collection time and sample ID belong to the collector; callers
/// must never rebase a cached sample onto a successor document or a new time.
pub struct AgentWorkCollectedAccount {
    pub origin: SemanticOrigin,
    pub binding: AgentContextAccountBinding,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentWorkAccountFailure {
    /// No independently collected facts for this document yet.
    Missing,
    /// The host collector cannot establish the current signed-in identity.
    Unavailable,
    /// Logout, account substitution, or a different enrolled service/session.
    IdentityChanged,
    /// Context substitution, replay with changed contents, or clock regression.
    InvalidEvidence,
}

/// Trusted, bounded, nonblocking host collection boundary. Implementations must
/// independently establish identity in the exact native document/session, and
/// return the original sample, including its original ID and collection time.
/// Collection time must use the run's monotonic Work clock domain (on macOS,
/// `zephium_engine::work_browser_monotonic_now`), never wall-clock time.
/// `context` is a lookup key, never permission to attach cached account facts to
/// that document. Return Missing/Unavailable if no such evidence exists.
///
/// For Notion, a user must first explicitly enroll the actual stable service
/// account identity and origin in the selected test profile. A host collector
/// must then independently verify that account in each retained document. The
/// current WKWebView profile and cookie primitives do not provide this proof;
/// absent such a collector, authenticated work must remain unavailable.
pub trait AgentWorkAccountCollector: Send {
    fn collect(
        &self,
        context: ContextJoin,
    ) -> Result<AgentWorkCollectedAccount, AgentWorkAccountFailure>;
}

/// Move-only, single-run account source. A detected identity change permanently
/// refuses the source, including if the original account subsequently returns.
/// The session separately enforces freshness and bounded attestation history.
pub struct AgentWorkEnrolledAccount {
    enrollment: AgentWorkAccountEnrollment,
    collector: Box<dyn AgentWorkAccountCollector>,
    identity: Cell<Option<ContextIdentity>>,
    previous: Cell<Option<AgentContextAccountBinding>>,
    refusal: Cell<Option<AgentWorkAccountFailure>>,
}

impl AgentWorkEnrolledAccount {
    pub fn enrollment(&self) -> &AgentWorkAccountEnrollment {
        &self.enrollment
    }

    pub fn sample_account(
        &self,
        context: ContextJoin,
    ) -> Result<AgentContextAccountBinding, AgentWorkAccountFailure> {
        use AgentWorkAccountFailure::{IdentityChanged, InvalidEvidence};
        if let Some(reason) = self.refusal.get() {
            return Err(reason);
        }
        if context.identity().profile() != self.enrollment.profile.profile()
            || context.identity().kind() != ContextKind::Owned
            || context.frame() != FrameId::MAIN
            || self
                .identity
                .get()
                .is_some_and(|identity| identity != context.identity())
        {
            self.refusal.set(Some(InvalidEvidence));
            return Err(InvalidEvidence);
        }
        self.identity.set(Some(context.identity()));
        let sample = match self.collector.collect(context) {
            Ok(sample) => sample,
            Err(error) => {
                if matches!(error, IdentityChanged | InvalidEvidence) {
                    self.refusal.set(Some(error));
                }
                return Err(error);
            }
        };
        if sample.origin != self.enrollment.origin
            || sample.binding.account() != AgentAccountScope::Authenticated(self.enrollment.account)
        {
            self.refusal.set(Some(IdentityChanged));
            return Err(IdentityChanged);
        }
        if sample.binding.context() != context
            || self.previous.get().is_some_and(|previous| {
                let prior = previous.context();
                sample.binding.observed_at() < previous.observed_at()
                    || (sample.binding.attestation() == previous.attestation()
                        && sample.binding != previous)
                    || (prior != context
                        && (prior.context_generation() != context.context_generation()
                            || prior.cancellation_generation()
                                != context.cancellation_generation()
                            || prior.navigation_epoch().get().checked_add(1)
                                != Some(context.navigation_epoch().get())
                            || prior.frame_generation().get().checked_add(1)
                                != Some(context.frame_generation().get())))
            })
        {
            self.refusal.set(Some(InvalidEvidence));
            return Err(InvalidEvidence);
        }
        self.previous.set(Some(sample.binding));
        Ok(sample.binding)
    }
}

impl AgentWorkAccountSource for AgentWorkEnrolledAccount {
    fn sample(&self, context: ContextJoin) -> Result<AgentContextAccountBinding, AgentWorkFailure> {
        self.sample_account(context)
            .map_err(|_| crate::contract_failure!())
    }
}

impl std::fmt::Debug for AgentWorkEnrolledAccount {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("AgentWorkEnrolledAccount([owned, redacted])")
    }
}

#[cfg(test)]
mod tests;
