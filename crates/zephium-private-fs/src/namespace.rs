use std::fs::{self, File};
use std::io::{Read, Seek, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, MutexGuard};

use crate::identity::{DirectoryIdentity, FileIdentity};
use crate::lease::NamespaceLease;
use crate::platform::{self, DirectoryMode, OpenPurpose, RegularMode};
use crate::streaming::{copy_exact, ExactCopyError};
use crate::{
    PrivateComponent, PrivateEntryName, PrivateFsError, PrivateFsTransitionError,
    StreamingFileLength, StreamingWriteError, MAX_IN_MEMORY_FILE_BYTES,
};

mod tree_removal;
pub use tree_removal::{
    TreeRemovalLimits, TreeRemovalReport, MAX_TREE_REMOVAL_DEPTH, MAX_TREE_REMOVAL_ENTRIES,
};

const MAX_INVENTORY_ENTRIES: usize = 4_096;
const LOCK_COMPONENT_NAME: &str = ".zephium-private-fs-lock-v1";
const LOCK_STAGING_COMPONENT_NAME: &str = ".zephium-private-fs-lock-staging-v1";
const LOCK_FILE_CONTENT: &[u8] = b"zephium-private-fs\nlock-format=1\n";

#[inline]
fn write_all_private_regular(file: &mut File, bytes: &[u8]) -> Result<(), PrivateFsError> {
    file.write_all(bytes).map_err(|_| PrivateFsError::Io)
}

#[inline]
fn copy_exact_to_private_regular(
    source: &mut (impl Read + ?Sized),
    file: &mut File,
    expected: StreamingFileLength,
) -> Result<(), ExactCopyError> {
    copy_exact(source, file, expected)
}

#[inline]
fn sync_private_regular(file: &File) -> Result<(), PrivateFsError> {
    #[cfg(target_os = "windows")]
    platform::sync_regular(file)?;
    #[cfg(not(target_os = "windows"))]
    file.sync_all().map_err(|_| PrivateFsError::Io)?;
    Ok(())
}

#[inline]
fn sync_private_directory(file: &File) -> Result<(), PrivateFsError> {
    #[cfg(target_os = "windows")]
    platform::sync_directory(file)?;
    #[cfg(not(target_os = "windows"))]
    file.sync_all().map_err(|_| PrivateFsError::Io)?;
    Ok(())
}

/// Validated nonzero bound for a single in-memory file operation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ByteLimit(usize);

impl ByteLimit {
    /// Creates a limit within the crate-wide hard allocation ceiling.
    pub fn new(max_bytes: usize) -> Result<Self, PrivateFsError> {
        if max_bytes == 0 || max_bytes > MAX_IN_MEMORY_FILE_BYTES {
            return Err(PrivateFsError::BoundExceeded);
        }
        Ok(Self(max_bytes))
    }

    /// Returns the exact accepted byte count.
    #[must_use]
    pub const fn get(self) -> usize {
        self.0
    }
}

struct DirectoryCore {
    path: PathBuf,
    handle: File,
    identity: DirectoryIdentity,
    parent: Option<ParentAuthority>,
}

struct ParentAuthority {
    path: PathBuf,
    handle: File,
    identity: DirectoryIdentity,
    child_name: OwnedChildName,
    mode: DirectoryMode,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum DirectoryRole {
    Root,
    Child,
}

#[derive(Clone, Copy)]
enum ChildName<'a> {
    Control(&'a PrivateComponent),
    Entry(&'a PrivateEntryName),
}

impl<'a> ChildName<'a> {
    fn as_str(self) -> &'a str {
        match self {
            Self::Control(component) => component.as_str(),
            Self::Entry(name) => name.as_str(),
        }
    }

    fn to_owned_name(self) -> OwnedChildName {
        match self {
            Self::Control(component) => OwnedChildName::Control(component.clone()),
            Self::Entry(name) => OwnedChildName::Entry(name.clone()),
        }
    }
}

#[derive(Clone)]
enum OwnedChildName {
    Control(PrivateComponent),
    Entry(PrivateEntryName),
}

impl OwnedChildName {
    fn as_str(&self) -> &str {
        match self {
            Self::Control(component) => component.as_str(),
            Self::Entry(name) => name.as_str(),
        }
    }
}

#[derive(Clone, Copy)]
enum ChildDirectoryMode {
    OpenExisting,
    CreateOrOpen,
    CreateNew,
}

/// Exact kind and opaque identity of one inspected private child entry.
///
/// This is an observation, not an open file or directory capability. The
/// parent lease remains responsible for serializing later operations, and a
/// caller must not treat an identity as proof that the entry is unchanged
/// after this method returns.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PrivateChildKind {
    /// A verified single-link private regular file.
    RegularFile(FileIdentity),
    /// A verified private directory.
    Directory(DirectoryIdentity),
}

/// Held, identity-bound private directory.
///
/// Caller-provided paths never enter this API after admission. On supported
/// platforms, every child operation is resolved relative to the held directory
/// descriptor. Protocol internals accept only [`PrivateComponent`], while
/// authenticated payload entries accept only [`PrivateEntryName`]; no raw path
/// strings cross the public boundary. Every directory also retains the root
/// namespace lease, lock, quarantine state, and operation mutex, so a child
/// cannot outlive or bypass its authority.
pub struct PrivateDirectory {
    core: DirectoryCore,
    lease: Arc<NamespaceLease>,
    role: DirectoryRole,
}

/// Linear capability for one exact read-only private directory.
///
/// The capability owns a live directory descriptor and a privately captured
/// immediate-parent descriptor, path, identity, expected mode, and typed exact
/// child name. It is intentionally not cloneable. Publication, unsealing, and
/// removal consume it so an old source authority cannot be reused after a
/// successful transition.
pub struct SealedPrivateDirectory {
    core: DirectoryCore,
    lease: Arc<NamespaceLease>,
}

/// One exact child directory admitted in either private lifecycle mode.
///
/// This sum type is intended for bounded crash recovery, where an interrupted
/// bottom-up seal may legitimately leave different nodes at `0700` and
/// `0500`. Both variants retain the same parent-bound identity and namespace
/// lease guarantees as their mode-specific open operations. Hostile modes,
/// links, ACLs, aliases, and identity races are still rejected.
pub enum OpenedPrivateDirectory {
    /// Exact writable `0700` child authority.
    Writable(PrivateDirectory),
    /// Exact sealed `0500` child authority.
    Sealed(SealedPrivateDirectory),
}

impl OpenedPrivateDirectory {
    /// Returns the admitted child's opaque open-handle identity.
    #[must_use]
    pub const fn identity(&self) -> DirectoryIdentity {
        match self {
            Self::Writable(directory) => directory.identity(),
            Self::Sealed(directory) => directory.identity(),
        }
    }
}

impl SealedPrivateDirectory {
    /// Returns this sealed directory's opaque open-handle identity.
    #[must_use]
    pub const fn identity(&self) -> DirectoryIdentity {
        self.core.identity
    }

    /// Opens one exact sealed protocol child beneath this sealed directory.
    pub fn open_sealed_private_child(
        &self,
        component: &PrivateComponent,
    ) -> Result<Self, PrivateFsError> {
        self.open_sealed_child(ChildName::Control(component))
    }

    /// Opens one exact sealed case-preserving child beneath this directory.
    pub fn open_sealed_entry_child(&self, name: &PrivateEntryName) -> Result<Self, PrivateFsError> {
        self.open_sealed_child(ChildName::Entry(name))
    }

    fn open_sealed_child(&self, name: ChildName<'_>) -> Result<Self, PrivateFsError> {
        let _operation = self.lease.begin()?;
        self.precheck_unlocked()?;
        let child = self.lease.observe(open_sealed_child_unlocked(
            &self.core,
            &self.lease,
            DirectoryMode::Sealed,
            name,
        ))?;
        self.precheck_unlocked()?;
        Ok(child)
    }

    /// Lists a bounded byte-sorted snapshot of exact payload entry names.
    ///
    /// Enumeration validates only the portable exact name grammar. Callers
    /// must pass every returned name through [`Self::inspect_entry`] or the
    /// bounded regular-reader API before trusting its kind or content.
    pub fn list_entry_names(
        &self,
        max_entries: usize,
    ) -> Result<Vec<PrivateEntryName>, PrivateFsError> {
        let _operation = self.lease.begin()?;
        if max_entries == 0 || max_entries > MAX_INVENTORY_ENTRIES {
            return Err(PrivateFsError::BoundExceeded);
        }
        self.precheck_unlocked()?;
        let mut entries = platform::list_names(&self.core.handle, &self.core.path, max_entries)?
            .into_iter()
            .map(|name| PrivateEntryName::new(name).map_err(|_| PrivateFsError::Unsafe))
            .collect::<Result<Vec<_>, _>>()?;
        entries.sort_unstable();
        self.precheck_unlocked()?;
        Ok(entries)
    }

    /// Inspects one optional exact payload entry through sealed-node admission.
    ///
    /// A regular file must be exact mode `0400`; a directory must be exact
    /// mode `0500`. Links, special nodes, writable nodes, unsafe ACLs, case
    /// aliases, and identity races are rejected instead of projected as kinds.
    pub fn inspect_entry(
        &self,
        name: &PrivateEntryName,
    ) -> Result<Option<PrivateChildKind>, PrivateFsError> {
        let _operation = self.lease.begin()?;
        self.precheck_unlocked()?;
        let inspected = inspect_sealed_entry_unlocked(&self.core, &self.lease, name)?;
        self.precheck_unlocked()?;
        Ok(inspected)
    }

    /// Runs a callback with one exact `0400` bounded payload-file reader.
    ///
    /// The callback sees only a [`Read`] capability capped at the admitted
    /// initial length. Its result remains nested so caller validation failures
    /// are distinct from filesystem failures. Exact mode, length, identity,
    /// spelling, and the sealed directory boundary are revalidated afterward,
    /// even when the callback returns an error. The callback executes under the
    /// non-reentrant operation mutex and must not call back into this namespace.
    pub fn with_bounded_entry_regular_reader<T, E>(
        &self,
        name: &PrivateEntryName,
        limit: ByteLimit,
        callback: impl FnOnce(&mut dyn Read) -> Result<T, E>,
    ) -> Result<Option<Result<T, E>>, PrivateFsError> {
        let _operation = self.lease.begin()?;
        self.precheck_unlocked()?;
        with_bounded_sealed_regular_reader_unlocked(
            &self.core,
            &self.lease,
            DirectoryMode::Sealed,
            ChildName::Entry(name),
            limit,
            |reader, _| callback(reader),
        )
    }

    /// Reads a sealed regular file's bounded logical length without reading payload.
    /// Exact spelling, identity, read-only mode and unchanged length are checked.
    pub fn entry_regular_length(
        &self,
        name: &PrivateEntryName,
        limit: ByteLimit,
    ) -> Result<Option<u64>, PrivateFsError> {
        let _operation = self.lease.begin()?;
        self.precheck_unlocked()?;
        with_bounded_sealed_regular_reader_unlocked(
            &self.core,
            &self.lease,
            DirectoryMode::Sealed,
            ChildName::Entry(name),
            limit,
            |_, length| Ok::<_, std::convert::Infallible>(length),
        )
        .map(|value| {
            value.map(|value| match value {
                Ok(length) => length,
                Err(never) => match never {},
            })
        })
    }

    /// Runs a synchronous callback with this sealed capability's verified path.
    ///
    /// This has the same synchronous-only authority and namespace-wide mutation
    /// pin contract as [`PrivateDirectory::with_verified_path`]. The path must
    /// not be retained and treated as authority after the callback returns.
    pub fn with_verified_path<T>(
        &self,
        callback: impl FnOnce(&Path) -> T,
    ) -> Result<T, PrivateFsError> {
        with_verified_path_callback(&self.core, &self.lease, DirectoryMode::Sealed, callback)
    }

    /// Unseals this directory to exact writable mode `0700`.
    ///
    /// Unsealing is top-down: the privately bound immediate parent must already
    /// be writable. A capability issued while that parent was sealed becomes
    /// stale when the parent transitions and fails closed; reopen the child from
    /// the freshly unsealed parent before calling this method.
    pub fn unseal(
        self,
    ) -> Result<PrivateDirectory, PrivateFsTransitionError<SealedPrivateDirectory>> {
        let operation = match self.lease.begin() {
            Ok(operation) => operation,
            Err(error) => return Err(PrivateFsTransitionError::terminal(error)),
        };
        if let Err(error) = self.precheck_unlocked() {
            drop(operation);
            return Err(PrivateFsTransitionError::terminal(error));
        }
        if let Err(error) = self.lease.mutation_allowed() {
            drop(operation);
            return Err(PrivateFsTransitionError::recoverable(error, self));
        }
        if self
            .core
            .parent
            .as_ref()
            .is_none_or(|parent| parent.mode != DirectoryMode::Writable)
        {
            drop(operation);
            return Err(PrivateFsTransitionError::recoverable(
                PrivateFsError::Unsafe,
                self,
            ));
        }

        let chmod = platform::set_directory_mode(&self.core.handle, DirectoryMode::Writable);
        #[cfg(test)]
        let chmod = chmod.and_then(|()| {
            if self
                .lease
                .take_lifecycle_fault(crate::lease::LifecycleFault::UnsealDirectoryCommitted)
            {
                Err(PrivateFsError::Io)
            } else {
                Ok(())
            }
        });
        if let Err(error) = chmod {
            let unchanged = verify_core_boundary(&self.core, DirectoryMode::Sealed)
                .and_then(|()| self.lease.verify_authority());
            drop(operation);
            return if unchanged.is_ok() {
                Err(PrivateFsTransitionError::recoverable(error, self))
            } else {
                Err(PrivateFsTransitionError::terminal(
                    terminal_settlement_error(&self.lease),
                ))
            };
        }

        let settlement = (|| {
            platform::sync_directory(&self.core.handle)?;
            #[cfg(test)]
            if self
                .lease
                .take_lifecycle_fault(crate::lease::LifecycleFault::UnsealDirectorySelfSync)
            {
                return Err(PrivateFsError::Io);
            }
            let parent = self
                .core
                .parent
                .as_ref()
                .ok_or(PrivateFsError::IdentityAmbiguous)?;
            platform::sync_directory(&parent.handle)?;
            #[cfg(test)]
            if self
                .lease
                .take_lifecycle_fault(crate::lease::LifecycleFault::UnsealDirectoryParentSync)
            {
                return Err(PrivateFsError::Io);
            }
            verify_core_boundary(&self.core, DirectoryMode::Writable)?;
            self.lease.verify_authority()
        })();
        if settlement.is_err() {
            drop(operation);
            return Err(PrivateFsTransitionError::terminal(
                terminal_settlement_error(&self.lease),
            ));
        }
        drop(operation);
        let Self { core, lease } = self;
        Ok(PrivateDirectory {
            core,
            lease,
            role: DirectoryRole::Child,
        })
    }

    /// Publishes this sealed directory under a lowercase protocol name.
    ///
    /// Publication uses one descriptor-relative no-replace rename and returns a
    /// freshly opened destination capability. The source and destination
    /// parents must share the exact same namespace lease. Same-parent
    /// publication is the universal layout and flushes that parent exactly
    /// once. On macOS and Linux distinct-parent publication, the consumed root
    /// alone transitions through owner-only `0700` for the native rename. The
    /// root is immediately resealed and synced before
    /// any capability can be returned; descendants are never unsealed. The
    /// namespace operation lock and no-path-pins gate span that entire window.
    /// Linux additionally supports distinct parents and flushes both; macOS
    /// deliberately retains the narrower same-parent publication contract.
    pub fn publish_noreplace(
        self,
        destination_parent: &PrivateDirectory,
        destination: &PrivateComponent,
    ) -> Result<Self, PrivateFsTransitionError<Self>> {
        self.publish_noreplace_named(destination_parent, ChildName::Control(destination))
    }

    /// Publishes this sealed directory under one case-preserving payload name.
    pub fn publish_entry_noreplace(
        self,
        destination_parent: &PrivateDirectory,
        destination: &PrivateEntryName,
    ) -> Result<Self, PrivateFsTransitionError<Self>> {
        self.publish_noreplace_named(destination_parent, ChildName::Entry(destination))
    }

