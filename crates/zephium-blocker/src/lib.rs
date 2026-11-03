//! Bounded, deterministic compilation of untrusted adblock lists.
//!
//! This crate deliberately owns no network updater and exposes no adblock
//! resources. In particular, scriptlets and redirect resources are rejected
//! before either native artifact is built.

#![deny(unsafe_code)]
#![deny(missing_docs)]

mod builtin;
mod cache;
mod compiler;
mod conditions;
mod cosmetics;
mod limits;
mod priority;
mod report;
mod rules;
mod scriptlets;
mod sha256;
#[cfg(feature = "webkit")]
mod webkit;
#[cfg(feature = "webkit")]
mod webkit_prune;
mod worker;

pub use cache::{CompiledArtifactCacheConfig, CompiledArtifactCacheConfigError};
pub use compiler::{
    official_source_trusts_scriptlets, CompileError, CompileTarget, Compiler, FilterSource,
    SourceFormat, SourceId, SourceIdError,
};
pub use conditions::ListConditions;
pub use cosmetics::{
    prepare_site_preferences, validate_personal_selector, CosmeticError, CosmeticPolicy,
    CosmeticReport,
};
pub use limits::{CompileLimitValues, CompileLimits, LimitConfigurationError};
pub use report::{
    CompilationReport, InputDropCount, InputDropReason, RuntimeCoverage, SourceReport,
    WebKitCoverage, WebKitDropCount, WebKitDropReason,
};
pub use rules::{
    ArtifactDigest, CompiledRules, MatchError, NetworkAction, NetworkDecision, NetworkRequest,
    PolicyDigest, RequestMethod, ResourceType, WebKitRules,
};
#[doc(hidden)]
pub use sha256::Sha256;
#[cfg(feature = "webkit")]
pub use webkit_prune::webkit_rule_digest;
pub use worker::{
    CatalogError, CatalogPreparationOutcome, CatalogReplacementDispatch, PolicyCatalog,
    PolicySource, StaticPolicyCatalog, WorkerBlocker,
};
pub use zephium_core::ports::blocker::BlockerCompileFailure;

/// Version of Zephium's canonical input and native-artifact format.
///
/// Increment this whenever the accepted rule subset, canonicalization, digest
/// construction, or WebKit serialization changes.
pub const POLICY_FORMAT_VERSION: u32 = 6;

/// Exact canonical WebKit JSON schema owned by Zephium.
pub use zephium_core::blocker::WEBKIT_ARTIFACT_FORMAT_VERSION;

/// Exact `adblock-rust` version compiled into this crate.
pub const ADBLOCK_ENGINE_VERSION: &str = "0.13.2";

/// Stable fingerprint of the exact compiler policy that can deterministically
/// reject an authenticated package.
///
/// The updater records this with a rejected package so a later binary whose
/// parser, native target, limits, or embedded engine changed can safely retry
/// the same signed repository revision without weakening rollback protection.
pub fn compiler_policy_fingerprint() -> [u8; 32] {
    let values = CompileLimitValues::default();
    let mut digest = Sha256::new();
    digest.update(b"zephium-blocker-compiler-policy-v1");
    digest.update(POLICY_FORMAT_VERSION.to_be_bytes());
    digest.update(WEBKIT_ARTIFACT_FORMAT_VERSION.to_be_bytes());
    digest.update(ADBLOCK_ENGINE_VERSION.as_bytes());
    digest.update(std::env::consts::OS.as_bytes());
    digest.update(std::env::consts::ARCH.as_bytes());
    digest.update([u8::from(cfg!(feature = "runtime"))]);
    digest.update([u8::from(cfg!(feature = "runtime-exact"))]);
    digest.update([u8::from(cfg!(feature = "webkit"))]);
    if cfg!(feature = "webkit") {
        digest.update(builtin::fingerprint());
    }
    #[cfg(feature = "webkit")]
    digest.update(webkit_prune::PrunedRules::shipped().fingerprint());
    for value in [
        values.max_sources,
        values.max_source_bytes,
        values.max_total_source_bytes,
        values.max_line_bytes,
        values.max_rules,
        values.max_physical_lines,
        values.max_webkit_rules,
        values.max_webkit_json_bytes,
        values.max_request_url_bytes,
        values.max_source_url_bytes,
    ] {
        digest.update((value as u64).to_be_bytes());
    }
    digest.finalize()
}

#[cfg(test)]
mod tests;
