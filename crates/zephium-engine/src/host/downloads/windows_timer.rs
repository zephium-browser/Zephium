//! One reserved download-coordinator timer per UI thread, separate from agent capacity.
use std::cell::RefCell;
use std::ffi::c_void;
use std::marker::PhantomData;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::Duration;
use windows::core::w;
use windows::Win32::Foundation::{ERROR_CLASS_ALREADY_EXISTS, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, KillTimer, PostMessageW, RegisterClassW,
    SetTimer, HWND_MESSAGE, WM_APP, WNDCLASSW,
};
type Callback = Box<dyn FnOnce()>;
type WakeCallback = Rc<dyn Fn()>;
type WakeEntry = (isize, WakeCallback, Weak<UiWake>);
const WM_DOWNLOAD_WAKE: u32 = WM_APP + 0x44;
static WAKE_CLASS: OnceLock<bool> = OnceLock::new();
thread_local! {
    static PENDING: RefCell<Option<(usize, Callback)>> = const { RefCell::new(None) };
    static WAKE_TARGETS: RefCell<Vec<WakeEntry>> = const { RefCell::new(Vec::new()) };
}
pub(in crate::host::downloads) struct DownloadTimer {
    id: usize,
    _thread: PhantomData<Rc<()>>,
}
pub(in crate::host::downloads) struct UiWake {
    hwnd: Mutex<isize>,
    armed: AtomicBool,
    pending: AtomicBool,
    posted_since_drain: AtomicBool,
}
pub(in crate::host::downloads) struct WakeWindow {
    raw: isize,
    wake: Arc<UiWake>,
    _thread: PhantomData<Rc<()>>,
}
impl Drop for DownloadTimer {
    fn drop(&mut self) {
        unsafe {
            let _ = KillTimer(None, self.id);
        }
        let _ = PENDING.try_with(|slot| {
            let Ok(mut slot) = slot.try_borrow_mut() else {
                return;
            };
            if slot.as_ref().is_some_and(|(id, _)| *id == self.id) {
                slot.take();
            }
        });
    }
}
unsafe extern "system" fn fired(_: HWND, _: u32, id: usize, _: u32) {
    let callback = PENDING
        .try_with(|slot| {
            let Ok(mut slot) = slot.try_borrow_mut() else {
                return None;
            };
            if slot.as_ref().is_some_and(|(expected, _)| *expected == id) {
                slot.take().map(|(_, callback)| callback)
            } else {
                None
            }
        })
        .ok()
        .flatten();
    if let Some(callback) = callback {
        unsafe {
            let _ = KillTimer(None, id);
        }
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(callback));
    }
}
pub(super) fn schedule(
    duration: Duration,
    callback: impl FnOnce() + 'static,
) -> Option<DownloadTimer> {
    if !PENDING.with(|slot| slot.borrow().is_none()) {
        return None;
    }
    let milliseconds = u32::try_from(duration.as_millis()).ok()?.max(10);
    // SAFETY: fixed ABI callback and a window-less timer on this UI thread.
    let id = unsafe { SetTimer(None, 0, milliseconds, Some(fired)) };
    if id == 0 {
        return None;
    }
    PENDING.with(|slot| *slot.borrow_mut() = Some((id, Box::new(callback))));
    Some(DownloadTimer {
        id,
        _thread: PhantomData,
    })
}

impl Drop for WakeWindow {
    fn drop(&mut self) {
        {
            let mut guard = self.wake.hwnd.lock().unwrap_or_else(|e| e.into_inner());
            *guard = 0;
        }
        let _ = WAKE_TARGETS.try_with(|targets| {
            let Ok(mut targets) = targets.try_borrow_mut() else {
                return;
            };
            targets.retain(|(raw, _, _)| *raw != self.raw);
        });
        // SAFETY: `WakeWindow` is created and dropped on the same UI STA thread;
        // `self.wake.hwnd` was cleared above so no future `PostMessageW` targets it.
        unsafe {
            let _ = DestroyWindow(HWND(self.raw as *mut c_void));
        }
    }
}

unsafe extern "system" fn wake_fired(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if msg != WM_DOWNLOAD_WAKE {
        // SAFETY: standard Win32 message pass-through for unhandled messages.
        return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
    }
    let key = hwnd.0 as isize;
    let target = WAKE_TARGETS
        .try_with(|targets| {
            let Ok(targets) = targets.try_borrow() else {
                return None;
            };
            targets
                .iter()
                .find(|(raw, _, _)| *raw == key)
                .map(|(_, callback, wake)| (callback.clone(), wake.clone()))
        })
        .ok()
        .flatten();
    if let Some((callback, wake)) = target {
        if let Some(wake) = wake.upgrade() {
            wake.pending.store(false, Ordering::SeqCst);
        }
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| callback()));
    }
    LRESULT(0)
}

