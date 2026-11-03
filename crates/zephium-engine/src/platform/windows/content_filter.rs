use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;

#[cfg(test)]
#[path = "content_filter_qualification.rs"]
mod qualification;

use webview2_com::Microsoft::Web::WebView2::Win32::{
    ICoreWebView2, ICoreWebView2_22, COREWEBVIEW2_WEB_RESOURCE_CONTEXT,
    COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL, COREWEBVIEW2_WEB_RESOURCE_CONTEXT_FETCH,
    COREWEBVIEW2_WEB_RESOURCE_CONTEXT_FONT, COREWEBVIEW2_WEB_RESOURCE_CONTEXT_IMAGE,
    COREWEBVIEW2_WEB_RESOURCE_CONTEXT_MEDIA, COREWEBVIEW2_WEB_RESOURCE_CONTEXT_PING,
    COREWEBVIEW2_WEB_RESOURCE_CONTEXT_SCRIPT, COREWEBVIEW2_WEB_RESOURCE_CONTEXT_STYLESHEET,
    COREWEBVIEW2_WEB_RESOURCE_CONTEXT_XML_HTTP_REQUEST,
    COREWEBVIEW2_WEB_RESOURCE_REQUEST_SOURCE_KINDS,
    COREWEBVIEW2_WEB_RESOURCE_REQUEST_SOURCE_KINDS_DOCUMENT,
};
#[cfg(test)]
use webview2_com::Microsoft::Web::WebView2::Win32::{
    COREWEBVIEW2_WEB_RESOURCE_CONTEXT_DOCUMENT,
    COREWEBVIEW2_WEB_RESOURCE_REQUEST_SOURCE_KINDS_SERVICE_WORKER,
    COREWEBVIEW2_WEB_RESOURCE_REQUEST_SOURCE_KINDS_SHARED_WORKER,
};
use webview2_com::WebResourceRequestedEventHandler;
use windows::Win32::Foundation::E_INVALIDARG;
use windows_core::{Interface, PCWSTR, PWSTR};
use wry::WebViewExtWindows;
use zephium_core::blocker::{
    ContentRuleApplyFailure, ContentRuleDigest, ContentRules, ContentRulesPayload, NetworkDecision,
    NetworkRequest, NetworkRequestPolicy, NetworkRequestSourceKind, NetworkResourceType,
    MAX_NETWORK_REQUEST_METHOD_BYTES, MAX_NETWORK_REQUEST_URL_BYTES,
};

const FILTER_PATTERNS: [PCWSTR; 2] = [windows_core::w!("https://*"), windows_core::w!("http://*")];
const FILTER_CONTEXTS: [COREWEBVIEW2_WEB_RESOURCE_CONTEXT; 8] = [
    COREWEBVIEW2_WEB_RESOURCE_CONTEXT_STYLESHEET,
    COREWEBVIEW2_WEB_RESOURCE_CONTEXT_IMAGE,
    COREWEBVIEW2_WEB_RESOURCE_CONTEXT_MEDIA,
    COREWEBVIEW2_WEB_RESOURCE_CONTEXT_FONT,
    COREWEBVIEW2_WEB_RESOURCE_CONTEXT_SCRIPT,
    COREWEBVIEW2_WEB_RESOURCE_CONTEXT_XML_HTTP_REQUEST,
    COREWEBVIEW2_WEB_RESOURCE_CONTEXT_FETCH,
    COREWEBVIEW2_WEB_RESOURCE_CONTEXT_PING,
];
const FILTER_REGISTRATION_COUNT: usize = FILTER_PATTERNS.len() * FILTER_CONTEXTS.len();
const FILTER_SOURCE_KINDS: COREWEBVIEW2_WEB_RESOURCE_REQUEST_SOURCE_KINDS =
    COREWEBVIEW2_WEB_RESOURCE_REQUEST_SOURCE_KINDS_DOCUMENT;

thread_local! {
    /// WebResourceRequested is delivered on the WebView's apartment thread.
    /// Reuse bounded UTF-8 capacity there, but never hold this borrow across
    /// COM response installation because a native call may pump re-entrant work.
    static REQUEST_BUFFERS: RefCell<RequestBuffers> = RefCell::new(RequestBuffers::default());
}

fn filter_registration(index: usize) -> Option<(PCWSTR, COREWEBVIEW2_WEB_RESOURCE_CONTEXT)> {
    if index >= FILTER_REGISTRATION_COUNT {
        return None;
    }
    Some((
        FILTER_PATTERNS[index / FILTER_CONTEXTS.len()],
        FILTER_CONTEXTS[index % FILTER_CONTEXTS.len()],
    ))
}

#[derive(Clone)]
pub(crate) enum NativeContentPolicy {
    AllowAll,
    Runtime {
        digest: ContentRuleDigest,
        policy: Arc<dyn NetworkRequestPolicy>,
    },
}

