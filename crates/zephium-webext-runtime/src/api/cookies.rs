//! URL-scoped cookie requests; the native profile remains the cookie authority.

use serde_json::Value;
use url::Url;

use crate::browser::{self, BrowserPort, BrowserRequest, ServiceRequest};
use crate::dispatch::{ApiTable, Invocation, RegistrationError};
use crate::ApiError;

/// An authorized native cookie operation, retaining generated filter fields.
#[derive(Clone, Debug)]
pub struct Request {
    /// Operation selected by the generated method contract.
    pub operation: Operation,
    /// HTTP(S) URL covered by the caller's current host grants.
    pub url: Option<Url>,
    /// Validated Chrome details, never a cookie-store authority.
    pub details: Value,
}

/// Native operations implemented by the URL-scoped adapter.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Operation {
    /// Describe the selected native cookie store.
    Stores,
    /// Select the applicable cookie with this name.
    Get,
    /// Enumerate only cookies applicable to the supplied URL.
    GetAll,
    /// Mutate, then read back the native cookie.
    Set,
    /// Delete the selected cookie, then confirm its absence.
    Remove,
}

pub(crate) const METHODS: &[&str] = &[
    "cookies.get",
    "cookies.getAll",
    "cookies.set",
    "cookies.remove",
    "cookies.getAllCookieStores",
];

/// Install generated shape and permission checks before native dispatch.
pub fn register<S: BrowserPort + 'static>(
    table: &mut ApiTable<S>,
) -> Result<(), RegistrationError> {
    for &name in METHODS {
        table.register_generated(name, |host: &mut S, call| {
            if call.sender().incognito() {
                return call.reply(Err(browser::unsupported()));
            }
            if call.name() == "cookies.getAllCookieStores" {
                return host.request(
                    BrowserRequest::Service(ServiceRequest::Cookies(Request {
                        operation: Operation::Stores,
                        url: None,
                        details: Value::Null,
                    })),
                    call,
                );
            }
            match prepare(&call) {
                Ok(request) => host.request(
                    BrowserRequest::Service(ServiceRequest::Cookies(request)),
                    call,
                ),
                Err(error) => call.reply(Err(error)),
            }
        })?;
    }
    Ok(())
}

fn prepare(call: &Invocation<'_>) -> Result<Request, ApiError> {
    let details = call
        .args()
        .first()
        .ok_or_else(ApiError::malformed_arguments)?;
    if details.get("partitionKey").is_some() {
        return Err(ApiError::other("Partitioned cookies are not supported."));
    }
    if details
        .get("storeId")
        .is_some_and(|store| !matches!(store.as_str(), Some("" | "0")))
    {
        return Err(ApiError::other(
            "The requested cookie store is unavailable.",
        ));
    }
    let url = details["url"]
        .as_str()
        .filter(|url| url.len() <= 8192 && !url.bytes().any(|byte| byte <= 0x20 || byte == 0x7f))
        .and_then(|url| Url::parse(url).ok())
        .filter(|url| {
            matches!(url.scheme(), "http" | "https")
                && url.host_str().is_some()
                && url.username().is_empty()
                && url.password().is_none()
        })
        .ok_or_else(|| ApiError::other("Cookie access requires an explicit HTTP(S) URL."))?;
    if !call.permissions().has_host_access(&url) {
        return Err(ApiError::other(
            "No host permission for cookies at the requested URL.",
        ));
    }
    let operation = match call.name() {
        "cookies.get" => Operation::Get,
        "cookies.getAll" => Operation::GetAll,
        "cookies.set" => Operation::Set,
        "cookies.remove" => Operation::Remove,
        _ => return Err(browser::unsupported()),
    };
    if matches!(operation, Operation::Set | Operation::Remove) && url.scheme() != "https" {
        return Err(ApiError::other(
            "Cookie mutations require HTTPS on this engine.",
        ));
    }
    if details.to_string().len() > 16 * 1024 {
        return Err(ApiError::malformed_arguments());
    }
    Ok(Request {
        operation,
        url: Some(url),
        details: details.clone(),
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
    fn cookie_scope_is_validated_before_any_native_request() {
        let sender = background_sender();
        let mut host = FakeBrowser::new(profile());
        let mut table = ApiTable::new();
        register(&mut table).unwrap();
        let mut dispatcher = Dispatcher::new(table);
        let (declaration, _) = PermissionDeclaration::from_manifest(&Manifest::from_value(
            json!({"permissions":["cookies"],"host_permissions":["https://a.test/*"]}),
        ));
        let permissions = InstallPermissions::new(
            declaration,
            &PermissionRequest::default(),
            SiteAccess::OnAllSites,
            false,
        );
        for (name, args) in [
            ("cookies.getAll", json!([{}])),
            ("cookies.get", json!([{"url":"https://b.test/","name":"x"}])),
            (
                "cookies.get",
                json!([{"url":"https://a.test/","name":"x","partitionKey":{}}]),
            ),
            (
                "cookies.get",
                json!([{"url":"https://a.test/","name":"x","storeId":"1"}]),
            ),
            (
                "cookies.get",
                json!([{"url":"https://user@a.test/","name":"x"}]),
            ),
            (
                "cookies.get",
                json!([{"url":"https://a.test/\n","name":"x"}]),
            ),
            ("cookies.set", json!([{"url":"http://a.test/","name":"x"}])),
        ] {
            assert!(
                host.call(&mut dispatcher, &sender, &permissions, name, args)
                    .is_err(),
                "{name}"
            );
        }
        assert!(host.requests.is_empty());
        host.reply = Ok(Value::Null);
        host.call(
            &mut dispatcher,
            &sender,
            &permissions,
            "cookies.get",
            json!([{"url":"https://a.test/path","name":"x","storeId":""}]),
        )
        .unwrap();
        assert!(matches!(
            host.requests.last(),
            Some(BrowserRequest::Service(ServiceRequest::Cookies(Request {
                operation: Operation::Get,
                ..
            })))
        ));
        host.call(
            &mut dispatcher,
            &sender,
            &permissions,
            "cookies.getAllCookieStores",
            json!([]),
        )
        .unwrap();
        assert!(matches!(
            host.requests.last(),
            Some(BrowserRequest::Service(ServiceRequest::Cookies(Request {
                operation: Operation::Stores,
                ..
            })))
        ));
    }
}
