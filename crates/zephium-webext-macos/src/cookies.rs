//! `cookies.onChanged` with Chrome's details. WebKit raises the event with
//! none, and extensions such as Grammarly read the cookie from it. While an
//! extension listens, the profile's cookie store is watched and compared
//! with the last snapshot after each burst of changes; each change goes to
//! listening extensions that may read that cookie.

use std::cell::RefCell;
use std::collections::HashMap;
use std::ptr::NonNull;
use std::rc::{Rc, Weak as RcWeak};

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::{NSObject, ProtocolObject};
use objc2::{define_class, msg_send, DefinedClass, MainThreadMarker, MainThreadOnly};
use objc2_foundation::{NSArray, NSHTTPCookie, NSObjectProtocol, NSString, NSTimer, NSURL};
use objc2_web_kit::{
    WKHTTPCookieStore, WKHTTPCookieStoreObserver, WKWebExtensionPermission,
    WKWebExtensionPermissionCookies,
};
use serde_json::{json, Value};

use crate::events;
use crate::runtime::Shared;

pub(crate) const EVENT: &str = "cookies.onChanged";
/// Pages set cookies in bursts; one comparison covers a burst.
const SETTLE_SECONDS: f64 = 1.5;
const MAX_COOKIES: usize = 20_000;

type Key = (String, String, String);

#[derive(Default)]
pub(crate) struct Watch {
    observer: RefCell<Option<Retained<Observer>>>,
    snapshot: RefCell<Option<HashMap<Key, Value>>>,
    timer: RefCell<Option<Retained<NSTimer>>>,
}

pub(crate) struct Ivars {
    shared: RcWeak<Shared>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "ZephiumWebExtCookieObserver"]
    #[ivars = Ivars]
    pub(crate) struct Observer;

    unsafe impl NSObjectProtocol for Observer {}

    unsafe impl WKHTTPCookieStoreObserver for Observer {
        #[unsafe(method(cookiesDidChangeInCookieStore:))]
        fn changed(&self, _store: &WKHTTPCookieStore) {
            if let Some(shared) = self.ivars().shared.upgrade() {
                schedule(&shared);
            }
        }
    }
);

impl Observer {
    fn new(mtm: MainThreadMarker, shared: RcWeak<Shared>) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(Ivars { shared });
        unsafe { msg_send![super(this), init] }
    }
}

fn store(shared: &Shared) -> Retained<WKHTTPCookieStore> {
    unsafe { shared.store.httpCookieStore() }
}

/// Starts watching when an extension listens and stops when none does.
pub(crate) fn refresh(shared: &Rc<Shared>) {
    let wanted = shared
        .loaded
        .borrow()
        .keys()
        .any(|id| events::listened(shared, id, EVENT));
    let watching = shared.cookies.observer.borrow().is_some();
    if wanted && !watching {
        let observer = Observer::new(shared.mtm, Rc::downgrade(shared));
        unsafe { store(shared).addObserver(ProtocolObject::from_ref(&*observer)) };
        *shared.cookies.observer.borrow_mut() = Some(observer);
        compare(shared);
    } else if !wanted && watching {
        if let Some(observer) = shared.cookies.observer.borrow_mut().take() {
            unsafe { store(shared).removeObserver(ProtocolObject::from_ref(&*observer)) };
        }
        if let Some(timer) = shared.cookies.timer.borrow_mut().take() {
            timer.invalidate();
        }
        shared.cookies.snapshot.borrow_mut().take();
    }
}

fn schedule(shared: &Rc<Shared>) {
    if shared.cookies.timer.borrow().is_some() {
        return;
    }
    let weak = Rc::downgrade(shared);
    let fire = RcBlock::new(move |_timer: NonNull<NSTimer>| {
        if let Some(shared) = weak.upgrade() {
            shared.cookies.timer.borrow_mut().take();
            compare(&shared);
        }
    });
    let timer = unsafe {
        NSTimer::scheduledTimerWithTimeInterval_repeats_block(SETTLE_SECONDS, false, &fire)
    };
    *shared.cookies.timer.borrow_mut() = Some(timer);
}

