//! Native invariants: WebKit invokes every entry on its web-process main loop.
//! Registry pointers are borrowed GObjects retired by weak notifications; a strong
//! ref is taken before calling native code. No RefCell borrow spans JSC, signals,
//! or unref. JSC contexts/values with full transfer are released in scope order.
//! Every C entry aborts on panic, including destroy and completion callbacks.

mod raw;
mod scripts;

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::ffi::{c_char, c_int, c_void, CString};
use std::ptr;
use std::rc::{Rc, Weak as RcWeak};
use std::sync::{Arc, Weak};

use crate::dnr::{DnrFrameAllow, DnrPolicySnapshot, MAX_DNR_SNAPSHOT_BYTES};
use crate::policy::{Decision, PolicySnapshot, RequestInfo, ResourceType};
use crate::protocol::{self, Attach, Deliver, Event, FormEvent, RequestEvent, UiMessage};
use crate::state::State;
use raw::{Object, Variant};

const MAX_NATIVE_PAGES: usize = 256;
const MAX_NATIVE_WORLDS: usize = 256;
const MAX_PENDING_EVENTS: usize = 64;
const MAX_PENDING_EVENT_BYTES: usize = 1024 * 1024;
const MAX_PROCESS_SCRIPT_BYTES: usize = 64 * 1024 * 1024;
const FORM_WORLD: &str = "zephium-web-process-form-v1";
const FACT_WORLD: &str = "zephium-web-process-facts-v2";
const UINT8_ARRAY: c_int = 5;
const G_TYPE_BOOLEAN: usize = 5 << 2;

thread_local! {
    static RUNTIME: RefCell<Option<Runtime>> = const { RefCell::new(None) };
}

struct Runtime {
    state: State,
    pages: BTreeMap<u64, NativePage>,
    worlds: BTreeMap<String, *mut Object>,
    timer_active: bool,
    event_inflight_bytes: usize,
    event_inflight_count: usize,
    injections: BTreeMap<u64, scripts::PendingInjection>,
    next_injection: u64,
    script_tables: BTreeMap<u64, (Weak<protocol::ScriptTable>, usize)>,
    policy_maps: BTreeMap<(u64, u64), RcWeak<PublishedPolicy>>,
}

struct NativePage {
    pointer: *mut Object,
    frames: BTreeMap<u64, NativeFrame>,
    policy: Option<Rc<PublishedPolicy>>,
    frame_allow: Vec<DnrFrameAllow>,
    pending_allow: Option<(String, Vec<DnrFrameAllow>)>,
    policy_generation: u64,
    requests: Vec<RequestEvent>,
    request_bytes: usize,
    requests_dropped: u64,
    matches_dropped: u64,
    events: VecDeque<Vec<u8>>,
    event_bytes: usize,
    batch_inflight: bool,
    form_connected: bool,
    scripts: Option<Arc<protocol::ScriptTable>>,
}

struct NativeFrame {
    pointer: *mut Object,
    weak_data: *mut (u64, u64),
    installed: BTreeSet<String>,
    receivers: BTreeMap<String, *mut Object>,
    scripts: Option<Arc<protocol::ScriptTable>>,
    phases: [bool; 3],
    bootstrapped: BTreeMap<String, bool>,
}

#[derive(Clone)]
struct Binding {
    page: u64,
    frame: u64,
    document_seq: u64,
    world: String,
    external_install: Option<String>,
}

struct Completion {
    page: u64,
    generation: u64,
    post_id: Option<u64>,
    event_bytes: usize,
    batch: bool,
}

struct PanicGuard;
impl Drop for PanicGuard {
    fn drop(&mut self) {
        if std::thread::panicking() {
            std::process::abort();
        }
    }
}
fn boundary<T>(f: impl FnOnce() -> T) -> T {
    let guard = PanicGuard;
    let result = f();
    std::mem::forget(guard);
    result
}

struct Owned(*mut Object);
impl Owned {
    fn borrowed(pointer: *mut Object) -> Option<Self> {
        if pointer.is_null() {
            None
        } else {
            Some(Self(unsafe { raw::g_object_ref(pointer) }))
        }
    }
    fn full(pointer: *mut Object) -> Option<Self> {
        if pointer.is_null() {
            None
        } else {
            Some(Self(pointer))
        }
    }
}
impl Drop for Owned {
    fn drop(&mut self) {
        unsafe { raw::g_object_unref(self.0) }
    }
}

fn with_runtime<T>(f: impl FnOnce(&mut Runtime) -> T) -> Option<T> {
    RUNTIME
        .try_with(|cell| cell.borrow_mut().as_mut().map(f))
        .ok()
        .flatten()
}

fn callback(pointer: *const ()) -> raw::Callback {
    // Linux's GLib GCallback is an erased C function pointer, restored by GLib.
    Some(unsafe { std::mem::transmute::<*const (), unsafe extern "C" fn()>(pointer) })
}
fn connect(
    object: *mut Object,
    signal: &'static std::ffi::CStr,
    function: *const (),
    data: *mut c_void,
) {
    unsafe {
        raw::g_signal_connect_data(object, signal.as_ptr(), callback(function), data, None, 0);
    }
}
fn unref_all(objects: Vec<*mut Object>) {
    for object in objects {
        if !object.is_null() {
            unsafe {
                raw::g_object_unref(object);
            }
        }
    }
}
fn release_frames(frames: BTreeMap<u64, NativeFrame>) {
    // Remove all watchers before any unref can reenter through a weak callback.
    for frame in frames.values() {
        if !frame.weak_data.is_null() {
            unsafe {
                raw::g_object_weak_unref(
                    frame.pointer,
                    Some(frame_destroyed),
                    frame.weak_data.cast(),
                );
                drop(Box::from_raw(frame.weak_data));
            }
        }
    }
    unref_all(
        frames
            .into_values()
            .flat_map(|frame| frame.receivers.into_values())
            .collect(),
    );
}

fn bounded_text(pointer: *const c_char, cap: usize) -> Option<String> {
    if pointer.is_null() {
        return None;
    }
    unsafe {
        for len in 0..=cap {
            if *pointer.add(len) == 0 {
                return std::str::from_utf8(std::slice::from_raw_parts(pointer.cast(), len))
                    .ok()
                    .map(str::to_owned);
            }
        }
    }
    None
}

/// WebKit owns both arguments for the duration of initialization.
#[no_mangle]
pub unsafe extern "C" fn webkit_web_extension_initialize_with_user_data(
    extension: *mut Object,
    data: *mut Variant,
) {
    boundary(|| unsafe {
        if extension.is_null()
            || data.is_null()
            || raw::webkit_get_major_version() != 2
            || raw::webkit_get_minor_version() < 54
            || !raw::webkit_get_minor_version().is_multiple_of(2)
            || raw::g_variant_is_of_type(data, c"(us)".as_ptr()) == 0
        {
            return;
        }
        let version = raw::g_variant_get_child_value(data, 0);
        let token = raw::g_variant_get_child_value(data, 1);
        let protocol_version = raw::g_variant_get_uint32(version);
        let mut token_len = 0;
        let token_ptr = raw::g_variant_get_string(token, &mut token_len);
        let secret = if token_len <= protocol::MAX_TOKEN_BYTES {
            bounded_text(token_ptr, protocol::MAX_TOKEN_BYTES)
        } else {
            None
        };
        raw::g_variant_unref(version);
        raw::g_variant_unref(token);
        let Some(secret) = secret else {
            return;
        };
        if protocol_version != protocol::PROTOCOL_VERSION || secret.is_empty() {
            return;
        }
        let state = State::new(protocol_version, secret);
        if !state.is_initialized() {
            return;
        }
        let initialized = RUNTIME.with(|cell| {
            let mut runtime = cell.borrow_mut();
            if runtime.is_some() {
                return false;
            }
            *runtime = Some(Runtime {
                state,
                pages: BTreeMap::new(),
                worlds: BTreeMap::new(),
                timer_active: false,
                event_inflight_bytes: 0,
                event_inflight_count: 0,
                injections: BTreeMap::new(),
                next_injection: 0,
                script_tables: BTreeMap::new(),
                policy_maps: BTreeMap::new(),
            });
            true
        });
        if !initialized {
            return;
        }
        let default_world = raw::webkit_script_world_get_default();
        with_runtime(|runtime| {
            runtime.worlds.insert(String::new(), default_world);
        });
        connect(
            default_world,
            c"window-object-cleared",
            window_cleared as *const (),
            Box::into_raw(Box::new(String::new())).cast(),
        );
        connect(
            extension,
            c"page-created",
            page_created as *const (),
            ptr::null_mut(),
        );
    });
}

