use webkit2gtk::{CookieManagerExt, WebsiteDataManager, WebsiteDataManagerExt};

pub(super) fn configure(manager: &WebsiteDataManager) -> Result<(), String> {
    let cookies = manager.cookie_manager().ok_or("missing cookie manager")?;
    // ITP owns third-party admission while enabled. NoThirdParty remains the
    // fallback; modern WebKit's ITP mode blocks third-party cookies by default.
    cookies.set_accept_policy(webkit2gtk::CookieAcceptPolicy::NoThirdParty);
    manager.set_itp_enabled(true);
    manager.set_persistent_credential_storage_enabled(false);
    if !manager.is_itp_enabled() || manager.is_persistent_credential_storage_enabled() {
        return Err("WebKitGTK did not retain required data policy".into());
    }
    apply_proxy(manager);
    Ok(())
}

pub(crate) fn apply_proxy(manager: &WebsiteDataManager) {
    match crate::network::current() {
        Some(proxy) => {
            let mut settings = webkit2gtk::NetworkProxySettings::new(Some(&proxy_uri(&proxy)), &[]);
            manager.set_network_proxy_settings(
                webkit2gtk::NetworkProxyMode::Custom,
                Some(&mut settings),
            );
        }
        None => manager.set_network_proxy_settings(webkit2gtk::NetworkProxyMode::Default, None),
    }
}

fn proxy_uri(proxy: &zephium_core::proxy::ProxyServer) -> String {
    let scheme = match proxy.kind {
        zephium_core::proxy::ProxyKind::Http => "http",
        zephium_core::proxy::ProxyKind::Socks5 => "socks5",
    };
    let host = if proxy.host.contains(':') {
        format!("[{}]", proxy.host)
    } else {
        proxy.host.clone()
    };
    format!("{scheme}://{host}:{}", proxy.port)
}

#[cfg(test)]
mod tests {
    #[test]
    fn proxy_settings_preserve_ipv6_and_protocol() {
        let proxy = zephium_core::proxy::from_settings("socks5", "[::1]:1080").unwrap();
        assert_eq!(super::proxy_uri(&proxy), "socks5://[::1]:1080");
        let proxy = zephium_core::proxy::from_settings("http", "localhost:8080").unwrap();
        assert_eq!(super::proxy_uri(&proxy), "http://localhost:8080");
    }
}
