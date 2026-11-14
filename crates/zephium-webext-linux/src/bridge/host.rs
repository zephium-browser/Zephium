//! GTK side of a profile bridge: context configuration, view signals, user
//! messages and teardown.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, VecDeque};
use std::os::fd::BorrowedFd;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gio::prelude::*;
use glib::variant::ToVariant;
use glib::VariantTy;
use gtk::prelude::WidgetExt;
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
    /// The installed WebKitGTK cannot support the module.
    UnsupportedRuntime(zephium_core::webkitgtk::AdmissionError),
    /// The configured module is absent.
    ModuleUnavailable(UnavailableReason),
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
}

/// Why a view has no extension transport.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnavailableReason {
    /// No web-process module exists in the configured directory.
    MissingModule,
    /// No module handled the attachment, including a module load failure.
    ModuleUnhandled,
    /// The module rejected the attachment or protocol.
    ModuleRefused,
}

impl std::fmt::Display for UnavailableReason {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let reason = match self {
            Self::MissingModule => "the web-process module is missing",
            Self::ModuleUnhandled => "the web-process module could not be loaded",
            Self::ModuleRefused => "the web-process module rejected the attachment",
        };
        let version = runtime_version();
        write!(
            formatter,
            "extensions unavailable on WebKitGTK {}.{}.{}: {reason}",
            version[0], version[1], version[2]
        )
    }
}

impl std::fmt::Display for BridgeError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ModuleDirectory => {
                formatter.write_str("extensions unavailable: invalid web-process module directory")
            }
            Self::UnsupportedRuntime(error) => write!(formatter, "extensions unavailable: {error}"),
            Self::ModuleUnavailable(reason) => reason.fmt(formatter),
            Self::Random(error) => {
                write!(formatter, "extensions unavailable: process token: {error}")
            }
        }
    }
}

impl std::error::Error for BridgeError {}

struct HostView {
    view: glib::WeakRef<webkit2gtk::WebView>,
    handlers: Vec<glib::SignalHandlerId>,
    owned: bool,
    cancellable: gio::Cancellable,
    process_alive: bool,
}

struct Shared<R> {
    core: RefCell<Core>,
    runtime: RefCell<R>,
    effects: RefCell<VecDeque<Effect>>,
    draining: Cell<bool>,
    hosts: RefCell<HashMap<u64, HostView>>,
    replies: RefCell<HashMap<u64, (u64, UserMessage)>>,
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

fn runtime_version() -> [u32; 3] {
    // SAFETY: No-argument WebKitGTK ABI functions return immutable library version constants.
    unsafe {
        [
            webkit2gtk::ffi::webkit_get_major_version(),
            webkit2gtk::ffi::webkit_get_minor_version(),
            webkit2gtk::ffi::webkit_get_micro_version(),
        ]
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
        let version = runtime_version();
        zephium_core::webkitgtk::admit_runtime(version[0], version[1], version[2])
            .map_err(BridgeError::UnsupportedRuntime)?;
        if !config
            .module_directory
            .join("libzephium_webkit_web_process.so")
            .is_file()
        {
            return Err(BridgeError::ModuleUnavailable(
                UnavailableReason::MissingModule,
            ));
        }
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

    /// Attaches a tab with the browser's stable Chrome tab identity for DNR.
    pub fn attach_tab_with_id(
        &self,
        tab: ItemId,
        tab_id: i32,
        view: &webkit2gtk::WebView,
        plan: &AttachPlan,
    ) -> Result<BridgeViewId, RegisterError> {
        self.check_view(view)?;
        let result = self
            .shared
            .core
            .borrow_mut()
            .register_tab_with_id(tab, tab_id, plan);
        self.finish_register(view, result, false)
    }

    /// Attaches a runtime-owned extension view. Call before its first load.
    pub fn attach_extension_view(
        &self,
        install: ExtensionInstallId,
        kind: ViewKind,
        view: &webkit2gtk::WebView,
        plan: &AttachPlan,
    ) -> Result<BridgeViewId, RegisterError> {
        self.check_view(view)?;
        let result = self
            .shared
            .core
            .borrow_mut()
            .register_owned(install, kind, plan);
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
                cancellable: gio::Cancellable::new(),
                process_alive: true,
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
        Ok(self.publish_sealed_policy(SealedPolicy::new(bytes)?))
    }

    /// Publishes a snapshot prepared and sealed by a worker thread.
    pub fn publish_sealed_policy(&self, policy: SealedPolicy) -> u64 {
        self.publish_prepared_policy(Rc::new(policy))
    }

    /// Publishes an already sealed snapshot, retaining shared rollback ownership.
    pub fn publish_prepared_policy(&self, policy: Rc<SealedPolicy>) -> u64 {
        let generation = self.shared.core.borrow_mut().publish_policy(policy);
        self.shared.collect();
        generation
    }

    /// `None` means unknown; `Err(false)` pending; `Err(true)` refused.
    pub fn policy_status(&self, generation: u64) -> Option<Result<(), bool>> {
        self.shared.core.borrow().policy_status(generation)
    }

    /// Publishes this profile's immutable script snapshot to future documents.
    pub fn publish_scripts(&self, table: ScriptTable) -> Result<u64, ScriptTableError> {
        let table = Rc::new(SealedScripts::new(table)?);
        let generation = self.shared.core.borrow_mut().publish_scripts(table)?;
        self.shared.collect();
        Ok(generation)
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
            })
    }

