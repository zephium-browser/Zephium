//! Conceals intermediate host/controller geometry during element fullscreen.
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::{Rc, Weak};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use webview2_com::ExecuteScriptCompletedHandler;
use webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2Controller;

use windows::core::{BOOL, PCWSTR};
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    ClientToScreen, GetMonitorInfoW, GetStockObject, MonitorFromWindow, BLACK_BRUSH, HBRUSH,
    MONITORINFO, MONITOR_DEFAULTTONEAREST,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Shell::{
    DefSubclassProc, GetWindowSubclass, RemoveWindowSubclass, SetWindowSubclass,
};
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::fullscreen::{window_settled, ScreenRect, WindowPlacement};

thread_local! {
    static TIMERS: RefCell<HashMap<usize, Weak<RefCell<State>>>> = RefCell::new(HashMap::new());
}

struct State {
    cover: Option<HWND>,
    parent: HWND,
    timer: usize,
    began: Instant,
    changed: Instant,
    fading: Option<Instant>,
    placement: WindowPlacement,
    animate: bool,
    controller: Option<ICoreWebView2Controller>,
    entering: bool,
    restore: Option<ScreenRect>,
    layout_seen: bool,
    epoch: u64,
    pending: bool,
    sampled: Instant,
    sample: Option<String>,
    stable: Instant,
    page_ready: bool,
}

const MAX_HOLD: Duration = Duration::from_secs(4);
const FADE: Duration = Duration::from_millis(180);
const SAMPLE_INTERVAL: Duration = Duration::from_millis(64);
const PAGE_SETTLE: Duration = Duration::from_millis(200);
// A visual hint only. No page data is returned and this cannot grant a view
// permission to display. Native geometry and the stage's admission remain owners.
const SAMPLE: &str = r#"(() => {
 const videos = Array.from(document.querySelectorAll('video')).slice(0,32)
   .map(v => { const r=v.getBoundingClientRect(); return [r.x,r.y,r.width,r.height].map(Math.round); })
   .filter(r => r[2]>0 && r[3]>0).slice(0,8);
 return [Math.round(innerWidth*devicePixelRatio), Math.round(innerHeight*devicePixelRatio),
   !!document.fullscreenElement, videos];
})()"#;

impl State {
    fn detach(&mut self) -> (HWND, Option<HWND>, usize, Option<ICoreWebView2Controller>) {
        (
            self.parent,
            self.cover.take(),
            std::mem::take(&mut self.timer),
            self.controller.take(),
        )
    }
}

fn release(
    (parent, cover, timer, controller): (
        HWND,
        Option<HWND>,
        usize,
        Option<ICoreWebView2Controller>,
    ),
) {
    if timer != 0 {
        let _ = unsafe { KillTimer(None, timer) };
        let _ = TIMERS.try_with(|timers| timers.borrow_mut().remove(&timer));
    }
    if let Some(cover) = cover {
        unsafe {
            let _ = RemoveWindowSubclass(parent, Some(parent_proc), cover.0 as usize);
            let _ = DestroyWindow(cover);
        }
    }
    drop(controller);
}

fn close(state: &Rc<RefCell<State>>) {
    let native = state.borrow_mut().detach();
    release(native);
}

impl Drop for State {
    fn drop(&mut self) {
        release(self.detach());
    }
}

#[derive(Clone)]
pub(super) struct FullscreenTransition(Rc<RefCell<State>>);

impl FullscreenTransition {
    pub(super) fn new(
        parent: HWND,
        controller: Option<ICoreWebView2Controller>,
        entering: bool,
        restore: Option<ScreenRect>,
    ) -> Option<Self> {
        let placement = placement(parent)?;
        // Owned popups support native alpha without the compatibility-manifest
        // and hit-testing problems of layered child windows. Follow owner resize
        // synchronously, allocating only its current client area.
        let module = unsafe { GetModuleHandleW(None) }.unwrap_or_default();
        let cover = unsafe {
            CreateWindowExW(
                WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW,
                cover_class(),
                PCWSTR::null(),
                WS_POPUP,
                0,
                0,
                0,
                0,
                Some(parent),
                None,
                Some(module.into()),
                None,
            )
        }
        .ok()?;
        let mut animation = BOOL(0);
        let animate = unsafe {
            SystemParametersInfoW(
                SPI_GETCLIENTAREAANIMATION,
                0,
                Some((&mut animation as *mut BOOL).cast()),
                SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
            )
        }
        .is_ok()
            && animation.as_bool();
        if unsafe { SetLayeredWindowAttributes(cover, COLORREF(0), 255, LWA_ALPHA) }.is_err() {
            let _ = unsafe { DestroyWindow(cover) };
            return None;
        }
        let now = Instant::now();
        let state = Rc::new(RefCell::new(State {
            cover: Some(cover),
            parent,
            timer: 0,
            began: now,
            changed: now,
            fading: None,
            placement,
            animate,
            controller,
            entering,
            restore,
            layout_seen: false,
            epoch: 0,
            pending: false,
            sampled: now,
            sample: None,
            stable: now,
            page_ready: false,
        }));
        let timer = unsafe { SetTimer(None, 0, 16, Some(tick)) };
        if timer == 0 {
            return None;
        }
        state.borrow_mut().timer = timer;
        TIMERS.with(|timers| timers.borrow_mut().insert(timer, Rc::downgrade(&state)));
        if !unsafe { SetWindowSubclass(parent, Some(parent_proc), cover.0 as usize, 0) }.as_bool() {
            return None;
        }
        let transition = Self(state);
        transition.raise();
        Some(transition)
    }

