use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::ptr::NonNull;
use std::rc::Rc;
use std::sync::Once;

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{MainThreadMarker, MainThreadOnly};
use objc2_foundation::{NSArray, NSError, NSSet, NSString, NSURL, NSUUID};
use objc2_web_kit::{
    WKWebExtension, WKWebExtensionContext, WKWebExtensionContextPermissionStatus,
    WKWebExtensionController, WKWebExtensionControllerConfiguration, WKWebExtensionDataRecord,
    WKWebExtensionDataType, WKWebExtensionDataTypeLocal, WKWebExtensionDataTypeSession,
    WKWebExtensionDataTypeSynchronized, WKWebExtensionMatchPattern, WKWebView,
    WKWebViewConfiguration, WKWebsiteDataStore,
};

use crate::delegate::Delegate;
use crate::surface::{Graph, Tab, Window, WindowSnapshot};
use crate::{application_name, Host, LogLevel};

/// Chrome's scheme, so extension origins (and everything stored under them)
/// match what servers and the extension itself expect.
pub(crate) const SCHEME: &str = "chrome-extension";

/// What the user consented to for one extension.
#[derive(Clone, Debug)]
pub enum Grants {
    /// Everything the manifest requires, as accepted at install.
    Requested,
    Explicit {
        permissions: Vec<String>,
        match_patterns: Vec<String>,
    },
}

#[derive(Clone, Debug)]
pub struct ExtensionSpec {
    /// The Chrome Web Store ID; it is also the origin host.
    pub id: String,
    /// A prepared package directory.
    pub root: PathBuf,
    pub grants: Grants,
    /// External hosts are separate from the compatibility native transport.
    pub external_native_messaging: bool,
    pub inspectable: bool,
}

#[derive(Clone, Debug)]
pub struct LoadedExtension {
    pub id: String,
    pub name: String,
    pub version: String,
    pub has_background: bool,
}

pub(crate) struct Loaded {
    pub(crate) context: Retained<WKWebExtensionContext>,
    pub(crate) external_native_messaging: bool,
}

/// One profile's state. WebKit gives an extension the same identifier in
/// every profile, so anything kept per extension lives here rather than in a
/// process-wide table.
pub(crate) struct Shared {
    host: Rc<dyn Host>,
    pub(crate) graph: RefCell<Graph>,
    pub(crate) loaded: RefCell<HashMap<String, Loaded>>,
    pub(crate) offscreen: crate::offscreen::Documents,
    pub(crate) workers: crate::lifetime::Workers,
    pub(crate) access: crate::access::Batches,
    pub(crate) opened: crate::opened::Opened,
    pub(crate) mtm: MainThreadMarker,
}

impl Shared {
    pub(crate) fn host(&self) -> &Rc<dyn Host> {
        &self.host
    }

    pub(crate) fn log(&self, context: &WKWebExtensionContext, level: LogLevel, message: &str) {
        let id = unsafe { context.uniqueIdentifier() }.to_string();
        self.host.log(&id, level, message);
    }
}

/// One profile's extension runtime.
pub struct Runtime {
    controller: Retained<WKWebExtensionController>,
    _delegate: Retained<Delegate>,
    shared: Rc<Shared>,
}

