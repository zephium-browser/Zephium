//! GTK side of a profile bridge: context configuration, view signals, user
//! messages, the pull fallback and teardown.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, VecDeque};
use std::os::fd::BorrowedFd;
use std::path::PathBuf;
use std::rc::{Rc, Weak};
use std::time::{Duration, Instant};

use gio::prelude::*;
use glib::variant::ToVariant;
use glib::VariantTy;
use gtk::prelude::WidgetExt;
use javascriptcore::ValueExt;
use webkit2gtk::{
    LoadEvent, UserMessage, UserMessageError, UserMessageExt, WebContextExt, WebViewExt,
};
use zephium_core::ids::{ExtensionInstallId, ItemId, ProfileId};
use zephium_webext_runtime::context::{
    Authority, BindError, ContextKey, ExtensionScheme, InstallAdmission, ViewKind,
};
use zephium_webkit_web_process_protocol::protocol::{
    decode_delivery_result, decode_event, DeliveryStatus, ScriptTable, EVENT_MESSAGE, MAX_UI_BYTES,
    PROTOCOL_VERSION,
};

use super::core::{
    AttachOutcome, BridgeStats, Core, Effect, InjectionError, Outgoing, Phase, RegisterError,
};
use super::plan::AttachPlan;
use super::policy::{PolicyError, SealedPolicy};
use super::pull::{self, PULL_MIN_INTERVAL};
use super::runtime::{BridgeViewId, DeliverError, Outbox, Runtime};
use super::scripts::{ScriptTableError, SealedScripts};
use super::ReplyToken;

const TICK_INTERVAL: Duration = Duration::from_millis(250);
const DRAIN_QUIET: Duration = Duration::from_millis(100);
const TOKEN_BYTES: usize = 32;

/// How one profile's bridge is set up.
#[derive(Clone, Debug)]
pub struct BridgeConfig {
    /// The profile.
    pub profile: ProfileId,
    /// Whether the profile is incognito.
    pub incognito: bool,
    /// The profile's extension scheme.
    pub scheme: ExtensionScheme,
    /// The directory holding `libzephium_webkit_web_process.so`. It is
    /// mounted read-only into every web process of the context.
    pub module_directory: PathBuf,
}

/// Why a bridge could not be created.
#[derive(Debug)]
pub enum BridgeError {
    /// The module directory is not an absolute UTF-8 path to a directory.
    ModuleDirectory,
    /// No randomness for the process token.
    Random(std::io::Error),
}

/// The runtime is already borrowed by the caller further up the stack.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Busy;

/// When an updated plan reaches the module.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reattach {
    /// Now. A structural widening preserves live contexts; other changes
    /// end contexts until the documents are recreated.
    Now,
    /// When the main frame next navigates. The host applies the new plan
    /// to claims at once, so narrowing takes effect immediately.
    AtNextNavigation,
}

/// How a view's messages travel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Transport {
    /// Waiting for, or sending, an attachment.
    Attaching,
    /// The web-process module.
    Module,
    /// The module refused the attachment; retried at the next navigation.
    Refused,
    /// Host pulls, for an extension-owned view without a usable module.
    Fallback,
}

struct PullState {
    cancellable: RefCell<gio::Cancellable>,
    inflight: Cell<bool>,
    interval: Cell<Duration>,
    timer: RefCell<Option<glib::SourceId>>,
}

impl PullState {
    fn new() -> Self {
        Self {
            cancellable: RefCell::new(gio::Cancellable::new()),
            inflight: Cell::new(false),
            interval: Cell::new(PULL_MIN_INTERVAL),
            timer: RefCell::new(None),
        }
    }

    fn cancel(&self) {
        if let Some(timer) = self.timer.borrow_mut().take() {
            timer.remove();
        }
        let old = self.cancellable.replace(gio::Cancellable::new());
        old.cancel();
        self.inflight.set(false);
    }
}

struct HostView {
    view: glib::WeakRef<webkit2gtk::WebView>,
    handlers: Vec<glib::SignalHandlerId>,
    owned: bool,
    pull: Option<Rc<PullState>>,
}

