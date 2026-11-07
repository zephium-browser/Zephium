//! Installed Chrome extensions: Web Store download, consent, the per-profile
//! registry on disk, and handing enabled extensions to the shell.
//!
//! Layout under the app data directory:
//! `webext/packages/<id>/<source-digest>.crx` keeps the signed original (or
//! `.zip`, or a `.src/` folder, for an extension installed from a file), and
//! `webext/packages/<id>/pkg-<publication-digest>/` is the tree WebKit loads,
//! rebuilt from the original whenever the compatibility layer changes. The publication
//! digest binds verified source provenance, transformation revision and prepared bytes.
//! `webext/profiles/<profile>.json` records what a profile installed.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use tauri::{Manager, State, WebviewWindow};
use zephium_app::Handle;
use zephium_core::ids::ProfileId;

use crate::{authorize, shutdown_started, CallerPolicy};

pub(crate) struct WebExtensions {
    #[cfg_attr(not(any(target_os = "macos", target_os = "windows")), allow(dead_code))]
    root: PathBuf,
    #[cfg_attr(not(any(target_os = "macos", target_os = "windows")), allow(dead_code))]
    state: Mutex<Option<Pending>>,
}

#[cfg_attr(not(any(target_os = "macos", target_os = "windows")), allow(dead_code))]
struct Pending {
    /// Single-use approval for this exact package and destination profile.
    review_id: String,
    profile: ProfileId,
    entry: Entry,
    staged: PathBuf,
    original: Original,
    /// The exact install and source the user reviewed replacing.
    previous: Option<(String, String)>,
}

/// What a package was installed from, kept to rebuild it later.
#[cfg_attr(not(any(target_os = "macos", target_os = "windows")), allow(dead_code))]
enum Original {
    Crx(Vec<u8>),
    Zip(Vec<u8>),
    Folder(PathBuf),
}

/// Which sites an extension may read and change.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "lowercase")]
enum Access {
    /// Every site it asked for.
    #[default]
    All,
    /// Only a tab where the user clicked its button.
    Click,
    /// Only these sites, among those it asked for.
    Sites { sites: Vec<String> },
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct Registry {
    extensions: Vec<Entry>,
}

#[cfg_attr(not(any(target_os = "macos", target_os = "windows")), allow(dead_code))]
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Entry {
    install: String,
    id: String,
    name: String,
    version: String,
    description: String,
    enabled: bool,
    /// Directory under `packages/<id>/` WebKit loads.
    package: String,
    /// Authenticated CRX bytes, ZIP bytes, or the frozen original folder tree.
    #[serde(default)]
    source: String,
    #[serde(default)]
    prepared_digest: String,
    /// Compatibility layer the package was prepared with.
    compat: String,
    /// Compatibility layer WebKit last started the background with.
    #[serde(default)]
    started: String,
    permissions: Vec<String>,
    /// Original API permissions accepted in review, excluding compatibility transport.
    #[serde(default)]
    reviewed_permissions: Vec<String>,
    /// CRX publisher possession was verified for this exact source package.
    #[serde(default)]
    publisher_verified: bool,
    /// The person allowed native-app access the extension declares optional.
    #[serde(default)]
    native_messaging_granted: bool,
    hosts: Vec<String>,
    #[serde(default)]
    icon: Option<String>,
    /// A newer version held back because it asks for more access.
    #[serde(default)]
    held_update: Option<String>,
    #[serde(default)]
    access: Access,
    /// Installed from a file rather than the Web Store; never updated from
    /// the store, so a developer's own build stays as it is.
    #[serde(default)]
    sideloaded: bool,
    /// Its button sits in the address field rather than in the tray.
    #[serde(default)]
    pinned: bool,
}

#[derive(Clone, Debug, Serialize, specta::Type)]
pub(crate) struct WebExtensionReview {
    /// Identifies the package/profile shown by this particular review.
    review_id: String,
    id: String,
    name: String,
    version: String,
    description: String,
    warnings: Vec<String>,
    /// What this browser can't do for the extension, apart from its access.
    limitations: Vec<String>,
    icon: Option<String>,
    update: bool,
    /// Installed from a file, so not checked against the Web Store.
    from_file: bool,
}

#[derive(Clone, Debug, Serialize, specta::Type)]
pub(crate) struct WebExtensionView {
    profile_id: String,
    install_id: String,
    id: String,
    name: String,
    version: String,
    description: String,
    enabled: bool,
    icon: Option<String>,
    /// `running`, `starting`, `failed` or `off`.
    state: String,
    error: Option<String>,
    warnings: Vec<String>,
    limitations: Vec<String>,
    /// `all`, `click` or `sites`; meaningful only when `site_scoped`.
    access: String,
    sites: Vec<String>,
    /// Whether the extension asked for access to websites at all.
    site_scoped: bool,
    has_options: bool,
    held_update: Option<String>,
    sideloaded: bool,
    pinned: bool,
}

impl WebExtensions {
    pub(crate) fn new(data_dir: &Path) -> Self {
        Self {
            root: data_dir.join("webext"),
            state: Mutex::new(None),
        }
    }

