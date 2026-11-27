//! The text a person pastes into a bug report: what runs, on what, and the
//! recent log with everything private taken out. It is built when asked for,
//! shown and copied by the person, and goes nowhere on its own.

mod redact;

use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tauri::{Manager, WebviewWindow};
use zephium_app::Handle;
use zephium_ipc::{BlockerProtection, BlockerStatusView};

use crate::webext::WebExtensions;
use crate::{authorize, CallerPolicy};
use redact::Redactor;

const LOG: &str = "zephium.log";
const OLDER_LOG: &str = "zephium.1.log";
const CRASH: &str = "last-crash.txt";
const LOG_LINES: usize = 300;
/// A GitHub issue holds 65,536 characters in all, so the log leaves room for
/// the rest of the form.
const LOG_BYTES: usize = 40 * 1024;
const READ_BYTES: u64 = 192 * 1024;
const CRASH_LINES: usize = 24;
const CRASH_BYTES: u64 = 8 * 1024;
const CRASH_AGE: Duration = Duration::from_secs(30 * 24 * 60 * 60);
const QUERY_TIMEOUT: Duration = Duration::from_secs(2);
const SETTINGS: &[(&str, &str)] = &[
    ("language", "ui.language"),
    ("appearance", "appearance"),
    ("material", "ui.material"),
    ("tabs", "ui.tab-layout"),
    ("memory", "performance.memory"),
];
const FEATURES: &[(&str, &str)] = &[
    ("AI", "ai"),
    ("Work", "work"),
    ("Notes", "notes"),
    ("Tasks", "tasks"),
    ("Time", "time"),
];

/// The text for a bug report: version, system and the redacted recent log.
#[tauri::command]
#[specta::specta]
pub(crate) async fn diagnostics_report(
    caller: WebviewWindow,
    app: tauri::AppHandle,
) -> Option<String> {
    if !authorize(&caller, CallerPolicy::Main, "diagnostics_report") {
        return None;
    }
    let status = app.try_state::<Handle>()?.focused_content_policy_status();
    let window = caller.clone();
    let handle = app.clone();
    let (displays, protection, local, files, engine) = tokio::join!(
        bounded(move || window.available_monitors().ok().map(|monitors| {
            monitors
                .iter()
                .map(|monitor| {
                    (
                        monitor.size().width,
                        monitor.size().height,
                        monitor.scale_factor(),
                    )
                })
                .collect::<Vec<_>>()
        })),
        bounded(move || status.recv_timeout(QUERY_TIMEOUT)),
        bounded(move || local_facts(&handle)),
        bounded({
            let app = app.clone();
            move || recent_files(&app)
        }),
        bounded(engine),
    );
    let mut rows = vec![
        ("App", app_line(&app)),
        (
            "Channel",
            channel(
                &app.package_info().version.to_string(),
                &app.config().identifier,
            ),
        ),
    ];
    if let Some(uptime) = crate::diagnostics::uptime() {
        rows.push(("Uptime", uptime_text(uptime.as_secs())));
    }
    rows.push((
        "System",
        format!(
            "{} ({})",
            crate::about::os_name(),
            crate::about::arch_name(std::env::consts::ARCH)
        ),
    ));
    rows.push(("Web engine", engine.unwrap_or_else(|| "unavailable".into())));
    rows.extend(local.unwrap_or_default());
    rows.push((
        "Displays",
        displays.flatten().map_or_else(
            || "unavailable".into(),
            |monitors| describe_displays(&monitors),
        ),
    ));
    rows.push((
        "Ad blocking",
        protection.map_or_else(
            || "unavailable".into(),
            |status| describe_protection(&status),
        ),
    ));
    let (crash, log) = files.unwrap_or_default();
    Some(compose(&rows, crash, log))
}

async fn bounded<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> Option<T> {
    tokio::time::timeout(QUERY_TIMEOUT, tauri::async_runtime::spawn_blocking(work))
        .await
        .ok()?
        .ok()
}

