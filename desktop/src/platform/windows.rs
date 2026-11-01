#[path = "windows_caption.rs"]
mod caption;
#[path = "windows_menus.rs"]
mod menus;
#[path = "windows_renderer.rs"]
pub(crate) mod renderer;
#[path = "windows_sidebar_edge.rs"]
mod sidebar_edge;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use tauri::{Manager, WebviewWindow};
use windows::core::Interface;

use crate::startup_alert::{runtime_failure, StartupFailure, StartupProblem};

use zephium_app::{
    ChromePresentation, ChromePresentationCallback, ChromePresentationDispatch, PresentationChrome,
    SharedChrome,
};
use zephium_core::geometry::Size;
use zephium_core::ports::chrome::{Chrome, ChromeFrame};
use zephium_engine::MainThreadDispatch;

static MATERIALS: std::sync::LazyLock<
    std::sync::Mutex<std::collections::HashMap<String, crate::material::Material>>,
> = std::sync::LazyLock::new(|| std::sync::Mutex::new(std::collections::HashMap::new()));

const PAGE_URL_UTF16_LIMIT: usize = 8 * 1_024;
const PAGE_URL_UTF8_LIMIT: usize = 8 * 1_024;
const WINDOWS_PATH_UTF16_LIMIT: usize = 32_767;
const WINDOWS_PATH_UTF8_LIMIT: usize = 4 * WINDOWS_PATH_UTF16_LIMIT;
const VERSION_UTF16_LIMIT: usize = 256;
const VERSION_UTF8_LIMIT: usize = 256;

pub type RuntimeUpdateCallback = Arc<dyn Fn() + Send + Sync + 'static>;

fn take_pwstr_bounded(
    source: windows::core::PWSTR,
    max_utf16_units: usize,
    max_utf8_bytes: usize,
) -> Option<String> {
    // WebView2 transfers out-strings with CoTaskMem ownership. Keep that
    // allocation under RAII while scanning only the policy-bounded prefix;
    // malformed, unterminated, or oversized callback data fails closed
    // without first allocating an attacker-sized Rust String.
    let source = webview2_com::CoTaskMemPWSTR::from(source);
    let pointer = source.as_ref().as_pcwstr().as_ptr();
    if pointer.is_null() {
        return Some(String::new());
    }
    let mut length = 0;
    while length <= max_utf16_units {
        // SAFETY: WebView2's out-string contract requires a NUL terminator.
        // The scan is capped at the policy maximum plus one distinguishing
        // unit, and `source` retains ownership for the complete conversion.
        if unsafe { pointer.add(length).read() } == 0 {
            // SAFETY: the bounded scan established this initialized prefix.
            let units = unsafe { std::slice::from_raw_parts(pointer, length) };
            let value = String::from_utf16(units).ok()?;
            return (value.len() <= max_utf8_bytes).then_some(value);
        }
        length += 1;
    }
    None
}

pub fn remove_privileged_version_observer(label: &str) -> bool {
    zephium_engine::detach_privileged_environment_update(label)
}

pub fn finalize_privileged_environment_observers(expected_labels: &[&str]) -> bool {
    zephium_engine::finalize_privileged_environment_registrations(expected_labels)
}

/// Starts the log before anything can fail, in the folder Tauri will resolve
/// as the app data directory: a launch that stops before setup (a refused
/// runtime, a failed build) would otherwise leave nothing to read.
pub fn open_startup_log(identifier: &str) {
    use std::os::windows::ffi::OsStringExt;
    use windows::Win32::System::Com::CoTaskMemFree;
    use windows::Win32::UI::Shell::{
        FOLDERID_RoamingAppData, SHGetKnownFolderPath, KF_FLAG_DEFAULT,
    };

    // SAFETY: the returned string is CoTaskMem-owned, read before it is freed.
    let roaming = unsafe {
        let Ok(folder) = SHGetKnownFolderPath(&FOLDERID_RoamingAppData, KF_FLAG_DEFAULT, None)
        else {
            return;
        };
        let path = std::path::PathBuf::from(std::ffi::OsString::from_wide(folder.as_wide()));
        CoTaskMemFree(Some(folder.0.cast()));
        path
    };
    let directory = roaming.join(identifier);
    if std::fs::create_dir_all(&directory).is_err() {
        return;
    }
    redirect_stderr(&directory);
    crate::diagnostics::install(directory);
}

