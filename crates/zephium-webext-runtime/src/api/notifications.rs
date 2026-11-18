//! Validated notification operations; native presentation and settlement stay in the port.

use crate::browser::{BrowserPort, BrowserRequest};
use crate::context::Sender;
use crate::dispatch::{ApiTable, RegistrationError};
use crate::ApiError;
use serde_json::Value;
use url::Url;

pub(crate) const METHODS: &[&str] = &[
    "notifications.create",
    "notifications.update",
    "notifications.clear",
    "notifications.getAll",
    "notifications.getPermissionLevel",
];
const MAX_NOTIFICATION_ID_BYTES: usize = 500;
const MAX_NOTIFICATION_IMAGE_BYTES: usize = 1024 * 1024;

/// Notification ids are always scoped to the authenticated install by the adapter.
#[derive(Clone, Debug)]
pub enum Request {
    /// Generate a fresh id if absent/empty; replace an existing same-install id.
    Create {
        /// Install-scoped id, absent for native allocation.
        notification_id: Option<String>,
        /// Generated-schema validated template.
        options: Value,
    },
    /// Merge with an existing native notification; settle false when missing.
    Update {
        /// Install-scoped notification identity.
        notification_id: String,
        /// Partial update; validate again after merging existing state.
        options: Value,
    },
    /// Remove a same-install notification; return whether it existed.
    Clear {
        /// Install-scoped notification identity.
        notification_id: String,
    },
    /// Return only this install's ids mapped to true.
    GetAll,
    /// Return the native notification permission level: granted or denied.
    GetPermissionLevel,
}

/// Validate merged options again at the adapter edge for update operations.
pub fn validate_options(options: &Value, creating: bool) -> Result<(), ApiError> {
    if creating
        && ["type", "iconUrl", "title", "message"]
            .iter()
            .any(|key| options.get(*key).and_then(Value::as_str).is_none())
    {
        return Err(ApiError::other(
            "Some of the required properties are missing: type, iconUrl, title and message.",
        ));
    }
    let kind = options.get("type").and_then(Value::as_str);
    if options.get("progress").is_some() && kind.is_some_and(|kind| kind != "progress") {
        return Err(ApiError::other(
            "The progress value should not be specified for non-progress notification",
        ));
    }
    if options.get("items").is_some() && kind.is_some_and(|kind| kind != "list") {
        return Err(ApiError::other(
            "List items provided for notification type != list",
        ));
    }
    if options.get("imageUrl").is_some() && kind.is_some_and(|kind| kind != "image") {
        return Err(ApiError::other(
            "Image resource provided for notification type != image",
        ));
    }
    if options
        .get("progress")
        .and_then(Value::as_i64)
        .is_some_and(|n| !(0..=100).contains(&n))
    {
        return Err(ApiError::other(
            "The progress value should range from 0 to 100",
        ));
    }
    if options
        .get("priority")
        .and_then(Value::as_i64)
        .is_some_and(|n| n < 0)
    {
        return Err(ApiError::other(
            "Low-priority notifications are deprecated on this platform.",
        ));
    }
    Ok(())
}

fn validate_image(host: &impl BrowserPort, sender: &Sender, raw: &str) -> Result<(), ApiError> {
    let invalid = || ApiError::other("Unable to successfully use the provided image.");
    if raw.len() > MAX_NOTIFICATION_IMAGE_BYTES || raw.is_empty() {
        return Err(invalid());
    }
    match Url::parse(raw) {
        Ok(url) if matches!(url.scheme(), "data" | "blob") => Ok(()),
        Ok(url) if sender.scheme().extension_of(&url).as_ref() == Some(sender.extension_id()) => {
            host.validate_resource(sender, url.path().trim_start_matches('/'))
        }
        Ok(_) => Err(invalid()),
        Err(_) => host.validate_resource(sender, raw.trim_start_matches('/')),
    }
}

