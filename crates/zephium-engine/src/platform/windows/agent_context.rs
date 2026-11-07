#![deny(unsafe_op_in_unsafe_fn)]
#![deny(clippy::undocumented_unsafe_blocks)]
#![deny(clippy::dbg_macro, clippy::print_stderr, clippy::print_stdout)]
#![cfg_attr(
    not(test),
    deny(clippy::panic, clippy::unreachable, clippy::unwrap_used)
)]

//! Hidden WebView2 construction for owned agent contexts.
//!
//! Extension-enabled human environments use a deterministic automation
//! subprofile. The pre-initialization gate attests its exact binding and
//! extension-free inventory before the host arms any navigation.

use std::cell::{Cell, RefCell};
use std::path::Path;
use std::rc::{Rc, Weak};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Instant;

use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use webview2_com::Microsoft::Web::WebView2::Win32::{
    ICoreWebView2, ICoreWebView2CookieManager, ICoreWebView2Environment, ICoreWebView2Profile,
    ICoreWebView2Profile2, ICoreWebView2Profile7, ICoreWebView2_13, ICoreWebView2_2,
    ICoreWebView2_3,
};
use webview2_com::TrySuspendCompletedHandler;
use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::GetFocus;
use windows::Win32::UI::WindowsAndMessaging::{
    GetClientRect, GetParent, IsChild, IsWindow, IsWindowVisible,
};
use windows_core::{Interface as _, PWSTR};
use wry::dpi::{LogicalPosition, LogicalSize, Position, Size};
use wry::{
    DownloadPolicy, PageClosePolicy, Rect, WebView, WebViewBuilder, WebViewBuilderExtWindows as _,
    WebViewExtWindows as _,
};
use zephium_agentic::{ContextConstructionProof, ContextOwnedViewport, ContextProfileStorageClass};
use zephium_agentic::{
    SemanticRuntimeInvocation, SemanticRuntimePortFailure, SemanticScreenshotNativeCapture,
    SemanticScreenshotNativeFailure, SemanticScreenshotNativeRequest, SemanticSnapshot,
};
use zephium_core::ids::ProfileId;

use crate::platform::agent_navigation::AgentNavigationController;
pub(crate) use crate::platform::agent_navigation::{
    AgentNavigationCommit, AgentNavigationTerminal,
};

const PROFILE_NAME_UTF16_LIMIT: usize = 64;
const PROFILE_NAME_UTF8_LIMIT: usize = 64;

/// Closed construction failure mapped by the native host boundary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum AgentOwnedViewConstructionError {
    /// The selected environment, profile name, or private-mode bit diverged.
    Storage,
    /// The controller profile or the isolated semantic world could not be proven.
    ExtensionIsolation,
    /// Wry/WebView2 refused construction, hardening, callbacks, or hidden state.
    Native,
}

/// Exact profile authority selected before one controller is constructed.
#[derive(Clone)]
pub(crate) enum AgentOwnedProfile {
    Selected,
    Automation { name: String },
}

impl AgentOwnedProfile {
    pub(crate) fn automation(profile: ProfileId) -> Self {
        Self::Automation {
            name: format!("agent-{profile}"),
        }
    }

    pub(crate) fn work(identity: [u8; 16]) -> Self {
        Self::Automation {
            name: format!("work-{:032x}", u128::from_be_bytes(identity)),
        }
    }
    pub(crate) fn work_site(target: &zephium_agentic::ContextNavigationTarget) -> Result<Self, ()> {
        use base64::Engine as _;
        use sha2::{Digest, Sha256};
        let site = zephium_agentic::registrable_site(target).ok_or(())?;
        let url = target.as_url();
        let key = format!(
            "{}:{}:{}",
            url.scheme(),
            url.port_or_known_default().ok_or(())?,
            site
        );
        Ok(Self::Automation {
            name: format!(
                "work-site-{}",
                base64::engine::general_purpose::URL_SAFE_NO_PAD
                    .encode(Sha256::digest(key.as_bytes()))
            ),
        })
    }
    pub(crate) const fn proof(&self) -> ContextConstructionProof {
        match self {
            Self::Selected => ContextConstructionProof::WindowsOwnedSelectedProfileEmptyInventory,
            Self::Automation { .. } => {
                ContextConstructionProof::WindowsOwnedAutomationSubprofileEmptyInventory
            }
        }
    }
}

/// Typed callback cohort retained by one WebView2 controller generation.
pub(crate) struct AgentOwnedViewCallbacks<
    Navigation,
    Location,
    RendererLost,
    BrowserLost,
    Invariant,
    Panic,
> {
    navigation: Navigation,
    location: Location,
    renderer_lost: RendererLost,
    browser_lost: BrowserLost,
    invariant: Invariant,
    panic: Panic,
    redirect: Option<Rc<dyn Fn(zephium_agentic::ContextNavigationTarget)>>,
    credential_denied: Option<Rc<dyn Fn()>>,
}

