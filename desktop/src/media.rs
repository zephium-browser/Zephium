//! Media & Files: native import into the profile's media store, bounded
//! serving of admitted images to privileged chrome, and OS open for the rest.
use std::borrow::Cow;
use std::time::Duration;
use tauri::{AppHandle, Manager, State, WebviewWindow};
use tauri_plugin_dialog::DialogExt;
use zephium_core::ids::{ProfileId, ResourceId};
use zephium_core::resources::{
    MediaAssetV1, MediaImport, MediaKind, MediaOrigin, ResourceCall, ResourceContent,
    ResourceError, ResourceResponse, MAX_MEDIA_FILE_BYTES, MAX_MEDIA_IMAGE_BYTES,
};
use zephium_ipc::MediaImportV1;

pub(crate) const SCHEME: &str = "zephium-media";

/// Blob paths for the custom scheme; bytes are admitted only through the
/// store actor.
pub(crate) struct MediaBlobs(pub(crate) zephium_store::MediaStore);

fn profile_of(value: &str) -> Option<ProfileId> {
    ProfileId::parse(value).filter(|id| id.to_string() == value)
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn media_import(
    caller: WebviewWindow,
    app: AppHandle,
    shell: State<'_, zephium_app::Handle>,
    expected_profile: String,
) -> Result<MediaImportV1, ()> {
    let refused = |error| Ok(MediaImportV1::Refused { error });
    if !super::authorize(&caller, super::CallerPolicy::Main, "media_import")
        || super::shutdown_started(&app)
    {
        return refused(ResourceError::Unavailable);
    }
    let Some(profile) = profile_of(&expected_profile) else {
        return refused(ResourceError::Invalid);
    };
    let picker = app.clone();
    let picked = tokio::task::spawn_blocking(move || {
        picker
            .dialog()
            .file()
            .set_title("Add to Work")
            .blocking_pick_file()
    })
    .await
    .map_err(|_| ())?;
    let Some(picked) = picked else {
        return Ok(MediaImportV1::Cancelled);
    };
    let path = match picked {
        tauri_plugin_dialog::FilePath::Path(path) => path,
        tauri_plugin_dialog::FilePath::Url(url) => match url.to_file_path() {
            Ok(path) => path,
            Err(()) => return refused(ResourceError::Invalid),
        },
    };
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "file".into());
    let read = tokio::task::spawn_blocking(move || -> Result<Vec<u8>, ResourceError> {
        let file = std::fs::File::open(&path).map_err(|_| ResourceError::NotFound)?;
        let metadata = file.metadata().map_err(|_| ResourceError::Unavailable)?;
        if !metadata.is_file() {
            return Err(ResourceError::Invalid);
        }
        if metadata.len() > u64::from(MAX_MEDIA_FILE_BYTES) {
            return Err(ResourceError::Capacity);
        }
        read_import_bytes(file, u64::from(MAX_MEDIA_FILE_BYTES))
    })
    .await
    .map_err(|_| ())?;
    let bytes = match read {
        Ok(bytes) => bytes,
        Err(error) => return refused(error),
    };
    let receiver = shell.import_media(
        profile,
        MediaImport {
            request_id: format!("media-import-{}", ResourceId::generate()),
            name,
            origin: MediaOrigin::Imported,
            bytes: std::sync::Arc::new(bytes),
        },
    );
    let reply = tokio::task::spawn_blocking(move || receiver.recv_timeout(Duration::from_secs(30)))
        .await
        .map_err(|_| ())?;
    let Ok(reply) = reply else {
        return refused(ResourceError::OutcomeUnknown);
    };
    match reply.response {
        ResourceResponse::Applied { record, .. } => {
            super::emit_media_changed(&app, &profile.to_string(), &record.id, &record.revision);
            Ok(MediaImportV1::Imported {
                record: Box::new(record),
            })
        }
        ResourceResponse::Error { error } => refused(error),
        _ => refused(ResourceError::Invalid),
    }
}

fn read_import_bytes(reader: impl std::io::Read, limit: u64) -> Result<Vec<u8>, ResourceError> {
    use std::io::Read;
    // A selected file can grow after its metadata check. Bound the read itself
    // and keep the same open file, rather than reopening a replaceable path.
    let mut bytes = Vec::new();
    reader
        .take(limit.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|_| ResourceError::Unavailable)?;
    if bytes.len() as u64 > limit {
        return Err(ResourceError::Capacity);
    }
    Ok(bytes)
}

async fn media_asset(
    shell: &zephium_app::Handle,
    profile: ProfileId,
    id: &str,
) -> Result<MediaAssetV1, ResourceError> {
    if !zephium_core::resources::valid_id(id) {
        return Err(ResourceError::Invalid);
    }
    let receiver = shell.resource_call(profile, ResourceCall::Get { id: id.to_owned() });
    let reply = tokio::task::spawn_blocking(move || receiver.recv_timeout(Duration::from_secs(8)))
        .await
        .map_err(|_| ResourceError::Unavailable)?
        .map_err(|_| ResourceError::Unavailable)?;
    match reply.response {
        ResourceResponse::Record { record } => match record.draft.content {
            ResourceContent::Media { asset } if !record.trashed => Ok(asset),
            _ => Err(ResourceError::NotFound),
        },
        ResourceResponse::Error { error } => Err(error),
        _ => Err(ResourceError::Invalid),
    }
}

