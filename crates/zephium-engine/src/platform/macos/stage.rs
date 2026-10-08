use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::ptr::NonNull;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use dispatch2::{DispatchQueue, MainThreadBound};
use objc2::rc::{Retained, Weak};
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{define_class, msg_send, AnyThread, DefinedClass, MainThreadOnly, Message};
use objc2_app_kit::{
    NSAppearanceCustomization, NSAppearanceNameAqua, NSAppearanceNameDarkAqua, NSColor, NSCursor,
    NSEvent, NSEventMask, NSTrackingArea, NSTrackingAreaOptions, NSView, NSWindow,
    NSWindowDidResignKeyNotification, NSWindowOrderingMode, NSWorkspace,
};
use objc2_core_graphics::CGImage;
use objc2_foundation::{
    ns_string, MainThreadMarker, NSArray, NSNotification, NSNotificationCenter, NSNumber,
    NSObjectProtocol, NSPoint, NSRect, NSSize, NSValue,
};
use objc2_quartz_core::{
    kCAFillModeForwards, CABasicAnimation, CAMediaTiming, CAMediaTimingFunction, CATransaction,
};

use zephium_core::geometry::Rect;
use zephium_core::ids::ItemId;
use zephium_core::split::{self, Divider, Pane};

use crate::pane_geometry::rounded_native_size;

type RatioCallback = Rc<dyn Fn(Pane)>;
type StageFailureCallback = Rc<dyn Fn(usize)>;
const MAX_ASYNC_STAGE_RETRIES: u8 = 4;

#[derive(Clone)]
struct HostView {
    view: Retained<NSView>,
    presentation_permit: Arc<AtomicBool>,
}

#[derive(Clone)]
struct PaintCover {
    token: u64,
    view: Retained<NSView>,
    // A restored page's last frame fades into the live page; a plain colour
    // cover is replaced within one frame and simply disappears.
    fades: bool,
}

define_class!(
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[name = "ZephiumNavigationPaintCover"]
    struct PaintCoverView;

    impl PaintCoverView {
        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, _event: &NSEvent) {}
        #[unsafe(method(mouseUp:))]
        fn mouse_up(&self, _event: &NSEvent) {}
        #[unsafe(method(mouseDragged:))]
        fn mouse_dragged(&self, _event: &NSEvent) {}
        #[unsafe(method(rightMouseDown:))]
        fn right_mouse_down(&self, _event: &NSEvent) {}
        #[unsafe(method(otherMouseDown:))]
        fn other_mouse_down(&self, _event: &NSEvent) {}
        #[unsafe(method(scrollWheel:))]
        fn scroll_wheel(&self, _event: &NSEvent) {}
    }
);

struct DragObservers {
    escape: Retained<AnyObject>,
    resigned: Retained<ProtocolObject<dyn NSObjectProtocol>>,
    cursor: Option<DragCursor>,
}

struct DragCursor {
    window: Retained<NSWindow>,
    cursor: Retained<NSCursor>,
    restore_rects: bool,
}

impl DragCursor {
    #[allow(deprecated)]
    fn new(window: Retained<NSWindow>, axis: split::Axis) -> Self {
        let cursor = match axis {
            split::Axis::Row => NSCursor::resizeLeftRightCursor(),
            split::Axis::Col => NSCursor::resizeUpDownCursor(),
        };
        let restore_rects = window.areCursorRectsEnabled();
        // During capture the pointer leaves the original gutter and crosses
        // WKWebView cursor regions. The gesture owns the cursor until it ends.
        if restore_rects {
            window.disableCursorRects();
        }
        cursor.push();
        Self {
            window,
            cursor,
            restore_rects,
        }
    }
}

impl Drop for DragCursor {
    fn drop(&mut self) {
        NSCursor::pop_class();
        if self.restore_rects {
            self.window.enableCursorRects();
        }
    }
}

impl Drop for DragObservers {
    fn drop(&mut self) {
        // Restore the cursor stack before native deregistration can re-enter.
        drop(self.cursor.take());
        // SAFETY: these exact tokens were returned by AppKit/Foundation and
        // are retired on the same main thread as their native registrations.
        unsafe {
            NSEvent::removeMonitor(&self.escape);
            NSNotificationCenter::defaultCenter().removeObserver((*self.resigned).as_ref());
        }
    }
}

define_class!(
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[name = "ZephiumDividerFeedback"]
    struct DividerFeedbackView;
    impl DividerFeedbackView {
        #[unsafe(method(hitTest:))]
        fn hit_test(&self, _point: NSPoint) -> *mut NSView { std::ptr::null_mut() }
    }
);

#[derive(Default)]
pub struct StageIvars {
    // Boxed: ItemId is a u128 (align 16), while objc2 0.6's `define_class!`
    // registrar supports ivar alignments only through 8 bytes. Keep the tree
    // in a correctly aligned Rust allocation behind a pointer-sized ivar.
    tree: RefCell<Option<Box<Pane>>>,
    views: RefCell<HashMap<ItemId, HostView>>,
    // A raw view is never exposed while it still contains WKWebView's
    // construction-time blank document. `visible` is the logical split
    // model; `ready` advances only after privileged chrome applies and
    // verifies the exact committed URL/revision.
    visible: RefCell<HashSet<ItemId>>,
    ready: RefCell<HashSet<ItemId>>,
    /// Leaves whose current AppKit frame occupies at least one backing pixel
    /// on each axis. A collapsed deep split is hidden, not promoted to an
    /// arbitrary surface; resize can repopulate this set reversibly.
    paintable: RefCell<HashSet<ItemId>>,
    paintable_changed: Cell<bool>,
    // Host layout calls can re-enter AppKit while applying frame/tree/view
    // mutations. This generation makes the newest nested call authoritative
    // over every older stack frame, including visibility of the stage itself.
    content_update_epoch: Cell<u64>,
    desired_container_visible: Cell<bool>,
    layout_epoch: Cell<u64>,
    // Geometry/visibility native calls can re-enter AppKit more often than
    // the bounded synchronous convergence loop permits. One retained main-
    // queue turn owns the retry; while it is pending the entire stage stays
    // hidden so stale split frames cannot paint beneath newer chrome.
    stage_retry_scheduled: Cell<bool>,
    stage_retry_attempts: Cell<u8>,
    stage_retry_terminal: Cell<bool>,
    geometry_pending: Cell<bool>,
    gap: Cell<f64>,
    drag: RefCell<Option<Divider>>,
    drag_anchor: Cell<(f64, f64)>,
    drag_generation: Cell<u64>,
    drag_observers: RefCell<Option<DragObservers>>,
    split_feedback: RefCell<Option<Retained<NSView>>>,
    feedback_dragging: Cell<bool>,
    divider_tracking: RefCell<Vec<(NSRect, Retained<NSTrackingArea>)>>,
    tracking_busy: Cell<bool>,
    covers: RefCell<HashMap<ItemId, PaintCover>>,
    indicator: RefCell<Option<Retained<NSView>>>,
    on_ratio: RefCell<Option<RatioCallback>>,
    on_stage_failure: RefCell<Option<StageFailureCallback>>,
    /// A slide that keeps the wider, older frame until it arrives, and the
    /// frame it then takes. The generation retires a completion whose slide
    /// a newer layout has already settled.
    held_frame: Cell<Option<NSRect>>,
    motion_generation: Cell<u64>,
}

