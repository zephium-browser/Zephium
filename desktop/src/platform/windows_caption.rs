//! An owned native popup stays above child WebViews without resizing content.
//! A near-transparent edge target reveals a compact native group; timers run only during transitions.
use tauri::WebviewWindow;
use windows::core::w;
use windows::Win32::{
    Foundation::*,
    Graphics::Gdi::*,
    UI::{
        Controls::*,
        HiDpi::{GetDpiForWindow, GetSystemMetricsForDpi},
        Input::KeyboardAndMouse::*,
        Shell::{DefSubclassProc, GetWindowSubclass, RemoveWindowSubclass, SetWindowSubclass},
        WindowsAndMessaging::*,
    },
};
const ID: usize = 0x5a434150;
const HIDE: usize = ID + 1;
const FADE: usize = ID + 2;
// A page filling the screen owns every pixel, the top edge included.
static SUPPRESSED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
struct State {
    owner: HWND,
    strip: HWND,
    edge: HWND,
    buttons: [HWND; 3],
    expanded: bool,
    window: WebviewWindow,
    previous_focus: HWND,
    laying_out: bool,
    hot_button: HWND,
    sizing: bool,
    changing_state: bool,
    opacity: u8,
    fade_from: u8,
    fade_started: std::time::Instant,
}

