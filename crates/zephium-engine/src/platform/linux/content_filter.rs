use std::ffi::{CStr, CString};
use std::path::Path;
use std::ptr::NonNull;
use std::rc::Rc;
use std::sync::Arc;

use webkit2gtk::gio::prelude::CancellableExt as _;
use webkit2gtk::glib::translate::ToGlibPtr;
use webkit2gtk::{glib, WebViewExt as _};
use wry::WebViewExtUnix;
use zephium_core::blocker::ContentRuleApplyFailure;
#[cfg(test)]
use zephium_core::blocker::{ContentRules, ContentRulesPayload, DeclarativeRuleFormat};

struct FilterStore(NonNull<webkit2gtk::ffi::WebKitUserContentFilterStore>);

impl Drop for FilterStore {
    fn drop(&mut self) {
        unsafe { glib::gobject_ffi::g_object_unref(self.0.as_ptr().cast()) };
    }
}

pub(crate) struct UserContentFilter(NonNull<webkit2gtk::ffi::WebKitUserContentFilter>);

impl Drop for UserContentFilter {
    fn drop(&mut self) {
        unsafe { webkit2gtk::ffi::webkit_user_content_filter_unref(self.0.as_ptr()) };
    }
}

#[derive(Clone)]
pub(crate) enum NativeContentPolicy {
    AllowAll,
    Declarative {
        digest: [u8; 32],
        filter: Rc<UserContentFilter>,
    },
}

pub(crate) fn same_policy(left: &NativeContentPolicy, right: &NativeContentPolicy) -> bool {
    match (left, right) {
        (NativeContentPolicy::AllowAll, NativeContentPolicy::AllowAll) => true,
        (
            NativeContentPolicy::Declarative { digest: left, .. },
            NativeContentPolicy::Declarative { digest: right, .. },
        ) => left == right,
        _ => false,
    }
}

pub(crate) fn content_policy_digest(policy: &NativeContentPolicy) -> Option<[u8; 32]> {
    match policy {
        NativeContentPolicy::AllowAll => None,
        NativeContentPolicy::Declarative { digest, .. } => Some(*digest),
    }
}

pub(crate) struct ContentPolicyRegistration {
    manager: Option<webkit2gtk::UserContentManager>,
    filter: Option<Rc<UserContentFilter>>,
    _pause: Option<crate::platform::content_pause::PauseRegistration>,
}

pub(crate) struct ContentPolicyCompilationCancellation(webkit2gtk::gio::Cancellable);

impl ContentPolicyCompilationCancellation {
    pub(crate) fn cancel(&self) {
        self.0.cancel();
    }
}

pub(crate) struct ContentPolicyCachePage {
    pub(crate) identifiers: Vec<String>,
    pub(crate) next_cursor: usize,
    pub(crate) scan_complete: bool,
    pub(crate) over_budget: bool,
}

pub(crate) struct ContentPolicyCacheMaintenanceCancellation(webkit2gtk::gio::Cancellable);

impl ContentPolicyCacheMaintenanceCancellation {
    pub(crate) fn cancel(&self) {
        self.0.cancel();
    }
}

const MAX_CACHE_IDENTIFIERS_PER_PAGE: usize = 128;
const MAX_CACHE_IDENTIFIER_CURSOR: usize = 4_096;
const MAX_CACHE_IDENTIFIER_BYTES: usize = "app.zephium.rules.v1.".len() + 64;

impl ContentPolicyRegistration {
    pub(crate) fn allow_all() -> Self {
        Self {
            manager: None,
            filter: None,
            _pause: None,
        }
    }

    pub(crate) fn retire(self) -> Result<(), ContentRuleApplyFailure> {
        drop(self);
        Ok(())
    }
}

impl Drop for ContentPolicyRegistration {
    fn drop(&mut self) {
        self._pause.take();
        if let (Some(manager), Some(filter)) = (&self.manager, &self.filter) {
            unsafe {
                webkit2gtk::ffi::webkit_user_content_manager_remove_filter(
                    manager.to_glib_none().0,
                    filter.0.as_ptr(),
                );
            }
        }
    }
}

pub(crate) fn install_scoped_on_view(
    view: &wry::WebView,
    policy: &NativeContentPolicy,
    pause: &crate::platform::content_pause::ContentPause,
) -> Result<ContentPolicyRegistration, ContentRuleApplyFailure> {
    let NativeContentPolicy::Declarative { filter, .. } = policy else {
        return Ok(ContentPolicyRegistration::allow_all());
    };
    let manager = view
        .webview()
        .user_content_manager()
        .ok_or(ContentRuleApplyFailure::NativeInstallation)?;
    let callback_manager = manager.clone();
    let callback_filter = filter.clone();
    let registration = pause
        .register(move |paused| unsafe {
            if paused {
                webkit2gtk::ffi::webkit_user_content_manager_remove_filter(
                    callback_manager.to_glib_none().0,
                    callback_filter.0.as_ptr(),
                );
            } else {
                webkit2gtk::ffi::webkit_user_content_manager_add_filter(
                    callback_manager.to_glib_none().0,
                    callback_filter.0.as_ptr(),
                );
            }
        })
        .ok_or(ContentRuleApplyFailure::NativeInstallation)?;
    Ok(ContentPolicyRegistration {
        manager: Some(manager),
        filter: Some(filter.clone()),
        _pause: Some(registration),
    })
}

