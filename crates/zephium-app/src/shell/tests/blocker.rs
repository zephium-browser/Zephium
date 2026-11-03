use super::*;

use crate::shell::blocker::{
    blocker_runtime_diagnostics_view, catalog_snapshot_valid, settled_runtime_policy_observer,
    BlockerProfileState, RuntimePolicyObserver, INTERNAL_CATALOG_POLL_OPERATION,
};
use zephium_core::blocker::{
    ContentRuleApplyFailure, NetworkDecision, NetworkPolicyDiagnostics, NetworkRequest,
    NetworkRequestPolicy,
};
use zephium_core::ports::blocker::BlockerCompileFailure;
use zephium_core::ports::engine::ContentRuleSettlement;

type CompileCallback = Box<dyn FnOnce(BlockerCompileOutcome) + Send>;

#[cfg(feature = "work-execution")]
#[test]
fn work_profile_binding_waits_for_native_policy_and_refuses_deletion_or_missing_profile() {
    use crate::AgentWorkProfileReadiness as Readiness;
    let (mut shell, _engine, _compiler, _store, _screen) = controlled_shell();
    assert_eq!(shell.work_profile_binding(), Readiness::ProfileMissing);
    shell.handle(Command::Bootstrap);
    let profile = shell.windows.focused().unwrap().profile;
    let Readiness::PolicyPending(binding) = shell.work_profile_binding() else {
        panic!("compile must remain pending");
    };
    assert_eq!(binding.profile(), profile);
    assert_eq!(
        binding.storage_class(),
        zephium_agentic::ContextProfileStorageClass::Durable
    );
    let generation = initial_installation(&shell, profile);
    shell.handle(Command::BlockerReady(profile));
    assert_eq!(
        shell.work_profile_binding(),
        Readiness::PolicyPending(binding)
    );
    shell.handle(Command::Engine(EngineEvent::ContentRulesSettled {
        profile,
        requested: generation,
        settlement: ContentRuleSettlement::Applied { generation },
    }));
    assert_eq!(shell.work_profile_binding(), Readiness::Ready(binding));
    shell.blocker.retire_profile(profile);
    // Even a Ready classification cannot substitute an absent native policy.
    shell.blocker.profiles.get_mut(&profile).unwrap().state = BlockerProfileState::Ready {
        applied: generation,
    };
    assert_eq!(shell.work_profile_binding(), Readiness::PolicyFailed);
    shell.profile_deletion.states.insert(
        profile,
        crate::shell::profile_deletion::ProfileDeletionState::new(
            crate::shell::profile_deletion::ProfileDeletionPhase::FailedClosed,
            None,
            0,
        ),
    );
    assert_eq!(shell.work_profile_binding(), Readiness::ProfileMissing);
    shell.profile_deletion.states.clear();
    shell.blocker.profiles.remove(&profile);
    assert_eq!(shell.work_profile_binding(), Readiness::PolicyMissing);
    shell.profiles.remove(profile);
    assert_eq!(shell.work_profile_binding(), Readiness::ProfileMissing);
}

#[test]
fn runtime_diagnostics_projection_is_exact_and_javascript_safe() {
    let view = blocker_runtime_diagnostics_view(NetworkPolicyDiagnostics {
        total_decisions: u64::MAX,
        candidate_budget_exhausted: 2,
        matcher_unavailable: 3,
        matcher_unprepared: 5,
        attribution_unavailable: 7,
        evaluation_errors: 11,
    });
    assert_eq!(view.total_decisions, u64::MAX.to_string());
    assert_eq!(view.candidate_budget_exhausted, "2");
    assert_eq!(view.matcher_unavailable, "3");
    assert_eq!(view.matcher_unprepared, "5");
    assert_eq!(view.attribution_unavailable, "7");
    assert_eq!(view.evaluation_errors, "11");
}

#[test]
fn digest_identical_settlement_retains_the_installed_diagnostics_cohort() {
    let installed: Arc<dyn NetworkRequestPolicy> = Arc::new(TestNetworkPolicy);
    let candidate: Arc<dyn NetworkRequestPolicy> = Arc::new(TestNetworkPolicy);
    let digest = ContentRuleDigest::from_bytes([41; 32]);
    let observer = settled_runtime_policy_observer(
        Some(RuntimePolicyObserver {
            digest,
            policy: Arc::downgrade(&installed),
        }),
        Some(RuntimePolicyObserver {
            digest,
            policy: Arc::downgrade(&candidate),
        }),
    )
    .expect("the installed observer must remain available");
    let observed = observer
        .policy
        .upgrade()
        .expect("the installed policy remains live");
    assert!(Arc::ptr_eq(&observed, &installed));
    assert!(!Arc::ptr_eq(&observed, &candidate));

    let replacement = settled_runtime_policy_observer(
        Some(observer),
        Some(RuntimePolicyObserver {
            digest: ContentRuleDigest::from_bytes([42; 32]),
            policy: Arc::downgrade(&candidate),
        }),
    )
    .expect("a different digest must publish its own observer");
    assert!(Arc::ptr_eq(
        &replacement.policy.upgrade().unwrap(),
        &candidate
    ));
}

struct CompileRequest {
    profile: ProfileId,
    generation: ContentPolicyGeneration,
    config: BlockerConfig,
    done: CompileCallback,
}

#[derive(Default)]
struct ControlledCompiler {
    requests: Mutex<VecDeque<CompileRequest>>,
    retired: Mutex<Vec<ProfileId>>,
    retirement_callbacks: Mutex<VecDeque<Box<dyn FnOnce() + Send>>>,
    reject: std::sync::atomic::AtomicBool,
    terminal: std::sync::atomic::AtomicBool,
    enabled_terminal: std::sync::atomic::AtomicBool,
    inline_outcome: Mutex<Option<BlockerCompileOutcome>>,
    shutdown_unclean: std::sync::atomic::AtomicBool,
    shutdowns: std::sync::atomic::AtomicUsize,
    shutdown_order: Mutex<Option<Arc<Mutex<Vec<&'static str>>>>>,
    catalog: Mutex<Option<BlockerCatalogSnapshot>>,
    refresh_admissions: Mutex<VecDeque<BlockerCatalogRefreshDispatch>>,
}

impl ControlledCompiler {
    fn request(&self, index: usize) -> (ProfileId, ContentPolicyGeneration, BlockerConfig) {
        let requests = self.requests.lock().unwrap();
        let request = &requests[index];
        (request.profile, request.generation, request.config)
    }

    fn complete_next(&self, outcome: BlockerCompileOutcome) -> ContentPolicyGeneration {
        let request = self
            .requests
            .lock()
            .unwrap()
            .pop_front()
            .expect("one compile must be pending");
        let generation = request.generation;
        (request.done)(outcome);
        generation
    }

    fn complete_retirement(&self) {
        let done = self
            .retirement_callbacks
            .lock()
            .unwrap()
            .pop_front()
            .expect("one retirement must be pending");
        done();
    }

    fn set_catalog(&self, catalog: BlockerCatalogSnapshot) {
        *self.catalog.lock().unwrap() = Some(catalog);
    }

    fn admit_refresh(&self, dispatch: BlockerCatalogRefreshDispatch) {
        self.refresh_admissions.lock().unwrap().push_back(dispatch);
    }

    fn complete_inline_with(&self, outcome: BlockerCompileOutcome) {
        *self.inline_outcome.lock().unwrap() = Some(outcome);
    }
}

impl BlockerCompiler for ControlledCompiler {
    fn compile(
        &self,
        profile: ProfileId,
        generation: ContentPolicyGeneration,
        config: BlockerConfig,
        done: CompileCallback,
    ) -> BlockerDispatch {
        if self.terminal.load(std::sync::atomic::Ordering::Acquire) {
            return BlockerDispatch::Terminal;
        }
        if config.enabled
            && self
                .enabled_terminal
                .load(std::sync::atomic::Ordering::Acquire)
        {
            return BlockerDispatch::EnabledPolicyTerminal;
        }
        if self.reject.load(std::sync::atomic::Ordering::Acquire) {
            return BlockerDispatch::Rejected;
        }
        if let Some(outcome) = self.inline_outcome.lock().unwrap().take() {
            done(outcome);
            return BlockerDispatch::Scheduled;
        }
        self.requests.lock().unwrap().push_back(CompileRequest {
            profile,
            generation,
            config,
            done,
        });
        BlockerDispatch::Scheduled
    }

    fn retire_profile(
        &self,
        profile: ProfileId,
        done: Box<dyn FnOnce() + Send>,
    ) -> BlockerRetirementDispatch {
        self.retired.lock().unwrap().push(profile);
        self.retirement_callbacks.lock().unwrap().push_back(done);
        // The controlled adapter deliberately leaves older requests
        // deliverable until the test crosses this explicit barrier.
        BlockerRetirementDispatch::Scheduled
    }

    fn shutdown_until(&self, _deadline: std::time::Instant) -> BlockerShutdownOutcome {
        self.shutdowns
            .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
        if let Some(order) = self.shutdown_order.lock().unwrap().as_ref() {
            order.lock().unwrap().push("blocker");
        }
        if self
            .shutdown_unclean
            .load(std::sync::atomic::Ordering::Acquire)
        {
            BlockerShutdownOutcome::Unclean
        } else {
            BlockerShutdownOutcome::Clean
        }
    }
}

impl BlockerCatalog for ControlledCompiler {
    fn maintain(&self) -> BlockerCatalogSnapshot {
        self.catalog
            .lock()
            .unwrap()
            .unwrap_or_else(test_catalog_snapshot)
    }

    fn request_refresh(&self) -> BlockerCatalogRefreshDispatch {
        let dispatch = self
            .refresh_admissions
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(BlockerCatalogRefreshDispatch::Busy);
        if let BlockerCatalogRefreshDispatch::Accepted { operation } = dispatch {
            let mut catalog = self.catalog.lock().unwrap();
            let mut refreshing = catalog.unwrap_or_else(test_catalog_snapshot);
            refreshing.revision = refreshing.revision.checked_add(1).unwrap();
            refreshing.phase = BlockerCatalogPhase::Refreshing;
            refreshing.refresh_operation = Some(operation);
            refreshing.repair_retry_pending = false;
            if refreshing.source_material_repair_retry_pending {
                refreshing.source_material_repair_retry_pending = false;
                refreshing.source_material_repair_pending = true;
            }
            *catalog = Some(refreshing);
        }
        dispatch
    }
}

fn allow_all() -> BlockerCompileOutcome {
    BlockerCompileOutcome::Compiled(ContentRules::allow_all(ContentRuleDigest::from_bytes(
        [7; 32],
    )))
}

struct TestNetworkPolicy;

impl NetworkRequestPolicy for TestNetworkPolicy {
    fn decide(&self, _request: &NetworkRequest<'_>) -> NetworkDecision {
        NetworkDecision::Allow
    }
}

fn enabled_rules() -> BlockerCompileOutcome {
    BlockerCompileOutcome::Compiled(
        ContentRules::runtime(
            ContentRuleDigest::from_bytes([8; 32]),
            zephium_core::blocker::ContentRuleCoverage {
                source_rules: 1,
                accepted_rules: 1,
                blocking_rule_entries: 1,
                ..zephium_core::blocker::ContentRuleCoverage::default()
            },
            Arc::new(TestNetworkPolicy),
        )
        .unwrap(),
    )
}

// Most lifecycle tests exercise a later explicit enable operation. Give them
// an existing opted-out profile instead of tying that setup to product defaults.
pub(super) fn opted_out_store() -> Arc<FakeStore> {
    let seed = Arc::new(FakeStore::default());
    let (mut shell, _, _) = setup_with(seed.clone());
    shell.handle(Command::Bootstrap);
    shell.handle(Command::Persist);
    let session = seed.saved.lock().unwrap().clone().unwrap();
    let store = Arc::new(FakeStore::default());
    *store.blocker_configs.lock().unwrap() = Some(
        session
            .profiles
            .iter()
            .map(|p| ProfileBlockerConfig {
                profile: p.id,
                revision: BlockerConfigRevision::INITIAL,
                config: BlockerConfig { enabled: false },
            })
            .collect(),
    );
    *store.saved.lock().unwrap() = Some(session);
    store
}

fn controlled_shell() -> (
    Shell,
    Arc<FakeEngine>,
    Arc<ControlledCompiler>,
    Arc<FakeStore>,
    Screen,
) {
    let engine = Arc::new(FakeEngine::default());
    let compiler = Arc::new(ControlledCompiler::default());
    let store = opted_out_store();
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
    let mut shell = Shell::new_with_blocker(
        engine.clone(),
        store.clone(),
        compiler.clone(),
        Arc::new(FakeChrome),
        Box::new(move |projection| {
            apply_projection(&mut sink.lock().unwrap(), projection);
        }),
    );
    shell.handle(Command::SetWindowSize(Size::new(1200.0, 800.0)));
    (shell, engine, compiler, store, screen)
}

fn controlled_shell_with_operation_log() -> (
    Shell,
    Arc<FakeEngine>,
    Arc<ControlledCompiler>,
    Arc<FakeStore>,
    OperationLog,
) {
    let engine = Arc::new(FakeEngine::default());
    let compiler = Arc::new(ControlledCompiler::default());
    let store = opted_out_store();
    let operations: OperationLog = Arc::new(Mutex::new(Vec::new()));
    let operation_sink = operations.clone();
    let mut shell = Shell::new_with_blocker(
        engine.clone(),
        store.clone(),
        compiler.clone(),
        Arc::new(FakeChrome),
        Box::new(move |projection| {
            if let Projection::OperationProcessed(completion) = projection {
                operation_sink.lock().unwrap().push(completion);
            }
        }),
    );
    shell.handle(Command::SetWindowSize(Size::new(1200.0, 800.0)));
    (shell, engine, compiler, store, operations)
}

fn shell_with_initial_catalog(catalog: BlockerCatalogSnapshot) -> (Shell, Arc<ControlledCompiler>) {
    let compiler = Arc::new(ControlledCompiler::default());
    compiler.set_catalog(catalog);
    let shell = Shell::new_with_blocker(
        Arc::new(FakeEngine::default()),
        opted_out_store(),
        compiler.clone(),
        Arc::new(FakeChrome),
        Box::new(|_| {}),
    );
    (shell, compiler)
}

