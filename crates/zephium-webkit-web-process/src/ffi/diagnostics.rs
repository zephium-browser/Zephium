//! Native evaluation evidence, including MAIN-world files with no privileged JS binding.

use super::{raw, Owned};
use std::ffi::{c_char, CStr};
use std::io::Write;
use std::sync::atomic::{AtomicUsize, Ordering};

thread_local! {
    static SCOPE: std::cell::RefCell<Option<serde_json::Value>> = const { std::cell::RefCell::new(None) };
}

pub(super) fn scope(page: u64, frame: u64, document: u64, world: &str, trigger: &str) {
    SCOPE.with_borrow_mut(|scope| {
        *scope = Some(serde_json::json!({
            "page":page, "frame":frame, "document":document, "world":world, "trigger":trigger
        }))
    });
}

static RECORDS: AtomicUsize = AtomicUsize::new(0);

pub(super) fn request(id: u64) {
    SCOPE.with_borrow_mut(|scope| {
        if let Some(scope) = scope {
            scope["requestId"] = id.into();
        }
    });
}

fn bounded(pointer: *const c_char) -> String {
    if pointer.is_null() {
        return String::new();
    }
    // JSC owns a terminated string until its exception is cleared.
    let length = unsafe { libc::strnlen(pointer, 4096) };
    let bytes = unsafe { std::slice::from_raw_parts(pointer.cast::<u8>(), length) };
    redact(&String::from_utf8_lossy(bytes))
}

fn redact(text: &str) -> String {
    text.split_whitespace()
        .map(|word| {
            if word.contains("://") && !word.contains("chrome-extension://") {
                "[network-url]".to_owned()
            } else {
                word.split(['?', '#']).next().unwrap_or("").to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

pub(super) fn evaluation(context: &Owned, source: &CStr, success: bool) {
    if std::env::var_os("ZEPHIUM_WEBEXT_DIAGNOSTICS_DIR").is_none() {
        return;
    }
    let source = source.to_string_lossy();
    let Some(path) = source.strip_prefix("chrome-extension://") else {
        return;
    };
    let Some((extension, _)) = path.split_once('/') else {
        return;
    };
    if extension.len() != 32 || !extension.bytes().all(|b| (b'a'..=b'p').contains(&b)) {
        return;
    }
    let sequence = RECORDS.fetch_add(1, Ordering::Relaxed);
    if sequence > 4096 {
        return;
    }
    let exception = unsafe { raw::jsc_context_get_exception(context.0) };
    let (message, stack) = if exception.is_null() {
        (String::new(), String::new())
    } else {
        (
            bounded(unsafe { raw::jsc_exception_get_message(exception) }),
            bounded(unsafe { raw::jsc_exception_get_backtrace_string(exception) }),
        )
    };
    let mut event = if sequence == 4096 {
        serde_json::json!({"kind":"native-capture-truncated"})
    } else {
        serde_json::json!({"kind":"native-evaluation", "source":redact(&source), "success":success, "message":message, "stack":stack})
    };
    if let Some(scope) = SCOPE.with_borrow_mut(Option::take) {
        event["scope"] = scope;
    }
    // The runner merges these bounded records into this extension's JSONL file.
    let row = serde_json::json!({"extension":extension, "context":"ContentScript", "process":std::process::id(), "sequence":sequence, "event":event});
    let mut text = row.to_string();
    if text.len() > 16 * 1024 {
        text = serde_json::json!({"extension":extension, "context":"ContentScript", "event":{"kind":"native-record-too-large"}}).to_string();
    }
    let _ = writeln!(std::io::stderr().lock(), "ZPD {text}");
}

pub(super) fn post_refused(page: u64, frame: u64, world: &str, error: &crate::state::StateError) {
    if std::env::var_os("ZEPHIUM_WEBEXT_DIAGNOSTICS_DIR").is_none() {
        return;
    }
    let Ok(extension) = std::env::var("ZEPHIUM_WEBEXT_DIAGNOSTIC_EXTENSION") else {
        return;
    };
    let sequence = RECORDS.fetch_add(1, Ordering::Relaxed);
    if sequence > 4096 {
        return;
    }
    let event = if sequence == 4096 {
        serde_json::json!({"kind":"native-capture-truncated"})
    } else {
        serde_json::json!({"kind":"native-post-refused","page":page,"frame":frame,"world":world,"reason":format!("{error:?}")})
    };
    let row = serde_json::json!({"extension":extension,"context":"transport","process":std::process::id(),"sequence":sequence,"event":event});
    let _ = writeln!(std::io::stderr().lock(), "ZPD {row}");
}

#[cfg(test)]
mod tests {
    #[test]
    fn redacts_network_urls_and_extension_query_strings() {
        assert_eq!(
            super::redact("run@chrome-extension://id/content.js:12:4"),
            "run@chrome-extension://id/content.js:12:4"
        );
        assert_eq!(
            super::redact(
                "at https://user:secret@host/path?q=s chrome-extension://id/file.js?secret#x"
            ),
            "at [network-url] chrome-extension://id/file.js"
        );
    }
}
