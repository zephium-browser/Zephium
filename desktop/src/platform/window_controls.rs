//! The window controls rest as three unlit discs and light up under the
//! pointer.
//!
//! The controls stay the real AppKit buttons at their real positions, only
//! transparent, so clicking, the zoom button's tiling menu, VoiceOver and the
//! system accent all behave exactly as the platform defines them. What rests
//! in their place is drawn here, beside them: the same disc, at the same size,
//! in the tone macOS itself gives the controls of an inactive window. Moving
//! the buttons out of the window instead would fight Tao, which re-insets them
//! on every resize, and a button nobody can reach can never be hovered.
//!
//! Visibility is faded on the buttons' layers, never through `alphaValue`:
//! setting a titlebar button's alpha makes AppKit lay the titlebar out again,
//! which discards Tao's inset and drops the controls back to AppKit's own
//! position, a few points up and to the right.

use std::cell::{Cell, RefCell};
use std::ptr::{null_mut, NonNull};

use block2::RcBlock;
use objc2::rc::{Retained, Weak};
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{
    define_class, msg_send, AllocAnyThread, ClassType, DefinedClass, MainThreadOnly, Message,
};
use objc2_app_kit::{
    NSAppearanceCustomization, NSAppearanceNameAqua, NSAppearanceNameDarkAqua,
    NSAutoresizingMaskOptions, NSBezierPath, NSButton, NSColor, NSEvent, NSResponder,
    NSTrackingArea, NSTrackingAreaOptions, NSView, NSViewLayerContentsRedrawPolicy, NSWindow,
    NSWindowButton, NSWindowDidBecomeKeyNotification, NSWindowDidDeminiaturizeNotification,
    NSWindowDidExitFullScreenNotification, NSWindowDidResignKeyNotification, NSWindowOrderingMode,
    NSWindowStyleMask, NSWindowWillEnterFullScreenNotification,
    NSWindowWillExitFullScreenNotification, NSWorkspace,
};
use objc2_foundation::{
    ns_string, MainThreadMarker, NSArray, NSInsetRect, NSNotification, NSNotificationCenter,
    NSNumber, NSObject, NSObjectProtocol, NSPoint, NSPointInRect, NSProcessInfo, NSRect,
    NSUnionRect,
};
use objc2_quartz_core::{CABasicAnimation, CAMediaTiming, CAMediaTimingFunction, CATransaction};
use tauri::WebviewWindow;

/// The pointer lights the controls a little before it reaches them, so the
/// colour is already there when it arrives rather than appearing under it.
const APPROACH: f64 = 6.0;
/// Lighting answers the pointer, so it is quick; letting go is slower, so a
/// pointer grazing the corner does not make the controls flicker.
const LIGHT_SECONDS: f64 = 0.14;
const REST_SECONDS: f64 = 0.24;

/// Measured from the controls of an inactive window on macOS 26: white at a
/// sixth over a dark ground. On a light ground the same step is taken in ink.
const REST_DARK: (f64, f64) = (1.0, 0.17);
const REST_LIGHT: (f64, f64) = (0.0, 0.13);

/// Before macOS 26 a control's frame is 14 by 16 points with a 12 point disc
/// centred in it; filling the frame would rest as a larger, taller oval.
const LEGACY_DISC: f64 = 12.0;

pub struct Ivars {
    tracking: RefCell<Option<Retained<NSTrackingArea>>>,
    lit: Cell<bool>,
    /// Whether the resting state has been applied. Layers only exist once
    /// the window is on screen, and until then there is nothing to fade.
    settled: Cell<bool>,
    /// Full screen hands the controls to the menu-bar reveal, where they must
    /// be visible the moment it slides down.
    suspended: Cell<bool>,
    observers: RefCell<Vec<Retained<ProtocolObject<dyn NSObjectProtocol>>>>,
    /// The browser window's chrome, told the moment fullscreen begins or
    /// ends so the room it keeps for the controls follows them at once.
    chrome: RefCell<Option<WebviewWindow>>,
}

impl Drop for Ivars {
    fn drop(&mut self) {
        let center = NSNotificationCenter::defaultCenter();
        for token in self.observers.get_mut().drain(..) {
            // SAFETY: each token was returned by this centre's block API.
            unsafe { center.removeObserver((*token).as_ref()) };
        }
    }
}

