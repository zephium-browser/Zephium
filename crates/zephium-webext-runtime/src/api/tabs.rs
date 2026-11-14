//! Chrome tab management over caller-scoped browser snapshots.

use std::sync::Arc;

use serde::Deserialize;
use serde_json::{json, Map, Value};
use url::Url;
use zephium_core::extensions::ExtensionBrowserRequestAction as CoreAction;
use zephium_core::injection::MatchPattern;

use crate::browser::{self, BrowserPort, BrowserRequest, BrowserSnapshot, BrowserTab};
use crate::context::{DocumentGeneration, Sender};
use crate::dispatch::{ApiTable, Handled, Invocation, RegistrationError};
use crate::pattern::UrlGlob;
use crate::permissions::InstallPermissions;
use crate::ApiError;

pub(crate) const METHODS: &[&str] = &[
    "tabs.query",
    "tabs.get",
    "tabs.getCurrent",
    "tabs.getSelected",
    "tabs.getAllInWindow",
    "tabs.create",
    "tabs.update",
    "tabs.remove",
    "tabs.reload",
    "tabs.duplicate",
    "tabs.move",
    "tabs.highlight",
    "tabs.group",
    "tabs.ungroup",
    "tabs.createSplit",
    "tabs.unsplit",
    "tabs.goBack",
    "tabs.goForward",
    "tabs.captureVisibleTab",
    "tabs.detectLanguage",
    "tabs.getZoom",
    "tabs.setZoom",
    "tabs.getZoomSettings",
    "tabs.setZoomSettings",
    "tabs.discard",
];

/// Tab creation options, after generated schema validation.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateProperties {
    /// Destination Chrome window.
    pub window_id: Option<i32>,
    /// Desired tab-strip position, clamped by the adapter.
    pub index: Option<usize>,
    /// Absolute, validated navigation target.
    pub url: Option<String>,
    /// Whether the tab becomes active; defaults to true.
    pub active: Option<bool>,
    /// Deprecated alias for active.
    pub selected: Option<bool>,
    /// Initial pinned state.
    pub pinned: Option<bool>,
    /// Same-window opener.
    pub opener_tab_id: Option<i32>,
    /// Unsupported split-view target.
    pub split_with_tab_id: Option<i32>,
}

/// Supported updates to one ordinary browser tab.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateProperties {
    /// Validated absolute navigation URL.
    pub url: Option<String>,
    /// Active-tab state.
    pub active: Option<bool>,
    /// Multi-selection state.
    pub highlighted: Option<bool>,
    /// Deprecated selection alias.
    pub selected: Option<bool>,
    /// Pinned state.
    pub pinned: Option<bool>,
    /// Audio mute state.
    pub muted: Option<bool>,
    /// Same-window opener.
    pub opener_tab_id: Option<i32>,
    /// Automatic-discard eligibility.
    pub auto_discardable: Option<bool>,
}

/// Browser mutations whose replies settle after native application.
#[derive(Clone, Debug)]
pub enum Request {
    /// Create, then return the new tab with caller-specific redaction.
    Create {
        /// Complete creation options.
        properties: CreateProperties,
        /// Corresponding Shell mutation.
        action: CoreAction,
    },
    /// Apply all options atomically, then return the actual tab.
    Update {
        /// Chrome tab identity.
        tab_id: i32,
        /// Full update, including presentation state missing in the Shell command.
        properties: UpdateProperties,
        /// Existing Shell operations, in application order.
        actions: Vec<CoreAction>,
    },
    /// Close each tab and settle with no value.
    Remove {
        /// Chrome identities.
        tab_ids: Vec<i32>,
        /// Existing Shell close operations.
        actions: Vec<CoreAction>,
    },
    /// Reload and settle with no value.
    Reload {
        /// Chrome identity.
        tab_id: i32,
        /// Force network revalidation.
        bypass_cache: bool,
        /// Existing Shell operation.
        action: CoreAction,
    },
    /// Duplicate history and state, returning the duplicate.
    Duplicate {
        /// Source Chrome identity.
        tab_id: i32,
        /// Existing Shell operation.
        action: CoreAction,
    },
    /// Move tabs, preserving caller order and scalar/array reply shape.
    Move {
        /// Chrome identities.
        tab_ids: Vec<i32>,
        /// Destination Chrome window.
        window_id: i32,
        /// Destination position; -1 appends.
        index: i32,
        /// Whether the caller supplied one integer.
        single: bool,
        /// Same-window Shell operation; cross-window moves need the adapter.
        action: Option<CoreAction>,
    },
    /// Replace selection and activate the first selected index.
    Highlight {
        /// Chrome window.
        window_id: i32,
        /// Chrome tab identities, in supplied index order.
        tab_ids: Vec<i32>,
        /// Existing activation operation.
        action: CoreAction,
    },
    /// Traverse a tab's own navigation list.
    Navigate {
        /// Chrome identity.
        tab_id: i32,
        /// True for forward traversal.
        forward: bool,
        /// Existing Shell operation.
        action: CoreAction,
    },
    /// Capture the authorized active tab into a data URL.
    Capture {
        /// Window identity.
        window_id: i32,
        /// Exact active tab authorized at admission; recheck before capture.
        tab_id: i32,
        /// Main document generation authorized at admission.
        document: DocumentGeneration,
        /// Browser-issued native document identity.
        document_id: String,
        /// png or jpeg; Chrome defaults to jpeg.
        format: String,
        /// JPEG quality from 0 through 100.
        quality: Option<u8>,
    },
    /// Return an ISO language code, or und when unknown.
    DetectLanguage {
        /// Authorized tab.
        tab_id: i32,
        /// Main document generation authorized at admission.
        document: DocumentGeneration,
        /// Browser-issued native document identity.
        document_id: String,
    },
    /// Apply the effective tab zoom and settle with no value.
    SetZoom {
        /// Chrome identity.
        tab_id: i32,
        /// Effective zoom factor; zero resets to the default.
        factor: f64,
        /// Existing Shell operation.
        action: CoreAction,
    },
    /// Discard an inactive tab without allocating a renderer.
    Discard {
        /// Chrome identity.
        tab_id: i32,
    },
}

/// Registers generated tab methods; messaging remains owned by the messaging service.
pub fn register<S: BrowserPort + 'static>(
    table: &mut ApiTable<S>,
) -> Result<(), RegistrationError> {
    for &name in METHODS {
        table.register_generated(name, handle)?;
    }
    Ok(())
}

