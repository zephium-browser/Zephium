//! Authoritative browser-shell state machine and effect coordination.

mod blocker;
mod blocker_sites;
mod blocker_statistics;
mod bookmarks;
mod bootstrap;
mod effects;
mod engine_events;
mod extension_actions;
mod extension_browser_requests;
mod extension_browser_surface;
mod extension_store;
mod favicon_probe;
mod favicons;
mod fullscreen;
mod history;
mod operations;
mod page_permissions;
mod page_requests;
mod persistence;
mod presentation;
mod private;
mod profile_deletion;
mod projections;
mod scope;
mod search;
mod tabs;
mod time;
mod user_content_status;
mod view_lifecycle;
mod webext;
pub use webext::{WebExtensionStatus, WebExtensionTarget};
mod window_layout;
mod work_authoring;
mod zoom;

use effects::{mutation_result, operation_result, NativeWork};
use extension_actions::ExtensionActionState;
use extension_browser_surface::ExtensionBrowserSurfaceState;
use favicons::{origin_of, FaviconState};
#[cfg(test)]
use favicons::{FAVICON_POLL_DELAYS, ICON_CACHE_CAPACITY};
use page_permissions::PagePermissionPromptState;
#[cfg(test)]
use presentation::PendingPresentation;
use presentation::PresentationState;
#[cfg(test)]
use presentation::MAX_PRESENTATION_ADMISSION_REJECTIONS;
use profile_deletion::{
    ProfileDeletionCoordinator, ProfileDeletionPhase, ProfileDeletionState,
    PROFILE_DELETION_STORE_TIMEOUT,
};
use search::SearchState;
use view_lifecycle::{CrashState, PendingDiscardProbe, ResidencyState, LIVE_VIEW_ABSOLUTE_LIMIT};
#[cfg(test)]
use view_lifecycle::{LIVE_VIEW_PRESSURE_LIMIT, MAX_CONCURRENT_DISCARD_PROBES};
use window_layout::GrabbedDivider;
use zoom::ZoomState;

use persistence::PersistenceState;
#[cfg(test)]
use persistence::{PERSIST_DEBOUNCE, PERSIST_MAX_AGE, URL_CHECKPOINT_INTERVAL};

#[cfg(test)]
use crate::actor::{spawn, Handle, TryPushError};
use crate::actor::{CallbackHandle, CommandQueue};
#[cfg(feature = "agentic-browser")]
use crate::api::AgentLifecycle;
use crate::api::PagePermissionPromptDecision;
use crate::api::TabAction;
use crate::api::{
    ChromePresentation, ChromePresentationDispatch, Command, ContentPolicyStatusQueryOutcome,
    EmitFn, SharedBlocker, SharedChrome, SharedEngine, SharedStore, ShellTerminalFailure,
    ShellTerminalFailureCallback, ShutdownOutcome,
};
#[cfg(test)]
use crate::api::{ChromePresentationCallback, PresentationChrome};
#[cfg(test)]
use crate::store_reads::FAVICON_CACHE_MAX_AGE_SECONDS;
use crate::store_reads::{StoreReadQueue, StoreReadResult};

#[cfg(test)]
use std::collections::VecDeque;
use std::sync::mpsc::{sync_channel, SyncSender};
use std::sync::{Arc, Mutex};

#[cfg(feature = "agentic-browser")]
use zephium_agentic::AgentBrowserShutdownOutcome;
#[cfg(test)]
use zephium_core::extensions::ExtensionBrowserRequestId;
use zephium_core::extensions::{
    ExtensionBrowserRequest, ExtensionBrowserRequestAction, ExtensionBrowserRequestRejection,
    ExtensionBrowserRequestResult, ExtensionBrowserRequestSettlement, ExtensionBrowserSurface,
    ExtensionBrowserSurfaceGeneration, ExtensionBrowserTab, ExtensionBrowserWindow,
};
use zephium_core::geometry::{Rect, Size};
use zephium_core::ids::{ItemId, ProfileId, SpaceId, WindowId};
use zephium_core::item::{ItemKind, Lifecycle, Placement, SpaceSection, TabState};
use zephium_core::items::{Effect, Items};
use zephium_core::layout;
use zephium_core::ports::blocker::BlockerShutdownOutcome;
#[cfg(test)]
use zephium_core::ports::chrome::Chrome as GeometryChrome;
use zephium_core::ports::chrome::ChromeFrame;
#[cfg(test)]
use zephium_core::ports::engine::Engine;
use zephium_core::ports::engine::{
    ContentScope, DiscardProbeId, EngineEvent, NativeAction, NativeDispatch,
    NavigationPresentationId, Partition, ProfileDataErasureOutcome, StageMotion, ZoomRequestId,
};
#[cfg(test)]
use zephium_core::ports::store::Store;
use zephium_core::ports::store::{
    PendingProfileDeletion, ProfileDeletionAuthorizeOutcome, ProfileDeletionFinalizeOutcome,
    ProfileDeletionLoad, SessionLoad, StoreShutdownOutcome, MAX_FAVICON_BATCH_ORIGINS,
};
use zephium_core::profiles::{Profile, ProfileKind, Profiles};
use zephium_core::session;
use zephium_core::spaces::{Space, Spaces};
use zephium_core::split::{self, Axis, Edge, Pane};
use zephium_core::windows::{WindowKind, Windows};
use zephium_core::{commands, navigation};
use zephium_ipc::{
    BlockerFailure, BlockerPhase, BlockerPreferenceState, BlockerProtection, BlockerRuleCoverage,
    BlockerRuntimeDiagnostics, BlockerSourceFailure, BlockerSourceIdentities, BlockerSourcePhase,
    BlockerSourceProvenance, BlockerStatusView, DividerView, ExtensionActionFailedView,
    ExtensionActionFailure, ExtensionActionRuntimeView, ExtensionActionShortcutView,
    ExtensionActionsView, ItemsState, LayoutState, OperationDisposition, OperationOutcome,
    OperationReason, PagePermissionKindView, PagePermissionPromptEntryView,
    PagePermissionPromptView, ProfileKindView, ProfileView, Projection, RuntimeSecurityAdvisory,
    RuntimeSecurityAdvisoryKind, RuntimeSecurityUpdateTarget, RuntimeStatus, SearchAction,
    SearchResult, SearchResults, SidebarNodeKindView, SidebarNodeView, SidebarSectionView,
    SpaceView, SplitGroupView, TabView,
};

// More simultaneous native renderers are neither usable in the current tiled
// layout nor safe to reconstruct synchronously after a restore/process loss.
pub(crate) const MAX_VISIBLE_PANES: usize = 8;
pub(super) const MAX_OPERATION_ID_BYTES: usize = 64;
pub(super) const MAINTENANCE_INTERVAL: std::time::Duration = std::time::Duration::from_secs(60);
#[cfg(not(test))]
// FIFO wait, storage-reader quiescence, snapshot construction, durability,
// native teardown, and thread joins consume
// this one caller-owned deadline.
pub(super) const END_TO_END_SHUTDOWN_TIMEOUT: std::time::Duration =
    std::time::Duration::from_secs(8);
// A clean shutdown joins the shell, timer and storage-reader threads, which
// took longer than 50 ms on loaded CI runners and failed tests that expect a
// clean outcome. Tests that wait for the deadline to expire pay this once.
#[cfg(test)]
pub(super) const END_TO_END_SHUTDOWN_TIMEOUT: std::time::Duration =
    std::time::Duration::from_millis(500);

#[cfg(feature = "agentic-browser")]
enum AgentLifecycleOwner {
    Absent,
    Owned(AgentLifecycle),
    Consumed,
}

#[cfg(feature = "agentic-browser")]
impl AgentLifecycleOwner {
    fn new(lifecycle: Option<AgentLifecycle>) -> Self {
        lifecycle.map_or(Self::Absent, Self::Owned)
    }
}

mod browser_pages;
mod kept_sites;
mod work_pane;

/// Pages a run may queue for a seat in its page group at once.
#[cfg(feature = "work-execution")]
const MAX_QUEUED_PAGES: usize = 8;

#[cfg(feature = "work-execution")]
fn trace_refusal(
    cause: crate::work_resources::product::RetainedRefusal,
    lane: crate::work_resources::product::RetainedLaneFacts,
) {
    crate::work_trace::record(format_args!(
        "work: phase=page_lane event=refused cause={cause:?} live={} settled={} lost={} queued={} group_failed={} group_sealed={}",
        lane.live, lane.settled, lane.lost, lane.queued, lane.group_failed, lane.group_sealed
    ));
}