struct EncodedRules(Arc<str>);

impl AsRef<[u8]> for EncodedRules {
    fn as_ref(&self) -> &[u8] {
        self.0.as_bytes()
    }
}

struct CacheMaintenanceContext<F> {
    _store: FilterStore,
    _cancellable: webkit2gtk::gio::Cancellable,
    owner_context: glib::MainContext,
    identifier: Option<CString>,
    cursor: usize,
    done: Option<F>,
}

impl<F> CacheMaintenanceContext<F> {
    fn finish<T: 'static>(self: Box<Self>, result: Result<T, ()>)
    where
        F: FnOnce(Result<T, ()>) + 'static,
    {
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.finish_inner(result);
        }));
    }

    fn finish_inner<T: 'static>(mut self: Box<Self>, result: Result<T, ()>)
    where
        F: FnOnce(Result<T, ()>) + 'static,
    {
        let done = self.done.take();
        let owner_context = self.owner_context.clone();
        // Drop the native store, GCancellable and identifier before crossing
        // back into the host. Deferring on the exact owner context proves the
        // GAsyncReadyCallback stack has returned before slot retirement.
        drop(self);
        if let Some(done) = done {
            if !owner_context.is_owner() {
                return;
            }
            drop(owner_context.spawn_local(async move {
                let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| done(result)));
            }));
        }
    }
}

struct OwnedStringVector(*mut *mut std::ffi::c_char);

impl Drop for OwnedStringVector {
    fn drop(&mut self) {
        unsafe { glib::ffi::g_strfreev(self.0) };
    }
}

pub(crate) fn enumerate_content_policy_cache<F>(
    cache: &Path,
    cursor: usize,
    done: F,
) -> ContentPolicyCacheMaintenanceCancellation
where
    F: FnOnce(Result<ContentPolicyCachePage, ()>) + 'static,
{
    // WebKitGTK materializes the GStrv internally. Keep it native-owned until
    // the exact callback, scan one bounded pointer window, copy only exact
    // ASCII identifiers, then free the complete vector at once.
    let cancellation = webkit2gtk::gio::Cancellable::new();
    let cancellation_handle = ContentPolicyCacheMaintenanceCancellation(cancellation.clone());
    let Some(context) = cache_maintenance_context(cache, cancellation, cursor, None, done) else {
        return cancellation_handle;
    };
    let store = context._store.0.as_ptr();
    let cancellable = context._cancellable.to_glib_none().0;
    let user_data = Box::into_raw(context).cast();
    unsafe {
        webkit2gtk::ffi::webkit_user_content_filter_store_fetch_identifiers(
            store,
            cancellable,
            Some(content_policy_cache_identifiers_fetched::<F>),
            user_data,
        );
    }
    cancellation_handle
}

pub(crate) fn remove_content_policy_cache_identifier<F>(
    cache: &Path,
    identifier: String,
    done: F,
) -> ContentPolicyCacheMaintenanceCancellation
where
    F: FnOnce(Result<(), ()>) + 'static,
{
    let cancellation = webkit2gtk::gio::Cancellable::new();
    let cancellation_handle = ContentPolicyCacheMaintenanceCancellation(cancellation.clone());
    let identifier = if identifier.len() == MAX_CACHE_IDENTIFIER_BYTES && identifier.is_ascii() {
        CString::new(identifier).ok()
    } else {
        None
    };
    let Some(identifier) = identifier else {
        done(Err(()));
        return cancellation_handle;
    };
    let Some(context) = cache_maintenance_context(cache, cancellation, 0, Some(identifier), done)
    else {
        return cancellation_handle;
    };
    let store = context._store.0.as_ptr();
    let identifier = context
        .identifier
        .as_ref()
        .expect("validated maintenance identifier")
        .as_ptr();
    let cancellable = context._cancellable.to_glib_none().0;
    let user_data = Box::into_raw(context).cast();
    unsafe {
        webkit2gtk::ffi::webkit_user_content_filter_store_remove(
            store,
            identifier,
            cancellable,
            Some(content_policy_cache_identifier_removed::<F>),
            user_data,
        );
    }
    cancellation_handle
}

