//! Profiles from privileged chrome: the roster, its mutations, and avatar
//! photos. A picked photo is cropped and downscaled here and staged under a
//! token; chrome previews it by URL and names the token when it saves, so
//! picture bytes never cross IPC.
use std::borrow::Cow;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tauri::{AppHandle, Manager, State, WebviewWindow};
use tauri_plugin_dialog::DialogExt;
use zephium_app::{Command, Handle};
use zephium_core::ids::ProfileId;
use zephium_core::ports::store::Store as _;
use zephium_core::profiles::{ProfileAvatar, ProfileAvatarStyle};
use zephium_ipc::{OperationAdmission, ProfileAvatarStyleView, ProfilesView};

const MAX_PICKED_BYTES: u64 = 32 * 1024 * 1024;

#[derive(Clone, Debug, serde::Deserialize, specta::Type)]
pub(crate) struct ProfileAvatarInputV1 {
    style: ProfileAvatarStyleView,
    color: u8,
    symbol: u8,
    /// The token `profile_avatar_pick` returned, for a photo style.
    photo: Option<String>,
}

#[derive(Clone, Debug, serde::Serialize, specta::Type)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum ProfileAvatarPickV1 {
    Picked { token: String },
    Cancelled,
    Unsupported,
    Unavailable,
}

#[derive(Clone, Debug, serde::Serialize, specta::Type)]
pub(crate) struct BookmarkFolderView {
    pub id: String,
    pub title: String,
}

/// One staged photo at a time: a newer pick replaces an unsaved one.
static STAGED: Mutex<Option<(String, Arc<Vec<u8>>)>> = Mutex::new(None);

fn staged(token: &str) -> Option<Arc<Vec<u8>>> {
    STAGED
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .as_ref()
        .filter(|(staged, _)| staged == token)
        .map(|(_, bytes)| bytes.clone())
}

fn profile_of(value: &str) -> Option<ProfileId> {
    ProfileId::parse(value).filter(|id| id.to_string() == value)
}

fn avatar_of(input: &ProfileAvatarInputV1) -> Option<(ProfileAvatar, Option<Arc<Vec<u8>>>)> {
    let style = match input.style {
        ProfileAvatarStyleView::Initials => ProfileAvatarStyle::Initials,
        ProfileAvatarStyleView::Symbol => ProfileAvatarStyle::Symbol,
        ProfileAvatarStyleView::Photo => ProfileAvatarStyle::Photo,
    };
    let photo = match (style, input.photo.as_deref()) {
        (ProfileAvatarStyle::Photo, Some(token)) if token.len() <= 64 => Some(staged(token)?),
        (ProfileAvatarStyle::Photo, _) => return None,
        (_, _) => None,
    };
    let avatar = ProfileAvatar {
        style,
        color: input.color,
        symbol: input.symbol,
        revision: 0,
    };
    avatar.valid().then_some((avatar, photo))
}

