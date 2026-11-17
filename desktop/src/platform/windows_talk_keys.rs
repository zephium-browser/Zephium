//! The talk key on Windows. Presses and the claimed keys arrive through the
//! web views' accelerator hook or the main window's own key messages, where
//! they can be consumed. WebView2 never reports a Shift release or a plain
//! key's release, so key-ups also come from raw input, which reaches the app
//! only while one of its windows is in front.

use std::cell::RefCell;

use tauri::{AppHandle, Manager, WebviewWindow};
use windows::core::BOOL;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, GetKeyState, VK_CONTROL, VK_ESCAPE, VK_LCONTROL, VK_LMENU, VK_LSHIFT,
    VK_LWIN, VK_MENU, VK_RCONTROL, VK_RETURN, VK_RMENU, VK_RSHIFT, VK_RWIN, VK_SHIFT,
};
use windows::Win32::UI::Input::{
    GetRawInputData, HRAWINPUT, RAWINPUT, RAWINPUTHEADER, RID_INPUT, RIM_TYPEKEYBOARD,
};
use windows::Win32::UI::Shell::{DefSubclassProc, SetWindowSubclass};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumThreadWindows, GetClassNameW, GetForegroundWindow, WA_INACTIVE, WM_ACTIVATE, WM_INPUT,
    WM_KEYDOWN, WM_KEYUP, WM_SYSKEYDOWN, WM_SYSKEYUP,
};

use crate::keymap::Keymap;
use crate::talk_key::{Capture, Claimed, KeyEvent, Modifiers, TalkKey, TalkSignal};

const SUBCLASS: usize = 0x7a_6c_6b;
const RAW_TARGET: &str = "Tao Thread Event Target";

#[derive(Clone, Copy, PartialEq, Eq)]
enum Source {
    View,
    Window,
    Raw,
}

struct Talk {
    app: AppHandle,
    main: isize,
    capture: Capture,
    /// A held key whose release came through raw input first; its key
    /// message, if one follows, is still the page's to miss.
    released: Option<u16>,
}

thread_local! {
    static TALK: RefCell<Option<Talk>> = const { RefCell::new(None) };
}

pub(crate) fn install_talk_keys(window: &WebviewWindow) {
    let Ok(main) = window.hwnd() else {
        return;
    };
    TALK.with(|slot| {
        *slot.borrow_mut() = Some(Talk {
            app: window.app_handle().clone(),
            main: main.0 as isize,
            capture: Capture::default(),
            released: None,
        });
    });
    // SAFETY: both windows belong to this thread, which runs their procedures
    // for as long as they live.
    unsafe {
        let _ = SetWindowSubclass(main, Some(main_proc), SUBCLASS, 0);
        if let Some(target) = raw_input_target() {
            let _ = SetWindowSubclass(target, Some(raw_proc), SUBCLASS, 0);
        }
    }
    zephium_engine::set_key_filter(|key| {
        let (scan, extended, repeat) = lparam_facts(key.lparam as isize, key.up);
        offer(Source::View, key.key as u16, scan, extended, repeat, key.up)
    });
}

fn raw_input_target() -> Option<HWND> {
    unsafe extern "system" fn visit(window: HWND, found: LPARAM) -> BOOL {
        let mut class = [0u16; 64];
        // SAFETY: the buffer outlives the call.
        let length = unsafe { GetClassNameW(window, &mut class) } as usize;
        if String::from_utf16_lossy(&class[..length]) == RAW_TARGET {
            // SAFETY: `found` points at the caller's slot, which outlives the walk.
            unsafe { *(found.0 as *mut isize) = window.0 as isize };
            return false.into();
        }
        true.into()
    }
    let mut found = 0isize;
    // SAFETY: walks this thread's windows into `found`.
    unsafe {
        let _ = EnumThreadWindows(
            GetCurrentThreadId(),
            Some(visit),
            LPARAM(&mut found as *mut isize as isize),
        );
    }
    (found != 0).then_some(HWND(found as *mut _))
}

unsafe extern "system" fn main_proc(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _: usize,
    _: usize,
) -> LRESULT {
    match message {
        WM_KEYDOWN | WM_SYSKEYDOWN | WM_KEYUP | WM_SYSKEYUP => {
            let up = message == WM_KEYUP || message == WM_SYSKEYUP;
            let (scan, extended, repeat) = lparam_facts(lparam.0, up);
            if offer(Source::Window, wparam.0 as u16, scan, extended, repeat, up) {
                return LRESULT(0);
            }
        }
        WM_ACTIVATE if (wparam.0 & 0xffff) as u32 == WA_INACTIVE => resign(),
        _ => {}
    }
    // SAFETY: passes the message on unchanged.
    unsafe { DefSubclassProc(window, message, wparam, lparam) }
}

unsafe extern "system" fn raw_proc(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _: usize,
    _: usize,
) -> LRESULT {
    if message == WM_INPUT {
        if let Some((key, scan, extended, true)) = raw_key(lparam) {
            offer(Source::Raw, key, scan, extended, false, true);
        }
    }
    // SAFETY: passes the message on unchanged; the record stays readable.
    unsafe { DefSubclassProc(window, message, wparam, lparam) }
}

