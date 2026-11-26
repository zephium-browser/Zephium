//! Ordinary Win32 spelling at native WebView2 filesystem boundaries.
//! Canonical paths remain the authority for confinement and identity checks.

use std::ffi::OsString;
use std::io;
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};

/// The caller pins the canonical parent against replacement for the transfer.
/// WebView2 accepts verbatim paths but fails their downloads with FILE_FAILED.
pub(crate) fn webview2_download_path(canonical: &Path) -> io::Result<PathBuf> {
    let projected = project_spelling(canonical)?;
    let parent = canonical.parent().ok_or_else(invalid)?;
    if std::fs::canonicalize(projected.parent().ok_or_else(invalid)?)? != parent
        || !std::fs::symlink_metadata(parent)?.is_dir()
    {
        return Err(invalid());
    }
    Ok(projected)
}

pub(crate) fn webview2_user_data_path(canonical: &Path) -> io::Result<PathBuf> {
    let projected = project_spelling(canonical)?;
    // Rust may use verbatim spelling internally for filesystem operations.
    // Lexical checks therefore reject Win32 aliases before this roundtrip
    // proves that the projected path retains the exact canonical directory.
    if std::fs::canonicalize(&projected)? != canonical
        || !std::fs::symlink_metadata(canonical)?.is_dir()
    {
        return Err(invalid());
    }
    // A native short name changes only the spelling passed to WebView2.
    // All confinement, erasure and environment proofs retain canonical paths.
    let shorter = verified_short_path::shorter(canonical, &projected)?;
    #[cfg(all(debug_assertions, feature = "native-agentic-work-lifetime-diagnostic"))]
    if std::env::var_os("ZEPHIUM_WINDOWS_WORK_DIAGNOSTIC_UDF_SHORT_PATH").as_deref()
        == Some(std::ffi::OsStr::new("1"))
        && shorter.is_none()
    {
        return Err(io::Error::other(
            "the diagnostic UDF has no shorter verified alias",
        ));
    }
    Ok(shorter.unwrap_or(projected))
}

fn project_spelling(canonical: &Path) -> io::Result<PathBuf> {
    let units: Vec<u16> = canonical.as_os_str().encode_wide().collect();
    let prefix = [b'\\' as u16, b'\\' as u16, b'?' as u16, b'\\' as u16];
    if units.len() < 7
        || units[..4] != prefix
        || !matches!(units[4], 65..=90 | 97..=122)
        || units[5] != b':' as u16
        || units[6] != b'\\' as u16
    {
        return Err(invalid());
    }
    if units.len() > 7 {
        for component in units[7..].split(|unit| *unit == b'\\' as u16) {
            validate_component(component)?;
        }
    }
    Ok(PathBuf::from(OsString::from_wide(&units[4..])))
}

fn validate_component(component: &[u16]) -> io::Result<()> {
    if component.is_empty()
        || matches!(component.last(), Some(32 | 46))
        || component
            .iter()
            .any(|unit| matches!(*unit, 0..=31 | 34 | 42 | 47 | 58 | 60 | 62 | 63 | 124))
    {
        return Err(invalid());
    }
    let base = component
        .split(|unit| *unit == b'.' as u16)
        .next()
        .unwrap_or_default();
    // DOS device recognition also trims spaces before the first extension.
    // Keep this alias defense for Windows versions that recognize such names.
    let base = &base[..base
        .iter()
        .rposition(|unit| *unit != b' ' as u16)
        .map_or(0, |index| index + 1)];
    let upper: Vec<u16> = base
        .iter()
        .map(|unit| match unit {
            97..=122 => *unit - 32,
            _ => *unit,
        })
        .collect();
    let reserved = ["CON", "PRN", "AUX", "NUL", "CONIN$", "CONOUT$"]
        .iter()
        .any(|name| upper.iter().copied().eq(name.encode_utf16()));
    let numbered = upper.len() == 4
        && (upper[..3] == [67, 79, 77] || upper[..3] == [76, 80, 84])
        && matches!(upper[3], 49..=57 | 0x00b9 | 0x00b2 | 0x00b3);
    if reserved || numbered {
        return Err(invalid());
    }
    Ok(())
}

