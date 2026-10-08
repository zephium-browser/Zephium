mod log;
mod sidebar_resize;
pub use log::redirect_stderr;

use std::cell::RefCell;
use std::ptr::null_mut;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use objc2::rc::Retained;
use objc2::runtime::{NSObject, ProtocolObject};
use objc2::{define_class, msg_send, ClassType, MainThreadOnly};
use objc2_foundation::{
    ns_string, MainThreadMarker, NSArray, NSBundle, NSObjectProtocol, NSProcessInfo, NSString,
    NSURL,
};
use objc2_web_kit::{
    WKFrameInfo, WKMediaCaptureType, WKOpenPanelParameters, WKPermissionDecision, WKSecurityOrigin,
    WKUIDelegate, WKWebView,
};
use tauri::{Manager as _, WebviewWindow};

use zephium_app::{
    ChromePresentation, ChromePresentationCallback, ChromePresentationDispatch, PresentationChrome,
    SharedChrome,
};
use zephium_core::geometry::Size;
use zephium_core::ports::chrome::{Chrome, ChromeFrame};
use zephium_engine::MainThreadDispatch;

static CHROME_GENERATION: AtomicU64 = AtomicU64::new(0);
static NEXT_CHROME_GENERATION: AtomicU64 = AtomicU64::new(1);
static NEXT_PRIVILEGED_DELEGATE_GENERATION: AtomicU64 = AtomicU64::new(1);
static CHROME_ORIGIN: Mutex<ChromeOrigin> = Mutex::new(ChromeOrigin::empty());
const MAX_PRIVILEGED_UI_DELEGATES: usize = 2;

struct ChromeViewState {
    generation: u64,
    webview: Retained<WKWebView>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct ChromeOrigin {
    generation: u64,
    x: f64,
    y: f64,
}

impl ChromeOrigin {
    const fn empty() -> Self {
        Self {
            generation: 0,
            x: 0.0,
            y: 0.0,
        }
    }

    fn install(&mut self, generation: u64) {
        self.generation = generation;
        self.x = 0.0;
        self.y = 0.0;
    }

    fn publish_if_current(
        &mut self,
        published_generation: u64,
        adapter_generation: u64,
        x: f64,
        y: f64,
    ) -> bool {
        if adapter_generation == 0
            || published_generation != adapter_generation
            || self.generation != adapter_generation
        {
            return false;
        }
        self.x = x;
        self.y = y;
        true
    }

    fn clear_if_current(&mut self, generation: u64) {
        if self.generation == generation {
            *self = Self::empty();
        }
    }

