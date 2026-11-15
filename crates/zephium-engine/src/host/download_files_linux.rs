use super::*;
use std::io::Read;
use std::os::unix::fs::PermissionsExt;

fn modes_enforced(file: &File) -> Result<bool, DownloadError> {
    let mut status = std::mem::MaybeUninit::<libc::statfs>::uninit();
    if unsafe { libc::fstatfs(file.as_raw_fd(), status.as_mut_ptr()) } != 0 {
        return Err(DownloadError::Protection);
    }
    let kind = unsafe { status.assume_init() }.f_type;
    // Unknown and userspace filesystems receive the conservative open policy.
    Ok(matches!(
        kind,
        0xef53 | 0x58465342 | 0x9123683e | 0x01021994 | 0x794c7630
    ))
}

pub(super) fn protect(file: &File, source: &str) -> Result<(), DownloadError> {
    let mode = file.metadata().map_err(map_io)?.mode() & 0o666;
    let changed = file.set_permissions(fs::Permissions::from_mode(mode));
    if modes_enforced(file)? {
        changed.map_err(|_| DownloadError::Protection)?;
    }
    verify(file)?;
    if let Some(source) = provenance(source) {
        unsafe {
            libc::fsetxattr(
                file.as_raw_fd(),
                c"user.xdg.origin.url".as_ptr(),
                source.as_ptr().cast(),
                source.len(),
                0,
            );
        }
    }
    Ok(())
}

pub(super) fn referrer(file: &File, source: &str) {
    if let Some(source) = provenance(source) {
        unsafe {
            libc::fsetxattr(
                file.as_raw_fd(),
                c"user.xdg.referrer.url".as_ptr(),
                source.as_ptr().cast(),
                source.len(),
                0,
            );
        }
    }
}

fn provenance(source: &str) -> Option<String> {
    if source.len() > 8192 {
        return None;
    }
    let mut url = url::Url::parse(source).ok()?;
    if !matches!(url.scheme(), "http" | "https") {
        return None;
    }
    url.set_username("").ok()?;
    url.set_password(None).ok()?;
    url.set_fragment(None);
    // Queries may carry bearer credentials; provenance needs no query string.
    url.set_query(None);
    Some(url.into())
}

pub(super) fn verify(file: &File) -> Result<(), DownloadError> {
    if modes_enforced(file)? && file.metadata().map_err(map_io)?.mode() & 0o111 != 0 {
        Err(DownloadError::Protection)
    } else {
        Ok(())
    }
}

pub(crate) fn must_reveal(path: &Path) -> Result<bool, DownloadError> {
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)
        .map_err(map_io)?;
    if !file.metadata().map_err(map_io)?.is_file() {
        return Err(DownloadError::ChangedFile);
    }
    if !modes_enforced(&file)? {
        return Ok(true);
    }
    let extension = path
        .extension()
        .and_then(|v| v.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if matches!(
        extension.as_str(),
        "desktop"
            | "appimage"
            | "run"
            | "jar"
            | "deb"
            | "rpm"
            | "flatpakref"
            | "flatpak"
            | "flatpakrepo"
            | "sh"
            | "bash"
            | "zsh"
            | "fish"
            | "py"
            | "pyw"
            | "pl"
            | "pm"
            | "rb"
            | "js"
            | "mjs"
            | "cjs"
            | "exe"
            | "msi"
            | "com"
            | "bat"
            | "cmd"
            | "ps1"
            | "vbs"
            | "bin"
    ) {
        return Ok(true);
    }
    let mut bytes = [0; 8192];
    let length = file.read(&mut bytes).map_err(map_io)?;
    let bytes = &bytes[..length];
    if bytes.starts_with(b"#!")
        || bytes.starts_with(b"\x7fELF")
        || bytes.starts_with(b"MZ")
        || bytes
            .windows(b"[Desktop Entry]".len())
            .any(|s| s == b"[Desktop Entry]")
    {
        return Ok(true);
    }
    let (content, uncertain) = gtk::gio::content_type_guess(Some(path), bytes);
    Ok(uncertain
        || [
            "application/x-executable",
            "application/x-sharedlib",
            "application/x-shellscript",
            "text/x-python",
            "application/x-perl",
            "application/x-desktop",
            "application/java-archive",
        ]
        .iter()
        .any(|kind| gtk::gio::content_type_is_a(&content, kind)))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn protection_clears_all_execute_bits_and_sniffs_disguised_executables() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("image.png");
        fs::write(&path, b"#!/usr/bin/python3\nprint('fixture')").unwrap();
        let file = File::open(&path).unwrap();
        file.set_permissions(fs::Permissions::from_mode(0o777))
            .unwrap();
        protect(&file, "").unwrap();
        assert_eq!(file.metadata().unwrap().mode() & 0o111, 0);
        assert!(must_reveal(&path).unwrap());
        fs::write(&path, b"\x7fELFfixture").unwrap();
        assert!(must_reveal(&path).unwrap());
        fs::write(&path, b"[Desktop Entry]\nExec=fixture").unwrap();
        assert!(must_reveal(&path).unwrap());
    }
    #[test]
    fn provenance_never_contains_userinfo_query_or_fragment() {
        assert_eq!(
            provenance("https://user:password@example.org/file?token=secret#secret"),
            Some("https://example.org/file".into())
        );
        assert_eq!(provenance(""), None);
        assert_eq!(provenance("data:text/plain,secret"), None);
    }
}