unsafe extern "C" fn page_created(_: *mut Object, page: *mut Object, _: *mut c_void) {
    boundary(|| unsafe {
        let id = raw::webkit_web_page_get_id(page);
        let added = with_runtime(|runtime| {
            if runtime.pages.len() >= MAX_NATIVE_PAGES || runtime.pages.contains_key(&id) {
                return false;
            }
            runtime.pages.insert(
                id,
                NativePage {
                    pointer: page,
                    frames: BTreeMap::new(),
                    policy: None,
                    frame_allow: Vec::new(),
                    pending_allow: None,
                    policy_generation: 0,
                    requests: Vec::new(),
                    request_bytes: 0,
                    requests_dropped: 0,
                    matches_dropped: 0,
                    events: VecDeque::new(),
                    event_bytes: 0,
                    batch_inflight: false,
                    form_connected: false,
                    scripts: None,
                },
            );
            true
        })
        .unwrap_or(false);
        if !added {
            return;
        }
        raw::g_object_weak_ref(
            page,
            Some(page_destroyed),
            Box::into_raw(Box::new(id)).cast(),
        );
        connect(
            page,
            c"user-message-received",
            receive_message as *const (),
            ptr::null_mut(),
        );
        connect(
            page,
            c"send-request",
            send_request as *const (),
            ptr::null_mut(),
        );
        connect(
            page,
            c"document-loaded",
            document_loaded as *const (),
            ptr::null_mut(),
        );
    });
}

unsafe extern "C" fn page_destroyed(data: *mut c_void, _: *mut Object) {
    boundary(|| unsafe {
        let id = *Box::from_raw(data.cast::<u64>());
        let removed = with_runtime(|runtime| {
            runtime.state.forget_page(id);
            runtime.pages.remove(&id)
        })
        .flatten();
        if let Some(page) = removed {
            release_frames(page.frames);
        }
    });
}
unsafe extern "C" fn frame_destroyed(data: *mut c_void, pointer: *mut Object) {
    boundary(|| unsafe {
        let (page, frame) = *Box::from_raw(data.cast::<(u64, u64)>());
        let removed = with_runtime(|runtime| {
            let native = runtime.pages.get_mut(&page)?;
            if native.frames.get(&frame)?.pointer != pointer {
                return None;
            }
            runtime.state.remove_frame(page, frame);
            let mut removed = native.frames.remove(&frame)?;
            removed.weak_data = ptr::null_mut();
            Some(removed)
        })
        .flatten();
        if let Some(frame) = removed {
            unref_all(frame.receivers.into_values().collect());
        }
    });
}

fn ensure_world(name: &str) -> Option<*mut Object> {
    if let Some(Some(world)) = with_runtime(|runtime| runtime.worlds.get(name).copied()) {
        return Some(world);
    }
    if !with_runtime(|runtime| runtime.worlds.len() < MAX_NATIVE_WORLDS).unwrap_or(false) {
        return None;
    }
    let name_c = CString::new(name).ok()?;
    let world = unsafe { raw::webkit_script_world_new_with_name(name_c.as_ptr()) };
    if world.is_null() {
        return None;
    }
    with_runtime(|runtime| {
        runtime.worlds.insert(name.to_owned(), world);
    });
    if name != FORM_WORLD && name != FACT_WORLD {
        connect(
            world,
            c"window-object-cleared",
            window_cleared as *const (),
            Box::into_raw(Box::new(name.to_owned())).cast(),
        );
    }
    Some(world)
}

fn queue_event(page: u64, event: Event) {
    let Ok(bytes) = protocol::encode_event(&event) else {
        return;
    };
    with_runtime(|runtime| {
        let Some(page) = runtime.pages.get_mut(&page) else {
            return;
        };
        if page.events.len() >= MAX_PENDING_EVENTS
            || page.event_bytes.saturating_add(bytes.len()) > MAX_PENDING_EVENT_BYTES
        {
            return;
        }
        page.event_bytes += bytes.len();
        page.events.push_back(bytes);
    });
}

fn start_timer() {
    if with_runtime(|runtime| {
        if runtime.timer_active {
            false
        } else {
            runtime.timer_active = true;
            true
        }
    })
    .unwrap_or(false)
    {
        unsafe {
            raw::g_timeout_add_full(0, 250, Some(flush_events), ptr::null_mut(), None);
        }
    }
}

fn flush_frame_events(id: u64) -> bool {
    loop {
        let next = with_runtime(|runtime| {
            if runtime.event_inflight_count >= 128
                || runtime.event_inflight_bytes >= 15 * 1024 * 1024
            {
                return Err(());
            }
            let generation = runtime.state.attachment(id).ok_or(())?.generation;
            let events = runtime.state.drain_frame_events(id);
            if events.is_empty() {
                return Ok(None);
            }
            let bytes =
                protocol::encode_event(&Event::Frames { generation, events }).map_err(|_| ())?;
            let native = runtime.pages.get(&id).ok_or(())?;
            runtime.event_inflight_count += 1;
            runtime.event_inflight_bytes += bytes.len();
            Ok(Some((native.pointer, generation, bytes)))
        });
        let Some(Ok(next)) = next else {
            return false;
        };
        let Some((pointer, generation, bytes)) = next else {
            return true;
        };
        let Some(page) = Owned::borrowed(pointer) else {
            return false;
        };
        let event_bytes = bytes.len();
        send_bytes(
            &page,
            bytes,
            Completion {
                page: id,
                generation,
                post_id: None,
                event_bytes,
                batch: false,
            },
        );
    }
}

fn send_event_immediately(id: u64, event: Event) -> bool {
    let Ok(bytes) = protocol::encode_event(&event) else {
        return false;
    };
    let next = with_runtime(|runtime| {
        if runtime.event_inflight_count >= 128
            || runtime.event_inflight_bytes.saturating_add(bytes.len()) > 16 * 1024 * 1024
        {
            return None;
        }
        let generation = runtime.state.attachment(id)?.generation;
        let pointer = runtime.pages.get(&id)?.pointer;
        runtime.event_inflight_count += 1;
        runtime.event_inflight_bytes += bytes.len();
        Some((pointer, generation))
    })
    .flatten();
    let Some((pointer, generation)) = next else {
        return false;
    };
    let Some(page) = Owned::borrowed(pointer) else {
        return false;
    };
    let event_bytes = bytes.len();
    send_bytes(
        &page,
        bytes,
        Completion {
            page: id,
            generation,
            post_id: None,
            event_bytes,
            batch: false,
        },
    );
    true
}

fn send_bytes(page: &Owned, bytes: Vec<u8>, completion: Completion) {
    unsafe {
        let owned = Box::new(bytes);
        let parameters = raw::g_variant_new_from_data(
            c"ay".as_ptr(),
            owned.as_ptr().cast(),
            owned.len(),
            1,
            Some(bytes_destroyed),
            Box::into_raw(owned).cast(),
        );
        let message = raw::webkit_user_message_new(c"zephium-webext-event".as_ptr(), parameters);
        raw::webkit_web_page_send_message_to_view(
            page.0,
            message,
            ptr::null_mut(),
            Some(message_finished),
            Box::into_raw(Box::new(completion)).cast(),
        );
    }
}

unsafe extern "C" fn bytes_destroyed(data: *mut c_void) {
    boundary(|| unsafe {
        drop(Box::from_raw(data.cast::<Vec<u8>>()));
    });
}

