//! The menu a long press or right click on Back or Forward opens: the
//! entries the tab can go to on that side. The shell reads them from the
//! engine; this draws them natively and sends the choice back with the
//! address that was shown, so a page that moved on is not sent elsewhere.

use super::*;
use zephium_core::ports::engine::{HistoryEntries, HistoryEntry};

pub(crate) const ID_PREFIX: &str = "historymenu.";
const MENU_ENTRIES: usize = 20;
const LABEL_CHARS: usize = 64;
const ANSWER_DEADLINE: std::time::Duration = std::time::Duration::from_secs(2);

/// The tab a menu was opened for and the (step, address) of each of its rows.
type ArmedMenu = (ItemId, Vec<(i32, String)>);

/// The menu on screen. A choice is checked against it, never against the
/// tab's present state.
#[derive(Default)]
pub(crate) struct HistoryMenuTarget(std::sync::Mutex<Option<ArmedMenu>>);

impl HistoryMenuTarget {
    fn arm(&self, id: ItemId, rows: Vec<(i32, String)>) -> bool {
        self.0
            .lock()
            .map(|mut slot| *slot = Some((id, rows)))
            .is_ok()
    }

    fn take(&self) -> Option<ArmedMenu> {
        self.0.lock().ok().and_then(|mut slot| slot.take())
    }
}

/// The rows of one side, nearest first and bounded.
fn side(list: HistoryEntries, forward: bool) -> Vec<HistoryEntry> {
    let entries = if forward { list.forward } else { list.back };
    entries.into_iter().take(MENU_ENTRIES).collect()
}

/// One line of text for a row: the title, else the host, with nothing that
/// would draw as a second line or a keyboard mnemonic.
fn label(entry: &HistoryEntry) -> String {
    let title = entry
        .title
        .split(|c: char| c.is_whitespace() || c.is_control())
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    let text = if title.is_empty() || title == "Untitled" {
        tauri::Url::parse(&entry.url)
            .ok()
            .and_then(|url| url.host_str().map(str::to_owned))
            .unwrap_or_else(|| entry.url.clone())
    } else {
        title
    };
    let shown = if text.chars().count() > LABEL_CHARS {
        let head: String = text.chars().take(LABEL_CHARS - 1).collect();
        format!("{}…", head.trim_end())
    } else {
        text
    };
    // A bare `&` would become a Windows mnemonic.
    shown.replace('&', "&&")
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn history_menu_popup(
    caller: WebviewWindow,
    app: tauri::AppHandle,
    id: String,
    forward: bool,
    x: f64,
    y: f64,
) -> bool {
    use tauri::menu::{Menu, MenuItemBuilder};
    if !authorize(&caller, CallerPolicy::Main, "history_menu_popup")
        || shutdown_started(&app)
        || !bounded(&id, MAX_ITEM_ID_BYTES)
    {
        return false;
    }
    let Some(item) = ItemId::parse(&id) else {
        return false;
    };
    let (Ok(inner_size), Ok(scale_factor)) = (caller.inner_size(), caller.scale_factor()) else {
        return false;
    };
    if !scale_factor.is_finite() || scale_factor <= 0.0 {
        return false;
    }
    let Some(anchor) = menu_popup_anchor(
        x,
        y,
        f64::from(inner_size.width) / scale_factor,
        f64::from(inner_size.height) / scale_factor,
    ) else {
        return false;
    };
    let (send, receive) = tokio::sync::oneshot::channel();
    let shell = app.state::<Handle>().inner().clone();
    if !shell.dispatch(Command::HistoryList {
        id: item,
        done: zephium_app::HistoryListCompletion::new(move |list| {
            let _ = send.send(list);
        }),
    }) {
        return false;
    }
    let Ok(Ok(list)) = tokio::time::timeout(ANSWER_DEADLINE, receive).await else {
        return false;
    };
    let entries = side(list, forward);
    if entries.is_empty() {
        return false;
    }
    let Some(target) = app.try_state::<HistoryMenuTarget>() else {
        return false;
    };
    if !target.arm(
        item,
        entries
            .iter()
            .map(|entry| (entry.offset, entry.url.clone()))
            .collect(),
    ) {
        return false;
    }
    let built = (|| -> tauri::Result<_> {
        let rows = entries
            .iter()
            .map(|entry| {
                MenuItemBuilder::with_id(format!("{ID_PREFIX}{}", entry.offset), label(entry))
                    .build(&app)
            })
            .collect::<tauri::Result<Vec<_>>>()?;
        let refs: Vec<&dyn tauri::menu::IsMenuItem<tauri::Wry>> = rows
            .iter()
            .map(|row| row as &dyn tauri::menu::IsMenuItem<tauri::Wry>)
            .collect();
        Menu::with_items(&app, &refs)
    })();
    let Ok(menu) = built else {
        return false;
    };
    app.run_on_main_thread(move || {
        let _ = caller.popup_menu_at(&menu, anchor);
    })
    .is_ok()
}

/// A row was chosen. Only a row of the menu just shown counts, once.
pub(crate) fn execute(app: &tauri::AppHandle, step: &str) -> zephium_ipc::OperationAdmission {
    let chosen = step.parse::<i32>().ok();
    let Some(shell) = app.try_state::<Handle>() else {
        return rejected_operation();
    };
    let Some(target) = app.try_state::<HistoryMenuTarget>() else {
        return rejected_operation();
    };
    match (chosen, target.take()) {
        (Some(offset), Some((id, rows))) => rows
            .into_iter()
            .find(|(step, _)| *step == offset)
            .map_or_else(rejected_operation, |(_, url)| {
                dispatch_operation(app, &shell, Command::GoToHistoryEntry { id, offset, url })
            }),
        _ => rejected_operation(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(offset: i32, title: &str, url: &str) -> HistoryEntry {
        HistoryEntry {
            offset,
            title: title.into(),
            url: url.into(),
        }
    }

    #[test]
    fn a_menu_lists_one_side_nearest_first_and_no_more_than_it_can_show() {
        let list = HistoryEntries {
            back: (1..=30)
                .map(|step| entry(-step, "Page", "https://a.example/"))
                .collect(),
            forward: vec![entry(1, "Next", "https://b.example/")],
        };
        let back = side(list.clone(), false);
        assert_eq!(back.len(), MENU_ENTRIES);
        assert_eq!(back[0].offset, -1);
        assert_eq!(side(list, true)[0].offset, 1);
    }

    #[test]
    fn a_row_is_one_short_line_without_mnemonics_or_controls() {
        assert_eq!(
            label(&entry(-1, "Fish &  Chips\n\u{7}", "https://a.example/")),
            "Fish && Chips"
        );
        assert_eq!(
            label(&entry(-1, "", "https://www.example.org/path")),
            "www.example.org"
        );
        assert_eq!(
            label(&entry(-1, "Untitled", "https://x.example/")),
            "x.example"
        );
        let long = label(&entry(-1, &"w".repeat(300), "https://a.example/"));
        assert_eq!(long.chars().count(), LABEL_CHARS);
        assert!(long.ends_with('…'));
    }

    #[test]
    fn a_choice_is_checked_against_the_menu_that_was_shown_and_only_once() {
        let target = HistoryMenuTarget::default();
        let tab = ItemId::generate();
        assert!(target.arm(tab, vec![(-1, "https://a.example/".into())]));
        let (armed, rows) = target.take().unwrap();
        assert_eq!(armed, tab);
        assert_eq!(rows, vec![(-1, "https://a.example/".to_owned())]);
        assert!(target.take().is_none());
    }
}
