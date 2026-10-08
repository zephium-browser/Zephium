//! Updates from GitHub Releases, signed with Zephium's updater key.
//!
//! A check downloads a newer release in the background and parks it on disk.
//! It installs when the person relaunches, or on macOS when they quit, always
//! after the ordinary orderly shutdown has saved the session.

use super::*;

mod artifact;
#[cfg(any(target_os = "windows", test))]
mod recovery;

use artifact::Artifact;
use std::sync::Mutex;
use std::time::Duration;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, specta::Type)]
#[serde(tag = "state", rename_all = "camelCase")]
pub(crate) enum UpdateStatus {
    // Development and unsupported builds never update themselves.
    Unavailable,
    Idle,
    Checking,
    UpToDate,
    Downloading,
    Ready {
        version: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        retry_reason: Option<String>,
    },
    ManualInstall {
        version: String,
    },
    Installing,
    Failed,
}

struct Parked {
    update: tauri_plugin_updater::Update,
    artifact: Artifact,
}

#[derive(Default)]
pub(crate) struct Updates {
    status: Mutex<Option<UpdateStatus>>,
    parked: Mutex<Option<Parked>>,
    staged: AtomicBool,
    #[cfg(target_os = "windows")]
    recovery_path: Mutex<Option<std::path::PathBuf>>,
}

#[cfg(target_os = "macos")]
static RELAUNCH_AFTER_EXIT: AtomicBool = AtomicBool::new(false);
#[cfg(target_os = "windows")]
static INSTALL_AFTER_EXIT: Mutex<Option<(Parked, std::path::PathBuf)>> = Mutex::new(None);

const SUPPORTED: bool = !cfg!(debug_assertions) && cfg!(any(target_os = "macos", windows));
/// The release's highlights, kept from its download for the first launch of it.
const HIGHLIGHTS: &str = "updates.highlights";
const MAX_HIGHLIGHTS: usize = 5;
const MAX_HIGHLIGHT_CHARS: usize = 120;
/// Native's own schedule backs up the window's: a build whose window cannot
/// run its script must still be able to update itself.
const NATIVE_FIRST_CHECK: Duration = Duration::from_secs(5 * 60);
const NATIVE_CHECK_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);

impl Updates {
    fn status(&self) -> UpdateStatus {
        if !SUPPORTED {
            return UpdateStatus::Unavailable;
        }
        lock(&self.status).clone().unwrap_or(UpdateStatus::Idle)
    }

    fn begin_check(&self, supported: bool) -> Result<(), UpdateStatus> {
        let mut state = lock(&self.status);
        let current = state.clone().unwrap_or(UpdateStatus::Idle);
        if !supported {
            return Err(UpdateStatus::Unavailable);
        }
        match current {
            UpdateStatus::Idle
            | UpdateStatus::UpToDate
            | UpdateStatus::Failed
            | UpdateStatus::ManualInstall { .. } => {
                *state = Some(UpdateStatus::Checking);
                Ok(())
            }
            other => Err(other),
        }
    }

