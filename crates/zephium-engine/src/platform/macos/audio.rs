//! Sound and mute for one page. WebKit says when a page starts or stops
//! making sound through key-value observation of `_isPlayingAudio`, the SPI
//! Safari's tab speaker uses, and mutes it with `_setPageMuted:`. Both are
//! used only when present; without them a page shows no speaker, rather than
//! being polled.
use objc2::rc::{Retained, Weak};
use objc2::runtime::{AnyObject, NSObject};
use objc2::{define_class, msg_send, sel, ClassType, DefinedClass, MainThreadOnly};
use objc2_foundation::{
    ns_string, MainThreadMarker, NSDictionary, NSKeyValueChangeKey, NSKeyValueObservingOptions,
    NSObjectNSKeyValueObserverRegistration, NSObjectProtocol, NSString,
};
use objc2_web_kit::WKWebView;
use std::ffi::c_void;
use std::ptr::null_mut;

/// `_WKMediaAudioMuted` in WebKit's `_WKMediaMutedState`.
const AUDIO_MUTED: usize = 1;

pub struct AudioObserverIvars {
    view: Weak<WKWebView>,
    changed: Box<dyn Fn(bool)>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "ZephiumAudioObserver"]
    #[ivars = AudioObserverIvars]
    pub struct AudioObserver;
    impl AudioObserver {
        #[unsafe(method(observeValueForKeyPath:ofObject:change:context:))]
        fn observed(&self, key: Option<&NSString>, _object: Option<&AnyObject>,
            _change: Option<&NSDictionary<NSKeyValueChangeKey, AnyObject>>, _context: *mut c_void) {
            if !key.is_some_and(|key| key.isEqualToString(ns_string!("_isPlayingAudio"))) {
                return;
            }
            if let Some(view) = self.ivars().view.load() {
                // SAFETY: main-thread WebKit access; installed only when present.
                let audible: bool = unsafe { msg_send![&*view, _isPlayingAudio] };
                (self.ivars().changed)(audible);
            }
        }
    }
    unsafe impl NSObjectProtocol for AudioObserver {}
);

impl AudioObserver {
    /// `None` when this WebKit has no `_isPlayingAudio`.
    pub(crate) fn install(
        view: &wry::WebView,
        changed: impl Fn(bool) + 'static,
    ) -> Option<Retained<Self>> {
        if !WKWebView::class().responds_to(sel!(_isPlayingAudio)) {
            return None;
        }
        let view = super::native_webview(view);
        let observer = Self::alloc(MainThreadMarker::new().expect("native audio main thread"))
            .set_ivars(AudioObserverIvars {
                view: Weak::from_retained(&view),
                changed: Box::new(changed),
            });
        let observer: Retained<Self> = unsafe { msg_send![super(observer), init] };
        // WebKit posts will/didChangeValueForKey for this key around every
        // change. The host owns the registration and removes it before
        // teardown.
        unsafe {
            view.addObserver_forKeyPath_options_context(
                &observer,
                ns_string!("_isPlayingAudio"),
                NSKeyValueObservingOptions::New,
                null_mut(),
            );
        }
        Some(observer)
    }
}

impl Drop for AudioObserver {
    fn drop(&mut self) {
        if let Some(view) = self.ivars().view.load() {
            unsafe { view.removeObserver_forKeyPath(self, ns_string!("_isPlayingAudio")) };
        }
    }
}

pub(crate) fn mute_supported() -> bool {
    WKWebView::class().responds_to(sel!(_setPageMuted:))
}

/// Sets only the audio bit, keeping any camera or screen capture mute.
pub(crate) fn set_muted(view: &wry::WebView, muted: bool) -> bool {
    if !mute_supported() {
        return false;
    }
    let view = super::native_webview(view);
    // SAFETY: main-thread WebKit access to SPI checked above; the state is an
    // NSUInteger bit set.
    unsafe {
        let current: Option<usize> = view
            .respondsToSelector(sel!(_mediaMutedState))
            .then(|| msg_send![&*view, _mediaMutedState]);
        let base = current.unwrap_or(0);
        let next = if muted {
            base | AUDIO_MUTED
        } else {
            base & !AUDIO_MUTED
        };
        if current != Some(next) {
            let _: () = msg_send![&*view, _setPageMuted: next];
        }
    }
    true
}
