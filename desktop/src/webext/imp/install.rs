//! Installing from the Chrome Web Store or from a file: download or read,
//! verification, staging and the user's confirmation.

use std::path::{Path, PathBuf};
use std::time::Duration;

use zephium_app::Handle;
use zephium_core::ids::{ExtensionInstallId, ItemId, ProfileId};
use zephium_webext::manifest::Manifest;
use zephium_webext::{archive, crx, permissions, store, ExtensionId};

use super::{compat_revision, prepare_package, target, MAX_PACKAGE_BYTES};
use crate::webext::{Entry, Original, Pending, WebExtensionReview, WebExtensions};

/// A client that talks only to Google's update and download hosts, where
/// the Web Store serves packages from.
pub(super) fn store_client() -> Result<reqwest::Client, String> {
    let policy = reqwest::redirect::Policy::custom(|attempt| {
        let google = attempt.url().host_str().is_some_and(|host| {
            ["google.com", "googleusercontent.com", "gvt1.com"]
                .iter()
                .any(|domain| host == *domain || host.ends_with(&format!(".{domain}")))
        });
        if attempt.previous().len() >= 5 || !google {
            attempt.stop()
        } else {
            attempt.follow()
        }
    });
    reqwest::Client::builder()
        .https_only(true)
        .redirect(policy)
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(90))
        .build()
        .map_err(|_| "Downloads are unavailable.".to_string())
}

pub(super) async fn download(id: &ExtensionId) -> Result<Vec<u8>, String> {
    let client = store_client()?;
    let url = store::download_url(id, store::CHROME_VERSION);
    let response = client
        .get(url)
        .send()
        .await
        .map_err(|_| "The Chrome Web Store could not be reached.")?;
    if !response.status().is_success() {
        return Err(format!(
            "The Chrome Web Store answered {}.",
            response.status()
        ));
    }
    super::response::read_body(
        response,
        MAX_PACKAGE_BYTES,
        "The extension is too large.",
        "The download was interrupted.",
    )
    .await
}

fn icon_data_url(dir: &Path, manifest: &Manifest) -> Option<String> {
    use base64::Engine as _;
    let path = manifest.icons().best(48)?.to_owned();
    let bytes = std::fs::read(dir.join(&path))
        .ok()
        .filter(|bytes| bytes.len() < 512 * 1024)?;
    let mime = if path.ends_with(".svg") {
        "image/svg+xml"
    } else if path.ends_with(".jpg") || path.ends_with(".jpeg") {
        "image/jpeg"
    } else {
        "image/png"
    };
    Some(format!(
        "data:{mime};base64,{}",
        base64::engine::general_purpose::STANDARD.encode(bytes)
    ))
}

pub(in crate::webext) async fn prepare(
    shell: &Handle,
    extensions: &WebExtensions,
    tab_id: &str,
) -> Result<WebExtensionReview, String> {
    let tab = ItemId::parse(tab_id).ok_or("Invalid tab.")?;
    let target = target(shell, Some(tab)).await?;
    let listing = target
        .listing_url
        .ok_or("Open an extension's Chrome Web Store page first.")?;
    let id = store::listing_id(&listing).ok_or("This isn't an extension page.")?;
    let bytes = download(&id).await?;
    review(extensions, target.profile, Source::Store(id, bytes)).await
}

/// Reviews an extension file or unpacked folder.
pub(in crate::webext) async fn prepare_file(
    shell: &Handle,
    extensions: &WebExtensions,
    path: PathBuf,
) -> Result<WebExtensionReview, String> {
    let profile = target(shell, None).await?.profile;
    review(extensions, profile, Source::File(path)).await
}

pub(in crate::webext) async fn choose_file(
    app: &tauri::AppHandle,
    shell: &Handle,
    extensions: &WebExtensions,
    folder: bool,
) -> Result<Option<WebExtensionReview>, String> {
    use tauri_plugin_dialog::DialogExt;
    let dialog = app.dialog().file();
    let picked = tauri::async_runtime::spawn_blocking(move || {
        if folder {
            dialog.blocking_pick_folder()
        } else {
            dialog
                .add_filter("Chrome extension", &["crx", "zip"])
                .blocking_pick_file()
        }
    })
    .await
    .map_err(|_| "The file couldn't be chosen.")?;
    let Some(path) = picked.and_then(|path| path.into_path().ok()) else {
        return Ok(None);
    };
    prepare_file(shell, extensions, path).await.map(Some)
}

/// Reviews the store's newest version of an extension whose update waits
/// for approval because it asks for more access.
pub(in crate::webext) async fn review_update(
    shell: &Handle,
    extensions: &WebExtensions,
    id: &str,
) -> Result<WebExtensionReview, String> {
    let profile = target(shell, None).await?.profile;
    let id = ExtensionId::parse(id).ok_or("That extension isn't installed.")?;
    let bytes = download(&id).await?;
    review(extensions, profile, Source::Store(id, bytes)).await
}