/// Query the runtime selected by WebView2 before Tauri creates any controller.
/// Capability checks still run per-view; an outdated runtime is reported as
/// an update advisory instead of refused.
pub fn enforce_runtime_security_floor(
) -> Result<zephium_core::runtime_security::RuntimeSecurityAdvisories, StartupFailure> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs());
    if let Some(name) = zephium_core::webview2::first_present_environment_override(|name| {
        std::env::var_os(name).is_some()
    }) {
        return Err(StartupFailure::new(StartupProblem::WebViewOverride, name));
    }
    let reported = tauri::webview_version().map_err(|error| {
        let error = error.to_string();
        // The loader finds no runtime at all with ERROR_FILE_NOT_FOUND.
        let problem = if error.to_ascii_lowercase().contains("0x80070002") {
            StartupProblem::WebViewMissing
        } else {
            StartupProblem::Other
        };
        StartupFailure::new(
            problem,
            format!("cannot query the selected WebView2 runtime: {error}"),
        )
    })?;
    crate::write_diagnostic(format_args!(
        "startup: WebView2 runtime selected at launch: {reported}"
    ));
    let (_, advisory) = zephium_core::webview2::assess_runtime(&reported, now)
        .map_err(|error| runtime_failure(&reported, error))?;
    Ok(advisory)
}

#[path = "windows_log.rs"]
mod windows_log;
pub use windows_log::redirect_stderr;

pub fn init(
    window: &WebviewWindow,
    expected_user_data_folder: &Path,
    on_runtime_update: RuntimeUpdateCallback,
) -> Result<(), StartupFailure> {
    round_corners(window);
    if !menus::install(window) {
        crate::write_diagnostic(format_args!("chrome: using system menu rendering"));
    }
    if caption::install(window) {
        bridge_caption_keys(window);
    } else {
        // A visual enhancement must never remove access to window controls.
        let _ = window.set_decorations(true);
    }
    let hardened = harden_privileged(window, expected_user_data_folder, on_runtime_update);
    apply_material(window, true);
    hardened
}

/// F10 and Alt+Space typed into the chrome WebView never reach the window, so
/// the caption hears them from the WebView's accelerator hook instead.
fn bridge_caption_keys(window: &WebviewWindow) {
    let scheduled = window.with_webview(|webview| {
        if let Err(error) = zephium_engine::bridge_caption_keys(&webview.controller()) {
            crate::write_diagnostic(format_args!(
                "caption: keyboard bridge unavailable: {error}"
            ));
        }
    });
    if scheduled.is_err() {
        crate::write_diagnostic(format_args!(
            "caption: keyboard bridge could not be scheduled"
        ));
    }
}

