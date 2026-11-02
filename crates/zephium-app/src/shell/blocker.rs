//! Profile-scoped content-policy compilation and native-install coordination.

use super::*;

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, VecDeque};

use zephium_core::blocker::{
    BlockerCompileFailure, BlockerConfig, BlockerConfigRevision, ContentPolicyFailure,
    ContentPolicyGeneration, ContentRuleApplyFailure, ContentRuleCoverage, ContentRuleDigest,
    NetworkPolicyDiagnostics, NetworkRequestPolicy, ProfileBlockerConfig,
    ProfileContentPolicyState, ProfileContentPolicyStatus,
};
use zephium_core::ports::blocker::{
    BlockerCatalogFailure, BlockerCatalogPhase, BlockerCatalogProvenance,
    BlockerCatalogRefreshDispatch, BlockerCatalogSnapshot, BlockerCompileOutcome, BlockerDispatch,
    BlockerRetirementDispatch, BlockerShutdownOutcome,
};
use zephium_core::ports::engine::ContentRuleSettlement;
use zephium_core::ports::store::{BlockerConfigLoadOutcome, BlockerConfigUpdateOutcome};

const MAX_HELD_EFFECTS_PER_PROFILE: usize = zephium_core::session::MAX_SESSION_ITEMS * 2;
const MAX_EXPLICIT_POLICY_RETRIES: u8 = 3;
const MAX_PREFERENCE_RECONCILE_ATTEMPTS: u8 = 3;
const PREFERENCE_RECONCILE_RETRY_BASE: std::time::Duration = std::time::Duration::from_millis(250);
const CATALOG_POLL_INITIAL: std::time::Duration = std::time::Duration::from_millis(250);
const CATALOG_POLL_MAX: std::time::Duration = std::time::Duration::from_secs(2);
const CATALOG_REFRESH_OPERATION_TIMEOUT: std::time::Duration =
    std::time::Duration::from_secs(5 * 60);
pub(super) const INTERNAL_CATALOG_POLL_OPERATION: u64 = 0;
const COMPILE_CALLBACK_PUBLISHED: u8 = 1 << 0;
const COMPILE_CALL_RETURNED: u8 = 1 << 1;

pub(super) type BlockerResultInbox = Arc<Mutex<BlockerInbox>>;

#[derive(Default)]
struct CompileWakeHandoff {
    state: std::sync::atomic::AtomicU8,
}

impl CompileWakeHandoff {
    /// Returns true only for the party that observes both publication and
    /// return. Exactly that party owns the wake, including the completion-
    /// before-return interleaving.
    fn callback_published(&self) -> bool {
        self.state.fetch_or(
            COMPILE_CALLBACK_PUBLISHED,
            std::sync::atomic::Ordering::AcqRel,
        ) & COMPILE_CALL_RETURNED
            != 0
    }

    fn call_returned(&self) -> bool {
        self.state
            .fetch_or(COMPILE_CALL_RETURNED, std::sync::atomic::Ordering::AcqRel)
            & COMPILE_CALLBACK_PUBLISHED
            != 0
    }
}

#[derive(Default)]
pub(super) struct BlockerInbox {
    pub(super) results: HashMap<ProfileId, PendingCompileResult>,
    store_results: HashMap<ProfileId, PendingStoreResult>,
    pub(super) site_results: HashMap<ProfileId, super::blocker_sites::SiteReplySlot>,
}

pub(super) type BlockerProfileState = ProfileContentPolicyState;

pub(super) struct BlockerProfile {
    pub(super) sites: super::blocker_sites::SitePreferencesState,
    pub(super) config: ProfileBlockerConfig,
    pub(super) state: BlockerProfileState,
    pub(super) held_effects: VecDeque<Effect>,
    desired_config: BlockerConfig,
    applied_config: Option<BlockerConfig>,
    pending_coverage: Option<(ContentPolicyGeneration, ContentRuleCoverage)>,
    applied_coverage: Option<ContentRuleCoverage>,
    pending_runtime_policy: Option<(ContentPolicyGeneration, RuntimePolicyObserver)>,
    applied_runtime_policy: Option<RuntimePolicyObserver>,
    preference: BlockerPreferenceAuthority,
    pending_mutation: Option<PendingBlockerMutation>,
    background_target: Option<BlockerConfig>,
    pending_catalog_revision: Option<u64>,
    compiling_catalog: Option<CatalogCompileAttempt>,
    applied_catalog_revision: Option<u64>,
    /// A locally constructed allow-all is being installed before the enabled
    /// startup target. This never changes the durable preference.
    startup_provisional: bool,
}

pub(super) struct RuntimePolicyObserver {
    pub(super) digest: ContentRuleDigest,
    pub(super) policy: std::sync::Weak<dyn NetworkRequestPolicy>,
}

