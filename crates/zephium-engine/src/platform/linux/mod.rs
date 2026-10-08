//! Linux adapter. Content views are built into a gtk::Fixed owned by the
//! composition root; the stage positions them and draws the drop indicator.

mod content_filter;
mod find;
mod stage;

pub(crate) fn external_app_name(_url: &str) -> Option<String> {
    None
}

pub(crate) fn open_external_app(_url: &str) -> bool {
    false
}

pub(crate) use content_filter::{
    compile as compile_content_policy, content_policy_digest, enumerate_content_policy_cache,
    install_scoped_on_view as install_scoped_content_policy_on_view,
    remove_content_policy_cache_identifier, same_policy as same_content_policy,
    ContentPolicyCacheMaintenanceCancellation, ContentPolicyCachePage,
    ContentPolicyCompilationCancellation, ContentPolicyRegistration, NativeContentPolicy,
};
pub(crate) use find::{find, FindReport, FindSession};
pub use stage::Stage;

use std::cell::RefCell;
use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use gtk::glib::prelude::{ObjectExt, ObjectType};
use gtk::glib::signal::{connect_raw, SignalHandlerId};
#[cfg(any(test, feature = "native-isolation-probes"))]
use gtk::glib::translate::FromGlibPtrFull;
use gtk::prelude::WidgetExt as _;
#[cfg(any(test, feature = "native-isolation-probes"))]
use webkit2gtk::UserContentManager;
use webkit2gtk::{
    DownloadExt, UserContentInjectedFrames, UserContentManagerExt, UserScript,
    UserScriptInjectionTime, WebContextExt, WebViewExt, WebsiteDataManagerExt,
};
use wry::WebViewExtUnix;
use zephium_core::ports::engine::{Partition, ScriptPrincipal};

#[cfg(any(test, feature = "native-isolation-probes"))]
#[allow(dead_code)]
const PRINCIPAL_MESSAGE_UTF16_LIMIT: usize = 64 * 1_024;
#[cfg(any(test, feature = "native-isolation-probes"))]
#[allow(dead_code)]
const PRINCIPAL_MESSAGE_UTF8_LIMIT: usize = 64 * 1_024;
const PRINCIPAL_WORLD_PREFIX: &str = "zephium-principal-";
#[cfg(any(test, feature = "native-isolation-probes"))]
const PRINCIPAL_HANDLER_PREFIX: &str = "zephiumPrincipal_";
const PRINCIPAL_TOKEN_MAX_BYTES: usize = "userscript-".len() + 32;

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(crate) struct PrincipalContentIdentity {
    principal: ScriptPrincipal,
    world_name: Box<str>,
    #[cfg(any(test, feature = "native-isolation-probes"))]
    handler_name: Box<str>,
}

impl PrincipalContentIdentity {
    pub(crate) fn new(principal: ScriptPrincipal) -> Self {
        let token = principal_token(principal);
        Self {
            principal,
            world_name: format!("{PRINCIPAL_WORLD_PREFIX}{token}").into(),
            #[cfg(any(test, feature = "native-isolation-probes"))]
            handler_name: format!("{PRINCIPAL_HANDLER_PREFIX}{token}").into(),
        }
    }

    #[allow(dead_code)]
    pub(crate) fn principal(&self) -> ScriptPrincipal {
        self.principal
    }

    pub(crate) fn world_name(&self) -> &str {
        &self.world_name
    }

    #[cfg(any(test, feature = "native-isolation-probes"))]
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
    let mut token = String::with_capacity(PRINCIPAL_TOKEN_MAX_BYTES);
    token.push_str(prefix);
    for byte in bytes {
        token.push(char::from(HEX[usize::from(byte >> 4)]));
        token.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    token
}

// Product builds intentionally do not compile the principal messaging bridge.
// JavaScriptCore converts and allocates the complete string before Rust can
// enforce either byte limit, and untrusted code in the principal world can
// otherwise invoke the raw native handler directly. Keep messaging unavailable
// until the native boundary can enforce a pre-allocation limit and expose a
// non-bypassable API.
#[cfg(any(test, feature = "native-isolation-probes"))]
#[derive(Clone, Debug, Eq, PartialEq)]
#[allow(dead_code)]
pub(crate) struct PrincipalContentMessage {
    identity: PrincipalContentIdentity,
    body: String,
}

#[cfg(any(test, feature = "native-isolation-probes"))]
#[allow(dead_code)]
impl PrincipalContentMessage {
    pub(crate) fn identity(&self) -> &PrincipalContentIdentity {
        &self.identity
    }

    pub(crate) fn body(&self) -> &str {
        &self.body
    }
}

// javascriptcore-rs is a private implementation dependency of Wry, not a
// Zephium API dependency. Use the stable JSC C ABI to perform the two bounded
// string operations needed at this native trust boundary.
#[cfg(any(test, feature = "native-isolation-probes"))]
#[allow(dead_code)]
unsafe extern "C" {
    fn jsc_value_is_string(value: *mut c_void) -> i32;
    fn jsc_value_to_string_as_bytes(value: *mut c_void) -> *mut gtk::glib::ffi::GBytes;
}

#[cfg(any(test, feature = "native-isolation-probes"))]
#[allow(dead_code)]
fn bounded_principal_message(result: &webkit2gtk::JavascriptResult) -> Option<String> {
    let value = result.js_value()?;
    let value = value.as_ptr().cast::<c_void>();
    if unsafe { jsc_value_is_string(value) } == 0 {
        return None;
    }
    let raw = unsafe { jsc_value_to_string_as_bytes(value) };
    if raw.is_null() {
        return None;
    }
    // SAFETY: jsc_value_to_string_as_bytes transfers one owned GBytes
    // reference. FromGlibPtrFull consumes exactly that reference.
    let bytes = unsafe { gtk::glib::Bytes::from_glib_full(raw) };
    bounded_principal_message_bytes(bytes.as_ref())
}

#[cfg(any(test, feature = "native-isolation-probes"))]
#[allow(dead_code)]
fn bounded_principal_message_bytes(bytes: &[u8]) -> Option<String> {
    if bytes.len() > PRINCIPAL_MESSAGE_UTF8_LIMIT {
        return None;
    }
    let body = std::str::from_utf8(bytes).ok()?;
    if body.encode_utf16().count() > PRINCIPAL_MESSAGE_UTF16_LIMIT {
        return None;
    }
    Some(body.to_owned())
}

#[cfg(any(test, feature = "native-isolation-probes"))]
#[allow(dead_code)]
fn dispatch_principal_message(
    identity: &PrincipalContentIdentity,
    body: String,
    on_message: &dyn Fn(PrincipalContentMessage),
) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        on_message(PrincipalContentMessage {
            identity: identity.clone(),
            body,
        });
    }));
}

/// Owns one world-scoped native message registration. A unique handler name
/// is derived from the typed principal, so the detailed GLib signal itself is
/// the authority; no identity supplied by JavaScript is consulted.
#[cfg(any(test, feature = "native-isolation-probes"))]
#[allow(dead_code)]
pub(crate) struct PrincipalMessageHandlerRegistration {
    identity: PrincipalContentIdentity,
    manager: UserContentManager,
    signal: Option<SignalHandlerId>,
}

