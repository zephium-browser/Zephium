//! The operating system's proxy, read the way each platform's web view reads
//! it. Anything it cannot reproduce exactly is `Unsupported`, never direct.

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod windows;

use crate::route::Route;

#[cfg(target_os = "macos")]
pub(crate) use macos::system_route;
#[cfg(target_os = "windows")]
pub(crate) use windows::system_route;

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub(crate) fn system_route() -> Route {
    Route::Unsupported
}

/// The CFNetwork proxy dictionary keys WebKit consults for an https page.
#[cfg(any(target_os = "macos", test))]
#[derive(Clone, Debug, Default)]
pub(crate) struct MacProxies {
    pub(crate) auto_config: bool,
    pub(crate) auto_discovery: bool,
    pub(crate) https: bool,
    pub(crate) https_host: Option<String>,
    pub(crate) https_port: Option<i64>,
    pub(crate) socks: bool,
    pub(crate) scoped: Scoped,
    pub(crate) supplemental: bool,
}

#[cfg(any(target_os = "macos", test))]
pub(crate) const MAX_SCOPED_ENTRIES: usize = 64;

/// Per-interface keys under `__SCOPED__` that route traffic through a proxy.
#[cfg(any(target_os = "macos", test))]
pub(crate) const SCOPED_ENABLE_KEYS: [&str; 5] = [
    "HTTPEnable",
    "HTTPSEnable",
    "SOCKSEnable",
    "ProxyAutoConfigEnable",
    "ProxyAutoDiscoveryEnable",
];

#[cfg(any(target_os = "macos", test))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Flag {
    Absent,
    Number(i64),
    Other,
}

/// The `__SCOPED__` dictionary: one entry per interface, `None` for an entry
/// that is not a dictionary.
#[cfg(any(target_os = "macos", test))]
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) enum Scoped {
    #[default]
    Absent,
    Unreadable,
    Entries(Vec<Option<[Flag; SCOPED_ENABLE_KEYS.len()]>>),
}

#[cfg(any(target_os = "macos", test))]
impl Scoped {
    fn enables_anything(&self) -> bool {
        match self {
            Scoped::Absent => false,
            Scoped::Unreadable => true,
            Scoped::Entries(entries) => {
                entries.len() > MAX_SCOPED_ENTRIES
                    || entries.iter().any(|entry| {
                        entry.is_none_or(|flags| {
                            flags
                                .iter()
                                .any(|flag| !matches!(flag, Flag::Absent | Flag::Number(0)))
                        })
                    })
            }
        }
    }
}

#[cfg(any(target_os = "macos", test))]
pub(crate) fn mac_route(proxies: &MacProxies) -> Route {
    if proxies.auto_config
        || proxies.auto_discovery
        || proxies.supplemental
        || proxies.scoped.enables_anything()
    {
        return Route::Unsupported;
    }
    if proxies.https {
        let host = proxies.https_host.as_deref().map(str::trim).unwrap_or("");
        let Some(port) = proxies
            .https_port
            .and_then(|port| u16::try_from(port).ok())
            .filter(|port| *port != 0)
        else {
            return Route::Unsupported;
        };
        let entry = if host.contains(':') {
            format!("[{host}]:{port}")
        } else {
            format!("{host}:{port}")
        };
        return crate::route::http_proxy(&entry);
    }
    if proxies.socks {
        return Route::Unsupported;
    }
    Route::Direct
}

/// "Automatically detect settings" (WPAD), as far as the system can prove it.
#[cfg(any(target_os = "windows", test))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Discovery {
    Off,
    NothingFound,
    Found,
    Unknown,
}

#[cfg(any(target_os = "windows", test))]
impl Discovery {
    /// Each `None` is a fact the system did not prove.
    pub(crate) fn proved(enabled: Option<bool>, discovered: Option<bool>) -> Self {
        match (enabled, discovered) {
            (Some(false), _) => Discovery::Off,
            (Some(true), Some(false)) => Discovery::NothingFound,
            (Some(true), Some(true)) => Discovery::Found,
            (None, _) | (Some(true), None) => Discovery::Unknown,
        }
    }
}