struct Shared<R> {
    core: RefCell<Core>,
    runtime: RefCell<R>,
    effects: RefCell<VecDeque<Effect>>,
    draining: Cell<bool>,
    hosts: RefCell<HashMap<u64, HostView>>,
    replies: RefCell<HashMap<u64, UserMessage>>,
    next_reply: Cell<u64>,
    pending: Rc<Cell<usize>>,
    tick: RefCell<Option<glib::SourceId>>,
    context: glib::WeakRef<webkit2gtk::WebContext>,
    clock: Instant,
}

/// One profile's bridge. Clones share it. GTK thread only.
pub struct ProfileBridge<R: Runtime + 'static> {
    shared: Rc<Shared<R>>,
}

impl<R: Runtime + 'static> Clone for ProfileBridge<R> {
    fn clone(&self) -> Self {
        Self {
            shared: self.shared.clone(),
        }
    }
}

fn token() -> Result<String, BridgeError> {
    let mut bytes = [0u8; TOKEN_BYTES];
    let mut filled = 0;
    while filled < TOKEN_BYTES {
        match rustix::rand::getrandom(&mut bytes[filled..], rustix::rand::GetRandomFlags::empty()) {
            Ok(0) => {}
            Ok(count) => filled += count,
            Err(rustix::io::Errno::INTR) => {}
            Err(errno) => return Err(BridgeError::Random(errno.into())),
        }
    }
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

impl<R: Runtime + 'static> ProfileBridge<R> {
    /// Configures `context` to load the module and creates the bridge.
    /// Call before the context starts any web process.
    pub fn new(
        context: &webkit2gtk::WebContext,
        config: BridgeConfig,
        runtime: R,
    ) -> Result<Self, BridgeError> {
        let directory = config
            .module_directory
            .to_str()
            .filter(|_| config.module_directory.is_absolute() && config.module_directory.is_dir())
            .ok_or(BridgeError::ModuleDirectory)?;
        let token = token()?;
        context.add_path_to_sandbox(&config.module_directory, true);
        context.set_web_extensions_directory(directory);
        context.set_web_extensions_initialization_user_data(
            &(PROTOCOL_VERSION, token.as_str()).to_variant(),
        );
        Ok(Self {
            shared: Rc::new(Shared {
                core: RefCell::new(Core::new(
                    config.profile,
                    config.incognito,
                    config.scheme,
                    token,
                )),
                runtime: RefCell::new(runtime),
                effects: RefCell::new(VecDeque::new()),
                draining: Cell::new(false),
                hosts: RefCell::new(HashMap::new()),
                replies: RefCell::new(HashMap::new()),
                next_reply: Cell::new(0),
                pending: Rc::new(Cell::new(0)),
                tick: RefCell::new(None),
                context: context.downgrade(),
                clock: Instant::now(),
            }),
        })
    }

    /// Starts a runtime generation of an install. Its existing contexts end.
    pub fn admit(&self, admission: InstallAdmission) -> Result<(), BindError> {
        let result = self.shared.core.borrow_mut().admit(admission);
        self.shared.collect();
        result
    }

    /// Stops an install; its contexts end and its views go stale.
    pub fn retire(&self, install: ExtensionInstallId) {
        self.shared.core.borrow_mut().retire(install);
        self.shared.collect();
    }

    /// Reads the profile's authority.
    pub fn with_authority<T>(&self, read: impl FnOnce(&Authority) -> T) -> T {
        read(self.shared.core.borrow().authority())
    }

    /// Attaches an ordinary tab's view. Call before its first load.
    pub fn attach_tab(
        &self,
        tab: ItemId,
        view: &webkit2gtk::WebView,
        plan: &AttachPlan,
    ) -> Result<BridgeViewId, RegisterError> {
        self.check_view(view)?;
        let result = self.shared.core.borrow_mut().register_tab(tab, plan);
        self.finish_register(view, result, false)
    }

    /// Attaches a runtime-owned extension view. Call before its first load.
    /// With `allow_fallback`, a view whose module refuses or is missing uses
    /// host pulls instead.
    pub fn attach_extension_view(
        &self,
        install: ExtensionInstallId,
        kind: ViewKind,
        view: &webkit2gtk::WebView,
        plan: &AttachPlan,
        allow_fallback: bool,
    ) -> Result<BridgeViewId, RegisterError> {
        self.check_view(view)?;
        let result =
            self.shared
                .core
                .borrow_mut()
                .register_owned(install, kind, plan, allow_fallback);
        self.finish_register(view, result, true)
    }

    fn check_view(&self, view: &webkit2gtk::WebView) -> Result<(), RegisterError> {
        let same_context = view
            .context()
            .zip(self.shared.context.upgrade())
            .is_some_and(|(theirs, ours)| theirs == ours);
        let known = self
            .shared
            .hosts
            .borrow()
            .values()
            .any(|host| host.view.upgrade().as_ref() == Some(view));
        if !same_context {
            return Err(RegisterError::Bind(BindError::WrongProfile));
        }
        if known {
            return Err(RegisterError::Duplicate);
        }
        Ok(())
    }

    fn finish_register(
        &self,
        view: &webkit2gtk::WebView,
        result: Result<u64, RegisterError>,
        owned: bool,
    ) -> Result<BridgeViewId, RegisterError> {
        let id = match result {
            Ok(id) => id,
            Err(error) => {
                self.shared.collect();
                return Err(error);
            }
        };
        let page = Rc::new(Cell::new(view.page_id()));
        let handlers = connect(&self.shared, id, view, page);
        self.shared.hosts.borrow_mut().insert(
            id,
            HostView {
                view: view.downgrade(),
                handlers,
                owned,
                pull: None,
            },
        );
        self.shared.collect();
        Ok(BridgeViewId(id))
    }

    /// Replaces a view's plan.
    pub fn update_plan(
        &self,
        view: BridgeViewId,
        plan: &AttachPlan,
        when: Reattach,
    ) -> Result<(), RegisterError> {
        let result = self
            .shared
            .core
            .borrow_mut()
            .update_plan(view.0, plan, when == Reattach::Now);
        self.shared.collect();
        result
    }

    /// Detaches a view; its contexts end. Tabs leave the authority.
    pub fn detach(&self, view: BridgeViewId) {
        self.shared.core.borrow_mut().detach(view.0);
        self.shared.collect();
        self.shared.forget_host(view.0);
    }

    /// Queues a frame for a bound context.
    pub fn deliver(&self, context: &ContextKey, frame: Vec<u8>) -> Result<(), DeliverError> {
        let result = self.shared.core.borrow_mut().deliver(context, frame);
        self.shared.collect();
        result
    }

    /// Resolves a Chrome frame and host document to an exact native target.
    pub fn frame_target(
        &self,
        view: BridgeViewId,
        frame: zephium_webext_runtime::context::FrameId,
        document: zephium_webext_runtime::context::DocumentGeneration,
    ) -> Option<zephium_webkit_web_process_protocol::protocol::InjectionTarget> {
        self.shared
            .core
            .borrow()
            .frame_target(view.0, frame, document)
    }

    /// Sends a host-authorized execution; results arrive through `Runtime`.
    /// Transport identity fields in `request` are replaced by host facts.
    pub fn inject(
        &self,
        view: BridgeViewId,
        install: ExtensionInstallId,
        request: zephium_webkit_web_process_protocol::protocol::InjectRequest,
    ) -> Result<u64, InjectionError> {
        let result =
            self.shared
                .core
                .borrow_mut()
                .inject(view.0, install, request, self.shared.now_ms());
        self.shared.collect();
        result
    }

    /// Publishes a request-policy snapshot to every attached view whose
    /// plan applies it, now and on every later attachment. Returns its
    /// policy generation.
    pub fn publish_policy(&self, bytes: &[u8]) -> Result<u64, PolicyError> {
        let policy = Rc::new(SealedPolicy::new(bytes)?);
        let generation = self.shared.core.borrow_mut().publish_policy(policy);
        self.shared.collect();
        Ok(generation)
    }

    /// Publishes this profile's immutable script snapshot to future documents.
    pub fn publish_scripts(&self, table: ScriptTable) -> Result<u64, ScriptTableError> {
        let table = Rc::new(SealedScripts::new(table)?);
        let generation = self.shared.core.borrow_mut().publish_scripts(table)?;
        self.shared.collect();
        Ok(generation)
    }

    /// A fallback view has something queued: pull now.
    pub fn ring(&self, view: BridgeViewId) {
        let pull = self
            .shared
            .hosts
            .borrow()
            .get(&view.0)
            .and_then(|host| host.pull.clone());
        if let Some(pull) = pull {
            pull.interval.set(PULL_MIN_INTERVAL);
            if !pull.inflight.get() {
                if let Some(timer) = pull.timer.borrow_mut().take() {
                    timer.remove();
                }
                start_pull(&self.shared, view.0);
            }
        }
    }

    /// Runs `work` against the runtime, then delivers what it queued.
    /// Refused when the runtime is already borrowed further up the stack.
    pub fn with_runtime<T>(&self, work: impl FnOnce(&mut R, &mut Outbox) -> T) -> Result<T, Busy> {
        let mut outbox = Outbox::default();
        let value = {
            let mut runtime = self.shared.runtime.try_borrow_mut().map_err(|_| Busy)?;
            work(&mut runtime, &mut outbox)
        };
        self.shared.deliver_outbox(outbox);
        self.shared.drain();
        Ok(value)
    }

    /// Transport counts.
    pub fn stats(&self) -> BridgeStats {
        self.shared.core.borrow().stats().clone()
    }

    /// How a view's messages travel now.
    pub fn transport(&self, view: BridgeViewId) -> Option<Transport> {
        self.shared
            .core
            .borrow()
            .phase(view.0)
            .map(|phase| match phase {
                Phase::Pending | Phase::InFlight(_) => Transport::Attaching,
                Phase::Live(_) => Transport::Module,
                Phase::Refused(_) => Transport::Refused,
                Phase::Fallback => Transport::Fallback,
            })
    }

    /// Ends the bridge: stops pulls, ends every context (the runtime
    /// cancels calls and disconnects ports), detaches every view,
    /// disconnects its signals and terminates extension-owned views' web
    /// processes. Then the view owners drop their views, and the caller
    /// keeps the GTK loop running until the returned drain settles.
    pub fn shutdown(&self) -> ShutdownDrain {
        for host in self.shared.hosts.borrow().values() {
            if let Some(pull) = &host.pull {
                pull.cancel();
            }
        }
        if let Some(tick) = self.shared.tick.borrow_mut().take() {
            tick.remove();
        }
        self.shared.core.borrow_mut().shutdown();
        self.shared.collect();
        let hosts: Vec<HostView> = self
            .shared
            .hosts
            .borrow_mut()
            .drain()
            .map(|(_, host)| host)
            .collect();
        let mut views = Vec::new();
        for host in hosts {
            if let Some(view) = host.view.upgrade() {
                for handler in host.handlers {
                    view.disconnect(handler);
                }
                if host.owned {
                    view.terminate_web_process();
                }
            }
            views.push(host.view);
        }
        for (_, message) in self.shared.replies.borrow_mut().drain() {
            message.send_reply(&UserMessage::new(EVENT_MESSAGE, None));
        }
        ShutdownDrain {
            views,
            pending: self.shared.pending.clone(),
            quiet_since: None,
        }
    }
}

