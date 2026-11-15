//! Native audio notifications and the union of person/focus mute intent.
use std::{cell::Cell, rc::Rc};

use gtk::glib::{prelude::ObjectExt, SignalHandlerId};
use webkit2gtk::WebViewExt;
use wry::WebViewExtUnix;

#[derive(Default)]
struct MuteIntent {
    person: Cell<bool>,
    focus: Cell<bool>,
}

impl MuteIntent {
    fn apply(&self, page: &webkit2gtk::WebView) {
        let wanted = self.person.get() || self.focus.get();
        if page.is_muted() != wanted {
            page.set_is_muted(wanted);
        }
    }
}

pub(crate) struct AudioObserver {
    page: webkit2gtk::WebView,
    intent: Rc<MuteIntent>,
    signals: Vec<SignalHandlerId>,
}

impl AudioObserver {
    pub(crate) fn set_muted(&self, muted: bool) {
        self.intent.person.set(muted);
        self.intent.apply(&self.page);
    }

    pub(crate) fn set_focus_muted(&self, muted: bool) {
        self.intent.focus.set(muted);
        self.intent.apply(&self.page);
    }
}

impl Drop for AudioObserver {
    fn drop(&mut self) {
        for signal in self.signals.drain(..) {
            self.page.disconnect(signal);
        }
    }
}

pub(crate) fn install_audio_observer(
    view: &wry::WebView,
    changed: impl Fn(bool) + 'static,
) -> Option<AudioObserver> {
    let page = view.webview();
    let intent = Rc::new(MuteIntent::default());
    let audio = page.connect_is_playing_audio_notify(move |page| changed(page.is_playing_audio()));
    let wanted = intent.clone();
    let mute = page.connect_is_muted_notify(move |page| wanted.apply(page));
    Some(AudioObserver {
        page,
        intent,
        signals: vec![audio, mute],
    })
}