pub(crate) fn same_policy(left: &NativeContentPolicy, right: &NativeContentPolicy) -> bool {
    match (left, right) {
        (NativeContentPolicy::AllowAll, NativeContentPolicy::AllowAll) => true,
        (
            NativeContentPolicy::Runtime { digest: left, .. },
            NativeContentPolicy::Runtime { digest: right, .. },
        ) => left == right,
        _ => false,
    }
}

pub(crate) struct ContentPolicyRegistration {
    core: Option<ICoreWebView2>,
    core22: Option<ICoreWebView2_22>,
    token: Option<i64>,
    installed_filters: Rc<Cell<usize>>,
    // Set before cleanup touches anything, for a pause change it interrupts.
    retired: Rc<Cell<bool>>,
    pause: Option<crate::platform::content_pause::PauseRegistration>,
}

impl ContentPolicyRegistration {
    pub(crate) fn allow_all() -> Self {
        Self {
            core: None,
            core22: None,
            token: None,
            installed_filters: Rc::default(),
            retired: Rc::default(),
            pause: None,
        }
    }

    pub(crate) fn retire(mut self) -> Result<(), ContentRuleApplyFailure> {
        self.cleanup()
    }

    fn cleanup(&mut self) -> Result<(), ContentRuleApplyFailure> {
        self.retired.set(true);
        // A retired cohort must not reinstall filters on a later pause change.
        self.pause = None;
        if self.core.is_none() && self.core22.is_none() {
            return if self.token.is_none() && self.installed_filters.get() == 0 {
                Ok(())
            } else {
                Err(ContentRuleApplyFailure::NativeCleanup)
            };
        }
        let (Some(core), Some(core22)) = (self.core.clone(), self.core22.clone()) else {
            return Err(ContentRuleApplyFailure::NativeCleanup);
        };

        // Stop the old callback before reducing the duplicate filter
        // reference counts. Replacement installs its complete handler/filter
        // cohort first, so this leaves the new policy continuously active.
        if let Some(token) = self.token {
            if let Err(error) = unsafe { core.remove_WebResourceRequested(token) } {
                eprintln!("content blocker: WebView2 handler removal failed: {error}");
                return Err(ContentRuleApplyFailure::NativeCleanup);
            }
            self.token = None;
        }
        while self.installed_filters.get() != 0 {
            let index = self.installed_filters.get() - 1;
            let Some((pattern, context)) = filter_registration(index) else {
                return Err(ContentRuleApplyFailure::NativeCleanup);
            };
            if let Err(error) = unsafe {
                core22.RemoveWebResourceRequestedFilterWithRequestSourceKinds(
                    pattern,
                    context,
                    FILTER_SOURCE_KINDS,
                )
            } {
                eprintln!("content blocker: WebView2 filter removal failed: {error}");
                return Err(ContentRuleApplyFailure::NativeCleanup);
            }
            self.installed_filters.set(index);
        }
        self.core = None;
        self.core22 = None;
        Ok(())
    }
}

impl Drop for ContentPolicyRegistration {
    fn drop(&mut self) {
        if self.cleanup().is_err() {
            eprintln!("content blocker: WebView2 registration cleanup remained incomplete");
        }
    }
}

pub(crate) fn prepare(
    rules: &ContentRules,
) -> Result<NativeContentPolicy, ContentRuleApplyFailure> {
    match rules.payload() {
        ContentRulesPayload::AllowAll => Ok(NativeContentPolicy::AllowAll),
        ContentRulesPayload::Runtime(policy) => Ok(NativeContentPolicy::Runtime {
            digest: rules.digest(),
            policy: policy.clone(),
        }),
        ContentRulesPayload::Declarative { .. } => {
            Err(ContentRuleApplyFailure::UnsupportedArtifact)
        }
    }
}

fn run_web_resource_callback_fail_open(
    callback: impl FnOnce() -> windows_core::Result<()>,
) -> windows_core::Result<()> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(callback)).unwrap_or(Ok(()))
}

#[cfg(feature = "agentic-browser")]
pub(crate) fn install_on_view(
    view: &wry::WebView,
    policy: &NativeContentPolicy,
) -> Result<ContentPolicyRegistration, ContentRuleApplyFailure> {
    install_scoped_on_view(
        view,
        policy,
        &crate::platform::content_pause::ContentPause::default(),
    )
}

