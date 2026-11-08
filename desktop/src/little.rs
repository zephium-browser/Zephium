//! Little windows: the native window around one page the shell keeps out of
//! the sidebar. Each has its own privileged toolbar (`little.html`) above a
//! content stage the engine builds inside it. The window is named by its
//! label, `little-<id>`, so a toolbar can only ever act on its own page.

use serde::Deserialize;
use std::sync::Arc;
use tauri::{Manager, State, WebviewWindow};
use zephium_app::{
    ChromePresentation, ChromePresentationCallback, ChromePresentationDispatch, Command, Handle,
    LittleAction,
};
use zephium_ipc::{LittleClosedView, LittleOpenedView, LittleView};

use crate::{authorize, emit_to_privileged, shutdown_started, CallerPolicy};

const LABEL_PREFIX: &str = "little-";
pub(crate) const EVENT_LITTLE: &str = "zephium:little";
const EVENT_LITTLE_PRESENTATION: &str = "zephium:little-presentation";
/// Space menu items: `little.space=<window>:<space>`.
const SPACE_MENU: &str = "little.space=";

/// Each open window's latest view, which its space menu is built from.
static VIEWS: std::sync::Mutex<Option<std::collections::HashMap<u64, LittleView>>> =
    std::sync::Mutex::new(None);

fn views() -> std::sync::MutexGuard<'static, Option<std::collections::HashMap<u64, LittleView>>> {
    VIEWS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}
const MIN_SIZE: (f64, f64) = (360.0, 240.0);

pub(crate) fn label(window: u64) -> String {
    format!("{LABEL_PREFIX}{window}")
}

/// The window a label names: canonical decimal only, so no two labels name
/// one window.
pub(crate) fn window_of(label: &str) -> Option<u64> {
    let digits = label.strip_prefix(LABEL_PREFIX)?;
    if digits.is_empty()
        || digits.len() > 20
        || digits.starts_with('0')
        || !digits.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    digits.parse().ok()
}

/// Labels of the Little windows open now.
pub(crate) fn labels(app: &tauri::AppHandle) -> Vec<String> {
    app.webview_windows()
        .into_keys()
        .filter(|label| window_of(label).is_some())
        .collect()
}

/// The page action a menu command means when a Little window is in front.
/// Menu key equivalents reach whichever window is key, so without this
/// Cmd+W in a Little window would close the main window's tab.
fn page_action(id: &str) -> Option<LittleAction> {
    match id {
        "tab.close" => Some(LittleAction::Close),
        "nav.reload" => Some(LittleAction::Reload),
        "nav.back" => Some(LittleAction::Back),
        "nav.forward" => Some(LittleAction::Forward),
        _ => None,
    }
}

/// The window and space a space menu item names.
fn space_choice(id: &str) -> Option<(u64, zephium_core::ids::SpaceId)> {
    let (window, space) = id.strip_prefix(SPACE_MENU)?.split_once(':')?;
    Some((
        window_of(&format!("{LABEL_PREFIX}{window}"))?,
        zephium_core::ids::SpaceId::parse(space)?,
    ))
}

/// Handles a menu event meant for a Little window: a space picked from its
/// space menu, or a page command while a Little window is in front. Returns
/// whether the event was taken.
pub(crate) fn on_menu(app: &tauri::AppHandle, id: &str) -> bool {
    if id.starts_with(SPACE_MENU) {
        if let (Some((window, space)), Some(shell)) = (space_choice(id), app.try_state::<Handle>())
        {
            shell.dispatch(Command::Little {
                window,
                action: LittleAction::OpenInSpace(space),
            });
        }
        return true;
    }
    let action = page_action(id);
    if action.is_none() && id != "url.focus" {
        return false;
    }
    let Some(window) = focused(app) else {
        return false;
    };
    match action {
        Some(action) => {
            if let Some(shell) = app.try_state::<Handle>() {
                shell.dispatch(Command::Little { window, action });
            }
        }
        None => {
            edit_address(app, window);
        }
    }
    true
}

