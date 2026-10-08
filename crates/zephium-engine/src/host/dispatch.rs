use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

#[cfg(any(target_os = "macos", target_os = "windows"))]
use raw_window_handle::RawWindowHandle;
use zephium_core::ids::ItemId;
use zephium_core::ids::ProfileId;
use zephium_core::ports::engine::{UserContent, UserContentGeneration};

use super::permits::Sink;
use super::resources::{NativeResourceLedger, MAX_NATIVE_VIEW_RESOURCES};
use super::EngineHost;
#[cfg(any(target_os = "macos", target_os = "windows"))]
use super::ParentHandle;

thread_local! {
    static HOST: RefCell<Option<EngineHost>> = const { RefCell::new(None) };
    static PENDING: RefCell<VecDeque<QueuedHostTask>> = const { RefCell::new(VecDeque::new()) };
    #[cfg(target_os = "macos")]
    static PENDING_EXTENSION_BROWSER_REQUEST_TERMINALS: Cell<ExtensionBrowserRequestTerminalSlots> =
        const { Cell::new(ExtensionBrowserRequestTerminalSlots::EMPTY) };
    #[cfg(target_os = "macos")]
    static EXTENSION_BROWSER_REQUEST_TERMINAL_INVARIANT_FAILED: Cell<bool> = const { Cell::new(false) };
    #[cfg(target_os = "macos")]
    static EXTENSION_BROWSER_REQUEST_TERMINAL_FAILURE_REPORTED: Cell<bool> = const { Cell::new(false) };
    #[cfg(target_os = "macos")]
    static PENDING_PAGE_PERMISSION_TERMINALS: Cell<PagePermissionTerminalSlots> =
        const { Cell::new(PagePermissionTerminalSlots::EMPTY) };
    #[cfg(target_os = "macos")]
    static PAGE_PERMISSION_TERMINAL_INVARIANT_FAILED: Cell<bool> = const { Cell::new(false) };
    #[cfg(target_os = "macos")]
    static PAGE_PERMISSION_TERMINAL_FAILURE_REPORTED: Cell<bool> = const { Cell::new(false) };
    #[cfg(not(target_os = "windows"))]
    static PENDING_CONTENT_POLICY_TERMINALS: Cell<ContentPolicyTerminalSlots> =
        const { Cell::new(ContentPolicyTerminalSlots::EMPTY) };
    static HOST_SEALED: Cell<bool> = const { Cell::new(false) };
    static HOST_INSTALLING: Cell<bool> = const { Cell::new(false) };
    #[cfg(target_os = "windows")]
    static PENDING_WINDOWS_CLEANUP_DEBTS: RefCell<Vec<(ProfileId, super::OwnedWindowsCleanupDebt)>> =
        const { RefCell::new(Vec::new()) };
    #[cfg(target_os = "windows")]
    static WINDOWS_CLEANUP_INVARIANT_FAILED: Cell<bool> = const { Cell::new(false) };
}

type HostTask = Box<dyn FnOnce(&mut EngineHost)>;

#[cfg(target_os = "macos")]
const EXTENSION_BROWSER_REQUEST_TERMINAL_CAPACITY: usize =
    2 * zephium_core::extensions::MAX_PENDING_EXTENSION_BROWSER_REQUESTS;

#[cfg(target_os = "macos")]
type ExtensionBrowserRequestTerminalSlots =
    ExactTerminalSlots<EXTENSION_BROWSER_REQUEST_TERMINAL_CAPACITY>;
#[cfg(target_os = "macos")]
const PAGE_PERMISSION_TERMINAL_CAPACITY: usize =
    3 * super::page_permissions::MAX_PENDING_PAGE_PERMISSION_REQUESTS;
#[cfg(target_os = "macos")]
type PagePermissionTerminalSlots = ExactTerminalSlots<PAGE_PERMISSION_TERMINAL_CAPACITY>;

/// Inline, noncoalescing terminal ring.
///
/// Native callbacks and their independently scheduled cancellation barriers
/// can contribute at most two exact tasks per logical reservation. The ring
/// never allocates, grows, replaces, or silently drops an accepted terminal.
#[cfg(target_os = "macos")]
struct ExactTerminalSlots<const CAPACITY: usize> {
    slots: [Option<HostTask>; CAPACITY],
    head: usize,
    len: usize,
    // The normal capacity is proven from the logical reservation ceiling. If
    // that proof is ever violated, retain the first rejected owner-bearing
    // closure in one fixed fail-stop slot instead of running its destructor.
    overflow_quarantine: Option<HostTask>,
    // Number of ring tasks that were already ahead of the quarantined task.
    // Reentrant tasks appended after overflow do not increment this fence.
    overflow_predecessors: usize,
}

#[cfg(target_os = "macos")]
impl<const CAPACITY: usize> ExactTerminalSlots<CAPACITY> {
    const EMPTY: Self = Self {
        slots: [const { None }; CAPACITY],
        head: 0,
        len: 0,
        overflow_quarantine: None,
        overflow_predecessors: 0,
    };

    fn push_back(&mut self, task: HostTask) -> Result<(), HostTask> {
        if self.len >= CAPACITY {
            return Err(task);
        }
        let index = (self.head + self.len) % CAPACITY;
        debug_assert!(self.slots[index].is_none());
        self.slots[index] = Some(task);
        self.len += 1;
        Ok(())
    }

    fn pop_front(&mut self) -> Option<HostTask> {
        if self.overflow_quarantine.is_some() && self.overflow_predecessors == 0 {
            return self.overflow_quarantine.take();
        }
        if self.len == 0 {
            debug_assert!(self.overflow_quarantine.is_none());
            return None;
        }
        let index = self.head;
        let task = self.slots[index].take();
        self.head = (self.head + 1) % CAPACITY;
        self.len -= 1;
        if self.overflow_quarantine.is_some() {
            debug_assert!(self.overflow_predecessors > 0);
            self.overflow_predecessors = self.overflow_predecessors.saturating_sub(1);
        }
        debug_assert!(task.is_some());
        task
    }

    fn quarantine_overflow(&mut self, task: HostTask) -> Result<(), HostTask> {
        if self.overflow_quarantine.is_some() {
            return Err(task);
        }
        self.overflow_predecessors = self.len;
        self.overflow_quarantine = Some(task);
        Ok(())
    }
}

#[cfg(target_os = "macos")]
impl<const CAPACITY: usize> Default for ExactTerminalSlots<CAPACITY> {
    fn default() -> Self {
        Self::EMPTY
    }
}

struct HostInstallClaim(std::marker::PhantomData<std::rc::Rc<()>>);

impl HostInstallClaim {
    fn acquire() -> Result<Self, String> {
        let claimed = HOST_INSTALLING.with(|installing| !installing.replace(true));
        if !claimed {
            return Err("engine host installation is already active on this thread".to_owned());
        }
        let claim = Self(std::marker::PhantomData);
        HOST.with(|cell| {
            let host = cell
                .try_borrow()
                .map_err(|_| "engine host is re-entrantly borrowed during install".to_owned())?;
            if host.is_some() {
                return Err("engine host is already installed on this thread".to_owned());
            }
            Ok(())
        })?;
        Ok(claim)
    }
}

impl Drop for HostInstallClaim {
    fn drop(&mut self) {
        HOST_INSTALLING.with(|installing| installing.set(false));
    }
}

#[cfg(not(target_os = "windows"))]
#[derive(Default)]
struct ContentPolicyTerminalSlots {
    first: Option<HostTask>,
    second: Option<HostTask>,
}

#[cfg(not(target_os = "windows"))]
impl ContentPolicyTerminalSlots {
    const EMPTY: Self = Self {
        first: None,
        second: None,
    };

    fn push_back(&mut self, task: HostTask) -> Result<(), HostTask> {
        if self.first.is_none() {
            self.first = Some(task);
            Ok(())
        } else if self.second.is_none() {
            self.second = Some(task);
            Ok(())
        } else {
            Err(task)
        }
    }

    fn pop_front(&mut self) -> Option<HostTask> {
        let first = self.first.take();
        self.first = self.second.take();
        first
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        usize::from(self.first.is_some()) + usize::from(self.second.is_some())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum HostTaskPriority {
    Normal,
    Observation,
    Lifecycle,
    Close,
    #[cfg(feature = "agentic-browser")]
    // Exact agent-context lifecycle and audit tasks own an independent fixed
    // band and are never coalesced or replaced.
    AgentContext,
    #[cfg(all(
        feature = "agentic-browser",
        any(target_os = "macos", target_os = "windows")
    ))]
    // A commit/timeout winner or one renderer-loss callback rejoins an owned
    // context. Neither can compete with new request ingress for capacity.
    AgentContextTerminal,
    ProfileErasure,
    Shutdown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HostTaskKey {
    // A renderer death supersedes a pending source observation for the same
    // view; an explicit close supersedes both. Priority prevents a later
    // lower-level callback from replacing the stronger transition.
    View(ItemId),
    // Replaceable source/history facts must never overwrite an adjacent
    // terminal navigation settlement or discard-safety phase for the same
    // view merely because they share an ItemId.
    Source(ItemId),
    DocumentStyle(ItemId),
    GenericStyle(ItemId),
    Title(ItemId),
    NavigationCommit(ItemId),
    NavigationSettlement(ItemId),
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    Fullscreen(ItemId),
    Discard(ItemId),
    #[cfg_attr(not(any(target_os = "macos", target_os = "windows")), allow(dead_code))]
    DiscardTerminal(ItemId),
    DiscardState(ProfileId, Option<ItemId>),
    #[cfg(target_os = "windows")]
    Profile(ProfileId, crate::platform::imp::BrowserProcessGeneration),
    #[cfg(target_os = "windows")]
    Suspend(ItemId),
    // Separate from `Suspend`: replacing a preflight or completion with its
    // deadline (or the reverse) would leave the attempt's guard unsettled.
    #[cfg(target_os = "windows")]
    SuspendDeadline(ItemId),
    #[cfg(target_os = "windows")]
    Extension(zephium_core::extensions::ExtensionRuntimeInstance, bool),
}

struct QueuedHostTask {
    priority: HostTaskPriority,
    key: Option<HostTaskKey>,
    task: HostTask,
}

const NORMAL_PENDING_HOST_TASK_CAPACITY: usize = 960;
// Normal UI work cannot consume this lifecycle band. One keyed native fact
// per maximum view/profile plus the bounded suspend batch stays below this
// ceiling, even during a nested native message-loop pump. One slot per
// maximum live profile is reserved for noncoalescible erasure tasks, and the
// final physical slot is reserved exclusively for shutdown. The single
// WebKit compiler/cache-maintenance slot's two exact physical callbacks have
// their own fixed FIFO.
const PENDING_HOST_TASK_CAPACITY: usize = 4096;
const NON_SHUTDOWN_PENDING_HOST_TASK_CAPACITY: usize = PENDING_HOST_TASK_CAPACITY - 1;
const PROFILE_ERASURE_PENDING_HOST_TASK_CAPACITY: usize =
    zephium_core::session::MAX_SESSION_PROFILES;