pub(crate) fn install_scoped_on_view(
    view: &wry::WebView,
    policy: &NativeContentPolicy,
    pause: &crate::platform::content_pause::ContentPause,
) -> Result<ContentPolicyRegistration, ContentRuleApplyFailure> {
    let NativeContentPolicy::Runtime { policy, .. } = policy else {
        return Ok(ContentPolicyRegistration::allow_all());
    };
    let controller = view.controller();
    let core = unsafe { controller.CoreWebView2() }
        .map_err(|_| ContentRuleApplyFailure::NativeInstallation)?;
    let core22 = core
        .cast::<ICoreWebView2_22>()
        .map_err(|_| ContentRuleApplyFailure::NativeInstallation)?;
    let callback_policy = policy.clone();
    // Empty, immutable and stream-free: the same response can be installed on
    // every blocked request in this apartment. Construct it transactionally
    // before registering the cohort, rather than allocating a COM response and
    // reparsing identical headers for every ad. No body stream can be consumed
    // or rewound and no page/request data is retained by this object.
    let blocked_response = unsafe {
        view.environment().CreateWebResourceResponse(
            None,
            403,
            windows_core::w!("Blocked"),
            windows_core::w!("Cache-Control: no-store\r\nContent-Length: 0"),
        )
    }
    .map_err(|_| ContentRuleApplyFailure::NativeInstallation)?;
    let paused = pause.signal();
    let statistics = pause.statistics().cloned();
    let handler = WebResourceRequestedEventHandler::create(Box::new(move |_sender, args| {
        #[cfg(test)]
        let mut sample = qualification::CallbackSample::start();
        let result = run_web_resource_callback_fail_open(|| {
            if paused.load(std::sync::atomic::Ordering::Relaxed) {
                #[cfg(test)]
                sample.outcome("paused");
                return Ok(());
            }
            // Callback errors, missing optional native values, oversized page
            // inputs, and unwind-enabled Rust panics all fail open. The shipped
            // release profile remains intentionally abort-on-panic. This path
            // is synchronous and deliberately performs no deferral, dispatch,
            // storage, filesystem or network I/O.
            let Some(args) = args else {
                return Ok(());
            };
            match decide_observed_request(&args, callback_policy.as_ref()) {
                Some(NetworkDecision::Block) => {}
                Some(NetworkDecision::Allow) => {
                    #[cfg(test)]
                    sample.outcome("allowed");
                    return Ok(());
                }
                None => return Ok(()),
            }
            if paused.load(std::sync::atomic::Ordering::Relaxed) {
                return Ok(());
            }
            let installed = unsafe { args.SetResponse(&blocked_response) }.is_ok();
            crate::platform::content_pause::record_installed_block(statistics.as_ref(), installed);
            #[cfg(test)]
            sample.outcome(if installed {
                "blocked"
            } else {
                "response_failed"
            });
            Ok(())
        });
        // The test-only guard covers all synchronous callback work, including
        // extraction, matching, SetResponse, statistics and panic containment.
        // Response creation now belongs to installation, outside the callback.
        // No clocks or sample buffers are compiled into shipping callbacks.
        #[cfg(test)]
        sample.finish();
        result
    }));

    let mut registration = ContentPolicyRegistration {
        core: Some(core.clone()),
        core22: Some(core22.clone()),
        token: None,
        installed_filters: Rc::default(),
        retired: Rc::default(),
        pause: None,
    };
    // A view paused from the start installs its filters only when resumed.
    let wanted = if pause.paused() {
        0
    } else {
        FILTER_REGISTRATION_COUNT
    };
    for index in 0..wanted {
        let Some((pattern, context)) = filter_registration(index) else {
            return Err(match registration.cleanup() {
                Ok(()) => ContentRuleApplyFailure::NativeInstallation,
                Err(_) => ContentRuleApplyFailure::NativeCleanup,
            });
        };
        if unsafe {
            core22.AddWebResourceRequestedFilterWithRequestSourceKinds(
                pattern,
                context,
                FILTER_SOURCE_KINDS,
            )
        }
        .is_err()
        {
            return Err(match registration.cleanup() {
                Ok(()) => ContentRuleApplyFailure::NativeInstallation,
                Err(_) => ContentRuleApplyFailure::NativeCleanup,
            });
        }
        registration.installed_filters.set(index + 1);
    }
    let mut token = 0;
    if unsafe { core.add_WebResourceRequested(&handler, &mut token) }.is_err() {
        return Err(match registration.cleanup() {
            Ok(()) => ContentRuleApplyFailure::NativeInstallation,
            Err(_) => ContentRuleApplyFailure::NativeCleanup,
        });
    }
    registration.token = Some(token);
    let (installed, retired, filters) = (
        registration.installed_filters.clone(),
        registration.retired.clone(),
        core22,
    );
    let (live, busy) = (pause.signal(), Cell::new(false));
    registration.pause = pause.register(move |_| {
        let paused = || live.load(std::sync::atomic::Ordering::Relaxed);
        let follow = Follow {
            installed: &installed,
            retired: &retired,
            paused: &paused,
        };
        follow_pause(&filters, &follow, &busy);
    });
    Ok(registration)
}

/// One view's cohort of request filters, as pausing and resuming change it.
trait FilterCohort {
    fn add(&self, index: usize) -> windows_core::Result<()>;
    fn remove(&self, index: usize) -> windows_core::Result<()>;
}

