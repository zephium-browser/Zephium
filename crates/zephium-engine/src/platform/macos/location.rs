//! Core Location behind a page's `navigator.geolocation`.
//!
//! WebKit on macOS has no position source of its own. A page the person
//! allowed waits until an embedder feeds the process pool's geolocation
//! manager, so without this provider every allowed request times out. WebKit
//! starts the provider only for a document its UI delegate allowed, and stops
//! it when no such document still listens.

use std::{
    cell::RefCell,
    ffi::{c_char, c_void, CStr},
    sync::OnceLock,
};

use objc2::{
    define_class, msg_send,
    rc::Retained,
    runtime::{AnyObject, NSObject, ProtocolObject},
    MainThreadOnly,
};
use objc2_core_location::{
    kCLErrorDomain, kCLLocationAccuracyBest, kCLLocationAccuracyHundredMeters,
    CLAuthorizationStatus, CLError, CLLocation, CLLocationManager, CLLocationManagerDelegate,
};
use objc2_foundation::{MainThreadMarker, NSArray, NSError, NSObjectProtocol};
use objc2_web_kit::WKWebView;

type WKTypeRef = *const c_void;
type Callback = extern "C" fn(WKTypeRef, *const c_void);

/// `WKGeolocationProviderV1`: the base's version and client info, then its
/// three callbacks.
#[repr(C)]
struct Provider {
    version: i32,
    client_info: *const c_void,
    start_updating: Callback,
    stop_updating: Callback,
    set_enable_high_accuracy: extern "C" fn(WKTypeRef, bool, *const c_void),
}

// SAFETY: a static table of function pointers and a null client pointer.
unsafe impl Sync for Provider {}

static PROVIDER: Provider = Provider {
    version: 1,
    client_info: std::ptr::null(),
    start_updating,
    stop_updating,
    set_enable_high_accuracy,
};

type Unary = unsafe extern "C" fn(WKTypeRef) -> WKTypeRef;
type SetProvider = unsafe extern "C" fn(WKTypeRef, *const Provider);
type Binary = unsafe extern "C" fn(WKTypeRef, WKTypeRef);
type Position = unsafe extern "C" fn(
    f64,
    f64,
    f64,
    f64,
    bool,
    f64,
    bool,
    f64,
    bool,
    f64,
    bool,
    f64,
) -> WKTypeRef;
type CreateString = unsafe extern "C" fn(*const c_char) -> WKTypeRef;
type Release = unsafe extern "C" fn(WKTypeRef);

/// WebKit's C geolocation API. It is not in the public SDK, so it is looked
/// up at run time; a macOS without one of these keeps today's behaviour, an
/// allowed request that times out, instead of failing to launch.
struct Api {
    manager_for_context: Unary,
    set_provider: SetProvider,
    did_change_position: Binary,
    did_fail: Binary,
    position: Position,
    string: CreateString,
    retain: Unary,
    release: Release,
}

const SYMBOLS: [&CStr; 8] = [
    c"WKContextGetGeolocationManager",
    c"WKGeolocationManagerSetProvider",
    c"WKGeolocationManagerProviderDidChangePosition",
    c"WKGeolocationManagerProviderDidFailToDeterminePositionWithErrorMessage",
    c"WKGeolocationPositionCreate_b",
    c"WKStringCreateWithUTF8CString",
    c"WKRetain",
    c"WKRelease",
];

