//! Keeps the page canvas from setting the cursor beneath a native pane.
//! WebKit sets the cursor from whichever web view receives mouse-moved
//! events, and AppKit delivers a tracking area's events even when another
//! view covers it, so the canvas and the pane both set it under the pointer.
//! While the pointer is over the covering view, the web views beneath it
//! park their tracking areas; clicks and drags still reach them.
#![deny(unsafe_op_in_unsafe_fn, clippy::undocumented_unsafe_blocks)]

use std::cell::RefCell;
use std::collections::HashMap;

use objc2::rc::{Retained, Weak};
use objc2::runtime::AnyObject;
use objc2::{define_class, msg_send, sel, DefinedClass, MainThreadOnly, Message};
use objc2_app_kit::{
    NSEvent, NSTrackingArea, NSTrackingAreaOptions, NSView, NSViewFrameDidChangeNotification,
    NSWindowDidBecomeKeyNotification, NSWindowDidResignKeyNotification,
};
use objc2_foundation::{
    MainThreadMarker, NSNotification, NSNotificationCenter, NSObject, NSObjectProtocol, NSRect,
};
use wry::WryWebView;

thread_local! {
    /// How many covers hold each canvas parked, by view address, so two panes
    /// over one canvas release it only when the pointer has left both.
    static HELD: RefCell<HashMap<usize, usize>> = RefCell::new(HashMap::new());
    /// Every live cover, for a sibling order changed under them.
    static COVERS: RefCell<Vec<Weak<Cover>>> = const { RefCell::new(Vec::new()) };
}

struct CoverIvars {
    view: Weak<NSView>,
    area: RefCell<Option<Retained<NSTrackingArea>>>,
    parked: RefCell<Vec<Retained<WryWebView>>>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "ZephiumPointerCover"]
    #[ivars = CoverIvars]
    struct Cover;

    impl Cover {
        #[unsafe(method(mouseEntered:))]
        fn mouse_entered(&self, _event: &NSEvent) {
            self.sync();
        }

        #[unsafe(method(mouseExited:))]
        fn mouse_exited(&self, _event: &NSEvent) {
            self.sync();
        }

        #[unsafe(method(coverNeedsSync:))]
        fn needs_sync(&self, _notification: &NSNotification) {
            self.sync();
        }
    }

    unsafe impl NSObjectProtocol for Cover {}
);

impl Cover {
    fn sync(&self) {
        let covered = self.ivars().view.load().filter(|view| pointer_inside(view));
        match covered {
            Some(view) => self.park(&view),
            None => self.release(),
        }
    }

    /// Parks every visible web view beneath `view` that shares its footprint
    /// and is not already out of the pointer's way.
    fn park(&self, view: &NSView) {
        // SAFETY: main-thread read of the view's parent for identity and order.
        let Some(parent) = (unsafe { view.superview() }) else {
            return self.release();
        };
        let cover = view.frame();
        let mut parked = self.ivars().parked.borrow_mut();
        for sibling in parent.subviews().iter() {
            if std::ptr::eq(&*sibling, view) {
                break;
            }
            let Some(canvas) = sibling.downcast_ref::<WryWebView>() else {
                continue;
            };
            let ours = parked.iter().any(|held| std::ptr::eq(&**held, canvas));
            if ours
                || canvas.isHidden()
                || canvas.is_pointer_inert()
                || !overlaps(canvas.frame(), cover)
            {
                continue;
            }
            hold(canvas);
            parked.push(canvas.retain());
        }
    }

    fn release(&self) {
        for canvas in self.ivars().parked.take() {
            let last = HELD.with_borrow_mut(|held| {
                let key = std::ptr::from_ref(&*canvas).addr();
                let count = held.get_mut(&key).map(|count| {
                    *count -= 1;
                    *count
                });
                if count == Some(0) {
                    held.remove(&key);
                }
                count == Some(0)
            });
            if last {
                canvas.set_tracking_parked(false);
            }
        }
    }

