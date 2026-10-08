//! Bounded command admission, coalescing, and timer scheduling.

#[cfg(test)]
mod tests;

use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex};

use zephium_core::blocker::ContentPolicyGeneration;
use zephium_core::ids::{ItemId, ProfileId};
use zephium_core::permissions::PagePermissionRequestId;
use zephium_core::ports::engine::{
    ContentScope, DiscardProbeId, EngineEvent, NavigationPresentationId,
};

use crate::{Command, StoreReadResult};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum CoalescedKey {
    #[cfg(feature = "work-execution")]
    Work,
    Bootstrap,
    FocusWake,
    Title(ItemId),
    Url(ItemId),
    Loading(ItemId),
    Favicon(ItemId),
    PageMemory(ItemId),
    FaviconPoll(ItemId),
    Presentation(ItemId),
    ChromePresentation(ItemId),
    PresentationFallback(ItemId),
    Fullscreen(ItemId),
    DiscardProbeTimeout(ItemId),
    ViewCapacityRetry(ItemId),
    BlockerReady(ProfileId),
    BlockerStoreReady(ProfileId),
    BlockerPreferenceRetry(ProfileId),
    BlockerCatalogPoll,
    ContentRules(ProfileId, ContentPolicyGeneration),
    // Generations are monotonic and the newest settlement is authoritative;
    // scope-only coalescing prevents a rapid sequence from defeating the
    // one-slot-per-scope lifecycle-capacity proof.
    UserContent(ContentScope),
    ProfileDeletionReady(ProfileId),
    ProfileDeletionRetry(ProfileId),
    StoreHistory,
    StoreFavicon(ItemId),
    StoreFaviconBatch,
    Navigation(ItemId),
    Zoom(ItemId),
    NativeAction(ItemId),
    ExtensionActions(ProfileId),
    ExtensionPageClosed(ItemId),
    ExtensionPageChanged(ItemId),
    Split(zephium_core::ids::WindowId),
    WindowSize,
    WindowVisible,
    MediaCapture(ItemId),
    MemoryPressure,
    SidebarWidth,
    SidebarGuide,
    SidebarGuideEnd,
    WorkPaneRect,
    DragOver,
    DividerDrag,
    Search,
    Persist,
    Tick,
}

impl CoalescedKey {
    fn of(event: &EngineEvent) -> Option<Self> {
        Some(match event {
            EngineEvent::TitleChanged { id, .. } => Self::Title(*id),
            EngineEvent::UrlChanged { id, .. } => Self::Url(*id),
            EngineEvent::LoadingChanged { id, .. } => Self::Loading(*id),
            EngineEvent::FaviconPixels { id, .. } => Self::Favicon(*id),
            EngineEvent::PageMemory { id, .. } => Self::PageMemory(*id),
            EngineEvent::PresentationPending { id, .. }
            | EngineEvent::PresentationReady { id, .. } => Self::Presentation(*id),
            EngineEvent::NavState { id, .. } => Self::Navigation(*id),
            EngineEvent::ZoomSettled { id, .. } => Self::Zoom(*id),
            EngineEvent::NativeActionFailed { id, .. }
            | EngineEvent::PageOpenBlocked { id, .. } => Self::NativeAction(*id),
            EngineEvent::ExtensionActionsInvalidated { profile } => {
                Self::ExtensionActions(*profile)
            }
            EngineEvent::ExtensionPageClosed { id, .. } => Self::ExtensionPageClosed(*id),
            EngineEvent::ExtensionPageChanged { id, .. } => Self::ExtensionPageChanged(*id),
            EngineEvent::MediaCaptureChanged { id, .. } => Self::MediaCapture(*id),
            EngineEvent::FullscreenChanged { id, .. } => Self::Fullscreen(*id),
            EngineEvent::SplitChanged { window, .. } => Self::Split(*window),
            EngineEvent::ContentRulesSettled {
                profile, requested, ..
            } => Self::ContentRules(*profile, *requested),
            EngineEvent::UserContentSettled { scope, .. } => Self::UserContent(*scope),
            _ => return None,
        })
    }
}

// The ordinary UI band stays small, while the lifecycle band can retain one
// latest URL, view-state and navigation result per maximum session item plus
// profile/split/runtime-update facts and the final shutdown barrier. A shared
// WebKit process may terminate all 1,024 live views in one native callback
// burst.
const NORMAL_COMMAND_CAPACITY: usize = 960;
// Each tracked tab can have one latest URL, presentation, navigation failure,
// terminal view-state, zoom settlement, native-action failure, media-capture
// and fullscreen fact. Each profile can independently have one process-exit
// fact, compiler-result wake, preference-store wake, native-policy settlement,
// and durable-deletion callback wakeup. The current single-window shell can
// have one native split fact, and the process can have one sticky
// runtime-update fact. Reserve all of those independently of the
// already-accepted user FIFO.
const MAX_CRITICAL_LIFECYCLE_FACTS: usize = zephium_core::session::MAX_SESSION_ITEMS * 10
    + zephium_core::session::MAX_SESSION_PROFILES * 7
    + zephium_core::extensions::MAX_PENDING_EXTENSION_BROWSER_REQUESTS
    + zephium_core::permissions::MAX_PENDING_PAGE_PERMISSION_REQUESTS
    + 5 // Includes replaceable memory-pressure and guide-cleanup facts.
    + cfg!(feature = "work-execution") as usize;
