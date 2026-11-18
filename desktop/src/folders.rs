//! Sidebar folders at the desktop edge: the typed folder intent, a folder's
//! native menu and the tab menu's folder section. Native follows the
//! projected folders only to name them in its menus; the shell checks every
//! action against the focused space again.
use std::sync::Mutex;

use tauri::menu::{IsMenuItem, Menu, MenuItemBuilder, PredefinedMenuItem, Submenu, SubmenuBuilder};
use tauri::{AppHandle, Manager, State, WebviewWindow};
use zephium_app::{Command, FolderCommand, Handle};
use zephium_core::ids::{ItemId, SpaceId};
use zephium_core::item::FolderColor;
use zephium_ipc::{FolderIntent, ItemsState, OperationAdmission, SidebarNodeKindView};

use crate::menu_text;

/// Bytes a folder name may carry over IPC; core bounds it again by chars.
const MAX_FOLDER_NAME_BYTES: usize = zephium_core::session::MAX_SESSION_NAME_CHARS * 4;
/// Folders the tab menu lists to move a tab into.
const MAX_LISTED_FOLDERS: usize = 32;

const FOLDER_MENU: &str = "foldermenu.";
const TAB_MENU_FOLDER: &str = "folder=";
pub(crate) const TAB_MENU_NEW_FOLDER: &str = "newFolder";
pub(crate) const TAB_MENU_UNFOLDER: &str = "unfolder";
/// Delivered to privileged chrome, which edits a name in place and holds the
/// folder its menu was opened on.
const RENAME_COMMAND: &str = "folder.rename";
const SUBFOLDER_COMMAND: &str = "folder.newSubfolder";
/// Chrome makes a folder of the tab its menu was opened on.
pub(crate) const TAB_NEW_FOLDER_COMMAND: &str = "tab.newFolder";

const COLORS: [(&str, Option<FolderColor>, &str); 7] = [
    ("none", None, "No Color"),
    ("sky", Some(FolderColor::Sky), "Sky"),
    ("mint", Some(FolderColor::Mint), "Mint"),
    ("lemon", Some(FolderColor::Lemon), "Lemon"),
    ("peach", Some(FolderColor::Peach), "Peach"),
    ("rose", Some(FolderColor::Rose), "Rose"),
    ("lilac", Some(FolderColor::Lilac), "Lilac"),
];

/// The active space's folders, in sidebar order, with their depth.
static FOLDERS: Mutex<Vec<(String, String, usize)>> = Mutex::new(Vec::new());

/// The folder a native menu was opened on. Popups are modal, so one at most.
#[derive(Default)]
pub(crate) struct FolderMenuTarget(Mutex<Option<ItemId>>);

impl FolderMenuTarget {
    fn arm(&self, id: ItemId) -> bool {
        self.0.lock().map(|mut slot| *slot = Some(id)).is_ok()
    }

    fn take(&self) -> Option<ItemId> {
        self.0.lock().ok().and_then(|mut slot| slot.take())
    }
}

/// Follows each sidebar projection, for the menus to name its folders.
pub(crate) fn follow(state: &ItemsState) {
    let mut depths: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    let mut folders = Vec::new();
    for node in &state.nodes {
        let SidebarNodeKindView::Folder { name, .. } = &node.kind else {
            continue;
        };
        let depth = node
            .parent_id
            .as_deref()
            .and_then(|parent| depths.get(parent))
            .map_or(0, |depth| depth + 1);
        depths.insert(node.id.as_str(), depth);
        if folders.len() < MAX_LISTED_FOLDERS {
            folders.push((node.id.clone(), name.clone(), depth));
        }
    }
    *FOLDERS.lock().unwrap_or_else(|e| e.into_inner()) = folders;
}

fn item(value: &str) -> Option<ItemId> {
    super::bounded(value, super::MAX_ITEM_ID_BYTES)
        .then(|| ItemId::parse(value))
        .flatten()
}

fn optional_item(value: Option<String>) -> Result<Option<ItemId>, ()> {
    value.map(|value| item(&value).ok_or(())).transpose()
}

fn name(value: String) -> Option<String> {
    super::bounded(&value, MAX_FOLDER_NAME_BYTES).then_some(value)
}

