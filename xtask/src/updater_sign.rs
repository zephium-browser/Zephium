//! Release asset verification and Windows updater signing. Secrets are accepted
//! through the existing Tauri environment contract, never on a command line.

use base64::Engine as _;
use minisign::{PublicKeyBox, SecretKeyBox};
use semver::Version;
use std::fs::File;
use std::io::Write;
use std::path::Path;

const TARGET: &str = "windows-x86_64-nsis";

pub(crate) fn check_release_assets(arguments: &[String]) -> Result<(), &'static str> {
    let [directory, tag, repository] = arguments else {
        return Err("usage: cargo xtask check-release-assets <directory> <tag> <owner/repo>");
    };
    let config: serde_json::Value = serde_json::from_slice(
        &std::fs::read(Path::new(env!("CARGO_MANIFEST_DIR")).join("../desktop/tauri.conf.json"))
            .map_err(|_| "cannot read the embedded updater public key")?,
    )
    .map_err(|_| "invalid application configuration")?;
    let public = config
        .pointer("/plugins/updater/pubkey")
        .and_then(|key| key.as_str())
        .ok_or("the embedded updater public key is missing")?;
    check_assets(Path::new(directory), tag, repository, public)
}

fn check_assets(
    directory: &Path,
    tag: &str,
    repository: &str,
    public: &str,
) -> Result<(), &'static str> {
    const MAC: &str = "Zephium-macOS-arm64.app.tar.gz";
    const WINDOWS: &str = "Zephium-Windows-x64-setup.exe";
    let expected = std::collections::BTreeSet::from([
        "SHA256SUMS".to_owned(),
        "latest.json".to_owned(),
        "Zephium-macOS-arm64.dmg".to_owned(),
        MAC.to_owned(),
        format!("{MAC}.sig"),
        WINDOWS.to_owned(),
        format!("{WINDOWS}.sig"),
    ]);
    let mut actual = std::collections::BTreeSet::new();
    for entry in std::fs::read_dir(directory).map_err(|_| "cannot read release assets")? {
        let entry = entry.map_err(|_| "cannot read release asset")?;
        if !entry.file_type().is_ok_and(|kind| kind.is_file()) {
            return Err("release assets must be regular files");
        }
        actual.insert(
            entry
                .file_name()
                .into_string()
                .map_err(|_| "invalid asset name")?,
        );
    }
    if actual != expected {
        return Err("incomplete or unexpected release asset set");
    }
    let manifest: serde_json::Value = serde_json::from_slice(
        &std::fs::read(directory.join("latest.json")).map_err(|_| "cannot read latest.json")?,
    )
    .map_err(|_| "invalid latest.json")?;
    let version = tag.strip_prefix('v').ok_or("invalid release tag")?;
    if manifest.get("version").and_then(|value| value.as_str()) != Some(version) {
        return Err("latest.json version does not match the tag");
    }
    let platforms = manifest
        .get("platforms")
        .and_then(|value| value.as_object())
        .ok_or("latest.json platforms are missing")?;
    let base = format!("https://github.com/{repository}/releases/download/{tag}/");
    for (platform, asset) in [
        ("darwin-aarch64", MAC),
        ("darwin-aarch64-app", MAC),
        ("windows-x86_64", WINDOWS),
        ("windows-x86_64-nsis", WINDOWS),
    ] {
        let entry = platforms
            .get(platform)
            .ok_or("required updater platform is missing")?;
        if entry.get("url").and_then(|url| url.as_str()) != Some(format!("{base}{asset}").as_str())
        {
            return Err("updater URL does not match this tag's platform asset");
        }
        let detached = std::fs::read_to_string(directory.join(format!("{asset}.sig")))
            .map_err(|_| "cannot read detached updater signature")?;
        if entry
            .get("signature")
            .and_then(|value| value.as_str())
            .map(str::trim)
            != Some(detached.trim())
        {
            return Err("manifest and detached updater signatures differ");
        }
    }
    let decode = |value: &str| {
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(value.trim())
            .map_err(|_| "invalid updater signature or key encoding")?;
        String::from_utf8(bytes).map_err(|_| "invalid updater signature or key encoding")
    };
    let public = PublicKeyBox::from_string(&decode(public)?)
        .and_then(|key| key.into_public_key())
        .map_err(|_| "invalid updater public key")?;
    for entry in platforms.values() {
        let asset = entry
            .get("url")
            .and_then(|url| url.as_str())
            .and_then(|url| url.strip_prefix(&base))
            .filter(|asset| [MAC, WINDOWS].contains(asset))
            .ok_or("updater URL is not an updater asset of this tag")?;
        let signature = entry
            .get("signature")
            .and_then(|value| value.as_str())
            .ok_or("updater signature is missing")?;
        let signature = minisign::SignatureBox::from_string(&decode(signature)?)
            .map_err(|_| "invalid updater signature")?;
        minisign::verify(
            &public,
            &signature,
            File::open(directory.join(asset)).map_err(|_| "cannot open updater asset")?,
            true,
            false,
            false,
        )
        .map_err(|_| "updater artifact signature verification failed")?;
    }
    Ok(())
}