/// Opens a non-image asset with the OS default application. The blob is the
/// profile's own snapshot; nothing outside the media store is reachable.
#[tauri::command]
#[specta::specta]
pub(crate) async fn media_open(
    caller: WebviewWindow,
    app: AppHandle,
    shell: State<'_, zephium_app::Handle>,
    expected_profile: String,
    id: String,
) -> Result<bool, ()> {
    if !super::authorize(&caller, super::CallerPolicy::Main, "media_open")
        || super::shutdown_started(&app)
    {
        return Ok(false);
    }
    let Some(profile) = profile_of(&expected_profile) else {
        return Ok(false);
    };
    let Ok(asset) = media_asset(&shell, profile, &id).await else {
        return Ok(false);
    };
    let Some(blobs) = app.try_state::<MediaBlobs>() else {
        return Ok(false);
    };
    let Some(path) = blobs.0.blob_path(profile, &asset.digest) else {
        return Ok(false);
    };
    if !path.is_file() {
        return Ok(false);
    }
    let opened = tokio::task::spawn_blocking(move || open_with_os(&path))
        .await
        .unwrap_or(false);
    Ok(opened)
}

#[cfg(not(target_os = "windows"))]
fn open_with_os(path: &std::path::Path) -> bool {
    #[cfg(target_os = "macos")]
    let mut command = {
        let mut command = std::process::Command::new("open");
        command.arg(path);
        command
    };
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let mut command = {
        let mut command = std::process::Command::new("xdg-open");
        command.arg(path);
        command
    };
    command
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

#[cfg(target_os = "windows")]
fn open_with_os(path: &std::path::Path) -> bool {
    windows_shell_path(path, false)
}

#[cfg(target_os = "windows")]
fn windows_shell_path(path: &std::path::Path, reveal: bool) -> bool {
    use windows::core::{w, PCWSTR};
    use windows::Win32::Foundation::RPC_E_CHANGED_MODE;
    use windows::Win32::System::Com::{
        CoInitializeEx, CoTaskMemFree, CoUninitialize, COINIT_APARTMENTTHREADED,
    };
    use windows::Win32::UI::Shell::{
        SHOpenFolderAndSelectItems, SHParseDisplayName, ShellExecuteW,
    };
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    let Some(path) = windows_shell_wide(path) else {
        return false;
    };
    let initialized = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
    if initialized.is_err() && initialized != RPC_E_CHANGED_MODE {
        return false;
    }
    let opened = if reveal {
        let mut item = std::ptr::null_mut();
        if unsafe { SHParseDisplayName(PCWSTR(path.as_ptr()), None, &mut item, 0, None) }.is_err() {
            false
        } else {
            let opened = unsafe { SHOpenFolderAndSelectItems(item, None, 0) }.is_ok();
            unsafe { CoTaskMemFree(Some(item.cast())) };
            opened
        }
    } else {
        let result = unsafe {
            ShellExecuteW(
                None,
                w!("open"),
                PCWSTR(path.as_ptr()),
                None,
                None,
                SW_SHOWNORMAL,
            )
        };
        result.0 as isize > 32
    };
    if initialized.is_ok() {
        unsafe { CoUninitialize() };
    }
    opened
}

#[cfg(target_os = "windows")]
fn windows_shell_wide(path: &std::path::Path) -> Option<Vec<u16>> {
    use std::os::windows::ffi::OsStrExt;
    let mut wide: Vec<_> = path.as_os_str().encode_wide().collect();
    if wide.is_empty() || wide.len() > 32766 || wide.contains(&0) {
        return None;
    }
    if wide.starts_with(&[92, 92, 63, 92, 85, 78, 67, 92]) {
        wide.drain(..6);
        wide[0] = 92;
    } else if wide.starts_with(&[92, 92, 63, 92]) {
        wide.drain(..4);
    }
    wide.push(0);
    Some(wide)
}

fn sniff_image(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("image/jpeg")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some("image/webp")
    } else {
        None
    }
}

fn respond(
    status: u16,
    body: Vec<u8>,
    content_type: &str,
) -> tauri::http::Response<Cow<'static, [u8]>> {
    tauri::http::Response::builder()
        .status(status)
        .header("Content-Type", content_type)
        .header("X-Content-Type-Options", "nosniff")
        .header("Cache-Control", "private, max-age=31536000, immutable")
        .header("Content-Security-Policy", "default-src 'none'; sandbox")
        .body(Cow::Owned(body))
        .unwrap_or_else(|_| tauri::http::Response::new(Cow::Borrowed(b"" as &[u8])))
}

/// `zephium-media://localhost/<profile>/<digest>[?w=<px>]` from privileged main
/// chrome only. Serves admitted image blobs, scaled down to `w` when the
/// picture is wider and one is asked for; every other request is 404. Work
/// happens off the main thread.
pub(crate) fn serve(
    ctx: tauri::UriSchemeContext<'_, tauri::Wry>,
    request: tauri::http::Request<Vec<u8>>,
    responder: tauri::UriSchemeResponder,
) {
    if ctx.webview_label() != super::MAIN_LABEL {
        return responder.respond(respond(403, Vec::new(), "text/plain"));
    }
    let app = ctx.app_handle().clone();
    let uri = request.uri().clone();
    tauri::async_runtime::spawn_blocking(move || {
        responder.respond(resolve(&app, uri.path(), uri.query()));
    });
}

