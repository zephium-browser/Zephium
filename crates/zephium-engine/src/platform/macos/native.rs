use std::panic::AssertUnwindSafe;

use objc2::{
    define_class, msg_send,
    rc::Retained,
    runtime::{NSObject, ProtocolObject},
    DefinedClass, MainThreadOnly,
};
use objc2_foundation::{MainThreadMarker, NSObjectProtocol, NSString};
use objc2_web_kit::{
    WKContentWorld, WKScriptMessage, WKScriptMessageHandler, WKUserContentController,
};
use zephium_core::ports::engine::{Partition, ScriptPrincipal};

// Phase 0a exposes the native bridge primitive before Phase 1 owns and retains
// registrations in the host. Keep its dormant pieces warning-clean until that
// integration seam is connected.
#[allow(dead_code)]
const PRINCIPAL_MESSAGE_UTF16_LIMIT: usize = 64 * 1_024;
#[allow(dead_code)]
const PRINCIPAL_MESSAGE_UTF8_LIMIT: usize = 64 * 1_024;
const PRINCIPAL_WORLD_PREFIX: &str = "zephium-principal-";
const PRINCIPAL_HANDLER_PREFIX: &str = "zephiumPrincipal_";
const PRINCIPAL_TOKEN_MAX_BYTES: usize = "userscript-".len() + 32;

/// A platform-local security principal for isolated content.
///
/// Names are derived only from the typed core principal. The variant remains
/// part of every native name, so equal userscript and extension ULIDs cannot
/// alias. JavaScript never supplies or selects this identity.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(crate) struct PrincipalContentIdentity {
    principal: ScriptPrincipal,
    world_name: Box<str>,
    handler_name: Box<str>,
}

impl PrincipalContentIdentity {
    pub(crate) fn new(principal: ScriptPrincipal) -> Self {
        let token = principal_token(principal);
        Self {
            principal,
            world_name: format!("{PRINCIPAL_WORLD_PREFIX}{token}").into(),
            handler_name: format!("{PRINCIPAL_HANDLER_PREFIX}{token}").into(),
        }
    }

    pub(crate) fn principal(&self) -> ScriptPrincipal {
        self.principal
    }

    pub(crate) fn world_name(&self) -> &str {
        &self.world_name
    }

    #[allow(dead_code)]
    pub(crate) fn handler_name(&self) -> &str {
        &self.handler_name
    }
}

fn principal_token(principal: ScriptPrincipal) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let (prefix, bytes) = match principal {
        ScriptPrincipal::Userscript(id) => ("userscript-", id.bytes()),
        ScriptPrincipal::Extension(id) => ("extension-", id.bytes()),
    };
    // Encode the fixed 128-bit identity ourselves instead of depending on a
    // Display implementation at an Objective-C/JavaScript naming boundary.
    let mut token = String::with_capacity(PRINCIPAL_TOKEN_MAX_BYTES);
    token.push_str(prefix);
    for byte in bytes {
        token.push(char::from(HEX[usize::from(byte >> 4)]));
        token.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    token
}

#[derive(Clone, Debug, Eq, PartialEq)]
#[allow(dead_code)]
pub(crate) struct PrincipalContentMessage {
    identity: PrincipalContentIdentity,
    body: String,
}

#[allow(dead_code)]
impl PrincipalContentMessage {
    pub(crate) fn identity(&self) -> &PrincipalContentIdentity {
        &self.identity
    }

    pub(crate) fn body(&self) -> &str {
        &self.body
    }
}

#[allow(dead_code)]
struct PrincipalMessageHandlerIvars {
    // objc2's dynamically registered ivar layout supports pointer alignment,
    // while `ScriptPrincipal` contains a 128-bit ULID with 16-byte alignment.
    // Keep the typed native identity behind one pointer so class registration
    // is valid on both Apple Silicon and Intel.
    identity: Box<PrincipalContentIdentity>,
    world: Retained<WKContentWorld>,
    handler_name: Retained<NSString>,
    on_message: Box<dyn Fn(PrincipalContentMessage)>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "ZephiumPrincipalMessageHandler"]
    #[ivars = PrincipalMessageHandlerIvars]
    struct PrincipalMessageHandler;

    unsafe impl NSObjectProtocol for PrincipalMessageHandler {}

    unsafe impl WKScriptMessageHandler for PrincipalMessageHandler {
        #[unsafe(method(userContentController:didReceiveScriptMessage:))]
        fn did_receive_script_message(
            this: &PrincipalMessageHandler,
            _controller: &WKUserContentController,
            message: &WKScriptMessage,
        ) {
            let ivars = this.ivars();
            // WebKit already scopes the registration to this named world and
            // handler. Check both again before attributing the native-bound
            // principal so a future registration refactor fails closed.
            let (world, name, body) = unsafe { (message.world(), message.name(), message.body()) };
            if Retained::as_ptr(&world) != Retained::as_ptr(&ivars.world)
                || !name.isEqualToString(&ivars.handler_name)
            {
                return;
            }
            let Ok(body) = body.downcast::<NSString>() else {
                return;
            };
            let Some(body) = bounded_principal_message_body(&body) else {
                return;
            };
            dispatch_principal_message(&ivars.identity, body, &ivars.on_message);
        }
    }
);

#[allow(dead_code)]
fn bounded_principal_message_body(body: &NSString) -> Option<String> {
    if body.length() > PRINCIPAL_MESSAGE_UTF16_LIMIT {
        return None;
    }
    let body = body.to_string();
    (body.len() <= PRINCIPAL_MESSAGE_UTF8_LIMIT).then_some(body)
}

#[allow(dead_code)]
fn dispatch_principal_message(
    identity: &PrincipalContentIdentity,
    body: String,
    on_message: &dyn Fn(PrincipalContentMessage),
) {
    // Never unwind through WebKit's Objective-C callback frame.
    let _ = std::panic::catch_unwind(AssertUnwindSafe(|| {
        on_message(PrincipalContentMessage {
            identity: identity.clone(),
            body,
        });
    }));
}

/// Retains one world-scoped message handler and unregisters the exact
/// `(world, handler)` pair on drop. The delegate stores the principal chosen
/// by native registration; JavaScript supplies only the bounded message body.
#[allow(dead_code)]
pub(crate) struct PrincipalMessageHandlerRegistration {
    identity: PrincipalContentIdentity,
    controller: Retained<WKUserContentController>,
    world: Retained<WKContentWorld>,
    handler_name: Retained<NSString>,
    _delegate: Retained<PrincipalMessageHandler>,
}

#[allow(dead_code)]
impl PrincipalMessageHandlerRegistration {
    pub(crate) fn identity(&self) -> &PrincipalContentIdentity {
        &self.identity
    }
}

impl Drop for PrincipalMessageHandlerRegistration {
    fn drop(&mut self) {
        let result = objc2::exception::catch(AssertUnwindSafe(|| unsafe {
            self.controller
                .removeScriptMessageHandlerForName_contentWorld(&self.handler_name, &self.world);
        }));
        if result.is_err() {
            eprintln!(
                "engine: failed to unregister isolated handler for principal {:?}",
                self.identity.principal()
            );
        }
    }
}

