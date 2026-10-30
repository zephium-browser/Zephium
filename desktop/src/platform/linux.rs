use std::cell::RefCell;
use std::collections::BTreeMap;
use std::fmt;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;
use tauri::WebviewWindow;

use zephium_app::{
    ChromePresentation, ChromePresentationCallback, ChromePresentationDispatch, PresentationChrome,
    SharedChrome,
};
use zephium_core::geometry::Size;
use zephium_core::ports::chrome::{Chrome, ChromeFrame};
use zephium_core::ports::engine::Shortcut;
use zephium_engine::MainThreadDispatch;

mod zero_fixed {
    use super::*;

    mod imp {
        use super::*;

        #[derive(Default)]
        pub struct ZeroFixed;

        #[glib::object_subclass]
        impl ObjectSubclass for ZeroFixed {
            const NAME: &'static str = "ZephiumFixed";
            type Type = super::ZeroFixed;
            type ParentType = gtk::Fixed;
        }

        impl ObjectImpl for ZeroFixed {}
        // Children keep their size_request (stable allocations across gtk
        // passes) while the container reports zero, so requests never
        // become a window minimum and live resize does not collapse views.
        impl WidgetImpl for ZeroFixed {
            fn preferred_width(&self) -> (i32, i32) {
                (0, 0)
            }

            fn preferred_height(&self) -> (i32, i32) {
                (0, 0)
            }
        }
        impl ContainerImpl for ZeroFixed {}
        impl FixedImpl for ZeroFixed {}
    }

    glib::wrapper! {
        pub struct ZeroFixed(ObjectSubclass<imp::ZeroFixed>)
            @extends gtk::Fixed, gtk::Container, gtk::Widget;
    }

    impl ZeroFixed {
        pub fn new() -> Self {
            glib::Object::new()
        }
    }
}

#[derive(Debug)]
pub(crate) enum LinuxInitError {
    NotGtkMainThread,
    GtkWindowUnavailable(String),
    DefaultVBoxUnavailable(String),
    PrivilegedChildCount(usize),
    PrivilegedChildIsNotWebView,
    MissingWebContext,
    PersistentWebView,
    PersistentWebContext,
    SandboxDisabled,
    ProcessSwapDisabled,
    ChromeParentMismatch,
    VBoxParentMismatch,
    ChromeReparentPostcondition,
    FixedParentPostcondition,
    TopLevelAlreadyExposed,
    TopLevelExposed,
    ContainerInstall(String),
    CompositionRollbackFailed(String),
    TopLevelHideFailed,
    TopLevelShowFailed,
}

impl fmt::Display for LinuxInitError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotGtkMainThread => formatter
                .write_str("privileged GTK initialization was attempted off the GTK main thread"),
            Self::GtkWindowUnavailable(error) => {
                write!(formatter, "Tauri GTK window is unavailable: {error}")
            }
            Self::DefaultVBoxUnavailable(error) => {
                write!(formatter, "Tauri GTK composition box is unavailable: {error}")
            }
            Self::PrivilegedChildCount(count) => write!(
                formatter,
                "Tauri GTK composition box contains {count} children; expected exactly one privileged WebKitWebView"
            ),
            Self::PrivilegedChildIsNotWebView => write!(
                formatter,
                "Tauri GTK composition box child is not the privileged WebKitWebView"
            ),
            Self::MissingWebContext => {
                formatter.write_str("privileged WebKitGTK view has no WebContext")
            }
            Self::PersistentWebView => formatter.write_str(
                "privileged WebKitGTK view was not constructed in ephemeral mode",
            ),
            Self::PersistentWebContext => formatter.write_str(
                "privileged WebKitGTK view was not constructed with an ephemeral WebContext",
            ),
            Self::SandboxDisabled => {
                formatter.write_str("privileged WebKitGTK process sandbox is disabled")
            }
            Self::ProcessSwapDisabled => formatter
                .write_str("privileged WebKitGTK cross-site process swapping is disabled"),
            Self::ChromeParentMismatch => formatter.write_str(
                "privileged WebKitGTK view is not a direct child of Tauri's composition box",
            ),
            Self::VBoxParentMismatch => formatter.write_str(
                "Tauri's composition box is not a direct child of its GTK window",
            ),
            Self::ChromeReparentPostcondition => formatter.write_str(
                "privileged WebKitGTK view did not enter the Zephium composition container",
            ),
            Self::FixedParentPostcondition => formatter.write_str(
                "Zephium composition container did not become the GTK window's direct child",
            ),
            Self::TopLevelAlreadyExposed => formatter.write_str(
                "privileged GTK window was visible before native hardening completed",
            ),
            Self::TopLevelExposed => formatter.write_str(
                "native composition unexpectedly exposed the privileged top-level window",
            ),
            Self::ContainerInstall(error) => {
                write!(formatter, "could not install the engine GTK container: {error}")
            }
            Self::CompositionRollbackFailed(original) => write!(
                formatter,
                "GTK composition rollback could not restore the hidden Tauri hierarchy after: {original}"
            ),
            Self::TopLevelHideFailed => formatter
                .write_str("privileged GTK top-level window could not be hidden fail-closed"),
            Self::TopLevelShowFailed => formatter
                .write_str("initialized privileged GTK top-level window did not become visible"),
        }
    }
}