// Initial empty policies are prepared locally, outside the compiler queue.
fn initial_installation(shell: &Shell, profile: ProfileId) -> ContentPolicyGeneration {
    let BlockerProfileState::Installing {
        desired,
        retained: None,
        ..
    } = shell.blocker.profiles[&profile].state
    else {
        panic!("initial empty policy must already await native installation");
    };
    desired
}

fn blocker_operation(operation_id: &str, enabled: bool) -> Command {
    Command::Operation {
        operation_id: operation_id.into(),
        command: Box::new(Command::SetFocusedContentBlockerEnabled(enabled)),
    }
}

fn blocker_refresh_operation(operation_id: &str) -> Command {
    Command::Operation {
        operation_id: operation_id.into(),
        command: Box::new(Command::RefreshContentBlockerSources),
    }
}

fn catalog_revision(status_revision: u64, package_revision: u64) -> BlockerCatalogSnapshot {
    BlockerCatalogSnapshot {
        revision: status_revision,
        phase: BlockerCatalogPhase::Fresh,
        enabled_policy_terminal: false,
        package_revision: Some(package_revision),
        package_manifest_sha256: Some([package_revision as u8; 32]),
        package_provenance: Some(
            zephium_core::ports::blocker::BlockerCatalogProvenance::TufRepository,
        ),
        package_created_unix: Some(package_revision),
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
        installed_revision: Some(package_revision),
        installed_manifest_sha256: Some([package_revision as u8; 32]),
        installed_provenance: Some(
            zephium_core::ports::blocker::BlockerCatalogProvenance::TufRepository,
        ),
        refresh_supported: true,
        source_material_epoch: 0,
        source_material_repair_pending: false,
        source_material_repair_retry_pending: false,
        activation_pending: false,
        repair_retry_pending: false,
        last_refresh_attempt_unix: Some(package_revision),
        refresh_operation: None,
    }
}

fn set_catalog_candidate(catalog: &mut BlockerCatalogSnapshot, candidate_revision: u64) {
    catalog.candidate_revision = Some(candidate_revision);
    catalog.candidate_manifest_sha256 = Some([candidate_revision as u8; 32]);
    catalog.candidate_provenance =
        Some(zephium_core::ports::blocker::BlockerCatalogProvenance::TufRepository);
    catalog.candidate_created_unix = Some(candidate_revision);
    catalog.candidate_expires_unix = Some(u64::MAX);
    catalog.candidate_source_count = Some(1);
    catalog.candidate_source_bytes = Some(1);
    catalog.activation_pending = true;
}

fn source_identities(status: &BlockerStatusView) -> &BlockerSourceIdentities {
    status
        .source_identities
        .as_deref()
        .expect("authenticated status must expose exact package identities")
}

#[test]
fn source_refresh_operation_completes_only_after_exact_compiler_activation() {
    let (mut shell, _engine, compiler, _store, operations) = controlled_shell_with_operation_log();
    compiler.admit_refresh(BlockerCatalogRefreshDispatch::Accepted { operation: 41 });

    shell.handle(blocker_refresh_operation("refresh-lists"));
    assert!(operations.lock().unwrap().is_empty());
    let refreshing = shell.focused_blocker_status_view();
    assert_eq!(refreshing.source_phase, BlockerSourcePhase::Refreshing);
    assert_eq!(
        refreshing.source_refresh_operation.as_deref(),
        Some("0000000000000029")
    );

    // A verified package which has not crossed the compiler barrier remains
    // pending and cannot complete the accepted UI operation.
    let mut activating = catalog_revision(3, 2);
    activating.installed_revision = Some(1);
    activating.installed_manifest_sha256 = Some([1; 32]);
    set_catalog_candidate(&mut activating, 2);
    compiler.set_catalog(activating);
    shell.handle(Command::Tick);
    assert!(operations.lock().unwrap().is_empty());

    compiler.set_catalog(catalog_revision(4, 2));
    shell.handle(Command::Tick);
    assert_eq!(
        operations.lock().unwrap().as_slice(),
        &[OperationDisposition {
            operation_id: "refresh-lists".into(),
            outcome: OperationOutcome::Applied,
            reason: OperationReason::ContentPolicySourcesRefreshed,
        }]
    );
}

#[test]
fn terminal_activation_failure_rejects_refresh_before_waiting_on_candidate() {
    let (mut shell, _engine, compiler, _store, operations) = controlled_shell_with_operation_log();
    compiler.admit_refresh(BlockerCatalogRefreshDispatch::Accepted { operation: 41 });
    shell.handle(blocker_refresh_operation("refresh-lists"));
    assert!(operations.lock().unwrap().is_empty());

    let mut failed = catalog_revision(3, 2);
    failed.phase =
        BlockerCatalogPhase::Failed(zephium_core::ports::blocker::BlockerCatalogFailure::Internal);
    failed.installed_revision = Some(1);
    failed.installed_manifest_sha256 = Some([1; 32]);
    set_catalog_candidate(&mut failed, 2);
    compiler.set_catalog(failed);
    shell.handle(Command::Tick);

    assert_eq!(
        operations.lock().unwrap().as_slice(),
        &[OperationDisposition {
            operation_id: "refresh-lists".into(),
            outcome: OperationOutcome::Rejected,
            reason: OperationReason::ContentPolicySourceRefreshFailed,
        }]
    );
}

#[test]
fn startup_candidate_advances_on_internal_polls_without_waiting_for_heartbeat() {
    let mut initial = catalog_revision(1, 1);
    set_catalog_candidate(&mut initial, 2);
    let (mut shell, compiler) = shell_with_initial_catalog(initial);
    shell.attach_queue(CommandQueue::new());

    assert_eq!(
        shell.pending_catalog_activation_poll_for_test(),
        Some((1, 1))
    );

    let mut committed = catalog_revision(2, 2);
    committed.installed_revision = Some(1);
    committed.installed_manifest_sha256 = Some([1; 32]);
    set_catalog_candidate(&mut committed, 2);
    compiler.set_catalog(committed);
    shell.on_blocker_catalog_poll(INTERNAL_CATALOG_POLL_OPERATION, 0);
    let catalog = shell.blocker_catalog_for_test();
    assert_eq!(catalog.package_revision, Some(2));
    assert_eq!(catalog.installed_revision, Some(1));
    assert!(catalog.activation_pending);

    compiler.set_catalog(catalog_revision(3, 2));
    shell.on_blocker_catalog_poll(INTERNAL_CATALOG_POLL_OPERATION, 0);
    let catalog = shell.blocker_catalog_for_test();
    assert_eq!(catalog.installed_revision, Some(2));
    assert!(!catalog.activation_pending);
    assert_eq!(shell.pending_catalog_activation_poll_for_test(), None);
}

#[test]
fn refreshing_candidate_keeps_fast_internal_settlement_polling() {
    let mut refreshing = catalog_revision(1, 1);
    set_catalog_candidate(&mut refreshing, 2);
    refreshing.phase = BlockerCatalogPhase::Refreshing;
    refreshing.refresh_operation = Some(7);
    let (mut shell, _compiler) = shell_with_initial_catalog(refreshing);
    shell.attach_queue(CommandQueue::new());

    assert_eq!(
        shell.pending_catalog_activation_poll_for_test(),
        Some((1, 1))
    );
}

#[test]
fn inline_compile_completion_has_one_prompt_wake_and_no_duplicate_native_effect() {
    let (mut shell, engine, compiler, _store, _screen) = controlled_shell();
    let queue = CommandQueue::new();
    shell.attach_queue(queue.clone());
    shell.handle(Command::Bootstrap);
    let profile = shell.windows.focused().unwrap().profile;
    let initial = initial_installation(&shell, profile);
    shell.handle(Command::BlockerReady(profile));
    shell.handle(Command::Engine(EngineEvent::ContentRulesSettled {
        profile,
        requested: initial,
        settlement: ContentRuleSettlement::Applied {
            generation: initial,
        },
    }));
    while queue.try_recv().is_some() {}

    compiler.complete_inline_with(enabled_rules());
    let callback = CallbackHandle {
        queue: Arc::downgrade(&queue.inner),
    };
    assert!(shell
        .blocker
        .start_compile(profile, BlockerConfig { enabled: true }, Some(callback),));
    assert!(matches!(
        shell.blocker.profiles[&profile].state,
        BlockerProfileState::Compiling { .. }
    ));

    let wake = queue
        .try_recv()
        .expect("inline completion must hand one prompt wake to the actor");
    assert!(matches!(wake, Command::BlockerReady(found) if found == profile));
    assert!(queue.try_recv().is_none());
    shell.handle(wake);

    let installed = engine
        .calls()
        .iter()
        .filter(|call| call.starts_with("install-content-rules "))
        .count();
    assert_eq!(installed, 2);
    let state_after_wake = shell.blocker.profiles[&profile].state;
    shell.handle(Command::BlockerReady(profile));
    assert_eq!(shell.blocker.profiles[&profile].state, state_after_wake);
    assert_eq!(
        engine
            .calls()
            .iter()
            .filter(|call| call.starts_with("install-content-rules "))
            .count(),
        installed
    );
}

#[test]
fn current_material_repair_keeps_fast_internal_settlement_polling() {
    let mut repairing = catalog_revision(1, 1);
    repairing.phase = BlockerCatalogPhase::Refreshing;
    repairing.refresh_operation = Some(7);
    repairing.source_material_repair_pending = true;
    let (mut shell, _compiler) = shell_with_initial_catalog(repairing);
    shell.attach_queue(CommandQueue::new());

    assert_eq!(
        shell.pending_catalog_activation_poll_for_test(),
        Some((1, 1))
    );
}

#[test]
fn exhausted_current_material_repair_blocks_enable_without_background_polling() {
    let (mut shell, _engine, compiler, _store, _operations) = controlled_shell_with_operation_log();
    shell.handle(Command::Bootstrap);
    let profile = shell.windows.focused().unwrap().profile;
    let initial = initial_installation(&shell, profile);
    shell.handle(Command::BlockerReady(profile));
    shell.handle(Command::Engine(EngineEvent::ContentRulesSettled {
        profile,
        requested: initial,
        settlement: ContentRuleSettlement::Applied {
            generation: initial,
        },
    }));

    let mut retry = catalog_revision(2, 1);
    retry.source_material_repair_retry_pending = true;
    compiler.set_catalog(retry);
    shell.handle(Command::Tick);

    assert_eq!(shell.pending_catalog_activation_poll_for_test(), None);
    let status = shell.focused_blocker_status_view();
    assert!(!status.source_material_repair_pending);
    assert!(status.source_material_repair_retry_pending);
    assert_eq!(
        shell.begin_focused_blocker_mutation("enable-broken-material".into(), true),
        Some(OperationDisposition {
            operation_id: String::new(),
            outcome: OperationOutcome::Rejected,
            reason: OperationReason::ContentPolicySourceUnavailable,
        })
    );

    compiler.admit_refresh(BlockerCatalogRefreshDispatch::Accepted { operation: 8 });
    shell.handle(blocker_refresh_operation("repair-current-material"));
    let status = shell.focused_blocker_status_view();
    assert!(status.source_material_repair_pending);
    assert!(!status.source_material_repair_retry_pending);
}

#[test]
fn failed_candidate_stops_internal_poll_until_explicit_retry_is_accepted() {
    let mut failed = catalog_revision(1, 1);
    set_catalog_candidate(&mut failed, 2);
    failed.phase =
        BlockerCatalogPhase::Failed(zephium_core::ports::blocker::BlockerCatalogFailure::Transport);
    failed.refresh_operation = Some(7);
    failed.repair_retry_pending = true;
    let (mut shell, compiler) = shell_with_initial_catalog(failed);
    shell.attach_queue(CommandQueue::new());

    assert_eq!(shell.pending_catalog_activation_poll_for_test(), None);
    assert_eq!(shell.pending_catalog_refresh_for_test(), None);

    compiler.admit_refresh(BlockerCatalogRefreshDispatch::Accepted { operation: 8 });
    shell.handle(blocker_refresh_operation("retry-lists"));

    assert_eq!(shell.pending_catalog_activation_poll_for_test(), None);
    assert!(matches!(
        shell.pending_catalog_refresh_for_test(),
        Some((8, attempts)) if attempts > 0
    ));
    assert_eq!(
        shell.blocker_catalog_for_test().phase,
        BlockerCatalogPhase::Refreshing
    );
}

#[test]
fn duplicate_and_failed_source_refreshes_have_exact_terminal_dispositions() {
    let (mut shell, _engine, compiler, _store, operations) = controlled_shell_with_operation_log();
    compiler.admit_refresh(BlockerCatalogRefreshDispatch::Accepted { operation: 9 });
    shell.handle(blocker_refresh_operation("first-refresh"));
    shell.handle(blocker_refresh_operation("duplicate-refresh"));

    assert_eq!(
        operations.lock().unwrap().as_slice(),
        &[OperationDisposition {
            operation_id: "duplicate-refresh".into(),
            outcome: OperationOutcome::Rejected,
            reason: OperationReason::ContentPolicySourceRefreshPending,
        }]
    );

    let mut failed = catalog_revision(3, 1);
    failed.phase =
        BlockerCatalogPhase::Failed(zephium_core::ports::blocker::BlockerCatalogFailure::Transport);
    failed.refresh_operation = Some(9);
    compiler.set_catalog(failed);
    shell.handle(Command::Tick);
    assert_eq!(
        operations.lock().unwrap().as_slice(),
        &[
            OperationDisposition {
                operation_id: "duplicate-refresh".into(),
                outcome: OperationOutcome::Rejected,
                reason: OperationReason::ContentPolicySourceRefreshPending,
            },
            OperationDisposition {
                operation_id: "first-refresh".into(),
                outcome: OperationOutcome::Rejected,
                reason: OperationReason::ContentPolicySourceRefreshFailed,
            },
        ]
    );
}

#[test]
fn catalog_identity_regression_terminalizes_without_erasing_last_known_metadata() {
    let (mut shell, _engine, compiler, _store, operations) = controlled_shell_with_operation_log();
    let mut regressed = BlockerCatalogSnapshot::not_configured();
    regressed.revision = 2;
    compiler.set_catalog(regressed);

    shell.handle(Command::Tick);
    let status = shell.focused_blocker_status_view();
    assert_eq!(status.source_phase, BlockerSourcePhase::Failed);
    assert_eq!(status.source_failure, Some(BlockerSourceFailure::Rollback));
    assert_eq!(
        status.source_package_revision.as_deref(),
        Some("0000000000000001")
    );

    shell.handle(blocker_refresh_operation("after-regression"));
    assert_eq!(
        operations.lock().unwrap().as_slice(),
        &[OperationDisposition {
            operation_id: "after-regression".into(),
            outcome: OperationOutcome::Rejected,
            reason: OperationReason::ContentPolicySourceUnavailable,
        }]
    );
}