unsafe extern "C" fn message_finished(page: *mut Object, result: *mut Object, data: *mut c_void) {
    boundary(|| unsafe {
        let completion = Box::from_raw(data.cast::<Completion>());
        let mut error = ptr::null_mut();
        let reply = raw::webkit_web_page_send_message_to_view_finish(page, result, &mut error);
        if !reply.is_null() {
            raw::g_object_unref(reply);
        }
        if !error.is_null() {
            raw::g_error_free(error);
        }
        with_runtime(|runtime| {
            if completion.event_bytes != 0 {
                runtime.event_inflight_bytes = runtime
                    .event_inflight_bytes
                    .saturating_sub(completion.event_bytes);
                runtime.event_inflight_count = runtime.event_inflight_count.saturating_sub(1);
            }
            if let Some(post) = completion.post_id {
                runtime.state.settle_post(post);
            } else if completion.batch
                && runtime
                    .state
                    .attachment(completion.page)
                    .is_some_and(|attach| attach.generation == completion.generation)
            {
                if let Some(page) = runtime.pages.get_mut(&completion.page) {
                    page.batch_inflight = false;
                }
            }
        });
    });
}

unsafe extern "C" fn flush_events(_: *mut c_void) -> c_int {
    boundary(|| {
        let work = with_runtime(|runtime| {
            let ids: Vec<_> = runtime.pages.keys().copied().collect();
            let mut work = Vec::new();
            let mut attached = false;
            for id in ids {
                let Some(attach) = runtime.state.attachment(id) else {
                    continue;
                };
                attached = true;
                let generation = attach.generation;
                let Some(native) = runtime.pages.get_mut(&id) else {
                    continue;
                };
                if native.batch_inflight {
                    continue;
                }
                let frames = if native.events.is_empty() {
                    runtime.state.drain_frame_events(id)
                } else {
                    Vec::new()
                };
                if !frames.is_empty() {
                    if let Ok(bytes) = protocol::encode_event(&Event::Frames {
                        generation,
                        events: frames,
                    }) {
                        if native.event_bytes.saturating_add(bytes.len()) <= MAX_PENDING_EVENT_BYTES
                            && native.events.len() < MAX_PENDING_EVENTS
                        {
                            native.event_bytes += bytes.len();
                            native.events.push_back(bytes);
                        }
                    }
                }
                if native.events.is_empty()
                    && (!native.requests.is_empty() || native.requests_dropped != 0)
                {
                    let observations = std::mem::take(&mut native.requests);
                    let row_count = observations.len() as u64;
                    let match_count = observations.iter().filter(|row| row.matched).count() as u64;
                    native.request_bytes = 0;
                    let dropped_observations = std::mem::take(&mut native.requests_dropped);
                    let dropped_matches = std::mem::take(&mut native.matches_dropped);
                    let bytes = protocol::encode_event(&Event::Requests {
                        generation,
                        observations,
                        dropped_observations,
                        dropped_matches,
                    });
                    if let Ok(bytes) = bytes {
                        if bytes.len() <= MAX_PENDING_EVENT_BYTES {
                            native.event_bytes += bytes.len();
                            native.events.push_back(bytes);
                        } else {
                            native.requests_dropped =
                                dropped_observations.saturating_add(row_count);
                            native.matches_dropped = dropped_matches.saturating_add(match_count);
                        }
                    } else {
                        native.requests_dropped = dropped_observations.saturating_add(row_count);
                        native.matches_dropped = dropped_matches.saturating_add(match_count);
                    }
                }
                let next_size = native.events.front().map_or(0, Vec::len);
                if runtime.event_inflight_count >= 128
                    || runtime.event_inflight_bytes.saturating_add(next_size) > 16 * 1024 * 1024
                {
                    continue;
                }
                if let Some(bytes) = native.events.pop_front() {
                    runtime.event_inflight_count += 1;
                    runtime.event_inflight_bytes += bytes.len();
                    native.event_bytes -= bytes.len();
                    native.batch_inflight = true;
                    work.push((id, generation, native.pointer, bytes));
                }
            }
            if !attached {
                runtime.timer_active = false;
            }
            (attached, work)
        });
        let Some((attached, work)) = work else {
            return 0;
        };
        for (id, generation, pointer, bytes) in work {
            if let Some(page) = Owned::borrowed(pointer) {
                let event_bytes = bytes.len();
                send_bytes(
                    &page,
                    bytes,
                    Completion {
                        page: id,
                        generation,
                        post_id: None,
                        event_bytes,
                        batch: true,
                    },
                );
            }
        }
        c_int::from(attached)
    })
}

unsafe extern "C" fn receive_message(
    page: *mut Object,
    message: *mut Object,
    _: *mut c_void,
) -> c_int {
    boundary(|| unsafe {
        let Some(name) = bounded_text(raw::webkit_user_message_get_name(message), 64) else {
            return 0;
        };
        if ![
            protocol::ATTACH_MESSAGE,
            protocol::DELIVER_MESSAGE,
            protocol::DETACH_MESSAGE,
            protocol::POLICY_MESSAGE,
            protocol::SCRIPTS_MESSAGE,
            protocol::INJECT_MESSAGE,
        ]
        .contains(&name.as_str())
        {
            return 0;
        }
        let parameters = raw::webkit_user_message_get_parameters(message);
        let mut delivery_status =
            (name == protocol::DELIVER_MESSAGE).then_some(protocol::DeliveryStatus::Denied);
        let accepted = if parameters.is_null()
            || raw::g_variant_is_of_type(parameters, c"ay".as_ptr()) == 0
            || raw::g_variant_get_size(parameters) > protocol::MAX_UI_BYTES
        {
            false
        } else {
            let mut length = 0;
            let data = raw::g_variant_get_fixed_array(parameters, &mut length, 1);
            let bytes = if length == 0 {
                Some(&[][..])
            } else if data.is_null() || length > protocol::MAX_UI_BYTES {
                None
            } else {
                Some(std::slice::from_raw_parts(data.cast::<u8>(), length))
            };
            bytes
                .and_then(|bytes| protocol::decode_ui(&name, bytes).ok())
                .is_some_and(|input| {
                    if let UiMessage::Deliver(deliver) = input {
                        let status = deliver_to_frame(raw::webkit_web_page_get_id(page), &deliver);
                        delivery_status = Some(status);
                        status == protocol::DeliveryStatus::Delivered
                    } else {
                        handle_message(page, message, input)
                    }
                })
        };
        if !accepted && name == protocol::ATTACH_MESSAGE {
            deactivate_page(raw::webkit_web_page_get_id(page));
        }
        if let Ok(name) = CString::new(name) {
            let parameters = if let Some(status) = delivery_status {
                let bytes = Box::new(protocol::encode_delivery_result(status).unwrap_or_default());
                raw::g_variant_new_from_data(
                    c"ay".as_ptr(),
                    bytes.as_ptr().cast(),
                    bytes.len(),
                    1,
                    Some(bytes_destroyed),
                    Box::into_raw(bytes).cast(),
                )
            } else {
                raw::g_variant_new_boolean(c_int::from(accepted))
            };
            raw::webkit_user_message_send_reply(
                message,
                raw::webkit_user_message_new(name.as_ptr(), parameters),
            );
        }
        1
    })
}

