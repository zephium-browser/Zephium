//! A hidden sidebar waits beneath the page, so the page's own view takes
//! every pointer move along the window's left edge and the chrome never sees
//! the pointer arrive. A tracking area across the whole window follows the
//! pointer instead: always, whichever window has the keyboard and whichever
//! view is on top, so coming back from another app needs no click first. The
//! chrome is told the moment the pointer reaches the edge, or is plainly
//! about to, and whether it is over the card the column comes out as.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::ptr::null_mut;

use objc2::rc::Retained;
use objc2::{define_class, msg_send, AllocAnyThread, DefinedClass, MainThreadOnly};
use objc2_app_kit::{
    NSAutoresizingMaskOptions, NSEvent, NSResponder, NSTrackingArea, NSTrackingAreaOptions, NSView,
    NSWindow, NSWindowOrderingMode,
};
use objc2_foundation::{MainThreadMarker, NSObject, NSPoint, NSPointInRect, NSRect};
use tauri::{Manager, WebviewWindow};
use zephium_core::layout::HIDDEN_CARD_INSET;

use crate::edge_reach::at_edge;

pub struct Ivars {
    window: WebviewWindow,
    near: Cell<bool>,
    over: Cell<bool>,
    last: Cell<Option<f64>>,
}

define_class!(
    #[unsafe(super(NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "ZephiumSidebarEdge"]
    #[ivars = Ivars]
    pub struct Edge;

    impl Edge {
        /// Never a target: it only listens.
        #[unsafe(method(hitTest:))]
        fn hit_test(&self, _point: NSPoint) -> *mut NSView {
            null_mut()
        }

        #[unsafe(method(mouseMoved:))]
        fn mouse_moved(&self, event: &NSEvent) {
            self.follow(Some(event.locationInWindow().x));
        }

        #[unsafe(method(mouseEntered:))]
        fn mouse_entered(&self, event: &NSEvent) {
            self.ivars().last.set(None);
            self.follow(Some(event.locationInWindow().x));
        }

        #[unsafe(method(mouseExited:))]
        fn mouse_exited(&self, _event: &NSEvent) {
            self.follow(None);
        }
    }
);

thread_local! {
    static EDGES: RefCell<Vec<(isize, Retained<Edge>)>> = const { RefCell::new(Vec::new()) };
    /// Each window's column width, which the card stands round.
    static WIDTHS: RefCell<HashMap<String, f64>> = RefCell::new(HashMap::new());
}

impl Edge {
    fn new(mtm: MainThreadMarker, window: &WebviewWindow, frame: NSRect) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(Ivars {
            window: window.clone(),
            near: Cell::new(false),
            over: Cell::new(false),
            last: Cell::new(None),
        });
        // SAFETY: the designated initializer of NSView.
        let this: Retained<Self> = unsafe { msg_send![super(this), initWithFrame: frame] };
        // SAFETY: the owner is this view, which outlives its own tracking
        // area; no user info is attached.
        let area = unsafe {
            NSTrackingArea::initWithRect_options_owner_userInfo(
                NSTrackingArea::alloc(),
                NSRect::ZERO,
                NSTrackingAreaOptions::MouseMoved
                    | NSTrackingAreaOptions::MouseEnteredAndExited
                    | NSTrackingAreaOptions::ActiveAlways
                    | NSTrackingAreaOptions::InVisibleRect,
                Some(this.as_ref()),
                None,
            )
        };
        this.addTrackingArea(&area);
        this
    }

    /// The pointer is at `x` in the window, or has left it.
    fn follow(&self, x: Option<f64>) {
        let ivars = self.ivars();
        let (near, over) = match x {
            Some(x) => {
                let step = ivars.last.get().map_or(0.0, |last| x - last);
                ivars.last.set(Some(x));
                let card = WIDTHS
                    .with(|widths| widths.borrow().get(ivars.window.label()).copied())
                    .map_or(0.0, |width| width + 3.0 * HIDDEN_CARD_INSET);
                (at_edge(x, step, ivars.near.get()), x <= card)
            }
            None => {
                ivars.last.set(None);
                (false, false)
            }
        };
        if near == ivars.near.get() && over == ivars.over.get() {
            return;
        }
        ivars.near.set(near);
        ivars.over.set(over);
        crate::emit_to_privileged(
            ivars.window.app_handle(),
            ivars.window.label(),
            "zephium:sidebar-edge",
            &serde_json::json!({ "near": near, "over": over }),
        );
    }
}

/// The column is this wide, wherever it stands. Runs on the main thread.
pub fn column_width(window: &WebviewWindow, width: f64) {
    WIDTHS.with(|widths| widths.borrow_mut().insert(window.label().to_owned(), width));
}

/// Starts or stops watching `window`'s left edge. Runs on the main thread.
pub fn watch(window: &WebviewWindow, on: bool) {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let Ok(pointer) = window.ns_window() else {
        return;
    };
    // SAFETY: Tao's live NSWindow, borrowed for this main-thread call only.
    let ns_window = unsafe { &*(pointer as *const NSWindow) };
    let number = ns_window.windowNumber();
    let old = EDGES.with(|edges| {
        let mut edges = edges.borrow_mut();
        let index = edges.iter().position(|(owner, _)| *owner == number);
        index.map(|index| edges.remove(index).1)
    });
    if let Some(old) = old {
        old.removeFromSuperview();
    }
    if !on {
        return;
    }
    // Laid in the window's frame view, beneath everything, across all of it:
    // the content's own views are reordered as pages come and go.
    let Some(content) = ns_window.contentView() else {
        return;
    };
    // SAFETY: a main-thread read of the content view's parent.
    let Some(frame_view) = (unsafe { content.superview() }) else {
        return;
    };
    let edge = Edge::new(mtm, window, frame_view.bounds());
    edge.setAutoresizingMask(
        NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewHeightSizable,
    );
    frame_view.addSubview_positioned_relativeTo(&edge, NSWindowOrderingMode::Below, None);
    // A pointer already in the window, as when the column is put away from
    // its own button, is where it is from the start.
    let at = ns_window.mouseLocationOutsideOfEventStream();
    if NSPointInRect(at, frame_view.bounds()) {
        edge.follow(Some(at.x));
    }
    EDGES.with(|edges| edges.borrow_mut().push((number, edge)));
}