define_class!(
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[name = "ZephiumContentStage"]
    #[ivars = StageIvars]
    pub struct ContentStage;

    impl ContentStage {
        #[unsafe(method(resizeSubviewsWithOldSize:))]
        fn resize_subviews(&self, _old: NSSize) {
            self.cancel_split_drag(false);
            self.bump_layout_epoch();
            if self.position_panes() && self.ivars().paintable_changed.replace(false) {
                let _ = self.sync_visibility();
            }
        }

        #[unsafe(method(updateTrackingAreas))]
        fn update_tracking_areas(&self) {
            let _: () = unsafe { msg_send![super(self), updateTrackingAreas] };
            self.sync_divider_tracking();
        }

        // Older macOS versions use the legacy native resize cursor selectors.
        #[allow(deprecated)]
        #[unsafe(method(resetCursorRects))]
        fn reset_cursor_rects(&self) {
            let _: () = unsafe { msg_send![super(self), resetCursorRects] };
            let tree = self.ivars().tree.try_borrow().ok().and_then(|tree| tree.as_deref().cloned());
            let h = self.bounds().size.height;
            if let Some(tree) = tree {
                for d in split::dividers(&tree, self.region(), self.ivars().gap.get()).into_iter().take(7) {
                    let cursor = match d.axis { split::Axis::Row => NSCursor::resizeLeftRightCursor(), split::Axis::Col => NSCursor::resizeUpDownCursor() };
                    self.addCursorRect_cursor(NSRect::new(NSPoint::new(d.strip.x, h - d.strip.y - d.strip.height), NSSize::new(d.strip.width, d.strip.height)), &cursor);
                }
            }
        }

        #[unsafe(method(mouseEntered:))]
        fn mouse_entered(&self, event: &NSEvent) { self.update_hover(event); }
        #[unsafe(method(mouseMoved:))]
        fn mouse_moved(&self, event: &NSEvent) { self.update_hover(event); }
        #[unsafe(method(mouseExited:))]
        fn mouse_exited(&self, _event: &NSEvent) {
            if self.ivars().drag.try_borrow().is_ok_and(|drag| drag.is_none()) { self.cancel_split_drag(false); }
        }
        #[unsafe(method(cancelOperation:))]
        fn cancel_operation(&self, _sender: Option<&AnyObject>) { self.cancel_split_drag(false); }
        #[unsafe(method(viewDidMoveToWindow))]
        fn view_did_move_to_window(&self) {
            let _: () = unsafe { msg_send![super(self), viewDidMoveToWindow] };
            // SAFETY: NSView parent-window access is main-thread confined.
            if self.window().is_none() { self.cancel_split_drag(false); }
        }

        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, event: &NSEvent) {
            let _retained = self.retain();
            if self.ivars().stage_retry_terminal.get() { return; }
            self.cancel_split_drag(false);
            let Some(hit) = self.pointer_divider(event) else { return };
            let axis = hit.axis;
            let rect = feedback_rect(&hit, true);
            let (px, py) = self.local_point(event);
            self.ivars().drag_anchor.set((px - hit.strip.x, py - hit.strip.y));
            if let Ok(mut drag) = self.ivars().drag.try_borrow_mut() { *drag = Some(hit); } else { return; }
            if !self.install_drag_observers(axis) { self.cancel_split_drag(false); return; }
            self.show_split_feedback(rect, true);
        }

        #[unsafe(method(mouseDragged:))]
        fn mouse_dragged(&self, event: &NSEvent) {
            let _retained = self.retain();
            if self.ivars().stage_retry_terminal.get() { return; }
            let ivars = self.ivars();
            let Some(grabbed) = ivars.drag.try_borrow().ok().and_then(|drag| drag.clone()) else { return };
            let generation = ivars.drag_generation.get();
            let cursor = ivars.drag_observers.try_borrow().ok().and_then(|observers| observers.as_ref().and_then(|observers| observers.cursor.as_ref()).map(|cursor| cursor.cursor.clone()));
            if let Some(cursor) = cursor { cursor.set(); }
            if ivars.drag_generation.get() != generation { return; }
            let Some(mut preview) = ivars.tree.try_borrow().ok().and_then(|tree| tree.as_deref().cloned()) else { return };
            let (px, py) = self.local_point(event);
            let region = self.region();
            let gap = ivars.gap.get();
            let Some(current) = split::divider_at_path(&preview, region, gap, &grabbed.path) else { self.cancel_split_drag(false); return };
            let ratio = anchored_drag_ratio(&current, gap, px, py, ivars.drag_anchor.get());
            // This clone describes only the guide. The actual pane tree and
            // every WK viewport remain unchanged until mouseUp.
            preview.set_ratio(&grabbed.path, ratio);
            if let Some(target) = split::divider_at_path(&preview, region, gap, &grabbed.path) {
                self.show_split_feedback(feedback_rect(&target, true), true);
            }
        }

        #[unsafe(method(mouseUp:))]
        fn mouse_up(&self, event: &NSEvent) {
            if self.ivars().stage_retry_terminal.get() {
                return;
            }
            let _retained = self.retain();
            let ivars = self.ivars();
            let Some(drag) = ivars
                .drag
                .try_borrow_mut()
                .ok()
                .and_then(|mut drag| drag.take())
            else {
                return;
            };
            // AppKit may deliver the final pointer coordinate in mouseUp
            // without a preceding mouseDragged at that exact position. Land
            // it before publishing the authoritative tree to the shell.
            let (px, py) = self.local_point(event);
            let region = self.region();
            let gap = ivars.gap.get();
            let changed = if let Ok(mut tree) = ivars.tree.try_borrow_mut() {
                tree.as_mut().is_some_and(|tree| {
                    let Some(current) = split::divider_at_path(tree, region, gap, &drag.path) else {
                        return false;
                    };
                    let ratio = anchored_drag_ratio(&current, gap, px, py, ivars.drag_anchor.get());
                    tree.set_ratio(&current.path, ratio);
                    true
                })
            } else {
                false
            };
            if changed { self.bump_layout_epoch(); }
            let epoch = self.ivars().layout_epoch.get();
            let content_epoch = self.ivars().content_update_epoch.get();
            self.cancel_split_drag(true);
            if !changed || !self.layout_epoch_is_current(epoch) { return; }
            if self.position_panes() && self.ivars().paintable_changed.replace(false) {
                let _ = self.sync_visibility();
            }
            if !self.layout_epoch_is_current(epoch) || self.ivars().content_update_epoch.get() != content_epoch { return; }
            let tree = ivars
                .tree
                .try_borrow()
                .ok()
                .and_then(|tree| tree.as_deref().cloned());
            let callback = ivars
                .on_ratio
                .try_borrow()
                .ok()
                .and_then(|callback| callback.clone());
            if let (Some(cb), Some(tree)) = (callback, tree) {
                // The callback may synchronously re-enter stage mutation, so
                // invoke it only after every RefCell guard has been dropped.
                cb(tree);
            }
        }
    }
);

