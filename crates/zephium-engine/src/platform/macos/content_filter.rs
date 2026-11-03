use std::cell::Cell;
use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;

use block2::RcBlock;
use dispatch2::{DispatchQoS, DispatchQueue, GlobalQueueIdentifier, MainThreadBound};
use objc2::rc::Retained;
use objc2_foundation::{MainThreadMarker, NSArray, NSError, NSString, NSURL};
use objc2_web_kit::{
    WKContentRuleList, WKContentRuleListStore, WKErrorCode, WKErrorDomain, WKUserContentController,
};
use wry::WebViewExtMacOS;
use zephium_core::blocker::ContentRuleApplyFailure;

#[derive(Clone)]
pub(crate) enum NativeContentPolicy {
    AllowAll,
    Declarative {
        digest: [u8; 32],
        identifier: Arc<str>,
        list: Retained<WKContentRuleList>,
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
    manager: Option<Retained<WKUserContentController>>,
    list: Option<Retained<WKContentRuleList>>,
    _pause: Option<crate::platform::content_pause::PauseRegistration>,
}

impl ContentPolicyRegistration {
    pub(crate) fn allow_all() -> Self {
        Self {
            manager: None,
            list: None,
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
        if let (Some(manager), Some(list)) = (&self.manager, &self.list) {
            // Remove only the exact Zephium-owned object. `removeAll...`
            // would erase future extension/user-script policy sharing this
            // controller.
            unsafe { manager.removeContentRuleList(list) };
        }
    }
}

pub(crate) struct ContentPolicyCompilationCancellation {
    cancelled: Rc<Cell<bool>>,
}

impl ContentPolicyCompilationCancellation {
    pub(crate) fn cancel(&self) {
        // WKContentRuleListStore exposes no native cancellation handle. This
        // advisory flag prevents a completed lookup/removal from starting the
        // next native phase after timeout. The host still retains the physical
        // slot until the currently outstanding exact callback settles.
        self.cancelled.set(true);
    }
}

pub(crate) struct ContentPolicyCachePage {
    pub(crate) identifiers: Vec<String>,
    pub(crate) next_cursor: usize,
    pub(crate) scan_complete: bool,
    pub(crate) over_budget: bool,
}

pub(crate) struct ContentPolicyCacheMaintenanceCancellation {
    cancelled: Rc<Cell<bool>>,
}

impl ContentPolicyCacheMaintenanceCancellation {
    pub(crate) fn cancel(&self) {
        // WKContentRuleListStore has no cancellation primitive. The host
        // retains the physical maintenance slot until WebKit calls the exact
        // completion, while this flag prevents any subsequent host phase.
        self.cancelled.set(true);
    }
}

const MAX_CACHE_IDENTIFIERS_PER_PAGE: usize = 128;
const MAX_CACHE_IDENTIFIER_CURSOR: usize = 4_096;
const MAX_CACHE_IDENTIFIER_BYTES: usize = "app.zephium.rules.v1.".len() + 64;

#[cfg(feature = "agentic-browser")]
pub(crate) fn install_on_view(
    view: &wry::WebView,
    policy: &NativeContentPolicy,
) -> Result<ContentPolicyRegistration, ContentRuleApplyFailure> {
    match policy {
        NativeContentPolicy::AllowAll => Ok(ContentPolicyRegistration::allow_all()),
        NativeContentPolicy::Declarative {
            identifier, list, ..
        } => {
            let native_identifier = unsafe { list.identifier() }.to_string();
            if native_identifier != identifier.as_ref() {
                return Err(ContentRuleApplyFailure::NativeInstallation);
            }
            let manager = view.manager();
            // The compiled list and controller are both retained and
            // main-thread-bound. This call has no fallible native return;
            // validating the identifier above excludes the only application
            // identity precondition.
            unsafe { manager.addContentRuleList(list) };
            Ok(ContentPolicyRegistration {
                manager: Some(manager),
                list: Some(list.clone()),
                _pause: None,
            })
        }
    }
}

pub(crate) fn install_scoped_on_view(
    view: &wry::WebView,
    policy: &NativeContentPolicy,
    pause: &crate::platform::content_pause::ContentPause,
    protection: &crate::platform::content_protection::ViewProtection,
) -> Result<ContentPolicyRegistration, ContentRuleApplyFailure> {
    // A content rule list is on the view whole, or the install fails with it.
    protection.settle(false);
    let NativeContentPolicy::Declarative {
        identifier, list, ..
    } = policy
    else {
        return Ok(ContentPolicyRegistration::allow_all());
    };
    if let Some(counter) = pause.statistics() {
        let _ = view.set_content_block_counter(identifier, counter.clone());
    }
    if unsafe { list.identifier() }.to_string() != identifier.as_ref() {
        return Err(ContentRuleApplyFailure::NativeInstallation);
    }
    let manager = view.manager();
    let callback_manager = manager.clone();
    let callback_list = list.clone();
    let registration = pause
        .register(move |paused| unsafe {
            if paused {
                callback_manager.removeContentRuleList(&callback_list);
            } else {
                callback_manager.addContentRuleList(&callback_list);
            }
        })
        .ok_or(ContentRuleApplyFailure::NativeInstallation)?;
    Ok(ContentPolicyRegistration {
        manager: Some(manager),
        list: Some(list.clone()),
        _pause: Some(registration),
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CompilationPhase {
    LookingUp,
    RemovingStale,
    Compiling,
    Finished,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LookupFailureAction {
    CompileAfterMiss,
    RemoveStaleThenCompile,
    Fail,
}

struct CompilationContext<F> {
    store: Cell<Option<Retained<WKContentRuleListStore>>>,
    identifier: Arc<str>,
    native_identifier: Cell<Option<Retained<NSString>>>,
    encoded: Cell<Option<Arc<dyn zephium_core::blocker::DeclarativeRuleText>>>,
    artifact_digest: [u8; 32],
    phase: Cell<CompilationPhase>,
    cancelled: Rc<Cell<bool>>,
    done: Cell<Option<F>>,
}

impl<F> CompilationContext<F>
where
    F: FnOnce(Result<NativeContentPolicy, ContentRuleApplyFailure>) + 'static,
{
    fn finish(&self, result: Result<NativeContentPolicy, ContentRuleApplyFailure>) {
        if self.phase.replace(CompilationPhase::Finished) == CompilationPhase::Finished {
            return;
        }
        let native_resources = (
            self.store.take(),
            self.native_identifier.take(),
            self.encoded.take(),
        );
        let done = self.done.take();
        // Empty every compiler-owned native/resource field before exposing
        // terminal settlement. Dispatching one main-queue turn later proves
        // the WK completion block has returned before engine shutdown can
        // acknowledge native quiescence.
        drop(native_resources);
        let Some(done) = done else {
            return;
        };
        let completion = Cell::new(Some((done, result)));
        let completion: RcBlock<dyn Fn()> = RcBlock::new(move || {
            if let Some((done, result)) = completion.take() {
                let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| done(result)));
            }
        });
        // SAFETY: dispatch_async copies this heap block before returning and
        // the exact callback is main-thread-bound by WKContentRuleListStore.
        unsafe {
            DispatchQueue::main().exec_async_with_block(RcBlock::as_ptr(&completion));
        }
    }
}

fn clone_cell_value<T: Clone>(cell: &Cell<Option<T>>) -> Option<T> {
    let value = cell.take()?;
    let cloned = value.clone();
    cell.set(Some(value));
    Some(cloned)
}

struct CacheMaintenanceContext<T, F> {
    store: Cell<Option<Retained<WKContentRuleListStore>>>,
    native_identifier: Cell<Option<Retained<NSString>>>,
    cancelled: Rc<Cell<bool>>,
    finished: Cell<bool>,
    done: Cell<Option<F>>,
    _result: std::marker::PhantomData<T>,
}

impl<T: 'static, F> CacheMaintenanceContext<T, F>
where
    F: FnOnce(Result<T, ()>) + 'static,
{
    fn finish(&self, result: Result<T, ()>) {
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.finish_inner(result);
        }));
    }