fn handle<S: BrowserPort>(host: &mut S, invocation: Invocation<'_>) -> Handled {
    let snapshot = match browser::snapshot(host, invocation.sender()) {
        Ok(snapshot) => snapshot,
        Err(error) => return invocation.reply(Err(error)),
    };
    let result = prepare(&snapshot, &invocation);
    match result {
        Ok(Prepared::Reply(reply)) => invocation.reply(Ok(reply)),
        Ok(Prepared::Request(request)) => host.request(BrowserRequest::Tabs(request), invocation),
        Err(error) => invocation.reply(Err(error)),
    }
}

enum Prepared {
    Reply(Value),
    Request(Request),
}

fn prepare(snapshot: &BrowserSnapshot, call: &Invocation<'_>) -> Result<Prepared, ApiError> {
    let args = call.args();
    let permissions = call.permissions();
    let sender = call.sender();
    let request = match call.name() {
        "tabs.query" => {
            return Ok(Prepared::Reply(query(
                snapshot,
                sender,
                &args[0],
                permissions,
            )?))
        }
        "tabs.get" => {
            return Ok(Prepared::Reply(project_for(
                snapshot.tab(integer(&args[0])?)?,
                sender,
                permissions,
            )))
        }
        "tabs.getCurrent" => {
            let tab = sender
                .tab()
                .and_then(|id| snapshot.tabs.iter().find(|tab| tab.tab.id() == id));
            return Ok(Prepared::Reply(
                tab.map_or(Value::Null, |tab| project_for(tab, sender, permissions)),
            ));
        }
        "tabs.getSelected" | "tabs.getAllInWindow" => {
            let window = current_window(snapshot, sender, args.first().and_then(as_integer))?;
            if call.name() == "tabs.getSelected" {
                let tab = snapshot
                    .tabs
                    .iter()
                    .find(|tab| tab.window_id == window && tab.active)
                    .ok_or_else(|| ApiError::other("No selected tab"))?;
                return Ok(Prepared::Reply(project_for(tab, sender, permissions)));
            }
            return Ok(Prepared::Reply(Value::Array(
                snapshot
                    .tabs
                    .iter()
                    .filter(|tab| tab.window_id == window)
                    .map(|tab| project_for(tab, sender, permissions))
                    .collect(),
            )));
        }
        "tabs.create" => {
            let mut properties: CreateProperties = decode(&args[0])?;
            if properties.split_with_tab_id.is_some() {
                return Err(browser::unsupported());
            }
            let window_id = current_window(snapshot, sender, properties.window_id)?;
            let window = snapshot.window(window_id)?;
            if let Some(id) = properties.opener_tab_id {
                require_same_window(snapshot, id, window_id)?;
            }
            properties.window_id = Some(window_id);
            properties.url = properties
                .url
                .map(|url| navigation_url(sender, &url))
                .transpose()?;
            let action = CoreAction::CreateTab {
                window: Some(window.native_id),
                url: properties.url.as_deref().map(Arc::from),
                active: properties.active.or(properties.selected).unwrap_or(true),
            };
            let action = if properties
                .url
                .as_deref()
                .and_then(|url| Url::parse(url).ok())
                .is_some_and(|url| sender.scheme().extension_of(&url).is_some())
            {
                CoreAction::OpenExtensionPage
            } else {
                action
            };
            Request::Create { properties, action }
        }
        "tabs.update" => {
            let tab = target_tab(snapshot, sender, args.first().and_then(as_integer))?;
            let mut properties: UpdateProperties = decode(&args[1])?;
            properties.url = properties
                .url
                .map(|url| navigation_url(sender, &url))
                .transpose()?;
            if let Some(id) = properties.opener_tab_id {
                require_same_window(snapshot, id, tab.window_id)?;
            }
            let mut actions = Vec::new();
            if let Some(url) = &properties.url {
                actions.push(
                    if Url::parse(url).is_ok_and(|url| sender.scheme().extension_of(&url).is_some())
                    {
                        CoreAction::OpenExtensionPage
                    } else {
                        CoreAction::LoadTabUrl {
                            tab: tab.tab.id(),
                            url: Arc::from(url.as_str()),
                        }
                    },
                );
            }
            if let Some(muted) = properties.muted {
                actions.push(CoreAction::SetTabMuted {
                    tab: tab.tab.id(),
                    muted,
                });
            }
            if properties.active == Some(true) || properties.selected == Some(true) {
                actions.push(CoreAction::ActivateTab { tab: tab.tab.id() });
            }
            Request::Update {
                tab_id: tab.id,
                properties,
                actions,
            }
        }
        "tabs.remove" => {
            let tab_ids = identities(&args[0])?;
            let actions = tab_ids
                .iter()
                .map(|id| {
                    snapshot
                        .tab(*id)
                        .map(|tab| CoreAction::CloseTab { tab: tab.tab.id() })
                })
                .collect::<Result<_, _>>()?;
            Request::Remove { tab_ids, actions }
        }
        "tabs.reload" => {
            let tab = target_tab(snapshot, sender, args.first().and_then(as_integer))?;
            Request::Reload {
                tab_id: tab.id,
                bypass_cache: args
                    .get(1)
                    .and_then(|value| value.get("bypassCache"))
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                action: CoreAction::ReloadTab { tab: tab.tab.id() },
            }
        }
        "tabs.duplicate" => {
            let tab = snapshot.tab(integer(&args[0])?)?;
            Request::Duplicate {
                tab_id: tab.id,
                action: CoreAction::DuplicateTab { tab: tab.tab.id() },
            }
        }
        "tabs.move" => {
            let tab_ids = identities(&args[0])?;
            let first = tab_ids.first().ok_or_else(ApiError::malformed_arguments)?;
            let source = snapshot.tab(*first)?;
            let window_id = args[1]
                .get("windowId")
                .and_then(as_integer)
                .map(|id| current_window(snapshot, sender, Some(id)))
                .transpose()?
                .unwrap_or(source.window_id);
            let destination = snapshot.window(window_id)?;
            if destination.kind != "normal" {
                return Err(ApiError::other(
                    "Tabs can only be moved to and from normal windows.",
                ));
            }
            for id in &tab_ids {
                let tab = snapshot.tab(*id)?;
                if snapshot.window(tab.window_id)?.kind != "normal" {
                    return Err(ApiError::other(
                        "Tabs can only be moved to and from normal windows.",
                    ));
                }
            }
            let index = integer(&args[1]["index"])?;
            let same_window = tab_ids.iter().all(|id| {
                snapshot
                    .tab(*id)
                    .is_ok_and(|tab| tab.window_id == window_id)
            });
            let action = if same_window {
                let mut remaining: Vec<_> = snapshot
                    .tabs
                    .iter()
                    .filter(|tab| tab.window_id == window_id && !tab_ids.contains(&tab.id))
                    .collect();
                remaining.sort_by_key(|tab| tab.index);
                let before = usize::try_from(index)
                    .ok()
                    .and_then(|index| remaining.get(index))
                    .map(|tab| tab.tab.id());
                Some(CoreAction::MoveTabs {
                    tabs: tab_ids
                        .iter()
                        .map(|id| snapshot.tab(*id).map(|tab| tab.tab.id()))
                        .collect::<Result<Vec<_>, _>>()?
                        .into_boxed_slice(),
                    before,
                })
            } else {
                None
            };
            Request::Move {
                tab_ids,
                window_id,
                index,
                single: args[0].is_number(),
                action,
            }
        }
        "tabs.highlight" => {
            let window_id = current_window(
                snapshot,
                sender,
                args[0].get("windowId").and_then(as_integer),
            )?;
            let indices = identities(&args[0]["tabs"])?;
            if indices.is_empty() {
                return Err(ApiError::other("No highlighted tab"));
            }
            let tab_ids = indices
                .iter()
                .map(|index| {
                    snapshot
                        .tabs
                        .iter()
                        .find(|tab| {
                            tab.window_id == window_id
                                && i32::try_from(tab.index).ok() == Some(*index)
                        })
                        .map(|tab| tab.id)
                        .ok_or_else(|| ApiError::other(format!("No tab at index: {index}.")))
                })
                .collect::<Result<Vec<_>, _>>()?;
            let tab = snapshot.tab(tab_ids[0])?;
            Request::Highlight {
                window_id,
                action: CoreAction::ActivateTab { tab: tab.tab.id() },
                tab_ids,
            }
        }
        "tabs.goBack" | "tabs.goForward" => {
            let tab = target_tab(snapshot, sender, args.first().and_then(as_integer))?;
            let forward = call.name() == "tabs.goForward";
            Request::Navigate {
                tab_id: tab.id,
                forward,
                action: if forward {
                    CoreAction::GoForward { tab: tab.tab.id() }
                } else {
                    CoreAction::GoBack { tab: tab.tab.id() }
                },
            }
        }
        "tabs.captureVisibleTab" => {
            let window_id = current_window(snapshot, sender, args.first().and_then(as_integer))?;
            let tab = snapshot
                .tabs
                .iter()
                .find(|tab| tab.window_id == window_id && tab.active)
                .ok_or_else(|| ApiError::other("No active tab in the specified window."))?;
            let url = tab.url()?;
            if !permissions.may_capture_tab(tab.tab.id(), tab.document, &url) {
                return Err(ApiError::other(
                    "Either the '<all_urls>' or 'activeTab' permission is required.",
                ));
            }
            // Privileged surfaces are absent from the snapshot; native adapters recheck the active document.
            let options = args.get(1).unwrap_or(&Value::Null);
            Request::Capture {
                window_id,
                tab_id: tab.id,
                document: tab.document,
                document_id: tab.document_id.clone(),
                format: options
                    .get("format")
                    .and_then(Value::as_str)
                    .unwrap_or("jpeg")
                    .into(),
                quality: options
                    .get("quality")
                    .and_then(Value::as_u64)
                    .and_then(|value| u8::try_from(value).ok()),
            }
        }
        "tabs.detectLanguage" => {
            let tab = target_tab(snapshot, sender, args.first().and_then(as_integer))?;
            if !readable_for(tab, sender, permissions) {
                return Err(ApiError::cannot_access_page());
            }
            if tab.tab.discarded() {
                return Err(ApiError::other("Cannot determine language: tab not loaded"));
            }
            Request::DetectLanguage {
                tab_id: tab.id,
                document: tab.document,
                document_id: tab.document_id.clone(),
            }
        }
        "tabs.getZoom" => {
            return Ok(Prepared::Reply(json!(
                target_tab(snapshot, sender, args.first().and_then(as_integer))?.zoom
            )));
        }
        "tabs.setZoom" => {
            let tab = target_tab(snapshot, sender, args.first().and_then(as_integer))?;
            let factor = args[1].as_f64().ok_or_else(ApiError::malformed_arguments)?;
            if !factor.is_finite() || factor < 0.0 {
                return Err(ApiError::other(
                    "Zoom factor must be greater than or equal to 0.",
                ));
            }
            let effective = if factor == 0.0 { 1.0 } else { factor };
            let percent = effective * 100.0;
            if percent < f64::from(zephium_core::zoom::MIN_PERCENT)
                || percent > f64::from(zephium_core::zoom::MAX_PERCENT)
            {
                return Err(ApiError::other("Zoom factor is out of range."));
            }
            Request::SetZoom {
                tab_id: tab.id,
                factor,
                action: CoreAction::SetTabZoom {
                    tab: tab.tab.id(),
                    percent: if factor == 0.0 {
                        None
                    } else {
                        Some(percent.round() as u16)
                    },
                },
            }
        }
        "tabs.discard" => {
            let tab = if let Some(id) = args.first().and_then(as_integer) {
                snapshot.tab(id)?
            } else {
                snapshot
                    .tabs
                    .iter()
                    .filter(|tab| !tab.active && !tab.tab.discarded() && tab.auto_discardable)
                    .min_by(|left, right| left.last_accessed.total_cmp(&right.last_accessed))
                    .ok_or_else(|| ApiError::other("Cannot find a tab to discard."))?
            };
            if tab.active || tab.tab.discarded() {
                return Err(ApiError::other("Cannot discard tab."));
            }
            Request::Discard { tab_id: tab.id }
        }
        _ => return Err(browser::unsupported()),
    };
    Ok(Prepared::Request(request))
}