/// The Little window in front, if one is.
pub(crate) fn focused(app: &tauri::AppHandle) -> Option<u64> {
    app.webview_windows()
        .into_iter()
        .find_map(|(label, window)| {
            let id = window_of(&label)?;
            window.is_focused().unwrap_or(false).then_some(id)
        })
}

/// Brings forward the Little window showing a page. Returns whether one does.
pub(crate) fn focus_page(app: &tauri::AppHandle, page: &str) -> bool {
    let window = views().as_ref().and_then(|views| {
        views
            .values()
            .find(|view| view.tab.as_ref().is_some_and(|tab| tab.id == page))
            .map(|view| view.window)
    });
    window
        .and_then(|window| app.get_webview_window(&label(window)))
        .is_some_and(|window| window.set_focus().is_ok())
}

/// Opens the launcher on the page's address, as an address bar would. The
/// page may have no address yet; the field then starts empty.
fn edit_address(app: &tauri::AppHandle, window: u64) -> bool {
    let Some(tab) = views()
        .as_ref()
        .and_then(|views| views.get(&window)?.tab.clone())
    else {
        return false;
    };
    let url = tab.url.unwrap_or_default();
    crate::overlay::request(app, move |panel| panel.edit(tab.id, url, true))
}

/// Pops the spaces "Open in Zephium" can put the page in, under the
/// toolbar's button at `x`, `y`.
#[tauri::command]
#[specta::specta]
pub(crate) fn little_space_menu(caller: WebviewWindow, x: f64, y: f64) -> bool {
    if !authorize(&caller, CallerPolicy::Little, "little_space_menu") {
        return false;
    }
    let Some(window) = window_of(caller.label()) else {
        return false;
    };
    let Some(size) = crate::platform::imp::window_content_size(&caller) else {
        return false;
    };
    let Some(anchor) = crate::menu_popup_anchor(x, y, size.width, size.height) else {
        return false;
    };
    let Some(view) = views()
        .as_ref()
        .and_then(|views| views.get(&window).cloned())
    else {
        return false;
    };
    if view.tab.is_none() || view.spaces.is_empty() {
        return false;
    }
    let menu = space_menu(caller.app_handle(), window, &view);
    menu.is_ok_and(|menu| caller.popup_menu_at(&menu, anchor).is_ok())
}

fn space_menu(
    app: &tauri::AppHandle,
    window: u64,
    view: &LittleView,
) -> tauri::Result<tauri::menu::Menu<tauri::Wry>> {
    use tauri::menu::{CheckMenuItemBuilder, IsMenuItem, Menu};
    // A space's name is the person's own text, shown as it is.
    let items = view
        .spaces
        .iter()
        .map(|space| {
            CheckMenuItemBuilder::with_id(format!("{SPACE_MENU}{window}:{}", space.id), &space.name)
                .checked(view.space.as_deref() == Some(space.id.as_str()))
                .build(app)
        })
        .collect::<tauri::Result<Vec<_>>>()?;
    let refs: Vec<&dyn IsMenuItem<tauri::Wry>> = items
        .iter()
        .map(|item| item as &dyn IsMenuItem<tauri::Wry>)
        .collect();
    Menu::with_items(app, &refs)
}

/// What builds Little windows: the toolbar's address and the engine that
/// hosts their pages.
struct Factory {
    url: tauri::Url,
    #[cfg(target_os = "macos")]
    engine: Arc<zephium_engine::WebviewEngine>,
}

pub(crate) fn install(
    app: &tauri::AppHandle,
    engine: Arc<zephium_engine::WebviewEngine>,
    url: tauri::Url,
) {
    #[cfg(not(target_os = "macos"))]
    let _ = engine;
    app.manage(Factory {
        url,
        #[cfg(target_os = "macos")]
        engine,
    });
}