const COMMAND_QUEUE_CAPACITY: usize = NORMAL_COMMAND_CAPACITY + MAX_CRITICAL_LIFECYCLE_FACTS + 1;
const LIFECYCLE_COMMAND_CAPACITY: usize = COMMAND_QUEUE_CAPACITY - 1;
// During a failed store barrier, keep at most the bounded set of lifecycle
// facts the native engine can produce for the maximum item/profile counts.
const POST_BARRIER_CRITICAL_CAPACITY: usize = MAX_CRITICAL_LIFECYCLE_FACTS;

#[derive(Clone)]
pub(crate) struct CommandQueue {
    pub(crate) inner: Arc<CommandQueueInner>,
}

pub(crate) struct CommandQueueInner {
    pub(crate) state: Mutex<CommandQueueState>,
    ready: Condvar,
    pub(crate) timer_state: Mutex<TimerState>,
    timer_ready: Condvar,
}

#[derive(Default)]
pub(crate) struct TimerState {
    #[cfg(feature = "work-execution")]
    work_deadline: Option<std::time::Instant>,
    stopped: bool,
    pub(crate) persist_deadline: Option<std::time::Instant>,
    focus_deadline: Option<std::time::Instant>,
    favicon_deadlines: std::collections::HashMap<ItemId, (std::time::Instant, u8)>,
    pub(crate) presentation_deadlines: std::collections::HashMap<ItemId, PresentationDeadline>,
    discard_deadlines: std::collections::HashMap<ItemId, (std::time::Instant, DiscardProbeId)>,
    capacity_deadlines: std::collections::HashMap<ItemId, std::time::Instant>,
    profile_deletion_deadlines: std::collections::HashMap<ProfileId, (std::time::Instant, u64)>,
    blocker_preference_deadlines: std::collections::HashMap<ProfileId, (std::time::Instant, u64)>,
    blocker_catalog_deadline: Option<(std::time::Instant, u64, u8)>,
    page_permission_deadline: Option<(
        std::time::Instant,
        ProfileId,
        ItemId,
        PagePermissionRequestId,
    )>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PresentationDeadline {
    pub(crate) wake: std::time::Instant,
    pub(crate) hard: std::time::Instant,
    pub(crate) navigation: NavigationPresentationId,
}

pub(crate) enum TimerWake {
    #[cfg(feature = "work-execution")]
    Work,
    Maintenance,
    Persist,
    Focus,
    Favicon {
        id: ItemId,
        attempt: u8,
    },
    Presentation {
        id: ItemId,
        navigation: NavigationPresentationId,
        hard_deadline: std::time::Instant,
    },
    DiscardProbe {
        id: ItemId,
        probe: DiscardProbeId,
    },
    ViewCapacity {
        id: ItemId,
    },
    ProfileDeletion {
        profile: ProfileId,
        generation: u64,
    },
    BlockerPreference {
        profile: ProfileId,
        token: u64,
    },
    BlockerCatalog {
        operation: u64,
        attempt: u8,
    },
    PagePermission {
        profile: ProfileId,
        item: ItemId,
        request: PagePermissionRequestId,
    },
    Stopped,
}

#[derive(Default)]
pub(crate) struct CommandQueueState {
    commands: VecDeque<Command>,
    // Critical native callbacks that race an in-progress store barrier. They
    // are not reported as accepted behind Shutdown, but must remain available
    // if that barrier fails and the live browser resumes.
    post_barrier_critical: VecDeque<Command>,
    closed: bool,
    shutdown_enqueued: bool,
    pub(crate) handles: usize,
}

pub(crate) enum TryPushError {
    Full(Command),
    Sealed(Command),
    Closed(Command),
}

// Refusal must return the exact command so lifecycle/native callers can retry
// or account for it without cloning authority-bearing payloads. Keep that
// allocation-free failure contract bounded as `Command` evolves.
const _: () = assert!(std::mem::size_of::<TryPushError>() <= 160);

#[derive(Clone, Copy, PartialEq, Eq)]
enum RecoveryKey {
    MediaCapture(ItemId),
    Fullscreen(ItemId),
    RuntimeRestart,
    MemoryPressure,
    SidebarGuideEnd,
    Url(ItemId),
    Presentation(ItemId),
    ChromePresentation(ItemId),
    ViewState(ItemId),
    NavigationFailure(ItemId),
    Zoom(ItemId),
    NativeAction(ItemId),
    Profile(ProfileId),
    BlockerCompile(ProfileId),
    BlockerStore(ProfileId),
    ContentRules(ProfileId, ContentPolicyGeneration),
    UserContent(ContentScope),
    ProfileDeletion(ProfileId),
    Split(zephium_core::ids::WindowId),
}

fn recovery_key(command: &Command) -> Option<RecoveryKey> {
    match command {
        Command::Engine(EngineEvent::MediaCaptureChanged { id, .. }) => {
            Some(RecoveryKey::MediaCapture(*id))
        }
        Command::Engine(EngineEvent::FullscreenChanged { id, .. }) => {
            Some(RecoveryKey::Fullscreen(*id))
        }
        Command::Engine(EngineEvent::RuntimeRestartRequired) => Some(RecoveryKey::RuntimeRestart),
        Command::SetMemoryPressure(_) => Some(RecoveryKey::MemoryPressure),
        Command::SidebarResizeGuide(None) => Some(RecoveryKey::SidebarGuideEnd),
        Command::Engine(EngineEvent::UrlChanged { id, .. }) => Some(RecoveryKey::Url(*id)),
        Command::Engine(
            EngineEvent::PresentationPending { id, .. } | EngineEvent::PresentationReady { id, .. },
        ) => Some(RecoveryKey::Presentation(*id)),
        Command::ChromePresentationApplied { id, .. } => Some(RecoveryKey::ChromePresentation(*id)),
        Command::Engine(EngineEvent::NavigationFailed { id, .. }) => {
            Some(RecoveryKey::NavigationFailure(*id))
        }
        Command::Engine(EngineEvent::ZoomSettled { id, .. }) => Some(RecoveryKey::Zoom(*id)),
        Command::Engine(EngineEvent::NativeActionFailed { id, .. }) => {
            Some(RecoveryKey::NativeAction(*id))
        }
        Command::Engine(
            EngineEvent::ViewCreationFailed { id }
            | EngineEvent::Crashed { id }
            | EngineEvent::ViewDiscarded { id, .. }
            | EngineEvent::ViewDiscardRefused { id, .. },
        ) => Some(RecoveryKey::ViewState(*id)),
        Command::Engine(EngineEvent::ProfileProcessExited { profile, .. }) => {
            Some(RecoveryKey::Profile(*profile))
        }
        Command::BlockerReady(profile) => Some(RecoveryKey::BlockerCompile(*profile)),
        Command::BlockerStoreReady(profile) => Some(RecoveryKey::BlockerStore(*profile)),
        Command::Engine(EngineEvent::ContentRulesSettled {
            profile, requested, ..
        }) => Some(RecoveryKey::ContentRules(*profile, *requested)),
        Command::Engine(EngineEvent::UserContentSettled { scope, .. }) => {
            Some(RecoveryKey::UserContent(*scope))
        }
        Command::ProfileDeletionReady(profile) => Some(RecoveryKey::ProfileDeletion(*profile)),
        Command::Engine(EngineEvent::SplitChanged { window, .. }) => {
            Some(RecoveryKey::Split(*window))
        }
        _ => None,
    }
}

fn retain_post_barrier(commands: &mut VecDeque<Command>, command: Command) {
    if let Some(key) = recovery_key(&command) {
        if let Some(index) = commands
            .iter()
            .rposition(|queued| recovery_key(queued) == Some(key))
        {
            commands.remove(index);
        }
    }
    if commands.len() < POST_BARRIER_CRITICAL_CAPACITY {
        commands.push_back(command);
    }
}

impl CommandQueue {
    #[cfg(feature = "work-execution")]
    pub(crate) fn schedule_work(&self, deadline: Option<std::time::Instant>) {
        let mut timer = self
            .inner
            .timer_state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if timer.work_deadline != deadline {
            timer.work_deadline = deadline;
            self.inner.timer_ready.notify_one();
        }
    }
    pub(crate) fn new() -> Self {
        Self {
            inner: Arc::new(CommandQueueInner {
                state: Mutex::new(CommandQueueState::default()),
                ready: Condvar::new(),
                timer_state: Mutex::new(TimerState::default()),
                timer_ready: Condvar::new(),
            }),
        }
    }

