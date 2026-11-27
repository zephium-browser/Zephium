//! Installable mini web apps.
//!
//! The durable registry is one bounded app setting, which keeps the feature
//! free of a second database. On Windows each installed app receives a
//! standalone WebView2 window and its own persistent data directory; it never
//! shares the browser profile or privileged Tauri capabilities. Navigation is
//! confined to the manifest scope (or the installing page's origin for a site
//! without a manifest), with out-of-scope HTTPS links handed to the browser.

use std::net::SocketAddr;
#[cfg(target_os = "windows")]
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
#[cfg(target_os = "windows")]
use std::{
    collections::HashSet,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, LazyLock,
    },
};
#[cfg(target_os = "windows")]
use std::{fs, path::PathBuf};

use reqwest::Url;
use serde::{Deserialize, Serialize};
use tauri::{Manager, WebviewWindow};
use zephium_core::ports::store::{AppSettingReadback, Store};
use zephium_miniapps::{
    app_id, is_public_host, is_public_ip, manifest_href, parse_manifest, same_origin,
    site_metadata, DisplayMode, MiniApp, MiniAppError, Registry, MAX_MANIFEST_BYTES,
};

use crate::{authorize, CallerPolicy, Command, Handle, APP_STORE};

const REGISTRY_SETTING: &str = "mini_apps.registry";
const REGISTRY_BACKUP_SETTING: &str = "mini_apps.registry.backup.v1";
const MAX_PAGE_BYTES: usize = 2 * 1024 * 1024;
const MAX_ICON_BYTES: usize = 512 * 1024;
const MAX_INSTALL_URL_BYTES: usize = 2_048;
const MAX_REDIRECTS: usize = 3;

/// Serialises read-modify-write cycles. A registry write is small, but two
/// installs arriving together must not drop one of them.
static REGISTRY_LOCK: Mutex<()> = Mutex::new(());
#[cfg(target_os = "windows")]
static STANDALONE_WINDOW_OPENED: AtomicBool = AtomicBool::new(false);
#[cfg(target_os = "windows")]
static STANDALONE_LAUNCH_FINISHED: AtomicBool = AtomicBool::new(false);
#[cfg(target_os = "windows")]
static OPENING_WINDOWS: LazyLock<Mutex<HashSet<String>>> =
    LazyLock::new(|| Mutex::new(HashSet::new()));

#[cfg(target_os = "windows")]
struct OpeningWindow(String);

#[cfg(target_os = "windows")]
impl Drop for OpeningWindow {
    fn drop(&mut self) {
        OPENING_WINDOWS
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(&self.0);
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, specta::Type)]
pub struct MiniAppView {
    pub id: String,
    pub name: String,
    pub start_url: String,
    pub scope: String,
    pub display: String,
    pub theme_color: Option<String>,
    pub background_color: Option<String>,
    pub icon: Option<String>,
    pub registry_recovered: bool,
}

impl From<&MiniApp> for MiniAppView {
    fn from(app: &MiniApp) -> Self {
        Self {
            id: app.id.clone(),
            name: app.name.clone(),
            start_url: app.start_url.clone(),
            scope: app.scope.clone(),
            display: app.display.id().to_owned(),
            theme_color: app.theme_color.clone(),
            background_color: app.background_color.clone(),
            icon: app.icon.clone(),
            registry_recovered: false,
        }
    }
}