fn resolve(
    app: &AppHandle,
    path: &str,
    query: Option<&str>,
) -> tauri::http::Response<Cow<'static, [u8]>> {
    let path = path.trim_start_matches('/');
    let width = thumb::width(query);
    if let Some(rest) = path.strip_prefix("frame/") {
        return serve_page_frame(app, rest, width);
    }
    let Some((profile_text, digest)) = path.split_once('/') else {
        return respond(404, Vec::new(), "text/plain");
    };
    let Some(profile) = profile_of(profile_text) else {
        return respond(404, Vec::new(), "text/plain");
    };
    let Some(blobs) = app.try_state::<MediaBlobs>() else {
        return respond(503, Vec::new(), "text/plain");
    };
    if let Some(width) = width {
        if let Some(hit) = thumb::cached(app, profile_text, digest, width) {
            return respond(200, hit.0, hit.1);
        }
    }
    let Some(bytes) = blobs
        .0
        .read(profile, digest, MAX_MEDIA_IMAGE_BYTES as usize)
    else {
        return respond(404, Vec::new(), "text/plain");
    };
    let Some(mime) = sniff_image(&bytes) else {
        return respond(404, Vec::new(), "text/plain");
    };
    if let Some(width) = width {
        if let Some((small, small_mime)) = thumb::scaled(&bytes, width) {
            thumb::keep(app, profile_text, digest, width, &small, small_mime);
            return respond(200, small, small_mime);
        }
    }
    respond(200, bytes, mime)
}

/// `frame/<attempt>/<step>/<generation>`: the newest frame of one agent page.
/// The generation only busts caches; the bytes are whatever is current.
fn serve_page_frame(
    app: &AppHandle,
    rest: &str,
    _width: Option<u32>,
) -> tauri::http::Response<Cow<'static, [u8]>> {
    let mut parts = rest.split('/');
    let ids = (parts.next(), parts.next());
    #[cfg(feature = "work-product")]
    {
        use zephium_core::work::{WorkAttemptId, WorkStepId};
        let (Some(attempt), Some(step)) = (
            ids.0.and_then(WorkAttemptId::parse),
            ids.1.and_then(WorkStepId::parse),
        ) else {
            return respond(404, Vec::new(), "text/plain");
        };
        let Some(state) = app.try_state::<super::work_product::WorkProductState>() else {
            return respond(503, Vec::new(), "text/plain");
        };
        match state.page_frame(attempt, step) {
            // Native Work captures already have fixed pixel/byte ceilings.
            // Preserve their text and fine edges through DPI and canvas zoom;
            // card-width JPEG copies would discard that information again.
            Some(png) => respond(200, png.as_ref().clone(), "image/png"),
            None => {
                #[cfg(feature = "work-development-traces")]
                super::work_provider::record_diagnostic(format_args!(
                    "work: phase=page_frame served=false attempt={attempt} step={step}"
                ));
                respond(404, Vec::new(), "text/plain")
            }
        }
    }
    #[cfg(not(feature = "work-product"))]
    {
        let _ = (app, ids);
        respond(404, Vec::new(), "text/plain")
    }
}

/// Scaled copies of pictures at the width the canvas draws them: a picture
/// decoded at 1600 px costs six megabytes of the interface's memory to show
/// at 300, so the interface asks for the small one. Made once, kept on disk.
mod thumb {
    use super::{AppHandle, Manager};
    use std::path::PathBuf;

    /// The widths a copy is made at, so a few variants exist per picture.
    const WIDTHS: [u32; 7] = [160, 240, 360, 540, 800, 1200, 1600];
    /// What the copies may hold on disk before the oldest go.
    const CACHE_BYTES: u64 = 96 * 1024 * 1024;

    pub(super) fn width(query: Option<&str>) -> Option<u32> {
        let asked: u32 = query?
            .split('&')
            .find_map(|pair| pair.strip_prefix("w=")?.parse().ok())?;
        WIDTHS.iter().copied().find(|step| *step >= asked)
    }

    fn path(app: &AppHandle, profile: &str, digest: &str, width: u32) -> Option<PathBuf> {
        let plain = digest.len() == 64 && digest.bytes().all(|b| b.is_ascii_hexdigit());
        if !plain {
            return None;
        }
        let dir = app.path().app_cache_dir().ok()?.join("media-thumbs");
        Some(dir.join(profile).join(format!("{digest}-{width}")))
    }

    pub(super) fn cached(
        app: &AppHandle,
        profile: &str,
        digest: &str,
        width: u32,
    ) -> Option<(Vec<u8>, &'static str)> {
        let base = path(app, profile, digest, width)?;
        for (extension, mime) in [("jpg", "image/jpeg"), ("png", "image/png")] {
            if let Ok(bytes) = std::fs::read(base.with_extension(extension)) {
                return Some((bytes, mime));
            }
        }
        None
    }

