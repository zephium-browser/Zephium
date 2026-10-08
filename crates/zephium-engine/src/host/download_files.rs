//! Blocking destination work. Invoked only by bounded download worker jobs.
//! Native transfers write inside a private directory on the destination volume;
//! publication never overwrites a file and preserves the protected inode.

use std::ffi::CString;
use std::fs::{self, File, OpenOptions};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use zephium_core::downloads::{
    collision_filename as collision_name, valid_filename, DownloadError, FileIdentity,
};
use zephium_core::ids::DownloadId;

pub(super) struct Destination {
    pub path: PathBuf,
    pub staging_path: PathBuf,
    pub staging_identity: FileIdentity,
    parent: File,
    staging: File,
    parent_path: PathBuf,
    stage_name: CString,
}

impl Destination {
    pub(super) fn prepare(
        id: DownloadId,
        requested: PathBuf,
        expected_directory: Option<String>,
    ) -> Result<Self, DownloadError> {
        if !requested.is_absolute() || requested.as_os_str().len() > 4096 {
            return Err(DownloadError::Destination);
        }
        let filename = requested
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or(DownloadError::Destination)?;
        if !valid_filename(filename) {
            return Err(DownloadError::Destination);
        }
        let parent_path = fs::canonicalize(requested.parent().ok_or(DownloadError::Destination)?)
            .map_err(map_io)?;
        let parent = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(&parent_path)
            .map_err(map_io)?;
        if let Some(expected) = expected_directory {
            if directory_identity(&parent.metadata().map_err(map_io)?) != expected {
                return Err(DownloadError::Destination);
            }
        }
        let stage_name = CString::new(format!(".zephium-download-{id}"))
            .map_err(|_| DownloadError::Destination)?;
        let staging_path = parent_path.join(
            stage_name
                .to_str()
                .map_err(|_| DownloadError::Destination)?,
        );
        fs::DirBuilder::new()
            .mode(0o700)
            .create(&staging_path)
            .map_err(map_io)?;
        let staging = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(&staging_path)
            .map_err(map_io)?;
        let staging_identity = identity(&staging.metadata().map_err(map_io)?);
        let value = Self {
            path: parent_path.join(filename),
            staging_path,
            staging_identity,
            parent,
            staging,
            parent_path,
            stage_name,
        };
        value.verify_namespace()?;
        Ok(value)
    }

    pub(super) fn payload(&self) -> PathBuf {
        self.staging_path.join("payload")
    }

    fn verify_namespace(&self) -> Result<(), DownloadError> {
        let parent = fs::symlink_metadata(&self.parent_path).map_err(map_io)?;
        let actual = self.parent.metadata().map_err(map_io)?;
        if !parent.is_dir() || parent.dev() != actual.dev() || parent.ino() != actual.ino() {
            return Err(DownloadError::ChangedFile);
        }
        let stage = fs::symlink_metadata(&self.staging_path).map_err(map_io)?;
        if !stage.is_dir()
            || stage.dev() != self.staging_identity.volume
            || stage.ino() != self.staging_identity.file
            || stage.mode() & 0o077 != 0
        {
            return Err(DownloadError::ChangedFile);
        }
        Ok(())
    }