impl FilterCohort for ICoreWebView2_22 {
    fn add(&self, index: usize) -> windows_core::Result<()> {
        let (pattern, context) =
            filter_registration(index).ok_or_else(|| windows_core::Error::from(E_INVALIDARG))?;
        // SAFETY: `self` is a live interface held by the registration on the
        // UI thread that created it, and the pattern is a static wide string.
        unsafe {
            self.AddWebResourceRequestedFilterWithRequestSourceKinds(
                pattern,
                context,
                FILTER_SOURCE_KINDS,
            )
        }
    }

    fn remove(&self, index: usize) -> windows_core::Result<()> {
        let (pattern, context) =
            filter_registration(index).ok_or_else(|| windows_core::Error::from(E_INVALIDARG))?;
        // SAFETY: as in `add`; the tuple is one this cohort added.
        unsafe {
            self.RemoveWebResourceRequestedFilterWithRequestSourceKinds(
                pattern,
                context,
                FILTER_SOURCE_KINDS,
            )
        }
    }
}

/// How many times a resume tries to install a cohort whole: the first time,
/// then once more after removing what the first left.
const RESUME_TRIES: usize = 2;
/// How many times one pause change follows a pause that keeps changing while
/// its filters are being changed.
const MAX_FOLLOWS: usize = 4;

/// What a pause change needs of its cohort's live state.
struct Follow<'a> {
    installed: &'a Cell<usize>,
    /// Set first by the registration's cleanup, which may run in a message
    /// pumped from inside one of this cohort's native calls.
    retired: &'a Cell<bool>,
    paused: &'a dyn Fn() -> bool,
}

struct Busy<'a>(&'a Cell<bool>);

impl Drop for Busy<'_> {
    fn drop(&mut self) {
        self.0.set(false);
    }
}

/// Pausing removes the cohort's filters, so a paused view's requests never
/// wait on this thread; resuming installs them again. A filter missing on a
/// resumed view would let its requests through unseen, so a cohort is meant to
/// be whole or empty: a resume that cannot install every filter tries again,
/// and after that removes them all, leaving the view unfiltered as if paused
/// until its next resume. Only filters that cannot be removed either are left
/// in place, and the log says how many. A pause that cannot remove a filter
/// leaves it to the callback's own pause check.
///
/// A native call can pump messages, and the pause can change again inside it.
/// That nested change only finds the cohort busy; this call reads the live
/// pause after every native call and follows wherever it ends up. A cleanup
/// pumped the same way retires the cohort: its filters are then the
/// cleanup's, and this call only takes back one it added meanwhile.
fn follow_pause(cohort: &impl FilterCohort, follow: &Follow<'_>, busy: &Cell<bool>) {
    if follow.retired.get() || busy.replace(true) {
        return;
    }
    let _busy = Busy(busy);
    for _ in 0..MAX_FOLLOWS {
        let target = (follow.paused)();
        let settled = || !follow.retired.get() && (follow.paused)() == target;
        if target {
            if let Err(error) = remove_filters(cohort, follow, &settled) {
                eprintln!("content blocker: WebView2 filter removal on pause failed: {error}");
            }
        } else {
            resume(cohort, follow, &settled);
        }
        if follow.retired.get() || settled() {
            return;
        }
    }
    let paused = (follow.paused)();
    eprintln!(
        "content blocker: pause kept changing during a filter change; {} of {FILTER_REGISTRATION_COUNT} filters for a {} view",
        follow.installed.get(),
        if paused { "paused" } else { "resumed" }
    );
    let live = || !follow.retired.get();
    if paused {
        let _ = remove_filters(cohort, follow, &live);
    } else {
        resume(cohort, follow, &live);
    }
}

fn resume(cohort: &impl FilterCohort, follow: &Follow<'_>, settled: &dyn Fn() -> bool) {
    let installed = follow.installed;
    let mut failure = None;
    for _ in 0..RESUME_TRIES {
        match add_filters(cohort, follow, settled) {
            Ok(()) => return,
            Err(error) => failure = Some(error),
        }
        let _ = remove_filters(cohort, follow, settled);
        if !settled() {
            return;
        }
    }
    if installed.get() != 0 {
        let _ = remove_filters(cohort, follow, settled);
    }
    let Some(error) = failure else {
        return;
    };
    if installed.get() == 0 {
        eprintln!(
            "content blocker: WebView2 filters could not be installed on resume, so the view stays unfiltered until its next resume: {error}"
        );
    } else {
        eprintln!(
            "content blocker: WebView2 filters could not be installed on resume or removed after, so the view is partially filtered ({} of {FILTER_REGISTRATION_COUNT}) until its next resume: {error}",
            installed.get()
        );
    }
}

/// Adds the cohort's missing filters while the pause stays where it was. A
/// filter is counted only once added, so a cleanup pumped by the call leaves
/// it to this one, which takes it back.
fn add_filters(
    cohort: &impl FilterCohort,
    follow: &Follow<'_>,
    settled: &dyn Fn() -> bool,
) -> windows_core::Result<()> {
    let installed = follow.installed;
    while installed.get() < FILTER_REGISTRATION_COUNT && settled() {
        let index = installed.get();
        cohort.add(index)?;
        if follow.retired.get() {
            if let Err(error) = cohort.remove(index) {
                eprintln!("content blocker: a filter added while its view retired stays: {error}");
            }
            return Ok(());
        }
        installed.set(index + 1);
    }
    Ok(())
}

