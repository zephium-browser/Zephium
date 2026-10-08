//! Bounded updater transport and an anonymous file owned for the pending update's
//! lifetime. Installation always verifies the exact bytes it consumes.
use std::fs::File;
use std::io::{Read, Seek};
use std::time::Duration;

use base64::Engine as _;
use minisign_verify::{PublicKey, Signature};
use tokio::io::AsyncWriteExt;

const MAX_BYTES: u64 = 128 * 1024 * 1024;
const MAX_SIGNATURE_BYTES: usize = 4096;

pub(super) struct Artifact {
    file: File,
    key: PublicKey,
    signature: Signature,
}

impl Artifact {
    fn new(file: File, key: &str, signature: &str) -> Result<Self, String> {
        let mut artifact = Self {
            file,
            key: PublicKey::decode(&decode(key)?).map_err(|_| "invalid update verification key")?,
            signature: Signature::decode(&decode(signature)?)
                .map_err(|_| "invalid update signature")?,
        };
        artifact.verified_bytes()?;
        Ok(artifact)
    }

    #[cfg(any(target_os = "windows", test))]
    pub(super) fn from_bytes(bytes: &[u8], key: &str, signature: &str) -> Result<Self, String> {
        use std::io::Write;
        if bytes.is_empty() || bytes.len() as u64 > MAX_BYTES {
            return Err("update file has an invalid size".into());
        }
        let key =
            PublicKey::decode(&decode(key)?).map_err(|_| "invalid update verification key")?;
        let signature =
            Signature::decode(&decode(signature)?).map_err(|_| "invalid update signature")?;
        key.verify(bytes, &signature, true)
            .map_err(|_| "update signature verification failed")?;
        let mut file = tempfile::tempfile().map_err(|_| "update staging is unavailable")?;
        file.write_all(bytes)
            .map_err(|_| "update staging write failed")?;
        Ok(Self {
            file,
            key,
            signature,
        })
    }

    pub(super) fn verified_bytes(&mut self) -> Result<Vec<u8>, String> {
        let size = self
            .file
            .metadata()
            .map_err(|_| "update file is unavailable")?
            .len();
        if size == 0 || size > MAX_BYTES {
            return Err("update file has an invalid size".into());
        }
        self.file
            .rewind()
            .map_err(|_| "update file cannot be read")?;
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(size as usize)
            .map_err(|_| "not enough memory to install update")?;
        (&mut self.file)
            .take(MAX_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| "update file cannot be read")?;
        if bytes.len() as u64 > MAX_BYTES {
            return Err("update file exceeds the size limit".into());
        }
        self.key
            .verify(&bytes, &self.signature, true)
            .map_err(|_| "update signature verification failed")?;
        Ok(bytes)
    }
}

fn decode(value: &str) -> Result<String, String> {
    if value.len() > MAX_SIGNATURE_BYTES {
        return Err("update signature metadata exceeds the size limit".into());
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(value)
        .map_err(|_| "invalid update signature encoding")?;
    String::from_utf8(bytes).map_err(|_| "invalid update signature text".into())
}

pub(super) async fn download(
    url: &reqwest::Url,
    key: String,
    signature: String,
) -> Result<Artifact, String> {
    // GitHub assets redirect to its CDN. HTTPS-only also applies to redirects;
    // the embedded key, not the CDN hostname, authenticates the payload.
    let client = reqwest::Client::builder()
        .https_only(true)
        .connect_timeout(Duration::from_secs(15))
        .read_timeout(Duration::from_secs(60))
        .timeout(Duration::from_secs(15 * 60))
        .redirect(reqwest::redirect::Policy::limited(5))
        .build()
        .map_err(|_| "update transport is unavailable")?;
    let mut response = client
        .get(url.clone())
        .send()
        .await
        .map_err(|_| "update download failed")?
        .error_for_status()
        .map_err(|_| "update server refused the download")?;
    if response
        .content_length()
        .is_some_and(|size| size > MAX_BYTES)
    {
        return Err("update download exceeds the size limit".into());
    }
    let file = tokio::task::spawn_blocking(tempfile::tempfile)
        .await
        .map_err(|_| "update staging failed")?
        .map_err(|_| "update staging is unavailable")?;
    let mut file = tokio::fs::File::from_std(file);
    let mut received = 0_u64;
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| "update download was interrupted")?
    {
        received = admitted_size(received, chunk.len())?;
        file.write_all(&chunk)
            .await
            .map_err(|_| "update staging write failed")?;
    }
    file.flush()
        .await
        .map_err(|_| "update staging flush failed")?;
    let file = file.into_std().await;
    tokio::task::spawn_blocking(move || Artifact::new(file, &key, &signature))
        .await
        .map_err(|_| "update verification did not complete")?
}

