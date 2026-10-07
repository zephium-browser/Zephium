//! Composition root: the only crate that knows Tauri. Wires the dependency graph
//! (window -> chrome positioning, engine, shell) and the command surface.

#[cfg(all(
    feature = "adblock-qa",
    any(
        not(debug_assertions),
        not(any(target_os = "macos", target_os = "windows"))
    )
))]
compile_error!("protection QA requires a macOS or Windows debug build");

#[cfg(all(
    feature = "file-workflows-qa",
    any(
        not(debug_assertions),
        not(any(target_os = "macos", target_os = "windows"))
    )
))]
compile_error!("file workflows QA requires a macOS or Windows debug build");

#[cfg(all(
    any(feature = "resource-ui-qa", feature = "work-integration-qa"),
    any(not(debug_assertions), not(target_os = "macos"))
))]
compile_error!("resource UI QA is macOS debug-only");

#[cfg(all(
    feature = "macos-work-rendering-probe",
    any(not(debug_assertions), not(target_os = "macos"))
))]
compile_error!("the actual-lifecycle rendering witness is macOS debug-only");
#[cfg(all(feature = "macos-work-rendering-probe", target_os = "macos"))]
mod foreground_rendering_probe;

#[cfg(all(
    feature = "macos-work-navigation-probe",
    any(not(debug_assertions), not(target_os = "macos"))
))]
compile_error!("the actual-application navigation witness is macOS debug-only");
#[cfg(all(feature = "macos-work-navigation-probe", target_os = "macos"))]
#[cfg_attr(feature = "macos-work-profile-enrollment", allow(dead_code))]
mod navigation_probe;
#[cfg(all(
    feature = "macos-work-profile-enrollment",
    any(
        feature = "macos-work-retained-notion-probe",
        feature = "macos-work-retained-notion-write-probe"
    )
))]
compile_error!("profile enrollment and authenticated execution are separate application builds");

#[cfg(feature = "macos-work")]
pub use zephium_work_composition::{
    MacosWorkComposition, PublicReadWorkAccount, PublicReadWorkInvocation, PublicReadWorkObjective,
    PublicReadWorkSettings, TrustedWorkRequest,
};
#[cfg(feature = "macos-work")]
mod work;
#[cfg(feature = "macos-work-public-inspection")]
mod work_development;
#[cfg(feature = "macos-work")]
pub use work::{
    admit_retained_trusted_work, admit_successor_trusted_work, admit_trusted_work,
    launch_public_read_work, selected_work_profile, WorkAdmissionFailure,
};

mod about;
mod blocker_service;
mod browser_credentials;
mod browser_import;
mod content_fullscreen;
mod default_browser;
mod diagnostics;
mod external_links;
#[cfg(feature = "work-product")]
mod favicon_probe;
mod focus_alerts;
mod intro_sound;
mod keymap;
mod launcher_trigger;
#[cfg(target_os = "linux")]
mod linux_global_shortcuts;
#[cfg(any(target_os = "linux", test))]
mod linux_shortcut;
#[cfg(any(target_os = "linux", test))]
mod linux_shortcut_portal;
#[cfg(any(target_os = "linux", test))]
mod linux_x11_shortcut;
mod material;
mod media;
mod memory_pressure;
mod notes;
mod overlay;
#[cfg(target_os = "macos")]
mod panel;
mod platform;
mod presence;
#[cfg(target_os = "windows")]
mod privileged_runtime_windows;
#[cfg(any(target_os = "windows", test))]
mod renderer_recovery;
mod resource_close;
mod search_providers;
mod startup_alert;
#[cfg(feature = "work-development-traces")]
mod startup_styles;
mod updates;
mod webext;
#[cfg(all(target_os = "windows", feature = "webext-qa"))]
mod webext_qa;
#[cfg(feature = "work-product")]
mod work_activity;
#[cfg(feature = "work-integration-qa")]
mod work_captures;
mod work_connections;
mod work_decision;
#[cfg(feature = "work-development-traces")]
mod work_diagnostics;
mod work_folders;
mod work_memory;
mod work_models;
#[cfg(any(feature = "work-product", test))]
mod work_operations;
mod work_personal;
mod work_product;
#[cfg(feature = "work-product")]
mod work_provider;
mod work_sites;

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU64, AtomicU8, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

#[cfg(any(target_os = "macos", target_os = "windows"))]
use raw_window_handle::HasWindowHandle;
use serde::{Deserialize, Serialize};
use tauri::{LogicalPosition, Manager, State, WebviewWindow};
use tauri_specta::{collect_commands, collect_events, Event};

use zephium_app::{
    ChromePresentation, ChromePresentationCallback, ChromePresentationDispatch, Command, EmitFn,
    Handle, PagePermissionPromptDecision, SharedChrome, ShellTerminalFailureCallback,
    ShutdownOutcome, TabAction,
};
use zephium_blocker_service::ManagedBlocker;
use zephium_core::extensions::{
    ExtensionActionRevision, ExtensionPopupAnchor, ExtensionRuntimeGeneration,
    ExtensionRuntimeInstance,
};
use zephium_core::geometry::{Rect, Size};
use zephium_core::ids::ScriptId;
use zephium_core::ids::{ExtensionInstallId, ItemId, ProfileId};
use zephium_core::injection::MatchSet;
use zephium_core::ports::blocker::{BlockerCompiler as _, BlockerShutdownOutcome};
use zephium_core::ports::engine::{
    Engine as _, ScriptOwner, UserContent, UserContentGeneration, UserStyle,
};
use zephium_core::ports::store::{Store as _, StoreShutdownOutcome};
use zephium_core::split::Axis;
use zephium_engine::{InitialUserContent, MainThreadDispatch, WebviewEngine};
use zephium_ipc::Projection;
use zephium_store::SqliteStore;

macro_rules! diagnostic {
    ($($argument:tt)*) => {{
        write_diagnostic(format_args!($($argument)*));
    }};
}

// Only the thumb may paint. Every other part stays transparent so the page
// background shows through the gutter; an unstyled scrollbar background is
// what rendered as a detached band along the edge.
macro_rules! scrollbar_base_css {
    () => {
        "::-webkit-scrollbar{width:10px;height:10px;background:transparent}::-webkit-scrollbar-thumb{background:rgba(140,140,150,.45);border-radius:8px;border:2px solid transparent;background-clip:padding-box}::-webkit-scrollbar-thumb:hover{background:rgba(140,140,150,.75);background-clip:padding-box}::-webkit-scrollbar-track{background:transparent}::-webkit-scrollbar-button{display:none}"
    };
}

// WebKit fills the main frame's styled scroll corner with the view's white
// base color before painting it, so any corner style shows white beside a
// dark page. Unstyled, the corner lets the page background through.
#[cfg(target_os = "macos")]
const SCROLLBAR_CSS: &str = scrollbar_base_css!();

// Chromium paints an unstyled corner with its default theme square.
#[cfg(not(target_os = "macos"))]
const SCROLLBAR_CSS: &str = concat!(
    scrollbar_base_css!(),
    "::-webkit-scrollbar-corner{background:transparent}"
);

static APP_STARTED: OnceLock<std::time::Instant> = OnceLock::new();
static APP_STORE: OnceLock<Arc<SqliteStore>> = OnceLock::new();
// Work provider futures (reqwest, rustls) run on these workers; the 2 MiB
// tokio default overflowed the same handshake on the agent runtime worker.
const ASYNC_WORKER_STACK_BYTES: usize = 16 * 1024 * 1024;
static ASYNC_RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
static AUTH_DENIAL_LOGS_REMAINING: AtomicUsize = AtomicUsize::new(16);
static NAVIGATION_DENIAL_LOGS_REMAINING: AtomicUsize = AtomicUsize::new(16);
static NEXT_OPERATION_ID: AtomicU64 = AtomicU64::new(1);
static NATIVE_APPEARANCE: AtomicU8 = AtomicU8::new(APPEARANCE_SYSTEM);
static BLOCKER_STATUS_QUERY_IN_FLIGHT: AtomicBool = AtomicBool::new(false);

struct AtomicFlagReset(&'static AtomicBool);

impl Drop for AtomicFlagReset {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

const APPEARANCE_SYSTEM: u8 = 0;
const APPEARANCE_LIGHT: u8 = 1;
const APPEARANCE_DARK: u8 = 2;

const EVENT_ITEMS: &str = "zephium:items";
const EVENT_TAB: &str = "zephium:tab";
const EVENT_FAVICONS: &str = "zephium:favicons";
const EVENT_EXTENSION_ACTIONS: &str = "zephium:extension-actions";
const EVENT_EXTENSION_ACTION_FAILED: &str = "zephium:extension-action-failed";
const EVENT_WEB_EXTENSION_ACCESS: &str = "zephium:web-extension-access";
const EVENT_WEB_EXTENSION_DROPPED: &str = "zephium:web-extension-dropped";
const EVENT_EXTENSION_ACTION_SHORTCUT: &str = "zephium:extension-action-shortcut";
const EVENT_PAGE_PERMISSION_PROMPT: &str = "zephium:page-permission-prompt";
const EVENT_PRESENTATION_TAB: &str = "zephium:presentation-tab";
const EVENT_UI: &str = "zephium:ui-command";
const EVENT_SEARCH: &str = "zephium:search";
const EVENT_LAYOUT: &str = "zephium:layout";
const EVENT_RUNTIME_STATUS: &str = "zephium:runtime-status";
const EVENT_BLOCKER_STATUS: &str = "zephium:blocker-status";
const EVENT_FOCUS: &str = "zephium:focus";
const EVENT_OPERATION_PROCESSED: &str = "zephium:operation-processed";
// Accepted operations are never evicted before privileged chrome explicitly
// acknowledges the actor's disposition. Refuse new admission at the bound
// rather than lose the public record of how accepted work was processed.
const MAX_OPERATION_LEDGER_ENTRIES: usize = 1024;
const BLOCKER_STATUS_QUERY_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(250);
const PRE_SHELL_CLEANUP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(8);

const PRIVILEGED_PERMISSIONS_POLICY: &str = "accelerometer=(), attribution-reporting=(), autoplay=(), browsing-topics=(), camera=(), clipboard-read=(), clipboard-write=(), compute-pressure=(), display-capture=(), document-domain=(), encrypted-media=(), fullscreen=(), gamepad=(), geolocation=(), gyroscope=(), hid=(), idle-detection=(), join-ad-interest-group=(), local-fonts=(), magnetometer=(), microphone=(), midi=(), payment=(), picture-in-picture=(), private-state-token-issuance=(), private-state-token-redemption=(), publickey-credentials-get=(), run-ad-auction=(), screen-wake-lock=(), serial=(), speaker-selection=(), storage-access=(), sync-xhr=(), unload=(), usb=(), web-share=(), window-management=(), xr-spatial-tracking=()";
const PRIVILEGED_BOOTSTRAP_URL: &str = "about:blank";
#[cfg(any(target_os = "windows", test))]
const PRIVILEGED_WEBVIEW2_BROWSER_ARGS: &str = "--disable-features=msWebOOUI,msPdfOOUI";

#[derive(Clone)]
struct ShutdownCoordinator {
    started: Arc<AtomicBool>,
    terminal_started: Arc<AtomicBool>,
    startup_admission: Arc<Mutex<()>>,
    terminal_failure: Arc<AtomicBool>,
    authorized_exit_code: Arc<AtomicI32>,
    watchdog: Arc<HardExitWatchdog>,
}

struct StartupOwner<T> {
    inner: Arc<Mutex<Option<Arc<T>>>>,
}

impl<T> Clone for StartupOwner<T> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

impl<T> Default for StartupOwner<T> {
    fn default() -> Self {
        Self {
            inner: Arc::new(Mutex::new(None)),
        }
    }
}

impl<T> StartupOwner<T> {
    fn install(&self, value: Arc<T>) -> bool {
        let mut slot = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if slot.is_some() {
            return false;
        }
        *slot = Some(value);
        true
    }

    fn take(&self) -> Option<Arc<T>> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
    }

    fn transfer_to(&self, owner: &Arc<T>) -> bool {
        let mut slot = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if !slot
            .as_ref()
            .is_some_and(|candidate| Arc::ptr_eq(candidate, owner))
        {
            return false;
        }
        slot.take();
        true
    }
}

type StartupEngine = StartupOwner<WebviewEngine>;
type StartupBlocker = StartupOwner<ManagedBlocker>;
/// Retains the storage actor from admission until the suspended Shell becomes
/// the sole ordered-cleanup owner.
type StartupStore = StartupOwner<SqliteStore>;

/// Complete move/temporary-owner set for a terminal failure before Shell has
/// become the sole lifecycle coordinator. Direct owners are retained when a
/// one-shot startup slot refuses installation, so no native worker is shut
/// down synchronously from Tauri's setup callback.
#[derive(Default)]
struct TerminalStartupResources {
    shell: Option<Handle>,
    engine_owner: Option<StartupEngine>,
    direct_blocker: Option<Arc<ManagedBlocker>>,
    blocker_owner: Option<StartupBlocker>,
    direct_store: Option<Arc<SqliteStore>>,
    store_owner: Option<StartupStore>,
}

#[derive(Default)]
struct ClaimedTerminalStartupResources {
    engine: Option<Arc<WebviewEngine>>,
    direct_blocker: Option<Arc<ManagedBlocker>>,
    retained_blocker: Option<Arc<ManagedBlocker>>,
    direct_store: Option<Arc<SqliteStore>>,
    retained_store: Option<Arc<SqliteStore>>,
}

impl TerminalStartupResources {
    fn claim(self) -> (Option<Handle>, ClaimedTerminalStartupResources) {
        let Self {
            shell,
            engine_owner,
            direct_blocker,
            blocker_owner,
            direct_store,
            store_owner,
        } = self;
        (
            shell,
            ClaimedTerminalStartupResources {
                engine: engine_owner.and_then(|owner| owner.take()),
                direct_blocker,
                retained_blocker: blocker_owner.and_then(|owner| owner.take()),
                direct_store,
                retained_store: store_owner.and_then(|owner| owner.take()),
            },
        )
    }
}

impl ClaimedTerminalStartupResources {
    fn is_empty(&self) -> bool {
        self.engine.is_none()
            && self.direct_blocker.is_none()
            && self.retained_blocker.is_none()
            && self.direct_store.is_none()
            && self.retained_store.is_none()
    }
}

#[derive(Default)]
struct HardExitWatchdog {
    prepared: AtomicBool,
    deadline: Mutex<Option<std::time::Instant>>,
    changed: std::sync::Condvar,
}

#[derive(Clone)]
struct UiStartupGate {
    expected_url: Arc<Mutex<tauri::Url>>,
    /// Set when this launch opened onboarding first: the window then shows
    /// onboarding, and later hands over to the browser in place.
    handover: Option<Handover>,
    document_loaded: Arc<AtomicBool>,
    frontend_ready: Arc<AtomicBool>,
    visible: Arc<AtomicBool>,
    handed_over: Arc<AtomicBool>,
    browser_revealed: Arc<AtomicBool>,
}

// WebView2 cold starts compete with runtime servicing and antivirus scanning.
// Keep initialization bounded without terminating an otherwise healthy launch.
#[cfg(target_os = "windows")]
const UI_INITIALIZATION_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);
#[cfg(not(target_os = "windows"))]
const UI_INITIALIZATION_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

#[derive(Clone)]
struct Handover {
    onboarding: tauri::Url,
    browser: tauri::Url,
}

impl UiStartupGate {
    fn new(expected_url: tauri::Url) -> Self {
        Self {
            expected_url: Arc::new(Mutex::new(expected_url)),
            handover: None,
            document_loaded: Arc::new(AtomicBool::new(false)),
            frontend_ready: Arc::new(AtomicBool::new(false)),
            visible: Arc::new(AtomicBool::new(false)),
            handed_over: Arc::new(AtomicBool::new(false)),
            browser_revealed: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Onboarding first, then the browser in the same window.
    fn onboarding_first(onboarding: tauri::Url, browser: tauri::Url) -> Self {
        Self {
            handover: Some(Handover {
                onboarding: onboarding.clone(),
                browser,
            }),
            ..Self::new(onboarding)
        }
    }

    fn expected(&self) -> tauri::Url {
        self.expected_url
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    fn mark_document_loaded(&self, window: &WebviewWindow, loaded_url: &tauri::Url) {
        if loaded_url != &self.expected() {
            return;
        }
        #[cfg(feature = "work-development-traces")]
        startup_styles::capture(window, &self.expected_url, "document-loaded");
        self.document_loaded.store(true, Ordering::Release);
        self.show_if_ready(window);
    }

    fn mark_frontend_ready(&self, window: &WebviewWindow) -> bool {
        let Ok(current_url) = window.url() else {
            return false;
        };
        if current_url != self.expected() {
            return false;
        }
        #[cfg(feature = "work-development-traces")]
        startup_styles::capture(window, &self.expected_url, "frontend-ready");
        self.frontend_ready.store(true, Ordering::Release);
        self.show_if_ready(window);
        true
    }

    fn show_if_ready(&self, window: &WebviewWindow) {
        if !self.document_loaded.load(Ordering::Acquire)
            || !self.frontend_ready.load(Ordering::Acquire)
        {
            return;
        }
        #[cfg(target_os = "windows")]
        if platform::imp::renderer::ready(window)
            && !platform::imp::set_chrome_hidden(window, false)
        {
            request_startup_failure(window.app_handle(), "could not reveal recovered chrome");
            return;
        }
        if self.handed_over.load(Ordering::Acquire) {
            self.reveal_browser(window);
            return;
        }
        if self
            .visible
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return;
        }

        if let Err(error) = show_initialized_main_window(window) {
            request_startup_failure(
                window.app_handle(),
                format_args!("could not show initialized main window: {error}"),
            );
        } else {
            if let Some(started) = APP_STARTED.get() {
                diagnostic!(
                    "startup: main document initialized and window shown after {} ms",
                    started.elapsed().as_millis()
                );
            }
            on_main_window_mapped(window);
            #[cfg(feature = "work-development-traces")]
            startup_styles::after_show(window, &self.expected_url);
        }
    }

    /// Whether the caller is the onboarding page this launch opened, and it
    /// has not yet handed the window over.
    fn serves_onboarding(&self, window: &WebviewWindow) -> bool {
        let Some(handover) = &self.handover else {
            return false;
        };
        !self.handed_over.load(Ordering::Acquire)
            && window.url().is_ok_and(|url| url == handover.onboarding)
    }

    /// Replaces onboarding with the browser in the same window. The page view
    /// is hidden until the browser has initialized, under the same two facts
    /// startup requires, so neither a half-built browser nor its opaque
    /// first paint is ever shown; the window keeps only its material.
    fn hand_over(&self, window: &WebviewWindow) -> bool {
        let Some(handover) = &self.handover else {
            return false;
        };
        if !self.visible.load(Ordering::Acquire)
            || !self.serves_onboarding(window)
            || self
                .handed_over
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_err()
        {
            return false;
        }
        *self
            .expected_url
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = handover.browser.clone();
        self.document_loaded.store(false, Ordering::Release);
        self.frontend_ready.store(false, Ordering::Release);
        if !platform::imp::set_chrome_hidden(window, true) {
            diagnostic!("onboarding: chrome could not be hidden for the handover");
        }
        if let Err(error) = window.navigate(handover.browser.clone()) {
            request_startup_failure(
                window.app_handle(),
                format_args!("could not open the browser after onboarding: {error}"),
            );
            return false;
        }
        let gate = self.clone();
        let app = window.app_handle().clone();
        let spawned = std::thread::Builder::new()
            .name("zephium-handover-watchdog".into())
            .spawn(move || {
                std::thread::sleep(UI_INITIALIZATION_TIMEOUT);
                if gate.browser_revealed.load(Ordering::Acquire) {
                    return;
                }
                let exit_app = app.clone();
                let _ = app.run_on_main_thread(move || {
                    if !gate.browser_revealed.load(Ordering::Acquire) {
                        request_startup_failure(
                            &exit_app,
                            "trusted browser document did not initialize after onboarding",
                        );
                    }
                });
            });
        if spawned.is_err() {
            diagnostic!("onboarding: handover watchdog could not start");
        }
        true
    }

    fn reveal_browser(&self, window: &WebviewWindow) {
        if self
            .browser_revealed
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return;
        }
        let _ = window.set_resizable(true);
        let _ = window.set_maximizable(true);
        if !platform::imp::set_chrome_hidden(window, false) {
            request_startup_failure(
                window.app_handle(),
                "could not show the browser after onboarding",
            );
        }
    }

    fn is_visible(&self) -> bool {
        self.visible.load(Ordering::Acquire)
    }

    #[cfg(target_os = "windows")]
    fn prepare_recovery(&self, window: &WebviewWindow) -> Option<tauri::Url> {
        self.document_loaded.store(false, Ordering::Release);
        self.frontend_ready.store(false, Ordering::Release);
        platform::imp::set_chrome_hidden(window, true).then(|| self.expected())
    }
}

#[cfg(target_os = "linux")]
fn on_main_window_mapped(window: &WebviewWindow) {
    linux_global_shortcuts::main_window_mapped(window);
}

#[cfg(target_os = "windows")]
fn on_main_window_mapped(window: &WebviewWindow) {
    // DWM can discard the backdrop installed while the window was hidden.
    // Restore it at the first reveal, not at some later focus event, and use
    // the person's saved appearance rather than the operating-system default.
    let appearance = APP_STORE
        .get()
        .and_then(|store| store.app_setting("appearance"))
        .unwrap_or_else(|| "system".to_owned());
    let dark = matches!(
        resolved_native_theme(window, &appearance),
        tauri::Theme::Dark
    );
    if !platform::imp::apply_material(window, dark) {
        diagnostic!("material: main-window backdrop could not be restored after initial reveal");
    }
}

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
fn on_main_window_mapped(_window: &WebviewWindow) {}

fn show_initialized_main_window(window: &WebviewWindow) -> Result<(), String> {
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        // Tao implements Window::show as gtk_window.show_all(), which also
        // remaps raw content children that the native stage deliberately hid
        // while the trusted frontend initialized. The Linux composition root
        // has already marked chrome and its container visible; reveal only
        // the top-level widget and preserve every stage-owned child state.
        platform::imp::show_initialized_top_level(window).map_err(|error| error.to_string())
    }
    #[cfg(not(all(unix, not(target_os = "macos"))))]
    {
        window.show().map_err(|error| error.to_string())
    }
}

const NO_AUTHORIZED_EXIT_CODE: i32 = -1;

pub(crate) fn write_diagnostic(arguments: std::fmt::Arguments<'_>) {
    // `eprintln!` panics when stderr writes fail. Several callers are native
    // Objective-C/COM/GTK callbacks where unwinding is forbidden, so keep
    // diagnostics best-effort and make failure unobservable to control flow.
    let stderr = std::io::stderr();
    let mut stderr = stderr.lock();
    write_diagnostic_to(&mut stderr, arguments);
}

fn write_diagnostic_to(writer: &mut dyn std::io::Write, arguments: std::fmt::Arguments<'_>) {
    let _ = writer.write_fmt(arguments);
    let _ = writer.write_all(b"\n");
}

fn cleanup_pre_shell_resources_until(
    resources: ClaimedTerminalStartupResources,
    deadline: std::time::Instant,
) {
    let ClaimedTerminalStartupResources {
        engine,
        direct_blocker,
        retained_blocker,
        direct_store,
        retained_store,
    } = resources;

    for store in [direct_store, retained_store].into_iter().flatten() {
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            store.shutdown_until(deadline)
        })) {
            Ok(StoreShutdownOutcome::Clean) => {}
            Ok(StoreShutdownOutcome::RetryableFailure) => {
                write_diagnostic(format_args!(
                    "startup: storage rejected terminal cleanup before the deadline"
                ));
            }
            Ok(StoreShutdownOutcome::Unclean) => write_diagnostic(format_args!(
                "startup: storage termination could not be proven before process exit"
            )),
            Err(_) => write_diagnostic(format_args!("startup: storage terminal cleanup panicked")),
        }
    }

    // Native teardown is admitted before blocker joins. Both then consume the
    // same caller-owned deadline, and the engine Arc remains alive until its
    // exact completion callback has been observed.
    let native_cleanup = engine.map(|engine| {
        let (native_done, native_wait) = std::sync::mpsc::sync_channel(1);
        if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            engine.shutdown(Box::new(move |clean| {
                let _ = native_done.send(clean);
            }));
        }))
        .is_err()
        {
            write_diagnostic(format_args!(
                "startup: pre-shell native engine cleanup admission panicked"
            ));
        }
        (engine, native_wait)
    });
    for blocker in [direct_blocker, retained_blocker].into_iter().flatten() {
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            blocker.shutdown_until(deadline)
        })) {
            Ok(BlockerShutdownOutcome::Clean) => {}
            Ok(BlockerShutdownOutcome::Unclean) => write_diagnostic(format_args!(
                "startup: pre-shell blocker updater/compiler cleanup was not proven before the deadline"
            )),
            Err(_) => write_diagnostic(format_args!(
                "startup: pre-shell blocker updater/compiler cleanup panicked"
            )),
        }
    }
    if let Some((_engine, native_wait)) = native_cleanup {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        if native_wait.recv_timeout(remaining) != Ok(true) {
            write_diagnostic(format_args!(
                "startup: pre-shell native engine cleanup was not proven before the deadline"
            ));
        }
    }
}

fn cleanup_supplemental_startup_resources(
    direct_blocker: Option<Arc<ManagedBlocker>>,
    direct_store: Option<Arc<SqliteStore>>,
) {
    if direct_blocker.is_none() && direct_store.is_none() {
        return;
    }
    tauri::async_runtime::spawn_blocking(move || {
        let now = std::time::Instant::now();
        let deadline = now.checked_add(PRE_SHELL_CLEANUP_TIMEOUT).unwrap_or(now);
        cleanup_pre_shell_resources_until(
            ClaimedTerminalStartupResources {
                direct_blocker,
                direct_store,
                ..ClaimedTerminalStartupResources::default()
            },
            deadline,
        );
    });
}

impl HardExitWatchdog {
    fn prepare(self: &Arc<Self>) -> std::io::Result<()> {
        if self
            .prepared
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Ok(());
        }

        let watchdog = self.clone();
        let spawned = std::thread::Builder::new()
            .name("zephium-hard-exit".into())
            .spawn(move || {
                let mut deadline = watchdog
                    .deadline
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                while deadline.is_none() {
                    deadline = watchdog
                        .changed
                        .wait(deadline)
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                }
                let armed_deadline = (*deadline).unwrap_or_else(std::time::Instant::now);
                drop(deadline);

                let remaining =
                    armed_deadline.saturating_duration_since(std::time::Instant::now());
                if !remaining.is_zero() {
                    std::thread::sleep(remaining);
                }
                write_diagnostic(format_args!(
                    "shutdown: process exit did not complete within the hard deadline; forcing unsuccessful termination"
                ));
                std::process::exit(1);
            });
        if let Err(error) = spawned {
            self.prepared.store(false, Ordering::Release);
            return Err(error);
        }
        Ok(())
    }

    fn arm(&self) -> bool {
        if !self.prepared.load(Ordering::Acquire) {
            return false;
        }
        let mut deadline = self
            .deadline
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if deadline.is_none() {
            *deadline = Some(std::time::Instant::now() + std::time::Duration::from_secs(12));
            self.changed.notify_one();
        }
        true
    }
}

impl Default for ShutdownCoordinator {
    fn default() -> Self {
        Self {
            started: Arc::new(AtomicBool::new(false)),
            terminal_started: Arc::new(AtomicBool::new(false)),
            startup_admission: Arc::new(Mutex::new(())),
            terminal_failure: Arc::new(AtomicBool::new(false)),
            authorized_exit_code: Arc::new(AtomicI32::new(NO_AUTHORIZED_EXIT_CODE)),
            watchdog: Arc::new(HardExitWatchdog::default()),
        }
    }
}

impl ShutdownCoordinator {
    /// Linearizes every terminal-start publication against the Shell's sole
    /// startup-admission transition. The atomic mirror keeps later setup
    /// gates read-only and lock-free.
    fn mark_terminal_start(&self) {
        let admission = self
            .startup_admission
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        self.terminal_started.store(true, Ordering::Release);
        drop(admission);
    }

    fn try_startup_admission(&self, admit: impl FnOnce() -> bool) -> bool {
        let admission = self
            .startup_admission
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let admitted = !self.terminal_started.load(Ordering::Acquire) && admit();
        drop(admission);
        admitted
    }

    fn try_admit_shell(&self, shell: &Handle) -> bool {
        self.try_startup_admission(|| shell.admit_startup())
    }

    fn prepare_hard_exit_watchdog(&self) -> std::io::Result<()> {
        self.watchdog.prepare()
    }

    /// Publishes terminal intent and arms the independent process deadline
    /// before actor-owned cleanup begins. This does not claim the teardown
    /// single-flight gate, so the correlated Shell request remains authoritative.
    fn signal_terminal_start(&self, app: &tauri::AppHandle) {
        self.mark_terminal_start();
        self.terminal_failure.store(true, Ordering::Release);
        if !self.arm_hard_exit_watchdog() {
            write_diagnostic(format_args!(
                "shutdown: terminal signal could not arm the prepared hard-exit watchdog"
            ));
            self.schedule_authorized_exit(app.clone(), 1);
        }
    }

    fn terminal_started(&self) -> bool {
        self.terminal_started.load(Ordering::Acquire)
    }

    fn request(&self, app: tauri::AppHandle, shell: Handle) {
        self.mark_terminal_start();
        #[cfg(target_os = "linux")]
        linux_global_shortcuts::shutdown(&app);
        if self.started.swap(true, Ordering::AcqRel) {
            return;
        }
        if !self.arm_hard_exit_watchdog() {
            self.terminal_failure.store(true, Ordering::Release);
            write_diagnostic(format_args!(
                "shutdown: hard-exit watchdog was not prepared; requesting immediate unsuccessful event-loop exit"
            ));
            self.schedule_authorized_exit(app, 1);
            return;
        }
        #[cfg(feature = "work-product")]
        if let Some(work) = app.try_state::<work_product::WorkProductState>() {
            let operations = work.operations.clone();
            let coordinator = self.clone();
            tauri::async_runtime::spawn(async move {
                if !operations.shutdown().await {
                    coordinator.terminal_failure.store(true, Ordering::Release);
                    diagnostic!("shutdown: Work operation cleanup was not proven");
                }
                coordinator.request_after_work(app, shell);
            });
            return;
        }
        self.request_after_work(app, shell);
    }

    fn request_after_work(&self, app: tauri::AppHandle, shell: Handle) {
        let completion = shell.shutdown();
        let coordinator = self.clone();
        // SQLite and the shell actor are blocking by design. Wait away from
        // the event loop, then schedule a permitted exit back onto it.
        tauri::async_runtime::spawn_blocking(move || {
            let outcome = shutdown_receive_outcome(completion.recv_until_deadline());
            coordinator.finish_requested_shutdown(app, outcome);
        });
    }

    fn finish_requested_shutdown(&self, app: tauri::AppHandle, outcome: ShutdownOutcome) {
        match outcome {
            ShutdownOutcome::RetryableFailure => {
                // The shell has not started native teardown, so an embedding
                // host could safely retry. A desktop close is terminal: exit
                // non-zero instead of leaving an unclosable process.
                diagnostic!(
                    "shutdown: final session durability was not proven before the deadline; exiting unsuccessfully"
                );
            }
            ShutdownOutcome::Clean => {}
            ShutdownOutcome::Unclean => {
                // The actor may be dead or native teardown may already be
                // partial, so resuming is unsafe.
                diagnostic!("shutdown: clean completion was not proven; exiting unsuccessfully");
            }
        }
        // Startup admission failure is sticky. Even a subsequently clean
        // shell/store teardown cannot turn failed initialization successful.
        let exit_code =
            coordinated_exit_code(outcome, self.terminal_failure.load(Ordering::Acquire));
        self.schedule_authorized_exit(app, exit_code);
    }

    fn request_terminal_startup_failure(
        &self,
        app: tauri::AppHandle,
        resources: TerminalStartupResources,
    ) {
        let TerminalStartupResources {
            shell,
            engine_owner,
            mut direct_blocker,
            blocker_owner,
            mut direct_store,
            store_owner,
        } = resources;
        self.mark_terminal_start();
        self.terminal_failure.store(true, Ordering::Release);
        if let Some(shell) = shell {
            cleanup_supplemental_startup_resources(direct_blocker.take(), direct_store.take());
            // Once the shell exists it is the sole authority for the ordered
            // Store -> native teardown protocol.
            // `request` observes the sticky failure bit and exits non-zero
            // even when cleanup is otherwise clean.
            self.request(app, shell);
            return;
        }
        if self.started.swap(true, Ordering::AcqRel) {
            // The winning callback owns every managed temporary slot. A
            // losing callback can still carry an uninstalled direct owner;
            // reap only that supplemental owner instead of dropping it or
            // racing the winner for shared slots.
            cleanup_supplemental_startup_resources(direct_blocker.take(), direct_store.take());
            return;
        }
        // Claim the single-flight gate before taking any temporary owner.
        // Otherwise concurrent failure callbacks could each remove one
        // resource while only one callback remains authorized to reap it.
        let (_, resources) = TerminalStartupResources {
            shell: None,
            engine_owner,
            direct_blocker,
            blocker_owner,
            direct_store,
            store_owner,
        }
        .claim();
        if !self.arm_hard_exit_watchdog() {
            write_diagnostic(format_args!(
                "startup: hard-exit watchdog was not prepared; continuing bounded cleanup before unsuccessful exit"
            ));
        }
        if !resources.is_empty() {
            let deadline = std::time::Instant::now() + PRE_SHELL_CLEANUP_TIMEOUT;
            let coordinator = self.clone();
            tauri::async_runtime::spawn_blocking(move || {
                cleanup_pre_shell_resources_until(resources, deadline);
                coordinator.schedule_authorized_exit(app, 1);
            });
        } else {
            // Storage admission itself failed, so there is no background
            // resource to reap. Request event-loop exit directly; do not
            // return an error through Tauri's native Ready callback.
            self.authorized_exit_code.store(1, Ordering::Release);
            app.exit(1);
        }
    }

    fn request_unrecoverable_native_failure(&self, app: tauri::AppHandle) {
        // Native authority has already become inconsistent, so asking the
        // shell to drive that same engine through ordered teardown is unsafe.
        // Still return through Tauri's event loop: App drop and Windows
        // `run_return` can then release/prove privileged environments before
        // the process exits unsuccessfully.
        if !self.begin_unrecoverable_native_failure() {
            return;
        }
        if !self.arm_hard_exit_watchdog() {
            write_diagnostic(format_args!(
                "security: hard-exit watchdog was not prepared; requesting immediate unsuccessful event-loop exit"
            ));
        }
        self.schedule_authorized_exit(app, 1);
    }

    fn begin_unrecoverable_native_failure(&self) -> bool {
        self.mark_terminal_start();
        self.terminal_failure.store(true, Ordering::Release);
        // Share the exact single-flight gate with ordinary shutdown. If a
        // durability barrier already owns teardown, make its result nonzero
        // without overtaking its store/native completion.
        !self.started.swap(true, Ordering::AcqRel)
    }

    fn schedule_authorized_exit(&self, app: tauri::AppHandle, exit_code: i32) {
        let exit_coordinator = self.clone();
        let exit_on_main = app.clone();
        if let Err(error) = app.run_on_main_thread(move || {
            // Authorize only this exact main-thread exit call. Publishing the
            // code on a worker before dispatch leaves a window where an
            // unrelated OS request can overtake the correlated result.
            let exit_code = if exit_coordinator.terminal_failure.load(Ordering::Acquire) {
                1
            } else {
                exit_code
            };
            exit_coordinator
                .authorized_exit_code
                .store(exit_code, Ordering::Release);
            exit_on_main.exit(exit_code);
        }) {
            write_diagnostic(format_args!(
                "shutdown: main-thread exit dispatch failed: {error}"
            ));
            // Keep the same invariant for the direct fallback: publish
            // immediately before the correlated request.
            let exit_code = if self.terminal_failure.load(Ordering::Acquire) {
                1
            } else {
                exit_code
            };
            self.authorized_exit_code
                .store(exit_code, Ordering::Release);
            app.exit(exit_code);
        }
    }

    fn arm_hard_exit_watchdog(&self) -> bool {
        self.watchdog.arm()
    }
}

fn shutdown_exit_code(outcome: ShutdownOutcome) -> i32 {
    if outcome == ShutdownOutcome::Clean {
        0
    } else {
        1
    }
}

fn coordinated_exit_code(outcome: ShutdownOutcome, terminal_failure: bool) -> i32 {
    if terminal_failure {
        1
    } else {
        shutdown_exit_code(outcome)
    }
}

fn exit_request_is_authorized(requested: Option<i32>, authorized: i32) -> bool {
    requested.is_some_and(|code| code == authorized && code != NO_AUTHORIZED_EXIT_CODE)
}

fn shutdown_receive_outcome(
    received: Result<ShutdownOutcome, std::sync::mpsc::RecvTimeoutError>,
) -> ShutdownOutcome {
    match received {
        Ok(outcome) => outcome,
        Err(error) => {
            // Timeout and disconnect are both terminal here. The actor either
            // exceeded the same end-to-end deadline it received at admission
            // or exited without proving native/private-data cleanup.
            diagnostic!("shutdown: shell actor did not acknowledge the barrier: {error}");
            ShutdownOutcome::Unclean
        }
    }
}