impl<Navigation, Location, RendererLost, BrowserLost, Invariant, Panic>
    AgentOwnedViewCallbacks<Navigation, Location, RendererLost, BrowserLost, Invariant, Panic>
{
    pub(crate) const fn new(
        navigation: Navigation,
        location: Location,
        renderer_lost: RendererLost,
        browser_lost: BrowserLost,
        invariant: Invariant,
        panic: Panic,
    ) -> Self {
        Self {
            navigation,
            location,
            renderer_lost,
            browser_lost,
            invariant,
            panic,
            redirect: None,
            credential_denied: None,
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
}

/// Admission shared only by pages in one exact retained Work site store.
pub(crate) struct WorkStoreSeed {
    metadata: Rc<super::work_seed_metadata::WorkSeedMetadata>,
    site_name: String,
    busy: Cell<bool>,
    active_pages: Cell<usize>,
    origins: RefCell<std::collections::HashSet<String>>,
    /// A sign-in handoff ran on a page of this store and its cookies have
    /// not been added to the person's profile yet.
    return_pending: Cell<bool>,
    return_refusals: Cell<u8>,
    waiters: RefCell<Vec<Weak<dyn Fn()>>>,
}
impl WorkStoreSeed {
    pub(crate) fn new(
        metadata: Rc<super::work_seed_metadata::WorkSeedMetadata>,
        site_name: String,
    ) -> Self {
        Self {
            metadata,
            site_name,
            busy: Cell::new(false),
            active_pages: Cell::new(0),
            origins: RefCell::new(Default::default()),
            return_pending: Cell::new(false),
            return_refusals: Cell::new(0),
            waiters: RefCell::new(Vec::new()),
        }
    }
    pub(crate) fn quiescent(&self) -> bool {
        !self.busy.get() && self.active_pages.get() == 0
    }
    /// The host keeps a store with a pending return so the next page of the
    /// site can still carry that sign-in to the person's profile.
    pub(crate) fn return_pending(&self) -> bool {
        self.return_pending.get()
    }
    /// Records how a return ended and whether it is still pending.
    fn settle_return(&self, result: crate::work_site_session::SyncResult) -> bool {
        use crate::work_site_session::{after_return, AfterReturn, SyncResult};
        let refusals = if result == SyncResult::Refused {
            self.return_refusals.get().saturating_add(1)
        } else {
            0
        };
        self.return_refusals.set(refusals);
        let pending = after_return(result, refusals) != AfterReturn::Refresh;
        self.return_pending.set(pending);
        if !pending {
            self.return_refusals.set(0);
        }
        pending
    }
    fn notify(&self) {
        let callbacks: Vec<_> = self
            .waiters
            .borrow()
            .iter()
            .filter_map(Weak::upgrade)
            .collect();
        self.waiters
            .borrow_mut()
            .retain(|waiter| waiter.strong_count() != 0);
        for callback in callbacks {
            callback();
        }
    }
}
struct WorkSeedJob {
    /// The person's own profile cookies for this site, read and written.
    profile: Option<ICoreWebView2CookieManager>,
    scope: zephium_agentic::ContextCookieScope,
    origin: String,
    deadline: Instant,
    completion: Box<dyn FnOnce(bool, bool)>,
    panic: Rc<dyn Fn()>,
}

/// One admission's view of its store, shared by the native callbacks that
/// carry it through return, refresh and opening.
struct WorkStorageAdmission {
    store: Rc<WorkStoreSeed>,
    ready: Rc<Cell<bool>>,
    transfer: Rc<RefCell<Option<super::cookie_transfer::WindowsAgentCookieTransfer>>>,
    work_store: ICoreWebView2CookieManager,
}

/// Exact native page and its one-at-a-time navigation policy.
pub(crate) struct AgentOwnedView {
    navigation: AgentNavigationController,
    work_navigation: Option<crate::platform::work_document_navigation::WorkDocumentNavigation>,
    semantic: Option<super::semantic_runtime::AgentSemanticRuntimeRegistration>,
    profile: AgentOwnedProfile,
    storage_class: ContextProfileStorageClass,
    expected_user_data_folder: std::path::PathBuf,
    expected_parent: HWND,
    viewport: ContextOwnedViewport,
    work_native_profile: Option<ProfileId>,
    work_storage_ready: Rc<Cell<bool>>,
    work_cookie_transfer: Rc<RefCell<Option<super::cookie_transfer::WindowsAgentCookieTransfer>>>,
    work_return_transfer: RefCell<Option<super::cookie_transfer::WindowsAgentCookieTransfer>>,
    work_store: Option<Rc<WorkStoreSeed>>,
    work_seed_job: Rc<RefCell<Option<WorkSeedJob>>>,
    work_seed_query: Rc<Cell<bool>>,
    work_seed_cancelled: Rc<Cell<bool>>,
    work_store_page_active: bool,
    work_parked: Cell<bool>,
    work_leased: Rc<Cell<bool>>,
    work_human_active: Rc<Cell<bool>>,
    work_idle_completion: WorkIdleCompletion,
    work_suspend_pending: Rc<Cell<bool>>,
    work_activity_notify: Rc<dyn Fn()>,
    work_activity_panic: Rc<dyn Fn()>,
    work_network: Option<super::work_network::WorkNetworkPolicy>,
    history: super::agent_history::AgentHistoryLedger,
    _crash_observer: super::CrashObserver,
    _navigation_observer: super::InstalledNavigationObserver,
    _security_policy: super::SecurityPolicy,
    view: WebView,
}

impl AgentOwnedView {
    pub(crate) const fn view(&self) -> &WebView {
        &self.view
    }

    pub(crate) const fn navigation(&self) -> &AgentNavigationController {
        &self.navigation
    }

    pub(crate) fn set_work_native_profile(&mut self, profile: ProfileId) {
        self.work_native_profile = Some(profile);
    }
    pub(crate) fn work_native_profile(&self) -> Option<ProfileId> {
        self.work_native_profile
    }
    /// Clears every kind of browsing data of this view's WebView2 profile and
    /// replies whether WebView2 reported success. False if it could not start.
    pub(crate) fn clear_profile_browsing_data(&self, reply: std::sync::mpsc::Sender<bool>) -> bool {
        let Ok(profile) = super::profile_for_erasure(&self.view) else {
            return false;
        };
        let handler =
            webview2_com::ClearBrowsingDataCompletedHandler::create(Box::new(move |result| {
                let _ = reply.send(result.is_ok());
                Ok(())
            }));
        // SAFETY: the STA-owned live profile and handler are reference counted.
        unsafe { profile.ClearBrowsingDataAll(&handler) }.is_ok()
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "Seed admission keeps exact store, scope, deadline and terminal obligations explicit."
    )]
    pub(crate) fn seed_work_session(
        &mut self,
        profile: Option<ICoreWebView2CookieManager>,
        scope: zephium_agentic::ContextCookieScope,
        origin: String,
        store: Rc<WorkStoreSeed>,
        deadline: Instant,
        completion: impl FnOnce(bool, bool) + 'static,
        callback_panicked: impl Fn() + 'static,
    ) {
        store
            .waiters
            .borrow_mut()
            .push(Rc::downgrade(&self.work_activity_notify));
        self.work_store = Some(store);
        self.work_storage_ready.set(false);
        *self.work_seed_job.borrow_mut() = Some(WorkSeedJob {
            profile,
            scope,
            origin,
            deadline,
            completion: Box::new(completion),
            panic: Rc::new(callback_panicked),
        });
        self.progress_work_storage();
    }

    pub(crate) fn progress_work_storage(&mut self) {
        let Some(store) = self.work_store.as_ref().cloned() else {
            return;
        };
        if store.busy.get() || self.work_seed_job.borrow().is_none() {
            return;
        }
        if self.work_seed_cancelled.get() {
            self.work_seed_job.borrow_mut().take();
            return;
        }
        let Some(job) = self.work_seed_job.borrow_mut().take() else {
            return;
        };
        if Instant::now() >= job.deadline {
            (job.completion)(false, false);
            return;
        }
        let initialized = match store.metadata.is_initialized(&store.site_name, &job.origin) {
            Ok(stamped) => stamped || store.origins.borrow().contains(&job.origin),
            Err(()) => {
                (job.completion)(false, false);
                return;
            }
        };
        let Ok((work_store, _)) = self.cookie_destination(&self.view.environment()) else {
            (job.completion)(false, false);
            return;
        };
        store.busy.set(true);
        self.work_seed_query.set(true);
        let query = self.work_seed_query.clone();
        let cancelled = self.work_seed_cancelled.clone();
        let origin = job.origin.clone();
        let callback_panic = job.panic.clone();
        let pending = Rc::new(RefCell::new(Some(job)));
        let deferred_job = self.work_seed_job.clone();
        let callback_pending = pending.clone();
        let admission = Rc::new(WorkStorageAdmission {
            store: store.clone(),
            ready: self.work_storage_ready.clone(),
            transfer: self.work_cookie_transfer.clone(),
            work_store: work_store.clone(),
        });
        let panic_admission = admission.clone();
        let handler =
            webview2_com::GetCookiesCompletedHandler::create(Box::new(move |result, cookies| {
                let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    query.set(false);
                    let Some(job) = callback_pending.borrow_mut().take() else {
                        return;
                    };
                    let mut count = 0;
                    let observed = result.is_ok()
                        && cookies.is_some_and(|cookies| {
                            // SAFETY: WebView2 supplies a live list on this STA; initialized output remains writable.
                            (unsafe { cookies.Count(&mut count).is_ok() })
                                && (0..=8192).contains(&count)
                        });
                    if !observed || cancelled.get() {
                        admission.store.busy.set(false);
                        (job.completion)(false, false);
                        admission.store.notify();
                        return;
                    }
                    use crate::work_site_session::{
                        admission as decide, Admission, AdmissionFacts,
                    };
                    let step = decide(AdmissionFacts {
                        pages_open: admission.store.active_pages.get() != 0,
                        initialized,
                        store_has_cookies: count > 0,
                        profile_available: job.profile.is_some(),
                        return_pending: admission.store.return_pending.get(),
                    });
                    match step {
                        Admission::Wait => {
                            admission.store.busy.set(false);
                            *deferred_job.borrow_mut() = Some(job);
                        }
                        Admission::Open { stamp } => open_work_store(&admission, job, stamp),
                        Admission::Refresh => refresh_work_store(&admission, job),
                        Admission::ReturnThenRefresh => return_then_refresh(&admission, job),
                    }
                }));
                if outcome.is_err() {
                    query.set(false);
                    panic_admission.ready.set(false);
                    // A handed-off transfer keeps the store closed until its
                    // own native terminal; a pre-write panic cannot mint debt.
                    if panic_admission
                        .transfer
                        .try_borrow()
                        .is_ok_and(|attempt| attempt.as_ref().is_none_or(|t| t.is_terminal()))
                    {
                        panic_admission.store.busy.set(false);
                    }
                    invoke_unit_callback(callback_panic.as_ref(), callback_panic.as_ref());
                }
                Ok(())
            }));
        let value = windows_core::HSTRING::from(origin);
        // SAFETY: attested profile manager and retained native callback on its owning STA; URL is valid through call.
        if unsafe { work_store.GetCookies(windows_core::PCWSTR(value.as_ptr()), &handler) }.is_err()
        {
            self.work_seed_query.set(false);
            store.busy.set(false);
            if let Some(job) = pending.borrow_mut().take() {
                (job.completion)(false, false);
            }
            store.notify();
        }
    }

    /// Marks that a sign-in handoff runs on this page, so its store's cookies
    /// go to the person's profile when it resolves or on the site's next page.
    pub(crate) fn note_work_sign_in(&self) {
        if let Some(store) = &self.work_store {
            store.return_pending.set(true);
        }
    }

    pub(crate) fn work_session_return_pending(&self) -> bool {
        self.work_store
            .as_ref()
            .is_some_and(|store| store.return_pending.get())
    }

    /// Adds this page's site cookies to the person's profile after a sign-in
    /// handoff. False when it could not start; the return then stays pending
    /// for the site's next page.
    pub(crate) fn return_work_session(
        &self,
        profile: ICoreWebView2CookieManager,
        scope: zephium_agentic::ContextCookieScope,
        deadline: Instant,
        completion: impl FnOnce(crate::work_site_session::SyncResult) + 'static,
        callback_panicked: impl Fn() + 'static,
    ) -> bool {
        let Some(store) = self.work_store.clone() else {
            return false;
        };
        if store.busy.get()
            || !store.return_pending.get()
            || self.work_seed_cancelled.get()
            || self
                .work_return_transfer
                .borrow()
                .as_ref()
                .is_some_and(|transfer| !transfer.is_terminal())
        {
            return false;
        }
        let Ok(work_store) = self.work_presence_cookie_manager() else {
            return false;
        };
        store.busy.set(true);
        let finished = store.clone();
        let attempt = super::cookie_transfer::WindowsAgentCookieTransfer::start_work_sync(
            work_store,
            profile,
            scope,
            deadline,
            move |terminal| {
                let result = sync_result(terminal);
                finished.settle_return(result);
                finished.busy.set(false);
                completion(result);
                finished.notify();
            },
            callback_panicked,
        );
        match attempt {
            Ok(attempt) => {
                retain_transfer(&self.work_return_transfer, attempt);
                true
            }
            Err(_) => {
                store.busy.set(false);
                store.notify();
                false
            }
        }
    }

    pub(crate) fn admit_work_store_page(&mut self) -> bool {
        let Some(store) = &self.work_store else {
            return true;
        };
        if store.busy.get() || !self.work_storage_ready() {
            return false;
        }
        if !self.work_store_page_active {
            self.work_store_page_active = true;
            store
                .active_pages
                .set(store.active_pages.get().saturating_add(1));
        }
        true
    }

    pub(crate) fn work_profile_matches(&self, expected: &AgentOwnedProfile) -> bool {
        matches!((&self.profile, expected), (AgentOwnedProfile::Automation { name: actual }, AgentOwnedProfile::Automation { name: expected }) if actual == expected)
    }
    #[cfg(feature = "native-agentic-work-lifetime-diagnostic")]
    pub(crate) fn work_cookie_profile_hash(&self) -> Result<String, ()> {
        use sha2::{Digest, Sha256};
        let profile = controller_profile(&self.view.webview())?;
        let name = profile_name(&profile)?;
        Ok(format!("{:x}", Sha256::digest(name.as_bytes())))
    }
    #[cfg(feature = "native-agentic-work-lifetime-diagnostic")]
    pub(crate) fn diagnose_work_cookie_disk(&self) {
        super::cookie_storage_diagnostic::observe_profile(
            &self.view.webview(),
            &self.expected_user_data_folder,
        );
    }
    pub(crate) fn work_presence_cookie_manager(&self) -> Result<ICoreWebView2CookieManager, ()> {
        attest_profile(
            &self.profile,
            &self.view.environment(),
            &self.view.webview(),
            self.storage_class,
            &self.expected_user_data_folder,
        )
        .map_err(|_| ())?;
        let core = self
            .view
            .webview()
            .cast::<ICoreWebView2_2>()
            .map_err(|_| ())?;
        // SAFETY: exact live controller/profile binding was reattested on its STA; returned manager is AddRef'd.
        unsafe { core.CookieManager() }.map_err(|_| ())
    }
    pub(crate) fn work_storage_ready(&self) -> bool {
        self.work_storage_ready.get()
            && !self.work_seed_query.get()
            && self
                .work_cookie_transfer
                .borrow()
                .as_ref()
                .is_none_or(|transfer| transfer.is_terminal())
    }
    pub(crate) fn cancel_work_storage(&self) -> bool {
        self.work_seed_cancelled.set(true);
        if self.work_seed_query.get() {
            return false;
        }
        let settled =
            |slot: &RefCell<Option<super::cookie_transfer::WindowsAgentCookieTransfer>>| {
                slot.borrow().as_ref().is_none_or(|transfer| {
                    if transfer.is_terminal() {
                        true
                    } else {
                        transfer.cancel(zephium_agentic::ContextCookieTransferFailure::Shutdown);
                        false
                    }
                })
            };
        let seeded = settled(&self.work_cookie_transfer);
        let returned = settled(&self.work_return_transfer);
        seeded && returned
    }
    pub(crate) fn park_semantic_runtime(
        &mut self,
        completion: impl FnOnce(bool) + 'static,
    ) -> Result<(), ()> {
        let clean = self
            .semantic()
            .is_some_and(|runtime| runtime.work_drained_for_audit() == Some(true));
        self.work_parked.set(clean);
        completion(clean);
        if clean {
            Ok(())
        } else {
            Err(())
        }
    }
    pub(crate) fn semantic_runtime_parked(&self) -> bool {
        self.work_parked.get()
    }
    pub(crate) fn work_native_activity_drained(&self) -> bool {
        !self.work_suspend_pending.get() && self.history.drained()
    }
    pub(crate) fn set_work_leased(&self, leased: bool) -> bool {
        self.work_leased.set(leased);
        self.set_work_native_active(leased || self.work_human_active.get())
    }
    pub(crate) fn activate_work_human(&self) -> bool {
        self.work_human_active.set(true);
        self.set_work_native_active(true)
    }
    /// Keeps continuation outstanding until suspension settles. Timeout refuses
    /// continuation while the view retains the outstanding native callback debt.
    pub(crate) fn finish_work_human_activity(&self, completion: impl FnOnce(bool) + 'static) {
        self.work_human_active.set(false);
        if self.work_leased.get() || self.work_idle_completion.borrow().is_some() {
            completion(false);
            return;
        }
        let pending = self.work_idle_completion.clone();
        let expired = pending.clone();
        let panic = self.work_activity_panic.clone();
        let notify = self.work_activity_notify.clone();
        let timeout = crate::platform::imp::schedule_content_policy_timeout(
            std::time::Duration::from_secs(3),
            move || {
                if complete_work_idle(&expired, false) {
                    invoke_unit_callback(panic.as_ref(), panic.as_ref());
                    invoke_unit_callback(notify.as_ref(), panic.as_ref());
                }
            },
        );
        let Some(timeout) = timeout else {
            completion(false);
            return;
        };
        *pending.borrow_mut() = Some(Box::new(move |clean| {
            drop(timeout);
            completion(clean);
        }));
        let admitted = self.set_work_native_active(false);
        if !admitted || !self.work_suspend_pending.get() {
            complete_work_idle(&pending, admitted);
        }
    }
    fn set_work_native_active(&self, active: bool) -> bool {
        if active {
            let _ = self
                .view
                .set_memory_usage_level(wry::MemoryUsageLevel::Normal);
            // SAFETY: retained controller and its core belong to the current STA; Resume cancels a late suspension.
            unsafe {
                self.view
                    .webview()
                    .cast::<ICoreWebView2_3>()
                    .is_ok_and(|core| core.Resume().is_ok())
                    && self.view.controller().SetIsVisible(true).is_ok()
            }
        } else {
            let _ = self.view.set_memory_usage_level(wry::MemoryUsageLevel::Low);
            // SAFETY: the exact idle controller is hidden before invoking native suspension.
            if unsafe { self.view.controller().SetIsVisible(false) }.is_err() {
                return false;
            }
            if self.work_suspend_pending.get() {
                return true;
            }
            if self.is_suspended().is_ok_and(|suspended| suspended) {
                return true;
            }
            self.work_suspend_pending.set(true);
            let pending = self.work_suspend_pending.clone();
            let leased = self.work_leased.clone();
            let human = self.work_human_active.clone();
            let idle_completion = self.work_idle_completion.clone();
            let core = self.view.webview().clone();
            let controller = self.view.controller().clone();
            let notify = self.work_activity_notify.clone();
            let panic = self.work_activity_panic.clone();
            let result = self.try_suspend(
                move |suspended| {
                    pending.set(false);
                    let active = leased.get() || human.get();
                    if active {
                        // SAFETY: these exact retained interfaces stay on their owning STA until this callback drains.
                        let resumed = unsafe {
                            core.cast::<ICoreWebView2_3>()
                                .is_ok_and(|core| core.Resume().is_ok())
                                && controller.SetIsVisible(true).is_ok()
                        };
                        if !resumed {
                            invoke_unit_callback(panic.as_ref(), panic.as_ref());
                        }
                    } else if !suspended {
                        invoke_unit_callback(panic.as_ref(), panic.as_ref());
                    }
                    complete_work_idle(&idle_completion, !active && suspended);
                    invoke_unit_callback(notify.as_ref(), panic.as_ref());
                },
                {
                    let panic = self.work_activity_panic.clone();
                    move || invoke_unit_callback(panic.as_ref(), panic.as_ref())
                },
            );
            if result.is_err() {
                self.work_suspend_pending.set(false);
            }
            result.is_ok()
        }
    }
    pub(crate) fn enroll_work_history_with_completion(
        &mut self,
        target: zephium_agentic::ContextNavigationTarget,
        new_lease: bool,
        completion: impl FnOnce(bool) + 'static,
    ) {
        if new_lease && self.history.clear().is_err() {
            completion(false);
            return;
        }
        let completed = Rc::new(std::cell::RefCell::new(Some(completion)));
        let callback = completed.clone();
        let history = self.history.clone();
        let notify = self.work_activity_notify.clone();
        let result = self.history.enroll(
            &self.view.webview(),
            target,
            Rc::new(move || {
                let completion = callback.borrow_mut().take();
                if let Some(completion) = completion {
                    completion(history.healthy());
                }
                notify();
            }),
            self.work_activity_panic.clone(),
        );
        if result.is_err() {
            if let Some(completion) = completed.borrow_mut().take() {
                completion(false);
            }
        }
    }
    pub(crate) fn prepare_history_back(&mut self) -> Result<super::AgentHistoryBackTicket, ()> {
        self.history.authorize()
    }
    pub(crate) fn reactivate_history_destination(
        &mut self,
        ticket: super::AgentHistoryBackTicket,
    ) -> Result<zephium_agentic::ContextNavigationTarget, ()> {
        let target = self.history.target(ticket).ok_or(())?;
        self.prepare_semantic_document_load()?;
        Ok(target)
    }
    pub(crate) fn dispatch_history_back_guarded(
        &mut self,
        ticket: super::AgentHistoryBackTicket,
        authority: Box<dyn Fn() -> bool>,
    ) -> bool {
        self.history.dispatch(
            &self.view.webview(),
            ticket,
            Rc::from(authority),
            self.work_activity_notify.clone(),
            self.work_activity_panic.clone(),
        )
    }
    pub(crate) fn semantic_runtime_ready_for_history(&self) -> bool {
        let current = super::current_url(&self.view);
        if self
            .semantic()
            .is_none_or(|runtime| runtime.document_content_available_for_audit() != Some(true))
            || self
                .work_navigation
                .as_ref()
                .is_none_or(|gate| !gate.ready(current.as_deref()))
        {
            return false;
        }
        self.history.ready_for_settlement(
            &self.view.webview(),
            self.work_activity_notify.clone(),
            self.work_activity_panic.clone(),
        )
    }
    pub(crate) fn settle_history_back(
        &mut self,
        ticket: super::AgentHistoryBackTicket,
    ) -> Result<(), ()> {
        self.history.settle(ticket)
    }
    pub(crate) fn refuse_history_back(&mut self, ticket: super::AgentHistoryBackTicket) -> bool {
        self.history.refuse(ticket)
    }
    pub(crate) fn work_navigation(
        &self,
    ) -> Option<&crate::platform::work_document_navigation::WorkDocumentNavigation> {
        self.work_navigation.as_ref()
    }

    pub(crate) fn sample_work_human_wall(
        &self,
        completion: impl FnOnce(String) + 'static,
    ) -> Result<(), ()> {
        // Only the original, live human-owned navigation phase may evaluate
        // this fixed survey. It is never an automated or hidden-decision tool.
        self.work_navigation()
            .and_then(|gate| gate.human_wall_probe())
            .ok_or(())?;
        self.semantic().ok_or(())?.sample_human_wall(completion);
        Ok(())
    }
    pub(crate) fn semantic(
        &self,
    ) -> Option<&super::semantic_runtime::AgentSemanticRuntimeController> {
        self.semantic
            .as_ref()
            .map(super::semantic_runtime::AgentSemanticRuntimeRegistration::controller)
    }

    pub(crate) fn prepare_semantic_document_load(&mut self) -> Result<(), ()> {
        self.semantic().ok_or(())?.begin_document_load()
    }

    pub(crate) fn retire_semantic_runtime(&mut self) -> bool {
        if self
            .work_network
            .as_mut()
            .is_some_and(|policy| !policy.retire())
        {
            return false;
        }
        self.work_network = None;
        self.semantic
            .take()
            .is_some_and(|registration| registration.retire().is_ok())
    }

    pub(crate) const fn semantic_is_live(&self) -> bool {
        self.semantic.is_some()
    }

    /// Dispatches a retained Work observation in its exact native isolated context.
    pub(crate) fn dispatch_semantic(
        &self,
        invocation: SemanticRuntimeInvocation,
        completion: impl FnOnce(Result<SemanticSnapshot, SemanticRuntimePortFailure>) + 'static,
    ) -> Result<(), SemanticRuntimePortFailure> {
        let Some(semantic) = self.semantic() else {
            completion(Err(SemanticRuntimePortFailure::Retired));
            return Err(SemanticRuntimePortFailure::Retired);
        };
        semantic.dispatch(invocation, completion)
    }

    // The legacy screenshot port remains unadmitted on Windows. Retained Work
    // frames use capture_work_frame with their own exact resource/document fence.
    #[allow(dead_code)]
    pub(crate) fn record_media_ports(
        &self,
    ) -> Option<(
        crate::platform::record_media::MediaDispatch,
        crate::platform::record_media::FrameCapture,
        crate::platform::record_media::PrivacyDispatch,
    )> {
        let semantic = self.semantic()?.clone();
        let native = self.view.webview();
        let controller = self.view.controller();
        let capture = Rc::new(
            move |crop, completion: crate::platform::record_media::FrameCompletion| {
                super::semantic_screenshot::capture_record_frame_native(
                    &native,
                    &controller,
                    crop,
                    completion,
                )
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

    #[allow(dead_code)]
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
        if self
            .semantic()
            .is_none_or(|semantic| semantic.document_content_available_for_audit() != Some(true))
            || (self.work_navigation.is_none()
                && attest_hidden_owner(&self.view, self.expected_parent, self.viewport).is_err())
        {
            return Err(SemanticScreenshotNativeFailure::NotReady);
        }
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

    pub(crate) fn dispatch_retained_semantic_action(
        &self,
        request: zephium_agentic::SemanticActionNativeRequest,
        admitted_at: Instant,
        authority: Box<dyn Fn() -> bool>,
        completion: impl FnOnce(zephium_agentic::SemanticActionNativeSettlement) + 'static,
    ) {
        let Some(semantic) = self.semantic() else {
            let at = request.requested_at();
            completion(request.fail(zephium_agentic::SemanticActionNativeFailure::Shutdown, at));
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

    pub(crate) fn semantic_pending_for_audit(&self) -> Option<bool> {
        // Logical cancellation cannot erase an original CDP callback or its
        // context-reporting cleanup obligation before physical return.
        self.semantic()?
            .work_drained_for_audit()
            .map(|drained| !drained)
    }

    #[cfg(feature = "native-agentic-semantic-probe")]
    pub(crate) fn semantic_work_drained_for_audit(&self) -> Option<bool> {
        self.semantic()?.work_drained_for_audit()
    }

    /// Starts WebView2's invisible-view suspension primitive.
    ///
    /// The host retains the transition task and a separate reconciliation
    /// claim because WebView2 exposes no cancellation for this callback.
    pub(crate) fn try_suspend(
        &self,
        completion: impl FnOnce(bool) + 'static,
        callback_panicked: impl Fn() + 'static,
    ) -> Result<(), AgentOwnedViewConstructionError> {
        attest_hidden_owner(&self.view, self.expected_parent, self.viewport)?;
        if self.is_suspended()? {
            return Err(AgentOwnedViewConstructionError::Native);
        }
        let core = self
            .view
            .webview()
            .cast::<ICoreWebView2_3>()
            .map_err(|_| AgentOwnedViewConstructionError::Native)?;
        let handler = TrySuspendCompletedHandler::create(Box::new(move |result, suspended| {
            if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                completion(result.is_ok() && suspended);
            }))
            .is_err()
            {
                let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(callback_panicked));
            }
            Ok(())
        }));
        // SAFETY: `core` and `handler` are live reference-counted COM
        // interfaces on the owning STA. WebView2 retains the one-shot handler
        // until completion; the host separately bounds and reconciles it.
        unsafe { core.TrySuspend(&handler) }.map_err(|_| AgentOwnedViewConstructionError::Native)
    }

    /// Reads the exact native suspension bit without exposing the COM object.
    pub(crate) fn is_suspended(&self) -> Result<bool, AgentOwnedViewConstructionError> {
        let core = self
            .view
            .webview()
            .cast::<ICoreWebView2_3>()
            .map_err(|_| AgentOwnedViewConstructionError::Native)?;
        let mut suspended = windows_core::BOOL::default();
        // SAFETY: `core` is the live view-owned interface and `suspended` is
        // initialized writable storage of the exact BOOL type.
        unsafe { core.IsSuspended(&mut suspended) }
            .map_err(|_| AgentOwnedViewConstructionError::Native)?;
        Ok(suspended.as_bool())
    }

    /// Reattests the fixed hidden owner and reads the final native bit.
    ///
    /// The read happens after every other attestation API because Microsoft
    /// documents that some WebView APIs can implicitly resume a suspended
    /// view. The returned bit is therefore the state the host may record.
    pub(crate) fn attest_suspension_state(&self) -> Result<bool, AgentOwnedViewConstructionError> {
        attest_hidden_owner(&self.view, self.expected_parent, self.viewport)?;
        self.is_suspended()
    }

    /// Requests native resume and returns whether active state was proven.
    ///
    /// An HRESULT alone is not the postcondition: a failed call can race an
    /// already-active view, while a successful call must still be checked.
    pub(crate) fn resume_and_attest_active(&self) -> Result<bool, AgentOwnedViewConstructionError> {
        let core = self
            .view
            .webview()
            .cast::<ICoreWebView2_3>()
            .map_err(|_| AgentOwnedViewConstructionError::Native)?;
        // SAFETY: `core` is owned by this view on its creating STA. Microsoft
        // documents that the page is immediately interactive after success.
        let _resume_result = unsafe { core.Resume() };
        attest_hidden_owner(&self.view, self.expected_parent, self.viewport)?;
        self.is_suspended().map(|suspended| !suspended)
    }

    pub(crate) fn attest(&self) -> Result<(), AgentOwnedViewConstructionError> {
        attest_profile(
            &self.profile,
            &self.view.environment(),
            &self.view.webview(),
            self.storage_class,
            &self.expected_user_data_folder,
        )?;
        if !self
            .semantic()
            .is_some_and(|semantic| semantic.attest(&self.view.webview()))
        {
            return Err(AgentOwnedViewConstructionError::ExtensionIsolation);
        }
        attest_hidden_owner(&self.view, self.expected_parent, self.viewport)
    }

    /// Returns the exact automation-subprofile cookie mutation authority.
    ///
    /// The selected/default profile is deliberately unrepresentable here: it
    /// is a read-only source for this bridge, never a destination. Hidden
    /// ownership, active suspension state, controller profile, and the
    /// original environment are re-attested before either COM owner escapes.
    pub(crate) fn cookie_destination(
        &self,
        expected_environment: &ICoreWebView2Environment,
    ) -> Result<(ICoreWebView2CookieManager, ICoreWebView2Profile2), AgentOwnedViewConstructionError>
    {
        if !matches!(self.profile, AgentOwnedProfile::Automation { .. }) {
            return Err(AgentOwnedViewConstructionError::ExtensionIsolation);
        }
        if !super::same_environment(&self.view.environment(), expected_environment) {
            return Err(AgentOwnedViewConstructionError::Storage);
        }
        self.attest()?;
        if self.attest_suspension_state()? {
            return Err(AgentOwnedViewConstructionError::Native);
        }
        let core = self
            .view
            .webview()
            .cast::<ICoreWebView2_2>()
            .map_err(|_| AgentOwnedViewConstructionError::Native)?;
        // SAFETY: `self.attest` rejoined this exact live controller with its
        // retained environment/profile and hidden owner. These getters return
        // separately AddRef'd profile-scoped COM owners on the same STA.
        let manager =
            unsafe { core.CookieManager() }.map_err(|_| AgentOwnedViewConstructionError::Native)?;
        let profile = controller_profile(&self.view.webview())
            .and_then(|profile| profile.cast::<ICoreWebView2Profile2>().map_err(|_| ()))
            .map_err(|_| AgentOwnedViewConstructionError::Native)?;
        Ok((manager, profile))
    }

    pub(crate) fn close(&mut self) -> Result<(), wry::WebView2CleanupDebt> {
        if self.semantic_is_live() {
            let _ = self.retire_semantic_runtime();
        }
        let result = self.view.close();
        if result.is_ok() && self.work_store_page_active {
            self.work_store_page_active = false;
            if let Some(store) = &self.work_store {
                store
                    .active_pages
                    .set(store.active_pages.get().saturating_sub(1));
                store.notify();
            }
        }
        result
    }
}

