//! A manual proxy for a view's website data store. WebKit takes it as an
//! array of Network framework proxy configurations on the store (macOS 14,
//! the least Zephium runs on). The store keeps it for every view that shares
//! it, so a view built later with the system's proxy has to clear it.

use std::ffi::{c_char, CString};
use std::sync::atomic::{AtomicBool, Ordering};

use objc2::rc::Retained;
use objc2_foundation::{ns_string, NSArray, NSObject, NSObjectNSKeyValueCoding};
use objc2_web_kit::WKWebViewConfiguration;
use zephium_core::proxy::{ProxyKind, ProxyServer};

#[link(name = "Network", kind = "framework")]
extern "C" {
    fn nw_endpoint_create_host(host: *const c_char, port: *const c_char) -> *mut NSObject;
    fn nw_proxy_config_create_http_connect(
        endpoint: *mut NSObject,
        tls_options: *mut NSObject,
    ) -> *mut NSObject;
    fn nw_proxy_config_create_socksv5(endpoint: *mut NSObject) -> *mut NSObject;
}

/// Set once a proxy has been given to any store, after which a view meant for
/// the system's proxy writes an empty list.
static ANY_APPLIED: AtomicBool = AtomicBool::new(false);

fn configuration_for(proxy: &ProxyServer) -> Option<Retained<NSObject>> {
    let host = CString::new(proxy.host.as_str()).ok()?;
    let port = CString::new(proxy.port.to_string()).ok()?;
    // SAFETY: both strings outlive the call; each `nw_*_create_*` returns a
    // retained object or null, and the endpoint is retained by the proxy
    // configuration made from it.
    unsafe {
        let endpoint = Retained::from_raw(nw_endpoint_create_host(host.as_ptr(), port.as_ptr()))?;
        let endpoint = Retained::as_ptr(&endpoint).cast_mut();
        let created = match proxy.kind {
            ProxyKind::Http => nw_proxy_config_create_http_connect(endpoint, std::ptr::null_mut()),
            ProxyKind::Socks5 => nw_proxy_config_create_socksv5(endpoint),
        };
        Retained::from_raw(created)
    }
}

/// Points the configuration's data store at `proxy`, or back at the system's
/// with `None`. A manual proxy that cannot be set is an error: a view must
/// never load directly when the person asked for it to go through one.
pub(crate) fn apply(
    configuration: &WKWebViewConfiguration,
    proxy: Option<&ProxyServer>,
) -> Result<(), &'static str> {
    let list: Retained<NSArray<NSObject>> = match proxy {
        Some(proxy) => {
            let proxy = configuration_for(proxy).ok_or("the proxy could not be set up")?;
            ANY_APPLIED.store(true, Ordering::Release);
            NSArray::from_retained_slice(&[proxy])
        }
        None if ANY_APPLIED.load(Ordering::Acquire) => NSArray::new(),
        None => return Ok(()),
    };
    // SAFETY: `proxyConfigurations` is WKWebsiteDataStore's own key and takes
    // an array of proxy configurations; the store is main-thread state and
    // this runs while a view is being built there.
    unsafe {
        configuration
            .websiteDataStore()
            .setValue_forKey(Some(&list), ns_string!("proxyConfigurations"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use zephium_core::proxy::from_settings;

    #[test]
    fn network_framework_accepts_each_kind_of_validated_proxy() {
        for (mode, address) in [
            ("http", "proxy.example.com:3128"),
            ("socks5", "127.0.0.1:9050"),
            ("socks5", "[::1]:1080"),
        ] {
            let server = from_settings(mode, address).expect(address);
            assert!(configuration_for(&server).is_some(), "{mode} {address}");
        }
    }
}