type SetupResult = Result<(), Box<dyn std::error::Error>>;

/// Tauri 2.11 turns a setup-hook `Err` into a panic from its runtime Ready
/// callback. On macOS that callback is entered from Objective-C, so release
/// `panic = "abort"` terminates the process before Tauri/native cleanup can
/// run. Keep environmental failures inside our own fallible transaction and
/// make the framework-facing hook unconditionally successful.
fn contain_tauri_setup_failure<E>(
    result: Result<(), E>,
    on_failure: impl FnOnce(E),
) -> SetupResult {
    if let Err(error) = result {
        on_failure(error);
    }
    Ok(())
}

fn request_startup_failure(app: &tauri::AppHandle, error: impl std::fmt::Display) {
    let error = error.to_string();
    write_diagnostic(format_args!(
        "startup: failed to initialize Zephium: {error}"
    ));
    startup_alert::show_then(
        app,
        startup_alert::StartupProblem::classify(&error),
        &error,
        request_orderly_terminal_failure,
    );
}

fn request_shell_terminal_failure(
    app: &tauri::AppHandle,
    coordinator: &ShutdownCoordinator,
    error: impl std::fmt::Display,
) {
    // This is deliberately first: even diagnostics, Tauri state lookup, or
    // async-runtime admission may fail after the actor has begun unwinding.
    coordinator.signal_terminal_start(app);
    write_diagnostic(format_args!(
        "runtime: terminal application shell failure: {error}"
    ));
    request_orderly_terminal_failure(app);
}

fn request_orderly_terminal_failure(app: &tauri::AppHandle) {
    let shell = app.try_state::<Handle>().map(|shell| shell.inner().clone());
    let engine_owner = if shell.is_none() {
        app.try_state::<StartupEngine>()
            .map(|engine| engine.inner().clone())
    } else {
        None
    };
    let blocker_owner = if shell.is_none() {
        app.try_state::<StartupBlocker>()
            .map(|blocker| blocker.inner().clone())
    } else {
        None
    };
    let store_owner = if shell.is_none() {
        app.try_state::<StartupStore>()
            .map(|store| store.inner().clone())
    } else {
        None
    };
    let Some(coordinator) = app.try_state::<ShutdownCoordinator>() else {
        // Builder installs this state before the event loop begins. Even if
        // that invariant is broken, retain the dependency-complete cleanup
        // path instead of abandoning managed workers on the event loop.
        write_diagnostic(format_args!("startup: shutdown coordinator is unavailable"));
        if let Some(shell) = shell {
            let completion = shell.shutdown();
            let exit_app = app.clone();
            tauri::async_runtime::spawn_blocking(move || {
                let _ = completion.recv_until_deadline();
                exit_app.exit(1);
            });
        } else {
            request_pre_shell_cleanup_without_coordinator(
                app,
                TerminalStartupResources {
                    engine_owner,
                    blocker_owner,
                    store_owner,
                    ..TerminalStartupResources::default()
                },
            );
        }
        return;
    };
    coordinator.request_terminal_startup_failure(
        app.clone(),
        TerminalStartupResources {
            shell,
            engine_owner,
            blocker_owner,
            store_owner,
            ..TerminalStartupResources::default()
        },
    );
}

/// Routes temporary startup owners into the same asynchronous,
/// dependency-ordered cleanup when the coordinator state is unavailable. This
/// helper is valid only before Shell construction; callers retain the normal
/// setup error so the outer containment callback can observe the already-owned
/// single-flight failure without starting a second teardown.
fn request_pre_shell_cleanup_without_coordinator(
    app: &tauri::AppHandle,
    resources: TerminalStartupResources,
) {
    let (shell, resources) = resources.claim();
    debug_assert!(shell.is_none());
    let exit_app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let now = std::time::Instant::now();
        let deadline = now.checked_add(PRE_SHELL_CLEANUP_TIMEOUT).unwrap_or(now);
        cleanup_pre_shell_resources_until(resources, deadline);
        exit_app.exit(1);
    });
}

/// Losslessly routes a Store actor refused by the one-shot temporary owner
/// into the same asynchronous dependency cleanup. No Store join may run on
/// Tauri's native setup callback.
fn request_pre_shell_startup_failure_with_store(
    app: &tauri::AppHandle,
    error: impl std::fmt::Display,
    store: Arc<SqliteStore>,
) {
    write_diagnostic(format_args!(
        "startup: failed to initialize Zephium: {error}"
    ));
    let store_owner = app
        .try_state::<StartupStore>()
        .map(|owner| owner.inner().clone());
    let resources = TerminalStartupResources {
        direct_store: Some(store),
        store_owner,
        ..TerminalStartupResources::default()
    };
    let Some(coordinator) = app.try_state::<ShutdownCoordinator>() else {
        write_diagnostic(format_args!("startup: shutdown coordinator is unavailable"));
        request_pre_shell_cleanup_without_coordinator(app, resources);
        return;
    };
    coordinator.request_terminal_startup_failure(app.clone(), resources);
}

/// Losslessly routes a blocker refused by the one-shot temporary owner into
/// the same asynchronous dependency cleanup. In particular, this helper must
/// be used from Tauri setup instead of waiting on blocker workers on the
/// native event-loop thread.
fn request_pre_shell_startup_failure_with_blocker(
    app: &tauri::AppHandle,
    error: impl std::fmt::Display,
    blocker: Arc<ManagedBlocker>,
) {
    write_diagnostic(format_args!(
        "startup: failed to initialize Zephium: {error}"
    ));
    let engine_owner = app
        .try_state::<StartupEngine>()
        .map(|engine| engine.inner().clone());
    let blocker_owner = app
        .try_state::<StartupBlocker>()
        .map(|owner| owner.inner().clone());
    let store_owner = app
        .try_state::<StartupStore>()
        .map(|store| store.inner().clone());
    let resources = TerminalStartupResources {
        engine_owner,
        direct_blocker: Some(blocker),
        blocker_owner,
        store_owner,
        ..TerminalStartupResources::default()
    };
    let Some(coordinator) = app.try_state::<ShutdownCoordinator>() else {
        write_diagnostic(format_args!("startup: shutdown coordinator is unavailable"));
        request_pre_shell_cleanup_without_coordinator(app, resources);
        return;
    };
    coordinator.request_terminal_startup_failure(app.clone(), resources);
}

fn request_unrecoverable_native_failure(app: &tauri::AppHandle, reason: &str) {
    write_diagnostic(format_args!(
        "security: terminal native lifecycle failure: {reason}"
    ));
    let Some(coordinator) = app.try_state::<ShutdownCoordinator>() else {
        write_diagnostic(format_args!(
            "security: shutdown coordinator is unavailable"
        ));
        app.exit(1);
        return;
    };
    coordinator.request_unrecoverable_native_failure(app.clone());
}

#[derive(Clone, Debug, Serialize, Deserialize, specta::Type, Event)]
struct WorkChanged(zephium_ipc::work::WorkChangedV1);

#[derive(Clone, Debug, Serialize, Deserialize, specta::Type, Event)]
#[tauri_specta(event_name = "zephium:work-human-changed")]
struct WorkHumanChanged(zephium_ipc::work::WorkHumanChangedV1);

#[derive(Clone, Debug, Serialize, Deserialize, specta::Type, Event)]
#[tauri_specta(event_name = "zephium:work-decision-preference-changed")]
struct WorkDecisionPreferenceChanged(zephium_ipc::work::WorkDecisionPreferenceChangedV1);

#[derive(Clone, Debug, Serialize, Deserialize, specta::Type, Event)]
struct WorkEnvironmentChanged(zephium_ipc::work::WorkEnvironmentChangedV1);

#[derive(Clone, Debug, Serialize, Deserialize, specta::Type, Event)]
struct NoteOpenRequested {
    profile: String,
    id: String,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
enum ResourceChangeKind {
    Task,
    TaskList,
    Object,
    Media,
}

#[derive(Clone, Debug, Serialize, Deserialize, specta::Type, Event)]
struct DownloadsChanged {
    profile: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, specta::Type, Event)]
struct ResourceChanged {
    profile: String,
    kind: ResourceChangeKind,
    id: String,
    revision: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, specta::Type, Event)]
struct ItemsChanged(zephium_ipc::ItemsState);

#[derive(Clone, Debug, Serialize, Deserialize, specta::Type, Event)]
struct TabChanged(zephium_ipc::TabView);

#[derive(Clone, Debug, Serialize, Deserialize, specta::Type, Event)]
struct FaviconsChanged(zephium_ipc::FaviconsView);

#[derive(Clone, Debug, Serialize, Deserialize, specta::Type, Event)]
struct ExtensionActionsChanged(zephium_ipc::ExtensionActionsView);

#[derive(Clone, Debug, Serialize, Deserialize, specta::Type, Event)]
struct ExtensionActionFailed(zephium_ipc::ExtensionActionFailedView);

#[derive(Clone, Debug, Serialize, Deserialize, specta::Type, Event)]
struct ExtensionActionShortcut(zephium_ipc::ExtensionActionShortcutView);

#[derive(Clone, Debug, Serialize, Deserialize, specta::Type, Event)]
struct WebExtensionAccessRequested(zephium_ipc::WebExtensionAccessRequestView);

#[derive(Clone, Debug, Serialize, Deserialize, specta::Type, Event)]
struct PagePermissionPromptChanged(zephium_ipc::PagePermissionPromptView);

#[derive(Clone, Debug, Serialize, Deserialize, specta::Type, Event)]
struct UiCommand(String);

#[derive(Clone, Debug, Serialize, Deserialize, specta::Type, Event)]
struct SearchChanged(zephium_ipc::SearchResults);

#[derive(Clone, Debug, Serialize, Deserialize, specta::Type, Event)]
struct FindChanged(zephium_ipc::FindResultView);

const EVENT_FIND: &str = "zephium:find";

#[derive(Clone, Debug, Serialize, Deserialize, specta::Type, Event)]
struct LayoutChanged(zephium_ipc::LayoutState);

#[derive(Clone, Debug, Serialize, Deserialize, specta::Type, Event)]
struct RuntimeStatusChanged(zephium_ipc::RuntimeStatus);

#[derive(Clone, Debug, Serialize, Deserialize, specta::Type, Event)]
struct BlockerStatusChanged(zephium_ipc::BlockerStatusView);

#[derive(Clone, Debug, Serialize, Deserialize, specta::Type, Event)]
struct FocusChanged(zephium_ipc::FocusStatus);

#[derive(Clone, Debug, Serialize, Deserialize, specta::Type, Event)]
struct OperationProcessed(zephium_ipc::OperationDisposition);

#[derive(Clone, Default)]
struct OperationLedger {
    inner: Arc<Mutex<OperationLedgerInner>>,
}

#[derive(Default)]
struct OperationLedgerInner {
    entries: HashMap<String, OperationRecord>,
    order: VecDeque<String>,
}

enum OperationRecord {
    Pending,
    Processed(zephium_ipc::OperationDisposition),
}

impl OperationLedger {
    fn reserve(&self, operation_id: &str) -> bool {
        if !valid_operation_id(operation_id) {
            return false;
        }
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if inner.entries.len() >= MAX_OPERATION_LEDGER_ENTRIES
            || inner.entries.contains_key(operation_id)
        {
            return false;
        }
        inner
            .entries
            .insert(operation_id.to_owned(), OperationRecord::Pending);
        inner.order.push_back(operation_id.to_owned());
        true
    }

    fn cancel_reservation(&self, operation_id: &str) {
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if matches!(
            inner.entries.get(operation_id),
            Some(OperationRecord::Pending)
        ) {
            inner.entries.remove(operation_id);
            inner.order.retain(|candidate| candidate != operation_id);
        }
    }

    /// Records the terminal actor result before any fallible WebView delivery.
    /// Duplicate or unreserved disposition is rejected instead of overwriting
    /// the first authoritative value.
    fn record_disposition(&self, disposition: zephium_ipc::OperationDisposition) -> bool {
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let Some(record) = inner.entries.get_mut(&disposition.operation_id) else {
            return false;
        };
        if !matches!(record, OperationRecord::Pending) {
            return false;
        }
        *record = OperationRecord::Processed(disposition);
        true
    }

    fn status(&self, operation_id: &str) -> zephium_ipc::OperationStatus {
        if !valid_operation_id(operation_id) {
            return zephium_ipc::OperationStatus::Unknown;
        }
        let inner = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match inner.entries.get(operation_id) {
            Some(OperationRecord::Pending) => zephium_ipc::OperationStatus::Pending,
            Some(OperationRecord::Processed(disposition)) => {
                zephium_ipc::OperationStatus::Processed {
                    disposition: disposition.clone(),
                }
            }
            None => zephium_ipc::OperationStatus::Unknown,
        }
    }

    fn processed(&self) -> Vec<zephium_ipc::OperationDisposition> {
        let inner = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        inner
            .order
            .iter()
            .filter_map(|operation_id| match inner.entries.get(operation_id) {
                Some(OperationRecord::Processed(disposition)) => Some(disposition.clone()),
                _ => None,
            })
            .collect()
    }

    fn acknowledge(&self, operation_id: &str) -> bool {
        if !valid_operation_id(operation_id) {
            return false;
        }
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if !matches!(
            inner.entries.get(operation_id),
            Some(OperationRecord::Processed(_))
        ) {
            return false;
        }
        inner.entries.remove(operation_id);
        inner.order.retain(|candidate| candidate != operation_id);
        true
    }
}

fn record_and_deliver_operation(
    ledger: &OperationLedger,
    disposition: zephium_ipc::OperationDisposition,
    deliver: impl FnOnce(&zephium_ipc::OperationDisposition) -> bool,
) -> bool {
    if !ledger.record_disposition(disposition.clone()) {
        return false;
    }
    // Delivery is opportunistic. The ledger remains authoritative until an
    // explicit privileged acknowledgement, including when no window/listener
    // exists or JavaScript evaluation fails.
    let _ = deliver(&disposition);
    true
}

fn specta_builder() -> tauri_specta::Builder<tauri::Wry> {
    tauri_specta::Builder::<tauri::Wry>::new()
        .commands(collect_commands![
            work_product::work_call,
            work_product::work_operation,
            work_product::work_operation_status,
            work_product::work_context_preview,
            media::media_import,
            media::media_open,
            media::media_admit_remote,
            media::work_admit_folder,
            media::work_pick_folder,
            media::work_reveal_path,
            work_memory::work_release_memory,
            work_folders::work_choose_folder,
            work_product::work_activity,
            work_product::human::work_human_pages,
            work_product::human::work_human_present,
            work_product::human::work_human_continue,
            work_product::human::work_human_release,
            work_decision::work_decision_preference,
            work_decision::work_set_decision_preference,
            work_decision::work_set_decision_key,
            work_decision::work_clear_decision_key,
            work_models::work_models,
            work_models::work_models_ready,
            work_models::work_choose_model,
            work_models::work_set_provider_key,
            work_models::work_test_provider_key,
            work_models::work_clear_provider_key,
            work_models::work_set_model_endpoint,
            work_models::work_more_models,
            work_sites::work_sites,
            work_sites::work_set_site,
            work_connections::work_connections,
            work_connections::work_save_connection,
            work_connections::work_preview_connection,
            work_connections::work_remove_connection,
            work_connections::work_check_connection,
            work_connections::work_sign_in_connection,
            work_connections::work_cancel_connection_sign_in,
            work_personal::work_memories,
            work_personal::work_change_memory,
            work_personal::work_skills,
            work_personal::work_skill_text,
            work_personal::work_change_skill,
            tabs_bootstrap,
            tabs_open,
            tabs_activate,
            tabs_close,
            tabs_set_essential,
            essentials_keep,
            profile_rename,
            onboarding_play_intro,
            onboarding_finish,
            tabs_navigate,
            tabs_reload,
            tabs_answer_page_request,
            diagnostics_show_logs,
            tabs_back,
            tabs_forward,
            work_pane_show,
            work_pane_set_rect,
            work_pane_hide,
            tabs_split,
            tabs_unsplit,
            tabs_leave_split,
            chrome_menu_popup,
            extension_action_invoke,
            webext::web_extension_prepare,
            webext::web_extension_confirm,
            webext::web_extension_cancel,
            webext::web_extension_list,
            webext::web_extension_set_enabled,
            webext::web_extension_uninstall,
            webext::web_extension_answer_access,
            webext::web_extension_set_access,
            webext::web_extension_open_options,
            webext::web_extension_choose_file,
            webext::web_extension_prepare_file,
            webext::web_extension_review_update,
            webext::web_extension_catalog_icon,
            browser_credentials::browser_credential_capability,
            browser_credentials::browser_passkey_authorization_request,
            page_permission_respond,
            capture_stop,
            blocker_status,
            blocker_stats,
            blocker_set_enabled,
            blocker_site_change,
            blocker_picker,
            blocker_retry,
            blocker_refresh_sources,
            profiles_delete,
            operation_status,
            operations_reconcile,
            operation_acknowledge,
            run_command,
            panel_hide,
            panel_ready,
            panel_intent,
            panel_layout,
            launcher_trigger,
            launcher_set_shortcut,
            launcher_record_shortcut,
            launcher_set_double_tap,
            launcher_open_accessibility,
            setting_get,
            setting_set,
            ui_info,
            ui_ready,
            menu_popup,
            add_menu_popup,
            tab_menu_popup,
            bookmark_menu_popup,
            profile_menu_popup,
            sidebar_menu_popup,
            tools_menu_popup,
            newtab_search_context,
            newtab_search,
            newtab_run,
            newtab_cancel,
            launcher_search,
            launcher_run,
            sidebar_set_width,
            sidebar_resize,
            sidebar_resize_guide,
            tab_drag_over,
            resource_call,
            notes::note_call,
            history_call,
            favicon_probe,
            download_call,
            download_open_access_settings,
            about::about_info,
            updates::update_status,
            updates::update_check,
            updates::update_relaunch,
            updates::update_highlights,
            updates::open_software_update,
            browser_open_url,
            keymap::keymap_entries,
            keymap::keymap_bind,
            keymap::keymap_reset,
            keymap::keymap_record,
            default_browser::default_browser_status,
            default_browser::default_browser_request,
            bookmark_call,
            time_call,
            focus_control,
            page_find,
            browser_import::import_sources,
            browser_import::import_start,
            browser_import::import_cancel,
            browser_import::import_open_permission,
            resource_close_ready,
            tab_drop,
            divider_grab,
            divider_drag,
            divider_release
        ])
        .events(collect_events![
            keymap::KeymapChanged,
            browser_import::ImportJobView,
            WorkEnvironmentChanged,
            WorkChanged,
            WorkHumanChanged,
            WorkDecisionPreferenceChanged,
            work_models::WorkModelsChanged,
            ItemsChanged,
            FaviconsChanged,
            ResourceChanged,
            notes::NotesChanged,
            DownloadsChanged,
            NoteOpenRequested,
            TabChanged,
            ExtensionActionsChanged,
            ExtensionActionFailed,
            ExtensionActionShortcut,
            WebExtensionAccessRequested,
            browser_credentials::BrowserCredentialCapabilityChanged,
            PagePermissionPromptChanged,
            UiCommand,
            FindChanged,
            SearchChanged,
            LayoutChanged,
            RuntimeStatusChanged,
            BlockerStatusChanged,
            FocusChanged,
            OperationProcessed
        ])
}

fn ui_navigation_allowed(url: &tauri::Url) -> bool {
    let clean_authority = url.username().is_empty() && url.password().is_none();
    // Privileged WebViews are constructed at the browser-generated empty
    // document, hardened natively, and only then navigated to the app origin.
    // Keep this exact: other about: URLs and even fragments/queries are not a
    // bootstrap document.
    let bootstrap = url.as_str() == PRIVILEGED_BOOTSTRAP_URL;
    #[cfg(target_os = "windows")]
    let bundled =
        url.scheme() == "http" && url.host_str() == Some("tauri.localhost") && url.port().is_none();
    #[cfg(not(target_os = "windows"))]
    let bundled =
        url.scheme() == "tauri" && url.host_str() == Some("localhost") && url.port().is_none();
    let development = cfg!(debug_assertions)
        && url.scheme() == "http"
        && url.host_str() == Some("localhost")
        && url.port() == Some(1420);
    clean_authority && (bootstrap || bundled || development)
}

fn privileged_app_url(
    app: &tauri::App,
    configured: &tauri::WebviewUrl,
) -> tauri::Result<tauri::Url> {
    use tauri::utils::config::FrontendDist;

    #[cfg(target_os = "windows")]
    let fallback = "http://tauri.localhost";
    #[cfg(not(target_os = "windows"))]
    let fallback = "tauri://localhost";

    let base = if tauri::is_dev() {
        match app.config().build.dev_url.clone() {
            Some(url) => url,
            None => tauri::Url::parse(fallback).map_err(tauri::Error::InvalidUrl)?,
        }
    } else if let Some(FrontendDist::Url(url)) = app.config().build.frontend_dist.as_ref() {
        url.clone()
    } else {
        tauri::Url::parse(fallback).map_err(tauri::Error::InvalidUrl)?
    };

    resolve_privileged_target(&base, configured).map_err(Into::into)
}

fn resolve_privileged_target(
    base: &tauri::Url,
    configured: &tauri::WebviewUrl,
) -> std::io::Result<tauri::Url> {
    let target = match configured {
        tauri::WebviewUrl::App(path) if path.to_str() == Some("index.html") => base
            .join("browser.html")
            .map_err(|error| std::io::Error::other(error.to_string()))?,
        tauri::WebviewUrl::App(path) => base
            .join(&path.to_string_lossy())
            .map_err(|error| std::io::Error::other(error.to_string()))?,
        tauri::WebviewUrl::External(url) | tauri::WebviewUrl::CustomProtocol(url) => url.clone(),
        _ => {
            return Err(std::io::Error::other(
                "unsupported privileged application URL configuration",
            ));
        }
    };
    if !ui_navigation_allowed(&target) || target.as_str() == PRIVILEGED_BOOTSTRAP_URL {
        return Err(std::io::Error::other(format!(
            "privileged application URL is outside the navigation policy: {target}"
        )));
    }
    Ok(target)
}

#[cfg(target_os = "windows")]
#[derive(Debug)]
struct PrivilegedRuntimeDirectories {
    main: std::path::PathBuf,
    panel: std::path::PathBuf,
}

#[cfg(target_os = "windows")]
fn prepare_privileged_runtime_directories(
    data_dir: &std::path::Path,
) -> std::io::Result<PrivilegedRuntimeDirectories> {
    let prepared = privileged_runtime_windows::prepare(data_dir, MAIN_LABEL, overlay::PANEL_LABEL)?;
    Ok(PrivilegedRuntimeDirectories {
        main: prepared.main,
        panel: prepared.panel,
    })
}

#[cfg(target_os = "windows")]
fn cleanup_privileged_runtime_after_exit() -> bool {
    privileged_runtime_windows::cleanup_current_after_proven_exit()
}

fn navigation_lock() -> tauri::plugin::TauriPlugin<tauri::Wry> {
    tauri::plugin::Builder::new("ui-navigation-lock")
        .on_navigation(|webview, url| {
            let allowed = ui_navigation_allowed(url);
            if !allowed
                && NAVIGATION_DENIAL_LOGS_REMAINING
                    .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
                        value.checked_sub(1)
                    })
                    .is_ok()
            {
                diagnostic!(
                    "security: blocked privileged webview {} navigation to scheme={} host={}",
                    webview.label(),
                    url.scheme(),
                    url.host_str().unwrap_or("<none>")
                );
            }
            allowed
        })
        .build()
}

fn harden_privileged_headers(headers: &mut tauri::http::HeaderMap) {
    use tauri::http::HeaderValue;

    headers.insert(
        "Permissions-Policy",
        HeaderValue::from_static(PRIVILEGED_PERMISSIONS_POLICY),
    );
    headers.insert("Referrer-Policy", HeaderValue::from_static("no-referrer"));
    headers.insert(
        "X-Content-Type-Options",
        HeaderValue::from_static("nosniff"),
    );
    headers.insert("X-DNS-Prefetch-Control", HeaderValue::from_static("off"));
    headers.insert("X-Frame-Options", HeaderValue::from_static("DENY"));
}

/// Delivers a projection directly to one privileged WebView. Generic Tauri
/// event listening accepts a caller-selected target, so capabilities cannot
/// stop a compromised panel from subscribing to the main window's events.
/// Native eval is label-scoped and the payload is serialized as JSON.
fn try_emit_to_privileged<T: Serialize>(
    app: &tauri::AppHandle,
    label: &str,
    event: &str,
    payload: &T,
) -> bool {
    let (Ok(event), Ok(payload)) = (serde_json::to_string(event), serde_json::to_string(payload))
    else {
        diagnostic!("projection: failed to serialize privileged event");
        return false;
    };
    let Some(window) = app.get_webview_window(label) else {
        return false;
    };
    let script = format!("window.dispatchEvent(new CustomEvent({event},{{detail:{payload}}}));");
    if let Err(error) = window.eval(&script) {
        diagnostic!("projection: delivery to {label} failed: {error}");
        return false;
    }
    true
}

pub(crate) fn emit_media_changed(app: &tauri::AppHandle, profile: &str, id: &str, revision: &str) {
    let event = ResourceChanged {
        profile: profile.to_owned(),
        kind: ResourceChangeKind::Media,
        id: id.to_owned(),
        revision: revision.to_owned(),
    };
    for label in [MAIN_LABEL, overlay::PANEL_LABEL] {
        emit_to_privileged(app, label, "zephium:resource-changed", &event);
    }
}

fn emit_to_privileged<T: Serialize>(app: &tauri::AppHandle, label: &str, event: &str, payload: &T) {
    let _ = try_emit_to_privileged(app, label, event, payload);
}

/// Applies one exact revision-bearing tab projection and verifies the
/// privileged DOM state in the same JavaScript evaluation. The callback is a
/// lifecycle fact only; the shell independently revalidates id, URL and native
/// navigation identity before revealing raw content.
pub(crate) fn apply_chrome_presentation(
    window: &WebviewWindow,
    presentation: ChromePresentation,
    done: ChromePresentationCallback,
) -> ChromePresentationDispatch {
    if presentation.tab.id != presentation.id.to_string()
        || presentation.tab.url.as_deref() != Some(presentation.url.as_str())
    {
        return ChromePresentationDispatch::Rejected;
    }
    let Ok(event) = serde_json::to_string(EVENT_PRESENTATION_TAB) else {
        return ChromePresentationDispatch::Rejected;
    };
    let Ok(payload) = serde_json::to_string(&presentation.tab) else {
        return ChromePresentationDispatch::Rejected;
    };
    let active = presentation.active.map(|id| id.to_string());
    let Ok(active) = serde_json::to_string(&active) else {
        return ChromePresentationDispatch::Rejected;
    };
    let nonce = format!(
        "zephium-presentation-v1:{}:{:032x}",
        presentation.id,
        presentation.navigation.into_raw()
    );
    let Ok(serialized_nonce) = serde_json::to_string(&nonce) else {
        return ChromePresentationDispatch::Rejected;
    };
    let settings_visible = presentation.settings_visible;
    let script = format!(
        r#"(() => {{
  "use strict";
  const rejected = "zephium-presentation-rejected";
  try {{
    const tab = {payload};
    const active = {active};
    window.dispatchEvent(new CustomEvent({event}, {{ detail: {{ tab, active }} }}));
    if ({settings_visible}) {{
      const shell = document.querySelector("[data-zephium-active-tab]");
      if (shell?.dataset.zephiumSurface !== "settings" || shell.dataset.zephiumActiveTab !== (active ?? "")) return rejected;
      return {serialized_nonce};
    }}
    let row = null;
    for (const candidate of document.querySelectorAll("[data-zephium-tab-id]")) {{
      if (candidate.dataset.zephiumTabId === tab.id) {{ row = candidate; break; }}
    }}
    if (!row || row.dataset.zephiumTabUrl !== (tab.url ?? "") ||
        row.dataset.zephiumProjectionRevision !== tab.projection_revision) return rejected;
    const label = row.querySelector("[data-zephium-tab-label]");
    if (!label || label.textContent !== tab.title) return rejected;
    const shell = document.querySelector("[data-zephium-active-tab]");
    if (!shell) return rejected;
    if (shell.dataset.zephiumActiveTab !== (active ?? "")) return rejected;
    if (active === tab.id) {{
      const address = document.querySelector("[data-zephium-address]");
      if (!(address instanceof HTMLInputElement)) return rejected;
      let expected = "";
      try {{ expected = new URL(tab.url).host; }} catch (_) {{ return rejected; }}
      if (address.value !== expected) {{
        address.value = expected;
        address.dispatchEvent(new Event("input", {{ bubbles: true }}));
      }}
      if (address.value !== expected) return rejected;
    }}
    if (active === tab.id && document.querySelector("[data-zephium-new-tab]")) return rejected;
    void document.documentElement.getBoundingClientRect();
    return {serialized_nonce};
  }} catch (_) {{
    return rejected;
  }}
}})()"#
    );
    let expected = nonce;
    let done = Arc::new(Mutex::new(Some(done)));
    match window.eval_with_callback(script, move |result| {
        let applied =
            serde_json::from_str::<String>(&result).is_ok_and(|returned| returned == expected);
        if let Some(done) = done
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
        {
            done(applied);
        }
    }) {
        Ok(()) => ChromePresentationDispatch::Scheduled,
        Err(_) => {
            diagnostic!("projection: privileged presentation evaluation was not admitted");
            ChromePresentationDispatch::Rejected
        }
    }
}

/// Restores the complete snapshot and checks visible browser identity before
/// the actor is allowed to reattach raw native content.
pub(crate) fn restore_browser_chrome(
    window: &WebviewWindow,
    revision: u64,
    items: zephium_ipc::ItemsState,
    done: ChromePresentationCallback,
) -> ChromePresentationDispatch {
    let Ok(payload) = serde_json::to_string(&items) else {
        return ChromePresentationDispatch::Rejected;
    };
    let nonce = format!("zephium-browser-return:{revision}");
    let Ok(expected) = serde_json::to_string(&nonce) else {
        return ChromePresentationDispatch::Rejected;
    };
    let script = format!(
        r#"(() => {{
      const items = {payload};
      window.dispatchEvent(new CustomEvent('zephium:browser-return', {{ detail: items }}));
      const shell = document.querySelector('[data-zephium-active-tab]');
      if (!shell || shell.dataset.zephiumSurface !== 'browse' || shell.dataset.zephiumActiveTab !== (items.active ?? '')) return '';
      const active = items.tabs.find(tab => tab.id === items.active);
      if (active) {{
        const rows = [...document.querySelectorAll('[data-zephium-tab-id]')].filter(row => row.dataset.zephiumTabId === active.id);
        if (rows.length !== 1) return '';
        const row = rows[0];
        if (row.dataset.zephiumTabUrl !== (active.url ?? '') || row.dataset.zephiumProjectionRevision !== active.projection_revision || row.querySelector('[data-zephium-tab-label]')?.textContent !== active.title) return '';
        const address = document.querySelector('[data-zephium-address]');
        if (!(address instanceof HTMLInputElement)) return '';
        const host = active.url ? new URL(active.url).host : '';
        if (address.value !== host) {{ address.value = host; address.dispatchEvent(new Event('input', {{ bubbles: true }})); }}
        if (address.value !== host) return '';
        if (!!document.querySelector('[data-zephium-new-tab]') !== ((active.content ?? 'web') === 'web' && !active.url && !active.loading)) return '';
      }}
      void document.documentElement.getBoundingClientRect();
      return {expected};
    }})()"#
    );
    let (sender, receiver) = tokio::sync::oneshot::channel();
    let sender = Arc::new(Mutex::new(Some(sender)));
    match window.eval_with_callback(script, move |result| {
        if let Some(sender) = sender.lock().unwrap_or_else(|e| e.into_inner()).take() {
            let _ = sender
                .send(serde_json::from_str::<String>(&result).is_ok_and(|value| value == nonce));
        }
    }) {
        Ok(()) => {
            tauri::async_runtime::spawn(async move {
                let applied = tokio::time::timeout(std::time::Duration::from_secs(5), receiver)
                    .await
                    .ok()
                    .and_then(Result::ok)
                    .unwrap_or(false);
                done(applied);
            });
            ChromePresentationDispatch::Scheduled
        }
        Err(_) => ChromePresentationDispatch::Rejected,
    }
}

fn emit_ui_command(app: &tauri::AppHandle, id: &str) {
    emit_to_privileged(app, MAIN_LABEL, EVENT_UI, &id);
    // The launcher follows appearance, motion and language only; anything else would
    // wake its hidden WebView for a command it ignores.
    if id.starts_with("theme.")
        || id.starts_with("preference.ui.reduce-motion=")
        || id.starts_with("preference.ui.language=")
    {
        emit_to_privileged(app, overlay::PANEL_LABEL, EVENT_UI, &id);
    }
}

/// Development builds name each privileged WebView's content process, so its
/// memory and CPU can be measured on its own instead of guessed from a list
/// of identical WebKit helpers.
#[cfg(all(debug_assertions, target_os = "macos"))]
fn log_webview_processes(main: &WebviewWindow, panel: &WebviewWindow) {
    let windows = [main.clone(), panel.clone()];
    // A content process is only assigned once navigation has started.
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(4)).await;
        for window in windows {
            log_webview_process(&window);
        }
    });
}

#[cfg(all(debug_assertions, target_os = "macos"))]
fn log_webview_process(window: &WebviewWindow) {
    {
        let label = window.label().to_owned();
        let _ = window.with_webview(move |webview| {
            // SAFETY: a main-thread callback with Tauri's live WKWebView; the
            // selector is WebKit's own, read without taking ownership.
            let pid: i32 = unsafe {
                objc2::msg_send![
                    &*webview.inner().cast::<objc2::runtime::AnyObject>(),
                    _webProcessIdentifier
                ]
            };
            write_diagnostic(format_args!("webview: {label} content process {pid}"));
        });
    }
}

fn shutdown_started(app: &tauri::AppHandle) -> bool {
    app.try_state::<ShutdownCoordinator>()
        .is_some_and(|state| state.terminal_started())
}

const MAIN_LABEL: &str = "main";
#[cfg(any(target_os = "windows", test))]
const PRIVILEGED_MAIN_ENVIRONMENT: u8 = 1 << 0;
#[cfg(any(target_os = "windows", test))]
const PRIVILEGED_PANEL_ENVIRONMENT: u8 = 1 << 1;
const MAX_ITEM_ID_BYTES: usize = 64;
const MAX_NAVIGATION_INPUT_BYTES: usize = 8 * 1024;
// Search terms may expand to three bytes per input byte when percent-encoded;
// keep the resulting launcher action below the navigation ceiling as well.
const MAX_LAUNCHER_QUERY_BYTES: usize = 2 * 1024;
const MAX_COMMAND_ID_BYTES: usize = 128;
const MAX_KEPT_SITE_ID_BYTES: usize = 32;
const MAX_PROFILE_NAME_BYTES: usize = 256;
const MAX_WINDOW_COORDINATE: f64 = 1_000_000.0;
use zephium_core::layout::{MAX_SIDEBAR_WIDTH, MIN_SIDEBAR_WIDTH};

/// The tab a native context menu was opened for.
///
/// Popup menus are modal on every platform, so exactly one target can be
/// pending at a time. The id is parsed and bounded at the IPC boundary and is
/// revalidated by the shell actor, which ignores unknown items.
#[derive(Default)]
struct TabMenuTarget(std::sync::Mutex<Option<ItemId>>);

impl TabMenuTarget {
    fn arm(&self, id: ItemId) -> bool {
        match self.0.lock() {
            Ok(mut slot) => {
                *slot = Some(id);
                true
            }
            Err(_) => false,
        }
    }

    fn take(&self) -> Option<ItemId> {
        self.0.lock().ok().and_then(|mut slot| slot.take())
    }
}

#[cfg(any(target_os = "windows", test))]
fn expected_privileged_environment_labels(bits: u8) -> Vec<&'static str> {
    let mut labels = Vec::with_capacity(2);
    if bits & PRIVILEGED_MAIN_ENVIRONMENT != 0 {
        labels.push(MAIN_LABEL);
    }
    if bits & PRIVILEGED_PANEL_ENVIRONMENT != 0 {
        labels.push(overlay::PANEL_LABEL);
    }
    labels
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CallerPolicy {
    Main,
    Panel,
    Both,
}

// `WebviewWindow` is injected from Tauri's invoke message (and omitted by
// Specta), so the label cannot be forged as a serialized command argument.
fn caller_allowed(policy: CallerPolicy, label: &str) -> bool {
    match policy {
        CallerPolicy::Main => label == MAIN_LABEL,
        CallerPolicy::Panel => label == overlay::PANEL_LABEL,
        CallerPolicy::Both => matches!(label, MAIN_LABEL | overlay::PANEL_LABEL),
    }
}

fn authorize(caller: &WebviewWindow, policy: CallerPolicy, command: &str) -> bool {
    let allowed = caller_allowed(policy, caller.label());
    let report = !allowed
        && AUTH_DENIAL_LOGS_REMAINING
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |remaining| {
                remaining.checked_sub(1)
            })
            .is_ok();
    if report {
        diagnostic!(
            "security: blocked {command} from privileged webview {}",
            caller.label()
        );
    }
    allowed
}

