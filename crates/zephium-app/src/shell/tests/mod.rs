use super::*;
use std::sync::Mutex;
use zephium_core::blocker::{
    BlockerConfig, BlockerConfigRevision, ContentPolicyFailure, ContentPolicyGeneration,
    ContentRuleCoverage, ContentRuleDigest, ContentRules, NetworkDecision, NetworkRequest,
    NetworkRequestPolicy, ProfileBlockerConfig,
};
use zephium_core::extensions::ExtensionActionRequest;
use zephium_core::ids::WindowId;
use zephium_core::ports::blocker::{
    BlockerCatalog, BlockerCatalogPhase, BlockerCatalogRefreshDispatch, BlockerCatalogSnapshot,
    BlockerCompileOutcome, BlockerCompiler, BlockerDispatch, BlockerRetirementDispatch,
    BlockerShutdownOutcome,
};
use zephium_core::ports::engine::{
    ContentScope, NavigationRequestId, UserContent, UserContentGeneration, ZoomRequestId,
};
use zephium_core::ports::store::{BlockerConfigLoadOutcome, BlockerConfigUpdateOutcome};
use zephium_core::ports::store::{
    PagePermissionCatalogLoadOutcome, PagePermissionCatalogMutationOutcome,
};
use zephium_core::session::{
    PersistedItem, PersistedKind, PersistedProfile, PersistedSpace, SessionState,
};

type HeldErasure = (ProfileId, Box<dyn FnOnce(ProfileDataErasureOutcome) + Send>);
type HeldBlockerUpdate = (
    ProfileId,
    BlockerConfigRevision,
    BlockerConfig,
    Box<dyn FnOnce(BlockerConfigUpdateOutcome) + Send>,
);
type HeldBlockerLoad = (ProfileId, Box<dyn FnOnce(BlockerConfigLoadOutcome) + Send>);
pub(crate) struct ImmediateAllowAllCompiler;

pub(super) fn test_catalog_snapshot() -> BlockerCatalogSnapshot {
    BlockerCatalogSnapshot {
        revision: 1,
        phase: BlockerCatalogPhase::Fresh,
        enabled_policy_terminal: false,
        package_revision: Some(1),
        package_manifest_sha256: Some([1; 32]),
        package_provenance: Some(
            zephium_core::ports::blocker::BlockerCatalogProvenance::TufRepository,
        ),
        package_created_unix: Some(1),
        package_expires_unix: Some(u64::MAX),
        package_stale: Some(false),
        source_refresh_due: false,
        source_count: Some(1),
        source_bytes: Some(1),
        candidate_revision: None,
        candidate_manifest_sha256: None,
        candidate_provenance: None,
        candidate_created_unix: None,
        candidate_expires_unix: None,
        candidate_source_count: None,
        candidate_source_bytes: None,
        installed_revision: Some(1),
        installed_manifest_sha256: Some([1; 32]),
        installed_provenance: Some(
            zephium_core::ports::blocker::BlockerCatalogProvenance::TufRepository,
        ),
        refresh_supported: true,
        source_material_epoch: 0,
        source_material_repair_pending: false,
        source_material_repair_retry_pending: false,
        activation_pending: false,
        repair_retry_pending: false,
        last_refresh_attempt_unix: Some(1),
        refresh_operation: None,
    }
}

struct ImmediateNetworkPolicy;

impl NetworkRequestPolicy for ImmediateNetworkPolicy {
    fn decide(&self, _request: &NetworkRequest<'_>) -> NetworkDecision {
        NetworkDecision::Allow
    }
}

impl BlockerCompiler for ImmediateAllowAllCompiler {
    fn validate_personal_selector(&self, selector: &str) -> Option<String> {
        (selector == ".banner").then(|| selector.to_owned())
    }
    fn prepare_site_preferences(
        &self,
        preferences: &zephium_core::blocker::BlockerSitePreferences,
    ) -> Option<Arc<zephium_core::blocker::PreparedBlockerSites>> {
        if preferences
            .hides()
            .iter()
            .any(|h| self.validate_personal_selector(&h.selector).is_none())
        {
            return None;
        }
        zephium_core::blocker::PreparedBlockerSites::new(
            preferences.revision(),
            preferences
                .paused_sites()
                .map(|site| (site.clone(), true, Arc::from(""))),
        )
    }

    fn compile(
        &self,
        _profile: ProfileId,
        _generation: ContentPolicyGeneration,
        config: BlockerConfig,
        done: Box<dyn FnOnce(BlockerCompileOutcome) + Send>,
    ) -> BlockerDispatch {
        let rules = if config.enabled {
            ContentRules::runtime(
                ContentRuleDigest::from_bytes([1; 32]),
                ContentRuleCoverage {
                    source_rules: 1,
                    accepted_rules: 1,
                    blocking_rule_entries: 1,
                    ..ContentRuleCoverage::default()
                },
                Arc::new(ImmediateNetworkPolicy),
            )
            .expect("test policy must be valid")
        } else {
            ContentRules::allow_all(ContentRuleDigest::from_bytes([0; 32]))
        };
        done(BlockerCompileOutcome::Compiled(rules));
        BlockerDispatch::Scheduled
    }

    fn retire_profile(
        &self,
        _profile: ProfileId,
        done: Box<dyn FnOnce() + Send>,
    ) -> BlockerRetirementDispatch {
        done();
        BlockerRetirementDispatch::Quiesced
    }

    fn shutdown_until(&self, _deadline: std::time::Instant) -> BlockerShutdownOutcome {
        BlockerShutdownOutcome::Clean
    }
}

impl BlockerCatalog for ImmediateAllowAllCompiler {
    fn maintain(&self) -> BlockerCatalogSnapshot {
        test_catalog_snapshot()
    }

    fn request_refresh(&self) -> BlockerCatalogRefreshDispatch {
        BlockerCatalogRefreshDispatch::Busy
    }
}

#[cfg(feature = "agentic-browser")]
#[derive(Default)]
pub(super) struct FakeAgentLifecycleState {
    pub(super) shutdown_calls: std::sync::atomic::AtomicUsize,
    pub(super) panic_on_shutdown: std::sync::atomic::AtomicBool,
    pub(super) dropped_without_shutdown: std::sync::atomic::AtomicBool,
    pub(super) clean: std::sync::atomic::AtomicBool,
    pub(super) shutdown_order: Mutex<Option<Arc<Mutex<Vec<&'static str>>>>>,
    pub(super) deadlines: Mutex<Vec<std::time::Instant>>,
}

#[cfg(feature = "agentic-browser")]
struct FakeAgentLifecycle {
    state: Arc<FakeAgentLifecycleState>,
}

#[cfg(feature = "agentic-browser")]
impl Drop for FakeAgentLifecycle {
    fn drop(&mut self) {
        if self
            .state
            .shutdown_calls
            .load(std::sync::atomic::Ordering::Acquire)
            == 0
        {
            self.state
                .dropped_without_shutdown
                .store(true, std::sync::atomic::Ordering::Release);
        }
    }
}

#[cfg(feature = "agentic-browser")]
impl zephium_agentic::AgentBrowserLifecycle for FakeAgentLifecycle {
    fn shutdown_until(
        self: Box<Self>,
        deadline: std::time::Instant,
    ) -> zephium_agentic::AgentBrowserShutdownOutcome {
        self.state
            .shutdown_calls
            .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
        self.state.deadlines.lock().unwrap().push(deadline);
        if let Some(order) = self.state.shutdown_order.lock().unwrap().as_ref() {
            order.lock().unwrap().push("agent");
        }
        assert!(
            !self
                .state
                .panic_on_shutdown
                .load(std::sync::atomic::Ordering::Acquire),
            "injected agent shutdown panic"
        );
        if self.state.clean.load(std::sync::atomic::Ordering::Acquire) {
            zephium_agentic::AgentBrowserShutdownOutcome::Clean(test_agent_native_shutdown_proof())
        } else {
            zephium_agentic::AgentBrowserShutdownOutcome::Unclean
        }
    }
}

