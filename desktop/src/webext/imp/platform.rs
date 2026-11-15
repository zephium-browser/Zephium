use std::path::Path;
use zephium_webext::prepare;

/// Added to every package: `nativeMessaging` for the browser's own bridges,
/// and `activeTab`, which grants nothing until the user clicks the
/// extension, so "on click" site access works for every extension, as it
/// does in Chrome.
#[cfg(target_os = "macos")]
const ADDED_PERMISSIONS: &[&str] = &["nativeMessaging", "activeTab"];

/// Extensions' own console errors are for whoever is building the browser;
/// in a release they would cost a message to the browser each and help no one.
#[cfg(target_os = "macos")]
const DIAGNOSTICS: bool = cfg!(any(debug_assertions, feature = "webext-qa"));

#[cfg(target_os = "macos")]
pub(super) fn compat_revision(_access: &super::Access) -> String {
    // FNV-1a over the layer and the Chrome identity it presents; changing
    // either rebuilds every package from its original.
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in zephium_webext_macos::compat::SCRIPT
        .bytes()
        .chain(zephium_webext::store::CHROME_VERSION.bytes())
        .chain(ADDED_PERMISSIONS.concat().bytes())
        .chain([u8::from(DIAGNOSTICS)])
    {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{hash:016x}")[..12].to_owned()
}

#[cfg(target_os = "macos")]
fn compat_layer() -> prepare::CompatLayer {
    prepare::CompatLayer::new(zephium_webext_macos::compat::SCRIPT)
        .with_permissions(ADDED_PERMISSIONS)
        .with_diagnostics(DIAGNOSTICS)
}

#[cfg(target_os = "windows")]
pub(super) fn compat_revision(access: &super::Access) -> String {
    zephium_webext::windows::access_revision(match access {
        super::Access::All => None,
        super::Access::Sites { sites } => Some(sites),
        super::Access::Click => Some(&[]),
    })
}

/// Linux injects its runtime at run time, so a package's revision is the
/// runtime bundle it was last started with.
#[cfg(target_os = "linux")]
pub(super) fn compat_revision(_access: &super::Access) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in zephium_webext_runtime::js::BUNDLE
        .bytes()
        .chain(zephium_webext_runtime::js::surface().bytes())
        .chain(zephium_webext::store::CHROME_VERSION.bytes())
    {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{hash:016x}")[..12].to_owned()
}

pub(super) fn prepare_package(
    dir: &Path,
    access: &super::Access,
) -> Result<prepare::PrepareReport, String> {
    // Packages stay byte-for-byte as published; nothing is written into them.
    #[cfg(target_os = "linux")]
    {
        let _ = (dir, access);
        Ok(prepare::PrepareReport::default())
    }
    #[cfg(target_os = "macos")]
    {
        let _ = access;
        prepare::prepare(dir, &compat_layer()).map_err(|error| error.to_string())
    }
    #[cfg(target_os = "windows")]
    {
        let sites = match access {
            super::Access::All => None,
            super::Access::Sites { sites } => Some(sites.as_slice()),
            super::Access::Click => {
                return Err("On click site access is not available on Windows yet.".into())
            }
        };
        zephium_webext::windows::prepare(dir, sites)?;
        Ok(prepare::PrepareReport::default())
    }
}

pub(super) fn record_signed_key(dir: &Path, key: &[u8]) -> Result<(), String> {
    use base64::Engine as _;
    let path = dir.join("manifest.json");
    let bytes = std::fs::read(&path).map_err(|e| e.to_string())?;
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    manifest["key"] = base64::engine::general_purpose::STANDARD.encode(key).into();
    std::fs::write(
        path,
        serde_json::to_vec_pretty(&manifest).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())
}