/// What a caller pumps after [`ProfileBridge::shutdown`] and after dropping
/// its views, so WebKit finishes page and process teardown while the GTK
/// thread is alive.
pub struct ShutdownDrain {
    views: Vec<glib::WeakRef<webkit2gtk::WebView>>,
    pending: Rc<Cell<usize>>,
    quiet_since: Option<Instant>,
}

impl ShutdownDrain {
    /// Views still alive somewhere.
    pub fn remaining_views(&self) -> usize {
        self.views
            .iter()
            .filter(|view| view.upgrade().is_some())
            .count()
    }

    /// Messages to web processes still awaiting completion.
    pub fn pending_messages(&self) -> usize {
        self.pending.get()
    }

    /// Whether every view is gone, every message completed and the loop
    /// has been idle for a quiet period. Call between loop iterations.
    pub fn poll(&mut self) -> bool {
        if self.remaining_views() > 0 || self.pending.get() > 0 || gtk::events_pending() {
            self.quiet_since = None;
            return false;
        }
        self.quiet_since.get_or_insert_with(Instant::now).elapsed() >= DRAIN_QUIET
    }

    /// Iterates the default main context until settled or `timeout`.
    pub fn run(&mut self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            while gtk::events_pending() && Instant::now() < deadline {
                gtk::main_iteration_do(false);
            }
            if self.poll() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        false
    }
}

