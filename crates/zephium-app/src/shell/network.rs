//! Which proxy the person's pages go through.

use zephium_core::proxy::{self, ProxyServer};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct ProxyPreference {
    mode: String,
    address: String,
}

impl ProxyPreference {
    pub(super) fn load(store: &dyn zephium_core::ports::store::Store) -> Self {
        let mut preference = Self::default();
        for key in ["network.proxy", "network.proxy-address"] {
            if let Some(value) = store.app_setting(key) {
                preference.apply(key, &value);
            }
        }
        preference
    }

    /// Takes a validated value; anything else leaves the choice as it was.
    /// Returns whether the proxy in force may have changed.
    pub(super) fn apply(&mut self, key: &str, value: &str) -> bool {
        match key {
            "network.proxy" => self.mode = value.to_owned(),
            "network.proxy-address" => self.address = value.to_owned(),
            _ => return false,
        }
        true
    }

    /// The manual proxy, or none for the system's.
    pub(super) fn server(&self) -> Option<ProxyServer> {
        proxy::from_settings(&self.mode, &self.address)
    }
}
