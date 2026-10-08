//! The browser side of the WebKit extension runtime: one runtime per durable
//! profile, attached to every tab of that profile from its first view.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use objc2::rc::Retained;
use objc2::MainThreadMarker;
use objc2_app_kit::{NSPopoverBehavior, NSView};
use objc2_foundation::{NSPoint, NSRect, NSRectEdge, NSSize, NSString};
use objc2_web_kit::{WKWebExtensionAction, WKWebView, WKWebViewConfiguration, WKWebsiteDataStore};
use zephium_core::extensions::{
    ExtensionActionRejection, ExtensionActionRequest, ExtensionActionRevision,
    ExtensionActionScope, ExtensionActionSettlement, ExtensionActionSnapshot,
    ExtensionActionSnapshotSettlement, ExtensionActionState, ExtensionBrowserRequest,
    ExtensionBrowserRequestAction, ExtensionBrowserRequestId, ExtensionBrowserRequestResult,
    ExtensionBrowserRequestSettlement, ExtensionBrowserSurface, ExtensionBrowserSurfaceGeneration,
    ExtensionRuntimeGeneration, ExtensionRuntimeInstance,
};
use zephium_core::geometry::Rect;
use zephium_core::ids::{ExtensionInstallId, ItemId, ProfileId, WindowId};
use zephium_core::ports::engine::{EngineEvent, WebExtensionLoad, WebExtensionLoaded};
use zephium_webext_macos::{
    ExtensionSpec, Grants, Host, LogLevel, Runtime, TabRequest, TabRequestDone, TabSnapshot,
    WindowSnapshot,
};

use objc2::runtime::ProtocolObject;
use objc2::{DefinedClass, MainThreadOnly};
use objc2_foundation::{NSObjectProtocol, NSURLRequest, NSURL};
use objc2_web_kit::{WKNavigationDelegate, WKUIDelegate};

use super::permits::{EventPermit, Sink};

#[derive(Default)]
pub(crate) struct WebextHost {
    profiles: HashMap<ProfileId, ProfileRuntime>,
    // Extensions loaded earlier in this session: loading one again (turning it
    // back on, an update) gets no startup event to wake its worker.
    loaded_before: std::collections::HashSet<(ProfileId, String)>,
    pages: HashMap<ItemId, Page>,
}

/// An extension page shown in an extension-owned tab.
struct Page {
    profile: ProfileId,
    view: Retained<WKWebView>,
    stage: Retained<crate::platform::imp::ContentStage>,
    _delegate: Retained<PageDelegate>,
}

pub(crate) enum BrowserRequestOutcome {
    NotOurs,
    Settled,
    /// The shell made an extension-owned tab; the page still has to be put
    /// into it.
    Page {
        extension_id: String,
        url: String,
        done: Option<TabRequestDone>,
    },
}

struct ProfileRuntime {
    runtime: Runtime,
    store: Retained<WKWebsiteDataStore>,
    bridge: Rc<Bridge>,
    installs: HashMap<ExtensionInstallId, Install>,
    surface: Option<ExtensionBrowserSurface>,
}

impl ProfileRuntime {
    /// Chrome closes an extension's own pages when it is turned off or
    /// removed; left open they would also keep its process running.
    fn close_pages(&self, extension_id: &str) {
        for tab in self.runtime.tabs_showing(extension_id) {
            if let Some(item) = self.bridge.item(tab) {
                self.bridge.request(
                    ExtensionBrowserRequestAction::CloseTab { tab: item },
                    Box::new(|_| {}),
                );
            }
        }
    }
}

struct Install {
    extension_id: String,
    generation: ExtensionRuntimeGeneration,
    loaded: bool,
    // Unlike the public revision, this identity cannot be reused when the
    // only installation is removed and reloaded. It gates async auth enablement.
    auth_load: Rc<()>,
}

impl WebextHost {
    /// A fresh configuration for one of the profile's tabs, attached to its
    /// runtime. The runtime is created with the profile's first view, so
    /// every tab can run content scripts.
    pub(crate) fn configuration(
        &mut self,
        profile: ProfileId,
        sink: &Sink,
    ) -> Retained<WKWebViewConfiguration> {
        let mtm = MainThreadMarker::new().expect("engine host runs on the main thread");
        let entry = self.profile(profile, sink);
        let configuration = unsafe { WKWebViewConfiguration::new(mtm) };
        unsafe { configuration.setWebsiteDataStore(&entry.store) };
        entry.runtime.configure(&configuration);
        configuration
    }

    fn profile(&mut self, profile: ProfileId, sink: &Sink) -> &mut ProfileRuntime {
        self.profiles.entry(profile).or_insert_with(|| {
            let mtm = MainThreadMarker::new().expect("engine host runs on the main thread");
            let identifier = profile_uuid(profile);
            let store = unsafe { WKWebsiteDataStore::dataStoreForIdentifier(&identifier, mtm) };
            let bridge = Rc::new(Bridge {
                profile,
                sink: sink.clone(),
                ids: RefCell::new(IdMap::default()),
                pending: RefCell::new(HashMap::new()),
                pending_pages: RefCell::new(HashMap::new()),
                next_request: Cell::new(1),
                popup: RefCell::new(None),
                self_weak: RefCell::new(std::rc::Weak::new()),
                repeats: RefCell::new(HashMap::new()),
                access: RefCell::new(HashMap::new()),
                auth_enabled: Cell::new(true),
                auth_extensions: RefCell::new(std::collections::HashSet::new()),
            });
            *bridge.self_weak.borrow_mut() = Rc::downgrade(&bridge);
            let runtime = Runtime::new(mtm, &store, Some(&identifier), bridge.clone());
            ProfileRuntime {
                runtime,
                store,
                bridge,
                installs: HashMap::new(),
                surface: None,
            }
        })
    }

    pub(crate) fn load(&mut self, profile: ProfileId, load: WebExtensionLoad, sink: &Sink) {
        let reloaded = !self
            .loaded_before
            .insert((profile, load.extension_id.clone()));
        let entry = self.profile(profile, sink);
        entry
            .bridge
            .auth_extensions
            .borrow_mut()
            .remove(&load.extension_id);
        cancel_extension_auth_flows(profile, &load.extension_id);
        if let Some(install) = entry.installs.remove(&load.install) {
            entry
                .bridge
                .auth_extensions
                .borrow_mut()
                .remove(&install.extension_id);
            cancel_extension_auth_flows(profile, &install.extension_id);
            // Their views belong to the context being replaced.
            entry.close_pages(&install.extension_id);
            entry.runtime.unload(&install.extension_id);
        }
        let generation = entry
            .installs
            .values()
            .map(|install| install.generation.get())
            .max()
            .and_then(|generation| ExtensionRuntimeGeneration::new(generation + 1))
            .unwrap_or(ExtensionRuntimeGeneration::new(1).expect("nonzero"));
        let auth_load = Rc::new(());
        entry.installs.insert(
            load.install,
            Install {
                extension_id: load.extension_id.clone(),
                generation,
                loaded: false,
                auth_load: auth_load.clone(),
            },
        );
        let spec = ExtensionSpec {
            id: load.extension_id.clone(),
            root: load.root,
            grants: Grants::Explicit {
                permissions: load.permissions,
                match_patterns: load.match_patterns,
            },
            inspectable: true,
        };
        let install = load.install;
        let start_background = load.start_background || reloaded;
        let sink = sink.clone();
        entry.runtime.load(spec, move |result| {
            let result = result.map(|loaded| {
                let extension_id = loaded.id.clone();
                super::dispatch::best_effort_with(move |host| {
                    if let Some(entry) = host.webext.profiles.get_mut(&profile) {
                        if let Some(install) = entry
                            .installs
                            .get_mut(&install)
                            .filter(|install| Rc::ptr_eq(&install.auth_load, &auth_load))
                        {
                            install.loaded = true;
                            entry
                                .bridge
                                .auth_extensions
                                .borrow_mut()
                                .insert(extension_id.clone());
                            if start_background {
                                entry.runtime.start_background(&extension_id, |_| {});
                            }
                        }
                    }
                });
                WebExtensionLoaded {
                    name: loaded.name,
                    version: loaded.version,
                }
            });
            if let Err(error) = &result {
                eprintln!("extensions: {} failed to load: {error}", install);
            }
            sink.emit(EngineEvent::WebExtensionSettled {
                profile,
                install,
                result,
            });
            sink.emit(EngineEvent::ExtensionActionsInvalidated { profile });
        });
    }

    pub(crate) fn unload(&mut self, profile: ProfileId, install: ExtensionInstallId, sink: &Sink) {
        if let Some(entry) = self.profiles.get_mut(&profile) {
            if let Some(install) = entry.installs.remove(&install) {
                entry
                    .bridge
                    .auth_extensions
                    .borrow_mut()
                    .remove(&install.extension_id);
                cancel_extension_auth_flows(profile, &install.extension_id);
                entry.close_pages(&install.extension_id);
                entry.runtime.unload(&install.extension_id);
                sink.emit(EngineEvent::ExtensionActionsInvalidated { profile });
            }
        }
    }