    pub(crate) fn retain_handle(&self) -> bool {
        {
            let mut state = self
                .inner
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if state.closed {
            } else if let Some(handles) = state.handles.checked_add(1) {
                state.handles = handles;
                return true;
            } else {
                // `Clone` cannot report failure. Seal the actor rather than
                // aborting or returning an uncounted live ingress that could
                // outlast the final owner. The returned Handle records that
                // it was not counted, so its Drop cannot underflow state.
                state.closed = true;
                state.post_barrier_critical.clear();
                self.inner.ready.notify_all();
            }
        }
        self.stop_ticker();
        false
    }

    /// Releases one public owner and reports whether this was the final owner.
    /// The caller uses that transition to cancel a still-suspended actor before
    /// its independent startup gate could otherwise remain asleep forever.
    pub(crate) fn release_handle(&self) -> bool {
        let should_stop = {
            let mut state = self
                .inner
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if state.handles == 0 {
                // A bookkeeping invariant failure must terminate admission,
                // not abort a release build. There is no safe owner count to
                // reconstruct, so fail closed and wake both actor threads.
                state.closed = true;
                state.post_barrier_critical.clear();
                self.inner.ready.notify_all();
                true
            } else {
                state.handles -= 1;
                if state.handles != 0 || state.closed {
                    false
                } else {
                    // Reject new internal/native work, but let the actor drain
                    // commands already accepted before its final public owner
                    // went away. In particular, dropping the handle after
                    // requesting Shutdown must not cancel that barrier.
                    state.closed = true;
                    state.post_barrier_critical.clear();
                    self.inner.ready.notify_all();
                    true
                }
            }
        };
        if should_stop {
            self.stop_ticker();
        }
        should_stop
    }

