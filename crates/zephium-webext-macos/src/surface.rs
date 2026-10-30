//! The browser's windows and tabs as WebKit's extension runtime sees them.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak as RcWeak};

use block2::DynBlock;
use objc2::rc::{Retained, Weak};
use objc2::runtime::{NSObject, ProtocolObject};
use objc2::{define_class, msg_send, DefinedClass, MainThreadOnly};
use objc2_foundation::{
    MainThreadMarker, NSArray, NSError, NSObjectProtocol, NSPoint, NSRect, NSSize, NSString, NSURL,
};
use objc2_web_kit::{
    WKWebExtensionContext, WKWebExtensionTab, WKWebExtensionWindow, WKWebExtensionWindowState,
    WKWebExtensionWindowType, WKWebView,
};

use crate::runtime::Shared;
use crate::{error, TabRequest};

/// One tab as published by the browser.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TabSnapshot {
    pub id: u64,
    pub title: String,
    pub url: Option<String>,
    pub loading: bool,
    pub pinned: bool,
}

/// One window as published by the browser, tabs in visual order.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WindowSnapshot {
    pub id: u64,
    pub tabs: Vec<TabSnapshot>,
    pub active: Option<u64>,
    pub frame: (f64, f64, f64, f64),
}

pub(crate) struct TabIvars {
    id: u64,
    shared: RcWeak<Shared>,
    window: RefCell<Option<Weak<Window>>>,
    webview: RefCell<Option<Weak<WKWebView>>>,
    snapshot: RefCell<TabSnapshot>,
    index: Cell<usize>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "ZephiumWebExtTab"]
    #[ivars = TabIvars]
    pub(crate) struct Tab;

    unsafe impl NSObjectProtocol for Tab {}

    unsafe impl WKWebExtensionTab for Tab {
        #[unsafe(method_id(windowForWebExtensionContext:))]
        fn window_for(
            &self,
            _context: &WKWebExtensionContext,
        ) -> Option<Retained<ProtocolObject<dyn WKWebExtensionWindow>>> {
            self.window().map(ProtocolObject::from_retained)
        }

        #[unsafe(method(indexInWindowForWebExtensionContext:))]
        fn index_for(&self, _context: &WKWebExtensionContext) -> usize {
            self.ivars().index.get()
        }

        #[unsafe(method_id(webViewForWebExtensionContext:))]
        fn webview_for(&self, _context: &WKWebExtensionContext) -> Option<Retained<WKWebView>> {
            self.webview()
        }

        #[unsafe(method_id(titleForWebExtensionContext:))]
        fn title_for(&self, _context: &WKWebExtensionContext) -> Option<Retained<NSString>> {
            Some(NSString::from_str(&self.ivars().snapshot.borrow().title))
        }

        #[unsafe(method_id(urlForWebExtensionContext:))]
        fn url_for(&self, _context: &WKWebExtensionContext) -> Option<Retained<NSURL>> {
            self.webview()
                .and_then(|view| unsafe { view.URL() })
                .or_else(|| {
                    let snapshot = self.ivars().snapshot.borrow();
                    snapshot
                        .url
                        .as_deref()
                        .and_then(|url| NSURL::URLWithString(&NSString::from_str(url)))
                })
        }

        #[unsafe(method(isLoadingCompleteForWebExtensionContext:))]
        fn loading_complete(&self, _context: &WKWebExtensionContext) -> bool {
            !self.ivars().snapshot.borrow().loading
        }

        #[unsafe(method(isPinnedForWebExtensionContext:))]
        fn pinned(&self, _context: &WKWebExtensionContext) -> bool {
            self.ivars().snapshot.borrow().pinned
        }

        #[unsafe(method(isSelectedForWebExtensionContext:))]
        fn selected(&self, _context: &WKWebExtensionContext) -> bool {
            self.window()
                .and_then(|window| window.active_id())
                .is_some_and(|active| active == self.id())
        }

        // An extension may use activeTab after the user invokes it on a tab,
        // as in Chrome: the grant lasts until that tab navigates away.
        #[unsafe(method(shouldGrantPermissionsOnUserGestureForWebExtensionContext:))]
        fn grants_on_gesture(&self, _context: &WKWebExtensionContext) -> bool {
            true
        }

        #[unsafe(method(loadURL:forWebExtensionContext:completionHandler:))]
        fn load_url(
            &self,
            url: &NSURL,
            _context: &WKWebExtensionContext,
            completion: &DynBlock<dyn Fn(*mut NSError)>,
        ) {
            let url = url
                .absoluteString()
                .map(|url| url.to_string())
                .unwrap_or_default();
            self.request(
                TabRequest::Load {
                    tab: self.id(),
                    url,
                },
                completion,
            );
        }

        #[unsafe(method(activateForWebExtensionContext:completionHandler:))]
        fn activate(
            &self,
            _context: &WKWebExtensionContext,
            completion: &DynBlock<dyn Fn(*mut NSError)>,
        ) {
            self.request(TabRequest::Activate { tab: self.id() }, completion);
        }

        #[unsafe(method(setSelected:forWebExtensionContext:completionHandler:))]
        fn set_selected(
            &self,
            selected: bool,
            _context: &WKWebExtensionContext,
            completion: &DynBlock<dyn Fn(*mut NSError)>,
        ) {
            if selected {
                self.request(TabRequest::Activate { tab: self.id() }, completion);
            } else {
                completion.call((std::ptr::null_mut(),));
            }
        }

        #[unsafe(method(closeForWebExtensionContext:completionHandler:))]
        fn close(
            &self,
            _context: &WKWebExtensionContext,
            completion: &DynBlock<dyn Fn(*mut NSError)>,
        ) {
            self.request(TabRequest::Close { tab: self.id() }, completion);
        }

        #[unsafe(method(reloadFromOrigin:forWebExtensionContext:completionHandler:))]
        fn reload(
            &self,
            from_origin: bool,
            _context: &WKWebExtensionContext,
            completion: &DynBlock<dyn Fn(*mut NSError)>,
        ) {
            self.request(
                TabRequest::Reload {
                    tab: self.id(),
                    bypass_cache: from_origin,
                },
                completion,
            );
        }

        #[unsafe(method(goBackForWebExtensionContext:completionHandler:))]
        fn go_back(
            &self,
            _context: &WKWebExtensionContext,
            completion: &DynBlock<dyn Fn(*mut NSError)>,
        ) {
            self.request(TabRequest::Back { tab: self.id() }, completion);
        }

        #[unsafe(method(goForwardForWebExtensionContext:completionHandler:))]
        fn go_forward(
            &self,
            _context: &WKWebExtensionContext,
            completion: &DynBlock<dyn Fn(*mut NSError)>,
        ) {
            self.request(TabRequest::Forward { tab: self.id() }, completion);
        }

        #[unsafe(method(setPinned:forWebExtensionContext:completionHandler:))]
        fn set_pinned(
            &self,
            pinned: bool,
            _context: &WKWebExtensionContext,
            completion: &DynBlock<dyn Fn(*mut NSError)>,
        ) {
            self.request(
                TabRequest::Pin {
                    tab: self.id(),
                    pinned,
                },
                completion,
            );
        }
    }
);