/// Chrome-shaped tab projection with sensitive fields omitted when unreadable.
pub fn project(tab: &BrowserTab, permissions: &InstallPermissions) -> Value {
    let mut result = json!({
        "id":tab.id,"windowId":tab.window_id,"index":tab.index,"active":tab.active,
        "selected":tab.active,"highlighted":tab.highlighted,"pinned":tab.tab.pinned(),
        "audible":tab.tab.audible(),"mutedInfo":{"muted":tab.tab.muted()},"discarded":tab.tab.discarded(),
        "autoDiscardable":tab.auto_discardable,"frozen":false,"incognito":tab.incognito,
        "groupId":-1,"splitViewId":-1,"status":status(tab),"lastAccessed":tab.last_accessed,
    });
    if let Some(id) = tab.opener_tab_id {
        result["openerTabId"] = json!(id);
    }
    if readable(tab, permissions) {
        result["url"] = json!(tab.committed_url().unwrap_or(""));
        result["title"] = json!(tab.tab.title());
        if let Some(url) = &tab.fav_icon_url {
            result["favIconUrl"] = json!(url);
        }
    }
    if let Some(url) = &tab.pending_url {
        if Url::parse(url)
            .is_ok_and(|url| permissions.may_read_tab(tab.tab.id(), tab.document, &url))
        {
            result["pendingUrl"] = json!(url);
        }
    }
    result
}

