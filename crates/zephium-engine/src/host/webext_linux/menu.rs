use super::*;
use gtk::gio;
use webkit2gtk::{ContextMenuExt, ContextMenuItemExt, HitTestResultExt};
use zephium_webext_runtime::api::context_menus::{MenuContext, MenuItem};

pub(super) fn attach(
    side: &Rc<EngineSide>,
    tab: ItemId,
    view: &webkit2gtk::WebView,
) -> glib::SignalHandlerId {
    let side = Rc::downgrade(side);
    view.connect_context_menu(move |view, menu, _, hit| {
        let Some(side) = side.upgrade() else {
            return false;
        };
        let context = MenuContext {
            menu_generation: side
                .runtime
                .borrow()
                .as_ref()
                .map_or(0, ProfileRuntime::context_menu_generation),
            document_id: side
                .runtime
                .borrow()
                .as_ref()
                .and_then(|runtime| runtime.context_menu_document(tab)),
            page_url: view.uri().map(|url| url.to_string()).unwrap_or_default(),
            link_url: hit.link_uri().map(|url| url.to_string()),
            src_url: hit.image_uri().map(|url| url.to_string()),
            media_type: hit.context_is_image().then(|| "image".into()),
            editable: hit.context_is_editable(),
            selection: hit.context_is_selection(),
        };
        let items = side
            .runtime
            .borrow()
            .as_ref()
            .map(|runtime| runtime.context_menu_items(tab, &context))
            .unwrap_or_default();
        if items.is_empty() {
            return false;
        }
        menu.append(&webkit2gtk::ContextMenuItem::new_separator());
        append(menu, &side, tab, &context, &items, None, 0);
        false
    })
}

fn append(
    menu: &webkit2gtk::ContextMenu,
    side: &Rc<EngineSide>,
    tab: ItemId,
    context: &MenuContext,
    items: &[(ExtensionInstallId, MenuItem)],
    parent: Option<(ExtensionInstallId, &str)>,
    depth: usize,
) {
    if depth >= 32 {
        return;
    }
    for (install, item) in items {
        let item_parent = item.properties["parentId"]
            .as_str()
            .map(|id| (*install, id));
        if item_parent != parent {
            continue;
        }
        let title = item.properties["title"]
            .as_str()
            .unwrap_or_default()
            .replace("%s", "");
        if item.properties["type"] == "separator" {
            menu.append(&webkit2gtk::ContextMenuItem::new_separator());
            continue;
        }
        if items.iter().any(|(child_install, child)| {
            child_install == install && child.properties["parentId"].as_str() == Some(&item.id)
        }) {
            let submenu = webkit2gtk::ContextMenu::new();
            append(
                &submenu,
                side,
                tab,
                context,
                items,
                Some((*install, &item.id)),
                depth + 1,
            );
            let action = gio::SimpleAction::new("extension-submenu", None);
            action.set_enabled(item.properties["enabled"] != false);
            let entry = webkit2gtk::ContextMenuItem::from_gaction(&action, &title, None);
            entry.set_submenu(Some(&submenu));
            menu.append(&entry);
            continue;
        }
        let action = if matches!(item.properties["type"].as_str(), Some("checkbox" | "radio")) {
            gio::SimpleAction::new_stateful(
                "extension-menu",
                None,
                &(item.properties["checked"] == true).to_variant(),
            )
        } else {
            gio::SimpleAction::new("extension-menu", None)
        };
        action.set_enabled(item.properties["enabled"] != false);
        let weak = Rc::downgrade(side);
        let (install, id, context) = (*install, item.id.clone(), context.clone());
        action.connect_activate(move |_, _| {
            if let Some(side) = weak.upgrade() {
                if let Some(runtime) = side.runtime.borrow().as_ref() {
                    let _ = runtime.context_menu_clicked(install, tab, &id, &context);
                }
            }
        });
        menu.append(&webkit2gtk::ContextMenuItem::from_gaction(
            &action, &title, None,
        ));
    }
}
