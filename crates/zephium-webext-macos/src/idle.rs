//! `chrome.idle`: whether the person is using the Mac, for extensions that
//! lock a vault or pause work while they are away.
//!
//! Locking and unlocking arrive as system notifications. Becoming idle is
//! checked once, when the earliest watching extension's interval could have
//! passed; only while someone is idle (and the screen isn't locked) is input
//! checked every few seconds, to notice them coming back.

use std::cell::RefCell;
use std::ptr::NonNull;
use std::rc::{Rc, Weak as RcWeak};
use std::time::{Duration, Instant};

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_foundation::{
    NSDistributedNotificationCenter, NSNotification, NSObjectProtocol, NSString, NSTimer,
};
use serde_json::json;

use crate::events;
use crate::runtime::Shared;

pub(crate) const EVENT: &str = "idle.onStateChanged";
const DEFAULT_INTERVAL: u64 = 60;
const MIN_INTERVAL: u64 = 15;
const MAX_INTERVAL: u64 = 4 * 60 * 60;
const RETURN_POLL_SECONDS: f64 = 5.0;
const SUBSCRIBE_GRACE: Duration = Duration::from_secs(10);

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGEventSourceSecondsSinceLastEventType(state: i32, event_type: u32) -> f64;
}

const HID_SYSTEM_STATE: i32 = 1;
const ANY_INPUT_EVENT: u32 = !0;

#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    Active,
    Idle,
    Locked,
}

impl State {
    fn name(self) -> &'static str {
        match self {
            State::Active => "active",
            State::Idle => "idle",
            State::Locked => "locked",
        }
    }
}

struct Watch {
    shared: RcWeak<Shared>,
    extension: String,
    interval: u64,
    reported: State,
    /// A watch starts before the listener's subscription reaches the browser.
    since: Instant,
}

#[derive(Default)]
struct Monitor {
    watches: Vec<Watch>,
    timer: Option<Retained<NSTimer>>,
    observers: Vec<Retained<ProtocolObject<dyn NSObjectProtocol>>>,
    locked: bool,
}

thread_local! {
    static MONITOR: RefCell<Monitor> = RefCell::new(Monitor::default());
}

fn idle_seconds() -> f64 {
    unsafe { CGEventSourceSecondsSinceLastEventType(HID_SYSTEM_STATE, ANY_INPUT_EVENT) }
}

fn clamp(interval: Option<u64>) -> u64 {
    interval
        .unwrap_or(DEFAULT_INTERVAL)
        .clamp(MIN_INTERVAL, MAX_INTERVAL)
}

fn state_for(interval: u64, locked: bool, idle: f64) -> State {
    if locked {
        State::Locked
    } else if idle >= interval as f64 {
        State::Idle
    } else {
        State::Active
    }
}

pub(crate) fn query(interval: Option<u64>) -> &'static str {
    let locked = MONITOR.with(|monitor| monitor.borrow().locked);
    state_for(clamp(interval), locked, idle_seconds()).name()
}

/// Starts or updates `extension`'s watch; it ends once nothing listens.
pub(crate) fn watch(shared: &Rc<Shared>, extension: &str, interval: Option<u64>) {
    let interval = clamp(interval);
    MONITOR.with(|monitor| {
        let mut monitor = monitor.borrow_mut();
        if monitor.observers.is_empty() {
            monitor.observers = observe_lock();
        }
        let current = state_for(interval, monitor.locked, idle_seconds());
        let existing = monitor.watches.iter_mut().find(|w| {
            w.extension == extension && std::ptr::eq(w.shared.as_ptr(), Rc::as_ptr(shared))
        });
        match existing {
            Some(watch) => watch.interval = interval,
            None => monitor.watches.push(Watch {
                shared: Rc::downgrade(shared),
                extension: extension.to_owned(),
                interval,
                reported: current,
                since: Instant::now(),
            }),
        }
    });
    update();
}

pub(crate) fn forget(shared: &Shared, extension: &str) {
    MONITOR.with(|monitor| {
        monitor
            .borrow_mut()
            .watches
            .retain(|w| !(w.extension == extension && std::ptr::eq(w.shared.as_ptr(), shared)));
    });
}

/// Reports changes and arms the next check.
fn update() {
    let idle = idle_seconds();
    let mut changes = Vec::new();
    let next = MONITOR.with(|monitor| {
        let mut monitor = monitor.borrow_mut();
        let locked = monitor.locked;
        monitor.watches.retain(|watch| {
            watch.shared.upgrade().is_some_and(|shared| {
                shared.loaded.borrow().contains_key(&watch.extension)
                    && (watch.since.elapsed() < SUBSCRIBE_GRACE
                        || events::listened(&shared, &watch.extension, EVENT))
            })
        });
        let mut next: Option<f64> = None;
        for watch in &mut monitor.watches {
            let state = state_for(watch.interval, locked, idle);
            if state != watch.reported {
                watch.reported = state;
                changes.push((watch.shared.clone(), watch.extension.clone(), state));
            }
            let wait = match state {
                State::Locked => None,
                State::Idle => Some(RETURN_POLL_SECONDS),
                State::Active => Some((watch.interval as f64 - idle).max(1.0)),
            };
            if let Some(wait) = wait {
                next = Some(next.map_or(wait, |next| next.min(wait)));
            }
        }
        if let Some(timer) = monitor.timer.take() {
            timer.invalidate();
        }
        if monitor.watches.is_empty() {
            let center = NSDistributedNotificationCenter::defaultCenter();
            for observer in monitor.observers.drain(..) {
                unsafe { center.removeObserver(observer.as_ref()) };
            }
        }
        next
    });
    for (shared, extension, state) in changes {
        if let Some(shared) = shared.upgrade() {
            events::emit(&shared, &extension, EVENT, json!([state.name()]), None);
        }
    }
    if let Some(seconds) = next {
        let fire = RcBlock::new(|_timer: NonNull<NSTimer>| {
            MONITOR.with(|monitor| monitor.borrow_mut().timer = None);
            update();
        });
        let timer =
            unsafe { NSTimer::scheduledTimerWithTimeInterval_repeats_block(seconds, false, &fire) };
        MONITOR.with(|monitor| monitor.borrow_mut().timer = Some(timer));
    }
}

fn observe_lock() -> Vec<Retained<ProtocolObject<dyn NSObjectProtocol>>> {
    let center = NSDistributedNotificationCenter::defaultCenter();
    [
        ("com.apple.screenIsLocked", true),
        ("com.apple.screenIsUnlocked", false),
    ]
    .into_iter()
    .map(|(name, locked)| {
        let block = RcBlock::new(move |_note: NonNull<NSNotification>| {
            MONITOR.with(|monitor| monitor.borrow_mut().locked = locked);
            update();
        });
        unsafe {
            center.addObserverForName_object_queue_usingBlock(
                Some(&NSString::from_str(name)),
                None,
                Some(&objc2_foundation::NSOperationQueue::mainQueue()),
                &block,
            )
        }
    })
    .collect()
}