fn bounded(value: &str, max_bytes: usize) -> bool {
    value.len() <= max_bytes
}

fn valid_operation_id(operation_id: &str) -> bool {
    operation_id.len() == 16
        && operation_id
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn point_in_bounds(x: f64, y: f64) -> bool {
    x.is_finite()
        && y.is_finite()
        && x.abs() <= MAX_WINDOW_COORDINATE
        && y.abs() <= MAX_WINDOW_COORDINATE
}

fn window_point(x: f64, y: f64) -> Option<(f64, f64)> {
    if !point_in_bounds(x, y) {
        return None;
    }
    let point = platform::imp::to_window(x, y);
    point_in_bounds(point.0, point.1).then_some(point)
}

fn fixed_nonzero_hex(value: &str) -> Option<u64> {
    if value.len() != 16
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return None;
    }
    let parsed = u64::from_str_radix(value, 16).ok()?;
    (parsed != 0).then_some(parsed)
}

fn extension_popup_anchor_in_bounds(
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    window_width: f64,
    window_height: f64,
) -> Option<ExtensionPopupAnchor> {
    let values = [x, y, width, height, window_width, window_height];
    if values.iter().any(|value| !value.is_finite())
        || x < 0.0
        || y < 0.0
        || width <= 0.0
        || height <= 0.0
        || window_width <= 0.0
        || window_height <= 0.0
        || window_width > MAX_WINDOW_COORDINATE
        || window_height > MAX_WINDOW_COORDINATE
    {
        return None;
    }
    let right = x + width;
    let bottom = y + height;
    if !right.is_finite() || !bottom.is_finite() || right > window_width || bottom > window_height {
        return None;
    }
    ExtensionPopupAnchor::new(Rect::new(x, y, width, height)).ok()
}

fn sidebar_width_in_bounds(width: f64) -> bool {
    width.is_finite() && (MIN_SIDEBAR_WIDTH..=MAX_SIDEBAR_WIDTH).contains(&width)
}

fn sidebar_resize_revision(revision: f64) -> Option<u64> {
    (revision.is_finite()
        && revision.fract() == 0.0
        && (1.0..=9_007_199_254_740_991.0).contains(&revision))
    .then_some(revision as u64)
}

fn setting_value_allowed(key: &str, value: &str) -> bool {
    zephium_core::preferences::value_allowed(key, value)
}

fn search_action_in_bounds(action: &zephium_ipc::SearchAction) -> bool {
    use zephium_ipc::SearchAction;

    match action {
        SearchAction::OpenNote { id } => zephium_core::resources::valid_id(id),
        SearchAction::ActivateTab { id } => bounded(id, MAX_ITEM_ID_BYTES),
        SearchAction::OpenUrl { url } => bounded(url, MAX_NAVIGATION_INPUT_BYTES),
        // The launcher may only run registry commands. Context-menu actions
        // resolve against an armed target and are reachable from main chrome
        // alone; the panel must never be able to replay one.
        SearchAction::RunCommand { id } => {
            bounded(id, MAX_COMMAND_ID_BYTES) && zephium_core::commands::get(id).is_some()
        }
    }
}

// Ids arrive as ULID strings from a semi-trusted webview; anything that does
// not parse is dropped here, before it reaches the shell.
fn rejected_operation() -> zephium_ipc::OperationAdmission {
    zephium_ipc::OperationAdmission {
        operation_id: None,
        accepted: false,
    }
}

fn finish_operation_admission(
    ledger: &OperationLedger,
    operation_id: String,
    accepted: bool,
) -> zephium_ipc::OperationAdmission {
    if accepted {
        zephium_ipc::OperationAdmission {
            operation_id: Some(operation_id),
            accepted: true,
        }
    } else {
        ledger.cancel_reservation(&operation_id);
        // Never expose an identity that reconciliation has intentionally
        // removed. A rejected FIFO admission is indistinguishable from the
        // other pre-admission failures at this API boundary.
        rejected_operation()
    }
}

fn dispatch_operation(
    app: &tauri::AppHandle,
    shell: &Handle,
    command: Command,
) -> zephium_ipc::OperationAdmission {
    let Ok(sequence) = NEXT_OPERATION_ID
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1))
    else {
        return rejected_operation();
    };
    let operation_id = format!("{sequence:016x}");
    let Some(ledger) = app.try_state::<OperationLedger>() else {
        return rejected_operation();
    };
    if !ledger.reserve(&operation_id) {
        return rejected_operation();
    }
    let accepted = shell.dispatch_operation(operation_id.clone(), command);
    finish_operation_admission(&ledger, operation_id, accepted)
}

fn dispatch_with_id(
    app: &tauri::AppHandle,
    shell: &Handle,
    id: &str,
    cmd: impl FnOnce(ItemId) -> Command,
) -> zephium_ipc::OperationAdmission {
    if !bounded(id, MAX_ITEM_ID_BYTES) {
        return rejected_operation();
    }
    if let Some(id) = ItemId::parse(id) {
        return dispatch_operation(app, shell, cmd(id));
    }
    rejected_operation()
}

#[tauri::command]
#[specta::specta]
fn tabs_bootstrap(caller: WebviewWindow, shell: State<'_, Handle>) {
    if !authorize(&caller, CallerPolicy::Main, "tabs_bootstrap") {
        return;
    }
    shell.dispatch(Command::Bootstrap);
}

static BLOCKER_STATS_QUERY_IN_FLIGHT: AtomicBool = AtomicBool::new(false);

#[tauri::command]
#[specta::specta]
async fn blocker_stats(
    caller: WebviewWindow,
    shell: State<'_, Handle>,
    profile: String,
) -> Result<zephium_ipc::BlockerStatsView, ()> {
    if !authorize(&caller, CallerPolicy::Main, "blocker_stats") {
        return Err(());
    }
    let Some(profile) = zephium_core::ids::ProfileId::parse(&profile) else {
        return Err(());
    };
    if BLOCKER_STATS_QUERY_IN_FLIGHT
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return Err(());
    }
    let _guard = AtomicFlagReset(&BLOCKER_STATS_QUERY_IN_FLIGHT);
    let request = shell.blocker_statistics(profile);
    tauri::async_runtime::spawn_blocking(move || {
        request
            .recv_timeout(std::time::Duration::from_secs(2))
            .ok()
            .flatten()
    })
    .await
    .ok()
    .flatten()
    .ok_or(())
}

#[tauri::command]
#[specta::specta]
async fn blocker_status(
    caller: WebviewWindow,
    shell: State<'_, Handle>,
) -> Result<zephium_ipc::BlockerStatusView, ()> {
    if !authorize(&caller, CallerPolicy::Main, "blocker_status") {
        return Ok(zephium_ipc::BlockerStatusView::unavailable());
    }
    if BLOCKER_STATUS_QUERY_IN_FLIGHT
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return Ok(zephium_ipc::BlockerStatusView::unavailable());
    }
    let _query_guard = AtomicFlagReset(&BLOCKER_STATUS_QUERY_IN_FLIGHT);
    let request = shell.focused_content_policy_status();
    let status = tauri::async_runtime::spawn_blocking(move || {
        request.recv_timeout(BLOCKER_STATUS_QUERY_TIMEOUT)
    })
    .await
    .unwrap_or_else(|_| zephium_ipc::BlockerStatusView::unavailable());
    Ok(status)
}

#[tauri::command]
#[specta::specta]
fn blocker_set_enabled(
    caller: WebviewWindow,
    shell: State<'_, Handle>,
    enabled: bool,
) -> zephium_ipc::OperationAdmission {
    if !authorize(&caller, CallerPolicy::Main, "blocker_set_enabled") {
        return rejected_operation();
    }
    dispatch_operation(
        caller.app_handle(),
        &shell,
        Command::SetFocusedContentBlockerEnabled(enabled),
    )
}

static BLOCKER_PICKER_QUERY_IN_FLIGHT: AtomicBool = AtomicBool::new(false);

#[tauri::command]
#[specta::specta]
async fn blocker_picker(
    caller: WebviewWindow,
    shell: State<'_, Handle>,
    context: zephium_ipc::BlockerSiteContext,
    action: zephium_ipc::BlockerPickerAction,
) -> Result<Option<zephium_ipc::BlockerPickerView>, ()> {
    if !authorize(&caller, CallerPolicy::Main, "blocker_picker")
        || context.profile.len() > 64
        || context.tab.len() > 64
        || context.site.len() > 253
        || context.revision.len() != 16
        || match &action {
            zephium_ipc::BlockerPickerAction::Start => false,
            zephium_ipc::BlockerPickerAction::Read { session }
            | zephium_ipc::BlockerPickerAction::Preview { session, .. }
            | zephium_ipc::BlockerPickerAction::Stop { session } => session.len() != 16,
        }
        || BLOCKER_PICKER_QUERY_IN_FLIGHT
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
    {
        return Ok(None);
    }
    let _guard = AtomicFlagReset(&BLOCKER_PICKER_QUERY_IN_FLIGHT);
    let request = shell.element_picker(context, action);
    Ok(tauri::async_runtime::spawn_blocking(move || {
        request
            .recv_timeout(std::time::Duration::from_secs(3))
            .ok()
            .flatten()
    })
    .await
    .ok()
    .flatten())
}

#[tauri::command]
#[specta::specta]
fn blocker_site_change(
    caller: WebviewWindow,
    shell: State<'_, Handle>,
    context: zephium_ipc::BlockerSiteContext,
    action: zephium_ipc::BlockerSiteAction,
) -> zephium_ipc::OperationAdmission {
    if !authorize(&caller, CallerPolicy::Main, "blocker_site_change")
        || context.profile.len() > 64
        || context.tab.len() > 64
        || context.site.len() > 253
        || context.revision.len() != 16
        || match &action {
            zephium_ipc::BlockerSiteAction::SaveSelection { session, selection } => {
                session.len() != 16 || selection.len() != 64
            }
            zephium_ipc::BlockerSiteAction::Pause { .. }
            | zephium_ipc::BlockerSiteAction::Retry => false,
            zephium_ipc::BlockerSiteAction::SetHideEnabled { id, .. }
            | zephium_ipc::BlockerSiteAction::RemoveHide { id } => id.len() != 16,
        }
    {
        return rejected_operation();
    }
    dispatch_operation(
        caller.app_handle(),
        &shell,
        Command::ChangeBlockerSite {
            context: Box::new(context),
            action,
        },
    )
}

#[tauri::command]
#[specta::specta]
fn blocker_retry(
    caller: WebviewWindow,
    shell: State<'_, Handle>,
    failed_generation: String,
) -> zephium_ipc::OperationAdmission {
    if !authorize(&caller, CallerPolicy::Main, "blocker_retry")
        || failed_generation.len() != 16
        || !failed_generation
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return rejected_operation();
    }
    let Some(failed_generation) = u64::from_str_radix(&failed_generation, 16)
        .ok()
        .and_then(zephium_core::blocker::ContentPolicyGeneration::new)
    else {
        return rejected_operation();
    };
    dispatch_operation(
        caller.app_handle(),
        &shell,
        Command::RetryFocusedContentPolicy { failed_generation },
    )
}

#[tauri::command]
#[specta::specta]
fn blocker_refresh_sources(
    caller: WebviewWindow,
    shell: State<'_, Handle>,
) -> zephium_ipc::OperationAdmission {
    if !authorize(&caller, CallerPolicy::Main, "blocker_refresh_sources") {
        return rejected_operation();
    }
    dispatch_operation(
        caller.app_handle(),
        &shell,
        Command::RefreshContentBlockerSources,
    )
}

#[tauri::command]
#[specta::specta]
fn tabs_open(caller: WebviewWindow, shell: State<'_, Handle>) -> zephium_ipc::OperationAdmission {
    if !authorize(&caller, CallerPolicy::Main, "tabs_open") {
        return rejected_operation();
    }
    dispatch_operation(caller.app_handle(), &shell, Command::Open)
}

#[tauri::command]
#[specta::specta]
fn tabs_activate(
    caller: WebviewWindow,
    shell: State<'_, Handle>,
    id: String,
) -> zephium_ipc::OperationAdmission {
    if !authorize(&caller, CallerPolicy::Main, "tabs_activate") {
        return rejected_operation();
    }
    dispatch_with_id(caller.app_handle(), &shell, &id, Command::Activate)
}

#[tauri::command]
#[specta::specta]
fn tabs_close(
    caller: WebviewWindow,
    shell: State<'_, Handle>,
    id: String,
) -> zephium_ipc::OperationAdmission {
    if !authorize(&caller, CallerPolicy::Main, "tabs_close") {
        return rejected_operation();
    }
    dispatch_with_id(caller.app_handle(), &shell, &id, Command::Close)
}

#[tauri::command]
#[specta::specta]
fn tabs_set_essential(
    caller: WebviewWindow,
    shell: State<'_, Handle>,
    id: String,
    essential: bool,
    before: Option<String>,
) -> zephium_ipc::OperationAdmission {
    if !authorize(&caller, CallerPolicy::Main, "tabs_set_essential")
        || !bounded(&id, MAX_ITEM_ID_BYTES)
    {
        return rejected_operation();
    }
    let Some(id) = ItemId::parse(&id) else {
        return rejected_operation();
    };
    let before = match before {
        Some(value) if bounded(&value, MAX_ITEM_ID_BYTES) => match ItemId::parse(&value) {
            Some(id) => Some(id),
            None => return rejected_operation(),
        },
        Some(_) => return rejected_operation(),
        None => None,
    };
    dispatch_operation(
        caller.app_handle(),
        &shell,
        Command::SetTabEssential {
            id,
            essential,
            before,
        },
    )
}

/// Onboarding keeps a site by its catalog id; native owns the address and
/// the mark, so chrome can never pin an arbitrary URL through this path.
#[tauri::command]
#[specta::specta]
fn essentials_keep(
    caller: WebviewWindow,
    shell: State<'_, Handle>,
    site: String,
) -> zephium_ipc::OperationAdmission {
    if !authorize_onboarding(&caller, "essentials_keep") || !bounded(&site, MAX_KEPT_SITE_ID_BYTES)
    {
        return rejected_operation();
    }
    dispatch_operation(caller.app_handle(), &shell, Command::KeepSite(site))
}

#[tauri::command]
#[specta::specta]
fn profile_rename(
    caller: WebviewWindow,
    shell: State<'_, Handle>,
    name: String,
) -> zephium_ipc::OperationAdmission {
    if !authorize_onboarding(&caller, "profile_rename") || !bounded(&name, MAX_PROFILE_NAME_BYTES) {
        return rejected_operation();
    }
    dispatch_operation(
        caller.app_handle(),
        &shell,
        Command::RenameFocusedProfile(name),
    )
}

#[tauri::command]
#[specta::specta]
fn tabs_navigate(
    caller: WebviewWindow,
    shell: State<'_, Handle>,
    id: String,
    input: String,
) -> zephium_ipc::OperationAdmission {
    if !authorize(&caller, CallerPolicy::Main, "tabs_navigate")
        || !bounded(&input, MAX_NAVIGATION_INPUT_BYTES)
    {
        return rejected_operation();
    }
    dispatch_with_id(caller.app_handle(), &shell, &id, |id| Command::Navigate {
        id,
        input,
    })
}

/// Shows the folder with Zephium's local log and crash report.
#[tauri::command]
#[specta::specta]
fn diagnostics_show_logs(caller: WebviewWindow) -> bool {
    authorize(&caller, CallerPolicy::Main, "diagnostics_show_logs") && diagnostics::reveal()
}

#[tauri::command]
#[specta::specta]
fn tabs_answer_page_request(
    caller: WebviewWindow,
    shell: State<'_, Handle>,
    id: String,
    answer: zephium_ipc::PageRequestAnswer,
) -> zephium_ipc::OperationAdmission {
    if !authorize(&caller, CallerPolicy::Main, "tabs_answer_page_request") {
        return rejected_operation();
    }
    let decision = match answer {
        zephium_ipc::PageRequestAnswer::Allow => zephium_app::PageRequestDecision::Allow,
        zephium_ipc::PageRequestAnswer::AlwaysAllow => {
            zephium_app::PageRequestDecision::AlwaysAllow
        }
        zephium_ipc::PageRequestAnswer::Dismiss => zephium_app::PageRequestDecision::Dismiss,
    };
    dispatch_with_id(caller.app_handle(), &shell, &id, |id| {
        Command::AnswerPageRequest { id, decision }
    })
}

#[tauri::command]
#[specta::specta]
fn tabs_reload(
    caller: WebviewWindow,
    shell: State<'_, Handle>,
    id: String,
) -> zephium_ipc::OperationAdmission {
    if !authorize(&caller, CallerPolicy::Main, "tabs_reload") {
        return rejected_operation();
    }
    dispatch_with_id(caller.app_handle(), &shell, &id, Command::Reload)
}

#[tauri::command]
#[specta::specta]
fn tabs_back(
    caller: WebviewWindow,
    shell: State<'_, Handle>,
    id: String,
) -> zephium_ipc::OperationAdmission {
    if !authorize(&caller, CallerPolicy::Main, "tabs_back") {
        return rejected_operation();
    }
    dispatch_with_id(caller.app_handle(), &shell, &id, Command::GoBack)
}

#[tauri::command]
#[specta::specta]
fn tabs_forward(
    caller: WebviewWindow,
    shell: State<'_, Handle>,
    id: String,
) -> zephium_ipc::OperationAdmission {
    if !authorize(&caller, CallerPolicy::Main, "tabs_forward") {
        return rejected_operation();
    }
    dispatch_with_id(caller.app_handle(), &shell, &id, Command::GoForward)
}

fn work_pane_rect(rect: zephium_ipc::WorkPaneRect) -> Option<zephium_core::geometry::Rect> {
    (point_in_bounds(rect.x, rect.y)
        && rect.width.is_finite()
        && rect.height.is_finite()
        && rect.width > 0.0
        && rect.height > 0.0
        && rect.width <= MAX_WINDOW_COORDINATE
        && rect.height <= MAX_WINDOW_COORDINATE)
        .then(|| zephium_core::geometry::Rect::new(rect.x, rect.y, rect.width, rect.height))
}

fn trusted_web_url(url: &str) -> bool {
    url.len() <= 8192
        && !url.chars().any(char::is_control)
        && tauri::Url::parse(url).is_ok_and(|parsed| {
            matches!(parsed.scheme(), "https" | "http")
                && parsed.host_str().is_some()
                && parsed.username().is_empty()
                && parsed.password().is_none()
        })
}

/// Shows the transient Work browser pane over a Space tab or a fresh tab at
/// an explicit trusted URL. The rect is the chrome's measured hole.
#[tauri::command]
#[specta::specta]
fn work_pane_show(
    caller: WebviewWindow,
    shell: State<'_, Handle>,
    target: zephium_ipc::WorkPaneTarget,
    rect: zephium_ipc::WorkPaneRect,
) -> zephium_ipc::OperationAdmission {
    if !authorize(&caller, CallerPolicy::Main, "work_pane_show") {
        return rejected_operation();
    }
    let Some(rect) = work_pane_rect(rect) else {
        return rejected_operation();
    };
    let target = match target {
        zephium_ipc::WorkPaneTarget::Tab { id } => {
            if !bounded(&id, MAX_ITEM_ID_BYTES) {
                return rejected_operation();
            }
            match ItemId::parse(&id) {
                Some(id) => zephium_app::WorkPaneTarget::Tab(id),
                None => return rejected_operation(),
            }
        }
        zephium_ipc::WorkPaneTarget::Url { url } => {
            if !trusted_web_url(&url) {
                return rejected_operation();
            }
            zephium_app::WorkPaneTarget::Url(url)
        }
    };
    dispatch_operation(
        caller.app_handle(),
        &shell,
        Command::WorkPaneShow { target, rect },
    )
}

#[tauri::command]
#[specta::specta]
fn work_pane_set_rect(
    caller: WebviewWindow,
    shell: State<'_, Handle>,
    rect: zephium_ipc::WorkPaneRect,
    generation: u32,
) {
    if !authorize(&caller, CallerPolicy::Main, "work_pane_set_rect") {
        return;
    }
    let Some(rect) = work_pane_rect(rect) else {
        return;
    };
    shell.dispatch(Command::WorkPaneSetRect { rect, generation });
}

#[tauri::command]
#[specta::specta]
fn work_pane_hide(
    caller: WebviewWindow,
    shell: State<'_, Handle>,
) -> zephium_ipc::OperationAdmission {
    if !authorize(&caller, CallerPolicy::Main, "work_pane_hide") {
        return rejected_operation();
    }
    work_product::release_human_presentations(caller.app_handle());
    dispatch_operation(caller.app_handle(), &shell, Command::WorkPaneHide)
}

#[tauri::command]
#[specta::specta]
fn tabs_split(
    caller: WebviewWindow,
    shell: State<'_, Handle>,
    other: String,
) -> zephium_ipc::OperationAdmission {
    if !authorize(&caller, CallerPolicy::Main, "tabs_split") {
        return rejected_operation();
    }
    dispatch_with_id(caller.app_handle(), &shell, &other, |other| {
        Command::SplitWith {
            other,
            axis: Axis::Row,
        }
    })
}

#[tauri::command]
#[specta::specta]
fn tabs_unsplit(
    caller: WebviewWindow,
    shell: State<'_, Handle>,
) -> zephium_ipc::OperationAdmission {
    if !authorize(&caller, CallerPolicy::Main, "tabs_unsplit") {
        return rejected_operation();
    }
    dispatch_operation(caller.app_handle(), &shell, Command::Unsplit)
}

#[tauri::command]
#[specta::specta]
fn tabs_leave_split(
    caller: WebviewWindow,
    shell: State<'_, Handle>,
    id: String,
) -> zephium_ipc::OperationAdmission {
    if !authorize(&caller, CallerPolicy::Main, "tabs_leave_split")
        || !bounded(&id, MAX_ITEM_ID_BYTES)
    {
        return rejected_operation();
    }
    let Some(id) = ItemId::parse(&id) else {
        return rejected_operation();
    };
    dispatch_operation(caller.app_handle(), &shell, Command::LeaveSplit(id))
}

#[tauri::command]
#[specta::specta]
#[allow(clippy::too_many_arguments)]
fn extension_action_invoke(
    caller: WebviewWindow,
    shell: State<'_, Handle>,
    profile_id: String,
    install_id: String,
    runtime_generation: String,
    action_revision: String,
    anchor_x: f64,
    anchor_y: f64,
    anchor_width: f64,
    anchor_height: f64,
) -> zephium_ipc::OperationAdmission {
    if !authorize(&caller, CallerPolicy::Main, "extension_action_invoke")
        || shutdown_started(caller.app_handle())
        || !bounded(&profile_id, MAX_ITEM_ID_BYTES)
        || !bounded(&install_id, MAX_ITEM_ID_BYTES)
    {
        return rejected_extension_action("caller-or-identity-boundary");
    }
    let Some(profile) =
        ProfileId::parse(&profile_id).filter(|profile| profile.to_string() == profile_id)
    else {
        return rejected_extension_action("profile-identity");
    };
    let Some(install) =
        ExtensionInstallId::parse(&install_id).filter(|install| install.to_string() == install_id)
    else {
        return rejected_extension_action("install-identity");
    };
    let Some(generation) =
        fixed_nonzero_hex(&runtime_generation).and_then(ExtensionRuntimeGeneration::new)
    else {
        return rejected_extension_action("runtime-generation");
    };
    let Some(revision) = fixed_nonzero_hex(&action_revision).and_then(ExtensionActionRevision::new)
    else {
        return rejected_extension_action("action-revision");
    };
    // On macOS the chrome webview shrinks to the sidebar beside a web page, so
    // its own size is not the window's; the anchor must fit the window.
    let window = platform::imp::content_size(&caller).unwrap_or_else(|| inner_logical(&caller));
    // DOMRect is relative to the positioned privileged chrome WebView, while
    // the native popup parent is the window content view. Apply the same
    // generation-checked chrome origin used by drag/menu coordinates; never
    // let a negative CSS coordinate become valid merely because of the inset.
    if anchor_x < 0.0 || anchor_y < 0.0 {
        return rejected_extension_action("negative-anchor");
    }
    let Some((window_anchor_x, window_anchor_y)) = window_point(anchor_x, anchor_y) else {
        return rejected_extension_action("chrome-origin");
    };
    let Some(anchor) = extension_popup_anchor_in_bounds(
        window_anchor_x,
        window_anchor_y,
        anchor_width,
        anchor_height,
        window.width,
        window.height,
    ) else {
        return rejected_extension_action("window-anchor-bounds");
    };
    let admission = dispatch_operation(
        caller.app_handle(),
        &shell,
        Command::InvokeExtensionAction {
            runtime: ExtensionRuntimeInstance::new(profile, install, generation),
            revision,
            anchor,
        },
    );
    if !admission.accepted {
        diagnostic!("extensions: toolbar action IPC rejected at actor-queue-admission");
    }
    admission
}

fn rejected_extension_action(stage: &'static str) -> zephium_ipc::OperationAdmission {
    diagnostic!("extensions: toolbar action IPC rejected at {stage}");
    rejected_operation()
}

#[derive(Clone, Copy, Debug, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
enum PagePermissionPromptDecisionInput {
    AllowOnce,
    AlwaysAllow,
    DenyOnce,
    AlwaysDeny,
}

/// Answers only the exact Shell-projected foreground page request. Origin and
/// capability names are intentionally absent: chrome can choose a disposition
/// but cannot mint or alter authority.
#[tauri::command]
#[specta::specta]
fn page_permission_respond(
    caller: WebviewWindow,
    shell: State<'_, Handle>,
    profile_id: String,
    item_id: String,
    request_id: String,
    decision: PagePermissionPromptDecisionInput,
) -> zephium_ipc::OperationAdmission {
    if !authorize(&caller, CallerPolicy::Main, "page_permission_respond")
        || shutdown_started(caller.app_handle())
        || !bounded(&profile_id, MAX_ITEM_ID_BYTES)
        || !bounded(&item_id, MAX_ITEM_ID_BYTES)
    {
        return rejected_operation();
    }
    let Some(profile) =
        ProfileId::parse(&profile_id).filter(|profile| profile.to_string() == profile_id)
    else {
        return rejected_operation();
    };
    let Some(item) = ItemId::parse(&item_id).filter(|item| item.to_string() == item_id) else {
        return rejected_operation();
    };
    let Some(request) = fixed_nonzero_hex(&request_id)
        .and_then(zephium_core::permissions::PagePermissionRequestId::new)
    else {
        return rejected_operation();
    };
    let decision = match decision {
        PagePermissionPromptDecisionInput::AllowOnce => PagePermissionPromptDecision::AllowOnce,
        PagePermissionPromptDecisionInput::AlwaysAllow => PagePermissionPromptDecision::AlwaysAllow,
        PagePermissionPromptDecisionInput::DenyOnce => PagePermissionPromptDecision::DenyOnce,
        PagePermissionPromptDecisionInput::AlwaysDeny => PagePermissionPromptDecision::AlwaysDeny,
    };
    dispatch_operation(
        caller.app_handle(),
        &shell,
        Command::RespondToPagePermissionPrompt {
            profile,
            item,
            request,
            decision,
        },
    )
}

#[tauri::command]
#[specta::specta]
fn capture_stop(
    caller: WebviewWindow,
    shell: State<'_, Handle>,
    item_id: String,
    navigation_id: String,
) -> zephium_ipc::OperationAdmission {
    if !authorize(&caller, CallerPolicy::Main, "capture_stop")
        || shutdown_started(caller.app_handle())
        || !bounded(&item_id, MAX_ITEM_ID_BYTES)
    {
        return rejected_operation();
    }
    let Some(item) = ItemId::parse(&item_id).filter(|item| item.to_string() == item_id) else {
        return rejected_operation();
    };
    let Some(navigation) = fixed_nonzero_hex(&navigation_id)
        .map(zephium_core::ports::engine::NavigationPresentationId::from_raw)
    else {
        return rejected_operation();
    };
    dispatch_operation(
        caller.app_handle(),
        &shell,
        Command::StopMediaCapture { item, navigation },
    )
}

#[tauri::command]
#[specta::specta]
fn profiles_delete(
    caller: WebviewWindow,
    shell: State<'_, Handle>,
    profile: String,
) -> zephium_ipc::OperationAdmission {
    if !authorize(&caller, CallerPolicy::Main, "profiles_delete")
        || !bounded(&profile, MAX_ITEM_ID_BYTES)
    {
        return rejected_operation();
    }
    let Some(parsed) = ProfileId::parse(&profile).filter(|parsed| parsed.to_string() == profile)
    else {
        return rejected_operation();
    };
    dispatch_operation(caller.app_handle(), &shell, Command::DeleteProfile(parsed))
}

#[tauri::command]
#[specta::specta]
fn operation_status(
    caller: WebviewWindow,
    app: tauri::AppHandle,
    operation_id: String,
) -> zephium_ipc::OperationStatus {
    if !authorize(&caller, CallerPolicy::Main, "operation_status")
        || !valid_operation_id(&operation_id)
    {
        return zephium_ipc::OperationStatus::Unknown;
    }
    app.try_state::<OperationLedger>()
        .map_or(zephium_ipc::OperationStatus::Unknown, |ledger| {
            ledger.status(&operation_id)
        })
}

#[tauri::command]
#[specta::specta]
fn operations_reconcile(
    caller: WebviewWindow,
    app: tauri::AppHandle,
) -> Vec<zephium_ipc::OperationDisposition> {
    if !authorize(&caller, CallerPolicy::Main, "operations_reconcile") {
        return Vec::new();
    }
    app.try_state::<OperationLedger>()
        .map(|ledger| ledger.processed())
        .unwrap_or_default()
}

#[tauri::command]
#[specta::specta]
fn operation_acknowledge(
    caller: WebviewWindow,
    app: tauri::AppHandle,
    operation_id: String,
) -> bool {
    if !authorize(&caller, CallerPolicy::Main, "operation_acknowledge")
        || !valid_operation_id(&operation_id)
    {
        return false;
    }
    app.try_state::<OperationLedger>()
        .is_some_and(|ledger| ledger.acknowledge(&operation_id))
}

const SETTING_KEYS: &[&str] = zephium_core::preferences::KEYS;

/// The person's overrides, for menus built on demand.
fn keymap_overrides(app: &tauri::AppHandle) -> std::collections::HashMap<String, String> {
    app.try_state::<keymap::Keymap>()
        .map(|keymap| keymap.overrides())
        .unwrap_or_default()
}

fn appearance_code(mode: &str) -> u8 {
    match mode {
        "light" => APPEARANCE_LIGHT,
        "dark" => APPEARANCE_DARK,
        _ => APPEARANCE_SYSTEM,
    }
}

#[cfg(target_os = "windows")]
fn resolved_native_theme(window: &WebviewWindow, mode: &str) -> tauri::Theme {
    match appearance_code(mode) {
        APPEARANCE_LIGHT => tauri::Theme::Light,
        APPEARANCE_DARK => tauri::Theme::Dark,
        _ => window.theme().unwrap_or(tauri::Theme::Dark),
    }
}

#[cfg(target_os = "windows")]
fn apply_native_materials(app: &tauri::AppHandle, mode: &str) {
    for label in [MAIN_LABEL, overlay::PANEL_LABEL] {
        if let Some(window) = app.get_webview_window(label) {
            let dark = matches!(resolved_native_theme(&window, mode), tauri::Theme::Dark);
            platform::imp::apply_material(&window, dark);
        }
    }
}

fn apply_native_theme(app: &tauri::AppHandle, mode: &str) {
    let appearance = appearance_code(mode);
    // Publish the mode before set_theme: a synchronous ThemeChanged callback
    // caused by a forced light/dark value must not be mistaken for a system
    // appearance transition.
    NATIVE_APPEARANCE.store(appearance, Ordering::Release);
    let theme = match appearance {
        APPEARANCE_LIGHT => Some(tauri::Theme::Light),
        APPEARANCE_DARK => Some(tauri::Theme::Dark),
        _ => None,
    };
    // Keeps vibrancy and native controls in step with a forced appearance.
    app.set_theme(theme);
    #[cfg(target_os = "windows")]
    apply_native_materials(app, mode);
}

fn accepted_ui_operation() -> zephium_ipc::OperationAdmission {
    zephium_ipc::OperationAdmission {
        operation_id: None,
        accepted: true,
    }
}

fn execute_command(app: &tauri::AppHandle, id: &str) -> zephium_ipc::OperationAdmission {
    if id == "browser.quit" {
        app.exit(0);
        return accepted_ui_operation();
    }
    if shutdown_started(app) {
        return rejected_operation();
    }
    if matches!(
        id,
        "settings.profiles"
            | "settings.account"
            | "settings.newtab"
            | "settings.ai"
            | "settings.focus"
            | "settings.shortcuts"
    ) {
        let section = id.replacen("settings.", "settings.section.", 1);
        let _ = try_emit_to_privileged(app, MAIN_LABEL, EVENT_UI, &section);
        return execute_command(app, "browser.settings");
    }
    if id == "settings.connections" || id.starts_with("settings.connections.") {
        let section = id.replacen("settings.", "settings.section.", 1);
        let _ = try_emit_to_privileged(app, MAIN_LABEL, EVENT_UI, &section);
        return execute_command(app, "browser.settings");
    }
    if BOOKMARK_MENU_ACTION_IDS.contains(&id) {
        let command = id.replacen("bookmarkmenu.", "bookmark.menu.", 1);
        return if try_emit_to_privileged(app, MAIN_LABEL, EVENT_UI, &command) {
            accepted_ui_operation()
        } else {
            rejected_operation()
        };
    }
    if id == "mode.work" {
        return execute_command(app, "browser.work");
    }
    if id == "mode.browse" {
        return execute_command(app, "browser.return");
    }
    if id == "mode.choose" {
        use tauri::menu::{Menu, MenuItemBuilder};
        let menu = (|| -> tauri::Result<_> {
            let browse = MenuItemBuilder::with_id("mode.browse", "Browse").build(app)?;
            let work = MenuItemBuilder::with_id("mode.work", "Work").build(app)?;
            Menu::with_items(app, &[&browse, &work])
        })();
        return match (app.get_webview_window(MAIN_LABEL), menu) {
            (Some(window), Ok(menu)) if window.popup_menu(&menu).is_ok() => accepted_ui_operation(),
            _ => rejected_operation(),
        };
    }
    // Site protection acts on the frame's focused page state.
    if matches!(id, PROTECTION_SITE_COMMAND | PROTECTION_HIDE_COMMAND) {
        return if try_emit_to_privileged(app, MAIN_LABEL, EVENT_UI, &id) {
            accepted_ui_operation()
        } else {
            rejected_operation()
        };
    }
    // Capture belongs to the frame, which decides where the new note opens;
    // the clipboard write stays in privileged chrome, which holds the
    // authoritative URL of the page it shows.
    if matches!(
        id,
        "note.new" | "page.copyLink" | "find.show" | "find.next" | "find.previous"
    ) {
        return if try_emit_to_privileged(app, MAIN_LABEL, EVENT_UI, &id) {
            accepted_ui_operation()
        } else {
            rejected_operation()
        };
    }
    if matches!(
        id,
        "tool.notes"
            | "tool.tasks"
            | "tool.ai"
            | "tool.time"
            | "tool.history"
            | "tool.downloads"
            | "tool.bookmarks"
            | "extensions.manage"
    ) {
        return if try_emit_to_privileged(app, MAIN_LABEL, EVENT_UI, &id) {
            accepted_ui_operation()
        } else {
            rejected_operation()
        };
    }
    if let Some(destination) = id.strip_prefix("browser.") {
        let page = match destination {
            "work" => Some(zephium_app::BrowserPage::Work),
            "settings" => Some(zephium_app::BrowserPage::Settings),
            "extensions" => Some(zephium_app::BrowserPage::Extensions),
            "history" => Some(zephium_app::BrowserPage::History),
            "downloads" => Some(zephium_app::BrowserPage::Downloads),
            "tasks" => Some(zephium_app::BrowserPage::Tasks),
            "notes" => Some(zephium_app::BrowserPage::Notes),
            "time" => Some(zephium_app::BrowserPage::Time),
            "return" => None,
            _ => return rejected_operation(),
        };
        return app
            .try_state::<Handle>()
            .map_or_else(rejected_operation, |shell| {
                dispatch_operation(app, &shell, Command::ShowBrowserPage(page))
            });
    }
    if id == "launcher.toggle" {
        return if overlay::request(app, |panel| panel.toggle()) {
            accepted_ui_operation()
        } else {
            rejected_operation()
        };
    }
    if id == "split.choose" {
        return if try_emit_to_privileged(app, MAIN_LABEL, EVENT_UI, &id) {
            accepted_ui_operation()
        } else {
            rejected_operation()
        };
    }
    if let Some(action) = id.strip_prefix("tabmenu.") {
        return execute_tab_menu_action(app, action);
    }
    // Sidebar presentation is privileged-chrome state persisted through the
    // settings allowlist, not an actor mutation. Deliver the intent and let
    // the frame own it.
    if id == SIDEBAR_COMPACT_COMMAND {
        return if try_emit_to_privileged(app, MAIN_LABEL, EVENT_UI, &id) {
            accepted_ui_operation()
        } else {
            rejected_operation()
        };
    }
    if let Some(mode) = id.strip_prefix("theme.") {
        if matches!(mode, "system" | "light" | "dark") {
            if let Some(shell) = app.try_state::<Handle>() {
                let admission = dispatch_operation(
                    app,
                    &shell,
                    Command::SetAppSetting {
                        key: "appearance".into(),
                        value: mode.into(),
                    },
                );
                return admission;
            }
        }
        return rejected_operation();
    }
    if zephium_core::commands::get(id).is_some() {
        if let Some(shell) = app.try_state::<Handle>() {
            return dispatch_operation(app, &shell, Command::Run(id.to_string()));
        }
    }
    rejected_operation()
}

