//! User-mode NT handle-relative operations. No child pathname is resolved
//! through a previously observed ambient directory path.

use std::ffi::c_void;
use std::fs::File;
use std::os::windows::io::{AsRawHandle, FromRawHandle};
use windows::core::PWSTR;
use windows::Win32::Foundation::{HANDLE, UNICODE_STRING};
use windows::Win32::Security::PSECURITY_DESCRIPTOR;
use windows::Win32::Storage::FileSystem::{
    FileIdType, GetFileInformationByHandle, GetFinalPathNameByHandleW,
    GetVolumeInformationByHandleW, OpenFileById, ReOpenFile, BY_HANDLE_FILE_INFORMATION,
    FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_FLAG_WRITE_THROUGH,
    FILE_ID_DESCRIPTOR, FILE_ID_DESCRIPTOR_0, FILE_NAME_NORMALIZED, FILE_SHARE_MODE,
    GETFINALPATHNAMEBYHANDLE_FLAGS, VOLUME_NAME_DOS,
};
use windows::Win32::System::IO::IO_STATUS_BLOCK;

use crate::PrivateFsError;

fn terminated_path(path: &std::path::Path) -> std::io::Result<Vec<u16>> {
    use std::os::windows::ffi::OsStrExt;
    let mut path: Vec<u16> = path.as_os_str().encode_wide().collect();
    if path.contains(&0) {
        return Err(std::io::Error::from(std::io::ErrorKind::InvalidInput));
    }
    path.push(0);
    Ok(path)
}

pub(super) fn rename_file_no_replace(
    source: &std::path::Path,
    target: &std::path::Path,
) -> std::io::Result<()> {
    use windows::core::PCWSTR;
    use windows::Win32::Storage::FileSystem::{MoveFileExW, MOVE_FILE_FLAGS};
    let source = terminated_path(source)?;
    let target = terminated_path(target)?;
    // SAFETY: both paths are owned, reject interior NULs, and remain valid
    // through this synchronous call. Zero flags forbid destination replacement.
    unsafe {
        MoveFileExW(
            PCWSTR(source.as_ptr()),
            PCWSTR(target.as_ptr()),
            MOVE_FILE_FLAGS(0),
        )
    }
    .map_err(|_| std::io::Error::last_os_error())
}

#[cfg(test)]
mod rename_path_tests {
    use super::terminated_path;
    use std::path::Path;

    #[test]
    fn unicode_paths_are_terminated_and_interior_nuls_are_refused() {
        let path = Path::new("C:\\Notes\\旅行 🦊.md");
        assert_eq!(
            terminated_path(path).unwrap(),
            path.to_str()
                .unwrap()
                .encode_utf16()
                .chain(Some(0))
                .collect::<Vec<_>>()
        );
        assert_eq!(
            terminated_path(Path::new("C:\\Notes\\a\0b.md"))
                .unwrap_err()
                .kind(),
            std::io::ErrorKind::InvalidInput
        );
    }
}

#[repr(C)]
struct ObjectAttributes {
    length: u32,
    root_directory: HANDLE,
    object_name: *const UNICODE_STRING,
    attributes: u32,
    security_descriptor: *const c_void,
    security_quality_of_service: *const c_void,
}

#[link(name = "ntdll")]
unsafe extern "system" {
    fn NtCreateFile(
        file: *mut HANDLE,
        access: u32,
        attributes: *const ObjectAttributes,
        status: *mut IO_STATUS_BLOCK,
        allocation: *const i64,
        file_attributes: u32,
        share: u32,
        disposition: u32,
        options: u32,
        ea: *const c_void,
        ea_length: u32,
    ) -> i32;
    fn NtQueryDirectoryFile(
        file: HANDLE,
        event: HANDLE,
        routine: *const c_void,
        context: *const c_void,
        status: *mut IO_STATUS_BLOCK,
        information: *mut c_void,
        length: u32,
        class: u32,
        single: u8,
        name: *const UNICODE_STRING,
        restart: u8,
    ) -> i32;
    fn NtSetInformationFile(
        file: HANDLE,
        status: *mut IO_STATUS_BLOCK,
        information: *const c_void,
        length: u32,
        class: u32,
    ) -> i32;
    fn NtQueryVolumeInformationFile(
        file: HANDLE,
        status: *mut IO_STATUS_BLOCK,
        information: *mut c_void,
        length: u32,
        class: u32,
    ) -> i32;
    fn NtFlushBuffersFile(file: HANDLE, status: *mut IO_STATUS_BLOCK) -> i32;
    fn RtlNtStatusToDosError(status: i32) -> u32;
}

