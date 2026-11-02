//! WebSockets for extension service workers.
//!
//! WebKit runs extension workers on the web process's main thread, and a
//! WebSocket opened from one deadlocks the worker for good. The compatibility
//! layer replaces the worker's `WebSocket` with one that talks to this bridge
//! over a native-messaging port; the connection itself runs in URLSession.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::{Rc, Weak as RcWeak};

use base64::Engine as _;
use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject, ProtocolObject};
use objc2::{define_class, msg_send, AllocAnyThread, DefinedClass, Message};
use objc2_foundation::{
    NSData, NSError, NSMutableURLRequest, NSObjectProtocol, NSOperationQueue, NSString,
    NSURLSession, NSURLSessionConfiguration, NSURLSessionDelegate, NSURLSessionTask,
    NSURLSessionTaskDelegate, NSURLSessionWebSocketCloseCode, NSURLSessionWebSocketDelegate,
    NSURLSessionWebSocketMessage, NSURLSessionWebSocketMessageType, NSURLSessionWebSocketTask,
    NSURL,
};
use objc2_web_kit::{WKWebExtensionContext, WKWebExtensionMessagePort};
use serde_json::{json, Value};

use crate::runtime::Shared;
use crate::{json, LogLevel};

pub(crate) const APPLICATION: &str = "app.zephium.socket";

thread_local! {
    static SOCKETS: RefCell<HashMap<usize, Rc<Socket>>> = RefCell::new(HashMap::new());
}

struct Socket {
    key: usize,
    extension: String,
    owner: RcWeak<Shared>,
    port: Retained<WKWebExtensionMessagePort>,
    session: RefCell<Option<Retained<NSURLSession>>>,
    task: RefCell<Option<Retained<NSURLSessionWebSocketTask>>>,
    delegate: RefCell<Option<Retained<SessionDelegate>>>,
    closed: Cell<bool>,
}

pub(crate) fn connect(
    shared: &Rc<Shared>,
    context: &WKWebExtensionContext,
    port: &WKWebExtensionMessagePort,
) {
    let socket = Rc::new(Socket {
        key: port as *const _ as usize,
        extension: unsafe { context.uniqueIdentifier() }.to_string(),
        owner: Rc::downgrade(shared),
        port: port.retain(),
        session: RefCell::new(None),
        task: RefCell::new(None),
        delegate: RefCell::new(None),
        closed: Cell::new(false),
    });
    let weak = Rc::downgrade(&socket);
    let shared_weak = Rc::downgrade(shared);
    let on_message = RcBlock::new(move |message: *mut AnyObject, _error: *mut NSError| {
        let Some(socket) = weak.upgrade() else { return };
        let message = json::from_object(unsafe { message.as_ref() });
        socket.receive(&shared_weak, message);
    });
    let weak = Rc::downgrade(&socket);
    let on_disconnect = RcBlock::new(move |_error: *mut NSError| {
        if let Some(socket) = weak.upgrade() {
            socket.shut(None);
        }
    });
    unsafe {
        port.setMessageHandler(Some(&on_message));
        port.setDisconnectHandler(Some(&on_disconnect));
    }
    SOCKETS.with(|sockets| sockets.borrow_mut().insert(socket.key, socket));
}

impl Socket {
    fn receive(self: &Rc<Self>, shared: &RcWeak<Shared>, message: Value) {
        match message.get("op").and_then(Value::as_str) {
            // WebKit drops what either side posts on a port that has only just
            // opened, so the page repeats "hello" until it hears one back.
            Some("hello") => self.post(json!({ "op": "hello" })),
            Some("open") if self.task.borrow().is_none() => self.open(shared, &message),
            Some("send") => self.send(&message),
            Some("close") => {
                let code = message.get("code").and_then(Value::as_i64).unwrap_or(1000);
                let reason = message.get("reason").and_then(Value::as_str).unwrap_or("");
                if let Some(task) = self.task.borrow().as_ref() {
                    let reason = NSData::with_bytes(reason.as_bytes());
                    task.cancelWithCloseCode_reason(
                        NSURLSessionWebSocketCloseCode(code as isize),
                        Some(&reason),
                    );
                }
            }
            _ => {}
        }
    }

