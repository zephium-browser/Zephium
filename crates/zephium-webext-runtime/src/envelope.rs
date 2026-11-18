//! The bridge wire format.
//!
//! One frame per transport message:
//!
//! ```text
//! u32 total length (LE, includes itself) | u8 version | u8 kind | body
//! ```
//!
//! Integers are little-endian; strings and blobs are length-prefixed. The
//! decoder accepts exactly one well-formed frame and nothing else, so every
//! accepted frame re-encodes to the same bytes. Frames carry no sender
//! identity: that comes from the host's binding of the delivering context.
//! API arguments are JSON; message and port payloads are opaque.

use std::cell::Cell;
use std::fmt;
use std::num::{NonZeroU32, NonZeroU64};

use serde::de::{DeserializeSeed, MapAccess, SeqAccess, Visitor};
use serde_json::{Map, Value};

use crate::limits::{MAX_API_NAME_BYTES, MAX_ARGUMENT_DEPTH};

/// The only version this build speaks.
pub const ENVELOPE_VERSION: u8 = 1;
/// Longest error text a reply or disconnect may carry.
pub const MAX_ERROR_MESSAGE_BYTES: usize = 16 * 1024;

const HEADER_BYTES: usize = 6;

const KIND_CALL: u8 = 1;
const KIND_REPLY: u8 = 2;
const KIND_EVENT: u8 = 3;
const KIND_EVENT_ACK: u8 = 4;
const KIND_PORT_MESSAGE: u8 = 5;
const KIND_PORT_DISCONNECT: u8 = 6;

/// Which way a frame travels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    /// From an extension context to the host.
    FromContext,
    /// From the host to an extension context.
    ToContext,
}

/// A context-chosen call id, unique among that context's outstanding calls.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct CallId(NonZeroU32);

impl CallId {
    /// A nonzero id.
    pub const fn new(value: u32) -> Option<Self> {
        match NonZeroU32::new(value) {
            Some(value) => Some(Self(value)),
            None => None,
        }
    }

    /// The raw id.
    pub const fn get(self) -> u32 {
        self.0.get()
    }
}

/// A host-minted port id.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PortId(NonZeroU64);

impl PortId {
    /// A nonzero id.
    pub const fn new(value: u64) -> Option<Self> {
        match NonZeroU64::new(value) {
            Some(value) => Some(Self(value)),
            None => None,
        }
    }

    /// The raw id.
    pub const fn get(self) -> u64 {
        self.0.get()
    }
}

/// A host-minted event delivery sequence, acknowledged by the context.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct EventSeq(NonZeroU64);

impl EventSeq {
    /// A nonzero sequence number.
    pub const fn new(value: u64) -> Option<Self> {
        match NonZeroU64::new(value) {
            Some(value) => Some(Self(value)),
            None => None,
        }
    }

    /// The raw value.
    pub const fn get(self) -> u64 {
        self.0.get()
    }
}

/// A validated dotted API name such as `tabs.query` or `runtime.onMessage`:
/// at least two segments, each an ASCII letter followed by letters, digits
/// or `_`, at most [`MAX_API_NAME_BYTES`] in all.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ApiName<'a>(&'a str);

impl<'a> ApiName<'a> {
    /// Validates a name.
    pub fn parse(name: &'a str) -> Option<Self> {
        let valid = name.len() <= MAX_API_NAME_BYTES
            && name.split('.').count() >= 2
            && name.split('.').all(|segment| {
                let mut bytes = segment.bytes();
                bytes
                    .next()
                    .is_some_and(|first| first.is_ascii_alphabetic())
                    && bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
            });
        valid.then_some(Self(name))
    }

    /// The name.
    pub const fn as_str(self) -> &'a str {
        self.0
    }
}

impl fmt::Display for ApiName<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.0)
    }
}

