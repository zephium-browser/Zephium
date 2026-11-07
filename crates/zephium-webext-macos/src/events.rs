//! Events the browser raises for APIs WebKit doesn't implement.
//!
//! Extension contexts that listen to such an event keep a native-messaging
//! port to [`APPLICATION`] and say which events they listen to. An event for
//! the worker is queued until the worker acknowledges it: WebKit unloads idle
//! workers without telling anyone, so a port can be dead while it looks open.
//! An unacknowledged event starts the worker again and is resent once the new
//! worker subscribes. Nothing runs while no events are pending.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet, VecDeque};
use std::ptr::NonNull;
use std::rc::{Rc, Weak as RcWeak};
use std::time::{Duration, Instant};

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::Message;
use objc2_foundation::{NSError, NSTimer};
use objc2_web_kit::{WKWebExtensionContext, WKWebExtensionMessagePort};
use serde_json::{json, Value};

use crate::runtime::Shared;
use crate::{json, LogLevel};

pub(crate) const APPLICATION: &str = "app.zephium.events";

const MAX_QUEUED: usize = 64;
const MAX_EVENT_NAMES: usize = 64;
/// A live worker acknowledges within milliseconds.
const ACK_SECONDS: f64 = 2.0;
/// How long an extension may take to answer an event that waits for it.
const REPLY_SECONDS: f64 = 120.0;

/// Settles an event that waits for the extension's answer; `None` when it
/// gave none.
pub(crate) type Reply = Box<dyn FnOnce(Option<Value>)>;

struct Port {
    key: usize,
    extension: String,
    port: Retained<WKWebExtensionMessagePort>,
    worker: Cell<bool>,
    ready: Cell<bool>,
    events: RefCell<HashSet<String>>,
}

impl Port {
    fn post(&self, message: &Value) {
        let object = json::to_object(message);
        unsafe { self.port.sendMessage_completionHandler(Some(&object), None) };
    }
}

struct Queued {
    seq: u64,
    name: String,
    args: Value,
    reply: Option<Reply>,
    /// When it was posted to the worker, once it has been.
    sent: Option<Instant>,
}

#[derive(Default)]
struct Listening {
    /// What the last worker listened to; it outlives the worker.
    worker_events: HashSet<String>,
    queue: VecDeque<Queued>,
    waking: bool,
}

#[derive(Default)]
pub(crate) struct Hub {
    ports: RefCell<Vec<Rc<Port>>>,
    extensions: RefCell<HashMap<String, Listening>>,
    replies: RefCell<HashMap<u64, (usize, Reply)>>,
    next: Cell<u64>,
    ack_timer: RefCell<Option<Retained<NSTimer>>>,
}

impl Hub {
    fn seq(&self) -> u64 {
        let seq = self.next.get().wrapping_add(1);
        self.next.set(seq);
        seq
    }

    fn port(&self, key: usize) -> Option<Rc<Port>> {
        self.ports
            .borrow()
            .iter()
            .find(|port| port.key == key)
            .cloned()
    }

    fn worker_port(&self, extension: &str) -> Option<Rc<Port>> {
        self.ports
            .borrow()
            .iter()
            .find(|port| port.extension == extension && port.worker.get() && port.ready.get())
            .cloned()
    }
}

pub(crate) fn connect(
    shared: &Rc<Shared>,
    context: &WKWebExtensionContext,
    port: &WKWebExtensionMessagePort,
) {
    let entry = Rc::new(Port {
        key: port as *const _ as usize,
        extension: unsafe { context.uniqueIdentifier() }.to_string(),
        port: port.retain(),
        worker: Cell::new(false),
        ready: Cell::new(false),
        events: RefCell::new(HashSet::new()),
    });
    let key = entry.key;
    let weak = Rc::downgrade(shared);
    let on_message = RcBlock::new(move |message: *mut AnyObject, _error: *mut NSError| {
        if let Some(shared) = weak.upgrade() {
            receive(&shared, key, json::from_object(unsafe { message.as_ref() }));
        }
    });
    let weak = Rc::downgrade(shared);
    let on_disconnect = RcBlock::new(move |_error: *mut NSError| {
        if let Some(shared) = weak.upgrade() {
            drop_port(&shared, key);
            crate::cookies::refresh(&shared);
            crate::data::refresh_interest(&shared);
        }
    });
    unsafe {
        port.setMessageHandler(Some(&on_message));
        port.setDisconnectHandler(Some(&on_disconnect));
    }
    shared.events.ports.borrow_mut().push(entry);
}

