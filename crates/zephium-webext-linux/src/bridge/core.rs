//! The bridge state machine, free of GTK: attachments, the frame tables,
//! claim checks, sender binding, context routing and delivery queues. The
//! GTK host feeds it facts and executes the [`Effect`]s it produces.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::rc::Rc;

use url::Url;
use zephium_core::ids::{ExtensionInstallId, ItemId, ProfileId};
use zephium_webext_runtime::context::{
    Authority, BindError, ClaimedWorld, CommittedDocument, ContextKey, ContextKind,
    DocumentGeneration, ExtensionId, ExtensionScheme, ExtensionView, FrameId, InjectedWorld,
    InstallAdmission, Sender, TabDocument, ViewKind,
};
use zephium_webext_runtime::limits::message_limit;
use zephium_webext_runtime::pattern::DocumentOrigin;
use zephium_webkit_web_process_protocol::protocol::{
    document_origin, encode_ui, Deliver, Detach, Event, FormEvent, PageRole, PolicyUpdate, Post,
    UiMessage, ATTACH_MESSAGE, DELIVER_MESSAGE, DETACH_MESSAGE, MAX_MESSAGE_BYTES, POLICY_MESSAGE,
};

use super::buffer::{Buffered, PostBuffer, BUFFER_TIMEOUT_MS};
use super::claims::{assess, AttributedWorld, ClaimContext, Pending, Refusal, Verdict};
use super::frames::{FrameChange, FrameTable};
use super::plan::{AttachPlan, PlanError, ResolvedPlan};
use super::policy::SealedPolicy;
use super::runtime::{BridgeViewId, CloseReason, DeliverError, FrameLifecycle, ViewRef};
use super::ReplyToken;

/// Views one profile bridge tracks.
pub const MAX_VIEWS: usize = 4096;
/// Bound contexts one profile tracks.
pub const MAX_CONTEXTS: usize = 65_536;
/// Frames queued for one context.
pub const MAX_QUEUED_FRAMES_PER_CONTEXT: usize = 256;
/// Bytes queued for one context.
pub const MAX_QUEUED_BYTES_PER_CONTEXT: usize = 64 * 1024 * 1024;
/// Bytes queued across a profile.
pub const MAX_QUEUED_BYTES_PER_PROFILE: usize = 256 * 1024 * 1024;
/// Deliveries awaiting the module's answer, per context and per view.
pub const MAX_INFLIGHT_PER_CONTEXT: usize = 16;
pub const MAX_INFLIGHT_PER_VIEW: usize = 64;

const TAB_MESSAGE_CAP: u64 = 8 * 1024 * 1024;
const FALLBACK_GENERATION: u64 = 0;

/// Transport-level counts. Refusals are per reason.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BridgeStats {
    /// Posts that produced no sender, by reason.
    pub refused: BTreeMap<Refusal, u64>,
    /// Posts bound and handed to the runtime.
    pub admitted: u64,
    /// Posts that waited for frame records.
    pub buffered: u64,
    /// Frame records the host table refused.
    pub frame_records_refused: u64,
    /// Events of a generation that is no longer current.
    pub stale_events: u64,
    /// Attachments the module refused.
    pub attach_refused: u64,
    /// Attachments that never reached a module.
    pub attach_failed: u64,
    /// Deliveries sent.
    pub delivered: u64,
    /// Deliveries the module refused.
    pub delivery_refused: u64,
    /// Policy snapshots the module refused.
    pub policy_refused: u64,
}

/// How an attachment request ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AttachOutcome {
    Accepted,
    Refused,
    /// No module handled the message.
    Unhandled,
    /// The message never reached a page.
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Outgoing {
    Attach { generation: u64 },
    Detach,
    Deliver { context: ContextKey },
    Policy { policy_generation: u64 },
}

pub(crate) enum Effect {
    Send {
        view: u64,
        name: &'static str,
        bytes: Vec<u8>,
        outgoing: Outgoing,
        policy: Option<Rc<SealedPolicy>>,
    },
    Reply(ReplyToken),
    Receive {
        sender: Sender,
        envelope: Vec<u8>,
    },
    Closed {
        context: ContextKey,
        reason: CloseReason,
    },
    Lifecycle(FrameLifecycle),
    Undeliverable {
        context: ContextKey,
        error: DeliverError,
    },
    Requests {
        view: u64,
        owner: ViewRef,
        observations: Vec<zephium_webkit_web_process_protocol::protocol::RequestEvent>,
    },
    Form {
        view: u64,
        owner: ViewRef,
        frame: FrameId,
        document: DocumentGeneration,
        fields: Vec<(String, String)>,
    },
    StartFallback {
        view: u64,
    },
    PullDeliver {
        view: u64,
        context: ContextKey,
        frame: Vec<u8>,
    },
}

#[derive(Debug)]
pub(crate) enum Role {
    Tab(ItemId),
    Owned(ExtensionView),
}

impl Role {
    fn owner(&self) -> ViewRef {
        match self {
            Self::Tab(tab) => ViewRef::Tab(*tab),
            Self::Owned(view) => ViewRef::Extension {
                install: view.install(),
                kind: view.kind(),
            },
        }
    }

