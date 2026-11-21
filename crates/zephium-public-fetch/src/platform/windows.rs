use windows_sys::core::PWSTR;
use windows_sys::Win32::Foundation::GlobalFree;
use windows_sys::Win32::Networking::WinHttp::{
    WinHttpDetectAutoProxyConfigUrl, WinHttpGetIEProxyConfigForCurrentUser,
    ERROR_WINHTTP_AUTODETECTION_FAILED, WINHTTP_AUTO_DETECT_TYPE_DHCP,
    WINHTTP_AUTO_DETECT_TYPE_DNS_A, WINHTTP_CURRENT_USER_IE_PROXY_CONFIG,
};
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

/// "Automatically detect settings" and, when it is on, whether DHCP or DNS
/// names a PAC address right now. Blocks for as long as detection takes.
pub(crate) fn auto_discovery() -> Discovery {
    let enabled = auto_detect_enabled();
    let discovered = if enabled == Some(true) {
        discovered()
    } else {
        None
    };
    Discovery::proved(enabled, discovered)
}

// Each non-null string WinHTTP returns below is the caller's, freed once with
// `GlobalFree` and never read.
fn free(text: PWSTR) {
    if !text.is_null() {
        // SAFETY: the invariant above; `text` is not used again.
        unsafe { GlobalFree(text.cast()) };
    }
}

fn auto_detect_enabled() -> Option<bool> {
    let mut config = WINHTTP_CURRENT_USER_IE_PROXY_CONFIG::default();
    // SAFETY: `config` is a writable struct for the call to fill.
    if unsafe { WinHttpGetIEProxyConfigForCurrentUser(&mut config) } == 0 {
        return None;
    }
    free(config.lpszAutoConfigUrl);
    free(config.lpszProxy);
    free(config.lpszProxyBypass);
    Some(config.fAutoDetect != 0)
}

fn discovered() -> Option<bool> {
    let mut address: PWSTR = std::ptr::null_mut();
    // SAFETY: `address` is a writable out-pointer for the call to fill.
    let found = unsafe {
        WinHttpDetectAutoProxyConfigUrl(
            WINHTTP_AUTO_DETECT_TYPE_DHCP | WINHTTP_AUTO_DETECT_TYPE_DNS_A,
            &mut address,
        )
    } != 0;
    if found {
        free(address);
        return Some(true);
    }
    let error = std::io::Error::last_os_error().raw_os_error();
    (error == i32::try_from(ERROR_WINHTTP_AUTODETECTION_FAILED).ok()).then_some(false)
}

#[cfg(test)]
mod tests {
    #[test]
    fn asking_this_host_whether_discovery_found_a_proxy_returns() {
        let _ = super::auto_discovery();
    }
}
