//! Authenticated browser facts and deferred adapter requests.
//!
//! The adapter resolves Chrome integer IDs to Shell identities. Snapshots contain
//! ordinary browser tabs only: Work, previews, launcher and privileged chrome
//! never enter this graph. Every request retains its Sender and runtime/document
//! generations; adapters recheck them at execution and settle only actual results.

use serde_json::Value;
use url::Url;
use zephium_core::extensions::{ExtensionBrowserSurface, ExtensionBrowserTab};
use zephium_core::ids::{ExtensionInstallId, ItemId, ProfileId};

use crate::context::{DocumentGeneration, FrameId, Sender};
use crate::dispatch::{Handled, Invocation};
use crate::pattern::DocumentOrigin;
use crate::permissions::{InstallPermissions, Target};
use crate::ApiError;

/// Private-profile execution policy from the admitted manifest.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum IncognitoMode {
    /// One regular instance; private access still requires explicit approval.
    #[default]
    Spanning,
    /// Separate ephemeral instance for each private profile.
    Split,
    /// No private access, even with a persisted approval.
    NotAllowed,
}

/// Immutable installation facts supplied to synchronous bootstrap bindings.
#[derive(Clone, Debug)]
pub struct ExtensionInfo {
    /// Original, validated manifest, including localized fields.
    pub manifest: Value,
    /// Manifest private-mode declaration.
    pub incognito_mode: IncognitoMode,
    /// Explicit browser setting allowing private access.
    pub incognito_allowed: bool,
    /// Platform facts using Chrome's os/arch/nacl_arch names.
    pub platform: Value,
    /// Live, same-install context descriptors for runtime.getContexts.
    pub contexts: Vec<Value>,
    /// Registered command descriptions and actual platform shortcuts.
    pub commands: Vec<Value>,
    /// Browser-owned installation metadata for management.getSelf.
    pub management: Value,
}

impl Default for ExtensionInfo {
    fn default() -> Self {
        Self {
            manifest: serde_json::json!({"manifest_version": 3}),
            incognito_mode: IncognitoMode::NotAllowed,
            incognito_allowed: false,
            platform: platform_info(),
            contexts: Vec::new(),
            commands: Vec::new(),
            management: serde_json::json!({}),
        }
    }
}

fn platform_info() -> Value {
    let os = match std::env::consts::OS {
        "windows" => "win",
        "macos" => "mac",
        os => os,
    };
    let arch = match std::env::consts::ARCH {
        "aarch64" => "arm64",
        "x86_64" => "x86-64",
        "x86" => "x86-32",
        arch => arch,
    };
    let mut info = serde_json::json!({"os":os, "arch":arch});
    if arch == "arm64" {
        info["nacl_arch"] = "arm".into();
    } else if matches!(arch, "arm" | "x86-32" | "x86-64" | "mips" | "mips64") {
        info["nacl_arch"] = arch.into();
    }
    info
}

/// Ordinary tab projection, retaining the Shell's authoritative logical record.
#[derive(Clone, Debug)]
pub struct BrowserTab {
    /// Chrome-visible positive integer, stable until this tab closes.
    pub id: i32,
    /// Chrome-visible owning window.
    pub window_id: i32,
    /// Position in the window.
    pub index: usize,
    /// Shell tab record; never a synthetic Work or browser-chrome view.
    pub tab: ExtensionBrowserTab,
    /// Selected tab in its window.
    pub active: bool,
    /// Multi-selection state.
    pub highlighted: bool,
    /// Private-profile membership.
    pub incognito: bool,
    /// Main document generation, including discarded tabs' last document.
    pub document: DocumentGeneration,
    /// Native document identity exposed by Chrome's documentId APIs.
    pub document_id: String,
    /// Native extension-guest URL when the Shell record intentionally omits it.
    pub native_url: Option<Url>,
    /// Pending navigation, if any.
    pub pending_url: Option<String>,
    /// Browser-approved favicon URL.
    pub fav_icon_url: Option<String>,
    /// Effective zoom factor.
    pub zoom: f64,
    /// Whether this tab is eligible for automatic discard.
    pub auto_discardable: bool,
    /// Opener Chrome tab identity.
    pub opener_tab_id: Option<i32>,
    /// Most recent activation in Unix milliseconds.
    pub last_accessed: f64,
}