impl ContentStage {
    pub fn new(
        mtm: MainThreadMarker,
        gap: f64,
        on_stage_failure: Box<dyn Fn(usize)>,
    ) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(StageIvars {
            gap: Cell::new(gap),
            on_stage_failure: RefCell::new(Some(Rc::from(on_stage_failure))),
            ..Default::default()
        });
        let this: Retained<Self> = unsafe { msg_send![super(this), init] };
        this.setWantsLayer(true);
        // `addSubview:` is a native painting boundary. A newly-created stage
        // remains fail-closed until one exact layout update finishes.
        this.setHidden(true);
        this
    }

    /// Whether a slide to `frame` can be carried out on the layer: the stage
    /// is on screen and only its horizontal extent changes.
    pub fn can_slide_to(&self, frame: NSRect) -> bool {
        let current = self.motion_target();
        !self.isHidden()
            && current.size.width > 0.0
            && current.origin.y == frame.origin.y
            && current.size.height == frame.size.height
            && current.origin.x != frame.origin.x
    }

    /// The frame the stage is at, or is travelling to.
    pub fn motion_target(&self) -> NSRect {
        self.ivars()
            .held_frame
            .get()
            .unwrap_or_else(|| self.frame())
    }

    /// Moves the stage to `frame` as one journey. The stage keeps whichever
    /// of the two frames is wider for the whole of it — the new one at once
    /// when the content grows, the old one until arrival when it shrinks —
    /// so its pages are laid out at most once and no edge ever opens a gap.
    /// Only the layer's translation animates, which the compositor carries
    /// without asking any page to draw again.
    pub fn slide_to(&self, frame: NSRect) {
        self.settle_motion();
        let current = self.frame();
        let Some(layer) = self.layer() else {
            self.setFrame(frame);
            return;
        };
        let generation = self.ivars().motion_generation.get().wrapping_add(1);
        self.ivars().motion_generation.set(generation);
        let (from, to) = if frame.size.width >= current.size.width {
            self.setFrame(frame);
            (current.origin.x - frame.origin.x, 0.0)
        } else {
            self.ivars().held_frame.set(Some(frame));
            (0.0, frame.origin.x - current.origin.x)
        };
        let animation = translation(from, to, SLIDE_SECONDS);
        let stage = Weak::from_retained(&self.retain());
        let arrived = block2::RcBlock::new(move || {
            if let Some(stage) = stage.load() {
                if stage.ivars().motion_generation.get() == generation {
                    stage.settle_motion();
                }
            }
        });
        CATransaction::begin();
        // SAFETY: the block holds the stage weakly and runs on the main thread.
        unsafe { CATransaction::setCompletionBlock(Some(&arrived)) };
        layer.addAnimation_forKey(&animation, Some(ns_string!("zephium.slide")));
        CATransaction::commit();
    }

    /// Brings the stage back into view after a browser page covered it: it
    /// settles in from a breath smaller and fully transparent, the way a
    /// browser page arrives in chrome.
    pub fn arrive(&self) {
        let Some(layer) = self.layer() else {
            return;
        };
        let bounds = self.bounds();
        let centre = (bounds.size.width / 2.0, bounds.size.height / 2.0);
        let fade = CABasicAnimation::animationWithKeyPath(Some(ns_string!("opacity")));
        let scale = CABasicAnimation::animationWithKeyPath(Some(ns_string!("transform.scale")));
        // Scaling about the layer's origin corner, shifted by exactly the
        // amount that makes it a scale about the centre.
        let shift =
            CABasicAnimation::animationWithKeyPath(Some(ns_string!("transform.translation")));
        // SAFETY: NSNumber and NSValue are the value types these key paths take.
        unsafe {
            fade.setFromValue(Some(&NSNumber::new_f64(0.0)));
            fade.setToValue(Some(&NSNumber::new_f64(1.0)));
            scale.setFromValue(Some(&NSNumber::new_f64(ARRIVE_SCALE)));
            scale.setToValue(Some(&NSNumber::new_f64(1.0)));
            shift.setFromValue(Some(&NSValue::valueWithSize(NSSize::new(
                centre.0 * (1.0 - ARRIVE_SCALE),
                centre.1 * (1.0 - ARRIVE_SCALE),
            ))));
            shift.setToValue(Some(&NSValue::valueWithSize(NSSize::new(0.0, 0.0))));
        }
        fade.setDuration(ARRIVE_SECONDS * 0.7);
        fade.setTimingFunction(Some(&ease_out()));
        for animation in [&scale, &shift] {
            animation.setDuration(ARRIVE_SECONDS);
            animation.setTimingFunction(Some(&emphasized()));
        }
        layer.addAnimation_forKey(&fade, Some(ns_string!("zephium.arrive.fade")));
        layer.addAnimation_forKey(&scale, Some(ns_string!("zephium.arrive.scale")));
        layer.addAnimation_forKey(&shift, Some(ns_string!("zephium.arrive.shift")));
    }

    /// Ends any journey at once: the held frame is taken and the layer's
    /// motion removed in the same transaction, so nothing is seen to jump.
    pub fn settle_motion(&self) {
        self.ivars()
            .motion_generation
            .set(self.ivars().motion_generation.get().wrapping_add(1));
        let held = self.ivars().held_frame.take();
        CATransaction::begin();
        CATransaction::setDisableActions(true);
        if let Some(frame) = held {
            self.setFrame(frame);
        }
        if let Some(layer) = self.layer() {
            layer.removeAnimationForKey(ns_string!("zephium.slide"));
        }
        CATransaction::commit();
    }

    /// Reserve authority for one host layout before performing any AppKit
    /// calls. A nested layout increments this epoch and permanently prevents
    /// the older stack frame from revealing the stage container afterward.
    pub fn begin_content_update(&self, visible: bool) -> Option<u64> {
        if self.ivars().stage_retry_terminal.get() {
            if !self.isHidden() {
                self.setHidden(true);
            }
            return None;
        }
        let epoch = self
            .ivars()
            .content_update_epoch
            .get()
            .wrapping_add(1)
            .max(1);
        self.ivars().content_update_epoch.set(epoch);
        self.ivars().desired_container_visible.set(visible);
        if !visible {
            // Native capture can disappear on hide/minimize without mouseUp.
            self.cancel_split_drag(false);
            if self.content_update_is_current(epoch) {
                self.clear_covers();
            }
        }
        Some(epoch)
    }

    pub fn content_update_is_current(&self, epoch: u64) -> bool {
        !self.ivars().stage_retry_terminal.get() && self.ivars().content_update_epoch.get() == epoch
    }

    pub fn has_terminal_failure(&self) -> bool {
        self.ivars().stage_retry_terminal.get()
    }

    /// Complete the exact update reserved by `begin_content_update`. Stale
    /// outer updates are no-ops; their nested successor already owns the
    /// retained desired state and native convergence obligation.
    pub fn finish_content_update(&self, epoch: u64) -> bool {
        if self.ivars().stage_retry_terminal.get() {
            if !self.isHidden() {
                self.setHidden(true);
            }
            return false;
        }
        if !self.content_update_is_current(epoch) {
            return true;
        }
        let _ = self.sync_container_visibility();
        // A retained main-queue retry is an accepted native obligation, not a
        // terminal application failure. Only exhaustion/panic makes the host
        // seal the engine; the stage remains hidden while a retry is pending.
        !self.ivars().stage_retry_terminal.get()
    }

    /// An exact layout whose native children could not be established must
    /// never leave the prior stage visible under newer browser chrome.
    pub fn abort_content_update(&self, epoch: u64) {
        if self.content_update_is_current(epoch) {
            self.ivars().desired_container_visible.set(false);
            let superseding = self
                .ivars()
                .content_update_epoch
                .get()
                .wrapping_add(1)
                .max(1);
            self.ivars().content_update_epoch.set(superseding);
            self.cancel_split_drag(false);
        }
        let _ = self.sync_container_visibility();
    }

    pub fn set_tree(&self, tree: Option<Pane>) -> bool {
        if self.ivars().stage_retry_terminal.get() {
            return false;
        }
        let Ok(mut current) = self.ivars().tree.try_borrow_mut() else {
            return false;
        };
        let next = tree.map(Box::new);
        if current.as_deref() == next.as_deref() {
            drop(current);
            let _ = self.position_panes();
            return !self.ivars().stage_retry_terminal.get();
        }
        let topology_changed = match (current.as_deref(), next.as_deref()) {
            (Some(current), Some(next)) => !current.same_topology(next),
            (None, None) => false,
            (Some(_), None) | (None, Some(_)) => true,
        };
        *current = next;
        drop(current);
        self.bump_layout_epoch();
        if topology_changed {
            self.cancel_split_drag(false);
        }
        let _ = self.position_panes();
        !self.ivars().stage_retry_terminal.get()
    }

    pub fn set_on_ratio(&self, f: Box<dyn Fn(Pane)>) {
        if self.ivars().stage_retry_terminal.get() {
            return;
        }
        if let Ok(mut callback) = self.ivars().on_ratio.try_borrow_mut() {
            *callback = Some(Rc::from(f));
        }
    }

    pub fn has_view(&self, id: ItemId) -> bool {
        self.ivars()
            .views
            .try_borrow()
            .is_ok_and(|views| views.contains_key(&id))
    }

    pub fn wants_visible(&self, id: ItemId) -> bool {
        self.ivars().desired_container_visible.get()
            && self
                .ivars()
                .visible
                .try_borrow()
                .map_or(true, |ids| ids.contains(&id))
    }

    /// A transient visual cover never grants document presentation authority.
    /// It is installed while the shared document permit is still revoked.
    pub fn cover(&self, id: ItemId, token: u64, image: Option<&CGImage>) -> bool {
        if self.ivars().stage_retry_terminal.get() || !self.wants_visible(id) {
            return false;
        }
        let epoch = self.ivars().layout_epoch.get();
        let Some(host) = self
            .ivars()
            .views
            .try_borrow()
            .ok()
            .and_then(|v| v.get(&id).cloned())
        else {
            return false;
        };
        // SAFETY: retained parent access on the AppKit main thread.
        if !unsafe { host.view.superview() }
            .as_deref()
            .is_some_and(|p| std::ptr::eq(p, &**self))
        {
            return false;
        }
        // This sibling absorbs pointer/scroll input until real page pixels
        // are ready. It never takes keyboard focus away from browser chrome.
        // SAFETY: standard NSView initialization of our main-thread subclass.
        let cover: Retained<PaintCoverView> =
            unsafe { msg_send![PaintCoverView::alloc(self.mtm()), init] };
        let view = cover.into_super();
        view.setWantsLayer(true);
        view.setTranslatesAutoresizingMaskIntoConstraints(false);
        view.setFrame(host.view.frame());
        if let Some(layer) = view.layer() {
            layer.setBackgroundColor(Some(&page_ground(self).CGColor()));
            if let Some(image) = image {
                let contents: &AnyObject = image.as_ref();
                // SAFETY: a CGImage is valid layer contents; the layer retains it.
                unsafe { layer.setContents(Some(contents)) };
                // SAFETY: immutable framework constant.
                layer.setContentsGravity(unsafe { objc2_quartz_core::kCAGravityResizeAspectFill });
            }
            if let Some(page_layer) = host.view.layer() {
                layer.setCornerRadius(page_layer.cornerRadius());
                // SAFETY: immutable framework constant.
                layer.setCornerCurve(unsafe { objc2_quartz_core::kCACornerCurveContinuous });
            }
            layer.setMasksToBounds(true);
        }
        if !self.layout_epoch_is_current(epoch) || !self.wants_visible(id) {
            return false;
        }
        // SAFETY: retained parent access on the AppKit main thread. Native
        // frame/layer calls above may have transferred this page elsewhere.
        if !unsafe { host.view.superview() }
            .as_deref()
            .is_some_and(|parent| std::ptr::eq(parent, &**self))
            || !self.layout_epoch_is_current(epoch)
        {
            return false;
        }
        let Ok(mut covers) = self.ivars().covers.try_borrow_mut() else {
            return false;
        };
        let previous = covers.insert(
            id,
            PaintCover {
                token,
                view: view.clone(),
                fades: image.is_some(),
            },
        );
        drop(covers);
        self.addSubview_positioned_relativeTo(&view, NSWindowOrderingMode::Above, Some(&host.view));
        if let Some(previous) = previous {
            previous.view.removeFromSuperview();
        }
        if !self.layout_epoch_is_current(epoch) || !self.wants_visible(id) {
            self.uncover(id, token);
            return false;
        }
        let retained = self
            .ivars()
            .covers
            .try_borrow()
            .is_ok_and(|covers| covers.get(&id).is_some_and(|cover| cover.token == token));
        if !retained {
            view.removeFromSuperview();
        }
        retained
    }

    pub fn uncover(&self, id: ItemId, token: u64) {
        let view = self
            .ivars()
            .covers
            .try_borrow_mut()
            .ok()
            .and_then(|mut covers| {
                if covers.get(&id).is_some_and(|cover| cover.token == token) {
                    covers.remove(&id)
                } else {
                    None
                }
            });
        if let Some(cover) = view {
            if cover.fades {
                fade_out(&cover.view);
            } else {
                cover.view.removeFromSuperview();
            }
        }
    }

    fn clear_cover(&self, id: ItemId) {
        let cover = self
            .ivars()
            .covers
            .try_borrow_mut()
            .ok()
            .and_then(|mut covers| covers.remove(&id));
        if let Some(cover) = cover {
            cover.view.removeFromSuperview();
        }
    }

    fn clear_covers(&self) {
        let covers = self
            .ivars()
            .covers
            .try_borrow_mut()
            .ok()
            .map(|mut covers| std::mem::take(&mut *covers));
        if let Some(covers) = covers {
            for cover in covers.into_values() {
                cover.view.removeFromSuperview();
            }
        }
    }

    /// Returns whether the latest authoritative layout expects this item.
    /// A view can be constructed after that layout task ran, so creation uses
    /// this retained model to attach the late native child without waiting for
    /// another resize or user interaction.
    pub fn allows_download_decision(&self, id: ItemId) -> bool {
        !self.ivars().stage_retry_terminal.get()
            && self.ivars().desired_container_visible.get()
            && self.contains_item(id)
    }

    pub fn contains_item(&self, id: ItemId) -> bool {
        self.ivars()
            .tree
            .try_borrow()
            .is_ok_and(|tree| tree.as_deref().is_some_and(|tree| tree.contains(id)))
    }

    pub fn attached_items(&self) -> Option<Vec<ItemId>> {
        self.ivars()
            .views
            .try_borrow()
            .ok()
            .map(|views| views.keys().copied().collect())
    }

    pub fn retire(&self) {
        self.ivars().stage_retry_terminal.set(true);
        self.cancel_split_drag(false);
        self.ivars().stage_retry_scheduled.set(false);
        self.setHidden(true);
        self.removeFromSuperview();
    }

    pub fn insert_view(
        &self,
        id: ItemId,
        view: Retained<NSView>,
        presentation_permit: Arc<AtomicBool>,
    ) -> bool {
        if self.ivars().stage_retry_terminal.get() {
            return false;
        }
        // A page WebKit is showing fullscreen is registered but left where it
        // is; `adopt_after_fullscreen` brings it in once WebKit lets go.
        let presenting = super::fullscreen::in_transition(&view);
        // `addSubview:` may paint synchronously. Hide before attaching so a
        // new WKWebView cannot expose its default white backing store between
        // construction and the first attributed, chrome-verified document.
        if !presenting {
            view.setHidden(true);
        }
        let Ok(mut ready) = self.ivars().ready.try_borrow_mut() else {
            return false;
        };
        ready.remove(&id);
        drop(ready);
        let Ok(mut views) = self.ivars().views.try_borrow_mut() else {
            return false;
        };
        views.insert(
            id,
            HostView {
                view: view.clone(),
                presentation_permit,
            },
        );
        drop(views);
        self.bump_layout_epoch();
        // `addSubview:` can synchronously enter AppKit callbacks. Native work
        // happens only after the view registry borrow has been released.
        if !presenting {
            self.addSubview(&view);
        }
        if self.ivars().stage_retry_terminal.get() {
            return false;
        }
        let _ = self.position_panes();
        let _ = self.sync_visibility();
        !self.ivars().stage_retry_terminal.get()
    }

    pub fn remove_view(&self, id: ItemId) {
        self.clear_cover(id);
        if self.ivars().stage_retry_terminal.get() && !self.isHidden() {
            self.setHidden(true);
        }
        if let Ok(mut visible) = self.ivars().visible.try_borrow_mut() {
            visible.remove(&id);
        }
        if let Ok(mut ready) = self.ivars().ready.try_borrow_mut() {
            ready.remove(&id);
        }
        if let Ok(mut paintable) = self.ivars().paintable.try_borrow_mut() {
            paintable.remove(&id);
        }
        let view = self
            .ivars()
            .views
            .try_borrow_mut()
            .ok()
            .and_then(|mut views| views.remove(&id));
        if let Some(view) = view {
            self.bump_layout_epoch();
            // Pulling a page out of WebKit's fullscreen window would strand
            // that window; the host lets WebKit hand it back before teardown.
            if !super::fullscreen::webkit_owns(&view.view, self) {
                view.view.removeFromSuperview();
            }
        }
    }

    /// Takes a page back after WebKit's fullscreen window let go of it and
    /// lays it out again. WebKit normally returns it to this stage itself.
    pub fn adopt_after_fullscreen(&self, id: ItemId) -> bool {
        if self.ivars().stage_retry_terminal.get() {
            return false;
        }
        let Some(view) = self
            .ivars()
            .views
            .try_borrow()
            .ok()
            .and_then(|views| views.get(&id).map(|view| view.view.clone()))
        else {
            return true;
        };
        if super::fullscreen::in_transition(&view) {
            return true;
        }
        // SAFETY: retained parent access on the AppKit main thread.
        let home = unsafe { view.superview() }
            .as_deref()
            .is_some_and(|parent| std::ptr::eq(parent, &**self));
        if !home {
            view.setHidden(true);
            let cover = self
                .ivars()
                .covers
                .try_borrow()
                .ok()
                .and_then(|covers| covers.get(&id).map(|cover| cover.view.clone()));
            self.addSubview_positioned_relativeTo(
                &view,
                NSWindowOrderingMode::Below,
                cover.as_deref(),
            );
        }
        self.bump_layout_epoch();
        let _ = self.position_panes();
        let _ = self.sync_visibility();
        !self.ivars().stage_retry_terminal.get()
    }

    pub fn set_visible(&self, visible: &[ItemId]) -> bool {
        if self.ivars().stage_retry_terminal.get() {
            return false;
        }
        let next = visible.iter().copied().collect::<HashSet<_>>();
        let removed_covers = self
            .ivars()
            .covers
            .try_borrow()
            .ok()
            .map(|covers| {
                covers
                    .keys()
                    .copied()
                    .filter(|id| !next.contains(id))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        for id in removed_covers {
            self.clear_cover(id);
        }
        let Some(changed) = self
            .ivars()
            .visible
            .try_borrow_mut()
            .ok()
            .map(|mut current| {
                let changed = *current != next;
                *current = next.clone();
                changed
            })
        else {
            return false;
        };
        if changed {
            self.bump_layout_epoch();
        }
        // Run even for an identical logical value: a prior AppKit re-entry
        // may have forced a fail-closed hide before the newest state settled.
        let _ = self.sync_visibility();
        !self.ivars().stage_retry_terminal.get()
    }

    /// Reveal one exact raw-view generation after privileged chrome verified
    /// its attributed URL/revision and the shell returned the same identity.
    pub fn set_ready(&self, id: ItemId) -> bool {
        if self.ivars().stage_retry_terminal.get() {
            return false;
        }
        let newly_ready = match self.ivars().ready.try_borrow_mut() {
            Ok(mut ready) => ready.insert(id),
            Err(_) => return false,
        };
        if !newly_ready {
            let _ = self.sync_visibility();
            return !self.ivars().stage_retry_terminal.get();
        }
        self.bump_layout_epoch();
        let _ = self.sync_visibility();
        !self.ivars().stage_retry_terminal.get()
    }

    /// Re-arm the presentation barrier for a newly committed main-frame
    /// document. The stage retains this logical state before native sync, so
    /// a later resize/layout pass cannot reveal the new pixels using the
    /// previous document's readiness acknowledgement.
    pub fn set_pending(&self, id: ItemId) -> bool {
        self.clear_cover(id);
        if self.ivars().stage_retry_terminal.get() {
            if !self.isHidden() {
                self.setHidden(true);
            }
            return false;
        }
        let attached = match self.ivars().views.try_borrow() {
            Ok(views) => views.contains_key(&id),
            Err(_) => return false,
        };
        if !attached {
            return true;
        }
        if let Ok(views) = self.ivars().views.try_borrow() {
            if let Some(view) = views.get(&id) {
                view.presentation_permit.store(false, Ordering::Release);
            }
        }
        let removed = match self.ivars().ready.try_borrow_mut() {
            Ok(mut ready) => ready.remove(&id),
            Err(_) => return false,
        };
        if removed {
            self.bump_layout_epoch();
        }
        // Run even when an identical pending value was already retained: a
        // prior AppKit re-entry may have interrupted the fail-closed hide.
        let _ = self.sync_visibility();
        !self.ivars().stage_retry_terminal.get()
    }

    pub fn set_drop_indicator(&self, zone: Option<Rect>) {
        if self.ivars().stage_retry_terminal.get() {
            return;
        }
        match zone {
            None => {
                let view = self
                    .ivars()
                    .indicator
                    .try_borrow_mut()
                    .ok()
                    .and_then(|mut indicator| indicator.take());
                if let Some(view) = view {
                    view.removeFromSuperview();
                }
            }
            Some(z) => {
                let existing = self
                    .ivars()
                    .indicator
                    .try_borrow()
                    .ok()
                    .and_then(|indicator| indicator.clone());
                let view = if let Some(view) = existing {
                    view
                } else {
                    let candidate = self.make_indicator();
                    self.addSubview(&candidate);
                    let Ok(mut indicator) = self.ivars().indicator.try_borrow_mut() else {
                        candidate.removeFromSuperview();
                        return;
                    };
                    if let Some(installed) = indicator.as_ref() {
                        let installed = installed.clone();
                        drop(indicator);
                        candidate.removeFromSuperview();
                        installed
                    } else {
                        *indicator = Some(candidate.clone());
                        candidate
                    }
                };
                let h = self.bounds().size.height;
                view.setFrame(NSRect::new(
                    NSPoint::new(z.x, h - z.y - z.height),
                    NSSize::new(z.width, z.height),
                ));
            }
        }
    }

    fn make_indicator(&self) -> Retained<NSView> {
        let v = NSView::new(self.mtm());
        v.setWantsLayer(true);
        if let Some(layer) = v.layer() {
            let fill = NSColor::colorWithWhite_alpha(1.0, 0.12);
            let border = NSColor::colorWithWhite_alpha(1.0, 0.42);
            layer.setBackgroundColor(Some(&fill.CGColor()));
            layer.setBorderColor(Some(&border.CGColor()));
            layer.setBorderWidth(1.5);
            layer.setCornerRadius(10.0);
        }
        v
    }

    fn region(&self) -> Rect {
        let b = self.bounds();
        Rect::new(0.0, 0.0, b.size.width, b.size.height)
    }

    fn local_point(&self, event: &NSEvent) -> (f64, f64) {
        let win = event.locationInWindow();
        let local = self.convertPoint_fromView(win, None);
        (local.x, self.bounds().size.height - local.y)
    }

    fn pointer_divider(&self, event: &NSEvent) -> Option<Divider> {
        let (x, y) = self.local_point(event);
        let tree = self.ivars().tree.try_borrow().ok()?.as_deref()?.clone();
        split::divider_at(&tree, self.region(), self.ivars().gap.get(), x, y)
    }

    fn update_hover(&self, event: &NSEvent) {
        if self
            .ivars()
            .drag
            .try_borrow()
            .map_or(true, |drag| drag.is_some())
        {
            return;
        }
        if !self.ivars().desired_container_visible.get() || self.isHidden() {
            self.cancel_split_drag(false);
            return;
        }
        if let Some(divider) = self.pointer_divider(event) {
            self.show_split_feedback(feedback_rect(&divider, false), false);
        } else {
            self.cancel_split_drag(false);
        }
    }

    fn show_split_feedback(&self, rect: Rect, dragging: bool) {
        let generation = self.ivars().drag_generation.get();
        let existing = self
            .ivars()
            .split_feedback
            .try_borrow()
            .ok()
            .and_then(|view| view.clone());
        let created = existing.is_none();
        let view = if let Some(view) = existing {
            view
        } else {
            // SAFETY: standard initialization of a main-thread NSView subclass.
            let view: Retained<DividerFeedbackView> =
                unsafe { msg_send![DividerFeedbackView::alloc(self.mtm()), init] };
            let view = view.into_super();
            view.setWantsLayer(true);
            view.setTranslatesAutoresizingMaskIntoConstraints(false);
            if self.ivars().drag_generation.get() != generation
                || !self.ivars().desired_container_visible.get()
            {
                return;
            }
            let Ok(mut feedback) = self.ivars().split_feedback.try_borrow_mut() else {
                return;
            };
            if feedback.is_some() {
                return;
            }
            *feedback = Some(view.clone());
            drop(feedback);
            self.addSubview_positioned_relativeTo(&view, NSWindowOrderingMode::Above, None);
            view
        };
        if !self.feedback_is_current(&view, generation) {
            view.removeFromSuperview();
            return;
        }
        CATransaction::begin();
        CATransaction::setDisableActions(true);
        if let Some(layer) = view.layer() {
            let changed_mode = self.ivars().feedback_dragging.replace(dragging) != dragging;
            if created || changed_mode {
                let paint = block2::RcBlock::new(|| {
                    let color = if dragging {
                        NSColor::labelColor()
                    } else {
                        NSColor::secondaryLabelColor()
                    };
                    layer.setBackgroundColor(Some(&color.CGColor()));
                });
                self.effectiveAppearance()
                    .performAsCurrentDrawingAppearance(&paint);
                layer.setCornerRadius(1.5);
            }
            layer.setOpacity(if dragging { 0.9 } else { 0.65 });
        }
        let h = self.bounds().size.height;
        view.setFrame(NSRect::new(
            NSPoint::new(rect.x, h - rect.y - rect.height),
            NSSize::new(rect.width, rect.height),
        ));
        CATransaction::commit();
        if !self.feedback_is_current(&view, generation) {
            view.removeFromSuperview();
        }
    }

    fn feedback_is_current(&self, view: &NSView, generation: u64) -> bool {
        self.ivars().drag_generation.get() == generation
            && self.ivars().desired_container_visible.get()
            && !self.ivars().stage_retry_terminal.get()
            && self
                .ivars()
                .split_feedback
                .try_borrow()
                .is_ok_and(|feedback| {
                    feedback
                        .as_deref()
                        .is_some_and(|current| std::ptr::eq(current, view))
                })
    }

    fn cancel_split_drag(&self, animate: bool) {
        if let Ok(mut drag) = self.ivars().drag.try_borrow_mut() {
            drag.take();
        }
        self.ivars()
            .drag_generation
            .set(self.ivars().drag_generation.get().wrapping_add(1));
        let feedback = self
            .ivars()
            .split_feedback
            .try_borrow_mut()
            .ok()
            .and_then(|mut view| view.take());
        let observers = self
            .ivars()
            .drag_observers
            .try_borrow_mut()
            .ok()
            .and_then(|mut observers| observers.take());
        // Native deregistration happens after every Rust capture has retired.
        drop(observers);
        if let Some(view) = feedback {
            if !animate || NSWorkspace::sharedWorkspace().accessibilityDisplayShouldReduceMotion() {
                view.removeFromSuperview();
            } else if let Some(layer) = view.layer() {
                let done = view.clone();
                let completed = block2::RcBlock::new(move || {
                    done.removeFromSuperview();
                });
                let fade = CABasicAnimation::animationWithKeyPath(Some(ns_string!("opacity")));
                // SAFETY: opacity takes NSNumber values; AppKit copies the completion block.
                unsafe {
                    fade.setFromValue(Some(&NSNumber::new_f64(layer.opacity() as f64)));
                    fade.setToValue(Some(&NSNumber::new_f64(0.0)));
                }
                fade.setDuration(0.10);
                CATransaction::begin();
                CATransaction::setDisableActions(true);
                unsafe {
                    CATransaction::setCompletionBlock(Some(&completed));
                }
                layer.setOpacity(0.0);
                layer.addAnimation_forKey(&fade, Some(ns_string!("zephium.split-guide.fade")));
                CATransaction::commit();
            } else {
                view.removeFromSuperview();
            }
        }
    }

    fn install_drag_observers(&self, axis: split::Axis) -> bool {
        // SAFETY: native window access is confined to the AppKit main thread.
        let Some(window) = self.window() else {
            return false;
        };
        let generation = self.ivars().drag_generation.get();
        let weak = Weak::from_retained(&self.retain());
        let escape = block2::RcBlock::new(move |event: NonNull<NSEvent>| -> *mut NSEvent {
            // SAFETY: AppKit lends the event for the duration of this local callback.
            let borrowed = unsafe { event.as_ref() };
            if borrowed.keyCode() == 53 {
                let canceled = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    if let Some(stage) = weak.load() {
                        if stage.ivars().drag_generation.get() == generation {
                            stage.cancel_split_drag(false);
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
        // SAFETY: a local, main-thread event monitor requires no global input permission.
        let Some(escape) = (unsafe {
            NSEvent::addLocalMonitorForEventsMatchingMask_handler(NSEventMask::KeyDown, &escape)
        }) else {
            return false;
        };
        let weak = MainThreadBound::new(Weak::from_retained(&self.retain()), self.mtm());
        let resigned = block2::RcBlock::new(move |_: NonNull<NSNotification>| {
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let Some(mtm) = MainThreadMarker::new() else {
                    return;
                };
                if let Some(stage) = weak.get(mtm).load() {
                    if stage.ivars().drag_generation.get() == generation {
                        stage.cancel_split_drag(false);
                    }
                }
            }));
        });
        // SAFETY: public window notification; the copied block holds only a main-thread weak stage.
        let resigned = unsafe {
            NSNotificationCenter::defaultCenter().addObserverForName_object_queue_usingBlock(
                Some(NSWindowDidResignKeyNotification),
                Some(&window),
                None,
                &resigned,
            )
        };
        let observers = DragObservers {
            escape,
            resigned,
            cursor: Some(DragCursor::new(window, axis)),
        };
        if self.ivars().drag_generation.get() != generation {
            return false;
        }
        let Ok(mut retained) = self.ivars().drag_observers.try_borrow_mut() else {
            return false;
        };
        *retained = Some(observers);
        true
    }

    fn sync_divider_tracking(&self) {
        if self.ivars().tracking_busy.replace(true) {
            return;
        }
        struct Reset<'a>(&'a Cell<bool>);
        impl Drop for Reset<'_> {
            fn drop(&mut self) {
                self.0.set(false);
            }
        }
        let _reset = Reset(&self.ivars().tracking_busy);
        let tree = self
            .ivars()
            .tree
            .try_borrow()
            .ok()
            .and_then(|tree| tree.as_deref().cloned());
        let h = self.bounds().size.height;
        let desired: Vec<_> = tree
            .map(|tree| split::dividers(&tree, self.region(), self.ivars().gap.get()))
            .unwrap_or_default()
            .into_iter()
            .take(7)
            .filter(|d| d.strip.width > 0.0 && d.strip.height > 0.0)
            .map(|d| {
                NSRect::new(
                    NSPoint::new(d.strip.x, h - d.strip.y - d.strip.height),
                    NSSize::new(d.strip.width, d.strip.height),
                )
            })
            .collect();
        if self
            .ivars()
            .divider_tracking
            .try_borrow()
            .is_ok_and(|areas| {
                areas
                    .iter()
                    .map(|(rect, _)| *rect)
                    .eq(desired.iter().copied())
            })
        {
            return;
        }
        let Some(old) = self
            .ivars()
            .divider_tracking
            .try_borrow_mut()
            .ok()
            .map(|mut areas| std::mem::take(&mut *areas))
        else {
            return;
        };
        for (_, area) in old {
            self.removeTrackingArea(&area);
        }
        let mut added = Vec::new();
        for rect in desired {
            let options = NSTrackingAreaOptions::MouseEnteredAndExited
                | NSTrackingAreaOptions::MouseMoved
                | NSTrackingAreaOptions::ActiveInKeyWindow;
            // SAFETY: main-thread NSView owner implements the requested mouse callbacks.
            let area = unsafe {
                NSTrackingArea::initWithRect_options_owner_userInfo(
                    NSTrackingArea::alloc(),
                    rect,
                    options,
                    Some(self),
                    None,
                )
            };
            self.addTrackingArea(&area);
            added.push((rect, area));
        }
        if let Ok(mut areas) = self.ivars().divider_tracking.try_borrow_mut() {
            *areas = added;
        }
    }

    fn position_panes(&self) -> bool {
        if self.ivars().stage_retry_terminal.get() {
            if !self.isHidden() {
                self.setHidden(true);
            }
            return false;
        }
        // A frame mutation can synchronously re-enter AppKit. Snapshot all
        // Rust ownership first, release RefCell guards, and retry from the
        // latest model if re-entry changed the tree/view generation. Never
        // touch another child after one native call supersedes this snapshot.
        'attempt: for _ in 0..4 {
            let ivars = self.ivars();
            let epoch = ivars.layout_epoch.get();
            let Some(views) = ivars.views.try_borrow().ok().map(|views| views.clone()) else {
                self.defer_stage_retry(true);
                return false;
            };
            let Some(tree) = ivars
                .tree
                .try_borrow()
                .ok()
                .and_then(|tree| tree.as_deref().cloned())
            else {
                let Ok(mut paintable) = ivars.paintable.try_borrow_mut() else {
                    self.defer_stage_retry(true);
                    return false;
                };
                let changed = !paintable.is_empty();
                paintable.clear();
                ivars.paintable_changed.set(changed);
                ivars.geometry_pending.set(false);
                return true;
            };
            let gap = ivars.gap.get();
            let bounds = self.bounds();
            if !self.layout_epoch_is_current(epoch) {
                continue 'attempt;
            }
            let region = Rect::new(0.0, 0.0, bounds.size.width, bounds.size.height);
            let mut paintable = HashSet::new();
            for (id, r) in split::layout(&tree, region, gap) {
                if let Some(view) = views.get(&id) {
                    if !self.layout_epoch_is_current(epoch) {
                        continue 'attempt;
                    }
                    let frame = NSRect::new(
                        NSPoint::new(r.x, bounds.size.height - r.y - r.height),
                        NSSize::new(r.width, r.height),
                    );
                    let backing = self.convertSizeToBacking(NSSize::new(r.width, r.height));
                    if rounded_native_size(backing.width, backing.height, 1.0).is_some() {
                        paintable.insert(id);
                    }
                    if super::fullscreen::webkit_owns(&view.view, self) {
                        continue;
                    }
                    let current = view.view.frame();
                    if !self.layout_epoch_is_current(epoch) {
                        continue 'attempt;
                    }
                    if !same_rect(current, frame) {
                        view.view.setFrame(frame);
                        if !self.layout_epoch_is_current(epoch) {
                            continue 'attempt;
                        }
                    }
                    let cover = ivars
                        .covers
                        .try_borrow()
                        .ok()
                        .and_then(|covers| covers.get(&id).cloned());
                    if let Some(cover) = cover {
                        cover.view.setFrame(frame);
                        if !self.layout_epoch_is_current(epoch) {
                            continue 'attempt;
                        }
                    }
                }
            }
            if self.layout_epoch_is_current(epoch) {
                let Ok(mut current) = self.ivars().paintable.try_borrow_mut() else {
                    self.defer_stage_retry(true);
                    return false;
                };
                let changed = *current != paintable;
                *current = paintable;
                drop(current);
                self.ivars().paintable_changed.set(changed);
                self.ivars().geometry_pending.set(false);
                self.sync_divider_tracking();
                // SAFETY: native cursor invalidation uses this main-thread view.
                if let Some(window) = self.window() {
                    window.invalidateCursorRectsForView(self);
                }
                if !self.layout_epoch_is_current(epoch) {
                    continue 'attempt;
                }
                return true;
            }
        }
        self.defer_stage_retry(true);
        false
    }

    fn sync_visibility(&self) -> bool {
        if self.ivars().stage_retry_terminal.get() {
            if !self.isHidden() {
                self.setHidden(true);
            }
            return false;
        }
        // `setHidden:` may synchronously enter AppKit. Apply all removals
        // before additions, and never show from a snapshot whose epoch was
        // superseded during native work. A retry observes the newest model;
        // exhaustion leaves uncertain views hidden until the next sync.
        for _ in 0..4 {
            let ivars = self.ivars();
            let epoch = ivars.layout_epoch.get();
            let Some(visible) = ivars.visible.try_borrow().ok().map(|set| set.clone()) else {
                self.defer_stage_retry(false);
                return false;
            };
            let Some(ready) = ivars.ready.try_borrow().ok().map(|set| set.clone()) else {
                self.defer_stage_retry(false);
                return false;
            };
            let Some(paintable) = ivars.paintable.try_borrow().ok().map(|set| set.clone()) else {
                self.defer_stage_retry(false);
                return false;
            };
            let Some(mut views) = ivars.views.try_borrow().ok().map(|views| {
                views
                    .iter()
                    .map(|(id, view)| (*id, view.clone()))
                    .collect::<Vec<_>>()
            }) else {
                self.defer_stage_retry(false);
                return false;
            };
            views.sort_by_key(|(id, _)| *id);

            let mut superseded = false;
            for (_, view) in views.iter().filter(|(id, view)| {
                !visible.contains(id)
                    || !ready.contains(id)
                    || !paintable.contains(id)
                    || !view.presentation_permit.load(Ordering::Acquire)
            }) {
                if super::fullscreen::webkit_owns(&view.view, self) {
                    continue;
                }
                if !view.view.isHidden() {
                    view.view.setHidden(true);
                }
                if !self.layout_epoch_is_current(epoch) {
                    superseded = true;
                    break;
                }
            }
            if superseded {
                continue;
            }

            for (id, view) in views.iter().filter(|(id, _)| {
                visible.contains(id) && ready.contains(id) && paintable.contains(id)
            }) {
                // Revalidate immediately before every reveal. This prevents
                // an outer stale pass from undoing a nested newer hide.
                let still_current = self.layout_epoch_is_current(epoch)
                    && view.presentation_permit.load(Ordering::Acquire)
                    && self
                        .ivars()
                        .visible
                        .try_borrow()
                        .is_ok_and(|current| current.contains(id))
                    && self
                        .ivars()
                        .ready
                        .try_borrow()
                        .is_ok_and(|current| current.contains(id))
                    && self
                        .ivars()
                        .paintable
                        .try_borrow()
                        .is_ok_and(|current| current.contains(id));
                if !still_current {
                    superseded = true;
                    break;
                }
                if super::fullscreen::webkit_owns(&view.view, self) {
                    continue;
                }
                let hidden = view.view.isHidden();
                if !self.layout_epoch_is_current(epoch)
                    || !view.presentation_permit.load(Ordering::Acquire)
                {
                    superseded = true;
                    break;
                }
                if hidden {
                    view.view.setHidden(false);
                }
                if !self.layout_epoch_is_current(epoch)
                    || !view.presentation_permit.load(Ordering::Acquire)
                {
                    // `setHidden(false)` may pump a native commit callback.
                    // A permit revoked during that re-entry wins before this
                    // outer pass returns to AppKit for painting.
                    if !view.view.isHidden() {
                        view.view.setHidden(true);
                    }
                    superseded = true;
                    break;
                }
            }
            if !superseded && self.layout_epoch_is_current(epoch) {
                return true;
            }
        }

        // Native re-entry kept changing ownership. Enforce the part that is
        // always safe from the latest model; a later identical sync may show
        // the now-settled desired leaves.
        let visible = self
            .ivars()
            .visible
            .try_borrow()
            .ok()
            .map(|set| set.clone())
            .unwrap_or_default();
        let ready = self
            .ivars()
            .ready
            .try_borrow()
            .ok()
            .map(|set| set.clone())
            .unwrap_or_default();
        let paintable = self
            .ivars()
            .paintable
            .try_borrow()
            .ok()
            .map(|set| set.clone())
            .unwrap_or_default();
        if let Ok(views) = self.ivars().views.try_borrow() {
            for (id, view) in views.iter() {
                if !view.view.isHidden()
                    && !super::fullscreen::webkit_owns(&view.view, self)
                    && (!visible.contains(id)
                        || !ready.contains(id)
                        || !paintable.contains(id)
                        || !view.presentation_permit.load(Ordering::Acquire))
                {
                    view.view.setHidden(true);
                }
            }
        }
        self.defer_stage_retry(false);
        false
    }

    fn defer_stage_retry(&self, geometry: bool) {
        if geometry {
            self.ivars().geometry_pending.set(true);
        }
        if self.ivars().stage_retry_terminal.get() {
            if !self.isHidden() {
                self.setHidden(true);
            }
            return;
        }
        let already_scheduled = self.ivars().stage_retry_scheduled.replace(true);
        // Conceal the container before queueing. A stale geometry snapshot is
        // unsafe even when every child still has a valid navigation permit.
        if !self.isHidden() {
            self.setHidden(true);
        }
        if self.ivars().stage_retry_terminal.get() {
            self.ivars().stage_retry_scheduled.set(false);
            return;
        }
        if already_scheduled {
            return;
        }
        let attempts = self.ivars().stage_retry_attempts.get();
        if attempts >= MAX_ASYNC_STAGE_RETRIES {
            self.fail_stage_retry_terminal();
            return;
        }
        self.ivars()
            .stage_retry_attempts
            .set(attempts.saturating_add(1));
        let Some(mtm) = MainThreadMarker::new() else {
            // ContentStage is MainThreadOnly, so this is an invariant guard.
            // Retain the pending bit and hidden container if it is violated.
            return;
        };
        let stage = MainThreadBound::new(self.retain(), mtm);
        DispatchQueue::main().exec_async(move || {
            // libdispatch callbacks have a C ABI; native re-entry must never
            // let a Rust unwind cross that boundary.
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let Some(mtm) = MainThreadMarker::new() else {
                    return false;
                };
                let stage = stage.get(mtm);
                stage.ivars().stage_retry_scheduled.set(false);
                if stage.position_panes() && stage.sync_visibility() {
                    return stage.sync_container_visibility();
                }
                false
            }));
            let Some(mtm) = MainThreadMarker::new() else {
                return;
            };
            let stage = stage.get(mtm);
            match outcome {
                Ok(true) => stage.ivars().stage_retry_attempts.set(0),
                Ok(false) => {}
                Err(_) => stage.fail_stage_retry_terminal(),
            }
        });
    }

    fn fail_stage_retry_terminal(&self) {
        if self.ivars().stage_retry_terminal.replace(true) {
            return;
        }
        self.ivars().stage_retry_scheduled.set(false);
        if !self.isHidden() {
            self.setHidden(true);
        }
        let callback = self
            .ivars()
            .on_stage_failure
            .try_borrow()
            .ok()
            .and_then(|callback| callback.clone());
        if let Some(callback) = callback {
            callback(self as *const Self as usize);
        }
    }

    fn sync_container_visibility(&self) -> bool {
        if self.ivars().stage_retry_terminal.get() {
            if !self.isHidden() {
                self.setHidden(true);
            }
            return false;
        }
        // `setHidden:` can pump AppKit. Revalidate the exact desired-layout
        // generation before and after a reveal; on any mismatch conceal first
        // and retry from the newest retained fact.
        for _ in 0..4 {
            let epoch = self.ivars().content_update_epoch.get();
            let desired = self.ivars().desired_container_visible.get();
            let presentation_safe = !self.ivars().stage_retry_terminal.get()
                && !self.ivars().geometry_pending.get()
                && !self.ivars().stage_retry_scheduled.get();
            if !desired || !presentation_safe {
                if !self.isHidden() {
                    self.setHidden(true);
                }
                if self.ivars().content_update_epoch.get() == epoch
                    && (!self.ivars().desired_container_visible.get()
                        || self.ivars().geometry_pending.get()
                        || self.ivars().stage_retry_scheduled.get()
                        || self.ivars().stage_retry_terminal.get())
                {
                    return presentation_safe;
                }
                continue;
            }

            let may_reveal = self.ivars().content_update_epoch.get() == epoch
                && self.ivars().desired_container_visible.get()
                && !self.ivars().stage_retry_terminal.get()
                && !self.ivars().geometry_pending.get()
                && !self.ivars().stage_retry_scheduled.get();
            if !may_reveal {
                continue;
            }
            let hidden = self.isHidden();
            if self.ivars().stage_retry_terminal.get()
                || self.ivars().content_update_epoch.get() != epoch
                || !self.ivars().desired_container_visible.get()
                || self.ivars().geometry_pending.get()
                || self.ivars().stage_retry_scheduled.get()
            {
                if !hidden {
                    self.setHidden(true);
                }
                continue;
            }
            if hidden {
                self.setHidden(false);
            }
            if self.ivars().content_update_epoch.get() != epoch
                || !self.ivars().desired_container_visible.get()
                || self.ivars().stage_retry_terminal.get()
                || self.ivars().geometry_pending.get()
                || self.ivars().stage_retry_scheduled.get()
            {
                // A nested hide or newer visible layout owns the container.
                // Conceal before retrying so this stale outer setter cannot
                // expose old leaves even for one compositor turn.
                if !self.isHidden() {
                    self.setHidden(true);
                }
                continue;
            }
            return true;
        }

        // Re-entry did not converge within the bounded synchronous budget.
        // The privileged chrome surface is preferable to stale page pixels;
        // a subsequent identical layout/ready update retries presentation.
        if !self.isHidden() {
            self.setHidden(true);
        }
        self.defer_stage_retry(false);
        false
    }

    fn bump_layout_epoch(&self) {
        let epoch = self.ivars().layout_epoch.get().wrapping_add(1);
        self.ivars().layout_epoch.set(epoch.max(1));
    }

    fn layout_epoch_is_current(&self, epoch: u64) -> bool {
        !self.ivars().stage_retry_terminal.get() && self.ivars().layout_epoch.get() == epoch
    }
}