/// One decoded frame, borrowing the transport buffer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Envelope<'a> {
    /// An API call.
    Call {
        /// The context's call id.
        id: CallId,
        /// `namespace.method`.
        name: ApiName<'a>,
        /// The JSON argument array, not yet parsed.
        args: &'a [u8],
    },
    /// The settlement of a call.
    Reply {
        /// The call being settled.
        id: CallId,
        /// The JSON result, or the error text.
        result: Result<&'a [u8], &'a str>,
    },
    /// An event for the context's listeners.
    Event {
        /// Delivery sequence to acknowledge.
        seq: EventSeq,
        /// `namespace.onEvent`.
        name: ApiName<'a>,
        /// The JSON argument array.
        args: &'a [u8],
    },
    /// The context handled an event.
    EventAck {
        /// The acknowledged delivery.
        seq: EventSeq,
    },
    /// A port message; the payload is opaque to the host.
    PortMessage {
        /// The port.
        port: PortId,
        /// The serialized message.
        payload: &'a [u8],
    },
    /// A port closed. Only the host attaches an error.
    PortDisconnect {
        /// The port.
        port: PortId,
        /// `runtime.lastError` for the other end.
        error: Option<&'a str>,
    },
}

/// Why a frame was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DecodeError {
    /// Shorter than its header or a declared field.
    Truncated,
    /// The length prefix disagrees with the transport message.
    LengthMismatch,
    /// Larger than the context may send.
    TooLarge,
    /// A version this build does not speak.
    UnsupportedVersion(u8),
    /// An unknown frame kind.
    UnknownKind(u8),
    /// A kind that does not travel this way.
    WrongDirection,
    /// A zero id or sequence.
    ZeroId,
    /// A malformed API name.
    InvalidName,
    /// A reply status or flag byte other than 0 or 1.
    InvalidFlag,
    /// Error text that is not UTF-8.
    InvalidUtf8,
    /// Error text over [`MAX_ERROR_MESSAGE_BYTES`].
    ErrorTooLong,
    /// Bytes after the body.
    TrailingBytes,
}

/// Why a frame could not be encoded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EncodeError {
    /// The frame would exceed the receiving context's limit.
    TooLarge,
    /// Error text over [`MAX_ERROR_MESSAGE_BYTES`].
    ErrorTooLong,
    /// A context never sends a disconnect error.
    WrongDirection,
}

struct Reader<'a> {
    bytes: &'a [u8],
}

impl<'a> Reader<'a> {
    fn take(&mut self, count: usize) -> Result<&'a [u8], DecodeError> {
        if count > self.bytes.len() {
            return Err(DecodeError::Truncated);
        }
        let (head, rest) = self.bytes.split_at(count);
        self.bytes = rest;
        Ok(head)
    }

    fn u8(&mut self) -> Result<u8, DecodeError> {
        Ok(self.take(1)?[0])
    }

    fn u32(&mut self) -> Result<u32, DecodeError> {
        let mut raw = [0; 4];
        raw.copy_from_slice(self.take(4)?);
        Ok(u32::from_le_bytes(raw))
    }

    fn u64(&mut self) -> Result<u64, DecodeError> {
        let mut raw = [0; 8];
        raw.copy_from_slice(self.take(8)?);
        Ok(u64::from_le_bytes(raw))
    }

    fn flag(&mut self) -> Result<bool, DecodeError> {
        match self.u8()? {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(DecodeError::InvalidFlag),
        }
    }

    fn blob(&mut self) -> Result<&'a [u8], DecodeError> {
        let length = usize::try_from(self.u32()?).map_err(|_| DecodeError::Truncated)?;
        self.take(length)
    }

    fn name(&mut self) -> Result<ApiName<'a>, DecodeError> {
        let length = usize::from(self.u8()?);
        let raw = self.take(length)?;
        std::str::from_utf8(raw)
            .ok()
            .and_then(ApiName::parse)
            .ok_or(DecodeError::InvalidName)
    }

    fn text(&mut self) -> Result<&'a str, DecodeError> {
        let raw = self.blob()?;
        if raw.len() > MAX_ERROR_MESSAGE_BYTES {
            return Err(DecodeError::ErrorTooLong);
        }
        std::str::from_utf8(raw).map_err(|_| DecodeError::InvalidUtf8)
    }

    fn call_id(&mut self) -> Result<CallId, DecodeError> {
        CallId::new(self.u32()?).ok_or(DecodeError::ZeroId)
    }

    fn port(&mut self) -> Result<PortId, DecodeError> {
        PortId::new(self.u64()?).ok_or(DecodeError::ZeroId)
    }

    fn seq(&mut self) -> Result<EventSeq, DecodeError> {
        EventSeq::new(self.u64()?).ok_or(DecodeError::ZeroId)
    }
}

