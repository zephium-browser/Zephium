use std::fs::File;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
#[cfg(test)]
use std::sync::atomic::{AtomicU16, AtomicU32};
use std::sync::{Mutex, MutexGuard};

use crate::platform;
use crate::{DirectoryIdentity, FileIdentity, PrivateComponent, PrivateFsError};

pub(crate) struct NamespaceLease {
    _lock: File,
    #[cfg(unix)]
    lock_owner: u32,
    lock_identity: FileIdentity,
    reserved_lock: PrivateComponent,
    reserved_lock_staging: PrivateComponent,
    root_path: PathBuf,
    root_handle: File,
    root_identity: DirectoryIdentity,
    operation: Mutex<()>,
    path_pins: AtomicUsize,
    quarantined: AtomicBool,
    #[cfg(all(test, unix))]
    fail_next_settlement: AtomicBool,
    #[cfg(test)]
    committed_mutation_faults: AtomicU16,
    #[cfg(test)]
    streaming_faults: AtomicU16,
    #[cfg(test)]
    lifecycle_faults: AtomicU32,
    #[cfg(test)]
    remove_empty_race: AtomicBool,
    #[cfg(test)]
    seal_child_identity_fault: AtomicBool,
    #[cfg(test)]
    same_parent_publish_syncs: AtomicUsize,
    #[cfg(all(
        test,
        any(target_os = "macos", target_os = "linux", target_os = "windows")
    ))]
    tree_removal_mutation_fault_ordinal: AtomicUsize,
    #[cfg(all(
        test,
        any(target_os = "macos", target_os = "linux", target_os = "windows")
    ))]
    tree_removal_final_parent_sync_fault: AtomicBool,
    #[cfg(all(
        test,
        any(target_os = "macos", target_os = "linux", target_os = "windows")
    ))]
    tree_removal_preexecution_identity_fault: AtomicBool,
}

#[cfg(unix)]
impl Drop for NamespaceLease {
    fn drop(&mut self) {
        // The last capability releases ownership, even if an unrelated spawn
        // has inherited the descriptor and has not reached CLOEXEC yet. A
        // forked copy must never unlock the original process's lease.
        if self.lock_owner == std::process::id() {
            platform::unlock_exclusive(&self._lock);
        }
    }
}

#[cfg(test)]
#[derive(Clone, Copy)]
pub(crate) enum CommittedMutationFault {
    Remove = 1 << 0,
    Replace = 1 << 1,
    PublishNoreplace = 1 << 2,
    CreateDirectory = 1 << 3,
}

#[cfg(test)]
#[derive(Clone, Copy)]
pub(crate) enum StreamingFault {
    Write = 1 << 0,
    FileSync = 1 << 1,
    DirectorySync = 1 << 2,
    CleanupDirectorySync = 1 << 3,
    InitialReadMetadata = 1 << 4,
}

#[cfg(test)]
#[derive(Clone, Copy)]
pub(crate) enum LifecycleFault {
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    PublishDestinationIdentity = 1 << 0,
    PublishSourceParentSync = 1 << 1,
    PublishDestinationParentSync = 1 << 2,
    SealRegularCommitted = 1 << 3,
    SealRegularFileSync = 1 << 4,
    SealRegularParentSync = 1 << 5,
    SealDirectoryCommitted = 1 << 6,
    SealDirectorySelfSync = 1 << 7,
    SealDirectoryParentSync = 1 << 8,
    UnsealDirectoryCommitted = 1 << 9,
    UnsealDirectorySelfSync = 1 << 10,
    UnsealDirectoryParentSync = 1 << 11,
    RemoveDirectoryCommitted = 1 << 12,
    RemoveDirectoryParentSync = 1 << 13,
    PublishSameParentSync = 1 << 14,
    RemoveChildOpenIdentity = 1 << 15,
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    PublishRootWritableCommitted = 1 << 16,
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    PublishRootResealCommitted = 1 << 17,
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    PublishRootResealSync = 1 << 18,
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    PublishRenameRefused = 1 << 19,
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    PublishRenameCommitted = 1 << 20,
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    PublishRootResealRefused = 1 << 21,
}

pub(crate) struct PathPinGuard<'a> {
    lease: &'a NamespaceLease,
    active: bool,
}

