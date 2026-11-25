use std::fs::{File, OpenOptions};
use std::io;
use std::path::Path;
use std::sync::Mutex;

pub(crate) fn shortcut_coordination_dir(config_dir: &Path, identifier: &str) -> std::path::PathBuf {
    let base_identifier = identifier
        .split_once(".userdata.")
        .map_or(identifier, |(base, _)| base);
    config_dir.join(base_identifier)
}

pub(crate) struct FileLease {
    file: File,
}

pub(crate) struct GlobalShortcutOwner {
    path: std::path::PathBuf,
    lease: Mutex<Option<FileLease>>,
}

impl GlobalShortcutOwner {
    pub(crate) fn acquire(coordination_dir: &Path) -> io::Result<Self> {
        let path = coordination_dir.join(".global-shortcut-owner.lock");
        let lease = FileLease::try_acquire(&path)?;
        Ok(Self {
            path,
            lease: Mutex::new(lease),
        })
    }

    pub(crate) fn owns(&self) -> bool {
        self.lease
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_some()
    }

    pub(crate) fn try_claim(&self) -> io::Result<bool> {
        let mut lease = self.lease.lock().unwrap_or_else(|e| e.into_inner());
        if lease.is_some() {
            return Ok(false);
        }
        *lease = FileLease::try_acquire(&self.path)?;
        Ok(lease.is_some())
    }
}

impl FileLease {
    pub(crate) fn try_acquire(path: &Path) -> io::Result<Option<Self>> {
        let file = match open(path) {
            Ok(file) => file,
            Err(error) if is_contended(&error) => return Ok(None),
            Err(error) => return Err(error),
        };
        if lock(&file)? {
            Ok(Some(Self { file }))
        } else {
            Ok(None)
        }
    }

    pub(crate) fn acquire(path: &Path) -> io::Result<Self> {
        Self::acquire_with_timeout(path, std::time::Duration::from_secs(10))
    }

    fn acquire_with_timeout(path: &Path, timeout: std::time::Duration) -> io::Result<Self> {
        let started = std::time::Instant::now();
        loop {
            let file = match open(path) {
                Ok(file) => Some(file),
                Err(error) if is_contended(&error) => None,
                Err(error) => return Err(error),
            };
            if let Some(file) = file {
                if lock(&file)? {
                    return Ok(Self { file });
                }
            }
            let elapsed = started.elapsed();
            if elapsed >= timeout {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "timed out waiting for the startup admission lock",
                ));
            }
            std::thread::sleep(std::time::Duration::from_millis(20).min(timeout - elapsed));
        }
    }
}

#[cfg(unix)]
fn is_contended(error: &io::Error) -> bool {
    error.kind() == io::ErrorKind::WouldBlock
}

#[cfg(windows)]
fn is_contended(error: &io::Error) -> bool {
    error.raw_os_error() == Some(32)
}

impl Drop for FileLease {
    fn drop(&mut self) {
        unlock(&self.file);
    }
}

fn open(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.share_mode(0);
    }
    options.open(path)
}

fn lock(file: &File) -> io::Result<bool> {
    match file.try_lock() {
        Ok(()) => Ok(true),
        Err(error) => {
            let error = io::Error::from(error);
            if error.kind() == io::ErrorKind::WouldBlock {
                Ok(false)
            } else {
                Err(error)
            }
        }
    }
}

fn unlock(file: &File) {
    let _ = file.unlock();
}

#[cfg(test)]
mod tests {
    use super::{shortcut_coordination_dir, FileLease, GlobalShortcutOwner};
    use std::io::BufRead;
    use std::time::Duration;

