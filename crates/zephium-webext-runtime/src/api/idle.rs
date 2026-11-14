//! Native idle observations, scoped to this install.
use crate::browser::{BrowserPort, BrowserRequest, ServiceRequest};
use crate::dispatch::{ApiTable, RegistrationError};

pub(crate) const METHODS: &[&str] = &[
    "idle.queryState",
    "idle.setDetectionInterval",
    "idle.getAutoLockDelay",
];

/// Generated validation enforces Chrome's fifteen-second threshold.
pub fn register<S: BrowserPort + 'static>(
    table: &mut ApiTable<S>,
) -> Result<(), RegistrationError> {
    for name in METHODS {
        table.register_generated(name, |host: &mut S, call| {
            let request = match call.name() {
                "idle.queryState" => {
                    ServiceRequest::QueryIdle(call.args()[0].as_u64().unwrap_or(15) as u32)
                }
                "idle.setDetectionInterval" => {
                    ServiceRequest::SetIdleInterval(call.args()[0].as_u64().unwrap_or(15) as u32)
                }
                _ => return call.reply(Err(crate::browser::unsupported())),
            };
            host.request(BrowserRequest::Service(request), call)
        })?;
    }
    Ok(())
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
    fn thresholds_and_permission_gates_precede_the_native_service() {
        let sender = background_sender();
        let mut host = FakeBrowser::new(profile());
        let mut table = ApiTable::new();
        register(&mut table).unwrap();
        let mut dispatcher = Dispatcher::new(table);
        let (declaration, _) = PermissionDeclaration::from_manifest(&Manifest::from_value(
            json!({"permissions":["idle"]}),
        ));
        let permissions = InstallPermissions::new(
            declaration,
            &PermissionRequest::default(),
            SiteAccess::OnAllSites,
            false,
        );
        assert!(host
            .call(
                &mut dispatcher,
                &sender,
                &InstallPermissions::new(
                    Default::default(),
                    &PermissionRequest::default(),
                    SiteAccess::OnAllSites,
                    false
                ),
                "idle.queryState",
                json!([15])
            )
            .is_err());
        assert!(host
            .call(
                &mut dispatcher,
                &sender,
                &permissions,
                "idle.queryState",
                json!([14])
            )
            .is_err());
        assert!(host.requests.is_empty());
        host.reply = Ok(json!("locked"));
        assert_eq!(
            host.call(
                &mut dispatcher,
                &sender,
                &permissions,
                "idle.queryState",
                json!([15])
            )
            .unwrap(),
            "locked"
        );
        assert!(matches!(
            host.requests.last(),
            Some(BrowserRequest::Service(ServiceRequest::QueryIdle(15)))
        ));
        host.call(
            &mut dispatcher,
            &sender,
            &permissions,
            "idle.setDetectionInterval",
            json!([60]),
        )
        .unwrap();
        assert!(matches!(
            host.requests.last(),
            Some(BrowserRequest::Service(ServiceRequest::SetIdleInterval(60)))
        ));
    }
}
