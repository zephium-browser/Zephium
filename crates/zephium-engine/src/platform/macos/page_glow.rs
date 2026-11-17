//! The working glow: a soft blue light along the inner edges of a Browse tab's
//! page while the assistant acts on it. Core Animation only. The layers hang
//! from the web view's own backing layer above its content, outside the
//! page's process and DOM, so they follow the view's clip, visibility and
//! motion without code and never take input.
//!
//! Only the edge band is drawn: four strips whose small colour-and-falloff
//! bitmaps slide (`contentsRect`) and four corner pieces whose colour steps
//! around the wheel, all on one shared clock in the render server. The
//! window server redraws the band alone, and the app does nothing per
//! frame; it lays the pieces out again only when the view's size changes.
//!
//! Voice shares the same pieces: the colours hold still and one layer
//! between the root and the colours breathes with the level, one short
//! animation per level. With no level, nothing is scheduled; off, every
//! animation goes with the root.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use objc2::rc::{Retained, Weak};
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{define_class, msg_send, AnyThread};
use objc2_app_kit::{NSColorSpace, NSWorkspace};
use objc2_core_foundation::{CFRetained, CGFloat, CGPoint, CGRect, CGSize};
use objc2_core_graphics::{
    kCGColorSpaceSRGB, CGBitmapContextCreate, CGBitmapContextCreateImage, CGColor, CGColorSpace,
    CGImage, CGImageAlphaInfo, CGImageByteOrderInfo,
};
use objc2_foundation::{
    ns_string, NSArray, NSDictionary, NSNull, NSNumber, NSObjectProtocol, NSString, NSValue,
};
use objc2_quartz_core::{
    kCAMediaTimingFunctionLinear, CAAction, CAAnimation, CAAutoresizingMask, CABasicAnimation,
    CACurrentMediaTime, CAFrameRateRange, CAKeyframeAnimation, CALayer, CAMediaTiming,
    CAMediaTimingFunction, CATransaction, CATransform3D,
};
use objc2_web_kit::WKWebView;
use zephium_core::ids::ItemId;

use crate::page_reader::{PageGlow, PageVoice};
use crate::voice_glow::{tune, Envelope, Look};

/// The luminous line hugging the edge, in points.
const LINE: f64 = 2.5;
/// How far the light falls into the page, in points.
const FALLOFF: f64 = 34.0;
/// One trip of the colours around the edge.
const LOOP_SECONDS: f64 = 7.0;
const FADE_IN_SECONDS: f64 = 0.3;
const FADE_OUT_SECONDS: f64 = 0.45;
/// The light stays this long after the last step before it fades.
const HOLD_SECONDS: f64 = 0.6;
/// However short the steps, it is seen at least this long.
const SHOWN_AT_LEAST_SECONDS: f64 = 1.4;
const PEAK_LIGHT: f32 = 0.55;
const PEAK_DARK: f32 = 0.68;
/// The person took over: the light holds still at this share of its peak.
const PAUSED_SHARE: f32 = 0.45;
/// Columns of a strip bitmap: two trips of the wheel.
const STRIP_PX: usize = 512;
/// Colour steps a corner takes per trip.
const CORNER_STEPS: usize = 12;
const TRAVEL: &str = "zephium.glow.travel";
const FADE: &str = "zephium.glow.fade";
const BREATH: &str = "zephium.glow.breath";

/// Zephium's blues, sRGB: azure, sky, periwinkle and a cooler sky. The
/// light travels through one family of colour, so it reads as the window's
/// own light at work rather than a spectrum running round the page.
const AURORA_LIGHT: [[f64; 3]; 4] = [
    [58.0, 118.0, 238.0],
    [86.0, 156.0, 250.0],
    [112.0, 116.0, 238.0],
    [64.0, 170.0, 244.0],
];
const AURORA_DARK: [[f64; 3]; 4] = [
    [104.0, 156.0, 255.0],
    [138.0, 190.0, 255.0],
    [150.0, 146.0, 255.0],
    [110.0, 198.0, 255.0],
];

define_class!(
    /// The glow's root: it lays its pieces out when its size changes.
    #[unsafe(super(CALayer))]
    #[name = "ZephiumPageGlowLayer"]
    struct GlowLayer;

    impl GlowLayer {
        #[unsafe(method(setBounds:))]
        fn set_bounds(&self, bounds: CGRect) {
            // SAFETY: forwards the exact selector and argument to CALayer.
            let _: () = unsafe { msg_send![super(self), setBounds: bounds] };
            resized(self);
        }
    }
);

impl GlowLayer {
    fn create() -> Retained<Self> {
        // SAFETY: plain NSObject initialization of a CALayer subclass that
        // declares no ivars.
        unsafe { msg_send![Self::alloc(), init] }
    }
}

#[derive(Clone, Copy, PartialEq)]
struct Layout {
    width: f64,
    height: f64,
    radius: f64,
    scale: f64,
    dark: bool,
}

