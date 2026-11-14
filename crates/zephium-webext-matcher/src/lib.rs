//! Dependency-light Chrome match patterns shared by UI and web processes.
#![forbid(unsafe_code)]

pub mod injection;
pub mod pattern;
pub use pattern::*;
