//! Native WebKit features needed by extension documents and content scripts.

use std::ffi::{c_char, c_int, c_void, CStr};

use glib::translate::ToGlibPtr;
use webkit2gtk::WebViewExt;

// These public APIs predate the admitted WebKitGTK 2.54 floor, but are
// absent from the pinned Rust bindings. Feature objects borrow their list.
unsafe extern "C" {
    fn webkit_settings_get_all_features() -> *mut c_void;
    fn webkit_feature_list_get_length(list: *mut c_void) -> usize;
    fn webkit_feature_list_get(list: *mut c_void, index: usize) -> *mut c_void;
    fn webkit_feature_get_identifier(feature: *mut c_void) -> *const c_char;
    fn webkit_feature_list_unref(list: *mut c_void);
    fn webkit_settings_set_feature_enabled(
        settings: *mut webkit2gtk::ffi::WebKitSettings,
        feature: *mut c_void,
        enabled: c_int,
    );
}

pub(crate) fn configure(view: &webkit2gtk::WebView) {
    let Some(settings) = view.settings() else {
        return;
    };
    // SAFETY: Called on the GTK thread with a live settings object. The
    // borrowed feature is used only while its owned list remains alive.
    unsafe {
        let features = webkit_settings_get_all_features();
        if features.is_null() {
            return;
        }
        for index in 0..webkit_feature_list_get_length(features).min(4096) {
            let feature = webkit_feature_list_get(features, index);
            if feature.is_null() {
                continue;
            }
            let identifier = webkit_feature_get_identifier(feature);
            if !identifier.is_null()
                && CStr::from_ptr(identifier).to_bytes() == b"RequestIdleCallback"
            {
                webkit_settings_set_feature_enabled(settings.to_glib_none().0, feature, 1);
                break;
            }
        }
        webkit_feature_list_unref(features);
    }
}