    pub(super) fn keep(
        app: &AppHandle,
        profile: &str,
        digest: &str,
        width: u32,
        bytes: &[u8],
        mime: &str,
    ) {
        use std::sync::atomic::{AtomicU32, Ordering};
        static WRITES: AtomicU32 = AtomicU32::new(0);
        let Some(base) = path(app, profile, digest, width) else {
            return;
        };
        let file = base.with_extension(if mime == "image/png" { "png" } else { "jpg" });
        let Some(dir) = file.parent() else { return };
        if std::fs::create_dir_all(dir).is_err() {
            return;
        }
        let temporary = file.with_extension("tmp");
        if std::fs::write(&temporary, bytes).is_ok() && std::fs::rename(&temporary, &file).is_err()
        {
            let _ = std::fs::remove_file(&temporary);
        }
        if WRITES.fetch_add(1, Ordering::Relaxed).is_multiple_of(32) {
            if let Some(root) = dir.parent() {
                prune(root);
            }
        }
    }

    /// The oldest copies go until what is kept fits the cache.
    fn prune(root: &std::path::Path) {
        let mut files: Vec<(std::time::SystemTime, u64, PathBuf)> = Vec::new();
        for profile in std::fs::read_dir(root).into_iter().flatten().flatten() {
            for entry in std::fs::read_dir(profile.path())
                .into_iter()
                .flatten()
                .flatten()
            {
                if let Ok(meta) = entry.metadata() {
                    let stamp = meta
                        .accessed()
                        .or_else(|_| meta.modified())
                        .unwrap_or(std::time::UNIX_EPOCH);
                    files.push((stamp, meta.len(), entry.path()));
                }
            }
        }
        let mut total: u64 = files.iter().map(|file| file.1).sum();
        files.sort_by_key(|file| file.0);
        for (_, size, path) in files {
            if total <= CACHE_BYTES {
                break;
            }
            if std::fs::remove_file(path).is_ok() {
                total -= size;
            }
        }
    }

    /// The picture at `width`, or None where it is already that narrow (or cannot be
    /// scaled: an animation keeps its frames): JPEG, PNG where it has transparency.
    #[cfg(feature = "work-product")]
    pub(super) fn scaled(bytes: &[u8], width: u32) -> Option<(Vec<u8>, &'static str)> {
        use image::{ImageFormat, ImageReader};
        use std::io::Cursor;
        static ONE_AT_A_TIME: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let format = image::guess_format(bytes).ok()?;
        if format == ImageFormat::Gif {
            return None;
        }
        let mut limits = image::Limits::default();
        limits.max_image_width = Some(zephium_core::resources::MAX_MEDIA_DIMENSION);
        limits.max_image_height = Some(zephium_core::resources::MAX_MEDIA_DIMENSION);
        limits.max_alloc = Some(256 * 1024 * 1024);
        let mut reader = ImageReader::with_format(Cursor::new(bytes), format);
        reader.limits(limits.clone());
        let (natural, height) = reader.into_dimensions().ok()?;
        if natural <= width {
            return None;
        }
        let _turn = ONE_AT_A_TIME
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut reader = ImageReader::with_format(Cursor::new(bytes), format);
        reader.limits(limits);
        let decoded = reader.decode().ok()?;
        let alpha = decoded.color().has_alpha()
            && decoded.to_rgba8().pixels().any(|pixel| pixel.0[3] < 255);
        let target = (u64::from(height) * u64::from(width)).div_ceil(u64::from(natural));
        let small = decoded.resize_exact(
            width,
            u32::try_from(target).ok()?.max(1),
            image::imageops::FilterType::Triangle,
        );
        let mut out = Vec::new();
        if alpha {
            small
                .to_rgba8()
                .write_to(&mut Cursor::new(&mut out), ImageFormat::Png)
                .ok()?;
            Some((out, "image/png"))
        } else {
            image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 82)
                .encode_image(&small.to_rgb8())
                .ok()?;
            Some((out, "image/jpeg"))
        }
    }

    #[cfg(not(feature = "work-product"))]
    pub(super) fn scaled(_bytes: &[u8], _width: u32) -> Option<(Vec<u8>, &'static str)> {
        None
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn a_width_is_asked_in_steps() {
            assert_eq!(width(Some("w=100")), Some(160));
            assert_eq!(width(Some("x=1&w=300")), Some(360));
            assert_eq!(width(Some("w=1600")), Some(1600));
            assert_eq!(width(Some("w=2000")), None);
            assert_eq!(width(Some("w=abc")), None);
            assert_eq!(width(None), None);
        }

        #[cfg(feature = "work-product")]
        #[test]
        fn a_wide_picture_is_scaled_and_a_narrow_one_left_alone() {
            let png = |width: u32, height: u32, alpha: u8| {
                let mut out = Vec::new();
                image::RgbaImage::from_pixel(width, height, image::Rgba([10, 20, 30, alpha]))
                    .write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
                    .unwrap();
                out
            };
            let (small, mime) = scaled(&png(1600, 900, 255), 360).unwrap();
            let decoded = image::load_from_memory(&small).unwrap();
            assert_eq!((decoded.width(), decoded.height()), (360, 203));
            assert_eq!(mime, "image/jpeg");
            let (clear, mime) = scaled(&png(1600, 900, 128), 360).unwrap();
            assert_eq!(image::load_from_memory(&clear).unwrap().width(), 360);
            assert_eq!(mime, "image/png");
            assert!(scaled(&png(300, 200, 255), 360).is_none());
            assert!(scaled(b"not a picture", 360).is_none());
        }
    }
}