#[cfg(any(test, feature = "native-isolation-probes"))]
#[allow(dead_code)]
impl PrincipalMessageHandlerRegistration {
    pub(crate) fn identity(&self) -> &PrincipalContentIdentity {
        &self.identity
    }
}

#[cfg(any(test, feature = "native-isolation-probes"))]
impl Drop for PrincipalMessageHandlerRegistration {
    fn drop(&mut self) {
        self.manager.unregister_script_message_handler_in_world(
            self.identity.handler_name(),
            self.identity.world_name(),
        );
        if let Some(signal) = self.signal.take() {
            self.manager.disconnect(signal);
        }
    }
}

#[cfg(any(test, feature = "native-isolation-probes"))]
#[allow(dead_code)]
pub(crate) fn register_principal_message_handler(
    view: &wry::WebView,
    principal: ScriptPrincipal,
    on_message: impl Fn(PrincipalContentMessage) + 'static,
) -> Result<PrincipalMessageHandlerRegistration, String> {
    let identity = PrincipalContentIdentity::new(principal);
    let manager = view
        .webview()
        .user_content_manager()
        .ok_or_else(|| "content WebKitGTK view has no UserContentManager".to_owned())?;
    let callback_identity = identity.clone();
    let signal = manager.connect_script_message_received(
        Some(identity.handler_name()),
        move |_manager, result| {
            let Some(body) = bounded_principal_message(result) else {
                return;
            };
            dispatch_principal_message(&callback_identity, body, &on_message);
        },
    );
    if !manager
        .register_script_message_handler_in_world(identity.handler_name(), identity.world_name())
    {
        manager.disconnect(signal);
        return Err(format!(
            "cannot register isolated handler for principal {:?}",
            identity.principal()
        ));
    }
    Ok(PrincipalMessageHandlerRegistration {
        identity,
        manager,
        signal: Some(signal),
    })
}

thread_local! {
    static CONTAINER: RefCell<Option<gtk::Fixed>> = const { RefCell::new(None) };
}

pub(crate) struct ContentPolicyTimeout {
    task: gtk::glib::JoinHandle<()>,
    fired: Rc<std::cell::Cell<bool>>,
}

impl ContentPolicyTimeout {
    pub(crate) fn cancel(self) {
        drop(self);
    }
}

impl Drop for ContentPolicyTimeout {
    fn drop(&mut self) {
        if !self.fired.get() {
            self.task.abort();
        }
    }
}

pub(crate) fn schedule_content_policy_timeout(
    duration: Duration,
    callback: impl FnOnce() + 'static,
) -> Option<ContentPolicyTimeout> {
    let context = gtk::glib::MainContext::ref_thread_default();
    if !context.is_owner() {
        return None;
    }
    let fired = Rc::new(std::cell::Cell::new(false));
    let callback_fired = fired.clone();
    let task = context.spawn_local(async move {
        gtk::glib::timeout_future(duration).await;
        // Mark first: timeout settlement takes and drops this exact watchdog
        // while its local task is still executing.
        callback_fired.set(true);
        callback();
    });
    Some(ContentPolicyTimeout { task, fired })
}

pub fn install_container(fixed: gtk::Fixed) -> Result<(), String> {
    CONTAINER.with(|cell| {
        let mut container = cell
            .try_borrow_mut()
            .map_err(|_| "WebKitGTK container is re-entrantly borrowed".to_owned())?;
        if container.is_some() {
            return Err("WebKitGTK container is already installed".to_owned());
        }
        *container = Some(fixed);
        Ok(())
    })
}

// This is deliberately a runtime check because Linux dynamically supplies
// WebKitGTK, so a successful build says nothing about the engine a user's
// machine will actually load. The pure version/deadline policy lives in core
// so CI and runtime cannot silently drift.

pub fn enforce_runtime_security_floor(
) -> Result<zephium_core::runtime_security::RuntimeSecurityAdvisories, String> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|_| {
            "system clock is before the Unix epoch; cannot assess WebKitGTK runtime security"
                .to_owned()
        })?
        .as_secs();
    enforce_runtime_preconditions(now, || {
        std::env::vars_os().find_map(|(name, _)| {
            let name = name.to_str()?;
            zephium_core::webkitgtk::environment_override_is_security_relevant(name)
                .then(|| name.to_owned())
        })
    })?;
    // SAFETY: These no-argument WebKitGTK ABI functions return immutable
    // library version constants and are safe after the library is loaded.
    let version = unsafe {
        (
            webkit2gtk::ffi::webkit_get_major_version(),
            webkit2gtk::ffi::webkit_get_minor_version(),
            webkit2gtk::ffi::webkit_get_micro_version(),
        )
    };
    enforce_runtime_version(version, now)
}

fn enforce_runtime_preconditions(
    unix_seconds: u64,
    security_override: impl FnOnce() -> Option<String>,
) -> Result<(), String> {
    if let Some(name) = security_override() {
        return Err(format!(
            "security-relevant WebKitGTK/JavaScriptCore environment override {name} is present; unset it before starting Zephium"
        ));
    }
    if unix_seconds < zephium_core::webkitgtk::SECURITY_FLOOR_PUBLISHED_UNIX_SECONDS {
        return Err(format!(
            "system clock predates the reviewed WebKitGTK security release {}; correct the clock before browsing",
            zephium_core::webkitgtk::SECURITY_FLOOR_PUBLISHED_ON,
        ));
    }
    Ok(())
}

fn enforce_runtime_version(
    version: (u32, u32, u32),
    unix_seconds: u64,
) -> Result<zephium_core::runtime_security::RuntimeSecurityAdvisories, String> {
    zephium_core::webkitgtk::assess_runtime(
        version.0,
        version.1,
        version.2,
        unix_seconds,
    )
    .map_err(|error| {
        format!(
            "{error}; latest stable {} was reviewed on {} (security floor source: {}; latest release source: {}). Install a supported stable WebKitGTK runtime before starting Zephium",
            zephium_core::webkitgtk::LATEST_REVIEWED_TEXT,
            zephium_core::webkitgtk::LATEST_REVIEWED_PUBLISHED_ON,
            zephium_core::webkitgtk::SECURITY_FLOOR_SOURCE_URL,
            zephium_core::webkitgtk::LATEST_REVIEWED_SOURCE_URL,
        )
    })
}

pub fn container() -> Option<gtk::Fixed> {
    CONTAINER.with(|cell| cell.borrow().clone())
}