    pub(super) fn raise(&self) {
        let native = self
            .0
            .try_borrow()
            .ok()
            .and_then(|state| state.cover.map(|cover| (state.parent, cover)));
        if let Some((parent, cover)) = native {
            position(parent, cover);
        }
    }

    pub(super) fn active(&self) -> bool {
        self.0.borrow().cover.is_some()
    }

    pub(super) fn layout_changed(&self) {
        let mut state = self.0.borrow_mut();
        state.layout_seen = true;
        state.epoch = state.epoch.wrapping_add(1);
        state.sample = None;
        state.page_ready = false;
        state.fading = None;
        state.changed = Instant::now();
    }
}

fn screen_rect(rect: RECT) -> ScreenRect {
    ScreenRect {
        left: rect.left,
        top: rect.top,
        right: rect.right,
        bottom: rect.bottom,
    }
}

fn placement(window: HWND) -> Option<WindowPlacement> {
    let mut outer = RECT::default();
    let mut client = RECT::default();
    let mut origin = POINT::default();
    unsafe {
        GetWindowRect(window, &mut outer).ok()?;
        GetClientRect(window, &mut client).ok()?;
        ClientToScreen(window, &mut origin)
            .as_bool()
            .then_some(())?;
    }
    Some(WindowPlacement {
        window: screen_rect(outer),
        client: ScreenRect {
            left: origin.x,
            top: origin.y,
            right: origin.x + client.right,
            bottom: origin.y + client.bottom,
        },
    })
}

/// The area the shell draws in, which is what the window has to return to
/// after a page has had the screen.
pub(super) fn client_rect(window: HWND) -> Option<ScreenRect> {
    placement(window).map(|placement| placement.client)
}

fn window_ready(
    parent: HWND,
    placement: WindowPlacement,
    entering: bool,
    restore: Option<ScreenRect>,
) -> bool {
    let monitor = unsafe { MonitorFromWindow(parent, MONITOR_DEFAULTTONEAREST) };
    let mut info = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    unsafe { GetMonitorInfoW(monitor, &mut info) }.as_bool()
        && window_settled(placement, screen_rect(info.rcMonitor), entering, restore)
}

fn sample_page(owner: &Rc<RefCell<State>>) {
    let request = {
        let mut state = owner.borrow_mut();
        if state.cover.is_none()
            || state.pending
            || state.fading.is_some()
            || !state.layout_seen
            || state.sampled.elapsed() < SAMPLE_INTERVAL
        {
            return;
        }
        let Some(controller) = state.controller.clone() else {
            return;
        };
        state.pending = true;
        state.sampled = Instant::now();
        (controller, state.epoch, state.entering, state.parent)
    };
    let weak = Rc::downgrade(owner);
    let (controller, epoch, entering, parent) = request;
    let mut bounds = RECT::default();
    let mut client = RECT::default();
    let core = unsafe {
        controller
            .Bounds(&mut bounds)
            .and_then(|_| controller.CoreWebView2())
    };
    let covers_window = !entering
        || (unsafe { GetClientRect(parent, &mut client) }.is_ok()
            && bounds.right - bounds.left == client.right
            && bounds.bottom - bounds.top == client.bottom);
    let done = ExecuteScriptCompletedHandler::create(Box::new(move |status, reply| {
        if let Some(owner) = weak.upgrade() {
            let mut state = owner.borrow_mut();
            state.pending = false;
            if state.cover.is_none() || epoch != state.epoch {
                return Ok(());
            }
            let valid = status.is_ok() && covers_window && sample_matches(&reply, bounds, entering);
            let now = Instant::now();
            if !valid || state.sample.as_deref() != Some(&reply) {
                state.sample = valid.then_some(reply);
                state.stable = now;
                state.page_ready = false;
            } else {
                state.page_ready = now.duration_since(state.stable) >= PAGE_SETTLE;
            }
        }
        Ok(())
    }));
    if core
        .and_then(|core| unsafe {
            core.ExecuteScript(&windows::core::HSTRING::from(SAMPLE), &done)
        })
        .is_err()
    {
        owner.borrow_mut().pending = false;
    }
}

