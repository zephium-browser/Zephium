//! Profile-scoped, content-addressed media blobs. Bytes are admitted by
//! bounded sniffing and decoding before they are written; the resource row
//! that describes them is minted in the same call.
use super::*;
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use zephium_core::resources::*;

const MAX_DECODE_ALLOC_BYTES: u64 = 256 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MediaAdmissionError {
    Empty,
    TooLarge,
    Unsupported,
    Malformed,
}

/// Bounded sniff and decode. Returns the asset description Rust will persist;
/// the caller stores the same bytes under `digest`.
pub fn admit(
    bytes: &[u8],
    name: &str,
    origin: MediaOrigin,
) -> Result<MediaAssetV1, MediaAdmissionError> {
    if bytes.is_empty() {
        return Err(MediaAdmissionError::Empty);
    }
    let size = u32::try_from(bytes.len()).map_err(|_| MediaAdmissionError::TooLarge)?;
    if size > MAX_MEDIA_FILE_BYTES {
        return Err(MediaAdmissionError::TooLarge);
    }
    let name = clean_name(name);
    let digest = hex(&Sha256::digest(bytes));
    let (kind, mime, width, height) = if let Ok(format) = image::guess_format(bytes) {
        let mime = match format {
            image::ImageFormat::Png => "image/png",
            image::ImageFormat::Jpeg => "image/jpeg",
            image::ImageFormat::WebP => "image/webp",
            image::ImageFormat::Gif => "image/gif",
            _ => return Err(MediaAdmissionError::Unsupported),
        };
        let limit = match origin {
            MediaOrigin::Fetched { .. } => MAX_MEDIA_FETCHED_IMAGE_BYTES,
            MediaOrigin::Imported => MAX_MEDIA_IMAGE_BYTES,
        };
        if size > limit {
            return Err(MediaAdmissionError::TooLarge);
        }
        let mut limits = image::Limits::default();
        limits.max_image_width = Some(MAX_MEDIA_DIMENSION);
        limits.max_image_height = Some(MAX_MEDIA_DIMENSION);
        limits.max_alloc = Some(MAX_DECODE_ALLOC_BYTES);
        let mut reader = image::ImageReader::with_format(std::io::Cursor::new(bytes), format);
        reader.limits(limits);
        let decoded = reader
            .decode()
            .map_err(|_| MediaAdmissionError::Malformed)?;
        (
            MediaKind::Image,
            mime,
            Some(decoded.width()),
            Some(decoded.height()),
        )
    } else if bytes.starts_with(b"%PDF-") {
        if matches!(origin, MediaOrigin::Fetched { .. }) {
            return Err(MediaAdmissionError::Unsupported);
        }
        (MediaKind::Pdf, "application/pdf", None, None)
    } else {
        if matches!(origin, MediaOrigin::Fetched { .. }) {
            return Err(MediaAdmissionError::Unsupported);
        }
        (MediaKind::File, "application/octet-stream", None, None)
    };
    let asset = MediaAssetV1 {
        version: 1,
        kind,
        mime: mime.into(),
        bytes: size,
        digest,
        name,
        origin,
        width,
        height,
    };
    if !asset.validate() {
        return Err(MediaAdmissionError::Malformed);
    }
    Ok(asset)
}

fn clean_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .filter(|c| !c.is_control() && *c != '/' && *c != '\\')
        .collect();
    let cleaned = cleaned.trim();
    if cleaned.is_empty() {
        return "file".into();
    }
    let mut end = cleaned.len().min(MAX_MEDIA_NAME_BYTES);
    while !cleaned.is_char_boundary(end) {
        end -= 1;
    }
    cleaned[..end].to_owned()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub(crate) fn valid_digest(digest: &str) -> bool {
    digest.len() == 64 && digest.bytes().all(|b| b.is_ascii_hexdigit())
}

/// On-disk layout: `<root>/<profile>/<digest>`; profile directories are 0700.
#[derive(Clone, Debug)]
pub struct MediaStore {
    root: PathBuf,
    scans: std::sync::Arc<std::sync::Mutex<std::collections::HashMap<ProfileId, OrphanScan>>>,
}

#[derive(Debug, Default)]
struct OrphanScan {
    entries: Option<std::fs::ReadDir>,
    complete: bool,
    retry: bool,
}