pub fn configure(
    webview: &wry::WebView,
    _radius: f64,
    partition: Partition,
    expected_data_directory: Option<&Path>,
) -> Result<(), String> {
    let view = webview.webview();
    // The host deliberately builds without an initial URL and calls this
    // before its first load. WebKit has therefore not launched a web process,
    // which is the required point for context-wide process policy.
    let context = view
        .context()
        .ok_or_else(|| "content WebKitGTK view has no WebContext".to_owned())?;
    if !context.is_sandbox_enabled() {
        return Err(
            "WebKitGTK content-process sandbox was not enabled at context construction".into(),
        );
    }
    if !context.is_process_swap_on_cross_site_navigation_enabled() {
        return Err("WebKitGTK cross-site Web-process swapping is disabled".into());
    }

    let expected_ephemeral = matches!(partition, Partition::Ephemeral(_));
    if view.is_ephemeral() != expected_ephemeral {
        return Err("WebKitGTK WebView persistence mode does not match its partition".into());
    }
    if context.is_ephemeral() != expected_ephemeral {
        return Err("WebKitGTK WebContext persistence mode does not match its partition".into());
    }
    attest_website_data_manager(&view, &context, partition, expected_data_directory)?;

    // `download-started` belongs to WebContext, not WebView. Persistent tabs
    // share a context, so registering Wry's per-builder callback on every tab
    // would retain one more closure for the lifetime of the profile. Mark the
    // GLib object and install one fail-closed handler before the first load.
    const DOWNLOAD_DENY_MARKER: &str = "zephium-download-deny-installed";
    // SAFETY: this private key is written and read only as `bool` here, and
    // the marker lives exactly as long as the context GLib object.
    let installed = unsafe { context.data::<bool>(DOWNLOAD_DENY_MARKER).is_some() };
    if !installed {
        context.connect_download_started(|_, download| download.cancel());
        // SAFETY: see the typed private-key invariant above.
        unsafe { context.set_data(DOWNLOAD_DENY_MARKER, true) };
    }

    Ok(())
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
) -> Result<UserScriptInjectionTime, String> {
    use zephium_core::ports::engine::RunAt;

    match run_at {
        RunAt::DocumentStart => Ok(UserScriptInjectionTime::Start),
        RunAt::DocumentEnd => Ok(UserScriptInjectionTime::End),
        RunAt::DocumentIdle => Err("document_idle scheduling is not implemented on Linux".into()),
    }
}

fn validated_native_injection_time(
    script: &zephium_core::ports::engine::UserScript,
) -> Result<UserScriptInjectionTime, String> {
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
            "pre-source per-frame match-set enforcement is not implemented on Linux".into(),
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

fn injected_frames(all_frames: bool) -> UserContentInjectedFrames {
    if all_frames {
        UserContentInjectedFrames::AllFrames
    } else {
        UserContentInjectedFrames::TopFrame
    }
}

fn add_page_user_script(
    view: &wry::WebView,
    script: &zephium_core::ports::engine::UserScript,
) -> Result<(), String> {
    use zephium_core::ports::engine::{ScriptOwner, World};

    if script.owner != ScriptOwner::Builtin || script.world != World::Page {
        return Err("page-world scripts must be owned by the builtin principal".into());
    }
    let time = validated_native_injection_time(script)?;
    let manager = view
        .webview()
        .user_content_manager()
        .ok_or_else(|| "content WebKitGTK view has no UserContentManager".to_owned())?;
    let native = UserScript::new(
        script.source.as_ref(),
        injected_frames(script.all_frames),
        time,
        &[],
        &[],
    );
    manager.add_script(&native);
    Ok(())
}

pub(crate) fn add_user_script_in_principal_world(
    view: &wry::WebView,
    script: &zephium_core::ports::engine::UserScript,
) -> Result<(), String> {
    use zephium_core::ports::engine::{ScriptOwner, World};

    let World::Isolated(principal) = script.world else {
        return Err("principal-world installer accepts only isolated scripts".into());
    };
    if script.owner != ScriptOwner::Principal(principal) {
        return Err("isolated script owner does not match its native world principal".into());
    }
    let time = validated_native_injection_time(script)?;
    let identity = PrincipalContentIdentity::new(principal);
    let manager = view
        .webview()
        .user_content_manager()
        .ok_or_else(|| "content WebKitGTK view has no UserContentManager".to_owned())?;
    let native = UserScript::for_world(
        script.source.as_ref(),
        injected_frames(script.all_frames),
        time,
        identity.world_name(),
        &[],
        &[],
    );
    manager.add_script(&native);
    Ok(())
}

fn attest_website_data_manager(
    view: &webkit2gtk::WebView,
    context: &webkit2gtk::WebContext,
    partition: Partition,
    expected_data_directory: Option<&Path>,
) -> Result<(), String> {
    let context_manager = context
        .website_data_manager()
        .ok_or_else(|| "content WebKitGTK context has no WebsiteDataManager".to_owned())?;
    let manager = view
        .website_data_manager()
        .ok_or_else(|| "content WebKitGTK view has no WebsiteDataManager".to_owned())?;
    if context_manager.as_ptr() != manager.as_ptr() {
        return Err("WebKitGTK view and context use different data managers".into());
    }

    let actual_data =
        canonical_reported_directory("base data", manager.base_data_directory().as_deref())?;
    let actual_cache =
        canonical_reported_directory("base cache", manager.base_cache_directory().as_deref())?;
    let expected = expected_data_directory
        .map(|path| direct_canonical_directory("expected profile", path))
        .transpose()?;

    validate_storage_postcondition(
        partition,
        manager.is_ephemeral(),
        expected.as_deref(),
        actual_data.as_deref(),
        actual_cache.as_deref(),
    )
}

fn canonical_reported_directory(
    kind: &str,
    reported: Option<&str>,
) -> Result<Option<PathBuf>, String> {
    reported
        .map(|reported| direct_canonical_directory(kind, Path::new(reported)))
        .transpose()
}

fn direct_canonical_directory(kind: &str, path: &Path) -> Result<PathBuf, String> {
    let metadata = std::fs::symlink_metadata(path).map_err(|error| {
        format!(
            "cannot inspect WebKitGTK {kind} directory {}: {error}",
            path.display()
        )
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(format!(
            "WebKitGTK {kind} path is not a direct directory: {}",
            path.display()
        ));
    }
    let canonical = path.canonicalize().map_err(|error| {
        format!(
            "cannot canonicalize WebKitGTK {kind} directory {}: {error}",
            path.display()
        )
    })?;
    // Reject relative spellings, `..`, and symlinked ancestors. A path that
    // merely resolves to the expected profile is not a stable storage
    // boundary because the alias can be retargeted after admission.
    if canonical != path {
        return Err(format!(
            "WebKitGTK {kind} path is not its direct canonical identity: {}",
            path.display()
        ));
    }
    Ok(canonical)
}

fn validate_storage_postcondition(
    partition: Partition,
    manager_is_ephemeral: bool,
    expected_directory: Option<&Path>,
    actual_data_directory: Option<&Path>,
    actual_cache_directory: Option<&Path>,
) -> Result<(), String> {
    match partition {
        Partition::Ephemeral(_) => {
            if !manager_is_ephemeral {
                return Err("ephemeral view received a durable WebKitGTK data manager".into());
            }
            if expected_directory.is_some()
                || actual_data_directory.is_some()
                || actual_cache_directory.is_some()
            {
                return Err(
                    "ephemeral WebKitGTK data manager exposes persistent base directories".into(),
                );
            }
        }
        Partition::Default(_) | Partition::Persistent(_) => {
            if manager_is_ephemeral {
                return Err("durable view received an ephemeral WebKitGTK data manager".into());
            }
            let expected = expected_directory
                .ok_or_else(|| "durable WebKitGTK view has no expected profile path".to_owned())?;
            if actual_data_directory != Some(expected) || actual_cache_directory != Some(expected) {
                return Err(
                    "WebKitGTK data/cache directories do not match the prepared profile path"
                        .into(),
                );
            }
        }
    }
    Ok(())
}

pub fn stop_loading(view: &wry::WebView) {
    view.webview().stop_loading();
}

/// WebKitGTK exposes renderer audio activity as a native property. A discard
/// probe must combine this with the DOM report so page-script tampering cannot
/// make an actually audible document look idle.
/// Focus does not cover pages on Linux, which is outside the release.
pub fn set_media_suspended(_view: &wry::WebView, _suspended: bool) {}

pub fn query_document_activity(view: &wry::WebView, done: impl FnOnce(bool) + 'static) -> bool {
    done(!view.webview().is_playing_audio());
    true
}

/// GLib signal registrations owned alongside the Wry WebView. The callbacks
/// capture only the item-id thunk supplied by the host; these strong native
/// handles exist solely so Drop can disconnect before the WebView is released.
pub struct NavigationObserver {
    webview: webkit2gtk::WebView,
    uri_token: Option<SignalHandlerId>,
    history: webkit2gtk::BackForwardList,
    history_token: Option<SignalHandlerId>,
}

pub type InstalledNavigationObserver = NavigationObserver;

pub fn install_navigation_observer(
    webview: &wry::WebView,
    on_change: impl Fn() + 'static,
) -> Result<NavigationObserver, &'static str> {
    let view = webview.webview();
    let history = view
        .back_forward_list()
        .ok_or("WebKitGTK did not provide a back-forward list")?;
    let on_change: Rc<dyn Fn()> = Rc::new(on_change);

    // `notify::uri` is emitted for the active main-frame URI, including
    // fragment and History API source changes.
    let on_uri = on_change.clone();
    let uri_token = view.connect_uri_notify(move |_| on_uri());

    // The generated bindings omit BackForwardList::changed because its GList
    // argument is untyped. Connect to the documented signal ABI directly; we
    // intentionally ignore all three native pointer arguments and query the
    // authoritative WebView state through the host after the callback.
    let history_token = connect_history_changed(&history, move || on_change());

    Ok(NavigationObserver {
        webview: view,
        uri_token: Some(uri_token),
        history,
        history_token: Some(history_token),
    })
}

fn connect_history_changed<F: Fn() + 'static>(
    history: &webkit2gtk::BackForwardList,
    callback: F,
) -> SignalHandlerId {
    unsafe extern "C" fn trampoline<F: Fn() + 'static>(
        _history: *mut c_void,
        _item_added: *mut c_void,
        _items_removed: *mut c_void,
        callback: gtk::glib::ffi::gpointer,
    ) {
        // SAFETY: connect_raw owns this boxed F until it disconnects or the
        // BackForwardList is finalized, and GLib invokes the trampoline with
        // the same user-data pointer.
        let callback = unsafe { &*(callback.cast::<F>()) };
        callback();
    }

    // SAFETY: WebKitBackForwardList::changed has three pointer parameters
    // followed by user_data. Pointer pointee types do not affect the C ABI,
    // and the trampoline never reads them. connect_raw installs the matching
    // destructor for the boxed closure.
    unsafe {
        let callback = Box::new(callback);
        connect_raw(
            history.as_ptr().cast(),
            c"changed".as_ptr(),
            Some(std::mem::transmute::<*const (), unsafe extern "C" fn()>(
                trampoline::<F> as *const (),
            )),
            Box::into_raw(callback),
        )
    }
}