#[allow(dead_code)]
pub(crate) fn register_principal_message_handler(
    view: &wry::WebView,
    principal: ScriptPrincipal,
    on_message: impl Fn(PrincipalContentMessage) + 'static,
) -> Result<PrincipalMessageHandlerRegistration, String> {
    let identity = PrincipalContentIdentity::new(principal);
    let mtm = MainThreadMarker::new().ok_or_else(|| {
        "isolated-content handler registration requires the main thread".to_owned()
    })?;
    let wk = webkit(view);
    let controller = unsafe { wk.configuration().userContentController() };
    let world_name = NSString::from_str(identity.world_name());
    // SAFETY: mtm proves WebKit main-thread affinity and the derived name is
    // non-empty, bounded ASCII without NUL.
    let world = unsafe { WKContentWorld::worldWithName(&world_name, mtm) };
    let handler_name = NSString::from_str(identity.handler_name());
    let delegate = PrincipalMessageHandler::alloc(mtm).set_ivars(PrincipalMessageHandlerIvars {
        identity: Box::new(identity.clone()),
        world: world.clone(),
        handler_name: handler_name.clone(),
        on_message: Box::new(on_message),
    });
    let delegate: Retained<PrincipalMessageHandler> = unsafe { msg_send![super(delegate), init] };
    let protocol_delegate = ProtocolObject::from_ref(&*delegate);
    objc2::exception::catch(AssertUnwindSafe(|| unsafe {
        controller.addScriptMessageHandler_contentWorld_name(
            protocol_delegate,
            &world,
            &handler_name,
        );
    }))
    .map_err(|error| {
        format!(
            "cannot register isolated handler for principal {:?}: {error:?}",
            identity.principal()
        )
    })?;

    Ok(PrincipalMessageHandlerRegistration {
        identity,
        controller,
        world,
        handler_name,
        _delegate: delegate,
    })
}

pub fn webkit(view: &wry::WebView) -> objc2::rc::Retained<objc2_web_kit::WKWebView> {
    use wry::WebViewExtMacOS;
    // SAFETY: WryWebView is a WKWebView subclass; this is a plain upcast.
    unsafe { objc2::rc::Retained::cast_unchecked(view.webview()) }
}

pub(crate) fn set_warm_spare_layout(webview: &wry::WebView, spare: bool) {
    use objc2_app_kit::NSAutoresizingMaskOptions as Mask;
    // A zero-sized spare must flex its margins, not its dimensions. Otherwise
    // AppKit derives a minimum content size from its fixed launch-size margins,
    // preventing the entire window from shrinking below its initial dimensions.
    webkit(webview).setAutoresizingMask(if spare {
        Mask::ViewMaxXMargin | Mask::ViewMinYMargin
    } else {
        Mask::ViewWidthSizable | Mask::ViewHeightSizable
    });
}

pub fn configure(
    webview: &wry::WebView,
    radius: f64,
    partition: Partition,
    expected_ephemeral_store: Option<&super::WebsiteDataStore>,
) -> Result<(), String> {
    use objc2_app_kit::{NSAutoresizingMaskOptions as Mask, NSColor, NSView};

    let wk = webkit(webview);
    let data_store = unsafe { wk.configuration().websiteDataStore() };
    let persistent = unsafe { data_store.isPersistent() };
    let identifier = unsafe { data_store.identifier() }.map(|identifier| identifier.as_bytes());
    validate_data_store_postcondition(
        partition,
        persistent,
        identifier,
        expected_ephemeral_store.is_some(),
    )?;
    if let Some(expected) = expected_ephemeral_store {
        if objc2::rc::Retained::as_ptr(&data_store) != objc2::rc::Retained::as_ptr(expected) {
            return Err("private WKWebView did not use its profile-owned data store".into());
        }
    }

    unsafe { wk.setInspectable(true) };
    // Pinch magnifies the page the way Safari's does; page zoom stays separate.
    unsafe { wk.setAllowsMagnification(true) };
    let view: &NSView = &wk;
    // Fill the assigned region and follow window resize in AppKit's layout pass.
    view.setTranslatesAutoresizingMaskIntoConstraints(true);
    view.setAutoresizingMask(Mask::ViewWidthSizable | Mask::ViewHeightSizable);
    if let Some(layer) = view.layer() {
        layer.setCornerRadius(radius);
        // The window's own corners are continuous, so a circular page corner
        // beside them reads as a different shape at the one place they meet.
        // SAFETY: an immutable framework constant.
        layer.setCornerCurve(unsafe { objc2_quartz_core::kCACornerCurveContinuous });
        layer.setMasksToBounds(true);
        // a hairline keeps the edge readable when page and backdrop are both
        // dark; without it the rounded corners visually vanish
        let border = NSColor::colorWithWhite_alpha(1.0, 0.09);
        layer.setBorderColor(Some(&border.CGColor()));
        layer.setBorderWidth(1.0);
    }

    Ok(())
}

fn validate_data_store_postcondition(
    partition: Partition,
    persistent: bool,
    identifier: Option<[u8; 16]>,
    has_expected_ephemeral_store: bool,
) -> Result<(), String> {
    match partition {
        Partition::Ephemeral(_) => {
            if !has_expected_ephemeral_store {
                return Err("ephemeral WKWebView has no profile-owned data-store proof".into());
            }
            if persistent {
                return Err("ephemeral WKWebView received a persistent website data store".into());
            }
            if identifier.is_some() {
                return Err("ephemeral WKWebView exposed a durable data-store identifier".into());
            }
        }
        Partition::Default(profile) | Partition::Persistent(profile) => {
            if has_expected_ephemeral_store {
                return Err("durable WKWebView received an ephemeral data-store proof".into());
            }
            if !persistent {
                return Err(
                    "durable WKWebView received a non-persistent website data store".into(),
                );
            }
            if identifier != Some(profile.bytes()) {
                return Err(
                    "durable WKWebView data-store identifier does not match its profile".into(),
                );
            }
        }
    }
    Ok(())
}

pub fn stop_loading(view: &wry::WebView) {
    unsafe { webkit(view).stopLoading() };
}

/// Suspension also refuses the page's own attempts to play until it is
/// lifted, so a covered page cannot start sound behind the focus surface.
pub fn set_media_suspended(view: &wry::WebView, suspended: bool) {
    unsafe { webkit(view).setAllMediaPlaybackSuspended_completionHandler(suspended, None) };
}

/// Validate the real OS owner again at native consent admission/settlement.
pub(crate) fn permission_owner_is_focused(parent: raw_window_handle::RawWindowHandle) -> bool {
    let Some(mtm) = MainThreadMarker::new() else {
        return false;
    };
    let raw_window_handle::RawWindowHandle::AppKit(parent) = parent else {
        return false;
    };
    // SAFETY: EngineHost owns the composition root's live main-window handle
    // throughout each child's lifetime. A consent modal deliberately detaches
    // the page from its stage, so focus belongs to this main-window owner.
    let view = unsafe { &*parent.ns_view.as_ptr().cast::<objc2_app_kit::NSView>() };
    objc2_app_kit::NSApplication::sharedApplication(mtm).isActive()
        && view.window().is_some_and(|window| window.isKeyWindow())
}