pub(super) const READ: u32 = 0x0012_0089; // FILE_GENERIC_READ
pub(super) const METADATA: u32 = 0x0005_0000; // DELETE | WRITE_DAC
pub(super) const WRITE: u32 = 0x0012_0116; // FILE_GENERIC_WRITE
pub(super) const TRAVERSE: u32 = 0x20; // directory-only FILE_TRAVERSE
pub(super) const OPEN: u32 = 1;
pub(super) const CREATE: u32 = 2;

pub(super) fn handle(file: &File) -> HANDLE {
    HANDLE(file.as_raw_handle())
}

fn name(value: &str) -> Result<Vec<u16>, PrivateFsError> {
    if value.is_empty()
        || matches!(value, "." | "..")
        || value.ends_with(['.', ' '])
        || value.chars().any(|c| matches!(c, '\\' | '/' | ':' | '\0'))
    {
        return Err(PrivateFsError::Unsafe);
    }
    let name: Vec<u16> = value.encode_utf16().collect();
    if name.len() > 255 {
        return Err(PrivateFsError::BoundExceeded);
    }
    Ok(name)
}

/// Only synchronous, single-component filesystem opens are accepted.
pub(super) fn open(
    parent: &File,
    child: &str,
    directory: Option<bool>,
    access: u32,
    share: u32,
    disposition: u32,
    security: Option<PSECURITY_DESCRIPTOR>,
) -> Result<File, PrivateFsError> {
    let mut buffer = name(child)?;
    let string = UNICODE_STRING {
        Length: (buffer.len() * 2) as u16,
        MaximumLength: (buffer.len() * 2) as u16,
        Buffer: PWSTR(buffer.as_mut_ptr()),
    };
    let attributes = ObjectAttributes {
        length: std::mem::size_of::<ObjectAttributes>() as u32,
        root_directory: handle(parent),
        object_name: &string,
        attributes: 0x40 | 0x1000, // OBJ_CASE_INSENSITIVE | OBJ_DONT_REPARSE
        security_descriptor: security.map_or(std::ptr::null(), |sd| sd.0),
        security_quality_of_service: std::ptr::null(),
    };
    let mut file = HANDLE::default();
    let mut io = IO_STATUS_BLOCK::default();
    let kind = match directory {
        Some(true) => 1,
        Some(false) => 0x40,
        None => 0,
    };
    // FILE_SYNCHRONOUS_IO_NONALERT | FILE_OPEN_REPARSE_POINT | FILE_WRITE_THROUGH.
    let options = kind | 0x20 | 0x0020_0000 | 2;
    // SAFETY: all buffers are aligned and live for this synchronous call;
    // parent is held and child contains exactly one non-special component.
    let status = unsafe {
        NtCreateFile(
            &raw mut file,
            access | 0x0010_0000 | if directory == Some(true) { TRAVERSE } else { 0 },
            &attributes,
            &raw mut io,
            std::ptr::null(),
            0x80,
            share,
            disposition,
            options,
            std::ptr::null(),
            0,
        )
    };
    if status < 0 {
        let failure = error(status);
        return Err(if disposition == CREATE {
            classify_create_error(parent, child, failure)
        } else {
            failure
        });
    }
    if file.is_invalid() {
        return Err(if disposition == CREATE {
            PrivateFsError::SettlementUnknown
        } else {
            PrivateFsError::IdentityAmbiguous
        });
    }
    if status != 0 {
        // An unexpected informational success still owns a returned handle.
        drop(unsafe { File::from_raw_handle(file.0) });
        return Err(if disposition == CREATE {
            PrivateFsError::SettlementUnknown
        } else {
            PrivateFsError::IdentityAmbiguous
        });
    }
    // SAFETY: a successful NtCreateFile transferred this handle to the caller.
    Ok(unsafe { File::from_raw_handle(file.0) })
}