/// Builds the native window for a Little window the shell opened. A window
/// that cannot be built is closed again in the shell.
pub(crate) fn open(app: &tauri::AppHandle, opened: LittleOpenedView) {
    let handle = app.clone();
    let scheduled = app.run_on_main_thread(move || {
        if !build(&handle, opened) {
            crate::write_diagnostic(format_args!("little: window {} not built", opened.window));
            if let Some(shell) = handle.try_state::<Handle>() {
                let _ = shell.dispatch(Command::Little {
                    window: opened.window,
                    action: LittleAction::Close,
                });
            }
        }
    });
    if scheduled.is_err() {
        if let Some(shell) = app.try_state::<Handle>() {
            let _ = shell.dispatch(Command::Little {
                window: opened.window,
                action: LittleAction::Close,
            });
        }
    }
}

#[cfg(not(target_os = "macos"))]
fn build(_app: &tauri::AppHandle, _opened: LittleOpenedView) -> bool {
    false
}

#[cfg(target_os = "macos")]
fn build(app: &tauri::AppHandle, opened: LittleOpenedView) -> bool {
    use raw_window_handle::HasWindowHandle;

    if shutdown_started(app) {
        return false;
    }
    let Some(factory) = app.try_state::<Factory>() else {
        return false;
    };
    let label = label(opened.window);
    if app.get_webview_window(&label).is_some() {
        return false;
    }
    let Ok(bootstrap) = tauri::Url::parse(crate::PRIVILEGED_BOOTSTRAP_URL) else {
        return false;
    };
    let size = zephium_app::LITTLE_WINDOW_SIZE;
    let built =
        tauri::WebviewWindowBuilder::new(app, &label, tauri::WebviewUrl::External(bootstrap))
            .title("Zephium")
            .incognito(true)
            .devtools(cfg!(debug_assertions))
            .general_autofill_enabled(false)
            .initialization_script_for_all_frames(zephium_engine::PAGE_PRINT_DENY_SCRIPT)
            .on_download(|_, _| false)
            .on_new_window(|_, _| tauri::webview::NewWindowResponse::Deny)
            .on_web_resource_request(|_, response| {
                crate::harden_privileged_headers(response.headers_mut())
            })
            .inner_size(size.width, size.height)
            .min_inner_size(MIN_SIZE.0, MIN_SIZE.1)
            .center()
            .title_bar_style(tauri::TitleBarStyle::Overlay)
            .hidden_title(true)
            // Measured on screen: this centres the lights on the 40px toolbar.
            .traffic_light_position(tauri::LogicalPosition::new(9.0, 23.0))
            .visible(false)
            .build();
    let window = match built {
        Ok(window) => window,
        Err(error) => {
            crate::write_diagnostic(format_args!("little: window build failed: {error}"));
            return false;
        }
    };
    let fail = |window: &WebviewWindow| {
        let _ = window.destroy();
        false
    };
    if !crate::platform::imp::harden_privileged(&window)
        || !crate::platform::imp::fit_toolbar(&window, zephium_app::LITTLE_TOOLBAR_HEIGHT)
    {
        return fail(&window);
    }
    let Ok(handle) = window.window_handle().map(|handle| handle.as_raw()) else {
        return fail(&window);
    };
    if !factory.engine.attach_window(opened.window, handle) {
        return fail(&window);
    }
    let events = window.clone();
    let id = opened.window;
    window.on_window_event(move |event| {
        let app = events.app_handle();
        let Some(shell) = app.try_state::<Handle>() else {
            return;
        };
        match event {
            tauri::WindowEvent::CloseRequested { api, .. } if !shutdown_started(app) => {
                api.prevent_close();
                crate::write_diagnostic(format_args!("little: window {id} close requested"));
                let _ = shell.dispatch(Command::Little {
                    window: id,
                    action: LittleAction::Close,
                });
            }
            // Moving to this window puts the launcher away.
            tauri::WindowEvent::Focused(_) => {
                if let Some(panel) = app.try_state::<crate::overlay::Overlay>() {
                    panel.focus_changed();
                }
            }
            tauri::WindowEvent::Resized(_) => {
                if let Some(size) = crate::platform::imp::window_content_size(&events) {
                    let _ = shell.dispatch(Command::LittleResized { window: id, size });
                }
            }
            _ => {}
        }
    });
    if window.navigate(factory.url.clone()).is_err() {
        return fail(&window);
    }
    true
}

