//! Registered shortcuts and trusted keyboard activation routing.

use crate::browser::{BrowserPort, BrowserTab};
use crate::dispatch::{ApiTable, RegistrationError};
use crate::permissions::InstallPermissions;
use crate::ApiError;
use serde_json::Value;

pub(crate) const METHODS: &[&str] = &["commands.getAll"];

/// Outcome of a native registered shortcut, never from an extension envelope.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Activation {
    /// Run the exact same action path as a trusted toolbar click; do not emit onCommand.
    Action,
    /// Deliver commands.onCommand with the ordinary active tab, redacted as usual.
    Command(String),
}

/// Validates native shortcut registration and grants invocation-scoped activeTab.
pub fn activate(
    registered: &[Value],
    name: &str,
    tab: &BrowserTab,
    permissions: &mut InstallPermissions,
) -> Result<Activation, ApiError> {
    if !registered.iter().any(|command| {
        command.get("name").and_then(Value::as_str) == Some(name)
            && command
                .get("shortcut")
                .and_then(Value::as_str)
                .is_some_and(|key| !key.is_empty())
    }) {
        return Err(ApiError::other("Command is not registered."));
    }
    if name == "_execute_action" {
        // The action path checks enabled state before granting access.
        return Ok(Activation::Action);
    }
    if let Ok(url) = tab.url() {
        permissions.grant_active_tab(tab.tab.id(), tab.document, &url);
    }
    Ok(Activation::Command(name.to_owned()))
}

/// Returns actual registration results; suggested manifest keys may have conflicts.
pub fn register<S: BrowserPort + 'static>(
    table: &mut ApiTable<S>,
) -> Result<(), RegistrationError> {
    table.register_generated(METHODS[0], |host: &mut S, call| {
        let result = host
            .extension_info(call.sender())
            .map(|info| Value::Array(info.commands));
        call.reply(result)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browser::fake::FakeBrowser;
    use crate::context::tests::{background_sender, profile};
    use crate::dispatch::Dispatcher;
    use crate::permissions::{PermissionDeclaration, PermissionRequest, SiteAccess};
    use serde_json::json;
    use zephium_webext::manifest::Manifest;

    #[test]
    fn actual_shortcuts_include_action_but_action_never_becomes_on_command() {
        let sender = background_sender();
        let mut host = FakeBrowser::new(profile());
        let window = host.add_window(false);
        let tab = host.add_tab(window, "https://example.com/", true);
        host.info.commands = vec![
            json!({"name":"_execute_action","shortcut":"Ctrl+Shift+Y"}),
            json!({"name":"custom","shortcut":"Ctrl+J"}),
            json!({"name":"conflicted","shortcut":""}),
        ];
        let (declaration, _) = PermissionDeclaration::from_manifest(&Manifest::from_value(
            json!({"permissions":["activeTab"],"commands":{}}),
        ));
        let mut permissions = InstallPermissions::new(
            declaration,
            &PermissionRequest::default(),
            SiteAccess::OnAllSites,
            false,
        );
        let mut table = ApiTable::new();
        register(&mut table).unwrap();
        let mut dispatcher = Dispatcher::new(table);
        assert_eq!(
            host.call(
                &mut dispatcher,
                &sender,
                &permissions,
                "commands.getAll",
                json!([])
            )
            .unwrap(),
            json!(host.info.commands)
        );
        let no_manifest = InstallPermissions::new(
            Default::default(),
            &PermissionRequest::default(),
            SiteAccess::OnAllSites,
            false,
        );
        assert_eq!(
            host.call(
                &mut dispatcher,
                &sender,
                &no_manifest,
                "commands.getAll",
                json!([])
            )
            .unwrap_err(),
            ApiError::missing_permission().message()
        );
        let browser_tab = host.snapshot.tab(tab).unwrap();
        assert!(activate(
            &host.info.commands,
            "conflicted",
            browser_tab,
            &mut permissions
        )
        .is_err());
        assert!(activate(&host.info.commands, "forged", browser_tab, &mut permissions).is_err());
        assert!(browser_tab.check_access(&sender, &permissions).is_err());
        assert_eq!(
            activate(
                &host.info.commands,
                "_execute_action",
                browser_tab,
                &mut permissions
            )
            .unwrap(),
            Activation::Action
        );
        assert!(browser_tab.check_access(&sender, &permissions).is_err());
        host.action
            .apply(Some(tab), &super::super::action::Update::Enabled(false))
            .unwrap();
        assert!(host
            .action
            .trusted_invoke(browser_tab, &mut permissions)
            .is_err());
        assert!(browser_tab.check_access(&sender, &permissions).is_err());
        host.action
            .apply(Some(tab), &super::super::action::Update::Enabled(true))
            .unwrap();
        host.action
            .trusted_invoke(browser_tab, &mut permissions)
            .unwrap();
        assert!(browser_tab.check_access(&sender, &permissions).is_ok());
        assert_eq!(
            activate(&host.info.commands, "custom", browser_tab, &mut permissions).unwrap(),
            Activation::Command("custom".into())
        );
    }
}
