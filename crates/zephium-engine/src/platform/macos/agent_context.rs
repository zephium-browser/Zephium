#![deny(unsafe_op_in_unsafe_fn)]
#![deny(clippy::undocumented_unsafe_blocks)]
#![deny(clippy::dbg_macro, clippy::print_stderr, clippy::print_stdout)]
#![cfg_attr(
    not(test),
    deny(clippy::panic, clippy::unreachable, clippy::unwrap_used)
)]

//! Extension-free, hidden WKWebView construction for owned agent contexts.

use std::rc::Rc;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Instant;
use wry::WebViewExtMacOS as _;

use raw_window_handle::HasWindowHandle;
use wry::dpi::{LogicalPosition, LogicalSize, Position, Size};
use wry::{
    DownloadPolicy, PageClosePolicy, Rect, WebView, WebViewBuilder, WebViewBuilderExtDarwin as _,
    WebViewBuilderExtMacos as _,
};
use zephium_agentic::{
    ContextOwnedViewport, ContextProfileStorageClass, SemanticActionNativeFailure,
    SemanticActionNativeRequest, SemanticActionNativeSettlement, SemanticRuntimeInvocation,
    SemanticRuntimePortFailure, SemanticScreenshotNativeCapture, SemanticScreenshotNativeFailure,
    SemanticScreenshotNativeRequest, SemanticSnapshot,
};
use zephium_core::ids::ProfileId;

use crate::platform::agent_navigation::AgentNavigationController;
pub(crate) use crate::platform::agent_navigation::{
    AgentNavigationCommit, AgentNavigationTerminal,
};

use super::semantic_runtime::{
    AgentSemanticRuntimeController, AgentSemanticRuntimeDispatchError, AgentSemanticRuntimeFailure,
    AgentSemanticRuntimeRegistration,
};
use super::WebsiteDataStore;

/// Closed construction failure mapped to the public native-port taxonomy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum AgentOwnedViewConstructionError {
    /// The exact selected-profile storage class or identity was not retained.
    Storage,
    /// Extension absence or exact private semantic-script inventory could not be proven.
    ExtensionIsolation,
    /// Wry/WebKit refused hidden child-view construction or hardening.
    Native,
}

fn invoke_owned_navigation_callback(
    callback: &dyn Fn(AgentNavigationTerminal),
    callback_panicked: &dyn Fn(),
    terminal: AgentNavigationTerminal,
) {
    if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| callback(terminal))).is_err() {
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(callback_panicked));
    }
}

fn invoke_owned_unit_callback(callback: &dyn Fn(), callback_panicked: &dyn Fn()) {
    if std::panic::catch_unwind(std::panic::AssertUnwindSafe(callback)).is_err() {
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(callback_panicked));
    }
}

fn new_owned_agent_configuration(
    profile: ProfileId,
    storage_class: ContextProfileStorageClass,
    ephemeral_store: Option<&WebsiteDataStore>,
) -> Result<
    objc2::rc::Retained<objc2_web_kit::WKWebViewConfiguration>,
    AgentOwnedViewConstructionError,
> {
    use objc2::rc::Retained;
    use objc2_foundation::{MainThreadMarker, NSUUID};
    use objc2_web_kit::{WKWebViewConfiguration, WKWebsiteDataStore};

    match (storage_class, ephemeral_store) {
        (ContextProfileStorageClass::Ephemeral, Some(store)) => {
            super::new_configuration_with_data_store(store)
                .map_err(|_| AgentOwnedViewConstructionError::Storage)
        }
        (ContextProfileStorageClass::Durable, None) => {
            let mtm = MainThreadMarker::new().ok_or(AgentOwnedViewConstructionError::Native)?;
            let identifier = NSUUID::from_bytes(profile.bytes());
            // SAFETY: `mtm` proves all WebKit messages run on the main thread;
            // the UUID and returned retained objects remain live throughout
            // the calls, and each result is attested before use.
            let (store, configuration, actual_store) = unsafe {
                let store = WKWebsiteDataStore::dataStoreForIdentifier(&identifier, mtm);
                if !store.isPersistent()
                    || store.identifier().map(|value| value.as_bytes()) != Some(profile.bytes())
                {
                    return Err(AgentOwnedViewConstructionError::Storage);
                }
                let configuration = WKWebViewConfiguration::new(mtm);
                configuration.setWebsiteDataStore(&store);
                let actual_store = configuration.websiteDataStore();
                (store, configuration, actual_store)
            };
            if Retained::as_ptr(&actual_store) != Retained::as_ptr(&store) {
                return Err(AgentOwnedViewConstructionError::Storage);
            }
            super::proxy::apply(&configuration, crate::network::current().as_ref())
                .map_err(|_| AgentOwnedViewConstructionError::Native)?;
            Ok(configuration)
        }
        // A durable class with its own store is Work's persistent store.
        (ContextProfileStorageClass::Durable, Some(store)) => {
            let mtm = MainThreadMarker::new().ok_or(AgentOwnedViewConstructionError::Native)?;
            if !work_store_matches(profile, store) {
                return Err(AgentOwnedViewConstructionError::Storage);
            }
            // SAFETY: `mtm` proves main-thread WebKit access; the retained
            // store and configuration stay live for these calls.
            let (configuration, actual_store) = unsafe {
                let configuration = WKWebViewConfiguration::new(mtm);
                configuration.setWebsiteDataStore(store);
                let actual_store = configuration.websiteDataStore();
                (configuration, actual_store)
            };
            if Retained::as_ptr(&actual_store) != Retained::as_ptr(store) {
                return Err(AgentOwnedViewConstructionError::Storage);
            }
            super::proxy::apply(&configuration, crate::network::current().as_ref())
                .map_err(|_| AgentOwnedViewConstructionError::Native)?;
            Ok(configuration)
        }
        (ContextProfileStorageClass::Ephemeral, None) => {
            Err(AgentOwnedViewConstructionError::Storage)
        }
    }
}

/// The profile's persistent Work store, created by WebKit on first use.
pub(crate) fn work_data_store(profile: ProfileId) -> Result<WebsiteDataStore, &'static str> {
    use objc2_foundation::{MainThreadMarker, NSUUID};
    use objc2_web_kit::WKWebsiteDataStore;

    let mtm = MainThreadMarker::new().ok_or("WKWebsiteDataStore requires the main thread")?;
    let identifier = NSUUID::from_bytes(crate::work_website_data::identifier(profile));
    // SAFETY: `mtm` proves main-thread WebKit access; the UUID outlives the call.
    let store = unsafe { WKWebsiteDataStore::dataStoreForIdentifier(&identifier, mtm) };
    if !work_store_matches(profile, &store) {
        return Err("Work website data store identity mismatch");
    }
    Ok(store)
}

/// Removes every kind of data from the profile's Work store, if one exists,
/// and replies whether a fresh fetch then finds none. Live Work pages keep
/// running on the emptied store.
pub(crate) fn clear_work_data_store(profile: ProfileId, reply: std::sync::mpsc::Sender<bool>) {
    use wry::WebViewExtDarwin;

    if super::initialize_data_store_enumeration().is_err() {
        let _ = reply.send(false);
        return;
    }
    let identifier = crate::work_website_data::identifier(profile);
    let fallback = reply.clone();
    let listed =
        <wry::WebView as WebViewExtDarwin>::fetch_data_store_identifiers(move |identifiers| {
            if !identifiers.contains(&identifier) {
                let _ = reply.send(true);
                return;
            }
            match work_data_store(profile) {
                Ok(store) => clear_data_store(store, reply),
                Err(_) => {
                    let _ = reply.send(false);
                }
            }
        });
    if listed.is_err() {
        let _ = fallback.send(false);
    }
}

