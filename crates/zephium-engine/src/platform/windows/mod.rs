//! Windows adapter. Content views are child HWNDs managed by the stage:
//! positioning, SetWindowRgn rounding, divider drags and the drop indicator
//! all mirror the macOS ContentStage.

// The native-input feasibility harness remains independently feature-gated and
// optimized-build-refused. The production agent owner below contains no probe
// route and is reachable only through the dormant `agentic-browser` port.
#[cfg(feature = "agentic-browser")]
mod agent_context;
#[cfg(feature = "agentic-browser")]
mod agent_history;
#[cfg(feature = "agentic-browser")]
pub(crate) mod work_seed_metadata;
#[cfg(feature = "agentic-browser")]
pub(crate) use agent_history::AgentHistoryBackTicket;
#[cfg(feature = "native-agentic-input-probe")]
mod agentic_input_probe;
#[cfg(any(
    feature = "native-agentic-input-probe",
    feature = "native-agentic-semantic-probe"
))]
mod agentic_probe_resources;
#[cfg(feature = "native-agentic-semantic-probe")]
mod agentic_semantic_probe;
#[cfg(any(test, feature = "windows-cdp-spike"))]
#[allow(dead_code)]
mod cdp;
mod content_filter;
mod find;
pub(crate) use find::{find, FindReport, FindSession};
mod fullscreen;
pub(crate) use fullscreen::{exit_fullscreen, fullscreen_state};
pub use fullscreen::{install_fullscreen_observer, FullscreenObserver};
#[cfg(feature = "native-agentic-work-lifetime-diagnostic")]
mod cookie_storage_diagnostic;
#[cfg(feature = "native-agentic-work-lifetime-diagnostic")]
pub(crate) use cookie_storage_diagnostic::shutdown_admission as diagnose_shutdown_admission;
#[cfg(feature = "agentic-browser")]
// The host is the sole transaction owner. Physical Windows qualification is
// still required before claiming runtime behavior beyond cross-compilation.
mod cookie_transfer;
pub(crate) mod extensions;
#[cfg(feature = "agentic-browser")]
mod semantic_action;
#[cfg(feature = "agentic-browser")]
// Compiled and lifecycle-bound while invocation remains closed until the
// physical Windows isolated-world qualifier promotes the support claim.
#[allow(dead_code)]
mod semantic_runtime;
// The bounded native capture adapter is compiled now, but the Windows host
// keeps screenshot dispatch closed with semantic support until qualification.
mod apps;
mod native_paths;
pub(crate) use apps::{external_app_name, open_external_app};
mod paint;
#[cfg(feature = "agentic-browser")]
#[allow(dead_code)]
mod semantic_screenshot;
mod stage;
#[cfg(feature = "agentic-browser")]
mod work_rendering;
pub(crate) use native_paths::webview2_user_data_path;
#[cfg(feature = "agentic-browser")]
pub use work_rendering::{install_work_rendering_backing, remove_work_rendering_backing};
#[cfg(feature = "agentic-browser")]
mod work_network;
#[cfg(feature = "agentic-browser")]
mod work_presentation;
#[cfg(feature = "agentic-browser")]
pub(crate) use semantic_screenshot::capture_work_frame;
#[cfg(feature = "agentic-browser")]
pub(crate) use work_presentation::{
    PresentationState, WorkHumanPresentation, WorkObservationPresentation,
};
mod timeout;

#[cfg(feature = "agentic-browser")]
pub(crate) use content_filter::install_on_view as install_content_policy_on_view;
pub(crate) use content_filter::{
    install_scoped_on_view as install_scoped_content_policy_on_view,
    prepare as prepare_content_policy, same_policy as same_content_policy,
    ContentPolicyRegistration, NativeContentPolicy,
};
pub(crate) use paint::PaintCover;
pub use stage::Stage;
#[cfg(feature = "agentic-browser")]
pub(crate) use timeout::schedule_content_policy_timeout;
pub(crate) use timeout::{schedule_browser_timeout, ContentPolicyTimeout};

#[cfg(feature = "agentic-browser")]
pub(crate) use crate::platform::agent_cookie_preflight::map_cookie_transfer_deadline;
#[cfg(feature = "agentic-browser")]
pub(crate) use agent_context::{
    build_owned_agent_view, build_owned_work_view, AgentNavigationCommit, AgentNavigationTerminal,
    AgentOwnedProfile, AgentOwnedView, AgentOwnedViewCallbacks, AgentOwnedViewConstructionError,
    WorkStoreSeed,
};
#[cfg(feature = "native-agentic-input-probe")]
pub(crate) use agentic_input_probe::run as run_agentic_input_matrix;
#[cfg(feature = "native-agentic-semantic-probe")]
pub(crate) use agentic_semantic_probe::run as run_agentic_semantic_probe;
#[cfg(all(debug_assertions, feature = "native-agentic-semantic-probe"))]
pub(crate) use agentic_semantic_probe::run_cookie_persistence_control;
#[cfg(feature = "native-agentic-semantic-probe")]
pub(crate) use agentic_semantic_probe::run_semantic_action_guard_probe;
#[cfg(feature = "native-agentic-semantic-probe")]
pub(crate) use agentic_semantic_probe::run_work_application_probe;
#[cfg(feature = "agentic-browser")]
pub(crate) use cookie_transfer::{
    selected_profile_cookie_manager, WindowsAgentCookieCleanup, WindowsAgentCookieTerminal,
    WindowsAgentCookieTransfer,
};

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;

use webview2_com::Microsoft::Web::WebView2::Win32::{
    ICoreWebView2, ICoreWebView2Controller, ICoreWebView2Environment, ICoreWebView2Environment10,
    ICoreWebView2Environment5, ICoreWebView2Environment7, ICoreWebView2Environment8,
    ICoreWebView2Profile2, ICoreWebView2Settings4, ICoreWebView2Settings7, ICoreWebView2_10,
    ICoreWebView2_13, ICoreWebView2_14, ICoreWebView2_18, ICoreWebView2_5,
    COREWEBVIEW2_BROWSER_PROCESS_EXIT_KIND_FAILED, COREWEBVIEW2_BROWSER_PROCESS_EXIT_KIND_NORMAL,
    COREWEBVIEW2_KEY_EVENT_KIND_KEY_DOWN, COREWEBVIEW2_KEY_EVENT_KIND_SYSTEM_KEY_DOWN,
    COREWEBVIEW2_PDF_TOOLBAR_ITEMS_PRINT, COREWEBVIEW2_PDF_TOOLBAR_ITEMS_SAVE,
    COREWEBVIEW2_PDF_TOOLBAR_ITEMS_SAVE_AS,
    COREWEBVIEW2_PROCESS_FAILED_KIND_BROWSER_PROCESS_EXITED,
    COREWEBVIEW2_PROCESS_FAILED_KIND_FRAME_RENDER_PROCESS_EXITED,
    COREWEBVIEW2_PROCESS_FAILED_KIND_RENDER_PROCESS_EXITED,
    COREWEBVIEW2_PROCESS_FAILED_KIND_RENDER_PROCESS_UNRESPONSIVE,
    COREWEBVIEW2_PROCESS_KIND_BROWSER, COREWEBVIEW2_SERVER_CERTIFICATE_ERROR_ACTION_CANCEL,
};
use webview2_com::{
    AcceleratorKeyPressedEventHandler, BasicAuthenticationRequestedEventHandler,
    BrowserProcessExitedEventHandler, ClearBrowsingDataCompletedHandler,
    ClientCertificateRequestedEventHandler, HistoryChangedEventHandler,
    LaunchingExternalUriSchemeEventHandler, NavigationStartingEventHandler,
    NewBrowserVersionAvailableEventHandler, ProcessFailedEventHandler,
    ServerCertificateErrorDetectedEventHandler, SourceChangedEventHandler,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, VK_CONTROL, VK_ESCAPE, VK_MENU, VK_SHIFT,
};
use windows::Win32::{
    Foundation::{HANDLE, WAIT_EVENT, WAIT_OBJECT_0, WAIT_TIMEOUT},
    System::Threading::{OpenProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE},
};
use windows_core::{IUnknown, Interface, PWSTR};
use wry::WebViewExtWindows;
use zephium_core::ports::engine::{EngineEvent, Shortcut};

const PAGE_URL_UTF16_LIMIT: usize = 8 * 1_024;
const PAGE_URL_UTF8_LIMIT: usize = 8 * 1_024;
const WINDOWS_PATH_UTF16_LIMIT: usize = 32_767;
const WINDOWS_PATH_UTF8_LIMIT: usize = 4 * WINDOWS_PATH_UTF16_LIMIT;
const VERSION_UTF16_LIMIT: usize = 256;
const VERSION_UTF8_LIMIT: usize = 256;

pub fn user_script_refusal(
    script: &zephium_core::ports::engine::UserScript,
) -> Option<zephium_core::ports::engine::UserScriptRefusalReason> {
    use zephium_core::ports::engine::{RunAt, ScriptOwner, UserScriptRefusalReason, World};

    if matches!(script.world, World::Isolated(_)) {
        return Some(UserScriptRefusalReason::UnsupportedWorld);
    }
    if matches!(script.owner, ScriptOwner::Principal(_))
        || !script.matches.is_unconditional_all_urls()
    {
        return Some(UserScriptRefusalReason::UnsupportedMatchSet);
    }
    (script.run_at != RunAt::DocumentStart).then_some(UserScriptRefusalReason::UnsupportedRunAt)
}

pub fn user_style_refusal(
    style: &zephium_core::ports::engine::UserStyle,
) -> Option<zephium_core::ports::engine::UserScriptRefusalReason> {
    use zephium_core::ports::engine::{ScriptOwner, UserScriptRefusalReason};

    if matches!(style.owner, ScriptOwner::Principal(_)) {
        return Some(UserScriptRefusalReason::UnsupportedWorld);
    }
    (!style.matches.is_unconditional_all_urls())
        .then_some(UserScriptRefusalReason::UnsupportedMatchSet)
}