    fn set(&self, status: UpdateStatus) {
        *lock(&self.status) = Some(status);
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[tauri::command]
#[specta::specta]
pub(crate) fn update_status(
    caller: WebviewWindow,
    updates: tauri::State<'_, Updates>,
) -> Option<UpdateStatus> {
    authorize(&caller, CallerPolicy::Main, "update_status").then(|| updates.status())
}

/// Checks for a newer release and downloads it. Returns the resulting status;
/// a check already in flight, or an update already waiting, is reported as is.
#[tauri::command]
#[specta::specta]
pub(crate) async fn update_check(
    caller: WebviewWindow,
    app: tauri::AppHandle,
) -> Option<UpdateStatus> {
    if !authorize(&caller, CallerPolicy::Main, "update_check") {
        return None;
    }
    Some(run_check(&app).await)
}

async fn run_check(app: &tauri::AppHandle) -> UpdateStatus {
    let updates = app.state::<Updates>();
    if let Err(status) = updates.begin_check(SUPPORTED) {
        return status;
    }
    let next = match check_and_download(app, &updates).await {
        Ok(status) => status,
        Err(error) => {
            write_diagnostic(format_args!("updates: {error}"));
            UpdateStatus::Failed
        }
    };
    updates.set(next.clone());
    next
}

/// Checks on native's own schedule, unless the person turned automatic
/// checks off, and tells the window what changed.
pub(crate) fn schedule_native_checks(app: &tauri::AppHandle) {
    if !SUPPORTED {
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(NATIVE_FIRST_CHECK).await;
        loop {
            let wanted = APP_STORE
                .get()
                .and_then(|store| store.app_setting("updates.auto-check"))
                .is_none_or(|value| value != "false");
            if wanted && !shutdown_started(&app) {
                let _ = run_check(&app).await;
                emit_ui_command(&app, "updates.changed");
            }
            tokio::time::sleep(NATIVE_CHECK_INTERVAL).await;
        }
    });
}

#[derive(Clone, Debug, Serialize, specta::Type)]
pub(crate) struct UpdateHighlights {
    version: String,
    items: Vec<String>,
}

/// What the release now running brought, when it arrived as an update.
#[tauri::command]
#[specta::specta]
pub(crate) fn update_highlights(
    caller: WebviewWindow,
    app: tauri::AppHandle,
) -> Option<UpdateHighlights> {
    if !authorize(&caller, CallerPolicy::Main, "update_highlights") {
        return None;
    }
    let stored = APP_STORE.get()?.app_setting(HIGHLIGHTS)?;
    let stored: serde_json::Value = serde_json::from_str(&stored).ok()?;
    let version = stored.get("version")?.as_str()?;
    if version != app.package_info().version.to_string() {
        return None;
    }
    let items = stored
        .get("items")?
        .as_array()?
        .iter()
        .filter_map(serde_json::Value::as_str)
        .take(MAX_HIGHLIGHTS)
        .map(str::to_owned)
        .collect();
    Some(UpdateHighlights {
        version: version.to_owned(),
        items,
    })
}

fn remember_highlights(version: &str, notes: Option<&str>) {
    let value = serde_json::json!({
        "version": version,
        "items": highlights(notes.unwrap_or_default()),
    });
    if let Some(store) = APP_STORE.get() {
        let _ = store.set_app_setting(HIGHLIGHTS.into(), value.to_string());
    }
}

/// A release's bullet points as plain words: Markdown emphasis, links,
/// credits and pull-request references removed, at most five, each short.
fn highlights(notes: &str) -> Vec<String> {
    notes
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            line.strip_prefix("- ")
                .or_else(|| line.strip_prefix("* "))
                .or_else(|| line.strip_prefix("• "))
        })
        .map(plain_highlight)
        .filter(|item| !item.is_empty() && !item.to_ascii_lowercase().starts_with("full changelog"))
        .take(MAX_HIGHLIGHTS)
        .collect()
}

fn plain_highlight(item: &str) -> String {
    // GitHub's generated notes end each line with " by @someone in <url>".
    let item = item.split(" by @").next().unwrap_or(item);
    let mut text = String::new();
    let mut rest = item;
    while let Some(open) = rest.find('[') {
        let (before, after) = rest.split_at(open);
        text.push_str(before);
        match after.find("](").zip(after.find(')')) {
            Some((label_end, link_end)) if label_end < link_end => {
                text.push_str(&after[1..label_end]);
                rest = &after[link_end + 1..];
            }
            _ => {
                text.push_str(after);
                rest = "";
            }
        }
    }
    text.push_str(rest);
    let text: String = text
        .replace("**", "")
        .replace("__", "")
        .replace('`', "")
        .split_whitespace()
        .filter(|word| !word.starts_with("http://") && !word.starts_with("https://"))
        .collect::<Vec<_>>()
        .join(" ");
    let text = text
        .trim_end_matches(|c: char| c == '.' || c.is_whitespace())
        .to_owned();
    let mut chars = text.chars();
    let mut short: String = chars.by_ref().take(MAX_HIGHLIGHT_CHARS).collect();
    if chars.next().is_some() {
        short.push('…');
    }
    short
}