    fn finish_inner(&self, result: Result<T, ()>) {
        if self.finished.replace(true) {
            return;
        }
        let native_resources = (self.store.take(), self.native_identifier.take());
        let done = self.done.take();
        // Release every retained store/identifier before crossing back into
        // the host. One main-queue turn also proves the exact WK completion
        // block has returned before the maintenance slot can be released.
        drop(native_resources);
        let Some(done) = done else {
            return;
        };
        let completion = Cell::new(Some((done, result)));
        let completion: RcBlock<dyn Fn()> = RcBlock::new(move || {
            if let Some((done, result)) = completion.take() {
                let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| done(result)));
            }
        });
        unsafe {
            DispatchQueue::main().exec_async_with_block(RcBlock::as_ptr(&completion));
        }
    }
}

fn content_rule_list_store(cache: &Path) -> Option<Retained<WKContentRuleListStore>> {
    let mtm = MainThreadMarker::new()?;
    let cache = cache.to_str()?;
    let cache_path = NSString::from_str(cache);
    let cache_url = NSURL::fileURLWithPath_isDirectory(&cache_path, true);
    unsafe { WKContentRuleListStore::storeWithURL(Some(&cache_url), mtm) }
}

pub(crate) fn enumerate_content_policy_cache(
    cache: &Path,
    cursor: usize,
    done: impl FnOnce(Result<ContentPolicyCachePage, ()>) + 'static,
) -> ContentPolicyCacheMaintenanceCancellation {
    // WebKit owns/materializes the returned NSArray. Never call `to_vec`:
    // inspect one bounded index window and copy only exact-size ASCII names.
    let cancelled = Rc::new(Cell::new(false));
    let cancellation = ContentPolicyCacheMaintenanceCancellation {
        cancelled: cancelled.clone(),
    };
    let Some(store) = content_rule_list_store(cache) else {
        done(Err(()));
        return cancellation;
    };
    let context = Rc::new(CacheMaintenanceContext {
        store: Cell::new(Some(store.clone())),
        native_identifier: Cell::new(None),
        cancelled,
        finished: Cell::new(false),
        done: Cell::new(Some(done)),
        _result: std::marker::PhantomData,
    });
    let callback_context = context.clone();
    let completion = RcBlock::new(move |identifiers: *mut NSArray<NSString>| {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            if callback_context.cancelled.get() {
                return Err(());
            }
            let Some(identifiers) = (unsafe { Retained::retain(identifiers) }) else {
                return Err(());
            };
            Ok(content_policy_cache_page(&identifiers, cursor))
        }))
        .unwrap_or(Err(()));
        callback_context.finish(result);
    });
    unsafe {
        store.getAvailableContentRuleListIdentifiers(Some(&completion));
    }
    cancellation
}