struct Glow {
    tab: ItemId,
    view: Weak<WKWebView>,
    root: Retained<GlowLayer>,
    /// The voice's breath: an opacity over the colours, on its own clock.
    breath: Retained<CALayer>,
    /// The pieces' clock: holding it holds the colours.
    colours: Retained<CALayer>,
    strips: [Retained<CALayer>; 4],
    corners: [Retained<CALayer>; 4],
    masks: [Retained<CALayer>; 4],
    epoch: f64,
    laid: Cell<Option<Layout>>,
    radius: Cell<f64>,
    scale: Cell<f64>,
    dark: Cell<bool>,
    assist: Cell<PageGlow>,
    voice: Cell<PageVoice>,
    shown: Cell<Look>,
    still: Cell<bool>,
    envelope: Cell<Envelope>,
    /// When the light last came on from off, in media time.
    since: Cell<f64>,
    generation: Rc<Cell<u64>>,
}

thread_local! {
    static GLOWS: RefCell<HashMap<ItemId, Glow>> = RefCell::new(HashMap::new());
    static IMAGES: RefCell<Vec<(Key, CFRetained<CGImage>)>> = const { RefCell::new(Vec::new()) };
}

#[derive(Clone, Copy, PartialEq)]
enum Key {
    Strip { dark: bool, scale_q: u32 },
    Corner { radius_q: u32, scale_q: u32 },
}

/// Shows, holds or removes the glow on `tab`'s page view.
pub(crate) fn set(tab: ItemId, view: &wry::WebView, glow: PageGlow, reduce_motion: bool) {
    show_on(tab, &super::native_webview(view), glow, reduce_motion);
}

pub(crate) fn show_on(tab: ItemId, view: &WKWebView, glow: PageGlow, reduce_motion: bool) {
    update(tab, view, Some(glow), None, reduce_motion);
}

/// Shows or removes the voice edge on `tab`'s page view.
pub(crate) fn set_voice(tab: ItemId, view: &wry::WebView, voice: PageVoice, reduce_motion: bool) {
    update(
        tab,
        &super::native_webview(view),
        None,
        Some(voice),
        reduce_motion,
    );
}

/// One voice level for `tab`'s edge; nothing unless it breathes.
pub(crate) fn level(tab: ItemId, level: u8) {
    GLOWS.with(|glows| {
        if let Some(glow) = glows.try_borrow().ok().as_ref().and_then(|g| g.get(&tab)) {
            glow.breathe(level);
        }
    });
}

fn update(
    tab: ItemId,
    view: &WKWebView,
    assist: Option<PageGlow>,
    voice: Option<PageVoice>,
    reduce_motion: bool,
) {
    let (assist, voice) = GLOWS.with(|glows| {
        let glows = glows.borrow();
        let current = glows.get(&tab);
        let assist = assist
            .or(current.map(|glow| glow.assist.get()))
            .unwrap_or_default();
        let voice = voice
            .or(current.map(|glow| glow.voice.get()))
            .unwrap_or_default();
        if let Some(current) = current {
            current.assist.set(assist);
            current.voice.set(voice);
        }
        (assist, voice)
    });
    let look = Look::of(assist, voice);
    if look == Look::Off {
        GLOWS.with(|glows| {
            if let Some(current) = glows.borrow().get(&tab) {
                current.off();
            }
        });
        return;
    }
    let still =
        reduce_motion || NSWorkspace::sharedWorkspace().accessibilityDisplayShouldReduceMotion();
    GLOWS.with(|glows| {
        let mut glows = glows.borrow_mut();
        let same = glows.get(&tab).is_some_and(|current| {
            current
                .view
                .load()
                .is_some_and(|known| std::ptr::eq(&*known, view))
        });
        if !same {
            glows.remove(&tab);
            if glows.len() >= 64 {
                glows.retain(|_, glow| glow.view.load().is_some());
            }
            let created = Glow::new(tab, view);
            created.assist.set(assist);
            created.voice.set(voice);
            glows.insert(tab, created);
        }
    });
    // Attaching resizes the root, which lays out through `resized`: the map
    // is not borrowed meanwhile.
    GLOWS.with(|glows| {
        if let Some(current) = glows.borrow().get(&tab) {
            current.show(view, look, still);
        }
    });
}

/// Removes `tab`'s glow and its animations now.
pub(crate) fn forget(tab: ItemId) {
    let removed = GLOWS.with(|glows| glows.borrow_mut().remove(&tab));
    drop(removed);
}

/// Removes every glow now.
pub(crate) fn clear() {
    let removed = GLOWS.with(|glows| std::mem::take(&mut *glows.borrow_mut()));
    drop(removed);
}

/// The view's corner changed (full screen): the line follows it.
pub(crate) fn reshape(view: &WKWebView) {
    GLOWS.with(|glows| {
        let Ok(glows) = glows.try_borrow() else {
            return;
        };
        for glow in glows.values() {
            if glow
                .view
                .load()
                .is_some_and(|known| std::ptr::eq(&*known, view))
            {
                glow.measure(view);
                glow.layout();
            }
        }
    });
}

fn resized(root: &GlowLayer) {
    GLOWS.with(|glows| {
        let Ok(glows) = glows.try_borrow() else {
            return;
        };
        if let Some(glow) = glows.values().find(|glow| std::ptr::eq(&*glow.root, root)) {
            glow.layout();
        }
    });
}

