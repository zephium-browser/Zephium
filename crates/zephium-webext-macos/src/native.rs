//! Native messaging: extensions talking to desktop applications.
//!
//! Hosts are found the way Chrome finds them, through the manifests desktop
//! applications register for Chrome and other Chromium browsers, and speak
//! Chrome's stdio protocol: JSON messages, each prefixed with its length as a
//! native-endian `u32`. A host runs until its port closes; a one-off message
//! gets a host of its own that ends after the first reply.

use std::cell::RefCell;
use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::rc::{Rc, Weak as RcWeak};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

use block2::{DynBlock, RcBlock};
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::Message;
use objc2_foundation::{NSError, NSString};
use objc2_web_kit::{WKWebExtensionContext, WKWebExtensionMessagePort};
use serde_json::Value;

use crate::runtime::Shared;
use crate::{json, LogLevel};

/// Chrome's limit for a message from a host.
const MAX_FROM_HOST: usize = 1024 * 1024;
const MAX_TO_HOST: usize = 64 * 1024 * 1024;
const MAX_MANIFEST: u64 = 64 * 1024;
const MAX_HOSTS_PER_EXTENSION: usize = 8;
/// How long a host may take to exit once its input closes before it is
/// killed.
const EXIT_GRACE: Duration = Duration::from_secs(2);

type Reply = RcBlock<dyn Fn(*mut AnyObject, *mut NSError)>;

thread_local! {
    static CONNECTIONS: RefCell<HashMap<usize, Rc<Connection>>> = RefCell::new(HashMap::new());
}

static NEXT_KEY: AtomicUsize = AtomicUsize::new(1);

enum Event {
    Message(Value),
    Closed(Option<String>),
}

struct Connection {
    key: usize,
    extension: String,
    host: String,
    port: Option<Retained<WKWebExtensionMessagePort>>,
    reply: RefCell<Option<Reply>>,
    outbound: RefCell<Option<mpsc::Sender<Vec<u8>>>>,
    shared: RcWeak<Shared>,
}

/// Opens a long-lived connection for `runtime.connectNative`.
pub(crate) fn connect(
    shared: &Rc<Shared>,
    context: &WKWebExtensionContext,
    port: &WKWebExtensionMessagePort,
    host: &str,
) -> Result<(), String> {
    let connection = open(shared, context, host, Some(port.retain()), None)?;
    let weak = Rc::downgrade(&connection);
    let on_message = RcBlock::new(move |message: *mut AnyObject, _error: *mut NSError| {
        if let Some(connection) = weak.upgrade() {
            connection.forward(&json::from_object(unsafe { message.as_ref() }));
        }
    });
    let weak = Rc::downgrade(&connection);
    let on_disconnect = RcBlock::new(move |_error: *mut NSError| {
        if let Some(connection) = weak.upgrade() {
            connection.close(None);
        }
    });
    unsafe {
        port.setMessageHandler(Some(&on_message));
        port.setDisconnectHandler(Some(&on_disconnect));
    }
    Ok(())
}

/// Sends one message for `runtime.sendNativeMessage` and replies with the
/// host's first answer.
pub(crate) fn send(
    shared: &Rc<Shared>,
    context: &WKWebExtensionContext,
    host: &str,
    message: &AnyObject,
    reply: &DynBlock<dyn Fn(*mut AnyObject, *mut NSError)>,
) -> Result<(), String> {
    let connection = open(shared, context, host, None, Some(reply.copy()))?;
    connection.forward(&json::from_object(Some(message)));
    Ok(())
}