    fn translate(self, x: f64, y: f64) -> (f64, f64) {
        (x + self.x, y + self.y)
    }
}

struct PrivilegedDelegateState {
    label: String,
    generation: u64,
    _delegate: Retained<PrivilegedUIDelegate>,
}

thread_local! {
    // Objective-C UI objects stay owned and dropped on the main thread. The
    // published generation lets queued cross-thread layout work reject a
    // destroyed or replaced view without ever transporting a raw pointer.
    static CHROME_VIEW: RefCell<Option<ChromeViewState>> = const { RefCell::new(None) };

    // WKWebView's UIDelegate property is weak. Retain one deny-only delegate
    // for each exact privileged-view generation and release it after that
    // window is destroyed. The label/generation pair prevents a late destroy
    // event from releasing a replacement view's delegate.
    static PRIVILEGED_UI_DELEGATES: RefCell<Vec<PrivilegedDelegateState>> = const { RefCell::new(Vec::new()) };
}

/// Reject an unsupported system WebKit before Tauri creates any privileged or
/// content WKWebView, and report an outdated one as an update advisory.
///
/// Safari's marketing version alone is not sufficient evidence: the Safari
/// bundle and the WebKit framework loaded for `WKWebView` should report the
/// same canonical build. On Sonoma and Sequoia that binds Apple's Safari
/// security release to the embedder; Tahoe's WebKit comes from the OS update.
pub fn enforce_runtime_security_floor(
) -> Result<zephium_core::runtime_security::RuntimeSecurityAdvisories, String> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs());

    let reported_os = NSProcessInfo::processInfo().operatingSystemVersion();
    let major = u32::try_from(reported_os.majorVersion)
        .map_err(|_| "NSProcessInfo reported an invalid macOS major version".to_string())?;
    let minor = u32::try_from(reported_os.minorVersion)
        .map_err(|_| "NSProcessInfo reported an invalid macOS minor version".to_string())?;
    let patch = u32::try_from(reported_os.patchVersion)
        .map_err(|_| "NSProcessInfo reported an invalid macOS patch version".to_string())?;
    let operating_system = format!("{major}.{minor}.{patch}");

    // Safari is a protected system application at this canonical path on all
    // admitted macOS lines. Do not resolve an arbitrary third-party bundle
    // with the same identifier from Launch Services.
    let safari = NSBundle::bundleWithPath(ns_string!("/Applications/Safari.app"))
        .ok_or_else(|| "cannot open the system Safari bundle".to_string())?;
    require_bundle_identifier(&safari, "com.apple.Safari", "Safari")?;
    let safari_version = bundle_string(&safari, ns_string!("CFBundleShortVersionString"))?;
    let safari_build = bundle_string(&safari, ns_string!("CFBundleVersion"))?;

    // `bundleForClass` identifies the framework actually supplying WKWebView,
    // unlike locating a possibly unloaded bundle by identifier.
    // SAFETY: `WKWebView::class()` is a live Objective-C class from the linked
    // WebKit framework, exactly as required by `bundleForClass:`.
    let webkit = unsafe { NSBundle::bundleForClass(WKWebView::class()) };
    require_bundle_identifier(&webkit, "com.apple.WebKit", "WKWebView")?;
    let webkit_build = bundle_string(&webkit, ns_string!("CFBundleVersion"))?;

    zephium_core::macos::assess_runtime(
        &operating_system,
        &safari_version,
        &safari_build,
        &webkit_build,
        now,
    )
    .map_err(|error| {
        format!(
            "macOS {operating_system} with Safari {safari_version} is not supported ({error}). Zephium needs macOS Sonoma 14 or later with Safari 26 or later"
        )
    })
}

fn require_bundle_identifier(
    bundle: &NSBundle,
    expected: &str,
    description: &str,
) -> Result<(), String> {
    let identifier = bundle
        .bundleIdentifier()
        .ok_or_else(|| format!("{description} bundle has no identifier"))?;
    if identifier.to_string() != expected {
        return Err(format!(
            "{description} bundle has unexpected identifier {identifier:?}"
        ));
    }
    Ok(())
}

fn bundle_string(bundle: &NSBundle, key: &NSString) -> Result<String, String> {
    let info = bundle
        .infoDictionary()
        .ok_or_else(|| "bundle has no information dictionary".to_string())?;
    let value = info
        .objectForKey(key)
        .ok_or_else(|| format!("bundle information dictionary has no {key}"))?;
    let value = value
        .downcast::<NSString>()
        .map_err(|_| format!("bundle information dictionary {key} is not a string"))?;
    Ok(value.to_string())
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "ZephiumPrivilegedUIDelegate"]
    #[ivars = ()]
    struct PrivilegedUIDelegate;

    unsafe impl NSObjectProtocol for PrivilegedUIDelegate {}

    unsafe impl WKUIDelegate for PrivilegedUIDelegate {
        #[unsafe(method(webView:requestMediaCapturePermissionForOrigin:initiatedByFrame:type:decisionHandler:))]
        fn deny_media_capture(
            &self,
            _webview: &WKWebView,
            _origin: &WKSecurityOrigin,
            _frame: &WKFrameInfo,
            _capture_type: WKMediaCaptureType,
            decision_handler: &block2::DynBlock<dyn Fn(WKPermissionDecision)>,
        ) {
            decision_handler.call((WKPermissionDecision::Deny,));
        }

        #[unsafe(method(webView:requestDeviceOrientationAndMotionPermissionForOrigin:initiatedByFrame:decisionHandler:))]
        fn deny_device_motion(
            &self,
            _webview: &WKWebView,
            _origin: &WKSecurityOrigin,
            _frame: &WKFrameInfo,
            decision_handler: &block2::DynBlock<dyn Fn(WKPermissionDecision)>,
        ) {
            decision_handler.call((WKPermissionDecision::Deny,));
        }

        #[unsafe(method(webView:runOpenPanelWithParameters:initiatedByFrame:completionHandler:))]
        fn deny_file_chooser(
            &self,
            _webview: &WKWebView,
            _parameters: &WKOpenPanelParameters,
            _frame: &WKFrameInfo,
            completion_handler: &block2::DynBlock<dyn Fn(*mut NSArray<NSURL>)>,
        ) {
            completion_handler.call((null_mut(),));
        }
    }
);