fn referenced_digests(conn: &Connection) -> rusqlite::Result<std::collections::HashSet<String>> {
    let mut statement = conn.prepare(
        "SELECT json_extract(body,'$.content.asset.digest') FROM user_resources WHERE kind='media'",
    )?;
    let references = statement
        .query_map([], |row| row.get::<_, String>(0))?
        .take(MAX_RESOURCES + 1)
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if references.len() > MAX_RESOURCES || references.iter().any(|digest| !valid_digest(digest)) {
        return Err(rusqlite::Error::InvalidQuery);
    }
    Ok(references
        .into_iter()
        .map(|digest| digest.to_ascii_lowercase())
        .collect())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SyncPoint {
    File,
    Directory,
}

fn sync_path(point: SyncPoint, path: &std::path::Path) -> std::io::Result<()> {
    match point {
        SyncPoint::File => flushable_file(path)?.sync_all(),
        SyncPoint::Directory => {
            #[cfg(unix)]
            std::fs::File::open(path)?.sync_all()?;
            #[cfg(not(unix))]
            let _ = path;
            Ok(())
        }
    }
}

fn flushable_file(path: &std::path::Path) -> std::io::Result<std::fs::File> {
    // Windows FlushFileBuffers requires a writable handle, including when a
    // previously published blob is reused after an interrupted import.
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
}

fn reserve_blob_temporary(
    dir: &std::path::Path,
    digest: &str,
    mut next: impl FnMut() -> u64,
) -> std::io::Result<(PathBuf, std::fs::File)> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    for _ in 0..16 {
        let temp = dir.join(format!(".{digest}.{}.{}.tmp", std::process::id(), next()));
        match options.open(&temp) {
            Ok(file) => return Ok((temp, file)),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
    }
    Err(std::io::Error::from(std::io::ErrorKind::AlreadyExists))
}

fn publish_file(temporary: &std::path::Path, path: &std::path::Path) -> std::io::Result<()> {
    #[cfg(target_os = "windows")]
    return crate::windows_file_identity::publish_file(temporary, path);
    #[cfg(not(target_os = "windows"))]
    std::fs::rename(temporary, path)
}
impl MediaStore {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            scans: Default::default(),
        }
    }

    /// Reclaims only exact digest files after proving all live and trashed
    /// references. A cursor bounds each store turn and continues on later use.
    pub(super) fn reconcile_orphans(
        &self,
        profile: ProfileId,
        conn: &Connection,
    ) -> rusqlite::Result<()> {
        self.reconcile_orphans_with_remove(profile, conn, |path| std::fs::remove_file(path))
    }

    fn reconcile_orphans_with_remove(
        &self,
        profile: ProfileId,
        conn: &Connection,
        mut remove: impl FnMut(&std::path::Path) -> std::io::Result<()>,
    ) -> rusqlite::Result<()> {
        const SCAN_PER_TURN: usize = 256;
        let mut scans = self.scans.lock().unwrap_or_else(|p| p.into_inner());
        let scan = scans.entry(profile).or_default();
        if scan.complete {
            return Ok(());
        }
        let io_error =
            |error: std::io::Error| rusqlite::Error::ToSqlConversionFailure(Box::new(error));
        let references = referenced_digests(conn)?;
        if scan.entries.is_none() {
            let dir = self.root.join(profile.to_string());
            match std::fs::symlink_metadata(&dir) {
                Ok(meta) if meta.file_type().is_dir() => {}
                Ok(_) => return Err(rusqlite::Error::InvalidPath(dir)),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    scan.complete = true;
                    return Ok(());
                }
                Err(error) => return Err(io_error(error)),
            }
            scan.entries = Some(std::fs::read_dir(dir).map_err(io_error)?);
        }
        for _ in 0..SCAN_PER_TURN {
            let Some(entry) = scan.entries.as_mut().and_then(Iterator::next) else {
                scan.entries = None;
                scan.complete = !scan.retry;
                scan.retry = false;
                break;
            };
            let entry = entry.map_err(io_error)?;
            let name = entry.file_name();
            let Some(name) = name.to_str() else { continue };
            // Unknown files and save temporaries remain recovery evidence.
            // Symlinks and other special nodes are never followed or removed.
            if !valid_digest(name)
                || name.bytes().any(|byte| byte.is_ascii_uppercase())
                || references.contains(name)
                || !entry.file_type().map_err(io_error)?.is_file()
            {
                continue;
            }
            if let Err(error) = remove(&entry.path()) {
                if error.kind() != std::io::ErrorKind::NotFound {
                    scan.retry = true;
                }
            }
        }
        Ok(())
    }

    fn retry_orphan_scan(&self, profile: ProfileId) {
        self.scans
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .entry(profile)
            .or_default()
            .complete = false;
    }
    pub fn blob_path(&self, profile: ProfileId, digest: &str) -> Option<PathBuf> {
        valid_digest(digest).then(|| self.root.join(profile.to_string()).join(digest))
    }
    /// Writes atomically; an existing identical blob is left untouched.
    pub fn put(&self, profile: ProfileId, digest: &str, bytes: &[u8]) -> std::io::Result<PathBuf> {
        self.put_with_sync(profile, digest, bytes, &mut sync_path)
    }

    fn put_with_sync(
        &self,
        profile: ProfileId,
        digest: &str,
        bytes: &[u8],
        sync: &mut impl FnMut(SyncPoint, &std::path::Path) -> std::io::Result<()>,
    ) -> std::io::Result<PathBuf> {
        let path = self
            .blob_path(profile, digest)
            .ok_or_else(|| std::io::Error::other("invalid digest"))?;
        let dir = path
            .parent()
            .ok_or_else(|| std::io::Error::other("no parent"))?;
        std::fs::create_dir_all(dir)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&self.root, std::fs::Permissions::from_mode(0o700)).ok();
            std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
        }
        if path.is_file() {
            sync(SyncPoint::File, &path)?;
        } else {
            use std::io::Write;
            use std::sync::atomic::{AtomicU64, Ordering};
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let (temp, mut file) =
                reserve_blob_temporary(dir, digest, || NEXT.fetch_add(1, Ordering::Relaxed))?;
            let result = (|| {
                file.write_all(bytes)?;
                sync(SyncPoint::File, &temp)?;
                drop(file);
                publish_file(&temp, &path)
            })();
            if let Err(error) = result {
                let _ = std::fs::remove_file(&temp);
                return Err(error);
            }
        }
        // Persist every newly created directory entry, from the blob outward,
        // before SQLite may commit a reference to these bytes. Windows uses a
        // write-through move because directory FlushFileBuffers is unsupported.
        sync(SyncPoint::Directory, dir)?;
        sync(SyncPoint::Directory, &self.root)?;
        if let Some(parent) = self.root.parent() {
            sync(SyncPoint::Directory, parent)?;
        }
        Ok(path)
    }
    pub fn read(&self, profile: ProfileId, digest: &str, max: usize) -> Option<Vec<u8>> {
        let path = self.blob_path(profile, digest)?;
        let metadata = std::fs::metadata(&path).ok()?;
        if !metadata.is_file() || metadata.len() > max as u64 {
            return None;
        }
        use std::io::Read;
        let mut bytes = Vec::new();
        std::fs::File::open(path)
            .ok()?
            .take(max as u64 + 1)
            .read_to_end(&mut bytes)
            .ok()?;
        (bytes.len() <= max).then_some(bytes)
    }
}