    fn publish_noreplace_named(
        self,
        destination_parent: &PrivateDirectory,
        destination: ChildName<'_>,
    ) -> Result<Self, PrivateFsTransitionError<Self>> {
        let operation = match self.lease.begin() {
            Ok(operation) => operation,
            Err(error) => return Err(PrivateFsTransitionError::terminal(error)),
        };
        if let Err(error) = self.precheck_unlocked() {
            drop(operation);
            return Err(PrivateFsTransitionError::terminal(error));
        }
        if let Err(error) = self.lease.mutation_allowed() {
            drop(operation);
            return Err(PrivateFsTransitionError::recoverable(error, self));
        }
        if !Arc::ptr_eq(&self.lease, &destination_parent.lease) {
            drop(operation);
            return Err(PrivateFsTransitionError::recoverable(
                PrivateFsError::NamespaceMismatch,
                self,
            ));
        }
        if let Err(error) = destination_parent.reject_reserved_name(destination) {
            drop(operation);
            return Err(PrivateFsTransitionError::recoverable(error, self));
        }
        if let Err(error) = destination_parent.verify_boundary_unlocked() {
            drop(operation);
            return Err(PrivateFsTransitionError::terminal(error));
        }
        let Some(source_parent) = self.core.parent.as_ref() else {
            drop(operation);
            return Err(PrivateFsTransitionError::recoverable(
                PrivateFsError::Unsafe,
                self,
            ));
        };
        let same_parent = source_parent.identity == destination_parent.core.identity;
        #[cfg(target_os = "macos")]
        if !same_parent {
            drop(operation);
            return Err(PrivateFsTransitionError::recoverable(
                PrivateFsError::PrimitiveUnavailable,
                self,
            ));
        }
        if same_parent && source_parent.child_name.as_str() == destination.as_str() {
            drop(operation);
            return Err(PrivateFsTransitionError::recoverable(
                PrivateFsError::Unsafe,
                self,
            ));
        }
        if sync_private_directory(&self.core.handle).is_err() {
            let unchanged = self.precheck_unlocked();
            drop(operation);
            return if unchanged.is_ok() {
                Err(PrivateFsTransitionError::recoverable(
                    PrivateFsError::Io,
                    self,
                ))
            } else {
                Err(PrivateFsTransitionError::terminal(
                    terminal_settlement_error(&self.lease),
                ))
            };
        }

        #[cfg(target_os = "macos")]
        let writable_root = true;
        #[cfg(target_os = "linux")]
        let writable_root = !same_parent;
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        if writable_root && self.prepare_publication_root_unlocked().is_err() {
            // A committed/ambiguous mode transition never returns the old
            // capability. Best-effort sealing is cleanup, not a rename retry.
            let _ = self.reseal_publication_root_unlocked();
            let error = terminal_settlement_error(&self.lease);
            drop(operation);
            return Err(PrivateFsTransitionError::terminal(error));
        }

        let rename = || {
            platform::atomic_publish_noreplace_between(
                &source_parent.handle,
                &source_parent.path,
                source_parent.child_name.as_str(),
                &destination_parent.core.handle,
                &destination_parent.core.path,
                destination.as_str(),
            )
        };
        #[cfg(all(test, any(target_os = "macos", target_os = "linux")))]
        let rename = if self
            .lease
            .take_lifecycle_fault(crate::lease::LifecycleFault::PublishRenameRefused)
        {
            Err(PrivateFsError::Io)
        } else {
            rename()
        };
        #[cfg(not(all(test, any(target_os = "macos", target_os = "linux"))))]
        let rename = rename();
        #[cfg(all(test, any(target_os = "macos", target_os = "linux")))]
        let rename = rename.and_then(|()| {
            if self
                .lease
                .take_lifecycle_fault(crate::lease::LifecycleFault::PublishRenameCommitted)
            {
                Err(PrivateFsError::Io)
            } else {
                Ok(())
            }
        });

        // Use the original descriptor, never either mutable name, for reseal.
        // This runs after both success and refusal, before classifying either.
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        if writable_root && self.reseal_publication_root_unlocked().is_err() {
            let error = terminal_settlement_error(&self.lease);
            drop(operation);
            return Err(PrivateFsTransitionError::terminal(error));
        }
        if let Err(error) = rename {
            let (clean_error, clean) = if error == PrivateFsError::AlreadyExists {
                let classification =
                    classify_existing_destination_unlocked(destination_parent, destination);
                let unchanged = self
                    .precheck_unlocked()
                    .and_then(|()| destination_parent.verify_boundary_unlocked());
                match (classification, unchanged) {
                    (Ok(ExistingDestination::Safe), Ok(())) => {
                        (PrivateFsError::AlreadyExists, Ok(()))
                    }
                    (Ok(ExistingDestination::Hostile), Ok(())) => (PrivateFsError::Unsafe, Ok(())),
                    (Err(classification), Ok(())) => (classification, Err(classification)),
                    (_, Err(unchanged)) => (unchanged, Err(unchanged)),
                }
            } else {
                (
                    error,
                    ensure_child_absent_unlocked(&destination_parent.core, destination)
                        .and_then(|()| self.precheck_unlocked())
                        .and_then(|()| destination_parent.verify_boundary_unlocked()),
                )
            };
            let terminal = clean
                .is_err()
                .then(|| terminal_settlement_error(&self.lease));
            drop(operation);
            return if let Some(error) = terminal {
                Err(PrivateFsTransitionError::terminal(error))
            } else {
                Err(PrivateFsTransitionError::recoverable(clean_error, self))
            };
        }

        #[cfg(all(test, any(target_os = "macos", target_os = "linux")))]
        if self
            .lease
            .take_lifecycle_fault(crate::lease::LifecycleFault::PublishDestinationIdentity)
            && replace_published_directory_identity_for_test(destination_parent, destination)
                .is_err()
        {
            let error = terminal_settlement_error(&self.lease);
            drop(operation);
            return Err(PrivateFsTransitionError::terminal(error));
        }

        let settlement = (|| {
            platform::sync_directory(&source_parent.handle)?;
            #[cfg(test)]
            if same_parent {
                self.lease.record_same_parent_publish_sync();
            }
            #[cfg(test)]
            if self.lease.take_lifecycle_fault(if same_parent {
                crate::lease::LifecycleFault::PublishSameParentSync
            } else {
                crate::lease::LifecycleFault::PublishSourceParentSync
            }) {
                return Err(PrivateFsError::Io);
            }
            if !same_parent {
                platform::sync_directory(&destination_parent.core.handle)?;
                #[cfg(test)]
                if self.lease.take_lifecycle_fault(
                    crate::lease::LifecycleFault::PublishDestinationParentSync,
                ) {
                    return Err(PrivateFsError::Io);
                }
            }
            let installed = open_sealed_child_unlocked(
                &destination_parent.core,
                &destination_parent.lease,
                DirectoryMode::Writable,
                destination,
            )?;
            if installed.core.identity != self.core.identity {
                return Err(PrivateFsError::IdentityAmbiguous);
            }
            ensure_child_absent_owned_unlocked(source_parent)?;
            verify_parent_authority(source_parent)?;
            destination_parent.verify_boundary_unlocked()?;
            self.lease.verify_authority()?;
            Ok(installed)
        })();
        // Publish quarantine while the operation mutex is still held. A
        // competing capability admission must never enter between uncertain
        // settlement and revocation of the shared namespace authority.
        let settlement = settlement.map_err(|_| terminal_settlement_error(&self.lease));
        drop(operation);
        match settlement {
            Ok(installed) => Ok(installed),
            Err(error) => Err(PrivateFsTransitionError::terminal(error)),
        }
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    fn prepare_publication_root_unlocked(&self) -> Result<(), PrivateFsError> {
        platform::set_directory_mode(&self.core.handle, DirectoryMode::Writable)?;
        #[cfg(test)]
        if self
            .lease
            .take_lifecycle_fault(crate::lease::LifecycleFault::PublishRootWritableCommitted)
        {
            return Err(PrivateFsError::Io);
        }
        verify_core_boundary(&self.core, DirectoryMode::Writable)?;
        self.lease.verify_authority()
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    fn reseal_publication_root_unlocked(&self) -> Result<(), PrivateFsError> {
        #[cfg(test)]
        if self
            .lease
            .take_lifecycle_fault(crate::lease::LifecycleFault::PublishRootResealRefused)
        {
            return Err(PrivateFsError::Io);
        }
        platform::set_directory_mode(&self.core.handle, DirectoryMode::Sealed)?;
        #[cfg(test)]
        if self
            .lease
            .take_lifecycle_fault(crate::lease::LifecycleFault::PublishRootResealCommitted)
        {
            return Err(PrivateFsError::Io);
        }
        platform::sync_directory(&self.core.handle)?;
        #[cfg(test)]
        if self
            .lease
            .take_lifecycle_fault(crate::lease::LifecycleFault::PublishRootResealSync)
        {
            return Err(PrivateFsError::Io);
        }
        // Source/destination name and parent joins are checked by settlement;
        // successful fchmod/fsync alone never constitutes a returned authority.
        Ok(())
    }

    /// Removes this directory only when its exact descriptor is empty.
    ///
    /// The operation is nonrecursive and consumes the capability. A clean
    /// pre-commit failure returns the unchanged sealed capability.
    pub fn remove_empty(self) -> Result<(), PrivateFsTransitionError<Self>> {
        remove_empty_sealed(self)
    }

    fn precheck_unlocked(&self) -> Result<(), PrivateFsError> {
        self.lease.observe(
            self.lease
                .verify_authority()
                .and_then(|()| verify_core_boundary(&self.core, DirectoryMode::Sealed)),
        )
    }
}

impl PrivateDirectory {
    /// Returns this directory's opaque open-handle identity.
    #[must_use]
    pub const fn identity(&self) -> DirectoryIdentity {
        self.core.identity
    }

    /// Creates or admits one private child directory.
    ///
    /// A newly created entry is settled only after the parent directory is
    /// flushed and both parent and child identities are revalidated.
    pub fn create_private_child(
        &self,
        component: &PrivateComponent,
    ) -> Result<Self, PrivateFsError> {
        self.child_directory(
            ChildName::Control(component),
            ChildDirectoryMode::CreateOrOpen,
        )
    }

    /// Opens one existing private protocol child directory without creating it.
    ///
    /// Absence returns [`PrivateFsError::NotFound`]. Existing hostile node
    /// kinds, permissions, ACLs, links, and identity races fail closed.
    pub fn open_private_child(&self, component: &PrivateComponent) -> Result<Self, PrivateFsError> {
        self.child_directory(
            ChildName::Control(component),
            ChildDirectoryMode::OpenExisting,
        )
    }

    /// Creates one new private protocol child directory.
    ///
    /// [`PrivateFsError::AlreadyExists`] is returned only after an existing
    /// directory under that name has itself passed private-directory admission.
    /// An existing hostile node is [`PrivateFsError::Unsafe`].
    pub fn create_new_private_child(
        &self,
        component: &PrivateComponent,
    ) -> Result<Self, PrivateFsError> {
        self.child_directory(ChildName::Control(component), ChildDirectoryMode::CreateNew)
    }

    /// Creates or admits one case-preserving payload child directory.
    pub fn create_entry_child(&self, name: &PrivateEntryName) -> Result<Self, PrivateFsError> {
        self.child_directory(ChildName::Entry(name), ChildDirectoryMode::CreateOrOpen)
    }

    /// Opens one existing case-preserving payload child without creating it.
    ///
    /// Absence returns [`PrivateFsError::NotFound`].
    pub fn open_entry_child(&self, name: &PrivateEntryName) -> Result<Self, PrivateFsError> {
        self.child_directory(ChildName::Entry(name), ChildDirectoryMode::OpenExisting)
    }

    /// Creates one new case-preserving payload child directory.
    ///
    /// [`PrivateFsError::AlreadyExists`] is returned only for a safely admitted
    /// existing directory. An existing file, link, special node, unsafe mode,
    /// or unsafe ACL is [`PrivateFsError::Unsafe`].
    pub fn create_new_entry_child(&self, name: &PrivateEntryName) -> Result<Self, PrivateFsError> {
        self.child_directory(ChildName::Entry(name), ChildDirectoryMode::CreateNew)
    }

    /// Opens one exact sealed protocol child directory.
    ///
    /// The child must have exact mode `0500`; a writable directory is not
    /// silently admitted through the sealed capability surface.
    pub fn open_sealed_private_child(
        &self,
        component: &PrivateComponent,
    ) -> Result<SealedPrivateDirectory, PrivateFsError> {
        self.open_sealed_child(ChildName::Control(component))
    }

    /// Opens one exact sealed case-preserving payload child directory.
    pub fn open_sealed_entry_child(
        &self,
        name: &PrivateEntryName,
    ) -> Result<SealedPrivateDirectory, PrivateFsError> {
        self.open_sealed_child(ChildName::Entry(name))
    }

    /// Opens one protocol child in its exact writable or sealed mode.
    ///
    /// Mode is observed from the same held descriptor whose identity and
    /// spelling are revalidated. This avoids using a failed mode-specific open
    /// as a discriminator during crash recovery.
    pub fn open_private_child_any_mode(
        &self,
        component: &PrivateComponent,
    ) -> Result<OpenedPrivateDirectory, PrivateFsError> {
        self.open_child_any_mode(ChildName::Control(component))
    }

    /// Opens one case-preserving payload child in exact writable or sealed mode.
    pub fn open_entry_child_any_mode(
        &self,
        name: &PrivateEntryName,
    ) -> Result<OpenedPrivateDirectory, PrivateFsError> {
        self.open_child_any_mode(ChildName::Entry(name))
    }

    fn open_child_any_mode(
        &self,
        name: ChildName<'_>,
    ) -> Result<OpenedPrivateDirectory, PrivateFsError> {
        let _operation = self.begin_operation()?;
        self.reject_reserved_name(name)?;
        self.precheck_unlocked()?;
        let (handle, identity, mode) =
            self.lease.observe(platform::open_child_directory_any_mode(
                &self.core.handle,
                &self.core.path,
                name.as_str(),
            ))?;
        let core = DirectoryCore {
            path: self.core.path.join(name.as_str()),
            handle,
            identity: DirectoryIdentity(identity),
            parent: Some(ParentAuthority {
                path: self.core.path.clone(),
                handle: self
                    .core
                    .handle
                    .try_clone()
                    .map_err(|_| PrivateFsError::Io)?,
                identity: self.core.identity,
                child_name: name.to_owned_name(),
                mode: DirectoryMode::Writable,
            }),
        };
        self.lease.observe(verify_core_boundary(&core, mode))?;
        self.precheck_unlocked()?;
        Ok(match mode {
            DirectoryMode::Writable => OpenedPrivateDirectory::Writable(PrivateDirectory {
                core,
                lease: Arc::clone(&self.lease),
                role: DirectoryRole::Child,
            }),
            DirectoryMode::Sealed => OpenedPrivateDirectory::Sealed(SealedPrivateDirectory {
                core,
                lease: Arc::clone(&self.lease),
            }),
        })
    }

    fn open_sealed_child(
        &self,
        name: ChildName<'_>,
    ) -> Result<SealedPrivateDirectory, PrivateFsError> {
        let _operation = self.begin_operation()?;
        self.reject_reserved_name(name)?;
        self.precheck_unlocked()?;
        let child = self.lease.observe(open_sealed_child_unlocked(
            &self.core,
            &self.lease,
            DirectoryMode::Writable,
            name,
        ))?;
        self.precheck_unlocked()?;
        Ok(child)
    }

    /// Seals one optional protocol regular file to exact mode `0400`.
    ///
    /// An already sealed exact file is returned idempotently. Any possible
    /// failure after descriptor chmod quarantines the namespace and reports
    /// [`PrivateFsError::SettlementUnknown`].
    pub fn seal_verified_regular(
        &self,
        component: &PrivateComponent,
    ) -> Result<Option<FileIdentity>, PrivateFsError> {
        self.seal_verified_regular_named(ChildName::Control(component))
    }

    /// Seals one optional case-preserving payload regular file to mode `0400`.
    pub fn seal_verified_entry_regular(
        &self,
        name: &PrivateEntryName,
    ) -> Result<Option<FileIdentity>, PrivateFsError> {
        self.seal_verified_regular_named(ChildName::Entry(name))
    }

    fn seal_verified_regular_named(
        &self,
        name: ChildName<'_>,
    ) -> Result<Option<FileIdentity>, PrivateFsError> {
        let _operation = self.begin_operation()?;
        self.lease.mutation_allowed()?;
        self.reject_reserved_name(name)?;
        self.precheck_unlocked()?;
        let Some(verified) = self.open_optional_regular_unlocked(name, OpenPurpose::Mutation)?
        else {
            self.precheck_unlocked()?;
            return Ok(None);
        };
        if platform::revalidate_regular_mode(
            &self.core.handle,
            &self.core.path,
            name.as_str(),
            &verified.file,
            verified.identity.0,
            RegularMode::Sealed,
        )
        .is_ok()
        {
            self.precheck_unlocked()?;
            return Ok(Some(verified.identity));
        }
        self.lease.observe(platform::revalidate_regular_mode(
            &self.core.handle,
            &self.core.path,
            name.as_str(),
            &verified.file,
            verified.identity.0,
            RegularMode::Writable,
        ))?;
        self.precheck_unlocked()?;

        let chmod = platform::set_regular_mode(&verified.file, RegularMode::Sealed);
        #[cfg(test)]
        let chmod = chmod.and_then(|()| {
            if self
                .lease
                .take_lifecycle_fault(crate::lease::LifecycleFault::SealRegularCommitted)
            {
                Err(PrivateFsError::Io)
            } else {
                Ok(())
            }
        });
        if let Err(error) = chmod {
            let unchanged = platform::revalidate_regular_mode(
                &self.core.handle,
                &self.core.path,
                name.as_str(),
                &verified.file,
                verified.identity.0,
                RegularMode::Writable,
            )
            .and_then(|()| self.verify_boundary_unlocked());
            return if unchanged.is_ok() {
                Err(error)
            } else {
                Err(terminal_settlement_error(&self.lease))
            };
        }

        let settlement: Result<FileIdentity, PrivateFsError> = (|| {
            sync_private_regular(&verified.file)?;
            #[cfg(test)]
            if self
                .lease
                .take_lifecycle_fault(crate::lease::LifecycleFault::SealRegularFileSync)
            {
                return Err(PrivateFsError::Io);
            }
            platform::revalidate_regular_mode(
                &self.core.handle,
                &self.core.path,
                name.as_str(),
                &verified.file,
                verified.identity.0,
                RegularMode::Sealed,
            )?;
            platform::sync_directory(&self.core.handle)?;
            #[cfg(test)]
            if self
                .lease
                .take_lifecycle_fault(crate::lease::LifecycleFault::SealRegularParentSync)
            {
                return Err(PrivateFsError::Io);
            }
            self.verify_boundary_unlocked()?;
            Ok(verified.identity)
        })();
        match settlement {
            Ok(identity) => Ok(Some(identity)),
            Err(_) => Err(terminal_settlement_error(&self.lease)),
        }
    }

    /// Seals this child directory to exact mode `0500`.
    ///
    /// This is a one-level linear transition. Every direct regular child must
    /// already pass dedicated exact `0400` admission and every direct directory
    /// must already pass dedicated exact `0500` admission, enforcing bottom-up
    /// sealing without recursive traversal. The namespace root cannot be
    /// sealed because its live lock authority requires writable mode `0700`.
    /// Previously issued child capabilities bind this directory's old expected
    /// parent mode and therefore fail closed after this transition; callers
    /// must reopen children from the returned sealed capability.
    pub fn seal(
        self,
    ) -> Result<SealedPrivateDirectory, PrivateFsTransitionError<PrivateDirectory>> {
        let operation = match self.begin_operation() {
            Ok(operation) => operation,
            Err(error) => return Err(PrivateFsTransitionError::terminal(error)),
        };
        if let Err(error) = self.precheck_unlocked() {
            drop(operation);
            return Err(PrivateFsTransitionError::terminal(error));
        }
        if let Err(error) = self.lease.mutation_allowed() {
            drop(operation);
            return Err(PrivateFsTransitionError::recoverable(error, self));
        }
        if self.role == DirectoryRole::Root {
            drop(operation);
            return Err(PrivateFsTransitionError::recoverable(
                PrivateFsError::Unsafe,
                self,
            ));
        }
        if let Err(error) = self.direct_children_are_sealed_unlocked() {
            if error == PrivateFsError::IdentityAmbiguous {
                drop(operation);
                return Err(PrivateFsTransitionError::terminal(error));
            }
            let unchanged = self.verify_boundary_unlocked();
            drop(operation);
            return if unchanged.is_ok() {
                Err(PrivateFsTransitionError::recoverable(error, self))
            } else {
                Err(PrivateFsTransitionError::terminal(
                    terminal_settlement_error(&self.lease),
                ))
            };
        }
        if let Err(error) = self.verify_boundary_unlocked() {
            drop(operation);
            return Err(PrivateFsTransitionError::terminal(error));
        }

        let chmod = platform::set_directory_mode(&self.core.handle, DirectoryMode::Sealed);
        #[cfg(test)]
        let chmod = chmod.and_then(|()| {
            if self
                .lease
                .take_lifecycle_fault(crate::lease::LifecycleFault::SealDirectoryCommitted)
            {
                Err(PrivateFsError::Io)
            } else {
                Ok(())
            }
        });
        if let Err(error) = chmod {
            let unchanged = verify_core_boundary(&self.core, DirectoryMode::Writable)
                .and_then(|()| self.lease.verify_authority());
            drop(operation);
            return if unchanged.is_ok() {
                Err(PrivateFsTransitionError::recoverable(error, self))
            } else {
                Err(PrivateFsTransitionError::terminal(
                    terminal_settlement_error(&self.lease),
                ))
            };
        }

        let settlement = (|| {
            platform::sync_directory(&self.core.handle)?;
            #[cfg(test)]
            if self
                .lease
                .take_lifecycle_fault(crate::lease::LifecycleFault::SealDirectorySelfSync)
            {
                return Err(PrivateFsError::Io);
            }
            let parent = self
                .core
                .parent
                .as_ref()
                .ok_or(PrivateFsError::IdentityAmbiguous)?;
            platform::sync_directory(&parent.handle)?;
            #[cfg(test)]
            if self
                .lease
                .take_lifecycle_fault(crate::lease::LifecycleFault::SealDirectoryParentSync)
            {
                return Err(PrivateFsError::Io);
            }
            verify_core_boundary(&self.core, DirectoryMode::Sealed)?;
            self.lease.verify_authority()
        })();
        if settlement.is_err() {
            drop(operation);
            return Err(PrivateFsTransitionError::terminal(
                terminal_settlement_error(&self.lease),
            ));
        }
        drop(operation);
        let Self {
            core,
            lease,
            role: _,
        } = self;
        Ok(SealedPrivateDirectory { core, lease })
    }

    fn direct_children_are_sealed_unlocked(&self) -> Result<(), PrivateFsError> {
        let names = platform::list_names(
            &self.core.handle,
            &self.core.path,
            MAX_INVENTORY_ENTRIES.saturating_add(1),
        )?;
        if names.len() > MAX_INVENTORY_ENTRIES {
            return Err(PrivateFsError::BoundExceeded);
        }
        for name in names {
            PrivateEntryName::new(name.clone()).map_err(|_| PrivateFsError::Unsafe)?;
            #[cfg(test)]
            if self.lease.take_seal_child_identity_fault() {
                return self.lease.observe(Err(PrivateFsError::IdentityAmbiguous));
            }
            match self.lease.observe(platform::inspect_child(
                &self.core.handle,
                &self.core.path,
                &name,
            ))? {
                Some(platform::RawChildKind::Regular(expected)) => {
                    let (file, identity) = self.lease.observe(platform::open_sealed_regular(
                        &self.core.handle,
                        &self.core.path,
                        &name,
                    ))?;
                    if identity != expected {
                        return self.lease.observe(Err(PrivateFsError::IdentityAmbiguous));
                    }
                    self.lease
                        .observe(platform::verify_exact_name(&file, &name))?;
                }
                Some(platform::RawChildKind::Directory(expected)) => {
                    let (directory, identity) =
                        self.lease.observe(platform::open_sealed_child_directory(
                            &self.core.handle,
                            &self.core.path,
                            &name,
                        ))?;
                    if identity != expected {
                        return self.lease.observe(Err(PrivateFsError::IdentityAmbiguous));
                    }
                    self.lease
                        .observe(platform::verify_exact_name(&directory, &name))?;
                }
                None => return self.lease.observe(Err(PrivateFsError::IdentityAmbiguous)),
            }
        }
        Ok(())
    }

    fn child_directory(
        &self,
        name: ChildName<'_>,
        mode: ChildDirectoryMode,
    ) -> Result<Self, PrivateFsError> {
        let _operation = self.begin_operation()?;
        self.reject_reserved_name(name)?;
        if matches!(mode, ChildDirectoryMode::OpenExisting) {
            self.precheck_unlocked()?;
            let child = self.lease.observe(self.open_child_unlocked(name))?;
            self.precheck_unlocked()?;
            return Ok(child);
        }
        self.lease.mutation_allowed()?;
        self.precheck_unlocked()?;

        let created = match self.create_directory_unlocked(name) {
            Ok(created) => created,
            Err(PrivateFsError::SettlementUnknown) => {
                return self
                    .lease
                    .settle_after_commit(Err(PrivateFsError::SettlementUnknown));
            }
            Err(error) => {
                self.precheck_unlocked()?;
                return Err(error);
            }
        };
        if created {
            let settlement = (|| {
                platform::sync_directory(&self.core.handle)?;
                let child = self.open_child_unlocked(name)?;
                self.verify_boundary_unlocked()?;
                Ok(child)
            })();
            return self.lease.settle_after_commit(settlement);
        }

        let observed = match self.open_child_unlocked(name) {
            Err(PrivateFsError::NotFound) => Err(PrivateFsError::IdentityAmbiguous),
            outcome => outcome,
        };
        let child = self.lease.observe(observed)?;
        self.precheck_unlocked()?;
        if matches!(mode, ChildDirectoryMode::CreateNew) {
            Err(PrivateFsError::AlreadyExists)
        } else {
            Ok(child)
        }
    }

    /// Lists a bounded, sorted snapshot of portable direct-child names.
    ///
    /// Node kinds are intentionally not trusted here. Callers must admit any
    /// named file through the regular-file methods before using it. The root
    /// namespace lock internals are never projected into caller inventory.
    pub fn list_components(
        &self,
        max_entries: usize,
    ) -> Result<Vec<PrivateComponent>, PrivateFsError> {
        let _operation = self.begin_operation()?;
        if max_entries == 0 || max_entries > MAX_INVENTORY_ENTRIES {
            return Err(PrivateFsError::BoundExceeded);
        }
        self.precheck_unlocked()?;
        let mut entries = platform::list_names(
            &self.core.handle,
            &self.core.path,
            max_entries.saturating_add(if self.role == DirectoryRole::Root {
                2
            } else {
                0
            }),
        )?
        .into_iter()
        .map(|name| PrivateComponent::new(name).map_err(|_| PrivateFsError::Unsafe))
        .collect::<Result<Vec<_>, _>>()?;
        if self.role == DirectoryRole::Root {
            if entries
                .iter()
                .any(|entry| self.lease.is_reserved_lock_staging_name(entry.as_str()))
            {
                // Staging is required to be absent when activation settles.
                // Never hide residue that appeared after that admission point.
                return Err(PrivateFsError::Unsafe);
            }
            if entries.iter().any(|entry| {
                self.lease.is_reserved_lock_name(entry.as_str())
                    && !self.lease.is_canonical_reserved_lock_name(entry.as_str())
            }) {
                return Err(PrivateFsError::Unsafe);
            }
            entries.retain(|entry| !self.lease.is_canonical_reserved_lock_name(entry.as_str()));
        }
        if entries.len() > max_entries {
            return Err(PrivateFsError::BoundExceeded);
        }
        entries.sort_unstable();
        self.precheck_unlocked()?;
        Ok(entries)
    }

    /// Lists a bounded, byte-sorted snapshot of direct payload entry names.
    ///
    /// Exact spelling is preserved. Node kinds are not trusted by enumeration;
    /// callers use [`Self::inspect_entry`] or a typed open/read method before
    /// consuming an entry. At the root, the exact canonical lock is hidden,
    /// while staging residue or a differently cased lock alias fails closed.
    pub fn list_entry_names(
        &self,
        max_entries: usize,
    ) -> Result<Vec<PrivateEntryName>, PrivateFsError> {
        let _operation = self.begin_operation()?;
        if max_entries == 0 || max_entries > MAX_INVENTORY_ENTRIES {
            return Err(PrivateFsError::BoundExceeded);
        }
        self.precheck_unlocked()?;
        let mut entries = platform::list_names(
            &self.core.handle,
            &self.core.path,
            max_entries.saturating_add(if self.role == DirectoryRole::Root {
                2
            } else {
                0
            }),
        )?
        .into_iter()
        .map(|name| PrivateEntryName::new(name).map_err(|_| PrivateFsError::Unsafe))
        .collect::<Result<Vec<_>, _>>()?;
        if self.role == DirectoryRole::Root {
            if entries
                .iter()
                .any(|entry| self.lease.is_reserved_lock_staging_name(entry.as_str()))
            {
                return Err(PrivateFsError::Unsafe);
            }
            if entries.iter().any(|entry| {
                self.lease.is_reserved_lock_name(entry.as_str())
                    && !self.lease.is_canonical_reserved_lock_name(entry.as_str())
            }) {
                return Err(PrivateFsError::Unsafe);
            }
            entries.retain(|entry| !self.lease.is_canonical_reserved_lock_name(entry.as_str()));
        }
        if entries.len() > max_entries {
            return Err(PrivateFsError::BoundExceeded);
        }
        entries.sort_unstable();
        self.precheck_unlocked()?;
        Ok(entries)
    }

    /// Returns whether a verified regular child exists.
    ///
    /// A symlink, directory, hard link, reparse point, unsafe permission, or
    /// reserved root-lock reference is an error rather than an affirmative
    /// result.
    pub fn regular_exists(&self, component: &PrivateComponent) -> Result<bool, PrivateFsError> {
        self.regular_exists_named(ChildName::Control(component))
    }

    /// Returns whether a verified case-preserving regular payload entry exists.
    ///
    /// Absence is `false`; every present non-regular or unsafe node is an error.
    pub fn entry_regular_exists(&self, name: &PrivateEntryName) -> Result<bool, PrivateFsError> {
        self.regular_exists_named(ChildName::Entry(name))
    }

    fn regular_exists_named(&self, name: ChildName<'_>) -> Result<bool, PrivateFsError> {
        let _operation = self.begin_operation()?;
        self.reject_reserved_name(name)?;
        self.precheck_unlocked()?;
        let exists = self.open_optional_regular_unlocked(name, OpenPurpose::Read)?;
        self.precheck_unlocked()?;
        Ok(exists.is_some())
    }

    /// Returns the opaque identity of an optional verified regular child.
    pub fn regular_identity(
        &self,
        component: &PrivateComponent,
    ) -> Result<Option<FileIdentity>, PrivateFsError> {
        self.regular_identity_named(ChildName::Control(component))
    }

    /// Returns the opaque identity of an optional verified regular payload entry.
    pub fn entry_regular_identity(
        &self,
        name: &PrivateEntryName,
    ) -> Result<Option<FileIdentity>, PrivateFsError> {
        self.regular_identity_named(ChildName::Entry(name))
    }

    fn regular_identity_named(
        &self,
        name: ChildName<'_>,
    ) -> Result<Option<FileIdentity>, PrivateFsError> {
        let _operation = self.begin_operation()?;
        self.reject_reserved_name(name)?;
        self.precheck_unlocked()?;
        let identity = self
            .open_optional_regular_unlocked(name, OpenPurpose::Read)?
            .map(|verified| verified.identity);
        self.precheck_unlocked()?;
        Ok(identity)
    }

    /// Inspects one optional payload entry without following links.
    ///
    /// On Unix this is descriptor-relative and nonblocking. A symlink,
    /// hard-linked regular file, FIFO, socket, device, unsafe permission or
    /// ACL, or identity race is rejected rather than reported as a kind.
    pub fn inspect_entry(
        &self,
        name: &PrivateEntryName,
    ) -> Result<Option<PrivateChildKind>, PrivateFsError> {
        let _operation = self.begin_operation()?;
        let name = ChildName::Entry(name);
        self.reject_reserved_name(name)?;
        self.precheck_unlocked()?;
        let inspected = self.lease.observe(platform::inspect_child(
            &self.core.handle,
            &self.core.path,
            name.as_str(),
        ))?;
        let inspected = inspected.map(|kind| match kind {
            platform::RawChildKind::Regular(identity) => {
                PrivateChildKind::RegularFile(FileIdentity(identity))
            }
            platform::RawChildKind::Directory(identity) => {
                PrivateChildKind::Directory(DirectoryIdentity(identity))
            }
        });
        self.precheck_unlocked()?;
        Ok(inspected)
    }

    /// Reads one optional regular child without exceeding `limit`.
    ///
    /// Empty files are returned as empty vectors. Subsystems decide whether
    /// emptiness is valid for their own protocol. The open handle and its
    /// descriptor-relative name are revalidated after the bounded read.
    pub fn read_bounded_regular(
        &self,
        component: &PrivateComponent,
        limit: ByteLimit,
    ) -> Result<Option<Vec<u8>>, PrivateFsError> {
        self.read_bounded_regular_named(ChildName::Control(component), limit)
    }

    /// Reads one optional regular payload entry without exceeding `limit`.
    ///
    /// The exact case-preserving name and open identity are revalidated after
    /// the bounded read. Empty files are returned as empty vectors.
    pub fn read_bounded_entry_regular(
        &self,
        name: &PrivateEntryName,
        limit: ByteLimit,
    ) -> Result<Option<Vec<u8>>, PrivateFsError> {
        self.read_bounded_regular_named(ChildName::Entry(name), limit)
    }

    /// Runs a callback with one bounded verified protocol-file reader.
    ///
    /// The callback receives only a [`Read`] capability capped at the file's
    /// admitted initial length, never a file or path. Its result is nested so
    /// caller-owned validation failures remain distinct from filesystem
    /// failures. Identity, exact spelling, length, and the namespace boundary
    /// are revalidated after the callback even when it returns an error value.
    /// The callback executes under the namespace operation lease and therefore
    /// must not re-enter this namespace.
    pub fn with_bounded_regular_reader<T, E>(
        &self,
        component: &PrivateComponent,
        limit: ByteLimit,
        callback: impl FnOnce(&mut dyn Read) -> Result<T, E>,
    ) -> Result<Option<Result<T, E>>, PrivateFsError> {
        self.with_bounded_regular_reader_named(ChildName::Control(component), limit, callback)
    }

    /// Runs a callback with one bounded verified payload-entry reader.
    ///
    /// The callback receives only a [`Read`] capability capped at the file's
    /// admitted initial length. Filesystem validation always takes precedence
    /// over the nested callback result. It must not re-enter this namespace.
    pub fn with_bounded_entry_regular_reader<T, E>(
        &self,
        name: &PrivateEntryName,
        limit: ByteLimit,
        callback: impl FnOnce(&mut dyn Read) -> Result<T, E>,
    ) -> Result<Option<Result<T, E>>, PrivateFsError> {
        self.with_bounded_regular_reader_named(ChildName::Entry(name), limit, callback)
    }

    /// Runs a callback with one exact `0400` protocol-file reader.
    ///
    /// Unlike [`Self::with_bounded_entry_regular_reader`], this admission is
    /// intentionally strict even though the containing directory remains
    /// writable: an exact `0600` file is refused. This is the read boundary for
    /// already sealed artifacts during staged materialization. The callback
    /// receives only a bounded [`Read`] capability, its result remains nested,
    /// and exact mode, length, identity, spelling, and the writable parent
    /// capability are revalidated afterward. It must not re-enter this
    /// namespace while the callback is running.
    pub fn with_bounded_sealed_regular_reader<T, E>(
        &self,
        component: &PrivateComponent,
        limit: ByteLimit,
        callback: impl FnOnce(&mut dyn Read) -> Result<T, E>,
    ) -> Result<Option<Result<T, E>>, PrivateFsError> {
        self.with_bounded_sealed_regular_reader_named(
            ChildName::Control(component),
            limit,
            callback,
        )
    }

    /// Runs a callback with one exact `0400` payload-file reader.
    ///
    /// This is the case-preserving entry-name variant of
    /// [`Self::with_bounded_sealed_regular_reader`] and has the same strict
    /// admission, bounded-reader, and post-callback reproof contract.
    pub fn with_bounded_sealed_entry_regular_reader<T, E>(
        &self,
        name: &PrivateEntryName,
        limit: ByteLimit,
        callback: impl FnOnce(&mut dyn Read) -> Result<T, E>,
    ) -> Result<Option<Result<T, E>>, PrivateFsError> {
        self.with_bounded_sealed_regular_reader_named(ChildName::Entry(name), limit, callback)
    }

    fn with_bounded_sealed_regular_reader_named<T, E>(
        &self,
        name: ChildName<'_>,
        limit: ByteLimit,
        callback: impl FnOnce(&mut dyn Read) -> Result<T, E>,
    ) -> Result<Option<Result<T, E>>, PrivateFsError> {
        let _operation = self.begin_operation()?;
        self.reject_reserved_name(name)?;
        self.precheck_unlocked()?;
        with_bounded_sealed_regular_reader_unlocked(
            &self.core,
            &self.lease,
            DirectoryMode::Writable,
            name,
            limit,
            |reader, _| callback(reader),
        )
    }

    /// Runs a synchronous callback with this capability's currently verified path.
    ///
    /// The operation mutex is released while the callback runs so read-only
    /// namespace operations may re-enter. A namespace-wide path pin makes every
    /// mutation return [`PrivateFsError::InUse`] until the callback finishes.
    /// On ordinary return the mutex is reacquired and the complete capability
    /// boundary is revalidated before the pin is released. Panic unwinding
    /// releases the pin through RAII; the next operation still performs its
    /// normal precheck. The borrowed path is synchronous validation context
    /// only: retaining or later reusing it does not retain filesystem authority.
    pub fn with_verified_path<T>(
        &self,
        callback: impl FnOnce(&Path) -> T,
    ) -> Result<T, PrivateFsError> {
        with_verified_path_callback(&self.core, &self.lease, DirectoryMode::Writable, callback)
    }

    fn with_bounded_regular_reader_named<T, E>(
        &self,
        name: ChildName<'_>,
        limit: ByteLimit,
        callback: impl FnOnce(&mut dyn Read) -> Result<T, E>,
    ) -> Result<Option<Result<T, E>>, PrivateFsError> {
        self.with_bounded_regular_reader_and_length_named(name, limit, |reader, _| callback(reader))
    }

    /// Reads a regular file's bounded logical length without reading its payload.
    /// The same exact-name, identity, mode and final-length checks as a bounded
    /// read protect the observation. It is a snapshot, not lasting file authority.
    pub fn entry_regular_length(
        &self,
        name: &PrivateEntryName,
        limit: ByteLimit,
    ) -> Result<Option<u64>, PrivateFsError> {
        self.with_bounded_regular_reader_and_length_named(
            ChildName::Entry(name),
            limit,
            |_, length| Ok::<_, std::convert::Infallible>(length),
        )
        .map(|value| {
            value.map(|value| match value {
                Ok(length) => length,
                Err(never) => match never {},
            })
        })
    }

    fn with_bounded_regular_reader_and_length_named<T, E>(
        &self,
        name: ChildName<'_>,
        limit: ByteLimit,
        callback: impl FnOnce(&mut dyn Read, u64) -> Result<T, E>,
    ) -> Result<Option<Result<T, E>>, PrivateFsError> {
        let _operation = self.begin_operation()?;
        self.reject_reserved_name(name)?;
        self.precheck_unlocked()?;
        let Some(mut verified) = self.open_optional_regular_unlocked(name, OpenPurpose::Read)?
        else {
            self.precheck_unlocked()?;
            return Ok(None);
        };
        #[cfg(test)]
        let initial_metadata = if self
            .lease
            .take_streaming_fault(crate::lease::StreamingFault::InitialReadMetadata)
        {
            Err(PrivateFsError::IdentityAmbiguous)
        } else {
            verified
                .file
                .metadata()
                .map_err(|_| PrivateFsError::IdentityAmbiguous)
        };
        #[cfg(not(test))]
        let initial_metadata = verified
            .file
            .metadata()
            .map_err(|_| PrivateFsError::IdentityAmbiguous);
        let initial_length = self.lease.observe(initial_metadata)?.len();
        if initial_length > u64::try_from(limit.get()).unwrap_or(u64::MAX) {
            return Err(PrivateFsError::BoundExceeded);
        }

        let callback_result = {
            let mut bounded = Read::by_ref(&mut verified.file).take(initial_length);
            callback(&mut bounded, initial_length)
        };
        let validation = (|| {
            let final_length = verified
                .file
                .metadata()
                .map_err(|_| PrivateFsError::IdentityAmbiguous)?
                .len();
            if final_length != initial_length {
                return Err(PrivateFsError::IdentityAmbiguous);
            }
            platform::revalidate_regular(
                &self.core.handle,
                &self.core.path,
                name.as_str(),
                &verified.file,
                verified.identity.0,
            )?;
            self.verify_boundary_unlocked()
        })();
        self.lease.observe(validation)?;
        Ok(Some(callback_result))
    }

    fn read_bounded_regular_named(
        &self,
        name: ChildName<'_>,
        limit: ByteLimit,
    ) -> Result<Option<Vec<u8>>, PrivateFsError> {
        let _operation = self.begin_operation()?;
        self.reject_reserved_name(name)?;
        self.precheck_unlocked()?;
        let Some(mut verified) = self.open_optional_regular_unlocked(name, OpenPurpose::Read)?
        else {
            self.precheck_unlocked()?;
            return Ok(None);
        };
        let metadata = verified.file.metadata().map_err(|_| PrivateFsError::Io)?;
        let length = usize::try_from(metadata.len()).map_err(|_| PrivateFsError::BoundExceeded)?;
        if length > limit.get() {
            return Err(PrivateFsError::BoundExceeded);
        }
        let mut bytes = Vec::with_capacity(length);
        let mut bounded = Read::by_ref(&mut verified.file).take(
            u64::try_from(limit.get())
                .unwrap_or(u64::MAX)
                .saturating_add(1),
        );
        bounded
            .read_to_end(&mut bytes)
            .map_err(|_| PrivateFsError::Io)?;
        if bytes.len() != length || bytes.len() > limit.get() {
            return Err(PrivateFsError::BoundExceeded);
        }
        self.lease.observe(platform::revalidate_regular(
            &self.core.handle,
            &self.core.path,
            name.as_str(),
            &verified.file,
            verified.identity.0,
        ))?;
        self.precheck_unlocked()?;
        Ok(Some(bytes))
    }

    /// Creates, writes, and durably settles one new private regular child.
    ///
    /// The file is flushed before its descriptor-relative identity is checked;
    /// the containing directory is then flushed before the final boundary
    /// check. Any failure after create-new commits returns
    /// [`PrivateFsError::SettlementUnknown`] and quarantines the shared lease.
    pub fn write_new_synced(
        &self,
        component: &PrivateComponent,
        bytes: &[u8],
        limit: ByteLimit,
    ) -> Result<FileIdentity, PrivateFsError> {
        self.write_new_synced_named(ChildName::Control(component), bytes, limit)
    }

    /// Creates, writes, and durably settles one new regular payload entry.
    ///
    /// This is create-new only and never overwrites an existing node. Any
    /// failure after creation may have committed and therefore quarantines the
    /// shared lease with [`PrivateFsError::SettlementUnknown`].
    pub fn write_new_entry_synced(
        &self,
        name: &PrivateEntryName,
        bytes: &[u8],
        limit: ByteLimit,
    ) -> Result<FileIdentity, PrivateFsError> {
        self.write_new_synced_named(ChildName::Entry(name), bytes, limit)
    }

    /// Streams one exact-length source into a new regular protocol child.
    ///
    /// This has the same create-new, exact-EOF, cleanup, durability, and
    /// quarantine contract as [`Self::write_new_entry_from_reader`], but it
    /// accepts the canonical [`PrivateComponent`] namespace used by subsystem
    /// metadata protocols. The reader executes under the non-reentrant
    /// namespace operation lease and therefore must not re-enter this
    /// namespace.
    pub fn write_new_from_reader<R: Read + ?Sized>(
        &self,
        component: &PrivateComponent,
        reader: &mut R,
        expected_length: StreamingFileLength,
    ) -> Result<FileIdentity, StreamingWriteError> {
        self.write_new_from_reader_named(ChildName::Control(component), reader, expected_length)
    }

    /// Streams one exact-length source into a new regular payload entry.
    ///
    /// The source must yield exactly `expected_length` bytes followed by EOF.
    /// Short, long, and source-error outcomes trigger held-identity cleanup;
    /// they are returned as clean source failures only after durable removal,
    /// exact absence, and the namespace boundary have all been proven. Source
    /// errors and source bytes are never retained. The reader executes under
    /// the non-reentrant namespace operation lease and therefore must not
    /// re-enter this namespace.
    pub fn write_new_entry_from_reader<R: Read + ?Sized>(
        &self,
        name: &PrivateEntryName,
        reader: &mut R,
        expected_length: StreamingFileLength,
    ) -> Result<FileIdentity, StreamingWriteError> {
        self.write_new_from_reader_named(ChildName::Entry(name), reader, expected_length)
    }

    fn write_new_from_reader_named(
        &self,
        name: ChildName<'_>,
        reader: &mut (impl Read + ?Sized),
        expected_length: StreamingFileLength,
    ) -> Result<FileIdentity, StreamingWriteError> {
        let _operation = self.begin_operation().map_err(StreamingWriteError::from)?;
        self.lease
            .mutation_allowed()
            .map_err(StreamingWriteError::from)?;
        self.reject_reserved_name(name)
            .map_err(StreamingWriteError::from)?;
        self.precheck_unlocked()
            .map_err(StreamingWriteError::from)?;

        let (file, identity) =
            match platform::create_new_regular(&self.core.handle, &self.core.path, name.as_str()) {
                Ok(created) => created,
                Err(PrivateFsError::SettlementUnknown) => {
                    return self.quarantine_streaming_write_unlocked();
                }
                Err(PrivateFsError::AlreadyExists) => {
                    let inspected = self
                        .lease
                        .observe(platform::inspect_child(
                            &self.core.handle,
                            &self.core.path,
                            name.as_str(),
                        ))
                        .map_err(StreamingWriteError::from)?;
                    let error = match inspected {
                        Some(platform::RawChildKind::Regular(_)) => PrivateFsError::AlreadyExists,
                        Some(platform::RawChildKind::Directory(_)) => PrivateFsError::Unsafe,
                        None => {
                            let error = self
                                .lease
                                .observe::<()>(Err(PrivateFsError::IdentityAmbiguous))
                                .err()
                                .unwrap_or(PrivateFsError::IdentityAmbiguous);
                            return Err(StreamingWriteError::Filesystem(error));
                        }
                    };
                    self.precheck_unlocked()
                        .map_err(StreamingWriteError::from)?;
                    return Err(StreamingWriteError::Filesystem(error));
                }
                Err(error) => {
                    self.precheck_unlocked()
                        .map_err(StreamingWriteError::from)?;
                    return Err(StreamingWriteError::Filesystem(error));
                }
            };
        let mut verified = VerifiedRegular {
            file,
            identity: FileIdentity(identity),
        };

        if platform::verify_exact_name(&verified.file, name.as_str()).is_err() {
            return self.quarantine_streaming_write_unlocked();
        }

        #[cfg(test)]
        let copy_result = if self
            .lease
            .take_streaming_fault(crate::lease::StreamingFault::Write)
        {
            Err(ExactCopyError::SinkWrite)
        } else {
            copy_exact_to_private_regular(reader, &mut verified.file, expected_length)
        };
        #[cfg(not(test))]
        let copy_result =
            copy_exact_to_private_regular(reader, &mut verified.file, expected_length);
        if let Err(error) = copy_result {
            return self.fail_streaming_write_unlocked(name, verified, error.into_public());
        }

        // `Read` is caller-controlled through the EOF probe. It may have
        // mutated the newly created inode by another descriptor while still
        // reporting exact EOF, so bind the result to the declared length only
        // after the final source invocation has returned.
        let exact_length = verified
            .file
            .metadata()
            .map(|metadata| metadata.len() == expected_length.get())
            .unwrap_or(false);
        if !exact_length {
            return self.fail_streaming_write_unlocked(
                name,
                verified,
                StreamingWriteError::SinkLengthMismatch,
            );
        }

        let file_sync = sync_private_regular(&verified.file);
        #[cfg(test)]
        let file_sync = file_sync.and_then(|()| {
            if self
                .lease
                .take_streaming_fault(crate::lease::StreamingFault::FileSync)
            {
                Err(PrivateFsError::Io)
            } else {
                Ok(())
            }
        });
        if let Err(error) = file_sync {
            return self.fail_streaming_write_unlocked(
                name,
                verified,
                StreamingWriteError::Filesystem(error),
            );
        }

        if platform::revalidate_regular(
            &self.core.handle,
            &self.core.path,
            name.as_str(),
            &verified.file,
            verified.identity.0,
        )
        .is_err()
        {
            return self.quarantine_streaming_write_unlocked();
        }

        let directory_sync = platform::sync_directory(&self.core.handle);
        #[cfg(test)]
        let directory_sync = directory_sync.and_then(|()| {
            if self
                .lease
                .take_streaming_fault(crate::lease::StreamingFault::DirectorySync)
            {
                Err(PrivateFsError::Io)
            } else {
                Ok(())
            }
        });
        if let Err(error) = directory_sync {
            return self.fail_streaming_write_unlocked(
                name,
                verified,
                StreamingWriteError::Filesystem(error),
            );
        }

        if self.verify_boundary_unlocked().is_err() {
            return self.quarantine_streaming_write_unlocked();
        }
        Ok(verified.identity)
    }

    fn write_new_synced_named(
        &self,
        name: ChildName<'_>,
        bytes: &[u8],
        limit: ByteLimit,
    ) -> Result<FileIdentity, PrivateFsError> {
        let _operation = self.begin_operation()?;
        self.lease.mutation_allowed()?;
        self.reject_reserved_name(name)?;
        if bytes.len() > limit.get() {
            return Err(PrivateFsError::BoundExceeded);
        }
        self.precheck_unlocked()?;
        let (mut file, identity) =
            match platform::create_new_regular(&self.core.handle, &self.core.path, name.as_str()) {
                Ok(created) => {
                    if platform::verify_exact_name(&created.0, name.as_str()).is_err() {
                        return self
                            .lease
                            .settle_after_commit(Err(PrivateFsError::SettlementUnknown));
                    }
                    created
                }
                Err(PrivateFsError::SettlementUnknown) => {
                    return self
                        .lease
                        .settle_after_commit(Err(PrivateFsError::SettlementUnknown));
                }
                Err(PrivateFsError::AlreadyExists) => {
                    let inspected = self.lease.observe(platform::inspect_child(
                        &self.core.handle,
                        &self.core.path,
                        name.as_str(),
                    ))?;
                    if inspected.is_none() {
                        return self.lease.observe(Err(PrivateFsError::IdentityAmbiguous));
                    }
                    self.precheck_unlocked()?;
                    return Err(PrivateFsError::AlreadyExists);
                }
                Err(error) => {
                    self.precheck_unlocked()?;
                    return Err(error);
                }
            };
        let settlement = (|| {
            write_all_private_regular(&mut file, bytes)?;
            sync_private_regular(&file)?;
            platform::revalidate_regular(
                &self.core.handle,
                &self.core.path,
                name.as_str(),
                &file,
                identity,
            )?;
            platform::sync_directory(&self.core.handle)?;
            self.verify_boundary_unlocked()?;
            Ok(FileIdentity(identity))
        })();
        self.lease.settle_after_commit(settlement)
    }

    /// Durably removes one optional child after regular-file admission.
    ///
    /// Once unlink commits, every failure becomes an unknown settlement and
    /// quarantines the lease; callers must not infer whether residue exists.
    pub fn remove_verified_regular(
        &self,
        component: &PrivateComponent,
    ) -> Result<bool, PrivateFsError> {
        self.remove_verified_regular_named(ChildName::Control(component))
    }

    /// Durably removes one optional verified regular payload entry.
    ///
    /// Links, directories, special nodes, unsafe permissions or ACLs, case
    /// aliases, and identity races fail closed without being removed.
    pub fn remove_verified_entry_regular(
        &self,
        name: &PrivateEntryName,
    ) -> Result<bool, PrivateFsError> {
        self.remove_verified_regular_named(ChildName::Entry(name))
    }

    fn remove_verified_regular_named(&self, name: ChildName<'_>) -> Result<bool, PrivateFsError> {
        let _operation = self.begin_operation()?;
        self.lease.mutation_allowed()?;
        self.reject_reserved_name(name)?;
        self.precheck_unlocked()?;
        let Some(verified) = self.open_optional_regular_unlocked(name, OpenPurpose::Mutation)?
        else {
            self.precheck_unlocked()?;
            return Ok(false);
        };
        if let Err(error) = self.remove_regular_unlocked(name) {
            let unchanged = platform::revalidate_regular(
                &self.core.handle,
                &self.core.path,
                name.as_str(),
                &verified.file,
                verified.identity.0,
            )
            .and_then(|()| self.verify_boundary_unlocked());
            return self.settle_failed_mutation_unlocked(unchanged, error);
        }
        let settlement = (|| {
            drop(verified);
            platform::sync_directory(&self.core.handle)?;
            self.ensure_absent_unlocked(name)?;
            self.verify_boundary_unlocked()?;
            Ok(true)
        })();
        self.lease.settle_after_commit(settlement)
    }

    /// Durably and atomically replaces `destination` with `source`.
    ///
    /// An existing destination remains intact until the replacing rename
    /// commits. After commit the directory is flushed, the installed identity
    /// must equal the held source identity, and the old source name must be
    /// absent. Unsettled commits quarantine the complete shared lease.
    pub fn replace_verified_regular(
        &self,
        source: &PrivateComponent,
        destination: &PrivateComponent,
    ) -> Result<FileIdentity, PrivateFsError> {
        let _operation = self.begin_operation()?;
        self.lease.mutation_allowed()?;
        self.reject_reserved(source)?;
        self.reject_reserved(destination)?;
        if source == destination {
            return Err(PrivateFsError::Unsafe);
        }
        self.precheck_unlocked()?;
        let source_file = self
            .open_optional_regular_unlocked(ChildName::Control(source), OpenPurpose::Mutation)?
            .ok_or(PrivateFsError::Unsafe)?;
        sync_private_regular(&source_file.file)?;
        self.lease.observe(platform::revalidate_regular(
            &self.core.handle,
            &self.core.path,
            source.as_str(),
            &source_file.file,
            source_file.identity.0,
        ))?;
        if let Some(destination_file) = self.open_optional_regular_unlocked(
            ChildName::Control(destination),
            OpenPurpose::Mutation,
        )? {
            drop(destination_file);
        }
        if let Err(error) = self.atomic_replace_unlocked(source, destination) {
            let unchanged = self.revalidate_failed_rename_unlocked(source, &source_file);
            return self.settle_failed_mutation_unlocked(unchanged, error);
        }
        self.settle_rename_unlocked(source, destination, source_file)
    }

    /// Durably publishes `source` only when `destination` is absent.
    ///
    /// This calls the platform's true no-replace primitive; it never emulates
    /// exclusivity with an observation followed by a replacing rename.
    pub fn publish_noreplace_verified_regular(
        &self,
        source: &PrivateComponent,
        destination: &PrivateComponent,
    ) -> Result<FileIdentity, PrivateFsError> {
        let _operation = self.begin_operation()?;
        self.lease.mutation_allowed()?;
        self.reject_reserved(source)?;
        self.reject_reserved(destination)?;
        if source == destination {
            return Err(PrivateFsError::Unsafe);
        }
        self.precheck_unlocked()?;
        let source_file = self
            .open_optional_regular_unlocked(ChildName::Control(source), OpenPurpose::Mutation)?
            .ok_or(PrivateFsError::Unsafe)?;
        sync_private_regular(&source_file.file)?;
        self.lease.observe(platform::revalidate_regular(
            &self.core.handle,
            &self.core.path,
            source.as_str(),
            &source_file.file,
            source_file.identity.0,
        ))?;
        if let Err(error) = self.atomic_publish_noreplace_unlocked(source, destination) {
            let unchanged = self.revalidate_failed_rename_unlocked(source, &source_file);
            return self.settle_failed_mutation_unlocked(unchanged, error);
        }
        self.settle_rename_unlocked(source, destination, source_file)
    }

    /// Removes this directory only when its exact descriptor is empty.
    ///
    /// This nonrecursive linear transition is unavailable for the namespace
    /// root. A clean pre-commit failure returns the unchanged capability;
    /// every possible committed removal returns no capability and quarantines
    /// the shared lease when durable settlement cannot be proven.
    pub fn remove_empty(self) -> Result<(), PrivateFsTransitionError<Self>> {
        remove_empty_writable(self)
    }

    /// Opens and consumes one named protocol child for exact empty removal.
    ///
    /// Absence is `false`. The relationship is captured internally from the
    /// opened child; no caller-provided parent/name relationship is trusted.
    pub fn remove_empty_private_child(
        &self,
        component: &PrivateComponent,
    ) -> Result<bool, PrivateFsError> {
        self.remove_empty_child_named(ChildName::Control(component))
    }

    /// Opens and consumes one case-preserving child for exact empty removal.
    pub fn remove_empty_entry_child(
        &self,
        name: &PrivateEntryName,
    ) -> Result<bool, PrivateFsError> {
        self.remove_empty_child_named(ChildName::Entry(name))
    }

    fn remove_empty_child_named(&self, name: ChildName<'_>) -> Result<bool, PrivateFsError> {
        let _operation = self.begin_operation()?;
        self.lease.mutation_allowed()?;
        self.reject_reserved_name(name)?;
        self.precheck_unlocked()?;
        #[cfg(test)]
        let opened = if self
            .lease
            .take_lifecycle_fault(crate::lease::LifecycleFault::RemoveChildOpenIdentity)
        {
            Err(PrivateFsError::IdentityAmbiguous)
        } else {
            self.open_child_unlocked(name)
        };
        #[cfg(not(test))]
        let opened = self.open_child_unlocked(name);
        let (core, mode) = match self.lease.observe(opened) {
            Ok(child) => (child.core, DirectoryMode::Writable),
            Err(PrivateFsError::NotFound) => {
                self.precheck_unlocked()?;
                return Ok(false);
            }
            Err(PrivateFsError::Unsafe) => {
                let child = self.lease.observe(open_sealed_child_unlocked(
                    &self.core,
                    &self.lease,
                    DirectoryMode::Writable,
                    name,
                ))?;
                (child.core, DirectoryMode::Sealed)
            }
            Err(error) => return Err(error),
        };
        match remove_empty_core_unlocked(&core, &self.lease, mode) {
            Ok(()) => Ok(true),
            Err(RemoveEmptyFailure::Clean(error) | RemoveEmptyFailure::Terminal(error)) => {
                Err(error)
            }
        }
    }

    /// Flushes this directory's metadata after serialized boundary checks.
    pub fn sync(&self) -> Result<(), PrivateFsError> {
        let _operation = self.begin_operation()?;
        self.precheck_unlocked()?;
        platform::sync_directory(&self.core.handle)?;
        self.precheck_unlocked()
    }

    fn begin_operation(&self) -> Result<MutexGuard<'_, ()>, PrivateFsError> {
        self.lease.begin()
    }

    fn create_directory_unlocked(&self, name: ChildName<'_>) -> Result<bool, PrivateFsError> {
        let result = platform::create_directory(&self.core.handle, &self.core.path, name.as_str());
        #[cfg(test)]
        if matches!(&result, Ok(true))
            && self.lease.take_committed_mutation_fault(
                crate::lease::CommittedMutationFault::CreateDirectory,
            )
        {
            return Err(PrivateFsError::SettlementUnknown);
        }
        result
    }

    fn remove_regular_unlocked(&self, name: ChildName<'_>) -> Result<(), PrivateFsError> {
        let result = platform::remove_regular(&self.core.handle, &self.core.path, name.as_str());
        #[cfg(test)]
        if result.is_ok()
            && self
                .lease
                .take_committed_mutation_fault(crate::lease::CommittedMutationFault::Remove)
        {
            return Err(PrivateFsError::Io);
        }
        result
    }

    fn atomic_replace_unlocked(
        &self,
        source: &PrivateComponent,
        destination: &PrivateComponent,
    ) -> Result<(), PrivateFsError> {
        let result = platform::atomic_replace(
            &self.core.handle,
            &self.core.path,
            source.as_str(),
            destination.as_str(),
        );
        #[cfg(test)]
        if result.is_ok()
            && self
                .lease
                .take_committed_mutation_fault(crate::lease::CommittedMutationFault::Replace)
        {
            return Err(PrivateFsError::Io);
        }
        result
    }

    fn atomic_publish_noreplace_unlocked(
        &self,
        source: &PrivateComponent,
        destination: &PrivateComponent,
    ) -> Result<(), PrivateFsError> {
        let result = platform::atomic_publish_noreplace(
            &self.core.handle,
            &self.core.path,
            source.as_str(),
            destination.as_str(),
        );
        #[cfg(test)]
        if result.is_ok()
            && self.lease.take_committed_mutation_fault(
                crate::lease::CommittedMutationFault::PublishNoreplace,
            )
        {
            return Err(PrivateFsError::Io);
        }
        result
    }

    fn reject_reserved(&self, component: &PrivateComponent) -> Result<(), PrivateFsError> {
        self.reject_reserved_name(ChildName::Control(component))
    }

    fn reject_reserved_name(&self, name: ChildName<'_>) -> Result<(), PrivateFsError> {
        if self.role == DirectoryRole::Root && self.lease.is_reserved_name(name.as_str()) {
            return Err(PrivateFsError::ReservedComponent);
        }
        Ok(())
    }

    fn precheck_unlocked(&self) -> Result<(), PrivateFsError> {
        self.lease.observe(self.verify_boundary_unlocked())
    }

    fn verify_boundary_unlocked(&self) -> Result<(), PrivateFsError> {
        self.lease.verify_authority()?;
        verify_core_boundary(&self.core, DirectoryMode::Writable)
    }

    fn open_child_unlocked(&self, name: ChildName<'_>) -> Result<Self, PrivateFsError> {
        let (handle, identity) =
            platform::open_child_directory(&self.core.handle, &self.core.path, name.as_str())?;
        platform::verify_exact_name(&handle, name.as_str())?;
        Ok(Self {
            core: DirectoryCore {
                path: self.core.path.join(name.as_str()),
                handle,
                identity: DirectoryIdentity(identity),
                parent: Some(ParentAuthority {
                    path: self.core.path.clone(),
                    handle: self
                        .core
                        .handle
                        .try_clone()
                        .map_err(|_| PrivateFsError::Io)?,
                    identity: self.core.identity,
                    child_name: name.to_owned_name(),
                    mode: DirectoryMode::Writable,
                }),
            },
            lease: Arc::clone(&self.lease),
            role: DirectoryRole::Child,
        })
    }

    fn open_optional_regular_unlocked(
        &self,
        name: ChildName<'_>,
        purpose: OpenPurpose,
    ) -> Result<Option<VerifiedRegular>, PrivateFsError> {
        let opened =
            platform::open_regular(&self.core.handle, &self.core.path, name.as_str(), purpose)
                .and_then(|(file, identity)| {
                    platform::verify_exact_name(&file, name.as_str())?;
                    Ok((file, identity))
                });
        match self.lease.observe(opened) {
            Ok((file, identity)) => Ok(Some(VerifiedRegular {
                file,
                identity: FileIdentity(identity),
            })),
            Err(PrivateFsError::NotFound) => Ok(None),
            Err(error) => Err(error),
        }
    }

    fn ensure_absent_unlocked(&self, name: ChildName<'_>) -> Result<(), PrivateFsError> {
        if self
            .open_optional_regular_unlocked(name, OpenPurpose::Read)?
            .is_some()
        {
            return Err(PrivateFsError::IdentityAmbiguous);
        }
        Ok(())
    }

    fn settle_rename_unlocked(
        &self,
        source: &PrivateComponent,
        destination: &PrivateComponent,
        source_file: VerifiedRegular,
    ) -> Result<FileIdentity, PrivateFsError> {
        let settlement = (|| {
            platform::sync_directory(&self.core.handle)?;
            let destination_file = self
                .open_optional_regular_unlocked(ChildName::Control(destination), OpenPurpose::Read)?
                .ok_or(PrivateFsError::IdentityAmbiguous)?;
            if source_file.identity != destination_file.identity {
                return Err(PrivateFsError::IdentityAmbiguous);
            }
            self.ensure_absent_unlocked(ChildName::Control(source))?;
            self.verify_boundary_unlocked()?;
            Ok(destination_file.identity)
        })();
        self.lease.settle_after_commit(settlement)
    }

    fn revalidate_failed_rename_unlocked(
        &self,
        source: &PrivateComponent,
        source_file: &VerifiedRegular,
    ) -> Result<(), PrivateFsError> {
        platform::revalidate_regular(
            &self.core.handle,
            &self.core.path,
            source.as_str(),
            &source_file.file,
            source_file.identity.0,
        )?;
        self.verify_boundary_unlocked()
    }

    fn settle_failed_mutation_unlocked<T>(
        &self,
        unchanged: Result<(), PrivateFsError>,
        syscall_error: PrivateFsError,
    ) -> Result<T, PrivateFsError> {
        if unchanged.is_ok() {
            Err(syscall_error)
        } else {
            self.lease
                .settle_after_commit(Err(PrivateFsError::SettlementUnknown))
        }
    }

    fn fail_streaming_write_unlocked<T>(
        &self,
        name: ChildName<'_>,
        verified: VerifiedRegular,
        clean_error: StreamingWriteError,
    ) -> Result<T, StreamingWriteError> {
        if self
            .cleanup_created_regular_unlocked(name, verified)
            .is_ok()
        {
            return Err(clean_error);
        }

        self.quarantine_streaming_write_unlocked()
    }

    fn quarantine_streaming_write_unlocked<T>(&self) -> Result<T, StreamingWriteError> {
        let settlement = self
            .lease
            .settle_after_commit::<()>(Err(PrivateFsError::SettlementUnknown));
        let error = settlement
            .err()
            .unwrap_or(PrivateFsError::SettlementUnknown);
        Err(StreamingWriteError::Filesystem(error))
    }

    fn cleanup_created_regular_unlocked(
        &self,
        name: ChildName<'_>,
        verified: VerifiedRegular,
    ) -> Result<(), PrivateFsError> {
        platform::revalidate_regular(
            &self.core.handle,
            &self.core.path,
            name.as_str(),
            &verified.file,
            verified.identity.0,
        )?;
        // `Read` is caller-controlled and may have invalidated the namespace
        // while the operation lease was held. Re-establish root, lock, and
        // directory authority immediately before the cleanup mutation. If
        // authority is ambiguous, leave the held inode as recovery residue and
        // let the caller quarantine the lease.
        self.verify_boundary_unlocked()?;
        self.remove_regular_unlocked(name)?;
        drop(verified);
        platform::sync_directory(&self.core.handle)?;
        #[cfg(test)]
        if self
            .lease
            .take_streaming_fault(crate::lease::StreamingFault::CleanupDirectorySync)
        {
            return Err(PrivateFsError::Io);
        }
        self.ensure_absent_unlocked(name)?;
        self.verify_boundary_unlocked()
    }
}

/// A private namespace held under one exact nonblocking cross-process lock.
pub struct LockedPrivateNamespace {
    directory: PrivateDirectory,
}

impl LockedPrivateNamespace {
    /// Creates or admits an absolute dedicated root and acquires the crate-owned
    /// canonical namespace lock.
    ///
    /// Missing ancestors are never created. Every existing ancestor must be a
    /// non-symlink directory accepted by the platform boundary checks. Root,
    /// administrators, and principals granted mutation through an ancestor ACL
    /// remain outside this boundary's guarantee. Unsupported Unix targets and
    /// Windows without its explicit debug validation feature fail before path
    /// inspection. The Windows validation adapter additionally requires local
    /// NTFS and protected explicit private-node DACLs.
    pub fn open_or_create(root: impl Into<PathBuf>) -> Result<Self, PrivateFsError> {
        Self::open(root.into(), true)
    }

