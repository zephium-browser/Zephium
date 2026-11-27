//! Owned, non-resizable extension popover. Coordinates at this boundary are physical.
use raw_window_handle::{
    HandleError, HasWindowHandle, RawWindowHandle, Win32WindowHandle, WindowHandle,
};
use std::{cell::Cell, num::NonZeroIsize};
use windows::core::w;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::{
    DwmSetWindowAttribute, DWMWA_BORDER_COLOR, DWMWA_USE_IMMERSIVE_DARK_MODE,
    DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND,
};
use windows::Win32::Graphics::Gdi::{
    ClientToScreen, GetMonitorInfoW, GetStockObject, MonitorFromPoint, BLACK_BRUSH, HBRUSH,
    MONITORINFO, MONITOR_DEFAULTTONEAREST,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows::Win32::UI::WindowsAndMessaging::*;
use zephium_core::geometry::Rect;

pub(super) const BACKGROUND: (u8, u8, u8, u8) = (26, 26, 29, 255);
pub(super) struct PopupWindow(pub(super) HWND, HWND, RECT, Cell<bool>, Cell<bool>);

impl PopupWindow {
    pub(super) fn new(parent: HWND, anchor: Rect) -> Result<Self, String> {
        // SAFETY: the browser owns parent on this thread; callbacks are static.
        unsafe {
            let module = GetModuleHandleW(None).map_err(|e| e.to_string())?;
            let class = WNDCLASSW {
                lpfnWndProc: Some(window_proc),
                hInstance: module.into(),
                hbrBackground: HBRUSH(GetStockObject(BLACK_BRUSH).0),
                lpszClassName: w!("ZephiumExtensionPopup"),
                ..Default::default()
            };
            RegisterClassW(&class);
            let scale = f64::from(GetDpiForWindow(parent).max(96)) / 96.0;
            let mut top = POINT {
                x: (anchor.x * scale).round() as i32,
                y: (anchor.y * scale).round() as i32,
            };
            if !ClientToScreen(parent, &mut top).as_bool() {
                return Err("Cannot anchor the extension popup.".into());
            }
            let anchor = RECT {
                left: top.x,
                top: top.y,
                right: top.x + (anchor.width * scale).round() as i32,
                bottom: top.y + (anchor.height * scale).round() as i32,
            };
            // A one-pixel non-client rim preserves DWM's system shadow/rounding.
            // WM_NCCALCSIZE removes the resize frame; WM_NCHITTEST disables resizing.
            let hwnd = CreateWindowExW(
                WS_EX_TOOLWINDOW,
                class.lpszClassName,
                w!("Extension — Zephium"),
                WS_POPUP | WS_THICKFRAME,
                top.x,
                top.y,
                402,
                27,
                Some(parent),
                None,
                Some(module.into()),
                None,
            )
            .map_err(|e| e.to_string())?;
            let result = Self(hwnd, parent, anchor, Cell::new(false), Cell::new(false));
            let corner = DWMWCP_ROUND;
            let dark = 1i32;
            let border = 0x003b_3838u32; // COLORREF, matching the dark neutral rim.
            let _ = DwmSetWindowAttribute(
                hwnd,
                DWMWA_WINDOW_CORNER_PREFERENCE,
                std::ptr::from_ref(&corner).cast(),
                std::mem::size_of_val(&corner) as u32,
            );
            let _ = DwmSetWindowAttribute(
                hwnd,
                DWMWA_USE_IMMERSIVE_DARK_MODE,
                (&dark as *const i32).cast(),
                std::mem::size_of_val(&dark) as u32,
            );
            let _ = DwmSetWindowAttribute(
                hwnd,
                DWMWA_BORDER_COLOR,
                (&border as *const u32).cast(),
                std::mem::size_of_val(&border) as u32,
            );
            if !SetWindowSubclass(parent, Some(owner_proc), hwnd.0 as usize, 0).as_bool() {
                return Err("Cannot observe the popup owner.".into());
            }
            // A failed document must not retain an invisible popup indefinitely.
            if SetTimer(Some(hwnd), 1, 15000, None) == 0 {
                return Err("Cannot bound popup startup.".into());
            }
            result.resize(400.0, 600.0)?;
            Ok(result)
        }
    }

    pub(super) fn resize(&self, width: f64, height: f64) -> Result<(i32, i32), String> {
        if !width.is_finite() || !height.is_finite() {
            return Err("Invalid popup size.".into());
        }
        // SAFETY: both handles remain owned on the creating thread.
        unsafe {
            let scale = f64::from(GetDpiForWindow(self.0).max(96)) / 96.0;
            let width = (width.clamp(25.0, 800.0) * scale).ceil() as i32;
            let height = (height.clamp(25.0, 600.0) * scale).ceil() as i32;
            let mut monitor = MONITORINFO {
                cbSize: std::mem::size_of::<MONITORINFO>() as u32,
                ..Default::default()
            };
            if !GetMonitorInfoW(
                MonitorFromPoint(
                    POINT {
                        x: self.2.left,
                        y: self.2.top,
                    },
                    MONITOR_DEFAULTTONEAREST,
                ),
                &mut monitor,
            )
            .as_bool()
            {
                return Err("Cannot find the popup work area.".into());
            }
            let frame = placement(
                self.2,
                monitor.rcWork,
                width + 2,
                height + 2,
                (6.0 * scale).round() as i32,
            );
            SetWindowPos(
                self.0,
                None,
                frame.left,
                frame.top,
                frame.right - frame.left,
                frame.bottom - frame.top,
                SWP_NOZORDER | SWP_NOACTIVATE,
            )
            .map_err(|e| e.to_string())?;
            Ok((frame.right - frame.left - 2, frame.bottom - frame.top - 2))
        }
    }

    pub(super) fn show(&self) -> bool {
        if self.3.replace(true) {
            return false;
        }
        // SAFETY: show only after content has supplied its first bounded size.
        unsafe {
            let _ = KillTimer(Some(self.0), 1);
            let _ = ShowWindow(self.0, SW_SHOW);
            let _ = SetForegroundWindow(self.0);
        }
        true
    }

    pub(super) fn owner_active(&self) -> bool {
        // A slow-loading hidden popup must not activate after the user left.
        unsafe {
            let foreground = GetForegroundWindow();
            foreground == self.0 || foreground == self.1
        }
    }

    pub(super) fn permission_pending(&self) -> bool {
        self.4.get()
    }

    pub(super) fn hide_for_permission(&self) {
        if !self.3.get() {
            return;
        }
        self.4.set(true);
        // Hide the native popover so the prompt can take focus while its
        // extension document remains alive to receive the decision.
        unsafe {
            self.restore_owner_focus();
            let _ = ShowWindow(self.0, SW_HIDE);
        }
    }

    pub(super) fn resume_after_permission(&self) -> bool {
        if !self.4.replace(false) || !self.owner_active() {
            return false;
        }
        unsafe {
            let _ = ShowWindow(self.0, SW_SHOW);
            let _ = SetForegroundWindow(self.0);
        }
        true
    }

    pub(super) fn cursor_on_anchor(&self) -> bool {
        let mut point = POINT::default();
        // SAFETY: OS writes the supplied point; the anchor is an owned value.
        unsafe {
            GetCursorPos(&mut point).is_ok()
                && point.x >= self.2.left
                && point.x < self.2.right
                && point.y >= self.2.top
                && point.y < self.2.bottom
        }
    }

    pub(super) fn restore_owner_focus(&self) -> bool {
        // Never steal focus back from an outside click or another application.
        unsafe {
            if GetForegroundWindow() != self.0 {
                return false;
            }
            let _ = SetForegroundWindow(self.1);
            true
        }
    }
}

pub(super) struct EscapeRegistration {
    controller: webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2Controller,
    token: i64,
}
impl EscapeRegistration {
    pub(super) fn new(
        view: &wry::WebView,
        hwnd: HWND,
        alive: std::rc::Rc<Cell<bool>>,
    ) -> windows::core::Result<Self> {
        use webview2_com::Microsoft::Web::WebView2::Win32::COREWEBVIEW2_KEY_EVENT_KIND_KEY_DOWN;
        use wry::WebViewExtWindows;
        let handler =
            webview2_com::AcceleratorKeyPressedEventHandler::create(Box::new(move |_, args| {
                let Some(args) = args.filter(|_| alive.get()) else {
                    return Ok(());
                };
                let mut kind = COREWEBVIEW2_KEY_EVENT_KIND_KEY_DOWN;
                let mut key = 0;
                // SAFETY: native event arguments are live for this callback.
                unsafe {
                    args.KeyEventKind(&mut kind)?;
                    args.VirtualKey(&mut key)?;
                    if key == 27 && kind == COREWEBVIEW2_KEY_EVENT_KIND_KEY_DOWN {
                        args.SetHandled(true)?;
                        // Close outside the synchronous WebView2 accelerator callback.
                        PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0))?;
                    }
                }
                Ok(())
            }));
        let controller = view.controller();
        let mut token = 0;
        // SAFETY: registration is owned and removed before controller teardown.
        unsafe {
            controller.add_AcceleratorKeyPressed(&handler, &mut token)?;
        }
        Ok(Self { controller, token })
    }
}
impl Drop for EscapeRegistration {
    fn drop(&mut self) {
        // SAFETY: token came from this controller and is removed only once.
        unsafe {
            let _ = self.controller.remove_AcceleratorKeyPressed(self.token);
        }
    }
}

