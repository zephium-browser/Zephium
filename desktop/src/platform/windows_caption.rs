//! A native caption revealed by translating page containers without reflow.
//! A cached alpha surface exposes the owner's backdrop material. Native buttons
//! retain keyboard/accessibility behavior on a separate, nearly transparent
//! input host.
#[path = "windows_caption_paint.rs"]
mod paint;
use tauri::WebviewWindow;
use windows::core::{w, PCWSTR};
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
const CLOSE_SETTLED: usize = ID + 3;
const REVEAL: usize = ID + 4;
const WATCH: usize = ID + 5;
// How often the pointer is read for the top of the window. A page's WebView2
// belongs to another process, so no move over it reaches this thread. With
// another window in front, or this one minimized, there is nothing to read:
// the watch stops and activation starts it again.
const WATCH_MS: u32 = 50;
// How far down from the window's top the pointer asks for the caption over a
// page: the gutter and a little of the page, not a page's own top bar.
const TOP_REACH: f64 = 20.0;
// How long the pointer must stay there: going for a page's own top bar
// passes through without the caption coming out over it.
const TOP_DWELL_MS: u32 = 400;
// The pointer must rest on the reveal edge this long before the caption
// slides in, so passing over it on the way elsewhere reveals nothing.
const REVEAL_DWELL_MS: u32 = 150;
/// Why the caption is out of the way. Each holder clears only its own, so a
/// page leaving fullscreen cannot unhide the caption of a window the person
/// had made fullscreen.
#[derive(Clone, Copy)]
pub enum Suppression {
    /// A page holds the screen.
    Content = 1,
    /// The person made the window itself fullscreen.
    Window = 2,
}
struct State {
    owner: HWND,
    strip: HWND,
    edge: HWND,
    buttons: [HWND; 3],
    expanded: bool,
    suppressed: bool,
    suppressions: u8,
    reveal_armed: bool,
    menu_open: bool,
    // The window state the buttons' accessible names were last set for.
    named: Option<bool>,
    // Where the pointer was when a window state change retired the caption.
    // Hover is ignored until it leaves, or the caption would return at once
    // under a pointer that has not moved.
    reveal_block: Option<POINT>,
    page_left: Option<f64>,
    dark: bool,
    previous_focus: HWND,
    laying_out: bool,
    hot_button: HWND,
    // The last button the pointer was on, which outlives the hover itself.
    last_hot: HWND,
    sizing: bool,
    changing_state: bool,
    opacity: u8,
    fade_from: u8,
    fade_started: std::time::Instant,
    pressed_button: HWND,
    drag_start: Option<POINT>,
    motion_paused: Option<std::time::Instant>,
    surface: Option<paint::Surface>,
    geometry: Option<(i32, i32, i32, i32, i32, u32)>,
    render_failed: bool,
    // The pointer was near the window's top at the last reading.
    near_top: bool,
    // The pointer watch is running.
    watching: bool,
}

pub fn install(window: &WebviewWindow) -> bool {
    let Ok(owner) = window.hwnd() else {
        return false;
    };
    // SAFETY: creation and subclass installation run on Tauri's UI thread.
    unsafe {
        let owner = HWND(owner.0);
        let Ok(strip) = CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_LAYERED | WS_EX_TRANSPARENT,
            w!("STATIC"),
            w!(""),
            WS_POPUP,
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
            WS_POPUP | WS_CLIPCHILDREN | WINDOW_STYLE(0x0100),
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
        let Some(surface) = paint::Surface::new(GetDpiForWindow(owner)) else {
            let _ = DestroyWindow(edge);
            let _ = DestroyWindow(strip);
            return false;
        };
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
                Some(edge),
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
            suppressed: false,
            suppressions: 0,
            reveal_armed: false,
            menu_open: false,
            named: None,
            reveal_block: None,
            page_left: None,
            dark: true,
            previous_focus: HWND::default(),
            laying_out: false,
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
            surface: Some(surface),
            geometry: None,
            render_failed: false,
            near_top: false,
            watching: true,
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
        SetTimer(Some(strip), WATCH, WATCH_MS, None);
        true
    }
}
/// The live state of a window's caption, found through its subclass. Runs on
/// the window's UI thread.
unsafe fn state_of(window: &WebviewWindow) -> Option<*mut State> {
    installed_state(HWND(window.hwnd().ok()?.0))
}
unsafe fn installed_state(owner: HWND) -> Option<*mut State> {
    let mut data = 0usize;
    // SAFETY: UI-thread lookup of this module's own subclass; its data is the
    // live State pointer until WM_NCDESTROY removes the subclass.
    (GetWindowSubclass(owner, Some(owner_proc), ID, Some(&mut data as *mut usize)).as_bool()
        && data != 0)
        .then_some(data as *mut State)
}

/// Hides the caption and its reveal edge while page content fills the
/// screen. Runs on the window's UI thread.
pub fn set_suppressed(window: &WebviewWindow, cause: Suppression, suppressed: bool) {
    // SAFETY: see `state_of`.
    unsafe {
        if let Some(state) = state_of(window) {
            let causes = (*state).suppressions;
            (*state).suppressions = if suppressed {
                causes | cause as u8
            } else {
                causes & !(cause as u8)
            };
            (*state).suppressed = (*state).suppressions != 0;
            layout(state);
        }
    }
}

/// The shell's admitted chrome boundary, in logical coordinates. Internal
/// pages need only the corner controls; web pages get a band over the page.
pub fn set_page_left(window: &WebviewWindow, page_left: Option<f64>) {
    // SAFETY: see `state_of`.
    unsafe {
        if let Some(state) = state_of(window) {
            if (*state).page_left != page_left {
                (*state).page_left = page_left;
                layout(state);
            }
        }
    }
}