    /// Opens an existing private namespace without creating its root or lock.
    ///
    /// Missing storage returns [`PrivateFsError::NotFound`]. Existing lock
    /// staging is checked and settled by the same exact-marker protocol as
    /// normal admission; payload files are never changed by this operation.
    pub fn open_existing(root: impl Into<PathBuf>) -> Result<Self, PrivateFsError> {
        Self::open(root.into(), false)
    }

    fn open(root: PathBuf, create: bool) -> Result<Self, PrivateFsError> {
        platform::admit_namespace_support()?;
        if !root.is_absolute() || root.file_name().is_none() {
            return Err(PrivateFsError::Unsafe);
        }
        let root_created = match fs::symlink_metadata(&root) {
            Ok(_) => false,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                if !create {
                    return Err(PrivateFsError::NotFound);
                }
                let parent = root.parent().ok_or(PrivateFsError::Unsafe)?;
                validate_directory_chain(parent, false)?;
                #[cfg(target_os = "windows")]
                {
                    let created = platform::create_private_root(&root)?;
                    if created && platform::sync_ancestor_directory(parent).is_err() {
                        return Err(PrivateFsError::SettlementUnknown);
                    }
                    created
                }
                #[cfg(not(target_os = "windows"))]
                {
                    let builder = private_directory_builder();
                    match builder.create(&root) {
                        Ok(()) => {
                            if platform::sync_ancestor_directory(parent).is_err() {
                                return Err(PrivateFsError::SettlementUnknown);
                            }
                            true
                        }
                        Err(create) if create.kind() == std::io::ErrorKind::AlreadyExists => false,
                        Err(create) => return Err(classify_root_create_error(&root, &create)),
                    }
                }
            }
            Err(_) => return Err(PrivateFsError::Io),
        };
        let core = match admit_root(root) {
            Ok(core) => core,
            Err(_) if root_created => return Err(PrivateFsError::SettlementUnknown),
            Err(error) => return Err(error),
        };
        let lock_component = canonical_lock_component()?;
        let lock_staging_component = lock_staging_component()?;
        let lock = if create {
            acquire_lock(&core, &lock_component, &lock_staging_component)?
        } else {
            let lock = open_lock_regular(&core, &lock_component)?;
            admit_existing_lock(&core, &lock_component, &lock_staging_component, lock)?
        };
        let lease_root_handle = core
            .handle
            .try_clone()
            .map_err(|_| PrivateFsError::SettlementUnknown)?;
        let lease = Arc::new(NamespaceLease::new(
            lock.file,
            lock.identity,
            lock_component,
            lock_staging_component,
            core.path.clone(),
            lease_root_handle,
            core.identity,
        ));
        Ok(Self {
            directory: PrivateDirectory {
                core,
                lease,
                role: DirectoryRole::Root,
            },
        })
    }

