//! One profile's extension runtime: the package scheme, the bridge, the
//! shared runtime's authority, dispatcher and services, and the views the
//! runtime owns, driven from the GTK main loop.
//!
//! Everything runs on the GTK thread. The only other thread is the storage
//! executor (see `runtime/storage.rs`). Timers are one glib timeout at the
//! earliest deadline any service has; native callbacks are queued and
//! handled from an idle source, never re-entrantly.

mod errors;
pub(crate) mod host;
mod storage;
pub(crate) mod surface;

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use glib::prelude::*;
use webkit2gtk::{WebProcessTerminationReason, WebViewExt};
use zephium_core::extensions::ExtensionRuntimeGeneration;
use zephium_core::ids::{ExtensionInstallId, ItemId, ProfileId};
use zephium_webext::manifest::Manifest;
use zephium_webext_runtime::context::{
    ContextKind, ExtensionId, ExtensionScheme, ExternallyConnectable, InstallAdmission, ViewKind,
};
use zephium_webext_runtime::i18n::{fallback_chain, LocaleEnvironment, Messages};
use zephium_webext_runtime::lifecycle::BackgroundState;
use zephium_webext_runtime::permissions::InstallPermissions;

pub use self::errors::{
    ExtensionError, ExtensionErrorKind, MAX_ERRORS_PER_INSTALL, MAX_ERROR_MESSAGE_BYTES,
};
pub use self::host::{Activation, BackgroundStatus, Notice, RuntimeHost};
use self::host::{Admitted, HostConfig, NativeOp};
use crate::background::{check_background, page_script, InstallRefusal};
use crate::bridge::{
    AttachPlan, BridgeConfig, BridgeError, BridgeViewId, ExtensionPages, ProfileBridge,
    RegisterError, ShutdownDrain,
};
use crate::scheme::{RequestHost, Requester, Scheme, SchemeError};
use crate::views::{
    create_view, ExtensionPage, Owned, PageEntry, PageKind, PageScript, ViewError, ViewKindKey,
    ViewRequests, Views,
};

const MAX_LOCALE_FILE_BYTES: u64 = 4 * 1024 * 1024;
const DRAIN_QUIET: Duration = Duration::from_millis(100);
const MAX_TIMER: Duration = Duration::from_secs(24 * 60 * 60);

/// How one profile's runtime is set up.
#[derive(Clone, Debug)]
pub struct ProfileRuntimeConfig {
    /// The profile.
    pub profile: ProfileId,
    /// Whether the profile is a private (incognito) partition.
    pub incognito: bool,
    /// Directory holding `libzephium_webkit_web_process.so`.
    pub module_directory: PathBuf,
    /// Private per-profile directory for extension storage and runtime
    /// state; `None` keeps everything in memory. Must be `None` for
    /// private profiles.
    pub state_directory: Option<PathBuf>,
    /// Browser locale facts for `chrome.i18n`.
    pub locale: LocaleEnvironment,
    /// Runs lifecycle, alarm and message timeouts this many times faster.
    /// 1 in production; only tests set more.
    pub time_scale: u32,
}

/// Facts the engine host supplies about tab views.
pub trait RuntimeEngine {
    /// Who a tab view is, for the package scheme. Runtime-owned views never
    /// reach this. Return `None` for privileged, Work and preview views.
    fn requester(&self, view: &webkit2gtk::WebView) -> Option<Requester>;

    /// A notice for the browser: an extension error, a crash-loop disable
    /// or `runtime.openOptionsPage()`.
    fn notice(&self, notice: &Notice) {
        let _ = notice;
    }
}

/// One install as the browser admitted it.
#[derive(Clone, Debug)]
pub struct InstallSpec {
    /// The profile-scoped install.
    pub install: ExtensionInstallId,
    /// Its extension id.
    pub extension_id: ExtensionId,
    /// A runtime generation newer than any this profile ran for it.
    pub generation: ExtensionRuntimeGeneration,
    /// The prepared, immutable package directory.
    pub directory: PathBuf,
    /// Its manifest.
    pub manifest: Manifest,
    /// The granted permissions.
    pub permissions: InstallPermissions,
    /// Whether the person allowed it in private windows.
    pub allowed_in_incognito: bool,
    /// Its `externally_connectable` pages.
    pub externally_connectable: Option<ExternallyConnectable>,
    /// Unpacked (developer) install: relaxed alarm minimum.
    pub unpacked: bool,
}

/// Why an install could not run.
#[derive(Debug)]
pub enum InstallError {
    /// The extension uses something this runtime refuses.
    Refused(InstallRefusal),
    /// The package scheme refused it.
    Scheme(SchemeError),
    /// The authority refused it (duplicate id, stale generation, incognito).
    Admission(String),
    /// Its locale catalogs are invalid.
    Locales(String),
    /// The profile runtime was retired.
    Retired,
}

