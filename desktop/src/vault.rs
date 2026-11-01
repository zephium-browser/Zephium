//! The vault: the person's own folder (often in OneDrive) managed as a file
//! explorer. Rust owns every path decision; the chrome only names
//! vault-relative paths and receives typed answers.
use std::borrow::Cow;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use base64::Engine as _;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, WebviewWindow};
use tauri_plugin_dialog::DialogExt;
use tauri_specta::Event;
use zephium_core::ids::ProfileId;
use zephium_core::vault::{
    VaultCall, VaultError, VaultResponse, MAX_VAULT_READ_BYTES, MAX_VAULT_WRITE_BYTES,
};
use zephium_vault::{NameIndex, Vault, VaultWatcher};

use crate::{authorize, emit_to_privileged, shutdown_started, CallerPolicy, MAIN_LABEL};

pub(crate) const SCHEME: &str = "zephium-vault";

/// Folders whose listing changed. An empty `dirs` means reload everything.
#[derive(Clone, Debug, Serialize, Deserialize, specta::Type, Event)]
pub(crate) struct VaultChanged {
    pub profile: String,
    pub dirs: Vec<String>,
}

struct Open {
    vault: Vault,
    names: Arc<Mutex<NameIndex>>,
    _watcher: Option<VaultWatcher>,
}

pub(crate) struct VaultState {
    data_dir: PathBuf,
    open: Mutex<HashMap<ProfileId, Arc<Open>>>,
    notes: Mutex<Option<Arc<zephium_notes::NoteService>>>,
}

impl VaultState {
    pub(crate) fn new(data_dir: &Path) -> Self {
        Self {
            data_dir: data_dir.to_path_buf(),
            open: Mutex::new(HashMap::new()),
            notes: Mutex::new(None),
        }
    }

    pub(crate) fn attach_notes(&self, notes: Arc<zephium_notes::NoteService>) {
        *self.notes.lock().unwrap_or_else(|e| e.into_inner()) = Some(notes);
    }

    fn pointer(&self, profile: ProfileId) -> PathBuf {
        self.data_dir.join("vault").join(format!("{profile}.root"))
    }

    fn root(&self, profile: ProfileId) -> Option<PathBuf> {
        zephium_notes::vault_root(&self.data_dir, &profile.to_string())
    }

    fn get(&self, app: &AppHandle, profile: ProfileId) -> Result<Arc<Open>, VaultError> {
        let mut open = self.open.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(existing) = open.get(&profile) {
            return Ok(existing.clone());
        }
        let root = self.root(profile).ok_or(VaultError::NoRoot)?;
        let vault = Vault::open(&root)?;
        let names = Arc::new(Mutex::new(NameIndex::default()));
        let watcher = {
            let app = app.clone();
            let names = names.clone();
            zephium_vault::watch(vault.clone(), move |dirs| {
                names.lock().unwrap_or_else(|e| e.into_inner()).invalidate();
                emit_to_privileged(
                    &app,
                    MAIN_LABEL,
                    "zephium:vault-changed",
                    &VaultChanged {
                        profile: profile.to_string(),
                        dirs,
                    },
                );
            })
            .ok()
        };
        let entry = Arc::new(Open {
            vault,
            names,
            _watcher: watcher,
        });
        open.insert(profile, entry.clone());
        Ok(entry)
    }

    fn set_root(&self, profile: ProfileId, root: &Path) -> Result<(), VaultError> {
        Vault::open(root)?;
        let pointer = self.pointer(profile);
        if let Some(parent) = pointer.parent() {
            std::fs::create_dir_all(parent).map_err(|_| VaultError::Io)?;
        }
        let text = root.to_str().ok_or(VaultError::Invalid)?;
        std::fs::write(&pointer, text).map_err(|_| VaultError::Io)?;
        self.open
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&profile);
        if let Some(notes) = self
            .notes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
        {
            notes.reopen(profile);
        }
        Ok(())
    }
}