pub(crate) fn remove_content_policy_cache_identifier(
    cache: &Path,
    identifier: String,
    done: impl FnOnce(Result<(), ()>) + 'static,
) -> ContentPolicyCacheMaintenanceCancellation {
    let cancelled = Rc::new(Cell::new(false));
    let cancellation = ContentPolicyCacheMaintenanceCancellation {
        cancelled: cancelled.clone(),
    };
    let Some(store) = content_rule_list_store(cache) else {
        done(Err(()));
        return cancellation;
    };
    if identifier.len() != MAX_CACHE_IDENTIFIER_BYTES || !identifier.is_ascii() {
        done(Err(()));
        return cancellation;
    }
    let native_identifier = NSString::from_str(&identifier);
    let context = Rc::new(CacheMaintenanceContext {
        store: Cell::new(Some(store.clone())),
        native_identifier: Cell::new(Some(native_identifier.clone())),
        cancelled,
        finished: Cell::new(false),
        done: Cell::new(Some(done)),
        _result: std::marker::PhantomData,
    });
    let callback_context = context.clone();
    let completion = RcBlock::new(move |error: *mut NSError| {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            if callback_context.cancelled.get() || !error.is_null() {
                Err(())
            } else {
                Ok(())
            }
        }))
        .unwrap_or(Err(()));
        callback_context.finish(result);
    });
    unsafe {
        store.removeContentRuleListForIdentifier_completionHandler(
            Some(&native_identifier),
            Some(&completion),
        );
    }
    cancellation
}

fn content_policy_cache_page(
    identifiers: &NSArray<NSString>,
    cursor: usize,
) -> ContentPolicyCachePage {
    let total = identifiers.len();
    let (start, end, scan_complete, over_budget) = content_policy_cache_window(total, cursor);
    let mut page = Vec::with_capacity((end - start).min(MAX_CACHE_IDENTIFIERS_PER_PAGE));
    for index in start..end {
        let identifier = identifiers.objectAtIndex(index);
        if identifier.len_utf16() == MAX_CACHE_IDENTIFIER_BYTES
            && identifier.len() == MAX_CACHE_IDENTIFIER_BYTES
        {
            let identifier = identifier.to_string();
            if identifier.is_ascii() {
                page.push(identifier);
            }
        }
    }
    ContentPolicyCachePage {
        identifiers: page,
        next_cursor: if scan_complete { 0 } else { end },
        scan_complete,
        over_budget,
    }
}

