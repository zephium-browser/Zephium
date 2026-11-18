//! Installation facts and deferred runtime lifecycle operations.

use serde_json::{json, Value};
use url::Url;

use crate::browser::{BrowserPort, BrowserRequest, ExtensionInfo};
use crate::context::Sender;
use crate::dispatch::{ApiTable, Handled, Invocation, RegistrationError};
use crate::ApiError;

pub(crate) const METHODS: &[&str] = &[
    "runtime.getPlatformInfo",
    "runtime.getManifest",
    "runtime.getURL",
    "runtime.getVersion",
    "runtime.reload",
    "runtime.openOptionsPage",
    "runtime.setUninstallURL",
    "runtime.getContexts",
    "runtime.getBackgroundPage",
    "runtime.requestUpdateCheck",
    "runtime.getPackageDirectoryEntry",
    "runtime.restart",
    "runtime.restartAfterDelay",
];

/// Adapter operations must settle after the lifecycle or durable setting changes.
#[derive(Clone, Debug)]
pub enum Request {
    /// Retire this generation before loading the installed version again.
    Reload,
    /// Focus an existing options page or open the admitted packaged URL.
    OpenOptionsPage {
        /// Admitted extension URL.
        url: String,
    },
    /// Persist a validated HTTP(S) URL, or clear it when empty.
    SetUninstallUrl {
        /// Validated HTTP(S) URL or empty string.
        url: String,
    },
    /// Ask the install updater; return status and optional version.
    RequestUpdateCheck,
}

/// Largest per-context synchronous data payload, before binding injection.
pub const MAX_BOOTSTRAP_BYTES: usize = 2 * 1024 * 1024;

