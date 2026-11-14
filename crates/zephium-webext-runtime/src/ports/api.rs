use serde_json::{Map, Value};

use crate::context::{ContextKind, ContextMask, ExtensionId};
use crate::dispatch::{ApiTable, Handled, Invocation, MethodSpec, RegistrationError};
use crate::errors::ApiError;
use crate::permissions::Gate;

/// Validated wire target. The adapter must resolve numeric ids to native tab
/// identities and check external-connectability before creating a `MessageRoute`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MessageTarget {
    /// Extension messaging; absent id means the caller's extension.
    Runtime {
        /// Explicit receiver package, if supplied.
        extension_id: Option<ExtensionId>,
    },
    /// Content scripts within a Chrome tab id.
    Tab {
        /// Chrome tab id from the argument, still requiring host resolution.
        tab_id: i64,
        /// A particular frame, or all frames.
        frame_id: Option<u64>,
        /// Adapter-owned Chrome document id, or the current document.
        document_id: Option<String>,
    },
}

/// A validated messaging API request, ready for adapter routing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MessageRequest {
    /// Begin an asynchronous port handshake; settle only with its allocated id.
    Connect {
        /// Requested receiving contexts.
        target: MessageTarget,
        /// Optional connection name, defaulting to the empty string.
        name: String,
    },
    /// Begin a one-shot request; settle its invocation with the final response.
    Send {
        /// Requested receiving contexts.
        target: MessageTarget,
        /// JSON serialization, relayed as opaque bytes by the service.
        payload: Vec<u8>,
    },
    /// Begin native-host discovery and a persistent native port handshake.
    ConnectNative {
        /// Validated native-host name.
        host: String,
    },
    /// Start a native host for one response, then close/reap it.
    SendNative {
        /// Validated native-host name.
        host: String,
        /// JSON serialization for the native protocol.
        payload: Vec<u8>,
    },
}

/// Host routing and deferred settlement seam for all messaging APIs.
pub trait MessagingHost {
    /// Resolves a validated request using authenticated invocation identity.
    /// Open ports and one-shot calls settle through observed native outcomes.
    fn message_request(&mut self, request: MessageRequest, invocation: Invocation<'_>) -> Handled;
}

/// Handwritten validator behind the shared replaceable schema-validation seam.
#[derive(Clone, Copy, Debug, Default)]
pub struct MessagingValidator;

impl crate::api::ArgumentValidator for MessagingValidator {
    fn validate(&self, method: &str, args: &[Value]) -> Result<(), ApiError> {
        parse_request(method, args).map(|_| ())
    }
}

/// Registers the implemented messaging methods with context and permission gates.
pub fn register<S: MessagingHost + 'static>(
    table: &mut ApiTable<S>,
) -> Result<(), RegistrationError> {
    for (name, gate, contexts) in [
        ("runtime.connect", Gate::Open, ContextMask::ALL),
        ("runtime.sendMessage", Gate::Open, ContextMask::ALL),
        ("tabs.connect", Gate::Open, ContextMask::EXTENSION_PAGES),
        ("tabs.sendMessage", Gate::Open, ContextMask::EXTENSION_PAGES),
        (
            "runtime.connectNative",
            Gate::Requires("nativeMessaging"),
            ContextMask::PRIVILEGED_PAGES,
        ),
        (
            "runtime.sendNativeMessage",
            Gate::Requires("nativeMessaging"),
            ContextMask::PRIVILEGED_PAGES,
        ),
    ] {
        table.register(
            name,
            MethodSpec::new(
                gate,
                contexts,
                move |host: &mut S, invocation| match parse_request(name, invocation.args()) {
                    Ok(request) => {
                        if invocation.sender().kind() == ContextKind::ExternalPage
                            && !explicit_target(&request, invocation.sender().extension_id())
                        {
                            invocation.reply(Err(ApiError::malformed_arguments()))
                        } else {
                            host.message_request(request, invocation)
                        }
                    }
                    Err(error) => invocation.reply(Err(error)),
                },
            ),
        )?;
    }
    Ok(())
}

fn explicit_target(request: &MessageRequest, bound: &ExtensionId) -> bool {
    matches!(request, MessageRequest::Connect { target: MessageTarget::Runtime { extension_id: Some(target) }, .. }
        | MessageRequest::Send { target: MessageTarget::Runtime { extension_id: Some(target) }, .. } if target == bound)
}