fn placement(anchor: RECT, work: RECT, width: i32, height: i32, gap: i32) -> RECT {
    let width = width.min((work.right - work.left).max(1));
    let height = height.min((work.bottom - work.top).max(1));
    let right_aligned = work.right - anchor.right < anchor.left - work.left;
    let x = if right_aligned {
        anchor.right - width
    } else {
        anchor.left
    };
    let below = anchor.bottom + gap;
    let y = if below + height <= work.bottom {
        below
    } else {
        anchor.top - gap - height
    };
    let left = x.clamp(work.left, work.right - width);
    let top = y.clamp(work.top, work.bottom - height);
    RECT {
        left,
        top,
        right: left + width,
        bottom: top + height,
    }
}

impl HasWindowHandle for PopupWindow {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        let hwnd = NonZeroIsize::new(self.0 .0 as isize).ok_or(HandleError::Unavailable)?;
        // SAFETY: the borrowed handle cannot outlive this owner.
        Ok(unsafe {
            WindowHandle::borrow_raw(RawWindowHandle::Win32(Win32WindowHandle::new(hwnd)))
        })
    }
}
impl Drop for PopupWindow {
    fn drop(&mut self) {
        // SAFETY: unregister before freeing the popup; controller teardown precedes Drop.
        unsafe {
            let _ = RemoveWindowSubclass(self.1, Some(owner_proc), self.0 .0 as usize);
            let _ = DestroyWindow(self.0);
        }
    }
}

