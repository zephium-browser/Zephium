//! Windows has no API that makes an app the default browser; an app registers
//! itself as a candidate and the person chooses it in Settings. Registration
//! is per user, so no installer or elevation is involved, and the installer
//! removes this identity's keys on uninstall.

use std::ffi::c_void;
use windows::core::{w, HSTRING, PCWSTR};
use windows::Win32::Foundation::{ERROR_SUCCESS, WIN32_ERROR};
use windows::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyExW, RegGetValueW, RegSetValueExW, HKEY, HKEY_CURRENT_USER,
    HKEY_LOCAL_MACHINE, KEY_WRITE, REG_OPTION_NON_VOLATILE, REG_SZ, RRF_RT_REG_SZ,
};
use windows::Win32::UI::Shell::{SHChangeNotify, ShellExecuteW, SHCNE_ASSOCCHANGED, SHCNF_IDLIST};
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

const SCHEMES: [&str; 2] = ["http", "https"];

/// The names one installed identity registers under, so uninstalling Dev or
/// Nightly never removes Stable's registration. Stable's product name gives
/// the names it has always used. The uninstall hook removes the same keys.
#[derive(Debug, PartialEq, Eq)]
struct Registration {
    client: String,
    prog_id: String,
}

static REGISTRATION: std::sync::OnceLock<Registration> = std::sync::OnceLock::new();

impl Registration {
    fn for_product(product: &str) -> Self {
        let client: String = product
            .chars()
            .filter(|c| *c != '\\' && !c.is_control())
            .collect();
        let client = match client.trim() {
            "" => "Zephium".to_owned(),
            name => name.to_owned(),
        };
        // A ProgID holds letters, digits and dots, at most 39 of them. A name
        // that fits as it is (Stable's) is used whole; any other is shortened
        // and told apart by a hash of the full name, so "Zephium Dev" and
        // "ZephiumDev" never share a class.
        let plain = client.len() <= 36 && client.chars().all(|c| c.is_ascii_alphanumeric());
        let prog_id = if plain {
            format!("{client}URL")
        } else {
            let stem: String = client
                .chars()
                .filter(char::is_ascii_alphanumeric)
                .take(26)
                .collect();
            let hash = client.bytes().fold(0x811c_9dc5_u32, |hash, byte| {
                (hash ^ u32::from(byte)).wrapping_mul(0x0100_0193)
            });
            format!("{stem}.{hash:08x}URL")
        };
        Self { client, prog_id }
    }

    fn client_key(&self) -> String {
        format!(r"Software\Clients\StartMenuInternet\{}", self.client)
    }

    fn capabilities(&self) -> String {
        format!(r"{}\Capabilities", self.client_key())
    }
}

/// Takes this install's product name as its registration identity; called
/// once at launch.
pub(crate) fn claim(product: &str) {
    let _ = REGISTRATION.set(Registration::for_product(product));
}

fn registration() -> &'static Registration {
    REGISTRATION.get_or_init(|| Registration::for_product("Zephium"))
}

/// Whether the person's own choice for both web schemes is Zephium.
pub(crate) fn is_default() -> bool {
    SCHEMES.iter().all(|scheme| {
        let key = format!(
            r"Software\Microsoft\Windows\Shell\Associations\UrlAssociations\{scheme}\UserChoice"
        );
        read_string(&key, "ProgId").as_deref() == Some(registration().prog_id.as_str())
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
    let Registration { client, prog_id } = registration();
    let (client_key, capabilities) = (registration().client_key(), registration().capabilities());
    let entries: [(String, &str, String); 14] = [
        (
            format!(r"Software\Classes\{prog_id}"),
            "",
            format!("{client} Web Page"),
        ),
        (
            format!(r"Software\Classes\{prog_id}"),
            "URL Protocol",
            String::new(),
        ),
        (
            format!(r"Software\Classes\{prog_id}\DefaultIcon"),
            "",
            icon.clone(),
        ),
        (
            format!(r"Software\Classes\{prog_id}\shell\open\command"),
            "",
            open,
        ),
        (client_key.clone(), "", client.clone()),
        (format!(r"{client_key}\DefaultIcon"), "", icon.clone()),
        (
            format!(r"{client_key}\shell\open\command"),
            "",
            format!("\"{exe}\""),
        ),
        (capabilities.clone(), "ApplicationName", client.clone()),
        (
            capabilities.clone(),
            "ApplicationDescription",
            "A quiet, fast browser built around your work.".into(),
        ),
        (capabilities.clone(), "ApplicationIcon", icon),
        (
            format!(r"{capabilities}\StartMenu"),
            "StartMenuInternet",
            client.clone(),
        ),
        (
            format!(r"{capabilities}\URLAssociations"),
            "http",
            prog_id.clone(),
        ),
        (
            format!(r"{capabilities}\URLAssociations"),
            "https",
            prog_id.clone(),
        ),
        (
            r"Software\RegisteredApplications".into(),
            client,
            capabilities.clone(),
        ),
    ];
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
    let client = uri_component(&registration().client);
    let target = HSTRING::from(format!(
        "ms-settings:defaultapps?registeredAppUser={client}"
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

/// Percent-encodes all but the unreserved URI characters.
fn uri_component(text: &str) -> String {
    text.bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                char::from(byte).to_string()
            }
            byte => format!("%{byte:02X}"),
        })
        .collect()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stable_keeps_its_names_and_every_other_identity_has_its_own() {
        let stable = Registration::for_product("Zephium");
        assert_eq!(stable.client, "Zephium");
        assert_eq!(stable.prog_id, "ZephiumURL");
        assert_eq!(
            stable.capabilities(),
            r"Software\Clients\StartMenuInternet\Zephium\Capabilities"
        );
        let dev = Registration::for_product("Zephium Dev");
        assert_eq!(dev.client, "Zephium Dev");
        let joined = Registration::for_product("ZephiumDev");
        assert_eq!(joined.prog_id, "ZephiumDevURL");
        assert_ne!(dev.prog_id, joined.prog_id);
        assert!(dev.prog_id.starts_with("ZephiumDev.") && dev.prog_id.ends_with("URL"));
        let qa = Registration::for_product("Zephium Protection QA");
        assert_ne!(qa.client_key(), stable.client_key());
        assert_ne!(qa.prog_id, stable.prog_id);
    }

    #[test]
    fn a_name_cannot_reach_outside_its_own_keys() {
        let odd = Registration::for_product("Zephium\\..\\Classes\u{7}");
        assert_eq!(odd.client, "Zephium..Classes");
        assert_eq!(Registration::for_product("  ").client, "Zephium");
        for name in ["Z".repeat(80), "Zephium Nightly ".repeat(5)] {
            let prog_id = Registration::for_product(&name).prog_id;
            assert!(prog_id.len() <= 39, "{prog_id}");
            assert!(prog_id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '.'));
        }
    }

    #[test]
    fn the_settings_link_carries_the_name_as_one_uri_component() {
        assert_eq!(uri_component("Zephium"), "Zephium");
        assert_eq!(
            uri_component("Zephium Dev #2 & 100%"),
            "Zephium%20Dev%20%232%20%26%20100%25"
        );
    }
}
