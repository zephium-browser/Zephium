// Copyright 2026 Zephium contributors
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

//! Atomic macOS bundle exchange. The public app path is never vacated.
use std::ffi::CString;
use std::fs::{self, File, OpenOptions};
use std::io;
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::Path;
use tempfile::TempDir;

#[derive(Clone, Copy, PartialEq, Eq)]
struct Identity {
    device: u64,
    inode: u64,
}

struct Slot {
    parent: File,
    name: CString,
    _directory: File,
    identity: Identity,
}
impl Slot {
    fn open(path: &Path) -> io::Result<Self> {
        let parent_path = fs::canonicalize(path.parent().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "app bundle has no parent")
        })?)?;
        let parent = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(parent_path)?;
        let name = CString::new(
            path.file_name()
                .ok_or_else(|| {
                    io::Error::new(io::ErrorKind::InvalidInput, "app bundle has no name")
                })?
                .as_encoded_bytes(),
        )
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "invalid app bundle name"))?;
        // SAFETY: the held parent descriptor and NUL-terminated name stay live.
        let fd = unsafe {
            libc::openat(
                parent.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        let directory = unsafe { File::from_raw_fd(fd) };
        let metadata = directory.metadata()?;
        Ok(Self {
            parent,
            name,
            _directory: directory,
            identity: Identity {
                device: metadata.dev(),
                inode: metadata.ino(),
            },
        })
    }
    fn current(&self) -> io::Result<Identity> {
        let mut metadata = std::mem::MaybeUninit::<libc::stat>::uninit();
        // SAFETY: the output is correctly sized; it is read only on success.
        if unsafe {
            libc::fstatat(
                self.parent.as_raw_fd(),
                self.name.as_ptr(),
                metadata.as_mut_ptr(),
                libc::AT_SYMLINK_NOFOLLOW,
            )
        } != 0
        {
            return Err(io::Error::last_os_error());
        }
        let metadata = unsafe { metadata.assume_init() };
        Ok(Identity {
            device: metadata.st_dev as u64,
            inode: metadata.st_ino,
        })
    }
    fn matches(&self, identity: Identity) -> bool {
        self.current().is_ok_and(|actual| actual == identity)
    }
}

