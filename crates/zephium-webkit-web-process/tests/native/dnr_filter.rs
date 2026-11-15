use glib::translate::ToGlibPtr;
use std::{cell::RefCell, ffi::CString, ptr, rc::Rc};
use webkit2gtk::WebViewExt;

type Saved = Rc<RefCell<Option<Result<*mut webkit2gtk::ffi::WebKitUserContentFilter, String>>>>;

unsafe extern "C" fn saved(
    source: *mut glib::gobject_ffi::GObject,
    result: *mut gio::ffi::GAsyncResult,
    data: glib::ffi::gpointer,
) {
    // The save call owns this box until its sole completion callback.
    let slot = unsafe { Box::from_raw(data.cast::<Saved>()) };
    let mut error = ptr::null_mut();
    let filter = unsafe {
        webkit2gtk::ffi::webkit_user_content_filter_store_save_finish(
            source.cast(),
            result,
            &mut error,
        )
    };
    let outcome = if error.is_null() {
        if filter.is_null() {
            Err("missing compiled filter".into())
        } else {
            Ok(filter)
        }
    } else {
        let message = unsafe { std::ffi::CStr::from_ptr((*error).message) }
            .to_string_lossy()
            .into_owned();
        unsafe { glib::ffi::g_error_free(error) };
        Err(message)
    };
    *slot.borrow_mut() = Some(outcome);
}

pub fn install(page: &webkit2gtk::WebView, source: &str) {
    let directory = tempfile::tempdir().unwrap();
    let path = CString::new(directory.path().to_str().unwrap()).unwrap();
    let bytes = glib::Bytes::from(source.as_bytes());
    let slot: Saved = Rc::new(RefCell::new(None));
    // WebKit retains the source bytes during save; the store remains alive until completion.
    let store = unsafe { webkit2gtk::ffi::webkit_user_content_filter_store_new(path.as_ptr()) };
    assert!(!store.is_null());
    unsafe {
        webkit2gtk::ffi::webkit_user_content_filter_store_save(
            store,
            c"dnr-coexistence".as_ptr(),
            bytes.to_glib_none().0,
            ptr::null_mut(),
            Some(saved),
            Box::into_raw(Box::new(slot.clone())).cast(),
        );
    }
    super::support::pump_until(|| slot.borrow().is_some());
    let filter = slot.borrow_mut().take().unwrap().unwrap();
    let manager = page.user_content_manager().unwrap();
    unsafe {
        webkit2gtk::ffi::webkit_user_content_manager_add_filter(manager.to_glib_none().0, filter);
        webkit2gtk::ffi::webkit_user_content_filter_unref(filter);
        glib::gobject_ffi::g_object_unref(store.cast());
    }
}
