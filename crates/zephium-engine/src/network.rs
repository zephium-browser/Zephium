//! The proxy new content views are created with. The shell sets it from the
//! person's preference before any view exists and again when it changes;
//! a view takes whatever is current as it is built.

use std::sync::RwLock;

use zephium_core::proxy::ProxyServer;

static PROXY: RwLock<Option<ProxyServer>> = RwLock::new(None);

pub(crate) fn set(proxy: Option<ProxyServer>) {
    *PROXY
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = proxy;
}

pub(crate) fn current() -> Option<ProxyServer> {
    PROXY
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
}

/// WebView2's browser arguments for a new environment. An environment keeps
/// them for as long as its browser process runs.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(crate) fn browser_arguments() -> String {
    let mut arguments = String::from("--disable-features=msWebOOUI,msPdfOOUI");
    if let Some(proxy) = current() {
        arguments.push(' ');
        arguments.push_str(&proxy.browser_argument());
    }
    arguments
}

#[cfg(test)]
mod tests {
    use super::*;
    use zephium_core::proxy::from_settings;

    #[test]
    fn a_new_environment_carries_the_proxy_chosen_when_it_is_made() {
        static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _serial = SERIAL
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        set(None);
        assert_eq!(
            browser_arguments(),
            "--disable-features=msWebOOUI,msPdfOOUI"
        );
        set(from_settings("socks5", "127.0.0.1:9050"));
        assert_eq!(
            browser_arguments(),
            "--disable-features=msWebOOUI,msPdfOOUI --proxy-server=socks5://127.0.0.1:9050"
        );
        set(None);
        assert_eq!(current(), None);
    }
}