fn handle_message(page: *mut Object, message: *mut Object, input: UiMessage) -> bool {
    let id = unsafe { raw::webkit_web_page_get_id(page) };
    match input {
        UiMessage::Attach(attach) => attach_page(id, page, attach),
        UiMessage::Detach(detach) => {
            let removed = with_runtime(|runtime| {
                if !runtime.state.detach(id, &detach) {
                    return None;
                }
                let native = runtime.pages.get_mut(&id)?;
                native.scripts = None;
                native.policy = None;
                native.frame_allow.clear();
                native.pending_allow = None;
                native.policy_generation = 0;
                native.requests.clear();
                native.request_bytes = 0;
                native.requests_dropped = 0;
                native.matches_dropped = 0;
                native.events.clear();
                native.event_bytes = 0;
                native.batch_inflight = false;
                Some(std::mem::take(&mut native.frames))
            })
            .flatten();
            if let Some(frames) = removed {
                release_frames(frames);
                true
            } else {
                false
            }
        }
        UiMessage::Deliver(deliver) => {
            deliver_to_frame(id, &deliver) == protocol::DeliveryStatus::Delivered
        }
        UiMessage::Inject(request) => scripts::inject(id, request),
        UiMessage::Scripts(update) => {
            if !with_runtime(|runtime| {
                runtime
                    .state
                    .validate_message(id, &update.token, update.generation)
                    && runtime.pages.get(&id).is_some_and(|page| {
                        page.scripts
                            .as_ref()
                            .is_none_or(|table| update.table_generation > table.generation)
                    })
            })
            .unwrap_or(false)
            {
                return false;
            }
            let Some(table) = map_sealed(
                message,
                update.fd_index,
                update.length as usize,
                protocol::MAX_SCRIPT_TABLE_BYTES,
                protocol::ScriptTable::decode,
            ) else {
                return false;
            };
            if table.generation != update.table_generation {
                return false;
            }
            with_runtime(|runtime| {
                if !runtime
                    .state
                    .validate_message(id, &update.token, update.generation)
                {
                    return false;
                }
                runtime
                    .script_tables
                    .retain(|_, (table, _)| table.strong_count() != 0);
                let shared =
                    if let Some((existing, _)) = runtime.script_tables.get(&table.generation) {
                        let Some(existing) = existing.upgrade() else {
                            return false;
                        };
                        if *existing != table {
                            return false;
                        }
                        existing
                    } else {
                        let bytes = runtime
                            .script_tables
                            .values()
                            .map(|(_, bytes)| bytes)
                            .sum::<usize>();
                        if bytes.saturating_add(update.length as usize) > MAX_PROCESS_SCRIPT_BYTES {
                            return false;
                        }
                        let table = Arc::new(table);
                        runtime.script_tables.insert(
                            table.generation,
                            (Arc::downgrade(&table), update.length as usize),
                        );
                        table
                    };
                let Some(page) = runtime.pages.get_mut(&id) else {
                    return false;
                };
                page.scripts = Some(shared);
                true
            })
            .unwrap_or(false)
        }
        UiMessage::Policy(update) => {
            if !with_runtime(|runtime| {
                runtime
                    .state
                    .validate_message(id, &update.token, update.generation)
                    && runtime
                        .pages
                        .get(&id)
                        .is_some_and(|page| update.policy_generation > page.policy_generation)
            })
            .unwrap_or(false)
            {
                return false;
            }
            let Some(policy) = map_policy(message, update.fd_index, update.length as usize) else {
                return false;
            };
            with_runtime(|runtime| {
                if !runtime
                    .state
                    .validate_message(id, &update.token, update.generation)
                {
                    return false;
                }
                let Some(page) = runtime.pages.get_mut(&id) else {
                    return false;
                };
                if update.policy_generation <= page.policy_generation {
                    return false;
                }
                page.policy = Some(policy);
                page.frame_allow.clear();
                page.pending_allow = None;
                page.policy_generation = update.policy_generation;
                true
            })
            .unwrap_or(false)
        }
    }
}

fn attach_page(id: u64, pointer: *mut Object, attach: Attach) -> bool {
    let generation = attach.generation;
    let worlds: Vec<_> = attach
        .worlds
        .iter()
        .map(|world| world.name.clone())
        .collect();
    let observe_forms = attach.observe_forms;
    let capacity = with_runtime(|runtime| {
        let missing = worlds
            .iter()
            .filter(|world| !runtime.worlds.contains_key(*world))
            .count();
        runtime
            .worlds
            .len()
            .saturating_add(missing)
            .saturating_add(usize::from(observe_forms))
            .saturating_add(usize::from(!runtime.worlds.contains_key(FACT_WORLD)))
            <= MAX_NATIVE_WORLDS
    })
    .unwrap_or(false);
    if !capacity {
        return false;
    }
    let removed = with_runtime(|runtime| {
        let preserve = runtime.state.attachment(id).is_some_and(|old| {
            old.generation == attach.generation && crate::state::attachment_widens(old, &attach)
        });
        runtime.state.attach(id, attach).ok()?;
        let native = runtime.pages.get_mut(&id)?;
        if preserve {
            return Some(BTreeMap::new());
        }
        native.scripts = None;
        native.policy = None;
        native.frame_allow.clear();
        native.pending_allow = None;
        native.policy_generation = 0;
        native.requests.clear();
        native.request_bytes = 0;
        native.requests_dropped = 0;
        native.matches_dropped = 0;
        native.events.clear();
        native.event_bytes = 0;
        native.batch_inflight = false;
        Some(std::mem::take(&mut native.frames))
    })
    .flatten();
    let Some(frames) = removed else {
        return false;
    };
    release_frames(frames);
    for world in worlds {
        if ensure_world(&world).is_none() {
            return false;
        }
    }
    if observe_forms {
        if let Some(world) = ensure_world(FORM_WORLD) {
            let manager = unsafe { raw::webkit_web_page_get_form_manager(pointer, world) };
            // A manager survives repeated attaches; its marker avoids duplicate hooks.
            let hook = with_runtime(|runtime| {
                runtime.pages.get_mut(&id).is_some_and(|native| {
                    if native.form_connected {
                        false
                    } else {
                        native.form_connected = true;
                        true
                    }
                })
            })
            .unwrap_or(false);
            if hook {
                unsafe {
                    raw::g_signal_connect_data(
                        manager,
                        c"will-submit-form".as_ptr(),
                        callback(will_submit_form as *const ()),
                        Box::into_raw(Box::new(id)).cast(),
                        Some(form_hook_destroyed),
                        0,
                    );
                }
            }
        }
    }
    queue_event(id, Event::Attached { generation });
    if ensure_world(FACT_WORLD).is_none() {
        return false;
    }
    flush_frame_events(id);
    start_timer();
    true
}

unsafe extern "C" fn window_cleared(
    _: *mut Object,
    page: *mut Object,
    frame: *mut Object,
    data: *mut c_void,
) {
    boundary(|| unsafe {
        let name = &*data.cast::<String>();
        if !name.is_empty() {
            return;
        }
        let page_id = raw::webkit_web_page_get_id(page);
        let frame_id = raw::webkit_frame_get_id(frame);
        let Some(uri) = bounded_text(raw::webkit_frame_get_uri(frame), protocol::MAX_URI_BYTES)
        else {
            return;
        };
        let is_main = raw::webkit_frame_is_main_frame(frame) != 0;
        let update = with_runtime(|runtime| {
            let generation = runtime.state.attachment(page_id)?.generation;
            runtime
                .state
                .clear_document(page_id, frame_id, &uri, is_main)
                .ok()?;
            runtime
                .state
                .set_frame_facts(page_id, frame_id, None, None, true);
            let native = runtime.pages.get_mut(&page_id)?;
            if is_main {
                native.frame_allow = native
                    .pending_allow
                    .take()
                    .filter(|(target, _)| target == &uri)
                    .map(|(_, allow)| allow)
                    .unwrap_or_default();
            }
            let table_generation = native.scripts.as_ref().map_or(0, |table| table.generation);
            runtime
                .state
                .set_frame_script_table(page_id, frame_id, table_generation);
            let retired = if is_main {
                std::mem::take(&mut native.frames)
            } else {
                native
                    .frames
                    .remove(&frame_id)
                    .map(|old| BTreeMap::from([(frame_id, old)]))
                    .unwrap_or_default()
            };
            let watcher = Box::into_raw(Box::new((page_id, frame_id)));
            native.frames.insert(
                frame_id,
                NativeFrame {
                    pointer: frame,
                    weak_data: watcher,
                    installed: BTreeSet::new(),
                    receivers: BTreeMap::new(),
                    scripts: native.scripts.clone(),
                    phases: [false; 3],
                    bootstrapped: BTreeMap::new(),
                },
            );
            Some((
                watcher,
                retired,
                generation,
                runtime.state.frame(page_id, frame_id)?.document_seq,
            ))
        })
        .flatten();
        let Some((watcher, retired, generation, document_seq)) = update else {
            return;
        };
        release_frames(retired);
        raw::g_object_weak_ref(frame, Some(frame_destroyed), watcher.cast());
        scripts::collect_facts(page_id, frame_id);
        with_runtime(|runtime| {
            runtime.state.commit_frame(page_id, frame_id);
        });
        flush_frame_events(page_id);
        scripts::install_lifecycle(page_id, frame_id, document_seq);
        let worlds = with_runtime(|runtime| runtime.state.worlds_for_frame(page_id, frame_id))
            .unwrap_or_default();
        for world in worlds {
            let bootstrap = with_runtime(|runtime| {
                let attach = runtime.state.attachment(page_id)?;
                if world.is_empty() {
                    let info = runtime.state.frame(page_id, frame_id)?;
                    if protocol::document_origin(&info.uri)
                        .is_some_and(|origin| attach.extension_origins.contains(&origin))
                    {
                        Some((String::new(), attach.extension_bootstrap.clone()))
                    } else {
                        None
                    }
                } else {
                    attach
                        .worlds
                        .iter()
                        .find(|rule| rule.name == world)
                        .map(|rule| (rule.install_id.clone(), rule.bootstrap.clone()))
                }
            })
            .flatten();
            if let Some((install, bootstrap)) = bootstrap {
                scripts::install_bridge(
                    Binding {
                        page: page_id,
                        frame: frame_id,
                        document_seq,
                        world,
                        external_install: None,
                    },
                    &install,
                    &bootstrap,
                );
            }
        }
        scripts::run_scripts(
            page_id,
            frame_id,
            document_seq,
            protocol::RunAt::DocumentStart,
        );
        scripts::install_external(page_id, frame_id, document_seq, generation);
    });
}

