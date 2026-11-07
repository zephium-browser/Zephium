//! Exercise the owner subclass without taking the test desktop's focus.

use super::*;
use std::cell::RefCell;

struct Focus {
    current: HWND,
    foreground: HWND,
    chrome: HWND,
    restored: Vec<HWND>,
}

thread_local! {
    static FOCUS: RefCell<Option<Focus>> = const { RefCell::new(None) };
}

pub(super) fn keyboard_focus() -> Option<HWND> {
    FOCUS.with(|focus| focus.borrow().as_ref().map(|focus| focus.current))
}

pub(super) fn foreground() -> Option<HWND> {
    FOCUS.with(|focus| focus.borrow().as_ref().map(|focus| focus.foreground))
}

pub(super) fn record_focus_target(target: HWND) -> bool {
    FOCUS.with(|focus| {
        let mut focus = focus.borrow_mut();
        let Some(focus) = focus.as_mut() else {
            return false;
        };
        focus.current = target;
        focus.restored.push(target);
        true
    })
}

unsafe extern "system" fn default_activation(
    hwnd: HWND,
    msg: u32,
    w: WPARAM,
    l: LPARAM,
    _: usize,
    _: usize,
) -> LRESULT {
    if msg == WM_ACTIVATE {
        // Model Wry's default activation handing focus to chrome. Intercept
        // before DefWindowProc so the fixture never changes real focus.
        FOCUS.with(|focus| {
            let mut focus = focus.borrow_mut();
            let focus = focus.as_mut().unwrap();
            focus.current = focus.chrome;
        });
        return LRESULT(0);
    }
    DefSubclassProc(hwnd, msg, w, l)
}

struct Fixture(Vec<HWND>);

impl Fixture {
    unsafe fn create(&mut self, parent: Option<HWND>) -> HWND {
        let hwnd = CreateWindowExW(
            WS_EX_NOACTIVATE,
            w!("STATIC"),
            w!(""),
            WS_VISIBLE | if parent.is_some() { WS_CHILD } else { WS_POPUP },
            -10000,
            -10000,
            100,
            100,
            parent,
            None,
            None,
            None,
        )
        .unwrap();
        self.0.push(hwnd);
        hwnd
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        // SAFETY: only this fixture's off-screen windows are destroyed. The
        // owner subclass owns its boxed State through WM_NCDESTROY.
        unsafe {
            for hwnd in self.0.iter().rev() {
                let _ = DestroyWindow(*hwnd);
            }
        }
        FOCUS.with(|focus| *focus.borrow_mut() = None);
    }
}

#[test]
fn owner_captures_before_default_and_restores_only_a_surviving_child() {
    // SAFETY: windows and subclass state are confined to this test thread;
    // activation input/output is recorded instead of changing desktop focus.
    unsafe {
        for invalid in ["", "destroyed", "reparented"] {
            let mut fixture = Fixture(Vec::new());
            let owner = fixture.create(None);
            let child = fixture.create(Some(owner));
            let chrome = fixture.create(Some(owner));
            let other = fixture.create(None);
            FOCUS.with(|focus| {
                *focus.borrow_mut() = Some(Focus {
                    current: child,
                    foreground: owner,
                    chrome,
                    restored: Vec::new(),
                });
            });
            let state = Box::into_raw(Box::new(State {
                owner,
                strip: HWND::default(),
                edge: HWND::default(),
                buttons: [HWND::default(); 3],
                expanded: false,
                suppressed: false,
                suppressions: 0,
                reveal_armed: false,
                menu_open: false,
                named: None,
                reveal_block: None,
                page_left: None,
                dark: true,
                previous_focus: HWND::default(),
                reactivation_focus: HWND::default(),
                // This fixture exercises focus ordering, not caption painting.
                laying_out: true,
                hot_button: HWND::default(),
                last_hot: HWND::default(),
                sizing: false,
                changing_state: false,
                opacity: 0,
                fade_from: 0,
                fade_started: std::time::Instant::now(),
                pressed_button: HWND::default(),
                drag_start: None,
                motion_paused: None,
                surface: None,
                geometry: None,
                render_failed: false,
                near_top: false,
            }));
            assert!(SetWindowSubclass(owner, Some(default_activation), ID + 10, 0).as_bool());
            assert!(SetWindowSubclass(owner, Some(owner_proc), ID, state as usize).as_bool());
            SendMessageW(
                owner,
                WM_ACTIVATE,
                Some(WPARAM(WA_INACTIVE as usize)),
                Some(LPARAM(0)),
            );
            assert_eq!((*state).reactivation_focus, child);
            assert_eq!(keyboard_focus(), Some(chrome));
            match invalid {
                "destroyed" => {
                    DestroyWindow(child).unwrap();
                    fixture.0.retain(|hwnd| *hwnd != child);
                }
                "reparented" => {
                    SetParent(child, Some(other)).unwrap();
                }
                _ => {}
            }
            SendMessageW(
                owner,
                WM_ACTIVATE,
                Some(WPARAM(WA_ACTIVE as usize)),
                Some(LPARAM(0)),
            );
            FOCUS.with(|focus| {
                let focus = focus.borrow();
                let focus = focus.as_ref().unwrap();
                if invalid.is_empty() {
                    assert_eq!(focus.current, child);
                    assert_eq!(focus.restored, vec![child]);
                } else {
                    assert_eq!(focus.current, chrome);
                    assert!(focus.restored.is_empty());
                }
            });
        }
    }
}
