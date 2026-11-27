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

#[derive(Clone, Copy)]
struct Click {
    target: HWND,
    created: Instant,
    shift: bool,
    released: bool,
}

thread_local! {
    static OWNER: Cell<Option<HWND>> = const { Cell::new(None) };
    static CLICK: Cell<Option<Click>> = const { Cell::new(None) };
}

/// Observe the foreground browser window from its UI thread. Child keyboard
/// focus may change in the middle of a click.
pub fn watch_middle_clicks(owner: Option<HWND>) {
    if OWNER.replace(owner) != owner {
        CLICK.set(None);
    }
}

/// Receives mouse button flags from the desktop's existing raw-input sink.
/// This notification runs after input delivery and cannot block the pointer.
/// The sink has already selected the foreground owner for this message.
pub fn record_middle_click(buttons: u16, position: POINT, shift: bool, age: Duration) {
    let Some(owner) = OWNER.get() else {
        return;
    };
    capture_message(buttons, position, shift, age, |position| {
        // SAFETY: native window queries on this UI thread using the message's
        // cursor position, even if the physical cursor has since moved.
        unsafe {
            let target = WindowFromPoint(position);
            eligible_target(owner, target, GetAncestor(target, GA_ROOT))
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
    let now = Instant::now();
    let Some(created) = now.checked_sub(age).filter(|created| fresh(*created, now)) else {
        CLICK.set(None);
        return;
    };
    capture_input(buttons, target_at(position), shift, created);
}

fn eligible_target(owner: HWND, target: HWND, root: HWND) -> Option<HWND> {
    (!target.0.is_null() && root == owner).then_some(target)
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
        CLICK.set(None);
        return;
    }
    if buttons & RI_MOUSE_MIDDLE_BUTTON_DOWN != 0 {
        CLICK.set(target.map(|target| Click {
            target,
            created,
            shift,
            released: false,
        }));
    }
    if buttons & RI_MOUSE_MIDDLE_BUTTON_UP != 0 {
        let pressed = CLICK.take();
        // A callback may already have consumed the press. Its release must
        // never create a second hint for the next request from that opener.
        CLICK.set(
            pressed
                .filter(|press| {
                    !press.released
                        && target == Some(press.target)
                        && created.saturating_duration_since(press.created) < MAX_AGE
                })
                .map(|press| Click {
                    target: press.target,
                    created,
                    shift,
                    released: true,
                }),
        );
    }
}

pub(crate) fn foreground(opener: HWND, clicked: bool, fallback: bool) -> bool {
    take_foreground(clicked, fallback, Instant::now(), |target| {
        let Some(owner) = OWNER.get() else {
            return false;
        };
        // SAFETY: both calls only query window ownership on the UI thread.
        let (front, root) = unsafe { (GetForegroundWindow(), GetAncestor(opener, GA_ROOT)) };
        if owner != front {
            watch_middle_clicks(None);
            return false;
        }
        owner == root && belongs_to(opener, target)
    })
}

pub(crate) fn forget_view(opener: HWND) {
    if CLICK
        .get()
        .is_some_and(|click| belongs_to(opener, click.target))
    {
        CLICK.set(None);
    }
}

fn belongs_to(opener: HWND, target: HWND) -> bool {
    // SAFETY: IsChild only queries the relationship between native handle values.
    target == opener || unsafe { IsChild(opener, target) }.as_bool()
}

fn fresh(created: Instant, now: Instant) -> bool {
    now.checked_duration_since(created)
        .is_some_and(|age| age < MAX_AGE)
}

fn take_foreground(
    clicked: bool,
    fallback: bool,
    now: Instant,
    matches_opener: impl FnOnce(HWND) -> bool,
) -> bool {
    let Some(click) = CLICK.get() else {
        return fallback;
    };
    if !fresh(click.created, now) {
        CLICK.set(None);
        return fallback;
    }
    if !clicked || !matches_opener(click.target) {
        return fallback;
    }
    CLICK.set(None);
    click.shift
}

#[cfg(test)]
mod tests {
    use super::*;

    fn capture(buttons: u16, target: Option<HWND>, shift: bool) {
        capture_input(buttons, target, shift, Instant::now());
    }

    fn disposition(opener: HWND, fallback: bool) -> bool {
        take_foreground(true, fallback, Instant::now(), |target| target == opener)
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
        assert!(disposition(target, false));
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
            assert!(disposition(target, true));
            assert!(CLICK.get().is_none());
        }
    }

    #[test]
    fn raw_button_capture_requires_a_matching_press_and_release() {
        let target = HWND(std::ptr::dangling_mut());
        for shift in [false, true] {
            capture(RI_MOUSE_MIDDLE_BUTTON_DOWN as u16, Some(target), shift);
            assert!(CLICK.get().is_some_and(|click| !click.released));
            capture(RI_MOUSE_MIDDLE_BUTTON_UP as u16, Some(target), shift);
            assert!(CLICK.get().is_some_and(|click| click.released));
            assert_eq!(disposition(target, !shift), shift);
            assert_eq!(disposition(target, !shift), !shift);
        }
    }

    #[test]
    fn capture_rejects_other_owners_and_mismatched_targets() {
        let owner = HWND(std::ptr::dangling_mut());
        let other = HWND(2_usize as *mut _);
        assert_eq!(eligible_target(owner, other, owner), Some(other));
        assert_eq!(eligible_target(owner, other, other), None);
        assert_eq!(eligible_target(owner, HWND::default(), owner), None);
        for rejected in [None, Some(other)] {
            capture(RI_MOUSE_MIDDLE_BUTTON_DOWN as u16, Some(owner), false);
            capture(RI_MOUSE_MIDDLE_BUTTON_UP as u16, rejected, false);
            assert!(disposition(owner, true));
        }
        capture(RI_MOUSE_MIDDLE_BUTTON_DOWN as u16, None, false);
        capture(RI_MOUSE_MIDDLE_BUTTON_UP as u16, Some(owner), false);
        assert!(disposition(owner, true));
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
            assert!(disposition(target, true));
            capture(RI_MOUSE_MIDDLE_BUTTON_UP as u16, Some(target), false);
            assert!(disposition(target, true));
            capture(RI_MOUSE_MIDDLE_BUTTON_DOWN as u16, Some(target), false);
            capture(RI_MOUSE_MIDDLE_BUTTON_UP as u16, Some(target), false);
            capture(cancel as u16, Some(target), false);
            assert!(disposition(target, true));
        }
    }

    #[test]
    fn released_middle_click_still_opens_in_background() {
        let target = HWND(std::ptr::dangling_mut());
        capture(RI_MOUSE_MIDDLE_BUTTON_DOWN as u16, Some(target), false);
        capture(RI_MOUSE_MIDDLE_BUTTON_UP as u16, Some(target), false);
        assert!(!disposition(target, true));
        assert!(disposition(target, true));
    }

    #[test]
    fn shift_middle_click_opens_in_foreground() {
        let target = HWND(std::ptr::dangling_mut());
        capture(RI_MOUSE_MIDDLE_BUTTON_DOWN as u16, Some(target), false);
        capture(RI_MOUSE_MIDDLE_BUTTON_UP as u16, Some(target), true);
        assert!(disposition(target, false));
    }

    #[test]
    fn stale_or_missing_click_preserves_native_disposition() {
        let target = HWND(std::ptr::dangling_mut());
        let created = Instant::now();
        for released in [false, true] {
            CLICK.set(Some(Click {
                target,
                created,
                shift: false,
                released,
            }));
            assert!(take_foreground(true, true, created + MAX_AGE, |_| true));
            assert!(CLICK.get().is_none());
            assert!(!take_foreground(false, false, created, |_| true));
        }
    }

    #[test]
    fn timed_out_press_cannot_create_a_receipt_on_release() {
        let target = HWND(std::ptr::dangling_mut());
        let pressed = Instant::now();
        capture_input(
            RI_MOUSE_MIDDLE_BUTTON_DOWN as u16,
            Some(target),
            false,
            pressed,
        );
        capture_input(
            RI_MOUSE_MIDDLE_BUTTON_UP as u16,
            Some(target),
            false,
            pressed + MAX_AGE,
        );
        assert!(CLICK.get().is_none());
    }

    #[test]
    fn click_is_consumed_once() {
        let target = HWND::default();
        CLICK.set(Some(Click {
            target,
            created: Instant::now(),
            shift: false,
            released: true,
        }));
        assert!(!disposition(target, true));
        assert!(disposition(target, true));
    }

    #[test]
    fn click_for_another_view_preserves_native_disposition() {
        let target = HWND(std::ptr::dangling_mut());
        let other = HWND(2_usize as *mut _);
        for released in [false, true] {
            capture(RI_MOUSE_MIDDLE_BUTTON_DOWN as u16, Some(target), false);
            if released {
                capture(RI_MOUSE_MIDDLE_BUTTON_UP as u16, Some(target), false);
            }
            assert!(disposition(other, true));
            assert!(!disposition(target, true));
        }
    }

    #[test]
    fn foreground_updates_only_clear_input_when_the_owner_changes() {
        let owner = HWND(std::ptr::dangling_mut());
        watch_middle_clicks(Some(owner));
        capture(RI_MOUSE_MIDDLE_BUTTON_DOWN as u16, Some(owner), false);
        watch_middle_clicks(Some(owner));
        assert_eq!(CLICK.get().map(|click| click.target), Some(owner));
        watch_middle_clicks(None);
        assert!(CLICK.get().is_none());
    }

    #[test]
    fn foreground_refusal_cannot_create_a_receipt() {
        let owner = HWND(std::ptr::dangling_mut());
        let other = HWND(2_usize as *mut _);
        watch_middle_clicks(Some(owner));
        capture(RI_MOUSE_MIDDLE_BUTTON_DOWN as u16, Some(owner), false);
        watch_middle_clicks(Some(other));
        capture(
            RI_MOUSE_MIDDLE_BUTTON_UP as u16,
            eligible_target(other, owner, owner),
            false,
        );
        assert!(disposition(owner, true));
        assert!(CLICK.get().is_none());
    }

    #[test]
    fn teardown_clears_pressed_and_released_input() {
        let owner = HWND(std::ptr::dangling_mut());
        for released in [false, true] {
            watch_middle_clicks(Some(owner));
            capture(RI_MOUSE_MIDDLE_BUTTON_DOWN as u16, Some(owner), false);
            if released {
                capture(RI_MOUSE_MIDDLE_BUTTON_UP as u16, Some(owner), false);
            }
            watch_middle_clicks(None);
            assert!(CLICK.get().is_none());
            assert!(disposition(owner, true));
            capture(RI_MOUSE_MIDDLE_BUTTON_UP as u16, Some(owner), false);
            assert!(disposition(owner, true));
        }
    }

    #[test]
    fn request_before_release_consumes_the_press_once() {
        let owner = HWND(std::ptr::dangling_mut());
        for shift in [false, true] {
            capture(RI_MOUSE_MIDDLE_BUTTON_DOWN as u16, Some(owner), shift);
            assert_eq!(disposition(owner, !shift), shift);
            assert_eq!(disposition(owner, !shift), !shift);
            capture(RI_MOUSE_MIDDLE_BUTTON_UP as u16, Some(owner), shift);
            assert_eq!(disposition(owner, !shift), !shift);
        }
    }

    #[test]
    fn automatic_request_leaves_the_hint_for_a_clicked_request() {
        let owner = HWND(std::ptr::dangling_mut());
        capture(RI_MOUSE_MIDDLE_BUTTON_DOWN as u16, Some(owner), false);
        assert!(take_foreground(false, true, Instant::now(), |_| {
            panic!("an automatic request must not match a click")
        }));
        assert!(!disposition(owner, true));
    }

    #[test]
    fn closing_the_view_clears_its_hint() {
        let target = HWND(std::ptr::dangling_mut());
        let other = HWND(2_usize as *mut _);
        for released in [false, true] {
            capture(RI_MOUSE_MIDDLE_BUTTON_DOWN as u16, Some(target), false);
            if released {
                capture(RI_MOUSE_MIDDLE_BUTTON_UP as u16, Some(target), false);
            }
            forget_view(other);
            assert!(CLICK.get().is_some());
            forget_view(target);
            assert!(disposition(target, true));
            capture(RI_MOUSE_MIDDLE_BUTTON_UP as u16, Some(target), false);
            assert!(disposition(target, true));
        }
    }

    #[test]
    fn two_rapid_clicks_are_consumed_with_either_callback_order() {
        let owner = HWND(std::ptr::dangling_mut());
        capture(RI_MOUSE_MIDDLE_BUTTON_DOWN as u16, Some(owner), false);
        assert!(!disposition(owner, true));
        capture(RI_MOUSE_MIDDLE_BUTTON_UP as u16, Some(owner), false);
        capture(RI_MOUSE_MIDDLE_BUTTON_DOWN as u16, Some(owner), true);
        capture(RI_MOUSE_MIDDLE_BUTTON_UP as u16, Some(owner), true);
        assert!(disposition(owner, false));
        assert!(!disposition(owner, false));
    }

    #[test]
    fn rapid_clicks_replace_unclaimed_receipts_and_remain_single_use() {
        let owner = HWND(std::ptr::dangling_mut());
        for shift in [false, true, false, true] {
            capture(RI_MOUSE_MIDDLE_BUTTON_DOWN as u16, Some(owner), !shift);
            capture(RI_MOUSE_MIDDLE_BUTTON_UP as u16, Some(owner), !shift);
            capture(RI_MOUSE_MIDDLE_BUTTON_DOWN as u16, Some(owner), shift);
            capture(RI_MOUSE_MIDDLE_BUTTON_UP as u16, Some(owner), shift);
            assert_eq!(disposition(owner, !shift), shift);
            assert_eq!(disposition(owner, !shift), !shift);
        }
    }
}