fn content_policy_cache_window(total: usize, cursor: usize) -> (usize, usize, bool, bool) {
    // The hard cursor ceiling prevents a corrupt native inventory from
    // turning maintenance into unbounded main-thread iteration. Normal GC
    // keeps Zephium's namespace far below it.
    let start = cursor.min(MAX_CACHE_IDENTIFIER_CURSOR).min(total);
    let end = start
        .saturating_add(MAX_CACHE_IDENTIFIERS_PER_PAGE)
        .min(total)
        .min(MAX_CACHE_IDENTIFIER_CURSOR);
    let over_budget = total > MAX_CACHE_IDENTIFIER_CURSOR;
    let scan_complete = end >= total || end >= MAX_CACHE_IDENTIFIER_CURSOR;
    (start, end, scan_complete, over_budget)
}

fn transition_phase(
    phase: &Cell<CompilationPhase>,
    expected: CompilationPhase,
    next: CompilationPhase,
) -> bool {
    if phase.get() != expected {
        return false;
    }
    phase.set(next);
    true
}

pub(crate) fn compile(
    cache: &Path,
    encoded: Arc<dyn zephium_core::blocker::DeclarativeRuleText>,
    artifact_digest: [u8; 32],
    done: impl FnOnce(Result<NativeContentPolicy, ContentRuleApplyFailure>) + 'static,
) -> ContentPolicyCompilationCancellation {
    let cancelled = Rc::new(Cell::new(false));
    let cancellation = ContentPolicyCompilationCancellation {
        cancelled: cancelled.clone(),
    };
    let Some(mtm) = MainThreadMarker::new() else {
        done(Err(ContentRuleApplyFailure::NativeCompilation));
        return cancellation;
    };
    let Some(cache) = cache.to_str() else {
        done(Err(ContentRuleApplyFailure::NativeCompilation));
        return cancellation;
    };
    let cache_path = NSString::from_str(cache);
    let cache_url = NSURL::fileURLWithPath_isDirectory(&cache_path, true);
    let Some(store) = (unsafe { WKContentRuleListStore::storeWithURL(Some(&cache_url), mtm) })
    else {
        done(Err(ContentRuleApplyFailure::NativeCompilation));
        return cancellation;
    };
    let identifier: Arc<str> = rule_identifier(artifact_digest).into();
    let context = Rc::new(CompilationContext {
        store: Cell::new(Some(store)),
        native_identifier: Cell::new(Some(NSString::from_str(&identifier))),
        identifier,
        encoded: Cell::new(Some(encoded.clone())),
        artifact_digest,
        phase: Cell::new(CompilationPhase::LookingUp),
        cancelled,
        done: Cell::new(Some(done)),
    });
    let Some(store) = clone_cell_value(&context.store) else {
        context.finish(Err(ContentRuleApplyFailure::NativeCompilation));
        return cancellation;
    };
    let Some(native_identifier) = clone_cell_value(&context.native_identifier) else {
        context.finish(Err(ContentRuleApplyFailure::NativeCompilation));
        return cancellation;
    };
    let context_for_lookup = context.clone();
    let lookup = RcBlock::new(
        move |list: *mut WKContentRuleList, error: *mut objc2_foundation::NSError| {
            let callback_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                handle_lookup_result(context_for_lookup.clone(), list, error);
            }));
            if callback_result.is_err() {
                context_for_lookup.finish(Err(ContentRuleApplyFailure::NativeCompilation));
            }
        },
    );
    // SAFETY: the retained store and identifier are main-thread values.
    // WebKit copies the block for its asynchronous lookup.
    unsafe {
        store.lookUpContentRuleListForIdentifier_completionHandler(
            Some(&native_identifier),
            Some(&lookup),
        );
    }
    cancellation
}