struct BoundedSize(usize);
impl std::io::Write for BoundedSize {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0 = self
            .0
            .checked_add(bytes.len())
            .filter(|size| *size <= MAX_BOOTSTRAP_BYTES)
            .ok_or_else(|| std::io::Error::other("Bootstrap data limit exceeded."))?;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn bounded(value: &impl serde::Serialize) -> Result<(), ApiError> {
    serde_json::to_writer(BoundedSize(0), value).map_err(|_| ApiError::message_too_large())
}

/// Synchronous data supplied only to this authenticated install's bindings.
pub fn bootstrap(sender: &Sender, info: &ExtensionInfo) -> Result<Value, ApiError> {
    bounded(&info.manifest)?;
    let value = json!({"id": sender.extension_id().as_str(), "manifest": info.manifest,
        "baseUrl": format!("{}://{}/", sender.scheme().as_str(), sender.extension_id().as_str()),
        "inIncognitoContext": sender.incognito()});
    bounded(&value)?;
    Ok(value)
}

/// Chrome resolves getURL relative paths under its own extension origin.
pub fn get_url(sender: &Sender, path: &str) -> Result<String, ApiError> {
    // Leading slashes are package-relative, including a double slash.
    let base = Url::parse(&format!(
        "{}://{}/",
        sender.scheme().as_str(),
        sender.extension_id().as_str()
    ))
    .map_err(|_| ApiError::context_invalidated())?;
    let url = base
        .join(path.trim_start_matches('/'))
        .map_err(|_| ApiError::other("Invalid URL."))?;
    if url.scheme() != base.scheme()
        || url.host_str() != base.host_str()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
    {
        return Err(ApiError::other("Invalid URL."));
    }
    Ok(url.into())
}

/// Applies all generated runtime context filters conjunctively.
pub fn contexts(info: &ExtensionInfo, sender: &Sender, filter: &Value) -> Value {
    const FIELDS: &[(&str, &str)] = &[
        ("contextIds", "contextId"),
        ("contextTypes", "contextType"),
        ("tabIds", "tabId"),
        ("windowIds", "windowId"),
        ("documentIds", "documentId"),
        ("frameIds", "frameId"),
        ("documentOrigins", "documentOrigin"),
        ("documentUrls", "documentUrl"),
    ];
    Value::Array(
        info.contexts
            .iter()
            .filter(|context| {
                context.get("incognito").and_then(Value::as_bool) == Some(sender.incognito())
                    && filter
                        .get("incognito")
                        .is_none_or(|private| context.get("incognito") == Some(private))
                    && FIELDS.iter().all(|(plural, field)| {
                        filter
                            .get(*plural)
                            .and_then(Value::as_array)
                            .is_none_or(|values| {
                                context
                                    .get(*field)
                                    .is_some_and(|value| values.contains(value))
                            })
                    })
            })
            .cloned()
            .collect(),
    )
}

fn handle(host: &mut impl BrowserPort, call: Invocation<'_>) -> Handled {
    let result = (|| {
        let info = host.extension_info(call.sender())?;
        bounded(&info.manifest)?;
        bounded(&info.platform)?;
        bounded(&info.contexts)?;
        if info.contexts.len() > 4096 {
            return Err(ApiError::message_too_large());
        }
        match call.name() {
            "runtime.getPlatformInfo" => Ok(info.platform),
            "runtime.getManifest" => Ok(info.manifest),
            "runtime.getVersion" => Ok(info.manifest.get("version").cloned().unwrap_or(json!(""))),
            "runtime.getURL" => get_url(call.sender(), call.args()[0].as_str().unwrap_or_default())
                .map(Value::String),
            "runtime.getContexts" => Ok(contexts(&info, call.sender(), &call.args()[0])),
            "runtime.getBackgroundPage" => {
                Err(ApiError::other("You do not have a background page."))
            }
            "runtime.restart" | "runtime.restartAfterDelay" => Err(ApiError::other(
                "API available only for ChromeOS kiosk mode.",
            )),
            "runtime.getPackageDirectoryEntry" => Err(crate::browser::unsupported()),
            _ => Ok(Value::Null),
        }
    })();
    if !matches!(
        call.name(),
        "runtime.reload"
            | "runtime.openOptionsPage"
            | "runtime.setUninstallURL"
            | "runtime.requestUpdateCheck"
    ) {
        return call.reply(result);
    }
    if let Err(error) = result {
        return call.reply(Err(error));
    }
    let request = match call.name() {
        "runtime.reload" => Request::Reload,
        "runtime.requestUpdateCheck" => Request::RequestUpdateCheck,
        "runtime.openOptionsPage" => {
            let result = host.extension_info(call.sender()).and_then(|info| {
                let path = info
                    .manifest
                    .pointer("/options_ui/page")
                    .or_else(|| info.manifest.get("options_page"))
                    .and_then(Value::as_str)
                    .filter(|path| !path.is_empty())
                    .ok_or_else(|| ApiError::other("Could not create an options page."))?;
                get_url(call.sender(), path)
            });
            match result {
                Ok(url) => Request::OpenOptionsPage { url },
                Err(error) => return call.reply(Err(error)),
            }
        }
        "runtime.setUninstallURL" => {
            let raw = call.args()[0].as_str().unwrap_or_default();
            if !raw.is_empty()
                && !Url::parse(raw).is_ok_and(|url| {
                    matches!(url.scheme(), "http" | "https") && url.host_str().is_some()
                })
            {
                let error = ApiError::other(format!("Invalid URL: \"{raw}\"."));
                return call.reply(Err(error));
            }
            Request::SetUninstallUrl {
                url: raw.to_owned(),
            }
        }
        _ => return call.reply(Err(crate::browser::unsupported())),
    };
    host.request(BrowserRequest::Runtime(request), call)
}

/// Registers generated runtime signatures without overriding messaging handlers.
pub fn register<S: BrowserPort + 'static>(
    table: &mut ApiTable<S>,
) -> Result<(), RegistrationError> {
    for name in METHODS {
        table.register_generated(name, |host: &mut S, call| handle(host, call))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::tests::background_sender;

    #[test]
    fn bootstrap_url_and_context_filters_remain_install_scoped() {
        let sender = background_sender();
        let mut info = ExtensionInfo {
            contexts: vec![
                json!({"contextId":"a","contextType":"BACKGROUND","incognito":false,"tabId":-1}),
                json!({"contextId":"b","contextType":"TAB","incognito":false,"tabId":2,"documentUrl":"https://example.com/"}),
                json!({"contextId":"c","contextType":"TAB","incognito":true,"tabId":3}),
            ],
            ..ExtensionInfo::default()
        };
        assert_eq!(
            contexts(&info, &sender, &json!({}))
                .as_array()
                .unwrap()
                .len(),
            2
        );
        assert_eq!(
            contexts(
                &info,
                &sender,
                &json!({"contextTypes":["TAB"],"tabIds":[2]})
            )[0]["contextId"],
            "b"
        );
        assert_eq!(
            contexts(&info, &sender, &json!({"contextIds":[]})),
            json!([])
        );
        assert_eq!(
            contexts(&info, &sender, &json!({"documentIds":["missing"]})),
            json!([])
        );
        assert_eq!(
            contexts(&info, &sender, &json!({"incognito":true})),
            json!([])
        );
        let base = bootstrap(&sender, &info).unwrap()["baseUrl"]
            .as_str()
            .unwrap()
            .to_owned();
        assert_eq!(
            get_url(&sender, "///icons/../popup.html").unwrap(),
            format!("{base}popup.html")
        );
        assert!(get_url(&sender, "https://foreign.example/").is_err());
        assert_eq!(
            bootstrap(&sender, &info).unwrap()["id"],
            sender.extension_id().as_str()
        );
        info.manifest = json!({"oversized":"a".repeat(MAX_BOOTSTRAP_BYTES)});
        assert!(bootstrap(&sender, &info).is_err());
    }
    #[test]
    fn lifecycle_mutations_forward_validated_requests_and_preserve_native_failures() {
        use crate::browser::fake::FakeBrowser;
        use crate::context::tests::profile;
        use crate::dispatch::Dispatcher;
        use crate::permissions::{InstallPermissions, PermissionRequest, SiteAccess};
        let sender = background_sender();
        let mut host = FakeBrowser::new(profile());
        host.info.manifest =
            json!({"manifest_version":3,"version":"1.2","options_ui":{"page":"options.html"}});
        let permissions = InstallPermissions::new(
            Default::default(),
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
                "runtime.getPlatformInfo",
                json!([])
            )
            .unwrap(),
            host.info.platform
        );
        assert_eq!(
            host.call(
                &mut dispatcher,
                &sender,
                &permissions,
                "runtime.getManifest",
                json!([])
            )
            .unwrap(),
            host.info.manifest
        );
        assert_eq!(
            host.call(
                &mut dispatcher,
                &sender,
                &permissions,
                "runtime.getVersion",
                json!([])
            )
            .unwrap(),
            "1.2"
        );
        host.call(
            &mut dispatcher,
            &sender,
            &permissions,
            "runtime.openOptionsPage",
            json!([]),
        )
        .unwrap();
        assert!(
            matches!(host.requests.last(),Some(BrowserRequest::Runtime(Request::OpenOptionsPage {url})) if url.ends_with("/options.html"))
        );
        host.call(
            &mut dispatcher,
            &sender,
            &permissions,
            "runtime.reload",
            json!([]),
        )
        .unwrap();
        assert!(matches!(
            host.requests.last(),
            Some(BrowserRequest::Runtime(Request::Reload))
        ));
        for url in ["https://example.com/uninstall", ""] {
            host.call(
                &mut dispatcher,
                &sender,
                &permissions,
                "runtime.setUninstallURL",
                json!([url]),
            )
            .unwrap();
            assert!(
                matches!(host.requests.last(),Some(BrowserRequest::Runtime(Request::SetUninstallUrl {url:stored})) if stored == url)
            );
        }
        let count = host.requests.len();
        for url in ["file:///private", "javascript:alert(1)", "invalid"] {
            assert!(host
                .call(
                    &mut dispatcher,
                    &sender,
                    &permissions,
                    "runtime.setUninstallURL",
                    json!([url])
                )
                .is_err());
        }
        assert_eq!(host.requests.len(), count);
        host.info.manifest = json!({"manifest_version":3});
        assert!(host
            .call(
                &mut dispatcher,
                &sender,
                &permissions,
                "runtime.openOptionsPage",
                json!([])
            )
            .unwrap_err()
            .contains("options page"));
        assert!(host
            .call(
                &mut dispatcher,
                &sender,
                &permissions,
                "runtime.getBackgroundPage",
                json!([])
            )
            .unwrap_err()
            .contains("background page"));
        host.reply = Err(ApiError::other("Autoupdate is not enabled."));
        assert_eq!(
            host.call(
                &mut dispatcher,
                &sender,
                &permissions,
                "runtime.requestUpdateCheck",
                json!([])
            )
            .unwrap_err(),
            "Autoupdate is not enabled."
        );
        host.info.manifest = json!({"large":"a".repeat(MAX_BOOTSTRAP_BYTES)});
        assert!(host
            .call(
                &mut dispatcher,
                &sender,
                &permissions,
                "runtime.getManifest",
                json!([])
            )
            .is_err());
    }
}