define_class!(
    #[unsafe(super(NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "ZephiumWindowControls"]
    #[ivars = Ivars]
    pub struct WindowControls;

    impl WindowControls {
        /// Never a target: every click falls through to the real button or to
        /// the chrome underneath, which owns the drag region.
        #[unsafe(method(hitTest:))]
        fn hit_test(&self, _point: NSPoint) -> *mut NSView {
            null_mut()
        }

        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, _dirty: NSRect) {
            if self.ivars().suspended.get() {
                return;
            }
            let (white, alpha) = if self.is_dark() { REST_DARK } else { REST_LIGHT };
            NSColor::colorWithWhite_alpha(white, alpha).setFill();
            for rect in self.control_rects() {
                NSBezierPath::bezierPathWithOvalInRect(disc(rect)).fill();
            }
        }

        #[unsafe(method(updateTrackingAreas))]
        fn update_tracking_areas(&self) {
            if let Some(old) = self.ivars().tracking.borrow_mut().take() {
                self.removeTrackingArea(&old);
            }
            if let Some(hot) = self.hot_rect() {
                // SAFETY: the owner is this view, which outlives its own
                // tracking areas; no user info is attached.
                let area = unsafe {
                    NSTrackingArea::initWithRect_options_owner_userInfo(
                        NSTrackingArea::alloc(),
                        hot,
                        NSTrackingAreaOptions::MouseEnteredAndExited
                            | NSTrackingAreaOptions::ActiveAlways,
                        Some(self.as_ref()),
                        None,
                    )
                };
                self.addTrackingArea(&area);
                *self.ivars().tracking.borrow_mut() = Some(area);
            }
            // Tao re-insets the buttons as the window changes shape, and this
            // is the one callback guaranteed to follow every such change.
            self.setNeedsDisplay(true);
            self.settle();
            // SAFETY: NSView requires overrides of this method to call super.
            unsafe { msg_send![super(self), updateTrackingAreas] }
        }

        #[unsafe(method(mouseEntered:))]
        fn mouse_entered(&self, _event: &NSEvent) {
            self.light(true, true);
        }

        #[unsafe(method(mouseExited:))]
        fn mouse_exited(&self, _event: &NSEvent) {
            self.light(false, true);
        }

        #[unsafe(method(viewDidChangeEffectiveAppearance))]
        fn appearance_changed(&self) {
            self.setNeedsDisplay(true);
        }
    }
);