fn invalid() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidInput,
        "WebView2 UDF is not an exact ordinary local Win32 path",
    )
}

pub(crate) fn directory_identity(file: &std::fs::File) -> io::Result<(u64, [u8; 16])> {
    let identity = verified_short_path::identity(file)?;
    Ok((identity.VolumeSerialNumber, identity.FileId.Identifier))
}

mod verified_short_path {
    use super::*;
    use std::fs::{File, OpenOptions};
    use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
    use std::os::windows::io::AsRawHandle;
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::Storage::FileSystem::{
        FileIdInfo, GetFileInformationByHandleEx, GetShortPathNameW, FILE_ATTRIBUTE_REPARSE_POINT,
        FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_ID_INFO,
        FILE_LIST_DIRECTORY, FILE_READ_ATTRIBUTES, FILE_SHARE_READ, FILE_SHARE_WRITE,
    };

    const MAX_NATIVE_PATH_UNITS: usize = 32768;

    pub(super) fn shorter(canonical: &Path, ordinary: &Path) -> io::Result<Option<PathBuf>> {
        // Keep the authoritative directory open without delete sharing through
        // lookup and both identity checks. Listing access participates in native
        // sharing checks; attributes-only access would not pin against rename.
        // No filesystem mutation is requested.
        let original = open_directory(canonical)?;
        let expected = identity(&original)?;
        let mut input: Vec<u16> = canonical.as_os_str().encode_wide().collect();
        if input.len() >= MAX_NATIVE_PATH_UNITS || input.contains(&0) {
            return Err(invalid());
        }
        input.push(0);
        // SAFETY: input is bounded, NUL-terminated UTF-16 and remains alive;
        // a null output requests only the bounded allocation size.
        let required = unsafe { GetShortPathNameW(PCWSTR(input.as_ptr()), None) } as usize;
        if required == 0 {
            return unavailable_or_error(io::Error::last_os_error());
        }
        if required > MAX_NATIVE_PATH_UNITS {
            return Err(invalid());
        }
        let mut output = vec![0; required];
        // SAFETY: the immutable input and correctly sized mutable output live
        // through the synchronous native call; no pointer escapes.
        let length =
            unsafe { GetShortPathNameW(PCWSTR(input.as_ptr()), Some(&mut output)) } as usize;
        if length == 0 {
            return unavailable_or_error(io::Error::last_os_error());
        }
        if length >= output.len() || output[length] != 0 || output[..length].contains(&0) {
            return Err(invalid());
        }
        output.truncate(length);
        let prefix = [92, 92, 63, 92];
        if !output.starts_with(&prefix) {
            let mut extended = prefix.to_vec();
            extended.extend(output);
            output = extended;
        }
        // The alias is still an ordinary local Win32 spelling: reject the
        // same device, component and namespace aliases as the long projection.
        let alias = project_spelling(Path::new(&OsString::from_wide(&output)))?;
        if alias.as_os_str().encode_wide().count() >= ordinary.as_os_str().encode_wide().count() {
            return Ok(None);
        }
        verify_binding(canonical, &alias, &original, expected)?;
        Ok(Some(alias))
    }

    fn verify_binding(
        canonical: &Path,
        alias: &Path,
        original: &File,
        expected: FILE_ID_INFO,
    ) -> io::Result<()> {
        let alias_file = open_directory(alias)?;
        if std::fs::canonicalize(alias)? != canonical
            || identity(&alias_file)? != expected
            || identity(original)? != expected
            || std::fs::canonicalize(canonical)? != canonical
        {
            return Err(invalid());
        }
        Ok(())
    }