/// Tauri does not expose Wry's permission callback. Install the WebView2
/// policy directly for privileged chrome so an XSS cannot prompt for ambient
/// capabilities or persist password/form suggestions in its shared UDF.
pub fn harden_privileged(
    window: &WebviewWindow,
    expected_user_data_folder: &Path,
    on_runtime_update: RuntimeUpdateCallback,
) -> Result<(), StartupFailure> {
    let installed = Arc::new(AtomicBool::new(false));
    let completed = installed.clone();
    let refused = Arc::new(Mutex::new(None::<StartupFailure>));
    let failure = refused.clone();
    let expected_user_data_folder = expected_user_data_folder.to_owned();
    let label = window.label().to_owned();
    let app = window.app_handle().clone();
    let scheduled = window.with_webview(move |webview| unsafe {
        use webview2_com::Microsoft::Web::WebView2::Win32::{
            ICoreWebView2Environment10, ICoreWebView2Environment7, ICoreWebView2Settings3,
            ICoreWebView2Settings4, ICoreWebView2Settings5, ICoreWebView2_10, ICoreWebView2_13,
            ICoreWebView2_18,
            ICoreWebView2_25, ICoreWebView2_5, COREWEBVIEW2_PERMISSION_STATE_DENY,
        };
        use webview2_com::{
            BasicAuthenticationRequestedEventHandler, ClientCertificateRequestedEventHandler,
            LaunchingExternalUriSchemeEventHandler, NavigationStartingEventHandler,
            PermissionRequestedEventHandler, SaveAsUIShowingEventHandler,
        };
        use windows::core::{BOOL, PWSTR};

        let mut runtime = None::<String>;
        let result = (|| -> Result<(), StartupFailure> {
            let environment = webview.environment();
            let mut reported_version = PWSTR::null();
            environment.BrowserVersionString(&mut reported_version)?;
            let reported_version = take_pwstr_bounded(
                reported_version,
                VERSION_UTF16_LIMIT,
                VERSION_UTF8_LIMIT,
            )
            .ok_or_else(|| {
                windows::core::Error::new(
                    windows::Win32::Foundation::E_INVALIDARG,
                    "privileged WebView2 version is malformed or exceeds the native string bound",
                )
            })?;
            // This is the runtime the environment was actually created on,
            // which the launch-time query can disagree with; both refuse
            // through the same admission and the same explanation.
            runtime = Some(reported_version.clone());
            crate::write_diagnostic(format_args!(
                "startup: privileged WebView2 environment runs on {reported_version}"
            ));
            zephium_core::webview2::admit_runtime(&reported_version)
                .map_err(|error| runtime_failure(&reported_version, error))?;

            let environment7 = require::<ICoreWebView2Environment7>(&environment, "Environment7")?;
            let mut actual_user_data_folder = PWSTR::null();
            environment7.UserDataFolder(&mut actual_user_data_folder)?;
            let actual_user_data_folder = take_pwstr_bounded(
                actual_user_data_folder,
                WINDOWS_PATH_UTF16_LIMIT,
                WINDOWS_PATH_UTF8_LIMIT,
            )
            .ok_or_else(|| {
                windows::core::Error::new(
                    windows::Win32::Foundation::E_INVALIDARG,
                    "privileged WebView2 user-data folder is malformed or exceeds the Windows path bound",
                )
            })?;
            let matches = zephium_core::webview2::user_data_directory_matches(
                &expected_user_data_folder,
                Path::new(&actual_user_data_folder),
            )
            .map_err(|error| {
                windows::core::Error::new(
                    windows::Win32::Foundation::E_ACCESSDENIED,
                    format!(
                        "cannot verify actual privileged WebView2 user-data folder {actual_user_data_folder:?} against {}: {error}",
                        expected_user_data_folder.display()
                    ),
                )
            })?;
            if !matches {
                return Err(windows::core::Error::new(
                    windows::Win32::Foundation::E_ACCESSDENIED,
                    format!(
                        "actual privileged WebView2 user-data folder {actual_user_data_folder:?} does not match {}",
                        expected_user_data_folder.display()
                    ),
                )
                .into());
            }

            // Wry silently falls back to a non-private controller when the
            // runtime predates Environment10. Refuse that runtime instead.
            let _environment10 = require::<ICoreWebView2Environment10>(&environment, "Environment10")?;
            let core = webview.controller().CoreWebView2()?;
            renderer::install(&core, app, label.clone())?;
            // External URI launch interception is only exposed by
            // ICoreWebView2_18. Treat that interface as part of the mandatory
            // runtime floor instead of silently accepting a weaker runtime.
            let core18 = require::<ICoreWebView2_18>(&core, "ICoreWebView2_18")?;
            let profile = require::<ICoreWebView2_13>(&core, "ICoreWebView2_13")?.Profile()?;
            let mut in_private = windows::core::BOOL::default();
            profile.IsInPrivateModeEnabled(&mut in_private)?;
            if !in_private.as_bool() {
                return Err(StartupFailure::new(
                    StartupProblem::Other,
                    "privileged WebView2 profile is not InPrivate",
                ));
            }
            let settings = core.Settings()?;
            settings.SetAreDefaultScriptDialogsEnabled(false)?;
            settings.SetAreDefaultContextMenusEnabled(false)?;
            // Privileged chrome has no reason to invoke WebView2's browser UI
            // accelerators (notably Ctrl+P). Require Settings3 instead of
            // accepting an older runtime with an unbrokered native print UI,
            // and verify the native postcondition before exposing the view.
            let settings3 = require::<ICoreWebView2Settings3>(&settings, "Settings3")?;
            settings3.SetAreBrowserAcceleratorKeysEnabled(false)?;
            let mut browser_accelerators_enabled = BOOL::default();
            settings3.AreBrowserAcceleratorKeysEnabled(&mut browser_accelerators_enabled)?;
            if browser_accelerators_enabled.as_bool() {
                return Err(windows::core::Error::new(
                    windows::Win32::Foundation::E_ACCESSDENIED,
                    "privileged WebView2 browser accelerators remained enabled",
                )
                .into());
            }
            zephium_engine::configure_windows_reputation_checking(&core)?;
            let settings4 = require::<ICoreWebView2Settings4>(&settings, "Settings4")?;
            settings4.SetIsPasswordAutosaveEnabled(false)?;
            settings4.SetIsGeneralAutofillEnabled(false)?;
            let mut password_autosave_enabled = BOOL::default();
            let mut general_autofill_enabled = BOOL::default();
            settings4.IsPasswordAutosaveEnabled(&mut password_autosave_enabled)?;
            settings4.IsGeneralAutofillEnabled(&mut general_autofill_enabled)?;
            if password_autosave_enabled.as_bool() || general_autofill_enabled.as_bool() {
                return Err(windows::core::Error::new(
                    windows::Win32::Foundation::E_ACCESSDENIED,
                    "privileged WebView2 password/autofill surfaces remained enabled",
                )
                .into());
            }
            // The launcher is a fixed-size surface whose window clips to its
            // measured height. Ctrl+wheel or a pinch would scale the page
            // inside that clip and cut its footer off.
            if label == crate::overlay::PANEL_LABEL {
                settings.SetIsZoomControlEnabled(false)?;
                require::<ICoreWebView2Settings5>(&settings, "Settings5")?
                    .SetIsPinchZoomEnabled(false)?;
            }

            let mut token = 0_i64;
            core.add_PermissionRequested(
                &PermissionRequestedEventHandler::create(Box::new(|_, args| {
                    if let Some(args) = args {
                        args.SetState(COREWEBVIEW2_PERMISSION_STATE_DENY)?;
                    }
                    Ok(())
                })),
                &mut token,
            )?;

            let core10 = require::<ICoreWebView2_10>(&core, "ICoreWebView2_10")?;
            let mut basic_auth_token = 0_i64;
            core10.add_BasicAuthenticationRequested(
                &BasicAuthenticationRequestedEventHandler::create(Box::new(|_, args| {
                    if let Some(args) = args {
                        args.SetCancel(true)?;
                    }
                    Ok(())
                })),
                &mut basic_auth_token,
            )?;

            let core5 = require::<ICoreWebView2_5>(&core, "ICoreWebView2_5")?;
            let mut client_certificate_token = 0_i64;
            core5.add_ClientCertificateRequested(
                &ClientCertificateRequestedEventHandler::create(Box::new(|_, args| {
                    if let Some(args) = args {
                        args.SetCancel(true)?;
                        args.SetHandled(true)?;
                    }
                    Ok(())
                })),
                &mut client_certificate_token,
            )?;

            // Save As is an independent native surface; context-menu,
            // accelerator, PDF-toolbar and download suppression do not cover
            // it. Cancellation is set before suppressing the default dialog
            // so a later HRESULT failure remains fail-closed.
            let core25 = require::<ICoreWebView2_25>(&core, "ICoreWebView2_25")?;
            let mut save_as_token = 0_i64;
            core25.add_SaveAsUIShowing(
                &SaveAsUIShowingEventHandler::create(Box::new(|_, args| {
                    if let Some(args) = args {
                        args.SetCancel(true)?;
                        args.SetSuppressDefaultDialog(true)?;
                    }
                    Ok(())
                })),
                &mut save_as_token,
            )?;

            // Tauri's navigation hook covers the main document, while
            // WebView2 routes subframes through this separate native event.
            // Default to cancellation so a malformed/unreadable URI fails
            // closed; only the exact privileged app origin may be framed.
            let mut frame_token = 0_i64;
            core.add_FrameNavigationStarting(
                &NavigationStartingEventHandler::create(Box::new(|_, args| {
                    let Some(args) = args else {
                        return Ok(());
                    };
                    args.SetCancel(true)?;
                    let mut uri = PWSTR::null();
                    args.Uri(&mut uri)?;
                    let Some(uri) = take_pwstr_bounded(
                        uri,
                        PAGE_URL_UTF16_LIMIT,
                        PAGE_URL_UTF8_LIMIT,
                    ) else {
                        return Ok(());
                    };
                    if tauri::Url::parse(&uri)
                        .ok()
                        .as_ref()
                        .is_some_and(crate::ui_navigation_allowed)
                    {
                        args.SetCancel(false)?;
                    }
                    Ok(())
                })),
                &mut frame_token,
            )?;

            // A custom protocol may launch another application without a
            // NavigationStarting callback. Privileged chrome has no reason to
            // invoke OS protocol handlers, so deny every request.
            let mut external_token = 0_i64;
            core18.add_LaunchingExternalUriScheme(
                &LaunchingExternalUriSchemeEventHandler::create(Box::new(|_, args| {
                    if let Some(args) = args {
                        args.SetCancel(true)?;
                    }
                    Ok(())
                })),
                &mut external_token,
            )?;
            zephium_engine::install_privileged_environment_registration(
                label,
                &environment,
                on_runtime_update,
            )?;
            Ok(())
        })();
        if let Err(error) = result {
            let error = match &runtime {
                Some(reported) => error.on_runtime(reported),
                None => error,
            };
            crate::write_diagnostic(format_args!(
                "security: privileged WebView2 hardening failed: {error}"
            ));
            *failure.lock().unwrap_or_else(|e| e.into_inner()) = Some(error);
            return;
        }
        completed.store(true, Ordering::Release);
    });
    if scheduled.is_err() {
        return Err(StartupFailure::new(
            StartupProblem::Other,
            "the browser window was not ready to be set up",
        ));
    }
    if let Some(failure) = refused.lock().unwrap_or_else(|e| e.into_inner()).take() {
        return Err(failure);
    }
    if installed.load(Ordering::Acquire) {
        Ok(())
    } else {
        Err(StartupFailure::new(
            StartupProblem::Other,
            "the browser window's set-up did not run",
        ))
    }
}

