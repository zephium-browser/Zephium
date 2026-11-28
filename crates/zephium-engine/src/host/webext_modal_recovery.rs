//! A disabled owner is checked once. A live native modal takes precedence.
fn should_recover(
    enabled: bool,
    visible_owned: bool,
    active_popup: bool,
    hidden_chromium: bool,
) -> bool {
    !enabled && !visible_owned && !active_popup && hidden_chromium
}

#[cfg(target_os = "windows")]
mod native {
    use super::should_recover;
    use crate::platform::imp::BrowserProcess;
    use std::cell::Cell;
    use std::time::{Duration, Instant};
    use windows::core::BOOL;
    use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
    use windows::Win32::System::Threading::GetCurrentProcessId;
    use windows::Win32::UI::Input::KeyboardAndMouse::{EnableWindow, IsWindowEnabled};
    use windows::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
    use windows::Win32::UI::WindowsAndMessaging::*;

    const ID: usize = 0x279_5745;
    const GRACE_MS: u32 = 750;
    const WIDGET_CLASS: &str = "Chrome_WidgetWin_0";

    thread_local! {
        static DISABLED: Cell<Option<(HWND, Instant)>> = const { Cell::new(None) };
    }

    pub(in crate::host) fn install(handle: raw_window_handle::RawWindowHandle) {
        let raw_window_handle::RawWindowHandle::Win32(handle) = handle else {
            return;
        };
        let hwnd = HWND(handle.hwnd.get() as *mut _);
        // The desktop installs the engine on the owning UI thread.
        unsafe {
            if IsWindow(Some(hwnd)).as_bool()
                && !SetWindowSubclass(hwnd, Some(window_proc), ID, 0).as_bool()
            {
                eprintln!("extensions: Windows modal recovery could not be installed");
            }
        }
    }

    unsafe extern "system" fn window_proc(
        hwnd: HWND,
        message: u32,
        wp: WPARAM,
        lp: LPARAM,
        id: usize,
        _: usize,
    ) -> LRESULT {
        match message {
            WM_ENABLE => unsafe {
                let _ = KillTimer(Some(hwnd), ID);
                let deadline = Instant::now() + Duration::from_millis(GRACE_MS.into());
                DISABLED.set(
                    (wp.0 == 0 && SetTimer(Some(hwnd), ID, GRACE_MS, None) != 0)
                        .then_some((hwnd, deadline)),
                );
            },
            WM_TIMER if wp.0 == ID => {
                let Some(disabled) = DISABLED
                    .get()
                    .filter(|(window, at)| *window == hwnd && *at <= Instant::now())
                else {
                    return LRESULT(0);
                };
                unsafe {
                    let _ = KillTimer(Some(hwnd), ID);
                }
                let _ = std::panic::catch_unwind(|| {
                    let _ = crate::host::dispatch::try_with(move |host| {
                        if DISABLED.get() != Some(disabled) {
                            return;
                        }
                        DISABLED.set(None);
                        let processes: Vec<_> = host
                            .browser_processes
                            .iter()
                            .filter(|(profile, _)| {
                                host.windows_extensions
                                    .has_native_views_for_profile(**profile)
                            })
                            .map(|(_, process)| process.clone())
                            .collect();
                        recover(hwnd, &processes);
                    });
                });
                return LRESULT(0);
            }
            WM_NCDESTROY => unsafe {
                let _ = KillTimer(Some(hwnd), ID);
                DISABLED.set(None);
                let _ = RemoveWindowSubclass(hwnd, Some(window_proc), id);
            },
            _ => {}
        }
        unsafe { DefSubclassProc(hwnd, message, wp, lp) }
    }