fn receive(shared: &Rc<Shared>, key: usize, message: Value) {
    let hub = &shared.events;
    let Some(port) = hub.port(key) else { return };
    match message.get("op").and_then(Value::as_str) {
        // WebKit drops what either side posts on a port that has only just
        // opened, so the context repeats "hello" until it hears one back.
        Some("hello") => port.post(&json!({ "op": "hello" })),
        Some("subscribe") => {
            let events: HashSet<String> = message
                .get("events")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .take(MAX_EVENT_NAMES)
                .map(|name| name.chars().take(80).collect())
                .collect();
            let worker = message.get("worker").and_then(Value::as_bool) == Some(true);
            port.ready.set(true);
            port.worker.set(worker);
            let mut unheard = VecDeque::new();
            if worker {
                let mut extensions = hub.extensions.borrow_mut();
                let listening = extensions.entry(port.extension.clone()).or_default();
                listening.worker_events = events.clone();
                listening.waking = false;
                let (kept, dropped): (VecDeque<Queued>, VecDeque<Queued>) =
                    std::mem::take(&mut listening.queue)
                        .into_iter()
                        .partition(|queued| events.contains(&queued.name));
                listening.queue = kept;
                for queued in &mut listening.queue {
                    queued.sent = None;
                }
                unheard = dropped;
            }
            *port.events.borrow_mut() = events;
            for queued in unheard {
                if let Some(reply) = queued.reply {
                    reply(None);
                }
            }
            if worker {
                pump(shared, &port.extension);
            }
            crate::cookies::refresh(shared);
            crate::data::refresh_interest(shared);
        }
        Some("ack") => {
            let Some(seq) = message.get("seq").and_then(Value::as_u64) else {
                return;
            };
            let mut extensions = hub.extensions.borrow_mut();
            let Some(listening) = extensions.get_mut(&port.extension) else {
                return;
            };
            if let Some(index) = listening
                .queue
                .iter()
                .position(|q| q.seq == seq && q.sent.is_some())
            {
                let queued = listening.queue.remove(index).expect("index is in range");
                if let Some(reply) = queued.reply {
                    hub.replies.borrow_mut().insert(seq, (key, reply));
                    schedule_reply_timeout(shared, seq);
                }
            }
        }
        Some("reply") => {
            let Some(seq) = message.get("seq").and_then(Value::as_u64) else {
                return;
            };
            let reply = {
                let mut replies = hub.replies.borrow_mut();
                match replies.get(&seq) {
                    Some((owner, _)) if *owner == key => replies.remove(&seq),
                    _ => None,
                }
            };
            if let Some((_, reply)) = reply {
                reply(
                    message
                        .get("value")
                        .filter(|value| !value.is_null())
                        .cloned(),
                );
            }
        }
        _ => {}
    }
}

/// Raises `name` with `args` (a JSON array) in every context of `extension`
/// that listens to it, starting the worker when it listens and isn't running.
/// With `reply`, only the worker receives it and its answer settles `reply`.
pub(crate) fn emit(
    shared: &Rc<Shared>,
    extension: &str,
    name: &str,
    args: Value,
    reply: Option<Reply>,
) {
    let hub = &shared.events;
    if reply.is_none() {
        let pages: Vec<Rc<Port>> = hub
            .ports
            .borrow()
            .iter()
            .filter(|port| {
                port.extension == extension
                    && !port.worker.get()
                    && port.ready.get()
                    && port.events.borrow().contains(name)
            })
            .cloned()
            .collect();
        for port in pages {
            port.post(&json!({ "op": "event", "name": name, "args": args }));
        }
    }
    let unanswered = {
        let mut extensions = hub.extensions.borrow_mut();
        match extensions.get_mut(extension) {
            Some(listening) if listening.worker_events.contains(name) => {
                let dropped = (listening.queue.len() >= MAX_QUEUED)
                    .then(|| listening.queue.pop_front())
                    .flatten();
                listening.queue.push_back(Queued {
                    seq: hub.seq(),
                    name: name.to_owned(),
                    args,
                    reply,
                    sent: None,
                });
                Ok(dropped.and_then(|queued| queued.reply))
            }
            _ => Err(reply),
        }
    };
    match unanswered {
        Ok(dropped) => {
            if let Some(reply) = dropped {
                reply(None);
            }
            pump(shared, extension);
        }
        Err(Some(reply)) => reply(None),
        Err(None) => {}
    }
}

