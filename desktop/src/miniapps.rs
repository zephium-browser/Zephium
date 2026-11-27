//! Installable mini web apps.
//!
//! The durable registry is one bounded app setting, which keeps the feature
//! free of a second database. On Windows each installed app receives a
//! standalone WebView2 window and its own persistent data directory; it never
//! shares the browser profile or privileged Tauri capabilities. Navigation is
//! confined to the manifest scope (or the installing page's origin for a site
//! without a manifest), with out-of-scope HTTPS links handed to the browser.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Mutex, RwLock};
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
    app_id, document_base_url, is_public_host, is_public_ip, manifest_href, parse_manifest,
    same_origin, site_metadata, DisplayMode, MiniApp, MiniAppError, Registry, MAX_MANIFEST_BYTES,
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
static NAVIGATION_SCOPES: RwLock<Option<HashMap<String, Url>>> = RwLock::new(None);
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
    let Ok(registry) = load_registry() else {
        refresh_navigation_scopes();
        return Vec::new();
    };
    publish_navigation_scopes(&registry);
    registry.apps().iter().map(MiniAppView::from).collect()
}

#[tauri::command]
#[specta::specta]
pub(crate) fn mini_apps_open(
    caller: WebviewWindow,
    app_handle: tauri::AppHandle,
    id: String,
) -> bool {
    if !authorize(&caller, CallerPolicy::Main, "mini_apps_open") || !valid_id(&id) {
        return false;
    }
    open_from_launch(&app_handle, &id)
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
    let discovered = tokio::time::timeout(Duration::from_secs(30), discover_and_parse(&url))
        .await
        .map_err(|_| "The app took too long to install.".to_owned())??;
    let mut mini_app = discovered.app;
    #[cfg(target_os = "windows")]
    {
        mini_app.storage_id = new_storage_id()?;
    }
    let mut view = MiniAppView::from(&mini_app);
    {
        let _guard = REGISTRY_LOCK
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let (mut registry, recovered) = registry_for_install()?;
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
        view.registry_recovered = recovered;
        publish_navigation_scopes(&registry);
    }
    #[cfg(target_os = "windows")]
    {
        let post_commit = tauri::async_runtime::spawn_blocking(move || {
            if let Some(icon) = discovered.native_icon.as_ref() {
                persist_windows_app_icon(&app_handle, &mini_app, icon);
            }
            let mut failures = Vec::new();
            for destination in ["start_menu", "desktop"] {
                if let Err(error) = create_windows_shortcut(&app_handle, &mini_app, destination) {
                    failures.push(error);
                }
            }
            failures
        })
        .await
        .map_err(|_| "Windows shortcut creation failed.".to_owned())?;
        if !post_commit.is_empty() {
            // The durable install remains authoritative and launchable; the
            // UI can retry shortcut creation without misreporting rollback.
            crate::write_diagnostic(format_args!(
                "mini_apps: installed app but shortcut creation needs retry: {}",
                post_commit.join("; ")
            ));
        }
    }
    #[cfg(not(target_os = "windows"))]
    let _ = (app_handle, discovered.native_icon);
    Ok(view)
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn mini_apps_remove(
    caller: WebviewWindow,
    app_handle: tauri::AppHandle,
    id: String,
) -> bool {
    if !authorize(&caller, CallerPolicy::Main, "mini_apps_remove") || !valid_id(&id) {
        return false;
    }
    tauri::async_runtime::spawn_blocking(move || remove_installed_app(&app_handle, &id))
        .await
        .unwrap_or(false)
}

fn remove_installed_app(app_handle: &tauri::AppHandle, id: &str) -> bool {
    if !valid_id(id) {
        return false;
    }
    let removed = {
        let _guard = REGISTRY_LOCK
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let Ok(mut registry) = load_registry() else {
            return false;
        };
        let Some(removed) = registry.find(id).cloned() else {
            return false;
        };
        if registry.remove(id).is_err() {
            return false;
        }
        let Ok(json) = registry.to_json() else {
            return false;
        };
        let saved = APP_STORE
            .get()
            .is_some_and(|store| store.set_app_setting(REGISTRY_SETTING.into(), json));
        if !saved {
            return false;
        }
        publish_navigation_scopes(&registry);
        removed
    };

    close_app_window(app_handle, &removed.id);
    #[cfg(target_os = "windows")]
    {
        remove_windows_shortcuts(app_handle, &removed);
        remove_app_storage(app_handle, &removed);
    }
    true
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
fn remove_app_storage(app: &tauri::AppHandle, mini_app: &MiniApp) {
    let Ok(path) = app_storage_path(app, mini_app_storage_id(mini_app)) else {
        return;
    };
    // This path uses a validated per-install hex id beneath app data, never a
    // site-controlled path. Reinstallations receive a different directory.
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
    let icon = app_storage_path(app, mini_app_storage_id(mini_app))
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
        property_store
            .Commit()
            .map_err(|_| "Windows could not save the shortcut identity.".to_owned())?;
        let persist: IPersistFile = link
            .cast()
            .map_err(|_| "Windows shortcut storage is unavailable.".to_owned())?;
        let shortcut_wide = wide(shortcut.as_os_str());
        persist
            .Save(PCWSTR(shortcut_wide.as_ptr()), true)
            .map_err(|_| "Windows could not save the app shortcut.".to_owned())
    }?;
    let marker = app_storage_path(app, mini_app_storage_id(mini_app))?
        .join(format!("shortcut-name-{destination}.txt"));
    if fs::write(marker, &filename).is_err() {
        if shortcut_is_owned(&shortcut, mini_app) {
            let _ = fs::remove_file(&shortcut);
        }
        return Err("The app shortcut settings could not be saved.".into());
    }
    for legacy_name in [
        safe_filename(&format!(
            "{} - {host} [{}]",
            mini_app.name,
            &mini_app.id[..8]
        )),
        safe_filename(&format!("{} - {host}", mini_app.name)),
    ] {
        let legacy = folder_path.join(format!("{legacy_name}.lnk"));
        if legacy != shortcut && shortcut_is_owned(&legacy, mini_app) {
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
    let data_directory = app_storage_path(app, mini_app_storage_id(mini_app))?;
    fs::create_dir_all(&data_directory)
        .map_err(|_| "The app shortcut settings could not be saved.".to_owned())?;
    let marker = data_directory.join(format!("shortcut-name-{destination}.txt"));
    if let Ok(name) = fs::read_to_string(&marker) {
        let name = name.trim();
        let marked_path = folder.join(format!("{name}.lnk"));
        if valid_shortcut_name(name)
            && (!marked_path.exists() || shortcut_is_owned(&marked_path, mini_app))
        {
            return Ok(name.to_owned());
        }
    }

    let mut base = safe_filename(&mini_app.name);
    if base.is_empty() {
        base = "Web app".into();
    }
    let candidates = [
        base.clone(),
        safe_filename(&format!("{base} ({host})")),
        safe_filename(&format!("{base} [{}]", &mini_app.id[..8])),
    ];
    candidates
        .into_iter()
        .find(|name| {
            valid_shortcut_name(name)
                && (!folder.join(format!("{name}.lnk")).exists()
                    || shortcut_is_owned(&folder.join(format!("{name}.lnk")), mini_app))
        })
        .ok_or_else(|| "A shortcut with this app name already exists.".to_owned())
}

#[cfg(target_os = "windows")]
fn remove_windows_shortcuts(app: &tauri::AppHandle, mini_app: &MiniApp) {
    use windows::Win32::System::Com::CoTaskMemFree;
    use windows::Win32::UI::Shell::{FOLDERID_Desktop, FOLDERID_Programs, SHGetKnownFolderPath};
    let host = Url::parse(&mini_app.start_url)
        .ok()
        .and_then(|url| url.host_str().map(str::to_owned))
        .unwrap_or_default();
    let legacy_filenames = [
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
    let data_directory = app_storage_path(app, mini_app_storage_id(mini_app)).ok();
    for (folder_id, destination) in [
        (&FOLDERID_Desktop, "desktop"),
        (&FOLDERID_Programs, "start_menu"),
    ] {
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
        let folder = PathBuf::from(folder);
        let mut filenames = Vec::with_capacity(3);
        if let Some(directory) = data_directory.as_ref() {
            let marker = directory.join(format!("shortcut-name-{destination}.txt"));
            if let Ok(name) = fs::read_to_string(marker) {
                let name = name.trim();
                if valid_shortcut_name(name) {
                    filenames.push(format!("{name}.lnk"));
                }
            }
        }
        filenames.extend(legacy_filenames.iter().cloned());
        for filename in filenames {
            let path = folder.join(filename);
            if path.is_file() && shortcut_is_owned(&path, mini_app) {
                let _ = fs::remove_file(path);
            }
        }
    }
}

#[cfg(target_os = "windows")]
fn shortcut_is_owned(path: &std::path::Path, mini_app: &MiniApp) -> bool {
    use std::os::windows::ffi::OsStringExt;
    use windows::core::{Interface, PCWSTR};
    use windows::Win32::Storage::FileSystem::WIN32_FIND_DATAW;
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoUninitialize, IPersistFile, CLSCTX_INPROC_SERVER,
        COINIT_APARTMENTTHREADED, STGM_READ,
    };
    use windows::Win32::UI::Shell::{IShellLinkW, ShellLink, SLGP_RAWPATH};

    if unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }.is_err() {
        return false;
    }
    struct ComApartment;
    impl Drop for ComApartment {
        fn drop(&mut self) {
            unsafe { CoUninitialize() };
        }
    }
    let _apartment = ComApartment;

    let Some(expected) = std::env::current_exe()
        .ok()
        .and_then(|path| std::fs::canonicalize(path).ok())
    else {
        return false;
    };
    let path_wide = wide(path.as_os_str());
    // SAFETY: COM is initialized as an STA on this worker thread. All buffers
    // passed to ShellLink are owned, writable UTF-16 arrays for these calls.
    unsafe {
        let Ok(link) = CoCreateInstance::<_, IShellLinkW>(&ShellLink, None, CLSCTX_INPROC_SERVER)
        else {
            return false;
        };
        let Ok(persist) = link.cast::<IPersistFile>() else {
            return false;
        };
        if persist.Load(PCWSTR(path_wide.as_ptr()), STGM_READ).is_err() {
            return false;
        }
        let mut target = vec![0u16; 32_768];
        if link
            .GetPath(
                &mut target,
                std::ptr::null_mut::<WIN32_FIND_DATAW>(),
                SLGP_RAWPATH.0 as u32,
            )
            .is_err()
        {
            return false;
        }
        let mut arguments = vec![0u16; 4_096];
        if link.GetArguments(&mut arguments).is_err() {
            return false;
        }
        let target_end = target
            .iter()
            .position(|unit| *unit == 0)
            .unwrap_or(target.len());
        let arguments_end = arguments
            .iter()
            .position(|unit| *unit == 0)
            .unwrap_or(arguments.len());
        let target = std::path::PathBuf::from(std::ffi::OsString::from_wide(&target[..target_end]));
        let same_target = std::fs::canonicalize(target).is_ok_and(|target| {
            target
                .to_string_lossy()
                .eq_ignore_ascii_case(&expected.to_string_lossy())
        });
        let arguments = String::from_utf16_lossy(&arguments[..arguments_end]);
        same_target
            && arguments.split_whitespace().collect::<Vec<_>>()
                == ["--mini-app", mini_app.id.as_str()]
    }
}

#[cfg(target_os = "windows")]
fn safe_filename(value: &str) -> String {
    let mut result = String::new();
    let mut utf16_len = 0;
    for character in value.chars().filter(|character| {
        !matches!(
            *character,
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*'
        )
    }) {
        utf16_len += character.len_utf16();
        if utf16_len > 64 {
            break;
        }
        result.push(character);
    }
    result.trim().trim_end_matches('.').to_owned()
}

#[cfg(target_os = "windows")]
fn valid_shortcut_name(value: &str) -> bool {
    !value.is_empty() && safe_filename(value) == value && value.encode_utf16().count() <= 64
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
        let guard_id = opening_id.clone();
        let window_app = app.clone();
        let scheduled = app
            .run_on_main_thread(move || {
                let _opening = OpeningWindow(guard_id);
                let Ok(registry) = load_registry() else {
                    return;
                };
                if registry.find(&opening_id).is_some_and(|current| {
                    current.id == mini_app.id && current.storage_id == mini_app.storage_id
                }) {
                    create_app_window(window_app, mini_app);
                }
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

#[cfg(target_os = "windows")]
fn mini_app_storage_id(mini_app: &MiniApp) -> &str {
    if valid_id(&mini_app.storage_id) {
        &mini_app.storage_id
    } else {
        &mini_app.id
    }
}

#[cfg(target_os = "windows")]
fn new_storage_id() -> Result<String, String> {
    let mut random = [0u8; 16];
    getrandom::fill(&mut random)
        .map_err(|_| "A private app storage identity could not be created.".to_owned())?;
    Ok(random.iter().map(|byte| format!("{byte:02x}")).collect())
}

/// Mini-app WebViews are the only unprivileged windows allowed to navigate to
/// remote pages. Resolve every navigation against the installed registry so a
/// guessed window label cannot widen the HTTPS scope.
pub(crate) fn allows_navigation(label: &str, url: &Url) -> bool {
    let Some(id) = label.strip_prefix("miniapp-") else {
        return false;
    };
    if !valid_id(id) {
        return false;
    }
    let scopes = NAVIGATION_SCOPES
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    navigation_scope_allows(scopes.as_ref().and_then(|scopes| scopes.get(id)), url)
}

fn navigation_scope_allows(scope: Option<&Url>, url: &Url) -> bool {
    url.scheme() == "https"
        && url.username().is_empty()
        && url.password().is_none()
        && url.host_str().is_some_and(is_public_host)
        && scope.is_some_and(|scope| zephium_miniapps::is_in_scope(url, scope))
}

/// A shortcut launch shares the browser process and services, but should not
/// reveal the browser's chrome window alongside the standalone app window.
#[cfg(target_os = "windows")]
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
    let Ok(data_directory) = app_storage_path(&app, mini_app_storage_id(mini_app)) else {
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
            let taskbar_icon = app_storage_path(&app, mini_app_storage_id(mini_app))
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
                let _ = window.close();
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

pub(crate) fn refresh_navigation_scopes() {
    let scopes = load_registry().ok();
    let scopes = scopes.map(|registry| navigation_scopes(&registry));
    *NAVIGATION_SCOPES
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = scopes;
}

fn publish_navigation_scopes(registry: &Registry) {
    *NAVIGATION_SCOPES
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(navigation_scopes(registry));
}

fn navigation_scopes(registry: &Registry) -> HashMap<String, Url> {
    registry
        .apps()
        .iter()
        .filter_map(|app| {
            Url::parse(&app.scope)
                .ok()
                .map(|scope| (app.id.clone(), scope))
        })
        .collect()
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
    let document_base = document_base_url(&html, &effective);
    let metadata = site_metadata(&html, &document_base);
    let mut app = if let Some(manifest_url) = manifest_href(&html, &document_base) {
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
    if start.query().is_some_and(str::is_empty) {
        start.set_query(None);
    }
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
        storage_id: String::new(),
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
        let Some((rgba, native_icon)) = crate::miniapp_icons::rasterize(&bytes) else {
            continue;
        };
        use base64::Engine as _;
        return (
            Some(format!(
                "rgba32:{}",
                base64::engine::general_purpose::STANDARD.encode(rgba)
            )),
            Some(native_icon),
        );
    }
    (None, None)
}

#[cfg(target_os = "windows")]
fn persist_windows_app_icon(app: &tauri::AppHandle, mini_app: &MiniApp, icon: &image::RgbaImage) {
    let Ok(directory) = app_storage_path(app, mini_app_storage_id(mini_app)) else {
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
        let addresses: Vec<SocketAddr> = tokio::time::timeout(
            Duration::from_secs(8),
            tokio::net::lookup_host((host, port)),
        )
        .await
        .map_err(|_| "The app could not be reached.".to_owned())?
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

#[cfg(test)]
mod tests {
    use super::navigation_scope_allows;
    use reqwest::Url;

    #[test]
    fn navigation_requires_available_scope_public_https_and_scope_membership() {
        let scope = Url::parse("https://example.com/app/").unwrap();
        assert!(navigation_scope_allows(
            Some(&scope),
            &Url::parse("https://example.com/app/page").unwrap()
        ));
        assert!(!navigation_scope_allows(
            Some(&scope),
            &Url::parse("https://example.com/outside").unwrap()
        ));
        assert!(!navigation_scope_allows(
            None,
            &Url::parse("https://example.com/app/page").unwrap()
        ));
        assert!(!navigation_scope_allows(
            Some(&scope),
            &Url::parse("http://example.com/app/page").unwrap()
        ));
        assert!(!navigation_scope_allows(
            Some(&scope),
            &Url::parse("https://user@example.com/app/page").unwrap()
        ));
        assert!(!navigation_scope_allows(
            Some(&scope),
            &Url::parse("https://127.0.0.1/app/page").unwrap()
        ));
    }
}
