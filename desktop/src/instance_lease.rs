use std::fs::{File, OpenOptions};
use std::io;
use std::path::Path;

pub(crate) struct FileLease {
    file: File,
}

pub(crate) struct GlobalShortcutOwner(Option<FileLease>);

impl GlobalShortcutOwner {
    pub(crate) fn acquire(data_dir: &Path) -> io::Result<Self> {
        Ok(Self(FileLease::try_acquire(
            &data_dir.join(".global-shortcut-owner.lock"),
        )?))
    }

    pub(crate) fn owns(&self) -> bool {
        self.0.is_some()
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
        loop {
            let file = match open(path) {
                Ok(file) => file,
                Err(error) if is_contended(&error) => {
                    std::thread::sleep(std::time::Duration::from_millis(20));
                    continue;
                }
                Err(error) => return Err(error),
            };
            if lock(&file)? {
                return Ok(Self { file });
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
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
    use super::FileLease;
    use std::io::BufRead;

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
    fn independent_roots_do_not_share_ownership() {
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        let first = FileLease::try_acquire(&first.path().join("owner.lock"))
            .unwrap()
            .expect("first root owner");
        let second = FileLease::try_acquire(&second.path().join("owner.lock"))
            .unwrap()
            .expect("second root owner");
        drop((first, second));
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
        std::thread::sleep(std::time::Duration::from_millis(300));
    }
}