    fn open(self: &Rc<Self>, shared: &RcWeak<Shared>, message: &Value) {
        let url = message
            .get("url")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let Some(url) = NSURL::URLWithString(&NSString::from_str(url)).filter(|url| {
            matches!(
                url.scheme().map(|s| s.to_string()).as_deref(),
                Some("ws" | "wss")
            )
        }) else {
            self.post(json!({ "op": "error", "message": "invalid WebSocket URL" }));
            return self.shut(Some((1006, "")));
        };
        let request = NSMutableURLRequest::requestWithURL(&url);
        let origin = format!("{}://{}", crate::runtime::SCHEME, self.extension);
        request.setValue_forHTTPHeaderField(
            Some(&NSString::from_str(&origin)),
            &NSString::from_str("Origin"),
        );
        if let Some(agent) = message.get("userAgent").and_then(Value::as_str) {
            request.setValue_forHTTPHeaderField(
                Some(&NSString::from_str(agent)),
                &NSString::from_str("User-Agent"),
            );
        }
        let protocols: Vec<&str> = message
            .get("protocols")
            .and_then(Value::as_array)
            .map(|items| items.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        if !protocols.is_empty() {
            request.setValue_forHTTPHeaderField(
                Some(&NSString::from_str(&protocols.join(", "))),
                &NSString::from_str("Sec-WebSocket-Protocol"),
            );
        }

        let delegate = SessionDelegate::new(self.key);
        let configuration = network_configuration();
        let session = unsafe {
            NSURLSession::sessionWithConfiguration_delegate_delegateQueue(
                &configuration,
                Some(ProtocolObject::from_ref(&*delegate)),
                Some(&NSOperationQueue::mainQueue()),
            )
        };
        let task = session.webSocketTaskWithRequest(&request);
        *self.delegate.borrow_mut() = Some(delegate);
        *self.session.borrow_mut() = Some(session);
        *self.task.borrow_mut() = Some(task.clone());
        task.resume();
        self.receive_next();
        if let Some(shared) = shared.upgrade() {
            shared.host().log(
                &self.extension,
                LogLevel::Info,
                &format!("worker WebSocket bridged to {}", url_text(&url)),
            );
        }
    }

    fn receive_next(self: &Rc<Self>) {
        let Some(task) = self.task.borrow().clone() else {
            return;
        };
        let weak = Rc::downgrade(self);
        let handler = RcBlock::new(
            move |message: *mut NSURLSessionWebSocketMessage, _error: *mut NSError| {
                let Some(socket) = weak.upgrade() else { return };
                let Some(message) = (unsafe { message.as_ref() }) else {
                    if crate::tracing() {
                        eprintln!("webext-trace: socket receive ended");
                    }
                    return;
                };
                let payload = if message.r#type() == NSURLSessionWebSocketMessageType::String {
                    json!({ "op": "message", "text": message.string().map(|s| s.to_string()).unwrap_or_default() })
                } else {
                    let bytes = message.data().map(|data| data.to_vec()).unwrap_or_default();
                    json!({ "op": "message", "base64": base64::engine::general_purpose::STANDARD.encode(bytes) })
                };
                socket.post(payload);
                socket.receive_next();
            },
        );
        unsafe { task.receiveMessageWithCompletionHandler(&handler) };
    }

    fn send(&self, message: &Value) {
        let Some(task) = self.task.borrow().clone() else {
            return;
        };
        let payload = if let Some(text) = message.get("text").and_then(Value::as_str) {
            NSURLSessionWebSocketMessage::initWithString(
                NSURLSessionWebSocketMessage::alloc(),
                &NSString::from_str(text),
            )
        } else if let Some(encoded) = message.get("base64").and_then(Value::as_str) {
            let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(encoded) else {
                return;
            };
            NSURLSessionWebSocketMessage::initWithData(
                NSURLSessionWebSocketMessage::alloc(),
                &NSData::with_bytes(&bytes),
            )
        } else {
            return;
        };
        let done = RcBlock::new(|_error: *mut NSError| {});
        unsafe { task.sendMessage_completionHandler(&payload, &done) };
    }

    fn post(&self, message: Value) {
        if self.closed.get() && message["op"] != "close" {
            return;
        }
        let object = json::to_object(&message);
        unsafe { self.port.sendMessage_completionHandler(Some(&object), None) };
    }

    /// Reports the close to the page (once) and releases everything.
    fn shut(&self, close: Option<(i64, &str)>) {
        if self.closed.replace(true) {
            return;
        }
        if let Some((code, reason)) = close {
            let object = json::to_object(&json!({ "op": "close", "code": code, "reason": reason }));
            unsafe { self.port.sendMessage_completionHandler(Some(&object), None) };
        }
        if let Some(task) = self.task.borrow_mut().take() {
            task.cancel();
        }
        if let Some(session) = self.session.borrow_mut().take() {
            session.invalidateAndCancel();
        }
        unsafe { self.port.disconnect() };
        let key = self.key;
        SOCKETS.with(|sockets| sockets.borrow_mut().remove(&key));
    }
}