struct NativeOpener {
    source: ItemId,
    activate_when_presentable: bool,
}

pub struct Shell {
    #[cfg(feature = "work-execution")]
    work: Option<Box<crate::work::ApplicationWork>>,
    #[cfg(feature = "work-execution")]
    retained_work: Option<Box<crate::work_resources::product::ProductWork>>,
    #[cfg(feature = "work-execution")]
    retained_pages: Vec<crate::work_resources::product::ProductWork>,
    /// Pages waiting for a seat in their run's page group, in arrival order.
    #[cfg(feature = "work-execution")]
    queued_pages: std::collections::VecDeque<crate::work_resources::product::ProductWork>,
    #[cfg(feature = "work-execution")]
    retained_page_runtime: Option<crate::work_resources::product::RetainedWorkGroup>,
    /// Retained works the runtime gave up on before they could close; they
    /// keep polling and shut down with the shell, out of the live slot.
    #[cfg(feature = "work-execution")]
    retained_graveyard: Vec<crate::work_resources::product::ProductWork>,
    profiles: Profiles,
    spaces: Spaces,
    items: Items,
    recently_closed: Vec<zephium_core::session::PersistedClosedTab>,
    windows: Windows,
    pending_size: Size,
    favicons: FaviconState,
    history: history::HistoryState,
    bookmarks: bookmarks::BookmarkState,
    search: SearchState,
    tab_preferences: tabs::TabPreferences,
    presentation: PresentationState,
    zoom: ZoomState,
    divider: Option<GrabbedDivider>,
    private: Option<private::PrivateSession>,
    /// Sites allowed to open a kind of application link without asking,
    /// for this run only.
    external_apps_allowed: std::collections::HashSet<(ProfileId, String, String)>,
    residency: ResidencyState,
    last_visits: std::collections::HashMap<ItemId, (String, std::time::Instant)>,
    time: time::TimeState,
    window_visible: bool,
    window_focused: bool,
    // Cells because every relayout, which takes `&self`, re-checks that the
    // fullscreen page may still be fullscreen and drops it at once if not.
    content_fullscreen: std::cell::Cell<Option<fullscreen::ContentFullscreen>>,
    host_fullscreen: std::cell::Cell<bool>,
    browser_page: Option<(WindowId, crate::BrowserPage)>,
    browser_page_projected: Option<(WindowId, Option<crate::BrowserPage>)>,
    browser_return_revision: u64,
    browser_after_return: Option<Box<Command>>,
    browser_return_ready: bool,
    browser_return: Option<browser_pages::PendingBrowserReturn>,
    work_pane: Option<work_pane::WorkPane>,
    work_pane_generation: u32,
    runtime_restart_required: bool,
    /// The saved session could not be restored this launch and was set aside.
    session_set_aside: bool,
    user_content_status: user_content_status::UserContentStatus,
    crash: CrashState,
    bootstrapped: bool,
    pending_external: Vec<String>,
    /// The page a find is running in, so its results and its end go there.
    find_target: Option<ItemId>,
    native_openers: std::collections::HashMap<ItemId, NativeOpener>,
    #[cfg(debug_assertions)]
    bootstrap_started: Option<std::time::Instant>,
    persistence: PersistenceState,
    shutdown_result: Option<ShutdownOutcome>,
    self_queue: Option<CommandQueue>,
    profile_deletion: ProfileDeletionCoordinator,
    /// Exact startup cohort whose per-profile history/favicon database was
    /// preserved but disabled by storage validation. Session/meta state and
    /// native website data remain independently usable.
    degraded_storage_profiles: std::collections::HashSet<ProfileId>,
    blocker: blocker::BlockerCoordinator,
    blocker_statistics: std::collections::HashMap<ProfileId, blocker_statistics::Statistics>,
    #[cfg(feature = "agentic-browser")]
    agent_lifecycle: AgentLifecycleOwner,
    extension_browser_surfaces: ExtensionBrowserSurfaceState,
    web_extensions: webext::WebExtensionState,
    extension_actions: ExtensionActionState,
    page_permissions: PagePermissionPromptState,
    terminal_failure: Option<ShellTerminalFailureCallback>,
    terminal_failure_handoff_panicked: bool,
    engine: SharedEngine,
    store: SharedStore,
    store_reads: Option<StoreReadQueue>,
    notes: Option<zephium_core::ports::notes::SharedNotes>,
    chrome: SharedChrome,
    emit: EmitFn,
    #[cfg(test)]
    auto_settle_content_rules: bool,
}

pub(super) struct ShellPorts {
    engine: SharedEngine,
    store: SharedStore,
    blocker: SharedBlocker,
    #[cfg(feature = "agentic-browser")]
    agent_lifecycle: Option<AgentLifecycle>,
    terminal_failure: ShellTerminalFailureCallback,
    chrome: SharedChrome,
    emit: EmitFn,
}

impl ShellPorts {
    pub(super) fn new(
        engine: SharedEngine,
        store: SharedStore,
        blocker: SharedBlocker,
        terminal_failure: ShellTerminalFailureCallback,
        chrome: SharedChrome,
        emit: EmitFn,
    ) -> Self {
        Self {
            engine,
            store,
            blocker,
            #[cfg(feature = "agentic-browser")]
            agent_lifecycle: None,
            terminal_failure,
            chrome,
            emit,
        }
    }

    #[cfg(feature = "agentic-browser")]
    pub(super) fn with_agent_lifecycle(mut self, lifecycle: Option<AgentLifecycle>) -> Self {
        self.agent_lifecycle = lifecycle;
        self
    }
}

impl Shell {
    #[cfg(test)]
    pub fn new(
        engine: SharedEngine,
        store: SharedStore,
        chrome: SharedChrome,
        emit: EmitFn,
    ) -> Self {
        Self::with_store_reads(
            ShellPorts::new(
                engine,
                store,
                Arc::new(tests::ImmediateAllowAllCompiler),
                Box::new(|_| {}),
                chrome,
                emit,
            ),
            None,
            true,
        )
    }

    #[cfg(test)]
    pub(super) fn new_with_blocker(
        engine: SharedEngine,
        store: SharedStore,
        blocker: SharedBlocker,
        chrome: SharedChrome,
        emit: EmitFn,
    ) -> Self {
        Self::with_store_reads(
            ShellPorts::new(engine, store, blocker, Box::new(|_| {}), chrome, emit),
            None,
            false,
        )
    }

    #[cfg(test)]
    pub(super) fn new_with_failure(
        engine: SharedEngine,
        store: SharedStore,
        blocker: SharedBlocker,
        terminal_failure: ShellTerminalFailureCallback,
        chrome: SharedChrome,
        emit: EmitFn,
    ) -> Self {
        Self::with_store_reads(
            ShellPorts::new(engine, store, blocker, terminal_failure, chrome, emit),
            None,
            true,
        )
    }

    #[cfg(all(test, feature = "agentic-browser"))]
    pub(super) fn new_with_agent_lifecycle(
        engine: SharedEngine,
        store: SharedStore,
        blocker: SharedBlocker,
        agent_lifecycle: AgentLifecycle,
        chrome: SharedChrome,
        emit: EmitFn,
    ) -> Self {
        Self::with_store_reads(
            ShellPorts::new(engine, store, blocker, Box::new(|_| {}), chrome, emit)
                .with_agent_lifecycle(Some(agent_lifecycle)),
            None,
            true,
        )
    }

    #[cfg(test)]
    pub(super) fn with_store_reads(
        ports: ShellPorts,
        store_reads: impl Into<Option<StoreReadQueue>>,
        #[cfg(test)] auto_settle_content_rules: bool,
    ) -> Self {
        let mut shell = Self::with_store_reads_deferred_blocker_catalog(
            ports,
            store_reads,
            #[cfg(test)]
            auto_settle_content_rules,
        );
        shell.initialize_blocker_catalog();
        shell
    }

