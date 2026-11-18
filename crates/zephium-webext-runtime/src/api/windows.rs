//! Native browser window management with caller-scoped tab projection.

use serde::Deserialize;
use serde_json::{json, Value};

use super::tabs;
use crate::browser::{self, BrowserPort, BrowserRequest, BrowserSnapshot, BrowserWindow};
use crate::dispatch::{ApiTable, Handled, Invocation, RegistrationError};
use crate::permissions::InstallPermissions;
use crate::ApiError;

pub(crate) const METHODS: &[&str] = &[
    "windows.get",
    "windows.getCurrent",
    "windows.getLastFocused",
    "windows.getAll",
    "windows.create",
    "windows.update",
    "windows.remove",
];

/// Options accepted by window creation and update requests.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Properties {
    /// Absolute URL or URLs to open during creation.
    pub url: Option<Value>,
    /// Existing tab to transfer into the new window.
    pub tab_id: Option<i32>,
    /// Horizontal position.
    pub left: Option<i32>,
    /// Vertical position.
    pub top: Option<i32>,
    /// Frame width in logical pixels.
    pub width: Option<i32>,
    /// Frame height in logical pixels.
    pub height: Option<i32>,
    /// Requested focus state.
    pub focused: Option<bool>,
    /// Private profile request; may never cross the invoking profile.
    pub incognito: Option<bool>,
    /// Browser window type, normal or popup.
    #[serde(rename = "type")]
    pub kind: Option<String>,
    /// Requested presentation state.
    pub state: Option<String>,
    /// Request or cancel desktop attention without changing focus.
    pub draw_attention: Option<bool>,
    /// Native opener binding; requires adapter support.
    pub set_self_as_opener: Option<bool>,
}

/// Native window requests; replies describe settled browser state.
#[derive(Clone, Debug)]
pub enum Request {
    /// Create a normal or popup window, returning a populated window.
    Create {
        /// Complete validated creation options.
        properties: Properties,
    },
    /// Update an existing window and return its settled projection.
    Update {
        /// Chrome identity.
        window_id: i32,
        /// Core native identity.
        native_id: zephium_core::ids::WindowId,
        /// Requested updates.
        properties: Properties,
    },
    /// Close a window and every tab inside it.
    Remove {
        /// Chrome identity.
        window_id: i32,
        /// Core native identity.
        native_id: zephium_core::ids::WindowId,
    },
}

/// Registers every generated windows method.
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
    match prepare(&snapshot, &invocation) {
        Ok(Prepared::Reply(value)) => invocation.reply(Ok(value)),
        Ok(Prepared::Request(request)) => {
            host.request(BrowserRequest::Windows(request), invocation)
        }
        Err(error) => invocation.reply(Err(error)),
    }
}

enum Prepared {
    Reply(Value),
    Request(Request),
}

