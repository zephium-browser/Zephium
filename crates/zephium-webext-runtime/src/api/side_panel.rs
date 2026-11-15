//! Side-panel configuration; native presentation remains an adapter operation.
use std::collections::BTreeMap;

use serde_json::{json, Value};

use crate::browser::BrowserPort;
use crate::dispatch::{ApiTable, RegistrationError};
use crate::ApiError;

/// Validated side-panel operation, scoped to the authenticated caller.
#[derive(Clone, Debug)]
pub enum Request {
    /// Read global or tab options.
    GetOptions(Option<i32>),
    /// Merge schema-validated options.
    SetOptions(Value),
    /// Read action-click behavior.
    GetBehavior,
    /// Set action-click behavior.
    SetBehavior(bool),
    /// Present a panel in a target window or tab.
    Open(Value),
    /// Close a panel in a target window or tab.
    Close(Value),
    /// Read the native panel position.
    GetLayout,
}

/// Configuration for one install. This does not claim a panel was presented.
#[derive(Default, Debug)]
pub struct OptionsStore {
    options: BTreeMap<Option<i32>, Value>,
    open_on_click: bool,
}

impl OptionsStore {
    /// Seed configuration from the admitted manifest.
    pub fn new(manifest: &Value) -> Self {
        let mut defaults = json!({"enabled":true});
        if let Some(path) = manifest
            .pointer("/side_panel/default_path")
            .and_then(Value::as_str)
        {
            defaults["path"] = path.into();
        }
        Self {
            options: BTreeMap::from([(None, defaults)]),
            open_on_click: false,
        }
    }

    /// Forget overrides when tabs leave this profile.
    pub fn retain_tabs(&mut self, mut live: impl FnMut(i32) -> bool) {
        self.options.retain(|tab, _| tab.is_none_or(&mut live));
    }

    /// Applies configuration only. A host without panel UI rejects presentation.
    pub fn configure(&mut self, request: Request) -> Result<Value, ApiError> {
        match request {
            Request::GetOptions(tab) => {
                let options = self
                    .options
                    .get(&tab)
                    .or_else(|| self.options.get(&None))
                    .cloned()
                    .unwrap_or_else(|| json!({"enabled":true}));
                Ok(options)
            }
            Request::SetOptions(options) => {
                let tab = options["tabId"].as_i64().map(|id| id as i32);
                if !self.options.contains_key(&tab) && self.options.len() >= 513 {
                    return Err(ApiError::message_too_large());
                }
                let mut merged = self
                    .options
                    .get(&tab)
                    .cloned()
                    .unwrap_or_else(|| json!({"enabled":true}));
                for (key, value) in options
                    .as_object()
                    .ok_or_else(ApiError::malformed_arguments)?
                {
                    merged[key] = value.clone();
                }
                self.options.insert(tab, merged);
                Ok(Value::Null)
            }
            Request::GetBehavior => Ok(json!({"openPanelOnActionClick":self.open_on_click})),
            Request::SetBehavior(true) => Err(crate::browser::unsupported()),
            Request::SetBehavior(false) => {
                self.open_on_click = false;
                Ok(Value::Null)
            }
            Request::Open(_) | Request::Close(_) | Request::GetLayout => {
                Err(crate::browser::unsupported())
            }
        }
    }
}

pub(crate) const METHODS: &[&str] = &[
    "sidePanel.getOptions",
    "sidePanel.setOptions",
    "sidePanel.getPanelBehavior",
    "sidePanel.setPanelBehavior",
    "sidePanel.open",
    "sidePanel.close",
    "sidePanel.getLayout",
];

