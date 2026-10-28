//! Native pointer capture for the Svelte sidebar. An expanded column follows
//! the pointer and stops at its narrowest; released well past that, it hides
//! and the page takes its place. It never folds into the rail. The rail's own
//! drag out is a change of shape, which waits for release.
use std::cell::{Cell, RefCell};
use std::ptr::NonNull;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};

use dispatch2::MainThreadBound;
use objc2::rc::{Retained, Weak};
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{define_class, msg_send, ClassType, DefinedClass, MainThreadOnly, Message};
use objc2_app_kit::{
    NSAppearanceCustomization, NSColor, NSCursor, NSEvent, NSEventMask, NSView, NSWindow,
    NSWindowDidResignKeyNotification, NSWindowDidResizeNotification, NSWindowOrderingMode,
    NSWindowWillCloseNotification, NSWorkspace,
};
use objc2_foundation::{
    ns_string, MainThreadMarker, NSNotification, NSNotificationCenter, NSNumber, NSObjectProtocol,
    NSPoint, NSRect, NSSize,
};
use objc2_quartz_core::{CABasicAnimation, CAMediaTiming, CATransaction};
use objc2_web_kit::WKWebView;
use zephium_core::layout::{MAX_SIDEBAR_WIDTH, MIN_SIDEBAR_WIDTH};

/// What a released (or abandoned) drag asks the column for.
#[derive(Clone, Copy)]
pub(super) enum Release {
    /// A width, travelling for a change of shape.
    Width { width: f64, travel: bool },
    /// Hide the column; it comes back at `width`.
    Hide { width: f64 },
}
type Commit = Rc<dyn Fn(Release, u64)>;
static REVISION: AtomicU64 = AtomicU64::new(0);
pub(super) fn publish_revision(revision: u64) -> bool {
    revision >= REVISION.fetch_max(revision, Ordering::AcqRel)
}
fn revision_current(revision: u64) -> bool {
    REVISION.load(Ordering::Acquire) == revision
}
/// The chrome draws the window's gutter before the column, so the column's
/// edge is this far in from the chrome's own.
pub(super) const GUTTER: f64 = 8.0;
/// The gap between the column and the page.
pub(super) const GAP: f64 = 8.0;
// The hot zone overlaps the sidebar by only 2pt, so the tab list's edge and
// overlay scrollbar stay reachable underneath it, fills the gap, and reaches
// a little over the page's edge, where the pointer tends to land.
const HOT_INSET: f64 = 2.0;
const HOT_OUTSET: f64 = 6.0;
const HOT_WIDTH: f64 = HOT_INSET + GAP + HOT_OUTSET;
const SNAP: f64 = 140.0;
const MIN_EXPANDED: f64 = 180.0;

#[derive(Clone, Copy)]
struct Capture {
    start_x: f64,
    original: f64,
    pending: f64,
    revision: u64,
    /// An expanded column resizes as it is dragged. The rail does not: its
    /// way out is a change of shape, which only release makes.
    live: bool,
    /// The width the column was last given during the drag.
    applied: f64,
}

struct Observers {
    cursor: Option<Cursor>,
    escape: Retained<AnyObject>,
    notifications: Vec<Retained<ProtocolObject<dyn NSObjectProtocol>>>,
}
struct Cursor {
    window: Retained<NSWindow>,
    cursor: Retained<NSCursor>,
    restore: bool,
}
impl Cursor {
    #[allow(deprecated)]
    fn new(window: Retained<NSWindow>) -> Self {
        let restore = window.areCursorRectsEnabled();
        if restore {
            window.disableCursorRects();
        }
        let cursor = NSCursor::resizeLeftRightCursor();
        cursor.push();
        Self {
            window,
            cursor,
            restore,
        }
    }
}
impl Drop for Cursor {
    fn drop(&mut self) {
        NSCursor::pop_class();
        if self.restore {
            self.window.enableCursorRects();
        }
    }
}
impl Drop for Observers {
    fn drop(&mut self) {
        drop(self.cursor.take());
        // SAFETY: these exact main-thread registrations are removed once.
        unsafe {
            NSEvent::removeMonitor(&self.escape);
            for observer in &self.notifications {
                NSNotificationCenter::defaultCenter().removeObserver((**observer).as_ref());
            }
        }
    }
}