/// The interface, or the missing feature named in the error: a runtime
/// that lacks one is too old for Zephium rather than broken.
fn require<T: Interface>(source: &impl Interface, feature: &str) -> windows::core::Result<T> {
    source.cast::<T>().map_err(|error| {
        windows::core::Error::new(
            error.code(),
            format!("this WebView2 runtime does not provide {feature}"),
        )
    })
}

impl From<windows::core::Error> for StartupFailure {
    fn from(error: windows::core::Error) -> Self {
        let problem = if error.code() == windows::Win32::Foundation::E_NOINTERFACE {
            StartupProblem::WebViewOutdated
        } else {
            StartupProblem::Other
        };
        Self::new(problem, error.to_string())
    }
}

/// Keeps the window's own caption out of a page that fills the screen.
pub fn set_caption_suppressed(window: &WebviewWindow, suppressed: bool) {
    caption::set_suppressed(window, caption::Suppression::Content, suppressed);
}

/// Keeps the caption out of a window the person made fullscreen themselves.
pub fn set_caption_window_fullscreen(window: &WebviewWindow, fullscreen: bool) {
    caption::set_suppressed(window, caption::Suppression::Window, fullscreen);
}

// Undecorated windows lose DWM rounding unless asked for explicitly.
pub fn round_corners(window: &WebviewWindow) {
    use windows::Win32::Graphics::Dwm::{
        DwmSetWindowAttribute, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND,
    };
    let Ok(hwnd) = window.hwnd() else {
        return;
    };
    let hwnd = windows::Win32::Foundation::HWND(hwnd.0);
    let pref = DWMWCP_ROUND;
    let _ = unsafe {
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE,
            &pref as *const _ as *const _,
            std::mem::size_of_val(&pref) as u32,
        )
    };
}