impl WindowControls {
    fn new(mtm: MainThreadMarker, frame: NSRect) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(Ivars {
            tracking: RefCell::new(None),
            lit: Cell::new(false),
            settled: Cell::new(false),
            suspended: Cell::new(false),
            observers: RefCell::new(Vec::new()),
            chrome: RefCell::new(None),
        });
        // SAFETY: the designated initializer of NSView.
        unsafe { msg_send![super(this), initWithFrame: frame] }
    }

    fn buttons(&self) -> Vec<Retained<NSButton>> {
        let Some(window) = self.window() else {
            return Vec::new();
        };
        [
            NSWindowButton::CloseButton,
            NSWindowButton::MiniaturizeButton,
            NSWindowButton::ZoomButton,
        ]
        .into_iter()
        .filter_map(|kind| window.standardWindowButton(kind))
        .collect()
    }

    fn control_rects(&self) -> Vec<NSRect> {
        self.buttons()
            .iter()
            .map(|button| {
                // SAFETY: a main-thread read of a live button's parent.
                let parent = unsafe { button.superview() };
                self.convertRect_fromView(button.frame(), parent.as_deref())
            })
            .collect()
    }

    fn hot_rect(&self) -> Option<NSRect> {
        let rects = self.control_rects();
        let first = *rects.first()?;
        let union = rects
            .iter()
            .fold(first, |all, rect| NSUnionRect(all, *rect));
        Some(NSInsetRect(union, -APPROACH, -APPROACH))
    }

    fn is_dark(&self) -> bool {
        // SAFETY: immutable framework constants.
        let (aqua, dark) = unsafe { (NSAppearanceNameAqua, NSAppearanceNameDarkAqua) };
        self.effectiveAppearance()
            .bestMatchFromAppearancesWithNames(&NSArray::from_slice(&[aqua, dark]))
            .is_some_and(|name| name.isEqualToString(dark))
    }

    fn light(&self, lit: bool, animate: bool) {
        self.ivars().lit.set(lit);
        if self.ivars().suspended.get() {
            return;
        }
        let reduce = NSWorkspace::sharedWorkspace().accessibilityDisplayShouldReduceMotion();
        let duration = match (animate && !reduce, lit) {
            (false, _) => 0.0,
            (true, true) => LIGHT_SECONDS,
            (true, false) => REST_SECONDS,
        };
        let (button_opacity, rest_opacity) = if lit { (1.0, 0.0) } else { (0.0, 1.0) };
        for button in self.buttons() {
            fade(&button, button_opacity, duration);
        }
        fade(self, rest_opacity, duration);
    }

    fn settle(&self) {
        if self.ivars().settled.get() || self.ivars().suspended.get() {
            return;
        }
        let buttons = self.buttons();
        if buttons.is_empty()
            || self.layer().is_none()
            || buttons.iter().any(|button| button.layer().is_none())
        {
            return;
        }
        self.ivars().settled.set(true);
        self.resync(false);
    }

    /// Puts the controls back in the state the pointer implies. Tracking
    /// areas miss the pointer leaving while the window is miniaturized or
    /// being closed, so this runs whenever the window comes back.
    fn resync(&self, animate: bool) {
        let Some(window) = self.window() else {
            return;
        };
        let point = self.convertPoint_fromView(window.mouseLocationOutsideOfEventStream(), None);
        let inside = self.hot_rect().is_some_and(|hot| NSPointInRect(point, hot));
        self.light(inside, animate);
    }

    fn suspend(&self, suspended: bool) {
        self.ivars().suspended.set(suspended);
        self.tell_chrome(suspended);
        if suspended {
            for button in self.buttons() {
                fade(&button, 1.0, 0.0);
            }
            fade(self, 0.0, 0.0);
        } else {
            // Leaving fullscreen, AppKit lays the titlebar out afresh at its
            // own position. The inset is applied as the content view draws,
            // which a window that comes back at the same size never asks for.
            self.reinset();
            self.setNeedsDisplay(true);
            self.resync(false);
        }
    }

    /// Leaving fullscreen, AppKit animates the titlebar back with the
    /// controls at its own position, a few points off the inset, and lays
    /// them out again until the animation ends. They are hidden for the
    /// length of it and come back, already in place, when it is done.
    fn leaving(&self) {
        self.tell_chrome(false);
        for button in self.buttons() {
            fade(&button, 0.0, 0.0);
        }
    }

    /// Puts the controls back at their inset now. The inset is applied as the
    /// content view draws; asking for that draw at once, rather than on the
    /// next pass, keeps AppKit's own position from showing for a frame.
    fn reinset(&self) {
        if let Some(content) = self.window().and_then(|window| window.contentView()) {
            content.setNeedsDisplay(true);
            content.displayIfNeeded();
        }
    }

    /// Tells the chrome whether the controls are gone for fullscreen. Sent
    /// as the transition begins either way, so the chrome moves with AppKit
    /// rather than after it; repeating the same news is harmless.
    fn tell_chrome(&self, fullscreen: bool) {
        if let Some(chrome) = self.ivars().chrome.borrow().as_ref() {
            let _ = chrome.eval(format!(
                "window.dispatchEvent(new CustomEvent('zephium:fullscreen',{{detail:{fullscreen}}}))"
            ));
        }
    }

    fn observe(&self, window: &NSWindow) {
        let center = NSNotificationCenter::defaultCenter();
        let weak = Weak::from_retained(&self.retain());
        let add = |name, action: fn(&WindowControls)| {
            let weak = weak.clone();
            let block = RcBlock::new(move |_: NonNull<NSNotification>| {
                if let Some(view) = weak.load() {
                    action(&view);
                }
            });
            // SAFETY: window notifications are posted on the main thread, the
            // block holds the view weakly, and the token is removed when the
            // view is deallocated.
            let token = unsafe {
                center.addObserverForName_object_queue_usingBlock(
                    Some(name),
                    Some(AsRef::<AnyObject>::as_ref(window)),
                    None,
                    &block,
                )
            };
            self.ivars().observers.borrow_mut().push(token);
        };
        // SAFETY: immutable framework constants.
        unsafe {
            add(NSWindowWillEnterFullScreenNotification, |view| {
                view.suspend(true)
            });
            add(NSWindowWillExitFullScreenNotification, |view| {
                view.leaving()
            });
            add(NSWindowDidExitFullScreenNotification, |view| {
                view.suspend(false)
            });
            add(NSWindowDidDeminiaturizeNotification, |view| {
                view.resync(false)
            });
            add(NSWindowDidBecomeKeyNotification, |view| view.resync(true));
            add(NSWindowDidResignKeyNotification, |view| view.resync(true));
        }
    }
}