impl Tab {
    pub(crate) fn new(mtm: MainThreadMarker, id: u64, shared: RcWeak<Shared>) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(TabIvars {
            id,
            shared,
            window: RefCell::new(None),
            webview: RefCell::new(None),
            snapshot: RefCell::new(TabSnapshot {
                id,
                ..TabSnapshot::default()
            }),
            index: Cell::new(0),
        });
        unsafe { msg_send![super(this), init] }
    }

    pub(crate) fn id(&self) -> u64 {
        self.ivars().id
    }

    pub(crate) fn window(&self) -> Option<Retained<Window>> {
        self.ivars().window.borrow().as_ref().and_then(Weak::load)
    }

    pub(crate) fn webview(&self) -> Option<Retained<WKWebView>> {
        self.ivars().webview.borrow().as_ref().and_then(Weak::load)
    }

    pub(crate) fn set_webview(&self, view: Option<&WKWebView>) {
        let current = self.webview();
        if current.as_deref().map(std::ptr::from_ref) == view.map(std::ptr::from_ref) {
            return;
        }
        if crate::tracing() {
            let had = current.is_some();
            if had != view.is_some() {
                eprintln!(
                    "webext-trace: tab {} view {}",
                    self.id(),
                    if view.is_some() { "bound" } else { "unbound" }
                );
            }
        }
        *self.ivars().webview.borrow_mut() = view.map(Weak::new);
    }

    pub(crate) fn place(&self, window: &Window, index: usize) {
        *self.ivars().window.borrow_mut() = Some(Weak::new(window));
        self.ivars().index.set(index);
    }

    pub(crate) fn detach(&self) {
        *self.ivars().window.borrow_mut() = None;
    }

    /// Stores the new snapshot and reports what an extension can observe
    /// changing.
    pub(crate) fn update(&self, snapshot: TabSnapshot) -> TabChanges {
        let mut current = self.ivars().snapshot.borrow_mut();
        let changes = TabChanges {
            title: current.title != snapshot.title,
            url: current.url != snapshot.url,
            loading: current.loading != snapshot.loading,
            pinned: current.pinned != snapshot.pinned,
        };
        *current = snapshot;
        changes
    }

    fn request(&self, request: TabRequest, completion: &DynBlock<dyn Fn(*mut NSError)>) {
        let completion = completion.copy();
        let Some(shared) = self.ivars().shared.upgrade() else {
            completion.call((Retained::as_ptr(&error("The browser is closing.")).cast_mut(),));
            return;
        };
        shared.host().tab_request(
            request,
            Box::new(move |result| match result {
                Ok(_) => completion.call((std::ptr::null_mut(),)),
                Err(message) => {
                    let error = error(&message);
                    completion.call((Retained::as_ptr(&error).cast_mut(),));
                }
            }),
        );
    }
}

