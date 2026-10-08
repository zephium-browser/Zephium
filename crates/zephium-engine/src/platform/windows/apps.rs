//! Hands an application link the person allowed to the app Windows has
//! registered for its scheme.
use windows::core::PCWSTR;
use windows::Win32::UI::Shell::{
    AssocQueryStringW, ShellExecuteW, ASSOCF_INIT_IGNOREUNKNOWN, ASSOCF_IS_PROTOCOL,
    ASSOCSTR_FRIENDLYAPPNAME,
};
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

/// The registered app's display name. None when nothing handles the scheme.
pub(crate) fn external_app_name(url: &str) -> Option<String> {
    let link = zephium_core::navigation::external_app_link(url)?;
    let scheme = wide(link.scheme());
    let mut name = [0u16; 260];
    let mut len = name.len() as u32;
    // SAFETY: both buffers outlive the call and `len` is the output capacity.
    unsafe {
        AssocQueryStringW(
            ASSOCF_IS_PROTOCOL | ASSOCF_INIT_IGNOREUNKNOWN,
            ASSOCSTR_FRIENDLYAPPNAME,
            PCWSTR(scheme.as_ptr()),
            PCWSTR::null(),
            Some(windows::core::PWSTR(name.as_mut_ptr())),
            &mut len,
        )
    }
    .ok()
    .ok()?;
    let end = name.iter().position(|&c| c == 0).unwrap_or(name.len());
    let name = String::from_utf16_lossy(&name[..end]);
    (!name.trim().is_empty()).then_some(name)
}

pub(crate) fn open_external_app(url: &str) -> bool {
    if zephium_core::navigation::external_app_link(url).is_none() {
        return false;
    }
    let target = wide(url);
    // SAFETY: a validated link as the file argument of a plain "open" verb;
    // ShellExecute reports success as any value above 32.
    let result = unsafe {
        ShellExecuteW(
            None,
            windows::core::w!("open"),
            PCWSTR(target.as_ptr()),
            None,
            None,
            SW_SHOWNORMAL,
        )
    };
    result.0 as isize > 32
}