impl Runtime {
    /// `identifier` names a persistent controller whose extension storage
    /// survives restarts; `None` keeps everything in memory.
    pub fn new(
        mtm: MainThreadMarker,
        store: &WKWebsiteDataStore,
        identifier: Option<&NSUUID>,
        host: Rc<dyn Host>,
    ) -> Self {
        static SCHEME_REGISTERED: Once = Once::new();
        SCHEME_REGISTERED.call_once(|| unsafe {
            WKWebExtensionMatchPattern::registerCustomURLScheme(&NSString::from_str(SCHEME), mtm)
        });

        let configuration = unsafe {
            match identifier {
                Some(identifier) => {
                    WKWebExtensionControllerConfiguration::configurationWithIdentifier(
                        identifier, mtm,
                    )
                }
                None => WKWebExtensionControllerConfiguration::nonPersistentConfiguration(mtm),
            }
        };
        let views = unsafe { WKWebViewConfiguration::new(mtm) };
        unsafe {
            views.setWebsiteDataStore(store);
            views.setApplicationNameForUserAgent(Some(&NSString::from_str(application_name())));
            configuration.setDefaultWebsiteDataStore(Some(store));
            configuration.setWebViewConfiguration(Some(&views));
        }
        let controller = unsafe {
            WKWebExtensionController::initWithConfiguration(
                WKWebExtensionController::alloc(mtm),
                &configuration,
            )
        };
        let shared = Rc::new(Shared {
            host,
            graph: RefCell::new(Graph::default()),
            loaded: RefCell::new(HashMap::new()),
            offscreen: Default::default(),
            workers: Default::default(),
            access: Default::default(),
            opened: Default::default(),
            mtm,
        });
        let delegate = Delegate::new(mtm, Rc::downgrade(&shared));
        unsafe { controller.setDelegate(Some(ProtocolObject::from_ref(&*delegate))) };
        Self {
            controller,
            _delegate: delegate,
            shared,
        }
    }

    /// Exercises native-open admission in the standalone main-thread lab.
    #[cfg(feature = "lab")]
    pub fn verify_native_open_admission(&self, id: &str) -> Result<(), String> {
        crate::native::verify_open_admission(&self.shared, id)
    }

    /// Exercises offscreen origin admission in the standalone main-thread lab.
    #[cfg(feature = "lab")]
    pub fn verify_offscreen_origin(&self, id: &str) -> Result<(), String> {
        let loaded = self.shared.loaded.borrow();
        let context = &loaded
            .get(id)
            .ok_or("fixture extension was not loaded")?
            .context;
        crate::offscreen::verify_origin(context)
    }

    pub fn controller(&self) -> &WKWebExtensionController {
        &self.controller
    }

    /// Attaches the runtime to a tab's configuration before its view exists.
    pub fn configure(&self, configuration: &WKWebViewConfiguration) {
        unsafe {
            configuration.setWebExtensionController(Some(&self.controller));
            configuration
                .setApplicationNameForUserAgent(Some(&NSString::from_str(application_name())));
        }
    }

    pub fn context(&self, id: &str) -> Option<Retained<WKWebExtensionContext>> {
        self.shared
            .loaded
            .borrow()
            .get(id)
            .map(|loaded| loaded.context.clone())
    }

    pub fn loaded_ids(&self) -> Vec<String> {
        self.shared.loaded.borrow().keys().cloned().collect()
    }

    /// Loads a prepared package. WebKit reads the package asynchronously.
    pub fn load(
        &self,
        spec: ExtensionSpec,
        done: impl FnOnce(Result<LoadedExtension, String>) + 'static,
    ) {
        if self.shared.loaded.borrow().contains_key(&spec.id) {
            done(Err(format!("{} is already loaded", spec.id)));
            return;
        }
        let Some(url) = directory_url(&spec.root) else {
            done(Err(format!(
                "{} is not a readable directory",
                spec.root.display()
            )));
            return;
        };
        let shared = Rc::downgrade(&self.shared);
        let controller = self.controller.clone();
        let done = RefCell::new(Some(done));
        let spec = RefCell::new(Some(spec));
        let completion =
            RcBlock::new(move |extension: *mut WKWebExtension, error: *mut NSError| {
                let (Some(done), Some(spec)) = (done.take(), spec.take()) else {
                    return;
                };
                let Some(shared) = shared.upgrade() else {
                    return done(Err("the runtime was closed".to_owned()));
                };
                let result = match unsafe { extension.as_ref() } {
                    Some(extension) => activate(&shared, &controller, extension, spec),
                    None => Err(describe(unsafe { error.as_ref() })),
                };
                done(result);
            });
        unsafe {
            WKWebExtension::extensionWithResourceBaseURL_completionHandler(
                &url,
                &completion,
                self.shared.mtm,
            )
        };
    }