#[tauri::command]
#[specta::specta]
pub(crate) fn mini_apps_list(caller: WebviewWindow) -> Vec<MiniAppView> {
    if !authorize(&caller, CallerPolicy::Main, "mini_apps_list") {
        return Vec::new();
    }
    load_registry()
        .map(|registry| registry.apps().iter().map(MiniAppView::from).collect())
        .unwrap_or_default()
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn mini_apps_install(
    caller: WebviewWindow,
    app_handle: tauri::AppHandle,
    url: String,
) -> Result<MiniAppView, String> {
    if !authorize(&caller, CallerPolicy::Main, "mini_apps_install")
        || url.is_empty()
        || url.len() > MAX_INSTALL_URL_BYTES
    {
        return Err("This address cannot be installed.".into());
    }
    let discovered = discover_and_parse(&url).await?;
    let mini_app = discovered.app;
    let mut view = MiniAppView::from(&mini_app);
    let _guard = REGISTRY_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let (mut registry, recovered) = registry_for_install()?;
    view.registry_recovered = recovered;
    registry
        .add(mini_app.clone())
        .map_err(|error| installation_error(&error))?;
    let json = registry
        .to_json()
        .map_err(|error| installation_error(&error))?;
    let stored = APP_STORE
        .get()
        .is_some_and(|store| store.set_app_setting(REGISTRY_SETTING.into(), json));
    if !stored {
        return Err("The app could not be saved.".into());
    }
    #[cfg(target_os = "windows")]
    if let Some(icon) = discovered.native_icon.as_ref() {
        persist_windows_app_icon(&app_handle, &mini_app, icon);
    }
    #[cfg(target_os = "windows")]
    for destination in ["start_menu", "desktop"] {
        create_windows_shortcut(&app_handle, &mini_app, destination)?;
    }
    #[cfg(not(target_os = "windows"))]
    let _ = (app_handle, discovered.native_icon);
    Ok(view)
}

#[tauri::command]
#[specta::specta]
pub(crate) fn mini_apps_remove(
    caller: WebviewWindow,
    app_handle: tauri::AppHandle,
    id: String,
) -> bool {
    if !authorize(&caller, CallerPolicy::Main, "mini_apps_remove") {
        return false;
    }
    let _guard = REGISTRY_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let Ok(mut registry) = load_registry() else {
        return false;
    };
    let Some(removed) = registry.find(&id).cloned() else {
        return false;
    };
    if registry.remove(&id).is_err() {
        return false;
    }
    let Ok(json) = registry.to_json() else {
        return false;
    };
    let saved = APP_STORE
        .get()
        .is_some_and(|store| store.set_app_setting(REGISTRY_SETTING.into(), json));
    if saved {
        close_app_window(&app_handle, &removed.id);
    }
    #[cfg(target_os = "windows")]
    if saved {
        remove_windows_shortcuts(&app_handle, &removed);
        remove_app_storage(&app_handle, &removed.id);
    }
    #[cfg(not(target_os = "windows"))]
    let _ = removed;
    saved
}

fn close_app_window(app: &tauri::AppHandle, id: &str) {
    if !valid_id(id) {
        return;
    }
    if let Some(window) = app.get_webview_window(&format!("miniapp-{id}")) {
        let _ = window.close();
    }
}

#[cfg(target_os = "windows")]
fn app_storage_path(app: &tauri::AppHandle, id: &str) -> Result<PathBuf, String> {
    if !valid_id(id) {
        return Err("The app identifier is invalid.".into());
    }
    app.path()
        .app_data_dir()
        .map(|root| root.join("mini-apps").join(id))
        .map_err(|_| "The app data folder is unavailable.".into())
}

#[cfg(target_os = "windows")]
fn remove_app_storage(app: &tauri::AppHandle, id: &str) {
    let Ok(path) = app_storage_path(app, id) else {
        return;
    };
    // This path is derived from a validated fixed-length hex id beneath the
    // application data directory, never from site-controlled manifest data.
    let _ = std::thread::Builder::new()
        .name("Zephium-mini-app-data-cleanup".into())
        .spawn(move || {
            for attempt in 0..8 {
                match fs::remove_dir_all(&path) {
                    Ok(()) => return,
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
                    Err(_) if attempt < 7 => std::thread::sleep(Duration::from_millis(250)),
                    Err(_) => return,
                }
            }
        });
}

#[cfg(target_os = "windows")]
fn create_windows_shortcut(
    app: &tauri::AppHandle,
    mini_app: &MiniApp,
    destination: &str,
) -> Result<(), String> {
    use windows::core::{Interface, PCWSTR};
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, IPersistFile,
        CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED,
    };
    use windows::Win32::UI::Shell::PropertiesSystem::IPropertyStore;
    use windows::Win32::UI::Shell::{
        FOLDERID_Desktop, FOLDERID_Programs, IShellLinkW, SHGetKnownFolderPath, ShellLink,
    };

    let base = match destination {
        "start_menu" => &FOLDERID_Programs,
        "desktop" => &FOLDERID_Desktop,
        _ => return Err("Choose Start menu or desktop.".into()),
    };
    let folder = unsafe { SHGetKnownFolderPath(base, Default::default(), None) }
        .map_err(|_| "Windows could not find the shortcut folder.".to_owned())?;
    let folder_path = unsafe {
        let path = folder.to_string();
        CoTaskMemFree(Some(folder.0.cast()));
        path
    }
    .map_err(|_| "Windows returned an invalid shortcut folder.".to_owned())?;
    let host = Url::parse(&mini_app.start_url)
        .ok()
        .and_then(|url| url.host_str().map(str::to_owned))
        .unwrap_or_default();
    let folder_path = PathBuf::from(folder_path);
    let filename = shortcut_filename(app, mini_app, destination, &folder_path, &host)?;
    let shortcut = folder_path.join(format!("{filename}.lnk"));
    let executable = std::env::current_exe()
        .map_err(|_| "Zephium's application path is unavailable.".to_owned())?;
    let icon = app
        .path()
        .resolve("icons/icon.ico", tauri::path::BaseDirectory::Resource)
        .unwrap_or(executable.clone());
    let icon = app_storage_path(app, &mini_app.id)
        .ok()
        .map(|directory| directory.join("icon.ico"))
        .filter(|path| path.is_file())
        .unwrap_or(icon);

    let initialized = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
    if initialized.is_err() {
        return Err("Windows shortcut services could not be initialized.".into());
    }
    struct ComApartment;
    impl Drop for ComApartment {
        fn drop(&mut self) {
            unsafe { CoUninitialize() };
        }
    }
    let _apartment = ComApartment;
    unsafe {
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)
            .map_err(|_| "Windows could not create the app shortcut.".to_owned())?;
        let executable_wide = wide(executable.as_os_str());
        let icon_wide = wide(icon.as_os_str());
        let arguments_wide = wide(std::ffi::OsStr::new(&format!("--mini-app {}", mini_app.id)));
        let description_wide = wide(std::ffi::OsStr::new(&mini_app.name));
        link.SetPath(PCWSTR(executable_wide.as_ptr()))
            .map_err(|_| "Windows could not set the shortcut target.".to_owned())?;
        link.SetArguments(PCWSTR(arguments_wide.as_ptr()))
            .map_err(|_| "Windows could not set the app address.".to_owned())?;
        link.SetDescription(PCWSTR(description_wide.as_ptr()))
            .map_err(|_| "Windows could not set the shortcut name.".to_owned())?;
        link.SetIconLocation(PCWSTR(icon_wide.as_ptr()), 0)
            .map_err(|_| "Windows could not set the app icon.".to_owned())?;
        let app_user_model_id =
            crate::platform::windows_taskbar_icon::windows_app_user_model_id(&mini_app.id);
        let property_store: IPropertyStore = link
            .cast()
            .map_err(|_| "Windows shortcut identity is unavailable.".to_owned())?;
        crate::platform::windows_taskbar_icon::set_property_store_app_user_model_id(
            &property_store,
            &app_user_model_id,
        )
        .map_err(|_| "Windows could not set the shortcut identity.".to_owned())?;
        let persist: IPersistFile = link
            .cast()
            .map_err(|_| "Windows shortcut storage is unavailable.".to_owned())?;
        let shortcut_wide = wide(shortcut.as_os_str());
        persist
            .Save(PCWSTR(shortcut_wide.as_ptr()), true)
            .map_err(|_| "Windows could not save the app shortcut.".to_owned())
    }?;
    for legacy_name in [
        safe_filename(&format!(
            "{} - {host} [{}]",
            mini_app.name,
            &mini_app.id[..8]
        )),
        safe_filename(&format!("{} - {host}", mini_app.name)),
    ] {
        let legacy = folder_path.join(format!("{legacy_name}.lnk"));
        if legacy != shortcut {
            let _ = fs::remove_file(legacy);
        }
    }
    Ok(())
}