#[derive(Clone, Copy, Default)]
pub(crate) struct TabChanges {
    pub(crate) title: bool,
    pub(crate) url: bool,
    pub(crate) loading: bool,
    pub(crate) pinned: bool,
}

impl TabChanges {
    pub(crate) fn any(self) -> bool {
        self.title || self.url || self.loading || self.pinned
    }
}

pub(crate) struct WindowIvars {
    id: u64,
    shared: RcWeak<Shared>,
    tabs: RefCell<Vec<Retained<Tab>>>,
    active: Cell<Option<u64>>,
    frame: Cell<(f64, f64, f64, f64)>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "ZephiumWebExtWindow"]
    #[ivars = WindowIvars]
    pub(crate) struct Window;

    unsafe impl NSObjectProtocol for Window {}

    unsafe impl WKWebExtensionWindow for Window {
        #[unsafe(method_id(tabsForWebExtensionContext:))]
        fn tabs_for(
            &self,
            _context: &WKWebExtensionContext,
        ) -> Retained<NSArray<ProtocolObject<dyn WKWebExtensionTab>>> {
            let tabs = self.ivars().tabs.borrow();
            let protocols: Vec<_> = tabs
                .iter()
                .map(|tab| ProtocolObject::from_retained(tab.clone()))
                .collect();
            NSArray::from_retained_slice(&protocols)
        }

        #[unsafe(method_id(activeTabForWebExtensionContext:))]
        fn active_tab_for(
            &self,
            _context: &WKWebExtensionContext,
        ) -> Option<Retained<ProtocolObject<dyn WKWebExtensionTab>>> {
            self.active_tab().map(ProtocolObject::from_retained)
        }

        #[unsafe(method(windowTypeForWebExtensionContext:))]
        fn window_type(&self, _context: &WKWebExtensionContext) -> WKWebExtensionWindowType {
            WKWebExtensionWindowType::Normal
        }

        #[unsafe(method(windowStateForWebExtensionContext:))]
        fn window_state(&self, _context: &WKWebExtensionContext) -> WKWebExtensionWindowState {
            WKWebExtensionWindowState::Normal
        }

        #[unsafe(method(isPrivateForWebExtensionContext:))]
        fn is_private(&self, _context: &WKWebExtensionContext) -> bool {
            false
        }

        #[unsafe(method(frameForWebExtensionContext:))]
        fn frame_for(&self, _context: &WKWebExtensionContext) -> NSRect {
            let (x, y, width, height) = self.ivars().frame.get();
            NSRect::new(NSPoint::new(x, y), NSSize::new(width, height))
        }

        #[unsafe(method(focusForWebExtensionContext:completionHandler:))]
        fn focus(
            &self,
            _context: &WKWebExtensionContext,
            completion: &DynBlock<dyn Fn(*mut NSError)>,
        ) {
            let completion = completion.copy();
            let Some(shared) = self.ivars().shared.upgrade() else {
                completion.call((std::ptr::null_mut(),));
                return;
            };
            shared.host().tab_request(
                TabRequest::FocusWindow { window: self.id() },
                Box::new(move |_| completion.call((std::ptr::null_mut(),))),
            );
        }

        // An extension's windows are tabs of this window (see the delegate's
        // openNewWindow), so removing one closes the tabs it opened here and
        // leaves the window and the user's tabs alone.
        #[unsafe(method(closeForWebExtensionContext:completionHandler:))]
        fn close(
            &self,
            context: &WKWebExtensionContext,
            completion: &DynBlock<dyn Fn(*mut NSError)>,
        ) {
            let completion = completion.copy();
            let Some(shared) = self.ivars().shared.upgrade() else {
                completion.call((Retained::as_ptr(&error("The browser is closing.")).cast_mut(),));
                return;
            };
            // SAFETY: WebKit passes `context` as a valid extension context for
            // the duration of this delegate callback.
            let extension = unsafe { context.uniqueIdentifier() }.to_string();
            let tabs = {
                let graph = shared.graph.borrow();
                shared.opened.take_in(&extension, self.id(), |tab| {
                    graph
                        .tab(tab)
                        .and_then(|tab| tab.window())
                        .map(|window| window.id())
                })
            };
            if tabs.is_empty() {
                let error = error("Only windows the extension opened can be closed.");
                completion.call((Retained::as_ptr(&error).cast_mut(),));
                return;
            }
            let remaining = Rc::new(Cell::new(tabs.len()));
            let failure: Rc<RefCell<Option<String>>> = Rc::default();
            for tab in tabs {
                let completion = completion.clone();
                let remaining = remaining.clone();
                let failure = failure.clone();
                let weak = Rc::downgrade(&shared);
                let extension = extension.clone();
                shared.host().tab_request(
                    TabRequest::Close { tab },
                    Box::new(move |result| {
                        if let Err(message) = result {
                            // Still open, so a later windows.remove can retry it.
                            if let Some(shared) = weak.upgrade() {
                                shared.opened.record(&extension, tab);
                            }
                            failure.borrow_mut().get_or_insert(message);
                        }
                        remaining.set(remaining.get() - 1);
                        if remaining.get() > 0 {
                            return;
                        }
                        match failure.borrow_mut().take() {
                            None => completion.call((std::ptr::null_mut(),)),
                            Some(message) => {
                                let error = error(&message);
                                completion.call((Retained::as_ptr(&error).cast_mut(),));
                            }
                        }
                    }),
                );
            }
        }
    }
);

