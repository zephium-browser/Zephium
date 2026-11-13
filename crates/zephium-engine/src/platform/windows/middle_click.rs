//! WebView2 does not include mouse modifiers in its new-window event. Keep a
//! short-lived selection hint for a middle click delivered to the opener.

use std::cell::Cell;
use std::time::{Duration, Instant};
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_SHIFT};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, GetAncestor, GetForegroundWindow, IsChild, SetWindowsHookExW,
    UnhookWindowsHookEx, WindowFromPoint, GA_ROOT, HHOOK, MSLLHOOKSTRUCT, WH_MOUSE_LL,
    WM_LBUTTONDOWN, WM_MBUTTONDOWN, WM_MBUTTONUP, WM_RBUTTONDOWN, WM_XBUTTONDOWN,
};

const MAX_AGE: Duration = Duration::from_millis(250);

struct Click {
    target: HWND,
    released: Instant,
    shift: bool,
}

thread_local! {
    static HOOK: Cell<Option<HHOOK>> = const { Cell::new(None) };
    static OWNER: Cell<Option<HWND>> = const { Cell::new(None) };
    static PRESSED: Cell<Option<HWND>> = const { Cell::new(None) };
    static CLICK: Cell<Option<Click>> = const { Cell::new(None) };
}

/// Observe the foreground browser window from its UI thread. Child keyboard
/// focus may change in the middle of a click.
pub fn watch_middle_clicks(owner: Option<HWND>) {
    if OWNER.replace(owner) == owner {
        return;
    }
    PRESSED.set(None);
    CLICK.set(None);
    if owner.is_some() {
        if HOOK.get().is_none() {
            // SAFETY: the callback runs on this UI thread, which pumps messages.
            let hook = unsafe {
                GetModuleHandleW(None).and_then(|module| {
                    SetWindowsHookExW(WH_MOUSE_LL, Some(mouse), Some(HINSTANCE(module.0)), 0)
                })
            };
            HOOK.set(hook.ok());
        }
    } else if let Some(hook) = HOOK.get() {
        // SAFETY: this thread owns the registered hook. Keep its handle if
        // removal fails so a later focus change never installs a duplicate.
        if unsafe { UnhookWindowsHookEx(hook) }.is_ok() {
            HOOK.set(None);
        }
    }
}

unsafe extern "system" fn mouse(code: i32, message: WPARAM, data: LPARAM) -> LRESULT {
    if let Some(owner) = OWNER
        .try_with(Cell::get)
        .ok()
        .flatten()
        .filter(|_| code >= 0 && data.0 != 0)
    {
        match message.0 as u32 {
            WM_MBUTTONDOWN | WM_MBUTTONUP => {
                // SAFETY: for these messages the hook supplies this struct
                // for the duration of the callback.
                let event = unsafe { &*(data.0 as *const MSLLHOOKSTRUCT) };
                let target = unsafe { WindowFromPoint(event.pt) };
                let target = (!target.0.is_null()
                    && unsafe {
                        GetAncestor(target, GA_ROOT) == owner && GetForegroundWindow() == owner
                    })
                .then_some(target);
                if message.0 as u32 == WM_MBUTTONDOWN {
                    CLICK.set(None);
                    PRESSED.set(target);
                } else {
                    let pressed = PRESSED.take();
                    CLICK.set(
                        target
                            .filter(|target| Some(*target) == pressed)
                            .map(|target| Click {
                                target,
                                released: Instant::now(),
                                shift: unsafe { GetAsyncKeyState(i32::from(VK_SHIFT.0)) } < 0,
                            }),
                    );
                }
            }
            WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_XBUTTONDOWN => {
                PRESSED.set(None);
                CLICK.set(None);
            }
            _ => {}
        }
    }
    // SAFETY: forward the original event without handling or changing it.
    unsafe { CallNextHookEx(None, code, message, data) }
}

pub(crate) fn take_foreground(opener: HWND, fallback: bool) -> bool {
    // A receipt can only affect selection for the physical view that received
    // the click. It does not grant popup or navigation admission.
    let click = CLICK.take().filter(|click| {
        click.target == opener || unsafe { IsChild(opener, click.target) }.as_bool()
    });
    foreground(fallback, click, Instant::now())
}

fn foreground(fallback: bool, click: Option<Click>, now: Instant) -> bool {
    click
        .filter(|click| {
            now.checked_duration_since(click.released)
                .is_some_and(|age| age < MAX_AGE)
        })
        .map_or(fallback, |click| click.shift)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn released_middle_click_still_opens_in_background() {
        let released = Instant::now();
        let click = Click {
            target: HWND::default(),
            released,
            shift: false,
        };
        assert!(!foreground(true, Some(click), released));
    }

    #[test]
    fn shift_middle_click_opens_in_foreground() {
        let released = Instant::now();
        let click = Click {
            target: HWND::default(),
            released,
            shift: true,
        };
        assert!(foreground(false, Some(click), released));
    }

    #[test]
    fn stale_or_missing_click_preserves_native_disposition() {
        let released = Instant::now();
        let click = Click {
            target: HWND::default(),
            released,
            shift: false,
        };
        assert!(foreground(true, Some(click), released + MAX_AGE));
        assert!(!foreground(false, None, released));
    }

    #[test]
    fn click_is_consumed_once() {
        let target = HWND::default();
        CLICK.set(Some(Click {
            target,
            released: Instant::now(),
            shift: false,
        }));
        assert!(!take_foreground(target, true));
        assert!(take_foreground(target, true));
    }

    #[test]
    fn click_for_another_view_preserves_native_disposition() {
        CLICK.set(Some(Click {
            target: HWND::default(),
            released: Instant::now(),
            shift: false,
        }));
        let other = HWND(std::ptr::dangling_mut());
        assert!(take_foreground(other, true));
        assert!(CLICK.take().is_none());
    }

    #[test]
    fn foreground_updates_only_clear_input_when_the_owner_changes() {
        let owner = HWND(std::ptr::dangling_mut());
        OWNER.set(Some(owner));
        PRESSED.set(Some(owner));
        watch_middle_clicks(Some(owner));
        assert_eq!(PRESSED.get(), Some(owner));
        watch_middle_clicks(None);
        assert_eq!(PRESSED.get(), None);
    }
}