    pub fn unload(&self, id: &str) -> bool {
        unload(&self.shared, &self.controller, id)
    }

    /// Unloads an extension and deletes everything it stored in this
    /// profile: its origin's website data, cleared from a page of its own
    /// while it is still loaded, then its `chrome.storage` areas.
    pub fn erase(&self, id: &str, done: impl FnOnce() + 'static) {
        let controller = self.controller.clone();
        let shared = self.shared.clone();
        let target = id.to_string();
        let finish = move || {
            unload(&shared, &controller, &target);
            erase_storage(&controller, &shared.host, &target);
            done();
        };
        match self.context(id) {
            Some(context) => crate::offscreen::clear_origin(&context, Box::new(finish)),
            None => finish(),
        }
    }

    /// Opens an extension's options page through the host; false when it
    /// has none or isn't loaded.
    pub fn open_options(&self, id: &str) -> bool {
        let url = self
            .context(id)
            .and_then(|context| unsafe { context.optionsPageURL() })
            .and_then(|url| url.absoluteString());
        match url {
            Some(url) => {
                self.shared.host.open_options(id, &url.to_string());
                true
            }
            None => false,
        }
    }

    /// Starts an extension's background now rather than on its first event.
    pub fn start_background(&self, id: &str, done: impl FnOnce(Result<(), String>) + 'static) {
        let Some(context) = self.context(id) else {
            return done(Err(format!("{id} is not loaded")));
        };
        let done = RefCell::new(Some(done));
        let completion = RcBlock::new(move |error: *mut NSError| {
            if let Some(done) = done.take() {
                done(match unsafe { error.as_ref() } {
                    None => Ok(()),
                    Some(error) => Err(describe(Some(error))),
                });
            }
        });
        unsafe { context.loadBackgroundContentWithCompletionHandler(&completion) };
    }

    /// Runs an extension's toolbar action as a user click on `tab`: WebKit
    /// either fires `action.onClicked` or asks the host to present the popup.
    pub fn perform_action(&self, id: &str, tab: Option<u64>) -> bool {
        let Some(context) = self.context(id) else {
            return false;
        };
        let tab = tab.and_then(|tab| self.shared.graph.borrow().tab(tab));
        unsafe {
            if let Some(tab) = &tab {
                context.userGesturePerformedInTab(ProtocolObject::from_ref(&**tab));
            }
            context.performActionForTab(tab.as_deref().map(ProtocolObject::from_ref));
        }
        true
    }