impl BrowserTab {
    /// Native committed address, falling back to the ordinary Shell record.
    pub fn committed_url(&self) -> Option<&str> {
        self.native_url
            .as_ref()
            .map(Url::as_str)
            .or_else(|| self.tab.url())
    }

    /// Current URL parsed from the Shell record.
    pub fn url(&self) -> Result<Url, ApiError> {
        Url::parse(self.committed_url().unwrap_or("about:blank"))
            .map_err(|_| ApiError::cannot_access_page())
    }

    /// Applies content access checks, including activeTab and file access.
    pub fn check_access(
        &self,
        sender: &Sender,
        permissions: &InstallPermissions,
    ) -> Result<(), ApiError> {
        let url = self.url()?;
        let origin = DocumentOrigin::of_url(&url);
        permissions.check_target(
            sender,
            &Target {
                profile: sender.profile(),
                tab: self.tab.id(),
                frame: FrameId::MAIN,
                main_document: self.document,
                url: &url,
                origin: &origin,
            },
        )
    }
}

/// Window presentation facts joined to a Shell surface by the adapter.
#[derive(Clone, Debug)]
pub struct BrowserWindow {
    /// Chrome integer identity; adapters retain the core WindowId mapping.
    pub id: i32,
    /// Core window identity.
    pub native_id: zephium_core::ids::WindowId,
    /// Private window.
    pub incognito: bool,
    /// Currently focused.
    pub focused: bool,
    /// Chrome type, normally normal or popup.
    pub kind: String,
    /// Chrome state: normal, minimized, maximized or fullscreen.
    pub state: String,
    /// Window coordinates and dimensions in logical pixels.
    pub bounds: [i32; 4],
    /// Native always-on-top state.
    pub always_on_top: bool,
}

/// One caller-scoped, generation-consistent browser observation.
#[derive(Clone, Debug)]
pub struct BrowserSnapshot {
    /// Profile owning every record.
    pub profile: ProfileId,
    /// Shell surface generation used to fence asynchronous mutations.
    pub generation: zephium_core::extensions::ExtensionBrowserSurfaceGeneration,
    /// Ordinary tabs only, with stable Chrome identity mapping.
    pub tabs: Vec<BrowserTab>,
    /// Ordinary windows in this profile.
    pub windows: Vec<BrowserWindow>,
    /// Caller-associated or focused Chrome window.
    pub current_window: Option<i32>,
    /// Last focused Chrome window even while the app is unfocused.
    pub last_focused_window: Option<i32>,
}

impl BrowserSnapshot {
    /// Builds a projection from the existing Shell surface and stable ID maps.
    /// Missing mappings fail closed instead of inventing identities.
    pub fn from_surface(
        surface: &ExtensionBrowserSurface,
        tab_id: impl Fn(ItemId) -> Option<i32>,
        window_id: impl Fn(zephium_core::ids::WindowId) -> Option<i32>,
        document: impl Fn(ItemId) -> Option<(DocumentGeneration, String)>,
    ) -> Result<Self, ApiError> {
        let mut tabs = Vec::new();
        let mut windows = Vec::new();
        for window in surface.windows() {
            let id = window_id(window.id())
                .filter(|id| *id > 0)
                .ok_or_else(ApiError::context_invalidated)?;
            windows.push(BrowserWindow {
                id,
                native_id: window.id(),
                incognito: window.is_private(),
                focused: surface.focused() == Some(window.id()),
                kind: "normal".into(),
                state: "normal".into(),
                bounds: [0, 0, 0, 0],
                always_on_top: false,
            });
            for (index, tab) in window.tabs().iter().enumerate() {
                let id = tab_id(tab.id())
                    .filter(|id| *id > 0)
                    .ok_or_else(ApiError::context_invalidated)?;
                let (document, document_id) =
                    document(tab.id()).ok_or_else(ApiError::context_invalidated)?;
                tabs.push(BrowserTab {
                    id,
                    window_id: windows.last().map_or(0, |w| w.id),
                    index,
                    tab: tab.clone(),
                    active: window.active() == Some(tab.id()),
                    highlighted: window.active() == Some(tab.id()),
                    incognito: window.is_private(),
                    document,
                    document_id,
                    native_url: None,
                    pending_url: None,
                    fav_icon_url: None,
                    zoom: 1.0,
                    auto_discardable: true,
                    opener_tab_id: None,
                    last_accessed: 0.0,
                });
            }
        }
        let focused = surface.focused().and_then(window_id);
        let snapshot = Self {
            profile: surface.profile(),
            generation: surface.generation(),
            tabs,
            windows,
            current_window: focused,
            last_focused_window: focused,
        };
        snapshot.validate()?;
        Ok(snapshot)
    }