pub(crate) fn run(arguments: &[String]) -> Result<(), &'static str> {
    let [installer, version] = arguments else {
        return Err("usage: cargo xtask sign-windows-updater <installer.exe> <version>");
    };
    let installer = Path::new(installer);
    if installer
        .extension()
        .and_then(|extension| extension.to_str())
        != Some("exe")
    {
        return Err("the Windows updater artifact must be an NSIS .exe");
    }
    let config: serde_json::Value = serde_json::from_slice(
        &std::fs::read(Path::new(env!("CARGO_MANIFEST_DIR")).join("../desktop/tauri.conf.json"))
            .map_err(|_| "cannot read the embedded updater public key")?,
    )
    .map_err(|_| "invalid application configuration")?;
    let public = config
        .pointer("/plugins/updater/pubkey")
        .and_then(|key| key.as_str())
        .ok_or("the embedded updater public key is missing")?;
    let secret = std::env::var("TAURI_SIGNING_PRIVATE_KEY")
        .map_err(|_| "TAURI_SIGNING_PRIVATE_KEY is required")?;
    let password = std::env::var("TAURI_SIGNING_PRIVATE_KEY_PASSWORD").unwrap_or_default();
    let signature = sign(installer, version, &secret, password, public)?;
    let output = installer.with_extension("exe.sig");
    let mut temporary =
        tempfile::NamedTempFile::new_in(installer.parent().ok_or("invalid artifact path")?)
            .map_err(|_| "cannot stage updater signature")?;
    temporary
        .write_all(signature.as_bytes())
        .map_err(|_| "cannot write updater signature")?;
    temporary
        .as_file()
        .sync_all()
        .map_err(|_| "cannot sync updater signature")?;
    temporary
        .persist(output)
        .map_err(|_| "cannot publish updater signature")?;
    Ok(())
}