/// WinINET's per-user settings, as WebView2 reads them: discovery or a PAC
/// address wins, then a manual `ProxyServer` list's `https=` entry, then its
/// `socks=` entry.
#[cfg(any(target_os = "windows", test))]
pub(crate) fn windows_route(
    auto_config: Option<&str>,
    enabled: bool,
    server: Option<&str>,
    discovery: Discovery,
) -> Route {
    if matches!(discovery, Discovery::Found | Discovery::Unknown)
        || auto_config.is_some_and(|address| !address.trim().is_empty())
    {
        return Route::Unsupported;
    }
    let server = server.map(str::trim).unwrap_or("");
    if !enabled || server.is_empty() {
        return Route::Direct;
    }
    if !server.contains('=') {
        return crate::route::http_proxy(server);
    }
    let mut https = None;
    let mut socks = false;
    for entry in server
        .split(';')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
    {
        let Some((scheme, value)) = entry.split_once('=') else {
            return Route::Unsupported;
        };
        match scheme.trim().to_ascii_lowercase().as_str() {
            "https" => https = Some(value),
            "socks" => socks = true,
            _ => {}
        }
    }
    match (https, socks) {
        (Some(value), _) => crate::route::http_proxy(value),
        (None, true) => Route::Unsupported,
        (None, false) => Route::Direct,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn http(host: &str, port: u16) -> Route {
        Route::Http {
            host: host.into(),
            port,
        }
    }

    #[test]
    fn mac_settings_map_to_what_webkit_does_for_https() {
        assert_eq!(mac_route(&MacProxies::default()), Route::Direct);
        let https = MacProxies {
            https: true,
            https_host: Some("proxy.corp.example".into()),
            https_port: Some(8080),
            ..MacProxies::default()
        };
        assert_eq!(mac_route(&https), http("proxy.corp.example", 8080));
        let ipv6 = MacProxies {
            https_host: Some("::1".into()),
            ..https.clone()
        };
        assert_eq!(mac_route(&ipv6), http("::1", 8080));
        for unsupported in [
            MacProxies {
                auto_config: true,
                ..https.clone()
            },
            MacProxies {
                auto_discovery: true,
                ..MacProxies::default()
            },
            MacProxies {
                socks: true,
                ..MacProxies::default()
            },
            MacProxies {
                https_port: None,
                ..https.clone()
            },
            MacProxies {
                https_port: Some(70000),
                ..https.clone()
            },
            MacProxies {
                https_host: None,
                ..https.clone()
            },
        ] {
            assert_eq!(
                mac_route(&unsupported),
                Route::Unsupported,
                "{unsupported:?}"
            );
        }
        let both = MacProxies {
            socks: true,
            ..https.clone()
        };
        assert_eq!(mac_route(&both), http("proxy.corp.example", 8080));
    }

    #[test]
    fn per_domain_proxy_settings_are_unsupported() {
        let supplemental = MacProxies {
            supplemental: true,
            ..MacProxies::default()
        };
        assert_eq!(mac_route(&supplemental), Route::Unsupported);
        let with_proxy = MacProxies {
            https: true,
            https_host: Some("proxy.corp.example".into()),
            https_port: Some(8080),
            ..supplemental
        };
        assert_eq!(mac_route(&with_proxy), Route::Unsupported);
    }

    #[test]
    fn a_scoped_interface_proxy_is_unsupported_and_a_harmless_one_is_not() {
        let scoped = |scoped| MacProxies {
            scoped,
            ..MacProxies::default()
        };
        let harmless = [
            Flag::Absent,
            Flag::Number(0),
            Flag::Absent,
            Flag::Number(0),
            Flag::Absent,
        ];
        for entries in [
            vec![],
            vec![Some([Flag::Absent; 5])],
            vec![Some(harmless); 2],
        ] {
            assert_eq!(mac_route(&scoped(Scoped::Entries(entries))), Route::Direct);
        }
        assert_eq!(
            mac_route(&scoped(Scoped::Entries(vec![
                Some(harmless);
                MAX_SCOPED_ENTRIES
            ]))),
            Route::Direct
        );
        for key in 0..SCOPED_ENABLE_KEYS.len() {
            for value in [Flag::Number(1), Flag::Number(2), Flag::Other] {
                let mut flags = harmless;
                flags[key] = value;
                let entries = vec![Some(harmless), Some(flags)];
                assert_eq!(
                    mac_route(&scoped(Scoped::Entries(entries))),
                    Route::Unsupported,
                    "{} {value:?}",
                    SCOPED_ENABLE_KEYS[key]
                );
            }
        }
        let https = MacProxies {
            https: true,
            https_host: Some("proxy.corp.example".into()),
            https_port: Some(8080),
            scoped: Scoped::Entries(vec![Some([Flag::Number(1); 5])]),
            ..MacProxies::default()
        };
        assert_eq!(mac_route(&https), Route::Unsupported);
        for unreadable in [
            Scoped::Unreadable,
            Scoped::Entries(vec![Some(harmless), None]),
            Scoped::Entries(vec![Some(harmless); MAX_SCOPED_ENTRIES + 1]),
        ] {
            assert_eq!(mac_route(&scoped(unreadable)), Route::Unsupported);
        }
    }

    #[test]
    fn windows_settings_map_to_what_webview2_does_for_https() {
        let cases = [
            (None, false, Some("proxy:8080"), Route::Direct),
            (None, true, None, Route::Direct),
            (Some(""), true, Some("proxy:8080"), http("proxy", 8080)),
            (
                Some("http://wpad/proxy.pac"),
                false,
                None,
                Route::Unsupported,
            ),
            (None, true, Some("http://proxy:3128"), http("proxy", 3128)),
            (
                None,
                true,
                Some("http=a:80;https=b.corp.example:8443;ftp=c:21"),
                http("b.corp.example", 8443),
            ),
            (None, true, Some("http=a:80;ftp=c:21"), Route::Direct),
            (
                None,
                true,
                Some("http=a:80;socks=s:1080"),
                Route::Unsupported,
            ),
            (
                None,
                true,
                Some("https=socks5://s:1080"),
                Route::Unsupported,
            ),
            (None, true, Some("https=b:1;garbage"), Route::Unsupported),
            (None, true, Some("socks5://s:1080"), Route::Unsupported),
        ];
        assert_eq!(Discovery::proved(Some(false), None), Discovery::Off);
        assert_eq!(
            Discovery::proved(Some(true), Some(false)),
            Discovery::NothingFound
        );
        assert_eq!(Discovery::proved(Some(true), Some(true)), Discovery::Found);
        for unproved in [(None, None), (None, Some(false)), (Some(true), None)] {
            assert_eq!(
                Discovery::proved(unproved.0, unproved.1),
                Discovery::Unknown
            );
        }
        for (auto_config, enabled, server, manual) in cases {
            for (discovery, expected) in [
                (Discovery::Off, manual.clone()),
                (Discovery::NothingFound, manual.clone()),
                (Discovery::Found, Route::Unsupported),
                (Discovery::Unknown, Route::Unsupported),
            ] {
                assert_eq!(
                    windows_route(auto_config, enabled, server, discovery),
                    expected,
                    "{auto_config:?} {enabled} {server:?} {discovery:?}"
                );
            }
        }
    }

    #[test]
    fn reading_this_hosts_proxy_settings_does_not_fail_open_or_panic() {
        let _ = system_route();
    }

    proptest! {
        #[test]
        fn no_proxy_setting_text_panics(auto in proptest::option::of("\\PC{0,40}"), enabled in any::<bool>(), server in proptest::option::of("[ -~]{0,80}"), discovery in proptest::sample::select(vec![Discovery::Off, Discovery::NothingFound, Discovery::Found, Discovery::Unknown])) {
            let route = windows_route(auto.as_deref(), enabled, server.as_deref(), discovery);
            if matches!(discovery, Discovery::Found | Discovery::Unknown) {
                prop_assert_eq!(route, Route::Unsupported);
            }
        }
    }
}
