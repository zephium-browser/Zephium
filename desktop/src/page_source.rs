//! The document a source tab shows. The shell keeps it in memory against the
//! tab; this hands it to the page that renders it as plain text.

use super::*;

#[derive(Clone, Debug, Serialize, specta::Type)]
pub(crate) struct PageSourceView {
    url: String,
    text: String,
    /// The page was longer than the engine reads.
    truncated: bool,
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn page_source(
    caller: WebviewWindow,
    app: tauri::AppHandle,
    id: String,
) -> Option<PageSourceView> {
    if !authorize(&caller, CallerPolicy::Main, "page_source")
        || shutdown_started(&app)
        || !bounded(&id, MAX_ITEM_ID_BYTES)
    {
        return None;
    }
    let id = ItemId::parse(&id)?;
    let (send, receive) = tokio::sync::oneshot::channel();
    let shell = app.state::<Handle>().inner().clone();
    if !shell.dispatch(Command::PageSource {
        id,
        done: zephium_app::SourceCompletion::new(move |document| {
            let _ = send.send(document);
        }),
    }) {
        return None;
    }
    let document = tokio::time::timeout(std::time::Duration::from_secs(8), receive)
        .await
        .ok()?
        .ok()??;
    Some(PageSourceView {
        url: document.url.clone(),
        text: document.text.clone(),
        truncated: document.truncated,
    })
}