async fn review(
    extensions: &WebExtensions,
    profile: ProfileId,
    source: Source,
) -> Result<WebExtensionReview, String> {
    let root = extensions.root.clone();
    let installed = extensions.registry(profile).extensions;
    #[cfg(target_os = "windows")]
    let enabled = extensions
        .profiles()
        .into_iter()
        .map(|owner| {
            extensions
                .registry(owner)
                .extensions
                .iter()
                .filter(|entry| entry.enabled)
                .count()
        })
        .sum::<usize>();
    #[cfg(target_os = "windows")]
    let enabled_ids: Vec<_> = installed
        .iter()
        .filter(|entry| entry.enabled)
        .map(|entry| entry.id.clone())
        .collect();
    let staged =
        tauri::async_runtime::spawn_blocking(move || stage(&root, source, profile, &installed))
            .await
            .map_err(|_| "Installation failed.")??;
    let review = staged.review.clone();
    #[cfg(target_os = "windows")]
    let review = {
        let mut review = review;
        if enabled >= zephium_core::extensions::MAX_EXTENSION_INSTALLS_PER_PROFILE
            && !enabled_ids.contains(&review.id)
        {
            review.warnings.push("Windows can run eight extensions at once across all profiles. This extension will wait until you disable or remove another running extension.".into());
        }
        review
    };
    let mut pending = extensions
        .state
        .lock()
        .map_err(|_| "Installation failed.")?;
    if let Some(previous) = pending.replace(staged.pending) {
        let _ = std::fs::remove_dir_all(previous.staged);
    }
    Ok(review)
}

/// Where a package comes from.
pub(super) enum Source {
    /// A download from the Chrome Web Store, which must be signed for `id`.
    Store(ExtensionId, Vec<u8>),
    /// A `.crx` or `.zip` file, or an unpacked folder, the user chose.
    File(PathBuf),
}

pub(super) struct Staged {
    pub(super) review: WebExtensionReview,
    pub(super) pending: Pending,
}

/// Verifies and unpacks a package into the staging area, prepares it for
/// WebKit and describes it for review; `installed` are the profile's
/// extensions, one of which it may update.
pub(super) fn stage(
    root: &Path,
    source: Source,
    profile: ProfileId,
    installed: &[Entry],
) -> Result<Staged, String> {
    let staging = root.join("staging");
    std::fs::create_dir_all(&staging).map_err(|e| e.to_string())?;
    let dir = staging.join(ExtensionInstallId::generate().to_string());
    let from_file = matches!(source, Source::File(_));
    let result = (|| {
        let (id, original) = unpack(source, &dir)?;
        let existing = installed.iter().find(|entry| entry.id == id.as_str());
        // An unsigned package takes its identity from a manifest key anyone
        // can copy. It may update a file install, never a store extension,
        // whose access, data and native-app trust it would otherwise inherit.
        if matches!(original, Original::Zip(_) | Original::Folder(_))
            && existing.is_some_and(|entry| !entry.sideloaded)
        {
            return Err("An extension from the Chrome Web Store with this identity is already installed. Remove it first to install this file.".into());
        }
        let manifest = Manifest::load(&dir)
            .map_err(|error| format!("The extension's manifest is invalid ({error})."))?;
        let warnings = permissions::warnings(&manifest);
        let icon = icon_data_url(&dir, &manifest);
        let mut hosts = manifest.host_permissions();
        for script in manifest.content_scripts() {
            hosts.extend(script.matches);
        }
        hosts.sort();
        hosts.dedup();
        let version = manifest
            .version()
            .ok_or("The extension has no valid version.")?
            .to_owned();
        let name = manifest.name().unwrap_or_else(|| id.to_string());
        let description = manifest.description().unwrap_or_default().to_owned();
        let access = existing
            .map(|entry| entry.access.clone())
            .unwrap_or_default();
        let report = prepare_package(&dir, &access)
            .map_err(|error| format!("The extension can't be prepared ({error})."))?;
        let mut permissions = manifest.permissions();
        permissions.extend(report.added_permissions);
        let revision = compat_revision(&access);
        let entry = Entry {
            install: existing
                .map(|entry| entry.install.clone())
                .unwrap_or_else(|| ExtensionInstallId::generate().to_string()),
            id: id.to_string(),
            name: name.clone(),
            version: version.clone(),
            description: description.clone(),
            enabled: true,
            package: format!("{version}-{revision}"),
            compat: revision,
            started: String::new(),
            permissions,
            hosts,
            icon: icon.clone(),
            held_update: None,
            access,
            sideloaded: from_file,
        };
        Ok(Staged {
            review: WebExtensionReview {
                id: id.to_string(),
                name,
                version,
                description,
                warnings,
                icon,
                update: existing.is_some(),
                from_file,
            },
            pending: Pending {
                profile,
                entry,
                staged: dir.clone(),
                original,
            },
        })
    })();
    if result.is_err() {
        let _ = std::fs::remove_dir_all(&dir);
    }
    result
}