impl Glow {
    fn new(tab: ItemId, view: &WKWebView) -> Self {
        let root = GlowLayer::create();
        let breath = CALayer::new();
        let colours = CALayer::new();
        let strips = [(); 4].map(|()| CALayer::new());
        let corners = [(); 4].map(|()| CALayer::new());
        let masks = [(); 4].map(|()| CALayer::new());
        CATransaction::begin();
        CATransaction::setDisableActions(true);
        let actions = quiet();
        root.setActions(Some(&actions));
        root.setAutoresizingMask(
            CAAutoresizingMask::LayerWidthSizable | CAAutoresizingMask::LayerHeightSizable,
        );
        root.setZPosition(10_000.0);
        root.setOpacity(0.0);
        breath.setActions(Some(&actions));
        breath.setAutoresizingMask(
            CAAutoresizingMask::LayerWidthSizable | CAAutoresizingMask::LayerHeightSizable,
        );
        root.addSublayer(&breath);
        colours.setActions(Some(&actions));
        colours.setAutoresizingMask(
            CAAutoresizingMask::LayerWidthSizable | CAAutoresizingMask::LayerHeightSizable,
        );
        breath.addSublayer(&colours);
        for strip in &strips {
            strip.setActions(Some(&actions));
            colours.addSublayer(strip);
        }
        for (corner, mask) in corners.iter().zip(&masks) {
            corner.setActions(Some(&actions));
            mask.setActions(Some(&actions));
            // SAFETY: each mask is a plain layer owned by exactly one corner.
            unsafe { corner.setMask(Some(mask)) };
            colours.addSublayer(corner);
        }
        CATransaction::commit();
        let epoch = colours.convertTime_fromLayer(CACurrentMediaTime(), None);
        let glow = Self {
            tab,
            view: Weak::new(view),
            root,
            breath,
            colours,
            strips,
            corners,
            masks,
            epoch,
            laid: Cell::new(None),
            radius: Cell::new(0.0),
            scale: Cell::new(2.0),
            dark: Cell::new(false),
            assist: Cell::new(PageGlow::Off),
            voice: Cell::new(PageVoice::Off),
            shown: Cell::new(Look::Off),
            still: Cell::new(false),
            envelope: Cell::new(Envelope::default()),
            since: Cell::new(0.0),
            generation: Rc::new(Cell::new(0)),
        };
        glow.measure(view);
        glow
    }

    /// The view's corner, backing scale and page tone.
    fn measure(&self, view: &WKWebView) {
        self.radius
            .set(view.layer().map_or(0.0, |layer| layer.cornerRadius()));
        self.scale.set(
            view.window()
                .map_or(2.0, |window| window.backingScaleFactor())
                .clamp(1.0, 3.0),
        );
    }

    fn show(&self, view: &WKWebView, look: Look, reduce_motion: bool) {
        let Some(host) = view.layer() else {
            crate::page_reader::note(format_args!(
                "page_glow refused tab={} reason=no-layer",
                self.tab
            ));
            return;
        };
        self.generation.set(self.generation.get().wrapping_add(1));
        self.measure(view);
        self.dark.set(page_is_dark(view));
        CATransaction::begin();
        CATransaction::setDisableActions(true);
        let attached = self
            .root
            .superlayer()
            .is_some_and(|parent| std::ptr::eq(&*parent, &*host));
        if !attached {
            self.root.removeFromSuperlayer();
            host.addSublayer(&self.root);
        }
        self.root.setFrame(host.bounds());
        if self.shown.get() == Look::Off {
            self.since.set(CACurrentMediaTime());
        }
        if !attached {
            let size = host.bounds().size;
            // SAFETY: main-thread reads of the live view's own hierarchy.
            let parent = unsafe { view.superview() }.map(|parent| parent.class().name().to_owned());
            crate::page_reader::note(format_args!(
                "page_glow attached tab={} size={}x{} radius={} scale={} window={} hidden={} visible={} host={}",
                self.tab,
                size.width.round(),
                size.height.round(),
                self.radius.get(),
                self.scale.get(),
                view.window().is_some(),
                view.isHiddenOrHasHiddenAncestor(),
                view.window().is_some_and(|window| window.isVisible()),
                parent.as_ref().and_then(|name| name.to_str().ok()).unwrap_or("none"),
            ));
        }
        self.layout();
        if look.travels(reduce_motion) {
            self.resume();
        } else {
            self.hold();
        }
        CATransaction::commit();
        let peak = if self.dark.get() {
            PEAK_DARK
        } else {
            PEAK_LIGHT
        };
        self.fade_in(peak * look.share(PAUSED_SHARE));
        let before = self.shown.replace(look);
        let was_still = self.still.replace(reduce_motion);
        if !look.voiced() {
            self.envelope.set(Envelope::default());
        }
        if before != look || was_still != reduce_motion {
            let to = self.envelope.get().breath(look, reduce_motion);
            self.glide(to, FADE_IN_SECONDS);
        }
    }

    /// One level: the breath glides to it, then settles to rest unless the
    /// next level comes first.
    fn breathe(&self, level: u8) {
        let (look, still) = (self.shown.get(), self.still.get());
        if !look.breathes(still) || self.root.superlayer().is_none() {
            return;
        }
        let mut envelope = self.envelope.get();
        envelope.feed(level, CACurrentMediaTime());
        self.envelope.set(envelope);
        let to = envelope.breath(look, still);
        let rest = look.rest(still);
        breathe_layer(&self.breath, to, rest);
    }