fn raw_key(lparam: LPARAM) -> Option<(u16, u16, bool, bool)> {
    const BREAK: u16 = 1;
    const E0: u16 = 2;
    let mut raw = RAWINPUT::default();
    let mut size = std::mem::size_of::<RAWINPUT>() as u32;
    // SAFETY: reads this message's record into a buffer of the stated size.
    let read = unsafe {
        GetRawInputData(
            HRAWINPUT(lparam.0 as *mut _),
            RID_INPUT,
            Some((&mut raw as *mut RAWINPUT).cast()),
            &mut size,
            std::mem::size_of::<RAWINPUTHEADER>() as u32,
        )
    };
    if read == u32::MAX || raw.header.dwType != RIM_TYPEKEYBOARD.0 {
        return None;
    }
    // SAFETY: the header says this record is a keyboard one.
    let keyboard = unsafe { raw.data.keyboard };
    (keyboard.VKey != 0xff).then_some((
        keyboard.VKey,
        keyboard.MakeCode,
        keyboard.Flags & E0 != 0,
        keyboard.Flags & BREAK != 0,
    ))
}

/// Scan code, extended flag and repeat from a key message's lParam.
fn lparam_facts(lparam: isize, up: bool) -> (u16, bool, bool) {
    (
        ((lparam >> 16) & 0xff) as u16,
        lparam & (1 << 24) != 0,
        !up && lparam & (1 << 30) != 0,
    )
}

fn offer(source: Source, key: u16, scan: u16, extended: bool, repeat: bool, up: bool) -> bool {
    let decided = TALK.with(|slot| {
        let mut slot = slot.try_borrow_mut().ok()?;
        let talk = slot.as_mut()?;
        Some(talk.key(source, sided(key, scan, extended), repeat, up))
    });
    let Some((consume, signal, app)) = decided else {
        return false;
    };
    deliver(&app, signal);
    consume
}

fn resign() {
    let decided = TALK.with(|slot| {
        let mut slot = slot.try_borrow_mut().ok()?;
        let talk = slot.as_mut()?;
        talk.released = None;
        Some((talk.capture.resign().signal, talk.app.clone()))
    });
    if let Some((signal, app)) = decided {
        deliver(&app, signal);
    }
}

impl Talk {
    fn key(
        &mut self,
        source: Source,
        code: u16,
        repeat: bool,
        up: bool,
    ) -> (bool, Option<TalkSignal>, AppHandle) {
        let app = self.app.clone();
        // SAFETY: reads the foreground window handle only.
        if unsafe { GetForegroundWindow() }.0 as isize != self.main {
            self.released = None;
            return (false, self.capture.resign().signal, app);
        }
        if source != Source::Raw && self.released == Some(code) {
            self.released = None;
            if up {
                return (true, None, app);
            }
        }
        let held = |key: u16| {
            // SAFETY: reads key state only.
            let state = unsafe {
                if source == Source::Raw {
                    GetAsyncKeyState(i32::from(key))
                } else {
                    GetKeyState(i32::from(key))
                }
            };
            state as u16 & 0x8000 != 0
        };
        let (kind, modifiers, repeat) = translate(code, up, repeat, held);
        let keymap = app.state::<Keymap>();
        if keymap.recording() {
            self.released = None;
            self.capture.resign();
            return (false, None, app.clone());
        }
        let talk = matches!(kind, KeyEvent::Down)
            && !modifiers.meta
            && keymap.talks(
                u32::from(code),
                modifiers.ctrl,
                modifiers.shift,
                modifiers.alt,
            );
        let claimed = match code {
            c if c == VK_ESCAPE.0 => Some(Claimed::Escape),
            c if c == VK_RETURN.0 => Some(Claimed::Enter),
            _ => None,
        }
        .filter(|key| {
            source != Source::Raw
                && keymap.offers("voice.talk")
                && app.state::<TalkKey>().claimed(*key)
        });
        let action = self
            .capture
            .event(kind, code, modifiers, repeat, talk, claimed);
        if source == Source::Raw && action.consume {
            self.released = Some(code);
        }
        (
            action.consume && source != Source::Raw,
            action.signal,
            app.clone(),
        )
    }
}

/// Tells the left and right modifier keys apart, as the capture tracks them.
fn sided(key: u16, scan: u16, extended: bool) -> u16 {
    match key {
        k if k == VK_SHIFT.0 && scan == 0x36 => VK_RSHIFT.0,
        k if k == VK_SHIFT.0 => VK_LSHIFT.0,
        k if k == VK_CONTROL.0 && extended => VK_RCONTROL.0,
        k if k == VK_CONTROL.0 => VK_LCONTROL.0,
        k if k == VK_MENU.0 && extended => VK_RMENU.0,
        k if k == VK_MENU.0 => VK_LMENU.0,
        k => k,
    }
}