pub struct Ivars {
    chrome: Weak<WKWebView>,
    width: Cell<f64>,
    enabled: Cell<bool>,
    revision: Cell<u64>,
    generation: Cell<u64>,
    capture: Cell<Option<Capture>>,
    guide: RefCell<Option<Retained<NSView>>>,
    observers: RefCell<Option<Observers>>,
    commit: Commit,
}

define_class!(
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[name = "ZephiumSidebarResizeControl"]
    #[ivars = Ivars]
    pub(super) struct Control;
    impl Control {
        #[unsafe(method(resetCursorRects))]
        #[allow(deprecated)]
        fn reset_cursor_rects(&self) {
            let _: () = unsafe { msg_send![super(self), resetCursorRects] };
            if self.ivars().enabled.get() { self.addCursorRect_cursor(self.bounds(), &NSCursor::resizeLeftRightCursor()); }
        }
        // A transparent view in a full-size-content window would otherwise
        // drag the window from the title-bar band instead of resizing.
        #[unsafe(method(mouseDownCanMoveWindow))]
        fn can_move_window(&self) -> bool { false }
        #[unsafe(method(acceptsFirstMouse:))]
        fn accepts_first_mouse(&self, _event: Option<&NSEvent>) -> bool { true }
        #[unsafe(method(viewDidMoveToWindow))]
        fn moved(&self) {
            let _: () = unsafe { msg_send![super(self), viewDidMoveToWindow] };
            if self.window().is_none() { self.cancel(false); }
        }
        #[unsafe(method(mouseDown:))]
        fn down(&self, event: &NSEvent) {
            let _owner = self.retain();
            self.cancel(false);
            if !self.ivars().enabled.get() || !revision_current(self.ivars().revision.get()) { return; }
            let Some(window) = self.window() else { return };
            let original = self.ivars().width.get();
            self.ivars().capture.set(Some(Capture { start_x: event.locationInWindow().x, original, pending: original, revision: self.ivars().revision.get(), live: original >= MIN_EXPANDED, applied: original }));
            if !self.install_observers(window) { self.cancel(false); return; }
            // A live drag is its own feedback; the rail's drag previews its landing.
            if original < MIN_EXPANDED { self.feedback(guide_x(original)); }
        }
        #[unsafe(method(mouseDragged:))]
        fn dragged(&self, event: &NSEvent) {
            let _owner = self.retain();
            let Some(mut capture) = self.ivars().capture.get() else { return };
            let generation = self.ivars().generation.get();
            let cursor = self.ivars().observers.try_borrow().ok().and_then(|observers| observers.as_ref().and_then(|observers| observers.cursor.as_ref()).map(|cursor| cursor.cursor.clone()));
            if let Some(cursor) = cursor { cursor.set(); }
            if self.ivars().generation.get() != generation || !revision_current(capture.revision) { self.cancel(false); return; }
            let requested = capture.original + event.locationInWindow().x - capture.start_x;
            if !requested.is_finite() { return; }
            capture.pending = requested.clamp(MIN_SIDEBAR_WIDTH, MAX_SIDEBAR_WIDTH);
            let landing = resolved_width(capture.pending, capture.live);
            if capture.live {
                // Past the narrowest, release hides the column; the guide
                // stands where the page will then begin.
                if hides(requested, &capture) { self.feedback(HIDE_GUIDE_X); } else { self.clear_guide(); }
                let changed = (landing - capture.applied).abs() >= 0.5;
                if changed { capture.applied = landing; }
                self.ivars().capture.set(Some(capture));
                if changed {
                    self.ivars().width.set(landing);
                    super::sidebar_overlay::set_column(landing);
                    self.place();
                    (self.ivars().commit)(Release::Width { width: landing, travel: false }, capture.revision);
                }
            } else {
                self.ivars().capture.set(Some(capture));
                // The rail's way out lands on release; the guide shows where.
                self.feedback(guide_x(landing));
            }
        }
        #[unsafe(method(mouseUp:))]
        fn up(&self, event: &NSEvent) {
            let _owner = self.retain();
            let Some(capture) = self.ivars().capture.get() else { return };
            if !revision_current(capture.revision) { self.cancel(false); return; }
            let requested = capture.original + event.locationInWindow().x - capture.start_x;
            let width = resolved_width(if requested.is_finite() { requested } else { capture.pending }, capture.live);
            let generation = self.ivars().generation.get();
            let shape = !capture.live;
            let hide = requested.is_finite() && hides(requested, &capture);
            if shape { self.feedback(guide_x(width)); }
            if self.ivars().generation.get() != generation { return; }
            self.cancel(true);
            if self.ivars().generation.get() != generation.wrapping_add(1) || !self.ivars().enabled.get() { return; }
            if !revision_current(capture.revision) { return; }
            // A drag that followed the pointer is already there; a change of
            // shape travels, which also covers the chrome's repaint.
            if hide { (self.ivars().commit)(Release::Hide { width: capture.original }, capture.revision); return; }
            if shape || (width - capture.applied).abs() >= 0.5 { (self.ivars().commit)(Release::Width { width, travel: shape }, capture.revision); }
        }
    }
);

