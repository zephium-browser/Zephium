//! A hidden sidebar floats over the page while it is called up, as in Arc.
//!
//! The chrome and the page stage are siblings, the stage above, so the chrome
//! is raised over it and later returned beneath it. Both moves reorder the
//! siblings in place: re-adding a web view takes it out of the window for a
//! moment, which blanks it and costs it first responder.
//!
//! The pointer is watched natively rather than by the chrome's own page: the
//! stage parks the chrome's tracking while the pointer is over a page, so the
//! chrome cannot be relied on to notice the pointer reaching the window's
//! edge. A pass-through view over the whole window follows the pointer and
//! says whether it is on the column's side: the window's left edge while the
//! column is hidden, the floating column and an allowance beside it while it
//! is up, and anywhere past the window's left side either way.
use std::cell::{Cell, RefCell};
use std::ffi::c_void;
use std::ptr::{null_mut, NonNull};

use objc2::rc::{Retained, Weak};
use objc2::{define_class, msg_send, AnyThread, DefinedClass, MainThreadOnly};
use objc2_app_kit::{
    NSAutoresizingMaskOptions, NSEvent, NSTrackingArea, NSTrackingAreaOptions, NSView,
    NSWindowOrderingMode,
};
use objc2_foundation::{MainThreadMarker, NSComparisonResult, NSPoint, NSRect};
use objc2_web_kit::WKWebView;

/// How far in from the window's left edge the pointer calls the column up:
/// only the edge itself, so a pointer sweeping left over the page does not
/// bring it up before it gets there. Past the window's left side counts too.
const EDGE: f64 = 3.0;
/// How far past the floating column's edge the pointer may stray, over the
/// page, before the column counts as left: Arc's allowance.
const MARGIN: f64 = 75.0;

thread_local! {
    /// The sibling the chrome sat beneath before it was raised, so lowering
    /// returns it exactly there and not under the window's own backdrop.
    static ABOVE: RefCell<Option<Weak<NSView>>> = const { RefCell::new(None) };
    static WATCH: RefCell<Option<Retained<Watch>>> = const { RefCell::new(None) };
}

pub struct WatchIvars {
    /// Told `true` as the pointer comes to the column's side, `false` as it
    /// leaves it.
    report: Box<dyn Fn(bool)>,
    area: RefCell<Option<Retained<NSTrackingArea>>>,
    /// How far in from the window's left edge the column's side reaches.
    zone: Cell<f64>,
    /// Whether the column is floating over the page.
    floating: Cell<bool>,
    /// What was last reported.
    inside: Cell<bool>,
}

define_class!(
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[name = "ZephiumSidebarEdge"]
    #[ivars = WatchIvars]
    struct Watch;

    impl Watch {
        /// Never a target: clicks and drags reach whatever is beneath.
        #[unsafe(method(hitTest:))]
        fn hit_test(&self, _point: NSPoint) -> *mut NSView {
            null_mut()
        }

        #[unsafe(method(updateTrackingAreas))]
        fn update_tracking_areas(&self) {
            if let Some(old) = self.ivars().area.borrow_mut().take() {
                self.removeTrackingArea(&old);
            }
            // SAFETY: the owner is this view, which outlives its own tracking
            // areas; no user info is attached.
            let area = unsafe {
                NSTrackingArea::initWithRect_options_owner_userInfo(
                    NSTrackingArea::alloc(),
                    NSRect::ZERO,
                    NSTrackingAreaOptions::MouseEnteredAndExited
                        | NSTrackingAreaOptions::MouseMoved
                        | NSTrackingAreaOptions::ActiveInKeyWindow
                        | NSTrackingAreaOptions::InVisibleRect,
                    Some(self),
                    None,
                )
            };
            self.addTrackingArea(&area);
            *self.ivars().area.borrow_mut() = Some(area);
            // SAFETY: NSView requires overrides of this method to call super.
            unsafe { msg_send![super(self), updateTrackingAreas] }
        }

        #[unsafe(method(mouseEntered:))]
        fn mouse_entered(&self, event: &NSEvent) {
            self.judge(event.locationInWindow());
        }

        #[unsafe(method(mouseMoved:))]
        fn mouse_moved(&self, event: &NSEvent) {
            self.judge(event.locationInWindow());
        }

        #[unsafe(method(mouseExited:))]
        fn mouse_exited(&self, event: &NSEvent) {
            self.judge(event.locationInWindow());
        }
    }
);