// Preserve the exact point grabbed within the gutter; ratio_for expects the
// leading edge, so feeding it the pointer itself shifts the divider on pickup.
fn anchored_drag_ratio(divider: &Divider, gap: f64, px: f64, py: f64, anchor: (f64, f64)) -> f64 {
    split::ratio_for(
        divider.axis,
        divider.rect,
        gap,
        px - anchor.0,
        py - anchor.1,
    )
}

fn feedback_rect(divider: &Divider, dragging: bool) -> Rect {
    let r = divider.rect;
    let strip = divider.strip;
    match divider.axis {
        split::Axis::Row => {
            let height = if dragging {
                r.height
            } else {
                28.0_f64.min(r.height)
            };
            Rect::new(
                strip.x + strip.width / 2.0 - 1.0,
                r.y + (r.height - height) / 2.0,
                2.0,
                height,
            )
        }
        split::Axis::Col => {
            let width = if dragging {
                r.width
            } else {
                28.0_f64.min(r.width)
            };
            Rect::new(
                r.x + (r.width - width) / 2.0,
                strip.y + strip.height / 2.0 - 1.0,
                width,
                2.0,
            )
        }
    }
}

fn same_rect(left: NSRect, right: NSRect) -> bool {
    left.origin.x == right.origin.x
        && left.origin.y == right.origin.y
        && left.size.width == right.size.width
        && left.size.height == right.size.height
}