/// A suggested default when no vault is chosen yet: the person's OneDrive
/// folder on Windows, else Documents.
fn suggested_root() -> Option<PathBuf> {
    std::env::var_os("OneDrive")
        .or_else(|| std::env::var_os("OneDriveConsumer"))
        .map(PathBuf::from)
        .filter(|path| path.is_dir())
        .or_else(|| {
            std::env::var_os("USERPROFILE")
                .or_else(|| std::env::var_os("HOME"))
                .map(|home| PathBuf::from(home).join("Documents"))
                .filter(|path| path.is_dir())
        })
}

fn profile_of(value: &str) -> Option<ProfileId> {
    ProfileId::parse(value).filter(|id| id.to_string() == value)
}

fn allowed(caller: &WebviewWindow, app: &AppHandle, command: &str) -> bool {
    authorize(caller, CallerPolicy::Main, command) && !shutdown_started(app)
}

/// Where the profile's vault is, if one is chosen.
#[tauri::command]
#[specta::specta]
pub(crate) fn vault_root(
    caller: WebviewWindow,
    app: AppHandle,
    expected_profile: String,
) -> Option<String> {
    if !allowed(&caller, &app, "vault_root") {
        return None;
    }
    let profile = profile_of(&expected_profile)?;
    let state = app.try_state::<VaultState>()?;
    state
        .root(profile)
        .and_then(|root| root.to_str().map(str::to_owned))
}

