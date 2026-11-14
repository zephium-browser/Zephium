//! Chrome extension semantics shared by every engine adapter: who is
//! calling, what they may do, the bridge wire format and its limits, and
//! the API dispatch pipeline.
//!
//! Engine adapters supply host facts, bytes and time. Services are synchronous;
//! the SQLite storage backend performs bounded durable transactions.

#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![deny(clippy::dbg_macro, clippy::print_stderr, clippy::print_stdout)]
#![cfg_attr(
    not(test),
    deny(clippy::panic, clippy::unreachable, clippy::unwrap_used)
)]

pub mod alarms;
pub mod api;
pub mod context;
pub mod dispatch;
pub mod envelope;
pub mod errors;
pub mod events;
pub mod i18n;
pub mod lifecycle;
pub mod limits;
pub mod pattern;
pub mod permissions;
pub mod ports;
pub mod scripting_registry;
pub mod storage;

pub use context::{
    Authority, BindError, ClaimedWorld, CommittedDocument, ContextKey, ContextKind, ContextMask,
    DocumentGeneration, ExtensionId, ExtensionScheme, ExtensionView, FrameId, InjectedWorld,
    InstallAdmission, Sender, TabDocument, ViewKind,
};
pub use dispatch::{ApiTable, Dispatcher, Handled, Inbound, Invocation, MethodSpec, Responder};
pub use errors::{ApiError, ApiErrorKind};
pub use permissions::{Gate, InstallPermissions, RevocationEffect, Target};