    fn validate(&self) -> Result<(), ApiError> {
        use std::collections::HashSet;
        use zephium_core::extensions::{MAX_EXTENSION_BROWSER_TABS, MAX_EXTENSION_BROWSER_WINDOWS};
        if self.tabs.len() > MAX_EXTENSION_BROWSER_TABS
            || self.windows.len() > MAX_EXTENSION_BROWSER_WINDOWS
        {
            return Err(ApiError::too_many_pending());
        }
        let mut windows = HashSet::new();
        let mut native_windows = HashSet::new();
        for window in &self.windows {
            if window.id <= 0
                || window.native_id == 0
                || !windows.insert(window.id)
                || !native_windows.insert(window.native_id)
            {
                return Err(ApiError::context_invalidated());
            }
        }
        let mut tabs = HashSet::new();
        let mut native_tabs = HashSet::new();
        let mut documents = HashSet::new();
        for tab in &self.tabs {
            if tab.id <= 0
                || !tabs.insert(tab.id)
                || !native_tabs.insert(tab.tab.id())
                || !windows.contains(&tab.window_id)
                || tab.document_id.is_empty()
                || tab.document_id.len() > 256
                || !documents.insert(&tab.document_id)
                || !tab.zoom.is_finite()
                || tab.zoom <= 0.0
            {
                return Err(ApiError::context_invalidated());
            }
        }
        if [self.current_window, self.last_focused_window]
            .into_iter()
            .flatten()
            .any(|id| !windows.contains(&id))
        {
            return Err(ApiError::context_invalidated());
        }
        Ok(())
    }

    /// Rejects foreign profiles and forbidden private surfaces before enumeration.
    pub fn authorize(&self, sender: &Sender, info: &ExtensionInfo) -> Result<(), ApiError> {
        self.validate()?;
        if self.profile != sender.profile()
            || self.tabs.iter().any(|t| t.incognito != sender.incognito())
            || self
                .windows
                .iter()
                .any(|w| w.incognito != sender.incognito())
            || (sender.incognito()
                && (!info.incognito_allowed || info.incognito_mode == IncognitoMode::NotAllowed))
        {
            return Err(ApiError::cannot_access_page());
        }
        Ok(())
    }

    /// Resolves an exposed tab ID without leaking other profiles or surfaces.
    pub fn tab(&self, id: i32) -> Result<&BrowserTab, ApiError> {
        self.tabs
            .iter()
            .find(|tab| tab.id == id)
            .ok_or_else(|| ApiError::other(format!("No tab with id: {id}.")))
    }

    /// Resolves a window ID, including WINDOW_ID_CURRENT (-2).
    pub fn window(&self, id: i32) -> Result<&BrowserWindow, ApiError> {
        let id = if id == -2 {
            self.current_window.unwrap_or(-1)
        } else {
            id
        };
        self.windows
            .iter()
            .find(|window| window.id == id)
            .ok_or_else(|| ApiError::other(format!("No window with id: {id}.")))
    }
}

