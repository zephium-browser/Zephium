//! The browser-facing namespaces on WebKitGTK: tabs and windows from the
//! shell's extension surface, frames the bridge verified, toolbar actions,
//! programmatic injection, and requests the engine settles.
//!
//! The engine publishes facts (`publish_surface`) and performs side effects
//! it receives as [`BrowserCall`]s; it answers each through
//! `ProfileRuntime::settle_browser`. Nothing here trusts a renderer claim the
//! bridge did not verify.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::path::PathBuf;
use std::rc::Rc;

use serde_json::{json, Value};
use url::Url;
use zephium_core::extension_data::{ExtensionDataCall, ExtensionDataReply};
use zephium_core::extensions::{
    ExtensionActionIcon, ExtensionBrowserRequestAction as CoreAction,
    ExtensionBrowserRequestRejection, ExtensionBrowserRequestResult,
    ExtensionBrowserRequestSettlement, ExtensionBrowserSurface,
};
use zephium_core::ids::{ExtensionInstallId, ItemId, ProfileId, WindowId};
use zephium_webext_runtime::api::action::{self, ActionStore};
use zephium_webext_runtime::api::scripting::{self, ExecutionResult, FrameResult, ScriptSource};
use zephium_webext_runtime::api::web_navigation::BrowserFrame;
use zephium_webext_runtime::api::{bookmarks, history, sessions, tabs, top_sites, windows};
use zephium_webext_runtime::browser::{
    unsupported, BrowserPort, BrowserRequest, BrowserSnapshot, BrowserTab, ExtensionInfo,
    ServiceRequest,
};
use zephium_webext_runtime::context::{ContextKind, DocumentGeneration, FrameId, Sender};
use zephium_webext_runtime::dispatch::{Handled, Invocation, Responder};
use zephium_webext_runtime::errors::ApiError;
use zephium_webext_runtime::pattern::DocumentOrigin;
use zephium_webext_runtime::permissions::{InstallPermissions, PermissionRequest};
use zephium_webext_runtime::scripting_registry::ExecutionWorld;
use zephium_webkit_web_process_protocol::protocol::{
    InjectionKind, InjectionResult, InjectionStatus,
};

use crate::bridge::{BridgeViewId, FrameLifecycle, ViewRef};

/// Bounded wait for a programmatic injection, per file.
const INJECTION_TIMEOUT_MS: u32 = 10_000;
/// Package files read for one `executeScript` or `insertCSS` call.
const MAX_INJECTED_FILE_BYTES: u64 = 2 * 1024 * 1024;
/// Requests waiting for the engine at once, per profile.
const MAX_PENDING: usize = 1024;
/// Frames remembered per tab.
const MAX_FRAMES_PER_TAB: usize = 512;
/// Synthetic documents for tabs whose main frame the bridge has not seen.
const SYNTHETIC_DOCUMENT: u64 = 1 << 62;

/// Work for the engine. Calls with a `request` must be answered through
/// `ProfileRuntime::settle_browser` with the matching settlement.
#[derive(Clone, Debug)]
pub enum BrowserCall {
    /// A Shell mutation of tabs; answer with [`BrowserSettlement::Mutation`].
    Mutate {
        /// The request.
        request: u64,
        /// The Shell operation.
        action: CoreAction,
    },
    /// History, bookmarks, closed tabs or top sites; answer with
    /// [`BrowserSettlement::Data`].
    Data {
        /// The request.
        request: u64,
        /// The data call.
        call: ExtensionDataCall,
    },
    /// Ask the person for optional access; answer with
    /// [`BrowserSettlement::Prompt`].
    Prompt {
        /// The request.
        request: u64,
        /// The extension.
        extension_id: String,
        /// Human-readable warnings for the requested access.
        warnings: Vec<String>,
        /// API permissions asked for.
        permissions: Vec<String>,
        /// Origins asked for.
        patterns: Vec<String>,
    },
    /// Access the extension gave up itself.
    AccessRemoved {
        /// The extension.
        extension_id: String,
        /// API permissions removed.
        permissions: Vec<String>,
        /// Origins removed.
        patterns: Vec<String>,
    },
    /// Toolbar state changed for at least one action.
    ActionsChanged,
    /// `action.openPopup()` for the install on this tab.
    OpenPopup {
        /// The install.
        install: ExtensionInstallId,
        /// The tab.
        tab: ItemId,
    },
    /// Capture the visible tab; answer with [`BrowserSettlement::Capture`].
    Capture {
        /// The request.
        request: u64,
        /// The tab.
        tab: ItemId,
        /// PNG when false, JPEG when true.
        jpeg: bool,
        /// JPEG quality.
        quality: Option<u8>,
    },
}

/// The engine's answer to a [`BrowserCall`].
#[derive(Clone, Debug)]
pub enum BrowserSettlement {
    /// A Shell mutation settled.
    Mutation(ExtensionBrowserRequestSettlement),
    /// A data call settled.
    Data(ExtensionDataReply),
    /// The person answered a prompt.
    Prompt(bool),
    /// A capture finished: a `data:` URL, or why not.
    Capture(Result<String, String>),
}

/// Work the runtime's GTK side performs after a dispatch.
pub(crate) enum Outgoing {
    Engine(BrowserCall),
    Inject {
        pending: u64,
        frame: usize,
        tab: ItemId,
        target: Box<BrowserFrame>,
        install: ExtensionInstallId,
        step: Box<Step>,
    },
}

#[derive(Clone, Debug)]
pub(crate) struct Step {
    pub world: String,
    pub kind: InjectionKind,
    pub source: String,
    pub source_url: String,
    pub args_json: String,
}

impl Step {
    pub(crate) fn timeout_ms(&self) -> u32 {
        INJECTION_TIMEOUT_MS
    }
}

#[derive(Clone, Debug)]
struct Frame {
    document: DocumentGeneration,
    url: Url,
    parent: Option<FrameId>,
    opaque: bool,
    creator: Option<Url>,
}

struct IdMap<K: std::hash::Hash + Eq + Copy> {
    forward: HashMap<K, i32>,
    back: HashMap<i32, K>,
    next: i32,
}

impl<K: std::hash::Hash + Eq + Copy> Default for IdMap<K> {
    fn default() -> Self {
        Self {
            forward: HashMap::new(),
            back: HashMap::new(),
            next: 1,
        }
    }
}

impl<K: std::hash::Hash + Eq + Copy> IdMap<K> {
    fn assign(&mut self, key: K) -> i32 {
        if let Some(id) = self.forward.get(&key) {
            return *id;
        }
        let id = self.next;
        self.next = self.next.saturating_add(1);
        self.forward.insert(key, id);
        self.back.insert(id, key);
        id
    }