    /// How many are enabled and how many are installed, for a bug report.
    pub(crate) fn counts(&self) -> (usize, usize) {
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        return imp::counts(self);
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        (0, 0)
    }

    /// Restores every profile's extensions at launch.
    pub(crate) fn restore(&self, shell: &Handle) {
        if let Some(data_dir) = self.root.parent() {
            remove_previous_repository(data_dir);
        }
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        {
            let root = self.root.clone();
            let callback = shell.callback_handle();
            restore_in_background(move || {
                let extensions = WebExtensions {
                    root,
                    state: Mutex::new(None),
                };
                imp::restore(&extensions, &callback);
            });
            imp::start_updates(&self.root, shell);
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        let _ = shell;
    }
}

/// Removes the package repository left by the previous extension stack, in
/// the background and best effort. It sealed its directories read-only, so
/// they are made writable first.
fn remove_previous_repository(data_dir: &Path) {
    let path = data_dir.join("extension-repository-v1");
    if !path.exists() {
        return;
    }
    let spawned = std::thread::Builder::new()
        .name("zephium-legacy-extension-cleanup".into())
        .spawn(move || {
            unseal(&path);
            if let Err(error) = std::fs::remove_dir_all(&path) {
                eprintln!("extensions: could not remove the previous repository: {error}");
            }
        });
    if let Err(error) = spawned {
        eprintln!("extensions: could not start the previous repository cleanup: {error}");
    }
}

fn unseal(path: &Path) {
    let Ok(metadata) = std::fs::symlink_metadata(path) else {
        return;
    };
    if metadata.file_type().is_symlink() {
        return;
    }
    #[cfg(unix)]
    let permissions = {
        use std::os::unix::fs::PermissionsExt;
        std::fs::Permissions::from_mode(if metadata.is_dir() { 0o700 } else { 0o600 })
    };
    #[cfg(not(unix))]
    let permissions = {
        let mut permissions = metadata.permissions();
        // Windows only clears the read-only attribute.
        #[allow(clippy::permissions_set_readonly_false)]
        permissions.set_readonly(false);
        permissions
    };
    let _ = std::fs::set_permissions(path, permissions);
    if metadata.is_dir() {
        if let Ok(entries) = std::fs::read_dir(path) {
            for entry in entries.flatten() {
                unseal(&entry.path());
            }
        }
    }
}

const UNAVAILABLE: &str = "Extensions are unavailable in this build.";

#[cfg(any(target_os = "macos", target_os = "windows"))]
static PUBLICATION_JOBS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

#[cfg(any(target_os = "macos", target_os = "windows"))]
struct PublicationSlot<'a>(&'a std::sync::atomic::AtomicUsize);

#[cfg(any(target_os = "macos", target_os = "windows"))]
impl<'a> PublicationSlot<'a> {
    fn acquire(jobs: &'a std::sync::atomic::AtomicUsize) -> Result<Self, String> {
        jobs.fetch_update(
            std::sync::atomic::Ordering::Relaxed,
            std::sync::atomic::Ordering::Relaxed,
            |active| (active < 8).then(|| active + 1),
        )
        .map_err(|_| "Extensions are busy. Try again in a moment.".to_owned())?;
        Ok(Self(jobs))
    }
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
impl Drop for PublicationSlot<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
    }
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
async fn publication(
    work: impl FnOnce() -> Result<(), String> + Send + 'static,
) -> Result<(), String> {
    let slot = PublicationSlot::acquire(&PUBLICATION_JOBS)?;
    publication_reserved(slot, work).await
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
async fn publication_reserved(
    slot: PublicationSlot<'static>,
    work: impl FnOnce() -> Result<(), String> + Send + 'static,
) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        // The job owns capacity even if its async caller stops waiting.
        let _slot = slot;
        work()
    })
    .await
    .map_err(|_| "Extension state is unavailable.".to_owned())?
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
fn take_pending_for_publication<'a>(
    extensions: &WebExtensions,
    id: &str,
    review_id: &str,
    jobs: &'a std::sync::atomic::AtomicUsize,
) -> Result<(PublicationSlot<'a>, Pending), String> {
    // Admission failure must leave the managed review and its snapshots owned.
    let slot = PublicationSlot::acquire(jobs)?;
    let pending = imp::take_pending(extensions, id, review_id)?;
    Ok((slot, pending))
}