fn connect<R: Runtime + 'static>(
    shared: &Rc<Shared<R>>,
    id: u64,
    view: &webkit2gtk::WebView,
    page: Rc<Cell<u64>>,
) -> Vec<glib::SignalHandlerId> {
    let weak = Rc::downgrade(shared);
    let mut handlers = Vec::new();
    let messages = weak.clone();
    handlers.push(view.connect_user_message_received(move |view, message| {
        if message.name().as_deref() != Some(EVENT_MESSAGE) {
            return false;
        }
        match messages.upgrade() {
            Some(shared) => shared.message(id, view, message),
            None => message.send_reply(&UserMessage::new(EVENT_MESSAGE, None)),
        }
        true
    }));
    let terminated = weak.clone();
    handlers.push(view.connect_web_process_terminated(move |_, _| {
        if let Some(shared) = terminated.upgrade() {
            shared.cancel_pull(id);
            shared.core.borrow_mut().process_gone(id);
            shared.collect();
        }
    }));
    let swapped = weak.clone();
    handlers.push(view.connect_page_id_notify(move |view| {
        if page.replace(view.page_id()) == view.page_id() {
            return;
        }
        if let Some(shared) = swapped.upgrade() {
            shared.cancel_pull(id);
            shared.core.borrow_mut().process_gone(id);
            shared.collect();
        }
    }));
    let loads = weak.clone();
    handlers.push(view.connect_load_changed(move |view, event| {
        let Some(shared) = loads.upgrade() else {
            return;
        };
        match event {
            LoadEvent::Started => {
                shared.cancel_pull(id);
                shared.core.borrow_mut().load_started(id);
            }
            LoadEvent::Committed => {
                let uri = view.uri();
                let now = shared.now_ms();
                shared
                    .core
                    .borrow_mut()
                    .load_committed(id, uri.as_deref(), now);
                if shared.core.borrow().phase(id) == Some(Phase::Fallback) {
                    start_pull(&shared, id);
                }
            }
            _ => return,
        }
        shared.collect();
    }));
    let destroyed = weak;
    handlers.push(view.connect_destroy(move |_| {
        if let Some(shared) = destroyed.upgrade() {
            shared.core.borrow_mut().detach(id);
            shared.collect();
            shared.forget_host(id);
        }
    }));
    handlers
}