    #[allow(
        clippy::result_large_err,
        reason = "mailbox refusal returns the exact command without allocating on saturation"
    )]
    pub(crate) fn try_push(&self, command: Command) -> Result<(), TryPushError> {
        let mut state = self
            .inner
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let shutdown = matches!(&command, Command::Shutdown { .. });
        if state.closed {
            return Err(TryPushError::Closed(command));
        }
        if state.shutdown_enqueued {
            if command_is_critical(&command) {
                // Retain a clone only for failed-barrier recovery. Returning
                // Sealed remains truthful to the native callback: this work
                // was not admitted after the ordered shutdown point.
                retain_post_barrier(&mut state.post_barrier_critical, command.clone());
            }
            return Err(TryPushError::Sealed(command));
        }
        let capacity = if shutdown {
            COMMAND_QUEUE_CAPACITY
        } else if command_is_critical(&command) {
            LIFECYCLE_COMMAND_CAPACITY
        } else {
            NORMAL_COMMAND_CAPACITY
        };
        // Observational diagnostics must never perturb browser coordination.
        // User mutations may displace an older replaceable presentation fact
        // at the ordinary ceiling; a read-only query instead fails boundedly
        // and lets its caller return an explicit unavailable result.
        if command_is_observational_query(&command) && state.commands.len() >= capacity {
            return Err(TryPushError::Full(command));
        }
        let critical = command_is_critical(&command);
        enqueue(&mut state.commands, command, capacity, critical).map_err(TryPushError::Full)?;
        if shutdown {
            // Seal admission at the same ordered point as the barrier. No
            // caller can receive `true` for work that would land behind it.
            state.shutdown_enqueued = true;
        }
        self.inner.ready.notify_one();
        Ok(())
    }

    pub(crate) fn recv(&self) -> Option<Command> {
        let mut state = self
            .inner
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        loop {
            if let Some(command) = state.commands.pop_front() {
                return Some(command);
            }
            if state.closed {
                return None;
            }
            state = self
                .inner
                .ready
                .wait(state)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
        }
    }

    #[cfg(test)]
    pub(crate) fn try_recv(&self) -> Option<Command> {
        let mut state = self
            .inner
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.commands.pop_front()
    }

    pub(crate) fn reopen_after_failed_shutdown(&self) -> Vec<Command> {
        let mut state = self
            .inner
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if !state.closed {
            state.shutdown_enqueued = false;
            return state.post_barrier_critical.drain(..).collect();
        }
        Vec::new()
    }

    pub(crate) fn close_and_drain(&self) -> Vec<Command> {
        let pending = {
            let mut state = self
                .inner
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            state.closed = true;
            state.post_barrier_critical.clear();
            let pending = state.commands.drain(..).collect();
            self.inner.ready.notify_all();
            pending
        };
        self.stop_ticker();
        pending
    }

    pub(crate) fn stop_ticker(&self) {
        let mut timer = self
            .inner
            .timer_state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        timer.stopped = true;
        timer.persist_deadline = None;
        timer.focus_deadline = None;
        timer.favicon_deadlines.clear();
        timer.presentation_deadlines.clear();
        timer.discard_deadlines.clear();
        timer.capacity_deadlines.clear();
        timer.profile_deletion_deadlines.clear();
        timer.blocker_preference_deadlines.clear();
        timer.blocker_catalog_deadline = None;
        timer.page_permission_deadline = None;
        self.inner.timer_ready.notify_all();
    }

    pub(crate) fn schedule_persist(&self, deadline: std::time::Instant) {
        let mut timer = self
            .inner
            .timer_state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if timer.stopped {
            return;
        }
        timer.persist_deadline = Some(deadline);
        self.inner.timer_ready.notify_one();
    }

    /// One deadline for the running focus session's next change; a later
    /// schedule replaces it, and `None` cancels it.
    pub(crate) fn schedule_focus(&self, deadline: Option<std::time::Instant>) {
        let mut timer = self
            .inner
            .timer_state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if timer.stopped {
            return;
        }
        timer.focus_deadline = deadline;
        self.inner.timer_ready.notify_one();
    }

    pub(crate) fn schedule_page_permission(
        &self,
        profile: ProfileId,
        item: ItemId,
        request: PagePermissionRequestId,
        deadline: std::time::Instant,
    ) {
        let mut timer = self
            .inner
            .timer_state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if timer.stopped {
            return;
        }
        timer.page_permission_deadline = Some((deadline, profile, item, request));
        self.inner.timer_ready.notify_one();
    }

    pub(crate) fn cancel_page_permission(
        &self,
        profile: ProfileId,
        item: ItemId,
        request: PagePermissionRequestId,
    ) {
        let mut timer = self
            .inner
            .timer_state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if timer.page_permission_deadline.is_some_and(
            |(_, current_profile, current_item, current_request)| {
                current_profile == profile && current_item == item && current_request == request
            },
        ) {
            timer.page_permission_deadline = None;
        }
    }

    pub(crate) fn cancel_persist(&self) {
        let mut timer = self
            .inner
            .timer_state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        timer.persist_deadline = None;
    }

    pub(crate) fn schedule_favicon(&self, id: ItemId, attempt: u8, deadline: std::time::Instant) {
        let mut timer = self
            .inner
            .timer_state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if timer.stopped {
            return;
        }
        timer.favicon_deadlines.insert(id, (deadline, attempt));
        self.inner.timer_ready.notify_one();
    }

    pub(crate) fn cancel_favicon(&self, id: ItemId) {
        let mut timer = self
            .inner
            .timer_state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        timer.favicon_deadlines.remove(&id);
    }

    pub(crate) fn schedule_presentation(
        &self,
        id: ItemId,
        navigation: NavigationPresentationId,
        wake: std::time::Instant,
        hard: std::time::Instant,
    ) {
        let mut timer = self
            .inner
            .timer_state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if timer.stopped {
            return;
        }
        // One native generation has at most one initially hidden epoch. Keep
        // the earliest deadline for a duplicate token so same-document URL
        // churn cannot postpone presentation indefinitely.
        match timer.presentation_deadlines.entry(id) {
            std::collections::hash_map::Entry::Occupied(mut entry)
                if entry.get().navigation == navigation =>
            {
                let current = entry.get_mut();
                current.wake = current.wake.min(wake);
                current.hard = current.hard.min(hard);
            }
            std::collections::hash_map::Entry::Occupied(mut entry) => {
                // A hidden page can commit a chain of cross-document
                // navigations before first presentation. Advance the exact
                // token, but retain both original absolute bounds so hostile
                // navigation churn cannot restart either grace period.
                let current = *entry.get();
                entry.insert(PresentationDeadline {
                    wake: current.wake.min(wake),
                    hard: current.hard.min(hard),
                    navigation,
                });
            }
            std::collections::hash_map::Entry::Vacant(entry) => {
                entry.insert(PresentationDeadline {
                    wake,
                    hard,
                    navigation,
                });
            }
        }
        self.inner.timer_ready.notify_one();
    }

    pub(crate) fn cancel_presentation(&self, id: ItemId) {
        let mut timer = self
            .inner
            .timer_state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        timer.presentation_deadlines.remove(&id);
    }

    /// Re-arms a fallback that could not enter the actor queue without
    /// allowing an escaped wake for an older navigation to replace a newer
    /// native presentation obligation for the same logical tab.
    pub(crate) fn retry_presentation(
        &self,
        id: ItemId,
        navigation: NavigationPresentationId,
        wake: std::time::Instant,
        hard: std::time::Instant,
    ) {
        let mut timer = self
            .inner
            .timer_state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if timer.stopped {
            return;
        }
        match timer.presentation_deadlines.entry(id) {
            std::collections::hash_map::Entry::Occupied(mut entry)
                if entry.get().navigation == navigation =>
            {
                let current = entry.get_mut();
                current.wake = current.wake.min(wake);
                current.hard = current.hard.min(hard);
            }
            std::collections::hash_map::Entry::Occupied(_) => return,
            std::collections::hash_map::Entry::Vacant(entry) => {
                entry.insert(PresentationDeadline {
                    wake,
                    hard,
                    navigation,
                });
            }
        }
        self.inner.timer_ready.notify_one();
    }

    pub(crate) fn schedule_discard_probe(
        &self,
        id: ItemId,
        probe: DiscardProbeId,
        deadline: std::time::Instant,
    ) {
        let mut timer = self
            .inner
            .timer_state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if timer.stopped {
            return;
        }
        timer.discard_deadlines.insert(id, (deadline, probe));
        self.inner.timer_ready.notify_one();
    }

    pub(crate) fn cancel_discard_probe(&self, id: ItemId) {
        let mut timer = self
            .inner
            .timer_state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        timer.discard_deadlines.remove(&id);
    }

    pub(crate) fn schedule_view_capacity(&self, id: ItemId, deadline: std::time::Instant) {
        let mut timer = self
            .inner
            .timer_state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if timer.stopped {
            return;
        }
        // Admission owns at most the visible-pane count of pending requests.
        if timer.capacity_deadlines.contains_key(&id)
            || timer.capacity_deadlines.len() < crate::shell::MAX_VISIBLE_PANES
        {
            timer.capacity_deadlines.insert(id, deadline);
            self.inner.timer_ready.notify_one();
        }
    }

    pub(crate) fn cancel_view_capacity(&self, id: ItemId) {
        let mut timer = self
            .inner
            .timer_state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        timer.capacity_deadlines.remove(&id);
    }

    pub(crate) fn schedule_profile_deletion(
        &self,
        profile: ProfileId,
        generation: u64,
        deadline: std::time::Instant,
    ) {
        let mut timer = self
            .inner
            .timer_state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if timer.stopped {
            return;
        }
        timer
            .profile_deletion_deadlines
            .insert(profile, (deadline, generation));
        self.inner.timer_ready.notify_one();
    }

    pub(crate) fn cancel_profile_deletion(&self, profile: ProfileId) {
        let mut timer = self
            .inner
            .timer_state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        timer.profile_deletion_deadlines.remove(&profile);
    }

    pub(crate) fn schedule_blocker_preference_reconciliation(
        &self,
        profile: ProfileId,
        token: u64,
        deadline: std::time::Instant,
    ) {
        let mut timer = self
            .inner
            .timer_state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if timer.stopped {
            return;
        }
        timer
            .blocker_preference_deadlines
            .insert(profile, (deadline, token));
        self.inner.timer_ready.notify_one();
    }

    pub(crate) fn cancel_blocker_preference_reconciliation(&self, profile: ProfileId) {
        let mut timer = self
            .inner
            .timer_state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        timer.blocker_preference_deadlines.remove(&profile);
    }

    pub(crate) fn schedule_blocker_catalog_poll(
        &self,
        operation: u64,
        attempt: u8,
        deadline: std::time::Instant,
    ) {
        let mut timer = self
            .inner
            .timer_state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if timer.stopped {
            return;
        }
        match timer.blocker_catalog_deadline {
            Some((current_deadline, current_operation, _)) if current_operation == operation => {
                if deadline < current_deadline {
                    timer.blocker_catalog_deadline = Some((deadline, operation, attempt));
                }
            }
            Some((_, current_operation, _)) if current_operation > operation => {}
            Some(_) | None => {
                timer.blocker_catalog_deadline = Some((deadline, operation, attempt));
            }
        }
        self.inner.timer_ready.notify_one();
    }

    pub(crate) fn cancel_blocker_catalog_poll(&self, operation: u64) {
        let mut timer = self
            .inner
            .timer_state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if timer
            .blocker_catalog_deadline
            .is_some_and(|(_, current, _)| current == operation)
        {
            timer.blocker_catalog_deadline = None;
        }
    }

    /// Waits for either the low-frequency maintenance heartbeat or the one
    /// coalesced persistence deadline. Recomputing after every notification
    /// lets navigation churn move the debounce later without polling.
    pub(crate) fn wait_for_timer(&self, maintenance_deadline: std::time::Instant) -> TimerWake {
        let mut timer = self
            .inner
            .timer_state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        loop {
            if timer.stopped {
                return TimerWake::Stopped;
            }
            let now = std::time::Instant::now();
            #[cfg(feature = "work-execution")]
            if timer.work_deadline.is_some_and(|deadline| now >= deadline) {
                timer.work_deadline = None;
                return TimerWake::Work;
            }
            if timer
                .persist_deadline
                .is_some_and(|deadline| now >= deadline)
            {
                timer.persist_deadline = None;
                return TimerWake::Persist;
            }
            if timer.focus_deadline.is_some_and(|deadline| now >= deadline) {
                timer.focus_deadline = None;
                return TimerWake::Focus;
            }
            let next_favicon = timer
                .favicon_deadlines
                .iter()
                .min_by_key(|(_, (deadline, _))| *deadline)
                .map(|(id, (deadline, attempt))| (*id, *deadline, *attempt));
            if let Some((id, deadline, attempt)) = next_favicon {
                if now >= deadline {
                    timer.favicon_deadlines.remove(&id);
                    return TimerWake::Favicon { id, attempt };
                }
            }
            let next_presentation = timer
                .presentation_deadlines
                .iter()
                .min_by_key(|(_, deadline)| deadline.wake)
                .map(|(id, deadline)| (*id, *deadline));
            if let Some((id, deadline)) = next_presentation {
                if now >= deadline.wake {
                    timer.presentation_deadlines.remove(&id);
                    return TimerWake::Presentation {
                        id,
                        navigation: deadline.navigation,
                        hard_deadline: deadline.hard,
                    };
                }
            }
            let next_discard = timer
                .discard_deadlines
                .iter()
                .min_by_key(|(_, (deadline, _))| *deadline)
                .map(|(id, (deadline, probe))| (*id, *deadline, *probe));
            if let Some((id, deadline, probe)) = next_discard {
                if now >= deadline {
                    timer.discard_deadlines.remove(&id);
                    return TimerWake::DiscardProbe { id, probe };
                }
            }
            let next_capacity = timer
                .capacity_deadlines
                .iter()
                .min_by_key(|(_, deadline)| **deadline)
                .map(|(id, deadline)| (*id, *deadline));
            if let Some((id, deadline)) = next_capacity {
                if now >= deadline {
                    timer.capacity_deadlines.remove(&id);
                    return TimerWake::ViewCapacity { id };
                }
            }
            let next_profile_deletion = timer
                .profile_deletion_deadlines
                .iter()
                .min_by_key(|(_, (deadline, _))| *deadline)
                .map(|(profile, (deadline, generation))| (*profile, *deadline, *generation));
            if let Some((profile, deadline, generation)) = next_profile_deletion {
                if now >= deadline {
                    timer.profile_deletion_deadlines.remove(&profile);
                    return TimerWake::ProfileDeletion {
                        profile,
                        generation,
                    };
                }
            }
            let next_blocker_preference = timer
                .blocker_preference_deadlines
                .iter()
                .min_by_key(|(_, (deadline, _))| *deadline)
                .map(|(profile, (deadline, token))| (*profile, *deadline, *token));
            if let Some((profile, deadline, token)) = next_blocker_preference {
                if now >= deadline {
                    timer.blocker_preference_deadlines.remove(&profile);
                    return TimerWake::BlockerPreference { profile, token };
                }
            }
            let next_blocker_catalog = timer.blocker_catalog_deadline;
            if let Some((deadline, operation, attempt)) = next_blocker_catalog {
                if now >= deadline {
                    timer.blocker_catalog_deadline = None;
                    return TimerWake::BlockerCatalog { operation, attempt };
                }
            }
            let next_page_permission = timer.page_permission_deadline;
            if let Some((deadline, profile, item, request)) = next_page_permission {
                if now >= deadline {
                    timer.page_permission_deadline = None;
                    return TimerWake::PagePermission {
                        profile,
                        item,
                        request,
                    };
                }
            }
            if now >= maintenance_deadline {
                return TimerWake::Maintenance;
            }
            let mut deadline = timer
                .persist_deadline
                .map_or(maintenance_deadline, |persist| {
                    persist.min(maintenance_deadline)
                });
            #[cfg(feature = "work-execution")]
            if let Some(work) = timer.work_deadline {
                deadline = deadline.min(work);
            }
            if let Some(focus) = timer.focus_deadline {
                deadline = deadline.min(focus);
            }
            if let Some((_, favicon, _)) = next_favicon {
                deadline = deadline.min(favicon);
            }
            if let Some((_, presentation)) = next_presentation {
                deadline = deadline.min(presentation.wake);
            }
            if let Some((_, discard, _)) = next_discard {
                deadline = deadline.min(discard);
            }
            if let Some((_, capacity)) = next_capacity {
                deadline = deadline.min(capacity);
            }
            if let Some((_, profile_deletion, _)) = next_profile_deletion {
                deadline = deadline.min(profile_deletion);
            }
            if let Some((_, blocker_preference, _)) = next_blocker_preference {
                deadline = deadline.min(blocker_preference);
            }
            if let Some((blocker_catalog, _, _)) = next_blocker_catalog {
                deadline = deadline.min(blocker_catalog);
            }
            if let Some((page_permission, _, _, _)) = next_page_permission {
                deadline = deadline.min(page_permission);
            }
            let timeout = deadline.saturating_duration_since(now);
            let (next, _) = self
                .inner
                .timer_ready
                .wait_timeout(timer, timeout)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            timer = next;
        }
    }

    #[cfg(test)]
    pub(crate) fn wait_for_tick(&self, interval: std::time::Duration) -> bool {
        matches!(
            self.wait_for_timer(std::time::Instant::now() + interval),
            TimerWake::Maintenance
        )
    }
}