fn compare(shared: &Rc<Shared>) {
    let weak = Rc::downgrade(shared);
    let done = RcBlock::new(move |cookies: NonNull<NSArray<NSHTTPCookie>>| {
        let Some(shared) = weak.upgrade() else { return };
        if shared.cookies.observer.borrow().is_none() {
            return;
        }
        let current: HashMap<Key, Value> = unsafe { cookies.as_ref() }
            .iter()
            .take(MAX_COOKIES)
            .map(|cookie| describe(&cookie))
            .collect();
        let previous = shared.cookies.snapshot.borrow_mut().take();
        if let Some(previous) = &previous {
            deliver(&shared, previous, &current);
        }
        // Delivery can end the last listener, which stops the watch.
        if shared.cookies.observer.borrow().is_some() {
            *shared.cookies.snapshot.borrow_mut() = Some(current);
        }
    });
    unsafe { store(shared).getAllCookies(&done) };
}

fn describe(cookie: &NSHTTPCookie) -> (Key, Value) {
    let name = cookie.name().to_string();
    let domain = cookie.domain().to_string();
    let path = cookie.path().to_string();
    let same_site = match cookie
        .sameSitePolicy()
        .map(|policy| policy.to_string())
        .as_deref()
    {
        Some("lax") => "lax",
        Some("strict") => "strict",
        Some(_) => "no_restriction",
        None => "unspecified",
    };
    let mut value = json!({
        "name": name,
        "value": cookie.value().to_string(),
        "domain": domain,
        "hostOnly": !domain.starts_with('.'),
        "path": path,
        "secure": cookie.isSecure(),
        "httpOnly": cookie.isHTTPOnly(),
        "sameSite": same_site,
        "session": cookie.isSessionOnly(),
        "storeId": "0",
    });
    if let Some(expires) = cookie.expiresDate() {
        value["expirationDate"] = json!(expires.timeIntervalSince1970());
    }
    ((name, domain, path), value)
}

fn deliver(shared: &Rc<Shared>, previous: &HashMap<Key, Value>, current: &HashMap<Key, Value>) {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0.0, |elapsed| elapsed.as_secs_f64());
    let mut changes = Vec::new();
    for (key, old) in previous {
        match current.get(key) {
            None => {
                let expired = old["expirationDate"].as_f64().is_some_and(|at| at <= now);
                changes.push((old, true, if expired { "expired" } else { "explicit" }));
            }
            Some(new) if new != old => {
                changes.push((old, true, "overwrite"));
                changes.push((new, false, "explicit"));
            }
            Some(_) => {}
        }
    }
    for (key, new) in current {
        if !previous.contains_key(key) {
            changes.push((new, false, "explicit"));
        }
    }
    if changes.is_empty() {
        return;
    }
    let listeners: Vec<(String, Retained<objc2_web_kit::WKWebExtensionContext>)> = shared
        .loaded
        .borrow()
        .iter()
        .filter(|(id, _)| events::listened(shared, id, EVENT))
        .map(|(id, loaded)| (id.clone(), loaded.context.clone()))
        .collect();
    let permission: Option<&WKWebExtensionPermission> = unsafe { WKWebExtensionPermissionCookies };
    for (id, context) in listeners {
        if !permission.is_some_and(|permission| unsafe { context.hasPermission(permission) }) {
            continue;
        }
        for (cookie, removed, cause) in &changes {
            let host = cookie["domain"]
                .as_str()
                .unwrap_or("")
                .trim_start_matches('.');
            let path = cookie["path"].as_str().unwrap_or("/");
            let Some(url) =
                NSURL::URLWithString(&NSString::from_str(&format!("https://{host}{path}")))
            else {
                continue;
            };
            if unsafe { context.hasAccessToURL(&url) } {
                let args = json!([{ "removed": removed, "cookie": cookie, "cause": cause }]);
                events::emit(shared, &id, EVENT, args, None);
            }
        }
    }
}