/// Cross-check renderer heuristics with WebKit's public media playback and
/// capture state. Playback is asynchronous; the caller's existing bounded
/// deadline handles a missing native completion without retaining the view.
/// `None` while capturing or once the view is gone; otherwise whether the
/// page may be producing sound. WebKit's audibility SPI answers exactly,
/// child frames and Web Audio included. Without it the public playback state
/// also counts muted media, so callers pair it with the renderer's report.
pub fn query_document_playback(
    view: &wry::WebView,
    done: impl FnOnce(Option<bool>) + 'static,
) -> bool {
    use std::cell::RefCell;
    use std::rc::Rc;

    use objc2_web_kit::{WKMediaCaptureState, WKMediaPlaybackState};

    let wk = webkit(view);
    let capturing = unsafe {
        wk.cameraCaptureState() != WKMediaCaptureState::None
            || wk.microphoneCaptureState() != WKMediaCaptureState::None
    };
    if capturing {
        done(None);
        return true;
    }
    // SAFETY: main-thread WebKit access; the SPI is used only when present.
    let audibility: bool =
        unsafe { objc2::msg_send![&*wk, respondsToSelector: objc2::sel!(_isPlayingAudio)] };
    if audibility {
        let audible: bool = unsafe { objc2::msg_send![&*wk, _isPlayingAudio] };
        done(Some(audible));
        return true;
    }

    let completion = Rc::new(RefCell::new(Some(done)));
    let callback_completion = completion.clone();
    let weak = objc2::rc::Weak::from_retained(&wk);
    let callback = block2::RcBlock::new(move |state: WKMediaPlaybackState| {
        if let Some(done) = callback_completion.borrow_mut().take() {
            let still_idle = weak.load().is_some_and(|page| unsafe {
                page.cameraCaptureState() == WKMediaCaptureState::None
                    && page.microphoneCaptureState() == WKMediaCaptureState::None
            });
            done(still_idle.then_some(state == WKMediaPlaybackState::Playing));
        }
    });
    unsafe { wk.requestMediaPlaybackStateWithCompletionHandler(&callback) };
    true
}

/// The renderer process footprint, as Activity Monitor reports it. WebKit
/// exposes the process only through SPI; without it no page counts as heavy.
pub(crate) fn page_footprint(view: &wry::WebView) -> Option<u64> {
    let page = webkit(view);
    // SAFETY: main-thread WebKit access; the SPI is used only when present.
    let supported: bool =
        unsafe { objc2::msg_send![&*page, respondsToSelector: objc2::sel!(_webProcessIdentifier)] };
    if !supported {
        return None;
    }
    let pid: libc::pid_t = unsafe { objc2::msg_send![&*page, _webProcessIdentifier] };
    if pid <= 0 {
        return None;
    }
    // SAFETY: a correctly sized, zeroed rusage_info_v4 for this flavor.
    let mut info: libc::rusage_info_v4 = unsafe { std::mem::zeroed() };
    let status = unsafe {
        libc::proc_pid_rusage(
            pid,
            libc::RUSAGE_INFO_V4,
            (&mut info as *mut libc::rusage_info_v4).cast(),
        )
    };
    (status == 0).then_some(info.ri_phys_footprint)
}

/// Hidden pages are throttled by default and suspended once dormant. WebKit
/// keeps a page running while it is audible or capturing, whatever this says,
/// and resumes it as soon as it becomes visible.
pub(crate) fn set_background_suspension(view: &wry::WebView, suspend: bool) {
    use objc2_web_kit::WKInactiveSchedulingPolicy as Policy;
    let target = if suspend {
        Policy::Suspend
    } else {
        Policy::Throttle
    };
    let page = webkit(view);
    // SAFETY: main-thread WebKit access; each view owns its preferences
    // object, so this changes only this page's background scheduling.
    unsafe {
        let preferences = page.configuration().preferences();
        if preferences.inactiveSchedulingPolicy() != target {
            preferences.setInactiveSchedulingPolicy(target);
        }
    }
}

pub(crate) fn native_discard_idle(view: &wry::WebView) -> bool {
    use objc2_web_kit::WKMediaCaptureState;
    let page = webkit(view);
    page.isHiddenOrHasHiddenAncestor()
        && page
            .window()
            .is_none_or(|window| window.attachedSheet().is_none())
        && unsafe {
            !page.isLoading()
                && page.cameraCaptureState() == WKMediaCaptureState::None
                && page.microphoneCaptureState() == WKMediaCaptureState::None
        }
}

pub fn add_user_script(
    view: &wry::WebView,
    script: &zephium_core::ports::engine::UserScript,
) -> Result<(), String> {
    use zephium_core::ports::engine::World;

    if let Some(reason) = user_script_refusal(script) {
        return Err(format!("unsupported user-script semantics: {reason:?}"));
    }

    match script.world {
        World::Page => add_page_user_script(view, script),
        World::Isolated(_) => add_user_script_in_principal_world(view, script),
    }
}

pub fn user_script_refusal(
    script: &zephium_core::ports::engine::UserScript,
) -> Option<zephium_core::ports::engine::UserScriptRefusalReason> {
    use zephium_core::ports::engine::{RunAt, ScriptOwner, UserScriptRefusalReason};

    if matches!(script.owner, ScriptOwner::Principal(_))
        || !script.matches.is_unconditional_all_urls()
    {
        return Some(UserScriptRefusalReason::UnsupportedMatchSet);
    }
    (script.run_at == RunAt::DocumentIdle).then_some(UserScriptRefusalReason::UnsupportedRunAt)
}

pub fn user_style_refusal(
    style: &zephium_core::ports::engine::UserStyle,
) -> Option<zephium_core::ports::engine::UserScriptRefusalReason> {
    use zephium_core::ports::engine::{ScriptOwner, UserScriptRefusalReason};

    (matches!(style.owner, ScriptOwner::Principal(_)) || !style.matches.is_unconditional_all_urls())
        .then_some(UserScriptRefusalReason::UnsupportedMatchSet)
}

fn native_injection_time(
    run_at: zephium_core::ports::engine::RunAt,
) -> Result<objc2_web_kit::WKUserScriptInjectionTime, String> {
    use objc2_web_kit::WKUserScriptInjectionTime;
    use zephium_core::ports::engine::RunAt;

    match run_at {
        RunAt::DocumentStart => Ok(WKUserScriptInjectionTime::AtDocumentStart),
        RunAt::DocumentEnd => Ok(WKUserScriptInjectionTime::AtDocumentEnd),
        RunAt::DocumentIdle => Err("document_idle scheduling is not implemented on macOS".into()),
    }
}

fn validated_native_injection_time(
    script: &zephium_core::ports::engine::UserScript,
) -> Result<objc2_web_kit::WKUserScriptInjectionTime, String> {
    use zephium_core::ports::engine::{ScriptOwner, World, MAX_USER_SCRIPT_BYTES};

    if script.source.is_empty() {
        return Err("user script source is empty".into());
    }
    if script.source.len() > MAX_USER_SCRIPT_BYTES {
        return Err(format!(
            "user script source exceeds {MAX_USER_SCRIPT_BYTES} bytes"
        ));
    }
    if !script.matches.is_unconditional_all_urls() {
        return Err(
            "pre-source per-frame match-set enforcement is not implemented on macOS".into(),
        );
    }
    let owner_matches_world = matches!(
        (script.owner, script.world),
        (ScriptOwner::Builtin, World::Page)
    ) || matches!(
        (script.owner, script.world),
        (ScriptOwner::Principal(owner), World::Isolated(world)) if owner == world
    );
    if !owner_matches_world {
        return Err("user script owner does not match its native world principal".into());
    }
    native_injection_time(script.run_at)
}

