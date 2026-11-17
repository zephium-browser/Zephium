//! Consecutive incomplete starts in one build identity's private data root.
//!
//! The composition root records an attempt before restoring tabs. It offers
//! preserved-session recovery when required, then marks this exact attempt
//! healthy only after trusted chrome and bootstrap have remained stable, or
//! clean after proven early shutdown. Dropping an attempt never marks success.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

const MARKER: &str = ".startup-recovery";
const LOCK: &str = ".startup-recovery-lock";
const MAGIC: &[u8; 4] = b"ZST\x01";
const RECORD_BYTES: usize = 29;
const INCOMPLETE_LIMIT: u8 = 2;
const MAX_DAMAGED_MARKER_BYTES: usize = 4 * 1024;
const MAX_PRESERVED_MARKERS: usize = 8;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Record {
    generation: u64,
    nonce: [u8; 16],
    incomplete: u8,
}

impl Record {
    fn decode(bytes: [u8; RECORD_BYTES]) -> io::Result<Self> {
        let mut generation = [0; 8];
        generation.copy_from_slice(&bytes[4..12]);
        let generation = u64::from_le_bytes(generation);
        let mut nonce = [0; 16];
        nonce.copy_from_slice(&bytes[12..28]);
        if &bytes[..4] != MAGIC
            || generation == 0
            || nonce == [0; 16]
            || bytes[28] > INCOMPLETE_LIMIT
        {
            return Err(invalid());
        }
        Ok(Self {
            generation,
            nonce,
            incomplete: bytes[28],
        })
    }

    fn encode(self) -> [u8; RECORD_BYTES] {
        let mut bytes = [0; RECORD_BYTES];
        bytes[..4].copy_from_slice(MAGIC);
        bytes[4..12].copy_from_slice(&self.generation.to_le_bytes());
        bytes[12..28].copy_from_slice(&self.nonce);
        bytes[28] = self.incomplete;
        bytes
    }
}

struct Root {
    path: PathBuf,
    directory: File,
    identity: Identity,
}

impl Root {
    fn open(path: &Path) -> io::Result<Self> {
        // Resolve trusted ancestor aliases once, while refusing a linked root.
        if !fs::symlink_metadata(path)?.is_dir() {
            return Err(invalid());
        }
        let path = path.canonicalize()?;
        let directory = open(&path, true, false)?;
        let identity = identity(&directory, true)?;
        let root = Self {
            path,
            directory,
            identity,
        };
        root.verify()?;
        Ok(root)
    }

    fn verify(&self) -> io::Result<()> {
        if identity(&self.directory, true)? != self.identity
            || identity(&open(&self.path, true, false)?, true)? != self.identity
        {
            return Err(invalid());
        }
        Ok(())
    }