#[cfg(target_os = "windows")]
fn shortcut_filename(
    app: &tauri::AppHandle,
    mini_app: &MiniApp,
    destination: &str,
    folder: &std::path::Path,
    host: &str,
) -> Result<String, String> {
    let data_directory = app_storage_path(app, &mini_app.id)?;
    fs::create_dir_all(&data_directory)
        .map_err(|_| "The app shortcut settings could not be saved.".to_owned())?;
    let marker = data_directory.join(format!("shortcut-name-{destination}.txt"));
    if let Ok(name) = fs::read_to_string(&marker) {
        let name = name.trim();
        if !name.is_empty() && safe_filename(name) == name && name.len() <= 100 {
            return Ok(name.to_owned());
        }
    }

    let mut name = safe_filename(&mini_app.name);
    if name.is_empty() {
        name = "Web app".into();
    }
    if folder.join(format!("{name}.lnk")).exists() {
        name = safe_filename(&format!("{name} ({host})"));
    }
    if folder.join(format!("{name}.lnk")).exists() {
        name = safe_filename(&format!("{name} [{}]", &mini_app.id[..8]));
    }
    let path = folder.join(format!("{name}.lnk"));
    if path.exists() {
        return Err("A shortcut with this app name already exists.".into());
    }
    fs::write(marker, &name)
        .map_err(|_| "The app shortcut settings could not be saved.".to_owned())?;
    Ok(name)
}

