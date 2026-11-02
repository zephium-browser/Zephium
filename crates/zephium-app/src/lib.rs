//! The actor shell around the pure core. One thread owns all state; commands
//! enter through a queue (UI intents and engine events alike), effects leave
//! through ports, projections go to the UI.

mod diagnostics;

#[cfg(all(test, feature = "work-execution-probe"))]
mod work_provider_fixture;

#[cfg(all(test, feature = "work-execution"))]
static WORK_RUNTIME_TEST_SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

macro_rules! diagnostic {
    ($($argument:tt)*) => {{
        crate::diagnostics::write(format_args!($($argument)*));
    }};
}

pub(crate) use diagnostic;

/// `AgentWorkFailure::Contract`, remembering where it was raised so a failed
/// page task names its cause in the log.
#[allow(unused_macros)]
macro_rules! contract_failure {
    () => {{
        #[cfg(feature = "work-execution")]
        zephium_agent_controller::mark_contract_site(concat!(file!(), ":", line!()));
        AgentWorkFailure::Contract
    }};
}
#[allow(unused_imports)]
pub(crate) use contract_failure;

mod actor;
mod api;
mod lookup;
mod onboarding;
mod reply;
mod shell;
mod store_reads;
#[cfg(feature = "work-execution")]
mod work;
#[cfg(feature = "work-execution")]
mod work_account;
#[cfg(feature = "work-execution")]
mod work_profile;
#[cfg(feature = "work-execution")]
pub use work_account::{
    AgentWorkAccountCollector, AgentWorkAccountEnrollment, AgentWorkAccountFailure,
    AgentWorkCollectedAccount, AgentWorkEnrolledAccount,
};
#[cfg(feature = "work-execution")]
pub use work_profile::{
    AgentWorkProfileBinding, AgentWorkProfileReadiness, AgentWorkProfileRequest,
};
#[cfg(feature = "agentic-browser")]
mod work_resources;

#[cfg(feature = "work-execution")]
pub use work_resources::product::{
    PreparedRetainedContinuation, PreparedRetainedWork, RetainedHumanPhase, RetainedHumanResume,
    RetainedHumanSnapshot, RetainedLaneFacts, RetainedPageAdmission, RetainedRefusal,
    RetainedWorkHandle, RetainedWorkNativeFactory, RetainedWorkPhase, RetainedWorkPorts,
    RetainedWorkSnapshot,
};

#[cfg(feature = "work-execution-probe")]
#[doc(hidden)]
pub use work_resources::probe as retained_work_probe;

#[cfg(feature = "work-execution")]
pub use work::{
    AgentWorkApplicationConfig, AgentWorkApplicationHandle, AgentWorkApplicationPhase,
    AgentWorkApplicationPorts, AgentWorkApplicationSnapshot, AgentWorkNativeFactory,
    AgentWorkReviewDecision, PreparedAgentWork,
};

pub use actor::{
    spawn, spawn_suspended, CallbackHandle, ContentPolicyStatusRequest,
    FocusedContentPolicyStatusRequest, Handle, ShutdownRequest, SpawnError, SpawnFailure,
};
#[cfg(feature = "agentic-browser")]
pub use actor::{spawn_agentic, spawn_agentic_suspended, AgenticSpawnFailure};
#[cfg(feature = "agentic-browser")]
pub use api::AgentLifecycle;
pub use api::{
    BrowserPage, ChromePresentation, ChromePresentationCallback, ChromePresentationDispatch,
    Command, ContentPolicyStatusQueryOutcome, EmitFn, PagePermissionPromptDecision,
    PageRequestDecision, PresentationChrome, SharedBlocker, SharedChrome, SharedEngine,
    SharedStore, ShellTerminalFailure, ShellTerminalFailureCallback, ShutdownOutcome, TabMetadata,
    WorkPaneTarget,
};
pub use lookup::{NoteMatch, TaskMatch};
pub use onboarding::{finish_onboarding, onboarding_due};
#[cfg(feature = "work-planning")]
pub mod work_context;
pub use shell::{Shell, WebExtensionStatus, WebExtensionTarget};

#[doc(hidden)]
pub use store_reads::{ImportWork, ImportedSite, StoreReadResult};

mod work_authoring;
mod work_authoring_intent;
pub use work_authoring::{WorkDocumentProjection, WorkDocumentRequest, WorkDocumentSubmission};
pub use work_authoring_intent::{WorkIntent, WorkUserEdit};

#[cfg(feature = "work-planning")]
pub mod voice;
#[cfg(feature = "work-runtime")]
pub mod work_agent;
#[cfg(feature = "work-runtime")]
mod work_changes;
#[cfg(feature = "work-runtime")]
pub mod work_commands;
#[cfg(feature = "work-runtime")]
pub mod work_computer;
#[cfg(feature = "work-runtime")]
pub mod work_connections;
#[cfg(feature = "work-runtime")]
pub mod work_coordination;
#[cfg(feature = "work-runtime")]
pub mod work_execution;
#[cfg(feature = "work-runtime")]
pub mod work_files;
#[cfg(feature = "work-runtime")]
pub mod work_lead;
#[cfg(feature = "work-planning")]
pub mod work_models;
#[cfg(feature = "work-runtime")]
pub mod work_personal;
#[cfg(feature = "work-planning")]
pub mod work_planning;
#[cfg(feature = "work-runtime")]
pub mod work_runtime;
#[cfg(feature = "work-planning")]
pub mod work_screenshots;
#[cfg(feature = "work-runtime")]
mod work_search;
#[cfg(feature = "work-runtime")]
pub mod work_sites;
#[cfg(feature = "work-runtime")]
mod work_synthesis;
#[cfg(any(feature = "work-execution", feature = "work-runtime"))]
pub mod work_trace;
pub use api::{
    BookmarkCompletion, BrowsingDataCompletion, ExportCompletion, FaviconPageSource, FaviconProbe,
    FaviconProber, FaviconProberAttachment, HistoryCompletion, HistoryListCompletion,
    ImportCompletion, ImportDestination, ImportDestinationCompletion, NoteCompletion,
    NotesAttachment, PageSource, ResourceCompletion, SourceCompletion, SpaceCommand, TabAction,
    TabCompletion, TimeCompletion, ZoomCompletion,
};