/// Downloads and verifies the extension on the active store page, and
/// returns what the user is asked to approve.
#[tauri::command]
#[specta::specta]
pub(crate) async fn web_extension_prepare(
    caller: WebviewWindow,
    shell: State<'_, Handle>,
    extensions: State<'_, WebExtensions>,
    tab_id: String,
) -> Result<WebExtensionReview, String> {
    if !authorize(&caller, CallerPolicy::Main, "web_extension_prepare")
        || shutdown_started(caller.app_handle())
    {
        return Err(UNAVAILABLE.into());
    }
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    return imp::prepare(&shell, &extensions, &tab_id).await;
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = (shell, extensions, tab_id);
        Err(UNAVAILABLE.into())
    }
}

#[derive(Clone, Copy, Debug, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub(crate) enum WebExtensionStoreButtonState {
    Add,
    Adding,
    Remove,
}

/// What the install button on the Web Store listing in `tab_id` shows. A
/// click on it arrives as `zephium:web-extension-store-click`.
#[tauri::command]
#[specta::specta]
pub(crate) fn web_extension_store_button(
    caller: WebviewWindow,
    shell: State<'_, Handle>,
    tab_id: String,
    extension_id: String,
    state: WebExtensionStoreButtonState,
    label: String,
) -> bool {
    use zephium_core::ports::engine::{StoreButton, StoreButtonState};
    if !authorize(&caller, CallerPolicy::Main, "web_extension_store_button") {
        return false;
    }
    let Some(tab) = zephium_core::ids::ItemId::parse(&tab_id) else {
        return false;
    };
    if zephium_webext::ExtensionId::parse(&extension_id).is_none() {
        return false;
    }
    let label_chars = label.chars().count();
    if label_chars == 0 || label_chars > zephium_core::ports::engine::MAX_STORE_BUTTON_LABEL_CHARS {
        return false;
    }
    let state = match state {
        WebExtensionStoreButtonState::Add => StoreButtonState::Add,
        WebExtensionStoreButtonState::Adding => StoreButtonState::Adding,
        WebExtensionStoreButtonState::Remove => StoreButtonState::Remove,
    };
    shell.dispatch(zephium_app::Command::ShowStoreButton {
        tab,
        button: StoreButton {
            extension: extension_id,
            state,
            label,
        },
    })
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn web_extension_confirm(
    caller: WebviewWindow,
    shell: State<'_, Handle>,
    extensions: State<'_, WebExtensions>,
    id: String,
    review_id: String,
) -> Result<(), String> {
    if !authorize(&caller, CallerPolicy::Main, "web_extension_confirm")
        || shutdown_started(caller.app_handle())
    {
        return Err(UNAVAILABLE.into());
    }
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    {
        // Freeze the exact managed review before queueing blocking work: a
        // newer review may replace the slot while publication is waiting.
        let (slot, pending) =
            take_pending_for_publication(&extensions, &id, &review_id, &PUBLICATION_JOBS)?;
        let ticket = match imp::publication_ticket(&extensions, pending.profile) {
            Ok(ticket) => ticket,
            Err(error) => {
                return publication_reserved(slot, move || {
                    imp::discard(&pending);
                    Err(error)
                })
                .await
            }
        };
        if imp::target(&shell, None).await.map(|target| target.profile) != Ok(pending.profile) {
            return publication_reserved(slot, move || {
                imp::discard(&pending);
                Err("The extension profile changed. Review it again before installing.".into())
            })
            .await;
        }
        let app = caller.app_handle().clone();
        let shell = shell.inner().clone();
        publication_reserved(slot, move || {
            let extensions = app.state::<WebExtensions>();
            imp::confirm(&shell, &extensions, pending, ticket)
        })
        .await
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = (shell, extensions, id, review_id);
        Err(UNAVAILABLE.into())
    }
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn web_extension_cancel(caller: WebviewWindow, review_id: String) {
    if !authorize(&caller, CallerPolicy::Main, "web_extension_cancel") {
        return;
    }
    let pending = {
        let extensions = caller.state::<WebExtensions>();
        cancel_pending(&extensions, &review_id)
    };
    if let Some(pending) = pending {
        discard_cancelled(pending).await;
    }
}

async fn discard_cancelled(pending: Pending) {
    let _ = tauri::async_runtime::spawn_blocking(move || discard_pending(&pending)).await;
}

fn discard_pending(pending: &Pending) {
    let _ = std::fs::remove_dir_all(&pending.staged);
    if let Original::Folder(path) = &pending.original {
        let _ = std::fs::remove_dir_all(path);
    }
}

fn cancel_pending(extensions: &WebExtensions, review_id: &str) -> Option<Pending> {
    let mut state = extensions.state.lock().ok()?;
    if state
        .as_ref()
        .is_some_and(|pending| pending.review_id == review_id)
    {
        state.take()
    } else {
        None
    }
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn web_extension_list(
    caller: WebviewWindow,
    shell: State<'_, Handle>,
    extensions: State<'_, WebExtensions>,
) -> Result<Vec<WebExtensionView>, String> {
    if !authorize(&caller, CallerPolicy::Main, "web_extension_list") {
        return Err(UNAVAILABLE.into());
    }
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    return imp::list(&shell, &extensions).await;
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = (shell, extensions);
        Err(UNAVAILABLE.into())
    }
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn web_extension_set_enabled(
    caller: WebviewWindow,
    shell: State<'_, Handle>,
    extensions: State<'_, WebExtensions>,
    id: String,
    enabled: bool,
) -> Result<(), String> {
    if !authorize(&caller, CallerPolicy::Main, "web_extension_set_enabled") {
        return Err(UNAVAILABLE.into());
    }
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    return imp::set_enabled(&shell, &extensions, &id, enabled).await;
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = (shell, extensions, id, enabled);
        Err(UNAVAILABLE.into())
    }
}

/// Puts the extension's button in the address field, or back in the tray.
#[tauri::command]
#[specta::specta]
pub(crate) async fn web_extension_set_pinned(
    caller: WebviewWindow,
    shell: State<'_, Handle>,
    extensions: State<'_, WebExtensions>,
    id: String,
    pinned: bool,
) -> Result<(), String> {
    if !authorize(&caller, CallerPolicy::Main, "web_extension_set_pinned") {
        return Err(UNAVAILABLE.into());
    }
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    return imp::set_pinned(&shell, &extensions, &id, pinned).await;
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = (shell, extensions, id, pinned);
        Err(UNAVAILABLE.into())
    }
}

/// Retries only the exact installation displayed in the Extensions page.
#[tauri::command]
#[specta::specta]
pub(crate) async fn web_extension_retry(
    caller: WebviewWindow,
    shell: State<'_, Handle>,
    extensions: State<'_, WebExtensions>,
    profile_id: String,
    install_id: String,
) -> Result<(), String> {
    if !authorize(&caller, CallerPolicy::Main, "web_extension_retry")
        || shutdown_started(caller.app_handle())
    {
        return Err(UNAVAILABLE.into());
    }
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    return imp::retry(
        &shell,
        &extensions,
        ProfileId::parse(&profile_id).ok_or("Invalid profile.")?,
        zephium_core::ids::ExtensionInstallId::parse(&install_id).ok_or("Invalid installation.")?,
    )
    .await;
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = (shell, extensions, profile_id, install_id);
        Err(UNAVAILABLE.into())
    }
}