fn take_pwstr_bounded(
    source: PWSTR,
    max_utf16_units: usize,
    max_utf8_bytes: usize,
) -> Option<String> {
    let mut value = String::new();
    take_pwstr_bounded_into(source, max_utf16_units, max_utf8_bytes, &mut value)?;
    Some(value)
}

fn take_pwstr_bounded_into(
    source: PWSTR,
    max_utf16_units: usize,
    max_utf8_bytes: usize,
    value: &mut String,
) -> Option<()> {
    // WebView2 owns this out-string via CoTaskMemAlloc. The guard frees it on
    // success and every rejection without an unbounded intermediate String.
    let source = webview2_com::CoTaskMemPWSTR::from(source);
    let pointer = source.as_ref().as_pcwstr().as_ptr();
    if pointer.is_null() {
        value.clear();
        return Some(());
    }
    let mut length = 0;
    while length <= max_utf16_units {
        // SAFETY: WebView2's out-string contract is NUL terminated; the scan
        // is bounded to the policy maximum plus the distinguishing unit.
        if unsafe { pointer.add(length).read() } == 0 {
            // SAFETY: the scan established this initialized prefix and the
            // CoTaskMem owner remains live through conversion.
            let units = unsafe { std::slice::from_raw_parts(pointer, length) };
            let mut utf8_bytes = 0usize;
            for character in char::decode_utf16(units.iter().copied()) {
                let character = character.unwrap_or(char::REPLACEMENT_CHARACTER);
                utf8_bytes = utf8_bytes.checked_add(character.len_utf8())?;
                if utf8_bytes > max_utf8_bytes {
                    return None;
                }
            }
            value.clear();
            value.try_reserve(utf8_bytes).ok()?;
            for character in char::decode_utf16(units.iter().copied()) {
                value.push(character.unwrap_or(char::REPLACEMENT_CHARACTER));
            }
            return Some(());
        }
        length += 1;
    }
    None
}

/// Install policy that Wry cannot express for subframes and external URI
/// schemes. This runs after controller construction but before the first
/// navigation. Any missing interface or failed registration rejects the view;
/// silently continuing would let a hostile page reach Windows shell handlers.
pub fn configure(
    webview: &wry::WebView,
    _radius: f64,
    expected_in_private: bool,
    _expected_user_data_folder: &Path,
) -> windows_core::Result<SecurityPolicy> {
    let controller = webview.controller();
    let core = unsafe { controller.CoreWebView2()? };
    // `LaunchingExternalUriScheme` was added on CoreWebView2_18. Requiring the
    // interface is the runtime security floor; the caller drops this view if
    // the installed Evergreen/fixed runtime predates it.
    let core18 = core.cast::<ICoreWebView2_18>()?;
    let core10 = core.cast::<ICoreWebView2_10>()?;
    let core5 = core.cast::<ICoreWebView2_5>()?;
    let core14 = core.cast::<ICoreWebView2_14>()?;
    let mut policy = SecurityPolicy {
        core: core.clone(),
        core18: core18.clone(),
        core10: core10.clone(),
        core5: core5.clone(),
        core14: core14.clone(),
        frame_token: None,
        external_token: None,
        basic_auth_token: None,
        client_certificate_token: None,
        certificate_error_token: None,
    };

    // A certificate the system does not trust ends the navigation, as it
    // does on macOS. WebView2's default interstitial can offer a way past.
    let certificate_error =
        ServerCertificateErrorDetectedEventHandler::create(Box::new(|_, args| {
            if let Some(args) = args {
                unsafe { args.SetAction(COREWEBVIEW2_SERVER_CERTIFICATE_ERROR_ACTION_CANCEL)? };
            }
            Ok(())
        }));
    let mut certificate_error_token = 0_i64;
    unsafe {
        core14.add_ServerCertificateErrorDetected(&certificate_error, &mut certificate_error_token)
    }?;
    policy.certificate_error_token = Some(certificate_error_token);

    // HTTP authentication and client-certificate selection have independent
    // native default dialogs; PermissionRequested and script-dialog settings
    // do not cover them. Until Zephium has an origin-labelled broker, cancel
    // both surfaces and mark certificate requests handled so WebView2 cannot
    // fall back to ambient OS credentials/UI.
    let basic_auth = BasicAuthenticationRequestedEventHandler::create(Box::new(|_, args| {
        if let Some(args) = args {
            unsafe { args.SetCancel(true)? };
        }
        Ok(())
    }));
    let mut basic_auth_token = 0_i64;
    unsafe { core10.add_BasicAuthenticationRequested(&basic_auth, &mut basic_auth_token)? };
    policy.basic_auth_token = Some(basic_auth_token);

    let client_certificate = ClientCertificateRequestedEventHandler::create(Box::new(|_, args| {
        if let Some(args) = args {
            unsafe {
                args.SetCancel(true)?;
                args.SetHandled(true)?;
            }
        }
        Ok(())
    }));
    let mut client_certificate_token = 0_i64;
    unsafe {
        core5.add_ClientCertificateRequested(&client_certificate, &mut client_certificate_token)
    }?;
    policy.client_certificate_token = Some(client_certificate_token);

    // Wry falls back to the ordinary controller constructor when
    // Environment10 is unavailable, silently ignoring its incognito request.
    // Treat the native profile as the postcondition for both directions:
    // private content must be InPrivate, and durable content must not inherit
    // an accidentally private controller.
    let profile = unsafe { core.cast::<ICoreWebView2_13>()?.Profile()? };
    let mut actual_in_private = windows_core::BOOL::default();
    unsafe { profile.IsInPrivateModeEnabled(&mut actual_in_private)? };
    if !in_private_postcondition_holds(expected_in_private, actual_in_private.as_bool()) {
        return Err(windows_core::Error::from_hresult(
            windows::Win32::Foundation::E_ACCESSDENIED,
        ));
    }

    // Raw content views never use host objects or web messages. Disable the
    // mechanisms in the runtime too, so a future accidental handler cannot
    // silently turn into a bridge.
    let settings = unsafe { core.Settings()? };
    // The PDF viewer has its own Save, Save As and Print surfaces. They do not
    // reliably pass through DownloadStarting, so Settings7 is mandatory for
    // the raw-content download-deny boundary.
    let settings4 = settings.cast::<ICoreWebView2Settings4>()?;
    let settings7 = settings.cast::<ICoreWebView2Settings7>()?;
    unsafe {
        settings.SetAreHostObjectsAllowed(false)?;
        settings.SetIsWebMessageEnabled(false)?;
        // Password/form UI and suggestions are profile-persistent native
        // surfaces. A caller preference is insufficient: require Settings4,
        // disable both independent controls, and verify the runtime applied
        // them before this controller can navigate untrusted content.
        settings4.SetIsPasswordAutosaveEnabled(false)?;
        settings4.SetIsGeneralAutofillEnabled(false)?;
        let mut password_autosave_enabled = windows_core::BOOL::default();
        let mut general_autofill_enabled = windows_core::BOOL::default();
        settings4.IsPasswordAutosaveEnabled(&mut password_autosave_enabled)?;
        settings4.IsGeneralAutofillEnabled(&mut general_autofill_enabled)?;
        if password_autosave_enabled.as_bool() || general_autofill_enabled.as_bool() {
            return Err(windows_core::Error::from_hresult(
                windows::Win32::Foundation::E_ACCESSDENIED,
            ));
        }
        // Generic and agent views retain download/menu denial. The human-view
        // constructor may enable a bounded native menu only after installing
        // its download broker and a separate SaveAsUIShowing denial hook;
        // document SaveAs is not part of the DownloadStarting protocol.
        settings.SetAreDefaultContextMenusEnabled(false)?;
        // Native modal dialogs are likewise disabled until browser chrome has
        // an origin-labelled, rate-limited dialog broker.
        settings.SetAreDefaultScriptDialogsEnabled(false)?;
        settings7.SetHiddenPdfToolbarItems(
            COREWEBVIEW2_PDF_TOOLBAR_ITEMS_SAVE
                | COREWEBVIEW2_PDF_TOOLBAR_ITEMS_SAVE_AS
                | COREWEBVIEW2_PDF_TOOLBAR_ITEMS_PRINT,
        )?;
    }

    // Wry's navigation callback covers top-level navigations. WebView2 has a
    // separate event for frames; cancel first so URI extraction or policy
    // evaluation failure cannot accidentally authorize the navigation.
    let frame_navigation = NavigationStartingEventHandler::create(Box::new(|_, args| {
        let Some(args) = args else {
            return Ok(());
        };
        unsafe {
            args.SetCancel(true)?;
            let mut uri = PWSTR::null();
            args.Uri(&mut uri)?;
            if take_pwstr_bounded(uri, PAGE_URL_UTF16_LIMIT, PAGE_URL_UTF8_LIMIT).is_some_and(
                |uri| {
                    zephium_core::navigation::is_allowed_str(&uri)
                        || zephium_core::navigation::is_subframe_document(&uri)
                },
            ) {
                args.SetCancel(false)?;
            }
        }
        Ok(())
    }));
    let mut frame_token = 0_i64;
    unsafe { core.add_FrameNavigationStarting(&frame_navigation, &mut frame_token)? };
    policy.frame_token = Some(frame_token);

    // Never delegate custom URI schemes (including registered application
    // protocols) to the OS. Launching them is an out-of-sandbox side effect
    // and WebView2 does not route this event through NavigationStarting.
    let external_uri = LaunchingExternalUriSchemeEventHandler::create(Box::new(|_, args| {
        if let Some(args) = args {
            unsafe { args.SetCancel(true)? };
        }
        Ok(())
    }));
    let mut external_token = 0_i64;
    unsafe { core18.add_LaunchingExternalUriScheme(&external_uri, &mut external_token)? };
    policy.external_token = Some(external_token);

    Ok(policy)
}

