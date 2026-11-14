//! Extension-owned views: creation, the registry the package scheme asks
//! who a request comes from, and the options and popup pages the caller
//! embeds. Pages in ordinary tabs are the engine's views; see
//! [`crate::runtime::ProfileRuntime::extension_tab_scripts`].

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::{Rc, Weak};

use glib::prelude::*;
use webkit2gtk::{SettingsExt, UserContentManagerExt};
use zephium_core::ids::ExtensionInstallId;
use zephium_webext_runtime::context::ViewKind;

use crate::bridge::BridgeViewId;
use crate::scheme::{RequestHost, Requester};

/// A script for an extension page's default world, injected at document
/// start into the top frame of the listed URLs only.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PageScript {
    /// The script source.
    pub source: String,
    /// WebKit URL patterns the script runs on.
    pub allow: Vec<String>,
    /// URL patterns it never runs on (manifest sandbox pages).
    pub block: Vec<String>,
}

impl PageScript {
    /// The WebKit user script, for a tab's `UserContentManager`.
    pub fn user_script(&self) -> webkit2gtk::UserScript {
        self.user_script_in(webkit2gtk::UserContentInjectedFrames::TopFrame)
    }

    /// The same script for the given frames; ordinary tabs also host
    /// extension pages framed by websites.
    pub fn user_script_in(
        &self,
        frames: webkit2gtk::UserContentInjectedFrames,
    ) -> webkit2gtk::UserScript {
        let allow: Vec<&str> = self.allow.iter().map(String::as_str).collect();
        let block: Vec<&str> = self.block.iter().map(String::as_str).collect();
        webkit2gtk::UserScript::new(
            &self.source,
            frames,
            webkit2gtk::UserScriptInjectionTime::Start,
            &allow,
            &block,
        )
    }
}

/// A view in `context`, never mapped by the runtime itself. The clipboard
/// is reachable only when the caller authorised it.
pub(crate) fn create_view(
    context: &webkit2gtk::WebContext,
    clipboard: bool,
    script: Option<&PageScript>,
) -> webkit2gtk::WebView {
    let settings = webkit2gtk::Settings::new();
    settings.set_javascript_can_access_clipboard(clipboard);
    settings.set_enable_developer_extras(false);
    let content = webkit2gtk::UserContentManager::new();
    if let Some(script) = script {
        content.add_script(&script.user_script());
    }
    webkit2gtk::WebView::builder()
        .web_context(context)
        .settings(&settings)
        .user_content_manager(&content)
        .build()
}

pub(crate) struct Owned {
    pub install: ExtensionInstallId,
    pub generation: u64,
    pub view: webkit2gtk::WebView,
    pub bridge: Option<BridgeViewId>,
    pub handlers: Vec<glib::SignalHandlerId>,
}

impl Owned {
    /// Disconnects the runtime's own signal handlers.
    pub(crate) fn disconnect(&mut self) {
        for handler in self.handlers.drain(..) {
            self.view.disconnect(handler);
        }
    }
}

/// Every view the runtime created or attached as extension-owned.
#[derive(Default)]
pub(crate) struct Views {
    pub owned: RefCell<HashMap<(ExtensionInstallId, ViewKindKey), Owned>>,
    pub pages: RefCell<Vec<PageEntry>>,
    pub ended: RefCell<Vec<(ExtensionInstallId, glib::WeakRef<webkit2gtk::WebView>)>>,
}

pub(crate) struct PageEntry {
    pub install: ExtensionInstallId,
    pub serial: u64,
    pub view: glib::WeakRef<webkit2gtk::WebView>,
    pub bridge: BridgeViewId,
}

/// Background and offscreen views are singletons per install.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum ViewKindKey {
    Background,
    Offscreen,
}

