//! Whether anyone can be at the screen: the machine awake, the session
//! unlocked and in front, the display on. Time stops otherwise. Every source
//! is an OS notification; nothing here polls.
#![cfg_attr(all(unix, not(target_os = "macos")), allow(dead_code))]

use std::sync::atomic::{AtomicU8, Ordering};

use tauri::Manager;
use zephium_app::{Command, Handle};

const ASLEEP: u8 = 1;
const DISPLAY_OFF: u8 = 2;
const LOCKED: u8 = 4;
/// Another user's session is in front (fast user switching).
const SWITCHED_OUT: u8 = 8;

static AWAY: AtomicU8 = AtomicU8::new(0);

fn mark(app: &tauri::AppHandle, reason: u8, away: bool) {
    let before = if away {
        AWAY.fetch_or(reason, Ordering::AcqRel)
    } else {
        AWAY.fetch_and(!reason, Ordering::AcqRel)
    };
    let after = if away {
        before | reason
    } else {
        before & !reason
    };
    if (before == 0) != (after == 0) {
        if let Some(shell) = app.try_state::<Handle>() {
            let _ = shell.dispatch(Command::SetSystemAwake(after == 0));
        }
    }
}

/// Time counts only while a Zephium window is the one in use.
pub fn report_app_active(app: &tauri::AppHandle) {
    let focused = |label: &str| {
        app.get_webview_window(label)
            .is_some_and(|window| window.is_focused().unwrap_or(false))
    };
    let active =
        crate::browser_window::focused(app).is_some() || focused(crate::overlay::PANEL_LABEL);
    if let Some(shell) = app.try_state::<Handle>() {
        let _ = shell.dispatch(Command::SetAppActive(active));
    }
}

#[cfg(target_os = "macos")]
pub fn install(app: &tauri::AppHandle, _window: &tauri::WebviewWindow) {
    use block2::RcBlock;
    use objc2_app_kit::{
        NSWorkspace, NSWorkspaceDidWakeNotification, NSWorkspaceScreensDidSleepNotification,
        NSWorkspaceScreensDidWakeNotification, NSWorkspaceSessionDidBecomeActiveNotification,
        NSWorkspaceSessionDidResignActiveNotification, NSWorkspaceWillSleepNotification,
    };
    use objc2_foundation::{NSDistributedNotificationCenter, NSNotification, NSString};
    use std::ptr::NonNull;

    if objc2::MainThreadMarker::new().is_none() {
        return;
    }
    let observe = |center: &objc2_foundation::NSNotificationCenter,
                   name: &objc2_foundation::NSString,
                   reason: u8,
                   away: bool| {
        let app = app.clone();
        let block = RcBlock::new(move |_: NonNull<NSNotification>| mark(&app, reason, away));
        // SAFETY: the block captures only a Send app handle and runs on the
        // posting thread; `mark` touches an atomic and the shell's queue.
        let token = unsafe {
            center.addObserverForName_object_queue_usingBlock(Some(name), None, None, &block)
        };
        // Observers live as long as the process does.
        std::mem::forget(token);
    };
    let workspace = NSWorkspace::sharedWorkspace().notificationCenter();
    // SAFETY: AppKit's notification names are immutable statics.
    unsafe {
        observe(&workspace, NSWorkspaceWillSleepNotification, ASLEEP, true);
        observe(&workspace, NSWorkspaceDidWakeNotification, ASLEEP, false);
        observe(
            &workspace,
            NSWorkspaceScreensDidSleepNotification,
            DISPLAY_OFF,
            true,
        );
        observe(
            &workspace,
            NSWorkspaceScreensDidWakeNotification,
            DISPLAY_OFF,
            false,
        );
        observe(
            &workspace,
            NSWorkspaceSessionDidResignActiveNotification,
            SWITCHED_OUT,
            true,
        );
        observe(
            &workspace,
            NSWorkspaceSessionDidBecomeActiveNotification,
            SWITCHED_OUT,
            false,
        );
    }
    // The lock screen has no public AppKit notification; loginwindow posts
    // these distributed ones, which every lock-aware app relies on.
    let distributed = NSDistributedNotificationCenter::defaultCenter();
    observe(
        &distributed,
        &NSString::from_str("com.apple.screenIsLocked"),
        LOCKED,
        true,
    );
    observe(
        &distributed,
        &NSString::from_str("com.apple.screenIsUnlocked"),
        LOCKED,
        false,
    );
}

#[cfg(target_os = "windows")]
mod windows_presence {
    use std::sync::OnceLock;