impl<R: Runtime + 'static> Shared<R> {
    fn now_ms(&self) -> u64 {
        u64::try_from(self.clock.elapsed().as_millis()).unwrap_or(u64::MAX)
    }

    fn is_view(&self, id: u64, view: &webkit2gtk::WebView) -> bool {
        self.hosts
            .borrow()
            .get(&id)
            .and_then(|host| host.view.upgrade())
            .is_some_and(|known| &known == view)
    }

    fn message(self: &Rc<Self>, id: u64, view: &webkit2gtk::WebView, message: &UserMessage) {
        if !self.is_view(id, view) {
            message.send_reply(&UserMessage::new(EVENT_MESSAGE, None));
            return;
        }
        let token = ReplyToken(self.next_reply.get() + 1);
        self.next_reply.set(token.0);
        self.replies.borrow_mut().insert(token.0, message.clone());
        let event = message
            .parameters()
            .filter(|parameters| {
                parameters.is_type(VariantTy::BYTE_STRING) && parameters.size() <= MAX_UI_BYTES
            })
            .and_then(|parameters| {
                parameters
                    .fixed_array::<u8>()
                    .ok()
                    .and_then(|bytes| decode_event(bytes).ok())
            });
        let now = self.now_ms();
        {
            let mut core = self.core.borrow_mut();
            match event {
                Some(event) => core.event(id, event, token, now),
                None => core.malformed(token),
            }
        }
        self.collect();
    }

    fn cancel_pull(&self, id: u64) {
        if let Some(pull) = self
            .hosts
            .borrow()
            .get(&id)
            .and_then(|host| host.pull.clone())
        {
            pull.cancel();
        }
    }

    fn forget_host(&self, id: u64) {
        let host = self.hosts.borrow_mut().remove(&id);
        if let Some(host) = host {
            if let Some(pull) = &host.pull {
                pull.cancel();
            }
            if let Some(view) = host.view.upgrade() {
                for handler in host.handlers {
                    view.disconnect(handler);
                }
            }
        }
    }

    /// Moves the core's effects onto the queue and drains it.
    fn collect(self: &Rc<Self>) {
        let effects = self.core.borrow_mut().take_effects();
        self.effects.borrow_mut().extend(effects);
        self.drain();
        self.schedule_tick();
    }

    fn drain(self: &Rc<Self>) {
        if self.draining.replace(true) {
            return;
        }
        loop {
            let Some(effect) = self.effects.borrow_mut().pop_front() else {
                break;
            };
            if let Err(effect) = self.execute(effect) {
                self.effects.borrow_mut().push_front(*effect);
                break;
            }
            let more = self.core.borrow_mut().take_effects();
            self.effects.borrow_mut().extend(more);
        }
        self.draining.set(false);
    }

    fn schedule_tick(self: &Rc<Self>) {
        if self.tick.borrow().is_some() || !self.core.borrow().has_buffered() {
            return;
        }
        let weak = Rc::downgrade(self);
        let source = glib::timeout_add_local(TICK_INTERVAL, move || {
            let Some(shared) = weak.upgrade() else {
                return glib::ControlFlow::Break;
            };
            let now = shared.now_ms();
            shared.core.borrow_mut().tick(now);
            let effects = shared.core.borrow_mut().take_effects();
            shared.effects.borrow_mut().extend(effects);
            shared.drain();
            if shared.core.borrow().has_buffered() {
                glib::ControlFlow::Continue
            } else {
                shared.tick.borrow_mut().take();
                glib::ControlFlow::Break
            }
        });
        *self.tick.borrow_mut() = Some(source);
    }

    fn deliver_outbox(&self, outbox: Outbox) {
        let mut core = self.core.borrow_mut();
        let mut failures = Vec::new();
        for (context, frame) in outbox.frames {
            if let Err(error) = core.deliver(&context, frame) {
                failures.push(Effect::Undeliverable { context, error });
            }
        }
        let effects = core.take_effects();
        drop(core);
        let mut queue = self.effects.borrow_mut();
        queue.extend(effects);
        queue.extend(failures);
    }

    /// Runs one effect. Hands it back when the runtime is borrowed by a
    /// caller further up the stack; that caller drains it afterwards.
    fn execute(self: &Rc<Self>, effect: Effect) -> Result<(), Box<Effect>> {
        match effect {
            Effect::Send {
                view,
                name,
                bytes,
                outgoing,
                policy,
                scripts,
            } => self.send(
                view,
                name,
                bytes,
                outgoing,
                policy
                    .as_ref()
                    .map(|policy| policy.fd())
                    .or_else(|| scripts.as_ref().map(|scripts| scripts.fd())),
            ),
            Effect::Reply(token) => {
                if let Some(message) = self.replies.borrow_mut().remove(&token.0) {
                    message.send_reply(&UserMessage::new(EVENT_MESSAGE, None));
                }
            }
            Effect::StartFallback { view } => {
                if let Some(host) = self.hosts.borrow_mut().get_mut(&view) {
                    host.pull.get_or_insert_with(|| Rc::new(PullState::new()));
                }
                if self.core.borrow().fallback_ready(view) {
                    start_pull(self, view);
                }
            }
            Effect::PullDeliver {
                view,
                context,
                frame,
            } => self.pull_deliver(view, context, &frame),
            effect => {
                let Ok(mut runtime) = self.runtime.try_borrow_mut() else {
                    return Err(Box::new(effect));
                };
                let mut outbox = Outbox::default();
                match effect {
                    Effect::Receive { sender, envelope } => {
                        runtime.receive(&sender, &envelope, &mut outbox)
                    }
                    Effect::Closed { context, reason } => {
                        runtime.context_closed(&context, reason, &mut outbox)
                    }
                    Effect::Lifecycle(event) => runtime.frame_lifecycle(&event, &mut outbox),
                    Effect::Injection { view, result } => {
                        runtime.injection_result(BridgeViewId(view), &result)
                    }
                    Effect::Undeliverable { context, error } => {
                        runtime.undeliverable(&context, error)
                    }
                    Effect::Requests {
                        view,
                        owner,
                        observations,
                    } => runtime.requests(BridgeViewId(view), owner, observations),
                    Effect::Form {
                        view,
                        owner,
                        frame,
                        document,
                        fields,
                    } => runtime.form_submitted(BridgeViewId(view), owner, frame, document, fields),
                    Effect::Send { .. }
                    | Effect::Reply(_)
                    | Effect::StartFallback { .. }
                    | Effect::PullDeliver { .. } => {}
                }
                drop(runtime);
                self.deliver_outbox(outbox);
            }
        }
        Ok(())
    }

    fn send(
        self: &Rc<Self>,
        id: u64,
        name: &'static str,
        bytes: Vec<u8>,
        outgoing: Outgoing,
        descriptor: Option<BorrowedFd<'_>>,
    ) {
        let view = self
            .hosts
            .borrow()
            .get(&id)
            .and_then(|host| host.view.upgrade());
        let Some(view) = view else {
            self.settle(id, outgoing, Err(None));
            return;
        };
        let parameters = glib::Variant::from_data::<Vec<u8>, _>(bytes);
        let message = match descriptor {
            Some(descriptor) => {
                let descriptors = gio::UnixFDList::new();
                if descriptors.append(descriptor).ok() != Some(0) {
                    self.settle(id, outgoing, Err(None));
                    return;
                }
                UserMessage::with_fd_list(name, Some(&parameters), Some(&descriptors))
            }
            None => UserMessage::new(name, Some(&parameters)),
        };
        self.pending.set(self.pending.get() + 1);
        let weak = Rc::downgrade(self);
        let pending = self.pending.clone();
        let delivery = matches!(outgoing, Outgoing::Deliver { .. });
        view.send_message_to_page(&message, None::<&gio::Cancellable>, move |reply| {
            pending.set(pending.get().saturating_sub(1));
            let Some(shared) = weak.upgrade() else {
                return;
            };
            let reply = reply
                .map(|reply| {
                    let parameters = reply.parameters();
                    if delivery {
                        parameters
                            .and_then(|parameters| {
                                parameters
                                    .fixed_array::<u8>()
                                    .ok()
                                    .and_then(|bytes| decode_delivery_result(bytes).ok())
                            })
                            .unwrap_or(DeliveryStatus::Denied)
                    } else if parameters.and_then(|parameters| parameters.get::<bool>())
                        == Some(true)
                    {
                        DeliveryStatus::Delivered
                    } else {
                        DeliveryStatus::Denied
                    }
                })
                .map_err(|error| Some(error.matches(UserMessageError::Message)));
            shared.settle(id, outgoing, reply);
            shared.collect();
        });
    }

    /// `Err(Some(true))`: no module handled it; `Err(_)`: never answered.
    fn settle(&self, id: u64, outgoing: Outgoing, reply: Result<DeliveryStatus, Option<bool>>) {
        let mut core = self.core.borrow_mut();
        match outgoing {
            Outgoing::Attach { generation } => {
                let outcome = match reply {
                    Ok(DeliveryStatus::Delivered) => AttachOutcome::Accepted,
                    Ok(_) => AttachOutcome::Refused,
                    Err(Some(true)) => AttachOutcome::Unhandled,
                    Err(_) => AttachOutcome::Failed,
                };
                core.attach_settled(id, generation, outcome);
            }
            Outgoing::Deliver { context } => {
                core.delivery_settled(id, &context, reply.unwrap_or(DeliveryStatus::Denied));
            }
            Outgoing::Policy { .. } => core.policy_settled(reply == Ok(DeliveryStatus::Delivered)),
            Outgoing::Scripts { table_generation } => {
                core.scripts_settled(id, table_generation, reply == Ok(DeliveryStatus::Delivered))
            }
            Outgoing::Inject { request_id } => {
                core.injection_transport_settled(request_id, reply == Ok(DeliveryStatus::Delivered))
            }
            Outgoing::Detach => {}
        }
    }

    fn pull_deliver(self: &Rc<Self>, id: u64, context: ContextKey, frame: &[u8]) {
        let host = self
            .hosts
            .borrow()
            .get(&id)
            .and_then(|host| Some((host.view.upgrade()?, host.pull.clone()?)));
        let Some((view, pull)) = host else {
            self.core
                .borrow_mut()
                .delivery_settled(id, &context, DeliveryStatus::Denied);
            return;
        };
        let arguments = glib::VariantDict::new(None);
        arguments.insert_value("bytes", &pull::encode(frame).to_variant());
        let cancellable = pull.cancellable.borrow().clone();
        self.pending.set(self.pending.get() + 1);
        let pending = self.pending.clone();
        let weak = Rc::downgrade(self);
        view.call_async_javascript_function(
            pull::DELIVER_BODY,
            Some(&arguments.end()),
            None,
            None,
            Some(&cancellable),
            move |result| {
                pending.set(pending.get().saturating_sub(1));
                let Some(shared) = weak.upgrade() else {
                    return;
                };
                let accepted = result.is_ok_and(|value| value.is_boolean() && value.to_boolean());
                shared.core.borrow_mut().delivery_settled(
                    id,
                    &context,
                    if accepted {
                        DeliveryStatus::Delivered
                    } else {
                        DeliveryStatus::NoReceiver
                    },
                );
                shared.collect();
            },
        );
    }
}