/// The chrome's --motion-page and --ease-emphasized, so the page and the
/// sidebar beside it travel as one surface.
const SLIDE_SECONDS: f64 = 0.4;
const ARRIVE_SECONDS: f64 = 0.4;
const ARRIVE_SCALE: f64 = 0.985;

fn emphasized() -> Retained<CAMediaTimingFunction> {
    CAMediaTimingFunction::functionWithControlPoints(0.16, 1.0, 0.3, 1.0)
}

fn ease_out() -> Retained<CAMediaTimingFunction> {
    CAMediaTimingFunction::functionWithControlPoints(0.22, 1.0, 0.36, 1.0)
}

/// A horizontal move added to the layer's resting position, held at its end
/// until the stage settles.
fn translation(from: f64, to: f64, seconds: f64) -> Retained<CABasicAnimation> {
    let animation =
        CABasicAnimation::animationWithKeyPath(Some(ns_string!("transform.translation.x")));
    // SAFETY: NSNumber is the value type a scalar key path takes.
    unsafe {
        animation.setFromValue(Some(&NSNumber::new_f64(from)));
        animation.setToValue(Some(&NSNumber::new_f64(to)));
        animation.setFillMode(kCAFillModeForwards);
    }
    animation.setAdditive(true);
    animation.setRemovedOnCompletion(false);
    animation.setDuration(seconds);
    animation.setTimingFunction(Some(&emphasized()));
    animation
}