impl PrivilegedUIDelegate {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(());
        unsafe { msg_send![super(this), init] }
    }
}

/// Pointer coords arrive chrome-relative; the shell expects window coords.
/// Here the chrome webview sits at the sidebar rect (Windows/Linux keep it
/// full-window, where this is the identity).
pub fn to_window(x: f64, y: f64) -> (f64, f64) {
    CHROME_ORIGIN
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .translate(x, y)
}

pub fn init(window: &WebviewWindow) -> bool {
    crate::material::install(window, false);
    super::window_controls::install(window);
    if !harden_privileged(window) {
        return false;
    }
    let installed = Arc::new(AtomicU64::new(0));
    let completed = installed.clone();
    let scheduled = window.with_webview(move |webview| {
        if MainThreadMarker::new().is_none() {
            return;
        }
        let generation = NEXT_CHROME_GENERATION.fetch_add(1, Ordering::AcqRel);
        if generation == 0 {
            // An epoch wrap would make stale work indistinguishable from new
            // work. It is unreachable in practice, but fail closed anyway.
            return;
        }
        // SAFETY: Tauri supplied a live WKWebView pointer for the duration of
        // this main-thread callback. Retaining it makes the lifetime explicit;
        // it is later released on this same thread by `clear_chrome_view`.
        let Some(webview) = (unsafe { Retained::retain(webview.inner().cast::<WKWebView>()) })
        else {
            return;
        };
        CHROME_VIEW.with(|slot| {
            *slot.borrow_mut() = Some(ChromeViewState {
                generation,
                webview,
            });
        });
        CHROME_GENERATION.store(generation, Ordering::Release);
        CHROME_ORIGIN
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .install(generation);
        completed.store(generation, Ordering::Release);
    });
    if scheduled.is_err() {
        return false;
    }
    let generation = installed.load(Ordering::Acquire);
    if generation == 0 {
        return false;
    }
    window.on_window_event(move |event| {
        if matches!(event, tauri::WindowEvent::Destroyed) {
            clear_chrome_view(generation);
        }
    });
    true
}

/// Hides or shows the chrome WebView while its window stays on screen, so the
/// window shows only its material, never a document that is still loading.
pub fn set_chrome_hidden(window: &WebviewWindow, hidden: bool) -> bool {
    window
        .with_webview(move |webview| {
            // SAFETY: Tauri supplies a live WKWebView pointer for the duration
            // of this main-thread callback.
            let webkit: &WKWebView = unsafe { &*webview.inner().cast() };
            webkit.setHidden(hidden);
        })
        .is_ok()
}