pub fn install(window: &WebviewWindow) -> bool {
    let Ok(owner) = window.hwnd() else {
        return false;
    };
    // SAFETY: creation and subclass installation run on Tauri's UI thread.
    unsafe {
        let owner = HWND(owner.0);
        let Ok(strip) = CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_LAYERED,
            w!("STATIC"),
            w!(""),
            WS_POPUP | WS_CLIPCHILDREN | WINDOW_STYLE(0x0100), /* SS_NOTIFY */
            0,
            0,
            0,
            0,
            Some(owner),
            None,
            None,
            None,
        ) else {
            return false;
        };
        // Alpha zero is excluded from layered-window hit testing. One alpha unit
        // retains pointer events without a visible border or painted caption rail.
        let Ok(edge) = CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_LAYERED,
            w!("STATIC"),
            w!(""),
            WS_POPUP | WINDOW_STYLE(0x0100),
            0,
            0,
            0,
            0,
            Some(owner),
            None,
            None,
            None,
        ) else {
            let _ = DestroyWindow(strip);
            return false;
        };
        let _ = SetLayeredWindowAttributes(edge, COLORREF(0), 1, LWA_ALPHA);
        let _ = SetLayeredWindowAttributes(strip, COLORREF(0), 0, LWA_ALPHA);
        let mut buttons = [HWND::default(); 3];
        for (index, title) in [w!("Minimize"), w!("Maximize or restore"), w!("Close")]
            .into_iter()
            .enumerate()
        {
            let Ok(button) = CreateWindowExW(
                WS_EX_NOPARENTNOTIFY,
                w!("BUTTON"),
                title,
                WS_CHILD | WS_TABSTOP | WINDOW_STYLE(BS_OWNERDRAW as u32),
                0,
                0,
                0,
                0,
                Some(strip),
                Some(HMENU((index + 1) as *mut _)),
                None,
                None,
            ) else {
                let _ = DestroyWindow(edge);
                let _ = DestroyWindow(strip);
                return false;
            };
            buttons[index] = button;
        }
        let pointer = Box::into_raw(Box::new(State {
            owner,
            strip,
            edge,
            buttons,
            expanded: false,
            window: window.clone(),
            previous_focus: HWND::default(),
            laying_out: false,
            hot_button: HWND::default(),
            sizing: false,
            changing_state: false,
            opacity: 0,
            fade_from: 0,
            fade_started: std::time::Instant::now(),
        }));
        let mut installed =
            SetWindowSubclass(owner, Some(owner_proc), ID, pointer as usize).as_bool();
        installed &= SetWindowSubclass(strip, Some(strip_proc), ID, pointer as usize).as_bool();
        installed &= SetWindowSubclass(edge, Some(edge_proc), ID, pointer as usize).as_bool();
        for button in buttons {
            installed &=
                SetWindowSubclass(button, Some(button_proc), ID, pointer as usize).as_bool();
        }
        if !installed {
            let _ = RemoveWindowSubclass(owner, Some(owner_proc), ID);
            let _ = DestroyWindow(edge);
            let _ = DestroyWindow(strip);
            drop(Box::from_raw(pointer));
            return false;
        }
        layout(pointer);
        true
    }
}
/// Hides the caption and its reveal edge while page content fills the
/// screen. Runs on the window's UI thread.
pub fn set_suppressed(window: &WebviewWindow, suppressed: bool) {
    SUPPRESSED.store(suppressed, std::sync::atomic::Ordering::Relaxed);
    let Ok(owner) = window.hwnd() else {
        return;
    };
    let mut data = 0usize;
    // SAFETY: UI-thread lookup of this module's own subclass; its data is the
    // live State pointer until WM_NCDESTROY removes the subclass.
    unsafe {
        if GetWindowSubclass(
            HWND(owner.0),
            Some(owner_proc),
            ID,
            Some(&mut data as *mut usize),
        )
        .as_bool()
            && data != 0
        {
            layout(data as *mut State);
        }
    }
}
unsafe fn layout(p: *mut State) {
    if (*p).laying_out {
        return;
    }
    (*p).laying_out = true;
    let owner = (*p).owner;
    let scale = GetDpiForWindow(owner).max(96) as f64 / 96.0;
    let button = (28.0 * scale).round() as i32;
    let padding = (5.0 * scale).round() as i32;
    let gap = (4.0 * scale).round() as i32;
    let width = button * 3 + padding * 2 + gap * 2;
    let height = button + padding * 2;
    let mut rect = RECT::default();
    let _ = GetClientRect(owner, &mut rect);
    let mut origin = POINT::default();
    let _ = ClientToScreen(owner, &mut origin);
    let foreground = GetForegroundWindow();
    let show = !(*p).changing_state
        && !SUPPRESSED.load(std::sync::atomic::Ordering::Relaxed)
        && !(*p).sizing
        && IsWindowVisible(owner).as_bool()
        && !IsIconic(owner).as_bool()
        && (foreground == owner || foreground == (*p).strip);
    if !show {
        (*p).expanded = false;
        (*p).opacity = 0;
        let _ = KillTimer(Some((*p).strip), FADE);
        let _ = KillTimer(Some((*p).strip), HIDE);
    }
    let _ = SetWindowPos(
        (*p).strip,
        None,
        origin.x + rect.right - width,
        origin.y,
        width,
        height,
        SWP_NOACTIVATE | SWP_NOZORDER | SWP_NOOWNERZORDER,
    );
    let radius = (24.0 * scale).round() as i32;
    let region = CreateRoundRectRgn(0, 0, width + 1, height + 1, radius, radius);
    if SetWindowRgn((*p).strip, Some(region), true) == 0 {
        let _ = DeleteObject(region.into());
    }
    let border = resize_border(owner);
    let _ = SetWindowPos(
        (*p).edge,
        None,
        origin.x,
        origin.y,
        rect.right.max(1),
        border + (6.0 * scale).ceil() as i32,
        SWP_NOACTIVATE | SWP_NOZORDER | SWP_NOOWNERZORDER,
    );
    for (index, hwnd) in (*p).buttons.into_iter().enumerate() {
        let _ = SetWindowPos(
            hwnd,
            None,
            padding + index as i32 * (button + gap),
            padding,
            button,
            button,
            SWP_NOZORDER | SWP_NOACTIVATE,
        );
        let _ = ShowWindow(hwnd, SW_SHOWNA);
    }
    let _ = SetLayeredWindowAttributes((*p).strip, COLORREF(0), (*p).opacity, LWA_ALPHA);
    let _ = ShowWindow(
        (*p).strip,
        if show && ((*p).expanded || (*p).opacity > 0) {
            SW_SHOWNA
        } else {
            SW_HIDE
        },
    );
    let _ = ShowWindow((*p).edge, if show { SW_SHOWNA } else { SW_HIDE });
    let _ = InvalidateRect(Some((*p).strip), None, false);
    (*p).laying_out = false;
}
// The top target forwards native resize gestures: the child WebView otherwise
// consumes this frameless client border before the owner sees it.
unsafe fn resize_border(owner: HWND) -> i32 {
    if IsZoomed(owner).as_bool()
        || GetWindowLongPtrW(owner, GWL_STYLE) as u32 & WS_THICKFRAME.0 == 0
    {
        return 0;
    }
    let dpi = GetDpiForWindow(owner).max(96);
    // Only the visible outer rim belongs to resize hit testing. The full
    // padded sizing frame extends into the caption and the page gutter.
    (2 * GetSystemMetricsForDpi(SM_CYBORDER, dpi)).max(1)
}
fn top_frame_hit(x: i32, y: i32, width: i32, border: i32, corner: i32) -> Option<u32> {
    if border <= 0 || x < 0 || x >= width || y < 0 || y >= border {
        return None;
    }
    Some(if x < corner {
        HTTOPLEFT
    } else if x >= width - corner {
        HTTOPRIGHT
    } else {
        HTTOP
    })
}
fn resize_direction(hit: u32) -> Option<u32> {
    match hit {
        HTTOP => Some(WMSZ_TOP),
        HTTOPLEFT => Some(WMSZ_TOPLEFT),
        HTTOPRIGHT => Some(WMSZ_TOPRIGHT),
        HTRIGHT => Some(WMSZ_RIGHT),
        _ => None,
    }
}
unsafe fn resize_hit(p: *mut State, l: LPARAM) -> Option<u32> {
    let mut point = POINT {
        x: l.0 as i16 as i32,
        y: (l.0 >> 16) as i16 as i32,
    };
    let _ = ScreenToClient((*p).owner, &mut point);
    let mut rect = RECT::default();
    let _ = GetClientRect((*p).owner, &mut rect);
    let border = resize_border((*p).owner);
    let dpi = GetDpiForWindow((*p).owner).max(96);
    let corner = 2
        * (GetSystemMetricsForDpi(SM_CYSIZEFRAME, dpi)
            + GetSystemMetricsForDpi(SM_CXPADDEDBORDER, dpi));
    top_frame_hit(point.x, point.y, rect.right, border, corner).or_else(|| {
        // Preserve the right resize gutter even while the caption is revealed.
        (border > 0
            && point.x >= rect.right - border
            && point.x < rect.right
            && point.y >= border
            && point.y < rect.bottom - border)
            .then_some(HTRIGHT)
    })
}
unsafe fn prepare_state_change(p: *mut State) {
    // Remove both owned popups before focus repaint and the system animation.
    (*p).changing_state = true;
    (*p).expanded = false;
    (*p).opacity = 0;
    (*p).hot_button = HWND::default();
    (*p).fade_from = 0;
    let _ = KillTimer(Some((*p).strip), FADE);
    let _ = KillTimer(Some((*p).strip), HIDE);
    let _ = SetLayeredWindowAttributes((*p).strip, COLORREF(0), 0, LWA_ALPHA);
    let _ = ShowWindow((*p).strip, SW_HIDE);
    let _ = ShowWindow((*p).edge, SW_HIDE);
}
unsafe fn transition(p: *mut State, expanded: bool) {
    if (*p).expanded == expanded {
        return;
    }
    (*p).expanded = expanded;
    let mut enabled = windows::core::BOOL(1);
    let _ = SystemParametersInfoW(
        SPI_GETCLIENTAREAANIMATION,
        0,
        Some((&mut enabled as *mut windows::core::BOOL).cast()),
        SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
    );
    if !enabled.as_bool() || super::menus::high_contrast() {
        (*p).opacity = if expanded { 255 } else { 0 };
        let _ = KillTimer(Some((*p).strip), FADE);
    } else {
        (*p).fade_from = (*p).opacity;
        (*p).fade_started = std::time::Instant::now();
        SetTimer(Some((*p).strip), FADE, 15, None);
    }
    layout(p);
}
unsafe fn reveal(p: *mut State) {
    if (*p).changing_state || (*p).sizing {
        return;
    }
    let _ = KillTimer(Some((*p).strip), HIDE);
    transition(p, true);
}
unsafe extern "system" fn edge_proc(
    hwnd: HWND,
    msg: u32,
    w: WPARAM,
    l: LPARAM,
    _: usize,
    data: usize,
) -> LRESULT {
    let p = data as *mut State;
    match msg {
        WM_NCHITTEST => {
            if let Some(hit) = resize_hit(p, l) {
                return LRESULT(hit as isize);
            }
        }
        WM_NCLBUTTONDOWN if resize_direction(w.0 as u32).is_some() => {
            let _ = ReleaseCapture();
            let _ = PostMessageW(
                Some((*p).owner),
                WM_SYSCOMMAND,
                WPARAM((SC_SIZE | resize_direction(w.0 as u32).unwrap()) as usize),
                l,
            );
            return LRESULT(0);
        }
        WM_SETCURSOR if l.0 as u16 == HTCLIENT as u16 => {
            if let Ok(cursor) = LoadCursorW(None, IDC_ARROW) {
                SetCursor(Some(cursor));
            }
            return LRESULT(1);
        }
        WM_MOUSEACTIVATE => return LRESULT(MA_NOACTIVATE as isize),
        WM_MOUSEMOVE => {
            reveal(p);
            track(hwnd, false);
        }
        WM_MOUSELEAVE => {
            SetTimer(Some((*p).strip), HIDE, 450, None);
        }
        // Keep the edge useful as a native drag handle.
        WM_LBUTTONDOWN => {
            let _ = ReleaseCapture();
            let _ = PostMessageW(
                Some((*p).owner),
                WM_SYSCOMMAND,
                WPARAM((SC_MOVE | HTCAPTION) as usize),
                LPARAM(0),
            );
            return LRESULT(0);
        }
        WM_ERASEBKGND => return LRESULT(1),
        WM_PAINT => {
            let mut paint = PAINTSTRUCT::default();
            let dc = BeginPaint(hwnd, &mut paint);
            FillRect(dc, &paint.rcPaint, HBRUSH(GetStockObject(BLACK_BRUSH).0));
            let _ = EndPaint(hwnd, &paint);
            return LRESULT(0);
        }
        WM_NCDESTROY => {
            let _ = RemoveWindowSubclass(hwnd, Some(edge_proc), ID);
        }
        _ => {}
    }
    DefSubclassProc(hwnd, msg, w, l)
}
unsafe fn track(hwnd: HWND, nonclient: bool) {
    let mut tracking = TRACKMOUSEEVENT {
        cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
        dwFlags: TME_LEAVE
            | if nonclient {
                TME_NONCLIENT
            } else {
                TRACKMOUSEEVENT_FLAGS(0)
            },
        hwndTrack: hwnd,
        ..Default::default()
    };
    let _ = TrackMouseEvent(&mut tracking);
}
unsafe extern "system" fn owner_proc(
    hwnd: HWND,
    msg: u32,
    w: WPARAM,
    l: LPARAM,
    _: usize,
    data: usize,
) -> LRESULT {
    let p = data as *mut State;
    if msg == WM_NCDESTROY {
        let _ = RemoveWindowSubclass(hwnd, Some(owner_proc), ID);
        // Owned popups are explicitly torn down before freeing subclass state.
        if IsWindow(Some((*p).strip)).as_bool() {
            let _ = DestroyWindow((*p).strip);
        }
        if IsWindow(Some((*p).edge)).as_bool() {
            let _ = DestroyWindow((*p).edge);
        }
        drop(Box::from_raw(p));
        return DefSubclassProc(hwnd, msg, w, l);
    }
    if msg == WM_SYSKEYDOWN && w.0 == VK_F10.0 as usize {
        reveal(p);
        let _ = SetFocus(Some((*p).buttons[0]));
        return LRESULT(0);
    }
    let state_command = msg == WM_SYSCOMMAND
        && matches!(w.0 as u32 & 0xfff0, SC_MINIMIZE | SC_MAXIMIZE | SC_RESTORE);
    if state_command {
        prepare_state_change(p);
    }
    if msg == WM_ENTERSIZEMOVE {
        (*p).sizing = true;
        layout(p);
    } else if msg == WM_EXITSIZEMOVE {
        (*p).sizing = false;
        layout(p);
    }
    if msg == WM_NCHITTEST {
        if let Some(hit) = resize_hit(p, l) {
            return LRESULT(hit as isize);
        }
    }
    if msg == WM_NCHITTEST && (*p).expanded {
        let point = POINT {
            x: l.0 as i16 as i32,
            y: (l.0 >> 16) as i16 as i32,
        };
        let mut rect = RECT::default();
        let _ = GetWindowRect((*p).buttons[1], &mut rect);
        if PtInRect(&rect, point).as_bool() {
            return LRESULT(HTMAXBUTTON as isize);
        }
    }
    let result = DefSubclassProc(hwnd, msg, w, l);
    // Keep the owned caption hidden through all synchronous size/focus messages.
    // WM_SIZE alone is too early: Windows may still be animating the owner.
    if state_command {
        (*p).changing_state = false;
        layout(p);
    }
    if matches!(
        msg,
        WM_SIZE
            | WM_DPICHANGED
            | WM_WINDOWPOSCHANGED
            | WM_PARENTNOTIFY
            | WM_ACTIVATE
            | WM_SHOWWINDOW
    ) {
        layout(p);
    }
    if matches!(msg, WM_THEMECHANGED | WM_SETTINGCHANGE) {
        let _ = InvalidateRect(Some((*p).strip), None, false);
        for button in (*p).buttons {
            let _ = InvalidateRect(Some(button), None, true);
        }
    }
    result
}
unsafe extern "system" fn strip_proc(
    hwnd: HWND,
    msg: u32,
    w: WPARAM,
    l: LPARAM,
    _: usize,
    data: usize,
) -> LRESULT {
    let p = data as *mut State;
    match msg {
        WM_NCHITTEST => {
            if let Some(hit) = resize_hit(p, l) {
                return LRESULT(hit as isize);
            }
        }
        WM_NCLBUTTONDOWN if resize_direction(w.0 as u32).is_some() => {
            let _ = ReleaseCapture();
            let _ = PostMessageW(
                Some((*p).owner),
                WM_SYSCOMMAND,
                WPARAM((SC_SIZE | resize_direction(w.0 as u32).unwrap()) as usize),
                l,
            );
            return LRESULT(0);
        }
        WM_SETCURSOR if l.0 as u16 == HTCLIENT as u16 => {
            if let Ok(cursor) = LoadCursorW(None, IDC_ARROW) {
                SetCursor(Some(cursor));
            }
            return LRESULT(1);
        }
        WM_MOUSEACTIVATE => return LRESULT(MA_NOACTIVATE as isize),
        WM_CLOSE => {
            let _ = PostMessageW(
                Some((*p).owner),
                WM_SYSCOMMAND,
                WPARAM(SC_CLOSE as usize),
                LPARAM(0),
            );
            return LRESULT(0);
        }
        WM_MOUSEMOVE => {
            reveal(p);
            track(hwnd, false);
        }
        WM_MOUSELEAVE => {
            SetTimer(Some(hwnd), HIDE, 450, None);
        }
        WM_TIMER if w.0 == FADE => {
            let t = ((*p).fade_started.elapsed().as_secs_f64() / 0.16).min(1.0);
            let eased = 1.0 - (1.0 - t).powi(3);
            let target = if (*p).expanded { 255.0 } else { 0.0 };
            (*p).opacity = (f64::from((*p).fade_from)
                + (target - f64::from((*p).fade_from)) * eased)
                .round() as u8;
            let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), (*p).opacity, LWA_ALPHA);
            if t >= 1.0 {
                let _ = KillTimer(Some(hwnd), FADE);
                if !(*p).expanded {
                    let _ = ShowWindow(hwnd, SW_HIDE);
                }
            }
            return LRESULT(0);
        }
        WM_TIMER if w.0 == HIDE => {
            let _ = KillTimer(Some(hwnd), HIDE);
            let mut cursor = POINT::default();
            let _ = GetCursorPos(&mut cursor);
            let mut rect = RECT::default();
            let _ = GetWindowRect(hwnd, &mut rect);
            let focus = GetFocus();
            let mut edge = RECT::default();
            let _ = GetWindowRect((*p).edge, &mut edge);
            edge.top += resize_border((*p).owner);
            // Moving along the reveal strip must not dismiss the controls.
            if !PtInRect(&rect, cursor).as_bool()
                && !PtInRect(&edge, cursor).as_bool()
                && !(*p).buttons.contains(&focus)
            {
                transition(p, false);
            }
        }
        WM_COMMAND if (w.0 >> 16) == BN_CLICKED as usize => {
            let target = if IsWindow(Some((*p).previous_focus)).as_bool() {
                (*p).previous_focus
            } else {
                (*p).owner
            };
            let command = match w.0 & 0xffff {
                1 => SC_MINIMIZE,
                2 => {
                    if IsZoomed((*p).owner).as_bool() {
                        SC_RESTORE
                    } else {
                        SC_MAXIMIZE
                    }
                }
                3 => SC_CLOSE,
                _ => return DefSubclassProc(hwnd, msg, w, l),
            };
            if matches!(command, SC_MINIMIZE | SC_MAXIMIZE | SC_RESTORE) {
                prepare_state_change(p);
            }
            let _ = SetFocus(Some(target));
            let _ = PostMessageW(
                Some((*p).owner),
                WM_SYSCOMMAND,
                WPARAM(command as usize),
                LPARAM(0),
            );
            return LRESULT(0);
        }
        WM_DRAWITEM if l.0 != 0 => {
            paint(p, &*(l.0 as *const DRAWITEMSTRUCT));
            return LRESULT(1);
        }
        WM_ERASEBKGND => return LRESULT(1),
        WM_PAINT => {
            let mut paint = PAINTSTRUCT::default();
            let dc = BeginPaint(hwnd, &mut paint);
            let mut rect = RECT::default();
            let _ = GetClientRect(hwnd, &mut rect);
            let dark = !matches!((*p).window.theme(), Ok(tauri::Theme::Light));
            let brush = CreateSolidBrush(if super::menus::high_contrast() {
                COLORREF(GetSysColor(COLOR_WINDOW))
            } else if dark {
                COLORREF(0x272727)
            } else {
                COLORREF(0xf8f8f8)
            });
            FillRect(dc, &rect, brush);
            let _ = EndPaint(hwnd, &paint);
            let _ = DeleteObject(brush.into());
            return LRESULT(1);
        }
        WM_NCDESTROY => {
            let _ = KillTimer(Some(hwnd), HIDE);
            let _ = KillTimer(Some(hwnd), FADE);
            let _ = RemoveWindowSubclass(hwnd, Some(strip_proc), ID);
        }
        _ => {}
    }
    DefSubclassProc(hwnd, msg, w, l)
}
unsafe extern "system" fn button_proc(
    hwnd: HWND,
    msg: u32,
    w: WPARAM,
    l: LPARAM,
    _: usize,
    data: usize,
) -> LRESULT {
    let p = data as *mut State;
    match msg {
        WM_NCHITTEST => {
            if let Some(hit) = resize_hit(p, l) {
                return LRESULT(hit as isize);
            }
        }
        WM_NCLBUTTONDOWN if resize_direction(w.0 as u32).is_some() => {
            let _ = ReleaseCapture();
            let _ = PostMessageW(
                Some((*p).owner),
                WM_SYSCOMMAND,
                WPARAM((SC_SIZE | resize_direction(w.0 as u32).unwrap()) as usize),
                l,
            );
            return LRESULT(0);
        }
        WM_SETCURSOR if l.0 as u16 == HTCLIENT as u16 => {
            if let Ok(cursor) = LoadCursorW(None, IDC_ARROW) {
                SetCursor(Some(cursor));
            }
            return LRESULT(1);
        }
        WM_SETFOCUS => {
            let previous = HWND(w.0 as *mut _);
            if !(*p).buttons.contains(&previous) {
                (*p).previous_focus = previous;
            }
        }
        WM_MOUSEMOVE => {
            reveal(p);
            track(hwnd, false);
            if (*p).hot_button != hwnd {
                (*p).hot_button = hwnd;
                let _ = InvalidateRect(Some(hwnd), None, false);
            }
        }
        WM_MOUSELEAVE | WM_KILLFOCUS => {
            if (*p).hot_button == hwnd {
                (*p).hot_button = HWND::default();
            }
            SetTimer(Some((*p).strip), HIDE, 450, None);
            let _ = InvalidateRect(Some(hwnd), None, false);
        }
        WM_KEYDOWN if w.0 == VK_ESCAPE.0 as usize => {
            let target = if IsWindow(Some((*p).previous_focus)).as_bool() {
                (*p).previous_focus
            } else {
                (*p).owner
            };
            let _ = SetFocus(Some(target));
            transition(p, false);
            return LRESULT(0);
        }
        WM_KEYDOWN if w.0 == VK_LEFT.0 as usize || w.0 == VK_RIGHT.0 as usize => {
            let index = (*p).buttons.iter().position(|b| *b == hwnd).unwrap_or(0);
            let next = (index + if w.0 == VK_LEFT.0 as usize { 2 } else { 1 }) % 3;
            let _ = SetFocus(Some((*p).buttons[next]));
            return LRESULT(0);
        }
        WM_NCDESTROY => {
            let _ = RemoveWindowSubclass(hwnd, Some(button_proc), ID);
        }
        _ => {}
    }
    DefSubclassProc(hwnd, msg, w, l)
}
unsafe fn paint(p: *mut State, draw: &DRAWITEMSTRUCT) {
    let dark = !matches!((*p).window.theme(), Ok(tauri::Theme::Light));
    let mut cursor = POINT::default();
    let _ = GetCursorPos(&mut cursor);
    let _ = ScreenToClient(draw.hwndItem, &mut cursor);
    let hot = PtInRect(&draw.rcItem, cursor).as_bool();
    let close = draw.CtlID == 3;
    let pressed = draw.itemState.0 & ODS_SELECTED.0 != 0;
    let contrast = super::menus::high_contrast();
    let bg = if contrast {
        COLORREF(GetSysColor(if hot {
            COLOR_HIGHLIGHT
        } else {
            COLOR_WINDOW
        }))
    } else if hot && close {
        COLORREF(if pressed { 0x201095 } else { 0x2311c4 })
    } else if hot {
        if dark {
            COLORREF(if pressed { 0x343434 } else { 0x424242 })
        } else {
            COLORREF(if pressed { 0xd5d5d5 } else { 0xe5e5e5 })
        }
    } else if dark {
        COLORREF(0x272727)
    } else {
        COLORREF(0xf8f8f8)
    };
    let ink = if contrast {
        COLORREF(GetSysColor(if hot {
            COLOR_HIGHLIGHTTEXT
        } else {
            COLOR_WINDOWTEXT
        }))
    } else if dark || (hot && close) {
        COLORREF(0xf4f4f4)
    } else {
        COLORREF(0x202020)
    };
    let base = if contrast {
        COLORREF(GetSysColor(COLOR_WINDOW))
    } else if dark {
        COLORREF(0x272727)
    } else {
        COLORREF(0xf8f8f8)
    };
    let brush = CreateSolidBrush(base);
    FillRect(draw.hDC, &draw.rcItem, brush);
    let _ = DeleteObject(brush.into());
    let brush = CreateSolidBrush(bg);
    let old_brush = SelectObject(draw.hDC, brush.into());
    let old_pen = SelectObject(draw.hDC, GetStockObject(NULL_PEN));
    let radius = (16.0 * GetDpiForWindow((*p).owner).max(96) as f64 / 96.0).round() as i32;
    let _ = RoundRect(
        draw.hDC,
        draw.rcItem.left,
        draw.rcItem.top,
        draw.rcItem.right,
        draw.rcItem.bottom,
        radius,
        radius,
    );
    SelectObject(draw.hDC, old_pen);
    SelectObject(draw.hDC, old_brush);
    let _ = DeleteObject(brush.into());
    let scale = GetDpiForWindow((*p).owner).max(96) as f64 / 96.0;
    let half = (4.0 * scale).round() as i32;
    let x = (draw.rcItem.right + draw.rcItem.left) / 2;
    let y = (draw.rcItem.bottom + draw.rcItem.top) / 2;
    let pen = CreatePen(PS_SOLID, (scale.round() as i32).max(1), ink);
    let old = SelectObject(draw.hDC, pen.into());
    let line = |a, b, c, d| {
        let _ = MoveToEx(draw.hDC, a, b, None);
        let _ = LineTo(draw.hDC, c, d);
    };
    match draw.CtlID {
        1 => line(x - half, y, x + half + 1, y),
        2 => {
            if IsZoomed((*p).owner).as_bool() {
                line(x - half + 2, y - half - 2, x + half + 2, y - half - 2);
                line(x + half + 2, y - half - 2, x + half + 2, y + half - 2);
            }
            line(x - half, y - half, x + half, y - half);
            line(x + half, y - half, x + half, y + half);
            line(x + half, y + half, x - half, y + half);
            line(x - half, y + half, x - half, y - half);
        }
        3 => {
            line(x - half, y - half, x + half + 1, y + half + 1);
            line(x + half, y - half, x - half - 1, y + half + 1);
        }
        _ => {}
    }
    SelectObject(draw.hDC, old);
    let _ = DeleteObject(pen.into());
    if draw.itemState.0 & ODS_FOCUS.0 != 0 {
        let mut rect = draw.rcItem;
        rect.left += 4;
        rect.right -= 4;
        rect.top += 4;
        rect.bottom -= 4;
        let _ = DrawFocusRect(draw.hDC, &rect);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn top_resize_band_precedes_reveal_and_preserves_corners() {
        for border in [2, 3, 4] {
            assert_eq!(top_frame_hit(200, 0, 800, border, 16), Some(HTTOP));
            assert_eq!(top_frame_hit(200, border - 1, 800, border, 16), Some(HTTOP));
            assert_eq!(top_frame_hit(200, border, 800, border, 16), None);
            assert_eq!(top_frame_hit(1, 1, 800, border, 16), Some(HTTOPLEFT));
            assert_eq!(top_frame_hit(799, 1, 800, border, 16), Some(HTTOPRIGHT));
        }
        // The top of a caption button (5 DIP inset) is never a resize target.
        assert_eq!(top_frame_hit(760, 5, 800, 2, 16), None);
        assert_eq!(top_frame_hit(790, 1, 800, 2, 16), Some(HTTOPRIGHT));
        assert_eq!(top_frame_hit(200, 0, 800, 0, 16), None);
        assert_eq!(top_frame_hit(-1, 0, 800, 8, 16), None);
        assert_eq!(top_frame_hit(800, 0, 800, 8, 16), None);
    }
}