    /// Returns the held private root.
    #[must_use]
    pub const fn directory(&self) -> &PrivateDirectory {
        &self.directory
    }
}

struct VerifiedRegular {
    file: File,
    identity: FileIdentity,
}

fn admit_root(path: PathBuf) -> Result<DirectoryCore, PrivateFsError> {
    validate_directory_chain(&path, true)?;
    let (handle, identity) = platform::open_directory(&path)?;
    Ok(DirectoryCore {
        path,
        handle,
        identity: DirectoryIdentity(identity),
        parent: None,
    })
}

fn acquire_lock(
    directory: &DirectoryCore,
    lock_name: &PrivateComponent,
    lock_staging_name: &PrivateComponent,
) -> Result<VerifiedRegular, PrivateFsError> {
    match open_lock_regular(directory, lock_name) {
        Ok(lock) => admit_existing_lock(directory, lock_name, lock_staging_name, lock),
        Err(PrivateFsError::NotFound) => {
            initialize_or_recover_lock(directory, lock_name, lock_staging_name)
        }
        Err(error) => Err(error),
    }
}

fn open_lock_regular(
    directory: &DirectoryCore,
    name: &PrivateComponent,
) -> Result<VerifiedRegular, PrivateFsError> {
    let (file, identity) = platform::open_regular(
        &directory.handle,
        &directory.path,
        name.as_str(),
        OpenPurpose::Lock,
    )?;
    platform::verify_exact_name(&file, name.as_str())?;
    Ok(VerifiedRegular {
        file,
        identity: FileIdentity(identity),
    })
}

