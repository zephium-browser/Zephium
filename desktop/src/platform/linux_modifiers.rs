use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};

use gtk::{gdk, glib, prelude::*};

const MONITOR_KEY: &str = "zephium-linux-modifier-monitor";
static CONTROL_DOWN: AtomicBool = AtomicBool::new(false);

thread_local! {
    static TRACKER: Rc<RefCell<Tracker>> = Rc::new(RefCell::new(Tracker::default()));
}

#[derive(Default)]
struct ControlKeys {
    left: bool,
    right: bool,
    inherited: bool,
}

impl ControlKeys {
    fn down(&self) -> bool {
        self.left || self.right || self.inherited
    }

    fn observe(&mut self, control: bool) {
        if !control {
            *self = Self::default();
        } else if !self.down() {
            self.inherited = true;
        }
    }

    fn key(&mut self, key: u32, pressed: bool, modifiers: gdk::ModifierType) {
        let left = key == *gdk::keys::constants::Control_L;
        let right = key == *gdk::keys::constants::Control_R;
        if !left && !right {
            self.observe(modifiers.contains(gdk::ModifierType::CONTROL_MASK));
        } else {
            if left {
                self.left = pressed;
            } else {
                self.right = pressed;
            }
            if !pressed {
                // GDK key-release state still includes the released modifier.
                self.inherited = false;
            }
        }
    }
}

#[derive(Default)]
struct Tracker {
    keys: ControlKeys,
    windows: Vec<glib::WeakRef<gtk::ApplicationWindow>>,
    reconcile_queued: bool,
}

fn publish(tracker: &Tracker) {
    CONTROL_DOWN.store(tracker.keys.down(), Ordering::Release);
}

pub(super) fn control_down() -> bool {
    CONTROL_DOWN.load(Ordering::Acquire)
}

fn reconcile_focus(tracker: &Rc<RefCell<Tracker>>) {
    if std::mem::replace(&mut tracker.borrow_mut().reconcile_queued, true) {
        return;
    }
    let weak = Rc::downgrade(tracker);
    glib::idle_add_local_once(move || {
        let Some(tracker) = weak.upgrade() else {
            return;
        };
        let windows = {
            let mut tracker = tracker.borrow_mut();
            tracker.reconcile_queued = false;
            tracker.windows.retain(|window| window.upgrade().is_some());
            tracker.windows.clone()
        };
        let active = windows
            .iter()
            .filter_map(glib::WeakRef::upgrade)
            .any(|window| !window.in_destruction() && window.is_mapped() && window.is_active());
        if !active {
            let mut tracker = tracker.borrow_mut();
            tracker.keys.observe(false);
            publish(&tracker);
        }
    });
}