impl std::error::Error for LinuxInitError {}

// The Fixed replaces tauri's vbox as the window's direct child: tauri's
// resize handler resolves the window via webview.parent().parent() and
// aborts on deeper nesting. The chrome tracks the window size; the sidebar
// is a region of its DOM and content views overlay it.
pub fn init(window: &WebviewWindow) -> Result<(), LinuxInitError> {
    require_gtk_main_thread()?;
    let gtk_window = window
        .gtk_window()
        .map_err(|error| LinuxInitError::GtkWindowUnavailable(error.to_string()))?;
    require_hidden(&gtk_window)?;
    let vbox = window
        .default_vbox()
        .map_err(|error| LinuxInitError::DefaultVBoxUnavailable(error.to_string()))?;
    let chrome = privileged_webview(&vbox)?;
    harden_privileged_webview(&chrome)?;
    install_composition(
        &gtk_window,
        &vbox,
        &chrome,
        zephium_engine::install_container,
    )
}

fn install_composition(
    gtk_window: &gtk::ApplicationWindow,
    vbox: &gtk::Box,
    chrome: &webkit2gtk::WebView,
    install: impl FnOnce(gtk::Fixed) -> Result<(), String>,
) -> Result<(), LinuxInitError> {
    require_hidden(gtk_window)?;
    let expected_vbox: gtk::Widget = vbox.clone().upcast();
    if chrome.parent().as_ref() != Some(&expected_vbox) {
        return Err(LinuxInitError::ChromeParentMismatch);
    }
    let expected_window: gtk::Widget = gtk_window.clone().upcast();
    if vbox.parent().as_ref() != Some(&expected_window) {
        return Err(LinuxInitError::VBoxParentMismatch);
    }

    let fixed: gtk::Fixed = zero_fixed::ZeroFixed::new().upcast();
    vbox.remove(chrome);
    gtk_window.remove(vbox);
    fixed.put(chrome, 0, 0);
    gtk_window.add(&fixed);

    let sized_chrome = chrome.clone();
    fixed.connect_size_allocate(move |_, allocation| {
        sized_chrome.set_size_request(allocation.width(), allocation.height());
    });
    fixed.show_all();

    let expected_fixed: gtk::Widget = fixed.clone().upcast();
    if gtk_window.is_visible() || gtk_window.is_mapped() {
        return Err(restore_tauri_hierarchy(
            gtk_window,
            vbox,
            &fixed,
            chrome,
            LinuxInitError::TopLevelExposed,
        ));
    }
    if chrome.parent().as_ref() != Some(&expected_fixed) {
        return Err(restore_tauri_hierarchy(
            gtk_window,
            vbox,
            &fixed,
            chrome,
            LinuxInitError::ChromeReparentPostcondition,
        ));
    }
    if fixed.parent().as_ref() != Some(&expected_window) {
        return Err(restore_tauri_hierarchy(
            gtk_window,
            vbox,
            &fixed,
            chrome,
            LinuxInitError::FixedParentPostcondition,
        ));
    }
    if let Err(error) = install(fixed.clone()) {
        return Err(restore_tauri_hierarchy(
            gtk_window,
            vbox,
            &fixed,
            chrome,
            LinuxInitError::ContainerInstall(error),
        ));
    }
    Ok(())
}

