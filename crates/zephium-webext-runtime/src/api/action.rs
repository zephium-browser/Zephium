//! Per-install action state and trusted toolbar activation.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use zephium_core::extensions::{
    ExtensionActionIcon, ExtensionActionRevision, ExtensionActionScope, ExtensionActionState,
    ExtensionRuntimeInstance, EXTENSION_ACTION_ICON_HEIGHT, EXTENSION_ACTION_ICON_WIDTH,
    MAX_EXTENSION_ACTION_BADGE_BYTES, MAX_EXTENSION_ACTION_LABEL_BYTES,
};
use zephium_core::ids::ItemId;

use crate::browser::{snapshot, BrowserPort, BrowserRequest, BrowserTab};
use crate::context::Sender;
use crate::dispatch::{ApiTable, Handled, Invocation, RegistrationError};
use crate::permissions::InstallPermissions;
use crate::ApiError;

pub(crate) const METHODS: &[&str] = &[
    "action.setTitle",
    "action.getTitle",
    "action.setIcon",
    "action.setPopup",
    "action.getPopup",
    "action.setBadgeText",
    "action.getBadgeText",
    "action.setBadgeBackgroundColor",
    "action.getBadgeBackgroundColor",
    "action.setBadgeTextColor",
    "action.getBadgeTextColor",
    "action.enable",
    "action.disable",
    "action.isEnabled",
    "action.openPopup",
    "action.getUserSettings",
];
/// Each untrusted icon representation is bounded before native image decoding.
pub const MAX_ICON_DIMENSION: u16 = 256;
/// A setIcon batch contains at most sixteen scale representations.
pub const MAX_ICON_REPRESENTATIONS: usize = 16;
const MAX_TITLE_BYTES: usize = 64 * 1024;
const MAX_BADGE_BYTES: usize = 100;

/// RGBA bytes whose checked dimensions precede any raster allocation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RgbaImage {
    /// Width and height in pixels.
    pub width: u16,
    /// Height in pixels.
    pub height: u16,
    /// Exactly width * height * four bytes in RGBA order.
    pub data: Vec<u8>,
}

/// Encoded packaged files are decoded and bounded by the native adapter before commit.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Icon {
    /// Scale size to extension-relative resource path.
    Paths(BTreeMap<u16, String>),
    /// Scale size to validated image pixels.
    Images(BTreeMap<u16, RgbaImage>),
}

/// One action property getter.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Property {
    /// Toolbar tooltip.
    Title,
    /// Packaged popup URL.
    Popup,
    /// Effective badge text.
    BadgeText,
    /// Badge background RGBA.
    BadgeBackgroundColor,
    /// Badge text RGBA.
    BadgeTextColor,
    /// Whether clicks are enabled.
    Enabled,
}

/// One atomic property change; absent badge text resets inheritance.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Update {
    /// Toolbar tooltip.
    Title(String),
    /// Packaged popup URL, empty to disable a popup.
    Popup(String),
    /// Badge text; None restores inheritance.
    BadgeText(Option<String>),
    /// Badge background RGBA.
    BadgeBackgroundColor([u8; 4]),
    /// Badge foreground RGBA with nonzero alpha.
    BadgeTextColor([u8; 4]),
    /// Validated icon resources.
    Icon(Icon),
    /// Enable or disable this action.
    Enabled(bool),
}

/// Browser action operations; adapters retain their install and generation scope.
#[derive(Clone, Debug)]
pub enum Request {
    /// Get effective per-tab state or the global default.
    Get {
        /// Ordinary Chrome tab identity, or default scope.
        tab_id: Option<i32>,
        /// Property to read.
        property: Property,
    },
    /// Commit state and publish a new Shell action snapshot.
    Set {
        /// Ordinary Chrome tab identity, or default scope.
        tab_id: Option<i32>,
        /// Atomic property change.
        update: Update,
    },
    /// Present the current tab's configured popup in the focused window.
    OpenPopup {
        /// Focused Chrome window identity.
        window_id: i32,
        /// Its active ordinary tab.
        tab_id: i32,
    },
    /// Return the actual browser toolbar pin setting.
    GetUserSettings,
}