async fn check_and_download(
    app: &tauri::AppHandle,
    updates: &Updates,
) -> Result<UpdateStatus, String> {
    use tauri_plugin_updater::UpdaterExt;

    // Without these a stalled connection would hold the status at Checking or
    // Downloading for the rest of the session and block every later check.
    let updater = app
        .updater_builder()
        .timeout(Duration::from_secs(30))
        .configure_client(|client| {
            client
                .https_only(true)
                .redirect(reqwest::redirect::Policy::limited(5))
                .connect_timeout(Duration::from_secs(15))
                .read_timeout(Duration::from_secs(60))
        })
        .build()
        .map_err(|error| error.to_string())?;
    let Some(update) = updater.check().await.map_err(|error| error.to_string())? else {
        return Ok(UpdateStatus::UpToDate);
    };
    updates.set(UpdateStatus::Downloading);
    let key = app
        .config()
        .plugins
        .0
        .get("updater")
        .and_then(|config| config.get("pubkey"))
        .and_then(serde_json::Value::as_str)
        .ok_or("update verification key is unavailable")?
        .to_owned();
    // Retained metadata is not authority. Only reuse bytes after a fresh HTTPS
    // release check returned the exact same version, target, URL and signature.
    #[cfg(target_os = "windows")]
    let recovered = {
        let path = lock(&updates.recovery_path).clone();
        let metadata = update.raw_json.clone();
        let signature = update.signature.clone();
        let key = key.clone();
        tauri::async_runtime::spawn_blocking(move || {
            let path = path?;
            let bytes = recovery::load_matching(&path, &metadata).ok()??;
            Artifact::from_bytes(&bytes, &key, &signature).ok()
        })
        .await
        .map_err(|_| "retained update verification did not complete")?
    };
    #[cfg(not(target_os = "windows"))]
    let recovered: Option<Artifact> = None;
    let retry_reason = recovered.as_ref().map(|_| {
        "The previous update did not finish. Your downloaded update is still ready to install."
            .to_owned()
    });
    let artifact = match recovered {
        Some(artifact) => artifact,
        None => artifact::download(&update.download_url, key, update.signature.clone()).await?,
    };
    let version = update.version.clone();
    remember_highlights(&version, update.body.as_deref());
    *lock(&updates.parked) = Some(Parked { update, artifact });
    Ok(UpdateStatus::Ready {
        version,
        retry_reason,
    })
}

/// Installs the parked update and relaunches through the orderly shutdown.
#[tauri::command]
#[specta::specta]
pub(crate) fn update_relaunch(caller: WebviewWindow, app: tauri::AppHandle) -> bool {
    if !authorize(&caller, CallerPolicy::Main, "update_relaunch") {
        return false;
    }
    let updates = app.state::<Updates>();
    if shutdown_started(&app) || resource_close::is_closing() {
        return false;
    }
    let version = {
        let mut status = lock(&updates.status);
        let Some(UpdateStatus::Ready { version, .. }) = status.as_ref() else {
            return false;
        };
        let version = version.clone();
        *status = Some(UpdateStatus::Installing);
        version
    };
    updates.staged.store(false, Ordering::Release);
    let install_app = app.clone();
    resource_close::request_with_result(app, move |saved| {
        let updates = install_app.state::<Updates>();
        if !saved || shutdown_started(&install_app) {
            updates.set(UpdateStatus::Ready { version, retry_reason: Some("Your work could not be saved. The update is still ready; try again after saving.".into()) });
            resource_close::cancel_prepared(&install_app);
            return;
        }
        let Some(parked) = lock(&updates.parked).take() else {
            updates.set(UpdateStatus::Failed);
            resource_close::cancel_prepared(&install_app);
            return;
        };
        let worker_app = install_app.clone();
        let spawned = std::thread::Builder::new()
            .name("zephium-update-install".into())
            .spawn(move || {
                let next_version = parked.update.version.clone();
                if let Err(error) = stage(parked, &worker_app) {
                    write_diagnostic(format_args!("updates: install failed: {error}"));
                    worker_app.state::<Updates>().set(match error {
                        InstallError::Manual => UpdateStatus::ManualInstall {
                            version: next_version,
                        },
                        InstallError::Failed(_) => UpdateStatus::Failed,
                    });
                    resource_close::cancel_prepared(&worker_app);
                    return;
                }
                worker_app
                    .state::<Updates>()
                    .staged
                    .store(true, Ordering::Release);
                worker_app.exit(0);
            });
        if spawned.is_err() {
            updates.set(UpdateStatus::Failed);
            resource_close::cancel_prepared(&install_app);
        }
    });
    true
}