define_class!(
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[name = "ZephiumSidebarResizeGuide"]
    struct Guide;
    impl Guide {
        #[unsafe(method(hitTest:))]
        fn hit_test(&self, _point: NSPoint) -> *mut NSView { std::ptr::null_mut() }
    }
);

impl Control {
    fn new(
        chrome: &Retained<WKWebView>,
        width: f64,
        commit: Commit,
        revision: u64,
        mtm: MainThreadMarker,
    ) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(Ivars {
            chrome: Weak::from_retained(chrome),
            width: Cell::new(width),
            enabled: Cell::new(true),
            revision: Cell::new(revision),
            generation: Cell::new(0),
            capture: Cell::new(None),
            guide: RefCell::new(None),
            observers: RefCell::new(None),
            commit,
        });
        // SAFETY: standard main-thread NSView initialization.
        let this: Retained<Self> = unsafe { msg_send![super(this), init] };
        this.setTranslatesAutoresizingMaskIntoConstraints(false);
        this
    }
    fn place(&self) {
        let Some(chrome) = self.ivars().chrome.load() else {
            self.cancel(false);
            self.setHidden(true);
            return;
        };
        let frame = chrome.frame();
        let width = self.ivars().width.get();
        // A live drag widens the column before the chrome has grown to it.
        let growing = self
            .ivars()
            .capture
            .get()
            .is_some_and(|capture| capture.live);
        let hidden =
            !self.ivars().enabled.get() || (!growing && frame.size.width + 0.5 < GUTTER + width);
        let target = NSRect::new(
            NSPoint::new(frame.origin.x + GUTTER + width - HOT_INSET, frame.origin.y),
            NSSize::new(HOT_WIDTH, frame.size.height),
        );
        let current = self.frame();
        if current.size.height != target.size.height {
            self.cancel(false);
        }
        // Runs for every frame of a window live resize; unchanged geometry
        // must not dirty the view or rebuild the window's cursor rects.
        let moved = current.origin.x != target.origin.x
            || current.origin.y != target.origin.y
            || current.size.width != target.size.width
            || current.size.height != target.size.height;
        if moved {
            self.setFrame(target);
        }
        if self.isHidden() != hidden {
            self.setHidden(hidden);
        }
        if hidden {
            self.cancel(false);
        }
        if moved {
            if let Some(window) = self.window() {
                window.invalidateCursorRectsForView(self);
            }
        }
    }
    /// Draws the guide `x` points in from the chrome's edge.
    fn feedback(&self, x: f64) {
        let Some(chrome) = self.ivars().chrome.load() else {
            return;
        };
        // SAFETY: parent queries stay on AppKit's main thread.
        let Some(parent) = (unsafe { chrome.superview() }) else {
            return;
        };
        let generation = self.ivars().generation.get();
        let frame = chrome.frame();
        let target = NSRect::new(
            NSPoint::new(frame.origin.x + x, frame.origin.y),
            NSSize::new(2.0, frame.size.height),
        );
        let existing = self
            .ivars()
            .guide
            .try_borrow()
            .ok()
            .and_then(|guide| guide.clone());
        let view = if let Some(view) = existing {
            view
        } else {
            // SAFETY: standard NSView initialization for our passive main-thread subclass.
            let view: Retained<Guide> = unsafe { msg_send![Guide::alloc(self.mtm()), init] };
            let view = view.into_super();
            view.setWantsLayer(true);
            view.setTranslatesAutoresizingMaskIntoConstraints(false);
            if self.ivars().generation.get() != generation || !self.ivars().enabled.get() {
                return;
            }
            let Ok(mut guide) = self.ivars().guide.try_borrow_mut() else {
                return;
            };
            if guide.is_some() {
                return;
            }
            *guide = Some(view.clone());
            drop(guide);
            parent.addSubview_positioned_relativeTo(&view, NSWindowOrderingMode::Above, None);
            view
        };
        if !self.guide_current(&view, generation) {
            view.removeFromSuperview();
            return;
        }
        CATransaction::begin();
        CATransaction::setDisableActions(true);
        view.setFrame(target);
        if let Some(layer) = view.layer() {
            let paint = block2::RcBlock::new(|| {
                layer.setBackgroundColor(Some(&NSColor::labelColor().CGColor()));
            });
            self.effectiveAppearance()
                .performAsCurrentDrawingAppearance(&paint);
            layer.setCornerRadius(1.0);
            layer.setOpacity(0.9);
        }
        CATransaction::commit();
        if !self.guide_current(&view, generation) {
            view.removeFromSuperview();
        }
    }
    fn clear_guide(&self) {
        let guide = self
            .ivars()
            .guide
            .try_borrow_mut()
            .ok()
            .and_then(|mut guide| guide.take());
        if let Some(guide) = guide {
            guide.removeFromSuperview();
        }
    }
    /// Escape gives back the width the drag started from.
    fn restore(&self) {
        let Some(capture) = self.ivars().capture.get() else {
            return;
        };
        if !capture.live
            || (capture.applied - capture.original).abs() < 0.5
            || !revision_current(capture.revision)
        {
            return;
        }
        self.ivars().width.set(capture.original);
        self.place();
        (self.ivars().commit)(
            Release::Width {
                width: capture.original,
                travel: false,
            },
            capture.revision,
        );
    }
    fn guide_current(&self, view: &NSView, generation: u64) -> bool {
        self.ivars().generation.get() == generation
            && self.ivars().enabled.get()
            && self.ivars().guide.try_borrow().is_ok_and(|guide| {
                guide
                    .as_deref()
                    .is_some_and(|current| std::ptr::eq(current, view))
            })
    }
    fn cancel(&self, fade: bool) {
        let captured = self.ivars().capture.take().is_some();
        self.ivars()
            .generation
            .set(self.ivars().generation.get().wrapping_add(1));
        let guide = self
            .ivars()
            .guide
            .try_borrow_mut()
            .ok()
            .and_then(|mut guide| guide.take());
        let observers = self
            .ivars()
            .observers
            .try_borrow_mut()
            .ok()
            .and_then(|mut observers| observers.take());
        drop(observers);
        if captured {
            super::sidebar_overlay::capture_ended();
        }
        if let Some(guide) = guide {
            if !fade || NSWorkspace::sharedWorkspace().accessibilityDisplayShouldReduceMotion() {
                guide.removeFromSuperview();
                return;
            }
            let Some(layer) = guide.layer() else {
                guide.removeFromSuperview();
                return;
            };
            let complete = guide.clone();
            let completed = block2::RcBlock::new(move || {
                complete.removeFromSuperview();
            });
            let animation = CABasicAnimation::animationWithKeyPath(Some(ns_string!("opacity")));
            // SAFETY: opacity takes NSNumber values; the native transaction copies the block.
            unsafe {
                animation.setFromValue(Some(&NSNumber::new_f64(layer.opacity() as f64)));
                animation.setToValue(Some(&NSNumber::new_f64(0.0)));
            }
            animation.setDuration(0.10);
            CATransaction::begin();
            CATransaction::setDisableActions(true);
            unsafe {
                CATransaction::setCompletionBlock(Some(&completed));
            }
            layer.setOpacity(0.0);
            layer.addAnimation_forKey(&animation, Some(ns_string!("zephium.sidebar-guide.fade")));
            CATransaction::commit();
        }
    }
    fn install_observers(&self, window: Retained<NSWindow>) -> bool {
        let generation = self.ivars().generation.get();
        let weak = Weak::from_retained(&self.retain());
        let escape = block2::RcBlock::new(move |event: NonNull<NSEvent>| -> *mut NSEvent {
            // SAFETY: the local event monitor lends its event throughout this callback.
            if unsafe { event.as_ref() }.keyCode() == 53 {
                let canceled = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    if let Some(control) = weak.load() {
                        if control.ivars().generation.get() == generation {
                            control.restore();
                            control.cancel(false);
                            return true;
                        }
                    }
                    false
                }))
                .unwrap_or(false);
                if canceled {
                    return std::ptr::null_mut();
                }
            }
            event.as_ptr()
        });
        // SAFETY: this is an app-local main-thread monitor, not a global input hook.
        let Some(escape) = (unsafe {
            NSEvent::addLocalMonitorForEventsMatchingMask_handler(NSEventMask::KeyDown, &escape)
        }) else {
            return false;
        };
        let weak = MainThreadBound::new(Weak::from_retained(&self.retain()), self.mtm());
        let changed = block2::RcBlock::new(move |_: NonNull<NSNotification>| {
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let Some(mtm) = MainThreadMarker::new() else {
                    return;
                };
                if let Some(control) = weak.get(mtm).load() {
                    if control.ivars().generation.get() == generation {
                        control.cancel(false);
                    }
                }
            }));
        });
        let mut notifications = Vec::new();
        // SAFETY: the exact window and copied sendable weak block live on the main thread.
        unsafe {
            for name in [
                NSWindowDidResignKeyNotification,
                NSWindowDidResizeNotification,
                NSWindowWillCloseNotification,
            ] {
                notifications.push(
                    NSNotificationCenter::defaultCenter()
                        .addObserverForName_object_queue_usingBlock(
                            Some(name),
                            Some(&window),
                            None,
                            &changed,
                        ),
                );
            }
        }
        let observers = Observers {
            cursor: Some(Cursor::new(window)),
            escape,
            notifications,
        };
        if self.ivars().generation.get() != generation {
            return false;
        }
        let Ok(mut retained) = self.ivars().observers.try_borrow_mut() else {
            return false;
        };
        *retained = Some(observers);
        true
    }
}