impl std::fmt::Display for InstallError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Refused(refusal) => refusal.fmt(formatter),
            Self::Scheme(error) => error.fmt(formatter),
            Self::Admission(reason) => write!(formatter, "extension not admitted: {reason}"),
            Self::Locales(reason) => write!(formatter, "invalid locale catalog: {reason}"),
            Self::Retired => formatter.write_str("profile runtime retired"),
        }
    }
}

impl std::error::Error for InstallError {}

/// Why the runtime could not be created.
#[derive(Debug)]
pub enum RuntimeError {
    /// The bridge could not configure the context.
    Bridge(BridgeError),
    /// The scheme could not be registered.
    Scheme(SchemeError),
    /// The runtime's API table or storage thread failed.
    Setup(String),
    /// A private profile was given a state directory.
    PrivateState,
}

impl std::fmt::Display for RuntimeError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Bridge(error) => write!(formatter, "bridge: {error:?}"),
            Self::Scheme(error) => write!(formatter, "scheme: {error}"),
            Self::Setup(reason) => write!(formatter, "runtime setup: {reason}"),
            Self::PrivateState => formatter.write_str("private profiles keep no extension state"),
        }
    }
}

impl std::error::Error for RuntimeError {}

/// Memory pressure the OS reported.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MemoryPressure {
    /// Stop the least recently active idle background.
    Moderate,
    /// Stop every idle background.
    Critical,
}

enum NativeEvent {
    Crashed {
        install: ExtensionInstallId,
        generation: u64,
    },
    LoadFailed {
        install: ExtensionInstallId,
        generation: u64,
    },
    OffscreenLoaded {
        install: ExtensionInstallId,
        loaded: bool,
    },
    OffscreenGone {
        install: ExtensionInstallId,
    },
}

struct Record {
    extension_id: ExtensionId,
    sandboxed: Vec<String>,
    manifest: Manifest,
}

struct Inner {
    context: webkit2gtk::WebContext,
    scheme: Rc<Scheme>,
    bridge: ProfileBridge<RuntimeHost>,
    engine: Weak<dyn RuntimeEngine>,
    views: Rc<Views>,
    records: RefCell<HashMap<ExtensionInstallId, Record>>,
    config: ProfileRuntimeConfig,
    scale: u64,
    timer: RefCell<Option<glib::SourceId>>,
    storage_source: RefCell<Option<glib::SourceId>>,
    idle: Cell<bool>,
    running: Cell<bool>,
    retired: Cell<bool>,
    events: RefCell<VecDeque<NativeEvent>>,
    next_page: Cell<u64>,
}

/// One profile's extension runtime. GTK thread only; clones share it.
#[derive(Clone)]
pub struct ProfileRuntime {
    inner: Rc<Inner>,
}

fn hex(install: ExtensionInstallId) -> String {
    install
        .bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn unix_millis() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |since| {
            u64::try_from(since.as_millis()).unwrap_or(u64::MAX)
        })
}

fn read_locales(
    root: &Path,
    manifest: &Manifest,
    locale: &LocaleEnvironment,
) -> Result<BTreeMap<String, serde_json::Value>, String> {
    let mut catalogs = BTreeMap::new();
    let chain = fallback_chain(&locale.ui_language, manifest.default_locale())
        .map_err(|error| error.message().to_owned())?;
    for name in chain {
        let path = root.join("_locales").join(&name).join("messages.json");
        let Ok(file) = std::fs::File::open(&path) else {
            continue;
        };
        let metadata = file.metadata().map_err(|error| error.to_string())?;
        if !metadata.is_file() || metadata.len() > MAX_LOCALE_FILE_BYTES {
            return Err(format!("{name}: catalog too large"));
        }
        let mut text = String::new();
        use std::io::Read;
        file.take(MAX_LOCALE_FILE_BYTES)
            .read_to_string(&mut text)
            .map_err(|_| format!("{name}: not UTF-8"))?;
        let value = zephium_webext::manifest::parse_lenient(text.trim_start_matches('\u{feff}'))
            .map_err(|_| format!("{name}: not JSON"))?;
        catalogs.insert(name, value);
    }
    Ok(catalogs)
}

