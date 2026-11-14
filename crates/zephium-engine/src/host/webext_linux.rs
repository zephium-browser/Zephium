//! Extensions on WebKitGTK: one Chrome-compatible runtime per durable profile
//! context, attached to ordinary tab views only. Private profiles run no
//! extensions. Work, agent, preview and privileged views never reach here.

mod popup;

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant};

use gtk::glib;
use gtk::glib::prelude::*;
use url::Url;
use webkit2gtk::{LoadEvent, WebViewExt, WebsiteDataManagerExt};
use zephium_core::extension_data::ExtensionDataReply;
use zephium_core::extensions::{
    ExtensionActionRejection, ExtensionActionRequest, ExtensionActionSettlement,
    ExtensionActionSnapshot, ExtensionActionSnapshotSettlement, ExtensionBrowserRequest,
    ExtensionBrowserRequestAction, ExtensionBrowserRequestId, ExtensionBrowserRequestSettlement,
    ExtensionBrowserSurface, ExtensionBrowserSurfaceGeneration, ExtensionRuntimeGeneration,
    ExtensionRuntimeInstance,
};
use zephium_core::ids::{ExtensionInstallId, ItemId, ProfileId};
use zephium_core::ports::engine::{EngineEvent, Partition, WebExtensionLoad, WebExtensionLoaded};
use zephium_webext::manifest::Manifest;
use zephium_webext_linux::bridge::BridgeViewId;
use zephium_webext_linux::runtime::{
    ActionActivation, BrowserCall, BrowserSettlement, InstallSpec, Notice, PermissionUpdate,
    ProfileRuntime, ProfileRuntimeConfig, RuntimeDrain, RuntimeEngine,
};
use zephium_webext_linux::scheme::Requester;
use zephium_webext_linux::views::PageKind;
use zephium_webext_runtime::context::ExtensionId;
use zephium_webext_runtime::i18n::LocaleEnvironment;
use zephium_webext_runtime::permissions::{
    InstallPermissions, PermissionDeclaration, PermissionRequest, SiteAccess,
};

use super::permits::Sink;

const MODULE_FILE: &str = "libzephium_webkit_web_process.so";
/// The shell answers every data or access request; this bounds one it never does.
const ANSWER_TIMEOUT: Duration = Duration::from_secs(60);
const DRAIN_TIMEOUT: Duration = Duration::from_secs(5);
const PRIVATE_UNSUPPORTED: &str = "Extensions do not run in private windows yet.";

thread_local! {
    /// Navigation admission for extension URLs, read from tab views'
    /// navigation handlers on the GTK thread.
    static GATES: RefCell<HashMap<ProfileId, std::rc::Weak<EngineSide>>> =
        RefCell::new(HashMap::new());
}

/// Whether a tab of `profile` may navigate (any frame) to the extension URL
/// `target`: the browser itself asked for exactly this load, or the
/// extension's own document or web-accessible-resource rules allow it.
pub(super) fn admits_extension_navigation(profile: ProfileId, tab: ItemId, target: &str) -> bool {
    let Some(side) =
        GATES.with(|gates| gates.borrow().get(&profile).and_then(|weak| weak.upgrade()))
    else {
        return false;
    };
    side.admits(tab, target)
}

#[derive(Default)]
struct TabFacts {
    view: Option<glib::WeakRef<webkit2gtk::WebView>>,
    committed: Option<Url>,
    /// A browser-authorized extension load not yet committed.
    authorized: Option<Url>,
    pending: Option<Url>,
}

/// What the runtime asks of the engine, on the GTK thread.
struct EngineSide {
    profile: ProfileId,
    sink: Sink,
    runtime: RefCell<Option<ProfileRuntime>>,
    installs: RefCell<HashMap<String, ExtensionInstallId>>,
    generations: RefCell<HashMap<ExtensionInstallId, ExtensionRuntimeGeneration>>,
    tabs: RefCell<HashMap<ItemId, TabFacts>>,
    requests: RefCell<HashMap<ExtensionBrowserRequestId, u64>>,
    answers: RefCell<HashMap<u64, u64>>,
    next: Cell<u64>,
    surface: Cell<Option<ExtensionBrowserSurfaceGeneration>>,
}

impl EngineSide {
    fn next(&self) -> u64 {
        let next = self.next.get().wrapping_add(1).max(1);
        self.next.set(next);
        next
    }

    fn install_of(&self, url: &Url) -> Option<ExtensionInstallId> {
        let id = zephium_core::navigation::extension_document_id(url)?;
        self.installs.borrow().get(id).copied()
    }

    fn admits(&self, tab: ItemId, target: &str) -> bool {
        let Ok(url) = Url::parse(target) else {
            return false;
        };
        if self.install_of(&url).is_none() {
            return false;
        }
        let authorized = self
            .tabs
            .borrow()
            .get(&tab)
            .and_then(|facts| facts.authorized.clone());
        if authorized.as_ref() == Some(&url) {
            if let Some(facts) = self.tabs.borrow_mut().get_mut(&tab) {
                facts.pending = Some(url);
            }
            return true;
        }
        let requester = self.requester_for(tab);
        let allowed = self
            .runtime
            .borrow()
            .as_ref()
            .is_some_and(|runtime| runtime.allows_navigation(&requester, target));
        allowed
    }

    fn requester_for(&self, tab: ItemId) -> Requester {
        let tabs = self.tabs.borrow();
        let facts = tabs.get(&tab);
        let pending = facts.and_then(|facts| facts.pending.clone());
        let committed = facts.and_then(|facts| facts.committed.clone());
        let pending_install = pending.as_ref().and_then(|url| self.install_of(url));
        let committed_install = committed.as_ref().and_then(|url| self.install_of(url));
        match (pending, pending_install) {
            (Some(url), Some(install)) => Requester::Tab {
                committed_top_level: url,
                extension_install: Some(install),
            },
            _ => Requester::Tab {
                committed_top_level: committed
                    .unwrap_or_else(|| Url::parse("about:blank").expect("about:blank")),
                extension_install: committed_install,
            },
        }
    }

    fn committed(&self, tab: ItemId, url: Option<Url>) {
        if let Some(facts) = self.tabs.borrow_mut().get_mut(&tab) {
            facts.committed = url;
            facts.pending = None;
            facts.authorized = None;
        }
    }

    fn request(&self, action: ExtensionBrowserRequestAction, local: u64) {
        let number = self.next();
        let Some(id) = ExtensionBrowserRequestId::new(number | (1 << 63)) else {
            return self.settle_later(local, rejected());
        };
        match ExtensionBrowserRequest::new(self.profile, id, action) {
            Ok(request) => {
                self.requests.borrow_mut().insert(id, local);
                self.sink
                    .emit(EngineEvent::ExtensionBrowserRequested { request });
            }
            Err(_) => self.settle_later(local, rejected()),
        }
    }

