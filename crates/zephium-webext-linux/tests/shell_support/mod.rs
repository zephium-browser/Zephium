//! A minimal browser shell over one profile runtime for native tests:
//! ordinary tabs it creates, closes and navigates on an extension's behalf,
//! the extension surface it publishes, and the requests it settles.

use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::io::Write;
use std::rc::Rc;

use serde_json::Value;
use webkit2gtk::{LoadEvent, WebViewExt};
use zephium_core::extension_data::ExtensionDataReply;
use zephium_core::extensions::{
    ExtensionBrowserRequestAction as Action, ExtensionBrowserRequestRejection,
    ExtensionBrowserRequestResult, ExtensionBrowserRequestSettlement as Settlement,
    ExtensionBrowserSurface, ExtensionBrowserSurfaceGeneration, ExtensionBrowserTab,
    ExtensionBrowserWindow,
};
use zephium_core::ids::{ExtensionInstallId, ItemId, ProfileId};
use zephium_webext_linux::runtime::{
    BrowserCall, BrowserSettlement, ProfileRuntime, RuntimeEngine,
};
use zephium_webext_linux::scheme::Requester;

pub const INSTALL: u128 = 77;
pub const PROFILE: u128 = 11;
const WINDOW: u64 = 1;

pub struct Tab {
    pub view: webkit2gtk::WebView,
    attached: zephium_webext_linux::bridge::BridgeViewId,
    pub url: Option<url::Url>,
    pub title: String,
    pub loading: bool,
}

#[derive(Default)]
pub struct Shell {
    pub tabs: BTreeMap<ItemId, Tab>,
    pub order: Vec<ItemId>,
    pub active: Option<ItemId>,
    generation: u64,
    next: u128,
    work: Vec<BrowserCall>,
}

pub struct Engine {
    pub extension: String,
    pub shell: Rc<RefCell<Shell>>,
}

impl RuntimeEngine for Engine {
    fn requester(&self, view: &webkit2gtk::WebView) -> Option<Requester> {
        let uri = view.uri().and_then(|uri| url::Url::parse(&uri).ok())?;
        let own = uri.scheme() == "chrome-extension" && uri.host_str() == Some(&self.extension);
        Some(Requester::Tab {
            committed_top_level: uri,
            extension_install: own.then(|| ExtensionInstallId::from(INSTALL)),
        })
    }

    fn browser_call(&self, call: BrowserCall) {
        self.shell.borrow_mut().work.push(call);
    }
}

pub fn emit(line: &Value) {
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "ZCH {line}");
    let _ = out.flush();
}

fn surface(shell: &mut Shell) -> ExtensionBrowserSurface {
    shell.generation += 1;
    let tabs: Vec<ExtensionBrowserTab> = shell
        .order
        .iter()
        .filter_map(|id| {
            let tab = shell.tabs.get(id)?;
            let url = tab
                .url
                .clone()
                .filter(zephium_core::navigation::is_browser_target);
            ExtensionBrowserTab::from_snapshot(
                None,
                *id,
                true,
                &zephium_core::item::sanitize_page_title(&tab.title),
                url.as_ref(),
                tab.loading,
                false,
            )
            .ok()
        })
        .collect();
    let active = shell
        .active
        .filter(|id| tabs.iter().any(|tab| tab.id() == *id));
    ExtensionBrowserSurface::new(
        ProfileId::from(PROFILE),
        ExtensionBrowserSurfaceGeneration::new(shell.generation).unwrap(),
        Some(WINDOW),
        vec![ExtensionBrowserWindow::new(WINDOW, false, active, tabs).unwrap()],
    )
    .unwrap()
}

pub struct Host {
    pub context: webkit2gtk::WebContext,
    pub runtime: ProfileRuntime,
    pub shell: Rc<RefCell<Shell>>,
    pub dirty: Rc<Cell<bool>>,
}

