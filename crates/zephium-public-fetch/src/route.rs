use zephium_core::proxy::{ProxyKind, ProxyServer};

/// How a session reaches sites: the way the person's web views would, or not at all.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Route {
    Direct,
    /// HTTP CONNECT through one proxy for every https request.
    Http {
        host: String,
        port: u16,
    },
    /// A setting that cannot be reproduced exactly (PAC, discovery, SOCKS,
    /// an unreadable configuration). Reads refuse rather than go direct.
    Unsupported,
}

impl Route {
    pub(crate) fn proxy_url(&self) -> Option<String> {
        match self {
            Route::Http { host, port } if host.contains(':') => {
                Some(format!("http://[{host}]:{port}"))
            }
            Route::Http { host, port } => Some(format!("http://{host}:{port}")),
            Route::Direct | Route::Unsupported => None,
        }
    }
}

/// How an anonymous reader may reach public sites, without where.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PublicRoute {
    Direct,
    Proxied,
    /// Refuse the read: the route cannot be reproduced exactly.
    Unsupported,
}

impl PublicRoute {
    pub(crate) fn of(route: &Route) -> Self {
        match route {
            Route::Direct => PublicRoute::Direct,
            Route::Http { .. } => PublicRoute::Proxied,
            Route::Unsupported => PublicRoute::Unsupported,
        }
    }
}

/// The app's own proxy wins, as it does for the web views; otherwise the
/// operating system's.
pub(crate) fn route(manual: Option<&ProxyServer>, system: impl FnOnce() -> Route) -> Route {
    match manual {
        Some(ProxyServer {
            kind: ProxyKind::Http,
            host,
            port,
        }) => Route::Http {
            host: host.clone(),
            port: *port,
        },
        Some(ProxyServer {
            kind: ProxyKind::Socks5,
            ..
        }) => Route::Unsupported,
        None => system(),
    }
}

/// `host:port`, `host` (port 80) or `http://host:port`, checked as strictly
/// as the person's own proxy address.
pub(crate) fn http_proxy(entry: &str) -> Route {
    let entry = entry.trim();
    let rest = match entry.split_once("://") {
        Some((scheme, rest)) if scheme.eq_ignore_ascii_case("http") => rest,
        Some(_) => return Route::Unsupported,
        None => entry,
    };
    let rest = rest.strip_suffix('/').unwrap_or(rest);
    let has_port = match rest.strip_prefix('[') {
        Some(bracketed) => bracketed.contains("]:"),
        None => rest.contains(':'),
    };
    let address = if has_port {
        rest.to_owned()
    } else {
        format!("{rest}:80")
    };
    match zephium_core::proxy::parse_address(&address) {
        Some((host, port)) => Route::Http { host, port },
        None => Route::Unsupported,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zephium_core::proxy::from_settings;

    #[test]
    fn the_apps_proxy_wins_and_socks_is_never_bypassed() {
        let system = || Route::Direct;
        assert_eq!(route(None, system), Route::Direct);
        assert_eq!(
            route(
                from_settings("http", "proxy.example.com:3128").as_ref(),
                system
            ),
            Route::Http {
                host: "proxy.example.com".into(),
                port: 3128
            }
        );
        assert_eq!(
            route(from_settings("socks5", "127.0.0.1:9050").as_ref(), system),
            Route::Unsupported
        );
        assert_eq!(route(None, || Route::Unsupported), Route::Unsupported);
    }

    #[test]
    fn the_public_class_agrees_with_the_internal_route() {
        let proxy = Route::Http {
            host: "proxy.example.com".into(),
            port: 3128,
        };
        for (internal, public) in [
            (Route::Direct, PublicRoute::Direct),
            (proxy, PublicRoute::Proxied),
            (Route::Unsupported, PublicRoute::Unsupported),
        ] {
            assert_eq!(PublicRoute::of(&internal), public, "{internal:?}");
        }
    }

    #[test]
    fn proxy_entries_parse_strictly_and_fail_closed() {
        let http = |host: &str, port| Route::Http {
            host: host.into(),
            port,
        };
        assert_eq!(
            http_proxy("proxy.corp.example:8080"),
            http("proxy.corp.example", 8080)
        );
        assert_eq!(http_proxy("http://10.0.0.2:3128/"), http("10.0.0.2", 3128));
        assert_eq!(http_proxy("HTTP://proxy"), http("proxy", 80));
        assert_eq!(http_proxy("[::1]:8080"), http("::1", 8080));
        assert_eq!(http_proxy("[::1]"), http("::1", 80));
        for refused in [
            "https://proxy:443",
            "socks://proxy:1080",
            "socks5://proxy:1080",
            "user:pw@proxy:80",
            "proxy:0",
            "proxy:99999",
            "::1",
            "",
            "proxy:80 extra",
        ] {
            assert_eq!(http_proxy(refused), Route::Unsupported, "{refused}");
        }
        assert_eq!(
            http("::1", 8080).proxy_url().as_deref(),
            Some("http://[::1]:8080")
        );
        assert_eq!(
            http("proxy", 80).proxy_url().as_deref(),
            Some("http://proxy:80")
        );
        assert_eq!(Route::Direct.proxy_url(), None);
    }
}
