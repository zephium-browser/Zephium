//! The action popup: a GTK popup window under the toolbar button hosting the
//! extension's popup page, sized to its content within the shared bounds and
//! dismissed by Escape, a click outside it, focus loss and tab switches.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use gtk::glib;
use gtk::glib::prelude::*;
use gtk::prelude::*;
use javascriptcore::ValueExt;
use webkit2gtk::WebViewExt;
use zephium_core::extensions::{
    MAX_EXTENSION_POPUP_HEIGHT, MAX_EXTENSION_POPUP_WIDTH, MIN_EXTENSION_POPUP_HEIGHT,
    MIN_EXTENSION_POPUP_WIDTH,
};
use zephium_core::geometry::Rect;
use zephium_core::ids::{ExtensionInstallId, ItemId, ProfileId};
use zephium_webext_linux::views::ExtensionPage;

/// Runs in a private world: page script cannot see or replace it.
const MEASURE: &str = "const r=document.documentElement,b=document.body;if(!r||!b)return 0;\
const s=getComputedStyle(b),x=(parseFloat(s.marginLeft)||0)+(parseFloat(s.marginRight)||0),\
y=(parseFloat(s.marginTop)||0)+(parseFloat(s.marginBottom)||0),q=b.getBoundingClientRect();\
const w=Math.ceil(Math.min(800,Math.max(1,b.scrollWidth,q.width)+x)),\
h=Math.ceil(Math.min(600,Math.max(1,b.scrollHeight,q.height)+y));return w*10000+h;";
const MEASURE_WORLD: &str = "zephium-popup-measure";
const MEASURE_INTERVAL: Duration = Duration::from_millis(200);
const INITIAL_WIDTH: i32 = 320;
const INITIAL_HEIGHT: i32 = 120;
/// A page that never finishes loading still opens, sized as measured so far.
const SHOW_DEADLINE: Duration = Duration::from_millis(1500);
/// The shell's floating-sheet radius (`--radius-panel`).
const RADIUS: f64 = 22.0;
/// Gap between the toolbar button and the popup.
const OFFSET: i32 = 6;

thread_local! {
    static SERIAL: Cell<u64> = const { Cell::new(0) };
}

pub(super) struct Popup {
    profile: ProfileId,
    install: ExtensionInstallId,
    tab: ItemId,
    serial: u64,
    window: gtk::Window,
    frame: gtk::Box,
    seat: Rc<RefCell<Option<gdk::Seat>>>,
    focus: Option<(gtk::Window, glib::SignalHandlerId)>,
    page: Option<ExtensionPage>,
    measuring: Option<glib::SourceId>,
}

fn clamp(value: f64, min: f64, max: f64) -> i32 {
    value.clamp(min, max).round() as i32
}