/// The appearance the app resolved for the window, which the paint path reads
/// instead of asking the window for its theme in the middle of a message.
pub fn set_dark(window: &WebviewWindow, dark: bool) {
    // SAFETY: see `state_of`.
    unsafe {
        if let Some(state) = state_of(window) {
            if (*state).dark != dark {
                (*state).dark = dark;
                let _ = render(state);
            }
        }
    }
}
/// Minimize and Maximize follow the owner's styles, which onboarding withdraws.
/// A maximized window can always be restored.
fn button_enabled(index: usize, style: u32, zoomed: bool) -> bool {
    match index {
        0 => style & WS_MINIMIZEBOX.0 != 0,
        1 => style & WS_MAXIMIZEBOX.0 != 0 || zoomed,
        _ => true,
    }
}
unsafe fn enabled_buttons(owner: HWND) -> [bool; 3] {
    let style = GetWindowLongPtrW(owner, GWL_STYLE) as u32;
    let zoomed = IsZoomed(owner).as_bool();
    std::array::from_fn(|index| button_enabled(index, style, zoomed))
}
/// A system menu string as an accessible name: without its mnemonic marker
/// (or the "(&N)" hint East Asian languages append) and accelerator text.
fn menu_label(raw: &str) -> String {
    let mut label = String::with_capacity(raw.len());
    let mut chars = raw.split('\t').next().unwrap_or_default().chars();
    while let Some(c) = chars.next() {
        match c {
            '&' => {
                if chars.clone().next() == Some('&') {
                    chars.next();
                    label.push('&');
                }
            }
            '(' => {
                let mut ahead = chars.clone();
                if ahead.next() == Some('&') && ahead.next().is_some() && ahead.next() == Some(')')
                {
                    chars = ahead;
                } else {
                    label.push(c);
                }
            }
            _ => label.push(c),
        }
    }
    label.trim().to_owned()
}
unsafe fn menu_name(menu: HMENU, command: u32) -> Option<String> {
    let mut buffer = [0u16; 128];
    let length = GetMenuStringW(menu, command, Some(&mut buffer), MF_BYCOMMAND);
    let label = menu_label(&String::from_utf16_lossy(
        buffer.get(..usize::try_from(length).ok()?)?,
    ));
    (!label.is_empty()).then_some(label)
}
/// The buttons are named from the system menu, which Windows has already
/// localized, and Maximize becomes Restore with the window.
unsafe fn name_buttons(p: *mut State, zoomed: bool) {
    let menu = GetSystemMenu((*p).owner, false);
    let names = [
        (SC_MINIMIZE, "Minimize"),
        if zoomed {
            (SC_RESTORE, "Restore")
        } else {
            (SC_MAXIMIZE, "Maximize")
        },
        (SC_CLOSE, "Close"),
    ];
    for ((command, fallback), button) in names.into_iter().zip((*p).buttons) {
        let name = if menu.is_invalid() {
            None
        } else {
            menu_name(menu, command)
        };
        let name: Vec<u16> = name
            .as_deref()
            .unwrap_or(fallback)
            .encode_utf16()
            .chain(Some(0))
            .collect();
        let _ = SetWindowTextW(button, PCWSTR(name.as_ptr()));
    }
}
/// The next enabled button in the direction of travel, wrapping around.
fn next_enabled(enabled: [bool; 3], from: usize, forward: bool) -> Option<usize> {
    (1..3)
        .map(|step| (from + if forward { step } else { 3 - step }) % 3)
        .find(|index| enabled[*index])
}
unsafe fn visible(hwnd: HWND, show: bool) {
    if (GetWindowLongPtrW(hwnd, GWL_STYLE) as u32 & WS_VISIBLE.0 != 0) != show {
        let _ = ShowWindow(hwnd, if show { SW_SHOWNA } else { SW_HIDE });
    }
}
unsafe fn render(p: *mut State) -> bool {
    if (*p).opacity == 0 {
        return true;
    }
    let dpi = GetDpiForWindow((*p).owner).max(96);
    let (width, height, _, _) = paint::dimensions(dpi);
    let mut rect = RECT::default();
    let _ = GetClientRect((*p).owner, &mut rect);
    let mut origin = POINT::default();
    let _ = ClientToScreen((*p).owner, &mut origin);
    let shown = (height as f64 * f64::from((*p).opacity) / 255.0).round() as i32;
    let foreground = GetForegroundWindow();
    let index = |hwnd| (*p).buttons.iter().position(|button| *button == hwnd);
    let appearance = paint::Appearance {
        dpi,
        dark: (*p).dark,
        active: foreground == (*p).owner || foreground == (*p).edge,
        maximized: IsZoomed((*p).owner).as_bool(),
        hot: index((*p).hot_button),
        pressed: index((*p).pressed_button),
        focused: index(GetFocus()),
        enabled: enabled_buttons((*p).owner),
        contrast: super::menus::high_contrast().then(|| {
            [
                GetSysColor(COLOR_WINDOW),
                GetSysColor(COLOR_WINDOWTEXT),
                GetSysColor(COLOR_HIGHLIGHT),
                GetSysColor(COLOR_HIGHLIGHTTEXT),
                GetSysColor(COLOR_GRAYTEXT),
            ]
        }),
    };
    let Some(mut surface) = (*p).surface.take() else {
        return false;
    };
    let ok = surface.present(
        (*p).strip,
        POINT {
            x: origin.x + rect.right - width,
            y: origin.y,
        },
        shown,
        (*p).opacity,
        appearance,
    );
    (*p).surface = Some(surface);
    if !ok && !(*p).render_failed {
        (*p).render_failed = true;
        crate::write_diagnostic(format_args!("caption: alpha surface presentation failed"));
    }
    ok
}
unsafe fn layout(p: *mut State) {
    if (*p).laying_out {
        return;
    }
    (*p).laying_out = true;
    let owner = (*p).owner;
    let dpi = GetDpiForWindow(owner).max(96);
    let scale = dpi as f64 / 96.0;
    let (controls_width, height, button_width, padding) = paint::dimensions(dpi);
    let button_height = height - padding * 2;
    let mut rect = RECT::default();
    let _ = GetClientRect(owner, &mut rect);
    let left = (*p)
        .page_left
        .map(|left| (left * scale).round() as i32)
        .unwrap_or(rect.right - controls_width)
        .clamp(0, (rect.right - controls_width).max(0));
    let mut origin = POINT::default();
    let _ = ClientToScreen(owner, &mut origin);
    // Owned popup z-order follows the owner. Hover may reveal an inactive
    // window, but must never raise it over another application's foreground.
    let show = !(*p).changing_state
        && !(*p).suppressed
        && IsWindowVisible(owner).as_bool()
        && IsWindowEnabled(owner).as_bool()
        && !IsIconic(owner).as_bool();
    if !show {
        (*p).expanded = false;
        (*p).opacity = 0;
        (*p).last_hot = HWND::default();
        (*p).pressed_button = HWND::default();
        (*p).drag_start = None;
        (*p).motion_paused = None;
        let capture = GetCapture();
        if capture == (*p).edge || (*p).buttons.contains(&capture) {
            let _ = ReleaseCapture();
        }
        cancel_reveal(p);
        let _ = KillTimer(Some((*p).strip), FADE);
        let _ = KillTimer(Some((*p).strip), HIDE);
    }
    let progress = f64::from((*p).opacity) / 255.0;
    let displacement = (height as f64 * progress).round() as i32;
    zephium_engine::set_window_caption_offset(
        owner.0 as isize,
        if (*p).page_left.is_some() {
            displacement as f64 / scale
        } else {
            0.0
        },
        (*p).page_left.is_some() && (*p).opacity > 0 && (*p).opacity < 255,
    );
    let geometry = (origin.x, origin.y, rect.right, left, displacement, dpi);
    if (*p).geometry != Some(geometry) {
        (*p).geometry = Some(geometry);
        let reveal_height = (8.0 * scale).round() as i32;
        let _ = SetWindowPos(
            (*p).edge,
            None,
            origin.x,
            origin.y,
            rect.right.max(1),
            height.max(reveal_height),
            SWP_NOACTIVATE | SWP_NOZORDER | SWP_NOOWNERZORDER,
        );
        let region = CreateRectRgn(0, 0, rect.right.max(1), reveal_height);
        let band = CreateRectRgn(left, 0, rect.right.max(1), displacement);
        let _ = CombineRgn(Some(region), Some(region), Some(band), RGN_OR);
        let _ = DeleteObject(band.into());
        // Where the controls stand is theirs even while they are away.
        let spot = CreateRectRgn(
            (rect.right - controls_width).max(0),
            0,
            rect.right.max(1),
            height,
        );
        let _ = CombineRgn(Some(region), Some(region), Some(spot), RGN_OR);
        let _ = DeleteObject(spot.into());
        if SetWindowRgn((*p).edge, Some(region), false) == 0 {
            let _ = DeleteObject(region.into());
        }
        for (index, hwnd) in (*p).buttons.into_iter().enumerate() {
            let _ = SetWindowPos(
                hwnd,
                None,
                rect.right - controls_width + padding + index as i32 * button_width,
                displacement - height + padding,
                button_width,
                button_height,
                SWP_NOZORDER | SWP_NOACTIVATE,
            );
        }
    }
    let zoomed = IsZoomed(owner).as_bool();
    if (*p).named != Some(zoomed) {
        (*p).named = Some(zoomed);
        name_buttons(p, zoomed);
    }
    let controls_visible = show && displacement > 0 && render(p);
    visible((*p).strip, controls_visible);
    // A withdrawn button is only painted. Taking its window off the input host
    // leaves nothing to click, focus or announce.
    let enabled = enabled_buttons(owner);
    for (button, enabled) in (*p).buttons.into_iter().zip(enabled) {
        visible(button, controls_visible && enabled);
    }
    visible((*p).edge, show);
    (*p).laying_out = false;
}
// The top target forwards native resize gestures: the child WebView otherwise
// consumes this frameless client border before the owner sees it.
unsafe fn resizable(owner: HWND) -> bool {
    !IsZoomed(owner).as_bool() && GetWindowLongPtrW(owner, GWL_STYLE) as u32 & WS_THICKFRAME.0 != 0
}
unsafe fn resize_border(owner: HWND) -> i32 {
    if !resizable(owner) {
        return 0;
    }
    let dpi = GetDpiForWindow(owner).max(96);
    // The right gutter lies over the page, so only the visible outer rim of
    // the sizing frame belongs to it.
    (2 * GetSystemMetricsForDpi(SM_CYBORDER, dpi)).max(1)
}
// The top band is the reveal edge itself, so it has the sizing frame's full
// depth: a few pixels are too narrow to catch without overshooting.
unsafe fn top_resize_border(owner: HWND) -> i32 {
    if !resizable(owner) {
        return 0;
    }
    let dpi = GetDpiForWindow(owner).max(96);
    GetSystemMetricsForDpi(SM_CYFRAME, dpi) + GetSystemMetricsForDpi(SM_CXPADDEDBORDER, dpi)
}
fn top_frame_hit(
    x: i32,
    y: i32,
    width: i32,
    border: i32,
    corner: i32,
    skip: Option<(i32, i32)>,
) -> Option<u32> {
    if border <= 0 || x < 0 || x >= width || y < 0 || y >= border {
        return None;
    }
    if skip.is_some_and(|(from, to)| x >= from && x < to) {
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
    let top = if (*p).suppressed {
        0
    } else {
        top_resize_border((*p).owner)
    };
    let dpi = GetDpiForWindow((*p).owner).max(96);
    let corner = 2
        * (GetSystemMetricsForDpi(SM_CYSIZEFRAME, dpi)
            + GetSystemMetricsForDpi(SM_CXPADDEDBORDER, dpi));
    // The buttons keep their own column of the top band while they are out.
    let controls = ((*p).opacity > 0).then(|| (rect.right - paint::dimensions(dpi).0, rect.right));
    top_frame_hit(point.x, point.y, rect.right, top, corner, controls).or_else(|| {
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
    let _ = ShowWindow((*p).strip, SW_HIDE);
    let _ = ShowWindow((*p).edge, SW_HIDE);
    (*p).expanded = false;
    (*p).opacity = 0;
    (*p).hot_button = HWND::default();
    (*p).last_hot = HWND::default();
    (*p).fade_from = 0;
    (*p).pressed_button = HWND::default();
    (*p).drag_start = None;
    (*p).motion_paused = None;
    let mut cursor = POINT::default();
    let _ = GetCursorPos(&mut cursor);
    (*p).reveal_block = Some(cursor);
    cancel_reveal(p);
    zephium_engine::set_window_caption_offset((*p).owner.0 as isize, 0.0, false);
    let _ = KillTimer(Some((*p).strip), FADE);
    let _ = KillTimer(Some((*p).strip), HIDE);
}
unsafe fn transition(p: *mut State, expanded: bool) {
    if (*p).expanded == expanded {
        return;
    }
    (*p).expanded = expanded;
    if !expanded {
        (*p).last_hot = HWND::default();
    }
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
    if (*p).suppressed || (*p).changing_state || (*p).sizing {
        return;
    }
    let _ = KillTimer(Some((*p).strip), HIDE);
    transition(p, true);
}
unsafe fn cancel_reveal(p: *mut State) {
    if (*p).reveal_armed {
        (*p).reveal_armed = false;
        let _ = KillTimer(Some((*p).strip), REVEAL);
    }
}
fn dragged_beyond(origin: POINT, point: POINT, width: i32, height: i32) -> bool {
    (point.x - origin.x).abs() >= width || (point.y - origin.y).abs() >= height
}
/// Whether the pointer has left the spot a state change retired the caption
/// under; leaving it ends the hold for good.
unsafe fn pointer_moved_on(p: *mut State) -> bool {
    let Some(origin) = (*p).reveal_block else {
        return true;
    };
    let mut cursor = POINT::default();
    let _ = GetCursorPos(&mut cursor);
    let moved = dragged_beyond(
        origin,
        cursor,
        GetSystemMetrics(SM_CXDRAG),
        GetSystemMetrics(SM_CYDRAG),
    );
    if moved {
        (*p).reveal_block = None;
    }
    moved
}
/// Hovering the reveal edge asks for the caption after a short dwell. With the
/// caption out, the pointer where the controls stand, or on Maximize (the
/// shell's Snap flyout opens from it), hovering brings it without waiting.
unsafe fn hover_reveal(p: *mut State, hot: HWND) {
    if (*p).suppressed || (*p).changing_state || (*p).sizing || !pointer_moved_on(p) {
        return;
    }
    if (*p).expanded || hot == (*p).buttons[1] || at_controls(p) {
        cancel_reveal(p);
        reveal(p);
    } else if !(*p).reveal_armed {
        (*p).reveal_armed = true;
        SetTimer(Some((*p).strip), REVEAL, REVEAL_DWELL_MS, None);
    }
}
unsafe fn in_front(owner: HWND) -> bool {
    GetAncestor(GetForegroundWindow(), GA_ROOTOWNER) == owner
}
/// Whether the pointer is over a web page, in front, within reach of the
/// window's top. The sidebar and Zephium's own pages are left alone: their
/// controls are there to be used, not reached past.
unsafe fn near_top(p: *mut State) -> bool {
    let owner = (*p).owner;
    if !in_front(owner) {
        return false;
    }
    let mut cursor = POINT::default();
    if GetCursorPos(&mut cursor).is_err() || GetAncestor(WindowFromPoint(cursor), GA_ROOT) != owner
    {
        return false;
    }
    let mut at = cursor;
    let mut client = RECT::default();
    if !ScreenToClient(owner, &mut at).as_bool() || GetClientRect(owner, &mut client).is_err() {
        return false;
    }
    let Some(page_left) = (*p).page_left else {
        return false;
    };
    let scale = GetDpiForWindow(owner).max(96) as f64 / 96.0;
    let reach = (TOP_REACH * scale).round() as i32;
    at.y >= 0 && at.y < reach && at.x >= (page_left * scale).round() as i32 && at.x < client.right
}
/// Whether the pointer is where the controls stand when they are out.
unsafe fn at_controls(p: *mut State) -> bool {
    let mut edge = RECT::default();
    if GetWindowRect((*p).edge, &mut edge).is_err() {
        return false;
    }
    let (controls_width, height, _, _) = paint::dimensions(GetDpiForWindow((*p).owner).max(96));
    let spot = RECT {
        left: edge.right - controls_width,
        top: edge.top,
        right: edge.right,
        bottom: edge.top + height,
    };
    let mut cursor = POINT::default();
    GetCursorPos(&mut cursor).is_ok() && PtInRect(&spot, cursor).as_bool()
}
/// Where focus returns when the caption lets go of it.
unsafe fn focus_target(p: *mut State) -> HWND {
    if IsWindow(Some((*p).previous_focus)).as_bool() {
        (*p).previous_focus
    } else {
        (*p).owner
    }
}
/// What the system menu offers for a window's styles and state, which Windows
/// only refreshes for a menu it opens itself.
fn system_menu_items(style: u32, zoomed: bool) -> [(u32, bool); 5] {
    [
        (SC_RESTORE, zoomed),
        (SC_MOVE, !zoomed),
        (SC_SIZE, !zoomed && style & WS_THICKFRAME.0 != 0),
        (SC_MINIMIZE, style & WS_MINIMIZEBOX.0 != 0),
        (SC_MAXIMIZE, !zoomed && style & WS_MAXIMIZEBOX.0 != 0),
    ]
}
/// The owner's system menu at a screen position, with the chosen command sent
/// as it would be from a native caption.
unsafe fn show_system_menu(p: *mut State, at: POINT) {
    let owner = (*p).owner;
    if (*p).menu_open || (*p).changing_state || (*p).sizing {
        return;
    }
    let menu = GetSystemMenu(owner, false);
    if menu.is_invalid() {
        return;
    }
    let zoomed = IsZoomed(owner).as_bool();
    let style = GetWindowLongPtrW(owner, GWL_STYLE) as u32;
    for (command, enabled) in system_menu_items(style, zoomed) {
        let state = if enabled { MF_ENABLED } else { MF_GRAYED };
        let _ = EnableMenuItem(menu, command, MF_BYCOMMAND | state);
    }
    let _ = SetMenuDefaultItem(menu, if zoomed { SC_RESTORE } else { SC_CLOSE }, 0);
    (*p).menu_open = true;
    let _ = SetForegroundWindow(owner);
    let align = if GetSystemMetrics(SM_MENUDROPALIGNMENT) != 0 {
        TPM_RIGHTALIGN
    } else {
        TPM_LEFTALIGN
    };
    let command = TrackPopupMenu(
        menu,
        TPM_RETURNCMD | TPM_RIGHTBUTTON | align,
        at.x,
        at.y,
        None,
        owner,
        None,
    )
    .0 as u32;
    // The menu runs its own message loop, and the window can be destroyed in it.
    if installed_state(owner) != Some(p) {
        return;
    }
    (*p).menu_open = false;
    SetTimer(Some((*p).strip), HIDE, 450, None);
    if command != 0 {
        let _ = PostMessageW(
            Some(owner),
            WM_SYSCOMMAND,
            WPARAM(command as usize),
            LPARAM(((at.y as u16 as u32) << 16 | at.x as u16 as u32) as isize),
        );
    }
}
/// Where the shell's Snap layouts flyout can be: under Maximize and about as
/// far out to either side as it is deep.
fn within_snap_reach(point: POINT, maximize: &RECT, reach: i32) -> bool {
    point.y >= maximize.bottom
        && point.y < maximize.bottom + reach
        && point.x >= maximize.left - reach
        && point.x < maximize.right + reach
}
/// Whether the pointer is over a window of another process that is a
/// top-level one. The WebView's own child windows belong to another process
/// too, but their top-level ancestor is ours.
unsafe fn over_foreign_window(point: POINT) -> bool {
    let hovered = WindowFromPoint(point);
    if hovered.0.is_null() {
        return false;
    }
    let root = GetAncestor(hovered, GA_ROOT);
    let mut process = 0u32;
    GetWindowThreadProcessId(
        if root.0.is_null() { hovered } else { root },
        Some(&mut process),
    );
    process != 0 && process != std::process::id()
}
unsafe fn button_at_cursor(p: *mut State) -> HWND {
    let mut cursor = POINT::default();
    let _ = GetCursorPos(&mut cursor);
    button_at_point(p, cursor)
}
fn button_hit(rects: &[RECT; 3], point: POINT) -> Option<usize> {
    rects.iter().position(|rect| {
        point.x >= rect.left && point.x < rect.right && point.y >= rect.top && point.y < rect.bottom
    })
}
/// Maximized, the pills sit a few pixels inside the screen, so a pointer
/// thrown into the corner would miss them. Their hit areas run on to the
/// client's top and right edges; what is painted does not change.
fn reach_corner(rects: &mut [RECT; 3], client: &RECT) {
    for rect in rects.iter_mut() {
        rect.top = rect.top.min(client.top);
    }
    rects[2].right = rects[2].right.max(client.right);
}
unsafe fn button_rects(p: *mut State) -> [RECT; 3] {
    let mut rects = [RECT::default(); 3];
    for (button, rect) in (*p).buttons.into_iter().zip(&mut rects) {
        let _ = GetWindowRect(button, rect);
    }
    if IsZoomed((*p).owner).as_bool() {
        let mut client = RECT::default();
        let _ = GetWindowRect((*p).edge, &mut client);
        reach_corner(&mut rects, &client);
    }
    rects
}
unsafe fn button_at_point(p: *mut State, point: POINT) -> HWND {
    if (*p).opacity == 0 || (*p).suppressed {
        return HWND::default();
    }
    let mut visible_bounds = RECT::default();
    let _ = GetWindowRect((*p).edge, &mut visible_bounds);
    if !PtInRect(&visible_bounds, point).as_bool() {
        // During reveal, part of each child lies above the clipped input host.
        // Releasing there must not activate an invisible part of the button.
        return HWND::default();
    }
    let rects = button_rects(p);
    let enabled = enabled_buttons((*p).owner);
    button_hit(&rects, point)
        .filter(|index| enabled[*index])
        .map_or(HWND::default(), |index| (*p).buttons[index])
}
unsafe fn update_hot(p: *mut State, hot: HWND) {
    if (*p).hot_button != hot {
        (*p).hot_button = hot;
        if !hot.0.is_null() {
            (*p).last_hot = hot;
        }
        let _ = render(p);
    }
}
unsafe fn pause_motion(p: *mut State) {
    (*p).motion_paused
        .get_or_insert_with(std::time::Instant::now);
    let _ = KillTimer(Some((*p).strip), FADE);
}
unsafe fn resume_motion(p: *mut State) {
    if let Some(paused) = (*p).motion_paused.take() {
        (*p).fade_started += paused.elapsed();
        if (*p).opacity != if (*p).expanded { 255 } else { 0 } {
            SetTimer(Some((*p).strip), FADE, 15, None);
        }
    }
}
// The same press/capture/release path covers the entire button, regardless of
// whether Windows delivered a client or nonclient message (Snap's HTMAXBUTTON).
unsafe fn pointer_input(
    p: *mut State,
    hwnd: HWND,
    msg: u32,
    w: WPARAM,
    l: LPARAM,
) -> Option<LRESULT> {
    match msg {
        WM_LBUTTONDOWN | WM_NCLBUTTONDOWN => {
            let button = button_at_cursor(p);
            if !button.0.is_null() && resize_hit_from_cursor(p).is_none() {
                let _ = SetForegroundWindow((*p).owner);
                (*p).pressed_button = button;
                pause_motion(p);
                SetCapture(hwnd);
                let _ = render(p);
                return Some(LRESULT(0));
            }
        }
        WM_LBUTTONUP | WM_NCLBUTTONUP if !(*p).pressed_button.0.is_null() => {
            let pressed = (*p).pressed_button;
            let activate = button_at_cursor(p) == pressed;
            (*p).pressed_button = HWND::default();
            resume_motion(p);
            let _ = ReleaseCapture();
            let _ = render(p);
            SetTimer(Some((*p).strip), HIDE, 450, None);
            if activate {
                let index = (*p)
                    .buttons
                    .iter()
                    .position(|button| *button == pressed)
                    .unwrap();
                let _ = SendMessageW(
                    (*p).strip,
                    WM_COMMAND,
                    Some(WPARAM(index + 1)),
                    Some(LPARAM(pressed.0 as isize)),
                );
            }
            return Some(LRESULT(0));
        }
        WM_CAPTURECHANGED | WM_CANCELMODE => {
            let pressed = (*p).pressed_button;
            (*p).pressed_button = HWND::default();
            (*p).drag_start = None;
            resume_motion(p);
            if !pressed.0.is_null() {
                let _ = render(p);
            }
            SetTimer(Some((*p).strip), HIDE, 450, None);
        }
        WM_MOUSEMOVE | WM_NCMOUSEMOVE => {
            if let Some(start) = (*p).drag_start {
                let mut point = POINT::default();
                let _ = GetCursorPos(&mut point);
                if dragged_beyond(
                    start,
                    point,
                    GetSystemMetrics(SM_CXDRAG),
                    GetSystemMetrics(SM_CYDRAG),
                ) {
                    cancel_reveal(p);
                    (*p).drag_start = None;
                    let _ = ReleaseCapture();
                    let _ = PostMessageW(
                        Some((*p).owner),
                        WM_SYSCOMMAND,
                        WPARAM((SC_MOVE | HTCAPTION) as usize),
                        LPARAM(0),
                    );
                    return Some(LRESULT(0));
                }
            }
            track(hwnd, msg == WM_NCMOUSEMOVE);
            let hot = button_at_cursor(p);
            hover_reveal(p, hot);
            update_hot(p, hot);
            if hot == (*p).buttons[1] && msg == WM_NCMOUSEMOVE {
                // Let the owner offer Snap layouts; clicks stay on our explicit
                // capture path instead of DefWindowProc tracking another HWND.
                let _ = SendMessageW((*p).owner, msg, Some(w), Some(l));
            }
            return Some(LRESULT(0));
        }
        WM_MOUSELEAVE | WM_NCMOUSELEAVE => {
            cancel_reveal(p);
            // Crossing into the neighbouring button ends this window's hover,
            // not the pointer's.
            update_hot(p, button_at_cursor(p));
            SetTimer(Some((*p).strip), HIDE, 450, None);
        }
        _ => {}
    }
    None
}
unsafe fn resize_hit_from_cursor(p: *mut State) -> Option<u32> {
    let mut cursor = POINT::default();
    let _ = GetCursorPos(&mut cursor);
    resize_hit(
        p,
        LPARAM(((cursor.y as u16 as u32) << 16 | cursor.x as u16 as u32) as isize),
    )
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
    if let Some(result) = pointer_input(p, hwnd, msg, w, l) {
        return result;
    }
    match msg {
        WM_NCHITTEST => {
            if let Some(hit) = resize_hit(p, l) {
                return LRESULT(hit as isize);
            }
            if button_at_point(
                p,
                POINT {
                    x: l.0 as i16 as i32,
                    y: (l.0 >> 16) as i16 as i32,
                },
            ) == (*p).buttons[1]
            {
                return LRESULT(HTMAXBUTTON as isize);
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
        WM_COMMAND => return SendMessageW((*p).strip, msg, Some(w), Some(l)),
        WM_DRAWITEM => return LRESULT(1),
        // Keep the edge useful as a native drag handle.
        WM_LBUTTONDOWN => {
            // Entering the move loop on the first click would hide the band
            // before a second click could maximize it.
            let mut point = POINT::default();
            let _ = GetCursorPos(&mut point);
            let _ = SetForegroundWindow((*p).owner);
            cancel_reveal(p);
            reveal(p);
            (*p).drag_start = Some(point);
            pause_motion(p);
            SetCapture(hwnd);
            return LRESULT(0);
        }
        WM_LBUTTONUP if (*p).drag_start.take().is_some() => {
            let _ = ReleaseCapture();
            return LRESULT(0);
        }
        WM_RBUTTONUP => {
            let mut point = POINT::default();
            let _ = GetCursorPos(&mut point);
            show_system_menu(p, point);
            return LRESULT(0);
        }
        WM_LBUTTONDBLCLK => {
            let command = if IsZoomed((*p).owner).as_bool() {
                SC_RESTORE
            } else if enabled_buttons((*p).owner)[1] {
                SC_MAXIMIZE
            } else {
                return LRESULT(0);
            };
            let _ = PostMessageW(
                Some((*p).owner),
                WM_SYSCOMMAND,
                WPARAM(command as usize),
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
        zephium_engine::forget_window_caption_offset(hwnd.0 as isize);
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
    if msg == WM_CLOSE {
        prepare_state_change(p);
        // A vetoed close must leave an operable window. This timer belongs to
        // the owner and is discarded automatically if close destroys it.
        SetTimer(Some(hwnd), CLOSE_SETTLED, 100, None);
        return DefSubclassProc(hwnd, msg, w, l);
    }
    if msg == WM_SYSCOMMAND && w.0 as u32 & 0xfff0 == SC_CLOSE {
        // DefWindowProc can destroy the subclass state synchronously.
        return DefSubclassProc(hwnd, msg, w, l);
    }
    // Alt+Space, as Windows or the WebView accelerator bridge sends it. The
    // frameless window has no caption for DefWindowProc to hang the menu on.
    if msg == WM_SYSCOMMAND && w.0 as u32 & 0xfff0 == SC_KEYMENU && l.0 == ' ' as isize {
        let mut origin = POINT::default();
        let _ = ClientToScreen(hwnd, &mut origin);
        show_system_menu(p, origin);
        return LRESULT(0);
    }
    if msg == WM_TIMER && w.0 == CLOSE_SETTLED {
        let _ = KillTimer(Some(hwnd), CLOSE_SETTLED);
        (*p).changing_state = false;
        layout(p);
        return LRESULT(0);
    }
    if !(*p).suppressed
        && !(*p).changing_state
        && !(*p).sizing
        && msg == WM_SYSKEYDOWN
        && w.0 == VK_F10.0 as usize
    {
        reveal(p);
        // Keyboard focus requires a fully visible native target immediately.
        (*p).opacity = 255;
        let _ = KillTimer(Some((*p).strip), FADE);
        layout(p);
        let first = enabled_buttons(hwnd).iter().position(|enabled| *enabled);
        let _ = SetFocus(Some((*p).buttons[first.unwrap_or(2)]));
        return LRESULT(0);
    }
    let state_command = msg == WM_SYSCOMMAND
        && matches!(w.0 as u32 & 0xfff0, SC_MINIMIZE | SC_MAXIMIZE | SC_RESTORE);
    if state_command {
        prepare_state_change(p);
    }
    if msg == WM_ENTERSIZEMOVE {
        (*p).sizing = true;
        pause_motion(p);
        layout(p);
    } else if msg == WM_EXITSIZEMOVE {
        (*p).sizing = false;
        resume_motion(p);
        SetTimer(Some((*p).strip), HIDE, 450, None);
        layout(p);
    }
    if msg == WM_NCHITTEST {
        if let Some(hit) = resize_hit(p, l) {
            return LRESULT(hit as isize);
        }
    }
    if msg == WM_NCHITTEST && (*p).expanded && enabled_buttons(hwnd)[1] {
        let point = POINT {
            x: l.0 as i16 as i32,
            y: (l.0 >> 16) as i16 as i32,
        };
        if PtInRect(&button_rects(p)[1], point).as_bool() {
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
            | WM_STYLECHANGED
            | WM_WINDOWPOSCHANGED
            | WM_PARENTNOTIFY
            | WM_ACTIVATE
            | WM_SHOWWINDOW
    ) {
        layout(p);
    }
    if matches!(msg, WM_THEMECHANGED | WM_SETTINGCHANGE) {
        let _ = render(p);
    }
    // The foreground may not have settled yet, so the first tick decides
    // whether the watch keeps running.
    if matches!(
        msg,
        WM_ACTIVATE | WM_ACTIVATEAPP | WM_NCACTIVATE | WM_SIZE | WM_SHOWWINDOW
    ) && !(*p).watching
        && !IsIconic(hwnd).as_bool()
    {
        (*p).watching = true;
        SetTimer(Some((*p).strip), WATCH, WATCH_MS, None);
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
            return LRESULT(HTCAPTION as isize);
        }
        WM_NCLBUTTONDOWN | WM_NCLBUTTONDBLCLK if w.0 == HTCAPTION as usize => {
            return SendMessageW((*p).owner, msg, Some(w), Some(l));
        }
        WM_NCRBUTTONUP if w.0 == HTCAPTION as usize => {
            let mut point = POINT::default();
            let _ = GetCursorPos(&mut point);
            show_system_menu(p, point);
            return LRESULT(0);
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
            hover_reveal(p, HWND::default());
            track(hwnd, false);
        }
        WM_NCMOUSEMOVE => {
            hover_reveal(p, HWND::default());
            track(hwnd, true);
        }
        WM_MOUSELEAVE | WM_NCMOUSELEAVE => {
            cancel_reveal(p);
            SetTimer(Some(hwnd), HIDE, 450, None);
        }
        WM_TIMER if w.0 == REVEAL => {
            let _ = KillTimer(Some(hwnd), REVEAL);
            (*p).reveal_armed = false;
            let mut cursor = POINT::default();
            let _ = GetCursorPos(&mut cursor);
            let hovered = WindowFromPoint(cursor);
            if (hovered == (*p).edge || (*p).buttons.contains(&hovered) || near_top(p))
                && pointer_moved_on(p)
            {
                reveal(p);
            }
            return LRESULT(0);
        }
        WM_TIMER if w.0 == WATCH => {
            let near = near_top(p);
            if near && !(*p).expanded {
                if !(*p).suppressed
                    && !(*p).changing_state
                    && !(*p).sizing
                    && !(*p).reveal_armed
                    && pointer_moved_on(p)
                {
                    (*p).reveal_armed = true;
                    SetTimer(Some(hwnd), REVEAL, TOP_DWELL_MS, None);
                }
            } else if !near && (*p).near_top {
                cancel_reveal(p);
                SetTimer(Some(hwnd), HIDE, 450, None);
            }
            (*p).near_top = near;
            if IsIconic((*p).owner).as_bool() || !in_front((*p).owner) {
                let _ = KillTimer(Some(hwnd), WATCH);
                (*p).watching = false;
            }
            return LRESULT(0);
        }
        WM_TIMER if w.0 == FADE => {
            // A press owns a stable target even when it begins mid-reveal.
            if (*p).motion_paused.is_some() {
                return LRESULT(0);
            }
            let t = ((*p).fade_started.elapsed().as_secs_f64() / 0.22).min(1.0);
            let eased = 1.0 - (1.0 - t).powi(3);
            let target = if (*p).expanded { 255.0 } else { 0.0 };
            (*p).opacity = (f64::from((*p).fade_from)
                + (target - f64::from((*p).fade_from)) * eased)
                .round() as u8;
            layout(p);
            if t >= 1.0 {
                let _ = KillTimer(Some(hwnd), FADE);
                if !(*p).expanded {
                    let _ = ShowWindow(hwnd, SW_HIDE);
                    layout(p);
                }
            }
            return LRESULT(0);
        }
        WM_TIMER if w.0 == HIDE => {
            let _ = KillTimer(Some(hwnd), HIDE);
            // Release, focus loss and the end of the move loop reschedule the
            // dismissal. No polling timer runs while a gesture owns the band.
            if (*p).sizing
                || (*p).menu_open
                || near_top(p)
                || !(*p).pressed_button.0.is_null()
                || (*p).drag_start.is_some()
                || (*p).buttons.contains(&GetFocus())
            {
                return LRESULT(0);
            }
            let mut cursor = POINT::default();
            let _ = GetCursorPos(&mut cursor);
            // Snap layouts open from Maximize as a flyout of the shell, and the
            // caption must stay for the whole pick. The next tick looks again.
            let reach = 500 * GetDpiForWindow((*p).owner).max(96) as i32 / 96;
            if (*p).last_hot == (*p).buttons[1]
                && within_snap_reach(cursor, &button_rects(p)[1], reach)
                && over_foreign_window(cursor)
            {
                SetTimer(Some(hwnd), HIDE, 450, None);
                return LRESULT(0);
            }
            let mut rect = RECT::default();
            let _ = GetWindowRect((*p).edge, &mut rect);
            rect.left += (*p).geometry.map_or(0, |(_, _, _, left, _, _)| left);
            let mut edge = RECT::default();
            let _ = GetWindowRect((*p).edge, &mut edge);
            edge.bottom =
                edge.top + (8.0 * GetDpiForWindow((*p).owner).max(96) as f64 / 96.0).round() as i32;
            // Moving along the reveal strip must not dismiss the controls.
            let hovered = WindowFromPoint(cursor);
            let on_caption = hovered == (*p).edge || (*p).buttons.contains(&hovered);
            if !on_caption
                || (!PtInRect(&rect, cursor).as_bool() && !PtInRect(&edge, cursor).as_bool())
            {
                transition(p, false);
            } else {
                // The input host stays visible, so a leave event can dismiss
                // the band without a polling timer during a stationary hover.
                track(hovered, button_at_cursor(p) == (*p).buttons[1]);
            }
        }
        WM_COMMAND if (w.0 >> 16) == BN_CLICKED as usize => {
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
            if !enabled_buttons((*p).owner)[(w.0 & 0xffff) - 1] {
                return LRESULT(0);
            }
            // Retire visible pixels before focus/default-button processing,
            // including Close, which used to repaint all three child buttons.
            prepare_state_change(p);
            // Only a keyboard activation leaves focus on a button. After a
            // click the page still holds it, and focusing the owner would have
            // wry move it to the chrome WebView.
            if (*p).buttons.contains(&GetFocus()) {
                let _ = SetFocus(Some(focus_target(p)));
            }
            let _ = PostMessageW(
                Some((*p).owner),
                WM_SYSCOMMAND,
                WPARAM(command as usize),
                LPARAM(0),
            );
            return LRESULT(0);
        }
        WM_ERASEBKGND => return LRESULT(1),
        WM_PAINT => {
            let mut paint = PAINTSTRUCT::default();
            let _ = BeginPaint(hwnd, &mut paint);
            let _ = EndPaint(hwnd, &paint);
            return LRESULT(1);
        }
        WM_NCDESTROY => {
            let _ = KillTimer(Some(hwnd), HIDE);
            let _ = KillTimer(Some(hwnd), FADE);
            let _ = KillTimer(Some(hwnd), REVEAL);
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
    if let Some(result) = pointer_input(p, hwnd, msg, w, l) {
        return result;
    }
    match msg {
        BM_CLICK => {
            // Accessibility invocation has no cursor location. Route the
            // requested native button directly instead of simulating a hit.
            if let Some(index) = (*p).buttons.iter().position(|button| *button == hwnd) {
                return SendMessageW(
                    (*p).strip,
                    WM_COMMAND,
                    Some(WPARAM(index + 1)),
                    Some(LPARAM(hwnd.0 as isize)),
                );
            }
        }
        WM_ERASEBKGND => return LRESULT(1),
        WM_PAINT => {
            // These controls supply semantics/input only. The alpha surface
            // owns every visible pixel, including focus and pressed feedback.
            let mut paint = PAINTSTRUCT::default();
            let _ = BeginPaint(hwnd, &mut paint);
            let _ = EndPaint(hwnd, &paint);
            return LRESULT(0);
        }
        WM_NCHITTEST => {
            if let Some(hit) = resize_hit(p, l) {
                return LRESULT(hit as isize);
            }
            if hwnd == (*p).buttons[1] {
                return LRESULT(HTMAXBUTTON as isize);
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
            let _ = render(p);
        }
        WM_KILLFOCUS => {
            if (*p).pressed_button == hwnd {
                (*p).pressed_button = HWND::default();
                resume_motion(p);
            }
            if (*p).hot_button == hwnd {
                (*p).hot_button = HWND::default();
            }
            SetTimer(Some((*p).strip), HIDE, 450, None);
            let _ = render(p);
        }
        WM_KEYDOWN if w.0 == VK_SPACE.0 as usize || w.0 == VK_RETURN.0 as usize => {
            (*p).pressed_button = hwnd;
            (*p).hot_button = hwnd;
            let _ = render(p);
        }
        // A native button clicks on Space alone; Enter is ours to complete.
        WM_KEYUP if w.0 == VK_RETURN.0 as usize => {
            let armed = (*p).pressed_button == hwnd;
            (*p).pressed_button = HWND::default();
            let _ = render(p);
            if let Some(index) = (*p).buttons.iter().position(|button| *button == hwnd) {
                if armed {
                    return SendMessageW(
                        (*p).strip,
                        WM_COMMAND,
                        Some(WPARAM(index + 1)),
                        Some(LPARAM(hwnd.0 as isize)),
                    );
                }
            }
            return LRESULT(0);
        }
        WM_KEYUP if w.0 == VK_SPACE.0 as usize => {
            (*p).pressed_button = HWND::default();
            let _ = render(p);
        }
        WM_KEYDOWN if w.0 == VK_ESCAPE.0 as usize => {
            let _ = SetFocus(Some(focus_target(p)));
            transition(p, false);
            return LRESULT(0);
        }
        WM_KEYDOWN
            if w.0 == VK_TAB.0 as usize
                || w.0 == VK_LEFT.0 as usize
                || w.0 == VK_RIGHT.0 as usize =>
        {
            let forward = if w.0 == VK_TAB.0 as usize {
                GetKeyState(VK_SHIFT.0 as i32) >= 0
            } else {
                w.0 == VK_RIGHT.0 as usize
            };
            let index = (*p).buttons.iter().position(|b| *b == hwnd).unwrap_or(0);
            if let Some(next) = next_enabled(enabled_buttons((*p).owner), index, forward) {
                let _ = SetFocus(Some((*p).buttons[next]));
            }
            return LRESULT(0);
        }
        WM_NCDESTROY => {
            let _ = RemoveWindowSubclass(hwnd, Some(button_proc), ID);
        }
        _ => {}
    }
    DefSubclassProc(hwnd, msg, w, l)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn complete_button_rectangles_hit_at_fractional_dpi_and_negative_monitor_coordinates() {
        for dpi in [96, 120, 144, 192] {
            let (_, outer_height, width, padding) = paint::dimensions(dpi);
            let height = outer_height - padding * 2;
            let rects = std::array::from_fn(|index| RECT {
                left: -800 + index as i32 * width,
                right: -800 + (index as i32 + 1) * width,
                top: -100,
                bottom: -100 + height,
            });
            for (index, rect) in rects.iter().enumerate() {
                for x in rect.left..rect.right {
                    for y in rect.top..rect.bottom {
                        assert_eq!(button_hit(&rects, POINT { x, y }), Some(index));
                    }
                }
            }
            assert_eq!(
                button_hit(
                    &rects,
                    POINT {
                        x: rects[2].right,
                        y: -90
                    }
                ),
                None
            );
            assert_eq!(
                button_hit(
                    &rects,
                    POINT {
                        x: -790,
                        y: rects[0].bottom
                    }
                ),
                None
            );
        }
    }
    #[test]
    fn maximized_corner_hit_areas_reach_the_screen_edges_without_overlapping() {
        let client = RECT {
            left: 0,
            top: 0,
            right: 1000,
            bottom: 600,
        };
        let pill = |index: i32| RECT {
            left: 865 + index * 44,
            top: 3,
            right: 865 + (index + 1) * 44,
            bottom: 29,
        };
        let mut rects = [pill(0), pill(1), pill(2)];
        let painted = rects;
        reach_corner(&mut rects, &client);
        assert!(rects.iter().all(|rect| rect.top == 0 && rect.bottom == 29));
        assert_eq!(rects[2].right, 1000);
        for index in 0..2 {
            assert_eq!(rects[index].left, painted[index].left);
            assert_eq!(rects[index].right, painted[index].right);
        }
        assert_eq!(rects[2].left, painted[2].left);
        // The top-right pixel is Close; one past the last pill no longer misses.
        assert_eq!(button_hit(&rects, POINT { x: 999, y: 0 }), Some(2));
        assert_eq!(button_hit(&rects, POINT { x: 900, y: 0 }), Some(0));
        assert_eq!(button_hit(&rects, POINT { x: 940, y: 1 }), Some(1));
        assert_eq!(button_hit(&rects, POINT { x: 999, y: 29 }), None);
        assert_eq!(button_hit(&rects, POINT { x: 864, y: 0 }), None);
    }
    #[test]
    fn system_menu_strings_become_plain_accessible_names() {
        assert_eq!(menu_label("&Restore"), "Restore");
        assert_eq!(menu_label("Mi&nimize"), "Minimize");
        assert_eq!(menu_label("Close\tAlt+F4"), "Close");
        assert_eq!(menu_label("&Close\tAlt+F4"), "Close");
        assert_eq!(menu_label("R&&D"), "R&D");
        assert_eq!(menu_label("最小化(&N)"), "最小化");
        assert_eq!(menu_label("Minimiser (&N)"), "Minimiser");
        assert_eq!(menu_label("(&"), "(");
        assert_eq!(menu_label("Plain (text)"), "Plain (text)");
        assert_eq!(menu_label(" \t"), "");
    }
    #[test]
    fn keyboard_travel_skips_withdrawn_buttons_and_wraps() {
        let all = [true; 3];
        assert_eq!(next_enabled(all, 0, true), Some(1));
        assert_eq!(next_enabled(all, 2, true), Some(0));
        assert_eq!(next_enabled(all, 0, false), Some(2));
        assert_eq!(next_enabled(all, 1, false), Some(0));
        let no_maximize = [true, false, true];
        assert_eq!(next_enabled(no_maximize, 0, true), Some(2));
        assert_eq!(next_enabled(no_maximize, 2, false), Some(0));
        let only_close = [false, false, true];
        assert_eq!(next_enabled(only_close, 2, true), None);
        assert_eq!(next_enabled(only_close, 2, false), None);
    }
    #[test]
    fn the_system_menu_offers_what_the_window_can_do() {
        let all = WS_MINIMIZEBOX.0 | WS_MAXIMIZEBOX.0 | WS_THICKFRAME.0;
        let offered = |style, zoomed| system_menu_items(style, zoomed).map(|(_, enabled)| enabled);
        // Restore, Move, Size, Minimize, Maximize.
        assert_eq!(offered(all, false), [false, true, true, true, true]);
        assert_eq!(offered(all, true), [true, false, false, true, false]);
        assert_eq!(
            offered(WS_THICKFRAME.0, false),
            [false, true, true, false, false]
        );
        assert_eq!(offered(0, false), [false, true, false, false, false]);
        assert_eq!(system_menu_items(all, false)[0].0, SC_RESTORE);
    }
    #[test]
    fn the_snap_flyout_is_expected_below_and_around_maximize() {
        let maximize = RECT {
            left: 900,
            top: 3,
            right: 944,
            bottom: 29,
        };
        assert!(within_snap_reach(POINT { x: 920, y: 29 }, &maximize, 500));
        assert!(within_snap_reach(POINT { x: 700, y: 400 }, &maximize, 500));
        assert!(within_snap_reach(POINT { x: 1400, y: 528 }, &maximize, 500));
        assert!(!within_snap_reach(POINT { x: 920, y: 28 }, &maximize, 500));
        assert!(!within_snap_reach(POINT { x: 920, y: 529 }, &maximize, 500));
        assert!(!within_snap_reach(POINT { x: 399, y: 100 }, &maximize, 500));
        assert!(!within_snap_reach(
            POINT { x: 1444, y: 100 },
            &maximize,
            500
        ));
    }
    #[test]
    fn minimize_and_maximize_follow_the_window_styles() {
        let both = WS_MINIMIZEBOX.0 | WS_MAXIMIZEBOX.0;
        assert!((0..3).all(|index| button_enabled(index, both, false)));
        assert!(!button_enabled(0, WS_MAXIMIZEBOX.0, false));
        assert!(!button_enabled(1, WS_MINIMIZEBOX.0, false));
        assert!(button_enabled(2, 0, false));
        // Onboarding clears both; a window already maximized can still restore.
        assert!(!button_enabled(0, 0, false));
        assert!(!button_enabled(1, 0, false));
        assert!(button_enabled(1, 0, true));
    }
    #[test]
    fn a_held_reveal_waits_for_the_pointer_to_leave_the_drag_rectangle() {
        let origin = POINT { x: 100, y: 0 };
        let near = POINT { x: 102, y: 3 };
        assert!(!dragged_beyond(origin, origin, 4, 4));
        assert!(!dragged_beyond(origin, near, 4, 4));
        assert!(dragged_beyond(origin, POINT { x: 104, y: 0 }, 4, 4));
        assert!(dragged_beyond(origin, POINT { x: 100, y: 4 }, 4, 4));
        assert!(dragged_beyond(origin, POINT { x: 96, y: 0 }, 4, 4));
    }
    #[test]
    fn top_resize_band_precedes_reveal_and_preserves_corners() {
        for border in [2, 3, 4, 8, 12] {
            assert_eq!(top_frame_hit(200, 0, 800, border, 16, None), Some(HTTOP));
            assert_eq!(
                top_frame_hit(200, border - 1, 800, border, 16, None),
                Some(HTTOP)
            );
            assert_eq!(top_frame_hit(200, border, 800, border, 16, None), None);
            assert_eq!(top_frame_hit(1, 1, 800, border, 16, None), Some(HTTOPLEFT));
            assert_eq!(
                top_frame_hit(799, 1, 800, border, 16, None),
                Some(HTTOPRIGHT)
            );
        }
        assert_eq!(top_frame_hit(200, 0, 800, 0, 16, None), None);
        assert_eq!(top_frame_hit(-1, 0, 800, 8, 16, None), None);
        assert_eq!(top_frame_hit(800, 0, 800, 8, 16, None), None);
    }
    #[test]
    fn revealed_buttons_keep_their_column_of_the_top_band() {
        // 3 buttons of 44 DIP and two paddings end at the client's right edge.
        let controls = Some((662, 800));
        assert_eq!(top_frame_hit(661, 0, 800, 8, 16, controls), Some(HTTOP));
        assert_eq!(top_frame_hit(662, 0, 800, 8, 16, controls), None);
        // The top of a caption button (3 DIP inset) is never a resize target.
        assert_eq!(top_frame_hit(760, 3, 800, 8, 16, controls), None);
        assert_eq!(top_frame_hit(799, 0, 800, 8, 16, controls), None);
        assert_eq!(top_frame_hit(10, 7, 800, 8, 16, controls), Some(HTTOPLEFT));
    }
}