    fn get(&self, key: K) -> Option<i32> {
        self.forward.get(&key).copied()
    }

    fn key(&self, id: i32) -> Option<K> {
        self.back.get(&id).copied()
    }

    fn retain(&mut self, keep: impl Fn(&K) -> bool) {
        self.forward.retain(|key, _| keep(key));
        let forward = &self.forward;
        self.back.retain(|_, key| forward.contains_key(key));
    }
}

/// What the engine port knows about one admitted install.
#[derive(Clone, Debug)]
pub(crate) struct InstallFacts {
    pub extension_id: String,
    pub root: PathBuf,
    pub manifest: Value,
}

#[derive(Clone, Debug)]
enum Reply {
    Null,
    Created,
    Tab(i32),
    Tabs(Vec<i32>, bool),
    Window(i32),
}

struct FrameJob {
    frame: BrowserFrame,
    steps: VecDeque<Step>,
    result: Option<Result<ExecutionResult, ApiError>>,
}

enum Shape {
    Mutations {
        remaining: VecDeque<CoreAction>,
        reply: Reply,
    },
    Data {
        method: &'static str,
        args: Vec<Value>,
    },
    Prompt(PermissionRequest),
    Capture,
    Inject {
        request: scripting::Request,
        jobs: Vec<FrameJob>,
    },
}

struct Pending {
    responder: Responder,
    install: ExtensionInstallId,
    sender: Sender,
    permissions: InstallPermissions,
    shape: Shape,
}

/// What a settled browser request leaves for the runtime host.
pub(crate) enum Finished {
    /// Settle the call.
    Reply(Responder, Result<Value, ApiError>),
    /// Grant before settling; the reply is whether it was granted.
    Grant {
        responder: Responder,
        install: ExtensionInstallId,
        request: PermissionRequest,
    },
}

/// Tab events the runtime host projects per install.
pub(crate) enum TabEvent {
    Created(Box<BrowserTab>),
    Removed {
        tab: i32,
        window: i32,
    },
    Activated {
        tab: i32,
        window: i32,
    },
    Updated {
        previous: Box<BrowserTab>,
        current: Box<BrowserTab>,
    },
}

#[derive(Default)]
pub(crate) struct BrowserState {
    profile: Option<ProfileId>,
    surface: Option<ExtensionBrowserSurface>,
    tabs: IdMap<ItemId>,
    windows: IdMap<WindowId>,
    views: HashMap<BridgeViewId, ItemId>,
    frames: HashMap<ItemId, BTreeMap<FrameId, Frame>>,
    installs: HashMap<ExtensionInstallId, InstallFacts>,
    actions: HashMap<ExtensionInstallId, ActionStore>,
    pending: HashMap<u64, Pending>,
    injections: HashMap<u64, (u64, usize)>,
    outgoing: VecDeque<Outgoing>,
    next: u64,
}

/// State shared by the port, the runtime host and the GTK side.
#[derive(Default)]
pub(crate) struct Shared {
    state: RefCell<BrowserState>,
    kick: RefCell<Option<Rc<dyn Fn()>>>,
}

/// The [`BrowserPort`] the runtime host dispatches to.
pub(crate) struct Browser {
    shared: Rc<Shared>,
}

impl Browser {
    pub(crate) fn new(shared: Rc<Shared>) -> Self {
        Self { shared }
    }
}

fn document_id(document: DocumentGeneration) -> String {
    format!("{:032X}", document.get())
}

fn extension_url(extension: &str, path: &str) -> Option<String> {
    let path = zephium_webext::manifest::normalize_resource(path)?;
    Some(format!("chrome-extension://{extension}/{path}"))
}

impl Shared {
    pub(crate) fn set_profile(&self, profile: ProfileId) {
        self.state.borrow_mut().profile = Some(profile);
    }

    /// Wakes the GTK side when work is queued from inside a dispatch.
    pub(crate) fn set_kick(&self, kick: Rc<dyn Fn()>) {
        *self.kick.borrow_mut() = Some(kick);
    }

    fn kick_if_queued(&self) {
        if self.state.borrow().outgoing.is_empty() {
            return;
        }
        let kick = self.kick.borrow().clone();
        if let Some(kick) = kick {
            kick();
        }
    }

    pub(crate) fn take_outgoing(&self) -> Vec<Outgoing> {
        self.state.borrow_mut().outgoing.drain(..).collect()
    }

    pub(crate) fn admit(&self, install: ExtensionInstallId, facts: InstallFacts) {
        let mut state = self.state.borrow_mut();
        let store = action_defaults(&facts);
        state.actions.insert(install, store);
        state.installs.insert(install, facts);
        state
            .outgoing
            .push_back(Outgoing::Engine(BrowserCall::ActionsChanged));
    }

    /// The install stopped: its pending requests are dropped unanswered (its
    /// contexts are gone) and its toolbar state forgotten.
    pub(crate) fn forget(&self, install: ExtensionInstallId) -> Vec<Responder> {
        let mut state = self.state.borrow_mut();
        state.installs.remove(&install);
        state.actions.remove(&install);
        let ids: Vec<u64> = state
            .pending
            .iter()
            .filter(|(_, pending)| pending.install == install)
            .map(|(id, _)| *id)
            .collect();
        let mut responders = Vec::new();
        for id in &ids {
            if let Some(pending) = state.pending.remove(id) {
                responders.push(pending.responder);
            }
        }
        state
            .injections
            .retain(|_, (pending, _)| !ids.contains(pending));
        state
            .outgoing
            .push_back(Outgoing::Engine(BrowserCall::ActionsChanged));
        responders
    }

    pub(crate) fn tab_view(&self, view: BridgeViewId, tab: Option<ItemId>) {
        let mut state = self.state.borrow_mut();
        match tab {
            Some(tab) => {
                state.views.insert(view, tab);
            }
            None => {
                if let Some(tab) = state.views.remove(&view) {
                    state.frames.remove(&tab);
                }
            }
        }
    }

    /// The Chrome tab id and current frame record of `document`, for a
    /// `webNavigation` observation.
    pub(crate) fn navigation_frame(
        &self,
        tab: ItemId,
        frame: FrameId,
        document: DocumentGeneration,
    ) -> Option<(i32, BrowserFrame)> {
        let state = self.state.borrow();
        let tab_id = state.tabs.get(tab)?;
        let found = state
            .browser_frames(tab)
            .into_iter()
            .find(|found| found.frame == frame && found.document == document)?;
        Some((tab_id, found))
    }

    /// Whether a tab's frame already holds a recorded document.
    pub(crate) fn navigation_frame_exists(&self, tab: ItemId, frame: FrameId) -> bool {
        self.state
            .borrow()
            .frames
            .get(&tab)
            .is_some_and(|frames| frames.contains_key(&frame))
    }