fn install_function(context: &Owned, name: &str, function: *const (), binding: Binding) {
    let Ok(name) = CString::new(name) else {
        return;
    };
    unsafe {
        let mut parameter_type = raw::jsc_value_get_type();
        let data = Box::into_raw(Box::new(binding));
        let Some(value) = Owned::full(raw::jsc_value_new_functionv(
            context.0,
            name.as_ptr(),
            callback(function),
            data.cast(),
            Some(binding_destroyed),
            G_TYPE_BOOLEAN,
            1,
            &mut parameter_type,
        )) else {
            drop(Box::from_raw(data));
            return;
        };
        raw::jsc_context_set_value(context.0, name.as_ptr(), value.0);
    }
}
unsafe extern "C" fn binding_destroyed(data: *mut c_void) {
    boundary(|| unsafe {
        drop(Box::from_raw(data.cast::<Binding>()));
    });
}

fn binding_valid(runtime: &Runtime, binding: &Binding) -> bool {
    runtime.state.attachment(binding.page).is_some()
        && runtime
            .state
            .frame(binding.page, binding.frame)
            .is_some_and(|frame| frame.document_seq == binding.document_seq)
        && binding.external_install.as_ref().is_none_or(|install| {
            runtime
                .state
                .attachment(binding.page)
                .is_some_and(|attach| {
                    runtime
                        .state
                        .frame(binding.page, binding.frame)
                        .is_some_and(|frame| {
                            binding.world.is_empty()
                                && attach
                                    .external
                                    .iter()
                                    .any(|rule| rule.install_id == *install && rule.admits(frame))
                        })
                })
        })
        && runtime
            .pages
            .get(&binding.page)
            .and_then(|page| page.frames.get(&binding.frame))
            .is_some_and(|frame| {
                frame.installed.contains(&scripts::receiver_key(
                    &binding.world,
                    binding.external_install.as_deref(),
                ))
            })
}

unsafe extern "C" fn register_receiver(value: *mut Object, data: *mut c_void) -> c_int {
    boundary(|| unsafe {
        if value.is_null() || raw::jsc_value_is_function(value) == 0 {
            return 0;
        }
        let binding = &*data.cast::<Binding>();
        if !with_runtime(|runtime| binding_valid(runtime, binding)).unwrap_or(false) {
            return 0;
        }
        let receiver = raw::jsc_weak_value_new(value);
        if receiver.is_null() {
            return 0;
        }
        let old = with_runtime(|runtime| {
            if !binding_valid(runtime, binding) {
                return Err(());
            }
            let Some(frame) = runtime
                .pages
                .get_mut(&binding.page)
                .and_then(|page| page.frames.get_mut(&binding.frame))
            else {
                return Err(());
            };
            Ok(frame.receivers.insert(
                scripts::receiver_key(&binding.world, binding.external_install.as_deref()),
                receiver,
            ))
        });
        match old {
            Some(Ok(old)) => {
                if let Some(old) = old {
                    raw::g_object_unref(old);
                }
                1
            }
            _ => {
                raw::g_object_unref(receiver);
                0
            }
        }
    })
}

unsafe extern "C" fn post_bytes(value: *mut Object, data: *mut c_void) -> c_int {
    boundary(|| unsafe {
        let binding = &*data.cast::<Binding>();
        if value.is_null()
            || raw::jsc_value_is_typed_array(value) == 0
            || raw::jsc_value_typed_array_get_type(value) != UINT8_ARRAY
        {
            return 0;
        }
        let length = raw::jsc_value_typed_array_get_size(value);
        let reservation = with_runtime(|runtime| {
            if !binding_valid(runtime, binding) {
                return None;
            }
            let attach = runtime.state.attachment(binding.page)?;
            if length
                > runtime
                    .state
                    .message_cap(binding.page, binding.frame, &binding.world)?
            {
                return None;
            }
            let generation = attach.generation;
            let frame = runtime.state.frame(binding.page, binding.frame)?.clone();
            let native = runtime.pages.get(&binding.page)?;
            let pointer = native.pointer;
            let frame_pointer = native.frames.get(&binding.frame)?.pointer;
            let now_ms = (raw::g_get_monotonic_time().max(0) as u64) / 1000;
            let post_id = runtime
                .state
                .reserve_post(
                    binding.page,
                    binding.frame,
                    binding.document_seq,
                    &binding.world,
                    length,
                    now_ms,
                )
                .ok()?;
            Some((post_id, generation, frame, pointer, frame_pointer))
        })
        .flatten();
        let Some((post_id, generation, mut frame, pointer, frame_pointer)) = reservation else {
            return 0;
        };
        let Some(page) = Owned::borrowed(pointer) else {
            with_runtime(|runtime| runtime.state.settle_post(post_id));
            return 0;
        };
        let Some(native_frame) = Owned::borrowed(frame_pointer) else {
            with_runtime(|runtime| runtime.state.settle_post(post_id));
            return 0;
        };
        let Some(uri) = bounded_text(
            raw::webkit_frame_get_uri(native_frame.0),
            protocol::MAX_URI_BYTES,
        ) else {
            with_runtime(|runtime| runtime.state.settle_post(post_id));
            return 0;
        };
        frame.uri = uri;
        // Type, byte cap and both budgets have been checked before asking for data.
        let mut actual = 0;
        let bytes = raw::jsc_value_typed_array_get_data(value, &mut actual);
        if actual != length || (length != 0 && bytes.is_null()) {
            with_runtime(|runtime| runtime.state.settle_post(post_id));
            return 0;
        }
        let payload = if length == 0 {
            &[][..]
        } else {
            std::slice::from_raw_parts(bytes.cast::<u8>(), length)
        };
        let encoded = protocol::encode_post_bytes_for_install(
            generation,
            post_id,
            &frame,
            &binding.world,
            payload,
            binding.external_install.as_deref(),
        );
        let Ok(encoded) = encoded else {
            with_runtime(|runtime| runtime.state.settle_post(post_id));
            return 0;
        };
        if !flush_frame_events(binding.page) {
            with_runtime(|runtime| runtime.state.settle_post(post_id));
            return 0;
        }
        send_bytes(
            &page,
            encoded,
            Completion {
                page: binding.page,
                generation,
                post_id: Some(post_id),
                event_bytes: 0,
                batch: false,
            },
        );
        1
    })
}