impl Watch {
    fn new(mtm: MainThreadMarker, report: Box<dyn Fn(bool)>) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(WatchIvars {
            report,
            area: RefCell::new(None),
            zone: Cell::new(EDGE),
            floating: Cell::new(false),
            inside: Cell::new(false),
        });
        // SAFETY: the designated initializer of NSView.
        unsafe { msg_send![super(this), initWithFrame: NSRect::ZERO] }
    }

    /// Whether the pointer at `location`, in window coordinates, is on the
    /// column's side: within its reach of the window's left edge, or past
    /// the window's left side altogether, as a pointer pushed off the edge
    /// is. Off any other side it is not.
    fn judge(&self, location: NSPoint) {
        let point = self.convertPoint_fromView(location, None);
        let bounds = self.bounds();
        let within = point.x >= bounds.origin.x
            && point.x < bounds.origin.x + bounds.size.width
            && point.y >= bounds.origin.y
            && point.y < bounds.origin.y + bounds.size.height;
        let left = point.x - bounds.origin.x;
        let inside = if within {
            left < self.ivars().zone.get()
        } else {
            left < 1.0
        };
        // A drag widening the column carries the pointer past its edge; the
        // drag's end decides.
        if !inside && super::sidebar_resize::capturing() {
            return;
        }
        let changed = self.ivars().inside.replace(inside) != inside;
        // Leaving the edge before the column is up says nothing: the pointer
        // is on its way in, and raising answers where it ended up. So every
        // departure reported comes after that answer, never before it.
        if changed && (inside || self.ivars().floating.get()) {
            (self.ivars().report)(inside);
        }
    }

    fn judge_now(&self) {
        if let Some(window) = self.window() {
            self.judge(window.mouseLocationOutsideOfEventStream());
        }
    }

    /// Watches for the window's left edge, or a floating column `column`
    /// wide and the allowance beside it, from above every sibling.
    fn watch_for(&self, parent: &NSView, column: Option<f64>) {
        use NSAutoresizingMaskOptions as Mask;
        self.ivars().floating.set(column.is_some());
        self.ivars().zone.set(zone(column));
        self.setAutoresizingMask(Mask::ViewWidthSizable | Mask::ViewHeightSizable);
        self.setFrame(parent.bounds());
        // SAFETY: a main-thread read of this view's own parent.
        let placed = unsafe { self.superview() }
            .as_deref()
            .is_some_and(|current| std::ptr::eq(current, parent));
        if !placed {
            parent.addSubview_positioned_relativeTo(self, NSWindowOrderingMode::Above, None);
        }
        self.judge_now();
    }
}

/// The column's side, measured from the column's own width: the chrome
/// fills the window on a new tab, so its frame says nothing of the column.
fn zone(column: Option<f64>) -> f64 {
    use super::sidebar_resize::{GAP, GUTTER};
    column.map_or(EDGE, |column| GUTTER + column + GAP + MARGIN)
}

/// Watches the window's edge for the pointer while the column is hidden,
/// reporting arrivals and departures, or stops when `report` is `None`.
pub(super) fn watch(chrome: &WKWebView, report: Option<Box<dyn Fn(bool)>>) {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let Some(report) = report else {
        dispose();
        return;
    };
    // SAFETY: parent queries stay on AppKit's main thread.
    let Some(parent) = (unsafe { chrome.superview() }) else {
        return;
    };
    dispose();
    let watch = Watch::new(mtm, report);
    WATCH.set(Some(watch.clone()));
    watch.watch_for(&parent, None);
}

/// Whether the pointer is on the column's side, as last judged.
pub(super) fn pointer_on_side() -> bool {
    WATCH.with_borrow(|watch| {
        watch
            .as_ref()
            .is_some_and(|watch| watch.ivars().inside.get())
    })
}

/// Whether the hidden column is floating over the page.
pub(super) fn floating() -> bool {
    WATCH.with_borrow(|watch| {
        watch
            .as_ref()
            .is_some_and(|watch| watch.ivars().floating.get())
    })
}

/// The floating column changed width; its side follows its edge.
pub(super) fn refresh(chrome: &WKWebView) {
    use super::sidebar_resize::{GAP, GUTTER};
    // SAFETY: parent queries stay on AppKit's main thread.
    let Some(parent) = (unsafe { chrome.superview() }) else {
        return;
    };
    let frame = chrome.frame();
    // A chrome filling the window (a new tab) is not the column's width.
    if frame.size.width + 0.5 >= parent.bounds().size.width {
        return;
    }
    set_column(frame.size.width - GUTTER - GAP);
}

