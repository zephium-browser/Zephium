//! Pointer moves over this process's windows, pages included. A page's
//! WebView2 belongs to another process, so none of its moves reach this
//! thread; raw mouse input does. It is registered only while a listening
//! window is in front. Move listeners run at most once a frame. Button
//! listeners keep registration alive between frames so a quick press and
//! release cannot be lost.
//!
//! Raw input rather than a low-level mouse hook: the hook would sit in the
//! path of every move on the desktop, and the system drops one that is slow
//! to answer. Raw input is only told about moves after the fact.

use std::cell::{Cell, RefCell};
use std::time::Duration;

use windows::core::w;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::System::SystemInformation::GetTickCount;
use windows::Win32::UI::Accessibility::{SetWinEventHook, UnhookWinEvent, HWINEVENTHOOK};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, VK_SHIFT};
use windows::Win32::UI::Input::{
    GetRawInputData, RegisterRawInputDevices, HRAWINPUT, RAWINPUT, RAWINPUTDEVICE, RAWINPUTHEADER,
    RIDEV_INPUTSINK, RIDEV_REMOVE, RID_INPUT, RIM_TYPEMOUSE,
};
use windows::Win32::UI::Shell::{DefSubclassProc, SetWindowSubclass};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DestroyWindow, GetAncestor, GetForegroundWindow, GetMessagePos, GetMessageTime,
    IsIconic, IsWindow, KillTimer, SetTimer, EVENT_SYSTEM_FOREGROUND, EVENT_SYSTEM_MINIMIZEEND,
    EVENT_SYSTEM_MINIMIZESTART, GA_ROOTOWNER, HWND_MESSAGE, WINDOW_EX_STYLE, WINDOW_STYLE,
    WINEVENT_OUTOFCONTEXT, WM_INPUT, WM_TIMER,
};

/// About a display frame, as often as the pointer was read while it moved.
const FRAME_MS: u32 = 16;
const SUBCLASS: usize = 0x5a50_4e54;
const SETTLE: usize = 1;
const HID_USAGE_PAGE_GENERIC: u16 = 0x01;
const HID_USAGE_GENERIC_MOUSE: u16 = 0x02;
/// A sink or registration that failed is tried again a second later, this many
/// times in a row, then every half minute until it works. Only if that timer
/// cannot be set does recovery wait: for the next listen, or for a change of
/// the window in front once the sink and its hooks exist.
const RETRY_MS: u32 = 1_000;
const FAST_RETRIES: u8 = 8;
const SLOW_RETRY_MS: u32 = 30_000;

/// Told that the pointer moved, or that a window came to the front or left
/// it. Receives the window it listens for; it reads the pointer itself.
pub type Listener = fn(HWND);
pub type ButtonListener = fn(u16, POINT, bool, Duration);

struct Sink {
    hwnd: HWND,
    hooks: [HWINEVENTHOOK; 2],
}

thread_local! {
    static LISTENERS: RefCell<Vec<(HWND, Listener)>> = const { RefCell::new(Vec::new()) };
    static BUTTON_LISTENERS: RefCell<Vec<(HWND, ButtonListener)>> = const { RefCell::new(Vec::new()) };
    static SINK: Cell<Option<Sink>> = const { Cell::new(None) };
    static REGISTERED: Cell<bool> = const { Cell::new(false) };
    static SETTLING: Cell<bool> = const { Cell::new(false) };
    static RETRY: Cell<usize> = const { Cell::new(0) };
    static RETRIES: Cell<u8> = const { Cell::new(0) };
}

/// Calls `listener` with `window` on pointer moves while it is in front.
/// Runs on the UI thread.
pub fn listen(window: HWND, listener: Listener) {
    LISTENERS.with(|listeners| {
        let mut listeners = listeners.borrow_mut();
        if !listeners.contains(&(window, listener)) {
            listeners.push((window, listener));
        }
    });
    if ensure_sink() {
        follow_front();
    } else {
        retry_later();
    }
}

/// Stops calling `listener` for `window`. Runs on the UI thread.
pub fn forget(window: HWND, listener: Listener) {
    LISTENERS.with(|listeners| {
        listeners
            .borrow_mut()
            .retain(|entry| *entry != (window, listener))
    });
    follow_front();
}

pub fn listen_buttons(window: HWND, listener: ButtonListener) {
    set_button_listener(window, listener, true);
    if ensure_sink() {
        follow_front();
    } else {
        retry_later();
    }
}

pub fn forget_buttons(window: HWND, listener: ButtonListener) {
    set_button_listener(window, listener, false);
    follow_front();
}