impl Hub {
    pub fn import_media(&mut self, profile: ProfileId, import: MediaImport) -> ResourceResponse {
        if !valid_request(&import.request_id) || !self.knows(profile) {
            return ResourceResponse::Error {
                error: ResourceError::Invalid,
            };
        }
        let Some(media) = self.media.clone() else {
            return ResourceResponse::Error {
                error: ResourceError::Unavailable,
            };
        };
        let asset = match admit(&import.bytes, &import.name, import.origin.clone()) {
            Ok(asset) => asset,
            Err(
                MediaAdmissionError::Empty
                | MediaAdmissionError::Unsupported
                | MediaAdmissionError::Malformed,
            ) => {
                return ResourceResponse::Error {
                    error: ResourceError::Invalid,
                }
            }
            Err(MediaAdmissionError::TooLarge) => {
                return ResourceResponse::Error {
                    error: ResourceError::Capacity,
                }
            }
        };
        let Ok(conn) = self.profile_conn(profile) else {
            return ResourceResponse::Error {
                error: ResourceError::Unavailable,
            };
        };
        if let Some(record) = super::resources::existing_fetched_media(conn, &asset) {
            return ResourceResponse::Applied {
                request_id: import.request_id,
                applied_revision: record.revision.clone(),
                record,
            };
        }
        let digest = asset.digest.clone();
        let title = asset.name.clone();
        let command = ResourceCommand {
            version: 1,
            request_id: import.request_id,
            intent: ResourceIntent::Create {
                draft: ResourceDraft {
                    title,
                    pinned: false,
                    content: ResourceContent::Media { asset },
                    related: vec![],
                },
            },
        };
        let mut publication_failed = false;
        let result = super::resources::mutate_before_commit(conn, profile, command, || {
            media
                .put(profile, &digest, &import.bytes)
                .map(|_| ())
                .map_err(|error| {
                    publication_failed = true;
                    rusqlite::Error::ToSqlConversionFailure(Box::new(error))
                })
        });
        let response = result.unwrap_or(ResourceResponse::Error {
            error: if publication_failed {
                ResourceError::Unavailable
            } else {
                ResourceError::OutcomeUnknown
            },
        });
        if !matches!(response, ResourceResponse::Applied { .. }) {
            media.retry_orphan_scan(profile);
            // A publication may precede a failed SQLite commit. The Store
            // actor is serialized: reclaim only after proving that no live or
            // trashed record references this digest, including an older import.
            let referenced = referenced_digests(conn);
            if matches!(referenced, Ok(references) if !references.contains(&digest.to_ascii_lowercase()))
            {
                if let Some(path) = media.blob_path(profile, &digest) {
                    let _ = std::fs::remove_file(path);
                }
            }
        }
        response
    }
    #[cfg(test)]
    pub(crate) fn media_store(&self) -> Option<&MediaStore> {
        self.media.as_ref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(width: u32, height: u32) -> Vec<u8> {
        let mut out = Vec::new();
        let image = image::RgbaImage::from_pixel(width, height, image::Rgba([10, 20, 30, 255]));
        image::DynamicImage::ImageRgba8(image)
            .write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
            .unwrap();
        out
    }

    #[test]
    fn admission_sniffs_bytes_bounds_decoding_and_cleans_names() {
        let asset = admit(&png(12, 7), "../shot.PNG", MediaOrigin::Imported).unwrap();
        assert_eq!(asset.kind, MediaKind::Image);
        assert_eq!(asset.mime, "image/png");
        assert_eq!((asset.width, asset.height), (Some(12), Some(7)));
        assert_eq!(asset.name, "..shot.PNG");
        assert_eq!(asset.digest.len(), 64);
        assert!(asset.validate());
        let pdf = admit(b"%PDF-1.7 minimal", "spec.pdf", MediaOrigin::Imported).unwrap();
        assert_eq!(pdf.kind, MediaKind::Pdf);
        assert_eq!(pdf.mime, "application/pdf");
        let file = admit(b"plain bytes", "", MediaOrigin::Imported).unwrap();
        assert_eq!(file.kind, MediaKind::File);
        assert_eq!(file.name, "file");
        assert_eq!(
            admit(&[], "x", MediaOrigin::Imported),
            Err(MediaAdmissionError::Empty)
        );
        assert_eq!(
            admit(
                b"\x89PNG\r\n\x1a\ntruncated",
                "x.png",
                MediaOrigin::Imported
            ),
            Err(MediaAdmissionError::Malformed)
        );
        let fetched = MediaOrigin::Fetched {
            url: "https://cdn.example/a.png".into(),
            observed_at: "2026-09-14".into(),
        };
        assert_eq!(
            admit(b"%PDF-1.7", "spec.pdf", fetched.clone()),
            Err(MediaAdmissionError::Unsupported)
        );
        assert!(admit(&png(3, 3), "logo.png", fetched).is_ok());
    }

    #[test]
    fn blobs_are_profile_scoped_content_addressed_and_written_once() {
        let dir = tempfile::tempdir().unwrap();
        let store = MediaStore::new(dir.path().join("media"));
        let profile = ProfileId::from(7);
        let bytes = png(2, 2);
        let asset = admit(&bytes, "a.png", MediaOrigin::Imported).unwrap();
        assert!(store.read(profile, &asset.digest, 1 << 20).is_none());
        let path = store.put(profile, &asset.digest, &bytes).unwrap();
        assert!(path.starts_with(dir.path().join("media").join(profile.to_string())));
        assert_eq!(
            store.read(profile, &asset.digest, 1 << 20),
            Some(bytes.clone())
        );
        assert_eq!(store.read(profile, &asset.digest, 4), None);
        assert!(store.put(profile, &asset.digest, &bytes).is_ok());
        assert!(store.blob_path(profile, "../etc/passwd").is_none());
        assert!(store
            .read(ProfileId::from(8), &asset.digest, 1 << 20)
            .is_none());
    }

    #[test]
    fn blob_publication_flushes_bytes_before_publishing_and_parent_entries_before_success() {
        let dir = tempfile::tempdir().unwrap();
        let store = MediaStore::new(dir.path().join("media"));
        let profile = ProfileId::from(7);
        let bytes = b"durable import";
        let digest = hex(&Sha256::digest(bytes));
        let published = store.blob_path(profile, &digest).unwrap();
        let mut durable_bytes = None;
        let mut durable_name = false;
        let mut directories = Vec::new();
        store
            .put_with_sync(profile, &digest, bytes, &mut |point, path| {
                match point {
                    SyncPoint::File => {
                        assert!(
                            !published.exists(),
                            "bytes must reach disk before publication"
                        );
                        durable_bytes = Some(std::fs::read(path)?);
                    }
                    SyncPoint::Directory => {
                        assert_eq!(durable_bytes.as_deref(), Some(bytes.as_slice()));
                        assert!(published.is_file());
                        durable_name = true;
                        directories.push(path.to_path_buf());
                    }
                }
                Ok(())
            })
            .unwrap();
        assert!(
            durable_name,
            "a crash must retain the published namespace entry"
        );
        assert_eq!(
            directories,
            [
                published.parent().unwrap(),
                store.root.as_path(),
                dir.path()
            ]
        );
    }

    #[test]
    fn failed_blob_flush_refuses_publication_and_removes_temporary_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let store = MediaStore::new(dir.path().join("media"));
        let profile = ProfileId::from(8);
        let digest = hex(&Sha256::digest(b"bytes"));
        let result = store.put_with_sync(profile, &digest, b"bytes", &mut |_, _| {
            Err(std::io::Error::other("injected failed flush"))
        });
        assert!(result.is_err());
        assert!(!store.blob_path(profile, &digest).unwrap().exists());
        assert_eq!(
            std::fs::read_dir(store.root.join(profile.to_string()))
                .unwrap()
                .count(),
            0
        );
    }

