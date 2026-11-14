//! The JavaScript binding against the Rust side: envelope byte fixtures that
//! both codecs must agree on, and the binding's own tests in Node with the
//! bundle exactly as [`zephium_webext_runtime::js`] assembles it.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;
use zephium_webext_runtime::envelope::{
    ApiName, CallId, DecodeError, Direction, Envelope, EventSeq, PortId,
};
use zephium_webext_runtime::js::{
    background_surface, BUNDLE, MATCHER, OWNER_BUNDLE, WORKER_BUNDLE,
};

fn js_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("js")
}

fn fixtures() -> Value {
    let text = std::fs::read_to_string(js_dir().join("test/envelopes.json")).expect("fixture file");
    serde_json::from_str(&text).expect("fixture JSON")
}

fn hex(text: &str) -> Vec<u8> {
    (0..text.len())
        .step_by(2)
        .map(|at| u8::from_str_radix(&text[at..at + 2], 16).expect("hex"))
        .collect()
}

fn direction(value: &Value) -> Direction {
    match value.as_str() {
        Some("fromContext") => Direction::FromContext,
        Some("toContext") => Direction::ToContext,
        other => panic!("unknown direction {other:?}"),
    }
}

fn field<'a>(frame: &'a Value, key: &str) -> &'a str {
    frame[key]
        .as_str()
        .unwrap_or_else(|| panic!("missing {key}"))
}

fn big(frame: &Value, key: &str) -> u64 {
    field(frame, key).parse().expect("u64")
}

fn bytes_of(frame: &Value, key: &str) -> Vec<u8> {
    match frame.get(format!("{key}Hex")) {
        Some(Value::String(encoded)) => hex(encoded),
        _ => field(frame, key).as_bytes().to_vec(),
    }
}

#[test]
fn envelope_fixtures_match_the_rust_codec() {
    let fixtures = fixtures();
    let max = fixtures["max"].as_u64().expect("max");
    let accepted = fixtures["accepted"].as_array().expect("accepted");
    assert!(accepted.len() >= 10);
    for case in accepted {
        let name = case["name"].as_str().expect("name");
        let frame = &case["frame"];
        let direction = direction(&case["direction"]);
        let blob = |key| bytes_of(frame, key);
        let (args, value, payload) = (
            frame.get("args").map(|_| blob("args")),
            frame.get("value").map(|_| blob("value")),
            frame
                .get("payload")
                .or(frame.get("payloadHex"))
                .map(|_| blob("payload")),
        );
        let error = frame.get("error").and_then(Value::as_str);
        let envelope = match field(frame, "kind") {
            "call" => Envelope::Call {
                id: CallId::new(frame["id"].as_u64().expect("id") as u32).expect("id"),
                name: ApiName::parse(field(frame, "name")).expect("name"),
                args: args.as_deref().expect("args"),
            },
            "reply" => Envelope::Reply {
                id: CallId::new(frame["id"].as_u64().expect("id") as u32).expect("id"),
                result: match frame["ok"].as_bool().expect("ok") {
                    true => Ok(value.as_deref().expect("value")),
                    false => Err(error.expect("error")),
                },
            },
            "event" => Envelope::Event {
                seq: EventSeq::new(big(frame, "seq")).expect("seq"),
                name: ApiName::parse(field(frame, "name")).expect("name"),
                args: args.as_deref().expect("args"),
            },
            "ack" => Envelope::EventAck {
                seq: EventSeq::new(big(frame, "seq")).expect("seq"),
            },
            "portMessage" => Envelope::PortMessage {
                port: PortId::new(big(frame, "port")).expect("port"),
                payload: payload.as_deref().expect("payload"),
            },
            "portDisconnect" => Envelope::PortDisconnect {
                port: PortId::new(big(frame, "port")).expect("port"),
                error,
            },
            other => panic!("unknown kind {other}"),
        };
        let expected = hex(case["hex"].as_str().expect("hex"));
        assert_eq!(
            envelope.encode(direction, max).as_deref(),
            Ok(expected.as_slice()),
            "{name}"
        );
        assert_eq!(
            Envelope::decode(&expected, direction, max),
            Ok(envelope),
            "{name}"
        );
    }
    let rejected = fixtures["rejected"].as_array().expect("rejected");
    assert!(rejected.len() >= 10);
    for case in rejected {
        let name = case["name"].as_str().expect("name");
        let bytes = hex(case["hex"].as_str().expect("hex"));
        let limit = case.get("max").and_then(Value::as_u64).unwrap_or(max);
        let refusal: DecodeError =
            Envelope::decode(&bytes, direction(&case["direction"]), limit).expect_err(name);
        let printed = format!("{refusal:?}");
        let variant = printed.split('(').next().unwrap_or_default();
        assert_eq!(variant, case["error"].as_str().expect("error"), "{name}");
    }
}

#[test]
fn binding_tests_pass_in_node() {
    let bundles = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("webext-js-bundles");
    std::fs::create_dir_all(&bundles).expect("bundle directory");
    std::fs::write(bundles.join("bundle.js"), BUNDLE).expect("write bundle");
    std::fs::write(bundles.join("matcher.js"), MATCHER).expect("write matcher");
    std::fs::write(bundles.join("owner.js"), OWNER_BUNDLE).expect("write owner relay");
    std::fs::write(bundles.join("worker.js"), WORKER_BUNDLE).expect("write worker binding");
    let projected: Vec<_> = [
        serde_json::json!({"manifest_version":3}),
        serde_json::json!({
            "manifest_version":3,"permissions":["storage"],
            "optional_permissions":["alarms","webNavigation","nativeMessaging"],"action":{}
        }),
    ]
    .into_iter()
    .map(|raw| {
        let manifest = zephium_webext::manifest::Manifest::from_value(raw.clone());
        let surface: Value = serde_json::from_str(&background_surface(&manifest)).unwrap();
        serde_json::json!({"manifest":raw,"surface":surface})
    })
    .collect();
    std::fs::write(
        bundles.join("projected-surfaces.json"),
        serde_json::to_vec(&projected).unwrap(),
    )
    .expect("write manifest projections");
    let mut tests: Vec<PathBuf> = std::fs::read_dir(js_dir())
        .expect("js directory")
        .map(|entry| entry.expect("entry").path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.ends_with(".test.mjs"))
        })
        .collect();
    tests.sort();
    assert!(!tests.is_empty());
    for surface in ["generated", "fixture"] {
        let output = match Command::new("node")
            .arg("--test")
            .args(&tests)
            .env("ZEPHIUM_WEBEXT_BUNDLE_DIR", &bundles)
            .env("ZEPHIUM_WEBEXT_TEST_SURFACE", surface)
            .output()
        {
            Ok(output) => output,
            // CI must run them; elsewhere Node is optional.
            Err(error)
                if error.kind() == std::io::ErrorKind::NotFound
                    && std::env::var_os("CI").is_none() =>
            {
                eprintln!("skipping: node is not installed");
                return;
            }
            Err(error) => panic!("could not run node: {error}"),
        };
        assert!(
            output.status.success(),
            "binding tests failed with {surface} surface:\n{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