    /// Removes an extension for good: unloads it and erases what it stored.
    /// Its origin's data can only be cleared from one of its own pages, so a
    /// disabled extension is loaded first, without its background.
    pub(crate) fn remove(&mut self, profile: ProfileId, load: WebExtensionLoad, sink: &Sink) {
        self.loaded_before
            .remove(&(profile, load.extension_id.clone()));
        let entry = self.profile(profile, sink);
        entry
            .bridge
            .auth_extensions
            .borrow_mut()
            .remove(&load.extension_id);
        cancel_extension_auth_flows(profile, &load.extension_id);
        entry.installs.remove(&load.install);
        sink.emit(EngineEvent::ExtensionActionsInvalidated { profile });
        let id = load.extension_id;
        entry.close_pages(&id);
        if entry.runtime.context(&id).is_some() {
            return entry.runtime.erase(&id, || {});
        }
        let spec = ExtensionSpec {
            id: id.clone(),
            root: load.root,
            grants: Grants::Explicit {
                permissions: load.permissions,
                match_patterns: load.match_patterns,
            },
            inspectable: false,
        };
        entry.runtime.load(spec, move |_| {
            super::dispatch::best_effort_with(move |host| {
                if let Some(entry) = host.webext.profiles.get(&profile) {
                    entry.runtime.erase(&id, || {});
                }
            });
        });
    }

    /// Mirrors the shell's windows and tabs into WebKit.
    pub(crate) fn publish(
        &mut self,
        surface: &ExtensionBrowserSurface,
        view_for: impl Fn(ItemId) -> Option<Retained<WKWebView>>,
    ) {
        let Some(entry) = self.profiles.get_mut(&surface.profile()) else {
            return;
        };
        let windows: Vec<WindowSnapshot> = {
            let mut ids = entry.bridge.ids.borrow_mut();
            surface
                .windows()
                .iter()
                .filter(|window| !window.is_private())
                .map(|window| WindowSnapshot {
                    id: ids.window(window.id()),
                    tabs: window
                        .tabs()
                        .iter()
                        .map(|tab| TabSnapshot {
                            id: ids.tab(tab.id()),
                            title: tab.title().to_owned(),
                            url: tab.url().map(str::to_owned),
                            loading: tab.loading(),
                            pinned: tab.pinned(),
                        })
                        .collect(),
                    active: window.active().map(|tab| ids.tab(tab)),
                    frame: (0.0, 0.0, 1200.0, 800.0),
                })
                .collect()
        };
        let focused = surface
            .focused()
            .map(|window| entry.bridge.ids.borrow_mut().window(window));
        entry.runtime.publish(&windows, focused);
        let open: std::collections::HashSet<ItemId> = surface.tabs().map(|tab| tab.id()).collect();
        settle_abandoned_auth_flows(surface.profile(), &open);
        // The engine's own views are the truth for residency; the shell's flag
        // trails view creation and would unbind a view bound on insertion.
        let mut views = Vec::new();
        for tab in surface.tabs() {
            let id = entry.bridge.ids.borrow_mut().tab(tab.id());
            // Extension pages are not engine views; without this, every
            // publish after one opens unbinds it, so its messages arrive
            // without a sender tab and the extension cannot close it.
            let view = view_for(tab.id()).or_else(|| {
                self.pages
                    .get(&tab.id())
                    .filter(|page| page.profile == surface.profile())
                    .map(|page| page.view.clone())
            });
            if zephium_webext_macos::tracing() {
                eprintln!(
                    "webext-trace: publish tab {id} resident={} view={} url={:?}",
                    tab.resident(),
                    view.is_some(),
                    tab.url().map(|u| u.split('?').next().unwrap_or(u))
                );
            }
            views.push((id, view));
        }
        entry
            .runtime
            .bind_views(views.iter().map(|(id, view)| (*id, view.as_deref())));
        entry.surface = Some(surface.clone());
    }

    pub(crate) fn bind_view(&mut self, profile: ProfileId, tab: ItemId, view: Option<&WKWebView>) {
        if view.is_none() {
            settle_auth_flows(
                |flow| flow.profile == profile && flow.tab.get() == Some(tab),
                "The sign-in tab was closed or its native view retired.",
                false,
            );
        }
        if let Some(entry) = self.profiles.get_mut(&profile) {
            let id = entry.bridge.ids.borrow_mut().tab(tab);
            entry.runtime.bind_view(id, view);
        }
    }

    pub(super) fn cancel_profile_auth_flows(&mut self, profile: ProfileId) {
        if let Some(entry) = self.profiles.get(&profile) {
            entry.bridge.auth_enabled.set(false);
        }
        settle_auth_flows(
            |flow| flow.profile == profile,
            "The sign-in profile was closed.",
            false,
        );
    }

    pub(super) fn cancel_auth_flows(&mut self) {
        for entry in self.profiles.values() {
            entry.bridge.auth_enabled.set(false);
        }
        settle_auth_flows(|_| true, "The browser is shutting down.", false);
    }

    pub(crate) fn actions(
        &mut self,
        profile: ProfileId,
        tab: ItemId,
        surface_generation: ExtensionBrowserSurfaceGeneration,
    ) -> ExtensionActionSnapshotSettlement {
        let Some(entry) = self.profiles.get_mut(&profile) else {
            return ExtensionActionSnapshotSettlement::Rejected(
                ExtensionActionRejection::RuntimeUnavailable,
            );
        };
        let tab_id = entry.bridge.ids.borrow_mut().tab(tab);
        let tab_object = entry.runtime.tab_object(tab_id);
        let mut actions = Vec::new();
        for (install_id, install) in &entry.installs {
            if !install.loaded {
                continue;
            }
            let Some(context) = entry.runtime.context(&install.extension_id) else {
                continue;
            };
            let Some(action) = (unsafe { context.actionForTab(tab_object.as_deref()) }) else {
                continue;
            };
            let label = first_line(&unsafe { action.label() }.to_string());
            let badge = first_line(&unsafe { action.badgeText() }.to_string());
            let icon = crate::platform::imp::rasterize_action_icon(&action);
            let revision = presentation_revision(&label, &badge, icon.as_ref().map(|i| i.rgba()));
            let runtime = ExtensionRuntimeInstance::new(profile, *install_id, install.generation);
            let badge = truncate(
                &badge,
                zephium_core::extensions::MAX_EXTENSION_ACTION_BADGE_BYTES,
            );
            // WebKit keeps the unread flag after an extension clears its badge.
            let unread = unsafe { action.hasUnreadBadgeText() } && !badge.is_empty();
            match ExtensionActionState::new(
                runtime,
                ExtensionActionScope::Tab(tab),
                revision,
                truncate(
                    &label,
                    zephium_core::extensions::MAX_EXTENSION_ACTION_LABEL_BYTES,
                ),
                badge,
                icon,
                unsafe { action.isEnabled() },
                unsafe { action.presentsPopup() },
                unread,
            ) {
                Ok(state) => actions.push(state),
                Err(error) => {
                    eprintln!(
                        "extensions: {} action not shown: {error:?}",
                        install.extension_id
                    )
                }
            }
        }
        actions.sort_by_key(|action| action.runtime().install_id());
        match ExtensionActionSnapshot::new(profile, tab, surface_generation, actions) {
            Ok(snapshot) => ExtensionActionSnapshotSettlement::Applied(snapshot),
            Err(_) => ExtensionActionSnapshotSettlement::Rejected(
                ExtensionActionRejection::NativeAdmissionFailed,
            ),
        }
    }

    /// Runs a toolbar action. A popup is presented from WebKit's delegate
    /// callback, anchored to the button the user clicked.
    pub(crate) fn invoke(
        &mut self,
        request: ExtensionActionRequest,
        parent: Option<Retained<NSView>>,
    ) -> ExtensionActionSettlement {
        let profile = request.runtime().profile();
        let Some(entry) = self.profiles.get_mut(&profile) else {
            return ExtensionActionSettlement::Rejected(
                ExtensionActionRejection::RuntimeUnavailable,
            );
        };
        let Some(install) = entry.installs.get(&request.runtime().install_id()) else {
            return ExtensionActionSettlement::Rejected(
                ExtensionActionRejection::RuntimeUnavailable,
            );
        };
        let extension_id = install.extension_id.clone();
        let tab = entry.bridge.ids.borrow_mut().tab(request.tab());
        if entry.runtime.context(&extension_id).is_none() {
            return ExtensionActionSettlement::Rejected(
                ExtensionActionRejection::RuntimeUnavailable,
            );
        }
        *entry.bridge.popup.borrow_mut() = parent.map(|parent| (parent, request.anchor().rect()));
        // WebKit drops what a popup sends while the worker sleeps, and the
        // popup then waits forever; wake it first (immediate when running).
        let waking = extension_id.clone();
        entry.runtime.start_background(&waking, move |_| {
            super::dispatch::best_effort_with(move |host| {
                if let Some(entry) = host.webext.profiles.get(&profile) {
                    // Only a popup consumes the anchor; left behind, it would
                    // retain the button and place a later action.openPopup
                    // on it, perhaps in a window since closed.
                    let tab_object = entry.runtime.tab_object(tab);
                    let popup = entry
                        .runtime
                        .context(&extension_id)
                        .and_then(|context| unsafe { context.actionForTab(tab_object.as_deref()) })
                        .is_some_and(|action| unsafe { action.presentsPopup() });
                    if !popup {
                        entry.bridge.popup.borrow_mut().take();
                    }
                    entry.runtime.perform_action(&extension_id, Some(tab));
                }
            });
        });
        ExtensionActionSettlement::Dispatched
    }