fn clear_data_store(store: WebsiteDataStore, reply: std::sync::mpsc::Sender<bool>) {
    use objc2_foundation::{MainThreadMarker, NSArray, NSDate};
    use objc2_web_kit::{WKWebsiteDataRecord, WKWebsiteDataStore};

    let Some(mtm) = MainThreadMarker::new() else {
        let _ = reply.send(false);
        return;
    };
    // SAFETY: `mtm` proves main-thread WebKit access for every call below;
    // each block keeps its retained store and types alive until WebKit calls it.
    unsafe {
        let types = WKWebsiteDataStore::allWebsiteDataTypes(mtm);
        let epoch = NSDate::dateWithTimeIntervalSince1970(0.0);
        let verify_store = store.clone();
        let verify_types = types.clone();
        let removed = block2::RcBlock::new(move || {
            let reply = reply.clone();
            let retained = verify_store.clone();
            let fetched = block2::RcBlock::new(
                move |records: std::ptr::NonNull<NSArray<WKWebsiteDataRecord>>| {
                    let _ = &retained;
                    let _ = reply.send(records.as_ref().count() == 0);
                },
            );
            verify_store.fetchDataRecordsOfTypes_completionHandler(&verify_types, &fetched);
        });
        store.removeDataOfTypes_modifiedSince_completionHandler(&types, &epoch, &removed);
    }
}

/// Whether a store is exactly the profile's persistent Work store.
pub(crate) fn work_store_matches(profile: ProfileId, store: &WebsiteDataStore) -> bool {
    let expected = crate::work_website_data::identifier(profile);
    // SAFETY: callers run on the main thread; the retained store stays live.
    let (persistent, identifier) = unsafe {
        (
            store.isPersistent(),
            store.identifier().map(|id| id.as_bytes()),
        )
    };
    persistent && identifier == Some(expected) && identifier != Some(profile.bytes())
}

/// Exact native page and its closed navigation policy handle.
pub(crate) struct AgentOwnedView {
    navigation: AgentNavigationController,
    work_navigation: Option<crate::platform::work_document_navigation::WorkDocumentNavigation>,
    semantic: Option<AgentSemanticRuntimeRegistration>,
    history: super::agent_history::AgentHistoryLedger,
    next_document_runtime: u64,
    viewport: ContextOwnedViewport,
    _navigation_observer: super::InstalledNavigationObserver,
    view: WebView,
}

impl AgentOwnedView {
    pub(crate) const fn view(&self) -> &WebView {
        &self.view
    }

    pub(crate) const fn navigation(&self) -> &AgentNavigationController {
        &self.navigation
    }
    pub(crate) fn work_navigation(
        &self,
    ) -> Option<&crate::platform::work_document_navigation::WorkDocumentNavigation> {
        self.work_navigation.as_ref()
    }

    #[cfg(feature = "native-agentic-semantic-probe")]
    pub(crate) fn form_observer_world(
        &self,
    ) -> Option<objc2::rc::Retained<objc2_web_kit::WKContentWorld>> {
        self.semantic.as_ref()?.form_observer_world()
    }

    pub(crate) fn sample_work_human_wall(
        &self,
        completion: impl FnOnce(String) + 'static,
    ) -> Result<(), ()> {
        // Only the original, live human-owned navigation phase may run this
        // fixed survey. It is never an automated or hidden-decision tool.
        self.work_navigation()
            .and_then(|gate| gate.human_wall_probe())
            .ok_or(())?;
        use wry::WebViewExtMacOS as _;
        self.semantic
            .as_ref()
            .ok_or(())?
            .sample_human_wall(&self.view.webview(), completion)
    }

    pub(crate) fn semantic(&self) -> Option<&AgentSemanticRuntimeController> {
        self.semantic
            .as_ref()
            .map(AgentSemanticRuntimeRegistration::controller)
    }

    pub(crate) fn prepare_semantic_document_load(&mut self) -> Result<(), ()> {
        self.semantic.as_mut().ok_or(())?.prepare_document_load()
    }

    pub(crate) fn retire_semantic_runtime(&mut self) -> bool {
        self.history.clear();
        self.semantic
            .take()
            .is_some_and(|registration| registration.retire().is_ok())
    }

    pub(crate) fn enroll_current_work_history_get(
        &mut self,
        target: zephium_agentic::ContextNavigationTarget,
    ) -> Result<(), ()> {
        let runtime = super::agent_history::AgentDocumentRuntimeId::new(self.next_document_runtime)
            .ok_or(())?;
        let page = super::native_webview(&self.view);
        // SAFETY: all owned-view lifecycle operations run on WebKit's main
        // thread. objc2 retains the current native history item.
        let item = unsafe { page.backForwardList().currentItem() }.ok_or(())?;
        self.history.enroll_get(item, target, runtime)?;
        if self
            .semantic
            .as_mut()
            .ok_or(())?
            .bind_active_runtime(runtime)
            .is_err()
        {
            self.history.clear();
            return Err(());
        }
        self.next_document_runtime = self.next_document_runtime.checked_add(1).ok_or(())?;
        Ok(())
    }

    pub(crate) fn begin_history_lease(
        &mut self,
        target: zephium_agentic::ContextNavigationTarget,
    ) -> Result<(), ()> {
        self.history.clear();
        self.semantic
            .as_mut()
            .ok_or(())?
            .reset_history_authority()?;
        self.enroll_current_work_history_get(target)
    }

    pub(crate) fn park_semantic_runtime(
        &mut self,
        completion: impl FnOnce(bool) + 'static,
    ) -> Result<(), ()> {
        self.semantic.as_mut().ok_or(())?.park_active(completion)
    }

    /// Enrolls a document a hand-on load put in place, so a later admitted
    /// navigation can park its runtime like any navigated document's.
    pub(crate) fn enroll_handed_on_document(
        &mut self,
        target: zephium_agentic::ContextNavigationTarget,
    ) -> Result<(), ()> {
        if !self
            .semantic
            .as_ref()
            .is_some_and(AgentSemanticRuntimeRegistration::active_unbound)
        {
            return Ok(());
        }
        self.enroll_current_work_history_get(target)
    }

    pub(crate) fn semantic_runtime_parked(&self) -> bool {
        self.semantic
            .as_ref()
            .is_some_and(AgentSemanticRuntimeRegistration::active_parked)
    }

    pub(crate) fn semantic_runtime_ready_for_history(&self) -> bool {
        self.semantic
            .as_ref()
            .is_some_and(AgentSemanticRuntimeRegistration::active_ready_for_history)
    }

    pub(crate) fn prepare_history_back(
        &mut self,
    ) -> Result<super::agent_history::AgentHistoryBackTicket, ()> {
        let page = super::native_webview(&self.view);
        // SAFETY: owned agent views and their native history are confined to
        // WebKit's main thread for this entire authorization read.
        let (current, predecessor) = unsafe {
            let list = page.backForwardList();
            (list.currentItem(), list.backItem())
        };
        self.history.authorize_back(
            current.as_deref().ok_or(())?,
            predecessor.as_deref().ok_or(())?,
        )
    }

