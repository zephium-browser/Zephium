//! Native video PiP in a dedicated content world, scoped to an ordinary tab.
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_foundation::{MainThreadMarker, NSError, NSNumber, NSString};
use objc2_web_kit::WKContentWorld;
use std::cell::{Cell, OnceCell};
use std::rc::Rc;

/// The world the program keeps its state in between calls, among it the
/// video it put in picture in picture and has to bring back. WebKit discards
/// a named world, and every global in it, once nothing holds it.
fn world(mtm: MainThreadMarker) -> Retained<WKContentWorld> {
    thread_local! {
        static WORLD: OnceCell<Retained<WKContentWorld>> = const { OnceCell::new() };
    }
    WORLD.with(|world| {
        world
            // SAFETY: `mtm` proves main-thread creation and the name is a live
            // retained NSString.
            .get_or_init(|| unsafe {
                WKContentWorld::worldWithName(
                    &NSString::from_str("zephium-automatic-video-pip"),
                    mtm,
                )
            })
            .clone()
    })
}

// Answers whether the document has left element fullscreen. Only a change
// that leaves fullscreen ends the wait early; the timeout answers too, with
// whatever the document says then.
const DOCUMENT_LEFT_FULLSCREEN: &str =
    "const fullscreen = () => !!(document.fullscreenElement || document.webkitFullscreenElement); \
     if (fullscreen()) await new Promise(resolve => { \
       const done = () => { \
         document.removeEventListener('fullscreenchange', changed); \
         document.removeEventListener('webkitfullscreenchange', changed); \
         clearTimeout(timer); \
         resolve(); \
       }; \
       const changed = () => { if (!fullscreen()) done(); }; \
       const timer = setTimeout(done, 1000); \
       document.addEventListener('fullscreenchange', changed); \
       document.addEventListener('webkitfullscreenchange', changed); \
     }); \
     return !fullscreen();";

pub(crate) fn automatic_pip(view: &wry::WebView, script: String, current: Rc<dyn Fn() -> bool>) {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    if !current() {
        return;
    }
    let page = super::native_webview(view);
    let done = block2::RcBlock::new(|_: *mut AnyObject, _: *mut NSError| {});
    unsafe {
        let world = world(mtm);
        page.evaluateJavaScript_inFrame_inContentWorld_completionHandler(
            &NSString::from_str(&script),
            None,
            &world,
            Some(&done),
        );
    }
}

/// Calls `done` once the page's document has left element fullscreen. A page
/// still fullscreen after a second, or one the probe could not ask, is left
/// alone: it would decline the video anyway.
pub(crate) fn after_document_fullscreen(view: &wry::WebView, done: impl FnOnce() + 'static) {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let page = super::native_webview(view);
    let done = Cell::new(Some(done));
    let completion = block2::RcBlock::new(move |result: *mut AnyObject, error: *mut NSError| {
        // SAFETY: WebKit passes either null or a live object for the duration
        // of the completion.
        let (result, error) = unsafe { (result.as_ref(), error.as_ref()) };
        if document_left(result, error) {
            if let Some(done) = done.take() {
                done();
            }
        }
    });
    let world = world(mtm);
    // SAFETY: main-thread WebKit call with a retained view and world; WebKit
    // copies the completion block.
    unsafe {
        page.callAsyncJavaScript_arguments_inFrame_inContentWorld_completionHandler(
            &NSString::from_str(DOCUMENT_LEFT_FULLSCREEN),
            None,
            None,
            &world,
            Some(&completion),
        );
    }
}

/// Whether the probe saw the document out of fullscreen. A failed script or
/// any answer other than `true` counts as still fullscreen.
fn document_left(result: Option<&AnyObject>, error: Option<&NSError>) -> bool {
    error.is_none()
        && result
            .and_then(|result| result.downcast_ref::<NSNumber>())
            .is_some_and(|left| left.as_bool())
}

#[cfg(test)]
mod tests {
    use super::*;
    use objc2_foundation::ns_string;

    #[test]
    fn only_a_document_out_of_fullscreen_lets_the_video_be_offered() {
        let left = NSNumber::new_bool(true);
        let still_fullscreen = NSNumber::new_bool(false);
        assert!(document_left(Some(&left), None));
        assert!(!document_left(Some(&still_fullscreen), None));
        assert!(!document_left(None, None));
        assert!(!document_left(Some(ns_string!("true")), None));
        let failed = NSError::new(1, ns_string!("WKErrorDomain"));
        assert!(!document_left(Some(&left), Some(&failed)));
    }
}