const NON_PROFILE_ERASURE_PENDING_HOST_TASK_CAPACITY: usize =
    NON_SHUTDOWN_PENDING_HOST_TASK_CAPACITY - PROFILE_ERASURE_PENDING_HOST_TASK_CAPACITY;
#[cfg(all(
    feature = "agentic-browser",
    any(target_os = "macos", target_os = "windows")
))]
const AGENT_CONTEXT_TERMINAL_PENDING_HOST_TASK_CAPACITY: usize =
    2 * zephium_agentic::MAX_LIVE_CONTEXTS;
#[cfg(all(
    feature = "agentic-browser",
    any(target_os = "macos", target_os = "windows")
))]
const NON_AGENT_CONTEXT_TERMINAL_PENDING_HOST_TASK_CAPACITY: usize =
    NON_PROFILE_ERASURE_PENDING_HOST_TASK_CAPACITY
        - AGENT_CONTEXT_TERMINAL_PENDING_HOST_TASK_CAPACITY;
#[cfg(all(
    feature = "agentic-browser",
    not(any(target_os = "macos", target_os = "windows"))
))]
const NON_AGENT_CONTEXT_TERMINAL_PENDING_HOST_TASK_CAPACITY: usize =
    NON_PROFILE_ERASURE_PENDING_HOST_TASK_CAPACITY;
#[cfg(feature = "agentic-browser")]
const AGENT_CONTEXT_PENDING_HOST_TASK_CAPACITY: usize =
    zephium_agentic::MAX_PENDING_NATIVE_CONTEXT_TASKS;
#[cfg(feature = "agentic-browser")]
const NON_AGENT_CONTEXT_PENDING_HOST_TASK_CAPACITY: usize =
    NON_AGENT_CONTEXT_TERMINAL_PENDING_HOST_TASK_CAPACITY
        - AGENT_CONTEXT_PENDING_HOST_TASK_CAPACITY;
#[cfg(not(feature = "agentic-browser"))]
const NON_AGENT_CONTEXT_PENDING_HOST_TASK_CAPACITY: usize =
    NON_PROFILE_ERASURE_PENDING_HOST_TASK_CAPACITY;
// One globally coalesced commit gate per native view remains admissible even
// if ordinary observations/lifecycle work fill their band. The native view
// resource ceiling proves no more distinct live commit keys can exist while
// the host is re-entrantly borrowed.
const NAVIGATION_COMMIT_PENDING_HOST_TASK_CAPACITY: usize = MAX_NATIVE_VIEW_RESOURCES;
const NON_NAVIGATION_COMMIT_PENDING_HOST_TASK_CAPACITY: usize =
    NON_AGENT_CONTEXT_PENDING_HOST_TASK_CAPACITY - NAVIGATION_COMMIT_PENDING_HOST_TASK_CAPACITY;
static PENDING_OVERFLOW_LOGS_REMAINING: AtomicUsize = AtomicUsize::new(4);

#[allow(
    clippy::too_many_arguments,
    reason = "startup injects distinct native ownership and terminal-failure capabilities"
)]
pub(crate) fn install(
    #[cfg(any(target_os = "macos", target_os = "windows"))] parent: RawWindowHandle,
    data_root: PathBuf,
    main_dispatch: crate::MainThreadDispatch,
    initial_user_content_generation: UserContentGeneration,
    initial_user_content: UserContent,
    #[cfg(any(target_os = "macos", target_os = "windows"))] native_open_authority: Arc<
        crate::NativeOpenAuthority,
    >,
    sink: crate::EngineEventIngressSink,
    native_terminal_failure: Arc<dyn Fn(&'static str) + Send + Sync>,
) -> Result<(), String> {
    let _install_claim = HostInstallClaim::acquire()?;
    let user_content = super::scripts::UserContentRegistry::with_initial_global(
        initial_user_content_generation,
        initial_user_content,
    )
    .map_err(|failure| format!("invalid initial user-content generation: {failure:?}"))?;
    // WebView2 needs a user-data folder even for InPrivate controllers. It
    // must never be the privileged Tauri chrome's folder, and stale runtime
    // metadata must not accumulate across browser sessions.
    #[cfg(not(target_os = "macos"))]
    let profiles_root = crate::erasure::canonical_owned_root(&data_root.join("profiles"))
        .map_err(|error| format!("cannot secure engine profile root: {error}"))?;
    #[cfg(not(target_os = "windows"))]
    let content_rule_cache = crate::erasure::canonical_owned_root(&data_root.join("content-rules"))
        .map_err(|error| format!("cannot secure content-rule cache: {error}"))?;
    #[cfg(target_os = "windows")]
    let private_runtime = zephium_core::webview2::RuntimeGeneration::prepare(
        &data_root.join("private-runtime"),
        zephium_core::webview2::RuntimeGenerationKind::RawPrivate,
    )
    .map_err(|error| format!("cannot create private WebView2 generation: {error}"))?;
    HOST_SEALED.with(|sealed| sealed.set(false));
    PENDING.with(|pending| {
        pending
            .try_borrow_mut()
            .map_err(|_| "engine host pending queue is re-entrantly borrowed".to_owned())?
            .clear();
        Ok::<(), String>(())
    })?;
    #[cfg(target_os = "macos")]
    PENDING_EXTENSION_BROWSER_REQUEST_TERMINALS.with(|pending| {
        drop(pending.replace(ExtensionBrowserRequestTerminalSlots::EMPTY));
    });
    #[cfg(target_os = "macos")]
    EXTENSION_BROWSER_REQUEST_TERMINAL_INVARIANT_FAILED.with(|failed| failed.set(false));
    #[cfg(target_os = "macos")]
    EXTENSION_BROWSER_REQUEST_TERMINAL_FAILURE_REPORTED.with(|reported| reported.set(false));
    #[cfg(target_os = "macos")]
    PENDING_PAGE_PERMISSION_TERMINALS.with(|pending| {
        drop(pending.replace(PagePermissionTerminalSlots::EMPTY));
    });
    #[cfg(target_os = "macos")]
    PAGE_PERMISSION_TERMINAL_INVARIANT_FAILED.with(|failed| failed.set(false));
    #[cfg(target_os = "macos")]
    PAGE_PERMISSION_TERMINAL_FAILURE_REPORTED.with(|reported| reported.set(false));
    #[cfg(not(target_os = "windows"))]
    PENDING_CONTENT_POLICY_TERMINALS.with(|pending| {
        drop(pending.replace(ContentPolicyTerminalSlots::EMPTY));
    });
    HOST.with(|cell| {
        let mut host = cell
            .try_borrow_mut()
            .map_err(|_| "engine host is re-entrantly borrowed during install".to_owned())?;
        debug_assert!(host.is_none(), "the install claim proves host vacancy");
        *host = Some(EngineHost {
            #[cfg(any(target_os = "macos", target_os = "windows"))]
            parent: ParentHandle(parent),
            #[cfg(not(target_os = "macos"))]
            profiles_root,
            #[cfg(not(target_os = "windows"))]
            content_rule_cache,
            #[cfg(target_os = "windows")]
            private_runtime,
            views: HashMap::new(),
            #[cfg(all(
                feature = "agentic-browser",
                any(target_os = "macos", target_os = "windows")
            ))]
            agent_contexts: HashMap::new(),
            #[cfg(all(feature = "agentic-browser", any(target_os = "macos", target_os = "windows")))]
            work_resources: HashMap::new(),
            #[cfg(all(feature = "agentic-browser", target_os = "windows"))]
            agent_cookie_transfers: HashMap::new(),
            #[cfg(all(feature = "agentic-browser", target_os = "windows"))]
            agent_cookie_quarantined_profiles: HashSet::new(),
            native_resources: NativeResourceLedger::default(),
            extension_browser_surfaces: HashMap::new(),
            #[cfg(target_os = "macos")]
            page_permissions: super::page_permissions::PagePermissionBroker::default(),
            #[cfg(any(target_os = "macos", target_os = "windows"))]
            downloads: None,
            #[cfg(any(target_os="macos",target_os="windows"))]
            native_open_authority,
            native_resource_accounting_failed: false,
            navigation_snapshots: HashMap::new(),
            partitions: HashMap::new(),
            profile_persistence_classes: HashMap::new(),
            content_policies: HashMap::new(),
            style_worker: None,
            main_dispatch,
            #[cfg(not(target_os = "windows"))]
            content_rule_preflight: None,
            #[cfg(not(target_os = "windows"))]
            preflight_cache_digests: Default::default(),
            blocker_statistics: HashMap::new(),
            focus_gate: Default::default(),
            blocker_sites: HashMap::new(),
            picker: None,
            next_picker: 0,
            declarative_content_policy_cache: HashMap::new(),
            declarative_content_policy_compilations: HashMap::new(),
            #[cfg(not(target_os = "windows"))]
            declarative_content_policy_queue: VecDeque::new(),
            #[cfg(not(target_os = "windows"))]
            active_declarative_content_policy_maintenance: None,
            #[cfg(not(target_os = "windows"))]
            declarative_content_policy_bytes: 0,
            #[cfg(not(target_os = "windows"))]
            next_declarative_content_policy_maintenance_attempt: 1,
            #[cfg(not(target_os = "windows"))]
            // Persistent native-cache maintenance is requested by an actual
            // policy settlement. Process startup alone does not yet know the
            // initial profile cohort's protected digests.
            content_rule_cache_gc_pending: false,
            #[cfg(not(target_os = "windows"))]
            content_rule_cache_gc_cursor: 0,
            #[cfg(not(target_os = "windows"))]
            content_rule_cache_gc_removed_in_cycle: false,
            spare: None,
            memory_pressure: zephium_core::ports::engine::MemoryPressure::Normal,
            #[cfg(target_os = "macos")]
            discarded_states: HashMap::new(),
            #[cfg(target_os = "macos")]
            prepared_discard_states: HashMap::new(),
            #[cfg(target_os = "macos")]
            restore_snapshots: HashMap::new(),
            #[cfg(target_os = "macos")]
            discarded_state_revision: 0,
            user_content,
            shortcuts: Arc::default(),
            #[cfg(any(target_os = "macos", target_os = "windows"))]
            fullscreen: crate::fullscreen::FullscreenLedger::default(),
            #[cfg(target_os = "macos")]
            fullscreen_retiring: HashMap::new(),
            #[cfg(target_os = "macos")]
            next_fullscreen_retirement: 0,
            stages: HashMap::new(),
            native_terminal_failure,
            #[cfg(not(target_os = "windows"))]
            shutdown_completion: None,
            #[cfg(target_os = "macos")]
            macos_ephemeral_data_stores: HashMap::new(),
            #[cfg(all(target_os = "macos", feature = "agentic-browser"))]
            anonymous_work_stores: HashMap::new(),
            #[cfg(any(target_os = "macos", target_os = "windows"))]
            work_site_loads: HashMap::new(),
            #[cfg(target_os = "macos")]
            webext: Default::default(),
            #[cfg(target_os = "windows")]
            windows_extensions: Default::default(),
            #[cfg(target_os = "windows")]
            hidden: std::collections::HashSet::new(),
            #[cfg(any(target_os = "windows", target_os = "macos"))]
            dormant: std::collections::HashSet::new(),
            #[cfg(target_os = "windows")]
            desired_dormant: std::collections::HashSet::new(),
            #[cfg(target_os = "windows")]
            suspending: std::collections::HashSet::new(),
            #[cfg(target_os = "windows")]
            suspend_failed: std::collections::HashSet::new(),
            #[cfg(target_os = "windows")]
            suspend_uncertain: std::collections::HashSet::new(),
            #[cfg(any(target_os = "windows", target_os = "macos"))]
            styles_missed: std::collections::HashSet::new(),
            #[cfg(not(target_os = "macos"))]
            web_contexts: HashMap::new(),
            #[cfg(all(unix, not(target_os = "macos")))]
            linux_data_managers: HashMap::new(),
            #[cfg(all(unix, not(target_os = "macos")))]
            linux_unverifiable_data_managers: HashSet::new(),
            #[cfg(target_os = "windows")]
            browser_version_observers: HashMap::new(),
            #[cfg(target_os = "windows")]
            environments: HashMap::new(),
            #[cfg(all(target_os = "windows", feature = "agentic-browser"))]
            anonymous_work_environments: HashMap::new(),
            #[cfg(all(target_os = "windows", feature = "agentic-browser"))]
            work_site_stores: HashMap::new(),
            #[cfg(target_os = "windows")]
            browser_processes: HashMap::new(),
            #[cfg(target_os = "windows")]
            browser_process_exit_observers: HashMap::new(),
            #[cfg(target_os = "windows")]
            windows_process_shutdown_started: false,
            #[cfg(target_os = "windows")]
            pending_profile_recovery: HashMap::new(),
            #[cfg(target_os = "windows")]
            exiting_browser_processes: HashSet::new(),
            #[cfg(target_os = "windows")]
            unverifiable_browser_processes: HashSet::new(),
            #[cfg(target_os = "windows")]
            construction_unproven: HashSet::new(),
            #[cfg(target_os = "windows")]
            unproven_browser_processes: HashMap::new(),
            #[cfg(target_os = "windows")]
            unproven_environments: HashMap::new(),
            #[cfg(target_os = "windows")]
            windows_cleanup_debts: HashMap::new(),
            #[cfg(target_os = "windows")]
            windows_cleanup_invariant_failed: false,
            erasure_tombstones: HashSet::new(),
            erasure_attempts: HashMap::new(),
            sink: Sink::new(sink),
        });
        Ok(())
    })
}

