//! A hidden sidebar waits beneath the page, and the page's WebView2 windows
//! belong to another process, so this thread never sees a pointer move over
//! them. While a window's sidebar is hidden its moves are followed through
//! the shared pointer watch instead, and the chrome is told the moment the
//! pointer reaches the window's left edge, or is plainly about to, and
//! whether it is over the card the column comes out as.

use std::cell::RefCell;
use std::collections::HashMap;

use tauri::{Manager, WebviewWindow};
use windows::Win32::Foundation::{HWND, POINT, RECT};
use windows::Win32::Graphics::Gdi::ScreenToClient;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::WindowsAndMessaging::{
    GetAncestor, GetClientRect, GetCursorPos, GetForegroundWindow, IsIconic, IsWindow,
    WindowFromPoint, GA_ROOTOWNER,
};

use super::pointer;
use crate::edge_reach::at_edge;
use zephium_core::layout::HIDDEN_CARD_INSET;

struct Edge {
    window: WebviewWindow,
    hwnd: HWND,
    near: bool,
    over: bool,
    last: Option<f64>,
}

thread_local! {
    static EDGES: RefCell<Vec<Edge>> = const { RefCell::new(Vec::new()) };
    /// Each window's column width, which the card stands round.
    static WIDTHS: RefCell<HashMap<String, f64>> = RefCell::new(HashMap::new());
}

/// The column is this wide, wherever it stands. Runs on the UI thread.
pub fn column_width(window: &WebviewWindow, width: f64) {
    WIDTHS.with(|widths| widths.borrow_mut().insert(window.label().to_owned(), width));
}

/// Starts or stops watching `window`'s left edge. Runs on the UI thread.
pub fn watch(window: &WebviewWindow, on: bool) {
    let Ok(hwnd) = window.hwnd() else {
        return;
    };
    let hwnd = HWND(hwnd.0);
    EDGES.with(|edges| {
        let mut edges = edges.borrow_mut();
        edges.retain(|edge| edge.hwnd != hwnd);
        if on {
            edges.push(Edge {
                window: window.clone(),
                hwnd,
                near: false,
                over: false,
                last: None,
            });
        }
    });
    if on {
        pointer::listen(hwnd, moved);
    } else {
        pointer::forget(hwnd, moved);
    }
}

fn moved(_: HWND) {
    let mut cursor = POINT::default();
    // SAFETY: a plain read of the pointer's screen position.
    let cursor = unsafe { GetCursorPos(&mut cursor) }.ok().map(|()| cursor);
    // SAFETY: a plain read of the window in front.
    let front = unsafe { GetForegroundWindow() };
    let mut changes = Vec::new();
    EDGES.with(|edges| {
        let Ok(mut edges) = edges.try_borrow_mut() else {
            return;
        };
        // SAFETY: a plain query of a handle value.
        edges.retain(|edge| unsafe { IsWindow(Some(edge.hwnd)) }.as_bool());
        for edge in edges.iter_mut() {
            let x = cursor
                .filter(|_| is_front(edge.hwnd, front))
                .and_then(|cursor| local_x(edge.hwnd, cursor));
            let (near, over) = match x {
                Some(x) => {
                    let step = edge.last.map_or(0.0, |last| x - last);
                    edge.last = Some(x);
                    let card = WIDTHS
                        .with(|widths| widths.borrow().get(edge.window.label()).copied())
                        .map_or(0.0, |width| width + 3.0 * HIDDEN_CARD_INSET);
                    (at_edge(x, step, edge.near), x <= card)
                }
                None => {
                    edge.last = None;
                    (false, false)
                }
            };
            if near != edge.near || over != edge.over {
                edge.near = near;
                edge.over = over;
                changes.push((edge.window.clone(), near, over));
            }
        }
    });
    for (window, near, over) in changes {
        crate::emit_to_privileged(
            window.app_handle(),
            window.label(),
            "zephium:sidebar-edge",
            &serde_json::json!({ "near": near, "over": over }),
        );
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