impl Popup {
    pub(super) fn show(
        profile: ProfileId,
        install: ExtensionInstallId,
        tab: ItemId,
        page: ExtensionPage,
        anchor: Rect,
    ) -> Result<Self, String> {
        let container = crate::platform::imp::container().ok_or("the browser window is gone")?;
        let serial = SERIAL.with(|serial| {
            serial.set(serial.get() + 1);
            serial.get()
        });
        let toplevel = container
            .toplevel()
            .and_then(|toplevel| toplevel.downcast::<gtk::Window>().ok())
            .ok_or("the browser window is gone")?;
        let (x, y) = container
            .translate_coordinates(&toplevel, anchor.x.round() as i32, anchor.y.round() as i32)
            .ok_or("the toolbar button is not in the browser window")?;
        let anchor = gdk::Rectangle::new(
            x,
            y,
            anchor.width.max(1.0).round() as i32,
            anchor.height.max(1.0).round() as i32,
        );
        // A popup window under a seat grab is what GTK menus use: on Wayland
        // it maps as an xdg_popup, takes its own input and is dismissed by
        // the compositor on an outside click. A popover in the browser window
        // receives no input over the native tab views.
        let window = gtk::Window::new(gtk::WindowType::Popup);
        window.set_type_hint(gdk::WindowTypeHint::PopupMenu);
        window.set_transient_for(Some(&toplevel));
        window.set_attached_to(Some(&container));
        window.set_app_paintable(true);
        if let Some(visual) = WidgetExt::screen(&window).and_then(|screen| screen.rgba_visual()) {
            window.set_visual(Some(&visual));
        }
        window.connect_draw(|window, cr| {
            // Transparent outside the rounded page.
            cr.set_operator(gtk::cairo::Operator::Source);
            cr.set_source_rgba(0.0, 0.0, 0.0, 0.0);
            let _ = cr.paint();
            cr.set_operator(gtk::cairo::Operator::Over);
            let _ = window;
            glib::Propagation::Proceed
        });
        let view = page.view().clone();
        view.set_size_request(INITIAL_WIDTH, INITIAL_HEIGHT);
        // Chrome closes a popup on Escape before the page sees the key.
        view.connect_key_press_event(move |_, event| {
            if event.keyval() == gdk::keys::constants::Escape {
                super::super::dispatch::best_effort_with(move |host| {
                    host.webext_linux.popup_closed(serial);
                });
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        // The page draws square; clip it to rounded corners with a hairline.
        let frame = gtk::Box::new(gtk::Orientation::Vertical, 0);
        frame.add(&view);
        frame.connect_draw(|frame, cr| {
            let (width, height) = (
                f64::from(frame.allocated_width()),
                f64::from(frame.allocated_height()),
            );
            let radius = RADIUS.min(width / 2.0).min(height / 2.0) - 1.0;
            let degrees = std::f64::consts::PI / 180.0;
            cr.new_sub_path();
            cr.arc(width - radius, radius, radius, -90.0 * degrees, 0.0);
            cr.arc(width - radius, height - radius, radius, 0.0, 90.0 * degrees);
            cr.arc(
                radius,
                height - radius,
                radius,
                90.0 * degrees,
                180.0 * degrees,
            );
            cr.arc(radius, radius, radius, 180.0 * degrees, 270.0 * degrees);
            cr.close_path();
            let outline = cr.copy_path();
            cr.clip();
            for child in frame.children() {
                frame.propagate_draw(&child, cr);
            }
            cr.reset_clip();
            if let Ok(outline) = outline {
                cr.new_path();
                cr.append_path(&outline);
                cr.set_line_width(1.0);
                cr.set_source_rgba(0.0, 0.0, 0.0, 0.16);
                let _ = cr.stroke();
            }
            glib::Propagation::Stop
        });
        window.add(&frame);
        // Under the grab, presses outside the popup arrive here, as for GTK menus.
        window.add_events(gdk::EventMask::BUTTON_PRESS_MASK);
        window.connect_button_press_event(move |window, event| {
            let (x, y) = event.position();
            let (width, height) = (
                f64::from(window.allocated_width()),
                f64::from(window.allocated_height()),
            );
            if x < 0.0 || y < 0.0 || x >= width || y >= height {
                super::super::dispatch::best_effort_with(move |host| {
                    host.webext_linux.popup_closed(serial);
                });
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        // The compositor ends the grab on an outside click or focus loss.
        window.connect_grab_broken_event(move |_, _| {
            super::super::dispatch::best_effort_with(move |host| {
                host.webext_linux.popup_closed(serial);
            });
            glib::Propagation::Proceed
        });
        let seat: Rc<RefCell<Option<gdk::Seat>>> = Rc::default();
        // A click into the browser window moves its focus: close then too,
        // whether or not the compositor granted the grab.
        let shown = Rc::new(Cell::new(false));
        let focus = toplevel.connect_set_focus({
            let shown = shown.clone();
            move |_, _| {
                if shown.get() {
                    super::super::dispatch::best_effort_with(move |host| {
                        host.webext_linux.popup_closed(serial);
                    });
                }
            }
        });
        // Chrome shows a popup once its page has loaded at its own size; it
        // stays hidden until the first measurement after load.
        let loaded = Rc::new(Cell::new(false));
        let reveal = {
            let (shown, window, view, seat) = (
                shown.clone(),
                window.downgrade(),
                view.downgrade(),
                seat.clone(),
            );
            Rc::new(move || {
                if shown.replace(true) {
                    return;
                }
                let (Some(window), Some(view)) = (window.upgrade(), view.upgrade()) else {
                    return;
                };
                window.realize();
                let Some(surface) = window.window() else {
                    return;
                };
                surface.move_to_rect(
                    &anchor,
                    gdk::Gravity::SouthWest,
                    gdk::Gravity::NorthWest,
                    gdk::AnchorHints::FLIP_Y
                        | gdk::AnchorHints::SLIDE_X
                        | gdk::AnchorHints::SLIDE_Y,
                    0,
                    OFFSET,
                );
                let grabbing = WidgetExt::display(&window).default_seat();
                let granted = grabbing.as_ref().is_some_and(|grabbing| {
                    let shown = window.clone();
                    grabbing.grab(
                        &surface,
                        gdk::SeatCapabilities::ALL,
                        true,
                        None,
                        None,
                        Some(&mut move |_, _| shown.show()),
                    ) == gdk::GrabStatus::Success
                });
                if granted {
                    *seat.borrow_mut() = grabbing;
                } else {
                    eprintln!("extensions: popup input grab refused; focus closes it");
                    window.show();
                }
                view.grab_focus();
            })
        };
        let measured = Rc::new(Cell::new((0, 0)));
        let measure: Rc<dyn Fn(&webkit2gtk::WebView)> = {
            let (measured, loaded, reveal) = (measured.clone(), loaded.clone(), reveal.clone());
            Rc::new(move |view: &webkit2gtk::WebView| {
                let (measured, loaded, reveal) = (measured.clone(), loaded.clone(), reveal.clone());
                let target = view.downgrade();
                view.call_async_javascript_function(
                    MEASURE,
                    None,
                    Some(MEASURE_WORLD),
                    None,
                    None::<&gtk::gio::Cancellable>,
                    move |result| {
                        let Some(view) = target.upgrade() else {
                            return;
                        };
                        let Ok(value) = result else {
                            return;
                        };
                        if !value.is_number() {
                            return;
                        }
                        let packed = value.to_double();
                        if !packed.is_finite() || packed <= 0.0 {
                            return;
                        }
                        let packed = packed as u64;
                        let size = (
                            clamp(
                                (packed / 10_000) as f64,
                                MIN_EXTENSION_POPUP_WIDTH,
                                MAX_EXTENSION_POPUP_WIDTH,
                            ),
                            clamp(
                                (packed % 10_000) as f64,
                                MIN_EXTENSION_POPUP_HEIGHT,
                                MAX_EXTENSION_POPUP_HEIGHT,
                            ),
                        );
                        if measured.replace(size) != size {
                            view.set_size_request(size.0, size.1);
                        }
                        if loaded.get() {
                            reveal();
                        }
                    },
                );
            })
        };
        {
            let (loaded, measure) = (loaded.clone(), measure.clone());
            view.connect_load_changed(move |view, event| {
                if event == webkit2gtk::LoadEvent::Finished {
                    loaded.set(true);
                    measure(view);
                }
            });
        }
        let weak = view.downgrade();
        let measuring = glib::timeout_add_local(MEASURE_INTERVAL, move || {
            let Some(view) = weak.upgrade() else {
                return glib::ControlFlow::Break;
            };
            measure(&view);
            glib::ControlFlow::Continue
        });
        glib::timeout_add_local_once(SHOW_DEADLINE, move || reveal());
        view.show();
        frame.show();
        Ok(Self {
            profile,
            install,
            tab,
            serial,
            window,
            frame,
            seat,
            focus: Some((toplevel, focus)),
            page: Some(page),
            measuring: Some(measuring),
        })
    }

    pub(super) fn profile(&self) -> ProfileId {
        self.profile
    }

    pub(super) fn install(&self) -> ExtensionInstallId {
        self.install
    }

    pub(super) fn tab(&self) -> ItemId {
        self.tab
    }

    pub(super) fn serial(&self) -> u64 {
        self.serial
    }

    pub(super) fn close(mut self) {
        if let Some(source) = self.measuring.take() {
            source.remove();
        }
        if let Some(seat) = self.seat.borrow_mut().take() {
            seat.ungrab();
        }
        if let Some((toplevel, handler)) = self.focus.take() {
            toplevel.disconnect(handler);
        }
        self.window.hide();
        if let Some(page) = self.page.take() {
            self.frame.remove(page.view());
            drop(page);
        }
        // SAFETY: the popup window is owned here and holds no other references.
        unsafe { self.window.destroy() };
    }
}

impl Drop for Popup {
    fn drop(&mut self) {
        if let Some(source) = self.measuring.take() {
            source.remove();
        }
        if let Some(seat) = self.seat.borrow_mut().take() {
            seat.ungrab();
        }
        if let Some((toplevel, handler)) = self.focus.take() {
            toplevel.disconnect(handler);
        }
    }
}
