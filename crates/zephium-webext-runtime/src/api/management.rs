//! Read-only metadata for the authenticated install.

use crate::browser::{BrowserPort, ExtensionInfo};
use crate::context::Sender;
use crate::dispatch::{ApiTable, RegistrationError};
use crate::permissions::InstallPermissions;
use serde_json::{json, Value};

pub(crate) const METHODS: &[&str] = &["management.getSelf"];

/// Joins manifest metadata to authoritative adapter-supplied management flags.
pub fn get_self(sender: &Sender, info: &ExtensionInfo, permissions: &InstallPermissions) -> Value {
    let manifest = &info.manifest;
    let state = permissions.snapshot();
    let mut result = json!({"id":sender.extension_id().as_str(),"name":manifest.get("name").and_then(Value::as_str).unwrap_or_default(),
        "shortName":manifest.get("short_name").or_else(|| manifest.get("name")).and_then(Value::as_str).unwrap_or_default(),
        "description":manifest.get("description").and_then(Value::as_str).unwrap_or_default(),
        "version":manifest.get("version").and_then(Value::as_str).unwrap_or_default(),"type":"extension",
        "permissions":state.permissions,"hostPermissions":state.origins});
    if let Some(fields) = info.management.as_object() {
        for (key, value) in fields {
            if !matches!(key.as_str(), "id" | "permissions" | "hostPermissions") {
                result[key] = value.clone();
            }
        }
    }
    if let Some(value) = manifest.get("version_name") {
        result["versionName"] = value.clone();
    }
    if let Some(value) = manifest.get("homepage_url") {
        result["homepageUrl"] = value.clone();
    }
    result
}

/// Registers only getSelf; other management methods belong to install management.
pub fn register<S: BrowserPort + 'static>(
    table: &mut ApiTable<S>,
) -> Result<(), RegistrationError> {
    table.register_generated(METHODS[0], |host: &mut S, call| {
        let result = host
            .extension_info(call.sender())
            .map(|info| get_self(call.sender(), &info, call.permissions()));
        call.reply(result)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::tests::background_sender;
    use crate::permissions::{PermissionRequest, SiteAccess};
    #[test]
    fn get_self_uses_authenticated_identity_and_real_native_flags() {
        let sender = background_sender();
        let info = ExtensionInfo {
            manifest: json!({"name":"Example","short_name":"Short","version":"1.2","version_name":"Beta","homepage_url":"https://example.com/"}),
            management: json!({"id":"forged","enabled":true,"installType":"development","mayDisable":false,"permissions":["forged"]}),
            ..ExtensionInfo::default()
        };
        let permissions = InstallPermissions::new(
            Default::default(),
            &PermissionRequest::default(),
            SiteAccess::OnAllSites,
            false,
        );
        let value = get_self(&sender, &info, &permissions);
        assert_eq!(value["id"], sender.extension_id().as_str());
        assert_eq!(value["permissions"], json!([]));
        assert_eq!(value["enabled"], true);
        assert_eq!(value["mayDisable"], false);
        assert_eq!(value["installType"], "development");
        assert_eq!(value["shortName"], "Short");
        assert_eq!(value["versionName"], "Beta");
    }
}
