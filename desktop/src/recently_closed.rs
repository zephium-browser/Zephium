//! Recently Closed in the History menu: the newest tabs the focused profile
//! can reopen, each a menu item that names its closed tab.
use std::sync::Mutex;

use tauri::menu::{IsMenuItem, MenuItemBuilder, Submenu, SubmenuBuilder};
use tauri::AppHandle;
use zephium_ipc::ClosedTabView;

use crate::menu_text;

/// An item's id is this prefix and the closed tab's id.
pub(crate) const ITEM: &str = "tab.reopen=";
const MENU: &str = "recentlyClosed";

/// (closed tab id, menu label), newest first.
static FOLLOWED: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());

fn followed() -> Vec<(String, String)> {
    FOLLOWED.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

fn label(tab: &ClosedTabView) -> String {
    let title = tab.title.trim();
    super::extension_menu_label(if title.is_empty() { &tab.url } else { title })
}

/// Follows each change to the list. The installed menu is edited in place,
/// since setting the whole bar again makes every title flash; where the menu
/// is built on each use, the next build reads the list.
pub(crate) fn follow(app: &AppHandle, tabs: &[ClosedTabView]) {
    let next: Vec<_> = tabs
        .iter()
        .map(|tab| (tab.id.clone(), label(tab)))
        .collect();
    {
        let mut shown = FOLLOWED.lock().unwrap_or_else(|e| e.into_inner());
        if *shown == next {
            return;
        }
        *shown = next;
    }
    #[cfg(target_os = "macos")]
    {
        let handle = app.clone();
        let queued = app.run_on_main_thread(move || {
            if sync(&handle).is_err() {
                crate::write_diagnostic(format_args!("menu: recently closed was not applied"));
            }
        });
        if queued.is_err() {
            crate::write_diagnostic(format_args!("menu: recently closed was not applied"));
        }
    }
    #[cfg(not(target_os = "macos"))]
    let _ = app;
}

fn entries(
    handle: &AppHandle,
    shown: &[(String, String)],
) -> tauri::Result<Vec<Box<dyn IsMenuItem<tauri::Wry>>>> {
    shown
        .iter()
        .map(|(id, label)| {
            MenuItemBuilder::with_id(format!("{ITEM}{id}"), label)
                .build(handle)
                .map(|item| Box::new(item) as Box<dyn IsMenuItem<tauri::Wry>>)
        })
        .collect()
}

/// "Recently Closed ▸", greyed while there is nothing to reopen.
pub(crate) fn build_menu_section(handle: &AppHandle) -> tauri::Result<Submenu<tauri::Wry>> {
    let shown = followed();
    let items = entries(handle, &shown)?;
    let refs: Vec<&dyn IsMenuItem<tauri::Wry>> = items.iter().map(|item| item.as_ref()).collect();
    SubmenuBuilder::with_id(handle, MENU, menu_text::label("Recently Closed"))
        .items(&refs)
        .enabled(!shown.is_empty())
        .build()
}

/// Brings the installed submenu up to date. Main thread only.
#[cfg(target_os = "macos")]
fn sync(app: &AppHandle) -> tauri::Result<()> {
    use tauri::menu::MenuItemKind;

    let Some(menu) = app.menu() else {
        return Ok(());
    };
    let shown = followed();
    for entry in menu.items()? {
        let MenuItemKind::Submenu(section) = entry else {
            continue;
        };
        for item in section.items()? {
            let MenuItemKind::Submenu(recent) = item else {
                continue;
            };
            if recent.id().as_ref() != MENU {
                continue;
            }
            while recent.remove_at(0)?.is_some() {}
            for item in entries(app, &shown)? {
                recent.append(item.as_ref())?;
            }
            recent.set_enabled(!shown.is_empty())?;
            return Ok(());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tab(title: &str, url: &str) -> ClosedTabView {
        ClosedTabView {
            id: "id".into(),
            url: url.into(),
            title: title.into(),
            icon: None,
        }
    }

    #[test]
    fn a_label_is_the_title_else_the_address_and_never_a_mnemonic() {
        assert_eq!(
            label(&tab("Docs & Notes", "https://a.example/")),
            "Docs && Notes"
        );
        assert_eq!(
            label(&tab("  ", "https://a.example/")),
            "https://a.example/"
        );
    }
}