// WebView2 construction pumps the Windows message loop. A nested main-thread
// dispatch must not re-enter the host, but dropping it can lose a close,
// navigation or security transition. Queue it and drain after the outer
// operation releases the mutable borrow.
/// Admit work whose loss is explicitly fail-safe and observed by a later
/// retry/timeout. Authoritative mutations must use `try_with` (or a stronger
/// priority-specific variant) and handle `false`.
pub(crate) fn best_effort_with<F>(f: F)
where
    F: FnOnce(&mut EngineHost) + 'static,
{
    let _ = with_priority(HostTaskPriority::Normal, None, f);
}

pub(crate) fn try_with<F>(f: F) -> bool
where
    F: FnOnce(&mut EngineHost) + 'static,
{
    with_priority(HostTaskPriority::Normal, None, f)
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
pub(super) fn try_open_native_tab(
    source: zephium_core::ids::ItemId,
    permit: &super::permits::EventPermit,
    activity: crate::navigation_epoch::NavigationActivity,
    url: &str,
    features: wry::NewWindowFeatures,
) -> wry::NewWindowResponse {
    if HOST_SEALED.with(Cell::get) || HOST_INSTALLING.with(Cell::get) {
        return wry::NewWindowResponse::Deny;
    }
    HOST.with(|host| {
        let Ok(mut slot) = host.try_borrow_mut() else {
            return wry::NewWindowResponse::Deny;
        };
        let Some(host) = slot.as_mut() else {
            return wry::NewWindowResponse::Deny;
        };
        host.open_native_tab(source, permit, activity, url, features)
    })
}

#[cfg(target_os = "windows")]
pub(super) fn try_open_windows_extension_tab(
    runtime: zephium_core::extensions::ExtensionRuntimeInstance,
    url: &str,
    features: wry::NewWindowFeatures,
) -> wry::NewWindowResponse {
    if HOST_SEALED.with(Cell::get) || HOST_INSTALLING.with(Cell::get) {
        return wry::NewWindowResponse::Deny;
    }
    HOST.with(|slot| {
        let Ok(mut slot) = slot.try_borrow_mut() else {
            return wry::NewWindowResponse::Deny;
        };
        slot.as_mut().map_or(wry::NewWindowResponse::Deny, |host| {
            host.open_windows_extension_tab(runtime, url, features)
        })
    })
}

#[cfg(target_os = "windows")]
pub(super) fn finish_windows_native_tab(child: zephium_core::ids::ItemId, attached: bool) -> bool {
    HOST.with(|slot| {
        let Ok(mut slot) = slot.try_borrow_mut() else {
            return false;
        };
        let Some(host) = slot.as_mut() else {
            return false;
        };
        host.finish_windows_native_tab(child, attached);
        true
    })
}

#[cfg(test)]
pub(crate) fn make_unavailable_for_test() {
    HOST_SEALED.with(|sealed| sealed.set(false));
    HOST_INSTALLING.with(|installing| installing.set(false));
    HOST.with(|host| *host.borrow_mut() = None);
    PENDING.with(|pending| pending.borrow_mut().clear());
    #[cfg(target_os = "macos")]
    PENDING_EXTENSION_BROWSER_REQUEST_TERMINALS.with(|pending| {
        drop(pending.replace(ExtensionBrowserRequestTerminalSlots::EMPTY));
    });
    #[cfg(target_os = "macos")]
    EXTENSION_BROWSER_REQUEST_TERMINAL_INVARIANT_FAILED.with(|failed| failed.set(false));
    #[cfg(target_os = "macos")]
    EXTENSION_BROWSER_REQUEST_TERMINAL_FAILURE_REPORTED.with(|reported| reported.set(false));
    #[cfg(target_os = "macos")]
    PENDING_PAGE_PERMISSION_TERMINALS.with(|pending| {
        drop(pending.replace(PagePermissionTerminalSlots::EMPTY));
    });
    #[cfg(target_os = "macos")]
    PAGE_PERMISSION_TERMINAL_INVARIANT_FAILED.with(|failed| failed.set(false));
    #[cfg(target_os = "macos")]
    PAGE_PERMISSION_TERMINAL_FAILURE_REPORTED.with(|reported| reported.set(false));
    #[cfg(not(target_os = "windows"))]
    PENDING_CONTENT_POLICY_TERMINALS.with(|pending| {
        drop(pending.replace(ContentPolicyTerminalSlots::EMPTY));
    });
}

#[cfg(target_os = "windows")]
pub(super) fn queue_windows_cleanup_debt(profile: ProfileId, debt: super::OwnedWindowsCleanupDebt) {
    PENDING_WINDOWS_CLEANUP_DEBTS.with(|pending| {
        let Ok(mut pending) = pending.try_borrow_mut() else {
            WINDOWS_CLEANUP_INVARIANT_FAILED.with(|failed| failed.set(true));
            std::mem::forget(debt);
            return;
        };
        if pending.len() >= super::MAX_WINDOWS_CLEANUP_DEBTS {
            WINDOWS_CLEANUP_INVARIANT_FAILED.with(|failed| failed.set(true));
            std::mem::forget(debt);
            return;
        }
        pending.push((profile, debt));
    });
}

#[cfg(target_os = "windows")]
pub(super) fn drain_windows_cleanup_debts() -> Vec<(ProfileId, super::OwnedWindowsCleanupDebt)> {
    PENDING_WINDOWS_CLEANUP_DEBTS.with(|pending| {
        let Ok(mut pending) = pending.try_borrow_mut() else {
            // Existing debts remain owned by the TLS queue. We cannot prove
            // which profile obligations were observed, so make the global
            // construction/erasure barrier sticky instead of panicking from
            // RefCell's dynamic borrow check.
            WINDOWS_CLEANUP_INVARIANT_FAILED.with(|failed| failed.set(true));
            return Vec::new();
        };
        std::mem::take(&mut *pending)
    })
}

#[cfg(target_os = "windows")]
pub(super) fn windows_cleanup_invariant_failed() -> bool {
    WINDOWS_CLEANUP_INVARIANT_FAILED.with(Cell::get)
}

/// Profile retirement owns a dedicated bounded band above every ordinary and
/// native-lifecycle task. It intentionally has no coalescing key: replacing a
/// duplicate would drop that request's exactly-once completion obligation.
pub(crate) fn try_with_profile_erasure<F>(f: F) -> bool
where
    F: FnOnce(&mut EngineHost) + 'static,
{
    with_priority(HostTaskPriority::ProfileErasure, None, f)
}

/// Admit one exact production agent-context lifecycle or resource-audit task.
///
/// Its fixed noncoalescing band is independently capped by the public native
/// port ceiling. Profile erasure and shutdown retain higher-priority space.
#[cfg(feature = "agentic-browser")]
pub(crate) fn try_with_agent_context<F>(f: F) -> bool
where
    F: FnOnce(&mut EngineHost) + 'static,
{
    with_priority(HostTaskPriority::AgentContext, None, f)
}

/// Admit one exact terminal for an already-owned agent-context operation.
#[cfg(all(
    feature = "agentic-browser",
    any(target_os = "macos", target_os = "windows")
))]
pub(crate) fn try_with_agent_context_terminal<F>(f: F) -> bool
where
    F: FnOnce(&mut EngineHost) + 'static,
{
    with_priority(HostTaskPriority::AgentContextTerminal, None, f)
}

/// Exact queued terminal depth used only by the privacy-preserving resource
/// audit while the host is already running on its owner thread.
#[cfg(all(
    feature = "agentic-browser",
    any(target_os = "macos", target_os = "windows")
))]
pub(crate) fn agent_context_terminal_depth_for_audit() -> Option<usize> {
    PENDING.with(|pending| {
        pending.try_borrow().ok().map(|pending| {
            pending
                .iter()
                .filter(|task| task.priority == HostTaskPriority::AgentContextTerminal)
                .count()
        })
    })
}