/// Whether this is macOS 26 or later, whose titlebar lays the controls out
/// differently from the releases before it.
fn redesigned_titlebar() -> bool {
    NSProcessInfo::processInfo()
        .operatingSystemVersion()
        .majorVersion
        >= 26
}

/// How much lower than the configured position the controls are put. The
/// configured one is measured on the releases before macOS 26; from 26 the
/// same inset lays them out 3pt higher in the titlebar, above the head they
/// line up with.
pub fn inset_lift() -> f64 {
    if redesigned_titlebar() {
        3.0
    } else {
        0.0
    }
}

/// The disc a control draws within its frame.
fn disc(frame: NSRect) -> NSRect {
    if redesigned_titlebar() {
        return frame;
    }
    NSInsetRect(
        frame,
        (frame.size.width - LEGACY_DISC) / 2.0,
        (frame.size.height - LEGACY_DISC) / 2.0,
    )
}

/// Moves a view's layer to `opacity`, continuing from wherever an unfinished
/// fade has got to so a pointer crossing back mid-fade never jumps.
fn fade(view: &NSView, opacity: f32, duration: f64) {
    let Some(layer) = view.layer() else {
        return;
    };
    // SAFETY: a main-thread read of the layer's in-flight rendering state.
    let from =
        unsafe { layer.presentationLayer() }.map_or(layer.opacity(), |shown| shown.opacity());
    CATransaction::begin();
    CATransaction::setDisableActions(true);
    layer.setOpacity(opacity);
    layer.removeAnimationForKey(ns_string!("opacity"));
    if duration > 0.0 && (from - opacity).abs() > f32::EPSILON {
        let animation = CABasicAnimation::animationWithKeyPath(Some(ns_string!("opacity")));
        // SAFETY: NSNumber is the value type Core Animation expects for a
        // scalar key path.
        unsafe {
            animation.setFromValue(Some(&NSNumber::new_f32(from)));
            animation.setToValue(Some(&NSNumber::new_f32(opacity)));
        }
        animation.setDuration(duration);
        // The interface's own --ease-out, so the controls settle the way the
        // chrome around them does.
        animation.setTimingFunction(Some(&CAMediaTimingFunction::functionWithControlPoints(
            0.22, 1.0, 0.36, 1.0,
        )));
        layer.addAnimation_forKey(&animation, Some(ns_string!("opacity")));
    }
    CATransaction::commit();
}

/// Installs the resting controls on a browser window. Idempotent, and a no-op
/// off the main thread or on a window without standard buttons.
pub fn install(window: &WebviewWindow) {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let Ok(pointer) = window.ns_window() else {
        return;
    };
    // SAFETY: `ns_window` returns Tao's live NSWindow, borrowed for this
    // main-thread call only.
    let ns_window = unsafe { &*(pointer as *const NSWindow) };
    let Some(close) = ns_window.standardWindowButton(NSWindowButton::CloseButton) else {
        return;
    };
    // SAFETY: a main-thread read of the live close button's parent.
    let Some(parent) = (unsafe { close.superview() }) else {
        return;
    };
    if parent
        .subviews()
        .iter()
        .any(|view| view.isKindOfClass(WindowControls::class()))
    {
        return;
    }
    let controls = WindowControls::new(mtm, parent.bounds());
    controls.setAutoresizingMask(
        NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewHeightSizable,
    );
    controls.setLayerContentsRedrawPolicy(NSViewLayerContentsRedrawPolicy::DuringViewResize);
    parent.addSubview_positioned_relativeTo(&controls, NSWindowOrderingMode::Below, Some(&close));
    controls.ivars().chrome.replace(Some(window.clone()));
    controls.observe(ns_window);
    if ns_window
        .styleMask()
        .contains(NSWindowStyleMask::FullScreen)
    {
        controls.suspend(true);
    } else {
        controls.settle();
    }
}