#[derive(Clone, Debug, Default)]
struct Overrides {
    title: Option<String>,
    popup: Option<String>,
    badge: Option<String>,
    background: Option<[u8; 4]>,
    foreground: Option<[u8; 4]>,
    icon: Option<Icon>,
    raster: Option<ExtensionActionIcon>,
    enabled: Option<bool>,
}

/// Reusable ephemeral state for one install, isolated from private instances.
#[derive(Clone, Debug)]
pub struct ActionStore {
    defaults: Overrides,
    tabs: BTreeMap<i32, Overrides>,
    dnr_badges: BTreeMap<i32, String>,
    revision: ExtensionActionRevision,
    /// Native toolbar pin preference; adapters update it from browser settings.
    pub is_on_toolbar: bool,
}

impl Default for ActionStore {
    fn default() -> Self {
        Self {
            defaults: Overrides {
                title: Some(String::new()),
                popup: Some(String::new()),
                badge: Some(String::new()),
                background: None,
                foreground: None,
                enabled: Some(true),
                ..Overrides::default()
            },
            tabs: BTreeMap::new(),
            dnr_badges: BTreeMap::new(),
            revision: ExtensionActionRevision::INITIAL,
            is_on_toolbar: false,
        }
    }
}

/// Only a trusted toolbar or shortcut callback may construct this delivery.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Activation {
    /// Deliver action.onClicked with a permission-redacted tab object.
    Click,
    /// Present this packaged popup without delivering onClicked.
    Popup(String),
}