#[cfg(feature = "agentic-browser")]
fn test_agent_native_shutdown_proof() -> zephium_agentic::AgentNativeShutdownProof {
    use zephium_agentic::{
        AgentNativeShutdownCoordinator, AgentNativeShutdownResources,
        ContextCookieTransferRegistry, ContextNativeResourceCounts, ContextNativeResourceSnapshot,
        ContextProfileLeaseRegistry, ContextRegistry, ContextResourceAuditId,
        ContextShutdownAuditSettlement, ContextShutdownDispatch,
        SemanticActionExecutionCoordinator, SemanticActionSettlementCoordinator,
        SemanticScreenshotCoordinator,
    };

    let mut contexts = ContextRegistry::new();
    contexts.seal_for_shutdown().expect("context seal");
    let mut profile_leases = ContextProfileLeaseRegistry::new();
    profile_leases
        .seal_for_shutdown()
        .expect("profile lease seal");
    let mut cookie_transfers = ContextCookieTransferRegistry::new();
    cookie_transfers
        .seal_for_shutdown()
        .expect("cookie transfer seal");
    let mut action_executions = SemanticActionExecutionCoordinator::new();
    action_executions.seal();
    let mut action_settlements = SemanticActionSettlementCoordinator::new();
    action_settlements.seal();
    let mut screenshots = SemanticScreenshotCoordinator::new();
    screenshots.seal_for_shutdown();
    let resources = AgentNativeShutdownResources::new(
        contexts,
        profile_leases,
        cookie_transfers,
        action_executions,
        action_settlements,
        screenshots,
    );
    let mut coordinator =
        AgentNativeShutdownCoordinator::try_new(resources).expect("logical drain");
    let audit = ContextResourceAuditId::new(1).expect("audit id");
    coordinator.begin_port_seal(audit).expect("begin seal");
    coordinator
        .account_port_seal(audit, ContextShutdownDispatch::AuditScheduled)
        .expect("account seal");
    let snapshot = ContextNativeResourceSnapshot::try_new(ContextNativeResourceCounts {
        known_bindings: 0,
        resident_views: 0,
        owned_reservations: 0,
        borrowed_leases: 0,
        visible_surfaces: 0,
        suspended_views: 0,
        pending_operations: 0,
        pending_captures: 0,
        queued_tasks: 0,
    })
    .expect("zero snapshot");
    coordinator
        .settle_shutdown_audit(ContextShutdownAuditSettlement::new(audit, Ok(snapshot)))
        .expect("zero settlement");
    coordinator.finish().expect("native zero proof")
}

#[cfg(feature = "agentic-browser")]
pub(super) fn agent_lifecycle_with_clean(
    clean: bool,
) -> (AgentLifecycle, Arc<FakeAgentLifecycleState>) {
    let state = Arc::new(FakeAgentLifecycleState::default());
    state
        .clean
        .store(clean, std::sync::atomic::Ordering::Release);
    (
        Box::new(FakeAgentLifecycle {
            state: Arc::clone(&state),
        }),
        state,
    )
}

type FirstUrlAfterReply = Option<(Arc<str>, NavigationRequestId)>;

#[derive(Default)]
pub(crate) struct FakeEngine {
    picker_result: Mutex<Option<zephium_core::blocker::ElementPickerResult>>,
    hold_picker: std::sync::atomic::AtomicBool,
    held_picker: Mutex<Option<zephium_core::blocker::ElementPickerCompletion>>,
    calls: Mutex<Vec<String>>,
    extension_browser_surfaces: Mutex<Vec<ExtensionBrowserSurface>>,
    extension_action_requests: Mutex<Vec<(ProfileId, ItemId, ExtensionBrowserSurfaceGeneration)>>,
    extension_action_invocations: Mutex<Vec<ExtensionActionRequest>>,
    extension_browser_settlements: Mutex<
        Vec<(
            ProfileId,
            ExtensionBrowserRequestId,
            ExtensionBrowserRequestSettlement,
        )>,
    >,
    extension_browser_first_urls: Mutex<Vec<FirstUrlAfterReply>>,
    page_permission_settlements: Mutex<
        Vec<(
            ProfileId,
            ItemId,
            zephium_core::permissions::PagePermissionRequestId,
            zephium_core::permissions::PagePermissionRequestSettlement,
        )>,
    >,
    warm_spare_calls: std::sync::atomic::AtomicUsize,
    media_capture_stops: Mutex<Vec<(ItemId, NavigationPresentationId)>>,
    navigation_requests: Mutex<Vec<NavigationRequestId>>,
    zoom_requests: Mutex<Vec<(ItemId, f64, ZoomRequestId)>>,
    shutdown_result: Mutex<Option<bool>>,
    shutdown_calls: std::sync::atomic::AtomicUsize,
    shutdown_order: Mutex<Option<Arc<Mutex<Vec<&'static str>>>>>,
    panic_on_shutdown: std::sync::atomic::AtomicBool,
    skip_shutdown_callback: std::sync::atomic::AtomicBool,
    reject_create_dispatch: std::sync::atomic::AtomicBool,
    reject_navigation_dispatch: std::sync::atomic::AtomicBool,
    reject_discard_dispatch: std::sync::atomic::AtomicBool,
    reject_native_dispatch: std::sync::atomic::AtomicBool,
    unsupported_presentation: std::sync::atomic::AtomicBool,
    runtime_restart_required: std::sync::atomic::AtomicBool,
    runtime_security_advisories: Mutex<zephium_core::runtime_security::RuntimeSecurityAdvisories>,
    erasure_outcomes: Mutex<VecDeque<ProfileDataErasureOutcome>>,
    erasure_requests: Mutex<Vec<ProfileId>>,
    held_erasures: Mutex<Vec<HeldErasure>>,
    hold_erasures: std::sync::atomic::AtomicBool,
    fills_window_for_fullscreen: std::sync::atomic::AtomicBool,
    fullscreen_exits: Mutex<Vec<ItemId>>,
    layout_regions: Mutex<Vec<Option<Rect>>>,
}

impl FakeEngine {
    fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
    fn log(&self, s: String) {
        self.calls.lock().unwrap().push(s);
    }
    fn warm_spare_calls(&self) -> usize {
        self.warm_spare_calls
            .load(std::sync::atomic::Ordering::Acquire)
    }
    fn last_layout(&self) -> Vec<String> {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .rev()
            .find_map(|c| {
                c.split_once(' ')
                    .filter(|(head, _)| head.starts_with("layout@"))
                    .map(|(_, rest)| rest)
            })
            .map(|s| {
                s.split(',')
                    .filter(|x| !x.is_empty())
                    .map(Into::into)
                    .collect()
            })
            .unwrap_or_default()
    }

    fn last_navigation_request(&self) -> NavigationRequestId {
        *self
            .navigation_requests
            .lock()
            .unwrap()
            .last()
            .expect("a navigation request must have been admitted")
    }

    fn last_zoom_request(&self) -> (ItemId, f64, ZoomRequestId) {
        *self
            .zoom_requests
            .lock()
            .unwrap()
            .last()
            .expect("a zoom request must have been admitted")
    }

    fn native_admission(&self) -> NativeDispatch {
        if self
            .reject_native_dispatch
            .load(std::sync::atomic::Ordering::Acquire)
        {
            NativeDispatch::Rejected
        } else {
            NativeDispatch::Scheduled
        }
    }

    fn extension_browser_surfaces(&self) -> Vec<ExtensionBrowserSurface> {
        self.extension_browser_surfaces.lock().unwrap().clone()
    }

    fn extension_action_requests(
        &self,
    ) -> Vec<(ProfileId, ItemId, ExtensionBrowserSurfaceGeneration)> {
        self.extension_action_requests.lock().unwrap().clone()
    }

    fn extension_action_invocations(&self) -> Vec<ExtensionActionRequest> {
        self.extension_action_invocations.lock().unwrap().clone()
    }

    fn extension_browser_settlements(
        &self,
    ) -> Vec<(
        ProfileId,
        ExtensionBrowserRequestId,
        ExtensionBrowserRequestSettlement,
    )> {
        self.extension_browser_settlements.lock().unwrap().clone()
    }

    fn extension_browser_first_urls(&self) -> Vec<FirstUrlAfterReply> {
        self.extension_browser_first_urls.lock().unwrap().clone()
    }