pub(super) fn close(hwnd: HWND, deactivated: bool) {
    if !super::super::dispatch::try_with(move |host| {
        if let Some(popup) = host
            .windows_extensions
            .popup
            .as_ref()
            .filter(|popup| popup.window.0 == hwnd)
        {
            if deactivated && popup.window.permission_pending() {
                return;
            }
            if deactivated && popup.window.cursor_on_anchor() {
                host.windows_extensions.dismissed_action =
                    Some((popup.runtime, popup.tab, std::time::Instant::now()));
            }
            host.close_windows_extension_popup();
        }
    }) {
        // Admission is bounded. If teardown cannot enter the host now, hide
        // immediately; the tracked owner/resource still retires on the next
        // toolbar toggle, profile retirement or shutdown. Never leave a stale
        // popup interactive or destroy its HWND behind the controller owner.
        unsafe {
            let _ = ShowWindow(hwnd, SW_HIDE);
        }
    }
}

unsafe extern "system" fn window_proc(hwnd: HWND, message: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match message {
        WM_NCCALCSIZE if wp.0 != 0 => {
            // SAFETY: Windows supplies NCCALCSIZE_PARAMS for this message form.
            let rect = unsafe { &mut (*(lp.0 as *mut NCCALCSIZE_PARAMS)).rgrc[0] };
            rect.left += 1;
            rect.top += 1;
            rect.right -= 1;
            rect.bottom -= 1;
            return LRESULT(0);
        }
        WM_NCHITTEST => return LRESULT(HTCLIENT as isize),
        WM_CLOSE => {
            close(hwnd, false);
            return LRESULT(0);
        }
        WM_TIMER if wp.0 == 1 => {
            // SAFETY: this is our single startup watchdog on the owned HWND.
            unsafe {
                let _ = KillTimer(Some(hwnd), 1);
            }
            close(hwnd, false);
            return LRESULT(0);
        }
        WM_ACTIVATE if wp.0 & 0xffff == WA_INACTIVE as usize => close(hwnd, true),
        _ => {}
    }
    // SAFETY: forward the unchanged native callback tuple.
    unsafe { DefWindowProcW(hwnd, message, wp, lp) }
}

unsafe extern "system" fn owner_proc(
    hwnd: HWND,
    message: u32,
    wp: WPARAM,
    lp: LPARAM,
    id: usize,
    _data: usize,
) -> LRESULT {
    if matches!(
        message,
        WM_MOVE | WM_SIZE | WM_ENTERSIZEMOVE | WM_DPICHANGED | WM_NCDESTROY
    ) {
        close(HWND(id as *mut _), false);
    }
    if message == WM_NCDESTROY {
        // SAFETY: this exact registration belongs to the owner being destroyed.
        unsafe {
            let _ = RemoveWindowSubclass(hwnd, Some(owner_proc), id);
        }
    }
    // SAFETY: preserve the browser and other subclasses' event handling.
    unsafe { DefSubclassProc(hwnd, message, wp, lp) }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn rect(x: i32, y: i32, w: i32, h: i32) -> RECT {
        RECT {
            left: x,
            top: y,
            right: x + w,
            bottom: y + h,
        }
    }
    #[test]
    fn anchors_to_nearest_edge_and_flips_above_at_both_scales() {
        for scale in [1.0, 1.5] {
            let px = |value: i32| (f64::from(value) * scale).round() as i32;
            let work = rect(0, 0, px(1200), px(800));
            let left = placement(
                rect(px(20), px(20), px(30), px(30)),
                work,
                px(400),
                px(300),
                px(6),
            );
            assert_eq!((left.left, left.top), (px(20), px(56)));
            let right = placement(
                rect(px(1100), px(730), px(30), px(30)),
                work,
                px(400),
                px(300),
                px(6),
            );
            assert_eq!((right.left, right.top), (px(730), px(424)));
        }
    }
    #[test]
    fn clamps_to_negative_coordinate_monitor_and_short_work_area() {
        let result = placement(
            rect(-20, 680, 30, 30),
            rect(-1280, 0, 1280, 720),
            1400,
            900,
            6,
        );
        assert_eq!(
            (result.left, result.top, result.right, result.bottom),
            (-1280, 0, 0, 720)
        );
    }
}