    /// The breath eases to `to` and stays.
    fn glide(&self, to: f32, seconds: f64) {
        // SAFETY: the presentation copy is read on the main thread.
        let from = unsafe { self.breath.presentationLayer() }
            .map_or_else(|| self.breath.opacity(), |shown| shown.opacity());
        if (from - to).abs() < f32::EPSILON {
            CATransaction::begin();
            CATransaction::setDisableActions(true);
            self.breath.removeAnimationForKey(ns_string!(BREATH));
            self.breath.setOpacity(to);
            CATransaction::commit();
            return;
        }
        let glide = CABasicAnimation::animationWithKeyPath(Some(ns_string!("opacity")));
        // SAFETY: opacity takes NSNumber values.
        unsafe {
            glide.setFromValue(Some(&NSNumber::new_f32(from)));
            glide.setToValue(Some(&NSNumber::new_f32(to)));
        }
        glide.setDuration(seconds);
        glide.setTimingFunction(Some(&CAMediaTimingFunction::functionWithControlPoints(
            0.25, 0.1, 0.25, 1.0,
        )));
        CATransaction::begin();
        CATransaction::setDisableActions(true);
        self.breath.setOpacity(to);
        self.breath
            .addAnimation_forKey(&glide, Some(ns_string!(BREATH)));
        CATransaction::commit();
    }

    /// The light finishes coming on, holds, then fades and goes: a short
    /// run of steps is still seen.
    fn off(&self) {
        let shown = self.shown.replace(Look::Off);
        if shown == Look::Off {
            return;
        }
        self.envelope.set(Envelope::default());
        let generation = self.generation.get().wrapping_add(1);
        self.generation.set(generation);
        let current = self.generation.clone();
        let root = self.root.clone();
        let breath = self.breath.clone();
        let pieces: Vec<Retained<CALayer>> =
            self.strips.iter().chain(&self.corners).cloned().collect();
        let tab = self.tab;
        let since = self.since.get();
        let done = block2::RcBlock::new(move || {
            if current.get() == generation {
                still_layers(&root, &breath, &pieces);
                crate::page_reader::note(format_args!(
                    "page_glow removed tab={tab} reason=off shown_ms={}",
                    ((CACurrentMediaTime() - since) * 1_000.0).round()
                ));
            }
        });
        // SAFETY: the presentation copy is read on the main thread.
        let from = unsafe { self.root.presentationLayer() }
            .map_or_else(|| self.root.opacity(), |shown| shown.opacity());
        let peak = self.root.opacity().max(from);
        let rise = if peak > 0.0 {
            FADE_IN_SECONDS * f64::from((peak - from) / peak)
        } else {
            0.0
        };
        let elapsed = CACurrentMediaTime() - since;
        let hold = if shown.voiced() {
            tune::VOICE_HOLD_SECONDS
        } else {
            HOLD_SECONDS.max(SHOWN_AT_LEAST_SECONDS - elapsed - FADE_OUT_SECONDS)
        };
        let total = rise + hold + FADE_OUT_SECONDS;
        let ease = CAMediaTimingFunction::functionWithControlPoints(0.25, 0.1, 0.25, 1.0);
        let still = CAMediaTimingFunction::functionWithControlPoints(0.0, 0.0, 1.0, 1.0);
        let fade = CAKeyframeAnimation::animationWithKeyPath(Some(ns_string!("opacity")));
        let values = [from, peak, peak, 0.0].map(NSNumber::new_f32);
        let objects: Vec<&AnyObject> = values.iter().map(|value| value.as_ref()).collect();
        let times = [0.0, rise / total, (rise + hold) / total, 1.0].map(NSNumber::new_f64);
        // SAFETY: opacity takes NSNumber values; key times are NSNumbers in
        // [0, 1], one per value, ascending.
        unsafe {
            fade.setValues(Some(&NSArray::from_slice(&objects)));
            fade.setKeyTimes(Some(&NSArray::from_retained_slice(&times)));
        }
        fade.setTimingFunctions(Some(&NSArray::from_retained_slice(&[
            ease.clone(),
            still,
            ease,
        ])));
        fade.setDuration(total);
        CATransaction::begin();
        CATransaction::setDisableActions(true);
        // SAFETY: Core Animation copies the block and runs it once, on the
        // main thread, when this transaction's animations end.
        unsafe { CATransaction::setCompletionBlock(Some(&*done)) };
        self.root.setOpacity(0.0);
        self.root.addAnimation_forKey(&fade, Some(ns_string!(FADE)));
        CATransaction::commit();
        self.laid.set(None);
    }

    fn fade_in(&self, to: f32) {
        // SAFETY: the presentation copy is read on the main thread.
        let from = unsafe { self.root.presentationLayer() }
            .map_or_else(|| self.root.opacity(), |shown| shown.opacity());
        let fade = CABasicAnimation::animationWithKeyPath(Some(ns_string!("opacity")));
        // SAFETY: opacity takes NSNumber values.
        unsafe {
            fade.setFromValue(Some(&NSNumber::new_f32(from)));
            fade.setToValue(Some(&NSNumber::new_f32(to)));
        }
        fade.setDuration(FADE_IN_SECONDS);
        fade.setTimingFunction(Some(&CAMediaTimingFunction::functionWithControlPoints(
            0.25, 0.1, 0.25, 1.0,
        )));
        CATransaction::begin();
        CATransaction::setDisableActions(true);
        self.root.setOpacity(to);
        self.root.addAnimation_forKey(&fade, Some(ns_string!(FADE)));
        CATransaction::commit();
    }

    /// Stops the colours where they are.
    fn hold(&self) {
        if self.colours.speed() == 0.0 {
            return;
        }
        let now = self
            .colours
            .convertTime_fromLayer(CACurrentMediaTime(), None);
        self.colours.setSpeed(0.0);
        self.colours.setTimeOffset(now);
    }

