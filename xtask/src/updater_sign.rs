//! Release-only Windows updater signing. Secrets are accepted through the
//! existing Tauri environment contract and never put on a command line.

use base64::Engine as _;
use minisign::{PublicKeyBox, SecretKeyBox};
use semver::Version;
use std::fs::File;
use std::io::Write;
use std::path::Path;

const TARGET: &str = "windows-x86_64-nsis";

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