fn revalidate_exact_lock_regular(
    directory: &DirectoryCore,
    name: &PrivateComponent,
    regular: &VerifiedRegular,
) -> Result<(), PrivateFsError> {
    platform::revalidate_regular(
        &directory.handle,
        &directory.path,
        name.as_str(),
        &regular.file,
        regular.identity.0,
    )
}

fn open_required_lock_regular(
    directory: &DirectoryCore,
    name: &PrivateComponent,
) -> Result<VerifiedRegular, PrivateFsError> {
    match open_lock_regular(directory, name) {
        Err(PrivateFsError::NotFound) => Err(PrivateFsError::IdentityAmbiguous),
        outcome => outcome,
    }
}

fn admit_existing_lock(
    directory: &DirectoryCore,
    lock_name: &PrivateComponent,
    lock_staging_name: &PrivateComponent,
    mut lock: VerifiedRegular,
) -> Result<VerifiedRegular, PrivateFsError> {
    revalidate_exact_lock_regular(directory, lock_name, &lock)?;
    platform::lock_exclusive(&lock.file)?;
    // The descriptor-relative lock entry and root identity must still bind to
    // the held handles after flock succeeds, before the shared lease exists.
    revalidate_exact_lock_regular(directory, lock_name, &lock)?;
    if read_lock_content(&mut lock.file)? != LOCK_FILE_CONTENT {
        return Err(PrivateFsError::Unsafe);
    }
    revalidate_exact_lock_regular(directory, lock_name, &lock)?;
    verify_core_boundary(directory, DirectoryMode::Writable)?;
    inspect_and_cleanup_lock_staging(directory, lock_name, &mut lock, lock_staging_name)?;
    if read_lock_content(&mut lock.file)? != LOCK_FILE_CONTENT {
        return Err(PrivateFsError::Unsafe);
    }
    revalidate_exact_lock_regular(directory, lock_name, &lock)?;
    Ok(lock)
}

fn initialize_or_recover_lock(
    directory: &DirectoryCore,
    lock_name: &PrivateComponent,
    lock_staging_name: &PrivateComponent,
) -> Result<VerifiedRegular, PrivateFsError> {
    let (mut staging, created) = match platform::create_new_regular(
        &directory.handle,
        &directory.path,
        lock_staging_name.as_str(),
    ) {
        Ok((file, identity)) => {
            if platform::verify_exact_name(&file, lock_staging_name.as_str()).is_err() {
                // Create-new has committed. If the exact directory-entry name
                // cannot be proven, callers must not infer what residue exists.
                return Err(PrivateFsError::SettlementUnknown);
            }
            (
                VerifiedRegular {
                    file,
                    identity: FileIdentity(identity),
                },
                true,
            )
        }
        Err(PrivateFsError::AlreadyExists) => {
            // The staging entry may have been atomically published between the
            // canonical miss and this observation. Prefer the canonical entry
            // if it now exists; otherwise admit only the staging protocol's
            // exact marker or recoverable strict-prefix scratch state.
            match open_lock_regular(directory, lock_name) {
                Ok(lock) => {
                    return admit_existing_lock(directory, lock_name, lock_staging_name, lock);
                }
                Err(PrivateFsError::NotFound) => {}
                Err(error) => return Err(error),
            }
            match open_lock_regular(directory, lock_staging_name) {
                Ok(staging) => (staging, false),
                Err(PrivateFsError::NotFound) => {
                    let lock = open_required_lock_regular(directory, lock_name)?;
                    return admit_existing_lock(directory, lock_name, lock_staging_name, lock);
                }
                Err(error) => return Err(error),
            }
        }
        Err(error) => return Err(error),
    };

    let mut staging_mutated = created;
    let preparation = prepare_staged_lock(
        directory,
        lock_staging_name,
        &mut staging,
        &mut staging_mutated,
    );
    if let Err(error) = preparation {
        return if staging_mutated {
            Err(PrivateFsError::SettlementUnknown)
        } else {
            Err(error)
        };
    }

    match platform::atomic_publish_noreplace(
        &directory.handle,
        &directory.path,
        lock_staging_name.as_str(),
        lock_name.as_str(),
    ) {
        Ok(()) => settle_published_lock(directory, lock_name, lock_staging_name, staging),
        Err(PrivateFsError::AlreadyExists) if !staging_mutated => {
            // The existing, exact staging residue was not mutated. A racing
            // initializer won publication, so admission restarts at the only
            // authoritative name.
            let lock = open_required_lock_regular(directory, lock_name)?;
            admit_existing_lock(directory, lock_name, lock_staging_name, lock)
        }
        Err(_) => Err(PrivateFsError::SettlementUnknown),
    }
}

fn prepare_staged_lock(
    directory: &DirectoryCore,
    staging_name: &PrivateComponent,
    staging: &mut VerifiedRegular,
    mutated: &mut bool,
) -> Result<(), PrivateFsError> {
    revalidate_exact_lock_regular(directory, staging_name, staging)?;
    platform::lock_exclusive(&staging.file)?;
    revalidate_exact_lock_regular(directory, staging_name, staging)?;

    let bytes = read_lock_content(&mut staging.file)?;
    if bytes != LOCK_FILE_CONTENT {
        if !LOCK_FILE_CONTENT.starts_with(&bytes) {
            return Err(PrivateFsError::Unsafe);
        }
        // A fixed staging entry is reserved scratch state for lock
        // initialization. Under its exact inode lock, an empty file or strict
        // marker prefix is treated as recoverable scratch residue. Canonical
        // admission never applies this recovery rule.
        *mutated = true;
        staging.file.set_len(0).map_err(|_| PrivateFsError::Io)?;
        staging.file.rewind().map_err(|_| PrivateFsError::Io)?;
        write_all_private_regular(&mut staging.file, LOCK_FILE_CONTENT)?;
    }
    sync_private_regular(&staging.file)?;
    revalidate_exact_lock_regular(directory, staging_name, staging)?;
    if read_lock_content(&mut staging.file)? != LOCK_FILE_CONTENT {
        return Err(PrivateFsError::IdentityAmbiguous);
    }
    verify_core_boundary(directory, DirectoryMode::Writable)
}

fn settle_published_lock(
    directory: &DirectoryCore,
    lock_name: &PrivateComponent,
    lock_staging_name: &PrivateComponent,
    mut lock: VerifiedRegular,
) -> Result<VerifiedRegular, PrivateFsError> {
    let settlement = (|| {
        revalidate_exact_lock_regular(directory, lock_name, &lock)?;
        platform::sync_directory(&directory.handle)?;
        if read_lock_content(&mut lock.file)? != LOCK_FILE_CONTENT {
            return Err(PrivateFsError::IdentityAmbiguous);
        }
        revalidate_exact_lock_regular(directory, lock_name, &lock)?;
        verify_core_boundary(directory, DirectoryMode::Writable)?;
        inspect_and_cleanup_lock_staging(directory, lock_name, &mut lock, lock_staging_name)?;
        Ok(lock)
    })();
    settlement.map_err(|_| PrivateFsError::SettlementUnknown)
}

fn inspect_and_cleanup_lock_staging(
    directory: &DirectoryCore,
    lock_name: &PrivateComponent,
    lock: &mut VerifiedRegular,
    lock_staging_name: &PrivateComponent,
) -> Result<(), PrivateFsError> {
    let mut staging = match open_lock_regular(directory, lock_staging_name) {
        Ok(staging) => staging,
        Err(PrivateFsError::NotFound) => return Ok(()),
        Err(error) => return Err(error),
    };
    revalidate_exact_lock_regular(directory, lock_staging_name, &staging)?;
    platform::lock_exclusive(&staging.file)?;
    revalidate_exact_lock_regular(directory, lock_staging_name, &staging)?;
    let bytes = read_lock_content(&mut staging.file)?;
    revalidate_exact_lock_regular(directory, lock_staging_name, &staging)?;
    if bytes != LOCK_FILE_CONTENT && !LOCK_FILE_CONTENT.starts_with(&bytes) {
        return Err(PrivateFsError::Unsafe);
    }

    if platform::remove_regular(
        &directory.handle,
        &directory.path,
        lock_staging_name.as_str(),
    )
    .is_err()
    {
        return Err(PrivateFsError::SettlementUnknown);
    }
    let settlement = (|| {
        platform::sync_directory(&directory.handle)?;
        match open_lock_regular(directory, lock_staging_name) {
            Err(PrivateFsError::NotFound) => {}
            Ok(_) | Err(_) => return Err(PrivateFsError::IdentityAmbiguous),
        }
        revalidate_exact_lock_regular(directory, lock_name, lock)?;
        if read_lock_content(&mut lock.file)? != LOCK_FILE_CONTENT {
            return Err(PrivateFsError::IdentityAmbiguous);
        }
        verify_core_boundary(directory, DirectoryMode::Writable)
    })();
    settlement.map_err(|_| PrivateFsError::SettlementUnknown)
}

fn canonical_lock_component() -> Result<PrivateComponent, PrivateFsError> {
    PrivateComponent::new(LOCK_COMPONENT_NAME).map_err(|_| PrivateFsError::PrimitiveUnavailable)
}

fn lock_staging_component() -> Result<PrivateComponent, PrivateFsError> {
    PrivateComponent::new(LOCK_STAGING_COMPONENT_NAME)
        .map_err(|_| PrivateFsError::PrimitiveUnavailable)
}

fn read_lock_content(file: &mut File) -> Result<Vec<u8>, PrivateFsError> {
    file.rewind().map_err(|_| PrivateFsError::Io)?;
    let mut bytes = Vec::with_capacity(LOCK_FILE_CONTENT.len().saturating_add(1));
    let mut bounded = Read::by_ref(file).take(
        u64::try_from(LOCK_FILE_CONTENT.len())
            .unwrap_or(u64::MAX)
            .saturating_add(1),
    );
    bounded
        .read_to_end(&mut bytes)
        .map_err(|_| PrivateFsError::Io)?;
    Ok(bytes)
}

#[cfg(not(target_os = "windows"))]
fn classify_root_create_error(root: &Path, _error: &std::io::Error) -> PrivateFsError {
    let absent = matches!(
        fs::symlink_metadata(root),
        Err(observed) if observed.kind() == std::io::ErrorKind::NotFound
    );
    if absent
        && root
            .parent()
            .is_some_and(|parent| validate_directory_chain(parent, false).is_ok())
    {
        PrivateFsError::Io
    } else {
        PrivateFsError::SettlementUnknown
    }
}

fn with_verified_path_callback<T>(
    core: &DirectoryCore,
    lease: &Arc<NamespaceLease>,
    mode: DirectoryMode,
    callback: impl FnOnce(&Path) -> T,
) -> Result<T, PrivateFsError> {
    let operation = lease.begin()?;
    lease.observe(
        lease
            .verify_authority()
            .and_then(|()| verify_core_boundary(core, mode)),
    )?;
    let pin = lease.pin_path()?;
    drop(operation);

    let callback_result = callback(&core.path);

    let operation = match lease.begin() {
        Ok(operation) => operation,
        Err(error) => {
            pin.release();
            return Err(error);
        }
    };
    let validation = lease.observe(
        lease
            .verify_authority()
            .and_then(|()| verify_core_boundary(core, mode)),
    );
    pin.release();
    drop(operation);
    validation?;
    Ok(callback_result)
}

fn open_sealed_child_unlocked(
    parent: &DirectoryCore,
    lease: &Arc<NamespaceLease>,
    parent_mode: DirectoryMode,
    name: ChildName<'_>,
) -> Result<SealedPrivateDirectory, PrivateFsError> {
    let (handle, identity) =
        platform::open_sealed_child_directory(&parent.handle, &parent.path, name.as_str())?;
    platform::verify_exact_name(&handle, name.as_str())?;
    Ok(SealedPrivateDirectory {
        core: DirectoryCore {
            path: parent.path.join(name.as_str()),
            handle,
            identity: DirectoryIdentity(identity),
            parent: Some(ParentAuthority {
                path: parent.path.clone(),
                handle: parent.handle.try_clone().map_err(|_| PrivateFsError::Io)?,
                identity: parent.identity,
                child_name: name.to_owned_name(),
                mode: parent_mode,
            }),
        },
        lease: Arc::clone(lease),
    })
}

fn inspect_sealed_entry_unlocked(
    parent: &DirectoryCore,
    lease: &NamespaceLease,
    name: &PrivateEntryName,
) -> Result<Option<PrivateChildKind>, PrivateFsError> {
    let Some(observed) = lease.observe(platform::inspect_child(
        &parent.handle,
        &parent.path,
        name.as_str(),
    ))?
    else {
        return Ok(None);
    };

    match observed {
        platform::RawChildKind::Regular(expected) => {
            let opened = platform::open_sealed_regular(&parent.handle, &parent.path, name.as_str());
            let (file, identity) = match opened {
                Ok(opened) => opened,
                Err(PrivateFsError::Unsafe) => {
                    let unchanged = matches!(
                        platform::inspect_child(&parent.handle, &parent.path, name.as_str()),
                        Ok(Some(platform::RawChildKind::Regular(identity))) if identity == expected
                    );
                    if unchanged {
                        return Err(PrivateFsError::Unsafe);
                    }
                    return lease.observe(Err(PrivateFsError::IdentityAmbiguous));
                }
                Err(PrivateFsError::NotFound | PrivateFsError::IdentityAmbiguous) => {
                    return lease.observe(Err(PrivateFsError::IdentityAmbiguous));
                }
                Err(error) => return lease.observe(Err(error)),
            };
            if identity != expected {
                return lease.observe(Err(PrivateFsError::IdentityAmbiguous));
            }
            if platform::verify_exact_name(&file, name.as_str()).is_err() {
                return lease.observe(Err(PrivateFsError::IdentityAmbiguous));
            }
            Ok(Some(PrivateChildKind::RegularFile(FileIdentity(identity))))
        }
        platform::RawChildKind::Directory(expected) => {
            let opened =
                platform::open_sealed_child_directory(&parent.handle, &parent.path, name.as_str());
            let (directory, identity) = match opened {
                Ok(opened) => opened,
                Err(PrivateFsError::Unsafe) => {
                    let unchanged = matches!(
                        platform::inspect_child(&parent.handle, &parent.path, name.as_str()),
                        Ok(Some(platform::RawChildKind::Directory(identity))) if identity == expected
                    );
                    if unchanged {
                        return Err(PrivateFsError::Unsafe);
                    }
                    return lease.observe(Err(PrivateFsError::IdentityAmbiguous));
                }
                Err(PrivateFsError::NotFound | PrivateFsError::IdentityAmbiguous) => {
                    return lease.observe(Err(PrivateFsError::IdentityAmbiguous));
                }
                Err(error) => return lease.observe(Err(error)),
            };
            if identity != expected {
                return lease.observe(Err(PrivateFsError::IdentityAmbiguous));
            }
            if platform::verify_exact_name(&directory, name.as_str()).is_err() {
                return lease.observe(Err(PrivateFsError::IdentityAmbiguous));
            }
            Ok(Some(PrivateChildKind::Directory(DirectoryIdentity(
                identity,
            ))))
        }
    }
}

