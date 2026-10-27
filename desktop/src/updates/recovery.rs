//! Retain an interrupted Windows update across terminal shutdown. The envelope
//! is never executable authority: its payload is reverified with the embedded
//! application key before it can become Ready or reach the installer.
use std::fs::OpenOptions;
use std::io::{self, Read, Write};
use std::path::Path;

const MAX_METADATA: usize = 256 * 1024;
const MAX_ARTIFACT: u64 = 128 * 1024 * 1024;

pub(super) fn save(path: &Path, metadata: &serde_json::Value, bytes: &[u8]) -> io::Result<()> {
    let metadata = serde_json::to_vec(metadata)?;
    if metadata.len() > MAX_METADATA || bytes.is_empty() || bytes.len() as u64 > MAX_ARTIFACT {
        return Err(io::Error::other("invalid retained update size"));
    }
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("missing update directory"))?;
    let mut file = tempfile::NamedTempFile::new_in(parent)?;
    file.write_all(&(metadata.len() as u32).to_le_bytes())?;
    file.write_all(&metadata)?;
    file.write_all(bytes)?;
    file.as_file().sync_all()?;
    file.persist(path).map_err(|error| error.error)?;
    Ok(())
}

pub(super) fn load_matching(
    path: &Path,
    trusted_release: &serde_json::Value,
) -> io::Result<Option<Vec<u8>>> {
    Ok(load(path)?.and_then(|(metadata, bytes)| (metadata == *trusted_release).then_some(bytes)))
}

/// The version the retained update would install, read only to recognise an
/// update that already finished; it is never authority to install.
pub(super) fn retained_version(path: &Path) -> Option<String> {
    let (metadata, _) = open(path).ok()??;
    let version = metadata.get("version")?.as_str()?;
    Some(version.trim_start_matches('v').to_owned())
}

fn load(path: &Path) -> io::Result<Option<(serde_json::Value, Vec<u8>)>> {
    let Some((metadata, file)) = open(path)? else {
        return Ok(None);
    };
    let mut bytes = Vec::new();
    file.take(MAX_ARTIFACT + 1).read_to_end(&mut bytes)?;
    if bytes.is_empty() || bytes.len() as u64 > MAX_ARTIFACT {
        return Err(io::Error::other("invalid retained update payload size"));
    }
    Ok(Some((metadata, bytes)))
}

/// Opens a checked envelope, leaving its payload unread for version-only checks.
fn open(path: &Path) -> io::Result<Option<(serde_json::Value, std::fs::File)>> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    if !metadata.is_file() || metadata.len() > MAX_ARTIFACT + MAX_METADATA as u64 + 4 {
        return Err(io::Error::other("invalid retained update file"));
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        // Open the reparse point itself, never follow a substituted junction.
        options.custom_flags(0x0020_0000); // FILE_FLAG_OPEN_REPARSE_POINT
    }
    let mut file = options.open(path)?;
    let opened = file.metadata()?;
    if !opened.is_file() {
        return Err(io::Error::other("retained update is not a regular file"));
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if opened.file_attributes() & 0x400 != 0 {
            // FILE_ATTRIBUTE_REPARSE_POINT
            return Err(io::Error::other("retained update is a reparse point"));
        }
    }
    let metadata = read_metadata(&mut file, opened.len())?;
    Ok(Some((metadata, file)))
}

fn read_metadata(reader: &mut impl Read, file_len: u64) -> io::Result<serde_json::Value> {
    let mut length = [0; 4];
    reader.read_exact(&mut length)?;
    let length = u32::from_le_bytes(length) as usize;
    if length == 0 || length > MAX_METADATA {
        return Err(io::Error::other("invalid retained update metadata size"));
    }
    if !file_len
        .checked_sub(4 + length as u64)
        .is_some_and(|payload| (1..=MAX_ARTIFACT).contains(&payload))
    {
        return Err(io::Error::other("invalid retained update payload size"));
    }
    let mut metadata = vec![0; length];
    reader.read_exact(&mut metadata)?;
    let metadata = serde_json::from_slice(&metadata)?;
    Ok(metadata)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn interrupted_update_survives_reopen_and_atomic_replacement() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("pending-update");
        assert!(load(&path).unwrap().is_none());
        let metadata = serde_json::json!({"version":"1.2.3"});
        save(&path, &metadata, b"signed fixture").unwrap();
        assert_eq!(
            load(&path).unwrap(),
            Some((metadata.clone(), b"signed fixture".to_vec()))
        );
        save(&path, &metadata, b"replacement").unwrap();
        assert_eq!(load(&path).unwrap().unwrap().1, b"replacement");
    }
    #[test]
    fn a_finished_update_is_recognised_by_its_version() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("pending-update");
        assert_eq!(retained_version(&path), None);
        save(&path, &serde_json::json!({"version":"v1.2.3"}), b"signed").unwrap();
        assert_eq!(retained_version(&path).as_deref(), Some("1.2.3"));
    }

    #[test]
    fn metadata_checks_stop_before_the_installer_payload() {
        let metadata = serde_json::json!({"version":"v1.2.3"});
        let json = serde_json::to_vec(&metadata).unwrap();
        let mut header = (json.len() as u32).to_le_bytes().to_vec();
        header.extend_from_slice(&json);
        let mut reader = io::Cursor::new(&header);
        // Only the header is readable: touching the payload would reach EOF.
        assert_eq!(
            read_metadata(&mut reader, header.len() as u64 + MAX_ARTIFACT).unwrap(),
            metadata
        );
        assert_eq!(reader.position(), header.len() as u64);
    }

    #[test]
    fn version_checks_reject_an_empty_or_oversized_installer_payload() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("pending-update");
        save(&path, &serde_json::json!({"version":"v1.2.3"}), b"x").unwrap();
        let file = OpenOptions::new().write(true).open(&path).unwrap();
        let header_len = file.metadata().unwrap().len() - 1;
        for payload_len in [0, MAX_ARTIFACT + 1] {
            file.set_len(header_len + payload_len).unwrap();
            assert!(retained_version(&path).is_none());
            assert!(load(&path).is_err());
        }
    }

    #[test]
    fn retained_version_and_signature_cannot_override_fresh_release_metadata() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("pending-update");
        let trusted = serde_json::json!({"version":"1.2.3", "signature":"current"});
        save(&path, &trusted, b"signed fixture").unwrap();
        assert_eq!(
            load_matching(&path, &trusted).unwrap().unwrap(),
            b"signed fixture"
        );
        for metadata in [
            serde_json::json!({"version":"9.9.9", "signature":"current"}),
            serde_json::json!({"version":"1.2.3", "signature":"old"}),
        ] {
            save(&path, &metadata, b"signed fixture").unwrap();
            assert!(load_matching(&path, &trusted).unwrap().is_none());
        }
    }

    #[cfg(unix)]
    #[test]
    fn recovery_refuses_symbolic_links() {
        let root = tempfile::tempdir().unwrap();
        let original = root.path().join("original");
        let link = root.path().join("pending-update");
        save(&original, &serde_json::json!({}), b"fixture").unwrap();
        std::os::unix::fs::symlink(&original, &link).unwrap();
        assert!(load(&link).is_err());
    }

    #[test]
    fn malformed_or_truncated_envelopes_are_not_ready_updates() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("pending-update");
        for bytes in [
            &b""[..],
            &u32::MAX.to_le_bytes(),
            &b"\x02\0\0\0{}"[..],
            &b"\x04\0\0\0{}"[..],
        ] {
            std::fs::write(&path, bytes).unwrap();
            assert!(load(&path).is_err());
            assert!(retained_version(&path).is_none());
        }
    }
}