#[test]
fn enabled_policy_terminal_is_monotonic_catalog_authority() {
    let (mut shell, _engine, compiler, _store, _operations) = controlled_shell_with_operation_log();
    let mut terminal = catalog_revision(2, 1);
    terminal.phase =
        BlockerCatalogPhase::Failed(zephium_core::ports::blocker::BlockerCatalogFailure::Internal);
    terminal.enabled_policy_terminal = true;
    compiler.set_catalog(terminal);
    shell.handle(Command::Tick);
    assert!(shell.blocker_catalog_for_test().enabled_policy_terminal);

    compiler.set_catalog(catalog_revision(3, 1));
    shell.handle(Command::Tick);
    let status = shell.focused_blocker_status_view();
    assert_eq!(status.source_phase, BlockerSourcePhase::Failed);
    assert_eq!(status.source_failure, Some(BlockerSourceFailure::Rollback));
    assert!(shell.blocker_catalog_for_test().enabled_policy_terminal);
}

#[test]
fn same_revision_manifest_or_metadata_equivocation_terminalizes() {
    for mutate in [
        |catalog: &mut BlockerCatalogSnapshot| {
            catalog.package_manifest_sha256 = Some([9; 32]);
            catalog.installed_manifest_sha256 = Some([9; 32]);
        },
        |catalog: &mut BlockerCatalogSnapshot| {
            catalog.package_created_unix = Some(2);
        },
    ] {
        let (mut shell, _engine, compiler, _store, _operations) =
            controlled_shell_with_operation_log();
        let mut equivocated = catalog_revision(2, 1);
        mutate(&mut equivocated);
        compiler.set_catalog(equivocated);

        shell.handle(Command::Tick);
        let status = shell.focused_blocker_status_view();
        assert_eq!(status.source_phase, BlockerSourcePhase::Failed);
        assert_eq!(status.source_failure, Some(BlockerSourceFailure::Rollback));
        assert_eq!(
            source_identities(&status)
                .package_manifest_sha256
                .as_deref(),
            Some("01".repeat(32).as_str())
        );
    }
}

#[test]
fn material_epoch_can_advance_only_for_unchanged_exact_installed_authority() {
    let (mut shell, _engine, compiler, _store, _operations) = controlled_shell_with_operation_log();
    let mut unrelated_package = catalog_revision(2, 2);
    unrelated_package.source_material_epoch = 1;
    compiler.set_catalog(unrelated_package);

    shell.handle(Command::Tick);
    let status = shell.focused_blocker_status_view();
    assert_eq!(status.source_phase, BlockerSourcePhase::Failed);
    assert_eq!(status.source_failure, Some(BlockerSourceFailure::Rollback));
    assert_eq!(
        status.source_package_revision.as_deref(),
        Some("0000000000000001")
    );
}

#[test]
fn a_new_package_revision_cannot_reuse_an_older_manifest_digest() {
    let (mut shell, _engine, compiler, _store, _operations) = controlled_shell_with_operation_log();
    let mut contradictory = catalog_revision(2, 2);
    contradictory.package_manifest_sha256 = Some([1; 32]);
    contradictory.installed_manifest_sha256 = Some([1; 32]);
    compiler.set_catalog(contradictory);

    shell.handle(Command::Tick);
    let status = shell.focused_blocker_status_view();
    assert_eq!(status.source_phase, BlockerSourcePhase::Failed);
    assert_eq!(status.source_failure, Some(BlockerSourceFailure::Rollback));
}

#[test]
fn rejected_candidate_high_water_rejects_later_same_revision_equivocation() {
    let (mut shell, _engine, compiler, _store, _operations) = controlled_shell_with_operation_log();
    let mut candidate = catalog_revision(2, 1);
    set_catalog_candidate(&mut candidate, 2);
    compiler.set_catalog(candidate);
    shell.handle(Command::Tick);
    let pending = shell.focused_blocker_status_view();
    assert_eq!(
        source_identities(&pending).candidate_revision.as_deref(),
        Some("0000000000000002")
    );
    assert_eq!(
        source_identities(&pending)
            .candidate_manifest_sha256
            .as_deref(),
        Some("02".repeat(32).as_str())
    );

    compiler.set_catalog(catalog_revision(3, 1));
    shell.handle(Command::Tick);

    let mut equivocated = catalog_revision(4, 1);
    set_catalog_candidate(&mut equivocated, 2);
    equivocated.candidate_manifest_sha256 = Some([9; 32]);
    compiler.set_catalog(equivocated);
    shell.handle(Command::Tick);

    let status = shell.focused_blocker_status_view();
    assert_eq!(status.source_phase, BlockerSourcePhase::Failed);
    assert_eq!(status.source_failure, Some(BlockerSourceFailure::Rollback));
    assert!(source_identities(&status).candidate_revision.is_none());
}

#[test]
fn initial_uncommitted_candidate_is_not_reported_as_current() {
    let mut candidate = BlockerCatalogSnapshot::not_configured();
    candidate.revision = 2;
    candidate.phase = BlockerCatalogPhase::Refreshing;
    candidate.refresh_operation = Some(7);
    set_catalog_candidate(&mut candidate, 1);

    let (shell, _compiler) = shell_with_initial_catalog(candidate);
    let status = shell.focused_blocker_status_view();
    assert!(status.source_package_revision.is_none());
    assert!(source_identities(&status).package_manifest_sha256.is_none());
    assert_eq!(
        source_identities(&status).candidate_revision.as_deref(),
        Some("0000000000000001")
    );
    assert_eq!(
        source_identities(&status)
            .candidate_manifest_sha256
            .as_deref(),
        Some("01".repeat(32).as_str())
    );
    assert!(status.source_installed_revision.is_none());
    assert!(status.source_activation_pending);
}

#[test]
fn initial_or_advancing_installed_identity_must_equal_exact_current() {
    let mut invalid_initial = catalog_revision(1, 3);
    invalid_initial.installed_revision = Some(1);
    invalid_initial.installed_manifest_sha256 = Some([1; 32]);
    set_catalog_candidate(&mut invalid_initial, 3);
    let (shell, _compiler) = shell_with_initial_catalog(invalid_initial);
    let status = shell.focused_blocker_status_view();
    assert_eq!(status.source_phase, BlockerSourcePhase::Failed);
    assert_eq!(status.source_failure, Some(BlockerSourceFailure::Internal));

    let (mut shell, _engine, compiler, _store, _operations) = controlled_shell_with_operation_log();
    let mut committed = catalog_revision(2, 3);
    committed.installed_revision = Some(1);
    committed.installed_manifest_sha256 = Some([1; 32]);
    set_catalog_candidate(&mut committed, 3);
    compiler.set_catalog(committed);
    shell.handle(Command::Tick);
    assert_eq!(
        shell.focused_blocker_status_view().source_phase,
        BlockerSourcePhase::Fresh
    );

    let mut contradictory_install = catalog_revision(3, 3);
    contradictory_install.installed_revision = Some(2);
    contradictory_install.installed_manifest_sha256 = Some([2; 32]);
    set_catalog_candidate(&mut contradictory_install, 3);
    compiler.set_catalog(contradictory_install);
    shell.handle(Command::Tick);

    let status = shell.focused_blocker_status_view();
    assert_eq!(status.source_phase, BlockerSourcePhase::Failed);
    assert_eq!(status.source_failure, Some(BlockerSourceFailure::Rollback));
}

#[test]
fn catalog_terminalization_settles_an_exact_native_mutation_operation() {
    let (mut shell, _engine, compiler, _store, operations) = controlled_shell_with_operation_log();
    shell.handle(Command::Bootstrap);
    let profile = shell.windows.focused().unwrap().profile;

    let initial = initial_installation(&shell, profile);
    shell.handle(Command::BlockerReady(profile));
    shell.handle(Command::Engine(EngineEvent::ContentRulesSettled {
        profile,
        requested: initial,
        settlement: ContentRuleSettlement::Applied {
            generation: initial,
        },
    }));
    shell.handle(blocker_operation("enable-during-regression", true));
    assert!(operations.lock().unwrap().is_empty());
    assert_eq!(
        shell.focused_blocker_status_view().preference,
        BlockerPreferenceState::Updating
    );

    let mut regressed = BlockerCatalogSnapshot::not_configured();
    regressed.revision = 2;
    compiler.set_catalog(regressed);
    shell.handle(Command::Tick);

    assert_eq!(
        operations.lock().unwrap().as_slice(),
        &[OperationDisposition {
            operation_id: "enable-during-regression".into(),
            outcome: OperationOutcome::NativeAdmissionFailed,
            reason: OperationReason::ContentPolicyApplyFailed,
        }]
    );
    assert_eq!(
        shell.focused_blocker_status_view().preference,
        BlockerPreferenceState::Authoritative
    );
}

#[test]
fn catalog_replacements_coalesce_without_displacing_an_inflight_native_generation() {
    let (mut shell, _engine, compiler, _store, _operations) = controlled_shell_with_operation_log();
    shell.handle(Command::Bootstrap);
    let profile = shell.windows.focused().unwrap().profile;

    let initial = initial_installation(&shell, profile);
    shell.handle(Command::BlockerReady(profile));
    shell.handle(Command::Engine(EngineEvent::ContentRulesSettled {
        profile,
        requested: initial,
        settlement: ContentRuleSettlement::Applied {
            generation: initial,
        },
    }));

    shell.handle(blocker_operation("enable-before-update", true));
    let enabled = compiler.complete_next(enabled_rules());
    shell.handle(Command::BlockerReady(profile));
    shell.handle(Command::Engine(EngineEvent::ContentRulesSettled {
        profile,
        requested: enabled,
        settlement: ContentRuleSettlement::Applied {
            generation: enabled,
        },
    }));
    assert_eq!(
        shell.focused_blocker_status_view().protection,
        BlockerProtection::Active
    );

    compiler.set_catalog(catalog_revision(2, 2));
    shell.handle(Command::Tick);
    let package_two = compiler.request(0).1;

    compiler.set_catalog(catalog_revision(3, 3));
    shell.handle(Command::Tick);
    assert_eq!(compiler.requests.lock().unwrap().len(), 1);

    compiler.complete_next(enabled_rules());
    shell.handle(Command::BlockerReady(profile));
    shell.handle(Command::Engine(EngineEvent::ContentRulesSettled {
        profile,
        requested: package_two,
        settlement: ContentRuleSettlement::Applied {
            generation: package_two,
        },
    }));
    assert_eq!(
        shell.focused_blocker_status_view().protection,
        BlockerProtection::Degraded
    );
    let package_three = compiler.request(0).1;
    assert!(package_three > package_two);

    compiler.complete_next(enabled_rules());
    shell.handle(Command::BlockerReady(profile));
    shell.handle(Command::Engine(EngineEvent::ContentRulesSettled {
        profile,
        requested: package_three,
        settlement: ContentRuleSettlement::Applied {
            generation: package_three,
        },
    }));
    let status = shell.focused_blocker_status_view();
    assert_eq!(status.protection, BlockerProtection::Active);
    assert_eq!(
        status.source_installed_revision.as_deref(),
        Some("0000000000000003")
    );
}

#[test]
fn bundled_refresh_advice_does_not_degrade_an_exact_active_policy() {
    let mut bundled = catalog_revision(2, 2);
    bundled.package_provenance =
        Some(zephium_core::ports::blocker::BlockerCatalogProvenance::ReleaseBundle);
    bundled.installed_provenance =
        Some(zephium_core::ports::blocker::BlockerCatalogProvenance::ReleaseBundle);
    bundled.refresh_supported = false;
    bundled.last_refresh_attempt_unix = None;
    bundled.source_refresh_due = true;
    assert!(catalog_snapshot_valid(bundled));
    let (mut shell, compiler) = shell_with_initial_catalog(bundled);
    shell.handle(Command::SetWindowSize(Size::new(1200.0, 800.0)));

    shell.handle(Command::Bootstrap);
    let profile = shell.windows.focused().unwrap().profile;
    let initial = initial_installation(&shell, profile);
    shell.handle(Command::BlockerReady(profile));
    shell.handle(Command::Engine(EngineEvent::ContentRulesSettled {
        profile,
        requested: initial,
        settlement: ContentRuleSettlement::Applied {
            generation: initial,
        },
    }));

    shell.handle(blocker_operation("enable-bundled", true));
    let enabled = compiler.complete_next(enabled_rules());
    shell.handle(Command::BlockerReady(profile));
    shell.handle(Command::Engine(EngineEvent::ContentRulesSettled {
        profile,
        requested: enabled,
        settlement: ContentRuleSettlement::Applied {
            generation: enabled,
        },
    }));

    let status = shell.focused_blocker_status_view();
    assert_eq!(status.protection, BlockerProtection::Active);
    assert!(status.source_refresh_due);
    assert_eq!(status.source_package_stale, Some(false));
    assert_eq!(
        status.source_package_provenance,
        Some(zephium_ipc::BlockerSourceProvenance::ReleaseBundle)
    );
    assert!(!status.can_refresh_sources);
}

#[test]
fn transient_catalog_failure_with_exact_installed_policy_still_admits_enable_cas() {
    let (mut shell, _engine, compiler, store, operations) = controlled_shell_with_operation_log();
    store
        .hold_blocker_updates
        .store(true, std::sync::atomic::Ordering::Release);
    shell.handle(Command::Bootstrap);

    let mut failed = catalog_revision(2, 1);
    failed.phase =
        BlockerCatalogPhase::Failed(zephium_core::ports::blocker::BlockerCatalogFailure::Transport);
    compiler.set_catalog(failed);
    shell.handle(blocker_operation("enable-transient", true));

    assert!(operations.lock().unwrap().is_empty());
    assert_eq!(store.held_blocker_updates.lock().unwrap().len(), 1);
}