/// Register methods with generated permission, context and argument metadata.
pub fn register<S: BrowserPort + 'static>(
    table: &mut ApiTable<S>,
) -> Result<(), RegistrationError> {
    for &name in METHODS {
        table.register_generated(name, move |host: &mut S, call| {
            let result = (|| {
                let args = call.args();
                if name == "sidePanel.open" {
                    if !host.user_gesture(call.sender()) {
                        return Err(ApiError::other(
                            "`sidePanel.open()` may only be called in response to a user gesture.",
                        ));
                    }
                    if args[0]["tabId"].is_null() && args[0]["windowId"].is_null() {
                        return Err(ApiError::other(
                            "At least one of `tabId` and `windowId` must be provided",
                        ));
                    }
                }
                if let Some(tab) = args.first().and_then(|v| v["tabId"].as_i64()) {
                    let tab = i32::try_from(tab).map_err(|_| ApiError::malformed_arguments())?;
                    crate::browser::snapshot(host, call.sender())?.tab(tab)?;
                }
                if name == "sidePanel.setOptions" {
                    if let Some(path) = args[0]["path"].as_str() {
                        host.validate_resource(call.sender(), path)?;
                    }
                }
                let request = match name {
                    "sidePanel.getOptions" => {
                        Request::GetOptions(args[0]["tabId"].as_i64().map(|id| id as i32))
                    }
                    "sidePanel.setOptions" => Request::SetOptions(args[0].clone()),
                    "sidePanel.getPanelBehavior" => Request::GetBehavior,
                    "sidePanel.setPanelBehavior" => Request::SetBehavior(
                        args[0]["openPanelOnActionClick"].as_bool().unwrap_or(false),
                    ),
                    "sidePanel.open" => Request::Open(args[0].clone()),
                    "sidePanel.close" => Request::Close(args[0].clone()),
                    _ => Request::GetLayout,
                };
                host.side_panel(call.sender(), request)
            })();
            call.reply(result)
        })?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browser::fake::FakeBrowser;
    use crate::context::tests::{background_sender, content_sender, profile};
    use crate::dispatch::Dispatcher;
    use crate::permissions::{
        InstallPermissions, PermissionDeclaration, PermissionRequest, SiteAccess,
    };
    use zephium_webext::manifest::Manifest;

    #[test]
    fn options_settle_through_the_port_and_never_claim_presentation() {
        let manifest = Manifest::from_value(
            json!({"manifest_version":3,"permissions":["sidePanel"],"side_panel":{"default_path":"panel.html"}}),
        );
        let (declaration, _) = PermissionDeclaration::from_manifest(&manifest);
        let permissions = InstallPermissions::new(
            declaration,
            &PermissionRequest::default(),
            SiteAccess::OnAllSites,
            false,
        );
        let none = InstallPermissions::new(
            Default::default(),
            &PermissionRequest::default(),
            SiteAccess::OnAllSites,
            false,
        );
        let mut host = FakeBrowser::new(profile());
        host.info.manifest = manifest.raw().clone();
        let mut table = ApiTable::new();
        register(&mut table).unwrap();
        let mut dispatcher = Dispatcher::new(table);
        let sender = background_sender();
        for (caller, grants) in [
            (&sender, &none),
            (&content_sender("https://example.com/"), &permissions),
        ] {
            assert!(host
                .call(
                    &mut dispatcher,
                    caller,
                    grants,
                    "sidePanel.setOptions",
                    json!([{"enabled":false}])
                )
                .is_err());
        }
        assert!(host.panels.is_empty());
        host.call(
            &mut dispatcher,
            &sender,
            &permissions,
            "sidePanel.setOptions",
            json!([{"enabled":false}]),
        )
        .unwrap();
        assert_eq!(
            host.call(
                &mut dispatcher,
                &sender,
                &permissions,
                "sidePanel.getOptions",
                json!([{}])
            )
            .unwrap(),
            json!({"enabled":false,"path":"panel.html"})
        );
        assert!(host
            .call(
                &mut dispatcher,
                &sender,
                &permissions,
                "sidePanel.setOptions",
                json!([{"enabled":"false"}])
            )
            .is_err());
        assert!(host
            .call(
                &mut dispatcher,
                &sender,
                &permissions,
                "sidePanel.setOptions",
                json!([{"tabId":999,"enabled":false}])
            )
            .is_err());
        assert!(host
            .call(
                &mut dispatcher,
                &sender,
                &permissions,
                "sidePanel.open",
                json!([{"windowId":1}])
            )
            .is_err());
    }

    #[test]
    fn tab_records_do_not_inherit_global_paths_and_missing_overrides_return_defaults() {
        let mut store = OptionsStore::new(&json!({"side_panel":{"default_path":"default.html"}}));
        assert_eq!(
            store.configure(Request::GetOptions(Some(7))).unwrap(),
            json!({"enabled":true,"path":"default.html"})
        );
        store
            .configure(Request::SetOptions(json!({"tabId":7})))
            .unwrap();
        assert_eq!(
            store.configure(Request::GetOptions(Some(7))).unwrap(),
            json!({"enabled":true,"tabId":7})
        );
        store
            .configure(Request::SetOptions(json!({"tabId":7,"path":"tab.html"})))
            .unwrap();
        store
            .configure(Request::SetOptions(json!({"tabId":7,"enabled":false})))
            .unwrap();
        assert_eq!(
            store.configure(Request::GetOptions(Some(7))).unwrap(),
            json!({"enabled":false,"tabId":7,"path":"tab.html"})
        );
    }

    #[test]
    fn overrides_are_bounded_and_retired_with_tabs() {
        let mut store = OptionsStore::new(&json!({}));
        assert!(store.configure(Request::SetBehavior(true)).is_err());
        assert_eq!(
            store.configure(Request::GetBehavior).unwrap(),
            json!({"openPanelOnActionClick":false})
        );
        for tab in 0..512 {
            store
                .configure(Request::SetOptions(json!({"tabId":tab,"enabled":false})))
                .unwrap();
        }
        assert!(store
            .configure(Request::SetOptions(json!({"tabId":512})))
            .is_err());
        store.retain_tabs(|tab| tab == 1);
        assert_eq!(store.options.len(), 2);
        assert_eq!(
            store.configure(Request::GetOptions(Some(2))).unwrap()["enabled"],
            true
        );
        assert_eq!(
            store.configure(Request::GetOptions(Some(1))).unwrap()["enabled"],
            false
        );
    }
}