    fn lock(&self) -> io::Result<File> {
        self.verify()?;
        let path = self.path.join(LOCK);
        let file = match create_new(&path) {
            Ok(file) => {
                file.sync_all()?;
                sync_directory(&self.directory)?;
                file
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => open(&path, false, true)?,
            Err(error) => return Err(error),
        };
        let expected = identity(&file, false)?;
        if file.metadata()?.len() != 0 {
            return Err(invalid());
        }
        file.try_lock().map_err(io::Error::from)?;
        // A replaced lock must never split cooperating owners into two locks.
        if identity(&open(&path, false, true)?, false)? != expected {
            return Err(invalid());
        }
        self.verify()?;
        Ok(file)
    }

    fn read(&self) -> io::Result<Option<Record>> {
        let Some(bytes) = self.read_bytes(MARKER, RECORD_BYTES)? else {
            return Ok(None);
        };
        Record::decode(bytes.try_into().map_err(|_| invalid())?).map(Some)
    }

    fn read_bytes(&self, name: &str, limit: usize) -> io::Result<Option<Vec<u8>>> {
        self.verify()?;
        let file = match open(&self.path.join(name), false, false) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        identity(&file, false)?;
        let length = file.metadata()?.len();
        if length > limit as u64 {
            return Err(invalid());
        }
        let mut bytes = Vec::new();
        file.take(limit as u64 + 1).read_to_end(&mut bytes)?;
        if bytes.len() as u64 != length {
            return Err(invalid());
        }
        Ok(Some(bytes))
    }

    fn write(&self, record: Record) -> io::Result<()> {
        // Ordinary writes never replace damaged or aliased marker data.
        self.read()?;
        self.replace_record(record)
    }

    fn replace_record(&self, record: Record) -> io::Result<()> {
        let mut staged = tempfile::Builder::new()
            .prefix(".startup-recovery-")
            .tempfile_in(&self.path)?;
        staged.write_all(&record.encode())?;
        staged.as_file().sync_all()?;
        self.verify()?;
        publish(&staged, &self.path.join(MARKER), true)?;
        sync_directory(&self.directory)?;
        if self.read()? != Some(record) {
            return Err(invalid());
        }
        Ok(())
    }

    fn preserve_damaged_marker(&self, bytes: &[u8]) -> io::Result<()> {
        if bytes.len() > MAX_DAMAGED_MARKER_BYTES {
            return Err(invalid());
        }
        let name = self.available_preserved_name()?.ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::AlreadyExists,
                "startup recovery evidence capacity",
            )
        })?;
        let mut staged = tempfile::Builder::new()
            .prefix(".startup-recovery-")
            .tempfile_in(&self.path)?;
        staged.write_all(bytes)?;
        staged.as_file().sync_all()?;
        self.verify()?;
        // Publication is no-replace even after the absence observation.
        publish(&staged, &self.path.join(&name), false)?;
        drop(staged);
        sync_directory(&self.directory)?;
        if self.read_bytes(&name, MAX_DAMAGED_MARKER_BYTES)?.as_deref() != Some(bytes) {
            return Err(invalid());
        }
        Ok(())
    }

    fn available_preserved_name(&self) -> io::Result<Option<String>> {
        self.verify()?;
        for slot in 0..MAX_PRESERVED_MARKERS {
            let name = format!(".startup-recovery-preserved-{slot}");
            match fs::symlink_metadata(self.path.join(&name)) {
                Ok(_) => continue,
                Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Some(name)),
                Err(error) => return Err(error),
            }
        }
        Ok(None)
    }
}

/// A persisted startup identity; clones retain the same generation and root.
#[derive(Clone)]
pub(crate) struct StartupAttempt {
    root: Arc<Root>,
    generation: u64,
    nonce: [u8; 16],
    recovery: bool,
}

/// Records a start before restoration in an already-created private data root.
/// I/O, malformed records and identity ambiguity refuse admission.
pub(crate) fn begin(data_dir: &Path) -> io::Result<StartupAttempt> {
    let root = Arc::new(Root::open(data_dir)?);
    let _lock = root.lock()?;
    let previous = root.read()?.unwrap_or(Record {
        generation: 0,
        nonce: [0; 16],
        incomplete: 0,
    });
    let generation = previous.generation.checked_add(1).ok_or_else(invalid)?;
    let current = fresh_record(
        generation,
        previous.incomplete.saturating_add(1).min(INCOMPLETE_LIMIT),
    )?;
    root.write(current)?;
    Ok(StartupAttempt {
        root,
        generation,
        nonce: current.nonce,
        recovery: previous.incomplete >= INCOMPLETE_LIMIT,
    })
}

/// Whether a failed begin can offer explicit preserved-session recovery.
/// Rechecks content under the lock; filesystem/identity failures never qualify.
pub(crate) fn marker_recovery_available(data_dir: &Path, error: &io::Error) -> bool {
    if error.kind() != io::ErrorKind::InvalidData {
        return false;
    }
    let available = || -> io::Result<bool> {
        let root = Root::open(data_dir)?;
        let _lock = root.lock()?;
        let damaged = root
            .read_bytes(MARKER, MAX_DAMAGED_MARKER_BYTES)?
            .is_some_and(|bytes| damaged(&bytes));
        Ok(damaged && root.available_preserved_name()?.is_some())
    };
    available().unwrap_or(false)
}