fn api() -> Option<&'static Api> {
    static API: OnceLock<Option<Api>> = OnceLock::new();
    API.get_or_init(|| {
        let mut found = [std::ptr::null_mut(); SYMBOLS.len()];
        for (slot, name) in found.iter_mut().zip(SYMBOLS) {
            // SAFETY: a NUL-terminated name looked up in the loaded images.
            *slot = unsafe { libc::dlsym(libc::RTLD_DEFAULT, name.as_ptr()) };
            if slot.is_null() {
                eprintln!("engine: WebKit lacks {}", name.to_string_lossy());
                return None;
            }
        }
        // SAFETY: each symbol is WebKit's exported C function with exactly
        // the signature of the field it fills.
        unsafe {
            Some(Api {
                manager_for_context: std::mem::transmute::<*mut c_void, Unary>(found[0]),
                set_provider: std::mem::transmute::<*mut c_void, SetProvider>(found[1]),
                did_change_position: std::mem::transmute::<*mut c_void, Binary>(found[2]),
                did_fail: std::mem::transmute::<*mut c_void, Binary>(found[3]),
                position: std::mem::transmute::<*mut c_void, Position>(found[4]),
                string: std::mem::transmute::<*mut c_void, CreateString>(found[5]),
                retain: std::mem::transmute::<*mut c_void, Unary>(found[6]),
                release: std::mem::transmute::<*mut c_void, Release>(found[7]),
            })
        }
    })
    .as_ref()
}

/// Feeds this view's process pool. Each wry view has its own pool, and a
/// pool that already has the provider simply keeps it.
pub(crate) fn serve(view: &WKWebView) {
    let Some(api) = api() else { return };
    // SAFETY: the configuration's pool is a live WKProcessPool, the very
    // object WebKit's C API takes as a WKContextRef.
    let pool: Retained<AnyObject> = unsafe { msg_send![&*view.configuration(), processPool] };
    let context = Retained::as_ptr(&pool).cast::<c_void>();
    unsafe {
        let manager = (api.manager_for_context)(context);
        if !manager.is_null() {
            (api.set_provider)(manager, &PROVIDER);
        }
    }
}

/// Whether macOS will refuse Zephium a position: Location Services is off,
/// or the person or a profile turned it off for Zephium. Asking the system
/// first is left to the moment a site is allowed, so an undecided Mac is not
/// blocked.
pub(crate) fn blocked_by_system() -> bool {
    let Some(mtm) = MainThreadMarker::new() else {
        return false;
    };
    let Some(status) = with_state(mtm, |state| unsafe { state.manager.authorizationStatus() })
    else {
        return false;
    };
    matches!(
        status,
        CLAuthorizationStatus::Denied | CLAuthorizationStatus::Restricted
    ) || !unsafe { CLLocationManager::locationServicesEnabled_class() }
}

struct Client {
    manager: WKTypeRef,
    high_accuracy: bool,
    /// Retained while WebKit wants positions for it.
    updating: bool,
}

struct State {
    manager: Retained<CLLocationManager>,
    _delegate: Retained<Delegate>,
    clients: Vec<Client>,
    running: bool,
    /// Asked macOS once; it remembers the answer itself.
    asked: bool,
}

thread_local! {
    static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
}

/// None only on reentry, which WebKit and Core Location do not do; these
/// run inside C callbacks, where a panic would abort.
fn with_state<T>(mtm: MainThreadMarker, f: impl FnOnce(&mut State) -> T) -> Option<T> {
    STATE.with(|cell| {
        let mut state = cell.try_borrow_mut().ok()?;
        let state = state.get_or_insert_with(|| {
            let delegate: Retained<Delegate> =
                unsafe { msg_send![super(Delegate::alloc(mtm).set_ivars(())), init] };
            let manager = unsafe { CLLocationManager::new() };
            unsafe { manager.setDelegate(Some(ProtocolObject::from_ref(&*delegate))) };
            State {
                manager,
                _delegate: delegate,
                clients: Vec::new(),
                running: false,
                asked: false,
            }
        });
        Some(f(state))
    })
}

fn client(state: &mut State, manager: WKTypeRef) -> &mut Client {
    let at = match state.clients.iter().position(|c| c.manager == manager) {
        Some(at) => at,
        None => {
            state.clients.push(Client {
                manager,
                high_accuracy: false,
                updating: false,
            });
            state.clients.len() - 1
        }
    };
    &mut state.clients[at]
}

