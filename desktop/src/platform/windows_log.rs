use std::fs::{File, Metadata, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

const ACTIVE_LOG_NAME: &str = "zephium.log";
const LOG_LOCK_NAME: &str = ".zephium-log.lock";

#[derive(Clone, Copy)]
struct LogPolicy {
    segment_bytes: u64,
    archives: usize,
    segment_age: Duration,
    retention: Duration,
}

// stderr can include URLs and local filesystem errors. Keep enough data for
// crash diagnosis without turning the profile into an unbounded activity log.
const LOG_POLICY: LogPolicy = LogPolicy {
    segment_bytes: 2 * 1024 * 1024,
    archives: 3,
    segment_age: Duration::from_secs(24 * 60 * 60),
    retention: Duration::from_secs(7 * 24 * 60 * 60),
};

struct RotatingLog {
    directory: PathBuf,
    policy: LogPolicy,
    current: Option<File>,
    current_bytes: u64,
    segment_started: SystemTime,
    // Keeping every directory component open without FILE_SHARE_DELETE makes
    // a checked path resistant to junction/symlink replacement while logging.
    _directory_chain: Vec<File>,
    // The lock is held for the broker's lifetime. A second process therefore
    // cannot rotate a file which this process is still writing.
    _process_lock: File,
}

impl RotatingLog {
    fn open(directory: &Path, policy: LogPolicy) -> io::Result<Self> {
        if policy.segment_bytes == 0 || policy.archives == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "log policy must have non-zero segment and archive limits",
            ));
        }
        if !directory.is_absolute() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "diagnostic directory must be absolute",
            ));
        }

        let directory_chain = open_directory_chain(directory)?;
        let process_lock = open_lock(&directory.join(LOG_LOCK_NAME))?;
        preflight_managed_logs(directory, policy)?;

        let now = SystemTime::now();
        purge_expired(directory, policy, now)?;
        bound_existing_segments(directory, policy)?;
        let current_path = directory.join(ACTIVE_LOG_NAME);
        let current = open_current(&current_path)?;
        let metadata = current.metadata()?;
        let current_bytes = metadata.len();
        // Legacy segments were bounded before the append-only handle opened.
        debug_assert!(current_bytes <= policy.segment_bytes);
        // `purge_expired` rejected missing/future creation and
        // modification times immediately before this handle opened.
        let segment_started = verified_segment_started(&metadata, now).unwrap_or(now);

        let mut sink = Self {
            directory: directory.to_owned(),
            policy,
            current: Some(current),
            current_bytes,
            segment_started,
            _directory_chain: directory_chain,
            _process_lock: process_lock,
        };
        let segment_is_old = current_bytes > 0
            && now
                .duration_since(segment_started)
                .is_ok_and(|age| age >= policy.segment_age);
        if current_bytes >= policy.segment_bytes || segment_is_old {
            sink.rotate(now)?;
        }
        Ok(sink)
    }

    fn write_all_bounded(&mut self, mut bytes: &[u8]) -> io::Result<()> {
        while !bytes.is_empty() {
            let now = SystemTime::now();
            let segment_is_old = self.current_bytes > 0
                && now
                    .duration_since(self.segment_started)
                    .is_ok_and(|age| age >= self.policy.segment_age);
            if self.current_bytes >= self.policy.segment_bytes || segment_is_old {
                self.rotate(now)?;
            }

            let available = (self.policy.segment_bytes - self.current_bytes) as usize;
            let requested = available.min(bytes.len());
            let Some(current) = self.current.as_mut() else {
                return Err(io::Error::other("diagnostic log is not open"));
            };
            match current.write(&bytes[..requested]) {
                Ok(0) => {
                    return Err(io::Error::new(
                        io::ErrorKind::WriteZero,
                        "failed to write diagnostic log",
                    ));
                }
                Ok(written) => {
                    self.current_bytes += written as u64;
                    bytes = &bytes[written..];
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }

    fn rotate(&mut self, now: SystemTime) -> io::Result<()> {
        if let Some(current) = self.current.take() {
            let _ = current.sync_data();
            drop(current);
        }
        preflight_managed_logs(&self.directory, self.policy)?;
        purge_expired(&self.directory, self.policy, now)?;
        rotate_paths(&self.directory, self.policy)?;

        let current = open_current(&self.directory.join(ACTIVE_LOG_NAME))?;
        self.current = Some(current);
        self.current_bytes = 0;
        self.segment_started = now;
        Ok(())
    }
}

fn archive_path(directory: &Path, index: usize) -> PathBuf {
    directory.join(format!("zephium.{index}.log"))
}

fn is_reparse_point(metadata: &Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    use windows::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;

    metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0
}

fn open_directory_chain(directory: &Path) -> io::Result<Vec<File>> {
    use std::os::windows::fs::OpenOptionsExt;
    use windows::Win32::Storage::FileSystem::{
        FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ, FILE_SHARE_WRITE,
    };

    let mut paths = directory
        .ancestors()
        .filter(|path| !path.as_os_str().is_empty())
        .collect::<Vec<_>>();
    paths.reverse();

    let mut handles = Vec::with_capacity(paths.len());
    for path in paths {
        let file = OpenOptions::new()
            .read(true)
            .share_mode((FILE_SHARE_READ | FILE_SHARE_WRITE).0)
            .custom_flags((FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT).0)
            .open(path)?;
        let metadata = file.metadata()?;
        if !metadata.is_dir() || is_reparse_point(&metadata) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!(
                    "diagnostic directory contains a reparse point: {}",
                    path.display()
                ),
            ));
        }
        handles.push(file);
    }
    if handles.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "diagnostic directory is empty",
        ));
    }
    Ok(handles)
}