fn set_button_listener(window: HWND, listener: ButtonListener, enabled: bool) {
    BUTTON_LISTENERS.with(|listeners| {
        let mut listeners = listeners.borrow_mut();
        let entry = (window, listener);
        if enabled {
            if !listeners.contains(&entry) {
                listeners.push(entry);
            }
        } else {
            listeners.retain(|registered| *registered != entry);
        }
    });
}

fn button_owner(front: HWND) -> Option<HWND> {
    BUTTON_LISTENERS.with(|listeners| {
        listeners.borrow().iter().find_map(|(window, _)| {
            // SAFETY: reads only the matching window's minimized state.
            (*window == front && !unsafe { IsIconic(*window) }.as_bool()).then_some(*window)
        })
    })
}

fn ensure_sink() -> bool {
    if let Some(sink) = SINK.take() {
        SINK.set(Some(sink));
        return true;
    }
    // SAFETY: a message-only window and out-of-context hooks owned by this
    // thread; both deliver through this thread's message loop.
    unsafe {
        let hwnd = match CreateWindowExW(
            WINDOW_EX_STYLE(0),
            w!("STATIC"),
            w!(""),
            WINDOW_STYLE(0),
            0,
            0,
            0,
            0,
            Some(HWND_MESSAGE),
            None,
            None,
            None,
        ) {
            Ok(hwnd) => hwnd,
            Err(error) => {
                crate::write_diagnostic(format_args!("pointer: raw input sink failed: {error}"));
                return false;
            }
        };
        if !SetWindowSubclass(hwnd, Some(sink_proc), SUBCLASS, 0).as_bool() {
            crate::write_diagnostic(format_args!("pointer: raw input subclass failed"));
            let _ = DestroyWindow(hwnd);
            return false;
        }
        let hooks = [
            (EVENT_SYSTEM_FOREGROUND, EVENT_SYSTEM_FOREGROUND),
            (EVENT_SYSTEM_MINIMIZESTART, EVENT_SYSTEM_MINIMIZEEND),
        ]
        .map(|(first, last)| {
            SetWinEventHook(
                first,
                last,
                None,
                Some(front_changed),
                0,
                0,
                WINEVENT_OUTOFCONTEXT,
            )
        });
        let sink = Sink { hwnd, hooks };
        // A hook that failed is not kept as done: the next listen starts over.
        if sink.hooks.iter().any(HWINEVENTHOOK::is_invalid) {
            crate::write_diagnostic(format_args!(
                "pointer: foreground event subscription failed"
            ));
            drop(sink);
            let _ = DestroyWindow(hwnd);
            return false;
        }
        SINK.set(Some(sink));
    }
    true
}

fn sink_hwnd() -> Option<HWND> {
    let sink = SINK.take()?;
    let hwnd = sink.hwnd;
    SINK.set(Some(sink));
    Some(hwnd)
}

/// Whether a listening window is in front and not minimized.
fn listening_in_front(front: HWND) -> bool {
    // SAFETY: plain reads of the window in front and its owner.
    let front = unsafe { (!front.0.is_null()).then(|| GetAncestor(front, GA_ROOTOWNER)) };
    let Some(front) = front else {
        return false;
    };
    LISTENERS.with(|listeners| {
        listeners.borrow_mut().retain(|(window, _)| {
            // SAFETY: a plain query of a handle value.
            unsafe { IsWindow(Some(*window)) }.as_bool()
        });
        listeners.borrow().iter().any(|(window, _)| {
            // SAFETY: a plain read of a live window's state.
            *window == front && !unsafe { IsIconic(*window) }.as_bool()
        })
    })
}

fn follow_front() {
    // SAFETY: read foreground ownership once for this notification.
    let front = unsafe { GetForegroundWindow() };
    let owner = button_owner(front);
    super::middle_click_foreground_changed(owner);
    let wanted = wants_registration(owner, SETTLING.get(), || listening_in_front(front));
    if wanted != REGISTERED.get() {
        register(wanted);
    }
}

fn wants_registration(
    button_owner: Option<HWND>,
    settling: bool,
    moves_in_front: impl FnOnce() -> bool,
) -> bool {
    button_owner.is_some() || (moves_in_front() && !settling)
}

