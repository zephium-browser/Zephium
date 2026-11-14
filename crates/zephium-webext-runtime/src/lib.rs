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

pub mod errors;
pub mod pattern;

pub use errors::{ApiError, ApiErrorKind};
