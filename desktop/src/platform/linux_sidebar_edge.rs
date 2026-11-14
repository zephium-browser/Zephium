use std::cell::RefCell;
use std::rc::Rc;

use gtk::prelude::*;
use tauri::{Manager, WebviewWindow};
use zephium_core::layout::HIDDEN_CARD_INSET;

const MONITOR_KEY: &str = "zephium-linux-sidebar-edge";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize)]
struct EdgeStatus {
    near: bool,
    over: bool,
}

#[derive(Default)]
struct EdgeState {
    hidden: bool,
    near: bool,
    over: bool,
    card_edge: f64,
    last: Option<f64>,
}

impl EdgeState {
    fn set_hidden(&mut self, hidden: bool) -> Option<EdgeStatus> {
        if self.hidden == hidden {
            return None;
        }
        self.hidden = hidden;
        self.moved(None)
    }

    fn set_width(&mut self, width: f64) -> Option<EdgeStatus> {
        if !width.is_finite() || width <= 0.0 {
            return None;
        }
        self.card_edge = width + 3.0 * HIDDEN_CARD_INSET;
        let over = self.last.is_some_and(|x| x <= self.card_edge);
        self.changed(self.near, over)
    }

    fn moved(&mut self, x: Option<f64>) -> Option<EdgeStatus> {
        let x = x.filter(|x| self.hidden && x.is_finite() && *x >= 0.0);
        let near = x.is_some_and(|x| {
            crate::edge_reach::at_edge(x, self.last.map_or(0.0, |last| x - last), self.near)
        });
        let over = x.is_some_and(|x| x <= self.card_edge);
        self.last = x;
        self.changed(near, over)
    }

    fn changed(&mut self, near: bool, over: bool) -> Option<EdgeStatus> {
        if near == self.near && over == self.over {
            return None;
        }
        self.near = near;
        self.over = over;
        Some(EdgeStatus { near, over })
    }
}

struct Observer {
    state: RefCell<EdgeState>,
    emit: Box<dyn Fn(EdgeStatus)>,
}

impl Observer {
    fn update(&self, update: impl FnOnce(&mut EdgeState) -> Option<EdgeStatus>) {
        let change = update(&mut self.state.borrow_mut());
        if let Some(status) = change {
            (self.emit)(status);
        }
    }

    fn motion(&self, controller: &gtk::EventControllerMotion, x: f64, y: f64) {
        let inside = controller
            .widget()
            .and_then(|widget| widget.downcast::<gtk::Window>().ok())
            .is_some_and(|window| {
                window.is_mapped()
                    && x.is_finite()
                    && y.is_finite()
                    && x >= 0.0
                    && y >= 0.0
                    && x < f64::from(window.allocated_width())
                    && y < f64::from(window.allocated_height())
            });
        self.update(|state| state.moved(inside.then_some(x)));
    }

    fn sample_pointer(&self, controller: &gtk::EventControllerMotion) {
        let position = controller.widget().and_then(|widget| {
            let window = widget.window()?;
            let pointer = window.display().default_seat()?.pointer()?;
            let (under_pointer, _, _) = pointer.window_at_position_double();
            if under_pointer?.toplevel() != window.toplevel() {
                return None;
            }
            let (_, x, y, _) = window.device_position_double(&pointer);
            Some((x, y))
        });
        if let Some((x, y)) = position {
            self.motion(controller, x, y);
        } else {
            self.update(|state| state.moved(None));
        }
    }
}

struct Monitor {
    controller: gtk::EventControllerMotion,
    observer: Rc<Observer>,
}

impl Monitor {
    fn new(window: &gtk::ApplicationWindow, emit: impl Fn(EdgeStatus) + 'static) -> Self {
        let observer = Rc::new(Observer {
            state: RefCell::new(EdgeState::default()),
            emit: Box::new(emit),
        });
        let controller = gtk::EventControllerMotion::new(window);
        controller.set_propagation_phase(gtk::PropagationPhase::None);
        let motion = observer.clone();
        controller.connect_motion(move |controller, x, y| motion.motion(controller, x, y));
        let enter = observer.clone();
        controller.connect_enter(move |controller, x, y| enter.motion(controller, x, y));
        let leave = observer.clone();
        // GTK also emits leave when crossing between child WebView surfaces.
        controller.connect_leave(move |controller| leave.sample_pointer(controller));
        let unfocused = observer.clone();
        window.connect_focus_out_event(move |_, _| {
            unfocused.update(|state| state.moved(None));
            gtk::glib::Propagation::Proceed
        });
        let unmapped = observer.clone();
        window.connect_unmap(move |_| unmapped.update(|state| state.moved(None)));
        let destroyed = observer.clone();
        window.connect_destroy(move |_| destroyed.update(|state| state.set_hidden(false)));
        Self {
            controller,
            observer,
        }
    }

    fn set_width(&self, width: f64) {
        self.observer.update(|state| state.set_width(width));
    }