    pub(super) fn finish(
        mut self,
        source: &str,
        expected_bytes: Option<u64>,
    ) -> Result<(PathBuf, FileIdentity), DownloadError> {
        self.verify_namespace()?;
        // SAFETY: both the directory descriptor and the fixed NUL-terminated
        // leaf are owned here. O_NOFOLLOW prevents replacing payload with a link.
        let fd = unsafe {
            libc::openat(
                self.staging.as_raw_fd(),
                c"payload".as_ptr(),
                libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        if fd < 0 {
            return Err(map_io(std::io::Error::last_os_error()));
        }
        let file = unsafe { File::from_raw_fd(fd) };
        let metadata = file.metadata().map_err(map_io)?;
        if !metadata.is_file() || metadata.nlink() != 1 {
            return Err(DownloadError::ChangedFile);
        }
        if expected_bytes.is_some_and(|bytes| metadata.len() != bytes) {
            return Err(DownloadError::Integrity);
        }
        quarantine(&self.payload(), source)?;
        self.verify_namespace()?;
        let after = fs::symlink_metadata(self.payload()).map_err(map_io)?;
        if after.dev() != metadata.dev() || after.ino() != metadata.ino() {
            return Err(DownloadError::ChangedFile);
        }
        verify_quarantine(&file)?;
        file.sync_all().map_err(map_io)?;
        let original = self
            .path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or(DownloadError::Destination)?
            .to_owned();
        for collision in 0..10_000 {
            let name = collision_name(&original, collision);
            let native_name =
                CString::new(name.as_bytes()).map_err(|_| DownloadError::Destination)?;
            // Same-volume, descriptor-relative and exclusive. There is no
            // check-then-overwrite race, even for concurrent same-name downloads.
            let result = unsafe {
                libc::renameatx_np(
                    self.staging.as_raw_fd(),
                    c"payload".as_ptr(),
                    self.parent.as_raw_fd(),
                    native_name.as_ptr(),
                    libc::RENAME_EXCL,
                )
            };
            if result == 0 {
                self.path = self.parent_path.join(name);
                self.parent.sync_all().map_err(map_io)?;
                return Ok((
                    self.path.clone(),
                    identity(&file.metadata().map_err(map_io)?),
                ));
            }
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() != Some(libc::EEXIST) {
                return Err(map_io(error));
            }
        }
        Err(DownloadError::Destination)
    }
}

impl Drop for Destination {
    fn drop(&mut self) {
        // Delete only our fixed payload in our held directory, then remove
        // the directory only when its current identity still matches. Never
        // recursively erase a user path or follow a substituted symlink.
        unsafe {
            libc::unlinkat(self.staging.as_raw_fd(), c"payload".as_ptr(), 0);
        }
        if self.verify_namespace().is_ok() {
            unsafe {
                libc::unlinkat(
                    self.parent.as_raw_fd(),
                    self.stage_name.as_ptr(),
                    libc::AT_REMOVEDIR,
                );
            }
        }
    }
}

pub(super) fn select_directory(path: PathBuf) -> Result<(String, String), DownloadError> {
    let path = fs::canonicalize(path).map_err(map_io)?;
    let directory = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(&path)
        .map_err(map_io)?;
    let metadata = directory.metadata().map_err(map_io)?;
    let path = path
        .to_str()
        .filter(|path| path.len() <= 4096)
        .ok_or(DownloadError::Destination)?
        .to_owned();
    Ok((path, directory_identity(&metadata)))
}

fn directory_identity(metadata: &fs::Metadata) -> String {
    format!("{:016x}:{:016x}", metadata.dev(), metadata.ino())
}

pub(super) fn verify_file(path: &Path, expected: &FileIdentity) -> Result<(), DownloadError> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                DownloadError::MissingFile
            } else {
                DownloadError::ChangedFile
            }
        })?;
    let metadata = file.metadata().map_err(map_io)?;
    let actual = identity(&metadata);
    if expected.file_high != 0
        || !metadata.is_file()
        || actual.volume != expected.volume
        || actual.file != expected.file
        || actual.bytes != expected.bytes
        || expected
            .modified
            .is_some_and(|modified| actual.modified != Some(modified))
    {
        return Err(DownloadError::ChangedFile);
    }
    verify_quarantine(&file)
}