pub(super) fn classify_create_error(
    parent: &File,
    child: &str,
    failure: PrivateFsError,
) -> PrivateFsError {
    // A failed create is clean only if exclusivity refused it or the exact
    // held-parent name is provably absent. Never grant reusable authority over
    // possible residue after an ambiguous filesystem/filter failure.
    if failure == PrivateFsError::AlreadyExists
        || matches!(
            open(parent, child, None, READ, 7, OPEN, None),
            Err(PrivateFsError::NotFound)
        )
    {
        failure
    } else {
        PrivateFsError::SettlementUnknown
    }
}

pub(super) fn error(status: i32) -> PrivateFsError {
    // SAFETY: pure NTSTATUS-to-Win32 error translation.
    match unsafe { RtlNtStatusToDosError(status) } {
        2 | 3 => PrivateFsError::NotFound,
        80 | 183 => PrivateFsError::AlreadyExists,
        145 => PrivateFsError::DirectoryNotEmpty,
        32 | 33 => PrivateFsError::LockUnavailable,
        4390..=4394 | 1920 => PrivateFsError::Unsafe,
        _ => PrivateFsError::Io,
    }
}

fn normalized_path(file: &File) -> Result<String, PrivateFsError> {
    let mut buffer = vec![0u16; 32_768];
    // SAFETY: GetFinalPathNameByHandleW returns the normalized long filename
    // for this live handle, allowing us to reject case and 8.3 aliases.
    let length = unsafe {
        GetFinalPathNameByHandleW(
            handle(file),
            &mut buffer,
            GETFINALPATHNAMEBYHANDLE_FLAGS(FILE_NAME_NORMALIZED.0 | VOLUME_NAME_DOS.0),
        )
    } as usize;
    if length == 0 || length >= buffer.len() {
        return Err(PrivateFsError::IdentityAmbiguous);
    }
    String::from_utf16(&buffer[..length]).map_err(|_| PrivateFsError::Unsafe)
}

pub(super) fn require_drive_root(file: &File, drive: u8) -> Result<(), PrivateFsError> {
    // A SUBST drive may hide ancestors outside the traversal below. Only an
    // actual normalized drive root can anchor the held-parent chain.
    let expected = format!("\\\\?\\{}:\\", char::from(drive));
    if !normalized_path(file)?.eq_ignore_ascii_case(&expected) {
        return Err(PrivateFsError::Unsafe);
    }
    Ok(())
}

pub(super) fn exact_name(file: &File, expected: &str) -> Result<(), PrivateFsError> {
    let path = normalized_path(file)?;
    if path.rsplit('\\').next() != Some(expected) {
        return Err(PrivateFsError::IdentityAmbiguous);
    }
    Ok(())
}

pub(super) fn names(directory: &File, max: usize) -> Result<Vec<String>, PrivateFsError> {
    const STATUS_NO_MORE_FILES: i32 = 0x8000_0006u32 as i32;
    let mut storage = vec![0u64; 8192];
    let mut result = Vec::new();
    let mut restart = 1;
    let mut batches = 0usize;
    loop {
        batches = batches
            .checked_add(1)
            .ok_or(PrivateFsError::BoundExceeded)?;
        // Even dot-only output cannot keep enumeration alive indefinitely.
        if batches > max.saturating_add(3) {
            return Err(PrivateFsError::BoundExceeded);
        }
        let mut io = IO_STATUS_BLOCK::default();
        // SAFETY: aligned bounded buffer; synchronous directory handle; class
        // FileNamesInformation contains only length-delimited UTF-16 names.
        let status = unsafe {
            NtQueryDirectoryFile(
                handle(directory),
                HANDLE::default(),
                std::ptr::null(),
                std::ptr::null(),
                &raw mut io,
                storage.as_mut_ptr().cast(),
                65_536,
                12,
                0,
                std::ptr::null(),
                restart,
            )
        };
        restart = 0;
        if status == STATUS_NO_MORE_FILES {
            break;
        }
        if status != 0 || io.Information == 0 || io.Information > 65_536 {
            return Err(PrivateFsError::Io);
        }
        // SAFETY: io.Information bounds the initialized bytes in this buffer.
        let bytes =
            unsafe { std::slice::from_raw_parts(storage.as_ptr().cast::<u8>(), io.Information) };
        let mut offset = 0;
        loop {
            let row = bytes
                .get(offset..)
                .filter(|row| row.len() >= 12)
                .ok_or(PrivateFsError::Unsafe)?;
            let next = u32::from_le_bytes(row[..4].try_into().map_err(|_| PrivateFsError::Unsafe)?)
                as usize;
            let length =
                u32::from_le_bytes(row[8..12].try_into().map_err(|_| PrivateFsError::Unsafe)?)
                    as usize;
            if length == 0
                || !length.is_multiple_of(2)
                || length > 510
                || 12 + length > row.len()
                || (next != 0 && (next < 12 + length || !next.is_multiple_of(4)))
            {
                return Err(PrivateFsError::Unsafe);
            }
            let wide: Vec<u16> = row[12..12 + length]
                .chunks_exact(2)
                .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
                .collect();
            let value = String::from_utf16(&wide).map_err(|_| PrivateFsError::Unsafe)?;
            if value != "." && value != ".." {
                name(&value)?;
                if result.len() >= max {
                    return Err(PrivateFsError::BoundExceeded);
                }
                result.push(value);
            }
            if next == 0 {
                break;
            }
            offset = offset
                .checked_add(next)
                .filter(|offset| *offset < bytes.len())
                .ok_or(PrivateFsError::Unsafe)?;
        }
    }
    result.sort_unstable();
    if result.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err(PrivateFsError::IdentityAmbiguous);
    }
    Ok(result)
}

