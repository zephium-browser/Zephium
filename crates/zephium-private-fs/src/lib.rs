//! Small, fail-closed filesystem primitives for browser-owned private data.
//!
//! This crate owns the platform-specific boundary checks shared by durable
//! browser subsystems. It deliberately does not own any subsystem's journal,
//! recovery, garbage-collection, or package-admission policy.
//!
//! Phase-one namespace activation is available only on macOS and Linux, where
//! child operations are descriptor-relative. Linux additionally requires a
//! readable `/proc/self/fd` so exact spelling can be proven from live file
//! descriptors even on case-folding filesystems. Missing or restricted procfs
//! fails closed. Other Unix targets and Windows fail closed until their native
//! adapters have passed dedicated live tests. Windows has an explicit
//! `windows-namespace-validation` feature for debug validation on local NTFS;
//! it is disabled by default and forbidden in optimized shipping builds.
//!
//! The boundary excludes unprivileged operating-system principals that have no
//! delegated access and rejects untrusted package contents. Root/administrator,
//! malicious code already running with the same user authority, and mutation
//! rights delegated through an accepted ancestor ACL are outside the guarantee;
//! the lease lock only serializes cooperating Zephium processes. Exact identity
//! checks detect observed path redirection and stickily quarantine the lease.

#![deny(missing_docs)]
#![deny(unsafe_code)]

#[cfg(all(feature = "windows-namespace-validation", not(debug_assertions)))]
compile_error!("Windows namespace validation must not enter an optimized shipping build");
#[cfg(all(feature = "windows-work-test-fixtures", not(debug_assertions)))]
compile_error!("Windows Work test fixtures must not enter an optimized shipping build");

mod component;
mod entry_name;
mod error;
mod identity;
mod lease;
mod namespace;
mod platform;
mod streaming;
mod transition;

pub use component::{PrivateComponent, PrivateComponentError};
pub use entry_name::{PrivateEntryName, PrivateEntryNameError, MAX_PRIVATE_ENTRY_NAME_BYTES};
pub use error::PrivateFsError;
pub use identity::{DirectoryIdentity, FileIdentity};
pub use namespace::{
    ByteLimit, LockedPrivateNamespace, OpenedPrivateDirectory, PrivateChildKind, PrivateDirectory,
    SealedPrivateDirectory, TreeRemovalLimits, TreeRemovalReport, MAX_TREE_REMOVAL_DEPTH,
    MAX_TREE_REMOVAL_ENTRIES,
};
#[cfg(all(target_os = "windows", feature = "windows-work-test-fixtures"))]
pub use platform::NativeWorkStorageTestSession;
#[cfg(target_os = "windows")]
pub use platform::{
    NativeApplication, NativeSession, NativeStorageAnchor, NativeStorageFile,
    NativeWorkStorageAnchor, NativeWorkStorageFile,
};
pub use streaming::{StreamingFileLength, StreamingWriteError, MAX_STREAMING_FILE_BYTES};
pub use transition::PrivateFsTransitionError;

/// Hard ceiling for any allocation or write accepted by the in-memory API.
///
/// Larger artifacts must use a separately reviewed streaming interface. This
/// prevents a caller-provided limit from silently becoming an unbounded memory
/// allocation.
pub const MAX_IN_MEMORY_FILE_BYTES: usize = 128 * 1024 * 1024;

/// Moves a regular file without replacing any existing destination.
///
/// This ambient filesystem primitive grants no private-namespace authority.
/// The caller owns path validation and lifecycle coordination. Failure leaves
/// the source in place; a case-only same-file rename is permitted.
#[cfg(target_os = "windows")]
pub fn rename_file_no_replace(
    source: &std::path::Path,
    target: &std::path::Path,
) -> std::io::Result<()> {
    platform::rename_file_no_replace(source, target)
}
