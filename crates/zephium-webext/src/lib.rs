//! Platform-neutral handling of Chrome extension packages: CRX3 verification,
//! safe extraction, manifest reading, install warnings, and compat-layer
//! preparation for the WebKit extension runtime.
#![forbid(unsafe_code)]

pub mod archive;
pub mod crx;
pub mod id;
pub mod limits;
pub mod manifest;
pub mod package;
pub mod permissions;
pub mod prepare;
pub mod store;
pub mod windows;

pub use id::ExtensionId;