pub(super) fn install(window: &gtk::ApplicationWindow) {
    // SAFETY: this module alone owns this key and stores this controller type.
    if unsafe { window.data::<gtk::EventControllerKey>(MONITOR_KEY) }.is_some() {
        return;
    }
    let tracker = TRACKER.with(Rc::clone);
    {
        let mut tracker = tracker.borrow_mut();
        tracker.windows.retain(|window| window.upgrade().is_some());
        tracker.windows.push(window.downgrade());
    }
    let controller = gtk::EventControllerKey::new(window);
    controller.set_propagation_phase(gtk::PropagationPhase::Capture);
    let pressed = tracker.clone();
    controller.connect_key_pressed(move |_, key, _, modifiers| {
        let mut tracker = pressed.borrow_mut();
        tracker.keys.key(key, true, modifiers);
        publish(&tracker);
        false
    });
    let released = tracker.clone();
    controller.connect_key_released(move |_, key, _, modifiers| {
        let mut tracker = released.borrow_mut();
        tracker.keys.key(key, false, modifiers);
        publish(&tracker);
    });
    let focused = tracker.clone();
    window.connect_focus_in_event(move |window, _| {
        let control = gdk::Keymap::for_display(&WidgetExt::display(window)).is_some_and(|keymap| {
            keymap.modifier_state() & gdk::ModifierType::CONTROL_MASK.bits() != 0
        });
        let mut tracker = focused.borrow_mut();
        tracker.keys.observe(control);
        publish(&tracker);
        glib::Propagation::Proceed
    });
    let unfocused = tracker.clone();
    window.connect_focus_out_event(move |_, _| {
        // A main-to-panel focus transfer must not commit a held Ctrl+Tab.
        reconcile_focus(&unfocused);
        glib::Propagation::Proceed
    });
    let unmapped = tracker.clone();
    window.connect_unmap(move |_| reconcile_focus(&unmapped));
    window.connect_destroy(move |_| reconcile_focus(&tracker));
    // GTK's controller has a weak widget link; the window owns its lifetime.
    // SAFETY: the private typed key is released with the native window.
    unsafe { window.set_data(MONITOR_KEY, controller) };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn either_control_key_remains_down_until_both_are_released() {
        let mut keys = ControlKeys::default();
        keys.key(
            *gdk::keys::constants::Control_L,
            true,
            gdk::ModifierType::empty(),
        );
        keys.key(
            *gdk::keys::constants::Control_R,
            true,
            gdk::ModifierType::CONTROL_MASK,
        );
        keys.key(
            *gdk::keys::constants::Control_L,
            false,
            gdk::ModifierType::CONTROL_MASK,
        );
        assert!(keys.down());
        keys.key(
            *gdk::keys::constants::Control_R,
            false,
            gdk::ModifierType::CONTROL_MASK,
        );
        assert!(!keys.down());
    }

    #[test]
    fn inherited_modifier_is_recovered_from_events_and_cleared_on_release() {
        let mut keys = ControlKeys::default();
        keys.key(
            *gdk::keys::constants::Tab,
            true,
            gdk::ModifierType::CONTROL_MASK,
        );
        assert!(keys.down());
        keys.key(
            *gdk::keys::constants::Tab,
            false,
            gdk::ModifierType::CONTROL_MASK,
        );
        assert!(keys.down());
        keys.key(
            *gdk::keys::constants::Control_R,
            false,
            gdk::ModifierType::CONTROL_MASK,
        );
        assert!(!keys.down());
        keys.observe(true);
        keys.key(*gdk::keys::constants::a, true, gdk::ModifierType::empty());
        assert!(!keys.down());
    }

    #[test]
    fn focus_loss_clears_known_and_inherited_modifiers() {
        let mut keys = ControlKeys {
            left: true,
            right: true,
            inherited: true,
        };
        keys.observe(false);
        assert!(!keys.down());
        keys.observe(true);
        assert!(keys.down());
        keys.observe(false);
        assert!(!keys.down());
    }
}

#[cfg(test)]
mod native_tests {
    use super::*;
    use std::ffi::{c_char, c_void, CStr};
    use std::time::{Duration, Instant};
    use webkit2gtk::WebContextExt;

    #[link(name = "xdo")]
    unsafe extern "C" {
        fn xdo_new(display: *const c_char) -> *mut c_void;
        fn xdo_send_keysequence_window_down(
            handle: *mut c_void,
            window: usize,
            keys: *const c_char,
            delay: u32,
        ) -> i32;
        fn xdo_send_keysequence_window_up(
            handle: *mut c_void,
            window: usize,
            keys: *const c_char,
            delay: u32,
        ) -> i32;
        fn xdo_free(handle: *mut c_void);
    }