    fn page_permission_settlements(
        &self,
    ) -> Vec<(
        ProfileId,
        ItemId,
        zephium_core::permissions::PagePermissionRequestId,
        zephium_core::permissions::PagePermissionRequestSettlement,
    )> {
        self.page_permission_settlements.lock().unwrap().clone()
    }

    fn push_erasure_outcomes(&self, outcomes: impl IntoIterator<Item = ProfileDataErasureOutcome>) {
        self.erasure_outcomes.lock().unwrap().extend(outcomes);
    }

    fn erasure_requests(&self) -> Vec<ProfileId> {
        self.erasure_requests.lock().unwrap().clone()
    }

    fn complete_held_erasure(&self, outcome: ProfileDataErasureOutcome) {
        let (_, done) = self.held_erasures.lock().unwrap().remove(0);
        done(outcome);
    }
}

impl Engine for FakeEngine {
    fn set_focus_gate(&self, gate: Option<zephium_core::time::FocusGate>) {
        self.log(match gate {
            Some(gate) => format!("focus-gate {}", gate.shut.join(",")),
            None => "focus-gate open".to_owned(),
        });
    }
    fn set_media_suspended(&self, id: ItemId, suspended: bool) {
        self.log(format!(
            "media {id} {}",
            if suspended { "still" } else { "free" }
        ));
    }
    fn load_web_extension(
        &self,
        profile: ProfileId,
        load: zephium_core::ports::engine::WebExtensionLoad,
    ) -> NativeDispatch {
        self.log(format!("extension-load {profile} {}", load.install));
        NativeDispatch::Scheduled
    }
    fn unload_web_extension(
        &self,
        profile: ProfileId,
        install: zephium_core::ids::ExtensionInstallId,
    ) -> NativeDispatch {
        self.log(format!("extension-unload {profile} {install}"));
        NativeDispatch::Scheduled
    }
    fn remove_web_extension(
        &self,
        profile: ProfileId,
        load: zephium_core::ports::engine::WebExtensionLoad,
    ) -> NativeDispatch {
        self.log(format!("extension-remove {profile} {}", load.install));
        NativeDispatch::Scheduled
    }

    fn element_picker(
        &self,
        _profile: ProfileId,
        _id: ItemId,
        _site: zephium_core::blocker::BlockerSite,
        _request: zephium_core::blocker::ElementPickerRequest,
        completion: zephium_core::blocker::ElementPickerCompletion,
    ) {
        if self.hold_picker.load(std::sync::atomic::Ordering::Relaxed) {
            *self.held_picker.lock().unwrap() = Some(completion);
        } else {
            completion.finish(self.picker_result.lock().unwrap().clone());
        }
    }

    fn set_blocker_site_preferences(
        &self,
        _profile: ProfileId,
        _preferences: Arc<zephium_core::blocker::PreparedBlockerSites>,
        completion: zephium_core::ports::engine::BlockerSiteCompletion,
    ) {
        completion.finish(true);
    }
    fn runtime_restart_required(&self) -> bool {
        self.runtime_restart_required
            .load(std::sync::atomic::Ordering::Acquire)
    }

    fn runtime_security_advisories(
        &self,
    ) -> zephium_core::runtime_security::RuntimeSecurityAdvisories {
        *self.runtime_security_advisories.lock().unwrap()
    }