impl Drop for NavigationObserver {
    fn drop(&mut self) {
        if let Some(token) = self.uri_token.take() {
            self.webview.disconnect(token);
        }
        if let Some(token) = self.history_token.take() {
            self.history.disconnect(token);
        }
    }
}

pub fn current_url(view: &wry::WebView) -> Option<String> {
    const PAGE_URL_UTF8_LIMIT: usize = 8 * 1_024;
    let uri = view.webview().uri()?;
    (uri.as_str().len() <= PAGE_URL_UTF8_LIMIT).then(|| uri.to_string())
}

pub fn enforce_navigation_pending(view: &wry::WebView) -> bool {
    // Keep WebKitGTK mapped so its compositing surface survives the gate;
    // opacity is the native non-painting primitive used by the stage and by
    // Wry's synchronous commit guard.
    let widget = view.webview();
    widget.set_opacity(0.0);
    widget.opacity() == 0.0
}

/// Strong native storage handles that must outlive every view created from
/// the context. A successful Wry build is not enough proof: later hardening,
/// observer, or first-load steps can still fail after WebKitGTK has created a
/// WebsiteDataManager and touched profile storage.
pub(crate) struct WebsiteDataManagerObligation {
    pub(crate) managers: Vec<webkit2gtk::WebsiteDataManager>,
    pub(crate) provenance_complete: bool,
}

pub(crate) fn website_data_manager_obligation(view: &wry::WebView) -> WebsiteDataManagerObligation {
    let view = view.webview();
    let context_manager = view
        .context()
        .and_then(|context| context.website_data_manager());
    let view_manager = view.website_data_manager();
    let provenance_complete = context_manager
        .as_ref()
        .zip(view_manager.as_ref())
        .is_some_and(|(context, view)| context.as_ptr() == view.as_ptr());

    // Preserve every native object even when the view/context relationship is
    // malformed. The host marks that profile unverifiable and will not delete
    // its directory, but retaining the objects prevents a subsequent empty
    // retry from fabricating absence.
    let mut managers = Vec::with_capacity(2);
    for manager in [context_manager, view_manager].into_iter().flatten() {
        if !managers
            .iter()
            .any(|existing: &webkit2gtk::WebsiteDataManager| existing.as_ptr() == manager.as_ptr())
        {
            managers.push(manager);
        }
    }
    WebsiteDataManagerObligation {
        managers,
        provenance_complete,
    }
}