/// Tauri has no permission-handler builder. The top-level response policy
/// blocks ambient APIs before script runs; this native signal is a second
/// fail-closed layer for every privileged WebKitGTK view.
pub fn harden_privileged(window: &WebviewWindow) -> Result<(), LinuxInitError> {
    require_gtk_main_thread()?;
    let gtk_window = window
        .gtk_window()
        .map_err(|error| LinuxInitError::GtkWindowUnavailable(error.to_string()))?;
    require_hidden(&gtk_window)?;
    let vbox = window
        .default_vbox()
        .map_err(|error| LinuxInitError::DefaultVBoxUnavailable(error.to_string()))?;
    let chrome = privileged_webview(&vbox)?;
    harden_privileged_webview(&chrome)
}

fn privileged_webview(vbox: &gtk::Box) -> Result<webkit2gtk::WebView, LinuxInitError> {
    let mut children = vbox.children();
    if children.len() != 1 {
        return Err(LinuxInitError::PrivilegedChildCount(children.len()));
    }
    children
        .pop()
        .and_then(|child| child.downcast::<webkit2gtk::WebView>().ok())
        .ok_or(LinuxInitError::PrivilegedChildIsNotWebView)
}

fn harden_privileged_webview(chrome: &webkit2gtk::WebView) -> Result<(), LinuxInitError> {
    use webkit2gtk::{
        DownloadExt, FileChooserRequestExt, PermissionRequestExt, WebContextExt, WebViewExt,
    };

    chrome.connect_permission_request(|_, request| {
        request.deny();
        true
    });
    chrome.connect_run_file_chooser(|_, request| {
        request.cancel();
        true
    });
    chrome.connect_script_dialog(|_, dialog| {
        dialog.close();
        true
    });
    chrome.connect_context_menu(|_, _, _, _| true);

    if !chrome.is_ephemeral() {
        return Err(LinuxInitError::PersistentWebView);
    }
    let context = chrome.context().ok_or(LinuxInitError::MissingWebContext)?;
    if !context.is_ephemeral() {
        return Err(LinuxInitError::PersistentWebContext);
    }
    if !context.is_sandbox_enabled() {
        return Err(LinuxInitError::SandboxDisabled);
    }
    if !context.is_process_swap_on_cross_site_navigation_enabled() {
        return Err(LinuxInitError::ProcessSwapDisabled);
    }

    const DOWNLOAD_DENY_MARKER: &str = "zephium-privileged-download-deny-installed";
    // SAFETY: this private key is used only as a bool in this module and is
    // destroyed with the GLib WebContext.
    let download_deny = unsafe { context.data::<bool>(DOWNLOAD_DENY_MARKER).is_some() };
    if !download_deny {
        context.connect_download_started(|_, download| download.cancel());
        // SAFETY: see the typed private-key invariant above.
        unsafe { context.set_data(DOWNLOAD_DENY_MARKER, true) };
    }
    Ok(())
}

fn require_gtk_main_thread() -> Result<(), LinuxInitError> {
    if gtk::is_initialized_main_thread() {
        Ok(())
    } else {
        Err(LinuxInitError::NotGtkMainThread)
    }
}

fn require_hidden(gtk_window: &gtk::ApplicationWindow) -> Result<(), LinuxInitError> {
    if !gtk_window.is_visible() && !gtk_window.is_mapped() {
        return Ok(());
    }
    gtk_window.hide();
    if gtk_window.is_visible() || gtk_window.is_mapped() {
        Err(LinuxInitError::TopLevelHideFailed)
    } else {
        Err(LinuxInitError::TopLevelAlreadyExposed)
    }
}

/// Reveal the initialized toplevel without recursively changing native-child
/// visibility. Tao's Linux `Window::show` uses `gtk_window.show_all()`, which
/// would override the content stage's hidden WebKitGTK widgets after session
/// restoration and place their native input surfaces above trusted chrome.
pub fn show_initialized_top_level(window: &WebviewWindow) -> Result<(), LinuxInitError> {
    require_gtk_main_thread()?;
    let gtk_window = window
        .gtk_window()
        .map_err(|error| LinuxInitError::GtkWindowUnavailable(error.to_string()))?;
    show_top_level_only(&gtk_window)
}

fn show_top_level_only(gtk_window: &gtk::ApplicationWindow) -> Result<(), LinuxInitError> {
    gtk_window.show();
    if gtk_window.is_visible() {
        Ok(())
    } else {
        Err(LinuxInitError::TopLevelShowFailed)
    }
}

