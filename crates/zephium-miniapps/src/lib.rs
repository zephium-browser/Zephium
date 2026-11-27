//! Installable mini web apps for Zephium.
//!
//! The crate owns the parts that must be identical on every platform: manifest
//! admission, scope policy, stable identity, and a small bounded registry. It
//! deliberately does not own windows or rendering. The desktop adapter chooses
//! the platform presentation and keeps each installed app's durable identity
//! and navigation scope consistent across platforms.

mod discovery;
mod error;
mod manifest;
mod model;
mod policy;
mod registry;

pub use discovery::{manifest_href, site_metadata, SiteMetadata};
pub use error::MiniAppError;
pub use manifest::{parse_manifest, MAX_MANIFEST_BYTES};
pub use model::{DisplayMode, MiniApp};
pub use policy::{app_id, is_in_scope, is_public_host, is_public_ip, same_origin};
pub use registry::{Registry, MAX_INSTALLED_APPS, MAX_REGISTRY_BYTES, REGISTRY_VERSION};