impl<'a> Envelope<'a> {
    /// Decodes exactly one frame travelling `direction`, refusing anything
    /// longer than `max_bytes` before reading its body.
    pub fn decode(
        bytes: &'a [u8],
        direction: Direction,
        max_bytes: u64,
    ) -> Result<Self, DecodeError> {
        if u64::try_from(bytes.len()).map_or(true, |length| length > max_bytes) {
            return Err(DecodeError::TooLarge);
        }
        let mut reader = Reader { bytes };
        let declared = reader.u32()?;
        if usize::try_from(declared).ok() != Some(bytes.len()) {
            return Err(DecodeError::LengthMismatch);
        }
        let version = reader.u8()?;
        if version != ENVELOPE_VERSION {
            return Err(DecodeError::UnsupportedVersion(version));
        }
        let kind = reader.u8()?;
        let from_context = direction == Direction::FromContext;
        let envelope = match kind {
            KIND_CALL if from_context => Self::Call {
                id: reader.call_id()?,
                name: reader.name()?,
                args: reader.blob()?,
            },
            KIND_REPLY if !from_context => {
                let id = reader.call_id()?;
                let failed = reader.flag()?;
                let result = if failed {
                    Err(reader.text()?)
                } else {
                    Ok(reader.blob()?)
                };
                Self::Reply { id, result }
            }
            KIND_EVENT if !from_context => Self::Event {
                seq: reader.seq()?,
                name: reader.name()?,
                args: reader.blob()?,
            },
            KIND_EVENT_ACK if from_context => Self::EventAck { seq: reader.seq()? },
            KIND_PORT_MESSAGE => Self::PortMessage {
                port: reader.port()?,
                payload: reader.blob()?,
            },
            KIND_PORT_DISCONNECT => {
                let port = reader.port()?;
                let error = if reader.flag()? {
                    if from_context {
                        return Err(DecodeError::WrongDirection);
                    }
                    Some(reader.text()?)
                } else {
                    None
                };
                Self::PortDisconnect { port, error }
            }
            KIND_CALL | KIND_REPLY | KIND_EVENT | KIND_EVENT_ACK => {
                return Err(DecodeError::WrongDirection)
            }
            other => return Err(DecodeError::UnknownKind(other)),
        };
        if !reader.bytes.is_empty() {
            return Err(DecodeError::TrailingBytes);
        }
        Ok(envelope)
    }