fn open_work_store(admission: &WorkStorageAdmission, job: WorkSeedJob, stamp: bool) {
    let store = &admission.store;
    let opened = Instant::now() < job.deadline
        && (!stamp
            || store
                .metadata
                .initialize(&store.site_name, &job.origin)
                .is_ok())
        && Instant::now() < job.deadline;
    if opened {
        if stamp {
            store.origins.borrow_mut().insert(job.origin);
        }
        admission.ready.set(true);
    }
    store.busy.set(false);
    (job.completion)(opened, false);
    store.notify();
}

fn fail_work_store(admission: &WorkStorageAdmission, job: WorkSeedJob, unproven: bool) {
    admission.ready.set(false);
    admission.store.busy.set(false);
    (job.completion)(false, unproven);
    admission.store.notify();
}

fn refresh_work_store(admission: &Rc<WorkStorageAdmission>, job: WorkSeedJob) {
    let Some(profile) = job.profile.clone() else {
        open_work_store(admission, job, false);
        return;
    };
    let scope = job.scope.clone();
    let deadline = job.deadline;
    let panic = job.panic.clone();
    let pending = Rc::new(RefCell::new(Some(job)));
    let finished = pending.clone();
    let owner = admission.clone();
    let attempt = super::cookie_transfer::WindowsAgentCookieTransfer::start_work_sync(
        profile,
        admission.work_store.clone(),
        scope,
        deadline,
        move |terminal| {
            use crate::work_site_session::{after_refresh, AfterRefresh};
            let result = sync_result(terminal);
            let Some(job) = finished.borrow_mut().take() else {
                return;
            };
            match after_refresh(result) {
                AfterRefresh::Open { stamp } => open_work_store(&owner, job, stamp),
                AfterRefresh::Fail => fail_work_store(&owner, job, true),
            }
        },
        move || panic(),
    );
    match attempt {
        Ok(attempt) => retain_transfer(&admission.transfer, attempt),
        Err(_) => {
            if let Some(job) = pending.borrow_mut().take() {
                open_work_store(admission, job, false);
            }
        }
    }
}

