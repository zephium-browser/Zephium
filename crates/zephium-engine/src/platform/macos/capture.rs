//! Native-only camera/microphone state for ordinary browser chrome.
use objc2::rc::{Retained, Weak};
use objc2::runtime::{AnyObject, NSObject};
use objc2::{define_class, msg_send, DefinedClass, MainThreadOnly};
use objc2_foundation::{
    ns_string, MainThreadMarker, NSDictionary, NSKeyValueChangeKey, NSKeyValueObservingOptions,
    NSObjectNSKeyValueObserverRegistration, NSObjectProtocol, NSString,
};
use objc2_web_kit::{WKMediaCaptureState, WKWebView};
use std::ffi::c_void;
use std::ptr::null_mut;
use zephium_core::ports::engine::{CaptureDeviceState, MediaCaptureState};

pub struct CaptureObserverIvars {
    view: Weak<WKWebView>,
    changed: Box<dyn Fn(MediaCaptureState)>,
}

fn device(state: WKMediaCaptureState) -> CaptureDeviceState {
    match state {
        WKMediaCaptureState::Active => CaptureDeviceState::Active,
        WKMediaCaptureState::Muted => CaptureDeviceState::Muted,
        _ => CaptureDeviceState::None,
    }
}

pub(crate) fn sample(view: &WKWebView) -> MediaCaptureState {
    unsafe {
        MediaCaptureState {
            camera: device(view.cameraCaptureState()),
            microphone: device(view.microphoneCaptureState()),
        }
    }
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "ZephiumCaptureObserver"]
    #[ivars = CaptureObserverIvars]
    pub struct CaptureObserver;
    impl CaptureObserver {
        #[unsafe(method(observeValueForKeyPath:ofObject:change:context:))]
        fn observed(&self, key: Option<&NSString>, _object: Option<&AnyObject>,
            _change: Option<&NSDictionary<NSKeyValueChangeKey, AnyObject>>, _context: *mut c_void) {
            if !key.is_some_and(|key| key.isEqualToString(ns_string!("cameraCaptureState"))
                || key.isEqualToString(ns_string!("microphoneCaptureState"))) { return; }
            if let Some(view) = self.ivars().view.load() {
                (self.ivars().changed)(sample(&view));
            }
        }
    }
    unsafe impl NSObjectProtocol for CaptureObserver {}
);

impl CaptureObserver {
    pub(crate) fn install(
        view: &wry::WebView,
        changed: impl Fn(MediaCaptureState) + 'static,
    ) -> Retained<Self> {
        let view = super::native_webview(view);
        let observer = Self::alloc(MainThreadMarker::new().expect("native capture main thread"))
            .set_ivars(CaptureObserverIvars {
                view: Weak::from_retained(&view),
                changed: Box::new(changed),
            });
        let observer: Retained<Self> = unsafe { msg_send![super(observer), init] };
        // Both capture properties are documented KVO-compliant. The host owns
        // this registration before the WebView and removes it before teardown.
        unsafe {
            for key in [
                ns_string!("cameraCaptureState"),
                ns_string!("microphoneCaptureState"),
            ] {
                view.addObserver_forKeyPath_options_context(
                    &observer,
                    key,
                    NSKeyValueObservingOptions::New,
                    null_mut(),
                );
            }
        }
        observer
    }
}

impl Drop for CaptureObserver {
    fn drop(&mut self) {
        if let Some(view) = self.ivars().view.load() {
            unsafe {
                for key in [
                    ns_string!("cameraCaptureState"),
                    ns_string!("microphoneCaptureState"),
                ] {
                    view.removeObserver_forKeyPath(self, key);
                }
            }
        }
    }
}

pub(crate) fn stop(view: &wry::WebView) {
    let view = super::native_webview(view);
    // Stopping every frame's tracks is an engine operation. UI state remains
    // unchanged until native KVO reports the actual result; Allow never stands
    // in for recording, and dispatch never stands in for stopped capture.
    unsafe {
        view.setCameraCaptureState_completionHandler(WKMediaCaptureState::None, None);
        view.setMicrophoneCaptureState_completionHandler(WKMediaCaptureState::None, None);
    }
}