#[cfg(target_os = "windows")]
fn remove_windows_shortcuts(app: &tauri::AppHandle, mini_app: &MiniApp) {
    use windows::Win32::System::Com::CoTaskMemFree;
    use windows::Win32::UI::Shell::{FOLDERID_Desktop, FOLDERID_Programs, SHGetKnownFolderPath};
    let host = Url::parse(&mini_app.start_url)
        .ok()
        .and_then(|url| url.host_str().map(str::to_owned))
        .unwrap_or_default();
    let mut filenames = vec![
        format!(
            "{}.lnk",
            safe_filename(&format!(
                "{} - {host} [{}]",
                mini_app.name,
                &mini_app.id[..8]
            ))
        ),
        format!(
            "{}.lnk",
            safe_filename(&format!("{} - {host}", mini_app.name))
        ),
    ];
    if let Ok(directory) = app_storage_path(app, &mini_app.id) {
        for destination in ["desktop", "start_menu"] {
            if let Ok(name) =
                fs::read_to_string(directory.join(format!("shortcut-name-{destination}.txt")))
            {
                let name = name.trim();
                if !name.is_empty() && safe_filename(name) == name && name.len() <= 100 {
                    filenames.push(format!("{name}.lnk"));
                }
            }
        }
    }
    for folder_id in [&FOLDERID_Desktop, &FOLDERID_Programs] {
        let Ok(folder) = (unsafe { SHGetKnownFolderPath(folder_id, Default::default(), None) })
        else {
            continue;
        };
        let folder_path = unsafe {
            let path = folder.to_string();
            CoTaskMemFree(Some(folder.0.cast()));
            path
        };
        let Ok(folder) = folder_path else {
            continue;
        };
        let Ok(entries) = fs::read_dir(folder) else {
            continue;
        };
        for entry in entries.flatten().take(512) {
            let path = entry.path();
            if path.file_name().is_some_and(|name| {
                filenames
                    .iter()
                    .any(|filename| name == std::ffi::OsStr::new(filename))
            }) {
                let _ = fs::remove_file(path);
            }
        }
    }
}

#[cfg(target_os = "windows")]
fn safe_filename(value: &str) -> String {
    value
        .chars()
        .filter(|character| {
            !matches!(
                *character,
                '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*'
            )
        })
        .take(80)
        .collect::<String>()
        .trim()
        .trim_end_matches('.')
        .to_owned()
}

#[cfg(target_os = "windows")]
fn wide(value: &std::ffi::OsStr) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    value.encode_wide().chain(std::iter::once(0)).collect()
}

pub(crate) fn open_from_launch(app: &tauri::AppHandle, id: &str) -> bool {
    if !valid_id(id) {
        return false;
    }
    let Ok(registry) = load_registry() else {
        return false;
    };
    let Some(mini_app) = registry.find(id) else {
        return false;
    };
    let (Ok(start_url), Ok(scope)) = (Url::parse(&mini_app.start_url), Url::parse(&mini_app.scope))
    else {
        return false;
    };
    if start_url.scheme() != "https"
        || !start_url.username().is_empty()
        || start_url.password().is_some()
        || !start_url.host_str().is_some_and(is_public_host)
        || !zephium_miniapps::is_in_scope(&start_url, &scope)
    {
        return false;
    }
    #[cfg(target_os = "windows")]
    {
        let label = format!("miniapp-{id}");
        if let Some(window) = app.get_webview_window(&label) {
            let _ = window.unminimize();
            let _ = window.set_focus();
            return true;
        }
        {
            let mut opening = OPENING_WINDOWS
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if !opening.insert(id.to_owned()) {
                return true;
            }
        }
        let app = app.clone();
        let mini_app = mini_app.clone();
        let id = id.to_owned();
        let opening_id = id.clone();
        let window_app = app.clone();
        let scheduled = app
            .run_on_main_thread(move || {
                let _opening = OpeningWindow(opening_id);
                create_app_window(window_app, mini_app);
            })
            .is_ok();
        if !scheduled {
            OPENING_WINDOWS
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .remove(&id);
        }
        scheduled
    }
    #[cfg(not(target_os = "windows"))]
    app.try_state::<Handle>().is_some_and(|shell| {
        shell.dispatch(Command::OpenUrl {
            input: mini_app.start_url.clone(),
            new_tab: true,
        })
    })
}