#[allow(dead_code)]
fn kind_is_image(asset: &MediaAssetV1) -> bool {
    asset.kind == MediaKind::Image
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn import_read_stops_when_a_file_outgrows_its_admitted_size() {
        assert_eq!(read_import_bytes(&b"data"[..], 4).unwrap(), b"data");
        assert!(matches!(
            read_import_bytes(std::io::repeat(b'x'), 4),
            Err(ResourceError::Capacity)
        ));
        assert!(read_import_bytes(std::io::empty(), 4).unwrap().is_empty());
    }

    #[test]
    fn image_sniffing_recognizes_only_admitted_raster_formats() {
        assert_eq!(sniff_image(b"\x89PNG\r\n\x1a\nrest"), Some("image/png"));
        assert_eq!(sniff_image(&[0xFF, 0xD8, 0xFF, 0xE0]), Some("image/jpeg"));
        assert_eq!(sniff_image(b"GIF89a...."), Some("image/gif"));
        assert_eq!(
            sniff_image(b"RIFF\x00\x00\x00\x00WEBPVP8 "),
            Some("image/webp")
        );
        assert_eq!(
            sniff_image(b"<svg xmlns='http://www.w3.org/2000/svg'/>"),
            None
        );
        assert_eq!(sniff_image(b"%PDF-1.7"), None);
        assert_eq!(sniff_image(b""), None);
    }
}

fn civil_date_today() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let days = (secs / 86_400) as i64;
    // Howard Hinnant's civil-from-days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}")
}

fn image_name(url: &tauri::Url) -> String {
    url.path_segments()
        .and_then(|mut segments| segments.rfind(|s| !s.is_empty()).map(str::to_owned))
        .filter(|name| name.len() <= 120)
        .unwrap_or_else(|| "image".into())
}

#[cfg(feature = "work-product")]
async fn environment_snapshot(
    shell: &zephium_app::Handle,
    profile: ProfileId,
    id: zephium_core::work::WorkEnvironmentId,
) -> Result<zephium_core::work::environment::WorkEnvironmentSnapshot, ResourceError> {
    use zephium_core::work::{environment::*, port::*};
    let request = shell
        .work_call(
            profile,
            zephium_ipc::work::WorkCallV1::Environment {
                version: 1,
                request: WorkEnvironmentCall::Read { id },
            },
        )
        .map_err(|_| ResourceError::Unavailable)?;
    let projection = tokio::time::timeout(Duration::from_secs(8), request)
        .await
        .map_err(|_| ResourceError::Unavailable)?
        .map_err(|_| ResourceError::NotFound)?;
    match projection.reply {
        WorkReply::Environment(WorkEnvironmentReply::Snapshot { snapshot })
            if projection.profile == profile =>
        {
            Ok(*snapshot)
        }
        _ => Err(ResourceError::NotFound),
    }
}

#[cfg(feature = "work-product")]
async fn environment_edit(
    shell: &zephium_app::Handle,
    profile: ProfileId,
    id: zephium_core::work::WorkEnvironmentId,
    expected: zephium_core::work::WorkRevision,
    edit: zephium_core::work::environment::WorkEnvironmentEdit,
) -> Result<zephium_core::work::environment::WorkEnvironmentSnapshot, ResourceError> {
    use zephium_core::work::{environment::*, port::*, WorkCommandId};
    let request = shell
        .work_call(
            profile,
            zephium_ipc::work::WorkCallV1::Environment {
                version: 1,
                request: WorkEnvironmentCall::Command {
                    command: WorkCommandId::generate(),
                    intent: WorkEnvironmentIntent::Edit { id, expected, edit },
                },
            },
        )
        .map_err(|_| ResourceError::Unavailable)?;
    let projection = tokio::time::timeout(Duration::from_secs(8), request)
        .await
        .map_err(|_| ResourceError::Unavailable)?
        .map_err(|_| ResourceError::Conflict)?;
    match projection.reply {
        WorkReply::Environment(WorkEnvironmentReply::Applied { snapshot, .. })
            if projection.profile == profile =>
        {
            Ok(*snapshot)
        }
        _ => Err(ResourceError::Conflict),
    }
}

/// Admits one public image for a subject already on the canvas: fetch
/// without cookies, bound and decode in the store, mint the Media resource,
/// add it next to the subject, and relate subject → media.
/// Admits a folder the person dropped or chose so the canvas can hold it
/// and later runs can read inside it. The same policy governs the run.
#[tauri::command]
#[specta::specta]
pub(crate) async fn work_admit_folder(
    caller: WebviewWindow,
    app: AppHandle,
    expected_profile: String,
    path: String,
) -> Result<zephium_ipc::WorkFolderAdmitV1, ()> {
    use zephium_ipc::WorkFolderAdmitV1;
    if !super::authorize(&caller, super::CallerPolicy::Main, "work_admit_folder")
        || super::shutdown_started(&app)
        || profile_of(&expected_profile).is_none()
    {
        return Ok(WorkFolderAdmitV1::Refused {
            not_a_folder: false,
        });
    }
    Ok(admit_folder(path))
}