fn with_bounded_sealed_regular_reader_unlocked<T, E>(
    parent: &DirectoryCore,
    lease: &NamespaceLease,
    parent_mode: DirectoryMode,
    name: ChildName<'_>,
    limit: ByteLimit,
    callback: impl FnOnce(&mut dyn Read, u64) -> Result<T, E>,
) -> Result<Option<Result<T, E>>, PrivateFsError> {
    let opened = platform::open_sealed_regular(&parent.handle, &parent.path, name.as_str())
        .and_then(|(file, identity)| {
            platform::verify_exact_name(&file, name.as_str())?;
            Ok((file, identity))
        });
    let (mut file, identity) = match lease.observe(opened) {
        Ok(opened) => opened,
        Err(PrivateFsError::NotFound) => {
            lease.observe(
                lease
                    .verify_authority()
                    .and_then(|()| verify_core_boundary(parent, parent_mode)),
            )?;
            return Ok(None);
        }
        Err(error) => return Err(error),
    };
    #[cfg(test)]
    let initial_metadata =
        if lease.take_streaming_fault(crate::lease::StreamingFault::InitialReadMetadata) {
            Err(PrivateFsError::IdentityAmbiguous)
        } else {
            file.metadata()
                .map_err(|_| PrivateFsError::IdentityAmbiguous)
        };
    #[cfg(not(test))]
    let initial_metadata = file
        .metadata()
        .map_err(|_| PrivateFsError::IdentityAmbiguous);
    let initial_length = lease.observe(initial_metadata)?.len();
    if initial_length > u64::try_from(limit.get()).unwrap_or(u64::MAX) {
        lease.observe(validate_sealed_regular_after_read(
            parent,
            lease,
            parent_mode,
            name,
            &file,
            identity,
            initial_length,
        ))?;
        return Err(PrivateFsError::BoundExceeded);
    }

    let callback_result = {
        let mut bounded = Read::by_ref(&mut file).take(initial_length);
        callback(&mut bounded, initial_length)
    };
    lease.observe(validate_sealed_regular_after_read(
        parent,
        lease,
        parent_mode,
        name,
        &file,
        identity,
        initial_length,
    ))?;
    Ok(Some(callback_result))
}

fn validate_sealed_regular_after_read(
    parent: &DirectoryCore,
    lease: &NamespaceLease,
    parent_mode: DirectoryMode,
    name: ChildName<'_>,
    file: &File,
    identity: platform::RawIdentity,
    initial_length: u64,
) -> Result<(), PrivateFsError> {
    let final_length = file
        .metadata()
        .map_err(|_| PrivateFsError::IdentityAmbiguous)?
        .len();
    if final_length != initial_length {
        return Err(PrivateFsError::IdentityAmbiguous);
    }
    platform::revalidate_regular_mode(
        &parent.handle,
        &parent.path,
        name.as_str(),
        file,
        identity,
        RegularMode::Sealed,
    )?;
    lease.verify_authority()?;
    verify_core_boundary(parent, parent_mode)
}

fn terminal_settlement_error(lease: &NamespaceLease) -> PrivateFsError {
    lease
        .settle_after_commit::<()>(Err(PrivateFsError::SettlementUnknown))
        .err()
        .unwrap_or(PrivateFsError::SettlementUnknown)
}

enum ExistingDestination {
    Safe,
    Hostile,
}

#[cfg(all(test, any(target_os = "macos", target_os = "linux")))]
fn replace_published_directory_identity_for_test(
    destination_parent: &PrivateDirectory,
    destination: ChildName<'_>,
) -> Result<(), PrivateFsError> {
    use std::os::unix::fs::PermissionsExt;

    let installed = destination_parent.core.path.join(destination.as_str());
    let recovery = destination_parent
        .core
        .path
        .join(format!(".{}.identity-race", destination.as_str()));
    // macOS 15 refuses to rename a `0500` directory even within one parent.
    fs::set_permissions(&installed, fs::Permissions::from_mode(0o700))
        .map_err(|_| PrivateFsError::Io)?;
    fs::rename(&installed, &recovery).map_err(|_| PrivateFsError::Io)?;
    fs::set_permissions(&recovery, fs::Permissions::from_mode(0o500))
        .map_err(|_| PrivateFsError::Io)?;
    private_directory_builder()
        .create(&installed)
        .map_err(|_| PrivateFsError::Io)?;
    fs::set_permissions(&installed, fs::Permissions::from_mode(0o500))
        .map_err(|_| PrivateFsError::Io)
}

fn classify_existing_destination_unlocked(
    destination_parent: &PrivateDirectory,
    destination: ChildName<'_>,
) -> Result<ExistingDestination, PrivateFsError> {
    match platform::inspect_child(
        &destination_parent.core.handle,
        &destination_parent.core.path,
        destination.as_str(),
    ) {
        Ok(Some(_)) => Ok(ExistingDestination::Safe),
        Err(PrivateFsError::Unsafe) => Ok(ExistingDestination::Hostile),
        Ok(None) | Err(PrivateFsError::NotFound | PrivateFsError::IdentityAmbiguous) => {
            Err(PrivateFsError::IdentityAmbiguous)
        }
        Err(error) => Err(error),
    }
}

fn ensure_child_absent_unlocked(
    parent: &DirectoryCore,
    name: ChildName<'_>,
) -> Result<(), PrivateFsError> {
    match platform::inspect_child(&parent.handle, &parent.path, name.as_str())? {
        None => Ok(()),
        Some(_) => Err(PrivateFsError::IdentityAmbiguous),
    }
}

fn ensure_child_absent_owned_unlocked(parent: &ParentAuthority) -> Result<(), PrivateFsError> {
    match platform::inspect_child(&parent.handle, &parent.path, parent.child_name.as_str())? {
        None => Ok(()),
        Some(_) => Err(PrivateFsError::IdentityAmbiguous),
    }
}

fn verify_parent_authority(parent: &ParentAuthority) -> Result<(), PrivateFsError> {
    if !platform::same_open_identity(&parent.handle, parent.identity.0) {
        return Err(PrivateFsError::IdentityAmbiguous);
    }
    let (current, identity) = platform::open_directory_with_mode(&parent.path, parent.mode)
        .map_err(|_| PrivateFsError::IdentityAmbiguous)?;
    if identity != parent.identity.0 || !platform::same_open_identity(&current, identity) {
        return Err(PrivateFsError::IdentityAmbiguous);
    }
    Ok(())
}

enum RemoveEmptyFailure {
    Clean(PrivateFsError),
    Terminal(PrivateFsError),
}

fn remove_empty_writable(
    directory: PrivateDirectory,
) -> Result<(), PrivateFsTransitionError<PrivateDirectory>> {
    let operation = match directory.lease.begin() {
        Ok(operation) => operation,
        Err(error) => return Err(PrivateFsTransitionError::terminal(error)),
    };
    if let Err(error) = directory.precheck_unlocked() {
        drop(operation);
        return Err(PrivateFsTransitionError::terminal(error));
    }
    if let Err(error) = directory.lease.mutation_allowed() {
        drop(operation);
        return Err(PrivateFsTransitionError::recoverable(error, directory));
    }
    if directory.role == DirectoryRole::Root {
        drop(operation);
        return Err(PrivateFsTransitionError::recoverable(
            PrivateFsError::Unsafe,
            directory,
        ));
    }
    let result =
        remove_empty_core_unlocked(&directory.core, &directory.lease, DirectoryMode::Writable);
    drop(operation);
    match result {
        Ok(()) => Ok(()),
        Err(RemoveEmptyFailure::Clean(error)) => {
            Err(PrivateFsTransitionError::recoverable(error, directory))
        }
        Err(RemoveEmptyFailure::Terminal(error)) => Err(PrivateFsTransitionError::terminal(error)),
    }
}

fn remove_empty_sealed(
    directory: SealedPrivateDirectory,
) -> Result<(), PrivateFsTransitionError<SealedPrivateDirectory>> {
    let operation = match directory.lease.begin() {
        Ok(operation) => operation,
        Err(error) => return Err(PrivateFsTransitionError::terminal(error)),
    };
    if let Err(error) = directory.precheck_unlocked() {
        drop(operation);
        return Err(PrivateFsTransitionError::terminal(error));
    }
    if let Err(error) = directory.lease.mutation_allowed() {
        drop(operation);
        return Err(PrivateFsTransitionError::recoverable(error, directory));
    }
    let result =
        remove_empty_core_unlocked(&directory.core, &directory.lease, DirectoryMode::Sealed);
    drop(operation);
    match result {
        Ok(()) => Ok(()),
        Err(RemoveEmptyFailure::Clean(error)) => {
            Err(PrivateFsTransitionError::recoverable(error, directory))
        }
        Err(RemoveEmptyFailure::Terminal(error)) => Err(PrivateFsTransitionError::terminal(error)),
    }
}

fn remove_empty_core_unlocked(
    core: &DirectoryCore,
    lease: &NamespaceLease,
    mode: DirectoryMode,
) -> Result<(), RemoveEmptyFailure> {
    let Some(parent) = core.parent.as_ref() else {
        return Err(RemoveEmptyFailure::Clean(PrivateFsError::Unsafe));
    };
    let precheck = lease
        .verify_authority()
        .and_then(|()| verify_core_boundary(core, mode));
    if let Err(error) = lease.observe(precheck) {
        return Err(RemoveEmptyFailure::Terminal(error));
    }
    if parent.mode != DirectoryMode::Writable {
        return Err(RemoveEmptyFailure::Clean(PrivateFsError::Unsafe));
    }
    match platform::directory_is_empty(&core.handle) {
        Ok(true) => {}
        Ok(false) => {
            return if verify_core_boundary(core, mode)
                .and_then(|()| lease.verify_authority())
                .is_ok()
            {
                Err(RemoveEmptyFailure::Clean(PrivateFsError::DirectoryNotEmpty))
            } else {
                Err(RemoveEmptyFailure::Terminal(
                    lease
                        .observe::<()>(Err(PrivateFsError::IdentityAmbiguous))
                        .err()
                        .unwrap_or(PrivateFsError::IdentityAmbiguous),
                ))
            };
        }
        Err(error) => {
            return if verify_core_boundary(core, mode)
                .and_then(|()| lease.verify_authority())
                .is_ok()
            {
                Err(RemoveEmptyFailure::Clean(error))
            } else {
                Err(RemoveEmptyFailure::Terminal(terminal_settlement_error(
                    lease,
                )))
            };
        }
    }
    #[cfg(test)]
    if lease.take_remove_empty_race() {
        match platform::create_new_regular(&core.handle, &core.path, "remove-race-entry") {
            Ok((file, _)) => drop(file),
            Err(_) => {
                return Err(RemoveEmptyFailure::Terminal(terminal_settlement_error(
                    lease,
                )));
            }
        }
    }
    if let Err(error) = verify_core_boundary(core, mode).and_then(|()| lease.verify_authority()) {
        let error = lease.observe::<()>(Err(error)).err().unwrap_or(error);
        return Err(RemoveEmptyFailure::Terminal(error));
    }

    let removed =
        platform::remove_directory(&parent.handle, &parent.path, parent.child_name.as_str());
    #[cfg(test)]
    let removed = removed.and_then(|()| {
        if lease.take_lifecycle_fault(crate::lease::LifecycleFault::RemoveDirectoryCommitted) {
            Err(PrivateFsError::Io)
        } else {
            Ok(())
        }
    });
    if let Err(error) = removed {
        return if verify_core_boundary(core, mode)
            .and_then(|()| lease.verify_authority())
            .is_ok()
        {
            Err(RemoveEmptyFailure::Clean(error))
        } else {
            Err(RemoveEmptyFailure::Terminal(terminal_settlement_error(
                lease,
            )))
        };
    }

    let settlement = (|| {
        platform::sync_directory(&parent.handle)?;
        #[cfg(test)]
        if lease.take_lifecycle_fault(crate::lease::LifecycleFault::RemoveDirectoryParentSync) {
            return Err(PrivateFsError::Io);
        }
        ensure_child_absent_owned_unlocked(parent)?;
        verify_parent_authority(parent)?;
        lease.verify_authority()
    })();
    if settlement.is_ok() {
        Ok(())
    } else {
        Err(RemoveEmptyFailure::Terminal(terminal_settlement_error(
            lease,
        )))
    }
}

fn verify_core_boundary(
    directory: &DirectoryCore,
    mode: DirectoryMode,
) -> Result<(), PrivateFsError> {
    if !platform::same_open_identity(&directory.handle, directory.identity.0) {
        return Err(PrivateFsError::IdentityAmbiguous);
    }
    if let Some(parent) = &directory.parent {
        if !platform::same_open_identity(&parent.handle, parent.identity.0) {
            return Err(PrivateFsError::IdentityAmbiguous);
        }
        let (current_parent, current_parent_identity) =
            platform::open_directory_with_mode(&parent.path, parent.mode)
                .map_err(|_| PrivateFsError::IdentityAmbiguous)?;
        if current_parent_identity != parent.identity.0
            || !platform::same_open_identity(&current_parent, parent.identity.0)
        {
            return Err(PrivateFsError::IdentityAmbiguous);
        }
        platform::revalidate_child_directory(
            &parent.handle,
            &parent.path,
            parent.child_name.as_str(),
            &directory.handle,
            directory.identity.0,
            mode,
        )
        .map_err(|_| PrivateFsError::IdentityAmbiguous)?;
        if directory.path != parent.path.join(parent.child_name.as_str()) {
            return Err(PrivateFsError::IdentityAmbiguous);
        }
    } else {
        let (current, identity) = platform::open_directory_with_mode(&directory.path, mode)
            .map_err(|_| PrivateFsError::IdentityAmbiguous)?;
        if identity != directory.identity.0 || !platform::same_open_identity(&current, identity) {
            return Err(PrivateFsError::IdentityAmbiguous);
        }
    }
    Ok(())
}

fn validate_directory_chain(path: &Path, private_leaf: bool) -> Result<(), PrivateFsError> {
    if !path.is_absolute() {
        return Err(PrivateFsError::Unsafe);
    }
    let mut ancestors: Vec<&Path> = path.ancestors().collect();
    ancestors.reverse();
    for current in ancestors {
        if current.as_os_str().is_empty() {
            continue;
        }
        let metadata = fs::symlink_metadata(current).map_err(|_| PrivateFsError::Unsafe)?;
        if private_leaf && current == path {
            platform::validate_private_directory_node(current, &metadata)?;
        } else {
            platform::validate_ancestor_node(current, &metadata)?;
        }
    }
    Ok(())
}

#[cfg(not(target_os = "windows"))]
fn private_directory_builder() -> fs::DirBuilder {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;

        let mut builder = fs::DirBuilder::new();
        builder.mode(0o700);
        builder
    }
    #[cfg(not(unix))]
    {
        fs::DirBuilder::new()
    }
}

#[cfg(all(test, any(target_os = "macos", target_os = "linux")))]
mod tests {
    #[test]
    fn opening_existing_namespace_never_creates_missing_storage() {
        let parent = tempfile::tempdir().unwrap();
        let root = parent.path().canonicalize().unwrap().join("missing");
        assert!(matches!(
            LockedPrivateNamespace::open_existing(&root),
            Err(PrivateFsError::NotFound)
        ));
        assert!(!root.exists());
        private_directory_builder().create(&root).unwrap();
        assert!(matches!(
            LockedPrivateNamespace::open_existing(&root),
            Err(PrivateFsError::NotFound)
        ));
        assert_eq!(fs::read_dir(&root).unwrap().count(), 0);
        let namespace = LockedPrivateNamespace::open_or_create(&root).unwrap();
        let record = PrivateComponent::new("record").unwrap();
        namespace
            .directory()
            .write_new_synced(&record, b"inert", ByteLimit::new(5).unwrap())
            .unwrap();
        drop(namespace);
        let reopened = LockedPrivateNamespace::open_existing(&root).unwrap();
        assert_eq!(
            reopened
                .directory()
                .read_bounded_regular(&record, ByteLimit::new(5).unwrap())
                .unwrap(),
            Some(b"inert".to_vec())
        );
    }

    #[test]
    fn metadata_length_is_bounded_for_writable_and_sealed_files() {
        let (_parent, namespace) = test_namespace("file-length-test");
        let child = namespace
            .directory
            .create_new_private_child(&PrivateComponent::new("child").unwrap())
            .unwrap();
        let name = PrivateEntryName::new("payload").unwrap();
        child
            .write_new_entry_synced(&name, b"exact bytes", ByteLimit::new(11).unwrap())
            .unwrap();
        assert_eq!(
            child.entry_regular_length(&name, ByteLimit::new(11).unwrap()),
            Ok(Some(11))
        );
        assert_eq!(
            child.entry_regular_length(&name, ByteLimit::new(10).unwrap()),
            Err(PrivateFsError::BoundExceeded)
        );
        child.seal_verified_entry_regular(&name).unwrap().unwrap();
        let sealed = child.seal().unwrap();
        assert_eq!(
            sealed.entry_regular_length(&name, ByteLimit::new(11).unwrap()),
            Ok(Some(11))
        );
        assert_eq!(
            sealed.entry_regular_length(&name, ByteLimit::new(10).unwrap()),
            Err(PrivateFsError::BoundExceeded)
        );
        assert_eq!(
            sealed.entry_regular_length(
                &PrivateEntryName::new("absent").unwrap(),
                ByteLimit::new(11).unwrap()
            ),
            Ok(None)
        );
        sealed
            .unseal()
            .unwrap()
            .remove_verified_entry_regular(&name)
            .unwrap();
    }

    use super::*;
    use crate::lease::{CommittedMutationFault, LifecycleFault, StreamingFault};
    use std::fs::OpenOptions;
    use std::io::Cursor;

    struct RenameOnFirstRead {
        bytes: Cursor<Vec<u8>>,
        source: PathBuf,
        destination: PathBuf,
        renamed: bool,
    }

