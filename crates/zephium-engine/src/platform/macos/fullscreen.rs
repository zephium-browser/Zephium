//! WebKit element fullscreen for ordinary tabs.
//!
//! On entry WebKit leaves a placeholder in the view's slot and moves the
//! WKWebView into a fullscreen window of its own; on exit it swaps the view
//! back. While that is under way the view belongs to WebKit, so the stage
//! asks [`webkit_owns`] before every frame, visibility or hierarchy change.
use std::cell::Cell;
use std::ffi::c_void;
use std::ptr::null_mut;

use objc2::rc::{Retained, Weak};
use objc2::runtime::{AnyObject, NSObject};
use objc2::{define_class, msg_send, sel, DefinedClass, MainThreadOnly};
use objc2_app_kit::NSView;
use objc2_foundation::{
    ns_string, MainThreadMarker, NSDictionary, NSError, NSKeyValueChangeKey,
    NSKeyValueObservingOptions, NSObjectNSKeyValueObserverRegistration, NSObjectProtocol, NSString,
};
use objc2_web_kit::{WKContentWorld, WKWebView};

use crate::fullscreen::NativeFullscreen;

const STATE_KEY: &str = "fullscreenState";

// It runs in the client world, so a page cannot replace these functions to
// stay fullscreen. Video presentation state is left alone: in WebKit a video
// in picture in picture also reports itself as displaying fullscreen.
const EXIT_SCRIPT: &str = "if (document.fullscreenElement) { await document.exitFullscreen(); } \
     else if (document.webkitFullscreenElement) { document.webkitExitFullscreen(); }";

fn raw_state(view: &AnyObject) -> Option<isize> {
    // SAFETY: a main-thread respondsToSelector: probe on a live object.
    let answers: bool = unsafe { msg_send![view, respondsToSelector: sel!(fullscreenState)] };
    // SAFETY: the selector exists and returns WKFullscreenState (NSInteger).
    answers.then(|| unsafe { msg_send![view, fullscreenState] })
}

/// Whether WebKit is moving or presenting `view` in its fullscreen window, or
/// has left it somewhere other than `stage`.
pub(crate) fn webkit_owns(view: &NSView, stage: &NSView) -> bool {
    if raw_state(view).is_some_and(|state| state != 0) {
        return true;
    }
    // SAFETY: retained parent access on the AppKit main thread.
    unsafe { view.superview() }
        .as_deref()
        .is_some_and(|parent| !std::ptr::eq(parent, stage))
}

/// Whether WebKit is moving or presenting `view` in its fullscreen window.
pub(crate) fn in_transition(view: &NSView) -> bool {
    raw_state(view).is_some_and(|state| state != 0)
}

pub(crate) fn state(view: &wry::WebView) -> NativeFullscreen {
    let page = super::native_webview(view);
    raw_state(&page).map_or(NativeFullscreen::Inactive, NativeFullscreen::from_webkit)
}

/// Asks the page to leave fullscreen. Picture in picture is a separate
/// presentation and keeps playing.
pub(crate) fn exit(view: &wry::WebView) {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let page = super::native_webview(view);
    if raw_state(&page).is_none_or(|state| state == 0) {
        return;
    }
    let done = block2::RcBlock::new(|_: *mut AnyObject, _: *mut NSError| {});
    // SAFETY: main-thread WebKit call with a retained view and world; WebKit
    // copies the completion block.
    unsafe {
        let world = WKContentWorld::defaultClientWorld(mtm);
        page.callAsyncJavaScript_arguments_inFrame_inContentWorld_completionHandler(
            &NSString::from_str(EXIT_SCRIPT),
            None,
            None,
            &world,
            Some(&done),
        );
    }
}

/// Ends every out-of-window presentation, picture in picture included, and
/// calls `done` once WebKit has handed the view back. Used only for a view
/// that is about to be destroyed.
pub(crate) fn close_presentations(view: &wry::WebView, done: impl FnOnce() + 'static) {
    let page = super::native_webview(view);
    let returned = Weak::from_retained(&page);
    let done = Cell::new(Some(done));
    let completion = block2::RcBlock::new(move || {
        // WebKit has put the page back in its old slot, which the stage no
        // longer tracks; keep it from painting over the next tab meanwhile.
        if let Some(page) = returned.load() {
            if !in_transition(&page) {
                page.setHidden(true);
            }
        }
        if let Some(done) = done.take() {
            done();
        }
    });
    // SAFETY: main-thread WebKit call; WebKit copies the completion block.
    unsafe { page.closeAllMediaPresentationsWithCompletionHandler(Some(&completion)) };
}