/// Asks for a folder and makes it the profile's vault. `Ok(None)` when the
/// person cancelled.
#[tauri::command]
#[specta::specta]
pub(crate) async fn vault_choose_root(
    caller: WebviewWindow,
    app: AppHandle,
    expected_profile: String,
) -> Result<Option<String>, VaultError> {
    if !allowed(&caller, &app, "vault_choose_root") {
        return Err(VaultError::Unavailable);
    }
    let profile = profile_of(&expected_profile).ok_or(VaultError::Invalid)?;
    let picker = app.clone();
    let picked = tokio::task::spawn_blocking(move || {
        let mut dialog = picker.dialog().file().set_title("Choose your vault folder");
        if let Some(start) = suggested_root() {
            dialog = dialog.set_directory(start);
        }
        dialog.blocking_pick_folder()
    })
    .await
    .map_err(|_| VaultError::Unavailable)?;
    let Some(picked) = picked else {
        return Ok(None);
    };
    let root = picked.into_path().map_err(|_| VaultError::Invalid)?;
    let state = app
        .try_state::<VaultState>()
        .ok_or(VaultError::Unavailable)?;
    state.set_root(profile, &root)?;
    Ok(root.to_str().map(str::to_owned))
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn vault_call(
    caller: WebviewWindow,
    app: AppHandle,
    expected_profile: String,
    call: VaultCall,
) -> VaultResponse {
    let failed = |error| VaultResponse::Error { error };
    if !allowed(&caller, &app, "vault_call") {
        return failed(VaultError::Unavailable);
    }
    if !call.validate() {
        return failed(VaultError::Invalid);
    }
    let Some(profile) = profile_of(&expected_profile) else {
        return failed(VaultError::Invalid);
    };
    let Some(state) = app.try_state::<VaultState>() else {
        return failed(VaultError::Unavailable);
    };
    let open = match state.get(&app, profile) {
        Ok(open) => open,
        Err(error) => return failed(error),
    };
    static ADMISSION: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(16);
    let Ok(_permit) = ADMISSION.try_acquire() else {
        return failed(VaultError::Unavailable);
    };
    tokio::task::spawn_blocking(move || run(&open, call))
        .await
        .unwrap_or(VaultResponse::Error {
            error: VaultError::Io,
        })
}

fn run(open: &Open, call: VaultCall) -> VaultResponse {
    let vault = &open.vault;
    let entry = |result: Result<_, VaultError>| match result {
        Ok(entry) => {
            open.names
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .invalidate();
            VaultResponse::Entry { entry }
        }
        Err(error) => VaultResponse::Error { error },
    };
    match call {
        VaultCall::List { dir } => match vault.list(&dir) {
            Ok(entries) => VaultResponse::Entries { entries },
            Err(error) => VaultResponse::Error { error },
        },
        VaultCall::Stat { path } => match vault.stat(&path) {
            Ok(entry) => VaultResponse::Entry { entry },
            Err(error) => VaultResponse::Error { error },
        },
        VaultCall::CreateFolder { dir, name } => entry(vault.create_folder(&dir, &name)),
        VaultCall::CreateFile { dir, name, text } => {
            entry(vault.create_file(&dir, &name, text.as_bytes()))
        }
        VaultCall::Rename { path, name } => entry(vault.rename(&path, &name)),
        VaultCall::Move { path, dest } => entry(vault.move_to(&path, &dest)),
        VaultCall::Trash { path } => entry(vault.trash(&path)),
        VaultCall::Downloads => match recent_downloads() {
            Some(entries) => VaultResponse::Entries { entries },
            None => VaultResponse::Error {
                error: VaultError::NotFound,
            },
        },
        VaultCall::Import { source, dest } => {
            let Some(folder) = dirs::download_dir() else {
                return VaultResponse::Error {
                    error: VaultError::NotFound,
                };
            };
            let source = folder.join(source);
            // Only a regular file directly in Downloads, never a link out of it.
            match std::fs::symlink_metadata(&source) {
                Ok(meta) if meta.is_file() => entry(vault.import(&source, &dest)),
                Ok(_) => VaultResponse::Error {
                    error: VaultError::Invalid,
                },
                Err(_) => VaultResponse::Error {
                    error: VaultError::NotFound,
                },
            }
        }
        VaultCall::Read { path } => match vault.read(&path, MAX_VAULT_READ_BYTES) {
            Ok((bytes, modified)) => VaultResponse::Bytes {
                base64: base64::engine::general_purpose::STANDARD.encode(bytes),
                modified,
            },
            Err(error) => VaultResponse::Error { error },
        },
        VaultCall::Write {
            path,
            base64,
            expected_modified,
        } => {
            let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(base64) else {
                return VaultResponse::Error {
                    error: VaultError::Invalid,
                };
            };
            if bytes.len() > MAX_VAULT_WRITE_BYTES {
                return VaultResponse::Error {
                    error: VaultError::TooLarge,
                };
            }
            match vault.write(&path, &bytes, &expected_modified) {
                Ok(modified) => match vault.stat(&path) {
                    Ok(mut entry) => {
                        entry.modified = modified;
                        VaultResponse::Entry { entry }
                    }
                    Err(error) => VaultResponse::Error { error },
                },
                Err(error) => VaultResponse::Error { error },
            }
        }
        VaultCall::Search {
            query,
            limit,
            folders_only,
        } => {
            let entries = open.names.lock().unwrap_or_else(|e| e.into_inner()).search(
                vault,
                &query,
                usize::from(limit),
                folders_only,
            );
            VaultResponse::Entries { entries }
        }
        VaultCall::OpenNative { path } => match vault.resolve(&path) {
            Ok(absolute) if absolute.exists() => {
                if crate::media::open_with_os(&absolute) {
                    VaultResponse::Done
                } else {
                    VaultResponse::Error {
                        error: VaultError::Io,
                    }
                }
            }
            Ok(_) => VaultResponse::Error {
                error: VaultError::NotFound,
            },
            Err(error) => VaultResponse::Error { error },
        },
        VaultCall::Reveal { path } => match vault.resolve(&path) {
            Ok(absolute) if absolute.exists() => {
                crate::notes::reveal_path(&absolute);
                VaultResponse::Done
            }
            Ok(_) => VaultResponse::Error {
                error: VaultError::NotFound,
            },
            Err(error) => VaultResponse::Error { error },
        },
    }
}

/// The newest files directly in the system Downloads folder, for filing.
fn recent_downloads() -> Option<Vec<zephium_core::vault::VaultEntry>> {
    use zephium_core::vault::{valid_name, VaultEntry, VaultEntryKind};
    const LIMIT: usize = 40;
    let folder = dirs::download_dir()?;
    let mut entries: Vec<(std::time::SystemTime, VaultEntry)> = std::fs::read_dir(folder)
        .ok()?
        .flatten()
        .filter_map(|item| {
            let name = item.file_name().to_str()?.to_owned();
            if zephium_vault::hidden(&name)
                || !valid_name(&name)
                || name.ends_with(".crdownload")
                || name.ends_with(".partial")
            {
                return None;
            }
            let meta = std::fs::symlink_metadata(item.path()).ok()?;
            if !meta.is_file() {
                return None;
            }
            let modified = meta.modified().ok()?;
            let millis = modified
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |since| since.as_millis());
            let ext = std::path::Path::new(&name)
                .extension()
                .and_then(|ext| ext.to_str())
                .map(str::to_ascii_lowercase)
                .unwrap_or_default();
            Some((
                modified,
                VaultEntry {
                    path: name.clone(),
                    name,
                    kind: VaultEntryKind::File,
                    ext,
                    size: meta.len().to_string(),
                    modified: millis.to_string(),
                },
            ))
        })
        .collect();
    entries.sort_by(|a, b| b.0.cmp(&a.0));
    Some(
        entries
            .into_iter()
            .take(LIMIT)
            .map(|(_, entry)| entry)
            .collect(),
    )
}