#[repr(C)]
struct RenameHeader {
    flags: u32,
    root: HANDLE,
    length: u32,
    name: [u16; 1],
}

pub(super) fn rename(
    file: &File,
    parent: &File,
    destination: &str,
    replace: bool,
) -> Result<(), PrivateFsError> {
    let name = name(destination)?;
    let offset = std::mem::offset_of!(RenameHeader, name);
    let length = offset + name.len() * 2;
    let mut bytes = vec![0usize; length.div_ceil(std::mem::size_of::<usize>())];
    let header = bytes.as_mut_ptr().cast::<RenameHeader>();
    // SAFETY: aligned buffer includes the variable-length filename tail.
    unsafe {
        (*header).flags = 2 | u32::from(replace);
        (*header).root = handle(parent);
        (*header).length = (name.len() * 2) as u32;
        std::ptr::copy_nonoverlapping(
            name.as_ptr(),
            bytes.as_mut_ptr().cast::<u8>().add(offset).cast(),
            name.len(),
        );
    }
    let mut io = IO_STATUS_BLOCK::default();
    // FileRenameInformationEx, POSIX semantics: preserve held old file
    // handles while atomically replacing a name. There is no copy fallback.
    let status = unsafe {
        NtSetInformationFile(
            handle(file),
            &raw mut io,
            bytes.as_ptr().cast(),
            length as u32,
            65,
        )
    };
    if status != 0 {
        #[cfg(feature = "windows-namespace-validation")]
        {
            use std::io::Write as _;
            let _ = writeln!(
                std::io::stderr().lock(),
                "private-fs-native: stage=rename status={status:#x}"
            );
        }
        return Err(error(status));
    }
    Ok(())
}

pub(super) fn delete(file: &File) -> Result<(), PrivateFsError> {
    let flags: u32 = 3;
    let mut io = IO_STATUS_BLOCK::default();
    // FileDispositionInformationEx DELETE | POSIX_SEMANTICS removes the name
    // even while outer identity-check handles remain open. No deferred-delete
    // fallback may be mistaken for settled namespace removal.
    let status = unsafe {
        NtSetInformationFile(handle(file), &raw mut io, (&raw const flags).cast(), 4, 64)
    };
    if status != 0 {
        #[cfg(feature = "windows-namespace-validation")]
        {
            use std::io::Write as _;
            let _ = writeln!(
                std::io::stderr().lock(),
                "private-fs-native: stage=delete status={status:#x}"
            );
        }
        return Err(error(status));
    }
    Ok(())
}

pub(super) fn flush(file: &File) -> Result<(), PrivateFsError> {
    let mut io = IO_STATUS_BLOCK::default();
    // No successful no-op: a filesystem which cannot provide this barrier is
    // refused by live validation, and must not be enabled for publication.
    let status = unsafe { NtFlushBuffersFile(handle(file), &raw mut io) };
    if status == 0 {
        return Ok(());
    }
    if status != 0xc000_0022u32 as i32 {
        return Err(PrivateFsError::PrimitiveUnavailable);
    }
    // A sealed directory may have been reopened read-only and subsequently
    // unsealed for recovery. Reopen the held object, never its ambient path.
    // Sealed ACLs still deny this request; no ACL is changed by a flush.
    let reopened = reopen_writable(file)?;
    if super::identity(&reopened, None)? != super::identity(file, None)? {
        return Err(PrivateFsError::IdentityAmbiguous);
    }
    let status = unsafe { NtFlushBuffersFile(handle(&reopened), &raw mut io) };
    if status != 0 {
        return Err(PrivateFsError::PrimitiveUnavailable);
    }
    Ok(())
}

