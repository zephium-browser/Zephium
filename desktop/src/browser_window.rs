//! Full browser windows beyond the launch window. Each native window receives
//! its own engine stage and UI startup gate while the shell remains the source
//! of truth for profiles, tabs and focus.

use raw_window_handle::HasWindowHandle;
#[cfg(target_os = "macos")]
use std::collections::HashMap;
use std::sync::Arc;
#[cfg(target_os = "macos")]
use std::sync::{Mutex, OnceLock};
use tauri::{Manager, WebviewWindow};
use zephium_app::{Command, Handle};
use zephium_core::geometry::Size;
use zephium_ipc::BrowserWindowOpenedView;

#[cfg(target_os = "macos")]
use crate::UiStartupGate;
use crate::{shutdown_started, MAIN_LABEL};

const LABEL_PREFIX: &str = "browser-";

#[cfg(target_os = "macos")]
struct Factory {
    app_url: tauri::Url,
    main_config: tauri::utils::config::WindowConfig,
    engine: Arc<zephium_engine::WebviewEngine>,
    #[cfg(target_os = "windows")]
    runtime: std::path::PathBuf,
}

#[cfg(target_os = "macos")]
static GATES: OnceLock<Mutex<HashMap<u64, UiStartupGate>>> = OnceLock::new();

#[cfg(target_os = "macos")]
fn gates() -> &'static Mutex<HashMap<u64, UiStartupGate>> {
    GATES.get_or_init(|| Mutex::new(HashMap::new()))
}

pub(crate) fn label(window: u64) -> String {
    if window == 1 {
        MAIN_LABEL.to_owned()
    } else {
        format!("{LABEL_PREFIX}{window}")
    }
}

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

pub(crate) fn labels(app: &tauri::AppHandle) -> Vec<String> {
    app.webview_windows()
        .into_keys()
        .filter(|label| *label == MAIN_LABEL || window_of(label).is_some())
        .collect()
}

pub(crate) fn focused(app: &tauri::AppHandle) -> Option<WebviewWindow> {
    labels(app).into_iter().find_map(|label| {
        app.get_webview_window(&label)
            .filter(|window| window.is_focused().unwrap_or(false))
    })
}

pub(crate) fn install(
    app: &tauri::AppHandle,
    app_url: tauri::Url,
    main_config: tauri::utils::config::WindowConfig,
    engine: Arc<zephium_engine::WebviewEngine>,
    #[cfg(target_os = "windows")] runtime: std::path::PathBuf,
) {
    #[cfg(target_os = "macos")]
    app.manage(Factory {
        app_url,
        main_config,
        engine,
    });
    #[cfg(not(target_os = "macos"))]
    let _ = (app, app_url, main_config, engine);
    #[cfg(all(not(target_os = "macos"), target_os = "windows"))]
    let _ = runtime;
}

pub(crate) fn open(app: &tauri::AppHandle, opened: BrowserWindowOpenedView) {
    let id = opened.window;
    let target = app.clone();
    if app
        .run_on_main_thread(move || {
            if !build(&target, opened) {
                unbuilt(&target, id);
            }
        })
        .is_err()
    {
        unbuilt(app, id);
    }
}

fn unbuilt(app: &tauri::AppHandle, id: u64) {
    crate::write_diagnostic(format_args!(
        "window: browser window {id} could not be built"
    ));
    if let Some(shell) = app.try_state::<Handle>() {
        shell.dispatch(Command::BrowserWindowClosed(id));
    }
}