impl Window {
    pub(crate) fn new(mtm: MainThreadMarker, id: u64, shared: RcWeak<Shared>) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(WindowIvars {
            id,
            shared,
            tabs: RefCell::new(Vec::new()),
            active: Cell::new(None),
            frame: Cell::new((0.0, 0.0, 0.0, 0.0)),
        });
        unsafe { msg_send![super(this), init] }
    }

    pub(crate) fn id(&self) -> u64 {
        self.ivars().id
    }

    pub(crate) fn active_id(&self) -> Option<u64> {
        self.ivars().active.get()
    }

    pub(crate) fn active_tab(&self) -> Option<Retained<Tab>> {
        self.tab(self.active_id()?)
    }

    pub(crate) fn tabs(&self) -> Vec<Retained<Tab>> {
        self.ivars().tabs.borrow().clone()
    }

    pub(crate) fn tab(&self, id: u64) -> Option<Retained<Tab>> {
        self.ivars()
            .tabs
            .borrow()
            .iter()
            .find(|tab| tab.id() == id)
            .cloned()
    }

    pub(crate) fn set_tabs(&self, tabs: Vec<Retained<Tab>>, active: Option<u64>) {
        for (index, tab) in tabs.iter().enumerate() {
            tab.place(self, index);
        }
        *self.ivars().tabs.borrow_mut() = tabs;
        self.ivars().active.set(active);
    }

    pub(crate) fn set_frame(&self, frame: (f64, f64, f64, f64)) {
        self.ivars().frame.set(frame);
    }
}

/// Keeps the published graph reachable while WebKit holds only weak views of
/// it.
#[derive(Default)]
pub(crate) struct Graph {
    pub(crate) windows: Vec<Retained<Window>>,
    pub(crate) focused: Option<u64>,
}

impl Graph {
    pub(crate) fn tab(&self, id: u64) -> Option<Retained<Tab>> {
        self.windows.iter().find_map(|window| window.tab(id))
    }

    pub(crate) fn window(&self, id: u64) -> Option<Retained<Window>> {
        self.windows
            .iter()
            .find(|window| window.id() == id)
            .cloned()
    }

    pub(crate) fn focused_window(&self) -> Option<Retained<Window>> {
        self.focused
            .and_then(|id| self.window(id))
            .or_else(|| self.windows.first().cloned())
    }
}
