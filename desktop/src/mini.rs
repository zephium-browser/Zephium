//! Mini windows: the native window around one page the shell keeps out of
//! the sidebar. Each has its own privileged toolbar (`mini.html`) above a
//! content stage the engine builds inside it. The window is named by its
//! label, `mini-<id>`, so a toolbar can only ever act on its own page.
//!
//! macOS gives the window a transparent title bar, its lights inside the
//! toolbar. Windows draws no frame: the toolbar carries the window's
//! controls and drags it, and Tauri's resize ring stays reachable around the
//! page. On Windows every toolbar joins the launcher's WebView2 browser, so
//! no window adds a privileged browser that teardown would not prove gone.

use serde::Deserialize;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};
use tauri::{Manager, State, WebviewWindow};
use zephium_app::{
    ChromePresentation, ChromePresentationCallback, ChromePresentationDispatch, Command, Handle,
    MiniAction,
};
use zephium_core::geometry::Size;
use zephium_ipc::{MiniClosedView, MiniOpenedView, MiniView, OperationAdmission};

use crate::{authorize, emit_to_privileged, shutdown_started, CallerPolicy};

const LABEL_PREFIX: &str = "mini-";
pub(crate) const EVENT_MINI: &str = "zephium:mini";
const EVENT_MINI_PRESENTATION: &str = "zephium:mini-presentation";
/// Space menu items: `mini.space=<window>:<space>`.
const SPACE_MENU: &str = "mini.space=";
/// How far each further open mini window steps from the last, so a new one
/// never lands exactly on another.
const CASCADE: f64 = 28.0;

/// Each open window's latest view, which its space menu is built from, and
/// whether it was last put above other windows.
static VIEWS: Mutex<Option<HashMap<u64, (MiniView, bool)>>> = Mutex::new(None);

fn views() -> MutexGuard<'static, Option<HashMap<u64, (MiniView, bool)>>> {
    VIEWS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

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

/// Labels of the mini windows open now.
pub(crate) fn labels(app: &tauri::AppHandle) -> Vec<String> {
    app.webview_windows()
        .into_keys()
        .filter(|label| window_of(label).is_some())
        .collect()
}

/// What a command means for a mini window: the page actions, the address,
/// and the keys only a mini window answers. Others go to the main window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Routed {
    Act(MiniAction),
    EditAddress,
}

fn routed(id: &str) -> Option<Routed> {
    Some(match zephium_core::commands::runs_as(id) {
        "tab.close" => Routed::Act(MiniAction::Close),
        "nav.reload" | "nav.hardReload" => Routed::Act(MiniAction::Reload),
        "nav.back" => Routed::Act(MiniAction::Back),
        "nav.forward" => Routed::Act(MiniAction::Forward),
        "mini.openInMain" => Routed::Act(MiniAction::OpenInMain),
        "mini.keepOnTop" => Routed::Act(MiniAction::KeepOnTop(None)),
        "url.focus" => Routed::EditAddress,
        _ => return None,
    })
}

/// Carries out `id` for the mini window in front, when one is and the
/// command is one it answers. Menu key equivalents and the browser's own
/// keys reach whichever window is in front, so without this Cmd+W in a mini
/// window would close the main window's tab. `None` when no mini window
/// answers `id`; otherwise whether it took the command. A refusal is final:
/// the command never falls through to the main window.
pub(crate) fn route(app: &tauri::AppHandle, id: &str) -> Option<OperationAdmission> {
    let route = routed(id)?;
    let window = focused(app)?;
    Some(carry_out(app, window, route))
}

/// The same for a key a page pressed: it acts on that page's own mini
/// window, wherever focus is reported.
pub(crate) fn route_page_key(
    app: &tauri::AppHandle,
    page: &str,
    id: &str,
) -> Option<OperationAdmission> {
    let route = routed(id)?;
    let window = window_showing(page)?;
    Some(carry_out(app, window, route))
}

fn carry_out(app: &tauri::AppHandle, window: u64, route: Routed) -> OperationAdmission {
    match route {
        Routed::EditAddress => crate::ui_admission(edit_address(app, window)),
        Routed::Act(action) => act(app, window, action),
    }
}