fn validate_open_file(file: &File, path: &Path) -> io::Result<()> {
    use std::os::windows::io::AsRawHandle;
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::Storage::FileSystem::{
        GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION,
    };

    let metadata = file.metadata()?;
    if !metadata.is_file() || is_reparse_point(&metadata) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!(
                "diagnostic path is not a regular non-reparse file: {}",
                path.display()
            ),
        ));
    }

    let mut information = BY_HANDLE_FILE_INFORMATION::default();
    unsafe {
        GetFileInformationByHandle(HANDLE(file.as_raw_handle()), &mut information)
            .map_err(|error| io::Error::other(error.to_string()))?;
    }
    if information.nNumberOfLinks != 1 {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("diagnostic path has hard links: {}", path.display()),
        ));
    }
    Ok(())
}

fn open_lock(path: &Path) -> io::Result<File> {
    use std::os::windows::fs::OpenOptionsExt;
    use windows::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT;

    let file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .share_mode(0)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT.0)
        .open(path)?;
    validate_open_file(&file, path)?;
    file.set_len(0)?;
    Ok(file)
}

fn open_current(path: &Path) -> io::Result<File> {
    use std::os::windows::fs::OpenOptionsExt;
    use windows::Win32::Storage::FileSystem::{
        FILE_ATTRIBUTE_NOT_CONTENT_INDEXED, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ,
    };

    let file = OpenOptions::new()
        .create(true)
        .read(true)
        .append(true)
        .share_mode(FILE_SHARE_READ.0)
        .custom_flags((FILE_FLAG_OPEN_REPARSE_POINT | FILE_ATTRIBUTE_NOT_CONTENT_INDEXED).0)
        .open(path)?;
    validate_open_file(&file, path)?;
    Ok(file)
}

fn truncate_legacy_segment(path: &Path, maximum_bytes: u64) -> io::Result<()> {
    use std::os::windows::fs::OpenOptionsExt;
    use windows::Win32::Storage::FileSystem::{
        FILE_ATTRIBUTE_NOT_CONTENT_INDEXED, FILE_FLAG_OPEN_REPARSE_POINT,
    };

    // An append-only handle intentionally lacks FILE_WRITE_DATA and cannot be
    // trusted to truncate on every Windows filesystem. Use a short-lived,
    // exclusive full-write handle, validate it, then reopen append-only.
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .share_mode(0)
        .custom_flags((FILE_FLAG_OPEN_REPARSE_POINT | FILE_ATTRIBUTE_NOT_CONTENT_INDEXED).0)
        .open(path)?;
    validate_open_file(&file, path)?;
    let modified = file.metadata()?.modified().ok();
    file.set_len(maximum_bytes)?;
    if let Some(modified) = modified {
        file.set_times(std::fs::FileTimes::new().set_modified(modified))?;
    }
    Ok(())
}