/// The frame's content ground (`--color-page` in frame/src/styles/tokens.css),
/// so a cover reads as the empty content pane, not a step to another grey.
fn page_ground(view: &NSView) -> Retained<NSColor> {
    // SAFETY: immutable framework constants.
    let (aqua, dark) = unsafe { (NSAppearanceNameAqua, NSAppearanceNameDarkAqua) };
    let is_dark = view
        .effectiveAppearance()
        .bestMatchFromAppearancesWithNames(&NSArray::from_slice(&[aqua, dark]))
        .is_some_and(|name| name.isEqualToString(dark));
    let (red, green, blue) = if is_dark {
        crate::platform::PAGE_GROUND_DARK
    } else {
        crate::platform::PAGE_GROUND_LIGHT
    };
    NSColor::colorWithSRGBRed_green_blue_alpha(
        f64::from(red) / 255.0,
        f64::from(green) / 255.0,
        f64::from(blue) / 255.0,
        1.0,
    )
}

fn fade_out(view: &Retained<NSView>) {
    let Some(layer) = view.layer() else {
        view.removeFromSuperview();
        return;
    };
    if NSWorkspace::sharedWorkspace().accessibilityDisplayShouldReduceMotion() {
        view.removeFromSuperview();
        return;
    }
    let done = view.clone();
    let completed = block2::RcBlock::new(move || {
        done.removeFromSuperview();
    });
    let fade = CABasicAnimation::animationWithKeyPath(Some(ns_string!("opacity")));
    // SAFETY: opacity takes NSNumber values; Core Animation copies the block.
    unsafe {
        fade.setFromValue(Some(&NSNumber::new_f64(layer.opacity() as f64)));
        fade.setToValue(Some(&NSNumber::new_f64(0.0)));
    }
    fade.setDuration(0.15);
    CATransaction::begin();
    CATransaction::setDisableActions(true);
    unsafe {
        CATransaction::setCompletionBlock(Some(&completed));
    }
    layer.setOpacity(0.0);
    layer.addAnimation_forKey(&fade, Some(ns_string!("zephium.restore-cover.fade")));
    CATransaction::commit();
}

