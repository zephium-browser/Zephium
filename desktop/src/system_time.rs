//! Whether the system clock is set to 24-hour time. WebKit and WebView2 take
//! the hour cycle from the page's language, not from the person's setting, so
//! the "system" clock format asks the operating system itself.

use super::*;

/// Whether a short-time pattern (ICU on macOS, NLS on Windows) counts hours
/// 0-23: its hour field is `H` (or `k`), where a 12-hour pattern has `h`.
fn is_24_hour(pattern: &str) -> bool {
    pattern
        .split('\'')
        .step_by(2)
        .any(|literal_free| literal_free.contains(['H', 'k']))
}

#[cfg(target_os = "macos")]
fn short_time_pattern() -> Option<String> {
    use objc2_foundation::{ns_string, NSDateFormatter, NSLocale};
    // "j" is the locale's preferred hour; the autoupdating locale carries the
    // 24-Hour Time switch of System Settings.
    NSDateFormatter::dateFormatFromTemplate_options_locale(
        ns_string!("j"),
        0,
        Some(&NSLocale::autoupdatingCurrentLocale()),
    )
    .map(|pattern| pattern.to_string())
}

#[cfg(target_os = "windows")]
fn short_time_pattern() -> Option<String> {
    use windows::core::PCWSTR;
    use windows::Win32::Globalization::{GetLocaleInfoEx, LOCALE_SSHORTTIME};
    // A null locale name is LOCALE_NAME_USER_DEFAULT.
    // SAFETY: the buffer is owned here and the call writes at most its length.
    unsafe {
        let mut buffer = [0u16; 80];
        let length = GetLocaleInfoEx(PCWSTR::null(), LOCALE_SSHORTTIME, Some(&mut buffer));
        let length = usize::try_from(length).ok().filter(|length| *length > 1)?;
        String::from_utf16(&buffer[..length - 1]).ok()
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn short_time_pattern() -> Option<String> {
    None
}

/// `None` where the system has no such setting to read: the page then keeps
/// its language's own hour cycle.
#[tauri::command]
#[specta::specta]
pub(crate) fn system_clock_24h(caller: WebviewWindow) -> Option<bool> {
    if !authorize(&caller, CallerPolicy::Main, "system_clock_24h") {
        return None;
    }
    short_time_pattern().map(|pattern| is_24_hour(&pattern))
}

#[cfg(test)]
mod tests {
    use super::is_24_hour;

    #[test]
    fn the_hour_field_decides_the_cycle() {
        for pattern in ["HH", "H", "HH:mm", "H:mm", "H時", "kk"] {
            assert!(is_24_hour(pattern), "{pattern}");
        }
        for pattern in ["h a", "h:mm tt", "hh:mm:ss tt", "a h", "ah時"] {
            assert!(!is_24_hour(pattern), "{pattern}");
        }
    }

    #[test]
    fn quoted_text_is_not_a_field() {
        assert!(!is_24_hour("h 'Hrs' a"));
        assert!(is_24_hour("HH 'Uhr'"));
    }
}