/// The event as the shared capture reads it. A modifier key is a change of
/// modifiers; the event's own key counts as already moved.
fn translate(
    code: u16,
    up: bool,
    was_down: bool,
    held: impl Fn(u16) -> bool,
) -> (KeyEvent, Modifiers, bool) {
    let code = sided(code, 0, false);
    let now = |key: u16| if key == code { !up } else { held(key) };
    let either = |left: u16, right: u16| now(left) || now(right);
    let modifiers = Modifiers {
        meta: either(VK_LWIN.0, VK_RWIN.0),
        ctrl: either(VK_LCONTROL.0, VK_RCONTROL.0),
        alt: either(VK_LMENU.0, VK_RMENU.0),
        shift: either(VK_LSHIFT.0, VK_RSHIFT.0),
        other: false,
    };
    let modifier = [
        VK_LSHIFT,
        VK_RSHIFT,
        VK_LCONTROL,
        VK_RCONTROL,
        VK_LMENU,
        VK_RMENU,
        VK_LWIN,
        VK_RWIN,
    ]
    .iter()
    .any(|key| key.0 == code);
    let kind = match (modifier, up) {
        (true, _) => KeyEvent::FlagsChanged,
        (false, true) => KeyEvent::Up,
        (false, false) => KeyEvent::Down,
    };
    (kind, modifiers, !up && !modifier && was_down)
}

fn deliver(app: &AppHandle, signal: Option<TalkSignal>) {
    let Some(signal) = signal else {
        return;
    };
    let talk = app.state::<TalkKey>();
    match signal {
        TalkSignal::Down => talk.press(),
        TalkSignal::Up => talk.release(),
        TalkSignal::Cancel => talk.take(Claimed::Escape),
        TalkSignal::Confirm => talk.take(Claimed::Enter),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::talk_key::KeyAction;

    const A: u16 = 0x41;

    fn none(_: u16) -> bool {
        false
    }

    #[test]
    fn modifiers_count_the_event_key_as_moved() {
        let chord = |key: u16| key == VK_LCONTROL.0 || key == VK_RSHIFT.0;
        let (kind, modifiers, repeat) = translate(A, false, false, chord);
        assert!(matches!(kind, KeyEvent::Down) && !repeat);
        assert!(modifiers.ctrl && modifiers.shift && !modifiers.alt && !modifiers.meta);
        let (kind, modifiers, _) = translate(VK_RSHIFT.0, true, true, chord);
        assert!(matches!(kind, KeyEvent::FlagsChanged));
        assert!(modifiers.ctrl && !modifiers.shift);
        let (kind, modifiers, repeat) = translate(VK_SHIFT.0, false, true, none);
        assert!(matches!(kind, KeyEvent::FlagsChanged) && !repeat && modifiers.shift);
        let (kind, _, repeat) = translate(A, true, true, chord);
        assert!(matches!(kind, KeyEvent::Up) && !repeat);
        assert!(translate(A, false, true, chord).2);
    }

    #[test]
    fn sides_and_repeats_come_from_the_message() {
        assert_eq!(sided(VK_SHIFT.0, 0x36, false), VK_RSHIFT.0);
        assert_eq!(sided(VK_SHIFT.0, 0x2a, false), VK_LSHIFT.0);
        assert_eq!(sided(VK_CONTROL.0, 0x1d, true), VK_RCONTROL.0);
        assert_eq!(sided(VK_MENU.0, 0x38, false), VK_LMENU.0);
        assert_eq!(sided(A, 0x1e, false), A);
        let held_down = 0x1e << 16 | 1 << 30 | 1;
        assert_eq!(lparam_facts(held_down, false), (0x1e, false, true));
        assert_eq!(lparam_facts(held_down, true), (0x1e, false, false));
        assert_eq!(
            lparam_facts(0x1d << 16 | 1 << 24, false),
            (0x1d, true, false)
        );
    }

    #[test]
    fn a_chord_released_modifier_first_keeps_consuming_the_held_key() {
        let mut capture = Capture::default();
        let mut keys = vec![VK_LCONTROL.0, VK_LSHIFT.0];
        let send = |capture: &mut Capture, code: u16, up: bool, keys: &mut Vec<u16>| {
            let was_down = keys.contains(&code);
            let (kind, modifiers, repeat) =
                translate(code, up, was_down, |key| keys.contains(&key));
            if up {
                keys.retain(|key| *key != code);
            } else if !was_down {
                keys.push(code);
            }
            capture.event(kind, code, modifiers, repeat, !up && code == A, None)
        };
        assert_eq!(
            send(&mut capture, A, false, &mut keys),
            KeyAction {
                consume: true,
                signal: Some(TalkSignal::Down)
            }
        );
        assert_eq!(
            send(&mut capture, A, false, &mut keys),
            KeyAction {
                consume: true,
                signal: None
            }
        );
        assert_eq!(
            send(&mut capture, VK_LCONTROL.0, true, &mut keys),
            KeyAction {
                consume: false,
                signal: Some(TalkSignal::Up)
            }
        );
        assert!(send(&mut capture, A, false, &mut keys).consume);
        assert_eq!(
            send(&mut capture, A, true, &mut keys),
            KeyAction {
                consume: true,
                signal: None
            }
        );
        assert_eq!(
            send(&mut capture, VK_LSHIFT.0, true, &mut keys),
            KeyAction::default()
        );
    }
}