/// A user close cannot terminate the process midway through a bundle swap.
pub(crate) fn blocks_exit(app: &tauri::AppHandle) -> bool {
    app.try_state::<Updates>().is_some_and(|updates| {
        matches!(*lock(&updates.status), Some(UpdateStatus::Installing))
            && !updates.staged.load(Ordering::Acquire)
    })
}

#[derive(Debug)]
enum InstallError {
    Manual,
    Failed(String),
}
impl std::fmt::Display for InstallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Manual => f.write_str("manual installation is required"),
            Self::Failed(error) => f.write_str(error),
        }
    }
}
impl From<String> for InstallError {
    fn from(error: String) -> Self {
        Self::Failed(error)
    }
}
impl From<tauri_plugin_updater::Error> for InstallError {
    fn from(error: tauri_plugin_updater::Error) -> Self {
        match error {
            tauri_plugin_updater::Error::ManualInstallRequired => Self::Manual,
            error => Self::Failed(error.to_string()),
        }
    }
}

#[cfg(target_os = "windows")]
pub(crate) fn restore(app: &tauri::AppHandle, directory: &std::path::Path) {
    if !SUPPORTED {
        return;
    }
    let path = directory.join("pending-update");
    *lock(&app.state::<Updates>().recovery_path) = Some(path.clone());
    if !path.exists() {
        return;
    }
    // The installer replaced this build and exited before Zephium could clean
    // up; running the retained version means the update finished.
    if recovery::retained_version(&path)
        .is_some_and(|version| version == app.package_info().version.to_string())
    {
        let _ = std::fs::remove_file(&path);
        return;
    }
    app.state::<Updates>().set(UpdateStatus::Checking);
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let updates = app.state::<Updates>();
        match check_and_download(&app, &updates).await {
            Ok(status) => {
                if status == UpdateStatus::UpToDate {
                    let _ = std::fs::remove_file(&path);
                }
                updates.set(status);
            }
            Err(error) => {
                // Keep the payload for a later explicit retry, including when
                // offline. Never trust its on-disk version label as Ready.
                write_diagnostic(format_args!(
                    "updates: recovery release check failed: {error}"
                ));
                updates.set(UpdateStatus::Failed);
            }
        }
    });
}

/// macOS replaces the bundle now; the running process keeps its own mapped
/// files until the relaunch after shutdown.
#[cfg(target_os = "macos")]
fn stage(mut parked: Parked, _app: &tauri::AppHandle) -> Result<(), InstallError> {
    let bytes = parked.artifact.verified_bytes()?;
    parked.update.install(bytes).map_err(InstallError::from)?;
    RELAUNCH_AFTER_EXIT.store(true, Ordering::Release);
    Ok(())
}