    /// Encodes the frame for a receiver that accepts `max_bytes`.
    pub fn encode(&self, direction: Direction, max_bytes: u64) -> Result<Vec<u8>, EncodeError> {
        let mut out = vec![0, 0, 0, 0, ENVELOPE_VERSION];
        let blob = |out: &mut Vec<u8>, bytes: &[u8]| -> Result<(), EncodeError> {
            let length = u32::try_from(bytes.len()).map_err(|_| EncodeError::TooLarge)?;
            out.extend_from_slice(&length.to_le_bytes());
            out.extend_from_slice(bytes);
            Ok(())
        };
        let text = |out: &mut Vec<u8>, text: &str| -> Result<(), EncodeError> {
            if text.len() > MAX_ERROR_MESSAGE_BYTES {
                return Err(EncodeError::ErrorTooLong);
            }
            blob(out, text.as_bytes())
        };
        let name = |out: &mut Vec<u8>, name: ApiName<'_>| {
            // `ApiName` is at most `MAX_API_NAME_BYTES`, below 256.
            out.push(name.0.len() as u8);
            out.extend_from_slice(name.0.as_bytes());
        };
        match self {
            Self::Call {
                id,
                name: api,
                args,
            } => {
                out.push(KIND_CALL);
                out.extend_from_slice(&id.get().to_le_bytes());
                name(&mut out, *api);
                blob(&mut out, args)?;
            }
            Self::Reply { id, result } => {
                out.push(KIND_REPLY);
                out.extend_from_slice(&id.get().to_le_bytes());
                match result {
                    Ok(value) => {
                        out.push(0);
                        blob(&mut out, value)?;
                    }
                    Err(message) => {
                        out.push(1);
                        text(&mut out, message)?;
                    }
                }
            }
            Self::Event {
                seq,
                name: api,
                args,
            } => {
                out.push(KIND_EVENT);
                out.extend_from_slice(&seq.get().to_le_bytes());
                name(&mut out, *api);
                blob(&mut out, args)?;
            }
            Self::EventAck { seq } => {
                out.push(KIND_EVENT_ACK);
                out.extend_from_slice(&seq.get().to_le_bytes());
            }
            Self::PortMessage { port, payload } => {
                out.push(KIND_PORT_MESSAGE);
                out.extend_from_slice(&port.get().to_le_bytes());
                blob(&mut out, payload)?;
            }
            Self::PortDisconnect { port, error } => {
                out.push(KIND_PORT_DISCONNECT);
                out.extend_from_slice(&port.get().to_le_bytes());
                match error {
                    Some(_) if direction == Direction::FromContext => {
                        return Err(EncodeError::WrongDirection)
                    }
                    Some(message) => {
                        out.push(1);
                        text(&mut out, message)?;
                    }
                    None => out.push(0),
                }
            }
        }
        let valid_direction = match self {
            Self::Call { .. } | Self::EventAck { .. } => direction == Direction::FromContext,
            Self::Reply { .. } | Self::Event { .. } => direction == Direction::ToContext,
            Self::PortMessage { .. } | Self::PortDisconnect { .. } => true,
        };
        if !valid_direction {
            return Err(EncodeError::WrongDirection);
        }
        let length = u32::try_from(out.len()).map_err(|_| EncodeError::TooLarge)?;
        if u64::from(length) > max_bytes {
            return Err(EncodeError::TooLarge);
        }
        out[..4].copy_from_slice(&length.to_le_bytes());
        debug_assert!(out.len() >= HEADER_BYTES);
        Ok(out)
    }
}

/// Why API arguments were refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArgumentsError {
    /// Not one well-formed JSON value.
    Malformed,
    /// Not a JSON array.
    NotAnArray,
    /// Nested deeper than [`MAX_ARGUMENT_DEPTH`].
    TooDeep,
    /// An object repeats a key.
    DuplicateKey,
}

/// Parses a call's JSON argument array strictly: one value, an array,
/// nesting at most [`MAX_ARGUMENT_DEPTH`] containers, no repeated keys.
pub fn parse_arguments(bytes: &[u8]) -> Result<Vec<Value>, ArgumentsError> {
    let fault = Cell::new(None);
    let mut deserializer = serde_json::Deserializer::from_slice(bytes);
    let value = Strict {
        depth: 0,
        fault: &fault,
    }
    .deserialize(&mut deserializer)
    .map_err(|_| fault.get().unwrap_or(ArgumentsError::Malformed))?;
    deserializer.end().map_err(|_| ArgumentsError::Malformed)?;
    match value {
        Value::Array(values) => Ok(values),
        _ => Err(ArgumentsError::NotAnArray),
    }
}

struct Strict<'f> {
    depth: usize,
    fault: &'f Cell<Option<ArgumentsError>>,
}

impl<'f> Strict<'f> {
    fn fail<E: serde::de::Error>(&self, fault: ArgumentsError) -> E {
        self.fault.set(Some(fault));
        E::custom("refused")
    }

    fn nested<E: serde::de::Error>(&self) -> Result<Self, E> {
        if self.depth >= MAX_ARGUMENT_DEPTH {
            return Err(self.fail(ArgumentsError::TooDeep));
        }
        Ok(Self {
            depth: self.depth + 1,
            fault: self.fault,
        })
    }
}

impl<'de> DeserializeSeed<'de> for Strict<'_> {
    type Value = Value;

    fn deserialize<D: serde::Deserializer<'de>>(self, deserializer: D) -> Result<Value, D::Error> {
        deserializer.deserialize_any(self)
    }
}