#[cfg(test)]
mod tests {
    use super::*;

    // Class registration validates ivar layout with the ObjC runtime; this
    // catches alignment regressions without launching the app.
    #[test]
    fn stage_class_registers() {
        let _ = <ContentStage as objc2::ClassType>::class();
    }

    #[test]
    fn divider_feedback_is_centered_and_stays_inside_its_nested_branch() {
        let first = ItemId::from(1);
        let mut original = Pane::leaf(first);
        original.split(first, ItemId::from(2), split::Axis::Row, false);
        original.split(first, ItemId::from(3), split::Axis::Col, false);
        let region = Rect::new(0.0, 0.0, 1000.0, 800.0);
        let real_layout = split::layout(&original, region, 8.0);

        let root = split::divider_at_path(&original, region, 8.0, &[]).unwrap();
        assert_eq!(
            feedback_rect(&root, false),
            Rect::new(499.0, 386.0, 2.0, 28.0)
        );
        assert_eq!(
            feedback_rect(&root, true),
            Rect::new(499.0, 0.0, 2.0, 800.0)
        );

        for anchor in [0.0, 4.0, 7.5] {
            assert!(
                (anchored_drag_ratio(&root, 8.0, root.strip.x + anchor, 200.0, (anchor, 200.0))
                    - 0.5)
                    .abs()
                    < 1e-12
            );
            assert!(
                (anchored_drag_ratio(
                    &root,
                    8.0,
                    root.strip.x + anchor + 100.0,
                    200.0,
                    (anchor, 200.0)
                ) - (0.5 + 100.0 / 992.0))
                    .abs()
                    < 1e-12
            );
        }
        let mut guide_only = original.clone();
        guide_only.set_ratio(&[0], 0.75);
        let nested = split::divider_at_path(&guide_only, region, 8.0, &[0]).unwrap();
        assert_eq!(
            feedback_rect(&nested, false),
            Rect::new(234.0, 597.0, 28.0, 2.0)
        );
        assert_eq!(
            feedback_rect(&nested, true),
            Rect::new(0.0, 597.0, 496.0, 2.0)
        );
        assert!(
            (anchored_drag_ratio(&nested, 8.0, 100.0, nested.strip.y + 4.0, (100.0, 4.0)) - 0.75)
                .abs()
                < 1e-12
        );
        assert_eq!(split::layout(&original, region, 8.0), real_layout);
    }
}