fn add_page_user_script(
    view: &wry::WebView,
    script: &zephium_core::ports::engine::UserScript,
) -> Result<(), String> {
    use objc2_web_kit::WKUserScript;
    use zephium_core::ports::engine::{ScriptOwner, World};

    if script.owner != ScriptOwner::Builtin || script.world != World::Page {
        return Err("page-world scripts must be owned by the builtin principal".into());
    }
    let time = validated_native_injection_time(script)?;
    let mtm = MainThreadMarker::new()
        .ok_or_else(|| "page script installation requires the main thread".to_owned())?;
    let wk = webkit(view);
    let source = NSString::from_str(script.source.as_ref());
    let user_script = unsafe {
        WKUserScript::initWithSource_injectionTime_forMainFrameOnly(
            WKUserScript::alloc(mtm),
            &source,
            time,
            !script.all_frames,
        )
    };
    objc2::exception::catch(AssertUnwindSafe(|| unsafe {
        wk.configuration()
            .userContentController()
            .addUserScript(&user_script);
    }))
    .map_err(|error| format!("cannot install page-world script: {error:?}"))
}

/// Installs an isolated script into the exact principal world selected by the
/// native owner. The caller must retain the same identity and handler
/// registration for the lifetime of the script set.
pub(crate) fn add_user_script_in_principal_world(
    view: &wry::WebView,
    script: &zephium_core::ports::engine::UserScript,
) -> Result<(), String> {
    use objc2_web_kit::WKUserScript;
    use zephium_core::ports::engine::{ScriptOwner, World};

    let World::Isolated(principal) = script.world else {
        return Err("principal-world installer accepts only isolated scripts".into());
    };
    if script.owner != ScriptOwner::Principal(principal) {
        return Err("isolated script owner does not match its native world principal".into());
    }
    let time = validated_native_injection_time(script)?;
    let identity = PrincipalContentIdentity::new(principal);
    let mtm = MainThreadMarker::new()
        .ok_or_else(|| "isolated script installation requires the main thread".to_owned())?;
    let wk = webkit(view);
    let source = NSString::from_str(script.source.as_ref());
    // SAFETY: mtm proves WebKit main-thread affinity and the typed principal
    // produces a non-empty, bounded ASCII world name.
    let world =
        unsafe { WKContentWorld::worldWithName(&NSString::from_str(identity.world_name()), mtm) };
    let user_script = unsafe {
        WKUserScript::initWithSource_injectionTime_forMainFrameOnly_inContentWorld(
            WKUserScript::alloc(mtm),
            &source,
            time,
            !script.all_frames,
            &world,
        )
    };
    objc2::exception::catch(AssertUnwindSafe(|| unsafe {
        wk.configuration()
            .userContentController()
            .addUserScript(&user_script);
    }))
    .map_err(|error| {
        format!(
            "cannot install isolated script for principal {:?}: {error:?}",
            identity.principal()
        )
    })
}

#[cfg(feature = "native-page-permission-probes")]
mod page_permission_probe {
    use std::cell::RefCell;
    use std::ffi::c_void;
    use std::io::{Read as _, Write as _};
    use std::net::{SocketAddr, TcpListener, TcpStream};
    use std::ptr::NonNull;
    use std::rc::Rc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::thread::JoinHandle;
    use std::time::{Duration, Instant};

    use objc2::rc::Retained;
    use objc2_app_kit::{
        NSApplication, NSApplicationActivationPolicy, NSBackingStoreType, NSView, NSWindow,
        NSWindowStyleMask,
    };
    use objc2_foundation::{NSDate, NSPoint, NSRect, NSRunLoop, NSSize};
    use raw_window_handle::{
        AppKitWindowHandle, HandleError, HasWindowHandle, RawWindowHandle, WindowHandle,
    };
    use wry::{
        PermissionRequest, PermissionRequestDisposition, PermissionRequestKind, PermissionResponse,
        WebViewBuilderExtMacos as _, WebViewExtMacOS as _,
    };

    use super::*;

    const PROBE_TIMEOUT: Duration = Duration::from_secs(10);
    const DENIED_TITLE: &str = "zephium-media-denied:NotAllowedError";
    const FIXTURE: &str = r#"<!doctype html>
<meta charset="utf-8">
<meta http-equiv="Content-Security-Policy" content="default-src 'none'; script-src 'unsafe-inline'">
<title>zephium-media-loading</title>
<script>
(() => {
  'use strict';
  const finish = value => { document.title = value; };
  if (!globalThis.isSecureContext) {
    finish('zephium-media-insecure-context');
    return;
  }
  if (typeof navigator.mediaDevices?.getUserMedia !== 'function') {
    finish('zephium-media-api-unavailable');
    return;
  }
  document.title = 'zephium-media-requesting';
  navigator.mediaDevices.getUserMedia({ audio: true, video: true }).then(
    stream => {
      for (const track of stream.getTracks()) track.stop();
      finish('zephium-media-unexpectedly-allowed');
    },
    error => finish(`zephium-media-denied:${String(error?.name || 'UnknownError')}`),
  );
})();
</script>"#;

    struct ProbeHostView {
        view: Retained<NSView>,
    }