pub fn harden_privileged(window: &WebviewWindow) -> bool {
    let installed = Arc::new(AtomicU64::new(0));
    let completed = installed.clone();
    let label = window.label().to_owned();
    let installed_label = label.clone();
    let scheduled = window.with_webview(move |webview| {
        let webkit: &objc2_web_kit::WKWebView = unsafe { &*webview.inner().cast() };
        let configuration = unsafe { webkit.configuration() };
        let data_store = unsafe { configuration.websiteDataStore() };
        if unsafe { data_store.isPersistent() } {
            crate::write_diagnostic(format_args!(
                "security: privileged WKWebView data store is persistent"
            ));
            return;
        }
        // Never depend on feature/default interactions for release inspector
        // exposure. Debug builds intentionally retain the developer workflow.
        unsafe {
            webkit.setInspectable(cfg!(debug_assertions));
            // Native force-click/long-press link previews bypass browser
            // chrome's origin-labelled popup policy.
            webkit.setAllowsLinkPreview(false);
        }
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        let can_install = PRIVILEGED_UI_DELEGATES.with(|delegates| {
            let delegates = delegates.borrow();
            delegates.len() < MAX_PRIVILEGED_UI_DELEGATES
                && !delegates.iter().any(|state| state.label == installed_label)
        });
        if !can_install {
            crate::write_diagnostic(format_args!(
                "security: privileged WKWebView delegate registry rejected label {installed_label}"
            ));
            return;
        }
        let generation = NEXT_PRIVILEGED_DELEGATE_GENERATION.fetch_add(1, Ordering::AcqRel);
        if generation == 0 {
            return;
        }
        let delegate = PrivilegedUIDelegate::new(mtm);
        unsafe {
            webkit.setUIDelegate(Some(ProtocolObject::from_ref(&*delegate)));
        }
        PRIVILEGED_UI_DELEGATES.with(|delegates| {
            delegates.borrow_mut().push(PrivilegedDelegateState {
                label: installed_label,
                generation,
                _delegate: delegate,
            });
        });
        completed.store(generation, Ordering::Release);
    });
    if scheduled.is_err() {
        return false;
    }
    let generation = installed.load(Ordering::Acquire);
    if generation == 0 {
        return false;
    }
    window.on_window_event(move |event| {
        if matches!(event, tauri::WindowEvent::Destroyed) {
            clear_privileged_delegate(&label, generation);
        }
    });
    true
}

fn clear_privileged_delegate(label: &str, generation: u64) {
    if MainThreadMarker::new().is_none() || generation == 0 {
        return;
    }
    PRIVILEGED_UI_DELEGATES.with(|delegates| {
        delegates
            .borrow_mut()
            .retain(|state| state.label != label || state.generation != generation);
    });
}

pub fn make_chrome(window: &WebviewWindow, dispatch: MainThreadDispatch) -> SharedChrome {
    Arc::new(ChromeAdapter {
        window: window.clone(),
        dispatch,
        generation: CHROME_GENERATION.load(Ordering::Acquire),
        layout: Arc::new(Mutex::new(ChromeLayoutState::default())),
    })
}

struct ChromeAdapter {
    window: WebviewWindow,
    dispatch: MainThreadDispatch,
    generation: u64,
    layout: Arc<Mutex<ChromeLayoutState>>,
}

#[derive(Default)]
struct ChromeLayoutState {
    pending: Option<ChromeFrame>,
    applied: Option<ChromeFrame>,
    scheduled: bool,
}

