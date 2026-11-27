//! What Settings shows under About, and what it copies into a bug report:
//! the release, the operating system and the processor family. Nothing about
//! the person or their browsing.

use super::*;

#[derive(Clone, Debug, Serialize, specta::Type)]
pub(crate) struct AboutInfo {
    pub(crate) version: String,
    /// Product name and version, such as "macOS 26.1.0".
    pub(crate) os: String,
    pub(crate) arch: String,
}

#[tauri::command]
#[specta::specta]
pub(crate) fn about_info(caller: WebviewWindow, app: tauri::AppHandle) -> Option<AboutInfo> {
    if !authorize(&caller, CallerPolicy::Main, "about_info") {
        return None;
    }
    Some(AboutInfo {
        version: app.package_info().version.to_string(),
        os: os_name(),
        arch: arch_name(std::env::consts::ARCH).to_owned(),
    })
}

/// The names people see in their system's own About panels.
pub(crate) fn arch_name(arch: &str) -> &str {
    match arch {
        "aarch64" => "arm64",
        "x86_64" => "x64",
        other => other,
    }
}

#[cfg(target_os = "macos")]
pub(crate) fn os_name() -> String {
    let version = objc2_foundation::NSProcessInfo::processInfo().operatingSystemVersion();
    format!(
        "macOS {}.{}.{}",
        version.majorVersion, version.minorVersion, version.patchVersion
    )
}

#[cfg(target_os = "windows")]
pub(crate) fn os_name() -> String {
    use crate::default_browser::windows::read_machine_string;
    const KEY: &str = r"SOFTWARE\Microsoft\Windows NT\CurrentVersion";
    let build = read_machine_string(KEY, "CurrentBuildNumber");
    // Windows 11 still reports itself as 10 in ProductName; the build decides.
    let product = match build.as_deref().and_then(|value| value.parse::<u32>().ok()) {
        Some(number) if number >= 22_000 => "Windows 11",
        _ => "Windows 10",
    };
    match (read_machine_string(KEY, "DisplayVersion"), build) {
        (Some(release), Some(build)) => format!("{product} {release} (build {build})"),
        (None, Some(build)) => format!("{product} (build {build})"),
        _ => product.to_owned(),
    }
}

#[cfg(target_os = "linux")]
pub(crate) fn os_name() -> String {
    std::fs::read_to_string("/etc/os-release")
        .ok()
        .and_then(|release| {
            release.lines().find_map(|line| {
                line.strip_prefix("PRETTY_NAME=")
                    .map(|value| value.trim_matches('"').to_owned())
            })
        })
        .filter(|name| !name.is_empty() && name.len() <= 128)
        .unwrap_or_else(|| "Linux".to_owned())
}

#[cfg(test)]
mod tests {
    #[test]
    fn processors_use_the_names_people_know() {
        assert_eq!(super::arch_name("aarch64"), "arm64");
        assert_eq!(super::arch_name("x86_64"), "x64");
        assert_eq!(super::arch_name("riscv64"), "riscv64");
    }
}