fn exchange(current: &Slot, staged: &Slot) -> io::Result<()> {
    // SAFETY: held directories anchor both leaf names. RENAME_SWAP exchanges
    // existing entries in one filesystem operation instead of vacating the app.
    if unsafe {
        libc::renameatx_np(
            current.parent.as_raw_fd(),
            current.name.as_ptr(),
            staged.parent.as_raw_fd(),
            staged.name.as_ptr(),
            libc::RENAME_SWAP,
        )
    } == 0
    {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

pub(crate) fn replace_bundle(current: &Path, staged: &Path, owner: TempDir) -> io::Result<()> {
    replace_with(current, staged, owner, exchange)
}

fn retain(owner: TempDir, reason: impl std::fmt::Display) -> io::Error {
    let path = owner.keep();
    io::Error::other(format!(
        "{reason}; app staging retained for recovery at {}",
        path.display()
    ))
}

fn replace_with(
    current: &Path,
    staged: &Path,
    owner: TempDir,
    mut swap: impl FnMut(&Slot, &Slot) -> io::Result<()>,
) -> io::Result<()> {
    if !staged.starts_with(owner.path()) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "app staging is not owned",
        ));
    }
    crate::macos_bundle::validate_bundle(current)?;
    crate::macos_bundle::validate_bundle(staged)?;
    crate::macos_bundle::sync_bundle(staged)?;
    let current = Slot::open(current)?;
    let staged = Slot::open(staged)?;
    if current.identity.device != staged.identity.device || current.identity == staged.identity {
        return Err(io::Error::new(io::ErrorKind::Unsupported, "atomic app update requires distinct bundles on the same filesystem; install the update manually"));
    }
    if !current.matches(current.identity) || !staged.matches(staged.identity) {
        return Err(io::Error::other("app bundle changed before installation"));
    }
    if let Err(error) = swap(&current, &staged) {
        if !staged.matches(staged.identity) {
            return Err(retain(owner, error));
        }
        return Err(io::Error::new(
            error.kind(),
            format!("atomic app update failed ({error}); install the update manually"),
        ));
    }
    let valid = current.matches(staged.identity) && staged.matches(current.identity);
    let synchronized = if valid {
        current
            .parent
            .sync_all()
            .and_then(|_| staged.parent.sync_all())
    } else {
        Err(io::Error::other("app bundle changed during installation"))
    };
    if let Err(error) = synchronized {
        // Restore in one atomic exchange only while the new bundle still owns
        // the public slot. Never erase an observed substitute or the old app.
        if !current.matches(staged.identity)
            || swap(&current, &staged).is_err()
            || !staged.matches(staged.identity)
        {
            return Err(retain(owner, error));
        }
        let _ = current.parent.sync_all();
        let _ = staged.parent.sync_all();
        return Err(error);
    }
    // The original remains in owned staging until the exchanged names have
    // synchronized. A crash before cleanup leaves both bundles, never no app.
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn fixture() -> (TempDir, PathBuf, PathBuf, TempDir) {
        let root = tempfile::tempdir().unwrap();
        let current = root.path().join("Current.app");
        fs::create_dir(&current).unwrap();
        crate::macos_bundle::tests::write_valid_bundle(&current);
        fs::write(current.join("marker"), b"original").unwrap();
        let owner = tempfile::tempdir_in(root.path()).unwrap();
        let staged = owner.path().join("Staged.app");
        fs::create_dir(&staged).unwrap();
        crate::macos_bundle::tests::write_valid_bundle(&staged);
        fs::write(staged.join("marker"), b"update").unwrap();
        (root, current, staged, owner)
    }

    #[test]
    fn atomic_exchange_keeps_old_and_new_bundles_at_every_publication_transition() {
        let (_root, current, staged, owner) = fixture();
        let owner_path = owner.path().to_owned();
        replace_with(&current, &staged, owner, |old, new| {
            assert_eq!(fs::read(current.join("marker")).unwrap(), b"original");
            assert_eq!(fs::read(staged.join("marker")).unwrap(), b"update");
            exchange(old, new)?;
            assert_eq!(fs::read(current.join("marker")).unwrap(), b"update");
            assert_eq!(fs::read(staged.join("marker")).unwrap(), b"original");
            Ok(())
        })
        .unwrap();
        assert_eq!(fs::read(current.join("marker")).unwrap(), b"update");
        assert!(!owner_path.exists());
    }

    #[test]
    fn failed_exchange_leaves_original_at_the_app_path() {
        let (_root, current, staged, owner) = fixture();
        let mut attempts = 0;
        assert!(replace_with(&current, &staged, owner, |_, _| {
            attempts += 1;
            assert_eq!(fs::read(current.join("marker")).unwrap(), b"original");
            Err(io::Error::from_raw_os_error(libc::ENOSPC))
        })
        .is_err());
        assert_eq!(attempts, 1);
        assert_eq!(fs::read(current.join("marker")).unwrap(), b"original");
    }

    #[test]
    fn permission_or_unsupported_exchange_refuses_without_vacating_the_app() {
        for code in [libc::EACCES, libc::ENOTSUP, libc::EXDEV] {
            let (_root, current, staged, owner) = fixture();
            assert!(replace_with(&current, &staged, owner, |_, _| Err(
                io::Error::from_raw_os_error(code)
            ))
            .is_err());
            assert_eq!(fs::read(current.join("marker")).unwrap(), b"original");
        }
    }

    #[test]
    fn substituted_destination_is_restored_without_erasing_it() {
        let (root, current, staged, owner) = fixture();
        let original = root.path().join("Moved.app");
        let mut swaps = 0;
        let result = replace_with(&current, &staged, owner, |old, new| {
            swaps += 1;
            if swaps == 1 {
                fs::rename(&current, &original).unwrap();
                fs::create_dir(&current).unwrap();
                fs::write(current.join("other"), b"unrelated").unwrap();
            }
            exchange(old, new)
        });
        assert!(result.is_err());
        assert_eq!(swaps, 2);
        assert_eq!(fs::read(current.join("other")).unwrap(), b"unrelated");
        assert_eq!(fs::read(original.join("marker")).unwrap(), b"original");
    }

    #[test]
    fn failed_reverse_exchange_retains_displaced_data_and_keeps_app_present() {
        let (root, current, staged, owner) = fixture();
        let owner_path = owner.path().to_owned();
        let original = root.path().join("Moved.app");
        let mut swaps = 0;
        let result = replace_with(&current, &staged, owner, |old, new| {
            swaps += 1;
            if swaps == 1 {
                fs::rename(&current, &original).unwrap();
                fs::create_dir(&current).unwrap();
                fs::write(current.join("other"), b"unrelated").unwrap();
                exchange(old, new)
            } else {
                Err(io::Error::from_raw_os_error(libc::EACCES))
            }
        });
        assert!(result.is_err());
        assert_eq!(fs::read(current.join("marker")).unwrap(), b"update");
        assert_eq!(fs::read(staged.join("other")).unwrap(), b"unrelated");
        assert_eq!(fs::read(original.join("marker")).unwrap(), b"original");
        assert!(owner_path.exists());
    }
}