    /// Builds only actor-owned state and does not enter any external port.
    /// The actor installs `ShellExitGuard` before completing catalog admission,
    /// so a panic cannot strand the Store/native/blocker cleanup graph outside
    /// an observable terminal path.
    pub(super) fn with_store_reads_deferred_blocker_catalog(
        ports: ShellPorts,
        store_reads: impl Into<Option<StoreReadQueue>>,
        #[cfg(test)] auto_settle_content_rules: bool,
    ) -> Self {
        let ShellPorts {
            engine,
            store,
            blocker,
            #[cfg(feature = "agentic-browser")]
            agent_lifecycle,
            terminal_failure,
            chrome,
            emit,
        } = ports;
        Self {
            profiles: Profiles::default(),
            spaces: Spaces::default(),
            items: Items::default(),
            recently_closed: Vec::new(),
            windows: Windows::default(),
            pending_size: Size::default(),
            favicons: FaviconState::default(),
            history: history::HistoryState::default(),
            bookmarks: bookmarks::BookmarkState::default(),
            time: time::TimeState::load(&store),
            search: SearchState {
                custom_url: store.app_setting("search.custom-url").unwrap_or_default(),
                engine: store
                    .app_setting("search.engine")
                    .as_deref()
                    .and_then(zephium_core::search::SearchEngine::from_id)
                    .unwrap_or_default(),
                include_history: store
                    .app_setting("search.history")
                    .is_none_or(|value| value != "false"),
                ..SearchState::default()
            },
            tab_preferences: tabs::TabPreferences::load(&*store),
            presentation: PresentationState::default(),
            zoom: ZoomState::default(),
            divider: None,
            private: None,
            external_apps_allowed: std::collections::HashSet::new(),
            residency: ResidencyState::load(&*store),
            last_visits: std::collections::HashMap::new(),
            window_visible: true,
            window_focused: false,
            content_fullscreen: std::cell::Cell::new(None),
            host_fullscreen: std::cell::Cell::new(false),
            browser_page: None,
            browser_page_projected: None,
            browser_return_revision: 0,
            browser_after_return: None,
            browser_return_ready: false,
            browser_return: None,
            work_pane: None,
            work_pane_generation: 0,
            runtime_restart_required: false,
            session_set_aside: false,
            user_content_status: user_content_status::UserContentStatus::default(),
            crash: CrashState::default(),
            bootstrapped: false,
            pending_external: Vec::new(),
            find_target: None,
            native_openers: std::collections::HashMap::new(),
            #[cfg(debug_assertions)]
            bootstrap_started: None,
            persistence: PersistenceState::default(),
            shutdown_result: None,
            self_queue: None,
            profile_deletion: ProfileDeletionCoordinator::default(),
            degraded_storage_profiles: std::collections::HashSet::new(),
            blocker: blocker::BlockerCoordinator::new_deferred(blocker),
            blocker_statistics: std::collections::HashMap::new(),
            #[cfg(feature = "agentic-browser")]
            agent_lifecycle: AgentLifecycleOwner::new(agent_lifecycle),
            #[cfg(feature = "work-execution")]
            work: None,
            #[cfg(feature = "work-execution")]
            retained_work: None,
            #[cfg(feature = "work-execution")]
            retained_pages: Vec::new(),
            #[cfg(feature = "work-execution")]
            queued_pages: std::collections::VecDeque::new(),
            #[cfg(feature = "work-execution")]
            retained_page_runtime: None,
            #[cfg(feature = "work-execution")]
            retained_graveyard: Vec::new(),
            extension_browser_surfaces: ExtensionBrowserSurfaceState::default(),
            web_extensions: webext::WebExtensionState::default(),
            extension_actions: ExtensionActionState::default(),
            page_permissions: PagePermissionPromptState::default(),
            terminal_failure: Some(terminal_failure),
            terminal_failure_handoff_panicked: false,
            engine,
            store,
            store_reads: store_reads.into(),
            notes: None,
            chrome,
            emit,
            #[cfg(test)]
            auto_settle_content_rules,
        }
    }

    pub(super) fn initialize_blocker_catalog(&mut self) {
        self.blocker.initialize_catalog();
    }

    pub(super) fn attach_queue(&mut self, queue: CommandQueue) {
        self.attach_queue_for_terminal_cleanup(queue);
        self.schedule_blocker_catalog_activation_poll();
    }

    /// Installs only the actor's self-queue for a startup cancellation. No
    /// timer or external coordinator work is admitted before the queued
    /// terminal barrier is consumed.
    pub(super) fn attach_queue_for_terminal_cleanup(&mut self, queue: CommandQueue) {
        self.self_queue = Some(queue);
    }

    pub(super) fn is_shutdown(&self) -> bool {
        self.shutdown_result.is_some()
    }

    pub(super) fn terminal_failure_handoff_panicked(&self) -> bool {
        self.terminal_failure_handoff_panicked
    }