fn return_then_refresh(admission: &Rc<WorkStorageAdmission>, job: WorkSeedJob) {
    let Some(profile) = job.profile.clone() else {
        open_work_store(admission, job, false);
        return;
    };
    let scope = job.scope.clone();
    let deadline = job.deadline;
    let panic = job.panic.clone();
    let pending = Rc::new(RefCell::new(Some(job)));
    let finished = pending.clone();
    let owner = admission.clone();
    let attempt = super::cookie_transfer::WindowsAgentCookieTransfer::start_work_sync(
        admission.work_store.clone(),
        profile,
        scope,
        deadline,
        move |terminal| {
            use crate::work_site_session::SyncResult;
            let result = sync_result(terminal);
            let Some(job) = finished.borrow_mut().take() else {
                return;
            };
            let pending = owner.store.settle_return(result);
            if result == SyncResult::Unproven {
                fail_work_store(&owner, job, true);
            } else if pending {
                open_work_store(&owner, job, false);
            } else {
                refresh_work_store(&owner, job);
            }
        },
        move || panic(),
    );
    match attempt {
        Ok(attempt) => retain_transfer(&admission.transfer, attempt),
        Err(_) => {
            if let Some(job) = pending.borrow_mut().take() {
                open_work_store(admission, job, false);
            }
        }
    }
}