/// Own every mandatory page-facing security registration installed during
/// controller hardening. Explicit removal makes teardown ordering auditable
/// and prevents a reused COM object from retaining callbacks beyond the exact
/// native view generation that authorized them.
pub struct SecurityPolicy {
    core: ICoreWebView2,
    core18: ICoreWebView2_18,
    core10: ICoreWebView2_10,
    core5: ICoreWebView2_5,
    core14: ICoreWebView2_14,
    frame_token: Option<i64>,
    external_token: Option<i64>,
    basic_auth_token: Option<i64>,
    client_certificate_token: Option<i64>,
    certificate_error_token: Option<i64>,
}

impl SecurityPolicy {
    /// Human tabs: WebView2 still never launches an app link itself; each
    /// one goes to `request`, so the browser can ask the person first.
    pub fn route_external_uris(
        &mut self,
        request: impl Fn(&str) + 'static,
    ) -> windows_core::Result<()> {
        let handler = LaunchingExternalUriSchemeEventHandler::create(Box::new(move |_, args| {
            let Some(args) = args else {
                return Ok(());
            };
            unsafe {
                args.SetCancel(true)?;
                let mut uri = PWSTR::null();
                args.Uri(&mut uri)?;
                if let Some(uri) =
                    take_pwstr_bounded(uri, PAGE_URL_UTF16_LIMIT, PAGE_URL_UTF8_LIMIT)
                {
                    request(&uri);
                }
            }
            Ok(())
        }));
        let mut token = 0_i64;
        // The new handler is in place before the cancel-only one goes, so no
        // moment exists where WebView2 would fall back to launching apps.
        unsafe {
            self.core18
                .add_LaunchingExternalUriScheme(&handler, &mut token)?
        };
        if let Some(previous) = self.external_token.replace(token) {
            let _ = unsafe { self.core18.remove_LaunchingExternalUriScheme(previous) };
        }
        Ok(())
    }
}

impl Drop for SecurityPolicy {
    fn drop(&mut self) {
        if let Some(token) = self.frame_token.take() {
            let _ = unsafe { self.core.remove_FrameNavigationStarting(token) };
        }
        if let Some(token) = self.external_token.take() {
            let _ = unsafe { self.core18.remove_LaunchingExternalUriScheme(token) };
        }
        if let Some(token) = self.basic_auth_token.take() {
            let _ = unsafe { self.core10.remove_BasicAuthenticationRequested(token) };
        }
        if let Some(token) = self.client_certificate_token.take() {
            let _ = unsafe { self.core5.remove_ClientCertificateRequested(token) };
        }
        if let Some(token) = self.certificate_error_token.take() {
            let _ = unsafe { self.core14.remove_ServerCertificateErrorDetected(token) };
        }
    }
}

/// Verify properties of the environment WebView2 actually created. Loader
/// environment checks at process startup are insufficient because registry
/// and group-policy values can override the executable and UDF selected by
/// programmatic options. This postcondition runs before the environment is
/// cached and before the first content navigation.
pub(crate) fn attest_environment(
    environment: &ICoreWebView2Environment,
    expected_user_data_folder: &Path,
) -> windows_core::Result<()> {
    let environment7 = environment.cast::<ICoreWebView2Environment7>()?;
    // Wry's upstream fallback silently ignores InPrivate/profile options.
    // The reviewed runtime floor supports Environment10, so absence is a
    // construction-environment failure rather than controller-local debt.
    let _environment10 = environment.cast::<ICoreWebView2Environment10>()?;

    let mut actual_user_data_folder = PWSTR::null();
    unsafe { environment7.UserDataFolder(&mut actual_user_data_folder)? };
    let actual_user_data_folder = take_pwstr_bounded(
        actual_user_data_folder,
        WINDOWS_PATH_UTF16_LIMIT,
        WINDOWS_PATH_UTF8_LIMIT,
    )
    .ok_or_else(|| {
        windows_core::Error::new(
            windows::Win32::Foundation::E_INVALIDARG,
            "WebView2 returned an over-limit user-data folder",
        )
    })?;
    let actual_user_data_folder_path = Path::new(&actual_user_data_folder);
    let matches = zephium_core::webview2::user_data_directory_matches(
        expected_user_data_folder,
        actual_user_data_folder_path,
    )
    .map_err(|error| {
        windows_core::Error::new(
            windows::Win32::Foundation::E_ACCESSDENIED,
            format!(
                "cannot verify actual WebView2 user-data folder {actual_user_data_folder:?} against {}: {error}",
                expected_user_data_folder.display()
            ),
        )
    })?;
    if !matches {
        return Err(windows_core::Error::new(
            windows::Win32::Foundation::E_ACCESSDENIED,
            format!(
                "actual WebView2 user-data folder {actual_user_data_folder:?} does not match {}",
                expected_user_data_folder.display()
            ),
        ));
    }

    let mut reported_version = PWSTR::null();
    unsafe { environment.BrowserVersionString(&mut reported_version)? };
    let reported_version =
        take_pwstr_bounded(reported_version, VERSION_UTF16_LIMIT, VERSION_UTF8_LIMIT).ok_or_else(
            || {
                windows_core::Error::new(
                    windows::Win32::Foundation::E_INVALIDARG,
                    "WebView2 returned an over-limit browser version",
                )
            },
        )?;
    zephium_core::webview2::admit_runtime(&reported_version).map_err(|error| {
        windows_core::Error::new(
            windows::Win32::Foundation::E_ACCESSDENIED,
            format!(
                "actual WebView2 environment runtime {reported_version:?} failed admission: {error}"
            ),
        )
    })?;
    Ok(())
}

fn in_private_postcondition_holds(expected: bool, actual: bool) -> bool {
    expected == actual
}

/// COM identity, not PID or wrapper address, defines one WebView2
/// environment. The host supplies its retained environment back to Wry for
/// every later controller; accepting a different object would make an
/// environment-scoped update registration incomplete.
pub(crate) fn same_environment(
    left: &ICoreWebView2Environment,
    right: &ICoreWebView2Environment,
) -> bool {
    let Ok(left) = left.cast::<IUnknown>() else {
        return false;
    };
    let Ok(right) = right.cast::<IUnknown>() else {
        return false;
    };
    left.as_raw() == right.as_raw()
}

/// Owns `NewBrowserVersionAvailable` for exactly one raw environment. The
/// callback deliberately carries no version string: the event means only
/// that the process must use ordered shutdown and a whole-application restart
/// before any component can claim to run the newly installed runtime.
pub(crate) struct BrowserVersionObserver {
    environment: ICoreWebView2Environment,
    token: i64,
}

impl Drop for BrowserVersionObserver {
    fn drop(&mut self) {
        let _ = unsafe {
            self.environment
                .remove_NewBrowserVersionAvailable(self.token)
        };
    }
}