/// The Windows installer replaces the running executable, so it starts only
/// after Zephium has shut down completely.
#[cfg(target_os = "windows")]
fn stage(mut parked: Parked, app: &tauri::AppHandle) -> Result<(), InstallError> {
    let path = lock(&app.state::<Updates>().recovery_path)
        .clone()
        .ok_or_else(|| InstallError::Failed("update recovery directory is unavailable".into()))?;
    let bytes = parked.artifact.verified_bytes()?;
    recovery::save(&path, &parked.update.raw_json, &bytes)
        .map_err(|_| InstallError::Failed("could not retain the update before shutdown".into()))?;
    *lock(&INSTALL_AFTER_EXIT) = Some((parked, path));
    Ok(())
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn stage(_parked: Parked, _app: &tauri::AppHandle) -> Result<(), InstallError> {
    Err(InstallError::Failed(
        "updates are not supported on this platform".to_owned(),
    ))
}

/// Called once the event loop has finished and every profile is saved.
#[cfg(target_os = "macos")]
pub(crate) fn finish_on_exit(app: &tauri::AppHandle) {
    let clean = app
        .try_state::<ShutdownCoordinator>()
        .is_some_and(|shutdown| {
            shutdown.authorized_exit_code.load(Ordering::Acquire) == 0
                && !shutdown.terminal_failure.load(Ordering::Acquire)
        });
    if RELAUNCH_AFTER_EXIT.swap(false, Ordering::AcqRel) {
        if clean {
            tauri::process::restart(&app.env());
        }
        return;
    }
    // An ordinary quit with an update waiting installs it now, so people who
    // never press Relaunch still update; the next launch is the new version.
    if clean {
        install_on_quit(app);
    }
}

#[cfg(target_os = "macos")]
fn install_on_quit(app: &tauri::AppHandle) {
    let Some(updates) = app.try_state::<Updates>() else {
        return;
    };
    if !matches!(updates.status(), UpdateStatus::Ready { .. }) {
        return;
    }
    let Some(mut parked) = lock(&updates.parked).take() else {
        return;
    };
    let result = parked
        .artifact
        .verified_bytes()
        .map_err(InstallError::from)
        .and_then(|bytes| parked.update.install(bytes).map_err(InstallError::from));
    if let Err(error) = result {
        write_diagnostic(format_args!(
            "updates: install on quit did not finish: {error}"
        ));
    }
}

/// Called after the WebView2 runtime has been released. On success the
/// installer takes over and this process exits; it relaunches Zephium itself.
#[cfg(target_os = "windows")]
pub(crate) fn finish_after_exit(clean: bool) {
    let Some((mut parked, _path)) = lock(&INSTALL_AFTER_EXIT).take() else {
        return;
    };
    if !clean {
        write_diagnostic(format_args!(
            "updates: installation cancelled after unclean shutdown"
        ));
        explain_deferred_update();
        return;
    }
    let result = parked.artifact.verified_bytes().and_then(|bytes| {
        parked
            .update
            .install(bytes)
            .map_err(|error| error.to_string())
    });
    if let Err(error) = result {
        write_diagnostic(format_args!("updates: install failed: {error}"));
        explain_deferred_update();
    }
}

#[cfg(target_os = "windows")]
fn explain_deferred_update() {
    rfd::MessageDialog::new()
        .set_title("Zephium update postponed")
        .set_level(rfd::MessageLevel::Warning)
        .set_description("Zephium could not finish the update safely. The downloaded update has been kept. Open Zephium again to retry from Settings → About.")
        .set_buttons(rfd::MessageButtons::Ok)
        .show();
}

/// Opens the system's own update settings for an outdated macOS or Safari.
#[tauri::command]
#[specta::specta]
pub(crate) fn open_software_update(caller: WebviewWindow) -> bool {
    if !authorize(&caller, CallerPolicy::Main, "open_software_update") {
        return false;
    }
    #[cfg(target_os = "macos")]
    {
        use objc2_app_kit::NSWorkspace;
        use objc2_foundation::{NSString, NSURL};
        let url = NSURL::URLWithString(&NSString::from_str(
            "x-apple.systempreferences:com.apple.Software-Update-Settings.extension",
        ));
        url.is_some_and(|url| NSWorkspace::sharedWorkspace().openURL(&url))
    }
    #[cfg(not(target_os = "macos"))]
    false
}

#[cfg(test)]
mod highlight_tests {
    use super::*;

    #[test]
    fn release_notes_become_a_short_plain_list() {
        let notes = "## What's Changed\n\
* feat(browse): ask before a page opens another app by @crynta in https://github.com/zephium-browser/Zephium/pull/60\n\
- **Page dialogs** now show as a sheet ([#61](https://github.com/x/y/pull/61)).\n\
- Fix `confirm()` returning false\n\
- one\n- two\n- three\n\
**Full Changelog**: https://github.com/x/y/compare/a...b\n";
        assert_eq!(
            highlights(notes),
            vec![
                "feat(browse): ask before a page opens another app",
                "Page dialogs now show as a sheet (#61)",
                "Fix confirm() returning false",
                "one",
                "two",
            ]
        );
        assert!(highlights("").is_empty());
        let long = format!("- {}", "word ".repeat(60));
        assert!(highlights(&long)[0].ends_with('…'));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repeat_checks_return_current_state_without_locking_again() {
        let updates = Updates::default();
        assert_eq!(updates.begin_check(true), Ok(()));
        assert_eq!(updates.begin_check(true), Err(UpdateStatus::Checking));
        for status in [
            UpdateStatus::Downloading,
            UpdateStatus::Ready {
                version: "1.0.1".into(),
                retry_reason: None,
            },
            UpdateStatus::Installing,
        ] {
            updates.set(status.clone());
            assert_eq!(updates.begin_check(true), Err(status));
        }
        updates.set(UpdateStatus::Failed);
        assert_eq!(updates.begin_check(true), Ok(()));
        assert_eq!(updates.begin_check(false), Err(UpdateStatus::Unavailable));
    }
}