#[cfg(all(
    feature = "agentic-browser",
    not(any(target_os = "macos", target_os = "windows"))
))]
pub(crate) const fn agent_context_terminal_depth_for_audit() -> Option<usize> {
    Some(0)
}

/// Admit a Shell settlement or watchdog for an already-retained native
/// browser-request or compatibility-broker completion. The fixed FIFO is
/// sized from both global request ceilings, so WebKit re-entry cannot crowd
/// it out with renderer work.
#[cfg(target_os = "macos")]
pub(crate) fn with_extension_browser_request_terminal<F>(f: F) -> bool
where
    F: FnOnce(&mut EngineHost) + 'static,
{
    let task: HostTask = Box::new(f);
    enum Admission {
        Accepted,
        Quarantined,
        Exhausted,
    }
    let admission = PENDING_EXTENSION_BROWSER_REQUEST_TERMINALS.with(|pending| {
        let mut slots = pending.take();
        let admission = match slots.push_back(task) {
            Ok(()) => Admission::Accepted,
            Err(task) => match slots.quarantine_overflow(task) {
                Ok(()) => Admission::Quarantined,
                Err(_task) => Admission::Exhausted,
            },
        };
        pending.set(slots);
        admission
    });
    match admission {
        Admission::Accepted => drain_extension_browser_request_terminals(),
        Admission::Quarantined | Admission::Exhausted => {
            EXTENSION_BROWSER_REQUEST_TERMINAL_INVARIANT_FAILED.with(|failed| failed.set(true));
            HOST_SEALED.with(|sealed| sealed.set(true));
            let _ = drain_extension_browser_request_terminals();
            false
        }
    }
}

#[cfg(target_os = "macos")]
fn drain_extension_browser_request_terminals() -> bool {
    enum Drain {
        Complete(bool),
        Deferred,
        Unavailable,
    }
    let drain = HOST.with(|cell| {
        let Ok(mut slot) = cell.try_borrow_mut() else {
            return Drain::Deferred;
        };
        let Some(host) = slot.as_mut() else {
            return Drain::Unavailable;
        };
        Drain::Complete(drain_extension_browser_request_terminals_with_host(host))
    });
    match drain {
        Drain::Complete(clean) => clean,
        Drain::Deferred => true,
        Drain::Unavailable => false,
    }
}

#[cfg(target_os = "macos")]
fn drain_extension_browser_request_terminals_with_host(host: &mut EngineHost) -> bool {
    loop {
        let task = PENDING_EXTENSION_BROWSER_REQUEST_TERMINALS.with(|pending| {
            let mut slots = pending.take();
            let task = slots.pop_front();
            pending.set(slots);
            task
        });
        let Some(task) = task else {
            break;
        };
        task(host);
    }
    if !EXTENSION_BROWSER_REQUEST_TERMINAL_INVARIANT_FAILED.with(Cell::get) {
        return true;
    }
    let report = EXTENSION_BROWSER_REQUEST_TERMINAL_FAILURE_REPORTED
        .with(|reported| !reported.replace(true));
    if report {
        (host.native_terminal_failure)(
            "extension browser request terminals exceeded their proven exact capacity",
        );
    }
    false
}

/// Admits Shell settlement, navigation revocation, and watchdog work for the
/// fixed page-permission cohort. This channel is independent from extension
/// delegates and ordinary renderer ingress: no unrelated callback can strand
/// a retained WebKit permission completion.
#[cfg(target_os = "macos")]
pub(crate) fn with_page_permission_terminal<F>(f: F) -> bool
where
    F: FnOnce(&mut EngineHost) + 'static,
{
    let task: HostTask = Box::new(f);
    enum Admission {
        Accepted,
        Quarantined,
        Exhausted,
    }
    let admission = PENDING_PAGE_PERMISSION_TERMINALS.with(|pending| {
        let mut slots = pending.take();
        let admission = match slots.push_back(task) {
            Ok(()) => Admission::Accepted,
            Err(task) => match slots.quarantine_overflow(task) {
                Ok(()) => Admission::Quarantined,
                Err(_task) => Admission::Exhausted,
            },
        };
        pending.set(slots);
        admission
    });
    match admission {
        Admission::Accepted => drain_page_permission_terminals(),
        Admission::Quarantined | Admission::Exhausted => {
            PAGE_PERMISSION_TERMINAL_INVARIANT_FAILED.with(|failed| failed.set(true));
            HOST_SEALED.with(|sealed| sealed.set(true));
            let _ = drain_page_permission_terminals();
            false
        }
    }
}

#[cfg(target_os = "macos")]
fn drain_page_permission_terminals() -> bool {
    enum Drain {
        Complete(bool),
        Deferred,
        Unavailable,
    }
    let drain = HOST.with(|cell| {
        let Ok(mut slot) = cell.try_borrow_mut() else {
            return Drain::Deferred;
        };
        let Some(host) = slot.as_mut() else {
            return Drain::Unavailable;
        };
        Drain::Complete(drain_page_permission_terminals_with_host(host))
    });
    match drain {
        Drain::Complete(clean) => clean,
        Drain::Deferred => true,
        Drain::Unavailable => false,
    }
}

#[cfg(target_os = "macos")]
fn drain_page_permission_terminals_with_host(host: &mut EngineHost) -> bool {
    loop {
        let task = PENDING_PAGE_PERMISSION_TERMINALS.with(|pending| {
            let mut slots = pending.take();
            let task = slots.pop_front();
            pending.set(slots);
            task
        });
        let Some(task) = task else {
            break;
        };
        task(host);
    }
    if !PAGE_PERMISSION_TERMINAL_INVARIANT_FAILED.with(Cell::get) {
        return true;
    }
    let report = PAGE_PERMISSION_TERMINAL_FAILURE_REPORTED.with(|reported| !reported.replace(true));
    if report {
        (host.native_terminal_failure)(
            "page permission terminals exceeded their proven exact capacity",
        );
    }
    false
}

pub(crate) fn try_with_close<F>(id: ItemId, f: F) -> bool
where
    F: FnOnce(&mut EngineHost) + 'static,
{
    with_priority(HostTaskPriority::Close, Some(HostTaskKey::View(id)), f)
}

pub(super) fn with_source_observation<F>(id: ItemId, f: F)
where
    F: FnOnce(&mut EngineHost) + 'static,
{
    let _ = with_priority(
        HostTaskPriority::Observation,
        Some(HostTaskKey::Source(id)),
        f,
    );
}

pub(super) fn with_document_style<F>(id: ItemId, f: F) -> bool
where
    F: FnOnce(&mut EngineHost) + 'static,
{
    with_priority(
        HostTaskPriority::Observation,
        Some(HostTaskKey::DocumentStyle(id)),
        f,
    )
}

pub(super) fn with_generic_style<F>(id: ItemId, f: F) -> bool
where
    F: FnOnce(&mut EngineHost) + 'static,
{
    with_priority(
        HostTaskPriority::Observation,
        Some(HostTaskKey::GenericStyle(id)),
        f,
    )
}

pub(super) fn with_title_observation<F>(id: ItemId, f: F)
where
    F: FnOnce(&mut EngineHost) + 'static,
{
    let _ = with_priority(
        HostTaskPriority::Observation,
        Some(HostTaskKey::Title(id)),
        f,
    );
}

pub(super) fn with_navigation_commit<F>(id: ItemId, f: F) -> bool
where
    F: FnOnce(&mut EngineHost) + 'static,
{
    with_priority(
        HostTaskPriority::Lifecycle,
        Some(HostTaskKey::NavigationCommit(id)),
        f,
    )
}

/// Each callback re-reads the view's native state when it runs, so only the
/// newest of adjacent callbacks for one view needs to survive.
#[cfg(any(target_os = "macos", target_os = "windows"))]
pub(super) fn with_fullscreen_observation<F>(id: ItemId, f: F)
where
    F: FnOnce(&mut EngineHost) + 'static,
{
    let _ = with_priority(
        HostTaskPriority::Lifecycle,
        Some(HostTaskKey::Fullscreen(id)),
        f,
    );
}

/// Native declarative compilation/cache maintenance is asynchronous and may
/// complete while WebKit has re-entered the host. The one physical slot owns
/// a dedicated two-entry FIFO for its timeout and terminal callback. Neither
/// ordinary queue saturation nor shutdown sealing may discard these debts.
#[cfg(not(target_os = "windows"))]
pub(super) fn with_content_policy_settlement<F>(_digest: [u8; 32], f: F) -> bool
where
    F: FnOnce(&mut EngineHost) + 'static,
{
    admit_content_policy_terminal_debt(Box::new(f))
}

/// The one physical WebKit maintenance watchdog must not coalesce with its
/// completion. If both arrive during native re-entry, FIFO order decides
/// whether the completion or deadline wins, and the exact attempt token makes
/// the losing task a no-op.
#[cfg(not(target_os = "windows"))]
pub(super) fn with_content_policy_timeout<F>(f: F) -> bool
where
    F: FnOnce(&mut EngineHost) + 'static,
{
    admit_content_policy_terminal_debt(Box::new(f))
}

#[cfg(not(target_os = "windows"))]
fn admit_content_policy_terminal_debt(task: HostTask) -> bool {
    let queued = PENDING_CONTENT_POLICY_TERMINALS.with(|pending| {
        let mut slots = pending.take();
        let queued = slots.push_back(task);
        pending.set(slots);
        queued.is_ok()
    });
    if !queued {
        return false;
    }
    drain_content_policy_terminal_debts()
}

#[cfg(not(target_os = "windows"))]
fn drain_content_policy_terminal_debts() -> bool {
    enum Drain {
        Executed,
        Empty,
        Deferred,
        Unavailable,
    }

    loop {
        let drained = HOST.with(|cell| {
            let Ok(mut slot) = cell.try_borrow_mut() else {
                return Drain::Deferred;
            };
            let Some(host) = slot.as_mut() else {
                return Drain::Unavailable;
            };
            let task = PENDING_CONTENT_POLICY_TERMINALS.with(|pending| {
                let mut slots = pending.take();
                let task = slots.pop_front();
                pending.set(slots);
                task
            });
            let Some(task) = task else {
                return Drain::Empty;
            };
            task(host);
            Drain::Executed
        });
        match drained {
            Drain::Executed => {}
            Drain::Empty | Drain::Deferred => return true,
            Drain::Unavailable => return false,
        }
    }
}

pub(super) fn with_navigation_settlement<F>(id: ItemId, f: F)
where
    F: FnOnce(&mut EngineHost) + 'static,
{
    let _ = with_priority(
        HostTaskPriority::Lifecycle,
        Some(HostTaskKey::NavigationSettlement(id)),
        f,
    );
}

