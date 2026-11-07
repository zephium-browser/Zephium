//! Pointer moves over this process's windows, pages included. A page's
//! WebView2 belongs to another process, so none of its moves reach this
//! thread; raw mouse input does. It is registered only while a listening
//! window is in front, and read at most once a frame: each move unregisters
//! it until the next frame, so a resting pointer costs nothing at all.
//!
//! Raw input rather than a low-level mouse hook: the hook would sit in the
//! path of every move on the desktop, and the system drops one that is slow
//! to answer. Raw input is only told about moves after the fact.

use std::cell::{Cell, RefCell};

use windows::core::w;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::UI::Accessibility::{SetWinEventHook, UnhookWinEvent, HWINEVENTHOOK};
use windows::Win32::UI::Input::{
    RegisterRawInputDevices, RAWINPUTDEVICE, RIDEV_INPUTSINK, RIDEV_REMOVE,
};
use windows::Win32::UI::Shell::{DefSubclassProc, SetWindowSubclass};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DestroyWindow, GetAncestor, GetForegroundWindow, IsIconic, IsWindow,
    KillTimer, SetTimer, EVENT_SYSTEM_FOREGROUND, EVENT_SYSTEM_MINIMIZEEND,
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

struct Sink {
    hwnd: HWND,
    hooks: [HWINEVENTHOOK; 2],
}

thread_local! {
    static LISTENERS: RefCell<Vec<(HWND, Listener)>> = const { RefCell::new(Vec::new()) };
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

fn ensure_sink() -> bool {
    if let Some(sink) = SINK.take() {
        SINK.set(Some(sink));
        return true;
    }
    // SAFETY: a message-only window and out-of-context hooks owned by this
    // thread; both deliver through this thread's message loop.
    unsafe {
        let Ok(hwnd) = CreateWindowExW(
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
        ) else {
            return false;
        };
        if !SetWindowSubclass(hwnd, Some(sink_proc), SUBCLASS, 0).as_bool() {
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
fn listening_in_front() -> bool {
    // SAFETY: plain reads of the window in front and its owner.
    let front = unsafe {
        let front = GetForegroundWindow();
        (!front.0.is_null()).then(|| GetAncestor(front, GA_ROOTOWNER))
    };
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
    let wanted = listening_in_front() && !SETTLING.get();
    if wanted != REGISTERED.get() {
        register(wanted);
    }
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
    if done.is_ok() {
        REGISTERED.set(on);
        RETRIES.set(0);
    } else if on {
        retry_later();
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
        if LISTENERS.with(|listeners| listeners.borrow().is_empty()) {
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
    _: u32,
    _: HWND,
    _: i32,
    _: i32,
    _: u32,
    _: u32,
) {
    let _ = std::panic::catch_unwind(|| {
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
        // Moves queued before the registration lapsed still arrive; the
        // frame they fall in has already been read.
        WM_INPUT if !SETTLING.get() => {
            // Input is held back only while a timer will end the frame;
            // without one it keeps arriving unthrottled rather than never.
            if SetTimer(Some(hwnd), SETTLE, FRAME_MS, None) != 0 {
                SETTLING.set(true);
                register(false);
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