fn pump(shared: &Rc<Shared>, extension: &str) {
    let hub = &shared.events;
    let Some(port) = hub.worker_port(extension) else {
        return wake(shared, extension);
    };
    let mut posted = false;
    if let Some(listening) = hub.extensions.borrow_mut().get_mut(extension) {
        for queued in listening
            .queue
            .iter_mut()
            .filter(|queued| queued.sent.is_none())
        {
            queued.sent = Some(Instant::now());
            posted = true;
            let mut message = json!({
                "op": "event", "name": queued.name, "args": queued.args, "seq": queued.seq,
            });
            if queued.reply.is_some() {
                message["reply"] = Value::Bool(true);
            }
            port.post(&message);
        }
    }
    if posted {
        // Delivery also keeps the worker alive, as an event does in Chrome.
        touch(shared, extension);
        schedule_ack_check(shared);
    }
}

fn wake(shared: &Rc<Shared>, extension: &str) {
    {
        let mut extensions = shared.events.extensions.borrow_mut();
        let Some(listening) = extensions.get_mut(extension) else {
            return;
        };
        if listening.waking || listening.queue.is_empty() {
            return;
        }
        listening.waking = true;
    }
    let Some(context) = shared
        .loaded
        .borrow()
        .get(extension)
        .map(|l| l.context.clone())
    else {
        return forget(shared, extension);
    };
    let weak = Rc::downgrade(shared);
    let id = extension.to_owned();
    let completion = RcBlock::new(move |error: *mut NSError| {
        let Some(shared) = weak.upgrade() else { return };
        match unsafe { error.as_ref() } {
            Some(error) => {
                if let Some(context) = shared.loaded.borrow().get(&id).map(|l| l.context.clone()) {
                    let text = crate::runtime::describe(Some(error));
                    shared.log(&context, LogLevel::Warning, &format!("events: {text}"));
                }
                fail(&shared, &id);
            }
            None => expect_subscription(&shared, &id),
        }
    });
    unsafe { context.loadBackgroundContentWithCompletionHandler(&completion) };
}

/// A started worker that no longer listens never subscribes; what waited for
/// it is dropped.
fn expect_subscription(shared: &Rc<Shared>, extension: &str) {
    let weak: RcWeak<Shared> = Rc::downgrade(shared);
    let id = extension.to_owned();
    let check = RcBlock::new(move |_timer: NonNull<NSTimer>| {
        let Some(shared) = weak.upgrade() else { return };
        let waiting = shared
            .events
            .extensions
            .borrow()
            .get(&id)
            .is_some_and(|listening| listening.waking);
        if waiting && shared.events.worker_port(&id).is_none() {
            fail(&shared, &id);
        }
    });
    unsafe {
        NSTimer::scheduledTimerWithTimeInterval_repeats_block(ACK_SECONDS * 3.0, false, &check)
    };
}

fn touch(shared: &Rc<Shared>, extension: &str) {
    if let Some(context) = shared
        .loaded
        .borrow()
        .get(extension)
        .map(|l| l.context.clone())
    {
        let completion = RcBlock::new(|_error: *mut NSError| {});
        unsafe { context.loadBackgroundContentWithCompletionHandler(&completion) };
    }
}

/// Drops everything queued for an extension whose worker can't start.
fn fail(shared: &Shared, extension: &str) {
    let queue = {
        let mut extensions = shared.events.extensions.borrow_mut();
        let Some(listening) = extensions.get_mut(extension) else {
            return;
        };
        listening.waking = false;
        std::mem::take(&mut listening.queue)
    };
    for queued in queue {
        if let Some(reply) = queued.reply {
            reply(None);
        }
    }
}

fn schedule_ack_check(shared: &Rc<Shared>) {
    let hub = &shared.events;
    if hub.ack_timer.borrow().is_some() {
        return;
    }
    let weak: RcWeak<Shared> = Rc::downgrade(shared);
    let check = RcBlock::new(move |_timer: NonNull<NSTimer>| {
        if let Some(shared) = weak.upgrade() {
            shared.events.ack_timer.borrow_mut().take();
            check_acks(&shared);
        }
    });
    let timer = unsafe {
        NSTimer::scheduledTimerWithTimeInterval_repeats_block(ACK_SECONDS, false, &check)
    };
    *hub.ack_timer.borrow_mut() = Some(timer);
}