/// Takes the native window away once the shell let it go. When its page
/// moved into the main window, that window comes forward to show it.
pub(crate) fn close(app: &tauri::AppHandle, closed: LittleClosedView) {
    crate::write_diagnostic(format_args!(
        "little: window {} closed by the shell, reveal_main={}",
        closed.window, closed.reveal_main
    ));
    if let Some(views) = views().as_mut() {
        views.remove(&closed.window);
    }
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || {
        if let Some(window) = handle.get_webview_window(&label(closed.window)) {
            let _ = window.destroy();
        }
        if closed.reveal_main {
            crate::external_links::bring_forward(&handle);
        }
    });
}

pub(crate) fn project(app: &tauri::AppHandle, view: &LittleView) {
    views()
        .get_or_insert_with(Default::default)
        .insert(view.window, view.clone());
    emit_to_privileged(app, &label(view.window), EVENT_LITTLE, view);
}

/// The presentation barrier for a Little window's page. The toolbar applies
/// the exact revision and the eval reads its DOM back in the same turn: the
/// page id, its URL, its title and the host it shows must all match before
/// the shell lets the page appear.
pub(crate) fn apply_presentation(
    app: &tauri::AppHandle,
    window: u64,
    presentation: ChromePresentation,
    done: ChromePresentationCallback,
) -> ChromePresentationDispatch {
    if presentation.tab.id != presentation.id.to_string()
        || presentation.tab.url.as_deref() != Some(presentation.url.as_str())
        || presentation.active != Some(presentation.id)
    {
        return ChromePresentationDispatch::Rejected;
    }
    let Some(toolbar) = app.get_webview_window(&label(window)) else {
        return ChromePresentationDispatch::Rejected;
    };
    let (Ok(event), Ok(payload)) = (
        serde_json::to_string(EVENT_LITTLE_PRESENTATION),
        serde_json::to_string(&presentation.tab),
    ) else {
        return ChromePresentationDispatch::Rejected;
    };
    let nonce = format!(
        "zephium-little-presentation-v1:{window}:{}:{:032x}",
        presentation.id,
        presentation.navigation.into_raw()
    );
    let Ok(serialized_nonce) = serde_json::to_string(&nonce) else {
        return ChromePresentationDispatch::Rejected;
    };
    let script = format!(
        r#"(() => {{
  "use strict";
  const rejected = "zephium-presentation-rejected";
  try {{
    const tab = {payload};
    window.dispatchEvent(new CustomEvent({event}, {{ detail: {{ tab }} }}));
    const shell = document.querySelector("[data-zephium-little-tab]");
    if (!shell || shell.dataset.zephiumLittleTab !== tab.id) return rejected;
    if (shell.dataset.zephiumTabUrl !== (tab.url ?? "") ||
        shell.dataset.zephiumProjectionRevision !== tab.projection_revision) return rejected;
    const label = shell.querySelector("[data-zephium-tab-label]");
    if (!label || label.textContent !== tab.title) return rejected;
    let expected = "";
    try {{ expected = new URL(tab.url).host; }} catch (_) {{ return rejected; }}
    const address = shell.querySelector("[data-zephium-address]");
    if (!address || address.textContent !== expected) return rejected;
    void document.documentElement.getBoundingClientRect();
    return {serialized_nonce};
  }} catch (_) {{
    return rejected;
  }}
}})()"#
    );
    let expected = nonce;
    let done = std::sync::Mutex::new(Some(done));
    match toolbar.eval_with_callback(script, move |result| {
        let applied =
            serde_json::from_str::<String>(&result).is_ok_and(|returned| returned == expected);
        if let Some(done) = done
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
        {
            done(applied);
        }
    }) {
        Ok(()) => ChromePresentationDispatch::Scheduled,
        Err(_) => ChromePresentationDispatch::Rejected,
    }
}