fn valid_id(id: &str) -> bool {
    id.len() == 32 && id.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// Mini-app WebViews are the only unprivileged windows allowed to navigate to
/// remote pages. Resolve every navigation against the installed registry so a
/// guessed window label cannot widen the HTTPS scope.
pub(crate) fn allows_navigation(label: &str, url: &Url) -> bool {
    let Some(id) = label.strip_prefix("miniapp-") else {
        return false;
    };
    if !valid_id(id)
        || url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some()
        || !url.host_str().is_some_and(is_public_host)
    {
        return false;
    }
    let Ok(registry) = load_registry() else {
        return false;
    };
    let Some(mini_app) = registry.find(id) else {
        return false;
    };
    Url::parse(&mini_app.scope).is_ok_and(|scope| zephium_miniapps::is_in_scope(url, &scope))
}

/// A shortcut launch shares the browser process and services, but should not
/// reveal the browser's chrome window alongside the standalone app window.
pub(crate) fn launched_standalone() -> bool {
    let id = std::env::args_os()
        .skip(1)
        .collect::<Vec<_>>()
        .windows(2)
        .find_map(|pair| (pair[0] == "--mini-app").then(|| pair[1].to_string_lossy().into_owned()));
    id.is_some_and(|id| {
        valid_id(&id) && load_registry().is_ok_and(|registry| registry.find(&id).is_some())
    })
}

#[cfg(target_os = "windows")]
pub(crate) fn standalone_window_opened() -> bool {
    STANDALONE_WINDOW_OPENED.load(std::sync::atomic::Ordering::Acquire)
}

#[cfg(target_os = "windows")]
pub(crate) fn standalone_launch_finished() -> bool {
    STANDALONE_LAUNCH_FINISHED.load(std::sync::atomic::Ordering::Acquire)
}

#[cfg(target_os = "windows")]
fn create_app_window(app: tauri::AppHandle, mini_app: MiniApp) {
    let standalone_launch = std::env::args_os()
        .skip(1)
        .any(|argument| argument == "--mini-app");
    let opened = create_app_window_inner(app.clone(), &mini_app);
    if standalone_launch {
        finish_standalone_launch(&app, opened);
    }
}

#[cfg(target_os = "windows")]
pub(crate) fn finish_standalone_launch(app: &tauri::AppHandle, opened: bool) {
    STANDALONE_WINDOW_OPENED.store(opened, std::sync::atomic::Ordering::Release);
    STANDALONE_LAUNCH_FINISHED.store(true, std::sync::atomic::Ordering::Release);
    notify_startup_gate(app);
}

#[cfg(target_os = "windows")]
fn create_app_window_inner(app: tauri::AppHandle, mini_app: &MiniApp) -> bool {
    use tauri::webview::NewWindowResponse;
    use tauri::{Manager, WebviewWindowBuilder};

    let label = format!("miniapp-{}", mini_app.id);
    if let Some(window) = app.get_webview_window(&label) {
        let _ = window.unminimize();
        let _ = window.set_focus();
        return true;
    }
    let (Ok(start_url), Ok(scope)) = (Url::parse(&mini_app.start_url), Url::parse(&mini_app.scope))
    else {
        return false;
    };
    let Ok(data_directory) = app_storage_path(&app, &mini_app.id) else {
        return false;
    };
    if fs::create_dir_all(&data_directory).is_err() {
        return false;
    }

    let title = format!("{} — Zephium", mini_app.name);
    let title_for_window = title.clone();
    let first_load = Arc::new(AtomicBool::new(true));
    let builder = WebviewWindowBuilder::new(&app, &label, tauri::WebviewUrl::External(start_url))
        .title(title)
        .visible(false)
        .inner_size(1120.0, 760.0)
        .min_inner_size(480.0, 360.0)
        .resizable(true)
        .focused(false)
        .data_directory(data_directory)
        // Mini apps use a dedicated persistent WebView2 profile. They cannot read
        // browser cookies, local storage, passwords, or extension state.
        .on_navigation({
            let app = app.clone();
            let scope = scope.clone();
            move |url| {
                if zephium_miniapps::is_in_scope(url, &scope) {
                    true
                } else {
                    handoff_to_browser(&app, url);
                    false
                }
            }
        })
        // A page cannot turn its own links or script-created popups into windows
        // with broader navigation or native capabilities.
        .on_new_window({
            let app = app.clone();
            move |url, _| {
                handoff_to_browser(&app, &url);
                NewWindowResponse::Deny
            }
        })
        .on_download(|_, _| false)
        .on_page_load({
            let first_load = first_load.clone();
            move |window, payload| {
                if matches!(payload.event(), tauri::webview::PageLoadEvent::Finished)
                    && first_load.swap(false, Ordering::AcqRel)
                {
                    let _ = window.set_focus();
                }
            }
        })
        .on_document_title_changed(move |window, _| {
            let _ = window.set_title(&title_for_window);
        });

    match builder.build() {
        Ok(window) => {
            let opened = true;
            if let Ok(icon) = image::load_from_memory(include_bytes!("../icons/128x128.png")) {
                let icon = icon.into_rgba8();
                let (width, height) = icon.dimensions();
                let _ = window.set_icon(tauri::image::Image::new_owned(
                    icon.into_raw(),
                    width,
                    height,
                ));
            }
            let app_user_model_id =
                crate::platform::windows_taskbar_icon::windows_app_user_model_id(&mini_app.id);
            let taskbar_icon = app_storage_path(&app, &mini_app.id)
                .ok()
                .map(|path| path.join("icon.ico"))
                .filter(|path| path.is_file());
            crate::platform::windows_taskbar_icon::set_taskbar_icon(
                &window,
                Some(&app_user_model_id),
                taskbar_icon.as_deref(),
            );
            if let Err(error) = window.show() {
                super::write_diagnostic(format_args!(
                    "mini_apps: native mini window could not be shown: {error}"
                ));
                return false;
            }
            opened
        }
        Err(error) => {
            super::write_diagnostic(format_args!(
                "mini_apps: mini window creation failed: {error}"
            ));
            false
        }
    }
}

#[cfg(target_os = "windows")]
fn notify_startup_gate(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        if let Some(gate) = window.try_state::<super::UiStartupGate>() {
            gate.show_if_ready(&window);
        }
    }
}