    /// Completes an extension's tab request with the shell's answer. Returns
    /// false when the request is not one of this runtime's.
    pub(crate) fn settle_browser_request(
        &mut self,
        profile: ProfileId,
        request: ExtensionBrowserRequestId,
        settlement: ExtensionBrowserRequestSettlement,
    ) -> BrowserRequestOutcome {
        let Some(entry) = self.profiles.get(&profile) else {
            return BrowserRequestOutcome::NotOurs;
        };
        if let Some(page) = entry.bridge.pending_pages.borrow_mut().remove(&request) {
            return match settlement {
                ExtensionBrowserRequestSettlement::Applied(
                    ExtensionBrowserRequestResult::ExtensionPageAuthorized { .. },
                ) => BrowserRequestOutcome::Page {
                    extension_id: page.extension_id,
                    url: page.url,
                    done: page.done,
                },
                _ => {
                    if let Some(done) = page.done {
                        done(Err("The browser declined to open the page.".into()));
                    }
                    BrowserRequestOutcome::Settled
                }
            };
        }
        let Some(done) = entry.bridge.pending.borrow_mut().remove(&request) else {
            return BrowserRequestOutcome::NotOurs;
        };
        match settlement {
            ExtensionBrowserRequestSettlement::Applied(
                ExtensionBrowserRequestResult::CreatedTab(tab),
            ) => {
                let id = entry.bridge.ids.borrow_mut().tab(tab);
                done(Ok(Some(id)));
            }
            ExtensionBrowserRequestSettlement::Applied(_) => done(Ok(None)),
            ExtensionBrowserRequestSettlement::Rejected(rejection) => done(Err(format!(
                "The browser declined the request ({rejection:?})."
            ))),
        }
        BrowserRequestOutcome::Settled
    }

    /// Shows an extension page in the tab the shell created for it.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn present_page(
        &mut self,
        profile: ProfileId,
        tab: ItemId,
        stage: Retained<crate::platform::imp::ContentStage>,
        permit: Arc<std::sync::atomic::AtomicBool>,
        extension_id: &str,
        url: &str,
        sink: &Sink,
    ) -> Result<(), String> {
        let entry = self
            .profiles
            .get(&profile)
            .ok_or("no runtime for this profile")?;
        let context = entry
            .runtime
            .context(extension_id)
            .ok_or("the extension is not running")?;
        let configuration = unsafe { context.webViewConfiguration() }
            .ok_or("the extension has no page configuration")?;
        let mtm = MainThreadMarker::new().ok_or("main thread required")?;
        let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(960.0, 720.0));
        let view = unsafe {
            WKWebView::initWithFrame_configuration(WKWebView::alloc(mtm), frame, &configuration)
        };
        let origin = format!("chrome-extension://{extension_id}/");
        let delegate = PageDelegate::new(
            mtm,
            profile,
            tab,
            origin,
            sink.clone(),
            Rc::downgrade(&entry.bridge),
        );
        unsafe {
            view.setNavigationDelegate(Some(ProtocolObject::from_ref(&*delegate)));
            view.setUIDelegate(Some(ProtocolObject::from_ref(&*delegate)));
            view.setInspectable(true);
        }
        let url = NSURL::URLWithString(&NSString::from_str(url)).ok_or("invalid page address")?;
        if !stage.insert_view(tab, Retained::into_super(view.clone()), permit) {
            return Err("the tab could not take the page".into());
        }
        unsafe { view.loadRequest(&NSURLRequest::requestWithURL(&url)) };
        let _ = stage.set_ready(tab);
        let id = entry.bridge.ids.borrow_mut().tab(tab);
        entry.runtime.bind_view(id, Some(&view));
        self.pages.insert(
            tab,
            Page {
                profile,
                view,
                stage,
                _delegate: delegate,
            },
        );
        Ok(())
    }

    pub(crate) fn tab_number(&self, profile: ProfileId, tab: ItemId) -> u64 {
        self.profiles
            .get(&profile)
            .map_or(0, |entry| entry.bridge.ids.borrow_mut().tab(tab))
    }

    /// Closes an extension page; false when `tab` shows none.
    pub(crate) fn close_page(&mut self, tab: ItemId) -> bool {
        let Some(page) = self.pages.remove(&tab) else {
            return false;
        };
        unsafe {
            page.view.stopLoading();
            page.view.setNavigationDelegate(None);
            page.view.setUIDelegate(None);
        }
        page.stage.remove_view(tab);
        if let Some(entry) = self.profiles.get(&page.profile) {
            let id = entry.bridge.ids.borrow_mut().tab(tab);
            entry.runtime.bind_view(id, None);
        }
        true
    }

    /// Reload (0), back (1), forward (2) or stop (3) on an extension page.
    pub(crate) fn navigate_page(&self, tab: ItemId, action: u8) -> bool {
        let Some(page) = self.pages.get(&tab) else {
            return false;
        };
        unsafe {
            match action {
                0 => drop(page.view.reload()),
                1 => drop(page.view.goBack()),
                2 => drop(page.view.goForward()),
                _ => page.view.stopLoading(),
            }
        }
        true
    }
}

fn presentation_revision(label: &str, badge: &str, icon: Option<&[u8]>) -> ExtensionActionRevision {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    (label, badge, icon).hash(&mut hasher);
    ExtensionActionRevision::new(hasher.finish().max(1)).expect("nonzero")
}

/// Chrome shows a multi-line title as a tooltip; a button's name is its first
/// line (Google Translate's title goes on to explain its clicks).
fn first_line(text: &str) -> String {
    let line = text
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or_default();
    line.chars().filter(|c| !c.is_control()).collect()
}

