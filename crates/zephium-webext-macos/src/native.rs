//! Native messaging: extensions talking to desktop applications.
//!
//! Hosts are found the way Chrome finds them, through the manifests desktop
//! applications register for Chrome and other Chromium browsers, and speak
//! Chrome's stdio protocol: JSON messages, each prefixed with its length as a
//! native-endian `u32`. A host runs until its port closes; a one-off message
//! gets a host of its own that ends after the first reply.

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::rc::{Rc, Weak as RcWeak};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Condvar, Mutex};
#[cfg(test)]
use std::time::Duration;

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
const MAX_PHYSICAL_HOSTS: usize = 64;
const MAX_OUTBOUND_FRAMES: usize = 32;
const MAX_QUEUED_BYTES: usize = 128 * 1024 * 1024;
const MAX_INBOUND_EVENTS: usize = 8;
const MAX_INBOUND_BYTES: usize = 2 * MAX_FROM_HOST;
static PHYSICAL_HOSTS: AtomicUsize = AtomicUsize::new(0);
static QUEUED_BYTES: AtomicUsize = AtomicUsize::new(0);

type Reply = RcBlock<dyn Fn(*mut AnyObject, *mut NSError)>;

thread_local! {
    static CONNECTIONS: RefCell<HashMap<usize, Rc<Connection>>> = RefCell::new(HashMap::new());
    static HOST_COUNTS: RefCell<HashMap<(usize, String), std::sync::Weak<AtomicUsize>>> = RefCell::new(HashMap::new());
}

static NEXT_KEY: AtomicUsize = AtomicUsize::new(1);

enum Event {
    Message(Value, usize),
    Closed(Option<String>),
}