    fn unavailable_or_error(error: io::Error) -> io::Result<Option<PathBuf>> {
        // Only explicit lack of native API/filesystem support is optional.
        // Access, lookup, identity and malformed-result errors remain failures.
        match error.raw_os_error() {
            Some(1 | 50 | 120) => Ok(None),
            _ => Err(error),
        }
    }

    fn open_directory(path: &Path) -> io::Result<File> {
        let file = OpenOptions::new()
            .access_mode(FILE_LIST_DIRECTORY.0 | FILE_READ_ATTRIBUTES.0)
            .share_mode(FILE_SHARE_READ.0 | FILE_SHARE_WRITE.0)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS.0 | FILE_FLAG_OPEN_REPARSE_POINT.0)
            .open(path)?;
        let metadata = file.metadata()?;
        if !metadata.is_dir() || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0 {
            return Err(invalid());
        }
        Ok(file)
    }

    pub(super) fn identity(file: &File) -> io::Result<FILE_ID_INFO> {
        let mut information = FILE_ID_INFO::default();
        // SAFETY: file owns this live handle, information has the exact native
        // layout and size for FileIdInfo, and the synchronous call cannot retain it.
        unsafe {
            GetFileInformationByHandleEx(
                HANDLE(file.as_raw_handle()),
                FileIdInfo,
                (&mut information as *mut FILE_ID_INFO).cast(),
                std::mem::size_of::<FILE_ID_INFO>() as u32,
            )
        }
        .map_err(io::Error::other)?;
        Ok(information)
    }

    #[test]
    fn unavailable_short_name_support_is_optional_but_security_errors_are_not() {
        for code in [1, 50, 120] {
            assert!(unavailable_or_error(io::Error::from_raw_os_error(code))
                .unwrap()
                .is_none());
        }
        for code in [2, 5, 32, 123] {
            assert_eq!(
                unavailable_or_error(io::Error::from_raw_os_error(code))
                    .unwrap_err()
                    .raw_os_error(),
                Some(code)
            );
        }
    }

    #[test]
    fn refuses_alias_with_different_canonical_path_or_full_directory_identity() {
        let root = tempfile::tempdir().unwrap();
        let original_path = root.path().join("original-user-data-directory");
        let unrelated_path = root.path().join("unrelated-directory");
        std::fs::create_dir(&original_path).unwrap();
        std::fs::create_dir(&unrelated_path).unwrap();
        let canonical = original_path.canonicalize().unwrap();
        let original = open_directory(&canonical).unwrap();
        let expected = identity(&original).unwrap();
        assert!(verify_binding(&canonical, &unrelated_path, &original, expected).is_err());

        let unrelated = open_directory(&unrelated_path).unwrap();
        let unrelated_identity = identity(&unrelated).unwrap();
        assert_ne!(expected, unrelated_identity);
        assert!(verify_binding(
            &canonical,
            &project_spelling(&canonical).unwrap(),
            &original,
            unrelated_identity
        )
        .is_err());
    }

    #[test]
    fn shorter_alias_preserves_exact_directory_when_native_names_exist() {
        let root = tempfile::tempdir().unwrap();
        let original = root.path().join("a-long-owned-webview-user-data-directory");
        std::fs::create_dir(&original).unwrap();
        let canonical = original.canonicalize().unwrap();
        let ordinary = project_spelling(&canonical).unwrap();
        if let Some(alias) = shorter(&canonical, &ordinary).unwrap() {
            assert!(
                alias.as_os_str().encode_wide().count()
                    < ordinary.as_os_str().encode_wide().count()
            );
            assert_eq!(alias.canonicalize().unwrap(), canonical);
            assert_eq!(
                identity(&open_directory(&alias).unwrap()).unwrap(),
                identity(&open_directory(&canonical).unwrap()).unwrap()
            );
        }
    }

    #[test]
    fn rejects_file_as_user_data_directory_and_pins_directory_identity() {
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("regular-file");
        std::fs::write(&file, b"not a directory").unwrap();
        assert!(open_directory(&file).is_err());

        let directory = root.path().join("owned-directory");
        std::fs::create_dir(&directory).unwrap();
        let held = open_directory(&directory).unwrap();
        let before = identity(&held).unwrap();
        assert!(std::fs::rename(&directory, root.path().join("replacement")).is_err());
        assert_eq!(identity(&held).unwrap(), before);
        assert_eq!(
            identity(&open_directory(&directory).unwrap()).unwrap(),
            before
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn download_projection_preserves_parent_and_rejects_native_aliases() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("zażółć 日本語");
        std::fs::create_dir(&directory).unwrap();
        let canonical = directory.canonicalize().unwrap();
        let projected = webview2_download_path(&canonical.join("payload")).unwrap();
        assert_eq!(
            projected.parent().unwrap().canonicalize().unwrap(),
            canonical
        );
        assert!(!projected.exists());
        assert!(webview2_download_path(&canonical.join("payload.")).is_err());
        assert!(webview2_download_path(&canonical.join("NUL.txt")).is_err());
        assert!(webview2_download_path(&canonical.join("missing").join("payload")).is_err());
    }

    #[test]
    fn projects_existing_canonical_directory_with_exact_roundtrip() {
        let root = tempfile::tempdir().unwrap();
        let canonical = root.path().canonicalize().unwrap();
        let ordinary = webview2_user_data_path(&canonical).unwrap();
        assert!(!ordinary
            .as_os_str()
            .encode_wide()
            .collect::<Vec<_>>()
            .starts_with(&[92, 92, 63, 92]));
        assert_eq!(ordinary.canonicalize().unwrap(), canonical);
    }

    #[test]
    fn projects_utf16_without_replacement_or_unicode_normalization() {
        let mut original: Vec<u16> = r"\\?\C:\owned\".encode_utf16().collect();
        original.extend([0xd800, 0x0065, 0x0301]);
        let path = PathBuf::from(OsString::from_wide(&original));
        assert_eq!(
            project_spelling(&path)
                .unwrap()
                .as_os_str()
                .encode_wide()
                .collect::<Vec<_>>(),
            original[4..]
        );
    }

    #[test]
    fn refuses_win32_aliases_devices_unc_and_nul() {
        for path in [
            r"C:\owned",
            r"\\?\UNC\server\share\owned",
            r"\\.\C:\owned",
            r"\\?\C:\owned.",
            r"\\?\C:\owned ",
            r"\\?\C:\CON",
            r"\\?\C:\con.db",
            r"\\?\C:\CON .txt",
            r"\\?\C:\COM1 .data",
            r"\\?\C:\LPT1",
            r"\\?\C:\COM¹",
            r"\\?\C:\a:b",
            r"\\?\C:\a\\b",
            r"\\?\C:\..\owned",
        ] {
            assert!(project_spelling(Path::new(path)).is_err());
        }
        let mut nul: Vec<u16> = r"\\?\C:\owned".encode_utf16().collect();
        nul.push(0);
        assert!(project_spelling(Path::new(&OsString::from_wide(&nul))).is_err());
    }

    #[test]
    fn preserves_existing_long_directory_with_exact_roundtrip() {
        let root = tempfile::tempdir().unwrap();
        let mut ordinary = root.path().to_path_buf();
        for index in 0..5 {
            ordinary.push(format!("segment-{index}-{}", "x".repeat(48)));
        }
        std::fs::create_dir_all(&ordinary).unwrap();
        let canonical = ordinary.canonicalize().unwrap();
        assert!(canonical.as_os_str().encode_wide().count() > 260);
        let projected = webview2_user_data_path(&canonical).unwrap();
        assert_eq!(projected.canonicalize().unwrap(), canonical);
    }
}
