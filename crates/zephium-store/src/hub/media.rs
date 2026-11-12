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
        Self { root }
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
        if media.put(profile, &asset.digest, &import.bytes).is_err() {
            return ResourceResponse::Error {
                error: ResourceError::Unavailable,
            };
        }
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
        super::resources::mutate(conn, profile, command).unwrap_or(ResourceResponse::Error {
            error: ResourceError::OutcomeUnknown,
        })
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
}