/// System backdrops take their light or dark rendition from the frame, not
/// from the app, so a dark interface over a light-framed Mica reads grey.
fn set_dark_frame(window: &WebviewWindow, dark: bool) {
    use windows::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWA_USE_IMMERSIVE_DARK_MODE};
    let Ok(hwnd) = window.hwnd() else {
        return;
    };
    let value = i32::from(dark);
    // SAFETY: a plain write of a live window's DWM attribute.
    let _ = unsafe {
        DwmSetWindowAttribute(
            windows::Win32::Foundation::HWND(hwnd.0),
            DWMWA_USE_IMMERSIVE_DARK_MODE,
            &value as *const _ as *const _,
            std::mem::size_of_val(&value) as u32,
        )
    };
}

/// The browser window wears Mica Alt, the backdrop Windows 11 made for tabbed
/// main windows: it takes the wallpaper's hue without its detail and stays
/// put while the window is inactive. The frame lays the sky's light over it
/// on a new tab. Acrylic is a flyout's material, so only the launcher keeps
/// it, and Windows 10, which has no Mica, falls back to it.
pub fn apply_material(window: &WebviewWindow, dark: bool) -> bool {
    use crate::material::Material;
    let launcher = window.label() == crate::overlay::PANEL_LABEL;
    // Win10 draws this tint; 22H2 and later ignore it and the frame's wash
    // carries the theme instead.
    let tint = match crate::material::themed_ground(!dark) {
        Some([r, g, b]) => (r, g, b, 96),
        None if dark => (24, 24, 28, 96),
        None => (245, 245, 249, 96),
    };
    set_dark_frame(window, dark);
    caption::set_dark(window, dark);
    // Use the fallible native adapter directly; the Tauri effect dispatcher
    // discards the underlying DWM error and can falsely report acrylic.
    let preference = crate::material::preference();
    let contrast = menus::high_contrast() || preference == crate::material::Preference::None;
    let result = if contrast {
        let _ = window_vibrancy::clear_tabbed(window);
        window_vibrancy::clear_acrylic(window).map(|()| Material::None)
    } else if launcher {
        window_vibrancy::apply_acrylic(window, Some(tint)).map(|()| Material::Acrylic)
    } else {
        // Each backdrop replaces the other, but a Win10 fallback leaves the
        // accent policy set, which would composite under Mica.
        let _ = window_vibrancy::clear_acrylic(window);
        window_vibrancy::apply_tabbed(window, Some(dark))
            .map(|()| Material::Mica)
            .or_else(|_| {
                window_vibrancy::apply_acrylic(window, Some(tint)).map(|()| Material::Acrylic)
            })
    };
    if let Err(e) = &result {
        crate::write_diagnostic(format_args!("material: window effects unavailable: {e}"));
    }
    let material = result.as_ref().copied().unwrap_or(Material::None);
    let first = MATERIALS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(window.label().to_owned(), material)
        .is_none();
    if first {
        let label = window.label().to_owned();
        window.on_window_event(move |event| {
            if matches!(event, tauri::WindowEvent::Destroyed) {
                MATERIALS
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .remove(&label);
            }
        });
    }
    if let Ok(value) = serde_json::to_string(&material) {
        let _ = window.eval(format!("window.dispatchEvent(new CustomEvent('zephium:ui-command',{{detail:'material.'+{value}}}))"));
    }
    result.is_ok()
}