fn cache_maintenance_context<F, T>(
    cache: &Path,
    cancellation: webkit2gtk::gio::Cancellable,
    cursor: usize,
    identifier: Option<CString>,
    done: F,
) -> Option<Box<CacheMaintenanceContext<F>>>
where
    F: FnOnce(Result<T, ()>) + 'static,
{
    let owner_context = glib::MainContext::ref_thread_default();
    if !owner_context.is_owner() {
        done(Err(()));
        return None;
    }
    let cache = cache.to_str().and_then(|cache| CString::new(cache).ok());
    let Some(cache) = cache else {
        done(Err(()));
        return None;
    };
    let Some(store) = NonNull::new(unsafe {
        webkit2gtk::ffi::webkit_user_content_filter_store_new(cache.as_ptr())
    }) else {
        done(Err(()));
        return None;
    };
    Some(Box::new(CacheMaintenanceContext {
        _store: FilterStore(store),
        _cancellable: cancellation,
        owner_context,
        identifier,
        cursor,
        done: Some(done),
    }))
}

unsafe extern "C" fn content_policy_cache_identifiers_fetched<F>(
    store: *mut glib::gobject_ffi::GObject,
    result: *mut webkit2gtk::gio::ffi::GAsyncResult,
    user_data: glib::ffi::gpointer,
) where
    F: FnOnce(Result<ContentPolicyCachePage, ()>) + 'static,
{
    let Some(user_data) = NonNull::new(user_data.cast::<CacheMaintenanceContext<F>>()) else {
        return;
    };
    let context = unsafe { Box::from_raw(user_data.as_ptr()) };
    let callback_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if store.cast::<webkit2gtk::ffi::WebKitUserContentFilterStore>()
            != context._store.0.as_ptr()
        {
            return Err(());
        }
        let Some(result) = NonNull::new(result) else {
            return Err(());
        };
        let identifiers = unsafe {
            webkit2gtk::ffi::webkit_user_content_filter_store_fetch_identifiers_finish(
                context._store.0.as_ptr(),
                result.as_ptr(),
            )
        };
        let Some(identifiers) = NonNull::new(identifiers) else {
            return Err(());
        };
        let identifiers = OwnedStringVector(identifiers.as_ptr());
        let page = unsafe { content_policy_cache_page(&identifiers, context.cursor) };
        Ok(page)
    }))
    .unwrap_or(Err(()));
    context.finish(callback_result);
}

unsafe extern "C" fn content_policy_cache_identifier_removed<F>(
    store: *mut glib::gobject_ffi::GObject,
    result: *mut webkit2gtk::gio::ffi::GAsyncResult,
    user_data: glib::ffi::gpointer,
) where
    F: FnOnce(Result<(), ()>) + 'static,
{
    let Some(user_data) = NonNull::new(user_data.cast::<CacheMaintenanceContext<F>>()) else {
        return;
    };
    let context = unsafe { Box::from_raw(user_data.as_ptr()) };
    let callback_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if store.cast::<webkit2gtk::ffi::WebKitUserContentFilterStore>()
            != context._store.0.as_ptr()
        {
            return Err(());
        }
        let Some(result) = NonNull::new(result) else {
            return Err(());
        };
        let mut error = std::ptr::null_mut();
        let removed = unsafe {
            webkit2gtk::ffi::webkit_user_content_filter_store_remove_finish(
                context._store.0.as_ptr(),
                result.as_ptr(),
                &mut error,
            ) != 0
        };
        if !error.is_null() {
            unsafe { glib::ffi::g_error_free(error) };
            return Err(());
        }
        removed.then_some(()).ok_or(())
    }))
    .unwrap_or(Err(()));
    context.finish(callback_result);
}

unsafe fn content_policy_cache_page(
    identifiers: &OwnedStringVector,
    cursor: usize,
) -> ContentPolicyCachePage {
    // Pointer traversal is capped even when a corrupt store returns an
    // enormous vector, and stops at the first native NUL sentinel.
    let bounded_cursor = cursor.min(MAX_CACHE_IDENTIFIER_CURSOR);
    let mut index = 0usize;
    let mut page = Vec::with_capacity(MAX_CACHE_IDENTIFIERS_PER_PAGE);
    let end = bounded_cursor
        .saturating_add(MAX_CACHE_IDENTIFIERS_PER_PAGE)
        .min(MAX_CACHE_IDENTIFIER_CURSOR);
    while index < end {
        let identifier = unsafe { *identifiers.0.add(index) };
        if identifier.is_null() {
            return ContentPolicyCachePage {
                identifiers: page,
                next_cursor: 0,
                scan_complete: true,
                over_budget: false,
            };
        }
        if index >= bounded_cursor {
            if let Some(identifier) = unsafe { exact_cache_identifier(identifier) } {
                page.push(identifier);
            }
        }
        index += 1;
    }
    let has_more = unsafe { !(*identifiers.0.add(index)).is_null() };
    let over_budget = index >= MAX_CACHE_IDENTIFIER_CURSOR && has_more;
    let scan_complete = !has_more || over_budget;
    ContentPolicyCachePage {
        identifiers: page,
        next_cursor: if scan_complete { 0 } else { index },
        scan_complete,
        over_budget,
    }
}