/// A transfer that settled synchronously needs no owner; keeping it would
/// replace the live one its terminal callback may already have started.
fn retain_transfer(
    slot: &RefCell<Option<super::cookie_transfer::WindowsAgentCookieTransfer>>,
    attempt: super::cookie_transfer::WindowsAgentCookieTransfer,
) {
    if !attempt.is_terminal() {
        if let Ok(mut slot) = slot.try_borrow_mut() {
            *slot = Some(attempt);
        }
    }
}

fn sync_result(
    terminal: super::cookie_transfer::WindowsAgentCookieTerminal,
) -> crate::work_site_session::SyncResult {
    crate::work_site_session::SyncResult::new(
        matches!(
            terminal.outcome(),
            zephium_agentic::ContextCookieTransferOutcome::Applied(_)
        ),
        terminal.cleanup() == super::cookie_transfer::WindowsAgentCookieCleanup::Unproven,
    )
}

type WorkIdleCompletion = Rc<RefCell<Option<Box<dyn FnOnce(bool)>>>>;
#[cfg(test)]
mod work_idle_completion_tests {
    use super::*;
    #[test]
    fn late_native_idle_terminal_cannot_complete_timed_out_human_again() {
        let values = Rc::new(RefCell::new(Vec::new()));
        let recorded = values.clone();
        let pending: WorkIdleCompletion = Rc::new(RefCell::new(Some(Box::new(move |clean| {
            recorded.borrow_mut().push(clean)
        }))));
        assert!(complete_work_idle(&pending, false));
        assert!(!complete_work_idle(&pending, true));
        assert_eq!(*values.borrow(), vec![false]);
    }
    #[test]
    fn idle_completion_releases_owner_before_successor_callback() {
        let pending: WorkIdleCompletion = Rc::new(RefCell::new(None));
        let successor = pending.clone();
        let called = Rc::new(Cell::new(false));
        let next = called.clone();
        *pending.borrow_mut() = Some(Box::new(move |clean| {
            assert!(clean);
            *successor
                .try_borrow_mut()
                .expect("old callback borrow released") =
                Some(Box::new(move |clean| next.set(clean)));
        }));
        assert!(complete_work_idle(&pending, true));
        assert!(complete_work_idle(&pending, true));
        assert!(called.get());
    }
}
fn complete_work_idle(pending: &WorkIdleCompletion, clean: bool) -> bool {
    let completion = pending.borrow_mut().take();
    if let Some(completion) = completion {
        completion(clean);
        true
    } else {
        false
    }
}

fn invoke_navigation_callback(
    callback: &dyn Fn(AgentNavigationTerminal),
    callback_panicked: &dyn Fn(),
    terminal: AgentNavigationTerminal,
) {
    if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| callback(terminal))).is_err() {
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(callback_panicked));
    }
}

fn invoke_unit_callback(callback: &dyn Fn(), callback_panicked: &dyn Fn()) {
    if std::panic::catch_unwind(std::panic::AssertUnwindSafe(callback)).is_err() {
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(callback_panicked));
    }
}

fn controller_profile(core: &ICoreWebView2) -> Result<ICoreWebView2Profile7, ()> {
    // SAFETY: `cast` yields a live, reference-counted WebView2 interface; the
    // property getter initializes and returns its own reference-counted profile.
    core.cast::<ICoreWebView2_13>()
        .and_then(|core| unsafe { core.Profile() })
        .and_then(|profile| profile.cast::<ICoreWebView2Profile7>())
        .map_err(|_| ())
}

fn profile_name(profile: &ICoreWebView2Profile7) -> Result<String, ()> {
    let profile = profile.cast::<ICoreWebView2Profile>().map_err(|_| ())?;
    let mut raw = PWSTR::null();
    // SAFETY: `profile` is a live COM interface and `raw` is a valid initialized
    // out pointer. WebView2 allocates the returned string; the bounded helper
    // consumes and releases it exactly once, including refusal paths.
    unsafe { profile.ProfileName(&mut raw) }.map_err(|_| ())?;
    super::take_pwstr_bounded(raw, PROFILE_NAME_UTF16_LIMIT, PROFILE_NAME_UTF8_LIMIT).ok_or(())
}