impl PathPinGuard<'_> {
    pub(crate) fn release(mut self) {
        self.release_inner();
    }

    fn release_inner(&mut self) {
        if !self.active {
            return;
        }
        self.active = false;
        if self.lease.path_pins.fetch_sub(1, Ordering::AcqRel) == 0 {
            self.lease.path_pins.store(0, Ordering::Release);
            self.lease.quarantined.store(true, Ordering::Release);
        }
    }
}

impl Drop for PathPinGuard<'_> {
    fn drop(&mut self) {
        self.release_inner();
    }
}

impl NamespaceLease {
    pub(crate) fn new(
        lock: File,
        lock_identity: FileIdentity,
        reserved_lock: PrivateComponent,
        reserved_lock_staging: PrivateComponent,
        root_path: PathBuf,
        root_handle: File,
        root_identity: DirectoryIdentity,
    ) -> Self {
        Self {
            _lock: lock,
            #[cfg(unix)]
            lock_owner: std::process::id(),
            lock_identity,
            reserved_lock,
            reserved_lock_staging,
            root_path,
            root_handle,
            root_identity,
            operation: Mutex::new(()),
            path_pins: AtomicUsize::new(0),
            quarantined: AtomicBool::new(false),
            #[cfg(all(test, unix))]
            fail_next_settlement: AtomicBool::new(false),
            #[cfg(test)]
            committed_mutation_faults: AtomicU16::new(0),
            #[cfg(test)]
            streaming_faults: AtomicU16::new(0),
            #[cfg(test)]
            lifecycle_faults: AtomicU32::new(0),
            #[cfg(test)]
            remove_empty_race: AtomicBool::new(false),
            #[cfg(test)]
            seal_child_identity_fault: AtomicBool::new(false),
            #[cfg(test)]
            same_parent_publish_syncs: AtomicUsize::new(0),
            #[cfg(all(
                test,
                any(target_os = "macos", target_os = "linux", target_os = "windows")
            ))]
            tree_removal_mutation_fault_ordinal: AtomicUsize::new(0),
            #[cfg(all(
                test,
                any(target_os = "macos", target_os = "linux", target_os = "windows")
            ))]
            tree_removal_final_parent_sync_fault: AtomicBool::new(false),
            #[cfg(all(
                test,
                any(target_os = "macos", target_os = "linux", target_os = "windows")
            ))]
            tree_removal_preexecution_identity_fault: AtomicBool::new(false),
        }
    }

    pub(crate) fn begin(&self) -> Result<MutexGuard<'_, ()>, PrivateFsError> {
        if self.quarantined.load(Ordering::Acquire) {
            return Err(PrivateFsError::Quarantined);
        }
        let guard = self.operation.lock().map_err(|_| {
            self.quarantined.store(true, Ordering::Release);
            PrivateFsError::Quarantined
        })?;
        if self.quarantined.load(Ordering::Acquire) {
            return Err(PrivateFsError::Quarantined);
        }
        Ok(guard)
    }

    pub(crate) fn is_reserved_name(&self, name: &str) -> bool {
        self.is_reserved_lock_name(name) || self.is_reserved_lock_staging_name(name)
    }

    pub(crate) fn mutation_allowed(&self) -> Result<(), PrivateFsError> {
        if self.path_pins.load(Ordering::Acquire) == 0 {
            Ok(())
        } else {
            Err(PrivateFsError::InUse)
        }
    }

    pub(crate) fn pin_path(&self) -> Result<PathPinGuard<'_>, PrivateFsError> {
        self.path_pins
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |pins| {
                pins.checked_add(1)
            })
            .map_err(|_| PrivateFsError::InUse)?;
        Ok(PathPinGuard {
            lease: self,
            active: true,
        })
    }

    pub(crate) fn is_reserved_lock_name(&self, name: &str) -> bool {
        name.eq_ignore_ascii_case(self.reserved_lock.as_str())
    }

    pub(crate) fn is_canonical_reserved_lock_name(&self, name: &str) -> bool {
        name == self.reserved_lock.as_str()
    }

    pub(crate) fn is_reserved_lock_staging_name(&self, name: &str) -> bool {
        name.eq_ignore_ascii_case(self.reserved_lock_staging.as_str())
    }

    pub(crate) fn verify_authority(&self) -> Result<(), PrivateFsError> {
        if !platform::same_open_identity(&self.root_handle, self.root_identity.0) {
            return Err(PrivateFsError::IdentityAmbiguous);
        }
        let (current_root, identity) = platform::open_directory(&self.root_path)
            .map_err(|_| PrivateFsError::IdentityAmbiguous)?;
        if identity != self.root_identity.0
            || !platform::same_open_identity(&current_root, identity)
        {
            return Err(PrivateFsError::IdentityAmbiguous);
        }
        platform::revalidate_regular(
            &self.root_handle,
            &self.root_path,
            self.reserved_lock.as_str(),
            &self._lock,
            self.lock_identity.0,
        )
        .map_err(|_| PrivateFsError::IdentityAmbiguous)
    }

    pub(crate) fn observe<T>(
        &self,
        result: Result<T, PrivateFsError>,
    ) -> Result<T, PrivateFsError> {
        if matches!(result, Err(PrivateFsError::IdentityAmbiguous)) {
            self.quarantined.store(true, Ordering::Release);
        }
        result
    }

    pub(crate) fn settle_after_commit<T>(
        &self,
        result: Result<T, PrivateFsError>,
    ) -> Result<T, PrivateFsError> {
        #[cfg(all(test, unix))]
        let result = if self.fail_next_settlement.swap(false, Ordering::AcqRel) {
            Err(PrivateFsError::Io)
        } else {
            result
        };
        match result {
            Ok(value) => Ok(value),
            Err(_) => {
                self.quarantined.store(true, Ordering::Release);
                Err(PrivateFsError::SettlementUnknown)
            }
        }
    }

    #[cfg(all(test, unix))]
    pub(crate) fn fail_next_settlement(&self) {
        self.fail_next_settlement.store(true, Ordering::Release);
    }

    #[cfg(all(
        test,
        any(
            target_os = "macos",
            target_os = "linux",
            all(target_os = "windows", feature = "windows-namespace-validation")
        )
    ))]
    pub(crate) fn inject_committed_mutation_fault(&self, fault: CommittedMutationFault) {
        self.committed_mutation_faults
            .fetch_or(fault as u16, Ordering::AcqRel);
    }

    #[cfg(test)]
    pub(crate) fn take_committed_mutation_fault(&self, fault: CommittedMutationFault) -> bool {
        let mask = fault as u16;
        self.committed_mutation_faults
            .fetch_and(!mask, Ordering::AcqRel)
            & mask
            != 0
    }

    #[cfg(all(
        test,
        any(
            target_os = "macos",
            target_os = "linux",
            all(target_os = "windows", feature = "windows-namespace-validation")
        )
    ))]
    pub(crate) fn inject_streaming_fault(&self, fault: StreamingFault) {
        self.streaming_faults
            .fetch_or(fault as u16, Ordering::AcqRel);
    }

    #[cfg(test)]
    pub(crate) fn take_streaming_fault(&self, fault: StreamingFault) -> bool {
        let mask = fault as u16;
        self.streaming_faults.fetch_and(!mask, Ordering::AcqRel) & mask != 0
    }

    #[cfg(all(
        test,
        any(
            target_os = "macos",
            target_os = "linux",
            all(target_os = "windows", feature = "windows-namespace-validation")
        )
    ))]
    pub(crate) fn inject_lifecycle_fault(&self, fault: LifecycleFault) {
        self.lifecycle_faults
            .fetch_or(fault as u32, Ordering::AcqRel);
    }

    #[cfg(test)]
    pub(crate) fn take_lifecycle_fault(&self, fault: LifecycleFault) -> bool {
        let mask = fault as u32;
        self.lifecycle_faults.fetch_and(!mask, Ordering::AcqRel) & mask != 0
    }

    #[cfg(all(
        test,
        any(
            target_os = "macos",
            target_os = "linux",
            all(target_os = "windows", feature = "windows-namespace-validation")
        )
    ))]
    pub(crate) fn inject_remove_empty_race(&self) {
        self.remove_empty_race.store(true, Ordering::Release);
    }

    #[cfg(test)]
    pub(crate) fn take_remove_empty_race(&self) -> bool {
        self.remove_empty_race.swap(false, Ordering::AcqRel)
    }

    #[cfg(all(
        test,
        any(
            target_os = "macos",
            target_os = "linux",
            all(target_os = "windows", feature = "windows-namespace-validation")
        )
    ))]
    pub(crate) fn inject_seal_child_identity_fault(&self) {
        self.seal_child_identity_fault
            .store(true, Ordering::Release);
    }

    #[cfg(test)]
    pub(crate) fn take_seal_child_identity_fault(&self) -> bool {
        self.seal_child_identity_fault.swap(false, Ordering::AcqRel)
    }

    #[cfg(test)]
    pub(crate) fn record_same_parent_publish_sync(&self) {
        self.same_parent_publish_syncs
            .fetch_add(1, Ordering::AcqRel);
    }

    #[cfg(all(
        test,
        any(
            target_os = "macos",
            target_os = "linux",
            all(target_os = "windows", feature = "windows-namespace-validation")
        )
    ))]
    pub(crate) fn same_parent_publish_sync_count(&self) -> usize {
        self.same_parent_publish_syncs.load(Ordering::Acquire)
    }

    #[cfg(all(
        test,
        any(
            target_os = "macos",
            target_os = "linux",
            all(target_os = "windows", feature = "windows-namespace-validation")
        )
    ))]
    pub(crate) fn inject_tree_removal_mutation_fault(&self, ordinal: usize) {
        assert_ne!(ordinal, 0, "tree-removal mutation ordinals are one-based");
        self.tree_removal_mutation_fault_ordinal
            .store(ordinal, Ordering::Release);
    }

    #[cfg(all(
        test,
        any(target_os = "macos", target_os = "linux", target_os = "windows")
    ))]
    pub(crate) fn take_tree_removal_mutation_fault_ordinal(&self) -> Option<usize> {
        match self
            .tree_removal_mutation_fault_ordinal
            .swap(0, Ordering::AcqRel)
        {
            0 => None,
            ordinal => Some(ordinal),
        }
    }

    #[cfg(all(
        test,
        any(
            target_os = "macos",
            target_os = "linux",
            all(target_os = "windows", feature = "windows-namespace-validation")
        )
    ))]
    pub(crate) fn inject_tree_removal_final_parent_sync_fault(&self) {
        self.tree_removal_final_parent_sync_fault
            .store(true, Ordering::Release);
    }

    #[cfg(all(
        test,
        any(target_os = "macos", target_os = "linux", target_os = "windows")
    ))]
    pub(crate) fn take_tree_removal_final_parent_sync_fault(&self) -> bool {
        self.tree_removal_final_parent_sync_fault
            .swap(false, Ordering::AcqRel)
    }

    #[cfg(all(
        test,
        any(
            target_os = "macos",
            target_os = "linux",
            all(target_os = "windows", feature = "windows-namespace-validation")
        )
    ))]
    pub(crate) fn inject_tree_removal_preexecution_identity_fault(&self) {
        self.tree_removal_preexecution_identity_fault
            .store(true, Ordering::Release);
    }

    #[cfg(all(
        test,
        any(target_os = "macos", target_os = "linux", target_os = "windows")
    ))]
    pub(crate) fn take_tree_removal_preexecution_identity_fault(&self) -> bool {
        self.tree_removal_preexecution_identity_fault
            .swap(false, Ordering::AcqRel)
    }
}