fn sample_matches(reply: &str, bounds: RECT, entering: bool) -> bool {
    if reply.len() > 4096 {
        return false;
    }
    let Ok(value) = serde_json::from_str::<serde_json::Value>(reply) else {
        return false;
    };
    let Some(parts) = value.as_array().filter(|parts| parts.len() == 4) else {
        return false;
    };
    parts[0]
        .as_i64()
        .is_some_and(|width| (width - i64::from(bounds.right - bounds.left)).abs() <= 2)
        && parts[1]
            .as_i64()
            .is_some_and(|height| (height - i64::from(bounds.bottom - bounds.top)).abs() <= 2)
        && parts[2].as_bool() == Some(entering)
        && parts[3].as_array().is_some_and(|videos| videos.len() <= 8)
}

// The cover is black by its class, not by a control style: a static
// rectangle takes the system's window-frame colour, which is grey on
// current Windows.
fn cover_class() -> PCWSTR {
    static NAME: OnceLock<Vec<u16>> = OnceLock::new();
    let name = NAME.get_or_init(|| {
        let name = "ZephiumFullscreenCover"
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect::<Vec<_>>();
        let class = WNDCLASSW {
            lpfnWndProc: Some(cover_proc),
            lpszClassName: PCWSTR(name.as_ptr()),
            hInstance: unsafe { GetModuleHandleW(None) }.unwrap_or_default().into(),
            hbrBackground: HBRUSH(unsafe { GetStockObject(BLACK_BRUSH) }.0),
            ..Default::default()
        };
        unsafe { RegisterClassW(&class) };
        name
    });
    PCWSTR(name.as_ptr())
}

unsafe extern "system" fn cover_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

fn position(parent: HWND, cover: HWND) {
    let mut rect = RECT::default();
    let mut origin = POINT::default();
    unsafe {
        if !IsWindowVisible(parent).as_bool()
            || IsIconic(parent).as_bool()
            || GetClientRect(parent, &mut rect).is_err()
            || !ClientToScreen(parent, &mut origin).as_bool()
        {
            let _ = ShowWindow(cover, SW_HIDE);
            return;
        }
        let _ = SetWindowPos(
            cover,
            Some(HWND_TOP),
            origin.x,
            origin.y,
            rect.right,
            rect.bottom,
            SWP_NOACTIVATE | SWP_SHOWWINDOW,
        );
    }
}

unsafe extern "system" fn parent_proc(
    parent: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    id: usize,
    _: usize,
) -> LRESULT {
    if msg == WM_DESTROY {
        let state = TIMERS.with(|timers| {
            timers
                .borrow()
                .values()
                .filter_map(Weak::upgrade)
                .find(|state| {
                    state.try_borrow().is_ok_and(|state| {
                        state.parent == parent
                            && state.cover.is_some_and(|cover| cover.0 as usize == id)
                    })
                })
        });
        if let Some(state) = state {
            close(&state);
        }
    }
    let result = unsafe { DefSubclassProc(parent, msg, wparam, lparam) };
    if matches!(msg, WM_SIZE | WM_MOVE | WM_WINDOWPOSCHANGED | WM_DPICHANGED)
        && unsafe { GetWindowSubclass(parent, Some(parent_proc), id, None) }.as_bool()
    {
        position(parent, HWND(id as *mut _));
    }
    result
}