fn handle_lookup_result<F>(
    context: Rc<CompilationContext<F>>,
    list: *mut WKContentRuleList,
    error: *mut NSError,
) where
    F: FnOnce(Result<NativeContentPolicy, ContentRuleApplyFailure>) + 'static,
{
    if context.phase.get() != CompilationPhase::LookingUp {
        return;
    }
    if context.cancelled.get() {
        context.finish(Err(ContentRuleApplyFailure::NativeCompilation));
        return;
    }
    if error.is_null() && !list.is_null() {
        context.finish(validated_policy(
            list,
            &context.identifier,
            context.artifact_digest,
        ));
        return;
    }
    if !list.is_null() {
        context.finish(Err(ContentRuleApplyFailure::NativeCompilation));
        return;
    }

    match classify_lookup_failure(error) {
        LookupFailureAction::CompileAfterMiss => {
            if transition_phase(
                &context.phase,
                CompilationPhase::LookingUp,
                CompilationPhase::Compiling,
            ) {
                compile_content_rule_list(context);
            }
        }
        LookupFailureAction::RemoveStaleThenCompile => {
            if transition_phase(
                &context.phase,
                CompilationPhase::LookingUp,
                CompilationPhase::RemovingStale,
            ) {
                remove_stale_content_rule_list(context);
            }
        }
        LookupFailureAction::Fail => {
            context.finish(Err(ContentRuleApplyFailure::NativeCompilation));
        }
    }
}

fn remove_stale_content_rule_list<F>(context: Rc<CompilationContext<F>>)
where
    F: FnOnce(Result<NativeContentPolicy, ContentRuleApplyFailure>) + 'static,
{
    let Some(store) = clone_cell_value(&context.store) else {
        context.finish(Err(ContentRuleApplyFailure::NativeCompilation));
        return;
    };
    let Some(native_identifier) = clone_cell_value(&context.native_identifier) else {
        context.finish(Err(ContentRuleApplyFailure::NativeCompilation));
        return;
    };
    let context_for_removal = context.clone();
    let removed = RcBlock::new(move |error: *mut objc2_foundation::NSError| {
        let callback_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            handle_stale_removal_result(context_for_removal.clone(), error);
        }));
        if callback_result.is_err() {
            context_for_removal.finish(Err(ContentRuleApplyFailure::NativeCompilation));
        }
    });
    // SAFETY: the retained store and identifier are main-thread values.
    // WebKit copies the block for this asynchronous, exact-identifier removal.
    unsafe {
        store.removeContentRuleListForIdentifier_completionHandler(
            Some(&native_identifier),
            Some(&removed),
        );
    }
}

fn handle_stale_removal_result<F>(context: Rc<CompilationContext<F>>, error: *mut NSError)
where
    F: FnOnce(Result<NativeContentPolicy, ContentRuleApplyFailure>) + 'static,
{
    if context.phase.get() != CompilationPhase::RemovingStale {
        return;
    }
    if context.cancelled.get() {
        context.finish(Err(ContentRuleApplyFailure::NativeCompilation));
        return;
    }
    if !error.is_null() {
        context.finish(Err(ContentRuleApplyFailure::NativeCompilation));
        return;
    }
    if transition_phase(
        &context.phase,
        CompilationPhase::RemovingStale,
        CompilationPhase::Compiling,
    ) {
        compile_content_rule_list(context);
    }
}

fn compile_content_rule_list<F>(context: Rc<CompilationContext<F>>)
where
    F: FnOnce(Result<NativeContentPolicy, ContentRuleApplyFailure>) + 'static,
{
    if context.phase.get() != CompilationPhase::Compiling {
        return;
    }
    let Some(encoded) = context.encoded.take() else {
        context.finish(Err(ContentRuleApplyFailure::NativeCompilation));
        return;
    };
    let Some(mtm) = MainThreadMarker::new() else {
        context.finish(Err(ContentRuleApplyFailure::NativeCompilation));
        return;
    };
    // A cached artifact reads, re-checks and validates tens of megabytes
    // before it can be compiled; none of that may hold the main thread.
    let resume = MainThreadBound::new(context, mtm);
    DispatchQueue::global_queue(GlobalQueueIdentifier::QualityOfService(
        DispatchQoS::UserInitiated,
    ))
    .exec_async(move || {
        let text = encoded
            .load()
            .map(|text| ImmutableText(NSString::from_str(&text)));
        drop(encoded);
        DispatchQueue::main().exec_async(move || {
            let Some(mtm) = MainThreadMarker::new() else {
                return;
            };
            let context = resume.into_inner(mtm);
            match text {
                Some(ImmutableText(text)) => submit_content_rule_list(context, text),
                None => context.finish(Err(ContentRuleApplyFailure::InvalidArtifact)),
            }
        });
    });
}

/// A string built on a background queue for the main-thread compiler.
struct ImmutableText(Retained<NSString>);

// SAFETY: `NSString::from_str` creates a new immutable string with no other
// reference; immutable Foundation strings are safe to use from any thread, and
// this one moves to the main queue exactly once without being touched here.
unsafe impl Send for ImmutableText {}