fn remove_from_actual_parent(widget: &gtk::Widget) {
    if let Some(parent) = widget
        .parent()
        .and_then(|parent| parent.downcast::<gtk::Container>().ok())
    {
        parent.remove(widget);
    }
}

fn restore_tauri_hierarchy(
    gtk_window: &gtk::ApplicationWindow,
    vbox: &gtk::Box,
    fixed: &gtk::Fixed,
    chrome: &webkit2gtk::WebView,
    failure: LinuxInitError,
) -> LinuxInitError {
    gtk_window.hide();
    let chrome_widget: gtk::Widget = chrome.clone().upcast();
    let fixed_widget: gtk::Widget = fixed.clone().upcast();
    let vbox_widget: gtk::Widget = vbox.clone().upcast();
    remove_from_actual_parent(&chrome_widget);
    remove_from_actual_parent(&fixed_widget);
    remove_from_actual_parent(&vbox_widget);
    gtk_window.add(vbox);
    vbox.pack_start(chrome, true, true, 0);

    let expected_window: gtk::Widget = gtk_window.clone().upcast();
    let restored = chrome.parent().as_ref() == Some(&vbox_widget)
        && vbox.parent().as_ref() == Some(&expected_window)
        && fixed.parent().is_none()
        && vbox.children().len() == 1
        && !gtk_window.is_visible()
        && !gtk_window.is_mapped();
    if restored {
        failure
    } else {
        LinuxInitError::CompositionRollbackFailed(failure.to_string())
    }
}

#[cfg(test)]
mod native_composition_tests {
    use super::*;

    fn tauri_tree(
        application: &gtk::Application,
    ) -> (
        gtk::ApplicationWindow,
        gtk::Box,
        webkit2gtk::WebView,
        webkit2gtk::WebContext,
    ) {
        let window = gtk::ApplicationWindow::new(application);
        let vbox = gtk::Box::new(gtk::Orientation::Vertical, 0);
        window.add(&vbox);
        let context = webkit2gtk::WebContext::new_ephemeral();
        let chrome = webkit2gtk::WebView::with_context(&context);
        vbox.pack_start(&chrome, true, true, 0);
        (window, vbox, chrome, context)
    }

    #[test]
    #[ignore = "requires a native GTK display"]
    fn linux_composition_transaction_preserves_hidden_state_and_rolls_back_exactly() {
        gtk::init().expect("GTK display");
        let application = gtk::Application::new(
            Some("dev.zephium.native-composition-test"),
            gtk::gio::ApplicationFlags::NON_UNIQUE,
        );
        application
            .register(None::<&gtk::gio::Cancellable>)
            .expect("register GTK test application");

        let (window, vbox, chrome, _context) = tauri_tree(&application);
        install_composition(&window, &vbox, &chrome, |_| Ok(())).expect("composition success");
        let fixed = chrome
            .parent()
            .and_then(|parent| parent.downcast::<gtk::Fixed>().ok())
            .expect("GtkFixed parent, including security-owned subclasses");
        let expected_window: gtk::Widget = window.clone().upcast();
        assert_eq!(fixed.parent().as_ref(), Some(&expected_window));
        assert!(!window.is_visible());
        assert!(!window.is_mapped());

        let (rollback_window, rollback_vbox, rollback_chrome, _rollback_context) =
            tauri_tree(&application);
        let error = install_composition(&rollback_window, &rollback_vbox, &rollback_chrome, |_| {
            Err("injected container installation failure".into())
        })
        .expect_err("injected installation failure");
        assert!(matches!(error, LinuxInitError::ContainerInstall(_)));
        let expected_vbox: gtk::Widget = rollback_vbox.clone().upcast();
        let expected_window: gtk::Widget = rollback_window.clone().upcast();
        assert_eq!(rollback_chrome.parent().as_ref(), Some(&expected_vbox));
        assert_eq!(rollback_vbox.parent().as_ref(), Some(&expected_window));
        assert!(!rollback_window.is_visible());
        assert!(!rollback_window.is_mapped());

        let (exposed_window, _exposed_vbox, _exposed_chrome, _exposed_context) =
            tauri_tree(&application);
        exposed_window.show_all();
        assert!(matches!(
            require_hidden(&exposed_window),
            Err(LinuxInitError::TopLevelAlreadyExposed)
        ));
        assert!(!exposed_window.is_visible());
        assert!(!exposed_window.is_mapped());

        let (reveal_window, reveal_vbox, reveal_chrome, _reveal_context) = tauri_tree(&application);
        let stage = gtk::Fixed::new();
        reveal_vbox.remove(&reveal_chrome);
        reveal_window.remove(&reveal_vbox);
        stage.put(&reveal_chrome, 0, 0);
        let hidden_content = gtk::DrawingArea::new();
        stage.put(&hidden_content, 0, 0);
        reveal_window.add(&stage);
        stage.show_all();
        hidden_content.set_sensitive(false);
        hidden_content.set_child_visible(false);
        hidden_content.hide();

        show_top_level_only(&reveal_window).expect("non-recursive top-level reveal");
        while gtk::events_pending() {
            gtk::main_iteration_do(false);
        }
        assert!(reveal_window.is_visible());
        assert!(stage.is_visible());
        assert!(reveal_chrome.is_visible());
        assert!(!hidden_content.is_child_visible());
        assert!(!hidden_content.is_mapped());
        assert!(!hidden_content.is_sensitive());
    }
}