    #[test]
    fn blob_flush_handles_have_write_access_without_truncating_bytes() {
        use std::io::{Seek, SeekFrom, Write};
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("blob");
        std::fs::write(&path, b"retained").unwrap();
        let mut file = flushable_file(&path).unwrap();
        assert_eq!(file.metadata().unwrap().len(), 8);
        file.seek(SeekFrom::End(0)).unwrap();
        file.write_all(b"!").unwrap();
        file.sync_all().unwrap();
        assert_eq!(std::fs::read(path).unwrap(), b"retained!");
    }

    #[test]
    fn blob_temporary_collisions_preserve_preexisting_recovery_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let digest = hex(&Sha256::digest(b"bytes"));
        let collision = dir
            .path()
            .join(format!(".{digest}.{}.0.tmp", std::process::id()));
        std::fs::write(&collision, b"prior interrupted save").unwrap();
        let mut next = 0;
        let (reserved, file) = reserve_blob_temporary(dir.path(), &digest, || {
            let value = next;
            next += 1;
            value
        })
        .unwrap();
        assert_ne!(reserved, collision);
        assert_eq!(std::fs::read(collision).unwrap(), b"prior interrupted save");
        drop(file);
        std::fs::remove_file(reserved).unwrap();
    }


    #[test]
    fn hub_import_mints_a_media_resource_that_callers_cannot_forge() {
        let dir = tempfile::tempdir().unwrap();
        let mut hub = Hub::open(dir.path().to_path_buf()).unwrap();
        let profile = ProfileId::from(41);
        hub.meta
            .execute(
                "INSERT OR IGNORE INTO profiles(id, name, kind, position) VALUES (?1, 'Fixture', 'named', 0)",
                [profile.to_string()],
            )
            .unwrap();
        hub.load_registry().unwrap();
        assert!(hub.knows(profile));
        let bytes = png(4, 4);
        let response = hub.import_media(
            profile,
            MediaImport {
                request_id: "media-import-request-0001".into(),
                name: "Keyboard.png".into(),
                origin: MediaOrigin::Imported,
                bytes: std::sync::Arc::new(bytes.clone()),
            },
        );
        let ResourceResponse::Applied { record, .. } = response else {
            panic!("import must apply");
        };
        let ResourceContent::Media { asset } = &record.draft.content else {
            panic!("media content");
        };
        assert!(hub
            .media_store()
            .unwrap()
            .read(profile, &asset.digest, 1 << 20)
            .is_some());
        assert_eq!(record.draft.title, "Keyboard.png");
        use zephium_core::work::{
            port::{WorkReply, WorkRequest},
            WorkError,
        };
        let request = WorkRequest::ReadMediaContext {
            resource: record.id.clone(),
            revision: record.revision.clone(),
        };
        let WorkReply::MediaContext(context) = hub.work_document(profile, request.clone()).unwrap()
        else {
            panic!()
        };
        assert_eq!(context.0, bytes);
        assert!(!format!("{context:?}").contains("PNG"));
        assert!(hub.work_document(ProfileId::from(42), request).is_err());
        assert!(matches!(
            hub.work_document(
                profile,
                WorkRequest::ReadMediaContext {
                    resource: record.id.clone(),
                    revision: "stale".into(),
                }
            ),
            Err(WorkError::Conflict)
        ));
        let forged = ResourceCall::Mutate {
            command: Box::new(ResourceCommand {
                version: 1,
                request_id: "media-forged-request-0002".into(),
                intent: ResourceIntent::Create {
                    draft: record.draft.clone(),
                },
            }),
        };
        assert!(!forged.validate());
        let replay = hub.import_media(
            profile,
            MediaImport {
                request_id: "media-import-request-0001".into(),
                name: "Keyboard.png".into(),
                origin: MediaOrigin::Imported,
                bytes: std::sync::Arc::new(bytes),
            },
        );
        assert!(
            matches!(replay, ResourceResponse::Applied { record: again, .. } if again.id == record.id)
        );
        // A public URL fetched again, or its bytes seen under another URL,
        // reuses the live resource; a second file import is a new resource.
        let fetched = |hub: &mut Hub, request: &str, url: &str, bytes: Vec<u8>| {
            let response = hub.import_media(
                profile,
                MediaImport {
                    request_id: request.into(),
                    name: "set.jpg".into(),
                    origin: MediaOrigin::Fetched {
                        url: url.into(),
                        observed_at: "2026-09-17".into(),
                    },
                    bytes: std::sync::Arc::new(bytes),
                },
            );
            let ResourceResponse::Applied { record, .. } = response else {
                panic!("fetched import must apply");
            };
            record.id
        };
        let first = fetched(
            &mut hub,
            "media-fetch-request-0001",
            "https://a.example/set.png",
            png(5, 5),
        );
        let same_url = fetched(
            &mut hub,
            "media-fetch-request-0002",
            "https://a.example/set.png",
            png(6, 6),
        );
        let same_bytes = fetched(
            &mut hub,
            "media-fetch-request-0003",
            "https://b.example/set.png",
            png(5, 5),
        );
        let other = fetched(
            &mut hub,
            "media-fetch-request-0004",
            "https://c.example/set.png",
            png(7, 7),
        );
        assert_eq!(first, same_url);
        assert_eq!(first, same_bytes);
        assert_ne!(first, other);
        let imported_again = hub.import_media(
            profile,
            MediaImport {
                request_id: "media-import-request-0003".into(),
                name: "Keyboard.png".into(),
                origin: MediaOrigin::Imported,
                bytes: std::sync::Arc::new(png(4, 4)),
            },
        );
        assert!(
            matches!(imported_again, ResourceResponse::Applied { record: again, .. } if again.id != record.id)
        );
    }

    #[test]
    fn rejected_media_imports_never_accumulate_unreferenced_blobs() {
        let dir = tempfile::tempdir().unwrap();
        let mut hub = Hub::open(dir.path().to_path_buf()).unwrap();
        let profile = ProfileId::from(61);
        hub.meta
            .execute(
                "INSERT INTO profiles(id,name,kind,position) VALUES(?1,'Fixture','named',0)",
                [profile.to_string()],
            )
            .unwrap();
        hub.load_registry().unwrap();
        let import = |request: &str, bytes: &[u8]| MediaImport {
            request_id: request.into(),
            name: "file.bin".into(),
            origin: MediaOrigin::Imported,
            bytes: std::sync::Arc::new(bytes.to_vec()),
        };
        assert!(matches!(
            hub.import_media(profile, import("media-request-original-001", b"retained")),
            ResourceResponse::Applied { .. }
        ));
        let conflict = hub.import_media(
            profile,
            import("media-request-original-001", b"conflicting"),
        );
        assert!(matches!(
            conflict,
            ResourceResponse::Error {
                error: ResourceError::Conflict
            }
        ));
        let conflict_digest = hex(&Sha256::digest(b"conflicting"));
        assert!(!hub
            .media_store()
            .unwrap()
            .blob_path(profile, &conflict_digest)
            .unwrap()
            .exists());
        hub.profile_conn(profile).unwrap().execute_batch("WITH RECURSIVE fill(n) AS (VALUES(1) UNION ALL SELECT n+1 FROM fill WHERE n<9999) INSERT INTO user_resources(id,kind,revision,title,pinned,trashed,created_at,updated_at,body,search_text) SELECT printf('%026d',n),kind,revision,title,pinned,trashed,created_at,updated_at,body,search_text FROM fill CROSS JOIN (SELECT * FROM user_resources LIMIT 1);").unwrap();
        for index in 0..3 {
            let bytes = format!("rejected capacity {index}");
            let response = hub.import_media(
                profile,
                import(
                    &format!("media-request-capacity-{index:03}"),
                    bytes.as_bytes(),
                ),
            );
            assert!(matches!(
                response,
                ResourceResponse::Error {
                    error: ResourceError::Capacity
                }
            ));
            let digest = hex(&Sha256::digest(bytes.as_bytes()));
            assert!(!hub
                .media_store()
                .unwrap()
                .blob_path(profile, &digest)
                .unwrap()
                .exists());
        }
    }

    #[test]
    fn failed_media_commit_reclaims_only_unreferenced_publications() {
        let dir = tempfile::tempdir().unwrap();
        let mut hub = Hub::open(dir.path().to_path_buf()).unwrap();
        let profile = ProfileId::from(62);
        hub.meta
            .execute(
                "INSERT INTO profiles(id,name,kind,position) VALUES(?1,'Fixture','named',0)",
                [profile.to_string()],
            )
            .unwrap();
        hub.load_registry().unwrap();
        let import = |request: &str, bytes: &[u8]| MediaImport {
            request_id: request.into(),
            name: "file.bin".into(),
            origin: MediaOrigin::Imported,
            bytes: std::sync::Arc::new(bytes.to_vec()),
        };
        assert!(matches!(
            hub.import_media(profile, import("media-request-retained-001", b"retained")),
            ResourceResponse::Applied { .. }
        ));
        let conn = hub.profile_conn(profile).unwrap();
        conn.execute_batch("UPDATE user_resources SET body=json_set(body,'$.content.asset.digest',upper(json_extract(body,'$.content.asset.digest')));").unwrap();
        conn.execute_batch("CREATE TRIGGER deferred_media_commit_failure AFTER INSERT ON user_resources BEGIN INSERT INTO user_resource_receipts(request_id,digest,resource_id,revision,retained) VALUES('injected-deferred',zeroblob(32),'00000000000000000000000000',1,0); END;").unwrap();
        for (request, bytes, retained) in [
            ("media-request-failure-001", b"new blob".as_slice(), false),
            ("media-request-failure-002", b"retained".as_slice(), true),
        ] {
            hub.profile_conn(profile)
                .unwrap()
                .execute_batch("PRAGMA defer_foreign_keys=ON;")
                .unwrap();
            assert!(matches!(
                hub.import_media(profile, import(request, bytes)),
                ResourceResponse::Error {
                    error: ResourceError::OutcomeUnknown
                }
            ));
            let digest = hex(&Sha256::digest(bytes));
            assert_eq!(
                hub.media_store()
                    .unwrap()
                    .blob_path(profile, &digest)
                    .unwrap()
                    .is_file(),
                retained
            );
        }
    }

    #[test]
    fn crash_orphans_are_reclaimed_after_reopen_while_shared_and_trashed_blobs_survive() {
        let dir = tempfile::tempdir().unwrap();
        let profile = ProfileId::from(63);
        let mut hub = Hub::open(dir.path().to_path_buf()).unwrap();
        hub.meta
            .execute(
                "INSERT INTO profiles(id,name,kind,position) VALUES(?1,'Fixture','named',0)",
                [profile.to_string()],
            )
            .unwrap();
        hub.load_registry().unwrap();
        for (index, bytes) in [
            b"shared".as_slice(),
            b"shared".as_slice(),
            b"trashed".as_slice(),
        ]
        .into_iter()
        .enumerate()
        {
            assert!(matches!(
                hub.import_media(
                    profile,
                    MediaImport {
                        request_id: format!("media-crash-retained-{index:04}"),
                        name: "file".into(),
                        origin: MediaOrigin::Imported,
                        bytes: std::sync::Arc::new(bytes.to_vec())
                    }
                ),
                ResourceResponse::Applied { .. }
            ));
        }
        let trashed = hex(&Sha256::digest(b"trashed"));
        hub.profile_conn(profile).unwrap().execute("UPDATE user_resources SET trashed=1 WHERE json_extract(body,'$.content.asset.digest')=?1", [&trashed]).unwrap();
        let orphan = hex(&Sha256::digest(b"crashed before commit"));
        let store = hub.media_store().unwrap().clone();
        let orphan_path = store
            .put(profile, &orphan, b"crashed before commit")
            .unwrap();
        let folder = orphan_path.parent().unwrap();
        std::fs::write(folder.join("unknown-recovery-file"), b"keep").unwrap();
        #[cfg(unix)]
        {
            let outside = dir.path().join("outside");
            std::fs::write(&outside, b"keep outside").unwrap();
            std::os::unix::fs::symlink(&outside, folder.join("f".repeat(64))).unwrap();
        }
        drop(hub);
        let mut reopened = Hub::open(dir.path().to_path_buf()).unwrap();
        reopened.profile_conn(profile).unwrap();
        assert!(!orphan_path.exists());
        assert!(store
            .blob_path(profile, &hex(&Sha256::digest(b"shared")))
            .unwrap()
            .is_file());
        assert!(store.blob_path(profile, &trashed).unwrap().is_file());
        assert!(folder.join("unknown-recovery-file").is_file());
        #[cfg(unix)]
        assert_eq!(
            std::fs::read(dir.path().join("outside")).unwrap(),
            b"keep outside"
        );
    }

    #[test]
    fn orphan_reconciliation_refuses_unknown_reference_inventory_and_preserves_case_variants() {
        let dir = tempfile::tempdir().unwrap();
        let store = MediaStore::new(dir.path().join("media"));
        let profile = ProfileId::from(64);
        let digest = hex(&Sha256::digest(b"referenced"));
        let path = store.put(profile, &digest, b"referenced").unwrap();
        let variant = path.with_file_name(digest.to_ascii_uppercase());
        std::fs::rename(&path, &variant).unwrap();
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE user_resources(kind TEXT,body TEXT);")
            .unwrap();
        conn.execute(
            "INSERT INTO user_resources VALUES('media',?1)",
            [serde_json::json!({"content":{"asset":{"digest":digest}}}).to_string()],
        )
        .unwrap();
        store.reconcile_orphans(profile, &conn).unwrap();
        assert!(variant.is_file());
        conn.execute_batch("UPDATE user_resources SET body=json_set(body,'$.content.asset.digest',upper(json_extract(body,'$.content.asset.digest')));").unwrap();
        let lowercase_profile = ProfileId::from(68);
        let lowercase = store
            .put(lowercase_profile, &digest, b"referenced")
            .unwrap();
        store.reconcile_orphans(lowercase_profile, &conn).unwrap();
        assert!(
            lowercase.is_file(),
            "uppercase record digests must retain lowercase file aliases"
        );

        let other = ProfileId::from(65);
        let path = store.put(other, &digest, b"referenced").unwrap();
        conn.execute_batch(
            "DELETE FROM user_resources; INSERT INTO user_resources VALUES('media','not JSON');",
        )
        .unwrap();
        assert!(store.reconcile_orphans(other, &conn).is_err());
        assert!(path.is_file());
        conn.execute_batch("DELETE FROM user_resources; WITH RECURSIVE fill(n) AS (VALUES(1) UNION ALL SELECT n+1 FROM fill WHERE n<10001) INSERT INTO user_resources SELECT 'media','{\"content\":{\"asset\":{\"digest\":\"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\"}}}' FROM fill;").unwrap();
        assert!(store.reconcile_orphans(other, &conn).is_err());
        assert!(path.is_file());
    }

    #[test]
    fn orphan_reconciliation_is_bounded_and_failed_unlinks_retry_on_later_turns() {
        let dir = tempfile::tempdir().unwrap();
        let store = MediaStore::new(dir.path().join("media"));
        let profile = ProfileId::from(66);
        let folder = store.root.join(profile.to_string());
        std::fs::create_dir_all(&folder).unwrap();
        for index in 0..300 {
            std::fs::write(
                folder.join(hex(&Sha256::digest(index.to_string().as_bytes()))),
                b"orphan",
            )
            .unwrap();
        }
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE user_resources(kind TEXT,body TEXT);")
            .unwrap();
        store.reconcile_orphans(profile, &conn).unwrap();
        assert_eq!(std::fs::read_dir(&folder).unwrap().count(), 44);
        store
            .reconcile_orphans_with_remove(profile, &conn, |_| {
                Err(std::io::Error::from(std::io::ErrorKind::PermissionDenied))
            })
            .unwrap();
        assert_eq!(std::fs::read_dir(&folder).unwrap().count(), 44);
        store.reconcile_orphans(profile, &conn).unwrap();
        assert_eq!(std::fs::read_dir(folder).unwrap().count(), 0);
    }
}