/// Adds one command to the single ordered queue. High-frequency engine state
/// may replace an older value only inside the trailing engine-only burst. It
/// can never cross a UI command, crash/result event, or shutdown barrier, so
/// a newer callback cannot make an older state appear after navigation.
fn enqueue(
    commands: &mut VecDeque<Command>,
    command: Command,
    hard_capacity: usize,
    critical: bool,
) -> Result<(), Command> {
    // Pressure is current OS state, not a user operation. Retain exactly the
    // latest fact across FIFO boundaries, including recovery to Normal.
    if matches!(command, Command::SetMemoryPressure(_)) {
        if let Some(index) = commands
            .iter()
            .position(|queued| matches!(queued, Command::SetMemoryPressure(_)))
        {
            commands.remove(index);
            commands.push_back(command);
            return Ok(());
        }
    }
    // Work wakes carry no ordered state; the actual bounded slots are read
    // after every shell command. One wake suffices across user FIFO barriers.
    #[cfg(feature = "work-execution")]
    if matches!(command, Command::WorkWake)
        && commands
            .iter()
            .any(|queued| matches!(queued, Command::WorkWake))
    {
        return Ok(());
    }
    let coalesced_key = command_coalesced_key(&command);
    if let Some(key) = coalesced_key {
        let burst_start = commands
            .iter()
            .rposition(|queued| command_coalesced_key(queued).is_none())
            .map_or(0, |index| index + 1);
        if let Some(index) = (burst_start..commands.len())
            .rev()
            .find(|index| command_coalesced_key(&commands[*index]) == Some(key))
        {
            commands.remove(index);
            commands.push_back(command);
            return Ok(());
        }
    }
    if commands.len() >= hard_capacity {
        // At the absolute lifecycle ceiling, replace an older fact for the
        // same bounded native object and move the newest observation to the
        // back. This preserves its ordering relative to intervening commands
        // while preventing a page from crowding out a renderer-death or final
        // source update with repeated callbacks for one tab.
        if critical {
            if let Some(key) = recovery_key(&command) {
                if let Some(index) = commands
                    .iter()
                    .position(|queued| recovery_key(queued) == Some(key))
                {
                    commands.remove(index);
                    commands.push_back(command);
                    return Ok(());
                }
            }
        }
        // An operation already reported as accepted is immutable FIFO state.
        // Lifecycle facts may replace only bounded presentation facts; they
        // must never evict open/close/navigate/settings or another mutation.
        let index = commands.iter().position(|queued| {
            !command_is_critical(queued) && command_coalesced_key(queued).is_some()
        });
        let Some(index) = index else {
            return Err(command);
        };
        commands.remove(index);
    }
    commands.push_back(command);
    Ok(())
}