/// What the store, the extension registry and the system answer; none of it
/// is private, and all of it is closed values.
fn local_facts(app: &tauri::AppHandle) -> Vec<(&'static str, String)> {
    let stored = |key: &str| crate::app_setting(key);
    let mut rows = Vec::new();
    if let Some(extensions) = app.try_state::<WebExtensions>() {
        let (enabled, total) = extensions.counts();
        rows.push((
            "Extensions",
            format!("{enabled} enabled, {total} installed"),
        ));
    }
    let features: Vec<String> = FEATURES
        .iter()
        .map(|(label, feature)| {
            let on = zephium_core::preferences::feature_enabled(stored, feature);
            format!("{label} {}", if on { "on" } else { "off" })
        })
        .collect();
    rows.push(("Features", features.join(", ")));
    let settings: Vec<String> = SETTINGS
        .iter()
        .map(|(label, key)| {
            let value = stored(key).map_or_else(|| "default".to_owned(), |value| fact(&value));
            format!("{label} {value}")
        })
        .collect();
    rows.push(("Settings", settings.join(", ")));
    rows
}

/// The recent crash report and log, read and cleaned.
fn recent_files(app: &tauri::AppHandle) -> (Vec<String>, Vec<String>) {
    let Some(dir) = crate::diagnostics::log_dir() else {
        return (Vec::new(), Vec::new());
    };
    let mut redactor = redactor(app);
    (
        crash_lines(dir, &mut redactor, SystemTime::now()),
        recent_lines(dir, &mut redactor),
    )
}

fn redactor(app: &tauri::AppHandle) -> Redactor {
    let mut homes: Vec<String> = ["HOME", "USERPROFILE"]
        .iter()
        .filter_map(std::env::var_os)
        .map(|home| home.to_string_lossy().into_owned())
        .collect();
    if let Ok(home) = app.path().home_dir() {
        homes.push(home.to_string_lossy().into_owned());
    }
    let user = ["USER", "USERNAME", "LOGNAME"]
        .iter()
        .find_map(|name| std::env::var(name).ok())
        .filter(|user| !user.is_empty());
    let mut redactor = Redactor::new(homes, user);
    if let Some(revision) = option_env!("ZEPHIUM_BUILD_REVISION") {
        redactor.keep(revision);
        redactor.keep(revision.split('+').next().unwrap_or(revision));
    }
    redactor
}

fn app_line(app: &tauri::AppHandle) -> String {
    let version = app.package_info().version.to_string();
    let revision = option_env!("ZEPHIUM_BUILD_REVISION").unwrap_or("unknown");
    format!("Zephium {version}, {}", build_label(revision))
}

/// The commit and whether the tree was clean: `882c92d8f1a2`, `…+dirty`.
fn build_label(revision: &str) -> String {
    let (hash, flag) = revision
        .split_once('+')
        .map_or((revision, ""), |(hash, flag)| (hash, flag));
    if hash.len() < 7 || !hash.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return "build unknown".into();
    }
    let short = &hash[..hash.len().min(12)];
    if flag.is_empty() {
        format!("build {short}")
    } else {
        format!("build {short}+{}", fact(flag))
    }
}

fn channel(version: &str, identifier: &str) -> String {
    if cfg!(debug_assertions) || identifier.ends_with(".dev") {
        return "development build".into();
    }
    match version.split_once('-') {
        Some((_, pre)) => format!("pre-release ({})", fact(pre)),
        None => "stable".into(),
    }
}

fn uptime_text(seconds: u64) -> String {
    match seconds {
        0..=59 => format!("{seconds} s"),
        60..=3599 => format!("{} min", seconds / 60),
        3600..=86_399 => format!("{} h {} min", seconds / 3600, seconds % 3600 / 60),
        _ => format!("{} d {} h", seconds / 86_400, seconds % 86_400 / 3600),
    }
}

fn describe_displays(monitors: &[(u32, u32, f64)]) -> String {
    if monitors.is_empty() {
        return "none reported".into();
    }
    let each: Vec<String> = monitors
        .iter()
        .map(|(width, height, scale)| {
            let scale = format!("{scale:.2}");
            let scale = scale.trim_end_matches('0').trim_end_matches('.');
            format!("{width}x{height} at {scale}x")
        })
        .collect();
    format!("{} ({})", monitors.len(), each.join(", "))
}

fn describe_protection(status: &BlockerStatusView) -> String {
    let state = match status.protection {
        BlockerProtection::Active => "active",
        BlockerProtection::Disabled => "off",
        BlockerProtection::Pending => "starting",
        BlockerProtection::Degraded => "degraded",
        BlockerProtection::Unavailable => return "unavailable".into(),
    };
    match status.failure {
        Some(failure) => format!("{state} (failure {failure:?})"),
        None => state.into(),
    }
}