fn sign(
    installer: &Path,
    version: &str,
    secret: &str,
    password: String,
    public: &str,
) -> Result<String, &'static str> {
    let parsed = Version::parse(version).map_err(|_| "invalid release version")?;
    if parsed.to_string() != version {
        return Err("the release version must be canonical semver");
    }
    let decode = |value: &str| {
        if value.len() > 8192 {
            return Err("invalid signing key encoding");
        }
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(value)
            .map_err(|_| "invalid signing key encoding")?;
        String::from_utf8(bytes).map_err(|_| "invalid signing key encoding")
    };
    let public = PublicKeyBox::from_string(&decode(public)?)
        .and_then(|key| key.into_public_key())
        .map_err(|_| "invalid updater public key")?;
    let secret = SecretKeyBox::from_string(&decode(secret)?)
        .and_then(|key| {
            key.clone()
                .into_unencrypted_secret_key()
                .or_else(|_| key.into_secret_key(Some(password)))
        })
        .map_err(|_| "cannot unlock updater signing key")?;
    let comment = format!("zephium-updater-v1 version={version} target={TARGET}");
    let signature = minisign::sign(
        Some(&public),
        &secret,
        File::open(installer).map_err(|_| "cannot open updater artifact")?,
        Some(&comment),
        None,
    )
    .map_err(|_| "cannot sign updater artifact")?;
    minisign::verify(
        &public,
        &signature,
        File::open(installer).map_err(|_| "cannot reopen updater artifact")?,
        true,
        false,
        false,
    )
    .map_err(|_| "updater artifact signature verification failed")?;
    Ok(base64::engine::general_purpose::STANDARD.encode(signature.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn release_manifest_requires_complete_assets_and_verified_platforms() {
        let directory = tempfile::tempdir().unwrap();
        let directory = directory.path();
        let pair = minisign::KeyPair::generate_unencrypted_keypair().unwrap();
        let encode = |value: String| base64::engine::general_purpose::STANDARD.encode(value);
        let secret = encode(pair.sk.to_box(None).unwrap().to_string());
        let public = encode(pair.pk.to_box().unwrap().to_string());
        let mut platforms = serde_json::Map::new();
        for (asset, keys) in [
            (
                "Zephium-macOS-arm64.app.tar.gz",
                ["darwin-aarch64", "darwin-aarch64-app"],
            ),
            (
                "Zephium-Windows-x64-setup.exe",
                ["windows-x86_64", "windows-x86_64-nsis"],
            ),
        ] {
            let path = directory.join(asset);
            std::fs::write(&path, format!("harmless fixture: {asset}")).unwrap();
            let signature = sign(&path, "1.2.4", &secret, String::new(), &public).unwrap();
            std::fs::write(directory.join(format!("{asset}.sig")), &signature).unwrap();
            for key in keys {
                platforms.insert(key.into(), serde_json::json!({
                    "url": format!("https://github.com/example/repo/releases/download/v1.2.4/{asset}"),
                    "signature": signature,
                }));
            }
        }
        let good = serde_json::json!({ "version": "1.2.4", "platforms": platforms });
        let check = |manifest: &serde_json::Value| {
            std::fs::write(directory.join("latest.json"), manifest.to_string()).unwrap();
            check_assets(directory, "v1.2.4", "example/repo", &public)
        };
        std::fs::write(directory.join("SHA256SUMS"), "fixture checksums").unwrap();
        std::fs::write(directory.join("Zephium-macOS-arm64.dmg"), "fixture DMG").unwrap();
        assert_eq!(check(&good), Ok(()));
        for key in platforms.keys() {
            let mut bad = good.clone();
            bad["platforms"].as_object_mut().unwrap().remove(key);
            assert!(check(&bad).is_err(), "missing platform {key}");
            for url in [
                "https://github.com/example/repo/releases/download/v1.2.3/Zephium-Windows-x64-setup.exe",
                "https://example.com/Zephium-Windows-x64-setup.exe",
                "https://github.com/example/repo/releases/download/v1.2.4/../Zephium-Windows-x64-setup.exe",
            ] {
                let mut bad = good.clone();
                bad["platforms"][key]["url"] = url.into();
                assert!(check(&bad).is_err(), "wrong URL for {key}");
            }
            let mut bad = good.clone();
            bad["platforms"][key]["signature"] = "invalid signature".into();
            assert!(check(&bad).is_err(), "bad signature for {key}");
        }
        let mut bad = good.clone();
        bad["version"] = "1.2.3".into();
        assert!(check(&bad).is_err(), "wrong version");
        assert_eq!(check(&good), Ok(()));
        for asset in std::fs::read_dir(directory).unwrap() {
            let asset = asset.unwrap().path();
            let bytes = std::fs::read(&asset).unwrap();
            std::fs::remove_file(&asset).unwrap();
            assert!(
                check_assets(directory, "v1.2.4", "example/repo", &public).is_err(),
                "missing {asset:?}"
            );
            std::fs::write(&asset, bytes).unwrap();
        }
        // Keep a well-formed signature but change the downloaded bytes.
        std::fs::write(directory.join("Zephium-Windows-x64-setup.exe"), "tampered").unwrap();
        assert!(
            check(&good).is_err(),
            "signature must verify the downloaded bytes"
        );
    }

    #[test]
    fn signing_helper_binds_exact_version_and_platform_and_verifies_the_key() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("installer.exe");
        std::fs::write(&path, b"harmless installer fixture").unwrap();
        let pair = minisign::KeyPair::generate_unencrypted_keypair().unwrap();
        let encode = |value: String| base64::engine::general_purpose::STANDARD.encode(value);
        let secret = encode(pair.sk.to_box(None).unwrap().to_string());
        let public = encode(pair.pk.to_box().unwrap().to_string());
        let signature = sign(&path, "1.2.4", &secret, String::new(), &public).unwrap();
        let decoded = String::from_utf8(
            base64::engine::general_purpose::STANDARD
                .decode(signature)
                .unwrap(),
        )
        .unwrap();
        let parsed = minisign::SignatureBox::from_string(&decoded).unwrap();
        assert_eq!(
            parsed.trusted_comment().unwrap(),
            "zephium-updater-v1 version=1.2.4 target=windows-x86_64-nsis"
        );
        let other = minisign::KeyPair::generate_unencrypted_keypair().unwrap();
        assert!(sign(
            &path,
            "1.2.4",
            &secret,
            String::new(),
            &encode(other.pk.to_box().unwrap().to_string())
        )
        .is_err());
        assert!(sign(&path, "v1.2.4", &secret, String::new(), &public).is_err());
        let encrypted =
            minisign::KeyPair::generate_encrypted_keypair(Some("fixture password".into())).unwrap();
        let secret = encode(encrypted.sk.to_box(None).unwrap().to_string());
        let public = encode(encrypted.pk.to_box().unwrap().to_string());
        assert!(sign(&path, "1.2.4", &secret, "fixture password".into(), &public).is_ok());
        assert!(sign(
            &path,
            "1.2.4",
            &secret,
            "wrong fixture password".into(),
            &public
        )
        .is_err());
    }
}
