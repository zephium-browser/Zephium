//! Chrome-style install warnings for the permissions a manifest requests.

use crate::manifest::Manifest;

const ALL_SITES: &str = "Read and change all your data on all websites";
const READ_HISTORY: &str = "Read your browsing history";

/// API permissions with a warning, in the order Chrome presents them.
const API_WARNINGS: &[(&str, &str)] = &[
    ("debugger", "Access the page debugger backend"),
    ("declarativeNetRequest", "Block content on any page"),
    ("history", "Read and change your browsing history"),
    ("tabs", READ_HISTORY),
    ("webNavigation", READ_HISTORY),
    ("declarativeNetRequestFeedback", READ_HISTORY),
    ("topSites", "Read a list of your most frequently visited websites"),
    ("favicon", "Read the icons of the websites you visit"),
    ("bookmarks", "Read and change your bookmarks"),
    ("readingList", "Read and change entries in the reading list"),
    ("tabGroups", "View and manage your tab groups"),
    ("clipboardRead", "Read data you copy and paste"),
    ("clipboardWrite", "Modify data you copy and paste"),
    ("downloads", "Manage your downloads"),
    ("geolocation", "Detect your physical location"),
    ("identity.email", "Know your email address"),
    ("management", "Manage your apps, extensions, and themes"),
    (
        "nativeMessaging",
        "Communicate with cooperating native applications",
    ),
    ("notifications", "Display notifications"),
    ("privacy", "Change your privacy-related settings"),
    (
        "contentSettings",
        "Change your settings that control websites' access to features such as cookies, JavaScript, plugins, geolocation, microphone, camera etc.",
    ),
    ("desktopCapture", "Capture content of your screen"),
    ("ttsEngine", "Read all text spoken using synthesized speech"),
];

/// Permissions that expose every page as fully as host access to all sites.
const ALL_SITES_EQUIVALENT: &[&str] = &["debugger", "proxy"];

enum HostAccess {
    All,
    Named(String),
}

pub fn warnings(manifest: &Manifest) -> Vec<String> {
    let patterns: Vec<String> = manifest
        .host_permissions()
        .into_iter()
        .chain(
            manifest
                .content_scripts()
                .into_iter()
                .flat_map(|script| script.matches),
        )
        .collect();
    access_warnings(&manifest.permissions(), &patterns)
}

/// Warnings for access asked for at run time, worded as at install.
pub fn access_warnings(permissions: &[String], patterns: &[String]) -> Vec<String> {
    let has = |name: &str| permissions.iter().any(|p| p == name);

    let mut all_sites = ALL_SITES_EQUIVALENT.iter().any(|p| has(p));
    let mut hosts = Vec::new();
    for pattern in patterns {
        match host_access(pattern) {
            Some(HostAccess::All) => all_sites = true,
            Some(HostAccess::Named(host)) => hosts.push(host),
            None => {}
        }
    }

    let mut warnings = Vec::new();
    if all_sites {
        warnings.push(ALL_SITES.to_owned());
    } else if let Some(hosts) = describe_hosts(hosts) {
        warnings.push(hosts);
    }
    let reads_history = all_sites || has("history");
    for (permission, warning) in API_WARNINGS {
        if !has(permission)
            || (*warning == READ_HISTORY && reads_history)
            || warnings.iter().any(|w| w == warning)
        {
            continue;
        }
        warnings.push((*warning).to_owned());
    }
    warnings
}

fn host_access(pattern: &str) -> Option<HostAccess> {
    if pattern == "<all_urls>" {
        return Some(HostAccess::All);
    }
    let (scheme, rest) = pattern.split_once("://")?;
    if !matches!(scheme, "*" | "http" | "https" | "ws" | "wss" | "ftp") {
        return None;
    }
    let authority = rest.split('/').next()?;
    let host = match authority.strip_prefix('[') {
        Some(ipv6) => &authority[..ipv6.find(']')? + 2],
        None => authority.split(':').next()?,
    };
    match host {
        "" => None,
        "*" => Some(HostAccess::All),
        host => Some(HostAccess::Named(host.to_ascii_lowercase())),
    }
}