/// Browser shortcuts fire regardless of focus: handlers connected on the
/// toplevel run before gtk forwards the key to the focused widget, so a
/// content webview never swallows Ctrl+T. Mirrors the Windows
/// AcceleratorKeyPressed hook; the table is the same resolved keymap.
pub fn install_shortcuts(
    window: &WebviewWindow,
    shortcuts: std::sync::Arc<std::sync::RwLock<Vec<Shortcut>>>,
    presses: FocusedShortcutPresses,
    on: impl Fn(&str) + 'static,
) {
    let Ok(gtk_window) = window.gtk_window() else {
        return;
    };
    presses.register_window(&gtk_window);
    let pressed = presses.clone();
    gtk_window.connect_key_press_event(move |_, event| {
        let state = event.state();
        let modifiers = observed_shortcut_modifiers(state);
        let keyval = normalize_keyval(event.keyval());
        // The table carries a bare key only while it means something (the
        // Work pane's Escape), so unmodified typing is otherwise untouched.
        let hit = shortcuts
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .find(|s| shortcut_modifiers(s) == modifiers && vk_keyval(s.key) == Some(keyval))
            .map(|s| s.id.clone());
        let Some(id) = hit else {
            return glib::Propagation::Proceed;
        };
        if pressed.admit(event.hardware_keycode()) {
            on(&id);
        }
        glib::Propagation::Stop
    });

    let released = presses.clone();
    gtk_window.connect_key_release_event(move |_, event| {
        released.release_after_repeat_grace(event.hardware_keycode());
        glib::Propagation::Proceed
    });

    // A release can be delivered to another application after focus leaves
    // Zephium. Clear only after the compositor has had time to transfer focus
    // between Zephium's main and panel windows; clearing synchronously on the
    // main window's focus-out would let the same held key repeat into the
    // newly-focused launcher and immediately close it again.
    let focus_presses = presses;
    gtk_window.connect_focus_out_event(move |_, _| {
        focus_presses.clear_if_application_unfocused_after_grace();
        glib::Propagation::Proceed
    });
}

const KEY_RELEASE_REPEAT_GRACE: Duration = Duration::from_millis(40);
const APPLICATION_FOCUS_TRANSFER_GRACE: Duration = Duration::from_millis(500);

#[derive(Clone, Default)]
pub struct FocusedShortcutPresses {
    inner: Rc<RefCell<FocusedShortcutPressState>>,
}

#[derive(Default)]
struct FocusedShortcutPressState {
    pressed: BTreeMap<u16, u64>,
    next_generation: u64,
    windows: Vec<glib::WeakRef<gtk::Window>>,
}

impl FocusedShortcutPresses {
    fn register_window(&self, window: &gtk::ApplicationWindow) {
        let weak = glib::WeakRef::new();
        weak.set(Some(&window.clone().upcast::<gtk::Window>()));
        self.inner.borrow_mut().windows.push(weak);
    }

    fn admit(&self, hardware_keycode: u16) -> bool {
        let mut state = self.inner.borrow_mut();
        state.next_generation = state.next_generation.wrapping_add(1);
        if state.next_generation == 0 {
            state.next_generation = 1;
        }
        let generation = state.next_generation;
        let first = !state.pressed.contains_key(&hardware_keycode);
        state.pressed.insert(hardware_keycode, generation);
        first
    }