fn admitted_size(current: u64, chunk: usize) -> Result<u64, String> {
    current
        .checked_add(chunk as u64)
        .filter(|size| *size <= MAX_BYTES)
        .ok_or_else(|| "update download exceeds the size limit".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{SeekFrom, Write};

    // Public minisign-verify fixture; no production signing key is needed.
    const KEY: &str =
        "untrusted comment: fixture\nRWQf6LRCGA9i53mlYecO4IzT51TGPpvWucNSCh1CBM0QTaLn73Y7GFO3";
    const SIGNATURE: &str = "untrusted comment: signature from minisign secret key\nRUQf6LRCGA9i559r3g7V1qNyJDApGip8MfqcadIgT9CuhV3EMhHoN1mGTkUidF/z7SrlQgXdy8ofjb7bNJJylDOocrCo8KLzZwo=\ntrusted comment: timestamp:1633700835\tfile:test\tprehashed\nwLMDjy9FLAuxZ3q4NlEvkgtyhrr0gtTu6KC4KBJdITbbOeAi1zBIYo0v4iTgt8jJpIidRJnp94ABQkJAgAooBQ==";

    fn fixture() -> Artifact {
        let mut file = tempfile::tempfile().unwrap();
        file.write_all(b"test").unwrap();
        let encode = |value: &str| base64::engine::general_purpose::STANDARD.encode(value);
        Artifact::new(file, &encode(KEY), &encode(SIGNATURE)).unwrap()
    }

    #[test]
    fn recovered_payload_must_still_match_the_embedded_key_and_signature() {
        let encode = |value: &str| base64::engine::general_purpose::STANDARD.encode(value);
        assert!(Artifact::from_bytes(b"test", &encode(KEY), &encode(SIGNATURE)).is_ok());
        assert!(Artifact::from_bytes(b"evil", &encode(KEY), &encode(SIGNATURE)).is_err());
    }

    #[test]
    fn installation_rejects_changed_previously_verified_bytes() {
        let mut artifact = fixture();
        assert_eq!(artifact.verified_bytes().unwrap(), b"test");
        artifact.file.seek(SeekFrom::Start(0)).unwrap();
        artifact.file.write_all(b"evil").unwrap();
        assert!(artifact.verified_bytes().is_err());
    }

    #[test]
    fn truncated_and_oversized_files_never_reach_installer() {
        let mut artifact = fixture();
        artifact.file.set_len(3).unwrap();
        assert!(artifact.verified_bytes().is_err());
        artifact.file.set_len(MAX_BYTES + 1).unwrap();
        assert!(artifact.verified_bytes().is_err());
    }

    #[test]
    fn streamed_limit_does_not_trust_content_length() {
        assert_eq!(admitted_size(MAX_BYTES - 1, 1).unwrap(), MAX_BYTES);
        assert!(admitted_size(MAX_BYTES, 1).is_err());
        assert!(admitted_size(u64::MAX, 1).is_err());
        assert!(decode(&"A".repeat(MAX_SIGNATURE_BYTES + 1)).is_err());
    }
}