    fn detach(&self) {
        // SAFETY: removes only the observations `attach` registered for `self`.
        unsafe { NSNotificationCenter::defaultCenter().removeObserver(self) };
        if let (Some(view), Some(area)) = (self.ivars().view.load(), self.ivars().area.take()) {
            view.removeTrackingArea(&area);
        }
        self.release();
    }
}

fn hold(canvas: &WryWebView) {
    let first = HELD.with_borrow_mut(|held| {
        let count = held.entry(std::ptr::from_ref(canvas).addr()).or_default();
        *count += 1;
        *count == 1
    });
    if first {
        canvas.set_tracking_parked(true);
    }
}

fn overlaps(a: NSRect, b: NSRect) -> bool {
    a.origin.x < b.origin.x + b.size.width
        && b.origin.x < a.origin.x + a.size.width
        && a.origin.y < b.origin.y + b.size.height
        && b.origin.y < a.origin.y + a.size.height
}

fn pointer_inside(view: &NSView) -> bool {
    let Some(window) = view.window() else {
        return false;
    };
    if !window.isKeyWindow() || view.isHiddenOrHasHiddenAncestor() {
        return false;
    }
    let point = view.convertPoint_fromView(window.mouseLocationOutsideOfEventStream(), None);
    let visible = view.visibleRect();
    point.x >= visible.origin.x
        && point.x < visible.origin.x + visible.size.width
        && point.y >= visible.origin.y
        && point.y < visible.origin.y + visible.size.height
}

/// Held by whoever presents `view` over the canvas; dropping it, or the
/// owner's `sync`, hands the canvas its cursor back.
pub(crate) struct PointerCover(Retained<Cover>);

impl PointerCover {
    pub(crate) fn attach(view: &NSView) -> Option<Self> {
        let mtm = MainThreadMarker::new()?;
        let cover = mtm.alloc::<Cover>().set_ivars(CoverIvars {
            view: Weak::from_retained(&view.retain()),
            area: RefCell::new(None),
            parked: RefCell::new(Vec::new()),
        });
        // SAFETY: plain NSObject initialisation.
        let cover: Retained<Cover> = unsafe { msg_send![super(cover), init] };
        // SAFETY: the owner outlives the area: `detach` removes it first.
        let area = unsafe {
            NSTrackingArea::initWithRect_options_owner_userInfo(
                mtm.alloc(),
                NSRect::ZERO,
                NSTrackingAreaOptions::MouseEnteredAndExited
                    | NSTrackingAreaOptions::ActiveInKeyWindow
                    | NSTrackingAreaOptions::InVisibleRect,
                Some(&cover),
                None,
            )
        };
        view.addTrackingArea(&area);
        *cover.ivars().area.borrow_mut() = Some(area);
        view.setPostsFrameChangedNotifications(true);
        let center = NSNotificationCenter::defaultCenter();
        let observer: &AnyObject = &cover;
        // SAFETY: `coverNeedsSync:` takes the notification and is defined above.
        unsafe {
            for (name, object) in [
                (NSViewFrameDidChangeNotification, Some(view as &AnyObject)),
                (NSWindowDidResignKeyNotification, None),
                (NSWindowDidBecomeKeyNotification, None),
            ] {
                center.addObserver_selector_name_object(
                    observer,
                    sel!(coverNeedsSync:),
                    Some(name),
                    object,
                );
            }
        }
        cover.sync();
        COVERS.with_borrow_mut(|covers| {
            covers.retain(|cover| cover.load().is_some());
            covers.push(Weak::from_retained(&cover));
        });
        Some(Self(cover))
    }

    /// Re-reads where the pointer is, for changes AppKit does not announce:
    /// the covering view hiding, closing or moving under a still pointer.
    pub(crate) fn sync(&self) {
        self.0.sync();
    }
}

/// Parks afresh beneath every cover after the views' order changed: one view
/// raised above a cover is no longer beneath it, and one lowered now is. The
/// order alone changing is not something AppKit announces.
pub fn resync_all() {
    let covers: Vec<_> =
        COVERS.with_borrow(|covers| covers.iter().filter_map(Weak::load).collect());
    for cover in covers {
        cover.release();
        cover.sync();
    }
}

impl Drop for PointerCover {
    fn drop(&mut self) {
        self.0.detach();
    }
}