fn open(
    shared: &Rc<Shared>,
    context: &WKWebExtensionContext,
    host: &str,
    port: Option<Retained<WKWebExtensionMessagePort>>,
    reply: Option<Reply>,
) -> Result<Rc<Connection>, String> {
    // The browser's own bridges never reach this point; outside hosts are
    // only for extensions that declared them, as the install review showed.
    let declared = unsafe { context.webExtension().requestedPermissions() }
        .containsObject(&NSString::from_str("nativeMessaging"));
    if !declared {
        return Err("Access to native messaging requires nativeMessaging permission.".into());
    }
    let extension = unsafe { context.uniqueIdentifier() }.to_string();
    let running = CONNECTIONS.with(|connections| {
        connections
            .borrow()
            .values()
            .filter(|connection| connection.belongs_to(shared, &extension))
            .count()
    });
    if running >= MAX_HOSTS_PER_EXTENSION {
        return Err("Too many native messaging hosts are running.".into());
    }
    let path = find_host(&search_roots(), host, &extension)?;
    let key = NEXT_KEY.fetch_add(1, Ordering::Relaxed);
    let outbound = spawn(&path, &extension, key).map_err(|error| {
        shared.host().log(
            &extension,
            LogLevel::Warning,
            &format!("native messaging host {host} failed to start: {error}"),
        );
        "Native host has exited.".to_string()
    })?;
    shared.host().log(
        &extension,
        LogLevel::Info,
        &format!("native messaging host {host} started"),
    );
    let connection = Rc::new(Connection {
        key,
        extension,
        host: host.to_string(),
        port,
        reply: RefCell::new(reply),
        outbound: RefCell::new(Some(outbound)),
        shared: Rc::downgrade(shared),
    });
    CONNECTIONS.with(|connections| connections.borrow_mut().insert(key, connection.clone()));
    Ok(connection)
}

impl Connection {
    fn belongs_to(&self, shared: &Shared, extension: &str) -> bool {
        self.extension == extension && std::ptr::eq(self.shared.as_ptr(), shared)
    }

    fn forward(&self, message: &Value) {
        let Ok(frame) = serde_json::to_vec(message) else {
            return;
        };
        if frame.len() > MAX_TO_HOST {
            return self.close(Some("Message exceeded maximum allowed size.".into()));
        }
        let sent = self
            .outbound
            .borrow()
            .as_ref()
            .is_some_and(|outbound| outbound.send(frame).is_ok());
        if !sent {
            self.close(Some("Native host has exited.".into()));
        }
    }

    fn post(&self, message: &Value) {
        if let Some(port) = &self.port {
            let object = json::to_object(message);
            unsafe { port.sendMessage_completionHandler(Some(&object), None) };
        }
    }

    fn receive(&self, event: Event) {
        match event {
            Event::Message(message) => {
                if let Some(reply) = self.reply.borrow_mut().take() {
                    let object = json::to_object(&message);
                    reply.call((Retained::as_ptr(&object).cast_mut(), std::ptr::null_mut()));
                    return self.close(None);
                }
                self.post(&message);
            }
            Event::Closed(error) => self.close(error),
        }
    }

    fn close(&self, error: Option<String>) {
        let removed = CONNECTIONS.with(|connections| connections.borrow_mut().remove(&self.key));
        if removed.is_none() {
            return;
        }
        // Dropping the sender closes the host's input, which ends it.
        self.outbound.borrow_mut().take();
        let message = error.unwrap_or_else(|| "Native host has exited.".into());
        if let Some(reply) = self.reply.borrow_mut().take() {
            let error = crate::error(&message);
            reply.call((std::ptr::null_mut(), Retained::as_ptr(&error).cast_mut()));
        }
        if let Some(port) = &self.port {
            let error = crate::error(&message);
            unsafe { port.disconnectWithError(Some(&error)) };
        }
        if let Some(shared) = self.shared.upgrade() {
            shared.host().log(
                &self.extension,
                LogLevel::Info,
                &format!("native messaging host {} closed", self.host),
            );
        }
    }
}

/// Ends every host an extension started in one profile.
pub(crate) fn close_extension(shared: &Shared, extension: &str) {
    let open: Vec<_> = CONNECTIONS.with(|connections| {
        connections
            .borrow()
            .values()
            .filter(|connection| connection.belongs_to(shared, extension))
            .cloned()
            .collect()
    });
    for connection in open {
        connection.close(None);
    }
}

fn deliver(key: usize, event: Event) {
    dispatch2::DispatchQueue::main().exec_async(move || {
        let connection = CONNECTIONS.with(|connections| connections.borrow().get(&key).cloned());
        if let Some(connection) = connection {
            connection.receive(event);
        }
    });
}