impl ProfileRuntime {
    /// Sets up the runtime on a profile's context. Call before the context
    /// starts any web process. The runtime holds `engine` weakly: the
    /// caller keeps it alive.
    pub fn new(
        context: &webkit2gtk::WebContext,
        config: ProfileRuntimeConfig,
        engine: Rc<dyn RuntimeEngine>,
    ) -> Result<Self, RuntimeError> {
        if config.incognito && config.state_directory.is_some() {
            return Err(RuntimeError::PrivateState);
        }
        let views = Rc::new(Views::default());
        let requests: Rc<dyn RequestHost> = Rc::new(ViewRequests {
            views: views.clone(),
            engine: Rc::downgrade(&engine),
        });
        let scheme =
            Scheme::register(context, requests.clone(), None).map_err(RuntimeError::Scheme)?;
        let scale = u64::from(config.time_scale.max(1));
        let origin = Instant::now();
        let clock: Rc<dyn Fn() -> u64> = Rc::new(move || {
            u64::try_from(origin.elapsed().as_nanos())
                .unwrap_or(u64::MAX)
                .saturating_mul(scale)
        });
        let host = RuntimeHost::new(HostConfig {
            profile: config.profile,
            incognito: config.incognito,
            ui_language: config.locale.ui_language.clone(),
            clock,
            unix_millis: Rc::new(unix_millis),
            scale,
        })
        .map_err(RuntimeError::Setup)?;
        let bridge = ProfileBridge::new(
            context,
            BridgeConfig {
                profile: config.profile,
                incognito: config.incognito,
                scheme: ExtensionScheme::CHROME_EXTENSION,
                module_directory: config.module_directory.clone(),
            },
            host,
        )
        .map_err(RuntimeError::Bridge)?;
        let inner = Rc::new(Inner {
            context: context.clone(),
            scheme,
            bridge,
            engine: Rc::downgrade(&engine),
            views,
            records: RefCell::new(HashMap::new()),
            config,
            scale,
            timer: RefCell::new(None),
            storage_source: RefCell::new(None),
            idle: Cell::new(false),
            running: Cell::new(false),
            retired: Cell::new(false),
            events: RefCell::new(VecDeque::new()),
            next_page: Cell::new(1),
        });
        let weak = Rc::downgrade(&inner);
        let kick: Rc<dyn Fn()> = Rc::new(move || {
            if let Some(inner) = weak.upgrade() {
                inner.schedule();
            }
        });
        let fd = inner
            .bridge
            .with_runtime(|host, _| {
                host.set_kick(kick);
                host.storage_fd()
            })
            .map_err(|_| RuntimeError::Setup("runtime busy".into()))?;
        let weak = Rc::downgrade(&inner);
        let source = glib::source::unix_fd_add_local(fd, glib::IOCondition::IN, move |_, _| {
            let Some(inner) = weak.upgrade() else {
                return glib::ControlFlow::Break;
            };
            let _ = inner.bridge.with_runtime(|host, out| {
                let done = host.storage_take();
                host.storage_done(done, out);
            });
            inner.run();
            glib::ControlFlow::Continue
        });
        *inner.storage_source.borrow_mut() = Some(source);
        Ok(Self { inner })
    }

    /// Checks what an extension needs before the browser accepts it.
    pub fn check_manifest(manifest: &Manifest, directory: &Path) -> Result<(), InstallRefusal> {
        check_background(manifest, directory).map(|_| ())
    }

    /// A freshly installed extension: its background starts and gets
    /// `runtime.onInstalled` with reason `install`.
    pub fn install_admitted(&self, spec: InstallSpec) -> Result<(), InstallError> {
        self.inner.activate(spec, &Activation::Installed)
    }

    /// An installed extension enabled again or loaded at browser start. Its
    /// background starts on its first event.
    pub fn install_enabled(&self, spec: InstallSpec) -> Result<(), InstallError> {
        self.inner.activate(spec, &Activation::Enabled)
    }

    /// A new version replaced a running one: the old background stops, its
    /// storage stays, its alarms are cleared, and the new background gets
    /// `runtime.onInstalled` with reason `update`.
    pub fn install_updated(
        &self,
        spec: InstallSpec,
        previous_version: &str,
    ) -> Result<(), InstallError> {
        let _ = self.inner.deactivate(spec.install, false);
        self.inner.activate(
            spec,
            &Activation::Updated {
                previous_version: previous_version.to_owned(),
            },
        )
    }

    /// Stops an extension and keeps its data. Pump the returned drain.
    pub fn install_disabled(&self, install: ExtensionInstallId) -> ExtensionStop {
        self.inner.deactivate(install, false)
    }

    /// Stops an extension and erases its storage and runtime state.
    pub fn install_removed(&self, install: ExtensionInstallId) -> ExtensionStop {
        self.inner.deactivate(install, true)
    }

    /// The browser finished starting: `runtime.onStartup` for extensions
    /// whose background listens to it.
    pub fn browser_started(&self) {
        let _ = self
            .inner
            .bridge
            .with_runtime(|host, out| host.browser_started(out));
        self.inner.run();
    }

    /// The OS reported memory pressure: idle backgrounds stop, least
    /// recently active first.
    pub fn memory_pressure(&self, pressure: MemoryPressure) {
        let reclaim = match pressure {
            MemoryPressure::Moderate => 1,
            MemoryPressure::Critical => usize::MAX,
        };
        let _ = self
            .inner
            .bridge
            .with_runtime(|host, out| host.memory_pressure(reclaim, out));
        self.inner.run();
    }

    /// Opens an options or popup page view. `path` overrides the manifest's
    /// page (as `action.setPopup` does); it must be one of the package's
    /// pages.
    pub fn open_page(
        &self,
        install: ExtensionInstallId,
        kind: PageKind,
        path: Option<&str>,
    ) -> Result<ExtensionPage, ViewError> {
        self.inner.open_page(install, kind, path)
    }

