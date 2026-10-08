#[path = "windows_caption.rs"]
mod caption;
#[path = "windows_menus.rs"]
mod menus;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use tauri::WebviewWindow;

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

/// Query the runtime selected by WebView2 before Tauri creates any controller.
/// Capability checks still run per-view; an outdated runtime is reported as
/// an update advisory instead of refused.
pub fn enforce_runtime_security_floor(
) -> Result<zephium_core::runtime_security::RuntimeSecurityAdvisories, String> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs());
    if let Some(name) = zephium_core::webview2::first_present_environment_override(|name| {
        std::env::var_os(name).is_some()
    }) {
        return Err(format!(
            "security-relevant WebView2 environment override {name} is present; unset it before starting Zephium"
        ));
    }
    let reported = tauri::webview_version()
        .map_err(|error| format!("cannot query the selected WebView2 runtime: {error}"))?;
    let (_, advisory) =
        zephium_core::webview2::assess_runtime(&reported, now).map_err(|error| {
            format!(
                "WebView2 runtime {reported:?} is not supported ({error}). Zephium needs the stable Evergreen WebView2 Runtime"
            )
        })?;
    Ok(advisory)
}

#[path = "windows_log.rs"]
mod windows_log;
pub use windows_log::redirect_stderr;

pub fn init(
    window: &WebviewWindow,
    expected_user_data_folder: &Path,
    on_runtime_update: RuntimeUpdateCallback,
) -> bool {
    round_corners(window);
    if !menus::install(window) {
        crate::write_diagnostic(format_args!("chrome: using system menu rendering"));
    }
    if !caption::install(window) {
        // A visual enhancement must never remove access to window controls.
        let _ = window.set_decorations(true);
    }
    let hardened = harden_privileged(window, expected_user_data_folder, on_runtime_update);
    apply_material(window, true);
    hardened
}

/// Tauri does not expose Wry's permission callback. Install the WebView2
/// policy directly for privileged chrome so an XSS cannot prompt for ambient
/// capabilities or persist password/form suggestions in its shared UDF.
pub fn harden_privileged(
    window: &WebviewWindow,
    expected_user_data_folder: &Path,
    on_runtime_update: RuntimeUpdateCallback,
) -> bool {
    let installed = Arc::new(AtomicBool::new(false));
    let completed = installed.clone();
    let expected_user_data_folder = expected_user_data_folder.to_owned();
    let label = window.label().to_owned();
    let scheduled = window.with_webview(move |webview| unsafe {
        use webview2_com::Microsoft::Web::WebView2::Win32::{
            ICoreWebView2Environment10, ICoreWebView2Environment7, ICoreWebView2Settings3,
            ICoreWebView2Settings4, ICoreWebView2_10, ICoreWebView2_13, ICoreWebView2_18,
            ICoreWebView2_25, ICoreWebView2_5, COREWEBVIEW2_PERMISSION_STATE_DENY,
        };
        use webview2_com::{
            BasicAuthenticationRequestedEventHandler, ClientCertificateRequestedEventHandler,
            LaunchingExternalUriSchemeEventHandler, NavigationStartingEventHandler,
            PermissionRequestedEventHandler, SaveAsUIShowingEventHandler,
        };
        use windows::core::{Interface, BOOL, PWSTR};

        let result = (|| -> windows::core::Result<()> {
            let environment = webview.environment();
            let environment7 = environment.cast::<ICoreWebView2Environment7>()?;
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
                ));
            }

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
            zephium_core::webview2::admit_runtime(&reported_version).map_err(|error| {
                windows::core::Error::new(
                    windows::Win32::Foundation::E_ACCESSDENIED,
                    format!(
                        "actual privileged WebView2 environment runtime {reported_version:?} failed admission: {error}"
                    ),
                )
            })?;

            // Wry silently falls back to a non-private controller when the
            // runtime predates Environment10. Refuse that runtime instead.
            let _environment10 = environment.cast::<ICoreWebView2Environment10>()?;
            let core = webview.controller().CoreWebView2()?;
            // External URI launch interception is only exposed by
            // ICoreWebView2_18. Treat that interface as part of the mandatory
            // runtime floor instead of silently accepting a weaker runtime.
            let core18 = core.cast::<ICoreWebView2_18>()?;
            let profile = core.cast::<ICoreWebView2_13>()?.Profile()?;
            let mut in_private = windows::core::BOOL::default();
            profile.IsInPrivateModeEnabled(&mut in_private)?;
            if !in_private.as_bool() {
                crate::write_diagnostic(format_args!(
                    "security: privileged WebView2 profile is not InPrivate"
                ));
                return Err(windows::core::Error::from_hresult(
                    windows::Win32::Foundation::E_FAIL,
                ));
            }
            let settings = core.Settings()?;
            settings.SetAreDefaultScriptDialogsEnabled(false)?;
            settings.SetAreDefaultContextMenusEnabled(false)?;
            // Privileged chrome has no reason to invoke WebView2's browser UI
            // accelerators (notably Ctrl+P). Require Settings3 instead of
            // accepting an older runtime with an unbrokered native print UI,
            // and verify the native postcondition before exposing the view.
            let settings3 = settings.cast::<ICoreWebView2Settings3>()?;
            settings3.SetAreBrowserAcceleratorKeysEnabled(false)?;
            let mut browser_accelerators_enabled = BOOL::default();
            settings3.AreBrowserAcceleratorKeysEnabled(&mut browser_accelerators_enabled)?;
            if browser_accelerators_enabled.as_bool() {
                return Err(windows::core::Error::new(
                    windows::Win32::Foundation::E_ACCESSDENIED,
                    "privileged WebView2 browser accelerators remained enabled",
                ));
            }
            let settings4 = settings.cast::<ICoreWebView2Settings4>()?;
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
                ));
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

            let core10 = core.cast::<ICoreWebView2_10>()?;
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

            let core5 = core.cast::<ICoreWebView2_5>()?;
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
            let core25 = core.cast::<ICoreWebView2_25>()?;
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
            crate::write_diagnostic(format_args!(
                "security: privileged WebView2 hardening failed: {error}"
            ));
            return;
        }
        completed.store(true, Ordering::Release);
    });
    scheduled.is_ok() && installed.load(Ordering::Acquire)
}