impl Host {
    pub fn open(&self, url: &str, active: bool) -> ItemId {
        let view = webkit2gtk::WebView::with_context(&self.context);
        let id = {
            let mut shell = self.shell.borrow_mut();
            shell.next += 1;
            ItemId::from(1000 + shell.next)
        };
        let attached = self.runtime.attach_tab(id, &view).expect("attach tab");
        let (shell, dirty) = (Rc::downgrade(&self.shell), self.dirty.clone());
        let (titles, retitled) = (Rc::downgrade(&self.shell), self.dirty.clone());
        view.connect_title_notify(move |view| {
            if let Some(shell) = titles.upgrade() {
                if let Some(tab) = shell.borrow_mut().tabs.get_mut(&id) {
                    tab.title = view.title().map(|t| t.to_string()).unwrap_or_default();
                }
                retitled.set(true);
            }
        });
        view.connect_load_changed(move |view, event| {
            let Some(shell) = shell.upgrade() else {
                return;
            };
            if let Some(tab) = shell.borrow_mut().tabs.get_mut(&id) {
                match event {
                    LoadEvent::Started => tab.loading = true,
                    LoadEvent::Committed => {
                        tab.url = view.uri().and_then(|uri| url::Url::parse(&uri).ok());
                    }
                    LoadEvent::Finished => {
                        tab.loading = false;
                        tab.title = view.title().map(|t| t.to_string()).unwrap_or_default();
                    }
                    _ => {}
                }
            }
            dirty.set(true);
        });
        {
            let mut shell = self.shell.borrow_mut();
            shell.tabs.insert(
                id,
                Tab {
                    view: view.clone(),
                    attached,
                    url: None,
                    title: String::new(),
                    loading: true,
                },
            );
            shell.order.push(id);
            if active || shell.active.is_none() {
                shell.active = Some(id);
            }
        }
        self.publish();
        view.load_uri(url);
        id
    }

    pub fn publish(&self) {
        let surface = surface(&mut self.shell.borrow_mut());
        self.runtime.publish_surface(surface);
    }

    pub fn close(&self, id: ItemId) -> bool {
        let tab = {
            let mut shell = self.shell.borrow_mut();
            shell.order.retain(|tab| *tab != id);
            if shell.active == Some(id) {
                shell.active = shell.order.last().copied();
            }
            shell.tabs.remove(&id)
        };
        let Some(tab) = tab else {
            return false;
        };
        self.runtime.detach_tab(tab.attached);
        tab.view.terminate_web_process();
        self.publish();
        true
    }

    fn mutate(&self, action: Action) -> Settlement {
        let tab_view = |id: ItemId| {
            self.shell
                .borrow()
                .tabs
                .get(&id)
                .map(|tab| tab.view.clone())
        };
        let applied = Settlement::Applied(ExtensionBrowserRequestResult::Complete);
        let missing = Settlement::Rejected(ExtensionBrowserRequestRejection::InvalidScope);
        match action {
            Action::CreateTab { url, active, .. } => {
                let id = self.open(url.as_deref().unwrap_or("about:blank"), active);
                Settlement::Applied(ExtensionBrowserRequestResult::CreatedTab(id))
            }
            Action::CloseTab { tab } | Action::CloseTabIfPristine { tab } => {
                if self.close(tab) {
                    applied
                } else {
                    missing
                }
            }
            Action::ActivateTab { tab } => {
                if self.shell.borrow().tabs.contains_key(&tab) {
                    self.shell.borrow_mut().active = Some(tab);
                    self.publish();
                    applied
                } else {
                    missing
                }
            }
            Action::LoadTabUrl { tab, url } => match tab_view(tab) {
                Some(view) => {
                    view.load_uri(&url);
                    applied
                }
                None => missing,
            },
            Action::ReloadTab { tab } => match tab_view(tab) {
                Some(view) => {
                    view.reload();
                    applied
                }
                None => missing,
            },
            Action::GoBack { tab } => match tab_view(tab) {
                Some(view) => {
                    view.go_back();
                    applied
                }
                None => missing,
            },
            Action::GoForward { tab } => match tab_view(tab) {
                Some(view) => {
                    view.go_forward();
                    applied
                }
                None => missing,
            },
            _ => Settlement::Rejected(ExtensionBrowserRequestRejection::Unsupported),
        }
    }

    pub fn work(&mut self) {
        loop {
            let work = std::mem::take(&mut self.shell.borrow_mut().work);
            if work.is_empty() {
                break;
            }
            for call in work {
                if std::env::var_os("ZEPHIUM_CONFORMANCE_DEBUG").is_some() {
                    eprintln!("shell call {call:?}");
                }
                match call {
                    BrowserCall::Mutate { request, action } => {
                        let settlement = self.mutate(action);
                        self.runtime
                            .settle_browser(request, BrowserSettlement::Mutation(settlement));
                    }
                    BrowserCall::Data { request, .. } => self.runtime.settle_browser(
                        request,
                        BrowserSettlement::Data(ExtensionDataReply::Unavailable),
                    ),
                    BrowserCall::Prompt { request, .. } => self
                        .runtime
                        .settle_browser(request, BrowserSettlement::Prompt(false)),
                    BrowserCall::Capture { request, .. } => self.runtime.settle_browser(
                        request,
                        BrowserSettlement::Capture(Err("Capture is not available.".into())),
                    ),
                    _ => {}
                }
            }
        }
        if self.dirty.replace(false) {
            self.publish();
        }
    }
}