    pub(crate) fn history_destination(
        &self,
        ticket: super::agent_history::AgentHistoryBackTicket,
    ) -> Option<&zephium_agentic::ContextNavigationTarget> {
        self.history.destination_target(ticket)
    }

    pub(crate) fn reactivate_history_destination(
        &mut self,
        ticket: super::agent_history::AgentHistoryBackTicket,
    ) -> Result<zephium_agentic::ContextNavigationTarget, ()> {
        let target = self.history.destination_target(ticket).cloned().ok_or(())?;
        self.semantic
            .as_mut()
            .ok_or(())?
            .reactivate_runtime(ticket.destination_runtime())?;
        Ok(target)
    }

    pub(crate) fn dispatch_history_back(
        &mut self,
        ticket: super::agent_history::AgentHistoryBackTicket,
    ) -> bool {
        let page = super::native_webview(&self.view);
        // SAFETY: all values are retained from this exact page's native list
        // and immediately revalidated by opaque identity before dispatch.
        let (current, predecessor) = unsafe {
            let list = page.backForwardList();
            (list.currentItem(), list.backItem())
        };
        let Some(item) = current.as_deref().and_then(|current| {
            predecessor
                .as_deref()
                .and_then(|predecessor| self.history.dispatch_item(ticket, current, predecessor))
        }) else {
            return false;
        };
        // SAFETY: the ledger returned the exact retained predecessor after an
        // immediate identity join against this page's current native list.
        unsafe { page.goToBackForwardListItem(item) }.is_some()
    }

    pub(crate) fn settle_history_back(
        &mut self,
        ticket: super::agent_history::AgentHistoryBackTicket,
    ) -> Result<(), ()> {
        let page = super::native_webview(&self.view);
        // SAFETY: currentItem is a bounded retained identity read on WebKit's
        // main thread; the ledger requires the authorized destination exactly.
        let current = unsafe { page.backForwardList().currentItem() }.ok_or(())?;
        self.history.settle_back(ticket, &current, None)
    }

    pub(crate) fn refuse_history_back(
        &mut self,
        ticket: super::agent_history::AgentHistoryBackTicket,
    ) -> bool {
        self.history.refuse_back(ticket)
    }

    pub(crate) fn dispatch_semantic(
        &self,
        invocation: SemanticRuntimeInvocation,
        completion: impl FnOnce(Result<SemanticSnapshot, SemanticRuntimePortFailure>) + 'static,
    ) -> Result<(), SemanticRuntimePortFailure> {
        let Some(semantic) = self.semantic() else {
            completion(Err(SemanticRuntimePortFailure::Retired));
            return Err(SemanticRuntimePortFailure::Retired);
        };
        semantic
            .dispatch(invocation, move |outcome| {
                completion(outcome.map_err(map_semantic_runtime_failure));
            })
            .map_err(map_semantic_dispatch_failure)
    }

    pub(crate) fn record_media_ports(
        &self,
    ) -> Option<(
        crate::platform::record_media::MediaDispatch,
        crate::platform::record_media::FrameCapture,
        crate::platform::record_media::PrivacyDispatch,
    )> {
        let semantic = self.semantic()?.clone();
        let native = self.view.webview();
        let capture = Rc::new(
            move |crop, completion: crate::platform::record_media::FrameCompletion| {
                super::semantic_screenshot::capture_record_frame_native(&native, crop, completion)
            },
        );
        let privacy_runtime = semantic.clone();
        let privacy = Rc::new(move |reply| privacy_runtime.dispatch_privacy(reply));
        let dispatch = Rc::new(
            move |plan: &zephium_agentic::SemanticRecordMediaPlan,
                  validate: bool,
                  fetch: crate::platform::record_media::MediaFetchAttempt,
                  completion: crate::platform::record_media::MediaCompletion| {
                semantic.dispatch_media(plan, validate, fetch, completion)
            },
        );
        Some((dispatch, capture, privacy))
    }

    pub(crate) fn dispatch_screenshot(
        &self,
        request: SemanticScreenshotNativeRequest,
        admitted_at: Instant,
        cancelled: Arc<AtomicBool>,
        completion: impl FnOnce(
                Result<
                    crate::platform::semantic_pixel_privacy::ModelCapture<
                        SemanticScreenshotNativeCapture,
                    >,
                    SemanticScreenshotNativeFailure,
                >,
            ) + 'static,
        callback_panicked: impl Fn() + 'static,
    ) -> Result<(), SemanticScreenshotNativeFailure> {
        let Some(semantic) = self.semantic().cloned() else {
            return Err(SemanticScreenshotNativeFailure::NotReady);
        };
        let native = self.view.webview();
        let capture_cancelled = cancelled.clone();
        let public = request.requires_public_page();
        crate::platform::semantic_pixel_privacy::guarded_model_capture(
            public,
            Rc::new(move |completion| {
                if public {
                    semantic.dispatch_public_privacy(completion)
                } else {
                    semantic.dispatch_privacy(completion)
                }
            }),
            move |completion| {
                super::semantic_screenshot::capture_viewport(
                    &native,
                    request,
                    admitted_at,
                    capture_cancelled,
                    completion,
                    callback_panicked,
                )
            },
            cancelled,
            completion,
        );
        Ok(())
    }

    pub(crate) fn dispatch_semantic_action(
        &self,
        request: SemanticActionNativeRequest,
        admitted_at: Instant,
        completion: impl FnOnce(SemanticActionNativeSettlement) + 'static,
    ) {
        let Some(semantic) = self.semantic() else {
            let completed_at = request.requested_at();
            completion(request.fail(SemanticActionNativeFailure::Shutdown, completed_at));
            return;
        };
        super::semantic_action::dispatch(&self.view, semantic, request, admitted_at, completion);
    }

    pub(crate) fn semantic_pending_for_audit(&self) -> Option<bool> {
        self.semantic()?.pending_for_audit()
    }

    pub(crate) fn dispatch_retained_semantic_action(
        &self,
        request: SemanticActionNativeRequest,
        admitted_at: Instant,
        authority: Box<dyn Fn() -> bool>,
        completion: impl FnOnce(SemanticActionNativeSettlement) + 'static,
    ) {
        let Some(semantic) = self.semantic() else {
            let completed_at = request.requested_at();
            completion(request.fail(SemanticActionNativeFailure::Shutdown, completed_at));
            return;
        };
        super::semantic_action::dispatch_guarded(
            &self.view,
            semantic,
            request,
            admitted_at,
            Some(authority),
            completion,
        );
    }

    pub(crate) fn attest(
        &self,
        profile: ProfileId,
        storage_class: ContextProfileStorageClass,
        ephemeral_store: Option<&WebsiteDataStore>,
    ) -> Result<(), AgentOwnedViewConstructionError> {
        let semantic = self
            .semantic
            .as_ref()
            .ok_or(AgentOwnedViewConstructionError::ExtensionIsolation)?;
        attest_owned_agent_view(
            &self.view,
            semantic,
            self.viewport,
            profile,
            storage_class,
            ephemeral_store,
        )
    }
}

const fn map_semantic_dispatch_failure(
    failure: AgentSemanticRuntimeDispatchError,
) -> SemanticRuntimePortFailure {
    match failure {
        AgentSemanticRuntimeDispatchError::NotReady => SemanticRuntimePortFailure::NotReady,
        AgentSemanticRuntimeDispatchError::Busy => SemanticRuntimePortFailure::ResourceExhausted,
        AgentSemanticRuntimeDispatchError::Exhausted => SemanticRuntimePortFailure::InvocationLimit,
        AgentSemanticRuntimeDispatchError::Retired => SemanticRuntimePortFailure::Retired,
    }
}