impl<'de> Visitor<'de> for Strict<'_> {
    type Value = Value;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a JSON value")
    }

    fn visit_unit<E>(self) -> Result<Value, E> {
        Ok(Value::Null)
    }

    fn visit_bool<E>(self, value: bool) -> Result<Value, E> {
        Ok(Value::Bool(value))
    }

    fn visit_i64<E>(self, value: i64) -> Result<Value, E> {
        Ok(Value::from(value))
    }

    fn visit_u64<E>(self, value: u64) -> Result<Value, E> {
        Ok(Value::from(value))
    }

    fn visit_f64<E: serde::de::Error>(self, value: f64) -> Result<Value, E> {
        serde_json::Number::from_f64(value)
            .map(Value::Number)
            .ok_or_else(|| E::custom("non-finite number"))
    }

    fn visit_str<E>(self, value: &str) -> Result<Value, E> {
        Ok(Value::String(value.to_owned()))
    }

    fn visit_string<E>(self, value: String) -> Result<Value, E> {
        Ok(Value::String(value))
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Value, A::Error> {
        let mut values = Vec::new();
        while let Some(value) = seq.next_element_seed(self.nested()?)? {
            values.push(value);
        }
        Ok(Value::Array(values))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Value, A::Error> {
        let mut object = Map::new();
        while let Some(key) = map.next_key::<String>()? {
            if object.contains_key(&key) {
                return Err(self.fail(ArgumentsError::DuplicateKey));
            }
            let value = map.next_value_seed(self.nested()?)?;
            object.insert(key, value);
        }
        Ok(Value::Object(object))
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    const MAX: u64 = 1 << 20;

    fn call(name: &str, args: &[u8]) -> Vec<u8> {
        Envelope::Call {
            id: CallId::new(7).expect("id"),
            name: ApiName::parse(name).expect("name"),
            args,
        }
        .encode(Direction::FromContext, MAX)
        .expect("encode")
    }

    #[test]
    fn a_call_has_the_documented_layout() {
        let bytes = call("tabs.query", b"[]");
        let mut expected = Vec::new();
        expected.extend_from_slice(&27_u32.to_le_bytes());
        expected.extend_from_slice(&[1, KIND_CALL]);
        expected.extend_from_slice(&7_u32.to_le_bytes());
        expected.push(10);
        expected.extend_from_slice(b"tabs.query");
        expected.extend_from_slice(&2_u32.to_le_bytes());
        expected.extend_from_slice(b"[]");
        assert_eq!(bytes, expected);
        assert_eq!(
            Envelope::decode(&bytes, Direction::FromContext, MAX),
            Ok(Envelope::Call {
                id: CallId::new(7).expect("id"),
                name: ApiName::parse("tabs.query").expect("name"),
                args: b"[]",
            })
        );
    }

    #[test]
    fn the_decoder_rejects_rather_than_guesses() {
        let good = call("tabs.query", b"[]");
        fn decode(bytes: &[u8]) -> Result<Envelope<'_>, DecodeError> {
            Envelope::decode(bytes, Direction::FromContext, MAX)
        }

        assert_eq!(decode(&good[..3]), Err(DecodeError::Truncated));
        assert_eq!(decode(&good[..5]), Err(DecodeError::LengthMismatch));
        let mut extra = good.clone();
        extra.push(0);
        assert_eq!(decode(&extra), Err(DecodeError::LengthMismatch));
        let mut trailing = extra.clone();
        trailing[..4].copy_from_slice(&(extra.len() as u32).to_le_bytes());
        assert_eq!(decode(&trailing), Err(DecodeError::TrailingBytes));
        let mut version = good.clone();
        version[4] = 2;
        assert_eq!(decode(&version), Err(DecodeError::UnsupportedVersion(2)));
        let mut kind = good.clone();
        kind[5] = 99;
        assert_eq!(decode(&kind), Err(DecodeError::UnknownKind(99)));
        let mut zero = good.clone();
        zero[6..10].copy_from_slice(&0_u32.to_le_bytes());
        assert_eq!(decode(&zero), Err(DecodeError::ZeroId));
        assert_eq!(
            Envelope::decode(&good, Direction::FromContext, good.len() as u64 - 1),
            Err(DecodeError::TooLarge)
        );
        assert_eq!(
            Envelope::decode(&good, Direction::ToContext, MAX),
            Err(DecodeError::WrongDirection)
        );
        let mut name = good.clone();
        name[11] = b'-';
        assert_eq!(decode(&name), Err(DecodeError::InvalidName));
        let mut long_args = good;
        let args_at = long_args.len() - 6;
        long_args[args_at..args_at + 4].copy_from_slice(&3_u32.to_le_bytes());
        assert_eq!(decode(&long_args), Err(DecodeError::Truncated));
    }

    #[test]
    fn names_are_dotted_identifiers() {
        for valid in [
            "tabs.query",
            "runtime.onMessage",
            "privacy.services.passwordSavingEnabled.set",
            "i18n.getUILanguage",
        ] {
            assert!(ApiName::parse(valid).is_some(), "{valid}");
        }
        for invalid in [
            "",
            "tabs",
            ".query",
            "tabs.",
            "tabs..query",
            "tabs.1query",
            "tabs.qu ery",
            "tabs.qüery",
            &format!("a.{}", "b".repeat(MAX_API_NAME_BYTES)),
        ] {
            assert!(ApiName::parse(invalid).is_none(), "{invalid}");
        }
    }

    #[test]
    fn contexts_cannot_send_host_frames_or_disconnect_errors() {
        let port = PortId::new(3).expect("port");
        let host_disconnect = Envelope::PortDisconnect {
            port,
            error: Some("Could not establish connection. Receiving end does not exist."),
        };
        assert_eq!(
            host_disconnect.encode(Direction::FromContext, MAX),
            Err(EncodeError::WrongDirection)
        );
        let bytes = host_disconnect
            .encode(Direction::ToContext, MAX)
            .expect("host may");
        assert_eq!(
            Envelope::decode(&bytes, Direction::FromContext, MAX),
            Err(DecodeError::WrongDirection)
        );
        let reply = Envelope::Reply {
            id: CallId::new(1).expect("id"),
            result: Ok(b"null"),
        };
        assert_eq!(
            reply.encode(Direction::FromContext, MAX),
            Err(EncodeError::WrongDirection)
        );
        let event_ack = Envelope::EventAck {
            seq: EventSeq::new(1).expect("seq"),
        };
        assert_eq!(
            event_ack.encode(Direction::ToContext, MAX),
            Err(EncodeError::WrongDirection)
        );
    }

    #[test]
    fn error_text_is_bounded_utf8() {
        let long = "x".repeat(MAX_ERROR_MESSAGE_BYTES + 1);
        let reply = Envelope::Reply {
            id: CallId::new(1).expect("id"),
            result: Err(&long),
        };
        assert_eq!(
            reply.encode(Direction::ToContext, MAX),
            Err(EncodeError::ErrorTooLong)
        );
        let mut bytes = Envelope::Reply {
            id: CallId::new(1).expect("id"),
            result: Err("ab"),
        }
        .encode(Direction::ToContext, MAX)
        .expect("encode");
        let last = bytes.len() - 1;
        bytes[last] = 0xff;
        assert_eq!(
            Envelope::decode(&bytes, Direction::ToContext, MAX),
            Err(DecodeError::InvalidUtf8)
        );
    }

    #[test]
    fn arguments_are_a_bounded_strict_json_array() {
        assert_eq!(
            parse_arguments(b"[1, {\"a\": [true, null]}]").map(|v| v.len()),
            Ok(2)
        );
        assert_eq!(parse_arguments(b"{}"), Err(ArgumentsError::NotAnArray));
        assert_eq!(parse_arguments(b"[1] [2]"), Err(ArgumentsError::Malformed));
        assert_eq!(parse_arguments(b"[1,]"), Err(ArgumentsError::Malformed));
        assert_eq!(parse_arguments(b"\xff"), Err(ArgumentsError::Malformed));
        assert_eq!(
            parse_arguments(b"[{\"a\": 1, \"a\": 2}]"),
            Err(ArgumentsError::DuplicateKey)
        );
        let nested = |depth: usize| {
            let mut text = "[".repeat(depth);
            text.push_str(&"]".repeat(depth));
            text
        };
        assert!(parse_arguments(nested(MAX_ARGUMENT_DEPTH).as_bytes()).is_ok());
        assert_eq!(
            parse_arguments(nested(MAX_ARGUMENT_DEPTH + 1).as_bytes()),
            Err(ArgumentsError::TooDeep)
        );
        assert_eq!(
            parse_arguments(nested(100_000).as_bytes()),
            Err(ArgumentsError::TooDeep)
        );
        assert_eq!(
            parse_arguments(b"[\"duplicate key\"]").map(|v| v.len()),
            Ok(1)
        );
    }

    fn api_name() -> impl Strategy<Value = String> {
        "[a-z][a-zA-Z0-9_]{0,8}\\.[a-z][a-zA-Z0-9_]{0,8}"
    }

    fn envelope_parts() -> impl Strategy<Value = (u8, u64, String, Vec<u8>, Option<String>)> {
        (
            1_u8..=6,
            1_u64..=u64::from(u32::MAX),
            api_name(),
            prop::collection::vec(any::<u8>(), 0..64),
            prop::option::of("[ -~]{0,32}"),
        )
    }

    fn build<'a>(
        kind: u8,
        id: u64,
        name: &'a str,
        bytes: &'a [u8],
        text: &'a Option<String>,
    ) -> (Envelope<'a>, Direction) {
        let call = CallId::new(id as u32).unwrap_or(CallId::new(1).expect("id"));
        let name = ApiName::parse(name).expect("generated name");
        let port = PortId::new(id).expect("nonzero");
        let seq = EventSeq::new(id).expect("nonzero");
        match kind {
            1 => (
                Envelope::Call {
                    id: call,
                    name,
                    args: bytes,
                },
                Direction::FromContext,
            ),
            2 => {
                let result = match text {
                    Some(message) => Err(message.as_str()),
                    None => Ok(bytes),
                };
                (Envelope::Reply { id: call, result }, Direction::ToContext)
            }
            3 => (
                Envelope::Event {
                    seq,
                    name,
                    args: bytes,
                },
                Direction::ToContext,
            ),
            4 => (Envelope::EventAck { seq }, Direction::FromContext),
            5 => (
                Envelope::PortMessage {
                    port,
                    payload: bytes,
                },
                Direction::FromContext,
            ),
            _ => (
                Envelope::PortDisconnect {
                    port,
                    error: text.as_deref(),
                },
                Direction::ToContext,
            ),
        }
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(2048))]

        #[test]
        fn frames_round_trip((kind, id, name, bytes, text) in envelope_parts()) {
            let (envelope, direction) = build(kind, id, &name, &bytes, &text);
            let encoded = envelope.encode(direction, MAX).expect("encode");
            prop_assert_eq!(Envelope::decode(&encoded, direction, MAX), Ok(envelope));
        }

        #[test]
        fn accepted_frames_are_canonical(bytes in prop::collection::vec(any::<u8>(), 0..96)) {
            for direction in [Direction::FromContext, Direction::ToContext] {
                if let Ok(envelope) = Envelope::decode(&bytes, direction, MAX) {
                    prop_assert_eq!(envelope.encode(direction, MAX), Ok(bytes.clone()));
                }
            }
        }

        #[test]
        fn mutated_frames_never_panic(
            (kind, id, name, bytes, text) in envelope_parts(),
            flips in prop::collection::vec((any::<prop::sample::Index>(), any::<u8>()), 1..4),
        ) {
            let (envelope, direction) = build(kind, id, &name, &bytes, &text);
            let mut encoded = envelope.encode(direction, MAX).expect("encode");
            for (index, value) in flips {
                let at = index.index(encoded.len());
                encoded[at] = value;
            }
            if let Ok(decoded) = Envelope::decode(&encoded, direction, MAX) {
                prop_assert_eq!(decoded.encode(direction, MAX), Ok(encoded.clone()));
            }
        }

        #[test]
        fn argument_parsing_never_panics(bytes in prop::collection::vec(any::<u8>(), 0..128)) {
            let _ = parse_arguments(&bytes);
        }
    }
}