/// Unpacks `source` into `dir` and identifies it: store and `.crx` packages
/// by their signature, others by their manifest's `key`, or else by where
/// they came from, as Chrome identifies unpacked extensions.
fn unpack(source: Source, dir: &Path) -> Result<(ExtensionId, Original), String> {
    let limits = archive::Limits::default();
    let unsafe_package =
        |error: archive::ArchiveError| format!("The package can't be unpacked safely ({error}).");
    match source {
        Source::Store(id, bytes) => {
            let verified = crx::verify(&bytes, Some(&id))
                .map_err(|_| "The package isn't correctly signed by its publisher.")?;
            archive::extract(verified.zip, dir, &limits).map_err(unsafe_package)?;
            super::platform::record_signed_key(dir, verified.public_key)?;
            Ok((id, Original::Crx(bytes)))
        }
        Source::File(path) => {
            let path = path
                .canonicalize()
                .map_err(|_| "That file can't be read.")?;
            if path.is_dir() {
                archive::copy_dir(&path, dir, &limits).map_err(unsafe_package)?;
                return Ok((unsigned_id(dir, &path)?, Original::Folder(path)));
            }
            let bytes = read_package(&path)?;
            if bytes.starts_with(b"Cr24") {
                let verified =
                    crx::verify(&bytes, None).map_err(|_| "The package isn't correctly signed.")?;
                let id = verified.id.clone();
                archive::extract(verified.zip, dir, &limits).map_err(unsafe_package)?;
                super::platform::record_signed_key(dir, verified.public_key)?;
                Ok((id, Original::Crx(bytes)))
            } else {
                archive::extract(&bytes, dir, &limits).map_err(unsafe_package)?;
                Ok((unsigned_id(dir, &path)?, Original::Zip(bytes)))
            }
        }
    }
}

fn read_package(path: &Path) -> Result<Vec<u8>, String> {
    let size = std::fs::metadata(path)
        .map_err(|_| "That file can't be read.")?
        .len();
    if size > MAX_PACKAGE_BYTES {
        return Err("The extension is too large.".into());
    }
    std::fs::read(path).map_err(|_| "That file can't be read.".into())
}

fn unsigned_id(dir: &Path, source: &Path) -> Result<ExtensionId, String> {
    use base64::Engine as _;
    let manifest = Manifest::load(dir)
        .map_err(|error| format!("The extension's manifest is invalid ({error})."))?;
    let key = manifest
        .raw()
        .get("key")
        .and_then(|key| key.as_str())
        .and_then(|key| {
            base64::engine::general_purpose::STANDARD
                .decode(key.trim())
                .ok()
        });
    Ok(match key {
        Some(key) => ExtensionId::from_public_key(&key),
        None if cfg!(target_os = "windows") => return Err("On Windows, use a signed CRX or an unpacked extension with a manifest key. A stable native identity is required.".into()),
        None => ExtensionId::from_source_path(&source.to_string_lossy()),
    })
}

pub(in crate::webext) fn confirm(
    shell: &Handle,
    extensions: &WebExtensions,
    id: &str,
) -> Result<(), String> {
    let pending = extensions
        .state
        .lock()
        .map_err(|_| "Installation failed.")?
        .take()
        .filter(|pending| pending.entry.id == id)
        .ok_or("Nothing is waiting to be installed.")?;
    let packages = extensions.packages(id);
    std::fs::create_dir_all(&packages).map_err(|e| e.to_string())?;
    keep_original(&packages, &pending.entry.version, &pending.original)?;
    let target = packages.join(&pending.entry.package);
    let _ = std::fs::remove_dir_all(&target);
    std::fs::rename(&pending.staged, &target).map_err(|e| e.to_string())?;

    let mut registry = extensions.registry(pending.profile);
    registry.extensions.retain(|entry| entry.id != id);
    registry.extensions.push(pending.entry);
    extensions.save(pending.profile, &registry)?;
    extensions.apply(shell, pending.profile, &mut registry);
    Ok(())
}

/// Stores what a package was installed from, so it can be rebuilt when the
/// compatibility layer changes.
pub(super) fn keep_original(
    packages: &Path,
    version: &str,
    original: &Original,
) -> Result<(), String> {
    match original {
        Original::Crx(bytes) => std::fs::write(packages.join(format!("{version}.crx")), bytes),
        Original::Zip(bytes) => std::fs::write(packages.join(format!("{version}.zip")), bytes),
        Original::Folder(path) => {
            let copy = packages.join(format!("{version}.src"));
            let _ = std::fs::remove_dir_all(&copy);
            archive::copy_dir(path, &copy, &archive::Limits::default())
                .map(|_| ())
                .map_err(|error| std::io::Error::other(error.to_string()))
        }
    }
    .map_err(|e| e.to_string())
}