impl ActionStore {
    /// Initialize admitted manifest defaults. Rasterize default icons in the adapter.
    pub fn new(manifest: &Value, sender: &Sender) -> Result<Self, ApiError> {
        let mut store = Self::default();
        let action = manifest.get("action").unwrap_or(&Value::Null);
        store.defaults.enabled =
            Some(action.get("default_state").and_then(Value::as_str) != Some("disabled"));
        store.defaults.title = Some(
            action
                .get("default_title")
                .filter(|value| value.as_str().is_some_and(|title| !title.is_empty()))
                .or_else(|| manifest.get("name"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
        );
        if let Some(path) = action
            .get("default_popup")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
        {
            store.defaults.popup = Some(super::runtime::get_url(sender, path)?);
        }
        if let Some(icon) = action.get("default_icon").or_else(|| manifest.get("icons")) {
            store.defaults.icon = Some(Icon::Paths(paths(icon)?));
        }
        Ok(store)
    }

    fn state(&self, tab: Option<i32>) -> Overrides {
        let local = tab.and_then(|id| self.tabs.get(&id));
        let field = |get: fn(&Overrides) -> Option<String>| {
            local.and_then(get).or_else(|| get(&self.defaults))
        };
        let background = local
            .and_then(|s| s.background)
            .or(self.defaults.background)
            .unwrap_or([0, 0, 0, 0]);
        let foreground = local
            .and_then(|s| s.foreground)
            .or(self.defaults.foreground)
            .unwrap_or([0, 0, 0, 0]);
        Overrides {
            title: field(|s| s.title.clone()),
            popup: field(|s| s.popup.clone()),
            badge: tab
                .and_then(|tab| self.dnr_badges.get(&tab).cloned())
                .or_else(|| field(|s| s.badge.clone())),
            background: Some(background),
            foreground: Some(foreground),
            enabled: local.and_then(|s| s.enabled).or(self.defaults.enabled),
            icon: local
                .and_then(|s| s.icon.clone())
                .or_else(|| self.defaults.icon.clone()),
            raster: local
                .and_then(|s| s.raster.clone())
                .or_else(|| self.defaults.raster.clone()),
        }
    }

    /// Gets one effective value, including inheritance after a default changes.
    pub fn get(&self, tab: Option<i32>, property: Property) -> Value {
        let state = self.state(tab);
        match property {
            Property::Title => json!(state.title.unwrap_or_default()),
            Property::Popup => json!(state.popup.unwrap_or_default()),
            Property::BadgeText => json!(state.badge.unwrap_or_default()),
            Property::BadgeBackgroundColor => json!(state.background),
            Property::BadgeTextColor => json!(state.foreground),
            Property::Enabled => json!(state.enabled.unwrap_or(true)),
        }
    }

    /// Applies a validated update only after native resource decoding has succeeded.
    pub fn apply(&mut self, tab: Option<i32>, update: &Update) -> Result<(), ApiError> {
        match update {
            Update::Title(title) if title.len() > MAX_TITLE_BYTES => return Err(ApiError::message_too_large()),
            Update::BadgeText(Some(text)) if text.len() > MAX_BADGE_BYTES => return Err(ApiError::other(format!("Badge text size is {} bytes which exceeds the limit of {MAX_BADGE_BYTES} bytes.", text.len()))),
            Update::BadgeTextColor([_,_,_,0]) => return Err(ApiError::other("The color specification could not be parsed.")),
            _ => {}
        }
        let revision = self
            .revision
            .next()
            .ok_or_else(ApiError::context_invalidated)?;
        let state = match tab {
            Some(id) => self.tabs.entry(id).or_default(),
            None => &mut self.defaults,
        };
        match update {
            Update::Title(value) => state.title = Some(value.clone()),
            Update::Popup(value) => state.popup = Some(value.clone()),
            Update::BadgeText(value) => state.badge = value.clone(),
            Update::BadgeBackgroundColor(value) => state.background = Some(*value),
            Update::BadgeTextColor(value) => state.foreground = Some(*value),
            Update::Enabled(value) => state.enabled = Some(*value),
            Update::Icon(icon) => {
                state.raster = match icon {
                    Icon::Images(images) => images
                        .iter()
                        .min_by_key(|(size, _)| i32::from(**size).abs_diff(32))
                        .map(|(_, image)| raster(image))
                        .transpose()?,
                    Icon::Paths(_) => None,
                };
                state.icon = Some(icon.clone());
            }
        }
        self.revision = revision;
        Ok(())
    }

    /// Commit an adapter-decoded fixed raster together with a packaged icon update.
    pub fn set_raster_icon(&mut self, tab: Option<i32>, icon: ExtensionActionIcon) {
        match tab {
            Some(id) => self.tabs.entry(id).or_default().raster = Some(icon),
            None => self.defaults.raster = Some(icon),
        }
    }

    /// Sets the DNR count overlay without replacing extension-authored badge text.
    pub fn set_dnr_badge(&mut self, tab: i32, text: Option<&str>) -> Result<(), ApiError> {
        if text.is_some_and(|text| text.len() > MAX_BADGE_BYTES) {
            return Err(ApiError::message_too_large());
        }
        if self.dnr_badges.get(&tab).map(String::as_str) == text {
            return Ok(());
        }
        let revision = self
            .revision
            .next()
            .ok_or_else(ApiError::context_invalidated)?;
        if let Some(text) = text {
            self.dnr_badges.insert(tab, text.to_owned());
        } else {
            self.dnr_badges.remove(&tab);
        }
        self.revision = revision;
        Ok(())
    }

    /// Navigation and tab close clear the complete override, including badge/icon/enabled.
    pub fn reset_tab(&mut self, tab: i32) -> Result<(), ApiError> {
        if self.tabs.contains_key(&tab) || self.dnr_badges.contains_key(&tab) {
            let revision = self
                .revision
                .next()
                .ok_or_else(ApiError::context_invalidated)?;
            self.tabs.remove(&tab);
            self.dnr_badges.remove(&tab);
            self.revision = revision;
        }
        Ok(())
    }

    /// Produce the existing bounded Shell projection without changing API getter data.
    pub fn shell_state(
        &self,
        tab_id: i32,
        tab: ItemId,
        runtime: ExtensionRuntimeInstance,
    ) -> Result<ExtensionActionState, ApiError> {
        let state = self.state(Some(tab_id));
        let title = display_text(
            &state.title.unwrap_or_default(),
            MAX_EXTENSION_ACTION_LABEL_BYTES,
        );
        let badge = display_text(
            &state.badge.unwrap_or_default(),
            MAX_EXTENSION_ACTION_BADGE_BYTES,
        );
        ExtensionActionState::new(
            runtime,
            ExtensionActionScope::Tab(tab),
            self.revision,
            &title,
            &badge,
            state.raster,
            state.enabled.unwrap_or(true),
            state.popup.is_some_and(|s| !s.is_empty()),
            !badge.is_empty(),
        )
        .map_err(|_| ApiError::other("This extension has no action specified."))
    }

    /// Native input entry point. Grant before delivering onClicked or presenting a popup.
    pub fn trusted_invoke(
        &self,
        tab: &BrowserTab,
        permissions: &mut InstallPermissions,
    ) -> Result<Activation, ApiError> {
        let state = self.state(Some(tab.id));
        if state.enabled == Some(false) {
            return Err(ApiError::other("This extension has no action specified."));
        }
        if let Ok(url) = tab.url() {
            permissions.grant_active_tab(tab.tab.id(), tab.document, &url);
        }
        Ok(match state.popup.filter(|url| !url.is_empty()) {
            Some(url) => Activation::Popup(url),
            None => Activation::Click,
        })
    }

    /// In-memory request handler for adapter state and deterministic test hosts.
    /// Native adapters must own popup admission and actual toolbar pin observations.
    pub fn handle(&mut self, request: &Request) -> Result<Value, ApiError> {
        match request {
            Request::Get { tab_id, property } => Ok(self.get(*tab_id, *property)),
            Request::Set { tab_id, update } => {
                self.apply(*tab_id, update)?;
                Ok(Value::Null)
            }
            Request::GetUserSettings => Ok(json!({"isOnToolbar":self.is_on_toolbar})),
            Request::OpenPopup { tab_id, .. } => {
                let state = self.state(Some(*tab_id));
                if state.enabled != Some(true) || state.popup.is_none_or(|url| url.is_empty()) {
                    return Err(ApiError::other(
                        "Extension does not have a popup on the active tab.",
                    ));
                }
                Ok(Value::Null)
            }
        }
    }
}

fn display_text(value: &str, limit: usize) -> String {
    let mut result = String::new();
    for ch in value.chars().filter(|ch| !ch.is_control()) {
        if result.len() + ch.len_utf8() > limit {
            break;
        }
        result.push(ch);
    }
    result
}

fn paths(value: &Value) -> Result<BTreeMap<u16, String>, ApiError> {
    let invalid = || ApiError::other("Icon invalid.");
    if let Some(path) = value.as_str() {
        if path.is_empty() || path.len() > 4096 {
            return Err(invalid());
        }
        return Ok(BTreeMap::from([(32, path.to_owned())]));
    }
    let entries = value
        .as_object()
        .filter(|values| !values.is_empty() && values.len() <= MAX_ICON_REPRESENTATIONS)
        .ok_or_else(invalid)?;
    entries
        .iter()
        .map(|(size, path)| {
            let size = size
                .parse::<u16>()
                .ok()
                .filter(|size| (1..=MAX_ICON_DIMENSION).contains(size))
                .ok_or_else(invalid)?;
            let path = path
                .as_str()
                .filter(|s| !s.is_empty() && s.len() <= 4096)
                .ok_or_else(invalid)?;
            Ok((size, path.to_owned()))
        })
        .collect()
}

fn image(value: &Value) -> Result<RgbaImage, ApiError> {
    let invalid = || ApiError::other("Icon invalid.");
    let dimension = |key| {
        value
            .get(key)
            .and_then(Value::as_u64)
            .filter(|n| (1..=u64::from(MAX_ICON_DIMENSION)).contains(n))
            .map(|n| n as u16)
            .ok_or_else(invalid)
    };
    let width = dimension("width")?;
    let height = dimension("height")?;
    let data = value
        .get("data")
        .and_then(Value::as_array)
        .filter(|bytes| bytes.len() == usize::from(width) * usize::from(height) * 4)
        .ok_or_else(invalid)?;
    let data = data
        .iter()
        .map(|value| {
            value
                .as_u64()
                .and_then(|n| u8::try_from(n).ok())
                .ok_or_else(invalid)
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(RgbaImage {
        width,
        height,
        data,
    })
}

fn images(value: &Value) -> Result<BTreeMap<u16, RgbaImage>, ApiError> {
    if value.get("width").is_some() {
        let image = image(value)?;
        return Ok(BTreeMap::from([(image.width, image)]));
    }
    let invalid = || ApiError::other("Icon invalid.");
    value
        .as_object()
        .filter(|values| !values.is_empty() && values.len() <= MAX_ICON_REPRESENTATIONS)
        .ok_or_else(invalid)?
        .iter()
        .map(|(size, value)| {
            let size = size
                .parse::<u16>()
                .ok()
                .filter(|size| (1..=MAX_ICON_DIMENSION).contains(size))
                .ok_or_else(invalid)?;
            Ok((size, image(value)?))
        })
        .collect()
}

fn raster(image: &RgbaImage) -> Result<ExtensionActionIcon, ApiError> {
    if image.width == 0
        || image.height == 0
        || image.width > MAX_ICON_DIMENSION
        || image.height > MAX_ICON_DIMENSION
        || image.data.len() != usize::from(image.width) * usize::from(image.height) * 4
    {
        return Err(ApiError::other("Icon invalid."));
    }
    let mut pixels = vec![0; EXTENSION_ACTION_ICON_WIDTH * EXTENSION_ACTION_ICON_HEIGHT * 4];
    for y in 0..EXTENSION_ACTION_ICON_HEIGHT {
        for x in 0..EXTENSION_ACTION_ICON_WIDTH {
            let source = (y * usize::from(image.height) / EXTENSION_ACTION_ICON_HEIGHT
                * usize::from(image.width)
                + x * usize::from(image.width) / EXTENSION_ACTION_ICON_WIDTH)
                * 4;
            let target = (y * EXTENSION_ACTION_ICON_WIDTH + x) * 4;
            pixels[target..target + 4].copy_from_slice(&image.data[source..source + 4]);
        }
    }
    ExtensionActionIcon::from_rgba(pixels).map_err(|_| ApiError::other("Icon invalid."))
}

fn handle(host: &mut impl BrowserPort, call: Invocation<'_>) -> Handled {
    let info = match host.extension_info(call.sender()) {
        Ok(info) => info,
        Err(error) => return call.reply(Err(error)),
    };
    if info.manifest.get("action").is_none() {
        return call.reply(Err(ApiError::other(
            "This extension has no action specified.",
        )));
    }
    if call.name() == "action.getUserSettings" {
        return host.request(BrowserRequest::Action(Request::GetUserSettings), call);
    }
    let details = call.args().first().unwrap_or(&Value::Null);
    let tab_id = details
        .as_i64()
        .or_else(|| details.get("tabId").and_then(Value::as_i64))
        .map(|id| id as i32);
    if let Some(id) = tab_id {
        let result =
            snapshot(host, call.sender()).and_then(|snapshot| snapshot.tab(id).map(|_| ()));
        if let Err(error) = result {
            return call.reply(Err(error));
        }
    }
    if call.name() == "action.openPopup" {
        let request = snapshot(host, call.sender()).and_then(|snapshot| {
            let id = details.get("windowId").and_then(Value::as_i64).map(|id| id as i32).or(snapshot.current_window)
                .ok_or_else(|| ApiError::other("Could not find an active browser window."))?;
            let window = snapshot.window(id)?;
            if !window.focused { return Err(ApiError::other("Cannot show popup for an inactive window. To show the popup for this window, first call `chrome.windows.update` with `focused` set to true.")); }
            let tab = snapshot.tabs.iter().find(|tab| tab.window_id == window.id && tab.active)
                .ok_or_else(|| ApiError::other("Extension does not have a popup on the active tab."))?;
            Ok(Request::OpenPopup { window_id:window.id, tab_id:tab.id })
        });
        return match request {
            Ok(request) => host.request(BrowserRequest::Action(request), call),
            Err(error) => call.reply(Err(error)),
        };
    }
    let property = match call.name() {
        "action.getTitle" => Some(Property::Title),
        "action.getPopup" => Some(Property::Popup),
        "action.getBadgeText" => Some(Property::BadgeText),
        "action.getBadgeBackgroundColor" => Some(Property::BadgeBackgroundColor),
        "action.getBadgeTextColor" => Some(Property::BadgeTextColor),
        "action.isEnabled" => Some(Property::Enabled),
        _ => None,
    };
    if let Some(property) = property {
        return host.request(
            BrowserRequest::Action(Request::Get { tab_id, property }),
            call,
        );
    }
    let update = (|| {
        Ok(match call.name() {
            "action.enable" => Update::Enabled(true),
            "action.disable" => Update::Enabled(false),
            "action.setTitle" => {
                let title = details["title"].as_str().unwrap_or_default();
                if title.len() > MAX_TITLE_BYTES {
                    return Err(ApiError::message_too_large());
                }
                Update::Title(title.to_owned())
            }
            "action.setPopup" => {
                let path = details["popup"].as_str().unwrap_or_default();
                Update::Popup(if path.is_empty() {
                    String::new()
                } else {
                    super::runtime::get_url(call.sender(), path)
                        .map_err(|_| ApiError::other("The specified popup path is invalid. Ensure it is a path to a file in this extension."))?
                })
            }
            "action.setBadgeText" => {
                let value = details
                    .get("text")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
                if value.as_ref().is_some_and(|s| s.len() > MAX_BADGE_BYTES) {
                    return Err(ApiError::other(format!("Badge text size is {} bytes which exceeds the limit of {MAX_BADGE_BYTES} bytes.",value.as_ref().map_or(0,String::len))));
                }
                Update::BadgeText(value)
            }
            "action.setBadgeBackgroundColor" => {
                Update::BadgeBackgroundColor(color(&details["color"])?)
            }
            "action.setBadgeTextColor" => {
                let value = color(&details["color"])?;
                if value[3] == 0 {
                    return Err(ApiError::other(
                        "The color specification could not be parsed.",
                    ));
                }
                Update::BadgeTextColor(value)
            }
            "action.setIcon" => {
                if details.get("path").is_some() == details.get("imageData").is_some() {
                    return Err(ApiError::other("Icon invalid."));
                }
                let icon = if let Some(value) = details.get("path") {
                    let paths = paths(value)?;
                    for path in paths.values() {
                        host.validate_resource(call.sender(), path)?;
                    }
                    Icon::Paths(paths)
                } else {
                    Icon::Images(images(&details["imageData"])?)
                };
                Update::Icon(icon)
            }
            _ => return Err(crate::browser::unsupported()),
        })
    })();
    match update {
        Ok(update) => host.request(
            BrowserRequest::Action(Request::Set { tab_id, update }),
            call,
        ),
        Err(error) => call.reply(Err(error)),
    }
}

/// Registers every generated action method using centralized context gates.
pub fn register<S: BrowserPort + 'static>(
    table: &mut ApiTable<S>,
) -> Result<(), RegistrationError> {
    for name in METHODS {
        table.register_generated(name, |host: &mut S, call| handle(host, call))?;
    }
    Ok(())
}

fn color(value: &Value) -> Result<[u8; 4], ApiError> {
    let invalid = || ApiError::other("The color specification could not be parsed.");
    if let Some(values) = value.as_array().filter(|values| values.len() == 4) {
        let mut result = [0; 4];
        for (out, input) in result.iter_mut().zip(values) {
            *out = input
                .as_i64()
                .map(|n| n.rem_euclid(256) as u8)
                .ok_or_else(invalid)?;
        }
        return Ok(result);
    }
    let source = value
        .as_str()
        .ok_or_else(invalid)?
        .trim()
        .to_ascii_lowercase();
    if let Some(hex) = source.strip_prefix('#') {
        return hex_color(hex).ok_or_else(invalid);
    }
    if source == "transparent" {
        return Ok([0, 0, 0, 0]);
    }
    if let Some(hex) = NAMED_COLORS.split_ascii_whitespace().find_map(|pair| {
        pair.split_once(':')
            .filter(|(name, _)| *name == source)
            .map(|(_, hex)| hex)
    }) {
        return hex_color(hex).ok_or_else(invalid);
    }
    let (function, args) = source.split_once('(').ok_or_else(invalid)?;
    let args = args.strip_suffix(')').ok_or_else(invalid)?;
    let parts = if args.contains(',') {
        if args.contains('/') {
            return Err(invalid());
        }
        let parts = args.split(',').map(str::trim).collect::<Vec<_>>();
        if parts.iter().any(|part| part.is_empty()) {
            return Err(invalid());
        }
        parts
    } else {
        let (channels, alpha) = args
            .split_once('/')
            .map_or((args, None), |(channels, alpha)| {
                (channels, Some(alpha.trim()))
            });
        let mut parts = channels.split_ascii_whitespace().collect::<Vec<_>>();
        if parts.len() != 3 || alpha.is_some_and(|alpha| alpha.is_empty() || alpha.contains('/')) {
            return Err(invalid());
        }
        if let Some(alpha) = alpha {
            parts.push(alpha);
        }
        parts
    };
    if !(3..=4).contains(&parts.len()) {
        return Err(invalid());
    }
    let number = |source: &str, scale: f64| -> Result<f64, ApiError> {
        let (source, scale) = source
            .strip_suffix('%')
            .map_or((source, 1.0), |s| (s, scale / 100.0));
        let number = source.parse::<f64>().map_err(|_| invalid())? * scale;
        if !number.is_finite() {
            return Err(invalid());
        }
        Ok(number)
    };
    let alpha = if parts.len() == 4 {
        (number(parts[3], 1.0)?.clamp(0.0, 1.0) * 255.0).round() as u8
    } else {
        255
    };
    if matches!(function, "rgb" | "rgba") {
        return Ok([
            (number(parts[0], 255.0)?.clamp(0.0, 255.0)).round() as u8,
            (number(parts[1], 255.0)?.clamp(0.0, 255.0)).round() as u8,
            (number(parts[2], 255.0)?.clamp(0.0, 255.0)).round() as u8,
            alpha,
        ]);
    }
    if matches!(function, "hsl" | "hsla") {
        let (hue, scale) = if let Some(value) = parts[0].strip_suffix("deg") {
            (value, 1.0)
        } else if let Some(value) = parts[0].strip_suffix("grad") {
            (value, 0.9)
        } else if let Some(value) = parts[0].strip_suffix("turn") {
            (value, 360.0)
        } else if let Some(value) = parts[0].strip_suffix("rad") {
            (value, 180.0 / std::f64::consts::PI)
        } else {
            (parts[0], 1.0)
        };
        let hue = hue.parse::<f64>().map_err(|_| invalid())? * scale;
        if !hue.is_finite() || !parts[1].ends_with('%') || !parts[2].ends_with('%') {
            return Err(invalid());
        }
        let h = hue.rem_euclid(360.0) / 60.0;
        let s = number(parts[1], 1.0)?.clamp(0.0, 1.0);
        let l = number(parts[2], 1.0)?.clamp(0.0, 1.0);
        let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
        let x = c * (1.0 - (h % 2.0 - 1.0).abs());
        let (r, g, b) = match h as u8 {
            0 => (c, x, 0.0),
            1 => (x, c, 0.0),
            2 => (0.0, c, x),
            3 => (0.0, x, c),
            4 => (x, 0.0, c),
            _ => (c, 0.0, x),
        };
        let m = l - c / 2.0;
        return Ok([
            ((r + m) * 255.0).round() as u8,
            ((g + m) * 255.0).round() as u8,
            ((b + m) * 255.0).round() as u8,
            alpha,
        ]);
    }
    Err(invalid())
}

fn hex_color(hex: &str) -> Option<[u8; 4]> {
    if !hex.is_ascii() {
        return None;
    }
    let mut result = [0, 0, 0, 255];
    match hex.len() {
        3 | 4 => {
            for (i, byte) in hex.bytes().enumerate() {
                result[i] = (byte as char).to_digit(16)? as u8 * 17;
            }
        }
        6 | 8 => {
            for i in 0..hex.len() / 2 {
                result[i] = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).ok()?;
            }
        }
        _ => return None,
    }
    Some(result)
}

const NAMED_COLORS: &str = "aliceblue:f0f8ff antiquewhite:faebd7 aqua:00ffff aquamarine:7fffd4 azure:f0ffff beige:f5f5dc bisque:ffe4c4 black:000000 blanchedalmond:ffebcd blue:0000ff blueviolet:8a2be2 brown:a52a2a burlywood:deb887 cadetblue:5f9ea0 chartreuse:7fff00 chocolate:d2691e coral:ff7f50 cornflowerblue:6495ed cornsilk:fff8dc crimson:dc143c cyan:00ffff darkblue:00008b darkcyan:008b8b darkgoldenrod:b8860b darkgray:a9a9a9 darkgrey:a9a9a9 darkgreen:006400 darkkhaki:bdb76b darkmagenta:8b008b darkolivegreen:556b2f darkorange:ff8c00 darkorchid:9932cc darkred:8b0000 darksalmon:e9967a darkseagreen:8fbc8f darkslateblue:483d8b darkslategray:2f4f4f darkslategrey:2f4f4f darkturquoise:00ced1 darkviolet:9400d3 deeppink:ff1493 deepskyblue:00bfff dimgray:696969 dimgrey:696969 dodgerblue:1e90ff firebrick:b22222 floralwhite:fffaf0 forestgreen:228b22 fuchsia:ff00ff gainsboro:dcdcdc ghostwhite:f8f8ff gold:ffd700 goldenrod:daa520 gray:808080 grey:808080 green:008000 greenyellow:adff2f honeydew:f0fff0 hotpink:ff69b4 indianred:cd5c5c indigo:4b0082 ivory:fffff0 khaki:f0e68c lavender:e6e6fa lavenderblush:fff0f5 lawngreen:7cfc00 lemonchiffon:fffacd lightblue:add8e6 lightcoral:f08080 lightcyan:e0ffff lightgoldenrodyellow:fafad2 lightgray:d3d3d3 lightgrey:d3d3d3 lightgreen:90ee90 lightpink:ffb6c1 lightsalmon:ffa07a lightseagreen:20b2aa lightskyblue:87cefa lightslategray:778899 lightslategrey:778899 lightsteelblue:b0c4de lightyellow:ffffe0 lime:00ff00 limegreen:32cd32 linen:faf0e6 magenta:ff00ff maroon:800000 mediumaquamarine:66cdaa mediumblue:0000cd mediumorchid:ba55d3 mediumpurple:9370db mediumseagreen:3cb371 mediumslateblue:7b68ee mediumspringgreen:00fa9a mediumturquoise:48d1cc mediumvioletred:c71585 midnightblue:191970 mintcream:f5fffa mistyrose:ffe4e1 moccasin:ffe4b5 navajowhite:ffdead navy:000080 oldlace:fdf5e6 olive:808000 olivedrab:6b8e23 orange:ffa500 orangered:ff4500 orchid:da70d6 palegoldenrod:eee8aa palegreen:98fb98 paleturquoise:afeeee palevioletred:db7093 papayawhip:ffefd5 peachpuff:ffdab9 peru:cd853f pink:ffc0cb plum:dda0dd powderblue:b0e0e6 purple:800080 rebeccapurple:663399 red:ff0000 rosybrown:bc8f8f royalblue:4169e1 saddlebrown:8b4513 salmon:fa8072 sandybrown:f4a460 seagreen:2e8b57 seashell:fff5ee sienna:a0522d silver:c0c0c0 skyblue:87ceeb slateblue:6a5acd slategray:708090 slategrey:708090 snow:fffafa springgreen:00ff7f steelblue:4682b4 tan:d2b48c teal:008080 thistle:d8bfd8 tomato:ff6347 turquoise:40e0d0 violet:ee82ee wheat:f5deb3 white:ffffff whitesmoke:f5f5f5 yellow:ffff00 yellowgreen:9acd32";

#[cfg(test)]
mod tests;