/// The floating column is `width` wide now.
pub(super) fn set_column(width: f64) {
    let Some(watch) = WATCH.with_borrow(|watch| watch.clone()) else {
        return;
    };
    if !watch.ivars().floating.get() || !width.is_finite() {
        return;
    }
    let next = zone(Some(width));
    if (watch.ivars().zone.replace(next) - next).abs() > 0.5 {
        watch.judge_now();
    }
}

/// A resize drag ended; a departure it held back is judged now.
pub(super) fn capture_ended() {
    if let Some(watch) = WATCH.with_borrow(|watch| watch.clone()) {
        watch.judge_now();
    }
}

pub(super) fn dispose() {
    if let Some(watch) = WATCH.take() {
        watch.removeFromSuperview();
    }
}

/// Raises the chrome over every sibling, or returns it to where it was.
/// `false` when the chrome is not in a view hierarchy to reorder.
pub(super) fn set_raised(chrome: &WKWebView, raised: bool, column: f64) -> bool {
    // SAFETY: parent queries stay on AppKit's main thread.
    let Some(parent) = (unsafe { chrome.superview() }) else {
        return false;
    };
    let chrome: &NSView = chrome;
    let watch = WATCH.with_borrow(|watch| watch.clone());
    let watch_view: Option<&NSView> = watch.as_deref().map(|watch| &**watch);
    let subviews = parent.subviews();
    // The watch always stands on top, out of the order being decided.
    let mut order: Vec<*const NSView> = subviews
        .iter()
        .map(|view| &*view as *const NSView)
        .filter(|view| watch_view.is_none_or(|watch| !std::ptr::eq(*view, watch)))
        .collect();
    let rank = |order: &[*const NSView], view: *const NSView| {
        order.iter().position(|current| *current == view)
    };
    let Some(index) = rank(&order, chrome) else {
        return false;
    };
    if raised {
        let own = |view: *const NSView| {
            subviews
                .iter()
                .find(|candidate| std::ptr::eq(&**candidate as *const NSView, view))
                .is_some_and(|view| super::sidebar_resize::is_own(&view))
        };
        // Where it came from: the first sibling above it that is not the
        // resize control, which is added above it while raised. The first
        // raise remembers that.
        if let Some(&above) = order[index + 1..].iter().find(|view| !own(**view)) {
            ABOVE.with_borrow_mut(|stored| {
                if stored.as_ref().and_then(Weak::load).is_none() {
                    if let Some(view) = subviews
                        .iter()
                        .find(|view| std::ptr::eq(&**view as *const NSView, above))
                    {
                        *stored = Some(Weak::from_retained(&view));
                    }
                }
            });
        }
        // Over every page, beneath only the resize control, so the floating
        // column can be resized by its edge.
        let (resize, mut rest): (Vec<_>, Vec<_>) = order
            .iter()
            .copied()
            .filter(|view| !std::ptr::eq(*view, chrome))
            .partition(|view| own(*view));
        rest.push(chrome);
        rest.extend(resize);
        order = rest;
    } else if let Some(above) = ABOVE
        .with_borrow_mut(Option::take)
        .and_then(|above| above.load())
    {
        order.remove(index);
        let target = rank(&order, &*above).unwrap_or(order.len());
        order.insert(target, chrome);
    }
    if let Some(watch) = watch_view {
        order.push(watch);
    }
    // SAFETY: the comparator reads `order` only for this synchronous sort, and
    // it lists every one of the parent's subviews.
    unsafe {
        parent.sortSubviewsUsingFunction_context(by_rank, &mut order as *mut Vec<_> as *mut c_void)
    };
    if let Some(watch) = watch.as_deref() {
        watch.watch_for(&parent, raised.then_some(column));
    }
    true
}

unsafe extern "C-unwind" fn by_rank(
    a: NonNull<NSView>,
    b: NonNull<NSView>,
    order: *mut c_void,
) -> NSComparisonResult {
    // SAFETY: `set_raised` lends its order for the duration of the sort.
    let order = unsafe { &*(order as *const Vec<*const NSView>) };
    let rank = |view: NonNull<NSView>| {
        order
            .iter()
            .position(|current| *current == view.as_ptr().cast_const())
    };
    rank(a).cmp(&rank(b)).into()
}