fn admitted(caller: &WebviewWindow, command: &str) -> bool {
    super::authorize(caller, super::CallerPolicy::Main, command)
        && !super::shutdown_started(caller.app_handle())
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn profiles_get(
    caller: WebviewWindow,
    shell: State<'_, Handle>,
) -> Result<Option<ProfilesView>, ()> {
    if !admitted(&caller, "profiles_get") {
        return Ok(None);
    }
    let receiver = shell.profiles();
    Ok(
        tokio::task::spawn_blocking(move || receiver.recv_timeout(Duration::from_secs(4)))
            .await
            .ok()
            .and_then(Result::ok)
            .flatten(),
    )
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn profiles_bookmark_folders(
    caller: WebviewWindow,
    profile: String,
) -> Vec<BookmarkFolderView> {
    if !admitted(&caller, "profiles_bookmark_folders") {
        return Vec::new();
    }
    let Some(profile) = profile_of(&profile) else {
        return Vec::new();
    };
    let Some(store) = crate::APP_STORE.get().cloned() else {
        return Vec::new();
    };
    tokio::task::spawn_blocking(move || {
        store
            .bookmark_folders(profile)
            .into_iter()
            .map(|folder| BookmarkFolderView {
                id: folder.id.to_string(),
                title: folder.title,
            })
            .collect()
    })
    .await
    .unwrap_or_default()
}

#[tauri::command]
#[specta::specta]
pub(crate) fn profiles_create(
    caller: WebviewWindow,
    shell: State<'_, Handle>,
    name: String,
    avatar: ProfileAvatarInputV1,
    and_switch: bool,
    bookmark_folder: Option<String>,
) -> OperationAdmission {
    if !admitted(&caller, "profiles_create")
        || !super::bounded(&name, super::MAX_PROFILE_NAME_BYTES)
    {
        return super::rejected_operation();
    }
    let Some((avatar, photo)) = avatar_of(&avatar) else {
        return super::rejected_operation();
    };
    let bookmark_folder = match bookmark_folder {
        Some(value) if value.len() <= 19 => match value.parse::<i64>() {
            Ok(folder) if folder > 0 => Some(folder),
            _ => return super::rejected_operation(),
        },
        Some(_) => return super::rejected_operation(),
        None => None,
    };
    super::dispatch_operation(
        caller.app_handle(),
        &shell,
        Command::CreateProfile {
            name,
            avatar,
            photo,
            switch: and_switch,
            bookmark_folder,
        },
    )
}

#[tauri::command]
#[specta::specta]
pub(crate) fn profiles_switch(
    caller: WebviewWindow,
    shell: State<'_, Handle>,
    profile: String,
) -> OperationAdmission {
    let Some(profile) = profile_of(&profile).filter(|_| admitted(&caller, "profiles_switch"))
    else {
        return super::rejected_operation();
    };
    super::dispatch_operation(caller.app_handle(), &shell, Command::SwitchProfile(profile))
}

#[tauri::command]
#[specta::specta]
pub(crate) fn profiles_rename(
    caller: WebviewWindow,
    shell: State<'_, Handle>,
    profile: String,
    name: String,
) -> OperationAdmission {
    let Some(profile) = profile_of(&profile).filter(|_| {
        admitted(&caller, "profiles_rename") && super::bounded(&name, super::MAX_PROFILE_NAME_BYTES)
    }) else {
        return super::rejected_operation();
    };
    super::dispatch_operation(
        caller.app_handle(),
        &shell,
        Command::RenameProfile { profile, name },
    )
}

#[tauri::command]
#[specta::specta]
pub(crate) fn profiles_set_avatar(
    caller: WebviewWindow,
    shell: State<'_, Handle>,
    profile: String,
    avatar: ProfileAvatarInputV1,
) -> OperationAdmission {
    let (Some(profile), Some((avatar, photo))) = (profile_of(&profile), avatar_of(&avatar)) else {
        return super::rejected_operation();
    };
    if !admitted(&caller, "profiles_set_avatar") {
        return super::rejected_operation();
    }
    super::dispatch_operation(
        caller.app_handle(),
        &shell,
        Command::SetProfileAvatar {
            profile,
            avatar,
            photo,
        },
    )
}

/// Picks a picture natively and stages its 256px square copy.
#[tauri::command]
#[specta::specta]
pub(crate) async fn profile_avatar_pick(
    caller: WebviewWindow,
    app: AppHandle,
) -> Result<ProfileAvatarPickV1, ()> {
    if !admitted(&caller, "profile_avatar_pick") {
        return Ok(ProfileAvatarPickV1::Unavailable);
    }
    let picker = app.clone();
    let picked = tokio::task::spawn_blocking(move || {
        picker
            .dialog()
            .file()
            .set_title("Choose a Photo")
            .add_filter("Images", &["png", "jpg", "jpeg", "webp", "gif"])
            .blocking_pick_file()
    })
    .await
    .map_err(|_| ())?;
    let Some(picked) = picked else {
        return Ok(ProfileAvatarPickV1::Cancelled);
    };
    let path = match picked {
        tauri_plugin_dialog::FilePath::Path(path) => path,
        tauri_plugin_dialog::FilePath::Url(url) => match url.to_file_path() {
            Ok(path) => path,
            Err(()) => return Ok(ProfileAvatarPickV1::Unsupported),
        },
    };
    let prepared = tokio::task::spawn_blocking(move || -> Option<Vec<u8>> {
        let metadata = std::fs::metadata(&path).ok()?;
        if !metadata.is_file() || metadata.len() > MAX_PICKED_BYTES {
            return None;
        }
        zephium_store::prepare_avatar_photo(&std::fs::read(&path).ok()?).ok()
    })
    .await
    .map_err(|_| ())?;
    let Some(bytes) = prepared else {
        return Ok(ProfileAvatarPickV1::Unsupported);
    };
    let token = ProfileId::generate().to_string();
    *STAGED
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some((token.clone(), Arc::new(bytes)));
    Ok(ProfileAvatarPickV1::Picked { token })
}

fn respond(
    status: u16,
    body: Vec<u8>,
    cache: &'static str,
) -> tauri::http::Response<Cow<'static, [u8]>> {
    let content_type = if body.starts_with(b"\x89PNG") {
        "image/png"
    } else if body.starts_with(&[0xff, 0xd8]) {
        "image/jpeg"
    } else {
        "text/plain"
    };
    tauri::http::Response::builder()
        .status(status)
        .header("Content-Type", content_type)
        .header("X-Content-Type-Options", "nosniff")
        .header("Cache-Control", cache)
        .header("Content-Security-Policy", "default-src 'none'; sandbox")
        .body(Cow::Owned(body))
        .unwrap_or_else(|_| tauri::http::Response::new(Cow::Borrowed(b"" as &[u8])))
}

/// `avatar/<profile>/<revision>` serves a saved photo; the revision only
/// busts caches. `avatar/staged/<token>` serves an unsaved pick.
pub(crate) fn serve_avatar(
    app: &AppHandle,
    rest: &str,
) -> tauri::http::Response<Cow<'static, [u8]>> {
    let missing = || respond(404, Vec::new(), "no-store");
    if let Some(token) = rest.strip_prefix("staged/") {
        return staged(token).map_or_else(missing, |bytes| {
            respond(200, bytes.as_ref().clone(), "no-store")
        });
    }
    let Some(profile) = rest.split('/').next().and_then(profile_of) else {
        return missing();
    };
    let Some(shell) = app.try_state::<Handle>() else {
        return missing();
    };
    match shell
        .profile_avatar_photo(profile)
        .recv_timeout(Duration::from_secs(3))
    {
        Ok(Some(bytes)) => respond(200, bytes, "private, max-age=31536000, immutable"),
        _ => missing(),
    }
}
