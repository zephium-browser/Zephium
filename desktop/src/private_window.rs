//! The private window in the menus. It is a session the one window shows
//! beside the regular tabs, so the menus offer both sides as checked entries
//! for as long as it exists, and the check follows whichever is in front.
use std::sync::Mutex;

use tauri::menu::{CheckMenuItem, CheckMenuItemBuilder, IsMenuItem};
use tauri::AppHandle;
use zephium_core::commands::ResolvedCommand;
use zephium_ipc::ProfilesView;

use crate::menu_text;

pub(crate) const REGULAR: &str = "window.showRegular";
pub(crate) const PRIVATE: &str = "window.showPrivate";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Standing {
    /// No private window exists.
    #[default]
    Absent,
    Regular,
    Private,
}

impl Standing {
    fn of(view: &ProfilesView) -> Self {
        match (view.private_tabs, view.private) {
            (None, _) => Self::Absent,
            (Some(_), false) => Self::Regular,
            (Some(_), true) => Self::Private,
        }
    }
}

static FOLLOWED: Mutex<Standing> = Mutex::new(Standing::Absent);

fn standing() -> Standing {
    *FOLLOWED.lock().unwrap_or_else(|e| e.into_inner())
}

/// Follows each roster projection. The menu bar is rebuilt only when the
/// private window appears or goes, since setting it makes every title flash;
/// moving between the sides only moves the check.
pub(crate) fn follow(app: &AppHandle, view: &ProfilesView) {
    let next = Standing::of(view);
    let previous = std::mem::replace(
        &mut *FOLLOWED.lock().unwrap_or_else(|e| e.into_inner()),
        next,
    );
    if previous == next {
        return;
    }
    #[cfg(target_os = "macos")]
    {
        let reshaped = (previous == Standing::Absent) != (next == Standing::Absent);
        let handle = app.clone();
        let queued = app.run_on_main_thread(move || {
            if !reshaped && sync_checks(&handle).is_ok_and(|synced| synced) {
                return;
            }
            rebuild_menu_bar(&handle);
        });
        if queued.is_err() {
            crate::write_diagnostic(format_args!("menu: private window was not applied"));
        }
    }
    #[cfg(not(target_os = "macos"))]
    let _ = (app, previous);
}

/// A click flips a check natively before anything else happens. Puts it back
/// to what is true, which a changed projection then moves on.
#[cfg_attr(not(target_os = "macos"), allow(unused_variables))]
pub(crate) fn settle_click(app: &AppHandle) {
    #[cfg(target_os = "macos")]
    {
        let handle = app.clone();
        let _ = app.run_on_main_thread(move || {
            if !sync_checks(&handle).is_ok_and(|synced| synced) {
                rebuild_menu_bar(&handle);
            }
        });
    }
}

#[cfg(target_os = "macos")]
fn rebuild_menu_bar(app: &AppHandle) {
    use tauri::Manager;
    let Some(keymap) = app.try_state::<crate::keymap::Keymap>() else {
        return;
    };
    if crate::install_menu_bar(app, &keymap.overrides(), keymap.work_pane_shown()).is_err() {
        crate::write_diagnostic(format_args!("menu: private window was not applied"));
    }
}

/// Moves the check on the installed menu bar. False when the bar is not the
/// shape the standing describes, which asks for a rebuild. Main thread only.
#[cfg(target_os = "macos")]
fn sync_checks(app: &AppHandle) -> tauri::Result<bool> {
    use tauri::menu::MenuItemKind;

    let Some(menu) = app.menu() else {
        return Ok(false);
    };
    let standing = standing();
    let mut found = 0;
    for entry in menu.items()? {
        let MenuItemKind::Submenu(section) = entry else {
            continue;
        };
        for item in section.items()? {
            let MenuItemKind::Check(check) = item else {
                continue;
            };
            let side = match check.id().as_ref() {
                REGULAR => Standing::Regular,
                PRIVATE => Standing::Private,
                _ => continue,
            };
            check.set_checked(standing == side)?;
            found += 1;
        }
    }
    Ok(found == if standing == Standing::Absent { 0 } else { 2 })
}

/// The two sides with a check on the one in front, or nothing while there is
/// no private window to go between.
pub(crate) fn items(
    handle: &AppHandle,
    resolved: &[ResolvedCommand],
) -> tauri::Result<Vec<CheckMenuItem<tauri::Wry>>> {
    let standing = standing();
    if standing == Standing::Absent {
        return Ok(Vec::new());
    }
    [(REGULAR, Standing::Regular), (PRIVATE, Standing::Private)]
        .into_iter()
        .map(|(id, side)| {
            let command = resolved
                .iter()
                .find(|command| command.id == id)
                .ok_or_else(|| {
                    tauri::Error::Io(std::io::Error::other(format!(
                        "native menu references unregistered command {id}"
                    )))
                })?;
            let mut builder =
                CheckMenuItemBuilder::with_id(command.id, menu_text::label(command.title))
                    .checked(standing == side);
            if let Some(accelerator) = &command.accelerator {
                builder = builder.accelerator(accelerator);
            }
            builder.build(handle)
        })
        .collect()
}

pub(crate) fn refs(items: &[CheckMenuItem<tauri::Wry>]) -> Vec<&dyn IsMenuItem<tauri::Wry>> {
    items
        .iter()
        .map(|item| item as &dyn IsMenuItem<tauri::Wry>)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view(private: bool, private_tabs: Option<u32>) -> ProfilesView {
        ProfilesView {
            current: None,
            private,
            private_tabs,
            profiles: Vec::new(),
        }
    }

    #[test]
    fn the_standing_names_which_side_is_in_front_once_a_private_window_exists() {
        assert_eq!(Standing::of(&view(false, None)), Standing::Absent);
        assert_eq!(Standing::of(&view(true, None)), Standing::Absent);
        assert_eq!(Standing::of(&view(false, Some(2))), Standing::Regular);
        assert_eq!(Standing::of(&view(true, Some(1))), Standing::Private);
    }
}