struct Connection {
    key: usize,
    extension: String,
    host: String,
    port: Option<Retained<WKWebExtensionMessagePort>>,
    reply: RefCell<Option<Reply>>,
    outbound: RefCell<Option<Outbound>>,
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
    let extension = unsafe { context.uniqueIdentifier() }.to_string();
    let allowed = shared
        .loaded
        .borrow()
        .get(&extension)
        .is_some_and(|loaded| {
            loaded.external_native_messaging.get()
                && std::ptr::eq(&*loaded.context, context)
                && unsafe { context.hasPermission(&NSString::from_str("nativeMessaging")) }
        });
    if !allowed {
        return Err("Access to native messaging requires nativeMessaging permission.".into());
    }
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
    let outbound = spawn(&path, &extension, key, Rc::as_ptr(shared) as usize).map_err(|error| {
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

/// Main-thread qualification used only by the standalone lab. The invalid
/// hostname stops an admitted request before any host registration is read.
#[cfg(feature = "lab")]
pub(crate) fn verify_open_admission(shared: &Rc<Shared>, id: &str) -> Result<(), String> {
    use objc2::{MainThreadMarker, MainThreadOnly};
    use objc2_web_kit::WKWebExtensionContextPermissionStatus;
    let mtm = MainThreadMarker::new().ok_or("native admission probe needs the main thread")?;
    let context = shared
        .loaded
        .borrow()
        .get(id)
        .map(|loaded| loaded.context.clone())
        .ok_or("fixture extension was not loaded")?;
    let check = |context: &WKWebExtensionContext, expected: &str| match open(
        shared,
        context,
        "../no-host-lookup",
        None,
        None,
    ) {
        Err(error) if error == expected => Ok(()),
        Err(error) => Err(format!(
            "native admission returned {error}; expected {expected}"
        )),
        Ok(connection) => {
            connection.close(None);
            Err("native admission unexpectedly opened a host".into())
        }
    };
    const DENIED: &str = "Access to native messaging requires nativeMessaging permission.";
    check(&context, "Invalid native messaging host name specified.")?;
    shared
        .loaded
        .borrow_mut()
        .get_mut(id)
        .unwrap()
        .external_native_messaging
        .set(false);
    let unreviewed = check(&context, DENIED);
    shared
        .loaded
        .borrow_mut()
        .get_mut(id)
        .unwrap()
        .external_native_messaging
        .set(true);
    unreviewed?;
    let mismatch = unsafe {
        WKWebExtensionContext::initForExtension(
            WKWebExtensionContext::alloc(mtm),
            &context.webExtension(),
        )
    };
    let permission = NSString::from_str("nativeMessaging");
    unsafe {
        mismatch.setUniqueIdentifier(&NSString::from_str(id));
        mismatch.setPermissionStatus_forPermission(
            WKWebExtensionContextPermissionStatus::GrantedExplicitly,
            &permission,
        );
    }
    check(&mismatch, DENIED)?;
    let loaded = shared.loaded.borrow_mut().remove(id).unwrap();
    let unloaded = check(&context, DENIED);
    shared.loaded.borrow_mut().insert(id.to_owned(), loaded);
    unloaded?;
    let previous = unsafe { context.permissionStatusForPermission(&permission) };
    unsafe {
        context.setPermissionStatus_forPermission(
            WKWebExtensionContextPermissionStatus::DeniedExplicitly,
            &permission,
        )
    };
    let no_permission = check(&context, DENIED);
    unsafe { context.setPermissionStatus_forPermission(previous, &permission) };
    no_permission
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
            Event::Message(message, _) => {
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
        // Independent child custody stops and reaps even a blocked pipe writer.
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

/// Only one bounded batch can be scheduled on the main queue per connection.
#[derive(Default)]
struct InboxState {
    events: VecDeque<Event>,
    bytes: usize,
    scheduled: bool,
    terminal: bool,
}
struct Inbox {
    key: usize,
    state: Mutex<InboxState>,
    drained: Condvar,
}
impl Inbox {
    fn push(self: &Arc<Self>, event: Event) -> bool {
        let Ok(mut state) = self.state.lock() else {
            return false;
        };
        let bytes = match &event {
            Event::Message(_, bytes) => *bytes,
            Event::Closed(_) => 0,
        };
        // Blocking only this pipe reader lets the OS pipe backpressure the host.
        // Main-queue delivery and cancellation both wake it without polling.
        while !state.terminal
            && (state.events.len() >= MAX_INBOUND_EVENTS || state.bytes + bytes > MAX_INBOUND_BYTES)
        {
            let Ok(next) = self.drained.wait(state) else {
                return false;
            };
            state = next;
        }
        if state.terminal {
            return false;
        }
        state.bytes += bytes;
        state.events.push_back(event);
        self.schedule(&mut state);
        true
    }
    fn finish(self: &Arc<Self>, error: Option<String>) {
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        if state.terminal {
            return;
        }
        state.terminal = true;
        self.drained.notify_all();
        // One terminal event has a reserved slot beyond the reply budget.
        state.events.push_back(Event::Closed(error));
        self.schedule(&mut state);
    }
    fn schedule(self: &Arc<Self>, state: &mut InboxState) {
        if state.scheduled {
            return;
        }
        state.scheduled = true;
        let inbox = self.clone();
        dispatch2::DispatchQueue::main().exec_async(move || {
            let events = {
                let Ok(mut state) = inbox.state.lock() else {
                    return;
                };
                state.bytes = 0;
                state.scheduled = false;
                let events = std::mem::take(&mut state.events);
                inbox.drained.notify_all();
                events
            };
            for event in events {
                let connection =
                    CONNECTIONS.with(|connections| connections.borrow().get(&inbox.key).cloned());
                if let Some(connection) = connection {
                    connection.receive(event);
                }
            }
        });
    }
}

/// A physical slot is released only after the owned child has been reaped.
struct PhysicalPermit(Arc<AtomicUsize>);
impl PhysicalPermit {
    fn acquire(owner: usize, extension: &str) -> io::Result<Self> {
        let count = HOST_COUNTS.with(|counts| {
            let mut counts = counts.borrow_mut();
            counts.retain(|_, count| count.strong_count() > 0);
            let key = (owner, extension.to_owned());
            if let Some(count) = counts.get(&key).and_then(std::sync::Weak::upgrade) {
                return count;
            }
            let count = Arc::new(AtomicUsize::new(0));
            counts.insert(key, Arc::downgrade(&count));
            count
        });
        PHYSICAL_HOSTS
            .fetch_update(Ordering::AcqRel, Ordering::Relaxed, |count| {
                (count < MAX_PHYSICAL_HOSTS).then_some(count + 1)
            })
            .map_err(|_| io::Error::other("too many physical native hosts"))?;
        if count
            .fetch_update(Ordering::AcqRel, Ordering::Relaxed, |count| {
                (count < MAX_HOSTS_PER_EXTENSION).then_some(count + 1)
            })
            .is_err()
        {
            PHYSICAL_HOSTS.fetch_sub(1, Ordering::AcqRel);
            return Err(io::Error::other(
                "too many physical native hosts for this extension",
            ));
        }
        Ok(Self(count))
    }
}
impl Drop for PhysicalPermit {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
        PHYSICAL_HOSTS.fetch_sub(1, Ordering::AcqRel);
    }
}
struct ChildCustody {
    child: Child,
    _permit: Option<PhysicalPermit>,
}
impl Drop for ChildCustody {
    fn drop(&mut self) {
        let _ = self.child.kill();
        if self.child.wait().is_err() {
            // Unknown reap status keeps capacity reserved instead of allowing
            // a replacement child to escape the physical process ceiling.
            if let Some(permit) = self._permit.take() {
                std::mem::forget(permit);
            }
        }
    }
}

struct QueuedFrame {
    body: Vec<u8>,
    bytes: Arc<AtomicUsize>,
}
impl Drop for QueuedFrame {
    fn drop(&mut self) {
        self.bytes.fetch_sub(self.body.len(), Ordering::AcqRel);
        QUEUED_BYTES.fetch_sub(self.body.len(), Ordering::AcqRel);
    }
}
struct Outbound {
    sender: mpsc::SyncSender<QueuedFrame>,
    stop: mpsc::SyncSender<()>,
    closed: Arc<AtomicBool>,
    bytes: Arc<AtomicUsize>,
    inbox: Arc<Inbox>,
}
impl Outbound {
    fn send(&self, body: Vec<u8>) -> Result<(), ()> {
        if self.closed.load(Ordering::Acquire) || body.len() > MAX_TO_HOST {
            return Err(());
        }
        let length = body.len();
        if self
            .bytes
            .fetch_update(Ordering::AcqRel, Ordering::Relaxed, |bytes| {
                bytes
                    .checked_add(length)
                    .filter(|bytes| *bytes <= MAX_TO_HOST)
            })
            .is_err()
        {
            self.close();
            return Err(());
        }
        if QUEUED_BYTES
            .fetch_update(Ordering::AcqRel, Ordering::Relaxed, |bytes| {
                bytes
                    .checked_add(length)
                    .filter(|bytes| *bytes <= MAX_QUEUED_BYTES)
            })
            .is_err()
        {
            self.bytes.fetch_sub(length, Ordering::AcqRel);
            self.close();
            return Err(());
        }
        if self
            .sender
            .try_send(QueuedFrame {
                body,
                bytes: self.bytes.clone(),
            })
            .is_err()
        {
            self.close();
            return Err(());
        }
        Ok(())
    }
    fn close(&self) {
        self.closed.store(true, Ordering::Release);
        self.inbox.finish(None);
        let _ = self.stop.try_send(());
    }
}
impl Drop for Outbound {
    fn drop(&mut self) {
        self.close();
    }
}

/// The independent supervisor owns the child, not the blocking writer. Stop
/// kills that exact host and reaps it without a main-loop timer or polling.
fn spawn(path: &Path, extension: &str, key: usize, owner: usize) -> io::Result<Outbound> {
    let permit = PhysicalPermit::acquire(owner, extension)?;
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
    let mut custody = ChildCustody {
        child: command.spawn()?,
        _permit: Some(permit),
    };
    let (Some(mut input), Some(mut output)) =
        (custody.child.stdin.take(), custody.child.stdout.take())
    else {
        return Err(io::Error::other(
            "the host's standard streams are unavailable",
        ));
    };
    let (stop, stopped) = mpsc::sync_channel(1);
    let (sender, receiver) = mpsc::sync_channel::<QueuedFrame>(MAX_OUTBOUND_FRAMES);
    let closed = Arc::new(AtomicBool::new(false));
    let inbox = Arc::new(Inbox {
        key,
        state: Mutex::new(InboxState::default()),
        drained: Condvar::new(),
    });
    let outbound = Outbound {
        sender,
        stop: stop.clone(),
        closed: closed.clone(),
        bytes: Arc::new(AtomicUsize::new(0)),
        inbox: inbox.clone(),
    };
    let supervised_inbox = inbox.clone();
    std::thread::Builder::new()
        .name("zephium-native-host-custody".into())
        .spawn(move || {
            let _ = stopped.recv();
            drop(custody);
            closed.store(true, Ordering::Release);
            supervised_inbox.finish(None);
        })?;
    let writer_stop = stop.clone();
    std::thread::Builder::new()
        .name("zephium-native-host-writer".into())
        .spawn(move || {
            for frame in receiver {
                let written = input
                    .write_all(&(frame.body.len() as u32).to_ne_bytes())
                    .and_then(|()| input.write_all(&frame.body))
                    .and_then(|()| input.flush());
                if written.is_err() {
                    break;
                }
            }
            drop(input);
            let _ = writer_stop.try_send(());
        })?;
    std::thread::Builder::new()
        .name("zephium-native-host-reader".into())
        .spawn(move || {
            let error = loop {
                match read_frame_sized(&mut output) {
                    Ok(Some((message, bytes))) => {
                        if !inbox.push(Event::Message(message, bytes)) {
                            break None;
                        }
                    }
                    Ok(None) => break None,
                    Err(error) => break Some(error),
                }
            };
            let _ = stop.try_send(());
            inbox.finish(error);
        })?;
    Ok(outbound)
}

/// Reads one message; `None` at the end of the stream.
fn read_frame_sized(stream: &mut impl Read) -> Result<Option<(Value, usize)>, String> {
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
        .map(|message| Some((message, length)))
        .map_err(|_| "Native host sent a message that is not valid JSON.".into())
}

#[cfg(test)]
fn read_frame(stream: &mut impl Read) -> Result<Option<Value>, String> {
    read_frame_sized(stream).map(|frame| frame.map(|(message, _)| message))
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

#[cfg(test)]
mod custody_tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    struct Fixture {
        directory: PathBuf,
        pid: u32,
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = Command::new("/bin/kill")
                .args(["-KILL", &self.pid.to_string()])
                .stderr(Stdio::null())
                .status();
            let _ = std::fs::remove_dir_all(&self.directory);
        }
    }
    fn stalled_host() -> (Fixture, Outbound) {
        let key = NEXT_KEY.fetch_add(1, Ordering::Relaxed);
        let directory =
            std::env::temp_dir().join(format!("zephium-stalled-host-{}-{key}", std::process::id()));
        std::fs::create_dir(&directory).unwrap();
        let pid_file = directory.join("pid");
        let path = directory.join("host");
        std::fs::write(
            &path,
            b"#!/bin/sh\nprintf '%s' \"$$\" > \"${0%/*}/pid\"\nexec /bin/sleep 60\n",
        )
        .unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        let outbound = spawn(&path, "fixture", key, 0).unwrap();
        let pid = (0..200)
            .find_map(|_| {
                let pid = std::fs::read_to_string(&pid_file)
                    .ok()
                    .and_then(|pid| pid.parse().ok());
                if pid.is_none() {
                    std::thread::sleep(Duration::from_millis(5));
                }
                pid
            })
            .expect("fixture started");
        (Fixture { directory, pid }, outbound)
    }

    #[test]
    fn stalled_native_host_cannot_accumulate_an_unbounded_outbound_backlog() {
        let (_fixture, outbound) = stalled_host();
        let admitted = (0..40)
            .filter(|_| outbound.send(vec![b'x'; 2 * 1024 * 1024]).is_ok())
            .count();
        assert!(
            admitted <= 32,
            "accepted {admitted} queued frames for a stalled host"
        );
    }

    #[test]
    fn closing_a_stalled_writer_terminates_the_physical_child() {
        let (fixture, outbound) = stalled_host();
        outbound.send(vec![b'x'; MAX_TO_HOST]).unwrap();
        std::thread::sleep(Duration::from_millis(100));
        drop(outbound);
        std::thread::sleep(Duration::from_millis(2300));
        let alive = Command::new("/bin/kill")
            .args(["-0", &fixture.pid.to_string()])
            .stderr(Stdio::null())
            .status()
            .unwrap()
            .success();
        assert!(
            !alive,
            "closing the logical port left its blocked child running"
        );
    }
}

#[cfg(test)]
mod backlog_tests {
    use super::*;

    #[test]
    fn full_inbox_blocks_reader_until_cancelled_instead_of_killing_host() {
        for (count, size) in [(MAX_INBOUND_EVENTS, 1), (2, MAX_FROM_HOST)] {
            let inbox = Arc::new(Inbox {
                key: 0,
                drained: Condvar::new(),
                state: Mutex::new(InboxState {
                    scheduled: true,
                    ..Default::default()
                }),
            });
            for _ in 0..count {
                assert!(inbox.push(Event::Message(Value::Null, size)));
            }
            let reader = inbox.clone();
            let (sent, received) = mpsc::channel();
            let worker = std::thread::spawn(move || {
                sent.send(reader.push(Event::Message(Value::Null, 1)))
                    .unwrap();
            });
            let blocked = received.recv_timeout(Duration::from_millis(100)).is_err();
            inbox.finish(None);
            worker.join().unwrap();
            assert!(
                blocked,
                "a full inbox terminated the reader instead of applying pipe backpressure"
            );
            let state = inbox.state.lock().unwrap();
            assert_eq!(state.events.len(), count + 1);
            assert!(matches!(state.events.back(), Some(Event::Closed(None))));
        }
    }

    #[test]
    fn draining_inbox_resumes_reader_and_preserves_order() {
        let inbox = Arc::new(Inbox {
            key: 0,
            drained: Condvar::new(),
            state: Mutex::new(InboxState {
                scheduled: true,
                ..Default::default()
            }),
        });
        for index in 0..MAX_INBOUND_EVENTS {
            assert!(inbox.push(Event::Message(serde_json::json!(index), 1)));
        }
        let reader = inbox.clone();
        let (sent, received) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            sent.send(reader.push(Event::Message(serde_json::json!(MAX_INBOUND_EVENTS), 1)))
                .unwrap();
        });
        assert!(received.recv_timeout(Duration::from_millis(100)).is_err());
        {
            let mut state = inbox.state.lock().unwrap();
            state.events.clear();
            state.bytes = 0;
            inbox.drained.notify_all();
        }
        assert!(received.recv_timeout(Duration::from_secs(2)).unwrap());
        worker.join().unwrap();
        let state = inbox.state.lock().unwrap();
        assert_eq!(state.events.len(), 1);
        assert!(
            matches!(state.events.front(), Some(Event::Message(value, _)) if value == &serde_json::json!(MAX_INBOUND_EVENTS))
        );
        assert!(!state.terminal);
    }

    #[test]
    fn normal_host_close_preserves_a_full_admitted_reply_batch() {
        let inbox = Arc::new(Inbox {
            key: 0,
            drained: Condvar::new(),
            state: Mutex::new(InboxState {
                scheduled: true,
                ..Default::default()
            }),
        });
        for index in 0..MAX_INBOUND_EVENTS {
            assert!(inbox.push(Event::Message(serde_json::json!(index), 1)));
        }
        inbox.finish(None);
        inbox.finish(Some("late".into()));
        assert!(!inbox.push(Event::Message(Value::Null, 1)));
        let state = inbox.state.lock().unwrap();
        assert_eq!(state.events.len(), MAX_INBOUND_EVENTS + 1);
        assert_eq!(state.bytes, MAX_INBOUND_EVENTS);
        for (index, event) in state.events.iter().take(MAX_INBOUND_EVENTS).enumerate() {
            assert!(
                matches!(event, Event::Message(value, _) if value == &serde_json::json!(index))
            );
        }
        assert!(matches!(state.events.back(), Some(Event::Closed(None))));
    }

    #[test]
    fn physical_extension_capacity_remains_held_until_permit_release() {
        let permits: Vec<_> = (0..MAX_HOSTS_PER_EXTENSION)
            .map(|_| PhysicalPermit::acquire(usize::MAX, "capacity-fixture").unwrap())
            .collect();
        assert!(PhysicalPermit::acquire(usize::MAX, "capacity-fixture").is_err());
        drop(permits);
        assert!(PhysicalPermit::acquire(usize::MAX, "capacity-fixture").is_ok());
    }
}