extern "C" fn start_updating(manager: WKTypeRef, _: *const c_void) {
    let (Some(mtm), Some(api)) = (MainThreadMarker::new(), api()) else {
        return;
    };
    with_state(mtm, |state| {
        let client = client(state, manager);
        if !client.updating {
            client.updating = true;
            unsafe { (api.retain)(manager) };
        }
    });
    reconcile(mtm);
}

extern "C" fn stop_updating(manager: WKTypeRef, _: *const c_void) {
    let (Some(mtm), Some(api)) = (MainThreadMarker::new(), api()) else {
        return;
    };
    let removed = with_state(mtm, |state| {
        let at = state.clients.iter().position(|c| c.manager == manager)?;
        Some(state.clients.swap_remove(at))
    })
    .flatten();
    if let Some(Client { updating: true, .. }) = removed {
        unsafe { (api.release)(manager) };
    }
    reconcile(mtm);
}

extern "C" fn set_enable_high_accuracy(manager: WKTypeRef, enabled: bool, _: *const c_void) {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    with_state(mtm, |state| client(state, manager).high_accuracy = enabled);
    reconcile(mtm);
}

#[derive(Debug, PartialEq)]
enum Plan {
    Idle,
    Ask,
    /// Run, with high accuracy when a page asked for it.
    Run(bool),
    Refuse,
}

fn plan(status: CLAuthorizationStatus, wanted: bool, high: bool, asked: bool) -> Plan {
    match status {
        _ if !wanted => Plan::Idle,
        // Asked once: macOS remembers the answer and reports it.
        CLAuthorizationStatus::NotDetermined if asked => Plan::Idle,
        CLAuthorizationStatus::NotDetermined => Plan::Ask,
        CLAuthorizationStatus::AuthorizedAlways | CLAuthorizationStatus::AuthorizedWhenInUse => {
            Plan::Run(high)
        }
        _ => Plan::Refuse,
    }
}

/// Runs Core Location exactly while an allowed page listens and macOS lets
/// Zephium, asking macOS the first time a page needs it.
fn reconcile(mtm: MainThreadMarker) {
    let Some((manager, stop, plan)) = with_state(mtm, |state| {
        let mut listening = state.clients.iter().filter(|c| c.updating).peekable();
        let wanted = listening.peek().is_some();
        let high = listening.any(|c| c.high_accuracy);
        let status = unsafe { state.manager.authorizationStatus() };
        let plan = plan(status, wanted, high, state.asked);
        state.asked |= plan == Plan::Ask;
        let running = matches!(plan, Plan::Run(_));
        let stop = std::mem::replace(&mut state.running, running) && !running;
        (state.manager.clone(), stop, plan)
    }) else {
        return;
    };
    // Core Location is called outside the state borrow, since it may call
    // the delegate back from inside a call.
    if stop {
        unsafe { manager.stopUpdatingLocation() };
    }
    match plan {
        Plan::Idle => {}
        Plan::Ask => unsafe { manager.requestWhenInUseAuthorization() },
        Plan::Run(high) => unsafe {
            manager.setDesiredAccuracy(if high {
                kCLLocationAccuracyBest
            } else {
                kCLLocationAccuracyHundredMeters
            });
            manager.startUpdatingLocation();
        },
        Plan::Refuse => fail(mtm, c"Location Services are off for this browser."),
    }
}

fn listening(mtm: MainThreadMarker) -> Vec<WKTypeRef> {
    with_state(mtm, |state| {
        state
            .clients
            .iter()
            .filter(|c| c.updating)
            .map(|c| c.manager)
            .collect()
    })
    .unwrap_or_default()
}