const fn map_semantic_runtime_failure(
    failure: AgentSemanticRuntimeFailure,
) -> SemanticRuntimePortFailure {
    match failure {
        AgentSemanticRuntimeFailure::Dispatch(failure) => map_semantic_dispatch_failure(failure),
        AgentSemanticRuntimeFailure::Cancelled => SemanticRuntimePortFailure::Cancelled,
        AgentSemanticRuntimeFailure::DocumentReplaced => {
            SemanticRuntimePortFailure::DocumentReplaced
        }
        AgentSemanticRuntimeFailure::RendererLost => SemanticRuntimePortFailure::RendererLost,
        AgentSemanticRuntimeFailure::TimedOut => SemanticRuntimePortFailure::TimedOut,
        AgentSemanticRuntimeFailure::Retired => SemanticRuntimePortFailure::Retired,
        AgentSemanticRuntimeFailure::Transport => SemanticRuntimePortFailure::Transport,
        AgentSemanticRuntimeFailure::Result(failure) => SemanticRuntimePortFailure::Result(failure),
    }
}

/// Typed callback cohort retained by one native view delegate graph.
pub(crate) struct AgentOwnedViewCallbacks<Navigation, Location, RendererLost, Invariant, Panic> {
    #[cfg(feature = "native-agentic-work-lifetime-diagnostic")]
    failure_diagnostic: Option<Rc<dyn Fn(crate::WorkResourceFailureCause)>>,
    redirect: Option<Rc<dyn Fn(zephium_agentic::ContextNavigationTarget)>>,
    credential_denied: Option<Rc<dyn Fn()>>,
    navigation: Navigation,
    location: Location,
    renderer_lost: RendererLost,
    invariant: Invariant,
    panic: Panic,
}

impl<Navigation, Location, RendererLost, Invariant, Panic>
    AgentOwnedViewCallbacks<Navigation, Location, RendererLost, Invariant, Panic>
{
    pub(crate) const fn new(
        navigation: Navigation,
        location: Location,
        renderer_lost: RendererLost,
        invariant: Invariant,
        panic: Panic,
    ) -> Self {
        Self {
            #[cfg(feature = "native-agentic-work-lifetime-diagnostic")]
            failure_diagnostic: None,
            redirect: None,
            credential_denied: None,
            navigation,
            location,
            renderer_lost,
            invariant,
            panic,
        }
    }
    /// Removal-only native signal, bound to this exact owned view cohort.
    pub(crate) fn with_credential_denied(mut self, callback: impl Fn() + 'static) -> Self {
        self.credential_denied = Some(Rc::new(callback));
        self
    }
    /// Ends a public document whose first load the Work gate refused as a
    /// redirect. Without it, that refusal is an invariant failure.
    pub(crate) fn with_redirect(
        mut self,
        redirect: impl Fn(zephium_agentic::ContextNavigationTarget) + 'static,
    ) -> Self {
        self.redirect = Some(Rc::new(redirect));
        self
    }
    #[cfg(feature = "native-agentic-work-lifetime-diagnostic")]
    pub(crate) fn with_failure_diagnostic(
        mut self,
        diagnostic: impl Fn(crate::WorkResourceFailureCause) + 'static,
    ) -> Self {
        self.failure_diagnostic = Some(Rc::new(diagnostic));
        self
    }
}

/// Builds one initially hidden, extension-free selected-profile WKWebView.
///
/// The only initial document is `about:blank`. Network navigation remains
/// denied until a later exact context-navigation adapter is installed, so the
/// caller can attach native content policy before any web request exists.
pub(crate) fn build_owned_agent_view<Navigation, Location, RendererLost, Invariant, Panic>(
    parent: &impl HasWindowHandle,
    viewport: ContextOwnedViewport,
    profile: ProfileId,
    storage_class: ContextProfileStorageClass,
    ephemeral_store: Option<&WebsiteDataStore>,
    callbacks: AgentOwnedViewCallbacks<Navigation, Location, RendererLost, Invariant, Panic>,
) -> Result<AgentOwnedView, AgentOwnedViewConstructionError>
where
    Navigation: Fn(AgentNavigationTerminal) + 'static,
    Location: Fn() + 'static,
    RendererLost: Fn() + 'static,
    Invariant: Fn() + 'static,
    Panic: Fn() + 'static,
{
    build_private_agent_view(
        parent,
        viewport,
        profile,
        storage_class,
        ephemeral_store,
        callbacks,
        None,
    )
}

/// Same hardened selected-profile constructor with a distinct run-free,
/// one-document Work navigation gate. It never uses the legacy controller.
pub(crate) fn build_owned_work_view<Navigation, Location, RendererLost, Invariant, Panic>(
    parent: &impl HasWindowHandle,
    viewport: ContextOwnedViewport,
    profile: ProfileId,
    storage_class: ContextProfileStorageClass,
    ephemeral_store: Option<&WebsiteDataStore>,
    callbacks: AgentOwnedViewCallbacks<Navigation, Location, RendererLost, Invariant, Panic>,
) -> Result<AgentOwnedView, AgentOwnedViewConstructionError>
where
    Navigation: Fn(AgentNavigationTerminal) + 'static,
    Location: Fn() + 'static,
    RendererLost: Fn() + 'static,
    Invariant: Fn() + 'static,
    Panic: Fn() + 'static,
{
    build_private_agent_view(
        parent,
        viewport,
        profile,
        storage_class,
        ephemeral_store,
        callbacks,
        Some(crate::platform::work_document_navigation::WorkDocumentNavigation::default()),
    )
}