/// Opens the folder picker and admits the choice under the run's policy.
#[tauri::command]
#[specta::specta]
pub(crate) async fn work_pick_folder(
    caller: WebviewWindow,
    app: AppHandle,
    expected_profile: String,
) -> Result<Option<zephium_ipc::WorkFolderAdmitV1>, ()> {
    if !super::authorize(&caller, super::CallerPolicy::Main, "work_pick_folder")
        || super::shutdown_started(&app)
        || profile_of(&expected_profile).is_none()
    {
        return Ok(None);
    }
    let picker = app.clone();
    let picked = tokio::task::spawn_blocking(move || {
        picker
            .dialog()
            .file()
            .set_title("Add a folder to Work")
            .blocking_pick_folder()
    })
    .await
    .map_err(|_| ())?;
    let Some(picked) = picked else {
        return Ok(None);
    };
    let path = match picked {
        tauri_plugin_dialog::FilePath::Path(path) => path,
        tauri_plugin_dialog::FilePath::Url(url) => match url.to_file_path() {
            Ok(path) => path,
            Err(()) => return Ok(None),
        },
    };
    Ok(Some(admit_folder(path.to_string_lossy().into_owned())))
}

/// Reveals an admitted folder, or a file inside one, in the native file manager.
#[tauri::command]
#[specta::specta]
pub(crate) async fn work_reveal_path(
    caller: WebviewWindow,
    app: AppHandle,
    expected_profile: String,
    path: String,
) -> Result<bool, ()> {
    if !super::authorize(&caller, super::CallerPolicy::Main, "work_reveal_path")
        || super::shutdown_started(&app)
        || profile_of(&expected_profile).is_none()
    {
        return Ok(false);
    }
    #[cfg(not(all(
        feature = "work-product",
        any(target_os = "macos", target_os = "windows")
    )))]
    {
        let _ = path;
        Ok(false)
    }
    #[cfg(all(
        feature = "work-product",
        any(target_os = "macos", target_os = "windows")
    ))]
    {
        let target = std::path::PathBuf::from(&path);
        let folder = if target.is_dir() {
            target.clone()
        } else {
            match target.parent() {
                Some(parent) => parent.to_path_buf(),
                None => return Ok(false),
            }
        };
        let (grant, _) =
            zephium_app::work_files::WorkFileGrant::admit(&[folder.to_string_lossy().into_owned()]);
        if grant.is_empty() {
            return Ok(false);
        }
        #[cfg(target_os = "windows")]
        {
            tokio::task::spawn_blocking(move || windows_shell_path(&target, true))
                .await
                .map_err(|_| ())
        }
        #[cfg(target_os = "macos")]
        Ok(std::process::Command::new("/usr/bin/open")
            .arg("-R")
            .arg(&target)
            .spawn()
            .is_ok())
    }
}