    fn create_view(&self, id: ItemId, partition: Partition, url: &str, _bounds: Rect) -> bool {
        if self
            .reject_create_dispatch
            .load(std::sync::atomic::Ordering::Acquire)
        {
            return false;
        }
        let kind = match partition {
            Partition::Default(_) => "default",
            Partition::Persistent(_) => "persistent",
            Partition::Ephemeral(_) => "ephemeral",
        };
        self.log(format!("create {id} {url} [{kind}]"));
        true
    }
    fn set_extension_browser_surface(&self, surface: ExtensionBrowserSurface) -> NativeDispatch {
        let admission = self.native_admission();
        if admission == NativeDispatch::Scheduled {
            self.log(format!(
                "extension-surface {} {}",
                surface.profile(),
                surface.generation().get()
            ));
            self.extension_browser_surfaces
                .lock()
                .unwrap()
                .push(surface);
        }
        admission
    }
    fn request_extension_actions(
        &self,
        profile: ProfileId,
        tab: ItemId,
        generation: ExtensionBrowserSurfaceGeneration,
    ) -> NativeDispatch {
        let admission = self.native_admission();
        if admission == NativeDispatch::Scheduled {
            self.extension_action_requests
                .lock()
                .unwrap()
                .push((profile, tab, generation));
        }
        admission
    }
    fn invoke_extension_action(&self, request: ExtensionActionRequest) -> NativeDispatch {
        let admission = self.native_admission();
        if admission == NativeDispatch::Scheduled {
            self.extension_action_invocations
                .lock()
                .unwrap()
                .push(request);
        }
        admission
    }
    fn settle_extension_browser_request(
        &self,
        profile: ProfileId,
        request: ExtensionBrowserRequestId,
        settlement: ExtensionBrowserRequestSettlement,
        first_url_after_reply: Option<(
            std::sync::Arc<str>,
            zephium_core::ports::engine::NavigationRequestId,
        )>,
    ) -> NativeDispatch {
        self.extension_browser_settlements
            .lock()
            .unwrap()
            .push((profile, request, settlement));
        self.extension_browser_first_urls
            .lock()
            .unwrap()
            .push(first_url_after_reply);
        self.native_admission()
    }
    fn settle_page_permission_request(
        &self,
        profile: ProfileId,
        item: ItemId,
        request: zephium_core::permissions::PagePermissionRequestId,
        settlement: zephium_core::permissions::PagePermissionRequestSettlement,
    ) -> NativeDispatch {
        self.page_permission_settlements
            .lock()
            .unwrap()
            .push((profile, item, request, settlement));
        self.native_admission()
    }
    fn fullscreen_presentation(&self) -> zephium_core::ports::engine::FullscreenPresentation {
        if self
            .fills_window_for_fullscreen
            .load(std::sync::atomic::Ordering::Acquire)
        {
            zephium_core::ports::engine::FullscreenPresentation::FillHostWindow
        } else {
            zephium_core::ports::engine::FullscreenPresentation::OwnWindow
        }
    }
    fn exit_fullscreen(&self, id: ItemId) -> NativeDispatch {
        self.fullscreen_exits.lock().unwrap().push(id);
        self.native_admission()
    }
    fn stop_media_capture(
        &self,
        item: ItemId,
        navigation: NavigationPresentationId,
    ) -> NativeDispatch {
        self.media_capture_stops
            .lock()
            .unwrap()
            .push((item, navigation));
        self.native_admission()
    }
    fn navigate(&self, id: ItemId, url: &str, request: NavigationRequestId) -> bool {
        if self
            .reject_navigation_dispatch
            .load(std::sync::atomic::Ordering::Acquire)
        {
            return false;
        }
        self.navigation_requests.lock().unwrap().push(request);
        self.log(format!("navigate {id} {url}"));
        true
    }
    fn present_navigation(
        &self,
        id: ItemId,
        navigation: NavigationPresentationId,
    ) -> NativeDispatch {
        self.log(format!("present {id} {}", navigation.into_raw()));
        if self
            .unsupported_presentation
            .load(std::sync::atomic::Ordering::Acquire)
        {
            NativeDispatch::Unsupported
        } else {
            self.native_admission()
        }
    }
    fn reload(&self, id: ItemId) -> NativeDispatch {
        self.log(format!("reload {id}"));
        self.native_admission()
    }
    fn stop(&self, _id: ItemId) -> NativeDispatch {
        self.native_admission()
    }
    fn go_back(&self, id: ItemId) -> NativeDispatch {
        self.log(format!("back {id}"));
        self.native_admission()
    }
    fn go_forward(&self, id: ItemId) -> NativeDispatch {
        self.log(format!("forward {id}"));
        self.native_admission()
    }
    fn close(&self, id: ItemId) -> NativeDispatch {
        self.log(format!("close {id}"));
        self.native_admission()
    }
    fn set_content(
        &self,
        window: WindowId,
        tree: Option<Pane>,
        region: Option<Rect>,
    ) -> NativeDispatch {
        self.layout_regions.lock().unwrap().push(region);
        let ids: Vec<String> = match (tree, region) {
            (Some(t), Some(_)) => t.tabs().iter().map(|id| id.to_string()).collect(),
            _ => Vec::new(),
        };
        self.log(format!("layout@{window} {}", ids.join(",")));
        self.native_admission()
    }
    fn set_drop_indicator(&self, _window: WindowId, _zone: Option<Rect>) -> NativeDispatch {
        self.native_admission()
    }
    fn hint_stage_motion(
        &self,
        window: WindowId,
        motion: zephium_core::ports::engine::StageMotion,
    ) -> NativeDispatch {
        self.log(format!("motion@{window} {motion:?}"));
        self.native_admission()
    }
    fn zoom(&self, id: ItemId, scale: f64, request: ZoomRequestId) -> NativeDispatch {
        self.log(format!("zoom {id} {scale}"));
        let admission = self.native_admission();
        if admission == NativeDispatch::Scheduled {
            self.zoom_requests
                .lock()
                .unwrap()
                .push((id, scale, request));
        }
        admission
    }
    fn set_muted(&self, _id: ItemId, _muted: bool) -> NativeDispatch {
        NativeDispatch::Unsupported
    }
    fn find(
        &self,
        id: ItemId,
        request: Option<zephium_core::ports::engine::FindRequest>,
    ) -> NativeDispatch {
        match request {
            Some(request) => self.log(format!(
                "find {id} {} {}",
                request.query,
                if request.forward { "next" } else { "previous" }
            )),
            None => self.log(format!("find {id} end")),
        }
        self.native_admission()
    }
    fn capture(&self, _id: ItemId) -> NativeDispatch {
        NativeDispatch::Unsupported
    }
    fn extract_html(&self, _id: ItemId) -> NativeDispatch {
        self.native_admission()
    }
    fn discover_favicon(&self, id: ItemId) -> NativeDispatch {
        self.log(format!("discover {id}"));
        self.native_admission()
    }
    fn probe_discard_safety(&self, id: ItemId, probe: DiscardProbeId) -> bool {
        self.log(format!("probe-discard {id} {}", probe.0));
        true
    }
    fn discard_view(&self, id: ItemId, probe: DiscardProbeId) -> bool {
        self.log(format!("discard {id} {}", probe.0));
        !self
            .reject_discard_dispatch
            .load(std::sync::atomic::Ordering::Acquire)
    }
    fn cancel_discard(&self, id: ItemId, probe: DiscardProbeId) {
        self.log(format!("cancel-discard {id} {}", probe.0));
    }
    fn forget_discarded_state(&self, profile: ProfileId, item: Option<ItemId>) {
        self.log(format!(
            "forget-discarded {profile} {}",
            item.map_or_else(|| "all".into(), |id| id.to_string())
        ));
    }
    fn set_dormant(&self, ids: Vec<ItemId>) {
        let mut ids: Vec<String> = ids.iter().map(ToString::to_string).collect();
        ids.sort();
        self.log(format!("dormant {}", ids.join(",")));
    }
    fn set_resize_guide(&self, window: WindowId, zone: Option<Rect>) -> NativeDispatch {
        self.log(format!("resize-guide@{window} {zone:?}"));
        self.native_admission()
    }
    fn print(&self, _id: ItemId) -> NativeDispatch {
        self.native_admission()
    }
    fn open_external_app(&self, url: &str) -> NativeDispatch {
        self.log(format!("open-app {url}"));
        self.native_admission()
    }
    fn set_user_content(
        &self,
        _scope: ContentScope,
        _generation: UserContentGeneration,
        _content: UserContent,
    ) -> NativeDispatch {
        self.native_admission()
    }
    fn set_shortcuts(&self, _shortcuts: Vec<zephium_core::ports::engine::Shortcut>) {}
    fn warm_spare(&self, _partition: Partition) {
        self.warm_spare_calls
            .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
    }
    fn install_content_rules(
        &self,
        profile: ProfileId,
        generation: ContentPolicyGeneration,
        _rules: Arc<ContentRules>,
    ) -> NativeDispatch {
        self.log(format!(
            "install-content-rules {profile} {}",
            generation.get()
        ));
        self.native_admission()
    }
    fn erase_profile_data(
        &self,
        profile: ProfileId,
        done: Box<dyn FnOnce(zephium_core::ports::engine::ProfileDataErasureOutcome) + Send>,
    ) {
        self.log(format!("erase-profile {profile}"));
        self.erasure_requests.lock().unwrap().push(profile);
        if self
            .hold_erasures
            .load(std::sync::atomic::Ordering::Acquire)
        {
            self.held_erasures.lock().unwrap().push((profile, done));
            return;
        }
        let outcome = self
            .erasure_outcomes
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(ProfileDataErasureOutcome::Failed);
        done(outcome);
    }
    fn shutdown(&self, done: Box<dyn FnOnce(bool) + Send>) {
        self.shutdown_calls
            .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
        if let Some(order) = self.shutdown_order.lock().unwrap().as_ref() {
            order.lock().unwrap().push("engine");
        }
        assert!(
            !self
                .panic_on_shutdown
                .load(std::sync::atomic::Ordering::Acquire),
            "injected engine shutdown panic"
        );
        if !self
            .skip_shutdown_callback
            .load(std::sync::atomic::Ordering::Acquire)
        {
            done(self.shutdown_result.lock().unwrap().unwrap_or(true));
        }
    }
}

type SiteUpdateCallback =
    Box<dyn FnOnce(zephium_core::ports::store::BlockerSiteUpdateOutcome) + Send>;

#[derive(Default)]
pub(crate) struct FakeStore {
    reject_time_clears: std::sync::atomic::AtomicBool,
    reject_focus_records: std::sync::atomic::AtomicBool,
    recorded_focus: Mutex<Vec<zephium_core::time::FocusRecord>>,
    pub(crate) statistics:
        Mutex<std::collections::HashMap<ProfileId, zephium_core::blocker::BlockerStatistics>>,
    pub(crate) statistics_writes: std::sync::atomic::AtomicUsize,
    saved: Mutex<Option<SessionState>>,
    events: Mutex<Vec<&'static str>>,
    flush_result: Mutex<Option<bool>>,
    shutdown_outcome: Mutex<Option<StoreShutdownOutcome>>,
    shutdown_calls: std::sync::atomic::AtomicUsize,
    shutdown_order: Mutex<Option<Arc<Mutex<Vec<&'static str>>>>>,
    barrier_order: Mutex<Option<Arc<Mutex<Vec<&'static str>>>>>,
    panic_on_save: std::sync::atomic::AtomicBool,
    panic_on_flush: std::sync::atomic::AtomicBool,
    panic_on_shutdown: std::sync::atomic::AtomicBool,
    load_failed: Mutex<bool>,
    recovery_reason: Mutex<Option<String>>,
    /// What `set_aside_session` restarts with; `None` cannot set aside.
    pub(crate) set_aside: Mutex<Option<SessionState>>,
    degraded_profiles: Mutex<Vec<ProfileId>>,
    site_preferences: Mutex<Arc<zephium_core::blocker::BlockerSitePreferences>>,
    site_store_calls: std::sync::atomic::AtomicUsize,
    site_updates_held: std::sync::atomic::AtomicBool,
    site_update_callbacks: Mutex<VecDeque<SiteUpdateCallback>>,
    blocker_configs: Mutex<Option<Vec<ProfileBlockerConfig>>>,
    blocker_update_outcomes: Mutex<VecDeque<BlockerConfigUpdateOutcome>>,
    blocker_load_outcomes: Mutex<VecDeque<BlockerConfigLoadOutcome>>,
    blocker_update_on_shutdown: Mutex<Option<BlockerConfigUpdateOutcome>>,
    blocker_load_calls: std::sync::atomic::AtomicUsize,
    blocker_load_after_shutdown_calls: std::sync::atomic::AtomicUsize,
    shutdown_entered: std::sync::atomic::AtomicBool,
    held_blocker_updates: Mutex<VecDeque<HeldBlockerUpdate>>,
    held_blocker_loads: Mutex<VecDeque<HeldBlockerLoad>>,
    hold_blocker_updates: std::sync::atomic::AtomicBool,
    hold_blocker_loads: std::sync::atomic::AtomicBool,
    reject_blocker_updates: std::sync::atomic::AtomicBool,
    reject_blocker_loads: std::sync::atomic::AtomicBool,
    panic_on_load: std::sync::atomic::AtomicBool,
    load_session_calls: std::sync::atomic::AtomicUsize,
    pending_deletion_load_calls: std::sync::atomic::AtomicUsize,
    history: Vec<zephium_core::ports::store::HistoryHit>,
    history_delay_ms: std::sync::atomic::AtomicU64,
    history_started: std::sync::atomic::AtomicBool,
    visits: Mutex<Vec<String>>,
    recorded_visits: Mutex<Vec<zephium_core::ports::store::HistoryVisit>>,
    pub(super) recorded_time: Mutex<Vec<(ProfileId, Vec<zephium_core::time::HourTally>)>>,
    icon_ages: Mutex<std::collections::HashMap<String, i64>>,
    icons: Mutex<Vec<(String, Vec<u8>)>>,
    pub(super) reject_settings: std::sync::atomic::AtomicBool,
    settings: Mutex<std::collections::HashMap<String, String>>,
    work_disabled: std::sync::atomic::AtomicBool,
    pending_deletions: Mutex<Vec<PendingProfileDeletion>>,
    pending_load_failures: std::sync::atomic::AtomicUsize,
    authorize_outcomes: Mutex<VecDeque<ProfileDeletionAuthorizeOutcome>>,
    authorize_unknown_commits: std::sync::atomic::AtomicBool,
    authorized_sessions: Mutex<Vec<(ProfileId, SessionState)>>,
    finalize_outcomes: Mutex<VecDeque<ProfileDeletionFinalizeOutcome>>,
    finalize_unknown_completes: std::sync::atomic::AtomicBool,
    page_permission_load_outcomes: Mutex<VecDeque<PagePermissionCatalogLoadOutcome>>,
    page_permission_mutation_outcomes: Mutex<VecDeque<PagePermissionCatalogMutationOutcome>>,
    page_permission_load_calls: Mutex<Vec<ProfileId>>,
    page_permission_mutation_calls: Mutex<
        Vec<(
            ProfileId,
            zephium_core::permissions::PagePermissionCatalogRevision,
            zephium_core::permissions::PagePermissionPatch,
        )>,
    >,
}

