use std::fs::{File, Metadata};
#[cfg(any(target_os = "macos", target_os = "linux"))]
use std::os::fd::AsRawFd;
use std::os::unix::fs::MetadataExt;
use std::path::Path;

use rustix::fs::{FileType, Mode, OFlags};

use super::{DirectoryMode, RegularMode};
use crate::PrivateFsError;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) struct RawIdentity {
    device: u64,
    inode: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RawChildKind {
    Regular(RawIdentity),
    Directory(RawIdentity),
}

#[derive(Clone, Copy)]
pub(crate) enum OpenPurpose {
    Read,
    Mutation,
    Lock,
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
pub(crate) fn admit_namespace_support() -> Result<(), PrivateFsError> {
    Ok(())
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub(crate) fn admit_namespace_support() -> Result<(), PrivateFsError> {
    Err(PrivateFsError::PrimitiveUnavailable)
}

pub(crate) fn current_user() -> u32 {
    rustix::process::geteuid().as_raw()
}

pub(crate) fn raw_identity(metadata: &Metadata) -> RawIdentity {
    RawIdentity {
        device: metadata.dev(),
        inode: metadata.ino(),
    }
}

pub(crate) fn open_regular(
    directory: &File,
    directory_path: &Path,
    name: &str,
    purpose: OpenPurpose,
) -> Result<(File, RawIdentity), PrivateFsError> {
    open_regular_with_mode(directory, directory_path, name, purpose, None)
}

pub(crate) fn open_sealed_regular(
    directory: &File,
    directory_path: &Path,
    name: &str,
) -> Result<(File, RawIdentity), PrivateFsError> {
    open_regular_with_mode(
        directory,
        directory_path,
        name,
        OpenPurpose::Read,
        Some(RegularMode::Sealed),
    )
}

fn open_regular_with_mode(
    directory: &File,
    _directory_path: &Path,
    name: &str,
    purpose: OpenPurpose,
    expected_mode: Option<RegularMode>,
) -> Result<(File, RawIdentity), PrivateFsError> {
    let access = match purpose {
        OpenPurpose::Read | OpenPurpose::Mutation => OFlags::RDONLY,
        OpenPurpose::Lock => OFlags::RDWR,
    };
    let descriptor = rustix::fs::openat(
        directory,
        name,
        access | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::NONBLOCK,
        Mode::empty(),
    )
    .map_err(map_child_open_error)?;
    let file = File::from(descriptor);
    let opened = file
        .metadata()
        .map_err(|_| PrivateFsError::IdentityAmbiguous)?;
    match expected_mode {
        Some(mode) => validate_regular_metadata_mode(&opened, mode)?,
        None => validate_regular_metadata(&opened)?,
    }
    let identity = raw_identity(&opened);
    if !acl_is_private(&file) {
        return Err(PrivateFsError::Unsafe);
    }
    if relative_regular_identity(directory, name)? != identity {
        return Err(PrivateFsError::IdentityAmbiguous);
    }
    Ok((file, identity))
}

pub(crate) fn create_new_regular(
    directory: &File,
    _directory_path: &Path,
    name: &str,
) -> Result<(File, RawIdentity), PrivateFsError> {
    let descriptor = rustix::fs::openat(
        directory,
        name,
        OFlags::RDWR
            | OFlags::CREATE
            | OFlags::EXCL
            | OFlags::CLOEXEC
            | OFlags::NOFOLLOW
            | OFlags::NONBLOCK,
        Mode::from_raw_mode(0o600),
    )
    .map_err(|error| map_failed_create(error, directory, name, true))?;
    let file = File::from(descriptor);
    let admission = (|| {
        let opened = file.metadata().map_err(|_| PrivateFsError::Io)?;
        validate_regular_metadata_mode(&opened, RegularMode::Writable)?;
        let identity = raw_identity(&opened);
        if !acl_is_private(&file) || relative_regular_identity(directory, name)? != identity {
            return Err(PrivateFsError::IdentityAmbiguous);
        }
        Ok(identity)
    })();
    match admission {
        Ok(identity) => Ok((file, identity)),
        Err(_) => Err(PrivateFsError::SettlementUnknown),
    }
}

pub(crate) fn revalidate_regular(
    directory: &File,
    _directory_path: &Path,
    name: &str,
    file: &File,
    expected: RawIdentity,
) -> Result<(), PrivateFsError> {
    let opened = file
        .metadata()
        .map_err(|_| PrivateFsError::IdentityAmbiguous)?;
    if validate_regular_metadata(&opened).is_err()
        || raw_identity(&opened) != expected
        || relative_regular_identity(directory, name)? != expected
        || !acl_is_private(file)
    {
        return Err(PrivateFsError::IdentityAmbiguous);
    }
    // Every admitted child name is exact, including lowercase protocol names.
    // Keep this proof in the central revalidation primitive so mutation and
    // failed-mutation settlement cannot accidentally level down to identity
    // alone on a case-folding filesystem.
    verify_exact_name(file, name).map_err(|_| PrivateFsError::IdentityAmbiguous)
}

pub(crate) fn revalidate_regular_mode(
    directory: &File,
    _directory_path: &Path,
    name: &str,
    file: &File,
    expected: RawIdentity,
    mode: RegularMode,
) -> Result<(), PrivateFsError> {
    let opened = file
        .metadata()
        .map_err(|_| PrivateFsError::IdentityAmbiguous)?;
    if validate_regular_metadata_mode(&opened, mode).is_err()
        || raw_identity(&opened) != expected
        || relative_regular_identity(directory, name)? != expected
        || !acl_is_private(file)
    {
        return Err(PrivateFsError::IdentityAmbiguous);
    }
    verify_exact_name(file, name).map_err(|_| PrivateFsError::IdentityAmbiguous)
}

pub(crate) fn set_regular_mode(file: &File, mode: RegularMode) -> Result<(), PrivateFsError> {
    let raw_mode = match mode {
        RegularMode::Writable => 0o600,
        RegularMode::Sealed => 0o400,
    };
    rustix::fs::fchmod(file, Mode::from_raw_mode(raw_mode)).map_err(|_| PrivateFsError::Io)
}

pub(crate) fn create_directory(
    parent: &File,
    _parent_path: &Path,
    name: &str,
) -> Result<bool, PrivateFsError> {
    let created = match rustix::fs::mkdirat(parent, name, Mode::from_raw_mode(0o700)) {
        Ok(()) => true,
        Err(rustix::io::Errno::EXIST) => false,
        Err(error) => return Err(map_failed_create(error, parent, name, false)),
    };
    Ok(created)
}

pub(crate) fn open_child_directory(
    parent: &File,
    _parent_path: &Path,
    name: &str,
) -> Result<(File, RawIdentity), PrivateFsError> {
    open_relative_directory(parent, name, Some(DirectoryMode::Writable))
}

pub(crate) fn open_sealed_child_directory(
    parent: &File,
    _parent_path: &Path,
    name: &str,
) -> Result<(File, RawIdentity), PrivateFsError> {
    open_relative_directory(parent, name, Some(DirectoryMode::Sealed))
}

pub(crate) fn open_child_directory_any_mode(
    parent: &File,
    parent_path: &Path,
    name: &str,
) -> Result<(File, RawIdentity, DirectoryMode), PrivateFsError> {
    let (file, identity) = open_relative_directory(parent, name, None)?;
    // Classify an already-stable case alias as hostile input, matching the
    // mode-specific openers. A later spelling failure during revalidation is
    // instead identity-ambiguous because the namespace may have raced after
    // this first exact-name proof.
    verify_exact_name(&file, name)?;
    let metadata = file
        .metadata()
        .map_err(|_| PrivateFsError::IdentityAmbiguous)?;
    // `open_relative_directory` already rejected a stable hostile mode. A
    // failure in this second descriptor snapshot therefore means the mode
    // changed after admission and must quarantine the namespace lease.
    let mode = admitted_directory_mode(&metadata).map_err(|_| PrivateFsError::IdentityAmbiguous)?;
    revalidate_child_directory(parent, parent_path, name, &file, identity, mode)?;
    Ok((file, identity, mode))
}

pub(crate) fn revalidate_child_directory(
    parent: &File,
    _parent_path: &Path,
    name: &str,
    directory: &File,
    expected: RawIdentity,
    mode: DirectoryMode,
) -> Result<(), PrivateFsError> {
    let metadata = directory
        .metadata()
        .map_err(|_| PrivateFsError::IdentityAmbiguous)?;
    if validate_private_directory_metadata_mode(&metadata, mode).is_err()
        || raw_identity(&metadata) != expected
        || !acl_is_private(directory)
    {
        return Err(PrivateFsError::IdentityAmbiguous);
    }
    verify_exact_name(directory, name).map_err(|_| PrivateFsError::IdentityAmbiguous)?;
    let (current, current_identity) = open_relative_directory(parent, name, Some(mode))
        .map_err(|_| PrivateFsError::IdentityAmbiguous)?;
    if current_identity != expected || !same_open_identity(&current, expected) {
        return Err(PrivateFsError::IdentityAmbiguous);
    }
    verify_exact_name(&current, name).map_err(|_| PrivateFsError::IdentityAmbiguous)
}

pub(crate) fn set_directory_mode(file: &File, mode: DirectoryMode) -> Result<(), PrivateFsError> {
    let raw_mode = match mode {
        DirectoryMode::Writable => 0o700,
        DirectoryMode::Sealed => 0o500,
    };
    rustix::fs::fchmod(file, Mode::from_raw_mode(raw_mode)).map_err(|_| PrivateFsError::Io)?;
    Ok(())
}

pub(crate) fn inspect_child(
    parent: &File,
    parent_path: &Path,
    name: &str,
) -> Result<Option<RawChildKind>, PrivateFsError> {
    let observed = match rustix::fs::statat(parent, name, rustix::fs::AtFlags::SYMLINK_NOFOLLOW) {
        Ok(observed) => observed,
        Err(rustix::io::Errno::NOENT) => return Ok(None),
        Err(_) => return Err(PrivateFsError::Unsafe),
    };
    inspect_observed_child(parent, parent_path, name, &observed).map(Some)
}

fn inspect_observed_child(
    parent: &File,
    parent_path: &Path,
    name: &str,
    observed: &rustix::fs::Stat,
) -> Result<RawChildKind, PrivateFsError> {
    let expected = raw_stat_identity(observed)?;
    match FileType::from_raw_mode(observed.st_mode) {
        FileType::RegularFile => {
            let (file, identity) = match open_regular(parent, parent_path, name, OpenPurpose::Read)
            {
                Err(PrivateFsError::NotFound) => {
                    return Err(PrivateFsError::IdentityAmbiguous);
                }
                outcome => outcome,
            }?;
            if identity != expected {
                return Err(PrivateFsError::IdentityAmbiguous);
            }
            verify_exact_name(&file, name)?;
            drop(file);
            Ok(RawChildKind::Regular(identity))
        }
        FileType::Directory => {
            let (directory, identity) = match open_relative_directory(parent, name, None) {
                Err(PrivateFsError::NotFound) => {
                    return Err(PrivateFsError::IdentityAmbiguous);
                }
                outcome => outcome,
            }?;
            if identity != expected {
                return Err(PrivateFsError::IdentityAmbiguous);
            }
            verify_exact_name(&directory, name)?;
            drop(directory);
            Ok(RawChildKind::Directory(identity))
        }
        _ => Err(PrivateFsError::Unsafe),
    }
}

#[cfg(target_os = "macos")]
pub(crate) fn verify_exact_name(file: &File, expected: &str) -> Result<(), PrivateFsError> {
    match super::macos_acl::has_exact_final_component(file.as_raw_fd(), expected) {
        Ok(true) => Ok(()),
        Ok(false) => Err(PrivateFsError::Unsafe),
        Err(()) => Err(PrivateFsError::IdentityAmbiguous),
    }
}

#[cfg(target_os = "linux")]
pub(crate) fn verify_exact_name(file: &File, expected: &str) -> Result<(), PrivateFsError> {
    const MAX_PROC_FD_TARGET_BYTES: usize = 4_096;

    let descriptor_path = format!("/proc/self/fd/{}", file.as_raw_fd());
    let mut target = [0_u8; MAX_PROC_FD_TARGET_BYTES];
    let length = rustix::fs::readlinkat_raw(rustix::fs::CWD, descriptor_path.as_str(), &mut target)
        .map_err(|error| {
            if [
                rustix::io::Errno::NOENT,
                rustix::io::Errno::ACCESS,
                rustix::io::Errno::PERM,
                rustix::io::Errno::NOSYS,
                rustix::io::Errno::RANGE,
            ]
            .contains(&error)
            {
                // A missing or restricted procfs means this platform cannot
                // prove exact case-folded entry spelling. Refuse the primitive
                // instead of silently weakening it.
                PrivateFsError::PrimitiveUnavailable
            } else {
                // The caller supplied a live owned descriptor; unexpected
                // descriptor/procfs failures make its binding ambiguous.
                PrivateFsError::IdentityAmbiguous
            }
        })?;
    if length == target.len() {
        return Err(PrivateFsError::PrimitiveUnavailable);
    }
    let target = &target[..length];
    let actual = target.rsplit(|byte| *byte == b'/').next().unwrap_or(target);
    if actual == expected.as_bytes() {
        Ok(())
    } else {
        Err(PrivateFsError::Unsafe)
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub(crate) fn verify_exact_name(_file: &File, _expected: &str) -> Result<(), PrivateFsError> {
    Err(PrivateFsError::PrimitiveUnavailable)
}

pub(crate) fn list_names(
    directory: &File,
    _directory_path: &Path,
    max_entries: usize,
) -> Result<Vec<String>, PrivateFsError> {
    let mut reader = rustix::fs::Dir::read_from(directory).map_err(|_| PrivateFsError::Io)?;
    let mut names = Vec::new();
    for entry in &mut reader {
        let entry = entry.map_err(|_| PrivateFsError::Io)?;
        let bytes = entry.file_name().to_bytes();
        if matches!(bytes, b"." | b"..") {
            continue;
        }
        if names.len() == max_entries {
            return Err(PrivateFsError::BoundExceeded);
        }
        names.push(
            std::str::from_utf8(bytes)
                .map_err(|_| PrivateFsError::Unsafe)?
                .to_owned(),
        );
    }
    Ok(names)
}

pub(crate) fn directory_is_empty(directory: &File) -> Result<bool, PrivateFsError> {
    let mut reader = rustix::fs::Dir::read_from(directory).map_err(|_| PrivateFsError::Io)?;
    for entry in &mut reader {
        let entry = entry.map_err(|_| PrivateFsError::Io)?;
        if !matches!(entry.file_name().to_bytes(), b"." | b"..") {
            return Ok(false);
        }
    }
    Ok(true)
}

pub(crate) fn remove_regular(
    directory: &File,
    _directory_path: &Path,
    name: &str,
) -> Result<(), PrivateFsError> {
    rustix::fs::unlinkat(directory, name, rustix::fs::AtFlags::empty())
        .map_err(|_| PrivateFsError::Io)?;
    Ok(())
}

pub(crate) fn remove_directory(
    parent: &File,
    _parent_path: &Path,
    name: &str,
) -> Result<(), PrivateFsError> {
    rustix::fs::unlinkat(parent, name, rustix::fs::AtFlags::REMOVEDIR).map_err(|error| {
        if matches!(
            error,
            rustix::io::Errno::NOTEMPTY | rustix::io::Errno::EXIST
        ) {
            PrivateFsError::DirectoryNotEmpty
        } else {
            PrivateFsError::Io
        }
    })?;
    Ok(())
}

pub(crate) fn open_directory(path: &Path) -> Result<(File, RawIdentity), PrivateFsError> {
    open_directory_with_mode(path, DirectoryMode::Writable)
}

pub(crate) fn open_directory_with_mode(
    path: &Path,
    mode: DirectoryMode,
) -> Result<(File, RawIdentity), PrivateFsError> {
    let before = std::fs::symlink_metadata(path).map_err(map_boundary_io)?;
    validate_private_directory_metadata_mode(&before, mode)?;
    let descriptor = rustix::fs::open(
        path,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
        Mode::empty(),
    )
    .map_err(|_| PrivateFsError::IdentityAmbiguous)?;
    let file = File::from(descriptor);
    let opened = file
        .metadata()
        .map_err(|_| PrivateFsError::IdentityAmbiguous)?;
    let after = std::fs::symlink_metadata(path).map_err(|_| PrivateFsError::IdentityAmbiguous)?;
    validate_private_directory_metadata_mode(&opened, mode)?;
    validate_private_directory_metadata_mode(&after, mode)?;
    let identity = raw_identity(&opened);
    if !acl_is_private(&file) {
        return Err(PrivateFsError::Unsafe);
    }
    if raw_identity(&before) != identity || raw_identity(&after) != identity {
        return Err(PrivateFsError::IdentityAmbiguous);
    }
    Ok((file, identity))
}

pub(crate) fn same_open_identity(file: &File, expected: RawIdentity) -> bool {
    file.metadata()
        .ok()
        .is_some_and(|metadata| raw_identity(&metadata) == expected)
}

pub(crate) fn validate_regular_metadata(metadata: &Metadata) -> Result<(), PrivateFsError> {
    if validate_regular_metadata_mode(metadata, RegularMode::Writable).is_ok()
        || validate_regular_metadata_mode(metadata, RegularMode::Sealed).is_ok()
    {
        Ok(())
    } else {
        Err(PrivateFsError::Unsafe)
    }
}

fn validate_regular_metadata_mode(
    metadata: &Metadata,
    expected: RegularMode,
) -> Result<(), PrivateFsError> {
    let expected_mode = match expected {
        RegularMode::Writable => 0o600,
        RegularMode::Sealed => 0o400,
    };
    if !metadata.is_file()
        || metadata.file_type().is_symlink()
        || metadata.nlink() != 1
        || metadata.uid() != current_user()
        || metadata.mode() & 0o7777 != expected_mode
    {
        return Err(PrivateFsError::Unsafe);
    }
    Ok(())
}

pub(crate) fn validate_private_directory_node(
    _path: &Path,
    metadata: &Metadata,
) -> Result<(), PrivateFsError> {
    validate_private_directory_metadata_mode(metadata, DirectoryMode::Writable)
}

fn validate_private_directory_metadata_mode(
    metadata: &Metadata,
    expected: DirectoryMode,
) -> Result<(), PrivateFsError> {
    let expected_mode = match expected {
        DirectoryMode::Writable => 0o700,
        DirectoryMode::Sealed => 0o500,
    };
    if !metadata.is_dir()
        || metadata.file_type().is_symlink()
        || metadata.uid() != current_user()
        || metadata.mode() & 0o7777 != expected_mode
    {
        return Err(PrivateFsError::Unsafe);
    }
    Ok(())
}

pub(crate) fn validate_ancestor_node(
    _path: &Path,
    metadata: &Metadata,
) -> Result<(), PrivateFsError> {
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(PrivateFsError::Unsafe);
    }
    let owner = metadata.uid();
    let mode = metadata.mode();
    if owner != 0 && owner != current_user() {
        return Err(PrivateFsError::Unsafe);
    }
    if mode & 0o022 != 0 && (owner != 0 || mode & 0o1000 == 0) {
        return Err(PrivateFsError::Unsafe);
    }
    Ok(())
}

pub(crate) fn lock_exclusive(file: &File) -> Result<(), PrivateFsError> {
    // A signal can interrupt flock before the kernel has made a locking
    // decision. Retry a small, deterministic number of times rather than
    // reporting a competing owner or spinning under a signal storm.
    const MAX_ATTEMPTS: usize = 4;
    for attempt in 0..MAX_ATTEMPTS {
        match rustix::fs::flock(file, rustix::fs::FlockOperation::NonBlockingLockExclusive) {
            Ok(()) => return Ok(()),
            Err(rustix::io::Errno::INTR) if attempt + 1 < MAX_ATTEMPTS => continue,
            Err(rustix::io::Errno::WOULDBLOCK) => {
                return Err(PrivateFsError::LockUnavailable);
            }
            Err(_) => return Err(PrivateFsError::Io),
        }
    }
    Err(PrivateFsError::Io)
}

pub(crate) fn unlock_exclusive(file: &File) {
    // Drop cannot report an error, and this crate has no logger. Interruption
    // handling stays bounded. If unlock fails, closing the descriptor releases
    // the lock only once no inherited duplicate remains open; until then,
    // admission reports `LockUnavailable` rather than sharing the namespace.
    for _ in 0..4 {
        match rustix::fs::flock(file, rustix::fs::FlockOperation::Unlock) {
            Err(rustix::io::Errno::INTR) => continue,
            _ => break,
        }
    }
}

pub(crate) fn atomic_replace(
    directory: &File,
    _directory_path: &Path,
    source: &str,
    destination: &str,
) -> Result<(), PrivateFsError> {
    rustix::fs::renameat(directory, source, directory, destination)
        .map_err(|_| PrivateFsError::Io)?;
    Ok(())
}

pub(crate) fn atomic_publish_noreplace(
    directory: &File,
    _directory_path: &Path,
    source: &str,
    destination: &str,
) -> Result<(), PrivateFsError> {
    use rustix::fs::{renameat_with, RenameFlags};

    renameat_with(
        directory,
        source,
        directory,
        destination,
        RenameFlags::NOREPLACE,
    )
    .map_err(|error| {
        if error == rustix::io::Errno::EXIST {
            PrivateFsError::AlreadyExists
        } else {
            PrivateFsError::Io
        }
    })?;
    Ok(())
}

pub(crate) fn atomic_publish_noreplace_between(
    source_directory: &File,
    _source_directory_path: &Path,
    source: &str,
    destination_directory: &File,
    _destination_directory_path: &Path,
    destination: &str,
) -> Result<(), PrivateFsError> {
    rename_noreplace_between(source_directory, source, destination_directory, destination)
        .map_err(|error| {
            if error == rustix::io::Errno::EXIST {
                PrivateFsError::AlreadyExists
            } else {
                PrivateFsError::Io
            }
        })?;
    Ok(())
}

// Keep the raw native result at this one syscall seam for the platform
// regression below. The public private-filesystem error contract is unchanged.
fn rename_noreplace_between(
    source_directory: &File,
    source: &str,
    destination_directory: &File,
    destination: &str,
) -> rustix::io::Result<()> {
    rustix::fs::renameat_with(
        source_directory,
        source,
        destination_directory,
        destination,
        rustix::fs::RenameFlags::NOREPLACE,
    )
}

pub(crate) fn sync_directory(file: &File) -> Result<(), PrivateFsError> {
    file.sync_all().map_err(|_| PrivateFsError::Io)?;
    Ok(())
}

pub(crate) fn sync_ancestor_directory(path: &Path) -> Result<(), PrivateFsError> {
    let before = std::fs::symlink_metadata(path).map_err(map_boundary_io)?;
    validate_ancestor_node(path, &before)?;
    let descriptor = rustix::fs::open(
        path,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
        Mode::empty(),
    )
    .map_err(|_| PrivateFsError::Unsafe)?;
    let file = File::from(descriptor);
    let opened = file.metadata().map_err(|_| PrivateFsError::Io)?;
    let after = std::fs::symlink_metadata(path).map_err(map_boundary_io)?;
    validate_ancestor_node(path, &opened)?;
    validate_ancestor_node(path, &after)?;
    if raw_identity(&before) != raw_identity(&opened)
        || raw_identity(&opened) != raw_identity(&after)
    {
        return Err(PrivateFsError::Unsafe);
    }
    file.sync_all().map_err(|_| PrivateFsError::Io)?;
    Ok(())
}

fn map_boundary_io(error: std::io::Error) -> PrivateFsError {
    if error.kind() == std::io::ErrorKind::NotFound {
        PrivateFsError::Unsafe
    } else {
        PrivateFsError::Io
    }
}

fn map_child_open_error(error: rustix::io::Errno) -> PrivateFsError {
    if error == rustix::io::Errno::NOENT {
        PrivateFsError::NotFound
    } else {
        PrivateFsError::Unsafe
    }
}

fn map_failed_create(
    error: rustix::io::Errno,
    parent: &File,
    name: &str,
    report_already_exists: bool,
) -> PrivateFsError {
    if report_already_exists && error == rustix::io::Errno::EXIST {
        PrivateFsError::AlreadyExists
    } else if relative_name_is_absent(parent, name) {
        // The held parent descriptor still reports no entry after the failed
        // creating syscall, so this operation has a proven clean outcome.
        PrivateFsError::Io
    } else {
        PrivateFsError::SettlementUnknown
    }
}

pub(crate) fn relative_name_is_absent(parent: &File, name: &str) -> bool {
    matches!(
        rustix::fs::statat(parent, name, rustix::fs::AtFlags::SYMLINK_NOFOLLOW),
        Err(rustix::io::Errno::NOENT)
    )
}

fn relative_regular_identity(directory: &File, name: &str) -> Result<RawIdentity, PrivateFsError> {
    let descriptor = rustix::fs::openat(
        directory,
        name,
        OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::NONBLOCK,
        Mode::empty(),
    )
    .map_err(|_| PrivateFsError::IdentityAmbiguous)?;
    let file = File::from(descriptor);
    let metadata = file
        .metadata()
        .map_err(|_| PrivateFsError::IdentityAmbiguous)?;
    validate_regular_metadata(&metadata).map_err(|_| PrivateFsError::IdentityAmbiguous)?;
    if !acl_is_private(&file) {
        return Err(PrivateFsError::IdentityAmbiguous);
    }
    Ok(raw_identity(&metadata))
}

#[allow(clippy::unnecessary_cast)]
fn raw_stat_identity(stat: &rustix::fs::Stat) -> Result<RawIdentity, PrivateFsError> {
    // rustix exposes target-native dev_t/ino_t widths. Both are nonnegative
    // kernel identifiers no wider than the u64 representation used by
    // `MetadataExt`; the casts are intentionally portable across Unix ABIs.
    Ok(RawIdentity {
        device: stat.st_dev as u64,
        inode: stat.st_ino as u64,
    })
}

fn open_relative_directory(
    parent: &File,
    name: &str,
    expected_mode: Option<DirectoryMode>,
) -> Result<(File, RawIdentity), PrivateFsError> {
    let descriptor = rustix::fs::openat(
        parent,
        name,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::NONBLOCK,
        Mode::empty(),
    )
    .map_err(map_child_open_error)?;
    let file = File::from(descriptor);
    let metadata = file
        .metadata()
        .map_err(|_| PrivateFsError::IdentityAmbiguous)?;
    match expected_mode {
        Some(mode) => validate_private_directory_metadata_mode(&metadata, mode)?,
        None => {
            if validate_private_directory_metadata_mode(&metadata, DirectoryMode::Writable).is_err()
                && validate_private_directory_metadata_mode(&metadata, DirectoryMode::Sealed)
                    .is_err()
            {
                return Err(PrivateFsError::Unsafe);
            }
        }
    }
    if !acl_is_private(&file) {
        return Err(PrivateFsError::Unsafe);
    }
    let second = rustix::fs::openat(
        parent,
        name,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::NONBLOCK,
        Mode::empty(),
    )
    .map_err(|_| PrivateFsError::IdentityAmbiguous)?;
    let second = File::from(second);
    let identity = raw_identity(&metadata);
    if !same_open_identity(&second, identity) || !acl_is_private(&second) {
        return Err(PrivateFsError::IdentityAmbiguous);
    }
    Ok((file, identity))
}

fn admitted_directory_mode(metadata: &Metadata) -> Result<DirectoryMode, PrivateFsError> {
    if validate_private_directory_metadata_mode(metadata, DirectoryMode::Writable).is_ok() {
        Ok(DirectoryMode::Writable)
    } else if validate_private_directory_metadata_mode(metadata, DirectoryMode::Sealed).is_ok() {
        Ok(DirectoryMode::Sealed)
    } else {
        Err(PrivateFsError::Unsafe)
    }
}

#[cfg(target_os = "macos")]
fn acl_is_private(file: &File) -> bool {
    super::macos_acl::has_no_extended_acl(file.as_raw_fd())
}

#[cfg(not(target_os = "macos"))]
fn acl_is_private(_file: &File) -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "macos")]
    #[test]
    fn nested_sealed_same_parent_publication_reports_native_operation_and_errno() {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};

        // Match the repository's nested tree, seal modes and independently
        // held same-parent descriptors. The root-only writable publication
        // transition is required by macOS 15; descendants stay sealed. There
        // is exactly one native NOREPLACE call per positive/negative case.
        let parent = tempfile::tempdir_in(std::env::current_dir().unwrap()).unwrap();
        let mode = |path: &Path, mode| {
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
        };
        mode(parent.path(), 0o700);
        let stage = parent.path().join("tree.stage");
        std::fs::create_dir(&stage).unwrap();
        for relative in ["assets", "assets/icons", "scripts"] {
            std::fs::create_dir(stage.join(relative)).unwrap();
            mode(&stage.join(relative), 0o700);
        }
        for relative in [
            "assets/icons/icon.txt",
            "manifest.json",
            "scripts/content.js",
        ] {
            let path = stage.join(relative);
            std::fs::write(&path, b"fixed test bytes").unwrap();
            mode(&path, 0o400);
            File::open(&path).unwrap().sync_all().unwrap();
        }
        for relative in ["assets/icons", "assets", "scripts", ""] {
            let path = stage.join(relative);
            mode(&path, 0o500);
            File::open(&path).unwrap().sync_all().unwrap();
        }
        let source_parent = File::open(parent.path()).unwrap();
        let destination_parent = File::open(parent.path()).unwrap();
        let occupied = parent.path().join("occupied.object");
        std::fs::create_dir(&occupied).unwrap();
        std::fs::write(occupied.join("sentinel"), b"must remain").unwrap();
        let sealed = File::open(&stage).unwrap();
        let identity = sealed.metadata().unwrap().ino();
        let publish = |root: &File, source: &str, destination: &str| {
            let fail =
                |operation, error: rustix::io::Errno| (operation, Some(error.raw_os_error()));
            assert_eq!(root.metadata().unwrap().mode() & 0o777, 0o500);
            rustix::fs::fchmod(root, Mode::from_raw_mode(0o700))
                .map_err(|error| fail("root_writable", error))?;
            let renamed =
                rename_noreplace_between(&source_parent, source, &destination_parent, destination);
            rustix::fs::fchmod(root, Mode::from_raw_mode(0o500))
                .map_err(|error| fail("root_reseal", error))?;
            root.sync_all()
                .map_err(|error| ("root_reseal_sync", error.raw_os_error()))?;
            assert_eq!(root.metadata().unwrap().mode() & 0o777, 0o500);
            renamed.map_err(|error| fail("rename_noreplace", error))
        };
        let mut destination_refusal = None;
        let outcome = (|| -> Result<(), (&str, Option<i32>)> {
            sealed
                .sync_all()
                .map_err(|error| ("pre_publish_sync", error.raw_os_error()))?;
            publish(&sealed, "tree.stage", "tree.object")?;
            source_parent
                .sync_all()
                .map_err(|error| ("post_publish_parent_sync", error.raw_os_error()))?;
            destination_refusal = Some(publish(&sealed, "tree.object", "occupied.object"));
            Ok(())
        })();
        if outcome.is_ok() {
            let installed = parent.path().join("tree.object");
            for relative in ["", "assets", "assets/icons", "scripts"] {
                assert_eq!(
                    std::fs::metadata(installed.join(relative)).unwrap().mode() & 0o777,
                    0o500
                );
            }
            for relative in [
                "assets/icons/icon.txt",
                "manifest.json",
                "scripts/content.js",
            ] {
                assert_eq!(
                    std::fs::metadata(installed.join(relative)).unwrap().mode() & 0o777,
                    0o400
                );
            }
        }
        // Restore only this fixture's exact directories for TempDir cleanup,
        // after the terminal outcome. This never authorizes a second attempt.
        for name in ["tree.stage", "tree.object"] {
            let root = parent.path().join(name);
            if root.exists() {
                for relative in ["", "assets", "assets/icons", "scripts"] {
                    mode(&root.join(relative), 0o700);
                }
            }
        }
        assert!(outcome.is_ok(), "native publication failed: {outcome:?}");
        assert_eq!(
            destination_refusal,
            Some(Err((
                "rename_noreplace",
                Some(rustix::io::Errno::EXIST.raw_os_error())
            ))),
            "native existing-destination refusal changed"
        );
        assert_eq!(
            std::fs::read(occupied.join("sentinel")).unwrap(),
            b"must remain"
        );
        assert!(!stage.exists());
        assert_eq!(
            std::fs::metadata(parent.path().join("tree.object"))
                .unwrap()
                .ino(),
            identity
        );
    }

    #[test]
    fn create_errors_are_clean_only_when_the_exact_name_is_absent() {
        let parent = tempfile::tempdir_in(std::env::current_dir().unwrap()).unwrap();
        let handle = File::open(parent.path()).unwrap();
        assert_eq!(
            map_failed_create(rustix::io::Errno::IO, &handle, "missing", true),
            PrivateFsError::Io
        );
        std::fs::write(parent.path().join("present"), b"payload").unwrap();
        assert_eq!(
            map_failed_create(rustix::io::Errno::IO, &handle, "present", true),
            PrivateFsError::SettlementUnknown
        );
        assert_eq!(
            map_failed_create(rustix::io::Errno::EXIST, &handle, "present", true),
            PrivateFsError::AlreadyExists
        );
    }

    #[test]
    fn inspection_rejects_identity_replacement_after_nofollow_observation() {
        use std::os::unix::fs::PermissionsExt;

        let parent = tempfile::tempdir_in(std::env::current_dir().unwrap()).unwrap();
        std::fs::set_permissions(parent.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let original = parent.path().join("Observed File");
        std::fs::write(&original, b"original").unwrap();
        std::fs::set_permissions(&original, std::fs::Permissions::from_mode(0o600)).unwrap();
        let handle = File::open(parent.path()).unwrap();
        let observed = rustix::fs::statat(
            &handle,
            "Observed File",
            rustix::fs::AtFlags::SYMLINK_NOFOLLOW,
        )
        .unwrap();

        std::fs::rename(&original, parent.path().join("Held Original")).unwrap();
        std::fs::write(&original, b"replacement").unwrap();
        std::fs::set_permissions(&original, std::fs::Permissions::from_mode(0o600)).unwrap();

        assert_eq!(
            inspect_observed_child(&handle, parent.path(), "Observed File", &observed),
            Err(PrivateFsError::IdentityAmbiguous)
        );
    }

    #[test]
    fn exact_name_probe_binds_the_live_descriptor_final_component() {
        use std::os::unix::fs::PermissionsExt;

        let parent = tempfile::tempdir_in(std::env::current_dir().unwrap()).unwrap();
        std::fs::set_permissions(parent.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let path = parent.path().join("Exact CASE Name");
        std::fs::write(&path, b"value").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        let file = File::open(path).unwrap();

        assert_eq!(verify_exact_name(&file, "Exact CASE Name"), Ok(()));
        assert_eq!(
            verify_exact_name(&file, "exact case name"),
            Err(PrivateFsError::Unsafe)
        );
    }

    #[test]
    fn regular_revalidation_rejects_a_case_renamed_held_inode() {
        use std::os::unix::fs::PermissionsExt;

        let parent = tempfile::tempdir_in(std::env::current_dir().unwrap()).unwrap();
        std::fs::set_permissions(parent.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let exact = "source.bin";
        let alias = "Source.BIN";
        let exact_path = parent.path().join(exact);
        std::fs::write(&exact_path, b"value").unwrap();
        std::fs::set_permissions(&exact_path, std::fs::Permissions::from_mode(0o600)).unwrap();
        let parent_handle = File::open(parent.path()).unwrap();
        let (file, identity) =
            open_regular(&parent_handle, parent.path(), exact, OpenPurpose::Mutation).unwrap();

        std::fs::rename(&exact_path, parent.path().join(alias)).unwrap();
        assert_eq!(
            revalidate_regular(&parent_handle, parent.path(), exact, &file, identity),
            Err(PrivateFsError::IdentityAmbiguous)
        );
    }
}