fn profile_is_private(profile: &ICoreWebView2Profile7) -> Result<bool, ()> {
    let profile = profile.cast::<ICoreWebView2Profile>().map_err(|_| ())?;
    let mut private = windows_core::BOOL::default();
    // SAFETY: `profile` is a live COM interface and `private` provides writable
    // storage of the exact BOOL type required by this synchronous getter.
    unsafe { profile.IsInPrivateModeEnabled(&mut private) }.map_err(|_| ())?;
    Ok(private.as_bool())
}

fn controller_environment_matches(
    environment: &ICoreWebView2Environment,
    core: &ICoreWebView2,
) -> bool {
    // SAFETY: `cast` yields a live, reference-counted WebView2 interface; the
    // environment getter returns a separately reference-counted COM object.
    core.cast::<ICoreWebView2_2>()
        .and_then(|core| unsafe { core.Environment() })
        .is_ok_and(|controller_environment| {
            super::same_environment(environment, &controller_environment)
        })
}

fn expected_parent_hwnd(
    parent: &impl HasWindowHandle,
) -> Result<HWND, AgentOwnedViewConstructionError> {
    match parent
        .window_handle()
        .map_err(|_| AgentOwnedViewConstructionError::Native)?
        .as_raw()
    {
        RawWindowHandle::Win32(parent) => Ok(HWND(parent.hwnd.get() as _)),
        _ => Err(AgentOwnedViewConstructionError::Native),
    }
}

fn attest_hidden_owner(
    view: &WebView,
    expected_parent: HWND,
    viewport: ContextOwnedViewport,
) -> Result<(), AgentOwnedViewConstructionError> {
    let container = view.hwnd();
    // SAFETY: these Win32 predicates accept opaque HWND values, including
    // stale ones, without dereferencing caller memory. Successful IsWindow
    // checks establish both handles before the parent query is trusted.
    let hierarchy_matches = unsafe {
        IsWindow(Some(expected_parent)).as_bool()
            && IsWindow(Some(container)).as_bool()
            && GetParent(container).ok() == Some(expected_parent)
    };
    if !hierarchy_matches {
        return Err(AgentOwnedViewConstructionError::Native);
    }
    let controller = view.controller();
    let mut controller_parent = HWND::default();
    let mut controller_visible = windows_core::BOOL::default();
    // SAFETY: `container` was proven to identify a live window above and this
    // query neither receives nor retains caller-owned pointers.
    let dpi = unsafe { GetDpiForWindow(container) };
    let Some(expected_width) = expected_physical_extent(viewport.width(), dpi) else {
        return Err(AgentOwnedViewConstructionError::Native);
    };
    let Some(expected_height) = expected_physical_extent(viewport.height(), dpi) else {
        return Err(AgentOwnedViewConstructionError::Native);
    };
    let mut container_bounds = RECT::default();
    let mut controller_bounds = RECT::default();
    // SAFETY: `controller` is the live reference-counted controller owned by
    // `view`; every out argument is initialized writable storage of the exact
    // COM/Win32 type, and `container` was proven live above.
    let native_state_matches = unsafe {
        controller.ParentWindow(&mut controller_parent).is_ok()
            && controller_parent == container
            && controller.IsVisible(&mut controller_visible).is_ok()
            && !controller_visible.as_bool()
            && !IsWindowVisible(container).as_bool()
            && GetClientRect(container, &mut container_bounds).is_ok()
            && controller.Bounds(&mut controller_bounds).is_ok()
    };
    if !native_state_matches
        || container_bounds.left != 0
        || container_bounds.top != 0
        || container_bounds.right != expected_width
        || container_bounds.bottom != expected_height
        || controller_bounds.left != 0
        || controller_bounds.top != 0
        || controller_bounds.right != expected_width
        || controller_bounds.bottom != expected_height
    {
        return Err(AgentOwnedViewConstructionError::Native);
    }
    // SAFETY: GetFocus returns an opaque HWND for this UI thread. `container`
    // is live, and IsChild accepts a null or stale candidate without touching
    // caller memory.
    let focus_inside = unsafe {
        let focus = GetFocus();
        !focus.0.is_null() && (focus == container || IsChild(container, focus).as_bool())
    };
    if focus_inside {
        return Err(AgentOwnedViewConstructionError::Native);
    }
    Ok(())
}

fn expected_physical_extent(logical: u16, dpi: u32) -> Option<i32> {
    if dpi == 0 {
        return None;
    }
    let scaled = u64::from(logical)
        .checked_mul(u64::from(dpi))?
        .checked_add(48)?
        / 96;
    i32::try_from(scaled).ok().filter(|extent| *extent > 0)
}

fn owned_profile_name_is_canonical(name: &str) -> bool {
    if let Some(identity) = name.strip_prefix("agent-") {
        return ProfileId::parse(identity).is_some_and(|profile| profile.to_string() == identity);
    }
    if let Some(identity) = name.strip_prefix("work-site-") {
        use base64::Engine as _;
        return base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(identity)
            .is_ok_and(|bytes| {
                bytes.len() == 32
                    && base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes) == identity
            });
    }
    name.strip_prefix("work-").is_some_and(|identity| {
        identity.len() == 32
            && identity
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    })
}

fn attest_profile(
    binding: &AgentOwnedProfile,
    environment: &ICoreWebView2Environment,
    core: &ICoreWebView2,
    storage_class: ContextProfileStorageClass,
    expected_user_data_folder: &Path,
) -> Result<(), AgentOwnedViewConstructionError> {
    super::attest_environment(environment, expected_user_data_folder)
        .map_err(|_| AgentOwnedViewConstructionError::Storage)?;
    if !controller_environment_matches(environment, core) {
        return Err(AgentOwnedViewConstructionError::Storage);
    }
    let profile = controller_profile(core)
        .map_err(|_| AgentOwnedViewConstructionError::ExtensionIsolation)?;
    match binding {
        AgentOwnedProfile::Selected => {
            if storage_class != ContextProfileStorageClass::Durable
                || profile_is_private(&profile) != Ok(false)
            {
                return Err(AgentOwnedViewConstructionError::Storage);
            }
        }
        AgentOwnedProfile::Automation { name } => {
            if name.len() > PROFILE_NAME_UTF8_LIMIT
                || !name.is_ascii()
                || !owned_profile_name_is_canonical(name)
                || profile_name(&profile).as_deref() != Ok(name.as_str())
                || profile_is_private(&profile)
                    != Ok(storage_class == ContextProfileStorageClass::Ephemeral)
            {
                return Err(AgentOwnedViewConstructionError::Storage);
            }
        }
    }
    Ok(())
}

/// Builds one initially hidden selected-profile or automation WebView2.
///
/// The only initial document is `about:blank`. No page-world script, IPC
/// handler, popup callback, generic native bridge, selector, or model-facing
/// program is installed. The semantic program is installed lazily only in a
/// native-proven non-universal isolated world. Network navigation remains
/// denied until the host arms one exact operation and attaches its native
/// content policy.
#[expect(
    clippy::too_many_arguments,
    reason = "Native construction independently attests parent, viewport, environment, profile, storage, deadline and callbacks."
)]
pub(crate) fn build_owned_agent_view<
    Navigation,
    Location,
    RendererLost,
    BrowserLost,
    Invariant,
    Panic,
>(
    parent: &impl HasWindowHandle,
    viewport: ContextOwnedViewport,
    environment: &ICoreWebView2Environment,
    profile: AgentOwnedProfile,
    storage_class: ContextProfileStorageClass,
    expected_user_data_folder: &Path,
    deadline: Instant,
    extensions_enabled: bool,
    callbacks: AgentOwnedViewCallbacks<
        Navigation,
        Location,
        RendererLost,
        BrowserLost,
        Invariant,
        Panic,
    >,
) -> Result<(AgentOwnedView, ContextConstructionProof), AgentOwnedViewConstructionError>
where
    Navigation: Fn(AgentNavigationTerminal) + 'static,
    Location: Fn() + 'static,
    RendererLost: Fn() + 'static,
    BrowserLost: Fn() + 'static,
    Invariant: Fn() + 'static,
    Panic: Fn() + 'static,
{
    build_owned_agent_view_impl(
        parent,
        viewport,
        environment,
        profile,
        storage_class,
        expected_user_data_folder,
        deadline,
        extensions_enabled,
        None,
        callbacks,
    )
}

#[expect(
    clippy::too_many_arguments,
    reason = "Native construction independently attests parent, viewport, environment, profile, storage, deadline and callbacks."
)]
pub(crate) fn build_owned_work_view<
    Navigation,
    Location,
    RendererLost,
    BrowserLost,
    Invariant,
    Panic,