unsafe fn exact_cache_identifier(identifier: *const std::ffi::c_char) -> Option<String> {
    let mut length = 0usize;
    while length <= MAX_CACHE_IDENTIFIER_BYTES {
        let byte = unsafe { *identifier.cast::<u8>().add(length) };
        if byte == 0 {
            break;
        }
        length += 1;
    }
    if length != MAX_CACHE_IDENTIFIER_BYTES {
        return None;
    }
    let bytes = unsafe { std::slice::from_raw_parts(identifier.cast::<u8>(), length) };
    if !bytes.is_ascii() {
        return None;
    }
    std::str::from_utf8(bytes).ok().map(ToOwned::to_owned)
}

struct CompileContext<F> {
    _store: FilterStore,
    _cancellable: webkit2gtk::gio::Cancellable,
    owner_context: glib::MainContext,
    identifier: CString,
    encoded: Option<Arc<str>>,
    _bytes: Option<glib::Bytes>,
    artifact_digest: [u8; 32],
    done: Option<F>,
}

enum LoadedAction {
    Finish(Result<NativeContentPolicy, ContentRuleApplyFailure>),
    Save,
}

impl<F> CompileContext<F>
where
    F: FnOnce(Result<NativeContentPolicy, ContentRuleApplyFailure>) + 'static,
{
    fn finish(mut self: Box<Self>, result: Result<NativeContentPolicy, ContentRuleApplyFailure>) {
        let done = self.done.take();
        let owner_context = self.owner_context.clone();
        // Release the store, cancellable, encoded bytes, and callback context
        // before publishing native quiescence to the engine. A task on the
        // exact captured owner context also guarantees the GAsyncReadyCallback
        // stack has returned before shutdown can acknowledge its final
        // barrier.
        drop(self);
        if let Some(done) = done {
            if !owner_context.is_owner() {
                return;
            }
            drop(owner_context.spawn_local(async move {
                let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| done(result)));
            }));
        }
    }
}

pub(crate) fn compile<F>(
    cache: &Path,
    encoded: Arc<str>,
    artifact_digest: [u8; 32],
    done: F,
) -> ContentPolicyCompilationCancellation
where
    F: FnOnce(Result<NativeContentPolicy, ContentRuleApplyFailure>) + 'static,
{
    let cancellation = webkit2gtk::gio::Cancellable::new();
    let cancellation_handle = ContentPolicyCompilationCancellation(cancellation.clone());
    let owner_context = glib::MainContext::ref_thread_default();
    if !owner_context.is_owner() {
        done(Err(ContentRuleApplyFailure::NativeCompilation));
        return cancellation_handle;
    }
    let Some(cache) = cache.to_str() else {
        done(Err(ContentRuleApplyFailure::NativeCompilation));
        return cancellation_handle;
    };
    let Ok(cache) = CString::new(cache) else {
        done(Err(ContentRuleApplyFailure::NativeCompilation));
        return cancellation_handle;
    };
    let identifier = rule_identifier(artifact_digest);
    let Ok(identifier) = CString::new(identifier) else {
        done(Err(ContentRuleApplyFailure::InvalidArtifact));
        return cancellation_handle;
    };
    let Some(store) = NonNull::new(unsafe {
        webkit2gtk::ffi::webkit_user_content_filter_store_new(cache.as_ptr())
    }) else {
        done(Err(ContentRuleApplyFailure::NativeCompilation));
        return cancellation_handle;
    };
    let context = Box::new(CompileContext {
        _store: FilterStore(store),
        _cancellable: cancellation,
        owner_context,
        identifier,
        encoded: Some(encoded.clone()),
        _bytes: None,
        artifact_digest,
        done: Some(done),
    });
    let store = context._store.0.as_ptr();
    let identifier = context.identifier.as_ptr();
    let cancellable = context._cancellable.to_glib_none().0;
    let user_data = Box::into_raw(context).cast();
    unsafe {
        webkit2gtk::ffi::webkit_user_content_filter_store_load(
            store,
            identifier,
            cancellable,
            Some(loaded::<F>),
            user_data,
        );
    }
    cancellation_handle
}