// Undecorated windows lose DWM rounding unless asked for explicitly.
/// Keeps the window's own caption out of a page that fills the screen.
pub fn set_caption_suppressed(window: &WebviewWindow, suppressed: bool) {
    caption::set_suppressed(window, suppressed);
}

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

// Acrylic with a deep tint; a high-alpha tint keeps the blur readable
// instead of smeared. Applies from Win10 up.
pub fn apply_material(window: &WebviewWindow, dark: bool) -> bool {
    let tint = if dark {
        (24, 24, 28, 210)
    } else {
        (245, 245, 249, 210)
    };
    // Use the fallible native adapter directly; the Tauri effect dispatcher
    // discards the underlying DWM error and can falsely report acrylic.
    let contrast = menus::high_contrast();
    let result = if contrast {
        window_vibrancy::clear_acrylic(window)
    } else {
        window_vibrancy::apply_acrylic(window, Some(tint))
    };
    if let Err(e) = &result {
        crate::write_diagnostic(format_args!("material: window effects unavailable: {e}"));
    }
    let material = if result.is_ok() && !contrast {
        crate::material::Material::Acrylic
    } else {
        crate::material::Material::None
    };
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

/// Hides or shows the chrome WebView while its window stays on screen, so the
/// window shows only its material, never a document that is still loading.
pub fn set_chrome_hidden(window: &WebviewWindow, hidden: bool) -> bool {
    window
        .with_webview(move |webview| {
            // SAFETY: Tauri supplies the live controller for the duration of
            // this UI-thread callback.
            if unsafe { webview.controller().SetIsVisible(!hidden) }.is_err() {
                crate::write_diagnostic(format_args!(
                    "onboarding: chrome WebView2 visibility was refused"
                ));
            }
        })
        .is_ok()
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
    fn position(&self, _frame: ChromeFrame) -> bool {
        true
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