    /// A same-document navigation of a tab's main frame: records `url` and
    /// returns the main document when the address changed.
    pub(crate) fn main_url_changed(
        &self,
        view: BridgeViewId,
        url: Url,
    ) -> Option<(ItemId, DocumentGeneration, Url)> {
        let mut state = self.state.borrow_mut();
        let tab = *state.views.get(&view)?;
        let main = state.frames.get_mut(&tab)?.get_mut(&FrameId::MAIN)?;
        if main.opaque || main.url == url || main.url.origin() != url.origin() {
            return None;
        }
        let previous = std::mem::replace(&mut main.url, url);
        Some((tab, main.document, previous))
    }

    /// Hosts of the non-opaque extension documents live in a tab view's frames.
    pub(crate) fn framed_extensions(&self, view: BridgeViewId) -> Vec<String> {
        let state = self.state.borrow();
        let Some(frames) = state.views.get(&view).and_then(|tab| state.frames.get(tab)) else {
            return Vec::new();
        };
        let mut hosts: Vec<String> = frames
            .values()
            .filter(|frame| !frame.opaque && frame.url.scheme() == "chrome-extension")
            .filter_map(|frame| frame.url.host_str().map(str::to_owned))
            .collect();
        hosts.sort_unstable();
        hosts.dedup();
        hosts
    }

    pub(crate) fn view_of(&self, tab: ItemId) -> Option<BridgeViewId> {
        self.state
            .borrow()
            .views
            .iter()
            .find(|(_, item)| **item == tab)
            .map(|(view, _)| *view)
    }

    /// `MessageSender.tab` for a sender in `tab`: the tab as Chrome shows
    /// it to the extension whose content script or page is in it.
    pub(crate) fn sender_tab(&self, tab: ItemId) -> Option<Value> {
        let state = self.state.borrow();
        let snapshot = state.snapshot().ok()?;
        let id = state.tabs.get(tab)?;
        let tab = snapshot.tab(id).ok()?;
        Some(json!({
            "id": tab.id, "index": tab.index, "windowId": tab.window_id,
            "active": tab.active, "highlighted": tab.highlighted, "selected": tab.active,
            "pinned": tab.tab.pinned(), "audible": tab.tab.audible(),
            "mutedInfo": {"muted": tab.tab.muted()}, "discarded": tab.tab.discarded(),
            "autoDiscardable": tab.auto_discardable, "frozen": false, "incognito": tab.incognito,
            "groupId": -1, "splitViewId": -1,
            "status": if tab.tab.loading() { "loading" } else { "complete" },
            "url": tab.committed_url().unwrap_or(""), "title": tab.tab.title(),
        }))
    }

    /// The Chrome tab id of a shell tab, given now if it has none yet.
    pub(crate) fn assign_tab_id(&self, tab: ItemId) -> i32 {
        self.state.borrow_mut().tabs.assign(tab)
    }

    /// The Chrome tab id of a shell tab, when it was published.
    pub(crate) fn tab_id(&self, tab: ItemId) -> Option<i32> {
        self.state.borrow().tabs.get(tab)
    }

    /// The shell tab behind a Chrome tab id.
    pub(crate) fn tab_item(&self, id: i64) -> Option<ItemId> {
        let id = i32::try_from(id).ok()?;
        self.state.borrow().tabs.key(id)
    }

    pub(crate) fn document_of(&self, tab: ItemId, document_id: &str) -> Option<DocumentGeneration> {
        let state = self.state.borrow();
        state
            .frames
            .get(&tab)?
            .values()
            .map(|frame| frame.document)
            .find(|document| self::document_id(*document) == document_id)
    }

    /// Records a frame fact the bridge accepted.
    pub(crate) fn frame_event(&self, event: &FrameLifecycle) {
        let mut state = self.state.borrow_mut();
        match event {
            FrameLifecycle::DocumentCreated {
                owner: ViewRef::Tab(tab),
                frame,
                parent,
                opaque,
                creator,
                document,
                url,
                ..
            } => {
                let frames = state.frames.entry(*tab).or_default();
                if frame.is_main() {
                    // A new main document ends the subframes that came before it.
                    frames.retain(|_, existing| existing.document > *document);
                }
                if frames.len() < MAX_FRAMES_PER_TAB || frames.contains_key(frame) {
                    frames.insert(
                        *frame,
                        Frame {
                            document: *document,
                            url: url.clone(),
                            parent: *parent,
                            opaque: *opaque,
                            creator: creator.clone(),
                        },
                    );
                }
            }
            FrameLifecycle::DocumentRemoved {
                owner: ViewRef::Tab(tab),
                frame,
                document,
                ..
            } => {
                if let Some(frames) = state.frames.get_mut(tab) {
                    if frames.get(frame).is_some_and(|f| f.document == *document) {
                        frames.remove(frame);
                    }
                }
            }
            FrameLifecycle::ViewReset {
                owner: ViewRef::Tab(tab),
                ..
            } => {
                state.frames.remove(tab);
            }
            FrameLifecycle::MainDocumentChanged { tab, .. } => {
                if let Some(id) = state.tabs.get(*tab) {
                    let mut changed = false;
                    for store in state.actions.values_mut() {
                        let before = store.get(Some(id), action::Property::BadgeText);
                        if store.reset_tab(id).is_ok()
                            && before != store.get(Some(id), action::Property::BadgeText)
                        {
                            changed = true;
                        }
                    }
                    if changed {
                        state
                            .outgoing
                            .push_back(Outgoing::Engine(BrowserCall::ActionsChanged));
                    }
                }
            }
            _ => {}
        }
    }

    /// Replaces the surface and returns the tab events it implies.
    pub(crate) fn publish_surface(&self, surface: ExtensionBrowserSurface) -> Vec<TabEvent> {
        // Tabs that exist when the browser first publishes were not created.
        let previous = if self.state.borrow().surface.is_some() {
            self.snapshot_any()
        } else {
            None
        };
        {
            let mut state = self.state.borrow_mut();
            if state
                .surface
                .as_ref()
                .is_some_and(|current| current.generation() > surface.generation())
            {
                return Vec::new();
            }
            for window in surface.windows() {
                state.windows.assign(window.id());
                for tab in window.tabs() {
                    state.tabs.assign(tab.id());
                }
            }
            let open: std::collections::HashSet<ItemId> =
                surface.tabs().map(|tab| tab.id()).collect();
            let windows: std::collections::HashSet<WindowId> =
                surface.windows().iter().map(|window| window.id()).collect();
            let closed: Vec<i32> = state
                .tabs
                .forward
                .iter()
                .filter(|(item, _)| !open.contains(item))
                .map(|(_, id)| *id)
                .collect();
            for id in closed {
                for store in state.actions.values_mut() {
                    let _ = store.reset_tab(id);
                }
            }
            state.tabs.retain(|item| open.contains(item));
            state.windows.retain(|window| windows.contains(window));
            state.surface = Some(surface);
        }
        let current = self.snapshot_any();
        let (Some(previous), Some(current)) = (previous, current) else {
            return Vec::new();
        };
        diff(&previous, &current)
    }