/// Native state transitions whose loss can leave Rust believing a destroyed
/// or failed view is still live. They receive reserved admission and may
/// displace older presentation facts at the absolute lifecycle ceiling.
fn command_is_critical(command: &Command) -> bool {
    #[cfg(feature = "work-execution")]
    if matches!(command, Command::WorkWake) {
        return true;
    }
    matches!(
        command,
        Command::SetWindowFocused(false)
            | Command::BlockerReady(_)
            | Command::SetMemoryPressure(_)
            | Command::SidebarResizeGuide(None)
            | Command::BlockerStoreReady(_)
            | Command::ProfileDeletionReady(_)
            | Command::PagePermissionCatalogLoaded { .. }
            | Command::PagePermissionCatalogMutated { .. }
            | Command::PagePermissionTimeout { .. }
            | Command::BrowserChromeRestored { .. }
            | Command::ChromePresentationApplied { .. }
            | Command::Engine(
                EngineEvent::NativeTabCloseRequested { .. }
                    | EngineEvent::NativeTabOpened { .. }
                    | EngineEvent::LinkedDownloadStarted { .. }
                    | EngineEvent::UrlChanged { .. }
                    | EngineEvent::PresentationPending { .. }
                    | EngineEvent::PresentationReady { .. }
                    | EngineEvent::RuntimeRestartRequired
                    | EngineEvent::ContentRulesSettled { .. }
                    | EngineEvent::UserContentSettled { .. }
                    | EngineEvent::ExtensionBrowserRequested { .. }
                    | EngineEvent::ExtensionCreatedTabReplied { .. }
                    | EngineEvent::ExtensionPageClosed { .. }
                    | EngineEvent::ExtensionPageChanged { .. }
                    | EngineEvent::PermissionRequested { .. }
                    | EngineEvent::MediaCaptureChanged { .. }
                    | EngineEvent::FullscreenChanged { .. }
                    | EngineEvent::ExtensionActionsInvalidated { .. }
                    | EngineEvent::NavigationFailed { .. }
                    | EngineEvent::ZoomSettled { .. }
                    | EngineEvent::NativeActionFailed { .. }
                    | EngineEvent::ViewCreationFailed { .. }
                    | EngineEvent::ProfileProcessExited { .. }
                    | EngineEvent::Crashed { .. }
                    | EngineEvent::ViewDiscarded { .. }
                    | EngineEvent::ViewDiscardRefused { .. }
                    | EngineEvent::SplitChanged { .. }
            )
    )
}

