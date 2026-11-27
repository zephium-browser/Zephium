use std::path::Path;

pub(crate) fn publish_file(temporary: &Path, destination: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows::core::PCWSTR;
    use windows::Win32::Storage::FileSystem::{
        MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
    };
    let terminated = |path: &Path| {
        let mut wide: Vec<u16> = path.as_os_str().encode_wide().collect();
        if wide.contains(&0) {
            return Err(std::io::Error::from(std::io::ErrorKind::InvalidInput));
        }
        wide.push(0);
        Ok(wide)
    };
    let temporary = terminated(temporary)?;
    let destination = terminated(destination)?;
    // SAFETY: owned NUL-terminated paths reject interior NUL and live through
    // the call. The content-addressed destination is replaced only by its own
    // admitted bytes, and write-through settles publication before return.
    unsafe {
        MoveFileExW(
            PCWSTR(temporary.as_ptr()),
            PCWSTR(destination.as_ptr()),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    }
    .map_err(|_| std::io::Error::last_os_error())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct WindowsFileIdentity {
    volume_serial_number: u64,
    file_id: [u8; 16],
}

/// Opens `path` without traversing a final reparse point and returns its exact
/// filesystem identity only when the handle names a regular, single-link file.
///
/// `FileIdInfo` supplies the full 128-bit identifier required for ReFS. The
/// older `BY_HANDLE_FILE_INFORMATION` index is intentionally used only for
/// attributes and link count because its 64-bit identifier is not guaranteed
/// unique on ReFS.
pub(crate) fn verified_file_identity(path: &Path) -> Option<WindowsFileIdentity> {
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::AsRawHandle;
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::Storage::FileSystem::{
        FileIdInfo, GetFileInformationByHandle, GetFileInformationByHandleEx,
        BY_HANDLE_FILE_INFORMATION, FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_REPARSE_POINT,
        FILE_FLAG_OPEN_REPARSE_POINT, FILE_ID_INFO, FILE_SHARE_DELETE, FILE_SHARE_READ,
        FILE_SHARE_WRITE,
    };

    let file = std::fs::OpenOptions::new()
        .read(true)
        .share_mode((FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE).0)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT.0)
        .open(path)
        .ok()?;
    let handle = HANDLE(file.as_raw_handle());
    let mut information = BY_HANDLE_FILE_INFORMATION::default();
    unsafe {
        GetFileInformationByHandle(handle, &mut information).ok()?;
    }
    if information.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY.0 != 0
        || information.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0
        || information.nNumberOfLinks != 1
    {
        return None;
    }

    let mut identity = FILE_ID_INFO::default();
    unsafe {
        GetFileInformationByHandleEx(
            handle,
            FileIdInfo,
            (&raw mut identity).cast(),
            u32::try_from(std::mem::size_of::<FILE_ID_INFO>()).ok()?,
        )
        .ok()?;
    }

    Some(WindowsFileIdentity {
        volume_serial_number: identity.VolumeSerialNumber,
        file_id: identity.FileId.Identifier,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_is_stable_for_the_same_regular_file() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("database.sqlite");
        std::fs::write(&path, b"database").unwrap();

        let first = verified_file_identity(&path).unwrap();
        let second = verified_file_identity(&path).unwrap();
        assert_eq!(first, second);
    }

    #[test]
    fn hard_link_is_rejected_fail_closed() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("database.sqlite");
        let alias = directory.path().join("alias.sqlite");
        std::fs::write(&path, b"database").unwrap();
        std::fs::hard_link(&path, alias).unwrap();

        assert!(verified_file_identity(&path).is_none());
    }

    #[test]
    fn directory_is_rejected_fail_closed() {
        let directory = tempfile::tempdir().unwrap();

        assert!(verified_file_identity(directory.path()).is_none());
    }

    #[test]
    fn missing_path_is_rejected_fail_closed() {
        let directory = tempfile::tempdir().unwrap();
        let missing = directory.path().join("missing.sqlite");

        assert!(verified_file_identity(&missing).is_none());
    }
}