    #[test]
    fn exclusive_owner_is_unique_and_released_on_drop() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("owner.lock");
        let owner = FileLease::try_acquire(&path).unwrap().expect("first owner");
        #[cfg(any(unix, windows))]
        assert!(FileLease::try_acquire(&path).unwrap().is_none());
        drop(owner);
        assert!(FileLease::try_acquire(&path).unwrap().is_some());
    }

    #[test]
    fn startup_admission_wait_has_a_deadline() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("startup.lock");
        let owner = FileLease::try_acquire(&path).unwrap().unwrap();
        let result = FileLease::acquire_with_timeout(&path, Duration::from_millis(40));
        assert!(matches!(result, Err(error) if error.kind() == std::io::ErrorKind::TimedOut));
        drop(owner);
        assert!(FileLease::acquire_with_timeout(&path, Duration::from_millis(40)).is_ok());
    }

    #[test]
    fn startup_admission_lease_is_released_after_success_and_error_results() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("startup.lock");
        for fail in [false, true] {
            let lease = FileLease::acquire(&path).unwrap();
            assert!(FileLease::try_acquire(&path).unwrap().is_none());
            let result: std::io::Result<()> = if fail {
                Err(std::io::Error::other("injected startup failure"))
            } else {
                Ok(())
            };
            drop(lease);
            assert_eq!(result.is_err(), fail);
            assert!(FileLease::try_acquire(&path).unwrap().is_some());
        }
    }

    #[test]
    fn distinct_data_roots_share_shortcut_ownership_and_promote_after_release() {
        let temp = tempfile::tempdir().unwrap();
        let first_data_root = temp.path().join("first-data-root");
        let second_data_root = temp.path().join("second-data-root");
        let coordination_dir = temp.path().join("app-config");
        std::fs::create_dir_all(&first_data_root).unwrap();
        std::fs::create_dir_all(&second_data_root).unwrap();
        std::fs::create_dir_all(&coordination_dir).unwrap();
        assert_ne!(first_data_root, second_data_root);

        let first_shortcut_dir =
            shortcut_coordination_dir(&coordination_dir, "app.zephium.userdata.first");
        let second_shortcut_dir =
            shortcut_coordination_dir(&coordination_dir, "app.zephium.userdata.second");
        assert_eq!(first_shortcut_dir, second_shortcut_dir);
        std::fs::create_dir_all(&first_shortcut_dir).unwrap();

        let first = GlobalShortcutOwner::acquire(&first_shortcut_dir).unwrap();
        let second = GlobalShortcutOwner::acquire(&second_shortcut_dir).unwrap();
        assert!(first.owns());
        assert!(!second.owns());

        drop(first);
        assert!(second.try_claim().unwrap());
        assert!(second.owns());
    }

    #[test]
    fn custom_roots_keep_one_shortcut_coordination_directory() {
        let config_dir = std::path::Path::new("/config");
        assert_eq!(
            shortcut_coordination_dir(config_dir, "app.zephium.userdata.first"),
            config_dir.join("app.zephium")
        );
        assert_eq!(
            shortcut_coordination_dir(config_dir, "app.zephium.userdata.second"),
            config_dir.join("app.zephium")
        );
        assert_eq!(
            shortcut_coordination_dir(config_dir, "app.zephium"),
            config_dir.join("app.zephium")
        );
    }

    #[test]
    fn surviving_monitor_promotes_after_owner_exits() {
        let root = tempfile::tempdir().unwrap();
        let owner = GlobalShortcutOwner::acquire(root.path()).unwrap();
        let survivor = GlobalShortcutOwner::acquire(root.path()).unwrap();
        assert!(owner.owns());
        assert!(!survivor.owns());
        drop(owner);
        assert!(survivor.try_claim().unwrap());
        assert!(survivor.owns());
    }

    #[test]
    fn cross_process_owner_is_released_after_exit() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("owner.lock");
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "instance_lease::tests::child_process_lock_holder",
                "--nocapture",
            ])
            .env("ZEPHIUM_TEST_INSTANCE_LOCK", &path)
            .stdout(std::process::Stdio::piped())
            .stdin(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let mut output = std::io::BufReader::new(child.stdout.take().unwrap());
        loop {
            let mut line = String::new();
            assert_ne!(output.read_line(&mut line).unwrap(), 0);
            if line.trim() == "lease-ready" {
                break;
            }
        }
        assert!(FileLease::try_acquire(&path).unwrap().is_none());
        drop(child.stdin.take());
        assert!(child.wait().unwrap().success());
        assert!(FileLease::try_acquire(&path).unwrap().is_some());
    }

    #[test]
    fn child_process_lock_holder() {
        let Ok(path) = std::env::var("ZEPHIUM_TEST_INSTANCE_LOCK") else {
            return;
        };
        let _lease = FileLease::try_acquire(std::path::Path::new(&path))
            .unwrap()
            .expect("child owns lease");
        println!("lease-ready");
        std::io::Write::flush(&mut std::io::stdout()).unwrap();
        let mut acknowledgment = String::new();
        let _ = std::io::BufRead::read_line(
            &mut std::io::BufReader::new(std::io::stdin()),
            &mut acknowledgment,
        );
    }
}
