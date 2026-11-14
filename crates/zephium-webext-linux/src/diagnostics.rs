//! Opt-in, bounded evidence for native tests; never compiled into normal builds.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

const MAX_ROWS: usize = 4096;
const MAX_ROW_BYTES: usize = 16 * 1024;

struct Log {
    file: File,
    rows: usize,
    api_file: File,
    api_rows: usize,
}

thread_local! {
    static LOGS: RefCell<BTreeMap<String, Log>> = const { RefCell::new(BTreeMap::new()) };
}

/// Creates one fresh JSONL file before installing an extension in a test host.
pub fn begin(directory: &Path, extension: &str) -> std::io::Result<PathBuf> {
    if extension.len() != 32 || !extension.bytes().all(|b| (b'a'..=b'p').contains(&b)) {
        return Err(std::io::Error::other("invalid diagnostic extension id"));
    }
    std::fs::create_dir_all(directory)?;
    let path = directory.join(format!("{extension}.jsonl"));
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)?;
    let api_file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(directory.join(format!("{extension}.api.jsonl")))?;
    LOGS.with_borrow_mut(|logs| {
        if logs.len() >= 16 || logs.contains_key(extension) {
            return Err(std::io::Error::other(
                "diagnostic log already active or full",
            ));
        }
        logs.insert(
            extension.into(),
            Log {
                file,
                rows: 0,
                api_file,
                api_rows: 0,
            },
        );
        Ok(())
    })?;
    record(
        extension,
        "host",
        json!({"kind":"capture-start", "maxRows":MAX_ROWS, "maxRowBytes":MAX_ROW_BYTES}),
    );
    Ok(path)
}

#[cfg(test)]
pub(crate) fn end(extension: &str) {
    LOGS.with_borrow_mut(|logs| {
        logs.remove(extension);
    });
}

pub(crate) fn transport(event: Value) {
    let ids = LOGS.with_borrow(|logs| logs.keys().cloned().collect::<Vec<_>>());
    for id in ids {
        record(&id, "transport", event.clone());
    }
}

pub(crate) fn enabled() -> bool {
    LOGS.with_borrow(|logs| !logs.is_empty())
}

/// Writes test evidence immediately, including before a later panic/timeout.
pub fn record(extension: &str, context: &str, event: Value) {
    LOGS.with_borrow_mut(|logs| {
        let Some(log) = logs.get_mut(extension) else { return };
        if event["kind"] == "javascript" && event["detail"]["kind"] == "api-trace" {
            if log.api_rows <= MAX_ROWS {
                let row = json!({"extension":extension,"context":context,"event":event});
                let text = row.to_string();
                if text.len() <= MAX_ROW_BYTES {
                    let _ = writeln!(log.api_file, "{text}");
                }
                log.api_rows += 1;
            }
            return;
        }
        if log.rows > MAX_ROWS { return; }
        let event = if log.rows == MAX_ROWS { json!({"kind":"capture-truncated"}) } else { event };
        let row = json!({"sequence":log.rows,"extension":extension,"context":context,"event":event});
        let text = row.to_string();
        if text.len() > MAX_ROW_BYTES {
            let _ = writeln!(log.file, "{}", json!({"sequence":log.rows,"extension":extension,"context":context,"event":{"kind":"record-too-large"}}));
        } else {
            let _ = writeln!(log.file, "{text}");
        }
        log.rows += 1;
    });
}

pub(crate) fn bundle(source: &str, worker: bool) -> String {
    if !enabled() {
        return source.to_owned();
    }
    let source = if worker {
        source.to_owned()
    } else {
        // Test instrumentation needs the private client, never exposed to extension code.
        let anchor = "return create(surface, bootstrap, T.nativeLink(I.global), {}).chrome;";
        assert_eq!(source.matches(anchor).count(), 1, "page binding changed");
        source.replace(
            anchor,
            "return create(surface, bootstrap, T.nativeLink(I.global), {});",
        )
    };
    format!("({})(({source}),{worker})", include_str!("diagnostics.js"))
}

pub(crate) fn redact(text: &str) -> String {
    let mut end = text.len().min(4096);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    let text = &text[..end];
    text.split_inclusive(char::is_whitespace)
        .map(|part| {
            let word = part.trim_end_matches(char::is_whitespace);
            let whitespace = &part[word.len()..];
            let word = if word.contains("://") && !word.contains("chrome-extension://") {
                "[network-url]"
            } else if word.contains("chrome-extension://") {
                word.split(['?', '#']).next().unwrap_or("")
            } else {
                word
            };
            format!("{word}{whitespace}")
        })
        .collect()
}

pub(crate) fn decode(args: &[u8]) -> Option<Value> {
    if args.len() > 8192 {
        return None;
    }
    let Value::Array(mut events) = serde_json::from_slice(args).ok()? else {
        return None;
    };
    if events.len() != 1
        || !events[0].is_object()
        || events[0]["kind"]
            .as_str()
            .is_none_or(|kind| kind.len() > 64)
    {
        return None;
    }
    events.pop()
}