fn deliver(mtm: MainThreadMarker, location: &CLLocation) {
    let Some(api) = api() else { return };
    let (horizontal, vertical, course, speed) = unsafe {
        (
            location.horizontalAccuracy(),
            location.verticalAccuracy(),
            location.course(),
            location.speed(),
        )
    };
    // A negative accuracy marks an invalid fix.
    if horizontal < 0.0 {
        return;
    }
    let coordinate = unsafe { location.coordinate() };
    let altitude = vertical >= 0.0;
    let position = unsafe {
        (api.position)(
            location.timestamp().timeIntervalSince1970(),
            coordinate.latitude,
            coordinate.longitude,
            horizontal,
            altitude,
            location.altitude(),
            altitude,
            vertical,
            course >= 0.0,
            course,
            speed >= 0.0,
            speed,
        )
    };
    if position.is_null() {
        return;
    }
    for manager in listening(mtm) {
        unsafe { (api.did_change_position)(manager, position) };
    }
    unsafe { (api.release)(position) };
}

fn fail(mtm: MainThreadMarker, message: &CStr) {
    let Some(api) = api() else { return };
    let message = unsafe { (api.string)(message.as_ptr()) };
    if message.is_null() {
        return;
    }
    for manager in listening(mtm) {
        unsafe { (api.did_fail)(manager, message) };
    }
    unsafe { (api.release)(message) };
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "ZephiumGeolocationProvider"]
    struct Delegate;

    unsafe impl NSObjectProtocol for Delegate {}

    unsafe impl CLLocationManagerDelegate for Delegate {
        #[unsafe(method(locationManager:didUpdateLocations:))]
        fn did_update_locations(
            &self,
            _manager: &CLLocationManager,
            locations: &NSArray<CLLocation>,
        ) {
            if let Some(location) = locations.lastObject() {
                deliver(self.mtm(), &location);
            }
        }

        #[unsafe(method(locationManager:didFailWithError:))]
        fn did_fail_with_error(&self, manager: &CLLocationManager, error: &NSError) {
            // Unknown is transient: Core Location keeps trying.
            if &*error.domain() == unsafe { kCLErrorDomain }
                && error.code() == CLError::LocationUnknown.0
            {
                return;
            }
            // Denied before the person answers the system prompt is only the
            // prompt still waiting.
            if unsafe { manager.authorizationStatus() } == CLAuthorizationStatus::NotDetermined {
                return;
            }
            fail(self.mtm(), c"The position is unavailable.");
        }

        #[unsafe(method(locationManagerDidChangeAuthorization:))]
        fn did_change_authorization(&self, _manager: &CLLocationManager) {
            reconcile(self.mtm());
        }
    }
);

#[cfg(test)]
mod tests {
    use super::{plan, CLAuthorizationStatus as Status, Plan};

    #[test]
    fn this_webkit_takes_positions_from_the_browser() {
        assert!(super::api().is_some());
    }

    #[test]
    fn the_delegate_matches_core_locations_protocol() {
        use objc2::ClassType;
        let _ = super::Delegate::class();
    }

    #[test]
    fn core_location_runs_only_while_an_allowed_page_listens() {
        for status in [
            Status::NotDetermined,
            Status::Denied,
            Status::AuthorizedAlways,
        ] {
            assert_eq!(plan(status, false, true, false), Plan::Idle);
        }
        assert_eq!(
            plan(Status::AuthorizedAlways, true, false, true),
            Plan::Run(false)
        );
        assert_eq!(
            plan(Status::AuthorizedWhenInUse, true, true, false),
            Plan::Run(true)
        );
    }

    #[test]
    fn macos_is_asked_once_and_a_refusal_reaches_the_page() {
        assert_eq!(plan(Status::NotDetermined, true, false, false), Plan::Ask);
        assert_eq!(plan(Status::NotDetermined, true, false, true), Plan::Idle);
        assert_eq!(plan(Status::Denied, true, false, true), Plan::Refuse);
        assert_eq!(plan(Status::Restricted, true, false, false), Plan::Refuse);
    }
}
