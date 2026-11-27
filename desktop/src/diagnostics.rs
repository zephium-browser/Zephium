//! Local diagnostics: where the log lives, a crash report written before the
//! process aborts, and showing that folder to the person. Nothing here sends
//! anything anywhere; a report leaves the device only if someone attaches it.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

const CRASH_REPORT: &str = "last-crash.txt";

static LOG_DIR: OnceLock<PathBuf> = OnceLock::new();
static STARTED: OnceLock<Instant> = OnceLock::new();

/// Records the log folder and installs the crash hook. With `panic = "abort"`
/// the default hook's message goes to a stderr nobody may be reading, or into
/// a pipe that never drains before the abort; this one also writes a small
/// file synchronously.
pub(crate) fn install(dir: PathBuf) {
    if LOG_DIR.set(dir).is_err() {
        return;
    }
    let _ = STARTED.set(Instant::now());
    std::panic::set_hook(Box::new(|info| {
        let location = info
            .location()
            .map(|at| format!("{}:{}:{}", at.file(), at.line(), at.column()))
            .unwrap_or_else(|| "an unknown place".to_owned());
        let payload = info.payload();
        let message = payload
            .downcast_ref::<&str>()
            .copied()
            .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
            .unwrap_or("a non-text panic");
        let report = format!(
            "Zephium {} stopped at {location}: {message}\n{}\n",
            env!("CARGO_PKG_VERSION"),
            std::backtrace::Backtrace::force_capture()
        );
        let _ = std::io::Write::write_all(&mut std::io::stderr(), report.as_bytes());
        if let Some(dir) = LOG_DIR.get() {
            let _ = write_private(&dir.join(CRASH_REPORT), report.as_bytes());
        }
    }));
}

/// The folder holding the log and the crash report, once the log is set up.
pub(crate) fn log_dir() -> Option<&'static Path> {
    LOG_DIR.get().map(PathBuf::as_path)
}

/// How long this run has been going, once the log is set up.
pub(crate) fn uptime() -> Option<Duration> {
    STARTED.get().map(Instant::elapsed)
}

fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let parent = path
        .parent()
        .ok_or_else(|| std::io::Error::other("crash report has no parent directory"))?;
    let parent_metadata = std::fs::symlink_metadata(parent)?;
    if !crate::data_root::plain_directory_metadata(&parent_metadata) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "crash report directory is not a plain directory",
        ));
    }
    let mut options = std::fs::OpenOptions::new();
    options.create(true).write(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc_nofollow());
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.custom_flags(0x0020_0000);
    }
    let mut file = options.open(path)?;
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if file.metadata()?.file_attributes() & 0x400 != 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "crash report is a reparse point",
            ));
        }
        let current_parent = std::fs::symlink_metadata(parent)?;
        if !crate::data_root::plain_directory_metadata(&current_parent) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "crash report directory is not a plain directory",
            ));
        }
    }
    file.write_all(bytes)
}

#[cfg(unix)]
fn libc_nofollow() -> i32 {
    #[cfg(target_os = "macos")]
    {
        libc::O_NOFOLLOW
    }
    #[cfg(not(target_os = "macos"))]
    {
        0
    }
}

/// Opens the log folder in Finder or Explorer.
pub(crate) fn reveal() -> bool {
    let Some(dir) = LOG_DIR.get() else {
        return false;
    };
    #[cfg(target_os = "macos")]
    {
        use objc2_app_kit::NSWorkspace;
        use objc2_foundation::{NSString, NSURL};
        let Some(path) = dir.to_str() else {
            return false;
        };
        let url = NSURL::fileURLWithPath(&NSString::from_str(path));
        NSWorkspace::sharedWorkspace().openURL(&url)
    }
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows::core::PCWSTR;
        use windows::Win32::UI::Shell::ShellExecuteW;
        use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
        let wide: Vec<u16> = dir.as_os_str().encode_wide().chain([0]).collect();
        // SAFETY: a NUL-terminated folder path for the plain "open" verb;
        // ShellExecute reports success as any value above 32.
        let result = unsafe {
            ShellExecuteW(
                None,
                windows::core::w!("open"),
                PCWSTR(wide.as_ptr()),
                None,
                None,
                SW_SHOWNORMAL,
            )
        };
        result.0 as isize > 32
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = dir;
        false
    }
}