fn submit_content_rule_list<F>(context: Rc<CompilationContext<F>>, encoded: Retained<NSString>)
where
    F: FnOnce(Result<NativeContentPolicy, ContentRuleApplyFailure>) + 'static,
{
    if context.phase.get() != CompilationPhase::Compiling {
        return;
    }
    if context.cancelled.get() {
        context.finish(Err(ContentRuleApplyFailure::NativeCompilation));
        return;
    }
    let Some(store) = clone_cell_value(&context.store) else {
        context.finish(Err(ContentRuleApplyFailure::NativeCompilation));
        return;
    };
    let Some(native_identifier) = clone_cell_value(&context.native_identifier) else {
        context.finish(Err(ContentRuleApplyFailure::NativeCompilation));
        return;
    };
    let retained_encoded = Cell::new(Some(encoded.clone()));
    let context_for_compile = context.clone();
    let compiled = RcBlock::new(
        move |list: *mut WKContentRuleList, error: *mut objc2_foundation::NSError| {
            let retained_encoded = retained_encoded.take();
            let callback_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                if context_for_compile.phase.get() != CompilationPhase::Compiling {
                    return;
                }
                if context_for_compile.cancelled.get() {
                    context_for_compile.finish(Err(ContentRuleApplyFailure::NativeCompilation));
                    return;
                }
                let result = if error.is_null() && !list.is_null() {
                    validated_policy(
                        list,
                        &context_for_compile.identifier,
                        context_for_compile.artifact_digest,
                    )
                } else {
                    Err(ContentRuleApplyFailure::NativeCompilation)
                };
                drop(retained_encoded);
                context_for_compile.finish(result);
            }));
            if callback_result.is_err() {
                context_for_compile.finish(Err(ContentRuleApplyFailure::NativeCompilation));
            }
        },
    );
    // SAFETY: the store, identifier, source and block are retained Objective-C
    // values created on the main thread. The block also retains the source
    // string until WebKit invokes the asynchronous completion.
    unsafe {
        store.compileContentRuleListForIdentifier_encodedContentRuleList_completionHandler(
            Some(&native_identifier),
            Some(&encoded),
            Some(&compiled),
        );
    }
}

fn validated_policy(
    list: *mut WKContentRuleList,
    expected_identifier: &Arc<str>,
    digest: [u8; 32],
) -> Result<NativeContentPolicy, ContentRuleApplyFailure> {
    // SAFETY: WebKit passes a valid +0 object for the duration of either
    // completion callback. Retaining it establishes host ownership first.
    let Some(list) = (unsafe { Retained::retain(list) }) else {
        return Err(ContentRuleApplyFailure::NativeCompilation);
    };
    if unsafe { list.identifier() }.to_string() != expected_identifier.as_ref() {
        return Err(ContentRuleApplyFailure::NativeCompilation);
    }
    Ok(NativeContentPolicy::Declarative {
        digest,
        identifier: expected_identifier.clone(),
        list,
    })
}