/// Starts the host and returns the channel its input is written through.
fn spawn(path: &Path, extension: &str, key: usize) -> io::Result<mpsc::Sender<Vec<u8>>> {
    let mut command = Command::new(path);
    command
        .arg(format!("{}://{extension}/", crate::runtime::SCHEME))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(if crate::tracing() {
            Stdio::inherit()
        } else {
            Stdio::null()
        });
    if let Some(directory) = path.parent() {
        command.current_dir(directory);
    }
    let mut child = command.spawn()?;
    let (Some(mut input), Some(mut output)) = (child.stdin.take(), child.stdout.take()) else {
        let _ = child.kill();
        return Err(io::Error::other(
            "the host's standard streams are unavailable",
        ));
    };
    let child = Arc::new(Mutex::new(child));
    let (sender, receiver) = mpsc::channel::<Vec<u8>>();

    let writer_child = child.clone();
    std::thread::Builder::new()
        .name("zephium-native-host-writer".into())
        .spawn(move || {
            for frame in receiver {
                let length = (frame.len() as u32).to_ne_bytes();
                let written = input
                    .write_all(&length)
                    .and_then(|()| input.write_all(&frame))
                    .and_then(|()| input.flush());
                if written.is_err() {
                    break;
                }
            }
            drop(input);
            reap(&writer_child);
        })?;
    std::thread::Builder::new()
        .name("zephium-native-host-reader".into())
        .spawn(move || loop {
            match read_frame(&mut output) {
                Ok(Some(message)) => deliver(key, Event::Message(message)),
                Ok(None) => return deliver(key, Event::Closed(None)),
                Err(error) => {
                    let _ = child.lock().map(|mut child| child.kill());
                    return deliver(key, Event::Closed(Some(error)));
                }
            }
        })?;
    Ok(sender)
}

fn reap(child: &Mutex<Child>) {
    let deadline = Instant::now() + EXIT_GRACE;
    loop {
        let Ok(mut child) = child.lock() else { return };
        match child.try_wait() {
            Ok(Some(_)) | Err(_) => return,
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return;
            }
            Ok(None) => {}
        }
        drop(child);
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Reads one message; `None` at the end of the stream.
fn read_frame(stream: &mut impl Read) -> Result<Option<Value>, String> {
    let mut length = [0u8; 4];
    match stream.read_exact(&mut length) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(error) => {
            return Err(format!(
                "Error when communicating with the native messaging host: {error}"
            ))
        }
    }
    let length = u32::from_ne_bytes(length) as usize;
    if length > MAX_FROM_HOST {
        return Err("Native host sent a message that exceeded the maximum allowed size.".into());
    }
    let mut body = vec![0u8; length];
    stream.read_exact(&mut body).map_err(|error| {
        format!("Error when communicating with the native messaging host: {error}")
    })?;
    serde_json::from_slice(&body)
        .map(Some)
        .map_err(|_| "Native host sent a message that is not valid JSON.".into())
}

/// Where Chromium browsers look for host manifests on macOS. Desktop
/// applications register with the browsers they know, so every common one is
/// searched, the user's own registrations first.
fn search_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        let support = home.join("Library/Application Support");
        for browser in [
            "Google/Chrome",
            "Chromium",
            "Microsoft Edge",
            "BraveSoftware/Brave-Browser",
            "Vivaldi",
            "Arc/User Data",
        ] {
            roots.push(support.join(browser).join("NativeMessagingHosts"));
        }
    }
    roots.push(PathBuf::from("/Library/Google/Chrome/NativeMessagingHosts"));
    roots.push(PathBuf::from(
        "/Library/Application Support/Chromium/NativeMessagingHosts",
    ));
    roots
}

/// Finds the first registration of `host` that admits `extension`.
fn find_host(roots: &[PathBuf], host: &str, extension: &str) -> Result<PathBuf, String> {
    const NOT_FOUND: &str = "Specified native messaging host not found.";
    if !valid_host_name(host) {
        return Err("Invalid native messaging host name specified.".into());
    }
    let origin = format!("{}://{extension}/", crate::runtime::SCHEME);
    let mut forbidden = false;
    for root in roots {
        let Some(manifest) = read_manifest(&root.join(format!("{host}.json"))) else {
            continue;
        };
        if manifest.get("name").and_then(Value::as_str) != Some(host)
            || manifest.get("type").and_then(Value::as_str) != Some("stdio")
        {
            continue;
        }
        let Some(path) = manifest
            .get("path")
            .and_then(Value::as_str)
            .map(PathBuf::from)
        else {
            continue;
        };
        if !path.is_absolute() || !path.is_file() {
            continue;
        }
        let allowed = manifest
            .get("allowed_origins")
            .and_then(Value::as_array)
            .is_some_and(|origins| origins.iter().any(|entry| entry.as_str() == Some(&origin)));
        if allowed {
            return Ok(path);
        }
        forbidden = true;
    }
    Err(if forbidden {
        "Access to the specified native messaging host is forbidden.".into()
    } else {
        NOT_FOUND.into()
    })
}

