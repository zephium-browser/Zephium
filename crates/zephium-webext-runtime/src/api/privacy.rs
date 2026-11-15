//! Browser-owned privacy preferences; absence is never mutable extension state.

use crate::browser::{BrowserPort, BrowserRequest, ServiceRequest};
use crate::dispatch::{ApiTable, RegistrationError};

/// Browser-managed form services exposed by the minimal privacy adapter.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FormSetting {
    /// Browser-managed password saving.
    PasswordSaving,
    /// Browser-managed postal address filling.
    AutofillAddress,
    /// Deprecated combined form autofill setting.
    Autofill,
    /// Browser-managed card filling.
    AutofillCreditCard,
}

pub(crate) fn methods() -> impl Iterator<Item = &'static str> {
    super::generated::METHOD_VALIDATORS
        .iter()
        .map(|(name, _)| *name)
        .filter(|name| name.starts_with("privacy.") && crate::schema::access(name).is_some())
}

/// Permission, caller and argument checks remain generated from Chromium.
pub fn register<S: BrowserPort + 'static>(
    table: &mut ApiTable<S>,
) -> Result<(), RegistrationError> {
    for name in methods() {
        table.register_generated(name, move |host: &mut S, call| {
            let setting = match name {
                "privacy.services.passwordSavingEnabled.get" => FormSetting::PasswordSaving,
                "privacy.services.autofillAddressEnabled.get" => FormSetting::AutofillAddress,
                "privacy.services.autofillEnabled.get" => FormSetting::Autofill,
                "privacy.services.autofillCreditCardEnabled.get" => FormSetting::AutofillCreditCard,
                _ => return host.request(BrowserRequest::Unavailable { api: name }, call),
            };
            if call.args()[0]
                .get("incognito")
                .and_then(serde_json::Value::as_bool)
                == Some(true)
            {
                return call.reply(Err(crate::browser::unsupported()));
            }
            host.request(
                BrowserRequest::Service(ServiceRequest::FormPrivacy(setting)),
                call,
            )
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
    fn form_settings_validate_before_querying_the_authoritative_adapter() {
        let sender = background_sender();
        let mut host = FakeBrowser::new(profile());
        let mut table = ApiTable::new();
        register(&mut table).unwrap();
        let mut dispatcher = Dispatcher::new(table);
        let permissions = |manifest| {
            let (declaration, _) =
                PermissionDeclaration::from_manifest(&Manifest::from_value(manifest));
            InstallPermissions::new(
                declaration,
                &PermissionRequest::default(),
                SiteAccess::OnAllSites,
                false,
            )
        };
        let granted = permissions(json!({"permissions":["privacy"]}));
        let name = "privacy.services.passwordSavingEnabled.get";
        assert!(host
            .call(
                &mut dispatcher,
                &sender,
                &permissions(json!({})),
                name,
                json!([{}])
            )
            .is_err());
        assert!(host
            .call(&mut dispatcher, &sender, &granted, name, json!([false]))
            .is_err());
        assert!(host
            .call(
                &mut dispatcher,
                &sender,
                &granted,
                name,
                json!([{"incognito":true}])
            )
            .is_err());
        assert!(host.requests.is_empty());
        host.reply = Ok(json!({"value":false,"levelOfControl":"not_controllable"}));
        for (name, setting) in [
            (name, FormSetting::PasswordSaving),
            (
                "privacy.services.autofillAddressEnabled.get",
                FormSetting::AutofillAddress,
            ),
            (
                "privacy.services.autofillCreditCardEnabled.get",
                FormSetting::AutofillCreditCard,
            ),
        ] {
            let answer = host
                .call(&mut dispatcher, &sender, &granted, name, json!([{}]))
                .unwrap();
            assert_eq!(answer, host.reply.clone().unwrap());
            assert!(
                matches!(host.requests.last(), Some(BrowserRequest::Service(ServiceRequest::FormPrivacy(actual))) if *actual == setting)
            );
        }
        host.reply = Err(crate::browser::unsupported());
        assert!(host
            .call(
                &mut dispatcher,
                &sender,
                &granted,
                "privacy.services.passwordSavingEnabled.set",
                json!([{"value":true}])
            )
            .is_err());
        assert!(matches!(
            host.requests.last(),
            Some(BrowserRequest::Unavailable { .. })
        ));
    }
}
