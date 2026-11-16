//! Bounded provider and policy-authorized semantic browser controllers.
//!
//! The shipping text-only runtime controller and the reusable locate/act session
//! share existing functional-core authorities. The session does not by itself
//! prove durable audit or application/native lifecycle closure. Its host must
//! retain terminal/recovery ownership until those independent duties settle.

#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![deny(clippy::dbg_macro, clippy::print_stderr, clippy::print_stdout)]
#![cfg_attr(
    not(test),
    deny(clippy::panic, clippy::unreachable, clippy::unwrap_used)
)]

/// Where the last contract refusal was raised: a source location, never
/// page or model content, so a failed page task can name its cause.
static CONTRACT_SITE: std::sync::Mutex<&'static str> = std::sync::Mutex::new("");

/// The source location of the most recent contract refusal.
pub fn last_contract_site() -> &'static str {
    *CONTRACT_SITE.lock().unwrap_or_else(|e| e.into_inner())
}

#[doc(hidden)]
pub fn mark_contract_site(site: &'static str) {
    *CONTRACT_SITE.lock().unwrap_or_else(|e| e.into_inner()) = site;
}

/// `AgentWorkFailure::Contract`, remembering where it was raised.
#[cfg(feature = "provider-transport")]
macro_rules! contract {
    () => {{
        $crate::mark_contract_site(concat!(file!(), ":", line!()));
        $crate::AgentWorkFailure::Contract
    }};
}
#[cfg(feature = "provider-transport")]
pub(crate) use contract;
#[cfg(feature = "provider-transport")]
macro_rules! continuation {
    () => {{
        $crate::mark_contract_site(concat!(file!(), ":", line!()));
        $crate::AgentBrowserProviderError::Continuation
    }};
}
#[cfg(feature = "provider-transport")]
pub(crate) use continuation;

#[cfg(all(feature = "probe-harness", not(debug_assertions)))]
compile_error!("the agent controller probe harness is forbidden in optimized builds");

#[cfg(feature = "provider-transport")]
mod terra;

#[cfg(feature = "provider-transport")]
mod action;

#[cfg(feature = "provider-transport")]
mod work_reinspection;
#[cfg(feature = "provider-transport")]
pub use work_reinspection::{
    AgentWorkEffectObservedState, AgentWorkEffectReadTarget, AgentWorkEffectReinspection,
    AgentWorkEffectReinspectionError, AgentWorkEffectReinspectionRefusal,
    AgentWorkEffectReobservation,
};

#[cfg(feature = "provider-transport")]
mod work_form;
#[cfg(feature = "provider-transport")]
pub use work_form::{
    AgentWorkFormExtractionTask, AgentWorkFormGoal, AgentWorkFormPhase, AgentWorkFormTask,
};
#[cfg(feature = "provider-transport")]
mod work_discovery;
#[cfg(feature = "provider-transport")]
pub use work_discovery::{
    AgentWorkAccountSource, AgentWorkDiscoveryTask, AgentWorkLocalActionPolicy,
};

#[cfg(feature = "provider-transport")]
pub use action::{
    AgentBrowserAction, AgentBrowserActionError, AgentBrowserActionFinalizationRefusal,
    AgentBrowserActionProposal, AgentBrowserVerifiedTransition,
};

#[cfg(feature = "provider-transport")]
pub use terra::{
    AgentBrowserAccountError, AgentBrowserDecisionProvider, AgentBrowserModel,
    AgentBrowserModelAccess, AgentBrowserProviderError, AgentBrowserProviderTurn,
    AgentBrowserRetention, AgentBrowserSession, AgentBrowserSessionFinishRefusal,
    AgentBrowserSessionTerminal, AgentWorkClosedUnsuccessfully, AgentWorkContextSpec,
    AgentWorkController, AgentWorkDecisionRoute, AgentWorkEvent, AgentWorkEventKind,
    AgentWorkExtractionTask, AgentWorkFailure, AgentWorkHandle, AgentWorkHumanRequest,
    AgentWorkInitialReadiness, AgentWorkOutcome, AgentWorkRecovery, AgentWorkRetainedBrowser,
    AgentWorkRetainedController, AgentWorkRetainedHandle, AgentWorkRetainedOutcome,
    AgentWorkRetainedRecovery, AgentWorkRetainedResourceSpec, AgentWorkRunInput,
    AgentWorkRunSettings, AgentWorkSuccess, AgentWorkTask, AgentWorkTaskProgress,
    AgentWorkWaitingForHuman, MAX_AGENT_WORK_EVENTS,
};

#[cfg(feature = "probe-harness")]
mod probe;

#[cfg(feature = "provider-transport")]
pub use terra::{
    TerraControllerClock, TerraControllerClockError, TerraControllerCompletion,
    TerraControllerCompletionState, TerraControllerConstructionError, TerraControllerIds,
    TerraControllerRecovery, TerraControllerRunInput, TerraControllerSuccess,
    TerraControllerTurnInput, TerraTextOnlyController,
};

#[cfg(feature = "probe-harness")]
pub use probe::{
    TerraProbeActionBridge, TerraProbeActionBridgeError, TerraProbeActionReport,
    TerraProbeVerifiedTransition,
};

#[cfg(feature = "probe-harness")]
pub use terra::{
    run_initial_terra_probe, AgentBrowserModel as AgenticProbeModel,
    AgentBrowserProviderError as TerraProbeProviderError,
    AgentBrowserProviderTurn as TerraProbeProviderTurn,
    AgentBrowserRetention as AgenticProbeRetention, AgentBrowserSession as TerraProbeSession,
};
