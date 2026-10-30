//! Windows has no API that makes an app the default browser; an app registers
//! itself as a candidate and the person chooses it in Settings. Registration
//! is per user, so no installer or elevation is involved, and both installers
//! remove these keys on uninstall.

use std::ffi::c_void;
use windows::core::{w, HSTRING, PCWSTR};
use windows::Win32::Foundation::{ERROR_SUCCESS, WIN32_ERROR};
use windows::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyExW, RegGetValueW, RegSetValueExW, HKEY, HKEY_CURRENT_USER,
    HKEY_LOCAL_MACHINE, KEY_WRITE, REG_OPTION_NON_VOLATILE, REG_SZ, RRF_RT_REG_SZ,
};
use windows::Win32::UI::Shell::{SHChangeNotify, ShellExecuteW, SHCNE_ASSOCCHANGED, SHCNF_IDLIST};
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

pub(crate) const CLIENT: &str = "Zephium";
pub(crate) const PROG_ID: &str = "ZephiumURL";
/// Handler for local web pages, with no `URL Protocol` value: Windows treats
/// a ProgId that has one as a link scheme, not a file type.
const HTML_PROG_ID: &str = "ZephiumHTML";
const FILE_TYPES: [&str; 4] = [".htm", ".html", ".xhtml", ".shtml"];
const CLIENT_KEY: &str = r"Software\Clients\StartMenuInternet\Zephium";
const CAPABILITIES: &str = r"Software\Clients\StartMenuInternet\Zephium\Capabilities";
const SCHEMES: [&str; 2] = ["http", "https"];

/// Whether the person's own choice for both web schemes is Zephium.
pub(crate) fn is_default() -> bool {
    SCHEMES.iter().all(|scheme| {
        let key = format!(
            r"Software\Microsoft\Windows\Shell\Associations\UrlAssociations\{scheme}\UserChoice"
        );
        read_string(&key, "ProgId").as_deref() == Some(PROG_ID)
    })
}

/// Registers Zephium as a browser for the current user. Idempotent; the
/// command line always points at the running executable, so it follows a
/// moved or updated install.
pub(crate) fn register() -> bool {
    let Ok(exe) = std::env::current_exe() else {
        return false;
    };
    let exe = exe.display().to_string();
    let open = format!("\"{exe}\" \"%1\"");
    let icon = format!("\"{exe}\",0");
    let mut entries: Vec<(String, &str, String)> = vec![
        (
            format!(r"Software\Classes\{PROG_ID}"),
            "",
            "Zephium Web Page".into(),
        ),
        (
            format!(r"Software\Classes\{PROG_ID}"),
            "URL Protocol",
            String::new(),
        ),
        (
            format!(r"Software\Classes\{PROG_ID}\DefaultIcon"),
            "",
            icon.clone(),
        ),
        (
            format!(r"Software\Classes\{PROG_ID}\shell\open\command"),
            "",
            open,
        ),
        (CLIENT_KEY.into(), "", CLIENT.into()),
        (format!(r"{CLIENT_KEY}\DefaultIcon"), "", icon.clone()),
        (
            format!(r"{CLIENT_KEY}\shell\open\command"),
            "",
            format!("\"{exe}\""),
        ),
        (CAPABILITIES.into(), "ApplicationName", CLIENT.into()),
        (
            CAPABILITIES.into(),
            "ApplicationDescription",
            "A quiet, fast browser built around your work.".into(),
        ),
        (CAPABILITIES.into(), "ApplicationIcon", icon),
        (
            format!(r"{CAPABILITIES}\StartMenu"),
            "StartMenuInternet",
            CLIENT.into(),
        ),
        (
            format!(r"{CAPABILITIES}\URLAssociations"),
            "http",
            PROG_ID.into(),
        ),
        (
            format!(r"{CAPABILITIES}\URLAssociations"),
            "https",
            PROG_ID.into(),
        ),
        (
            r"Software\RegisteredApplications".into(),
            CLIENT,
            CAPABILITIES.into(),
        ),
        (
            format!(r"Software\Classes\{HTML_PROG_ID}"),
            "",
            "Zephium HTML Document".into(),
        ),
        (
            format!(r"Software\Classes\{HTML_PROG_ID}\DefaultIcon"),
            "",
            format!("\"{exe}\",0"),
        ),
        (
            format!(r"Software\Classes\{HTML_PROG_ID}\shell\open\command"),
            "",
            format!("\"{exe}\" \"%1\""),
        ),
    ];
    entries.extend(FILE_TYPES.iter().map(|extension| {
        (
            format!(r"{CAPABILITIES}\FileAssociations"),
            *extension,
            HTML_PROG_ID.into(),
        )
    }));
    let written = entries
        .iter()
        .all(|(key, name, value)| write_string(key, name, value));
    // SAFETY: a documented broadcast with no item pointers.
    unsafe { SHChangeNotify(SHCNE_ASSOCCHANGED, SHCNF_IDLIST, None, None) };
    written
}

/// Opens Default apps at Zephium's own page (Windows 11), or the list on
/// Windows 10, which ignores the parameter.
pub(crate) fn open_settings() -> bool {
    let target = HSTRING::from(format!(
        "ms-settings:defaultapps?registeredAppUser={CLIENT}"
    ));
    // SAFETY: all strings outlive the call; no window owner is needed.
    let result = unsafe {
        ShellExecuteW(
            None,
            w!("open"),
            &target,
            PCWSTR::null(),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        )
    };
    // ShellExecute reports success as any value above 32.
    result.0 as usize > 32
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

fn write_string(key: &str, name: &str, value: &str) -> bool {
    let key = wide(key);
    let name = wide(name);
    let data: Vec<u8> = wide(value).into_iter().flat_map(u16::to_le_bytes).collect();
    let mut handle = HKEY::default();
    // SAFETY: pointers come from live buffers; the handle is closed below.
    unsafe {
        if RegCreateKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(key.as_ptr()),
            None,
            PCWSTR::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_WRITE,
            None,
            &mut handle,
            None,
        ) != ERROR_SUCCESS
        {
            return false;
        }
        let written = RegSetValueExW(handle, PCWSTR(name.as_ptr()), None, REG_SZ, Some(&data));
        let _ = RegCloseKey(handle);
        written == ERROR_SUCCESS
    }
}

fn read_string(key: &str, name: &str) -> Option<String> {
    read_string_in(HKEY_CURRENT_USER, key, name)
}

/// A machine-wide string value, such as the Windows build About reports.
pub(crate) fn read_machine_string(key: &str, name: &str) -> Option<String> {
    read_string_in(HKEY_LOCAL_MACHINE, key, name)
}

fn read_string_in(root: HKEY, key: &str, name: &str) -> Option<String> {
    let key = wide(key);
    let name = wide(name);
    let mut buffer = [0u16; 128];
    let mut size = std::mem::size_of_val(&buffer) as u32;
    // SAFETY: the buffer and its byte size describe the same live array.
    let status: WIN32_ERROR = unsafe {
        RegGetValueW(
            root,
            PCWSTR(key.as_ptr()),
            PCWSTR(name.as_ptr()),
            RRF_RT_REG_SZ,
            None,
            Some(buffer.as_mut_ptr().cast::<c_void>()),
            Some(&mut size),
        )
    };
    if status != ERROR_SUCCESS {
        return None;
    }
    let length = (size as usize / 2).saturating_sub(1).min(buffer.len());
    Some(String::from_utf16_lossy(&buffer[..length]))
}