    impl Read for RenameOnFirstRead {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            let read = self.bytes.read(buffer)?;
            if read != 0 && !self.renamed {
                fs::rename(&self.source, &self.destination)?;
                self.renamed = true;
            }
            Ok(read)
        }
    }

    struct RemoveOnFirstRead {
        bytes: Cursor<Vec<u8>>,
        target: PathBuf,
        removed: bool,
    }

    impl Read for RemoveOnFirstRead {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            let read = self.bytes.read(buffer)?;
            if read != 0 && !self.removed {
                fs::remove_file(&self.target)?;
                self.removed = true;
            }
            Ok(read)
        }
    }

    #[derive(Clone, Copy)]
    enum EofMutation {
        Append,
        Truncate,
    }

    struct MutateOnEof {
        bytes: Cursor<Vec<u8>>,
        target: PathBuf,
        mutation: EofMutation,
        mutated: bool,
    }

    impl Read for MutateOnEof {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            let read = self.bytes.read(buffer)?;
            if read == 0 && !self.mutated {
                let mut options = OpenOptions::new();
                options.write(true);
                match self.mutation {
                    EofMutation::Append => {
                        options.append(true).open(&self.target)?.write_all(b"x")?;
                    }
                    EofMutation::Truncate => {
                        options.truncate(true).open(&self.target)?;
                    }
                }
                self.mutated = true;
            }
            Ok(read)
        }
    }

    fn test_namespace(name: &str) -> (tempfile::TempDir, LockedPrivateNamespace) {
        let parent = tempfile::tempdir_in(std::env::current_dir().unwrap()).unwrap();
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(parent.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let namespace = LockedPrivateNamespace::open_or_create(parent.path().join(name)).unwrap();
        (parent, namespace)
    }

    /// Moves a sealed directory aside as an external actor would. macOS 15
    /// requires write permission on the moved directory even for a
    /// same-parent rename, so the `0500` mode is lifted and then restored.
    fn rename_sealed_directory(from: &Path, to: &Path) {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(from, fs::Permissions::from_mode(0o700)).unwrap();
        fs::rename(from, to).unwrap();
        fs::set_permissions(to, fs::Permissions::from_mode(0o500)).unwrap();
    }

    fn transition_error_while_pinned<T, S>(
        lease: &Arc<NamespaceLease>,
        transition: impl FnOnce() -> Result<T, PrivateFsTransitionError<S>>,
    ) -> PrivateFsTransitionError<S> {
        let operation = lease.begin().unwrap();
        let pin = lease.pin_path().unwrap();
        drop(operation);
        let error = transition().err().unwrap();
        drop(pin);
        error
    }

    fn assert_terminal_identity_ambiguity<S>(error: PrivateFsTransitionError<S>) {
        let (kind, state) = error.into_parts();
        assert_eq!(kind, PrivateFsError::IdentityAmbiguous);
        assert!(
            state.is_none(),
            "terminal transition returned stale authority"
        );
    }

    #[test]
    fn reported_post_chmod_directory_failure_is_terminal_and_quarantines() {
        let (_parent, namespace) = test_namespace("seal-commit-fault-test");
        let child = namespace
            .directory
            .create_new_private_child(&PrivateComponent::new("child").unwrap())
            .unwrap();
        child
            .lease
            .inject_lifecycle_fault(crate::lease::LifecycleFault::SealDirectoryCommitted);

        let error = child.seal().err().unwrap();
        assert_eq!(error.error(), PrivateFsError::SettlementUnknown);
        assert!(!error.is_recoverable());
        assert_eq!(
            namespace.directory.list_components(4),
            Err(PrivateFsError::Quarantined)
        );
    }

    #[test]
    fn committed_unseal_failure_returns_no_capability_and_quarantines() {
        use std::os::unix::fs::PermissionsExt;

        let (parent, namespace) = test_namespace("unseal-commit-fault-test");
        let child = namespace
            .directory
            .create_new_private_child(&PrivateComponent::new("child").unwrap())
            .unwrap()
            .seal()
            .unwrap();
        child
            .lease
            .inject_lifecycle_fault(LifecycleFault::UnsealDirectoryCommitted);

        let error = child.unseal().err().unwrap();
        let (kind, state) = error.into_parts();
        assert_eq!(kind, PrivateFsError::SettlementUnknown);
        assert!(state.is_none());
        assert_eq!(
            fs::metadata(parent.path().join("unseal-commit-fault-test/child"))
                .unwrap()
                .permissions()
                .mode()
                & 0o7777,
            0o700
        );
        assert_eq!(
            namespace.directory.list_components(4),
            Err(PrivateFsError::Quarantined)
        );
    }

    #[test]
    fn committed_regular_seal_failure_is_terminal_and_quarantines() {
        use std::os::unix::fs::PermissionsExt;

        let (parent, namespace) = test_namespace("regular-seal-commit-fault-test");
        let child = namespace
            .directory
            .create_new_private_child(&PrivateComponent::new("child").unwrap())
            .unwrap();
        let payload = PrivateComponent::new("payload.bin").unwrap();
        child
            .write_new_synced(&payload, b"payload", ByteLimit::new(16).unwrap())
            .unwrap();
        child
            .lease
            .inject_lifecycle_fault(LifecycleFault::SealRegularCommitted);

        assert_eq!(
            child.seal_verified_regular(&payload),
            Err(PrivateFsError::SettlementUnknown)
        );
        assert_eq!(
            fs::metadata(
                parent
                    .path()
                    .join("regular-seal-commit-fault-test/child/payload.bin")
            )
            .unwrap()
            .permissions()
            .mode()
                & 0o7777,
            0o400
        );
        assert_eq!(
            namespace.directory.list_components(4),
            Err(PrivateFsError::Quarantined)
        );
    }

    #[test]
    fn every_consuming_transition_reproves_stale_authority_before_pin_refusal() {
        {
            let (parent, namespace) = test_namespace("stale-pinned-seal-test");
            let name = PrivateComponent::new("child").unwrap();
            let child = namespace.directory.create_new_private_child(&name).unwrap();
            fs::rename(
                parent.path().join("stale-pinned-seal-test/child"),
                parent.path().join("stale-pinned-seal-test/moved"),
            )
            .unwrap();
            let lease = Arc::clone(&child.lease);
            let error = transition_error_while_pinned(&lease, || child.seal());
            assert_terminal_identity_ambiguity(error);
            assert_eq!(
                namespace.directory.list_components(4),
                Err(PrivateFsError::Quarantined)
            );
        }

        {
            let (parent, namespace) = test_namespace("stale-pinned-unseal-test");
            let name = PrivateComponent::new("child").unwrap();
            let child = namespace
                .directory
                .create_new_private_child(&name)
                .unwrap()
                .seal()
                .unwrap();
            rename_sealed_directory(
                &parent.path().join("stale-pinned-unseal-test/child"),
                &parent.path().join("stale-pinned-unseal-test/moved"),
            );
            let lease = Arc::clone(&child.lease);
            let error = transition_error_while_pinned(&lease, || child.unseal());
            assert_terminal_identity_ambiguity(error);
            assert_eq!(
                namespace.directory.list_components(4),
                Err(PrivateFsError::Quarantined)
            );
        }

        {
            let (parent, namespace) = test_namespace("stale-pinned-publish-test");
            let container = namespace
                .directory
                .create_new_private_child(&PrivateComponent::new("objects").unwrap())
                .unwrap();
            let child = container
                .create_new_private_child(&PrivateComponent::new("source").unwrap())
                .unwrap()
                .seal()
                .unwrap();
            rename_sealed_directory(
                &parent
                    .path()
                    .join("stale-pinned-publish-test/objects/source"),
                &parent
                    .path()
                    .join("stale-pinned-publish-test/objects/moved"),
            );
            let lease = Arc::clone(&child.lease);
            let error = transition_error_while_pinned(&lease, || {
                child.publish_noreplace(&container, &PrivateComponent::new("installed").unwrap())
            });
            assert_terminal_identity_ambiguity(error);
            assert_eq!(
                namespace.directory.list_components(4),
                Err(PrivateFsError::Quarantined)
            );
        }

        {
            let (parent, namespace) = test_namespace("stale-pinned-writable-remove-test");
            let child = namespace
                .directory
                .create_new_private_child(&PrivateComponent::new("child").unwrap())
                .unwrap();
            fs::rename(
                parent
                    .path()
                    .join("stale-pinned-writable-remove-test/child"),
                parent
                    .path()
                    .join("stale-pinned-writable-remove-test/moved"),
            )
            .unwrap();
            let lease = Arc::clone(&child.lease);
            let error = transition_error_while_pinned(&lease, || child.remove_empty());
            assert_terminal_identity_ambiguity(error);
            assert_eq!(
                namespace.directory.list_components(4),
                Err(PrivateFsError::Quarantined)
            );
        }

        {
            let (parent, namespace) = test_namespace("stale-pinned-sealed-remove-test");
            let child = namespace
                .directory
                .create_new_private_child(&PrivateComponent::new("child").unwrap())
                .unwrap()
                .seal()
                .unwrap();
            rename_sealed_directory(
                &parent.path().join("stale-pinned-sealed-remove-test/child"),
                &parent.path().join("stale-pinned-sealed-remove-test/moved"),
            );
            let lease = Arc::clone(&child.lease);
            let error = transition_error_while_pinned(&lease, || child.remove_empty());
            assert_terminal_identity_ambiguity(error);
            assert_eq!(
                namespace.directory.list_components(4),
                Err(PrivateFsError::Quarantined)
            );
        }
    }

    #[test]
    fn sealed_frontier_identity_ambiguity_never_returns_writable_authority() {
        let (_parent, namespace) = test_namespace("seal-frontier-identity-test");
        let child = namespace
            .directory
            .create_new_private_child(&PrivateComponent::new("child").unwrap())
            .unwrap();
        let payload = PrivateComponent::new("payload.bin").unwrap();
        child
            .write_new_synced(&payload, b"payload", ByteLimit::new(16).unwrap())
            .unwrap();
        child.seal_verified_regular(&payload).unwrap().unwrap();
        child.lease.inject_seal_child_identity_fault();

        let error = child.seal().err().unwrap();
        assert_terminal_identity_ambiguity(error);
        assert_eq!(
            namespace.directory.list_components(4),
            Err(PrivateFsError::Quarantined)
        );
    }

    #[test]
    fn remove_child_identity_observation_stickily_quarantines() {
        let (_parent, namespace) = test_namespace("remove-open-identity-test");
        let name = PrivateComponent::new("child").unwrap();
        let child = namespace.directory.create_new_private_child(&name).unwrap();
        namespace
            .directory
            .lease
            .inject_lifecycle_fault(LifecycleFault::RemoveChildOpenIdentity);

        assert_eq!(
            namespace.directory.remove_empty_private_child(&name),
            Err(PrivateFsError::IdentityAmbiguous)
        );
        assert_eq!(
            namespace.directory.list_components(4),
            Err(PrivateFsError::Quarantined)
        );
        drop(child);
    }

    #[test]
    fn empty_removal_race_returns_directory_not_empty_with_capability() {
        let (parent, namespace) = test_namespace("remove-empty-race-test");
        let name = PrivateComponent::new("child").unwrap();
        let child = namespace.directory.create_new_private_child(&name).unwrap();
        child.lease.inject_remove_empty_race();

        let error = child.remove_empty().err().unwrap();
        let (kind, child) = error.into_parts();
        assert_eq!(kind, PrivateFsError::DirectoryNotEmpty);
        let child = child.expect("rmdir race did not commit removal");
        assert_eq!(
            child.list_components(4).unwrap(),
            vec![PrivateComponent::new("remove-race-entry").unwrap()]
        );
        assert!(parent
            .path()
            .join("remove-empty-race-test/child/remove-race-entry")
            .is_file());
    }

    #[test]
    fn same_parent_publication_syncs_once_and_settles_identity() {
        let (_parent, namespace) = test_namespace("same-parent-publish-test");
        let container = namespace
            .directory
            .create_new_private_child(&PrivateComponent::new("objects").unwrap())
            .unwrap();
        let source = container
            .create_new_private_child(&PrivateComponent::new("source").unwrap())
            .unwrap()
            .seal()
            .unwrap();
        let identity = source.identity();
        source
            .lease
            .inject_lifecycle_fault(LifecycleFault::PublishDestinationParentSync);

        let installed = source
            .publish_noreplace(&container, &PrivateComponent::new("installed").unwrap())
            .unwrap();
        assert_eq!(installed.identity(), identity);
        assert_eq!(installed.lease.same_parent_publish_sync_count(), 1);
        installed.with_verified_path(|_| ()).unwrap();
    }

    #[test]
    fn same_parent_publication_sync_failure_is_terminal() {
        let (parent, namespace) = test_namespace("same-parent-sync-fault-test");
        let container = namespace
            .directory
            .create_new_private_child(&PrivateComponent::new("objects").unwrap())
            .unwrap();
        let source = container
            .create_new_private_child(&PrivateComponent::new("source").unwrap())
            .unwrap()
            .seal()
            .unwrap();
        source
            .lease
            .inject_lifecycle_fault(LifecycleFault::PublishSameParentSync);

        let error = source
            .publish_noreplace(&container, &PrivateComponent::new("installed").unwrap())
            .err()
            .unwrap();
        assert_eq!(error.error(), PrivateFsError::SettlementUnknown);
        assert!(!error.is_recoverable());
        assert!(parent
            .path()
            .join("same-parent-sync-fault-test/objects/installed")
            .is_dir());
        assert_eq!(
            namespace.directory.list_components(4),
            Err(PrivateFsError::Quarantined)
        );
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn publication_root_committed_frontiers_quarantine_without_capability() {
        use std::os::unix::fs::PermissionsExt;
        for (label, fault, committed_name) in [
            (
                "writable",
                LifecycleFault::PublishRootWritableCommitted,
                "source",
            ),
            (
                "reseal",
                LifecycleFault::PublishRootResealCommitted,
                "installed",
            ),
            ("sync", LifecycleFault::PublishRootResealSync, "installed"),
            (
                "rename",
                LifecycleFault::PublishRenameCommitted,
                "installed",
            ),
        ] {
            let namespace_name = format!("publish-root-{label}-fault-test");
            let (_parent, namespace) = test_namespace(&namespace_name);
            let container = namespace
                .directory
                .create_new_private_child(&PrivateComponent::new("objects").unwrap())
                .unwrap();
            #[cfg(target_os = "linux")]
            let destination_directory = container
                .create_new_private_child(&PrivateComponent::new("destination").unwrap())
                .unwrap();
            #[cfg(target_os = "linux")]
            let destination_parent = &destination_directory;
            #[cfg(target_os = "macos")]
            let destination_parent = &container;
            let source = container
                .create_new_private_child(&PrivateComponent::new("source").unwrap())
                .unwrap()
                .seal()
                .unwrap();
            source.lease.inject_lifecycle_fault(fault);
            let error = source
                .publish_noreplace(
                    destination_parent,
                    &PrivateComponent::new("installed").unwrap(),
                )
                .err()
                .unwrap();
            assert_eq!(error.error(), PrivateFsError::SettlementUnknown);
            assert!(!error.is_recoverable());
            assert_eq!(
                namespace.directory.list_components(4),
                Err(PrivateFsError::Quarantined)
            );
            let committed_parent = if committed_name == "source" {
                &container.core.path
            } else {
                &destination_parent.core.path
            };
            let path = committed_parent.join(committed_name);
            assert_eq!(
                std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
                0o500
            );
        }
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn publication_native_refusal_returns_only_freshly_resealed_source() {
        let (_parent, namespace) = test_namespace("publish-root-refusal-test");
        let container = namespace
            .directory
            .create_new_private_child(&PrivateComponent::new("objects").unwrap())
            .unwrap();
        #[cfg(target_os = "linux")]
        let destination_directory = container
            .create_new_private_child(&PrivateComponent::new("destination").unwrap())
            .unwrap();
        #[cfg(target_os = "linux")]
        let destination_parent = &destination_directory;
        #[cfg(target_os = "macos")]
        let destination_parent = &container;
        let source = container
            .create_new_private_child(&PrivateComponent::new("source").unwrap())
            .unwrap()
            .seal()
            .unwrap();
        let identity = source.identity();
        source
            .lease
            .inject_lifecycle_fault(LifecycleFault::PublishRenameRefused);
        let error = source
            .publish_noreplace(
                destination_parent,
                &PrivateComponent::new("installed").unwrap(),
            )
            .err()
            .unwrap();
        let (kind, source) = error.into_parts();
        assert_eq!(kind, PrivateFsError::Io);
        let source = source.expect("confirmed refusal returns resealed original authority");
        assert_eq!(source.identity(), identity);
        source.with_verified_path(|_| ()).unwrap();
        assert_eq!(
            destination_parent
                .open_sealed_private_child(&PrivateComponent::new("installed").unwrap(),)
                .err(),
            Some(PrivateFsError::NotFound)
        );
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn publication_reseal_refusal_never_recovers_or_admits_crash_residue_as_sealed() {
        use std::os::unix::fs::PermissionsExt;
        for rename_refused in [false, true] {
            let (parent, namespace) = test_namespace("publish-root-reseal-refusal-test");
            let container = namespace
                .directory
                .create_new_private_child(&PrivateComponent::new("objects").unwrap())
                .unwrap();
            #[cfg(target_os = "linux")]
            let destination_directory = container
                .create_new_private_child(&PrivateComponent::new("destination").unwrap())
                .unwrap();
            #[cfg(target_os = "linux")]
            let destination_parent = &destination_directory;
            #[cfg(target_os = "macos")]
            let destination_parent = &container;
            let source = container
                .create_new_private_child(&PrivateComponent::new("source").unwrap())
                .unwrap()
                .seal()
                .unwrap();
            source
                .lease
                .inject_lifecycle_fault(LifecycleFault::PublishRootResealRefused);
            if rename_refused {
                source
                    .lease
                    .inject_lifecycle_fault(LifecycleFault::PublishRenameRefused);
            }
            let error = source
                .publish_noreplace(
                    destination_parent,
                    &PrivateComponent::new("installed").unwrap(),
                )
                .err()
                .unwrap();
            assert_eq!(error.error(), PrivateFsError::SettlementUnknown);
            assert!(!error.is_recoverable());
            assert_eq!(
                namespace.directory.list_components(4),
                Err(PrivateFsError::Quarantined)
            );
            let name = PrivateComponent::new(if rename_refused {
                "source"
            } else {
                "installed"
            })
            .unwrap();
            let root = parent.path().join("publish-root-reseal-refusal-test");
            let committed_parent = if rename_refused {
                container.core.path.clone()
            } else {
                destination_parent.core.path.clone()
            };
            assert_eq!(
                std::fs::metadata(committed_parent.join(name.as_str()))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o700
            );
            #[cfg(target_os = "linux")]
            drop(destination_directory);
            drop(container);
            drop(namespace);
            // A reopened process gets no sealed capability for incomplete
            // publication. The existing explicit mixed-mode recovery surface
            // remains the only way to handle the privately owned residue.
            let reopened = LockedPrivateNamespace::open_or_create(root).unwrap();
            let objects = reopened
                .directory
                .open_private_child(&PrivateComponent::new("objects").unwrap())
                .unwrap();
            #[cfg(target_os = "linux")]
            let objects = if rename_refused {
                objects
            } else {
                objects
                    .open_private_child(&PrivateComponent::new("destination").unwrap())
                    .unwrap()
            };
            assert!(objects.open_sealed_private_child(&name).is_err());
            assert!(matches!(
                objects.open_private_child_any_mode(&name),
                Ok(OpenedPrivateDirectory::Writable(_))
            ));
        }
    }

    #[test]
    fn destination_identity_substitution_after_publication_is_terminal() {
        let (parent, namespace) = test_namespace("publish-identity-fault-test");
        let container = namespace
            .directory
            .create_new_private_child(&PrivateComponent::new("objects").unwrap())
            .unwrap();
        let source = container
            .create_new_private_child(&PrivateComponent::new("source").unwrap())
            .unwrap()
            .seal()
            .unwrap();
        source
            .lease
            .inject_lifecycle_fault(LifecycleFault::PublishDestinationIdentity);

        let error = source
            .publish_noreplace(&container, &PrivateComponent::new("installed").unwrap())
            .err()
            .unwrap();
        assert_eq!(error.error(), PrivateFsError::SettlementUnknown);
        assert!(parent
            .path()
            .join("publish-identity-fault-test/objects/installed")
            .is_dir());
        assert!(parent
            .path()
            .join("publish-identity-fault-test/objects/.installed.identity-race")
            .is_dir());
        assert_eq!(
            namespace.directory.list_components(4),
            Err(PrivateFsError::Quarantined)
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn distinct_parent_publication_quarantines_on_each_parent_sync_fault() {
        for (suffix, fault) in [
            ("source", LifecycleFault::PublishSourceParentSync),
            ("destination", LifecycleFault::PublishDestinationParentSync),
        ] {
            let namespace_name = format!("distinct-parent-{suffix}-sync-test");
            let (_parent, namespace) = test_namespace(&namespace_name);
            let source_parent = namespace
                .directory
                .create_new_private_child(&PrivateComponent::new("staging").unwrap())
                .unwrap();
            let destination_parent = namespace
                .directory
                .create_new_private_child(&PrivateComponent::new("objects").unwrap())
                .unwrap();
            let source = source_parent
                .create_new_private_child(&PrivateComponent::new("source").unwrap())
                .unwrap()
                .seal()
                .unwrap();
            source.lease.inject_lifecycle_fault(fault);

            let error = source
                .publish_noreplace(
                    &destination_parent,
                    &PrivateComponent::new("installed").unwrap(),
                )
                .err()
                .unwrap();
            assert_eq!(error.error(), PrivateFsError::SettlementUnknown);
            assert_eq!(
                namespace.directory.list_components(4),
                Err(PrivateFsError::Quarantined)
            );
        }
    }

    #[test]
    fn every_seal_and_unseal_sync_frontier_is_terminal() {
        for (suffix, fault) in [
            ("seal-self", LifecycleFault::SealDirectorySelfSync),
            ("seal-parent", LifecycleFault::SealDirectoryParentSync),
        ] {
            let namespace_name = format!("{suffix}-fault-test");
            let (_parent, namespace) = test_namespace(&namespace_name);
            let child = namespace
                .directory
                .create_new_private_child(&PrivateComponent::new("child").unwrap())
                .unwrap();
            child.lease.inject_lifecycle_fault(fault);
            let error = child.seal().err().unwrap();
            assert_eq!(error.error(), PrivateFsError::SettlementUnknown);
            assert_eq!(
                namespace.directory.list_components(4),
                Err(PrivateFsError::Quarantined)
            );
        }

        for (suffix, fault) in [
            ("unseal-self", LifecycleFault::UnsealDirectorySelfSync),
            ("unseal-parent", LifecycleFault::UnsealDirectoryParentSync),
        ] {
            let namespace_name = format!("{suffix}-fault-test");
            let (_parent, namespace) = test_namespace(&namespace_name);
            let child = namespace
                .directory
                .create_new_private_child(&PrivateComponent::new("child").unwrap())
                .unwrap()
                .seal()
                .unwrap();
            child.lease.inject_lifecycle_fault(fault);
            let error = child.unseal().err().unwrap();
            assert_eq!(error.error(), PrivateFsError::SettlementUnknown);
            assert_eq!(
                namespace.directory.list_components(4),
                Err(PrivateFsError::Quarantined)
            );
        }
    }

    #[test]
    fn regular_seal_sync_frontiers_are_terminal() {
        for (suffix, fault) in [
            ("file", LifecycleFault::SealRegularFileSync),
            ("parent", LifecycleFault::SealRegularParentSync),
        ] {
            let namespace_name = format!("regular-seal-{suffix}-fault-test");
            let (_parent, namespace) = test_namespace(&namespace_name);
            let child = namespace
                .directory
                .create_new_private_child(&PrivateComponent::new("child").unwrap())
                .unwrap();
            let file = PrivateComponent::new("payload.bin").unwrap();
            child
                .write_new_synced(&file, b"payload", ByteLimit::new(16).unwrap())
                .unwrap();
            child.lease.inject_lifecycle_fault(fault);

            assert_eq!(
                child.seal_verified_regular(&file),
                Err(PrivateFsError::SettlementUnknown)
            );
            assert_eq!(
                namespace.directory.list_components(4),
                Err(PrivateFsError::Quarantined)
            );
        }
    }

    #[test]
    fn empty_removal_post_commit_frontiers_are_terminal() {
        for (suffix, fault) in [
            ("commit", LifecycleFault::RemoveDirectoryCommitted),
            ("sync", LifecycleFault::RemoveDirectoryParentSync),
        ] {
            let namespace_name = format!("remove-{suffix}-fault-test");
            let (_parent, namespace) = test_namespace(&namespace_name);
            let child = namespace
                .directory
                .create_new_private_child(&PrivateComponent::new("child").unwrap())
                .unwrap();
            child.lease.inject_lifecycle_fault(fault);

            let error = child.remove_empty().err().unwrap();
            assert_eq!(error.error(), PrivateFsError::SettlementUnknown);
            assert!(!error.is_recoverable());
            assert_eq!(
                namespace.directory.list_components(4),
                Err(PrivateFsError::Quarantined)
            );
        }
    }

    #[test]
    fn post_commit_failure_quarantines_the_shared_lease() {
        let (_parent, namespace) = test_namespace("settlement-test");
        namespace.directory.lease.fail_next_settlement();
        assert_eq!(
            namespace.directory.write_new_synced(
                &PrivateComponent::new("value.bin").unwrap(),
                b"value",
                ByteLimit::new(16).unwrap(),
            ),
            Err(PrivateFsError::SettlementUnknown)
        );
        assert_eq!(
            namespace
                .directory
                .regular_exists(&PrivateComponent::new("value.bin").unwrap()),
            Err(PrivateFsError::Quarantined)
        );
    }

    #[test]
    fn poisoned_operation_mutex_stickily_quarantines_the_lease() {
        let (_parent, namespace) = test_namespace("mutex-poison-test");
        let lease = Arc::clone(&namespace.directory.lease);
        let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _operation = lease.begin().unwrap();
            panic!("inject operation panic");
        }));
        assert!(panic.is_err());
        assert_eq!(
            namespace.directory.list_components(8),
            Err(PrivateFsError::Quarantined)
        );
    }

    #[test]
    fn committed_unlink_reported_as_error_has_unknown_sticky_settlement() {
        let (parent, namespace) = test_namespace("unlink-fault-test");
        let file = PrivateComponent::new("value.bin").unwrap();
        namespace
            .directory
            .write_new_synced(&file, b"value", ByteLimit::new(16).unwrap())
            .unwrap();
        namespace
            .directory
            .lease
            .inject_committed_mutation_fault(CommittedMutationFault::Remove);

        assert_eq!(
            namespace.directory.remove_verified_regular(&file),
            Err(PrivateFsError::SettlementUnknown)
        );
        assert!(!parent.path().join("unlink-fault-test/value.bin").exists());
        assert_eq!(
            namespace.directory.list_components(8),
            Err(PrivateFsError::Quarantined)
        );
    }

    #[test]
    fn reported_stream_write_and_sync_failures_cleanly_remove_the_created_entry() {
        let (parent, namespace) = test_namespace("stream-io-cleanup-test");

        for (index, fault) in [
            StreamingFault::Write,
            StreamingFault::FileSync,
            StreamingFault::DirectorySync,
        ]
        .into_iter()
        .enumerate()
        {
            let name = PrivateEntryName::new(format!("Payload {index}.bin")).unwrap();
            namespace.directory.lease.inject_streaming_fault(fault);
            let mut source = Cursor::new(b"value");
            assert_eq!(
                namespace.directory.write_new_entry_from_reader(
                    &name,
                    &mut source,
                    StreamingFileLength::new(5).unwrap(),
                ),
                Err(StreamingWriteError::Filesystem(PrivateFsError::Io))
            );
            assert!(!parent
                .path()
                .join("stream-io-cleanup-test")
                .join(name.as_str())
                .exists());
            assert!(namespace.directory.list_entry_names(8).unwrap().is_empty());

            let mut retry = Cursor::new(b"value");
            namespace
                .directory
                .write_new_entry_from_reader(
                    &name,
                    &mut retry,
                    StreamingFileLength::new(5).unwrap(),
                )
                .unwrap();
            assert!(namespace
                .directory
                .remove_verified_entry_regular(&name)
                .unwrap());
        }
    }

    #[test]
    fn control_streaming_writer_has_the_exact_cleanup_and_durability_contract() {
        let (parent, namespace) = test_namespace("control-stream-write-test");
        let complete = PrivateComponent::new("complete.stage").unwrap();
        let incomplete = PrivateComponent::new("incomplete.stage").unwrap();

        let mut source = Cursor::new(b"value");
        namespace
            .directory
            .write_new_from_reader(&complete, &mut source, StreamingFileLength::new(5).unwrap())
            .unwrap();
        assert_eq!(
            namespace
                .directory
                .read_bounded_regular(&complete, ByteLimit::new(5).unwrap())
                .unwrap(),
            Some(b"value".to_vec())
        );

        let mut short = Cursor::new(b"four");
        assert_eq!(
            namespace.directory.write_new_from_reader(
                &incomplete,
                &mut short,
                StreamingFileLength::new(5).unwrap(),
            ),
            Err(StreamingWriteError::SourceTooShort)
        );
        assert!(!parent
            .path()
            .join("control-stream-write-test/incomplete.stage")
            .exists());
        assert_eq!(
            namespace.directory.list_components(8).unwrap(),
            vec![complete]
        );
    }

    #[test]
    fn streaming_cleanup_unlink_ambiguity_is_unknown_and_stickily_quarantined() {
        let (parent, namespace) = test_namespace("stream-cleanup-unlink-test");
        let name = PrivateEntryName::new("Payload.bin").unwrap();
        namespace
            .directory
            .lease
            .inject_committed_mutation_fault(CommittedMutationFault::Remove);
        let mut short = Cursor::new(b"four");

        assert_eq!(
            namespace.directory.write_new_entry_from_reader(
                &name,
                &mut short,
                StreamingFileLength::new(5).unwrap(),
            ),
            Err(StreamingWriteError::Filesystem(
                PrivateFsError::SettlementUnknown
            ))
        );
        assert!(!parent
            .path()
            .join("stream-cleanup-unlink-test/Payload.bin")
            .exists());
        assert_eq!(
            namespace.directory.list_entry_names(8),
            Err(PrivateFsError::Quarantined)
        );
    }

    #[test]
    fn streaming_cleanup_directory_sync_ambiguity_is_unknown_and_stickily_quarantined() {
        let (parent, namespace) = test_namespace("stream-cleanup-sync-test");
        let name = PrivateEntryName::new("Payload.bin").unwrap();
        namespace
            .directory
            .lease
            .inject_streaming_fault(StreamingFault::CleanupDirectorySync);
        let mut short = Cursor::new(b"four");

        assert_eq!(
            namespace.directory.write_new_entry_from_reader(
                &name,
                &mut short,
                StreamingFileLength::new(5).unwrap(),
            ),
            Err(StreamingWriteError::Filesystem(
                PrivateFsError::SettlementUnknown
            ))
        );
        assert!(!parent
            .path()
            .join("stream-cleanup-sync-test/Payload.bin")
            .exists());
        assert_eq!(
            namespace.directory.list_entry_names(8),
            Err(PrivateFsError::Quarantined)
        );
    }

    #[test]
    fn bounded_reader_initial_metadata_failure_is_identity_ambiguous_and_quarantines() {
        use std::cell::Cell;

        let (_parent, namespace) = test_namespace("stream-read-metadata-test");
        let name = PrivateEntryName::new("Payload.bin").unwrap();
        namespace
            .directory
            .write_new_entry_synced(&name, b"value", ByteLimit::new(16).unwrap())
            .unwrap();
        namespace
            .directory
            .lease
            .inject_streaming_fault(StreamingFault::InitialReadMetadata);
        let called = Cell::new(false);

        assert_eq!(
            namespace.directory.with_bounded_entry_regular_reader(
                &name,
                ByteLimit::new(16).unwrap(),
                |_reader| {
                    called.set(true);
                    Ok::<_, ()>(())
                },
            ),
            Err(PrivateFsError::IdentityAmbiguous)
        );
        assert!(!called.get());
        assert_eq!(
            namespace.directory.list_entry_names(8),
            Err(PrivateFsError::Quarantined)
        );
    }

    #[test]
    fn final_stream_identity_ambiguity_quarantines_without_unlinking_the_moved_inode() {
        let (parent, namespace) = test_namespace("stream-final-identity-test");
        let root = parent.path().join("stream-final-identity-test");
        let name = PrivateEntryName::new("Payload.bin").unwrap();
        let moved = root.join("Moved Payload.bin");
        let mut source = RenameOnFirstRead {
            bytes: Cursor::new(b"value".to_vec()),
            source: root.join(name.as_str()),
            destination: moved.clone(),
            renamed: false,
        };

        assert_eq!(
            namespace.directory.write_new_entry_from_reader(
                &name,
                &mut source,
                StreamingFileLength::new(5).unwrap(),
            ),
            Err(StreamingWriteError::Filesystem(
                PrivateFsError::SettlementUnknown
            ))
        );
        assert!(moved.exists(), "ambiguous moved inode must not be unlinked");
        assert_eq!(fs::read(moved).unwrap(), b"value");
        assert_eq!(
            namespace.directory.list_entry_names(8),
            Err(PrivateFsError::Quarantined)
        );
    }

    #[test]
    fn final_stream_lock_ambiguity_quarantines_without_attempting_payload_cleanup() {
        let (parent, namespace) = test_namespace("stream-final-lock-test");
        let root = parent.path().join("stream-final-lock-test");
        let name = PrivateEntryName::new("Payload.bin").unwrap();
        let mut source = RemoveOnFirstRead {
            bytes: Cursor::new(b"value".to_vec()),
            target: root.join(LOCK_COMPONENT_NAME),
            removed: false,
        };

        assert_eq!(
            namespace.directory.write_new_entry_from_reader(
                &name,
                &mut source,
                StreamingFileLength::new(5).unwrap(),
            ),
            Err(StreamingWriteError::Filesystem(
                PrivateFsError::SettlementUnknown
            ))
        );
        assert_eq!(
            fs::read(root.join(name.as_str())).unwrap(),
            b"value",
            "payload must remain when lock authority is already ambiguous"
        );
        assert_eq!(
            namespace.directory.list_entry_names(8),
            Err(PrivateFsError::Quarantined)
        );
    }

    #[test]
    fn failed_stream_with_lost_lock_quarantines_without_cleanup_mutation() {
        let (parent, namespace) = test_namespace("stream-failed-lock-test");
        let root = parent.path().join("stream-failed-lock-test");
        let name = PrivateEntryName::new("Payload.bin").unwrap();
        let mut source = RemoveOnFirstRead {
            bytes: Cursor::new(b"val".to_vec()),
            target: root.join(LOCK_COMPONENT_NAME),
            removed: false,
        };

        assert_eq!(
            namespace.directory.write_new_entry_from_reader(
                &name,
                &mut source,
                StreamingFileLength::new(5).unwrap(),
            ),
            Err(StreamingWriteError::Filesystem(
                PrivateFsError::SettlementUnknown
            ))
        );
        assert_eq!(
            fs::read(root.join(name.as_str())).unwrap(),
            b"val",
            "partial payload must remain when cleanup authority is ambiguous"
        );
        assert_eq!(
            namespace.directory.list_entry_names(8),
            Err(PrivateFsError::Quarantined)
        );
    }

    #[test]
    fn exact_stream_length_is_revalidated_after_the_hostile_eof_probe() {
        let (parent, namespace) = test_namespace("stream-eof-mutation-test");
        let root = parent.path().join("stream-eof-mutation-test");

        for (entry, mutation) in [
            ("Appended.bin", EofMutation::Append),
            ("Truncated.bin", EofMutation::Truncate),
        ] {
            let name = PrivateEntryName::new(entry).unwrap();
            let target = root.join(name.as_str());
            let mut source = MutateOnEof {
                bytes: Cursor::new(b"value".to_vec()),
                target: target.clone(),
                mutation,
                mutated: false,
            };

            assert_eq!(
                namespace.directory.write_new_entry_from_reader(
                    &name,
                    &mut source,
                    StreamingFileLength::new(5).unwrap(),
                ),
                Err(StreamingWriteError::SinkLengthMismatch)
            );
            assert!(
                !target.exists(),
                "length-mismatched held inode must be durably cleaned"
            );
            assert_eq!(namespace.directory.list_entry_names(8).unwrap(), vec![]);
        }
    }

    #[test]
    fn committed_replace_reported_as_error_has_unknown_sticky_settlement() {
        let (parent, namespace) = test_namespace("replace-fault-test");
        let source = PrivateComponent::new("stage.bin").unwrap();
        let destination = PrivateComponent::new("current.bin").unwrap();
        namespace
            .directory
            .write_new_synced(&source, b"next", ByteLimit::new(16).unwrap())
            .unwrap();
        namespace
            .directory
            .write_new_synced(&destination, b"old", ByteLimit::new(16).unwrap())
            .unwrap();
        namespace
            .directory
            .lease
            .inject_committed_mutation_fault(CommittedMutationFault::Replace);

        assert_eq!(
            namespace
                .directory
                .replace_verified_regular(&source, &destination),
            Err(PrivateFsError::SettlementUnknown)
        );
        assert!(!parent.path().join("replace-fault-test/stage.bin").exists());
        assert_eq!(
            namespace.directory.regular_exists(&destination),
            Err(PrivateFsError::Quarantined)
        );
    }

    #[test]
    fn committed_noreplace_reported_as_error_has_unknown_sticky_settlement() {
        let (parent, namespace) = test_namespace("noreplace-fault-test");
        let source = PrivateComponent::new("stage.bin").unwrap();
        let destination = PrivateComponent::new("current.bin").unwrap();
        namespace
            .directory
            .write_new_synced(&source, b"next", ByteLimit::new(16).unwrap())
            .unwrap();
        namespace
            .directory
            .lease
            .inject_committed_mutation_fault(CommittedMutationFault::PublishNoreplace);

        assert_eq!(
            namespace
                .directory
                .publish_noreplace_verified_regular(&source, &destination),
            Err(PrivateFsError::SettlementUnknown)
        );
        assert!(!parent
            .path()
            .join("noreplace-fault-test/stage.bin")
            .exists());
        assert_eq!(
            namespace.directory.regular_exists(&destination),
            Err(PrivateFsError::Quarantined)
        );
    }

    #[test]
    fn ambiguous_child_creation_stickily_quarantines_the_lease() {
        let (parent, namespace) = test_namespace("mkdir-fault-test");
        let child = PrivateComponent::new("objects").unwrap();
        namespace
            .directory
            .lease
            .inject_committed_mutation_fault(CommittedMutationFault::CreateDirectory);

        assert_eq!(
            namespace.directory.create_private_child(&child).err(),
            Some(PrivateFsError::SettlementUnknown)
        );
        assert!(parent.path().join("mkdir-fault-test/objects").is_dir());
        assert_eq!(
            namespace.directory.list_components(8),
            Err(PrivateFsError::Quarantined)
        );
    }

    #[test]
    fn ambiguous_entry_child_creation_stickily_quarantines_the_lease() {
        let (parent, namespace) = test_namespace("entry-mkdir-fault-test");
        let child = PrivateEntryName::new("Objects With Case").unwrap();
        namespace
            .directory
            .lease
            .inject_committed_mutation_fault(CommittedMutationFault::CreateDirectory);

        assert_eq!(
            namespace.directory.create_new_entry_child(&child).err(),
            Some(PrivateFsError::SettlementUnknown)
        );
        assert!(parent
            .path()
            .join("entry-mkdir-fault-test/Objects With Case")
            .is_dir());
        assert_eq!(
            namespace.directory.list_entry_names(8),
            Err(PrivateFsError::Quarantined)
        );
    }

    #[test]
    fn failed_root_create_is_clean_only_when_the_leaf_is_proven_absent() {
        let parent = tempfile::tempdir_in(std::env::current_dir().unwrap()).unwrap();
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(parent.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let root = parent.path().join("root-create-fault");
        let injected = std::io::Error::from_raw_os_error(rustix::io::Errno::IO.raw_os_error());

        assert_eq!(
            classify_root_create_error(&root, &injected),
            PrivateFsError::Io
        );
        fs::create_dir(&root).unwrap();
        assert_eq!(
            classify_root_create_error(&root, &injected),
            PrivateFsError::SettlementUnknown
        );
    }

    #[test]
    fn published_lock_with_failed_root_revalidation_is_settlement_unknown() {
        use std::os::unix::fs::PermissionsExt;

        let parent = tempfile::tempdir_in(std::env::current_dir().unwrap()).unwrap();
        fs::set_permissions(parent.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let root = parent.path().join("lock-publish-settlement-test");
        private_directory_builder().create(&root).unwrap();
        let directory = admit_root(root.clone()).unwrap();
        let lock_name = canonical_lock_component().unwrap();
        let staging_name = lock_staging_component().unwrap();
        let (file, identity) =
            platform::create_new_regular(&directory.handle, &directory.path, staging_name.as_str())
                .unwrap();
        let mut staging = VerifiedRegular {
            file,
            identity: FileIdentity(identity),
        };
        let mut mutated = true;
        prepare_staged_lock(&directory, &staging_name, &mut staging, &mut mutated).unwrap();
        platform::atomic_publish_noreplace(
            &directory.handle,
            &directory.path,
            staging_name.as_str(),
            lock_name.as_str(),
        )
        .unwrap();

        let moved = parent.path().join("moved-lock-publish-settlement-test");
        fs::rename(&root, moved).unwrap();
        private_directory_builder().create(&root).unwrap();

        assert_eq!(
            settle_published_lock(&directory, &lock_name, &staging_name, staging).err(),
            Some(PrivateFsError::SettlementUnknown)
        );
    }

    #[test]
    fn staging_cleanup_with_failed_root_revalidation_is_settlement_unknown() {
        use std::os::unix::fs::PermissionsExt;

        let parent = tempfile::tempdir_in(std::env::current_dir().unwrap()).unwrap();
        fs::set_permissions(parent.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let root = parent.path().join("staging-cleanup-settlement-test");
        private_directory_builder().create(&root).unwrap();
        let directory = admit_root(root.clone()).unwrap();
        let lock_name = canonical_lock_component().unwrap();
        let staging_name = lock_staging_component().unwrap();
        let mut lock = acquire_lock(&directory, &lock_name, &staging_name).unwrap();
        let (mut staging, _) =
            platform::create_new_regular(&directory.handle, &directory.path, staging_name.as_str())
                .unwrap();
        staging.write_all(LOCK_FILE_CONTENT).unwrap();
        staging.sync_all().unwrap();
        drop(staging);

        let moved = parent.path().join("moved-staging-cleanup-settlement-test");
        fs::rename(&root, &moved).unwrap();
        private_directory_builder().create(&root).unwrap();

        assert_eq!(
            inspect_and_cleanup_lock_staging(&directory, &lock_name, &mut lock, &staging_name,),
            Err(PrivateFsError::SettlementUnknown)
        );
        assert!(!moved.join(staging_name.as_str()).exists());
    }
}

#[cfg(all(test, target_os = "windows", feature = "windows-namespace-validation"))]
mod windows_tests;
