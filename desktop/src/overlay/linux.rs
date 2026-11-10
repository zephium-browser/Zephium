use gtk::prelude::*;

use crate::linux_display::{display_backend, DisplayBackend};

/// Refresh the owner before mapping, returning whether global coordinates exist.
pub(super) fn prepare(
    window: &gtk::ApplicationWindow,
    owner: Option<&gtk::ApplicationWindow>,
) -> bool {
    let backend = display_backend(window);
    // An unmapped/minimized owner must not prevent the global launcher from
    // being mapped independently. Reattach on its next presentation when the
    // browser is visible again.
    let parent = owner
        .filter(|owner| {
            owner.is_mapped()
                && owner.window().is_some_and(|window| {
                    !window.state().contains(gtk::gdk::WindowState::ICONIFIED)
                })
        })
        .map(|owner| owner.upcast_ref::<gtk::Window>());
    if window.transient_for().as_ref() != parent {
        window.set_transient_for(parent);
    }
    if backend == DisplayBackend::Wayland {
        // A placement hint only: Wayland compositors choose the final position
        // and do not expose global pointer coordinates or absolute movement.
        let position = if parent.is_some() {
            gtk::WindowPosition::CenterOnParent
        } else {
            gtk::WindowPosition::None
        };
        if window.window_position() != position {
            window.set_position(position);
        }
    }
    backend == DisplayBackend::X11
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::rc::Rc;
    use std::time::{Duration, Instant};

    fn wait_for(mut ready: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            while gtk::events_pending() {
                gtk::main_iteration_do(false);
            }
            if ready() {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "GTK window mapping did not settle"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    #[ignore = "requires a disposable X11 or Wayland display; never run on the host desktop"]
    fn launcher_parent_tracks_owner_without_blocking_independent_mapping() {
        gtk::init().expect("isolated GTK display");
        let application = gtk::Application::new(
            Some("dev.zephium.launcher-placement-test"),
            gtk::gio::ApplicationFlags::NON_UNIQUE,
        );
        application
            .register(None::<&gtk::gio::Cancellable>)
            .expect("register isolated test application");
        let owner = gtk::ApplicationWindow::new(&application);
        let panel = gtk::ApplicationWindow::new(&application);
        owner.set_default_size(800, 600);
        panel.set_default_size(400, 100);
        let expected = match std::env::var("GDK_BACKEND").as_deref() {
            Ok("x11") => DisplayBackend::X11,
            Ok("wayland") => DisplayBackend::Wayland,
            _ => panic!("test requires an explicit GDK_BACKEND=x11 or wayland"),
        };
        assert_eq!(display_backend(&panel), expected);
        let notifications = Rc::new(Cell::new(0));
        let observed = notifications.clone();
        panel.connect_transient_for_notify(move |_| observed.set(observed.get() + 1));

        owner.show();
        wait_for(|| owner.is_mapped());
        assert_eq!(
            prepare(&panel, Some(&owner)),
            expected == DisplayBackend::X11
        );
        assert_eq!(panel.transient_for().as_ref(), Some(owner.upcast_ref()));
        let attached = notifications.get();
        prepare(&panel, Some(&owner));
        assert_eq!(notifications.get(), attached);
        if expected == DisplayBackend::Wayland {
            assert_eq!(panel.window_position(), gtk::WindowPosition::CenterOnParent);
        }
        panel.show();
        wait_for(|| panel.is_mapped());

        panel.hide();
        owner.hide();
        wait_for(|| !owner.is_mapped() && !panel.is_mapped());
        prepare(&panel, Some(&owner));
        assert!(panel.transient_for().is_none());
        panel.show();
        wait_for(|| panel.is_mapped());
        assert!(!owner.is_visible());
        assert!(!owner.is_mapped());

        panel.hide();
        owner.show();
        wait_for(|| owner.is_mapped());
        prepare(&panel, Some(&owner));
        assert_eq!(panel.transient_for().as_ref(), Some(owner.upcast_ref()));
        panel.show();
        wait_for(|| panel.is_mapped());

        // SAFETY: test-owned windows are no longer used after destruction.
        unsafe {
            panel.destroy();
            owner.destroy();
        }
    }
}