    /// The page-world script every enabled extension needs in the
    /// `UserContentManager` of an ordinary tab, for its pages opened there.
    pub fn extension_tab_scripts(&self) -> Vec<PageScript> {
        let installs: Vec<ExtensionInstallId> =
            self.inner.records.borrow().keys().copied().collect();
        installs
            .into_iter()
            .filter_map(|install| self.inner.page_script(install, ContextKind::ExtensionTab))
            .collect()
    }

    /// The extension origins an ordinary tab's attach plan lists so that
    /// extension documents committed in it get the bridge.
    pub fn extension_pages(&self) -> Vec<ExtensionPages> {
        self.inner
            .records
            .borrow()
            .values()
            .map(|record| ExtensionPages {
                extension: record.extension_id.clone(),
                sandboxed_paths: record.sandboxed.clone(),
            })
            .collect()
    }

    /// Attaches an ordinary tab's view to the bridge before its first load.
    /// Include [`Self::extension_pages`] in `plan` and the tab scripts in
    /// its content manager; when a document commits in it the bridge binds
    /// it as an extension tab (or content scripts) by itself.
    pub fn attach_tab(
        &self,
        tab: ItemId,
        view: &webkit2gtk::WebView,
        plan: &AttachPlan,
    ) -> Result<BridgeViewId, RegisterError> {
        self.inner.bridge.attach_tab(tab, view, plan)
    }

    /// Detaches a tab; its contexts end.
    pub fn detach_tab(&self, view: BridgeViewId) {
        self.inner.bridge.detach(view);
    }

    /// Whether navigating `requester` to `uri` may promote it to that
    /// extension document (the navigation-policy seam).
    pub fn allows_navigation(&self, requester: &Requester, uri: &str) -> bool {
        self.inner.scheme.allows(requester, uri)
    }

    /// The bridge, for content-script plans and policy publication.
    pub fn bridge(&self) -> &ProfileBridge<RuntimeHost> {
        &self.inner.bridge
    }

    /// An install's background state.
    pub fn background(&self, install: ExtensionInstallId) -> Option<BackgroundStatus> {
        self.inner
            .bridge
            .with_runtime(|host, _| host.status(install))
            .ok()
            .flatten()
    }

    /// An install's recent errors, oldest first.
    pub fn errors(&self, install: ExtensionInstallId) -> Vec<ExtensionError> {
        self.inner
            .bridge
            .with_runtime(|host, _| host.errors(install))
            .unwrap_or_default()
    }

    /// API methods and events the install used that this runtime does not
    /// implement, with call counts.
    pub fn unhandled_calls(&self, install: ExtensionInstallId) -> BTreeMap<String, u64> {
        self.inner
            .bridge
            .with_runtime(|host, _| host.unhandled(install))
            .unwrap_or_default()
    }

    /// Whether the install has an open offscreen document.
    pub fn has_offscreen_document(&self, install: ExtensionInstallId) -> bool {
        self.inner
            .bridge
            .with_runtime(|host, _| host.has_offscreen(install))
            .unwrap_or(false)
    }

    /// The URL `runtime.setUninstallURL` set.
    pub fn uninstall_url(&self, install: ExtensionInstallId) -> Option<String> {
        self.inner
            .bridge
            .with_runtime(|host, _| host.uninstall_url(install))
            .ok()
            .flatten()
    }

    /// Ends the profile's runtime: every background, offscreen document and
    /// page stops and the bridge shuts down. Drop every [`ExtensionPage`]
    /// and the widgets embedding them, then keep the GTK loop running until
    /// the returned drain settles before the GTK thread exits.
    pub fn retire(&self) -> RuntimeDrain {
        self.inner.retire()
    }
}

/// Teardown of one extension: its views are gone once this settles.
pub struct ExtensionStop {
    views: Rc<Views>,
    install: ExtensionInstallId,
    quiet_since: Option<Instant>,
}

impl ExtensionStop {
    /// Views of the install still alive somewhere.
    pub fn remaining_views(&self) -> usize {
        self.views.alive(Some(self.install))
    }

    /// Whether everything is gone and the loop was quiet for a moment.
    pub fn poll(&mut self) -> bool {
        if self.remaining_views() > 0 || gtk::events_pending() {
            self.quiet_since = None;
            return false;
        }
        self.quiet_since.get_or_insert_with(Instant::now).elapsed() >= DRAIN_QUIET
    }

    /// Iterates the default main context until settled or `timeout`.
    pub fn run(&mut self, timeout: Duration) -> bool {
        pump(timeout, || self.poll())
    }
}

/// Profile teardown: the bridge's drain plus the runtime's own views.
pub struct RuntimeDrain {
    bridge: ShutdownDrain,
    views: Rc<Views>,
}

impl RuntimeDrain {
    /// Views still alive somewhere.
    pub fn remaining_views(&self) -> usize {
        self.bridge.remaining_views() + self.views.alive(None)
    }

    /// Whether every view is gone, every message settled and the loop was
    /// quiet for a moment. Call between loop iterations.
    pub fn poll(&mut self) -> bool {
        self.views.alive(None) == 0 && self.bridge.poll()
    }

    /// Iterates the default main context until settled or `timeout`.
    pub fn run(&mut self, timeout: Duration) -> bool {
        pump(timeout, || self.poll())
    }
}