/// Forgets access an extension gave up itself, so the next launch doesn't
/// grant it again.
pub(crate) fn access_removed(
    app: &tauri::AppHandle,
    removal: zephium_ipc::WebExtensionAccessRemovalView,
) {
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    {
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            let result = publication(move || {
                let extensions = app.state::<WebExtensions>();
                imp::access_removed(&extensions, removal)
            })
            .await;
            if let Err(error) = result {
                eprintln!("webext: could not record removed access: {error}");
            }
        });
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let _ = (app, removal);
}

/// Answers an extension's run-time request for access; granted access is
/// kept for the next launch.
#[tauri::command]
#[specta::specta]
pub(crate) async fn web_extension_answer_access(
    caller: WebviewWindow,
    shell: State<'_, Handle>,
    request: zephium_ipc::WebExtensionAccessRequestView,
    allowed: bool,
) -> Result<(), String> {
    if !authorize(&caller, CallerPolicy::Main, "web_extension_answer_access") {
        return Err(UNAVAILABLE.into());
    }
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    {
        let app = caller.app_handle().clone();
        let shell = shell.inner().clone();
        publication(move || {
            let extensions = app.state::<WebExtensions>();
            imp::answer_access(&shell, &extensions, request, allowed)
        })
        .await
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = (shell, request, allowed);
        Err(UNAVAILABLE.into())
    }
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn web_extension_uninstall(
    caller: WebviewWindow,
    shell: State<'_, Handle>,
    extensions: State<'_, WebExtensions>,
    id: String,
) -> Result<(), String> {
    if !authorize(&caller, CallerPolicy::Main, "web_extension_uninstall") {
        return Err(UNAVAILABLE.into());
    }
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    return imp::uninstall(&shell, &extensions, &id).await;
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = (shell, extensions, id);
        Err(UNAVAILABLE.into())
    }
}