#[test]
fn enabled_policy_terminal_rejects_enable_before_durable_cas() {
    let (mut shell, _engine, compiler, store, operations) = controlled_shell_with_operation_log();
    store
        .hold_blocker_updates
        .store(true, std::sync::atomic::Ordering::Release);
    shell.handle(Command::Bootstrap);

    let mut terminal = catalog_revision(2, 1);
    terminal.phase =
        BlockerCatalogPhase::Failed(zephium_core::ports::blocker::BlockerCatalogFailure::Internal);
    terminal.enabled_policy_terminal = true;
    compiler.set_catalog(terminal);
    shell.handle(blocker_operation("enable-terminal", true));

    assert!(store.held_blocker_updates.lock().unwrap().is_empty());
    assert_eq!(
        operations.lock().unwrap().as_slice(),
        &[OperationDisposition {
            operation_id: "enable-terminal".into(),
            outcome: OperationOutcome::Rejected,
            reason: OperationReason::ContentPolicySourceUnavailable,
        }]
    );
}

#[test]
fn enabled_policy_terminal_short_circuits_only_enabled_compilation() {
    let (mut shell, _engine, compiler, _store, _screen) = controlled_shell();
    shell.handle(Command::Bootstrap);
    let profile = shell.windows.focused().unwrap().profile;
    let initial = initial_installation(&shell, profile);
    shell.handle(Command::BlockerReady(profile));
    shell.handle(Command::Engine(EngineEvent::ContentRulesSettled {
        profile,
        requested: initial,
        settlement: ContentRuleSettlement::Applied {
            generation: initial,
        },
    }));

    let mut terminal = catalog_revision(2, 1);
    terminal.phase =
        BlockerCatalogPhase::Failed(zephium_core::ports::blocker::BlockerCatalogFailure::Internal);
    terminal.enabled_policy_terminal = true;
    compiler.set_catalog(terminal);
    shell.handle(Command::Tick);

    assert!(!shell
        .blocker
        .start_compile(profile, BlockerConfig { enabled: true }, None));
    assert!(compiler.requests.lock().unwrap().is_empty());
    assert!(matches!(
        shell.blocker.profiles[&profile].state,
        BlockerProfileState::Failed {
            failure: ContentPolicyFailure::CompilerUnavailable,
            ..
        }
    ));

    assert!(shell
        .blocker
        .start_compile(profile, BlockerConfig { enabled: false }, None));
    assert!(!compiler.request(0).2.enabled);
}

#[test]
fn racing_enabled_terminal_dispatch_does_not_seal_allow_all_compilation() {
    let (mut shell, _engine, compiler, _store, _screen) = controlled_shell();
    shell.handle(Command::Bootstrap);
    let profile = shell.windows.focused().unwrap().profile;
    let initial = initial_installation(&shell, profile);
    shell.handle(Command::BlockerReady(profile));
    shell.handle(Command::Engine(EngineEvent::ContentRulesSettled {
        profile,
        requested: initial,
        settlement: ContentRuleSettlement::Applied {
            generation: initial,
        },
    }));

    compiler
        .enabled_terminal
        .store(true, std::sync::atomic::Ordering::Release);
    assert!(!shell
        .blocker
        .start_compile(profile, BlockerConfig { enabled: true }, None));
    assert!(matches!(
        shell.blocker.profiles[&profile].state,
        BlockerProfileState::Failed {
            failure: ContentPolicyFailure::CompilerUnavailable,
            ..
        }
    ));

    compiler
        .enabled_terminal
        .store(false, std::sync::atomic::Ordering::Release);
    assert!(shell
        .blocker
        .start_compile(profile, BlockerConfig { enabled: false }, None));
    assert!(!compiler.request(0).2.enabled);
}

#[test]
fn durable_toggle_is_not_completed_or_projected_before_the_exact_store_callback() {
    let store = opted_out_store();
    store
        .hold_blocker_updates
        .store(true, std::sync::atomic::Ordering::Release);
    let (mut shell, _engine, _screen, operations) = setup_with_operation_log(store.clone());
    shell.handle(Command::Bootstrap);
    let profile = shell.windows.focused().unwrap().profile;

    shell.handle(blocker_operation("enable-1", true));
    assert!(operations.lock().unwrap().is_empty());
    assert_eq!(
        shell.blocker.profiles[&profile].config.config,
        BlockerConfig { enabled: false }
    );
    assert_eq!(
        shell.focused_blocker_status_view().preference,
        BlockerPreferenceState::Updating
    );

    shell.handle(blocker_operation("enable-duplicate", true));
    assert_eq!(
        operations.lock().unwrap().as_slice(),
        &[OperationDisposition {
            operation_id: "enable-duplicate".into(),
            outcome: OperationOutcome::Rejected,
            reason: OperationReason::StoreWorkPending,
        }]
    );

    store.complete_blocker_update(BlockerConfigUpdateOutcome::Updated(ProfileBlockerConfig {
        profile,
        revision: BlockerConfigRevision::INITIAL.next().unwrap(),
        config: BlockerConfig { enabled: true },
    }));
    // Model a saturated command queue: Tick must recover the retained result.
    shell.handle(Command::Tick);
    let operations = operations.lock().unwrap();
    assert_eq!(operations.len(), 2);
    assert_eq!(operations[1].operation_id, "enable-1");
    assert_eq!(operations[1].outcome, OperationOutcome::Applied);
    assert_eq!(operations[1].reason, OperationReason::MutationApplied);
    drop(operations);
    let status = shell.focused_blocker_status_view();
    assert_eq!(status.preference, BlockerPreferenceState::Authoritative);
    assert_eq!(status.protection, BlockerProtection::Active);
    assert_eq!(status.desired_enabled, Some(true));
    assert_eq!(status.applied_enabled, Some(true));
}

#[test]
fn indeterminate_store_outcome_self_heals_without_guessing_the_preference() {
    let store = opted_out_store();
    store
        .blocker_update_outcomes
        .lock()
        .unwrap()
        .push_back(BlockerConfigUpdateOutcome::OutcomeUnknown);
    store
        .hold_blocker_loads
        .store(true, std::sync::atomic::Ordering::Release);
    let (mut shell, _engine, _screen, operations) = setup_with_operation_log(store.clone());
    shell.handle(Command::Bootstrap);
    let profile = shell.windows.focused().unwrap().profile;

    shell.handle(blocker_operation("enable-unknown", true));
    assert_eq!(
        operations.lock().unwrap().as_slice(),
        &[OperationDisposition {
            operation_id: "enable-unknown".into(),
            outcome: OperationOutcome::Deferred,
            reason: OperationReason::StoreOutcomeUnknown,
        }]
    );
    let unknown = shell.focused_blocker_status_view();
    assert_eq!(unknown.preference, BlockerPreferenceState::Reconciling);
    assert_eq!(unknown.protection, BlockerProtection::Unavailable);
    assert_eq!(unknown.desired_enabled, Some(false));
    assert_eq!(unknown.applied_enabled, Some(false));

    store.complete_blocker_load(BlockerConfigLoadOutcome::Loaded(ProfileBlockerConfig {
        profile,
        revision: BlockerConfigRevision::INITIAL.next().unwrap(),
        config: BlockerConfig { enabled: true },
    }));
    shell.handle(Command::Tick);
    let reconciled = shell.focused_blocker_status_view();
    assert_eq!(reconciled.preference, BlockerPreferenceState::Authoritative);
    assert_eq!(reconciled.protection, BlockerProtection::Active);
    assert_eq!(reconciled.desired_enabled, Some(true));
    assert_eq!(reconciled.applied_enabled, Some(true));
    assert_eq!(operations.lock().unwrap().len(), 1);
}

#[test]
fn conflicting_toggle_reconciles_returned_authority_and_rejects_the_operation() {
    let store = opted_out_store();
    let (mut shell, _engine, _screen, operations) = setup_with_operation_log(store.clone());
    shell.handle(Command::Bootstrap);
    let profile = shell.windows.focused().unwrap().profile;
    store
        .blocker_update_outcomes
        .lock()
        .unwrap()
        .push_back(BlockerConfigUpdateOutcome::Conflict(ProfileBlockerConfig {
            profile,
            revision: BlockerConfigRevision::INITIAL.next().unwrap(),
            config: BlockerConfig { enabled: false },
        }));

    shell.handle(blocker_operation("enable-conflict", true));
    assert_eq!(
        operations.lock().unwrap().as_slice(),
        &[OperationDisposition {
            operation_id: "enable-conflict".into(),
            outcome: OperationOutcome::Rejected,
            reason: OperationReason::StoreConflict,
        }]
    );
    let status = shell.focused_blocker_status_view();
    assert_eq!(status.preference, BlockerPreferenceState::Authoritative);
    assert_eq!(status.desired_enabled, Some(false));
    assert_eq!(status.applied_enabled, Some(false));
    assert_eq!(
        status.config_revision,
        Some(format!(
            "{:016x}",
            BlockerConfigRevision::INITIAL.next().unwrap().get()
        ))
    );
}

#[test]
fn matching_store_conflict_completes_noop_only_after_native_reconciliation() {
    let store = opted_out_store();
    let (mut shell, _engine, _screen, operations) = setup_with_operation_log(store.clone());
    shell.handle(Command::Bootstrap);
    let profile = shell.windows.focused().unwrap().profile;
    store
        .blocker_update_outcomes
        .lock()
        .unwrap()
        .push_back(BlockerConfigUpdateOutcome::Conflict(ProfileBlockerConfig {
            profile,
            revision: BlockerConfigRevision::INITIAL.next().unwrap(),
            config: BlockerConfig { enabled: true },
        }));

    shell.handle(blocker_operation("enable-conflict-match", true));
    assert_eq!(
        operations.lock().unwrap().as_slice(),
        &[OperationDisposition {
            operation_id: "enable-conflict-match".into(),
            outcome: OperationOutcome::NoOp,
            reason: OperationReason::StateUnchanged,
        }]
    );
    let status = shell.focused_blocker_status_view();
    assert_eq!(status.preference, BlockerPreferenceState::Authoritative);
    assert_eq!(status.protection, BlockerProtection::Active);
    assert_eq!(status.desired_enabled, Some(true));
    assert_eq!(status.applied_enabled, Some(true));
}

#[test]
fn toggle_completion_waits_for_exact_native_settlement_and_reports_failure() {
    let (mut shell, _engine, compiler, _store, operations) = controlled_shell_with_operation_log();
    shell.handle(Command::Bootstrap);
    let profile = shell.windows.focused().unwrap().profile;
    let retained = initial_installation(&shell, profile);
    shell.handle(Command::BlockerReady(profile));
    shell.handle(Command::Engine(EngineEvent::ContentRulesSettled {
        profile,
        requested: retained,
        settlement: ContentRuleSettlement::Applied {
            generation: retained,
        },
    }));

    shell.handle(blocker_operation("enable-native-fail", true));
    assert!(operations.lock().unwrap().is_empty());
    let requested = compiler.complete_next(enabled_rules());
    shell.handle(Command::BlockerReady(profile));
    assert!(operations.lock().unwrap().is_empty());
    shell.handle(Command::Engine(EngineEvent::ContentRulesSettled {
        profile,
        requested,
        settlement: ContentRuleSettlement::Retained {
            generation: retained,
            failure: ContentRuleApplyFailure::NativeCompilation,
        },
    }));
    assert_eq!(
        operations.lock().unwrap().as_slice(),
        &[OperationDisposition {
            operation_id: "enable-native-fail".into(),
            outcome: OperationOutcome::NativeAdmissionFailed,
            reason: OperationReason::ContentPolicyApplyFailed,
        }]
    );
}

#[test]
fn disabling_does_not_wait_behind_an_enabled_compile_in_flight() {
    let (mut shell, _engine, compiler, store, operations) = controlled_shell_with_operation_log();
    store.saved.lock().unwrap().take();
    store.blocker_configs.lock().unwrap().take();
    shell.handle(Command::Bootstrap);
    let profile = shell.windows.focused().unwrap().profile;
    let provisional = initial_installation(&shell, profile);
    shell.handle(Command::Engine(EngineEvent::ContentRulesSettled {
        profile,
        requested: provisional,
        settlement: ContentRuleSettlement::Applied {
            generation: provisional,
        },
    }));
    assert_eq!(compiler.requests.lock().unwrap().len(), 1);
    assert!(compiler.request(0).2.enabled);

    shell.handle(blocker_operation("disable", false));
    assert_eq!(
        compiler.requests.lock().unwrap().len(),
        1,
        "turning protection off needs no compiler"
    );
    shell.handle(Command::BlockerReady(profile));
    let BlockerProfileState::Installing {
        desired: disabled, ..
    } = shell.blocker.profiles[&profile].state
    else {
        panic!("the allow-all must go straight to native installation");
    };
    shell.handle(Command::Engine(EngineEvent::ContentRulesSettled {
        profile,
        requested: disabled,
        settlement: ContentRuleSettlement::Applied {
            generation: disabled,
        },
    }));
    let status = shell.focused_blocker_status_view();
    assert_eq!(status.phase, BlockerPhase::Ready);
    assert_eq!(status.desired_enabled, Some(false));
    assert_eq!(status.applied_enabled, Some(false));
    assert!(operations
        .lock()
        .unwrap()
        .iter()
        .any(|operation| operation.operation_id == "disable"
            && operation.outcome == OperationOutcome::Applied));

    // The replaced compile finishing late changes nothing.
    compiler.complete_next(enabled_rules());
    shell.handle(Command::BlockerReady(profile));
    let status = shell.focused_blocker_status_view();
    assert_eq!(status.phase, BlockerPhase::Ready);
    assert_eq!(status.applied_enabled, Some(false));
}