struct DeliveryReservation(u64);
impl Drop for DeliveryReservation {
    fn drop(&mut self) {
        with_runtime(|runtime| runtime.state.settle_post(self.0));
    }
}
fn deliver_to_frame(page: u64, deliver: &Deliver) -> protocol::DeliveryStatus {
    let reservation = with_runtime(|runtime| {
        let now_ms = unsafe { raw::g_get_monotonic_time().max(0) as u64 } / 1000;
        runtime.state.reserve_delivery(page, deliver, now_ms)
    });
    let reservation = match reservation {
        Some(Ok(reservation)) => reservation,
        Some(Err(
            crate::state::StateError::RateLimited | crate::state::StateError::InflightLimited,
        )) => return protocol::DeliveryStatus::RateLimited,
        Some(Err(crate::state::StateError::WrongWorld)) => return protocol::DeliveryStatus::Denied,
        _ => return protocol::DeliveryStatus::StaleTarget,
    };
    let _reservation = DeliveryReservation(reservation);
    let target = with_runtime(|runtime| {
        if !runtime.state.valid_target(page, deliver) {
            return None;
        }
        let native = runtime.pages.get(&page)?.frames.get(&deliver.frame_id)?;
        Some((
            native.pointer,
            *runtime.worlds.get(&deliver.world)?,
            *native.receivers.get(&scripts::receiver_key(
                &deliver.world,
                deliver.external_install.as_deref(),
            ))?,
        ))
    })
    .flatten();
    let Some((frame, world, receiver)) = target else {
        return protocol::DeliveryStatus::NoReceiver;
    };
    // No native call between reading the weak registry and pinning its objects.
    let Some(frame) = Owned::borrowed(frame) else {
        return protocol::DeliveryStatus::NoReceiver;
    };
    let Some(receiver) = Owned::borrowed(receiver) else {
        return protocol::DeliveryStatus::NoReceiver;
    };
    unsafe {
        let Some(context) = Owned::full(raw::webkit_frame_get_js_context_for_script_world(
            frame.0, world,
        )) else {
            return protocol::DeliveryStatus::NoReceiver;
        };
        let Some(receiver_value) = Owned::full(raw::jsc_weak_value_get_value(receiver.0)) else {
            return protocol::DeliveryStatus::NoReceiver;
        };
        let Some(array) = Owned::full(raw::jsc_value_new_typed_array(
            context.0,
            UINT8_ARRAY,
            deliver.bytes.len(),
        )) else {
            return protocol::DeliveryStatus::NoReceiver;
        };
        if !deliver.bytes.is_empty() {
            let mut actual = 0;
            let pointer = raw::jsc_value_typed_array_get_data(array.0, &mut actual);
            if pointer.is_null() || actual != deliver.bytes.len() {
                return protocol::DeliveryStatus::NoReceiver;
            }
            ptr::copy_nonoverlapping(deliver.bytes.as_ptr(), pointer.cast::<u8>(), actual);
        }
        if !with_runtime(|runtime| runtime.state.valid_target(page, deliver)).unwrap_or(false) {
            return protocol::DeliveryStatus::NoReceiver;
        }
        let mut argument = array.0;
        let result = Owned::full(raw::jsc_value_function_callv(
            receiver_value.0,
            1,
            &mut argument,
        ));
        let accepted = raw::jsc_context_get_exception(context.0).is_null();
        drop(result);
        raw::jsc_context_clear_exception(context.0);
        if accepted {
            protocol::DeliveryStatus::Delivered
        } else {
            protocol::DeliveryStatus::EvaluationFailed
        }
    }
}

unsafe extern "C" fn document_loaded(page: *mut Object, _: *mut c_void) {
    boundary(|| unsafe {
        let page = raw::webkit_web_page_get_id(page);
        with_runtime(|runtime| {
            let Some(native) = runtime.pages.get(&page) else {
                return;
            };
            let frames: Vec<_> = native.frames.keys().copied().collect();
            for frame in frames {
                runtime.state.commit_frame(page, frame);
            }
        });
    });
}

