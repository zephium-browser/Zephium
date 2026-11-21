use system_configuration::core_foundation::{
    base::{CFType, TCFType},
    dictionary::CFDictionary,
    number::CFNumber,
    string::CFString,
};
use system_configuration::dynamic_store::SCDynamicStoreBuilder;

use super::{mac_route, Flag, MacProxies, Scoped, MAX_SCOPED_ENTRIES, SCOPED_ENABLE_KEYS};
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

fn flag(proxies: &CFDictionary<CFString, CFType>, key: &'static str) -> Flag {
    match proxies.find(CFString::from_static_string(key)) {
        None => Flag::Absent,
        Some(value) => value
            .downcast::<CFNumber>()
            .and_then(|value| value.to_i64())
            .map_or(Flag::Other, Flag::Number),
    }
}

fn scoped(proxies: &CFDictionary<CFString, CFType>) -> Scoped {
    let Some(value) = proxies.find(CFString::from_static_string("__SCOPED__")) else {
        return Scoped::Absent;
    };
    let Some(scoped) = value.downcast::<CFDictionary>() else {
        return Scoped::Unreadable;
    };
    if scoped.len() > MAX_SCOPED_ENTRIES {
        return Scoped::Unreadable;
    }
    let (_, values) = scoped.get_keys_and_values();
    Scoped::Entries(
        values
            .into_iter()
            .map(|value| {
                if value.is_null() {
                    return None;
                }
                // SAFETY: `value` is a non-null value of `scoped`, which is alive and retains it;
                // the get rule takes our own reference.
                let value = unsafe { CFType::wrap_under_get_rule(value) };
                let entry = value.downcast::<CFDictionary>()?;
                // SAFETY: `entry` is a live CFDictionary (checked by `downcast`) from a property
                // list, so every key and value is a CF object and may be read as `CFType`.
                let entry: CFDictionary<CFString, CFType> =
                    unsafe { TCFType::wrap_under_get_rule(entry.as_concrete_TypeRef()) };
                Some(SCOPED_ENABLE_KEYS.map(|key| flag(&entry, key)))
            })
            .collect(),
    )
}

fn read() -> Option<MacProxies> {
    let store = SCDynamicStoreBuilder::new("zephium-public-fetch").build()?;
    let proxies = store.get_proxies()?;
    let enabled = |key| number(&proxies, key) == Some(1);
    Some(MacProxies {
        auto_config: enabled("ProxyAutoConfigEnable"),
        auto_discovery: enabled("ProxyAutoDiscoveryEnable"),
        https: enabled("HTTPSEnable"),
        https_host: text(&proxies, "HTTPSProxy"),
        https_port: number(&proxies, "HTTPSPort"),
        socks: enabled("SOCKSEnable"),
        scoped: scoped(&proxies),
        supplemental: proxies
            .find(CFString::from_static_string("__SUPPLEMENTAL__"))
            .is_some(),
    })
}

pub(crate) fn system_route() -> Route {
    read().map_or(Route::Unsupported, |proxies| mac_route(&proxies))
}

#[cfg(test)]
mod tests {
    #[test]
    fn reading_this_hosts_scoped_proxy_dictionary_returns() {
        let _ = super::read();
    }
}
