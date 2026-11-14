//! The UI-process side of the extension message bridge.
//!
//! A [`ProfileBridge`] configures one profile's `WebContext` to load the
//! web-process module, attaches views to it, and turns what the module
//! reports into [`Sender`](zephium_webext_runtime::Sender)s through the
//! profile's [`Authority`](zephium_webext_runtime::Authority):
//!
//! - which `WebView` emitted a message fixes the profile and the tab or
//!   extension-owned view; that is the only identity the bridge trusts;
//! - the frame id, document sequence, world and URI in a post are renderer
//!   claims, accepted only when consistent with the host's own frame table,
//!   attachment generation and committed main document, and with the worlds
//!   and origins the host granted that view;
//! - inconsistent claims are dropped and counted ([`BridgeStats`]).
//!
//! Admitted envelopes go to a [`Runtime`]; [`DispatchRuntime`] hands them to
//! the shared `Dispatcher`, which enforces the authoritative limits.

mod buffer;
mod claims;
mod core;
mod frames;
mod host;
mod plan;
mod policy;
mod pull;
mod runtime;
mod scripts;

pub use self::buffer::{
    BUFFER_TIMEOUT_MS, MAX_BUFFERED_BYTES_PER_PROFILE, MAX_BUFFERED_BYTES_PER_VIEW,
    MAX_BUFFERED_POSTS_PER_VIEW,
};
pub use self::claims::Refusal;
pub use self::core::{
    BridgeStats, InjectionError, RegisterError, MAX_CONTEXTS, MAX_INFLIGHT_PER_CONTEXT,
    MAX_INFLIGHT_PER_VIEW, MAX_QUEUED_BYTES_PER_CONTEXT, MAX_QUEUED_BYTES_PER_PROFILE,
    MAX_QUEUED_FRAMES_PER_CONTEXT, MAX_VIEWS,
};
pub use self::frames::MAX_FRAMES_PER_VIEW;
pub use self::host::{
    BridgeConfig, BridgeError, Busy, ProfileBridge, Reattach, ShutdownDrain, Transport,
};
pub use self::plan::{world_name, AttachPlan, ExtensionPages, PlanError, WorldGrant};
pub use self::policy::{PolicyError, SealedPolicy};
pub use self::pull::{PULL_MAX_INTERVAL, PULL_MIN_INTERVAL};
pub use self::runtime::{
    BridgeViewId, CloseReason, DeliverError, DispatchRuntime, FrameLifecycle, Outbox, PortTraffic,
    Ports, Runtime, ViewRef,
};
pub use self::scripts::{ScriptTableError, SealedScripts};

/// Identifies a module event awaiting its reply.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct ReplyToken(pub u64);

#[cfg(test)]
mod tests;

/// Shared native injection and matcher contracts.
pub use zephium_webkit_web_process_protocol::protocol::{ExternalRule, MatchRule, ScriptTable};