    /// Ends the bridge: ends every context (the runtime
    /// cancels calls and disconnects ports), detaches every view,
    /// disconnects its signals and terminates extension-owned views' web
    /// processes. Then the view owners drop their views, and the caller
    /// keeps the GTK loop running until the returned drain settles.
    pub fn shutdown(&self) -> ShutdownDrain {
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
            host.cancellable.cancel();
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
        for (_, (_, message)) in self.shared.replies.borrow_mut().drain() {
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
            shared.process_gone(id);
            shared.collect();
        }
    }));
    let swapped = weak.clone();
    handlers.push(view.connect_page_id_notify(move |view| {
        if page.replace(view.page_id()) == view.page_id() {
            return;
        }
        if let Some(shared) = swapped.upgrade() {
            shared.process_gone(id);
            if view.page_id() != 0 {
                shared.process_started(id);
            }
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
                shared.process_started(id);
            }
            LoadEvent::Committed => {
                let uri = view.uri();
                let now = shared.now_ms();
                shared
                    .core
                    .borrow_mut()
                    .load_committed(id, uri.as_deref(), now);
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
        self.replies
            .borrow_mut()
            .insert(token.0, (id, message.clone()));
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

    fn process_gone(&self, id: u64) {
        let cancelled = self.hosts.borrow_mut().get_mut(&id).map(|host| {
            host.process_alive = false;
            std::mem::replace(&mut host.cancellable, gio::Cancellable::new())
        });
        self.core.borrow_mut().process_gone(id);
        self.replies.borrow_mut().retain(|_, (view, _)| *view != id);
        if let Some(cancelled) = cancelled {
            cancelled.cancel();
        }
    }

    fn process_started(&self, id: u64) {
        if let Some(host) = self.hosts.borrow_mut().get_mut(&id) {
            host.process_alive = true;
        }
        self.core.borrow_mut().load_started(id);
    }

    fn forget_host(&self, id: u64) {
        let host = self.hosts.borrow_mut().remove(&id);
        if let Some(host) = host {
            host.cancellable.cancel();
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
                if let Some((_, message)) = self.replies.borrow_mut().remove(&token.0) {
                    message.send_reply(&UserMessage::new(EVENT_MESSAGE, None));
                }
            }
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
                        dropped_observations,
                        dropped_matches,
                    } => {
                        runtime.request_loss(dropped_observations, dropped_matches);
                        runtime.observed_requests(
                            BridgeViewId(view),
                            owner,
                            observations,
                            &mut outbox,
                        );
                    }
                    Effect::Form {
                        view,
                        owner,
                        frame,
                        document,
                        fields,
                    } => runtime.form_submitted(BridgeViewId(view), owner, frame, document, fields),
                    Effect::Unavailable {
                        view,
                        owner,
                        reason,
                    } => runtime.transport_unavailable(BridgeViewId(view), owner, reason),
                    Effect::Send { .. } | Effect::Reply(_) => {}
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
        let host = self.hosts.borrow().get(&id).and_then(|host| {
            host.process_alive
                .then(|| (host.view.upgrade(), host.cancellable.clone()))
        });
        let Some((Some(view), cancellable)) = host else {
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
        let cancelled = cancellable.clone();
        view.send_message_to_page(&message, Some(&cancellable), move |reply| {
            pending.set(pending.get().saturating_sub(1));
            if cancelled.is_cancelled() {
                return;
            }
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
            Outgoing::Policy { policy_generation } => {
                core.policy_settled(policy_generation, reply == Ok(DeliveryStatus::Delivered))
            }
            Outgoing::Scripts { table_generation } => {
                core.scripts_settled(id, table_generation, reply == Ok(DeliveryStatus::Delivered))
            }
            Outgoing::Inject { request_id } => {
                core.injection_transport_settled(request_id, reply == Ok(DeliveryStatus::Delivered))
            }
            Outgoing::Detach => {}
        }
    }
}
