//! WebView2 does not include mouse modifiers in its new-window event. Keep a
//! short-lived selection hint for a middle click delivered to the opener.

use std::cell::Cell;
use std::time::{Duration, Instant};
use windows::Win32::Foundation::{HWND, POINT};
use windows::Win32::UI::WindowsAndMessaging::{
    GetAncestor, GetForegroundWindow, IsChild, WindowFromPoint, GA_ROOT, RI_MOUSE_BUTTON_4_DOWN,
    RI_MOUSE_BUTTON_5_DOWN, RI_MOUSE_LEFT_BUTTON_DOWN, RI_MOUSE_MIDDLE_BUTTON_DOWN,
    RI_MOUSE_MIDDLE_BUTTON_UP, RI_MOUSE_RIGHT_BUTTON_DOWN,
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
/// Position, modifiers and age describe the queued input message.
pub fn record_middle_click(buttons: u16, position: POINT, shift: bool, age: Duration) {
    let Some(owner) = OWNER.get() else {
        return;
    };
    capture_message(buttons, position, shift, age, |position| {
        // SAFETY: native window queries on this UI thread using the message's
        // cursor position, even if the physical cursor has since moved.
        unsafe {
            let target = WindowFromPoint(position);
            eligible_target(
                owner,
                target,
                GetAncestor(target, GA_ROOT),
                GetForegroundWindow(),
            )
        }
    });
}

fn capture_message(
    buttons: u16,
    position: POINT,
    shift: bool,
    age: Duration,
    target_at: impl FnOnce(POINT) -> Option<HWND>,
) {
    let Some(created) = Instant::now().checked_sub(age).filter(|_| age < MAX_AGE) else {
        PRESSED.set(None);
        CLICK.set(None);
        return;
    };
    capture_input(buttons, target_at(position), shift, created);
}

fn eligible_target(owner: HWND, target: HWND, root: HWND, front: HWND) -> Option<HWND> {
    (!target.0.is_null() && root == owner && front == owner).then_some(target)
}

fn capture_input(buttons: u16, target: Option<HWND>, shift: bool, created: Instant) {
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
                    released: created,
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

    fn capture(buttons: u16, target: Option<HWND>, shift: bool) {
        capture_input(buttons, target, shift, Instant::now());
    }

    #[test]
    fn queued_click_keeps_its_event_position_and_shift() {
        let target = HWND(std::ptr::dangling_mut());
        let event_position = POINT { x: -100, y: 50 };
        let current_position = POINT { x: 600, y: 50 };
        for buttons in [RI_MOUSE_MIDDLE_BUTTON_DOWN, RI_MOUSE_MIDDLE_BUTTON_UP] {
            capture_message(
                buttons as u16,
                event_position,
                true,
                Duration::from_millis(20),
                |position| {
                    assert_eq!(position.x, event_position.x);
                    assert_ne!(position.x, current_position.x);
                    Some(target)
                },
            );
        }
        assert!(take_foreground(target, false));
    }

    #[test]
    fn queued_release_expires_from_message_creation() {
        let target = HWND(std::ptr::dangling_mut());
        for age in [MAX_AGE, Duration::from_millis(400)] {
            capture(RI_MOUSE_MIDDLE_BUTTON_DOWN as u16, Some(target), false);
            capture_message(
                RI_MOUSE_MIDDLE_BUTTON_UP as u16,
                POINT::default(),
                false,
                age,
                |_| panic!("an expired event must not look up a target"),
            );
            assert!(take_foreground(target, true));
            assert_eq!(PRESSED.take(), None);
        }
    }

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