fn prepare(snapshot: &BrowserSnapshot, call: &Invocation<'_>) -> Result<Prepared, ApiError> {
    let args = call.args();
    let sender = call.sender();
    let request = match call.name() {
        "windows.get" | "windows.getCurrent" | "windows.getLastFocused" | "windows.getAll" => {
            let options = args
                .get(usize::from(call.name() == "windows.get"))
                .unwrap_or(&Value::Null);
            let populate = options
                .get("populate")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let allowed = |window: &&BrowserWindow| match options
                .get("windowTypes")
                .and_then(Value::as_array)
            {
                Some(types) => types
                    .iter()
                    .any(|kind| kind.as_str() == Some(window.kind.as_str())),
                None => matches!(window.kind.as_str(), "normal" | "popup"),
            };
            if call.name() == "windows.getAll" {
                return Ok(Prepared::Reply(Value::Array(
                    snapshot
                        .windows
                        .iter()
                        .filter(allowed)
                        .map(|window| {
                            project_for(snapshot, window, populate, sender, call.permissions())
                        })
                        .collect(),
                )));
            }
            let id = match call.name() {
                "windows.get" => tabs::current_window(
                    snapshot,
                    sender,
                    args[0].as_i64().and_then(|id| i32::try_from(id).ok()),
                )?,
                "windows.getCurrent" => tabs::current_window(snapshot, sender, None)?,
                _ => snapshot
                    .last_focused_window
                    .ok_or_else(|| ApiError::other("No last-focused window."))?,
            };
            let window = snapshot
                .windows
                .iter()
                .filter(allowed)
                .find(|window| window.id == id)
                .ok_or_else(|| ApiError::other(format!("No window with id: {id}.")))?;
            return Ok(Prepared::Reply(project_for(
                snapshot,
                window,
                populate,
                sender,
                call.permissions(),
            )));
        }
        "windows.create" => {
            let value = args
                .first()
                .filter(|value| !value.is_null())
                .cloned()
                .unwrap_or_else(|| json!({}));
            let mut properties: Properties =
                serde_json::from_value(value).map_err(|_| ApiError::malformed_arguments())?;
            validate(&properties)?;
            if properties
                .incognito
                .is_some_and(|incognito| incognito != sender.incognito())
            {
                return Err(ApiError::other("Incognito mode is disabled."));
            }
            properties.incognito = Some(sender.incognito());
            if properties
                .kind
                .as_deref()
                .is_some_and(|kind| !matches!(kind, "normal" | "popup"))
                || properties.set_self_as_opener == Some(true)
            {
                return Err(browser::unsupported());
            }
            if let Some(id) = properties.tab_id {
                snapshot.tab(id)?;
            }
            if let Some(value) = properties.url.take() {
                let value = match value {
                    Value::String(url) => Value::String(tabs::navigation_url(sender, &url)?),
                    Value::Array(urls) => Value::Array(
                        urls.iter()
                            .map(|url| {
                                url.as_str()
                                    .ok_or_else(ApiError::malformed_arguments)
                                    .and_then(|url| tabs::navigation_url(sender, url))
                                    .map(Value::String)
                            })
                            .collect::<Result<_, _>>()?,
                    ),
                    _ => return Err(ApiError::malformed_arguments()),
                };
                properties.url = Some(value);
            }
            Request::Create { properties }
        }
        "windows.update" | "windows.remove" => {
            let id = args[0]
                .as_i64()
                .and_then(|id| i32::try_from(id).ok())
                .ok_or_else(ApiError::malformed_arguments)?;
            let id = tabs::current_window(snapshot, sender, Some(id))?;
            let window = snapshot.window(id)?;
            if call.name() == "windows.remove" {
                Request::Remove {
                    window_id: id,
                    native_id: window.native_id,
                }
            } else {
                let properties: Properties = serde_json::from_value(args[1].clone())
                    .map_err(|_| ApiError::malformed_arguments())?;
                validate(&properties)?;
                Request::Update {
                    window_id: id,
                    native_id: window.native_id,
                    properties,
                }
            }
        }
        _ => return Err(ApiError::unknown_method()),
    };
    Ok(Prepared::Request(request))
}

fn validate(properties: &Properties) -> Result<(), ApiError> {
    if properties
        .state
        .as_deref()
        .is_some_and(|state| matches!(state, "minimized" | "maximized" | "fullscreen"))
        && [
            properties.left,
            properties.top,
            properties.width,
            properties.height,
        ]
        .iter()
        .any(Option::is_some)
    {
        return Err(ApiError::other("The 'minimized', 'maximized', and 'fullscreen' states cannot be combined with 'left', 'top', 'width', or 'height'."));
    }
    if properties.state.as_deref() == Some("minimized") && properties.focused == Some(true) {
        return Err(ApiError::other("Cannot focus a minimized window."));
    }
    if properties.focused == Some(false)
        && properties
            .state
            .as_deref()
            .is_some_and(|state| matches!(state, "maximized" | "fullscreen"))
    {
        return Err(ApiError::other(
            "Cannot unfocus a maximized or fullscreen window.",
        ));
    }
    if properties.state.as_deref() == Some("locked-fullscreen") {
        return Err(browser::unsupported());
    }
    Ok(())
}

/// Chrome window projection. Populated tab objects retain the caller's read restrictions.
pub fn project(
    snapshot: &BrowserSnapshot,
    window: &BrowserWindow,
    populate: bool,
    permissions: &InstallPermissions,
) -> Value {
    let [left, top, width, height] = window.bounds;
    let mut value = json!({"id":window.id,"focused":window.focused,"incognito":window.incognito,"type":window.kind,"state":window.state,"alwaysOnTop":window.always_on_top,"left":left,"top":top,"width":width,"height":height});
    if populate {
        let mut tabs: Vec<_> = snapshot
            .tabs
            .iter()
            .filter(|tab| tab.window_id == window.id)
            .collect();
        tabs.sort_by_key(|tab| tab.index);
        value["tabs"] = Value::Array(
            tabs.into_iter()
                .map(|tab| tabs::project(tab, permissions))
                .collect(),
        );
    }
    value
}

