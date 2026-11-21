use system_configuration::core_foundation::{
    base::CFType, dictionary::CFDictionary, number::CFNumber, string::CFString,
};
use system_configuration::dynamic_store::SCDynamicStoreBuilder;

use super::{mac_route, MacProxies};
use crate::route::Route;

fn number(proxies: &CFDictionary<CFString, CFType>, key: &'static str) -> Option<i64> {
    proxies
        .find(CFString::from_static_string(key))
        .and_then(|value| value.downcast::<CFNumber>())
        .and_then(|value| value.to_i64())
}

fn text(proxies: &CFDictionary<CFString, CFType>, key: &'static str) -> Option<String> {
    proxies
        .find(CFString::from_static_string(key))
        .and_then(|value| value.downcast::<CFString>())
        .map(|value| value.to_string())
}

pub(crate) fn system_route() -> Route {
    let Some(store) = SCDynamicStoreBuilder::new("zephium-public-fetch").build() else {
        return Route::Unsupported;
    };
    let Some(proxies) = store.get_proxies() else {
        return Route::Unsupported;
    };
    let enabled = |key| number(&proxies, key) == Some(1);
    mac_route(&MacProxies {
        auto_config: enabled("ProxyAutoConfigEnable"),
        auto_discovery: enabled("ProxyAutoDiscoveryEnable"),
        https: enabled("HTTPSEnable"),
        https_host: text(&proxies, "HTTPSProxy"),
        https_port: number(&proxies, "HTTPSPort"),
        socks: enabled("SOCKSEnable"),
    })
}