pub(super) struct PendingCompileResult {
    generation: ContentPolicyGeneration,
    outcome: BlockerCompileOutcome,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BlockerPreferenceAuthority {
    Authoritative,
    Updating,
    Reconciling {
        token: u64,
        attempts_remaining: u8,
        retry_at: Option<std::time::Instant>,
    },
    Unavailable,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PendingMutationPhase {
    Store {
        token: u64,
    },
    Native {
        generation: Option<ContentPolicyGeneration>,
    },
}

pub(super) struct PendingBlockerMutation {
    operation_id: String,
    expected_revision: BlockerConfigRevision,
    requested: BlockerConfig,
    phase: PendingMutationPhase,
    success_outcome: OperationOutcome,
}

struct PendingCatalogRefresh {
    operation_id: String,
    service_operation: u64,
    after_catalog_revision: u64,
    next_poll_attempt: u8,
    deadline: std::time::Instant,
}

#[derive(Clone, Copy)]
struct PendingCatalogActivationPoll {
    catalog_revision: u64,
    next_attempt: u8,
    deadline: std::time::Instant,
}

pub(super) struct PendingStoreResult {
    token: u64,
    outcome: PendingStoreOutcome,
}

pub(super) enum PendingStoreOutcome {
    Mutation(BlockerConfigUpdateOutcome),
    Reconciliation(BlockerConfigLoadOutcome),
}

fn publish_store_result(
    inbox: &BlockerResultInbox,
    profile: ProfileId,
    result: PendingStoreResult,
) {
    let mut inbox = inbox
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if inbox.store_results.len() >= zephium_core::session::MAX_SESSION_PROFILES
        && !inbox.store_results.contains_key(&profile)
    {
        return;
    }
    match inbox.store_results.entry(profile) {
        std::collections::hash_map::Entry::Occupied(mut entry)
            if entry.get().token < result.token =>
        {
            entry.insert(result);
        }
        std::collections::hash_map::Entry::Vacant(entry) => {
            entry.insert(result);
        }
        std::collections::hash_map::Entry::Occupied(_) => {}
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FocusedBlockerStatus {
    Unavailable {
        catalog: BlockerCatalogSnapshot,
    },
    Found {
        status: ProfileContentPolicyStatus,
        preference: BlockerPreferenceAuthority,
        applied_catalog_revision: Option<u64>,
        source_material_retry_ready: bool,
        catalog: BlockerCatalogSnapshot,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct CatalogPackageIdentity {
    revision: u64,
    manifest_sha256: [u8; 32],
    created_unix: u64,
    expires_unix: u64,
    source_count: u32,
    source_bytes: u64,
    provenance: BlockerCatalogProvenance,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct CatalogInstalledIdentity {
    revision: u64,
    manifest_sha256: [u8; 32],
    provenance: BlockerCatalogProvenance,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct CatalogCompileAttempt {
    generation: ContentPolicyGeneration,
    installed: CatalogInstalledIdentity,
    source_material_epoch: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct CatalogAuthority {
    current: Option<CatalogPackageIdentity>,
    high_water: Option<CatalogPackageIdentity>,
    installed: Option<CatalogInstalledIdentity>,
}

pub(super) struct BlockerCoordinator {
    pub(super) profiles: HashMap<ProfileId, BlockerProfile>,
    pub(super) inbox: BlockerResultInbox,
    pub(super) service: SharedBlocker,
    catalog: BlockerCatalogSnapshot,
    catalog_authority: CatalogAuthority,
    next_generation: Option<u64>,
    next_store_token: Option<u64>,
    pending_catalog_refresh: Option<PendingCatalogRefresh>,
    pending_catalog_activation_poll: Option<PendingCatalogActivationPoll>,
    exhausted_catalog_activation_poll_revision: Option<u64>,
    compiler_terminal: bool,
    catalog_initialized: bool,
    last_projected_status: Cell<Option<FocusedBlockerStatus>>,
    last_projected_site: RefCell<Option<zephium_ipc::BlockerSiteView>>,
}

impl BlockerCoordinator {
    /// Constructs a fail-closed coordinator without entering the external
    /// catalog port. The production actor uses this narrow form so it can put
    /// the complete Shell behind its terminal cleanup guard before a faulty
    /// blocker implementation is allowed to unwind.
    pub(super) fn new_deferred(service: SharedBlocker) -> Self {
        Self {
            profiles: HashMap::new(),
            inbox: Arc::new(Mutex::new(BlockerInbox::default())),
            service,
            catalog: BlockerCatalogSnapshot::not_configured(),
            catalog_authority: CatalogAuthority::default(),
            next_generation: Some(1),
            next_store_token: Some(1),
            pending_catalog_refresh: None,
            pending_catalog_activation_poll: None,
            exhausted_catalog_activation_poll_revision: None,
            // No compilation can be authorized until the guarded initial
            // snapshot has passed the same structural validation as before.
            compiler_terminal: true,
            catalog_initialized: false,
            last_projected_status: Cell::new(None),
            last_projected_site: RefCell::new(None),
        }
    }

    /// Enters the external catalog port exactly once during Shell admission.
    /// Callers must already own the Shell through an unexpected-exit guard.
    pub(super) fn initialize_catalog(&mut self) {
        if self.catalog_initialized {
            return;
        }
        let mut catalog = self.service.maintain();
        let catalog_authority = if catalog_snapshot_valid(catalog) {
            CatalogAuthority::from_initial(catalog)
        } else {
            None
        };
        let compiler_terminal = catalog_authority.is_none();
        let catalog_authority = catalog_authority.unwrap_or_default();
        if compiler_terminal {
            catalog.phase = BlockerCatalogPhase::Failed(BlockerCatalogFailure::Internal);
        }
        self.catalog = catalog;
        self.catalog_authority = catalog_authority;
        self.compiler_terminal = compiler_terminal;
        self.catalog_initialized = true;
    }

    fn allocate_generation(&mut self) -> Option<ContentPolicyGeneration> {
        let raw = self.next_generation?;
        if raw == u64::MAX {
            self.next_generation = None;
            return None;
        }
        self.next_generation = raw.checked_add(1);
        ContentPolicyGeneration::new(raw)
    }

    pub(super) fn allocate_store_token(&mut self) -> Option<u64> {
        let token = self.next_store_token?;
        if token == u64::MAX {
            self.next_store_token = None;
            return None;
        }
        self.next_store_token = token.checked_add(1);
        Some(token)
    }

    fn retained_generation(state: BlockerProfileState) -> Option<ContentPolicyGeneration> {
        match state {
            BlockerProfileState::Ready { applied } => Some(applied),
            BlockerProfileState::Compiling { retained, .. }
            | BlockerProfileState::Installing { retained, .. }
            | BlockerProfileState::Failed { retained, .. } => retained,
            BlockerProfileState::Uninitialized | BlockerProfileState::Retired => None,
        }
    }

    fn failed_state(
        desired: ContentPolicyGeneration,
        retained: Option<ContentPolicyGeneration>,
        failure: ContentPolicyFailure,
        retries_remaining: u8,
    ) -> BlockerProfileState {
        BlockerProfileState::Failed {
            desired,
            retained,
            failure,
            retries_remaining: if failure.retryable() {
                retries_remaining
            } else {
                0
            },
        }
    }

    fn terminalize_compiler(&mut self) {
        if self.compiler_terminal {
            return;
        }
        self.compiler_terminal = true;
        let mut inbox = self
            .inbox
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        inbox.results.clear();
        drop(inbox);

        for entry in self.profiles.values_mut() {
            entry.compiling_catalog = None;
            entry.state = match entry.state {
                BlockerProfileState::Compiling {
                    desired, retained, ..
                } => Self::failed_state(
                    desired,
                    retained,
                    ContentPolicyFailure::CompilerUnavailable,
                    0,
                ),
                BlockerProfileState::Failed {
                    desired,
                    retained,
                    failure,
                    ..
                } if failure.retryable() => Self::failed_state(
                    desired,
                    retained,
                    ContentPolicyFailure::CompilerUnavailable,
                    0,
                ),
                state => state,
            };
        }
    }

    pub(super) fn status(&self, profile: ProfileId) -> Option<ProfileContentPolicyStatus> {
        let entry = self.profiles.get(&profile)?;
        Some(ProfileContentPolicyStatus {
            profile,
            config_revision: entry.config.revision,
            desired_config: if entry.startup_provisional {
                entry.config.config
            } else {
                entry.desired_config
            },
            applied_config: entry.applied_config,
            applied_coverage: entry.applied_coverage,
            state: entry.state,
        })
    }

    fn preference(&self, profile: ProfileId) -> Option<BlockerPreferenceAuthority> {
        self.profiles.get(&profile).map(|entry| {
            if entry.pending_mutation.is_some() {
                BlockerPreferenceAuthority::Updating
            } else {
                entry.preference
            }
        })
    }

    pub(super) fn native_policy_available(&self, profile: ProfileId) -> bool {
        self.profiles.get(&profile).is_some_and(|profile| {
            Self::retained_generation(profile.state).is_some() && profile.applied_config.is_some()
        })
    }

    fn should_project(
        &self,
        status: FocusedBlockerStatus,
        site: &Option<zephium_ipc::BlockerSiteView>,
    ) -> bool {
        let mut previous_site = self.last_projected_site.borrow_mut();
        if self.last_projected_status.get() == Some(status) && *previous_site == *site {
            return false;
        }
        self.last_projected_status.set(Some(status));
        previous_site.clone_from(site);
        true
    }

    pub(super) fn initialize_cohort(
        &mut self,
        profiles: &Profiles,
        configs: Vec<ProfileBlockerConfig>,
    ) -> bool {
        if configs.len() != profiles.iter().count()
            || configs.len() > zephium_core::session::MAX_SESSION_PROFILES
        {
            return false;
        }
        let expected: std::collections::HashSet<_> =
            profiles.iter().map(|profile| profile.id).collect();
        let mut observed = std::collections::HashSet::with_capacity(configs.len());
        if configs
            .iter()
            .any(|config| !expected.contains(&config.profile) || !observed.insert(config.profile))
            || observed != expected
        {
            return false;
        }
        if !self.profiles.is_empty() {
            return self.profiles.len() == configs.len()
                && configs.iter().all(|config| {
                    self.profiles
                        .get(&config.profile)
                        .is_some_and(|entry| entry.config == *config)
                });
        }
        for config in configs {
            self.profiles.insert(
                config.profile,
                BlockerProfile {
                    sites: Default::default(),
                    config,
                    state: BlockerProfileState::Uninitialized,
                    held_effects: VecDeque::new(),
                    desired_config: config.config,
                    applied_config: None,
                    pending_coverage: None,
                    applied_coverage: None,
                    pending_runtime_policy: None,
                    applied_runtime_policy: None,
                    preference: BlockerPreferenceAuthority::Authoritative,
                    pending_mutation: None,
                    background_target: None,
                    pending_catalog_revision: None,
                    compiling_catalog: None,
                    applied_catalog_revision: None,
                    startup_provisional: false,
                },
            );
        }
        true
    }

    pub(super) fn initialize_new_profile(&mut self, profile: ProfileId) -> bool {
        if self.profiles.len() >= zephium_core::session::MAX_SESSION_PROFILES
            || self.profiles.contains_key(&profile)
        {
            return false;
        }
        self.profiles.insert(
            profile,
            BlockerProfile {
                sites: super::blocker_sites::SitePreferencesState::fresh(),
                config: ProfileBlockerConfig {
                    profile,
                    revision: BlockerConfigRevision::INITIAL,
                    config: BlockerConfig::default(),
                },
                state: BlockerProfileState::Uninitialized,
                held_effects: VecDeque::new(),
                desired_config: BlockerConfig::default(),
                applied_config: None,
                pending_coverage: None,
                applied_coverage: None,
                pending_runtime_policy: None,
                applied_runtime_policy: None,
                preference: BlockerPreferenceAuthority::Authoritative,
                pending_mutation: None,
                background_target: None,
                pending_catalog_revision: None,
                compiling_catalog: None,
                applied_catalog_revision: None,
                startup_provisional: false,
            },
        );
        true
    }

    pub(super) fn start_uninitialized(&mut self, profile: ProfileId) -> bool {
        let Some(entry) = self.profiles.get(&profile) else {
            return false;
        };
        if entry.state != BlockerProfileState::Uninitialized {
            return true;
        }
        let target = entry.config.config;
        // Browsing must not wait for source parsing or native compilation.
        // Deliver an explicit, allocation-small allow-all artifact locally,
        // then use the ordinary exact native settlement before starting the
        // enabled target. No compiler queue, JSON or native rule list is used.
        let Some(generation) = self.allocate_generation() else {
            return false;
        };
        let entry = self
            .profiles
            .get_mut(&profile)
            .expect("profile was checked above");
        entry.startup_provisional = target.enabled;
        entry.desired_config = BlockerConfig { enabled: false };
        entry.background_target = target.enabled.then_some(target);
        entry.state = BlockerProfileState::Compiling {
            desired: generation,
            retained: None,
            retries_remaining: MAX_EXPLICIT_POLICY_RETRIES,
        };
        self.inbox
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .results
            .insert(
                profile,
                PendingCompileResult {
                    generation,
                    outcome: BlockerCompileOutcome::Compiled(
                        zephium_core::blocker::ContentRules::allow_all(
                            ContentRuleDigest::from_bytes([0; 32]),
                        ),
                    ),
                },
            );
        true
    }

    pub(super) fn start_compile(
        &mut self,
        profile: ProfileId,
        config: BlockerConfig,
        callback: Option<CallbackHandle>,
    ) -> bool {
        self.start_compile_with_retry_budget(profile, config, MAX_EXPLICIT_POLICY_RETRIES, callback)
    }

    fn start_compile_with_retry_budget(
        &mut self,
        profile: ProfileId,
        config: BlockerConfig,
        retries_remaining: u8,
        callback: Option<CallbackHandle>,
    ) -> bool {
        let Some(previous) = self.profiles.get(&profile).map(|entry| entry.state) else {
            return false;
        };
        // One profile may own only one unsettled generation. Starting another
        // attempt here would make the first attempt's exact native settlement
        // indistinguishable from a stale callback and could lose the
        // generation the engine actually applied. A future live-settings path
        // can coalesce a bounded latest desired config outside this state
        // machine; it must not displace the in-flight authority.
        if matches!(
            previous,
            BlockerProfileState::Compiling { .. }
                | BlockerProfileState::Installing { .. }
                | BlockerProfileState::Retired
        ) {
            return false;
        }
        if let Some(entry) = self.profiles.get_mut(&profile) {
            entry.startup_provisional = false;
        }
        let Some(generation) = self.allocate_generation() else {
            let Some(exhausted) = ContentPolicyGeneration::new(u64::MAX) else {
                return false;
            };
            if let Some(entry) = self.profiles.get_mut(&profile) {
                entry.desired_config = config;
                entry.pending_coverage = None;
                entry.pending_runtime_policy = None;
                entry.compiling_catalog = None;
                entry.state = Self::failed_state(
                    exhausted,
                    Self::retained_generation(previous),
                    ContentPolicyFailure::GenerationExhausted,
                    0,
                );
            }
            return false;
        };
        let retained = Self::retained_generation(previous);
        if self.compiler_terminal {
            if let Some(entry) = self.profiles.get_mut(&profile) {
                entry.desired_config = config;
                entry.pending_coverage = None;
                entry.pending_runtime_policy = None;
                entry.compiling_catalog = None;
                entry.state = Self::failed_state(
                    generation,
                    retained,
                    ContentPolicyFailure::CompilerUnavailable,
                    0,
                );
            }
            return false;
        }
        if config.enabled && self.catalog.enabled_policy_terminal {
            if let Some(entry) = self.profiles.get_mut(&profile) {
                entry.desired_config = config;
                entry.pending_coverage = None;
                entry.pending_runtime_policy = None;
                entry.compiling_catalog = None;
                entry.state = Self::failed_state(
                    generation,
                    retained,
                    ContentPolicyFailure::CompilerUnavailable,
                    0,
                );
            }
            return false;
        }
        if let Some(entry) = self.profiles.get_mut(&profile) {
            entry.desired_config = config;
            entry.pending_coverage = None;
            entry.pending_runtime_policy = None;
            entry.compiling_catalog = config.enabled.then_some(()).and_then(|()| {
                catalog_installed_identity(self.catalog)
                    .ok()
                    .flatten()
                    .map(|installed| CatalogCompileAttempt {
                        generation,
                        installed,
                        source_material_epoch: self.catalog.source_material_epoch,
                    })
            });
            entry.state = BlockerProfileState::Compiling {
                desired: generation,
                retained,
                retries_remaining,
            };
        }

        let inbox = self.inbox.clone();
        let wake_handoff = Arc::new(CompileWakeHandoff::default());
        let callback_handoff = Arc::clone(&wake_handoff);
        let callback_wake = callback.clone();
        let dispatch = self.service.compile(
            profile,
            generation,
            config,
            Box::new(move |outcome| {
                let mut inbox = inbox
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                if inbox.results.len() < zephium_core::session::MAX_SESSION_PROFILES
                    || inbox.results.contains_key(&profile)
                {
                    match inbox.results.entry(profile) {
                        std::collections::hash_map::Entry::Occupied(mut entry)
                            if entry.get().generation < generation =>
                        {
                            entry.insert(PendingCompileResult {
                                generation,
                                outcome,
                            });
                        }
                        std::collections::hash_map::Entry::Vacant(entry) => {
                            entry.insert(PendingCompileResult {
                                generation,
                                outcome,
                            });
                        }
                        std::collections::hash_map::Entry::Occupied(_) => {}
                    }
                }
                drop(inbox);
                if callback_handoff.callback_published() {
                    if let Some(callback) = callback_wake {
                        let _ = callback.dispatch(Command::BlockerReady(profile));
                    }
                }
            }),
        );
        if wake_handoff.call_returned() {
            if let Some(callback) = callback {
                let _ = callback.dispatch(Command::BlockerReady(profile));
            }
        }
        match dispatch {
            BlockerDispatch::Scheduled => true,
            BlockerDispatch::Rejected => {
                if let Some(entry) = self.profiles.get_mut(&profile) {
                    entry.compiling_catalog = None;
                    entry.state = Self::failed_state(
                        generation,
                        retained,
                        ContentPolicyFailure::CompilerDispatchRejected,
                        retries_remaining,
                    );
                }
                false
            }
            BlockerDispatch::EnabledPolicyTerminal => {
                // The service's monotonic enabled-source seal may race the
                // preceding catalog observation. It is not proof that the
                // independent disabled allow-all path is terminal.
                if let Some(entry) = self.profiles.get_mut(&profile) {
                    entry.compiling_catalog = None;
                    entry.state = Self::failed_state(
                        generation,
                        retained,
                        ContentPolicyFailure::CompilerUnavailable,
                        0,
                    );
                }
                false
            }
            BlockerDispatch::Terminal => {
                self.terminalize_compiler();
                false
            }
        }
    }

    pub(super) fn retry_failed(
        &mut self,
        profile: ProfileId,
        failed_generation: ContentPolicyGeneration,
        callback: Option<CallbackHandle>,
    ) -> BlockerRetry {
        let Some(entry) = self.profiles.get(&profile) else {
            return BlockerRetry::UnknownProfile;
        };
        if entry.state == BlockerProfileState::Retired {
            return BlockerRetry::UnknownProfile;
        }
        let BlockerProfileState::Failed {
            desired,
            failure,
            retries_remaining,
            ..
        } = entry.state
        else {
            return BlockerRetry::NotEligible;
        };
        if desired != failed_generation || !failure.retryable() || retries_remaining == 0 {
            return BlockerRetry::NotEligible;
        }
        if failure == ContentPolicyFailure::Compile(BlockerCompileFailure::SourceUnavailable)
            && !source_material_retry_ready(entry, self.catalog)
        {
            return BlockerRetry::NotEligible;
        }
        let config = entry.desired_config;
        if self.start_compile_with_retry_budget(profile, config, retries_remaining - 1, callback) {
            BlockerRetry::Scheduled
        } else {
            BlockerRetry::AdmissionRejected
        }
    }

    pub(super) fn take_compile_result(&self, profile: ProfileId) -> Option<PendingCompileResult> {
        self.inbox
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .results
            .remove(&profile)
    }

    pub(super) fn pending_result_profiles(&self) -> Vec<ProfileId> {
        self.inbox
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .results
            .keys()
            .copied()
            .collect()
    }

    fn take_store_result(&self, profile: ProfileId) -> Option<PendingStoreResult> {
        self.inbox
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .store_results
            .remove(&profile)
    }

    fn pending_store_result_profiles(&self) -> Vec<ProfileId> {
        self.inbox
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .store_results
            .keys()
            .copied()
            .collect()
    }

    #[cfg(test)]
    pub(super) fn compiler_inbox_state(&self) -> (bool, usize) {
        let inbox = self
            .inbox
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        (self.compiler_terminal, inbox.results.len())
    }

    pub(super) fn hold_effect(&mut self, profile: ProfileId, effect: Effect) -> bool {
        let Some(entry) = self.profiles.get_mut(&profile) else {
            return false;
        };
        if entry.state == BlockerProfileState::Retired {
            return false;
        }
        match effect {
            Effect::Close { id } => {
                entry.held_effects.retain(|pending| match pending {
                    Effect::CreateView { id: pending, .. }
                    | Effect::Navigate { id: pending, .. }
                    | Effect::Close { id: pending } => *pending != id,
                });
                true
            }
            Effect::Navigate { id, url, request } => {
                if let Some(Effect::CreateView {
                    url: initial_url, ..
                }) = entry
                    .held_effects
                    .iter_mut()
                    .find(|pending| matches!(pending, Effect::CreateView { id: pending, .. } if *pending == id))
                {
                    // No native controller exists yet. Folding the newest
                    // target into its eventual initial load avoids spawning a
                    // renderer for an already-obsolete URL.
                    *initial_url = url;
                    return true;
                }
                entry.held_effects.retain(
                    |pending| !matches!(pending, Effect::Navigate { id: pending, .. } if *pending == id),
                );
                if entry.held_effects.len() >= MAX_HELD_EFFECTS_PER_PROFILE {
                    return false;
                }
                entry
                    .held_effects
                    .push_back(Effect::Navigate { id, url, request });
                true
            }
            Effect::CreateView { id, .. } => {
                if entry
                    .held_effects
                    .iter()
                    .any(|pending| matches!(pending, Effect::CreateView { id: pending, .. } if *pending == id))
                {
                    return true;
                }
                if entry.held_effects.len() >= MAX_HELD_EFFECTS_PER_PROFILE {
                    return false;
                }
                entry.held_effects.push_back(effect);
                true
            }
        }
    }

    fn remove_held_item_effects(&mut self, id: ItemId) -> bool {
        let mut canceled_unmaterialized_create = false;
        for entry in self.profiles.values_mut() {
            entry.held_effects.retain(|pending| {
                let matches_item = match pending {
                    Effect::CreateView { id: pending, .. }
                    | Effect::Navigate { id: pending, .. }
                    | Effect::Close { id: pending } => *pending == id,
                };
                if matches_item && matches!(pending, Effect::CreateView { .. }) {
                    canceled_unmaterialized_create = true;
                }
                !matches_item
            });
        }
        canceled_unmaterialized_create
    }

    pub(super) fn take_held_effects(&mut self, profile: ProfileId) -> Vec<Effect> {
        self.profiles
            .get_mut(&profile)
            .map(|entry| entry.held_effects.drain(..).collect())
            .unwrap_or_default()
    }

    pub(super) fn retire_profile(&mut self, profile: ProfileId) {
        if let Some(entry) = self.profiles.get_mut(&profile) {
            entry.sites = Default::default();
            entry.held_effects.clear();
            entry.applied_config = None;
            entry.pending_coverage = None;
            entry.applied_coverage = None;
            entry.pending_runtime_policy = None;
            entry.applied_runtime_policy = None;
            entry.preference = BlockerPreferenceAuthority::Unavailable;
            entry.pending_mutation = None;
            entry.background_target = None;
            entry.pending_catalog_revision = None;
            entry.compiling_catalog = None;
            entry.applied_catalog_revision = None;
            entry.state = BlockerProfileState::Retired;
        }
        {
            let mut inbox = self
                .inbox
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            inbox.results.remove(&profile);
            inbox.store_results.remove(&profile);
            inbox.site_results.remove(&profile);
        }
        let inbox = self.inbox.clone();
        let retirement = self.service.retire_profile(
            profile,
            Box::new(move || {
                // This ordered cleanup closes the lost-wake case: a callback
                // already delivering at retirement may publish after the
                // eager removal above, but cannot publish after this barrier.
                inbox
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .results
                    .remove(&profile);
                inbox
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .store_results
                    .remove(&profile);
            }),
        );
        // A completion callback which crossed its delivery point before the
        // barrier may still arrive. The retired state and globally unique
        // generation remain the authoritative filters; no process-lifetime
        // profile tombstone is necessary.
        if retirement == BlockerRetirementDispatch::Terminal {
            self.terminalize_compiler();
        }
    }

    pub(super) fn discard_uncommitted_profile(&mut self, profile: ProfileId) {
        self.retire_profile(profile);
        self.profiles.remove(&profile);
    }

    pub(super) fn shutdown_until(&self, deadline: std::time::Instant) -> BlockerShutdownOutcome {
        self.service.shutdown_until(deadline)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum BlockerRetry {
    Scheduled,
    UnknownProfile,
    NotEligible,
    AdmissionRejected,
}

impl Shell {
    #[cfg(test)]
    pub(super) fn blocker_catalog_for_test(&self) -> BlockerCatalogSnapshot {
        self.blocker.catalog
    }

    #[cfg(test)]
    pub(super) fn pending_catalog_activation_poll_for_test(&self) -> Option<(u64, u8)> {
        self.blocker
            .pending_catalog_activation_poll
            .map(|pending| (pending.catalog_revision, pending.next_attempt))
    }

    #[cfg(test)]
    pub(super) fn pending_catalog_refresh_for_test(&self) -> Option<(u64, u8)> {
        self.blocker
            .pending_catalog_refresh
            .as_ref()
            .map(|pending| (pending.service_operation, pending.next_poll_attempt))
    }

    fn focused_blocker_status(&self) -> FocusedBlockerStatus {
        let catalog = self.blocker.catalog;
        let Some(profile) = self.windows.focused().map(|window| window.profile) else {
            return FocusedBlockerStatus::Unavailable { catalog };
        };
        let Some(status) = self.blocker.status(profile) else {
            return FocusedBlockerStatus::Unavailable { catalog };
        };
        let Some(preference) = self.blocker.preference(profile) else {
            return FocusedBlockerStatus::Unavailable { catalog };
        };
        let Some(entry) = self.blocker.profiles.get(&profile) else {
            return FocusedBlockerStatus::Unavailable { catalog };
        };
        FocusedBlockerStatus::Found {
            status,
            preference,
            applied_catalog_revision: entry.applied_catalog_revision,
            source_material_retry_ready: source_material_retry_ready(entry, catalog),
            catalog,
        }
    }

    fn focused_runtime_policy_diagnostics(&self) -> Option<NetworkPolicyDiagnostics> {
        let profile = self.windows.focused()?.profile;
        self.blocker
            .profiles
            .get(&profile)?
            .applied_runtime_policy
            .as_ref()?
            .policy
            .upgrade()
            .map(|policy| policy.diagnostics())
            .filter(|diagnostics| diagnostics.is_consistent())
    }

    pub(super) fn focused_blocker_status_view(&self) -> BlockerStatusView {
        let mut view = blocker_status_view(
            self.focused_blocker_status(),
            self.focused_runtime_policy_diagnostics(),
            self.next_projection_revision(),
        );
        view.site = self.focused_blocker_site_view().map(Box::new);
        view
    }

    #[cfg(feature = "work-execution")]
    pub(super) fn work_profile_binding(&self) -> crate::AgentWorkProfileReadiness {
        use crate::{AgentWorkProfileBinding as Binding, AgentWorkProfileReadiness as Readiness};
        let Some(profile) = self
            .windows
            .focused()
            .and_then(|window| self.profiles.get(window.profile))
        else {
            return Readiness::ProfileMissing;
        };
        if self.profile_deletion_quarantines(profile.id) {
            return Readiness::ProfileMissing;
        }
        let binding = Binding::from_profile(profile);
        let Some(status) = self.blocker.status(profile.id) else {
            return Readiness::PolicyMissing;
        };
        match status.state {
            ProfileContentPolicyState::Ready { .. }
                if self.blocker.native_policy_available(profile.id) =>
            {
                Readiness::Ready(binding)
            }
            ProfileContentPolicyState::Uninitialized
            | ProfileContentPolicyState::Compiling { .. }
            | ProfileContentPolicyState::Installing { .. } => Readiness::PolicyPending(binding),
            _ => Readiness::PolicyFailed,
        }
    }

    pub(super) fn project_blocker_status(&self) {
        let status = self.focused_blocker_status();
        let site = self.focused_blocker_site_view();
        if self.blocker.should_project(status, &site) {
            let mut view = blocker_status_view(
                status,
                self.focused_runtime_policy_diagnostics(),
                self.next_projection_revision(),
            );
            view.site = site.map(Box::new);
            (self.emit)(Projection::BlockerStatus(view));
        }
    }

    pub(super) fn maintain_blocker_catalog(&mut self) {
        let observed = self.blocker.service.maintain();
        let current = self.blocker.catalog;
        if observed.revision < current.revision {
            self.fail_blocker_catalog(BlockerCatalogFailure::Internal);
            return;
        }
        if observed.revision == current.revision {
            if observed != current {
                self.fail_blocker_catalog(BlockerCatalogFailure::Internal);
            } else {
                self.reconcile_pending_blocker_catalog_refresh();
                self.schedule_pending_blocker_catalog_poll();
                self.schedule_blocker_catalog_activation_poll();
            }
            return;
        }
        if !catalog_snapshot_valid(observed) {
            self.fail_blocker_catalog(BlockerCatalogFailure::Internal);
            return;
        }
        let Some(next_authority) = self.blocker.catalog_authority.advance(current, observed) else {
            self.fail_blocker_catalog(BlockerCatalogFailure::Rollback);
            return;
        };

        let newly_installed = observed
            .installed_revision
            .filter(|revision| Some(*revision) != current.installed_revision);
        let source_material_advanced =
            observed.source_material_epoch > current.source_material_epoch;
        self.blocker.catalog_authority = next_authority;
        self.blocker.catalog = observed;
        self.blocker.exhausted_catalog_activation_poll_revision = None;
        let mut drive = Vec::new();
        if let Some(revision) = newly_installed {
            for (profile, entry) in &mut self.blocker.profiles {
                if !entry.desired_config.enabled
                    || matches!(
                        entry.state,
                        BlockerProfileState::Uninitialized | BlockerProfileState::Retired
                    )
                    || entry
                        .applied_catalog_revision
                        .is_some_and(|applied| applied >= revision)
                {
                    continue;
                }
                entry.pending_catalog_revision = Some(
                    entry
                        .pending_catalog_revision
                        .map_or(revision, |pending| pending.max(revision)),
                );
                entry.background_target = Some(entry.desired_config);
                drive.push(*profile);
            }
        }
        if source_material_advanced {
            for (profile, entry) in &mut self.blocker.profiles {
                let eligible = source_material_retry_ready(entry, observed);
                if eligible && !drive.contains(profile) {
                    entry.background_target = Some(entry.desired_config);
                    drive.push(*profile);
                }
            }
        }
        for profile in drive {
            self.drive_blocker_native_target(profile);
        }
        self.reconcile_pending_blocker_catalog_refresh();
        self.schedule_pending_blocker_catalog_poll();
        self.schedule_blocker_catalog_activation_poll();
        self.project_blocker_status();
    }

    pub(super) fn begin_blocker_catalog_refresh(
        &mut self,
        operation_id: String,
    ) -> Option<OperationDisposition> {
        self.maintain_blocker_catalog();
        if self.blocker.compiler_terminal || self.blocker.catalog.enabled_policy_terminal {
            return Some(operation_result(
                OperationOutcome::Rejected,
                OperationReason::ContentPolicySourceUnavailable,
            ));
        }
        if self.blocker.pending_catalog_refresh.is_some() {
            return Some(operation_result(
                OperationOutcome::Rejected,
                OperationReason::ContentPolicySourceRefreshPending,
            ));
        }
        let after_catalog_revision = self.blocker.catalog.revision;
        let Some(deadline) =
            std::time::Instant::now().checked_add(CATALOG_REFRESH_OPERATION_TIMEOUT)
        else {
            return Some(operation_result(
                OperationOutcome::Rejected,
                OperationReason::ContentPolicySourceUnavailable,
            ));
        };
        match self.blocker.service.request_refresh() {
            BlockerCatalogRefreshDispatch::Accepted { operation } => {
                self.blocker.pending_catalog_refresh = Some(PendingCatalogRefresh {
                    operation_id,
                    service_operation: operation,
                    after_catalog_revision,
                    next_poll_attempt: 0,
                    deadline,
                });
                self.maintain_blocker_catalog();
                self.schedule_pending_blocker_catalog_poll();
                None
            }
            BlockerCatalogRefreshDispatch::Busy => Some(operation_result(
                OperationOutcome::Rejected,
                OperationReason::ContentPolicySourceRefreshPending,
            )),
            BlockerCatalogRefreshDispatch::RetryLimitReached => Some(operation_result(
                OperationOutcome::Rejected,
                OperationReason::ContentPolicySourceRefreshFailed,
            )),
            BlockerCatalogRefreshDispatch::Unsupported
            | BlockerCatalogRefreshDispatch::Unavailable(_)
            | BlockerCatalogRefreshDispatch::Terminal => Some(operation_result(
                OperationOutcome::Rejected,
                OperationReason::ContentPolicySourceUnavailable,
            )),
        }
    }

    pub(super) fn on_blocker_catalog_poll(&mut self, operation: u64, attempt: u8) {
        if operation == INTERNAL_CATALOG_POLL_OPERATION {
            if self
                .blocker
                .pending_catalog_activation_poll
                .is_some_and(|pending| {
                    pending.catalog_revision == self.blocker.catalog.revision
                        && attempt < pending.next_attempt
                })
            {
                self.maintain_blocker_catalog();
            }
            return;
        }
        if self
            .blocker
            .pending_catalog_refresh
            .as_ref()
            .is_some_and(|pending| pending.service_operation == operation)
        {
            self.maintain_blocker_catalog();
        }
    }

    pub(super) fn schedule_blocker_catalog_activation_poll(&mut self) {
        let Some(queue) = &self.self_queue else {
            return;
        };
        if self.blocker.pending_catalog_refresh.is_some() {
            return;
        }
        let catalog = self.blocker.catalog;
        if (!catalog.activation_pending && !catalog.source_material_repair_pending)
            || self.blocker.compiler_terminal
            || catalog.enabled_policy_terminal
            || catalog.repair_retry_pending
        {
            queue.cancel_blocker_catalog_poll(INTERNAL_CATALOG_POLL_OPERATION);
            self.blocker.pending_catalog_activation_poll = None;
            self.blocker.exhausted_catalog_activation_poll_revision = None;
            return;
        }
        if self.blocker.exhausted_catalog_activation_poll_revision == Some(catalog.revision) {
            return;
        }

        let now = std::time::Instant::now();
        let pending = self
            .blocker
            .pending_catalog_activation_poll
            .get_or_insert_with(|| PendingCatalogActivationPoll {
                catalog_revision: catalog.revision,
                next_attempt: 0,
                deadline: now
                    .checked_add(CATALOG_REFRESH_OPERATION_TIMEOUT)
                    .unwrap_or(now),
            });
        if pending.catalog_revision != catalog.revision {
            *pending = PendingCatalogActivationPoll {
                catalog_revision: catalog.revision,
                next_attempt: 0,
                deadline: now
                    .checked_add(CATALOG_REFRESH_OPERATION_TIMEOUT)
                    .unwrap_or(now),
            };
        }
        if now >= pending.deadline {
            self.blocker.exhausted_catalog_activation_poll_revision = Some(catalog.revision);
            self.blocker.pending_catalog_activation_poll = None;
            queue.cancel_blocker_catalog_poll(INTERNAL_CATALOG_POLL_OPERATION);
            return;
        }

        let attempt = pending.next_attempt;
        pending.next_attempt = attempt.saturating_add(1);
        let multiplier = 1_u32 << u32::from(attempt.min(3));
        let delay = CATALOG_POLL_INITIAL
            .saturating_mul(multiplier)
            .min(CATALOG_POLL_MAX);
        let deadline = now
            .checked_add(delay)
            .unwrap_or(pending.deadline)
            .min(pending.deadline);
        queue.schedule_blocker_catalog_poll(INTERNAL_CATALOG_POLL_OPERATION, attempt, deadline);
    }

    fn schedule_pending_blocker_catalog_poll(&mut self) {
        let Some(queue) = &self.self_queue else {
            return;
        };
        let Some(pending) = self.blocker.pending_catalog_refresh.as_mut() else {
            return;
        };
        let attempt = pending.next_poll_attempt;
        pending.next_poll_attempt = attempt.saturating_add(1);
        let multiplier = 1_u32 << u32::from(attempt.min(3));
        let delay = CATALOG_POLL_INITIAL
            .saturating_mul(multiplier)
            .min(CATALOG_POLL_MAX);
        let deadline = std::time::Instant::now()
            .checked_add(delay)
            .unwrap_or(pending.deadline)
            .min(pending.deadline);
        queue.schedule_blocker_catalog_poll(pending.service_operation, attempt, deadline);
    }

    fn reconcile_pending_blocker_catalog_refresh(&mut self) {
        let Some(pending) = self.blocker.pending_catalog_refresh.as_ref() else {
            return;
        };
        let service_operation = pending.service_operation;
        let after_catalog_revision = pending.after_catalog_revision;
        let deadline = pending.deadline;
        let catalog = self.blocker.catalog;

        if std::time::Instant::now() >= deadline {
            self.finish_blocker_catalog_refresh(
                OperationOutcome::Deferred,
                OperationReason::ContentPolicySourceRefreshPending,
            );
            return;
        }
        if catalog.refresh_operation == Some(service_operation) {
            match catalog.phase {
                BlockerCatalogPhase::Refreshing => return,
                BlockerCatalogPhase::Failed(_) => {
                    self.finish_blocker_catalog_refresh(
                        OperationOutcome::Rejected,
                        OperationReason::ContentPolicySourceRefreshFailed,
                    );
                    return;
                }
                _ => {
                    self.finish_blocker_catalog_refresh(
                        OperationOutcome::Rejected,
                        OperationReason::ContentPolicySourceRefreshFailed,
                    );
                    return;
                }
            }
        }
        if catalog.refresh_operation.is_some() {
            self.finish_blocker_catalog_refresh(
                OperationOutcome::Rejected,
                OperationReason::ContentPolicySourceRefreshFailed,
            );
            return;
        }
        if catalog.revision <= after_catalog_revision {
            return;
        }
        match catalog.phase {
            BlockerCatalogPhase::Failed(_) => {
                self.finish_blocker_catalog_refresh(
                    OperationOutcome::Rejected,
                    OperationReason::ContentPolicySourceRefreshFailed,
                );
                return;
            }
            BlockerCatalogPhase::Unavailable(_) | BlockerCatalogPhase::Shutdown => {
                self.finish_blocker_catalog_refresh(
                    OperationOutcome::Rejected,
                    OperationReason::ContentPolicySourceUnavailable,
                );
                return;
            }
            BlockerCatalogPhase::Idle
            | BlockerCatalogPhase::Fresh
            | BlockerCatalogPhase::Stale
            | BlockerCatalogPhase::Refreshing => {}
        }
        if catalog.activation_pending {
            return;
        }
        match catalog.phase {
            BlockerCatalogPhase::Fresh | BlockerCatalogPhase::Stale
                if catalog.package_revision.is_some()
                    && catalog.package_revision == catalog.installed_revision =>
            {
                self.finish_blocker_catalog_refresh(
                    OperationOutcome::Applied,
                    OperationReason::ContentPolicySourcesRefreshed,
                );
            }
            BlockerCatalogPhase::Idle
            | BlockerCatalogPhase::Fresh
            | BlockerCatalogPhase::Stale
            | BlockerCatalogPhase::Refreshing
            | BlockerCatalogPhase::Failed(_)
            | BlockerCatalogPhase::Unavailable(_)
            | BlockerCatalogPhase::Shutdown => self.finish_blocker_catalog_refresh(
                OperationOutcome::Rejected,
                OperationReason::ContentPolicySourceRefreshFailed,
            ),
        }
    }

    fn finish_blocker_catalog_refresh(
        &mut self,
        outcome: OperationOutcome,
        reason: OperationReason,
    ) {
        let Some(pending) = self.blocker.pending_catalog_refresh.take() else {
            return;
        };
        if let Some(queue) = &self.self_queue {
            queue.cancel_blocker_catalog_poll(pending.service_operation);
        }
        (self.emit)(Projection::OperationProcessed(OperationDisposition {
            operation_id: pending.operation_id,
            outcome,
            reason,
        }));
    }

    fn fail_blocker_catalog(&mut self, failure: BlockerCatalogFailure) {
        self.blocker.catalog.phase = BlockerCatalogPhase::Failed(failure);
        if let Some(queue) = &self.self_queue {
            queue.cancel_blocker_catalog_poll(INTERNAL_CATALOG_POLL_OPERATION);
        }
        self.blocker.pending_catalog_activation_poll = None;
        self.blocker.exhausted_catalog_activation_poll_revision = None;
        self.blocker.terminalize_compiler();
        self.finish_terminalized_blocker_native_operations();
        self.finish_blocker_catalog_refresh(
            OperationOutcome::Rejected,
            OperationReason::ContentPolicySourceRefreshFailed,
        );
        self.project_blocker_status();
    }

    pub(super) fn initialize_blocker_cohort(&mut self, configs: Vec<ProfileBlockerConfig>) -> bool {
        self.blocker.initialize_cohort(&self.profiles, configs)
    }

    pub(super) fn initialize_new_blocker_profile(&mut self, profile: ProfileId) -> bool {
        if !self.blocker.initialize_new_profile(profile) {
            return false;
        }
        self.ensure_blocker_statistics(profile, true);
        true
    }

    pub(super) fn start_blocker_profile(&mut self, profile: ProfileId) -> bool {
        self.ensure_blocker_statistics(profile, false);
        self.ensure_blocker_site_preferences(profile);
        self.ensure_popup_sites(profile);
        let accepted = self.blocker.start_uninitialized(profile);
        self.finish_terminalized_blocker_native_operations();
        self.project_blocker_status();
        // Deterministic compilers may complete inside `compile`. Consume that
        // result now; the queued wake is generation-checked and becomes inert.
        self.consume_blocker_compile_result(profile);
        accepted
    }

    pub(super) fn operation_retry_content_policy(
        &mut self,
        profile: ProfileId,
        failed_generation: ContentPolicyGeneration,
    ) -> OperationDisposition {
        let callback = self.self_queue.as_ref().map(|queue| CallbackHandle {
            queue: Arc::downgrade(&queue.inner),
        });
        let disposition = match self
            .blocker
            .retry_failed(profile, failed_generation, callback)
        {
            BlockerRetry::Scheduled => {
                if let Some(entry) = self.blocker.profiles.get_mut(&profile) {
                    entry.sites.failed = false;
                }
                self.ensure_blocker_site_preferences(profile);
                self.project_blocker_status();
                // Preserve the same synchronous-completion contract as
                // startup: a deterministic compiler cannot strand its result
                // merely because no callback wake was necessary.
                self.consume_blocker_compile_result(profile);
                operation_result(
                    OperationOutcome::Deferred,
                    OperationReason::NativeWorkPending,
                )
            }
            BlockerRetry::UnknownProfile => {
                operation_result(OperationOutcome::Rejected, OperationReason::InvalidScope)
            }
            BlockerRetry::NotEligible => {
                operation_result(OperationOutcome::Rejected, OperationReason::StateUnchanged)
            }
            BlockerRetry::AdmissionRejected => operation_result(
                OperationOutcome::NativeAdmissionFailed,
                OperationReason::NativeDispatchRejected,
            ),
        };
        self.finish_terminalized_blocker_native_operations();
        disposition
    }

    /// Returns an immediate terminal disposition, or `None` after transferring
    /// the operation id to the exact durable/native completion state machine.
    pub(super) fn begin_focused_blocker_mutation(
        &mut self,
        operation_id: String,
        enabled: bool,
    ) -> Option<OperationDisposition> {
        // Observe the latest source authority before admitting a durable
        // preference mutation. An enabled-policy terminal transition must
        // reject enablement before the store CAS, while disablement remains
        // available through the independent allow-all path.
        self.maintain_blocker_catalog();
        let Some(profile) = self.windows.focused().map(|window| window.profile) else {
            return Some(operation_result(
                OperationOutcome::Rejected,
                OperationReason::NoFocusedWindow,
            ));
        };
        let Some(entry) = self.blocker.profiles.get(&profile) else {
            return Some(operation_result(
                OperationOutcome::Rejected,
                OperationReason::InvalidScope,
            ));
        };
        if entry.state == BlockerProfileState::Retired {
            return Some(operation_result(
                OperationOutcome::Rejected,
                OperationReason::InvalidScope,
            ));
        }
        if enabled
            && (self.blocker.compiler_terminal
                || !catalog_has_compiler_policy(self.blocker.catalog))
        {
            return Some(operation_result(
                OperationOutcome::Rejected,
                OperationReason::ContentPolicySourceUnavailable,
            ));
        }
        if entry.pending_mutation.is_some()
            || entry.preference != BlockerPreferenceAuthority::Authoritative
        {
            return Some(operation_result(
                OperationOutcome::Rejected,
                OperationReason::StoreWorkPending,
            ));
        }
        let requested = BlockerConfig { enabled };
        if entry.config.config == requested {
            return Some(operation_result(
                OperationOutcome::NoOp,
                OperationReason::StateUnchanged,
            ));
        }
        let expected_revision = entry.config.revision;
        let Some(token) = self.blocker.allocate_store_token() else {
            return Some(operation_result(
                OperationOutcome::Rejected,
                OperationReason::StoreAdmissionRejected,
            ));
        };
        if let Some(entry) = self.blocker.profiles.get_mut(&profile) {
            entry.preference = BlockerPreferenceAuthority::Updating;
            entry.pending_mutation = Some(PendingBlockerMutation {
                operation_id,
                expected_revision,
                requested,
                phase: PendingMutationPhase::Store { token },
                success_outcome: OperationOutcome::Applied,
            });
        }
        self.project_blocker_status();

        let inbox = self.blocker.inbox.clone();
        let callback = self.self_queue.as_ref().map(|queue| CallbackHandle {
            queue: Arc::downgrade(&queue.inner),
        });
        let store_returned = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let callback_after_return = store_returned.clone();
        let accepted = self.store.update_profile_blocker_config(
            profile,
            expected_revision,
            requested,
            Box::new(move |outcome| {
                publish_store_result(
                    &inbox,
                    profile,
                    PendingStoreResult {
                        token,
                        outcome: PendingStoreOutcome::Mutation(outcome),
                    },
                );
                if callback_after_return.load(std::sync::atomic::Ordering::Acquire) {
                    if let Some(callback) = callback {
                        let _ = callback.dispatch(Command::BlockerStoreReady(profile));
                    }
                }
            }),
        );
        store_returned.store(true, std::sync::atomic::Ordering::Release);
        if !accepted {
            let disposition = self
                .take_pending_blocker_operation(profile)
                .map(|operation_id| OperationDisposition {
                    operation_id,
                    outcome: OperationOutcome::Rejected,
                    reason: OperationReason::StoreAdmissionRejected,
                })
                .unwrap_or_else(|| {
                    operation_result(
                        OperationOutcome::Rejected,
                        OperationReason::StoreAdmissionRejected,
                    )
                });
            if let Some(entry) = self.blocker.profiles.get_mut(&profile) {
                entry.preference = BlockerPreferenceAuthority::Authoritative;
            }
            self.project_blocker_status();
            return Some(disposition);
        }

        // Test adapters and simple embedders may complete synchronously.
        self.consume_blocker_store_result(profile);
        None
    }

    fn take_pending_blocker_operation(&mut self, profile: ProfileId) -> Option<String> {
        self.blocker
            .profiles
            .get_mut(&profile)
            .and_then(|entry| entry.pending_mutation.take())
            .map(|pending| pending.operation_id)
    }

    fn finish_blocker_operation(
        &mut self,
        profile: ProfileId,
        outcome: OperationOutcome,
        reason: OperationReason,
    ) {
        let operation_id = self.take_pending_blocker_operation(profile);
        if let Some(operation_id) = operation_id {
            (self.emit)(Projection::OperationProcessed(OperationDisposition {
                operation_id,
                outcome,
                reason,
            }));
        }
    }

    fn begin_blocker_preference_reconciliation(&mut self, profile: ProfileId) {
        let Some(token) = self.blocker.allocate_store_token() else {
            if let Some(entry) = self.blocker.profiles.get_mut(&profile) {
                entry.preference = BlockerPreferenceAuthority::Unavailable;
            }
            self.project_blocker_status();
            return;
        };
        if let Some(entry) = self.blocker.profiles.get_mut(&profile) {
            entry.preference = BlockerPreferenceAuthority::Reconciling {
                token,
                attempts_remaining: MAX_PREFERENCE_RECONCILE_ATTEMPTS,
                retry_at: None,
            };
        }
        self.dispatch_blocker_preference_reconciliation(profile, token);
    }

    fn dispatch_blocker_preference_reconciliation(&mut self, profile: ProfileId, token: u64) {
        let inbox = self.blocker.inbox.clone();
        let callback = self.self_queue.as_ref().map(|queue| CallbackHandle {
            queue: Arc::downgrade(&queue.inner),
        });
        let store_returned = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let callback_after_return = store_returned.clone();
        let accepted = self.store.load_profile_blocker_config(
            profile,
            Box::new(move |outcome| {
                publish_store_result(
                    &inbox,
                    profile,
                    PendingStoreResult {
                        token,
                        outcome: PendingStoreOutcome::Reconciliation(outcome),
                    },
                );
                if callback_after_return.load(std::sync::atomic::Ordering::Acquire) {
                    if let Some(callback) = callback {
                        let _ = callback.dispatch(Command::BlockerStoreReady(profile));
                    }
                }
            }),
        );
        store_returned.store(true, std::sync::atomic::Ordering::Release);
        if accepted {
            self.consume_blocker_store_result(profile);
        } else {
            self.defer_or_fail_blocker_reconciliation(profile, token);
        }
    }

    fn defer_or_fail_blocker_reconciliation(&mut self, profile: ProfileId, token: u64) {
        let Some(entry) = self.blocker.profiles.get_mut(&profile) else {
            return;
        };
        let BlockerPreferenceAuthority::Reconciling {
            token: current,
            attempts_remaining,
            ..
        } = entry.preference
        else {
            return;
        };
        if current != token {
            return;
        }
        let remaining = attempts_remaining.saturating_sub(1);
        let retry_deadline = if remaining == 0 {
            entry.preference = BlockerPreferenceAuthority::Unavailable;
            None
        } else {
            let attempted = MAX_PREFERENCE_RECONCILE_ATTEMPTS.saturating_sub(remaining);
            let multiplier = 1_u32 << u32::from(attempted.saturating_sub(1).min(4));
            let retry_at = std::time::Instant::now()
                + PREFERENCE_RECONCILE_RETRY_BASE.saturating_mul(multiplier);
            entry.preference = BlockerPreferenceAuthority::Reconciling {
                token,
                attempts_remaining: remaining,
                retry_at: Some(retry_at),
            };
            Some(retry_at)
        };
        if let Some(queue) = &self.self_queue {
            if let Some(deadline) = retry_deadline {
                queue.schedule_blocker_preference_reconciliation(profile, token, deadline);
            } else {
                queue.cancel_blocker_preference_reconciliation(profile);
            }
        }
        self.project_blocker_status();
    }

    pub(super) fn on_blocker_preference_reconciliation_retry(
        &mut self,
        profile: ProfileId,
        token: u64,
    ) {
        let now = std::time::Instant::now();
        if self.blocker.profiles.get(&profile).is_some_and(|entry| {
            matches!(
                entry.preference,
                BlockerPreferenceAuthority::Reconciling {
                    token: current,
                    retry_at: Some(retry_at),
                    ..
                } if current == token && now >= retry_at
            )
        }) {
            self.drive_blocker_preference_reconciliations();
        }
    }

    pub(super) fn drive_blocker_preference_reconciliations(&mut self) {
        let now = std::time::Instant::now();
        let due: Vec<_> = self
            .blocker
            .profiles
            .iter()
            .filter_map(|(profile, entry)| match entry.preference {
                BlockerPreferenceAuthority::Reconciling {
                    token,
                    retry_at: Some(retry_at),
                    ..
                } if now >= retry_at => Some((*profile, token)),
                _ => None,
            })
            .collect();
        for (profile, old_token) in due {
            let Some(next_token) = self.blocker.allocate_store_token() else {
                if let Some(entry) = self.blocker.profiles.get_mut(&profile) {
                    entry.preference = BlockerPreferenceAuthority::Unavailable;
                }
                if let Some(queue) = &self.self_queue {
                    queue.cancel_blocker_preference_reconciliation(profile);
                }
                continue;
            };
            let Some(entry) = self.blocker.profiles.get_mut(&profile) else {
                continue;
            };
            let BlockerPreferenceAuthority::Reconciling {
                token,
                attempts_remaining,
                retry_at: Some(_),
            } = entry.preference
            else {
                continue;
            };
            if token != old_token {
                continue;
            }
            entry.preference = BlockerPreferenceAuthority::Reconciling {
                token: next_token,
                attempts_remaining,
                retry_at: None,
            };
            if let Some(queue) = &self.self_queue {
                queue.cancel_blocker_preference_reconciliation(profile);
            }
            self.dispatch_blocker_preference_reconciliation(profile, next_token);
        }
    }

    pub(super) fn consume_blocker_store_result(&mut self, profile: ProfileId) {
        let Some(result) = self.blocker.take_store_result(profile) else {
            return;
        };
        match result.outcome {
            PendingStoreOutcome::Mutation(outcome) => {
                self.consume_blocker_mutation_result(profile, result.token, outcome)
            }
            PendingStoreOutcome::Reconciliation(outcome) => {
                self.consume_blocker_reconciliation_result(profile, result.token, outcome)
            }
        }
    }

    fn consume_blocker_mutation_result(
        &mut self,
        profile: ProfileId,
        token: u64,
        outcome: BlockerConfigUpdateOutcome,
    ) {
        let Some(entry) = self.blocker.profiles.get(&profile) else {
            return;
        };
        let Some(pending) = entry.pending_mutation.as_ref() else {
            return;
        };
        if pending.phase != (PendingMutationPhase::Store { token }) {
            return;
        }
        let expected_revision = pending.expected_revision;
        let requested = pending.requested;
        match outcome {
            BlockerConfigUpdateOutcome::Updated(authoritative)
                if authoritative.profile == profile
                    && Some(authoritative.revision) == expected_revision.next()
                    && authoritative.config == requested =>
            {
                if let Some(entry) = self.blocker.profiles.get_mut(&profile) {
                    entry.config = authoritative;
                    entry.preference = BlockerPreferenceAuthority::Authoritative;
                    if let Some(pending) = entry.pending_mutation.as_mut() {
                        pending.phase = PendingMutationPhase::Native { generation: None };
                    }
                }
                self.drive_blocker_native_target(profile);
            }
            BlockerConfigUpdateOutcome::Conflict(authoritative)
                if authoritative.profile == profile
                    && authoritative.revision > expected_revision =>
            {
                let matching_request = authoritative.config == requested;
                if let Some(entry) = self.blocker.profiles.get_mut(&profile) {
                    entry.config = authoritative;
                    entry.preference = BlockerPreferenceAuthority::Authoritative;
                    if matching_request {
                        if let Some(pending) = entry.pending_mutation.as_mut() {
                            pending.phase = PendingMutationPhase::Native { generation: None };
                            pending.success_outcome = OperationOutcome::NoOp;
                        }
                    } else {
                        entry.background_target = Some(authoritative.config);
                    }
                }
                if matching_request {
                    self.drive_blocker_native_target(profile);
                } else {
                    self.finish_blocker_operation(
                        profile,
                        OperationOutcome::Rejected,
                        OperationReason::StoreConflict,
                    );
                    self.drive_blocker_native_target(profile);
                }
            }
            BlockerConfigUpdateOutcome::OutcomeUnknown => {
                self.finish_blocker_operation(
                    profile,
                    OperationOutcome::Deferred,
                    OperationReason::StoreOutcomeUnknown,
                );
                self.begin_blocker_preference_reconciliation(profile);
            }
            BlockerConfigUpdateOutcome::NotRegistered | BlockerConfigUpdateOutcome::NotAdmitted => {
                if let Some(entry) = self.blocker.profiles.get_mut(&profile) {
                    entry.preference = BlockerPreferenceAuthority::Authoritative;
                }
                self.finish_blocker_operation(
                    profile,
                    OperationOutcome::Rejected,
                    OperationReason::StoreAdmissionRejected,
                );
            }
            BlockerConfigUpdateOutcome::Failed => {
                if let Some(entry) = self.blocker.profiles.get_mut(&profile) {
                    entry.preference = BlockerPreferenceAuthority::Authoritative;
                }
                self.finish_blocker_operation(
                    profile,
                    OperationOutcome::Rejected,
                    OperationReason::StoreReconciliationFailed,
                );
            }
            BlockerConfigUpdateOutcome::Updated(_) | BlockerConfigUpdateOutcome::Conflict(_) => {
                self.finish_blocker_operation(
                    profile,
                    OperationOutcome::Deferred,
                    OperationReason::StoreOutcomeUnknown,
                );
                self.begin_blocker_preference_reconciliation(profile);
            }
        }
        self.project_blocker_status();
    }

    fn consume_blocker_reconciliation_result(
        &mut self,
        profile: ProfileId,
        token: u64,
        outcome: BlockerConfigLoadOutcome,
    ) {
        let Some(entry) = self.blocker.profiles.get(&profile) else {
            return;
        };
        if !matches!(
            entry.preference,
            BlockerPreferenceAuthority::Reconciling {
                token: current,
                retry_at: None,
                ..
            } if current == token
        ) {
            return;
        }
        match outcome {
            BlockerConfigLoadOutcome::Loaded(authoritative) if authoritative.profile == profile => {
                if let Some(queue) = &self.self_queue {
                    queue.cancel_blocker_preference_reconciliation(profile);
                }
                if let Some(entry) = self.blocker.profiles.get_mut(&profile) {
                    entry.config = authoritative;
                    entry.preference = BlockerPreferenceAuthority::Authoritative;
                    entry.background_target = Some(authoritative.config);
                }
                self.drive_blocker_native_target(profile);
            }
            BlockerConfigLoadOutcome::NotRegistered => {
                if let Some(queue) = &self.self_queue {
                    queue.cancel_blocker_preference_reconciliation(profile);
                }
                if let Some(entry) = self.blocker.profiles.get_mut(&profile) {
                    entry.preference = BlockerPreferenceAuthority::Unavailable;
                }
            }
            BlockerConfigLoadOutcome::NotAdmitted | BlockerConfigLoadOutcome::Failed => {
                self.defer_or_fail_blocker_reconciliation(profile, token);
            }
            BlockerConfigLoadOutcome::Loaded(_) => {
                if let Some(queue) = &self.self_queue {
                    queue.cancel_blocker_preference_reconciliation(profile);
                }
                if let Some(entry) = self.blocker.profiles.get_mut(&profile) {
                    entry.preference = BlockerPreferenceAuthority::Unavailable;
                }
            }
        }
        self.project_blocker_status();
    }

    fn drive_blocker_native_target(&mut self, profile: ProfileId) {
        let Some(entry) = self.blocker.profiles.get(&profile) else {
            return;
        };
        if entry.preference != BlockerPreferenceAuthority::Authoritative
            || entry.state == BlockerProfileState::Retired
        {
            return;
        }
        let operation_target = entry.pending_mutation.as_ref().and_then(|pending| {
            matches!(pending.phase, PendingMutationPhase::Native { .. })
                .then_some(pending.requested)
        });
        let target = operation_target.or(entry.background_target);
        let Some(target) = target else {
            return;
        };
        match entry.state {
            BlockerProfileState::Compiling { desired, .. }
            | BlockerProfileState::Installing { desired, .. } => {
                if entry.desired_config == target {
                    if let Some(pending) = self
                        .blocker
                        .profiles
                        .get_mut(&profile)
                        .and_then(|entry| entry.pending_mutation.as_mut())
                    {
                        pending.phase = PendingMutationPhase::Native {
                            generation: Some(desired),
                        };
                    }
                }
                return;
            }
            BlockerProfileState::Ready { applied: _ }
                if entry.applied_config == Some(target)
                    && entry.desired_config == target
                    && (!target.enabled || entry.pending_catalog_revision.is_none()) =>
            {
                if operation_target.is_some() {
                    let outcome = entry
                        .pending_mutation
                        .as_ref()
                        .map(|pending| pending.success_outcome)
                        .unwrap_or(OperationOutcome::Applied);
                    self.finish_blocker_operation(
                        profile,
                        outcome,
                        if outcome == OperationOutcome::NoOp {
                            OperationReason::StateUnchanged
                        } else {
                            OperationReason::MutationApplied
                        },
                    );
                }
                if let Some(entry) = self.blocker.profiles.get_mut(&profile) {
                    entry.background_target = None;
                }
                self.project_blocker_status();
                return;
            }
            _ => {}
        }

        let callback = self.self_queue.as_ref().map(|queue| CallbackHandle {
            queue: Arc::downgrade(&queue.inner),
        });
        let admitted = self.blocker.start_compile(profile, target, callback);
        let generation = self.blocker.profiles.get(&profile).and_then(|entry| {
            if entry.desired_config != target {
                return None;
            }
            match entry.state {
                BlockerProfileState::Compiling { desired, .. }
                | BlockerProfileState::Installing { desired, .. }
                | BlockerProfileState::Failed { desired, .. } => Some(desired),
                BlockerProfileState::Ready { applied } => Some(applied),
                BlockerProfileState::Uninitialized | BlockerProfileState::Retired => None,
            }
        });
        if let Some(entry) = self.blocker.profiles.get_mut(&profile) {
            entry.background_target = None;
            if let Some(pending) = entry.pending_mutation.as_mut() {
                pending.phase = PendingMutationPhase::Native { generation };
            }
        }
        self.finish_terminalized_blocker_native_operations();
        self.project_blocker_status();
        self.consume_blocker_compile_result(profile);
        if !admitted {
            self.finish_terminal_blocker_native_operation(profile);
        }
    }

    fn finish_terminal_blocker_native_operation(&mut self, profile: ProfileId) {
        let Some(entry) = self.blocker.profiles.get(&profile) else {
            return;
        };
        let Some(pending) = entry.pending_mutation.as_ref() else {
            return;
        };
        let PendingMutationPhase::Native {
            generation: Some(generation),
        } = pending.phase
        else {
            return;
        };
        match entry.state {
            BlockerProfileState::Ready { applied }
                if applied == generation && entry.applied_config == Some(pending.requested) =>
            {
                let outcome = pending.success_outcome;
                self.finish_blocker_operation(
                    profile,
                    outcome,
                    if outcome == OperationOutcome::NoOp {
                        OperationReason::StateUnchanged
                    } else {
                        OperationReason::MutationApplied
                    },
                );
            }
            BlockerProfileState::Failed { desired, .. } if desired == generation => {
                self.finish_blocker_operation(
                    profile,
                    OperationOutcome::NativeAdmissionFailed,
                    OperationReason::ContentPolicyApplyFailed,
                );
            }
            _ => {}
        }
    }

    pub(super) fn finish_terminalized_blocker_native_operations(&mut self) {
        let profiles: Vec<_> = self
            .blocker
            .profiles
            .iter()
            .filter_map(|(profile, entry)| {
                let pending_native = entry.pending_mutation.as_ref().is_some_and(|pending| {
                    matches!(pending.phase, PendingMutationPhase::Native { .. })
                });
                let compiler_terminal = matches!(
                    entry.state,
                    BlockerProfileState::Failed {
                        failure: ContentPolicyFailure::CompilerUnavailable,
                        ..
                    }
                );
                (pending_native && compiler_terminal).then_some(*profile)
            })
            .collect();
        for profile in profiles {
            self.finish_blocker_operation(
                profile,
                OperationOutcome::NativeAdmissionFailed,
                OperationReason::ContentPolicyApplyFailed,
            );
        }
    }

    pub(super) fn cancel_pending_blocker_mutation(
        &mut self,
        profile: ProfileId,
        reason: OperationReason,
    ) {
        self.finish_blocker_site_operation(profile, OperationOutcome::Rejected, reason);
        self.finish_blocker_operation(profile, OperationOutcome::Rejected, reason);
        if let Some(entry) = self.blocker.profiles.get_mut(&profile) {
            entry.preference = BlockerPreferenceAuthority::Unavailable;
            entry.background_target = None;
        }
        self.blocker
            .inbox
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .store_results
            .remove(&profile);
        if let Some(queue) = &self.self_queue {
            queue.cancel_blocker_preference_reconciliation(profile);
        }
    }

    pub(super) fn finish_pending_blocker_operations_for_shutdown(&mut self) {
        let profiles: Vec<_> = self.blocker.profiles.keys().copied().collect();
        for profile in profiles {
            self.finish_blocker_site_operation(
                profile,
                OperationOutcome::Deferred,
                OperationReason::StoreOutcomeUnknown,
            );
        }
        let pending: Vec<_> = self
            .blocker
            .profiles
            .iter()
            .filter_map(|(profile, entry)| {
                entry
                    .pending_mutation
                    .as_ref()
                    .map(|pending| (*profile, pending.phase))
            })
            .collect();
        for (profile, phase) in pending {
            let reason = match phase {
                PendingMutationPhase::Store { .. } => OperationReason::StoreOutcomeUnknown,
                PendingMutationPhase::Native { .. } => OperationReason::NativeWorkPending,
            };
            self.finish_blocker_operation(profile, OperationOutcome::Deferred, reason);
        }
        if self.blocker.pending_catalog_refresh.is_some() {
            self.finish_blocker_catalog_refresh(
                OperationOutcome::Deferred,
                OperationReason::ContentPolicySourceRefreshPending,
            );
        }
    }

    /// Drops terminally late compiler and Store results without interpreting
    /// them. Once Store teardown has begun, ordinary result consumers are no
    /// longer safe: an indeterminate mutation can schedule a reconciliation
    /// Store read, and a compile result can dispatch new native policy work.
    /// Pending user operations are settled separately from their last known
    /// phase by [`Self::finish_pending_blocker_operations_for_shutdown`].
    pub(super) fn discard_blocker_inbox_for_shutdown(&mut self) {
        let mut inbox = self
            .blocker
            .inbox
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        inbox.results.clear();
        inbox.store_results.clear();
        inbox.site_results.clear();
    }

    pub(super) fn consume_blocker_compile_result(&mut self, profile: ProfileId) {
        if let Some(entry) = self.blocker.profiles.get_mut(&profile) {
            if entry.desired_config.enabled && !entry.sites.native_ready {
                if entry.sites.failed {
                    if let BlockerProfileState::Compiling {
                        desired,
                        retained,
                        retries_remaining,
                    } = entry.state
                    {
                        entry.state = BlockerCoordinator::failed_state(
                            desired,
                            retained,
                            ContentPolicyFailure::SitePreferencesUnavailable,
                            retries_remaining,
                        );
                    }
                    self.finish_terminal_blocker_native_operation(profile);
                    self.project_blocker_status();
                }
                return;
            }
        }
        let Some(result) = self.blocker.take_compile_result(profile) else {
            return;
        };
        let Some(state) = self.blocker.profiles.get(&profile).map(|entry| entry.state) else {
            return;
        };
        let BlockerProfileState::Compiling {
            desired,
            retained,
            retries_remaining,
        } = state
        else {
            return;
        };
        if desired != result.generation {
            return;
        }
        let rules = match result.outcome {
            BlockerCompileOutcome::Compiled(rules) => rules,
            BlockerCompileOutcome::Failed(failure) => {
                if let Some(entry) = self.blocker.profiles.get_mut(&profile) {
                    entry.pending_coverage = None;
                    entry.pending_runtime_policy = None;
                    if failure != BlockerCompileFailure::SourceUnavailable {
                        entry.compiling_catalog = None;
                    }
                    entry.state = BlockerCoordinator::failed_state(
                        desired,
                        retained,
                        ContentPolicyFailure::Compile(failure),
                        retries_remaining,
                    );
                }
                if failure == BlockerCompileFailure::SourceUnavailable {
                    self.maintain_blocker_catalog();
                    if let Some(entry) = self.blocker.profiles.get_mut(&profile) {
                        if source_material_retry_ready(entry, self.blocker.catalog) {
                            entry.background_target = Some(entry.desired_config);
                        }
                    }
                }
                self.finish_terminal_blocker_native_operation(profile);
                self.drive_blocker_native_target(profile);
                self.project_blocker_status();
                return;
            }
        };
        if self
            .blocker
            .profiles
            .get(&profile)
            .is_none_or(|entry| entry.desired_config.enabled != rules.enabled())
        {
            if let Some(entry) = self.blocker.profiles.get_mut(&profile) {
                entry.pending_coverage = None;
                entry.pending_runtime_policy = None;
                entry.compiling_catalog = None;
                entry.state = BlockerCoordinator::failed_state(
                    desired,
                    retained,
                    ContentPolicyFailure::CompiledArtifactMismatch,
                    retries_remaining,
                );
            }
            self.finish_terminal_blocker_native_operation(profile);
            self.drive_blocker_native_target(profile);
            self.project_blocker_status();
            return;
        }
        let coverage = rules.coverage();
        let runtime_policy = rules.runtime_policy().map(|policy| RuntimePolicyObserver {
            digest: rules.digest(),
            policy: std::sync::Arc::downgrade(policy),
        });
        let admission = self.engine.install_content_rules(profile, desired, rules);
        match admission {
            NativeDispatch::Scheduled => {
                if let Some(entry) = self.blocker.profiles.get_mut(&profile) {
                    entry.pending_coverage = Some((desired, coverage));
                    entry.pending_runtime_policy = runtime_policy.map(|policy| (desired, policy));
                    entry.state = BlockerProfileState::Installing {
                        desired,
                        retained,
                        retries_remaining,
                    };
                }
                self.project_blocker_status();
                #[cfg(test)]
                if self.auto_settle_content_rules {
                    self.on_content_rules_settled(
                        profile,
                        desired,
                        ContentRuleSettlement::Applied {
                            generation: desired,
                        },
                    );
                }
            }
            NativeDispatch::Rejected => {
                if let Some(entry) = self.blocker.profiles.get_mut(&profile) {
                    entry.pending_coverage = None;
                    entry.pending_runtime_policy = None;
                    entry.compiling_catalog = None;
                    entry.state = BlockerCoordinator::failed_state(
                        desired,
                        retained,
                        ContentPolicyFailure::NativeDispatchRejected,
                        retries_remaining,
                    );
                }
                self.project_blocker_status();
            }
            NativeDispatch::Unsupported => {
                if let Some(entry) = self.blocker.profiles.get_mut(&profile) {
                    entry.pending_coverage = None;
                    entry.pending_runtime_policy = None;
                    entry.compiling_catalog = None;
                    entry.state = BlockerCoordinator::failed_state(
                        desired,
                        retained,
                        ContentPolicyFailure::NativeUnsupported,
                        retries_remaining,
                    );
                }
                self.project_blocker_status();
            }
        }
        self.finish_terminal_blocker_native_operation(profile);
        self.drive_blocker_native_target(profile);
    }

    pub(super) fn drain_blocker_inbox(&mut self) {
        let site_profiles: Vec<_> = self
            .blocker
            .inbox
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .site_results
            .keys()
            .copied()
            .collect();
        for profile in site_profiles {
            self.consume_blocker_site_result(profile);
        }
        for profile in self.blocker.pending_result_profiles() {
            self.consume_blocker_compile_result(profile);
        }
        for profile in self.blocker.pending_store_result_profiles() {
            self.consume_blocker_store_result(profile);
        }
    }

    pub(super) fn on_content_rules_settled(
        &mut self,
        profile: ProfileId,
        requested: ContentPolicyGeneration,
        settlement: ContentRuleSettlement,
    ) {
        let Some(state) = self.blocker.profiles.get(&profile).map(|entry| entry.state) else {
            return;
        };
        let BlockerProfileState::Installing {
            desired,
            retained,
            retries_remaining,
        } = state
        else {
            return;
        };
        if requested != desired {
            return;
        }
        let attempted_catalog_revision = self
            .blocker
            .profiles
            .get(&profile)
            .and_then(|entry| entry.compiling_catalog)
            .filter(|attempt| attempt.generation == desired)
            .map(|attempt| attempt.installed.revision);
        if let Some(entry) = self.blocker.profiles.get_mut(&profile) {
            entry.compiling_catalog = None;
        }
        match settlement {
            ContentRuleSettlement::Applied { generation } if generation == desired => {
                if let Some(entry) = self.blocker.profiles.get_mut(&profile) {
                    let coverage = entry
                        .pending_coverage
                        .take()
                        .filter(|(generation, _)| *generation == desired)
                        .map(|(_, coverage)| coverage);
                    let runtime_policy = entry
                        .pending_runtime_policy
                        .take()
                        .filter(|(generation, _)| *generation == desired)
                        .map(|(_, policy)| policy);
                    if let Some(coverage) = coverage {
                        entry.applied_config = Some(entry.desired_config);
                        entry.applied_coverage = Some(coverage);
                        entry.applied_runtime_policy = settled_runtime_policy_observer(
                            entry.applied_runtime_policy.take(),
                            runtime_policy,
                        );
                        if entry.desired_config.enabled {
                            entry.applied_catalog_revision = attempted_catalog_revision;
                            if entry.pending_catalog_revision.is_some_and(|pending| {
                                attempted_catalog_revision.is_some_and(|applied| applied >= pending)
                            }) {
                                entry.pending_catalog_revision = None;
                            }
                        } else {
                            entry.applied_catalog_revision = None;
                            entry.pending_catalog_revision = None;
                        }
                        entry.state = BlockerProfileState::Ready { applied: desired };
                    } else {
                        entry.applied_config = None;
                        entry.applied_coverage = None;
                        entry.applied_runtime_policy = None;
                        entry.applied_catalog_revision = None;
                        entry.state = BlockerCoordinator::failed_state(
                            desired,
                            None,
                            ContentPolicyFailure::ContradictoryNativeSettlement,
                            0,
                        );
                        self.finish_terminal_blocker_native_operation(profile);
                        self.drive_blocker_native_target(profile);
                        self.project_blocker_status();
                        return;
                    }
                }
                let effects = self.blocker.take_held_effects(profile);
                if !effects.is_empty() {
                    let _ = self.apply(effects);
                    let _ = self.relayout();
                    self.maintain_views();
                    self.project_items();
                }
            }
            ContentRuleSettlement::Retained {
                failure: zephium_core::blocker::ContentRuleApplyFailure::NativeCleanup,
                ..
            } => {
                // Cleanup failure means multiple registrations may coexist;
                // even an otherwise matching retained generation is not an
                // authoritative policy. The native adapter separately drives
                // terminal process handling, while the shell immediately
                // refuses all further view admission.
                if let Some(entry) = self.blocker.profiles.get_mut(&profile) {
                    entry.applied_config = None;
                    entry.pending_coverage = None;
                    entry.applied_coverage = None;
                    entry.pending_runtime_policy = None;
                    entry.applied_runtime_policy = None;
                    entry.applied_catalog_revision = None;
                    entry.state = BlockerCoordinator::failed_state(
                        desired,
                        None,
                        ContentPolicyFailure::Native(
                            zephium_core::blocker::ContentRuleApplyFailure::NativeCleanup,
                        ),
                        0,
                    );
                }
            }
            ContentRuleSettlement::Retained {
                generation,
                failure,
            } if Some(generation) == retained => {
                if let Some(entry) = self.blocker.profiles.get_mut(&profile) {
                    entry.pending_coverage = None;
                    entry.pending_runtime_policy = None;
                    entry.state = BlockerCoordinator::failed_state(
                        desired,
                        Some(generation),
                        ContentPolicyFailure::Native(failure),
                        retries_remaining,
                    );
                }
            }
            ContentRuleSettlement::Unavailable { failure } => {
                if let Some(entry) = self.blocker.profiles.get_mut(&profile) {
                    entry.applied_config = None;
                    entry.pending_coverage = None;
                    entry.applied_coverage = None;
                    entry.pending_runtime_policy = None;
                    entry.applied_runtime_policy = None;
                    entry.applied_catalog_revision = None;
                    entry.state = BlockerCoordinator::failed_state(
                        desired,
                        None,
                        ContentPolicyFailure::Native(failure),
                        retries_remaining,
                    );
                }
            }
            ContentRuleSettlement::Applied { .. } | ContentRuleSettlement::Retained { .. } => {
                // A contradictory generation is not authoritative proof of
                // either the desired policy or the previously retained one.
                crate::diagnostic!(
                    "content blocker: rejected contradictory native policy settlement"
                );
                if let Some(entry) = self.blocker.profiles.get_mut(&profile) {
                    entry.applied_config = None;
                    entry.pending_coverage = None;
                    entry.applied_coverage = None;
                    entry.pending_runtime_policy = None;
                    entry.applied_runtime_policy = None;
                    entry.applied_catalog_revision = None;
                    entry.state = BlockerCoordinator::failed_state(
                        desired,
                        None,
                        ContentPolicyFailure::ContradictoryNativeSettlement,
                        0,
                    );
                }
            }
        }
        self.finish_terminal_blocker_native_operation(profile);
        self.drive_blocker_native_target(profile);
        self.project_blocker_status();
    }

    pub(super) fn blocker_gate_effects(
        &mut self,
        effects: Vec<Effect>,
    ) -> (Vec<Effect>, NativeWork) {
        let mut admitted = Vec::with_capacity(effects.len());
        let mut native = NativeWork::default();
        for effect in effects {
            if let Effect::Close { id } = &effect {
                if self.blocker.remove_held_item_effects(*id) {
                    // The matching CreateView never crossed the blocker gate,
                    // so there is no native view to close.
                    native.scheduled = true;
                } else {
                    // A held Navigate can belong to an already materialized
                    // view. Removing it must not consume the native Close.
                    admitted.push(effect);
                }
                continue;
            }
            let id = match &effect {
                Effect::CreateView { id, .. } | Effect::Navigate { id, .. } => *id,
                Effect::Close { .. } => unreachable!("close effects were handled above"),
            };
            let Some(profile) = self.profile_of_item(id) else {
                native.rejected = true;
                continue;
            };
            if self
                .blocker
                .profiles
                .get(&profile)
                .is_some_and(|entry| entry.state == BlockerProfileState::Uninitialized)
                && !self.start_blocker_profile(profile)
            {
                crate::diagnostic!(
                    "content blocker: profile policy compilation was not admitted; views remain unavailable"
                );
            }
            if self.blocker.native_policy_available(profile) {
                admitted.push(effect);
            } else if self.blocker.hold_effect(profile, effect) {
                native.scheduled = true;
            } else {
                let reason = match self.blocker.profiles.get(&profile) {
                    None => "profile-absent",
                    Some(entry) if entry.state == BlockerProfileState::Retired => "profile-retired",
                    Some(entry) if entry.held_effects.len() >= MAX_HELD_EFFECTS_PER_PROFILE => {
                        "hold-capacity"
                    }
                    Some(_) => "hold-refused",
                };
                crate::diagnostic!("view-create: content policy hold refused reason={reason}");
                self.rollback_blocker_rejected_effect(id);
                native.rejected = true;
            }
        }
        (admitted, native)
    }

    fn rollback_blocker_rejected_effect(&mut self, id: ItemId) {
        self.zoom.pending.remove(&id);
        self.items.view_creation_failed(id);
    }
}

fn blocker_status_view(
    focused: FocusedBlockerStatus,
    runtime_diagnostics: Option<NetworkPolicyDiagnostics>,
    projection_revision: u128,
) -> BlockerStatusView {
    let revision = format!("{projection_revision:032x}");
    let (
        status,
        preference_authority,
        applied_catalog_revision,
        source_material_retry_ready,
        catalog,
    ) = match focused {
        FocusedBlockerStatus::Unavailable { catalog } => {
            let mut unavailable = BlockerStatusView::unavailable();
            unavailable.projection_revision = revision;
            apply_catalog_status(&mut unavailable, catalog);
            return unavailable;
        }
        FocusedBlockerStatus::Found {
            status,
            preference,
            applied_catalog_revision,
            source_material_retry_ready,
            catalog,
        } => (
            status,
            preference,
            applied_catalog_revision,
            source_material_retry_ready,
            catalog,
        ),
    };

    let (phase, desired, retained, failure, retries_remaining) = match status.state {
        ProfileContentPolicyState::Uninitialized => {
            (BlockerPhase::Uninitialized, None, None, None, 0)
        }
        ProfileContentPolicyState::Compiling {
            desired,
            retained,
            retries_remaining,
        } => (
            BlockerPhase::Compiling,
            Some(desired),
            retained,
            None,
            retries_remaining,
        ),
        ProfileContentPolicyState::Installing {
            desired,
            retained,
            retries_remaining,
        } => (
            BlockerPhase::Installing,
            Some(desired),
            retained,
            None,
            retries_remaining,
        ),
        ProfileContentPolicyState::Ready { applied } => {
            (BlockerPhase::Ready, Some(applied), Some(applied), None, 0)
        }
        ProfileContentPolicyState::Failed {
            desired,
            retained,
            failure,
            retries_remaining,
        } => (
            BlockerPhase::Failed,
            Some(desired),
            retained,
            Some(failure),
            retries_remaining,
        ),
        ProfileContentPolicyState::Retired => (BlockerPhase::Retired, None, None, None, 0),
    };

    let authoritative_applied = status
        .applied_config
        .zip(status.applied_coverage)
        .filter(|_| {
            matches!(status.state, ProfileContentPolicyState::Ready { .. }) || retained.is_some()
        });
    let base_protection = if matches!(
        preference_authority,
        BlockerPreferenceAuthority::Reconciling { .. } | BlockerPreferenceAuthority::Unavailable
    ) {
        BlockerProtection::Unavailable
    } else {
        match status.state {
            ProfileContentPolicyState::Uninitialized => BlockerProtection::Pending,
            ProfileContentPolicyState::Compiling { .. }
            | ProfileContentPolicyState::Installing { .. } => match authoritative_applied {
                Some((config, _)) if config.enabled => BlockerProtection::Active,
                Some((config, _)) if !status.desired_config.enabled && !config.enabled => {
                    BlockerProtection::Disabled
                }
                _ => BlockerProtection::Pending,
            },
            ProfileContentPolicyState::Ready { .. } => match authoritative_applied {
                Some((config, _)) if config.enabled => BlockerProtection::Active,
                Some(_) if status.desired_config.enabled => BlockerProtection::Pending,
                Some(_) => BlockerProtection::Disabled,
                None => BlockerProtection::Unavailable,
            },
            ProfileContentPolicyState::Failed { .. } => match authoritative_applied {
                Some((config, _)) if config.enabled => BlockerProtection::Degraded,
                Some((config, _)) if !status.desired_config.enabled && !config.enabled => {
                    BlockerProtection::Disabled
                }
                _ => BlockerProtection::Unavailable,
            },
            ProfileContentPolicyState::Retired => BlockerProtection::Unavailable,
        }
    };
    let protection = if base_protection == BlockerProtection::Active
        && !catalog_proves_current_policy(catalog, applied_catalog_revision)
    {
        BlockerProtection::Degraded
    } else {
        base_protection
    };
    let retryable = failure.is_some_and(|failure| {
        failure.retryable()
            && (failure != ContentPolicyFailure::Compile(BlockerCompileFailure::SourceUnavailable)
                || source_material_retry_ready)
    }) && retries_remaining > 0;

    let mut view = BlockerStatusView {
        projection_revision: revision,
        protection,
        phase,
        preference: match preference_authority {
            BlockerPreferenceAuthority::Authoritative => BlockerPreferenceState::Authoritative,
            BlockerPreferenceAuthority::Updating => BlockerPreferenceState::Updating,
            BlockerPreferenceAuthority::Reconciling { .. } => BlockerPreferenceState::Reconciling,
            BlockerPreferenceAuthority::Unavailable => BlockerPreferenceState::Unavailable,
        },
        config_revision: Some(format!("{:016x}", status.config_revision.get())),
        desired_enabled: Some(status.desired_config.enabled),
        applied_enabled: authoritative_applied.map(|(config, _)| config.enabled),
        desired_generation: desired.map(|generation| format!("{:016x}", generation.get())),
        retained_generation: retained.map(|generation| format!("{:016x}", generation.get())),
        failure: failure.map(blocker_failure_view),
        retryable,
        retries_remaining,
        applied_coverage: authoritative_applied
            .and_then(|(_, coverage)| blocker_coverage_view(coverage))
            .map(Box::new),
        runtime_diagnostics: runtime_diagnostics
            .map(blocker_runtime_diagnostics_view)
            .map(Box::new),
        ..BlockerStatusView::unavailable()
    };
    apply_catalog_status(&mut view, catalog);
    view
}

impl CatalogAuthority {
    fn from_initial(catalog: BlockerCatalogSnapshot) -> Option<Self> {
        let current = catalog_current_identity(catalog).ok().flatten();
        let candidate = catalog_candidate_identity(catalog).ok().flatten();
        let installed = catalog_installed_identity(catalog).ok().flatten();
        if installed.is_some()
            && !current.zip(installed).is_some_and(|(current, installed)| {
                current.revision == installed.revision
                    && current.manifest_sha256 == installed.manifest_sha256
                    && current.provenance == installed.provenance
            })
        {
            return None;
        }
        Some(Self {
            current,
            high_water: newest_catalog_identity(current, candidate),
            installed,
        })
    }

    /// Admits only monotonic, exact package authority.
    ///
    /// A rejected candidate may leave the durable catalog high-water ahead of
    /// current. The unchanged current is therefore allowed to remain below
    /// `high_water`, but any newly observed current or candidate must
    /// match that high-water exactly or advance it.
    fn advance(
        self,
        previous: BlockerCatalogSnapshot,
        observed: BlockerCatalogSnapshot,
    ) -> Option<Self> {
        if previous.enabled_policy_terminal && !observed.enabled_policy_terminal {
            return None;
        }
        if observed.source_material_epoch < previous.source_material_epoch {
            return None;
        }
        let next_current = catalog_current_identity(observed).ok()?;
        let next_candidate = catalog_candidate_identity(observed).ok()?;
        let next_installed = catalog_installed_identity(observed).ok()?;
        if observed.source_material_epoch > previous.source_material_epoch {
            let exact_installed_current =
                next_current
                    .zip(next_installed)
                    .is_some_and(|(current, installed)| {
                        current.revision == installed.revision
                            && current.manifest_sha256 == installed.manifest_sha256
                            && current.provenance == installed.provenance
                    });
            if self.current != next_current
                || self.installed != next_installed
                || !exact_installed_current
            {
                return None;
            }
        }

        match (self.current, next_current) {
            (Some(_), None) => return None,
            (Some(before), Some(after)) if after.revision < before.revision => return None,
            (Some(before), Some(after)) if after.revision == before.revision && after != before => {
                return None;
            }
            _ => {}
        }
        let verified_official = next_current
            .is_some_and(|identity| identity.provenance == BlockerCatalogProvenance::OfficialHttps)
            && observed.source_material_epoch > previous.source_material_epoch;
        if !verified_official
            && self.current == next_current
            && previous.package_stale == Some(true)
            && observed.package_stale == Some(false)
        {
            return None;
        }
        if !verified_official
            && self.current == next_current
            && previous.source_refresh_due
            && !observed.source_refresh_due
        {
            return None;
        }

        let mut high_water = self.high_water;
        if next_current != self.current {
            admit_catalog_identity(&mut high_water, next_current?)?;
        }
        if let Some(candidate) = next_candidate {
            admit_catalog_identity(&mut high_water, candidate)?;
        }

        match (self.installed, next_installed) {
            (Some(_), None) => return None,
            (Some(before), Some(after)) if after.revision < before.revision => return None,
            (Some(before), Some(after))
                if after.revision == before.revision
                    && (after.manifest_sha256 != before.manifest_sha256
                        || after.provenance != before.provenance) =>
            {
                return None;
            }
            _ => {}
        }
        if next_installed != self.installed {
            let (Some(current), Some(installed)) = (next_current, next_installed) else {
                return None;
            };
            if installed.revision != current.revision
                || installed.manifest_sha256 != current.manifest_sha256
                || installed.provenance != current.provenance
            {
                return None;
            }
        }

        Some(Self {
            current: next_current,
            high_water,
            installed: next_installed,
        })
    }
}

fn admit_catalog_identity(
    highest: &mut Option<CatalogPackageIdentity>,
    observed: CatalogPackageIdentity,
) -> Option<()> {
    match *highest {
        Some(current) if observed.revision < current.revision => None,
        Some(current) if observed.revision == current.revision && observed != current => None,
        Some(current) if observed.revision == current.revision => Some(()),
        Some(current) if observed.manifest_sha256 == current.manifest_sha256 => None,
        _ => {
            *highest = Some(observed);
            Some(())
        }
    }
}

fn newest_catalog_identity(
    first: Option<CatalogPackageIdentity>,
    second: Option<CatalogPackageIdentity>,
) -> Option<CatalogPackageIdentity> {
    match (first, second) {
        (Some(first), Some(second)) if second.revision > first.revision => Some(second),
        (Some(first), _) => Some(first),
        (None, second) => second,
    }
}

fn catalog_current_identity(
    catalog: BlockerCatalogSnapshot,
) -> Result<Option<CatalogPackageIdentity>, ()> {
    catalog_package_identity(
        catalog.package_revision,
        catalog.package_manifest_sha256,
        catalog.package_provenance,
        catalog.package_created_unix,
        catalog.package_expires_unix,
        catalog.source_count,
        catalog.source_bytes,
    )
}

fn catalog_candidate_identity(
    catalog: BlockerCatalogSnapshot,
) -> Result<Option<CatalogPackageIdentity>, ()> {
    catalog_package_identity(
        catalog.candidate_revision,
        catalog.candidate_manifest_sha256,
        catalog.candidate_provenance,
        catalog.candidate_created_unix,
        catalog.candidate_expires_unix,
        catalog.candidate_source_count,
        catalog.candidate_source_bytes,
    )
}

fn catalog_package_identity(
    revision: Option<u64>,
    manifest_sha256: Option<[u8; 32]>,
    provenance: Option<BlockerCatalogProvenance>,
    created_unix: Option<u64>,
    expires_unix: Option<u64>,
    source_count: Option<u32>,
    source_bytes: Option<u64>,
) -> Result<Option<CatalogPackageIdentity>, ()> {
    match (
        revision,
        manifest_sha256,
        provenance,
        created_unix,
        expires_unix,
        source_count,
        source_bytes,
    ) {
        (None, None, None, None, None, None, None) => Ok(None),
        (
            Some(revision),
            Some(manifest_sha256),
            Some(provenance),
            Some(created_unix),
            Some(expires_unix),
            Some(source_count),
            Some(source_bytes),
        ) if revision > 0
            && created_unix <= expires_unix
            && source_count > 0
            && source_bytes > 0 =>
        {
            Ok(Some(CatalogPackageIdentity {
                revision,
                manifest_sha256,
                created_unix,
                expires_unix,
                source_count,
                source_bytes,
                provenance,
            }))
        }
        _ => Err(()),
    }
}

fn catalog_installed_identity(
    catalog: BlockerCatalogSnapshot,
) -> Result<Option<CatalogInstalledIdentity>, ()> {
    match (
        catalog.installed_revision,
        catalog.installed_manifest_sha256,
        catalog.installed_provenance,
    ) {
        (None, None, None) => Ok(None),
        (Some(revision), Some(manifest_sha256), Some(provenance)) if revision > 0 => {
            Ok(Some(CatalogInstalledIdentity {
                revision,
                manifest_sha256,
                provenance,
            }))
        }
        _ => Err(()),
    }
}

fn source_material_retry_ready(entry: &BlockerProfile, catalog: BlockerCatalogSnapshot) -> bool {
    let BlockerProfileState::Failed {
        desired,
        failure: ContentPolicyFailure::Compile(BlockerCompileFailure::SourceUnavailable),
        ..
    } = entry.state
    else {
        return false;
    };
    let repaired = catalog_installed_identity(catalog).ok().flatten();
    entry.desired_config.enabled
        && !catalog.source_material_repair_pending
        && !catalog.source_material_repair_retry_pending
        && entry.compiling_catalog.is_some_and(|attempt| {
            attempt.generation == desired
                && Some(attempt.installed) == repaired
                && attempt.source_material_epoch < catalog.source_material_epoch
        })
}

pub(super) fn catalog_snapshot_valid(catalog: BlockerCatalogSnapshot) -> bool {
    if catalog.revision == 0 {
        return false;
    }
    let Ok(current) = catalog_current_identity(catalog) else {
        return false;
    };
    let Ok(candidate) = catalog_candidate_identity(catalog) else {
        return false;
    };
    let Ok(installed) = catalog_installed_identity(catalog) else {
        return false;
    };
    let provenances = [
        current.map(|identity| identity.provenance),
        candidate.map(|identity| identity.provenance),
        installed.map(|identity| identity.provenance),
    ];
    if provenances.into_iter().flatten().any(|provenance| {
        matches!(
            (catalog.refresh_supported, provenance),
            (
                false,
                BlockerCatalogProvenance::TufRepository | BlockerCatalogProvenance::OfficialHttps
            )
        )
    }) {
        return false;
    }
    if current.is_some() != catalog.package_stale.is_some() {
        return false;
    }
    if current.is_none() && catalog.source_refresh_due {
        return false;
    }
    if let (Some(current), Some(candidate)) = (current, candidate) {
        if candidate.revision < current.revision
            || (candidate.revision == current.revision && candidate != current)
        {
            return false;
        }
    }
    if let Some(installed) = installed {
        let Some(current) = current else {
            return false;
        };
        if installed.revision > current.revision
            || (installed.revision == current.revision
                && (installed.manifest_sha256 != current.manifest_sha256
                    || installed.provenance != current.provenance))
        {
            return false;
        }
    }
    if candidate.is_some() != catalog.activation_pending {
        return false;
    }
    let installed_current = current.zip(installed).is_some_and(|(current, installed)| {
        current.revision == installed.revision
            && current.manifest_sha256 == installed.manifest_sha256
            && current.provenance == installed.provenance
    });
    let source_material_repair =
        catalog.source_material_repair_pending || catalog.source_material_repair_retry_pending;
    if !catalog.refresh_supported
        && (catalog.activation_pending
            || catalog.repair_retry_pending
            || source_material_repair
            || catalog.last_refresh_attempt_unix.is_some()
            || catalog.refresh_operation.is_some()
            || matches!(
                catalog.phase,
                BlockerCatalogPhase::Idle | BlockerCatalogPhase::Refreshing
            ))
    {
        return false;
    }
    if source_material_repair
        && (!installed_current
            || catalog.enabled_policy_terminal
            || matches!(
                catalog.phase,
                BlockerCatalogPhase::Unavailable(_)
                    | BlockerCatalogPhase::Idle
                    | BlockerCatalogPhase::Shutdown
            ))
    {
        return false;
    }
    if catalog.source_material_repair_pending && catalog.source_material_repair_retry_pending {
        return false;
    }
    if catalog.source_material_repair_retry_pending
        && !matches!(
            catalog.phase,
            BlockerCatalogPhase::Fresh
                | BlockerCatalogPhase::Stale
                | BlockerCatalogPhase::Failed(_)
        )
    {
        return false;
    }
    if catalog.repair_retry_pending
        && (!catalog.activation_pending || !matches!(catalog.phase, BlockerCatalogPhase::Failed(_)))
    {
        return false;
    }
    if catalog.activation_pending {
        let current_installed = current.zip(installed).is_some_and(|(current, installed)| {
            current.revision == installed.revision
                && current.manifest_sha256 == installed.manifest_sha256
                && current.provenance == installed.provenance
        });
        if current.is_some() && current_installed && candidate == current {
            return false;
        }
    }
    let phase_valid = match catalog.phase {
        BlockerCatalogPhase::Fresh => {
            catalog.package_revision.is_some() && catalog.package_stale == Some(false)
        }
        BlockerCatalogPhase::Stale => {
            catalog.package_revision.is_some() && catalog.package_stale == Some(true)
        }
        BlockerCatalogPhase::Idle => catalog.package_revision.is_none(),
        BlockerCatalogPhase::Unavailable(_)
        | BlockerCatalogPhase::Refreshing
        | BlockerCatalogPhase::Failed(_)
        | BlockerCatalogPhase::Shutdown => true,
    };
    let operation_valid = match catalog.phase {
        BlockerCatalogPhase::Refreshing => catalog.refresh_operation.is_some(),
        BlockerCatalogPhase::Unavailable(_)
        | BlockerCatalogPhase::Idle
        | BlockerCatalogPhase::Fresh
        | BlockerCatalogPhase::Stale
        | BlockerCatalogPhase::Shutdown => catalog.refresh_operation.is_none(),
        BlockerCatalogPhase::Failed(_) => true,
    };
    phase_valid && operation_valid
}

fn catalog_has_compiler_policy(catalog: BlockerCatalogSnapshot) -> bool {
    !catalog.enabled_policy_terminal
        && !catalog.source_material_repair_pending
        && !catalog.source_material_repair_retry_pending
        && catalog.package_revision.is_some()
        && catalog.package_revision == catalog.installed_revision
        && !matches!(
            catalog.phase,
            BlockerCatalogPhase::Unavailable(_)
                | BlockerCatalogPhase::Idle
                | BlockerCatalogPhase::Shutdown
        )
}

fn catalog_proves_current_policy(
    catalog: BlockerCatalogSnapshot,
    applied_catalog_revision: Option<u64>,
) -> bool {
    !catalog.activation_pending
        && catalog.package_stale == Some(false)
        && catalog.package_revision.is_some()
        && catalog.package_revision == catalog.installed_revision
        && catalog.installed_revision == applied_catalog_revision
        && matches!(
            catalog.phase,
            BlockerCatalogPhase::Fresh | BlockerCatalogPhase::Refreshing
        )
}

fn apply_catalog_status(view: &mut BlockerStatusView, catalog: BlockerCatalogSnapshot) {
    let (phase, failure) = match catalog.phase {
        BlockerCatalogPhase::Unavailable(reason) => (
            match reason {
                zephium_core::ports::blocker::BlockerCatalogUnavailable::NotConfigured => {
                    BlockerSourcePhase::NotConfigured
                }
                zephium_core::ports::blocker::BlockerCatalogUnavailable::DurableActivationUnsupported => {
                    BlockerSourcePhase::DurableActivationUnsupported
                }
                zephium_core::ports::blocker::BlockerCatalogUnavailable::StorageUnavailable => {
                    BlockerSourcePhase::StorageUnavailable
                }
                zephium_core::ports::blocker::BlockerCatalogUnavailable::ClockUnsafe => {
                    BlockerSourcePhase::ClockUnsafe
                }
            },
            None,
        ),
        BlockerCatalogPhase::Idle => (BlockerSourcePhase::Idle, None),
        BlockerCatalogPhase::Fresh => (BlockerSourcePhase::Fresh, None),
        BlockerCatalogPhase::Stale => (BlockerSourcePhase::Stale, None),
        BlockerCatalogPhase::Refreshing => (BlockerSourcePhase::Refreshing, None),
        BlockerCatalogPhase::Failed(failure) => (
            BlockerSourcePhase::Failed,
            Some(match failure {
                BlockerCatalogFailure::Transport => BlockerSourceFailure::Transport,
                BlockerCatalogFailure::Metadata => BlockerSourceFailure::Metadata,
                BlockerCatalogFailure::Clock => BlockerSourceFailure::Clock,
                BlockerCatalogFailure::Manifest => BlockerSourceFailure::Manifest,
                BlockerCatalogFailure::Target => BlockerSourceFailure::Target,
                BlockerCatalogFailure::License => BlockerSourceFailure::License,
                BlockerCatalogFailure::Rollback => BlockerSourceFailure::Rollback,
                BlockerCatalogFailure::Storage => BlockerSourceFailure::Storage,
                BlockerCatalogFailure::Catalog => BlockerSourceFailure::Catalog,
                BlockerCatalogFailure::Internal => BlockerSourceFailure::Internal,
            }),
        ),
        BlockerCatalogPhase::Shutdown => (BlockerSourcePhase::Shutdown, None),
    };
    view.source_phase = phase;
    view.source_failure = failure;
    view.source_package_revision = catalog
        .package_revision
        .map(|revision| format!("{revision:016x}"));
    view.source_installed_revision = catalog
        .installed_revision
        .map(|revision| format!("{revision:016x}"));
    view.source_package_provenance = catalog.package_provenance.map(source_provenance_view);
    view.source_installed_provenance = catalog.installed_provenance.map(source_provenance_view);
    if catalog.package_manifest_sha256.is_some()
        || catalog.candidate_revision.is_some()
        || catalog.candidate_manifest_sha256.is_some()
        || catalog.installed_manifest_sha256.is_some()
    {
        view.source_identities = Some(Box::new(BlockerSourceIdentities {
            package_manifest_sha256: catalog.package_manifest_sha256.map(format_manifest_sha256),
            candidate_revision: catalog
                .candidate_revision
                .map(|revision| format!("{revision:016x}")),
            candidate_manifest_sha256: catalog
                .candidate_manifest_sha256
                .map(format_manifest_sha256),
            installed_manifest_sha256: catalog
                .installed_manifest_sha256
                .map(format_manifest_sha256),
        }));
    }
    view.source_package_created_unix = catalog.package_created_unix.map(|value| value.to_string());
    view.source_package_expires_unix = catalog.package_expires_unix.map(|value| value.to_string());
    view.source_package_stale = catalog.package_stale;
    view.source_refresh_due = catalog.source_refresh_due;
    view.source_count = catalog.source_count;
    view.source_bytes = catalog
        .source_bytes
        .and_then(|value| u32::try_from(value).ok());
    view.source_activation_pending = catalog.activation_pending;
    view.source_repair_retry_pending = catalog.repair_retry_pending;
    view.source_material_repair_pending = catalog.source_material_repair_pending;
    view.source_material_repair_retry_pending = catalog.source_material_repair_retry_pending;
    view.source_last_refresh_attempt_unix = catalog
        .last_refresh_attempt_unix
        .map(|value| value.to_string());
    view.source_refresh_operation = catalog
        .refresh_operation
        .map(|operation| format!("{operation:016x}"));
    view.can_enable = catalog_has_compiler_policy(catalog);
    view.can_refresh_sources = catalog.refresh_supported
        && !catalog.enabled_policy_terminal
        && (!catalog.activation_pending || catalog.repair_retry_pending)
        && !matches!(
            catalog.phase,
            BlockerCatalogPhase::Unavailable(_)
                | BlockerCatalogPhase::Refreshing
                | BlockerCatalogPhase::Shutdown
        );
}

const fn source_provenance_view(provenance: BlockerCatalogProvenance) -> BlockerSourceProvenance {
    match provenance {
        BlockerCatalogProvenance::ReleaseBundle => BlockerSourceProvenance::ReleaseBundle,
        BlockerCatalogProvenance::TufRepository => BlockerSourceProvenance::TufRepository,
        BlockerCatalogProvenance::OfficialHttps => BlockerSourceProvenance::OfficialHttps,
    }
}

fn format_manifest_sha256(digest: [u8; 32]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(64);
    for byte in digest {
        encoded.push(char::from(HEX[usize::from(byte >> 4)]));
        encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    encoded
}

fn blocker_failure_view(failure: ContentPolicyFailure) -> BlockerFailure {
    match failure {
        ContentPolicyFailure::SitePreferencesUnavailable => {
            BlockerFailure::SitePreferencesUnavailable
        }
        ContentPolicyFailure::GenerationExhausted => BlockerFailure::GenerationExhausted,
        ContentPolicyFailure::CompilerDispatchRejected => BlockerFailure::CompilerDispatchRejected,
        ContentPolicyFailure::CompilerUnavailable => BlockerFailure::CompilerUnavailable,
        ContentPolicyFailure::Compile(BlockerCompileFailure::SourceUnavailable) => {
            BlockerFailure::CompileSourceUnavailable
        }
        ContentPolicyFailure::Compile(BlockerCompileFailure::InvalidSource) => {
            BlockerFailure::CompileInvalidSource
        }
        ContentPolicyFailure::Compile(BlockerCompileFailure::ResourceLimit) => {
            BlockerFailure::CompileResourceLimit
        }
        ContentPolicyFailure::Compile(BlockerCompileFailure::Internal) => {
            BlockerFailure::CompileInternal
        }
        ContentPolicyFailure::CompiledArtifactMismatch => BlockerFailure::CompiledArtifactMismatch,
        ContentPolicyFailure::NativeDispatchRejected => BlockerFailure::NativeDispatchRejected,
        ContentPolicyFailure::NativeUnsupported => BlockerFailure::NativeUnsupported,
        ContentPolicyFailure::Native(ContentRuleApplyFailure::UnsupportedArtifact) => {
            BlockerFailure::NativeUnsupportedArtifact
        }
        ContentPolicyFailure::Native(ContentRuleApplyFailure::InvalidArtifact) => {
            BlockerFailure::NativeInvalidArtifact
        }
        ContentPolicyFailure::Native(ContentRuleApplyFailure::NativeCompilation) => {
            BlockerFailure::NativeCompilation
        }
        ContentPolicyFailure::Native(ContentRuleApplyFailure::NativeInstallation) => {
            BlockerFailure::NativeInstallation
        }
        ContentPolicyFailure::Native(ContentRuleApplyFailure::NativeCleanup) => {
            BlockerFailure::NativeCleanup
        }
        ContentPolicyFailure::Native(ContentRuleApplyFailure::Superseded) => {
            BlockerFailure::NativeSuperseded
        }
        ContentPolicyFailure::ContradictoryNativeSettlement => {
            BlockerFailure::ContradictoryNativeSettlement
        }
    }
}

fn blocker_coverage_view(coverage: ContentRuleCoverage) -> Option<BlockerRuleCoverage> {
    Some(BlockerRuleCoverage {
        source_rules: u32::try_from(coverage.source_rules).ok()?,
        accepted_rules: u32::try_from(coverage.accepted_rules).ok()?,
        rejected_rules: u32::try_from(coverage.rejected_rules).ok()?,
        platform_omitted_rules: u32::try_from(coverage.platform_omitted_rules).ok()?,
        platform_approximated_rules: u32::try_from(coverage.platform_approximated_rules).ok()?,
        platform_resource_approximated_rules: u32::try_from(
            coverage.platform_resource_approximated_rules,
        )
        .ok()?,
        platform_source_kind_approximated_rules: u32::try_from(
            coverage.platform_source_kind_approximated_rules,
        )
        .ok()?,
        platform_attribution_approximated_rules: u32::try_from(
            coverage.platform_attribution_approximated_rules,
        )
        .ok()?,
        blocking_rule_entries: u32::try_from(coverage.blocking_rule_entries).ok()?,
    })
}

pub(super) fn settled_runtime_policy_observer(
    applied: Option<RuntimePolicyObserver>,
    candidate: Option<RuntimePolicyObserver>,
) -> Option<RuntimePolicyObserver> {
    match (applied, candidate) {
        (Some(applied), Some(candidate))
            if applied.digest == candidate.digest && applied.policy.strong_count() != 0 =>
        {
            // Digest-identical native promotion intentionally retains the
            // installed policy object so existing and future WebView2
            // callbacks continue sharing one diagnostics counter cohort.
            Some(applied)
        }
        (_, candidate) => candidate,
    }
}

pub(super) fn blocker_runtime_diagnostics_view(
    diagnostics: NetworkPolicyDiagnostics,
) -> BlockerRuntimeDiagnostics {
    BlockerRuntimeDiagnostics {
        total_decisions: diagnostics.total_decisions.to_string(),
        candidate_budget_exhausted: diagnostics.candidate_budget_exhausted.to_string(),
        matcher_unavailable: diagnostics.matcher_unavailable.to_string(),
        matcher_unprepared: diagnostics.matcher_unprepared.to_string(),
        attribution_unavailable: diagnostics.attribution_unavailable.to_string(),
        evaluation_errors: diagnostics.evaluation_errors.to_string(),
    }
}