pub fn material(label: &str) -> crate::material::Material {
    MATERIALS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(label)
        .copied()
        .unwrap_or_default()
}

// Full-window chrome: client coords already are window coords.
pub fn to_window(x: f64, y: f64) -> (f64, f64) {
    (x, y)
}

#[derive(Default)]
struct ChromeVisibility {
    hidden: bool,
    minimized: bool,
    // The memory target the frame had asked for before the window was
    // minimized, given back on restore.
    memory: Option<
        webview2_com::Microsoft::Web::WebView2::Win32::COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL,
    >,
}

thread_local! {
    static CHROME_VISIBILITY: std::cell::RefCell<std::collections::HashMap<String, ChromeVisibility>> =
        std::cell::RefCell::new(std::collections::HashMap::new());
}

/// Hides or shows the chrome WebView while its window stays on screen, so the
/// window shows only its material, never a document that is still loading.
pub fn set_chrome_hidden(window: &WebviewWindow, hidden: bool) -> bool {
    let applied = Arc::new(AtomicBool::new(false));
    let completed = applied.clone();
    let label = window.label().to_owned();
    let dispatched = window
        .with_webview(move |webview| {
            let visible = CHROME_VISIBILITY.with(|views| {
                let mut views = views.borrow_mut();
                let view = views.entry(label).or_default();
                view.hidden = hidden;
                !view.hidden && !view.minimized
            });
            // SAFETY: Tauri supplies the live controller for the duration of
            // this UI-thread callback.
            if unsafe { webview.controller().SetIsVisible(visible) }.is_err() {
                crate::write_diagnostic(format_args!("chrome: WebView2 visibility was refused"));
            } else {
                completed.store(true, Ordering::Release);
            }
        })
        .is_ok();
    dispatched && applied.load(Ordering::Acquire)
}