#[cfg(target_os = "windows")]
fn handoff_to_browser(app: &tauri::AppHandle, url: &Url) {
    if url.scheme() != "https" || !zephium_core::navigation::is_allowed_str(url.as_str()) {
        return;
    }
    let opened = app.try_state::<Handle>().is_some_and(|shell| {
        shell.dispatch(Command::OpenUrl {
            input: url.to_string(),
            new_tab: true,
        })
    });
    if opened {
        crate::external_links::bring_forward(app);
    }
}

fn load_registry() -> Result<Registry, MiniAppError> {
    let raw = APP_STORE
        .get()
        .and_then(|store| store.app_setting(REGISTRY_SETTING))
        .unwrap_or_default();
    Registry::from_json(&raw)
}

fn registry_for_install() -> Result<(Registry, bool), String> {
    let store = APP_STORE
        .get()
        .ok_or_else(|| "The app list could not be read.".to_owned())?;
    match store.app_setting_readback(REGISTRY_SETTING) {
        AppSettingReadback::Missing => Ok((Registry::new(), false)),
        AppSettingReadback::Unavailable => Err(
            "The app list could not be read. Try again after the browser finishes starting.".into(),
        ),
        AppSettingReadback::Value(raw) => {
            match Registry::from_json(&raw) {
                Ok(registry) => Ok((registry, false)),
                Err(MiniAppError::InvalidRegistry | MiniAppError::RegistryTooLarge) => {
                    match store.app_setting_readback(REGISTRY_BACKUP_SETTING) {
                        AppSettingReadback::Missing => {
                            if !store.set_app_setting(REGISTRY_BACKUP_SETTING.into(), raw) {
                                return Err("The damaged app list could not be backed up.".into());
                            }
                        }
                        AppSettingReadback::Value(backup) if backup == raw => {}
                        AppSettingReadback::Value(_) => {
                            return Err("A recovery backup already exists; the app list was left unchanged.".into());
                        }
                        AppSettingReadback::Unavailable => {
                            return Err("The app list could not be backed up.".into());
                        }
                    }
                    Ok((Registry::new(), true))
                }
                Err(error) => Err(installation_error(&error)),
            }
        }
    }
}

fn installation_error(error: &MiniAppError) -> String {
    match error {
        MiniAppError::RegistryTooLarge => {
            "The app list is full. Remove an app before installing another.".into()
        }
        MiniAppError::RegistryFull => {
            "The app list is full. Remove an app before installing another.".into()
        }
        MiniAppError::InvalidJson | MiniAppError::MissingStartUrl => {
            "This page does not offer an installable web app.".into()
        }
        MiniAppError::InsecureUrl | MiniAppError::CredentialsInUrl => {
            "Only public HTTPS web apps can be installed.".into()
        }
        MiniAppError::ScopeOutsideOrigin | MiniAppError::StartOutsideScope => {
            "This app's scope is not contained by its start address.".into()
        }
        MiniAppError::ManifestTooLarge | MiniAppError::InvalidName => {
            "This app's manifest is not usable.".into()
        }
        MiniAppError::NotFound => "The app is no longer installed.".into(),
        MiniAppError::InvalidRegistry => "The app list could not be read.".into(),
    }
}

struct DiscoveredMiniApp {
    app: MiniApp,
    native_icon: Option<image::RgbaImage>,
}