fn bound_existing_segments(directory: &Path, policy: LogPolicy) -> io::Result<()> {
    for path in std::iter::once(directory.join(ACTIVE_LOG_NAME))
        .chain((1..=policy.archives).map(|index| archive_path(directory, index)))
    {
        let metadata = match std::fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        if metadata.len() > policy.segment_bytes {
            truncate_legacy_segment(&path, policy.segment_bytes)?;
        }
    }
    Ok(())
}

fn preflight_managed_logs(directory: &Path, policy: LogPolicy) -> io::Result<()> {
    for path in std::iter::once(directory.join(ACTIVE_LOG_NAME))
        .chain((1..=policy.archives).map(|index| archive_path(directory, index)))
    {
        let metadata = match std::fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        if !metadata.is_file() || is_reparse_point(&metadata) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!("unsafe diagnostic log path: {}", path.display()),
            ));
        }
    }
    Ok(())
}

fn purge_expired(directory: &Path, policy: LogPolicy, now: SystemTime) -> io::Result<()> {
    for path in std::iter::once(directory.join(ACTIVE_LOG_NAME))
        .chain((1..=policy.archives).map(|index| archive_path(directory, index)))
    {
        let metadata = match std::fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        // Creation time bounds the oldest record even when a small active
        // segment is appended across many process launches. Modification time
        // is also required to be present and non-future as a tamper/clock
        // sanity check.
        let expired = verified_segment_started(&metadata, now)
            .and_then(|started| now.duration_since(started).ok())
            .is_none_or(|age| age >= policy.retention);
        if expired {
            std::fs::remove_file(path)?;
        }
    }
    Ok(())
}

fn verified_segment_started(metadata: &Metadata, now: SystemTime) -> Option<SystemTime> {
    let created = metadata.created().ok()?;
    let modified = metadata.modified().ok()?;
    (created <= now && modified <= now).then_some(created)
}