unsafe extern "C" fn loaded<F>(
    store: *mut glib::gobject_ffi::GObject,
    result: *mut webkit2gtk::gio::ffi::GAsyncResult,
    user_data: glib::ffi::gpointer,
) where
    F: FnOnce(Result<NativeContentPolicy, ContentRuleApplyFailure>) + 'static,
{
    let Some(user_data) = NonNull::new(user_data.cast::<CompileContext<F>>()) else {
        return;
    };
    // SAFETY: `compile` transferred this exact Box to the one-shot GLib
    // callback. Keep ownership outside the unwind boundary so every caught
    // panic can publish one terminal failure or transfer it to save.
    let context = unsafe { Box::from_raw(user_data.as_ptr()) };
    let callback_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if store.cast::<webkit2gtk::ffi::WebKitUserContentFilterStore>()
            != context._store.0.as_ptr()
        {
            return LoadedAction::Finish(Err(ContentRuleApplyFailure::NativeCompilation));
        }
        let Some(result) = NonNull::new(result) else {
            return LoadedAction::Finish(Err(ContentRuleApplyFailure::NativeCompilation));
        };
        let mut error = std::ptr::null_mut();
        // SAFETY: the result and retained store belong to this load callback.
        let filter = unsafe {
            webkit2gtk::ffi::webkit_user_content_filter_store_load_finish(
                context._store.0.as_ptr(),
                result.as_ptr(),
                &mut error,
            )
        };
        if !error.is_null() {
            if let Some(filter) = NonNull::new(filter) {
                // SAFETY: a non-null finish result is transferred full.
                unsafe { webkit2gtk::ffi::webkit_user_content_filter_unref(filter.as_ptr()) };
            }
            let cache_miss = is_cache_miss(error);
            // SAFETY: GLib transferred this GError to the caller.
            unsafe { glib::ffi::g_error_free(error) };
            if cache_miss {
                LoadedAction::Save
            } else {
                LoadedAction::Finish(Err(ContentRuleApplyFailure::NativeCompilation))
            }
        } else if let Some(filter) = NonNull::new(filter) {
            LoadedAction::Finish(validated_policy(
                filter,
                &context.identifier,
                context.artifact_digest,
            ))
        } else {
            LoadedAction::Finish(Err(ContentRuleApplyFailure::NativeCompilation))
        }
    }));
    let action = callback_result.unwrap_or(LoadedAction::Finish(Err(
        ContentRuleApplyFailure::NativeCompilation,
    )));
    match action {
        LoadedAction::Finish(result) => context.finish(result),
        LoadedAction::Save => save_after_cache_miss(context),
    }
}

fn save_after_cache_miss<F>(mut context: Box<CompileContext<F>>)
where
    F: FnOnce(Result<NativeContentPolicy, ContentRuleApplyFailure>) + 'static,
{
    // Preparation can allocate and invoke GLib wrapper code. Keep the Box
    // owned outside the unwind boundary until all pointers into it are ready.
    let prepared = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let encoded = context
            .encoded
            .take()
            .ok_or(ContentRuleApplyFailure::NativeCompilation)?;
        context._bytes = Some(glib::Bytes::from_owned(EncodedRules(encoded)));
        let bytes = context
            ._bytes
            .as_ref()
            .ok_or(ContentRuleApplyFailure::NativeCompilation)?;
        let bytes_ptr = ToGlibPtr::<*const glib::ffi::GBytes>::to_glib_none(bytes)
            .0
            .cast_mut();
        Ok::<_, ContentRuleApplyFailure>((
            context._store.0.as_ptr(),
            context.identifier.as_ptr(),
            bytes_ptr,
            context._cancellable.to_glib_none().0,
        ))
    }));
    let (store, identifier, bytes, cancellable) = match prepared {
        Ok(Ok(prepared)) => prepared,
        Ok(Err(failure)) => {
            context.finish(Err(failure));
            return;
        }
        Err(_) => {
            context.finish(Err(ContentRuleApplyFailure::NativeCompilation));
            return;
        }
    };
    let user_data = Box::into_raw(context).cast();
    unsafe {
        webkit2gtk::ffi::webkit_user_content_filter_store_save(
            store,
            identifier,
            bytes,
            cancellable,
            Some(saved::<F>),
            user_data,
        );
    }
}

unsafe extern "C" fn saved<F>(
    store: *mut glib::gobject_ffi::GObject,
    result: *mut webkit2gtk::gio::ffi::GAsyncResult,
    user_data: glib::ffi::gpointer,
) where
    F: FnOnce(Result<NativeContentPolicy, ContentRuleApplyFailure>) + 'static,
{
    let Some(user_data) = NonNull::new(user_data.cast::<CompileContext<F>>()) else {
        return;
    };
    // SAFETY: `save_after_cache_miss` transferred this exact Box to the
    // one-shot GLib callback. Ownership remains outside the unwind boundary.
    let context = unsafe { Box::from_raw(user_data.as_ptr()) };
    let callback_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if store.cast::<webkit2gtk::ffi::WebKitUserContentFilterStore>()
            != context._store.0.as_ptr()
        {
            return Err(ContentRuleApplyFailure::NativeCompilation);
        }
        let Some(result) = NonNull::new(result) else {
            return Err(ContentRuleApplyFailure::NativeCompilation);
        };
        let mut error = std::ptr::null_mut();
        // SAFETY: the result and retained store belong to this save callback.
        let filter = unsafe {
            webkit2gtk::ffi::webkit_user_content_filter_store_save_finish(
                context._store.0.as_ptr(),
                result.as_ptr(),
                &mut error,
            )
        };
        if !error.is_null() {
            if let Some(filter) = NonNull::new(filter) {
                // SAFETY: a non-null finish result is transferred full.
                unsafe { webkit2gtk::ffi::webkit_user_content_filter_unref(filter.as_ptr()) };
            }
            // SAFETY: GLib transferred this GError to the caller.
            unsafe { glib::ffi::g_error_free(error) };
            Err(ContentRuleApplyFailure::NativeCompilation)
        } else if let Some(filter) = NonNull::new(filter) {
            validated_policy(filter, &context.identifier, context.artifact_digest)
        } else {
            Err(ContentRuleApplyFailure::NativeCompilation)
        }
    }));
    let result = callback_result.unwrap_or(Err(ContentRuleApplyFailure::NativeCompilation));
    context.finish(result);
}