#[test]
fn disabling_waits_for_the_exact_native_allow_all_settlement() {
    let (mut shell, _engine, compiler, _store, operations) = controlled_shell_with_operation_log();
    shell.handle(Command::Bootstrap);
    let profile = shell.windows.focused().unwrap().profile;

    let initial = initial_installation(&shell, profile);
    shell.handle(Command::BlockerReady(profile));
    shell.handle(Command::Engine(EngineEvent::ContentRulesSettled {
        profile,
        requested: initial,
        settlement: ContentRuleSettlement::Applied {
            generation: initial,
        },
    }));

    shell.handle(blocker_operation("enable", true));
    let enabled = compiler.complete_next(enabled_rules());
    shell.handle(Command::BlockerReady(profile));
    assert!(operations.lock().unwrap().is_empty());
    shell.handle(Command::Engine(EngineEvent::ContentRulesSettled {
        profile,
        requested: enabled,
        settlement: ContentRuleSettlement::Applied {
            generation: enabled,
        },
    }));
    assert_eq!(
        operations.lock().unwrap().as_slice(),
        &[OperationDisposition {
            operation_id: "enable".into(),
            outcome: OperationOutcome::Applied,
            reason: OperationReason::MutationApplied,
        }]
    );

    shell.handle(blocker_operation("disable", false));
    assert_eq!(operations.lock().unwrap().len(), 1);
    let disabled = compiler.complete_next(allow_all());
    shell.handle(Command::BlockerReady(profile));
    assert_eq!(
        operations.lock().unwrap().len(),
        1,
        "compilation is not proof that the native engine replaced the enabled rules"
    );
    let replacing = shell.focused_blocker_status_view();
    assert_eq!(replacing.preference, BlockerPreferenceState::Updating);
    assert_eq!(replacing.phase, BlockerPhase::Installing);
    assert_eq!(replacing.protection, BlockerProtection::Active);
    assert_eq!(replacing.desired_enabled, Some(false));
    assert_eq!(replacing.applied_enabled, Some(true));

    shell.handle(Command::Engine(EngineEvent::ContentRulesSettled {
        profile,
        requested: disabled,
        settlement: ContentRuleSettlement::Applied {
            generation: disabled,
        },
    }));
    assert_eq!(
        operations.lock().unwrap().as_slice(),
        &[
            OperationDisposition {
                operation_id: "enable".into(),
                outcome: OperationOutcome::Applied,
                reason: OperationReason::MutationApplied,
            },
            OperationDisposition {
                operation_id: "disable".into(),
                outcome: OperationOutcome::Applied,
                reason: OperationReason::MutationApplied,
            },
        ]
    );
    let status = shell.focused_blocker_status_view();
    assert_eq!(status.preference, BlockerPreferenceState::Authoritative);
    assert_eq!(status.protection, BlockerProtection::Disabled);
    assert_eq!(status.desired_enabled, Some(false));
    assert_eq!(status.applied_enabled, Some(false));
}

#[test]
fn enabled_source_terminal_does_not_seal_the_allow_all_disable_path() {
    let (mut shell, _engine, compiler, _store, operations) = controlled_shell_with_operation_log();
    shell.handle(Command::Bootstrap);
    let profile = shell.windows.focused().unwrap().profile;

    let initial = initial_installation(&shell, profile);
    shell.handle(Command::BlockerReady(profile));
    shell.handle(Command::Engine(EngineEvent::ContentRulesSettled {
        profile,
        requested: initial,
        settlement: ContentRuleSettlement::Applied {
            generation: initial,
        },
    }));
    shell.handle(blocker_operation("enable", true));
    let enabled = compiler.complete_next(enabled_rules());
    shell.handle(Command::BlockerReady(profile));
    shell.handle(Command::Engine(EngineEvent::ContentRulesSettled {
        profile,
        requested: enabled,
        settlement: ContentRuleSettlement::Applied {
            generation: enabled,
        },
    }));

    let mut terminal = catalog_revision(2, 1);
    terminal.phase =
        BlockerCatalogPhase::Failed(zephium_core::ports::blocker::BlockerCatalogFailure::Internal);
    terminal.enabled_policy_terminal = true;
    compiler.set_catalog(terminal);
    shell.handle(Command::Tick);

    shell.handle(blocker_operation("disable-after-terminal", false));
    let (_, disabled, config) = compiler.request(0);
    assert!(!config.enabled);
    assert_eq!(operations.lock().unwrap().len(), 1);
    assert_eq!(compiler.complete_next(allow_all()), disabled);
    shell.handle(Command::BlockerReady(profile));
    shell.handle(Command::Engine(EngineEvent::ContentRulesSettled {
        profile,
        requested: disabled,
        settlement: ContentRuleSettlement::Applied {
            generation: disabled,
        },
    }));

    assert_eq!(
        operations.lock().unwrap().as_slice(),
        &[
            OperationDisposition {
                operation_id: "enable".into(),
                outcome: OperationOutcome::Applied,
                reason: OperationReason::MutationApplied,
            },
            OperationDisposition {
                operation_id: "disable-after-terminal".into(),
                outcome: OperationOutcome::Applied,
                reason: OperationReason::MutationApplied,
            },
        ]
    );
    let status = shell.focused_blocker_status_view();
    assert_eq!(status.protection, BlockerProtection::Disabled);
    assert_eq!(status.desired_enabled, Some(false));
    assert_eq!(status.applied_enabled, Some(false));
}

#[test]
fn restored_enabled_profile_holds_views_for_its_enabled_policy() {
    let saved = opted_out_store();
    let (mut original, _, _) = setup_with(saved.clone());
    original.handle(Command::Bootstrap);
    original.persist();
    let profile = original.windows.focused().unwrap().profile;

    let (mut shell, engine, compiler, store, screen) = controlled_shell();
    *store.saved.lock().unwrap() = saved.saved.lock().unwrap().clone();
    *store.blocker_configs.lock().unwrap() = Some(vec![ProfileBlockerConfig {
        profile,
        revision: BlockerConfigRevision::INITIAL,
        config: BlockerConfig { enabled: true },
    }]);
    shell.handle(Command::Bootstrap);
    assert!(
        compiler.requests.lock().unwrap().is_empty(),
        "provisional policy must bypass compilation"
    );
    let BlockerProfileState::Installing {
        desired: provisional,
        ..
    } = shell.blocker.profiles[&profile].state
    else {
        panic!("the provisional policy must await exact native installation");
    };
    let status = shell.focused_blocker_status_view();
    assert_eq!(status.desired_enabled, Some(true));
    assert_eq!(status.protection, BlockerProtection::Pending);

    shell.handle(Command::Engine(EngineEvent::ContentRulesSettled {
        profile,
        requested: provisional,
        settlement: ContentRuleSettlement::Applied {
            generation: provisional,
        },
    }));
    assert!(shell.blocker.native_policy_available(profile));
    let (_, enabled, config) = compiler.request(0);
    assert!(config.enabled);
    assert!(enabled > provisional);
    assert_eq!(
        shell.focused_blocker_status_view().protection,
        BlockerProtection::Pending
    );
    shell.handle(Command::Navigate {
        id: active_id(&screen),
        input: "instant.example".into(),
    });
    let created = |engine: &FakeEngine| {
        engine
            .calls()
            .iter()
            .any(|call| call.starts_with("create ") && call.contains("instant.example"))
    };
    assert!(
        !created(&engine),
        "the first page must wait for the enabled policy, not the provisional allow-all"
    );

    // Compilation is not installation. Starting remains visible until the
    // exact enabled generation settles, and a late provisional event is inert.
    compiler.complete_next(enabled_rules());
    shell.handle(Command::BlockerReady(profile));
    assert_eq!(
        shell.focused_blocker_status_view().protection,
        BlockerProtection::Pending
    );
    shell.handle(Command::Engine(EngineEvent::ContentRulesSettled {
        profile,
        requested: provisional,
        settlement: ContentRuleSettlement::Applied {
            generation: provisional,
        },
    }));
    assert_eq!(
        shell.focused_blocker_status_view().protection,
        BlockerProtection::Pending
    );
    shell.handle(Command::Engine(EngineEvent::ContentRulesSettled {
        profile,
        requested: enabled,
        settlement: ContentRuleSettlement::Applied {
            generation: enabled,
        },
    }));
    assert_eq!(
        shell.focused_blocker_status_view().protection,
        BlockerProtection::Active
    );
    assert!(created(&engine), "{:?}", engine.calls());
    assert_eq!(shell.blocker.startup_unprotected_views, 0);
    assert!(shell.blocker.profiles[&profile].config.config.enabled);
}

#[test]
fn startup_deadline_opens_held_views_unprotected_and_counts_them() {
    let (mut shell, engine, compiler, store, screen) = controlled_shell();
    store.saved.lock().unwrap().take();
    store.blocker_configs.lock().unwrap().take();
    shell.handle(Command::Bootstrap);
    let profile = shell.windows.focused().unwrap().profile;
    let provisional = initial_installation(&shell, profile);
    shell.handle(Command::Engine(EngineEvent::ContentRulesSettled {
        profile,
        requested: provisional,
        settlement: ContentRuleSettlement::Applied {
            generation: provisional,
        },
    }));
    shell.handle(Command::Navigate {
        id: active_id(&screen),
        input: "late-protection.example".into(),
    });
    let created = |engine: &FakeEngine| {
        engine
            .calls()
            .iter()
            .filter(|call| call.starts_with("create ") && call.contains("late-protection.example"))
            .count()
    };
    assert_eq!(created(&engine), 0);

    shell.handle(Command::BlockerStartupDeadline {
        profile,
        token: provisional.get() + 1,
    });
    assert_eq!(
        created(&engine),
        0,
        "a stale deadline must not release views"
    );

    shell.handle(Command::BlockerStartupDeadline {
        profile,
        token: provisional.get(),
    });
    assert_eq!(created(&engine), 1, "{:?}", engine.calls());
    assert_eq!(shell.blocker.startup_unprotected_views, 1);
    shell.handle(Command::OpenUrl {
        input: "https://late-protection.example/second".into(),
        new_tab: true,
    });
    assert_eq!(created(&engine), 2, "{:?}", engine.calls());
    assert_eq!(
        shell.blocker.startup_unprotected_views, 2,
        "a view opened after the deadline is counted too"
    );

    let generation = compiler.complete_next(enabled_rules());
    shell.handle(Command::BlockerReady(profile));
    shell.handle(Command::Engine(EngineEvent::ContentRulesSettled {
        profile,
        requested: generation,
        settlement: ContentRuleSettlement::Applied { generation },
    }));
    assert_eq!(
        shell.focused_blocker_status_view().protection,
        BlockerProtection::Active
    );
    assert_eq!(created(&engine), 2);
    assert_eq!(shell.blocker.startup_unprotected_views, 2);
    shell.handle(Command::OpenUrl {
        input: "https://late-protection.example/third".into(),
        new_tab: true,
    });
    assert_eq!(created(&engine), 3);
    assert_eq!(shell.blocker.startup_unprotected_views, 2);
}

#[test]
fn fresh_profile_defaults_on_and_its_first_page_waits_for_protection() {
    let (mut shell, engine, compiler, store, screen) = controlled_shell();
    store.saved.lock().unwrap().take();
    store.blocker_configs.lock().unwrap().take();
    shell.handle(Command::Bootstrap);
    let profile = shell.windows.focused().unwrap().profile;
    assert!(shell.blocker.profiles[&profile].config.config.enabled);
    let provisional = initial_installation(&shell, profile);
    assert!(compiler.requests.lock().unwrap().is_empty());
    shell.handle(Command::Navigate {
        id: active_id(&screen),
        input: "default-protection.example".into(),
    });
    shell.handle(Command::BlockerReady(profile));
    shell.handle(Command::Engine(EngineEvent::ContentRulesSettled {
        profile,
        requested: provisional,
        settlement: ContentRuleSettlement::Applied {
            generation: provisional,
        },
    }));
    let opened = |engine: &FakeEngine| {
        engine
            .calls()
            .iter()
            .any(|call| call.contains("https://default-protection.example/"))
    };
    assert!(!opened(&engine));
    assert!(!shell.blocker.profiles[&profile].held_effects.is_empty());
    assert_eq!(compiler.requests.lock().unwrap().len(), 1);
    assert!(compiler.request(0).2.enabled);
    let generation = compiler.complete_next(enabled_rules());
    shell.handle(Command::BlockerReady(profile));
    shell.handle(Command::Engine(EngineEvent::ContentRulesSettled {
        profile,
        requested: generation,
        settlement: ContentRuleSettlement::Applied { generation },
    }));
    assert_eq!(
        shell.focused_blocker_status_view().protection,
        BlockerProtection::Active
    );
    assert!(opened(&engine));
    assert!(shell.blocker.profiles[&profile].held_effects.is_empty());
}

#[test]
fn first_view_waits_for_exact_native_allow_all_settlement() {
    let (mut shell, engine, compiler, _store, screen) = controlled_shell();
    shell.handle(Command::Bootstrap);
    let profile = shell.windows.focused().unwrap().profile;
    let id = active_id(&screen);
    assert_eq!(
        engine.last_layout(),
        Vec::<String>::new(),
        "the logical first view must remain absent from native layout until policy settlement"
    );
    shell.handle(Command::Navigate {
        id,
        input: "first-policy.example".into(),
    });
    let generation = initial_installation(&shell, profile);
    assert!(compiler.requests.lock().unwrap().is_empty());
    assert!(!engine
        .calls()
        .iter()
        .any(|call| call.starts_with("create ")));
    assert_eq!(shell.blocker.profiles[&profile].held_effects.len(), 1);
    shell.handle(Command::Navigate {
        id: active_id(&screen),
        input: "newest-policy.example".into(),
    });
    assert_eq!(shell.blocker.profiles[&profile].held_effects.len(), 1);

    shell.handle(Command::BlockerReady(profile));
    assert!(engine
        .calls()
        .iter()
        .any(|call| { call == &format!("install-content-rules {profile} {}", generation.get()) }));
    assert!(!engine
        .calls()
        .iter()
        .any(|call| call.starts_with("create ")));

    shell.handle(Command::Engine(EngineEvent::ContentRulesSettled {
        profile,
        requested: generation,
        settlement: ContentRuleSettlement::Applied { generation },
    }));
    assert!(
        engine
            .calls()
            .iter()
            .any(|call| call.contains("https://newest-policy.example/")),
        "{:?}",
        engine.calls()
    );
    assert_eq!(engine.last_layout(), vec![id.to_string()]);
    assert!(!engine
        .calls()
        .iter()
        .any(|call| call.starts_with("navigate ")));
    assert_eq!(
        shell.blocker.profiles[&profile].state,
        BlockerProfileState::Ready {
            applied: generation
        }
    );
}