#[cfg(target_os = "macos")]
fn engine() -> String {
    crate::platform::imp::webkit_version().unwrap_or_else(|| "WebKit, version unavailable".into())
}

#[cfg(target_os = "windows")]
fn engine() -> String {
    use zephium_core::webview2::WebView2Version;
    match tauri::webview_version() {
        Ok(reported) => match reported.parse::<WebView2Version>() {
            Ok(version) => {
                let channel = version
                    .channel()
                    .map_or_else(|| "stable".to_owned(), |channel| channel.to_string());
                let [major, minor, build, patch] = version.components();
                format!("WebView2 {major}.{minor}.{build}.{patch} (Evergreen, {channel})")
            }
            Err(_) => format!("WebView2 {}", fact(&reported)),
        },
        Err(_) => "WebView2, version unavailable".into(),
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn engine() -> String {
    tauri::webview_version().map_or_else(
        |_| "WebKitGTK, version unavailable".into(),
        |version| format!("WebKitGTK {}", fact(&version)),
    )
}

/// A short value from the system or the store, with nothing that could break
/// a line or run on.
fn fact(value: &str) -> String {
    value.chars().filter(|c| !c.is_control()).take(48).collect()
}

/// The newest lines of the log, from the one before it when this one is new.
fn recent_lines(dir: &Path, redactor: &mut Redactor) -> Vec<String> {
    let mut lines = tail_lines(&dir.join(LOG));
    if lines.len() < LOG_LINES {
        let mut older = tail_lines(&dir.join(OLDER_LOG));
        let keep = LOG_LINES - lines.len();
        older.drain(..older.len().saturating_sub(keep));
        older.append(&mut lines);
        lines = older;
    }
    lines.drain(..lines.len().saturating_sub(LOG_LINES));
    let mut cleaned: Vec<String> = lines.iter().map(|line| redactor.line(line)).collect();
    let mut bytes: usize = cleaned.iter().map(|line| line.len() + 1).sum();
    let mut drop = 0;
    while bytes > LOG_BYTES && drop < cleaned.len() {
        bytes -= cleaned[drop].len() + 1;
        drop += 1;
    }
    cleaned.drain(..drop);
    cleaned
}

/// The first lines of the last crash report, when it is recent.
fn crash_lines(dir: &Path, redactor: &mut Redactor, now: SystemTime) -> Vec<String> {
    let path = dir.join(CRASH);
    let Some(modified) = std::fs::symlink_metadata(&path)
        .ok()
        .filter(|metadata| metadata.is_file())
        .and_then(|metadata| metadata.modified().ok())
        .filter(|modified| {
            now.duration_since(*modified)
                .is_ok_and(|age| age <= CRASH_AGE)
        })
    else {
        return Vec::new();
    };
    let Some(text) = read_head(&path, CRASH_BYTES) else {
        return Vec::new();
    };
    let secs = modified
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs());
    std::iter::once(format!("Last crash, {}:", utc(secs)))
        .chain(
            text.lines()
                .filter(|line| !line.trim().is_empty())
                .take(CRASH_LINES)
                .map(|line| redactor.line(line)),
        )
        .collect()
}

fn read_head(path: &Path, limit: u64) -> Option<String> {
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .ok()?
        .take(limit)
        .read_to_end(&mut bytes)
        .ok()?;
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

/// The whole lines in the last stretch of a plain file; a link or a folder
/// by that name reads as empty.
fn tail_lines(path: &Path) -> Vec<String> {
    let Some(metadata) = std::fs::symlink_metadata(path)
        .ok()
        .filter(|metadata| metadata.is_file())
    else {
        return Vec::new();
    };
    let start = metadata.len().saturating_sub(READ_BYTES);
    let Ok(mut file) = std::fs::File::open(path) else {
        return Vec::new();
    };
    let mut bytes = Vec::new();
    if file.seek(SeekFrom::Start(start)).is_err()
        || file.take(READ_BYTES).read_to_end(&mut bytes).is_err()
    {
        return Vec::new();
    }
    let text = String::from_utf8_lossy(&bytes);
    let mut lines = text.lines();
    if start > 0 {
        lines.next();
    }
    lines
        .filter(|line| !line.trim().is_empty())
        .map(str::to_owned)
        .collect()
}

fn compose(rows: &[(&str, String)], crash: Vec<String>, log: Vec<String>) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs());
    let mut text = format!("Zephium diagnostics\nCreated: {}\n", utc(now));
    for (label, value) in rows {
        text.push_str(&format!("{label}: {value}\n"));
    }
    text.push_str(
        "Removed from the log: web addresses, searches, page titles, emails, keys and tokens, names, and where files are on your disk.\n",
    );
    if !crash.is_empty() {
        text.push('\n');
        text.push_str(&crash.join("\n"));
        text.push('\n');
    }
    text.push('\n');
    if log.is_empty() {
        text.push_str("Log: empty or unavailable.\n");
    } else {
        text.push_str(&format!(
            "Log, last {} lines (timestamps are Unix milliseconds):\n",
            log.len()
        ));
        text.push_str(&log.join("\n"));
        text.push('\n');
    }
    text
}