    struct Windows<'a> {
        main: HWND,
        processes: &'a [BrowserProcess],
        visible_owned: bool,
        hidden: Vec<HWND>,
    }

    fn is_hidden_browser_frame(window: HWND, processes: &[BrowserProcess]) -> bool {
        unsafe {
            if processes.is_empty()
                || IsWindowVisible(window).as_bool()
                || GetWindow(window, GW_OWNER).is_ok()
            {
                return false;
            }
            let mut class = [0; 32];
            let length = GetClassNameW(window, &mut class);
            if length <= 0
                || !class[..length as usize]
                    .iter()
                    .copied()
                    .eq(WIDGET_CLASS.encode_utf16())
            {
                return false;
            }
            let mut pid = 0;
            GetWindowThreadProcessId(window, Some(&mut pid)) != 0
                && processes
                    .iter()
                    .any(|process| process.id() == pid && process.is_running())
        }
    }

    unsafe extern "system" fn collect(window: HWND, context: LPARAM) -> BOOL {
        // EnumWindows calls synchronously with the stack context in recover.
        let windows = unsafe { &mut *(context.0 as *mut Windows<'_>) };
        unsafe {
            if window != windows.main
                && IsWindowVisible(window).as_bool()
                && GetAncestor(window, GA_ROOTOWNER) == windows.main
            {
                windows.visible_owned = true;
            } else if windows.hidden.len() < 32
                && is_hidden_browser_frame(window, windows.processes)
            {
                windows.hidden.push(window);
            }
        }
        BOOL(1)
    }

    fn snapshot(main: HWND, processes: &[BrowserProcess]) -> Option<Windows<'_>> {
        let mut windows = Windows {
            main,
            processes,
            visible_owned: false,
            hidden: Vec::new(),
        };
        unsafe {
            EnumWindows(Some(collect), LPARAM((&raw mut windows) as isize)).ok()?;
        }
        Some(windows)
    }

    fn active_popup(main: HWND, processes: &[BrowserProcess]) -> bool {
        unsafe {
            let popup = GetLastActivePopup(main);
            if popup != main && IsWindowVisible(popup).as_bool() {
                return true;
            }
            // A browser-owned dialog can be top-level without a Win32 owner.
            // Treat a visible foreground window in the app or browser
            // process as a dialog even if it was absent from the owner chain.
            let foreground = GetForegroundWindow();
            let mut pid = 0;
            foreground != main
                && IsWindowVisible(foreground).as_bool()
                && GetWindowThreadProcessId(foreground, Some(&mut pid)) != 0
                && (pid == GetCurrentProcessId()
                    || processes
                        .iter()
                        .any(|process| process.id() == pid && process.is_running()))
        }
    }

    fn recover(main: HWND, processes: &[BrowserProcess]) {
        unsafe {
            if !IsWindow(Some(main)).as_bool()
                || IsWindowEnabled(main).as_bool()
                || processes.is_empty()
            {
                return;
            }
            let Some(windows) = snapshot(main, processes) else {
                return;
            };
            if !should_recover(
                IsWindowEnabled(main).as_bool(),
                windows.visible_owned,
                active_popup(main, processes),
                !windows.hidden.is_empty(),
            ) {
                return;
            }
            let mut closed = 0;
            for frame in windows.hidden {
                if is_hidden_browser_frame(frame, processes)
                    && PostMessageW(Some(frame), WM_CLOSE, WPARAM(0), LPARAM(0)).is_ok()
                {
                    closed += 1;
                }
            }
            // Browsers can present a real dialog while their windows are being
            // scanned. Recheck immediately before enabling its possible owner.
            let reenable = snapshot(main, &[]).is_some_and(|windows| !windows.visible_owned)
                && !active_popup(main, processes)
                && !IsWindowEnabled(main).as_bool();
            if reenable {
                let _ = EnableWindow(main, true);
            }
            if closed > 0 || reenable {
                eprintln!(
                    "extensions: recovered hidden Windows WebView2 modal ({closed} close requests)"
                );
            }
        }
    }
}

#[cfg(target_os = "windows")]
pub(super) use native::install;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_disabled_owner_with_only_hidden_chromium_frames_can_be_recovered() {
        assert!(should_recover(false, false, false, true));
    }

    #[test]
    fn a_visible_owned_dialog_or_active_popup_is_preserved() {
        assert!(!should_recover(false, true, false, true));
        assert!(!should_recover(false, false, true, true));
    }

    #[test]
    fn an_enabled_owner_or_a_disable_without_a_chromium_frame_is_ignored() {
        assert!(!should_recover(true, false, false, true));
        assert!(!should_recover(false, false, false, false));
    }
}