/// Chooses which sites an extension may use: `all` it asked for, only on
/// `click`, or the listed `sites`.
#[tauri::command]
#[specta::specta]
pub(crate) async fn web_extension_set_access(
    caller: WebviewWindow,
    shell: State<'_, Handle>,
    extensions: State<'_, WebExtensions>,
    id: String,
    mode: String,
    sites: Vec<String>,
) -> Result<(), String> {
    if !authorize(&caller, CallerPolicy::Main, "web_extension_set_access") {
        return Err(UNAVAILABLE.into());
    }
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    return imp::set_access(&shell, &extensions, &id, &mode, sites).await;
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = (shell, extensions, id, mode, sites);
        Err(UNAVAILABLE.into())
    }
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn web_extension_open_options(
    caller: WebviewWindow,
    shell: State<'_, Handle>,
    id: String,
) -> Result<(), String> {
    if !authorize(&caller, CallerPolicy::Main, "web_extension_open_options") {
        return Err(UNAVAILABLE.into());
    }
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    return imp::open_options(&shell, &id).await;
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = (shell, id);
        Err(UNAVAILABLE.into())
    }
}

/// Lets the user pick a `.crx` or `.zip` file, or with `folder` an unpacked
/// extension, and returns what they are asked to approve; `None` when they
/// cancel the picker.
#[tauri::command]
#[specta::specta]
pub(crate) async fn web_extension_choose_file(
    caller: WebviewWindow,
    shell: State<'_, Handle>,
    extensions: State<'_, WebExtensions>,
    folder: bool,
) -> Result<Option<WebExtensionReview>, String> {
    if !authorize(&caller, CallerPolicy::Main, "web_extension_choose_file")
        || shutdown_started(caller.app_handle())
    {
        return Err(UNAVAILABLE.into());
    }
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    return imp::choose_file(caller.app_handle(), &shell, &extensions, folder).await;
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = (shell, extensions, folder);
        Err(UNAVAILABLE.into())
    }
}

/// Reviews an extension file dropped on the browser.
#[tauri::command]
#[specta::specta]
pub(crate) async fn web_extension_prepare_file(
    caller: WebviewWindow,
    shell: State<'_, Handle>,
    extensions: State<'_, WebExtensions>,
    path: String,
) -> Result<WebExtensionReview, String> {
    if !authorize(&caller, CallerPolicy::Main, "web_extension_prepare_file")
        || shutdown_started(caller.app_handle())
    {
        return Err(UNAVAILABLE.into());
    }
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    return imp::prepare_file(&shell, &extensions, PathBuf::from(path)).await;
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = (shell, extensions, path);
        Err(UNAVAILABLE.into())
    }
}

/// Reviews the newest store version of an extension whose update waits for
/// the user's approval.
#[tauri::command]
#[specta::specta]
pub(crate) async fn web_extension_review_update(
    caller: WebviewWindow,
    shell: State<'_, Handle>,
    extensions: State<'_, WebExtensions>,
    id: String,
) -> Result<WebExtensionReview, String> {
    if !authorize(&caller, CallerPolicy::Main, "web_extension_review_update")
        || shutdown_started(caller.app_handle())
    {
        return Err(UNAVAILABLE.into());
    }
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    return imp::review_update(&shell, &extensions, &id).await;
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = (shell, extensions, id);
        Err(UNAVAILABLE.into())
    }
}