/// Caller-aware projection, including Chrome's implicit access to the install's own origin.
pub fn project_for(tab: &BrowserTab, sender: &Sender, permissions: &InstallPermissions) -> Value {
    let mut value = project(tab, permissions);
    if tab.url().is_ok_and(|url| is_own_origin(sender, &url)) {
        value["url"] = json!(tab.committed_url().unwrap_or(""));
        value["title"] = json!(tab.tab.title());
        if let Some(url) = &tab.fav_icon_url {
            value["favIconUrl"] = json!(url);
        }
    }
    if let Some(pending) = &tab.pending_url {
        if Url::parse(pending).is_ok_and(|url| is_own_origin(sender, &url)) {
            value["pendingUrl"] = json!(pending);
        }
    }
    value
}

/// Changed properties for onUpdated; selection and movement have their own events.
/// Sensitive changes are filtered using the new document's authority.
pub fn change_info(
    previous: &BrowserTab,
    current: &BrowserTab,
    permissions: &InstallPermissions,
) -> Value {
    let old = changes(previous);
    let raw = changes(current);
    let new = project(current, permissions);
    let mut changed = Map::new();
    for (field, value) in raw {
        if old.get(&field) != Some(&value) {
            if let Some(value) = new.get(&field) {
                changed.insert(field, value.clone());
            }
        }
    }
    Value::Object(changed)
}

/// Caller-aware changed properties, including implicit same-extension origin access.
pub fn change_info_for(
    previous: &BrowserTab,
    current: &BrowserTab,
    sender: &Sender,
    permissions: &InstallPermissions,
) -> Value {
    let old = changes(previous);
    let raw = changes(current);
    let new = project_for(current, sender, permissions);
    let mut changed = Map::new();
    for (field, value) in raw {
        if old.get(&field) != Some(&value) {
            if let Some(value) = new.get(&field) {
                changed.insert(field, value.clone());
            }
        }
    }
    Value::Object(changed)
}

fn is_own_origin(sender: &Sender, url: &Url) -> bool {
    sender
        .scheme()
        .extension_of(url)
        .is_some_and(|id| &id == sender.extension_id())
}
fn readable_for(tab: &BrowserTab, sender: &Sender, permissions: &InstallPermissions) -> bool {
    tab.url().is_ok_and(|url| is_own_origin(sender, &url)) || readable(tab, permissions)
}

fn changes(tab: &BrowserTab) -> Map<String, Value> {
    let value = json!({
        "url":tab.committed_url(),"pendingUrl":tab.pending_url,"title":tab.tab.title(),
        "favIconUrl":tab.fav_icon_url,"status":status(tab),"pinned":tab.tab.pinned(),
        "audible":tab.tab.audible(),"mutedInfo":{"muted":tab.tab.muted()},
        "discarded":tab.tab.discarded(),"autoDiscardable":tab.auto_discardable,
    });
    match value {
        Value::Object(value) => value,
        _ => Map::new(),
    }
}

fn readable(tab: &BrowserTab, permissions: &InstallPermissions) -> bool {
    tab.url()
        .is_ok_and(|url| permissions.may_read_tab(tab.tab.id(), tab.document, &url))
}

fn status(tab: &BrowserTab) -> &'static str {
    if tab.tab.discarded() {
        "unloaded"
    } else if tab.tab.loading() {
        "loading"
    } else {
        "complete"
    }
}

/// Resolves the current window from the caller's tab before falling back to adapter focus.
pub(crate) fn current_window(
    snapshot: &BrowserSnapshot,
    sender: &Sender,
    id: Option<i32>,
) -> Result<i32, ApiError> {
    let id = match id {
        Some(id) if id != -2 => id,
        _ => sender
            .tab()
            .and_then(|id| snapshot.tabs.iter().find(|tab| tab.tab.id() == id))
            .map(|tab| tab.window_id)
            .or(snapshot.current_window)
            .or(snapshot.last_focused_window)
            .ok_or_else(|| ApiError::other("No current window."))?,
    };
    Ok(snapshot.window(id)?.id)
}

fn target_tab<'a>(
    snapshot: &'a BrowserSnapshot,
    sender: &Sender,
    id: Option<i32>,
) -> Result<&'a BrowserTab, ApiError> {
    if let Some(id) = id {
        return snapshot.tab(id);
    }
    let window_id = current_window(snapshot, sender, None)?;
    snapshot
        .tabs
        .iter()
        .find(|tab| tab.window_id == window_id && tab.active)
        .ok_or_else(|| ApiError::other("No selected tab"))
}

fn require_same_window(
    snapshot: &BrowserSnapshot,
    id: i32,
    window_id: i32,
) -> Result<(), ApiError> {
    if snapshot.tab(id)?.window_id != window_id {
        return Err(ApiError::other(
            "Tab opener must be in the same window as the updated tab.",
        ));
    }
    Ok(())
}

