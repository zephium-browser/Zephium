//! The notes folder service and its privileged IPC surface.
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tauri::{Manager, WebviewWindow};
use tauri_specta::Event;
use zephium_app::{Command, Handle};
use zephium_core::ids::ProfileId;
use zephium_core::notes::{NoteCall, NoteError, NoteReply, NoteResponse};
use zephium_core::ports::store::Store;
use zephium_core::resources::ResourceContent;
use zephium_notes::legacy::LegacyNote;
use zephium_notes::{Host, NoteService};
use zephium_store::SqliteStore;

use crate::{
    authorize, emit_to_privileged, resource_close, shutdown_started, CallerPolicy, MAIN_LABEL,
};

/// A store answer the one-time import waits for. The notes thread is the
/// only waiter, and a slow store only postpones the import to a later open.
const STORE_WAIT: Duration = Duration::from_secs(10);

#[derive(Clone, Debug, Serialize, Deserialize, specta::Type, Event)]
pub(crate) struct NotesChanged(zephium_core::notes::NoteChanges);

struct DesktopHost {
    app: tauri::AppHandle,
    store: Arc<SqliteStore>,
}

impl Host for DesktopHost {
    fn changed(&self, event: zephium_core::notes::NoteChanges) {
        let event = NotesChanged(event);
        emit_to_privileged(&self.app, MAIN_LABEL, "zephium:notes-changed", &event);
    }

    fn reveal(&self, path: &Path) {
        let path = path.to_path_buf();
        let _ = self.app.run_on_main_thread(move || reveal(&path));
    }

    fn legacy_notes(&self, profile: ProfileId) -> Option<Vec<LegacyNote>> {
        let (send, receive) = std::sync::mpsc::sync_channel(1);
        self.store.legacy_notes(
            profile,
            Box::new(move |records| {
                let _ = send.send(records);
            }),
        );
        let records = receive.recv_timeout(STORE_WAIT).ok()??;
        Some(
            records
                .into_iter()
                .filter_map(|record| match record.draft.content {
                    ResourceContent::Note { document } => Some(LegacyNote {
                        id: record.id,
                        title: record.draft.title,
                        pinned: record.draft.pinned,
                        trashed: record.trashed,
                        updated_at: record.updated_at.parse().unwrap_or(0),
                        document,
                    }),
                    ResourceContent::Task { .. }
                    | ResourceContent::Object { .. }
                    | ResourceContent::Media { .. } => None,
                })
                .collect(),
        )
    }

    fn retire_legacy_notes(&self, profile: ProfileId, ids: Vec<String>) -> bool {
        let (send, receive) = std::sync::mpsc::sync_channel(1);
        self.store.retire_legacy_notes(
            profile,
            ids,
            Box::new(move |retired| {
                let _ = send.send(retired);
            }),
        );
        receive.recv_timeout(STORE_WAIT).unwrap_or(false)
    }
}

/// Shows a vault file or folder in the system file manager.
pub(crate) fn reveal_path(path: &Path) {
    reveal(path);
}

/// A folder opens; a note is selected in its folder.
#[cfg(target_os = "macos")]
fn reveal(path: &Path) {
    use objc2_app_kit::NSWorkspace;
    use objc2_foundation::{NSArray, NSString, NSURL};
    let Some(text) = path.to_str() else { return };
    let url = NSURL::fileURLWithPath(&NSString::from_str(text));
    let workspace = NSWorkspace::sharedWorkspace();
    if path.is_dir() {
        workspace.openURL(&url);
    } else {
        workspace.activateFileViewerSelectingURLs(&NSArray::from_retained_slice(&[url]));
    }
}

#[cfg(target_os = "windows")]
fn reveal(path: &Path) {
    let mut explorer = std::process::Command::new("explorer.exe");
    if path.is_dir() {
        explorer.arg(path);
    } else {
        let mut argument = std::ffi::OsString::from("/select,");
        argument.push(path);
        explorer.arg(argument);
    }
    let _ = explorer.spawn();
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn reveal(path: &Path) {
    let folder = if path.is_dir() {
        path
    } else {
        path.parent().unwrap_or(path)
    };
    let _ = std::process::Command::new("xdg-open").arg(folder).spawn();
}

/// Starts the notes thread and hands it to the shell. Notes are optional:
/// if the thread cannot start, the browser runs and Notes reports itself
/// unavailable.
pub(crate) fn install(
    app: &tauri::AppHandle,
    data_dir: &Path,
    store: Arc<SqliteStore>,
    shell: &Handle,
) {
    let host = Arc::new(DesktopHost {
        app: app.clone(),
        store,
    });
    match NoteService::start(data_dir.to_path_buf(), host) {
        Ok(service) => {
            let service = Arc::new(service);
            if let Some(vault) = app.try_state::<crate::vault::VaultState>() {
                vault.attach_notes(service.clone());
            }
            let service: zephium_core::ports::notes::SharedNotes = service;
            if !shell.dispatch(Command::AttachNotes(zephium_app::NotesAttachment(service))) {
                crate::write_diagnostic(format_args!("notes: the shell refused its notes service"));
            }
        }
        Err(error) => {
            crate::write_diagnostic(format_args!("notes: service did not start: {error}"))
        }
    }
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn note_call(
    caller: WebviewWindow,
    app: tauri::AppHandle,
    expected_profile: String,
    call: NoteCall,
) -> NoteReply {
    let failed = |error| NoteReply {
        profile: None,
        response: NoteResponse::Error { error },
    };
    if !authorize(&caller, CallerPolicy::Both, "note_call") || shutdown_started(&app) {
        return failed(NoteError::Unavailable);
    }
    if !call.validate() {
        return failed(NoteError::Invalid);
    }
    if !resource_close::touch(&app, caller.label()) {
        return failed(NoteError::Unavailable);
    }
    let Some(expected_profile) =
        ProfileId::parse(&expected_profile).filter(|id| id.to_string() == expected_profile)
    else {
        return failed(NoteError::Invalid);
    };
    let shell = app.state::<Handle>().inner().clone();
    static NOTE_ADMISSION: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(8);
    let Ok(permit) = NOTE_ADMISSION.try_acquire() else {
        return failed(NoteError::Capacity);
    };
    let (send, receive) = tokio::sync::oneshot::channel();
    if !shell.dispatch(Command::NoteCall {
        expected_profile,
        call: Arc::new(call),
        done: zephium_app::NoteCompletion::new(move |reply| {
            let _permit = permit;
            let _ = send.send(reply);
        }),
    }) {
        return failed(NoteError::Unavailable);
    }
    match tokio::time::timeout(Duration::from_secs(8), receive).await {
        Ok(Ok(reply)) => reply,
        _ => failed(NoteError::OutcomeUnknown),
    }
}