impl FakeStore {
    fn complete_blocker_update(&self, outcome: BlockerConfigUpdateOutcome) {
        let (_, _, _, done) = self
            .held_blocker_updates
            .lock()
            .unwrap()
            .pop_front()
            .expect("one blocker update must be pending");
        done(outcome);
    }

    fn complete_blocker_load(&self, outcome: BlockerConfigLoadOutcome) {
        let (_, done) = self
            .held_blocker_loads
            .lock()
            .unwrap()
            .pop_front()
            .expect("one blocker reconciliation must be pending");
        done(outcome);
    }
}

impl Store for FakeStore {
    fn load_page_permission_catalog(
        &self,
        profile: ProfileId,
        done: Box<dyn FnOnce(PagePermissionCatalogLoadOutcome) + Send>,
    ) -> bool {
        self.page_permission_load_calls
            .lock()
            .unwrap()
            .push(profile);
        let outcome = self
            .page_permission_load_outcomes
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(PagePermissionCatalogLoadOutcome::Failed);
        done(outcome);
        true
    }

    fn mutate_page_permission_catalog(
        &self,
        profile: ProfileId,
        expected: zephium_core::permissions::PagePermissionCatalogRevision,
        patch: zephium_core::permissions::PagePermissionPatch,
        done: Box<dyn FnOnce(PagePermissionCatalogMutationOutcome) + Send>,
    ) -> bool {
        self.page_permission_mutation_calls
            .lock()
            .unwrap()
            .push((profile, expected, patch));
        let outcome = self
            .page_permission_mutation_outcomes
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(PagePermissionCatalogMutationOutcome::Failed);
        done(outcome);
        true
    }

    fn save_session(&self, session: SessionState) {
        if let Some(order) = self.barrier_order.lock().unwrap().as_ref() {
            order.lock().unwrap().push("persist");
        }
        assert!(
            !self
                .panic_on_save
                .load(std::sync::atomic::Ordering::Acquire),
            "injected store save panic"
        );
        *self.saved.lock().unwrap() = Some(session);
        self.events.lock().unwrap().push("save");
    }
    fn flush(&self) -> bool {
        if let Some(order) = self.barrier_order.lock().unwrap().as_ref() {
            order.lock().unwrap().push("store-preflight");
        }
        assert!(
            !self
                .panic_on_flush
                .load(std::sync::atomic::Ordering::Acquire),
            "injected store flush panic"
        );
        self.events.lock().unwrap().push("flush");
        self.flush_result.lock().unwrap().unwrap_or(true)
    }
    fn shutdown_until(&self, deadline: std::time::Instant) -> StoreShutdownOutcome {
        self.shutdown_entered
            .store(true, std::sync::atomic::Ordering::Release);
        self.shutdown_calls
            .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
        if let Some(order) = self.barrier_order.lock().unwrap().as_ref() {
            order.lock().unwrap().push("store-final");
        }
        if let Some(order) = self.shutdown_order.lock().unwrap().as_ref() {
            order.lock().unwrap().push("store");
        }
        assert!(
            !self
                .panic_on_shutdown
                .load(std::sync::atomic::Ordering::Acquire),
            "injected store shutdown panic"
        );
        let late_blocker_update = self.blocker_update_on_shutdown.lock().unwrap().take();
        if let Some(outcome) = late_blocker_update {
            self.complete_blocker_update(outcome);
        }
        self.shutdown_outcome.lock().unwrap().unwrap_or_else(|| {
            if self.flush_until(deadline) {
                StoreShutdownOutcome::Clean
            } else {
                StoreShutdownOutcome::RetryableFailure
            }
        })
    }
    fn set_aside_session(&self) -> Option<(SessionLoad, Option<std::path::PathBuf>)> {
        let state = self.set_aside.lock().unwrap().take()?;
        *self.recovery_reason.lock().unwrap() = None;
        *self.saved.lock().unwrap() = Some(state);
        Some((self.load_session(), None))
    }