fn pump(timeout: Duration, mut settled: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        while gtk::events_pending() && Instant::now() < deadline {
            gtk::main_iteration_do(false);
        }
        if settled() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    false
}

impl Inner {
    fn schedule(self: &Rc<Self>) {
        if self.idle.replace(true) {
            return;
        }
        let weak = Rc::downgrade(self);
        glib::idle_add_local_once(move || {
            if let Some(inner) = weak.upgrade() {
                inner.idle.set(false);
                inner.run();
            }
        });
    }

    /// Handles queued native events and the host's native work, then arms
    /// the timer.
    fn run(self: &Rc<Self>) {
        if self.running.replace(true) {
            return;
        }
        loop {
            let event = self.events.borrow_mut().pop_front();
            if let Some(event) = event {
                self.native_event(event);
                continue;
            }
            let ops = self
                .bridge
                .with_runtime(|host, _| host.take_ops())
                .unwrap_or_default();
            if ops.is_empty() {
                break;
            }
            for op in ops {
                self.execute(op);
            }
        }
        self.running.set(false);
        self.arm();
    }

    fn native_event(self: &Rc<Self>, event: NativeEvent) {
        let _ = self.bridge.with_runtime(|host, out| match event {
            NativeEvent::Crashed {
                install,
                generation,
            } => host.background_crashed(install, generation, out),
            NativeEvent::LoadFailed {
                install,
                generation,
            } => host.background_failed(
                install,
                generation,
                "The background page failed to load.",
                out,
            ),
            NativeEvent::OffscreenLoaded { install, loaded } => {
                host.offscreen_loaded(install, loaded, out);
            }
            NativeEvent::OffscreenGone { install } => host.offscreen_gone(install, out),
        });
    }

    fn push_event(weak: &Weak<Self>, event: NativeEvent) {
        if let Some(inner) = weak.upgrade() {
            inner.events.borrow_mut().push_back(event);
            inner.schedule();
        }
    }

    fn arm(self: &Rc<Self>) {
        if self.retired.get() {
            return;
        }
        let Ok((deadline, now)) = self
            .bridge
            .with_runtime(|host, _| (host.next_deadline(), host.now()))
        else {
            return;
        };
        if let Some(previous) = self.timer.borrow_mut().take() {
            previous.remove();
        }
        let _ = self.bridge.with_runtime(|host, _| host.set_armed(deadline));
        let Some(deadline) = deadline else {
            return;
        };
        let nanos = deadline.saturating_sub(now).div_ceil(self.scale);
        let delay = Duration::from_nanos(nanos).clamp(Duration::from_millis(1), MAX_TIMER);
        let weak = Rc::downgrade(self);
        let source = glib::timeout_add_local_once(delay, move || {
            let Some(inner) = weak.upgrade() else {
                return;
            };
            inner.timer.borrow_mut().take();
            let _ = inner.bridge.with_runtime(|host, out| host.tick(out));
            inner.run();
        });
        *self.timer.borrow_mut() = Some(source);
    }

    fn execute(self: &Rc<Self>, op: NativeOp) {
        match op {
            NativeOp::StartBackground {
                install,
                generation,
                launch,
            } => {
                if let Err(message) = self.start_background(install, generation, *launch) {
                    let _ = self.bridge.with_runtime(|host, out| {
                        host.background_failed(install, generation, &message, out);
                    });
                }
            }
            NativeOp::StopBackground {
                install,
                generation,
            } => {
                self.drop_owned(install, ViewKindKey::Background, Some(generation));
                let _ = self
                    .bridge
                    .with_runtime(|host, out| host.background_stopped(install, generation, out));
            }
            NativeOp::DiscardBackground {
                install,
                generation,
                crashed,
            } => {
                if crashed {
                    self.drop_dead(install, ViewKindKey::Background, Some(generation));
                } else {
                    self.drop_owned(install, ViewKindKey::Background, Some(generation));
                }
            }
            NativeOp::CreateOffscreen {
                install,
                url,
                clipboard,
            } => {
                if let Err(()) = self.create_offscreen(install, &url, clipboard) {
                    let _ = self
                        .bridge
                        .with_runtime(|host, out| host.offscreen_loaded(install, false, out));
                }
            }
            NativeOp::OffscreenReady { install } => {
                let _ = self
                    .bridge
                    .with_runtime(|host, out| host.offscreen_loaded(install, true, out));
            }
            NativeOp::CloseOffscreen { install } => {
                self.drop_owned(install, ViewKindKey::Offscreen, None);
            }
            NativeOp::Notify(notice) => {
                if let Some(engine) = self.engine.upgrade() {
                    engine.notice(&notice);
                }
            }
        }
    }

    fn own_plan(&self, install: ExtensionInstallId) -> Option<AttachPlan> {
        let records = self.records.borrow();
        let record = records.get(&install)?;
        Some(AttachPlan {
            extension_pages: vec![ExtensionPages {
                extension: record.extension_id.clone(),
                sandboxed_paths: record.sandboxed.clone(),
            }],
            extension_bootstrap: Arc::from(""),
            ..AttachPlan::default()
        })
    }