/// Caller-aware populated window projection, including access to the install's own pages.
pub fn project_for(
    snapshot: &BrowserSnapshot,
    window: &BrowserWindow,
    populate: bool,
    sender: &crate::context::Sender,
    permissions: &InstallPermissions,
) -> Value {
    let mut value = project(snapshot, window, false, permissions);
    if populate {
        let mut tabs: Vec<_> = snapshot
            .tabs
            .iter()
            .filter(|tab| tab.window_id == window.id)
            .collect();
        tabs.sort_by_key(|tab| tab.index);
        value["tabs"] = Value::Array(
            tabs.into_iter()
                .map(|tab| tabs::project_for(tab, sender, permissions))
                .collect(),
        );
    }
    value
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browser::{fake::FakeBrowser, IncognitoMode};
    use crate::context::tests::{background_sender, profile};
    use crate::permissions::{PermissionDeclaration, PermissionRequest, SiteAccess};
    use zephium_webext::manifest::Manifest;

    fn permissions(tabs: bool) -> InstallPermissions {
        let (declaration, _) = PermissionDeclaration::from_manifest(&Manifest::from_value(
            json!({"manifest_version":3,"permissions":if tabs{vec!["tabs"]}else{Vec::<&str>::new()}}),
        ));
        InstallPermissions::new(
            declaration,
            &PermissionRequest::default(),
            SiteAccess::OnAllSites,
            false,
        )
    }

    #[test]
    fn populated_windows_share_tab_redaction_and_default_type_filters() {
        let sender = background_sender();
        let mut browser = FakeBrowser::new(profile());
        let mut dispatcher = FakeBrowser::dispatcher();
        let normal = browser.add_window(false);
        browser.add_tab(normal, "https://example.com/", true);
        let popup = browser.add_window(false);
        browser.snapshot.windows[1].kind = "popup".into();
        browser.add_tab(popup, "https://popup.test/", true);
        browser.add_window(false);
        browser.snapshot.windows[2].kind = "devtools".into();
        for tabs in [false, true] {
            let result = browser
                .call(
                    &mut dispatcher,
                    &sender,
                    &permissions(tabs),
                    "windows.getAll",
                    json!([{"populate":true}]),
                )
                .unwrap();
            assert_eq!(result.as_array().unwrap().len(), 2);
            assert_eq!(result[0]["tabs"][0].get("url").is_some(), tabs);
            assert_eq!(result[1]["type"], "popup");
        }
        let all = browser
            .call(
                &mut dispatcher,
                &sender,
                &permissions(false),
                "windows.getAll",
                json!([{"windowTypes":["devtools"]}]),
            )
            .unwrap();
        assert_eq!(all.as_array().unwrap().len(), 1);
        assert!(all[0].get("tabs").is_none());
        assert!(browser
            .call(
                &mut dispatcher,
                &sender,
                &permissions(false),
                "windows.get",
                json!([normal,{"windowTypes":["popup"]}])
            )
            .is_err());
    }

    #[test]
    fn popup_creation_relative_urls_bounds_and_focus_settle_into_actual_state() {
        let sender = background_sender();
        let mut browser = FakeBrowser::new(profile());
        let mut dispatcher = FakeBrowser::dispatcher();
        let first = browser.add_window(false);
        browser.add_tab(first, "https://example.com/", true);
        let created=browser.call(&mut dispatcher,&sender,&permissions(true),"windows.create",json!([{"url":"popup.html","type":"popup","focused":false,"left":15,"top":25,"width":400,"height":300}])).unwrap();
        assert_eq!(created["type"], "popup");
        assert_eq!(created["focused"], false);
        assert_eq!(created["left"], 15);
        assert!(created["tabs"][0]["url"]
            .as_str()
            .unwrap()
            .ends_with("/popup.html"));
        let id = created["id"].as_i64().unwrap();
        let updated = browser
            .call(
                &mut dispatcher,
                &sender,
                &permissions(false),
                "windows.update",
                json!([id,{"focused":true,"width":500}]),
            )
            .unwrap();
        assert_eq!(updated["focused"], true);
        assert_eq!(updated["width"], 500);
        assert_eq!(updated["height"], 300);
        let last = browser
            .call(
                &mut dispatcher,
                &sender,
                &permissions(false),
                "windows.getLastFocused",
                json!([]),
            )
            .unwrap();
        assert_eq!(last["id"], id);
        assert_eq!(
            browser
                .snapshot
                .windows
                .iter()
                .filter(|window| window.focused)
                .count(),
            1
        );
        assert!(browser
            .events
            .iter()
            .any(|(name, args)| name == "windows.onFocusChanged" && args[0] == id));
    }

    #[test]
    fn last_focused_survives_application_unfocus() {
        let sender = background_sender();
        let mut browser = FakeBrowser::new(profile());
        let mut dispatcher = FakeBrowser::dispatcher();
        let window = browser.add_window(false);
        browser.add_tab(window, "https://example.com/", true);
        browser
            .call(
                &mut dispatcher,
                &sender,
                &permissions(false),
                "windows.update",
                json!([window,{"focused":false}]),
            )
            .unwrap();
        assert!(!browser.snapshot.windows[0].focused);
        assert_eq!(
            browser
                .call(
                    &mut dispatcher,
                    &sender,
                    &permissions(false),
                    "windows.getLastFocused",
                    json!([])
                )
                .unwrap()["id"],
            window
        );
        assert_eq!(
            browser
                .call(
                    &mut dispatcher,
                    &sender,
                    &permissions(false),
                    "windows.getCurrent",
                    json!([])
                )
                .unwrap()["id"],
            window
        );
    }

    #[test]
    fn create_can_transfer_existing_tab_and_remove_reports_window_closure() {
        let sender = background_sender();
        let mut browser = FakeBrowser::new(profile());
        let mut dispatcher = FakeBrowser::dispatcher();
        let source = browser.add_window(false);
        browser.add_tab(source, "https://example.com/", true);
        browser.add_tab(source, "https://other.test/", false);
        let created = browser
            .call(
                &mut dispatcher,
                &sender,
                &permissions(true),
                "windows.create",
                json!([{"tabId":1}]),
            )
            .unwrap();
        let destination = created["id"].as_i64().unwrap();
        assert_eq!(created["tabs"][0]["id"], 1);
        assert_eq!(created["tabs"].as_array().unwrap().len(), 1);
        assert!(browser
            .events
            .iter()
            .any(|(name, args)| name == "tabs.onDetached" && args[0] == 1));
        assert!(
            browser
                .events
                .iter()
                .any(|(name, args)| name == "tabs.onAttached"
                    && args[1]["newWindowId"] == destination)
        );
        browser.events.clear();
        browser
            .call(
                &mut dispatcher,
                &sender,
                &permissions(false),
                "windows.remove",
                json!([destination]),
            )
            .unwrap();
        assert!(browser.snapshot.tab(1).is_err());
        assert!(browser.snapshot.tab(2).is_ok());
        let events: Vec<_> = browser
            .events
            .iter()
            .filter(|(name, _)| name == "tabs.onRemoved" || name == "windows.onRemoved")
            .collect();
        assert_eq!(events[0].0, "tabs.onRemoved");
        assert_eq!(events[0].1[1]["isWindowClosing"], true);
        assert_eq!(events[1].0, "windows.onRemoved");
    }

    #[test]
    fn invalid_window_combinations_and_cross_privacy_creation_fail_before_mutation() {
        let sender = background_sender();
        let mut browser = FakeBrowser::new(profile());
        let mut dispatcher = FakeBrowser::dispatcher();
        let window = browser.add_window(false);
        browser.info.incognito_allowed = true;
        browser.info.incognito_mode = IncognitoMode::Spanning;
        for (name, args) in [
            ("windows.create", json!([{"incognito":true}])),
            ("windows.create", json!([{"type":"panel"}])),
            ("windows.create", json!([{"setSelfAsOpener":true}])),
            ("windows.create", json!([{"url":"javascript:alert(1)"}])),
            ("windows.create", json!([{"state":"maximized","width":400}])),
            (
                "windows.update",
                json!([window,{"focused":true,"state":"minimized"}]),
            ),
            (
                "windows.update",
                json!([window,{"focused":false,"state":"fullscreen"}]),
            ),
            ("windows.remove", json!([999])),
        ] {
            let count = browser.requests.len();
            assert!(
                browser
                    .call(&mut dispatcher, &sender, &permissions(false), name, args)
                    .is_err(),
                "{name}"
            );
            assert_eq!(browser.requests.len(), count);
        }
        assert_eq!(browser.snapshot.windows.len(), 1);
    }

    #[test]
    fn foreign_profile_and_private_surface_are_never_enumerated() {
        let sender = background_sender();
        let mut browser = FakeBrowser::new(zephium_core::ids::ProfileId::from(99));
        let mut dispatcher = FakeBrowser::dispatcher();
        browser.add_window(false);
        assert!(browser
            .call(
                &mut dispatcher,
                &sender,
                &permissions(true),
                "windows.getAll",
                json!([])
            )
            .is_err());
        browser.snapshot.profile = profile();
        browser.snapshot.windows[0].incognito = true;
        assert!(browser
            .call(
                &mut dispatcher,
                &sender,
                &permissions(true),
                "windows.getAll",
                json!([])
            )
            .is_err());
        assert!(browser.requests.is_empty());
    }
    #[test]
    fn complete_bounds_events_and_final_window_focus_removal_follow_settled_state() {
        let sender = background_sender();
        let mut browser = FakeBrowser::new(profile());
        let mut dispatcher = FakeBrowser::dispatcher();
        let id = browser.add_window(false);
        browser.add_tab(id, "https://example.com/", true);
        browser
            .call(
                &mut dispatcher,
                &sender,
                &permissions(false),
                "windows.update",
                json!([id,{"width":500}]),
            )
            .unwrap();
        let bounds = &browser
            .events
            .iter()
            .find(|(name, _)| name == "windows.onBoundsChanged")
            .unwrap()
            .1[0];
        assert_eq!(bounds["width"], 500);
        assert_eq!(bounds["height"], 768);
        assert_eq!(bounds["type"], "normal");
        assert_eq!(bounds["focused"], true);
        browser.events.clear();
        browser
            .call(
                &mut dispatcher,
                &sender,
                &permissions(false),
                "windows.remove",
                json!([id]),
            )
            .unwrap();
        assert!(browser
            .events
            .iter()
            .any(|(name, args)| name == "windows.onFocusChanged" && args == &json!([-1])));
        assert!(browser.snapshot.current_window.is_none());
        assert!(browser.snapshot.last_focused_window.is_none());
    }

    #[test]
    fn private_creation_stays_in_the_invoking_ephemeral_profile_and_records_no_history() {
        use crate::context::tests::{admission, committed, install, EXTENSION};
        use crate::context::{Authority, ExtensionScheme, ViewKind};
        let mut authority = Authority::new(profile(), true, ExtensionScheme::CHROME_EXTENSION);
        let mut admitted = admission(install(), EXTENSION, 1);
        admitted.allowed_in_incognito = true;
        authority.admit(admitted).unwrap();
        let view = authority
            .open_view(install(), ViewKind::Background)
            .unwrap();
        let sender = authority
            .bind_view(
                &view,
                &committed(
                    &format!("chrome-extension://{EXTENSION}/background.html"),
                    1,
                ),
            )
            .unwrap();
        let mut browser = FakeBrowser::new(profile());
        browser.info.incognito_allowed = true;
        browser.info.incognito_mode = IncognitoMode::Split;
        let mut dispatcher = FakeBrowser::dispatcher();
        let created = browser
            .call(
                &mut dispatcher,
                &sender,
                &permissions(true),
                "windows.create",
                json!([{}]),
            )
            .unwrap();
        assert_eq!(created["incognito"], true);
        assert_eq!(created["tabs"][0]["incognito"], true);
        assert_eq!(created["tabs"][0]["url"], "about:blank");
        assert_eq!(created["tabs"][0]["title"], "New Tab");
        let id = created["tabs"][0]["id"].as_i64().unwrap();
        browser
            .call(
                &mut dispatcher,
                &sender,
                &permissions(true),
                "tabs.update",
                json!([id,{"url":"https://private.test/"}]),
            )
            .unwrap();
        assert!(browser.history.is_empty());
    }
    #[test]
    fn default_regular_window_and_tab_creation_have_valid_blank_tab_records() {
        let sender = background_sender();
        let mut browser = FakeBrowser::new(profile());
        let mut dispatcher = FakeBrowser::dispatcher();
        let created = browser
            .call(
                &mut dispatcher,
                &sender,
                &permissions(true),
                "windows.create",
                json!([]),
            )
            .unwrap();
        assert_eq!(created["incognito"], false);
        assert_eq!(created["tabs"][0]["url"], "about:blank");
        assert_eq!(created["tabs"][0]["title"], "New Tab");
        let tab = browser
            .call(
                &mut dispatcher,
                &sender,
                &permissions(true),
                "tabs.create",
                json!([{}]),
            )
            .unwrap();
        assert_eq!(tab["url"], "about:blank");
        assert_eq!(tab["title"], "New Tab");
    }
}
