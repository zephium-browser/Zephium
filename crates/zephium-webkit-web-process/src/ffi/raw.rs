use std::ffi::{c_char, c_int, c_uint, c_ulong, c_void};

pub type Object = c_void;
pub type Variant = c_void;
pub type Callback = Option<unsafe extern "C" fn()>;
pub type AsyncCallback = Option<unsafe extern "C" fn(*mut Object, *mut Object, *mut c_void)>;
pub type Notify = Option<unsafe extern "C" fn(*mut c_void)>;
pub type WeakNotify = Option<unsafe extern "C" fn(*mut c_void, *mut Object)>;

unsafe extern "C" {
    pub fn webkit_get_major_version() -> c_uint;
    pub fn webkit_get_minor_version() -> c_uint;
    pub fn webkit_script_world_get_default() -> *mut Object;
    pub fn webkit_script_world_new_with_name(name: *const c_char) -> *mut Object;
    pub fn webkit_frame_get_id(frame: *mut Object) -> u64;
    pub fn webkit_frame_is_main_frame(frame: *mut Object) -> c_int;
    pub fn webkit_frame_get_uri(frame: *mut Object) -> *const c_char;
    pub fn webkit_frame_get_javascript_global_context(frame: *mut Object) -> *mut Object;
    pub fn webkit_frame_get_js_context_for_script_world(
        frame: *mut Object,
        world: *mut Object,
    ) -> *mut Object;
    pub fn webkit_web_page_get_id(page: *mut Object) -> u64;
    pub fn webkit_web_page_get_form_manager(page: *mut Object, world: *mut Object) -> *mut Object;
    pub fn webkit_web_page_send_message_to_view(
        page: *mut Object,
        message: *mut Object,
        cancellable: *mut Object,
        callback: AsyncCallback,
        data: *mut c_void,
    );
    pub fn webkit_web_page_send_message_to_view_finish(
        page: *mut Object,
        result: *mut Object,
        error: *mut *mut Object,
    ) -> *mut Object;
    pub fn webkit_user_message_new(name: *const c_char, parameters: *mut Variant) -> *mut Object;
    pub fn webkit_user_message_get_name(message: *mut Object) -> *const c_char;
    pub fn webkit_user_message_get_parameters(message: *mut Object) -> *mut Variant;
    pub fn webkit_user_message_get_fd_list(message: *mut Object) -> *mut Object;
    pub fn webkit_user_message_send_reply(message: *mut Object, reply: *mut Object);
    pub fn webkit_uri_request_get_uri(request: *mut Object) -> *const c_char;
    pub fn webkit_uri_request_set_uri(request: *mut Object, uri: *const c_char);
    pub fn webkit_uri_request_get_http_method(request: *mut Object) -> *const c_char;
    pub fn webkit_uri_request_get_http_headers(request: *mut Object) -> *mut Object;

    pub fn g_object_ref(object: *mut Object) -> *mut Object;
    pub fn g_object_unref(object: *mut Object);
    pub fn g_object_weak_unref(object: *mut Object, notify: WeakNotify, data: *mut c_void);
    pub fn g_object_weak_ref(object: *mut Object, notify: WeakNotify, data: *mut c_void);
    pub fn g_signal_connect_data(
        object: *mut Object,
        signal: *const c_char,
        callback: Callback,
        data: *mut c_void,
        destroy: Option<unsafe extern "C" fn(*mut c_void, *mut Object)>,
        flags: c_uint,
    ) -> c_ulong;
    pub fn g_variant_is_of_type(value: *mut Variant, ty: *const c_char) -> c_int;
    pub fn g_variant_get_child_value(value: *mut Variant, index: usize) -> *mut Variant;
    pub fn g_variant_get_uint32(value: *mut Variant) -> u32;
    pub fn g_variant_get_string(value: *mut Variant, length: *mut usize) -> *const c_char;
    pub fn g_variant_get_fixed_array(
        value: *mut Variant,
        length: *mut usize,
        element_size: usize,
    ) -> *const c_void;
    pub fn g_variant_get_size(value: *mut Variant) -> usize;
    pub fn g_variant_new_from_data(
        ty: *const c_char,
        data: *const c_void,
        size: usize,
        trusted: c_int,
        notify: Notify,
        user_data: *mut c_void,
    ) -> *mut Variant;
    pub fn g_variant_new_boolean(value: c_int) -> *mut Variant;
    pub fn g_variant_unref(value: *mut Variant);
    pub fn g_unix_fd_list_get(list: *mut Object, index: c_int, error: *mut *mut Object) -> c_int;
    pub fn g_unix_fd_list_get_length(list: *mut Object) -> c_int;
    pub fn g_error_free(error: *mut Object);
    pub fn g_free(pointer: *mut c_void);
    pub fn g_get_monotonic_time() -> i64;
    pub fn g_timeout_add_full(
        priority: c_int,
        interval: c_uint,
        function: Option<unsafe extern "C" fn(*mut c_void) -> c_int>,
        data: *mut c_void,
        notify: Notify,
    ) -> c_uint;

    pub fn jsc_value_get_type() -> usize;
    pub fn jsc_value_is_typed_array(value: *mut Object) -> c_int;
    pub fn jsc_value_typed_array_get_type(value: *mut Object) -> c_int;
    pub fn jsc_value_typed_array_get_size(value: *mut Object) -> usize;
    pub fn jsc_value_typed_array_get_data(value: *mut Object, length: *mut usize) -> *mut c_void;
    pub fn jsc_value_new_typed_array(context: *mut Object, ty: c_int, length: usize)
        -> *mut Object;
    pub fn jsc_value_new_functionv(
        context: *mut Object,
        name: *const c_char,
        callback: Callback,
        data: *mut c_void,
        notify: Notify,
        result_type: usize,
        count: c_uint,
        types: *mut usize,
    ) -> *mut Object;
    pub fn jsc_value_object_get_property_at_index(value: *mut Object, index: c_uint)
        -> *mut Object;
    pub fn jsc_value_to_int32(value: *mut Object) -> i32;
    pub fn jsc_value_to_string(value: *mut Object) -> *mut c_char;
    pub fn jsc_value_is_string(value: *mut Object) -> c_int;
    pub fn jsc_value_object_get_property(value: *mut Object, name: *const c_char) -> *mut Object;
    pub fn jsc_value_is_function(value: *mut Object) -> c_int;
    pub fn jsc_value_function_callv(
        function: *mut Object,
        count: c_uint,
        parameters: *mut *mut Object,
    ) -> *mut Object;
    pub fn jsc_context_set_value(context: *mut Object, name: *const c_char, value: *mut Object);
    pub fn jsc_context_evaluate(
        context: *mut Object,
        source: *const c_char,
        length: isize,
    ) -> *mut Object;
    pub fn jsc_context_get_exception(context: *mut Object) -> *mut Object;
    pub fn jsc_context_clear_exception(context: *mut Object);
    pub fn jsc_weak_value_new(value: *mut Object) -> *mut Object;
    pub fn jsc_weak_value_get_value(value: *mut Object) -> *mut Object;
    pub fn soup_message_headers_get_one(headers: *mut Object, name: *const c_char)
        -> *const c_char;
    pub fn soup_message_headers_replace(
        headers: *mut Object,
        name: *const c_char,
        value: *const c_char,
    );
}