#[test]
fn close_of_a_materialized_view_is_not_consumed_by_its_held_navigation() {
    let (mut shell, engine, _compiler, _store, screen) = controlled_shell();
    shell.handle(Command::Bootstrap);
    let profile = shell.windows.focused().unwrap().profile;
    let initial = initial_installation(&shell, profile);
    shell.handle(Command::BlockerReady(profile));
    shell.handle(Command::Engine(EngineEvent::ContentRulesSettled {
        profile,
        requested: initial,
        settlement: ContentRuleSettlement::Applied {
            generation: initial,
        },
    }));

    let first = active_id(&screen);
    shell.handle(Command::Open);
    let closing = active_id(&screen);
    assert_ne!(closing, first);
    shell.handle(Command::Navigate {
        id: closing,
        input: "materialized-close.example".into(),
    });
    assert!(
        engine
            .calls()
            .iter()
            .any(|call| call.starts_with(&format!("create {closing} "))),
        "{:?}",
        engine.calls()
    );

    shell.blocker.profiles.get_mut(&profile).unwrap().state = BlockerProfileState::Failed {
        desired: initial,
        retained: None,
        failure: ContentPolicyFailure::CompilerUnavailable,
        retries_remaining: 0,
    };
    let navigate = shell.operation_navigate(closing, "held-close.example".into());
    assert_eq!(navigate.outcome, OperationOutcome::Deferred);
    assert_eq!(shell.blocker.profiles[&profile].held_effects.len(), 1);
    assert!(!engine
        .calls()
        .iter()
        .any(|call| call.starts_with(&format!("navigate {closing} "))));

    let close = shell.operation_close(closing);
    assert_eq!(close.outcome, OperationOutcome::Deferred);
    assert_eq!(close.reason, OperationReason::NativeWorkPending);
    assert!(shell.blocker.profiles[&profile].held_effects.is_empty());
    assert!(
        engine
            .calls()
            .iter()
            .any(|call| call == &format!("close {closing}")),
        "{:?}",
        engine.calls()
    );
}

#[test]
fn an_inflight_policy_generation_cannot_be_displaced_before_exact_settlement() {
    let (mut shell, _engine, compiler, _store, _screen) = controlled_shell();
    shell.handle(Command::Bootstrap);
    let profile = shell.windows.focused().unwrap().profile;
    let first = initial_installation(&shell, profile);

    assert!(!shell
        .blocker
        .start_compile(profile, BlockerConfig { enabled: true }, None));
    assert!(compiler.requests.lock().unwrap().is_empty());
    assert_eq!(
        shell.blocker.profiles[&profile].state,
        BlockerProfileState::Installing {
            desired: first,
            retained: None,
            retries_remaining: 3,
        }
    );

    shell.handle(Command::BlockerReady(profile));
    assert!(!shell
        .blocker
        .start_compile(profile, BlockerConfig { enabled: true }, None));
    assert!(compiler.requests.lock().unwrap().is_empty());
    assert_eq!(
        shell.blocker.profiles[&profile].state,
        BlockerProfileState::Installing {
            desired: first,
            retained: None,
            retries_remaining: 3,
        }
    );

    shell.handle(Command::Engine(EngineEvent::ContentRulesSettled {
        profile,
        requested: first,
        settlement: ContentRuleSettlement::Applied { generation: first },
    }));
    assert_eq!(
        shell.blocker.profiles[&profile].state,
        BlockerProfileState::Ready { applied: first }
    );
    assert_eq!(
        shell.blocker.status(profile).unwrap().applied_config,
        Some(BlockerConfig { enabled: false })
    );
    assert_eq!(
        shell.blocker.status(profile).unwrap().applied_coverage,
        Some(zephium_core::blocker::ContentRuleCoverage::default())
    );
}

#[test]
fn focused_status_reports_only_exact_applied_coverage_and_retained_protection() {
    let (mut shell, _engine, compiler, _store, _screen) = controlled_shell();
    shell.handle(Command::Bootstrap);
    let profile = shell.windows.focused().unwrap().profile;

    let allow_all_generation = initial_installation(&shell, profile);
    shell.handle(Command::BlockerReady(profile));
    shell.handle(Command::Engine(EngineEvent::ContentRulesSettled {
        profile,
        requested: allow_all_generation,
        settlement: ContentRuleSettlement::Applied {
            generation: allow_all_generation,
        },
    }));
    let (reply, result) = std::sync::mpsc::sync_channel(1);
    shell.handle(Command::FocusedContentPolicyStatus { reply });
    let disabled = result.recv().unwrap();
    assert_eq!(disabled.protection, BlockerProtection::Disabled);
    assert_eq!(disabled.phase, BlockerPhase::Ready);
    assert_eq!(disabled.applied_enabled, Some(false));
    assert_eq!(
        disabled.applied_coverage,
        Some(Box::new(BlockerRuleCoverage::default()))
    );

    assert!(shell
        .blocker
        .start_compile(profile, BlockerConfig { enabled: true }, None));
    let enabled_generation = compiler.complete_next(enabled_rules());
    shell.consume_blocker_compile_result(profile);
    let (reply, result) = std::sync::mpsc::sync_channel(1);
    shell.handle(Command::FocusedContentPolicyStatus { reply });
    let installing = result.recv().unwrap();
    assert_eq!(installing.phase, BlockerPhase::Installing);
    assert_eq!(installing.protection, BlockerProtection::Pending);
    assert_eq!(installing.applied_enabled, Some(false));
    assert_eq!(
        installing.applied_coverage,
        Some(Box::new(BlockerRuleCoverage::default()))
    );

    shell.handle(Command::Engine(EngineEvent::ContentRulesSettled {
        profile,
        requested: enabled_generation,
        settlement: ContentRuleSettlement::Applied {
            generation: enabled_generation,
        },
    }));
    let (reply, result) = std::sync::mpsc::sync_channel(1);
    shell.handle(Command::FocusedContentPolicyStatus { reply });
    let active = result.recv().unwrap();
    assert_eq!(active.phase, BlockerPhase::Ready);
    assert_eq!(active.protection, BlockerProtection::Active);
    assert_eq!(active.applied_enabled, Some(true));
    assert_eq!(
        active.applied_coverage,
        Some(Box::new(BlockerRuleCoverage {
            source_rules: 1,
            accepted_rules: 1,
            blocking_rule_entries: 1,
            ..BlockerRuleCoverage::default()
        }))
    );

    assert!(shell
        .blocker
        .start_compile(profile, BlockerConfig { enabled: true }, None));
    let replacement = compiler.complete_next(enabled_rules());
    shell.consume_blocker_compile_result(profile);
    shell.handle(Command::Engine(EngineEvent::ContentRulesSettled {
        profile,
        requested: replacement,
        settlement: ContentRuleSettlement::Retained {
            generation: enabled_generation,
            failure: ContentRuleApplyFailure::NativeCompilation,
        },
    }));
    let (reply, result) = std::sync::mpsc::sync_channel(1);
    shell.handle(Command::FocusedContentPolicyStatus { reply });
    let degraded = result.recv().unwrap();
    assert_eq!(degraded.phase, BlockerPhase::Failed);
    assert_eq!(degraded.protection, BlockerProtection::Degraded);
    assert_eq!(degraded.applied_enabled, Some(true));
    assert_eq!(degraded.failure, Some(BlockerFailure::NativeCompilation));
    assert!(degraded.retryable);
    assert_eq!(
        degraded.applied_coverage, active.applied_coverage,
        "failed replacement retains only the prior exact applied coverage"
    );
    assert!(degraded.projection_revision > active.projection_revision);
}

#[test]
fn disabled_startup_bypasses_an_unavailable_compiler() {
    let (mut shell, engine, compiler, _store, screen) = controlled_shell();
    compiler
        .terminal
        .store(true, std::sync::atomic::Ordering::Release);
    shell.handle(Command::Bootstrap);
    let profile = shell.windows.focused().unwrap().profile;
    let generation = initial_installation(&shell, profile);
    assert!(compiler.requests.lock().unwrap().is_empty());
    shell.handle(Command::Navigate {
        id: active_id(&screen),
        input: "instant-disabled.example".into(),
    });
    shell.handle(Command::Engine(EngineEvent::ContentRulesSettled {
        profile,
        requested: generation,
        settlement: ContentRuleSettlement::Applied { generation },
    }));
    assert!(engine
        .calls()
        .iter()
        .any(|call| call.starts_with("create ") && call.contains("instant-disabled.example")));
    assert_eq!(
        shell.focused_blocker_status_view().protection,
        BlockerProtection::Disabled
    );
    assert!(compiler.requests.lock().unwrap().is_empty());
}

#[test]
fn exact_explicit_retry_recovers_initial_native_failure_without_releasing_the_gate_early() {
    let (mut shell, engine, compiler, _store, screen) = controlled_shell();
    shell.handle(Command::Bootstrap);
    let profile = shell.windows.focused().unwrap().profile;
    shell.handle(Command::Navigate {
        id: active_id(&screen),
        input: "held-until-policy.example".into(),
    });

    let failed = initial_installation(&shell, profile);
    shell.handle(Command::BlockerReady(profile));
    shell.handle(Command::Engine(EngineEvent::ContentRulesSettled {
        profile,
        requested: failed,
        settlement: ContentRuleSettlement::Unavailable {
            failure: zephium_core::blocker::ContentRuleApplyFailure::NativeInstallation,
        },
    }));

    let (reply, status) = std::sync::mpsc::sync_channel(1);
    shell.handle(Command::ContentPolicyStatus { profile, reply });
    assert_eq!(
        status.recv().unwrap(),
        ContentPolicyStatusQueryOutcome::Found(zephium_core::blocker::ProfileContentPolicyStatus {
            profile,
            config_revision: BlockerConfigRevision::INITIAL,
            desired_config: BlockerConfig { enabled: false },
            applied_config: None,
            applied_coverage: None,
            state: BlockerProfileState::Failed {
                desired: failed,
                retained: None,
                failure: ContentPolicyFailure::Native(
                    zephium_core::blocker::ContentRuleApplyFailure::NativeInstallation,
                ),
                retries_remaining: 3,
            },
        })
    );
    assert_eq!(
        shell.blocker.profiles[&profile].state,
        BlockerProfileState::Failed {
            desired: failed,
            retained: None,
            failure: ContentPolicyFailure::Native(
                zephium_core::blocker::ContentRuleApplyFailure::NativeInstallation,
            ),
            retries_remaining: 3,
        }
    );
    let (unknown_reply, unknown_status) = std::sync::mpsc::sync_channel(1);
    shell.handle(Command::ContentPolicyStatus {
        profile: ProfileId::from(999_999),
        reply: unknown_reply,
    });
    assert_eq!(
        unknown_status.recv().unwrap(),
        ContentPolicyStatusQueryOutcome::UnknownProfile
    );
    assert_eq!(shell.blocker.profiles[&profile].held_effects.len(), 1);
    assert!(!engine
        .calls()
        .iter()
        .any(|call| call.starts_with("create ")));

    let accepted = shell.operation_retry_content_policy(profile, failed);
    assert_eq!(accepted.outcome, OperationOutcome::Deferred);
    let retried = compiler.request(0).1;
    assert_ne!(retried, failed);
    assert_eq!(
        shell.blocker.profiles[&profile].state,
        BlockerProfileState::Compiling {
            desired: retried,
            retained: None,
            retries_remaining: 2,
        }
    );

    let duplicate = shell.operation_retry_content_policy(profile, failed);
    assert_eq!(duplicate.outcome, OperationOutcome::Rejected);
    assert_eq!(duplicate.reason, OperationReason::StateUnchanged);
    assert_eq!(compiler.requests.lock().unwrap().len(), 1);

    compiler.complete_next(allow_all());
    shell.handle(Command::BlockerReady(profile));
    shell.handle(Command::Engine(EngineEvent::ContentRulesSettled {
        profile,
        requested: failed,
        settlement: ContentRuleSettlement::Applied { generation: failed },
    }));
    assert!(matches!(
        shell.blocker.profiles[&profile].state,
        BlockerProfileState::Installing {
            desired,
            retries_remaining: 2,
            ..
        } if desired == retried
    ));
    assert!(!engine
        .calls()
        .iter()
        .any(|call| call.starts_with("create ")));

    shell.handle(Command::Engine(EngineEvent::ContentRulesSettled {
        profile,
        requested: retried,
        settlement: ContentRuleSettlement::Applied {
            generation: retried,
        },
    }));
    assert_eq!(
        shell.blocker.profiles[&profile].state,
        BlockerProfileState::Ready { applied: retried }
    );
    assert!(shell.blocker.profiles[&profile].held_effects.is_empty());
    assert!(engine
        .calls()
        .iter()
        .any(|call| call.contains("https://held-until-policy.example/")));
}

#[test]
fn explicit_retry_budget_is_exact_and_never_loops_automatically() {
    let (mut shell, _engine, compiler, _store, _screen) = controlled_shell();
    shell.handle(Command::Bootstrap);
    let profile = shell.windows.focused().unwrap().profile;

    let mut failed = initial_installation(&shell, profile);
    shell.handle(Command::BlockerReady(profile));
    shell.handle(Command::Engine(EngineEvent::ContentRulesSettled {
        profile,
        requested: failed,
        settlement: ContentRuleSettlement::Unavailable {
            failure: zephium_core::blocker::ContentRuleApplyFailure::NativeCompilation,
        },
    }));

    for expected_remaining in [2, 1, 0] {
        let retry = shell.operation_retry_content_policy(profile, failed);
        assert_eq!(retry.outcome, OperationOutcome::Deferred);
        failed = compiler.complete_next(allow_all());
        shell.consume_blocker_compile_result(profile);
        shell.handle(Command::Engine(EngineEvent::ContentRulesSettled {
            profile,
            requested: failed,
            settlement: ContentRuleSettlement::Unavailable {
                failure: zephium_core::blocker::ContentRuleApplyFailure::NativeCompilation,
            },
        }));
        assert!(matches!(
            shell.blocker.profiles[&profile].state,
            BlockerProfileState::Failed {
                desired,
                retries_remaining,
                ..
            } if desired == failed && retries_remaining == expected_remaining
        ));
        assert!(compiler.requests.lock().unwrap().is_empty());
    }

    let exhausted = shell.operation_retry_content_policy(profile, failed);
    assert_eq!(exhausted.outcome, OperationOutcome::Rejected);
    assert_eq!(exhausted.reason, OperationReason::StateUnchanged);
    assert!(compiler.requests.lock().unwrap().is_empty());
}