    pub fn handle(&mut self, cmd: Command) {
        // No late engine/UI/timer work may mutate state after the final
        // snapshot. Repeated shutdown requests receive the original result.
        if let Some(outcome) = self.shutdown_result {
            if let Command::Shutdown { ack, .. } = cmd {
                let _ = ack.send(outcome);
            }
            return;
        }
        match cmd {
            Command::WorkDocument(submission) => self.work_document(submission),
            Command::WorkEnvironmentChanged(change) => {
                if self.windows.focused().is_some_and(|window| {
                    window.profile.to_string() == change.profile
                        && !self.profile_deletion_quarantines(window.profile)
                }) {
                    (self.emit)(Projection::WorkEnvironmentChanged(change));
                }
            }
            Command::WorkChanged(change) => {
                if self.windows.focused().is_some_and(|window| {
                    window.profile.to_string() == change.profile
                        && !self.profile_deletion_quarantines(window.profile)
                }) {
                    (self.emit)(Projection::WorkChanged(change));
                }
            }
            #[cfg(feature = "work-execution")]
            Command::AttachRetainedWork(attachment) => {
                if let Some(work) = crate::work_resources::product::ProductWork::take(&attachment) {
                    self.retire_settled_pages();
                    if self
                        .retained_work
                        .as_ref()
                        .is_some_and(|work| work.is_stuck())
                    {
                        if let Some(mut stuck) = self.retained_work.take() {
                            stuck.begin_shutdown();
                            self.retained_graveyard.push(*stuck);
                        }
                    }
                    self.attach_retained(work);
                }
            }
            #[cfg(feature = "work-execution")]
            Command::AttachWork(attachment) => {
                if let Some(mut work) = crate::work::ApplicationWork::take_attachment(&attachment) {
                    if !work.belongs_to_store(&self.store)
                        || self.retained_work.is_some()
                        || self.retained_page_runtime.is_some()
                        || !work.belongs_to_engine(&self.engine)
                        || !work.accepts_predecessor(self.work.as_deref())
                        || !matches!(self.agent_lifecycle, AgentLifecycleOwner::Absent)
                    {
                        work.refuse_attachment();
                    } else {
                        if let Some(previous) = &self.work {
                            previous.retire_projection();
                        }
                        work.initialize();
                        self.work = Some(Box::new(work));
                    }
                }
            }
            #[cfg(feature = "work-execution")]
            Command::AdmitWork(submission) => {
                let profile = self.work_profile_binding();
                if let Some(work) = &mut self.work {
                    work.admit(submission, Some(profile));
                }
            }
            #[cfg(feature = "work-execution")]
            Command::WorkControl(control) => {
                if let Some(work) = &mut self.work {
                    work.control(*control);
                }
            }
            #[cfg(feature = "work-execution")]
            Command::WorkWake => {}
            Command::Operation {
                operation_id,
                command,
            } => {
                // Public construction rejects nested operations and shutdown
                // barriers. Keep the actor defensive if a future in-process
                // caller bypasses that constructor.
                if matches!(
                    command.as_ref(),
                    Command::Operation { .. } | Command::Shutdown { .. }
                ) {
                    return;
                }
                let command = *command;
                if let Command::RespondToPagePermissionPrompt {
                    profile,
                    item,
                    request,
                    decision,
                } = &command
                {
                    if let Some(mut completion) = self.begin_page_permission_response(
                        operation_id.clone(),
                        *profile,
                        *item,
                        *request,
                        *decision,
                    ) {
                        completion.operation_id = operation_id;
                        (self.emit)(Projection::OperationProcessed(completion));
                    }
                    return;
                }
                if let Command::DeleteProfile(profile) = &command {
                    let profile = *profile;
                    let mut completion =
                        self.begin_profile_deletion(profile, Some(operation_id.clone()));
                    completion.operation_id = operation_id;
                    let deferred = completion.outcome == OperationOutcome::Deferred;
                    if deferred {
                        // Admission already told the caller this operation is
                        // owned by the FIFO. Retain its id and emit no
                        // misleading terminal disposition while either durable phase
                        // is pending or retrying.
                        self.drive_profile_deletion(profile);
                    } else {
                        (self.emit)(Projection::OperationProcessed(completion));
                    }
                    return;
                }
                if let Command::SetFocusedContentBlockerEnabled(enabled) = &command {
                    if let Some(mut completion) =
                        self.begin_focused_blocker_mutation(operation_id.clone(), *enabled)
                    {
                        completion.operation_id = operation_id;
                        (self.emit)(Projection::OperationProcessed(completion));
                    }
                    return;
                }
                if let Command::ChangeBlockerSite { context, action } = &command {
                    if let Some(mut completion) =
                        self.begin_blocker_site_mutation(operation_id.clone(), context, action)
                    {
                        completion.operation_id = operation_id;
                        (self.emit)(Projection::OperationProcessed(completion));
                    }
                    return;
                }
                if matches!(&command, Command::RefreshContentBlockerSources) {
                    if let Some(mut completion) =
                        self.begin_blocker_catalog_refresh(operation_id.clone())
                    {
                        completion.operation_id = operation_id;
                        (self.emit)(Projection::OperationProcessed(completion));
                    }
                    return;
                }
                let mut completion = self.handle_operation(command);
                completion.operation_id = operation_id;
                (self.emit)(Projection::OperationProcessed(completion));
            }
            Command::Bootstrap => self.bootstrap(),
            Command::SetWebExtensions {
                profile,
                extensions,
            } => self.set_web_extensions(profile, extensions),
            Command::OpenWebExtensionOptions {
                profile,
                extension_id,
            } => {
                let _ = self
                    .engine
                    .open_web_extension_options(profile, extension_id);
            }
            Command::AnswerWebExtensionAccess {
                profile,
                request,
                allowed,
            } => {
                let _ = self
                    .engine
                    .answer_web_extension_access(profile, request, allowed);
            }
            Command::RemoveWebExtension { profile, extension } => {
                self.remove_web_extension(profile, *extension)
            }
            Command::ResolveWebExtensionTarget { tab, reply } => {
                let _ = reply.try_send(self.web_extension_target(tab));
            }
            Command::WebExtensionStatus { profile, reply } => {
                let _ = reply.try_send(self.web_extension_status(profile));
            }
            Command::Open => {
                let _ = self.operation_open();
            }
            Command::Activate(id) => {
                let _ = self.operation_activate(id);
            }
            Command::SetTabEssential {
                id,
                essential,
                before,
            } => {
                let _ = self.operation_set_tab_essential(id, essential, before);
            }
            Command::KeepSite(id) => {
                let _ = self.operation_keep_site(&id);
            }
            Command::RenameFocusedProfile(name) => {
                let _ = self.operation_rename_focused_profile(&name);
            }
            Command::Close(id) => {
                let _ = self.operation_close(id);
            }
            Command::Navigate { id, input } => {
                let _ = self.operation_navigate(id, input);
            }
            Command::Reload(id) => {
                let _ = self.operation_reload(id);
            }
            Command::AnswerPageRequest { id, decision } => {
                let _ = self.operation_answer_page_request(id, decision);
            }
            Command::GoBack(id) => {
                let _ = self.operation_history(id, false);
            }
            Command::GoForward(id) => {
                let _ = self.operation_history(id, true);
            }
            Command::SplitWith { other, axis } => {
                let _ = self.operation_split(other, axis);
            }
            Command::Unsplit => {
                let _ = self.operation_unsplit();
            }
            Command::LeaveSplit(id) => {
                let _ = self.operation_leave_split(id);
            }
            Command::TabAction { id, action } => {
                let _ = self.operation_tab_action(id, action);
            }
            Command::SetWindowSize(size) => match self.windows.focused_mut() {
                Some(win) => {
                    win.size = size;
                    // macOS resizes natively via autoresizing masks; Windows
                    // and Linux have no equivalent, the shell must relayout.
                    let _ = self.relayout();
                }
                None => self.pending_size = size,
            },
            Command::SetWindowFocused(focused) => {
                self.window_focused = focused;
                if !focused {
                    self.cancel_page_permission_if_not_foreground();
                }
            }
            Command::StopMediaCapture { item, navigation } => {
                let _ = self.operation_stop_media_capture(item, navigation);
            }
            Command::SetWindowVisible(visible) => {
                if self.window_visible != visible {
                    self.window_visible = visible;
                    // Do not immediately suspend a page that was actively in
                    // use when the window minimized. The ordinary idle grace
                    // still applies after every content view becomes hidden.
                    if !visible {
                        // OS pointer capture cannot remain authoritative while
                        // its window is hidden/minimized.
                        self.drop_divider();
                        if let Some(active) = self.windows.focused().and_then(|w| w.active) {
                            self.touch(active);
                        }
                        self.cancel_page_permission_if_not_foreground();
                    }
                    let _ = self.relayout();
                    self.maintain_views();
                }
            }
            Command::BrowserChromeRestored { revision, applied } => {
                self.browser_chrome_restored(revision, applied)
            }
            Command::ShowBrowserPage(page) => {
                let _ = self.operation_show_browser_page(page);
            }
            Command::SetSidebarWidth(width, animate) => {
                if let Some(win) = self.windows.focused_mut() {
                    win.metrics.sidebar_width = zephium_core::layout::clamp_sidebar_width(width);
                }
                if animate {
                    if let Some(win) = self.windows.focused() {
                        let _ = self.engine.hint_stage_motion(win.id, StageMotion::Slide);
                    }
                }
                let _ = self.relayout_with(animate);
            }
            Command::SidebarResizeGuide(width) => {
                if let Some(win) = self.windows.focused() {
                    let zone = width.filter(|width| width.is_finite()).map(|width| {
                        let padding = win.metrics.padding;
                        let x = (padding + zephium_core::layout::clamp_sidebar_width(width) - 2.0)
                            .min((win.size.width - padding - 2.0).max(padding));
                        Rect::new(x, padding, 2.0, (win.size.height - 2.0 * padding).max(0.0))
                    });
                    let _ = self.engine.set_resize_guide(win.id, zone);
                }
            }
            Command::DownloadCall {
                expected_profile,
                call,
                done,
            } => {
                use zephium_core::downloads::{DownloadError, DownloadResponse};
                let authorized = self
                    .windows
                    .focused()
                    .is_some_and(|window| window.profile == expected_profile)
                    && !self.profile_deletion_quarantines(expected_profile)
                    && call.validate();
                if !authorized {
                    done.finish(DownloadResponse::Error {
                        error: DownloadError::Invalid,
                    });
                } else {
                    let partition = match self
                        .profiles
                        .get(expected_profile)
                        .map(|profile| profile.kind)
                    {
                        Some(zephium_core::profiles::ProfileKind::Incognito) => {
                            zephium_core::ports::engine::Partition::Ephemeral(expected_profile)
                        }
                        Some(_) => {
                            zephium_core::ports::engine::Partition::Persistent(expected_profile)
                        }
                        None => {
                            done.finish(DownloadResponse::Error {
                                error: DownloadError::Invalid,
                            });
                            return;
                        }
                    };
                    self.engine.download_call(partition, *call, done);
                }
            }
            Command::HistoryCall {
                expected_profile,
                call,
                done,
            } => self.history_call(expected_profile, *call, done),
            Command::BookmarkCall {
                expected_profile,
                call,
                done,
            } => self.bookmark_call(expected_profile, *call, done),
            Command::Import { work, done } => self.import_into_focused(*work, done),
            Command::AttachFaviconProber(attachment) => self.attach_favicon_prober(attachment.0),
            Command::ProbeFavicons { profile, origins } => self.probe_favicons(profile, origins),
            Command::FaviconProbed {
                profile,
                origin,
                rgba,
            } => self.favicon_probed(profile, origin, rgba),
            Command::AttachNotes(attachment) => {
                self.notes.get_or_insert(attachment.0);
            }
            Command::NoteCall {
                expected_profile,
                call,
                done,
            } => {
                use zephium_core::notes::{NoteError, NoteReply, NoteResponse};
                // Private profiles keep nothing on disk, notes included.
                let profile = self
                    .windows
                    .focused()
                    .map(|window| window.profile)
                    .filter(|profile| *profile == expected_profile)
                    .filter(|profile| {
                        self.profiles.get(*profile).is_some_and(|p| {
                            p.kind != zephium_core::profiles::ProfileKind::Incognito
                        })
                    });
                match (profile, &self.notes) {
                    (Some(profile), Some(notes)) => notes.call(
                        profile,
                        Arc::unwrap_or_clone(call),
                        Box::new(move |response| {
                            done.finish(NoteReply {
                                profile: Some(profile.to_string()),
                                response,
                            })
                        }),
                    ),
                    _ => done.finish(NoteReply {
                        profile: None,
                        response: NoteResponse::Error {
                            error: NoteError::Unavailable,
                        },
                    }),
                }
            }
            Command::WorkPaneSetRect { rect, generation } => {
                self.work_pane_set_rect(rect, generation);
            }
            Command::WorkPaneShow { target, rect } => {
                let _ = self.operation_work_pane_show(target, rect);
            }
            Command::WorkPaneHide => {
                let _ = self.operation_work_pane_hide();
            }
            Command::ResourceCall {
                expected_profile,
                call,
                done,
            } => {
                use zephium_core::resources::{ResourceError, ResourceReply, ResourceResponse};
                if let Some(profile) = self
                    .windows
                    .focused()
                    .map(|window| window.profile)
                    .filter(|profile| *profile == expected_profile)
                    .filter(|profile| !self.incognito_profile(*profile))
                {
                    self.store.resource_call(
                        profile,
                        Arc::unwrap_or_clone(call),
                        Box::new(move |response| {
                            done.finish(ResourceReply {
                                profile: Some(profile.to_string()),
                                response,
                            })
                        }),
                    );
                } else {
                    done.finish(ResourceReply {
                        profile: None,
                        response: ResourceResponse::Error {
                            error: ResourceError::Unavailable,
                        },
                    });
                }
            }
            Command::ImportMedia {
                expected_profile,
                import,
                done,
            } => {
                use zephium_core::resources::{ResourceError, ResourceReply, ResourceResponse};
                if let Some(profile) = self
                    .windows
                    .focused()
                    .map(|window| window.profile)
                    .filter(|profile| *profile == expected_profile)
                    .filter(|profile| !self.incognito_profile(*profile))
                {
                    self.store.import_media(
                        profile,
                        *import,
                        Box::new(move |response| {
                            done.finish(ResourceReply {
                                profile: Some(profile.to_string()),
                                response,
                            })
                        }),
                    );
                } else {
                    done.finish(ResourceReply {
                        profile: None,
                        response: ResourceResponse::Error {
                            error: ResourceError::Unavailable,
                        },
                    });
                }
            }
            Command::DragOver { point } => {
                if let Some(win) = self.windows.focused().map(|w| w.id) {
                    let zone = point
                        .and_then(|(x, y)| self.resolve_drop(x, y))
                        .map(|d| d.zone);
                    let _ = self.engine.set_drop_indicator(win, zone);
                }
            }
            Command::DropTab { id, x, y } => {
                let _ = self.operation_drop_tab(id, x, y);
            }
            Command::DividerGrab { x, y } => {
                self.drop_divider();
                self.divider = self.locate_divider(x, y);
            }
            Command::DividerDrag { x, y } => self.divider_drag(x, y),
            Command::DividerRelease { x, y } => {
                let _ = self.operation_divider_release(x.zip(y));
            }
            Command::Run(id) => {
                let _ = self.operation_run_command(&id);
            }
            // This privileged mutation must carry a desktop operation id.
            Command::InvokeExtensionAction { .. } => {}
            Command::RespondToPagePermissionPrompt { .. } => {}
            Command::PagePermissionCatalogLoaded {
                profile,
                item,
                request,
                outcome,
            } => self.settle_page_permission_catalog_load(profile, item, request, *outcome),
            Command::PagePermissionCatalogMutated {
                profile,
                item,
                request,
                outcome,
            } => self.settle_page_permission_catalog_mutation(profile, item, request, *outcome),
            Command::PagePermissionTimeout {
                profile,
                item,
                request,
            } => self.on_page_permission_timeout(profile, item, request),
            Command::Search(query) => {
                self.search.context = None;
                self.search(&query);
            }
            Command::SearchSupplementaryFinished { context, query } => {
                self.search_supplementary_finished(*context, query)
            }
            Command::SearchAdditional {
                context,
                query,
                results,
            } => self.search_additional(*context, query, results),
            Command::SearchScoped { query, context } => self.search_scoped(&query, *context),
            Command::CancelSearch { session_id } => self.cancel_scoped_search(&session_id),
            Command::RunSearchAction {
                context,
                action,
                background,
            } => {
                let _ = self.operation_run_search_action(*context, action, background);
            }
            Command::OpenUrl { input, new_tab } => {
                let _ = self.operation_open_url(input, new_tab);
            }
            Command::OpenExternal(urls) => self.open_external(urls),
            Command::Find(request) => self.find_in_page(request),
            Command::SetAppSetting { key, value } => {
                let _ = self.operation_set_app_setting(key, value);
            }
            // These mutations are accepted only through `Command::Operation`
            // so every foreground request has one truthful terminal identity.
            Command::DeleteProfile(_)
            | Command::RetryContentPolicy { .. }
            | Command::SetFocusedContentBlockerEnabled(_)
            | Command::ChangeBlockerSite { .. }
            | Command::RetryFocusedContentPolicy { .. }
            | Command::RefreshContentBlockerSources => {}
            Command::ContentPolicyStatus { profile, reply } => {
                let outcome = self
                    .blocker
                    .status(profile)
                    .map(ContentPolicyStatusQueryOutcome::Found)
                    .unwrap_or(ContentPolicyStatusQueryOutcome::UnknownProfile);
                let _ = reply.send(outcome);
            }
            Command::ElementPicker {
                context,
                action,
                reply,
            } => self.element_picker(&context, action, reply),
            Command::BlockerStatistics { profile, reply } => {
                self.query_blocker_statistics(profile, reply)
            }
            Command::FocusedContentPolicyStatus { reply } => {
                self.maintain_blocker_catalog();
                let _ = reply.send(self.focused_blocker_status_view());
            }
            #[cfg(feature = "work-execution")]
            Command::WorkProfileBinding { reply } => {
                let _ = reply.send(self.work_profile_binding());
            }
            Command::TabMetadata {
                profile,
                ids,
                reply,
            } => {
                let _ = reply.send(self.tab_metadata(profile, &ids));
            }
            Command::WindowTabs { profile, reply } => {
                let _ = reply.send(self.window_tabs(profile));
            }
            Command::FaviconPoll { id, attempt } => self.poll_favicon(id, attempt),
            Command::PresentationFallback {
                id,
                navigation,
                hard_deadline,
            } => self.on_presentation_fallback(id, navigation, hard_deadline),
            Command::ChromePresentationApplied {
                id,
                navigation,
                url,
                active,
                projection_revision,
                applied,
            } => self.on_chrome_presentation_applied(
                id,
                navigation,
                url,
                active,
                projection_revision,
                applied,
            ),
            Command::DiscardProbeTimeout { id, probe } => self.on_discard_probe_timeout(id, probe),
            Command::ViewCapacityRetry(id) => self.on_view_capacity_retry(id),
            Command::ProfileDeletionReady(profile) => {
                self.consume_profile_deletion_outcome(profile)
            }
            Command::BlockerReady(profile) => self.consume_blocker_compile_result(profile),
            Command::BlockerStoreReady(profile) => {
                self.consume_blocker_store_result(profile);
                self.consume_blocker_site_result(profile);
            }
            Command::BlockerPreferenceRetry { profile, token } => {
                self.on_blocker_preference_reconciliation_retry(profile, token)
            }
            Command::BlockerCatalogPoll { operation, attempt } => {
                self.on_blocker_catalog_poll(operation, attempt)
            }
            Command::ProfileDeletionRetry {
                profile,
                generation,
            } => {
                if self
                    .profile_deletion
                    .states
                    .get(&profile)
                    .is_some_and(|state| state.retry_generation == generation)
                {
                    self.drive_profile_deletion(profile);
                }
            }
            Command::StoreRead(result) => self.on_store_read(result),
            Command::Persist => self.persist(),
            Command::Tick => {
                if !self.bootstrapped {
                    self.bootstrap();
                    if !self.bootstrapped {
                        return;
                    }
                }
                self.maintain_blocker_statistics();
                self.flush_time(None);
                self.maintain_blocker_catalog();
                self.drain_blocker_inbox();
                self.drive_blocker_preference_reconciliations();
                self.drain_profile_deletion_inbox();
                self.reconcile_runtime_restart_requirement();
                let extension_surfaces = self.retry_extension_browser_surfaces();
                if extension_surfaces.native.rejected {
                    crate::diagnostic!(
                        "extensions: maintenance could not reconcile browser metadata"
                    );
                }
                let extension_actions = self.maintain_extension_actions();
                if extension_actions.rejected {
                    crate::diagnostic!("extensions: maintenance could not refresh toolbar actions");
                }
                if self.maintain_views() {
                    self.project_items();
                }
            }
            Command::Engine(event) => self.on_engine_event(event),
            Command::Shutdown { deadline, ack } => self.shutdown_until(deadline, ack),
            Command::TimeCall {
                expected_profile,
                call,
                done,
            } => self.time_call(expected_profile, *call, done),
            Command::TimeReportRead {
                profile,
                report,
                done,
            } => self.on_time_report(profile, report, done),
            Command::SetAppActive(active) => self.set_app_active(active),
            Command::SetSystemAwake(awake) => self.set_system_awake(awake),
            Command::SetMemoryPressure(pressure) => self.on_memory_pressure(pressure),
            Command::FocusWake => self.focus_wake(),
            // Accepted only through `Command::Operation`.
            Command::Focus(_) => {}
        }
        if self.shutdown_result.is_none() {
            self.refresh_time();
            self.refresh_focus_cover();
        }
        #[cfg(feature = "work-execution")]
        self.poll_work();
    }