pub(super) fn reopen_writable(file: &File) -> Result<File, PrivateFsError> {
    let expected = super::identity(file, None)?;
    let security = super::security::snapshot(file)?;
    let access = READ | WRITE | METADATA | if expected.1 { TRAVERSE } else { 0 };
    let flags = FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_WRITE_THROUGH;
    let raw = if expected.1 {
        let mut info = BY_HANDLE_FILE_INFORMATION::default();
        // SAFETY: the original live directory remains held through ID lookup,
        // native reopening and complete identity/security revalidation.
        unsafe { GetFileInformationByHandle(handle(file), &raw mut info) }
            .map_err(|_| PrivateFsError::PrimitiveUnavailable)?;
        let descriptor = FILE_ID_DESCRIPTOR {
            dwSize: std::mem::size_of::<FILE_ID_DESCRIPTOR>() as u32,
            Type: FileIdType,
            Anonymous: FILE_ID_DESCRIPTOR_0 {
                FileId: (((info.nFileIndexHigh as u64) << 32) | info.nFileIndexLow as u64) as i64,
            },
        };
        // SAFETY: OpenFileById admits a held file as its same-volume hint and
        // supports directories with BACKUP_SEMANTICS. The 64-bit ID is only a
        // locator: the held original prevents reuse, and full 128-bit identity,
        // kind and owner/DACL are checked before any authority is returned.
        unsafe {
            OpenFileById(
                handle(file),
                &descriptor,
                access,
                FILE_SHARE_MODE(7),
                None,
                flags,
            )
        }
    } else {
        // SAFETY: ReOpenFile reopens the held regular file without resolving
        // an ambient path; the returned handle is independently owned.
        unsafe { ReOpenFile(handle(file), access, FILE_SHARE_MODE(7), flags) }
    }
    .map_err(|_| PrivateFsError::PrimitiveUnavailable)?;
    if raw.is_invalid() {
        return Err(PrivateFsError::PrimitiveUnavailable);
    }
    // SAFETY: the successful native open transfers an independently owned handle.
    let reopened = unsafe { File::from_raw_handle(raw.0) };
    if super::identity(&reopened, None)? != expected
        || super::identity(file, None)? != expected
        || super::security::snapshot(&reopened)? != security
        || super::security::snapshot(file)? != security
    {
        return Err(PrivateFsError::IdentityAmbiguous);
    }
    Ok(reopened)
}

pub(super) fn require_local_ntfs(file: &File) -> Result<(), PrivateFsError> {
    // FILE_FS_DEVICE_INFORMATION (class 4), queried from the held drive root.
    // Drive letters can name SMB mappings; a DOS spelling is not locality.
    let mut device = [0u32; 2];
    let mut io = IO_STATUS_BLOCK::default();
    let status = unsafe {
        NtQueryVolumeInformationFile(handle(file), &raw mut io, device.as_mut_ptr().cast(), 8, 4)
    };
    if status != 0 || io.Information != 8 || device[0] != 7 || device[1] & 0x10 != 0 {
        return Err(PrivateFsError::PrimitiveUnavailable);
    }
    let mut filesystem = [0u16; 32];
    let mut flags = 0;
    unsafe {
        GetVolumeInformationByHandleW(
            handle(file),
            None,
            None,
            None,
            Some(&raw mut flags),
            Some(&mut filesystem),
        )
    }
    .map_err(|_| PrivateFsError::PrimitiveUnavailable)?;
    let length = filesystem
        .iter()
        .position(|c| *c == 0)
        .ok_or(PrivateFsError::Unsafe)?;
    if filesystem[..length] != ['N' as u16, 'T' as u16, 'F' as u16, 'S' as u16] || flags & 8 == 0 {
        // FILE_PERSISTENT_ACLS
        return Err(PrivateFsError::PrimitiveUnavailable);
    }
    Ok(())
}