    fn set_hidden(&self, hidden: bool) {
        let changed = self.observer.state.borrow().hidden != hidden;
        self.controller.set_propagation_phase(if hidden {
            gtk::PropagationPhase::Capture
        } else {
            gtk::PropagationPhase::None
        });
        self.observer.update(|state| state.set_hidden(hidden));
        if hidden && changed {
            self.observer.sample_pointer(&self.controller);
        }
    }
}

fn monitor(window: &WebviewWindow, create: bool) -> Option<Rc<Monitor>> {
    let Ok(native) = window.gtk_window() else {
        return None;
    };
    if native.in_destruction() {
        return None;
    }
    // The window owns one monitor. GTK's controller holds only a weak widget
    // pointer; callbacks own state, never the native window or controller.
    // SAFETY: this module alone uses the key, always for Rc<Monitor>.
    let monitor = unsafe { native.data::<Rc<Monitor>>(MONITOR_KEY) };
    let monitor = match monitor {
        // SAFETY: this module alone uses the key, always for Rc<Monitor>.
        Some(monitor) => unsafe { monitor.as_ref().clone() },
        None if !create => return None,
        None => {
            let app = window.app_handle().clone();
            let label = window.label().to_owned();
            let monitor = Rc::new(Monitor::new(&native, move |status| {
                crate::emit_to_privileged(&app, &label, "zephium:sidebar-edge", &status);
            }));
            // SAFETY: the private typed key is released with the native window.
            unsafe { native.set_data(MONITOR_KEY, monitor.clone()) };
            monitor
        }
    };
    Some(monitor)
}

pub(super) fn watch(window: &WebviewWindow, hidden: bool) {
    if let Some(monitor) = monitor(window, hidden) {
        monitor.set_hidden(hidden);
    }
}

pub(super) fn column_width(window: &WebviewWindow, width: f64) {
    if let Some(monitor) = monitor(window, true) {
        monitor.set_width(width);
    }
}

#[cfg(test)]
mod tests {
    use super::{EdgeState, EdgeStatus};

    fn status(near: bool, over: bool) -> Option<EdgeStatus> {
        Some(EdgeStatus { near, over })
    }

    #[test]
    fn visible_sidebars_do_not_track_motion_and_toggles_clear_the_edge() {
        let mut state = EdgeState::default();
        state.set_width(200.0);
        assert_eq!(state.moved(Some(0.0)), None);
        assert_eq!(state.last, None);
        assert_eq!(state.set_hidden(true), None);
        assert_eq!(state.moved(Some(0.0)), status(true, true));
        assert_eq!(state.set_hidden(true), None);
        assert_eq!(state.set_hidden(false), status(false, false));
        assert_eq!(state.moved(Some(0.0)), None);
        assert_eq!(state.set_hidden(false), None);
    }

    #[test]
    fn edge_and_card_transitions_are_deduplicated_independently() {
        let mut state = EdgeState::default();
        state.set_width(40.0);
        state.set_hidden(true);
        assert_eq!(state.moved(Some(40.0)), status(false, true));
        assert_eq!(state.moved(Some(8.0)), status(true, true));
        assert_eq!(state.moved(Some(8.0)), None);
        assert_eq!(state.moved(Some(16.0)), None);
        assert_eq!(state.moved(Some(30.0)), status(false, true));
        assert_eq!(state.moved(Some(64.0)), None);
        assert_eq!(state.moved(Some(65.0)), status(false, false));
        assert_eq!(state.moved(Some(65.0)), None);
        assert_eq!(state.set_width(41.0), status(false, true));
        assert_eq!(state.set_width(41.0), None);
        assert_eq!(state.set_width(40.0), status(false, false));
        for invalid in [f64::NAN, f64::INFINITY, -1.0, 0.0] {
            assert_eq!(state.set_width(invalid), None);
            assert_eq!(state.card_edge, 64.0);
        }
    }

    #[test]
    fn leaving_or_losing_focus_clears_card_and_velocity_before_reentry() {
        let mut state = EdgeState::default();
        state.set_width(40.0);
        state.set_hidden(true);
        assert_eq!(state.moved(Some(80.0)), None);
        assert_eq!(state.moved(Some(40.0)), status(true, true));
        assert_eq!(state.moved(None), status(false, false));
        assert_eq!(state.moved(None), None);
        assert_eq!(state.moved(Some(40.0)), status(false, true));
        for invalid in [f64::NAN, f64::INFINITY, -1.0] {
            assert_eq!(state.moved(Some(0.0)), status(true, true));
            assert_eq!(state.moved(Some(invalid)), status(false, false));
            assert_eq!(state.last, None);
        }
    }
}

#[cfg(test)]
mod native_tests {
    use super::*;
    use std::time::{Duration, Instant};
    use webkit2gtk::WebContextExt;