    fn start_background(
        self: &Rc<Self>,
        install: ExtensionInstallId,
        generation: u64,
        launch: crate::background::Launch,
    ) -> Result<(), String> {
        self.drop_owned(install, ViewKindKey::Background, None);
        for (path, resource) in launch.resources {
            self.scheme
                .set_runtime_resource(install, path, resource)
                .map_err(|error| format!("The background could not be published: {error}"))?;
        }
        self.scheme
            .set_worker_wrapper(install, &launch.script, launch.wrapper)
            .map_err(|error| format!("The background could not be published: {error}"))?;
        let plan = self
            .own_plan(install)
            .ok_or_else(|| "The extension is not running.".to_owned())?;
        let view = create_view(&self.context, false, None);
        self.views.owned.borrow_mut().insert(
            (install, ViewKindKey::Background),
            Owned {
                install,
                generation,
                view: view.clone(),
                bridge: None,
                handlers: Vec::new(),
            },
        );
        let attached =
            self.bridge
                .attach_extension_view(install, ViewKind::Background, &view, &plan, false);
        let id = match attached {
            Ok(id) => id,
            Err(error) => {
                self.drop_owned(install, ViewKindKey::Background, Some(generation));
                return Err(format!("The background could not be attached: {error:?}"));
            }
        };
        let weak = Rc::downgrade(self);
        let crashed = view.connect_web_process_terminated(move |_, reason| {
            if reason != WebProcessTerminationReason::TerminatedByApi {
                Inner::push_event(
                    &weak,
                    NativeEvent::Crashed {
                        install,
                        generation,
                    },
                );
            }
        });
        let weak = Rc::downgrade(self);
        let failed = view.connect_load_failed(move |_, _, _, _| {
            Inner::push_event(
                &weak,
                NativeEvent::LoadFailed {
                    install,
                    generation,
                },
            );
            false
        });
        if let Some(owned) = self
            .views
            .owned
            .borrow_mut()
            .get_mut(&(install, ViewKindKey::Background))
        {
            owned.bridge = Some(id);
            owned.handlers = vec![crashed, failed];
        }
        view.load_uri(&launch.url);
        Ok(())
    }

    fn create_offscreen(
        self: &Rc<Self>,
        install: ExtensionInstallId,
        url: &str,
        clipboard: bool,
    ) -> Result<(), ()> {
        self.drop_owned(install, ViewKindKey::Offscreen, None);
        let script = self
            .page_script(install, ContextKind::Offscreen)
            .ok_or(())?;
        let plan = self.own_plan(install).ok_or(())?;
        let view = create_view(&self.context, clipboard, Some(&script));
        self.views.owned.borrow_mut().insert(
            (install, ViewKindKey::Offscreen),
            Owned {
                install,
                generation: 0,
                view: view.clone(),
                bridge: None,
                handlers: Vec::new(),
            },
        );
        let Ok(id) =
            self.bridge
                .attach_extension_view(install, ViewKind::Offscreen, &view, &plan, false)
        else {
            self.drop_owned(install, ViewKindKey::Offscreen, None);
            return Err(());
        };
        let weak = Rc::downgrade(self);
        let failed = view.connect_load_failed(move |_, _, _, _| {
            Inner::push_event(
                &weak,
                NativeEvent::OffscreenLoaded {
                    install,
                    loaded: false,
                },
            );
            false
        });
        let weak = Rc::downgrade(self);
        let gone = view.connect_web_process_terminated(move |_, reason| {
            if reason != WebProcessTerminationReason::TerminatedByApi {
                if let Some(inner) = weak.upgrade() {
                    inner.drop_dead(install, ViewKindKey::Offscreen, None);
                }
                Inner::push_event(&weak, NativeEvent::OffscreenGone { install });
            }
        });
        if let Some(owned) = self
            .views
            .owned
            .borrow_mut()
            .get_mut(&(install, ViewKindKey::Offscreen))
        {
            owned.bridge = Some(id);
            owned.handlers = vec![failed, gone];
        }
        view.load_uri(url);
        Ok(())
    }

    /// Detaches, terminates and drops a runtime-owned view.
    fn drop_owned(&self, install: ExtensionInstallId, key: ViewKindKey, generation: Option<u64>) {
        if let Some(owned) = self.take_owned(install, key, generation) {
            owned.view.terminate_web_process();
            self.views.ended(install, &owned.view);
        }
    }