/// Applies one tab context-menu action to the exact tab the menu was armed
/// for. Taking the target means a duplicated or delayed menu event cannot
/// replay the action against whatever tab happens to be focused later.
fn execute_tab_menu_action(
    app: &tauri::AppHandle,
    action: &str,
) -> zephium_ipc::OperationAdmission {
    let Some(target) = app.try_state::<TabMenuTarget>() else {
        return rejected_operation();
    };
    let Some(id) = target.take() else {
        return rejected_operation();
    };
    if action == "copyLink" {
        // The clipboard write stays in privileged chrome, which already holds
        // the authoritative URL for this tab. Native never handles page text.
        return if try_emit_to_privileged(app, MAIN_LABEL, EVENT_UI, &TAB_MENU_COPY_LINK_COMMAND) {
            accepted_ui_operation()
        } else {
            rejected_operation()
        };
    }
    let Some(shell) = app.try_state::<Handle>() else {
        return rejected_operation();
    };
    let tab_action = |action| Command::TabAction { id, action };
    let keep = |essential| Command::SetTabEssential {
        id,
        essential,
        before: None,
    };
    match action {
        "reload" => dispatch_operation(app, &shell, Command::Reload(id)),
        "close" => dispatch_operation(app, &shell, Command::Close(id)),
        "duplicate" => dispatch_operation(app, &shell, tab_action(TabAction::Duplicate)),
        "bookmark" => dispatch_operation(app, &shell, tab_action(TabAction::Bookmark)),
        "closeOthers" => dispatch_operation(app, &shell, tab_action(TabAction::CloseOthers)),
        "closeBelow" => dispatch_operation(app, &shell, tab_action(TabAction::CloseBelow)),
        "keep" => dispatch_operation(app, &shell, keep(true)),
        "unkeep" => dispatch_operation(app, &shell, keep(false)),
        "split" => dispatch_operation(
            app,
            &shell,
            Command::SplitWith {
                other: id,
                axis: Axis::Row,
            },
        ),
        _ => rejected_operation(),
    }
}

#[tauri::command]
#[specta::specta]
fn run_command(
    caller: WebviewWindow,
    app: tauri::AppHandle,
    id: String,
) -> zephium_ipc::OperationAdmission {
    if !authorize(&caller, CallerPolicy::Main, "run_command") || !bounded(&id, MAX_COMMAND_ID_BYTES)
    {
        return rejected_operation();
    }
    execute_command(&app, &id)
}

/// New Tab has its own main-only entry. The actor revalidates the bound blank
/// tab and focused profile/space before searching or executing any result.
#[tauri::command]
#[specta::specta]
fn newtab_search_context(
    caller: WebviewWindow,
    app: tauri::AppHandle,
    tab_id: String,
) -> Option<zephium_ipc::SearchContext> {
    if !authorize(&caller, CallerPolicy::Main, "newtab_search_context")
        || ItemId::parse(&tab_id).is_none()
    {
        return None;
    }
    static NEXT_SEARCH_SESSION: AtomicU64 = AtomicU64::new(1);
    let session = NEXT_SEARCH_SESSION
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
            value.checked_add(1)
        })
        .ok()?;
    app.try_state::<overlay::ContextCache>()?
        .newtab_context(&tab_id, session)
}

#[tauri::command]
#[specta::specta]
fn newtab_search(
    caller: WebviewWindow,
    app: tauri::AppHandle,
    shell: State<'_, Handle>,
    query: String,
    context: zephium_ipc::SearchContext,
) -> bool {
    if !authorize(&caller, CallerPolicy::Main, "newtab_search")
        || !bounded(&query, MAX_LAUNCHER_QUERY_BYTES)
        || !newtab_context_valid(&context)
    {
        return false;
    }
    let accepted = shell.dispatch(Command::SearchScoped {
        query: query.clone(),
        context: Box::new(context.clone()),
    });
    if accepted {
        search_providers::schedule(caller, app, shell.inner().clone(), query, context);
    }
    accepted
}

fn newtab_context_valid(context: &zephium_ipc::SearchContext) -> bool {
    context
        .session_id
        .strip_prefix("newtab:")
        .and_then(|value| value.split_once(':'))
        .filter(|(_, nonce)| {
            !nonce.is_empty()
                && nonce.len() <= 20
                && nonce.bytes().all(|byte| byte.is_ascii_digit())
        })
        .and_then(|(id, _)| ItemId::parse(id))
        .is_some()
        && bounded(&context.request_id, 64)
        && !context.request_id.is_empty()
        && bounded(&context.window_id, 64)
        && bounded(&context.profile_id, 64)
        && bounded(&context.space_id, 64)
}

#[tauri::command]
#[specta::specta]
fn newtab_cancel(
    caller: WebviewWindow,
    shell: State<'_, Handle>,
    context: zephium_ipc::SearchContext,
) -> bool {
    if !authorize(&caller, CallerPolicy::Main, "newtab_cancel") || !newtab_context_valid(&context) {
        return false;
    }
    search_providers::cancel(&context.session_id);
    shell.dispatch(Command::CancelSearch {
        session_id: context.session_id,
    })
}

#[tauri::command]
#[specta::specta]
fn newtab_run(
    caller: WebviewWindow,
    app: tauri::AppHandle,
    shell: State<'_, Handle>,
    action: zephium_ipc::SearchAction,
    context: zephium_ipc::SearchContext,
) -> zephium_ipc::OperationAdmission {
    if !authorize(&caller, CallerPolicy::Main, "newtab_run")
        || !newtab_context_valid(&context)
        || !search_action_in_bounds(&action)
    {
        return rejected_operation();
    }
    dispatch_operation(
        &app,
        &shell,
        Command::RunSearchAction {
            context: Box::new(context),
            action,
            background: false,
        },
    )
}

#[tauri::command]
#[specta::specta]
fn launcher_search(
    caller: WebviewWindow,
    app: tauri::AppHandle,
    shell: State<'_, Handle>,
    query: String,
    request_id: String,
) -> bool {
    if !authorize(&caller, CallerPolicy::Panel, "launcher_search")
        || !bounded(&query, MAX_LAUNCHER_QUERY_BYTES)
    {
        return false;
    }
    let Some(context) = app
        .try_state::<overlay::Overlay>()
        .and_then(|overlay| overlay.search_context(&request_id))
    else {
        diagnostic!("launcher: search refused outside a presented session");
        return false;
    };
    let accepted = shell.dispatch(Command::SearchScoped {
        query: query.clone(),
        context: Box::new(context.clone()),
    });
    if !accepted {
        diagnostic!("launcher: search refused by a full actor queue");
    }
    if accepted {
        search_providers::schedule(caller, app, shell.inner().clone(), query, context);
    }
    accepted
}

#[tauri::command]
#[specta::specta]
fn launcher_run(
    caller: WebviewWindow,
    app: tauri::AppHandle,
    action: zephium_ipc::SearchAction,
    context: zephium_ipc::SearchContext,
    background: bool,
) -> zephium_ipc::OperationAdmission {
    if !authorize(&caller, CallerPolicy::Panel, "launcher_run") || !search_action_in_bounds(&action)
    {
        return rejected_operation();
    }
    let (Some(shell), Some(overlay), Some(ledger)) = (
        app.try_state::<Handle>(),
        app.try_state::<overlay::Overlay>(),
        app.try_state::<OperationLedger>(),
    ) else {
        return rejected_operation();
    };
    let Ok(sequence) = NEXT_OPERATION_ID
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1))
    else {
        return rejected_operation();
    };
    let operation_id = format!("{sequence:016x}");
    if !ledger.reserve(&operation_id) {
        return rejected_operation();
    }
    // Only an address can be sent behind the current tab; every other action
    // is about where the user is going next.
    let background = background && matches!(action, zephium_ipc::SearchAction::OpenUrl { .. });
    if !overlay.arm_action(&operation_id, &context, background) {
        return finish_operation_admission(&ledger, operation_id, false);
    }
    let accepted = shell.dispatch_operation(
        operation_id.clone(),
        Command::RunSearchAction {
            context: Box::new(context),
            action,
            background,
        },
    );
    if !accepted {
        overlay.action_rejected(&operation_id);
    }
    finish_operation_admission(&ledger, operation_id, accepted)
}

#[derive(Clone, Debug, Serialize, Deserialize, specta::Type)]
struct UiInfo {
    material: material::Material,
}

#[tauri::command]
#[specta::specta]
fn ui_info(caller: WebviewWindow) -> UiInfo {
    if !authorize(&caller, CallerPolicy::Both, "ui_info") {
        return UiInfo {
            material: material::Material::None,
        };
    }
    #[cfg(target_os = "macos")]
    let material = material::current(caller.label());
    #[cfg(target_os = "windows")]
    let material = platform::imp::material(caller.label());
    #[cfg(all(unix, not(target_os = "macos")))]
    let material = material::Material::None;
    UiInfo { material }
}

#[tauri::command]
#[specta::specta]
fn ui_ready(caller: WebviewWindow, gate: State<'_, UiStartupGate>) -> bool {
    if !authorize(&caller, CallerPolicy::Main, "ui_ready") {
        return false;
    }
    gate.mark_frontend_ready(&caller)
}

fn menu_popup_anchor(x: f64, y: f64, width: f64, height: f64) -> Option<LogicalPosition<f64>> {
    if !x.is_finite()
        || !y.is_finite()
        || !width.is_finite()
        || !height.is_finite()
        || width <= 0.0
        || height <= 0.0
        || x < 0.0
        || y < 0.0
        || x > width
        || y > height
    {
        return None;
    }
    Some(LogicalPosition::new(x, y))
}

#[tauri::command]
#[specta::specta]
fn menu_popup(caller: WebviewWindow, app: tauri::AppHandle, x: f64, y: f64) -> bool {
    if !authorize(&caller, CallerPolicy::Main, "menu_popup") {
        return false;
    }
    let Ok(inner_size) = caller.inner_size() else {
        return false;
    };
    let Ok(scale_factor) = caller.scale_factor() else {
        return false;
    };
    if !scale_factor.is_finite() || scale_factor <= 0.0 {
        return false;
    }
    let Some(anchor) = menu_popup_anchor(
        x,
        y,
        f64::from(inner_size.width) / scale_factor,
        f64::from(inner_size.height) / scale_factor,
    ) else {
        return false;
    };
    let keymap = keymap_overrides(&app);
    let Ok((menu, _)) = build_menu(&app, &keymap) else {
        return false;
    };
    caller.popup_menu_at(&menu, anchor).is_ok()
}

const ADD_MENU_COMMAND_IDS: [&str; 2] = ["tab.new", "split.choose"];

#[tauri::command]
#[specta::specta]
fn add_menu_popup(
    caller: WebviewWindow,
    app: tauri::AppHandle,
    x: f64,
    y: f64,
    can_split: bool,
) -> bool {
    if !authorize(&caller, CallerPolicy::Main, "add_menu_popup") {
        return false;
    }
    let Ok(inner_size) = caller.inner_size() else {
        return false;
    };
    let Ok(scale_factor) = caller.scale_factor() else {
        return false;
    };
    if !scale_factor.is_finite() || scale_factor <= 0.0 {
        return false;
    }
    let Some(anchor) = menu_popup_anchor(
        x,
        y,
        f64::from(inner_size.width) / scale_factor,
        f64::from(inner_size.height) / scale_factor,
    ) else {
        return false;
    };
    let keymap = keymap_overrides(&app);
    let Ok(menu) = build_add_menu(&app, &keymap, can_split) else {
        return false;
    };
    caller.popup_menu_at(&menu, anchor).is_ok()
}

#[tauri::command]
#[specta::specta]
fn resource_close_ready(caller: WebviewWindow, token: String, success: bool) -> bool {
    authorize(&caller, CallerPolicy::Both, "resource_close_ready")
        && resource_close::complete(caller.label(), &token, success)
}

/// How long a resource call waits for an admission permit. A typing pause
/// in several views at once queues briefly instead of failing; a call that
/// still cannot start is refused before it has touched anything.
const RESOURCE_ADMISSION_WAIT: std::time::Duration = std::time::Duration::from_secs(2);
/// The whole call, admission included, inside the frame's own deadline.
const RESOURCE_CALL_DEADLINE: std::time::Duration = std::time::Duration::from_secs(8);

async fn admit(
    admission: &'static tokio::sync::Semaphore,
    wait: std::time::Duration,
) -> Option<tokio::sync::SemaphorePermit<'static>> {
    tokio::time::timeout(wait, admission.acquire())
        .await
        .ok()?
        .ok()
}

#[tauri::command]
#[specta::specta]
async fn resource_call(
    caller: WebviewWindow,
    app: tauri::AppHandle,
    expected_profile: String,
    call: zephium_ipc::ResourceCall,
) -> zephium_ipc::ResourceReply {
    use zephium_core::resources::{ResourceError, ResourceReply, ResourceResponse};
    let failed = |error| ResourceReply {
        profile: None,
        response: ResourceResponse::Error { error },
    };
    if !authorize(&caller, CallerPolicy::Both, "resource_call") || shutdown_started(&app) {
        return failed(ResourceError::Unavailable);
    }
    if !call.validate() || serde_json::to_vec(&call).map_or(true, |bytes| bytes.len() > 524288) {
        return failed(ResourceError::Invalid);
    }
    if !resource_close::touch(&app, caller.label()) {
        return failed(ResourceError::Unavailable);
    }
    let Some(expected_profile) =
        ProfileId::parse(&expected_profile).filter(|id| id.to_string() == expected_profile)
    else {
        return failed(ResourceError::Invalid);
    };
    let shell = app.state::<Handle>().inner().clone();
    static RESOURCE_ADMISSION: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(4);
    let deadline = tokio::time::Instant::now() + RESOURCE_CALL_DEADLINE;
    let Some(permit) = admit(&RESOURCE_ADMISSION, RESOURCE_ADMISSION_WAIT).await else {
        return failed(ResourceError::Capacity);
    };
    let (send, receive) = tokio::sync::oneshot::channel();
    if !shell.dispatch(Command::ResourceCall {
        expected_profile,
        call: Arc::new(call),
        done: zephium_app::ResourceCompletion::new(move |reply| {
            let _permit = permit;
            let changed = match &reply.response {
                ResourceResponse::Applied { record, .. } => {
                    use zephium_core::resources::ResourceKind;
                    match record.draft.kind() {
                        ResourceKind::Task => Some(ResourceChangeKind::Task),
                        ResourceKind::Object => Some(ResourceChangeKind::Object),
                        ResourceKind::Media => Some(ResourceChangeKind::Media),
                        ResourceKind::Note => None,
                    }
                    .map(|kind| (kind, &record.id, &record.revision))
                }
                ResourceResponse::TaskListApplied { list, .. } => {
                    Some((ResourceChangeKind::TaskList, &list.id, &list.revision))
                }
                _ => None,
            };
            if let (Some(profile), Some((kind, id, revision))) = (&reply.profile, changed) {
                let event = ResourceChanged {
                    profile: profile.clone(),
                    kind,
                    id: id.clone(),
                    revision: revision.clone(),
                };
                // The launcher never hosts a resource view, so waking it for
                // every write would only cost a hidden WebView work.
                emit_to_privileged(&app, MAIN_LABEL, "zephium:resource-changed", &event);
            }
            let _ = send.send(reply);
        }),
    }) {
        return failed(ResourceError::Unavailable);
    }
    match tokio::time::timeout_at(deadline, receive).await {
        Ok(Ok(reply)) => reply,
        _ => failed(ResourceError::OutcomeUnknown),
    }
}

/// Opens an address in the focused window. The launcher panel and the history
/// surfaces have no tab id to navigate, and must not be given one.
#[tauri::command]
#[specta::specta]
fn browser_open_url(
    caller: WebviewWindow,
    shell: State<'_, Handle>,
    url: String,
    new_tab: bool,
) -> zephium_ipc::OperationAdmission {
    if !authorize(&caller, CallerPolicy::Both, "browser_open_url")
        || !bounded(&url, MAX_NAVIGATION_INPUT_BYTES)
        || !zephium_core::navigation::is_allowed_str(&url)
    {
        return rejected_operation();
    }
    dispatch_operation(
        caller.app_handle(),
        &shell,
        Command::OpenUrl {
            input: url,
            new_tab,
        },
    )
}

#[tauri::command]
#[specta::specta]
async fn download_call(
    caller: WebviewWindow,
    app: tauri::AppHandle,
    expected_profile: String,
    call: zephium_core::downloads::DownloadCall,
) -> zephium_core::downloads::DownloadResponse {
    use zephium_core::downloads::{DownloadCompletion, DownloadError, DownloadResponse};
    let failed = |error| DownloadResponse::Error { error };
    if !authorize(&caller, CallerPolicy::Both, "download_call") || shutdown_started(&app) {
        return failed(DownloadError::Unavailable);
    }
    if !call.validate() {
        return failed(DownloadError::Invalid);
    }
    let Some(profile) =
        ProfileId::parse(&expected_profile).filter(|id| id.to_string() == expected_profile)
    else {
        return failed(DownloadError::Invalid);
    };
    static ADMISSION: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(4);
    let Ok(permit) = ADMISSION.try_acquire() else {
        return failed(DownloadError::Capacity);
    };
    let (send, receive) = tokio::sync::oneshot::channel();
    let interactive = matches!(call, zephium_core::downloads::DownloadCall::ChooseDirectory);
    let shell = app.state::<Handle>().inner().clone();
    if !shell.dispatch(Command::DownloadCall {
        expected_profile: profile,
        call: Box::new(call),
        done: DownloadCompletion::new(move |response| {
            let _permit = permit;
            let _ = send.send(response);
        }),
    }) {
        return failed(DownloadError::Unavailable);
    }
    match tokio::time::timeout(
        std::time::Duration::from_secs(if interactive { 24 * 60 * 60 } else { 15 }),
        receive,
    )
    .await
    {
        Ok(Ok(response)) => response,
        _ => failed(DownloadError::Unavailable),
    }
}

/// Opens the system pane that grants folder access after a download was
/// refused by the platform (macOS Files and Folders). Elsewhere there is no
/// single pane to send people to, so the caller offers another folder instead.
#[tauri::command]
#[specta::specta]
fn download_open_access_settings(caller: WebviewWindow) -> bool {
    if !authorize(&caller, CallerPolicy::Both, "download_open_access_settings") {
        return false;
    }
    #[cfg(target_os = "macos")]
    {
        use objc2_app_kit::NSWorkspace;
        use objc2_foundation::{NSString, NSURL};
        let url = NSURL::URLWithString(&NSString::from_str(
            "x-apple.systempreferences:com.apple.preference.security?Privacy_FilesAndFolders",
        ));
        url.is_some_and(|url| NSWorkspace::sharedWorkspace().openURL(&url))
    }
    #[cfg(not(target_os = "macos"))]
    false
}

/// Asks for the site icons of origins chrome shows outside a tab. Held icons
/// arrive on the ordinary favicon event; missing ones are probed anonymously.
/// Returns whether the request was queued.
#[tauri::command]
#[specta::specta]
fn favicon_probe(
    caller: WebviewWindow,
    app: tauri::AppHandle,
    expected_profile: String,
    origins: Vec<String>,
) -> bool {
    if !authorize(&caller, CallerPolicy::Main, "favicon_probe") || shutdown_started(&app) {
        return false;
    }
    let Some(profile) =
        ProfileId::parse(&expected_profile).filter(|id| id.to_string() == expected_profile)
    else {
        return false;
    };
    if origins.len() > zephium_core::ports::store::MAX_FAVICON_BATCH_ORIGINS
        || origins.iter().any(|origin| origin.len() > 2048)
    {
        return false;
    }
    app.state::<Handle>()
        .dispatch(Command::ProbeFavicons { profile, origins })
}

#[tauri::command]
#[specta::specta]
async fn history_call(
    caller: WebviewWindow,
    app: tauri::AppHandle,
    expected_profile: String,
    call: zephium_ipc::HistoryCall,
) -> zephium_ipc::HistoryResponse {
    use zephium_ipc::{HistoryError, HistoryResponse};
    let failed = |error| HistoryResponse::Error { error };
    if !authorize(&caller, CallerPolicy::Both, "history_call") || shutdown_started(&app) {
        return failed(HistoryError::Unavailable);
    }
    if !call.validate() {
        return failed(HistoryError::Invalid);
    }
    let Some(expected_profile) =
        ProfileId::parse(&expected_profile).filter(|id| id.to_string() == expected_profile)
    else {
        return failed(HistoryError::Invalid);
    };
    let shell = app.state::<Handle>().inner().clone();
    static HISTORY_ADMISSION: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(2);
    let Ok(permit) = HISTORY_ADMISSION.try_acquire() else {
        return failed(HistoryError::Capacity);
    };
    let (send, receive) = tokio::sync::oneshot::channel();
    if !shell.dispatch(Command::HistoryCall {
        expected_profile,
        call: Box::new(call),
        done: zephium_app::HistoryCompletion::new(move |response| {
            let _permit = permit;
            let _ = send.send(response);
        }),
    }) {
        return failed(HistoryError::Unavailable);
    }
    match tokio::time::timeout(std::time::Duration::from_secs(8), receive).await {
        Ok(Ok(response)) => response,
        _ => failed(HistoryError::Unavailable),
    }
}

/// Finds `query` in the page in front, stepping forward or back when it
/// repeats; no query ends the search. Results arrive as `zephium:find`.
#[tauri::command]
#[specta::specta]
fn page_find(
    caller: WebviewWindow,
    shell: State<'_, Handle>,
    query: Option<String>,
    forward: bool,
) -> bool {
    use zephium_core::ports::engine::{FindRequest, MAX_FIND_QUERY_BYTES};
    if !authorize(&caller, CallerPolicy::Main, "page_find")
        || query
            .as_deref()
            .is_some_and(|query| query.is_empty() || query.len() > MAX_FIND_QUERY_BYTES)
    {
        return false;
    }
    shell.dispatch(Command::Find(
        query.map(|query| FindRequest { query, forward }),
    ))
}

/// The Bookmarks panel's reads and writes, scoped to the focused profile.
#[tauri::command]
#[specta::specta]
async fn bookmark_call(
    caller: WebviewWindow,
    app: tauri::AppHandle,
    expected_profile: String,
    call: zephium_ipc::BookmarkCall,
) -> zephium_ipc::BookmarkResponse {
    use zephium_ipc::{BookmarkError, BookmarkResponse};
    let failed = |error| BookmarkResponse::Error { error };
    if !authorize(&caller, CallerPolicy::Main, "bookmark_call") || shutdown_started(&app) {
        return failed(BookmarkError::Unavailable);
    }
    if !call.validate() {
        return failed(BookmarkError::Invalid);
    }
    let Some(expected_profile) =
        ProfileId::parse(&expected_profile).filter(|id| id.to_string() == expected_profile)
    else {
        return failed(BookmarkError::Invalid);
    };
    let shell = app.state::<Handle>().inner().clone();
    static ADMISSION: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(4);
    let Ok(permit) = ADMISSION.try_acquire() else {
        return failed(BookmarkError::Capacity);
    };
    let (send, receive) = tokio::sync::oneshot::channel();
    if !shell.dispatch(Command::BookmarkCall {
        expected_profile,
        call: Box::new(call),
        done: zephium_app::BookmarkCompletion::new(move |response| {
            let _permit = permit;
            let _ = send.send(response);
        }),
    }) {
        return failed(BookmarkError::Unavailable);
    }
    match tokio::time::timeout(std::time::Duration::from_secs(8), receive).await {
        Ok(Ok(response)) => response,
        _ => failed(BookmarkError::Unavailable),
    }
}

#[tauri::command]
#[specta::specta]
async fn time_call(
    caller: WebviewWindow,
    app: tauri::AppHandle,
    expected_profile: String,
    call: zephium_ipc::TimeCall,
) -> zephium_ipc::TimeResponse {
    use zephium_ipc::{TimeError, TimeResponse};
    let failed = |error| TimeResponse::Error { error };
    if !authorize(&caller, CallerPolicy::Main, "time_call") || shutdown_started(&app) {
        return failed(TimeError::Unavailable);
    }
    if !call.validate() {
        return failed(TimeError::Invalid);
    }
    let Some(expected_profile) =
        ProfileId::parse(&expected_profile).filter(|id| id.to_string() == expected_profile)
    else {
        return failed(TimeError::Invalid);
    };
    let shell = app.state::<Handle>().inner().clone();
    static ADMISSION: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(4);
    let Ok(permit) = ADMISSION.try_acquire() else {
        return failed(TimeError::Capacity);
    };
    let (send, receive) = tokio::sync::oneshot::channel();
    if !shell.dispatch(Command::TimeCall {
        expected_profile,
        call: Box::new(call),
        done: zephium_app::TimeCompletion::new(move |response| {
            let _permit = permit;
            let _ = send.send(response);
        }),
    }) {
        return failed(TimeError::Unavailable);
    }
    match tokio::time::timeout(std::time::Duration::from_secs(8), receive).await {
        Ok(Ok(response)) => response,
        _ => failed(TimeError::Unavailable),
    }
}

#[tauri::command]
#[specta::specta]
fn focus_control(
    caller: WebviewWindow,
    app: tauri::AppHandle,
    shell: State<'_, Handle>,
    control: zephium_ipc::FocusControl,
) -> zephium_ipc::OperationAdmission {
    if !authorize(&caller, CallerPolicy::Main, "focus_control")
        || shutdown_started(&app)
        || !control.validate()
    {
        return rejected_operation();
    }
    dispatch_operation(&app, &shell, Command::Focus(control))
}

#[tauri::command]
#[specta::specta]
async fn setting_get(caller: WebviewWindow, key: String) -> Option<String> {
    if !authorize(&caller, CallerPolicy::Both, "setting_get") {
        return None;
    }
    if !SETTING_KEYS.contains(&key.as_str()) {
        return None;
    }
    // A synchronous command runs on the main thread, and the store may be
    // busy for seconds (an import, a deletion); the read waits off it.
    tauri::async_runtime::spawn_blocking(move || {
        APP_STORE.get().and_then(|store| store.app_setting(&key))
    })
    .await
    .ok()
    .flatten()
}

#[tauri::command]
#[specta::specta]
fn setting_set(
    caller: WebviewWindow,
    app: tauri::AppHandle,
    shell: State<'_, Handle>,
    key: String,
    value: String,
) -> zephium_ipc::OperationAdmission {
    if !authorize(&caller, CallerPolicy::Main, "setting_set") {
        return rejected_operation();
    }
    if shutdown_started(&app) {
        return rejected_operation();
    }
    #[cfg(feature = "file-workflows-qa")]
    if key == "__files_qa_diagnostic" && value.len() <= 4096 {
        eprintln!("files-qa: {value}");
        return rejected_operation();
    }
    if SETTING_KEYS.contains(&key.as_str()) && setting_value_allowed(&key, &value) {
        #[cfg(feature = "work-product")]
        if matches!(key.as_str(), "ai.enabled" | "work.enabled") {
            let Some(owner) = app.try_state::<work_product::WorkProductState>() else {
                return rejected_operation();
            };
            return owner
                .operations
                .preference(&key, &value, || {
                    dispatch_operation(
                        &app,
                        &shell,
                        Command::SetAppSetting {
                            key: key.clone(),
                            value: value.clone(),
                        },
                    )
                })
                .unwrap_or_else(|_| rejected_operation());
        }
        return dispatch_operation(&app, &shell, Command::SetAppSetting { key, value });
    }
    rejected_operation()
}

#[tauri::command]
#[specta::specta]
fn tab_menu_popup(
    caller: WebviewWindow,
    app: tauri::AppHandle,
    id: String,
    x: f64,
    y: f64,
    context: TabMenuContext,
) -> bool {
    if !authorize(&caller, CallerPolicy::Main, "tab_menu_popup") || !bounded(&id, MAX_ITEM_ID_BYTES)
    {
        return false;
    }
    let Some(item) = ItemId::parse(&id) else {
        return false;
    };
    let Ok(inner_size) = caller.inner_size() else {
        return false;
    };
    let Ok(scale_factor) = caller.scale_factor() else {
        return false;
    };
    if !scale_factor.is_finite() || scale_factor <= 0.0 {
        return false;
    }
    let Some(anchor) = menu_popup_anchor(
        x,
        y,
        f64::from(inner_size.width) / scale_factor,
        f64::from(inner_size.height) / scale_factor,
    ) else {
        return false;
    };
    let Some(target) = app.try_state::<TabMenuTarget>() else {
        return false;
    };
    if !target.arm(item) {
        return false;
    }
    let Ok(menu) = build_tab_menu(&app, context) else {
        return false;
    };
    caller.popup_menu_at(&menu, anchor).is_ok()
}

#[tauri::command]
#[specta::specta]
fn bookmark_menu_popup(
    caller: WebviewWindow,
    app: tauri::AppHandle,
    x: f64,
    y: f64,
    folder: bool,
) -> bool {
    if !authorize(&caller, CallerPolicy::Main, "bookmark_menu_popup") {
        return false;
    }
    let Ok(inner_size) = caller.inner_size() else {
        return false;
    };
    let Ok(scale_factor) = caller.scale_factor() else {
        return false;
    };
    if !scale_factor.is_finite() || scale_factor <= 0.0 {
        return false;
    }
    let Some(anchor) = menu_popup_anchor(
        x,
        y,
        f64::from(inner_size.width) / scale_factor,
        f64::from(inner_size.height) / scale_factor,
    ) else {
        return false;
    };
    let Ok(menu) = build_bookmark_menu(&app, folder) else {
        return false;
    };
    caller.popup_menu_at(&menu, anchor).is_ok()
}

#[tauri::command]
#[specta::specta]
fn sidebar_menu_popup(
    caller: WebviewWindow,
    app: tauri::AppHandle,
    x: f64,
    y: f64,
    site_protected: Option<bool>,
    can_hide: bool,
) -> bool {
    if !authorize(&caller, CallerPolicy::Main, "sidebar_menu_popup") {
        return false;
    }
    let Ok(inner_size) = caller.inner_size() else {
        return false;
    };
    let Ok(scale_factor) = caller.scale_factor() else {
        return false;
    };
    if !scale_factor.is_finite() || scale_factor <= 0.0 {
        return false;
    }
    let Some(anchor) = menu_popup_anchor(
        x,
        y,
        f64::from(inner_size.width) / scale_factor,
        f64::from(inner_size.height) / scale_factor,
    ) else {
        return false;
    };
    let keymap = keymap_overrides(&app);
    let Ok(menu) = build_sidebar_menu(&app, &keymap, site_protected, can_hide) else {
        return false;
    };
    caller.popup_menu_at(&menu, anchor).is_ok()
}

/// The menu for the browser's own interface, in place of the engine's menu
/// for a web page. `page` says whether a web page is in front to act on.
#[tauri::command]
#[specta::specta]
fn chrome_menu_popup(
    caller: WebviewWindow,
    app: tauri::AppHandle,
    x: f64,
    y: f64,
    page: bool,
    can_split: bool,
) -> bool {
    if !authorize(&caller, CallerPolicy::Main, "chrome_menu_popup") {
        return false;
    }
    let Ok(inner_size) = caller.inner_size() else {
        return false;
    };
    let Ok(scale_factor) = caller.scale_factor() else {
        return false;
    };
    if !scale_factor.is_finite() || scale_factor <= 0.0 {
        return false;
    }
    let Some(anchor) = menu_popup_anchor(
        x,
        y,
        f64::from(inner_size.width) / scale_factor,
        f64::from(inner_size.height) / scale_factor,
    ) else {
        return false;
    };
    let keymap = keymap_overrides(&app);
    let Ok(menu) = build_chrome_menu(&app, &keymap, page, can_split) else {
        return false;
    };
    caller.popup_menu_at(&menu, anchor).is_ok()
}

#[tauri::command]
#[specta::specta]
fn profile_menu_popup(caller: WebviewWindow, app: tauri::AppHandle, x: f64, y: f64) -> bool {
    if !authorize(&caller, CallerPolicy::Main, "profile_menu_popup") {
        return false;
    }
    let Ok(inner_size) = caller.inner_size() else {
        return false;
    };
    let Ok(scale_factor) = caller.scale_factor() else {
        return false;
    };
    if !scale_factor.is_finite() || scale_factor <= 0.0 {
        return false;
    }
    let Some(anchor) = menu_popup_anchor(
        x,
        y,
        f64::from(inner_size.width) / scale_factor,
        f64::from(inner_size.height) / scale_factor,
    ) else {
        return false;
    };
    let keymap = keymap_overrides(&app);
    let Ok(menu) = build_profile_menu(&app, &keymap) else {
        return false;
    };
    caller.popup_menu_at(&menu, anchor).is_ok()
}

#[tauri::command]
#[specta::specta]
fn tools_menu_popup(caller: WebviewWindow, app: tauri::AppHandle, x: f64, y: f64) -> bool {
    if !authorize(&caller, CallerPolicy::Main, "tools_menu_popup") {
        return false;
    }
    let Ok(inner_size) = caller.inner_size() else {
        return false;
    };
    let Ok(scale_factor) = caller.scale_factor() else {
        return false;
    };
    if !scale_factor.is_finite() || scale_factor <= 0.0 {
        return false;
    }
    let Some(anchor) = menu_popup_anchor(
        x,
        y,
        f64::from(inner_size.width) / scale_factor,
        f64::from(inner_size.height) / scale_factor,
    ) else {
        return false;
    };
    let keymap = keymap_overrides(&app);
    let Ok(menu) = build_tools_menu(&app, &keymap) else {
        return false;
    };
    caller.popup_menu_at(&menu, anchor).is_ok()
}

#[tauri::command]
#[specta::specta]
fn panel_hide(caller: WebviewWindow, app: tauri::AppHandle) {
    if !authorize(&caller, CallerPolicy::Panel, "panel_hide") {
        return;
    }
    if let Some(overlay) = app.try_state::<overlay::Overlay>() {
        overlay.hide();
    }
}

#[tauri::command]
#[specta::specta]
fn panel_ready(
    caller: WebviewWindow,
    overlay: State<'_, overlay::Overlay>,
) -> Option<zephium_ipc::PanelState> {
    authorize(&caller, CallerPolicy::Panel, "panel_ready").then(|| {
        #[cfg(target_os = "windows")]
        platform::imp::renderer::ready(&caller);
        overlay.ready()
    })
}
#[tauri::command]
#[specta::specta]
fn panel_intent(
    caller: WebviewWindow,
    app: tauri::AppHandle,
    intent: zephium_ipc::PanelIntent,
) -> bool {
    if !authorize(&caller, CallerPolicy::Both, "panel_intent") {
        return false;
    }
    overlay::request(&app, move |panel| panel.intent(intent))
}
/// Onboarding's own page, which a first run opens in the main window
/// instead of the browser.
const ONBOARDING_PAGE: &str = "onboarding.html";

/// Whether this launch opens onboarding. `ZEPHIUM_ONBOARDING=1 pnpm dev`
/// opens it on an existing profile without recording anything until it is
/// finished.
fn onboarding_first(store: &SqliteStore) -> bool {
    #[cfg(debug_assertions)]
    if std::env::var("ZEPHIUM_ONBOARDING").as_deref() == Ok("1") {
        return true;
    }
    zephium_app::onboarding_due(store)
}

/// Onboarding's commands answer only the onboarding page, and only until it
/// has handed the window to the browser.
fn authorize_onboarding(caller: &WebviewWindow, command: &str) -> bool {
    authorize(caller, CallerPolicy::Main, command)
        && caller
            .try_state::<UiStartupGate>()
            .is_some_and(|gate| gate.serves_onboarding(caller))
}

/// Onboarding's intro sound, played natively; chrome keeps `autoplay=()`.
#[tauri::command]
#[specta::specta]
fn onboarding_play_intro(caller: WebviewWindow, app: tauri::AppHandle) -> bool {
    if !authorize_onboarding(&caller, "onboarding_play_intro") {
        return false;
    }
    intro_sound::play(&app);
    true
}

/// Finishes onboarding: records it, then opens the browser in its place.
#[tauri::command]
#[specta::specta]
fn onboarding_finish(caller: WebviewWindow, gate: State<'_, UiStartupGate>) -> bool {
    if !authorize_onboarding(&caller, "onboarding_finish") {
        return false;
    }
    // Opening the browser matters more than the record: if it is lost,
    // onboarding simply shows once more on the next launch.
    if !APP_STORE
        .get()
        .is_some_and(|store| zephium_app::finish_onboarding(store.as_ref()))
    {
        diagnostic!("onboarding: finishing could not be recorded");
    }
    gate.hand_over(&caller)
}

#[tauri::command]
#[specta::specta]
fn launcher_trigger(caller: WebviewWindow) -> Option<launcher_trigger::LauncherTrigger> {
    authorize(&caller, CallerPolicy::Main, "launcher_trigger")
        .then(|| launcher_trigger::snapshot(caller.app_handle()))
}