impl Views {
    pub(crate) fn owner_of(&self, view: &webkit2gtk::WebView) -> Option<ExtensionInstallId> {
        let owned = self
            .owned
            .borrow()
            .values()
            .find(|owned| &owned.view == view)
            .map(|owned| owned.install);
        owned.or_else(|| {
            self.pages
                .borrow()
                .iter()
                .find(|page| page.view.upgrade().as_ref() == Some(view))
                .map(|page| page.install)
        })
    }

    /// Remembers a view that left the runtime, for teardown drains.
    pub(crate) fn ended(&self, install: ExtensionInstallId, view: &webkit2gtk::WebView) {
        let mut ended = self.ended.borrow_mut();
        ended.retain(|(_, view)| view.upgrade().is_some());
        ended.push((install, view.downgrade()));
    }

    pub(crate) fn alive(&self, install: Option<ExtensionInstallId>) -> usize {
        let ended = self
            .ended
            .borrow()
            .iter()
            .filter(|(owner, view)| install.is_none_or(|i| i == *owner) && view.upgrade().is_some())
            .count();
        let pages = self
            .pages
            .borrow()
            .iter()
            .filter(|page| {
                install.is_none_or(|i| i == page.install) && page.view.upgrade().is_some()
            })
            .count();
        ended + pages
    }
}

/// The scheme's view facts: runtime-owned views are the runtime's; every
/// other view is the engine's to answer for.
pub(crate) struct ViewRequests {
    pub views: Rc<Views>,
    pub engine: Weak<dyn crate::runtime::RuntimeEngine>,
}

impl RequestHost for ViewRequests {
    fn requester(&self, view: &webkit2gtk::WebView) -> Option<Requester> {
        if let Some(install) = self.views.owner_of(view) {
            return Some(Requester::ExtensionOwned { install });
        }
        self.engine.upgrade()?.requester(view)
    }
}

/// What a caller embeds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PageKind {
    /// `options_ui.page` / `options_page`.
    Options,
    /// The action popup.
    Popup,
}

impl PageKind {
    pub(crate) fn view_kind(self) -> ViewKind {
        match self {
            Self::Options => ViewKind::Options,
            Self::Popup => ViewKind::Popup,
        }
    }
}

/// An options or popup page the runtime created and attached. The caller
/// embeds [`Self::view`]; dropping this detaches the page from the bridge
/// (its contexts end), after which the caller drops the widget.
pub struct ExtensionPage {
    pub(crate) view: webkit2gtk::WebView,
    pub(crate) install: ExtensionInstallId,
    pub(crate) serial: u64,
    pub(crate) detach: Option<Box<dyn FnOnce(u64)>>,
}

impl ExtensionPage {
    /// The page's view, not yet in any container.
    pub fn view(&self) -> &webkit2gtk::WebView {
        &self.view
    }

    /// The install.
    pub fn install(&self) -> ExtensionInstallId {
        self.install
    }
}

impl Drop for ExtensionPage {
    fn drop(&mut self) {
        if let Some(detach) = self.detach.take() {
            detach(self.serial);
        }
    }
}

impl std::fmt::Debug for ExtensionPage {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ExtensionPage")
            .field("install", &self.install)
            .finish_non_exhaustive()
    }
}

/// Why a page view could not be opened.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ViewError {
    /// The install is not running in this profile.
    UnknownInstall,
    /// The manifest declares no such page.
    NoPage,
    /// The path is not one of the extension's pages.
    InvalidPath,
    /// The bridge refused the view.
    Attach(String),
    /// The profile runtime was retired.
    Retired,
}

impl std::fmt::Display for ViewError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownInstall => formatter.write_str("extension is not running"),
            Self::NoPage => formatter.write_str("extension declares no such page"),
            Self::InvalidPath => formatter.write_str("invalid extension page path"),
            Self::Attach(reason) => write!(formatter, "view could not be attached: {reason}"),
            Self::Retired => formatter.write_str("profile runtime retired"),
        }
    }
}

impl std::error::Error for ViewError {}