    /// Drops a view whose process crashed. WebKit keeps the replies to
    /// messages sent to it after the crash (the bridge's re-attachment and
    /// detachment) pending, each holding the view, until the page has a
    /// process again; so it gets a blank one first and is dropped once
    /// that commits, or after a bound.
    fn drop_dead(
        self: &Rc<Self>,
        install: ExtensionInstallId,
        key: ViewKindKey,
        generation: Option<u64>,
    ) {
        let Some(owned) = self.take_owned(install, key, generation) else {
            return;
        };
        let view = owned.view;
        self.views.ended(install, &view);
        let slot: Rc<RefCell<Option<(webkit2gtk::WebView, Option<glib::SignalHandlerId>)>>> =
            Rc::new(RefCell::new(None));
        let release = {
            let slot = slot.clone();
            move || {
                if let Some((view, handler)) = slot.borrow_mut().take() {
                    if let Some(handler) = handler {
                        view.disconnect(handler);
                    }
                    view.terminate_web_process();
                }
            }
        };
        let committed = Rc::new(release);
        let on_load = committed.clone();
        let handler = view.connect_load_changed(move |_, event| {
            if matches!(
                event,
                webkit2gtk::LoadEvent::Committed | webkit2gtk::LoadEvent::Finished
            ) {
                let release = on_load.clone();
                glib::idle_add_local_once(move || release());
            }
        });
        *slot.borrow_mut() = Some((view.clone(), Some(handler)));
        let bound = committed.clone();
        glib::timeout_add_local_once(Duration::from_secs(5), move || bound());
        view.load_uri("about:blank");
    }

    fn take_owned(
        &self,
        install: ExtensionInstallId,
        key: ViewKindKey,
        generation: Option<u64>,
    ) -> Option<Owned> {
        let removed = {
            let mut owned = self.views.owned.borrow_mut();
            match owned.get(&(install, key)) {
                Some(view) if generation.is_none_or(|g| g == view.generation) => {
                    owned.remove(&(install, key))
                }
                _ => None,
            }
        };
        let mut owned = removed?;
        owned.disconnect();
        if let Some(id) = owned.bridge {
            self.bridge.detach(id);
        }
        Some(owned)
    }

    fn page_script(&self, install: ExtensionInstallId, kind: ContextKind) -> Option<PageScript> {
        let records = self.records.borrow();
        let record = records.get(&install)?;
        let bootstrap = self
            .bridge
            .with_runtime(|host, _| host.page_bootstrap(install, kind))
            .ok()
            .flatten()?;
        let id = record.extension_id.as_str();
        Some(PageScript {
            source: page_script(&bootstrap),
            allow: vec![format!("chrome-extension://{id}/*")],
            block: record
                .sandboxed
                .iter()
                .map(|path| format!("chrome-extension://{id}{path}"))
                .collect(),
        })
    }

    fn activate(
        self: &Rc<Self>,
        spec: InstallSpec,
        activation: &Activation,
    ) -> Result<(), InstallError> {
        if self.retired.get() {
            return Err(InstallError::Retired);
        }
        let background =
            check_background(&spec.manifest, &spec.directory).map_err(InstallError::Refused)?;
        if self.records.borrow().contains_key(&spec.install) {
            let _ = self.deactivate(spec.install, false);
        }
        let catalogs = read_locales(&spec.directory, &spec.manifest, &self.config.locale)
            .map_err(InstallError::Locales)?;
        let messages = Messages::new(
            spec.extension_id.clone(),
            self.config.locale.clone(),
            spec.manifest.default_locale(),
            &catalogs,
        )
        .map_err(|error| InstallError::Locales(error.message().to_owned()))?;
        self.scheme
            .add_install(
                spec.install,
                spec.extension_id.as_str(),
                &spec.directory,
                &spec.manifest,
            )
            .map_err(InstallError::Scheme)?;
        let admitted = self.bridge.admit(InstallAdmission {
            install: spec.install,
            extension_id: spec.extension_id.clone(),
            generation: spec.generation,
            allowed_in_incognito: spec.allowed_in_incognito,
            externally_connectable: spec.externally_connectable.clone(),
        });
        if let Err(error) = admitted {
            self.scheme.remove_install(spec.install);
            return Err(InstallError::Admission(format!("{error:?}")));
        }
        let state_directory = self
            .config
            .state_directory
            .as_ref()
            .map(|directory| directory.join(hex(spec.install)));
        let snapshot = |name: &str| {
            state_directory
                .as_ref()
                .and_then(|directory| storage::read_state(&directory.join(name)))
        };
        let sandboxed = spec
            .manifest
            .sandbox_pages()
            .into_iter()
            .map(|page| format!("/{page}"))
            .collect();
        self.records.borrow_mut().insert(
            spec.install,
            Record {
                extension_id: spec.extension_id.clone(),
                sandboxed,
                manifest: spec.manifest.clone(),
            },
        );
        let admitted = Admitted {
            install: spec.install,
            extension_id: spec.extension_id,
            listeners: snapshot("listeners.json"),
            alarms: snapshot("alarms.json"),
            manifest: spec.manifest,
            root: spec.directory,
            background,
            messages,
            permissions: spec.permissions,
            unpacked: spec.unpacked,
            state_directory,
        };
        let _ = self
            .bridge
            .with_runtime(|host, out| host.admit(admitted, activation, out));
        self.run();
        Ok(())
    }