thread_local! {
    static CONTROL: RefCell<Option<(u64, Retained<Control>)>> = const { RefCell::new(None) };
    static CONFIG_EPOCH: Cell<u64> = const { Cell::new(0) };
}
fn configuration_current(epoch: u64, generation: u64, revision: u64) -> bool {
    CONFIG_EPOCH.with(|current| current.get() == epoch)
        && super::CHROME_GENERATION.load(Ordering::Acquire) == generation
        && revision_current(revision)
}
fn registered(control: &Control, generation: u64) -> bool {
    CONTROL.with(|slot| {
        slot.try_borrow().is_ok_and(|slot| {
            slot.as_ref().is_some_and(|(owner, stored)| {
                *owner == generation && std::ptr::eq(&**stored, control)
            })
        })
    })
}
pub(super) fn configure(
    chrome: &Retained<WKWebView>,
    generation: u64,
    width: f64,
    enabled: bool,
    revision: u64,
    commit: Commit,
) -> bool {
    let Some(mtm) = MainThreadMarker::new() else {
        return false;
    };
    // Switching the guide off is always safe, so it is never refused as
    // stale: every width the column publishes advances the revision, and a
    // refused "off" left the guide live at an edge the column had left.
    if !enabled {
        disable();
        return false;
    }
    if !revision_current(revision) {
        return false;
    }
    let epoch = CONFIG_EPOCH.with(|current| {
        let next = current.get().wrapping_add(1);
        current.set(next);
        next
    });
    let existing = CONTROL.with(|slot| {
        slot.try_borrow().ok().and_then(|slot| {
            slot.as_ref()
                .filter(|(owner, _)| *owner == generation)
                .map(|(_, control)| control.clone())
        })
    });
    let control = if let Some(control) = existing {
        control
    } else {
        dispose(None);
        if !enabled || !configuration_current(epoch, generation, revision) {
            return false;
        }
        let control = Control::new(chrome, width, commit, revision, mtm);
        // SAFETY: native parent access is confined to the main thread.
        let Some(parent) = (unsafe { chrome.superview() }) else {
            return false;
        };
        if !configuration_current(epoch, generation, revision) {
            return false;
        }
        let published = CONTROL.with(|slot| {
            let Ok(mut slot) = slot.try_borrow_mut() else {
                return false;
            };
            if slot.is_some() {
                return false;
            }
            *slot = Some((generation, control.clone()));
            true
        });
        if !published {
            return false;
        }
        parent.addSubview_positioned_relativeTo(&control, NSWindowOrderingMode::Above, None);
        if !configuration_current(epoch, generation, revision) || !registered(&control, generation)
        {
            if registered(&control, generation) {
                CONTROL.with(|slot| {
                    slot.borrow_mut().take();
                });
            }
            control.cancel(false);
            control.removeFromSuperview();
            return false;
        }
        control
    };
    // A live drag publishes widths as it goes, and the frame echoes each one
    // back, late; the drag owns the width until release.
    let dragging = enabled
        && control
            .ivars()
            .capture
            .get()
            .is_some_and(|capture| capture.live && capture.revision == revision);
    if !dragging
        && (control.ivars().capture.get().is_some()
            || control.ivars().width.get() != width
            || !enabled)
    {
        control.cancel(false);
    }
    if !configuration_current(epoch, generation, revision) || !registered(&control, generation) {
        return false;
    }
    if !dragging {
        control.ivars().width.set(width);
    }
    control.ivars().enabled.set(enabled);
    control.ivars().revision.set(revision);
    control.place();
    let current =
        configuration_current(epoch, generation, revision) && registered(&control, generation);
    let installed = current && enabled && !control.isHidden() && control.window().is_some();
    if current && enabled && !installed {
        control.ivars().enabled.set(false);
        control.cancel(false);
        control.setHidden(true);
    }
    installed
}
fn disable() {
    let control = CONTROL.with(|slot| {
        slot.try_borrow()
            .ok()
            .and_then(|slot| slot.as_ref().map(|(_, control)| control.clone()))
    });
    if let Some(control) = control {
        control.ivars().enabled.set(false);
        control.cancel(false);
        control.place();
    }
}
/// The column's width as native has applied it. The guide belongs to the
/// column's edge, so wherever the column now ends somewhere else (a tool
/// panel beside the rail, the rail itself) it is switched off until the
/// frame configures it again at the new edge.
pub(super) fn column_width(width: f64) {
    let control = CONTROL.with(|slot| {
        slot.try_borrow()
            .ok()
            .and_then(|slot| slot.as_ref().map(|(_, control)| control.clone()))
    });
    if control.is_some_and(|control| (control.ivars().width.get() - width).abs() > 0.5) {
        disable();
    }
}
/// Whether `view` is the resize control or its guide, which stand above the
/// floating column so it can be resized.
pub(super) fn is_own(view: &NSView) -> bool {
    view.isKindOfClass(Control::class()) || view.isKindOfClass(Guide::class())
}
/// Whether a drag holds the pointer.
pub(super) fn capturing() -> bool {
    CONTROL.with(|slot| {
        slot.try_borrow().ok().is_some_and(|slot| {
            slot.as_ref()
                .is_some_and(|(_, control)| control.ivars().capture.get().is_some())
        })
    })
}
pub(super) fn refresh() {
    let control = CONTROL.with(|slot| {
        slot.try_borrow()
            .ok()
            .and_then(|slot| slot.as_ref().map(|(_, control)| control.clone()))
    });
    if let Some(control) = control {
        control.place();
    }
}
pub(super) fn dispose(generation: Option<u64>) {
    let old = CONTROL.with(|slot| {
        slot.try_borrow_mut().ok().and_then(|mut slot| {
            if generation.is_some_and(|generation| {
                slot.as_ref().is_some_and(|(owner, _)| *owner != generation)
            }) {
                return None;
            }
            slot.take()
        })
    });
    if let Some((_, control)) = old {
        control.ivars().enabled.set(false);
        control.cancel(false);
        control.removeFromSuperview();
    }
}
/// The guide for a column `width` wide: at the page's edge, where the hot
/// zone ends, not the column's.
fn guide_x(width: f64) -> f64 {
    GUTTER + width + GAP - 2.0
}
/// The guide for a release that hides the column: where the page will begin.
const HIDE_GUIDE_X: f64 = GUTTER - 2.0;
/// An expanded column dragged this far short of its narrowest hides. One
/// already floating over the page is hidden, and stops at its narrowest.
fn hides(requested: f64, capture: &Capture) -> bool {
    capture.live && requested < SNAP && !super::sidebar_overlay::floating()
}
/// Where a drag lands. An expanded column stops at its narrowest; only the
/// rail's drag can stay on the rail, when it does not reach the snap.
fn resolved_width(width: f64, expanded: bool) -> f64 {
    if !width.is_finite() {
        return if expanded {
            MIN_EXPANDED
        } else {
            MIN_SIDEBAR_WIDTH
        };
    }
    if !expanded && width < SNAP {
        MIN_SIDEBAR_WIDTH
    } else {
        width.round().clamp(MIN_EXPANDED, MAX_SIDEBAR_WIDTH)
    }
}
