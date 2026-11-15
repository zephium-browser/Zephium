//! Legacy extension facts; JavaScript Window objects remain binding-local.

use crate::browser::{BrowserPort, IncognitoMode};
use crate::dispatch::{ApiTable, RegistrationError};
use serde_json::{json, Value};

pub(crate) const METHODS: &[&str] = &[
    "extension.getURL",
    "extension.isAllowedIncognitoAccess",
    "extension.isAllowedFileSchemeAccess",
    "extension.getViews",
    "extension.getBackgroundPage",
    "extension.getExtensionTabs",
    "extension.setUpdateUrlData",
];

/// Registers the safe MV3 legacy surfaces. MV3 has no background Window.
pub fn register<S: BrowserPort + 'static>(
    table: &mut ApiTable<S>,
) -> Result<(), RegistrationError> {
    for name in METHODS {
        table.register_generated(name, |host: &mut S, call| {
            let result = match call.name() {
                "extension.getURL" => super::runtime::get_url(
                    call.sender(),
                    call.args()[0].as_str().unwrap_or_default(),
                )
                .map(Value::String),
                "extension.isAllowedFileSchemeAccess" => {
                    Ok(json!(call.permissions().file_access()))
                }
                "extension.isAllowedIncognitoAccess" => {
                    host.extension_info(call.sender()).map(|info| {
                        json!(
                            info.incognito_allowed
                                && info.incognito_mode != IncognitoMode::NotAllowed
                        )
                    })
                }
                "extension.getBackgroundPage" => Ok(Value::Null),
                // Native transport cannot return live Window identities. The bootstrap owns these methods.
                "extension.getViews" | "extension.getExtensionTabs" => {
                    Err(crate::browser::unsupported())
                }
                _ => Err(crate::browser::unsupported()),
            };
            call.reply(result)
        })?;
    }
    Ok(())
}

/// Binding-local MV3 helpers. `views` must be a live same-install Window registry.
pub const VIEWS_BOOTSTRAP: &str = r#"(views, current) => ({
  getBackgroundPage: () => null,
  getViews: (filter = {}) => views().filter(view =>
    (filter.type === undefined || view.type === filter.type) &&
    (filter.tabId === undefined || view.tabId === filter.tabId) &&
    (filter.windowId === undefined || view.windowId === filter.windowId) &&
    view.incognito === current.incognito).map(view => view.window)
})"#;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browser::fake::FakeBrowser;
    use crate::context::tests::{background_sender, profile};
    use crate::dispatch::Dispatcher;
    use crate::permissions::{InstallPermissions, PermissionRequest, SiteAccess};
    #[test]
    fn mv3_background_and_access_flags_reflect_effective_browser_policy() {
        let sender = background_sender();
        let mut host = FakeBrowser::new(profile());
        let mut table = ApiTable::new();
        register(&mut table).unwrap();
        let mut dispatcher = Dispatcher::new(table);
        let mut permissions = InstallPermissions::new(
            Default::default(),
            &PermissionRequest::default(),
            SiteAccess::OnAllSites,
            false,
        );
        assert_eq!(
            host.call(
                &mut dispatcher,
                &sender,
                &permissions,
                "extension.getBackgroundPage",
                json!([])
            )
            .unwrap(),
            Value::Null
        );
        assert_eq!(
            host.call(
                &mut dispatcher,
                &sender,
                &permissions,
                "extension.isAllowedFileSchemeAccess",
                json!([])
            )
            .unwrap(),
            false
        );
        assert!(permissions.set_file_access(true).is_empty());
        assert_eq!(
            host.call(
                &mut dispatcher,
                &sender,
                &permissions,
                "extension.isAllowedFileSchemeAccess",
                json!([])
            )
            .unwrap(),
            true
        );
        host.info.incognito_allowed = true;
        host.info.incognito_mode = IncognitoMode::NotAllowed;
        assert_eq!(
            host.call(
                &mut dispatcher,
                &sender,
                &permissions,
                "extension.isAllowedIncognitoAccess",
                json!([])
            )
            .unwrap(),
            false
        );
        host.info.incognito_mode = IncognitoMode::Split;
        assert_eq!(
            host.call(
                &mut dispatcher,
                &sender,
                &permissions,
                "extension.isAllowedIncognitoAccess",
                json!([])
            )
            .unwrap(),
            true
        );
        assert_eq!(
            host.call(
                &mut dispatcher,
                &sender,
                &permissions,
                "extension.getURL",
                json!(["/a.html"])
            )
            .unwrap(),
            json!(super::super::runtime::get_url(&sender, "a.html").unwrap())
        );
    }
}