fn admit_folder(path: String) -> zephium_ipc::WorkFolderAdmitV1 {
    use zephium_ipc::WorkFolderAdmitV1;
    let not_a_folder = std::path::Path::new(&path).is_file();
    #[cfg(not(feature = "work-product"))]
    {
        WorkFolderAdmitV1::Refused { not_a_folder }
    }
    #[cfg(feature = "work-product")]
    {
        let (grant, _) = zephium_app::work_files::WorkFileGrant::admit(&[path]);
        let Some(root) = grant.roots().first() else {
            return WorkFolderAdmitV1::Refused { not_a_folder };
        };
        let name = root
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        if name.is_empty() {
            return WorkFolderAdmitV1::Refused { not_a_folder };
        }
        WorkFolderAdmitV1::Admitted {
            path: root.to_string_lossy().into_owned(),
            name,
        }
    }
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn media_admit_remote(
    caller: WebviewWindow,
    app: AppHandle,
    shell: State<'_, zephium_app::Handle>,
    expected_profile: String,
    environment: String,
    element: String,
    url: String,
) -> Result<zephium_ipc::MediaAdmitV1, ()> {
    use zephium_ipc::MediaAdmitV1;
    let refused = |error| Ok(MediaAdmitV1::Refused { error });
    if !super::authorize(&caller, super::CallerPolicy::Main, "media_admit_remote")
        || super::shutdown_started(&app)
    {
        return refused(ResourceError::Unavailable);
    }
    let Some(profile) = profile_of(&expected_profile) else {
        return refused(ResourceError::Invalid);
    };
    let started = std::time::Instant::now();
    let mut trace = MediaAdmitTrace::default();
    let result = admit_remote(&app, &shell, profile, environment, element, url, &mut trace).await;
    let outcome = match &result {
        Ok(MediaAdmitV1::Admitted { .. }) => "admitted",
        Ok(MediaAdmitV1::Refused { .. }) => "refused",
        Err(()) => "unknown",
    };
    let error = match &result {
        Ok(MediaAdmitV1::Refused { error }) => Some(*error),
        _ => None,
    };
    super::work_provider::record_diagnostic(format_args!(
        "work: phase=media_admit outcome={outcome} error={error:?} fetch={:?} bytes={} elapsed_ms={}",
        trace.fetch,
        trace.bytes,
        started.elapsed().as_millis()
    ));
    result
}

/// Closed facts about one admission, never the URL.
#[derive(Default)]
struct MediaAdmitTrace {
    bytes: usize,
    #[cfg(feature = "work-product")]
    fetch: Option<zephium_agentic::public_asset::PublicAssetError>,
    #[cfg(not(feature = "work-product"))]
    fetch: Option<()>,
}

async fn admit_remote(
    app: &AppHandle,
    shell: &zephium_app::Handle,
    profile: ProfileId,
    environment: String,
    element: String,
    url: String,
    trace: &mut MediaAdmitTrace,
) -> Result<zephium_ipc::MediaAdmitV1, ()> {
    use zephium_ipc::MediaAdmitV1;
    let refused = |error| Ok(MediaAdmitV1::Refused { error });
    #[cfg(not(feature = "work-product"))]
    {
        let _ = (app, shell, profile, environment, element, url, trace);
        refused(ResourceError::Unavailable)
    }
    #[cfg(feature = "work-product")]
    {
        use zephium_core::work::environment::*;
        use zephium_core::work::{WorkElementId, WorkEnvironmentId};
        let (Some(environment), Some(element)) = (
            WorkEnvironmentId::parse(&environment),
            WorkElementId::parse(&element),
        ) else {
            return refused(ResourceError::Invalid);
        };
        let Ok(parsed) = tauri::Url::parse(&url) else {
            return refused(ResourceError::Invalid);
        };
        if !zephium_agentic::public_asset::public_https(&parsed) {
            return refused(ResourceError::Invalid);
        }
        let snapshot = match environment_snapshot(shell, profile, environment).await {
            Ok(snapshot) => snapshot,
            Err(error) => return refused(error),
        };
        let Some(subject) = snapshot
            .elements
            .iter()
            .find(|candidate| candidate.id == element)
        else {
            return refused(ResourceError::NotFound);
        };
        if !remote_image_target(&subject.reference) {
            return refused(ResourceError::Invalid);
        }
        let area = subject.area;
        let bytes = match zephium_agentic::public_asset::fetch_public_image(parsed.as_str()).await {
            Ok(bytes) => bytes,
            Err(error) => {
                trace.fetch = Some(error);
                return refused(match error {
                    zephium_agentic::public_asset::PublicAssetError::TooLarge => {
                        ResourceError::Capacity
                    }
                    _ => ResourceError::Unavailable,
                });
            }
        };
        let bytes = match tokio::task::spawn_blocking(move || fit_for_display(bytes)).await {
            Ok(Some(bytes)) => bytes,
            _ => return refused(ResourceError::Capacity),
        };
        trace.bytes = bytes.len();
        let receiver = shell.import_media(
            profile,
            MediaImport {
                request_id: format!("media-admit-{}", ResourceId::generate()),
                name: image_name(&parsed),
                origin: MediaOrigin::Fetched {
                    url: parsed.to_string(),
                    observed_at: civil_date_today(),
                },
                bytes: std::sync::Arc::new(bytes),
            },
        );
        let reply =
            tokio::task::spawn_blocking(move || receiver.recv_timeout(Duration::from_secs(30)))
                .await
                .map_err(|_| ())?;
        let record = match reply.map(|reply| reply.response) {
            Ok(ResourceResponse::Applied { record, .. }) => record,
            Ok(ResourceResponse::Error { error }) => return refused(error),
            _ => return refused(ResourceError::OutcomeUnknown),
        };
        super::emit_media_changed(app, &profile.to_string(), &record.id, &record.revision);
        let Some(resource) = ResourceId::parse(&record.id) else {
            return refused(ResourceError::Invalid);
        };
        let reference = WorkEnvironmentReference::Resource { resource };
        let current = match snapshot
            .elements
            .iter()
            .find(|candidate| candidate.reference == reference)
        {
            Some(existing) => (snapshot.clone(), existing.id),
            None => {
                let added = match environment_edit(
                    shell,
                    profile,
                    environment,
                    snapshot.revision,
                    WorkEnvironmentEdit::Add {
                        reference: reference.clone(),
                        area,
                    },
                )
                .await
                {
                    Ok(added) => added,
                    Err(error) => return refused(error),
                };
                let Some(media) = added
                    .elements
                    .iter()
                    .find(|candidate| candidate.reference == reference)
                else {
                    return refused(ResourceError::OutcomeUnknown);
                };
                let id = media.id;
                (added, id)
            }
        };
        let (snapshot, media_element) = current;
        let related = snapshot.relations.iter().any(|relation| {
            relation.from == element
                && relation.to == media_element
                && relation.kind == WorkRelationKind::Uses
        });
        if !related
            && environment_edit(
                shell,
                profile,
                environment,
                snapshot.revision,
                WorkEnvironmentEdit::Relate {
                    from: element,
                    to: media_element,
                    relation: WorkRelationKind::Uses,
                },
            )
            .await
            .is_err()
        {
            return refused(ResourceError::Conflict);
        }
        Ok(MediaAdmitV1::Admitted {
            element: media_element,
        })
    }
}

/// A fetched picture as the canvas shows it: at most 1600 px on its long
/// edge and within the store's fetched-image limit. A picture already that
/// size passes unchanged; a larger one is scaled down and stored as JPEG, or
/// as PNG when it has transparency. None when it cannot be made to fit.
#[cfg(feature = "work-product")]
fn fit_for_display(bytes: Vec<u8>) -> Option<Vec<u8>> {
    use image::{ImageFormat, ImageReader};
    use std::io::Cursor;
    const EDGE: u32 = zephium_agentic::public_asset::DISPLAY_EDGE;
    let limit = zephium_core::resources::MAX_MEDIA_FETCHED_IMAGE_BYTES as usize;
    let format = image::guess_format(&bytes).ok()?;
    let mut reader = ImageReader::with_format(Cursor::new(bytes.as_slice()), format);
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(zephium_core::resources::MAX_MEDIA_DIMENSION);
    limits.max_image_height = Some(zephium_core::resources::MAX_MEDIA_DIMENSION);
    limits.max_alloc = Some(256 * 1024 * 1024);
    reader.limits(limits.clone());
    let (width, height) = reader.into_dimensions().ok()?;
    if width.max(height) <= EDGE && bytes.len() <= limit {
        return Some(bytes);
    }
    if format == ImageFormat::Gif {
        return None;
    }
    let mut reader = ImageReader::with_format(Cursor::new(bytes.as_slice()), format);
    reader.limits(limits);
    let decoded = reader.decode().ok()?;
    let alpha =
        decoded.color().has_alpha() && decoded.to_rgba8().pixels().any(|pixel| pixel.0[3] < 255);
    for edge in [EDGE, 1200, 900] {
        let scaled = if decoded.width().max(decoded.height()) > edge {
            decoded.resize(edge, edge, image::imageops::FilterType::CatmullRom)
        } else {
            decoded.clone()
        };
        let mut out = Vec::new();
        let written = if alpha {
            scaled
                .to_rgba8()
                .write_to(&mut Cursor::new(&mut out), ImageFormat::Png)
        } else {
            image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 84)
                .encode_image(&scaled.to_rgb8())
        };
        if written.is_ok() && !out.is_empty() && out.len() <= limit {
            return Some(out);
        }
    }
    None
}