/// A worker that left events unacknowledged is gone: its port is dropped and
/// the events go to the next worker.
fn check_acks(shared: &Rc<Shared>) {
    // One timer serves every send, so an event posted after it was armed is
    // still inside its own window and gets one more round.
    let window = Duration::from_secs_f64(ACK_SECONDS);
    let overdue = |queued: &Queued| queued.sent.is_some_and(|at| at.elapsed() >= window);
    let mut waiting = false;
    let mut stale = Vec::new();
    for (extension, listening) in shared.events.extensions.borrow().iter() {
        if listening.queue.iter().any(overdue) {
            stale.push(extension.clone());
        } else if listening.queue.iter().any(|queued| queued.sent.is_some()) {
            waiting = true;
        }
    }
    if waiting {
        schedule_ack_check(shared);
    }
    for extension in stale {
        if crate::tracing() {
            eprintln!("webext-trace: events of {extension} unacknowledged; restarting its worker");
        }
        let workers: Vec<usize> = shared
            .events
            .ports
            .borrow()
            .iter()
            .filter(|port| port.extension == extension && port.worker.get())
            .map(|port| port.key)
            .collect();
        for key in workers {
            close_port(shared, key);
        }
        if let Some(listening) = shared.events.extensions.borrow_mut().get_mut(&extension) {
            for queued in &mut listening.queue {
                queued.sent = None;
            }
        }
        pump(shared, &extension);
    }
}

fn schedule_reply_timeout(shared: &Rc<Shared>, seq: u64) {
    let weak: RcWeak<Shared> = Rc::downgrade(shared);
    let expire = RcBlock::new(move |_timer: NonNull<NSTimer>| {
        let Some(shared) = weak.upgrade() else { return };
        let reply = shared.events.replies.borrow_mut().remove(&seq);
        if let Some((_, reply)) = reply {
            reply(None);
        }
    });
    unsafe { NSTimer::scheduledTimerWithTimeInterval_repeats_block(REPLY_SECONDS, false, &expire) };
}

fn close_port(shared: &Shared, key: usize) {
    if let Some(port) = shared.events.port(key) {
        unsafe { port.port.disconnect() };
    }
    drop_port(shared, key);
}

fn drop_port(shared: &Shared, key: usize) {
    shared
        .events
        .ports
        .borrow_mut()
        .retain(|port| port.key != key);
    let orphaned: Vec<Reply> = {
        let mut replies = shared.events.replies.borrow_mut();
        let keys: Vec<u64> = replies
            .iter()
            .filter(|(_, (owner, _))| *owner == key)
            .map(|(seq, _)| *seq)
            .collect();
        keys.into_iter()
            .filter_map(|seq| replies.remove(&seq).map(|(_, reply)| reply))
            .collect()
    };
    for reply in orphaned {
        reply(None);
    }
}

/// A new worker started: the previous one's ports are stale (see
/// [`crate::worker_gone`]) and anything sent to them goes to the new one.
pub(crate) fn worker_gone(shared: &Shared, extension: &str) {
    let workers: Vec<usize> = shared
        .events
        .ports
        .borrow()
        .iter()
        .filter(|port| port.extension == extension && port.worker.get())
        .map(|port| port.key)
        .collect();
    for key in workers {
        close_port(shared, key);
    }
    if let Some(listening) = shared.events.extensions.borrow_mut().get_mut(extension) {
        for queued in &mut listening.queue {
            queued.sent = None;
        }
    }
}

/// The extension was unloaded.
pub(crate) fn forget(shared: &Shared, extension: &str) {
    let keys: Vec<usize> = shared
        .events
        .ports
        .borrow()
        .iter()
        .filter(|port| port.extension == extension)
        .map(|port| port.key)
        .collect();
    for key in keys {
        close_port(shared, key);
    }
    fail(shared, extension);
    shared.events.extensions.borrow_mut().remove(extension);
}

/// Whether any context of `extension` listens to `name`, so producers skip
/// work nobody would see.
pub(crate) fn listened(shared: &Shared, extension: &str, name: &str) -> bool {
    shared
        .events
        .extensions
        .borrow()
        .get(extension)
        .is_some_and(|listening| listening.worker_events.contains(name))
        || shared
            .events
            .ports
            .borrow()
            .iter()
            .any(|port| port.extension == extension && port.events.borrow().contains(name))
}