    fn settle_later(&self, request: u64, settlement: ExtensionBrowserRequestSettlement) {
        let profile = self.profile;
        glib::idle_add_local_once(move || {
            settle(profile, request, BrowserSettlement::Mutation(settlement));
        });
    }

    fn answer_timeout(&self, number: u64, settlement: BrowserSettlement) {
        let profile = self.profile;
        glib::timeout_add_local_once(ANSWER_TIMEOUT, move || {
            super::dispatch::best_effort_with(move |host| {
                if let Some(entry) = host.webext_linux.profiles.get(&profile) {
                    let local = entry.side.answers.borrow_mut().remove(&number);
                    if let Some(local) = local {
                        entry.runtime.settle_browser(local, settlement);
                    }
                }
            });
        });
    }
}

fn rejected() -> ExtensionBrowserRequestSettlement {
    ExtensionBrowserRequestSettlement::Rejected(
        zephium_core::extensions::ExtensionBrowserRequestRejection::NativeAdmissionFailed,
    )
}

fn settle(profile: ProfileId, request: u64, settlement: BrowserSettlement) {
    super::dispatch::best_effort_with(move |host| {
        if let Some(entry) = host.webext_linux.profiles.get(&profile) {
            entry.runtime.settle_browser(request, settlement);
        }
    });
}

impl RuntimeEngine for EngineSide {
    fn requester(&self, view: &webkit2gtk::WebView) -> Option<Requester> {
        let tab = self.tabs.borrow().iter().find_map(|(tab, facts)| {
            facts
                .view
                .as_ref()
                .and_then(glib::WeakRef::upgrade)
                .filter(|candidate| candidate == view)
                .map(|_| *tab)
        })?;
        Some(self.requester_for(tab))
    }

    fn notice(&self, notice: &Notice) {
        let profile = self.profile;
        match notice {
            Notice::NetworkStatus { install, status } => {
                if let Some(error) = &status.error {
                    eprintln!(
                        "extension {install} network rules kept their previous policy: {} characters",
                        error.chars().count()
                    );
                }
            }
            Notice::Error(error) => {
                if cfg!(debug_assertions) {
                    eprintln!("extension {} error: {}", error.install, error.message);
                } else {
                    eprintln!(
                        "extension {} error: {} characters",
                        error.install,
                        error.message.chars().count()
                    );
                }
            }
            Notice::Disabled(install) => {
                self.sink.emit(EngineEvent::WebExtensionSettled {
                    profile,
                    install: *install,
                    result: Err(
                        "The extension stopped after its background crashed repeatedly.".into(),
                    ),
                });
            }
            Notice::OpenOptionsPage(install) => {
                let install = *install;
                super::dispatch::best_effort_with(move |host| {
                    host.webext_linux.open_options(profile, install);
                });
            }
            Notice::Reload(install) => {
                let install = *install;
                super::dispatch::best_effort_with(move |host| {
                    let sink = host.sink.clone();
                    host.webext_linux.reload(profile, install, &sink);
                });
            }
        }
    }

    fn browser_call(&self, call: BrowserCall) {
        let profile = self.profile;
        match call {
            BrowserCall::Mutate { request, action } => self.request(action, request),
            BrowserCall::Data { request, call } => {
                let number = self.next();
                self.answers.borrow_mut().insert(number, request);
                self.sink
                    .emit(EngineEvent::WebExtensionDataRequested(Box::new(
                        zephium_core::ports::engine::WebExtensionDataRequest {
                            profile,
                            request: number,
                            call,
                        },
                    )));
                self.answer_timeout(
                    number,
                    BrowserSettlement::Data(ExtensionDataReply::Unavailable),
                );
            }
            BrowserCall::Prompt {
                request,
                extension_id,
                warnings,
                permissions,
                patterns,
            } => {
                let number = self.next();
                self.answers.borrow_mut().insert(number, request);
                self.sink
                    .emit(EngineEvent::WebExtensionAccessRequested(Box::new(
                        zephium_core::ports::engine::WebExtensionAccessRequest {
                            profile,
                            request: number,
                            extension_id,
                            warnings,
                            permissions,
                            patterns,
                        },
                    )));
            }
            BrowserCall::AccessRemoved {
                extension_id,
                permissions,
                patterns,
            } => self
                .sink
                .emit(EngineEvent::WebExtensionAccessRemoved(Box::new(
                    zephium_core::ports::engine::WebExtensionAccessRemoval {
                        profile,
                        extension_id,
                        permissions,
                        patterns,
                    },
                ))),
            BrowserCall::ActionsChanged => self
                .sink
                .emit(EngineEvent::ExtensionActionsInvalidated { profile }),
            BrowserCall::OpenPopup { install, tab } => {
                let generation = self.generations.borrow().get(&install).copied();
                if let (Some(generation), Some(surface)) = (generation, self.surface.get()) {
                    self.sink
                        .emit(EngineEvent::ExtensionActionShortcutRequested {
                            runtime: ExtensionRuntimeInstance::new(profile, install, generation),
                            tab,
                            surface_generation: surface,
                        });
                }
            }
            BrowserCall::Capture {
                request, tab, jpeg, ..
            } => {
                let _ = jpeg;
                super::dispatch::best_effort_with(move |host| {
                    host.capture_for_extension(profile, tab, request)
                });
            }
        }
    }
}

/// Grants being replaced on a running install, and what it reports once done.
type GrantChange = Result<(PermissionUpdate, WebExtensionLoaded), String>;

struct Install {
    load: WebExtensionLoad,
    generation: ExtensionRuntimeGeneration,
}

struct ProfileEntry {
    runtime: ProfileRuntime,
    context: webkit2gtk::WebContext,
    side: Rc<EngineSide>,
    installs: HashMap<ExtensionInstallId, Install>,
    tabs: HashMap<ItemId, BridgeViewId>,
    handlers: HashMap<ItemId, Vec<glib::SignalHandlerId>>,
    state: PathBuf,
    next_generation: u64,
}

/// The engine's Linux extension host.
#[derive(Default)]
pub(crate) struct LinuxExtensions {
    profiles: HashMap<ProfileId, ProfileEntry>,
    /// Why a profile has no runtime, when creating it failed.
    failed: HashMap<ProfileId, String>,
    module: Option<Result<PathBuf, String>>,
    popup: Option<popup::Popup>,
    draining: Vec<RuntimeDrain>,
    /// Work for a profile whose context does not exist yet; its first tab
    /// creates the context and the runtime with it.
    queued: HashMap<ProfileId, Vec<Queued>>,
}

enum Queued {
    Load(WebExtensionLoad),
    Unload(ExtensionInstallId),
    Remove(WebExtensionLoad),
}