#[tauri::command]
#[specta::specta]
fn launcher_set_shortcut(
    caller: WebviewWindow,
    accelerator: String,
) -> Option<launcher_trigger::TriggerChange> {
    authorize(&caller, CallerPolicy::Main, "launcher_set_shortcut")
        .then(|| launcher_trigger::set_shortcut(caller.app_handle(), &accelerator))
}

/// Silences the current shortcut while a new one is being recorded.
#[tauri::command]
#[specta::specta]
fn launcher_record_shortcut(caller: WebviewWindow, active: bool) {
    if authorize(&caller, CallerPolicy::Main, "launcher_record_shortcut") {
        launcher_trigger::recording(caller.app_handle(), active);
    }
}

#[tauri::command]
#[specta::specta]
fn launcher_set_double_tap(
    caller: WebviewWindow,
    mode: launcher_trigger::DoubleTap,
) -> Option<launcher_trigger::LauncherTrigger> {
    authorize(&caller, CallerPolicy::Main, "launcher_set_double_tap")
        .then(|| launcher_trigger::set_double_tap(caller.app_handle(), mode))
}

#[tauri::command]
#[specta::specta]
fn launcher_open_accessibility(caller: WebviewWindow) {
    if authorize(&caller, CallerPolicy::Main, "launcher_open_accessibility") {
        launcher_trigger::open_accessibility_settings();
    }
}

/// The launcher's content reports what it shows; native sizes the window and
/// places the material behind it.
#[tauri::command]
#[specta::specta]
fn panel_layout(
    caller: WebviewWindow,
    overlay: State<'_, overlay::Overlay>,
    layout: zephium_ipc::PanelLayout,
) {
    if authorize(&caller, CallerPolicy::Panel, "panel_layout") {
        overlay.layout(layout);
    }
}

#[tauri::command]
#[specta::specta]
fn sidebar_set_width(
    caller: WebviewWindow,
    shell: State<'_, Handle>,
    width: f64,
    animate: bool,
    revision: f64,
) {
    let Some(revision) = sidebar_resize_revision(revision) else {
        return;
    };
    if !authorize(&caller, CallerPolicy::Main, "sidebar_set_width")
        || !sidebar_width_in_bounds(width)
    {
        return;
    }
    #[cfg(target_os = "macos")]
    if !platform::imp::publish_sidebar_resize_revision(revision) {
        return;
    }
    #[cfg(not(target_os = "macos"))]
    let _ = revision;
    shell.dispatch(Command::SetSidebarWidth(width, animate));
}

#[tauri::command]
#[specta::specta]
async fn sidebar_resize(caller: WebviewWindow, width: f64, enabled: bool, revision: f64) -> bool {
    let Some(revision) = sidebar_resize_revision(revision) else {
        return false;
    };
    if !authorize(&caller, CallerPolicy::Main, "sidebar_resize") || !sidebar_width_in_bounds(width)
    {
        return false;
    }
    let Some(shell) = caller
        .try_state::<Handle>()
        .map(|state| state.inner().clone())
    else {
        return false;
    };
    #[cfg(target_os = "macos")]
    {
        platform::imp::configure_sidebar_resize(&caller, width, enabled, revision, shell).await
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (shell, enabled, revision);
        false
    }
}

#[tauri::command]
#[specta::specta]
fn sidebar_resize_guide(
    caller: WebviewWindow,
    shell: State<'_, Handle>,
    width: Option<f64>,
) -> bool {
    if !authorize(&caller, CallerPolicy::Main, "sidebar_resize_guide")
        || width.is_some_and(|width| !sidebar_width_in_bounds(width))
    {
        return false;
    }
    #[cfg(target_os = "windows")]
    {
        shell.dispatch(Command::SidebarResizeGuide(width))
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = (shell, width);
        false
    }
}

#[tauri::command]
#[specta::specta]
fn tab_drag_over(caller: WebviewWindow, shell: State<'_, Handle>, x: Option<f64>, y: Option<f64>) {
    if !authorize(&caller, CallerPolicy::Main, "tab_drag_over") {
        return;
    }
    let point = match (x, y) {
        (None, None) => None,
        (Some(x), Some(y)) => match window_point(x, y) {
            Some(point) => Some(point),
            None => return,
        },
        _ => return,
    };
    shell.dispatch(Command::DragOver { point });
}

#[tauri::command]
#[specta::specta]
fn tab_drop(
    caller: WebviewWindow,
    shell: State<'_, Handle>,
    id: String,
    x: f64,
    y: f64,
) -> zephium_ipc::OperationAdmission {
    if !authorize(&caller, CallerPolicy::Main, "tab_drop") {
        return rejected_operation();
    }
    let Some((x, y)) = window_point(x, y) else {
        return rejected_operation();
    };
    dispatch_with_id(caller.app_handle(), &shell, &id, |id| Command::DropTab {
        id,
        x,
        y,
    })
}

#[tauri::command]
#[specta::specta]
fn divider_grab(caller: WebviewWindow, shell: State<'_, Handle>, x: f64, y: f64) {
    if !authorize(&caller, CallerPolicy::Main, "divider_grab") {
        return;
    }
    let Some((x, y)) = window_point(x, y) else {
        return;
    };
    shell.dispatch(Command::DividerGrab { x, y });
}

#[tauri::command]
#[specta::specta]
fn divider_drag(caller: WebviewWindow, shell: State<'_, Handle>, x: f64, y: f64) {
    if !authorize(&caller, CallerPolicy::Main, "divider_drag") {
        return;
    }
    let Some((x, y)) = window_point(x, y) else {
        return;
    };
    shell.dispatch(Command::DividerDrag { x, y });
}

#[tauri::command]
#[specta::specta]
fn divider_release(
    caller: WebviewWindow,
    shell: State<'_, Handle>,
    x: Option<f64>,
    y: Option<f64>,
) -> zephium_ipc::OperationAdmission {
    if !authorize(&caller, CallerPolicy::Main, "divider_release") {
        return rejected_operation();
    }
    let final_pointer = match (x, y) {
        (Some(x), Some(y)) => {
            let Some((x, y)) = window_point(x, y) else {
                return rejected_operation();
            };
            (Some(x), Some(y))
        }
        (None, None) => (None, None),
        _ => return rejected_operation(),
    };
    dispatch_operation(
        caller.app_handle(),
        &shell,
        Command::DividerRelease {
            x: final_pointer.0,
            y: final_pointer.1,
        },
    )
}

fn inner_logical(window: &tauri::WebviewWindow) -> Size {
    let scale = window.scale_factor().unwrap_or(1.0);
    window
        .inner_size()
        .map(|s| Size::new(s.width as f64 / scale, s.height as f64 / scale))
        .unwrap_or_default()
}

fn build_command_menu_item(
    handle: &tauri::AppHandle,
    resolved: &[zephium_core::commands::ResolvedCommand],
    id: &str,
) -> tauri::Result<tauri::menu::MenuItem<tauri::Wry>> {
    build_command_menu_item_enabled(handle, resolved, id, true)
}

fn build_command_menu_item_enabled(
    handle: &tauri::AppHandle,
    resolved: &[zephium_core::commands::ResolvedCommand],
    id: &str,
    enabled: bool,
) -> tauri::Result<tauri::menu::MenuItem<tauri::Wry>> {
    use tauri::menu::MenuItemBuilder;

    let command = resolved
        .iter()
        .find(|command| command.id == id)
        .ok_or_else(|| {
            tauri::Error::Io(std::io::Error::other(format!(
                "native menu references unregistered command {id}"
            )))
        })?;
    let mut builder = MenuItemBuilder::with_id(command.id, command.title).enabled(enabled);
    if let Some(accelerator) = &command.accelerator {
        builder = builder.accelerator(accelerator);
    }
    builder.build(handle)
}

fn build_quit_menu_item(
    handle: &tauri::AppHandle,
) -> tauri::Result<tauri::menu::MenuItem<tauri::Wry>> {
    // Native terminate: bypasses Tauri's ExitRequested path on macOS.
    // An ordinary menu event reaches the existing draft/store shutdown barrier.
    tauri::menu::MenuItemBuilder::with_id("browser.quit", "Quit Zephium")
        .accelerator("CmdOrCtrl+Q")
        .build(handle)
}

/// The application menu: the macOS menu bar, and the "more" popup on Windows
/// and Linux. Returns the Work pane items, which only the menu bar keeps.
fn build_menu(
    handle: &tauri::AppHandle,
    overrides: &std::collections::HashMap<String, String>,
) -> tauri::Result<(
    tauri::menu::Menu<tauri::Wry>,
    Vec<tauri::menu::MenuItem<tauri::Wry>>,
)> {
    use tauri::menu::{Menu, SubmenuBuilder};

    let resolved = zephium_core::commands::resolve(overrides);
    let item = |id: &str| build_command_menu_item(handle, &resolved, id);

    let app_menu = SubmenuBuilder::new(handle, "Zephium")
        .about(None)
        .separator()
        .item(&item("browser.settings")?)
        .separator()
        .services()
        .separator()
        .hide()
        .hide_others()
        .show_all()
        .separator()
        .item(&build_quit_menu_item(handle)?)
        .build()?;
    let file = SubmenuBuilder::new(handle, "File")
        .item(&item("tab.new")?)
        .item(&item("window.newPrivate")?)
        .item(&item("note.new")?)
        .item(&item("split.choose")?)
        .separator()
        .item(&item("page.print")?)
        .separator()
        .item(&item("tab.close")?)
        .build()?;
    let find = SubmenuBuilder::new(handle, "Find")
        .item(&item("find.show")?)
        .item(&item("find.next")?)
        .item(&item("find.previous")?)
        .build()?;
    // Standard Edit selectors keep Cmd+C/V/X working inside every webview.
    let edit = SubmenuBuilder::new(handle, "Edit")
        .undo()
        .redo()
        .separator()
        .cut()
        .copy()
        .paste()
        .select_all()
        .separator()
        .item(&find)
        .item(&item("page.copyLink")?)
        .build()?;
    let appearance = SubmenuBuilder::new(handle, "Appearance")
        .item(&item("theme.system")?)
        .item(&item("theme.light")?)
        .item(&item("theme.dark")?)
        .build()?;
    let view = SubmenuBuilder::new(handle, "View")
        .item(&item("nav.reload")?)
        .item(&item("nav.stop")?)
        .separator()
        .item(&item("zoom.in")?)
        .item(&item("zoom.out")?)
        .item(&item("zoom.reset")?)
        .separator()
        .item(&item("sidebar.toggleCompact")?)
        .item(&appearance)
        .separator()
        .item(&item("url.focus")?)
        .item(&item("page.devtools")?)
        .build()?;
    let history = SubmenuBuilder::new(handle, "History")
        .item(&item("nav.back")?)
        .item(&item("nav.forward")?)
        .separator()
        .item(&item("tab.reopen")?)
        .item(&item("browser.history")?)
        .build()?;
    let bookmarks = SubmenuBuilder::new(handle, "Bookmarks")
        .item(&item("bookmark.add")?)
        .item(&item("tool.bookmarks")?)
        .build()?;
    let window = SubmenuBuilder::new(handle, "Window")
        .minimize()
        .fullscreen()
        .separator()
        .item(&item("tab.next")?)
        .item(&item("tab.previous")?)
        .separator()
        .item(&item("tool.downloads")?)
        .item(&item("browser.tasks")?)
        .item(&item("browser.notes")?)
        .build()?;
    // Page keystrokes never reach privileged chrome, so the pane's dismissal
    // keys live in the menu and are enabled exactly while a pane is shown.
    let pane_close = build_command_menu_item_enabled(handle, &resolved, "work.pane.close", false)?;
    let pane_open =
        build_command_menu_item_enabled(handle, &resolved, "work.pane.openInBrowse", false)?;
    let work = SubmenuBuilder::new(handle, "Work")
        .item(&pane_close)
        .item(&pane_open)
        .build()?;
    let help = SubmenuBuilder::new(handle, "Help")
        .item(&item("settings.shortcuts")?)
        .build()?;

    let menu = Menu::with_items(
        handle,
        &[
            &app_menu, &file, &edit, &view, &history, &bookmarks, &work, &window, &help,
        ],
    )?;
    Ok((menu, vec![pane_close, pane_open]))
}

/// Installs (or reinstalls, after a rebinding) the macOS menu bar.
#[cfg(target_os = "macos")]
fn install_menu_bar(
    app: &tauri::AppHandle,
    overrides: &std::collections::HashMap<String, String>,
) -> tauri::Result<()> {
    let (menu, work_items) = build_menu(app, overrides)?;
    app.set_menu(menu)?;
    if let Some(keymap) = app.try_state::<keymap::Keymap>() {
        keymap.adopt_work_menu_items(work_items);
    }
    Ok(())
}

fn build_add_menu(
    handle: &tauri::AppHandle,
    overrides: &std::collections::HashMap<String, String>,
    can_split: bool,
) -> tauri::Result<tauri::menu::Menu<tauri::Wry>> {
    use tauri::menu::Menu;

    let resolved = zephium_core::commands::resolve(overrides);
    let item = |id: &str| build_command_menu_item(handle, &resolved, id);

    let new_tab = item(ADD_MENU_COMMAND_IDS[0])?;
    // A new tab has nothing to pair with, so the action is shown unavailable
    // rather than offered and then silently refused by the actor.
    let split_view =
        build_command_menu_item_enabled(handle, &resolved, ADD_MENU_COMMAND_IDS[1], can_split)?;
    Menu::with_items(handle, &[&new_tab, &split_view])
}

/// Delivered to privileged chrome, which holds the authoritative URL for the
/// armed tab. Native code never handles page-derived text for the clipboard.
const TAB_MENU_COPY_LINK_COMMAND: &str = "tab.copyLink";

/// Registry command delivered to the frame, which owns sidebar presentation.
const SIDEBAR_COMPACT_COMMAND: &str = "sidebar.toggleCompact";

/// The rail cannot show a navigation cluster, so the collapsed menu carries
/// the whole one rather than a subset of it.
const SIDEBAR_MENU_COMMAND_IDS: [&str; 4] = [
    "nav.back",
    "nav.forward",
    "nav.reload",
    SIDEBAR_COMPACT_COMMAND,
];

const PROTECTION_SITE_COMMAND: &str = "protection.site";
const PROTECTION_HIDE_COMMAND: &str = "protection.hide";

/// The bookmark panel arms the row it opened the menu on and applies the
/// choice itself, through the same calls a click makes. Native only relays
/// which entry was chosen, so it never holds a bookmark id.
const BOOKMARK_MENU_ACTION_IDS: [&str; 6] = [
    "bookmarkmenu.open",
    "bookmarkmenu.openNewTab",
    "bookmarkmenu.copyLink",
    "bookmarkmenu.rename",
    "bookmarkmenu.remove",
    "bookmarkmenu.removeFolder",
];

const TAB_MENU_ACTION_IDS: [&str; 10] = [
    "tabmenu.reload",
    "tabmenu.duplicate",
    "tabmenu.copyLink",
    "tabmenu.bookmark",
    "tabmenu.keep",
    "tabmenu.unkeep",
    "tabmenu.split",
    "tabmenu.close",
    "tabmenu.closeOthers",
    "tabmenu.closeBelow",
];

/// What the tab menu can offer for the tab it opens on, as the sidebar sees
/// it. Only availability: the shell checks every action again.
#[derive(Clone, Copy, Debug, Deserialize, specta::Type)]
struct TabMenuContext {
    /// A web page is loaded, so it can be copied, duplicated or bookmarked.
    page: bool,
    can_split: bool,
    essential: bool,
    /// Other open tabs exist to close.
    others: bool,
    /// Open tabs follow this one.
    below: bool,
}

/// `site_protected` is the page's protection standing, absent where site
/// controls do not apply; the frame owns both actions.
fn build_sidebar_menu(
    handle: &tauri::AppHandle,
    overrides: &std::collections::HashMap<String, String>,
    site_protected: Option<bool>,
    can_hide: bool,
) -> tauri::Result<tauri::menu::Menu<tauri::Wry>> {
    use tauri::menu::{
        CheckMenuItemBuilder, IsMenuItem, Menu, MenuItemBuilder, PredefinedMenuItem,
    };

    let resolved = zephium_core::commands::resolve(overrides);
    let back = build_command_menu_item(handle, &resolved, SIDEBAR_MENU_COMMAND_IDS[0])?;
    let forward = build_command_menu_item(handle, &resolved, SIDEBAR_MENU_COMMAND_IDS[1])?;
    let reload = build_command_menu_item(handle, &resolved, SIDEBAR_MENU_COMMAND_IDS[2])?;
    let compact = build_command_menu_item(handle, &resolved, SIDEBAR_MENU_COMMAND_IDS[3])?;
    let separator = PredefinedMenuItem::separator(handle)?;

    let protection = match site_protected {
        Some(protected) => Some((
            PredefinedMenuItem::separator(handle)?,
            CheckMenuItemBuilder::with_id(PROTECTION_SITE_COMMAND, "Block Ads and Trackers")
                .checked(protected)
                .build(handle)?,
            MenuItemBuilder::with_id(PROTECTION_HIDE_COMMAND, "Hide Elements…")
                .enabled(can_hide)
                .build(handle)?,
        )),
        None => None,
    };
    let mut items: Vec<&dyn IsMenuItem<tauri::Wry>> = vec![&back, &forward, &reload];
    if let Some((rule, site, hide)) = &protection {
        items.extend([rule as &dyn IsMenuItem<tauri::Wry>, site, hide]);
    }
    items.extend([&separator as &dyn IsMenuItem<tauri::Wry>, &compact]);

    #[cfg(target_os = "macos")]
    {
        Menu::with_items(handle, &items)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let window_separator = PredefinedMenuItem::separator(handle)?;
        let minimize = PredefinedMenuItem::minimize(handle, None)?;
        let maximize = PredefinedMenuItem::maximize(handle, None)?;
        let close = PredefinedMenuItem::close_window(handle, None)?;
        items.extend([
            &window_separator as &dyn IsMenuItem<tauri::Wry>,
            &minimize,
            &maximize,
            &close,
        ]);
        Menu::with_items(handle, &items)
    }
}

fn build_chrome_menu(
    handle: &tauri::AppHandle,
    overrides: &std::collections::HashMap<String, String>,
    page: bool,
    can_split: bool,
) -> tauri::Result<tauri::menu::Menu<tauri::Wry>> {
    use tauri::menu::{Menu, PredefinedMenuItem};
    let resolved = zephium_core::commands::resolve(overrides);
    let item =
        |id: &str, enabled: bool| build_command_menu_item_enabled(handle, &resolved, id, enabled);
    let new_tab = item("tab.new", true)?;
    let new_private = item("window.newPrivate", true)?;
    let reopen = item("tab.reopen", true)?;
    let first = PredefinedMenuItem::separator(handle)?;
    let bookmark = item("bookmark.add", page)?;
    let copy_link = item("page.copyLink", page)?;
    let second = PredefinedMenuItem::separator(handle)?;
    let split = item("split.choose", can_split)?;
    let compact = item(SIDEBAR_COMPACT_COMMAND, true)?;
    let third = PredefinedMenuItem::separator(handle)?;
    let bookmarks = item("tool.bookmarks", true)?;
    let settings = item("browser.settings", true)?;
    Menu::with_items(
        handle,
        &[
            &new_tab,
            &new_private,
            &reopen,
            &first,
            &bookmark,
            &copy_link,
            &second,
            &split,
            &compact,
            &third,
            &bookmarks,
            &settings,
        ],
    )
}

fn build_tab_menu(
    handle: &tauri::AppHandle,
    context: TabMenuContext,
) -> tauri::Result<tauri::menu::Menu<tauri::Wry>> {
    use tauri::menu::{Menu, MenuItemBuilder, PredefinedMenuItem};
    let item = |index: usize, title: &str, enabled: bool| {
        MenuItemBuilder::with_id(TAB_MENU_ACTION_IDS[index], title)
            .enabled(enabled)
            .build(handle)
    };
    let reload = item(0, "Reload Tab", true)?;
    let duplicate = item(1, "Duplicate Tab", context.page)?;
    let copy_link = item(2, "Copy Link", context.page)?;
    let bookmark = item(3, "Bookmark Tab", context.page)?;
    let first = PredefinedMenuItem::separator(handle)?;
    let keep = if context.essential {
        item(5, "Remove from Essentials", true)?
    } else {
        item(4, "Add to Essentials", context.page)?
    };
    let split = item(6, "Open in Split View", context.can_split)?;
    let second = PredefinedMenuItem::separator(handle)?;
    let close = item(7, "Close Tab", true)?;
    let others = item(8, "Close Other Tabs", context.others)?;
    let below = item(9, "Close Tabs Below", context.below)?;
    Menu::with_items(
        handle,
        &[
            &reload, &duplicate, &copy_link, &bookmark, &first, &keep, &split, &second, &close,
            &others, &below,
        ],
    )
}

fn build_bookmark_menu(
    handle: &tauri::AppHandle,
    folder: bool,
) -> tauri::Result<tauri::menu::Menu<tauri::Wry>> {
    use tauri::menu::{Menu, MenuItemBuilder, PredefinedMenuItem};
    let item = |index: usize, title: &str| {
        MenuItemBuilder::with_id(BOOKMARK_MENU_ACTION_IDS[index], title).build(handle)
    };
    let open = item(0, "Open")?;
    let rename = item(3, "Rename")?;
    if folder {
        let separator = PredefinedMenuItem::separator(handle)?;
        let remove = item(5, "Delete Folder")?;
        return Menu::with_items(handle, &[&open, &separator, &rename, &remove]);
    }
    let new_tab = item(1, "Open in New Tab")?;
    let first = PredefinedMenuItem::separator(handle)?;
    let copy_link = item(2, "Copy Link")?;
    let second = PredefinedMenuItem::separator(handle)?;
    let remove = item(4, "Delete")?;
    Menu::with_items(
        handle,
        &[
            &open, &new_tab, &first, &copy_link, &second, &rename, &remove,
        ],
    )
}

/// Identity and browser destinations use a native menu at every sidebar width.
fn build_tools_menu(
    handle: &tauri::AppHandle,
    overrides: &std::collections::HashMap<String, String>,
) -> tauri::Result<tauri::menu::Menu<tauri::Wry>> {
    use tauri::menu::{Menu, MenuItemBuilder, PredefinedMenuItem};
    let resolved = zephium_core::commands::resolve(overrides);
    let item = |id: &str| build_command_menu_item(handle, &resolved, id);
    let notes = MenuItemBuilder::with_id("tool.notes", "Notes").build(handle)?;
    let tasks = MenuItemBuilder::with_id("tool.tasks", "Tasks").build(handle)?;
    let activity = MenuItemBuilder::with_id("tool.time", "Time").build(handle)?;
    let first = PredefinedMenuItem::separator(handle)?;
    let history = MenuItemBuilder::with_id("tool.history", "History").build(handle)?;
    let downloads = item("tool.downloads")?;
    let bookmarks = item("tool.bookmarks")?;
    let second = PredefinedMenuItem::separator(handle)?;
    // The panel is the quick way in; the full destination is its own entry, the
    // same shape as Show All History.
    let all_tasks = item("browser.tasks")?;
    let all_notes = item("browser.notes")?;
    let settings = item("browser.settings")?;
    Menu::with_items(
        handle,
        &[
            &notes, &tasks, &activity, &first, &history, &downloads, &bookmarks, &second,
            &all_notes, &all_tasks, &settings,
        ],
    )
}

fn build_profile_menu(
    handle: &tauri::AppHandle,
    overrides: &std::collections::HashMap<String, String>,
) -> tauri::Result<tauri::menu::Menu<tauri::Wry>> {
    use tauri::menu::{Menu, MenuItemBuilder, PredefinedMenuItem};
    let resolved = zephium_core::commands::resolve(overrides);
    let item = |id: &str| build_command_menu_item(handle, &resolved, id);
    let profile = MenuItemBuilder::with_id("settings.profiles", "Profile…").build(handle)?;
    let account = MenuItemBuilder::with_id("settings.account", "Account…").build(handle)?;
    let first = PredefinedMenuItem::separator(handle)?;
    let new_tab = item("tab.new")?;
    let new_private = item("window.newPrivate")?;
    let split = MenuItemBuilder::with_id("split.choose", "Split View…").build(handle)?;
    let second = PredefinedMenuItem::separator(handle)?;
    let notes = MenuItemBuilder::with_id("tool.notes", "Notes").build(handle)?;
    let tasks = MenuItemBuilder::with_id("tool.tasks", "Tasks").build(handle)?;
    let time = MenuItemBuilder::with_id("tool.time", "Time").build(handle)?;
    let history = MenuItemBuilder::with_id("tool.history", "History").build(handle)?;
    let downloads = item("tool.downloads")?;
    let bookmarks = item("tool.bookmarks")?;
    let extensions = MenuItemBuilder::with_id("extensions.manage", "Extensions…").build(handle)?;
    let third = PredefinedMenuItem::separator(handle)?;
    let settings = item("browser.settings")?;
    let quit = build_quit_menu_item(handle)?;
    Menu::with_items(
        handle,
        &[
            &profile,
            &account,
            &first,
            &new_tab,
            &new_private,
            &split,
            &second,
            &notes,
            &tasks,
            &time,
            &history,
            &downloads,
            &bookmarks,
            &extensions,
            &third,
            &settings,
            &quit,
        ],
    )
}

fn handle_run_event(app: &tauri::AppHandle, event: tauri::RunEvent) {
    #[cfg(feature = "macos-work-public-inspection")]
    work_development::on_run_event(app, &event);
    #[cfg(all(
        feature = "macos-work-navigation-probe",
        not(feature = "macos-work-profile-enrollment"),
        target_os = "macos"
    ))]
    if navigation_probe::on_run_event(app, &event) {
        return;
    }
    #[cfg(all(feature = "macos-work-rendering-probe", target_os = "macos"))]
    if foreground_rendering_probe::on_run_event(app, &event) {
        return;
    }
    #[cfg(target_os = "linux")]
    if matches!(&event, tauri::RunEvent::Exit) {
        linux_global_shortcuts::shutdown(app);
    }
    #[cfg(target_os = "macos")]
    if matches!(&event, tauri::RunEvent::Exit) {
        updates::finish_on_exit(app);
    }
    #[cfg(target_os = "macos")]
    if let tauri::RunEvent::Opened { urls } = &event {
        external_links::hand_off(app, urls.iter().map(|url| url.to_string()), true);
        return;
    }
    let tauri::RunEvent::ExitRequested { code, api, .. } = event else {
        return;
    };
    if code != Some(1) && updates::blocks_exit(app) {
        api.prevent_exit();
        return;
    }
    let Some(coordinator) = app.try_state::<ShutdownCoordinator>() else {
        write_diagnostic(format_args!(
            "shutdown: exit requested before coordinator setup"
        ));
        // Do not panic or terminate directly from the native event callback.
        // A second, explicitly unsuccessful request is allowed through so a
        // broken composition invariant still converges instead of looping.
        if code == Some(1) {
            return;
        }
        api.prevent_exit();
        app.exit(1);
        return;
    };
    // Tauri restart cannot normally be prevented. A startup failure is
    // sticky, however: never allow a failed initialization to masquerade as
    // a successful restart into a potentially rollback-vulnerable state.
    if code == Some(tauri::RESTART_EXIT_CODE)
        && !coordinator.terminal_failure.load(Ordering::Acquire)
    {
        return;
    };
    if exit_request_is_authorized(
        code,
        coordinator.authorized_exit_code.load(Ordering::Acquire),
    ) {
        if coordinator.terminal_failure.load(Ordering::Acquire) && code != Some(1) {
            // Cleanup may have completed just before a startup watchdog made
            // failure sticky. Replace the already-authorized successful exit
            // with a correlated failure; do not let the stale status through.
            api.prevent_exit();
            coordinator.schedule_authorized_exit(app.clone(), 1);
            return;
        }
        return;
    }
    api.prevent_exit();
    if let Some(shell) = app.try_state::<Handle>() {
        let owner = coordinator.inner().clone();
        let exit_app = app.clone();
        let handle = shell.inner().clone();
        resource_close::request(app.clone(), move || owner.request(exit_app, handle));
    } else {
        write_diagnostic(format_args!("shutdown: exit requested before shell setup"));
        let engine_owner = app
            .try_state::<StartupEngine>()
            .map(|engine| engine.inner().clone());
        let blocker_owner = app
            .try_state::<StartupBlocker>()
            .map(|blocker| blocker.inner().clone());
        let store_owner = app
            .try_state::<StartupStore>()
            .map(|store| store.inner().clone());
        coordinator.request_terminal_startup_failure(
            app.clone(),
            TerminalStartupResources {
                engine_owner,
                blocker_owner,
                store_owner,
                ..TerminalStartupResources::default()
            },
        );
    }
}