/// Sends `action` through the operation ledger, so its settlement reaches
/// the caller like any other shell operation.
fn act(app: &tauri::AppHandle, window: u64, action: MiniAction) -> OperationAdmission {
    match app.try_state::<Handle>() {
        Some(shell) => crate::dispatch_operation(app, &shell, Command::Mini { window, action }),
        None => crate::ui_admission(false),
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

/// Handles a space picked from a mini window's space menu. Returns whether
/// the menu event was one.
pub(crate) fn on_menu(app: &tauri::AppHandle, id: &str) -> bool {
    if !id.starts_with(SPACE_MENU) {
        return false;
    }
    if let Some((window, space)) = space_choice(id) {
        let _ = act(app, window, MiniAction::OpenInSpace(space));
    }
    true
}

/// The mini window in front, if one is.
pub(crate) fn focused(app: &tauri::AppHandle) -> Option<u64> {
    app.webview_windows()
        .into_iter()
        .find_map(|(label, window)| {
            let id = window_of(&label)?;
            window.is_focused().unwrap_or(false).then_some(id)
        })
}

/// The mini window showing a page.
fn window_showing(page: &str) -> Option<u64> {
    views().as_ref().and_then(|views| {
        views
            .values()
            .find(|(view, _)| view.tab.as_ref().is_some_and(|tab| tab.id == page))
            .map(|(view, _)| view.window)
    })
}

/// Brings forward the mini window showing a page. Returns whether one does.
pub(crate) fn focus_page(app: &tauri::AppHandle, page: &str) -> bool {
    window_showing(page)
        .and_then(|window| app.get_webview_window(&label(window)))
        .is_some_and(|window| window.set_focus().is_ok())
}

/// Opens the launcher on the page's address, as an address bar would. The
/// page may have no address yet; the field then starts empty.
fn edit_address(app: &tauri::AppHandle, window: u64) -> bool {
    let Some(tab) = views()
        .as_ref()
        .and_then(|views| views.get(&window)?.0.tab.clone())
    else {
        return false;
    };
    let url = tab.url.unwrap_or_default();
    crate::overlay::request(app, move |panel| panel.edit(tab.id, url, true))
}

/// The size of a window's content area in logical points.
fn content_size(window: &WebviewWindow) -> Option<Size> {
    #[cfg(target_os = "macos")]
    {
        crate::platform::imp::window_content_size(window)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let size = window.inner_size().ok()?;
        let scale = window.scale_factor().ok()?;
        (scale.is_finite() && scale > 0.0).then(|| {
            Size::new(
                f64::from(size.width) / scale,
                f64::from(size.height) / scale,
            )
        })
    }
}

/// Pops the spaces "Open in Zephium" can put the page in, under the
/// toolbar's button at `x`, `y`.
#[tauri::command]
#[specta::specta]
pub(crate) fn mini_space_menu(caller: WebviewWindow, x: f64, y: f64) -> bool {
    if !authorize(&caller, CallerPolicy::Mini, "mini_space_menu") {
        return false;
    }
    let Some(window) = window_of(caller.label()) else {
        return false;
    };
    let Some(size) = content_size(&caller) else {
        return false;
    };
    let Some(anchor) = crate::menu_popup_anchor(x, y, size.width, size.height) else {
        return false;
    };
    let Some(view) = views()
        .as_ref()
        .and_then(|views| views.get(&window).map(|(view, _)| view.clone()))
    else {
        return false;
    };
    if view.tab.is_none() || view.spaces.len() < 2 {
        return false;
    }
    let menu = space_menu(caller.app_handle(), window, &view);
    menu.is_ok_and(|menu| caller.popup_menu_at(&menu, anchor).is_ok())
}

fn space_menu(
    app: &tauri::AppHandle,
    window: u64,
    view: &MiniView,
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

/// What builds mini windows: the toolbar's address, the engine that hosts
/// their pages and, on Windows, the launcher's user-data folder they share.
struct Factory {
    url: tauri::Url,
    engine: Arc<zephium_engine::WebviewEngine>,
    #[cfg(target_os = "windows")]
    runtime: std::path::PathBuf,
}

pub(crate) fn install(
    app: &tauri::AppHandle,
    engine: Arc<zephium_engine::WebviewEngine>,
    url: tauri::Url,
    #[cfg(target_os = "windows")] runtime: std::path::PathBuf,
) {
    app.manage(Factory {
        url,
        engine,
        #[cfg(target_os = "windows")]
        runtime,
    });
}

/// Builds the native window for a mini window the shell opened. A window
/// that cannot be built hands its page back to the shell, which opens it as
/// a tab instead.
pub(crate) fn open(app: &tauri::AppHandle, opened: MiniOpenedView) {
    let window = opened.window;
    let handle = app.clone();
    let build_on_main = move || {
        if !build(&handle, opened) {
            unbuilt(&handle, window);
        }
    };
    // Windows: the toolbar joins the launcher's browser, so that browser is
    // made and registered first. The request runs on the main thread.
    #[cfg(target_os = "windows")]
    let scheduled = crate::overlay::request(app, move |_| build_on_main());
    #[cfg(not(target_os = "windows"))]
    let scheduled = app.run_on_main_thread(build_on_main).is_ok();
    if !scheduled {
        unbuilt(app, window);
    }
}

fn unbuilt(app: &tauri::AppHandle, window: u64) {
    crate::write_diagnostic(format_args!("mini: window {window} not built"));
    if let Some(shell) = app.try_state::<Handle>() {
        let _ = shell.dispatch(Command::Mini {
            window,
            action: MiniAction::Unbuilt,
        });
    }
}

fn build(app: &tauri::AppHandle, opened: MiniOpenedView) -> bool {
    use raw_window_handle::HasWindowHandle;

    // Only macOS and Windows harden a toolbar window; nowhere else builds one.
    if cfg!(not(any(target_os = "macos", target_os = "windows"))) || shutdown_started(app) {
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
    let Some(size) = zephium_app::admitted_mini_size(Size::new(opened.width, opened.height)) else {
        return false;
    };
    let min = zephium_app::MINI_WINDOW_MIN_SIZE;
    let builder =
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
            .inner_size(size.width.max(min.width), size.height.max(min.height))
            .min_inner_size(min.width, min.height)
            .maximizable(false)
            .fullscreen(false)
            .visible(false);
    #[cfg(target_os = "macos")]
    let builder = builder
        .title_bar_style(tauri::TitleBarStyle::Overlay)
        .hidden_title(true)
        // Measured on screen: this centres the lights on the toolbar.
        .traffic_light_position(tauri::LogicalPosition::new(14.0, 20.0));
    #[cfg(target_os = "windows")]
    let builder = builder
        .decorations(false)
        .shadow(true)
        .data_directory(factory.runtime.clone())
        .additional_browser_args(crate::PRIVILEGED_WEBVIEW2_BROWSER_ARGS);
    let window = match builder.build() {
        Ok(window) => window,
        Err(error) => {
            crate::write_diagnostic(format_args!("mini: window build failed: {error}"));
            return false;
        }
    };
    let fail = |window: &WebviewWindow| {
        let _ = window.destroy();
        false
    };
    #[cfg(target_os = "macos")]
    if !crate::platform::imp::harden_privileged(&window)
        || !crate::platform::imp::fit_toolbar(&window, zephium_app::MINI_TOOLBAR_HEIGHT)
    {
        return fail(&window);
    }
    #[cfg(target_os = "windows")]
    {
        crate::platform::imp::round_corners(&window);
        if let Err(error) = crate::platform::imp::harden_privileged(
            &window,
            &factory.runtime,
            crate::platform::imp::PrivilegedEnvironment::Joined(crate::overlay::PANEL_LABEL),
        ) {
            crate::write_diagnostic(format_args!("mini: toolbar refused: {error}"));
            return fail(&window);
        }
    }
    place(app, &window, opened.window);
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
                let _ = shell.dispatch(Command::Mini {
                    window: id,
                    action: MiniAction::Close,
                });
            }
            // Moving to this window puts the launcher away.
            tauri::WindowEvent::Focused(_) => {
                if let Some(panel) = app.try_state::<crate::overlay::Overlay>() {
                    panel.focus_changed();
                }
            }
            tauri::WindowEvent::Resized(_) | tauri::WindowEvent::ScaleFactorChanged { .. } => {
                if let Some(size) = content_size(&events) {
                    let _ = shell.dispatch(Command::MiniResized { window: id, size });
                }
            }
            // WebView2 places popups, IME and accessibility by the parent's
            // position, so a move lays the page out again, as in the main
            // window. A minimized window reports no size and is skipped.
            #[cfg(target_os = "windows")]
            tauri::WindowEvent::Moved(_) if !events.is_minimized().unwrap_or(false) => {
                if let Some(size) = content_size(&events) {
                    let _ = shell.dispatch(Command::MiniResized { window: id, size });
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

/// Centres the first mini window on the screen the main window is on, and
/// steps each further one down and across so none hides another.
fn place(app: &tauri::AppHandle, window: &WebviewWindow, id: u64) {
    let _ = window.center();
    let others = labels(app)
        .iter()
        .filter(|other| window_of(other) != Some(id))
        .count();
    if others == 0 {
        return;
    }
    let (Ok(position), Ok(scale)) = (window.outer_position(), window.scale_factor()) else {
        return;
    };
    let step = cascade_step(others) * scale;
    let _ = window.set_position(tauri::PhysicalPosition::new(
        f64::from(position.x) + step,
        f64::from(position.y) + step,
    ));
}

/// The logical offset for a window opening beside `others`, wrapping after
/// a few steps so it stays near the middle of the screen.
fn cascade_step(others: usize) -> f64 {
    // Bounded by MAX_MINI_WINDOWS; the cast cannot lose anything.
    (others % 6) as f64 * CASCADE
}

/// Toolbar reloads a mini window gets before its page moves to the main
/// window instead, so a toolbar that keeps failing never loops.
const MAX_TOOLBAR_RELOADS: u8 = 3;

static RELOADS: Mutex<Option<HashMap<u64, u8>>> = Mutex::new(None);

/// Counts one more reload of a window's toolbar; false once it had its share.
fn take_reload(reloads: &mut HashMap<u64, u8>, window: u64) -> bool {
    let count = reloads.entry(window).or_default();
    if *count >= MAX_TOOLBAR_RELOADS {
        return false;
    }
    *count += 1;
    true
}

/// The toolbar of a mini window lost its renderer. It loads again and
/// reports ready, which shows the page again; the page itself never stopped.
/// When the toolbar cannot come back, its page moves to the main window, so
/// nothing the person had open is lost.
pub(crate) fn recover(app: &tauri::AppHandle, toolbar: &str) {
    let Some(id) = window_of(toolbar) else {
        return;
    };
    if shutdown_started(app) {
        return;
    }
    let reload = take_reload(
        RELOADS
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get_or_insert_with(Default::default),
        id,
    );
    let reloaded = reload
        && app
            .get_webview_window(toolbar)
            .zip(app.try_state::<Factory>())
            .is_some_and(|(window, factory)| window.navigate(factory.url.clone()).is_ok());
    crate::write_diagnostic(format_args!(
        "mini: toolbar {id} renderer ended, {}",
        if reloaded {
            "reloading"
        } else {
            "moving its page"
        }
    ));
    if !reloaded {
        if let Some(shell) = app.try_state::<Handle>() {
            let _ = shell.dispatch(Command::Mini {
                window: id,
                action: MiniAction::OpenInMain,
            });
        }
    }
}

/// Takes the native window away once the shell let it go. When its page
/// moved into the main window, that window comes forward to show it.
pub(crate) fn close(app: &tauri::AppHandle, closed: MiniClosedView) {
    if let Some(views) = views().as_mut() {
        views.remove(&closed.window);
    }
    if let Some(reloads) = RELOADS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .as_mut()
    {
        reloads.remove(&closed.window);
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

pub(crate) fn project(app: &tauri::AppHandle, view: &MiniView) {
    let raise = {
        let mut views = views();
        let views = views.get_or_insert_with(Default::default);
        let was = views.get(&view.window).map(|(_, pinned)| *pinned);
        views.insert(view.window, (view.clone(), view.pinned));
        (was != Some(view.pinned)).then_some(view.pinned)
    };
    if let Some(pinned) = raise {
        let handle = app.clone();
        let window = view.window;
        let _ = app.run_on_main_thread(move || {
            if let Some(window) = handle.get_webview_window(&label(window)) {
                let _ = window.set_always_on_top(pinned);
                // Kept on top, it follows the person across desktops.
                let _ = window.set_visible_on_all_workspaces(pinned);
            }
        });
    }
    emit_to_privileged(app, &label(view.window), EVENT_MINI, view);
}

/// The presentation barrier for a mini window's page. The toolbar applies
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
        serde_json::to_string(EVENT_MINI_PRESENTATION),
        serde_json::to_string(&presentation.tab),
    ) else {
        return ChromePresentationDispatch::Rejected;
    };
    let nonce = format!(
        "zephium-mini-presentation-v1:{window}:{}:{:032x}",
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
    const shell = document.querySelector("[data-zephium-mini-tab]");
    if (!shell || shell.dataset.zephiumMiniTab !== tab.id) return rejected;
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
    let done = Mutex::new(Some(done));
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

/// What a mini window's toolbar may ask of its page or its window.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub(crate) enum MiniIntent {
    Back,
    Forward,
    Reload,
    OpenInMain,
    Close,
    /// Opens the launcher on the page's address.
    EditAddress,
    /// Keeps the window above other windows, or stops.
    KeepOnTop,
    /// Windows only: the toolbar carries the window's own controls.
    Minimize,
}

/// The toolbar finished its first render: the window shows, and the shell
/// lays its page out and lets it be verified.
#[tauri::command]
#[specta::specta]
pub(crate) fn mini_ready(caller: WebviewWindow, shell: State<'_, Handle>) -> bool {
    if !authorize(&caller, CallerPolicy::Mini, "mini_ready")
        || shutdown_started(caller.app_handle())
    {
        return false;
    }
    let (Some(window), Some(size)) = (window_of(caller.label()), content_size(&caller)) else {
        return false;
    };
    let _ = caller.show();
    let _ = caller.set_focus();
    shell.dispatch(Command::MiniAttached { window, size })
}

#[tauri::command]
#[specta::specta]
pub(crate) fn mini_action(caller: WebviewWindow, action: MiniIntent) -> OperationAdmission {
    if !authorize(&caller, CallerPolicy::Mini, "mini_action") {
        return crate::ui_admission(false);
    }
    let Some(window) = window_of(caller.label()) else {
        return crate::ui_admission(false);
    };
    let action = match action {
        MiniIntent::EditAddress => {
            return crate::ui_admission(edit_address(caller.app_handle(), window));
        }
        MiniIntent::Minimize => return crate::ui_admission(caller.minimize().is_ok()),
        MiniIntent::Back => MiniAction::Back,
        MiniIntent::Forward => MiniAction::Forward,
        MiniIntent::Reload => MiniAction::Reload,
        MiniIntent::OpenInMain => MiniAction::OpenInMain,
        MiniIntent::Close => MiniAction::Close,
        MiniIntent::KeepOnTop => MiniAction::KeepOnTop(None),
    };
    act(caller.app_handle(), window, action)
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
            format!("{SPACE_MENU}:{space}"),
            format!("{SPACE_MENU}-3:{space}"),
            format!("{SPACE_MENU}99999999999999999999999:{space}"),
            format!("mini.spaces=3:{space}"),
        ] {
            assert_eq!(space_choice(&id), None, "{id}");
        }
    }

    #[test]
    fn page_and_window_commands_act_on_the_mini_window() {
        let act = |id| match routed(id) {
            Some(Routed::Act(action)) => Some(action),
            _ => None,
        };
        assert_eq!(act("tab.close"), Some(MiniAction::Close));
        assert_eq!(act("nav.reload"), Some(MiniAction::Reload));
        assert_eq!(act("nav.reload.alt"), Some(MiniAction::Reload));
        assert_eq!(act("nav.hardReload"), Some(MiniAction::Reload));
        assert_eq!(act("nav.back"), Some(MiniAction::Back));
        assert_eq!(act("nav.back.alt"), Some(MiniAction::Back));
        assert_eq!(act("nav.forward"), Some(MiniAction::Forward));
        assert_eq!(act("mini.openInMain"), Some(MiniAction::OpenInMain));
        assert_eq!(act("mini.keepOnTop"), Some(MiniAction::KeepOnTop(None)));
        assert_eq!(routed("url.focus"), Some(Routed::EditAddress));
        // The main window's own commands still reach it.
        for id in [
            "tab.new",
            "window.newMini",
            "window.newPrivate",
            "",
            "mini.",
        ] {
            assert_eq!(routed(id), None, "{id}");
        }
    }

    #[test]
    fn only_canonical_labels_name_a_window() {
        assert_eq!(window_of("mini-2"), Some(2));
        assert_eq!(window_of(&label(41)), Some(41));
        for label in [
            "mini-",
            "mini-0",
            "mini-02",
            "mini-2a",
            "mini--2",
            "mini-+2",
            "mini- 2",
            "main",
            "panel",
            "Mini-2",
            "little-2",
            "mini-99999999999999999999999",
        ] {
            assert_eq!(window_of(label), None, "{label}");
        }
    }

    #[test]
    fn a_failing_toolbar_reloads_a_bounded_number_of_times() {
        let mut reloads = HashMap::new();
        for _ in 0..MAX_TOOLBAR_RELOADS {
            assert!(take_reload(&mut reloads, 4));
        }
        assert!(!take_reload(&mut reloads, 4));
        assert!(!take_reload(&mut reloads, 4));
        // Each window has its own share.
        assert!(take_reload(&mut reloads, 5));
    }

    #[test]
    fn further_windows_step_aside_and_wrap() {
        assert_eq!(cascade_step(0), 0.0);
        assert_eq!(cascade_step(1), CASCADE);
        assert_eq!(cascade_step(5), 5.0 * CASCADE);
        assert_eq!(cascade_step(6), 0.0);
        assert_eq!(cascade_step(7), CASCADE);
    }
}