impl Chrome for ChromeAdapter {
    fn position(&self, frame: ChromeFrame) -> bool {
        let generation = self.generation;
        if generation == 0 || CHROME_GENERATION.load(Ordering::Acquire) != generation {
            return false;
        }
        let published = CHROME_GENERATION.load(Ordering::Acquire);
        if !CHROME_ORIGIN
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .publish_if_current(published, generation, frame.rect.x, frame.rect.y)
        {
            return false;
        }
        let schedule = {
            let mut layout = self
                .layout
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if layout.pending == Some(frame)
                || (layout.pending.is_none() && layout.applied == Some(frame))
            {
                return true;
            }
            layout.pending = Some(frame);
            if layout.scheduled {
                false
            } else {
                layout.scheduled = true;
                true
            }
        };
        if schedule
            && !dispatch_chrome_layout(self.dispatch.clone(), generation, self.layout.clone())
        {
            let mut layout = self
                .layout
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            layout.pending = None;
            layout.scheduled = false;
            crate::write_diagnostic(format_args!(
                "layout: macOS chrome frame was rejected by the main event loop"
            ));
            return false;
        }
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

fn dispatch_chrome_layout(
    dispatch: MainThreadDispatch,
    generation: u64,
    layout: Arc<Mutex<ChromeLayoutState>>,
) -> bool {
    let next_dispatch = dispatch.clone();
    dispatch(Box::new(move || {
        let frame = layout
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .pending
            .take();
        if let Some(frame) = frame {
            if let Some(held) =
                with_chrome_view(generation, |webview| set_chrome_frame(webview, frame))
            {
                layout
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .applied = Some(frame);
                if held {
                    settle_held_chrome(generation, layout.clone(), frame);
                }
            }
        }

        let reschedule = {
            let mut state = layout
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if state.pending.is_some() {
                true
            } else {
                state.scheduled = false;
                false
            }
        };
        if reschedule && !dispatch_chrome_layout(next_dispatch, generation, layout.clone()) {
            let mut state = layout
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            state.pending = None;
            state.scheduled = false;
            crate::write_diagnostic(format_args!(
                "layout: coalesced macOS chrome frame was rejected by the main event loop"
            ));
        }
    }))
}

impl Drop for ChromeAdapter {
    fn drop(&mut self) {
        let generation = self.generation;
        if generation == 0 {
            return;
        }
        let _ = (self.dispatch)(Box::new(move || clear_chrome_view(generation)));
    }
}

fn with_chrome_view<T>(generation: u64, f: impl FnOnce(&WKWebView) -> T) -> Option<T> {
    MainThreadMarker::new()?;
    if generation == 0 || CHROME_GENERATION.load(Ordering::Acquire) != generation {
        return None;
    }
    let webview = CHROME_VIEW.with(|slot| {
        let state = slot.try_borrow().ok()?;
        let state = state.as_ref()?;
        if state.generation != generation {
            return None;
        }
        Some(state.webview.clone())
    })?;

    // Never hold the RefCell borrow while entering AppKit or caller code:
    // either may synchronously re-enter window teardown/layout. The local
    // retain keeps the exact generation alive without fabricating a borrow.
    if CHROME_GENERATION.load(Ordering::Acquire) != generation || webview.window().is_none() {
        return None;
    }
    let result = f(&webview);

    // Native work may have synchronously destroyed or replaced the view.
    // Results such as content geometry are valid only for the still-published
    // exact generation and an attached surface.
    if CHROME_GENERATION.load(Ordering::Acquire) != generation || webview.window().is_none() {
        return None;
    }
    let still_published = CHROME_VIEW.with(|slot| {
        slot.try_borrow()
            .ok()
            .and_then(|state| state.as_ref().map(|state| state.generation))
            .is_some_and(|published| published == generation)
    });
    still_published.then_some(result)
}

pub fn publish_sidebar_resize_revision(revision: u64) -> bool {
    sidebar_resize::publish_revision(revision)
}

pub async fn configure_sidebar_resize(
    window: &WebviewWindow,
    width: f64,
    enabled: bool,
    revision: u64,
    shell: zephium_app::Handle,
) -> bool {
    if !sidebar_resize::publish_revision(revision) {
        return false;
    }
    let generation = CHROME_GENERATION.load(Ordering::Acquire);
    let sender = window.clone();
    let (send, receive) = tokio::sync::oneshot::channel();
    let scheduled = window.run_on_main_thread(move || {
        let installed = with_chrome_view(generation, |view| {
            // SAFETY: the published chrome retains this exact main-thread WK view.
            let Some(view) =
                (unsafe { Retained::retain(view as *const WKWebView as *mut WKWebView) })
            else {
                return false;
            };
            let commit = std::rc::Rc::new(move |width, revision| {
                if CHROME_GENERATION.load(Ordering::Acquire) != generation
                    || !sidebar_resize::publish_revision(revision)
                {
                    return;
                }
                // A guide release is a snap to a new shape: the content
                // travels there, which also covers the chrome's repaint.
                if shell.dispatch(zephium_app::Command::SetSidebarWidth(width, true)) {
                    crate::emit_to_privileged(
                        sender.app_handle(),
                        crate::MAIN_LABEL,
                        "zephium:sidebar-width-selected",
                        &serde_json::json!({"width": width, "revision": revision}),
                    );
                }
            });
            sidebar_resize::configure(&view, generation, width, enabled, revision, commit)
        })
        .unwrap_or(false);
        let _ = send.send(installed);
    });
    if scheduled.is_err() {
        return false;
    }
    receive.await.unwrap_or(false)
}

fn clear_chrome_view(generation: u64) {
    if MainThreadMarker::new().is_none()
        || generation == 0
        || CHROME_GENERATION.load(Ordering::Acquire) != generation
    {
        return;
    }
    CHROME_VIEW.with(|slot| {
        let matches = slot
            .borrow()
            .as_ref()
            .is_some_and(|state| state.generation == generation);
        if matches {
            // Unpublish first so work queued concurrently cannot enter with a
            // generation whose owning retain is about to be released.
            if CHROME_GENERATION
                .compare_exchange(generation, 0, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                CHROME_ORIGIN
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .clear_if_current(generation);
                slot.borrow_mut().take();
                sidebar_resize::dispose(Some(generation));
            }
        }
    });
}

// inner_size() reflects the shrunk chrome webview, so the window's real
// content area is read from the webview's superview instead.
pub fn content_size(_window: &WebviewWindow) -> Option<Size> {
    let generation = CHROME_GENERATION.load(Ordering::Acquire);
    with_chrome_view(generation, |webview| {
        let superview = unsafe { webview.superview() }?;
        let bounds = superview.bounds();
        Some(Size::new(bounds.size.width, bounds.size.height))
    })?
}

/// How long a narrowing chrome keeps its width: the content's slide
/// (--motion-page), and a frame of margin so the slide has visibly ended.
const CHROME_HOLD: std::time::Duration = std::time::Duration::from_millis(420);

/// Narrows a chrome that `set_chrome_frame` held wide for a journey, once the
/// journey is over — unless a newer frame has been applied in the meantime,
/// which is then the one that stands.
fn settle_held_chrome(generation: u64, layout: Arc<Mutex<ChromeLayoutState>>, frame: ChromeFrame) {
    let Ok(when) = dispatch2::DispatchTime::try_from(CHROME_HOLD) else {
        return;
    };
    let _ = dispatch2::DispatchQueue::main().after(when, move || {
        let current = {
            let state = layout
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            state.pending.is_none() && state.applied == Some(frame)
        };
        if current {
            let settled = ChromeFrame {
                travel: false,
                ..frame
            };
            let _ = with_chrome_view(generation, |webview| set_chrome_frame(webview, settled));
        }
    });
}

/// Applies `frame`, and reports whether it held the chrome at its current
/// width because the frame narrows it on a journey.
fn set_chrome_frame(view: &WKWebView, frame: ChromeFrame) -> bool {
    use objc2_app_kit::NSAutoresizingMaskOptions as Mask;
    use objc2_foundation::{NSPoint, NSRect, NSSize};

    let Some(sv) = (unsafe { view.superview() }) else {
        return false;
    };
    let h = sv.bounds().size.height;
    let r = frame.rect;
    // Height follows the window; width either follows (fill) or stays fixed
    // and pinned left. AppKit applies this in the window's own layout pass.
    let mask = if frame.fill_width {
        Mask::ViewWidthSizable | Mask::ViewHeightSizable
    } else {
        Mask::ViewMaxXMargin | Mask::ViewHeightSizable
    };
    let current = view.frame();
    // Narrowing on a journey keeps the old width: the page slides over what
    // the chrome draws there, and cutting it away first shows a cropped frame.
    let hold = frame.travel
        && !frame.fill_width
        && current.origin.x == r.x
        && current.size.width > r.width;
    let width = if hold { current.size.width } else { r.width };
    let f = NSRect::new(
        NSPoint::new(r.x, h - r.y - r.height),
        NSSize::new(width, r.height),
    );
    view.setTranslatesAutoresizingMaskIntoConstraints(true);
    view.setAutoresizingMask(mask);
    if current.origin.x != f.origin.x
        || current.origin.y != f.origin.y
        || current.size.width != f.size.width
        || current.size.height != f.size.height
    {
        view.setFrame(f);
    }
    sidebar_resize::refresh();
    hold
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chrome_layout_mailbox_applies_one_latest_frame_per_main_loop_turn() {
        type Task = Box<dyn FnOnce() + Send + 'static>;
        let tasks = Arc::new(Mutex::new(Vec::<Task>::new()));
        let queued = tasks.clone();
        let dispatch: MainThreadDispatch = Arc::new(move |task| {
            queued
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .push(task);
            true
        });
        let frame = ChromeFrame {
            rect: zephium_core::geometry::Rect::new(8.0, 8.0, 240.0, 700.0),
            fill_width: false,
            travel: false,
        };
        let layout = Arc::new(Mutex::new(ChromeLayoutState {
            pending: Some(frame),
            applied: None,
            scheduled: true,
        }));

        assert!(dispatch_chrome_layout(dispatch, 0, layout.clone()));
        let task = tasks
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .pop()
            .unwrap();
        task();

        let layout = layout
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        // Generation zero has no live chrome view. A failed native lookup
        // must not poison duplicate suppression with a frame never applied.
        assert_eq!(layout.applied, None);
        assert!(layout.pending.is_none());
        assert!(!layout.scheduled);
    }

    #[test]
    fn privileged_ui_delegate_class_registers() {
        let _ = <PrivilegedUIDelegate as objc2::ClassType>::class();
    }

    #[test]
    fn chrome_origin_rejects_stale_generations_and_publishes_coordinates_as_a_pair() {
        let mut origin = ChromeOrigin::empty();
        origin.install(7);
        assert!(origin.publish_if_current(7, 7, 11.0, 29.0));
        assert_eq!(origin.translate(2.0, 3.0), (13.0, 32.0));

        assert!(!origin.publish_if_current(8, 7, 101.0, 202.0));
        assert!(!origin.publish_if_current(7, 6, 303.0, 404.0));
        assert_eq!(origin.translate(2.0, 3.0), (13.0, 32.0));
        origin.clear_if_current(6);
        assert_eq!(origin.generation, 7);
        origin.clear_if_current(7);
        assert_eq!(origin, ChromeOrigin::empty());
    }

    #[test]
    #[ignore = "requires a release-qualified system Safari/WebKit pair; the explicit native security probe owns this environment-dependent gate"]
    fn system_safari_matches_the_framework_owning_wkwebview() {
        let safari = NSBundle::bundleWithPath(ns_string!("/Applications/Safari.app"))
            .expect("system Safari bundle");
        require_bundle_identifier(&safari, "com.apple.Safari", "Safari").unwrap();
        let safari_build = bundle_string(&safari, ns_string!("CFBundleVersion")).unwrap();

        // SAFETY: `WKWebView::class()` is a live linked Objective-C class.
        let webkit = unsafe { NSBundle::bundleForClass(WKWebView::class()) };
        require_bundle_identifier(&webkit, "com.apple.WebKit", "WKWebView").unwrap();
        let webkit_build = bundle_string(&webkit, ns_string!("CFBundleVersion")).unwrap();

        assert_eq!(safari_build, webkit_build);
    }
}

thread_local! {
    static KEY_MONITOR: RefCell<Option<Retained<objc2::runtime::AnyObject>>> =
        const { RefCell::new(None) };
}

/// Keyboard-only commands (Select Tab 1-9) have no menu item to carry their
/// key equivalent, so a local monitor matches them before the focused web
/// view sees the keystroke. Only the main window's keys are considered; the
/// launcher panel keeps its own.
pub fn install_key_monitor(
    window: &WebviewWindow,
    table: Arc<std::sync::RwLock<Vec<(zephium_core::accelerator::Accelerator, &'static str)>>>,
    on: impl Fn(&str) + 'static,
) {
    use block2::RcBlock;
    use objc2_app_kit::{NSEvent, NSEventMask, NSEventModifierFlags};
    use std::ptr::NonNull;

    let Ok(main_window) = window.ns_window() else {
        return;
    };
    let main_window = main_window as usize;
    let handler = RcBlock::new(move |event: NonNull<NSEvent>| -> *mut NSEvent {
        // SAFETY: AppKit passes a live event for the duration of the block.
        let event_ref = unsafe { event.as_ref() };
        let Some(mtm) = MainThreadMarker::new() else {
            return event.as_ptr();
        };
        let in_main = event_ref
            .window(mtm)
            .is_some_and(|w| Retained::as_ptr(&w) as usize == main_window);
        if !in_main {
            return event.as_ptr();
        }
        let flags = event_ref.modifierFlags();
        let character = event_ref
            .charactersByApplyingModifiers(NSEventModifierFlags::empty())
            .map(|text| text.to_string().to_lowercase());
        let key_code = event_ref.keyCode();
        let hit = table
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .find(|(accelerator, _)| {
                flags.contains(NSEventModifierFlags::Command) == accelerator.meta
                    && flags.contains(NSEventModifierFlags::Control) == accelerator.ctrl
                    && flags.contains(NSEventModifierFlags::Option) == accelerator.alt
                    && flags.contains(NSEventModifierFlags::Shift) == accelerator.shift
                    && key_matches(accelerator.key, character.as_deref(), key_code)
            })
            .map(|(_, id)| *id);
        match hit {
            Some(id) => {
                if !event_ref.isARepeat() {
                    on(id);
                }
                std::ptr::null_mut()
            }
            None => event.as_ptr(),
        }
    });
    // SAFETY: the block returns the event it was given, or null to consume it,
    // as AppKit requires.
    let monitor = unsafe {
        NSEvent::addLocalMonitorForEventsMatchingMask_handler(NSEventMask::KeyDown, &handler)
    };
    KEY_MONITOR.with(|slot| *slot.borrow_mut() = monitor);
}

/// Printable keys compare by the character they type unmodified, so the
/// layout decides; the rest compare by macOS virtual key code.
fn key_matches(key: zephium_core::accelerator::Key, character: Option<&str>, code: u16) -> bool {
    use zephium_core::accelerator::Key;
    if let Some(expected) = key.character() {
        return character.is_some_and(|typed| typed.chars().eq(std::iter::once(expected)));
    }
    let expected: u16 = match key {
        Key::Tab => 48,
        Key::Space => 49,
        Key::Enter => 36,
        Key::Escape => 53,
        Key::Backspace => 51,
        Key::Delete => 117,
        Key::Left => 123,
        Key::Right => 124,
        Key::Down => 125,
        Key::Up => 126,
        Key::Home => 115,
        Key::End => 119,
        Key::PageUp => 116,
        Key::PageDown => 121,
        Key::Function(number) => match number {
            1 => 122,
            2 => 120,
            3 => 99,
            4 => 118,
            5 => 96,
            6 => 97,
            7 => 98,
            8 => 100,
            9 => 101,
            10 => 109,
            11 => 103,
            12 => 111,
            _ => return false,
        },
        _ => return false,
    };
    code == expected
}
