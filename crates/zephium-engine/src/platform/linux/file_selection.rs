use gtk::glib::translate::ToGlibPtr;
use std::ffi::c_void;
use webkit2gtk::WebViewExt;

unsafe extern "C" {
    fn webkit_settings_get_all_features() -> *mut c_void;
    fn webkit_feature_list_find(list: *mut c_void, name: *const std::ffi::c_char) -> *mut c_void;
    fn webkit_feature_list_unref(list: *mut c_void);
    fn webkit_settings_set_feature_enabled(
        settings: *mut webkit2gtk::ffi::WebKitSettings,
        feature: *mut c_void,
        enabled: i32,
    );
    fn webkit_settings_get_feature_enabled(
        settings: *mut webkit2gtk::ffi::WebKitSettings,
        feature: *mut c_void,
    ) -> i32;
}

pub(super) fn disable_directories(view: &webkit2gtk::WebView) -> Result<(), String> {
    let settings = view.settings().ok_or("missing file-selection settings")?;
    // These public 2.42 APIs are newer than the Rust binding. Runtime admission
    // requires a newer WebKit; the list owns the borrowed feature through use.
    let disabled = unsafe {
        let list = webkit_settings_get_all_features();
        if list.is_null() {
            return Err("missing WebKit feature list".into());
        }
        let feature = webkit_feature_list_find(list, c"DirectoryUpload".as_ptr());
        let disabled = if feature.is_null() {
            false
        } else {
            webkit_settings_set_feature_enabled(settings.to_glib_none().0, feature, 0);
            webkit_settings_get_feature_enabled(settings.to_glib_none().0, feature) == 0
        };
        webkit_feature_list_unref(list);
        disabled
    };
    if disabled {
        Ok(())
    } else {
        Err("directory upload could not be disabled".into())
    }
}