/// `2026-10-25 14:03:21 UTC`, from Unix seconds.
fn utc(seconds: u64) -> String {
    let days = (seconds / 86_400) as i64;
    let of_day = seconds % 86_400;
    // Howard Hinnant's civil-from-days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02}:{:02} UTC",
        of_day / 3600,
        of_day % 3600 / 60,
        of_day % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn redactor() -> Redactor {
        Redactor::new(["/Users/jane".to_owned()], Some("jane".into()))
    }

    #[test]
    fn dates_read_in_utc() {
        assert_eq!(utc(0), "1970-01-01 00:00:00 UTC");
        assert_eq!(utc(1_792_627_200), "2026-10-22 00:00:00 UTC");
        assert_eq!(utc(1_761_392_601), "2025-10-25 11:43:21 UTC");
        assert_eq!(utc(951_782_400 + 86_399), "2000-02-29 23:59:59 UTC");
    }

    #[test]
    fn the_build_names_its_commit_and_whether_it_was_clean() {
        let hash = "882c92d8f1a2b3c4d5e6f708192a3b4c5d6e7f80";
        assert_eq!(build_label(hash), "build 882c92d8f1a2");
        assert_eq!(
            build_label(&format!("{hash}+dirty")),
            "build 882c92d8f1a2+dirty"
        );
        assert_eq!(build_label("unknown"), "build unknown");
        assert_eq!(build_label("not a revision"), "build unknown");
    }

    #[test]
    fn uptime_and_displays_read_plainly() {
        assert_eq!(uptime_text(42), "42 s");
        assert_eq!(uptime_text(125), "2 min");
        assert_eq!(uptime_text(3 * 3600 + 12 * 60), "3 h 12 min");
        assert_eq!(uptime_text(2 * 86_400 + 5 * 3600), "2 d 5 h");
        assert_eq!(
            describe_displays(&[(3024, 1964, 2.0), (2560, 1440, 1.0), (1920, 1080, 1.25)]),
            "3 (3024x1964 at 2x, 2560x1440 at 1x, 1920x1080 at 1.25x)"
        );
        assert_eq!(describe_displays(&[]), "none reported");
    }

    #[test]
    fn the_log_tail_is_whole_lines_from_the_end() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(LOG);
        let line = "x".repeat(99);
        let count = (READ_BYTES as usize / 100) + 50;
        std::fs::write(
            &path,
            (0..count).map(|_| format!("{line}\n")).collect::<String>(),
        )
        .unwrap();
        let lines = tail_lines(&path);
        assert!(lines.len() < count);
        assert!(
            lines.iter().all(|line| line.len() == 99),
            "the cut first line is dropped"
        );
        assert!(tail_lines(&dir.path().join("missing.log")).is_empty());
    }

    #[test]
    fn a_new_log_continues_from_the_one_before_and_is_bounded() {
        let dir = tempfile::tempdir().unwrap();
        let older: String = (0..400).map(|n| format!("older {n}\n")).collect();
        let newer: String = (0..10).map(|n| format!("newer {n}\n")).collect();
        std::fs::write(dir.path().join(OLDER_LOG), older).unwrap();
        std::fs::write(dir.path().join(LOG), newer).unwrap();
        let lines = recent_lines(dir.path(), &mut redactor());
        assert_eq!(lines.len(), LOG_LINES);
        assert_eq!(lines.first().map(String::as_str), Some("older 110"));
        assert_eq!(lines.last().map(String::as_str), Some("newer 9"));

        let long = format!("{}\n", "word ".repeat(70));
        std::fs::write(dir.path().join(LOG), long.repeat(300)).unwrap();
        let lines = recent_lines(dir.path(), &mut redactor());
        let bytes: usize = lines.iter().map(|line| line.len() + 1).sum();
        assert!(bytes <= LOG_BYTES, "{bytes}");
        assert!(lines.len() < LOG_LINES, "the oldest lines go first");
    }

    #[test]
    fn a_linked_or_missing_log_reads_as_empty() {
        let dir = tempfile::tempdir().unwrap();
        let elsewhere = dir.path().join("elsewhere");
        std::fs::write(&elsewhere, "secret\n").unwrap();
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&elsewhere, dir.path().join(LOG)).unwrap();
            assert!(recent_lines(dir.path(), &mut redactor()).is_empty());
        }
        std::fs::create_dir(dir.path().join(OLDER_LOG)).unwrap();
        assert!(tail_lines(&dir.path().join(OLDER_LOG)).is_empty());
    }

    #[test]
    fn a_recent_crash_is_included_and_an_old_one_is_not() {
        let dir = tempfile::tempdir().unwrap();
        let report = "Zephium 1.0.3 stopped at desktop/src/lib.rs:1:2: bad value for https://example.com/a\nstack\n\nframe /Users/jane/x\n";
        std::fs::write(dir.path().join(CRASH), report).unwrap();
        let now = SystemTime::now();
        let lines = crash_lines(dir.path(), &mut redactor(), now);
        assert!(lines[0].starts_with("Last crash, 20"));
        assert_eq!(
            lines[1],
            "Zephium 1.0.3 stopped at desktop/src/lib.rs:1:2: bad value for <url>"
        );
        assert_eq!(lines.last().map(String::as_str), Some("frame <path>"));
        let later = now + CRASH_AGE + Duration::from_secs(60);
        assert!(crash_lines(dir.path(), &mut redactor(), later).is_empty());
    }

    #[test]
    fn the_report_reads_in_order_and_never_carries_what_the_log_held() {
        let dir = tempfile::tempdir().unwrap();
        let log = [
            "ts_ms=1792627200000 session=1792627200000-4242 startup: WebView2 runtime selected at launch: 156.0.4314.42",
            "ts_ms=1792627200100 session=1792627200000-4242 updates: error sending request for url (https://updates.example.org/v1/latest.json?token=abc123)",
            "ts_ms=1792627200200 session=1792627200000-4242 store: cannot open /Users/jane/Library/Application Support/app.zephium/zephium.db for jane@example.com",
            "ts_ms=1792627200300 session=1792627200000-4242 work: phase=lead title=\"My bank statement\" query=cats and dogs content=redacted",
            "ts_ms=1792627200400 session=1792627200000-4242 account: Authorization: Bearer eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0In0.c2lnbmF0dXJl",
            "ts_ms=1792627200500 session=1792627200000-4242 extensions: could not update abcdefghijklmnopabcdefghijklmnop: HTTP 500 from update.googleapis.com",
            "ts_ms=1792627200600 session=1792627200000-4242 provider: key sk-proj-AbCdEf0123456789AbCdEf0123456789 refused",
        ];
        std::fs::write(dir.path().join(LOG), log.join("\n") + "\n").unwrap();
        let lines = recent_lines(dir.path(), &mut redactor());
        let rows = vec![
            ("App", "Zephium 1.0.3, build 882c92d8f1a2".to_owned()),
            ("Channel", "stable".to_owned()),
            ("System", "macOS 26.1.0 (arm64)".to_owned()),
            ("Web engine", "WebKit 21622.2.11 (Safari 26.1)".to_owned()),
        ];
        let text = compose(&rows, Vec::new(), lines);

        assert!(text.starts_with("Zephium diagnostics\nCreated: 20"));
        assert!(text.contains("\nApp: Zephium 1.0.3, build 882c92d8f1a2\n"));
        assert!(text.contains("\nLog, last 7 lines"));
        assert!(text.contains("WebView2 runtime selected at launch: 156.0.4314.42"));
        assert!(text.contains("zephium.db for <email>"));
        for private in [
            "updates.example.org",
            "abc123",
            "jane",
            "My bank",
            "cats",
            "eyJhbGci",
            "abcdefghijklmnop",
            "update.googleapis.com",
            "sk-proj",
            "AbCdEf",
        ] {
            assert!(!text.contains(private), "{private} leaked:\n{text}");
        }
    }
}
