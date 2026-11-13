//! Handle-relative Windows private namespace adapter. Shipping admission stays
//! gated until the Windows validation campaign has established live behavior.

#![allow(unsafe_code)]

mod native;
pub(super) mod native_storage;
mod security;
#[cfg(test)]
mod tests;

use std::fs::{File, Metadata};
use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Component, Path, Prefix};
use windows::Win32::Foundation::{GetLastError, ERROR_LOCK_VIOLATION};
use windows::Win32::Storage::FileSystem::{
    FileIdInfo, GetFileInformationByHandle, GetFileInformationByHandleEx, LockFileEx,
    BY_HANDLE_FILE_INFORMATION, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
    FILE_ID_INFO, LOCKFILE_EXCLUSIVE_LOCK, LOCKFILE_FAIL_IMMEDIATELY,
};
use windows::Win32::System::IO::OVERLAPPED;

use super::{DirectoryMode, RegularMode};
use crate::PrivateFsError;

pub(crate) fn rename_file_no_replace(source: &Path, target: &Path) -> std::io::Result<()> {
    if !std::fs::symlink_metadata(source)?.file_type().is_file() {
        return Err(std::io::Error::from(std::io::ErrorKind::InvalidInput));
    }
    native::rename_file_no_replace(source, target)
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) struct RawIdentity {
    volume_serial_number: u64,
    file_id: [u8; 16],
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

pub(crate) fn admit_namespace_support() -> Result<(), PrivateFsError> {
    if cfg!(feature = "windows-namespace-validation") {
        Ok(())
    } else {
        Err(PrivateFsError::PrimitiveUnavailable)
    }
}

fn identity(file: &File, directory: Option<bool>) -> Result<(RawIdentity, bool), PrivateFsError> {
    identity_impl(file, directory, false)
}

fn identity_impl(
    file: &File,
    directory: Option<bool>,
    allow_unlinked_held_file: bool,
) -> Result<(RawIdentity, bool), PrivateFsError> {
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    unsafe { GetFileInformationByHandle(native::handle(file), &raw mut info) }
        .map_err(|_| PrivateFsError::Unsafe)?;
    let is_directory = info.dwFileAttributes & 0x10 != 0;
    if info.dwFileAttributes & 0x400 != 0
        || directory.is_some_and(|kind| kind != is_directory)
        || (!is_directory
            && info.nNumberOfLinks != 1
            && !(allow_unlinked_held_file && info.nNumberOfLinks == 0))
    {
        return Err(PrivateFsError::Unsafe);
    }
    let mut id = FILE_ID_INFO::default();
    unsafe {
        GetFileInformationByHandleEx(
            native::handle(file),
            FileIdInfo,
            (&raw mut id).cast(),
            std::mem::size_of::<FILE_ID_INFO>() as u32,
        )
    }
    .map_err(|_| PrivateFsError::PrimitiveUnavailable)?;
    Ok((
        RawIdentity {
            volume_serial_number: id.VolumeSerialNumber,
            file_id: id.FileId.Identifier,
        },
        is_directory,
    ))
}

fn open_child(
    parent: &File,
    name: &str,
    directory: bool,
    access: u32,
    share: u32,
) -> Result<(File, RawIdentity, bool), PrivateFsError> {
    let file = native::open(
        parent,
        name,
        Some(directory),
        access,
        share,
        native::OPEN,
        None,
    )?;
    let id = identity(&file, Some(directory))?.0;
    native::exact_name(&file, name)?;
    let sealed = security::mode(&file)?;
    Ok((file, id, sealed))
}

pub(crate) fn open_regular(
    directory: &File,
    _path: &Path,
    name: &str,
    purpose: OpenPurpose,
) -> Result<(File, RawIdentity), PrivateFsError> {
    let access = match purpose {
        OpenPurpose::Read => native::READ,
        OpenPurpose::Mutation => native::READ | native::METADATA,
        OpenPurpose::Lock => native::READ | native::WRITE,
    };
    // The exact lock is enforced by LockFileEx and identity revalidation.
    // Share-delete is required for recovery to publish a held staging lock.
    let (mut file, id, sealed) = open_child(directory, name, false, access, 7)?;
    if matches!(purpose, OpenPurpose::Lock) && sealed {
        return Err(PrivateFsError::Unsafe);
    }
    if matches!(purpose, OpenPurpose::Mutation) && !sealed {
        file = native::reopen_writable(&file)?;
        if identity(&file, Some(false))?.0 != id || security::mode(&file)? {
            return Err(PrivateFsError::IdentityAmbiguous);
        }
    }
    Ok((file, id))
}

pub(crate) fn open_sealed_regular(
    parent: &File,
    _path: &Path,
    name: &str,
) -> Result<(File, RawIdentity), PrivateFsError> {
    let (file, id, sealed) = open_child(parent, name, false, native::READ | native::METADATA, 7)?;
    if !sealed {
        return Err(PrivateFsError::Unsafe);
    }
    Ok((file, id))
}

pub(crate) fn create_new_regular(
    parent: &File,
    _path: &Path,
    name: &str,
) -> Result<(File, RawIdentity), PrivateFsError> {
    let sd = security::descriptor(false, false)?;
    let file = native::open(
        parent,
        name,
        Some(false),
        native::READ | native::WRITE | native::METADATA,
        7,
        native::CREATE,
        Some(sd.0),
    )?;
    let verified = (|| {
        let id = identity(&file, Some(false))?.0;
        if security::mode(&file)? {
            return Err(PrivateFsError::Unsafe);
        }
        native::exact_name(&file, name)?;
        Ok(id)
    })();
    match verified {
        Ok(id) => Ok((file, id)),
        Err(_) => Err(PrivateFsError::SettlementUnknown),
    }
}

pub(crate) fn revalidate_regular(
    parent: &File,
    path: &Path,
    name: &str,
    file: &File,
    expected: RawIdentity,
) -> Result<(), PrivateFsError> {
    if identity(file, Some(false))?.0 != expected {
        return Err(PrivateFsError::IdentityAmbiguous);
    }
    security::mode(file)?;
    native::exact_name(file, name)?;
    let (_, current) = open_regular(parent, path, name, OpenPurpose::Read)?;
    if current != expected {
        return Err(PrivateFsError::IdentityAmbiguous);
    }
    Ok(())
}

pub(crate) fn revalidate_regular_mode(
    parent: &File,
    path: &Path,
    name: &str,
    file: &File,
    expected: RawIdentity,
    mode: RegularMode,
) -> Result<(), PrivateFsError> {
    revalidate_regular(parent, path, name, file, expected)?;
    if security::mode(file)? != (mode == RegularMode::Sealed) {
        return Err(PrivateFsError::IdentityAmbiguous);
    }
    Ok(())
}

pub(crate) fn set_regular_mode(file: &File, mode: RegularMode) -> Result<(), PrivateFsError> {
    identity(file, Some(false))?;
    security::set_mode(file, mode == RegularMode::Sealed)
}

pub(crate) fn create_directory(
    parent: &File,
    _path: &Path,
    name: &str,
) -> Result<bool, PrivateFsError> {
    let sd = security::descriptor(false, true)?;
    match native::open(
        parent,
        name,
        Some(true),
        native::READ | native::WRITE | native::METADATA,
        7,
        native::CREATE,
        Some(sd.0),
    ) {
        Ok(file) => {
            if identity(&file, Some(true)).is_err()
                || security::mode(&file) != Ok(false)
                || native::exact_name(&file, name).is_err()
            {
                return Err(PrivateFsError::SettlementUnknown);
            }
            Ok(true)
        }
        Err(PrivateFsError::AlreadyExists) => Ok(false),
        Err(error) => Err(error),
    }
}

pub(crate) fn open_child_directory(
    parent: &File,
    _path: &Path,
    name: &str,
) -> Result<(File, RawIdentity), PrivateFsError> {
    let (file, id, sealed) = open_child(
        parent,
        name,
        true,
        native::READ | native::WRITE | native::METADATA,
        7,
    )?;
    if sealed {
        return Err(PrivateFsError::Unsafe);
    }
    Ok((file, id))
}
pub(crate) fn open_sealed_child_directory(
    parent: &File,
    _path: &Path,
    name: &str,
) -> Result<(File, RawIdentity), PrivateFsError> {
    let (file, id, sealed) = open_child(parent, name, true, native::READ | native::METADATA, 7)?;
    if !sealed {
        return Err(PrivateFsError::Unsafe);
    }
    Ok((file, id))
}
pub(crate) fn open_child_directory_any_mode(
    parent: &File,
    _path: &Path,
    name: &str,
) -> Result<(File, RawIdentity, DirectoryMode), PrivateFsError> {
    let (file, id, sealed) = open_child(parent, name, true, native::READ | native::METADATA, 7)?;
    let file = if sealed {
        file
    } else {
        native::reopen_writable(&file)?
    };
    if identity(&file, Some(true))?.0 != id {
        return Err(PrivateFsError::IdentityAmbiguous);
    }
    Ok((
        file,
        id,
        if sealed {
            DirectoryMode::Sealed
        } else {
            DirectoryMode::Writable
        },
    ))
}
pub(crate) fn revalidate_child_directory(
    parent: &File,
    path: &Path,
    name: &str,
    directory: &File,
    expected: RawIdentity,
    mode: DirectoryMode,
) -> Result<(), PrivateFsError> {
    if identity(directory, Some(true))?.0 != expected {
        return Err(PrivateFsError::IdentityAmbiguous);
    }
    native::exact_name(directory, name)?;
    let (_, current, current_mode) = open_child_directory_any_mode(parent, path, name)?;
    if current != expected
        || current_mode != mode
        || security::mode(directory)? != (mode == DirectoryMode::Sealed)
    {
        return Err(PrivateFsError::IdentityAmbiguous);
    }
    Ok(())
}
pub(crate) fn set_directory_mode(file: &File, mode: DirectoryMode) -> Result<(), PrivateFsError> {
    identity(file, Some(true))?;
    security::set_mode(file, mode == DirectoryMode::Sealed)?;
    Ok(())
}

pub(crate) fn inspect_child(
    parent: &File,
    _path: &Path,
    name: &str,
) -> Result<Option<RawChildKind>, PrivateFsError> {
    let file = match native::open(parent, name, None, native::READ, 7, native::OPEN, None) {
        Ok(file) => file,
        Err(PrivateFsError::NotFound) => return Ok(None),
        Err(error) => return Err(error),
    };
    let (id, dir) = identity(&file, None)?;
    native::exact_name(&file, name)?;
    security::mode(&file)?;
    Ok(Some(if dir {
        RawChildKind::Directory(id)
    } else {
        RawChildKind::Regular(id)
    }))
}
pub(crate) fn verify_exact_name(file: &File, expected: &str) -> Result<(), PrivateFsError> {
    native::exact_name(file, expected)
}
pub(crate) fn list_names(
    file: &File,
    _path: &Path,
    max: usize,
) -> Result<Vec<String>, PrivateFsError> {
    native::names(file, max)
}
pub(crate) fn directory_is_empty(file: &File) -> Result<bool, PrivateFsError> {
    match native::names(file, 0) {
        Ok(_) => Ok(true),
        Err(PrivateFsError::BoundExceeded) => Ok(false),
        Err(error) => Err(error),
    }
}
pub(crate) fn relative_name_is_absent(parent: &File, name: &str) -> bool {
    matches!(
        native::open(parent, name, None, native::READ, 7, native::OPEN, None),
        Err(PrivateFsError::NotFound)
    )
}
pub(crate) fn remove_regular(parent: &File, path: &Path, name: &str) -> Result<(), PrivateFsError> {
    let (file, _) = open_regular(parent, path, name, OpenPurpose::Mutation)?;
    native::delete(&file)?;
    drop(file);
    if !relative_name_is_absent(parent, name) {
        return Err(PrivateFsError::IdentityAmbiguous);
    }
    Ok(())
}
pub(crate) fn remove_directory(
    parent: &File,
    _path: &Path,
    name: &str,
) -> Result<(), PrivateFsError> {
    // Unlink needs the named directory link, not a by-ID handle used for
    // writable flushing. The held-parent open already grants DELETE and
    // validates exact spelling, identity, kind and private DACL.
    let (file, _, _) = open_child(parent, name, true, native::READ | native::METADATA, 7)?;
    native::delete(&file)?;
    drop(file);
    if !relative_name_is_absent(parent, name) {
        return Err(PrivateFsError::IdentityAmbiguous);
    }
    Ok(())
}

// Only a local drive root is opened ambiently. Every subsequent component is
// opened against its held parent with reparse traversal disabled.
fn absolute_directory(path: &Path, private_leaf: bool) -> Result<File, PrivateFsError> {
    let mut components = path.components();
    let drive = match components.next() {
        Some(Component::Prefix(prefix)) => match prefix.kind() {
            Prefix::Disk(drive) | Prefix::VerbatimDisk(drive) => drive,
            _ => return Err(PrivateFsError::Unsafe),
        },
        _ => return Err(PrivateFsError::Unsafe),
    };
    if !matches!(components.next(), Some(Component::RootDir)) {
        return Err(PrivateFsError::Unsafe);
    }
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .access_mode(native::READ | native::TRAVERSE)
        .share_mode(7)
        .custom_flags((FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT).0)
        .open(format!("{}:\\", char::from(drive)))
        .map_err(|_| PrivateFsError::Unsafe)?;
    identity(&file, Some(true))?;
    native::require_local_ntfs(&file)?;
    native::require_drive_root(&file, drive)?;
    security::ancestor(&file)?;
    let names = components
        .map(|part| match part {
            Component::Normal(name) => name.to_str().ok_or(PrivateFsError::Unsafe),
            _ => Err(PrivateFsError::Unsafe),
        })
        .collect::<Result<Vec<_>, _>>()?;
    if names.is_empty() && private_leaf {
        return Err(PrivateFsError::Unsafe);
    }
    for (index, name) in names.iter().enumerate() {
        let last = index + 1 == names.len();
        let access = native::READ
            | if last && private_leaf {
                native::METADATA
            } else {
                0
            };
        file = native::open(&file, name, Some(true), access, 7, native::OPEN, None)?;
        identity(&file, Some(true))?;
        native::exact_name(&file, name)?;
        if last && private_leaf {
            security::mode(&file)?;
        } else {
            security::ancestor(&file)?;
        }
    }
    if private_leaf && !security::mode(&file)? {
        let id = identity(&file, Some(true))?.0;
        file = native::reopen_writable(&file)?;
        if identity(&file, Some(true))?.0 != id {
            return Err(PrivateFsError::IdentityAmbiguous);
        }
    }
    Ok(file)
}

pub(crate) fn open_directory(path: &Path) -> Result<(File, RawIdentity), PrivateFsError> {
    open_directory_with_mode(path, DirectoryMode::Writable)
}
pub(crate) fn open_directory_with_mode(
    path: &Path,
    mode: DirectoryMode,
) -> Result<(File, RawIdentity), PrivateFsError> {
    let file = absolute_directory(path, true)?;
    let id = identity(&file, Some(true))?.0;
    if security::mode(&file)? != (mode == DirectoryMode::Sealed) {
        return Err(PrivateFsError::Unsafe);
    }
    Ok((file, id))
}
pub(crate) fn create_private_root(path: &Path) -> Result<bool, PrivateFsError> {
    let parent = path.parent().ok_or(PrivateFsError::Unsafe)?;
    let directory = absolute_directory(parent, false)?;
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or(PrivateFsError::Unsafe)?;
    create_directory(&directory, parent, name)
}
pub(crate) fn same_open_identity(file: &File, expected: RawIdentity) -> bool {
    // Deletion settlement can inspect the original held, now unlinked regular
    // file. New opens and admission still require exactly one link.
    identity_impl(file, None, true).is_ok_and(|(id, _)| id == expected)
}
pub(crate) fn validate_private_directory_node(
    path: &Path,
    metadata: &Metadata,
) -> Result<(), PrivateFsError> {
    if !metadata.is_dir() || metadata.file_attributes() & 0x400 != 0 {
        return Err(PrivateFsError::Unsafe);
    }
    let file = absolute_directory(path, true)?;
    if security::mode(&file)? {
        return Err(PrivateFsError::Unsafe);
    }
    Ok(())
}
pub(crate) fn validate_ancestor_node(
    path: &Path,
    metadata: &Metadata,
) -> Result<(), PrivateFsError> {
    if !metadata.is_dir() || metadata.file_attributes() & 0x400 != 0 {
        return Err(PrivateFsError::Unsafe);
    }
    absolute_directory(path, false).map(|_| ())
}
pub(crate) fn lock_exclusive(file: &File) -> Result<(), PrivateFsError> {
    let mut overlapped = OVERLAPPED::default();
    if unsafe {
        LockFileEx(
            native::handle(file),
            LOCKFILE_EXCLUSIVE_LOCK | LOCKFILE_FAIL_IMMEDIATELY,
            None,
            u32::MAX,
            u32::MAX,
            &raw mut overlapped,
        )
    }
    .is_ok()
    {
        return Ok(());
    }
    if unsafe { GetLastError() } == ERROR_LOCK_VIOLATION {
        Err(PrivateFsError::LockUnavailable)
    } else {
        Err(PrivateFsError::Io)
    }
}

fn rename(
    parent: &File,
    source: &str,
    destination_parent: &File,
    destination: &str,
    replace: bool,
) -> Result<(), PrivateFsError> {
    let file = native::open(
        parent,
        source,
        None,
        native::READ | native::METADATA,
        7,
        native::OPEN,
        None,
    )?;
    identity(&file, None)?;
    security::mode(&file)?;
    native::exact_name(&file, source)?;
    native::rename(&file, destination_parent, destination, replace)?;
    Ok(())
}
pub(crate) fn atomic_replace(
    parent: &File,
    _path: &Path,
    source: &str,
    destination: &str,
) -> Result<(), PrivateFsError> {
    rename(parent, source, parent, destination, true)
}
pub(crate) fn atomic_publish_noreplace(
    parent: &File,
    _path: &Path,
    source: &str,
    destination: &str,
) -> Result<(), PrivateFsError> {
    rename(parent, source, parent, destination, false)
}
pub(crate) fn atomic_publish_noreplace_between(
    parent: &File,
    _path: &Path,
    source: &str,
    target: &File,
    _target_path: &Path,
    destination: &str,
) -> Result<(), PrivateFsError> {
    rename(parent, source, target, destination, false)
}
pub(crate) fn sync_directory(file: &File) -> Result<(), PrivateFsError> {
    identity(file, Some(true))?;
    native::flush(file)?;
    Ok(())
}
pub(crate) fn sync_regular(file: &File) -> Result<(), PrivateFsError> {
    identity(file, Some(false))?;
    native::flush(file)
}
pub(crate) fn sync_ancestor_directory(path: &Path) -> Result<(), PrivateFsError> {
    sync_directory(&absolute_directory(path, false)?)
}