const MAX_QUEUED: usize = 4 * zephium_core::extensions::MAX_EXTENSION_INSTALLS_PER_PROFILE;

fn locale() -> LocaleEnvironment {
    let raw = ["LC_ALL", "LC_MESSAGES", "LANG"]
        .iter()
        .find_map(|name| std::env::var(name).ok().filter(|value| !value.is_empty()))
        .unwrap_or_else(|| "en_US".into());
    let language = raw
        .split(['.', '@'])
        .next()
        .unwrap_or("en_US")
        .replace('_', "-");
    let language = if language == "C" || language == "POSIX" || language.is_empty() {
        "en-US".to_owned()
    } else {
        language
    };
    LocaleEnvironment {
        ui_language: language.clone(),
        accept_languages: vec![language],
        right_to_left: false,
    }
}

/// The directory WebKit loads the web-process module from. It must hold
/// only the module and be writable by nobody but its owner.
fn resolve_module() -> Result<PathBuf, String> {
    if let Some(directory) = std::env::var_os("ZEPHIUM_WEBEXT_MODULE_DIR").map(PathBuf::from) {
        return if directory.join(MODULE_FILE).is_file() {
            Ok(directory)
        } else {
            Err(format!("{} has no {MODULE_FILE}", directory.display()))
        };
    }
    let exe = std::env::current_exe().map_err(|error| error.to_string())?;
    let bin = exe.parent().ok_or("the executable has no directory")?;
    let mut installed = vec![
        bin.join("../lib/zephium/web-process"),
        bin.join("../lib64/zephium/web-process"),
    ];
    if Path::new("/.flatpak-info").exists() {
        installed.insert(0, PathBuf::from("/app/lib/zephium/web-process"));
    }
    for directory in installed {
        if directory.join(MODULE_FILE).is_file() {
            return directory.canonicalize().map_err(|error| error.to_string());
        }
    }
    // A development build keeps the module beside the executable, among other
    // libraries WebKit must not load; it runs from a private read-only copy.
    let built = bin.join(MODULE_FILE);
    if built.is_file() {
        return private_copy(&built);
    }
    Err("The Zephium web-process module is not installed.".into())
}

fn private_copy(module: &Path) -> Result<PathBuf, String> {
    use sha2::Digest;
    use std::os::unix::fs::PermissionsExt;
    let bytes = std::fs::read(module).map_err(|error| error.to_string())?;
    let digest = sha2::Sha256::digest(&bytes);
    let name: String = digest[..8]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let base = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .filter(|path| path.is_dir())
        .unwrap_or_else(std::env::temp_dir);
    let directory = base.join(format!("zephium-web-process-{name}"));
    let target = directory.join(MODULE_FILE);
    if target.is_file()
        && std::fs::read(&target).is_ok_and(|existing| existing == bytes)
        && std::fs::read_dir(&directory).is_ok_and(|entries| entries.count() == 1)
    {
        return Ok(directory);
    }
    let staging = tempfile::Builder::new()
        .prefix("zephium-web-process-")
        .tempdir_in(&base)
        .map_err(|error| error.to_string())?;
    std::fs::write(staging.path().join(MODULE_FILE), &bytes).map_err(|error| error.to_string())?;
    std::fs::set_permissions(
        staging.path().join(MODULE_FILE),
        std::fs::Permissions::from_mode(0o444),
    )
    .map_err(|error| error.to_string())?;
    std::fs::set_permissions(staging.path(), std::fs::Permissions::from_mode(0o555))
        .map_err(|error| error.to_string())?;
    let staged = staging.keep();
    if std::fs::rename(&staged, &directory).is_err() {
        let _ = std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o700));
        let _ = std::fs::remove_dir_all(&staged);
        if !target.is_file() {
            return Err("The web-process module could not be staged.".into());
        }
    }
    Ok(directory)
}