fn command_is_observational_query(command: &Command) -> bool {
    #[cfg(feature = "work-execution")]
    if matches!(command, Command::WorkProfileBinding { .. }) {
        return true;
    }
    matches!(
        command,
        Command::ContentPolicyStatus { .. }
            | Command::FocusedContentPolicyStatus { .. }
            | Command::BlockerStatistics { .. }
    )
}

fn command_coalesced_key(command: &Command) -> Option<CoalescedKey> {
    match command {
        #[cfg(feature = "work-execution")]
        Command::WorkWake => Some(CoalescedKey::Work),
        Command::Bootstrap => Some(CoalescedKey::Bootstrap),
        Command::Engine(event) => CoalescedKey::of(event),
        Command::SetWindowSize(_) => Some(CoalescedKey::WindowSize),
        Command::SetWindowVisible(_) => Some(CoalescedKey::WindowVisible),
        Command::SetMemoryPressure(_) => Some(CoalescedKey::MemoryPressure),
        Command::SetSidebarWidth(..) => Some(CoalescedKey::SidebarWidth),
        Command::SidebarResizeGuide(Some(_)) => Some(CoalescedKey::SidebarGuide),
        Command::SidebarResizeGuide(None) => Some(CoalescedKey::SidebarGuideEnd),
        Command::WorkPaneSetRect { .. } => Some(CoalescedKey::WorkPaneRect),
        Command::DragOver { .. } => Some(CoalescedKey::DragOver),
        Command::DividerDrag { .. } => Some(CoalescedKey::DividerDrag),
        Command::Search(_) | Command::SearchScoped { .. } => Some(CoalescedKey::Search),
        Command::FaviconPoll { id, .. } => Some(CoalescedKey::FaviconPoll(*id)),
        Command::PresentationFallback { id, .. } => Some(CoalescedKey::PresentationFallback(*id)),
        Command::ChromePresentationApplied { id, .. } => {
            Some(CoalescedKey::ChromePresentation(*id))
        }
        Command::DiscardProbeTimeout { id, .. } => Some(CoalescedKey::DiscardProbeTimeout(*id)),
        Command::ViewCapacityRetry(id) => Some(CoalescedKey::ViewCapacityRetry(*id)),
        Command::BlockerReady(profile) => Some(CoalescedKey::BlockerReady(*profile)),
        Command::BlockerStoreReady(profile) => Some(CoalescedKey::BlockerStoreReady(*profile)),
        Command::BlockerPreferenceRetry { profile, .. } => {
            Some(CoalescedKey::BlockerPreferenceRetry(*profile))
        }
        Command::BlockerCatalogPoll { .. } => Some(CoalescedKey::BlockerCatalogPoll),
        Command::ProfileDeletionReady(profile) => {
            Some(CoalescedKey::ProfileDeletionReady(*profile))
        }
        Command::ProfileDeletionRetry { profile, .. } => {
            Some(CoalescedKey::ProfileDeletionRetry(*profile))
        }
        Command::StoreRead(StoreReadResult::History { .. }) => Some(CoalescedKey::StoreHistory),
        Command::StoreRead(StoreReadResult::Favicon { id, .. }) => {
            Some(CoalescedKey::StoreFavicon(*id))
        }
        Command::StoreRead(StoreReadResult::FaviconBatch { .. }) => {
            Some(CoalescedKey::StoreFaviconBatch)
        }
        Command::Persist => Some(CoalescedKey::Persist),
        Command::FocusWake => Some(CoalescedKey::FocusWake),
        Command::Tick => Some(CoalescedKey::Tick),
        _ => None,
    }
}