/// Reaps only a journalled private staging directory with the same native
/// identity. A missing directory is already clean; observed substitution is
/// rejected. Cleanup removes only the fixed payload and an empty owned folder.
pub(super) fn recover_staging(
    record: &zephium_core::downloads::DownloadRecord,
) -> Result<(), DownloadError> {
    let (Some(path), Some(expected)) = (&record.staging, &record.staging_identity) else {
        return Ok(());
    };
    if record.writer.is_some()
        || !record.state.terminal()
        || path.file_name().and_then(|name| name.to_str())
            != Some(format!(".zephium-download-{}", record.id).as_str())
    {
        return Err(DownloadError::Invalid);
    }
    let directory = match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)
    {
        Ok(directory) => directory,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(_) => return Err(DownloadError::ChangedFile),
    };
    let metadata = directory.metadata().map_err(map_io)?;
    if expected.file_high != 0
        || metadata.dev() != expected.volume
        || metadata.ino() != expected.file
        || metadata.mode() & 0o077 != 0
    {
        return Err(DownloadError::ChangedFile);
    }
    let parent_path = path.parent().ok_or(DownloadError::Destination)?;
    let parent = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(parent_path)
        .map_err(map_io)?;
    let observed = fs::symlink_metadata(path).map_err(map_io)?;
    if observed.dev() != metadata.dev() || observed.ino() != metadata.ino() || !observed.is_dir() {
        return Err(DownloadError::ChangedFile);
    }
    let name = CString::new(format!(".zephium-download-{}", record.id))
        .map_err(|_| DownloadError::Invalid)?;
    let removed = unsafe { libc::unlinkat(directory.as_raw_fd(), c"payload".as_ptr(), 0) };
    if removed != 0 && std::io::Error::last_os_error().kind() != std::io::ErrorKind::NotFound {
        return Err(DownloadError::Destination);
    }
    let removed = unsafe { libc::unlinkat(parent.as_raw_fd(), name.as_ptr(), libc::AT_REMOVEDIR) };
    if removed != 0 {
        return Err(DownloadError::Destination);
    }
    parent.sync_all().map_err(map_io)
}

fn identity(metadata: &fs::Metadata) -> FileIdentity {
    FileIdentity {
        file_high: 0,
        volume: metadata.dev(),
        file: metadata.ino(),
        bytes: metadata.len(),
        modified: Some((metadata.mtime(), metadata.mtime_nsec())),
    }
}

fn map_io(error: std::io::Error) -> DownloadError {
    match error.raw_os_error() {
        Some(libc::ENOSPC) => DownloadError::DiskFull,
        // EPERM is what macOS privacy consent returns for Downloads, Desktop
        // and Documents; EACCES is an ordinary permission denial.
        Some(libc::EPERM | libc::EACCES | libc::EROFS) => DownloadError::Permission,
        Some(libc::EBUSY | libc::EMFILE | libc::ENFILE) => DownloadError::FileBusy,
        Some(libc::EFBIG) => DownloadError::FileTooLarge,
        _ => DownloadError::Destination,
    }
}

fn verify_quarantine(file: &File) -> Result<(), DownloadError> {
    // Read metadata on the held file, not on a path that may have changed.
    let size = unsafe {
        libc::fgetxattr(
            file.as_raw_fd(),
            c"com.apple.quarantine".as_ptr(),
            std::ptr::null_mut(),
            0,
            0,
            0,
        )
    };
    if size > 0 {
        Ok(())
    } else {
        Err(DownloadError::Protection)
    }
}

fn quarantine(path: &Path, source: &str) -> Result<(), DownloadError> {
    objc2::rc::autoreleasepool(|_| quarantine_in_pool(path, source))
}