    /// Admits a retained work now, or, for a page that waits only for a
    /// seat in the run's page group, keeps it queued in order until one frees.
    /// A settled page whose native audit ended holds no browser any more: it
    /// keeps its recorded debt, but neither a seat nor its run's group, so a
    /// lost page never holds the pages after it, in its run or another.
    #[cfg(feature = "work-execution")]
    fn attach_retained(&mut self, mut work: crate::work_resources::product::ProductWork) {
        use crate::work_resources::product::RetainedRefusal as Refusal;
        let lane = self.page_lane();
        let refusal = if self.work.is_some()
            || !matches!(self.agent_lifecycle, AgentLifecycleOwner::Absent)
        {
            Some(Refusal::Busy)
        } else if work.given_up() {
            Some(Refusal::GivenUp)
        } else if !work.admits(&self.engine, &self.store, self.work_profile_binding()) {
            Some(Refusal::Stale)
        } else {
            None
        };
        if let Some(refusal) = refusal {
            trace_refusal(refusal, lane);
            work.refuse(refusal, lane);
            return;
        }
        if !work.is_page() {
            if self
                .retained_work
                .as_ref()
                .is_some_and(|work| !work.is_closed())
                || self.retained_page_runtime.is_some()
            {
                trace_refusal(Refusal::Busy, lane);
                work.refuse(Refusal::Busy, lane);
            } else {
                // Install original ownership before native construction.
                self.retained_work = Some(Box::new(work));
                self.retained_work.as_mut().unwrap().initialize();
            }
            return;
        }
        let seated = self
            .retained_work
            .as_ref()
            .is_none_or(|work| work.is_closed())
            && work.admits_peers(
                &self.retained_pages,
                self.retained_graveyard
                    .iter()
                    .filter(|work| work.holds_seat()),
            )
            && !self
                .retained_page_runtime
                .as_ref()
                .is_some_and(|group| group.is_failed() || group.is_sealed());
        // A page without a seat waits for one, in order, within its own
        // bounded wait: a full or sealed group of its run, or another run's
        // pages still closing, give way once their members have closed.
        if !seated {
            if self.queued_pages.len() < MAX_QUEUED_PAGES {
                if work.begin_wait() {
                    crate::work_trace::record(format_args!(
                        "work: phase=page_lane event=waiting live={} settled={} lost={} queued={} group_failed={} group_sealed={}",
                        lane.live,
                        lane.settled,
                        lane.lost,
                        lane.queued,
                        lane.group_failed,
                        lane.group_sealed
                    ));
                }
                self.queued_pages.push_back(work);
            } else {
                trace_refusal(Refusal::QueueFull, lane);
                work.refuse(Refusal::QueueFull, lane);
            }
            return;
        }
        if self.retained_page_runtime.is_none() {
            self.retained_page_runtime = work.new_runtime_group().ok();
        }
        if let Some(waited) = work.waited() {
            crate::work_trace::record(format_args!(
                "work: phase=page_lane event=seated waited_ms={}",
                waited.as_millis()
            ));
        }
        if let Some(group) = &self.retained_page_runtime {
            work.set_runtime_group(group.clone());
            self.retained_pages.push(work);
            self.retained_pages.last_mut().unwrap().initialize();
        } else {
            trace_refusal(Refusal::GroupStart, lane);
            work.refuse(Refusal::GroupStart, lane);
        }
    }

