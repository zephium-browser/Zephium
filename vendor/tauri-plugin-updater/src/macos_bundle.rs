// Copyright 2026 Zephium contributors
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

//! Bounded extraction and structural validation before replacing a running app.
use std::ffi::{CStr, CString};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Cursor, Read};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Component, Path, PathBuf};

const MAX_INFO_BYTES: u64 = 1024 * 1024;
const MAX_EXTRACTED_BYTES: u64 = 512 * 1024 * 1024;
const MAX_ENTRIES: usize = 20_000;

fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

fn app_path(path: &Path) -> io::Result<(std::ffi::OsString, PathBuf)> {
    if path.as_os_str().len() > 4096 {
        return Err(invalid("app archive path exceeds limit"));
    }
    let mut parts = path.components().filter(|part| *part != Component::CurDir);
    let Some(Component::Normal(root)) = parts.next() else {
        return Err(invalid("invalid app archive root"));
    };
    if !root
        .to_str()
        .is_some_and(|name| name.len() > 4 && name.ends_with(".app"))
    {
        return Err(invalid("archive must contain a single app bundle"));
    }
    let mut relative = PathBuf::new();
    for part in parts {
        let Component::Normal(name) = part else {
            return Err(invalid("unsafe app archive path"));
        };
        relative.push(name);
    }
    Ok((root.to_owned(), relative))
}

pub(crate) fn extract_bundle(bytes: &[u8], directory: &Path) -> io::Result<PathBuf> {
    // Include headers/padding in the read bound, including GNU/PAX metadata
    // that tar may consume internally before yielding an ordinary entry.
    let decoder = flate2::read::GzDecoder::new(Cursor::new(bytes))
        .take(MAX_EXTRACTED_BYTES + MAX_ENTRIES as u64 * 1024);
    let mut archive = tar::Archive::new(decoder);
    let mut root = None;
    let mut size = 0_u64;
    for (index, entry) in archive.entries()?.enumerate() {
        if index >= MAX_ENTRIES {
            return Err(invalid("app archive has too many entries"));
        }
        let mut entry = entry?;
        let (name, relative) = app_path(&entry.path()?)?;
        if root.as_ref().is_some_and(|root| root != &name) {
            return Err(invalid("archive contains multiple app bundles"));
        }
        root = Some(name.clone());
        size = size
            .checked_add(entry.header().size()?)
            .filter(|size| *size <= MAX_EXTRACTED_BYTES)
            .ok_or_else(|| invalid("expanded app archive exceeds limit"))?;
        let kind = entry.header().entry_type();
        if !(kind.is_file() || kind.is_dir() || kind.is_symlink() || kind.is_hard_link())
            || (relative.as_os_str().is_empty() && !kind.is_dir())
        {
            return Err(invalid("unsupported app archive entry"));
        }
        if kind.is_symlink() || kind.is_hard_link() {
            let link = entry
                .link_name()?
                .ok_or_else(|| invalid("missing app archive link target"))?;
            if kind.is_hard_link() {
                if app_path(&link)?.0 != name {
                    return Err(invalid("hard link leaves app bundle"));
                }
            } else {
                let mut depth = relative
                    .parent()
                    .map_or(0, |path| path.components().count());
                for part in link.components() {
                    match part {
                        Component::Normal(_) => depth += 1,
                        Component::CurDir => {}
                        Component::ParentDir if depth > 0 => depth -= 1,
                        _ => return Err(invalid("symbolic link leaves app bundle")),
                    }
                }
            }
        }
        if !entry.unpack_in(directory)? {
            return Err(invalid("unsafe app archive extraction"));
        }
    }
    let bundle = directory.join(root.ok_or_else(|| invalid("app archive is empty"))?);
    validate_bundle(&bundle)?;
    Ok(bundle)
}

pub(crate) fn validate_bundle(bundle: &Path) -> io::Result<()> {
    validate_bundle_version(bundle, None)
}