#[cfg(feature = "work-product")]
fn remote_image_target(
    reference: &zephium_core::work::environment::WorkEnvironmentReference,
) -> bool {
    use zephium_core::work::environment::WorkEnvironmentReference;
    // A lead's picks object is one artifact element whose items each name
    // their own pictures; each admitted picture relates to that element.
    matches!(
        reference,
        WorkEnvironmentReference::Subject { .. }
            | WorkEnvironmentReference::Link { .. }
            | WorkEnvironmentReference::Artifact { .. }
    )
}

#[cfg(all(test, feature = "work-product"))]
mod display_tests {
    use super::fit_for_display;

    fn png(width: u32, height: u32, alpha: u8) -> Vec<u8> {
        let image = image::RgbaImage::from_fn(width, height, |x, y| {
            image::Rgba([(x / 13) as u8, (y / 10) as u8, 120, alpha])
        });
        let mut bytes = Vec::new();
        image::DynamicImage::ImageRgba8(image)
            .write_to(
                &mut std::io::Cursor::new(&mut bytes),
                image::ImageFormat::Png,
            )
            .unwrap();
        bytes
    }

    #[test]
    fn a_large_product_photo_is_scaled_to_display_size_instead_of_refused() {
        let small = png(800, 600, 255);
        assert_eq!(fit_for_display(small.clone()), Some(small));
        for alpha in [255, 128] {
            let fitted = fit_for_display(png(3200, 2400, alpha)).unwrap();
            let decoded = image::load_from_memory(&fitted).unwrap();
            assert_eq!((decoded.width(), decoded.height()), (1600, 1200));
            assert!(
                fitted.len() <= zephium_core::resources::MAX_MEDIA_FETCHED_IMAGE_BYTES as usize
            );
            assert_eq!(
                image::guess_format(&fitted).unwrap(),
                if alpha == 255 {
                    image::ImageFormat::Jpeg
                } else {
                    image::ImageFormat::Png
                }
            );
        }
    }
}

#[cfg(test)]
mod date_tests {
    #[cfg(feature = "work-product")]
    #[test]
    fn remote_thumbnails_accept_subjects_links_and_placed_objects_only() {
        use zephium_core::work::environment::WorkEnvironmentReference;
        assert!(super::remote_image_target(
            &WorkEnvironmentReference::Link {
                url: "https://example.com/video".into(),
                title: "example.com".into(),
            }
        ));
        assert!(super::remote_image_target(
            &WorkEnvironmentReference::Subject {
                objective: 1.into(),
                execution: 2.into(),
                artifact: 3.into(),
                index: 0,
            }
        ));
        assert!(super::remote_image_target(
            &WorkEnvironmentReference::Artifact {
                objective: 1.into(),
                execution: 2.into(),
                artifact: 3.into(),
            }
        ));
        assert!(!super::remote_image_target(
            &WorkEnvironmentReference::Resource { resource: 4.into() }
        ));
    }
    #[test]
    fn civil_date_is_iso_shaped() {
        let today = super::civil_date_today();
        assert_eq!(today.len(), 10);
        assert!(today.starts_with("20"));
        assert_eq!(&today[4..5], "-");
        assert_eq!(&today[7..8], "-");
    }
}