/// Clear every native data manager retained from this profile after its views
/// and WebContexts have been released, then remove and verify the owned disk
/// directory on a worker. Incognito WebKitGTK views each own an independent
/// ephemeral manager, so all managers must acknowledge the clear operation.
pub(crate) fn erase_profile_data(
    mut managers: Vec<webkit2gtk::WebsiteDataManager>,
    manager_provenance_valid: bool,
    roots: Vec<std::path::PathBuf>,
    profile: zephium_core::ids::ProfileId,
    completion: Arc<crate::erasure::Completion>,
) {
    use webkit2gtk::{WebsiteDataManagerExt, WebsiteDataManagerExtManual, WebsiteDataTypes};

    let mut seen = std::collections::HashSet::new();
    managers.retain(|manager| seen.insert(manager.as_ptr() as usize));
    if managers.is_empty() {
        if manager_provenance_valid {
            remove_linux_profile_directories_async(roots, profile, completion);
        } else {
            completion.finish(zephium_core::ports::engine::ProfileDataErasureOutcome::Failed);
        }
        return;
    }

    let remaining = Arc::new(AtomicUsize::new(managers.len()));
    // Missing/mismatched native manager provenance is sticky. Known managers
    // are still cleared and fetched best-effort, but disk deletion is denied.
    let failed = Arc::new(AtomicBool::new(!manager_provenance_valid));
    for manager in managers {
        // clear() requires a Send callback even though WebKit invokes it on
        // this GTK main context. SendWeakRef is the binding's thread-checked
        // bridge; upgrading it in the callback also proves the native source
        // object survived long enough to start verification.
        let manager_ref: gtk::glib::SendWeakRef<webkit2gtk::WebsiteDataManager> =
            manager.downgrade().into();
        let remaining = remaining.clone();
        let failed = failed.clone();
        let roots = roots.clone();
        let completion = completion.clone();
        manager.clear(
            WebsiteDataTypes::ALL,
            gtk::glib::TimeSpan::from_seconds(0),
            None::<&gtk::gio::Cancellable>,
            move |result| {
                if result.is_err() {
                    failed.store(true, Ordering::Release);
                    complete_linux_manager_erasure(&remaining, &failed, roots, profile, completion);
                    return;
                }
                let Some(manager) = manager_ref.upgrade() else {
                    failed.store(true, Ordering::Release);
                    complete_linux_manager_erasure(&remaining, &failed, roots, profile, completion);
                    return;
                };
                let retained_manager = manager.clone();
                manager.fetch(
                    WebsiteDataTypes::ALL,
                    None::<&gtk::gio::Cancellable>,
                    move |result| {
                        // Keep a strong native manager reference through the
                        // fetch callback; dropping it earlier could turn an
                        // ephemeral-context verification into a use-after-
                        // release race hidden by an empty directory.
                        drop(retained_manager);
                        if !matches!(result, Ok(ref records) if records.is_empty()) {
                            failed.store(true, Ordering::Release);
                        }
                        complete_linux_manager_erasure(
                            &remaining, &failed, roots, profile, completion,
                        );
                    },
                );
            },
        );
    }
}

fn complete_linux_manager_erasure(
    remaining: &AtomicUsize,
    failed: &AtomicBool,
    roots: Vec<std::path::PathBuf>,
    profile: zephium_core::ids::ProfileId,
    completion: Arc<crate::erasure::Completion>,
) {
    if remaining.fetch_sub(1, Ordering::AcqRel) != 1 {
        return;
    }
    if failed.load(Ordering::Acquire) {
        completion.finish(zephium_core::ports::engine::ProfileDataErasureOutcome::Failed);
    } else {
        remove_linux_profile_directories_async(roots, profile, completion);
    }
}