/// The intent with its ids parsed and its text bounded, or nothing.
fn command(intent: FolderIntent) -> Option<FolderCommand> {
    Some(match intent {
        FolderIntent::Create {
            name: label,
            color,
            tabs,
            parent,
            before,
        } => {
            if tabs.len() > zephium_core::session::MAX_SESSION_ITEMS {
                return None;
            }
            FolderCommand::Create {
                name: name(label)?,
                color,
                tabs: tabs.iter().map(|tab| item(tab)).collect::<Option<_>>()?,
                parent: optional_item(parent).ok()?,
                before: optional_item(before).ok()?,
            }
        }
        FolderIntent::Rename { id, name: label } => FolderCommand::Rename {
            id: item(&id)?,
            name: name(label)?,
        },
        FolderIntent::SetColor { id, color } => FolderCommand::SetColor {
            id: item(&id)?,
            color,
        },
        FolderIntent::SetCollapsed { id, collapsed } => FolderCommand::SetCollapsed {
            id: item(&id)?,
            collapsed,
        },
        FolderIntent::Move { id, parent, before } => FolderCommand::Move {
            id: item(&id)?,
            parent: optional_item(parent).ok()?,
            before: optional_item(before).ok()?,
        },
        FolderIntent::Ungroup { id } => FolderCommand::Ungroup(item(&id)?),
        FolderIntent::CloseTabs { id } => FolderCommand::CloseTabs(item(&id)?),
        FolderIntent::Delete { id } => FolderCommand::Delete(item(&id)?),
    })
}

fn dispatch(app: &AppHandle, shell: &Handle, command: FolderCommand) -> OperationAdmission {
    super::dispatch_operation(app, shell, Command::Folder(Box::new(command)))
}

#[tauri::command]
#[specta::specta]
pub(crate) fn tabs_folder(
    caller: WebviewWindow,
    shell: State<'_, Handle>,
    intent: FolderIntent,
) -> OperationAdmission {
    if !super::authorize(&caller, super::CallerPolicy::Main, "tabs_folder")
        || super::shutdown_started(caller.app_handle())
    {
        return super::rejected_operation();
    }
    match command(intent) {
        Some(command) => dispatch(caller.app_handle(), &shell, command),
        None => super::rejected_operation(),
    }
}

/// What a folder's menu can offer, as the sidebar sees it. Only
/// availability: the shell checks every action again.
#[derive(Clone, Debug, serde::Deserialize, specta::Type)]
pub(crate) struct FolderMenuContext {
    color: Option<FolderColor>,
    /// Another folder level fits inside this one.
    can_nest: bool,
    /// A page in it is loaded.
    awake: bool,
}

#[tauri::command]
#[specta::specta]
pub(crate) fn folder_menu_popup(
    caller: WebviewWindow,
    app: AppHandle,
    id: String,
    x: f64,
    y: f64,
    context: FolderMenuContext,
) -> bool {
    if !super::authorize(&caller, super::CallerPolicy::Main, "folder_menu_popup") {
        return false;
    }
    let Some(folder) = item(&id) else {
        return false;
    };
    let (Ok(size), Ok(scale)) = (caller.inner_size(), caller.scale_factor()) else {
        return false;
    };
    if !scale.is_finite() || scale <= 0.0 {
        return false;
    }
    let Some(anchor) = super::menu_popup_anchor(
        x,
        y,
        f64::from(size.width) / scale,
        f64::from(size.height) / scale,
    ) else {
        return false;
    };
    let Some(target) = app.try_state::<FolderMenuTarget>() else {
        return false;
    };
    if !target.arm(folder) {
        return false;
    }
    let Ok(menu) = build_folder_menu(&app, &context) else {
        return false;
    };
    caller.popup_menu_at(&menu, anchor).is_ok()
}

type Items = Vec<Box<dyn IsMenuItem<tauri::Wry>>>;

fn refs(items: &Items) -> Vec<&dyn IsMenuItem<tauri::Wry>> {
    items.iter().map(|item| item.as_ref()).collect()
}

fn entry(
    handle: &AppHandle,
    id: &str,
    title: &str,
    enabled: bool,
) -> tauri::Result<Box<dyn IsMenuItem<tauri::Wry>>> {
    Ok(Box::new(
        MenuItemBuilder::with_id(format!("{FOLDER_MENU}{id}"), menu_text::label(title))
            .enabled(enabled)
            .build(handle)?,
    ))
}