fn validated_policy(
    filter: NonNull<webkit2gtk::ffi::WebKitUserContentFilter>,
    expected_identifier: &CStr,
    digest: [u8; 32],
) -> Result<NativeContentPolicy, ContentRuleApplyFailure> {
    let filter = Rc::new(UserContentFilter(filter));
    // SAFETY: the finish functions returned a live, transfer-full filter.
    let identifier =
        unsafe { webkit2gtk::ffi::webkit_user_content_filter_get_identifier(filter.0.as_ptr()) };
    let valid = if identifier.is_null() {
        false
    } else {
        // SAFETY: WebKit documents a borrowed NUL-terminated identifier for
        // the lifetime of the filter. Both strings must be valid UTF-8 and
        // exactly equal; byte equality alone would accept malformed native
        // text that happened to share an ASCII prefix.
        let native = unsafe { CStr::from_ptr(identifier) }.to_str();
        matches!(
            (native, expected_identifier.to_str()),
            (Ok(native), Ok(expected)) if native == expected
        )
    };
    if !valid {
        return Err(ContentRuleApplyFailure::NativeCompilation);
    }
    Ok(NativeContentPolicy::Declarative { digest, filter })
}

fn is_cache_miss(error: *mut glib::ffi::GError) -> bool {
    !error.is_null()
        && unsafe {
            glib::ffi::g_error_matches(
                error,
                webkit2gtk::ffi::webkit_user_content_filter_error_quark(),
                webkit2gtk::ffi::WEBKIT_USER_CONTENT_FILTER_ERROR_NOT_FOUND,
            ) != 0
        }
}

