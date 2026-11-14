//! WebKitGTK web-process transport. Install the Linux cdylib in
//! `$libdir/zephium/web-process/` and mount that directory read-only in WebKit.
//! The UI must authenticate and authorize every renderer claim independently.

#![deny(unsafe_op_in_unsafe_fn)]

#[cfg(target_os = "linux")]
mod ffi;
pub mod state;

pub use zephium_webkit_web_process_protocol::{policy, protocol};