/// An extension package among files dropped on the browser: one `.crx` or
/// `.zip` file, or one folder with a manifest.
pub(crate) fn dropped_package(paths: &[PathBuf]) -> Option<String> {
    let [path] = paths else {
        return None;
    };
    let package = match path.extension().and_then(|extension| extension.to_str()) {
        Some("crx" | "zip") => path.is_file(),
        _ => path.join("manifest.json").is_file(),
    };
    package.then(|| path.to_string_lossy().into_owned())
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
mod imp;

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn removes_the_previous_repository_even_when_sealed() {
        use std::os::unix::fs::PermissionsExt;
        let data = tempfile::tempdir().unwrap();
        let sealed = data.path().join("extension-repository-v1/objects");
        std::fs::create_dir_all(&sealed).unwrap();
        std::fs::write(sealed.join("package"), b"old").unwrap();
        std::fs::set_permissions(&sealed, std::fs::Permissions::from_mode(0o500)).unwrap();

        let path = data.path().join("extension-repository-v1");
        unseal(&path);
        std::fs::remove_dir_all(&path).unwrap();
        assert!(!path.exists());
    }
}

#[cfg(all(test, any(target_os = "macos", target_os = "windows")))]
mod publication_tests {
    #[test]
    fn publication_capacity_is_bounded_and_returns_after_drop_or_panic() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let jobs = AtomicUsize::new(0);
        let mut slots = (0..8)
            .map(|_| super::PublicationSlot::acquire(&jobs).unwrap())
            .collect::<Vec<_>>();
        let saturated = super::PublicationSlot::acquire(&jobs);
        assert!(
            saturated.is_err(),
            "publication admitted an unbounded ninth job"
        );
        drop(saturated);
        drop(slots.pop());
        let returned = super::PublicationSlot::acquire(&jobs).unwrap();
        assert_eq!(jobs.load(Ordering::Relaxed), 8);
        let result = std::panic::catch_unwind(move || {
            let _slot = returned;
            panic!("fixture job panic");
        });
        assert!(result.is_err());
        assert_eq!(jobs.load(Ordering::Relaxed), 7);
        drop(slots);
        assert_eq!(jobs.load(Ordering::Relaxed), 0);
        drop(super::PublicationSlot::acquire(&jobs).unwrap());
        assert_eq!(jobs.load(Ordering::Relaxed), 0);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_publication_lock_wait_leaves_the_command_executor_responsive() {
        let lease = std::sync::Arc::new(std::sync::Mutex::new(()));
        let held = lease.clone();
        let (ready, locked) = std::sync::mpsc::channel();
        let (arrived, waiting) = std::sync::mpsc::channel();
        let (progress, observed) = std::sync::mpsc::channel();
        let supervisor = std::thread::spawn(move || {
            let guard = held.lock().unwrap();
            let _ = ready.send(());
            let started = waiting
                .recv_timeout(std::time::Duration::from_secs(5))
                .is_ok();
            let responsive = observed
                .recv_timeout(std::time::Duration::from_secs(5))
                .is_ok();
            drop(guard);
            started && responsive
        });
        locked
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        let (entered, started) = tokio::sync::oneshot::channel();
        let operation = tokio::spawn(super::publication(move || {
            let _ = arrived.send(());
            let _ = entered.send(());
            let _guard = lease.lock().unwrap();
            Ok(())
        }));
        started.await.unwrap();
        let _ = progress.send(());
        operation.await.unwrap().unwrap();
        assert!(
            supervisor.join().unwrap(),
            "the executor must run while publication is blocked"
        );
    }
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
fn restore_in_background(work: impl FnOnce() + Send + 'static) {
    tauri::async_runtime::spawn_blocking(work);
}

#[cfg(all(test, any(target_os = "macos", target_os = "windows")))]
#[test]
fn startup_restore_never_waits_for_package_preparation() {
    let (release, blocked) = std::sync::mpsc::sync_channel(1);
    let (returned, result) = std::sync::mpsc::sync_channel(1);
    let caller = std::thread::spawn(move || {
        restore_in_background(move || {
            let _ = blocked.recv_timeout(std::time::Duration::from_secs(3));
        });
        returned.send(()).unwrap();
    });
    let immediate = result.recv_timeout(std::time::Duration::from_millis(250));
    release.send(()).unwrap();
    caller.join().unwrap();
    assert!(
        immediate.is_ok(),
        "setup waited for legacy package preparation"
    );
}
