//! Markdown notes stored as files the person owns.
#![forbid(unsafe_code)]

mod folder;
mod index;
pub mod legacy;
mod library;
mod markdown;
mod names;
mod service;

pub use library::vault_root;
pub use service::{Host, NoteService};