    /// Sets the colours travelling again from where they stopped.
    fn resume(&self) {
        if self.colours.speed() != 0.0 {
            return;
        }
        let paused = self.colours.timeOffset();
        self.colours.setSpeed(1.0);
        self.colours.setTimeOffset(0.0);
        self.colours.setBeginTime(0.0);
        let since = self
            .colours
            .convertTime_fromLayer(CACurrentMediaTime(), None)
            - paused;
        self.colours.setBeginTime(since);
    }

    /// Places the strips and corners around the current bounds and sets
    /// each on the shared clock at its place on the wheel.
    fn layout(&self) {
        let size = self.root.bounds().size;
        let layout = Layout {
            width: size.width,
            height: size.height,
            radius: self.radius.get(),
            scale: self.scale.get(),
            dark: self.dark.get(),
        };
        if self.root.superlayer().is_none() || self.laid.replace(Some(layout)) == Some(layout) {
            return;
        }
        let scale_q = (layout.scale * 4.0).round() as u32;
        let strip = image(Key::Strip {
            dark: layout.dark,
            scale_q,
        });
        let corner_image = image(Key::Corner {
            radius_q: (layout.radius * 4.0).round() as u32,
            scale_q,
        });
        let (Some(strip), Some(corner_image)) = (strip, corner_image) else {
            return;
        };
        let (w, h) = (layout.width, layout.height);
        let c = corner_points(layout.radius).min(w / 2.0).min(h / 2.0);
        let f = FALLOFF.min(c);
        let a = (w - 2.0 * c).max(0.0);
        let b = (h - 2.0 * c).max(0.0);
        let perimeter = (2.0 * a + 2.0 * b + 4.0 * c).max(1.0);
        let flipped = self.root.contentsAreFlipped();
        let place = |layer: &CALayer, centre: (f64, f64), extent: (f64, f64), quarter: f64| {
            layer.setBounds(CGRect::new(
                CGPoint::new(0.0, 0.0),
                CGSize::new(extent.0, extent.1),
            ));
            layer.setPosition(CGPoint::new(
                centre.0,
                if flipped { centre.1 } else { h - centre.1 },
            ));
            let turn = quarter * std::f64::consts::FRAC_PI_2 * if flipped { 1.0 } else { -1.0 };
            layer.setTransform(CATransform3D::new_rotation(turn, 0.0, 0.0, 1.0));
        };
        CATransaction::begin();
        CATransaction::setDisableActions(true);
        self.breath.setFrame(self.root.bounds());
        self.colours.setFrame(self.root.bounds());
        // Clockwise from the top-left: each strip's own x runs with the
        // edge, and `s` is where it starts along the band.
        let strips = [
            ((c + a / 2.0, f / 2.0), a, 0.0, 0.0),
            ((w - f / 2.0, c + b / 2.0), b, 1.0, a + c),
            ((c + a / 2.0, h - f / 2.0), a, 2.0, a + b + 2.0 * c),
            ((f / 2.0, c + b / 2.0), b, 3.0, 2.0 * a + b + 3.0 * c),
        ];
        for (layer, (centre, length, quarter, s)) in self.strips.iter().zip(strips) {
            let contents: &AnyObject = strip.as_ref();
            // SAFETY: a CGImage is valid layer contents; the layer retains it.
            unsafe { layer.setContents(Some(contents)) };
            layer.setContentsScale(layout.scale);
            layer.setHidden(length <= 0.0);
            place(layer, centre, (length, f), quarter);
            let start = (s / perimeter).rem_euclid(1.0);
            let span = length / perimeter;
            layer.addAnimation_forKey(&self.clocked(slide(start, span)), Some(ns_string!(TRAVEL)));
        }
        let corners = [
            ((c / 2.0, c / 2.0), 0.0, 2.0 * a + 2.0 * b + 3.5 * c),
            ((w - c / 2.0, c / 2.0), 1.0, a + 0.5 * c),
            ((w - c / 2.0, h - c / 2.0), 2.0, a + b + 1.5 * c),
            ((c / 2.0, h - c / 2.0), 3.0, 2.0 * a + b + 2.5 * c),
        ];
        let stops = if layout.dark {
            &AURORA_DARK
        } else {
            &AURORA_LIGHT
        };
        for ((layer, mask), (centre, quarter, s)) in
            self.corners.iter().zip(&self.masks).zip(corners)
        {
            place(layer, centre, (c, c), quarter);
            let contents: &AnyObject = corner_image.as_ref();
            // SAFETY: a CGImage is valid layer contents; the layer retains it.
            unsafe { mask.setContents(Some(contents)) };
            mask.setContentsScale(layout.scale);
            mask.setFrame(CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(c, c)));
            let at = (s / perimeter).rem_euclid(1.0);
            layer.setBackgroundColor(Some(&srgb(hue(stops, at))));
            layer.addAnimation_forKey(&self.clocked(step(stops, at)), Some(ns_string!(TRAVEL)));
        }
        CATransaction::commit();
    }

    /// Every piece runs on one clock that started with the glow, so a new
    /// layout keeps the colours where they were.
    fn clocked<T: AsRef<CAAnimation>>(&self, animation: Retained<T>) -> Retained<T> {
        let base: &CAAnimation = (*animation).as_ref();
        base.setBeginTime(self.epoch);
        base.setDuration(LOOP_SECONDS);
        base.setRepeatCount(f32::INFINITY);
        // SAFETY: an immutable framework constant.
        base.setTimingFunction(Some(&CAMediaTimingFunction::functionWithName(unsafe {
            kCAMediaTimingFunctionLinear
        })));
        // A slow drift needs few frames; this keeps the render server's
        // work low on fast displays.
        if base.respondsToSelector(objc2::sel!(setPreferredFrameRateRange:)) {
            base.setPreferredFrameRateRange(CAFrameRateRange {
                minimum: 10.0,
                maximum: 30.0,
                preferred: 20.0,
            });
        }
        animation
    }

    #[cfg(feature = "page-reader-probe")]
    fn travels(&self) -> bool {
        // SAFETY: a main-thread lookup by a fixed key; only its presence is
        // tested.
        self.root.superlayer().is_some()
            && unsafe { self.strips[0].animationForKey(ns_string!(TRAVEL)) }.is_some()
            && self.colours.speed() != 0.0
    }

    #[cfg(feature = "page-reader-probe")]
    fn facts(&self) -> GlowFacts {
        let bounds = self.root.bounds();
        let laid = self.laid.get();
        GlowFacts {
            attached: self.root.superlayer().is_some(),
            opacity: self.root.opacity(),
            travelling: self.travels(),
            fitted: laid.is_some_and(|laid| {
                laid.width == bounds.size.width && laid.height == bounds.size.height
            }),
            bounds,
        }
    }
}