enum PolicyData {
    Exact(PolicySnapshot),
    Dnr(DnrPolicySnapshot<'static>),
}
struct PublishedPolicy {
    // The dependent descriptors must be dropped before the mapping.
    data: PolicyData,
    mapping: MappedBytes,
}
struct MappedBytes {
    pointer: *mut c_void,
    length: usize,
    identity: (u64, u64),
}
impl Drop for MappedBytes {
    fn drop(&mut self) {
        unsafe {
            libc::munmap(self.pointer, self.length);
        }
    }
}

fn map_policy(message: *mut Object, index: u32, length: usize) -> Option<Rc<PublishedPolicy>> {
    let mapping = map_region(message, index, length, MAX_DNR_SNAPSHOT_BYTES)?;
    if let Some(Some(shared)) = with_runtime(|runtime| {
        runtime
            .policy_maps
            .get(&mapping.identity)
            .and_then(RcWeak::upgrade)
    }) {
        return Some(shared);
    }
    let allowed = with_runtime(|runtime| {
        runtime
            .policy_maps
            .retain(|_, policy| policy.strong_count() != 0);
        let total: usize = runtime
            .policy_maps
            .values()
            .filter_map(RcWeak::upgrade)
            .map(|policy| policy.mapping.length)
            .sum();
        total.saturating_add(length) <= 512 * 1024 * 1024
    })
    .unwrap_or(false);
    if !allowed {
        return None;
    }
    // The read-only mapping is immutable and owned by PublishedPolicy. Only its
    // private descriptors borrow this lifetime; they never escape that owner.
    let bytes: &'static [u8] =
        unsafe { std::slice::from_raw_parts(mapping.pointer.cast::<u8>(), mapping.length) };
    let data = if DnrPolicySnapshot::is_dnr(bytes) {
        PolicyData::Dnr(DnrPolicySnapshot::load(bytes).ok()?)
    } else {
        PolicyData::Exact(PolicySnapshot::decode(bytes).ok()?)
    };
    let identity = mapping.identity;
    let shared = Rc::new(PublishedPolicy { data, mapping });
    with_runtime(|runtime| {
        runtime.policy_maps.insert(identity, Rc::downgrade(&shared));
    });
    Some(shared)
}

fn map_sealed<T, E>(
    message: *mut Object,
    index: u32,
    length: usize,
    maximum: usize,
    decode: impl FnOnce(&[u8]) -> Result<T, E>,
) -> Option<T> {
    let mapping = map_region(message, index, length, maximum)?;
    decode(unsafe { std::slice::from_raw_parts(mapping.pointer.cast::<u8>(), mapping.length) }).ok()
}

fn map_region(
    message: *mut Object,
    index: u32,
    length: usize,
    maximum: usize,
) -> Option<MappedBytes> {
    if length == 0 || length > maximum || index > c_int::MAX as u32 {
        return None;
    }
    unsafe {
        let list = raw::webkit_user_message_get_fd_list(message);
        if list.is_null() || index as c_int >= raw::g_unix_fd_list_get_length(list) {
            return None;
        }
        let mut error = ptr::null_mut();
        let fd = raw::g_unix_fd_list_get(list, index as c_int, &mut error);
        if !error.is_null() {
            raw::g_error_free(error);
        }
        if fd < 0 {
            return None;
        }
        struct Descriptor(c_int);
        impl Drop for Descriptor {
            fn drop(&mut self) {
                unsafe {
                    libc::close(self.0);
                }
            }
        }
        let fd = Descriptor(fd);
        let required =
            libc::F_SEAL_SEAL | libc::F_SEAL_SHRINK | libc::F_SEAL_GROW | libc::F_SEAL_WRITE;
        let seals = libc::fcntl(fd.0, libc::F_GET_SEALS);
        if seals < 0 || seals & required != required {
            return None;
        }
        let mut info = std::mem::MaybeUninit::<libc::stat>::uninit();
        if libc::fstat(fd.0, info.as_mut_ptr()) != 0 {
            return None;
        }
        let info = info.assume_init();
        if info.st_mode & libc::S_IFMT != libc::S_IFREG || info.st_size != length as libc::off_t {
            return None;
        }
        let mapped = libc::mmap(
            ptr::null_mut(),
            length,
            libc::PROT_READ,
            libc::MAP_PRIVATE,
            fd.0,
            0,
        );
        if mapped == libc::MAP_FAILED {
            return None;
        }
        Some(MappedBytes {
            pointer: mapped,
            length,
            identity: (info.st_dev, info.st_ino),
        })
    }
}

unsafe extern "C" fn send_request(
    page: *mut Object,
    request: *mut Object,
    redirected_response: *mut Object,
    _: *mut c_void,
) -> c_int {
    boundary(|| unsafe {
        let page_id = raw::webkit_web_page_get_id(page);
        let Some((generation, observe, tab_id)) = with_runtime(|runtime| {
            runtime
                .state
                .attachment(page_id)
                .map(|attach| (attach.generation, attach.observe_requests, attach.tab_id))
        })
        .flatten() else {
            return 0;
        };
        let started = observe.then(std::time::Instant::now);
        let Some(url) = bounded_text(
            raw::webkit_uri_request_get_uri(request),
            protocol::MAX_URI_BYTES,
        ) else {
            return 1;
        };
        let method = bounded_text(raw::webkit_uri_request_get_http_method(request), 32);
        let headers = raw::webkit_uri_request_get_http_headers(request);
        let header = |name: &std::ffi::CStr, cap| {
            if headers.is_null() {
                None
            } else {
                bounded_text(
                    raw::soup_message_headers_get_one(headers, name.as_ptr()),
                    cap,
                )
            }
        };
        let destination = header(c"Sec-Fetch-Dest", 64);
        let fetch_mode = header(c"Sec-Fetch-Mode", 64);
        let content_type = header(c"Content-Type", 128);
        let mut resource_type = match destination.as_deref() {
            Some(destination) => ResourceType::infer(Some(destination)),
            None => {
                let page_uri = raw::webkit_web_page_get_uri(page);
                let navigation = !page_uri.is_null()
                    && (std::ffi::CStr::from_ptr(page_uri).to_bytes() == url.as_bytes()
                        || (!redirected_response.is_null() && {
                            let previous = raw::webkit_uri_response_get_uri(redirected_response);
                            !previous.is_null()
                                && std::ffi::CStr::from_ptr(previous)
                                    == std::ffi::CStr::from_ptr(page_uri)
                        }));
                ResourceType::infer_without_metadata(header(c"Accept", 256).as_deref(), navigation)
            }
        };
        // WebKit labels object/embed documents as iframe navigations.
        if resource_type == ResourceType::SubFrame && embedded_by_plugin_element(page_id, &url) {
            resource_type = ResourceType::Object;
        }
        // These headers are supplied by WebKit's loader; URL suffixes never
        // establish a resource type, and Accept is consulted only without them.
        if destination.as_deref() == Some("empty") {
            resource_type = if content_type.as_deref().is_some_and(|value| {
                value.starts_with("application/csp-report")
                    || value.starts_with("application/reports+json")
            }) {
                ResourceType::CspReport
            } else if header(c"Ping-To", protocol::MAX_URI_BYTES).is_some()
                || (fetch_mode.as_deref() == Some("no-cors") && method.as_deref() == Some("POST"))
            {
                ResourceType::Ping
            } else {
                ResourceType::Fetch
            };
        }
        let claimed_initiator = header(c"Origin", protocol::MAX_URI_BYTES)
            .or_else(|| header(c"Referer", protocol::MAX_URI_BYTES));
        // A suppressed referrer remains unknown. Never turn a page-provided
        // cross-origin URL into host access without a live native document.
        let initiator = claimed_initiator
            .as_deref()
            .and_then(protocol::document_origin)
            .filter(|origin| {
                with_runtime(|runtime| {
                    runtime.pages.get(&page_id).is_some_and(|native| {
                        native
                            .frames
                            .keys()
                            .filter_map(|id| runtime.state.frame(page_id, *id))
                            .any(|frame| {
                                !frame.opaque
                                    && protocol::document_origin(&frame.uri).as_ref()
                                        == Some(origin)
                            })
                    })
                })
                .unwrap_or(false)
            });
        let top = with_runtime(|runtime| {
            let native = runtime.pages.get(&page_id)?;
            native
                .frames
                .keys()
                .filter_map(|id| runtime.state.frame(page_id, *id))
                .find(|frame| frame.is_main)
                .map(|frame| (frame.frame_id, frame.uri.clone()))
        })
        .flatten();
        let info = RequestInfo {
            url: url.clone(),
            tab_id,
            method: method.clone(),
            resource_type,
            frame_id: (resource_type == ResourceType::MainFrame)
                .then(|| top.as_ref().map(|(id, _)| *id))
                .flatten(),
            document_url: top.as_ref().map(|(_, url)| url.clone()),
            initiator: initiator.clone(),
            is_main_frame: resource_type == ResourceType::MainFrame,
        };
        let matched = with_runtime(|runtime| {
            let page = runtime.pages.get_mut(&page_id)?;
            let policy = page.policy.as_ref()?;
            match &policy.data {
                PolicyData::Exact(policy) => {
                    let matched = policy.evaluate(&info);
                    Some((matched.rule_id, matched.decision, Vec::new()))
                }
                PolicyData::Dnr(policy) => {
                    let inherited = if info.is_main_frame {
                        &[][..]
                    } else {
                        &page.frame_allow
                    };
                    let matched = policy.evaluate(&info, inherited);
                    if info.is_main_frame {
                        page.pending_allow = if matches!(
                            matched.decision,
                            Decision::Allow | Decision::ModifyRequestHeaders(_)
                        ) {
                            Some((info.url.clone(), matched.frame_allow_updates))
                        } else {
                            None
                        };
                    }
                    let first = matched.matched_rules.first().map(|rule| rule.rule_id);
                    Some((first, matched.decision, matched.matched_rules))
                }
            }
        })
        .flatten();
        let (rule_id, decision, matched_rules) =
            matched.unwrap_or((None, Decision::Allow, Vec::new()));
        let label = match &decision {
            Decision::Allow => "allow",
            Decision::Cancel => "cancel",
            Decision::Redirect(_) => "redirect",
            Decision::SetRequestHeaders(_) | Decision::ModifyRequestHeaders(_) => {
                "modify-request-headers"
            }
        };
        let cancel = match &decision {
            Decision::Cancel => true,
            Decision::Redirect(url) => {
                if let Ok(url) = CString::new(url.as_str()) {
                    raw::webkit_uri_request_set_uri(request, url.as_ptr());
                    false
                } else {
                    true
                }
            }
            Decision::SetRequestHeaders(changes) => {
                if !headers.is_null() {
                    for header in changes {
                        if let (Ok(name), Ok(value)) = (
                            CString::new(header.name.as_str()),
                            CString::new(header.value.as_str()),
                        ) {
                            raw::soup_message_headers_replace(
                                headers,
                                name.as_ptr(),
                                value.as_ptr(),
                            );
                        }
                    }
                }
                false
            }
            Decision::ModifyRequestHeaders(changes) => {
                if !headers.is_null() {
                    for change in changes {
                        let Ok(name) = CString::new(change.header.as_str()) else {
                            continue;
                        };
                        match change.operation {
                            zephium_webkit_web_process_protocol::dnr::HeaderOperation::Remove => {
                                raw::soup_message_headers_remove(headers, name.as_ptr());
                            }
                            operation => {
                                if let Some(value) = change
                                    .value
                                    .as_deref()
                                    .and_then(|value| CString::new(value).ok())
                                {
                                    if operation == zephium_webkit_web_process_protocol::dnr::HeaderOperation::Append {
                                raw::soup_message_headers_append(headers, name.as_ptr(), value.as_ptr());
                            } else { raw::soup_message_headers_replace(headers, name.as_ptr(), value.as_ptr()); }
                                }
                            }
                        }
                    }
                }
                false
            }
            Decision::Allow => false,
        };
        let evaluation_time_ns = started.map_or(0, |start| {
            start.elapsed().as_nanos().min(u64::MAX as u128) as u64
        });
        if observe || rule_id.is_some() {
            let observation = RequestEvent {
                url,
                method: method.unwrap_or_default(),
                resource_type: format!("{resource_type:?}"),
                frame_id: info.frame_id.unwrap_or(0),
                document_url: info.document_url.unwrap_or_default(),
                is_main_frame: info.is_main_frame,
                decision: label.to_owned(),
                matched: rule_id.is_some(),
                matched_rule_id: rule_id,
                matched_rules,
                evaluation_time_ns,
                initiator,
            };
            with_runtime(|runtime| {
                if runtime
                    .state
                    .attachment(page_id)
                    .is_none_or(|attach| attach.generation != generation)
                {
                    return;
                }
                let Some(page) = runtime.pages.get_mut(&page_id) else {
                    return;
                };
                let bytes = observation.url.len()
                    + observation.method.len()
                    + observation.document_url.len()
                    + observation.initiator.as_ref().map_or(0, String::len)
                    + observation
                        .matched_rules
                        .iter()
                        .map(|rule| rule.extension_id.len() + rule.ruleset_id.len() + 16)
                        .sum::<usize>()
                    + 128;
                if page.requests.len() < protocol::MAX_REQUEST_EVENTS
                    && page.request_bytes.saturating_add(bytes) <= MAX_PENDING_EVENT_BYTES
                {
                    page.request_bytes += bytes;
                    page.requests.push(observation);
                } else {
                    page.requests_dropped = page.requests_dropped.saturating_add(1);
                    page.matches_dropped = page
                        .matches_dropped
                        .saturating_add(u64::from(observation.matched));
                }
            });
        }
        c_int::from(cancel)
    })
}

const EMBED_SOURCE: &str = "(url => { for (const element of document.querySelectorAll('object,embed')) { if ((element instanceof HTMLObjectElement ? element.data : element.src) === url) return true; } return false; })";

/// Runs only for sub-frame navigations, in the private facts world of each
/// live non-opaque frame, so page scripts cannot alter the DOM accessors used.
fn embedded_by_plugin_element(page_id: u64, url: &str) -> bool {
    let Ok(url) = CString::new(url) else {
        return false;
    };
    let Some((world, frames)) = with_runtime(|runtime| {
        let world = *runtime.worlds.get(FACT_WORLD)?;
        let native = runtime.pages.get(&page_id)?;
        let frames: Vec<_> = native
            .frames
            .iter()
            .filter(|(id, _)| {
                runtime
                    .state
                    .frame(page_id, **id)
                    .is_some_and(|frame| !frame.opaque)
            })
            .map(|(_, frame)| frame.pointer)
            .take(64)
            .collect();
        Some((world, frames))
    })
    .flatten() else {
        return false;
    };
    let frames: Vec<_> = frames.into_iter().filter_map(Owned::borrowed).collect();
    frames.iter().any(|frame| unsafe {
        let Some(context) = Owned::full(raw::webkit_frame_get_js_context_for_script_world(
            frame.0, world,
        )) else {
            return false;
        };
        let Some(helper) = Owned::full(raw::jsc_context_evaluate(
            context.0,
            EMBED_SOURCE.as_ptr().cast(),
            EMBED_SOURCE.len() as isize,
        )) else {
            return false;
        };
        let Some(argument) = Owned::full(raw::jsc_value_new_string(context.0, url.as_ptr())) else {
            return false;
        };
        let mut argument = argument.0;
        let result = Owned::full(raw::jsc_value_function_callv(helper.0, 1, &mut argument));
        if !raw::jsc_context_get_exception(context.0).is_null() {
            raw::jsc_context_clear_exception(context.0);
            return false;
        }
        result.is_some_and(|value| raw::jsc_value_to_boolean(value.0) != 0)
    })
}

// This world is private to the native form manager; page and extension scripts
// cannot replace its DOM prototypes or the bounded result's array properties.
const FORM_SOURCE: &str = r#"(form => {
    const result = []; let total = 0; let optionCount = 0;
    const append = (name, value) => {
        if (result.length >= 64 || typeof name !== 'string' || !name.length || name.length > 256 || typeof value !== 'string' || value.length > 2048 || total + name.length + value.length > 16384) return;
        total += name.length + value.length; result.push([name, value]);
    };
    const elements = Object.getOwnPropertyDescriptor(HTMLFormElement.prototype, 'elements').get.call(form);
    const count = Math.min(elements.length, 256);
    for (let i = 0; i < count && result.length < 64; ++i) {
        const element = elements.item(i);
        if (!(element instanceof HTMLInputElement || element instanceof HTMLTextAreaElement || element instanceof HTMLSelectElement)) continue;
        if (Element.prototype.matches.call(element, ':disabled') || element.type === 'file' || element.type === 'submit' || element.type === 'button' || element.type === 'reset' || element.type === 'image') continue;
        if ((element.type === 'checkbox' || element.type === 'radio') && !element.checked) continue;
        if (element instanceof HTMLSelectElement) {
            const options = Object.getOwnPropertyDescriptor(HTMLSelectElement.prototype, 'options').get.call(element);
            for (let j = 0; j < options.length && optionCount < 256; ++j, ++optionCount) {
                const option = options.item(j);
                if (!option.selected || option.disabled || (option.parentElement instanceof HTMLOptGroupElement && option.parentElement.disabled)) continue;
                append(element.name, option.value);
            }
        } else append(element.name, element.value);
    }
    return result;
})"#;