/// WebView2 keeps a minimized window's chrome painting and its timers
/// running. Hidden, it stops both, and its memory target drops until restore.
pub fn set_chrome_minimized(window: &WebviewWindow, minimized: bool) {
    use webview2_com::Microsoft::Web::WebView2::Win32::{
        ICoreWebView2_19, COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL,
        COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_LOW,
    };
    let label = window.label().to_owned();
    let _ = window.with_webview(move |webview| {
        // No borrow is held across WebView2 calls, which can pump messages.
        let visible = CHROME_VISIBILITY.with(|views| {
            let mut views = views.borrow_mut();
            let view = views.entry(label.clone()).or_default();
            (view.minimized != minimized).then(|| {
                view.minimized = minimized;
                !view.hidden && !minimized
            })
        });
        let Some(visible) = visible else {
            return;
        };
        // SAFETY: Tauri supplies the live controller for the duration of this
        // UI-thread callback.
        unsafe {
            let controller = webview.controller();
            let _ = controller.SetIsVisible(visible);
            let Ok(memory) = controller
                .CoreWebView2()
                .and_then(|core| core.cast::<ICoreWebView2_19>())
            else {
                return;
            };
            if minimized {
                let mut level = COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL::default();
                if memory.MemoryUsageTargetLevel(&mut level).is_ok()
                    && memory
                        .SetMemoryUsageTargetLevel(COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_LOW)
                        .is_ok()
                {
                    CHROME_VISIBILITY.with(|views| {
                        if let Some(view) = views.borrow_mut().get_mut(&label) {
                            view.memory = Some(level);
                        }
                    });
                }
            } else if let Some(level) = CHROME_VISIBILITY.with(|views| {
                views
                    .borrow_mut()
                    .get_mut(&label)
                    .and_then(|view| view.memory.take())
            }) {
                let _ = memory.SetMemoryUsageTargetLevel(level);
            }
        }
    });
}

thread_local! {
    /// The windows whose sidebar is hidden. The caption's reveal begins where
    /// the sidebar's card ends, not where the chrome behind the page does.
    static HIDDEN_SIDEBARS: std::cell::RefCell<std::collections::HashSet<String>> =
        std::cell::RefCell::new(std::collections::HashSet::new());
}

/// The sidebar is hidden or back: the pointer is read for its left edge
/// while it is away.
pub fn sidebar_hidden(window: &WebviewWindow, hidden: bool) {
    let owner = window.clone();
    let _ = window.run_on_main_thread(move || {
        HIDDEN_SIDEBARS.with(|windows| {
            let mut windows = windows.borrow_mut();
            if hidden {
                windows.insert(owner.label().to_owned());
            } else {
                windows.remove(owner.label());
            }
        });
        sidebar_edge::watch(&owner, hidden);
    });
}

pub fn make_chrome(window: &WebviewWindow, _dispatch: MainThreadDispatch) -> SharedChrome {
    Arc::new(ChromeAdapter {
        window: window.clone(),
    })
}

// The chrome webview stays full-window (wry keeps it sized to the client
// area); the sidebar is a region of its DOM and content views overlay it,
// so the chrome owns every background pixel and the divider strips.
struct ChromeAdapter {
    window: WebviewWindow,
}

impl Chrome for ChromeAdapter {
    fn position(&self, frame: ChromeFrame) -> bool {
        let window = self.window.clone();
        let hidden = HIDDEN_SIDEBARS.with(|windows| windows.borrow().contains(self.window.label()));
        let edge = if hidden {
            zephium_core::layout::hidden_card_edge(frame.rect.width)
        } else {
            frame.rect.width
        };
        let page_left = (!frame.fill_width).then_some(frame.rect.x + edge);
        self.window
            .run_on_main_thread(move || caption::set_page_left(&window, page_left))
            .is_ok()
    }
}

impl PresentationChrome for ChromeAdapter {
    fn restore_browser_chrome(
        &self,
        revision: u64,
        items: zephium_ipc::ItemsState,
        done: ChromePresentationCallback,
    ) -> ChromePresentationDispatch {
        crate::restore_browser_chrome(&self.window, revision, items, done)
    }

    fn apply_tab_for_presentation(
        &self,
        presentation: ChromePresentation,
        done: ChromePresentationCallback,
    ) -> ChromePresentationDispatch {
        crate::apply_chrome_presentation(&self.window, presentation, done)
    }
}

pub fn content_size(_window: &WebviewWindow) -> Option<Size> {
    None
}

#[cfg(test)]
#[path = "windows_startup_tests.rs"]
mod startup_tests;
