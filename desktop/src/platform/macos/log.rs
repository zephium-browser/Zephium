//! The macOS half of the private diagnostic log. stderr goes to a bounded,
//! owner-only file under ~/Library/Logs, which nothing ever sends anywhere;
//! a person can attach it to a bug report from Settings.

use std::fs::{self, File, OpenOptions};
use std::io;
use std::os::fd::AsRawFd;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

const ACTIVE: &str = "zephium.log";
const ARCHIVES: usize = 3;
const SEGMENT_BYTES: u64 = 2 * 1024 * 1024;
const SEGMENT_AGE: Duration = Duration::from_secs(24 * 60 * 60);
const RETENTION: Duration = Duration::from_secs(7 * 24 * 60 * 60);
/// A busy log is checked this often, so weeks of uptime stay within bounds.
const CHECK_EVERY: Duration = Duration::from_secs(10 * 60);

/// Sends this process's stderr to the log folder, unless a terminal is
/// already reading it.
pub fn redirect_stderr(dir: &Path) {
    use std::io::IsTerminal;
    if io::stderr().is_terminal() || prepare(dir).is_err() {
        return;
    }
    let Ok(file) = open_segment(dir) else {
        return;
    };
    // SAFETY: both descriptors are valid; dup2 replaces fd 2 atomically.
    if unsafe { libc::dup2(file.as_raw_fd(), libc::STDERR_FILENO) } < 0 {
        return;
    }
    drop(file);
    let dir = dir.to_owned();
    let _ = std::thread::Builder::new()
        .name("zephium-log-rotation".to_owned())
        .spawn(move || loop {
            std::thread::sleep(CHECK_EVERY);
            if stderr_bytes().is_some_and(|bytes| bytes >= SEGMENT_BYTES) {
                if let Ok(file) = open_segment(&dir) {
                    // SAFETY: as above.
                    unsafe { libc::dup2(file.as_raw_fd(), libc::STDERR_FILENO) };
                }
            }
        });
}

fn prepare(dir: &Path) -> io::Result<()> {
    fs::create_dir_all(dir)?;
    let metadata = fs::symlink_metadata(dir)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(io::Error::other("log folder is not a plain folder"));
    }
    fs::set_permissions(dir, fs::Permissions::from_mode(0o700))
}

fn archive(dir: &Path, index: usize) -> PathBuf {
    dir.join(format!("zephium.{index}.log"))
}

/// Rotates the active file when it is full or a day old, drops week-old
/// archives, and opens the active file for appending.
fn open_segment(dir: &Path) -> io::Result<File> {
    let active = dir.join(ACTIVE);
    let now = SystemTime::now();
    let older_than = |path: &Path, age: Duration| {
        fs::symlink_metadata(path)
            .and_then(|metadata| metadata.modified())
            .is_ok_and(|modified| now.duration_since(modified).is_ok_and(|held| held >= age))
    };
    let full = fs::symlink_metadata(&active).is_ok_and(|metadata| metadata.len() >= SEGMENT_BYTES);
    if full || older_than(&active, SEGMENT_AGE) {
        for index in (1..ARCHIVES).rev() {
            let _ = fs::rename(archive(dir, index), archive(dir, index + 1));
        }
        let _ = fs::rename(&active, archive(dir, 1));
    }
    for index in 1..=ARCHIVES {
        let path = archive(dir, index);
        if older_than(&path, RETENTION) {
            let _ = fs::remove_file(path);
        }
    }
    OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(active)
}

fn stderr_bytes() -> Option<u64> {
    // SAFETY: fstat writes only into the zeroed buffer it is given.
    let mut stat: libc::stat = unsafe { std::mem::zeroed() };
    (unsafe { libc::fstat(libc::STDERR_FILENO, &mut stat) } == 0)
        .then(|| u64::try_from(stat.st_size).unwrap_or(0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_full_log_rotates_and_old_archives_go() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::write(root.join(ACTIVE), vec![b'x'; SEGMENT_BYTES as usize]).unwrap();
        fs::write(archive(root, 1), b"one").unwrap();
        fs::write(archive(root, 3), b"three").unwrap();

        drop(open_segment(root).unwrap());

        assert_eq!(fs::metadata(root.join(ACTIVE)).unwrap().len(), 0);
        assert_eq!(
            fs::read(archive(root, 1)).unwrap().len(),
            SEGMENT_BYTES as usize
        );
        assert_eq!(fs::read(archive(root, 2)).unwrap(), b"one");
        assert!(!archive(root, 4).exists(), "only three archives are kept");
        let mode = fs::metadata(root.join(ACTIVE))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn a_symlinked_log_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let elsewhere = dir.path().join("elsewhere");
        fs::write(&elsewhere, b"").unwrap();
        std::os::unix::fs::symlink(&elsewhere, dir.path().join(ACTIVE)).unwrap();
        assert!(open_segment(dir.path()).is_err());
    }
}