/// Resolves extension-relative URLs and rejects executable or privileged navigation targets.
pub(crate) fn navigation_url(sender: &Sender, input: &str) -> Result<String, ApiError> {
    let url = match Url::parse(input) {
        Ok(url) => url,
        Err(url::ParseError::RelativeUrlWithoutBase) => sender
            .url()
            .join(input)
            .map_err(|_| ApiError::other("Invalid URL."))?,
        Err(_) => return Err(ApiError::other("Invalid URL.")),
    };
    if sender
        .scheme()
        .extension_of(&url)
        .is_some_and(|id| &id != sender.extension_id())
    {
        return Err(ApiError::cannot_access_other_extension());
    }
    let own_extension = sender.scheme().extension_of(&url).is_some();
    if !url.username().is_empty()
        || url.password().is_some()
        || !(zephium_core::navigation::is_allowed(&url) || own_extension)
    {
        return Err(ApiError::other("Cannot navigate to this URL."));
    }
    if url.as_str().len() > zephium_core::extensions::MAX_EXTENSION_BROWSER_REQUEST_URL_BYTES {
        return Err(ApiError::other("Invalid URL."));
    }
    Ok(url.into())
}

fn query(
    snapshot: &BrowserSnapshot,
    sender: &Sender,
    info: &Value,
    permissions: &InstallPermissions,
) -> Result<Value, ApiError> {
    let title = info
        .get("title")
        .and_then(Value::as_str)
        .map(UrlGlob::parse)
        .transpose()
        .map_err(|_| ApiError::other("Invalid title pattern."))?;
    let patterns = info
        .get("url")
        .map(|value| {
            strings(value)
                .into_iter()
                .map(|pattern| {
                    MatchPattern::parse(pattern)
                        .map_err(|_| ApiError::other(format!("Invalid url pattern '{pattern}'")))
                })
                .collect::<Result<Vec<_>, _>>()
        })
        .transpose()?;
    let current = current_window(snapshot, sender, None).ok();
    let mut result = Vec::new();
    for tab in &snapshot.tabs {
        let window = snapshot.window(tab.window_id)?;
        let bools = [
            ("active", tab.active),
            ("pinned", tab.tab.pinned()),
            ("audible", tab.tab.audible()),
            ("muted", tab.tab.muted()),
            ("highlighted", tab.highlighted),
            ("frozen", false),
            ("discarded", tab.tab.discarded()),
            ("autoDiscardable", tab.auto_discardable),
            ("currentWindow", Some(tab.window_id) == current),
            (
                "lastFocusedWindow",
                Some(tab.window_id) == snapshot.last_focused_window,
            ),
        ];
        if bools.iter().any(|(key, actual)| {
            info.get(key)
                .and_then(Value::as_bool)
                .is_some_and(|expected| expected != *actual)
        }) {
            continue;
        }
        if info
            .get("windowId")
            .and_then(as_integer)
            .is_some_and(|id| tab.window_id != if id == -2 { current.unwrap_or(-1) } else { id })
            || info
                .get("index")
                .and_then(Value::as_u64)
                .is_some_and(|index| u64::try_from(tab.index).ok() != Some(index))
            || info
                .get("groupId")
                .and_then(as_integer)
                .is_some_and(|id| id != -1)
            || info
                .get("splitViewId")
                .and_then(as_integer)
                .is_some_and(|id| id != -1)
            || info
                .get("status")
                .and_then(Value::as_str)
                .is_some_and(|value| value != status(tab))
            || info
                .get("windowType")
                .and_then(Value::as_str)
                .is_some_and(|value| value != window.kind)
        {
            continue;
        }
        if readable_for(tab, sender, permissions) {
            if title
                .as_ref()
                .is_some_and(|pattern| !pattern.matches(tab.tab.title()))
            {
                continue;
            }
            if let Some(patterns) = &patterns {
                let mut url = tab.url()?;
                url.set_fragment(None);
                if !patterns.iter().any(|pattern| pattern.matches_url(&url)) {
                    continue;
                }
            }
        }
        result.push(project_for(tab, sender, permissions));
    }
    Ok(Value::Array(result))
}