    fn release_after_repeat_grace(&self, hardware_keycode: u16) {
        let generation = self.inner.borrow().pressed.get(&hardware_keycode).copied();
        let Some(generation) = generation else {
            return;
        };
        let state = self.clone();
        glib::timeout_add_local_once(KEY_RELEASE_REPEAT_GRACE, move || {
            let mut state = state.inner.borrow_mut();
            if state.pressed.get(&hardware_keycode) == Some(&generation) {
                state.pressed.remove(&hardware_keycode);
            }
        });
    }

    fn clear_if_application_unfocused_after_grace(&self) {
        let state = self.clone();
        glib::timeout_add_local_once(APPLICATION_FOCUS_TRANSFER_GRACE, move || {
            // Never hold the RefCell across a GObject accessor: a platform
            // call may dispatch nested GTK work. Detach weak references,
            // resolve them, then restore the still-live set before querying
            // native focus state.
            let weak_windows = {
                let mut inner = state.inner.borrow_mut();
                std::mem::take(&mut inner.windows)
            };
            let mut live_weak = Vec::with_capacity(weak_windows.len());
            let mut live_windows = Vec::with_capacity(weak_windows.len());
            for weak in weak_windows {
                if let Some(window) = weak.upgrade() {
                    live_windows.push(window);
                    live_weak.push(weak);
                }
            }
            let generation = {
                let mut inner = state.inner.borrow_mut();
                inner.windows.extend(live_weak);
                inner.next_generation
            };
            let application_is_focused = live_windows.iter().any(|window| window.is_active());
            if !application_is_focused {
                let mut inner = state.inner.borrow_mut();
                if inner.next_generation == generation {
                    inner.pressed.clear();
                }
            }
        });
    }
}

fn shortcut_modifiers(shortcut: &Shortcut) -> gtk::gdk::ModifierType {
    let mut modifiers = gtk::gdk::ModifierType::empty();
    if shortcut.ctrl {
        modifiers |= gtk::gdk::ModifierType::CONTROL_MASK;
    }
    if shortcut.shift {
        modifiers |= gtk::gdk::ModifierType::SHIFT_MASK;
    }
    if shortcut.alt {
        modifiers |= gtk::gdk::ModifierType::MOD1_MASK;
    }
    modifiers
}

fn observed_shortcut_modifiers(state: gtk::gdk::ModifierType) -> gtk::gdk::ModifierType {
    observed_shortcut_modifiers_with_mask(state, gtk::accelerator_get_default_mod_mask())
}

fn observed_shortcut_modifiers_with_mask(
    state: gtk::gdk::ModifierType,
    accelerator_mask: gtk::gdk::ModifierType,
) -> gtk::gdk::ModifierType {
    // GTK's default accelerator mask deliberately excludes lock modifiers.
    // Keep the virtual modifiers explicit as a defense against a process-wide
    // mask override: the shared Linux parser rejects Meta/Super/Hyper, so the
    // focused fallback must never accept them as an invisible extra chord.
    let relevant = accelerator_mask
        | gtk::gdk::ModifierType::SUPER_MASK
        | gtk::gdk::ModifierType::HYPER_MASK
        | gtk::gdk::ModifierType::META_MASK;
    state & relevant
}

#[cfg(test)]
mod shortcut_tests {
    use super::*;

    fn launcher_shortcut() -> Shortcut {
        Shortcut {
            id: "launcher.toggle".to_owned(),
            ctrl: true,
            shift: true,
            alt: false,
            key: 0x20,
        }
    }

    #[test]
    fn focused_shortcut_requires_exact_non_lock_modifiers() {
        // Exercise the pure filter without GTK initialization or a display.
        let observed_shortcut_modifiers = |state| {
            observed_shortcut_modifiers_with_mask(
                state,
                gtk::gdk::ModifierType::CONTROL_MASK
                    | gtk::gdk::ModifierType::SHIFT_MASK
                    | gtk::gdk::ModifierType::MOD1_MASK,
            )
        };
        let expected = shortcut_modifiers(&launcher_shortcut());
        assert_eq!(
            observed_shortcut_modifiers(
                expected | gtk::gdk::ModifierType::LOCK_MASK | gtk::gdk::ModifierType::MOD2_MASK,
            ),
            expected
        );
        for unexpected in [
            gtk::gdk::ModifierType::MOD1_MASK,
            gtk::gdk::ModifierType::SUPER_MASK,
            gtk::gdk::ModifierType::HYPER_MASK,
            gtk::gdk::ModifierType::META_MASK,
        ] {
            assert_ne!(observed_shortcut_modifiers(expected | unexpected), expected);
        }
        // Even a process-wide override cannot hide virtual modifiers.
        assert_eq!(
            observed_shortcut_modifiers_with_mask(
                gtk::gdk::ModifierType::SUPER_MASK,
                gtk::gdk::ModifierType::empty(),
            ),
            gtk::gdk::ModifierType::SUPER_MASK,
        );
    }

