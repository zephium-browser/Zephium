//! One persistent native window, with bounded search/tool routes and no feature workers while hidden.
mod geometry;
mod model;
use model::{Model, Owner};
use std::sync::{Arc, Mutex};
use tauri::{LogicalSize, Manager, PhysicalPosition, WebviewWindow};
use zephium_core::ports::store::Store;
use zephium_ipc::{
    OperationDisposition, OperationOutcome, PanelIntent, PanelOwner, PanelRoute, PanelState,
    SearchContext, ToolKind,
};
pub const PANEL_LABEL: &str = "panel";
pub const PANEL_SIZE: (f64, f64) = geometry::DEFAULT;
#[cfg(target_os = "macos")]
pub const PANEL_RADIUS: u16 = model::RADIUS;
pub const EVENT_STATE: &str = "zephium:panel-state";
#[derive(Default)]
pub struct ContextCache(Mutex<Option<Owner>>);
struct State {
    model: Model,
    pending_document: Option<tauri::Url>,
    geometry: Option<geometry::Geometry>,
    persisted_geometry: Option<geometry::Geometry>,
    positioned: bool,
    geometry_loaded: bool,
    geometry_revision: u64,
    focus_revision: u64,
    geometry_timer: bool,
    focus_timer: bool,
    always_floating: bool,
    preference_changed: bool,
    closed: bool,
}
#[derive(Clone)]
pub struct Overlay {
    window: WebviewWindow,
    state: Arc<Mutex<State>>,
}
impl Overlay {
    pub fn new(window: WebviewWindow, pending_document: Option<tauri::Url>) -> Self {
        let owner = window
            .app_handle()
            .try_state::<ContextCache>()
            .and_then(|cache| cache.0.lock().ok()?.clone());
        let mut model = Model::default();
        model.set_owner(owner);
        let this = Self {
            window,
            state: Arc::new(Mutex::new(State {
                model,
                pending_document,
                geometry: None,
                persisted_geometry: None,
                positioned: false,
                geometry_loaded: false,
                geometry_revision: 0,
                focus_revision: 0,
                geometry_timer: false,
                focus_timer: false,
                always_floating: false,
                preference_changed: false,
                closed: false,
            })),
        };
        #[cfg(target_os = "macos")]
        {
            let w = this.window.clone();
            let _ = this
                .window
                .run_on_main_thread(move || crate::panel::configure(&w));
        }
        let restore = this.clone();
        tauri::async_runtime::spawn(async move {
            let values = tauri::async_runtime::spawn_blocking(|| {
                crate::APP_STORE.get().map(|store| {
                    (
                        store.app_setting(geometry::KEY),
                        store.app_setting("tools.presentation"),
                    )
                })
            })
            .await
            .ok()
            .flatten();
            restore.on_main(move |this| {
                {
                    let mut state = this.state.lock().unwrap_or_else(|e| e.into_inner());
                    if let Some((geometry, preference)) = values {
                        if !state.positioned {
                            state.geometry =
                                geometry.and_then(|value| geometry::Geometry::decode(&value));
                            state.persisted_geometry = state.geometry.clone();
                        }
                        if !state.preference_changed {
                            state.always_floating = preference.as_deref() == Some("floating");
                        }
                    }
                    state.geometry_loaded = true;
                }
                this.present(true);
            });
        });
        this
    }
    fn on_main(&self, f: impl FnOnce(&Self) + Send + 'static) {
        let this = self.clone();
        let _ = self.window.run_on_main_thread(move || {
            if !crate::shutdown_started(this.window.app_handle())
                && !this.state.lock().unwrap_or_else(|e| e.into_inner()).closed
            {
                f(&this);
            }
        });
    }
    pub fn window_app(&self) -> &tauri::AppHandle {
        self.window.app_handle()
    }
    pub fn private(&self) -> bool {
        self.state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .model
            .owner
            .as_ref()
            .is_none_or(|owner| owner.private)
    }
    pub fn snapshot(&self) -> PanelState {
        let mut snapshot = self
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .model
            .snapshot();
        snapshot.position_restorable = position_supported();
        snapshot
    }
    /// Whether the panel is on screen with `tool` open.
    pub fn showing(&self, tool: ToolKind) -> bool {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let model = &state.model;
        model.presented
            && !model.suppressed
            && matches!(&model.route, PanelRoute::Tool { tool: open } if *open == tool)
    }
    pub fn always_floating(&self) -> bool {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .always_floating
    }
    pub fn preference(&self, floating: bool) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.always_floating = floating;
        state.preference_changed = true;
    }
    pub fn ready(&self) -> PanelState {
        {
            self.state
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .model
                .ready = true;
        }
        self.on_main(|this| this.present(true));
        self.snapshot()
    }
    pub fn intent(&self, intent: PanelIntent) {
        self.on_main(move |this| {
            let old = this.snapshot().session_id;
            {
                let mut state = this.state.lock().unwrap_or_else(|e| e.into_inner());
                match intent {
                    PanelIntent::Search | PanelIntent::Back => state.model.search(),
                    PanelIntent::Dismiss => state.model.hide(),
                    PanelIntent::Tool { tool } => state.model.tool(tool),
                }
            }
            this.cancel_search(old);
            this.present(true);
        });
    }
    pub fn toggle(&self) {
        self.on_main(|this| {
            let old = this.snapshot().session_id;
            this.state
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .model
                .toggle();
            this.cancel_search(old);
            this.present(true);
        });
    }
    #[cfg(target_os = "linux")]
    pub fn toggle_with_activation(&self, activation_token: Option<String>, timestamp: Option<u32>) {
        self.on_main(move |this| {
            if crate::shutdown_started(this.window.app_handle()) {
                return;
            }
            let old = this.snapshot().session_id;
            this.state
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .model
                .toggle();
            this.cancel_search(old);
            this.present(true);
            if this.snapshot().visible {
                do_show_with_activation(&this.window, activation_token.as_deref(), timestamp);
            }
        });
    }
    pub fn hide(&self) {
        self.intent(PanelIntent::Dismiss);
    }
    fn cancel_search(&self, session_id: String) {
        if let Some(shell) = self.window.app_handle().try_state::<zephium_app::Handle>() {
            crate::search_providers::cancel(&session_id);
            shell.dispatch(zephium_app::Command::CancelSearch { session_id });
        }
    }
    fn publish(&self) {
        crate::emit_to_privileged(
            self.window.app_handle(),
            PANEL_LABEL,
            EVENT_STATE,
            &self.snapshot(),
        );
    }
    fn present(&self, focus: bool) {
        let snapshot = self.snapshot();
        let (ready, document) = {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            let document = if snapshot.visible && !state.model.ready {
                state.pending_document.take()
            } else {
                None
            };
            (state.model.ready, document)
        };
        // Windows keeps the hardened native panel at about:blank until the
        // first real request. Loading an unused second application graph at
        // startup competes with the main surface and retains renderer state.
        // The authoritative model keeps the latest route/owner while loading;
        // panel_ready returns that snapshot before the first native reveal.
        if let Some(document) = document {
            if let Err(error) = self.window.navigate(document) {
                crate::request_startup_failure(
                    self.window.app_handle(),
                    format_args!("could not load trusted panel document: {error}"),
                );
            }
        }
        if !ready {
            return;
        }
        self.publish();
        if !snapshot.visible {
            self.persist_geometry();
            do_hide(&self.window);
            return;
        }
        if !self
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .geometry_loaded
        {
            return;
        }
        if !visible(&self.window) {
            self.state
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .positioned = false;
        }
        self.prepare_geometry();
        #[cfg(target_os = "macos")]
        {
            crate::panel::set_tool_mode(
                &self.window,
                matches!(snapshot.route, PanelRoute::Tool { .. }),
            );
            if focus {
                crate::panel::show(&self.window);
            } else {
                crate::panel::show_unfocused(&self.window);
            }
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = self.window.show();
            if focus {
                let _ = self.window.set_focus();
            }
        }
    }
    pub fn search_context(&self, request_id: &str) -> Option<SearchContext> {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .model
            .context(request_id)
    }
    pub fn arm_action(&self, id: &str, context: &SearchContext) -> bool {
        {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            if !state.model.admits(context) || state.model.pending_action.is_some() {
                return false;
            }
            state.model.clear_error();
            state.model.pending_action = Some(id.into());
        }
        self.on_main(|this| this.publish());
        true
    }
    pub fn action_rejected(&self, id: &str) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.model.pending_action.as_deref() == Some(id) {
            state.model.pending_action = None;
        }
    }
    pub fn operation(&self, result: OperationDisposition) {
        self.on_main(move |this| {
            let old_session = this.snapshot().session_id;
            let mut state = this.state.lock().unwrap_or_else(|e| e.into_inner());
            if state.model.pending_action.as_deref() != Some(&result.operation_id) {
                return;
            }
            state.model.pending_action = None;
            let accepted = matches!(
                result.outcome,
                OperationOutcome::Applied | OperationOutcome::NoOp | OperationOutcome::Deferred
            );
            if accepted {
                state.model.hide();
            } else {
                state.model.reject();
            }
            drop(state);
            if accepted {
                this.cancel_search(old_session);
            }
            this.present(false);
            if accepted {
                if let Some(main) = this
                    .window
                    .app_handle()
                    .get_webview_window(crate::MAIN_LABEL)
                {
                    let _ = main.set_focus();
                }
            }
        });
    }
    pub fn focus_changed(&self) {
        {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            state.focus_revision = state.focus_revision.saturating_add(1);
            if state.focus_timer {
                return;
            }
            state.focus_timer = true;
        }
        let this = self.clone();
        tauri::async_runtime::spawn(async move {
            let mut observed = this
                .state
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .focus_revision;
            loop {
                tokio::time::sleep(std::time::Duration::from_millis(35)).await;
                let settled = {
                    let mut state = this.state.lock().unwrap_or_else(|e| e.into_inner());
                    if state.closed {
                        return;
                    }
                    if state.focus_revision == observed {
                        state.focus_timer = false;
                        true
                    } else {
                        observed = state.focus_revision;
                        false
                    }
                };
                if settled {
                    break;
                }
            }
            this.on_main(|this| {
                if !this
                    .state
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .model
                    .ready
                {
                    return;
                }
                let panel_focused = this.window.is_focused().unwrap_or(false);
                let main_focused = this
                    .window
                    .app_handle()
                    .get_webview_window(crate::MAIN_LABEL)
                    .is_some_and(|window| window.is_focused().unwrap_or(false));
                let app_active = application_active(panel_focused, main_focused);
                let old = this.snapshot();
                this.state
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .model
                    .focus(panel_focused, main_focused, app_active);
                if this.snapshot() != old {
                    this.cancel_search(old.session_id);
                    this.present(false);
                }
            });
        });
    }
    pub fn geometry_changed(&self) {
        {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            state.geometry_revision = state.geometry_revision.saturating_add(1);
            if state.geometry_timer {
                return;
            }
            state.geometry_timer = true;
        }
        let this = self.clone();
        tauri::async_runtime::spawn(async move {
            let mut observed = this
                .state
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .geometry_revision;
            loop {
                tokio::time::sleep(std::time::Duration::from_millis(250)).await;
                let settled = {
                    let mut state = this.state.lock().unwrap_or_else(|e| e.into_inner());
                    if state.closed {
                        return;
                    }
                    if state.geometry_revision == observed {
                        state.geometry_timer = false;
                        true
                    } else {
                        observed = state.geometry_revision;
                        false
                    }
                };
                if settled {
                    break;
                }
            }
            this.on_main(|this| this.persist_geometry());
        });
    }
    pub fn display_changed(&self) {
        self.on_main(|this| {
            this.persist_geometry();
            this.state
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .positioned = false;
            if this.snapshot().visible {
                this.prepare_geometry();
            }
        });
    }
    pub fn destroyed(&self) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.closed = true;
    }
    fn prepare_geometry(&self) {
        if self
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .positioned
        {
            return;
        }
        let saved = self
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .geometry
            .clone();
        let monitor = self
            .window
            .available_monitors()
            .ok()
            .and_then(|monitors| {
                saved.as_ref().and_then(|saved| {
                    monitors
                        .iter()
                        .find(|monitor| monitor.name().cloned() == saved.monitor)
                        .cloned()
                })
            })
            .or_else(|| {
                self.window
                    .app_handle()
                    .cursor_position()
                    .ok()
                    .and_then(|cursor| {
                        self.window
                            .available_monitors()
                            .ok()?
                            .into_iter()
                            .find(|monitor| {
                                let area = monitor.work_area();
                                cursor.x >= f64::from(area.position.x)
                                    && cursor.y >= f64::from(area.position.y)
                                    && cursor.x
                                        < f64::from(area.position.x) + f64::from(area.size.width)
                                    && cursor.y
                                        < f64::from(area.position.y) + f64::from(area.size.height)
                            })
                    })
            })
            .or_else(|| self.window.current_monitor().ok().flatten())
            .or_else(|| self.window.primary_monitor().ok().flatten());
        let Some(monitor) = monitor else {
            return;
        };
        let scale = monitor.scale_factor();
        if !scale.is_finite() || scale <= 0.0 {
            return;
        }
        let work = monitor.work_area();
        let width = f64::from(work.size.width) / scale;
        let height = f64::from(work.size.height) / scale;
        let mut geometry = saved
            .unwrap_or(geometry::Geometry {
                version: 1,
                monitor: monitor.name().cloned(),
                x: (width - PANEL_SIZE.0) / 2.0,
                y: (height - PANEL_SIZE.1) * 0.22,
                width: PANEL_SIZE.0,
                height: PANEL_SIZE.1,
            })
            .fit(width, height);
        geometry.monitor = monitor.name().cloned();
        let (min, max) = geometry::limits(width, height);
        let _ = self
            .window
            .set_min_size(Some(LogicalSize::new(min.0, min.1)));
        let _ = self
            .window
            .set_max_size(Some(LogicalSize::new(max.0, max.1)));
        let _ = self
            .window
            .set_size(LogicalSize::new(geometry.width, geometry.height));
        if position_supported() {
            let _ = self.window.set_position(PhysicalPosition::new(
                f64::from(work.position.x) + geometry.x * scale,
                f64::from(work.position.y) + geometry.y * scale,
            ));
        }
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.positioned = true;
        state.geometry = Some(geometry);
    }
    fn persist_geometry(&self) {
        if !self
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .positioned
        {
            return;
        }
        let (Ok(Some(monitor)), Ok(size)) =
            (self.window.current_monitor(), self.window.inner_size())
        else {
            return;
        };
        let position = self.window.outer_position().ok();
        let scale = monitor.scale_factor();
        if !scale.is_finite() || scale <= 0.0 {
            return;
        }
        let work = monitor.work_area();
        let value = geometry::Geometry {
            version: 1,
            monitor: monitor.name().cloned(),
            x: position.map_or(24.0, |p| {
                (f64::from(p.x) - f64::from(work.position.x)) / scale
            }),
            y: position.map_or(24.0, |p| {
                (f64::from(p.y) - f64::from(work.position.y)) / scale
            }),
            width: f64::from(size.width) / scale,
            height: f64::from(size.height) / scale,
        }
        .fit(
            f64::from(work.size.width) / scale,
            f64::from(work.size.height) / scale,
        );
        if !value.valid() {
            return;
        }
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.persisted_geometry.as_ref() == Some(&value) {
            return;
        }
        state.geometry = Some(value.clone());
        drop(state);
        if let (Some(store), Ok(json)) = (crate::APP_STORE.get(), serde_json::to_string(&value)) {
            if store.set_app_setting(geometry::KEY.into(), json) {
                self.state
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .persisted_geometry = Some(value);
            }
        }
    }
    pub fn flush(&self) {
        self.persist_geometry();
    }
}
pub fn update_context(app: &tauri::AppHandle, context: &PanelOwner) {
    let owner = Some(Owner {
        private: context.private,
        window: context.window_id.clone(),
        profile: context.profile_id.clone(),
        name: context.profile_name.clone(),
        space: context.space_id.clone(),
    });
    let changed = app.try_state::<ContextCache>().is_some_and(|cache| {
        let mut current = cache.0.lock().unwrap_or_else(|e| e.into_inner());
        if *current == owner {
            false
        } else {
            *current = owner.clone();
            true
        }
    });
    if !changed {
        return;
    }
    if let Some(overlay) = app.try_state::<Overlay>() {
        overlay.on_main(move |this| {
            let old = this.snapshot();
            this.state
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .model
                .set_owner(owner);
            if this.snapshot() != old {
                this.cancel_search(old.session_id);
                this.present(false);
            }
        });
    }
}