    /// Replaces the browser's windows and tabs, telling WebKit what changed.
    pub fn publish(&self, windows: &[WindowSnapshot], focused: Option<u64>) {
        let mtm = self.shared.mtm;
        let (previous_windows, previous_tabs, previous_active, previous_focus) = {
            let graph = self.shared.graph.borrow();
            let tabs: HashMap<u64, (Retained<Tab>, u64, usize)> = graph
                .windows
                .iter()
                .flat_map(|window| {
                    window
                        .tabs()
                        .into_iter()
                        .enumerate()
                        .map(move |(index, tab)| (tab.id(), (tab, window.id(), index)))
                        .collect::<Vec<_>>()
                })
                .collect();
            let active: HashMap<u64, Option<u64>> = graph
                .windows
                .iter()
                .map(|window| (window.id(), window.active_id()))
                .collect();
            (graph.windows.clone(), tabs, active, graph.focused)
        };

        let mut windows_out = Vec::with_capacity(windows.len());
        let mut opened_windows = Vec::new();
        let mut opened_tabs = Vec::new();
        let mut changed = Vec::new();
        let mut moved = Vec::new();
        let mut activated = Vec::new();
        let mut kept_tabs = HashMap::new();
        for snapshot in windows {
            let window = previous_windows
                .iter()
                .find(|window| window.id() == snapshot.id)
                .cloned()
                .unwrap_or_else(|| {
                    let window = Window::new(mtm, snapshot.id, Rc::downgrade(&self.shared));
                    opened_windows.push(window.clone());
                    window
                });
            window.set_frame(snapshot.frame);
            let reordered = reordered(snapshot, &previous_tabs);
            let mut tabs = Vec::with_capacity(snapshot.tabs.len());
            for tab_snapshot in &snapshot.tabs {
                let tab = match previous_tabs.get(&tab_snapshot.id) {
                    Some((tab, window_id, old_index)) => {
                        if *window_id != snapshot.id || reordered.contains(&tab_snapshot.id) {
                            moved.push((tab.clone(), *old_index, *window_id));
                        }
                        tab.clone()
                    }
                    None => {
                        let tab = Tab::new(mtm, tab_snapshot.id, Rc::downgrade(&self.shared));
                        opened_tabs.push(tab.clone());
                        tab
                    }
                };
                let changes = tab.update(tab_snapshot.clone());
                if changes.any() && previous_tabs.contains_key(&tab_snapshot.id) {
                    changed.push((tab.clone(), changes));
                }
                kept_tabs.insert(tab_snapshot.id, ());
                tabs.push(tab);
            }
            let before = previous_active.get(&snapshot.id).copied().flatten();
            window.set_tabs(tabs, snapshot.active);
            if snapshot.active != before {
                activated.push((window.clone(), before));
            }
            windows_out.push(window);
        }
        let closed_tabs: Vec<_> = previous_tabs
            .iter()
            .filter(|(id, _)| !kept_tabs.contains_key(id))
            .map(|(_, (tab, window_id, _))| (tab.clone(), *window_id))
            .collect();
        let closed_windows: Vec<_> = previous_windows
            .iter()
            .filter(|window| windows.iter().all(|snapshot| snapshot.id != window.id()))
            .cloned()
            .collect();
        {
            let mut graph = self.shared.graph.borrow_mut();
            graph.windows = windows_out;
            graph.focused = focused;
        }

        if crate::tracing()
            && (!opened_tabs.is_empty() || !closed_tabs.is_empty() || !activated.is_empty())
        {
            eprintln!(
                "webext-trace: graph opened {:?} closed {:?} activated {:?}",
                opened_tabs.iter().map(|tab| tab.id()).collect::<Vec<_>>(),
                closed_tabs
                    .iter()
                    .map(|(tab, _)| tab.id())
                    .collect::<Vec<_>>(),
                activated
                    .iter()
                    .map(|(window, _)| window.active_id())
                    .collect::<Vec<_>>(),
            );
        }
        let controller = &self.controller;
        unsafe {
            for window in &opened_windows {
                controller.didOpenWindow(ProtocolObject::from_ref(&**window));
            }
            for tab in &opened_tabs {
                controller.didOpenTab(ProtocolObject::from_ref(&**tab));
            }
            for (tab, window_id) in &closed_tabs {
                let window_closing = closed_windows
                    .iter()
                    .any(|window| window.id() == *window_id);
                controller
                    .didCloseTab_windowIsClosing(ProtocolObject::from_ref(&**tab), window_closing);
                tab.detach();
            }
            for window in &closed_windows {
                controller.didCloseWindow(ProtocolObject::from_ref(&**window));
            }
            for (tab, old_index, old_window) in &moved {
                let old_window = previous_windows
                    .iter()
                    .find(|window| window.id() == *old_window);
                controller.didMoveTab_fromIndex_inWindow(
                    ProtocolObject::from_ref(&**tab),
                    *old_index,
                    old_window.map(|window| ProtocolObject::from_ref(&**window)),
                );
            }
            for (tab, changes) in &changed {
                controller.didChangeTabProperties_forTab(
                    tab_properties(*changes),
                    ProtocolObject::from_ref(&**tab),
                );
            }
            for (window, before) in &activated {
                if let Some(active) = window.active_tab() {
                    let previous = before.and_then(|id| self.shared.graph.borrow().tab(id));
                    controller.didActivateTab_previousActiveTab(
                        ProtocolObject::from_ref(&*active),
                        previous.as_deref().map(ProtocolObject::from_ref),
                    );
                }
            }
            if focused != previous_focus {
                let window = focused.and_then(|id| self.shared.graph.borrow().window(id));
                controller.didFocusWindow(window.as_deref().map(ProtocolObject::from_ref));
            }
        }
    }