fn remove_linux_profile_directories_async(
    roots: Vec<std::path::PathBuf>,
    profile: zephium_core::ids::ProfileId,
    completion: Arc<crate::erasure::Completion>,
) {
    let attempt = completion.attempt_flag();
    // Capture the GTK owner's thread-default context while still inside the
    // native callback. A process-global default source could be dispatched by
    // the wrong loop in embedded/multi-context hosts.
    let owner_context = gtk::glib::MainContext::ref_thread_default();
    let failed = completion.clone();
    let task = move || {
        let verified = crate::erasure::remove_profile_directories_verified(&roots, profile);
        completion.finish(if verified {
            zephium_core::ports::engine::ProfileDataErasureOutcome::Verified
        } else {
            zephium_core::ports::engine::ProfileDataErasureOutcome::Failed
        });
        if verified {
            // WebsiteDataManager is main-thread-bound. Ask the GTK owner to
            // release the retained strong handles only after this exact
            // attempt verified disk absence. Generation matching in the host
            // prevents a late callback from erasing a newer retry's proof.
            owner_context.invoke(move || {
                crate::host::release_linux_erasure_obligations(profile, attempt);
            });
        }
    };
    if let Err(error) = std::thread::Builder::new()
        .name("zephium-linux-profile-delete".into())
        .spawn(task)
    {
        eprintln!("privacy: cannot start Linux profile-directory deletion: {error}");
        failed.finish(zephium_core::ports::engine::ProfileDataErasureOutcome::Failed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gtk::prelude::{ContainerExt, WidgetExt};
    use std::cell::{Cell, RefCell};
    use std::collections::{HashMap, HashSet};
    use std::rc::Rc;
    use std::sync::atomic::AtomicBool;
    use std::sync::{mpsc, Arc};
    use std::time::{Duration, Instant};
    use webkit2gtk::{WebContextExt, WebViewExt};
    use wry::{WebViewBuilder, WebViewBuilderExtUnix, WebViewExtUnix};
    use zephium_core::ids::{ExtensionInstallId, ProfileId, ScriptId, UserscriptId};
    use zephium_core::injection::{MatchOptions, MatchSet};
    use zephium_core::ports::engine::{
        RunAt, ScriptOwner, ScriptPrincipal, UserScript as EngineUserScript, World,
    };

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
        let exact = vec![b'a'; PRINCIPAL_MESSAGE_UTF8_LIMIT];
        assert_eq!(
            bounded_principal_message_bytes(&exact).as_deref(),
            std::str::from_utf8(&exact).ok()
        );
        let mut over = exact;
        over.push(b'a');
        assert!(bounded_principal_message_bytes(&over).is_none());
        assert!(bounded_principal_message_bytes(&[0xff]).is_none());

        let utf8_over = "🙂".repeat(PRINCIPAL_MESSAGE_UTF8_LIMIT / 4 + 1);
        assert!(utf8_over.encode_utf16().count() < PRINCIPAL_MESSAGE_UTF16_LIMIT);
        assert!(bounded_principal_message_bytes(utf8_over.as_bytes()).is_none());
    }

    #[test]
    fn native_install_validation_refuses_unenforced_semantics() {
        let principal = test_principals()[0];
        let baseline = EngineUserScript {
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
    fn content_policy_watchdog_is_owner_context_local_and_cancelable() {
        let context = gtk::glib::MainContext::new();
        let owner_thread = std::thread::current().id();
        let fired_on = Rc::new(Cell::new(None));
        let callback_fired_on = fired_on.clone();
        context
            .with_thread_default(|| {
                let timeout =
                    schedule_content_policy_timeout(Duration::from_millis(1), move || {
                        callback_fired_on.set(Some(std::thread::current().id()))
                    })
                    .expect("owned thread-default context must admit its watchdog");
                while fired_on.get().is_none() {
                    assert!(context.iteration(true));
                }
                drop(timeout);
            })
            .expect("test must own its isolated GLib context");
        assert_eq!(fired_on.get(), Some(owner_thread));

        let canceled = Rc::new(Cell::new(false));
        let callback_canceled = canceled.clone();
        context
            .with_thread_default(|| {
                let timeout =
                    schedule_content_policy_timeout(Duration::from_millis(1), move || {
                        callback_canceled.set(true)
                    })
                    .expect("owned thread-default context must admit its watchdog");
                drop(timeout);
                std::thread::sleep(Duration::from_millis(5));
                while context.pending() {
                    assert!(context.iteration(false));
                }
            })
            .expect("test must reacquire its isolated GLib context");
        assert!(!canceled.get());
    }

    #[test]
    fn content_policy_watchdog_never_falls_back_to_the_process_default_context() {
        let source = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/src/platform/linux/mod.rs"
        ));
        let schedule = source
            .split_once("pub(crate) fn schedule_content_policy_timeout")
            .expect("watchdog scheduler disappeared")
            .1
            .split_once("pub fn install_container")
            .expect("watchdog scheduler boundary disappeared")
            .0;
        assert!(schedule.contains("MainContext::ref_thread_default()"));
        assert!(schedule.contains("if !context.is_owner()"));
        assert!(schedule.contains("context.spawn_local("));
        assert!(schedule.contains("timeout_future(duration)"));
        assert!(!schedule.contains("timeout_add_local_once"));
    }

    fn proc_parent_map() -> HashMap<u32, u32> {
        let mut parents = HashMap::new();
        for entry in std::fs::read_dir("/proc").expect("Linux procfs is required") {
            let Ok(entry) = entry else { continue };
            let Some(pid) = entry
                .file_name()
                .to_str()
                .and_then(|name| name.parse::<u32>().ok())
            else {
                continue;
            };
            let Ok(status) = std::fs::read_to_string(entry.path().join("status")) else {
                continue;
            };
            let Some(parent) = status.lines().find_map(|line| {
                line.strip_prefix("PPid:")
                    .and_then(|value| value.trim().parse::<u32>().ok())
            }) else {
                continue;
            };
            parents.insert(pid, parent);
        }
        parents
    }

    fn descendants_of(root: u32) -> HashSet<u32> {
        let parents = proc_parent_map();
        let mut descendants = HashSet::from([root]);
        let mut changed = true;
        while changed {
            changed = false;
            for (&pid, &parent) in &parents {
                if descendants.contains(&parent) && descendants.insert(pid) {
                    changed = true;
                }
            }
        }
        descendants.remove(&root);
        descendants
    }

    fn web_process_descendant(root: u32) -> Option<u32> {
        descendants_of(root).into_iter().find(|pid| {
            std::fs::read(format!("/proc/{pid}/cmdline"))
                .ok()
                .map(|bytes| {
                    String::from_utf8_lossy(&bytes)
                        .replace('\0', " ")
                        .contains("WebKitWebProcess")
                })
                .unwrap_or(false)
        })
    }

    fn wait_for_web_process(root: u32) -> u32 {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            while gtk::events_pending() {
                gtk::main_iteration_do(false);
            }
            if let Some(pid) = web_process_descendant(root) {
                return pid;
            }
            assert!(
                Instant::now() < deadline,
                "WebKitGTK did not spawn a descendant WebKitWebProcess"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn proc_status_value<'a>(status: &'a str, key: &str) -> Option<&'a str> {
        status
            .lines()
            .find_map(|line| line.strip_prefix(key).map(str::trim))
    }

    fn prove_web_process_namespace_and_filter_state(pid: u32) {
        let parent =
            std::fs::read_to_string("/proc/self/status").expect("read browser process status");
        let status = std::fs::read_to_string(format!("/proc/{pid}/status"))
            .expect("read WebKitWebProcess status");
        assert_eq!(
            proc_status_value(&parent, "NoNewPrivs:"),
            Some("1"),
            "native confinement probe must enter through a no-new-privileges launcher"
        );
        assert_eq!(
            proc_status_value(&status, "NoNewPrivs:"),
            Some("1"),
            "WebKitWebProcess must forbid privilege escalation"
        );
        assert_eq!(
            proc_status_value(&status, "Seccomp:"),
            Some("2"),
            "WebKitWebProcess must run under a seccomp filter"
        );
        assert!(
            renderer_filter_count_exceeds_parent(&parent, &status),
            "renderer filter count must exceed its parent; inheritance alone is not evidence"
        );

        for namespace in ["mnt", "user", "pid"] {
            let host_namespace = std::fs::read_link(format!("/proc/self/ns/{namespace}"))
                .unwrap_or_else(|error| panic!("read host {namespace} namespace: {error}"));
            let renderer_namespace = std::fs::read_link(format!("/proc/{pid}/ns/{namespace}"))
                .unwrap_or_else(|error| {
                    panic!("read WebKitWebProcess {namespace} namespace: {error}")
                });
            assert_ne!(
                host_namespace, renderer_namespace,
                "WebKitWebProcess must not share the browser's {namespace} namespace"
            );
        }

        // Reading /proc/<pid>/root from this observer cannot prove renderer
        // path denial: ptrace/userns access checks may reject the observer even
        // when the renderer can read the file. Filesystem denial still needs
        // an in-renderer (or equivalent-credential) native qualification probe.
        // Filter counts likewise identify neither the installer nor its policy.
    }

    fn renderer_filter_count_exceeds_parent(parent: &str, renderer: &str) -> bool {
        let count = |status| {
            proc_status_value(status, "Seccomp_filters:")
                .and_then(|value| value.parse::<u64>().ok())
        };
        matches!((count(parent), count(renderer)), (Some(parent), Some(renderer)) if renderer > parent)
    }

    #[test]
    fn renderer_seccomp_evidence_rejects_inherited_missing_or_invalid_filters() {
        let status = |count| format!("Seccomp:\t2\nSeccomp_filters:\t{count}\n");
        assert!(renderer_filter_count_exceeds_parent(&status(0), &status(1)));
        assert!(renderer_filter_count_exceeds_parent(&status(2), &status(3)));
        for (parent, renderer) in [
            (status(1), status(1)),
            (status(2), status(1)),
            (status(0), "Seccomp:\t2\n".into()),
            ("".into(), status(1)),
            ("Seccomp_filters:\tinvalid\n".into(), status(1)),
            (status(0), "Seccomp_filters:\t18446744073709551616\n".into()),
        ] {
            assert!(!renderer_filter_count_exceeds_parent(&parent, &renderer));
        }
    }

    #[test]
    fn storage_postcondition_rejects_mode_and_path_mismatches() {
        let profile = ProfileId::from(7);
        let expected = Path::new("/owned/profile");
        let other = Path::new("/other/profile");

        assert!(validate_storage_postcondition(
            Partition::Persistent(profile),
            false,
            Some(expected),
            Some(expected),
            Some(expected),
        )
        .is_ok());
        assert!(validate_storage_postcondition(
            Partition::Ephemeral(profile),
            true,
            None,
            None,
            None,
        )
        .is_ok());

        assert!(validate_storage_postcondition(
            Partition::Persistent(profile),
            true,
            Some(expected),
            Some(expected),
            Some(expected),
        )
        .is_err());
        assert!(validate_storage_postcondition(
            Partition::Persistent(profile),
            false,
            Some(expected),
            Some(expected),
            Some(other),
        )
        .is_err());
        assert!(validate_storage_postcondition(
            Partition::Persistent(profile),
            false,
            None,
            Some(expected),
            Some(expected),
        )
        .is_err());
        assert!(validate_storage_postcondition(
            Partition::Ephemeral(profile),
            false,
            None,
            None,
            None,
        )
        .is_err());
        assert!(validate_storage_postcondition(
            Partition::Ephemeral(profile),
            true,
            None,
            Some(expected),
            None,
        )
        .is_err());
    }

    #[cfg(unix)]
    #[test]
    fn reported_storage_directory_rejects_alias_spellings() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().unwrap();
        let expected = temp.path().join("expected");
        let alias = temp.path().join("alias");
        let parent_alias = temp.path().join("parent-alias");
        let parent_child = expected.join("child");
        std::fs::create_dir(&expected).unwrap();
        std::fs::create_dir(&parent_child).unwrap();
        symlink(&expected, &alias).unwrap();
        symlink(&expected, &parent_alias).unwrap();

        assert!(direct_canonical_directory("base data", &expected).is_ok());
        assert!(direct_canonical_directory("base data", &alias).is_err());
        assert!(direct_canonical_directory("base data", &parent_alias.join("child")).is_err());
        assert!(
            direct_canonical_directory("base data", &parent_child.join("..").join("child"))
                .is_err()
        );
    }

    #[test]
    fn webkitgtk_floor_matches_the_reviewed_security_advisory() {
        let reviewed_at = zephium_core::webkitgtk::SECURITY_FLOOR_PUBLISHED_UNIX_SECONDS;
        assert!(enforce_runtime_version((2, 52, 6), reviewed_at).is_err());
        assert!(enforce_runtime_version((2, 53, 92), reviewed_at).is_err());
        assert_eq!(
            enforce_runtime_version((2, 54, 1), reviewed_at),
            Ok(zephium_core::runtime_security::RuntimeSecurityAdvisories::new())
        );
        assert!(enforce_runtime_version((2, 55, 0), reviewed_at).is_err());
        assert_eq!(
            enforce_runtime_version((2, 56, 0), reviewed_at),
            Ok(
                zephium_core::runtime_security::RuntimeSecurityAdvisories::from_advisory(
                    zephium_core::runtime_security::RuntimeSecurityAdvisory::unreviewed_runtime(),
                ),
            )
        );
        assert!(enforce_runtime_version((3, 0, 0), reviewed_at).is_err());
    }

    #[test]
    fn runtime_preconditions_reject_clock_rollback_and_sandbox_overrides() {
        assert!(enforce_runtime_preconditions(
            zephium_core::webkitgtk::SECURITY_FLOOR_PUBLISHED_UNIX_SECONDS - 1,
            || None,
        )
        .unwrap_err()
        .contains("system clock predates"));
        assert!(enforce_runtime_preconditions(
            zephium_core::webkitgtk::SECURITY_FLOOR_PUBLISHED_UNIX_SECONDS,
            || None,
        )
        .is_ok());
        let before_deadline =
            zephium_core::webkitgtk::SECURITY_FLOOR_REVIEW_DEADLINE_EXCLUSIVE_UNIX_SECONDS - 1;
        assert!(enforce_runtime_preconditions(before_deadline, || None).is_ok());
        assert!(enforce_runtime_preconditions(before_deadline, || Some(
            "WEBKIT_DISABLE_SANDBOX_THIS_IS_DANGEROUS".into()
        ))
        .unwrap_err()
        .contains("WEBKIT_DISABLE_SANDBOX_THIS_IS_DANGEROUS"));
        assert!(enforce_runtime_preconditions(before_deadline, || Some(
            "WEBKIT_FORCE_SANDBOX".into()
        ))
        .unwrap_err()
        .contains("WEBKIT_FORCE_SANDBOX"));
        assert!(enforce_runtime_preconditions(before_deadline, || Some(
            "WEBKIT_INSPECTOR_SERVER".into()
        ))
        .unwrap_err()
        .contains("WEBKIT_INSPECTOR_SERVER"));
        assert!(enforce_runtime_preconditions(
            zephium_core::webkitgtk::SECURITY_FLOOR_REVIEW_DEADLINE_EXCLUSIVE_UNIX_SECONDS,
            || None,
        )
        .is_ok());
        assert_eq!(
            enforce_runtime_version(
                (2, 54, 1),
                zephium_core::webkitgtk::SECURITY_FLOOR_REVIEW_DEADLINE_EXCLUSIVE_UNIX_SECONDS,
            ),
            Ok(
                zephium_core::runtime_security::RuntimeSecurityAdvisories::from_advisory(
                    zephium_core::runtime_security::RuntimeSecurityAdvisory::review_overdue(),
                ),
            )
        );
    }

    #[test]
    fn invalid_manager_provenance_cannot_be_forgotten_by_empty_retry() {
        let temp = tempfile::tempdir().unwrap();
        let root = crate::erasure::canonical_owned_root(&temp.path().join("profiles")).unwrap();
        let profile = ProfileId::from(70);
        let path = crate::erasure::prepare_profile_directory(&root, profile).unwrap();
        std::fs::write(path.join("retained"), b"must-not-delete").unwrap();

        for _ in 0..2 {
            let (tx, rx) = mpsc::channel();
            let completion = crate::erasure::Completion::start(
                Box::new(move |outcome| tx.send(outcome).unwrap()),
                Arc::new(AtomicBool::new(true)),
            );
            erase_profile_data(Vec::new(), false, vec![root.clone()], profile, completion);
            assert_eq!(
                rx.recv_timeout(std::time::Duration::from_millis(100))
                    .unwrap(),
                zephium_core::ports::engine::ProfileDataErasureOutcome::Failed
            );
            assert_eq!(
                std::fs::read(path.join("retained")).unwrap(),
                b"must-not-delete"
            );
        }
    }

    #[test]
    #[ignore = "requires Xvfb and WebKitGTK at the supported security floor; CI runs this explicitly"]
    fn principal_world_handlers_are_mutually_isolated_from_three_peers_and_page_world() {
        enforce_runtime_security_floor().expect("test runner must use supported WebKitGTK");
        gtk::init().expect("GTK requires an Xvfb/Wayland display for native WebView tests");

        let window = gtk::Window::new(gtk::WindowType::Toplevel);
        let container = gtk::Fixed::new();
        window.add(&container);
        window.realize();
        let view = WebViewBuilder::new()
            .build_gtk(&container)
            .expect("principal-isolation Wry WebView");
        let manager = view
            .webview()
            .user_content_manager()
            .expect("principal-isolation UserContentManager");
        let identities = test_principals().map(PrincipalContentIdentity::new);
        let handler_names = identities
            .iter()
            .map(|identity| format!("\"{}\"", identity.handler_name()))
            .collect::<Vec<_>>()
            .join(",");
        let (tx, rx) = mpsc::channel();
        let mut registrations = Vec::new();

        for identity in &identities {
            let tx = tx.clone();
            registrations.push(
                register_principal_message_handler(&view, identity.principal(), move |message| {
                    let _ = tx.send(message);
                })
                .expect("register a distinct world-scoped principal handler"),
            );
            let source = format!(
                r#"(() => {{
                    const names = [{handler_names}];
                    const visible = names.filter(
                        name => !!globalThis.webkit?.messageHandlers?.[name]
                    );
                    const inherited = typeof globalThis.__zephiumPrincipalProbe;
                    globalThis.__zephiumPrincipalProbe = {world:?};
                    globalThis.webkit.messageHandlers[{handler:?}].postMessage(
                        JSON.stringify({{ visible, inherited, claimed: "another-principal" }})
                    );
                }})()"#,
                world = identity.world_name(),
                handler = identity.handler_name(),
            );
            manager.add_script(&UserScript::for_world(
                &source,
                UserContentInjectedFrames::AllFrames,
                UserScriptInjectionTime::Start,
                identity.world_name(),
                &[],
                &[],
            ));
        }
        drop(tx);

        let page_handler_checks = identities
            .iter()
            .map(|identity| {
                format!(
                    "!globalThis.webkit?.messageHandlers?.[{:?}]",
                    identity.handler_name()
                )
            })
            .collect::<Vec<_>>()
            .join("&&");
        window.show_all();
        view.load_url(&format!(
            "data:text/html,<title>loading</title><script>\
             globalThis.__zephiumPrincipalProbe='page';\
             document.title=({page_handler_checks})?'page-isolated':'page-handler-leak';\
             </script>"
        ))
        .expect("load hostile page-world principal probe");

        let deadline = Instant::now() + Duration::from_secs(5);
        let mut received = HashMap::new();
        while received.len() < identities.len() && Instant::now() < deadline {
            while gtk::events_pending() {
                gtk::main_iteration_do(false);
            }
            match rx.try_recv() {
                Ok(message) => {
                    received.insert(message.identity().principal(), message.body().to_owned());
                }
                Err(mpsc::TryRecvError::Empty) => std::thread::sleep(Duration::from_millis(10)),
                Err(error) => panic!("principal handler channel failed: {error}"),
            }
        }
        assert_eq!(received.len(), identities.len());
        for identity in &identities {
            let body = &received[&identity.principal()];
            assert!(body.contains(&format!("\"{}\"", identity.handler_name())));
            assert!(body.contains(r#""inherited":"undefined""#));
            assert!(body.contains(r#""claimed":"another-principal""#));
            for peer in identities
                .iter()
                .filter(|peer| peer.principal() != identity.principal())
            {
                assert!(!body.contains(&format!("\"{}\"", peer.handler_name())));
            }
        }

        while view.webview().title().as_deref() == Some("loading") && Instant::now() < deadline {
            while gtk::events_pending() {
                gtk::main_iteration_do(false);
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(view.webview().title().as_deref(), Some("page-isolated"));
        drop(registrations);
    }

    #[test]
    #[ignore = "requires Xvfb and WebKitGTK at the supported security floor; CI runs this explicitly"]
    fn wry_contexts_enable_sandbox_and_cross_site_process_swap() {
        enforce_runtime_security_floor().expect("test runner must use supported WebKitGTK");
        gtk::init().expect("GTK requires an Xvfb/Wayland display for native WebView tests");

        let window = gtk::Window::new(gtk::WindowType::Toplevel);
        let container = gtk::Fixed::new();
        window.add(&container);
        window.realize();

        let data = tempfile::tempdir().expect("temporary profile directory");
        let data_path = data
            .path()
            .canonicalize()
            .expect("canonical temporary profile directory");
        let profile = ProfileId::from(7);
        let mut web_context = wry::WebContext::try_new(Some(data_path.clone()))
            .expect("secure persistent WebContext");
        let (ipc_tx, ipc_rx) = mpsc::channel();
        let persistent = WebViewBuilder::new_with_web_context(&mut web_context)
            .with_ipc_handler(move |request| {
                let _ = ipc_tx.send(request.into_body());
            })
            .build_gtk(&container)
            .expect("persistent Wry WebView");
        configure(
            &persistent,
            0.0,
            Partition::Persistent(profile),
            Some(&data_path),
        )
        .expect("persistent storage and process postconditions");
        let persistent_obligation = website_data_manager_obligation(&persistent);
        assert!(persistent_obligation.provenance_complete);
        assert_eq!(persistent_obligation.managers.len(), 1);
        let persistent_context = persistent
            .webview()
            .context()
            .expect("persistent native WebContext");
        assert!(!persistent_context.is_ephemeral());
        assert!(persistent_context.is_sandbox_enabled());
        assert!(persistent_context.is_process_swap_on_cross_site_navigation_enabled());
        window.show_all();
        persistent
            .load_url(
                "data:text/html,<title>hostile renderer probe</title><script>\
                 window.ipc.postMessage(window.webkit?.messageHandlers?.wryIpc \
                   ? 'page-native-handler-visible' : 'bounded-ipc-ok');\
                 document.dispatchEvent(new CustomEvent('wry-ipc-message-v1', \
                   {detail: 'x'.repeat(70000)}));\
                 </script>",
            )
            .expect("start a real WebKitWebProcess");
        let web_process = wait_for_web_process(std::process::id());
        prove_web_process_namespace_and_filter_state(web_process);

        let ipc_deadline = Instant::now() + Duration::from_secs(5);
        let first_ipc = loop {
            while gtk::events_pending() {
                gtk::main_iteration_do(false);
            }
            match ipc_rx.try_recv() {
                Ok(message) => break message,
                Err(mpsc::TryRecvError::Empty) if Instant::now() < ipc_deadline => {
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(error) => panic!("bounded isolated-world IPC did not arrive: {error}"),
            }
        };
        assert_eq!(first_ipc, "bounded-ipc-ok");
        let oversize_deadline = Instant::now() + Duration::from_millis(500);
        while Instant::now() < oversize_deadline {
            while gtk::events_pending() {
                gtk::main_iteration_do(false);
            }
            assert!(
                matches!(ipc_rx.try_recv(), Err(mpsc::TryRecvError::Empty)),
                "an over-limit page-world event reached the native IPC handler"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        let durable_related_view = persistent.webview();
        drop(persistent);
        assert!(
            WebViewBuilder::new_with_web_context(&mut web_context)
                .with_incognito(true)
                .build_gtk(&container)
                .is_err(),
            "incognito construction must reject a durable supplied context"
        );

        let mut private_context =
            wry::WebContext::new_ephemeral().expect("secure ephemeral WebContext");
        assert!(
            WebViewBuilder::new_with_web_context(&mut private_context)
                .with_incognito(true)
                .with_related_view(durable_related_view)
                .build_gtk(&container)
                .is_err(),
            "a durable related view must not override an ephemeral incognito context"
        );
        let incognito = WebViewBuilder::new_with_web_context(&mut private_context)
            .with_incognito(true)
            .build_gtk(&container)
            .expect("incognito Wry WebView");
        configure(
            &incognito,
            0.0,
            Partition::Ephemeral(ProfileId::from(8)),
            None,
        )
        .expect("ephemeral storage and process postconditions");
        let incognito_obligation = website_data_manager_obligation(&incognito);
        assert!(incognito_obligation.provenance_complete);
        assert_eq!(incognito_obligation.managers.len(), 1);
        let incognito_context = incognito
            .webview()
            .context()
            .expect("incognito native WebContext");
        assert!(incognito_context.is_ephemeral());
        assert!(incognito_context.is_sandbox_enabled());
        assert!(incognito_context.is_process_swap_on_cross_site_navigation_enabled());

        let second_incognito = WebViewBuilder::new_with_web_context(&mut private_context)
            .with_incognito(true)
            .with_related_view(incognito.webview())
            .build_gtk(&container)
            .expect("second incognito Wry WebView");
        configure(
            &second_incognito,
            0.0,
            Partition::Ephemeral(ProfileId::from(8)),
            None,
        )
        .expect("shared ephemeral storage and process postconditions");
        let second_obligation = website_data_manager_obligation(&second_incognito);
        assert!(second_obligation.provenance_complete);
        assert_eq!(second_obligation.managers.len(), 1);
        assert_eq!(
            incognito_obligation.managers[0].as_ptr(),
            second_obligation.managers[0].as_ptr(),
            "one private profile must retain exactly one ephemeral native manager"
        );
    }
}