/// Puts the controls away while the sidebar is hidden, which leaves the page
/// under the corner they rest in: invisible buttons there would still take
/// its clicks. They come back, at their inset, with the sidebar.
pub fn set_tucked(window: &WebviewWindow, tucked: bool) {
    if MainThreadMarker::new().is_none() {
        return;
    }
    let Ok(pointer) = window.ns_window() else {
        return;
    };
    // SAFETY: Tao's live NSWindow, borrowed for this main-thread call only.
    let ns_window = unsafe { &*(pointer as *const NSWindow) };
    let Some(close) = ns_window.standardWindowButton(NSWindowButton::CloseButton) else {
        return;
    };
    // SAFETY: a main-thread read of the live close button's parent.
    let Some(parent) = (unsafe { close.superview() }) else {
        return;
    };
    let Some(controls) = parent
        .subviews()
        .iter()
        .find_map(|view| view.downcast_ref::<WindowControls>().map(|c| c.retain()))
    else {
        return;
    };
    if controls.isHidden() == tucked {
        return;
    }
    for button in controls.buttons() {
        button.setHidden(tucked);
    }
    controls.setHidden(tucked);
    if !tucked && !controls.ivars().suspended.get() {
        controls.reinset();
        controls.resync(false);
        // They come in with the card rather than ahead of it; they leave at
        // once, so a pointer that has moved on is not left with them.
        if let Some(layer) = controls.layer() {
            if !NSWorkspace::sharedWorkspace().accessibilityDisplayShouldReduceMotion() {
                let fade = CABasicAnimation::animationWithKeyPath(Some(ns_string!("opacity")));
                // SAFETY: opacity animates between NSNumber values.
                unsafe {
                    fade.setFromValue(Some(&NSNumber::new_f64(0.0)));
                    fade.setToValue(Some(&NSNumber::new_f64(1.0)));
                }
                fade.setDuration(TUCK_SECONDS);
                fade.setTimingFunction(Some(&CAMediaTimingFunction::functionWithControlPoints(
                    0.2, 0.0, 0.0, 1.0,
                )));
                layer.addAnimation_forKey(&fade, Some(ns_string!("zephium.controls.untuck")));
            }
        }
    }
}

const TUCK_SECONDS: f64 = 0.22;

/// How far a hidden sidebar's card stands in from the window's side, and so
/// how far its controls move in with it to keep their place in its head. Its
/// head keeps the window's own height, so they stay at theirs.
const CARD_INSET: f64 = 8.0;

/// Moves the controls into the hidden sidebar's card, or back to their own
/// inset. Tao re-insets them from the content view's draw, which is how they
/// go back; until it draws, they keep the card's place.
pub fn set_in_card(window: &WebviewWindow, in_card: bool) {
    if MainThreadMarker::new().is_none() {
        return;
    }
    let Ok(pointer) = window.ns_window() else {
        return;
    };
    // SAFETY: Tao's live NSWindow, borrowed for this main-thread call only.
    let ns_window = unsafe { &*(pointer as *const NSWindow) };
    let buttons: Vec<_> = [
        NSWindowButton::CloseButton,
        NSWindowButton::MiniaturizeButton,
        NSWindowButton::ZoomButton,
    ]
    .into_iter()
    .filter_map(|kind| ns_window.standardWindowButton(kind))
    .collect();
    let Some(close) = buttons.first() else {
        return;
    };
    // SAFETY: main-thread reads of the live close button's ancestors.
    let Some(parent) = (unsafe { close.superview() }) else {
        return;
    };
    let Some(container) = (unsafe { parent.superview() }) else {
        return;
    };
    let controls = parent
        .subviews()
        .iter()
        .find_map(|view| view.downcast_ref::<WindowControls>().map(|c| c.retain()));
    // From Tao's own inset each time, so asking twice moves them once.
    if let Some(controls) = controls.as_ref() {
        controls.reinset();
        controls.setNeedsDisplay(true);
    }
    if !in_card {
        return;
    }
    // Tao's inset puts the first button at its x and grows the titlebar to
    // the button's height plus its y; the card's place is that, moved over.
    let x = close.frame().origin.x + CARD_INSET;
    let y = container.frame().size.height - close.frame().size.height;
    let height = close.frame().size.height + y;
    let mut bar = container.frame();
    bar.size.height = height;
    bar.origin.y = ns_window.frame().size.height - height;
    container.setFrame(bar);
    let space = buttons
        .get(1)
        .map_or(20.0, |next| next.frame().origin.x - close.frame().origin.x);
    for (index, button) in buttons.iter().enumerate() {
        let mut origin = button.frame().origin;
        origin.x = x + index as f64 * space;
        button.setFrameOrigin(origin);
    }
    if let Some(controls) = controls {
        controls.setNeedsDisplay(true);
        // SAFETY: the view re-reads its buttons' rects for its hover area.
        let _: () = unsafe { msg_send![&*controls, updateTrackingAreas] };
    }
}