/// Checks Chrome overloads and options before host delegation. Generated schema
/// validation may run before this parser; services do not inspect relay payloads.
pub fn parse_request(method: &str, args: &[Value]) -> Result<MessageRequest, ApiError> {
    let bad = || ApiError::no_matching_signature(method, signature(method));
    match method {
        "runtime.connect" => {
            let (extension_id, info) = match args {
                [] => (None, None),
                [Value::String(id)] => (Some(extension(id).ok_or_else(bad)?), None),
                [info] => (None, Some(info)),
                [Value::String(id), info] => (Some(extension(id).ok_or_else(bad)?), Some(info)),
                _ => return Err(bad()),
            };
            let name = connect_options(info, false).map_err(|_| bad())?.0;
            Ok(MessageRequest::Connect {
                target: MessageTarget::Runtime { extension_id },
                name,
            })
        }
        "runtime.sendMessage" => {
            let (extension_id, message) = match args {
                [message] => (None, message),
                [message, options] if runtime_options(options) => (None, message),
                [Value::String(id), message] => (Some(extension(id).ok_or_else(bad)?), message),
                [Value::String(id), message, options] if runtime_options(options) => {
                    (Some(extension(id).ok_or_else(bad)?), message)
                }
                _ => return Err(bad()),
            };
            Ok(MessageRequest::Send {
                target: MessageTarget::Runtime { extension_id },
                payload: serialize(message)?,
            })
        }
        "tabs.connect" => {
            let (tab, info) = match args {
                [tab] => (tab, None),
                [tab, info] => (tab, Some(info)),
                _ => return Err(bad()),
            };
            let tab_id = chrome_tab(tab).ok_or_else(bad)?;
            let (name, frame_id, document_id) = connect_options(info, true).map_err(|_| bad())?;
            Ok(MessageRequest::Connect {
                target: MessageTarget::Tab {
                    tab_id,
                    frame_id,
                    document_id,
                },
                name,
            })
        }
        "tabs.sendMessage" => {
            let (tab, message, options) = match args {
                [tab, message] => (tab, message, None),
                [tab, message, options] => (tab, message, Some(options)),
                _ => return Err(bad()),
            };
            let tab_id = chrome_tab(tab).ok_or_else(bad)?;
            let (_, frame_id, document_id) = tab_options(options).map_err(|_| bad())?;
            Ok(MessageRequest::Send {
                target: MessageTarget::Tab {
                    tab_id,
                    frame_id,
                    document_id,
                },
                payload: serialize(message)?,
            })
        }
        "runtime.connectNative" => match args {
            [Value::String(host)] if super::valid_native_host(host) => {
                Ok(MessageRequest::ConnectNative { host: host.clone() })
            }
            _ => Err(bad()),
        },
        "runtime.sendNativeMessage" => match args {
            [Value::String(host), message] if super::valid_native_host(host) => {
                Ok(MessageRequest::SendNative {
                    host: host.clone(),
                    payload: serialize(message)?,
                })
            }
            _ => Err(bad()),
        },
        _ => Err(ApiError::unknown_method()),
    }
}

fn signature(method: &str) -> &'static str {
    match method {
        "runtime.connect" => "optional string extensionId, optional object connectInfo",
        "runtime.sendMessage" => "optional string extensionId, any message, optional object options, optional function callback",
        "tabs.connect" => "integer tabId, optional object connectInfo",
        "tabs.sendMessage" => "integer tabId, any message, optional object options, optional function callback",
        "runtime.connectNative" => "string application",
        "runtime.sendNativeMessage" => "string application, any message, optional function callback",
        _ => "",
    }
}

fn extension(value: &str) -> Option<ExtensionId> {
    ExtensionId::parse(value)
}
fn chrome_tab(value: &Value) -> Option<i64> {
    value
        .as_i64()
        .filter(|id| (0..=i64::from(i32::MAX)).contains(id))
}

fn runtime_options(value: &Value) -> bool {
    value.is_null()
        || value.as_object().is_some_and(|options| {
            options
                .iter()
                .all(|(key, value)| key == "includeTlsChannelId" && value.is_boolean())
        })
}

type Options = (String, Option<u64>, Option<String>);

fn connect_options(value: Option<&Value>, tab: bool) -> Result<Options, ()> {
    let Some(value) = value.filter(|v| !v.is_null()) else {
        return Ok((String::new(), None, None));
    };
    let options = value.as_object().ok_or(())?;
    let allowed = if tab {
        &["name", "frameId", "documentId"][..]
    } else {
        &["name", "includeTlsChannelId"][..]
    };
    if !options.keys().all(|key| allowed.contains(&key.as_str())) {
        return Err(());
    }
    let name = options
        .get("name")
        .map(|v| {
            v.as_str()
                .filter(|s| s.len() <= 1_024)
                .map(str::to_owned)
                .ok_or(())
        })
        .transpose()?
        .unwrap_or_default();
    if options
        .get("includeTlsChannelId")
        .is_some_and(|v| !v.is_boolean())
    {
        return Err(());
    }
    let (frame, document) = target_options(options)?;
    Ok((name, frame, document))
}

fn tab_options(value: Option<&Value>) -> Result<Options, ()> {
    let Some(value) = value.filter(|v| !v.is_null()) else {
        return Ok((String::new(), None, None));
    };
    let options = value.as_object().ok_or(())?;
    if !options
        .keys()
        .all(|key| matches!(key.as_str(), "frameId" | "documentId"))
    {
        return Err(());
    }
    let (frame, document) = target_options(options)?;
    Ok((String::new(), frame, document))
}

fn target_options(options: &Map<String, Value>) -> Result<(Option<u64>, Option<String>), ()> {
    let frame = options
        .get("frameId")
        .map(|v| v.as_u64().filter(|id| *id <= i32::MAX as u64).ok_or(()))
        .transpose()?;
    let document = options
        .get("documentId")
        .map(|v| {
            v.as_str()
                .filter(|s| !s.is_empty() && s.len() <= 128 && !s.chars().any(char::is_control))
                .map(str::to_owned)
                .ok_or(())
        })
        .transpose()?;
    Ok((frame, document))
}

fn serialize(value: &Value) -> Result<Vec<u8>, ApiError> {
    serde_json::to_vec(value).map_err(|_| ApiError::other("Could not serialize message."))
}