fn build_private_agent_view<Navigation, Location, RendererLost, Invariant, Panic>(
    parent: &impl HasWindowHandle,
    viewport: ContextOwnedViewport,
    profile: ProfileId,
    storage_class: ContextProfileStorageClass,
    ephemeral_store: Option<&WebsiteDataStore>,
    callbacks: AgentOwnedViewCallbacks<Navigation, Location, RendererLost, Invariant, Panic>,
    work_navigation: Option<crate::platform::work_document_navigation::WorkDocumentNavigation>,
) -> Result<AgentOwnedView, AgentOwnedViewConstructionError>
where
    Navigation: Fn(AgentNavigationTerminal) + 'static,
    Location: Fn() + 'static,
    RendererLost: Fn() + 'static,
    Invariant: Fn() + 'static,
    Panic: Fn() + 'static,
{
    let AgentOwnedViewCallbacks {
        #[cfg(feature = "native-agentic-work-lifetime-diagnostic")]
        failure_diagnostic,
        redirect: on_redirect,
        credential_denied: on_credential_denied,
        navigation: on_navigation,
        location: on_location,
        renderer_lost: on_renderer_lost,
        invariant: on_invariant_failure,
        panic: on_callback_panic,
    } = callbacks;
    #[cfg(feature = "native-agentic-work-lifetime-diagnostic")]
    let navigation_diagnostic = failure_diagnostic.clone();
    let navigation = AgentNavigationController::default();
    let navigation_policy = navigation.clone();
    let navigation_events = navigation.clone();
    let renderer_events = navigation.clone();
    let work_policy = work_navigation.clone();
    let work_events = work_navigation.clone();
    let work_renderer = work_navigation.clone();
    let navigation_callback = Rc::new(on_navigation);
    let location_callback = Rc::new(on_location);
    let navigation_event_location_callback = location_callback.clone();
    let renderer_lost_callback = Rc::new(on_renderer_lost);
    let invariant_failure_callback = Rc::new(on_invariant_failure);
    let navigation_invariant_failure = invariant_failure_callback.clone();
    let renderer_invariant_failure = invariant_failure_callback.clone();
    let on_callback_panic = Rc::new(on_callback_panic);
    let redirect_invariant_failure = invariant_failure_callback.clone();
    let redirect_callback_panicked = on_callback_panic.clone();
    // A refused redirect ends the document like any refused load, but its
    // owner reads it as a redirect, never as a broken invariant.
    let on_redirect: Rc<dyn Fn(zephium_agentic::ContextNavigationTarget)> = match on_redirect {
        Some(redirect) => Rc::new(move |destination| {
            invoke_owned_unit_callback(
                &|| redirect(destination.clone()),
                redirect_callback_panicked.as_ref(),
            );
        }),
        None => Rc::new(move |_| {
            invoke_owned_unit_callback(
                redirect_invariant_failure.as_ref(),
                redirect_callback_panicked.as_ref(),
            )
        }),
    };
    let policy_redirect = on_redirect.clone();
    let event_redirect = on_redirect;
    let navigation_callback_panicked = on_callback_panic.clone();
    let location_callback_panicked = on_callback_panic.clone();
    let renderer_callback_panicked = on_callback_panic.clone();
    let credential_callback_panicked = on_callback_panic.clone();
    let configuration = new_owned_agent_configuration(profile, storage_class, ephemeral_store)?;
    let semantic = AgentSemanticRuntimeRegistration::install(
        &configuration,
        invariant_failure_callback.clone(),
        on_callback_panic.clone(),
    )
    .map_err(|_| AgentOwnedViewConstructionError::ExtensionIsolation)?;
    let navigation_semantic = semantic.controller().clone();
    let redirect_semantic = semantic.controller().clone();
    let human_semantic_prepare = semantic.human_navigation_preparer();
    let follow_semantic_prepare = semantic.follow_navigation_preparer();
    let renderer_semantic = semantic.controller().clone();
    let builder = WebViewBuilder::new()
        .with_url("about:blank")
        // Ready Work contexts must keep their bounded semantic channel runnable.
        // Preserve background CPU throttling; only the Rust lifecycle may close
        // this owned page. Do not opt Browse tabs out of their ordinary policy.
        .with_background_throttling(wry::BackgroundThrottlingPolicy::Throttle)
        .with_bounds(Rect {
            position: Position::Logical(LogicalPosition::new(0.0, 0.0)),
            size: Size::Logical(LogicalSize::new(
                f64::from(viewport.width()),
                f64::from(viewport.height()),
            )),
        })
        .with_visible(false)
        .with_focused(false)
        .with_devtools(owned_agent_view_inspectable())
        .with_autoplay(false)
        .with_fullscreen_enabled(false)
        .with_picture_in_picture_enabled(false)
        .with_general_autofill_enabled(false)
        .with_credential_challenge_denied_handler(move || {
            if let Some(notify) = on_credential_denied.as_ref() {
                invoke_owned_unit_callback(notify.as_ref(), credential_callback_panicked.as_ref());
            }
        })
        .with_apple_navigation_action_handler(move |target, action| {
            let prepare_human = work_policy.as_ref().is_some_and(|gate| {
                gate.human_load_preparation_needed(action.target_is_main_frame)
                    && !gate.human_fragment_move(&target)
            });
            let allowed = work_policy.as_ref().map_or_else(
                || navigation_policy.allows(&target),
                |gate| gate.allows_apple_action(&target, action),
            );
            // An action's own admitted load replaces the document: the old
            // document's reads end, and the runtime waits for the new one.
            let allowed = allowed
                && (!work_policy.as_ref().is_some_and(|gate| gate.take_hand_on())
                    || follow_semantic_prepare());
            if let Some(destination) = work_policy
                .as_ref()
                .filter(|_| !allowed)
                .and_then(|gate| gate.take_redirect_report())
            {
                redirect_semantic.cancel();
                policy_redirect(destination);
            }
            #[cfg(feature = "agentic-browser-qa")]
            if action.target_is_main_frame != Some(false) {
                super::agentic_liveness_probe::trace(format_args!(
                    "agent_view frame=main requested=true allowed={allowed} human_prepare={prepare_human} cause={:?} get={}",
                    action.navigation_type, action.is_get));
            }
            if allowed && prepare_human && !human_semantic_prepare() {
                #[cfg(feature = "agentic-browser-qa")]
                {
                    super::agentic_liveness_probe::trace(format_args!(
                        "agent_view human_prepare_failed=true"
                    ));
                }
                if let Some(gate) = &work_policy {
                    gate.retire();
                }
                return false;
            }
            #[cfg(feature = "agentic-browser-qa")]
            if url::Url::parse(&target)
                .is_ok_and(|url| url.host_str() == Some("challenges.cloudflare.com"))
            {
                super::agentic_liveness_probe::trace(format_args!(
                    "agent_view frame=challenge requested=true allowed={allowed} main={:?}",
                    action.target_is_main_frame
                ));
            }
            #[cfg(feature = "native-agentic-semantic-probe")]
            if let Some(gate) = &work_policy {
                gate.record_form_probe_navigation(action, allowed);
            }
            allowed
        })
        .with_navigation_event_handler(move |event| {
            #[cfg(feature = "agentic-browser-qa")]
            {
                super::agentic_liveness_probe::trace(format_args!(
                    "agent_view frame=main phase={:?}",
                    event.phase
                ));
            }
            if let Some(gate) = &work_events {
                match gate.observe(event) {
                    Ok((committed, notify)) => {
                        if let Some(destination) = gate.redirected() {
                            event_redirect(destination);
                            navigation_semantic.cancel();
                        } else if gate.owned_refusal() {
                            invoke_owned_unit_callback(
                                navigation_event_location_callback.as_ref(),
                                location_callback_panicked.as_ref(),
                            );
                            navigation_semantic.cancel();
                        } else if gate.failed() {
                            #[cfg(feature = "native-agentic-work-lifetime-diagnostic")]
                            if let Some(report) = &navigation_diagnostic {
                                report(crate::WorkResourceFailureCause::NavigationEventRefused);
                            }
                            invoke_owned_unit_callback(
                                navigation_invariant_failure.as_ref(),
                                navigation_callback_panicked.as_ref(),
                            );
                            navigation_semantic.cancel();
                        }
                        if committed {
                            navigation_semantic.document_committed();
                        }
                        if notify {
                            invoke_owned_unit_callback(
                                navigation_event_location_callback.as_ref(),
                                location_callback_panicked.as_ref(),
                            );
                        }
                    }
                    Err(()) => invoke_owned_unit_callback(
                        navigation_invariant_failure.as_ref(),
                        navigation_callback_panicked.as_ref(),
                    ),
                }
                return;
            }
            match navigation_events.observe(event) {
                Ok(observation) => {
                    if observation.did_commit_document() {
                        navigation_semantic.document_committed();
                    }
                    if observation.should_check_location() {
                        invoke_owned_unit_callback(
                            navigation_event_location_callback.as_ref(),
                            location_callback_panicked.as_ref(),
                        );
                    }
                    if let Some(terminal) = observation.into_terminal() {
                        invoke_owned_navigation_callback(
                            navigation_callback.as_ref(),
                            navigation_callback_panicked.as_ref(),
                            terminal,
                        );
                    }
                }
                Err(()) => invoke_owned_unit_callback(
                    navigation_invariant_failure.as_ref(),
                    navigation_callback_panicked.as_ref(),
                ),
            }
        })
        .with_on_web_content_process_terminate_handler(move || {
            if let Some(gate) = &work_renderer {
                gate.refuse();
                renderer_semantic.renderer_lost();
                invoke_owned_unit_callback(
                    renderer_lost_callback.as_ref(),
                    renderer_callback_panicked.as_ref(),
                );
                return;
            }
            match renderer_events.claim_renderer_loss() {
                Ok(true) => {
                    renderer_semantic.renderer_lost();
                    invoke_owned_unit_callback(
                        renderer_lost_callback.as_ref(),
                        renderer_callback_panicked.as_ref(),
                    );
                }
                Ok(false) => {}
                Err(()) => invoke_owned_unit_callback(
                    renderer_invariant_failure.as_ref(),
                    renderer_callback_panicked.as_ref(),
                ),
            }
        })
        .with_permission_handler(|_| wry::PermissionResponse::Deny)
        .with_download_policy(DownloadPolicy::DenyWithoutMetadata)
        .with_page_close_policy(PageClosePolicy::Ignore)
        .with_allow_link_preview(false)
        .with_user_agent(super::safari_user_agent())
        .with_webview_configuration(configuration);

    let builder = if storage_class == ContextProfileStorageClass::Ephemeral {
        builder.with_incognito(true)
    } else {
        builder
    };

    let view = builder
        .build_as_child(parent)
        .map_err(|_| AgentOwnedViewConstructionError::Native)?;
    semantic
        .bind_view(&super::native_webview(&view))
        .map_err(|_| AgentOwnedViewConstructionError::Native)?;
    harden_owned_agent_view(&view)?;
    attest_owned_agent_view(
        &view,
        &semantic,
        viewport,
        profile,
        storage_class,
        ephemeral_store,
    )?;
    let location_events = navigation.clone();
    let work_location = work_navigation.clone();
    let location_invariant = invariant_failure_callback.clone();
    let location_panic = on_callback_panic.clone();
    let work_location_semantic = semantic.controller().clone();
    let navigation_observer = super::install_navigation_observer(&view, move |observation| {
        match work_location.as_ref().map_or_else(
            || location_events.request_location_check(),
            |gate| match observation {
                super::navigation::NavigationObservation::Url(current) => {
                    gate.location_changed(current.as_deref())
                }
                super::navigation::NavigationObservation::HistoryAvailability => {
                    Ok(gate.history_availability_changed())
                }
            },
        ) {
            Ok(true) => {
                if work_location.as_ref().is_some_and(|gate| gate.failed()) {
                    // Close semantic admission and install the exact in-flight
                    // lifetime witness before any callback can progress the host.
                    work_location_semantic.revoke_document_authority();
                    #[cfg(feature = "native-agentic-work-lifetime-diagnostic")]
                    if let Some(report) = &failure_diagnostic {
                        report(
                            work_location
                                .as_ref()
                                .and_then(|gate| gate.url_observation_failure())
                                .map_or(
                                    crate::WorkResourceFailureCause::SemanticNativeInvariant,
                                    crate::WorkResourceFailureCause::UrlObservationRefused,
                                ),
                        );
                    }
                    invoke_owned_unit_callback(
                        location_invariant.as_ref(),
                        location_panic.as_ref(),
                    );
                }
                invoke_owned_unit_callback(location_callback.as_ref(), location_panic.as_ref())
            }
            Ok(false) => {}
            Err(()) => {
                invoke_owned_unit_callback(location_invariant.as_ref(), location_panic.as_ref())
            }
        }
    })
    .map_err(|_| AgentOwnedViewConstructionError::Native)?;
    Ok(AgentOwnedView {
        navigation,
        work_navigation,
        semantic: Some(semantic),
        history: super::agent_history::AgentHistoryLedger::default(),
        next_document_runtime: 1,
        viewport,
        _navigation_observer: navigation_observer,
        view,
    })
}