pub(crate) fn validate_bundle_version(bundle: &Path, expected: Option<&str>) -> io::Result<()> {
    for directory in [
        bundle.to_owned(),
        bundle.join("Contents"),
        bundle.join("Contents/MacOS"),
    ] {
        if !fs::symlink_metadata(directory)?.is_dir() {
            return Err(invalid("app bundle directory is missing or unsafe"));
        }
    }
    let info_path = bundle.join("Contents/Info.plist");
    let info_metadata = fs::symlink_metadata(&info_path)?;
    if !info_metadata.is_file() || info_metadata.len() == 0 || info_metadata.len() > MAX_INFO_BYTES
    {
        return Err(invalid("app bundle Info.plist is missing or exceeds limit"));
    }
    let mut bytes = Vec::new();
    OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(info_path)?
        .take(MAX_INFO_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_INFO_BYTES {
        return Err(invalid("app bundle Info.plist exceeds limit"));
    }
    let plist = plist::Value::from_reader(Cursor::new(bytes))
        .map_err(|_| invalid("invalid app bundle Info.plist"))?;
    let info = plist
        .as_dictionary()
        .ok_or_else(|| invalid("invalid app bundle Info.plist"))?;
    if info
        .get("CFBundlePackageType")
        .and_then(plist::Value::as_string)
        != Some("APPL")
    {
        return Err(invalid("update is not an application bundle"));
    }
    if let Some(expected) = expected {
        if info
            .get("CFBundleShortVersionString")
            .and_then(plist::Value::as_string)
            != Some(expected)
        {
            return Err(invalid(
                "app bundle version does not match the announced update",
            ));
        }
    }
    let executable = info
        .get("CFBundleExecutable")
        .and_then(plist::Value::as_string)
        .filter(|name| {
            !name.is_empty()
                && name.len() <= 255
                && !name.contains(['/', '\\', '\0'])
                && *name != "."
                && *name != ".."
        })
        .ok_or_else(|| invalid("app bundle executable name is unsafe or missing"))?;
    let metadata = fs::symlink_metadata(bundle.join("Contents/MacOS").join(executable))?;
    if !metadata.is_file() || metadata.len() == 0 || metadata.permissions().mode() & 0o111 == 0 {
        return Err(invalid(
            "app bundle executable is missing, empty or not executable",
        ));
    }
    Ok(())
}

struct DirectoryStream(*mut libc::DIR);
impl Drop for DirectoryStream {
    fn drop(&mut self) {
        // SAFETY: fdopendir owns the duplicated descriptor until closedir.
        unsafe { libc::closedir(self.0) };
    }
}

fn names(directory: &File, count: &mut usize) -> io::Result<Vec<CString>> {
    let duplicate = unsafe { libc::dup(directory.as_raw_fd()) };
    if duplicate < 0 {
        return Err(io::Error::last_os_error());
    }
    let raw = unsafe { libc::fdopendir(duplicate) };
    if raw.is_null() {
        let error = io::Error::last_os_error();
        unsafe { libc::close(duplicate) };
        return Err(error);
    }
    let stream = DirectoryStream(raw);
    let mut names = Vec::new();
    loop {
        // SAFETY: this live DIR stream is used only here. readdir's returned
        // name is terminated and is copied before the next call changes it.
        unsafe { *libc::__error() = 0 };
        let entry = unsafe { libc::readdir(stream.0) };
        if entry.is_null() {
            let error = io::Error::last_os_error();
            if error.raw_os_error() != Some(0) {
                return Err(error);
            }
            break;
        }
        let name = unsafe { CStr::from_ptr((*entry).d_name.as_ptr()) };
        if matches!(name.to_bytes(), b"." | b"..") {
            continue;
        }
        *count += 1;
        if *count > MAX_ENTRIES {
            return Err(invalid("app sync has too many entries"));
        }
        names.push(name.to_owned());
    }
    Ok(names)
}

pub(crate) fn sync_bundle(bundle: &Path) -> io::Result<()> {
    let root = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(bundle)?;
    let mut work = vec![(root, false)];
    let mut count = 0;
    let mut barrier = None;
    while let Some((directory, done)) = work.pop() {
        if done {
            sync_inode(&directory)?;
            continue;
        }
        let entries = names(&directory, &mut count)?;
        let mut children = Vec::new();
        for name in entries {
            let mut metadata = std::mem::MaybeUninit::<libc::stat>::uninit();
            // SAFETY: held parent, terminated leaf and correctly sized output.
            if unsafe {
                libc::fstatat(
                    directory.as_raw_fd(),
                    name.as_ptr(),
                    metadata.as_mut_ptr(),
                    libc::AT_SYMLINK_NOFOLLOW,
                )
            } != 0
            {
                return Err(io::Error::last_os_error());
            }
            let kind = unsafe { metadata.assume_init() }.st_mode & libc::S_IFMT;
            if kind == libc::S_IFLNK {
                continue;
            }
            if kind != libc::S_IFDIR && kind != libc::S_IFREG {
                return Err(invalid("unsafe app sync entry"));
            }
            let flags = libc::O_RDONLY
                | libc::O_NOFOLLOW
                | libc::O_CLOEXEC
                | libc::O_NONBLOCK
                | if kind == libc::S_IFDIR {
                    libc::O_DIRECTORY
                } else {
                    0
                };
            let fd = unsafe { libc::openat(directory.as_raw_fd(), name.as_ptr(), flags) };
            if fd < 0 {
                return Err(io::Error::last_os_error());
            }
            let file = unsafe { File::from_raw_fd(fd) };
            if kind == libc::S_IFDIR {
                children.push((file, false));
            } else {
                if !file.metadata()?.is_file() {
                    return Err(invalid("app file changed while syncing"));
                }
                sync_inode(&file)?;
                if barrier.is_none() {
                    barrier = Some(file);
                }
            }
        }
        // Synchronize directory metadata after its descendants. Every child
        // opened relative to its held parent refuses links, so none are followed.
        work.push((directory, true));
        work.extend(children);
    }
    let barrier = barrier.ok_or_else(|| invalid("app bundle has no files to synchronize"))?;
    // One device-cache barrier after all file and directory fsyncs; never a
    // full drive flush for each asset. Failure prevents the public exchange.
    if unsafe { libc::fcntl(barrier.as_raw_fd(), libc::F_FULLFSYNC) } == -1 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn sync_inode(file: &File) -> io::Result<()> {
    // Apple std::File::sync_all performs a full storage barrier. Flush each
    // inode with fsync here, then use one F_FULLFSYNC after the whole tree.
    if unsafe { libc::fsync(file.as_raw_fd()) } == -1 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn write_valid_bundle(bundle: &Path) {
        fs::create_dir_all(bundle.join("Contents/MacOS")).unwrap();
        let mut info = plist::Dictionary::new();
        info.insert("CFBundlePackageType".into(), "APPL".into());
        info.insert("CFBundleExecutable".into(), "Zephium".into());
        info.insert("CFBundleShortVersionString".into(), "1.2.3".into());
        plist::Value::Dictionary(info)
            .to_file_xml(bundle.join("Contents/Info.plist"))
            .unwrap();
        let executable = bundle.join("Contents/MacOS/Zephium");
        fs::write(&executable, b"generated executable fixture").unwrap();
        fs::set_permissions(executable, fs::Permissions::from_mode(0o755)).unwrap();
    }

    fn archive(bundle: &Path) -> Vec<u8> {
        let encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        let mut tar = tar::Builder::new(encoder);
        tar.append_dir_all("Zephium.app", bundle).unwrap();
        tar.into_inner().unwrap().finish().unwrap()
    }

    #[test]
    fn version_must_match_exactly_in_xml_and_binary_plists() {
        for binary in [false, true] {
            let bundle = tempfile::tempdir().unwrap();
            write_valid_bundle(bundle.path());
            let path = bundle.path().join("Contents/Info.plist");
            let mut info = plist::Value::from_file(&path).unwrap();
            if binary {
                info.to_file_binary(&path).unwrap();
            }
            assert!(validate_bundle_version(bundle.path(), Some("1.2.3")).is_ok());
            for wrong in ["1.2.4", "1.2.3-beta.1", "v1.2.3"] {
                assert!(validate_bundle_version(bundle.path(), Some(wrong)).is_err());
            }
            info.as_dictionary_mut()
                .unwrap()
                .remove("CFBundleShortVersionString");
            info.to_file_xml(&path).unwrap();
            assert!(validate_bundle_version(bundle.path(), Some("1.2.3")).is_err());
        }
    }

    #[test]
    fn valid_archive_is_extracted_as_one_validated_bundle() {
        let root = tempfile::tempdir().unwrap();
        let staged = tempfile::tempdir().unwrap();
        write_valid_bundle(root.path());
        let bundle = extract_bundle(&archive(root.path()), staged.path()).unwrap();
        assert!(bundle.ends_with("Zephium.app"));
        validate_bundle(&bundle).unwrap();
    }

    #[test]
    fn invalid_bundles_leave_the_current_application_untouched() {
        let root = tempfile::tempdir().unwrap();
        let current = root.path().join("Current.app");
        fs::create_dir(&current).unwrap();
        write_valid_bundle(&current);
        fs::write(current.join("original"), b"keep").unwrap();
        for variant in [
            "empty",
            "missing",
            "malformed",
            "unsafe",
            "zero",
            "mode",
            "symlink",
        ] {
            let staged = tempfile::tempdir().unwrap();
            write_valid_bundle(staged.path());
            let binary = staged.path().join("Contents/MacOS/Zephium");
            match variant {
                "empty" => fs::remove_dir_all(staged.path().join("Contents")).unwrap(),
                "missing" => fs::remove_file(&binary).unwrap(),
                "malformed" => {
                    fs::write(staged.path().join("Contents/Info.plist"), b"not a plist").unwrap()
                }
                "unsafe" => {
                    let mut info = plist::Dictionary::new();
                    info.insert("CFBundlePackageType".into(), "APPL".into());
                    info.insert("CFBundleExecutable".into(), "../../outside".into());
                    plist::Value::Dictionary(info)
                        .to_file_xml(staged.path().join("Contents/Info.plist"))
                        .unwrap();
                }
                "zero" => fs::write(&binary, b"").unwrap(),
                "mode" => fs::set_permissions(&binary, fs::Permissions::from_mode(0o644)).unwrap(),
                "symlink" => {
                    fs::remove_file(&binary).unwrap();
                    std::os::unix::fs::symlink(current.join("original"), &binary).unwrap();
                }
                _ => unreachable!(),
            }
            let backup = tempfile::tempdir_in(root.path()).unwrap();
            assert!(
                crate::macos_install::replace_bundle(&current, staged.path(), backup).is_err(),
                "{variant}"
            );
            assert_eq!(fs::read(current.join("original")).unwrap(), b"keep");
        }
    }

    #[test]
    fn empty_or_wrong_layout_archives_are_rejected() {
        let empty = tempfile::tempdir().unwrap();
        let output = tempfile::tempdir().unwrap();
        assert!(extract_bundle(&archive(empty.path()), output.path()).is_err());
        let encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        let mut tar = tar::Builder::new(encoder);
        let mut header = tar::Header::new_gnu();
        header.set_size(3);
        header.set_mode(0o644);
        header.set_cksum();
        tar.append_data(&mut header, "wrong.txt", &b"bad"[..])
            .unwrap();
        assert!(
            extract_bundle(&tar.into_inner().unwrap().finish().unwrap(), output.path()).is_err()
        );
    }

    #[test]
    fn binary_plist_is_validated_with_the_same_executable_contract() {
        let bundle = tempfile::tempdir().unwrap();
        write_valid_bundle(bundle.path());
        let path = bundle.path().join("Contents/Info.plist");
        let value = plist::Value::from_file(&path).unwrap();
        value.to_file_binary(&path).unwrap();
        validate_bundle(bundle.path()).unwrap();
        fs::write(&path, vec![0; MAX_INFO_BYTES as usize + 1]).unwrap();
        assert!(validate_bundle(bundle.path()).is_err());
    }

    #[test]
    fn archive_links_cannot_escape_the_bundle() {
        let output = tempfile::tempdir().unwrap();
        let encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        let mut tar = tar::Builder::new(encoder);
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(tar::EntryType::Symlink);
        header.set_mode(0o777);
        header.set_size(0);
        header.set_cksum();
        tar.append_link(&mut header, "Zephium.app/outside", "../outside")
            .unwrap();
        let bytes = tar.into_inner().unwrap().finish().unwrap();
        assert!(extract_bundle(&bytes, output.path()).is_err());
        assert!(!output.path().join("Zephium.app/outside").exists());
    }

    #[test]
    fn bundle_content_barrier_uses_actual_filesystem_and_skips_symlink_targets() {
        let bundle = tempfile::tempdir().unwrap();
        write_valid_bundle(bundle.path());
        // A nonexistent out-of-tree target would make a following traversal
        // fail; synchronization must ignore the link itself instead.
        std::os::unix::fs::symlink(
            "/nonexistent/zephium-sync-fixture",
            bundle.path().join("outside"),
        )
        .unwrap();
        sync_bundle(bundle.path()).unwrap();
    }
}
