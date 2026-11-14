//! Menu requests use generated metadata and settle through the browser port.
use crate::browser::BrowserPort;
use crate::dispatch::{ApiTable, RegistrationError};
use crate::ApiError;
use serde_json::Value;

pub(crate) const METHODS: &[&str] = &[
    "contextMenus.create",
    "contextMenus.update",
    "contextMenus.remove",
    "contextMenus.removeAll",
];

/// A caller-scoped menu operation.
#[derive(Clone, Debug)]
pub enum Request {
    /// Register an item after native admission.
    Create(Value),
    /// Update an existing item.
    Update(Value, Value),
    /// Remove one existing item.
    Remove(Value),
    /// Remove all items belonging to this install.
    RemoveAll,
}

/// An adapter with no menu UI has no registered items; never claim creation.
pub fn unavailable(request: Request) -> Result<Value, ApiError> {
    match request {
        Request::RemoveAll => Ok(Value::Null),
        Request::Remove(id) | Request::Update(id, _) => Err(ApiError::other(format!(
            "Cannot find menu item with id {}",
            id.as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| id.to_string())
        ))),
        Request::Create(_) => Err(crate::browser::unsupported()),
    }
}

/// Register signatures, permissions and context restrictions from Chromium.
pub fn register<S: BrowserPort + 'static>(
    table: &mut ApiTable<S>,
) -> Result<(), RegistrationError> {
    for &name in METHODS {
        table.register_generated(name, move |host: &mut S, call| {
            let args = call.args();
            if name == "contextMenus.create" && args[0]["id"].is_null() {
                match host.extension_info(call.sender()) {
                    Ok(info) if info.manifest.pointer("/background/service_worker").is_some() || info.manifest.pointer("/background/persistent") == Some(&Value::Bool(false)) => {
                        return call.reply(Err(ApiError::other("Extensions using event pages or Service Workers must pass an id parameter to chrome.contextMenus.create")));
                    }
                    Err(error) => return call.reply(Err(error)),
                    _ => {}
                }
            }
            let request = match name {
                "contextMenus.create" => Request::Create(args[0].clone()),
                "contextMenus.update" => Request::Update(args[0].clone(), args[1].clone()),
                "contextMenus.remove" => Request::Remove(args[0].clone()),
                _ => Request::RemoveAll,
            };
            let result = host.context_menu(call.sender(), request);
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
    use serde_json::json;
    use zephium_webext::manifest::Manifest;

    #[test]
    fn unavailable_native_menu_is_defined_permission_checked_and_never_created() {
        let manifest = Manifest::from_value(
            json!({"permissions":["contextMenus"],"background":{"service_worker":"worker.js"}}),
        );
        let (declaration, _) = PermissionDeclaration::from_manifest(&manifest);
        let permission = InstallPermissions::new(
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
        assert!(host
            .call(
                &mut dispatcher,
                &sender,
                &none,
                "contextMenus.removeAll",
                json!([])
            )
            .is_err());
        assert!(host
            .call(
                &mut dispatcher,
                &content_sender("https://example.com/"),
                &permission,
                "contextMenus.removeAll",
                json!([])
            )
            .is_err());
        assert_eq!(
            host.call(
                &mut dispatcher,
                &sender,
                &permission,
                "contextMenus.removeAll",
                json!([])
            )
            .unwrap(),
            Value::Null
        );
        assert_eq!(
            host.call(
                &mut dispatcher,
                &sender,
                &permission,
                "contextMenus.remove",
                json!(["missing"])
            )
            .unwrap_err(),
            "Cannot find menu item with id missing"
        );
        assert!(host
            .call(
                &mut dispatcher,
                &sender,
                &permission,
                "contextMenus.create",
                json!([{"title":"Missing id"}])
            )
            .unwrap_err()
            .contains("must pass an id"));
        assert_eq!(
            host.call(
                &mut dispatcher,
                &sender,
                &permission,
                "contextMenus.create",
                json!([{"id":"item","title":"Item"}])
            )
            .unwrap_err(),
            "This function is not supported."
        );
        assert!(host
            .call(
                &mut dispatcher,
                &sender,
                &permission,
                "contextMenus.create",
                json!([{"id":false}])
            )
            .is_err());
    }
}