#[cfg(target_os = "macos")]
fn build(app: &tauri::AppHandle, opened: BrowserWindowOpenedView) -> bool {
    if shutdown_started(app) || opened.window <= 1 {
        return false;
    }
    let Some(factory) = app.try_state::<Factory>() else {
        return false;
    };
    let label = label(opened.window);
    if app.get_webview_window(&label).is_some() {
        return false;
    }
    let min_width = factory.main_config.min_width.unwrap_or(680.0);
    let min_height = factory.main_config.min_height.unwrap_or(480.0);
    let mut config = factory.main_config.clone();
    config.label = label.clone();
    let gate = UiStartupGate::new(factory.app_url.clone());
    let page_gate = gate.clone();
    let builder = match tauri::WebviewWindowBuilder::from_config(app, &config) {
        Ok(builder) => builder,
        Err(error) => {
            crate::write_diagnostic(format_args!("window: browser config refused: {error}"));
            return false;
        }
    }
    .visible(false)
    .incognito(true)
    .devtools(cfg!(debug_assertions))
    .general_autofill_enabled(false)
    .initialization_script_for_all_frames(zephium_engine::PAGE_PRINT_DENY_SCRIPT)
    .on_download(|_, _| false)
    .on_new_window(|_, _| tauri::webview::NewWindowResponse::Deny)
    .on_web_resource_request(|_, response| crate::harden_privileged_headers(response.headers_mut()))
    .on_page_load(move |window, payload| {
        if matches!(payload.event(), tauri::webview::PageLoadEvent::Finished) {
            page_gate.mark_document_loaded(&window, payload.url());
        } else if matches!(payload.event(), tauri::webview::PageLoadEvent::Started) {
            page_gate.mark_document_started();
        }
    })
    .inner_size(opened.width.max(min_width), opened.height.max(min_height));
    let window = match builder.build() {
        Ok(window) => window,
        Err(error) => {
            crate::write_diagnostic(format_args!("window: browser window build failed: {error}"));
            return false;
        }
    };
    if !crate::platform::imp::harden_privileged(&window) {
        let _ = window.destroy();
        return false;
    }
    crate::material::install(&window, false);
    let keymap = app.state::<crate::keymap::Keymap>().key_table();
    let shortcut_app = app.clone();
    crate::platform::imp::install_key_monitor(&window, keymap, move |command| {
        let _ = crate::execute_command(&shortcut_app, command);
    });
    let Ok(handle) = window.window_handle().map(|handle| handle.as_raw()) else {
        let _ = window.destroy();
        return false;
    };
    if !factory.engine.attach_window(opened.window, handle) {
        let _ = window.destroy();
        return false;
    }
    gates()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert(opened.window, gate);

    let event_window = window.clone();
    let id = opened.window;
    window.on_window_event(move |event| {
        let app = event_window.app_handle();
        let Some(shell) = app.try_state::<Handle>() else {
            return;
        };
        match event {
            tauri::WindowEvent::CloseRequested { api, .. } if !shutdown_started(app) => {
                api.prevent_close();
                shell.dispatch(Command::BrowserWindowClosed(id));
            }
            tauri::WindowEvent::Destroyed => {
                crate::platform::imp::remove_key_monitor(&event_window);
                gates()
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .remove(&id);
                shell.dispatch(Command::BrowserWindowClosed(id));
            }
            tauri::WindowEvent::Resized(_) | tauri::WindowEvent::ScaleFactorChanged { .. } => {
                if let Some(size) = crate::platform::imp::content_size(&event_window) {
                    shell.dispatch(Command::BrowserWindowResized { window: id, size });
                }
            }
            tauri::WindowEvent::Focused(focused) => {
                shell.dispatch(Command::BrowserWindowFocused {
                    window: id,
                    focused: *focused,
                });
                if *focused {
                    shell.dispatch(Command::BrowserWindowVisible {
                        window: id,
                        visible: true,
                    });
                } else if event_window.is_minimized().unwrap_or(false) {
                    shell.dispatch(Command::BrowserWindowVisible {
                        window: id,
                        visible: false,
                    });
                }
            }
            _ => {}
        }
    });

    if window.navigate(factory.app_url.clone()).is_err() {
        let _ = window.destroy();
        return false;
    }
    let size = crate::platform::imp::content_size(&window)
        .unwrap_or_else(|| Size::new(opened.width, opened.height));
    if let Some(shell) = app.try_state::<Handle>() {
        shell.dispatch(Command::BrowserWindowAttached {
            window: opened.window,
            size,
        });
        shell.dispatch(Command::BrowserWindowFocused {
            window: opened.window,
            focused: window.is_focused().unwrap_or(false),
        });
    }
    true
}

#[cfg(not(target_os = "macos"))]
fn build(_app: &tauri::AppHandle, _opened: BrowserWindowOpenedView) -> bool {
    false
}

pub(crate) fn close(app: &tauri::AppHandle, id: u64) {
    let label = label(id);
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || {
        if let Some(window) = handle.get_webview_window(&label) {
            let _ = window.destroy();
        }
    });
}

pub(crate) fn ui_ready(window: &WebviewWindow) -> bool {
    #[cfg(not(target_os = "macos"))]
    return false;
    #[cfg(target_os = "macos")]
    {
        let Some(id) = window_of(window.label()) else {
            return false;
        };
        gates()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(&id)
            .is_some_and(|gate| gate.mark_frontend_ready(window))
    }
}

pub(crate) fn navigation_allowed(label: &str, url: &tauri::Url) -> Option<bool> {
    #[cfg(not(target_os = "macos"))]
    return None;
    #[cfg(target_os = "macos")]
    {
        let id = window_of(label)?;
        let allowed = gates()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(&id)
            .is_some_and(|gate| gate.permits_navigation(url));
        Some(allowed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn browser_window_labels_are_canonical() {
        assert_eq!(window_of("browser-2"), Some(2));
        for invalid in ["browser-", "browser-0", "browser-02", "browser-x", "main"] {
            assert_eq!(window_of(invalid), None, "{invalid}");
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn navigation_from_an_unregistered_browser_window_is_denied() {
        gates()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(&2);
        let url = tauri::Url::parse("http://localhost:1420/browser.html").unwrap();
        assert_eq!(navigation_allowed("browser-2", &url), Some(false));
    }
}