#[test]
fn source_unavailable_retry_waits_for_exact_material_epoch_advance() {
    let (mut shell, _engine, compiler, _store, _screen) = controlled_shell();
    shell.handle(Command::Bootstrap);
    let profile = shell.windows.focused().unwrap().profile;
    let initial = initial_installation(&shell, profile);
    shell.handle(Command::BlockerReady(profile));
    shell.handle(Command::Engine(EngineEvent::ContentRulesSettled {
        profile,
        requested: initial,
        settlement: ContentRuleSettlement::Applied {
            generation: initial,
        },
    }));
    assert!(shell
        .blocker
        .start_compile(profile, BlockerConfig { enabled: true }, None,));
    let failed = compiler.complete_next(BlockerCompileOutcome::Failed(
        BlockerCompileFailure::SourceUnavailable,
    ));
    shell.handle(Command::BlockerReady(profile));

    let stale = ContentPolicyGeneration::new(failed.get() + 1).unwrap();
    let rejected = shell.operation_retry_content_policy(profile, stale);
    assert_eq!(rejected.outcome, OperationOutcome::Rejected);
    assert!(compiler.requests.lock().unwrap().is_empty());

    let unchanged_material = shell.operation_retry_content_policy(profile, failed);
    assert_eq!(unchanged_material.outcome, OperationOutcome::Rejected);
    assert!(!shell.focused_blocker_status_view().retryable);
    assert!(compiler.requests.lock().unwrap().is_empty());

    let mut repaired = compiler.maintain();
    repaired.revision += 1;
    repaired.source_material_epoch += 1;
    compiler.set_catalog(repaired);
    shell.maintain_blocker_catalog();

    let recovered = compiler.complete_next(enabled_rules());
    shell.consume_blocker_compile_result(profile);
    assert!(matches!(
        shell.blocker.profiles[&profile].state,
        BlockerProfileState::Installing {
            desired,
            retries_remaining: 3,
            ..
        } if desired == recovered
    ));
}

#[test]
fn delayed_second_profile_source_failure_retries_after_existing_epoch_advance() {
    let (mut shell, _engine, compiler, _store, _screen) = controlled_shell();
    shell.handle(Command::Bootstrap);
    let first = shell.windows.focused().unwrap().profile;
    let first_initial = initial_installation(&shell, first);
    shell.handle(Command::BlockerReady(first));
    shell.handle(Command::Engine(EngineEvent::ContentRulesSettled {
        profile: first,
        requested: first_initial,
        settlement: ContentRuleSettlement::Applied {
            generation: first_initial,
        },
    }));
    let second = add_inactive_named_profile(&mut shell, 40_500);
    let entry = shell.blocker.profiles.get_mut(&second).unwrap();
    entry.config.config = BlockerConfig { enabled: false };
    assert!(shell.start_blocker_profile(second));
    let second_initial = initial_installation(&shell, second);
    shell.handle(Command::BlockerReady(second));
    shell.handle(Command::Engine(EngineEvent::ContentRulesSettled {
        profile: second,
        requested: second_initial,
        settlement: ContentRuleSettlement::Applied {
            generation: second_initial,
        },
    }));
    assert!(shell
        .blocker
        .start_compile(first, BlockerConfig { enabled: true }, None,));
    assert!(shell
        .blocker
        .start_compile(second, BlockerConfig { enabled: true }, None,));
    assert_eq!(compiler.requests.lock().unwrap().len(), 2);

    compiler.complete_next(BlockerCompileOutcome::Failed(
        BlockerCompileFailure::SourceUnavailable,
    ));
    shell.handle(Command::BlockerReady(first));

    let mut repaired = compiler.maintain();
    repaired.revision += 1;
    repaired.source_material_epoch += 1;
    compiler.set_catalog(repaired);
    shell.maintain_blocker_catalog();
    assert_eq!(compiler.requests.lock().unwrap().len(), 2);
    assert_eq!(compiler.request(0).0, second);
    assert_eq!(compiler.request(1).0, first);

    let delayed = compiler.complete_next(BlockerCompileOutcome::Failed(
        BlockerCompileFailure::SourceUnavailable,
    ));
    shell.handle(Command::BlockerReady(second));

    assert_eq!(compiler.requests.lock().unwrap().len(), 2);
    assert_eq!(compiler.request(0).0, first);
    assert_eq!(compiler.request(1).0, second);
    assert!(matches!(
        shell.blocker.profiles[&second].state,
        BlockerProfileState::Compiling { desired, .. }
            if desired == compiler.request(1).1 && desired != delayed
    ));
}

#[test]
fn rejected_compiler_admission_is_typed_and_can_be_explicitly_retried() {
    let (mut shell, _engine, compiler, _store, _screen) = controlled_shell();
    compiler
        .reject
        .store(true, std::sync::atomic::Ordering::Release);
    shell.handle(Command::Bootstrap);
    let profile = add_inactive_named_profile(&mut shell, 40_001);
    assert!(!shell
        .blocker
        .start_compile(profile, BlockerConfig { enabled: false }, None));
    let BlockerProfileState::Failed {
        desired: failed,
        retained: None,
        failure: ContentPolicyFailure::CompilerDispatchRejected,
        retries_remaining: 3,
    } = shell.blocker.profiles[&profile].state
    else {
        panic!("compiler rejection must remain a typed recoverable failure");
    };

    compiler
        .reject
        .store(false, std::sync::atomic::Ordering::Release);
    shell.handle(Command::RetryContentPolicy {
        profile,
        failed_generation: failed,
    });
    assert!(compiler.requests.lock().unwrap().is_empty());
    let accepted = shell.operation_retry_content_policy(profile, failed);
    assert_eq!(accepted.outcome, OperationOutcome::Deferred);
    assert_eq!(compiler.requests.lock().unwrap().len(), 1);
}

#[test]
fn terminal_compiler_dispatch_is_global_typed_and_preserves_a_known_good_policy() {
    let (mut shell, _engine, compiler, _store, _screen) = controlled_shell();
    shell.handle(Command::Bootstrap);
    let profile = shell.windows.focused().unwrap().profile;
    let first = initial_installation(&shell, profile);
    shell.handle(Command::BlockerReady(profile));
    shell.handle(Command::Engine(EngineEvent::ContentRulesSettled {
        profile,
        requested: first,
        settlement: ContentRuleSettlement::Applied { generation: first },
    }));

    compiler
        .terminal
        .store(true, std::sync::atomic::Ordering::Release);
    assert!(!shell
        .blocker
        .start_compile(profile, BlockerConfig { enabled: true }, None));
    let status = shell.blocker.status(profile).unwrap();
    let BlockerProfileState::Failed { desired, .. } = status.state else {
        panic!("terminal compiler dispatch must fail the desired generation");
    };
    assert_eq!(
        status.state,
        BlockerProfileState::Failed {
            desired,
            retained: Some(first),
            failure: ContentPolicyFailure::CompilerUnavailable,
            retries_remaining: 0,
        }
    );
    assert_eq!(
        status.applied_config,
        Some(BlockerConfig { enabled: false })
    );
    assert!(shell.blocker.native_policy_available(profile));
    assert_eq!(shell.blocker.compiler_inbox_state(), (true, 0));
    assert_eq!(
        shell
            .operation_retry_content_policy(profile, desired)
            .outcome,
        OperationOutcome::Rejected
    );
}

#[test]
fn arbitrary_profile_retirement_churn_does_not_terminalize_the_compiler() {
    let (mut shell, engine, compiler, _store, _screen) = controlled_shell();
    shell.handle(Command::Bootstrap);
    // Exercise worker lifetimes independently of the local startup artifact.
    let profile = add_inactive_named_profile(&mut shell, 40_001);
    assert!(shell
        .blocker
        .start_compile(profile, BlockerConfig { enabled: false }, None));
    let compiling = compiler.request(0).1;

    for offset in 0..=(zephium_core::session::MAX_SESSION_PROFILES * 2) {
        shell
            .blocker
            .retire_profile(ProfileId::from(50_000 + offset as u128));
        compiler.complete_retirement();
    }

    assert_eq!(shell.blocker.compiler_inbox_state(), (false, 0));
    assert_eq!(
        shell.blocker.profiles[&profile].state,
        BlockerProfileState::Compiling {
            desired: compiling,
            retained: None,
            retries_remaining: 3,
        }
    );

    compiler.complete_next(allow_all());
    shell.handle(Command::BlockerReady(profile));
    assert!(engine
        .calls()
        .iter()
        .any(|call| call.starts_with(&format!("install-content-rules {profile} "))));

    let later = add_inactive_named_profile(&mut shell, 60_000);
    assert!(shell.start_blocker_profile(later));
    assert!(matches!(
        shell.blocker.profiles[&later].state,
        BlockerProfileState::Installing {
            retained: None,
            retries_remaining: 3,
            ..
        }
    ));
    assert!(compiler.requests.lock().unwrap().is_empty());
}

#[test]
fn compiler_artifact_must_match_the_exact_enabled_preference() {
    let (mut shell, engine, compiler, _store, _screen) = controlled_shell();
    shell.handle(Command::Bootstrap);
    // Exercise worker lifetimes independently of the local startup artifact.
    let profile = add_inactive_named_profile(&mut shell, 40_001);
    assert!(shell
        .blocker
        .start_compile(profile, BlockerConfig { enabled: false }, None));
    let generation = compiler.complete_next(enabled_rules());
    shell.handle(Command::BlockerReady(profile));

    assert_eq!(
        shell.blocker.profiles[&profile].state,
        BlockerProfileState::Failed {
            desired: generation,
            retained: None,
            failure: ContentPolicyFailure::CompiledArtifactMismatch,
            retries_remaining: 0,
        }
    );
    assert!(!engine
        .calls()
        .iter()
        .any(|call| call.starts_with(&format!("install-content-rules {profile} "))));
}

#[test]
fn a_stale_native_settlement_cannot_replace_the_desired_generation() {
    let (mut shell, _engine, compiler, _store, _screen) = controlled_shell();
    shell.handle(Command::Bootstrap);
    let profile = shell.windows.focused().unwrap().profile;
    let retired = initial_installation(&shell, profile);
    shell.consume_blocker_compile_result(profile);
    shell.handle(Command::Engine(EngineEvent::ContentRulesSettled {
        profile,
        requested: retired,
        settlement: ContentRuleSettlement::Unavailable {
            failure: zephium_core::blocker::ContentRuleApplyFailure::NativeCompilation,
        },
    }));
    assert_eq!(
        shell
            .operation_retry_content_policy(profile, retired)
            .outcome,
        OperationOutcome::Deferred
    );
    let desired = compiler.request(0).1;

    compiler.complete_next(allow_all());
    shell.consume_blocker_compile_result(profile);
    shell.handle(Command::Engine(EngineEvent::ContentRulesSettled {
        profile,
        requested: retired,
        settlement: ContentRuleSettlement::Applied {
            generation: retired,
        },
    }));
    assert_eq!(
        shell.blocker.profiles[&profile].state,
        BlockerProfileState::Installing {
            desired,
            retained: None,
            retries_remaining: 2,
        }
    );
}

#[test]
fn failed_replacement_retains_last_known_good_policy_and_view_admission() {
    let (mut shell, engine, compiler, _store, _screen) = controlled_shell();
    shell.handle(Command::Bootstrap);
    let profile = shell.windows.focused().unwrap().profile;
    let first = initial_installation(&shell, profile);
    shell.handle(Command::BlockerReady(profile));
    shell.handle(Command::Engine(EngineEvent::ContentRulesSettled {
        profile,
        requested: first,
        settlement: ContentRuleSettlement::Applied { generation: first },
    }));

    assert!(shell
        .blocker
        .start_compile(profile, BlockerConfig { enabled: true }, None,));
    let replacement = compiler.complete_next(enabled_rules());
    shell.consume_blocker_compile_result(profile);
    shell.handle(Command::Engine(EngineEvent::ContentRulesSettled {
        profile,
        requested: replacement,
        settlement: ContentRuleSettlement::Retained {
            generation: first,
            failure: zephium_core::blocker::ContentRuleApplyFailure::NativeCompilation,
        },
    }));
    assert_eq!(
        shell.blocker.profiles[&profile].state,
        BlockerProfileState::Failed {
            desired: replacement,
            retained: Some(first),
            failure: ContentPolicyFailure::Native(
                zephium_core::blocker::ContentRuleApplyFailure::NativeCompilation,
            ),
            retries_remaining: 3,
        }
    );
    let status = shell.blocker.status(profile).unwrap();
    assert_eq!(status.desired_config, BlockerConfig { enabled: true });
    assert_eq!(
        status.applied_config,
        Some(BlockerConfig { enabled: false })
    );

    let creates_before = engine
        .calls()
        .iter()
        .filter(|call| call.starts_with("create "))
        .count();
    shell.handle(Command::Open);
    shell.handle(Command::Navigate {
        id: shell.windows.focused().unwrap().active.unwrap(),
        input: "retained-policy.example".into(),
    });
    assert_eq!(
        engine
            .calls()
            .iter()
            .filter(|call| call.starts_with("create "))
            .count(),
        creates_before + 1,
        "{:?}",
        engine.calls()
    );
}

#[test]
fn native_cleanup_failure_never_treats_a_claimed_prior_generation_as_safe() {
    let (mut shell, _engine, compiler, _store, _screen) = controlled_shell();
    shell.handle(Command::Bootstrap);
    let profile = shell.windows.focused().unwrap().profile;
    let first = initial_installation(&shell, profile);
    shell.handle(Command::BlockerReady(profile));
    shell.handle(Command::Engine(EngineEvent::ContentRulesSettled {
        profile,
        requested: first,
        settlement: ContentRuleSettlement::Applied { generation: first },
    }));

    assert!(shell
        .blocker
        .start_compile(profile, BlockerConfig { enabled: true }, None));
    let replacement = compiler.complete_next(enabled_rules());
    shell.consume_blocker_compile_result(profile);
    shell.handle(Command::Engine(EngineEvent::ContentRulesSettled {
        profile,
        requested: replacement,
        settlement: ContentRuleSettlement::Retained {
            generation: first,
            failure: zephium_core::blocker::ContentRuleApplyFailure::NativeCleanup,
        },
    }));

    assert_eq!(
        shell.blocker.profiles[&profile].state,
        BlockerProfileState::Failed {
            desired: replacement,
            retained: None,
            failure: ContentPolicyFailure::Native(
                zephium_core::blocker::ContentRuleApplyFailure::NativeCleanup,
            ),
            retries_remaining: 0,
        }
    );
    assert!(!shell.blocker.native_policy_available(profile));
}

#[test]
fn exact_blocker_config_cohort_is_required_before_compilation() {
    let (mut shell, _engine, compiler, _store, _screen) = controlled_shell();
    let first = ProfileId::from(100);
    let second = ProfileId::from(101);
    for profile in [first, second] {
        assert!(shell.profiles.insert(Profile {
            id: profile,
            name: profile.to_string(),
            kind: ProfileKind::Named,
        }));
    }
    let config = |profile| ProfileBlockerConfig {
        profile,
        revision: BlockerConfigRevision::INITIAL,
        config: BlockerConfig { enabled: false },
    };
    assert!(!shell.initialize_blocker_cohort(vec![config(first)]));
    assert!(!shell.initialize_blocker_cohort(vec![config(first), config(first),]));
    assert!(!shell.initialize_blocker_cohort(vec![config(first), config(ProfileId::from(999)),]));
    assert!(shell.initialize_blocker_cohort(vec![config(second), config(first)]));
    assert!(shell.initialize_blocker_cohort(vec![config(first), config(second)]));
    let mut changed = config(first);
    changed.config.enabled = true;
    assert!(!shell.initialize_blocker_cohort(vec![changed, config(second)]));
    assert!(compiler.requests.lock().unwrap().is_empty());
}