fn strings(value: &Value) -> Vec<&str> {
    match value {
        Value::String(value) => vec![value],
        Value::Array(values) => values.iter().filter_map(Value::as_str).collect(),
        _ => Vec::new(),
    }
}
fn identities(value: &Value) -> Result<Vec<i32>, ApiError> {
    if let Some(values) = value.as_array() {
        values.iter().map(integer).collect()
    } else {
        Ok(vec![integer(value)?])
    }
}
fn integer(value: &Value) -> Result<i32, ApiError> {
    as_integer(value).ok_or_else(ApiError::malformed_arguments)
}
fn as_integer(value: &Value) -> Option<i32> {
    value.as_i64().and_then(|value| i32::try_from(value).ok())
}
fn decode<T: serde::de::DeserializeOwned>(value: &Value) -> Result<T, ApiError> {
    serde_json::from_value(value.clone()).map_err(|_| ApiError::malformed_arguments())
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;
    use zephium_webext::manifest::Manifest;

    use super::*;
    use crate::browser::fake::FakeBrowser;
    use crate::context::tests::{background_sender, content_sender, profile};
    use crate::dispatch::{Dispatcher, Inbound, Responder};
    use crate::envelope::{ApiName, CallId, Direction, Envelope};
    use crate::permissions::{PermissionDeclaration, PermissionRequest, SiteAccess};

    fn permissions(api: &[&str], hosts: &[&str]) -> InstallPermissions {
        let (declaration, issues) = PermissionDeclaration::from_manifest(&Manifest::from_value(
            json!({
                "manifest_version":3,"name":"test","version":"1","permissions":api,"host_permissions":hosts,
            }),
        ));
        assert!(issues.is_empty());
        InstallPermissions::new(
            declaration,
            &PermissionRequest::default(),
            SiteAccess::OnAllSites,
            false,
        )
    }

    fn browser() -> (FakeBrowser, Dispatcher<FakeBrowser>, Sender) {
        let mut browser = FakeBrowser::new(profile());
        let window = browser.add_window(false);
        browser.add_tab(window, "https://example.com/one", true);
        browser.add_tab(window, "https://other.test/two", false);
        (browser, FakeBrowser::dispatcher(), background_sender())
    }

    #[test]
    fn projection_redacts_committed_and_pending_addresses_independently() {
        let (mut browser, mut dispatcher, sender) = browser();
        browser.snapshot.tabs[0].pending_url = Some("https://other.test/pending".into());
        browser.snapshot.tabs[0].fav_icon_url = Some("https://example.com/icon.png".into());
        let none = permissions(&[], &[]);
        let hosts = permissions(&[], &["https://example.com/*"]);
        let tabs = permissions(&["tabs"], &[]);
        for (permissions, first, second, pending) in [
            (&none, false, false, false),
            (&hosts, true, false, false),
            (&tabs, true, true, true),
        ] {
            let result = browser
                .call(
                    &mut dispatcher,
                    &sender,
                    permissions,
                    "tabs.query",
                    json!([{}]),
                )
                .unwrap();
            for key in ["url", "title", "favIconUrl"] {
                assert_eq!(result[0].get(key).is_some(), first, "{key}");
            }
            assert_eq!(result[0].get("pendingUrl").is_some(), pending);
            assert_eq!(result[1].get("url").is_some(), second);
            assert_eq!(result[0]["active"], true);
        }
    }

    #[test]
    fn query_supports_every_generated_filter_and_ignores_unreadable_sensitive_filters() {
        let (mut browser, mut dispatcher, sender) = browser();
        browser.snapshot.tabs[0].fav_icon_url = Some("https://example.com/icon.png".into());
        let granted = permissions(&["tabs"], &[]);
        let result=browser.call(&mut dispatcher,&sender,&granted,"tabs.query",json!([{
            "active":true,"pinned":false,"audible":false,"muted":false,"highlighted":true,
            "frozen":false,"discarded":false,"autoDiscardable":true,"currentWindow":true,
            "lastFocusedWindow":true,"status":"complete","title":"example.*","url":["https://example.com/one"],
            "groupId":-1,"splitViewId":-1,"windowId":-2,"windowType":"normal","index":0,
        }])).unwrap();
        assert_eq!(result.as_array().unwrap().len(), 1);
        assert_eq!(result[0]["id"], 1);
        for filter in [
            json!({"active":false,"index":0}),
            json!({"groupId":0}),
            json!({"splitViewId":0}),
            json!({"windowType":"popup"}),
            json!({"status":"loading"}),
            json!({"frozen":true}),
        ] {
            assert_eq!(
                browser
                    .call(
                        &mut dispatcher,
                        &sender,
                        &granted,
                        "tabs.query",
                        json!([filter])
                    )
                    .unwrap(),
                json!([])
            );
        }
        let unreadable = browser
            .call(
                &mut dispatcher,
                &sender,
                &permissions(&[], &[]),
                "tabs.query",
                json!([{"url":"https://no-match.test/*","title":"no-match"}]),
            )
            .unwrap();
        assert_eq!(unreadable.as_array().unwrap().len(), 2);
        assert!(browser
            .call(
                &mut dispatcher,
                &sender,
                &granted,
                "tabs.query",
                json!([{"url":"invalid"}])
            )
            .is_err());
    }

    #[test]
    fn management_does_not_require_host_permission_and_replies_remain_redacted() {
        let (mut browser, mut dispatcher, sender) = browser();
        let none = permissions(&[], &[]);
        let created=browser.call(&mut dispatcher,&sender,&none,"tabs.create",json!([{"url":"https://third.test/","active":false,"index":0,"pinned":true,"openerTabId":1}])).unwrap();
        assert_eq!(created["index"], 0);
        assert_eq!(created["pinned"], true);
        assert!(created.get("url").is_none());
        let id = created["id"].as_i64().unwrap();
        let updated = browser
            .call(
                &mut dispatcher,
                &sender,
                &none,
                "tabs.update",
                json!([id,{"muted":true,"autoDiscardable":false,"active":true}]),
            )
            .unwrap();
        assert_eq!(updated["mutedInfo"]["muted"], true);
        assert_eq!(updated["active"], true);
        assert!(updated.get("title").is_none());
        assert_eq!(
            browser
                .call(
                    &mut dispatcher,
                    &sender,
                    &none,
                    "tabs.remove",
                    json!([[id]])
                )
                .unwrap(),
            Value::Null
        );
        assert!(browser
            .call(&mut dispatcher, &sender, &none, "tabs.get", json!([id]))
            .is_err());
    }

    #[test]
    fn get_current_distinguishes_background_and_tab_contexts() {
        let (mut browser, mut dispatcher, sender) = browser();
        let granted = permissions(&["tabs"], &[]);
        assert_eq!(
            browser
                .call(
                    &mut dispatcher,
                    &sender,
                    &granted,
                    "tabs.getCurrent",
                    json!([])
                )
                .unwrap(),
            Value::Null
        );
        let sender = content_sender("https://example.com/");
        // Sender tab IDs are native ItemIds, never Chrome IDs supplied by JavaScript.
        let tab = browser.snapshot.tabs[0].clone();
        browser.snapshot.tabs[0].tab =
            zephium_core::extensions::ExtensionBrowserTab::from_snapshot(
                None,
                sender.tab().unwrap(),
                true,
                tab.tab.title(),
                tab.url().ok().as_ref(),
                false,
                false,
            )
            .unwrap();
        // Generated context restrictions prevent content scripts from reaching the handler.
        assert!(browser
            .call(
                &mut dispatcher,
                &sender,
                &granted,
                "tabs.getCurrent",
                json!([])
            )
            .is_err());
    }

    #[test]
    fn capture_requires_all_urls_or_live_active_tab_and_rejects_narrow_host_grants() {
        let (mut browser, mut dispatcher, sender) = browser();
        let mut active = permissions(&["activeTab"], &[]);
        let tab = browser.snapshot.tabs[0].clone();
        active.grant_active_tab(tab.tab.id(), tab.document, &tab.url().unwrap());
        for permission in [
            permissions(&[], &[]),
            permissions(&["tabs"], &[]),
            permissions(&[], &["https://example.com/*"]),
        ] {
            let before = browser.requests.len();
            assert!(browser
                .call(
                    &mut dispatcher,
                    &sender,
                    &permission,
                    "tabs.captureVisibleTab",
                    json!([])
                )
                .is_err());
            assert_eq!(before, browser.requests.len());
        }
        for permission in [permissions(&[], &["<all_urls>"]), active.clone()] {
            assert!(browser
                .call(
                    &mut dispatcher,
                    &sender,
                    &permission,
                    "tabs.captureVisibleTab",
                    json!([{"format":"png"}])
                )
                .unwrap()
                .as_str()
                .unwrap()
                .starts_with("data:image/"));
        }
        browser.snapshot.tabs[0].document =
            DocumentGeneration::new(tab.document.get() + 100).unwrap();
        assert!(browser
            .call(
                &mut dispatcher,
                &sender,
                &active,
                "tabs.captureVisibleTab",
                json!([])
            )
            .is_err());
    }

    #[test]
    fn mutations_validate_every_identity_before_reaching_the_adapter() {
        let (mut browser, mut dispatcher, sender) = browser();
        let granted = permissions(&["tabs"], &[]);
        for (name, args) in [
            ("tabs.remove", json!([[1, 999]])),
            ("tabs.move", json!([[1,999],{"index":0}])),
            ("tabs.highlight", json!([{"tabs":[0,99]}])),
            ("tabs.create", json!([{"url":"javascript:alert(1)"}])),
            ("tabs.create", json!([{"url":"file:///private.txt"}])),
            (
                "tabs.create",
                json!([{"url":"https://name:secret@example.com/"}]),
            ),
            ("tabs.update", json!([1,{"openerTabId":999}])),
            ("tabs.setZoom", json!([1, -1])),
            ("tabs.group", json!([{"tabIds":[1]}])),
            ("tabs.setZoomSettings", json!([1,{"mode":"manual"}])),
        ] {
            let before = browser.requests.len();
            assert!(
                browser
                    .call(&mut dispatcher, &sender, &granted, name, args)
                    .is_err(),
                "{name}"
            );
            assert_eq!(before, browser.requests.len(), "{name}");
        }
        assert_eq!(browser.snapshot.tabs.len(), 2);
    }

    #[test]
    fn history_duplicate_move_highlight_zoom_and_discard_keep_browser_state() {
        let (mut browser, mut dispatcher, sender) = browser();
        let granted = permissions(&["tabs"], &[]);
        browser
            .call(
                &mut dispatcher,
                &sender,
                &granted,
                "tabs.update",
                json!([1,{"url":"https://example.com/next"}]),
            )
            .unwrap();
        browser
            .call(
                &mut dispatcher,
                &sender,
                &granted,
                "tabs.goBack",
                json!([1]),
            )
            .unwrap();
        assert_eq!(
            browser.snapshot.tab(1).unwrap().tab.url(),
            Some("https://example.com/one")
        );
        browser
            .call(
                &mut dispatcher,
                &sender,
                &granted,
                "tabs.goForward",
                json!([1]),
            )
            .unwrap();
        let duplicate = browser
            .call(
                &mut dispatcher,
                &sender,
                &granted,
                "tabs.duplicate",
                json!([1]),
            )
            .unwrap();
        let id = duplicate["id"].as_i64().unwrap();
        assert_eq!(duplicate["index"], 1);
        browser
            .call(
                &mut dispatcher,
                &sender,
                &granted,
                "tabs.goBack",
                json!([id]),
            )
            .unwrap();
        assert_eq!(
            browser.snapshot.tab(id as i32).unwrap().tab.url(),
            Some("https://example.com/one")
        );
        let moved = browser
            .call(
                &mut dispatcher,
                &sender,
                &granted,
                "tabs.move",
                json!([[id],{"index":-1}]),
            )
            .unwrap();
        assert_eq!(moved[0]["index"], 2);
        let window = browser
            .call(
                &mut dispatcher,
                &sender,
                &granted,
                "tabs.highlight",
                json!([{"tabs":[0,1]}]),
            )
            .unwrap();
        assert_eq!(window["tabs"][0]["active"], true);
        assert_eq!(window["tabs"][1]["highlighted"], true);
        browser
            .call(
                &mut dispatcher,
                &sender,
                &granted,
                "tabs.setZoom",
                json!([1, 1.5]),
            )
            .unwrap();
        assert_eq!(
            browser
                .call(
                    &mut dispatcher,
                    &sender,
                    &granted,
                    "tabs.getZoom",
                    json!([1])
                )
                .unwrap(),
            json!(1.5)
        );
        assert!(browser
            .call(
                &mut dispatcher,
                &sender,
                &granted,
                "tabs.discard",
                json!([1])
            )
            .is_err());
        let discarded = browser
            .call(
                &mut dispatcher,
                &sender,
                &granted,
                "tabs.discard",
                json!([2]),
            )
            .unwrap();
        assert_eq!(discarded["discarded"], true);
        assert!(browser
            .call(
                &mut dispatcher,
                &sender,
                &granted,
                "tabs.detectLanguage",
                json!([2])
            )
            .is_err());
    }

    #[test]
    fn own_extension_documents_use_native_guest_projection_and_privileged_broker_action() {
        let (mut browser, mut dispatcher, sender) = browser();
        let permissions = permissions(&[], &[]);
        let created = browser
            .call(
                &mut dispatcher,
                &sender,
                &permissions,
                "tabs.create",
                json!([{"url":"welcome.html"}]),
            )
            .unwrap();
        let id = created["id"].as_i64().unwrap() as i32;
        assert!(created["url"]
            .as_str()
            .unwrap()
            .starts_with("chrome-extension://"));
        let native_guest = !zephium_core::navigation::is_browser_target(
            &Url::parse(created["url"].as_str().unwrap()).unwrap(),
        );
        assert_eq!(
            browser.snapshot.tab(id).unwrap().tab.url().is_none(),
            native_guest
        );
        assert!(browser
            .call(
                &mut dispatcher,
                &sender,
                &permissions,
                "tabs.get",
                json!([id])
            )
            .unwrap()
            .get("url")
            .is_some());
        let queried = browser
            .call(
                &mut dispatcher,
                &sender,
                &permissions,
                "tabs.query",
                json!([{"title":"no-match","url":"https://no-match.test/*"}]),
            )
            .unwrap();
        assert!(queried
            .as_array()
            .unwrap()
            .iter()
            .all(|tab| tab["id"] != id));
        let populated = browser
            .call(
                &mut dispatcher,
                &sender,
                &permissions,
                "windows.getCurrent",
                json!([{"populate":true}]),
            )
            .unwrap();
        assert!(populated["tabs"]
            .as_array()
            .unwrap()
            .iter()
            .find(|tab| tab["id"] == id)
            .unwrap()
            .get("url")
            .is_some());

        assert!(matches!(
            browser.requests.last(),
            Some(BrowserRequest::Tabs(Request::Create {
                action: CoreAction::OpenExtensionPage,
                ..
            }))
        ));
        let updated = browser
            .call(
                &mut dispatcher,
                &sender,
                &permissions,
                "tabs.update",
                json!([id,{"url":"options.html"}]),
            )
            .unwrap();
        assert!(updated["url"].as_str().unwrap().ends_with("/options.html"));
        assert!(
            matches!(browser.requests.last(),Some(BrowserRequest::Tabs(Request::Update{actions,..})) if matches!(actions.as_slice(),[CoreAction::OpenExtensionPage]))
        );
        browser
            .call(
                &mut dispatcher,
                &sender,
                &permissions,
                "tabs.update",
                json!([id,{"url":"https://example.com/ordinary"}]),
            )
            .unwrap();
        assert!(browser.snapshot.tab(id).unwrap().native_url.is_none());
        assert_eq!(
            browser.snapshot.tab(id).unwrap().tab.url(),
            Some("https://example.com/ordinary")
        );
    }

    #[test]
    fn fake_history_data_queries_real_navigation_and_deletion() {
        let (mut browser, _, sender) = browser();
        let granted = permissions(&["tabs", "history", "topSites"], &[]);
        let mut table = ApiTable::new();
        register(&mut table).unwrap();
        crate::api::history::register(&mut table).unwrap();
        crate::api::top_sites::register(&mut table).unwrap();
        let mut dispatcher = Dispatcher::new(table);
        browser.time_ms = 10_000;
        browser
            .call(
                &mut dispatcher,
                &sender,
                &granted,
                "tabs.update",
                json!([1,{"url":"https://example.com/visited"}]),
            )
            .unwrap();
        browser.time_ms = 20_000;
        browser
            .call(
                &mut dispatcher,
                &sender,
                &granted,
                "history.addUrl",
                json!([{"url":"https://example.com/visited"}]),
            )
            .unwrap();
        let visits = browser
            .call(
                &mut dispatcher,
                &sender,
                &granted,
                "history.getVisits",
                json!([{"url":"https://example.com/visited"}]),
            )
            .unwrap();
        assert_eq!(visits.as_array().unwrap().len(), 2);
        assert_ne!(visits[0]["visitId"], visits[1]["visitId"]);
        browser.time_ms = 30_000;
        let results = browser
            .call(
                &mut dispatcher,
                &sender,
                &granted,
                "history.search",
                json!([{"text":"visited","startTime":0}]),
            )
            .unwrap();
        assert_eq!(results.as_array().unwrap().len(), 1);
        assert_eq!(results[0]["visitCount"], 2);
        let top = browser
            .call(
                &mut dispatcher,
                &sender,
                &granted,
                "topSites.get",
                json!([]),
            )
            .unwrap();
        assert_eq!(top[0]["url"], "https://example.com/visited");
        browser
            .call(
                &mut dispatcher,
                &sender,
                &granted,
                "history.deleteRange",
                json!([{"startTime":0,"endTime":15_000}]),
            )
            .unwrap();
        let visits = browser
            .call(
                &mut dispatcher,
                &sender,
                &granted,
                "history.getVisits",
                json!([{"url":"https://example.com/visited"}]),
            )
            .unwrap();
        assert_eq!(visits.as_array().unwrap().len(), 1);
        browser
            .call(
                &mut dispatcher,
                &sender,
                &granted,
                "history.deleteAll",
                json!([]),
            )
            .unwrap();
        let visits = browser
            .call(
                &mut dispatcher,
                &sender,
                &granted,
                "history.getVisits",
                json!([{"url":"https://example.com/visited"}]),
            )
            .unwrap();
        assert_eq!(visits, json!([]));
    }

    struct DeferredBrowser {
        browser: FakeBrowser,
        responder: Option<Responder>,
    }
    impl BrowserPort for DeferredBrowser {
        fn snapshot(&self, sender: &Sender) -> Result<BrowserSnapshot, ApiError> {
            self.browser.snapshot(sender)
        }
        fn extension_info(
            &self,
            sender: &Sender,
        ) -> Result<crate::browser::ExtensionInfo, ApiError> {
            self.browser.extension_info(sender)
        }
        fn request(&mut self, _: BrowserRequest, invocation: Invocation<'_>) -> Handled {
            let (handled, responder) = invocation.defer();
            self.responder = Some(responder);
            handled
        }
    }

    #[test]
    fn deferred_mutation_is_not_success_and_context_cancellation_discards_late_settlement() {
        let (browser, _, sender) = browser();
        let mut host = DeferredBrowser {
            browser,
            responder: None,
        };
        let mut table = ApiTable::new();
        register(&mut table).unwrap();
        let mut dispatcher = Dispatcher::new(table);
        let bytes = Envelope::Call {
            id: CallId::new(1).unwrap(),
            name: ApiName::parse("tabs.remove").unwrap(),
            args: b"[1]",
        }
        .encode(Direction::FromContext, u64::MAX)
        .unwrap();
        let permissions = permissions(&[], &[]);
        assert!(matches!(
            dispatcher.receive(&mut host, &permissions, &sender, &bytes, 0),
            Inbound::Pending
        ));
        assert_eq!(dispatcher.pending_calls(), 1);
        assert_eq!(host.browser.snapshot.tabs.len(), 2);
        assert_eq!(dispatcher.cancel_context(&sender.context_key()), 1);
        assert!(dispatcher
            .settle(host.responder.take().unwrap(), Ok(Value::Null))
            .is_none());
    }

    proptest! {
        #[test]
        fn navigation_event_order_and_change_info_follow_each_actual_transition(paths in prop::collection::vec(1u16..500,1..25)) {
            let (mut browser,mut dispatcher,sender)=browser();
            let permissions=permissions(&["tabs"],&[]);
            for (step,path) in paths.into_iter().enumerate() {
                let url=format!("https://example.com/path-{step}-{path}");
                browser.events.clear();
                browser.call(&mut dispatcher,&sender,&permissions,"tabs.update",json!([1,{"url":url}])).unwrap();
                let events:Vec<_>=browser.events.iter().filter(|(name,_)|name=="tabs.onUpdated").collect();
                prop_assert_eq!(events.len(),3);
                prop_assert_eq!(&events[0].1[1]["status"],&json!("loading"));
                prop_assert!(events[0].1[1].get("url").is_none());
                prop_assert_eq!(&events[2].1[1]["status"],&json!("complete"));
                prop_assert!(events[2].1[1].get("url").is_none());
                prop_assert!(events.iter().all(|(_,args)|args[0]==1));
                prop_assert_eq!(browser.snapshot.tabs.iter().filter(|tab|tab.active).count(),1);
            }
        }
    }
}