fn truncate(text: &str, max: usize) -> &str {
    let mut end = text.len().min(max);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

fn profile_uuid(profile: ProfileId) -> Retained<objc2_foundation::NSUUID> {
    let bytes = profile.bytes();
    let text = format!(
        "{:02X}{:02X}{:02X}{:02X}-{:02X}{:02X}-{:02X}{:02X}-{:02X}{:02X}-{:02X}{:02X}{:02X}{:02X}{:02X}{:02X}",
        bytes[0],
        bytes[1],
        bytes[2],
        bytes[3],
        bytes[4],
        bytes[5],
        bytes[6],
        bytes[7],
        bytes[8],
        bytes[9],
        bytes[10],
        bytes[11],
        bytes[12],
        bytes[13],
        bytes[14],
        bytes[15]
    );
    objc2_foundation::NSUUID::from_string(&NSString::from_str(&text)).expect("formatted UUID")
}

/// Stable runtime-local numbers for the shell's tab and window identities.
#[derive(Default)]
struct IdMap {
    tabs: HashMap<ItemId, u64>,
    tabs_back: HashMap<u64, ItemId>,
    windows: HashMap<WindowId, u64>,
    windows_back: HashMap<u64, WindowId>,
    next: u64,
}

impl IdMap {
    fn tab(&mut self, id: ItemId) -> u64 {
        if let Some(number) = self.tabs.get(&id) {
            return *number;
        }
        self.next += 1;
        self.tabs.insert(id, self.next);
        self.tabs_back.insert(self.next, id);
        self.next
    }

    fn window(&mut self, id: WindowId) -> u64 {
        if let Some(number) = self.windows.get(&id) {
            return *number;
        }
        self.next += 1;
        self.windows.insert(id, self.next);
        self.windows_back.insert(self.next, id);
        self.next
    }
}

struct Bridge {
    profile: ProfileId,
    sink: Sink,
    ids: RefCell<IdMap>,
    pending: RefCell<HashMap<ExtensionBrowserRequestId, TabRequestDone>>,
    pending_pages: RefCell<HashMap<ExtensionBrowserRequestId, PendingPage>>,
    next_request: Cell<u64>,
    popup: RefCell<Option<(Retained<NSView>, Rect)>>,
    self_weak: RefCell<std::rc::Weak<Bridge>>,
    repeats: RefCell<HashMap<u64, (std::time::Instant, u32)>>,
    access: RefCell<HashMap<u64, AccessAnswer>>,
    auth_enabled: Cell<bool>,
    auth_extensions: RefCell<std::collections::HashSet<String>>,
}

type AccessAnswer = Box<dyn FnOnce(bool)>;

impl Bridge {
    fn request(&self, action: ExtensionBrowserRequestAction, done: TabRequestDone) {
        let number = self.next_request.get();
        self.next_request.set(number + 1);
        // The high bit keeps these apart from the previous runtime's ids.
        let Some(id) = ExtensionBrowserRequestId::new(number | (1 << 63)) else {
            return done(Err("request identity exhausted".into()));
        };
        match ExtensionBrowserRequest::new(self.profile, id, action) {
            Ok(request) => {
                self.pending.borrow_mut().insert(id, done);
                self.sink
                    .emit(EngineEvent::ExtensionBrowserRequested { request });
            }
            Err(_) => done(Err("That address cannot be opened.".into())),
        }
    }

    fn item(&self, tab: u64) -> Option<ItemId> {
        self.ids.borrow().tabs_back.get(&tab).copied()
    }

    /// Asks the shell for an extension-owned tab showing one of an
    /// extension's own pages; WebKit serves those pages only to views built
    /// from that extension's configuration.
    fn open_page(&self, extension_id: String, url: String, done: Option<TabRequestDone>) {
        let number = self.next_request.get();
        self.next_request.set(number + 1);
        let Some(id) = ExtensionBrowserRequestId::new(number | (1 << 63)) else {
            if let Some(done) = done {
                done(Err("request identity exhausted".into()));
            }
            return;
        };
        match ExtensionBrowserRequest::new(
            self.profile,
            id,
            ExtensionBrowserRequestAction::OpenExtensionPage,
        ) {
            Ok(request) => {
                self.pending_pages.borrow_mut().insert(
                    id,
                    PendingPage {
                        extension_id,
                        url,
                        done,
                    },
                );
                self.sink
                    .emit(EngineEvent::ExtensionBrowserRequested { request });
            }
            Err(_) => {
                if let Some(done) = done {
                    done(Err("The page cannot be opened.".into()));
                }
            }
        }
    }
}

struct PendingPage {
    extension_id: String,
    url: String,
    done: Option<TabRequestDone>,
}

const MAX_PENDING_AUTH_FLOWS: usize = 16;
const AUTH_FLOW_TIMEOUT: Duration = Duration::from_secs(10 * 60);

/// Owns one extension callback and the exact native generation of its login
/// tab. A retired generation is never rebound to a replacement view.
struct AuthFlow {
    id: u64,
    profile: ProfileId,
    extension: String,
    initial_url: Arc<str>,
    cleanup: zephium_core::extensions::AuthTabCleanupPermit,
    tab: Rc<Cell<Option<ItemId>>>,
    generation: Option<std::sync::Weak<AtomicBool>>,
    bridge: std::rc::Weak<Bridge>,
    deadline: Instant,
    watchdog: Option<crate::platform::imp::ContentPolicyTimeout>,
    done: Box<dyn FnOnce(Result<String, String>)>,
}

impl AuthFlow {
    fn finish(self, result: Result<String, String>, close_tab: bool) {
        // Removal and timer cancellation precede arbitrary extension callback
        // code. Reentrant sign-in cannot observe or complete this flow again.
        drop(self.watchdog);
        let completed = result.is_ok();
        (self.done)(result);
        let Some(token) = self.generation.and_then(|generation| generation.upgrade()) else {
            return;
        };
        if !close_tab || !token.load(Ordering::Acquire) {
            return;
        }
        let (Some(tab), Some(bridge)) = (self.tab.get(), self.bridge.upgrade()) else {
            return;
        };
        let profile = self.profile;
        let initial_url = self.initial_url;
        let cleanup = self.cleanup;
        // Read the live view after the callback, which may synchronously
        // navigate or retire it. The Shell then rechecks the exact document
        // when this queued close reaches its authoritative mutation turn.
        let _ = super::dispatch::try_with(move |host| {
            let Some(view) = host.views.get(&tab) else {
                return;
            };
            if !view.event_permit.matches_token(&token)
                || !view.navigation.auth_cleanup_is_owned()
                || host
                    .partitions
                    .get(&tab)
                    .is_none_or(|partition| partition.profile() != profile)
            {
                return;
            }
            let snapshot = if completed {
                view.navigation.resident_document_snapshot()
            } else {
                view.navigation.committed_snapshot()
            };
            let Some((epoch, url)) = snapshot else {
                return;
            };
            if !auth_cleanup_matches(&initial_url, &url, completed)
                || crate::platform::imp::current_url(&view.view).as_deref() != Some(url.as_str())
                || !view.event_permit.matches_token(&token)
                || !view.navigation.auth_cleanup_is_owned()
                || view.navigation.resident_document_snapshot() != Some((epoch, url.clone()))
            {
                return;
            }
            bridge.request(
                ExtensionBrowserRequestAction::CloseTabIfUnchanged {
                    tab,
                    navigation: epoch.presentation_id(),
                    url: Arc::from(url),
                    cleanup,
                },
                Box::new(|_| {}),
            );
        });
    }
}

fn auth_cleanup_matches(initial: &str, current: &str, completed: bool) -> bool {
    let (Ok(initial), Ok(current)) = (url::Url::parse(initial), url::Url::parse(current)) else {
        return false;
    };
    zephium_core::navigation::is_browser_target(&current)
        && current.as_str() != "about:blank"
        && (completed || initial.origin() == current.origin())
}

/// A native link/history/reload may be the person repurposing the sign-in
/// tab, even on the same provider origin. This only relinquishes cleanup;
/// legitimate OAuth completion remains independently authenticated below.
pub(super) fn note_auth_native_navigation(
    navigation: &crate::navigation_epoch::NavigationEpochTracker,
    action: wry::AppleNavigationAction,
) {
    if action.target_is_main_frame == Some(true)
        && matches!(
            action.navigation_type,
            wry::AppleNavigationType::LinkActivated
                | wry::AppleNavigationType::BackForward
                | wry::AppleNavigationType::Reload
        )
    {
        navigation.release_auth_cleanup();
    }
}

pub(super) fn bind_auth_cleanup_fence(
    profile: ProfileId,
    tab: ItemId,
    permit: &EventPermit,
    navigation: &crate::navigation_epoch::NavigationEpochTracker,
) {
    let Some(token) = permit.active_token() else {
        return;
    };
    let fence = AUTH_FLOWS.with(|flows| {
        flows
            .borrow()
            .pending
            .iter()
            .find(|flow| {
                flow.profile == profile
                    && flow.tab.get() == Some(tab)
                    && flow.generation.as_ref().is_some_and(|generation| {
                        generation.upgrade().is_some_and(|bound| {
                            Arc::ptr_eq(&bound, &token) && bound.load(Ordering::Acquire)
                        })
                    })
            })
            .map(|flow| flow.cleanup.clone())
    });
    if let Some(fence) = fence {
        navigation.attach_auth_cleanup_fence(fence);
    }
}

#[derive(Default)]
struct AuthFlowRegistry {
    next_id: u64,
    pending: Vec<AuthFlow>,
}

impl AuthFlowRegistry {
    fn reserve(&mut self, profile: ProfileId, extension: &str) -> Option<u64> {
        if self.pending.len() >= MAX_PENDING_AUTH_FLOWS
            && !self
                .pending
                .iter()
                .any(|flow| flow.profile == profile && flow.extension == extension)
        {
            return None;
        }
        self.next_id = self.next_id.checked_add(1)?;
        Some(self.next_id)
    }

    fn insert(&mut self, flow: AuthFlow) -> Option<AuthFlow> {
        let previous = self.pending.iter().position(|previous| {
            previous.profile == flow.profile && previous.extension == flow.extension
        });
        let previous = previous.map(|index| self.pending.remove(index));
        self.pending.push(flow);
        previous
    }

    fn bind_created_tab(&self, id: u64, tab: ItemId) -> bool {
        let Some(flow) = self.pending.iter().find(|flow| flow.id == id) else {
            return false;
        };
        // A native tabs.create terminal is at most once. Reject duplicates
        // rather than changing the identity that was bound at construction.
        if flow.tab.get().is_some() || Instant::now() >= flow.deadline {
            return false;
        }
        flow.tab.set(Some(tab));
        true
    }

    fn take_matching(&mut self, matches: impl Fn(&AuthFlow) -> bool) -> Vec<AuthFlow> {
        let (taken, kept) = std::mem::take(&mut self.pending)
            .into_iter()
            .partition(matches);
        self.pending = kept;
        taken
    }

    fn take_redirect(
        &mut self,
        profile: ProfileId,
        tab: ItemId,
        token: &Arc<AtomicBool>,
        url: &str,
    ) -> Option<AuthFlow> {
        let url = url::Url::parse(url).ok()?;
        if !zephium_core::navigation::is_allowed(&url) || url.scheme() != "https" {
            return None;
        }
        let index = self.pending.iter().position(|flow| {
            flow.profile == profile
                && flow.tab.get() == Some(tab)
                && url.host_str() == Some(format!("{}.chromiumapp.org", flow.extension).as_str())
                && url.port_or_known_default() == Some(443)
                && flow.generation.as_ref().is_some_and(|generation| {
                    generation.upgrade().is_some_and(|bound| {
                        Arc::ptr_eq(&bound, token) && bound.load(Ordering::Acquire)
                    })
                })
        })?;
        Some(self.pending.remove(index))
    }
}

thread_local! {
    static AUTH_FLOWS: RefCell<AuthFlowRegistry> = RefCell::new(AuthFlowRegistry::default());
}

/// The Shell replies to tabs.create before first navigation. Bind only at
/// native construction/adoption, never from an untrusted navigation callback.
pub(super) fn bind_auth_flow_view(
    profile: ProfileId,
    tab: ItemId,
    token: &Arc<AtomicBool>,
    initial_url: &str,
) {
    let retired = AUTH_FLOWS.with(|flows| {
        let mut flows = flows.borrow_mut();
        for flow in &mut flows.pending {
            if flow.profile == profile
                && flow.tab.get() == Some(tab)
                && flow.generation.is_none()
                && url::Url::parse(initial_url)
                    .is_ok_and(|url| url.as_str() == flow.initial_url.as_ref())
            {
                flow.generation = Some(Arc::downgrade(token));
            }
        }
        flows.take_matching(|flow| {
            flow.profile == profile
                && flow.tab.get() == Some(tab)
                && !flow.generation.as_ref().is_some_and(|generation| {
                    generation.upgrade().is_some_and(|bound| {
                        Arc::ptr_eq(&bound, token) && bound.load(Ordering::Acquire)
                    })
                })
        })
    });
    for flow in retired {
        flow.finish(
            Err("The sign-in tab's native view was replaced.".into()),
            false,
        );
    }
}

/// Only the current auth tab's main-frame native policy callback can complete
/// a flow. Subframes, other profiles/tabs and retired view callbacks leave it
/// untouched; URL equality never grants authority.
pub(super) fn intercept_auth_redirect(
    profile: ProfileId,
    tab: ItemId,
    permit: &EventPermit,
    navigation: &crate::navigation_epoch::NavigationEpochTracker,
    main_frame: Option<bool>,
    url: &str,
) -> bool {
    if main_frame != Some(true)
        || !permit.allows_navigation(url)
        || !navigation.admits_target(url)
        || navigation.activity_snapshot().is_none()
    {
        return false;
    }
    let Some(token) = permit.active_token() else {
        return false;
    };
    let flow = AUTH_FLOWS.with(|flows| flows.borrow_mut().take_redirect(profile, tab, &token, url));
    let Some(flow) = flow else {
        return false;
    };
    let result = if Instant::now() >= flow.deadline {
        Err("The sign-in flow timed out.".into())
    } else {
        Ok(url.to_owned())
    };
    flow.finish(result, true);
    true
}

fn settle_auth_flows(matches: impl Fn(&AuthFlow) -> bool, message: &str, close_tab: bool) {
    let flows = AUTH_FLOWS.with(|flows| flows.borrow_mut().take_matching(matches));
    for flow in flows {
        flow.finish(Err(message.to_owned()), close_tab);
    }
}

fn cancel_extension_auth_flows(profile: ProfileId, extension: &str) {
    settle_auth_flows(
        |flow| flow.profile == profile && flow.extension == extension,
        "The extension's sign-in runtime was retired.",
        true,
    );
}

fn settle_abandoned_auth_flows(profile: ProfileId, open: &std::collections::HashSet<ItemId>) {
    settle_auth_flows(
        |flow| flow.profile == profile && flow.tab.get().is_some_and(|tab| !open.contains(&tab)),
        "The user did not approve access.",
        false,
    );
}

/// The extension a `chrome-extension://<id>/…` address belongs to.
fn extension_of(url: &str) -> Option<&str> {
    let rest = url.strip_prefix(concat!("chrome-extension", "://"))?;
    let id = rest.split('/').next()?;
    (id.len() == 32 && id.bytes().all(|b| (b'a'..=b'p').contains(&b))).then_some(id)
}

impl Bridge {
    fn answer_access(&self, request: u64, allowed: bool) {
        let done = self.access.borrow_mut().remove(&request);
        if let Some(done) = done {
            done(allowed);
        }
    }
}

impl Host for Bridge {
    fn prompt_access(
        &self,
        request: zephium_webext_macos::AccessRequest,
        done: Box<dyn FnOnce(bool)>,
    ) {
        let number = self.next_request.get();
        self.next_request.set(number.wrapping_add(1));
        self.access.borrow_mut().insert(number, done);
        self.sink
            .emit(EngineEvent::WebExtensionAccessRequested(Box::new(
                zephium_core::ports::engine::WebExtensionAccessRequest {
                    profile: self.profile,
                    request: number,
                    extension_id: request.extension,
                    warnings: request.warnings,
                    permissions: request.permissions,
                    patterns: request.patterns,
                },
            )));
    }

    fn log(&self, extension: &str, level: LogLevel, message: &str) {
        // Extensions retry failing calls in loops; print each distinct line
        // once a minute with how often it repeated.
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        (extension, message).hash(&mut hasher);
        let now = std::time::Instant::now();
        let repeated = {
            let mut repeats = self.repeats.borrow_mut();
            if repeats.len() > 512 {
                repeats.retain(|_, (since, _)| now.duration_since(*since).as_secs() < 60);
            }
            let entry = repeats.entry(hasher.finish()).or_insert((now, 0));
            if entry.1 > 0 && now.duration_since(entry.0).as_secs() < 60 {
                entry.1 += 1;
                return;
            }
            let repeated = entry.1.saturating_sub(1);
            *entry = (now, 1);
            repeated
        };
        let suffix = if repeated > 0 {
            format!(" (repeated {repeated} more times)")
        } else {
            String::new()
        };
        let tag = match level {
            LogLevel::Info => "info",
            LogLevel::Warning => "warning",
            LogLevel::Error => "error",
        };
        // What extensions log is often page addresses, page text or tokens.
        // The local log a person may attach to a report keeps only its size.
        if cfg!(debug_assertions) {
            eprintln!("extension {extension} {tag}: {message}{suffix}");
        } else {
            let length = message.chars().count();
            eprintln!("extension {extension} {tag}: {length} characters{suffix}");
        }
    }

    fn tab_request(&self, request: TabRequest, done: TabRequestDone) {
        let tab = |id: u64| self.item(id);
        match &request {
            TabRequest::Create { url: Some(url), .. } => {
                if let Some(extension) = extension_of(url) {
                    return self.open_page(extension.to_owned(), url.clone(), Some(done));
                }
            }
            TabRequest::Load { tab: id, url } => {
                if let (Some(extension), Some(item)) = (extension_of(url), tab(*id)) {
                    // A website sending its tab to an extension page (1Password's
                    // sign-in) gets the page in an extension-owned tab instead.
                    let extension = extension.to_owned();
                    let url = url.clone();
                    self.request(
                        ExtensionBrowserRequestAction::CloseTab { tab: item },
                        Box::new(|_| {}),
                    );
                    return self.open_page(extension, url, Some(done));
                }
            }
            _ => {}
        }
        let action = match request {
            TabRequest::Create {
                window,
                url,
                active,
                ..
            } => ExtensionBrowserRequestAction::CreateTab {
                window: window.and_then(|w| self.ids.borrow().windows_back.get(&w).copied()),
                url: url.filter(|url| url != "about:blank").map(Arc::from),
                active,
            },
            TabRequest::Activate { tab: id } => match tab(id) {
                Some(tab) => ExtensionBrowserRequestAction::ActivateTab { tab },
                None => return done(Err("No tab with that id.".into())),
            },
            TabRequest::Close { tab: id } => match tab(id) {
                Some(tab) => ExtensionBrowserRequestAction::CloseTab { tab },
                None => return done(Err("No tab with that id.".into())),
            },
            TabRequest::Load { tab: id, url } => match tab(id) {
                Some(tab) => ExtensionBrowserRequestAction::LoadTabUrl {
                    tab,
                    url: Arc::from(url),
                },
                None => return done(Err("No tab with that id.".into())),
            },
            TabRequest::Reload { tab: id, .. } => match tab(id) {
                Some(tab) => ExtensionBrowserRequestAction::ReloadTab { tab },
                None => return done(Err("No tab with that id.".into())),
            },
            TabRequest::Back { tab: id } => match tab(id) {
                Some(tab) => ExtensionBrowserRequestAction::GoBack { tab },
                None => return done(Err("No tab with that id.".into())),
            },
            TabRequest::Forward { tab: id } => match tab(id) {
                Some(tab) => ExtensionBrowserRequestAction::GoForward { tab },
                None => return done(Err("No tab with that id.".into())),
            },
            TabRequest::Pin { .. } | TabRequest::FocusWindow { .. } => {
                return done(Ok(None));
            }
        };
        self.request(action, done);
    }

    fn start_auth_flow(
        &self,
        extension: &str,
        url: &str,
        done: Box<dyn FnOnce(Result<String, String>)>,
    ) {
        if !self.auth_enabled.get() || !self.auth_extensions.borrow().contains(extension) {
            return done(Err("The sign-in runtime is no longer available.".into()));
        }
        if !zephium_core::navigation::is_allowed_str(url)
            || url.len() > zephium_core::extensions::MAX_EXTENSION_BROWSER_REQUEST_URL_BYTES
            || url == "about:blank"
            || extension_of(&format!("chrome-extension://{extension}/")).is_none()
        {
            return done(Err("The authorization page must be a web address.".into()));
        }
        let Ok(initial_url) = url::Url::parse(url) else {
            return done(Err("The authorization page must be a web address.".into()));
        };
        let Some(id) = AUTH_FLOWS.with(|flows| flows.borrow_mut().reserve(self.profile, extension))
        else {
            return done(Err("Too many sign-in flows are pending.".into()));
        };
        let Some(watchdog) =
            crate::platform::imp::schedule_content_policy_timeout(AUTH_FLOW_TIMEOUT, move || {
                settle_auth_flows(|flow| flow.id == id, "The sign-in flow timed out.", true);
            })
        else {
            return done(Err("The sign-in timeout could not be installed.".into()));
        };
        let tab = Rc::new(Cell::new(None));
        let replaced = AUTH_FLOWS.with(|flows| {
            flows.borrow_mut().insert(AuthFlow {
                id,
                profile: self.profile,
                extension: extension.to_owned(),
                initial_url: Arc::from(initial_url.as_str()),
                cleanup: zephium_core::extensions::AuthTabCleanupPermit::default(),
                tab,
                generation: None,
                bridge: self.self_weak.borrow().clone(),
                deadline: Instant::now() + AUTH_FLOW_TIMEOUT,
                watchdog: Some(watchdog),
                done,
            })
        });
        if let Some(previous) = replaced {
            previous.finish(Err("Another sign-in started.".into()), true);
        }
        // A replaced callback can synchronously start a newer flow. Do not
        // create an orphan tab for this now-cancelled identity.
        if !AUTH_FLOWS.with(|flows| flows.borrow().pending.iter().any(|flow| flow.id == id)) {
            return;
        }
        let ids = self.self_weak.borrow().clone();
        self.request(
            ExtensionBrowserRequestAction::CreateTab {
                window: None,
                url: Some(Arc::from(url)),
                active: true,
            },
            Box::new(move |result| {
                let bridge = ids.upgrade();
                let item = match (result, bridge.as_ref()) {
                    (Ok(Some(number)), Some(bridge)) => bridge.item(number),
                    _ => None,
                };
                if let Some(item) = item {
                    let bound = AUTH_FLOWS.with(|flows| flows.borrow().bind_created_tab(id, item));
                    if !bound {
                        settle_auth_flows(
                            |flow| flow.id == id,
                            "The sign-in page could not be opened.",
                            false,
                        );
                        // Cancellation can precede an accepted tabs.create
                        // reply. Close that exact newly-created logical tab;
                        // never resurrect the expired callback or generation.
                        if let Some(bridge) = bridge {
                            bridge.request(
                                ExtensionBrowserRequestAction::CloseTabIfPristine { tab: item },
                                Box::new(|_| {}),
                            );
                        }
                    }
                } else {
                    settle_auth_flows(
                        |flow| flow.id == id,
                        "The sign-in page could not be opened.",
                        false,
                    );
                }
            }),
        );
    }

    fn open_options(&self, extension: &str, url: &str) {
        self.open_page(extension.to_owned(), url.to_owned(), None);
    }

    fn present_popup(&self, _extension: &str, action: &WKWebExtensionAction) -> bool {
        let Some((parent, anchor)) = self.popup.borrow_mut().take() else {
            return false;
        };
        let Some(popover) = (unsafe { action.popupPopover() }) else {
            return false;
        };
        let bounds = parent.bounds();
        let flipped = parent.isFlipped();
        let y = if flipped {
            anchor.y
        } else {
            bounds.size.height - anchor.y - anchor.height
        };
        let rect = NSRect::new(
            NSPoint::new(anchor.x, y),
            NSSize::new(anchor.width.max(1.0), anchor.height.max(1.0)),
        );
        let edge = if flipped {
            NSRectEdge::MaxY
        } else {
            NSRectEdge::MinY
        };
        popover.setBehavior(NSPopoverBehavior::Transient);
        popover.showRelativeToRect_ofView_preferredEdge(rect, &parent, edge);
        popover.isShown()
    }
}

impl super::EngineHost {
    pub(crate) fn load_web_extension(&mut self, profile: ProfileId, load: WebExtensionLoad) {
        let sink = self.sink.clone();
        self.webext.load(profile, load, &sink);
    }

    pub(crate) fn unload_web_extension(&mut self, profile: ProfileId, install: ExtensionInstallId) {
        let sink = self.sink.clone();
        self.webext.unload(profile, install, &sink);
    }

    pub(crate) fn open_web_extension_options(&mut self, profile: ProfileId, extension_id: &str) {
        if let Some(entry) = self.webext.profiles.get(&profile) {
            entry.runtime.open_options(extension_id);
        }
    }

    pub(crate) fn answer_web_extension_access(
        &mut self,
        profile: ProfileId,
        request: u64,
        allowed: bool,
    ) {
        if let Some(entry) = self.webext.profiles.get(&profile) {
            entry.bridge.answer_access(request, allowed);
        }
    }

    pub(crate) fn remove_web_extension(&mut self, profile: ProfileId, load: WebExtensionLoad) {
        let sink = self.sink.clone();
        self.webext.remove(profile, load, &sink);
    }
}

// objc2 lays out ivars only up to 8-byte alignment; the 16-byte-aligned
// identities stay behind pointers.
struct PageIvars {
    profile: Box<ProfileId>,
    tab: Box<ItemId>,
    origin: String,
    sink: Sink,
    bridge: std::rc::Weak<Bridge>,
}

objc2::define_class!(
    #[unsafe(super(objc2::runtime::NSObject))]
    #[thread_kind = objc2::MainThreadOnly]
    #[name = "ZephiumWebExtPageDelegate"]
    #[ivars = PageIvars]
    struct PageDelegate;

    unsafe impl NSObjectProtocol for PageDelegate {}

    unsafe impl WKNavigationDelegate for PageDelegate {
        #[unsafe(method(webView:decidePolicyForNavigationAction:decisionHandler:))]
        fn decide_policy(
            &self,
            _view: &WKWebView,
            action: &objc2_web_kit::WKNavigationAction,
            decision: &block2::DynBlock<dyn Fn(objc2_web_kit::WKNavigationActionPolicy)>,
        ) {
            use objc2_web_kit::WKNavigationActionPolicy as Policy;
            let url = unsafe { action.request() }
                .URL()
                .and_then(|url| url.absoluteString())
                .map(|url| url.to_string())
                .unwrap_or_default();
            let main_frame =
                unsafe { action.targetFrame() }.is_some_and(|frame| unsafe { frame.isMainFrame() });
            // An extension-owned tab only shows its extension's pages; leaving
            // for the web continues in a normal tab.
            if main_frame && !url.starts_with(&self.ivars().origin) && url.starts_with("http") {
                decision.call((Policy::Cancel,));
                if let Some(bridge) = self.ivars().bridge.upgrade() {
                    bridge.request(
                        ExtensionBrowserRequestAction::CreateTab {
                            window: None,
                            url: Some(Arc::from(url.as_str())),
                            active: true,
                        },
                        Box::new(|_| {}),
                    );
                }
                return;
            }
            decision.call((Policy::Allow,));
        }

        #[unsafe(method(webView:didCommitNavigation:))]
        fn did_commit(&self, view: &WKWebView, _navigation: Option<&objc2_web_kit::WKNavigation>) {
            self.changed(view);
        }

        #[unsafe(method(webView:didFinishNavigation:))]
        fn did_finish(&self, view: &WKWebView, _navigation: Option<&objc2_web_kit::WKNavigation>) {
            self.changed(view);
        }

        #[unsafe(method(webView:didFailNavigation:withError:))]
        fn did_fail(
            &self,
            view: &WKWebView,
            _navigation: Option<&objc2_web_kit::WKNavigation>,
            _error: &objc2_foundation::NSError,
        ) {
            self.changed(view);
        }
    }

    unsafe impl WKUIDelegate for PageDelegate {
        // WebKit's default for an omitted method is its own prompt; extension
        // pages never receive camera, microphone or motion access.
        #[unsafe(method(webView:requestMediaCapturePermissionForOrigin:initiatedByFrame:type:decisionHandler:))]
        fn deny_media_capture(
            &self,
            _view: &WKWebView,
            _origin: &objc2_web_kit::WKSecurityOrigin,
            _frame: &objc2_web_kit::WKFrameInfo,
            _capture_type: objc2_web_kit::WKMediaCaptureType,
            decision: &block2::DynBlock<dyn Fn(objc2_web_kit::WKPermissionDecision)>,
        ) {
            decision.call((objc2_web_kit::WKPermissionDecision::Deny,));
        }

        #[unsafe(method(webView:requestDeviceOrientationAndMotionPermissionForOrigin:initiatedByFrame:decisionHandler:))]
        fn deny_device_motion(
            &self,
            _view: &WKWebView,
            _origin: &objc2_web_kit::WKSecurityOrigin,
            _frame: &objc2_web_kit::WKFrameInfo,
            decision: &block2::DynBlock<dyn Fn(objc2_web_kit::WKPermissionDecision)>,
        ) {
            decision.call((objc2_web_kit::WKPermissionDecision::Deny,));
        }
        #[unsafe(method(webViewDidClose:))]
        fn did_close(&self, _view: &WKWebView) {
            let ivars = self.ivars();
            ivars.sink.emit(EngineEvent::ExtensionPageClosed {
                profile: *ivars.profile,
                id: *ivars.tab,
            });
        }

        #[unsafe(method_id(webView:createWebViewWithConfiguration:forNavigationAction:windowFeatures:))]
        fn create_view(
            &self,
            _view: &WKWebView,
            _configuration: &WKWebViewConfiguration,
            action: &objc2_web_kit::WKNavigationAction,
            _features: &objc2_web_kit::WKWindowFeatures,
        ) -> Option<Retained<WKWebView>> {
            let url: Option<String> = unsafe { action.request() }
                .URL()
                .and_then(|url| url.absoluteString())
                .map(|url| url.to_string());
            if let (Some(url), Some(bridge)) = (url, self.ivars().bridge.upgrade()) {
                match extension_of(&url) {
                    Some(extension) => bridge.open_page(extension.to_owned(), url, None),
                    None => bridge.request(
                        ExtensionBrowserRequestAction::CreateTab {
                            window: None,
                            url: Some(Arc::from(url.as_str())),
                            active: true,
                        },
                        Box::new(|_| {}),
                    ),
                }
            }
            None
        }
    }
);

impl PageDelegate {
    fn new(
        mtm: MainThreadMarker,
        profile: ProfileId,
        tab: ItemId,
        origin: String,
        sink: Sink,
        bridge: std::rc::Weak<Bridge>,
    ) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(PageIvars {
            profile: Box::new(profile),
            tab: Box::new(tab),
            origin,
            sink,
            bridge,
        });
        unsafe { objc2::msg_send![super(this), init] }
    }

    fn changed(&self, view: &WKWebView) {
        let ivars = self.ivars();
        let title = unsafe { view.title() }
            .map(|title| title.to_string())
            .unwrap_or_default();
        ivars.sink.emit(EngineEvent::ExtensionPageChanged {
            profile: *ivars.profile,
            id: *ivars.tab,
            title: zephium_core::item::sanitize_page_title(&title),
            loading: unsafe { view.isLoading() },
            can_go_back: unsafe { view.canGoBack() },
            can_go_forward: unsafe { view.canGoForward() },
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const AUTH_EXTENSION: &str = "abcdefghijklmnopabcdefghijklmnop";
    const AUTH_START: &str = "https://accounts.example/login";
    const AUTH_REDIRECT: &str =
        "https://abcdefghijklmnopabcdefghijklmnop.chromiumapp.org/cb?code=synthetic&state=fixture";
    type AuthResults = Rc<RefCell<Vec<Result<String, String>>>>;

    fn fixture_flow(id: u64, profile: ProfileId, tab: ItemId, results: &AuthResults) -> AuthFlow {
        let results = results.clone();
        AuthFlow {
            id,
            profile,
            extension: AUTH_EXTENSION.to_owned(),
            initial_url: Arc::from(AUTH_START),
            cleanup: zephium_core::extensions::AuthTabCleanupPermit::default(),
            tab: Rc::new(Cell::new(Some(tab))),
            generation: None,
            bridge: std::rc::Weak::new(),
            deadline: Instant::now() + AUTH_FLOW_TIMEOUT,
            watchdog: None,
            done: Box::new(move |result| results.borrow_mut().push(result)),
        }
    }

    fn reset_auth_registry() {
        AUTH_FLOWS.with(|flows| *flows.borrow_mut() = AuthFlowRegistry::default());
    }

    #[test]
    fn cancelled_auth_cleanup_stays_on_provider_but_completion_preserves_cross_origin_redirects() {
        assert!(auth_cleanup_matches(
            AUTH_START,
            "https://accounts.example/challenge",
            false
        ));
        for current in [
            "https://reading.example/article",
            "https://accounts.example.evil/article",
            "http://accounts.example/login",
            "https://accounts.example:444/login",
        ] {
            assert!(!auth_cleanup_matches(AUTH_START, current, false));
        }
        // The exact native OAuth callback has completed; an intermediate SSO
        // provider may legitimately differ from the initially requested host.
        assert!(auth_cleanup_matches(
            AUTH_START,
            "https://sso.example/consent",
            true
        ));
        assert!(!auth_cleanup_matches(AUTH_START, "about:blank", true));
    }

    #[test]
    fn auth_initial_view_binding_rejects_a_repurposed_creation_target() {
        reset_auth_registry();
        let profile = ProfileId::from(1);
        let tab = ItemId::from(100);
        let results = AuthResults::default();
        AUTH_FLOWS.with(|flows| {
            flows
                .borrow_mut()
                .insert(fixture_flow(1, profile, tab, &results))
        });
        let token = Arc::new(AtomicBool::new(true));
        bind_auth_flow_view(profile, tab, &token, "https://accounts.example/article");
        assert_eq!(results.borrow().len(), 1);
        assert!(results.borrow()[0].is_err());
        AUTH_FLOWS.with(|flows| assert!(flows.borrow().pending.is_empty()));
        bind_auth_flow_view(profile, tab, &token, AUTH_START);
        assert_eq!(results.borrow().len(), 1);
    }

    #[test]
    fn provider_dashboard_link_gives_up_cleanup_without_breaking_oauth_completion() {
        reset_auth_registry();
        let profile = ProfileId::from(1);
        let tab = ItemId::from(100);
        let results = AuthResults::default();
        let flow = fixture_flow(1, profile, tab, &results);
        let fence = flow.cleanup.clone();
        AUTH_FLOWS.with(|flows| flows.borrow_mut().insert(flow));
        let token = Arc::new(AtomicBool::new(true));
        let permit = EventPermit::bound(&token);
        bind_auth_flow_view(profile, tab, &token, AUTH_START);
        let navigation = crate::navigation_epoch::NavigationEpochTracker::new();
        navigation.begin(AUTH_START).unwrap();
        bind_auth_cleanup_fence(profile, tab, &permit, &navigation);
        note_auth_native_navigation(
            &navigation,
            wry::AppleNavigationAction {
                navigation_type: wry::AppleNavigationType::LinkActivated,
                is_get: true,
                target_is_main_frame: Some(true),
            },
        );
        assert!(!navigation.auth_cleanup_is_owned());
        assert!(!fence.is_owned());
        assert!(intercept_auth_redirect(
            profile,
            tab,
            &permit,
            &navigation,
            Some(true),
            AUTH_REDIRECT
        ));
        assert_eq!(results.borrow().as_slice(), &[Ok(AUTH_REDIRECT.to_owned())]);
    }

    #[test]
    fn native_provider_redirects_forms_and_child_links_keep_the_disposable_signin_tab() {
        let navigation = crate::navigation_epoch::NavigationEpochTracker::new();
        navigation.begin(AUTH_START).unwrap();
        for (kind, frame) in [
            (wry::AppleNavigationType::Other, Some(true)),
            (wry::AppleNavigationType::FormSubmitted, Some(true)),
            (wry::AppleNavigationType::FormResubmitted, Some(true)),
            (wry::AppleNavigationType::LinkActivated, Some(false)),
            (wry::AppleNavigationType::LinkActivated, None),
        ] {
            note_auth_native_navigation(
                &navigation,
                wry::AppleNavigationAction {
                    navigation_type: kind,
                    is_get: true,
                    target_is_main_frame: frame,
                },
            );
            assert!(navigation.auth_cleanup_is_owned());
        }
    }

    #[test]
    fn auth_redirect_requires_own_profile_tab_live_generation_and_main_frame() {
        reset_auth_registry();
        let profile = ProfileId::from(1);
        let tab = ItemId::from(100);
        let results = AuthResults::default();
        AUTH_FLOWS.with(|flows| {
            flows
                .borrow_mut()
                .insert(fixture_flow(1, profile, tab, &results));
        });
        let token = Arc::new(AtomicBool::new(true));
        let permit = EventPermit::bound(&token);
        let navigation = crate::navigation_epoch::NavigationEpochTracker::new();
        navigation.begin("https://accounts.example/login").unwrap();
        let attempt = |profile, tab, permit: &EventPermit, frame, url: &str| {
            intercept_auth_redirect(profile, tab, permit, &navigation, frame, url)
        };
        // Navigation cannot grant a flow its initial generation.
        assert!(!attempt(profile, tab, &permit, Some(true), AUTH_REDIRECT));
        bind_auth_flow_view(ProfileId::from(2), tab, &token, AUTH_START);
        assert!(!attempt(profile, tab, &permit, Some(true), AUTH_REDIRECT));
        bind_auth_flow_view(profile, tab, &token, AUTH_START);
        assert!(!attempt(
            ProfileId::from(2),
            tab,
            &permit,
            Some(true),
            AUTH_REDIRECT
        ));
        assert!(!attempt(
            profile,
            ItemId::from(101),
            &permit,
            Some(true),
            AUTH_REDIRECT
        ));
        assert!(!attempt(profile, tab, &permit, Some(false), AUTH_REDIRECT));
        assert!(!attempt(profile, tab, &permit, None, AUTH_REDIRECT));
        let replacement = Arc::new(AtomicBool::new(true));
        assert!(!attempt(
            profile,
            tab,
            &EventPermit::bound(&replacement),
            Some(true),
            AUTH_REDIRECT
        ));
        for url in [
            "http://abcdefghijklmnopabcdefghijklmnop.chromiumapp.org/cb",
            "https://abcdefghijklmnopabcdefghijklmnop.chromiumapp.org:444/cb",
            "https://abcdefghijklmnopabcdefghijklmnop.chromiumapp.org.evil.example/cb",
            "https://user@abcdefghijklmnopabcdefghijklmnop.chromiumapp.org/cb",
        ] {
            assert!(!attempt(profile, tab, &permit, Some(true), url));
        }
        assert!(results.borrow().is_empty());
        assert!(attempt(profile, tab, &permit, Some(true), AUTH_REDIRECT));
        assert!(!attempt(profile, tab, &permit, Some(true), AUTH_REDIRECT));
        assert_eq!(results.borrow().as_slice(), &[Ok(AUTH_REDIRECT.to_owned())]);
    }

    #[test]
    fn auth_native_replacement_cancels_without_rebinding_or_accepting_old_callbacks() {
        reset_auth_registry();
        let profile = ProfileId::from(1);
        let tab = ItemId::from(100);
        let results = AuthResults::default();
        AUTH_FLOWS.with(|flows| {
            flows
                .borrow_mut()
                .insert(fixture_flow(1, profile, tab, &results));
        });
        let original = Arc::new(AtomicBool::new(true));
        bind_auth_flow_view(profile, tab, &original, AUTH_START);
        original.store(false, Ordering::Release);
        let replacement = Arc::new(AtomicBool::new(true));
        bind_auth_flow_view(profile, tab, &replacement, AUTH_START);
        assert_eq!(results.borrow().len(), 1);
        assert!(results.borrow()[0].is_err());
        AUTH_FLOWS.with(|flows| assert!(flows.borrow().pending.is_empty()));
        bind_auth_flow_view(profile, tab, &original, AUTH_START);
        assert_eq!(results.borrow().len(), 1);
    }

    #[test]
    fn auth_expired_or_revoked_navigation_cannot_return_success() {
        reset_auth_registry();
        let profile = ProfileId::from(1);
        let tab = ItemId::from(100);
        let results = AuthResults::default();
        let mut flow = fixture_flow(1, profile, tab, &results);
        flow.deadline = Instant::now();
        AUTH_FLOWS.with(|flows| flows.borrow_mut().insert(flow));
        let token = Arc::new(AtomicBool::new(true));
        let permit = EventPermit::bound(&token);
        bind_auth_flow_view(profile, tab, &token, AUTH_START);
        let navigation = crate::navigation_epoch::NavigationEpochTracker::new();
        assert!(!intercept_auth_redirect(
            profile,
            tab,
            &permit,
            &navigation,
            Some(true),
            AUTH_REDIRECT
        ));
        navigation.begin("https://accounts.example/login").unwrap();
        assert!(intercept_auth_redirect(
            profile,
            tab,
            &permit,
            &navigation,
            Some(true),
            AUTH_REDIRECT
        ));
        assert_eq!(
            results.borrow().as_slice(),
            &[Err("The sign-in flow timed out.".to_owned())]
        );

        AUTH_FLOWS.with(|flows| {
            flows
                .borrow_mut()
                .insert(fixture_flow(2, profile, tab, &results))
        });
        bind_auth_flow_view(profile, tab, &token, AUTH_START);
        permit.revoke();
        assert!(!intercept_auth_redirect(
            profile,
            tab,
            &permit,
            &navigation,
            Some(true),
            AUTH_REDIRECT
        ));
        let permit = EventPermit::bound(&token);
        navigation.revoke();
        assert!(!intercept_auth_redirect(
            profile,
            tab,
            &permit,
            &navigation,
            Some(true),
            AUTH_REDIRECT
        ));
        settle_auth_flows(|_| true, "cancelled", false);
        assert_eq!(results.borrow().len(), 2);
    }

    #[test]
    fn auth_replacement_is_profile_scoped_and_stale_timeout_is_inert() {
        reset_auth_registry();
        let first_profile = ProfileId::from(1);
        let second_profile = ProfileId::from(2);
        let first_results = AuthResults::default();
        let second_results = AuthResults::default();
        let replacement_results = AuthResults::default();
        let insert = |profile, tab, results: &AuthResults| {
            AUTH_FLOWS.with(|flows| {
                let mut flows = flows.borrow_mut();
                let id = flows.reserve(profile, AUTH_EXTENSION).unwrap();
                (id, flows.insert(fixture_flow(id, profile, tab, results)))
            })
        };
        let (first, previous) = insert(first_profile, ItemId::from(100), &first_results);
        assert!(previous.is_none());
        let (_, previous) = insert(second_profile, ItemId::from(200), &second_results);
        assert!(previous.is_none());
        let (_, previous) = insert(first_profile, ItemId::from(101), &replacement_results);
        let previous = previous.unwrap();
        assert_eq!(previous.id, first);
        previous.finish(Err("replaced".into()), false);
        settle_auth_flows(|flow| flow.id == first, "stale timeout", true);
        assert_eq!(first_results.borrow().len(), 1);
        assert!(second_results.borrow().is_empty());
        assert!(replacement_results.borrow().is_empty());
        cancel_extension_auth_flows(first_profile, AUTH_EXTENSION);
        assert_eq!(replacement_results.borrow().len(), 1);
        assert!(second_results.borrow().is_empty());
        settle_abandoned_auth_flows(second_profile, &std::collections::HashSet::new());
        assert_eq!(second_results.borrow().len(), 1);
        AUTH_FLOWS.with(|flows| assert!(flows.borrow().pending.is_empty()));
    }

    #[test]
    fn auth_terminals_release_registry_before_reentrant_callback() {
        reset_auth_registry();
        let profile = ProfileId::from(1);
        let tab = ItemId::from(100);
        let results = AuthResults::default();
        let mut flow = fixture_flow(1, profile, tab, &results);
        flow.done = Box::new(move |_| {
            AUTH_FLOWS.with(|flows| {
                let mut flows = flows.borrow_mut();
                assert!(flows.pending.is_empty());
                let id = flows.reserve(profile, AUTH_EXTENSION).unwrap();
                flows.insert(fixture_flow(
                    id,
                    profile,
                    ItemId::from(101),
                    &AuthResults::default(),
                ));
            });
        });
        AUTH_FLOWS.with(|flows| {
            let mut flows = flows.borrow_mut();
            flows.next_id = 1;
            flows.insert(flow);
        });
        settle_auth_flows(|flow| flow.id == 1, "cancelled", false);
        AUTH_FLOWS.with(|flows| {
            let flows = flows.borrow();
            assert_eq!(flows.pending.len(), 1);
            assert_eq!(flows.pending[0].tab.get(), Some(ItemId::from(101)));
        });
        reset_auth_registry();
    }

    #[test]
    fn auth_capacity_is_bounded_and_identity_never_wraps() {
        let mut registry = AuthFlowRegistry::default();
        let results = AuthResults::default();
        for value in 1..=MAX_PENDING_AUTH_FLOWS {
            let profile = ProfileId::from(value as u128);
            let id = registry.reserve(profile, AUTH_EXTENSION).unwrap();
            registry.insert(fixture_flow(
                id,
                profile,
                ItemId::from(value as u128),
                &results,
            ));
        }
        assert!(registry
            .reserve(ProfileId::from(100), AUTH_EXTENSION)
            .is_none());
        // Replacing an existing owner needs no extra registry slot.
        let id = registry
            .reserve(ProfileId::from(1), AUTH_EXTENSION)
            .unwrap();
        assert!(registry
            .insert(fixture_flow(
                id,
                ProfileId::from(1),
                ItemId::from(101),
                &results
            ))
            .is_some());
        assert_eq!(registry.pending.len(), MAX_PENDING_AUTH_FLOWS);
        registry.next_id = u64::MAX;
        assert!(registry
            .reserve(ProfileId::from(1), AUTH_EXTENSION)
            .is_none());
        assert_eq!(registry.pending.len(), MAX_PENDING_AUTH_FLOWS);
    }

    #[test]
    fn auth_creation_reply_cannot_resurrect_cancelled_flow_or_change_its_tab() {
        let mut registry = AuthFlowRegistry::default();
        let profile = ProfileId::from(1);
        let results = AuthResults::default();
        let id = registry.reserve(profile, AUTH_EXTENSION).unwrap();
        let flow = fixture_flow(id, profile, ItemId::from(100), &results);
        flow.tab.set(None);
        registry.insert(flow);
        assert!(registry.bind_created_tab(id, ItemId::from(100)));
        assert!(!registry.bind_created_tab(id, ItemId::from(101)));
        assert_eq!(registry.pending[0].tab.get(), Some(ItemId::from(100)));
        let cancelled = registry.take_matching(|flow| flow.id == id);
        for flow in cancelled {
            flow.finish(Err("cancelled".into()), false);
        }
        let replacement = registry.reserve(profile, AUTH_EXTENSION).unwrap();
        let flow = fixture_flow(replacement, profile, ItemId::from(200), &results);
        flow.tab.set(None);
        registry.insert(flow);
        assert!(!registry.bind_created_tab(id, ItemId::from(101)));
        assert!(registry.bind_created_tab(replacement, ItemId::from(200)));
        assert_eq!(results.borrow().len(), 1);
        assert_eq!(registry.pending.len(), 1);
        assert_eq!(registry.pending[0].tab.get(), Some(ItemId::from(200)));
    }

    #[test]
    fn an_action_is_named_by_the_first_line_of_its_title() {
        assert_eq!(
            first_line("Google Translate\n\nLeft-click to translate!"),
            "Google Translate"
        );
        assert_eq!(first_line("\n  Dark Reader \t"), "Dark Reader");
        assert_eq!(first_line("a\u{7}b"), "ab");
        assert_eq!(first_line(""), "");
    }
}