fn position_supported() -> bool {
    #[cfg(target_os = "linux")]
    {
        std::env::var_os("WAYLAND_DISPLAY").is_none()
    }
    #[cfg(not(target_os = "linux"))]
    {
        true
    }
}
fn do_hide(window: &WebviewWindow) {
    #[cfg(target_os = "macos")]
    crate::panel::hide(window);
    #[cfg(not(target_os = "macos"))]
    let _ = window.hide();
}
#[cfg(target_os = "linux")]
fn do_show_with_activation(
    window: &WebviewWindow,
    activation_token: Option<&str>,
    timestamp: Option<u32>,
) {
    use gtk::prelude::*;
    if let Ok(gtk_window) = window.gtk_window() {
        if let Some(token) = activation_token {
            gtk_window.set_startup_id(token);
        }
        let _ = window.show();
        if let Some(timestamp) = timestamp {
            gtk_window.present_with_time(timestamp);
        } else {
            gtk_window.present();
        }
    }
}

fn visible(window: &WebviewWindow) -> bool {
    #[cfg(target_os = "macos")]
    {
        crate::panel::is_visible(window)
    }
    #[cfg(not(target_os = "macos"))]
    {
        window.is_visible().unwrap_or(false)
    }
}

fn application_active(panel_focused: bool, main_focused: bool) -> bool {
    if panel_focused || main_focused {
        return true;
    }
    #[cfg(target_os = "macos")]
    {
        crate::panel::application_active()
    }
    #[cfg(target_os = "windows")]
    {
        use windows::Win32::UI::WindowsAndMessaging::{
            GetForegroundWindow, GetWindowThreadProcessId,
        };
        let mut process = 0;
        // SAFETY: queries only an OS-owned foreground window handle; the output
        // pointer is a live stack u32 and no ownership crosses this call.
        unsafe {
            GetWindowThreadProcessId(GetForegroundWindow(), Some(&mut process));
        }
        process == std::process::id()
    }
    #[cfg(target_os = "linux")]
    {
        use gtk::prelude::*;
        gtk::Window::list_toplevels()
            .into_iter()
            .filter_map(|widget| widget.downcast::<gtk::Window>().ok())
            .any(|window| window.is_active())
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    {
        false
    }
}