    /// Associates a tab with the web view currently showing it.
    pub fn bind_view(&self, tab: u64, view: Option<&WKWebView>) {
        if let Some(tab) = self.shared.graph.borrow().tab(tab) {
            tab.set_webview(view);
        }
    }

    /// [`Self::bind_view`] for many tabs at once, such as every tab after a
    /// publish.
    pub fn bind_views<'a>(&self, views: impl IntoIterator<Item = (u64, Option<&'a WKWebView>)>) {
        let tabs: HashMap<u64, Retained<Tab>> = self
            .shared
            .graph
            .borrow()
            .windows
            .iter()
            .flat_map(|window| window.tabs())
            .map(|tab| (tab.id(), tab))
            .collect();
        for (id, view) in views {
            if let Some(tab) = tabs.get(&id) {
                tab.set_webview(view);
            }
        }
    }

    /// Tabs showing one of an extension's own pages, which keep its process
    /// running until they close.
    pub fn tabs_showing(&self, id: &str) -> Vec<u64> {
        let graph = self.shared.graph.borrow();
        graph
            .windows
            .iter()
            .flat_map(|window| window.tabs())
            .filter(|tab| {
                tab.webview()
                    .and_then(|view| unsafe { view.URL() })
                    .is_some_and(|url| {
                        url.scheme()
                            .is_some_and(|scheme| scheme.to_string() == SCHEME)
                            && url.host().is_some_and(|host| host.to_string() == id)
                    })
            })
            .map(|tab| tab.id())
            .collect()
    }

    pub fn tab_object(
        &self,
        tab: u64,
    ) -> Option<Retained<ProtocolObject<dyn objc2_web_kit::WKWebExtensionTab>>> {
        self.shared
            .graph
            .borrow()
            .tab(tab)
            .map(ProtocolObject::from_retained)
    }
}

/// Tabs of `snapshot` whose order changed among the tabs that stayed in that
/// window. Opening or closing a tab shifts every later index but moves none
/// of them; reporting those would wake every `tabs.onMoved` listener.
fn reordered(
    snapshot: &WindowSnapshot,
    previous: &HashMap<u64, (Retained<Tab>, u64, usize)>,
) -> HashSet<u64> {
    let after: Vec<u64> = snapshot
        .tabs
        .iter()
        .map(|tab| tab.id)
        .filter(|id| {
            previous
                .get(id)
                .is_some_and(|(_, window, _)| *window == snapshot.id)
        })
        .collect();
    let mut before = after.clone();
    before.sort_by_key(|id| previous.get(id).map_or(0, |(_, _, index)| *index));
    moved_between(&before, &after)
}

fn moved_between(before: &[u64], after: &[u64]) -> HashSet<u64> {
    let predecessors = |order: &[u64]| -> HashMap<u64, Option<u64>> {
        order
            .iter()
            .enumerate()
            .map(|(index, id)| (*id, index.checked_sub(1).map(|p| order[p])))
            .collect()
    };
    let (old, new) = (predecessors(before), predecessors(after));
    after
        .iter()
        .copied()
        .filter(|id| old.get(id) != new.get(id))
        .collect()
}