fn start_pull<R: Runtime + 'static>(shared: &Rc<Shared<R>>, id: u64) {
    let host = shared
        .hosts
        .borrow()
        .get(&id)
        .and_then(|host| Some((host.view.upgrade()?, host.pull.clone()?)));
    let Some((view, pull)) = host else {
        return;
    };
    if pull.inflight.replace(true) {
        return;
    }
    let cancellable = pull.cancellable.borrow().clone();
    let cancelled = cancellable.clone();
    shared.pending.set(shared.pending.get() + 1);
    let pending = shared.pending.clone();
    let weak = Rc::downgrade(shared);
    view.call_async_javascript_function(
        &pull::pull_body(),
        None,
        None,
        None,
        Some(&cancellable),
        move |result| {
            pending.set(pending.get().saturating_sub(1));
            let Some(shared) = weak.upgrade() else {
                return;
            };
            if cancelled.is_cancelled() {
                return;
            }
            pull.inflight.set(false);
            let envelope = result
                .ok()
                .filter(|value| value.is_string())
                .and_then(|value| {
                    let text = value.to_string_as_bytes()?;
                    pull::decode(&text)
                });
            let received = match envelope {
                Some(envelope) if !envelope.is_empty() => {
                    let accepted = shared.core.borrow_mut().fallback_post(id, envelope).is_ok();
                    shared.collect();
                    accepted
                }
                _ => false,
            };
            let interval = if received {
                Duration::ZERO
            } else {
                let next = pull::next_interval(pull.interval.get());
                pull.interval.set(next);
                next
            };
            if received {
                pull.interval.set(PULL_MIN_INTERVAL);
            }
            schedule_pull(&shared, id, &pull, interval);
        },
    );
}

fn schedule_pull<R: Runtime + 'static>(
    shared: &Rc<Shared<R>>,
    id: u64,
    pull: &Rc<PullState>,
    interval: Duration,
) {
    if let Some(timer) = pull.timer.borrow_mut().take() {
        timer.remove();
    }
    let weak: Weak<Shared<R>> = Rc::downgrade(shared);
    let state = Rc::downgrade(pull);
    let source = glib::timeout_add_local_once(interval, move || {
        if let Some(state) = state.upgrade() {
            state.timer.borrow_mut().take();
        }
        if let Some(shared) = weak.upgrade() {
            start_pull(&shared, id);
        }
    });
    *pull.timer.borrow_mut() = Some(source);
}