fn build_folder_menu(
    handle: &AppHandle,
    context: &FolderMenuContext,
) -> tauri::Result<Menu<tauri::Wry>> {
    use tauri::menu::CheckMenuItemBuilder;
    let mut colors: Items = Vec::new();
    for (id, color, title) in COLORS {
        colors.push(Box::new(
            CheckMenuItemBuilder::with_id(
                format!("{FOLDER_MENU}color={id}"),
                menu_text::label(title),
            )
            .checked(context.color == color)
            .build(handle)?,
        ));
    }
    let color = SubmenuBuilder::new(handle, menu_text::label("Color"))
        .items(&refs(&colors))
        .build()?;
    let mut items: Items = vec![
        entry(handle, "rename", "Rename Folder…", true)?,
        Box::new(color),
        entry(handle, "subfolder", "New Folder Inside", context.can_nest)?,
        Box::new(PredefinedMenuItem::separator(handle)?),
        entry(handle, "closeTabs", "Close Tabs in Folder", context.awake)?,
        entry(handle, "ungroup", "Ungroup", true)?,
    ];
    if let Some(spaces) = super::spaces::tab_menu_section(handle, &format!("{FOLDER_MENU}space="))?
    {
        items.push(Box::new(spaces));
    }
    items.push(Box::new(PredefinedMenuItem::separator(handle)?));
    items.push(entry(handle, "delete", "Delete Folder and Tabs", true)?);
    Menu::with_items(handle, &refs(&items))
}

/// "Move to Folder ▸" for the tab menu: the space's folders, indented by
/// depth, then a new one; and the way out when the tab is in one.
pub(crate) fn tab_menu_section(
    handle: &AppHandle,
    prefix: &str,
    in_folder: bool,
) -> tauri::Result<Option<Submenu<tauri::Wry>>> {
    let folders = FOLDERS.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let mut items: Items = Vec::new();
    for (id, name, depth) in &folders {
        let label = format!(
            "{}{}",
            "    ".repeat((*depth).min(4)),
            super::extension_menu_label(name)
        );
        items.push(Box::new(
            MenuItemBuilder::with_id(format!("{prefix}{TAB_MENU_FOLDER}{id}"), label)
                .build(handle)?,
        ));
    }
    if !items.is_empty() {
        items.push(Box::new(PredefinedMenuItem::separator(handle)?));
    }
    items.push(Box::new(
        MenuItemBuilder::with_id(
            format!("{prefix}{TAB_MENU_NEW_FOLDER}"),
            menu_text::label("New Folder…"),
        )
        .build(handle)?,
    ));
    if in_folder {
        items.push(Box::new(
            MenuItemBuilder::with_id(
                format!("{prefix}{TAB_MENU_UNFOLDER}"),
                menu_text::label("Remove from Folder"),
            )
            .build(handle)?,
        ));
    }
    SubmenuBuilder::new(handle, menu_text::label("Move to Folder"))
        .items(&refs(&items))
        .build()
        .map(Some)
}

/// A tab menu action naming a folder, or asking for a new one or out.
pub(crate) fn tab_action(app: &AppHandle, tab: ItemId, action: &str) -> Option<OperationAdmission> {
    let shell = || app.try_state::<Handle>();
    if let Some(folder) = action.strip_prefix(TAB_MENU_FOLDER) {
        return Some(match (shell(), item(folder)) {
            (Some(shell), Some(folder)) => dispatch(
                app,
                &shell,
                FolderCommand::Move {
                    id: tab,
                    parent: Some(folder),
                    before: None,
                },
            ),
            _ => super::rejected_operation(),
        });
    }
    match action {
        TAB_MENU_UNFOLDER => Some(shell().map_or_else(super::rejected_operation, |shell| {
            dispatch(app, &shell, FolderCommand::RemoveFromFolder(tab))
        })),
        TAB_MENU_NEW_FOLDER => Some(emit(app, TAB_NEW_FOLDER_COMMAND)),
        _ => None,
    }
}

fn emit(app: &AppHandle, command: &str) -> OperationAdmission {
    if super::try_emit_to_privileged(app, super::MAIN_LABEL, super::EVENT_UI, &command) {
        super::accepted_ui_operation()
    } else {
        super::rejected_operation()
    }
}