fn install_async_runtime() {
    match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_stack_size(ASYNC_WORKER_STACK_BYTES)
        .build()
    {
        Ok(runtime) => {
            tauri::async_runtime::set(ASYNC_RUNTIME.get_or_init(|| runtime).handle().clone())
        }
        Err(_) => diagnostic!("startup: async runtime stack configuration unavailable"),
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    if cfg!(all(unix, not(target_os = "macos"))) {
        diagnostic!(
            "Zephium isn't available on Linux yet. We're working on it; follow https://zephium.app for news."
        );
        std::process::exit(1);
    }
    install_async_runtime();
    APP_STARTED.get_or_init(std::time::Instant::now);
    #[cfg(target_os = "macos")]
    let runtime_security_advisories = match platform::imp::enforce_runtime_security_floor() {
        Ok(advisory) => advisory,
        Err(error) => {
            // WebKit is dynamically supplied by macOS. Only an unsupported
            // or unparseable runtime stops here, before Builder creates even
            // the privileged blank bootstrap WKWebView; an outdated one
            // starts with an update advisory.
            diagnostic!("security: {error}");
            startup_alert::show_blocking(startup_alert::StartupProblem::UnsupportedSystem, &error);
            std::process::exit(78);
        }
    };

    #[cfg(all(unix, not(target_os = "macos")))]
    let runtime_security_advisories = match zephium_engine::enforce_runtime_security_floor() {
        Ok(advisory) => advisory,
        Err(error) => {
            // WebKitGTK is dynamically supplied by the OS. Reject known
            // obsolete/development runtimes and security overrides before
            // Builder constructs even the privileged blank bootstrap WebView.
            diagnostic!("security: {error}");
            std::process::exit(78);
        }
    };

    #[cfg(target_os = "windows")]
    let runtime_security_advisories = match platform::imp::enforce_runtime_security_floor() {
        Ok(advisory) => advisory,
        Err(error) => {
            // This runs before Builder creates either privileged chrome
            // or raw content. Preview, overridden, or unparseable runtimes
            // remain hard failures; an outdated one is an advisory.
            diagnostic!("security: {error}");
            startup_alert::show_blocking(startup_alert::StartupProblem::UnsupportedSystem, &error);
            std::process::exit(78);
        }
    };

    let specta = specta_builder();
    #[cfg(target_os = "windows")]
    let attempted_privileged_environments = Arc::new(AtomicU8::new(0));
    #[cfg(target_os = "windows")]
    let setup_privileged_environments = attempted_privileged_environments.clone();
    let builder = tauri::Builder::default()
        // Every Tauri-managed webview is zone 2. It may load only the bundled
        // application origin (or the exact Vite origin in debug builds).
        .plugin(navigation_lock())
        .register_asynchronous_uri_scheme_protocol(media::SCHEME, media::serve)
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        // Must register first: a second launch (file association, dock, a
        // stale instance holding the global hotkey and the profile dbs)
        // focuses the running window and exits.
        .plugin(tauri_plugin_single_instance::init(|app, argv, _cwd| {
            external_links::bring_forward(app);
            external_links::hand_off(app, argv.into_iter().skip(1), true);
        }));
    // global-hotkey is X11-only on Linux. Initializing its Tauri plugin on a
    // native Wayland session can report success while receiving no keys (or
    // fail startup when Xwayland is absent). Linux selects an actual GDK
    // backend below and uses either direct X11 grabs or the desktop portal.
    #[cfg(not(target_os = "linux"))]
    let builder = builder.plugin(tauri_plugin_global_shortcut::Builder::new().build());
    let app = builder
        .invoke_handler(specta.invoke_handler())
        // This state must predate the setup hook: every environmental failure
        // is contained inside that native Ready callback and converted into a
        // correlated event-loop exit instead of escaping as Tauri's panic.
        .manage(ShutdownCoordinator::default())
        .manage(updates::Updates::default())
        // Storage is admitted first inside setup, but its temporary cleanup
        // owner must already exist so installation and later Shell transfer are
        // one exact, failure-observable transaction.
        .manage(StartupStore::default())
        // Own a successfully installed native host until shell ownership is
        // published. Setup failures in that narrow interval can therefore
        // execute the same explicit bounded engine teardown.
        .manage(StartupEngine::default())
        // The managed blocker starts its own compiler/updater workers. Keep
        // their exact Arc reachable from the instant construction succeeds
        // until the app-managed shell publishes authoritative ownership.
        .manage(StartupBlocker::default())
        .setup(move |app| {
            let setup_result: SetupResult = (|| {
                let shutdown = app
                    .try_state::<ShutdownCoordinator>()
                    .ok_or_else(|| {
                        std::io::Error::other("shutdown coordinator state is unavailable")
                    })?;
                // Allocate the independent deadline thread before any native
                // callback or persistent actor exists. Later callbacks only
                // signal it and can never fail while spawning a watchdog.
                shutdown.prepare_hard_exit_watchdog()?;
                specta.mount_events(app);
                let data_dir = app.path().app_data_dir()?;
                #[cfg(all(target_os = "windows", feature = "webext-qa"))]
                let qa_session = webext_qa::session_label()?;
                #[cfg(all(target_os = "windows", feature = "webext-qa"))]
                let data_dir = webext_qa::data_dir(data_dir, qa_session.as_deref())?;
                #[cfg(all(feature = "macos-work-rendering-probe", target_os = "macos"))]
                foreground_rendering_probe::validate_data_root(&data_dir)?;
                #[cfg(all(feature = "macos-work-navigation-probe", target_os = "macos"))]
                navigation_probe::validate_data_root(&data_dir)?;
                std::fs::create_dir_all(&data_dir)?;
                #[cfg(target_os = "windows")]
                updates::restore(app.handle(), &data_dir);
                #[cfg(feature = "work-product")]
                zephium_app::work_lead::skills::install_root(data_dir.clone());
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    std::fs::set_permissions(&data_dir, std::fs::Permissions::from_mode(0o700))?;
                }
                #[cfg(all(feature = "macos-work-profile-enrollment", target_os = "macos"))]
                navigation_probe::mark_profile_enrollment(&data_dir)?;
                #[cfg(target_os = "windows")]
                {
                    // Release builds have no console. Establish the bounded,
                    // private diagnostic sink before storage admission so a
                    // schema/corruption failure is still actionable without
                    // constructing WebView2 state merely to initialize logs.
                    platform::imp::redirect_stderr(&data_dir);
                    diagnostics::install(data_dir.clone());
                    diagnostic!("zephium {} starting", env!("CARGO_PKG_VERSION"));
                }
                #[cfg(target_os = "macos")]
                {
                    // Finder-launched apps write stderr nowhere; keep it in
                    // a private, bounded log the person can choose to share.
                    let logs = app.path().app_log_dir()?;
                    platform::imp::redirect_stderr(&logs);
                    diagnostics::install(logs);
                    diagnostic!("zephium {} starting", env!("CARGO_PKG_VERSION"));
                }

                // Storage is the first fallible subsystem admitted after the
                // private data root is secured. A corrupt/newer database must
                // fail before either privileged chrome or raw content creates
                // native renderer state.
                #[cfg(all(target_os = "windows", feature = "work-product"))]
                let store = {
                    #[cfg(feature = "webext-qa")]
                    let session = qa_session.as_deref();
                    #[cfg(not(feature = "webext-qa"))]
                    let session = None;
                    let work_storage = zephium_store::WindowsWorkStorage::for_application(
                        &app.config().identifier,
                        session,
                    )?;
                    Arc::new(SqliteStore::open_with_windows_work_storage(&data_dir, work_storage)?)
                };
                #[cfg(not(all(target_os = "windows", feature = "work-product")))]
                let store = Arc::new(SqliteStore::open(&data_dir)?);
                app.manage(media::MediaBlobs(zephium_store::MediaStore::new(
                    data_dir.join("media"),
                )));
                #[cfg(feature = "work-product")]
                zephium_app::work_connections::store::install(&data_dir);
                #[cfg(feature = "work-product")]
                app.manage(work_product::WorkFrames(Arc::new(
                    zephium_store::WorkFrameStore::new(data_dir.join("work-frames")),
                )));
                let startup_store = app.try_state::<StartupStore>().ok_or_else(|| {
                    std::io::Error::other("startup storage cleanup owner is unavailable")
                })?;
                if !startup_store.install(store.clone()) {
                    let error =
                        std::io::Error::other("startup storage cleanup owner is already armed");
                    request_pre_shell_startup_failure_with_store(
                        app.handle(),
                        &error,
                        store.clone(),
                    );
                    return Err(error.into());
                }
                APP_STORE.set(store.clone()).map_err(|_| {
                    std::io::Error::other("process-global application store is already installed")
                })?;
                updates::schedule_native_checks(app.handle());
                work_models::install(app.handle());

                #[cfg(target_os = "windows")]
                let privileged_runtime = prepare_privileged_runtime_directories(&data_dir)?;

                let mut main_config = app
                .config()
                .app
                .windows
                .first()
                .cloned()
                .ok_or_else(|| std::io::Error::other("main window configuration is missing"))?;
                // Decided before either page loads: a first run opens onboarding
                // in this window, and the browser replaces it there once it is
                // finished. The browser bundle carries none of it.
                let onboarding = onboarding_first(store.as_ref());
                let browser_url = privileged_app_url(app, &main_config.url)?;
                let app_url = if onboarding {
                    privileged_app_url(app, &tauri::WebviewUrl::App(ONBOARDING_PAGE.into()))?
                } else {
                    browser_url.clone()
                };
                main_config.url = tauri::WebviewUrl::External(
                tauri::Url::parse(PRIVILEGED_BOOTSTRAP_URL)
                    .map_err(|error| std::io::Error::other(error.to_string()))?,
            );
                let main_builder = tauri::WebviewWindowBuilder::from_config(app, &main_config)?
                // Do not expose an interactive privileged renderer until the
                // platform deny handlers below have replaced WebKit/WebView2
                // defaults. This also closes the small post-build attachment
                // window while we remain on Tauri's public construction API.
                .visible(false)
                // Privileged chrome is a projection of Rust-owned state. It
                // must not persist cookies, cache, service workers, or web
                // storage across launches.
                .incognito(true)
                .devtools(cfg!(debug_assertions))
                .general_autofill_enabled(false)
                .initialization_script_for_all_frames(zephium_engine::PAGE_PRINT_DENY_SCRIPT);
            #[cfg(feature = "work-development-traces")]
            let main_builder = main_builder.initialization_script(startup_styles::SCRIPT);
            #[cfg(target_os = "windows")]
            let main_builder = main_builder
                .decorations(false)
                // Seed the native parent's first erase, not just WebView2's
                // transparent renderer. Otherwise a hidden decorated window
                // can reveal an unpainted white client surface until redraw.
                .background_color(tauri::utils::config::Color(0, 0, 0, 0))
                .data_directory(privileged_runtime.main.clone())
                // Supplying any explicit value replaces Wry's default, which
                // also disables msSmartScreenProtection. Keep only the two
                // browser-UI suppressions for privileged chrome.
                .additional_browser_args(PRIVILEGED_WEBVIEW2_BROWSER_ARGS);
            #[cfg(not(all(unix, not(target_os = "macos"))))]
            let main_builder = main_builder.on_download(|_, _| false);
            #[cfg(feature = "file-workflows-qa")]
            let main_builder = main_builder.initialization_script(r#"
              (() => {
                const report = value => {
                  try { window.__TAURI_INTERNALS__.invoke('setting_set', {key:'__files_qa_diagnostic',value:String(value).slice(0,4096)}).catch(()=>{}); } catch {}
                };
                const originalError = console.error.bind(console);
                console.error = (...args) => { report(args.map(value => value?.stack || String(value)).join(' ')); originalError(...args); };
                window.addEventListener('error', event => report(`error: ${event.message || event.target?.src || event.target?.href || 'resource'} ${event.filename || ''}:${event.lineno || ''}`), true);
                window.addEventListener('unhandledrejection', event => report(`rejection: ${event.reason?.stack || event.reason?.message || event.reason}`));
                window.addEventListener('DOMContentLoaded', () => report(`document ready; root children=${document.getElementById('root')?.childElementCount}`));
              })();
            "#);
            let ui_startup_gate = if onboarding {
                UiStartupGate::onboarding_first(app_url.clone(), browser_url)
            } else {
                UiStartupGate::new(app_url.clone())
            };
            app.manage(ui_startup_gate.clone());
            #[cfg(target_os = "windows")]
            app.manage(platform::imp::renderer::Renderers::default());
            let page_gate = ui_startup_gate.clone();
            #[cfg(target_os = "windows")]
            setup_privileged_environments
                .fetch_or(PRIVILEGED_MAIN_ENVIRONMENT, Ordering::Release);
            let window = main_builder
                .on_new_window(|_, _| tauri::webview::NewWindowResponse::Deny)
                .on_web_resource_request(|request, response| {
                    #[cfg(feature = "work-development-traces")]
                    startup_styles::stylesheet_response(&request, response);
                    harden_privileged_headers(response.headers_mut());
                    // Onboarding is left for good: never kept in the
                    // back-forward cache behind the browser that replaced it.
                    if request.uri().path().strip_prefix('/') == Some(ONBOARDING_PAGE) {
                        response.headers_mut().insert(
                            tauri::http::header::CACHE_CONTROL,
                            tauri::http::HeaderValue::from_static("no-store"),
                        );
                    }
                })
                .on_page_load(move |window, payload| {
                    // The native window is transparent and its privileged
                    // WebView is deliberately constructed at about:blank so
                    // deny handlers can be installed before app code runs.
                    // Never expose that engine-default backing surface. A
                    // native Finished event proves only document loading; the
                    // trusted frontend separately acknowledges deterministic
                    // theme/DOM initialization. The opaque bootstrap surface
                    // remains in place until then; the gate requires both
                    // facts and is idempotent in either order.
                    if matches!(payload.event(), tauri::webview::PageLoadEvent::Finished) {
                        page_gate.mark_document_loaded(&window, payload.url());
                    }
                })
                .build()?;
            // Onboarding composes for one size; the browser takes the window
            // back resizable once it has replaced onboarding.
            if onboarding {
                window.set_resizable(false)?;
                window.set_maximizable(false)?;
            }
            let startup_watchdog_gate = ui_startup_gate.clone();
            let startup_watchdog_app = app.handle().clone();
            std::thread::Builder::new()
                .name("zephium-ui-startup-watchdog".into())
                .spawn(move || {
                    std::thread::sleep(UI_INITIALIZATION_TIMEOUT);
                    if startup_watchdog_gate.is_visible() {
                        return;
                    }
                    let exit_app = startup_watchdog_app.clone();
                    let exit_gate = startup_watchdog_gate.clone();
                    let _ = startup_watchdog_app.run_on_main_thread(move || {
                        if !exit_gate.is_visible() {
                            request_startup_failure(
                                &exit_app,
                                "trusted application document did not finish and initialize before the deadline",
                            );
                        }
                    });
                })
                .map_err(|error| {
                    std::io::Error::other(format!(
                        "could not start UI startup watchdog: {error}"
                    ))
                })?;
            #[cfg(any(target_os = "macos", target_os = "windows"))]
            let parent = window.window_handle()?.as_raw();
            let handle = app.handle().clone();

            // Privileged WebView2 environments exist before the shell and raw
            // engine do. Keep an early callback target plus a sticky bit; once
            // the engine exists, every privileged notification enters its
            // process-global dedupe gate and the shell can reconcile the bit
            // even if its command queue was not installed yet.
            let slot: Arc<OnceLock<zephium_app::CallbackHandle>> = Arc::new(OnceLock::new());
            #[cfg(target_os = "windows")]
            let runtime_engine_slot: Arc<OnceLock<Arc<zephium_engine::WebviewEngine>>> =
                Arc::new(OnceLock::new());
            #[cfg(target_os = "windows")]
            let pending_runtime_update = Arc::new(AtomicBool::new(false));
            #[cfg(target_os = "windows")]
            let runtime_update_notifier: platform::imp::RuntimeUpdateCallback = {
                let engine_slot = runtime_engine_slot.clone();
                let pending = pending_runtime_update.clone();
                Arc::new(move || {
                    if let Some(engine) = engine_slot.get() {
                        engine.notify_runtime_restart_required();
                    } else {
                        pending.store(true, Ordering::Release);
                    }
                })
            };

            let dispatch_handle = handle.clone();
            let dispatch: MainThreadDispatch = Arc::new(move |task: Box<dyn FnOnce() + Send>| {
                dispatch_handle.run_on_main_thread(task).is_ok()
            });

            #[cfg(target_os = "windows")]
            let platform_initialized = platform::imp::init(
                &window,
                &privileged_runtime.main,
                runtime_update_notifier.clone(),
            );
            #[cfg(target_os = "macos")]
            let platform_initialized = platform::imp::init(&window);
            #[cfg(any(target_os = "macos", target_os = "windows"))]
            if !platform_initialized {
                return Err(std::io::Error::other(
                    "required privileged-WebView hardening or native composition failed",
                )
                .into());
            }
            #[cfg(all(unix, not(target_os = "macos")))]
            platform::imp::init(&window).map_err(|error| {
                std::io::Error::other(format!(
                    "required privileged WebKitGTK hardening or GTK composition failed: {error}"
                ))
            })?;
            // The engine is owned by the shell. Retaining a strong Handle in
            // its callback would form Shell -> Engine -> Handle -> queue and
            // keep the actor/ticker alive after every application owner drops.
            let sink_slot = slot.clone();
            let sink_app = handle.clone();
            let terminal_failure_app = handle.clone();
            let engine = Arc::new(zephium_engine::install(
                #[cfg(any(target_os = "macos", target_os = "windows"))]
                parent,
                dispatch.clone(),
                data_dir.join("web-content"),
                runtime_security_advisories,
                InitialUserContent::new(
                    UserContentGeneration::new(1)
                        .expect("initial user-content generation is nonzero"),
                    UserContent {
                        scripts: Vec::new(),
                        styles: vec![UserStyle {
                            id: ScriptId::from(4),
                            owner: ScriptOwner::Builtin,
                            css: SCROLLBAR_CSS.into(),
                            matches: MatchSet::all_urls(),
                            all_frames: true,
                        }],
                    },
                ),
                move |event| {
                    #[cfg(all(feature = "macos-work-retained-controller-probe", target_os = "macos"))]
                    if zephium_work_composition::retained_qualification::retained_controller_policy_event(&event) { return; }
                    #[cfg(all(feature = "macos-work-resource-probe", target_os = "macos"))]
                    if zephium_engine::work_resource_policy_event(&event) { return; }
                    #[cfg(all(feature = "macos-work-rendering-probe", target_os = "macos"))]
                    if zephium_engine::foreground_rendering_policy_event(&event) { return; }
                    if let zephium_core::ports::engine::EngineEvent::ShortcutPressed {
                        command,
                        ..
                    } = &event
                    {
                        let _ = execute_command(&sink_app, command);
                        return;
                    }
                    if let Some(shell) = sink_slot.get() {
                        shell.dispatch(Command::Engine(event));
                    }
                },
                move |reason| {
                    // A refused native close/erasure transition can leave a
                    // renderer executing after Rust has retired authority.
                    // Skip engine-driven teardown, but preserve Tauri/App and
                    // Windows privileged-environment finalization.
                    request_unrecoverable_native_failure(&terminal_failure_app, reason);
                },
            )?);
            let downloads_app = handle.clone();
            if !engine.initialize_downloads(store.clone(), move |profile| {
                let event = DownloadsChanged { profile: profile.to_string() };
                for label in [MAIN_LABEL, overlay::PANEL_LABEL] {
                    emit_to_privileged(&downloads_app, label, "zephium:downloads-changed", &event);
                }
            }) { return Err(std::io::Error::other("download service initialization was not admitted").into()); }
            let startup_engine = app.try_state::<StartupEngine>().ok_or_else(|| {
                std::io::Error::other("startup engine cleanup owner is unavailable")
            })?;
            if !startup_engine.install(engine.clone()) {
                return Err(
                    std::io::Error::other("startup engine cleanup owner is already armed").into(),
                );
            }
            #[cfg(target_os = "windows")]
            runtime_engine_slot
                .set(engine.clone())
                .map_err(|_| std::io::Error::other("runtime engine callback slot already set"))?;
            let operation_ledger = OperationLedger::default();
            app.manage(operation_ledger.clone());
            app.manage(TabMenuTarget::default());

            app.manage(keymap::Keymap::load());
            app.manage(browser_import::ImportJobs::default());
            let keymap = keymap_overrides(&handle);
            // Windows and Linux get the same menu as a popup from the sidebar
            // "more" button instead of a persistent bar.
            #[cfg(target_os = "macos")]
            install_menu_bar(&handle, &keymap)?;
            app.on_menu_event(|app, event| {
                let _ = execute_command(app, event.id().0.as_str());
            });
            let keys_engine = engine.clone();
            app.state::<keymap::Keymap>()
                .attach_engine(Box::new(move |table| keys_engine.set_shortcuts(table)));
            #[cfg(target_os = "macos")]
            {
                let shortcut_app = handle.clone();
                platform::imp::install_key_monitor(
                    &window,
                    app.state::<keymap::Keymap>().key_table(),
                    move |id| {
                        let _ = execute_command(&shortcut_app, id);
                    },
                );
            }
            #[cfg(target_os = "linux")]
            let global_registration = linux_shortcut_portal::GlobalRegistration::default();
            #[cfg(target_os = "linux")]
            let focused_shortcut_presses = platform::imp::FocusedShortcutPresses::default();
            #[cfg(target_os = "linux")]
            let linux_launcher_shortcut = {
                let accelerator = zephium_core::commands::resolve(&keymap)
                    .iter()
                    .find(|command| command.id == linux_shortcut::LAUNCHER_COMMAND_ID)
                    .and_then(|command| command.accelerator.clone());
                let shortcut = accelerator
                    .as_deref()
                    .and_then(linux_shortcut::LinuxLauncherShortcut::parse);
                if accelerator.is_some() && shortcut.is_none() {
                    write_diagnostic(format_args!(
                        "global shortcut: launcher accelerator is not representable consistently on Linux; focused and global registration are disabled"
                    ));
                }
                shortcut
            };
            #[cfg(target_os = "linux")]
            let window_shortcuts =
                app.state::<keymap::Keymap>().window_table(linux_launcher_shortcut);
            #[cfg(target_os = "linux")]
            {
                let focused_registration = global_registration.clone();
                let shortcut_app = handle.clone();
                platform::imp::install_shortcuts(
                    &window,
                    window_shortcuts.clone(),
                    focused_shortcut_presses.clone(),
                    move |id| {
                        if id == linux_shortcut::LAUNCHER_COMMAND_ID
                            && focused_registration.is_live()
                        {
                            return;
                        }
                        let _ = execute_command(&shortcut_app, id);
                    },
                );
            }

            app.manage(overlay::ContextCache::default());
            let emit_handle = handle.clone();
            let disposition_ledger = operation_ledger.clone();
            let emit: EmitFn = Box::new(move |projection| match projection {
                Projection::WorkEnvironmentChanged(change) => emit_to_privileged(
                    &emit_handle, MAIN_LABEL, "zephium:work-environment-changed", &change,
                ),
                Projection::WorkChanged(change) => emit_to_privileged(
                    &emit_handle,
                    MAIN_LABEL,
                    "zephium:work-changed",
                    &change,
                ),
                Projection::PanelOwner(owner) => overlay::update_context(&emit_handle, &owner),
                Projection::Items(state) => {
                    emit_to_privileged(&emit_handle, MAIN_LABEL, EVENT_ITEMS, &state)
                }
                Projection::Tab(tab) => {
                    emit_to_privileged(&emit_handle, MAIN_LABEL, EVENT_TAB, &tab)
                }
                Projection::Favicons(favicons) => emit_to_privileged(
                    &emit_handle,
                    match favicons.surface {
                        zephium_ipc::IconSurface::Chrome => MAIN_LABEL,
                        zephium_ipc::IconSurface::Panel => overlay::PANEL_LABEL,
                    },
                    EVENT_FAVICONS,
                    &favicons,
                ),
                Projection::ExtensionActions(actions) => emit_to_privileged(
                    &emit_handle,
                    MAIN_LABEL,
                    EVENT_EXTENSION_ACTIONS,
                    &actions,
                ),
                Projection::ExtensionActionFailed(failure) => emit_to_privileged(
                    &emit_handle,
                    MAIN_LABEL,
                    EVENT_EXTENSION_ACTION_FAILED,
                    &failure,
                ),
                Projection::WebExtensionAccessRequest(request) => emit_to_privileged(
                    &emit_handle,
                    MAIN_LABEL,
                    EVENT_WEB_EXTENSION_ACCESS,
                    &request,
                ),
                Projection::ExtensionActionShortcut(shortcut) => emit_to_privileged(
                    &emit_handle,
                    MAIN_LABEL,
                    EVENT_EXTENSION_ACTION_SHORTCUT,
                    &shortcut,
                ),
                Projection::PagePermissionPrompt(prompt) => emit_to_privileged(
                    &emit_handle,
                    MAIN_LABEL,
                    EVENT_PAGE_PERMISSION_PROMPT,
                    &prompt,
                ),
                Projection::FindResult(result) => {
                    emit_to_privileged(&emit_handle, MAIN_LABEL, EVENT_FIND, &result)
                }
                Projection::UiCommand(id) => {
                    if id.starts_with("preference.search.") { search_providers::cancel_all(); }
                    #[cfg(target_os = "macos")]
                    if let Some(value) = id.strip_prefix("preference.ui.reduce-motion=") {
                        panel::set_reduce_motion(value == "true");
                    }
                    if let Some(alert) = id.strip_prefix("focus.alert=") {
                        focus_alerts::phase_ended(&emit_handle, alert);
                    }
                    if let Some(mode) = id.strip_prefix("theme.") {
                        if matches!(mode, "system" | "light" | "dark") {
                            apply_native_theme(&emit_handle, mode);
                        }
                    }
                    emit_ui_command(&emit_handle, &id);
                }
                Projection::OpenNote { profile, id } => emit_to_privileged(&emit_handle, MAIN_LABEL, "zephium:note-open-requested", &NoteOpenRequested { profile, id }),
                Projection::Search(results) => {
                    emit_to_privileged(&emit_handle, if results.context.as_ref().is_some_and(|context| context.session_id.starts_with("newtab:")) { MAIN_LABEL } else { overlay::PANEL_LABEL }, EVENT_SEARCH, &results)
                }
                Projection::Layout(layout) => {
                    if let Some(keymap) = emit_handle.try_state::<keymap::Keymap>() {
                        keymap.set_work_pane_shown(layout.work_pane.is_some());
                    }
                    emit_to_privileged(&emit_handle, MAIN_LABEL, EVENT_LAYOUT, &layout)
                }
                Projection::HostFullscreen(active) => content_fullscreen::follow(&emit_handle, active),
                Projection::RuntimeStatus(status) => {
                    emit_to_privileged(&emit_handle, MAIN_LABEL, EVENT_RUNTIME_STATUS, &status)
                }
                Projection::BlockerStatus(status) => {
                    emit_to_privileged(&emit_handle, MAIN_LABEL, EVENT_BLOCKER_STATUS, &status)
                }
                Projection::Focus(status) => {
                    if status.session.is_some() {
                        focus_alerts::session_started();
                    }
                    emit_to_privileged(&emit_handle, MAIN_LABEL, EVENT_FOCUS, &status)
                }
                Projection::OperationProcessed(disposition) => {
                    let panel_result=disposition.clone();
                    if record_and_deliver_operation(&disposition_ledger,disposition,|disposition| {
                        work_product::preference_processed(&emit_handle, disposition);
                        try_emit_to_privileged(&emit_handle,MAIN_LABEL,EVENT_OPERATION_PROCESSED,disposition)
                    }) {
                        if let Some(panel)=emit_handle.try_state::<overlay::Overlay>() {panel.operation(panel_result);}
                    } else {
                        diagnostic!("operation: rejected duplicate or unreserved actor disposition");
                    }
                }

            });

            let chrome: SharedChrome = platform::imp::make_chrome(&window, dispatch.clone());
            // Official source candidates must pass the same native compiler
            // as profile policies before becoming the durable current lists.
            let native_validation:zephium_blocker_service::NativeRuleValidator={
                let engine=engine.clone();
                Arc::new(move |rules,done|zephium_core::ports::engine::Engine::validate_content_rules(engine.as_ref(),rules,done))
            };
            let blocker = blocker_service::start_with_updates(&data_dir,native_validation).map_err(|error| {
                std::io::Error::other(format!(
                    "failed to start managed content-policy service: {error}"
                ))
            })?;
            let startup_blocker = app.try_state::<StartupBlocker>().ok_or_else(|| {
                std::io::Error::other("startup blocker cleanup owner is unavailable")
            })?;
            if !startup_blocker.install(blocker.clone()) {
                // A second setup transaction is an invariant violation, but
                // the newly-created workers are still ours to reap because
                // they were never published into the temporary owner.
                let error = std::io::Error::other(
                    "startup blocker cleanup owner is already armed",
                );
                request_pre_shell_startup_failure_with_blocker(
                    app.handle(),
                    &error,
                    blocker,
                );
                return Err(error.into());
            }

            let terminal_failure_app = app.handle().clone();
            let terminal_failure_shutdown = shutdown.inner().clone();
            let shell_terminal_failure: ShellTerminalFailureCallback = Box::new(move |failure| {
                // An unopenable session would otherwise leave an empty window
                // that does nothing. Say so, then quit in order; the store
                // has kept the saved data as it was.
                if matches!(
                    failure,
                    zephium_app::ShellTerminalFailure::SessionUnavailable
                        | zephium_app::ShellTerminalFailure::SessionFromNewerVersion
                ) {
                    let app = terminal_failure_app.clone();
                    let shutdown = terminal_failure_shutdown.clone();
                    let explained = terminal_failure_app.run_on_main_thread(move || {
                        #[cfg(not(target_os = "linux"))]
                        startup_alert::show_blocking(
                            if matches!(
                                failure,
                                zephium_app::ShellTerminalFailure::SessionFromNewerVersion
                            ) {
                                startup_alert::StartupProblem::NewerProfile
                            } else {
                                startup_alert::StartupProblem::DamagedProfile
                            },
                            "the saved tabs and settings could not be opened",
                        );
                        request_shell_terminal_failure(&app, &shutdown, failure);
                    });
                    if explained.is_ok() {
                        return;
                    }
                }
                request_shell_terminal_failure(
                    &terminal_failure_app,
                    &terminal_failure_shutdown,
                    failure,
                )
            });
            let shell = match zephium_app::spawn_suspended(
                engine.clone(),
                store.clone(),
                blocker.clone(),
                shell_terminal_failure,
                chrome,
                emit,
            ) {
                Ok(shell) => shell,
                Err(failure) => {
                    if !failure.worker_cleanup_proven() {
                        write_diagnostic(format_args!(
                            "startup: application helper-worker cleanup was not proven after Shell construction failed"
                        ));
                    }
                    return Err(failure.into());
                }
            };
            if !app.manage(shell.clone()) {
                let error = std::io::Error::other("shell cleanup state is already installed");
                // The local actor owns the Store, engine, and blocker even
                // though Tauri refused to publish its Handle. Route that
                // exact owner through the coordinator before the outer setup
                // error callback can observe unrelated managed state.
                shutdown.request_terminal_startup_failure(
                    app.handle().clone(),
                    TerminalStartupResources {
                        shell: Some(shell),
                        ..TerminalStartupResources::default()
                    },
                );
                return Err(error.into());
            }
            #[cfg(feature = "work-product")]
            if !work_product::install(
                app.handle(),
                engine.clone(),
                store.clone(),
                app.try_state::<work_product::WorkFrames>().map(|frames| frames.0.clone()),
            ) {
                let error = std::io::Error::other("Work product owner is already installed");
                shutdown.request_terminal_startup_failure(
                    app.handle().clone(),
                    TerminalStartupResources { shell: Some(shell), ..TerminalStartupResources::default() },
                );
                return Err(error.into());
            }
            notes::install(app.handle(), &data_dir, store.clone(), &shell);
            external_links::adopt(app.handle());
            #[cfg(feature = "work-product")]
            favicon_probe::install(&shell);
            let web_extensions = webext::WebExtensions::new(&data_dir);
            web_extensions.restore(&shell);
            app.manage(web_extensions);
            #[cfg(feature = "macos-work")]
            if !work::install(app.handle(), engine.clone(), store.clone()) {
                let error = std::io::Error::other("Work composition owner is already installed");
                shutdown.request_terminal_startup_failure(
                    app.handle().clone(),
                    TerminalStartupResources { shell: Some(shell), ..TerminalStartupResources::default() },
                );
                return Err(error.into());
            }
            #[cfg(all(feature = "macos-work-rendering-probe", target_os = "macos"))]
            if !foreground_rendering_probe::install(app.handle(), engine.clone(), store.clone()) {
                return Err(std::io::Error::other("rendering probe owner already installed").into());
            }
            #[cfg(all(
                feature = "macos-work-navigation-probe",
                not(feature = "macos-work-profile-enrollment"),
                target_os = "macos"
            ))]
            navigation_probe::install(app.handle())?;
            #[cfg(feature = "macos-work-public-inspection")]
            work_development::install(app.handle())?;
            if !startup_engine.transfer_to(&engine) {
                return Err(std::io::Error::other(
                    "startup engine ownership did not transfer to the shell",
                )
                .into());
            }
            if !startup_blocker.transfer_to(&blocker) {
                return Err(std::io::Error::other(
                    "startup blocker ownership did not transfer to the shell",
                )
                .into());
            }
            if !startup_store.transfer_to(&store) {
                return Err(std::io::Error::other(
                    "startup storage ownership did not transfer to the shell",
                )
                .into());
            }
            // Publish the weak callback ingress before actor admission, then
            // make the single exact transition only after every temporary
            // cleanup owner has been cleared. A concurrent terminal request may
            // cancel admission and is never allowed to be overtaken here.
            slot.set(shell.callback_handle()).map_err(|_| {
                std::io::Error::other("native engine callback ingress is already published")
            })?;
            if !shutdown.try_admit_shell(&shell) {
                return Err(std::io::Error::other(
                    "terminal shutdown started before Shell actor admission, or admission was already consumed",
                )
                .into());
            }
            if shutdown.terminal_started() {
                return Err(std::io::Error::other(
                    "terminal shutdown overtook Shell actor admission",
                )
                .into());
            }
            #[cfg(target_os = "windows")]
            if pending_runtime_update.swap(false, Ordering::AcqRel) {
                engine.notify_runtime_restart_required();
            }

            let initial =
                platform::imp::content_size(&window).unwrap_or_else(|| inner_logical(&window));
            shell.dispatch(Command::SetWindowSize(initial));
            shell.dispatch(Command::SetWindowFocused(window.is_focused().unwrap_or(false)));

            let resize_shell = shell.clone();
            let resize_window = window.clone();
            let exit_handle = handle.clone();
            let window_shutdown = shutdown.inner().clone();
            window.on_window_event(move |event| {
                match event {
                    tauri::WindowEvent::Resized(size)
                        if !window_shutdown.terminal_started() =>
                    {
                        let minimized = resize_window
                            .is_minimized()
                            .unwrap_or(size.width == 0 || size.height == 0);
                        resize_shell.dispatch(Command::SetWindowVisible(!minimized));
                        if minimized {
                            return;
                        }
                        let size = platform::imp::content_size(&resize_window)
                            .unwrap_or_else(|| inner_logical(&resize_window));
                        resize_shell.dispatch(Command::SetWindowSize(size));
                    }
                    #[cfg(target_os = "windows")]
                    tauri::WindowEvent::Moved(_)
                        if !window_shutdown.terminal_started()
                            && !resize_window.is_minimized().unwrap_or(false) =>
                    {
                        // WebView2 requires an explicit parent-position
                        // notification for IME, accessibility and popup
                        // coordinates. Re-submit the coalescible layout even
                        // when logical size is unchanged; the Windows stage
                        // emits only the position-dependent native delta.
                        let size = platform::imp::content_size(&resize_window)
                            .unwrap_or_else(|| inner_logical(&resize_window));
                        resize_shell.dispatch(Command::SetWindowSize(size));
                    }
                    tauri::WindowEvent::ScaleFactorChanged { .. }
                        if !window_shutdown.terminal_started() =>
                    {
                        let minimized = resize_window.is_minimized().unwrap_or(false);
                        resize_shell.dispatch(Command::SetWindowVisible(!minimized));
                        if !minimized {
                            // Re-read logical content size at the new scale.
                            // The layout refresh also rebuilds Windows
                            // physical bounds/corner radii and parent-position
                            // notifications using the new per-monitor DPI.
                            let size = platform::imp::content_size(&resize_window)
                                .unwrap_or_else(|| inner_logical(&resize_window));
                            resize_shell.dispatch(Command::SetWindowSize(size));
                        }
                    }
                    tauri::WindowEvent::ThemeChanged(_theme)
                        if NATIVE_APPEARANCE.load(Ordering::Acquire) == APPEARANCE_SYSTEM =>
                    {
                        #[cfg(target_os = "windows")]
                        apply_native_materials(&exit_handle, "system");
                    }
                    // Keep the main window alive and responsive until every
                    // shell command queued before close has been snapshotted.
                    tauri::WindowEvent::CloseRequested { api, .. } => {
                        api.prevent_close();
                        if updates::blocks_exit(&exit_handle) { return; }
                        let owner=window_shutdown.clone();
                        let app=exit_handle.clone();
                        let shell=resize_shell.clone();
                        resource_close::request(exit_handle.clone(),move ||owner.request(app,shell));
                    }
                    // Fallback for platform/programmatic destruction paths
                    // that do not emit a preventable close request first.
                    tauri::WindowEvent::Destroyed => {
                        #[cfg(target_os = "windows")]
                        if !platform::imp::remove_privileged_version_observer(MAIN_LABEL) {
                            diagnostic!(
                                "runtime: main WebView2 update observer removal was reentrant"
                            );
                        }
                        window_shutdown.request(exit_handle.clone(), resize_shell.clone())
                    }
                    tauri::WindowEvent::DragDrop(tauri::DragDropEvent::Drop {
                        paths,
                        position,
                    }) => {
                        // Offer extension packages in Browse; Work routes the same drop as context.
                        if let Some(path) = webext::dropped_package(paths) {
                            emit_to_privileged(
                                &exit_handle,
                                MAIN_LABEL,
                                EVENT_WEB_EXTENSION_DROPPED,
                                &path,
                            );
                        }
                        // Finder drops reach the frame as a DOM event with the
                        // dropped paths and the drop point in CSS pixels; the
                        // frame admits each path through the folder policy.
                        let scale = resize_window.scale_factor().unwrap_or(1.0);
                        let logical = position.to_logical::<f64>(scale);
                        let paths: Vec<String> = paths
                            .iter()
                            .map(|path| path.to_string_lossy().into_owned())
                            .collect();
                        if let Ok(detail) = serde_json::to_string(&serde_json::json!({
                            "paths": paths,
                            "x": logical.x,
                            "y": logical.y,
                        })) {
                            let _ = resize_window.eval(format!(
                                "window.dispatchEvent(new CustomEvent('zephium:work-paths-dropped',{{detail:{detail}}}))"
                            ));
                        }
                    }
                    tauri::WindowEvent::Focused(true) => {
                        resize_shell.dispatch(Command::SetWindowFocused(true));
                        // Some window managers restore without a distinct
                        // resize notification. Wake content before it can be
                        // interacted with.
                        resize_shell.dispatch(Command::SetWindowVisible(true));
                        presence::report_app_active(resize_window.app_handle());
                    }
                    // Losing focus alone does not make a browser tab
                    // background work: audio and timers must continue. Only
                    // an OS-minimized window hides all content views.
                    tauri::WindowEvent::Focused(false) => {
                        resize_shell.dispatch(Command::SetWindowFocused(false));
                        if resize_window.is_minimized().unwrap_or(false) {
                            resize_shell.dispatch(Command::SetWindowVisible(false));
                        }
                        presence::report_app_active(resize_window.app_handle());
                    }
                    _ => {}
                }
            });

            let panel_url = privileged_app_url(app, &tauri::WebviewUrl::App("panel.html".into()))?;
            let panel_handle = handle.clone();
            #[cfg(target_os = "linux")]
            let panel_registration = global_registration.clone();
            let create_panel = move || -> SetupResult {
                let app = &panel_handle;
                #[cfg(target_os = "linux")]
                let handle = app.clone();
                let window = app.get_webview_window(MAIN_LABEL).ok_or_else(|| std::io::Error::other("launcher owner is unavailable"))?;
                let shutdown = app.state::<ShutdownCoordinator>();
                #[cfg(target_os = "linux")]
                let global_registration = panel_registration;
            if shutdown.terminal_started() {
                return Err(std::io::Error::other(
                    "terminal shutdown started before privileged panel construction",
                )
                .into());
            }
            let panel_builder = tauri::WebviewWindowBuilder::new(
                app,
                overlay::PANEL_LABEL,
                tauri::WebviewUrl::External(
                    tauri::Url::parse(PRIVILEGED_BOOTSTRAP_URL)
                        .map_err(|error| std::io::Error::other(error.to_string()))?,
                ),
            )
            .title("Zephium")
            .incognito(true)
            .devtools(cfg!(debug_assertions))
            .general_autofill_enabled(false)
            .initialization_script_for_all_frames(zephium_engine::PAGE_PRINT_DENY_SCRIPT);
            #[cfg(target_os = "windows")]
            let panel_builder = panel_builder
                .data_directory(privileged_runtime.panel.clone())
                .additional_browser_args(PRIVILEGED_WEBVIEW2_BROWSER_ARGS);
            #[cfg(not(all(unix, not(target_os = "macos"))))]
            let panel_builder = panel_builder.on_download(|_, _| false);
            #[cfg(target_os = "windows")]
            setup_privileged_environments
                .fetch_or(PRIVILEGED_PANEL_ENVIRONMENT, Ordering::Release);
            let panel_window = panel_builder
                .background_throttling(tauri::utils::config::BackgroundThrottlingPolicy::Suspend)
                .on_new_window(|_, _| tauri::webview::NewWindowResponse::Deny)
                .on_web_resource_request(|_, response| {
                    harden_privileged_headers(response.headers_mut())
                })
                .inner_size(overlay::PANEL_SIZE.0, overlay::PANEL_SIZE.1)
                .decorations(false)
                .transparent(true)
                .always_on_top(true)
                .skip_taskbar(true)
                .resizable(false)
                .maximizable(false)
                .fullscreen(false)
                .shadow(true)
                .visible(false)
                .build()?;

            #[cfg(target_os = "macos")]
            {
                if !platform::imp::harden_privileged(&panel_window) {
                    return Err(std::io::Error::other(
                        "required privileged panel WKWebView hardening failed",
                    )
                    .into());
                }
                material::install(&panel_window, true);
            }
            #[cfg(target_os = "windows")]
            {
                platform::imp::round_corners(&panel_window);
                if !platform::imp::harden_privileged(
                    &panel_window,
                    &privileged_runtime.panel,
                    runtime_update_notifier.clone(),
                ) {
                    return Err(std::io::Error::other(
                        "required privileged panel WebView2 hardening failed",
                    )
                    .into());
                }
                platform::imp::apply_material(&panel_window, true);
            }
            #[cfg(all(unix, not(target_os = "macos")))]
            platform::imp::harden_privileged(&panel_window).map_err(|error| {
                std::io::Error::other(format!(
                    "required privileged panel WebKitGTK hardening failed: {error}"
                ))
            })?;

            #[cfg(target_os = "linux")]
            {
                // When portal/direct-X11 registration is unavailable the
                // focused fallback must also close a launcher whose panel,
                // rather than the main window, owns keyboard focus.
                let focused_registration = global_registration.clone();
                let shortcut_app = handle.clone();
                platform::imp::install_shortcuts(
                    &panel_window,
                    window_shortcuts,
                    focused_shortcut_presses,
                    move |id| {
                        if id == linux_shortcut::LAUNCHER_COMMAND_ID
                            && focused_registration.is_live()
                        {
                            return;
                        }
                        let _ = execute_command(&shortcut_app, id);
                    },
                );
            }

            let overlay = overlay::Overlay::new(
                panel_window.clone(),
                cfg!(not(target_os = "linux")).then(|| panel_url.clone()),
            );
            let main_focus_overlay = overlay.clone();
            window.on_window_event(move |event| { if matches!(event, tauri::WindowEvent::Focused(_)) { main_focus_overlay.focus_changed(); } });
            let blur_overlay = overlay.clone();
            panel_window.on_window_event(move |event| {
                if matches!(event,tauri::WindowEvent::Destroyed){blur_overlay.destroyed();}
                match event {
                tauri::WindowEvent::Focused(_) => {
                    blur_overlay.focus_changed();
                    presence::report_app_active(blur_overlay.window_app());
                }
                tauri::WindowEvent::ScaleFactorChanged { .. } => blur_overlay.display_changed(),
                tauri::WindowEvent::CloseRequested { api, .. } if !shutdown_started(blur_overlay.window_app()) => { api.prevent_close(); blur_overlay.hide(); },
                #[cfg(target_os = "windows")]
                tauri::WindowEvent::Destroyed
                    if !platform::imp::remove_privileged_version_observer(overlay::PANEL_LABEL) =>
                {
                    diagnostic!("runtime: panel WebView2 update observer removal was reentrant");
                }
                _ => {}
            }});
            app.manage(overlay);
            #[cfg(all(debug_assertions, target_os = "macos"))]
            log_webview_processes(&window, &panel_window);


                apply_native_theme(app, &APP_STORE.get().and_then(|store| store.app_setting("appearance")).unwrap_or_else(|| "system".into()));
                if shutdown.terminal_started() {
                    return Err(std::io::Error::other("terminal shutdown started before trusted panel navigation").into());
                }
                #[cfg(target_os = "linux")]
                panel_window.navigate(panel_url)?;
                Ok(())
            };
            #[cfg(not(target_os = "linux"))]
            app.manage(overlay::Factory::new(create_panel));
            #[cfg(target_os = "linux")]
            create_panel()?;

            #[cfg(not(target_os = "linux"))]
            {
                app.manage(launcher_trigger::Trigger::default());
                // Honoured for a profile that stored one before the launcher
                // had its own recorder; the keymap no longer rebinds it.
                let keymap_override = keymap
                    .get("launcher.toggle")
                    .filter(|accelerator| {
                        !accelerator.is_empty() && *accelerator != launcher_trigger::DEFAULT
                    })
                    .cloned();
                launcher_trigger::install(&handle, keymap_override);
            }
            #[cfg(target_os = "linux")]
            {
                let shortcut_app = handle.clone();
                let global_shortcuts = linux_global_shortcuts::LinuxGlobalShortcuts::install(
                    &window,
                    linux_launcher_shortcut,
                    global_registration,
                    move |context| {
                        if shutdown_started(&shortcut_app) {
                            return;
                        }
                        if let Some(overlay) = shortcut_app.try_state::<overlay::Overlay>() {
                            overlay.toggle_with_activation(
                                context.activation_token,
                                context.timestamp,
                            );
                        }
                    },
                );
                if !app.manage(global_shortcuts) {
                    return Err(std::io::Error::other(
                        "Linux global-shortcut lifecycle state is already installed",
                    )
                    .into());
                }
            }

            presence::install(&handle, &window);
            memory_pressure::install(&handle);
            presence::report_app_active(&handle);

            let appearance = APP_STORE
                .get()
                .and_then(|store| store.app_setting("appearance"))
                .unwrap_or_else(|| "system".to_owned());
            apply_native_theme(&handle, &appearance);

            // No bundled application asset or page script runs before every
            // mandatory native deny handler has installed. Tauri's trusted
            // IPC/document-start plumbing and app protocols were registered
            // when the blank WebViews were built, so this navigation activates
            // the normal application without rebuilding the native views.
            if shutdown.terminal_started() {
                return Err(std::io::Error::other(
                    "terminal shutdown started before trusted application navigation",
                )
                .into());
            }
            if shutdown.terminal_started() {
                return Err(std::io::Error::other(
                    "terminal shutdown overtook privileged panel navigation",
                )
                .into());
            }
            window.navigate(app_url)?;
            #[cfg(all(unix, not(target_os = "macos")))]
            if std::env::var("ZEPHIUM_NATIVE_STARTUP_PROBE").as_deref() == Ok("1") {
                // CI accepts this marker only after both privileged WebViews
                // exist, their deny-by-default GTK policy/composition has
                // completed, and both trusted navigations were admitted.
                diagnostic!("startup-probe: Linux privileged WebViews are hardened and composed");
            }
            Ok(())
            })();

            contain_tauri_setup_failure(setup_result, |error| {
                request_startup_failure(app.handle(), error)
            })
        })
        .build(tauri::generate_context!())
        .unwrap_or_else(|error| {
            diagnostic!("startup: failed to build Zephium: {error}");
            #[cfg(not(target_os = "linux"))]
            startup_alert::show_blocking(
                startup_alert::StartupProblem::Other,
                &error.to_string(),
            );
            std::process::exit(1);
        });

    #[cfg(target_os = "windows")]
    {
        // The reviewed local Tauri runtime seals and drains its native window
        // registry before `run_return` completes, including dispatcher cycles
        // retained by window listeners. Only then can Environment5 plus the
        // exact process HANDLE authorize deletion of these private UDFs. A
        // failure remains visible in the process status and quarantines the
        // generation for a later verified cleanup pass.
        let exit_code = app.run_return(handle_run_event);
        // Setup can fail after zero or one privileged environment exists.
        // Every attempted build may have created native state before returning
        // an error. Require an exact observer/PID/HANDLE proof for that
        // conservative set; a partial registration therefore mismatches and
        // keeps the runtime generation quarantined.
        let expected_environments = expected_privileged_environment_labels(
            attempted_privileged_environments.load(Ordering::Acquire),
        );
        let process_exit_proven =
            platform::imp::finalize_privileged_environment_observers(&expected_environments);
        let cleanup_succeeded = process_exit_proven && cleanup_privileged_runtime_after_exit();
        if !process_exit_proven {
            diagnostic!(
                "privacy: privileged WebView2 Environment5/PID/HANDLE exit was not proven; leaving this run's UDF generation quarantined"
            );
        }
        updates::finish_after_exit(cleanup_succeeded && exit_code == 0);
        std::process::exit(if cleanup_succeeded || exit_code != 0 {
            exit_code
        } else {
            1
        });
    }

    #[cfg(not(target_os = "windows"))]
    app.run(handle_run_event);
}