>(
    parent: &impl HasWindowHandle,
    viewport: ContextOwnedViewport,
    environment: &ICoreWebView2Environment,
    profile: AgentOwnedProfile,
    storage_class: ContextProfileStorageClass,
    expected_user_data_folder: &Path,
    deadline: Instant,
    extensions_enabled: bool,
    callbacks: AgentOwnedViewCallbacks<
        Navigation,
        Location,
        RendererLost,
        BrowserLost,
        Invariant,
        Panic,
    >,
) -> Result<(AgentOwnedView, ContextConstructionProof), AgentOwnedViewConstructionError>
where
    Navigation: Fn(AgentNavigationTerminal) + 'static,
    Location: Fn() + 'static,
    RendererLost: Fn() + 'static,
    BrowserLost: Fn() + 'static,
    Invariant: Fn() + 'static,
    Panic: Fn() + 'static,
{
    build_owned_agent_view_impl(
        parent,
        viewport,
        environment,
        profile,
        storage_class,
        expected_user_data_folder,
        deadline,
        extensions_enabled,
        Some(crate::platform::work_document_navigation::WorkDocumentNavigation::default()),
        callbacks,
    )
}

#[expect(
    clippy::too_many_arguments,
    reason = "Native construction independently attests parent, viewport, environment, profile, storage, deadline and callbacks."
)]
pub(crate) fn build_owned_agent_view_impl<
    Navigation,
    Location,
    RendererLost,
    BrowserLost,
    Invariant,
    Panic,
>(
    parent: &impl HasWindowHandle,
    viewport: ContextOwnedViewport,
    environment: &ICoreWebView2Environment,
    profile: AgentOwnedProfile,
    storage_class: ContextProfileStorageClass,
    expected_user_data_folder: &Path,
    deadline: Instant,
    extensions_enabled: bool,
    work_navigation: Option<crate::platform::work_document_navigation::WorkDocumentNavigation>,
    callbacks: AgentOwnedViewCallbacks<
        Navigation,
        Location,
        RendererLost,
        BrowserLost,
        Invariant,
        Panic,
    >,
) -> Result<(AgentOwnedView, ContextConstructionProof), AgentOwnedViewConstructionError>
where
    Navigation: Fn(AgentNavigationTerminal) + 'static,
    Location: Fn() + 'static,
    RendererLost: Fn() + 'static,
    BrowserLost: Fn() + 'static,
    Invariant: Fn() + 'static,
    Panic: Fn() + 'static,
{
    if Instant::now() >= deadline {
        return Err(AgentOwnedViewConstructionError::Native);
    }
    let expected_parent = expected_parent_hwnd(parent)?;
    let proof = profile.proof();
    let semantic_plan = super::semantic_runtime::AgentSemanticRuntimePlan::prepare()
        .map_err(|_| AgentOwnedViewConstructionError::Native)?;
    let AgentOwnedViewCallbacks {
        navigation: on_navigation,
        location: on_location,
        renderer_lost: on_renderer_lost,
        browser_lost: on_browser_lost,
        invariant: on_invariant_failure,
        panic: on_callback_panic,
        redirect: on_redirect,
        credential_denied: on_credential_denied,
    } = callbacks;
    let navigation = AgentNavigationController::default();
    let navigation_policy = navigation.clone();
    let work_policy = work_navigation.clone();
    let work_events = work_navigation.clone();
    let work_location = work_navigation.clone();
    let navigation_events = navigation.clone();
    let renderer_events = navigation.clone();
    let navigation_callback = Rc::new(on_navigation);
    let location_callback = Rc::new(on_location);
    let navigation_event_location_callback = location_callback.clone();
    let work_activity_notify: Rc<dyn Fn()> = location_callback.clone();
    let renderer_lost_callback = Rc::new(on_renderer_lost);
    let browser_lost_callback = Rc::new(on_browser_lost);
    let invariant_callback = Rc::new(on_invariant_failure);
    let panic_callback = Rc::new(on_callback_panic);
    let redirect_invariant = invariant_callback.clone();
    let redirect_panic = panic_callback.clone();
    // A refused redirect ends the document like any refused load, but its
    // owner reads it as a redirect, never as a broken invariant.
    let on_redirect: Rc<dyn Fn(zephium_agentic::ContextNavigationTarget)> = match on_redirect {
        Some(redirect) => Rc::new(move |destination| {
            invoke_unit_callback(&|| redirect(destination.clone()), redirect_panic.as_ref());
        }),
        None => Rc::new(move |_| {
            invoke_unit_callback(redirect_invariant.as_ref(), redirect_panic.as_ref())
        }),
    };
    let policy_redirect = on_redirect.clone();
    let event_redirect = on_redirect.clone();
    let location_redirect = on_redirect;
    let work_activity_panic: Rc<dyn Fn()> = invariant_callback.clone();
    let navigation_invariant = invariant_callback.clone();
    let renderer_invariant = invariant_callback.clone();
    let semantic_invariant = invariant_callback.clone();
    let navigation_panic = panic_callback.clone();
    let location_panic = panic_callback.clone();
    let renderer_panic = panic_callback.clone();
    let browser_panic = panic_callback.clone();
    let semantic_panic = panic_callback.clone();
    let semantic_navigation = semantic_plan.clone();
    let semantic_policy = semantic_plan.clone();
    let semantic_renderer = semantic_plan.clone();
    let semantic_browser = semantic_plan.clone();

    let mut builder = WebViewBuilder::new()
        .with_url("about:blank")
        .with_bounds(Rect {
            position: Position::Logical(LogicalPosition::new(0.0, 0.0)),
            size: Size::Logical(LogicalSize::new(
                f64::from(viewport.width()),
                f64::from(viewport.height()),
            )),
        })
        .with_visible(false)
        .with_focused(false)
        .with_devtools(false)
        .with_autoplay(false)
        .with_hotkeys_zoom(false)
        .with_clipboard(false)
        .with_fullscreen_enabled(false)
        .with_picture_in_picture_enabled(false)
        .with_general_autofill_enabled(false)
        .with_navigation_handler(move |target| {
            let Some(gate) = work_policy.as_ref() else {
                return navigation_policy.allows(&target);
            };
            let human_prepare = gate.human_load_preparation_needed(Some(true))
                && !gate.human_fragment_move(&target);
            let allowed = gate.allows(&target);
            if let Some(destination) = gate.take_redirect_report().filter(|_| !allowed) {
                policy_redirect(destination);
            }
            let prepare = allowed && (human_prepare || gate.take_hand_on());
            let prepared = !prepare || semantic_policy.begin_document_load().is_ok();
            if !prepared {
                gate.refuse();
                return false;
            }
            allowed
        })
        .with_navigation_event_handler(move |event| {
            if let Some(gate) = &work_events {
                match gate.observe(event) {
                    Ok((committed, notify)) => {
                        if committed && semantic_navigation.document_committed().is_err() {
                            invoke_unit_callback(
                                navigation_invariant.as_ref(),
                                navigation_panic.as_ref(),
                            );
                            return;
                        }
                        if let Some(destination) = gate.redirected() {
                            event_redirect(destination);
                        } else if gate.owned_refusal() {
                            invoke_unit_callback(
                                navigation_event_location_callback.as_ref(),
                                location_panic.as_ref(),
                            );
                        } else if gate.failed() {
                            invoke_unit_callback(
                                navigation_invariant.as_ref(),
                                navigation_panic.as_ref(),
                            );
                        }
                        // Native commit admits visual projection while the
                        // semantic/input gate still waits for Finished. Wake
                        // the original resource owner once for its first paint.
                        if notify || committed {
                            invoke_unit_callback(
                                navigation_event_location_callback.as_ref(),
                                location_panic.as_ref(),
                            );
                        }
                    }
                    Err(()) => invoke_unit_callback(
                        navigation_invariant.as_ref(),
                        navigation_panic.as_ref(),
                    ),
                }
                return;
            }
            match navigation_events.observe(event) {
                Ok(observation) => {
                    if observation.did_commit_document()
                        && semantic_navigation.document_committed().is_err()
                    {
                        invoke_unit_callback(
                            navigation_invariant.as_ref(),
                            navigation_panic.as_ref(),
                        );
                        return;
                    }
                    if observation.should_check_location() {
                        invoke_unit_callback(
                            navigation_event_location_callback.as_ref(),
                            location_panic.as_ref(),
                        );
                    }
                    if let Some(terminal) = observation.into_terminal() {
                        invoke_navigation_callback(
                            navigation_callback.as_ref(),
                            navigation_panic.as_ref(),
                            terminal,
                        );
                    }
                }
                Err(()) => {
                    invoke_unit_callback(navigation_invariant.as_ref(), navigation_panic.as_ref())
                }
            }
        })
        .with_permission_handler(|_| wry::PermissionResponse::Deny)
        .with_download_policy(DownloadPolicy::DenyWithoutMetadata)
        .with_page_close_policy(PageClosePolicy::Ignore)
        .with_additional_browser_args(crate::network::browser_arguments())
        .with_browser_accelerator_keys(false)
        .with_default_context_menus(false)
        .with_environment(environment.clone());
    if work_navigation.is_none() {
        builder = builder.with_navigation_presentation_guard(|| {});
    }
    // Work owns rendering through its presenter and exact preview callback.
    // Its navigation gate revokes semantic document/action authority before
    // load; Wry's ordinary reveal guard would instead hide that retained owner
    // at ContentLoading and strand a preview without revoking the presenter.
    if storage_class == ContextProfileStorageClass::Ephemeral {
        builder = builder.with_incognito(true);
    }

    if let AgentOwnedProfile::Automation { name } = &profile {
        builder = builder.with_profile_name(name.clone());
    }

    builder =
        builder.with_controller_initialization_hook(|_, core| crate::reputation::configure(core));

    if extensions_enabled {
        if !matches!(&profile, AgentOwnedProfile::Automation { .. }) {
            return Err(AgentOwnedViewConstructionError::ExtensionIsolation);
        }
        let expected_environment = environment.clone();
        let expected_path = expected_user_data_folder.to_owned();
        let expected_profile = profile.clone();
        builder = builder.with_browser_extension_startup_gate(move |environment, core| {
            if !super::same_environment(&expected_environment, environment) {
                return Err(windows::Win32::Foundation::E_ACCESSDENIED.into());
            }
            attest_profile(
                &expected_profile,
                environment,
                core,
                storage_class,
                &expected_path,
            )
            .map_err(|_| windows::core::Error::from(windows::Win32::Foundation::E_ACCESSDENIED))?;
            let profile = super::extensions::profile(core)?;
            let items = super::extensions::list(&profile).map_err(|_| {
                windows::core::Error::from(windows::Win32::Foundation::E_ACCESSDENIED)
            })?;
            for item in items {
                let id = super::extensions::extension_id(&item)?;
                if !super::extensions::is_runtime_component(&id) {
                    return Err(windows::Win32::Foundation::E_ACCESSDENIED.into());
                }
            }
            Ok(())
        });
    }

    let view = builder
        .build_as_child(parent)
        .map_err(|_| AgentOwnedViewConstructionError::Native)?;
    attest_profile(
        &profile,
        &view.environment(),
        &view.webview(),
        storage_class,
        expected_user_data_folder,
    )?;
    let location_semantic = semantic_plan.clone();
    let semantic = semantic_plan
        .bind(&view.webview(), semantic_invariant, semantic_panic)
        .map_err(|_| AgentOwnedViewConstructionError::Native)?;
    if !semantic.controller().attest(&view.webview()) {
        return Err(AgentOwnedViewConstructionError::ExtensionIsolation);
    }
    let credential_panic = panic_callback.clone();
    let credential_denied = on_credential_denied.map(|notify| {
        Rc::new(move || invoke_unit_callback(notify.as_ref(), credential_panic.as_ref()))
            as Rc<dyn Fn()>
    });
    let security_policy = super::configure_with_credential_denied(
        &view,
        0.0,
        storage_class == ContextProfileStorageClass::Ephemeral,
        expected_user_data_folder,
        credential_denied,
    )
    .map_err(|_| AgentOwnedViewConstructionError::Native)?;
    attest_hidden_owner(&view, expected_parent, viewport)?;

    let crash_observer = super::install_crash_handler(&view, move |failure| match failure {
        super::ProcessFailure::Renderer => match renderer_events.claim_renderer_loss() {
            Ok(true) => {
                semantic_renderer.renderer_lost();
                invoke_unit_callback(renderer_lost_callback.as_ref(), renderer_panic.as_ref())
            }
            Ok(false) => {}
            Err(()) => invoke_unit_callback(renderer_invariant.as_ref(), renderer_panic.as_ref()),
        },
        super::ProcessFailure::Browser => {
            semantic_browser.renderer_lost();
            let _ = renderer_events.claim_renderer_loss();
            invoke_unit_callback(browser_lost_callback.as_ref(), browser_panic.as_ref());
        }
    })
    .map_err(|_| AgentOwnedViewConstructionError::Native)?;
    let work_network = work_navigation
        .as_ref()
        .map(|gate| super::work_network::WorkNetworkPolicy::install(&view, gate.clone()))
        .transpose()
        .map_err(|_| AgentOwnedViewConstructionError::Native)?;
    let location_events = navigation.clone();
    let location_invariant = invariant_callback.clone();
    let location_panic = panic_callback.clone();
    let location_core = view.webview();
    let navigation_observer = super::install_navigation_observer(&view, move || {
        if let Some(gate) = &work_location {
            match gate.location_changed(super::current_url_core(&location_core).as_deref()) {
                Ok(changed) => {
                    if let Some(destination) = gate.redirected() {
                        location_semantic.revoke_document_authority();
                        location_redirect(destination);
                    } else if gate.failed() {
                        location_semantic.revoke_document_authority();
                        invoke_unit_callback(location_invariant.as_ref(), location_panic.as_ref());
                    }
                    if changed {
                        invoke_unit_callback(location_callback.as_ref(), location_panic.as_ref());
                    }
                }
                Err(()) => {
                    location_semantic.revoke_document_authority();
                    invoke_unit_callback(location_invariant.as_ref(), location_panic.as_ref());
                }
            }
            return;
        }
        match location_events.request_location_check() {
            Ok(true) => invoke_unit_callback(location_callback.as_ref(), location_panic.as_ref()),
            Ok(false) => {}
            Err(()) => invoke_unit_callback(location_invariant.as_ref(), location_panic.as_ref()),
        }
    })
    .map_err(|_| AgentOwnedViewConstructionError::Native)?;
    Ok((
        AgentOwnedView {
            navigation,
            work_navigation,
            semantic: Some(semantic),
            profile,
            storage_class,
            expected_user_data_folder: expected_user_data_folder.to_owned(),
            expected_parent,
            viewport,
            work_native_profile: None,
            work_storage_ready: Rc::new(Cell::new(true)),
            work_cookie_transfer: Rc::new(RefCell::new(None)),
            work_return_transfer: RefCell::new(None),
            work_store: None,
            work_seed_job: Rc::new(RefCell::new(None)),
            work_seed_query: Rc::new(Cell::new(false)),
            work_seed_cancelled: Rc::new(Cell::new(false)),
            work_store_page_active: false,
            work_parked: Cell::new(false),
            work_leased: Rc::new(Cell::new(true)),
            work_human_active: Rc::new(Cell::new(false)),
            work_idle_completion: Rc::new(RefCell::new(None)),
            work_suspend_pending: Rc::new(Cell::new(false)),
            work_activity_notify,
            work_activity_panic,
            work_network,
            history: super::agent_history::AgentHistoryLedger::default(),
            _crash_observer: crash_observer,
            _navigation_observer: navigation_observer,
            _security_policy: security_policy,
            view,
        },
        proof,
    ))
}