/// Closes every WebSocket an extension's worker opened in one profile.
pub(crate) fn close_extension(shared: &Shared, extension: &str) {
    let open: Vec<_> = SOCKETS.with(|sockets| {
        sockets
            .borrow()
            .values()
            .filter(|socket| {
                socket.extension == extension && std::ptr::eq(socket.owner.as_ptr(), shared)
            })
            .cloned()
            .collect()
    });
    for socket in open {
        socket.shut(None);
    }
}

fn url_text(url: &NSURL) -> String {
    let host = url.host().map(|host| host.to_string()).unwrap_or_default();
    let scheme = url
        .scheme()
        .map(|scheme| scheme.to_string())
        .unwrap_or_default();
    format!("{scheme}://{host}")
}

// URLSession requires a delegate usable from any thread; it is only ever
// called on the main queue, where the socket table lives.
pub(crate) struct DelegateIvars {
    key: usize,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[name = "ZephiumWebExtSocketDelegate"]
    #[ivars = DelegateIvars]
    struct SessionDelegate;

    unsafe impl NSObjectProtocol for SessionDelegate {}
    unsafe impl NSURLSessionDelegate for SessionDelegate {}

    unsafe impl NSURLSessionTaskDelegate for SessionDelegate {
        #[unsafe(method(URLSession:task:didCompleteWithError:))]
        fn did_complete(
            &self,
            _session: &NSURLSession,
            _task: &NSURLSessionTask,
            error: Option<&NSError>,
        ) {
            if let Some(socket) = self.socket() {
                if let Some(error) = error {
                    socket.post(json!({ "op": "error", "message": crate::describe_error(error) }));
                }
                socket.shut(Some((1006, "")));
            }
        }
    }

    unsafe impl NSURLSessionWebSocketDelegate for SessionDelegate {
        #[unsafe(method(URLSession:webSocketTask:didOpenWithProtocol:))]
        fn did_open(
            &self,
            _session: &NSURLSession,
            _task: &NSURLSessionWebSocketTask,
            protocol: Option<&NSString>,
        ) {
            if let Some(socket) = self.socket() {
                let protocol = protocol.map(|p| p.to_string()).unwrap_or_default();
                socket.post(json!({ "op": "open", "protocol": protocol }));
            }
        }

        #[unsafe(method(URLSession:webSocketTask:didCloseWithCode:reason:))]
        fn did_close(
            &self,
            _session: &NSURLSession,
            _task: &NSURLSessionWebSocketTask,
            code: NSURLSessionWebSocketCloseCode,
            reason: Option<&NSData>,
        ) {
            if let Some(socket) = self.socket() {
                let reason = reason
                    .map(|data| String::from_utf8_lossy(&data.to_vec()).into_owned())
                    .unwrap_or_default();
                socket.shut(Some((code.0 as i64, &reason)));
            }
        }
    }
);

impl SessionDelegate {
    fn new(key: usize) -> Retained<Self> {
        let this = Self::alloc().set_ivars(DelegateIvars { key });
        unsafe { msg_send![super(this), init] }
    }

    fn socket(&self) -> Option<Rc<Socket>> {
        let key = self.ivars().key;
        SOCKETS
            .try_with(|sockets| sockets.borrow().get(&key).cloned())
            .ok()
            .flatten()
    }
}

/// Configuration for one broker socket.
fn network_configuration() -> Retained<NSURLSessionConfiguration> {
    // The compatibility bridge has no profile cookie or credential API.
    // Do not borrow Foundation's process-wide persistent network state. Each
    // socket is stateless even relative to another extension in this profile.
    let configuration = NSURLSessionConfiguration::ephemeralSessionConfiguration();
    configuration.setHTTPShouldSetCookies(false);
    configuration.setHTTPCookieStorage(None);
    configuration.setURLCredentialStorage(None);
    configuration.setURLCache(None);
    configuration
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn broker_sockets_have_no_ambient_network_stores() {
        for _principal in [
            "profile-a/extension-a",
            "profile-b/extension-a",
            "profile-a/extension-b",
        ] {
            let configuration = network_configuration();
            assert!(!configuration.HTTPShouldSetCookies());
            assert!(configuration.HTTPCookieStorage().is_none());
            assert!(configuration.URLCredentialStorage().is_none());
            assert!(configuration.URLCache().is_none());
        }
    }
}