async fn discover_and_parse(raw_url: &str) -> Result<DiscoveredMiniApp, String> {
    let url = Url::parse(raw_url).map_err(|_| "Enter a full HTTPS address.".to_owned())?;
    if url.scheme() != "https" || !url.username().is_empty() || url.password().is_some() {
        return Err("Enter a public HTTPS address.".into());
    }
    if !url.host_str().is_some_and(is_public_host) {
        return Err("Enter a public HTTPS address.".into());
    }
    let (body, effective, content_type) = fetch(&url).await?;
    if looks_like_manifest(&body, &content_type) {
        let mut app =
            parse_manifest(&body, &effective, now()).map_err(|error| installation_error(&error))?;
        let (icon, native_icon) = match app.icon.clone() {
            Some(icon) => fetch_app_icon(vec![icon]).await,
            None => (None, None),
        };
        app.icon = icon;
        return Ok(DiscoveredMiniApp { app, native_icon });
    }
    let html = String::from_utf8_lossy(&body);
    let metadata = site_metadata(&html, &effective);
    let mut app = if let Some(manifest_url) = manifest_href(&html, &effective) {
        match fetch(&manifest_url).await {
            Ok((manifest, manifest_url, _)) => parse_manifest(&manifest, &manifest_url, now())
                .ok()
                .filter(|app| {
                    Url::parse(&app.start_url).is_ok_and(|start| same_origin(&start, &effective))
                })
                .unwrap_or_else(|| site_app(&effective, metadata.name.as_deref())),
            Err(_) => site_app(&effective, metadata.name.as_deref()),
        }
    } else {
        site_app(&effective, metadata.name.as_deref())
    };
    let host_name = effective
        .host_str()
        .unwrap_or_default()
        .trim_start_matches("www.");
    if app.name.eq_ignore_ascii_case(host_name) {
        if let Some(name) = metadata.name.as_deref() {
            app.name = name.to_owned();
        }
    }

    let mut icon_candidates = Vec::with_capacity(5);
    if let Some(icon) = app.icon.clone() {
        icon_candidates.push(icon);
    }
    icon_candidates.extend(metadata.icons);
    if let Ok(favicon) = effective.join("/favicon.ico") {
        if favicon.scheme() == "https" {
            icon_candidates.push(favicon.to_string());
        }
    }
    let (icon, native_icon) = fetch_app_icon(icon_candidates).await;
    app.icon = icon;
    Ok(DiscoveredMiniApp { app, native_icon })
}

fn site_app(page_url: &Url, page_name: Option<&str>) -> MiniApp {
    let mut start = page_url.clone();
    start.set_fragment(None);
    let mut scope = start.clone();
    scope.set_path("/");
    scope.set_query(None);
    scope.set_fragment(None);
    let host = start
        .host_str()
        .unwrap_or("Web app")
        .trim_start_matches("www.");
    let name = page_name.unwrap_or(host).to_owned();
    let start_url = start.to_string();
    MiniApp {
        id: app_id(&start_url),
        name,
        start_url,
        scope: scope.to_string(),
        display: DisplayMode::Standalone,
        theme_color: None,
        background_color: None,
        icon: None,
        installed_at: now(),
    }
}

async fn fetch_app_icon(candidates: Vec<String>) -> (Option<String>, Option<image::RgbaImage>) {
    for candidate in candidates.into_iter().take(5) {
        let Ok(url) = Url::parse(&candidate) else {
            continue;
        };
        let Ok((bytes, _, _)) = fetch_limited(&url, MAX_ICON_BYTES).await else {
            continue;
        };
        let Some(rgba) = crate::favicon_probe::rasterize(&bytes) else {
            continue;
        };
        let native_icon = crate::miniapp_icons::rasterize(&bytes);
        use base64::Engine as _;
        return (
            Some(format!(
                "rgba32:{}",
                base64::engine::general_purpose::STANDARD.encode(rgba)
            )),
            native_icon,
        );
    }
    (None, None)
}

#[cfg(target_os = "windows")]
fn persist_windows_app_icon(app: &tauri::AppHandle, mini_app: &MiniApp, icon: &image::RgbaImage) {
    let Ok(directory) = app_storage_path(app, &mini_app.id) else {
        return;
    };
    if fs::create_dir_all(&directory).is_err() {
        return;
    }
    let Some(ico) = encode_windows_icon(icon) else {
        return;
    };
    let _ = fs::write(directory.join("icon.ico"), ico);
}