    struct Keyboard(*mut c_void);
    impl Keyboard {
        fn key(&self, key: &CStr, pressed: bool) {
            // SAFETY: this test owns the private X11 connection and static key name.
            let result = unsafe {
                if pressed {
                    xdo_send_keysequence_window_down(self.0, 0, key.as_ptr(), 0)
                } else {
                    xdo_send_keysequence_window_up(self.0, 0, key.as_ptr(), 0)
                }
            };
            assert_eq!(result, 0, "private XTest key event");
        }
    }
    impl Drop for Keyboard {
        fn drop(&mut self) {
            // SAFETY: release both modifiers even if a fixture assertion panics.
            unsafe {
                xdo_send_keysequence_window_up(self.0, 0, c"Control_L".as_ptr(), 0);
                xdo_send_keysequence_window_up(self.0, 0, c"Control_R".as_ptr(), 0);
                xdo_free(self.0);
            }
        }
    }
    fn until(mut condition: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            while gtk::events_pending() {
                gtk::main_iteration_do(false);
            }
            if condition() {
                return;
            }
            assert!(Instant::now() < deadline, "native modifier event deadline");
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    #[test]
    #[ignore = "requires a disposable X11 display; never run on the host desktop"]
    fn capture_tracks_both_controls_across_panel_focus_and_clears_lifecycle_state() {
        assert_eq!(std::env::var("GDK_BACKEND").as_deref(), Ok("x11"));
        gtk::init().expect("isolated GTK display");
        let application = gtk::Application::new(
            Some("dev.zephium.modifier-test"),
            gtk::gio::ApplicationFlags::NON_UNIQUE,
        );
        application
            .register(None::<&gtk::gio::Cancellable>)
            .expect("register isolated app");
        let main = gtk::ApplicationWindow::new(&application);
        main.set_default_size(400, 300);
        let context = webkit2gtk::WebContext::new_ephemeral();
        context.set_sandbox_enabled(true);
        main.add(&webkit2gtk::WebView::with_context(&context));
        install(&main);
        install(&main);
        let consumed = Rc::new(std::cell::Cell::new(false));
        let received = consumed.clone();
        main.connect_key_press_event(move |_, event| {
            if event.keyval() == gdk::keys::constants::Control_L {
                assert!(control_down(), "capture precedes consuming native handler");
                received.set(true);
            }
            glib::Propagation::Stop
        });
        let panel = gtk::ApplicationWindow::new(&application);
        panel.add(&gtk::Entry::new());
        install(&panel);
        let outside = gtk::ApplicationWindow::new(&application);
        main.show_all();
        main.window().expect("main native window").focus(0);
        until(|| main.is_active());
        // SAFETY: the wrapper supplies this disposable X11 display only.
        let keyboard = Keyboard(unsafe { xdo_new(std::ptr::null()) });
        assert!(!keyboard.0.is_null());
        keyboard.key(c"Control_L", true);
        until(|| consumed.get());
        keyboard.key(c"Control_R", true);
        until(|| TRACKER.with(|tracker| tracker.borrow().keys.right));
        keyboard.key(c"Control_L", false);
        until(|| TRACKER.with(|tracker| !tracker.borrow().keys.left));
        assert!(control_down());
        panel.show_all();
        panel.window().expect("panel native window").focus(0);
        until(|| panel.is_active());
        assert!(
            control_down(),
            "internal focus transfer retains held Control"
        );
        keyboard.key(c"Control_R", false);
        until(|| !control_down());
        keyboard.key(c"Control_L", true);
        until(control_down);
        outside.show();
        outside.window().expect("outside native window").focus(0);
        until(|| outside.is_active() && !control_down());
        keyboard.key(c"Control_L", false);
        main.window().expect("main native window").focus(0);
        until(|| main.is_active());
        keyboard.key(c"Control_L", true);
        until(control_down);
        main.hide();
        until(|| !control_down());
        keyboard.key(c"Control_L", false);
        main.show();
        main.window().expect("main native window").focus(0);
        until(|| main.is_active());
        keyboard.key(c"Control_R", true);
        until(control_down);
        // SAFETY: no fixture widgets are used after destruction.
        unsafe {
            main.destroy();
            panel.destroy();
            outside.destroy();
        }
        until(|| !control_down());
    }
}