    fn until(mut condition: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            while gtk::events_pending() {
                gtk::main_iteration_do(false);
            }
            if condition() {
                return;
            }
            assert!(Instant::now() < deadline, "native event deadline");
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    #[test]
    #[ignore = "requires a disposable X11 display; never run on the host desktop"]
    fn capture_observes_consumed_webview_motion_and_clears_lifecycle_state() {
        gtk::init().expect("isolated GTK display");
        let application = gtk::Application::new(
            Some("dev.zephium.sidebar-edge-test"),
            gtk::gio::ApplicationFlags::NON_UNIQUE,
        );
        application
            .register(None::<&gtk::gio::Cancellable>)
            .expect("register isolated application");
        let window = gtk::ApplicationWindow::new(&application);
        window.set_default_size(400, 300);
        let context = webkit2gtk::WebContext::new_ephemeral();
        context.set_sandbox_enabled(true);
        let view = webkit2gtk::WebView::with_context(&context);
        window.add(&view);
        let changes = Rc::new(RefCell::new(Vec::new()));
        let received = changes.clone();
        let monitor = Monitor::new(&window, move |status| received.borrow_mut().push(status));
        monitor.set_width(40.0);
        let inside = EdgeStatus {
            near: true,
            over: true,
        };
        let outside = EdgeStatus::default();
        let order = Rc::new(RefCell::new(Vec::new()));
        let captured = order.clone();
        monitor
            .controller
            .connect_motion(move |_, _, _| captured.borrow_mut().push("capture"));
        let consumed = order.clone();
        view.connect_motion_notify_event(move |_, _| {
            consumed.borrow_mut().push("child");
            gtk::glib::Propagation::Stop
        });
        window.show_all();
        until(|| window.is_mapped());
        let native = window.window().expect("native window");
        native.focus(0);
        until(|| window.is_active());
        let (_, origin_x, origin_y) = native.origin();
        let screen = gtk::prelude::GtkWindowExt::screen(&window).expect("X11 screen");
        let pointer = native
            .display()
            .default_seat()
            .and_then(|seat| seat.pointer())
            .expect("X11 pointer");
        pointer.warp(&screen, origin_x + 4, origin_y + 100);
        until(|| order.borrow().contains(&"child"));
        assert!(changes.borrow().is_empty());
        monitor.set_hidden(true);
        assert_eq!(
            *changes.borrow(),
            [inside],
            "sample an already-resting pointer"
        );
        monitor.set_hidden(true);
        assert_eq!(*changes.borrow(), [inside], "repeated state is inert");

        order.borrow_mut().clear();
        pointer.warp(&screen, origin_x + 100, origin_y + 100);
        until(|| order.borrow().contains(&"child"));
        let events = order.borrow();
        let capture = events
            .iter()
            .position(|event| *event == "capture")
            .expect("capture event");
        let child = events
            .iter()
            .position(|event| *event == "child")
            .expect("consuming child event");
        assert!(capture < child);
        drop(events);
        assert_eq!(*changes.borrow(), [inside, outside]);
        pointer.warp(&screen, origin_x + 4, origin_y + 100);
        until(|| monitor.observer.state.borrow().near);
        pointer.warp(&screen, origin_x + 450, origin_y + 100);
        until(|| !monitor.observer.state.borrow().near);
        pointer.warp(&screen, origin_x + 4, origin_y + 100);
        until(|| monitor.observer.state.borrow().near);

        let other = gtk::ApplicationWindow::new(&application);
        other.move_(600, 400);
        other.show();
        until(|| other.is_mapped());
        other.window().expect("other native window").focus(0);
        until(|| !window.is_active());
        assert!(
            !monitor.observer.state.borrow().near,
            "focus loss clears edge"
        );
        assert!(
            !monitor.observer.state.borrow().over,
            "focus loss clears card"
        );
        pointer.warp(&screen, origin_x + 30, origin_y + 100);
        until(|| monitor.observer.state.borrow().over);
        assert!(!window.is_active(), "hover must not activate the window");
        assert!(!monitor.observer.state.borrow().near);
        monitor.set_width(1.0);
        assert!(
            !monitor.observer.state.borrow().over,
            "resize updates resting pointer"
        );
        monitor.set_width(40.0);
        assert!(monitor.observer.state.borrow().over);
        pointer.warp(&screen, origin_x + 6, origin_y + 100);
        until(|| monitor.observer.state.borrow().near);
        window.hide();
        assert!(!monitor.observer.state.borrow().near, "unmap clears edge");
        assert!(!monitor.observer.state.borrow().over, "unmap clears card");
        window.show();
        native.focus(0);
        until(|| window.is_active());
        pointer.warp(&screen, origin_x + 8, origin_y + 100);
        until(|| monitor.observer.state.borrow().near);
        // SAFETY: no native children are accessed after destroying the fixtures.
        unsafe {
            window.destroy();
            other.destroy();
        }
        assert!(!monitor.observer.state.borrow().over);
        assert!(!monitor.observer.state.borrow().hidden);
        assert!(!monitor.observer.state.borrow().near);
        let weak_controller = monitor.controller.downgrade();
        drop(monitor);
        assert!(weak_controller.upgrade().is_none());
    }
}