    fn deactivate(self: &Rc<Self>, install: ExtensionInstallId, erase: bool) -> ExtensionStop {
        let _ = self
            .bridge
            .with_runtime(|host, out| host.stop(install, out));
        self.run();
        self.drop_owned(install, ViewKindKey::Background, None);
        self.drop_owned(install, ViewKindKey::Offscreen, None);
        let pages: Vec<PageEntry> = {
            let mut pages = self.views.pages.borrow_mut();
            let (ours, rest): (Vec<_>, Vec<_>) =
                pages.drain(..).partition(|page| page.install == install);
            *pages = rest;
            ours
        };
        for page in pages {
            self.bridge.detach(page.bridge);
            if let Some(view) = page.view.upgrade() {
                view.terminate_web_process();
                self.views.ended(install, &view);
            }
        }
        let _ = self
            .bridge
            .with_runtime(|host, out| host.forget(install, erase, out));
        self.bridge.retire(install);
        self.scheme.remove_install(install);
        self.records.borrow_mut().remove(&install);
        self.run();
        ExtensionStop {
            views: self.views.clone(),
            install,
            quiet_since: None,
        }
    }

    fn open_page(
        self: &Rc<Self>,
        install: ExtensionInstallId,
        kind: PageKind,
        path: Option<&str>,
    ) -> Result<ExtensionPage, ViewError> {
        if self.retired.get() {
            return Err(ViewError::Retired);
        }
        let (extension, declared) = {
            let records = self.records.borrow();
            let record = records.get(&install).ok_or(ViewError::UnknownInstall)?;
            let declared = match kind {
                PageKind::Options => record.manifest.options_page(),
                PageKind::Popup => record
                    .manifest
                    .action()
                    .and_then(|action| action.default_popup),
            };
            (record.extension_id.clone(), declared)
        };
        let page = match path {
            Some(path) => zephium_webext::manifest::normalize_resource(path)
                .filter(|path| !path.starts_with('_'))
                .ok_or(ViewError::InvalidPath)?,
            None => declared.ok_or(ViewError::NoPage)?,
        };
        let context = match kind {
            PageKind::Options => ContextKind::Options,
            PageKind::Popup => ContextKind::Popup,
        };
        let script = self
            .page_script(install, context)
            .ok_or(ViewError::UnknownInstall)?;
        let plan = self.own_plan(install).ok_or(ViewError::UnknownInstall)?;
        let view = create_view(&self.context, false, Some(&script));
        let serial = self.next_page.get();
        self.next_page.set(serial + 1);
        let id = self
            .bridge
            .attach_extension_view(install, kind.view_kind(), &view, &plan, false)
            .map_err(|error| ViewError::Attach(format!("{error:?}")))?;
        self.views.pages.borrow_mut().push(PageEntry {
            install,
            serial,
            view: view.downgrade(),
            bridge: id,
        });
        view.load_uri(&format!("chrome-extension://{}/{page}", extension.as_str()));
        let weak = Rc::downgrade(self);
        Ok(ExtensionPage {
            view,
            install,
            serial,
            detach: Some(Box::new(move |serial| {
                let Some(inner) = weak.upgrade() else {
                    return;
                };
                let entry = {
                    let mut pages = inner.views.pages.borrow_mut();
                    pages
                        .iter()
                        .position(|page| page.serial == serial)
                        .map(|index| pages.remove(index))
                };
                if let Some(entry) = entry {
                    inner.bridge.detach(entry.bridge);
                    if let Some(view) = entry.view.upgrade() {
                        view.terminate_web_process();
                        inner.views.ended(entry.install, &view);
                    }
                }
            })),
        })
    }

    fn retire(self: &Rc<Self>) -> RuntimeDrain {
        if !self.retired.get() {
            let _ = self.bridge.with_runtime(|host, out| host.shutdown(out));
            self.run();
            self.retired.set(true);
        }
        if let Some(timer) = self.timer.borrow_mut().take() {
            timer.remove();
        }
        if let Some(source) = self.storage_source.borrow_mut().take() {
            source.remove();
        }
        let owned: Vec<(ExtensionInstallId, ViewKindKey)> =
            self.views.owned.borrow().keys().copied().collect();
        for (install, key) in owned {
            self.drop_owned(install, key, None);
        }
        let pages: Vec<PageEntry> = self.views.pages.borrow_mut().drain(..).collect();
        for page in pages {
            if let Some(view) = page.view.upgrade() {
                self.views.ended(page.install, &view);
            }
        }
        let drain = self.bridge.shutdown();
        let _ = self.bridge.with_runtime(|host, _| host.stop_storage());
        self.scheme.retire();
        RuntimeDrain {
            bridge: drain,
            views: self.views.clone(),
        }
    }
}

impl Drop for Inner {
    fn drop(&mut self) {
        if let Some(timer) = self.timer.get_mut().take() {
            timer.remove();
        }
        if let Some(source) = self.storage_source.get_mut().take() {
            source.remove();
        }
    }
}

impl std::fmt::Debug for ProfileRuntime {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ProfileRuntime")
            .field("profile", &self.inner.config.profile)
            .finish_non_exhaustive()
    }
}

/// The state machine's view of one background, re-exported for callers.
pub type BackgroundLifecycle = BackgroundState;