#[cfg(test)]
mod frame_sources;

#[cfg(test)]
mod tests {

    use tauri::Url;
    use zephium_ipc::{
        OperationDisposition, OperationOutcome, OperationReason, OperationStatus, SearchAction,
    };

    fn completion(operation_id: &str) -> OperationDisposition {
        OperationDisposition {
            operation_id: operation_id.into(),
            outcome: OperationOutcome::Applied,
            reason: OperationReason::ProfileDeletionCompleted,
        }
    }

    #[tokio::test]
    async fn a_resource_call_waits_briefly_for_admission_instead_of_failing() {
        use std::time::Duration;
        static GATE: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(1);
        let held = super::admit(&GATE, Duration::from_millis(10))
            .await
            .unwrap();
        assert!(super::admit(&GATE, Duration::from_millis(20))
            .await
            .is_none());
        let waiting = tokio::spawn(super::admit(&GATE, Duration::from_secs(5)));
        tokio::time::sleep(Duration::from_millis(20)).await;
        drop(held);
        assert!(waiting.await.unwrap().is_some());
    }

    #[test]
    fn quit_menus_use_the_coordinated_exit_request() {
        let source = include_str!("lib.rs").split("#[cfg(test)]").next().unwrap();
        assert!(!source.contains(".quit()"));
        assert!(!source.contains("PredefinedMenuItem::quit"));
        assert!(source.contains("if id == \"browser.quit\""));
        assert!(source.contains("app.exit(0)"));
    }

    #[test]
    fn export_typescript_bindings() {
        super::specta_builder()
            .export(
                specta_typescript::Typescript::default(),
                "../frame/src/shared/ipc/bindings.ts",
            )
            .expect("export bindings");
    }

    #[test]
    fn native_menu_anchor_accepts_only_finite_window_local_coordinates() {
        assert_eq!(
            super::menu_popup_anchor(12.5, 40.0, 800.0, 600.0),
            Some(tauri::LogicalPosition::new(12.5, 40.0))
        );
        for invalid in [
            super::menu_popup_anchor(-1.0, 40.0, 800.0, 600.0),
            super::menu_popup_anchor(12.5, -1.0, 800.0, 600.0),
            super::menu_popup_anchor(801.0, 40.0, 800.0, 600.0),
            super::menu_popup_anchor(12.5, 601.0, 800.0, 600.0),
            super::menu_popup_anchor(f64::NAN, 40.0, 800.0, 600.0),
            super::menu_popup_anchor(12.5, f64::INFINITY, 800.0, 600.0),
            super::menu_popup_anchor(12.5, 40.0, 0.0, 600.0),
            super::menu_popup_anchor(12.5, 40.0, 800.0, f64::NAN),
        ] {
            assert_eq!(invalid, None);
        }
    }

    #[test]
    fn extension_action_identity_and_anchor_are_exact_and_window_local() {
        assert_eq!(super::fixed_nonzero_hex("0000000000000001"), Some(1));
        for invalid in [
            "0000000000000000",
            "000000000000001",
            "00000000000000001",
            "000000000000000A",
            "000000000000000g",
        ] {
            assert_eq!(super::fixed_nonzero_hex(invalid), None);
        }

        let valid = super::extension_popup_anchor_in_bounds(110.5, 8.0, 28.0, 28.0, 240.0, 800.0)
            .expect("a visible logical button rectangle is accepted");
        assert_eq!(
            valid.rect(),
            zephium_core::geometry::Rect::new(110.5, 8.0, 28.0, 28.0)
        );
        for invalid in [
            super::extension_popup_anchor_in_bounds(-1.0, 8.0, 28.0, 28.0, 240.0, 800.0),
            super::extension_popup_anchor_in_bounds(220.0, 8.0, 28.0, 28.0, 240.0, 800.0),
            super::extension_popup_anchor_in_bounds(8.0, 790.0, 28.0, 28.0, 240.0, 800.0),
            super::extension_popup_anchor_in_bounds(8.0, 8.0, 0.0, 28.0, 240.0, 800.0),
            super::extension_popup_anchor_in_bounds(f64::NAN, 8.0, 28.0, 28.0, 240.0, 800.0),
        ] {
            assert!(invalid.is_none());
        }
    }

    #[test]
    fn add_menu_contains_only_registered_truthful_actions() {
        assert_eq!(super::ADD_MENU_COMMAND_IDS, ["tab.new", "split.choose"]);
        for id in super::ADD_MENU_COMMAND_IDS {
            assert!(
                zephium_core::commands::get(id).is_some(),
                "native add-menu command is not registered: {id}"
            );
        }
    }

