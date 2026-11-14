//! Chrome extension semantics shared by every engine adapter: who is
//! calling, what they may do, the bridge wire format and its limits, and
//! the API dispatch pipeline.
//!
//! No I/O, threads or async runtime. Engine adapters supply host facts,
//! bytes and time; this crate decides.

#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![deny(clippy::dbg_macro, clippy::print_stderr, clippy::print_stdout)]
#![cfg_attr(
    not(test),
    deny(clippy::panic, clippy::unreachable, clippy::unwrap_used)
)]

pub mod api;
pub mod context;
pub mod dispatch;
pub mod envelope;
pub mod errors;
pub mod limits;
pub mod pattern;
pub mod permissions;

pub use context::{
    Authority, BindError, ClaimedWorld, CommittedDocument, ContextKey, ContextKind, ContextMask,
    DocumentGeneration, ExtensionId, ExtensionScheme, ExtensionView, FrameId, InjectedWorld,
    InstallAdmission, Sender, TabDocument, ViewKind,
};
pub use dispatch::{ApiTable, Dispatcher, Handled, Inbound, Invocation, MethodSpec, Responder};
pub use errors::{ApiError, ApiErrorKind};
pub use permissions::{Gate, InstallPermissions, RevocationEffect, Target};