    impl HasWindowHandle for ProbeHostView {
        fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
            let pointer = NonNull::from(&*self.view).cast::<c_void>();
            let raw = RawWindowHandle::AppKit(AppKitWindowHandle::new(pointer));
            // SAFETY: `self.view` owns the exact NSView for this borrow, and
            // the retained host window outlives the Wry child.
            Ok(unsafe { WindowHandle::borrow_raw(raw) })
        }
    }

    struct FixtureServer {
        address: SocketAddr,
        stop: Arc<AtomicBool>,
        worker: Option<JoinHandle<()>>,
    }

    impl FixtureServer {
        fn start() -> Result<Self, String> {
            let listener = TcpListener::bind(("127.0.0.1", 0))
                .map_err(|error| format!("cannot bind page-permission fixture: {error}"))?;
            let address = listener
                .local_addr()
                .map_err(|error| format!("cannot read fixture address: {error}"))?;
            listener
                .set_nonblocking(true)
                .map_err(|error| format!("cannot bound fixture acceptance: {error}"))?;
            let stop = Arc::new(AtomicBool::new(false));
            let thread_stop = stop.clone();
            let worker = std::thread::Builder::new()
                .name("zephium-page-permission-probe-http".into())
                .spawn(move || serve(listener, thread_stop))
                .map_err(|error| format!("cannot start page-permission fixture: {error}"))?;
            Ok(Self {
                address,
                stop,
                worker: Some(worker),
            })
        }

        fn url(&self) -> String {
            format!("http://{}/", self.address)
        }
    }

    impl Drop for FixtureServer {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Release);
            if let Some(worker) = self.worker.take() {
                let _ = worker.join();
            }
        }
    }

    fn serve(listener: TcpListener, stop: Arc<AtomicBool>) {
        while !stop.load(Ordering::Acquire) {
            match listener.accept() {
                Ok((stream, _)) => serve_once(stream),
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(2));
                }
                Err(_) => break,
            }
        }
    }

    fn serve_once(mut stream: TcpStream) {
        let _ = stream.set_read_timeout(Some(Duration::from_secs(1)));
        let mut request = [0_u8; 8 * 1_024];
        let mut received = 0;
        while received < request.len() {
            match stream.read(&mut request[received..]) {
                Ok(0) => return,
                Ok(read) => {
                    received += read;
                    if request[..received]
                        .windows(4)
                        .any(|window| window == b"\r\n\r\n")
                    {
                        break;
                    }
                }
                Err(_) => return,
            }
        }
        if !request[..received].starts_with(b"GET / HTTP/1.1\r\n") {
            return;
        }
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nCache-Control: no-store\r\nPermissions-Policy: camera=(self), microphone=(self)\r\nConnection: close\r\n\r\n{}",
            FIXTURE.len(),
            FIXTURE,
        );
        let _ = stream.write_all(response.as_bytes());
        let _ = stream.flush();
    }

    #[derive(Default)]
    struct ProbeState {
        title: String,
        request: Option<PermissionRequest>,
        duplicate_request: bool,
    }

    fn run_loop_until(
        run_loop: &NSRunLoop,
        state: &RefCell<ProbeState>,
        predicate: impl Fn(&ProbeState) -> bool,
        description: &str,
    ) -> Result<(), String> {
        let deadline = Instant::now() + PROBE_TIMEOUT;
        loop {
            {
                let state = state.borrow();
                if predicate(&state) {
                    return Ok(());
                }
                if matches!(
                    state.title.as_str(),
                    "zephium-media-insecure-context"
                        | "zephium-media-api-unavailable"
                        | "zephium-media-unexpectedly-allowed"
                ) {
                    return Err(format!(
                        "page-permission fixture entered terminal state {:?} while {description}",
                        state.title
                    ));
                }
            }
            if Instant::now() >= deadline {
                let state = state.borrow();
                return Err(format!(
                    "timed out {description}; title={:?}; request_observed={}",
                    state.title,
                    state.request.is_some(),
                ));
            }
            objc2::rc::autoreleasepool(|_| {
                run_loop.runUntilDate(&NSDate::dateWithTimeIntervalSinceNow(0.01));
            });
        }
    }

    pub(super) fn run() -> Result<(), String> {
        objc2::rc::autoreleasepool(|_| run_in_autorelease_pool())
    }

    fn run_in_autorelease_pool() -> Result<(), String> {
        let mtm = MainThreadMarker::new()
            .ok_or_else(|| "page-permission probe must run on process main".to_owned())?;
        let server = FixtureServer::start()?;
        let expected_port = server.address.port();
        let app = NSApplication::sharedApplication(mtm);
        let _ = app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
        app.finishLaunching();

        // SAFETY: `mtm` proves AppKit affinity. The retained window is
        // non-autoreleasing and outlives its host view and Wry child.
        let window = unsafe {
            NSWindow::initWithContentRect_styleMask_backing_defer(
                NSWindow::alloc(mtm),
                NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(640.0, 480.0)),
                NSWindowStyleMask::Borderless,
                NSBackingStoreType::Buffered,
                false,
            )
        };
        // SAFETY: the probe retains the exact window until after child
        // teardown, so closing must not consume the retained owner.
        unsafe { window.setReleasedWhenClosed(false) };
        let host = ProbeHostView {
            view: window
                .contentView()
                .ok_or_else(|| "page-permission probe window has no content view".to_owned())?,
        };

        let state = Rc::new(RefCell::new(ProbeState::default()));
        let title_state = state.clone();
        let request_state = state.clone();
        let view = wry::WebViewBuilder::new()
            .with_incognito(true)
            .with_document_title_changed_handler(move |title| {
                title_state.borrow_mut().title = title;
            })
            .with_permission_request_handler(move |request| {
                let mut state = request_state.borrow_mut();
                if state.request.replace(request).is_some() {
                    state.duplicate_request = true;
                }
                PermissionRequestDisposition::Defer
            })
            .build_as_child(&host)
            .map_err(|error| format!("cannot construct page-permission WebView: {error}"))?;
        window.orderFrontRegardless();
        view.load_url(&server.url())
            .map_err(|error| format!("cannot load page-permission fixture: {error}"))?;

        let run_loop = NSRunLoop::mainRunLoop();
        run_loop_until(
            &run_loop,
            &state,
            |state| state.request.is_some(),
            "waiting for the native media request",
        )?;
        let request = {
            let state = state.borrow();
            if state.duplicate_request {
                return Err(
                    "WebKit emitted more than one native request for one atomic call".into(),
                );
            }
            state
                .request
                .clone()
                .ok_or_else(|| "native media request disappeared".to_owned())?
        };
        let origin = request.origin();
        if origin.scheme() != "http"
            || origin.host() != "127.0.0.1"
            || origin.port() != Some(expected_port)
        {
            return Err(format!(
                "native request reported the wrong structured origin: {}://{}:{:?}",
                origin.scheme(),
                origin.host(),
                origin.port(),
            ));
        }
        if request.kind() != PermissionRequestKind::CameraAndMicrophone {
            return Err(format!(
                "combined getUserMedia request lost atomicity: {:?}",
                request.kind()
            ));
        }
        if !view.resolve_permission_request(request.id(), PermissionResponse::Deny) {
            return Err("exact deferred media request refused its first denial".into());
        }
        if view.resolve_permission_request(request.id(), PermissionResponse::Deny) {
            return Err("deferred media request accepted a duplicate settlement".into());
        }
        run_loop_until(
            &run_loop,
            &state,
            |state| state.title == DENIED_TITLE,
            "waiting for JavaScript denial",
        )?;

        drop(view);
        window.close();
        drop(server);
        println!(
            "native-probe: macOS page permission denial passed; origin=loopback; capability=camera-and-microphone; disposition=deferred; settlement=deny; exactly_once=passed; javascript_rejection=NotAllowedError; native_allow=never"
        );
        Ok(())
    }
}

#[cfg(feature = "native-page-permission-probes")]
pub(crate) fn run_page_permission_probe() -> Result<(), String> {
    page_permission_probe::run()
}

#[cfg(feature = "native-isolation-probes")]
mod principal_isolation_probe {
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::ffi::c_void;
    use std::ptr::NonNull;
    use std::rc::Rc;
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

    use objc2::{rc::Retained, ClassType};
    use objc2_app_kit::{
        NSApplication, NSApplicationActivationPolicy, NSBackingStoreType, NSView, NSWindow,
        NSWindowStyleMask,
    };
    use objc2_foundation::{
        NSBundle, NSDate, NSPoint, NSProcessInfo, NSRect, NSRunLoop, NSSize, NSString,
    };
    use objc2_web_kit::WKWebView;
    use raw_window_handle::{
        AppKitWindowHandle, HandleError, HasWindowHandle, RawWindowHandle, WindowHandle,
    };
    use serde_json::Value;
    use zephium_core::ids::{ExtensionInstallId, ScriptId, UserscriptId};
    use zephium_core::injection::MatchSet;
    use zephium_core::ports::engine::{RunAt, ScriptOwner, UserScript, World};

    use super::*;

    const PROBE_TIMEOUT: Duration = Duration::from_secs(10);
    const PAGE_ISOLATED_TITLE: &str = "zephium-page-isolated";
    const RUNTIME_EVIDENCE_BYTES_LIMIT: usize = 128;

    struct ProbeHostView {
        view: Retained<NSView>,
    }