pub(super) fn with_discard_observation<F>(id: ItemId, f: F)
where
    F: FnOnce(&mut EngineHost) + 'static,
{
    let _ = with_priority(
        HostTaskPriority::Observation,
        Some(HostTaskKey::Discard(id)),
        f,
    );
}

#[cfg_attr(not(any(target_os = "macos", target_os = "windows")), allow(dead_code))]
pub(crate) fn with_discard_terminal<F>(id: ItemId, f: F) -> bool
where
    F: FnOnce(&mut EngineHost) + 'static,
{
    with_priority(
        HostTaskPriority::Lifecycle,
        Some(HostTaskKey::DiscardTerminal(id)),
        f,
    )
}

pub(crate) fn try_with_discard_state_erasure<F>(
    profile: ProfileId,
    item: Option<ItemId>,
    f: F,
) -> bool
where
    F: FnOnce(&mut EngineHost) + 'static,
{
    with_priority(
        HostTaskPriority::Lifecycle,
        Some(HostTaskKey::DiscardState(profile, item)),
        f,
    )
}

pub(super) fn with_renderer_exit<F>(id: ItemId, f: F) -> bool
where
    F: FnOnce(&mut EngineHost) + 'static,
{
    with_priority(HostTaskPriority::Lifecycle, Some(HostTaskKey::View(id)), f)
}

#[cfg(target_os = "windows")]
pub(super) fn with_extension_lifecycle<F>(
    runtime: zephium_core::extensions::ExtensionRuntimeInstance,
    popup: bool,
    f: F,
) where
    F: FnOnce(&mut EngineHost) + 'static,
{
    let _ = with_priority(
        HostTaskPriority::Lifecycle,
        Some(HostTaskKey::Extension(runtime, popup)),
        f,
    );
}

#[cfg(target_os = "windows")]
pub(super) fn with_profile_exit<F>(
    profile: ProfileId,
    generation: crate::platform::imp::BrowserProcessGeneration,
    f: F,
) where
    F: FnOnce(&mut EngineHost) + 'static,
{
    let _ = with_priority(
        HostTaskPriority::Lifecycle,
        Some(HostTaskKey::Profile(profile, generation)),
        f,
    );
}

#[cfg(target_os = "windows")]
pub(super) fn with_suspend_result<F>(id: ItemId, f: F) -> bool
where
    F: FnOnce(&mut EngineHost) + 'static,
{
    with_priority(
        HostTaskPriority::Lifecycle,
        Some(HostTaskKey::Suspend(id)),
        f,
    )
}

#[cfg(target_os = "windows")]
pub(super) fn with_suspend_deadline<F>(id: ItemId, f: F) -> bool
where
    F: FnOnce(&mut EngineHost) + 'static,
{
    with_priority(
        HostTaskPriority::Lifecycle,
        Some(HostTaskKey::SuspendDeadline(id)),
        f,
    )
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
pub(super) fn seal_ingress() {
    HOST_SEALED.with(|sealed| sealed.set(true));
}

#[cfg(target_os = "macos")]
pub(super) fn try_with_stage_failure<F>(f: F) -> bool
where
    F: FnOnce(&mut EngineHost) + 'static,
{
    with_priority(HostTaskPriority::Close, None, f)
}

fn with_priority<F>(priority: HostTaskPriority, key: Option<HostTaskKey>, f: F) -> bool
where
    F: FnOnce(&mut EngineHost) + 'static,
{
    enum Access {
        Executed,
        Reentrant,
        Unavailable,
        #[cfg(target_os = "macos")]
        TerminalFailed,
    }

    if HOST_SEALED.with(Cell::get) {
        return false;
    }

    let mut task: Option<HostTask> = Some(Box::new(f));
    let access = HOST.with(|cell| match cell.try_borrow_mut() {
        Ok(mut slot) => {
            let Some(host) = slot.as_mut() else {
                return Access::Unavailable;
            };
            if priority == HostTaskPriority::Shutdown {
                #[cfg(target_os = "macos")]
                if !drain_extension_browser_request_terminals_with_host(host) {
                    return Access::TerminalFailed;
                }
                #[cfg(target_os = "macos")]
                if !drain_page_permission_terminals_with_host(host) {
                    return Access::TerminalFailed;
                }
                // The host exists and the barrier is about to execute. Seal
                // before native teardown so a callback pumped by teardown
                // cannot recreate a controller behind it.
                HOST_SEALED.with(|sealed| sealed.set(true));
            }
            if let Some(task) = task.take() {
                task(host);
            }
            #[cfg(target_os = "windows")]
            host.retry_windows_cleanup_debts(1);
            Access::Executed
        }
        Err(_) => Access::Reentrant,
    });
    match access {
        Access::Unavailable => return false,
        #[cfg(target_os = "macos")]
        Access::TerminalFailed => return false,
        Access::Reentrant => {
            return PENDING.with(|pending| {
                let Ok(mut pending) = pending.try_borrow_mut() else {
                    // Queue mutation can run destructors for replaced work.
                    // If one reenters here, reject this admission explicitly;
                    // a RefCell panic would abort production builds.
                    eprintln!("engine: rejected recursively borrowed host queue admission");
                    return false;
                };
                let Some(task) = task.take() else {
                    HOST_SEALED.with(|sealed| sealed.set(true));
                    return false;
                };
                let queued = QueuedHostTask {
                    priority,
                    key,
                    task,
                };
                let accepted = enqueue_pending(&mut pending, queued);
                if accepted && priority == HostTaskPriority::Shutdown {
                    // Non-shutdown admission stops one slot early, so a first
                    // shutdown is guaranteed a physical queue slot. Seal only
                    // after that barrier has actually been admitted.
                    HOST_SEALED.with(|sealed| sealed.set(true));
                }
                if !accepted
                    && PENDING_OVERFLOW_LOGS_REMAINING
                        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
                            value.checked_sub(1)
                        })
                        .is_ok()
                {
                    // Reentrant WebView2 construction pumps the native loop. A
                    // hostile renderer must not turn that into an unbounded queue;
                    // blocking here would deadlock the same main-thread borrow.
                    eprintln!("engine: dropping reentrant native task at bounded capacity");
                }
                accepted
            });
        }
        Access::Executed => {}
    }

    #[cfg(target_os = "macos")]
    if !drain_extension_browser_request_terminals() {
        HOST_SEALED.with(|sealed| sealed.set(true));
        return false;
    }

    #[cfg(target_os = "macos")]
    if !drain_page_permission_terminals() {
        HOST_SEALED.with(|sealed| sealed.set(true));
        return false;
    }

    #[cfg(not(target_os = "windows"))]
    if !drain_content_policy_terminal_debts() {
        // An exact native compiler terminal is a lifecycle debt, not a
        // replaceable observation. If its dedicated queue cannot be drained,
        // ordinary ingress must stop and the callback owner will invoke the
        // mandatory terminal-failure path.
        HOST_SEALED.with(|sealed| sealed.set(true));
        return false;
    }

    loop {
        let queued = PENDING.with(|pending| {
            pending
                .try_borrow_mut()
                .map(|mut pending| pending.pop_front())
        });
        let queued = match queued {
            Ok(Some(queued)) => queued,
            Ok(None) => break,
            Err(_) => {
                // An authoritative drain cannot be resumed in an unknown
                // ordering state. Seal ingress rather than aborting or
                // executing accepted work out of order.
                HOST_SEALED.with(|sealed| sealed.set(true));
                return false;
            }
        };
        let mut queued = Some(queued);
        let accessed = HOST.with(|cell| {
            let Ok(mut slot) = cell.try_borrow_mut() else {
                return false;
            };
            if let Some(host) = slot.as_mut() {
                if let Some(queued) = queued.take() {
                    (queued.task)(host);
                    #[cfg(target_os = "windows")]
                    host.retry_windows_cleanup_debts(1);
                }
                true
            } else {
                false
            }
        });
        if !accessed {
            // Preserve the already-admitted task if the host was unexpectedly
            // still borrowed, but seal all new ingress because its ordering
            // relative to the active callback can no longer be proved.
            if let Some(queued) = queued.take() {
                let _ = PENDING.with(|pending| {
                    pending
                        .try_borrow_mut()
                        .map(|mut pending| pending.push_front(queued))
                });
            }
            HOST_SEALED.with(|sealed| sealed.set(true));
            return false;
        }
        #[cfg(target_os = "macos")]
        if !drain_extension_browser_request_terminals() {
            HOST_SEALED.with(|sealed| sealed.set(true));
            return false;
        }
        #[cfg(target_os = "macos")]
        if !drain_page_permission_terminals() {
            HOST_SEALED.with(|sealed| sealed.set(true));
            return false;
        }
        #[cfg(not(target_os = "windows"))]
        if !drain_content_policy_terminal_debts() {
            HOST_SEALED.with(|sealed| sealed.set(true));
            return false;
        }
    }
    true
}

fn enqueue_pending(pending: &mut VecDeque<QueuedHostTask>, queued: QueuedHostTask) -> bool {
    let contains_shutdown = pending
        .iter()
        .any(|task| task.priority == HostTaskPriority::Shutdown);
    if contains_shutdown {
        // Nothing may be admitted behind the teardown barrier. `HOST_SEALED`
        // enforces this at the public ingress; keep the queue primitive safe
        // when exercised directly as well.
        return false;
    }
    #[cfg(feature = "agentic-browser")]
    if queued.priority == HostTaskPriority::AgentContext
        && pending
            .iter()
            .filter(|task| task.priority == HostTaskPriority::AgentContext)
            .count()
            >= AGENT_CONTEXT_PENDING_HOST_TASK_CAPACITY
    {
        return false;
    }
    #[cfg(all(
        feature = "agentic-browser",
        any(target_os = "macos", target_os = "windows")
    ))]
    if queued.priority == HostTaskPriority::AgentContextTerminal
        && pending
            .iter()
            .filter(|task| task.priority == HostTaskPriority::AgentContextTerminal)
            .count()
            >= AGENT_CONTEXT_TERMINAL_PENDING_HOST_TASK_CAPACITY
    {
        return false;
    }
    #[cfg(all(
        feature = "agentic-browser",
        any(target_os = "macos", target_os = "windows")
    ))]
    if matches!(
        queued.priority,
        HostTaskPriority::AgentContext | HostTaskPriority::AgentContextTerminal
    ) {
        return enqueue_bounded_pending(pending, queued);
    }
    #[cfg(all(
        feature = "agentic-browser",
        not(any(target_os = "macos", target_os = "windows"))
    ))]
    if queued.priority == HostTaskPriority::AgentContext {
        return enqueue_bounded_pending(pending, queued);
    }
    let Some(key) = queued.key else {
        return enqueue_bounded_pending(pending, queued);
    };
    if matches!(key, HostTaskKey::NavigationCommit(_)) {
        // A newer exact commit makes an older still-queued commit task a
        // stale no-op. Coalesce globally (not merely adjacently), keeping
        // at most one reserved security gate per bounded native view.
        if let Some(index) = pending.iter().rposition(|task| task.key == Some(key)) {
            pending.remove(index);
            pending.push_back(queued);
            return true;
        }
    }
    if let Some(back) = pending.back().filter(|task| task.key == Some(key)) {
        // Coalesce only an adjacent callback. Crossing an intervening host
        // task can invert native facts around a create/navigation (most
        // critically, a profile-process exit around a profile rebuild).
        if queued.priority < back.priority {
            return true;
        }
        pending.pop_back();
        pending.push_back(queued);
        return true;
    }
    enqueue_bounded_pending(pending, queued)
}

