//! The Help menu, and where its links lead. Each opens in a tab of Zephium
//! itself; About is the About page of Settings.

use tauri::menu::{
    IsMenuItem, Menu, MenuItem, MenuItemBuilder, PredefinedMenuItem, Submenu, SubmenuBuilder,
    HELP_SUBMENU_ID,
};

use crate::menu_text;

pub(crate) const ABOUT: &str = "settings.about";
pub(crate) const MENU: &str = "help.menu";
/// Opens About, where the diagnostics for a bug report are copied.
pub(crate) const DIAGNOSTICS: &str = "help.diagnostics";
const DOCS: &str = "help.docs";
const RELEASES: &str = "help.releases";
const ISSUES: &str = "help.issues";

pub(crate) fn url(id: &str) -> Option<&'static str> {
    match id {
        DOCS => Some("https://zephium.app/docs"),
        RELEASES => Some("https://zephium.app/changelog"),
        ISSUES => Some("https://github.com/zephium-browser/Zephium/issues"),
        _ => None,
    }
}

type Entries = Vec<Box<dyn IsMenuItem<tauri::Wry>>>;

fn entries(handle: &tauri::AppHandle, shortcuts: &MenuItem<tauri::Wry>) -> tauri::Result<Entries> {
    let docs = MenuItemBuilder::with_id(DOCS, menu_text::label("Zephium Help")).build(handle)?;
    let releases =
        MenuItemBuilder::with_id(RELEASES, menu_text::label("Release Notes")).build(handle)?;
    let issues =
        MenuItemBuilder::with_id(ISSUES, menu_text::label("Report an Issue…")).build(handle)?;
    let diagnostics =
        MenuItemBuilder::with_id(DIAGNOSTICS, menu_text::label("Diagnostics…")).build(handle)?;
    #[cfg_attr(target_os = "macos", allow(unused_mut))]
    let mut entries: Entries = vec![
        Box::new(docs),
        Box::new(releases),
        Box::new(PredefinedMenuItem::separator(handle)?),
        Box::new(shortcuts.clone()),
        Box::new(PredefinedMenuItem::separator(handle)?),
        Box::new(issues),
        Box::new(diagnostics),
    ];
    // Windows and Linux have no application menu to hold it.
    #[cfg(not(target_os = "macos"))]
    {
        let about =
            MenuItemBuilder::with_id(ABOUT, menu_text::label("About Zephium")).build(handle)?;
        entries.push(Box::new(PredefinedMenuItem::separator(handle)?));
        entries.push(Box::new(about));
    }
    Ok(entries)
}

/// With the macOS help-menu id, the system adds its menu search to it.
pub(crate) fn build_menu(
    handle: &tauri::AppHandle,
    shortcuts: &MenuItem<tauri::Wry>,
) -> tauri::Result<Submenu<tauri::Wry>> {
    let entries = entries(handle, shortcuts)?;
    let refs: Vec<&dyn IsMenuItem<tauri::Wry>> = entries.iter().map(|item| item.as_ref()).collect();
    SubmenuBuilder::with_id(handle, HELP_SUBMENU_ID, menu_text::label("Help"))
        .items(&refs)
        .build()
}

/// The same entries where there is no menu bar to hold them.
pub(crate) fn build_popup(
    handle: &tauri::AppHandle,
    shortcuts: &MenuItem<tauri::Wry>,
) -> tauri::Result<Menu<tauri::Wry>> {
    let entries = entries(handle, shortcuts)?;
    let refs: Vec<&dyn IsMenuItem<tauri::Wry>> = entries.iter().map(|item| item.as_ref()).collect();
    Menu::with_items(handle, &refs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links_lead_to_the_documentation_the_releases_and_the_issue_tracker() {
        assert_eq!(url(DOCS), Some("https://zephium.app/docs"));
        assert_eq!(url(RELEASES), Some("https://zephium.app/changelog"));
        assert!(url(ISSUES).is_some_and(|url| url.ends_with("/issues")));
        assert_eq!(url(ABOUT), None);
        assert_eq!(url(DIAGNOSTICS), None);
        assert_eq!(url("browser.settings"), None);
    }

    #[test]
    fn every_link_is_one_the_shell_would_open() {
        for id in [DOCS, RELEASES, ISSUES] {
            let url = url(id).unwrap();
            assert!(
                zephium_core::navigation::external_target(url).is_some(),
                "{id}"
            );
        }
    }
}
