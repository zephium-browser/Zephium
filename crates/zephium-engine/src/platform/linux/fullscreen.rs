//! Native element-fullscreen observations; the shared shell owns the host window.
use crate::fullscreen::NativeFullscreen;
use gtk::prelude::*;
use std::cell::Cell;
use std::rc::Rc;
use webkit2gtk::{SettingsExt, WebViewExt};
use wry::WebViewExtUnix;

const STATE: &str = "zephium-owned-fullscreen-state";
const EXIT: &str = "if(document.fullscreenElement){document.exitFullscreen()}else if(document.webkitFullscreenElement){document.webkitExitFullscreen()}";

pub(crate) struct FullscreenObserver {
    page: webkit2gtk::WebView,
    signals: Vec<gtk::glib::SignalHandlerId>,
    window: gtk::Window,
    key: Option<gtk::glib::SignalHandlerId>,
}
impl Drop for FullscreenObserver {
    fn drop(&mut self) {
        for signal in self.signals.drain(..) {
            self.page.disconnect(signal);
        }
        if let Some(key) = self.key.take() {
            self.window.disconnect(key);
        }
        // SAFETY: only this module installs or removes this typed private key.
        unsafe {
            self.page.steal_data::<Rc<Cell<bool>>>(STATE);
        }
    }
}

pub(crate) fn install_fullscreen_observer(
    view: &wry::WebView,
    allowed: impl Fn() -> bool + 'static,
    changed: impl Fn() + 'static,
) -> Option<FullscreenObserver> {
    let page = view.webview();
    let window = page.toplevel()?.downcast::<gtk::Window>().ok()?;
    let settings = WebViewExt::settings(&page)?;
    let state = Rc::new(Cell::new(false));
    // SAFETY: private key with a single registration owned by this view.
    unsafe {
        page.set_data(STATE, state.clone());
    }
    let changed = Rc::new(changed);
    let entering = state.clone();
    let notify = changed.clone();
    let enter = page.connect_enter_fullscreen(move |page| {
        if !allowed()
            || !page.is_mapped()
            || !page
                .toplevel()
                .and_then(|widget| widget.downcast::<gtk::Window>().ok())
                .is_some_and(|window| window.is_active())
        {
            exit_page(page);
            return true;
        }
        entering.set(true);
        notify();
        true
    });
    let leaving = state.clone();
    let leave = page.connect_leave_fullscreen(move |_| {
        leaving.set(false);
        changed();
        true
    });
    let weak = page.downgrade();
    let key = window.connect_event(move |_, event| {
        if state.get()
            && event.event_type() == gtk::gdk::EventType::KeyPress
            && event.keyval() == Some(0xff1b)
        {
            if let Some(page) = weak.upgrade() {
                exit_page(&page);
            }
            gtk::glib::Propagation::Stop
        } else {
            gtk::glib::Propagation::Proceed
        }
    });
    settings.set_enable_fullscreen(true);
    Some(FullscreenObserver {
        page,
        signals: vec![enter, leave],
        window,
        key: Some(key),
    })
}

pub(crate) fn fullscreen_state(view: &wry::WebView) -> NativeFullscreen {
    // SAFETY: only this module writes this private key and fixes its type.
    let active = unsafe {
        view.webview()
            .data::<Rc<Cell<bool>>>(STATE)
            .is_some_and(|state| state.as_ref().get())
    };
    NativeFullscreen::from_webkit(if active { 2 } else { 0 })
}
pub(crate) fn exit_fullscreen(view: &wry::WebView) {
    exit_page(&view.webview());
}
fn exit_page(page: &webkit2gtk::WebView) {
    page.evaluate_javascript(
        EXIT,
        Some("zephium-fullscreen"),
        None,
        None::<&gtk::gio::Cancellable>,
        |_| {},
    );
}