    fn snapshot_any(&self) -> Option<BrowserSnapshot> {
        let state = self.state.borrow();
        state.snapshot().ok()
    }

    /// The toolbar projection of every admitted install for `tab`.
    pub(crate) fn action_states(
        &self,
        tab: ItemId,
        runtimes: &HashMap<ExtensionInstallId, zephium_core::extensions::ExtensionRuntimeInstance>,
    ) -> Vec<zephium_core::extensions::ExtensionActionState> {
        let state = self.state.borrow();
        let Some(id) = state.tabs.get(tab) else {
            return Vec::new();
        };
        let mut actions: Vec<_> = state
            .actions
            .iter()
            .filter_map(|(install, store)| {
                let runtime = runtimes.get(install)?;
                store.shell_state(id, tab, *runtime).ok()
            })
            .collect();
        actions.sort_by_key(|action| action.runtime().install_id());
        actions
    }

    /// A trusted toolbar invocation: what the action does on `tab`, after
    /// granting `activeTab`.
    pub(crate) fn invoke_action(
        &self,
        install: ExtensionInstallId,
        tab: ItemId,
        permissions: &mut InstallPermissions,
    ) -> Result<(action::Activation, BrowserTab), ApiError> {
        let state = self.state.borrow();
        let snapshot = state.snapshot()?;
        let id = state
            .tabs
            .get(tab)
            .ok_or_else(ApiError::cannot_access_page)?;
        let tab = snapshot.tab(id)?.clone();
        let store = state
            .actions
            .get(&install)
            .ok_or_else(|| ApiError::other("This extension has no action specified."))?;
        Ok((store.trusted_invoke(&tab, permissions)?, tab))
    }

    /// Takes the request a settlement answers and decides what follows.
    pub(crate) fn settle(&self, request: u64, settlement: BrowserSettlement) -> Option<Finished> {
        let mut state = self.state.borrow_mut();
        let mut pending = state.pending.remove(&request)?;
        let result = match (&mut pending.shape, settlement) {
            (Shape::Mutations { remaining, reply }, BrowserSettlement::Mutation(settlement)) => {
                match settlement {
                    ExtensionBrowserRequestSettlement::Rejected(rejection) => {
                        Err(rejection_error(rejection))
                    }
                    ExtensionBrowserRequestSettlement::Applied(result) => {
                        if let ExtensionBrowserRequestResult::CreatedTab(item) = result {
                            let id = state.tabs.assign(item);
                            if matches!(reply, Reply::Created) {
                                *reply = Reply::Tab(id);
                            }
                        }
                        if let Some(action) = remaining.pop_front() {
                            state
                                .outgoing
                                .push_back(Outgoing::Engine(BrowserCall::Mutate {
                                    request,
                                    action,
                                }));
                            state.pending.insert(request, pending);
                            return None;
                        }
                        {
                            let reply = reply.clone();
                            Ok(state.mutation_reply(&reply, &pending))
                        }
                    }
                }
            }
            (Shape::Data { method, args }, BrowserSettlement::Data(reply)) => {
                data_reply(method, args, &reply)
            }
            (Shape::Prompt(request), BrowserSettlement::Prompt(granted)) => {
                if granted {
                    return Some(Finished::Grant {
                        responder: pending.responder,
                        install: pending.install,
                        request: request.clone(),
                    });
                }
                Ok(Value::Bool(false))
            }
            (Shape::Capture, BrowserSettlement::Capture(result)) => {
                result.map(Value::String).map_err(ApiError::other)
            }
            _ => Err(ApiError::context_invalidated()),
        };
        Some(Finished::Reply(pending.responder, result))
    }

    /// The bridge assigned `bridge_request` to one frame step.
    pub(crate) fn injection_sent(&self, pending: u64, frame: usize, bridge_request: u64) {
        self.state
            .borrow_mut()
            .injections
            .insert(bridge_request, (pending, frame));
    }

    /// An injection step could not be sent.
    pub(crate) fn injection_failed(&self, pending: u64, frame: usize) -> Option<Finished> {
        self.step_done(
            pending,
            frame,
            Err(ApiError::other("Frame with the given id was not found.")),
        )
    }

    pub(crate) fn injection_result(&self, result: &InjectionResult) -> Option<Finished> {
        let (pending, frame) = self
            .state
            .borrow_mut()
            .injections
            .remove(&result.request_id)?;
        let outcome = match result.status {
            InjectionStatus::Success => Ok(ExecutionResult::Value(
                serde_json::from_str(&result.result_json).unwrap_or(Value::Null),
            )),
            InjectionStatus::EvaluationFailed | InjectionStatus::TimedOut => {
                Ok(ExecutionResult::Exception)
            }
            InjectionStatus::StaleTarget => Err(ApiError::other("Frame was removed.")),
            InjectionStatus::Denied => Err(ApiError::cannot_access_page()),
            InjectionStatus::Oversized => Err(ApiError::message_too_large()),
            InjectionStatus::InvalidArguments => Err(ApiError::malformed_arguments()),
            InjectionStatus::Unsupported => Err(unsupported()),
        };
        self.step_done(pending, frame, outcome)
    }

    fn step_done(
        &self,
        id: u64,
        frame: usize,
        outcome: Result<ExecutionResult, ApiError>,
    ) -> Option<Finished> {
        let mut state = self.state.borrow_mut();
        let pending = state.pending.get_mut(&id)?;
        let install = pending.install;
        let Shape::Inject { jobs, .. } = &mut pending.shape else {
            return None;
        };
        let job = jobs.get_mut(frame)?;
        let next = if outcome.is_ok() {
            job.steps.pop_front()
        } else {
            job.steps.clear();
            None
        };
        job.result = Some(outcome);
        if let Some(step) = next {
            let target = Box::new(job.frame.clone());
            state.outgoing.push_back(Outgoing::Inject {
                pending: id,
                frame,
                tab: target.tab,
                target,
                install,
                step: Box::new(step),
            });
            return None;
        }
        if jobs
            .iter()
            .any(|job| job.result.is_none() || !job.steps.is_empty())
        {
            return None;
        }
        let pending = state.pending.remove(&id)?;
        let Shape::Inject { request, jobs } = pending.shape else {
            return None;
        };
        let results = jobs
            .into_iter()
            .map(|job| FrameResult {
                frame: job.frame,
                result: job
                    .result
                    .unwrap_or_else(|| Err(ApiError::context_invalidated())),
            })
            .collect();
        Some(Finished::Reply(
            pending.responder,
            scripting::reply(&request, results),
        ))
    }
}