/// Call only after the person explicitly chose preserved-session recovery.
/// Keeps exact bounded damaged bytes without replacing any earlier evidence.
pub(crate) fn recover_marker_after_choice(data_dir: &Path) -> io::Result<StartupAttempt> {
    let root = Arc::new(Root::open(data_dir)?);
    let _lock = root.lock()?;
    let bytes = root
        .read_bytes(MARKER, MAX_DAMAGED_MARKER_BYTES)?
        .filter(|bytes| damaged(bytes))
        .ok_or_else(invalid)?;
    root.preserve_damaged_marker(&bytes)?;
    if root.read_bytes(MARKER, MAX_DAMAGED_MARKER_BYTES)? != Some(bytes) {
        return Err(invalid());
    }
    let current = fresh_record(1, 1)?;
    root.replace_record(current)?;
    Ok(StartupAttempt {
        root,
        generation: current.generation,
        nonce: current.nonce,
        recovery: false,
    })
}

fn damaged(bytes: &[u8]) -> bool {
    <[u8; RECORD_BYTES]>::try_from(bytes)
        .ok()
        .is_none_or(|bytes| Record::decode(bytes).is_err())
}

fn fresh_record(generation: u64, incomplete: u8) -> io::Result<Record> {
    // A removed/recreated marker must not reuse an old callback's identity.
    let mut nonce = [0; 16];
    getrandom::fill(&mut nonce).map_err(|_| io::Error::other("startup identity unavailable"))?;
    if nonce == [0; 16] {
        return Err(invalid());
    }
    Ok(Record {
        generation,
        nonce,
        incomplete,
    })
}

impl StartupAttempt {
    pub(crate) fn requires_recovery(&self) -> bool {
        self.recovery
    }

    /// Call only after trusted chrome + bootstrap have remained stable.
    /// False means a successor owns the marker; no state was changed.
    pub(crate) fn mark_healthy(&self) -> io::Result<bool> {
        self.clear()
    }

    /// Call only after proven clean shutdown, including a clean early quit.
    pub(crate) fn mark_clean(&self) -> io::Result<bool> {
        self.clear()
    }

    fn clear(&self) -> io::Result<bool> {
        let _lock = self.root.lock()?;
        let Some(current) = self.root.read()? else {
            return Ok(false);
        };
        if current.generation != self.generation || current.nonce != self.nonce {
            return Ok(false);
        }
        if current.incomplete != 0 {
            self.root.write(Record {
                generation: self.generation,
                nonce: self.nonce,
                incomplete: 0,
            })?;
        }
        Ok(true)
    }
}

fn invalid() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "invalid startup recovery state")
}

fn create_new(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true).write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

fn open(path: &Path, directory: bool, writable: bool) -> io::Result<File> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.is_dir() != directory
        || (!directory && !metadata.is_file())
        || metadata.file_type().is_symlink()
    {
        return Err(invalid());
    }
    let mut options = OpenOptions::new();
    options.read(true).write(writable);
    #[cfg(target_os = "macos")]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        // Open reparse points themselves; pin the root/lock against replacement.
        options.custom_flags(0x0020_0000 | if directory { 0x0200_0000 } else { 0 });
        if directory || writable {
            options.share_mode(3); // FILE_SHARE_READ | FILE_SHARE_WRITE
        }
    }
    let file = options.open(path)?;
    identity(&file, directory)?;
    Ok(file)
}

#[cfg(unix)]
type Identity = (u64, u64);

#[cfg(unix)]
fn identity(file: &File, directory: bool) -> io::Result<Identity> {
    use std::os::unix::fs::MetadataExt;
    let metadata = file.metadata()?;
    if metadata.is_dir() != directory
        || (!directory && (!metadata.is_file() || metadata.nlink() != 1))
        || metadata.mode() & 0o077 != 0
    {
        return Err(invalid());
    }
    Ok((metadata.dev(), metadata.ino()))
}

#[cfg(windows)]
type Identity = (u32, u64);

#[cfg(windows)]
fn identity(file: &File, directory: bool) -> io::Result<Identity> {
    use std::os::windows::io::AsRawHandle;
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::Storage::FileSystem::{
        GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION,
    };
    let mut information = BY_HANDLE_FILE_INFORMATION::default();
    // SAFETY: file owns a live handle and information is writable for the call.
    unsafe { GetFileInformationByHandle(HANDLE(file.as_raw_handle()), &mut information) }
        .map_err(io::Error::other)?;
    if information.dwFileAttributes & 0x400 != 0
        || (information.dwFileAttributes & 0x10 != 0) != directory
        || (!directory && information.nNumberOfLinks != 1)
    {
        return Err(invalid());
    }
    Ok((
        information.dwVolumeSerialNumber,
        (u64::from(information.nFileIndexHigh) << 32) | u64::from(information.nFileIndexLow),
    ))
}