/// Registers generated signatures; binary decode limits remain mandatory in native adapters.
pub fn register<S: BrowserPort + 'static>(
    table: &mut ApiTable<S>,
) -> Result<(), RegistrationError> {
    for name in METHODS {
        table.register_generated(name, |host: &mut S, call| {
            let id = call.args().first().and_then(Value::as_str);
            if id.is_some_and(|id| id.len() > MAX_NOTIFICATION_ID_BYTES) {
                return call.reply(Err(ApiError::other(
                    "The notification's ID should be 500 characters or less",
                )));
            }
            let request = match call.name() {
                "notifications.create" | "notifications.update" => {
                    let options = call.args()[1].clone();
                    if let Err(error) =
                        validate_options(&options, call.name() == "notifications.create")
                    {
                        return call.reply(Err(error));
                    }
                    for key in ["iconUrl", "imageUrl", "appIconMaskUrl"] {
                        if let Some(raw) = options.get(key).and_then(Value::as_str) {
                            if let Err(error) = validate_image(host, call.sender(), raw) {
                                return call.reply(Err(error));
                            }
                        }
                    }
                    if let Some(buttons) = options.get("buttons").and_then(Value::as_array) {
                        for button in buttons {
                            if let Some(raw) = button.get("iconUrl").and_then(Value::as_str) {
                                if let Err(error) = validate_image(host, call.sender(), raw) {
                                    return call.reply(Err(error));
                                }
                            }
                        }
                    }
                    if call.name() == "notifications.create" {
                        Request::Create {
                            notification_id: id.filter(|id| !id.is_empty()).map(str::to_owned),
                            options,
                        }
                    } else {
                        Request::Update {
                            notification_id: id.unwrap_or_default().to_owned(),
                            options,
                        }
                    }
                }
                "notifications.clear" => Request::Clear {
                    notification_id: id.unwrap_or_default().to_owned(),
                },
                "notifications.getAll" => Request::GetAll,
                _ => Request::GetPermissionLevel,
            };
            host.request(BrowserRequest::Notifications(request), call)
        })?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn create_requires_complete_template_and_update_checks_merged_kind() {
        assert!(validate_options(&json!({"title":"a"}), true).is_err());
        assert!(validate_options(&json!({"title":"a"}), false).is_ok());
        for value in [
            json!({"type":"basic","progress":1}),
            json!({"type":"basic","items":[]}),
            json!({"type":"basic","imageUrl":"a"}),
            json!({"type":"progress","progress":101}),
            json!({"priority":-1}),
        ] {
            assert!(validate_options(&value, false).is_err(), "{value}");
        }
        assert!(validate_options(
            &json!({"type":"progress","progress":100,"title":"","message":"","iconUrl":"i.png"}),
            true
        )
        .is_ok());
    }
    #[test]
    fn notifications_validate_images_and_templates_before_native_requests() {
        use crate::browser::fake::FakeBrowser;
        use crate::context::tests::{background_sender, profile};
        use crate::dispatch::Dispatcher;
        use crate::permissions::{
            InstallPermissions, PermissionDeclaration, PermissionRequest, SiteAccess,
        };
        use zephium_webext::manifest::Manifest;
        let sender = background_sender();
        let mut host = FakeBrowser::new(profile());
        host.resources.insert("icon.png".into());
        let (declaration, _) = PermissionDeclaration::from_manifest(&Manifest::from_value(
            json!({"permissions":["notifications"]}),
        ));
        let permissions = InstallPermissions::new(
            declaration,
            &PermissionRequest::default(),
            SiteAccess::OnAllSites,
            false,
        );
        let mut table = ApiTable::new();
        register(&mut table).unwrap();
        let mut dispatcher = Dispatcher::new(table);
        for options in [
            json!({"title":"missing"}),
            json!({"type":"basic","title":"a","message":"b","iconUrl":"https://foreign.example/icon.png"}),
            json!({"type":"basic","title":"a","message":"b","iconUrl":"icon.png","progress":3}),
        ] {
            assert!(host
                .call(
                    &mut dispatcher,
                    &sender,
                    &permissions,
                    "notifications.create",
                    json!([options])
                )
                .is_err());
        }
        assert!(host.requests.is_empty());
        host.reply = Ok(json!("generated"));
        assert_eq!(
            host.call(
                &mut dispatcher,
                &sender,
                &permissions,
                "notifications.create",
                json!([{"type":"basic","title":"a","message":"b","iconUrl":"icon.png"}])
            )
            .unwrap(),
            "generated"
        );
        assert!(matches!(
            host.requests.last(),
            Some(BrowserRequest::Notifications(Request::Create {
                notification_id: None,
                ..
            }))
        ));
        host.reply = Ok(json!(false));
        assert_eq!(
            host.call(
                &mut dispatcher,
                &sender,
                &permissions,
                "notifications.update",
                json!(["missing",{"title":"partial"}])
            )
            .unwrap(),
            false
        );
        assert!(
            matches!(host.requests.last(),Some(BrowserRequest::Notifications(Request::Update{notification_id,..})) if notification_id == "missing")
        );
        assert_eq!(
            host.call(
                &mut dispatcher,
                &sender,
                &permissions,
                "notifications.clear",
                json!(["missing"])
            )
            .unwrap(),
            false
        );
        host.reply = Ok(json!({"generated":true}));
        assert_eq!(
            host.call(
                &mut dispatcher,
                &sender,
                &permissions,
                "notifications.getAll",
                json!([])
            )
            .unwrap(),
            json!({"generated":true})
        );
        host.reply = Ok(json!("denied"));
        assert_eq!(
            host.call(
                &mut dispatcher,
                &sender,
                &permissions,
                "notifications.getPermissionLevel",
                json!([])
            )
            .unwrap(),
            "denied"
        );
        let count = host.requests.len();
        assert!(host
            .call(
                &mut dispatcher,
                &sender,
                &permissions,
                "notifications.clear",
                json!(["a".repeat(501)])
            )
            .is_err());
        assert_eq!(host.requests.len(), count);
    }
}