fn enqueue_bounded_pending(pending: &mut VecDeque<QueuedHostTask>, queued: QueuedHostTask) -> bool {
    if queued.priority == HostTaskPriority::Normal
        && pending.len() >= NORMAL_PENDING_HOST_TASK_CAPACITY
    {
        return false;
    }
    let capacity = match queued.priority {
        HostTaskPriority::Shutdown => PENDING_HOST_TASK_CAPACITY,
        HostTaskPriority::ProfileErasure => NON_SHUTDOWN_PENDING_HOST_TASK_CAPACITY,
        #[cfg(all(
            feature = "agentic-browser",
            any(target_os = "macos", target_os = "windows")
        ))]
        HostTaskPriority::AgentContextTerminal => NON_PROFILE_ERASURE_PENDING_HOST_TASK_CAPACITY,
        #[cfg(feature = "agentic-browser")]
        HostTaskPriority::AgentContext => NON_AGENT_CONTEXT_TERMINAL_PENDING_HOST_TASK_CAPACITY,
        _ if matches!(queued.key, Some(HostTaskKey::NavigationCommit(_))) => {
            NON_AGENT_CONTEXT_PENDING_HOST_TASK_CAPACITY
        }
        _ => NON_NAVIGATION_COMMIT_PENDING_HOST_TASK_CAPACITY,
    };
    if pending.len() < capacity {
        pending.push_back(queued);
        return true;
    }

    // At this priority class's bounded ceiling only, retain the newest fact
    // for the same native object even across intervening work. This overload
    // escape hatch prevents one object from crowding itself out; ordinary
    // operation above preserves every ordering barrier.
    if let Some(key) = queued.key {
        if let Some(index) = pending.iter().rposition(|task| task.key == Some(key)) {
            if queued.priority < pending[index].priority {
                return true;
            }
            pending.remove(index);
            pending.push_back(queued);
            return true;
        }
    }

    let replace = match queued.priority {
        HostTaskPriority::Normal => None,
        HostTaskPriority::Observation | HostTaskPriority::Lifecycle | HostTaskPriority::Close => {
            pending
                .iter()
                .position(|task| task.priority < queued.priority)
        }
        #[cfg(all(
            feature = "agentic-browser",
            any(target_os = "macos", target_os = "windows")
        ))]
        HostTaskPriority::AgentContext | HostTaskPriority::AgentContextTerminal => None,
        #[cfg(all(
            feature = "agentic-browser",
            not(any(target_os = "macos", target_os = "windows"))
        ))]
        HostTaskPriority::AgentContext => None,
        // Its dedicated band guarantees the bounded first cohort. Past that
        // point rejecting this attempt is safer than dropping an already
        // admitted close/lifecycle obligation; the public retirement gate has
        // already made the requested profile inaccessible.
        HostTaskPriority::ProfileErasure => None,
        // The non-shutdown ceiling guarantees this arm cannot be reached for
        // the first shutdown barrier.
        HostTaskPriority::Shutdown => None,
    };
    if let Some(index) = replace {
        pending.remove(index);
        pending.push_back(queued);
        return true;
    }

    false
}

