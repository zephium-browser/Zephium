use windows_sys::core::PWSTR;
use windows_sys::Win32::Foundation::GlobalFree;
use windows_sys::Win32::Networking::WinHttp::{
    WinHttpDetectAutoProxyConfigUrl, WinHttpGetIEProxyConfigForCurrentUser,
    ERROR_WINHTTP_AUTODETECTION_FAILED, WINHTTP_AUTO_DETECT_TYPE_DHCP,
    WINHTTP_AUTO_DETECT_TYPE_DNS_A, WINHTTP_CURRENT_USER_IE_PROXY_CONFIG,
};

use super::{windows_route, Discovery};
use crate::route::Route;

pub(crate) fn system_route() -> Route {
    let Some(settings) = settings() else {
        return Route::Unsupported;
    };
    let discovery = Discovery::proved(
        Some(settings.auto_detect),
        settings.auto_detect.then(discovered).flatten(),
    );
    windows_route(
        settings.auto_config.as_deref(),
        settings.proxy.is_some(),
        settings.proxy.as_deref(),
        discovery,
    )
}

/// The current user's settings for the active connection, as WinINET and
/// WebView2 read them. A manual proxy or PAC address is only present while
/// it is turned on.
struct Settings {
    auto_detect: bool,
    auto_config: Option<String>,
    proxy: Option<String>,
}

/// `None` when the settings cannot be read or a string in them is not UTF-16.
fn settings() -> Option<Settings> {
    let mut config = WINHTTP_CURRENT_USER_IE_PROXY_CONFIG::default();
    // SAFETY: `config` is a writable struct for the call to fill.
    if unsafe { WinHttpGetIEProxyConfigForCurrentUser(&mut config) } == 0 {
        return None;
    }
    // SAFETY: on success each string is null or a NUL-terminated buffer the
    // caller owns, and none has been freed yet.
    let auto_config = unsafe { text(config.lpszAutoConfigUrl) };
    // SAFETY: as above.
    let proxy = unsafe { text(config.lpszProxy) };
    // SAFETY: the call above handed this string to the caller; it was read
    // above if at all and is not used again.
    unsafe { free(config.lpszAutoConfigUrl) };
    // SAFETY: as above.
    unsafe { free(config.lpszProxy) };
    // SAFETY: as above; the bypass list is never read.
    unsafe { free(config.lpszProxyBypass) };
    Some(Settings {
        auto_detect: config.fAutoDetect != 0,
        auto_config: auto_config?,
        proxy: proxy?,
    })
}

/// Whether DHCP or DNS names a PAC address right now; `None` when detection
/// failed for any other reason. Blocks for as long as detection takes.
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
        // SAFETY: on success the call handed `address` to the caller; it is
        // never read and not used again.
        unsafe { free(address) };
        return Some(true);
    }
    let error = std::io::Error::last_os_error().raw_os_error();
    (error == i32::try_from(ERROR_WINHTTP_AUTODETECTION_FAILED).ok()).then_some(false)
}

/// A copy of `text`: `Some(None)` when it is null, `None` when it is not
/// UTF-16.
///
/// # Safety
/// `text` is null or a NUL-terminated buffer that stays valid for the call.
unsafe fn text(text: PWSTR) -> Option<Option<String>> {
    if text.is_null() {
        return Some(None);
    }
    let mut length = 0;
    // SAFETY: the buffer is NUL-terminated, so each unit up to the NUL is in it.
    while unsafe { *text.add(length) } != 0 {
        length += 1;
    }
    // SAFETY: the `length` units before the NUL were just read in place.
    let units = unsafe { std::slice::from_raw_parts(text, length) };
    String::from_utf16(units).ok().map(Some)
}

/// Frees a string WinHTTP handed to the caller.
///
/// # Safety
/// `text` is null or such a string, not yet freed and not used again.
unsafe fn free(text: PWSTR) {
    if !text.is_null() {
        // SAFETY: the caller's guarantee.
        unsafe { GlobalFree(text.cast()) };
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn reading_this_hosts_settings_in_one_call_returns() {
        let _ = super::settings();
    }
}
