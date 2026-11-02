//! A hidden sidebar waits beneath the page, and the page's WebView2 windows
//! belong to another process, so this thread never sees a pointer move over
//! them. While a window's sidebar is hidden the pointer is read instead, on a
//! short timer, and the chrome is told the moment it reaches the window's
//! left edge, or is plainly about to.
//!
//! A timer rather than a low-level mouse hook: the hook would be called for
//! every move on the desktop, in or out of this window, and the system drops
//! one that is slow to answer. The timer reads one position per tick, only
//! while a hidden sidebar's window is in front, and does nothing otherwise.

use std::cell::{Cell, RefCell};

use tauri::{Manager, WebviewWindow};
use windows::Win32::Foundation::{HWND, POINT, RECT};
use windows::Win32::Graphics::Gdi::ScreenToClient;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::WindowsAndMessaging::{
    GetAncestor, GetClientRect, GetCursorPos, GetForegroundWindow, IsIconic, IsWindow, KillTimer,
    SetTimer, WindowFromPoint, GA_ROOTOWNER,
};

use crate::edge_reach::at_edge;

/// About a display frame: the column is on its way before the pointer lands.
const WATCHING_MS: u32 = 16;
/// While no hidden sidebar's window is in front there is nothing to read,
/// only a moment to notice that one has come to the front.
const WAITING_MS: u32 = 250;

struct Edge {
    window: WebviewWindow,
    hwnd: HWND,
    near: bool,
    last: Option<f64>,
}

thread_local! {
    static EDGES: RefCell<Vec<Edge>> = const { RefCell::new(Vec::new()) };
    static TIMER: Cell<Option<(usize, u32)>> = const { Cell::new(None) };
}

/// Starts or stops watching `window`'s left edge. Runs on the UI thread.
pub fn watch(window: &WebviewWindow, on: bool) {
    let Ok(hwnd) = window.hwnd() else {
        return;
    };
    let hwnd = HWND(hwnd.0);
    let any = EDGES.with(|edges| {
        let mut edges = edges.borrow_mut();
        edges.retain(|edge| edge.hwnd != hwnd);
        if on {
            edges.push(Edge {
                window: window.clone(),
                hwnd,
                near: false,
                last: None,
            });
        }
        !edges.is_empty()
    });
    if any {
        arm(WATCHING_MS);
    } else {
        disarm();
    }
}

fn arm(interval: u32) {
    let (id, current) = TIMER.get().unwrap_or((0, 0));
    if current == interval {
        return;
    }
    // SAFETY: a window-less timer of this thread, re-armed in place by its
    // own id; the callback runs on this thread's message loop.
    let id = unsafe { SetTimer(None, id, interval, Some(tick)) };
    TIMER.set((id != 0).then_some((id, interval)));
}

fn disarm() {
    if let Some((id, _)) = TIMER.take() {
        // SAFETY: kills the timer this thread created above.
        let _ = unsafe { KillTimer(None, id) };
    }
}

unsafe extern "system" fn tick(_: HWND, _: u32, _: usize, _: u32) {
    let _ = std::panic::catch_unwind(read);
}

fn read() {
    let mut cursor = POINT::default();
    // SAFETY: a plain read of the pointer's screen position.
    let cursor = unsafe { GetCursorPos(&mut cursor) }.ok().map(|()| cursor);
    // SAFETY: a plain read of the window in front.
    let front = unsafe { GetForegroundWindow() };
    let mut changes = Vec::new();
    let mut watching = false;
    EDGES.with(|edges| {
        let Ok(mut edges) = edges.try_borrow_mut() else {
            return;
        };
        // SAFETY: a plain query of a handle value.
        edges.retain(|edge| unsafe { IsWindow(Some(edge.hwnd)) }.as_bool());
        for edge in edges.iter_mut() {
            let in_front = is_front(edge.hwnd, front);
            watching |= in_front;
            let x = cursor
                .filter(|_| in_front)
                .and_then(|cursor| local_x(edge.hwnd, cursor));
            let near = match x {
                Some(x) => {
                    let step = edge.last.map_or(0.0, |last| x - last);
                    edge.last = Some(x);
                    at_edge(x, step, edge.near)
                }
                None => {
                    edge.last = None;
                    false
                }
            };
            if near != edge.near {
                edge.near = near;
                changes.push((edge.window.clone(), near));
            }
        }
    });
    for (window, near) in changes {
        crate::emit_to_privileged(
            window.app_handle(),
            window.label(),
            "zephium:sidebar-edge",
            &serde_json::json!({ "near": near }),
        );
    }
    let any = EDGES.with(|edges| edges.try_borrow().is_ok_and(|edges| !edges.is_empty()));
    if any {
        arm(if watching { WATCHING_MS } else { WAITING_MS });
    } else {
        disarm();
    }
}

fn is_front(window: HWND, front: HWND) -> bool {
    // SAFETY: plain reads of live windows' owners.
    unsafe {
        !front.0.is_null()
            && GetAncestor(front, GA_ROOTOWNER) == window
            && !IsIconic(window).as_bool()
    }
}

/// The pointer's distance from the window's left edge in DIPs, when it is
/// over the window itself and not another one laid over it.
fn local_x(window: HWND, cursor: POINT) -> Option<f64> {
    let mut at = cursor;
    let mut client = RECT::default();
    // SAFETY: plain reads of a live window's client area and DPI, and of the
    // window under the pointer.
    unsafe {
        if !ScreenToClient(window, &mut at).as_bool()
            || GetClientRect(window, &mut client).is_err()
            || GetAncestor(WindowFromPoint(cursor), GA_ROOTOWNER) != window
        {
            return None;
        }
        let dpi = GetDpiForWindow(window);
        (at.x >= 0 && at.y >= 0 && at.x < client.right && at.y < client.bottom && dpi != 0)
            .then(|| f64::from(at.x) * 96.0 / f64::from(dpi))
    }
}