fn action_defaults(facts: &InstallFacts) -> ActionStore {
    let mut store = ActionStore::default();
    let manifest = &facts.manifest;
    let action = manifest
        .get("action")
        .filter(|action| action.is_object())
        .unwrap_or(&Value::Null);
    let title = action
        .get("default_title")
        .and_then(Value::as_str)
        .filter(|title| !title.is_empty())
        .or_else(|| manifest.get("name").and_then(Value::as_str))
        .unwrap_or_default();
    let _ = store.apply(None, &action::Update::Title(title.to_owned()));
    if let Some(popup) = action
        .get("default_popup")
        .and_then(Value::as_str)
        .filter(|popup| !popup.is_empty())
        .and_then(|popup| extension_url(&facts.extension_id, popup))
    {
        let _ = store.apply(None, &action::Update::Popup(popup));
    }
    if action.get("default_state").and_then(Value::as_str) == Some("disabled") {
        let _ = store.apply(None, &action::Update::Enabled(false));
    }
    let icon = action
        .get("default_icon")
        .or_else(|| manifest.get("icons"))
        .and_then(best_icon_path);
    if let Some(raster) = icon.and_then(|path| super::icons::package_icon(&facts.root, &path)) {
        store.set_raster_icon(None, raster);
    }
    store
}

fn best_icon_path(icon: &Value) -> Option<String> {
    if let Some(path) = icon.as_str() {
        return Some(path.to_owned());
    }
    icon.as_object()?
        .iter()
        .filter_map(|(size, path)| Some((size.parse::<u32>().ok()?, path.as_str()?)))
        .min_by_key(|(size, _)| size.abs_diff(32) * 2 + u32::from(*size < 32))
        .map(|(_, path)| path.to_owned())
}

fn rejection_error(rejection: ExtensionBrowserRequestRejection) -> ApiError {
    match rejection {
        ExtensionBrowserRequestRejection::Unsupported => unsupported(),
        ExtensionBrowserRequestRejection::TabDiscarded => {
            ApiError::other("Cannot access contents of a discarded tab.")
        }
        ExtensionBrowserRequestRejection::CapacityExceeded => ApiError::too_many_pending(),
        _ => ApiError::other("The browser declined the request."),
    }
}

fn data_reply(method: &str, args: &[Value], reply: &ExtensionDataReply) -> Result<Value, ApiError> {
    match reply {
        ExtensionDataReply::Unavailable => {
            return Err(ApiError::other("This data is unavailable in this profile."))
        }
        ExtensionDataReply::Busy => return Err(ApiError::too_many_pending()),
        _ => {}
    }
    if method.starts_with("history.") {
        history::reply(method, reply)
    } else if method.starts_with("bookmarks.") {
        bookmarks::reply(method, args, reply)
    } else if method.starts_with("sessions.") {
        sessions::reply(method, reply, None)
    } else if method.starts_with("topSites.") {
        top_sites::reply(reply)
    } else {
        Err(unsupported())
    }
}

fn diff(previous: &BrowserSnapshot, current: &BrowserSnapshot) -> Vec<TabEvent> {
    let mut events = Vec::new();
    for tab in &previous.tabs {
        if current.tab(tab.id).is_err() {
            events.push(TabEvent::Removed {
                tab: tab.id,
                window: tab.window_id,
            });
        }
    }
    for tab in &current.tabs {
        match previous.tab(tab.id) {
            Err(_) => events.push(TabEvent::Created(Box::new(tab.clone()))),
            Ok(before) => {
                if tab.active && !before.active {
                    events.push(TabEvent::Activated {
                        tab: tab.id,
                        window: tab.window_id,
                    });
                }
                events.push(TabEvent::Updated {
                    previous: Box::new(before.clone()),
                    current: Box::new(tab.clone()),
                });
            }
        }
    }
    for tab in &current.tabs {
        if tab.active && previous.tab(tab.id).is_err() {
            events.push(TabEvent::Activated {
                tab: tab.id,
                window: tab.window_id,
            });
        }
    }
    events
}

impl BrowserState {
    fn snapshot(&self) -> Result<BrowserSnapshot, ApiError> {
        let Some(surface) = self.surface.as_ref() else {
            // Before the shell publishes its first surface there are no tabs.
            return Ok(BrowserSnapshot {
                profile: self.profile.ok_or_else(ApiError::context_invalidated)?,
                generation: zephium_core::extensions::ExtensionBrowserSurfaceGeneration::new(1)
                    .ok_or_else(ApiError::context_invalidated)?,
                tabs: Vec::new(),
                windows: Vec::new(),
                current_window: None,
                last_focused_window: None,
            });
        };
        let mut snapshot = BrowserSnapshot::from_surface(
            surface,
            |item| self.tabs.get(item),
            |window| self.windows.get(window),
            |item| {
                let id = self.tabs.get(item)?;
                Some(
                    match self.frames.get(&item).and_then(|f| f.get(&FrameId::MAIN)) {
                        Some(frame) => (frame.document, document_id(frame.document)),
                        None => {
                            let document = DocumentGeneration::new(
                                SYNTHETIC_DOCUMENT | u64::from(id.unsigned_abs()),
                            )?;
                            (document, document_id(document))
                        }
                    },
                )
            },
        )?;
        for tab in &mut snapshot.tabs {
            if let Some(frame) = self
                .frames
                .get(&tab.tab.id())
                .and_then(|frames| frames.get(&FrameId::MAIN))
            {
                if frame.url.scheme() == "chrome-extension" && tab.tab.url().is_none() {
                    tab.native_url = Some(frame.url.clone());
                }
            }
        }
        Ok(snapshot)
    }