unsafe extern "system" fn tick(_: HWND, _: u32, timer: usize, _: u32) {
    let state = TIMERS.with(|timers| timers.borrow().get(&timer).and_then(Weak::upgrade));
    let Some(owner) = state else {
        let _ = unsafe { KillTimer(None, timer) };
        return;
    };
    let Ok(current) = owner.try_borrow() else {
        return;
    };
    let parent = current.parent;
    drop(current);
    let current = placement(parent);
    let (cover, alpha) = {
        let Ok(mut state) = owner.try_borrow_mut() else {
            return;
        };
        let Some(current) = current else {
            drop(state);
            close(&owner);
            return;
        };
        let now = Instant::now();
        if state.placement != current {
            state.placement = current;
            state.changed = now;
            state.fading = None;
            state.epoch = state.epoch.wrapping_add(1);
            state.sample = None;
            state.page_ready = false;
        }
        let elapsed = now.duration_since(state.began);
        // A crashed renderer or missing callback must not leave the browser
        // black. This is a failure deadline, never the normal reveal trigger.
        if elapsed >= MAX_HOLD + FADE + Duration::from_millis(100) {
            drop(state);
            close(&owner);
            return;
        }
        if state.fading.is_none()
            && (elapsed >= MAX_HOLD
                || (state.page_ready
                    && state.layout_seen
                    && window_ready(parent, current, state.entering, state.restore)
                    && now.duration_since(state.changed) >= PAGE_SETTLE))
        {
            state.fading = Some(now);
        }
        let alpha = match state.fading {
            None => 255,
            Some(_) if !state.animate => 0,
            Some(start) => {
                let fraction =
                    (now.duration_since(start).as_secs_f64() / FADE.as_secs_f64()).min(1.0);
                (255.0 * (1.0 - fraction * fraction * (3.0 - 2.0 * fraction))) as u8
            }
        };
        if alpha == 0 {
            drop(state);
            close(&owner);
            return;
        }
        (state.cover, alpha)
    };
    if let Some(cover) = cover {
        let _ = unsafe { SetLayeredWindowAttributes(cover, COLORREF(0), alpha, LWA_ALPHA) };
    }
    sample_page(&owner);
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::core::w;

    #[test]
    fn page_sample_requires_current_viewport_and_fullscreen_state() {
        let bounds = RECT {
            left: 0,
            top: 0,
            right: 1200,
            bottom: 800,
        };
        assert!(sample_matches(
            "[1200,800,true,[[0,0,1200,800]]]",
            bounds,
            true
        ));
        for stale in [
            "null",
            "[800,600,true,[]]",
            "[1200,800,false,[]]",
            "[1200,800,true]",
        ] {
            assert!(!sample_matches(stale, bounds, true));
        }
        assert!(sample_matches("[1200,800,false,[]]", bounds, false));
    }

    #[test]
    fn fullscreen_cover_expires_and_replacement_releases_native_resources() {
        struct Host(HWND);
        impl Drop for Host {
            fn drop(&mut self) {
                let _ = unsafe { DestroyWindow(self.0) };
            }
        }
        let parent = Host(unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("STATIC"),
                w!(""),
                WS_OVERLAPPEDWINDOW,
                0,
                0,
                800,
                600,
                None,
                None,
                None,
                None,
            )
            .unwrap()
        });
        let placed = placement(parent.0).unwrap();
        assert!(
            placed.window.left <= placed.client.left
                && placed.window.top <= placed.client.top
                && placed.window.right >= placed.client.right
                && placed.window.bottom >= placed.client.bottom
        );
        let first = FullscreenTransition::new(parent.0, None, true, None).unwrap();
        let first_cover = first.0.borrow().cover.unwrap();
        assert!(unsafe { IsWindow(Some(first_cover)) }.as_bool());
        assert_eq!(
            unsafe { GetClassLongPtrW(first_cover, GCLP_HBRBACKGROUND) },
            unsafe { GetStockObject(BLACK_BRUSH) }.0 as usize,
            "the cover is black, not the system window-frame colour"
        );
        let second = FullscreenTransition::new(parent.0, None, true, None).unwrap();
        drop(first);
        assert!(!unsafe { IsWindow(Some(first_cover)) }.as_bool());
        let second_cover = second.0.borrow().cover.unwrap();
        second.0.borrow_mut().began = Instant::now() - Duration::from_millis(700);
        let timer = second.0.borrow().timer;
        unsafe { tick(HWND::default(), 0, timer, 0) };
        assert!(
            second.active(),
            "an elapsed cosmetic delay is not page readiness"
        );
        second.0.borrow_mut().began = Instant::now() - MAX_HOLD;
        let deadline = Instant::now() + Duration::from_secs(2);
        while second.0.borrow().cover.is_some() {
            assert!(Instant::now() < deadline, "fullscreen cover did not expire");
            unsafe {
                let mut message = MSG::default();
                while PeekMessageW(&mut message, None, 0, 0, PM_REMOVE).as_bool() {
                    let _ = TranslateMessage(&message);
                    DispatchMessageW(&message);
                }
                MsgWaitForMultipleObjectsEx(None, 16, QS_ALLINPUT, MWMO_INPUTAVAILABLE);
            }
        }
        assert!(!unsafe { IsWindow(Some(second_cover)) }.as_bool());
        assert!(TIMERS.with(|timers| timers.borrow().is_empty()));
        let closing = FullscreenTransition::new(parent.0, None, false, None).unwrap();
        let closing_cover = closing.0.borrow().cover.unwrap();
        drop(parent);
        assert!(closing.0.borrow().cover.is_none());
        assert!(!unsafe { IsWindow(Some(closing_cover)) }.as_bool());
        assert!(TIMERS.with(|timers| timers.borrow().is_empty()));
    }
}