/// Typed requests crossing the browser adapter boundary.
#[derive(Clone, Debug)]
pub enum BrowserRequest {
    /// Shell-compatible logical tab mutations.
    Core(zephium_core::extensions::ExtensionBrowserRequestAction),
    /// Existing durable data-call contract.
    Data(zephium_core::extension_data::ExtensionDataCall),
    /// Additional tab operations and mutation reply shaping.
    Tabs(crate::api::tabs::Request),
    /// Native window management.
    Windows(crate::api::windows::Request),
    /// Action state and popup presentation.
    Action(crate::api::action::Request),
    /// Permission prompting, durable grants and revocation effects.
    Permissions(crate::api::permissions::Request),
    /// Runtime lifecycle operations.
    Runtime(crate::api::runtime::Request),
    /// Script/CSS injection against exact authorized documents.
    Scripting(crate::api::scripting::Request),
    /// Native notification operations.
    Notifications(crate::api::notifications::Request),
    /// Desktop service requests.
    Service(ServiceRequest),
}

/// Small native desktop services with no Shell mutation counterpart.
#[derive(Clone, Debug)]
pub enum ServiceRequest {
    /// Query active/idle/locked using the supplied threshold in seconds.
    QueryIdle(u32),
    /// Change the install's idle observation interval in seconds.
    SetIdleInterval(u32),
    /// Return Chrome fontId/displayName records from the platform font list.
    FontList,
    /// Search using the profile's configured engine and typed disposition.
    Search(crate::api::search::Request),
}

/// The sole browser-facing interface used by namespace handlers.
///
/// No thread, executor, Send or Sync requirement is imposed. `request` may
/// defer through Invocation::defer and retain its Responder until core/native
/// settlement. Admission must never be reported as durable or native success.
pub trait BrowserPort {
    /// Returns same-profile ordinary browser facts. Must omit all privileged and Work views.
    fn snapshot(&self, sender: &Sender) -> Result<BrowserSnapshot, ApiError>;
    /// Returns current installation/bootstrap facts for the authenticated generation.
    fn extension_info(&self, sender: &Sender) -> Result<ExtensionInfo, ApiError>;
    /// Executes or queues a validated typed request, rechecking identity and document generations.
    /// Data replies must be converted with the namespace's reply helper before settlement.
    /// Shell history-add and restore admissions are not completion evidence: wait
    /// for the store write/native reopen, then supply its authoritative result.
    fn request(&mut self, request: BrowserRequest, invocation: Invocation<'_>) -> Handled;
    /// Current ordinary frames; the adapter omits extension-owned and privileged frames.
    fn frames(
        &self,
        _sender: &Sender,
        _tab: ItemId,
    ) -> Result<Vec<crate::api::web_navigation::BrowserFrame>, ApiError> {
        Err(unsupported())
    }
    /// Native, non-forgeable gesture state for this exact invocation context.
    fn user_gesture(&self, _sender: &Sender) -> bool {
        false
    }
    /// Validates an extension-relative packaged resource without exposing file paths.
    fn validate_resource(&self, _sender: &Sender, _path: &str) -> Result<(), ApiError> {
        Err(ApiError::other("Could not load file."))
    }
    /// Commits DNR-owned badge text through the native action model. None clears the
    /// DNR overlay and restores extension action text; it does not clear that text.
    fn set_dnr_badge(
        &mut self,
        _install: ExtensionInstallId,
        _tab_id: i32,
        _text: Option<&str>,
    ) -> Result<(), ApiError> {
        Err(unsupported())
    }
    /// Unix time for history defaults and event timestamps.
    fn now_ms(&self) -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| u64::try_from(duration.as_millis()).unwrap_or(u64::MAX))
            .unwrap_or(0)
    }
}

/// Obtains and authorizes one consistent browser snapshot.
pub fn snapshot(host: &impl BrowserPort, sender: &Sender) -> Result<BrowserSnapshot, ApiError> {
    let snapshot = host.snapshot(sender)?;
    snapshot.authorize(sender, &host.extension_info(sender)?)?;
    Ok(snapshot)
}

/// Chrome's generic unavailable-function failure.
pub fn unsupported() -> ApiError {
    ApiError::other("This function is not supported.")
}

#[cfg(test)]
pub(crate) mod fake;

#[cfg(test)]
mod tests;
