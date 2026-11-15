//! Nonblocking native request observations. Missing engine facts stay absent.
use crate::permissions::InstallPermissions;
use serde_json::Value;
use url::Url;

/// One native lifecycle event with an adapter-owned stable request identity.
#[derive(Clone, Debug)]
pub struct Observation {
    /// Generated Chrome event name.
    pub event: &'static str,
    /// Requested web address.
    pub url: Url,
    /// Initiator, when the engine establishes it.
    pub initiator: Option<Url>,
    /// Native details without listener-specific optional headers.
    pub details: Value,
    /// Observed headers, never speculative headers that may be added later.
    pub headers: Vec<(String, String)>,
}

impl Observation {
    /// Enforces both target and known initiator host grants at delivery time.
    pub fn project(&self, permissions: &InstallPermissions, extra: &[String]) -> Option<Value> {
        if !permissions.has_api("webRequest")
            || !matches!(self.url.scheme(), "http" | "https")
            || !permissions.has_host_access(&self.url)
            || (self.initiator.is_none()
                && !permissions
                    .snapshot()
                    .origins
                    .iter()
                    .any(|origin| origin == "<all_urls>"))
            || self
                .initiator
                .as_ref()
                .is_some_and(|url| !permissions.has_host_access(url))
        {
            return None;
        }
        let mut details = self.details.clone();
        details["url"] = self.url.as_str().into();
        if let Some(initiator) = &self.initiator {
            details["initiator"] = initiator.origin().ascii_serialization().into();
        }
        let key = match self.event {
            "webRequest.onBeforeSendHeaders" => Some("requestHeaders"),
            "webRequest.onHeadersReceived"
            | "webRequest.onCompleted"
            | "webRequest.onBeforeRedirect" => Some("responseHeaders"),
            _ => None,
        };
        if let Some(key) = key.filter(|key| extra.iter().any(|option| option == key)) {
            let sensitive = extra.iter().any(|option| option == "extraHeaders");
            details[key] = Value::Array(
                self.headers
                    .iter()
                    .filter(|(name, _)| visible_header(key, name, sensitive))
                    .map(|(name, value)| serde_json::json!({"name":name,"value":value}))
                    .collect(),
            );
        }
        Some(details)
    }
}

fn visible_header(key: &str, name: &str, extra: bool) -> bool {
    let name = name.to_ascii_lowercase();
    if key == "requestHeaders" {
        !matches!(
            name.as_str(),
            "authorization"
                | "cache-control"
                | "connection"
                | "content-length"
                | "host"
                | "if-modified-since"
                | "if-none-match"
                | "if-range"
                | "partial-data"
                | "pragma"
                | "proxy-authorization"
                | "proxy-connection"
                | "transfer-encoding"
        ) && (extra
            || !matches!(
                name.as_str(),
                "cookie" | "referer" | "origin" | "accept-language" | "accept-encoding"
            ))
    } else {
        extra || name != "set-cookie"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::permissions::{PermissionDeclaration, PermissionRequest, SiteAccess};
    use serde_json::json;
    use zephium_webext::manifest::Manifest;
    #[test]
    fn request_and_response_header_visibility_follow_chrome() {
        for name in [
            "Authorization",
            "Proxy-Authorization",
            "Host",
            "Content-Length",
            "Cache-Control",
        ] {
            assert!(!visible_header("requestHeaders", name, false));
            assert!(!visible_header("requestHeaders", name, true));
        }
        for name in [
            "Cookie",
            "Origin",
            "Referer",
            "Accept-Language",
            "Accept-Encoding",
        ] {
            assert!(!visible_header("requestHeaders", name, false));
            assert!(visible_header("requestHeaders", name, true));
        }
        assert!(!visible_header("responseHeaders", "Set-Cookie", false));
        assert!(visible_header("responseHeaders", "Set-Cookie", true));
        assert!(visible_header("responseHeaders", "Cache-Control", false));
        assert!(visible_header("responseHeaders", "Origin", false));
    }
    #[test]
    fn unknown_initiators_require_effective_all_host_access() {
        let (declaration, _) = PermissionDeclaration::from_manifest(&Manifest::from_value(
            json!({"permissions":["webRequest"],"host_permissions":["<all_urls>"]}),
        ));
        let mut permissions = InstallPermissions::new(
            declaration,
            &PermissionRequest::default(),
            SiteAccess::OnAllSites,
            false,
        );
        let observation = Observation {
            event: "webRequest.onBeforeRequest",
            url: Url::parse("https://example.com/").unwrap(),
            initiator: None,
            details: json!({"requestId":"1"}),
            headers: Vec::new(),
        };
        assert!(observation.project(&permissions, &[]).is_some());
        assert!(!permissions
            .set_site_access(SiteAccess::OnSpecificSites(vec![
                crate::pattern::HostPattern::parse("https://example.com/*").unwrap(),
            ]))
            .is_empty());
        assert!(permissions.has_host_access(&observation.url));
        assert!(observation.project(&permissions, &[]).is_none());
        assert!(!permissions.set_site_access(SiteAccess::OnClick).is_empty());
        assert!(observation.project(&permissions, &[]).is_none());
    }
    #[test]
    fn observations_recheck_hosts_and_only_include_requested_headers() {
        let (declaration, _) = PermissionDeclaration::from_manifest(&Manifest::from_value(
            json!({"permissions":["webRequest"],"host_permissions":["https://example.com/*"]}),
        ));
        let permissions = InstallPermissions::new(
            declaration,
            &PermissionRequest::default(),
            SiteAccess::OnAllSites,
            false,
        );
        let mut observation = Observation {
            event: "webRequest.onHeadersReceived",
            url: Url::parse("https://example.com/").unwrap(),
            initiator: Some(Url::parse("https://example.com/").unwrap()),
            details: json!({"requestId":"1"}),
            headers: vec![
                ("Content-Type".into(), "text/html".into()),
                ("Set-Cookie".into(), "secret".into()),
            ],
        };
        assert!(observation
            .project(&permissions, &[])
            .unwrap()
            .get("responseHeaders")
            .is_none());
        assert_eq!(
            observation
                .project(&permissions, &["responseHeaders".into()])
                .unwrap()["responseHeaders"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            observation
                .project(
                    &permissions,
                    &["responseHeaders".into(), "extraHeaders".into()]
                )
                .unwrap()["responseHeaders"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        observation.initiator = Some(Url::parse("https://foreign.test/").unwrap());
        assert!(observation.project(&permissions, &[]).is_none());
    }
}