fn classify_lookup_failure(error: *mut NSError) -> LookupFailureAction {
    // SAFETY: an Objective-C completion owns the +0 NSError for the duration
    // of this call. Retaining it avoids depending on an autorelease scope
    // while reading the domain and code.
    let Some(error) = (unsafe { Retained::retain(error) }) else {
        return LookupFailureAction::Fail;
    };
    if !error.domain().isEqualToString(unsafe { WKErrorDomain }) {
        return LookupFailureAction::Fail;
    }
    match WKErrorCode(error.code()) {
        WKErrorCode::ContentRuleListStoreLookUpFailed => LookupFailureAction::CompileAfterMiss,
        WKErrorCode::ContentRuleListStoreVersionMismatch => {
            LookupFailureAction::RemoveStaleThenCompile
        }
        _ => LookupFailureAction::Fail,
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

    fn webkit_error(code: WKErrorCode) -> Retained<NSError> {
        unsafe { NSError::errorWithDomain_code_userInfo(WKErrorDomain, code.0, None) }
    }

    fn classify_retained_error(error: &Retained<NSError>) -> LookupFailureAction {
        classify_lookup_failure(Retained::as_ptr(error).cast_mut())
    }

    #[test]
    fn rule_identifiers_are_namespaced_and_digest_exact() {
        let left = rule_identifier([0; 32]);
        let right = rule_identifier([1; 32]);
        assert_eq!(left.len(), "app.zephium.rules.v1.".len() + 64);
        assert_ne!(left, right);
        assert!(left
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'.'));
    }

    #[test]
    fn only_exact_webkit_lookup_failures_are_recoverable() {
        let cache_miss = webkit_error(WKErrorCode::ContentRuleListStoreLookUpFailed);
        assert_eq!(
            classify_retained_error(&cache_miss),
            LookupFailureAction::CompileAfterMiss
        );

        let version_mismatch = webkit_error(WKErrorCode::ContentRuleListStoreVersionMismatch);
        assert_eq!(
            classify_retained_error(&version_mismatch),
            LookupFailureAction::RemoveStaleThenCompile
        );

        let removal_failure = webkit_error(WKErrorCode::ContentRuleListStoreRemoveFailed);
        assert_eq!(
            classify_retained_error(&removal_failure),
            LookupFailureAction::Fail
        );

        let foreign_domain = NSString::from_str("app.zephium.tests.not-webkit");
        let foreign_error = unsafe {
            NSError::errorWithDomain_code_userInfo(
                &foreign_domain,
                WKErrorCode::ContentRuleListStoreVersionMismatch.0,
                None,
            )
        };
        assert_eq!(
            classify_retained_error(&foreign_error),
            LookupFailureAction::Fail
        );
        assert_eq!(
            classify_lookup_failure(std::ptr::null_mut()),
            LookupFailureAction::Fail
        );
    }

    #[test]
    fn stale_removal_transition_is_exactly_once() {
        let phase = Cell::new(CompilationPhase::RemovingStale);
        assert!(transition_phase(
            &phase,
            CompilationPhase::RemovingStale,
            CompilationPhase::Compiling,
        ));
        assert!(!transition_phase(
            &phase,
            CompilationPhase::RemovingStale,
            CompilationPhase::Compiling,
        ));
        assert_eq!(phase.get(), CompilationPhase::Compiling);
    }

    #[test]
    fn cache_enumeration_pages_are_hard_bounded_and_terminate() {
        assert_eq!(content_policy_cache_window(0, 0), (0, 0, true, false));
        assert_eq!(content_policy_cache_window(129, 0), (0, 128, false, false));
        assert_eq!(
            content_policy_cache_window(129, 128),
            (128, 129, true, false)
        );
        assert_eq!(
            content_policy_cache_window(usize::MAX, MAX_CACHE_IDENTIFIER_CURSOR - 1),
            (
                MAX_CACHE_IDENTIFIER_CURSOR - 1,
                MAX_CACHE_IDENTIFIER_CURSOR,
                true,
                true,
            )
        );
    }

    #[test]
    fn cache_maintenance_owns_exact_callbacks_and_never_bulk_removes() {
        let source = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/src/platform/macos/content_filter.rs"
        ));
        let production = source
            .split_once("#[cfg(test)]")
            .expect("content-filter test boundary disappeared")
            .0;
        assert!(production.contains("getAvailableContentRuleListIdentifiers"));
        assert!(production.contains("removeContentRuleListForIdentifier_completionHandler"));
        assert!(!production.contains("removeAllContentRuleLists"));

        let context = production
            .find("struct CacheMaintenanceContext")
            .expect("cache maintenance ownership context disappeared");
        let compiler = production
            .find("fn transition_phase")
            .expect("cache maintenance boundary disappeared");
        let maintenance = &production[context..compiler];
        let finish = maintenance
            .find("fn finish(&self, result:")
            .expect("cache maintenance terminal disappeared");
        let enumerate = maintenance
            .find("pub(crate) fn enumerate_content_policy_cache")
            .expect("cache enumeration disappeared");
        let remove = maintenance
            .find("pub(crate) fn remove_content_policy_cache_identifier")
            .expect("cache exact removal disappeared");
        assert!(finish < enumerate && enumerate < remove);
        let finish_source = &maintenance[finish..enumerate];
        let release_store = finish_source
            .find("self.store.take()")
            .expect("cache store is not retired");
        let release_identifier = finish_source
            .find("self.native_identifier.take()")
            .expect("cache identifier is not retired");
        let drop_resources = finish_source
            .find("drop(native_resources);")
            .expect("cache native resources are not dropped");
        let dispatch = finish_source
            .find("exec_async_with_block")
            .expect("cache completion is not deferred past the WK callback");
        assert!(
            release_store < drop_resources
                && release_identifier < drop_resources
                && drop_resources < dispatch
        );
        assert_eq!(
            maintenance.matches("catch_unwind").count(),
            4,
            "enumeration, removal, retirement, and deferred host delivery must contain unwinding"
        );
        assert_eq!(
            maintenance
                .matches("callback_context.cancelled.get()")
                .count(),
            2,
            "both native maintenance callbacks must honor advisory cancellation"
        );
        assert!(maintenance.contains("identifier.len() != MAX_CACHE_IDENTIFIER_BYTES"));
        assert!(maintenance.contains("page.push(identifier);"));
    }

    #[test]
    fn persistent_lookup_repair_precedes_single_compile_and_validates_results() {
        let source = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/src/platform/macos/content_filter.rs"
        ));
        let production = source
            .split_once("#[cfg(test)]")
            .expect("content-filter test boundary disappeared")
            .0;
        let compiler = production
            .find("pub(crate) fn compile(")
            .map(|start| &production[start..])
            .expect("content-rule compiler boundary disappeared");
        let lookup = compiler
            .find("lookUpContentRuleListForIdentifier_completionHandler")
            .expect("persistent content-rule lookup disappeared");
        let removal = compiler
            .find("removeContentRuleListForIdentifier_completionHandler")
            .expect("exact stale content-rule removal disappeared");
        let compile = compiler
            .find("compileContentRuleListForIdentifier_encodedContentRuleList_completionHandler")
            .expect("content-rule compilation disappeared");
        assert!(
            lookup < removal && removal < compile,
            "lookup and bounded stale repair must precede native compilation"
        );
        assert_eq!(
            compiler
                .matches(
                    "compileContentRuleListForIdentifier_encodedContentRuleList_completionHandler"
                )
                .count(),
            1,
            "there must be only one native compilation site"
        );
        assert!(
            !production.contains("removeAllContentRuleLists"),
            "cache repair must never remove another owner or digest"
        );
        let removal_handler = compiler
            .find("fn handle_stale_removal_result")
            .expect("stale-removal settlement disappeared");
        let compile_function = compiler
            .find("fn compile_content_rule_list")
            .expect("single native compile function disappeared");
        let removal_handler = &compiler[removal_handler..compile_function];
        let require_success = removal_handler
            .find("if !error.is_null()")
            .expect("stale removal no longer requires native success");
        let one_shot_transition = removal_handler
            .find("transition_phase(")
            .expect("stale removal lost its one-shot phase transition");
        let compile_after_success = removal_handler
            .find("compile_content_rule_list(context);")
            .expect("successful stale removal no longer compiles");
        assert!(
            require_success < one_shot_transition && one_shot_transition < compile_after_success,
            "only a successful one-shot removal transition may compile"
        );
        assert_eq!(
            compiler[..compiler
                .find("fn validated_policy")
                .expect("native identity validator disappeared")]
                .matches("validated_policy(")
                .count(),
            2,
            "cache hits and compiled lists must both validate native identity"
        );
        assert!(compiler.contains("list.identifier()"));
        assert_eq!(
            compiler.matches("catch_unwind").count(),
            3,
            "lookup, repair, and compilation callbacks may not unwind across Objective-C"
        );
        assert_eq!(
            compiler.matches("if callback_result.is_err()").count(),
            3,
            "every caught WebKit callback panic must settle the compiler attempt"
        );
        assert_eq!(
            compiler.matches(".cancelled.get()").count(),
            4,
            "timeout must stop lookup, stale-removal, text loading and compilation callbacks"
        );
        let finish = production
            .find("fn finish(&self, result:")
            .expect("compiler terminal retirement disappeared");
        let clone_cell = production
            .find("fn clone_cell_value")
            .expect("native resource clone boundary disappeared");
        let finish_source = &production[finish..clone_cell];
        let release_store = finish_source
            .find("self.store.take()")
            .expect("compiler store is retained through terminal delivery");
        let take_done = finish_source
            .find("let done = self.done.take();")
            .expect("compiler completion ownership disappeared");
        let drop_resources = finish_source
            .find("drop(native_resources);")
            .expect("compiler native resources are not explicitly retired");
        let dispatch = finish_source
            .find("exec_async_with_block")
            .expect("terminal callback is not deferred past the WK callback stack");
        assert!(
            release_store < take_done && take_done < drop_resources && drop_resources < dispatch,
            "native compiler resources must be retired before deferred host settlement"
        );
        assert!(
            compiler.contains("let retained_encoded = retained_encoded.take();"),
            "the WK completion block can retain its encoded NSString after terminal settlement"
        );
    }
}
