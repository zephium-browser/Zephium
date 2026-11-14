//! Wire contract between the UI process and the WebKitGTK web-process module:
//! the bounded UserMessage codec and the request-policy snapshot format.
//! Both sides link this crate, so neither re-implements the other's codec.

#![forbid(unsafe_code)]

pub mod policy;
pub mod protocol;