    use windows::Win32::Foundation::{HANDLE, HWND, LPARAM, LRESULT, WPARAM};
    use windows::Win32::System::Power::{RegisterPowerSettingNotification, POWERBROADCAST_SETTING};
    use windows::Win32::System::RemoteDesktop::{
        WTSRegisterSessionNotification, NOTIFY_FOR_THIS_SESSION,
    };
    use windows::Win32::System::SystemServices::GUID_CONSOLE_DISPLAY_STATE;
    use windows::Win32::UI::Shell::{DefSubclassProc, SetWindowSubclass};
    use windows::Win32::UI::WindowsAndMessaging::{
        DEVICE_NOTIFY_WINDOW_HANDLE, PBT_APMRESUMEAUTOMATIC, PBT_APMRESUMESUSPEND, PBT_APMSUSPEND,
        PBT_POWERSETTINGCHANGE, WM_POWERBROADCAST, WM_WTSSESSION_CHANGE, WTS_CONSOLE_CONNECT,
        WTS_CONSOLE_DISCONNECT, WTS_SESSION_LOCK, WTS_SESSION_UNLOCK,
    };

    use super::{mark, ASLEEP, DISPLAY_OFF, LOCKED, SWITCHED_OUT};

    const SUBCLASS_ID: usize = 0x5a65_7469;
    static APP: OnceLock<tauri::AppHandle> = OnceLock::new();

    fn away(reason: u8, away: bool) {
        if let Some(app) = APP.get() {
            mark(app, reason, away);
        }
    }

    unsafe extern "system" fn subclass(
        hwnd: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
        _id: usize,
        _data: usize,
    ) -> LRESULT {
        match message {
            WM_POWERBROADCAST => match u32::try_from(wparam.0).unwrap_or(0) {
                PBT_APMSUSPEND => away(ASLEEP, true),
                PBT_APMRESUMEAUTOMATIC | PBT_APMRESUMESUSPEND => away(ASLEEP, false),
                PBT_POWERSETTINGCHANGE if lparam.0 != 0 => {
                    // SAFETY: for PBT_POWERSETTINGCHANGE the OS passes a live
                    // POWERBROADCAST_SETTING whose Data holds DataLength bytes.
                    let setting = unsafe { &*(lparam.0 as *const POWERBROADCAST_SETTING) };
                    if setting.PowerSetting == GUID_CONSOLE_DISPLAY_STATE && setting.DataLength >= 4
                    {
                        // SAFETY: DataLength was checked to cover a u32.
                        let state = unsafe {
                            std::ptr::read_unaligned(setting.Data.as_ptr().cast::<u32>())
                        };
                        // 0 is off, 1 on, 2 dimmed; a dimmed display is still in use.
                        away(DISPLAY_OFF, state == 0);
                    }
                }
                _ => {}
            },
            WM_WTSSESSION_CHANGE => match u32::try_from(wparam.0).unwrap_or(0) {
                WTS_SESSION_LOCK => away(LOCKED, true),
                WTS_SESSION_UNLOCK => away(LOCKED, false),
                WTS_CONSOLE_DISCONNECT => away(SWITCHED_OUT, true),
                WTS_CONSOLE_CONNECT => away(SWITCHED_OUT, false),
                _ => {}
            },
            _ => {}
        }
        // SAFETY: forwards the original message to the next window procedure.
        unsafe { DefSubclassProc(hwnd, message, wparam, lparam) }
    }

    pub fn install(app: &tauri::AppHandle, window: &tauri::WebviewWindow) {
        let Ok(hwnd) = window.hwnd() else {
            return;
        };
        let hwnd = HWND(hwnd.0);
        if APP.set(app.clone()).is_err() {
            return;
        }
        // SAFETY: the main window outlives the process's interest in these
        // notifications; the subclass procedure only reads OS-owned data.
        unsafe {
            let _ = SetWindowSubclass(hwnd, Some(subclass), SUBCLASS_ID, 0);
            let _ = WTSRegisterSessionNotification(hwnd, NOTIFY_FOR_THIS_SESSION);
            let _ = RegisterPowerSettingNotification(
                HANDLE(hwnd.0),
                &GUID_CONSOLE_DISPLAY_STATE,
                DEVICE_NOTIFY_WINDOW_HANDLE,
            );
        }
    }
}

#[cfg(target_os = "windows")]
pub use windows_presence::install;

/// Linux is outside the release; window focus alone governs time there.
#[cfg(all(unix, not(target_os = "macos")))]
pub fn install(_app: &tauri::AppHandle, _window: &tauri::WebviewWindow) {}