    #[test]
    fn add_menu_is_main_only_and_uses_the_bounded_local_anchor() {
        let source = include_str!("lib.rs");
        let command = source
            .split("fn add_menu_popup(")
            .nth(1)
            .expect("add-menu popup command")
            .split("fn setting_get")
            .next()
            .expect("bounded add-menu popup command");

        assert!(command.contains(r#"CallerPolicy::Main, "add_menu_popup""#));
        assert!(command.contains("menu_popup_anchor("));
        assert!(command.contains("build_add_menu(&app, &keymap, can_split)"));
        assert!(command.contains("caller.popup_menu_at(&menu, anchor)"));
        assert!(!command.contains("cursor_position"));
        assert!(!command.contains("popup_menu(&menu)"));
    }

    #[test]
    fn tab_menu_exposes_only_actions_the_shell_can_execute() {
        assert_eq!(
            super::TAB_MENU_ACTION_IDS,
            [
                "tabmenu.reload",
                "tabmenu.duplicate",
                "tabmenu.copyLink",
                "tabmenu.bookmark",
                "tabmenu.keep",
                "tabmenu.unkeep",
                "tabmenu.split",
                "tabmenu.close",
                "tabmenu.closeOthers",
                "tabmenu.closeBelow"
            ]
        );
        // Context-menu actions carry their own target and must never collide
        // with the registry ids the launcher and the keymap may run directly.
        for id in super::TAB_MENU_ACTION_IDS {
            assert!(
                zephium_core::commands::get(id).is_none(),
                "tab-menu action shadows a registered command: {id}"
            );
        }
    }

    #[test]
    fn tab_menu_target_is_armed_once_and_consumed_by_the_first_action() {
        let target = super::TabMenuTarget::default();
        assert_eq!(target.take(), None);

        let id = zephium_core::ids::ItemId::generate();
        assert!(target.arm(id));
        assert_eq!(target.take(), Some(id));
        // A duplicated or delayed menu event cannot replay against a stale tab.
        assert_eq!(target.take(), None);
    }

    #[test]
    fn tab_menu_is_main_only_and_uses_the_bounded_local_anchor() {
        let source = include_str!("lib.rs");
        let command = source
            .split("fn tab_menu_popup(")
            .nth(1)
            .expect("tab menu popup command")
            .split("fn profile_menu_popup")
            .next()
            .expect("bounded tab menu popup command");

        assert!(command.contains(r#"CallerPolicy::Main, "tab_menu_popup""#));
        assert!(command.contains("bounded(&id, MAX_ITEM_ID_BYTES)"));
        assert!(command.contains("ItemId::parse(&id)"));
        assert!(command.contains("menu_popup_anchor("));
        assert!(command.contains("caller.popup_menu_at(&menu, anchor)"));
        assert!(!command.contains("cursor_position"));
        assert!(!command.contains("popup_menu(&menu)"));
    }

    #[test]
    fn the_launcher_may_only_run_registered_commands() {
        use zephium_ipc::SearchAction;

        assert!(super::search_action_in_bounds(&SearchAction::RunCommand {
            id: "tab.new".into(),
        }));
        // The panel is privileged but must not be able to drive a context-menu
        // action against whichever tab main chrome last armed.
        for id in super::TAB_MENU_ACTION_IDS
            .into_iter()
            .chain(super::BOOKMARK_MENU_ACTION_IDS)
        {
            assert!(!super::search_action_in_bounds(&SearchAction::RunCommand {
                id: id.into(),
            }));
        }
    }

    #[test]
    fn split_selection_is_a_delivered_ui_action_not_a_mutation_admission() {
        let source = include_str!("lib.rs");
        let branch = source
            .split(r#"if id == "split.choose" {"#)
            .nth(1)
            .expect("split-selection command branch")
            .split("if let Some(mode)")
            .next()
            .expect("bounded split-selection branch");

        assert!(branch.contains("try_emit_to_privileged(app, MAIN_LABEL, EVENT_UI, &id)"));
        assert!(branch.contains("accepted_ui_operation()"));
        assert!(branch.contains("rejected_operation()"));
        assert!(!branch.contains("dispatch_operation"));
        assert!(!branch.contains("Command::Run"));
    }

    #[test]
    fn svelte_sidebar_routes_add_and_split_selection_through_trusted_native_state() {
        let sidebar = crate::frame_sources::SRC_APP_SHELL_SVELTE;
        let shelf = crate::frame_sources::SRC_FEATURES_DOCK_TOOLSHELF_SVELTE;

        // The tool shelf opens its own tool menu and keeps the native menu on
        // its secondary click. Neither route mutates tabs from the frame.
        assert!(shelf.contains("<Disclosure"));
        assert!(shelf.contains("oncontextmenu={nativeMenu}"));
        assert!(shelf.contains("toolsMenuPopup"));
        assert!(!shelf.contains("tabs.split("));
        assert!(!shelf.contains("addMenuPopup"));
        let native_menu = include_str!("lib.rs")
            .split("fn build_profile_menu(")
            .nth(1)
            .unwrap()
            .split("fn handle_run_event")
            .next()
            .unwrap();
        assert!(native_menu.contains(r#"with_id("split.choose", "Split View…")"#));
        assert!(native_menu.contains(r#"item("tab.new")"#));
        assert!(sidebar.contains(r#"command.id === "split.choose""#));
        assert!(sidebar.contains("splitting = true"));
        assert!(!sidebar.contains("onclick={tabs.open}"));
    }

    #[test]
    fn privileged_html_does_not_add_style_nonces_that_disable_runtime_style_restoration() {
        for html in [
            crate::frame_sources::INDEX_HTML,
            crate::frame_sources::PANEL_HTML,
            crate::frame_sources::ONBOARDING_HTML,
        ] {
            assert!(
                !html.to_ascii_lowercase().contains("<style"),
                "Tauri adds nonces to inline style blocks; this disables the configured unsafe-inline and can strand dropdown pointer locks"
            );
        }
        for css in [
            include_str!("../../frame/src/styles/global.css"),
            include_str!("../../frame/src/styles/panel.css"),
        ] {
            assert!(css.starts_with("@import \"./axes/bootstrap.css\";"));
        }
    }

    #[test]
    fn bootstrap_paint_matches_the_canvas_token() {
        let tokens = crate::frame_sources::SRC_STYLES_TOKENS_CSS;
        let bootstrap = crate::frame_sources::SRC_BOOTSTRAP_CSS;

        let canvas = |block: &str| -> String {
            let rest = &tokens[tokens.find(block).expect("theme block")..];
            let at = rest.find("--color-canvas:").expect("canvas token") + "--color-canvas:".len();
            rest[at..].trim_start()[..7].to_owned()
        };
        let paints: Vec<&str> = bootstrap
            .match_indices("background: #")
            .map(|(at, found)| &bootstrap[at + found.len() - 1..at + found.len() + 6])
            .collect();
        assert_eq!(
            paints,
            [
                canvas("@theme static {"),
                canvas("[data-theme=\"light\"] {")
            ]
        );
    }

    #[test]
    fn runtime_advisory_listener_precedes_bootstrap() {
        // The advisories have no chrome surface at present: the notification
        // dialog left with the sidebar footer and their next home is not
        // decided. The projection ordering it depended on is still a native
        // contract, because runtime status shares the actor-ordered bootstrap
        // that supplies tabs.
        let app = crate::frame_sources::SRC_APP_APP_SVELTE;
        let runtime_listener = app
            .find("const runtimeReady = runtime.init()")
            .expect("runtime projection listener");
        let tab_bootstrap = app
            .find("const tabsReady = tabs.init()")
            .expect("tab bootstrap");

        assert!(runtime_listener < tab_bootstrap);
    }

    #[test]
    fn the_compact_sidebar_keeps_the_presentation_barrier_reachable() {
        let address = crate::frame_sources::SRC_FEATURES_SIDEBAR_ADDRESS_ADDRESSFIELD_SVELTE;
        let rail = crate::frame_sources::SRC_FEATURES_SIDEBAR_TABS_TABRAIL_SVELTE;
        let essentials =
            crate::frame_sources::SRC_FEATURES_SIDEBAR_ESSENTIALS_ESSENTIALSRAIL_SVELTE;

        // The barrier commits and verifies the authoritative host through the
        // address input whenever the active tab presents. Rail width must hide
        // it presentationally; unmounting it would conceal page content for as
        // long as the sidebar stays compact.
        assert!(address.contains("class:sr-only={compact}"));
        assert_eq!(address.matches("data-zephium-address").count(), 1);

        for (name, source) in [("tab rail", rail), ("essentials rail", essentials)] {
            for sentinel in [
                "data-zephium-tab-id",
                "data-zephium-tab-url",
                "data-zephium-projection-revision",
                "data-zephium-tab-label",
            ] {
                assert_eq!(
                    source.matches(sentinel).count(),
                    1,
                    "{name} must own exactly one {sentinel} binding"
                );
            }
            let label = source
                .split("<span data-zephium-tab-label")
                .nth(1)
                .and_then(|rest| rest.split("</span>").next())
                .expect("bounded exact title sentinel");
            assert_eq!(label.matches("{tab.title}").count(), 1);
            assert!(
                label.trim_end().ends_with("{tab.title}"),
                "{name} title must be the sentinel's sole child"
            );
        }
    }

    #[test]
    fn the_sidebar_shape_is_a_bounded_allowlisted_preference() {
        assert!(super::SETTING_KEYS.contains(&"sidebar.mode"));
        assert!(super::setting_value_allowed("sidebar.mode", "default"));
        assert!(super::setting_value_allowed("sidebar.mode", "compact"));
        assert!(!super::setting_value_allowed("sidebar.mode", "collapsed"));
        assert!(!super::setting_value_allowed("sidebar.width", "56"));

        // Compact is a real sidebar width, so the native floor has to admit
        // the rail rather than reject it as out of bounds.
        assert!(super::sidebar_width_in_bounds(56.0));
        assert_eq!(
            super::SIDEBAR_MENU_COMMAND_IDS,
            [
                "nav.back",
                "nav.forward",
                "nav.reload",
                super::SIDEBAR_COMPACT_COMMAND
            ]
        );
        for id in super::SIDEBAR_MENU_COMMAND_IDS {
            assert!(
                zephium_core::commands::get(id).is_some(),
                "collapsed sidebar menu references an unregistered command: {id}"
            );
        }
    }

    #[test]
    fn svelte_tab_rows_open_a_native_context_menu_rather_than_a_dom_one() {
        let list = crate::frame_sources::SRC_FEATURES_SIDEBAR_TABS_TABLIST_SVELTE;
        let state = crate::frame_sources::SRC_DOMAIN_TABS_TABS_SVELTE_TS;

        // A DOM menu cannot paint over a content WebView, so the tab menu must
        // stay native and must carry the exact tab it was opened for.
        assert!(list.contains("event.preventDefault()"));
        assert!(list.contains("tabs.openTabMenu(tab.id, event.clientX, event.clientY)"));
        assert!(state.contains("commands.tabMenuPopup(id, x, y, tabMenuContext(id))"));
    }

    #[test]
    fn interface_languages_match_what_native_accepts() {
        let source = crate::frame_sources::SRC_SHARED_LIB_LOCALE_SVELTE_TS;
        let list = source
            .split("const INTERFACE_LANGUAGES = [")
            .nth(1)
            .and_then(|rest| rest.split(']').next())
            .expect("frame language list");
        let frame: Vec<&str> = list
            .split(',')
            .map(|entry| entry.trim().trim_matches('"'))
            .filter(|entry| !entry.is_empty())
            .collect();
        assert_eq!(frame, zephium_core::preferences::LANGUAGES[1..]);
    }

    #[test]
    fn native_menu_is_never_positioned_from_the_global_pointer() {
        let source = include_str!("lib.rs");
        let command = source
            .split("fn menu_popup(caller:")
            .nth(1)
            .expect("menu popup command")
            .split("fn setting_get")
            .next()
            .expect("bounded menu popup command");
        assert!(command.contains("caller.popup_menu_at(&menu, anchor)"));
        assert!(!command.contains("popup_menu(&menu)"));
        assert!(!command.contains("cursor_position"));
    }

    #[test]
    fn failed_or_missing_webview_delivery_remains_reconcilable_until_acknowledged() {
        let ledger = super::OperationLedger::default();
        let operation_id = "0000000000000001";
        assert!(ledger.reserve(operation_id));
        assert_eq!(ledger.status(operation_id), OperationStatus::Pending);

        assert!(super::record_and_deliver_operation(
            &ledger,
            completion(operation_id),
            |_| false,
        ));
        assert_eq!(
            ledger.status(operation_id),
            OperationStatus::Processed {
                disposition: completion(operation_id),
            }
        );
        assert_eq!(ledger.processed(), vec![completion(operation_id)]);
        assert!(ledger.acknowledge(operation_id));
        assert_eq!(ledger.status(operation_id), OperationStatus::Unknown);
        assert!(ledger.processed().is_empty());
    }

    #[test]
    fn operation_ledger_is_bounded_and_never_evicts_an_accepted_result() {
        let ledger = super::OperationLedger::default();
        for sequence in 1..=super::MAX_OPERATION_LEDGER_ENTRIES {
            assert!(ledger.reserve(&format!("{sequence:016x}")));
        }
        let first = "0000000000000001";
        assert!(ledger.record_disposition(completion(first)));
        assert!(!ledger.reserve("0000000000000401"));
        assert_eq!(ledger.processed(), vec![completion(first)]);
        assert!(ledger.acknowledge(first));
        assert!(ledger.reserve("0000000000000401"));
        assert!(!ledger.record_disposition(completion(first)));
    }

    #[test]
    fn rejected_shell_dispatch_revokes_its_unqueryable_operation_id() {
        let ledger = super::OperationLedger::default();
        let operation_id = "0000000000000001";
        assert!(ledger.reserve(operation_id));

        let admission = super::finish_operation_admission(&ledger, operation_id.to_owned(), false);

        assert!(!admission.accepted);
        assert_eq!(admission.operation_id, None);
        assert_eq!(ledger.status(operation_id), OperationStatus::Unknown);
    }

    #[test]
    fn disconnected_shell_shutdown_is_terminal() {
        let (sender, receiver) = std::sync::mpsc::sync_channel(1);
        drop(sender);
        assert_eq!(
            super::shutdown_receive_outcome(
                receiver.recv_timeout(std::time::Duration::from_millis(1)),
            ),
            zephium_app::ShutdownOutcome::Unclean
        );
    }

    #[test]
    fn unacknowledged_shell_shutdown_deadline_is_terminal() {
        let (_sender, receiver) = std::sync::mpsc::sync_channel(1);
        assert_eq!(
            super::shutdown_receive_outcome(
                receiver.recv_timeout(std::time::Duration::from_millis(1)),
            ),
            zephium_app::ShutdownOutcome::Unclean
        );
    }

    #[test]
    fn every_unproven_desktop_shutdown_is_a_terminal_failure() {
        assert_eq!(
            super::shutdown_exit_code(zephium_app::ShutdownOutcome::Clean),
            0
        );
        assert_eq!(
            super::shutdown_exit_code(zephium_app::ShutdownOutcome::RetryableFailure),
            1
        );
        assert_eq!(
            super::shutdown_exit_code(zephium_app::ShutdownOutcome::Unclean),
            1
        );
    }

    #[test]
    fn startup_failure_is_sticky_even_after_clean_teardown() {
        assert_eq!(
            super::coordinated_exit_code(zephium_app::ShutdownOutcome::Clean, false),
            0
        );
        for outcome in [
            zephium_app::ShutdownOutcome::Clean,
            zephium_app::ShutdownOutcome::RetryableFailure,
            zephium_app::ShutdownOutcome::Unclean,
        ] {
            assert_eq!(super::coordinated_exit_code(outcome, true), 1);
        }
    }

    #[test]
    fn native_fatal_shares_the_orderly_shutdown_single_flight_gate() {
        let fresh = super::ShutdownCoordinator::default();
        assert!(fresh.begin_unrecoverable_native_failure());
        assert!(!fresh.begin_unrecoverable_native_failure());
        assert!(fresh
            .terminal_failure
            .load(std::sync::atomic::Ordering::Acquire));

        let active_shutdown = super::ShutdownCoordinator::default();
        active_shutdown
            .started
            .store(true, std::sync::atomic::Ordering::Release);
        assert!(!active_shutdown.begin_unrecoverable_native_failure());
        assert!(active_shutdown
            .terminal_failure
            .load(std::sync::atomic::Ordering::Acquire));
    }

    #[test]
    fn hard_exit_watchdog_must_be_prepared_before_a_callback_can_arm_it() {
        let coordinator = super::ShutdownCoordinator::default();
        assert!(!coordinator.arm_hard_exit_watchdog());
    }

    #[test]
    fn terminal_start_and_shell_admission_have_one_linearization_order() {
        let terminal_first = super::ShutdownCoordinator::default();
        terminal_first.mark_terminal_start();
        let mut admission_called = false;
        assert!(!terminal_first.try_startup_admission(|| {
            admission_called = true;
            true
        }));
        assert!(!admission_called);

        let admission_first = super::ShutdownCoordinator::default();
        assert!(admission_first.try_startup_admission(|| true));
        admission_first.mark_terminal_start();
        assert!(admission_first.terminal_started());
        assert!(!admission_first.try_startup_admission(|| {
            panic!("terminal publication must suppress every later admission")
        }));
    }

    #[test]
    fn startup_resource_owner_transfers_exactly_once() {
        let owner = super::StartupOwner::<u8>::default();
        let resource = std::sync::Arc::new(7);
        let unrelated = std::sync::Arc::new(7);

        assert!(owner.install(resource.clone()));
        assert!(!owner.install(unrelated.clone()));
        assert!(!owner.transfer_to(&unrelated));
        assert!(owner.transfer_to(&resource));
        assert!(owner.take().is_none());
        assert!(owner.install(resource.clone()));
        assert!(std::sync::Arc::ptr_eq(&owner.take().unwrap(), &resource));
    }

    #[test]
    fn pre_shell_blocker_owner_reaps_the_real_managed_compiler() {
        let root = tempfile::tempdir().unwrap();
        let blocker = super::blocker_service::start_seed_only(root.path()).unwrap();
        let owner = super::StartupBlocker::default();
        assert!(owner.install(blocker.clone()));

        let owned = owner.take().expect("pre-shell blocker owner");
        assert!(std::sync::Arc::ptr_eq(&owned, &blocker));
        assert_eq!(
            zephium_core::ports::blocker::BlockerCompiler::shutdown_until(
                owned.as_ref(),
                std::time::Instant::now() + std::time::Duration::from_secs(2),
            ),
            zephium_core::ports::blocker::BlockerShutdownOutcome::Clean
        );
    }

    #[test]
    fn critical_diagnostics_ignore_stderr_write_failure() {
        struct FailingWriter;

        impl std::io::Write for FailingWriter {
            fn write(&mut self, _buffer: &[u8]) -> std::io::Result<usize> {
                Err(std::io::Error::other("simulated closed stderr"))
            }

            fn flush(&mut self) -> std::io::Result<()> {
                Err(std::io::Error::other("simulated closed stderr"))
            }
        }

        super::write_diagnostic_to(&mut FailingWriter, format_args!("terminal diagnostic"));
    }

    #[test]
    fn presentation_eval_verifies_exact_privileged_dom_before_returning_its_nonce() {
        let source = include_str!("lib.rs");
        let barrier = source
            .split("pub(crate) fn apply_chrome_presentation(")
            .nth(1)
            .expect("privileged presentation barrier")
            .split("fn emit_ui_command")
            .next()
            .expect("bounded privileged presentation barrier");

        let dispatch = barrier
            .find("window.dispatchEvent(new CustomEvent")
            .expect("exact projection dispatch");
        let row = barrier
            .find("[data-zephium-tab-id]")
            .expect("exact tab row verification");
        let title = barrier
            .find("[data-zephium-tab-label]")
            .expect("visible title verification");
        let address = barrier
            .find("[data-zephium-address]")
            .expect("active address verification");
        let new_tab = barrier
            .find("[data-zephium-new-tab]")
            .expect("real New Tab removal verification");
        let layout = barrier
            .find("getBoundingClientRect")
            .expect("synchronous style/layout resolution");
        assert!(dispatch < row && row < title && title < address && address < new_tab);
        assert!(new_tab < layout);
        assert!(barrier.contains("tab.projection_revision"));
        assert!(barrier.contains("zephium-presentation-v1:"));
        assert!(barrier.contains("eval_with_callback"));
    }

    #[test]
    fn svelte_chrome_keeps_the_synchronous_presentation_contract() {
        let entry = crate::frame_sources::SRC_MAIN_TS;
        let shell = crate::frame_sources::SRC_APP_SHELL_SVELTE;
        let list = crate::frame_sources::SRC_FEATURES_SIDEBAR_TABS_TABLIST_SVELTE;
        let row = crate::frame_sources::SRC_FEATURES_SIDEBAR_TABS_TABROW_SVELTE;
        let split_group = crate::frame_sources::SRC_FEATURES_SIDEBAR_TABS_SPLITGROUPROW_SVELTE;
        let address = crate::frame_sources::SRC_FEATURES_SIDEBAR_ADDRESS_ADDRESSFIELD_SVELTE;
        let tabs = crate::frame_sources::SRC_DOMAIN_TABS_TABS_SVELTE_TS;

        let mount = entry.find("mount(App, { target })").expect("Svelte mount");
        let initial_flush = entry
            .find("flushSync();")
            .expect("synchronous initial mount flush");
        assert!(mount < initial_flush);

        assert_eq!(shell.matches("data-zephium-active-tab").count(), 1);
        assert!(shell.contains(r#"data-zephium-active-tab={tabs.activeId() ?? ""}"#));
        assert_eq!(shell.matches("data-zephium-new-tab").count(), 1);
        // The focus cover, when there is one, stands in the same chain ahead
        // of New Tab, so the two never render together.
        assert!(shell.contains("{:else if newTabShown}"));
        assert!(shell
            .contains(r#"if (!tab || tab.url || (tab.content ?? "web") !== "web") return false;"#));
        assert!(shell.contains("data-zephium-surface="));
        assert!(!shell.contains("transition:"));
        assert!(!shell.contains("out:"));

        for sentinel in [
            "data-zephium-tab-id",
            "data-zephium-tab-url",
            "data-zephium-projection-revision",
            "data-zephium-tab-label",
        ] {
            assert_eq!(
                row.matches(sentinel).count(),
                1,
                "tab row must own exactly one {sentinel} binding"
            );
        }
        assert!(row.contains("data-zephium-tab-id={tab.id}"));
        assert!(row.contains(r#"data-zephium-tab-url={tab.url ?? ""}"#));
        assert!(row.contains("data-zephium-projection-revision={tab.projection_revision}"));
        assert!(list.contains("{#each displayUnits as unit, index (unit.key)}"));
        assert!(list.contains("<SplitGroupRow"));
        assert!(split_group.contains("<TabRow"));
        for sentinel in [
            "data-zephium-tab-id",
            "data-zephium-tab-url",
            "data-zephium-projection-revision",
            "data-zephium-tab-label",
        ] {
            assert!(
                !split_group.contains(sentinel),
                "split wrapper must not duplicate the member-owned {sentinel}"
            );
        }
        let label = row
            .split("<span data-zephium-tab-label")
            .nth(1)
            .and_then(|source| source.split("</span>").next())
            .expect("bounded exact title sentinel");
        assert_eq!(label.matches("{tab.title}").count(), 1);
        assert!(
            label.trim_end().ends_with("{tab.title}"),
            "title must be the sentinel's sole child"
        );

        let address_input = address
            .split("<input")
            .nth(1)
            .and_then(|source| source.split("/>").next())
            .expect("bounded address input");
        assert!(address_input.contains("data-zephium-address"));
        assert!(address_input.contains("{value}"));
        assert!(address_input.contains("oninput={handleInput}"));
        assert!(!address_input.contains("bind:value"));
        assert!(!address_input.contains("isTrusted"));

        let presentation = tabs
            .split("events.presentationTab.listen")
            .nth(1)
            .and_then(|source| source.split("\n  ]);").next())
            .expect("bounded presentation listener");
        let flush = presentation
            .find("flushSync(() =>")
            .expect("synchronous presentation flush");
        let admission = presentation
            .find("model.applyPresentation")
            .expect("presentation revision admission");
        let publication = presentation
            .find("publishModelState()")
            .expect("presentation state publication");
        assert!(flush < admission && admission < publication);
        assert!(!presentation.contains("await"));
        assert!(!presentation.contains("requestAnimationFrame"));
    }

    #[test]
    fn tauri_setup_error_is_contained_instead_of_returned_to_native_ready_callback() {
        let mut captured = None;
        let framework_result = super::contain_tauri_setup_failure(
            Err::<(), _>("simulated setup admission failure"),
            |error| captured = Some(error),
        );

        assert!(framework_result.is_ok());
        assert_eq!(captured, Some("simulated setup admission failure"));
    }

    #[test]
    fn every_tauri_config_leaves_privileged_window_construction_to_rust() {
        for (name, source, must_define_windows) in [
            ("base", include_str!("../tauri.conf.json"), true),
            ("macOS", include_str!("../tauri.macos.conf.json"), false),
            ("Windows", include_str!("../tauri.windows.conf.json"), false),
            ("Linux", include_str!("../tauri.linux.conf.json"), false),
        ] {
            let config: serde_json::Value =
                serde_json::from_str(source).expect("valid Tauri configuration JSON");
            let windows = config
                .pointer("/app/windows")
                .and_then(serde_json::Value::as_array);
            let Some(windows) = windows else {
                assert!(
                    !must_define_windows,
                    "{name} must define the inherited main template"
                );
                continue;
            };
            assert_eq!(windows.len(), 1, "{name} must define one main template");
            assert_eq!(
                windows[0].get("label").and_then(serde_json::Value::as_str),
                Some(super::MAIN_LABEL),
                "{name} must preserve the main label when replacing app.windows"
            );
            assert_eq!(
                windows[0]
                    .get("create")
                    .and_then(serde_json::Value::as_bool),
                Some(false),
                "{name} must not auto-create privileged chrome before Rust installs its guards"
            );
        }
    }

    #[test]
    fn onboarding_hands_the_window_to_the_browser_without_showing_it_unready() {
        let source = include_str!("lib.rs");
        let production = source
            .split("#[cfg(test)]")
            .next()
            .expect("desktop production source");
        let hand_over = production
            .split("fn hand_over(&self, window: &WebviewWindow) -> bool {")
            .nth(1)
            .and_then(|body| body.split("fn reveal_browser").next())
            .expect("onboarding handover");
        let hidden = hand_over
            .find("set_chrome_hidden(window, true)")
            .expect("chrome hidden for the handover");
        let navigated = hand_over
            .find("window.navigate(handover.browser.clone())")
            .expect("browser navigation");
        assert!(hidden < navigated);
        assert!(hand_over.contains("self.serves_onboarding(window)"));
        assert!(hand_over.contains("zephium-handover-watchdog"));

        // Shown again only through the gate, once the browser has loaded and
        // acknowledged initialization.
        let gate = production
            .split("fn show_if_ready(&self, window: &WebviewWindow)")
            .nth(1)
            .and_then(|body| body.split("fn serves_onboarding").next())
            .expect("startup gate");
        let facts = gate.find("frontend_ready.load").expect("both facts");
        let reveal = gate
            .find("self.reveal_browser(window)")
            .expect("browser reveal");
        assert!(facts < reveal);
        let recovery_reveal = gate
            .find("set_chrome_hidden(window, false)")
            .expect("recovered chrome reveal");
        assert!(facts < recovery_reveal);

        for command in [
            "onboarding_play_intro",
            "onboarding_finish",
            "essentials_keep",
            "profile_rename",
        ] {
            assert!(
                production.contains(&format!("authorize_onboarding(&caller, \"{command}\")")),
                "{command} must answer only the onboarding page"
            );
        }
    }

    #[test]
    fn linux_global_shortcut_lifecycle_preserves_fallback_and_startup_order() {
        let source = include_str!("lib.rs");
        let production = source
            .split("#[cfg(test)]")
            .next()
            .expect("desktop production source");
        let plugin = production
            .find("let builder = builder.plugin(tauri_plugin_global_shortcut")
            .expect("non-Linux Tauri global-shortcut plugin");
        let plugin_gate = production[..plugin]
            .rfind("#[cfg(not(target_os = \"linux\"))]")
            .expect("Linux exclusion for X11-only Tauri plugin");
        assert!(plugin - plugin_gate < 100);

        let setup = production
            .split(".setup(move |app| {")
            .nth(1)
            .expect("desktop setup hook")
            .split(".build(tauri::generate_context!())")
            .next()
            .expect("bounded desktop setup hook");
        let state = setup
            .find("let global_registration = linux_shortcut_portal::GlobalRegistration::default()")
            .expect("Linux global registration state");
        let focused = setup[state..]
            .find("platform::imp::install_shortcuts")
            .map(|offset| state + offset)
            .expect("focused Linux shortcut fallback");
        let global = setup[focused..]
            .find("LinuxGlobalShortcuts::install")
            .map(|offset| focused + offset)
            .expect("native Linux global-shortcut backend");
        let navigation = setup
            .find("window.navigate(app_url)?")
            .expect("main privileged navigation");
        assert!(state < focused && focused < global && global < navigation);
        assert!(setup.contains("id == linux_shortcut::LAUNCHER_COMMAND_ID"));
        assert_eq!(
            setup
                .matches("linux_shortcut::LinuxLauncherShortcut::parse")
                .count(),
            1,
            "the Linux launcher accelerator must be parsed into one shared IR exactly once"
        );
        let panel_fallback = setup
            .split("let panel_window = panel_builder")
            .nth(1)
            .and_then(|body| body.split("let overlay = overlay::Overlay::new").next())
            .expect("panel construction and hardening");
        assert!(panel_fallback.contains("platform::imp::install_shortcuts("));
        assert!(panel_fallback.contains("global_registration.clone()"));

        let startup_gate = production
            .split("fn show_if_ready(&self, window: &WebviewWindow)")
            .nth(1)
            .and_then(|body| body.split("fn is_visible").next())
            .expect("bounded main-window startup gate");
        let reveal = startup_gate
            .find("show_initialized_main_window(window)")
            .expect("native top-level reveal");
        let mapped = startup_gate
            .find("on_main_window_mapped(window)")
            .expect("post-reveal shortcut startup");
        assert!(reveal < mapped);

        let linux_backend = include_str!("linux_global_shortcuts.rs");
        let portal_start = linux_backend
            .split("pub(crate) fn start_after_main_mapped")
            .nth(1)
            .and_then(|body| body.split("pub(crate) fn shutdown").next())
            .expect("bounded post-map Wayland startup");
        assert!(portal_start.contains("zephium-wayland-shortcut"));
    }

    #[test]
    fn linux_packaging_uses_one_canonical_machine_identity_and_visible_name() {
        let config: serde_json::Value =
            serde_json::from_str(include_str!("../tauri.linux.conf.json"))
                .expect("valid Linux Tauri configuration");
        assert_eq!(
            config
                .get("productName")
                .and_then(serde_json::Value::as_str),
            Some("app.zephium")
        );
        for pointer in [
            "/bundle/linux/deb/desktopTemplate",
            "/bundle/linux/rpm/desktopTemplate",
        ] {
            assert_eq!(
                config.pointer(pointer).and_then(serde_json::Value::as_str),
                Some("linux/zephium.desktop.hbs")
            );
        }
        let template = include_str!("../linux/zephium.desktop.hbs");
        for exact in [
            "Name=Zephium",
            "StartupWMClass=app.zephium",
            "Exec={{exec}} %U",
            "Icon={{icon}}",
        ] {
            assert_eq!(template.lines().filter(|line| *line == exact).count(), 1);
        }
    }

    #[test]
    fn setup_stages_startup_owners_for_lossless_shell_handoff() {
        let source = include_str!("lib.rs");
        let production = source
            .split("#[cfg(test)]")
            .next()
            .expect("desktop production source");
        let setup = production
            .split(".setup(move |app| {")
            .nth(1)
            .expect("desktop setup hook")
            .split(".build(tauri::generate_context!())")
            .next()
            .expect("bounded desktop setup hook");

        assert_eq!(
            production
                .matches(".manage(ShutdownCoordinator::default())")
                .count(),
            1,
            "exactly one coordinator must exist before setup begins"
        );
        assert_eq!(
            production
                .matches(".manage(StartupBlocker::default())")
                .count(),
            1,
            "exactly one temporary blocker owner must predate setup"
        );
        assert_eq!(
            production
                .matches(".manage(StartupStore::default())")
                .count(),
            1,
            "exactly one temporary Store owner must predate setup"
        );
        assert!(setup.contains("let setup_result: SetupResult = (|| {"));
        assert!(setup.contains("contain_tauri_setup_failure(setup_result"));
        assert!(setup.contains("APP_STORE.set(store.clone()).map_err"));

        let watchdog = setup
            .find("shutdown.prepare_hard_exit_watchdog()")
            .expect("pre-armed hard-exit watchdog");
        let storage = setup
            .find("SqliteStore::open(&data_dir)")
            .expect("early storage admission");
        let main_webview = setup
            .find("WebviewWindowBuilder::from_config")
            .expect("main privileged WebView construction");
        assert!(watchdog < storage);
        assert!(storage < main_webview);
        let windows_storage = setup
            .find("SqliteStore::open_with_windows_work_storage")
            .expect("Windows protected Work storage selection");
        assert!(watchdog < windows_storage);
        assert!(windows_storage < main_webview);

        let parent_handle = setup
            .find("let parent = window.window_handle()?.as_raw()")
            .expect("native parent-handle acquisition");
        let parent_gate = setup[..parent_handle]
            .rfind("#[cfg(any(target_os = \"macos\", target_os = \"windows\"))]")
            .expect("native parent-handle platform gate");
        assert!(parent_handle - parent_gate < 100);

        let engine_install = setup
            .find("let engine = Arc::new(zephium_engine::install")
            .expect("native engine installation");
        let startup_engine_owner = setup
            .find("if !startup_engine.install(engine.clone())")
            .expect("temporary pre-shell engine owner");
        let shell_owner = setup
            .find("if !app.manage(shell.clone())")
            .expect("managed shell cleanup owner");
        let engine_transfer = setup
            .find("if !startup_engine.transfer_to(&engine)")
            .expect("exact engine ownership transfer");
        let startup_store_owner = setup
            .find("if !startup_store.install(store.clone())")
            .expect("temporary pre-shell Store owner");
        let store_transfer = setup
            .find("if !startup_store.transfer_to(&store)")
            .expect("exact Store ownership transfer");
        let blocker_start = setup
            .find("let blocker = blocker_service::start_with_updates(&data_dir")
            .expect("managed blocker construction");
        let startup_blocker_owner = setup
            .find("if !startup_blocker.install(blocker.clone())")
            .expect("temporary pre-shell blocker owner");
        let terminal_failure_callback = setup
            .find("let shell_terminal_failure: ShellTerminalFailureCallback")
            .expect("terminal Shell failure callback");
        let shell_spawn = setup
            .find("let shell = match zephium_app::spawn_suspended(")
            .expect("suspended shell actor construction");
        let blocker_transfer = setup
            .find("if !startup_blocker.transfer_to(&blocker)")
            .expect("exact blocker ownership transfer");
        let callback_publication = setup
            .find("slot.set(shell.callback_handle())")
            .expect("native callback ingress publication");
        let actor_admission = setup
            .find("if !shutdown.try_admit_shell(&shell)")
            .expect("linearized post-publication actor admission");
        let post_admission_terminal_gate = setup
            .find("terminal shutdown overtook Shell actor admission")
            .expect("post-admission terminal gate");
        let pre_panel_terminal_gate = setup
            .find("terminal shutdown started before privileged panel construction")
            .expect("pre-panel terminal gate");
        let panel_webview = setup
            .find("let panel_builder = tauri::WebviewWindowBuilder::new")
            .expect("panel privileged WebView construction");
        let pre_navigation_terminal_gate = setup
            .find("terminal shutdown started before trusted application navigation")
            .expect("pre-navigation terminal gate");
        let panel_navigation = setup
            .find("panel_window.navigate(panel_url)?")
            .expect("hardened panel navigation");
        let post_panel_navigation_terminal_gate = setup
            .find("terminal shutdown overtook privileged panel navigation")
            .expect("post-panel-navigation terminal gate");
        let main_navigation = setup
            .find("window.navigate(app_url)?")
            .expect("hardened main navigation");
        let linux_startup_probe = setup
            .find("startup-probe: Linux privileged WebViews are hardened and composed")
            .expect("post-hardening Linux startup probe");
        assert!(engine_install < startup_engine_owner);
        assert!(startup_store_owner < engine_install);
        assert!(startup_engine_owner < shell_owner);
        assert!(shell_owner < engine_transfer);
        assert!(blocker_start < startup_blocker_owner);
        assert!(startup_blocker_owner < terminal_failure_callback);
        assert!(terminal_failure_callback < shell_spawn);
        assert!(shell_spawn < shell_owner);
        assert!(shell_owner < store_transfer);
        assert!(shell_owner < blocker_transfer);
        assert!(engine_transfer < blocker_transfer);
        assert!(blocker_transfer < store_transfer);
        assert!(engine_transfer < actor_admission);
        assert!(blocker_transfer < actor_admission);
        assert!(store_transfer < actor_admission);
        assert!(engine_transfer < callback_publication);
        assert!(blocker_transfer < callback_publication);
        assert!(store_transfer < callback_publication);
        assert!(callback_publication < actor_admission);
        assert!(actor_admission < post_admission_terminal_gate);
        assert!(post_admission_terminal_gate < pre_panel_terminal_gate);
        assert!(pre_panel_terminal_gate < panel_webview);
        assert!(actor_admission < panel_webview);
        assert!(shell_owner < panel_webview);
        assert!(panel_webview < pre_navigation_terminal_gate);
        assert!(panel_navigation < pre_navigation_terminal_gate);
        assert!(panel_webview < panel_navigation);
        assert!(panel_navigation < post_panel_navigation_terminal_gate);
        assert!(post_panel_navigation_terminal_gate < main_navigation);
        assert!(panel_navigation < main_navigation);
        assert!(main_navigation < linux_startup_probe);
        assert!(!setup.contains("app.manage(shutdown.clone())"));
        for forbidden_wait in [
            "wait_for_startup_until(",
            "retry_startup_until(",
            "settle_startup_until(",
        ] {
            assert!(
                !setup.contains(forbidden_wait),
                "Tauri setup must not wait on native settlement: {forbidden_wait}"
            );
        }
        assert!(setup.contains("failure.worker_cleanup_proven()"));
        assert!(setup.contains("request_shell_terminal_failure("));
        assert!(setup.contains("&terminal_failure_shutdown"));

        let terminal_failure_diagnostic = production
            .split("fn request_shell_terminal_failure(")
            .nth(1)
            .expect("Shell terminal-failure dispatcher")
            .split("fn request_orderly_terminal_failure(")
            .next()
            .expect("bounded Shell terminal-failure dispatcher");
        assert!(terminal_failure_diagnostic
            .contains("runtime: terminal application shell failure: {error}"));
        assert!(terminal_failure_diagnostic.contains("coordinator.signal_terminal_start(app)"));
        assert!(terminal_failure_diagnostic.contains("request_orderly_terminal_failure(app)"));
        let terminal_signal = terminal_failure_diagnostic
            .find("coordinator.signal_terminal_start(app)")
            .unwrap();
        let terminal_diagnostic = terminal_failure_diagnostic
            .find("write_diagnostic(format_args!")
            .unwrap();
        let terminal_request = terminal_failure_diagnostic
            .find("request_orderly_terminal_failure(app)")
            .unwrap();
        assert!(terminal_signal < terminal_diagnostic);
        assert!(terminal_diagnostic < terminal_request);

        let terminal_marker = production
            .split("fn mark_terminal_start(&self)")
            .nth(1)
            .expect("terminal-start linearization")
            .split("fn try_admit_shell(&self")
            .next()
            .expect("bounded terminal-start linearization");
        let terminal_lock = terminal_marker.find("startup_admission").unwrap();
        let terminal_publish = terminal_marker
            .find("self.terminal_started.store(true, Ordering::Release)")
            .unwrap();
        assert!(terminal_lock < terminal_publish);
        let shell_admission = production
            .split("fn try_startup_admission(&self")
            .nth(1)
            .expect("Shell-admission linearization")
            .split("fn try_admit_shell(&self")
            .next()
            .expect("bounded Shell-admission linearization");
        let admission_lock = shell_admission.find("startup_admission").unwrap();
        let terminal_observation = shell_admission
            .find("self.terminal_started.load(Ordering::Acquire)")
            .unwrap();
        let gate_admission = shell_admission.find("admit()").unwrap();
        assert!(admission_lock < terminal_observation);
        assert!(terminal_observation < gate_admission);
        let handle_admission = production
            .split("fn try_admit_shell(&self")
            .nth(1)
            .expect("Handle startup-gate admission")
            .split("fn prepare_hard_exit_watchdog")
            .next()
            .expect("bounded Handle startup-gate admission");
        assert!(handle_admission.contains("self.try_startup_admission"));
        assert!(handle_admission.contains("shell.admit_startup()"));

        let shell_manage_failure = &setup[shell_owner..engine_transfer];
        assert!(shell_manage_failure.contains("shutdown.request_terminal_startup_failure("));
        assert!(shell_manage_failure.contains("Some(shell)"));

        let run_event = production
            .split("fn handle_run_event(")
            .nth(1)
            .expect("desktop run-event handler")
            .split("pub fn run()")
            .next()
            .expect("bounded desktop run-event handler");
        assert!(
            !run_event.contains("std::process::exit"),
            "native exit callbacks must return through Tauri's event loop"
        );
        assert!(
            !production.contains("std::process::exit(70)"),
            "terminal native failures must preserve App/run_return finalization"
        );
    }

    #[test]
    fn pre_shell_failure_reaps_store_native_and_blocker_under_one_deadline() {
        let source = include_str!("lib.rs");
        let production = source
            .split("#[cfg(test)]")
            .next()
            .expect("desktop production source");
        let claim = production
            .split("impl TerminalStartupResources {")
            .nth(1)
            .expect("terminal startup owner claim")
            .split("impl ClaimedTerminalStartupResources {")
            .next()
            .expect("bounded terminal startup owner claim");
        let request = production
            .split("fn request_terminal_startup_failure(")
            .nth(1)
            .expect("terminal startup cleanup request")
            .split("fn request_unrecoverable_native_failure(")
            .next()
            .expect("bounded terminal startup cleanup request");
        let cleanup = production
            .split("fn cleanup_pre_shell_resources_until(")
            .nth(1)
            .expect("pre-shell cleanup implementation")
            .split("fn cleanup_supplemental_startup_resources(")
            .next()
            .expect("bounded pre-shell cleanup implementation");

        let engine_take = claim
            .find("engine_owner.and_then(|owner| owner.take())")
            .expect("engine temporary-owner take");
        let blocker_take = claim
            .find("blocker_owner.and_then(|owner| owner.take())")
            .expect("blocker temporary-owner take");
        let store_take = claim
            .find("store_owner.and_then(|owner| owner.take())")
            .expect("Store temporary-owner take");
        assert!(engine_take < blocker_take);
        assert!(blocker_take < store_take);

        let single_flight = request
            .find("self.started.swap(true, Ordering::AcqRel)")
            .expect("startup cleanup single-flight gate");
        let owner_claim = request
            .find(".claim();")
            .expect("atomic temporary-owner claim");
        let deadline = request
            .find("let deadline = std::time::Instant::now()")
            .expect("single cleanup deadline");
        assert!(single_flight < owner_claim);
        assert!(owner_claim < deadline);

        let store = cleanup
            .find("store.shutdown_until(deadline)")
            .expect("storage durability barrier");
        let native = cleanup
            .find("engine.shutdown(Box::new")
            .expect("native teardown admission");
        let blocker = cleanup
            .find("blocker.shutdown_until(deadline)")
            .expect("blocker worker teardown");
        let native_wait = cleanup
            .find("native_wait.recv_timeout(remaining)")
            .expect("native teardown completion");

        assert!(store < native);
        assert!(native < blocker);
        assert!(blocker < native_wait);
        assert_eq!(request.matches("PRE_SHELL_CLEANUP_TIMEOUT").count(), 1);
        assert_eq!(
            cleanup.matches("std::panic::catch_unwind").count(),
            3,
            "Store, engine admission, and blocker cleanup must be independently panic-contained"
        );

        let startup_failure = production
            .split("fn request_startup_failure(")
            .nth(1)
            .expect("startup failure dispatcher")
            .split("fn request_pre_shell_startup_failure_with_blocker(")
            .next()
            .expect("bounded startup failure dispatcher");
        assert!(startup_failure.contains("try_state::<StartupBlocker>()"));
        assert!(startup_failure.contains("try_state::<StartupEngine>()"));
        assert!(startup_failure.contains("try_state::<StartupStore>()"));
        assert!(!startup_failure.contains("|blocker| blocker.take()"));
        assert!(!startup_failure.contains("|owner| owner.take()"));

        let run_event = production
            .split("fn handle_run_event(")
            .nth(1)
            .expect("desktop run-event handler")
            .split("pub fn run()")
            .next()
            .expect("bounded desktop run-event handler");
        assert!(run_event.contains("try_state::<StartupBlocker>()"));
        assert!(run_event.contains("try_state::<StartupEngine>()"));
        assert!(run_event.contains("try_state::<StartupStore>()"));
        assert!(!run_event.contains("|blocker| blocker.take()"));
        assert!(!run_event.contains("|owner| owner.take()"));
    }

    #[test]
    fn windows_finalization_requires_every_privileged_environment_build_attempted() {
        assert!(super::expected_privileged_environment_labels(0).is_empty());
        assert_eq!(
            super::expected_privileged_environment_labels(super::PRIVILEGED_MAIN_ENVIRONMENT),
            vec![super::MAIN_LABEL]
        );
        assert_eq!(
            super::expected_privileged_environment_labels(
                super::PRIVILEGED_MAIN_ENVIRONMENT | super::PRIVILEGED_PANEL_ENVIRONMENT
            ),
            vec![super::MAIN_LABEL, super::overlay::PANEL_LABEL]
        );
    }

    #[test]
    fn vendored_runtime_drains_privileged_webviews_before_windows_process_proof() {
        let runtime = include_str!("../../vendor/tauri-runtime-wry/src/lib.rs");
        let request_exit = runtime
            .split_once("Message::RequestExit(code) => {")
            .expect("Tauri runtime exit handler")
            .1
            .split_once("Message::Window(id, WindowMessage::Close)")
            .expect("bounded Tauri runtime exit handler")
            .0;
        let drain = request_exit
            .find("drain_windows_for_exit(&windows, &window_id_map)")
            .expect("terminal native-window drain");
        let loop_exit = request_exit
            .find("*control_flow = ControlFlow::ExitWithCode(code)")
            .expect("event-loop exit transition");
        assert!(drain < loop_exit);
        assert!(!request_exit.contains("*control_flow = ControlFlow::Exit;"));

        let wrapper_teardown = runtime
            .split_once("fn teardown_for_exit(mut self)")
            .expect("ordered native WindowWrapper teardown")
            .1
            .split_once("fn drain_windows_for_exit")
            .expect("bounded native WindowWrapper teardown")
            .0;
        let child_webviews = wrapper_teardown
            .find("self.webviews.clear()")
            .expect("child WebView release");
        let parent_window = wrapper_teardown
            .find("self.inner.take()")
            .expect("Tao parent release");
        assert!(child_webviews < parent_window);

        let desktop = include_str!("lib.rs");
        let run_return = desktop
            .find("app.run_return(handle_run_event)")
            .expect("returning Windows event loop");
        let process_proof = desktop
            .find("platform::imp::finalize_privileged_environment_observers")
            .expect("privileged Environment5/PID/HANDLE proof");
        let private_cleanup = desktop
            .find("process_exit_proven && cleanup_privileged_runtime_after_exit()")
            .expect("proof-gated privileged UDF cleanup");
        assert!(run_return < process_proof);
        assert!(process_proof < private_cleanup);
    }

    #[test]
    fn shutdown_authorizes_only_the_exact_correlated_exit_code() {
        assert!(!super::exit_request_is_authorized(
            None,
            super::NO_AUTHORIZED_EXIT_CODE
        ));
        assert!(!super::exit_request_is_authorized(
            Some(0),
            super::NO_AUTHORIZED_EXIT_CODE
        ));
        assert!(!super::exit_request_is_authorized(Some(0), 1));
        assert!(!super::exit_request_is_authorized(None, 1));
        assert!(super::exit_request_is_authorized(Some(0), 0));
        assert!(super::exit_request_is_authorized(Some(1), 1));
    }

    #[test]
    fn privileged_webview2_args_keep_smartscreen_and_process_sandboxes_enabled() {
        assert_eq!(
            super::PRIVILEGED_WEBVIEW2_BROWSER_ARGS,
            "--disable-features=msWebOOUI,msPdfOOUI"
        );
        let args = super::PRIVILEGED_WEBVIEW2_BROWSER_ARGS.to_ascii_lowercase();
        assert!(!args.contains("smartscreen"));
        assert!(!args.contains("no-sandbox"));
    }

    #[test]
    fn every_privileged_webview_installs_the_print_guard_at_construction() {
        let source = include_str!("lib.rs");
        let all_frames_call = [
            ".initialization_script_for_all_frames(",
            "zephium_engine::PAGE_PRINT_DENY_SCRIPT)",
        ]
        .concat();
        assert_eq!(source.matches(&all_frames_call).count(), 2);
    }

    #[test]
    fn privileged_webview2_frame_uri_is_bounded_before_rust_allocation() {
        let source = include_str!("platform/windows.rs");
        let frame_handler = source
            .split("core.add_FrameNavigationStarting")
            .nth(1)
            .expect("privileged frame-navigation handler")
            .split("core18.add_LaunchingExternalUriScheme")
            .next()
            .expect("bounded privileged frame-navigation handler");
        assert!(frame_handler.contains("args.SetCancel(true)?"));
        assert!(frame_handler.contains("take_pwstr_bounded("));
        assert!(frame_handler.contains("PAGE_URL_UTF16_LIMIT"));
        assert!(frame_handler.contains("PAGE_URL_UTF8_LIMIT"));
        assert!(!frame_handler.contains("take_pwstr(uri)"));
    }

    #[test]
    fn privileged_webview2_save_as_is_cancelled_before_dialog_suppression() {
        let source = include_str!("platform/windows.rs");
        let handler = source
            .split("let core25 = core.cast::<ICoreWebView2_25>()?")
            .nth(1)
            .expect("mandatory privileged SaveAsUIShowing interface")
            .split("// Tauri's navigation hook")
            .next()
            .expect("bounded privileged SaveAsUIShowing handler");
        assert!(handler.contains("core25.add_SaveAsUIShowing"));
        let cancel = handler.find("args.SetCancel(true)?").unwrap();
        let suppress = handler
            .find("args.SetSuppressDefaultDialog(true)?")
            .unwrap();
        assert!(cancel < suppress);
    }

    #[test]
    fn privileged_webview2_autofill_surfaces_are_read_back_disabled() {
        let source = include_str!("platform/windows.rs");
        let policy = source
            .split("let settings4 = settings.cast::<ICoreWebView2Settings4>()?")
            .nth(1)
            .expect("mandatory privileged Settings4 policy")
            .split("let mut token = 0_i64")
            .next()
            .expect("pre-navigation privileged Settings4 policy");
        for required in [
            "SetIsPasswordAutosaveEnabled(false)?",
            "SetIsGeneralAutofillEnabled(false)?",
            "IsPasswordAutosaveEnabled(&mut password_autosave_enabled)?",
            "IsGeneralAutofillEnabled(&mut general_autofill_enabled)?",
            "password_autosave_enabled.as_bool() || general_autofill_enabled.as_bool()",
            "E_ACCESSDENIED",
        ] {
            assert!(
                policy.contains(required),
                "privileged WebView2 autofill postcondition lost invariant: {required}"
            );
        }
    }

    #[test]
    fn privileged_webviews_cannot_navigate_remote() {
        #[cfg(not(target_os = "windows"))]
        let bundled = "tauri://localhost/index.html";
        #[cfg(target_os = "windows")]
        let bundled = "http://tauri.localhost/index.html";
        assert!(super::ui_navigation_allowed(&Url::parse(bundled).unwrap()));
        assert!(super::ui_navigation_allowed(
            &Url::parse(super::PRIVILEGED_BOOTSTRAP_URL).unwrap()
        ));
        for rejected in [
            "about:blank#fragment",
            "about:blank?query",
            "about:srcdoc",
            "about:config",
        ] {
            assert!(!super::ui_navigation_allowed(
                &Url::parse(rejected).unwrap()
            ));
        }
        assert!(!super::ui_navigation_allowed(
            &Url::parse("tauri://user@localhost/index.html").unwrap()
        ));
        assert!(!super::ui_navigation_allowed(
            &Url::parse("http://tauri.localhost:8080/index.html").unwrap()
        ));
        assert!(!super::ui_navigation_allowed(
            &Url::parse("https://example.com/").unwrap()
        ));
        assert!(!super::ui_navigation_allowed(
            &Url::parse("data:text/html,hostile").unwrap()
        ));
        assert!(!super::ui_navigation_allowed(
            &Url::parse("file:///etc/passwd").unwrap()
        ));
    }

    #[test]
    fn macos_bundle_declares_page_media_usage_without_enabling_authority() {
        let plist = include_str!("../Info.plist");
        assert!(plist.contains("<key>NSCameraUsageDescription</key>"));
        assert!(plist.contains("<key>NSMicrophoneUsageDescription</key>"));
        assert!(
            plist.contains("Zephium uses the camera only when you allow a website to access it.")
        );
        assert!(plist
            .contains("Zephium uses the microphone only when you allow a website to access it."));
    }

    #[test]
    fn macos_bundle_explains_download_folder_consent() {
        let plist = include_str!("../Info.plist");
        for key in [
            "NSDownloadsFolderUsageDescription",
            "NSDesktopFolderUsageDescription",
            "NSDocumentsFolderUsageDescription",
        ] {
            assert!(plist.contains(&format!("<key>{key}</key>")), "{key}");
        }
    }

    #[test]
    fn production_csp_remains_fail_closed_for_privileged_chrome() {
        let config: serde_json::Value =
            serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
        let csp = config
            .pointer("/app/security/csp")
            .and_then(serde_json::Value::as_object)
            .expect("production CSP object");

        let directive = |name: &str| {
            csp.get(name)
                .and_then(serde_json::Value::as_str)
                .unwrap_or_else(|| panic!("missing production CSP directive: {name}"))
        };

        assert_eq!(directive("default-src"), "'self'");
        assert_eq!(directive("script-src"), "'self'");
        assert_eq!(
            directive("connect-src"),
            "ipc: http://ipc.localhost",
            "privileged IPC origins must remain exact"
        );
        assert_eq!(directive("style-src"), "'self' 'unsafe-inline'");
        assert_eq!(
            directive("img-src"),
            "'self' data: zephium-media: http://zephium-media.localhost"
        );
        assert_eq!(directive("font-src"), "'self' data:");
        // Same-origin workers only: the diagram layout engine runs off the page thread.
        assert_eq!(directive("worker-src"), "'self'");

        for name in [
            "child-src",
            "frame-src",
            "media-src",
            "object-src",
            "base-uri",
            "form-action",
            "frame-ancestors",
        ] {
            assert_eq!(directive(name), "'none'", "{name} must remain denied");
        }

        for forbidden in ["'unsafe-eval'", "'unsafe-inline'", "data:", "blob:"] {
            assert!(
                !directive("script-src")
                    .split_ascii_whitespace()
                    .any(|source| source == forbidden),
                "production script-src admitted forbidden source {forbidden}"
            );
        }
    }

    #[test]
    fn linux_initialized_reveal_never_uses_recursive_tao_visibility() {
        let desktop = include_str!("lib.rs");
        let reveal = desktop
            .split("fn show_initialized_main_window")
            .nth(1)
            .and_then(|source| source.split("const NO_AUTHORIZED_EXIT_CODE").next())
            .expect("main-window reveal seam");
        let linux = reveal
            .split("#[cfg(all(unix, not(target_os = \"macos\")))]")
            .nth(1)
            .and_then(|source| {
                source
                    .split("#[cfg(not(all(unix, not(target_os = \"macos\"))))]")
                    .next()
            })
            .expect("Linux reveal branch");
        assert!(linux.contains("platform::imp::show_initialized_top_level(window)"));
        assert!(!linux.contains("window.show()"));

        let adapter = include_str!("platform/linux.rs");
        let top_level = adapter
            .split("fn show_top_level_only")
            .nth(1)
            .and_then(|source| source.split("fn remove_from_actual_parent").next())
            .expect("native top-level-only reveal");
        assert!(top_level.contains("gtk_window.show()"));
        assert!(!top_level.contains("show_all"));
    }

    #[test]
    fn privileged_target_is_resolved_then_checked_before_bootstrap_navigation() {
        #[cfg(not(target_os = "windows"))]
        let base = Url::parse("tauri://localhost").unwrap();
        #[cfg(target_os = "windows")]
        let base = Url::parse("http://tauri.localhost").unwrap();

        let index =
            super::resolve_privileged_target(&base, &tauri::WebviewUrl::App("index.html".into()))
                .unwrap();
        assert_eq!(index.path(), "/browser.html");

        let nested = super::resolve_privileged_target(
            &base,
            &tauri::WebviewUrl::App("settings/index.html".into()),
        )
        .unwrap();
        assert_eq!(nested.path(), "/settings/index.html");

        assert!(super::resolve_privileged_target(
            &base,
            &tauri::WebviewUrl::External(Url::parse(super::PRIVILEGED_BOOTSTRAP_URL).unwrap()),
        )
        .is_err());
        assert!(super::resolve_privileged_target(
            &base,
            &tauri::WebviewUrl::External(Url::parse("https://example.com/").unwrap()),
        )
        .is_err());
    }

    #[test]
    fn caller_authorization_is_explicit_and_deny_by_default() {
        use super::CallerPolicy::{Both, Main, Panel};

        assert!(super::caller_allowed(Main, "main"));
        assert!(!super::caller_allowed(Main, "panel"));
        assert!(!super::caller_allowed(Main, "content"));

        assert!(super::caller_allowed(Panel, "panel"));
        assert!(!super::caller_allowed(Panel, "main"));
        assert!(!super::caller_allowed(Panel, "content"));

        assert!(super::caller_allowed(Both, "main"));
        assert!(super::caller_allowed(Both, "panel"));
        assert!(!super::caller_allowed(Both, "content"));
        assert!(!super::caller_allowed(Both, "Main"));
        assert!(!super::caller_allowed(Both, ""));
    }

    #[test]
    fn blocker_diagnostics_are_main_only_and_have_no_profile_selector() {
        let source = include_str!("lib.rs");
        let command = source
            .split("async fn blocker_status(")
            .nth(1)
            .expect("blocker status command")
            .split("#[tauri::command]")
            .next()
            .expect("bounded blocker status command");
        assert!(command.contains("CallerPolicy::Main"));
        assert!(command.contains("focused_content_policy_status()"));
        assert!(command.contains("spawn_blocking"));
        assert!(command.contains("BLOCKER_STATUS_QUERY_TIMEOUT"));
        assert!(command.contains("BLOCKER_STATUS_QUERY_IN_FLIGHT"));
        assert!(!command.contains("ProfileId"));
        assert!(!command.contains("profile:"));

        let delivery = source
            .split("Projection::BlockerStatus(status)")
            .nth(1)
            .expect("blocker projection route")
            .split("Projection::OperationProcessed")
            .next()
            .expect("bounded blocker projection route");
        assert!(delivery.contains("MAIN_LABEL"));
        assert!(delivery.contains("EVENT_BLOCKER_STATUS"));
        assert!(!delivery.contains("PANEL_LABEL"));

        for command_name in [
            "blocker_set_enabled",
            "blocker_retry",
            "blocker_refresh_sources",
        ] {
            let command = source
                .split(&format!("fn {command_name}("))
                .nth(1)
                .expect("blocker mutation command")
                .split("#[tauri::command]")
                .next()
                .expect("bounded blocker mutation command");
            assert!(command.contains("CallerPolicy::Main"));
            assert!(command.contains("dispatch_operation"));
            assert!(!command.contains("ProfileId"));
            assert!(!command.contains("profile:"));
        }
    }

    #[test]
    fn blocker_status_single_flight_flag_has_scope_owned_reset() {
        static FLAG: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
        assert!(FLAG
            .compare_exchange(
                false,
                true,
                std::sync::atomic::Ordering::AcqRel,
                std::sync::atomic::Ordering::Acquire,
            )
            .is_ok());
        {
            let _reset = super::AtomicFlagReset(&FLAG);
            assert!(FLAG.load(std::sync::atomic::Ordering::Acquire));
        }
        assert!(!FLAG.load(std::sync::atomic::Ordering::Acquire));
    }

    #[test]
    fn numeric_inputs_must_be_finite_and_bounded() {
        assert!(super::point_in_bounds(0.0, -1.0));
        assert!(super::point_in_bounds(
            super::MAX_WINDOW_COORDINATE,
            -super::MAX_WINDOW_COORDINATE
        ));
        assert!(!super::point_in_bounds(f64::NAN, 0.0));
        assert!(!super::point_in_bounds(0.0, f64::INFINITY));
        assert!(!super::point_in_bounds(
            super::MAX_WINDOW_COORDINATE + 1.0,
            0.0
        ));

        assert!(super::sidebar_width_in_bounds(super::MIN_SIDEBAR_WIDTH));
        assert!(super::sidebar_width_in_bounds(super::MAX_SIDEBAR_WIDTH));
        assert!(!super::sidebar_width_in_bounds(
            super::MIN_SIDEBAR_WIDTH - 1.0
        ));
        assert!(!super::sidebar_width_in_bounds(f64::NAN));
    }

    #[test]
    fn work_pane_inputs_are_finite_positive_and_trusted() {
        let rect = |x, y, width, height| {
            super::work_pane_rect(zephium_ipc::WorkPaneRect {
                x,
                y,
                width,
                height,
            })
        };
        assert_eq!(
            rect(10.0, 20.0, 640.0, 480.0),
            Some(zephium_core::geometry::Rect::new(10.0, 20.0, 640.0, 480.0))
        );
        assert_eq!(rect(-5.0, 20.0, 640.0, 480.0).map(|r| r.x), Some(-5.0));
        assert!(rect(10.0, 20.0, 0.0, 480.0).is_none());
        assert!(rect(10.0, 20.0, 640.0, -1.0).is_none());
        assert!(rect(f64::NAN, 20.0, 640.0, 480.0).is_none());
        assert!(rect(10.0, 20.0, f64::INFINITY, 480.0).is_none());
        assert!(rect(10.0, 20.0, super::MAX_WINDOW_COORDINATE + 1.0, 480.0).is_none());

        assert!(super::trusted_web_url("https://example.com/path?q=1"));
        assert!(super::trusted_web_url("http://example.com"));
        assert!(!super::trusted_web_url("javascript:alert(1)"));
        assert!(!super::trusted_web_url("https://user:pw@example.com"));
        assert!(!super::trusted_web_url("file:///etc/hosts"));
        assert!(!super::trusted_web_url("https://example.com/\u{7}"));
        assert!(!super::trusted_web_url(&format!(
            "https://example.com/{}",
            "a".repeat(8192)
        )));
    }

    #[test]
    fn text_and_tagged_inputs_are_bounded() {
        assert!(super::bounded("abc", 3));
        assert!(!super::bounded("abcd", 3));
        assert!(super::setting_value_allowed("appearance", "system"));
        assert!(super::setting_value_allowed("appearance", "light"));
        assert!(super::setting_value_allowed("appearance", "dark"));
        assert!(!super::setting_value_allowed("appearance", "sepia"));
        assert!(!super::setting_value_allowed("keymap", "dark"));

        assert!(super::search_action_in_bounds(&SearchAction::ActivateTab {
            id: "a".repeat(super::MAX_ITEM_ID_BYTES),
        }));
        assert!(!super::search_action_in_bounds(
            &SearchAction::ActivateTab {
                id: "a".repeat(super::MAX_ITEM_ID_BYTES + 1),
            }
        ));
        assert!(super::search_action_in_bounds(&SearchAction::OpenUrl {
            url: "a".repeat(super::MAX_NAVIGATION_INPUT_BYTES),
        }));
        assert!(!super::search_action_in_bounds(&SearchAction::OpenUrl {
            url: "a".repeat(super::MAX_NAVIGATION_INPUT_BYTES + 1),
        }));
        assert!(!super::search_action_in_bounds(&SearchAction::RunCommand {
            id: "a".repeat(super::MAX_COMMAND_ID_BYTES + 1),
        }));
    }

    #[test]
    fn appearance_codes_remain_explicit() {
        assert_eq!(super::appearance_code("system"), super::APPEARANCE_SYSTEM);
        assert_eq!(super::appearance_code("light"), super::APPEARANCE_LIGHT);
        assert_eq!(super::appearance_code("dark"), super::APPEARANCE_DARK);
        assert_eq!(super::appearance_code("invalid"), super::APPEARANCE_SYSTEM);
    }
}