fn rule_identifier(digest: [u8; 32]) -> String {
    use std::fmt::Write as _;

    let mut identifier = String::with_capacity(20 + 64);
    identifier.push_str("app.zephium.rules.v1.");
    for byte in digest {
        let _ = write!(identifier, "{byte:02x}");
    }
    identifier
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};
    use std::io::Read as _;
    use std::time::Duration;
    use zephium_core::blocker::{ContentRuleCoverage, ContentRuleDigest};

    #[test]
    fn rule_identifier_is_exact_and_bounded() {
        let identifier = rule_identifier([0xab; 32]);
        assert_eq!(identifier.len(), "app.zephium.rules.v1.".len() + 64);
        assert!(identifier.ends_with(&"ab".repeat(32)));
    }

    #[test]
    fn cache_identifier_copy_is_exact_ascii_and_bounded() {
        let exact = CString::new(rule_identifier([0xab; 32])).unwrap();
        assert_eq!(
            unsafe { exact_cache_identifier(exact.as_ptr()) },
            Some(rule_identifier([0xab; 32]))
        );
        let short = CString::new("app.zephium.rules.v1.short").unwrap();
        assert_eq!(unsafe { exact_cache_identifier(short.as_ptr()) }, None);
        let long = CString::new(format!("{}0", rule_identifier([0xab; 32]))).unwrap();
        assert_eq!(unsafe { exact_cache_identifier(long.as_ptr()) }, None);
    }

    #[test]
    fn cache_maintenance_owns_gio_callbacks_cancellation_and_strv() {
        let source = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/src/platform/linux/content_filter.rs"
        ));
        let production = source
            .split_once("\n#[cfg(test)]\nmod tests {")
            .expect("content-filter test boundary disappeared")
            .0;
        let context = production
            .find("struct CacheMaintenanceContext")
            .expect("cache maintenance context disappeared");
        let compiler = production
            .find("struct CompileContext")
            .expect("cache maintenance boundary disappeared");
        let maintenance = &production[context..compiler];
        assert!(maintenance.contains("webkit_user_content_filter_store_fetch_identifiers("));
        assert!(maintenance.contains("webkit_user_content_filter_store_remove("));
        assert!(maintenance.contains("webkit_user_content_filter_store_remove_finish("));
        assert!(maintenance.contains("GCancellable"));
        assert!(maintenance.contains("g_strfreev"));
        assert!(maintenance.contains("MAX_CACHE_IDENTIFIERS_PER_PAGE"));
        assert!(maintenance.contains("MAX_CACHE_IDENTIFIER_CURSOR"));

        let finish = maintenance
            .find("fn finish<T:")
            .expect("cache maintenance terminal disappeared");
        let enumerate = maintenance
            .find("pub(crate) fn enumerate_content_policy_cache")
            .expect("cache enumeration disappeared");
        let finish_source = &maintenance[finish..enumerate];
        let take_done = finish_source
            .find("let done = self.done.take();")
            .expect("cache completion ownership disappeared");
        let drop_context = finish_source
            .find("drop(self);")
            .expect("cache native context is not retired");
        let defer = finish_source
            .find("owner_context.spawn_local")
            .expect("cache completion is not deferred on its exact owner context");
        assert!(take_done < drop_context && drop_context < defer);
        assert_eq!(
            maintenance
                .matches("let context = unsafe { Box::from_raw(user_data.as_ptr()) };")
                .count(),
            2,
            "each exact native callback must recover one Box outside the unwind boundary"
        );
        assert_eq!(
            maintenance.matches("catch_unwind").count(),
            4,
            "enumeration, removal, retirement, and deferred host delivery must contain unwinding"
        );
        assert!(
            maintenance
                .find("OwnedStringVector(identifiers.as_ptr())")
                .expect("native identifier vector ownership disappeared")
                < maintenance
                    .find("content_policy_cache_page(&identifiers")
                    .expect("bounded native identifier scan disappeared")
        );
    }

    #[test]
    fn persistent_lookup_precedes_save_and_both_results_are_validated() {
        let source = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/src/platform/linux/content_filter.rs"
        ));
        let production = source
            .split_once("\n#[cfg(test)]\nmod tests {")
            .expect("content-filter test boundary disappeared")
            .0;
        let compiler = production
            .find("struct CompileContext")
            .map(|start| &production[start..])
            .expect("content-filter compiler ownership boundary disappeared");
        let load = compiler
            .find("webkit_user_content_filter_store_load(")
            .expect("persistent content-filter lookup disappeared");
        let save = compiler
            .find("webkit_user_content_filter_store_save(")
            .expect("content-filter save disappeared");
        assert!(load < save, "persistent lookup must precede save");
        assert!(compiler.contains("let cancellation = webkit2gtk::gio::Cancellable::new();"));
        assert!(compiler.contains("_cancellable: cancellation,"));
        assert_eq!(
            compiler
                .matches("context._cancellable.to_glib_none().0")
                .count(),
            2,
            "load and save must derive their native handle from the retained cancellable"
        );
        for (operation, offset) in [("load", load), ("save", save)] {
            let call = &compiler[offset..];
            let call = &call[..call
                .find("\n        );")
                .expect("content-filter FFI call boundary disappeared")];
            assert!(
                call.contains("\n            cancellable,"),
                "{operation} must receive the retained non-null GIO cancellable"
            );
        }
        assert_eq!(
            compiler
                .split_whitespace()
                .collect::<String>()
                .matches("validated_policy(filter,&context.identifier")
                .count(),
            2,
            "cache hits and saved filters must both validate native identity"
        );
        let miss = compiler
            .find("let cache_miss = is_cache_miss(error);")
            .expect("exact cache-miss classification disappeared");
        let save_after_miss = compiler
            .find("LoadedAction::Save => save_after_cache_miss(context),")
            .expect("cache-miss save transition disappeared");
        assert!(
            miss < save_after_miss,
            "save must require an exact cache miss"
        );
        assert!(compiler.contains("webkit_user_content_filter_get_identifier"));
        assert_eq!(
            compiler.matches("catch_unwind").count(),
            4,
            "neither GLib nor deferred idle completion may unwind across a native boundary"
        );
        assert_eq!(
            compiler
                .matches("let context = unsafe { Box::from_raw(user_data.as_ptr()) };")
                .count(),
            2,
            "callback context ownership must be recovered outside both unwind boundaries"
        );
        let finish = compiler
            .find("fn finish(mut self: Box<Self>, result:")
            .expect("compiler terminal retirement disappeared");
        let compile = compiler[finish..]
            .find("pub(crate) fn compile")
            .map(|offset| finish + offset)
            .expect("compiler boundary disappeared");
        let finish_source = &compiler[finish..compile];
        let take_done = finish_source
            .find("let done = self.done.take();")
            .expect("compiler completion ownership disappeared");
        let drop_context = finish_source
            .find("drop(self);")
            .expect("compiler store/cancellable context is retained");
        let defer = finish_source
            .find("owner_context.spawn_local")
            .expect("terminal callback is not deferred on its exact GIO owner context");
        let deliver = finish_source
            .find("done(result)")
            .expect("deferred terminal delivery disappeared");
        assert!(
            take_done < drop_context && drop_context < defer && defer < deliver,
            "native compiler context must be retired before deferred host settlement"
        );
        assert!(!finish_source.contains("idle_add_local_once"));
    }

    #[test]
    fn gio_cancellable_has_a_non_null_native_handle_and_observes_cancel() {
        let cancellation = webkit2gtk::gio::Cancellable::new();
        let native: *mut webkit2gtk::gio::ffi::GCancellable = cancellation.to_glib_none().0;
        assert!(!native.is_null());
        assert!(!cancellation.is_cancelled());
        cancellation.cancel();
        assert!(cancellation.is_cancelled());
    }

    #[test]
    #[ignore = "requires Xvfb and WebKitGTK at the supported security floor; Fedora CI materializes and compiles the exact bundled EasyList + EasyPrivacy artifact"]
    fn exact_bundled_easylist_and_easyprivacy_compile_and_reload_natively() {
        crate::platform::linux::enforce_runtime_security_floor()
            .expect("test runner must use supported WebKitGTK");
        gtk::init().expect("GTK requires an Xvfb/Wayland display for native WebKit tests");

        let encoded = read_exact_ci_artifact();
        let coverage = ContentRuleCoverage {
            source_rules: 1,
            accepted_rules: 1,
            rejected_rules: 0,
            platform_omitted_rules: 0,
            platform_approximated_rules: 0,
            platform_resource_approximated_rules: 0,
            platform_source_kind_approximated_rules: 0,
            platform_attribution_approximated_rules: 0,
            blocking_rule_entries: 1,
        };
        let rules = ContentRules::declarative(
            ContentRuleDigest::from_bytes([0x5a; 32]),
            coverage,
            DeclarativeRuleFormat::WebKitContentBlockerV1,
            encoded,
        )
        .expect("verified release artifact must satisfy the engine boundary");
        let ContentRulesPayload::Declarative {
            artifact_digest, ..
        } = rules.payload()
        else {
            panic!("verified release artifact changed representation");
        };
        let artifact_digest = *artifact_digest.as_bytes();
        let cache = tempfile::tempdir().expect("isolated native content-filter cache");
        let context = glib::MainContext::new();

        context
            .with_thread_default(|| {
                let compiled = wait_for_native_compilation(
                    &context,
                    cache.path(),
                    rules.clone(),
                    artifact_digest,
                )
                .expect("exact release artifact must compile in native WebKitGTK");
                assert_eq!(content_policy_digest(&compiled), Some(artifact_digest));

                let reloaded =
                    wait_for_native_compilation(&context, cache.path(), rules, artifact_digest)
                        .expect("exact release artifact must reload from the native cache");
                assert_eq!(content_policy_digest(&reloaded), Some(artifact_digest));
                drop((compiled, reloaded));
            })
            .expect("test must own its isolated GLib context");
    }

    fn read_exact_ci_artifact() -> Arc<str> {
        let path = std::env::var_os("ZEPHIUM_BLOCKER_WEBKIT_ARTIFACT")
            .map(std::path::PathBuf::from)
            .expect("CI must provide the verified WebKit blocker artifact");
        let path_metadata =
            std::fs::symlink_metadata(&path).expect("inspect verified WebKit blocker artifact");
        assert!(
            path_metadata.is_file() && !path_metadata.file_type().is_symlink(),
            "verified WebKit blocker artifact must be a regular non-symlink file"
        );
        assert!(
            path_metadata.len() > 2
                && path_metadata.len() <= zephium_core::blocker::MAX_DECLARATIVE_RULE_BYTES as u64,
            "verified WebKit blocker artifact has an invalid byte length"
        );
        let mut file = std::fs::File::open(&path).expect("open verified WebKit blocker artifact");
        let opened_metadata = file
            .metadata()
            .expect("inspect open verified WebKit blocker artifact");
        assert_eq!(
            opened_metadata.len(),
            path_metadata.len(),
            "verified WebKit blocker artifact changed before open"
        );
        let mut bytes = Vec::with_capacity(opened_metadata.len() as usize);
        file.by_ref()
            .take(zephium_core::blocker::MAX_DECLARATIVE_RULE_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
            .expect("read verified WebKit blocker artifact");
        assert_eq!(
            bytes.len() as u64,
            opened_metadata.len(),
            "verified WebKit blocker artifact changed while being read"
        );
        assert_eq!(bytes.first(), Some(&b'['));
        assert_eq!(bytes.last(), Some(&b']'));
        Arc::from(String::from_utf8(bytes).expect("verified WebKit blocker artifact is UTF-8"))
    }

    fn wait_for_native_compilation(
        context: &glib::MainContext,
        cache: &Path,
        rules: Arc<ContentRules>,
        artifact_digest: [u8; 32],
    ) -> Result<NativeContentPolicy, ContentRuleApplyFailure> {
        let result = Rc::new(RefCell::new(None));
        let callback_result = result.clone();
        let timed_out = Rc::new(Cell::new(false));
        let timeout_fired = timed_out.clone();
        let cancellation = compile(
            cache,
            match rules.payload() {
                ContentRulesPayload::Declarative { encoded, .. } => encoded.clone(),
                _ => panic!("declarative fixture"),
            },
            artifact_digest,
            move |outcome| {
                *callback_result.borrow_mut() = Some(outcome);
            },
        );
        let timeout = crate::platform::linux::schedule_content_policy_timeout(
            Duration::from_secs(120),
            move || timeout_fired.set(true),
        )
        .expect("owned native compiler context must admit its watchdog");
        while result.borrow().is_none() && !timed_out.get() {
            let _ = context.iteration(true);
        }
        if timed_out.get() {
            cancellation.cancel();
            panic!("native WebKitGTK content-filter compilation exceeded 120 seconds");
        }
        timeout.cancel();
        let outcome = result
            .borrow_mut()
            .take()
            .expect("native content-filter compiler must settle exactly once");
        outcome
    }
}