    fn mutation_reply(&self, reply: &Reply, pending: &Pending) -> Value {
        let Ok(snapshot) = self.snapshot() else {
            return Value::Null;
        };
        let project = |id: i32| {
            snapshot
                .tab(id)
                .ok()
                .map(|tab| tabs::project_for(tab, &pending.sender, &pending.permissions))
        };
        match reply {
            Reply::Null => Value::Null,
            Reply::Created => Value::Null,
            Reply::Tab(id) => project(*id).unwrap_or_else(|| {
                json!({"id": id, "index": 0, "windowId": snapshot.current_window.unwrap_or(-1),
                    "active": true, "highlighted": true, "selected": true, "pinned": false,
                    "audible": false, "discarded": false, "autoDiscardable": true, "frozen": false,
                    "incognito": false, "groupId": -1, "splitViewId": -1, "status": "loading",
                    "mutedInfo": {"muted": false}})
            }),
            Reply::Tabs(ids, single) => {
                let values: Vec<Value> = ids.iter().filter_map(|id| project(*id)).collect();
                if *single {
                    values.into_iter().next().unwrap_or(Value::Null)
                } else {
                    Value::Array(values)
                }
            }
            Reply::Window(id) => snapshot
                .windows
                .iter()
                .find(|window| window.id == *id)
                .map(|window| {
                    windows::project_for(
                        &snapshot,
                        window,
                        false,
                        &pending.sender,
                        &pending.permissions,
                    )
                })
                .unwrap_or(Value::Null),
        }
    }

    fn browser_frames(&self, tab: ItemId) -> Vec<BrowserFrame> {
        let Some(frames) = self.frames.get(&tab) else {
            return Vec::new();
        };
        frames
            .iter()
            .map(|(id, frame)| {
                let origin = if frame.opaque {
                    DocumentOrigin::Opaque {
                        precursor: frame
                            .creator
                            .as_ref()
                            .and_then(zephium_webext_runtime::pattern::TupleOrigin::of_url),
                    }
                } else {
                    match &frame.creator {
                        Some(creator) if !matches!(frame.url.scheme(), "http" | "https") => {
                            DocumentOrigin::of_url(creator)
                        }
                        _ => DocumentOrigin::of_url(&frame.url),
                    }
                };
                let parent = frame
                    .parent
                    .and_then(|parent| frames.get(&parent).map(|p| (parent, p)));
                BrowserFrame {
                    tab,
                    frame: *id,
                    document: frame.document,
                    document_id: document_id(frame.document),
                    url: frame.url.clone(),
                    origin,
                    parent_frame_id: parent.map(|(id, _)| id),
                    parent_document_id: parent.map(|(_, p)| document_id(p.document)),
                    document_lifecycle: "active".into(),
                    frame_type: if id.is_main() {
                        "outermost_frame".into()
                    } else {
                        "sub_frame".into()
                    },
                    error_occurred: false,
                }
            })
            .collect()
    }

    /// Chrome's unsupported-function error; the host counts these.
    fn refuse(&mut self, invocation: Invocation<'_>) -> Handled {
        invocation.reply(Err(unsupported()))
    }

    fn pend(
        &mut self,
        invocation: Invocation<'_>,
        shape: Shape,
        first: Option<Outgoing>,
    ) -> Handled {
        if self.pending.len() >= MAX_PENDING {
            return invocation.reply(Err(ApiError::too_many_pending()));
        }
        let sender = invocation.sender().clone();
        let permissions = invocation.permissions().clone();
        let (handled, responder) = invocation.defer();
        self.next = self.next.wrapping_add(1);
        let id = self.next;
        let first = first.map(|first| match first {
            Outgoing::Engine(call) => Outgoing::Engine(with_request(call, id)),
            Outgoing::Inject {
                frame,
                tab,
                target,
                install,
                step,
                ..
            } => Outgoing::Inject {
                pending: id,
                frame,
                tab,
                target,
                install,
                step,
            },
        });
        self.pending.insert(
            id,
            Pending {
                responder,
                install: sender.install(),
                sender,
                permissions,
                shape,
            },
        );
        if let Some(first) = first {
            self.outgoing.push_back(first);
        }
        handled
    }

    fn pend_injection(
        &mut self,
        invocation: Invocation<'_>,
        request: scripting::Request,
        jobs: Vec<FrameJob>,
    ) -> Handled {
        if self.pending.len() >= MAX_PENDING {
            return invocation.reply(Err(ApiError::too_many_pending()));
        }
        let install = invocation.sender().install();
        let sender = invocation.sender().clone();
        let permissions = invocation.permissions().clone();
        let (handled, responder) = invocation.defer();
        self.next = self.next.wrapping_add(1);
        let id = self.next;
        let mut jobs = jobs;
        for (index, job) in jobs.iter_mut().enumerate() {
            if let Some(step) = job.steps.pop_front() {
                self.outgoing.push_back(Outgoing::Inject {
                    pending: id,
                    frame: index,
                    tab: job.frame.tab,
                    target: Box::new(job.frame.clone()),
                    install,
                    step: Box::new(step),
                });
            }
        }
        self.pending.insert(
            id,
            Pending {
                responder,
                install,
                sender,
                permissions,
                shape: Shape::Inject { request, jobs },
            },
        );
        handled
    }

    fn mutate(
        &mut self,
        invocation: Invocation<'_>,
        mut actions: VecDeque<CoreAction>,
        reply: Reply,
    ) -> Handled {
        let Some(first) = actions.pop_front() else {
            let value = match reply {
                Reply::Tab(id) => {
                    let sender = invocation.sender().clone();
                    let permissions = invocation.permissions().clone();
                    self.snapshot()
                        .ok()
                        .and_then(|s| {
                            s.tab(id)
                                .ok()
                                .map(|t| tabs::project_for(t, &sender, &permissions))
                        })
                        .unwrap_or(Value::Null)
                }
                _ => Value::Null,
            };
            return invocation.reply(Ok(value));
        };
        self.pend(
            invocation,
            Shape::Mutations {
                remaining: actions,
                reply,
            },
            Some(Outgoing::Engine(BrowserCall::Mutate {
                request: 0,
                action: first,
            })),
        )
    }

    fn tabs_request(&mut self, request: tabs::Request, invocation: Invocation<'_>) -> Handled {
        use tabs::Request;
        match request {
            Request::Create { action, .. } => {
                self.mutate(invocation, VecDeque::from([action]), Reply::Created)
            }
            Request::Update {
                tab_id, actions, ..
            } => self.mutate(invocation, actions.into(), Reply::Tab(tab_id)),
            Request::Remove { actions, .. } => self.mutate(invocation, actions.into(), Reply::Null),
            Request::Reload { action, .. }
            | Request::Navigate { action, .. }
            | Request::SetZoom { action, .. } => {
                self.mutate(invocation, VecDeque::from([action]), Reply::Null)
            }
            Request::Highlight {
                window_id, action, ..
            } => self.mutate(
                invocation,
                VecDeque::from([action]),
                Reply::Window(window_id),
            ),
            Request::Duplicate { action, .. } => {
                self.mutate(invocation, VecDeque::from([action]), Reply::Created)
            }
            Request::Move {
                tab_ids,
                single,
                action,
                ..
            } => match action {
                Some(action) => self.mutate(
                    invocation,
                    VecDeque::from([action]),
                    Reply::Tabs(tab_ids, single),
                ),
                None => self.refuse(invocation),
            },
            Request::Capture {
                tab_id,
                format,
                quality,
                ..
            } => {
                let Some(tab) = self.tabs.key(tab_id) else {
                    return invocation
                        .reply(Err(ApiError::other(format!("No tab with id: {tab_id}."))));
                };
                self.pend(
                    invocation,
                    Shape::Capture,
                    Some(Outgoing::Engine(BrowserCall::Capture {
                        request: 0,
                        tab,
                        jpeg: format != "png",
                        quality,
                    })),
                )
            }
            Request::DetectLanguage { .. } => invocation.reply(Ok(json!("und"))),
            Request::Discard { .. } => self.refuse(invocation),
        }
    }