#[cfg(target_os = "windows")]
fn encode_windows_icon(icon: &image::RgbaImage) -> Option<Vec<u8>> {
    use image::codecs::ico::{IcoEncoder, IcoFrame};
    use image::ExtendedColorType;

    let frames = [16, 24, 32, 48, 64, 128, 256]
        .into_iter()
        .map(|side| {
            let raster =
                image::imageops::resize(icon, side, side, image::imageops::FilterType::Lanczos3);
            IcoFrame::as_png(raster.as_raw(), side, side, ExtendedColorType::Rgba8).ok()
        })
        .collect::<Option<Vec<_>>>()?;
    let mut output = std::io::Cursor::new(Vec::new());
    IcoEncoder::new(&mut output).encode_images(&frames).ok()?;
    Some(output.into_inner())
}

fn looks_like_manifest(body: &[u8], content_type: &str) -> bool {
    content_type.contains("json")
        || serde_json::from_slice::<serde_json::Value>(body)
            .ok()
            .is_some_and(|value| value.get("start_url").is_some())
}

/// One bounded, HTTPS-only read. Redirects are limited and the effective URL is
/// returned because a manifest's relative addresses resolve against where it
/// actually came from.
async fn fetch(url: &Url) -> Result<(Vec<u8>, Url, String), String> {
    fetch_limited(url, MAX_PAGE_BYTES).await
}

async fn fetch_limited(url: &Url, byte_limit: usize) -> Result<(Vec<u8>, Url, String), String> {
    let mut target = url.clone();
    let mut redirects = 0;
    let mut response = loop {
        if target.scheme() != "https"
            || !target.username().is_empty()
            || target.password().is_some()
            || !target.host_str().is_some_and(is_public_host)
        {
            return Err("Only public HTTPS web apps can be installed.".to_owned());
        }

        // Resolve once, validate every answer, and pin this request to exactly
        // that set. A second DNS lookup by the HTTP client would permit rebinding.
        let host = target
            .host_str()
            .ok_or_else(|| "Only public HTTPS web apps can be installed.".to_owned())?;
        let port = target.port_or_known_default().unwrap_or(443);
        let addresses: Vec<SocketAddr> = tokio::net::lookup_host((host, port))
            .await
            .map_err(|_| "The app could not be reached.".to_owned())?
            .collect();
        if addresses.is_empty() || addresses.iter().any(|address| !is_public_ip(address.ip())) {
            return Err("Only public HTTPS web apps can be installed.".to_owned());
        }

        let client = reqwest::Client::builder()
            .https_only(true)
            // A proxy would resolve the hostname outside this process and
            // bypass the checked, pinned address set.
            .no_proxy()
            .resolve_to_addrs(host, &addresses)
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(3))
            .timeout(Duration::from_secs(10))
            .pool_max_idle_per_host(1)
            .user_agent("Zephium/1.0 (mini apps)")
            .build()
            .map_err(|_| "The install request could not start.".to_owned())?;
        let response = client
            .get(target.clone())
            .header(
                reqwest::header::ACCEPT,
                "application/manifest+json,text/html;q=0.9,*/*;q=0.1",
            )
            .send()
            .await
            .map_err(|_| "The app could not be reached.".to_owned())?;

        if !response.status().is_redirection() {
            break response;
        }
        if redirects >= MAX_REDIRECTS {
            return Err("The app could not be reached.".to_owned());
        }
        let location = response
            .headers()
            .get(reqwest::header::LOCATION)
            .and_then(|value| value.to_str().ok())
            .ok_or_else(|| "The app could not be reached.".to_owned())?;
        target = target
            .join(location)
            .map_err(|_| "The app could not be reached.".to_owned())?;
        redirects += 1;
    };
    if !response.status().is_success() {
        return Err("The app could not be reached.".to_owned());
    }
    let effective = response.url().clone();
    if effective.scheme() != "https" || !effective.host_str().is_some_and(is_public_host) {
        return Err("Only public HTTPS web apps can be installed.".to_owned());
    }
    let content_limit = if response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.contains("manifest"))
    {
        MAX_MANIFEST_BYTES
    } else {
        MAX_PAGE_BYTES
    };
    let limit = byte_limit.min(content_limit);
    if response
        .content_length()
        .is_some_and(|length| length > limit as u64)
    {
        return Err("The app's metadata is too large.".to_owned());
    }
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let mut body = Vec::with_capacity(32 * 1024);
    loop {
        match response.chunk().await {
            Ok(Some(chunk)) => {
                let Some(remaining) = limit.checked_sub(body.len()) else {
                    return Err("The app's metadata is too large.".to_owned());
                };
                if chunk.len() > remaining {
                    return Err("The app's metadata is too large.".to_owned());
                }
                body.extend_from_slice(&chunk);
            }
            Ok(None) => break,
            Err(_) => return Err("The app could not be read.".to_owned()),
        }
    }
    Ok((body, effective, content_type))
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}