#[cfg(not(windows))]
fn publish(staged: &tempfile::NamedTempFile, destination: &Path, replace: bool) -> io::Result<()> {
    if replace {
        fs::rename(staged.path(), destination)
    } else {
        // Same-directory hard-link publication cannot replace a raced-in name.
        // Dropping staged removes its source name before identity verification.
        fs::hard_link(staged.path(), destination)
    }
}

#[cfg(windows)]
fn publish(staged: &tempfile::NamedTempFile, destination: &Path, replace: bool) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows::core::PCWSTR;
    use windows::Win32::Storage::FileSystem::{
        MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
    };
    let source: Vec<_> = staged.path().as_os_str().encode_wide().chain([0]).collect();
    let destination: Vec<_> = destination.as_os_str().encode_wide().chain([0]).collect();
    // SAFETY: both buffers are owned, NUL-terminated paths for this call.
    unsafe {
        MoveFileExW(
            PCWSTR(source.as_ptr()),
            PCWSTR(destination.as_ptr()),
            if replace {
                MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH
            } else {
                MOVEFILE_WRITE_THROUGH
            },
        )
    }
    .map_err(io::Error::other)
}

fn sync_directory(directory: &File) -> io::Result<()> {
    #[cfg(not(windows))]
    return directory.sync_all();
    // Windows replacement uses MOVEFILE_WRITE_THROUGH after syncing file data.
    #[cfg(windows)]
    {
        let _ = directory;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tempdir() -> tempfile::TempDir {
        let root = tempfile::tempdir().unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
        }
        root
    }

    fn record(generation: u64, incomplete: u8) -> Record {
        Record {
            generation,
            nonce: [1; 16],
            incomplete,
        }
    }

    #[test]
    fn two_incomplete_starts_require_recovery_before_the_third_restore() {
        let root = tempdir();
        let first = begin(root.path()).unwrap();
        assert!(!first.requires_recovery());
        let second = begin(root.path()).unwrap();
        assert!(!second.requires_recovery());
        let third = begin(root.path()).unwrap();
        assert!(third.requires_recovery());
        for _ in 0..4 {
            assert!(begin(root.path()).unwrap().requires_recovery());
        }
        assert_eq!(third.root.read().unwrap().unwrap().incomplete, 2);
    }

    #[test]
    fn healthy_and_clean_starts_reset_the_budget_without_reusing_generations() {
        let root = tempdir();
        let early = begin(root.path()).unwrap();
        assert!(early.mark_clean().unwrap());
        let healthy = begin(root.path()).unwrap();
        assert!(!healthy.requires_recovery());
        assert!(healthy.generation > early.generation);
        assert!(healthy.mark_healthy().unwrap());
        assert!(healthy.mark_healthy().unwrap());
        assert!(!begin(root.path()).unwrap().requires_recovery());
        drop(begin(root.path()).unwrap());
        let recovery = begin(root.path()).unwrap();
        assert!(recovery.requires_recovery());
        assert!(recovery.mark_healthy().unwrap());
        assert!(!begin(root.path()).unwrap().requires_recovery());
    }

    #[test]
    fn delayed_healthy_and_clean_callbacks_cannot_clear_a_successor() {
        let root = tempdir();
        let old = begin(root.path()).unwrap();
        let delayed = old.clone();
        let current = begin(root.path()).unwrap();
        assert!(!old.mark_healthy().unwrap());
        assert!(!delayed.mark_clean().unwrap());
        assert_eq!(current.root.read().unwrap().unwrap().incomplete, 2);
        assert!(begin(root.path()).unwrap().requires_recovery());
    }

    #[test]
    fn build_roots_are_independent_and_drop_does_not_claim_clean_shutdown() {
        let product = tempdir();
        let qa = tempdir();
        drop(begin(product.path()).unwrap());
        drop(begin(product.path()).unwrap());
        assert!(begin(product.path()).unwrap().requires_recovery());
        assert!(!begin(qa.path()).unwrap().requires_recovery());
    }

    #[test]
    fn corrupt_oversized_and_exhausted_records_refuse_without_replacement() {
        let root = tempdir();
        let attempt = begin(root.path()).unwrap();
        let path = root.path().join(MARKER);
        let mut invalid_count = record(3, 2).encode();
        invalid_count[28] = 3;
        for bytes in [
            vec![],
            vec![0; RECORD_BYTES],
            vec![0; RECORD_BYTES - 1],
            vec![0; RECORD_BYTES + 1],
            invalid_count.to_vec(),
            record(0, 1).encode().to_vec(),
            Record {
                nonce: [0; 16],
                ..record(3, 1)
            }
            .encode()
            .to_vec(),
            record(u64::MAX, 2).encode().to_vec(),
        ] {
            fs::write(&path, &bytes).unwrap();
            let error = begin(root.path()).err().unwrap();
            assert_eq!(
                marker_recovery_available(root.path(), &error),
                damaged(&bytes)
            );
            assert_eq!(fs::read(&path).unwrap(), bytes);
        }
        assert!(!attempt.mark_clean().unwrap());
    }

    #[test]
    fn an_occupied_lock_refuses_instead_of_racing_generation_updates() {
        let root = tempdir();
        let attempt = begin(root.path()).unwrap();
        let _held = attempt.root.lock().unwrap();
        let error = begin(root.path()).err().unwrap();
        assert!(!marker_recovery_available(root.path(), &error));
        assert!(recover_marker_after_choice(root.path()).is_err());
        assert!(attempt.mark_healthy().is_err());
        assert_eq!(attempt.root.read().unwrap().unwrap().incomplete, 1);
    }

    #[test]
    fn recreated_marker_cannot_reuse_a_delayed_callbacks_identity() {
        let root = tempdir();
        let old = begin(root.path()).unwrap();
        fs::remove_file(root.path().join(MARKER)).unwrap();
        let successor = begin(root.path()).unwrap();
        assert_eq!(old.generation, successor.generation);
        assert!(!old.mark_clean().unwrap());
        assert_eq!(successor.root.read().unwrap().unwrap().incomplete, 1);
    }

    #[test]
    fn explicit_damage_recovery_preserves_exact_bytes_and_retires_old_callbacks() {
        let root = tempdir();
        let old = begin(root.path()).unwrap();
        let bytes = b"damaged startup marker";
        fs::write(root.path().join(MARKER), bytes).unwrap();
        let error = begin(root.path()).err().unwrap();
        assert!(marker_recovery_available(root.path(), &error));
        assert_eq!(fs::read(root.path().join(MARKER)).unwrap(), bytes);
        let recovered = recover_marker_after_choice(root.path()).unwrap();
        assert!(!recovered.requires_recovery());
        assert_eq!(
            fs::read(root.path().join(".startup-recovery-preserved-0")).unwrap(),
            bytes
        );
        assert!(!old.mark_clean().unwrap());
        assert_eq!(recovered.root.read().unwrap().unwrap().incomplete, 1);
        assert!(!marker_recovery_available(root.path(), &error));
        assert!(recover_marker_after_choice(root.path()).is_err());
    }

    #[test]
    fn damage_recovery_has_an_exact_byte_bound_and_preserves_existing_evidence() {
        let root = tempdir();
        let attempt = begin(root.path()).unwrap();
        let first = root.path().join(".startup-recovery-preserved-0");
        fs::write(&first, b"previous evidence").unwrap();
        let bytes = vec![b'x'; MAX_DAMAGED_MARKER_BYTES];
        fs::write(root.path().join(MARKER), &bytes).unwrap();
        let error = begin(root.path()).err().unwrap();
        assert!(marker_recovery_available(root.path(), &error));
        recover_marker_after_choice(root.path()).unwrap();
        assert_eq!(fs::read(&first).unwrap(), b"previous evidence");
        assert_eq!(
            fs::read(root.path().join(".startup-recovery-preserved-1")).unwrap(),
            bytes
        );
        let oversized = vec![b'x'; MAX_DAMAGED_MARKER_BYTES + 1];
        fs::write(root.path().join(MARKER), &oversized).unwrap();
        assert!(!marker_recovery_available(root.path(), &error));
        assert!(recover_marker_after_choice(root.path()).is_err());
        assert_eq!(fs::read(root.path().join(MARKER)).unwrap(), oversized);
        assert!(attempt.mark_healthy().is_err());
    }

    #[test]
    fn full_evidence_capacity_never_overwrites_a_preserved_marker() {
        let root = tempdir();
        let _attempt = begin(root.path()).unwrap();
        fs::write(root.path().join(MARKER), b"new damage").unwrap();
        for slot in 0..MAX_PRESERVED_MARKERS {
            fs::write(
                root.path()
                    .join(format!(".startup-recovery-preserved-{slot}")),
                format!("original evidence {slot}"),
            )
            .unwrap();
        }
        let error = begin(root.path()).err().unwrap();
        assert!(!marker_recovery_available(root.path(), &error));
        assert!(recover_marker_after_choice(root.path()).is_err());
        assert_eq!(fs::read(root.path().join(MARKER)).unwrap(), b"new damage");
        for slot in 0..MAX_PRESERVED_MARKERS {
            assert_eq!(
                fs::read(
                    root.path()
                        .join(format!(".startup-recovery-preserved-{slot}"))
                )
                .unwrap(),
                format!("original evidence {slot}").as_bytes()
            );
        }
    }

    #[test]
    fn hard_linked_marker_and_lock_are_refused_without_modifying_aliases() {
        for name in [MARKER, LOCK] {
            let root = tempdir();
            let target = root.path().join("target");
            let bytes = if name == MARKER {
                record(1, 1).encode().to_vec()
            } else {
                vec![]
            };
            let mut file = create_new(&target).unwrap();
            file.write_all(&bytes).unwrap();
            drop(file);
            fs::hard_link(&target, root.path().join(name)).unwrap();
            let error = begin(root.path()).err().unwrap();
            assert!(!marker_recovery_available(root.path(), &error));
            assert!(recover_marker_after_choice(root.path()).is_err());
            assert_eq!(fs::read(&target).unwrap(), bytes);
        }
    }

    #[cfg(unix)]
    #[test]
    fn linked_marker_lock_and_root_are_refused_without_touching_targets() {
        use std::os::unix::fs::{symlink, PermissionsExt};
        let root = tempdir();
        let target = root.path().join("target");
        fs::write(&target, []).unwrap();
        fs::set_permissions(&target, fs::Permissions::from_mode(0o600)).unwrap();
        for name in [MARKER, LOCK] {
            let bytes = if name == MARKER {
                record(1, 1).encode().to_vec()
            } else {
                vec![]
            };
            fs::write(&target, &bytes).unwrap();
            let path = root.path().join(name);
            symlink(&target, &path).unwrap();
            assert!(begin(root.path()).is_err());
            assert_eq!(fs::read(&target).unwrap(), bytes);
            fs::remove_file(&path).unwrap();
            if name == MARKER {
                fs::remove_file(root.path().join(LOCK)).unwrap();
            }
        }
        let link = root.path().join("root-link");
        symlink(root.path(), &link).unwrap();
        assert!(begin(&link).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn damage_recovery_does_not_bypass_private_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempdir();
        let _attempt = begin(root.path()).unwrap();
        let marker = root.path().join(MARKER);
        fs::write(&marker, b"damage").unwrap();
        fs::set_permissions(&marker, fs::Permissions::from_mode(0o644)).unwrap();
        let error = begin(root.path()).err().unwrap();
        assert!(!marker_recovery_available(root.path(), &error));
        assert!(recover_marker_after_choice(root.path()).is_err());
        assert_eq!(fs::read(marker).unwrap(), b"damage");
        fs::set_permissions(root.path(), fs::Permissions::from_mode(0o755)).unwrap();
        assert!(!marker_recovery_available(root.path(), &error));
        assert!(recover_marker_after_choice(root.path()).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn replaced_root_cannot_be_cleared_by_its_old_owner() {
        use std::os::unix::fs::PermissionsExt;
        let parent = tempdir();
        let path = parent.path().join("data");
        fs::create_dir(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        let old = begin(&path).unwrap();
        fs::rename(&path, parent.path().join("retired")).unwrap();
        fs::create_dir(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        let successor = begin(&path).unwrap();
        assert!(old.mark_healthy().is_err());
        assert_eq!(successor.root.read().unwrap().unwrap().incomplete, 1);
    }
}