    fn windows_request(
        &mut self,
        request: windows::Request,
        invocation: Invocation<'_>,
    ) -> Handled {
        match request {
            windows::Request::Update { window_id, .. } => {
                let sender = invocation.sender().clone();
                let permissions = invocation.permissions().clone();
                let value = self.snapshot().ok().and_then(|snapshot| {
                    snapshot
                        .windows
                        .iter()
                        .find(|window| window.id == window_id)
                        .map(|window| {
                            windows::project_for(&snapshot, window, false, &sender, &permissions)
                        })
                });
                match value {
                    Some(value) => invocation.reply(Ok(value)),
                    None => invocation.reply(Err(ApiError::other(format!(
                        "No window with id: {window_id}."
                    )))),
                }
            }
            windows::Request::Create { .. } | windows::Request::Remove { .. } => {
                self.refuse(invocation)
            }
        }
    }

    fn action_request(&mut self, request: action::Request, invocation: Invocation<'_>) -> Handled {
        let install = invocation.sender().install();
        if let action::Request::OpenPopup { tab_id, .. } = request {
            let Some(tab) = self.tabs.key(tab_id) else {
                return invocation
                    .reply(Err(ApiError::other(format!("No tab with id: {tab_id}."))));
            };
            self.outgoing
                .push_back(Outgoing::Engine(BrowserCall::OpenPopup { install, tab }));
            return invocation.reply(Ok(Value::Null));
        }
        if let action::Request::Set {
            tab_id,
            update: action::Update::Icon(action::Icon::Paths(paths)),
        } = &request
        {
            // Chrome resolves icon paths against the calling page's URL.
            let base = invocation.sender().url().clone();
            let raster = self.installs.get(&install).and_then(|facts| {
                let path = paths
                    .iter()
                    .min_by_key(|(size, _)| i32::from(**size).abs_diff(32))
                    .map(|(_, path)| path.clone())?;
                let resolved = base.join(&path).ok()?;
                if resolved.scheme() != "chrome-extension"
                    || resolved.host_str() != Some(facts.extension_id.as_str())
                {
                    return None;
                }
                super::icons::package_icon(&facts.root, resolved.path().trim_start_matches('/'))
            });
            let Some(raster) = raster else {
                return invocation.reply(Err(ApiError::other("Failed to set icon.")));
            };
            return self.set_icon(invocation, *tab_id, raster);
        }
        let Some(store) = self.actions.get_mut(&install) else {
            return invocation.reply(Err(ApiError::other(
                "This extension has no action specified.",
            )));
        };
        let changes = matches!(request, action::Request::Set { .. });
        let result = store.handle(&request);
        if changes && result.is_ok() {
            self.outgoing
                .push_back(Outgoing::Engine(BrowserCall::ActionsChanged));
        }
        invocation.reply(result)
    }

    fn set_icon(
        &mut self,
        invocation: Invocation<'_>,
        tab: Option<i32>,
        raster: ExtensionActionIcon,
    ) -> Handled {
        let install = invocation.sender().install();
        let Some(store) = self.actions.get_mut(&install) else {
            return invocation.reply(Err(ApiError::other(
                "This extension has no action specified.",
            )));
        };
        store.set_raster_icon(tab, raster);
        self.outgoing
            .push_back(Outgoing::Engine(BrowserCall::ActionsChanged));
        invocation.reply(Ok(Value::Null))
    }

    fn scripting_request(
        &mut self,
        request: scripting::Request,
        invocation: Invocation<'_>,
    ) -> Handled {
        let install = invocation.sender().install();
        let Some(facts) = self.installs.get(&install).cloned() else {
            return invocation.reply(Err(ApiError::context_invalidated()));
        };
        let isolated = crate::bridge::world_name(
            install,
            &zephium_webext_runtime::context::InjectedWorld::Isolated,
        );
        let steps: Result<Vec<Step>, ApiError> = match &request {
            scripting::Request::Execute { source, world, .. } => {
                let world = match world {
                    ExecutionWorld::Isolated => isolated,
                    ExecutionWorld::Main => String::new(),
                };
                match source {
                    ScriptSource::Function { source, args } => Ok(vec![Step {
                        world,
                        kind: InjectionKind::Function,
                        source: source.clone(),
                        source_url: format!("chrome-extension://{}/", facts.extension_id),
                        args_json: Value::Array(args.clone()).to_string(),
                    }]),
                    ScriptSource::Files(files) => files
                        .iter()
                        .map(|file| {
                            let (source, url) = read_package_file(&facts, file)?;
                            Ok(Step {
                                world: world.clone(),
                                kind: InjectionKind::Script,
                                source,
                                source_url: url,
                                args_json: "[]".into(),
                            })
                        })
                        .collect(),
                }
            }
            scripting::Request::Css { .. } => Err(unsupported()),
        };
        let steps = match steps {
            Ok(steps) => steps,
            Err(error) => return invocation.reply(Err(error)),
        };
        let jobs: Vec<FrameJob> = request
            .target()
            .frames
            .iter()
            .map(|frame| FrameJob {
                frame: frame.clone(),
                steps: steps.iter().cloned().collect(),
                result: None,
            })
            .collect();
        if jobs.is_empty() || steps.is_empty() {
            return invocation.reply(scripting::reply(&request, Vec::new()));
        }
        self.pend_injection(invocation, request, jobs)
    }
}

/// `fontSettings.getFontList`: the families the system font map offers.
fn font_list() -> Vec<Value> {
    use gtk::pango::prelude::FontFamilyExt;
    use gtk::prelude::WidgetExt;
    const MAX_FONTS: usize = 4096;
    let context = gtk::Label::new(None).pango_context();
    let mut names: Vec<String> = context
        .list_families()
        .iter()
        .map(|family| family.name().to_string())
        .filter(|name| !name.is_empty() && name.len() <= 256)
        .collect();
    names.sort_by_key(|name| name.to_lowercase());
    names.dedup();
    names
        .into_iter()
        .take(MAX_FONTS)
        .map(|name| json!({"fontId": name, "displayName": name}))
        .collect()
}