fn activate(
    shared: &Rc<Shared>,
    controller: &WKWebExtensionController,
    extension: &WKWebExtension,
    spec: ExtensionSpec,
) -> Result<LoadedExtension, String> {
    let context = unsafe { WKWebExtensionContext::contextForExtension(extension) };
    let base = NSURL::URLWithString(&NSString::from_str(&format!("{SCHEME}://{}/", spec.id)))
        .ok_or_else(|| format!("{} is not a valid extension ID", spec.id))?;
    unsafe {
        context.setUniqueIdentifier(&NSString::from_str(&spec.id));
        context.setBaseURL(&base);
        context.setInspectable(spec.inspectable);
    }
    apply_grants(shared.mtm, &context, extension, &spec.grants);
    unsafe { controller.loadExtensionContext_error(&context) }
        .map_err(|error| describe(Some(&error)))?;

    let name = unsafe { extension.displayName() }
        .map(|name| name.to_string())
        .unwrap_or_else(|| spec.id.clone());
    let loaded = LoadedExtension {
        id: spec.id.clone(),
        name,
        version: unsafe { extension.version() }
            .map(|version| version.to_string())
            .unwrap_or_default(),
        has_background: unsafe { extension.hasBackgroundContent() },
    };
    for error in unsafe { context.errors() }.iter() {
        shared.log(&context, LogLevel::Warning, &describe(Some(&error)));
    }
    shared.loaded.borrow_mut().insert(
        spec.id,
        Loaded {
            context,
            external_native_messaging: spec.external_native_messaging,
        },
    );
    Ok(loaded)
}

fn apply_grants(
    mtm: MainThreadMarker,
    context: &WKWebExtensionContext,
    extension: &WKWebExtension,
    grants: &Grants,
) {
    let granted = WKWebExtensionContextPermissionStatus::GrantedExplicitly;
    unsafe {
        match grants {
            Grants::Requested => {
                for permission in extension.requestedPermissions().iter() {
                    context.setPermissionStatus_forPermission(granted, &permission);
                }
                for pattern in extension.allRequestedMatchPatterns().iter() {
                    context.setPermissionStatus_forMatchPattern(granted, &pattern);
                }
            }
            Grants::Explicit {
                permissions,
                match_patterns,
            } => {
                for permission in permissions {
                    context.setPermissionStatus_forPermission(
                        granted,
                        &NSString::from_str(permission),
                    );
                }
                for pattern in match_patterns {
                    if let Some(pattern) = WKWebExtensionMatchPattern::matchPatternWithString(
                        &NSString::from_str(pattern),
                        mtm,
                    ) {
                        context.setPermissionStatus_forMatchPattern(granted, &pattern);
                    }
                }
            }
        }
        // This grant is transport for the browser's reserved bridges only.
        // The external-host broker checks separate original reviewed authority.
        context.setPermissionStatus_forPermission(granted, &NSString::from_str("nativeMessaging"));
        // With a custom scheme registered, WebKit counts other extensions'
        // pages as part of <all_urls>; Chrome never does.
        for scheme in [SCHEME, "webkit-extension"] {
            if let Some(pattern) = WKWebExtensionMatchPattern::matchPatternWithString(
                &NSString::from_str(&format!("{scheme}://*/*")),
                mtm,
            ) {
                context.setPermissionStatus_forMatchPattern(
                    WKWebExtensionContextPermissionStatus::DeniedExplicitly,
                    &pattern,
                );
            }
        }
    }
}

fn tab_properties(
    changes: crate::surface::TabChanges,
) -> objc2_web_kit::WKWebExtensionTabChangedProperties {
    use objc2_web_kit::WKWebExtensionTabChangedProperties as Properties;
    let mut properties = Properties::empty();
    if changes.title {
        properties |= Properties::Title;
    }
    if changes.url {
        properties |= Properties::URL;
    }
    if changes.loading {
        properties |= Properties::Loading;
    }
    if changes.pinned {
        properties |= Properties::Pinned;
    }
    properties
}

