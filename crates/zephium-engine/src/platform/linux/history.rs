//! Bounded snapshots and exact native history jumps.
use gtk::glib::translate::ToGlibPtr;
use webkit2gtk::{BackForwardListExt, WebViewExt};
use wry::WebViewExtUnix;
use zephium_core::ports::engine::MAX_HISTORY_SIDE;

pub(crate) fn history_session(
    view: &wry::WebView,
    done: impl FnOnce(Option<(Vec<(String, String)>, usize)>),
) -> bool {
    let Some(list) = view.webview().back_forward_list() else {
        done(None);
        return true;
    };
    let Some(current) = list.nth_item(0) else {
        done(None);
        return true;
    };
    let mut back = Vec::new();
    for distance in 1..=MAX_HISTORY_SIDE as i32 {
        let Some(item) = list.nth_item(-distance) else {
            break;
        };
        back.push(entry(&item));
    }
    back.reverse();
    let current_at = back.len();
    back.push(entry(&current));
    for distance in 1..=MAX_HISTORY_SIDE as i32 {
        let Some(item) = list.nth_item(distance) else {
            break;
        };
        back.push(entry(&item));
    }
    done(Some((back, current_at)));
    true
}

pub(crate) fn go_to_history_entry(view: &wry::WebView, offset: i32, url: &str) -> bool {
    if !valid_jump(offset, url) {
        return false;
    }
    let page = view.webview();
    let Some(item) = page
        .back_forward_list()
        .and_then(|list| list.nth_item(offset))
    else {
        return false;
    };
    if entry(&item).1 != url {
        return false;
    }
    page.go_to_back_forward_list_item(&item);
    true
}
fn valid_jump(offset: i32, url: &str) -> bool {
    offset != 0
        && offset.unsigned_abs() as usize <= MAX_HISTORY_SIDE
        && !url.is_empty()
        && url.len() <= 8192
}
fn entry(item: &webkit2gtk::BackForwardListItem) -> (String, String) {
    // SAFETY: both accessors return C strings owned by this live native item.
    unsafe {
        (
            bounded(webkit2gtk::ffi::webkit_back_forward_list_item_get_title(
                item.to_glib_none().0,
            )),
            bounded(webkit2gtk::ffi::webkit_back_forward_list_item_get_uri(
                item.to_glib_none().0,
            )),
        )
    }
}
unsafe fn bounded(pointer: *const std::ffi::c_char) -> String {
    if pointer.is_null() {
        return String::new();
    }
    // SAFETY: caller supplies a live native NUL-terminated string.
    let length = unsafe { libc::strnlen(pointer, 8193) };
    if length > 8192 {
        return String::new();
    }
    // SAFETY: admitted prefix lies within the live native string.
    std::str::from_utf8(unsafe { std::slice::from_raw_parts(pointer.cast::<u8>(), length) })
        .unwrap_or_default()
        .to_owned()
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn jump_refuses_overflow_zero_and_unbounded_addresses() {
        for offset in [0, i32::MIN, i32::MAX, MAX_HISTORY_SIDE as i32 + 1] {
            assert!(!valid_jump(offset, "https://example.com/"));
        }
        assert!(!valid_jump(1, ""));
        assert!(!valid_jump(-1, &"x".repeat(8193)));
        assert!(valid_jump(-1, "https://example.com/"));
    }
}