    fn load_session(&self) -> SessionLoad {
        self.load_session_calls
            .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
        assert!(
            !self
                .panic_on_load
                .load(std::sync::atomic::Ordering::Acquire),
            "injected store panic"
        );
        if *self.load_failed.lock().unwrap() {
            return SessionLoad::Failed;
        }
        if let Some(reason) = self.recovery_reason.lock().unwrap().clone() {
            return SessionLoad::RecoveryRequired { reason };
        }
        let Some(state) = self.saved.lock().unwrap().clone() else {
            return SessionLoad::Absent;
        };
        let profiles = self.degraded_profiles.lock().unwrap().clone();
        let blocker_configs = self
            .blocker_configs
            .lock()
            .unwrap()
            .clone()
            .unwrap_or_else(|| {
                state
                    .profiles
                    .iter()
                    .map(|profile| ProfileBlockerConfig {
                        profile: profile.id,
                        revision: BlockerConfigRevision::INITIAL,
                        config: BlockerConfig::default(),
                    })
                    .collect()
            });
        if profiles.is_empty() {
            SessionLoad::Loaded {
                state,
                blocker_configs,
            }
        } else {
            SessionLoad::LoadedWithDegradedProfiles {
                state,
                profiles,
                blocker_configs,
            }
        }
    }
    fn update_profile_blocker_config(
        &self,
        profile: ProfileId,
        expected: BlockerConfigRevision,
        next: BlockerConfig,
        done: Box<dyn FnOnce(BlockerConfigUpdateOutcome) + Send>,
    ) -> bool {
        if self
            .reject_blocker_updates
            .load(std::sync::atomic::Ordering::Acquire)
        {
            return false;
        }
        if self
            .hold_blocker_updates
            .load(std::sync::atomic::Ordering::Acquire)
        {
            self.held_blocker_updates
                .lock()
                .unwrap()
                .push_back((profile, expected, next, done));
            return true;
        }
        let outcome = self
            .blocker_update_outcomes
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(|| {
                BlockerConfigUpdateOutcome::Updated(ProfileBlockerConfig {
                    profile,
                    revision: expected.next().expect("test revision must advance"),
                    config: next,
                })
            });
        done(outcome);
        true
    }
    fn load_profile_blocker_config(
        &self,
        profile: ProfileId,
        done: Box<dyn FnOnce(BlockerConfigLoadOutcome) + Send>,
    ) -> bool {
        self.blocker_load_calls
            .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
        if self
            .shutdown_entered
            .load(std::sync::atomic::Ordering::Acquire)
        {
            self.blocker_load_after_shutdown_calls
                .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
        }
        if self
            .reject_blocker_loads
            .load(std::sync::atomic::Ordering::Acquire)
        {
            return false;
        }
        if self
            .hold_blocker_loads
            .load(std::sync::atomic::Ordering::Acquire)
        {
            self.held_blocker_loads
                .lock()
                .unwrap()
                .push_back((profile, done));
            return true;
        }
        let outcome = self
            .blocker_load_outcomes
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(BlockerConfigLoadOutcome::Failed);
        done(outcome);
        true
    }
    fn load_profile_blocker_sites(
        &self,
        _profile: ProfileId,
        done: Box<dyn FnOnce(zephium_core::ports::store::BlockerSiteLoadOutcome) + Send>,
    ) -> bool {
        self.site_store_calls
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        done(zephium_core::ports::store::BlockerSiteLoadOutcome::Loaded(
            self.site_preferences.lock().unwrap().clone(),
        ));
        true
    }
    fn update_profile_blocker_sites(
        &self,
        _profile: ProfileId,
        revision: u64,
        next: Arc<zephium_core::blocker::BlockerSitePreferences>,
        done: Box<dyn FnOnce(zephium_core::ports::store::BlockerSiteUpdateOutcome) + Send>,
    ) -> bool {
        use zephium_core::ports::store::BlockerSiteUpdateOutcome;
        self.site_store_calls
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let mut stored = self.site_preferences.lock().unwrap();
        let result = if stored.revision() == revision {
            *stored = next.clone();
            BlockerSiteUpdateOutcome::Updated(next)
        } else {
            BlockerSiteUpdateOutcome::Conflict(stored.clone())
        };
        drop(stored);
        if self
            .site_updates_held
            .load(std::sync::atomic::Ordering::Relaxed)
        {
            self.site_update_callbacks.lock().unwrap().push_back(done);
        } else {
            done(result);
        }
        true
    }
    fn record_time(
        &self,
        profile: ProfileId,
        tallies: Vec<zephium_core::time::HourTally>,
        _keep_from_hour: i64,
    ) -> bool {
        self.recorded_time.lock().unwrap().push((profile, tallies));
        true
    }

    fn clear_time(&self, _profile: ProfileId, _since_hour: Option<i64>) -> bool {
        !self
            .reject_time_clears
            .load(std::sync::atomic::Ordering::Acquire)
    }

    fn record_focus(&self, record: zephium_core::time::FocusRecord, _day: i64) -> bool {
        if self
            .reject_focus_records
            .load(std::sync::atomic::Ordering::Acquire)
        {
            return false;
        }
        self.recorded_focus.lock().unwrap().push(record);
        true
    }

    fn record_visit(&self, _profile: ProfileId, url: String, title: String) {
        let mut visits = self.recorded_visits.lock().unwrap();
        let id = i64::try_from(visits.len()).unwrap_or(i64::MAX) + 1;
        visits.push(zephium_core::ports::store::HistoryVisit {
            id,
            url: url.clone(),
            title,
            visited_at: id,
        });
        self.visits.lock().unwrap().push(url);
    }
    fn app_setting(&self, key: &str) -> Option<String> {
        if key == "work.enabled"
            && self
                .work_disabled
                .load(std::sync::atomic::Ordering::Acquire)
        {
            return Some("false".into());
        }
        self.settings.lock().unwrap().get(key).cloned()
    }
    fn set_app_setting(&self, key: String, value: String) -> bool {
        if self
            .reject_settings
            .load(std::sync::atomic::Ordering::Acquire)
        {
            return false;
        }
        self.settings.lock().unwrap().insert(key, value);
        true
    }
    fn search_history(
        &self,
        _profile: ProfileId,
        _query: &str,
        _limit: u32,
    ) -> Vec<zephium_core::ports::store::HistoryHit> {
        self.history_started
            .store(true, std::sync::atomic::Ordering::Release);
        let delay = self
            .history_delay_ms
            .load(std::sync::atomic::Ordering::Acquire);
        if delay != 0 {
            std::thread::sleep(std::time::Duration::from_millis(delay));
        }
        self.history.clone()
    }
    fn history_page(
        &self,
        _profile: ProfileId,
        query: &str,
        since: Option<i64>,
        before: Option<i64>,
        limit: u32,
    ) -> Vec<zephium_core::ports::store::HistoryVisit> {
        let needle = query.trim().to_lowercase();
        self.recorded_visits
            .lock()
            .unwrap()
            .iter()
            .rev()
            .filter(|visit| before.is_none_or(|cursor| visit.id < cursor))
            .filter(|visit| since.is_none_or(|floor| visit.visited_at >= floor))
            .filter(|visit| {
                needle.is_empty()
                    || visit.title.to_lowercase().contains(&needle)
                    || visit.url.to_lowercase().contains(&needle)
            })
            .take(limit as usize)
            .cloned()
            .collect()
    }

    fn forget_history_urls(&self, _profile: ProfileId, urls: &[String]) -> u32 {
        let mut visits = self.recorded_visits.lock().unwrap();
        let before = visits.len();
        visits.retain(|visit| !urls.contains(&visit.url));
        u32::try_from(before - visits.len()).unwrap_or(u32::MAX)
    }

    fn load_blocker_statistics(
        &self,
        profile: ProfileId,
        done: Box<dyn FnOnce(Option<zephium_core::blocker::BlockerStatistics>) + Send>,
    ) -> bool {
        done(Some(
            self.statistics
                .lock()
                .unwrap()
                .get(&profile)
                .cloned()
                .unwrap_or_default(),
        ));
        true
    }
    fn save_blocker_statistics(
        &self,
        profile: ProfileId,
        statistics: zephium_core::blocker::BlockerStatistics,
        done: Box<dyn FnOnce(bool) + Send>,
    ) -> bool {
        self.statistics_writes
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.statistics.lock().unwrap().insert(profile, statistics);
        done(true);
        true
    }
    fn clear_history(&self, _profile: ProfileId, since: Option<i64>) -> u32 {
        let mut visits = self.recorded_visits.lock().unwrap();
        let before = visits.len();
        match since {
            Some(since) => visits.retain(|visit| visit.visited_at < since),
            None => visits.clear(),
        }
        u32::try_from(before - visits.len()).unwrap_or(u32::MAX)
    }

    fn amend_visit_title(&self, _profile: ProfileId, url: String, title: String) -> bool {
        let mut visits = self.recorded_visits.lock().unwrap();
        if let Some(visit) = visits.iter_mut().rev().find(|visit| visit.url == url) {
            visit.title = title;
        }
        true
    }