impl UiWake {
    pub(in crate::host::downloads) fn create(
        callback: impl Fn() + 'static,
    ) -> (Arc<Self>, Option<WakeWindow>) {
        let wake = Arc::new(Self {
            hwnd: Mutex::new(0),
            armed: AtomicBool::new(false),
            pending: AtomicBool::new(false),
            posted_since_drain: AtomicBool::new(false),
        });
        let class_name = w!("ZephiumDownloadWake");
        // SAFETY: process-wide HINSTANCE and static `wake_fired` WNDPROC for hidden wake windows.
        let Ok(module) = (unsafe { GetModuleHandleW(None) }) else {
            return (wake, None);
        };
        WAKE_CLASS.get_or_init(|| {
            let class = WNDCLASSW {
                lpfnWndProc: Some(wake_fired),
                hInstance: module.into(),
                lpszClassName: class_name,
                ..Default::default()
            };
            // SAFETY: valid static class descriptor on this process instance.
            let atom = unsafe { RegisterClassW(&class) };
            atom != 0
                || unsafe {
                    windows::Win32::Foundation::GetLastError() == ERROR_CLASS_ALREADY_EXISTS
                }
        });
        // SAFETY: creates a thread-owned message-only window (`HWND_MESSAGE`) with no parent or menu.
        let Ok(hwnd) = (unsafe {
            CreateWindowExW(
                Default::default(),
                class_name,
                w!(""),
                Default::default(),
                0,
                0,
                0,
                0,
                Some(HWND_MESSAGE),
                None,
                Some(module.into()),
                None,
            )
        }) else {
            return (wake, None);
        };
        if hwnd.is_invalid() {
            return (wake, None);
        }
        let raw = hwnd.0 as isize;
        *wake.hwnd.lock().unwrap_or_else(|e| e.into_inner()) = raw;
        let weak = Arc::downgrade(&wake);
        WAKE_TARGETS.with(|targets| {
            targets.borrow_mut().push((raw, Rc::new(callback), weak));
        });
        let window = WakeWindow {
            raw,
            wake: wake.clone(),
            _thread: PhantomData,
        };
        (wake, Some(window))
    }

    pub(in crate::host::downloads) fn begin_drain(&self) {
        self.posted_since_drain.store(false, Ordering::SeqCst);
    }

    pub(in crate::host::downloads) fn on_message_sent(&self) {
        self.posted_since_drain.store(true, Ordering::SeqCst);
        if self.armed.load(Ordering::SeqCst) {
            self.post();
        }
    }

    pub(in crate::host::downloads) fn arm(&self, force_post: bool) {
        self.armed.store(true, Ordering::SeqCst);
        if force_post || self.posted_since_drain.load(Ordering::SeqCst) {
            self.post();
        }
    }

    pub(in crate::host::downloads) fn disarm(&self) {
        self.armed.store(false, Ordering::Release);
    }

    fn post(&self) {
        if self.pending.swap(true, Ordering::AcqRel) {
            return;
        }
        let guard = self.hwnd.lock().unwrap_or_else(|e| e.into_inner());
        let raw = *guard;
        if raw == 0 {
            self.pending.store(false, Ordering::Release);
            return;
        }
        // SAFETY: `raw` is a live `HWND_MESSAGE` protected by `self.hwnd` until cleared
        // before `DestroyWindow`; `WM_DOWNLOAD_WAKE` is a private `WM_APP` code.
        let posted = unsafe {
            PostMessageW(
                Some(HWND(raw as *mut c_void)),
                WM_DOWNLOAD_WAKE,
                WPARAM(0),
                LPARAM(0),
            )
        };
        if posted.is_err() {
            self.pending.store(false, Ordering::Release);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::time::Instant;
    use windows::Win32::UI::WindowsAndMessaging::{DispatchMessageW, PeekMessageW, MSG, PM_REMOVE};

    #[test]
    fn wake_window_delivers_worker_signal_to_ui_thread_without_set_timer() {
        let ticks = Rc::new(Cell::new(0usize));
        let counter = ticks.clone();
        let (wake, window) = UiWake::create(move || {
            counter.set(counter.get() + 1);
        });
        assert!(window.is_some(), "expected HWND_MESSAGE wake window");
        wake.begin_drain();
        wake.arm(false);
        let worker_wake = wake.clone();
        let handle = std::thread::spawn(move || {
            worker_wake.on_message_sent();
        });
        let deadline = Instant::now() + Duration::from_secs(2);
        while ticks.get() == 0 && Instant::now() < deadline {
            let mut msg = MSG::default();
            // SAFETY: thread-owned message loop pumping test thread messages.
            while unsafe { PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE) }.as_bool() {
                // SAFETY: dispatches retrieved `MSG` from `PeekMessageW`.
                unsafe {
                    DispatchMessageW(&msg);
                }
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        handle.join().unwrap();
        assert_eq!(ticks.get(), 1);
        // Verify pre-arm send between `begin_drain` and `arm` still wakes the UI loop.
        wake.begin_drain();
        wake.on_message_sent();
        wake.arm(false);
        let deadline = Instant::now() + Duration::from_secs(2);
        while ticks.get() < 2 && Instant::now() < deadline {
            let mut msg = MSG::default();
            // SAFETY: thread-owned message loop pumping test thread messages.
            while unsafe { PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE) }.as_bool() {
                // SAFETY: dispatches retrieved `MSG` from `PeekMessageW`.
                unsafe {
                    DispatchMessageW(&msg);
                }
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(ticks.get(), 2);
        drop(window);
        // Sending after WakeWindow drops is a clean no-op.
        wake.on_message_sent();
    }
}