fn read_manifest(path: &Path) -> Option<Value> {
    let file = std::fs::File::open(path).ok()?;
    if !file.metadata().ok()?.is_file() {
        return None;
    }
    let mut bytes = Vec::new();
    file.take(MAX_MANIFEST + 1).read_to_end(&mut bytes).ok()?;
    if bytes.len() as u64 > MAX_MANIFEST {
        return None;
    }
    serde_json::from_slice(&bytes).ok()
}

/// Chrome's rule: lowercase letters, digits, underscores and single dots
/// between them. It also keeps the name from leaving the manifest directory.
fn valid_host_name(name: &str) -> bool {
    !name.is_empty()
        && name.split('.').all(|part| {
            !part.is_empty()
                && part
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const EXTENSION: &str = "aeblfdkhhhdcdjpifhhbdiojplfjncoa";

    fn register(root: &Path, name: &str, path: &str, origins: &[&str]) {
        std::fs::create_dir_all(root).unwrap();
        let manifest = json!({
            "name": name,
            "description": "Test host",
            "path": path,
            "type": "stdio",
            "allowed_origins": origins,
        });
        std::fs::write(root.join(format!("{name}.json")), manifest.to_string()).unwrap();
    }

    #[test]
    fn host_names_follow_chrome_rules() {
        assert!(valid_host_name("com.1password.1password"));
        assert!(valid_host_name("com.8bit.bitwarden"));
        for name in [
            "",
            ".com",
            "com.",
            "com..x",
            "Com.x",
            "../etc/passwd",
            "a/b",
            "a-b",
        ] {
            assert!(!valid_host_name(name), "{name}");
        }
    }

    #[test]
    fn finds_the_first_registration_that_admits_the_extension() {
        let dir = std::env::temp_dir().join(format!("zephium-native-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (first, second) = (dir.join("chrome"), dir.join("edge"));
        let origin = format!("chrome-extension://{EXTENSION}/");
        register(
            &first,
            "com.example.host",
            "/bin/cat",
            &["chrome-extension://otherotherotherotherotherotherot/"],
        );
        register(&second, "com.example.host", "/bin/cat", &[&origin]);
        register(&first, "com.example.relative", "cat", &[&origin]);

        let roots = [first.clone(), second.clone()];
        assert_eq!(
            find_host(&roots, "com.example.host", EXTENSION),
            Ok(PathBuf::from("/bin/cat"))
        );
        assert!(find_host(&roots[..1], "com.example.host", EXTENSION)
            .unwrap_err()
            .contains("forbidden"));
        assert!(find_host(&roots, "com.example.relative", EXTENSION)
            .unwrap_err()
            .contains("not found"));
        assert!(find_host(&roots, "com.example.missing", EXTENSION)
            .unwrap_err()
            .contains("not found"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn frames_are_length_prefixed_json_within_chrome_limits() {
        let mut stream = Vec::new();
        for message in [json!({ "a": 1 }), json!("text")] {
            let body = serde_json::to_vec(&message).unwrap();
            stream.extend((body.len() as u32).to_ne_bytes());
            stream.extend(body);
        }
        let mut reader = stream.as_slice();
        assert_eq!(read_frame(&mut reader), Ok(Some(json!({ "a": 1 }))));
        assert_eq!(read_frame(&mut reader), Ok(Some(json!("text"))));
        assert_eq!(read_frame(&mut reader), Ok(None));

        let oversized = ((MAX_FROM_HOST + 1) as u32).to_ne_bytes();
        assert!(read_frame(&mut oversized.as_slice())
            .unwrap_err()
            .contains("maximum"));
        let invalid = [&3u32.to_ne_bytes()[..], b"{x}"].concat();
        assert!(read_frame(&mut invalid.as_slice())
            .unwrap_err()
            .contains("JSON"));
    }
}