fn directory_url(path: &std::path::Path) -> Option<Retained<NSURL>> {
    if !path.is_dir() {
        return None;
    }
    let path = NSString::from_str(path.to_str()?);
    Some(NSURL::fileURLWithPath_isDirectory(&path, true))
}

pub(crate) fn describe(error: Option<&NSError>) -> String {
    let Some(error) = error else {
        return "unknown error".to_owned();
    };
    // WebKit hands out WKNSError proxies that forward NSError's methods, which
    // objc2's debug-time method verification rejects; send them unchecked.
    unsafe fn text(receiver: &NSError, selector: objc2::runtime::Sel) -> Option<String> {
        let send: unsafe extern "C" fn(*const NSError, objc2::runtime::Sel) -> *const NSString = unsafe {
            std::mem::transmute(objc2::ffi::objc_msgSend as unsafe extern "C-unwind" fn())
        };
        let result = unsafe { send(receiver, selector) };
        unsafe { result.as_ref() }.map(|value| value.to_string())
    }
    let description = unsafe { text(error, objc2::sel!(localizedDescription)) };
    let reason = unsafe { text(error, objc2::sel!(localizedFailureReason)) };
    match (description, reason) {
        (Some(description), Some(reason)) => format!("{description}: {reason}"),
        (Some(description), None) => description,
        _ => unsafe { text(error, objc2::sel!(description)) }
            .unwrap_or_else(|| "unknown error".into()),
    }
}

fn unload(shared: &Shared, controller: &WKWebExtensionController, id: &str) -> bool {
    let Some(loaded) = shared.loaded.borrow_mut().remove(id) else {
        return false;
    };
    crate::offscreen::close(shared, id);
    crate::worker_gone(shared, id);
    crate::lifetime::forget(shared, id);
    unsafe { controller.unloadExtensionContext_error(&loaded.context) }.is_ok()
}

/// Deletes an unloaded extension's `chrome.storage` areas.
fn erase_storage(controller: &Retained<WKWebExtensionController>, host: &Rc<dyn Host>, id: &str) {
    let types: Vec<&WKWebExtensionDataType> = unsafe {
        [
            WKWebExtensionDataTypeLocal,
            WKWebExtensionDataTypeSession,
            WKWebExtensionDataTypeSynchronized,
        ]
    }
    .into_iter()
    .flatten()
    .collect();
    let types = NSSet::from_slice(&types);
    let removal_types = types.clone();
    let removal = controller.clone();
    let host = host.clone();
    let target = id.to_string();
    let fetched = RcBlock::new(move |records: NonNull<NSArray<WKWebExtensionDataRecord>>| {
        let matching: Vec<_> = unsafe { records.as_ref() }
            .iter()
            .filter(|record| unsafe { record.uniqueIdentifier() }.to_string() == target)
            .collect();
        if matching.is_empty() {
            return;
        }
        let (host, target) = (host.clone(), target.clone());
        let removed = RcBlock::new(move || {
            host.log(&target, LogLevel::Info, "erased the extension's data");
        });
        unsafe {
            removal.removeDataOfTypes_fromDataRecords_completionHandler(
                &removal_types,
                &NSArray::from_retained_slice(&matching),
                &removed,
            )
        };
    });
    unsafe { controller.fetchDataRecordsOfTypes_completionHandler(&types, &fetched) };
}

#[cfg(test)]
mod reorder_tests {
    use super::moved_between;

    #[test]
    fn only_a_real_reorder_counts_as_a_move() {
        assert!(moved_between(&[1, 2, 3], &[1, 2, 3]).is_empty());
        let mut moved: Vec<_> = moved_between(&[1, 2, 3], &[2, 3, 1]).into_iter().collect();
        moved.sort_unstable();
        assert_eq!(moved, [1, 2]);
    }
}