pub struct FullscreenObserverIvars {
    view: Weak<WKWebView>,
    changed: Box<dyn Fn()>,
    // The page's corner radius and hairline, taken off while WebKit shows the
    // view edge to edge and put back once it is home again.
    edge: Cell<Option<(f64, f64)>>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "ZephiumFullscreenObserver"]
    #[ivars = FullscreenObserverIvars]
    pub struct FullscreenObserver;
    impl FullscreenObserver {
        #[unsafe(method(observeValueForKeyPath:ofObject:change:context:))]
        fn observed(&self, key: Option<&NSString>, _object: Option<&AnyObject>,
            _change: Option<&NSDictionary<NSKeyValueChangeKey, AnyObject>>, _context: *mut c_void) {
            if !key.is_some_and(|key| key.isEqualToString(ns_string!(STATE_KEY))) { return; }
            let Some(view) = self.ivars().view.load() else { return };
            let state = raw_state(&view).map_or(NativeFullscreen::Inactive, NativeFullscreen::from_webkit);
            self.update_edge(&view, state);
            (self.ivars().changed)();
        }
    }
    unsafe impl NSObjectProtocol for FullscreenObserver {}
);

impl FullscreenObserver {
    pub(crate) fn install(view: &wry::WebView, changed: impl Fn() + 'static) -> Retained<Self> {
        let view = super::native_webview(view);
        let observer = Self::alloc(MainThreadMarker::new().expect("native fullscreen main thread"))
            .set_ivars(FullscreenObserverIvars {
                view: Weak::from_retained(&view),
                changed: Box::new(changed),
                edge: Cell::new(None),
            });
        let observer: Retained<Self> = unsafe { msg_send![super(observer), init] };
        // `fullscreenState` is documented KVO-compliant. The host owns this
        // registration before the WebView and removes it before teardown.
        unsafe {
            view.addObserver_forKeyPath_options_context(
                &observer,
                ns_string!(STATE_KEY),
                NSKeyValueObservingOptions::New,
                null_mut(),
            );
        }
        observer
    }

    fn update_edge(&self, view: &WKWebView, state: NativeFullscreen) {
        let Some(layer) = view.layer() else {
            return;
        };
        match state {
            NativeFullscreen::Entering | NativeFullscreen::Active => {
                if self.ivars().edge.get().is_none() {
                    self.ivars()
                        .edge
                        .set(Some((layer.cornerRadius(), layer.borderWidth())));
                    layer.setCornerRadius(0.0);
                    layer.setBorderWidth(0.0);
                }
            }
            NativeFullscreen::Inactive => {
                if let Some((radius, border)) = self.ivars().edge.take() {
                    layer.setCornerRadius(radius);
                    layer.setBorderWidth(border);
                }
            }
            NativeFullscreen::Exiting => {}
        }
    }
}

impl Drop for FullscreenObserver {
    fn drop(&mut self) {
        if let Some(view) = self.ivars().view.load() {
            unsafe { view.removeObserver_forKeyPath(self, ns_string!(STATE_KEY)) };
        }
    }
}

#[cfg(test)]
mod tests {
    use objc2::rc::Retained;
    use objc2::runtime::NSObject;
    use objc2::{define_class, msg_send, AnyThread};

    use super::raw_state;

    define_class!(
        #[unsafe(super(NSObject))]
        #[name = "ZephiumFullscreenStateStub"]
        struct PresentingStub;
        impl PresentingStub {
            #[unsafe(method(fullscreenState))]
            fn fullscreen_state(&self) -> isize { 2 }
        }
    );

    #[test]
    fn webkit_fullscreen_state_is_read_through_its_selector() {
        let presenting: Retained<PresentingStub> =
            unsafe { msg_send![PresentingStub::alloc(), init] };
        assert_eq!(raw_state(&presenting), Some(2));
        assert_eq!(raw_state(&NSObject::new()), None);
    }
}