    fn favicon_age(&self, _profile: ProfileId, origin: &str) -> Option<i64> {
        self.icon_ages.lock().unwrap().get(origin).copied()
    }
    fn save_favicon(
        &self,
        _profile: ProfileId,
        origin: String,
        _content_type: Option<String>,
        bytes: Vec<u8>,
    ) {
        self.icons.lock().unwrap().push((origin, bytes));
    }
    fn favicon_bytes(
        &self,
        _profile: ProfileId,
        origin: &str,
    ) -> Option<(Option<String>, Vec<u8>)> {
        self.icons
            .lock()
            .unwrap()
            .iter()
            .rev()
            .find(|(stored_origin, _)| stored_origin == origin)
            .map(|(_, bytes)| {
                (
                    Some(zephium_core::icon::RGBA32_MIME.to_owned()),
                    bytes.clone(),
                )
            })
    }
    fn favicon_raster_with_age(&self, profile: ProfileId, origin: &str) -> Option<(Vec<u8>, i64)> {
        let age = self.favicon_age(profile, origin)?;
        self.favicon_bytes(profile, origin)
            .map(|(_, bytes)| (bytes, age))
    }
    fn pending_profile_deletions(&self) -> ProfileDeletionLoad {
        self.pending_deletion_load_calls
            .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
        if self
            .pending_load_failures
            .fetch_update(
                std::sync::atomic::Ordering::AcqRel,
                std::sync::atomic::Ordering::Acquire,
                |remaining| remaining.checked_sub(1),
            )
            .is_ok()
        {
            return ProfileDeletionLoad::Failed;
        }
        ProfileDeletionLoad::Loaded(self.pending_deletions.lock().unwrap().clone())
    }
    fn authorize_profile_deletion(
        &self,
        profile: ProfileId,
        filtered_session: SessionState,
        _deadline: std::time::Instant,
    ) -> ProfileDeletionAuthorizeOutcome {
        self.events.lock().unwrap().push("authorize-delete");
        self.authorized_sessions
            .lock()
            .unwrap()
            .push((profile, filtered_session.clone()));
        let outcome = self
            .authorize_outcomes
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(ProfileDeletionAuthorizeOutcome::NotRegistered);
        if matches!(
            outcome,
            ProfileDeletionAuthorizeOutcome::Authorized
                | ProfileDeletionAuthorizeOutcome::AlreadyAuthorized
        ) || (outcome == ProfileDeletionAuthorizeOutcome::OutcomeUnknown
            && self
                .authorize_unknown_commits
                .load(std::sync::atomic::Ordering::Acquire))
        {
            *self.saved.lock().unwrap() = Some(filtered_session);
            let mut pending = self.pending_deletions.lock().unwrap();
            if !pending.iter().any(|deletion| deletion.profile == profile) {
                pending.push(PendingProfileDeletion {
                    profile,
                    native_erasure_verified: false,
                });
            }
        }
        outcome
    }
    fn finalize_profile_deletion(
        &self,
        profile: ProfileId,
        _deadline: std::time::Instant,
    ) -> ProfileDeletionFinalizeOutcome {
        self.events.lock().unwrap().push("finalize-delete");
        let outcome = self
            .finalize_outcomes
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(ProfileDeletionFinalizeOutcome::NotAuthorized);
        if outcome == ProfileDeletionFinalizeOutcome::Completed
            || (outcome == ProfileDeletionFinalizeOutcome::OutcomeUnknown
                && self
                    .finalize_unknown_completes
                    .load(std::sync::atomic::Ordering::Acquire))
        {
            self.pending_deletions
                .lock()
                .unwrap()
                .retain(|deletion| deletion.profile != profile);
        } else if let Some(deletion) = self
            .pending_deletions
            .lock()
            .unwrap()
            .iter_mut()
            .find(|deletion| deletion.profile == profile)
        {
            // A finalize attempt durably records native proof before its
            // local purge can fail.
            deletion.native_erasure_verified = true;
        }
        outcome
    }
}

pub(crate) struct FakeChrome;
impl GeometryChrome for FakeChrome {
    fn position(&self, _frame: ChromeFrame) -> bool {
        true
    }
}
impl PresentationChrome for FakeChrome {
    fn restore_browser_chrome(
        &self,
        _revision: u64,
        _items: ItemsState,
        _done: ChromePresentationCallback,
    ) -> ChromePresentationDispatch {
        ChromePresentationDispatch::Applied
    }

    fn apply_tab_for_presentation(
        &self,
        _presentation: ChromePresentation,
        _done: ChromePresentationCallback,
    ) -> ChromePresentationDispatch {
        ChromePresentationDispatch::Applied
    }
}

#[derive(Default)]
struct AsyncChrome {
    pending: Mutex<VecDeque<(ChromePresentation, ChromePresentationCallback)>>,
    browser_returns: Mutex<VecDeque<(u64, ItemsState, ChromePresentationCallback)>>,
    reject_admission: std::sync::atomic::AtomicBool,
    frames: Mutex<Vec<ChromeFrame>>,
}

impl AsyncChrome {
    fn last_frame(&self) -> Option<ChromeFrame> {
        self.frames.lock().unwrap().last().cloned()
    }
}

impl GeometryChrome for AsyncChrome {
    fn position(&self, frame: ChromeFrame) -> bool {
        self.frames.lock().unwrap().push(frame);
        true
    }
}

impl PresentationChrome for AsyncChrome {
    fn restore_browser_chrome(
        &self,
        revision: u64,
        items: ItemsState,
        done: ChromePresentationCallback,
    ) -> ChromePresentationDispatch {
        self.browser_returns
            .lock()
            .unwrap()
            .push_back((revision, items, done));
        ChromePresentationDispatch::Scheduled
    }

    fn apply_tab_for_presentation(
        &self,
        presentation: ChromePresentation,
        done: ChromePresentationCallback,
    ) -> ChromePresentationDispatch {
        if self
            .reject_admission
            .load(std::sync::atomic::Ordering::Acquire)
        {
            return ChromePresentationDispatch::Rejected;
        }
        self.pending.lock().unwrap().push_back((presentation, done));
        ChromePresentationDispatch::Scheduled
    }
}

impl AsyncChrome {
    fn complete_browser_return(&self, applied: bool) -> (u64, ItemsState) {
        let (revision, items, done) = self
            .browser_returns
            .lock()
            .unwrap()
            .pop_front()
            .expect("an exact browser return must be pending");
        done(applied);
        (revision, items)
    }

    fn presentations(&self) -> Vec<ChromePresentation> {
        self.pending
            .lock()
            .unwrap()
            .iter()
            .map(|(presentation, _)| presentation.clone())
            .collect()
    }

    fn complete_next(&self, applied: bool) -> ChromePresentation {
        let (presentation, done) = self
            .pending
            .lock()
            .unwrap()
            .pop_front()
            .expect("an exact privileged presentation must be pending");
        done(applied);
        presentation
    }
}

// Materializes projections the way the frontend store does: snapshots
// replace, deltas patch one row.
type Screen = Arc<Mutex<ItemsState>>;

fn apply_projection(view: &mut ItemsState, p: Projection) {
    match p {
        Projection::WorkChanged(_) | Projection::WorkEnvironmentChanged(_) => {}
        Projection::Items(s) => *view = s,
        Projection::Tab(t) => {
            if let Some(slot) = view.tabs.iter_mut().find(|x| x.id == t.id) {
                *slot = t;
            }
        }
        Projection::ExtensionActions(_) => {}
        Projection::ExtensionActionFailed(_) => {}
        Projection::ExtensionActionShortcut(_) => {}
        Projection::PagePermissionPrompt(_) => {}
        Projection::WebExtensionAccessRequest(_) => {}
        Projection::Favicons(_) => {}
        Projection::PanelOwner(_) => {}
        Projection::UiCommand(_) => {}
        Projection::FindResult(_) => {}
        Projection::Search(_) => {}
        Projection::Layout(_) => {}
        Projection::HostFullscreen(_) => {}
        Projection::RuntimeStatus(_) => {}
        Projection::BlockerStatus(_) => {}
        Projection::Focus(_) => {}
        Projection::OperationProcessed(_) => {}
        Projection::OpenNote { .. } => {}
    }
}

pub(super) fn setup_with(store: Arc<FakeStore>) -> (Shell, Arc<FakeEngine>, Screen) {
    let engine = Arc::new(FakeEngine::default());
    let screen: Screen = Arc::new(Mutex::new(ItemsState {
        projection_revision: String::new(),
        profile: None,
        spaces: Vec::new(),
        active_space_id: None,
        nodes: Vec::new(),
        tabs: Vec::new(),
        active: None,
        split_group: None,
    }));
    let sink = screen.clone();
    let mut shell = Shell::new_with_failure(
        engine.clone(),
        store,
        Arc::new(ImmediateAllowAllCompiler),
        Box::new(|_| {}),
        Arc::new(FakeChrome),
        Box::new(move |p| apply_projection(&mut sink.lock().unwrap(), p)),
    );
    shell.handle(Command::SetWindowSize(Size::new(1200.0, 800.0)));
    (shell, engine, screen)
}

fn setup() -> (Shell, Arc<FakeEngine>, Screen) {
    setup_with(Arc::new(FakeStore::default()))
}

type IconLog = Arc<Mutex<Vec<zephium_ipc::FaviconsView>>>;

