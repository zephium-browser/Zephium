//! Platform font inventory; settings mutation belongs to the preference owner.
use crate::browser::{BrowserPort, BrowserRequest, ServiceRequest};
use crate::dispatch::{ApiTable, RegistrationError};

pub(crate) const METHODS: &[&str] = &["fontSettings.getFontList"];

/// The adapter returns fontId/displayName objects from its native font inventory.
pub fn register<S: BrowserPort + 'static>(
    table: &mut ApiTable<S>,
) -> Result<(), RegistrationError> {
    table.register_generated(METHODS[0], |host: &mut S, call| {
        host.request(BrowserRequest::Service(ServiceRequest::FontList), call)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browser::fake::FakeBrowser;
    use crate::context::tests::{background_sender, profile};
    use crate::dispatch::Dispatcher;
    use crate::permissions::{
        InstallPermissions, PermissionDeclaration, PermissionRequest, SiteAccess,
    };
    use serde_json::json;
    use zephium_webext::manifest::Manifest;
    #[test]
    fn font_inventory_requires_permission_and_preserves_native_names() {
        let sender = background_sender();
        let mut host = FakeBrowser::new(profile());
        let mut table = ApiTable::new();
        register(&mut table).unwrap();
        let mut dispatcher = Dispatcher::new(table);
        let (declaration, _) = PermissionDeclaration::from_manifest(&Manifest::from_value(
            json!({"permissions":["fontSettings"]}),
        ));
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
        assert!(host
            .call(
                &mut dispatcher,
                &sender,
                &none,
                "fontSettings.getFontList",
                json!([])
            )
            .is_err());
        assert!(host.requests.is_empty());
        host.reply = Ok(json!([{"fontId":"Example Mono","displayName":"Example Mono"}]));
        assert_eq!(
            host.call(
                &mut dispatcher,
                &sender,
                &permissions,
                "fontSettings.getFontList",
                json!([])
            )
            .unwrap(),
            host.reply.clone().unwrap()
        );
        assert!(matches!(
            host.requests.last(),
            Some(BrowserRequest::Service(ServiceRequest::FontList))
        ));
    }
}