impl Drop for Glow {
    fn drop(&mut self) {
        self.generation.set(self.generation.get().wrapping_add(1));
        if self.root.superlayer().is_some() {
            crate::page_reader::note(format_args!(
                "page_glow removed tab={} reason=closed shown_ms={}",
                self.tab,
                ((CACurrentMediaTime() - self.since.get()) * 1_000.0).round()
            ));
        }
        let pieces: Vec<Retained<CALayer>> =
            self.strips.iter().chain(&self.corners).cloned().collect();
        still_layers(&self.root, &self.breath, &pieces);
    }
}

/// Every animation of the glow goes and the root leaves the page: nothing
/// remains for the render server to run.
fn still_layers(root: &CALayer, breath: &CALayer, pieces: &[Retained<CALayer>]) {
    for piece in pieces {
        piece.removeAllAnimations();
    }
    breath.removeAllAnimations();
    breath.setOpacity(1.0);
    root.removeAllAnimations();
    root.removeFromSuperlayer();
}

/// The breath glides from where it shows to `to`, then settles to `rest`;
/// a later level replaces it from wherever it then is.
fn breathe_layer(breath: &CALayer, to: f32, rest: f32) {
    // SAFETY: the presentation copy is read on the main thread.
    let from = unsafe { breath.presentationLayer() }
        .map_or_else(|| breath.opacity(), |shown| shown.opacity());
    let total = tune::GLIDE_SECONDS + tune::TAIL_SECONDS;
    let animation = CAKeyframeAnimation::animationWithKeyPath(Some(ns_string!("opacity")));
    let values = [from, to, rest].map(NSNumber::new_f32);
    let objects: Vec<&AnyObject> = values.iter().map(|value| value.as_ref()).collect();
    let times = [0.0, tune::GLIDE_SECONDS / total, 1.0].map(NSNumber::new_f64);
    // SAFETY: opacity takes NSNumber values; key times are NSNumbers in
    // [0, 1], one per value, ascending.
    unsafe {
        animation.setValues(Some(&NSArray::from_slice(&objects)));
        animation.setKeyTimes(Some(&NSArray::from_retained_slice(&times)));
    }
    let out = CAMediaTimingFunction::functionWithControlPoints(0.2, 0.0, 0.2, 1.0);
    animation.setTimingFunctions(Some(&NSArray::from_retained_slice(&[out.clone(), out])));
    animation.setDuration(total);
    if animation.respondsToSelector(objc2::sel!(setPreferredFrameRateRange:)) {
        animation.setPreferredFrameRateRange(CAFrameRateRange {
            minimum: 15.0,
            maximum: 30.0,
            preferred: 30.0,
        });
    }
    CATransaction::begin();
    CATransaction::setDisableActions(true);
    breath.setOpacity(rest);
    breath.addAnimation_forKey(&animation, Some(ns_string!(BREATH)));
    CATransaction::commit();
}

/// What a probe can see of a tab's glow.
#[cfg(feature = "page-reader-probe")]
#[derive(Clone, Copy, Debug)]
pub(crate) struct GlowFacts {
    pub(crate) attached: bool,
    pub(crate) opacity: f32,
    pub(crate) travelling: bool,
    pub(crate) fitted: bool,
    pub(crate) bounds: CGRect,
}

#[cfg(feature = "page-reader-probe")]
pub(crate) fn facts(tab: ItemId) -> Option<GlowFacts> {
    GLOWS.with(|glows| glows.borrow().get(&tab).map(Glow::facts))
}

/// A strip's window onto two trips of the wheel, sliding back by one trip:
/// the colours move along the band.
fn slide(start: f64, span: f64) -> Retained<CABasicAnimation> {
    let window = |from: f64| {
        NSValue::new(CGRect::new(
            CGPoint::new(from / 2.0, 0.0),
            CGSize::new(span / 2.0, 1.0),
        ))
    };
    let slide = CABasicAnimation::animationWithKeyPath(Some(ns_string!("contentsRect")));
    // SAFETY: contentsRect takes CGRect values boxed in NSValue.
    unsafe {
        slide.setFromValue(Some(&window(start + 1.0)));
        slide.setToValue(Some(&window(start)));
    }
    slide
}

