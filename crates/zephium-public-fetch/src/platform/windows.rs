use winreg::{enums::HKEY_CURRENT_USER, RegKey};

use super::{windows_route, Discovery};
use crate::route::Route;

const INTERNET_SETTINGS: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Internet Settings";

pub(crate) fn system_route() -> Route {
    let Ok(settings) = RegKey::predef(HKEY_CURRENT_USER).open_subkey(INTERNET_SETTINGS) else {
        return Route::Unsupported;
    };
    let auto_config = settings.get_value::<String, _>("AutoConfigURL").ok();
    let enabled = settings
        .get_value::<u32, _>("ProxyEnable")
        .is_ok_and(|enabled| enabled != 0);
    let server = settings.get_value::<String, _>("ProxyServer").ok();
    windows_route(
        auto_config.as_deref(),
        enabled,
        server.as_deref(),
        auto_discovery(),
    )
}

pub(crate) fn auto_discovery() -> Discovery {
    Discovery::proved(None, None)
}