    #[test]
    fn focused_shortcut_press_is_shared_and_repeat_suppressed() {
        let presses = FocusedShortcutPresses::default();
        assert!(presses.admit(65));
        assert!(!presses.admit(65));
        assert!(presses.admit(66));

        let source = include_str!("linux.rs");
        let adapter = source
            .split("pub fn install_shortcuts(")
            .nth(1)
            .and_then(|source| source.split("const KEY_RELEASE_REPEAT_GRACE").next())
            .expect("focused GTK shortcut adapter");
        assert!(adapter.contains("connect_key_release_event"));
        assert!(adapter.contains("release_after_repeat_grace"));
        assert!(adapter.contains("connect_focus_out_event"));
        assert!(adapter.contains("clear_if_application_unfocused_after_grace"));
    }
}

// Shift+Tab arrives as ISO_Left_Tab; letters arrive in shifted case.
fn normalize_keyval(key: gtk::gdk::keys::Key) -> u32 {
    let raw: u32 = *key;
    if raw == 0xfe20 {
        return 0xff09;
    }
    match key.to_unicode() {
        Some(c) if c.is_ascii_graphic() || c == ' ' => c.to_ascii_lowercase() as u32,
        _ => raw,
    }
}

// The shared shortcut table speaks Windows VK codes; translate to keyvals.
fn vk_keyval(vk: u32) -> Option<u32> {
    Some(match vk {
        0x08 => 0xff08,
        0x09 => 0xff09,
        0x0D => 0xff0d,
        0x1B => 0xff1b,
        0x20 => 0x20,
        0x21 => 0xff55,
        0x22 => 0xff56,
        0x23 => 0xff57,
        0x24 => 0xff50,
        0x25 => 0xff51,
        0x26 => 0xff52,
        0x27 => 0xff53,
        0x28 => 0xff54,
        0x2E => 0xffff,
        f @ 0x70..=0x7B => 0xffbe + (f - 0x70),
        0xBA => ';' as u32,
        0xBB => '=' as u32,
        0xBC => ',' as u32,
        0xBD => '-' as u32,
        0xBE => '.' as u32,
        0xBF => '/' as u32,
        0xC0 => '`' as u32,
        0xDB => '[' as u32,
        0xDC => '\\' as u32,
        0xDD => ']' as u32,
        0xDE => '\'' as u32,
        v @ 0x41..=0x5A => (v as u8).to_ascii_lowercase() as u32,
        v @ 0x30..=0x39 => v,
        _ => return None,
    })
}

// Full-window chrome: client coords already are window coords.
/// Linux is not an onboarding target; the handover shows the page as it
/// loads.
pub fn set_chrome_hidden(_window: &WebviewWindow, _hidden: bool) -> bool {
    true
}

pub fn to_window(x: f64, y: f64) -> (f64, f64) {
    (x, y)
}

pub fn make_chrome(window: &WebviewWindow, _dispatch: MainThreadDispatch) -> SharedChrome {
    Arc::new(ChromeAdapter {
        window: window.clone(),
    })
}

struct ChromeAdapter {
    window: WebviewWindow,
}

impl Chrome for ChromeAdapter {
    fn position(&self, _frame: ChromeFrame) -> bool {
        true
    }
}

impl PresentationChrome for ChromeAdapter {
    fn restore_browser_chrome(
        &self,
        revision: u64,
        items: zephium_ipc::ItemsState,
        done: ChromePresentationCallback,
    ) -> ChromePresentationDispatch {
        crate::restore_browser_chrome(&self.window, revision, items, done)
    }

    fn apply_tab_for_presentation(
        &self,
        presentation: ChromePresentation,
        done: ChromePresentationCallback,
    ) -> ChromePresentationDispatch {
        crate::apply_chrome_presentation(&self.window, presentation, done)
    }
}

pub fn content_size(_window: &WebviewWindow) -> Option<Size> {
    None
}