#[cfg(test)]
mod tests {
    use super::{expected_physical_extent, owned_profile_name_is_canonical, AgentOwnedProfile};
    use zephium_core::ids::ProfileId;

    #[test]
    fn viewport_extent_uses_the_same_positive_half_up_dpi_rounding_as_wry() {
        assert_eq!(expected_physical_extent(1_280, 96), Some(1_280));
        assert_eq!(expected_physical_extent(800, 120), Some(1_000));
        assert_eq!(expected_physical_extent(1, 144), Some(2));
        assert_eq!(expected_physical_extent(1_280, 0), None);
    }
    #[test]
    fn retained_work_site_profile_is_stable_for_one_site_and_isolates_other_sites() {
        let name = |url| {
            let target = zephium_agentic::ContextNavigationTarget::parse(url).unwrap();
            let AgentOwnedProfile::Automation { name } =
                AgentOwnedProfile::work_site(&target).unwrap()
            else {
                unreachable!()
            };
            assert!(name.len() <= 64 && owned_profile_name_is_canonical(&name));
            name
        };
        assert_eq!(
            name("https://app.slack.com/a"),
            name("https://login.slack.com/b")
        );
        assert_eq!(
            name("https://app.slack.com/a"),
            name("https://app.slack.com/a")
        );
        assert_ne!(
            name("https://app.slack.com/a"),
            name("https://linear.app/a")
        );
        assert_ne!(
            name("https://app.slack.com/a"),
            name("http://app.slack.com/a")
        );
        assert_ne!(
            name("https://app.slack.com/a"),
            name("https://app.slack.com:444/a")
        );
        assert_ne!(name("https://a.github.io/a"), name("https://b.github.io/b"));
        assert!(!owned_profile_name_is_canonical(
            "work-site-../../../Default"
        ));
        assert!(!owned_profile_name_is_canonical(
            "work-site-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA="
        ));
    }
    #[test]
    fn native_owned_profiles_accept_only_exact_host_minted_identities() {
        for profile in [
            AgentOwnedProfile::automation(ProfileId::from(123)),
            AgentOwnedProfile::work([0xabu8; 16]),
        ] {
            let AgentOwnedProfile::Automation { name } = profile else {
                unreachable!()
            };
            assert!(owned_profile_name_is_canonical(&name));
            assert!(!owned_profile_name_is_canonical(&format!(
                "{name}/../Default"
            )));
        }
        for malformed in [
            "work-",
            "work-0000000000000000000000000000000",
            "work-000000000000000000000000000000000",
            "work-ABCDEF00000000000000000000000000",
            "work-0000000000000000000000000000000g",
            "agent-default",
            "agent-0000000000000000000000000a",
            "Default",
        ] {
            assert!(!owned_profile_name_is_canonical(malformed));
        }
    }
}