#[cfg(all(test, unix))]
mod release_tests {
    use super::*;

    #[test]
    fn inherited_lease_drop_does_not_unlock_the_owning_process() {
        let root = tempfile::tempdir().unwrap();
        let lock_path = root.path().join("lock");
        let lock = File::create(&lock_path).unwrap();
        platform::lock_exclusive(&lock).unwrap();
        // This duplicate represents the original process's open description.
        // Simulate the child's PID without running Rust destructors after fork.
        let owning_descriptor = lock.try_clone().unwrap();
        let lock_identity = FileIdentity(platform::raw_identity(&lock.metadata().unwrap()));
        let root_handle = File::open(root.path()).unwrap();
        let root_identity =
            DirectoryIdentity(platform::raw_identity(&root_handle.metadata().unwrap()));
        let mut inherited = NamespaceLease::new(
            lock,
            lock_identity,
            PrivateComponent::new("lock").unwrap(),
            PrivateComponent::new("staging").unwrap(),
            root.path().to_owned(),
            root_handle,
            root_identity,
        );
        inherited.lock_owner = std::process::id().wrapping_add(1);
        drop(inherited);

        let contender = File::open(lock_path).unwrap();
        assert_eq!(
            platform::lock_exclusive(&contender),
            Err(PrivateFsError::LockUnavailable)
        );
        platform::unlock_exclusive(&owning_descriptor);
        platform::lock_exclusive(&contender).unwrap();
    }
}