const fn owned_agent_view_inspectable() -> bool {
    cfg!(all(debug_assertions, feature = "agentic-browser-qa"))
}

fn harden_owned_agent_view(view: &WebView) -> Result<(), AgentOwnedViewConstructionError> {
    use objc2_app_kit::{NSAutoresizingMaskOptions as Mask, NSView};
    use objc2_foundation::MainThreadMarker;

    let _mtm = MainThreadMarker::new().ok_or(AgentOwnedViewConstructionError::Native)?;
    let page = super::native_webview(view);
    // SAFETY: the marker above proves main-thread access and `page` is the
    // retained WKWebView owned by the live Wry handle for this call.
    unsafe { page.setInspectable(owned_agent_view_inspectable()) };
    let native_view: &NSView = &page;
    native_view.setTranslatesAutoresizingMaskIntoConstraints(true);
    native_view.setAutoresizingMask(Mask::ViewNotSizable);
    native_view.setHidden(true);
    Ok(())
}

pub(crate) fn attest_owned_agent_view(
    view: &WebView,
    semantic: &AgentSemanticRuntimeRegistration,
    viewport: ContextOwnedViewport,
    profile: ProfileId,
    storage_class: ContextProfileStorageClass,
    ephemeral_store: Option<&WebsiteDataStore>,
) -> Result<(), AgentOwnedViewConstructionError> {
    use objc2::rc::Retained;
    use objc2_app_kit::{NSAutoresizingMaskOptions as Mask, NSView};
    use objc2_foundation::MainThreadMarker;

    let _mtm = MainThreadMarker::new().ok_or(AgentOwnedViewConstructionError::Native)?;
    let page = super::native_webview(view);
    // SAFETY: the marker above proves main-thread access; `page` and every
    // object returned from it are retained by objc2 for these bounded reads.
    let (configuration, extension_controller, scheduling) = unsafe {
        let configuration = page.configuration();
        let extension_controller = configuration.webExtensionController();
        let scheduling = configuration.preferences().inactiveSchedulingPolicy();
        (configuration, extension_controller, scheduling)
    };
    // Available since macOS 14.0, below Zephium's deployment floor. Ready
    // owned pages must remain runnable without disabling background CPU limits.
    if scheduling != objc2_web_kit::WKInactiveSchedulingPolicy::Throttle {
        return Err(AgentOwnedViewConstructionError::Native);
    }
    if extension_controller.is_some() {
        return Err(AgentOwnedViewConstructionError::ExtensionIsolation);
    }
    if semantic.attest_configuration(&configuration).is_err() {
        return Err(AgentOwnedViewConstructionError::ExtensionIsolation);
    }

    // SAFETY: `configuration` is the retained configuration of the live page;
    // its retained data store remains valid for these identity reads.
    let (actual_store, persistent, identifier) = unsafe {
        let actual_store = configuration.websiteDataStore();
        let persistent = actual_store.isPersistent();
        let identifier = actual_store.identifier().map(|value| value.as_bytes());
        (actual_store, persistent, identifier)
    };
    let storage_valid = match storage_class {
        ContextProfileStorageClass::Durable => match ephemeral_store {
            None => persistent && identifier == Some(profile.bytes()),
            Some(expected) => {
                work_store_matches(profile, &actual_store)
                    && Retained::as_ptr(&actual_store) == Retained::as_ptr(expected)
            }
        },
        ContextProfileStorageClass::Ephemeral => {
            let Some(expected) = ephemeral_store else {
                return Err(AgentOwnedViewConstructionError::Storage);
            };
            !persistent
                && identifier.is_none()
                && Retained::as_ptr(&actual_store) == Retained::as_ptr(expected)
        }
    };
    if !storage_valid {
        return Err(AgentOwnedViewConstructionError::Storage);
    }

    let native_view: &NSView = &page;
    let frame = native_view.frame();
    // SAFETY: main-thread access and the live retained page were proven above.
    if unsafe { page.isInspectable() } != owned_agent_view_inspectable()
        || native_view.autoresizingMask() != Mask::ViewNotSizable
        || frame.size.width != f64::from(viewport.width())
        || frame.size.height != f64::from(viewport.height())
        || !native_view.isHidden()
    {
        return Err(AgentOwnedViewConstructionError::Native);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicBool;
    use std::sync::Arc;

    use zephium_agentic::{
        ContextCapabilities, ContextId, ContextIdentity, ContextOperationId, ContextRegistry,
        ContextRunId,
    };
    use zephium_core::ids::ProfileId;

    #[cfg(not(feature = "agentic-browser-qa"))]
    #[test]
    fn ordinary_agent_views_never_enable_inspection() {
        assert!(!super::owned_agent_view_inspectable());
    }

    #[cfg(feature = "agentic-browser-qa")]
    #[test]
    fn qa_agent_views_enable_inspection_only_in_debug_builds() {
        assert_eq!(
            super::owned_agent_view_inspectable(),
            cfg!(debug_assertions)
        );
    }

    #[test]
    fn refused_url_closes_semantic_authority_before_notifying_any_observer() {
        let source = include_str!("agent_context.rs")
            .split("#[cfg(test)]")
            .next()
            .unwrap();
        let branch = source
            .split("if work_location.as_ref().is_some_and(|gate| gate.failed()) {")
            .nth(1)
            .unwrap();
        let close = branch
            .find("work_location_semantic.revoke_document_authority()")
            .unwrap();
        assert!(close < branch.find("report(").unwrap());
        assert!(close < branch.find("invoke_owned_unit_callback(").unwrap());
    }

    fn navigation_operation() -> zephium_agentic::ContextOperationJoin {
        let identity = ContextIdentity::new(
            ContextId::generate(),
            ContextRunId::generate(),
            ProfileId::from(4),
            zephium_agentic::ContextKind::Owned,
        );
        let capabilities = ContextCapabilities::try_new(
            zephium_agentic::ContextKind::Owned,
            &[zephium_agentic::ContextCapability::Navigate],
        )
        .expect("capabilities");
        let mut registry = ContextRegistry::new();
        registry.reserve(identity, capabilities).expect("reserve");
        let construction = registry
            .begin_context(
                identity.id(),
                ContextOperationId::new(1).expect("operation"),
            )
            .expect("construction");
        registry
            .settle_construction(
                identity.id(),
                construction,
                zephium_agentic::ContextSettlement::Applied,
            )
            .expect("settle construction");
        registry
            .begin_navigation(
                identity.id(),
                ContextOperationId::new(2).expect("operation"),
            )
            .expect("navigation")
    }

    fn recovery_operation() -> zephium_agentic::ContextOperationJoin {
        let identity = ContextIdentity::new(
            ContextId::generate(),
            ContextRunId::generate(),
            ProfileId::from(5),
            zephium_agentic::ContextKind::Owned,
        );
        let capabilities = ContextCapabilities::try_new(
            zephium_agentic::ContextKind::Owned,
            &[zephium_agentic::ContextCapability::Recover],
        )
        .expect("capabilities");
        let mut registry = ContextRegistry::new();
        registry.reserve(identity, capabilities).expect("reserve");
        let construction = registry
            .begin_context(
                identity.id(),
                ContextOperationId::new(1).expect("operation"),
            )
            .expect("construction");
        registry
            .settle_construction(
                identity.id(),
                construction,
                zephium_agentic::ContextSettlement::Applied,
            )
            .expect("settle construction");
        let prior = registry.join(identity.id()).expect("join");
        registry
            .renderer_lost(identity.id(), prior)
            .expect("renderer loss");
        registry
            .begin_recovery(
                identity.id(),
                ContextOperationId::new(2).expect("operation"),
            )
            .expect("recovery")
    }

    #[test]
    fn construction_source_has_no_model_or_page_program_surface() {
        let source = include_str!("agent_context.rs");
        for forbidden in [
            concat!("with_ipc_", "handler"),
            concat!("with_initialization_", "script"),
            concat!("evaluate_", "script"),
            concat!("with_new_window_req_", "handler"),
            concat!("with_web_extension_", "controller"),
        ] {
            assert!(!source.contains(forbidden));
        }
        assert!(source.contains("with_apple_navigation_action_handler(move |target, action|"));
        assert!(source.contains("state.bootstrap_available = false"));
        assert!(source.contains("controller.userScripts()"));
    }

    #[test]
    fn navigation_gate_consumes_bootstrap_and_denies_every_unarmed_page_target() {
        let gate = super::AgentNavigationController::default();
        assert!(gate.allows("about:blank"));
        assert!(!gate.allows("about:blank"));
        assert!(!gate.allows("https://example.test/"));
    }

    #[test]
    fn construction_bootstrap_commit_releases_semantic_readiness_without_a_shell_terminal() {
        let gate = super::AgentNavigationController::default();
        assert!(gate.allows("about:blank"));
        let started = gate
            .observe(wry::NavigationEvent {
                id: wry::NavigationId::from_raw(70),
                phase: wry::NavigationEventPhase::Started,
                url: "about:blank".to_owned(),
            })
            .expect("started");
        assert!(!started.did_commit_document());
        assert!(started.is_none());

        let committed = gate
            .observe(wry::NavigationEvent {
                id: wry::NavigationId::from_raw(70),
                phase: wry::NavigationEventPhase::Committed,
                url: "about:blank".to_owned(),
            })
            .expect("committed");
        assert!(committed.did_commit_document());
        assert!(committed.is_none());
        assert!(gate
            .observe(wry::NavigationEvent {
                id: wry::NavigationId::from_raw(70),
                phase: wry::NavigationEventPhase::Committed,
                url: "about:blank".to_owned(),
            })
            .expect("duplicate")
            .is_none());
    }

    #[test]
    fn navigation_gate_is_exact_and_has_one_commit_or_failure_winner() {
        let gate = super::AgentNavigationController::default();
        let operation = navigation_operation();
        let target = zephium_agentic::ContextNavigationTarget::parse("https://example.test/path")
            .expect("target");
        let terminal = Arc::new(AtomicBool::new(false));
        gate.arm(operation, target.clone(), terminal).expect("arm");
        assert!(gate.allows("https://example.test/path"));
        assert!(!gate.allows("https://example.test/redirect"));
        assert!(!gate.allows("about:blank"));

        // A delayed construction callback cannot settle the newly armed
        // shell operation because it lacks the matching native start.
        assert!(gate
            .observe(wry::NavigationEvent {
                id: wry::NavigationId::from_raw(99),
                phase: wry::NavigationEventPhase::Committed,
                url: "about:blank".to_owned(),
            })
            .expect("state")
            .is_none());
        assert!(gate
            .observe(wry::NavigationEvent {
                id: wry::NavigationId::from_raw(1),
                phase: wry::NavigationEventPhase::Started,
                url: "https://example.test/path".to_owned(),
            })
            .expect("state")
            .is_none());

        let committed = gate
            .observe(wry::NavigationEvent {
                id: wry::NavigationId::from_raw(1),
                phase: wry::NavigationEventPhase::Committed,
                url: "https://example.test/path".to_owned(),
            })
            .expect("state")
            .expect("commit");
        assert_eq!(committed.operation(), operation);
        assert_eq!(
            committed.into_outcome(),
            Ok(super::AgentNavigationCommit::Web(target))
        );
        assert!(!gate.allows("https://example.test/path"));
        assert!(gate
            .observe(wry::NavigationEvent {
                id: wry::NavigationId::from_raw(1),
                phase: wry::NavigationEventPhase::Failed,
                url: "https://example.test/path".to_owned(),
            })
            .expect("state")
            .is_none());
    }

    #[test]
    fn navigation_timeout_claim_prevents_a_late_native_commit() {
        let gate = super::AgentNavigationController::default();
        let operation = navigation_operation();
        let target = zephium_agentic::ContextNavigationTarget::parse("https://example.test/late")
            .expect("target");
        let terminal = Arc::new(AtomicBool::new(false));
        gate.arm(operation, target, terminal.clone()).expect("arm");
        assert!(gate
            .observe(wry::NavigationEvent {
                id: wry::NavigationId::from_raw(2),
                phase: wry::NavigationEventPhase::Started,
                url: "https://example.test/late".to_owned(),
            })
            .expect("state")
            .is_none());
        assert!(!terminal.swap(true, std::sync::atomic::Ordering::AcqRel));
        assert!(gate
            .observe(wry::NavigationEvent {
                id: wry::NavigationId::from_raw(2),
                phase: wry::NavigationEventPhase::Committed,
                url: "https://example.test/late".to_owned(),
            })
            .expect("state")
            .is_none());
        assert!(gate.disarm(operation));
    }

    #[test]
    fn renderer_loss_is_one_shot_and_revokes_an_armed_navigation() {
        let gate = super::AgentNavigationController::default();
        let operation = navigation_operation();
        let target =
            zephium_agentic::ContextNavigationTarget::parse("https://example.test/renderer-loss")
                .expect("target");
        let terminal = Arc::new(AtomicBool::new(false));
        gate.arm(operation, target, terminal.clone()).expect("arm");
        assert!(gate.allows("https://example.test/renderer-loss"));

        assert_eq!(gate.claim_renderer_loss(), Ok(true));
        assert!(terminal.load(std::sync::atomic::Ordering::Acquire));
        assert_eq!(gate.claim_renderer_loss(), Ok(false));
        assert!(!gate.allows("https://example.test/renderer-loss"));
        assert!(gate
            .observe(wry::NavigationEvent {
                id: wry::NavigationId::from_raw(3),
                phase: wry::NavigationEventPhase::Committed,
                url: "https://example.test/renderer-loss".to_owned(),
            })
            .expect("state")
            .is_none());
        assert!(gate.matches_for_audit(Some(operation), true));
        assert!(gate.disarm(operation));
        assert!(gate.matches_for_audit(None, true));
    }

    #[test]
    fn recovery_rearms_only_from_loss_and_restores_repeatable_loss_detection() {
        let gate = super::AgentNavigationController::default();
        let operation = recovery_operation();
        let terminal = Arc::new(AtomicBool::new(false));
        assert!(gate
            .arm_recovery(operation, None, terminal.clone())
            .is_err());
        assert_eq!(gate.claim_renderer_loss(), Ok(true));
        gate.arm_recovery(operation, None, terminal)
            .expect("recovery arm");
        assert!(gate.allows("about:blank"));
        assert!(!gate.allows("https://example.test/"));
        assert!(gate
            .observe(wry::NavigationEvent {
                id: wry::NavigationId::from_raw(4),
                phase: wry::NavigationEventPhase::Started,
                url: "about:blank".to_owned(),
            })
            .expect("state")
            .is_none());
        let committed = gate
            .observe(wry::NavigationEvent {
                id: wry::NavigationId::from_raw(4),
                phase: wry::NavigationEventPhase::Committed,
                url: "about:blank".to_owned(),
            })
            .expect("state")
            .expect("commit");
        assert_eq!(committed.operation(), operation);
        assert_eq!(
            committed.into_outcome(),
            Ok(super::AgentNavigationCommit::Bootstrap)
        );
        assert!(gate.settle_recovery(operation, true));
        assert!(gate.matches_for_audit(None, false));
        assert_eq!(gate.claim_renderer_loss(), Ok(true));
    }

    #[test]
    fn loss_after_recovery_commit_survives_terminal_settlement() {
        let gate = super::AgentNavigationController::default();
        let operation = recovery_operation();
        assert_eq!(gate.claim_renderer_loss(), Ok(true));
        gate.arm_recovery(operation, None, Arc::new(AtomicBool::new(false)))
            .expect("recovery arm");
        assert!(gate
            .observe(wry::NavigationEvent {
                id: wry::NavigationId::from_raw(5),
                phase: wry::NavigationEventPhase::Started,
                url: "about:blank".to_owned(),
            })
            .expect("state")
            .is_none());
        assert!(gate
            .observe(wry::NavigationEvent {
                id: wry::NavigationId::from_raw(5),
                phase: wry::NavigationEventPhase::Committed,
                url: "about:blank".to_owned(),
            })
            .expect("state")
            .is_some());
        assert_eq!(gate.claim_renderer_loss(), Ok(true));
        assert!(gate.settle_recovery(operation, true));
        assert!(gate.matches_for_audit(None, true));
    }

    #[test]
    fn failed_web_recovery_restores_loss_and_never_widens_its_target() {
        let gate = super::AgentNavigationController::default();
        let operation = recovery_operation();
        let target =
            zephium_agentic::ContextNavigationTarget::parse("https://example.test/recover")
                .expect("target");
        assert_eq!(gate.claim_renderer_loss(), Ok(true));
        gate.arm_recovery(operation, Some(target), Arc::new(AtomicBool::new(false)))
            .expect("recovery arm");
        assert!(gate.allows("https://example.test/recover"));
        assert!(!gate.allows("https://example.test/redirect"));
        assert!(gate
            .observe(wry::NavigationEvent {
                id: wry::NavigationId::from_raw(6),
                phase: wry::NavigationEventPhase::Started,
                url: "https://example.test/recover".to_owned(),
            })
            .expect("state")
            .is_none());
        let failed = gate
            .observe(wry::NavigationEvent {
                id: wry::NavigationId::from_raw(6),
                phase: wry::NavigationEventPhase::Failed,
                url: "https://example.test/recover".to_owned(),
            })
            .expect("state")
            .expect("failure");
        assert_eq!(
            failed.into_outcome(),
            Err(zephium_agentic::ContextPortFailure::NativeRefused)
        );
        assert!(gate.settle_recovery(operation, false));
        assert!(gate.matches_for_audit(None, true));
        assert!(!gate.allows("https://example.test/recover"));
    }
}