    impl HasWindowHandle for ProbeHostView {
        fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
            let pointer = NonNull::from(&*self.view).cast::<c_void>();
            let raw = RawWindowHandle::AppKit(AppKitWindowHandle::new(pointer));
            // SAFETY: `self.view` owns the exact NSView for the returned
            // borrow, and the host plus its NSWindow outlive the child WebView.
            Ok(unsafe { WindowHandle::borrow_raw(raw) })
        }
    }

    #[derive(Default)]
    struct ProbeState {
        messages: HashMap<ScriptPrincipal, String>,
        duplicate_message: bool,
        title: String,
    }

    struct RuntimeEvidence {
        operating_system: String,
        safari_version: String,
        safari_build: String,
        webkit_build: String,
        advisory_count: usize,
    }

    fn principals() -> [ScriptPrincipal; 3] {
        [
            ScriptPrincipal::Userscript(UserscriptId::from(1)),
            ScriptPrincipal::Userscript(UserscriptId::from(2)),
            ScriptPrincipal::Extension(ExtensionInstallId::from(1)),
        ]
    }

    fn bounded_string(value: &NSString, description: &str) -> Result<String, String> {
        if value.length() > RUNTIME_EVIDENCE_BYTES_LIMIT {
            return Err(format!("{description} exceeds the native evidence limit"));
        }
        let value = value.to_string();
        if value.is_empty() || value.len() > RUNTIME_EVIDENCE_BYTES_LIMIT {
            return Err(format!(
                "{description} is empty or exceeds the evidence limit"
            ));
        }
        Ok(value)
    }

    fn require_bundle_identifier(
        bundle: &NSBundle,
        expected: &str,
        description: &str,
    ) -> Result<(), String> {
        let identifier = bundle
            .bundleIdentifier()
            .ok_or_else(|| format!("{description} bundle has no identifier"))?;
        let identifier = bounded_string(&identifier, "bundle identifier")?;
        if identifier != expected {
            return Err(format!(
                "{description} bundle has unexpected identifier {identifier:?}"
            ));
        }
        Ok(())
    }

    fn bundle_string(bundle: &NSBundle, key: &str) -> Result<String, String> {
        let key = NSString::from_str(key);
        let value = bundle
            .infoDictionary()
            .and_then(|info| info.objectForKey(&key))
            .ok_or_else(|| format!("bundle information dictionary has no {key}"))?;
        let value = value
            .downcast::<NSString>()
            .map_err(|_| format!("bundle information dictionary {key} is not a string"))?;
        bounded_string(&value, "bundle version")
    }

    fn runtime_evidence() -> Result<RuntimeEvidence, String> {
        let reported_os = NSProcessInfo::processInfo().operatingSystemVersion();
        let major = u32::try_from(reported_os.majorVersion)
            .map_err(|_| "NSProcessInfo reported an invalid macOS major version".to_owned())?;
        let minor = u32::try_from(reported_os.minorVersion)
            .map_err(|_| "NSProcessInfo reported an invalid macOS minor version".to_owned())?;
        let patch = u32::try_from(reported_os.patchVersion)
            .map_err(|_| "NSProcessInfo reported an invalid macOS patch version".to_owned())?;
        let operating_system = format!("{major}.{minor}.{patch}");

        let safari = NSBundle::bundleWithPath(&NSString::from_str("/Applications/Safari.app"))
            .ok_or_else(|| "cannot open the canonical system Safari bundle".to_owned())?;
        require_bundle_identifier(&safari, "com.apple.Safari", "Safari")?;
        let safari_version = bundle_string(&safari, "CFBundleShortVersionString")?;
        let safari_build = bundle_string(&safari, "CFBundleVersion")?;

        // SAFETY: WKWebView::class() is the live Objective-C class from the
        // framework this probe will instantiate; bundleForClass therefore
        // identifies the loaded implementation rather than a lookup alias.
        let webkit = unsafe { NSBundle::bundleForClass(WKWebView::class()) };
        require_bundle_identifier(&webkit, "com.apple.WebKit", "WKWebView")?;
        let webkit_build = bundle_string(&webkit, "CFBundleVersion")?;

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|error| format!("system UTC clock predates the Unix epoch: {error}"))?
            .as_secs();
        let advisories = zephium_core::macos::assess_runtime(
            &operating_system,
            &safari_version,
            &safari_build,
            &webkit_build,
            now,
        )
        .map_err(|error| format!("macOS/WebKit runtime admission failed: {error}"))?;

        Ok(RuntimeEvidence {
            operating_system,
            safari_version,
            safari_build,
            webkit_build,
            advisory_count: advisories.len(),
        })
    }

    fn wait_for_navigation(
        state: &RefCell<ProbeState>,
        run_loop: &NSRunLoop,
        expected_messages: usize,
        deadline: Instant,
    ) -> Result<(), String> {
        loop {
            {
                let state = state.borrow();
                if state.title == "zephium-page-isolation-failed" {
                    return Err("page world reached a principal handler or isolated global".into());
                }
                if state.messages.len() == expected_messages && state.title == PAGE_ISOLATED_TITLE {
                    return Ok(());
                }
            }
            if Instant::now() >= deadline {
                let state = state.borrow();
                return Err(format!(
                    "timed out with {}/{} principal messages and page title {:?}",
                    state.messages.len(),
                    expected_messages,
                    state.title
                ));
            }
            objc2::rc::autoreleasepool(|_| {
                run_loop.runUntilDate(&NSDate::dateWithTimeIntervalSinceNow(0.01));
            });
        }
    }

    fn run_quiet_window(run_loop: &NSRunLoop) {
        let quiet_deadline = Instant::now() + Duration::from_millis(250);
        while Instant::now() < quiet_deadline {
            objc2::rc::autoreleasepool(|_| {
                run_loop.runUntilDate(&NSDate::dateWithTimeIntervalSinceNow(0.01));
            });
        }
    }

    fn validate_message(identity: &PrincipalContentIdentity, body: &str) -> Result<(), String> {
        let body: Value = serde_json::from_str(body)
            .map_err(|error| format!("principal message is not valid JSON: {error}"))?;
        let visible = body
            .get("visible")
            .and_then(Value::as_array)
            .ok_or_else(|| "principal message has no visible-handler array".to_owned())?;
        if visible.len() != 1
            || visible[0].as_str() != Some(identity.handler_name())
            || body.get("inherited").and_then(Value::as_str) != Some("undefined")
            || body.get("world").and_then(Value::as_str) != Some(identity.world_name())
            || body.get("claimed").and_then(Value::as_str) != Some("another-principal")
        {
            return Err(format!(
                "principal {:?} observed a peer/page world or lost native attribution: {body}",
                identity.principal()
            ));
        }
        Ok(())
    }

    pub(super) fn run() -> Result<(), String> {
        objc2::rc::autoreleasepool(|_| run_in_autorelease_pool())
    }

    fn run_in_autorelease_pool() -> Result<(), String> {
        let mtm = MainThreadMarker::new().ok_or_else(|| {
            "native principal-isolation probe must run on process main".to_owned()
        })?;
        let runtime = runtime_evidence()?;
        let app = NSApplication::sharedApplication(mtm);
        // An unbundled command-line process can truthfully refuse this cosmetic
        // Dock/menu-bar transition even though AppKit and WKWebView are fully
        // available. It is not part of the isolation boundary, so keep the
        // probe non-activating when supported without making that preference a
        // false-negative security gate.
        let _ = app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
        app.finishLaunching();

        // SAFETY: `mtm` proves AppKit main-thread affinity. The retained window
        // is explicitly non-autoreleasing and outlives its content view and Wry
        // child.
        let window = unsafe {
            NSWindow::initWithContentRect_styleMask_backing_defer(
                NSWindow::alloc(mtm),
                NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(640.0, 480.0)),
                NSWindowStyleMask::Borderless,
                NSBackingStoreType::Buffered,
                false,
            )
        };
        // SAFETY: The probe retains `window` until after the child WebView is
        // dropped, so AppKit must not release it as a side effect of closing.
        unsafe { window.setReleasedWhenClosed(false) };
        let host = ProbeHostView {
            view: window
                .contentView()
                .ok_or_else(|| "probe NSWindow has no content view".to_owned())?,
        };

        let state = Rc::new(RefCell::new(ProbeState::default()));
        let title_state = state.clone();
        let view = wry::WebViewBuilder::new()
            .with_incognito(true)
            .with_document_title_changed_handler(move |title| {
                title_state.borrow_mut().title = title;
            })
            .build_as_child(&host)
            .map_err(|error| format!("cannot construct principal-isolation WebView: {error}"))?;
        window.orderFrontRegardless();

        let identities = principals().map(PrincipalContentIdentity::new);
        let handler_names = identities
            .iter()
            .map(PrincipalContentIdentity::handler_name)
            .collect::<Vec<_>>();
        let handler_names_json = serde_json::to_string(&handler_names)
            .map_err(|error| format!("cannot encode native handler names: {error}"))?;
        let mut registrations = Vec::with_capacity(identities.len());

        for (index, identity) in identities.iter().enumerate() {
            let message_state = state.clone();
            registrations.push(register_principal_message_handler(
                &view,
                identity.principal(),
                move |message| {
                    let mut state = message_state.borrow_mut();
                    if state
                        .messages
                        .insert(message.identity().principal(), message.body().to_owned())
                        .is_some()
                    {
                        state.duplicate_message = true;
                    }
                },
            )?);

            let world = serde_json::to_string(identity.world_name())
                .map_err(|error| format!("cannot encode native world name: {error}"))?;
            let handler = serde_json::to_string(identity.handler_name())
                .map_err(|error| format!("cannot encode native handler name: {error}"))?;
            let source = format!(
                r#"(() => {{
                    const names = {handler_names_json};
                    const visible = names.filter(
                        name => !!globalThis.webkit?.messageHandlers?.[name]
                    );
                    const inherited = typeof globalThis.__zephiumPrincipalProbe;
                    globalThis.__zephiumPrincipalProbe = {world};
                    globalThis.webkit.messageHandlers[{handler}].postMessage(
                        JSON.stringify({{
                            visible,
                            inherited,
                            world: {world},
                            claimed: "another-principal"
                        }})
                    );
                }})()"#,
            );
            add_user_script_in_principal_world(
                &view,
                &UserScript {
                    id: ScriptId::from(index as u128 + 1),
                    owner: ScriptOwner::Principal(identity.principal()),
                    source: source.into(),
                    world: World::Isolated(identity.principal()),
                    matches: MatchSet::all_urls(),
                    run_at: RunAt::DocumentStart,
                    all_frames: true,
                },
            )?;
        }

        let page = format!(
            r#"<!doctype html><meta charset="utf-8"><title>loading</title><script>
                const names = {handler_names_json};
                const handlersAbsent = names.every(
                    name => !globalThis.webkit?.messageHandlers?.[name]
                );
                const worldsAbsent = typeof globalThis.__zephiumPrincipalProbe === "undefined";
                globalThis.__zephiumPrincipalProbe = "page";
                document.title = handlersAbsent && worldsAbsent
                    ? "{PAGE_ISOLATED_TITLE}"
                    : "zephium-page-isolation-failed";
            </script>"#,
        );
        view.load_html(&page)
            .map_err(|error| format!("cannot load hostile isolation document: {error}"))?;

        let run_loop = NSRunLoop::mainRunLoop();
        let deadline = Instant::now() + PROBE_TIMEOUT;
        wait_for_navigation(&state, &run_loop, identities.len(), deadline)?;

        let (page_tx, page_rx) = std::sync::mpsc::channel();
        view.evaluate_script_with_callback(
            &format!(
                r#"({{
                    handlersAbsent: {handler_names_json}.every(
                        name => !globalThis.webkit?.messageHandlers?.[name]
                    ),
                    marker: globalThis.__zephiumPrincipalProbe
                }})"#,
            ),
            move |result| {
                let _ = page_tx.send(result);
            },
        )
        .map_err(|error| format!("cannot evaluate page-world isolation result: {error}"))?;
        let page_deadline = Instant::now() + PROBE_TIMEOUT;
        let page_result = loop {
            match page_rx.try_recv() {
                Ok(result) => break result,
                Err(std::sync::mpsc::TryRecvError::Empty) if Instant::now() < page_deadline => {
                    objc2::rc::autoreleasepool(|_| {
                        run_loop.runUntilDate(&NSDate::dateWithTimeIntervalSinceNow(0.01));
                    });
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {
                    return Err("timed out evaluating the page-world isolation result".into());
                }
                Err(error) => return Err(format!("page-world result channel failed: {error}")),
            }
        };
        let page_result: Value = serde_json::from_str(&page_result)
            .map_err(|error| format!("page-world result is not valid JSON: {error}"))?;
        if page_result.get("handlersAbsent").and_then(Value::as_bool) != Some(true)
            || page_result.get("marker").and_then(Value::as_str) != Some("page")
        {
            return Err(format!(
                "page world reached a principal handler or isolated global after load: {page_result}"
            ));
        }

        run_quiet_window(&run_loop);

        {
            let state = state.borrow();
            if state.duplicate_message {
                return Err("a principal handler received more than one top-frame message".into());
            }
            for identity in &identities {
                let body = state.messages.get(&identity.principal()).ok_or_else(|| {
                    format!("principal {:?} produced no message", identity.principal())
                })?;
                validate_message(identity, body)?;
            }
        }

        let removed = identities[1].clone();
        drop(registrations.remove(1));
        // WebKit permits only one handler for an exact `(world, name)` pair.
        // Re-registering that pair is therefore deterministic native proof
        // that RAII removed the original registration, rather than an
        // inference from the absence of a later callback.
        let replacement = register_principal_message_handler(
            &view,
            removed.principal(),
            |_unexpected_message| {},
        )?;
        drop(replacement);
        *state.borrow_mut() = ProbeState::default();
        view.load_html(&page)
            .map_err(|error| format!("cannot load handler-removal document: {error}"))?;
        wait_for_navigation(
            &state,
            &run_loop,
            identities.len() - 1,
            Instant::now() + PROBE_TIMEOUT,
        )?;
        run_quiet_window(&run_loop);
        {
            let state = state.borrow();
            if state.duplicate_message || state.messages.contains_key(&removed.principal()) {
                return Err(format!(
                    "removed principal {:?} remained reachable or peers duplicated",
                    removed.principal()
                ));
            }
            for identity in identities
                .iter()
                .filter(|identity| identity.principal() != removed.principal())
            {
                let body = state.messages.get(&identity.principal()).ok_or_else(|| {
                    format!(
                        "peer principal {:?} stopped after exact handler removal",
                        identity.principal()
                    )
                })?;
                validate_message(identity, body)?;
            }
        }

        drop(registrations);
        drop(view);
        window.close();
        println!(
            "native-probe: macOS principal isolation passed; os={}; safari={}; safari_build={}; webkit_build={}; advisories={}; principals=3; page_handlers=0; exact_deregistration=passed",
            runtime.operating_system,
            runtime.safari_version,
            runtime.safari_build,
            runtime.webkit_build,
            runtime.advisory_count,
        );
        Ok(())
    }
}