    fn page_role(&self) -> PageRole {
        match self {
            Self::Tab(_) => PageRole::OrdinaryTab,
            Self::Owned(_) => PageRole::ExtensionOwned,
        }
    }

    fn message_cap(&self) -> u64 {
        match self {
            Self::Tab(_) => TAB_MESSAGE_CAP,
            Self::Owned(_) => MAX_MESSAGE_BYTES as u64,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Phase {
    Pending,
    InFlight(u64),
    Live(u64),
    Refused(u64),
    Fallback,
}

struct View {
    role: Role,
    plan: ResolvedPlan,
    attached: Option<(u64, ResolvedPlan)>,
    phase: Phase,
    reattach_on_navigation: bool,
    allow_fallback: bool,
    frames: FrameTable,
    buffer: PostBuffer,
    host_main: Option<Url>,
    fallback_document: Option<DocumentGeneration>,
    contexts: HashSet<ContextKey>,
    inflight: usize,
}

impl View {
    fn generation(&self) -> Option<u64> {
        match self.phase {
            Phase::InFlight(generation) | Phase::Live(generation) => Some(generation),
            _ => None,
        }
    }

    fn attached_plan(&self) -> Option<&ResolvedPlan> {
        let generation = self.generation()?;
        self.attached
            .as_ref()
            .filter(|(attached, _)| *attached == generation)
            .map(|(_, plan)| plan)
    }
}

struct Target {
    view: u64,
    module_frame: u64,
    world: String,
    document_seq: u64,
    generation: u64,
    kind: ContextKind,
    frame: FrameId,
    document: DocumentGeneration,
    queue: VecDeque<Vec<u8>>,
    queued_bytes: usize,
    inflight: usize,
}

enum Admission {
    Bound(Box<(Sender, Target)>),
    Wait(Pending),
    Refused(Refusal),
}

pub(crate) struct Core {
    authority: Authority,
    scheme: ExtensionScheme,
    extensions: HashMap<ExtensionInstallId, ExtensionId>,
    token: String,
    views: BTreeMap<u64, View>,
    contexts: HashMap<ContextKey, Target>,
    next_view: u64,
    next_generation: u64,
    next_document: u64,
    policy: Option<(u64, Rc<SealedPolicy>)>,
    next_policy: u64,
    buffered_bytes: usize,
    queued_bytes: usize,
    stats: BridgeStats,
    closed: bool,
    effects: Vec<Effect>,
}

/// Why a view could not be registered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RegisterError {
    /// The plan is invalid.
    Plan(PlanError),
    /// The authority refused the view.
    Bind(BindError),
    /// Too many views.
    Capacity,
    /// The tab already has a view.
    Duplicate,
    /// The bridge is shut down.
    Closed,
}

impl Core {
    pub fn new(
        profile: ProfileId,
        incognito: bool,
        scheme: ExtensionScheme,
        token: String,
    ) -> Self {
        Self {
            authority: Authority::new(profile, incognito, scheme),
            scheme,
            extensions: HashMap::new(),
            token,
            views: BTreeMap::new(),
            contexts: HashMap::new(),
            next_view: 0,
            next_generation: 0,
            next_document: 0,
            policy: None,
            next_policy: 0,
            buffered_bytes: 0,
            queued_bytes: 0,
            stats: BridgeStats::default(),
            closed: false,
            effects: Vec::new(),
        }
    }

    pub fn take_effects(&mut self) -> Vec<Effect> {
        std::mem::take(&mut self.effects)
    }

    pub fn authority(&self) -> &Authority {
        &self.authority
    }

    pub fn stats(&self) -> &BridgeStats {
        &self.stats
    }

    pub fn has_buffered(&self) -> bool {
        self.views.values().any(|view| !view.buffer.is_empty())
    }

    pub fn phase(&self, view: u64) -> Option<Phase> {
        self.views.get(&view).map(|view| view.phase)
    }

    /// Whether a fallback view has a committed document to pull from.
    pub fn fallback_ready(&self, view: u64) -> bool {
        self.views
            .get(&view)
            .is_some_and(|view| view.phase == Phase::Fallback && view.fallback_document.is_some())
    }

    pub fn views(&self) -> Vec<u64> {
        self.views.keys().copied().collect()
    }

    #[cfg(test)]
    pub fn context_view(&self, context: &ContextKey) -> Option<u64> {
        self.contexts.get(context).map(|target| target.view)
    }

    // ---- installs ----

    pub fn admit(&mut self, admission: InstallAdmission) -> Result<(), BindError> {
        let install = admission.install;
        let extension = admission.extension_id.clone();
        self.authority.admit(admission)?;
        self.extensions.insert(install, extension);
        self.close_install(install);
        Ok(())
    }

    pub fn retire(&mut self, install: ExtensionInstallId) {
        self.authority.retire(install);
        self.extensions.remove(&install);
        self.close_install(install);
    }

    fn close_install(&mut self, install: ExtensionInstallId) {
        let keys: Vec<ContextKey> = self
            .contexts
            .keys()
            .filter(|key| key.install() == install)
            .cloned()
            .collect();
        for key in keys {
            self.close_context(&key, CloseReason::InstallRetired);
        }
    }

    // ---- views ----

    pub fn register_tab(&mut self, tab: ItemId, plan: &AttachPlan) -> Result<u64, RegisterError> {
        if self
            .views
            .values()
            .any(|view| matches!(view.role, Role::Tab(existing) if existing == tab))
        {
            return Err(RegisterError::Duplicate);
        }
        let resolved = ResolvedPlan::resolve(plan, self.authority_scheme(), None)
            .map_err(RegisterError::Plan)?;
        self.register(Role::Tab(tab), resolved, false)
    }

    pub fn register_owned(
        &mut self,
        install: ExtensionInstallId,
        kind: ViewKind,
        plan: &AttachPlan,
        allow_fallback: bool,
    ) -> Result<u64, RegisterError> {
        if self.closed {
            return Err(RegisterError::Closed);
        }
        let extension = self
            .installed_extension(install)
            .ok_or(RegisterError::Bind(BindError::UnknownInstall))?;
        let resolved = ResolvedPlan::resolve(plan, self.authority_scheme(), Some(&extension))
            .map_err(RegisterError::Plan)?;
        if self.views.len() >= MAX_VIEWS {
            return Err(RegisterError::Capacity);
        }
        let view = self
            .authority
            .open_view(install, kind)
            .map_err(RegisterError::Bind)?;
        self.register(Role::Owned(view), resolved, allow_fallback)
    }

    fn installed_extension(&self, install: ExtensionInstallId) -> Option<ExtensionId> {
        self.extensions.get(&install).cloned()
    }

    fn authority_scheme(&self) -> ExtensionScheme {
        self.scheme
    }

    fn register(
        &mut self,
        role: Role,
        plan: ResolvedPlan,
        allow_fallback: bool,
    ) -> Result<u64, RegisterError> {
        if self.closed {
            if let Role::Owned(view) = role {
                self.authority.close_view(view);
            }
            return Err(RegisterError::Closed);
        }
        if self.views.len() >= MAX_VIEWS {
            if let Role::Owned(view) = role {
                self.authority.close_view(view);
            }
            return Err(RegisterError::Capacity);
        }
        self.next_view += 1;
        let id = self.next_view;
        self.views.insert(
            id,
            View {
                role,
                plan,
                attached: None,
                phase: Phase::Pending,
                reattach_on_navigation: false,
                allow_fallback,
                frames: FrameTable::default(),
                buffer: PostBuffer::default(),
                host_main: None,
                fallback_document: None,
                contexts: HashSet::new(),
                inflight: 0,
            },
        );
        self.attach_now(id);
        Ok(id)
    }

    pub fn update_plan(
        &mut self,
        id: u64,
        plan: &AttachPlan,
        immediately: bool,
    ) -> Result<(), RegisterError> {
        let scheme = self.authority_scheme();
        let view = self.views.get(&id).ok_or(RegisterError::Closed)?;
        let owner = match &view.role {
            Role::Owned(owned) => Some(
                self.installed_extension(owned.install())
                    .ok_or(RegisterError::Bind(BindError::UnknownInstall))?,
            ),
            Role::Tab(_) => None,
        };
        let resolved =
            ResolvedPlan::resolve(plan, scheme, owner.as_ref()).map_err(RegisterError::Plan)?;
        let Some(view) = self.views.get_mut(&id) else {
            return Err(RegisterError::Closed);
        };
        view.plan = resolved;
        let unchanged = view
            .attached
            .as_ref()
            .is_some_and(|(_, attached)| *attached == view.plan);
        view.reattach_on_navigation = !unchanged && view.phase != Phase::Fallback;
        let revoked: Vec<ContextKey> = view
            .contexts
            .iter()
            .filter(|key| {
                self.contexts
                    .get(*key)
                    .is_none_or(|target| !Self::still_granted(view, target))
            })
            .cloned()
            .collect();
        for key in revoked {
            self.close_context(&key, CloseReason::Revoked);
        }
        if immediately && !unchanged {
            self.attach_now(id);
        }
        Ok(())
    }

    fn still_granted(view: &View, target: &Target) -> bool {
        let origin = if target.generation == FALLBACK_GENERATION {
            view.host_main
                .as_ref()
                .and_then(|url| document_origin(url.as_str()))
        } else {
            view.frames
                .get(target.module_frame)
                .and_then(|frame| frame.document.origin.clone())
        };
        let Some(origin) = origin else {
            return false;
        };
        if target.world.is_empty() {
            view.plan.admits_page(&origin).is_some()
        } else {
            view.plan.admits_world(&target.world, &origin).is_some()
        }
    }

    pub fn detach(&mut self, id: u64) {
        let Some(view) = self.views.get(&id) else {
            return;
        };
        if let Some(generation) = view.generation() {
            if let Ok(bytes) = encode_ui(&UiMessage::Detach(Detach {
                token: self.token.clone(),
                generation,
            })) {
                self.effects.push(Effect::Send {
                    view: id,
                    name: DETACH_MESSAGE,
                    bytes,
                    outgoing: Outgoing::Detach,
                    policy: None,
                });
            }
        }
        self.reset_view(id, CloseReason::Detached);
        let Some(view) = self.views.remove(&id) else {
            return;
        };
        match view.role {
            Role::Tab(tab) => self.authority.tab_closed(tab),
            Role::Owned(owned) => self.authority.close_view(owned),
        }
    }

    pub fn shutdown(&mut self) {
        for id in self.views() {
            self.detach(id);
        }
        self.closed = true;
        self.policy = None;
    }

    fn attach_now(&mut self, id: u64) {
        if self.closed {
            return;
        }
        let Some(view) = self.views.get(&id) else {
            return;
        };
        if view.phase == Phase::Fallback {
            return;
        }
        let reason = if view.generation().is_some() {
            CloseReason::Reattached
        } else {
            CloseReason::ProcessGone
        };
        self.reset_view(id, reason);
        self.next_generation += 1;
        let generation = self.next_generation;
        let Some(view) = self.views.get_mut(&id) else {
            return;
        };
        let attach = view
            .plan
            .attach(&self.token, generation, view.role.page_role());
        let Ok(bytes) = encode_ui(&UiMessage::Attach(attach)) else {
            view.phase = Phase::Refused(generation);
            self.stats.attach_refused += 1;
            return;
        };
        view.attached = Some((generation, view.plan.clone()));
        view.phase = Phase::InFlight(generation);
        view.reattach_on_navigation = false;
        self.effects.push(Effect::Send {
            view: id,
            name: ATTACH_MESSAGE,
            bytes,
            outgoing: Outgoing::Attach { generation },
            policy: None,
        });
    }

    pub fn attach_settled(&mut self, id: u64, generation: u64, outcome: AttachOutcome) {
        let Some(view) = self.views.get_mut(&id) else {
            return;
        };
        if view.phase != Phase::InFlight(generation) {
            return;
        }
        match outcome {
            AttachOutcome::Accepted => {
                view.phase = Phase::Live(generation);
                let apply = view.plan.apply_policy;
                if apply {
                    if let Some((policy_generation, policy)) = self.policy.clone() {
                        self.send_policy(id, policy_generation, policy);
                    }
                }
            }
            AttachOutcome::Refused | AttachOutcome::Unhandled
                if view.allow_fallback && matches!(view.role, Role::Owned(_)) =>
            {
                self.stats.attach_refused += 1;
                self.reset_view(id, CloseReason::Reattached);
                let document = self.mint_document();
                if let Some(view) = self.views.get_mut(&id) {
                    view.phase = Phase::Fallback;
                    view.attached = None;
                    if let Some(url) = view.host_main.clone() {
                        view.fallback_document = document;
                        let owner = view.role.owner();
                        if let Some(document) = document {
                            self.effects
                                .push(Effect::Lifecycle(FrameLifecycle::DocumentCreated {
                                    view: BridgeViewId(id),
                                    owner,
                                    frame: FrameId::MAIN,
                                    document,
                                    url,
                                }));
                        }
                    }
                }
                self.effects.push(Effect::StartFallback { view: id });
            }
            AttachOutcome::Refused | AttachOutcome::Unhandled => {
                self.stats.attach_refused += 1;
                view.phase = Phase::Refused(generation);
            }
            AttachOutcome::Failed => {
                self.stats.attach_failed += 1;
                view.phase = Phase::Pending;
            }
        }
    }

    /// The main frame started a navigation: a pending attachment applies to
    /// the documents it creates.
    pub fn load_started(&mut self, id: u64) {
        let Some(view) = self.views.get(&id) else {
            return;
        };
        let attach =
            view.reattach_on_navigation || matches!(view.phase, Phase::Pending | Phase::Refused(_));
        if view.phase == Phase::Fallback {
            self.reset_view(id, CloseReason::DocumentGone);
        } else if attach {
            self.attach_now(id);
        }
    }

    /// The host saw the main frame commit `uri`.
    pub fn load_committed(&mut self, id: u64, uri: Option<&str>, now_ms: u64) {
        let fallback = {
            let Some(view) = self.views.get_mut(&id) else {
                return;
            };
            view.host_main = uri.and_then(|uri| Url::parse(uri).ok());
            view.phase == Phase::Fallback
        };
        if fallback {
            self.reset_view(id, CloseReason::DocumentGone);
            let document = self.mint_document();
            let Some(view) = self.views.get_mut(&id) else {
                return;
            };
            view.fallback_document = document;
            if let (Some(document), Some(url)) = (document, view.host_main.clone()) {
                let owner = view.role.owner();
                self.effects
                    .push(Effect::Lifecycle(FrameLifecycle::DocumentCreated {
                        view: BridgeViewId(id),
                        owner,
                        frame: FrameId::MAIN,
                        document,
                        url,
                    }));
            }
        } else {
            self.reassess(id, now_ms);
        }
    }

    /// The view's web process went away or was replaced.
    pub fn process_gone(&mut self, id: u64) {
        let Some(view) = self.views.get_mut(&id) else {
            return;
        };
        if view.phase == Phase::Fallback {
            view.host_main = None;
            self.reset_view(id, CloseReason::ProcessGone);
            return;
        }
        self.reset_view(id, CloseReason::ProcessGone);
        if let Some(view) = self.views.get_mut(&id) {
            view.phase = Phase::Pending;
            view.host_main = None;
        }
        self.attach_now(id);
    }

    /// Closes every context and frame of a view and answers its waiting
    /// posts.
    fn reset_view(&mut self, id: u64, reason: CloseReason) {
        let Some(view) = self.views.get_mut(&id) else {
            return;
        };
        let mut changes = Vec::new();
        view.frames.clear(&mut changes);
        view.fallback_document = None;
        let buffered = view.buffer.take();
        let had_state = !changes.is_empty() || !view.contexts.is_empty() || !buffered.is_empty();
        let contexts: Vec<ContextKey> = view.contexts.iter().cloned().collect();
        let owner = view.role.owner();
        for entry in buffered {
            self.buffered_bytes = self.buffered_bytes.saturating_sub(entry.post.bytes.len());
            self.refuse(entry.reply, Refusal::Closed);
        }
        self.apply_changes(id, changes, reason);
        for key in contexts {
            self.close_context(&key, reason);
        }
        if had_state {
            self.effects
                .push(Effect::Lifecycle(FrameLifecycle::ViewReset {
                    view: BridgeViewId(id),
                    owner,
                    reason,
                }));
        }
    }

    // ---- inbound ----

    /// One decoded module event delivered by view `id`.
    pub fn event(&mut self, id: u64, event: Event, reply: ReplyToken, now_ms: u64) {
        let Some(view) = self.views.get(&id) else {
            self.refuse(reply, Refusal::Closed);
            return;
        };
        let current = view.generation();
        match event {
            Event::Post(post) => {
                let deadline = now_ms.saturating_add(BUFFER_TIMEOUT_MS);
                self.post(id, post, reply, deadline, now_ms);
                return;
            }
            Event::Attached { generation } if Some(generation) == current => {}
            Event::Frames { generation, events } if Some(generation) == current => {
                let mut changes = Vec::new();
                let mut refused = 0;
                if let Some(view) = self.views.get_mut(&id) {
                    for event in &events {
                        let next = &mut self.next_document;
                        let mut mint = || {
                            *next = next.checked_add(1)?;
                            DocumentGeneration::new(*next)
                        };
                        if view.frames.apply(event, &mut mint, &mut changes).is_err() {
                            refused += 1;
                        }
                    }
                }
                self.stats.frame_records_refused += refused;
                self.apply_changes(id, changes, CloseReason::DocumentGone);
                self.reassess(id, now_ms);
            }
            Event::Requests {
                generation,
                observations,
            } if Some(generation) == current => {
                if view
                    .attached_plan()
                    .is_some_and(|plan| plan.observe_requests)
                {
                    self.effects.push(Effect::Requests {
                        view: id,
                        owner: view.role.owner(),
                        observations,
                    });
                }
            }
            Event::Form(form) if Some(form.generation) == current => self.form(id, form),
            _ => self.stats.stale_events += 1,
        }
        self.effects.push(Effect::Reply(reply));
    }

    /// A transport message from view `id` that was not a valid event.
    pub fn malformed(&mut self, reply: ReplyToken) {
        self.refuse(reply, Refusal::Malformed);
    }

    fn form(&mut self, id: u64, form: FormEvent) {
        let Some(view) = self.views.get(&id) else {
            return;
        };
        if !view.attached_plan().is_some_and(|plan| plan.observe_forms) {
            return;
        }
        let Some(frame) = view
            .frames
            .get(form.frame_id)
            .filter(|frame| frame.document.seq == form.document_seq)
        else {
            self.stats.stale_events += 1;
            return;
        };
        self.effects.push(Effect::Form {
            view: id,
            owner: view.role.owner(),
            frame: frame.chrome,
            document: frame.document.generation,
            fields: form.fields,
        });
    }

    fn post(&mut self, id: u64, post: Post, reply: ReplyToken, deadline_ms: u64, now_ms: u64) {
        let (sender, target) = match self.admit_post(id, &post) {
            Admission::Bound(bound) => *bound,
            Admission::Refused(refusal) => {
                self.refuse(reply, refusal);
                return;
            }
            Admission::Wait(pending) => {
                if now_ms >= deadline_ms {
                    self.refuse(reply, pending.refusal());
                    return;
                }
                let size = post.bytes.len();
                let entry = Buffered {
                    post,
                    reply,
                    deadline_ms,
                };
                let held = self.buffered_bytes;
                let Some(view) = self.views.get_mut(&id) else {
                    self.refuse(entry.reply, Refusal::Closed);
                    return;
                };
                match view.buffer.push(entry, held) {
                    Ok(()) => {
                        self.buffered_bytes += size;
                        self.stats.buffered += 1;
                    }
                    Err(entry) => self.refuse(entry.reply, Refusal::BufferFull),
                }
                return;
            }
        };
        if !self.bind_target(id, sender.context_key(), target) {
            self.refuse(reply, Refusal::Unbound);
            return;
        }
        self.stats.admitted += 1;
        self.effects.push(Effect::Reply(reply));
        self.effects.push(Effect::Receive {
            sender,
            envelope: post.bytes,
        });
    }

    /// Checks a post's claims and binds its sender, changing nothing.
    fn admit_post(&self, id: u64, post: &Post) -> Admission {
        if self.closed {
            return Admission::Refused(Refusal::Closed);
        }
        let Some(view) = self.views.get(&id) else {
            return Admission::Refused(Refusal::Closed);
        };
        if view.phase == Phase::Fallback {
            return Admission::Refused(Refusal::Unattached);
        }
        let Some(generation) = view.generation() else {
            return Admission::Refused(Refusal::Unattached);
        };
        let context = ClaimContext {
            owned: matches!(view.role, Role::Owned(_)),
            generation: Some(generation),
            attached: view.attached_plan(),
            plan: &view.plan,
            frames: &view.frames,
            host_main: view.host_main.as_ref(),
        };
        let attributed = match assess(&context, post) {
            Verdict::Attributed(attributed) => attributed,
            Verdict::Refused(refusal) => return Admission::Refused(refusal),
            Verdict::Wait(pending) => return Admission::Wait(pending),
        };
        let frame = attributed.frame.chrome;
        let document = attributed.frame.document.generation;
        let committed = CommittedDocument {
            generation: document,
            url: attributed.url,
            origin: attributed.origin,
        };
        let bound = match (&view.role, attributed.world) {
            (Role::Tab(tab), world) => {
                let claimed = match world {
                    AttributedWorld::Page => ClaimedWorld::Page,
                    AttributedWorld::Script {
                        install,
                        world: InjectedWorld::Isolated,
                    } => ClaimedWorld::Isolated(install),
                    AttributedWorld::Script {
                        install,
                        world: InjectedWorld::UserScript(world),
                    } => ClaimedWorld::UserScript { install, world },
                };
                self.authority.bind_tab(
                    &TabDocument {
                        tab: *tab,
                        frame,
                        committed,
                    },
                    &claimed,
                )
            }
            (Role::Owned(owned), AttributedWorld::Page) => {
                self.authority.bind_view(owned, &committed)
            }
            (Role::Owned(_), AttributedWorld::Script { .. }) => Err(BindError::NoExtensionContext),
        };
        let sender = match bound {
            Ok(sender) => sender,
            Err(BindError::NotInjected) => return Admission::Refused(Refusal::NotInjected),
            Err(_) => return Admission::Refused(Refusal::Unbound),
        };
        let cap = message_limit(sender.kind()).min(view.role.message_cap());
        if post.bytes.len() as u64 > cap {
            return Admission::Refused(Refusal::Oversized);
        }
        let target = Target {
            view: id,
            module_frame: post.frame_id,
            world: post.world.clone(),
            document_seq: post.document_seq,
            generation,
            kind: sender.kind(),
            frame,
            document,
            queue: VecDeque::new(),
            queued_bytes: 0,
            inflight: 0,
        };
        Admission::Bound(Box::new((sender, target)))
    }

    fn bind_target(&mut self, id: u64, key: ContextKey, target: Target) -> bool {
        match self.contexts.get_mut(&key) {
            Some(existing) => {
                let same = existing.view == target.view
                    && existing.module_frame == target.module_frame
                    && existing.world == target.world
                    && existing.document_seq == target.document_seq
                    && existing.generation == target.generation;
                if !same {
                    return false;
                }
            }
            None => {
                if self.contexts.len() >= MAX_CONTEXTS {
                    return false;
                }
                self.contexts.insert(key.clone(), target);
                if let Some(view) = self.views.get_mut(&id) {
                    view.contexts.insert(key);
                }
            }
        }
        true
    }

    /// A host-pulled envelope from an extension-owned view in fallback.
    pub fn fallback_post(&mut self, id: u64, envelope: Vec<u8>) -> Result<(), Refusal> {
        let view = self.views.get(&id).ok_or(Refusal::Closed)?;
        let (Role::Owned(owned), Phase::Fallback) = (&view.role, view.phase) else {
            return Err(Refusal::Unattached);
        };
        let (Some(document), Some(url)) = (view.fallback_document, view.host_main.clone()) else {
            return Err(Refusal::HostMismatch);
        };
        let origin = document_origin(url.as_str()).ok_or(Refusal::NotAdmitted)?;
        let pages = view.plan.admits_page(&origin).ok_or(Refusal::NotAdmitted)?;
        let origin = if pages.sandboxed.contains(url.path()) {
            DocumentOrigin::opaque(Some(&url))
        } else {
            DocumentOrigin::of_url(&url)
        };
        let committed = CommittedDocument {
            generation: document,
            url,
            origin,
        };
        let sender = self
            .authority
            .bind_view(owned, &committed)
            .map_err(|_| Refusal::Unbound)?;
        if envelope.len() as u64 > message_limit(sender.kind()) {
            return Err(Refusal::Oversized);
        }
        let key = sender.context_key();
        let target = Target {
            view: id,
            module_frame: 0,
            world: String::new(),
            document_seq: 0,
            generation: FALLBACK_GENERATION,
            kind: sender.kind(),
            frame: FrameId::MAIN,
            document,
            queue: VecDeque::new(),
            queued_bytes: 0,
            inflight: 0,
        };
        if !self.bind_target(id, key, target) {
            return Err(Refusal::Unbound);
        }
        self.stats.admitted += 1;
        self.effects.push(Effect::Receive { sender, envelope });
        Ok(())
    }

    /// Re-evaluates waiting posts after host state moved, and expires them.
    pub fn reassess(&mut self, id: u64, now_ms: u64) {
        let Some(view) = self.views.get_mut(&id) else {
            return;
        };
        if view.buffer.is_empty() {
            return;
        }
        let entries = view.buffer.take();
        for entry in entries {
            self.buffered_bytes = self.buffered_bytes.saturating_sub(entry.post.bytes.len());
            self.post(id, entry.post, entry.reply, entry.deadline_ms, now_ms);
        }
    }

    pub fn tick(&mut self, now_ms: u64) {
        for id in self.views() {
            self.reassess(id, now_ms);
        }
    }

    fn refuse(&mut self, reply: ReplyToken, refusal: Refusal) {
        *self.stats.refused.entry(refusal).or_default() += 1;
        self.effects.push(Effect::Reply(reply));
    }

    fn mint_document(&mut self) -> Option<DocumentGeneration> {
        self.next_document = self.next_document.checked_add(1)?;
        DocumentGeneration::new(self.next_document)
    }

    fn apply_changes(&mut self, id: u64, changes: Vec<FrameChange>, reason: CloseReason) {
        let Some(view) = self.views.get(&id) else {
            return;
        };
        let owner = view.role.owner();
        let tab = match view.role {
            Role::Tab(tab) => Some(tab),
            Role::Owned(_) => None,
        };
        for change in changes {
            match change {
                FrameChange::Created {
                    frame,
                    main,
                    document,
                    url,
                    ..
                } => {
                    if let Some(tab) = tab {
                        self.authority.document_committed(tab, frame, document);
                        self.record_injections(id, tab, frame, document, &url);
                    }
                    self.effects
                        .push(Effect::Lifecycle(FrameLifecycle::DocumentCreated {
                            view: BridgeViewId(id),
                            owner,
                            frame,
                            document,
                            url,
                        }));
                    if let (Some(tab), true) = (tab, main) {
                        self.effects
                            .push(Effect::Lifecycle(FrameLifecycle::MainDocumentChanged {
                                tab,
                                document,
                            }));
                    }
                }
                FrameChange::Loaded { frame, document } => {
                    self.effects
                        .push(Effect::Lifecycle(FrameLifecycle::DocumentLoaded {
                            view: BridgeViewId(id),
                            owner,
                            frame,
                            document,
                        }));
                }
                FrameChange::Removed {
                    frame, document, ..
                } => {
                    if let Some(tab) = tab {
                        self.authority.frame_removed(tab, frame);
                    }
                    let gone: Vec<ContextKey> = self
                        .views
                        .get(&id)
                        .map(|view| {
                            view.contexts
                                .iter()
                                .filter(|key| {
                                    self.contexts.get(*key).is_some_and(|target| {
                                        target.frame == frame && target.document == document
                                    })
                                })
                                .cloned()
                                .collect()
                        })
                        .unwrap_or_default();
                    for key in gone {
                        self.close_context(&key, reason);
                    }
                    self.effects
                        .push(Effect::Lifecycle(FrameLifecycle::DocumentRemoved {
                            view: BridgeViewId(id),
                            owner,
                            frame,
                            document,
                        }));
                }
            }
        }
    }

    /// Records the worlds the module installed the bridge in for this
    /// document: those both the attached plan and the host's current plan
    /// admit for its origin.
    fn record_injections(
        &mut self,
        id: u64,
        tab: ItemId,
        frame: FrameId,
        document: DocumentGeneration,
        url: &Url,
    ) {
        let Some(origin) = document_origin(url.as_str()) else {
            return;
        };
        let Some(view) = self.views.get(&id) else {
            return;
        };
        let Some(attached) = view.attached_plan() else {
            return;
        };
        let grants: Vec<(ExtensionInstallId, InjectedWorld)> = attached
            .worlds
            .iter()
            .filter(|(name, world)| {
                world.origins.contains(&origin) && view.plan.admits_world(name, &origin).is_some()
            })
            .map(|(_, world)| (world.install, world.world.clone()))
            .collect();
        for (install, world) in grants {
            // A retired install's grant stays in a stale plan; it binds nothing.
            let _ = self
                .authority
                .record_injection(tab, frame, document, install, world);
        }
    }

    fn close_context(&mut self, key: &ContextKey, reason: CloseReason) {
        let Some(target) = self.contexts.remove(key) else {
            return;
        };
        self.queued_bytes = self.queued_bytes.saturating_sub(target.queued_bytes);
        if let Some(view) = self.views.get_mut(&target.view) {
            view.contexts.remove(key);
        }
        self.effects.push(Effect::Closed {
            context: key.clone(),
            reason,
        });
    }

    // ---- outbound ----

    pub fn deliver(&mut self, key: &ContextKey, frame: Vec<u8>) -> Result<(), DeliverError> {
        if self.closed {
            return Err(DeliverError::Closed);
        }
        let Some(target) = self.contexts.get(key) else {
            return Err(DeliverError::Stale);
        };
        let Some(view) = self.views.get(&target.view) else {
            return Err(DeliverError::Stale);
        };
        let current = match view.phase {
            Phase::Fallback => target.generation == FALLBACK_GENERATION,
            Phase::Live(generation) => target.generation == generation,
            _ => false,
        };
        if !current {
            self.close_context(key, CloseReason::Reattached);
            return Err(DeliverError::Stale);
        }
        let cap = message_limit(target.kind).min(view.role.message_cap());
        if frame.len() as u64 > cap {
            return Err(DeliverError::TooLarge);
        }
        let id = target.view;
        let Some(target) = self.contexts.get_mut(key) else {
            return Err(DeliverError::Stale);
        };
        if target.queue.len() >= MAX_QUEUED_FRAMES_PER_CONTEXT
            || target.queued_bytes.saturating_add(frame.len()) > MAX_QUEUED_BYTES_PER_CONTEXT
            || self.queued_bytes.saturating_add(frame.len()) > MAX_QUEUED_BYTES_PER_PROFILE
        {
            return Err(DeliverError::Backpressure);
        }
        target.queued_bytes += frame.len();
        self.queued_bytes += frame.len();
        target.queue.push_back(frame);
        self.flush(id);
        Ok(())
    }

    fn flush(&mut self, id: u64) {
        let Some(view) = self.views.get(&id) else {
            return;
        };
        let keys: Vec<ContextKey> = view.contexts.iter().cloned().collect();
        let fallback = view.phase == Phase::Fallback;
        for key in keys {
            loop {
                let Some(view) = self.views.get(&id) else {
                    return;
                };
                if view.inflight >= MAX_INFLIGHT_PER_VIEW {
                    return;
                }
                let Some(target) = self.contexts.get_mut(&key) else {
                    break;
                };
                if target.inflight >= MAX_INFLIGHT_PER_CONTEXT {
                    break;
                }
                let Some(frame) = target.queue.pop_front() else {
                    break;
                };
                target.queued_bytes -= frame.len();
                self.queued_bytes = self.queued_bytes.saturating_sub(frame.len());
                let effect = if fallback {
                    Effect::PullDeliver {
                        view: id,
                        context: key.clone(),
                        frame,
                    }
                } else {
                    let message = UiMessage::Deliver(Deliver {
                        token: self.token.clone(),
                        generation: target.generation,
                        frame_id: target.module_frame,
                        document_seq: target.document_seq,
                        world: target.world.clone(),
                        bytes: frame,
                    });
                    match encode_ui(&message) {
                        Ok(bytes) => Effect::Send {
                            view: id,
                            name: DELIVER_MESSAGE,
                            bytes,
                            outgoing: Outgoing::Deliver {
                                context: key.clone(),
                            },
                            policy: None,
                        },
                        Err(_) => {
                            self.effects.push(Effect::Undeliverable {
                                context: key.clone(),
                                error: DeliverError::TooLarge,
                            });
                            continue;
                        }
                    }
                };
                target.inflight += 1;
                if let Some(view) = self.views.get_mut(&id) {
                    view.inflight += 1;
                }
                self.stats.delivered += 1;
                self.effects.push(effect);
            }
        }
    }

    pub fn delivery_settled(&mut self, id: u64, key: &ContextKey, accepted: bool) {
        if let Some(view) = self.views.get_mut(&id) {
            view.inflight = view.inflight.saturating_sub(1);
        }
        if let Some(target) = self.contexts.get_mut(key) {
            if target.view == id {
                target.inflight = target.inflight.saturating_sub(1);
            }
        }
        if !accepted {
            self.stats.delivery_refused += 1;
            self.effects.push(Effect::Undeliverable {
                context: key.clone(),
                error: DeliverError::Refused,
            });
        }
        self.flush(id);
    }

    // ---- policy ----

    pub fn publish_policy(&mut self, policy: Rc<SealedPolicy>) -> u64 {
        self.next_policy += 1;
        let generation = self.next_policy;
        self.policy = Some((generation, policy.clone()));
        let targets: Vec<u64> = self
            .views
            .iter()
            .filter(|(_, view)| view.plan.apply_policy && matches!(view.phase, Phase::Live(_)))
            .map(|(id, _)| *id)
            .collect();
        for id in targets {
            self.send_policy(id, generation, policy.clone());
        }
        generation
    }

    fn send_policy(&mut self, id: u64, policy_generation: u64, policy: Rc<SealedPolicy>) {
        let Some(Phase::Live(generation)) = self.views.get(&id).map(|view| view.phase) else {
            return;
        };
        let message = UiMessage::Policy(PolicyUpdate {
            token: self.token.clone(),
            generation,
            policy_generation,
            fd_index: 0,
            length: policy.len(),
        });
        if let Ok(bytes) = encode_ui(&message) {
            self.effects.push(Effect::Send {
                view: id,
                name: POLICY_MESSAGE,
                bytes,
                outgoing: Outgoing::Policy { policy_generation },
                policy: Some(policy),
            });
        }
    }

    pub fn policy_settled(&mut self, accepted: bool) {
        if !accepted {
            self.stats.policy_refused += 1;
        }
    }
}
