use serde_json::Value;

use crate::context::{ContextKind, ExtensionId};
use crate::dispatch::{ApiTable, Handled, Invocation, RegistrationError};
use crate::errors::ApiError;

/// Validated wire target. The adapter must resolve numeric ids to native tab
/// identities and check external-connectability before creating a `MessageRoute`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MessageTarget {
    /// Extension messaging; absent id means the caller's extension.
    Runtime {
        /// Explicit receiver package, if supplied.
        extension_id: Option<ExtensionId>,
    },
    /// Resolved, permission-checked exact document routes; produced by registration.
    ResolvedTab {
        /// One route per current authorized frame, each fenced by document generation.
        routes: Vec<super::MessageRoute>,
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

/// Registers messaging against the generated surface and Zephium policy.
pub fn register<S: MessagingHost + crate::browser::BrowserPort + 'static>(
    table: &mut ApiTable<S>,
) -> Result<(), RegistrationError> {
    for name in [
        "runtime.connect",
        "runtime.sendMessage",
        "tabs.connect",
        "tabs.sendMessage",
        "runtime.connectNative",
        "runtime.sendNativeMessage",
    ] {
        table.register_generated(
            name,
            move |host: &mut S, invocation| match normalized_request(name, invocation.args()) {
                Ok(request) => {
                    if invocation.sender().kind() == ContextKind::ExternalPage
                        && !explicit_target(&request, invocation.sender().extension_id())
                    {
                        invocation.reply(Err(ApiError::malformed_arguments()))
                    } else {
                        match resolve_tab_request(
                            host,
                            invocation.sender(),
                            invocation.permissions(),
                            request,
                        ) {
                            Ok(request) => host.message_request(request, invocation),
                            Err(error) => invocation.reply(Err(error)),
                        }
                    }
                }
                Err(error) => invocation.reply(Err(error)),
            },
        )?;
    }
    Ok(())
}

/// Resolves tab-side frame/document targeting using the ordinary browser graph.
/// Adapters must recheck these exact routes immediately before native delivery.
pub fn resolve_tab_request(
    host: &impl crate::browser::BrowserPort,
    sender: &crate::Sender,
    permissions: &crate::InstallPermissions,
    mut request: MessageRequest,
) -> Result<MessageRequest, ApiError> {
    let target = match &mut request {
        MessageRequest::Connect { target, .. } | MessageRequest::Send { target, .. } => target,
        _ => return Ok(request),
    };
    let MessageTarget::Tab {
        tab_id,
        frame_id,
        document_id,
    } = target
    else {
        return Ok(request);
    };
    let snapshot = crate::browser::snapshot(host, sender)?;
    let tab = snapshot.tab(i32::try_from(*tab_id).map_err(|_| super::no_receiver())?)?;
    if tab.tab.discarded() {
        return Err(super::no_receiver());
    }
    let frames = crate::api::web_navigation::frames(host, sender, tab)?;
    let explicit = frame_id.is_some() || document_id.is_some();
    let mut routes = Vec::new();
    for frame in frames {
        if frame.tab != tab.tab.id()
            || frame_id.is_some_and(|id| frame.frame.get() != id)
            || document_id
                .as_ref()
                .is_some_and(|id| &frame.document_id != id)
        {
            continue;
        }
        if frame.document_lifecycle != "active" {
            if explicit {
                return Err(super::no_receiver());
            }
            continue;
        }
        let access = permissions.check_target(
            sender,
            &crate::Target {
                profile: snapshot.profile,
                tab: tab.tab.id(),
                frame: frame.frame,
                main_document: tab.document,
                url: &frame.url,
                origin: &frame.origin,
            },
        );
        if let Err(error) = access {
            if explicit {
                return Err(error);
            }
            continue;
        }
        routes.push(super::MessageRoute::Tab {
            tab: tab.tab.id(),
            frame: Some(frame.frame),
            document: Some(frame.document),
        });
    }
    if routes.is_empty() {
        return Err(super::no_receiver());
    }
    *target = MessageTarget::ResolvedTab { routes };
    Ok(request)
}

fn explicit_target(request: &MessageRequest, bound: &ExtensionId) -> bool {
    matches!(request, MessageRequest::Connect { target: MessageTarget::Runtime { extension_id: Some(target) }, .. }
        | MessageRequest::Send { target: MessageTarget::Runtime { extension_id: Some(target) }, .. } if target == bound)
}

/// Validates Chrome overloads, then resolves service-specific target identities.
pub fn parse_request(method: &str, args: &[Value]) -> Result<MessageRequest, ApiError> {
    normalized_request(method, &crate::schema::normalize(method, args)?)
}

fn normalized_request(method: &str, args: &[Value]) -> Result<MessageRequest, ApiError> {
    let bad = ApiError::malformed_arguments;
    match method {
        "runtime.connect" | "runtime.sendMessage" => {
            let extension_id = args[0]
                .as_str()
                .map(|id| ExtensionId::parse(id).ok_or_else(bad))
                .transpose()?;
            let target = MessageTarget::Runtime { extension_id };
            if method == "runtime.connect" {
                Ok(MessageRequest::Connect {
                    target,
                    name: connection_name(&args[1])?,
                })
            } else {
                Ok(MessageRequest::Send {
                    target,
                    payload: serialize(&args[1])?,
                })
            }
        }
        "tabs.connect" | "tabs.sendMessage" => {
            let options = &args[if method == "tabs.connect" { 1 } else { 2 }];
            let document_id = options
                .get("documentId")
                .and_then(Value::as_str)
                .map(|id| {
                    if id.is_empty() || id.len() > 128 || id.chars().any(char::is_control) {
                        Err(bad())
                    } else {
                        Ok(id.to_owned())
                    }
                })
                .transpose()?;
            let target = MessageTarget::Tab {
                tab_id: args[0].as_i64().ok_or_else(bad)?,
                frame_id: options.get("frameId").and_then(Value::as_u64),
                document_id,
            };
            if method == "tabs.connect" {
                Ok(MessageRequest::Connect {
                    target,
                    name: connection_name(options)?,
                })
            } else {
                Ok(MessageRequest::Send {
                    target,
                    payload: serialize(&args[1])?,
                })
            }
        }
        "runtime.connectNative" | "runtime.sendNativeMessage" => {
            let host = args[0]
                .as_str()
                .filter(|host| super::valid_native_host(host))
                .ok_or_else(bad)?
                .to_owned();
            if method == "runtime.connectNative" {
                Ok(MessageRequest::ConnectNative { host })
            } else {
                Ok(MessageRequest::SendNative {
                    host,
                    payload: serialize(&args[1])?,
                })
            }
        }
        _ => Err(ApiError::unknown_method()),
    }
}

fn connection_name(options: &Value) -> Result<String, ApiError> {
    let name = options
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if name.len() > 1_024 {
        return Err(ApiError::malformed_arguments());
    }
    Ok(name.to_owned())
}

fn serialize(value: &Value) -> Result<Vec<u8>, ApiError> {
    serde_json::to_vec(value).map_err(|_| ApiError::other("Could not serialize message."))
}