/// Removes the cohort's filters while the pause stays where it was. A filter
/// is uncounted before its call, so a cleanup pumped by the call never removes
/// it a second time.
fn remove_filters(
    cohort: &impl FilterCohort,
    follow: &Follow<'_>,
    settled: &dyn Fn() -> bool,
) -> windows_core::Result<()> {
    let installed = follow.installed;
    while installed.get() > 0 && settled() {
        let index = installed.get() - 1;
        installed.set(index);
        if let Err(error) = cohort.remove(index) {
            if follow.retired.get() {
                eprintln!("content blocker: a filter of a retired view stays: {error}");
            } else {
                installed.set(index + 1);
            }
            return Err(error);
        }
    }
    Ok(())
}

#[derive(Default)]
struct RequestBuffers {
    url: String,
    method: String,
}

fn decide_observed_request(
    args: &webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2WebResourceRequestedEventArgs,
    policy: &dyn NetworkRequestPolicy,
) -> Option<NetworkDecision> {
    REQUEST_BUFFERS
        .try_with(|buffers| {
            let mut buffers = buffers.try_borrow_mut().ok()?;
            let resource_type = extract_request_into(args, &mut buffers)?;
            let request = NetworkRequest::source_independent(
                &buffers.url,
                &buffers.method,
                resource_type,
                // The registration admits only the DOCUMENT source kind. Avoid a
                // redundant per-request interface cast and COM query on this
                // synchronous hot path.
                NetworkRequestSourceKind::Document,
            )?;
            #[cfg(test)]
            let started = std::time::Instant::now();
            let decision = policy.decide(&request);
            #[cfg(test)]
            qualification::record_matcher(started.elapsed());
            Some(decision)
        })
        .ok()
        .flatten()
}

fn extract_request_into(
    args: &webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2WebResourceRequestedEventArgs,
    buffers: &mut RequestBuffers,
) -> Option<NetworkResourceType> {
    let mut context = COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL;
    unsafe { args.ResourceContext(&mut context) }.ok()?;
    // Defend against a runtime violating the exact filter tuple before
    // allocating request strings across COM.
    let resource_type = resource_type(context)?;

    let native_request = unsafe { args.Request() }.ok()?;
    let mut uri = PWSTR::null();
    unsafe { native_request.Uri(&mut uri) }.ok()?;
    super::take_pwstr_bounded_into(
        uri,
        MAX_NETWORK_REQUEST_URL_BYTES,
        MAX_NETWORK_REQUEST_URL_BYTES,
        &mut buffers.url,
    )?;
    let mut method = PWSTR::null();
    unsafe { native_request.Method(&mut method) }.ok()?;
    super::take_pwstr_bounded_into(
        method,
        MAX_NETWORK_REQUEST_METHOD_BYTES,
        MAX_NETWORK_REQUEST_METHOD_BYTES,
        &mut buffers.method,
    )?;

    // WebView2 does not distinguish main documents, iframes, and some worker
    // scripts in DOCUMENT context, and collapses several other resource types
    // into OTHER. Supplying a fabricated type can overblock navigation. V1
    // therefore intercepts only contexts with exact native classifications;
    // compiler coverage reports the resulting resource- and source-kind
    // dimensions separately.
    Some(resource_type)
}