fn read_package_file(facts: &InstallFacts, file: &str) -> Result<(String, String), ApiError> {
    let missing = || ApiError::other(format!("Could not load file: '{file}'."));
    let path = zephium_webext::manifest::normalize_resource(file)
        .filter(|path| !path.starts_with('_'))
        .ok_or_else(missing)?;
    let full = facts.root.join(&path);
    let metadata = std::fs::symlink_metadata(&full).map_err(|_| missing())?;
    if !metadata.is_file() || metadata.len() > MAX_INJECTED_FILE_BYTES {
        return Err(missing());
    }
    let bytes = std::fs::read(&full).map_err(|_| missing())?;
    let text = String::from_utf8(bytes).map_err(|_| missing())?;
    Ok((
        text,
        format!("chrome-extension://{}/{path}", facts.extension_id),
    ))
}

fn with_request(call: BrowserCall, id: u64) -> BrowserCall {
    match call {
        BrowserCall::Mutate { action, .. } => BrowserCall::Mutate {
            request: id,
            action,
        },
        BrowserCall::Data { call, .. } => BrowserCall::Data { request: id, call },
        BrowserCall::Prompt {
            extension_id,
            warnings,
            permissions,
            patterns,
            ..
        } => BrowserCall::Prompt {
            request: id,
            extension_id,
            warnings,
            permissions,
            patterns,
        },
        BrowserCall::Capture {
            tab, jpeg, quality, ..
        } => BrowserCall::Capture {
            request: id,
            tab,
            jpeg,
            quality,
        },
        other => other,
    }
}

impl Browser {
    fn dispatch(&mut self, request: BrowserRequest, invocation: Invocation<'_>) -> Handled {
        let mut state = self.shared.state.borrow_mut();
        match request {
            BrowserRequest::Core(action) => {
                state.mutate(invocation, VecDeque::from([action]), Reply::Null)
            }
            BrowserRequest::Tabs(request) => state.tabs_request(request, invocation),
            BrowserRequest::Windows(request) => state.windows_request(request, invocation),
            BrowserRequest::Action(request) => state.action_request(request, invocation),
            BrowserRequest::Scripting(request) => state.scripting_request(request, invocation),
            BrowserRequest::Data(call) => {
                let method = invocation.name();
                let args = invocation.args().to_vec();
                state.pend(
                    invocation,
                    Shape::Data { method, args },
                    Some(Outgoing::Engine(BrowserCall::Data { request: 0, call })),
                )
            }
            BrowserRequest::Permissions(
                zephium_webext_runtime::api::permissions::Request::Prompt { request },
            ) => {
                let extension_id = invocation.sender().extension_id().as_str().to_owned();
                let permissions: Vec<String> = request
                    .permissions
                    .iter()
                    .map(|permission| permission.as_str().to_owned())
                    .collect();
                let patterns: Vec<String> = request
                    .origins
                    .iter()
                    .map(|origin| origin.as_str().to_owned())
                    .collect();
                let warnings = request.warnings();
                state.pend(
                    invocation,
                    Shape::Prompt(request),
                    Some(Outgoing::Engine(BrowserCall::Prompt {
                        request: 0,
                        extension_id,
                        warnings,
                        permissions,
                        patterns,
                    })),
                )
            }
            BrowserRequest::Permissions(_) => state.refuse(invocation),
            BrowserRequest::Service(ServiceRequest::QueryIdle(_)) => {
                invocation.reply(Ok(json!("active")))
            }
            BrowserRequest::Service(ServiceRequest::SetIdleInterval(_)) => {
                invocation.reply(Ok(Value::Null))
            }
            BrowserRequest::Service(ServiceRequest::FontList) => {
                invocation.reply(Ok(Value::Array(font_list())))
            }
            BrowserRequest::Notifications(_)
            | BrowserRequest::Service(_)
            | BrowserRequest::Runtime(_) => state.refuse(invocation),
        }
    }
}

impl BrowserPort for Browser {
    fn set_dnr_badge(
        &mut self,
        install: ExtensionInstallId,
        tab_id: i32,
        text: Option<&str>,
    ) -> Result<(), ApiError> {
        let mut state = self.shared.state.borrow_mut();
        let store = state
            .actions
            .get_mut(&install)
            .ok_or_else(ApiError::context_invalidated)?;
        let before = store.get(Some(tab_id), action::Property::BadgeText);
        store.set_dnr_badge(tab_id, text)?;
        if before != store.get(Some(tab_id), action::Property::BadgeText) {
            state
                .outgoing
                .push_back(Outgoing::Engine(BrowserCall::ActionsChanged));
        }
        drop(state);
        self.shared.kick_if_queued();
        Ok(())
    }

    fn snapshot(&self, sender: &Sender) -> Result<BrowserSnapshot, ApiError> {
        let state = self.shared.state.borrow();
        if state.profile != Some(sender.profile()) {
            return Err(ApiError::cannot_access_page());
        }
        state.snapshot()
    }

    fn extension_info(&self, sender: &Sender) -> Result<ExtensionInfo, ApiError> {
        let state = self.shared.state.borrow();
        let commands = state
            .installs
            .get(&sender.install())
            .and_then(|facts| facts.manifest.get("commands")?.as_object().cloned())
            .unwrap_or_default()
            .into_iter()
            .take(100)
            .map(|(name, command)| {
                let description = command
                    .get("description")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                // Shortcuts are not bound on Linux yet, so none is reported.
                json!({"name": name, "description": description, "shortcut": ""})
            })
            .collect();
        Ok(ExtensionInfo {
            commands,
            ..ExtensionInfo::default()
        })
    }

    fn request(&mut self, request: BrowserRequest, invocation: Invocation<'_>) -> Handled {
        let handled = self.dispatch(request, invocation);
        self.shared.kick_if_queued();
        handled
    }

    fn frames(&self, sender: &Sender, tab: ItemId) -> Result<Vec<BrowserFrame>, ApiError> {
        let state = self.shared.state.borrow();
        if state.profile != Some(sender.profile()) {
            return Err(ApiError::cannot_access_page());
        }
        Ok(state.browser_frames(tab))
    }

    fn user_gesture(&self, sender: &Sender) -> bool {
        // The browser's own extension UI (popup, options) is opened and used
        // by the person; page gestures are not visible to the UI process.
        matches!(sender.kind(), ContextKind::Popup | ContextKind::Options)
    }
}

#[cfg(test)]
#[path = "browser_tests.rs"]
mod tests;
