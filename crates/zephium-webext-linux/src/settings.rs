//! Native WebKit features for runtime-owned extension views.

use std::cell::Cell;
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
    fn webkit_settings_get_feature_enabled(
        settings: *mut webkit2gtk::ffi::WebKitSettings,
        feature: *mut c_void,
    ) -> c_int;
    fn webkit_settings_set_feature_enabled(
        settings: *mut webkit2gtk::ffi::WebKitSettings,
        feature: *mut c_void,
        enabled: c_int,
    );
}

pub(crate) fn configure_owned(view: &webkit2gtk::WebView) {
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
                && matches!(
                    CStr::from_ptr(identifier).to_bytes(),
                    b"RequestIdleCallback" | b"StorageAPI" | b"StorageAPIEstimate"
                )
            {
                webkit_settings_set_feature_enabled(settings.to_glib_none().0, feature, 1);
            }
        }
        webkit_feature_list_unref(features);
    }
}

pub(crate) fn storage_for_navigation(
    view: &webkit2gtk::WebView,
    extension: bool,
    previous: &Cell<Option<[bool; 2]>>,
) {
    if !extension && previous.get().is_none() {
        return;
    }
    let Some(settings) = view.settings() else {
        return;
    };
    // Settings and feature-list ownership stay on the GTK thread.
    unsafe {
        let features = webkit_settings_get_all_features();
        if features.is_null() {
            return;
        }
        let mut saved = previous.get().unwrap_or([false; 2]);
        for index in 0..webkit_feature_list_get_length(features).min(4096) {
            let feature = webkit_feature_list_get(features, index);
            if feature.is_null() {
                continue;
            }
            let identifier = webkit_feature_get_identifier(feature);
            if identifier.is_null() {
                continue;
            }
            let slot = match CStr::from_ptr(identifier).to_bytes() {
                b"StorageAPI" => 0,
                b"StorageAPIEstimate" => 1,
                _ => continue,
            };
            if extension && previous.get().is_none() {
                saved[slot] =
                    webkit_settings_get_feature_enabled(settings.to_glib_none().0, feature) != 0;
            }
            webkit_settings_set_feature_enabled(
                settings.to_glib_none().0,
                feature,
                i32::from(extension || saved[slot]),
            );
        }
        webkit_feature_list_unref(features);
        previous.set(extension.then_some(saved));
    }
}

pub(crate) fn link_prefetch_enabled(settings: &webkit2gtk::Settings) -> Option<bool> {
    // The feature borrows its list; all access stays on the GTK thread.
    unsafe {
        let list = webkit_settings_get_all_features();
        if list.is_null() {
            return None;
        }
        let count = webkit_feature_list_get_length(list);
        let mut result = None;
        if count <= 4096 {
            for index in 0..count {
                let feature = webkit_feature_list_get(list, index);
                if feature.is_null() {
                    continue;
                }
                let identifier = webkit_feature_get_identifier(feature);
                if !identifier.is_null() && CStr::from_ptr(identifier).to_bytes() == b"LinkPrefetch"
                {
                    result = Some(
                        webkit_settings_get_feature_enabled(settings.to_glib_none().0, feature)
                            != 0,
                    );
                    break;
                }
            }
        }
        webkit_feature_list_unref(list);
        result
    }
}