fn rotate_paths(directory: &Path, policy: LogPolicy) -> io::Result<()> {
    let oldest = archive_path(directory, policy.archives);
    match std::fs::remove_file(&oldest) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    for index in (1..policy.archives).rev() {
        let source = archive_path(directory, index);
        let destination = archive_path(directory, index + 1);
        match std::fs::rename(source, destination) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    let current = directory.join(ACTIVE_LOG_NAME);
    match std::fs::rename(current, archive_path(directory, 1)) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

const STDERR_BUFFER_CHUNKS: usize = 32;

fn drain_stderr(mut pipe: File, queue: std::sync::mpsc::SyncSender<Vec<u8>>) {
    let mut buffer = [0_u8; 16 * 1024];
    loop {
        let read = match pipe.read(&mut buffer) {
            Ok(0) => return,
            Ok(read) => read,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(_) => return,
        };
        match queue.try_send(buffer[..read].to_vec()) {
            Ok(()) | Err(std::sync::mpsc::TrySendError::Full(_)) => {}
            Err(std::sync::mpsc::TrySendError::Disconnected(_)) => return,
        }
    }
}

fn write_stderr(queue: std::sync::mpsc::Receiver<Vec<u8>>, mut sink: RotatingLog) -> bool {
    let mut logging_available = true;
    while let Ok(bytes) = queue.recv() {
        if logging_available && sink.write_all_bounded(&bytes).is_err() {
            // Continue draining the bounded memory queue after a disk/path
            // failure. The independent pipe reader never waits on filesystem
            // I/O and drops chunks when this writer is temporarily stalled.
            logging_available = false;
        }
    }
    logging_available
        && sink
            .current
            .as_ref()
            .is_some_and(|file| file.sync_all().is_ok())
}

type LogDrain = (File, std::sync::mpsc::Receiver<bool>);
static LOG_DRAIN: std::sync::Mutex<Option<LogDrain>> = std::sync::Mutex::new(None);

/// Close the broker input, then wait for queued bytes and the file sync within
/// the caller's bound. A stuck disk must not prevent final process termination.
pub fn flush_stderr_until(deadline: std::time::Instant) -> bool {
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::System::Console::{SetStdHandle, STD_ERROR_HANDLE};
    let drain = LOG_DRAIN
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .take();
    let Some((pipe, done)) = drain else {
        return true;
    };
    // Detach before closing our owned handle so later diagnostics fail harmlessly.
    let _ = unsafe { SetStdHandle(STD_ERROR_HANDLE, HANDLE::default()) };
    drop(pipe);
    done.recv_timeout(deadline.saturating_duration_since(std::time::Instant::now())) == Ok(true)
}

// GUI-subsystem builds have no stderr. Route it through a bounded broker so
// Windows-only failures remain diagnosable without an indefinitely growing,
// privacy-sensitive append-only file. Filesystem anomalies disable file
// logging; they never make the browser follow an attacker-controlled path.
pub fn redirect_stderr(dir: &Path) {
    use std::os::windows::io::{AsRawHandle, FromRawHandle};
    use windows::Win32::Foundation::{GetHandleInformation, HANDLE};
    use windows::Win32::System::Console::{GetStdHandle, SetStdHandle, STD_ERROR_HANDLE};
    use windows::Win32::System::Pipes::CreatePipe;

    let live = unsafe { GetStdHandle(STD_ERROR_HANDLE) }.is_ok_and(|handle| {
        if handle.is_invalid() || handle.0.is_null() {
            return false;
        }
        let mut flags = 0_u32;
        unsafe { GetHandleInformation(handle, &mut flags) }.is_ok()
    });
    if live {
        return;
    }

    let Ok(sink) = RotatingLog::open(dir, LOG_POLICY) else {
        return;
    };

    let mut read_handle = HANDLE::default();
    let mut write_handle = HANDLE::default();
    if unsafe { CreatePipe(&mut read_handle, &mut write_handle, None, 64 * 1024) }.is_err() {
        return;
    }
    let read_pipe = unsafe { File::from_raw_handle(read_handle.0) };
    let write_pipe = unsafe { File::from_raw_handle(write_handle.0) };
    let (queue, receiver) = std::sync::mpsc::sync_channel(STDERR_BUFFER_CHUNKS);
    let (flushed, done) = std::sync::mpsc::sync_channel(1);
    let Ok(_writer) = std::thread::Builder::new()
        .name("zephium-stderr-writer".to_owned())
        .spawn(move || {
            let _ = flushed.send(write_stderr(receiver, sink));
        })
    else {
        return;
    };
    let Ok(_reader) = std::thread::Builder::new()
        .name("zephium-stderr-reader".to_owned())
        .spawn(move || drain_stderr(read_pipe, queue))
    else {
        return;
    };

    if unsafe { SetStdHandle(STD_ERROR_HANDLE, HANDLE(write_pipe.as_raw_handle())) }.is_ok() {
        *LOG_DRAIN.lock().unwrap_or_else(|error| error.into_inner()) = Some((write_pipe, done));
    }
}

#[cfg(test)]
mod log_tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_DIRECTORY_ID: AtomicU64 = AtomicU64::new(1);

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new() -> Self {
            let id = TEST_DIRECTORY_ID.fetch_add(1, Ordering::Relaxed);
            let nonce = SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .expect("test clock follows Unix epoch")
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "zephium-log-test-{}-{id}-{nonce}",
                std::process::id()
            ));
            std::fs::create_dir(&path).expect("create isolated log test directory");
            Self(path)
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn policy() -> LogPolicy {
        LogPolicy {
            segment_bytes: 32,
            archives: 2,
            segment_age: Duration::from_secs(60 * 60),
            retention: Duration::from_secs(24 * 60 * 60),
        }
    }

    #[test]
    fn shutdown_drains_queued_log_bytes_before_acknowledging() {
        let directory = TestDirectory::new();
        let sink = RotatingLog::open(&directory.0, policy()).unwrap();
        let (send, receive) = std::sync::mpsc::sync_channel(2);
        send.send(b"final shutdown record\n".to_vec()).unwrap();
        drop(send);
        assert!(write_stderr(receive, sink));
        assert_eq!(
            std::fs::read(directory.0.join(ACTIVE_LOG_NAME)).unwrap(),
            b"final shutdown record\n"
        );
    }

    #[test]
    fn rotation_is_strictly_byte_bounded() {
        let directory = TestDirectory::new();
        let mut sink = RotatingLog::open(&directory.0, policy()).expect("open bounded log");
        let input = (0_u8..100).collect::<Vec<_>>();
        sink.write_all_bounded(&input).expect("write test record");
        drop(sink);

        let current = std::fs::read(directory.0.join(ACTIVE_LOG_NAME)).expect("read current");
        let first = std::fs::read(archive_path(&directory.0, 1)).expect("read first archive");
        let second = std::fs::read(archive_path(&directory.0, 2)).expect("read second archive");
        assert_eq!(current, input[96..]);
        assert_eq!(first, input[64..96]);
        assert_eq!(second, input[32..64]);
        assert!([current.len(), first.len(), second.len()]
            .into_iter()
            .all(|bytes| bytes <= policy().segment_bytes as usize));
    }

    #[test]
    fn normal_start_preserves_and_appends_existing_diagnostics() {
        let directory = TestDirectory::new();
        std::fs::write(directory.0.join(ACTIVE_LOG_NAME), b"old\n")
            .expect("seed existing diagnostics");
        let mut sink = RotatingLog::open(&directory.0, policy()).expect("open existing log");
        sink.write_all_bounded(b"new\n")
            .expect("append diagnostics");
        drop(sink);

        assert_eq!(
            std::fs::read(directory.0.join(ACTIVE_LOG_NAME)).expect("read appended log"),
            b"old\nnew\n"
        );
    }

    #[test]
    fn lifetime_lock_rejects_a_second_writer() {
        let directory = TestDirectory::new();
        let first = RotatingLog::open(&directory.0, policy()).expect("open first writer");
        assert!(RotatingLog::open(&directory.0, policy()).is_err());
        drop(first);
        RotatingLog::open(&directory.0, policy()).expect("lock released with first writer");
    }

    #[test]
    fn oversized_legacy_log_is_bounded_before_archival() {
        let directory = TestDirectory::new();
        std::fs::write(directory.0.join(ACTIVE_LOG_NAME), vec![7_u8; 100])
            .expect("seed legacy log");
        let sink = RotatingLog::open(&directory.0, policy()).expect("migrate legacy log");
        drop(sink);

        assert_eq!(
            std::fs::metadata(archive_path(&directory.0, 1))
                .expect("archived legacy log")
                .len(),
            policy().segment_bytes
        );
        assert_eq!(
            std::fs::metadata(directory.0.join(ACTIVE_LOG_NAME))
                .expect("new current log")
                .len(),
            0
        );
    }

    #[test]
    fn unverifiable_future_timestamp_is_purged() {
        let directory = TestDirectory::new();
        let path = directory.0.join(ACTIVE_LOG_NAME);
        std::fs::write(&path, b"privacy-sensitive diagnostics").expect("seed future log");
        let file = OpenOptions::new()
            .write(true)
            .open(&path)
            .expect("open future log");
        file.set_times(
            std::fs::FileTimes::new()
                .set_modified(SystemTime::now() + Duration::from_secs(60 * 60)),
        )
        .expect("set future timestamp");
        drop(file);

        let sink = RotatingLog::open(&directory.0, policy()).expect("open after purge");
        drop(sink);
        assert_eq!(std::fs::metadata(path).expect("new current log").len(), 0);
    }
}