pub(crate) fn argument_shape(args: &[u8]) -> Value {
    fn shape(value: &Value, depth: usize) -> Value {
        match value {
            Value::Null => json!("null"),
            Value::Bool(_) => json!("boolean"),
            Value::Number(_) => json!("number"),
            Value::String(_) => json!("string"),
            Value::Array(items) if depth < 2 => {
                Value::Array(items.iter().take(16).map(|v| shape(v, depth + 1)).collect())
            }
            Value::Object(items) if depth < 2 => {
                json!({"type":"object", "fields":items.values().take(16).map(|v| shape(v, depth + 1)).collect::<Vec<_>>()})
            }
            Value::Array(_) => json!("array"),
            Value::Object(_) => json!("object"),
        }
    }
    if args.len() > 8192 {
        return json!("over-shape-budget");
    }
    serde_json::from_slice::<Value>(args).map_or(json!("invalid-json"), |value| shape(&value, 0))
}

/// Resource failures from a test view, including loads outside the JS error events.
pub fn observe_view(view: &webkit2gtk::WebView, extension: &str, context: &str) {
    use std::cell::Cell;
    use std::rc::Rc;
    use webkit2gtk::{URIResponseExt, WebResourceExt, WebViewExt};
    if !enabled() {
        return;
    }
    let generation = Rc::new(Cell::new(0_u64));
    let loading = generation.clone();
    view.connect_load_changed(move |_, event| {
        if event == webkit2gtk::LoadEvent::Started {
            loading.set(loading.get().saturating_add(1));
        }
    });
    let (id, kind) = (extension.to_owned(), context.to_owned());
    view.connect_load_failed(move |_, _, uri, error| {
        record(&id, &kind, json!({"kind":"load-failed", "resource":resource(uri), "domain":error.domain().as_str().to_string(), "message":redact(error.message())}));
        false
    });
    let (id, kind) = (extension.to_owned(), context.to_owned());
    view.connect_resource_load_started(move |_, resource, _| {
        let document = generation.get();
        let current = generation.clone();
        let (id, kind) = (id.clone(), kind.clone());
        let (failed_id, failed_kind) = (id.clone(), kind.clone());
        resource.connect_failed(move |item, error| {
            record(&failed_id, &failed_kind, json!({"kind":"resource-failed", "resource":self::resource(item.uri().as_deref().unwrap_or("")), "document":document,"currentDocument":current.get(),"cancelled":error.matches(webkit2gtk::NetworkError::Cancelled),"domain":error.domain().as_str().to_string(), "message":redact(error.message())}));
        });
        resource.connect_finished(move |item| {
            if let Some(response) = item.response() {
                if response.status_code() >= 400 {
                    record(&id, &kind, json!({"kind":"resource-http-error", "resource":self::resource(item.uri().as_deref().unwrap_or("")), "status":response.status_code()}));
                }
            }
        });
    });
}

fn resource(value: &str) -> String {
    match url::Url::parse(value) {
        Ok(url) if url.scheme() == "chrome-extension" => format!(
            "chrome-extension://{}{}",
            url.host_str().unwrap_or(""),
            url.path()
        ),
        Ok(url) if matches!(url.scheme(), "http" | "https") => url.origin().ascii_serialization(),
        Ok(url) => format!("{}://[redacted]", url.scheme()),
        Err(_) => "[unknown]".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diagnostic_binding_tests_pass_in_node() {
        let output = std::process::Command::new("node")
            .arg(
                std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/diagnostics.test.mjs"),
            )
            .output()
            .expect("pinned Node toolchain");
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn malformed_reports_and_argument_values_are_not_kept() {
        for input in [b"[]".as_slice(), b"[1]", b"[{},{}]", b"invalid", b"[{}]"] {
            assert!(decode(input).is_none());
        }
        assert!(decode(&vec![b' '; 8193]).is_none());
        assert!(decode(br#"[{"kind":"uncaught","message":"bad"}]"#).is_some());
        let shape = argument_shape(br#"[{"password":"secret","count":12}]"#).to_string();
        assert!(!shape.contains("secret") && !shape.contains("password") && !shape.contains("12"));
    }

    #[test]
    fn bounds_evidence_and_rejects_path_input() {
        let dir = tempfile::tempdir().unwrap();
        assert!(begin(dir.path(), "../bad").is_err());
        let id = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let path = begin(dir.path(), id).unwrap();
        assert!(bundle(zephium_webext_runtime::js::BUNDLE, false).contains("diagnosticBinding"));
        record(
            id,
            "popup",
            json!({"kind":"oversized", "data":"x".repeat(MAX_ROW_BYTES)}),
        );
        for _ in 0..MAX_ROWS + 10 {
            record(id, "popup", json!({"kind":"test"}));
        }
        let text = std::fs::read_to_string(path).unwrap();
        assert_eq!(text.lines().count(), MAX_ROWS + 1);
        assert!(text.contains("record-too-large"));
        assert!(text.lines().last().unwrap().contains("capture-truncated"));
        assert!(text.lines().all(|line| line.len() <= MAX_ROW_BYTES));
        LOGS.with_borrow_mut(|logs| logs.clear());
    }
}