fn quarantine_in_pool(path: &Path, source: &str) -> Result<(), DownloadError> {
    use objc2::runtime::AnyObject;
    use objc2_foundation::{
        NSBundle, NSDate, NSDictionary, NSString, NSURLQuarantinePropertiesKey, NSURL,
    };
    // CoreServices exports CFString constants. CFString and NSString are
    // toll-free bridged; the linked framework owns these immutable objects.
    #[link(name = "CoreServices", kind = "framework")]
    unsafe extern "C" {
        static kLSQuarantineAgentNameKey: *const NSString;
        static kLSQuarantineAgentBundleIdentifierKey: *const NSString;
        static kLSQuarantineTimeStampKey: *const NSString;
        static kLSQuarantineTypeKey: *const NSString;
        static kLSQuarantineOriginURLKey: *const NSString;
        static kLSQuarantineTypeWebDownload: *const NSString;
    }
    let path = path.to_str().ok_or(DownloadError::Destination)?;
    let url = NSURL::fileURLWithPath(&NSString::from_str(path));
    let agent = NSString::from_str("Zephium");
    let date = NSDate::date();
    let source =
        NSURL::URLWithString(&NSString::from_str(source)).ok_or(DownloadError::Protection)?;
    unsafe {
        let bundle = NSBundle::mainBundle().bundleIdentifier();
        let mut keys = vec![
            &*kLSQuarantineAgentNameKey,
            &*kLSQuarantineTimeStampKey,
            &*kLSQuarantineTypeKey,
            &*kLSQuarantineOriginURLKey,
        ];
        let mut values: Vec<&AnyObject> =
            vec![&agent, &date, &*kLSQuarantineTypeWebDownload, &source];
        if let Some(bundle) = &bundle {
            keys.push(&*kLSQuarantineAgentBundleIdentifierKey);
            values.push(bundle);
        }
        let properties = NSDictionary::<NSString, AnyObject>::from_slices(&keys, &values);
        url.setResourceValue_forKey_error(Some(&properties), NSURLQuarantinePropertiesKey)
            .map_err(|_| DownloadError::Protection)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_saved_directory_cannot_be_redirected_before_the_next_download() {
        let root = tempfile::tempdir().unwrap();
        let selected = root.path().join("selected");
        let other = root.path().join("other");
        fs::create_dir(&selected).unwrap();
        fs::create_dir(&other).unwrap();
        let (_, receipt) = select_directory(selected.clone()).unwrap();
        fs::rename(&selected, root.path().join("original")).unwrap();
        std::os::unix::fs::symlink(&other, &selected).unwrap();
        assert!(matches!(
            Destination::prepare(
                DownloadId::generate(),
                selected.join("file.txt"),
                Some(receipt)
            ),
            Err(DownloadError::Destination)
        ));
        assert_eq!(fs::read_dir(other).unwrap().count(), 0);
    }

    #[test]
    fn recovery_removes_only_the_journalled_private_stage() {
        use zephium_core::downloads::{DownloadRecord, DownloadState};
        let root = tempfile::tempdir().unwrap();
        let id = DownloadId::generate();
        let path = root.path().join(format!(".zephium-download-{id}"));
        fs::DirBuilder::new().mode(0o700).create(&path).unwrap();
        let staging_identity = identity(&fs::metadata(&path).unwrap());
        // Pin the original inode for the substitution assertion, without
        // leaking resources or relying on the filesystem's inode reuse policy.
        let _original_directory = File::open(&path).unwrap();
        fs::write(path.join("payload"), b"partial bytes").unwrap();
        let record = DownloadRecord {
            id,
            session: DownloadId::generate(),
            revision: 1,
            created_at: 1,
            filename: "file.txt".into(),
            source: "https://example.com".into(),
            source_is_context: false,
            state: DownloadState::Interrupted,
            received: 0,
            total: None,
            error: None,
            destination: Some(root.path().join("file.txt")),
            staging: Some(path.clone()),
            staging_identity: Some(staging_identity),
            identity: None,
            writer: None,
            writer_released: false,
        };
        recover_staging(&record).unwrap();
        assert!(!path.exists());
        fs::create_dir(&path).unwrap();
        fs::write(path.join("payload"), b"replacement user data").unwrap();
        assert_eq!(recover_staging(&record), Err(DownloadError::ChangedFile));
        assert_eq!(
            fs::read(path.join("payload")).unwrap(),
            b"replacement user data"
        );
    }
    #[test]
    fn exclusive_publication_preserves_existing_file_and_quarantine() {
        let directory = tempfile::tempdir().unwrap();
        let output = directory.path().join("report.txt");
        fs::write(&output, b"existing user data").unwrap();
        let destination =
            Destination::prepare(DownloadId::generate(), output.clone(), None).unwrap();
        fs::write(destination.payload(), b"download bytes").unwrap();
        let stage = destination.staging_path.clone();
        let (published, identity) = destination.finish("https://example.com", None).unwrap();
        assert_eq!(fs::read(output).unwrap(), b"existing user data");
        assert_eq!(published.file_name().unwrap(), "report (1).txt");
        assert_eq!(fs::read(&published).unwrap(), b"download bytes");
        verify_file(&published, &identity).unwrap();
        assert!(!stage.exists());
        fs::write(&published, b"modified bytes").unwrap();
        assert_eq!(
            verify_file(&published, &identity),
            Err(DownloadError::ChangedFile)
        );
    }
    #[test]
    fn payload_symlink_is_never_published_or_followed() {
        let directory = tempfile::tempdir().unwrap();
        let victim = directory.path().join("victim");
        fs::write(&victim, b"keep").unwrap();
        let destination = Destination::prepare(
            DownloadId::generate(),
            directory.path().join("download"),
            None,
        )
        .unwrap();
        std::os::unix::fs::symlink(&victim, destination.payload()).unwrap();
        assert!(destination.finish("https://example.com", None).is_err());
        assert_eq!(fs::read(victim).unwrap(), b"keep");
        assert!(!directory.path().join("download").exists());
    }

    #[test]
    fn repeated_multidot_and_unicode_downloads_keep_numbered_names() {
        let root = tempfile::tempdir().unwrap();
        for name in ["archive.tar.gz", "Résumé.pdf", "download"] {
            fs::write(root.path().join(name), b"existing").unwrap();
            for suffix in 1..=2 {
                let destination =
                    Destination::prepare(DownloadId::generate(), root.path().join(name), None)
                        .unwrap();
                fs::write(destination.payload(), b"new").unwrap();
                let (path, receipt) = destination.finish("https://example.com", Some(3)).unwrap();
                assert_eq!(
                    path.file_name().unwrap(),
                    collision_name(name, suffix).as_str()
                );
                verify_file(&path, &receipt).unwrap();
            }
            assert_eq!(fs::read(root.path().join(name)).unwrap(), b"existing");
        }
    }

    #[test]
    fn concurrent_same_name_downloads_publish_distinct_files() {
        let root = tempfile::tempdir().unwrap();
        let output = root.path().join("report.txt");
        fs::write(&output, b"existing").unwrap();
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(8));
        let workers: Vec<_> = (0..8)
            .map(|index| {
                let destination =
                    Destination::prepare(DownloadId::generate(), output.clone(), None).unwrap();
                let bytes = format!("worker {index}");
                fs::write(destination.payload(), &bytes).unwrap();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    let (path, receipt) = destination
                        .finish("https://example.com", Some(bytes.len() as u64))
                        .unwrap();
                    assert_eq!(fs::read_to_string(&path).unwrap(), bytes);
                    verify_file(&path, &receipt).unwrap();
                    path
                })
            })
            .collect();
        let published: std::collections::HashSet<_> = workers
            .into_iter()
            .map(|worker| worker.join().unwrap())
            .collect();
        assert_eq!(published.len(), 8);
        assert_eq!(fs::read(output).unwrap(), b"existing");
        assert_eq!(fs::read_dir(root.path()).unwrap().count(), 9);
    }

    #[test]
    fn native_file_byte_mismatch_never_publishes_a_partial_file() {
        let root = tempfile::tempdir().unwrap();
        let output = root.path().join("file.txt");
        let destination =
            Destination::prepare(DownloadId::generate(), output.clone(), None).unwrap();
        fs::write(destination.payload(), b"short").unwrap();
        assert!(matches!(
            destination.finish("https://example.com", Some(100)),
            Err(DownloadError::Integrity)
        ));
        assert!(!output.exists());
        assert_eq!(fs::read_dir(root.path()).unwrap().count(), 0);
    }
}