unsafe extern "C" fn will_submit_form(
    _: *mut Object,
    form: *mut Object,
    source: *mut Object,
    _: *mut Object,
    data: *mut c_void,
) {
    boundary(|| unsafe {
        let page = *data.cast::<u64>();
        let frame_id = raw::webkit_frame_get_id(source);
        let target = with_runtime(|runtime| {
            let attach = runtime.state.attachment(page)?;
            if !attach.observe_forms {
                return None;
            }
            let frame = runtime.state.frame(page, frame_id)?;
            Some((
                attach.generation,
                frame.document_seq,
                *runtime.worlds.get(FORM_WORLD)?,
            ))
        })
        .flatten();
        let Some((generation, document_seq, world)) = target else {
            return;
        };
        let Some(context) = Owned::full(raw::webkit_frame_get_js_context_for_script_world(
            source, world,
        )) else {
            return;
        };
        let Some(helper) = Owned::full(raw::jsc_context_evaluate(
            context.0,
            FORM_SOURCE.as_ptr().cast(),
            FORM_SOURCE.len() as isize,
        )) else {
            return;
        };
        let mut argument = form;
        let result = Owned::full(raw::jsc_value_function_callv(helper.0, 1, &mut argument));
        if !raw::jsc_context_get_exception(context.0).is_null() {
            raw::jsc_context_clear_exception(context.0);
            return;
        }
        let Some(result) = result else {
            return;
        };
        let Some(length) = Owned::full(raw::jsc_value_object_get_property(
            result.0,
            c"length".as_ptr(),
        )) else {
            return;
        };
        let count = raw::jsc_value_to_int32(length.0);
        if !(0..=protocol::MAX_FORM_FIELDS as i32).contains(&count) {
            return;
        }
        let mut fields = Vec::new();
        let mut total = 0usize;
        for index in 0..count {
            let Some(pair) = Owned::full(raw::jsc_value_object_get_property_at_index(
                result.0,
                index as u32,
            )) else {
                return;
            };
            let Some(name) = Owned::full(raw::jsc_value_object_get_property_at_index(pair.0, 0))
            else {
                return;
            };
            let Some(value) = Owned::full(raw::jsc_value_object_get_property_at_index(pair.0, 1))
            else {
                return;
            };
            let Some(name) = form_string(name.0, 1024) else {
                return;
            };
            let Some(value) = form_string(value.0, 8192) else {
                return;
            };
            total = total.saturating_add(name.len()).saturating_add(value.len());
            if total > 64 * 1024 {
                break;
            }
            fields.push((name, value));
        }
        raw::jsc_context_clear_exception(context.0);
        if !fields.is_empty() {
            queue_event(
                page,
                Event::Form(FormEvent {
                    generation,
                    frame_id,
                    document_seq,
                    fields,
                }),
            );
        }
    });
}

fn form_string(value: *mut Object, cap: usize) -> Option<String> {
    unsafe {
        if raw::jsc_value_is_string(value) == 0 {
            return None;
        }
        let pointer = raw::jsc_value_to_string(value);
        let result = bounded_text(pointer, cap);
        raw::g_free(pointer.cast());
        result
    }
}

fn deactivate_page(id: u64) {
    let frames = with_runtime(|runtime| {
        runtime.state.remove_page(id);
        let native = runtime.pages.get_mut(&id)?;
        native.scripts = None;
        native.policy = None;
        native.frame_allow.clear();
        native.pending_allow = None;
        native.policy_generation = 0;
        native.requests.clear();
        native.request_bytes = 0;
        native.requests_dropped = 0;
        native.matches_dropped = 0;
        native.events.clear();
        native.event_bytes = 0;
        native.batch_inflight = false;
        Some(std::mem::take(&mut native.frames))
    })
    .flatten();
    if let Some(frames) = frames {
        release_frames(frames);
    }
}
unsafe extern "C" fn form_hook_destroyed(data: *mut c_void, _: *mut Object) {
    boundary(|| unsafe {
        drop(Box::from_raw(data.cast::<u64>()));
    });
}
