//! The JavaScript binding against the Rust side: envelope byte fixtures that
//! both codecs must agree on.

use std::path::{Path, PathBuf};

use serde_json::Value;
use zephium_webext_runtime::envelope::{
    ApiName, CallId, DecodeError, Direction, Envelope, EventSeq, PortId,
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
