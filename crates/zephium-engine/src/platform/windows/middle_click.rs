//! WebView2 does not include mouse modifiers in its new-window event. Keep a
//! short-lived selection hint for a middle click delivered to the opener.

use std::cell::Cell;
use std::time::{Duration, Instant};
use windows::Win32::Foundation::{HWND, POINT};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_SHIFT};
use windows::Win32::UI::WindowsAndMessaging::{
    GetAncestor, GetCursorPos, GetForegroundWindow, IsChild, WindowFromPoint, GA_ROOT,
    RI_MOUSE_BUTTON_4_DOWN, RI_MOUSE_BUTTON_5_DOWN, RI_MOUSE_LEFT_BUTTON_DOWN,
    RI_MOUSE_MIDDLE_BUTTON_DOWN, RI_MOUSE_MIDDLE_BUTTON_UP, RI_MOUSE_RIGHT_BUTTON_DOWN,
};

const MAX_AGE: Duration = Duration::from_millis(250);

struct Click {
    target: HWND,
    released: Instant,
    shift: bool,
}

thread_local! {
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
}

/// Receives mouse button flags from the desktop's existing raw-input sink.
/// This notification runs after input delivery and cannot block the pointer.
pub fn record_middle_click(buttons: u16) {
    let Some(owner) = OWNER.get() else {
        return;
    };
    let mut point = POINT::default();
    // SAFETY: queries on this UI thread, with an initialized cursor out value.
    let (target, root, front, shift) = unsafe {
        let target = GetCursorPos(&mut point)
            .map(|()| WindowFromPoint(point))
            .unwrap_or_default();
        (
            target,
            GetAncestor(target, GA_ROOT),
            GetForegroundWindow(),
            GetAsyncKeyState(i32::from(VK_SHIFT.0)) < 0,
        )
    };
    capture(buttons, eligible_target(owner, target, root, front), shift);
}

fn eligible_target(owner: HWND, target: HWND, root: HWND, front: HWND) -> Option<HWND> {
    (!target.0.is_null() && root == owner && front == owner).then_some(target)
}

fn capture(buttons: u16, target: Option<HWND>, shift: bool) {
    let buttons = u32::from(buttons);
    if buttons
        & (RI_MOUSE_LEFT_BUTTON_DOWN
            | RI_MOUSE_RIGHT_BUTTON_DOWN
            | RI_MOUSE_BUTTON_4_DOWN
            | RI_MOUSE_BUTTON_5_DOWN)
        != 0
    {
        PRESSED.set(None);
        CLICK.set(None);
        return;
    }
    if buttons & RI_MOUSE_MIDDLE_BUTTON_DOWN != 0 {
        CLICK.set(None);
        PRESSED.set(target);
    }
    if buttons & RI_MOUSE_MIDDLE_BUTTON_UP != 0 {
        let pressed = PRESSED.take();
        CLICK.set(
            target
                .filter(|target| Some(*target) == pressed)
                .map(|target| Click {
                    target,
                    released: Instant::now(),
                    shift,
                }),
        );
    }
}

pub(crate) fn take_foreground(opener: HWND, fallback: bool) -> bool {
    // A receipt can only affect selection for the physical view that received
    // the click. It does not grant popup or navigation admission.
    let click = CLICK.take().and_then(|click| {
        if click.released.elapsed() >= MAX_AGE {
            return None;
        }
        if click.target == opener || unsafe { IsChild(opener, click.target) }.as_bool() {
            Some(click)
        } else {
            CLICK.set(Some(click));
            None
        }
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
    fn raw_button_capture_requires_a_matching_press_and_release() {
        let target = HWND(std::ptr::dangling_mut());
        for shift in [false, true] {
            capture(RI_MOUSE_MIDDLE_BUTTON_DOWN as u16, Some(target), shift);
            assert!(CLICK.take().is_none());
            capture(RI_MOUSE_MIDDLE_BUTTON_UP as u16, Some(target), shift);
            assert_eq!(take_foreground(target, !shift), shift);
            assert_eq!(take_foreground(target, !shift), !shift);
        }
    }

    #[test]
    fn capture_rejects_other_owners_and_mismatched_targets() {
        let owner = HWND(std::ptr::dangling_mut());
        let other = HWND(2_usize as *mut _);
        assert_eq!(eligible_target(owner, other, owner, owner), Some(other));
        assert_eq!(eligible_target(owner, other, owner, other), None);
        assert_eq!(eligible_target(owner, other, other, owner), None);
        assert_eq!(eligible_target(owner, HWND::default(), owner, owner), None);
        for rejected in [None, Some(other)] {
            capture(RI_MOUSE_MIDDLE_BUTTON_DOWN as u16, Some(owner), false);
            capture(RI_MOUSE_MIDDLE_BUTTON_UP as u16, rejected, false);
            assert!(take_foreground(owner, true));
        }
        capture(RI_MOUSE_MIDDLE_BUTTON_DOWN as u16, None, false);
        capture(RI_MOUSE_MIDDLE_BUTTON_UP as u16, Some(owner), false);
        assert!(take_foreground(owner, true));
    }

    #[test]
    fn another_button_cancels_pressed_and_released_middle_clicks() {
        let target = HWND(std::ptr::dangling_mut());
        for cancel in [
            RI_MOUSE_LEFT_BUTTON_DOWN,
            RI_MOUSE_RIGHT_BUTTON_DOWN,
            RI_MOUSE_BUTTON_4_DOWN,
            RI_MOUSE_BUTTON_5_DOWN,
        ] {
            capture(RI_MOUSE_MIDDLE_BUTTON_DOWN as u16, Some(target), false);
            capture(cancel as u16, Some(target), false);
            capture(RI_MOUSE_MIDDLE_BUTTON_UP as u16, Some(target), false);
            assert!(take_foreground(target, true));
            capture(RI_MOUSE_MIDDLE_BUTTON_DOWN as u16, Some(target), false);
            capture(RI_MOUSE_MIDDLE_BUTTON_UP as u16, Some(target), false);
            capture(cancel as u16, Some(target), false);
            assert!(take_foreground(target, true));
        }
    }

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
        assert!(!take_foreground(HWND::default(), true));
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
