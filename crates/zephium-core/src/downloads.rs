//! Browser-owned download state and bounded native/store contracts.

use crate::ids::DownloadId;
use serde::{Deserialize, Serialize};
use specta::Type;
use std::path::PathBuf;

pub const MAX_ACTIVE_DOWNLOADS: usize = 8;
pub const MAX_DOWNLOAD_PAGE: u32 = 100;
pub const MAX_DOWNLOAD_HISTORY: usize = 10_000;
pub const MAX_DOWNLOAD_RECORD_BYTES: usize = 24 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum DownloadState {
    Pending,
    Receiving,
    // The retained native transfer can resume; never replay its request URL.
    Paused,
    // Fully received but of a type that can harm the computer. It stays in
    // its hidden staging folder until the person keeps or deletes it.
    Held,
    Cancelling,
    Finalizing,
    Completed,
    Cancelled,
    Interrupted,
    Failed,
}
impl DownloadState {
    pub fn terminal(self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Cancelled | Self::Interrupted | Self::Failed
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum DownloadError {
    Invalid,
    Unavailable,
    Unsupported,
    Capacity,
    Storage,
    Destination,
    // The system refused the destination folder: macOS privacy consent, a
    // denied ACL or Windows Controlled Folder Access.
    Permission,
    Network,
    ConnectionLost,
    Timeout,
    Authentication,
    Certificate,
    Server,
    Source,
    FileBusy,
    FileTooLarge,
    Integrity,
    Runtime,
    DiskFull,
    Protection,
    MissingFile,
    ChangedFile,
    Cancelled,
}

/// Only Rust's store/native boundary sees paths and on-disk identity.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DownloadRecord {
    pub id: DownloadId,
    pub session: DownloadId,
    pub revision: u32,
    pub created_at: i64,
    pub filename: String,
    /// An HTTP(S) origin, never a signed URL, credentials, query or blob token.
    pub source: String,
    /// For origin-less generated data, the displayed origin is page context.
    #[serde(default)]
    pub source_is_context: bool,
    pub state: DownloadState,
    pub received: u64,
    pub total: Option<u64>,
    pub error: Option<DownloadError>,
    pub destination: Option<PathBuf>,
    pub staging: Option<PathBuf>,
    pub staging_identity: Option<FileIdentity>,
    pub identity: Option<FileIdentity>,
    /// Native Windows writer incarnation, never exposed through UI IPC.
    #[serde(default)]
    pub writer: Option<DownloadWriter>,
    /// A native completion/cancellation or process-exit proof ended writes.
    #[serde(default)]
    pub writer_released: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DownloadWriter {
    pub process: u32,
    pub created: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileIdentity {
    pub volume: u64,
    pub file: u64,
    /// Upper half of a Windows 128-bit file ID; zero for legacy/Unix identities.
    #[serde(default)]
    pub file_high: u64,
    pub bytes: u64,
    #[serde(default)]
    pub modified: Option<(i64, i64)>,
}

impl DownloadRecord {
    pub fn validate(&self) -> bool {
        self.writer
            .as_ref()
            .is_none_or(|writer| writer.process != 0 && writer.created != 0)
            && self.revision > 0
            && self.created_at >= 0
            && !self.filename.is_empty()
            && self.filename.len() <= 240
            && valid_filename(&self.filename)
            && self.source.len() <= 1024
            && crate::permissions::PageOrigin::parse_exact(&self.source).is_ok()
            && [&self.destination, &self.staging].into_iter().all(|path| {
                path.as_ref()
                    .is_none_or(|path| path.is_absolute() && path.as_os_str().len() <= 4096)
            })
            && (self.state != DownloadState::Completed
                || (self.destination.is_some() && self.identity.is_some()))
    }

    /// A native progress getter may pump a terminal callback. Recheck state
    /// after that call so stale samples cannot invalidate a terminal Store ack.
    pub fn update_progress(&mut self, received: u64, total: Option<u64>) -> bool {
        if self.state != DownloadState::Receiving
            || (self.received == received && total.is_none_or(|value| self.total == Some(value)))
        {
            return false;
        }
        let Some(next) = self.revision.checked_add(1) else {
            return false;
        };
        self.revision = next;
        self.received = received;
        if total.is_some() {
            self.total = total;
        }
        true
    }

    pub fn view(&self) -> DownloadView {
        DownloadView {
            id: self.id.to_string(),
            revision: format!("{:08x}", self.revision),
            created_at: self.created_at.to_string(),
            filename: self.filename.clone(),
            source: self.source.clone(),
            source_is_context: self.source_is_context,
            state: self.state,
            received: self.received.to_string(),
            total: self.total.map(|value| value.to_string()),
            error: self.error,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, Type)]
pub struct DownloadView {
    pub id: String,
    pub revision: String,
    pub created_at: String,
    pub filename: String,
    pub source: String,
    /// For origin-less generated data, the displayed origin is page context.
    #[serde(default)]
    pub source_is_context: bool,
    pub state: DownloadState,
    pub received: String,
    pub total: Option<String>,
    pub error: Option<DownloadError>,
}

/// The default saves straight to the system Downloads folder, as other
/// browsers do; asking first is a choice people opt into.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(deny_unknown_fields)]
pub struct DownloadPreferences {
    pub ask_destination: bool,
    /// Set only by the native directory picker, never accepted from page IPC.
    pub directory: Option<String>,
    /// Read-only native directory identity; only the OS picker can mint it.
    #[serde(default)]
    pub directory_identity: Option<String>,
}
impl DownloadPreferences {
    pub fn validate(&self) -> bool {
        self.directory_identity.as_ref().is_none_or(|value| {
            matches!(value.len(), 33 | 49)
                && value.as_bytes()[16] == b':'
                && value
                    .bytes()
                    .enumerate()
                    .all(|(index, byte)| index == 16 || byte.is_ascii_hexdigit())
        }) && self.directory.as_ref().is_none_or(|value| {
            !value.contains('\0')
                && value.len() <= 4096
                && std::path::Path::new(value).is_absolute()
        })
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, Type)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum DownloadCall {
    Updates,
    RetryCleanup,
    List {
        before: Option<String>,
        limit: u32,
    },
    Cancel {
        id: String,
    },
    /// Stops receiving and keeps what was received for `Resume`.
    Pause {
        id: String,
    },
    Resume {
        id: String,
    },
    /// Publishes a finished download that was held for its type.
    Keep {
        id: String,
    },
    /// Deletes a finished download that was held for its type.
    Discard {
        id: String,
    },
    Open {
        id: String,
    },
    Reveal {
        id: String,
    },
    Forget {
        id: String,
    },
    // Forgets every finished download; files on disk are untouched.
    Clear,
    // Forgets the finished downloads started at or after `since` (unix
    // seconds). Only the shell sends it; a frame cannot.
    #[serde(skip)]
    ClearSince {
        since: i64,
    },
    Preferences,
    ChooseDirectory,
    SetAskDestination {
        enabled: bool,
    },
}
impl DownloadCall {
    pub fn validate(&self) -> bool {
        match self {
            Self::List { before, limit } => {
                *limit > 0
                    && *limit <= MAX_DOWNLOAD_PAGE
                    && before.as_ref().is_none_or(|id| canonical_id(id))
            }
            Self::Cancel { id }
            | Self::Pause { id }
            | Self::Resume { id }
            | Self::Keep { id }
            | Self::Discard { id }
            | Self::Open { id }
            | Self::Reveal { id }
            | Self::Forget { id } => canonical_id(id),
            Self::ClearSince { since } => *since > 0,
            _ => true,
        }
    }
}
/// Types an operating system runs, installs or opens as a program, or that
/// carry macros or scripts a click can start: the ones other browsers also
/// ask about before keeping. Archives and documents are not here; the system
/// checks what they hold when they are opened.
const DANGEROUS_EXTENSIONS: &[&str] = &[
    // Windows programs, installers and shortcuts
    "exe",
    "msi",
    "msp",
    "mst",
    "com",
    "scr",
    "pif",
    "cpl",
    "dll",
    "sys",
    "drv",
    "ocx",
    "lnk",
    "url",
    "scf",
    "gadget",
    "application",
    "appref-ms",
    "appx",
    "appxbundle",
    "msix",
    "msixbundle",
    "msu",
    "xll",
    "wll",
    "xnk",
    "ade",
    "adp",
    "mde",
    "mdb",
    "mda",
    "mdz",
    "mdt",
    "mdw",
    // Scripts and shells
    "bat",
    "cmd",
    "ps1",
    "ps1xml",
    "ps2",
    "ps2xml",
    "psc1",
    "psc2",
    "psm1",
    "psd1",
    "msh",
    "msh1",
    "msh2",
    "mshxml",
    "msh1xml",
    "msh2xml",
    "vb",
    "vbe",
    "vbs",
    "vbscript",
    "jse",
    "ws",
    "wsc",
    "wsf",
    "wsh",
    "hta",
    "inf",
    "ins",
    "isp",
    "reg",
    "sct",
    "shb",
    "shs",
    "pcd",
    "plg",
    "prf",
    "prg",
    "ops",
    "sh",
    "bash",
    "zsh",
    "csh",
    "ksh",
    "command",
    "tool",
    "terminal",
    "scpt",
    "scptd",
    "workflow",
    "action",
    // Java, browser and office carriers
    "jar",
    "jnlp",
    "class",
    "xbap",
    "crx",
    "vsmacros",
    "vss",
    "vst",
    "vsw",
    "docm",
    "dotm",
    "xlsm",
    "xltm",
    "xlam",
    "pptm",
    "potm",
    "ppam",
    "ppsm",
    "sldm",
    // macOS and Linux packages and applications
    "app",
    "dmg",
    "pkg",
    "mpkg",
    "iso",
    "apk",
    "deb",
    "rpm",
    "appimage",
    "run",
    "bin",
    "mobileconfig",
];

/// Script types Windows runs from a double click, which other platforms open
/// in an editor; a developer's `.js` or `.py` on a Mac is not a warning.
const WINDOWS_SCRIPT_EXTENSIONS: &[&str] = &["js", "py", "pyw", "pyc", "pyo", "rb", "rbw", "pl"];

/// Whether a download of this name must be confirmed before it is kept. The
/// last extension decides, after the trailing dots and spaces Windows ignores
/// are dropped, so `report.pdf.exe` and `setup.exe.` are caught and
/// `exe.txt` is not.
pub fn is_dangerous_file(name: &str) -> bool {
    is_dangerous_file_on(name, cfg!(target_os = "windows"))
}

pub fn is_dangerous_file_on(name: &str, windows: bool) -> bool {
    let name = name.trim_end_matches(['.', ' ']);
    name.rsplit_once('.').is_some_and(|(stem, extension)| {
        let known = |list: &[&str]| {
            list.iter()
                .any(|known| known.eq_ignore_ascii_case(extension))
        };
        !stem.is_empty()
            && !extension.is_empty()
            && (known(DANGEROUS_EXTENSIONS) || (windows && known(WINDOWS_SCRIPT_EXTENSIONS)))
    })
}

pub fn canonical_id(id: &str) -> bool {
    DownloadId::parse(id).is_some_and(|value| value.to_string() == id)
}

#[derive(Clone, Debug, Serialize, Deserialize, Type)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DownloadResponse {
    Updates {
        entries: Vec<DownloadView>,
        removed: Vec<String>,
        cleanup: DownloadCleanup,
    },
    Page {
        entries: Vec<DownloadView>,
        next: Option<String>,
        supported: bool,
        cleanup: DownloadCleanup,
    },
    Preferences {
        preferences: DownloadPreferences,
        supported: bool,
        site_downloads_require_confirmation: bool,
    },
    Accepted,
    Applied,
    Error {
        error: DownloadError,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize, Type)]
pub struct DownloadCleanup {
    pub running: bool,
    pub error: Option<DownloadError>,
}

/// Reply fallback makes queue rejection/teardown observable rather than hanging
/// a frontend promise indefinitely. Clones share one exactly-once response.
type CompletionFn = Box<dyn FnOnce(DownloadResponse) + Send>;
struct CompletionInner(std::sync::Mutex<Option<CompletionFn>>);
#[derive(Clone)]
pub struct DownloadCompletion(std::sync::Arc<CompletionInner>);
impl DownloadCompletion {
    pub fn new(done: impl FnOnce(DownloadResponse) + Send + 'static) -> Self {
        Self(std::sync::Arc::new(CompletionInner(std::sync::Mutex::new(
            Some(Box::new(done)),
        ))))
    }
    pub fn finish(self, response: DownloadResponse) {
        let done = self
            .0
             .0
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take();
        if let Some(done) = done {
            done(response);
        }
    }
}
impl std::fmt::Debug for DownloadCompletion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("DownloadCompletion")
    }
}
impl Drop for CompletionInner {
    fn drop(&mut self) {
        if let Some(done) = self
            .0
            .get_mut()
            .unwrap_or_else(|error| error.into_inner())
            .take()
        {
            done(DownloadResponse::Error {
                error: DownloadError::Unavailable,
            });
        }
    }
}

#[derive(Clone, Debug)]
pub enum DownloadStoreCall {
    Save(Box<DownloadRecord>),
    List {
        before: Option<DownloadId>,
        limit: u32,
        session: DownloadId,
        active: Vec<DownloadId>,
    },
    /// Cleanup ownership is independent of visible history retention.
    RecoveryPage {
        after: Option<DownloadId>,
        limit: u32,
        session: DownloadId,
        active: Vec<DownloadId>,
    },
    Get(DownloadId),
    Forget(DownloadId),
    /// Deletes every terminal record. Transfers still writing keep theirs.
    Clear,
    /// Deletes the terminal records created at or after the unix second.
    ClearSince(i64),
    ClearStaging {
        id: DownloadId,
        expected: FileIdentity,
    },
    Preferences,
    SetPreferences(DownloadPreferenceChange),
}
#[derive(Clone, Debug)]
pub enum DownloadPreferenceChange {
    AskDestination(bool),
    Directory { path: String, identity: String },
}

#[derive(Clone, Debug)]
pub enum DownloadStoreReply {
    Saved,
    Profiles(Vec<crate::ids::ProfileId>),
    Page(Vec<DownloadRecord>),
    Record(Option<Box<DownloadRecord>>),
    Preferences(DownloadPreferences),
    Error(DownloadError),
}

/// Portable suggestion only; destination authority always comes from the user
/// or a previously selected download directory. Never trust a server path.
pub fn safe_filename(suggested: &str) -> String {
    sanitize_filename(suggested, 220)
}

pub fn valid_filename(value: &str) -> bool {
    !value.is_empty() && value.len() <= 240 && sanitize_filename(value, 240) == value
}

/// Keep collision suffixes inside the portable filename budget, preserving a
/// normal extension and complete UTF-8 characters even for user-entered names.
pub fn collision_filename(name: &str, index: usize) -> String {
    if index == 0 {
        return name.to_owned();
    }
    let path = std::path::Path::new(name);
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .filter(|value| value.len() <= 32)
        .map(|value| format!(".{value}"))
        .unwrap_or_default();
    let stem = if extension.is_empty() {
        name
    } else {
        &name[..name.len() - extension.len()]
    };
    let suffix = format!(" ({index}){extension}");
    let budget = 240 - suffix.len();
    let mut end = stem.len().min(budget);
    while !stem.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}{suffix}", &stem[..end])
}

fn sanitize_filename(suggested: &str, limit: usize) -> String {
    let leaf = suggested.rsplit(['/', '\\']).next().unwrap_or_default();
    let mut name = String::new();
    for ch in leaf.chars() {
        if name.len() + ch.len_utf8() > limit {
            break;
        }
        if ch.is_control()
            || matches!(ch, ':' | '*' | '?' | '"' | '<' | '>' | '|' | '\u{061c}' | '\u{200e}' | '\u{200f}' | '\u{2028}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
        {
            name.push('_');
        } else {
            name.push(ch);
        }
    }
    let name = name.trim_matches([' ', '.']);
    if name.is_empty() {
        return "download".into();
    }
    let stem = name
        .split('.')
        .next()
        .unwrap_or_default()
        .trim_end_matches([' ', '.'])
        .to_ascii_uppercase();
    let device_number = stem
        .strip_prefix("COM")
        .or_else(|| stem.strip_prefix("LPT"));
    let reserved = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL" | "CLOCK$")
        || device_number.is_some_and(|number| {
            matches!(
                number,
                "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
            )
        });
    if reserved {
        format!("_{name}")
    } else {
        name.into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn filenames_cannot_carry_paths_devices_streams_or_bidi_controls() {
        for (input, expected) in [
            ("../../evil.txt", "evil.txt"),
            ("C:\\tmp\\CON.txt", "_CON.txt"),
            ("file:secret", "file_secret"),
            (" .. ", "download"),
            ("report\u{202e}exe.txt", "report_exe.txt"),
            ("LPT9.", "_LPT9"),
            ("COM¹.txt", "_COM¹.txt"),
            ("CON .txt", "_CON .txt"),
        ] {
            assert_eq!(safe_filename(input), expected);
        }
        let bounded = safe_filename(&"é".repeat(1000));
        assert!(bounded.len() <= 220);
        assert_eq!(safe_filename(&bounded), bounded);
    }
    #[test]
    fn collision_suffixes_fit_portable_names_without_splitting_unicode() {
        for name in [
            format!("{}.txt", "a".repeat(236)),
            format!("{}.txt", "é".repeat(118)),
            "a".repeat(240),
        ] {
            for index in [1, 9999] {
                let value = collision_filename(&name, index);
                assert!(valid_filename(&value), "{value}");
                assert!(value.contains(&format!(" ({index})")));
                if name.ends_with(".txt") {
                    assert!(value.ends_with(".txt"));
                }
            }
        }
    }
    #[test]
    fn terminal_progress_samples_cannot_invalidate_persistence_acknowledgements() {
        let mut record = DownloadRecord {
            id: DownloadId::generate(),
            session: DownloadId::generate(),
            revision: 1,
            created_at: 1,
            filename: "report.txt".into(),
            source: "https://example.com".into(),
            source_is_context: false,
            state: DownloadState::Receiving,
            received: 0,
            total: None,
            error: None,
            destination: None,
            staging: None,
            staging_identity: None,
            identity: None,
            writer: None,
            writer_released: false,
        };
        assert!(record.update_progress(5, Some(10)));
        assert!(!record.update_progress(5, None));
        for state in [
            DownloadState::Cancelling,
            DownloadState::Finalizing,
            DownloadState::Completed,
            DownloadState::Cancelled,
            DownloadState::Failed,
        ] {
            record.state = state;
            let revision = record.revision;
            assert!(!record.update_progress(10, Some(10)));
            assert_eq!(record.revision, revision);
            assert_eq!(record.received, 5);
        }
    }
    #[test]
    fn ipc_rejects_noncanonical_ids_and_unbounded_pages() {
        assert!(!DownloadCall::Cancel {
            id: "../../file".into()
        }
        .validate());
        assert!(!DownloadCall::List {
            before: None,
            limit: 101
        }
        .validate());
        assert!(DownloadCall::List {
            before: None,
            limit: 100
        }
        .validate());
    }
    #[test]
    fn pause_keep_and_discard_name_a_canonical_download_like_cancel() {
        let id = DownloadId::generate().to_string();
        for call in [
            DownloadCall::Pause { id: id.clone() },
            DownloadCall::Keep { id: id.clone() },
            DownloadCall::Discard { id: id.clone() },
        ] {
            assert!(call.validate());
        }
        for bad in ["../x", "", "not-an-id"] {
            assert!(!DownloadCall::Pause { id: bad.into() }.validate());
            assert!(!DownloadCall::Keep { id: bad.into() }.validate());
            assert!(!DownloadCall::Discard { id: bad.into() }.validate());
        }
        assert!(!DownloadState::Held.terminal());
    }
    #[test]
    fn programs_installers_and_scripts_are_held_and_documents_are_not() {
        for name in [
            "setup.exe",
            "Setup.EXE",
            "installer.msi",
            "Zephium.dmg",
            "tool.pkg",
            "run.bat",
            "run.cmd",
            "profile.ps1",
            "screensaver.scr",
            "app.jar",
            "archive.app",
            "script.vbs",
            "macro.docm",
            "tool.sh",
            "link.url",
            "drive.iso",
            "a.b.exe",
        ] {
            assert!(is_dangerous_file(name), "{name}");
        }
        for name in [
            "report.pdf",
            "photo.jpg",
            "notes.txt",
            "bundle.zip",
            "data.csv",
            "exe.txt",
            "exe",
            ".exe",
            "download",
            "page.html",
            "movie.mp4",
            "README",
        ] {
            assert!(!is_dangerous_file(name), "{name}");
        }
    }
    #[test]
    fn scripts_that_only_windows_runs_on_a_double_click_are_held_only_there() {
        for name in ["build.js", "tool.py", "gem.rb", "x.pl"] {
            assert!(is_dangerous_file_on(name, true), "{name}");
            assert!(!is_dangerous_file_on(name, false), "{name}");
        }
        // Programs are held everywhere.
        assert!(is_dangerous_file_on("setup.exe", false));
        assert!(is_dangerous_file_on("setup.exe", true));
    }
    #[test]
    fn a_disguised_extension_is_judged_by_what_the_system_will_run() {
        // The last extension decides, however the name is dressed.
        assert!(is_dangerous_file("invoice.pdf.exe"));
        assert!(is_dangerous_file("setup.exe."));
        assert!(is_dangerous_file("setup.exe  "));
        assert!(is_dangerous_file("setup.exe. . "));
        assert!(!is_dangerous_file("setup.exe.txt"));
        assert!(!is_dangerous_file("setup.exe.pdf"));
        // A name that sanitizing leaves is still judged by its extension.
        assert!(is_dangerous_file(&safe_filename("..\\..\\evil.EXE")));
    }
    #[test]
    fn abandoned_native_calls_complete_with_unavailable_once() {
        let (send, recv) = std::sync::mpsc::channel();
        drop(DownloadCompletion::new(move |result| {
            send.send(result).unwrap();
        }));
        assert!(matches!(
            recv.recv().unwrap(),
            DownloadResponse::Error {
                error: DownloadError::Unavailable
            }
        ));
        assert!(recv.recv().is_err());
    }
}