/// A corner's colour stepping round the wheel from its place `at`.
fn step(stops: &[[f64; 3]; 4], at: f64) -> Retained<CAKeyframeAnimation> {
    let colours: Vec<CFRetained<CGColor>> = (0..=CORNER_STEPS)
        .map(|k| srgb(hue(stops, at - k as f64 / CORNER_STEPS as f64)))
        .collect();
    let objects: Vec<&AnyObject> = colours.iter().map(|colour| colour.as_ref()).collect();
    let step = CAKeyframeAnimation::animationWithKeyPath(Some(ns_string!("backgroundColor")));
    // SAFETY: backgroundColor takes CGColor values.
    unsafe { step.setValues(Some(&NSArray::from_slice(&objects))) };
    step
}

fn srgb([r, g, b]: [f64; 3]) -> CFRetained<CGColor> {
    CGColor::new_srgb(r, g, b, 1.0)
}

fn to_linear(c: f64) -> f64 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn to_srgb(c: f64) -> f64 {
    let c = c.clamp(0.0, 1.0);
    if c <= 0.003_130_8 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

/// The aurora at `u` turns round the wheel: the four stops eased into each
/// other in linear light. sRGB, 0 to 1.
fn hue(stops: &[[f64; 3]; 4], u: f64) -> [f64; 3] {
    let turn = u.rem_euclid(1.0) * 4.0;
    let from = turn.floor() as usize % 4;
    let to = (from + 1) % 4;
    let t = turn.fract();
    let t = t * t * (3.0 - 2.0 * t);
    [0, 1, 2].map(|channel| {
        let a = to_linear(stops[from][channel] / 255.0);
        let b = to_linear(stops[to][channel] / 255.0);
        to_srgb(a + (b - a) * t)
    })
}

fn quiet() -> Retained<NSDictionary<NSString, ProtocolObject<dyn CAAction>>> {
    let null = NSNull::null();
    let none: &ProtocolObject<dyn CAAction> = ProtocolObject::from_ref(&*null);
    let keys = [
        ns_string!("bounds"),
        ns_string!("position"),
        ns_string!("frame"),
        ns_string!("opacity"),
        ns_string!("contents"),
        ns_string!("contentsRect"),
        ns_string!("contentsScale"),
        ns_string!("backgroundColor"),
        ns_string!("sublayers"),
        ns_string!("hidden"),
        ns_string!("onOrderIn"),
        ns_string!("onOrderOut"),
        ns_string!("transform"),
    ];
    NSDictionary::from_slices(&keys, &[none; 13])
}

fn page_is_dark(view: &WKWebView) -> bool {
    // SAFETY: a main-thread property read on a live view.
    let ground = unsafe { view.underPageBackgroundColor() };
    let Some(ground) = ground.colorUsingColorSpace(&NSColorSpace::sRGBColorSpace()) else {
        return false;
    };
    let linear = |c: CGFloat| to_linear(c.clamp(0.0, 1.0));
    let luminance = 0.2126 * linear(ground.redComponent())
        + 0.7152 * linear(ground.greenComponent())
        + 0.0722 * linear(ground.blueComponent());
    luminance < 0.18
}

/// A corner piece's side, in points: the corner's curve plus the falloff.
fn corner_points(radius: f64) -> f64 {
    (radius.max(0.0) + FALLOFF).ceil()
}

/// The band's alpha at `inside` points from the edge: the line, then a soft
/// falloff into the page.
fn band_alpha(inside: f64, scale: f64) -> f64 {
    if inside <= -1.0 / scale {
        return 0.0;
    }
    let edge = (inside * scale + 0.5).clamp(0.0, 1.0);
    let line = ((LINE - inside) * scale + 0.5).clamp(0.0, 1.0);
    let fall = (1.0 - inside / FALLOFF).clamp(0.0, 1.0);
    let glow = 0.48 * fall.powf(1.6);
    (edge * (line + (1.0 - line) * glow)).clamp(0.0, 1.0)
}

fn image(key: Key) -> Option<CFRetained<CGImage>> {
    IMAGES.with(|images| {
        if let Some((_, image)) = images.borrow().iter().find(|(known, _)| *known == key) {
            return Some(image.clone());
        }
        let (mut pixels, width, height) = match key {
            Key::Strip { dark, scale_q } => strip_pixels(
                if dark { &AURORA_DARK } else { &AURORA_LIGHT },
                f64::from(scale_q) / 4.0,
            ),
            Key::Corner { radius_q, scale_q } => {
                corner_pixels(f64::from(radius_q) / 4.0, f64::from(scale_q) / 4.0)
            }
        };
        let made = bitmap_image(&mut pixels, width, height)?;
        let mut images = images.borrow_mut();
        if images.len() >= 8 {
            images.remove(0);
        }
        images.push((key, made.clone()));
        Some(made)
    })
}

/// Two trips of the wheel along x, the band's falloff down y from the edge
/// at the top; premultiplied.
fn strip_pixels(stops: &[[f64; 3]; 4], scale: f64) -> (Vec<u8>, usize, usize) {
    let height = (FALLOFF * scale).round() as usize;
    let mut pixels = vec![0_u8; STRIP_PX * height * 4];
    let columns: Vec<[f64; 3]> = (0..STRIP_PX)
        .map(|x| hue(stops, (x as f64 + 0.5) / (STRIP_PX as f64 / 2.0)))
        .collect();
    for y in 0..height {
        let alpha = band_alpha((y as f64 + 0.5) / scale, scale);
        for (x, colour) in columns.iter().enumerate() {
            let at = (y * STRIP_PX + x) * 4;
            for channel in 0..3 {
                pixels[at + channel] = (colour[channel] * alpha * 255.0).round() as u8;
            }
            pixels[at + 3] = (alpha * 255.0).round() as u8;
        }
    }
    (pixels, STRIP_PX, height)
}

/// The top-left corner of the band around a rounded corner of `radius`.
fn corner_pixels(radius: f64, scale: f64) -> (Vec<u8>, usize, usize) {
    let side_points = corner_points(radius);
    let side = (side_points * scale).round() as usize;
    let mut pixels = vec![0_u8; side * side * 4];
    for y in 0..side {
        for x in 0..side {
            let px = (x as f64 + 0.5) / scale;
            let py = (y as f64 + 0.5) / scale;
            let dx = (radius - px).max(0.0);
            let dy = (radius - py).max(0.0);
            let inside = if dx > 0.0 && dy > 0.0 {
                radius - dx.hypot(dy)
            } else {
                px.min(py)
            };
            pixels[(y * side + x) * 4 + 3] = (band_alpha(inside, scale) * 255.0).round() as u8;
        }
    }
    (pixels, side, side)
}

fn bitmap_image(pixels: &mut [u8], width: usize, height: usize) -> Option<CFRetained<CGImage>> {
    // SAFETY: an immutable framework constant.
    let space = CGColorSpace::with_name(Some(unsafe { kCGColorSpaceSRGB }))?;
    // SAFETY: `pixels` holds exactly width * height premultiplied RGBA
    // pixels and outlives the context, which is dropped before returning.
    let context = unsafe {
        CGBitmapContextCreate(
            pixels.as_mut_ptr().cast(),
            width,
            height,
            8,
            width * 4,
            Some(&space),
            CGImageAlphaInfo::PremultipliedLast.0 | CGImageByteOrderInfo::Order32Big.0,
        )
    }?;
    let image = CGBitmapContextCreateImage(Some(&context));
    drop(context);
    image
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_band_is_bright_at_the_edge_and_clear_inside() {
        let scale = 2.0;
        let (corner, side, _) = corner_pixels(10.0, scale);
        let alpha = |x: usize, y: usize| corner[(y * side + x) * 4 + 3];
        assert_eq!(alpha(side - 1, 1), 255, "the line hugs the edge");
        assert_eq!(alpha(side - 1, 1), alpha(1, side - 1), "both edges alike");
        assert_eq!(alpha(0, 0), 0, "the corner is rounded");
        assert_eq!(
            alpha(side - 1, side - 1),
            0,
            "the page shows through inside"
        );
        let (strip, width, height) = strip_pixels(&AURORA_LIGHT, scale);
        assert_eq!((width, height), (STRIP_PX, (FALLOFF * scale) as usize));
        let strip_alpha = |y: usize| strip[(y * width) * 4 + 3];
        assert_eq!(strip_alpha(1), alpha(side - 1, 1), "strips meet corners");
        assert!(
            strip_alpha(20) > 40 && strip_alpha(20) < 160,
            "the light falls off"
        );
        assert!(strip_alpha(height - 1) < 4);
    }

    #[test]
    fn levels_breathe_and_off_leaves_no_animation() {
        let root = CALayer::new();
        let breath = CALayer::new();
        let pieces = [CALayer::new(), CALayer::new()];
        root.addSublayer(&breath);
        let host = CALayer::new();
        host.addSublayer(&root);
        breathe_layer(&breath, 0.9, tune::LISTEN_REST);
        // SAFETY: a lookup by a fixed key on a layer this test owns.
        assert!(unsafe { breath.animationForKey(ns_string!(BREATH)) }.is_some());
        assert_eq!(breath.opacity(), tune::LISTEN_REST, "a lost level rests");
        breathe_layer(&breath, 1.0, tune::LISTEN_REST);
        assert_eq!(breath.animationKeys().map_or(0, |keys| keys.count()), 1);
        still_layers(&root, &breath, &pieces);
        assert!(breath.animationKeys().is_none());
        assert!(root.animationKeys().is_none());
        assert!(pieces.iter().all(|piece| piece.animationKeys().is_none()));
        assert!(root.superlayer().is_none(), "nothing stays on the page");
        assert_eq!(breath.opacity(), 1.0);
    }

    #[test]
    fn the_wheel_closes_on_itself() {
        let a = hue(&AURORA_DARK, 0.0);
        let b = hue(&AURORA_DARK, 1.0);
        assert!(a.iter().zip(b).all(|(a, b)| (a - b).abs() < 1e-9));
        let left = (STRIP_PX as f64 / 2.0 - 0.5) / (STRIP_PX as f64 / 2.0);
        let c = hue(&AURORA_DARK, left);
        assert!(
            a.iter().zip(c).all(|(a, c)| (a - c).abs() < 0.02),
            "two seamless trips"
        );
    }
}