    /// The page lane as it stands, in closed counts.
    #[cfg(feature = "work-execution")]
    fn page_lane(&self) -> crate::work_resources::product::RetainedLaneFacts {
        let count = |n: usize| u8::try_from(n).unwrap_or(u8::MAX);
        crate::work_resources::product::RetainedLaneFacts {
            live: count(self.retained_pages.len()),
            settled: count(
                self.retained_graveyard
                    .iter()
                    .filter(|work| work.holds_seat())
                    .count(),
            ),
            lost: count(
                self.retained_graveyard
                    .iter()
                    .filter(|work| work.beyond_closing())
                    .count(),
            ),
            queued: count(self.queued_pages.len()),
            group_failed: self
                .retained_page_runtime
                .as_ref()
                .is_some_and(|group| group.is_failed()),
            group_sealed: self
                .retained_page_runtime
                .as_ref()
                .is_some_and(|group| group.is_sealed()),
        }
    }

    /// Queued pages take seats in order as they free; a page that can sit
    /// now does not wait behind one that cannot (another run's page waiting
    /// for this run's group to close). One whose run ended or ran out of
    /// time while it waited is refused.
    #[cfg(feature = "work-execution")]
    fn admit_queued_pages(&mut self) {
        for work in std::mem::take(&mut self.queued_pages) {
            self.attach_retained(work);
        }
    }

    /// A settled page that cannot close with its group moves to the graveyard
    /// and stops holding admission; the group no longer waits on it.
    #[cfg(feature = "work-execution")]
    fn retire_settled_pages(&mut self) {
        self.retained_pages.retain(|page| !page.is_closed());
        let mut index = 0;
        while index < self.retained_pages.len() {
            if self.retained_pages[index].leaves_group() {
                let mut page = self.retained_pages.remove(index);
                crate::work_trace::record(format_args!(
                    "work: phase=page_lane event=left_group lost={} live={}",
                    page.beyond_closing(),
                    self.retained_pages.len()
                ));
                page.leave_group();
                page.begin_shutdown();
                self.retained_graveyard.push(page);
            } else {
                index += 1;
            }
        }
        // The group, and the native group it stands for, ends with its last
        // member, live or graveyarded, not with its last live page: a
        // graveyarded page still owns a native seat and an audit, and a fresh
        // group cannot start natively until every member has closed.
        // A graveyarded member whose own native audit has ended holds no
        // seat any more: nothing further can close it, and the next group's
        // native admission still requires every member's proof.
        if self.retained_pages.is_empty()
            && !self.retained_graveyard.iter().any(|work| work.holds_seat())
            && self.retained_page_runtime.take().is_some()
        {
            crate::work_trace::record(format_args!(
                "work: phase=page_lane event=group_closed lost={}",
                self.retained_graveyard.len()
            ));
        }
    }

    /// One native audit at a time across live and graveyarded pages. The
    /// audit counts the whole native browser, so it starts only once no
    /// member holds a resource. Live pages take the turn first; the first turn
    /// seals the group against new pages.
    #[cfg(feature = "work-execution")]
    fn grant_native_audit(&mut self) {
        self.retire_settled_pages();
        if self
            .retained_pages
            .iter()
            .chain(&self.retained_graveyard)
            .any(|work| work.holds_native_audit())
            || !self
                .retained_pages
                .iter()
                .chain(
                    self.retained_graveyard
                        .iter()
                        .filter(|work| work.native_member()),
                )
                .all(|page| page.ready_for_group_shutdown())
        {
            return;
        }
        let granted = if let Some(page) = self
            .retained_pages
            .iter_mut()
            .find(|page| !page.is_closed())
        {
            page.allow_group_shutdown();
            true
        } else if let Some(work) = self
            .retained_graveyard
            .iter_mut()
            .find(|work| work.awaits_group_audit())
        {
            work.allow_group_shutdown();
            true
        } else {
            false
        };
        if granted {
            if let Some(group) = &self.retained_page_runtime {
                group.seal();
            }
        }
    }