pub(crate) fn shutdown(done: Box<dyn FnOnce(bool) + Send>) {
    // Keep completion in the host task itself: `with` may queue during a
    // reentrant WebView2 construction pump, and acknowledging before that
    // queued task runs would let the process exit with live controllers.
    let completion = Arc::new(std::sync::Mutex::new(Some(done)));
    let queued_completion = completion.clone();
    let admitted = with_priority(HostTaskPriority::Shutdown, None, move |host| {
        let Some(done) = queued_completion
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
        else {
            return;
        };
        #[cfg(target_os = "windows")]
        {
            let (done, download_exit) = if let Some(downloads) = &host.downloads {
                let (native_done, download_done) = super::lifecycle::join_download_shutdown(done);
                let notice = downloads.runtime_exit_notifier();
                downloads.quiesce(None, download_done);
                (native_done, Some(notice))
            } else {
                (done, None)
            };
            let download_exit = Arc::new(std::sync::Mutex::new(download_exit));
            let spawn_failure_exit = download_exit.clone();
            let (browser_processes, process_provenance_valid) = host.shutdown();
            let private_runtime_cleanup = host.private_runtime.cleanup_ticket();
            let worker_completion = Arc::new(std::sync::Mutex::new(Some(done)));
            let spawn_failure = worker_completion.clone();
            // Environment5, not the main process HANDLE alone, proves every
            // child process and UDF resource has been released. Keep the app
            // shutdown barrier open for one globally bounded proof wait.
            let spawned = std::thread::Builder::new()
                .name("zephium-webview2-shutdown".into())
                .spawn(move || {
                    let finish = |clean: bool| {
                        if !clean {
                            if let Some(notice)=download_exit.lock().unwrap_or_else(|error|error.into_inner()).take(){notice(false);}
                        }
                        if let Some(done) = worker_completion
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner())
                            .take()
                        {
                            done(clean);
                        }
                    };
                    #[cfg(all(debug_assertions, feature = "native-agentic-work-lifetime-diagnostic"))]
                    {
                        use std::io::Write as _;
                        let _ = writeln!(std::io::stderr().lock(),
                            "windows-work-shutdown: stage=process_wait_start obligations={} provenance_valid={process_provenance_valid}; content=redacted",
                            browser_processes.len());
                    }
                    let process_exit_proven = process_provenance_valid
                        && crate::platform::imp::wait_for_browser_process_shutdown(
                            browser_processes,
                            std::time::Duration::from_secs(5),
                        );
                    #[cfg(all(debug_assertions, feature = "native-agentic-work-lifetime-diagnostic"))]
                    {
                        use std::io::Write as _;
                        let _ = writeln!(std::io::stderr().lock(),
                            "windows-work-shutdown: stage=process_wait_complete proven={process_exit_proven}; content=redacted");
                    }
                    if !process_exit_proven {
                        eprintln!(
                            "privacy: WebView2 full process-group shutdown could not be proven"
                        );
                        finish(false);
                        return;
                    }
                    if let Some(notice)=download_exit.lock().unwrap_or_else(|error|error.into_inner()).take(){notice(true);}
                    let cleaned = match private_runtime_cleanup.cleanup_after_proven_exit() {
                        Ok(()) => true,
                        Err(error) => {
                            eprintln!(
                                "privacy: could not remove private WebView2 runtime data at {}: {error}",
                                private_runtime_cleanup.root().display()
                            );
                            false
                        }
                    };
                    #[cfg(all(debug_assertions, feature = "native-agentic-work-lifetime-diagnostic"))]
                    {
                        use std::io::Write as _;
                        let _ = writeln!(std::io::stderr().lock(),
                            "windows-work-shutdown: stage=private_cleanup_complete clean={cleaned}; content=redacted");
                    }
                    finish(cleaned);
                });
            if let Err(error) = spawned {
                eprintln!("shutdown: could not start WebView2 cleanup worker: {error}");
                if let Some(notice) = spawn_failure_exit
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .take()
                {
                    notice(false);
                }
                if let Some(done) = spawn_failure
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .take()
                {
                    done(false);
                }
            }
        }
        #[cfg(not(target_os = "windows"))]
        {
            host.shutdown(done);
        }
    });
    if !admitted {
        if let Some(done) = completion
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
        {
            done(false);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "windows")]
    use super::super::profiles::{
        transferred_erasure_exit_settlement, windows_profile_provenance_presence_is_consistent,
        TransferredErasureExitSettlement,
    };

    fn queued(priority: HostTaskPriority) -> QueuedHostTask {
        QueuedHostTask {
            priority,
            key: None,
            task: Box::new(|_: &mut EngineHost| {}),
        }
    }

    fn keyed(priority: HostTaskPriority, key: HostTaskKey) -> QueuedHostTask {
        QueuedHostTask {
            priority,
            key: Some(key),
            task: Box::new(|_: &mut EngineHost| {}),
        }
    }

    #[test]
    fn install_claim_is_reentrant_safe_and_never_resets_live_ingress() {
        make_unavailable_for_test();
        HOST_SEALED.with(|sealed| sealed.set(true));
        PENDING.with(|pending| {
            pending
                .borrow_mut()
                .push_back(queued(HostTaskPriority::Lifecycle));
        });
        #[cfg(not(target_os = "windows"))]
        PENDING_CONTENT_POLICY_TERMINALS.with(|pending| {
            let mut slots = pending.take();
            assert!(slots.push_back(Box::new(|_| {})).is_ok());
            pending.set(slots);
        });

        let claim = HostInstallClaim::acquire().expect("first install must claim vacancy");
        let Err(error) = HostInstallClaim::acquire() else {
            panic!("a nested install must not acquire the live claim");
        };
        assert!(error.contains("already active"));
        assert!(HOST_SEALED.with(Cell::get));
        assert_eq!(PENDING.with(|pending| pending.borrow().len()), 1);
        #[cfg(not(target_os = "windows"))]
        assert_eq!(
            PENDING_CONTENT_POLICY_TERMINALS.with(|pending| {
                let slots = pending.take();
                let len = slots.len();
                pending.set(slots);
                len
            }),
            1
        );
        drop(claim);

        HOST.with(|host| {
            let _reentrant_host_borrow = host.borrow_mut();
            let Err(error) = HostInstallClaim::acquire() else {
                panic!("a reentrant host borrow must reject installation");
            };
            assert!(error.contains("re-entrantly borrowed"));
        });
        assert!(!HOST_INSTALLING.with(Cell::get));
        assert!(HOST_SEALED.with(Cell::get));
        assert_eq!(PENDING.with(|pending| pending.borrow().len()), 1);

        make_unavailable_for_test();
    }

    #[test]
    fn host_vacancy_is_claimed_before_install_resets_ingress() {
        let source = include_str!("dispatch.rs");
        let install = source
            .split_once("pub(crate) fn install(")
            .expect("install exists")
            .1
            .split_once("\n}\n\n// WebView2 construction")
            .expect("install body has an audit boundary")
            .0;
        let claim = install
            .find("HostInstallClaim::acquire()")
            .expect("install claims exclusive vacancy");
        let seal_reset = install
            .find("HOST_SEALED.with")
            .expect("install resets ingress after claiming");
        let pending_reset = install
            .find("PENDING.with")
            .expect("install clears pending work after claiming");
        assert!(claim < seal_reset);
        assert!(claim < pending_reset);
    }

    #[test]
    fn tasks_are_rejected_when_the_host_is_unavailable() {
        HOST_SEALED.with(|sealed| sealed.set(false));
        HOST.with(|host| *host.borrow_mut() = None);
        PENDING.with(|pending| pending.borrow_mut().clear());
        assert!(!try_with(|_| panic!(
            "an unavailable host must not run work"
        )));
        assert!(!try_with_close(ItemId::from(1), |_| panic!(
            "an unavailable host must not admit close"
        )));
        assert!(PENDING.with(|pending| pending.borrow().is_empty()));
    }

    #[test]
    fn shutdown_seals_only_after_the_barrier_is_admitted() {
        HOST_SEALED.with(|sealed| sealed.set(false));
        HOST.with(|host| *host.borrow_mut() = None);
        PENDING.with(|pending| pending.borrow_mut().clear());
        assert!(!with_priority(
            HostTaskPriority::Shutdown,
            None,
            |_| panic!("unavailable host must not run shutdown")
        ));
        assert!(!HOST_SEALED.with(Cell::get));

        HOST.with(|host| {
            let _borrow = host.borrow_mut();
            assert!(with_priority(HostTaskPriority::Shutdown, None, |_| panic!(
                "reentrant shutdown must be queued"
            )));
            assert!(HOST_SEALED.with(Cell::get));
            assert!(!try_with(|_| panic!("sealed host must reject later work")));
        });
        assert_eq!(PENDING.with(|pending| pending.borrow().len()), 1);
        PENDING.with(|pending| pending.borrow_mut().clear());
        HOST_SEALED.with(|sealed| sealed.set(false));
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn content_policy_terminals_survive_recursive_queue_borrow_and_shutdown_seal() {
        HOST_SEALED.with(|sealed| sealed.set(true));
        HOST.with(|host| *host.borrow_mut() = None);
        PENDING.with(|pending| pending.borrow_mut().clear());
        PENDING_CONTENT_POLICY_TERMINALS.with(|pending| {
            drop(pending.replace(ContentPolicyTerminalSlots::EMPTY));
        });

        HOST.with(|host| {
            let _active_host_borrow = host.borrow_mut();
            PENDING.with(|pending| {
                let _recursive_pending_borrow = pending.borrow_mut();
                assert!(admit_content_policy_terminal_debt(Box::new(|_| {
                    panic!("a reentrant terminal debt must not execute early")
                })));
                assert!(admit_content_policy_terminal_debt(Box::new(|_| {
                    panic!("a reentrant terminal debt must not execute early")
                })));
                assert!(
                    !admit_content_policy_terminal_debt(Box::new(|_| {})),
                    "one timeout plus one native terminal is the exact physical bound"
                );
            });
        });
        assert_eq!(
            PENDING_CONTENT_POLICY_TERMINALS.with(|pending| {
                let slots = pending.take();
                let len = slots.len();
                pending.set(slots);
                len
            }),
            2
        );
        assert!(
            PENDING.with(|pending| pending.borrow().is_empty()),
            "native compiler terminals must not depend on the ordinary queue"
        );
        PENDING_CONTENT_POLICY_TERMINALS.with(|pending| {
            drop(pending.replace(ContentPolicyTerminalSlots::EMPTY));
        });
        HOST_SEALED.with(|sealed| sealed.set(false));
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn content_policy_terminal_slots_are_fifo() {
        struct DropMarker {
            value: usize,
            order: Arc<std::sync::Mutex<Vec<usize>>>,
        }

        impl Drop for DropMarker {
            fn drop(&mut self) {
                self.order
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push(self.value);
            }
        }

        let order = Arc::new(std::sync::Mutex::new(Vec::new()));
        let first = DropMarker {
            value: 1,
            order: order.clone(),
        };
        let second = DropMarker {
            value: 2,
            order: order.clone(),
        };
        let mut slots = ContentPolicyTerminalSlots::default();
        assert!(slots.push_back(Box::new(move |_| drop(first))).is_ok());
        assert!(slots.push_back(Box::new(move |_| drop(second))).is_ok());
        drop(slots.pop_front());
        drop(slots.pop_front());
        assert_eq!(
            *order
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            vec![1, 2]
        );
    }

    #[cfg(feature = "agentic-browser")]
    #[test]
    fn agent_context_band_is_fixed_noncoalescing_and_recovers_exact_capacity() {
        let mut pending = VecDeque::new();
        for _ in 0..AGENT_CONTEXT_PENDING_HOST_TASK_CAPACITY {
            assert!(enqueue_pending(
                &mut pending,
                queued(HostTaskPriority::AgentContext)
            ));
        }
        assert!(!enqueue_pending(
            &mut pending,
            queued(HostTaskPriority::AgentContext)
        ));
        assert_eq!(pending.len(), AGENT_CONTEXT_PENDING_HOST_TASK_CAPACITY);
        assert!(pending
            .iter()
            .all(|task| task.priority == HostTaskPriority::AgentContext));

        pending.pop_front();
        assert!(enqueue_pending(
            &mut pending,
            queued(HostTaskPriority::AgentContext)
        ));
        assert_eq!(pending.len(), AGENT_CONTEXT_PENDING_HOST_TASK_CAPACITY);
    }

    #[cfg(all(
        feature = "agentic-browser",
        any(target_os = "macos", target_os = "windows")
    ))]
    #[test]
    fn agent_context_terminals_have_an_independent_exact_band() {
        assert_eq!(
            AGENT_CONTEXT_TERMINAL_PENDING_HOST_TASK_CAPACITY,
            2 * zephium_agentic::MAX_LIVE_CONTEXTS
        );
        let mut pending = VecDeque::new();
        for _ in 0..AGENT_CONTEXT_PENDING_HOST_TASK_CAPACITY {
            assert!(enqueue_pending(
                &mut pending,
                queued(HostTaskPriority::AgentContext)
            ));
        }
        for _ in 0..AGENT_CONTEXT_TERMINAL_PENDING_HOST_TASK_CAPACITY {
            assert!(enqueue_pending(
                &mut pending,
                queued(HostTaskPriority::AgentContextTerminal)
            ));
        }
        assert!(!enqueue_pending(
            &mut pending,
            queued(HostTaskPriority::AgentContextTerminal)
        ));
        assert_eq!(
            pending
                .iter()
                .filter(|task| task.priority == HostTaskPriority::AgentContextTerminal)
                .count(),
            AGENT_CONTEXT_TERMINAL_PENDING_HOST_TASK_CAPACITY
        );
    }

    #[test]
    fn reentrant_queue_bounds_shutdown_and_prioritizes_close() {
        let mut pending = VecDeque::new();
        for _ in 0..NORMAL_PENDING_HOST_TASK_CAPACITY {
            assert!(enqueue_pending(
                &mut pending,
                queued(HostTaskPriority::Normal)
            ));
        }
        assert!(!enqueue_pending(
            &mut pending,
            queued(HostTaskPriority::Normal)
        ));
        for _ in pending.len()..NON_NAVIGATION_COMMIT_PENDING_HOST_TASK_CAPACITY {
            assert!(enqueue_pending(
                &mut pending,
                queued(HostTaskPriority::Observation)
            ));
        }
        assert!(enqueue_pending(
            &mut pending,
            queued(HostTaskPriority::Close)
        ));
        assert_eq!(
            pending.len(),
            NON_NAVIGATION_COMMIT_PENDING_HOST_TASK_CAPACITY
        );
        assert_eq!(pending.back().unwrap().priority, HostTaskPriority::Close);

        for raw in 1..=NAVIGATION_COMMIT_PENDING_HOST_TASK_CAPACITY {
            assert!(enqueue_pending(
                &mut pending,
                keyed(
                    HostTaskPriority::Lifecycle,
                    HostTaskKey::NavigationCommit(ItemId::from(raw as u128))
                )
            ));
        }
        assert_eq!(pending.len(), NON_AGENT_CONTEXT_PENDING_HOST_TASK_CAPACITY);
        #[cfg(feature = "agentic-browser")]
        for _ in 0..AGENT_CONTEXT_PENDING_HOST_TASK_CAPACITY {
            assert!(enqueue_pending(
                &mut pending,
                queued(HostTaskPriority::AgentContext)
            ));
        }
        #[cfg(all(
            feature = "agentic-browser",
            any(target_os = "macos", target_os = "windows")
        ))]
        for _ in 0..AGENT_CONTEXT_TERMINAL_PENDING_HOST_TASK_CAPACITY {
            assert!(enqueue_pending(
                &mut pending,
                queued(HostTaskPriority::AgentContextTerminal)
            ));
        }
        assert_eq!(
            pending.len(),
            NON_PROFILE_ERASURE_PENDING_HOST_TASK_CAPACITY
        );

        for _ in 0..PROFILE_ERASURE_PENDING_HOST_TASK_CAPACITY {
            assert!(enqueue_pending(
                &mut pending,
                queued(HostTaskPriority::ProfileErasure)
            ));
        }
        assert_eq!(pending.len(), NON_SHUTDOWN_PENDING_HOST_TASK_CAPACITY);
        assert!(enqueue_pending(
            &mut pending,
            queued(HostTaskPriority::Shutdown)
        ));
        assert_eq!(pending.len(), PENDING_HOST_TASK_CAPACITY);
        assert_eq!(pending.back().unwrap().priority, HostTaskPriority::Shutdown);
        assert!(!enqueue_pending(
            &mut pending,
            queued(HostTaskPriority::Shutdown)
        ));
        assert_eq!(
            pending
                .iter()
                .filter(|task| task.priority == HostTaskPriority::Shutdown)
                .count(),
            1
        );
    }

    #[test]
    fn erasure_and_shutdown_slots_survive_full_lifecycle_saturation() {
        let mut pending = VecDeque::new();
        for _ in 0..NON_NAVIGATION_COMMIT_PENDING_HOST_TASK_CAPACITY {
            assert!(enqueue_pending(
                &mut pending,
                queued(HostTaskPriority::Lifecycle)
            ));
        }
        assert!(!enqueue_pending(
            &mut pending,
            queued(HostTaskPriority::Lifecycle)
        ));

        for raw in 1..=NAVIGATION_COMMIT_PENDING_HOST_TASK_CAPACITY {
            assert!(enqueue_pending(
                &mut pending,
                keyed(
                    HostTaskPriority::Lifecycle,
                    HostTaskKey::NavigationCommit(ItemId::from(raw as u128))
                )
            ));
        }
        assert_eq!(pending.len(), NON_AGENT_CONTEXT_PENDING_HOST_TASK_CAPACITY);
        #[cfg(feature = "agentic-browser")]
        for _ in 0..AGENT_CONTEXT_PENDING_HOST_TASK_CAPACITY {
            assert!(enqueue_pending(
                &mut pending,
                queued(HostTaskPriority::AgentContext)
            ));
        }
        #[cfg(all(
            feature = "agentic-browser",
            any(target_os = "macos", target_os = "windows")
        ))]
        for _ in 0..AGENT_CONTEXT_TERMINAL_PENDING_HOST_TASK_CAPACITY {
            assert!(enqueue_pending(
                &mut pending,
                queued(HostTaskPriority::AgentContextTerminal)
            ));
        }
        assert_eq!(
            pending.len(),
            NON_PROFILE_ERASURE_PENDING_HOST_TASK_CAPACITY
        );

        for _ in 0..PROFILE_ERASURE_PENDING_HOST_TASK_CAPACITY {
            assert!(enqueue_pending(
                &mut pending,
                queued(HostTaskPriority::ProfileErasure)
            ));
        }
        assert_eq!(pending.len(), NON_SHUTDOWN_PENDING_HOST_TASK_CAPACITY);
        assert_eq!(
            pending
                .iter()
                .filter(|task| task.priority == HostTaskPriority::ProfileErasure)
                .count(),
            PROFILE_ERASURE_PENDING_HOST_TASK_CAPACITY
        );
        assert!(!enqueue_pending(
            &mut pending,
            queued(HostTaskPriority::ProfileErasure)
        ));
        assert_eq!(
            pending
                .iter()
                .filter(|task| task.priority == HostTaskPriority::Lifecycle)
                .count(),
            NON_AGENT_CONTEXT_PENDING_HOST_TASK_CAPACITY
        );
        assert_eq!(
            pending
                .iter()
                .filter(|task| task.priority == HostTaskPriority::ProfileErasure)
                .count(),
            PROFILE_ERASURE_PENDING_HOST_TASK_CAPACITY
        );

        assert!(enqueue_pending(
            &mut pending,
            queued(HostTaskPriority::Shutdown)
        ));
        assert_eq!(pending.len(), PENDING_HOST_TASK_CAPACITY);
        assert_eq!(
            pending
                .iter()
                .filter(|task| task.priority == HostTaskPriority::Lifecycle)
                .count(),
            NON_AGENT_CONTEXT_PENDING_HOST_TASK_CAPACITY
        );
        assert_eq!(
            pending
                .iter()
                .filter(|task| task.priority == HostTaskPriority::Shutdown)
                .count(),
            1
        );
    }

    #[test]
    fn keyed_native_callbacks_coalesce_and_stronger_lifecycle_wins() {
        let id = ItemId::from(7);
        let key = HostTaskKey::View(id);
        let mut pending = VecDeque::new();

        assert!(enqueue_pending(
            &mut pending,
            keyed(HostTaskPriority::Observation, key)
        ));
        assert!(enqueue_pending(
            &mut pending,
            keyed(HostTaskPriority::Observation, key)
        ));
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].priority, HostTaskPriority::Observation);

        assert!(enqueue_pending(
            &mut pending,
            keyed(HostTaskPriority::Lifecycle, key)
        ));
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].priority, HostTaskPriority::Lifecycle);

        // A stale KVO/SourceChanged callback racing after process death is
        // safely subsumed and cannot replace the queued crash transition.
        assert!(enqueue_pending(
            &mut pending,
            keyed(HostTaskPriority::Observation, key)
        ));
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].priority, HostTaskPriority::Lifecycle);
    }

    #[test]
    fn source_settlement_and_discard_obligations_do_not_replace_each_other() {
        let id = ItemId::from(7);
        let mut pending = VecDeque::new();
        assert!(enqueue_pending(
            &mut pending,
            keyed(HostTaskPriority::Observation, HostTaskKey::Source(id))
        ));
        assert!(enqueue_pending(
            &mut pending,
            keyed(
                HostTaskPriority::Lifecycle,
                HostTaskKey::NavigationSettlement(id)
            )
        ));
        assert!(enqueue_pending(
            &mut pending,
            keyed(HostTaskPriority::Observation, HostTaskKey::Discard(id))
        ));

        assert_eq!(pending.len(), 3);
        assert_eq!(pending[0].key, Some(HostTaskKey::Source(id)));
        assert_eq!(pending[1].key, Some(HostTaskKey::NavigationSettlement(id)));
        assert_eq!(pending[2].key, Some(HostTaskKey::Discard(id)));
    }

    #[test]
    fn committed_document_gate_has_one_reserved_globally_coalesced_slot_per_native_view() {
        let id = ItemId::from(7);
        let commit = HostTaskKey::NavigationCommit(id);
        let mut pending = VecDeque::new();
        assert!(enqueue_pending(
            &mut pending,
            keyed(HostTaskPriority::Lifecycle, commit)
        ));
        assert!(enqueue_pending(
            &mut pending,
            keyed(
                HostTaskPriority::Lifecycle,
                HostTaskKey::NavigationSettlement(id)
            )
        ));
        assert!(enqueue_pending(
            &mut pending,
            keyed(HostTaskPriority::Lifecycle, commit)
        ));
        assert_eq!(pending.len(), 2);
        assert_eq!(
            pending
                .iter()
                .filter(|task| task.key == Some(commit))
                .count(),
            1
        );
        assert_eq!(pending.back().and_then(|task| task.key), Some(commit));

        while pending.len() < NON_NAVIGATION_COMMIT_PENDING_HOST_TASK_CAPACITY {
            assert!(enqueue_pending(
                &mut pending,
                queued(HostTaskPriority::Lifecycle)
            ));
        }
        for raw in 1..NAVIGATION_COMMIT_PENDING_HOST_TASK_CAPACITY {
            let other = HostTaskKey::NavigationCommit(ItemId::from(100 + raw as u128));
            assert!(enqueue_pending(
                &mut pending,
                keyed(HostTaskPriority::Lifecycle, other)
            ));
        }
        // Replacing this view's existing commit remains admitted at the
        // ordinary ceiling and never grows the reserved key cohort.
        assert!(enqueue_pending(
            &mut pending,
            keyed(HostTaskPriority::Lifecycle, commit)
        ));
        assert!(pending.len() <= NON_AGENT_CONTEXT_PENDING_HOST_TASK_CAPACITY);
    }

    #[test]
    fn keyed_coalescing_never_crosses_intervening_host_work_below_capacity() {
        let key = HostTaskKey::View(ItemId::from(7));
        let mut pending = VecDeque::new();

        assert!(enqueue_pending(
            &mut pending,
            keyed(HostTaskPriority::Observation, key)
        ));
        assert!(enqueue_pending(
            &mut pending,
            queued(HostTaskPriority::Normal)
        ));
        assert!(enqueue_pending(
            &mut pending,
            keyed(HostTaskPriority::Observation, key)
        ));

        assert_eq!(pending.len(), 3);
        assert_eq!(pending[0].key, Some(key));
        assert_eq!(pending[1].priority, HostTaskPriority::Normal);
        assert_eq!(pending[2].key, Some(key));
    }

    #[test]
    fn same_key_may_cross_an_ordering_barrier_only_at_absolute_overload() {
        let key = HostTaskKey::View(ItemId::from(7));
        let mut pending = VecDeque::new();
        assert!(enqueue_pending(
            &mut pending,
            keyed(HostTaskPriority::Observation, key)
        ));
        assert!(enqueue_pending(
            &mut pending,
            queued(HostTaskPriority::Normal)
        ));
        while pending.len() < NON_NAVIGATION_COMMIT_PENDING_HOST_TASK_CAPACITY {
            assert!(enqueue_pending(
                &mut pending,
                queued(HostTaskPriority::Observation)
            ));
        }

        assert!(enqueue_pending(
            &mut pending,
            keyed(HostTaskPriority::Observation, key)
        ));
        assert_eq!(
            pending.len(),
            NON_NAVIGATION_COMMIT_PENDING_HOST_TASK_CAPACITY
        );
        assert_eq!(pending.front().unwrap().priority, HostTaskPriority::Normal);
        assert_eq!(pending.back().unwrap().key, Some(key));
        assert_eq!(
            pending.iter().filter(|task| task.key == Some(key)).count(),
            1
        );
    }

    #[test]
    fn native_callbacks_use_the_reserved_band_beyond_normal_saturation() {
        let mut pending = VecDeque::new();
        for _ in 0..NORMAL_PENDING_HOST_TASK_CAPACITY {
            assert!(enqueue_pending(
                &mut pending,
                queued(HostTaskPriority::Normal)
            ));
        }
        assert!(!enqueue_pending(
            &mut pending,
            queued(HostTaskPriority::Normal)
        ));
        assert!(enqueue_pending(
            &mut pending,
            keyed(
                HostTaskPriority::Observation,
                HostTaskKey::View(ItemId::from(1))
            )
        ));
        assert!(enqueue_pending(
            &mut pending,
            queued(HostTaskPriority::ProfileErasure)
        ));
        assert_eq!(
            pending.back().map(|task| task.priority),
            Some(HostTaskPriority::ProfileErasure)
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn coalesced_process_failed_still_settles_a_transferred_exact_exit_proof() {
        let profile = ProfileId::from(91);
        let generation = crate::platform::imp::BrowserProcessGeneration::for_test(7);
        let key = HostTaskKey::Profile(profile, generation);
        let mut pending = VecDeque::new();

        // Environment5 records the shared proof before queuing its host task.
        // A reentrant ProcessFailed callback then replaces that adjacent task
        // because both lifecycle facts intentionally share the exact key.
        assert!(enqueue_pending(
            &mut pending,
            keyed(HostTaskPriority::Lifecycle, key)
        ));
        assert!(enqueue_pending(
            &mut pending,
            keyed(HostTaskPriority::Lifecycle, key)
        ));
        assert_eq!(pending.len(), 1);
        assert_eq!(pending.front().and_then(|task| task.key), Some(key));

        // The surviving ProcessFailed path must consult the already-recorded
        // observer state. It releases the exact process/observer pair, restoring
        // the exact empty-set shutdown invariant after successful erasure.
        assert_eq!(
            transferred_erasure_exit_settlement(41, 41, true, true, false),
            TransferredErasureExitSettlement::Proven
        );
        assert!(windows_profile_provenance_presence_is_consistent(
            false, false, false, false
        ));
    }
}
