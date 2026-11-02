//! A hidden sidebar waits beneath the page, so the page's own view takes
//! every pointer move along the window's left edge and the chrome never sees
//! the pointer arrive. Reading the moves app-wide tells the chrome the moment
//! the pointer reaches the edge, or is plainly about to.

use std::cell::RefCell;
use std::ptr::NonNull;

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_app_kit::{NSEvent, NSEventMask, NSWindow};
use objc2_foundation::MainThreadMarker;
use tauri::{Manager, WebviewWindow};

use crate::edge_reach::at_edge;

struct Edge {
    window: WebviewWindow,
    number: isize,
    near: bool,
    last: Option<f64>,
}

thread_local! {
    static EDGES: RefCell<Vec<Edge>> = const { RefCell::new(Vec::new()) };
    static MONITOR: RefCell<Option<Retained<AnyObject>>> = const { RefCell::new(None) };
}

fn observe(event: &NSEvent) {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let Some(window) = event.window(mtm) else {
        return;
    };
    let number = window.windowNumber();
    let x = event.locationInWindow().x;
    let changed = EDGES.with(|edges| {
        let mut edges = edges.try_borrow_mut().ok()?;
        let edge = edges.iter_mut().find(|edge| edge.number == number)?;
        let step = edge.last.map_or(0.0, |last| x - last);
        edge.last = Some(x);
        let near = at_edge(x, step, edge.near);
        (near != edge.near).then(|| {
            edge.near = near;
            (edge.window.clone(), near)
        })
    });
    if let Some((window, near)) = changed {
        crate::emit_to_privileged(
            window.app_handle(),
            window.label(),
            "zephium:sidebar-edge",
            &serde_json::json!({ "near": near }),
        );
    }
}

/// Starts or stops watching `window`'s left edge. Runs on the main thread.
pub fn watch(window: &WebviewWindow, on: bool) {
    if MainThreadMarker::new().is_none() {
        return;
    }
    let Ok(pointer) = window.ns_window() else {
        return;
    };
    // SAFETY: Tao's live NSWindow, borrowed for this main-thread call only.
    let number = unsafe { &*(pointer as *const NSWindow) }.windowNumber();
    let watching = EDGES.with(|edges| {
        let mut edges = edges.borrow_mut();
        edges.retain(|edge| edge.number != number);
        if on {
            edges.push(Edge {
                window: window.clone(),
                number,
                near: false,
                last: None,
            });
        }
        !edges.is_empty()
    });
    MONITOR.with(|monitor| {
        let mut monitor = monitor.borrow_mut();
        match (watching, monitor.is_some()) {
            (true, false) => {
                let handler = RcBlock::new(|event: NonNull<NSEvent>| -> *mut NSEvent {
                    // SAFETY: the local monitor lends its event for this call.
                    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        observe(unsafe { event.as_ref() })
                    }));
                    event.as_ptr()
                });
                // SAFETY: an app-local main-thread monitor that hands every
                // event back untouched.
                *monitor = unsafe {
                    NSEvent::addLocalMonitorForEventsMatchingMask_handler(
                        NSEventMask::MouseMoved,
                        &handler,
                    )
                };
            }
            (false, true) => {
                if let Some(old) = monitor.take() {
                    // SAFETY: this exact registration is removed once.
                    unsafe { NSEvent::removeMonitor(&old) };
                }
            }
            _ => {}
        }
    });
}
