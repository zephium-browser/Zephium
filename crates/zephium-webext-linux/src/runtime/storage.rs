//! One storage thread per profile runtime. `storage.get` reads a whole area
//! (up to 1 GiB with `unlimitedStorage`) and every write commits a SQLite
//! transaction with a 5 s busy timeout, so none of it may run on the GTK
//! thread. One FIFO thread keeps each extension's calls in issue order, owns
//! every `Storage` (they are not `Send`), and wakes the GTK loop through a
//! socket the loop watches.

use std::collections::HashMap;
use std::io::{ErrorKind, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread::JoinHandle;

use serde_json::Value;
use zephium_core::ids::{ExtensionInstallId, ProfileId};
use zephium_webext_runtime::context::Sender;
use zephium_webext_runtime::errors::ApiError;
use zephium_webext_runtime::storage::{self, Area, SqliteBackend, Storage, StorageChangeEvent};

/// Largest snapshot file the runtime writes or reads back.
pub(crate) const MAX_STATE_FILE_BYTES: u64 = 2 * 1024 * 1024;

pub(crate) enum Job {
    Open {
        install: ExtensionInstallId,
        profile: ProfileId,
        incognito: bool,
        directory: Option<PathBuf>,
    },
    Call {
        ticket: u64,
        install: ExtensionInstallId,
        sender: Box<Sender>,
        area: Area,
        member: &'static str,
        args: Vec<Value>,
        unlimited: bool,
        now: u64,
    },
    Close {
        install: ExtensionInstallId,
    },
    Destroy {
        install: ExtensionInstallId,
        directory: Option<PathBuf>,
    },
    CommitScripts {
        ticket: u64,
        path: PathBuf,
        bytes: Vec<u8>,
    },
    Write {
        path: PathBuf,
        bytes: Vec<u8>,
    },
    Remove {
        path: PathBuf,
    },
}

#[derive(Debug)]
pub(crate) enum Done {
    Destroyed {
        install: ExtensionInstallId,
        failed: bool,
    },
    Scripts {
        ticket: u64,
        result: Result<(), ApiError>,
    },
    Call {
        ticket: u64,
        result: Result<Value, ApiError>,
        change: Option<StorageChangeEvent>,
    },
    Failed {
        install: Option<ExtensionInstallId>,
        message: &'static str,
    },
}

pub(crate) struct StorageExecutor {
    jobs: Option<mpsc::Sender<Job>>,
    done: mpsc::Receiver<Done>,
    wake: UnixStream,
    thread: Option<JoinHandle<()>>,
}

impl StorageExecutor {
    pub(crate) fn start() -> std::io::Result<Self> {
        let (wake, signal) = UnixStream::pair()?;
        wake.set_nonblocking(true)?;
        signal.set_nonblocking(true)?;
        let (jobs, inbox) = mpsc::channel();
        let (outbox, done) = mpsc::channel();
        let thread = std::thread::Builder::new()
            .name("zephium-webext-storage".into())
            .spawn(move || run(&inbox, &outbox, &signal))?;
        Ok(Self {
            jobs: Some(jobs),
            done,
            wake,
            thread: Some(thread),
        })
    }

    pub(crate) fn submit(&self, job: Job) -> bool {
        self.jobs
            .as_ref()
            .is_some_and(|jobs| jobs.send(job).is_ok())
    }

    pub(crate) fn fd(&self) -> std::os::fd::RawFd {
        self.wake.as_raw_fd()
    }

    /// Drains the wake socket and every completed job.
    pub(crate) fn take(&self) -> Vec<Done> {
        let mut buffer = [0u8; 64];
        let mut wake = &self.wake;
        while matches!(wake.read(&mut buffer), Ok(count) if count > 0) {}
        self.done.try_iter().collect()
    }

    /// Finishes queued work and joins the thread.
    pub(crate) fn stop(&mut self) {
        self.jobs.take();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for StorageExecutor {
    fn drop(&mut self) {
        self.stop();
    }
}

fn run(inbox: &mpsc::Receiver<Job>, outbox: &mpsc::Sender<Done>, mut signal: &UnixStream) {
    let mut stores: HashMap<ExtensionInstallId, Storage> = HashMap::new();
    for job in inbox {
        let done = match job {
            Job::Open {
                install,
                profile,
                incognito,
                directory,
            } => open(
                &mut stores,
                install,
                profile,
                incognito,
                directory.as_deref(),
            ),
            Job::Call {
                ticket,
                install,
                sender,
                area,
                member,
                args,
                unlimited,
                now,
            } => {
                let outcome = match stores.get_mut(&install) {
                    Some(store) => {
                        storage::execute(store, &sender, area, member, &args, unlimited, now)
                    }
                    None => Err(ApiError::other("Storage operation failed.")),
                };
                Some(match outcome {
                    Ok((value, change)) => Done::Call {
                        ticket,
                        result: Ok(value),
                        change,
                    },
                    Err(error) => Done::Call {
                        ticket,
                        result: Err(error),
                        change: None,
                    },
                })
            }
            Job::Close { install } => {
                stores.remove(&install);
                None
            }
            Job::Destroy { install, directory } => {
                let failed = stores
                    .remove(&install)
                    .is_some_and(|store| store.destroy().is_err());
                let directory_failed = directory.is_some_and(|directory| {
                    std::fs::remove_dir_all(directory)
                        .is_err_and(|error| error.kind() != ErrorKind::NotFound)
                });
                Some(Done::Destroyed {
                    install,
                    failed: failed || directory_failed,
                })
            }
            Job::CommitScripts {
                ticket,
                path,
                bytes,
            } => Some(Done::Scripts {
                ticket,
                result: write_atomic(&path, &bytes).map_err(|_| {
                    ApiError::other("Extension script registrations could not be saved.")
                }),
            }),
            Job::Write { path, bytes } => write_atomic(&path, &bytes).err().map(|_| Done::Failed {
                install: None,
                message: "Extension state could not be saved.",
            }),
            Job::Remove { path } => {
                let _ = std::fs::remove_file(path);
                None
            }
        };
        if let Some(done) = done {
            if outbox.send(done).is_err() {
                return;
            }
            match signal.write(&[1]) {
                Ok(_) => {}
                Err(error) if error.kind() == ErrorKind::WouldBlock => {}
                Err(_) => return,
            }
        }
    }
}

fn open(
    stores: &mut HashMap<ExtensionInstallId, Storage>,
    install: ExtensionInstallId,
    profile: ProfileId,
    incognito: bool,
    directory: Option<&Path>,
) -> Option<Done> {
    let store = match directory {
        Some(directory) if !incognito => private_directory(directory)
            .ok()
            .and_then(|()| SqliteBackend::open(directory.join("storage.sqlite")).ok())
            .and_then(|backend| Storage::new(profile, install, false, backend).ok()),
        _ => Some(Storage::memory(profile, install, incognito)),
    };
    match store {
        Some(store) => {
            stores.insert(install, store);
            None
        }
        None => Some(Done::Failed {
            install: Some(install),
            message: "Extension storage could not be opened.",
        }),
    }
}

pub(crate) fn private_directory(path: &Path) -> std::io::Result<()> {
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(path)
}

fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let parent = path.parent().ok_or(ErrorKind::InvalidInput)?;
    private_directory(parent)?;
    let directory = std::fs::File::open(parent)?;
    let temporary = path.with_extension("tmp");
    {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
    }
    std::fs::rename(&temporary, path)?;
    directory.sync_all()
}

/// Reads a small state file the runtime wrote earlier.
pub(crate) fn read_state(path: &Path) -> Option<Vec<u8>> {
    let file = std::fs::File::open(path).ok()?;
    if file.metadata().ok()?.len() > MAX_STATE_FILE_BYTES {
        return None;
    }
    let mut bytes = Vec::new();
    file.take(MAX_STATE_FILE_BYTES)
        .read_to_end(&mut bytes)
        .ok()?;
    Some(bytes)
}

/// A missing registration snapshot is valid; unreadable or oversized state is not.
pub(crate) fn read_scripts(path: &Path) -> std::io::Result<Option<Vec<u8>>> {
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.len() > MAX_STATE_FILE_BYTES {
        return Err(ErrorKind::InvalidData.into());
    }
    let mut bytes = Vec::new();
    file.take(MAX_STATE_FILE_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_STATE_FILE_BYTES {
        return Err(ErrorKind::InvalidData.into());
    }
    Ok(Some(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn script_snapshot_read_distinguishes_missing_and_unreadable_state() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempfile::tempdir().unwrap();
        assert!(read_scripts(&directory.path().join("missing.json"))
            .unwrap()
            .is_none());
        assert_eq!(
            read_scripts(directory.path()).unwrap_err().kind(),
            ErrorKind::InvalidData
        );
        let file = directory.path().join("file");
        std::fs::write(&file, b"[]").unwrap();
        assert!(read_scripts(&file.join("scripts.json")).is_err());
        assert_eq!(read_scripts(&file).unwrap(), Some(b"[]".to_vec()));
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o000)).unwrap();
        let unreadable = read_scripts(&file);
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(unreadable.unwrap_err().kind(), ErrorKind::PermissionDenied);
    }

    #[test]
    fn script_snapshot_read_accepts_the_limit_and_refuses_oversized_state() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("scripts.json");
        let file = std::fs::File::create(&path).unwrap();
        file.set_len(MAX_STATE_FILE_BYTES).unwrap();
        assert_eq!(
            read_scripts(&path).unwrap().unwrap().len() as u64,
            MAX_STATE_FILE_BYTES
        );
        file.set_len(MAX_STATE_FILE_BYTES + 1).unwrap();
        assert_eq!(
            read_scripts(&path).unwrap_err().kind(),
            ErrorKind::InvalidData
        );
    }
}