fn register(on: bool) {
    let Some(sink) = sink_hwnd() else {
        return;
    };
    let device = RAWINPUTDEVICE {
        usUsagePage: HID_USAGE_PAGE_GENERIC,
        usUsage: HID_USAGE_GENERIC_MOUSE,
        dwFlags: if on { RIDEV_INPUTSINK } else { RIDEV_REMOVE },
        hwndTarget: if on { sink } else { HWND::default() },
    };
    // SAFETY: registers one sized device record for this process.
    let done = unsafe { RegisterRawInputDevices(&[device], size_of::<RAWINPUTDEVICE>() as u32) };
    match done {
        Ok(()) => {
            REGISTERED.set(on);
            RETRIES.set(0);
        }
        Err(error) => {
            crate::write_diagnostic(format_args!(
                "pointer: raw input registration failed: {error}"
            ));
            if on {
                retry_later();
            }
        }
    }
}

/// Arms one retry of the sink and the registration, unless one is armed. A
/// timer that cannot be set counts for nothing: the next listen or change
/// of the window in front arms it again.
fn retry_later() {
    if RETRY.get() != 0 {
        return;
    }
    let fast = RETRIES.get() < FAST_RETRIES;
    let delay = if fast { RETRY_MS } else { SLOW_RETRY_MS };
    // SAFETY: a thread timer whose callback runs on this thread's loop.
    let timer = unsafe { SetTimer(None, 0, delay, Some(retry)) };
    if timer != 0 && fast {
        RETRIES.set(RETRIES.get() + 1);
    }
    RETRY.set(timer);
}

unsafe extern "system" fn retry(_: HWND, _: u32, id: usize, _: u32) {
    let _ = KillTimer(None, id);
    RETRY.set(0);
    let _ = std::panic::catch_unwind(|| {
        if LISTENERS.with(|listeners| listeners.borrow().is_empty())
            && BUTTON_LISTENERS.with(|listeners| listeners.borrow().is_empty())
        {
            return;
        }
        if ensure_sink() {
            follow_front();
        } else {
            retry_later();
        }
    });
}

fn notify() {
    let listeners = LISTENERS.with(|listeners| listeners.borrow().clone());
    for (window, listener) in listeners {
        listener(window);
    }
}

unsafe extern "system" fn front_changed(
    _: HWINEVENTHOOK,
    event: u32,
    window: HWND,
    _: i32,
    _: i32,
    _: u32,
    _: u32,
) {
    let _ = std::panic::catch_unwind(|| {
        if event == EVENT_SYSTEM_FOREGROUND {
            // Observe loss of ownership even if focus has already returned.
            super::middle_click_foreground_changed(button_owner(window));
        }
        follow_front();
        notify();
    });
}

unsafe extern "system" fn sink_proc(
    hwnd: HWND,
    msg: u32,
    w: WPARAM,
    l: LPARAM,
    _: usize,
    _: usize,
) -> LRESULT {
    match msg {
        // Button transitions must survive move-listener frame throttling.
        WM_INPUT => {
            let mouse = std::panic::catch_unwind(|| notify_input(l)).unwrap_or(false);
            if !mouse || SETTLING.get() {
                return DefSubclassProc(hwnd, msg, w, l);
            }
            // Input is held back only while a timer will end the frame;
            // without one it keeps arriving unthrottled rather than never.
            if SetTimer(Some(hwnd), SETTLE, FRAME_MS, None) != 0 {
                SETTLING.set(true);
                follow_front();
            }
            let _ = std::panic::catch_unwind(notify);
        }
        // The frame is over: read where the pointer came to rest, and listen
        // again for the next move.
        WM_TIMER if w.0 == SETTLE => {
            let _ = KillTimer(Some(hwnd), SETTLE);
            SETTLING.set(false);
            follow_front();
            let _ = std::panic::catch_unwind(notify);
            return LRESULT(0);
        }
        _ => {}
    }
    DefSubclassProc(hwnd, msg, w, l)
}

fn notify_input(data: LPARAM) -> bool {
    let mut input = RAWINPUT::default();
    let mut size = size_of::<RAWINPUT>() as u32;
    // SAFETY: WM_INPUT supplies the handle. The output is aligned and bounded
    // by size.
    let read = unsafe {
        GetRawInputData(
            HRAWINPUT(data.0 as *mut _),
            RID_INPUT,
            Some((&mut input as *mut RAWINPUT).cast()),
            &mut size,
            size_of::<RAWINPUTHEADER>() as u32,
        )
    };
    let Some(buttons) = mouse_buttons(&input, read) else {
        return false;
    };
    notify_buttons(
        buttons,
        // SAFETY: read foreground ownership once for all button listeners.
        || button_owner(unsafe { GetForegroundWindow() }),
        button_metadata,
    );
    true
}