/// Records the raster deltas chrome would receive alongside the usual screen.
fn setup_with_icon_log(store: Arc<FakeStore>) -> (Shell, Arc<FakeEngine>, Screen, IconLog) {
    let engine = Arc::new(FakeEngine::default());
    let screen: Screen = Arc::new(Mutex::new(ItemsState {
        projection_revision: String::new(),
        profile: None,
        spaces: Vec::new(),
        active_space_id: None,
        nodes: Vec::new(),
        tabs: Vec::new(),
        active: None,
        split_group: None,
    }));
    let icons: IconLog = Arc::new(Mutex::new(Vec::new()));
    let sink = screen.clone();
    let icon_sink = icons.clone();
    let mut shell = Shell::new_with_failure(
        engine.clone(),
        store,
        Arc::new(ImmediateAllowAllCompiler),
        Box::new(|_| {}),
        Arc::new(FakeChrome),
        Box::new(move |p| {
            if let Projection::Favicons(view) = &p {
                icon_sink.lock().unwrap().push(view.clone());
            }
            apply_projection(&mut sink.lock().unwrap(), p);
        }),
    );
    shell.handle(Command::SetWindowSize(Size::new(1200.0, 800.0)));
    (shell, engine, screen, icons)
}

fn setup_with_async_chrome() -> (Shell, Arc<FakeEngine>, Arc<AsyncChrome>, Screen) {
    let engine = Arc::new(FakeEngine::default());
    let chrome = Arc::new(AsyncChrome::default());
    let screen: Screen = Arc::new(Mutex::new(ItemsState {
        projection_revision: String::new(),
        profile: None,
        spaces: Vec::new(),
        active_space_id: None,
        nodes: Vec::new(),
        tabs: Vec::new(),
        active: None,
        split_group: None,
    }));
    let sink = screen.clone();
    let mut shell = Shell::new(
        engine.clone(),
        Arc::new(FakeStore::default()),
        chrome.clone(),
        Box::new(move |projection| apply_projection(&mut sink.lock().unwrap(), projection)),
    );
    shell.handle(Command::SetWindowSize(Size::new(1200.0, 800.0)));
    (shell, engine, chrome, screen)
}

type OperationLog = Arc<Mutex<Vec<OperationDisposition>>>;

fn setup_with_operation_log(
    store: Arc<FakeStore>,
) -> (Shell, Arc<FakeEngine>, Screen, OperationLog) {
    setup_with_operation_log_and_failure(store, Box::new(|_| {}))
}

pub(super) fn setup_with_operation_log_and_failure(
    store: Arc<FakeStore>,
    terminal_failure: ShellTerminalFailureCallback,
) -> (Shell, Arc<FakeEngine>, Screen, OperationLog) {
    let engine = Arc::new(FakeEngine::default());
    let screen: Screen = Arc::new(Mutex::new(ItemsState {
        projection_revision: String::new(),
        profile: None,
        spaces: Vec::new(),
        active_space_id: None,
        nodes: Vec::new(),
        tabs: Vec::new(),
        active: None,
        split_group: None,
    }));
    let operations: OperationLog = Arc::new(Mutex::new(Vec::new()));
    let sink = screen.clone();
    let operation_sink = operations.clone();
    let mut shell = Shell::new_with_failure(
        engine.clone(),
        store,
        Arc::new(ImmediateAllowAllCompiler),
        terminal_failure,
        Arc::new(FakeChrome),
        Box::new(move |projection| {
            if let Projection::OperationProcessed(completion) = &projection {
                operation_sink.lock().unwrap().push(completion.clone());
            }
            apply_projection(&mut sink.lock().unwrap(), projection);
        }),
    );
    shell.handle(Command::SetWindowSize(Size::new(1200.0, 800.0)));
    (shell, engine, screen, operations)
}

fn add_inactive_named_profile(shell: &mut Shell, seed: u128) -> ProfileId {
    let profile = ProfileId::from(seed);
    let space = SpaceId::from(seed + 1);
    assert!(shell.profiles.insert(Profile {
        id: profile,
        name: "Deletable".into(),
        kind: ProfileKind::Named,
    }));
    assert!(shell.initialize_new_blocker_profile(profile));
    assert!(shell.spaces.insert(Space {
        id: space,
        profile,
        name: "Deletable".into(),
    }));
    profile
}

fn delete_operation(operation_id: &str, profile: ProfileId) -> Command {
    Command::Operation {
        operation_id: operation_id.into(),
        command: Box::new(Command::DeleteProfile(profile)),
    }
}

fn test_shutdown_deadline() -> std::time::Instant {
    std::time::Instant::now() + END_TO_END_SHUTDOWN_TIMEOUT
}

fn last(screen: &Screen) -> ItemsState {
    screen.lock().unwrap().clone()
}

fn active_id(screen: &Screen) -> ItemId {
    ItemId::parse(&last(screen).active.unwrap()).unwrap()
}

fn persisted_zoom(store: &FakeStore, id: ItemId) -> f64 {
    let saved = store.saved.lock().unwrap().clone().unwrap();
    let item = saved.items.iter().find(|item| item.id == id).unwrap();
    let PersistedKind::Tab { zoom, .. } = &item.kind else {
        panic!("expected persisted tab")
    };
    *zoom
}

fn navigate_and_commit(shell: &mut Shell, id: ItemId, input: &str) {
    let url = navigation::classify(input)
        .expect("test navigation must be valid")
        .to_string();
    shell.handle(Command::Navigate {
        id,
        input: input.into(),
    });
    shell.handle(Command::Engine(EngineEvent::UrlChanged {
        id,
        url: url.clone(),
    }));
    present_committed(shell, id, &url);
}

fn present_committed(shell: &mut Shell, id: ItemId, url: &str) {
    static NEXT_PRESENTATION: std::sync::atomic::AtomicU64 =
        std::sync::atomic::AtomicU64::new(10_000);
    let navigation = NEXT_PRESENTATION.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    shell.handle(Command::Engine(presentation_pending(
        id,
        NavigationPresentationId::from_raw(navigation),
        url,
    )));
}

fn commit_url(shell: &mut Shell, id: ItemId, url: &str) {
    shell.handle(Command::Engine(EngineEvent::UrlChanged {
        id,
        url: url.into(),
    }));
}

fn presentation_pending(
    id: ItemId,
    navigation: NavigationPresentationId,
    url: &str,
) -> EngineEvent {
    EngineEvent::PresentationPending {
        id,
        navigation,
        url: url.into(),
    }
}

fn presentation_ready(id: ItemId, navigation: NavigationPresentationId, url: &str) -> EngineEvent {
    EngineEvent::PresentationReady {
        id,
        navigation,
        url: url.into(),
    }
}

fn first_probing_discard(shell: &Shell) -> (ItemId, DiscardProbeId) {
    shell
        .residency
        .discard_probes
        .iter()
        .find_map(|(id, state)| match state {
            PendingDiscardProbe::Probing { probe, .. } => Some((*id, *probe)),
            PendingDiscardProbe::Closing { .. } => None,
        })
        .expect("a discard probe must be in flight")
}

fn acknowledge_safe_discard(shell: &mut Shell, id: ItemId, probe: DiscardProbeId) {
    let profile = shell.profile_of_item(id).unwrap();
    shell.handle(Command::Engine(EngineEvent::DiscardSafety {
        id,
        probe,
        can_discard: true,
    }));
    assert!(matches!(
        shell.residency.discard_probes.get(&id),
        Some(PendingDiscardProbe::Closing { probe: pending, .. }) if *pending == probe
    ));
    shell.handle(Command::Engine(EngineEvent::ViewDiscarded {
        id,
        profile,
        probe,
    }));
}

mod actor_integration;
mod blocker;
mod bootstrap;
mod engine_events;
mod extension_actions;
mod extension_browser_requests;
mod extension_browser_surface;
mod favicons;
mod fullscreen;
mod history;
#[path = "navigation.rs"]
mod navigation_tests;
mod notes;
mod onboarding;
mod operations;
mod page_permissions;
mod persistence;
mod presentation;
mod private;
mod profile_deletion;
mod projections;
mod search;
mod shutdown;
mod tabs;
mod time;
mod view_lifecycle;
mod window_layout;
mod work_pane;
mod zoom;

mod blocker_sites;

#[cfg(target_os = "windows")]
mod webext_windows;