/// What a Little window's toolbar may ask of its page.
#[derive(Clone, Copy, Debug, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub(crate) enum LittleIntent {
    Back,
    Forward,
    Reload,
    OpenInMain,
    Close,
    /// Opens the launcher on the page's address.
    EditAddress,
}

/// The toolbar finished its first render: the window shows, and the shell
/// lays its page out and lets it be verified.
#[tauri::command]
#[specta::specta]
pub(crate) fn little_ready(caller: WebviewWindow, shell: State<'_, Handle>) -> bool {
    if !authorize(&caller, CallerPolicy::Little, "little_ready")
        || shutdown_started(caller.app_handle())
    {
        return false;
    }
    let (Some(window), Some(size)) = (
        window_of(caller.label()),
        crate::platform::imp::window_content_size(&caller),
    ) else {
        return false;
    };
    let _ = caller.show();
    let _ = caller.set_focus();
    shell.dispatch(Command::LittleAttached { window, size })
}

#[tauri::command]
#[specta::specta]
pub(crate) fn little_action(
    caller: WebviewWindow,
    shell: State<'_, Handle>,
    action: LittleIntent,
) -> bool {
    if !authorize(&caller, CallerPolicy::Little, "little_action") {
        return false;
    }
    let Some(window) = window_of(caller.label()) else {
        return false;
    };
    let action = match action {
        LittleIntent::EditAddress => return edit_address(caller.app_handle(), window),
        LittleIntent::Back => LittleAction::Back,
        LittleIntent::Forward => LittleAction::Forward,
        LittleIntent::Reload => LittleAction::Reload,
        LittleIntent::OpenInMain => LittleAction::OpenInMain,
        LittleIntent::Close => LittleAction::Close,
    };
    shell.dispatch(Command::Little { window, action })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_space_menu_item_names_its_window_and_space() {
        let space = zephium_core::ids::SpaceId::generate();
        assert_eq!(
            space_choice(&format!("{SPACE_MENU}3:{space}")),
            Some((3, space))
        );
        for id in [
            format!("{SPACE_MENU}03:{space}"),
            format!("{SPACE_MENU}3"),
            format!("{SPACE_MENU}3:not-a-space"),
            format!("{SPACE_MENU}0:{space}"),
        ] {
            assert_eq!(space_choice(&id), None, "{id}");
        }
    }

    #[test]
    fn page_commands_from_the_menu_act_on_the_little_page() {
        assert!(matches!(
            page_action("tab.close"),
            Some(LittleAction::Close)
        ));
        assert!(matches!(
            page_action("nav.reload"),
            Some(LittleAction::Reload)
        ));
        assert!(matches!(page_action("nav.back"), Some(LittleAction::Back)));
        assert!(matches!(
            page_action("nav.forward"),
            Some(LittleAction::Forward)
        ));
        assert!(page_action("tab.new").is_none());
        assert!(page_action("window.newLittle").is_none());
    }

    #[test]
    fn only_canonical_labels_name_a_window() {
        assert_eq!(window_of("little-2"), Some(2));
        assert_eq!(window_of(&label(41)), Some(41));
        for label in [
            "little-",
            "little-0",
            "little-02",
            "little-2a",
            "little--2",
            "little-+2",
            "main",
            "panel",
            "Little-2",
            "little-99999999999999999999999",
        ] {
            assert_eq!(window_of(label), None, "{label}");
        }
    }
}