fn describe_hosts(mut hosts: Vec<String>) -> Option<String> {
    let domain = |host: &String| host.trim_start_matches("*.").to_owned();
    hosts.sort_by_key(domain);
    hosts.dedup();
    let names: Vec<String> = hosts
        .into_iter()
        .map(|host| match host.strip_prefix("*.") {
            Some(domain) => format!("all {domain} sites"),
            None => host,
        })
        .collect();
    let list = match names.as_slice() {
        [] => return None,
        [one] => one.clone(),
        [a, b] => format!("{a} and {b}"),
        [a, b, c] => format!("{a}, {b}, and {c}"),
        [a, b, c, rest @ ..] => {
            let others = rest.len();
            let noun = if others == 1 { "site" } else { "sites" };
            format!("{a}, {b}, {c}, and {others} other {noun}")
        }
    };
    Some(format!("Read and change your data on {list}"))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    #[test]
    fn runtime_requests_are_worded_like_installs() {
        let all = super::access_warnings(&["clipboardRead".into()], &["*://*/*".into()]);
        assert_eq!(
            all,
            [
                "Read and change all your data on all websites",
                "Read data you copy and paste"
            ]
        );
        let one = super::access_warnings(&[], &["https://*.github.com/*".into()]);
        assert_eq!(one.len(), 1);
        assert!(one[0].contains("github.com"), "{one:?}");
    }

    use super::*;

    fn warnings_for(value: serde_json::Value) -> Vec<String> {
        warnings(&Manifest::from_value(value))
    }

    #[test]
    fn all_sites_access_comes_first_and_subsumes_history_reads() {
        let warnings = warnings_for(json!({
            "manifest_version": 3,
            "permissions": ["notifications", "tabs", "storage", "nativeMessaging"],
            "host_permissions": ["*://*/*"]
        }));
        assert_eq!(
            warnings,
            [
                ALL_SITES,
                "Communicate with cooperating native applications",
                "Display notifications"
            ]
        );
    }

    #[test]
    fn names_a_few_hosts_and_counts_the_rest() {
        let warnings = warnings_for(json!({
            "manifest_version": 2,
            "permissions": ["https://b.test/*", "http://a.test:8080/*", "tabs", "file:///*"],
            "content_scripts": [{"matches": ["https://*.c.test/*", "https://d.test/x", "https://e.test/*"]}]
        }));
        assert_eq!(
            warnings,
            [
                "Read and change your data on a.test, b.test, all c.test sites, and 2 other sites",
                READ_HISTORY
            ]
        );
    }

    #[test]
    fn describes_small_host_sets_naturally() {
        assert_eq!(
            warnings_for(json!({"host_permissions": ["https://example.com/*"]})),
            ["Read and change your data on example.com"]
        );
        assert_eq!(
            warnings_for(json!({"host_permissions": ["https://a.test/*", "https://b.test/*"]})),
            ["Read and change your data on a.test and b.test"]
        );
    }

    #[test]
    fn history_subsumes_reading_history() {
        let warnings = warnings_for(json!({
            "manifest_version": 3,
            "permissions": ["tabs", "history", "webNavigation", "declarativeNetRequest"]
        }));
        assert_eq!(
            warnings,
            [
                "Block content on any page",
                "Read and change your browsing history"
            ]
        );
    }

    #[test]
    fn debugger_implies_all_sites() {
        assert_eq!(
            warnings_for(json!({"permissions": ["debugger"]})),
            [ALL_SITES, "Access the page debugger backend"]
        );
    }
}

/// External applications require original reviewed access and authenticated
/// publisher identity. Compatibility-injected permissions and a public manifest
/// key are insufficient; legacy registries and unreviewed optional grants deny.
pub fn external_native_messaging_allowed(reviewed: &[String], publisher_verified: bool) -> bool {
    publisher_verified
        && reviewed
            .iter()
            .any(|permission| permission == "nativeMessaging")
}

#[cfg(test)]
mod native_authority_tests {
    use super::*;

    #[test]
    fn compatibility_permission_cannot_authorize_external_native_hosts() {
        let prepared = vec!["nativeMessaging".to_owned()];
        assert!(!external_native_messaging_allowed(&[], true));
        assert!(!external_native_messaging_allowed(&prepared, false));
        assert!(external_native_messaging_allowed(&prepared, true));
        assert!(!external_native_messaging_allowed(&[], false));
    }
}
