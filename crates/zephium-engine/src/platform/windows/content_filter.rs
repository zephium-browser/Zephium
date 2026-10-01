use std::cell::RefCell;
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
    installed_filters: usize,
}

impl ContentPolicyRegistration {
    pub(crate) fn allow_all() -> Self {
        Self {
            core: None,
            core22: None,
            token: None,
            installed_filters: 0,
        }
    }

    pub(crate) fn retire(mut self) -> Result<(), ContentRuleApplyFailure> {
        self.cleanup()
    }

    fn cleanup(&mut self) -> Result<(), ContentRuleApplyFailure> {
        if self.core.is_none() && self.core22.is_none() {
            return if self.token.is_none() && self.installed_filters == 0 {
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
        while self.installed_filters != 0 {
            let index = self.installed_filters - 1;
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
            self.installed_filters = index;
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
        installed_filters: 0,
    };
    for index in 0..FILTER_REGISTRATION_COUNT {
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
        registration.installed_filters += 1;
    }
    let mut token = 0;
    if unsafe { core.add_WebResourceRequested(&handler, &mut token) }.is_err() {
        return Err(match registration.cleanup() {
            Ok(()) => ContentRuleApplyFailure::NativeInstallation,
            Err(_) => ContentRuleApplyFailure::NativeCleanup,
        });
    }
    registration.token = Some(token);
    Ok(registration)
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
        assert_eq!(registration.installed_filters, 0);
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
