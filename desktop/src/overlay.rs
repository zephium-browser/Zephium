//! A native launcher created on first use and put to sleep while hidden. It hosts search only:
//! destinations are handed to the browser, which already has their views
//! loaded, so nothing heavy is ever instantiated a second time here.
mod geometry;
mod idle;
mod model;
#[cfg(target_os = "windows")]
mod viewport;
use model::{Model, Owner};
use std::sync::{Arc, Mutex};
use tauri::{LogicalSize, Manager, PhysicalPosition, WebviewWindow};
#[cfg(target_os = "macos")]
use zephium_core::ports::store::Store;
use zephium_ipc::{
    OperationDisposition, OperationOutcome, PanelIntent, PanelLayout, PanelOwner, PanelState,
    SearchContext, ToolKind,
};
pub const PANEL_LABEL: &str = "panel";
pub const PANEL_SIZE: (f64, f64) = (geometry::WIDTH, geometry::INITIAL_HEIGHT);
#[cfg(target_os = "macos")]
pub const PANEL_RADIUS: u16 = model::RADIUS;
pub const EVENT_STATE: &str = "zephium:panel-state";
#[derive(Default)]
pub struct ContextCache(Mutex<Option<Owner>>);
impl ContextCache {
    /// A provider must match the current browser owner even before the panel exists.
    pub fn search_private(&self, context: &SearchContext) -> Option<bool> {
        let current = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let owner = current.as_ref()?;
        (owner.window == context.window_id
            && owner.profile == context.profile_id
            && owner.space == context.space_id)
            .then_some(owner.private)
    }
    /// Browser search shares the focused owner, not the launcher's window lifetime.
    pub fn newtab_context(&self, tab_id: &str, session: u64) -> Option<SearchContext> {
        let owner = self.0.lock().unwrap_or_else(|e| e.into_inner()).clone()?;
        Some(SearchContext {
            window_id: owner.window,
            profile_id: owner.profile,
            space_id: owner.space,
            session_id: format!("newtab:{tab_id}:{session}"),
            request_id: String::new(),
        })
    }
}
struct Pending {
    id: String,
    background: bool,
}
struct State {
    model: Model,
    pending: Option<Pending>,
    /// Height the launcher's content asked for, before the display bounds it.
    content_height: f64,
    layout_revision: u64,
    placement: Option<geometry::Placement>,
    pending_document: Option<tauri::Url>,
    focus_revision: u64,
    focus_timer: bool,
    closed: bool,
    suspending: bool,
}
impl State {
    /// A new presentation never inherits an action still awaiting settlement.
    /// A disposition that never reaches the launcher, such as one the ledger
    /// drops as a duplicate, would otherwise refuse every later action.
    fn begin(&mut self) {
        self.pending = None;
        self.model.search();
    }
}
#[derive(Clone)]
pub struct Overlay {
    window: WebviewWindow,
    state: Arc<Mutex<State>>,
    #[cfg(target_os = "windows")]
    recovery_document: Option<tauri::Url>,
}
impl Overlay {
    pub fn new(window: WebviewWindow, pending_document: Option<tauri::Url>) -> Self {
        let owner = window
            .app_handle()
            .try_state::<ContextCache>()
            .and_then(|cache| cache.0.lock().ok()?.clone());
        let mut model = Model::default();
        model.set_owner(owner);
        // The owned window clips a stable renderer viewport on Windows. Native
        // height changes no longer trigger a second WebView2 surface resize.
        #[cfg(target_os = "windows")]
        {
            let _ = window.as_ref().set_auto_resize(false);
            if !viewport::install(&window) {
                crate::write_diagnostic(format_args!(
                    "launcher: stable viewport installation failed"
                ));
            }
        }
        let this = Self {
            window,
            #[cfg(target_os = "windows")]
            recovery_document: pending_document.clone(),
            state: Arc::new(Mutex::new(State {
                model,
                pending: None,
                content_height: geometry::INITIAL_HEIGHT,
                layout_revision: 0,
                placement: None,
                pending_document,
                focus_revision: 0,
                focus_timer: false,
                closed: false,
                suspending: false,
            })),
        };
        #[cfg(target_os = "macos")]
        {
            let w = this.window.clone();
            let _ = this
                .window
                .run_on_main_thread(move || crate::panel::configure(&w));
            // Live changes arrive as a UI command; this is the value at launch.
            tauri::async_runtime::spawn_blocking(|| {
                let reduce = crate::APP_STORE
                    .get()
                    .and_then(|store| store.app_setting("ui.reduce-motion"));
                crate::panel::set_reduce_motion(reduce.as_deref() == Some("true"));
            });
        }
        this
    }
    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }
    fn on_main(&self, f: impl FnOnce(&Self) + Send + 'static) {
        let this = self.clone();
        let _ = self.window.run_on_main_thread(move || {
            if !crate::shutdown_started(this.window.app_handle()) && !this.state().closed {
                f(&this);
            }
        });
    }
    pub fn window_app(&self) -> &tauri::AppHandle {
        self.window.app_handle()
    }
    pub fn snapshot(&self) -> PanelState {
        self.state().model.snapshot()
    }
    /// A renderer acknowledgement, after its pending captures have settled.
    pub fn idle(&self, revision: String) {
        self.on_main(move |this| {
            let mut state = this.state();
            if crate::resource_close::is_closing()
                || state.model.presented
                || state.pending.is_some()
                || state.suspending
                || state.model.snapshot().revision != revision
            {
                return;
            }
            state.suspending = true;
            drop(state);
            let panel = this.clone();
            idle::suspend(&this.window, move || {
                panel.state().suspending = false;
                if panel.snapshot().visible || crate::resource_close::is_closing() {
                    idle::resume(&panel.window);
                }
            });
        });
    }
    pub fn wake(&self) {
        idle::resume(&self.window);
    }
    pub fn ready(&self) -> PanelState {
        self.state().model.ready = true;
        self.on_main(|this| this.present());
        self.snapshot()
    }
    #[cfg(target_os = "windows")]
    pub(crate) fn prepare_recovery(&self) -> Option<tauri::Url> {
        let document = self.recovery_document.clone()?;
        {
            let mut state = self.state();
            state.model.ready = false;
            state.pending = None;
            // Recovery itself loads the fixed document. A concurrent launcher
            // intent must not start a second navigation through present().
            state.pending_document = None;
        }
        self.window.hide().ok()?;
        self.wake();
        Some(document)
    }
    pub fn intent(&self, intent: PanelIntent) {
        if let PanelIntent::Idle { revision } = intent {
            self.idle(revision);
            return;
        }
        self.on_main(move |this| {
            if crate::resource_close::is_closing() {
                return;
            }
            let old = this.snapshot().session_id;
            match intent {
                PanelIntent::Idle { .. } => {
                    unreachable!("idle intents are handled before presentation")
                }
                PanelIntent::Search => this.state().begin(),
                PanelIntent::Dismiss => {
                    this.state().model.hide();
                    #[cfg(target_os = "macos")]
                    crate::panel::return_to_previous();
                }
                PanelIntent::Open { tool } => {
                    this.state().model.hide();
                    this.cancel_search(old);
                    this.present();
                    this.raise_browser();
                    let _ = crate::execute_command(this.window.app_handle(), destination(tool));
                    return;
                }
            }
            this.cancel_search(old);
            this.present();
        });
    }
    pub fn toggle(&self) {
        self.on_main(|this| {
            if crate::resource_close::is_closing() {
                return;
            }
            let old = this.snapshot().session_id;
            let dismissing = this.state().model.presented;
            if dismissing {
                this.state().model.hide();
            } else {
                this.state().begin();
            }
            this.cancel_search(old);
            this.present();
            #[cfg(target_os = "macos")]
            if dismissing {
                crate::panel::return_to_previous();
            }
            #[cfg(not(target_os = "macos"))]
            let _ = dismissing;
        });
    }
    #[cfg(target_os = "linux")]
    pub fn toggle_with_activation(&self, activation_token: Option<String>, timestamp: Option<u32>) {
        self.on_main(move |this| {
            if crate::resource_close::is_closing() {
                return;
            }
            let old = this.snapshot().session_id;
            if this.state().model.presented {
                this.state().model.hide();
            } else {
                this.state().begin();
            }
            this.cancel_search(old);
            if !this.snapshot().visible {
                this.present();
                return;
            }
            this.publish();
            if this.state().model.ready {
                this.place();
                do_show_with_activation(&this.window, activation_token.as_deref(), timestamp);
            }
        });
    }
    pub fn hide(&self) {
        self.intent(PanelIntent::Dismiss);
    }
    /// The launcher's content reports what it shows. The window follows its
    /// height within the display's bounds, keeping its top edge where it is;
    /// where native draws the shapes, they move first and a smaller window
    /// follows once they have settled, so nothing is cut off mid-motion.
    pub fn layout(&self, layout: PanelLayout) {
        if !layout.bounded() {
            return;
        }
        self.on_main(move |this| {
            let height = layout
                .height
                .clamp(geometry::RESTING_HEIGHT, geometry::MAX_HEIGHT);
            let showing = this.snapshot().visible && visible(&this.window);
            #[cfg(target_os = "macos")]
            crate::panel::layout(&layout, showing);
            let (_grows, _revision) = {
                let mut state = this.state();
                let grows = height > state.content_height;
                state.content_height = height;
                state.layout_revision = state.layout_revision.wrapping_add(1);
                (grows, state.layout_revision)
            };
            if !showing {
                return;
            }
            #[cfg(target_os = "windows")]
            {
                this.resize_windows();
            }
            #[cfg(not(target_os = "windows"))]
            if _grows || !animated_shapes() {
                this.resize();
                return;
            }
            #[cfg(not(target_os = "windows"))]
            {
                let later = this.clone();
                tauri::async_runtime::spawn(async move {
                    tokio::time::sleep(SHAPES_SETTLE).await;
                    later.on_main(move |this| {
                        if this.state().layout_revision == _revision {
                            this.resize();
                        }
                    });
                });
            }
        });
    }
    /// Onboarding steps its drawn launcher aside while the real one is up.
    /// The browser hears only that the launcher came or went, never what
    /// it holds.
    fn tell_browser(&self, event: &'static str) {
        crate::emit_to_privileged(
            self.window.app_handle(),
            crate::MAIN_LABEL,
            crate::EVENT_UI,
            &event,
        );
    }
    fn raise_browser(&self) {
        #[cfg(target_os = "macos")]
        crate::panel::forget_previous();
        let Some(main) = self
            .window
            .app_handle()
            .get_webview_window(crate::MAIN_LABEL)
        else {
            return;
        };
        #[cfg(target_os = "macos")]
        crate::panel::raise(&main);
        #[cfg(not(target_os = "macos"))]
        {
            let _ = main.unminimize();
            let _ = main.show();
            let _ = main.set_focus();
        }
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
    fn present(&self) {
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
        // First use creates and hardens the native panel at about:blank. Loading an unused second application graph at
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
        if snapshot.visible {
            self.wake();
        }
        self.publish();
        if !snapshot.visible {
            if visible(&self.window) {
                self.tell_browser("launcher.dismissed");
            }
            do_hide(&self.window);
            return;
        }
        if !visible(&self.window) {
            self.place();
            self.tell_browser("launcher.presented");
        }
        #[cfg(target_os = "macos")]
        crate::panel::show(&self.window);
        #[cfg(not(target_os = "macos"))]
        {
            let _ = self.window.show();
            let _ = self.window.set_focus();
        }
    }
    pub fn search_context(&self, request_id: &str) -> Option<SearchContext> {
        self.state().model.context(request_id)
    }
    pub fn arm_action(&self, id: &str, context: &SearchContext, background: bool) -> bool {
        {
            let mut state = self.state();
            if !state.model.admits(context) || state.pending.is_some() {
                return false;
            }
            state.model.clear_error();
            state.pending = Some(Pending {
                id: id.into(),
                background,
            });
        }
        self.on_main(|this| this.publish());
        true
    }
    pub fn action_rejected(&self, id: &str) {
        let mut state = self.state();
        if state
            .pending
            .as_ref()
            .is_some_and(|pending| pending.id == id)
        {
            state.pending = None;
        }
    }
    pub fn operation(&self, result: OperationDisposition) {
        self.on_main(move |this| {
            let old_session = this.snapshot().session_id;
            let mut state = this.state();
            let Some(pending) = state
                .pending
                .take_if(|pending| pending.id == result.operation_id)
            else {
                return;
            };
            let accepted = matches!(
                result.outcome,
                OperationOutcome::Applied | OperationOutcome::NoOp | OperationOutcome::Deferred
            );
            if !accepted {
                state.model.reject();
                drop(state);
                this.publish();
                return;
            }
            // A background open leaves the launcher where it is, so several
            // results can be sent to the browser in one visit.
            if pending.background {
                return;
            }
            state.model.hide();
            drop(state);
            this.cancel_search(old_session);
            this.present();
            this.raise_browser();
        });
    }
    pub fn focus_changed(&self) {
        {
            let mut state = self.state();
            state.focus_revision = state.focus_revision.saturating_add(1);
            if state.focus_timer {
                return;
            }
            state.focus_timer = true;
        }
        let this = self.clone();
        tauri::async_runtime::spawn(async move {
            let mut observed = this.state().focus_revision;
            loop {
                tokio::time::sleep(std::time::Duration::from_millis(35)).await;
                let settled = {
                    let mut state = this.state();
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
                if !this.state().model.ready {
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
                this.state()
                    .model
                    .focus(panel_focused, main_focused, app_active);
                if this.snapshot() != old {
                    #[cfg(target_os = "macos")]
                    crate::panel::forget_previous();
                    this.cancel_search(old.session_id);
                    this.present();
                }
            });
        });
    }
    pub fn display_changed(&self) {
        self.on_main(|this| {
            if this.snapshot().visible {
                this.place();
            }
        });
    }
    pub fn destroyed(&self) {
        self.state().closed = true;
        idle::destroyed();
    }
    /// Places the launcher on the display under the pointer, where the user is
    /// looking, at the same height every time.
    fn place(&self) {
        let monitor = self
            .window
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
                            && cursor.x < f64::from(area.position.x) + f64::from(area.size.width)
                            && cursor.y < f64::from(area.position.y) + f64::from(area.size.height)
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
        let placement = geometry::Placement::new(
            f64::from(work.size.width) / scale,
            f64::from(work.size.height) / scale,
            if animated_shapes() {
                geometry::SHAPE_INSET
            } else {
                0.0
            },
        );
        #[cfg(target_os = "windows")]
        {
            let _ = self.window.as_ref().set_bounds(tauri::Rect {
                position: tauri::LogicalPosition::new(0.0, 0.0).into(),
                size: LogicalSize::new(placement.width, placement.height(geometry::MAX_HEIGHT))
                    .into(),
            });
        }
        let height = placement.height(self.state().content_height);
        let _ = self
            .window
            .set_size(LogicalSize::new(placement.width, height));
        if position_supported() {
            let _ = self.window.set_position(PhysicalPosition::new(
                f64::from(work.position.x) + placement.x * scale,
                f64::from(work.position.y) + placement.y * scale,
            ));
        }
        self.state().placement = Some(placement);
    }
    #[cfg(target_os = "windows")]
    fn resize_windows(&self) {
        let (placement, content) = {
            let state = self.state();
            (state.placement, state.content_height)
        };
        let Some(placement) = placement else {
            return;
        };
        let height = placement.height(content);
        let scale = self.window.scale_factor().unwrap_or(1.0);
        if self
            .window
            .inner_size()
            .is_ok_and(|size| (f64::from(size.height) / scale - height).abs() < 1.0)
        {
            return;
        }
        // Chromium has painted the target content in a stable viewport. Commit
        // one clip change: independently animating DWM's material here exposes
        // a trailing band after the DOM has already moved its footer.
        let _ = self
            .window
            .set_size(LogicalSize::new(placement.width, height));
    }
    #[cfg(not(target_os = "windows"))]
    fn resize(&self) {
        let (placement, content) = {
            let state = self.state();
            (state.placement, state.content_height)
        };
        let Some(placement) = placement else {
            return;
        };
        let height = placement.height(content);
        #[cfg(target_os = "macos")]
        crate::panel::set_height(&self.window, height);
        #[cfg(not(target_os = "macos"))]
        let _ = self
            .window
            .set_size(LogicalSize::new(placement.width, height));
    }
}
/// Slightly longer than the shapes' own settle, so the window never shrinks
/// under a shape still moving.
#[cfg(not(target_os = "windows"))]
const SHAPES_SETTLE: std::time::Duration = std::time::Duration::from_millis(300);

/// Native draws the launcher's material as separate shapes, which the window
/// has to leave room around.
fn animated_shapes() -> bool {
    #[cfg(target_os = "macos")]
    {
        crate::panel::shapes_active()
    }
    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}

fn destination(tool: ToolKind) -> &'static str {
    match tool {
        ToolKind::Notes => "browser.notes",
        ToolKind::Tasks => "browser.tasks",
        ToolKind::History => "browser.history",
        ToolKind::Downloads => "browser.downloads",
        // Bookmarks have no full page; the browser opens its panel.
        ToolKind::Bookmarks => "tool.bookmarks",
        ToolKind::Ai => "tool.ai",
        ToolKind::Time => "browser.time",
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
                this.present();
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

/// First-use construction stays on the UI thread; no unused launcher renderer.
#[cfg(not(target_os = "linux"))]
type BuildPanel = Box<dyn FnOnce() -> crate::SetupResult + Send>;
#[cfg(not(target_os = "linux"))]
pub struct Factory(Mutex<FactoryState>);
#[cfg(not(target_os = "linux"))]
type PanelAction = Box<dyn FnOnce(&Overlay) + Send>;
#[cfg(not(target_os = "linux"))]
struct FactoryState {
    build: Option<BuildPanel>,
    queued: Vec<PanelAction>,
}
#[cfg(not(target_os = "linux"))]
impl Factory {
    pub fn new(build: impl FnOnce() -> crate::SetupResult + Send + 'static) -> Self {
        Self(Mutex::new(FactoryState {
            build: Some(Box::new(build)),
            queued: Vec::new(),
        }))
    }
}
pub fn request(app: &tauri::AppHandle, action: impl FnOnce(&Overlay) + Send + 'static) -> bool {
    let handle = app.clone();
    app.run_on_main_thread(move || {
        if crate::shutdown_started(&handle) {
            return;
        }
        if let Some(panel) = handle.try_state::<Overlay>() {
            action(&panel);
            return;
        }
        #[cfg(not(target_os = "linux"))]
        if let Some(factory) = handle.try_state::<Factory>() {
            let build = {
                let mut factory = factory.0.lock().unwrap_or_else(|e| e.into_inner());
                // WebView construction pumps native messages. Retain requests
                // that reenter here while the first request is still building.
                if factory.queued.len() >= 32 {
                    return;
                }
                factory.queued.push(Box::new(action));
                factory.build.take()
            };
            let Some(build) = build else {
                return;
            };
            if let Err(error) = build() {
                factory
                    .0
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .queued
                    .clear();
                crate::request_startup_failure(
                    &handle,
                    format_args!("launcher construction failed: {error}"),
                );
                return;
            }
            let queued =
                std::mem::take(&mut factory.0.lock().unwrap_or_else(|e| e.into_inner()).queued);
            if let Some(panel) = handle.try_state::<Overlay>() {
                for action in queued {
                    if crate::shutdown_started(&handle) {
                        break;
                    }
                    action(&panel);
                }
            }
        }
    })
    .is_ok()
}

#[cfg(test)]
mod context_cache_tests {
    use super::*;
    #[test]
    fn newtab_search_does_not_require_a_launcher_window() {
        let cache = ContextCache::default();
        assert!(cache.newtab_context("tab", 1).is_none());
        *cache.0.lock().unwrap() = Some(Owner {
            private: false,
            window: "window".into(),
            profile: "profile".into(),
            name: "Personal".into(),
            space: "space".into(),
        });
        let context = cache.newtab_context("tab", 7).unwrap();
        assert_eq!(context.window_id, "window");
        assert_eq!(context.profile_id, "profile");
        assert_eq!(context.space_id, "space");
        assert_eq!(context.session_id, "newtab:tab:7");
        assert_eq!(cache.search_private(&context), Some(false));
        cache.0.lock().unwrap().as_mut().unwrap().private = true;
        assert_eq!(cache.search_private(&context), Some(true));
        let mut stale = context.clone();
        stale.profile_id = "another-profile".into();
        assert_eq!(cache.search_private(&stale), None);
        stale = context.clone();
        stale.window_id = "another-window".into();
        assert_eq!(cache.search_private(&stale), None);
        stale = context.clone();
        stale.space_id = "another-space".into();
        assert_eq!(cache.search_private(&stale), None);
        cache.0.lock().unwrap().as_mut().unwrap().space = "next".into();
        assert_eq!(cache.newtab_context("tab", 8).unwrap().space_id, "next");
    }
}