fn mouse_buttons(input: &RAWINPUT, read: u32) -> Option<u16> {
    if read != size_of::<RAWINPUT>() as u32 || input.header.dwType != RIM_TYPEMOUSE.0 {
        return None;
    }
    // SAFETY: the complete record's type tag identifies the mouse union field.
    Some(unsafe { input.data.mouse.Anonymous.Anonymous.usButtonFlags })
}

fn notify_buttons(
    buttons: u16,
    read_owner: impl FnOnce() -> Option<HWND>,
    read_metadata: impl FnOnce() -> (POINT, bool, Duration),
) {
    if buttons & 0x03ff == 0 {
        return;
    }
    let owner = read_owner();
    super::middle_click_foreground_changed(owner);
    let Some(owner) = owner else {
        return;
    };
    let (position, shift, age) = read_metadata();
    let listeners = BUTTON_LISTENERS.with(|listeners| listeners.borrow().clone());
    for (window, listener) in listeners {
        if window == owner {
            listener(buttons, position, shift, age);
        }
    }
}

fn button_metadata() -> (POINT, bool, Duration) {
    // SAFETY: position and time describe the last retrieved message.
    // GetKeyState reads the calling thread's queued modifier state.
    unsafe {
        let packed = GetMessagePos();
        (
            POINT {
                x: i32::from(packed as u16 as i16),
                y: i32::from((packed >> 16) as u16 as i16),
            },
            GetKeyState(i32::from(VK_SHIFT.0)) < 0,
            Duration::from_millis(u64::from(
                GetTickCount().wrapping_sub(GetMessageTime() as u32),
            )),
        )
    }
}

impl Drop for Sink {
    fn drop(&mut self) {
        // SAFETY: unhooks this thread's own hooks.
        unsafe {
            for hook in self.hooks {
                let _ = UnhookWinEvent(hook);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::UI::Input::RIM_TYPEKEYBOARD;
    use windows::Win32::UI::WindowsAndMessaging::RI_MOUSE_MIDDLE_BUTTON_DOWN;

    thread_local! {
        static BUTTON_REPORTS: Cell<u32> = const { Cell::new(0) };
    }

    fn count_button_report(_: u16, _: POINT, _: bool, _: Duration) {
        BUTTON_REPORTS.set(BUTTON_REPORTS.get() + 1);
    }

    #[test]
    fn only_complete_mouse_records_supply_button_flags() {
        let mut input = RAWINPUT::default();
        let size = size_of::<RAWINPUT>() as u32;
        assert_eq!(mouse_buttons(&input, size), Some(0));
        assert_eq!(mouse_buttons(&input, size - 1), None);
        assert_eq!(mouse_buttons(&input, u32::MAX), None);
        input.header.dwType = RIM_TYPEKEYBOARD.0;
        assert_eq!(mouse_buttons(&input, size), None);
    }

    #[test]
    fn button_dispatch_skips_movement_and_unowned_input() {
        for buttons in [0, 0x0400] {
            notify_buttons(
                buttons,
                || panic!("movement and wheel input need no ownership lookup"),
                || panic!("movement and wheel input need no button metadata"),
            );
        }
        notify_buttons(
            RI_MOUSE_MIDDLE_BUTTON_DOWN as u16,
            || None,
            || panic!("unowned input needs no button metadata"),
        );
    }

    #[test]
    fn button_registration_deduplicates_and_removes_only_the_matching_window() {
        let owner = HWND(std::ptr::dangling_mut());
        let other = HWND(2_usize as *mut _);
        for window in [owner, owner, other] {
            set_button_listener(window, count_button_report, true);
        }
        let metadata = || (POINT::default(), false, Duration::ZERO);
        let buttons = RI_MOUSE_MIDDLE_BUTTON_DOWN as u16;
        notify_buttons(buttons, || Some(owner), metadata);
        assert_eq!(BUTTON_REPORTS.get(), 1);
        set_button_listener(owner, count_button_report, false);
        notify_buttons(buttons, || Some(owner), metadata);
        assert_eq!(BUTTON_REPORTS.get(), 1);
        notify_buttons(buttons, || Some(other), metadata);
        assert_eq!(BUTTON_REPORTS.get(), 2);
    }

    #[test]
    fn button_listeners_keep_raw_input_registered_while_movement_settles() {
        let owner = HWND(std::ptr::dangling_mut());
        assert!(wants_registration(Some(owner), true, || false));
        assert!(!wants_registration(None, true, || true));
        assert!(wants_registration(None, false, || true));
        assert!(!wants_registration(None, false, || false));
    }
}