fn resource_type(context: COREWEBVIEW2_WEB_RESOURCE_CONTEXT) -> Option<NetworkResourceType> {
    match context {
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_STYLESHEET => Some(NetworkResourceType::Stylesheet),
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_IMAGE => Some(NetworkResourceType::Image),
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_MEDIA => Some(NetworkResourceType::Media),
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_FONT => Some(NetworkResourceType::Font),
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_SCRIPT => Some(NetworkResourceType::Script),
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_XML_HTTP_REQUEST => {
            Some(NetworkResourceType::XmlHttpRequest)
        }
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_FETCH => Some(NetworkResourceType::Fetch),
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_PING => Some(NetworkResourceType::Ping),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zephium_core::blocker::{ContentRuleCoverage, NetworkAttribution};

    /// A cohort whose adds fail at `fail_at` for the first `failures` attempts.
    struct FlakyCohort {
        fail_at: usize,
        failures: Cell<usize>,
        remove_failures: Cell<usize>,
    }

    impl FilterCohort for FlakyCohort {
        fn add(&self, index: usize) -> windows_core::Result<()> {
            if index == self.fail_at && self.failures.get() > 0 {
                self.failures.set(self.failures.get() - 1);
                return Err(E_INVALIDARG.into());
            }
            Ok(())
        }

        fn remove(&self, _index: usize) -> windows_core::Result<()> {
            if self.remove_failures.get() > 0 {
                self.remove_failures.set(self.remove_failures.get() - 1);
                return Err(E_INVALIDARG.into());
            }
            Ok(())
        }
    }

    fn flaky(fail_at: usize, failures: usize) -> FlakyCohort {
        FlakyCohort {
            fail_at,
            failures: Cell::new(failures),
            remove_failures: Cell::new(0),
        }
    }

    fn follow(cohort: &impl FilterCohort, installed: &Cell<usize>, paused: bool) {
        let follow = Follow {
            installed,
            retired: &Cell::new(false),
            paused: &|| paused,
        };
        follow_pause(cohort, &follow, &Cell::new(false));
    }

    /// A cohort whose native calls can change the pause, or retire the cohort
    /// as its cleanup would, from inside the call, the way a message pumped by
    /// a WebView2 call would. `filters` counts what WebView2 would hold.
    #[derive(Default)]
    struct Reentrant {
        calls: Cell<usize>,
        flip_at: Option<usize>,
        flip_every: bool,
        retire_at: Option<usize>,
        paused: Rc<Cell<bool>>,
        installed: Rc<Cell<usize>>,
        retired: Rc<Cell<bool>>,
        busy: Rc<Cell<bool>>,
        filters: Cell<isize>,
    }

    impl Reentrant {
        fn follow(&self) {
            let paused = self.paused.clone();
            let paused = move || paused.get();
            let follow = Follow {
                installed: &self.installed,
                retired: &self.retired,
                paused: &paused,
            };
            follow_pause(self, &follow, &self.busy);
        }

        fn call(&self) {
            self.calls.set(self.calls.get() + 1);
            if self.retire_at == Some(self.calls.get()) {
                // As the registration's cleanup: retired first, then every
                // counted filter removed.
                self.retired.set(true);
                while self.installed.get() > 0 {
                    self.installed.set(self.installed.get() - 1);
                    self.filters.set(self.filters.get() - 1);
                }
            }
            if self.flip_every || self.flip_at == Some(self.calls.get()) {
                self.paused.set(!self.paused.get());
                self.follow();
            }
        }
    }

    impl FilterCohort for Reentrant {
        fn add(&self, _index: usize) -> windows_core::Result<()> {
            self.call();
            self.filters.set(self.filters.get() + 1);
            Ok(())
        }

        fn remove(&self, _index: usize) -> windows_core::Result<()> {
            self.call();
            self.filters.set(self.filters.get() - 1);
            Ok(())
        }
    }

    fn reentrant(paused: bool, installed: usize) -> Reentrant {
        Reentrant {
            paused: Rc::new(Cell::new(paused)),
            installed: Rc::new(Cell::new(installed)),
            filters: Cell::new(installed as isize),
            ..Reentrant::default()
        }
    }

    #[test]
    fn a_pause_change_inside_a_native_call_is_followed_to_where_it_ends() {
        for (start_paused, start_installed) in [(true, FILTER_REGISTRATION_COUNT), (false, 0)] {
            let cohort = Reentrant {
                flip_at: Some(3),
                ..reentrant(start_paused, start_installed)
            };
            cohort.follow();
            assert_ne!(cohort.paused.get(), start_paused);
            let whole = if cohort.paused.get() {
                0
            } else {
                FILTER_REGISTRATION_COUNT
            };
            assert_eq!(cohort.installed.get(), whole);
            assert_eq!(cohort.filters.get(), whole as isize);
            assert!(!cohort.busy.get());
        }
    }

    #[test]
    fn a_cohort_retired_inside_a_native_call_is_left_with_no_filter() {
        for (start_paused, start_installed) in [(false, 0), (true, FILTER_REGISTRATION_COUNT)] {
            let cohort = Reentrant {
                retire_at: Some(3),
                ..reentrant(start_paused, start_installed)
            };
            cohort.follow();
            assert_eq!(cohort.installed.get(), 0);
            assert_eq!(cohort.filters.get(), 0, "nothing leaks or is removed twice");
            assert!(!cohort.busy.get());
            // A later pause change finds the cohort retired and adds nothing.
            cohort.paused.set(false);
            cohort.follow();
            assert_eq!(cohort.filters.get(), 0);
        }
    }

    #[test]
    fn a_pause_that_never_settles_still_leaves_the_cohort_whole_or_empty() {
        let cohort = Reentrant {
            flip_every: true,
            ..reentrant(false, 0)
        };
        cohort.follow();
        assert!(matches!(
            cohort.installed.get(),
            0 | FILTER_REGISTRATION_COUNT
        ));
        assert_eq!(cohort.filters.get(), cohort.installed.get() as isize);
        assert!(!cohort.busy.get());
    }

    #[test]
    fn a_resume_rebuilds_a_cohort_that_did_not_install_whole() {
        let installed = Cell::new(0);
        follow(&flaky(3, 1), &installed, false);
        assert_eq!(installed.get(), FILTER_REGISTRATION_COUNT);
    }

    #[test]
    fn a_resume_that_keeps_failing_leaves_no_filter_half_installed() {
        let installed = Cell::new(0);
        follow(&flaky(3, RESUME_TRIES), &installed, false);
        assert_eq!(installed.get(), 0);
        // The next resume tries again.
        follow(&flaky(3, 0), &installed, false);
        assert_eq!(installed.get(), FILTER_REGISTRATION_COUNT);
    }

    #[test]
    fn a_resume_whose_rollback_fails_still_ends_unfiltered_or_counts_what_stayed() {
        // Both rollbacks fail, the last removal succeeds: nothing is left.
        let installed = Cell::new(0);
        let late = FlakyCohort {
            remove_failures: Cell::new(RESUME_TRIES),
            ..flaky(3, RESUME_TRIES)
        };
        follow(&late, &installed, false);
        assert_eq!(installed.get(), 0);
        // Removals keep failing: the filters added before the failure stay,
        // and only those.
        let installed = Cell::new(0);
        let stuck = FlakyCohort {
            remove_failures: Cell::new(usize::MAX),
            ..flaky(3, RESUME_TRIES)
        };
        follow(&stuck, &installed, false);
        assert_eq!(installed.get(), 3);
    }

    #[test]
    fn a_pause_removes_every_filter_and_keeps_those_it_cannot() {
        let installed = Cell::new(FILTER_REGISTRATION_COUNT);
        follow(&flaky(0, 0), &installed, true);
        assert_eq!(installed.get(), 0);
        let installed = Cell::new(FILTER_REGISTRATION_COUNT);
        let stuck = FlakyCohort {
            remove_failures: Cell::new(usize::MAX),
            ..flaky(0, 0)
        };
        follow(&stuck, &installed, true);
        assert_eq!(installed.get(), FILTER_REGISTRATION_COUNT);
    }

    struct PanickingPolicy;

    impl NetworkRequestPolicy for PanickingPolicy {
        fn decide(&self, _request: &NetworkRequest<'_>) -> NetworkDecision {
            panic!("injected matcher panic");
        }
    }

    struct AllowingPolicy;

    impl NetworkRequestPolicy for AllowingPolicy {
        fn decide(&self, _request: &NetworkRequest<'_>) -> NetworkDecision {
            NetworkDecision::Allow
        }
    }

    fn runtime_rules(digest: [u8; 32]) -> Arc<ContentRules> {
        ContentRules::runtime(
            ContentRuleDigest::from_bytes(digest),
            ContentRuleCoverage {
                source_rules: 1,
                accepted_rules: 1,
                blocking_rule_entries: 1,
                ..ContentRuleCoverage::default()
            },
            Arc::new(AllowingPolicy),
        )
        .expect("test coverage must describe one blocking rule")
    }

    #[test]
    fn runtime_policy_equivalence_is_digest_exact_not_allocation_identity() {
        let first = prepare(&runtime_rules([1; 32])).expect("first policy");
        let same_bytes_new_allocation =
            prepare(&runtime_rules([1; 32])).expect("equivalent policy");
        let changed = prepare(&runtime_rules([2; 32])).expect("changed policy");

        assert!(same_policy(&first, &same_bytes_new_allocation));
        assert!(!same_policy(&first, &changed));
        assert!(!same_policy(&NativeContentPolicy::AllowAll, &first));
    }

    #[test]
    fn every_native_context_maps_without_panicking() {
        assert_eq!(
            resource_type(COREWEBVIEW2_WEB_RESOURCE_CONTEXT_SCRIPT),
            Some(NetworkResourceType::Script)
        );
        assert_eq!(
            resource_type(COREWEBVIEW2_WEB_RESOURCE_CONTEXT(usize::MAX as i32)),
            None
        );
        assert_eq!(
            resource_type(COREWEBVIEW2_WEB_RESOURCE_CONTEXT_DOCUMENT),
            None
        );
    }

    #[test]
    fn exactly_typed_requests_do_not_depend_on_top_level_source() {
        let request = NetworkRequest::source_independent(
            "https://cdn.example/script.js",
            "GET",
            NetworkResourceType::Script,
            NetworkRequestSourceKind::Document,
        )
        .expect("exactly typed request lost source-independent matching");
        assert_eq!(request.attribution(), NetworkAttribution::SourceIndependent);
        assert_eq!(request.source_url(), None);
    }

    #[test]
    fn explicit_allow_all_owns_no_handler_or_filter() {
        let registration = ContentPolicyRegistration::allow_all();
        assert!(registration.core.is_none());
        assert!(registration.core22.is_none());
        assert!(registration.token.is_none());
        assert_eq!(registration.installed_filters.get(), 0);
        assert!(registration.pause.is_none());
    }

    #[test]
    fn per_view_filters_never_multiply_shared_environment_worker_callbacks() {
        assert_eq!(
            FILTER_SOURCE_KINDS,
            COREWEBVIEW2_WEB_RESOURCE_REQUEST_SOURCE_KINDS_DOCUMENT
        );
        assert_eq!(
            FILTER_SOURCE_KINDS.0 & COREWEBVIEW2_WEB_RESOURCE_REQUEST_SOURCE_KINDS_SHARED_WORKER.0,
            0
        );
        assert_eq!(
            FILTER_SOURCE_KINDS.0 & COREWEBVIEW2_WEB_RESOURCE_REQUEST_SOURCE_KINDS_SERVICE_WORKER.0,
            0
        );
        let production = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/src/platform/windows/content_filter.rs"
        ));
        let production = production
            .split_once("#[cfg(test)]\nmod tests")
            .expect("test module boundary disappeared")
            .0;
        assert!(!production.contains("RequestedSourceKind("));
        assert!(!production.contains("ICoreWebView2WebResourceRequestedEventArgs2"));
    }

    #[test]
    fn native_filters_subscribe_only_to_exact_request_contexts() {
        assert_eq!(FILTER_REGISTRATION_COUNT, 16);
        for index in 0..FILTER_REGISTRATION_COUNT {
            let (_, context) = filter_registration(index).expect("filter tuple disappeared");
            assert!(resource_type(context).is_some());
            assert_ne!(context, COREWEBVIEW2_WEB_RESOURCE_CONTEXT_DOCUMENT);
        }
        assert!(filter_registration(FILTER_REGISTRATION_COUNT).is_none());
    }

    #[test]
    fn replacement_filter_cohort_uses_webview2s_documented_reference_count_contract() {
        // Microsoft specifies that adding an identical filter multiple times
        // requires the same number of removals before it becomes ineffective:
        // https://learn.microsoft.com/dotnet/api/microsoft.web.webview2.core.corewebview2.removewebresourcerequestedfilter
        //
        // EngineHost installs the complete replacement registration before
        // retiring the prior one. Pin both sides here: every registration owns
        // one exact add/remove per tuple, so retiring the old cohort leaves the
        // replacement cohort's reference effective.
        assert_eq!(FILTER_REGISTRATION_COUNT, 16);
        for index in 0..FILTER_REGISTRATION_COUNT {
            assert!(filter_registration(index).is_some());
        }
        let source = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/src/platform/windows/content_filter.rs"
        ));
        let installation = source
            .split_once("pub(crate) fn install_on_view(")
            .expect("content-policy installation disappeared")
            .1
            .split_once("struct RequestBuffers")
            .expect("content-policy installation boundary disappeared")
            .0;
        assert!(installation.contains("AddWebResourceRequestedFilterWithRequestSourceKinds"));
        let cleanup = source
            .split_once("fn cleanup(&mut self)")
            .expect("content-policy cleanup disappeared")
            .1
            .split_once("impl Drop for ContentPolicyRegistration")
            .expect("content-policy cleanup boundary disappeared")
            .0;
        assert!(cleanup.contains("RemoveWebResourceRequestedFilterWithRequestSourceKinds"));
    }

    #[test]
    fn unexpected_context_is_rejected_before_request_string_allocation() {
        let source = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/src/platform/windows/content_filter.rs"
        ));
        let extraction = source
            .split_once("fn extract_request_into(")
            .expect("request extraction disappeared")
            .1
            .split_once("fn resource_type(")
            .expect("request extraction boundary disappeared")
            .0;
        let context = extraction
            .find("args.ResourceContext")
            .expect("context admission disappeared");
        let request = extraction
            .find("args.Request()")
            .expect("request extraction disappeared");
        assert!(context < request);
    }

    #[test]
    fn synchronous_request_callback_never_defers_or_dispatches() {
        let source = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/src/platform/windows/content_filter.rs"
        ));
        let callback = source
            .split_once("let handler = WebResourceRequestedEventHandler::create")
            .expect("content-policy callback registration disappeared")
            .1
            .split_once("let mut registration = ContentPolicyRegistration")
            .expect("content-policy callback boundary disappeared")
            .0;
        for forbidden in [
            "GetDeferral",
            "with_priority",
            "try_with",
            "std::fs",
            "std::thread",
            "tokio",
            "unwrap(",
            "expect(",
        ] {
            assert!(
                !callback.contains(forbidden),
                "page-facing request callback acquired forbidden capability: {forbidden}"
            );
        }
    }

    #[test]
    fn synchronous_request_callback_contains_unwinding_matcher_panics_and_fails_open() {
        let request = NetworkRequest::source_independent(
            "https://cdn.example/script.js",
            "GET",
            NetworkResourceType::Script,
            NetworkRequestSourceKind::Document,
        )
        .expect("test request must be valid");
        let policy = PanickingPolicy;

        let result = run_web_resource_callback_fail_open(|| {
            let _ = policy.decide(&request);
            Ok(())
        });

        assert!(
            result.is_ok(),
            "an unwinding matcher panic must fail open at the COM ABI"
        );
    }
}
