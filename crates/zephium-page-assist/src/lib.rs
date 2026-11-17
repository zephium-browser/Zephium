//! The assistant's pure core: how pages are framed for the model, how streamed text
//! is cited and paced, what a conversation holds, and which page styles and
//! steps are admitted.
//!
//! No I/O, clocks, threads, async runtimes or logging. Time arrives as
//! milliseconds from the caller; page text arrives already read and is only
//! ever treated as data.

#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![deny(clippy::dbg_macro, clippy::print_stderr, clippy::print_stdout)]
#![cfg_attr(
    not(test),
    deny(clippy::panic, clippy::unreachable, clippy::unwrap_used)
)]

pub mod actions;
pub mod citations;
pub mod conversation;
pub mod cost;
pub mod css;
#[cfg(test)]
mod fixtures;
pub mod guard;
pub mod outline;
pub mod pacer;
pub mod page;
pub mod policy;
pub mod prompt;
pub mod refs;
pub mod stream;
pub mod suggest;
pub mod text;
pub mod voice;

pub use zephium_ipc::page_assist as ipc;
