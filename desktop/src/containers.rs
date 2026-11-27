//! Privileged command surface for website-data containers.
use tauri::{AppHandle, Manager, State, WebviewWindow};
use zephium_app::{Command, ContainerCommand, Handle};
use zephium_core::containers::SpaceAccess;
use zephium_core::ids::{ContainerId, ItemId, SpaceId};
use zephium_core::spaces::{SpaceIcon, SpaceLook};
use zephium_ipc::{OperationAdmission, SpaceIconView};

fn admitted(caller: &WebviewWindow, command: &str) -> bool {
    super::authorize(caller, super::CallerPolicy::Main, command)
        && !super::shutdown_started(caller.app_handle())
}

fn dispatch(app: &AppHandle, shell: &Handle, command: ContainerCommand) -> OperationAdmission {
    super::dispatch_operation(app, shell, Command::Container(Box::new(command)))
}

fn id(value: &str) -> Option<ContainerId> {
    ContainerId::parse(value).filter(|id| id.to_string() == value)
}

fn tab_id(value: &str) -> Option<ItemId> {
    ItemId::parse(value).filter(|id| id.to_string() == value)
}

fn look(icon: Option<SpaceIconView>, color: u8) -> Option<SpaceLook> {
    let look = SpaceLook {
        icon: icon.map(|icon| match icon {
            SpaceIconView::Symbol { index } => SpaceIcon::Symbol(index),
            SpaceIconView::Emoji { text } => SpaceIcon::Emoji(text),
        }),
        color,
        theme: None,
    };
    look.valid().then_some(look)
}

fn access(spaces: Option<Vec<String>>) -> Option<SpaceAccess> {
    let Some(spaces) = spaces else {
        return Some(SpaceAccess::All);
    };
    let ids = spaces
        .iter()
        .map(|value| SpaceId::parse(value).filter(|id| id.to_string() == *value))
        .collect::<Option<Vec<_>>>()?;
    Some(SpaceAccess::Selected(ids))
}

#[tauri::command]
#[specta::specta]
pub(crate) fn containers_create(
    caller: WebviewWindow,
    shell: State<'_, Handle>,
    name: String,
    icon: Option<SpaceIconView>,
    color: u8,
    spaces: Option<Vec<String>>,
) -> OperationAdmission {
    let (Some(look), Some(spaces)) = (look(icon, color), access(spaces)) else {
        return super::rejected_operation();
    };
    if !admitted(&caller, "containers_create") || !super::bounded(&name, 256) {
        return super::rejected_operation();
    }
    dispatch(
        caller.app_handle(),
        &shell,
        ContainerCommand::Create { name, look, spaces },
    )
}

#[tauri::command]
#[specta::specta]
pub(crate) fn containers_update(
    caller: WebviewWindow,
    shell: State<'_, Handle>,
    container: String,
    name: String,
    icon: Option<SpaceIconView>,
    color: u8,
    spaces: Option<Vec<String>>,
) -> OperationAdmission {
    let (Some(id), Some(look), Some(spaces)) = (id(&container), look(icon, color), access(spaces))
    else {
        return super::rejected_operation();
    };
    if !admitted(&caller, "containers_update") || !super::bounded(&name, 256) {
        return super::rejected_operation();
    }
    dispatch(
        caller.app_handle(),
        &shell,
        ContainerCommand::Update {
            id,
            name,
            look,
            spaces,
        },
    )
}

#[tauri::command]
#[specta::specta]
pub(crate) fn containers_remove_from_space(
    caller: WebviewWindow,
    shell: State<'_, Handle>,
    container: String,
) -> OperationAdmission {
    let Some(id) = id(&container) else {
        return super::rejected_operation();
    };
    if !admitted(&caller, "containers_remove_from_space") {
        return super::rejected_operation();
    }
    dispatch(
        caller.app_handle(),
        &shell,
        ContainerCommand::RemoveFromSpace(id),
    )
}

#[tauri::command]
#[specta::specta]
pub(crate) fn containers_assign_tab(
    caller: WebviewWindow,
    shell: State<'_, Handle>,
    tab: String,
    container: Option<String>,
) -> OperationAdmission {
    let (Some(tab), Some(container)) = (
        tab_id(&tab),
        container.map_or(Some(None), |value| id(&value).map(Some)),
    ) else {
        return super::rejected_operation();
    };
    if !admitted(&caller, "containers_assign_tab") {
        return super::rejected_operation();
    }
    dispatch(
        caller.app_handle(),
        &shell,
        ContainerCommand::AssignTab { tab, container },
    )
}