/// `zephium-vault://localhost/<percent-encoded "<profile>/<vault path>">` serves a file's
/// bytes to the main chrome view only, for viewers (PDF, images, Office).
pub(crate) fn serve(
    ctx: tauri::UriSchemeContext<'_, tauri::Wry>,
    request: tauri::http::Request<Vec<u8>>,
    responder: tauri::UriSchemeResponder,
) {
    if ctx.webview_label() != MAIN_LABEL {
        return responder.respond(respond(403, Vec::new(), "text/plain"));
    }
    let app = ctx.app_handle().clone();
    let uri = request.uri().clone();
    tauri::async_runtime::spawn_blocking(move || {
        responder.respond(resolve(&app, uri.path()));
    });
}

fn decode(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let hex = std::str::from_utf8(bytes.get(index + 1..index + 3)?).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            index += 3;
        } else {
            out.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(out).ok()
}

fn resolve(app: &AppHandle, path: &str) -> tauri::http::Response<Cow<'static, [u8]>> {
    let not_found = || respond(404, Vec::new(), "text/plain");
    // The frame builds URLs with `convertFileSrc("<profile>/<path>")`, which
    // percent-encodes the separators: `zephium-vault://localhost/<encoded>` on
    // macOS and `http://zephium-vault.localhost/<encoded>` on Windows.
    let Some(decoded) = decode(path.trim_start_matches('/')) else {
        return not_found();
    };
    let Some((profile_text, relative)) = decoded.split_once('/') else {
        return not_found();
    };
    let Some(profile) = profile_of(profile_text) else {
        return not_found();
    };
    let relative = relative.to_owned();
    let Some(state) = app.try_state::<VaultState>() else {
        return not_found();
    };
    let Ok(open) = state.get(app, profile) else {
        return not_found();
    };
    match open.vault.read(&relative, MAX_VAULT_READ_BYTES) {
        Ok((bytes, _)) => respond(200, bytes, mime_for(&relative)),
        Err(_) => not_found(),
    }
}

fn mime_for(path: &str) -> &'static str {
    let ext = path
        .rsplit_once('.')
        .map(|(_, ext)| ext.to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "pdf" => "application/pdf",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        "avif" => "image/avif",
        // SVG can carry script; serve it as inert text, never as an image document.
        _ => "application/octet-stream",
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
        .header("Cache-Control", "no-store")
        .header("Content-Security-Policy", "default-src 'none'; sandbox")
        .header("Access-Control-Allow-Origin", "*")
        .body(Cow::Owned(body))
        .unwrap_or_else(|_| tauri::http::Response::new(Cow::Borrowed(b"" as &[u8])))
}