fn hex(install: ExtensionInstallId) -> String {
    install
        .bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// The site-access choice the desktop encoded in the granted patterns.
fn site_access(manifest: &Manifest, granted: &[String]) -> SiteAccess {
    let mut requested: Vec<String> = manifest.host_permissions();
    for script in manifest.content_scripts() {
        requested.extend(script.matches);
    }
    if requested.is_empty() || requested.iter().all(|pattern| granted.contains(pattern)) {
        return SiteAccess::OnAllSites;
    }
    if granted.is_empty() {
        return SiteAccess::OnClick;
    }
    SiteAccess::OnSpecificSites(
        granted
            .iter()
            .filter_map(|pattern| zephium_webext_runtime::pattern::HostPattern::parse(pattern).ok())
            .collect(),
    )
}

fn install_permissions(manifest: &Manifest, load: &WebExtensionLoad) -> InstallPermissions {
    let (declaration, _) = PermissionDeclaration::from_manifest(manifest);
    let permissions: Vec<&str> = load
        .permissions
        .iter()
        .map(String::as_str)
        .filter(|name| PermissionRequest::parse(&[*name], &[]).is_ok())
        .collect();
    let origins: Vec<&str> = load
        .match_patterns
        .iter()
        .map(String::as_str)
        .filter(|origin| PermissionRequest::parse(&[], &[*origin]).is_ok())
        .collect();
    let granted = PermissionRequest::parse(&permissions, &origins).unwrap_or_default();
    InstallPermissions::new(
        declaration,
        &granted,
        site_access(manifest, &load.match_patterns),
        false,
    )
}

impl LinuxExtensions {
    fn module(&mut self) -> Result<PathBuf, String> {
        self.module.get_or_insert_with(resolve_module).clone()
    }

    /// Sets up the profile's runtime on its new context, before any view of
    /// it loads. Private profiles get none.
    /// Starts the profile's runtime on a context that has no WebView yet:
    /// WebKit refuses sandbox paths once the context has a Web process. A
    /// runtime left on a context whose construction failed moves to this one.
    pub(super) fn prepare_context(
        &mut self,
        partition: Partition,
        context: &webkit2gtk::WebContext,
        profile_directory: Option<&Path>,
        sink: &Sink,
    ) {
        let profile = partition.profile();
        if let Some(entry) = self.profiles.get(&profile) {
            if entry.context == *context {
                return;
            }
            self.retire_profile(profile);
        }
        if self.failed.contains_key(&profile) {
            return;
        }
        self.create_runtime(partition, context, profile_directory, sink);
        let queued = self.queued.remove(&profile).unwrap_or_default();
        for work in queued {
            match work {
                Queued::Load(load) => self.load(profile, load, sink),
                Queued::Unload(install) => self.unload(profile, install, sink),
                Queued::Remove(load) => {
                    use webkit2gtk::WebContextExt;
                    let manager = context.website_data_manager();
                    self.remove(profile, load, manager, sink);
                }
            }
        }
        // The profile's installs restored with its first view: browser start.
        if let Some(entry) = self.profiles.get(&profile) {
            entry.runtime.browser_started();
        }
    }

    /// The load to run now, or `None` when it waits for the profile's context.
    fn queue_or(
        &mut self,
        profile: ProfileId,
        load: WebExtensionLoad,
        work: fn(WebExtensionLoad) -> Queued,
    ) -> Option<WebExtensionLoad> {
        if self.profiles.contains_key(&profile) || self.failed.contains_key(&profile) {
            return Some(load);
        }
        let queued = self.queued.entry(profile).or_default();
        if queued.len() >= MAX_QUEUED {
            return Some(load);
        }
        queued.push(work(load));
        None
    }

    fn create_runtime(
        &mut self,
        partition: Partition,
        context: &webkit2gtk::WebContext,
        profile_directory: Option<&Path>,
        sink: &Sink,
    ) {
        let profile = partition.profile();
        let (Partition::Default(_) | Partition::Persistent(_), Some(directory)) =
            (partition, profile_directory)
        else {
            return;
        };
        if self.profiles.contains_key(&profile) {
            return;
        }
        let module = match self.module() {
            Ok(module) => module,
            Err(error) => {
                eprintln!("extensions: unavailable: {error}");
                self.failed.insert(profile, error);
                return;
            }
        };
        let state = directory.join("webext");
        let side = Rc::new(EngineSide {
            profile,
            sink: sink.clone(),
            runtime: RefCell::new(None),
            installs: RefCell::new(HashMap::new()),
            generations: RefCell::new(HashMap::new()),
            tabs: RefCell::new(HashMap::new()),
            requests: RefCell::new(HashMap::new()),
            answers: RefCell::new(HashMap::new()),
            next: Cell::new(0),
            surface: Cell::new(None),
        });
        let config = ProfileRuntimeConfig {
            profile,
            incognito: false,
            module_directory: module,
            state_directory: Some(state.clone()),
            locale: locale(),
            time_scale: 1,
        };
        let engine: Rc<dyn RuntimeEngine> = side.clone();
        match ProfileRuntime::new(context, config, engine) {
            Ok(runtime) => {
                *side.runtime.borrow_mut() = Some(runtime.clone());
                GATES.with(|gates| {
                    gates.borrow_mut().insert(profile, Rc::downgrade(&side));
                });
                self.failed.remove(&profile);
                self.profiles.insert(
                    profile,
                    ProfileEntry {
                        runtime,
                        context: context.clone(),
                        side,
                        installs: HashMap::new(),
                        tabs: HashMap::new(),
                        handlers: HashMap::new(),
                        state,
                        next_generation: 1,
                    },
                );
            }
            Err(error) => {
                eprintln!("extensions: profile runtime unavailable: {error}");
                self.failed
                    .insert(profile, "The extension runtime could not start.".into());
            }
        }
    }

    /// Attaches an ordinary tab before its first load.
    pub(super) fn attach_tab(&mut self, partition: Partition, tab: ItemId, view: &wry::WebView) {
        use wry::WebViewExtUnix;
        let Some(entry) = self.profiles.get_mut(&partition.profile()) else {
            return;
        };
        if matches!(partition, Partition::Ephemeral(_)) {
            return;
        }
        let webview = view.webview();
        entry.side.tabs.borrow_mut().insert(
            tab,
            TabFacts {
                view: Some(webview.downgrade()),
                ..TabFacts::default()
            },
        );
        let side = Rc::downgrade(&entry.side);
        let loaded = webview.connect_load_changed(move |view, event| {
            let Some(side) = side.upgrade() else {
                return;
            };
            if event == LoadEvent::Committed {
                side.committed(tab, view.uri().and_then(|uri| Url::parse(&uri).ok()));
            }
        });
        let side = Rc::downgrade(&entry.side);
        let failed = webview.connect_load_failed(move |_, _, _, _| {
            if let Some(side) = side.upgrade() {
                if let Some(facts) = side.tabs.borrow_mut().get_mut(&tab) {
                    facts.pending = None;
                    facts.authorized = None;
                }
            }
            false
        });
        entry.handlers.insert(tab, vec![loaded, failed]);
        match entry.runtime.attach_tab(tab, &webview) {
            Ok(id) => {
                entry.tabs.insert(tab, id);
            }
            Err(error) => eprintln!("extensions: a tab could not be attached: {error:?}"),
        }
    }

    /// The tab's view is going away.
    pub(super) fn detach_tab(&mut self, tab: ItemId, view: Option<&wry::WebView>) {
        use wry::WebViewExtUnix;
        if self.popup.as_ref().is_some_and(|popup| popup.tab() == tab) {
            self.close_popup();
        }
        for entry in self.profiles.values_mut() {
            if let Some(id) = entry.tabs.remove(&tab) {
                entry.runtime.detach_tab(id);
            }
            if let Some(handlers) = entry.handlers.remove(&tab) {
                if let Some(view) = view {
                    let webview = view.webview();
                    for handler in handlers {
                        webview.disconnect(handler);
                    }
                }
            }
            entry.side.tabs.borrow_mut().remove(&tab);
        }
    }

    /// The browser itself navigates `tab` to `url` (address field, an
    /// extension's `tabs.create`/`update`, the options page).
    /// Whether the browser may open a new tab of `partition` directly at
    /// `url`: a page of an install this profile's runtime is running.
    pub(super) fn admits_initial(&self, partition: Partition, url: &str) -> bool {
        !matches!(partition, Partition::Ephemeral(_))
            && Url::parse(url).is_ok_and(|url| {
                zephium_core::navigation::extension_document_id(&url).is_some()
                    && self
                        .profiles
                        .get(&partition.profile())
                        .is_some_and(|entry| entry.side.install_of(&url).is_some())
            })
    }

    pub(super) fn authorize_navigation(&self, profile: ProfileId, tab: ItemId, url: &str) {
        let Some(entry) = self.profiles.get(&profile) else {
            return;
        };
        let Ok(url) = Url::parse(url) else {
            return;
        };
        if entry.side.install_of(&url).is_none() {
            return;
        }
        if let Some(facts) = entry.side.tabs.borrow_mut().get_mut(&tab) {
            facts.authorized = Some(url);
        }
    }

    pub(super) fn publish(&mut self, surface: &ExtensionBrowserSurface) {
        let Some(entry) = self.profiles.get(&surface.profile()) else {
            return;
        };
        entry.side.surface.set(Some(surface.generation()));
        if let Some(popup) = &self.popup {
            let active = surface
                .windows()
                .iter()
                .any(|window| window.active() == Some(popup.tab()));
            if popup.profile() == surface.profile() && !active {
                self.close_popup();
            }
        }
        let entry = self.profiles.get(&surface.profile()).expect("entry");
        entry.runtime.publish_surface(surface.clone());
    }

    pub(super) fn load(&mut self, profile: ProfileId, load: WebExtensionLoad, sink: &Sink) {
        let install = load.install;
        let Some(load) = self.queue_or(profile, load, Queued::Load) else {
            return;
        };
        if let Some(changed) = self.change_grants(profile, &load) {
            let sink = sink.clone();
            match changed {
                Ok((update, loaded)) => {
                    // Success is reported only once the grants are in force.
                    let started = Instant::now();
                    glib::timeout_add_local(Duration::from_millis(10), move || {
                        let result = match update.poll() {
                            None if started.elapsed() < DRAIN_TIMEOUT => {
                                return glib::ControlFlow::Continue;
                            }
                            None => Err("The extension's access did not update in time.".into()),
                            Some(result) => result.map(|()| loaded.clone()),
                        };
                        sink.emit(EngineEvent::WebExtensionSettled {
                            profile,
                            install,
                            result,
                        });
                        sink.emit(EngineEvent::ExtensionActionsInvalidated { profile });
                        glib::ControlFlow::Break
                    });
                }
                Err(error) => sink.emit(EngineEvent::WebExtensionSettled {
                    profile,
                    install,
                    result: Err(error),
                }),
            }
            return;
        }
        let result = self.load_install(profile, load);
        if let Err(error) = &result {
            eprintln!("extensions: {install} failed to load: {error}");
        }
        sink.emit(EngineEvent::WebExtensionSettled {
            profile,
            install,
            result,
        });
        sink.emit(EngineEvent::ExtensionActionsInvalidated { profile });
    }

    /// A load of an install already running at the same version: only its
    /// access changed, so its grants are replaced in place.
    fn change_grants(
        &mut self,
        profile: ProfileId,
        load: &WebExtensionLoad,
    ) -> Option<GrantChange> {
        let entry = self.profiles.get_mut(&profile)?;
        entry.installs.get(&load.install)?;
        let manifest = Manifest::load(&load.root).ok()?;
        let loaded = WebExtensionLoaded {
            name: manifest.name().unwrap_or_else(|| load.extension_id.clone()),
            version: manifest.version().unwrap_or_default().to_owned(),
        };
        let recorded =
            std::fs::read_to_string(entry.state.join("versions").join(hex(load.install))).ok()?;
        if recorded != loaded.version {
            return None;
        }
        let update = entry
            .runtime
            .permissions_changed(load.install, install_permissions(&manifest, load));
        if update.is_ok() {
            if let Some(install) = entry.installs.get_mut(&load.install) {
                install.load = load.clone();
            }
        }
        Some(update.map(|update| (update, loaded)))
    }

    fn load_install(
        &mut self,
        profile: ProfileId,
        load: WebExtensionLoad,
    ) -> Result<WebExtensionLoaded, String> {
        let Some(entry) = self.profiles.get_mut(&profile) else {
            return Err(self
                .failed
                .get(&profile)
                .cloned()
                .unwrap_or_else(|| "Extensions are unavailable for this profile.".into()));
        };
        let extension_id = ExtensionId::parse(&load.extension_id)
            .ok_or_else(|| "Invalid extension identity.".to_owned())?;
        let manifest = Manifest::load(&load.root).map_err(|error| error.to_string())?;
        ProfileRuntime::check_manifest(&manifest, &load.root)
            .map_err(|refusal| refusal.to_string())?;
        let loaded = WebExtensionLoaded {
            name: manifest.name().unwrap_or_else(|| load.extension_id.clone()),
            version: manifest.version().unwrap_or_default().to_owned(),
        };
        let generation = ExtensionRuntimeGeneration::new(entry.next_generation)
            .ok_or("Extension generation exhausted.")?;
        entry.next_generation += 1;
        let version_file = entry.state.join("versions").join(hex(load.install));
        let previous = std::fs::read_to_string(&version_file).ok();
        let spec = InstallSpec {
            install: load.install,
            extension_id,
            generation,
            directory: load.root.clone(),
            permissions: install_permissions(&manifest, &load),
            allowed_in_incognito: false,
            externally_connectable: None,
            unpacked: false,
            manifest,
        };
        // Install ids are ULIDs: their leading bytes order installs by time.
        let mut ordinal = [0; 8];
        ordinal.copy_from_slice(&load.install.bytes()[..8]);
        // An update keeps the order its install was activated with.
        let _ = entry
            .runtime
            .set_install_order(load.install, u64::from_be_bytes(ordinal));
        let result = match previous.as_deref() {
            None => entry.runtime.install_admitted(spec),
            Some(version) if version != loaded.version => {
                entry.runtime.install_updated(spec, version)
            }
            Some(_) => entry.runtime.install_enabled(spec),
        };
        result.map_err(|error| error.to_string())?;
        if let Some(parent) = version_file.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(&version_file, &loaded.version);
        entry
            .side
            .installs
            .borrow_mut()
            .insert(load.extension_id.clone(), load.install);
        entry
            .side
            .generations
            .borrow_mut()
            .insert(load.install, generation);
        entry
            .installs
            .insert(load.install, Install { load, generation });
        Ok(loaded)
    }

    fn stop(
        &mut self,
        profile: ProfileId,
        install: ExtensionInstallId,
        erase: bool,
    ) -> Option<WebExtensionLoad> {
        if self
            .popup
            .as_ref()
            .is_some_and(|popup| popup.profile() == profile && popup.install() == install)
        {
            self.close_popup();
        }
        let entry = self.profiles.get_mut(&profile)?;
        let removed = entry.installs.remove(&install);
        if let Some(installed) = &removed {
            entry
                .side
                .installs
                .borrow_mut()
                .remove(&installed.load.extension_id);
        }
        entry.side.generations.borrow_mut().remove(&install);
        let mut stop = if erase {
            entry.runtime.install_removed(install)
        } else {
            entry.runtime.install_disabled(install)
        };
        // Pumped from the loop; erasure of the origin's data follows.
        glib::timeout_add_local(Duration::from_millis(10), {
            let started = Instant::now();
            move || {
                if stop.poll() || started.elapsed() > DRAIN_TIMEOUT {
                    glib::ControlFlow::Break
                } else {
                    glib::ControlFlow::Continue
                }
            }
        });
        removed.map(|installed| installed.load)
    }

    pub(super) fn unload(&mut self, profile: ProfileId, install: ExtensionInstallId, sink: &Sink) {
        if let Some(queued) = self.queued.get_mut(&profile) {
            queued.retain(|work| !matches!(work, Queued::Load(load) if load.install == install));
            if !self.profiles.contains_key(&profile) {
                queued.push(Queued::Unload(install));
                return;
            }
        }
        self.stop(profile, install, false);
        sink.emit(EngineEvent::ExtensionActionsInvalidated { profile });
    }

    pub(super) fn remove(
        &mut self,
        profile: ProfileId,
        load: WebExtensionLoad,
        manager: Option<webkit2gtk::WebsiteDataManager>,
        sink: &Sink,
    ) {
        if let Some(queued) = self.queued.get_mut(&profile) {
            queued.retain(
                |work| !matches!(work, Queued::Load(queued) if queued.install == load.install),
            );
        }
        let Some(load) = self.queue_or(profile, load, Queued::Remove) else {
            return;
        };
        self.stop(profile, load.install, true);
        if let Some(entry) = self.profiles.get(&profile) {
            let state = entry.state.clone();
            let _ = std::fs::remove_file(state.join("versions").join(hex(load.install)));
            let directory = state.join(hex(load.install));
            glib::timeout_add_local_once(Duration::from_millis(200), move || {
                let _ = std::fs::remove_dir_all(directory);
            });
        }
        if let Some(manager) = manager {
            erase_origin(&manager, &load.extension_id);
        }
        sink.emit(EngineEvent::ExtensionActionsInvalidated { profile });
    }

    fn reload(&mut self, profile: ProfileId, install: ExtensionInstallId, sink: &Sink) {
        let Some(load) = self.stop(profile, install, false) else {
            return;
        };
        self.load(profile, load, sink);
    }

    fn open_options(&mut self, profile: ProfileId, install: ExtensionInstallId) {
        let Some(entry) = self.profiles.get(&profile) else {
            return;
        };
        let Some(installed) = entry.installs.get(&install) else {
            return;
        };
        let Some(page) = Manifest::load(&installed.load.root)
            .ok()
            .and_then(|manifest| manifest.options_page())
        else {
            return;
        };
        let url = format!("chrome-extension://{}/{page}", installed.load.extension_id);
        let local = entry.side.next() | (1 << 62);
        entry.side.request(
            ExtensionBrowserRequestAction::CreateTab {
                window: None,
                url: Some(std::sync::Arc::from(url.as_str())),
                active: true,
            },
            local,
        );
    }

    pub(super) fn open_options_for(&mut self, profile: ProfileId, extension_id: &str) {
        let install = self
            .profiles
            .get(&profile)
            .and_then(|entry| entry.side.installs.borrow().get(extension_id).copied());
        if let Some(install) = install {
            self.open_options(profile, install);
        }
    }

    pub(super) fn settle_browser_request(
        &mut self,
        profile: ProfileId,
        request: ExtensionBrowserRequestId,
        settlement: ExtensionBrowserRequestSettlement,
    ) -> bool {
        let Some(entry) = self.profiles.get(&profile) else {
            return false;
        };
        let Some(local) = entry.side.requests.borrow_mut().remove(&request) else {
            return false;
        };
        if local & (1 << 62) == 0 {
            entry
                .runtime
                .settle_browser(local, BrowserSettlement::Mutation(settlement));
        }
        true
    }

    pub(super) fn answer_access(&mut self, profile: ProfileId, request: u64, allowed: bool) {
        if let Some(entry) = self.profiles.get(&profile) {
            let local = entry.side.answers.borrow_mut().remove(&request);
            if let Some(local) = local {
                entry
                    .runtime
                    .settle_browser(local, BrowserSettlement::Prompt(allowed));
            }
        }
    }

    pub(super) fn answer_data(
        &mut self,
        profile: ProfileId,
        request: u64,
        reply: ExtensionDataReply,
    ) {
        if let Some(entry) = self.profiles.get(&profile) {
            let local = entry.side.answers.borrow_mut().remove(&request);
            if let Some(local) = local {
                entry
                    .runtime
                    .settle_browser(local, BrowserSettlement::Data(reply));
            }
        }
    }

    pub(super) fn settle_capture(
        &mut self,
        profile: ProfileId,
        request: u64,
        result: Result<String, String>,
    ) {
        if let Some(entry) = self.profiles.get(&profile) {
            entry
                .runtime
                .settle_browser(request, BrowserSettlement::Capture(result));
        }
    }

    pub(super) fn actions(
        &mut self,
        profile: ProfileId,
        tab: ItemId,
        surface_generation: ExtensionBrowserSurfaceGeneration,
    ) -> ExtensionActionSnapshotSettlement {
        let Some(entry) = self.profiles.get(&profile) else {
            return ExtensionActionSnapshotSettlement::Rejected(
                ExtensionActionRejection::RuntimeUnavailable,
            );
        };
        let runtimes: HashMap<ExtensionInstallId, ExtensionRuntimeInstance> = entry
            .installs
            .iter()
            .map(|(install, installed)| {
                (
                    *install,
                    ExtensionRuntimeInstance::new(profile, *install, installed.generation),
                )
            })
            .collect();
        let actions = entry.runtime.action_states(tab, &runtimes);
        match ExtensionActionSnapshot::new(profile, tab, surface_generation, actions) {
            Ok(snapshot) => ExtensionActionSnapshotSettlement::Applied(snapshot),
            Err(_) => ExtensionActionSnapshotSettlement::Rejected(
                ExtensionActionRejection::NativeAdmissionFailed,
            ),
        }
    }

    pub(super) fn invoke(&mut self, request: ExtensionActionRequest) -> ExtensionActionSettlement {
        let profile = request.runtime().profile();
        let install = request.runtime().install_id();
        let Some(entry) = self.profiles.get(&profile) else {
            return ExtensionActionSettlement::Rejected(
                ExtensionActionRejection::RuntimeUnavailable,
            );
        };
        let Some(installed) = entry.installs.get(&install) else {
            return ExtensionActionSettlement::Rejected(
                ExtensionActionRejection::RuntimeUnavailable,
            );
        };
        if installed.generation != request.runtime().generation() {
            return ExtensionActionSettlement::Rejected(
                ExtensionActionRejection::RuntimeSuperseded,
            );
        }
        let extension_id = installed.load.extension_id.clone();
        if request.is_menu() {
            return ExtensionActionSettlement::Rejected(
                ExtensionActionRejection::ActionUnavailable,
            );
        }
        let toggling = self
            .popup
            .as_ref()
            .is_some_and(|popup| popup.install() == install && popup.tab() == request.tab());
        self.close_popup();
        if toggling {
            return ExtensionActionSettlement::Dispatched;
        }
        let entry = self.profiles.get(&profile).expect("entry");
        let activation = match entry.runtime.invoke_action(install, request.tab()) {
            Ok(activation) => activation,
            Err(error) => {
                eprintln!("extensions: action not invoked: {error}");
                return ExtensionActionSettlement::Rejected(
                    ExtensionActionRejection::ActionDisabled,
                );
            }
        };
        let ActionActivation::Popup(url) = activation else {
            return ExtensionActionSettlement::Dispatched;
        };
        let path = Url::parse(&url)
            .ok()
            .filter(|parsed| {
                zephium_core::navigation::extension_document_id(parsed)
                    == Some(extension_id.as_str())
            })
            .map(|parsed| parsed.path().trim_start_matches('/').to_owned());
        let Some(path) = path else {
            return ExtensionActionSettlement::Rejected(ExtensionActionRejection::PopupUnavailable);
        };
        let page = match entry
            .runtime
            .open_page(install, PageKind::Popup, Some(&path))
        {
            Ok(page) => page,
            Err(error) => {
                eprintln!("extensions: popup unavailable: {error:?}");
                return ExtensionActionSettlement::Rejected(
                    ExtensionActionRejection::PopupUnavailable,
                );
            }
        };
        match popup::Popup::show(
            profile,
            install,
            request.tab(),
            page,
            request.anchor().rect(),
        ) {
            Ok(popup) => {
                self.popup = Some(popup);
                ExtensionActionSettlement::Dispatched
            }
            Err(error) => {
                eprintln!("extensions: popup not shown: {error}");
                ExtensionActionSettlement::Rejected(ExtensionActionRejection::PopupUnavailable)
            }
        }
    }

    pub(super) fn close_popup(&mut self) {
        if let Some(popup) = self.popup.take() {
            popup.close();
        }
    }

    pub(super) fn popup_closed(&mut self, serial: u64) {
        if self
            .popup
            .as_ref()
            .is_some_and(|popup| popup.serial() == serial)
        {
            self.close_popup();
        }
    }

    /// Profile teardown: every view of the runtime goes; the drain is pumped
    /// from the loop.
    pub(super) fn retire_profile(&mut self, profile: ProfileId) {
        if self
            .popup
            .as_ref()
            .is_some_and(|popup| popup.profile() == profile)
        {
            self.close_popup();
        }
        GATES.with(|gates| {
            gates.borrow_mut().remove(&profile);
        });
        if let Some(entry) = self.profiles.remove(&profile) {
            for (_, id) in entry.tabs {
                entry.runtime.detach_tab(id);
            }
            *entry.side.runtime.borrow_mut() = None;
            let drain = entry.runtime.retire();
            self.draining.push(drain);
            self.pump_drains();
        }
    }

    fn pump_drains(&mut self) {
        if self.draining.is_empty() {
            return;
        }
        let mut drains = std::mem::take(&mut self.draining);
        let started = Instant::now();
        glib::timeout_add_local(Duration::from_millis(10), move || {
            drains.retain_mut(|drain| !drain.poll());
            if drains.is_empty() || started.elapsed() > DRAIN_TIMEOUT {
                glib::ControlFlow::Break
            } else {
                glib::ControlFlow::Continue
            }
        });
    }

    /// App shutdown: retires every runtime and calls `done` once their views
    /// have drained, or after a bound.
    pub(super) fn shutdown(
        &mut self,
        done: Box<dyn FnOnce(bool) + Send>,
    ) -> Box<dyn FnOnce(bool) + Send> {
        self.close_popup();
        let profiles: Vec<ProfileId> = self.profiles.keys().copied().collect();
        let mut drains = Vec::new();
        for profile in profiles {
            GATES.with(|gates| {
                gates.borrow_mut().remove(&profile);
            });
            if let Some(entry) = self.profiles.remove(&profile) {
                for (_, id) in entry.tabs {
                    entry.runtime.detach_tab(id);
                }
                *entry.side.runtime.borrow_mut() = None;
                drains.push(entry.runtime.retire());
            }
        }
        drains.append(&mut self.draining);
        if drains.is_empty() {
            return done;
        }
        let joint: std::sync::Arc<std::sync::Mutex<Joint>> = std::sync::Arc::default();
        joint.lock().unwrap_or_else(|error| error.into_inner()).done = Some(done);
        let drained = joint.clone();
        let started = Instant::now();
        glib::timeout_add_local(Duration::from_millis(10), move || {
            drains.retain_mut(|drain| !drain.poll());
            let timed_out = started.elapsed() > DRAIN_TIMEOUT;
            if drains.is_empty() || timed_out {
                Joint::finish(&drained, None, Some(!timed_out));
                glib::ControlFlow::Break
            } else {
                glib::ControlFlow::Continue
            }
        });
        Box::new(move |clean| Joint::finish(&joint, Some(clean), None))
    }
}

#[derive(Default)]
struct Joint {
    done: Option<Box<dyn FnOnce(bool) + Send>>,
    engine: Option<bool>,
    drained: Option<bool>,
}

impl Joint {
    fn finish(joint: &std::sync::Mutex<Joint>, engine: Option<bool>, drained: Option<bool>) {
        let ready = {
            let mut joint = joint.lock().unwrap_or_else(|error| error.into_inner());
            if engine.is_some() {
                joint.engine = engine;
            }
            if drained.is_some() {
                joint.drained = drained;
            }
            match (joint.engine, joint.drained) {
                (Some(engine), Some(drained)) => {
                    joint.done.take().map(|done| (done, engine && drained))
                }
                _ => None,
            }
        };
        if let Some((done, clean)) = ready {
            done(clean);
        }
    }
}

/// Removes the extension origin's website data (IndexedDB, local storage,
/// caches) from the profile's data manager.
fn erase_origin(manager: &webkit2gtk::WebsiteDataManager, extension_id: &str) {
    let types = webkit2gtk::WebsiteDataTypes::ALL;
    let manager_for_remove = manager.clone();
    let extension = extension_id.to_owned();
    manager.fetch(types, None::<&gtk::gio::Cancellable>, move |result| {
        let Ok(records) = result else {
            return;
        };
        let records: Vec<webkit2gtk::WebsiteData> = records
            .into_iter()
            .filter(|record| record.name().is_some_and(|name| name.contains(&extension)))
            .collect();
        if records.is_empty() {
            return;
        }
        let records: Vec<&webkit2gtk::WebsiteData> = records.iter().collect();
        webkit2gtk::WebsiteDataManagerExtManual::remove(
            &manager_for_remove,
            types,
            &records,
            None::<&gtk::gio::Cancellable>,
            |result| {
                if result.is_err() {
                    eprintln!("extensions: a removed extension's website data was not erased");
                }
            },
        );
    });
}

impl super::EngineHost {
    pub(crate) fn load_web_extension(&mut self, profile: ProfileId, load: WebExtensionLoad) {
        let sink = self.sink.clone();
        if self
            .profile_persistence_classes
            .get(&profile)
            .is_some_and(|class| *class == super::ProfilePersistenceClass::Ephemeral)
        {
            sink.emit(EngineEvent::WebExtensionSettled {
                profile,
                install: load.install,
                result: Err(PRIVATE_UNSUPPORTED.into()),
            });
            return;
        }
        self.webext_linux.load(profile, load, &sink);
    }

    pub(crate) fn unload_web_extension(&mut self, profile: ProfileId, install: ExtensionInstallId) {
        let sink = self.sink.clone();
        self.webext_linux.unload(profile, install, &sink);
    }

    pub(crate) fn remove_web_extension(&mut self, profile: ProfileId, load: WebExtensionLoad) {
        let sink = self.sink.clone();
        let manager = self
            .linux_data_managers
            .get(&profile)
            .and_then(|managers| managers.first().cloned());
        self.webext_linux.remove(profile, load, manager, &sink);
    }

    pub(crate) fn open_web_extension_options(&mut self, profile: ProfileId, extension_id: &str) {
        self.webext_linux.open_options_for(profile, extension_id);
    }

    pub(crate) fn answer_web_extension_access(
        &mut self,
        profile: ProfileId,
        request: u64,
        allowed: bool,
    ) {
        self.webext_linux.answer_access(profile, request, allowed);
    }

    pub(crate) fn answer_web_extension_data(
        &mut self,
        profile: ProfileId,
        request: u64,
        reply: ExtensionDataReply,
    ) {
        self.webext_linux.answer_data(profile, request, reply);
    }

    pub(crate) fn web_extension_data_changed(
        &mut self,
        _profile: ProfileId,
        _change: zephium_core::extension_data::ExtensionDataChange,
    ) {
    }

    pub(crate) fn extension_actions_snapshot(
        &mut self,
        profile: ProfileId,
        tab: ItemId,
        surface_generation: ExtensionBrowserSurfaceGeneration,
    ) -> ExtensionActionSnapshotSettlement {
        self.webext_linux.actions(profile, tab, surface_generation)
    }

    pub(crate) fn invoke_extension_action(
        &mut self,
        request: ExtensionActionRequest,
    ) -> ExtensionActionSettlement {
        self.webext_linux.invoke(request)
    }

    pub(crate) fn settle_linux_extension_browser_request(
        &mut self,
        profile: ProfileId,
        request: ExtensionBrowserRequestId,
        settlement: ExtensionBrowserRequestSettlement,
    ) -> bool {
        self.webext_linux
            .settle_browser_request(profile, request, settlement)
    }

    fn capture_for_extension(&mut self, profile: ProfileId, tab: ItemId, request: u64) {
        let Some(url) = self
            .views
            .get(&tab)
            .filter(|_| {
                self.partitions
                    .get(&tab)
                    .is_some_and(|partition| partition.profile() == profile)
            })
            .and_then(|view| view.navigation.committed_snapshot())
            .map(|(_, committed)| committed)
        else {
            self.webext_linux.settle_capture(
                profile,
                request,
                Err("Cannot capture a tab that is not shown.".into()),
            );
            return;
        };
        self.capture_tab_preview(
            tab,
            url,
            Box::new(move |bytes| {
                super::dispatch::best_effort_with(move |host| {
                    let result = bytes
                        .map(|bytes| {
                            use base64::Engine as _;
                            format!(
                                "data:image/jpeg;base64,{}",
                                base64::engine::general_purpose::STANDARD.encode(bytes)
                            )
                        })
                        .ok_or_else(|| "Failed to capture tab: the tab is not visible.".to_owned());
                    host.webext_linux.settle_capture(profile, request, result);
                });
            }),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(json: &str) -> Manifest {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("manifest.json"), json).unwrap();
        Manifest::load(directory.path()).unwrap()
    }

    #[test]
    fn granted_patterns_map_to_the_site_access_choice() {
        let manifest = parse(
            r#"{"manifest_version":3,"name":"x","version":"1","host_permissions":["*://*/*"],
            "content_scripts":[{"matches":["<all_urls>"],"js":["a.js"]}]}"#,
        );
        let all = vec!["*://*/*".to_owned(), "<all_urls>".to_owned()];
        assert_eq!(site_access(&manifest, &all), SiteAccess::OnAllSites);
        assert_eq!(site_access(&manifest, &[]), SiteAccess::OnClick);
        assert!(matches!(
            site_access(&manifest, &["*://example.com/*".to_owned()]),
            SiteAccess::OnSpecificSites(sites) if sites.len() == 1
        ));
        let none = parse(r#"{"manifest_version":3,"name":"x","version":"1"}"#);
        assert_eq!(site_access(&none, &[]), SiteAccess::OnAllSites);
    }

    #[test]
    fn locales_use_bcp47_and_fall_back_to_english() {
        let environment = locale();
        assert!(!environment.ui_language.contains('_'));
        assert!(!environment.ui_language.contains('.'));
    }

    #[test]
    fn unknown_tabs_and_foreign_urls_are_not_extension_navigations() {
        let side = EngineSide {
            profile: ProfileId::from(1),
            sink: Sink::new(std::sync::Arc::new(|_| {})),
            runtime: RefCell::new(None),
            installs: RefCell::new(HashMap::new()),
            generations: RefCell::new(HashMap::new()),
            tabs: RefCell::new(HashMap::new()),
            requests: RefCell::new(HashMap::new()),
            answers: RefCell::new(HashMap::new()),
            next: Cell::new(0),
            surface: Cell::new(None),
        };
        let extension = "abcdefghijklmnopabcdefghijklmnop";
        let target = format!("chrome-extension://{extension}/options.html");
        assert!(!side.admits(ItemId::from(1), &target));
        side.installs
            .borrow_mut()
            .insert(extension.to_owned(), ExtensionInstallId::from(5));
        side.tabs
            .borrow_mut()
            .insert(ItemId::from(1), TabFacts::default());
        // Without a runtime and without a browser authorization: refused.
        assert!(!side.admits(ItemId::from(1), &target));
        side.tabs
            .borrow_mut()
            .get_mut(&ItemId::from(1))
            .unwrap()
            .authorized = Some(Url::parse(&target).unwrap());
        assert!(side.admits(ItemId::from(1), &target));
        assert!(!side.admits(
            ItemId::from(1),
            &format!("chrome-extension://{extension}/other.html")
        ));
        // The pending authorized load makes the tab its extension's document.
        assert!(matches!(
            side.requester_for(ItemId::from(1)),
            Requester::Tab { extension_install: Some(install), .. } if install == ExtensionInstallId::from(5)
        ));
        side.committed(ItemId::from(1), Url::parse("https://example.com/").ok());
        assert!(matches!(
            side.requester_for(ItemId::from(1)),
            Requester::Tab {
                extension_install: None,
                ..
            }
        ));
        assert!(!side.admits(ItemId::from(1), "https://example.com/"));
    }
}
