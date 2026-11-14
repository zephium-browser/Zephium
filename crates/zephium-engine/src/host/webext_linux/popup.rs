//! The action popup: a GTK popover in the browser window hosting the
//! extension's popup page, sized to its content within the shared bounds and
//! dismissed by Escape, an outside click, focus loss and tab switches.

use std::cell::Cell;
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

thread_local! {
    static SERIAL: Cell<u64> = const { Cell::new(0) };
}

pub(super) struct Popup {
    profile: ProfileId,
    install: ExtensionInstallId,
    tab: ItemId,
    serial: u64,
    popover: gtk::Popover,
    page: Option<ExtensionPage>,
    measuring: Option<glib::SourceId>,
    focus: Option<(gtk::Window, glib::SignalHandlerId)>,
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
        let popover = gtk::Popover::new(Some(&container));
        popover.set_pointing_to(&gtk::gdk::Rectangle::new(
            anchor.x.round() as i32,
            anchor.y.round() as i32,
            anchor.width.max(1.0).round() as i32,
            anchor.height.max(1.0).round() as i32,
        ));
        popover.set_position(gtk::PositionType::Bottom);
        popover.set_constrain_to(gtk::PopoverConstraint::Window);
        popover.set_modal(true);
        let view = page.view().clone();
        view.set_size_request(INITIAL_WIDTH, INITIAL_HEIGHT);
        // Chrome closes a popup on Escape before the page sees the key.
        view.connect_key_press_event(move |_, event| {
            if event.keyval() == gtk::gdk::keys::constants::Escape {
                super::super::dispatch::best_effort_with(move |host| {
                    host.webext_linux.popup_closed(serial);
                });
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        popover.add(&view);
        popover.connect_closed(move |_| {
            super::super::dispatch::best_effort_with(move |host| {
                host.webext_linux.popup_closed(serial);
            });
        });
        let focus = container
            .toplevel()
            .and_then(|toplevel| toplevel.downcast::<gtk::Window>().ok())
            .map(|window| {
                let handler = window.connect_is_active_notify(move |window| {
                    if !window.is_active() {
                        super::super::dispatch::best_effort_with(move |host| {
                            host.webext_linux.popup_closed(serial);
                        });
                    }
                });
                (window, handler)
            });
        let measured = Rc::new(Cell::new((0, 0)));
        let weak = view.downgrade();
        let measuring = glib::timeout_add_local(MEASURE_INTERVAL, move || {
            let Some(view) = weak.upgrade() else {
                return glib::ControlFlow::Break;
            };
            let measured = measured.clone();
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
                },
            );
            glib::ControlFlow::Continue
        });
        view.show();
        popover.popup();
        view.grab_focus();
        Ok(Self {
            profile,
            install,
            tab,
            serial,
            popover,
            page: Some(page),
            measuring: Some(measuring),
            focus,
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
        if let Some((window, handler)) = self.focus.take() {
            window.disconnect(handler);
        }
        self.popover.popdown();
        if let Some(page) = self.page.take() {
            self.popover.remove(page.view());
            drop(page);
        }
        // SAFETY: the popover is owned here and holds no other references.
        unsafe { self.popover.destroy() };
    }
}

impl Drop for Popup {
    fn drop(&mut self) {
        if let Some(source) = self.measuring.take() {
            source.remove();
        }
        if let Some((window, handler)) = self.focus.take() {
            window.disconnect(handler);
        }
    }
}