    #[cfg(feature = "work-execution")]
    fn poll_work(&mut self) {
        self.grant_native_audit();
        self.admit_queued_pages();
        for page in &mut self.retained_pages {
            page.poll();
            if let Some(queue) = &self.self_queue {
                queue.schedule_work(page.next_deadline());
            }
        }
        self.retire_settled_pages();
        if let Some(work) = &mut self.retained_work {
            work.poll();
            if let Some(queue) = &self.self_queue {
                queue.schedule_work(work.next_deadline());
            }
        }
        self.retained_graveyard.retain(|work| !work.is_closed());
        for work in &mut self.retained_graveyard {
            work.poll();
            if let Some(queue) = &self.self_queue {
                queue.schedule_work(work.next_deadline());
            }
        }
        if let Some(work) = &mut self.work {
            work.poll();
            if let Some(queue) = &self.self_queue {
                queue.schedule_work(work.next_deadline());
            }
        }
    }

    fn shutdown_until(&mut self, deadline: std::time::Instant, ack: SyncSender<ShutdownOutcome>) {
        #[cfg(feature = "work-runtime")]
        crate::work_commands::shutdown();
        #[cfg(feature = "work-execution")]
        let lane = self.page_lane();
        #[cfg(feature = "work-execution")]
        for mut page in std::mem::take(&mut self.queued_pages) {
            page.refuse(
                crate::work_resources::product::RetainedRefusal::Discarded,
                lane,
            );
        }
        #[cfg(feature = "work-execution")]
        for page in &mut self.retained_pages {
            page.begin_shutdown();
        }
        #[cfg(feature = "work-execution")]
        if let Some(work) = &mut self.retained_work {
            work.begin_shutdown();
        }
        #[cfg(feature = "work-execution")]
        for work in &mut self.retained_graveyard {
            work.begin_shutdown();
        }
        #[cfg(feature = "work-execution")]
        if let Some(work) = &mut self.work {
            work.begin_shutdown();
        }
        if std::time::Instant::now() >= deadline {
            self.retryable_shutdown_failure(ack);
            return;
        }

        self.cancel_pending_page_permission_for_shutdown();

        let mut terminal_clean = true;
        if let Some(reads) = &self.store_reads {
            match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                reads.quiesce_until(deadline)
            })) {
                Ok(true) => {}
                Ok(false) => {
                    self.retryable_shutdown_failure(ack);
                    return;
                }
                Err(_) => {
                    crate::diagnostic!("shutdown: storage-reader quiescence panicked");
                    terminal_clean = false;
                }
            }
        }
        self.clear_pending_store_reads();

        if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.flush_time(None))).is_err()
        {
            crate::diagnostic!("shutdown: final time flush panicked");
        }
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.flush_pending_focus_records(true);
            !self.has_unadmitted_time_writes()
        })) {
            Ok(true) => {}
            Ok(false) => {
                self.retryable_shutdown_failure(ack);
                return;
            }
            Err(_) => {
                crate::diagnostic!("shutdown: focus admission preflight panicked");
                terminal_clean = false;
            }
        }
        if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.persist())).is_err() {
            crate::diagnostic!("shutdown: final session snapshot panicked");
            terminal_clean = false;
        }

        // Preserve retryability only while every earlier boundary is still
        // known-good.
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.store.flush_until(deadline)
        })) {
            Ok(true) => {}
            Ok(false) if terminal_clean => {
                self.retryable_shutdown_failure(ack);
                return;
            }
            Ok(false) => {
                crate::diagnostic!("shutdown: storage durability preflight was not proven");
                terminal_clean = false;
            }
            Err(_) => {
                crate::diagnostic!("shutdown: storage durability preflight panicked");
                terminal_clean = false;
            }
        }

        #[cfg(feature = "agentic-browser")]
        let agent_lifecycle_clean = self.shutdown_agent_lifecycle_until(deadline);
        #[cfg(not(feature = "agentic-browser"))]
        let agent_lifecycle_clean = true;
        // Fold every result already published before Store's terminal
        // barrier while ordinary Store/native admission is still valid. Any
        // follow-up reconciliation is then ordered ahead of Store shutdown.
        // A projection or adapter panic is terminal, but cannot skip the
        // independent Store/native/blocker barriers below.
        let pre_store_coordination_clean = if std::time::Instant::now() >= deadline {
            crate::diagnostic!("shutdown: pre-Store blocker result folding deadline passed");
            false
        } else if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.drain_blocker_inbox();
        }))
        .is_err()
        {
            crate::diagnostic!("shutdown: pre-Store blocker result folding panicked");
            false
        } else {
            true
        };
        if !pre_store_coordination_clean {
            crate::diagnostic!("shutdown: pre-Store blocker result folding panicked");
        }
        terminal_clean &= self.flush_blocker_statistics_until(deadline);
        let storage_clean = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.store.shutdown_until(deadline)
        })) {
            Ok(StoreShutdownOutcome::Clean) => true,
            Ok(StoreShutdownOutcome::RetryableFailure) => {
                crate::diagnostic!("shutdown: storage rejected terminal teardown");
                false
            }
            Ok(StoreShutdownOutcome::Unclean) => {
                crate::diagnostic!(
                    "shutdown: storage actor termination was not proven before the deadline"
                );
                false
            }
            Err(_) => {
                crate::diagnostic!("shutdown: storage terminal barrier panicked");
                false
            }
        };

        // Projection callbacks are composition ports too. Contain this phase
        // independently so an unhealthy UI cannot skip native or blocker
        // teardown after terminal ownership transfer has begun.
        let post_store_coordination_clean =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                self.discard_blocker_inbox_for_shutdown();
                self.finish_pending_blocker_operations_for_shutdown();
                self.fail_pending_history_calls();
                self.fail_pending_bookmark_calls();
            }))
            .is_ok();
        if !post_store_coordination_clean {
            crate::diagnostic!("shutdown: pending operation finalization panicked");
        }
        let coordination_clean = pre_store_coordination_clean && post_store_coordination_clean;
        let reads_stopped = self.store_reads.as_ref().is_none_or(|reads| {
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| reads.stop())).is_ok()
        });
        if !reads_stopped {
            crate::diagnostic!("shutdown: storage-reader stop panicked");
        }

        let (native_clean, blocker_clean) = self.shutdown_native_and_blocker_until(deadline);

        let clean = terminal_clean
            && agent_lifecycle_clean
            && storage_clean
            && coordination_clean
            && reads_stopped
            && native_clean
            && blocker_clean;
        if !clean {
            crate::diagnostic!(
                "shutdown: clean proof incomplete terminal={terminal_clean} agent_lifecycle={agent_lifecycle_clean} storage={storage_clean} coordination={coordination_clean} readers={reads_stopped} native={native_clean} blocker={blocker_clean}"
            );
        }
        let outcome = if clean {
            ShutdownOutcome::Clean
        } else {
            ShutdownOutcome::Unclean
        };
        self.shutdown_result = Some(outcome);
        let _ = ack.send(outcome);
    }

    fn shutdown_native_and_blocker_until(&self, deadline: std::time::Instant) -> (bool, bool) {
        let (native_done, native_wait) = sync_channel(1);
        let native_admitted = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.engine.shutdown(Box::new(move |clean| {
                let _ = native_done.send(clean);
            }));
        }))
        .is_ok();
        if !native_admitted {
            crate::diagnostic!("shutdown: native cleanup admission panicked");
        }

        // Native teardown and blocker joins are independent and share the
        // same caller-owned absolute deadline.
        let blocker_clean = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.blocker.shutdown_until(deadline)
        })) {
            Ok(BlockerShutdownOutcome::Clean) => true,
            Ok(BlockerShutdownOutcome::Unclean) => {
                crate::diagnostic!(
                    "shutdown: content-policy compiler termination was not proven before the deadline"
                );
                false
            }
            Err(_) => {
                crate::diagnostic!("shutdown: content-policy compiler shutdown panicked");
                false
            }
        };
        let native_budget = deadline.saturating_duration_since(std::time::Instant::now());
        // Observe the callback independently even when admission panicked: a
        // faulty adapter may have retained callback ownership before unwind.
        let native_ack_clean = native_wait.recv_timeout(native_budget).unwrap_or(false);
        let native_clean = native_admitted && native_ack_clean;
        if !native_clean {
            crate::diagnostic!(
                "shutdown: native cleanup did not acknowledge cleanly; forcing process exit"
            );
        }
        (native_clean, blocker_clean)
    }

    /// Best-effort terminal cleanup for actor unwind or loss of the last
    /// public handle. No mutable state is persisted and no refusal is
    /// retryable because the sole authoritative actor is already exiting.
    pub(super) fn cleanup_after_unexpected_exit_until(
        &mut self,
        deadline: std::time::Instant,
    ) -> bool {
        let reads_quiesced = self.store_reads.as_ref().is_none_or(|reads| {
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                reads.quiesce_until(deadline)
            }))
            .unwrap_or(false)
        });
        let reads_stopped = self.store_reads.as_ref().is_none_or(|reads| {
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| reads.stop())).is_ok()
        });
        #[cfg(feature = "agentic-browser")]
        let agent_lifecycle_clean = self.shutdown_agent_lifecycle_until(deadline);
        #[cfg(not(feature = "agentic-browser"))]
        let agent_lifecycle_clean = true;
        let storage_clean = matches!(
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                self.store.shutdown_until(deadline)
            })),
            Ok(StoreShutdownOutcome::Clean)
        );
        if !storage_clean {
            crate::diagnostic!(
                "shutdown: storage cleanup was not proven during unexpected shell exit"
            );
        }
        let (native_clean, blocker_clean) = self.shutdown_native_and_blocker_until(deadline);
        reads_quiesced
            && reads_stopped
            && agent_lifecycle_clean
            && storage_clean
            && native_clean
            && blocker_clean
    }

    pub(super) fn report_terminal_failure(&mut self, failure: ShellTerminalFailure) {
        let Some(callback) = self.terminal_failure.take() else {
            return;
        };
        if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| callback(failure))).is_err() {
            crate::diagnostic!("shutdown: terminal shell failure handoff panicked");
            self.terminal_failure_handoff_panicked = true;
        }
    }

    /// Consumes the optional complete agent runtime before terminal Store and
    /// engine teardown. A clean result cannot exist without its native zero
    /// proof; the proof is deliberately consumed inside the actor barrier.
    #[cfg(feature = "agentic-browser")]
    fn shutdown_agent_lifecycle_until(&mut self, deadline: std::time::Instant) -> bool {
        #[cfg(feature = "work-execution")]
        let mut buried = true;
        #[cfg(feature = "work-execution")]
        if self.retained_page_runtime.is_some() {
            for page in &mut self.retained_pages {
                page.begin_shutdown();
            }
            let open = |shell: &Self| {
                shell.retained_pages.iter().any(|page| !page.is_closed())
                    || shell
                        .retained_graveyard
                        .iter()
                        .any(|work| work.awaits_native_close())
            };
            while std::time::Instant::now() < deadline && open(self) {
                self.grant_native_audit();
                for work in self
                    .retained_pages
                    .iter_mut()
                    .chain(&mut self.retained_graveyard)
                {
                    work.poll();
                }
                if open(self) {
                    std::thread::sleep(std::time::Duration::from_millis(1));
                }
            }
            buried &= self.retained_pages.iter().all(|page| page.is_closed());
        }
        #[cfg(feature = "work-execution")]
        let retained_clean = self
            .retained_work
            .as_mut()
            .map(|work| work.shutdown_until(deadline));
        // What the group could not close audits last, one at a time.
        #[cfg(feature = "work-execution")]
        for work in &mut self.retained_graveyard {
            work.allow_group_shutdown();
            buried &= work.shutdown_until(deadline);
        }
        #[cfg(feature = "work-execution")]
        if let Some(clean) = retained_clean {
            return clean && buried;
        }
        #[cfg(feature = "work-execution")]
        if !buried {
            return false;
        }
        #[cfg(feature = "work-execution")]
        if let Some(work) = &mut self.work {
            return work.shutdown_until(deadline);
        }
        let owner = std::mem::replace(&mut self.agent_lifecycle, AgentLifecycleOwner::Consumed);
        let lifecycle = match owner {
            AgentLifecycleOwner::Absent => return true,
            AgentLifecycleOwner::Owned(lifecycle) => lifecycle,
            AgentLifecycleOwner::Consumed => {
                crate::diagnostic!("shutdown: agent browser lifecycle owner is missing");
                return false;
            }
        };
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            lifecycle.shutdown_until(deadline)
        })) {
            Ok(AgentBrowserShutdownOutcome::Clean(native_zero_proof)) => {
                drop(native_zero_proof);
                true
            }
            Ok(AgentBrowserShutdownOutcome::Unclean) => {
                crate::diagnostic!(
                    "shutdown: agent browser lifecycle did not prove complete cleanup"
                );
                false
            }
            Err(_) => {
                crate::diagnostic!("shutdown: agent browser lifecycle shutdown panicked");
                false
            }
        }
    }

    fn retryable_shutdown_failure(&mut self, ack: SyncSender<ShutdownOutcome>) {
        let recovered = self
            .self_queue
            .as_ref()
            .map(CommandQueue::reopen_after_failed_shutdown)
            .unwrap_or_default();
        // These callbacks were truthfully rejected after the barrier, but the
        // browser is about to resume. Fold their bounded latest state into the
        // actor before acknowledging failure or accepting newly dispatched
        // work.
        for command in recovered {
            self.handle(command);
        }
        let now = std::time::Instant::now();
        if let Some(queue) = &self.self_queue {
            // A timer wake removes its entry before trying to enter the actor.
            // If it raced the shutdown barrier it was truthfully rejected as
            // sealed, so explicitly restore every still-live exact reveal
            // obligation when the retryable barrier reopens. Both maps remain
            // bounded to one entry per logical item.
            for (id, pending) in &self.presentation.pending_presentations {
                queue.schedule_presentation(*id, pending.navigation, now, pending.hard_deadline);
            }
        }
        if let Some(reads) = &self.store_reads {
            reads.resume();
        }
        // A failed terminal storage admission may have invalidated pending
        // presentation reads. Restart only currently visible leaves (at most
        // the split ceiling), never all restored tabs.
        let visible = self
            .pane_tree()
            .map(|tree| tree.tabs())
            .or_else(|| {
                self.windows
                    .focused()
                    .and_then(|window| window.active)
                    .map(|id| vec![id])
            })
            .unwrap_or_default();
        for id in visible {
            self.maybe_discover_favicon(id);
        }
        let _ = ack.send(ShutdownOutcome::RetryableFailure);
    }

    fn clear_pending_store_reads(&mut self) {
        self.clear_pending_favicon_probe_lookups();
        self.search.pending = None;
        self.favicons.pending_batch = None;
        self.favicons.store_reads.clear();
    }

    fn on_store_read(&mut self, result: StoreReadResult) {
        match result {
            StoreReadResult::History {
                generation,
                profile,
                query,
                hits,
            } => self.on_history_read(generation, profile, query, hits),
            StoreReadResult::Favicon {
                generation,
                id,
                profile,
                origin,
                rgba,
                stale,
            } => self.on_favicon_read(generation, id, profile, origin, rgba, stale),
            StoreReadResult::FaviconBatch {
                generation,
                profile,
                space,
                origins,
                rasters,
            } => self.on_favicon_batch_read(generation, profile, space, origins, rasters),
            StoreReadResult::FaviconProbe {
                generation,
                profile,
                origins,
                rasters,
            } => self.on_favicon_probe_read(generation, profile, origins, rasters),
            StoreReadResult::HistorySurface {
                token,
                profile,
                visits,
                next,
                removed,
            } => self.on_history_surface_read(token, profile, visits, next, removed),
            StoreReadResult::HistorySurfaceFailed { token, profile } => {
                self.on_history_surface_failed(token, profile)
            }
            StoreReadResult::Bookmarks {
                token,
                profile,
                reply,
            } => self.on_bookmarks_read(token, profile, reply),
            StoreReadResult::Imported { token, added } => self.on_import_read(token, added),
        }
    }
}

#[cfg(test)]
pub(crate) mod tests;

#[cfg(all(test, feature = "work-runtime"))]
mod work_address_tests;
#[cfg(all(test, feature = "work-planning"))]
mod work_context_tests;
#[cfg(all(test, feature = "work-runtime"))]
mod work_coordination_tests;
#[cfg(all(test, feature = "work-runtime"))]
mod work_lead_tests;
#[cfg(all(test, feature = "work-runtime"))]
mod work_personal_tests;
#[cfg(all(test, feature = "work-planning"))]
mod work_planning_tests;
#[cfg(all(test, feature = "work-runtime"))]
mod work_product_tests;
#[cfg(all(test, feature = "work-runtime"))]
mod work_runtime_tests;