#[cfg(feature = "native-isolation-probes")]
pub(crate) fn run_principal_isolation_probe() -> Result<(), String> {
    principal_isolation_probe::run()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::HashSet;

    use zephium_core::ids::{ExtensionInstallId, ProfileId, ScriptId, UserscriptId};
    use zephium_core::injection::{MatchOptions, MatchSet};
    use zephium_core::ports::engine::{RunAt, ScriptOwner, UserScript, World};

    fn test_principals() -> [ScriptPrincipal; 3] {
        [
            ScriptPrincipal::Userscript(UserscriptId::from(1)),
            ScriptPrincipal::Userscript(UserscriptId::from(2)),
            ScriptPrincipal::Extension(ExtensionInstallId::from(1)),
        ]
    }

    #[test]
    fn three_principals_have_distinct_world_and_handler_identities() {
        let identities = test_principals().map(PrincipalContentIdentity::new);
        let worlds = identities
            .iter()
            .map(PrincipalContentIdentity::world_name)
            .collect::<HashSet<_>>();
        let handlers = identities
            .iter()
            .map(PrincipalContentIdentity::handler_name)
            .collect::<HashSet<_>>();

        assert_eq!(worlds.len(), identities.len());
        assert_eq!(handlers.len(), identities.len());
        assert!(worlds
            .iter()
            .all(|name| name.starts_with(PRINCIPAL_WORLD_PREFIX)
                && name.len() <= PRINCIPAL_WORLD_PREFIX.len() + PRINCIPAL_TOKEN_MAX_BYTES
                && name.is_ascii()));
        assert!(handlers
            .iter()
            .all(|name| name.starts_with(PRINCIPAL_HANDLER_PREFIX)
                && name.len() <= PRINCIPAL_HANDLER_PREFIX.len() + PRINCIPAL_TOKEN_MAX_BYTES
                && name.is_ascii()));
        assert!(!worlds.contains("page"));
        // Equal ULID bytes in different principal variants must never alias.
        assert_ne!(identities[0].world_name(), identities[2].world_name());
        assert_ne!(identities[0].handler_name(), identities[2].handler_name());
    }

    #[test]
    fn message_payload_cannot_relabel_its_native_principal() {
        let delivered = RefCell::new(Vec::new());
        for principal in test_principals() {
            let identity = PrincipalContentIdentity::new(principal);
            dispatch_principal_message(
                &identity,
                r#"{"principal":"extension-controlled-lie"}"#.into(),
                &|message| {
                    delivered
                        .borrow_mut()
                        .push((message.identity().principal(), message.body().to_owned()));
                },
            );
        }

        let delivered = delivered.into_inner();
        assert_eq!(delivered.len(), 3);
        for ((native, body), expected) in delivered.into_iter().zip(test_principals()) {
            assert_eq!(native, expected);
            assert!(body.contains("extension-controlled-lie"));
        }
    }

    #[test]
    fn principal_message_limits_are_inclusive_and_check_both_encodings() {
        let exact = "a".repeat(PRINCIPAL_MESSAGE_UTF8_LIMIT);
        assert_eq!(
            bounded_principal_message_body(&NSString::from_str(&exact)).as_deref(),
            Some(exact.as_str())
        );
        let over = format!("{exact}a");
        assert!(bounded_principal_message_body(&NSString::from_str(&over)).is_none());

        // This stays below the UTF-16 ceiling but exceeds the UTF-8 ceiling.
        let utf8_over = "🙂".repeat(PRINCIPAL_MESSAGE_UTF8_LIMIT / 4 + 1);
        assert!(utf8_over.encode_utf16().count() < PRINCIPAL_MESSAGE_UTF16_LIMIT);
        assert!(bounded_principal_message_body(&NSString::from_str(&utf8_over)).is_none());
    }

    #[test]
    fn native_install_validation_refuses_unenforced_semantics() {
        let principal = test_principals()[0];
        let baseline = UserScript {
            id: ScriptId::from(1),
            owner: ScriptOwner::Principal(principal),
            source: "globalThis.__zephiumProbe = true;".into(),
            world: World::Isolated(principal),
            matches: MatchSet::all_urls(),
            run_at: RunAt::DocumentStart,
            all_frames: true,
        };
        assert!(validated_native_injection_time(&baseline).is_ok());

        let mut script = baseline.clone();
        script.matches = MatchSet::parse(
            ["https://example.com/*"],
            std::iter::empty::<&str>(),
            MatchOptions::default(),
        )
        .unwrap();
        assert!(validated_native_injection_time(&script)
            .unwrap_err()
            .contains("match-set"));

        let mut script = baseline.clone();
        script.run_at = RunAt::DocumentIdle;
        assert!(validated_native_injection_time(&script)
            .unwrap_err()
            .contains("document_idle"));

        let mut script = baseline.clone();
        script.owner = ScriptOwner::Builtin;
        assert!(validated_native_injection_time(&script)
            .unwrap_err()
            .contains("owner"));

        let mut script = baseline;
        script.source = "".into();
        assert!(validated_native_injection_time(&script)
            .unwrap_err()
            .contains("empty"));
    }

    #[test]
    fn data_store_postcondition_binds_persistence_and_profile_identity() {
        let profile = ProfileId::from(7);
        let other = ProfileId::from(8);

        assert!(validate_data_store_postcondition(
            Partition::Persistent(profile),
            true,
            Some(profile.bytes()),
            false,
        )
        .is_ok());
        assert!(validate_data_store_postcondition(
            Partition::Default(profile),
            true,
            Some(profile.bytes()),
            false,
        )
        .is_ok());
        assert!(validate_data_store_postcondition(
            Partition::Ephemeral(profile),
            false,
            None,
            true
        )
        .is_ok());

        assert!(validate_data_store_postcondition(
            Partition::Persistent(profile),
            false,
            Some(profile.bytes()),
            false,
        )
        .is_err());
        assert!(validate_data_store_postcondition(
            Partition::Persistent(profile),
            true,
            Some(other.bytes()),
            false,
        )
        .is_err());
        assert!(validate_data_store_postcondition(
            Partition::Persistent(profile),
            true,
            None,
            false
        )
        .is_err());
        assert!(
            validate_data_store_postcondition(Partition::Ephemeral(profile), true, None, true)
                .is_err()
        );
        assert!(validate_data_store_postcondition(
            Partition::Ephemeral(profile),
            false,
            Some(profile.bytes()),
            true,
        )
        .is_err());
        assert!(validate_data_store_postcondition(
            Partition::Ephemeral(profile),
            false,
            None,
            false
        )
        .is_err());
        assert!(validate_data_store_postcondition(
            Partition::Persistent(profile),
            true,
            Some(profile.bytes()),
            true,
        )
        .is_err());
    }
}