#[test]
fn bootstrap_never_invents_a_missing_durable_profile_policy() {
    let store = opted_out_store();
    let (mut first, _engine, _screen) = setup_with(store.clone());
    first.handle(Command::Bootstrap);
    first.handle(Command::Persist);
    *store.blocker_configs.lock().unwrap() = Some(Vec::new());

    let (mut restarted, engine, screen) = setup_with(store);
    restarted.handle(Command::Bootstrap);

    assert!(!restarted.bootstrapped);
    assert!(restarted.windows.focused().is_none());
    assert!(last(&screen).tabs.is_empty());
    assert!(!engine
        .calls()
        .iter()
        .any(|call| call.starts_with("create ")));
}

#[test]
fn retirement_suppresses_a_late_compiler_callback_and_drops_held_work() {
    let (mut shell, engine, compiler, _store, _screen) = controlled_shell();
    shell.handle(Command::Bootstrap);
    // Exercise worker lifetimes independently of the local startup artifact.
    let profile = add_inactive_named_profile(&mut shell, 40_001);
    assert!(shell
        .blocker
        .start_compile(profile, BlockerConfig { enabled: false }, None));
    shell.blocker.retire_profile(profile);
    compiler.complete_next(allow_all());
    shell.handle(Command::BlockerReady(profile));

    assert_eq!(
        shell.blocker.profiles[&profile].state,
        BlockerProfileState::Retired
    );
    assert!(shell.blocker.inbox.lock().unwrap().results.is_empty());
    assert!(!engine.calls().iter().any(|call| {
        call.starts_with(&format!("install-content-rules {profile} "))
            || call.starts_with("create ")
    }));
}

#[test]
fn retirement_barrier_removes_a_late_result_even_when_its_wake_is_lost() {
    let (mut shell, _engine, compiler, _store, _screen) = controlled_shell();
    shell.handle(Command::Bootstrap);
    // Exercise worker lifetimes independently of the local startup artifact.
    let profile = add_inactive_named_profile(&mut shell, 40_001);
    assert!(shell
        .blocker
        .start_compile(profile, BlockerConfig { enabled: false }, None));
    shell.blocker.retire_profile(profile);

    // Model a callback which crossed delivery before retirement and then a
    // saturated/lost shell wake: the result reaches the bounded inbox but the
    // command is deliberately never handled.
    compiler.complete_next(allow_all());
    assert!(shell
        .blocker
        .inbox
        .lock()
        .unwrap()
        .results
        .contains_key(&profile));

    compiler.complete_retirement();
    assert!(!shell
        .blocker
        .inbox
        .lock()
        .unwrap()
        .results
        .contains_key(&profile));
    assert_eq!(
        shell.blocker.profiles[&profile].state,
        BlockerProfileState::Retired
    );
}

#[test]
fn stale_result_from_a_retired_lifecycle_cannot_affect_recreated_same_id_profile() {
    let (mut shell, engine, compiler, _store, _screen) = controlled_shell();
    shell.handle(Command::Bootstrap);
    // Exercise worker lifetimes independently of the local startup artifact.
    let profile = add_inactive_named_profile(&mut shell, 40_001);
    assert!(shell
        .blocker
        .start_compile(profile, BlockerConfig { enabled: false }, None));
    let old_generation = compiler.request(0).1;

    shell.blocker.discard_uncommitted_profile(profile);
    assert!(shell.blocker.initialize_new_profile(profile));
    // The controlled adapter intentionally admits this before its retirement
    // callback; the production worker rejects that window. Exercising the
    // stronger adversarial ordering proves the app does not rely solely on
    // adapter quiescence.
    assert!(shell
        .blocker
        .start_compile(profile, BlockerConfig { enabled: false }, None));
    let new_generation = compiler.request(1).1;
    assert!(new_generation > old_generation);

    assert_eq!(compiler.complete_next(allow_all()), old_generation);
    shell.handle(Command::BlockerReady(profile));
    assert_eq!(
        shell.blocker.profiles[&profile].state,
        BlockerProfileState::Compiling {
            desired: new_generation,
            retained: None,
            retries_remaining: 3,
        }
    );
    assert!(!engine
        .calls()
        .iter()
        .any(|call| call.starts_with(&format!("install-content-rules {profile} "))));

    assert_eq!(compiler.complete_next(allow_all()), new_generation);
    shell.handle(Command::BlockerReady(profile));
    assert_eq!(
        shell.blocker.profiles[&profile].state,
        BlockerProfileState::Installing {
            desired: new_generation,
            retained: None,
            retries_remaining: 3,
        }
    );
    assert!(engine
        .calls()
        .iter()
        .any(|call| call == &format!("install-content-rules {profile} {}", new_generation.get())));
}

#[test]
fn durable_profile_deletion_retires_compiler_work_before_native_erasure() {
    let (mut shell, engine, compiler, store, _screen) = controlled_shell();
    shell.handle(Command::Bootstrap);
    let profile = add_inactive_named_profile(&mut shell, 41_000);
    store
        .authorize_outcomes
        .lock()
        .unwrap()
        .push_back(ProfileDeletionAuthorizeOutcome::Authorized);
    store
        .finalize_outcomes
        .lock()
        .unwrap()
        .push_back(ProfileDeletionFinalizeOutcome::Completed);
    engine.push_erasure_outcomes([ProfileDataErasureOutcome::Verified]);

    shell.handle(delete_operation("delete-with-blocker", profile));

    assert!(compiler.retired.lock().unwrap().contains(&profile));
    assert!(engine
        .calls()
        .iter()
        .any(|call| call == &format!("erase-profile {profile}")));
    assert_eq!(
        shell.blocker.profiles[&profile].state,
        BlockerProfileState::Retired
    );
}

#[test]
fn profile_deletion_wins_over_a_stale_explicit_policy_retry() {
    let (mut shell, engine, compiler, store, _screen) = controlled_shell();
    shell.handle(Command::Bootstrap);
    // Complete the focused profile so the controlled queue contains only the
    // independently managed profile used by this race.
    let focused = shell.windows.focused().unwrap().profile;
    let focused_generation = initial_installation(&shell, focused);
    shell.handle(Command::BlockerReady(focused));
    shell.handle(Command::Engine(EngineEvent::ContentRulesSettled {
        profile: focused,
        requested: focused_generation,
        settlement: ContentRuleSettlement::Applied {
            generation: focused_generation,
        },
    }));

    let profile = add_inactive_named_profile(&mut shell, 42_000);
    assert!(shell
        .blocker
        .start_compile(profile, BlockerConfig { enabled: false }, None));
    let failed = compiler.complete_next(BlockerCompileOutcome::Failed(
        BlockerCompileFailure::Internal,
    ));
    shell.consume_blocker_compile_result(profile);
    assert_eq!(
        shell
            .operation_retry_content_policy(profile, failed)
            .outcome,
        OperationOutcome::Deferred
    );
    let retried = compiler.request(0).1;

    store
        .authorize_outcomes
        .lock()
        .unwrap()
        .push_back(ProfileDeletionAuthorizeOutcome::Authorized);
    store
        .finalize_outcomes
        .lock()
        .unwrap()
        .push_back(ProfileDeletionFinalizeOutcome::Completed);
    engine.push_erasure_outcomes([ProfileDataErasureOutcome::Verified]);
    shell.handle(delete_operation("delete-policy-race", profile));
    assert_eq!(
        shell.blocker.profiles[&profile].state,
        BlockerProfileState::Retired
    );
    let (reply, status) = std::sync::mpsc::sync_channel(1);
    shell.handle(Command::ContentPolicyStatus { profile, reply });
    assert!(matches!(
        status.recv().unwrap(),
        ContentPolicyStatusQueryOutcome::Found(zephium_core::blocker::ProfileContentPolicyStatus {
            state: BlockerProfileState::Retired,
            ..
        })
    ));

    let requests_before = compiler.requests.lock().unwrap().len();
    let retry = shell.operation_retry_content_policy(profile, failed);
    assert_eq!(retry.outcome, OperationOutcome::Rejected);
    assert_eq!(retry.reason, OperationReason::InvalidScope);
    assert_eq!(compiler.requests.lock().unwrap().len(), requests_before);

    compiler.complete_next(allow_all());
    shell.handle(Command::BlockerReady(profile));
    shell.handle(Command::Engine(EngineEvent::ContentRulesSettled {
        profile,
        requested: retried,
        settlement: ContentRuleSettlement::Applied {
            generation: retried,
        },
    }));
    assert_eq!(
        shell.blocker.profiles[&profile].state,
        BlockerProfileState::Retired
    );
    assert!(!engine
        .calls()
        .iter()
        .any(|call| { call == &format!("install-content-rules {profile} {}", retried.get()) }));
}

#[test]
fn shutdown_makes_late_retry_completion_and_settlement_inert() {
    let (mut shell, engine, compiler, _store, _screen) = controlled_shell();
    shell.handle(Command::Bootstrap);
    // Exercise worker lifetimes independently of the local startup artifact.
    let profile = add_inactive_named_profile(&mut shell, 40_001);
    assert!(shell
        .blocker
        .start_compile(profile, BlockerConfig { enabled: false }, None));
    let failed = compiler.complete_next(BlockerCompileOutcome::Failed(
        BlockerCompileFailure::Internal,
    ));
    shell.consume_blocker_compile_result(profile);
    assert_eq!(
        shell
            .operation_retry_content_policy(profile, failed)
            .outcome,
        OperationOutcome::Deferred
    );
    let retried = compiler.request(0).1;

    let (ack, outcome) = std::sync::mpsc::sync_channel(1);
    shell.handle(Command::Shutdown {
        deadline: test_shutdown_deadline(),
        ack,
    });
    assert_eq!(outcome.recv().unwrap(), ShutdownOutcome::Clean);

    compiler.complete_next(allow_all());
    shell.handle(Command::BlockerReady(profile));
    shell.handle(Command::Engine(EngineEvent::ContentRulesSettled {
        profile,
        requested: retried,
        settlement: ContentRuleSettlement::Applied {
            generation: retried,
        },
    }));
    assert!(!engine
        .calls()
        .iter()
        .any(|call| call == &format!("install-content-rules {profile} {}", retried.get())));
}

#[test]
fn compiler_shutdown_failure_is_terminal_and_does_not_skip_native_teardown() {
    let (mut shell, engine, compiler, _store, _screen) = controlled_shell();
    let order = Arc::new(Mutex::new(Vec::new()));
    *compiler.shutdown_order.lock().unwrap() = Some(order.clone());
    *engine.shutdown_order.lock().unwrap() = Some(order.clone());
    compiler
        .shutdown_unclean
        .store(true, std::sync::atomic::Ordering::Release);
    shell.handle(Command::Bootstrap);
    let (ack, outcome) = std::sync::mpsc::sync_channel(1);
    shell.handle(Command::Shutdown {
        deadline: test_shutdown_deadline(),
        ack,
    });
    assert_eq!(outcome.recv().unwrap(), ShutdownOutcome::Unclean);
    assert_eq!(
        compiler
            .shutdowns
            .load(std::sync::atomic::Ordering::Acquire),
        1
    );
    assert_eq!(
        engine
            .shutdown_calls
            .load(std::sync::atomic::Ordering::Acquire),
        1
    );
    assert_eq!(order.lock().unwrap().as_slice(), &["engine", "blocker"]);
}

#[test]
fn official_https_freshness_needs_exact_verified_material_and_preserves_signed_expiry_rules() {
    use zephium_core::ports::blocker::BlockerCatalogProvenance;
    let mut stale = catalog_revision(10, 10);
    stale.package_provenance = Some(BlockerCatalogProvenance::OfficialHttps);
    stale.installed_provenance = Some(BlockerCatalogProvenance::OfficialHttps);
    stale.phase = BlockerCatalogPhase::Stale;
    stale.package_stale = Some(true);
    stale.source_refresh_due = true;
    let (mut shell, compiler) = shell_with_initial_catalog(stale);
    shell.handle(Command::Bootstrap);
    let mut checked = stale;
    checked.revision += 1;
    checked.phase = BlockerCatalogPhase::Fresh;
    checked.package_stale = Some(false);
    checked.source_refresh_due = false;
    checked.source_material_epoch = 1;
    compiler.set_catalog(checked);
    shell.maintain_blocker_catalog();
    assert_eq!(
        shell.focused_blocker_status_view().source_phase,
        BlockerSourcePhase::Fresh
    );
    assert!(shell.focused_blocker_status_view().can_enable);
    let (mut unverified, compiler) = shell_with_initial_catalog(stale);
    unverified.handle(Command::Bootstrap);
    checked.source_material_epoch = 0;
    compiler.set_catalog(checked);
    unverified.maintain_blocker_catalog();
    assert_eq!(
        unverified.focused_blocker_status_view().source_failure,
        Some(BlockerSourceFailure::Rollback)
    );
    stale.package_provenance = Some(BlockerCatalogProvenance::TufRepository);
    stale.installed_provenance = Some(BlockerCatalogProvenance::TufRepository);
    let (mut signed, compiler) = shell_with_initial_catalog(stale);
    signed.handle(Command::Bootstrap);
    checked.package_provenance = stale.package_provenance;
    checked.installed_provenance = stale.installed_provenance;
    checked.source_material_epoch = 1;
    compiler.set_catalog(checked);
    signed.maintain_blocker_catalog();
    assert_eq!(
        signed.focused_blocker_status_view().source_failure,
        Some(BlockerSourceFailure::Rollback)
    );
}

#[test]
fn pending_source_candidate_does_not_withhold_the_working_compiler_catalog() {
    let mut catalog = catalog_revision(10, 10);
    set_catalog_candidate(&mut catalog, 11);
    let (mut shell, _) = shell_with_initial_catalog(catalog);
    shell.handle(Command::Bootstrap);
    assert!(shell.focused_blocker_status_view().can_enable);
}