/// Menu ids this module owns; anything else is not a folder action.
pub(crate) fn execute(app: &AppHandle, id: &str) -> Option<OperationAdmission> {
    let action = id.strip_prefix(FOLDER_MENU)?;
    // Chrome keeps the folder it opened the menu on for what it edits
    // itself; everything else is armed here and taken once.
    match action {
        "rename" => return Some(emit(app, RENAME_COMMAND)),
        "subfolder" => return Some(emit(app, SUBFOLDER_COMMAND)),
        _ => {}
    }
    let (Some(target), Some(shell)) = (
        app.try_state::<FolderMenuTarget>(),
        app.try_state::<Handle>(),
    ) else {
        return Some(super::rejected_operation());
    };
    let Some(folder) = target.take() else {
        return Some(super::rejected_operation());
    };
    let command = if let Some(color) = action.strip_prefix("color=") {
        match COLORS.iter().find(|(id, _, _)| *id == color) {
            Some((_, color, _)) => FolderCommand::SetColor {
                id: folder,
                color: *color,
            },
            None => return Some(super::rejected_operation()),
        }
    } else if let Some(space) = action.strip_prefix("space=") {
        match SpaceId::parse(space).filter(|id| id.to_string() == space) {
            Some(space) => FolderCommand::MoveToSpace { id: folder, space },
            None => return Some(super::rejected_operation()),
        }
    } else {
        match action {
            "closeTabs" => FolderCommand::CloseTabs(folder),
            "ungroup" => FolderCommand::Ungroup(folder),
            "delete" => FolderCommand::Delete(folder),
            _ => return Some(super::rejected_operation()),
        }
    };
    Some(dispatch(app, &shell, command))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id() -> String {
        ItemId::generate().to_string()
    }

    #[test]
    fn folder_intents_are_admitted_only_with_parsed_ids_and_bounded_text() {
        let good = id();
        assert!(matches!(
            command(FolderIntent::Rename {
                id: good.clone(),
                name: "Trips".into()
            }),
            Some(FolderCommand::Rename { .. })
        ));
        for refused in [
            FolderIntent::Rename {
                id: "not an id".into(),
                name: "x".into(),
            },
            FolderIntent::Rename {
                id: good.clone(),
                name: "x".repeat(MAX_FOLDER_NAME_BYTES + 1),
            },
            FolderIntent::Move {
                id: good.clone(),
                parent: Some("x".repeat(super::super::MAX_ITEM_ID_BYTES + 1)),
                before: None,
            },
            FolderIntent::Create {
                name: "x".into(),
                color: None,
                tabs: vec![good.clone(), "bogus".into()],
                parent: None,
                before: None,
            },
            FolderIntent::Create {
                name: "x".into(),
                color: None,
                tabs: vec![good.clone(); zephium_core::session::MAX_SESSION_ITEMS + 1],
                parent: None,
                before: None,
            },
            FolderIntent::Delete { id: String::new() },
        ] {
            assert!(command(refused.clone()).is_none(), "{refused:?}");
        }
        assert!(matches!(
            command(FolderIntent::Create {
                name: String::new(),
                color: Some(FolderColor::Mint),
                tabs: vec![good.clone()],
                parent: None,
                before: Some(good),
            }),
            Some(FolderCommand::Create { tabs, .. }) if tabs.len() == 1
        ));
    }

    #[test]
    fn every_folder_menu_label_has_an_english_message() {
        let english: std::collections::HashMap<String, String> =
            serde_json::from_str(include_str!("../../frame/messages/en.json")).unwrap();
        let entries = include_str!("folders.rs")
            .split("entry(handle, \"")
            .skip(1)
            .filter_map(|rest| rest.split_once("\", \"")?.1.split_once('"'))
            .map(|(label, _)| label);
        let labels: Vec<&str> = COLORS
            .iter()
            .map(|(_, _, title)| *title)
            .chain(entries)
            .collect();
        assert_eq!(labels.len(), 12);
        for label in labels {
            assert_eq!(
                english.get(&menu_text::key(label)).map(String::as_str),
                Some(label),
                "{label}"
            );
        }
    }

    #[test]
    fn the_tab_menu_lists_the_projected_folders_by_depth() {
        let node = |id: &str, parent: Option<&str>, name: &str| zephium_ipc::SidebarNodeView {
            id: id.into(),
            parent_id: parent.map(Into::into),
            section: zephium_ipc::SidebarSectionView::Today,
            kind: SidebarNodeKindView::Folder {
                name: name.into(),
                color: None,
                collapsed: false,
            },
        };
        let state = ItemsState {
            projection_revision: String::new(),
            profile: None,
            spaces: Vec::new(),
            active_space_id: None,
            nodes: vec![node("a", None, "Outer"), node("b", Some("a"), "Inner")],
            tabs: Vec::new(),
            active: None,
            split_group: None,
        };
        follow(&state);
        let listed = FOLDERS.lock().unwrap().clone();
        assert_eq!(
            listed,
            [
                ("a".into(), "Outer".into(), 0),
                ("b".into(), "Inner".into(), 1)
            ]
        );
    }
}
