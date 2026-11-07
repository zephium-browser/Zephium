//! Taking bookmarks or history out of the browser as a file. The shell reads
//! the focused profile and formats the file; this asks where to put it.

use super::*;
use zephium_core::export::{ExportKind, ExportRefusal};

#[derive(Clone, Copy, Debug, Serialize, Deserialize, specta::Type, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ExportKindView {
    Bookmarks,
    History,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, specta::Type, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ExportOutcomeView {
    Saved,
    /// The person closed the save dialog.
    Cancelled,
    /// A private window keeps no bookmarks or history.
    Private,
    Failed,
}

/// A history export reads every visit, so one runs at a time.
static ADMISSION: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(1);
const READ_DEADLINE: std::time::Duration = std::time::Duration::from_secs(60);

struct Target {
    kind: ExportKind,
    name: &'static str,
    filter: &'static str,
    extension: &'static str,
}

fn target(kind: ExportKindView) -> Target {
    match kind {
        ExportKindView::Bookmarks => Target {
            kind: ExportKind::Bookmarks,
            name: "Zephium bookmarks.html",
            filter: "HTML",
            extension: "html",
        },
        ExportKindView::History => Target {
            kind: ExportKind::History,
            name: "Zephium history.csv",
            filter: "CSV",
            extension: "csv",
        },
    }
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn export_browser_data(
    caller: WebviewWindow,
    app: tauri::AppHandle,
    kind: ExportKindView,
) -> ExportOutcomeView {
    use tauri_plugin_dialog::DialogExt;
    if !authorize(&caller, CallerPolicy::Main, "export_browser_data") || shutdown_started(&app) {
        return ExportOutcomeView::Failed;
    }
    let Ok(_permit) = ADMISSION.try_acquire() else {
        return ExportOutcomeView::Failed;
    };
    let target = target(kind);
    let (send, receive) = tokio::sync::oneshot::channel();
    let shell = app.state::<Handle>().inner().clone();
    if !shell.dispatch(Command::Export {
        kind: target.kind,
        done: zephium_app::ExportCompletion::new(move |reply| {
            let _ = send.send(reply);
        }),
    }) {
        return ExportOutcomeView::Failed;
    }
    let text = match tokio::time::timeout(READ_DEADLINE, receive).await {
        Ok(Ok(Ok(text))) => text,
        Ok(Ok(Err(ExportRefusal::Private))) => return ExportOutcomeView::Private,
        _ => return ExportOutcomeView::Failed,
    };
    let picked = tokio::task::spawn_blocking(move || {
        app.dialog()
            .file()
            .set_file_name(target.name)
            .add_filter(target.filter, &[target.extension])
            .blocking_save_file()
    })
    .await
    .ok()
    .flatten();
    let Some(path) = picked.and_then(|picked| picked.into_path().ok()) else {
        return ExportOutcomeView::Cancelled;
    };
    match tokio::task::spawn_blocking(move || std::fs::write(path, text)).await {
        Ok(Ok(())) => ExportOutcomeView::Saved,
        _ => ExportOutcomeView::Failed,
    }
}