pub(crate) fn install_browser_version_observer(
    environment: &ICoreWebView2Environment,
    on_available: impl Fn() + 'static,
) -> windows_core::Result<BrowserVersionObserver> {
    let handler = NewBrowserVersionAvailableEventHandler::create(Box::new(move |_, _| {
        // A malformed optional sender must not abort the browser. Invocation
        // itself is sufficient to choose the conservative restart-required
        // state, so there is no native value to unwrap or echo.
        on_available();
        Ok(())
    }));
    let mut token = 0_i64;
    unsafe { environment.add_NewBrowserVersionAvailable(&handler, &mut token)? };
    Ok(BrowserVersionObserver {
        environment: environment.clone(),
        token,
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProcessFailure {
    Renderer,
    Browser,
}

/// Owns the exact ProcessFailed registration for one WebView generation.
/// Removing it before the controller drops prevents a retained COM callback
/// from reporting failure under a later logical item binding.
pub struct CrashObserver {
    core: ICoreWebView2,
    token: i64,
}

impl Drop for CrashObserver {
    fn drop(&mut self) {
        let _ = unsafe { self.core.remove_ProcessFailed(self.token) };
    }
}

/// Surface renderer/browser death to the shell. Utility and GPU process
/// failures are intentionally excluded: WebView2 recovers those internally
/// and reloading every tab would create a failure storm.
pub fn install_crash_handler(
    webview: &wry::WebView,
    on_failure: impl Fn(ProcessFailure) + 'static,
) -> windows_core::Result<CrashObserver> {
    let core = webview.webview();
    let handler = ProcessFailedEventHandler::create(Box::new(move |_, args| {
        let Some(args) = args else {
            return Ok(());
        };
        let mut kind = Default::default();
        unsafe { args.ProcessFailedKind(&mut kind)? };
        match kind {
            COREWEBVIEW2_PROCESS_FAILED_KIND_BROWSER_PROCESS_EXITED => {
                on_failure(ProcessFailure::Browser)
            }
            COREWEBVIEW2_PROCESS_FAILED_KIND_RENDER_PROCESS_EXITED => {
                on_failure(ProcessFailure::Renderer)
            }
            // A subframe failure is not a top-level page death. Likewise an
            // unresponsive page must not be destroyed without user consent.
            COREWEBVIEW2_PROCESS_FAILED_KIND_FRAME_RENDER_PROCESS_EXITED
            | COREWEBVIEW2_PROCESS_FAILED_KIND_RENDER_PROCESS_UNRESPONSIVE => {}
            _ => {}
        }
        Ok(())
    }));
    let mut token = Default::default();
    unsafe { core.add_ProcessFailed(&handler, &mut token)? };
    Ok(CrashObserver { core, token })
}

/// Registrations that make the native WebView2 source and back/forward list
/// authoritative for browser chrome. Explicitly removing the handlers before
/// the Wry controller drops avoids leaving callbacks attached to a reused COM
/// object, and neither handler captures `core` or the WebView.
pub struct NavigationObserver {
    core: ICoreWebView2,
    source_token: i64,
    history_token: i64,
}

pub type InstalledNavigationObserver = NavigationObserver;

pub fn install_navigation_observer(
    webview: &wry::WebView,
    on_change: impl Fn() + 'static,
) -> windows_core::Result<NavigationObserver> {
    let core = webview.webview();
    let on_change: Rc<dyn Fn()> = Rc::new(on_change);

    // SourceChanged covers full navigations, fragment changes and History API
    // mutations. HistoryChanged is still required because pushState can add a
    // history entry without changing the serialized source at all.
    let on_source = on_change.clone();
    let source = SourceChangedEventHandler::create(Box::new(move |_, _| {
        on_source();
        Ok(())
    }));
    let mut source_token = 0_i64;
    unsafe { core.add_SourceChanged(&source, &mut source_token)? };

    let history = HistoryChangedEventHandler::create(Box::new(move |_, _| {
        on_change();
        Ok(())
    }));
    let mut history_token = 0_i64;
    if let Err(error) = unsafe { core.add_HistoryChanged(&history, &mut history_token) } {
        let _ = unsafe { core.remove_SourceChanged(source_token) };
        return Err(error);
    }

    Ok(NavigationObserver {
        core,
        source_token,
        history_token,
    })
}

impl Drop for NavigationObserver {
    fn drop(&mut self) {
        let _ = unsafe { self.core.remove_SourceChanged(self.source_token) };
        let _ = unsafe { self.core.remove_HistoryChanged(self.history_token) };
    }
}

pub fn current_url(view: &wry::WebView) -> Option<String> {
    current_url_core(&view.webview())
}

fn current_url_core(core: &ICoreWebView2) -> Option<String> {
    let mut source = PWSTR::null();
    // SAFETY: exact STA-owned core and initialized out pointer, freed by bounded string owner.
    unsafe { core.Source(&mut source) }.ok()?;
    take_pwstr_bounded(source, PAGE_URL_UTF16_LIMIT, PAGE_URL_UTF8_LIMIT)
}

pub fn enforce_navigation_pending(view: &wry::WebView) -> bool {
    // Wry's ContentLoading guard has already hidden both the child HWND and
    // controller. Re-drive the operation here so COM failure becomes an
    // exact host lifecycle failure rather than best-effort presentation.
    view.set_visible(false).is_ok()
}

/// A synchronization handle opened while the WebView2 browser process is
/// known alive. Holding the OS handle (rather than only a PID) avoids PID
/// reuse races when profile deletion waits for the UDF session to end.
#[derive(Clone)]
pub(crate) struct BrowserProcess {
    id: u32,
    // Erasure and app shutdown may overlap. Both retain the same opened
    // kernel object; neither reopens a numeric PID or owns a second process.
    handle: Arc<OwnedHandle>,
}

impl BrowserProcess {
    pub(crate) fn id(&self) -> u32 {
        self.id
    }

    pub(crate) fn is_running(&self) -> bool {
        let raw = HANDLE(self.handle.as_raw_handle());
        unsafe { WaitForSingleObject(raw, 0) == WAIT_TIMEOUT }
    }

    pub(crate) fn has_exited(&self) -> bool {
        let raw = HANDLE(self.handle.as_raw_handle());
        // Only the signalled state is proof. WAIT_FAILED and every unexpected
        // value remain fail-closed; in particular they must not be collapsed
        // into "not running" at a deletion boundary.
        wait_result_proves_exit(unsafe { WaitForSingleObject(raw, 0) })
    }
}

fn wait_result_proves_exit(result: WAIT_EVENT) -> bool {
    result == WAIT_OBJECT_0
}

pub(crate) fn browser_process(view: &wry::WebView) -> windows_core::Result<BrowserProcess> {
    let core = view.webview();
    let mut id = 0_u32;
    unsafe { core.BrowserProcessId(&mut id)? };
    if id == 0 {
        return Err(windows_core::Error::from_hresult(
            windows::Win32::Foundation::E_FAIL,
        ));
    }
    open_browser_process(id)
}

fn open_browser_process(id: u32) -> windows_core::Result<BrowserProcess> {
    if id == 0 {
        return Err(windows_core::Error::from_hresult(
            windows::Win32::Foundation::E_INVALIDARG,
        ));
    }
    let raw = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, false, id)? };
    // SAFETY: OpenProcess returned a new owned HANDLE. OwnedHandle closes it
    // exactly once when the profile process record is dropped.
    let handle = unsafe { OwnedHandle::from_raw_handle(raw.0) };
    Ok(BrowserProcess {
        id,
        handle: Arc::new(handle),
    })
}

/// Capture the browser process directly from an environment, before Wry starts
/// controller construction. Environment8's process collection is the earliest
/// stable API that identifies this native obligation, so a later controller,
/// handler, or initialization failure cannot leave an opaque process group.
pub(crate) fn browser_process_for_environment(
    environment: &ICoreWebView2Environment,
) -> windows_core::Result<BrowserProcess> {
    let environment8 = environment.cast::<ICoreWebView2Environment8>()?;
    let processes = unsafe { environment8.GetProcessInfos()? };
    let mut count = 0_u32;
    unsafe { processes.Count(&mut count)? };
    // A normal environment has a small collection. Bound hostile/corrupt COM
    // output before iterating it on the UI thread.
    if count == 0 || count > 4_096 {
        return Err(windows_core::Error::from_hresult(
            windows::Win32::Foundation::E_UNEXPECTED,
        ));
    }
    let mut browser_id = None;
    for index in 0..count {
        let process = unsafe { processes.GetValueAtIndex(index)? };
        let mut kind = Default::default();
        unsafe { process.Kind(&mut kind)? };
        if kind != COREWEBVIEW2_PROCESS_KIND_BROWSER {
            continue;
        }
        let mut process_id = 0_i32;
        unsafe { process.ProcessId(&mut process_id)? };
        let process_id = u32::try_from(process_id).map_err(|_| {
            windows_core::Error::from_hresult(windows::Win32::Foundation::E_UNEXPECTED)
        })?;
        if process_id == 0 || browser_id.replace(process_id).is_some() {
            return Err(windows_core::Error::from_hresult(
                windows::Win32::Foundation::E_UNEXPECTED,
            ));
        }
    }
    open_browser_process(browser_id.ok_or_else(|| {
        windows_core::Error::from_hresult(windows::Win32::Foundation::E_UNEXPECTED)
    })?)
}

/// Process-lifetime identity for one Environment5 registration. PID values can
/// be reused while old host callbacks are still queued, so every lifecycle
/// callback must match this non-wrapping generation as well as the PID.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct BrowserProcessGeneration(u64);

#[cfg(test)]
impl BrowserProcessGeneration {
    pub(crate) const fn for_test(value: u64) -> Self {
        Self(value)
    }
}

static NEXT_BROWSER_PROCESS_GENERATION: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(1);

fn next_browser_process_generation() -> windows_core::Result<BrowserProcessGeneration> {
    NEXT_BROWSER_PROCESS_GENERATION
        .fetch_update(
            std::sync::atomic::Ordering::Relaxed,
            std::sync::atomic::Ordering::Relaxed,
            |generation| generation.checked_add(1),
        )
        .map(BrowserProcessGeneration)
        .map_err(|_| windows_core::Error::from_hresult(windows::Win32::Foundation::E_OUTOFMEMORY))
}

pub(crate) fn browser_process_callback_matches(
    current_process_id: u32,
    current_generation: BrowserProcessGeneration,
    callback_process_id: u32,
    callback_generation: BrowserProcessGeneration,
) -> bool {
    current_process_id == callback_process_id && current_generation == callback_generation
}

pub(crate) fn browser_process_reuse_is_safe(
    retained_process_id: u32,
    observer_process_id: u32,
    returned_process_id: u32,
    observer_proof_pending: bool,
    exact_handle_running: bool,
) -> bool {
    retained_process_id == observer_process_id
        && retained_process_id == returned_process_id
        && observer_proof_pending
        && exact_handle_running
}

pub(crate) fn wait_for_browser_process_exit(
    process: &BrowserProcess,
    proof: &BrowserProcessExitProof,
    deadline: std::time::Instant,
) -> bool {
    if process.id() != proof.expected_process_id() {
        return false;
    }
    wait_for_exact_exit_with_pump(
        deadline,
        || match proof.snapshot() {
            BrowserProcessExitProofState::Invalid => Some(false),
            BrowserProcessExitProofState::Exited if process.has_exited() => Some(true),
            _ => None,
        },
        pump_browser_exit_callbacks,
    )
}

fn wait_for_exact_exit_with_pump(
    deadline: std::time::Instant,
    mut terminal: impl FnMut() -> Option<bool>,
    mut pump: impl FnMut(std::time::Instant) -> bool,
) -> bool {
    loop {
        if std::time::Instant::now() >= deadline {
            return false;
        }
        if let Some(proven) = terminal() {
            return proven;
        }
        if !pump(deadline) {
            return false;
        }
    }
}

pub(crate) fn pump_browser_exit_callbacks(deadline: std::time::Instant) -> bool {
    use windows::Win32::UI::WindowsAndMessaging::{
        DispatchMessageW, MsgWaitForMultipleObjectsEx, PeekMessageW, PostQuitMessage,
        TranslateMessage, MSG, MWMO_INPUTAVAILABLE, PM_REMOVE, QS_ALLINPUT, WM_QUIT,
    };
    let remaining = deadline.saturating_duration_since(std::time::Instant::now());
    if remaining.is_zero() {
        return false;
    }
    // SAFETY: this bounded pump runs on the same apartment that owns the
    // environment. Reentrant host commands remain deferred by its dispatcher.
    unsafe {
        let _ = MsgWaitForMultipleObjectsEx(
            None,
            remaining.as_millis().clamp(1, 25) as u32,
            QS_ALLINPUT,
            MWMO_INPUTAVAILABLE,
        );
        for _ in 0..256 {
            if std::time::Instant::now() >= deadline {
                return false;
            }
            let mut message = MSG::default();
            if !PeekMessageW(&mut message, None, 0, 0, PM_REMOVE).as_bool() {
                break;
            }
            if message.message == WM_QUIT {
                PostQuitMessage(message.wParam.0 as i32);
                return false;
            }
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
    true
}

/// A browser-process exit reported by the environment. Unlike ProcessFailed,
/// this event is raised only after the complete WebView2 process group has
/// terminated and released its UDF resources.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BrowserProcessExitEvent {
    Exited {
        expected_process_id: u32,
        generation: BrowserProcessGeneration,
        observed_process_id: u32,
    },
    Invalid {
        expected_process_id: u32,
        generation: BrowserProcessGeneration,
    },
}

impl BrowserProcessExitEvent {
    pub(crate) fn generation(self) -> BrowserProcessGeneration {
        match self {
            Self::Exited { generation, .. } | Self::Invalid { generation, .. } => generation,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BrowserProcessExitProofState {
    Pending,
    Exited,
    Invalid,
}

struct SharedBrowserProcessExitProof {
    data: std::sync::Mutex<BrowserProcessExitProofData>,
    ready: std::sync::Condvar,
}

struct BrowserProcessExitProofData {
    state: BrowserProcessExitProofState,
    // One profile can have only one admitted erasure attempt. Keeping exactly
    // one continuation makes late settlement allocation-bounded without a
    // permanently parked waiter thread.
    exit_action: Option<Box<dyn FnOnce() + Send>>,
}

/// Cloneable, thread-safe observation of one Environment5 registration. The
/// expected PID is captured while the exact process HANDLE is opened, making
/// an event for a prior or later process fail closed instead of satisfying an
/// erasure attempt through numeric PID reuse.
#[derive(Clone)]
pub(crate) struct BrowserProcessExitProof {
    expected_process_id: u32,
    generation: BrowserProcessGeneration,
    shared: Arc<SharedBrowserProcessExitProof>,
}

impl BrowserProcessExitProof {
    pub(crate) fn expected_process_id(&self) -> u32 {
        self.expected_process_id
    }

    pub(crate) fn generation(&self) -> BrowserProcessGeneration {
        self.generation
    }

    fn record(&self, event: BrowserProcessExitEvent) {
        let observed = match event {
            BrowserProcessExitEvent::Exited {
                expected_process_id,
                generation,
                observed_process_id,
                ..
            } if expected_process_id == self.expected_process_id
                && generation == self.generation
                && observed_process_id == self.expected_process_id =>
            {
                BrowserProcessExitProofState::Exited
            }
            BrowserProcessExitEvent::Exited { .. } | BrowserProcessExitEvent::Invalid { .. } => {
                BrowserProcessExitProofState::Invalid
            }
        };
        let mut data = self
            .shared
            .data
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        // A mismatch or unreadable callback permanently poisons this proof.
        // Once the expected event was accepted, later environment events are
        // irrelevant to this already-completed process lifetime.
        data.state = match (data.state, observed) {
            (BrowserProcessExitProofState::Pending, next) => next,
            (BrowserProcessExitProofState::Invalid, _) => BrowserProcessExitProofState::Invalid,
            (BrowserProcessExitProofState::Exited, _) => BrowserProcessExitProofState::Exited,
        };
        let action = if data.state == BrowserProcessExitProofState::Exited {
            data.exit_action.take()
        } else if data.state == BrowserProcessExitProofState::Invalid {
            // Drop a continuation whose proof was poisoned. Its Completion
            // remains active in the host admission map and the generic
            // watchdog is solely responsible for the public timeout.
            drop(data.exit_action.take());
            None
        } else {
            None
        };
        self.shared.ready.notify_all();
        drop(data);
        if let Some(action) = action {
            action();
        }
    }

    fn wait_until(&self, deadline: std::time::Instant) -> BrowserProcessExitProofState {
        let mut data = self
            .shared
            .data
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        while data.state == BrowserProcessExitProofState::Pending {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() {
                break;
            }
            let (next, timeout) = self
                .shared
                .ready
                .wait_timeout(data, remaining)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            data = next;
            if timeout.timed_out() {
                break;
            }
        }
        data.state
    }

    fn snapshot(&self) -> BrowserProcessExitProofState {
        self.shared
            .data
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .state
    }

    fn run_after_exit(&self, action: Box<dyn FnOnce() + Send>) -> bool {
        let mut data = self
            .shared
            .data
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match data.state {
            BrowserProcessExitProofState::Pending if data.exit_action.is_none() => {
                data.exit_action = Some(action);
                true
            }
            BrowserProcessExitProofState::Exited => {
                drop(data);
                action();
                true
            }
            BrowserProcessExitProofState::Pending | BrowserProcessExitProofState::Invalid => false,
        }
    }
}

/// Owns the Environment5 event registration on the UI thread. Erasure and
/// shutdown workers receive only `BrowserProcessExitProof`, while this guard
/// keeps the native handler registered until the matching event is delivered.
pub(crate) struct BrowserProcessExitObserver {
    environment: ICoreWebView2Environment5,
    token: i64,
    proof: BrowserProcessExitProof,
}

impl BrowserProcessExitObserver {
    pub(crate) fn proof(&self) -> BrowserProcessExitProof {
        self.proof.clone()
    }

    pub(crate) fn expected_process_id(&self) -> u32 {
        self.proof.expected_process_id()
    }

    pub(crate) fn generation(&self) -> BrowserProcessGeneration {
        self.proof.generation()
    }

    pub(crate) fn observed_expected_exit(&self) -> bool {
        self.proof.snapshot() == BrowserProcessExitProofState::Exited
    }

    pub(crate) fn is_pending(&self) -> bool {
        self.proof.snapshot() == BrowserProcessExitProofState::Pending
    }

    pub(crate) fn is_invalid(&self) -> bool {
        self.proof.snapshot() == BrowserProcessExitProofState::Invalid
    }
}

impl Drop for BrowserProcessExitObserver {
    fn drop(&mut self) {
        let _ = unsafe { self.environment.remove_BrowserProcessExited(self.token) };
    }
}

pub(crate) fn install_browser_process_exit_observer(
    environment: &ICoreWebView2Environment,
    expected_process_id: u32,
    on_event: impl Fn(BrowserProcessExitEvent) + 'static,
) -> windows_core::Result<BrowserProcessExitObserver> {
    if expected_process_id == 0 {
        return Err(windows_core::Error::from_hresult(
            windows::Win32::Foundation::E_INVALIDARG,
        ));
    }
    let environment = environment.cast::<ICoreWebView2Environment5>()?;
    let generation = next_browser_process_generation()?;
    let proof = BrowserProcessExitProof {
        expected_process_id,
        generation,
        shared: Arc::new(SharedBrowserProcessExitProof {
            data: std::sync::Mutex::new(BrowserProcessExitProofData {
                state: BrowserProcessExitProofState::Pending,
                exit_action: None,
            }),
            ready: std::sync::Condvar::new(),
        }),
    };
    let callback_proof = proof.clone();
    let handler = BrowserProcessExitedEventHandler::create(Box::new(move |_, args| {
        let event = args
            .and_then(|args| {
                let mut process_id = 0_u32;
                let mut exit_kind = Default::default();
                unsafe {
                    args.BrowserProcessId(&mut process_id).ok()?;
                    args.BrowserProcessExitKind(&mut exit_kind).ok()?;
                }
                if process_id == 0 {
                    return None;
                }
                match exit_kind {
                    COREWEBVIEW2_BROWSER_PROCESS_EXIT_KIND_NORMAL
                    | COREWEBVIEW2_BROWSER_PROCESS_EXIT_KIND_FAILED => {}
                    _ => return None,
                }
                Some(BrowserProcessExitEvent::Exited {
                    expected_process_id,
                    generation,
                    observed_process_id: process_id,
                })
            })
            .unwrap_or(BrowserProcessExitEvent::Invalid {
                expected_process_id,
                generation,
            });
        callback_proof.record(event);
        on_event(event);
        Ok(())
    }));
    let mut token = 0_i64;
    unsafe { environment.add_BrowserProcessExited(&handler, &mut token)? };
    Ok(BrowserProcessExitObserver {
        environment,
        token,
        proof,
    })
}

/// UI-apartment-owned proof for a privileged WebView2 environment.
///
/// Tauri owns these controllers, so the desktop cannot participate in the
/// engine host's profile lifecycle. This deliberately narrow adapter gives
/// the composition root the same two-factor deletion precondition used for
/// raw content: an `Environment5.BrowserProcessExited` event for the captured
/// PID and a retained, non-reusable HANDLE that is independently signalled.
/// Keeping the guard alive also keeps the Environment5 registration alive.
pub(crate) struct PrivilegedEnvironmentExitGuard {
    process: BrowserProcess,
    observer: BrowserProcessExitObserver,
}

impl PrivilegedEnvironmentExitGuard {
    pub(crate) fn install(
        environment: &ICoreWebView2Environment,
    ) -> windows_core::Result<PrivilegedEnvironmentExitGuard> {
        let process = browser_process_for_environment(environment)?;
        if !process.is_running() {
            return Err(windows_core::Error::from_hresult(
                windows::Win32::Foundation::E_UNEXPECTED,
            ));
        }
        let observer = install_browser_process_exit_observer(environment, process.id(), |_| {})?;
        // A privileged environment whose browser died during hardening is not
        // a usable security boundary. Reject it even if its exit callback was
        // delivered; startup will abandon this generation without deletion.
        if !process.is_running() {
            return Err(windows_core::Error::from_hresult(
                windows::Win32::Foundation::E_UNEXPECTED,
            ));
        }
        Ok(Self { process, observer })
    }

    pub(crate) fn expected_process_id(&self) -> u32 {
        self.process.id()
    }

    pub(crate) fn exact_exit_is_proven(&self) -> bool {
        self.observer.observed_expected_exit() && self.process.has_exited()
    }

    pub(crate) fn proof_is_invalid(&self) -> bool {
        self.observer.is_invalid()
    }
}

struct PrivilegedEnvironmentRegistration {
    environment: ICoreWebView2Environment,
    update_token: Option<i64>,
    exit_guard: PrivilegedEnvironmentExitGuard,
}

impl Drop for PrivilegedEnvironmentRegistration {
    fn drop(&mut self) {
        if let Some(token) = self.update_token.take() {
            let _ = unsafe { self.environment.remove_NewBrowserVersionAvailable(token) };
        }
    }
}

impl PrivilegedEnvironmentRegistration {
    fn detach_update(&mut self) -> bool {
        let Some(token) = self.update_token.take() else {
            return true;
        };
        if unsafe { self.environment.remove_NewBrowserVersionAvailable(token) }.is_ok() {
            true
        } else {
            // Preserve the token so Drop can make one final same-apartment
            // removal attempt after the exit proof result is fixed.
            self.update_token = Some(token);
            false
        }
    }
}

thread_local! {
    static PRIVILEGED_ENVIRONMENT_REGISTRATIONS:
        RefCell<HashMap<String, PrivilegedEnvironmentRegistration>> =
        RefCell::new(HashMap::new());
    static PRIVILEGED_ENVIRONMENT_FINALIZING: Cell<bool> = const { Cell::new(false) };
}

/// Install the runtime-update event and exact process-exit proof for one
/// Tauri-owned privileged environment. All COM work remains on this UI
/// apartment; no registration is dropped while its RefCell is borrowed.
pub fn install_privileged_environment_registration(
    label: String,
    environment: &ICoreWebView2Environment,
    on_runtime_update: Arc<dyn Fn() + Send + Sync + 'static>,
) -> windows_core::Result<()> {
    if PRIVILEGED_ENVIRONMENT_FINALIZING.with(Cell::get) {
        return Err(windows_core::Error::new(
            windows::Win32::Foundation::E_UNEXPECTED,
            "privileged WebView2 environment registration attempted during final teardown",
        ));
    }
    let existing = PRIVILEGED_ENVIRONMENT_REGISTRATIONS.with(|registrations| {
        let registrations = registrations.try_borrow().map_err(|_| {
            windows_core::Error::new(
                windows::Win32::Foundation::E_UNEXPECTED,
                "privileged WebView2 observer registry was re-entrantly borrowed",
            )
        })?;
        Ok::<_, windows_core::Error>(
            registrations
                .get(&label)
                .map(|registration| registration.environment.clone()),
        )
    })?;
    if let Some(existing) = existing {
        return if same_environment(&existing, environment) {
            Ok(())
        } else {
            Err(windows_core::Error::new(
                windows::Win32::Foundation::E_UNEXPECTED,
                "privileged WebView label changed WebView2 environment before destruction",
            ))
        };
    }
    let at_capacity = PRIVILEGED_ENVIRONMENT_REGISTRATIONS.with(|registrations| {
        registrations
            .try_borrow()
            .map(|registrations| registrations.len() >= 2)
            .map_err(|_| {
                windows_core::Error::new(
                    windows::Win32::Foundation::E_UNEXPECTED,
                    "privileged WebView2 observer registry was re-entrantly borrowed",
                )
            })
    })?;
    if at_capacity {
        return Err(windows_core::Error::new(
            windows::Win32::Foundation::E_OUTOFMEMORY,
            "privileged WebView2 observer registry exceeded main/panel bound",
        ));
    }

    let exit_guard = PrivilegedEnvironmentExitGuard::install(environment)?;
    let handler = NewBrowserVersionAvailableEventHandler::create(Box::new(move |_, _| {
        on_runtime_update();
        Ok(())
    }));
    let mut token = 0_i64;
    unsafe { environment.add_NewBrowserVersionAvailable(&handler, &mut token)? };
    let candidate = PrivilegedEnvironmentRegistration {
        environment: environment.clone(),
        update_token: Some(token),
        exit_guard,
    };

    let inserted = PRIVILEGED_ENVIRONMENT_REGISTRATIONS.with(|registrations| {
        let mut registrations = registrations.try_borrow_mut().map_err(|_| {
            windows_core::Error::new(
                windows::Win32::Foundation::E_UNEXPECTED,
                "privileged WebView2 observer registry was re-entrantly borrowed",
            )
        })?;
        if registrations.contains_key(&label) || registrations.len() >= 2 {
            return Ok::<bool, windows_core::Error>(false);
        }
        registrations.insert(label, candidate);
        Ok(true)
    })?;
    if inserted {
        Ok(())
    } else {
        Err(windows_core::Error::new(
            windows::Win32::Foundation::E_UNEXPECTED,
            "privileged WebView2 observer registry changed during installation",
        ))
    }
}

/// Remove only the runtime-update event when a privileged window is
/// destroyed. Its exact Environment5/HANDLE proof stays registered through
/// `run_return` and the bounded final apartment pump.
pub fn detach_privileged_environment_update(label: &str) -> bool {
    let removal = PRIVILEGED_ENVIRONMENT_REGISTRATIONS.with(|registrations| {
        let Ok(mut registrations) = registrations.try_borrow_mut() else {
            return Err(());
        };
        Ok(registrations.get_mut(label).and_then(|registration| {
            registration
                .update_token
                .take()
                .map(|token| (registration.environment.clone(), token))
        }))
    });
    let Ok(removal) = removal else {
        return false;
    };
    if let Some((environment, token)) = removal {
        if unsafe { environment.remove_NewBrowserVersionAvailable(token) }.is_err() {
            // Restore ownership for the final same-apartment retry. Installation
            // for an existing label is immutable, so no replacement can have
            // legitimately acquired this empty token slot.
            let restored = PRIVILEGED_ENVIRONMENT_REGISTRATIONS.with(|registrations| {
                let Ok(mut registrations) = registrations.try_borrow_mut() else {
                    return false;
                };
                let Some(registration) = registrations.get_mut(label) else {
                    return false;
                };
                if registration.update_token.is_some() {
                    return false;
                }
                registration.update_token = Some(token);
                true
            });
            if !restored {
                eprintln!(
                    "runtime: could not restore failed privileged update-observer removal token"
                );
            }
            return false;
        }
    }
    true
}

fn privileged_observer_set_is_exact(expected_labels: &[&str], observed: &[(String, u32)]) -> bool {
    let expected = expected_labels
        .iter()
        .copied()
        .collect::<std::collections::HashSet<_>>();
    let labels = observed
        .iter()
        .map(|(label, _)| label.as_str())
        .collect::<std::collections::HashSet<_>>();
    let processes = observed
        .iter()
        .map(|(_, process)| *process)
        .collect::<std::collections::HashSet<_>>();
    expected.len() == expected_labels.len()
        && observed.len() == expected_labels.len()
        && labels == expected
        && processes.len() == observed.len()
}

/// After Tauri's `run_return`, pump this same UI apartment for a bounded tail
/// and require the exact main/panel environment set before authorizing UDF
/// deletion. The registrations are always released after the result is fixed.
pub fn finalize_privileged_environment_registrations(expected_labels: &[&str]) -> bool {
    if PRIVILEGED_ENVIRONMENT_FINALIZING.with(|finalizing| finalizing.replace(true)) {
        return false;
    }
    let registrations = PRIVILEGED_ENVIRONMENT_REGISTRATIONS.with(|registrations| {
        let Ok(mut registrations) = registrations.try_borrow_mut() else {
            return None;
        };
        Some(std::mem::take(&mut *registrations))
    });
    let Some(mut registrations) = registrations else {
        return false;
    };
    for registration in registrations.values_mut() {
        if !registration.detach_update() {
            eprintln!(
                "runtime: privileged WebView2 update observer removal will be retried at drop"
            );
        }
    }
    let observed = registrations
        .iter()
        .map(|(label, registration)| (label.clone(), registration.exit_guard.expected_process_id()))
        .collect::<Vec<_>>();
    let exact_set = privileged_observer_set_is_exact(expected_labels, &observed);
    if exact_set {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while registrations
            .values()
            .any(|registration| !registration.exit_guard.exact_exit_is_proven())
            && registrations
                .values()
                .all(|registration| !registration.exit_guard.proof_is_invalid())
            && std::time::Instant::now() < deadline
        {
            pump_privileged_exit_callbacks(deadline);
        }
    }
    let proven = exact_set
        && registrations.values().all(|registration| {
            let proven = registration.exit_guard.exact_exit_is_proven();
            if !proven {
                eprintln!(
                    "privacy: privileged WebView2 process {} exit proof was {}",
                    registration.exit_guard.expected_process_id(),
                    if registration.exit_guard.proof_is_invalid() {
                        "invalid"
                    } else {
                        "not delivered or contradicted by the exact process HANDLE"
                    }
                );
            }
            proven
        });
    drop(registrations);
    proven
}

fn pump_privileged_exit_callbacks(deadline: std::time::Instant) {
    use windows::Win32::UI::WindowsAndMessaging::{
        DispatchMessageW, MsgWaitForMultipleObjectsEx, PeekMessageW, TranslateMessage, MSG,
        MWMO_INPUTAVAILABLE, PM_REMOVE, QS_ALLINPUT, WM_QUIT,
    };

    let remaining = deadline.saturating_duration_since(std::time::Instant::now());
    if remaining.is_zero() {
        return;
    }
    let wait_millis = remaining.as_millis().clamp(1, 25) as u32;
    unsafe {
        let _ = MsgWaitForMultipleObjectsEx(None, wait_millis, QS_ALLINPUT, MWMO_INPUTAVAILABLE);
        for _ in 0..256 {
            let mut message = MSG::default();
            if !PeekMessageW(&mut message, None, 0, 0, PM_REMOVE).as_bool() {
                break;
            }
            if message.message == WM_QUIT {
                continue;
            }
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
}

pub(crate) fn profile_for_erasure(
    view: &wry::WebView,
) -> windows_core::Result<ICoreWebView2Profile2> {
    unsafe {
        view.webview()
            .cast::<ICoreWebView2_13>()?
            .Profile()?
            .cast::<ICoreWebView2Profile2>()
    }
}

fn spawn_remove_after_proven_exit(
    process: Option<BrowserProcess>,
    roots: Vec<std::path::PathBuf>,
    profile: zephium_core::ids::ProfileId,
    completion: Arc<crate::erasure::Completion>,
) {
    let spawn_failure = completion.clone();
    let spawned = std::thread::Builder::new()
        .name("zephium-webview2-erasure".into())
        .spawn(move || {
            if let Some(process) = process {
                let raw = HANDLE(process.handle.as_raw_handle());
                // Environment5 says the whole browser process group has exited and
                // released the UDF. The retained exact-process HANDLE must already
                // agree; never turn a contradictory event into an unbounded wait.
                let wait = unsafe { WaitForSingleObject(raw, 0) };
                if wait != WAIT_OBJECT_0 {
                    eprintln!(
                        "privacy: WebView2 exit event contradicted exact process handle: {wait:?}"
                    );
                    // The Environment5 event was terminal, but this contradictory
                    // HANDLE state prevents deletion proof. Leave admission
                    // occupied; the generic watchdog owns caller-visible timeout.
                    return;
                }
            }

            let outcome = if crate::erasure::remove_profile_directories_verified(&roots, profile) {
                zephium_core::ports::engine::ProfileDataErasureOutcome::Verified
            } else {
                zephium_core::ports::engine::ProfileDataErasureOutcome::Failed
            };
            completion.finish(outcome);
        });
    if let Err(error) = spawned {
        eprintln!("privacy: could not start WebView2 erasure worker: {error}");
        spawn_failure.finish(zephium_core::ports::engine::ProfileDataErasureOutcome::Failed);
    }
}

fn wait_for_exit_and_remove(
    process: Option<BrowserProcess>,
    exit_proof: Option<BrowserProcessExitProof>,
    roots: Vec<std::path::PathBuf>,
    profile: zephium_core::ids::ProfileId,
    completion: Arc<crate::erasure::Completion>,
) {
    match (process, exit_proof) {
        (None, None) => spawn_remove_after_proven_exit(None, roots, profile, completion),
        (Some(process), Some(proof)) if proof.expected_process_id() == process.id() => {
            // Clear completion may precede or follow BrowserProcessExited.
            // Register one bounded continuation instead of parking a worker;
            // the event can settle this attempt long after the public 8s
            // watchdog has reported TimedOut.
            let action = Box::new(move || {
                spawn_remove_after_proven_exit(Some(process), roots, profile, completion);
            });
            let _registered = proof.run_after_exit(action);
        }
        _ => {
            // Missing/mismatched provenance has no safe terminal continuation.
            // Drop this local reference and let the generic watchdog report;
            // the host's active attempt flag intentionally remains set.
        }
    }
}

/// Wait for the exact browser process group to exit, then delete and verify
/// every owned UDF root. Profile2 clearing is defense in depth only: WebView2
/// documents that closing its WebView before clear completion may release the
/// handler without invoking it, so authoritative deletion must never depend
/// on that callback.
pub(crate) fn erase_profile_data(
    native_profile: Option<ICoreWebView2Profile2>,
    process: Option<BrowserProcess>,
    exit_proof: Option<BrowserProcessExitProof>,
    process_provenance_valid: bool,
    roots: Vec<std::path::PathBuf>,
    profile: zephium_core::ids::ProfileId,
    completion: Arc<crate::erasure::Completion>,
) {
    if !process_provenance_valid {
        // A native process obligation exists but its Environment5 proof was
        // not registered or was poisoned. This is not a settled failure: a
        // retry in the same process must remain blocked. The generic watchdog
        // owns the only caller-visible TimedOut report.
        return;
    }
    // Arm the Environment5 -> exact HANDLE -> verified-directory-removal path
    // before starting the opportunistic clear. It can settle after the public
    // watchdog even if WebView2 never invokes the clear callback.
    wait_for_exit_and_remove(process, exit_proof, roots, profile, completion);

    let Some(native_profile) = native_profile else {
        return;
    };
    let handler = ClearBrowsingDataCompletedHandler::create(Box::new(move |result| {
        if result.is_err() {
            eprintln!("privacy: WebView2 profile clear failed; deleting the released UDF");
        }
        Ok(())
    }));
    if unsafe { native_profile.ClearBrowsingDataAll(&handler) }.is_err() {
        eprintln!("privacy: WebView2 profile clear could not start; UDF deletion remains armed");
    }
}

pub(crate) struct BrowserProcessShutdownObligation {
    process: BrowserProcess,
    exit_proof: BrowserProcessExitProof,
}

impl BrowserProcessShutdownObligation {
    pub(crate) fn new(
        process: BrowserProcess,
        exit_proof: BrowserProcessExitProof,
    ) -> Option<Self> {
        (process.id() == exit_proof.expected_process_id()).then_some(Self {
            process,
            exit_proof,
        })
    }
}

/// Prove every retained environment's complete runtime group has exited using
/// one global bound. A shutdown timeout is unclean and must not be followed by
/// private-UDF deletion in this process.
pub(crate) fn wait_for_browser_process_shutdown(
    obligations: Vec<BrowserProcessShutdownObligation>,
    timeout: std::time::Duration,
) -> bool {
    let deadline = std::time::Instant::now() + timeout;
    for obligation in &obligations {
        let state = obligation.exit_proof.wait_until(deadline);
        #[cfg(feature = "native-agentic-work-lifetime-diagnostic")]
        cookie_storage_diagnostic::shutdown_wait(
            obligation.process.id(),
            state == BrowserProcessExitProofState::Exited,
            state == BrowserProcessExitProofState::Invalid,
            obligation.process.has_exited(),
        );
        if state != BrowserProcessExitProofState::Exited {
            return false;
        }
    }
    obligations.into_iter().all(|obligation| {
        let raw = HANDLE(obligation.process.handle.as_raw_handle());
        // An Environment5 callback can arrive just before its original process
        // HANDLE signals. Wait only the remainder of the same global bound.
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        let millis = remaining.as_millis().min(u128::from(u32::MAX - 1)) as u32;
        unsafe { WaitForSingleObject(raw, millis) == WAIT_OBJECT_0 }
    })
}

// WebView2 swallows browser accelerators before the page or any menu sees
// them; matching happens here and the command travels the engine event path.
pub struct AcceleratorRegistration {
    controller: ICoreWebView2Controller,
    token: i64,
}

impl Drop for AcceleratorRegistration {
    fn drop(&mut self) {
        let _ = unsafe { self.controller.remove_AcceleratorKeyPressed(self.token) };
    }
}

pub fn install_accelerators(
    view: &wry::WebView,
    shortcuts: Arc<std::sync::RwLock<Vec<Shortcut>>>,
    sink: Arc<dyn Fn(EngineEvent) + Send + Sync>,
    item: impl Fn() -> zephium_core::ids::ItemId + 'static,
) -> windows_core::Result<Option<AcceleratorRegistration>> {
    let controller = view.controller();
    let handler = AcceleratorKeyPressedEventHandler::create(Box::new(move |controller, args| {
        let Some(args) = args else {
            return Ok(());
        };
        let mut kind = COREWEBVIEW2_KEY_EVENT_KIND_KEY_DOWN;
        unsafe { args.KeyEventKind(&mut kind)? };
        if kind != COREWEBVIEW2_KEY_EVENT_KIND_KEY_DOWN
            && kind != COREWEBVIEW2_KEY_EVENT_KIND_SYSTEM_KEY_DOWN
        {
            return Ok(());
        }
        let mut key = 0u32;
        unsafe { args.VirtualKey(&mut key)? };
        let down = |vk: i32| unsafe { (GetKeyState(vk) as u16 & 0x8000) != 0 };
        let ctrl = down(VK_CONTROL.0 as i32);
        let shift = down(VK_SHIFT.0 as i32);
        let alt = down(VK_MENU.0 as i32);
        // Escape always leaves fullscreen, whatever the page does with the
        // key; the page still receives it.
        if key == u32::from(VK_ESCAPE.0)
            && !ctrl
            && !shift
            && !alt
            && controller
                .as_ref()
                .and_then(|controller| unsafe { controller.CoreWebView2() }.ok())
                .is_some_and(|core| fullscreen::exit_core(&core))
        {
            return Ok(());
        }
        let hit = shortcuts
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .find(|s| s.key == key && s.ctrl == ctrl && s.shift == shift && s.alt == alt)
            .map(|shortcut| shortcut.id.clone());
        if let Some(command) = hit {
            unsafe { args.SetHandled(true)? };
            sink(EngineEvent::ShortcutPressed {
                item: item(),
                command,
            });
        }
        Ok(())
    }));
    let mut token = Default::default();
    unsafe { controller.add_AcceleratorKeyPressed(&handler, &mut token)? };
    Ok(Some(AcceleratorRegistration { controller, token }))
}

// Edge's sleeping-tabs primitive: a suspended view frees most of its
// renderer working set and wakes on SetIsVisible(true).
pub fn try_suspend(view: &wry::WebView, done: impl FnOnce(bool) + 'static) -> bool {
    use webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2_3;
    use webview2_com::TrySuspendCompletedHandler;
    use windows_core::Interface;
    let controller = view.controller();
    let Ok(core) = (unsafe { controller.CoreWebView2() }) else {
        return false;
    };
    let Ok(v3) = core.cast::<ICoreWebView2_3>() else {
        return false;
    };
    let handler = TrySuspendCompletedHandler::create(Box::new(move |result, suspended| {
        done(result.is_ok() && suspended);
        Ok(())
    }));
    if let Err(e) = unsafe { v3.TrySuspend(&handler) } {
        eprintln!("engine: suspend failed: {e}");
        return false;
    }
    true
}

pub fn resume(view: &wry::WebView) {
    use webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2_3;
    use windows_core::Interface;
    let controller = view.controller();
    let Ok(core) = (unsafe { controller.CoreWebView2() }) else {
        return;
    };
    let Ok(v3) = core.cast::<ICoreWebView2_3>() else {
        return;
    };
    let _ = unsafe { v3.Resume() };
}

pub fn stop_loading(view: &wry::WebView) {
    let controller = view.controller();
    if let Ok(core) = unsafe { controller.CoreWebView2() } {
        let _ = unsafe { core.Stop() };
    }
}

/// WebView2 has no playback suspension; muting is what it offers, and a
/// page cannot unmute itself.
pub fn set_media_suspended(view: &wry::WebView, suspended: bool) {
    use webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2_8;
    use windows_core::Interface;

    if let Ok(core8) = view.webview().cast::<ICoreWebView2_8>() {
        let _ = unsafe { core8.SetIsMuted(suspended) };
    }
}

/// Permission UI belongs to the actual foreground browser window, rather
/// than whichever logical tab was last selected while another app is active.
pub(crate) fn permission_window_is_foreground(hwnd: isize) -> bool {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;
    hwnd != 0 && unsafe { GetForegroundWindow() == HWND(hwnd as *mut std::ffi::c_void) }
}

/// WebView2's native audio bit cannot be overridden by page JavaScript. API
/// or COM failure is uncertainty and therefore a discard veto.
pub fn query_document_activity(view: &wry::WebView, done: impl FnOnce(bool) + 'static) -> bool {
    use webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2_8;
    use windows_core::Interface;

    let core = view.webview();
    let Ok(core8) = core.cast::<ICoreWebView2_8>() else {
        return false;
    };
    let mut playing = windows_core::BOOL::default();
    if unsafe { core8.IsDocumentPlayingAudio(&mut playing) }.is_err() {
        return false;
    }
    done(!playing.as_bool());
    true
}

mod replay;
pub(crate) use replay::RequestWitness;

pub(crate) fn discard_history_allows(view: &wry::WebView, owned_blank: bool) -> bool {
    let core = view.webview();
    let mut back = windows_core::BOOL::default();
    let mut forward = windows_core::BOOL::default();
    (unsafe { core.CanGoBack(&mut back).is_ok() && core.CanGoForward(&mut forward).is_ok() })
        && !forward.as_bool()
        && (!back.as_bool() || owned_blank)
}

#[cfg(test)]
mod process_exit_tests {
    use super::*;

    fn proof(expected_process_id: u32) -> BrowserProcessExitProof {
        BrowserProcessExitProof {
            expected_process_id,
            generation: BrowserProcessGeneration(7),
            shared: Arc::new(SharedBrowserProcessExitProof {
                data: std::sync::Mutex::new(BrowserProcessExitProofData {
                    state: BrowserProcessExitProofState::Pending,
                    exit_action: None,
                }),
                ready: std::sync::Condvar::new(),
            }),
        }
    }

    #[test]
    fn erasure_and_shutdown_share_one_original_live_process_handle() {
        // SAFETY: queries this test's own current process; no foreign process
        // is launched, terminated, or given additional access.
        let id = unsafe { windows::Win32::System::Threading::GetCurrentProcessId() };
        let original = open_browser_process(id).unwrap();
        let erasure = original.clone();
        assert!(Arc::ptr_eq(&original.handle, &erasure.handle));
        let shutdown = BrowserProcessShutdownObligation::new(original.clone(), proof(id)).unwrap();
        assert!(Arc::ptr_eq(&shutdown.process.handle, &erasure.handle));
        assert!(
            BrowserProcessShutdownObligation::new(original.clone(), proof(id.wrapping_add(1)))
                .is_none()
        );
        drop(original);
        assert!(erasure.is_running());
        drop(erasure);
        assert!(shutdown.process.is_running());
        // Shared ownership does not prove exit or turn a pending Environment5
        // registration into a clean zero-obligation shutdown.
        assert!(!wait_for_browser_process_shutdown(
            vec![shutdown],
            std::time::Duration::ZERO
        ));
    }

    #[test]
    fn queued_group_exit_waits_for_the_held_handle_before_generation_rollover() {
        let proof = proof(41);
        let handle_exited = std::cell::Cell::new(false);
        let pumps = std::cell::Cell::new(0);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
        assert!(wait_for_exact_exit_with_pump(
            deadline,
            || match proof.snapshot() {
                BrowserProcessExitProofState::Invalid => Some(false),
                BrowserProcessExitProofState::Exited if handle_exited.get() => Some(true),
                _ => None,
            },
            |_| {
                pumps.set(pumps.get() + 1);
                if pumps.get() == 1 {
                    proof.record(BrowserProcessExitEvent::Exited {
                        expected_process_id: 41,
                        generation: proof.generation(),
                        observed_process_id: 41,
                    });
                } else {
                    handle_exited.set(true);
                }
                true
            },
        ));
        assert_eq!(
            pumps.get(),
            2,
            "the group event alone cannot authorize rollover"
        );
        assert!(!browser_process_callback_matches(
            43,
            BrowserProcessGeneration(8),
            41,
            proof.generation(),
        ));
    }

    #[test]
    fn pending_group_exit_cannot_authorize_rollover_after_the_original_deadline() {
        let proof = proof(41);
        let pumped = std::cell::Cell::new(false);
        assert!(!wait_for_exact_exit_with_pump(
            std::time::Instant::now(),
            || (proof.snapshot() == BrowserProcessExitProofState::Exited).then_some(true),
            |_| {
                pumped.set(true);
                true
            },
        ));
        assert!(!pumped.get());
        assert_eq!(proof.snapshot(), BrowserProcessExitProofState::Pending);
    }

    #[test]
    fn only_the_recorded_pid_satisfies_group_exit_proof() {
        let proof = proof(41);
        proof.record(BrowserProcessExitEvent::Exited {
            expected_process_id: 41,
            generation: BrowserProcessGeneration(7),
            observed_process_id: 41,
        });
        assert_eq!(
            proof.wait_until(std::time::Instant::now()),
            BrowserProcessExitProofState::Exited
        );
    }

    #[test]
    fn pid_mismatch_permanently_poisons_group_exit_proof() {
        let proof = proof(41);
        proof.record(BrowserProcessExitEvent::Exited {
            expected_process_id: 41,
            generation: BrowserProcessGeneration(7),
            observed_process_id: 42,
        });
        proof.record(BrowserProcessExitEvent::Exited {
            expected_process_id: 41,
            generation: BrowserProcessGeneration(7),
            observed_process_id: 41,
        });
        assert_eq!(
            proof.wait_until(std::time::Instant::now()),
            BrowserProcessExitProofState::Invalid
        );
    }

    #[test]
    fn unreadable_event_cannot_be_rehabilitated_by_a_later_callback() {
        let proof = proof(41);
        proof.record(BrowserProcessExitEvent::Invalid {
            expected_process_id: 41,
            generation: BrowserProcessGeneration(7),
        });
        proof.record(BrowserProcessExitEvent::Exited {
            expected_process_id: 41,
            generation: BrowserProcessGeneration(7),
            observed_process_id: 41,
        });
        assert_eq!(
            proof.wait_until(std::time::Instant::now()),
            BrowserProcessExitProofState::Invalid
        );
    }

    #[test]
    fn privileged_exit_set_requires_exact_labels_and_distinct_processes() {
        let expected = ["main", "panel"];
        assert!(privileged_observer_set_is_exact(
            &expected,
            &[("main".into(), 10), ("panel".into(), 11)]
        ));
        assert!(!privileged_observer_set_is_exact(
            &expected,
            &[("main".into(), 10)]
        ));
        assert!(!privileged_observer_set_is_exact(
            &expected,
            &[("main".into(), 10), ("unknown".into(), 11)]
        ));
        assert!(!privileged_observer_set_is_exact(
            &expected,
            &[("main".into(), 10), ("panel".into(), 10)]
        ));
        assert!(!privileged_observer_set_is_exact(
            &["main", "main"],
            &[("main".into(), 10), ("panel".into(), 11)]
        ));
    }

    #[test]
    fn stale_generation_is_rejected_even_when_windows_reuses_the_pid() {
        assert!(!browser_process_callback_matches(
            41,
            BrowserProcessGeneration(8),
            41,
            BrowserProcessGeneration(7),
        ));
        assert!(browser_process_callback_matches(
            41,
            BrowserProcessGeneration(8),
            41,
            BrowserProcessGeneration(8),
        ));
    }

    #[test]
    fn same_pid_reuse_requires_pending_proof_and_unsignalled_exact_handle() {
        assert!(browser_process_reuse_is_safe(41, 41, 41, true, true));
        assert!(!browser_process_reuse_is_safe(41, 41, 41, false, true));
        assert!(!browser_process_reuse_is_safe(41, 41, 41, true, false));
        assert!(!browser_process_reuse_is_safe(41, 42, 41, true, true));
    }

    #[test]
    fn only_a_signalled_exact_handle_proves_browser_process_exit() {
        use windows::Win32::Foundation::WAIT_FAILED;

        assert!(wait_result_proves_exit(WAIT_OBJECT_0));
        assert!(!wait_result_proves_exit(WAIT_TIMEOUT));
        assert!(!wait_result_proves_exit(WAIT_FAILED));
        assert!(!wait_result_proves_exit(WAIT_EVENT(7)));
    }

    #[test]
    fn native_in_private_mode_must_match_both_storage_classes() {
        assert!(in_private_postcondition_holds(true, true));
        assert!(in_private_postcondition_holds(false, false));
        assert!(!in_private_postcondition_holds(true, false));
        assert!(!in_private_postcondition_holds(false, true));
    }

    #[test]
    fn late_group_exit_runs_the_bounded_erasure_continuation_once() {
        let proof = proof(41);
        let runs = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let action_runs = runs.clone();
        assert!(proof.run_after_exit(Box::new(move || {
            action_runs.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        })));
        assert!(!proof.run_after_exit(Box::new(|| {
            panic!("only one erasure continuation may be registered")
        })));

        let event = BrowserProcessExitEvent::Exited {
            expected_process_id: 41,
            generation: BrowserProcessGeneration(7),
            observed_process_id: 41,
        };
        proof.record(event);
        proof.record(event);
        assert_eq!(runs.load(std::sync::atomic::Ordering::Relaxed), 1);
    }
}
