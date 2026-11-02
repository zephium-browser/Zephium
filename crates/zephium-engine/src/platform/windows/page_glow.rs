//! The working glow on Windows, mirroring macOS: a composition tree on the
//! page's own container window, drawn by DWM above the WebView2 child
//! windows (a topmost desktop target), so it never takes input and never
//! reaches the page. Every piece is sized relative to the window and every
//! colour runs on one animated clock through expression animations: DWM
//! follows resizes and travels the colours with no app work per frame.
//!
//! Four edge strips carry the colour along the edge and the falloff across
//! it; four corner pieces round the line. The strips' falloffs overlap a
//! little inside each corner, where the light pools slightly.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::time::{Duration, Instant};

use windows::core::{Interface as _, HSTRING};
use windows::Foundation::TimeSpan;
use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_USE_IMMERSIVE_DARK_MODE};
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::WindowsAndMessaging::{
    GetAncestor, GetClientRect, IsWindowVisible, GA_ROOT,
};
use windows::UI::Color;
use windows::UI::Composition::{
    AnimationIterationBehavior, CompositionBrush, CompositionGradientExtendMode,
    CompositionLinearGradientBrush, CompositionMappingMode, CompositionPropertySet, Compositor,
    ContainerVisual,
};
use windows_numerics::{Vector2, Vector3};
use wry::WebViewExtWindows as _;
use zephium_core::ids::ItemId;

use crate::page_reader::PageGlow;

const LINE: f32 = 2.5;
const FALLOFF: f32 = 34.0;
/// The container window's corner (the stage's rounding).
const RADIUS: f32 = 8.0;
const LOOP: Duration = Duration::from_secs(7);
const FADE_IN: Duration = Duration::from_millis(300);
const FADE_OUT: Duration = Duration::from_millis(450);
/// The light stays this long after the last step before it fades.
const HOLD: Duration = Duration::from_millis(600);
/// However short the steps, it is seen at least this long.
const SHOWN_AT_LEAST: Duration = Duration::from_millis(1_400);
const PEAK_LIGHT: f32 = 0.55;
const PEAK_DARK: f32 = 0.68;
const PAUSED_SHARE: f32 = 0.45;
const WHEEL_STOPS: usize = 12;

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

struct Glow {
    tab: ItemId,
    window: HWND,
    layer: Rc<super::page_layer::PageLayer>,
    root: ContainerVisual,
    clock: CompositionPropertySet,
    dark: bool,
    /// Physical pixels per DIP: composition draws in the window's pixels.
    scale: f32,
    shown: Cell<PageGlow>,
    /// When the light last came on from off, and when it was last asked on.
    since: Cell<Instant>,
    asked: Cell<Instant>,
    travelling: Cell<bool>,
    attached: Cell<bool>,
    generation: Rc<Cell<u64>>,
    removal: RefCell<Option<super::ContentPolicyTimeout>>,
}

thread_local! {
    static GLOWS: RefCell<HashMap<ItemId, Glow>> = RefCell::new(HashMap::new());
}

/// Shows, holds or removes the glow on `tab`'s page view.
pub(crate) fn set(tab: ItemId, view: &wry::WebView, glow: PageGlow, reduce_motion: bool) {
    if glow == PageGlow::Off {
        GLOWS.with(|glows| {
            if let Some(current) = glows.borrow().get(&tab) {
                current.off();
            }
        });
        return;
    }
    let mut window = HWND::default();
    // SAFETY: reads the container window of this live controller.
    if unsafe { view.controller().ParentWindow(&mut window) }.is_err() || window.is_invalid() {
        crate::page_reader::note(format_args!("page_glow refused tab={tab} reason=no-window"));
        return;
    }
    GLOWS.with(|glows| {
        let mut glows = glows.borrow_mut();
        let dark = dark_theme(window);
        // SAFETY: a plain read of a live window's DPI; 0 when it has none.
        let dpi = unsafe { GetDpiForWindow(window) };
        let scale = if dpi == 0 { 1.0 } else { dpi as f32 / 96.0 };
        if glows.get(&tab).is_none_or(|current| {
            current.window != window || current.dark != dark || current.scale != scale
        }) {
            glows.remove(&tab);
            let Some(created) = Glow::new(tab, window, dark, scale) else {
                crate::page_reader::note(format_args!(
                    "page_glow refused tab={tab} reason=composition"
                ));
                return;
            };
            glows.insert(tab, created);
        }
        if let Some(current) = glows.get(&tab) {
            current.show(glow, reduce_motion);
        }
    });
}

/// Removes `tab`'s glow now.
pub(crate) fn forget(tab: ItemId) {
    let removed = GLOWS.with(|glows| glows.borrow_mut().remove(&tab));
    drop(removed);
}

/// Removes every glow now.
pub(crate) fn clear() {
    let removed = GLOWS.with(|glows| std::mem::take(&mut *glows.borrow_mut()));
    drop(removed);
}

fn dark_theme(window: HWND) -> bool {
    // SAFETY: plain reads of a live window's root and its DWM attribute.
    unsafe {
        let root = GetAncestor(window, GA_ROOT);
        let mut dark = 0_i32;
        DwmGetWindowAttribute(
            root,
            DWMWA_USE_IMMERSIVE_DARK_MODE,
            std::ptr::addr_of_mut!(dark).cast(),
            std::mem::size_of::<i32>() as u32,
        )
        .is_ok()
            && dark != 0
    }
}

fn linear(c: f64) -> f64 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn srgb(c: f64) -> f64 {
    let c = c.clamp(0.0, 1.0);
    if c <= 0.003_130_8 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

fn hue(stops: &[[f64; 3]; 4], u: f64) -> Color {
    let turn = u.rem_euclid(1.0) * 4.0;
    let from = turn.floor() as usize % 4;
    let to = (from + 1) % 4;
    let t = turn.fract();
    let t = t * t * (3.0 - 2.0 * t);
    let channel = |i: usize| {
        let a = linear(stops[from][i] / 255.0);
        let b = linear(stops[to][i] / 255.0);
        (srgb(a + (b - a) * t) * 255.0).round() as u8
    };
    Color {
        A: 255,
        R: channel(0),
        G: channel(1),
        B: channel(2),
    }
}

fn band_alpha(inside: f32) -> f32 {
    let line = (LINE - inside + 0.5).clamp(0.0, 1.0);
    let fall = (1.0 - inside / FALLOFF).clamp(0.0, 1.0);
    (line + (1.0 - line) * 0.48 * fall.powf(1.6)).clamp(0.0, 1.0)
}

/// Where a piece runs along the band: its own axis, its direction, and
/// where along the band its own origin lies, as an expression of the
/// window's size.
struct Run {
    vertical: bool,
    reversed: bool,
    start: &'static str,
}

/// The band's length around the window, as an expression.
const PERIMETER: &str = "(2 * (host.Size.X + host.Size.Y) - 4 * r)";

impl Glow {
    fn new(tab: ItemId, window: HWND, dark: bool, scale: f32) -> Option<Self> {
        let compositor = super::page_layer::compositor()?;
        let layer = super::page_layer::layer(window)?;
        let root = compositor.CreateContainerVisual().ok()?;
        root.SetRelativeSizeAdjustment(Vector2 { X: 1.0, Y: 1.0 })
            .ok()?;
        root.SetOpacity(0.0).ok()?;
        let clock = compositor.CreatePropertySet().ok()?;
        clock.InsertScalar(&HSTRING::from("t"), 0.0).ok()?;
        let stops = if dark { &AURORA_DARK } else { &AURORA_LIGHT };
        let glow = Self {
            tab,
            window,
            layer,
            root,
            clock,
            dark,
            scale,
            shown: Cell::new(PageGlow::Off),
            since: Cell::new(Instant::now()),
            asked: Cell::new(Instant::now()),
            travelling: Cell::new(false),
            attached: Cell::new(false),
            generation: Rc::new(Cell::new(0)),
            removal: RefCell::new(None),
        };
        let (r, f) = (RADIUS * scale, FALLOFF * scale);
        // Clockwise from the top-left; a corner continues the strip before
        // it, and a reversed piece's band position falls as its own
        // coordinate grows.
        let pieces: [(Vector2, Vector2, Vector3, Vector3, Run, bool); 8] = [
            (
                v2(-2.0 * r, f),
                v2(1.0, 0.0),
                v3(r, 0.0),
                v3(0.0, 0.0),
                run(false, false, "0"),
                false,
            ),
            (
                v2(r, r),
                v2(0.0, 0.0),
                v3(-r, 0.0),
                v3(1.0, 0.0),
                run(false, false, "(host.Size.X - 2 * r)"),
                true,
            ),
            (
                v2(f, -2.0 * r),
                v2(0.0, 1.0),
                v3(-f, r),
                v3(1.0, 0.0),
                run(true, false, "(host.Size.X - r)"),
                false,
            ),
            (
                v2(r, r),
                v2(0.0, 0.0),
                v3(-r, -r),
                v3(1.0, 1.0),
                run(true, false, "(host.Size.X + host.Size.Y - 3 * r)"),
                true,
            ),
            (
                v2(-2.0 * r, f),
                v2(1.0, 0.0),
                v3(r, -f),
                v3(0.0, 1.0),
                run(false, true, "(2 * host.Size.X + host.Size.Y - 4 * r)"),
                false,
            ),
            (
                v2(r, r),
                v2(0.0, 0.0),
                v3(0.0, -r),
                v3(0.0, 1.0),
                run(false, true, "(2 * host.Size.X + host.Size.Y - 3 * r)"),
                true,
            ),
            (
                v2(f, -2.0 * r),
                v2(0.0, 1.0),
                v3(0.0, r),
                v3(0.0, 0.0),
                run(true, true, "(2 * host.Size.X + 2 * host.Size.Y - 5 * r)"),
                false,
            ),
            (
                v2(r, r),
                v2(0.0, 0.0),
                v3(0.0, 0.0),
                v3(0.0, 0.0),
                run(true, true, "(2 * host.Size.X + 2 * host.Size.Y - 4 * r)"),
                true,
            ),
        ];
        for (index, (size, relative_size, offset, relative_offset, run, corner)) in
            pieces.into_iter().enumerate()
        {
            let sprite = compositor.CreateSpriteVisual().ok()?;
            sprite.SetSize(size).ok()?;
            sprite.SetRelativeSizeAdjustment(relative_size).ok()?;
            sprite.SetOffset(offset).ok()?;
            sprite.SetRelativeOffsetAdjustment(relative_offset).ok()?;
            let colour = glow.wheel(&compositor, stops, &run)?;
            let mask = if corner {
                glow.corner_mask(&compositor, index / 2)?
            } else {
                glow.edge_mask(&compositor, index / 2)?
            };
            let brush = compositor.CreateMaskBrush().ok()?;
            brush.SetSource(&colour).ok()?;
            brush.SetMask(&mask).ok()?;
            sprite.SetBrush(&brush).ok()?;
            glow.root.Children().ok()?.InsertAtTop(&sprite).ok()?;
        }
        Some(glow)
    }

    /// The aurora along one run: a wrapping gradient whose scale is the
    /// band's length and whose offset follows the clock.
    fn wheel(
        &self,
        compositor: &Compositor,
        stops: &[[f64; 3]; 4],
        run: &Run,
    ) -> Option<CompositionLinearGradientBrush> {
        let wheel = compositor.CreateLinearGradientBrush().ok()?;
        wheel
            .SetMappingMode(CompositionMappingMode::Absolute)
            .ok()?;
        wheel
            .SetExtendMode(CompositionGradientExtendMode::Wrap)
            .ok()?;
        let sign = if run.reversed { -1.0 } else { 1.0 };
        let end = if run.vertical {
            v2(0.0, sign)
        } else {
            v2(sign, 0.0)
        };
        wheel.SetStartPoint(v2(0.0, 0.0)).ok()?;
        wheel.SetEndPoint(end).ok()?;
        let colour_stops = wheel.ColorStops().ok()?;
        for k in 0..=WHEEL_STOPS {
            let at = k as f64 / WHEEL_STOPS as f64;
            colour_stops
                .Append(
                    &compositor
                        .CreateColorGradientStopWithOffsetAndColor(at as f32, hue(stops, at))
                        .ok()?,
                )
                .ok()?;
        }
        let axis = |value: &str| {
            if run.vertical {
                format!("Vector2(0, {value})")
            } else {
                format!("Vector2({value}, 0)")
            }
        };
        let scale = if run.vertical {
            format!("Vector2(1, {PERIMETER})")
        } else {
            format!("Vector2({PERIMETER}, 1)")
        };
        // The colour at `s` along the band is the wheel at s / P - t.
        let offset = if run.reversed {
            axis(&format!("{} - clock.t * {PERIMETER}", run.start))
        } else {
            axis(&format!("clock.t * {PERIMETER} - {}", run.start))
        };
        for (property, expression) in [("Scale", scale), ("Offset", offset)] {
            let animation = compositor
                .CreateExpressionAnimationWithExpression(&HSTRING::from(expression))
                .ok()?;
            animation
                .SetReferenceParameter(&HSTRING::from("host"), &self.root)
                .ok()?;
            animation
                .SetReferenceParameter(&HSTRING::from("clock"), &self.clock)
                .ok()?;
            animation
                .SetScalarParameter(&HSTRING::from("r"), RADIUS * self.scale)
                .ok()?;
            wheel
                .StartAnimation(&HSTRING::from(property), &animation)
                .ok()?;
        }
        Some(wheel)
    }

    /// The falloff across an edge: `side` 0 top, 1 right, 2 bottom, 3 left.
    fn edge_mask(&self, compositor: &Compositor, side: usize) -> Option<CompositionBrush> {
        let mask = compositor.CreateLinearGradientBrush().ok()?;
        let (start, end) = match side {
            0 => (v2(0.0, 0.0), v2(0.0, 1.0)),
            1 => (v2(1.0, 0.0), v2(0.0, 0.0)),
            2 => (v2(0.0, 1.0), v2(0.0, 0.0)),
            _ => (v2(0.0, 0.0), v2(1.0, 0.0)),
        };
        mask.SetStartPoint(start).ok()?;
        mask.SetEndPoint(end).ok()?;
        self.falloff_stops(compositor, &mask.ColorStops().ok()?, FALLOFF)?;
        mask.cast().ok()
    }

    /// The line round a corner of `RADIUS`: `corner` 0 top-right, 1
    /// bottom-right, 2 bottom-left, 3 top-left.
    fn corner_mask(&self, compositor: &Compositor, corner: usize) -> Option<CompositionBrush> {
        let mask = compositor.CreateRadialGradientBrush().ok()?;
        mask.SetMappingMode(CompositionMappingMode::Absolute).ok()?;
        let r = RADIUS * self.scale;
        let centre = match corner {
            0 => v2(0.0, r),
            1 => v2(0.0, 0.0),
            2 => v2(r, 0.0),
            _ => v2(r, r),
        };
        mask.SetEllipseCenter(centre).ok()?;
        mask.SetEllipseRadius(v2(r, r)).ok()?;
        let stops = mask.ColorStops().ok()?;
        // Offset 1 is the corner's edge; inward is toward the centre.
        for k in 0..=16 {
            let at = k as f32 / 16.0;
            let alpha = band_alpha((1.0 - at) * RADIUS);
            stops
                .Append(
                    &compositor
                        .CreateColorGradientStopWithOffsetAndColor(at, white(alpha))
                        .ok()?,
                )
                .ok()?;
        }
        mask.cast().ok()
    }

    fn falloff_stops(
        &self,
        compositor: &Compositor,
        stops: &windows::UI::Composition::CompositionColorGradientStopCollection,
        depth: f32,
    ) -> Option<()> {
        // Enough stops that the falloff is a curve rather than a few
        // straight ramps, which DWM shows as bands.
        let mut points = vec![0.0, LINE - 0.5, LINE + 0.5];
        points.extend((1..=20).map(|k| LINE + 0.5 + (depth - LINE - 0.5) * k as f32 / 20.0));
        for inside in points {
            stops
                .Append(
                    &compositor
                        .CreateColorGradientStopWithOffsetAndColor(
                            (inside / depth).clamp(0.0, 1.0),
                            white(band_alpha(inside)),
                        )
                        .ok()?,
                )
                .ok()?;
        }
        Some(())
    }

    fn show(&self, glow: PageGlow, reduce_motion: bool) {
        self.generation.set(self.generation.get().wrapping_add(1));
        self.removal.borrow_mut().take();
        if self.shown.get() == PageGlow::Off {
            self.since.set(Instant::now());
        }
        self.asked.set(Instant::now());
        if !self.attached.replace(true) {
            let _ = self.layer.attach(&self.root);
            let mut client = RECT::default();
            // SAFETY: plain reads of the live container window.
            let (sized, visible) = unsafe {
                (
                    GetClientRect(self.window, &mut client).is_ok(),
                    IsWindowVisible(self.window).as_bool(),
                )
            };
            crate::page_reader::note(format_args!(
                "page_glow attached tab={} size={}x{} scale={} visible={} sized={sized} host=container",
                self.tab,
                client.right - client.left,
                client.bottom - client.top,
                self.scale,
                visible,
            ));
        }
        let travel = glow == PageGlow::Working && !reduce_motion;
        if travel && !self.travelling.get() {
            let _ = self.travel();
        } else if !travel && self.travelling.get() {
            let _ = self.clock.StopAnimation(&HSTRING::from("t"));
            self.travelling.set(false);
        }
        let peak = if self.dark { PEAK_DARK } else { PEAK_LIGHT };
        let target = if glow == PageGlow::Paused {
            peak * PAUSED_SHARE
        } else {
            peak
        };
        let _ = self.fade(target, FADE_IN);
        self.shown.set(glow);
    }

    /// The clock runs on from where it stopped; a whole trip later the
    /// colours are where they began, so each loop restarts unseen.
    fn travel(&self) -> Option<()> {
        let compositor = super::page_layer::compositor()?;
        let animation = compositor.CreateScalarKeyFrameAnimation().ok()?;
        let linear = compositor.CreateLinearEasingFunction().ok()?;
        animation
            .InsertExpressionKeyFrame(0.0, &HSTRING::from("this.StartingValue"))
            .ok()?;
        animation
            .InsertExpressionKeyFrameWithEasingFunction(
                1.0,
                &HSTRING::from("this.StartingValue + 1"),
                &linear,
            )
            .ok()?;
        animation.SetDuration(TimeSpan::from(LOOP)).ok()?;
        animation
            .SetIterationBehavior(AnimationIterationBehavior::Forever)
            .ok()?;
        self.clock
            .StartAnimation(&HSTRING::from("t"), &animation)
            .ok()?;
        self.travelling.set(true);
        Some(())
    }

    fn fade(&self, to: f32, duration: Duration) -> Option<()> {
        let compositor = super::page_layer::compositor()?;
        let animation = compositor.CreateScalarKeyFrameAnimation().ok()?;
        animation.InsertKeyFrame(1.0, to).ok()?;
        animation.SetDuration(TimeSpan::from(duration)).ok()?;
        self.root
            .StartAnimation(&HSTRING::from("Opacity"), &animation)
            .ok()
    }

    /// The light finishes coming on, holds, then fades and goes: a short
    /// run of steps is still seen.
    fn off(&self) {
        let shown = self.shown.replace(PageGlow::Off);
        if shown == PageGlow::Off {
            return;
        }
        let generation = self.generation.get().wrapping_add(1);
        self.generation.set(generation);
        let peak = match (self.dark, shown) {
            (true, PageGlow::Paused) => PEAK_DARK * PAUSED_SHARE,
            (true, _) => PEAK_DARK,
            (false, PageGlow::Paused) => PEAK_LIGHT * PAUSED_SHARE,
            (false, _) => PEAK_LIGHT,
        };
        let rise = FADE_IN.saturating_sub(self.asked.get().elapsed());
        let hold = HOLD.max(
            SHOWN_AT_LEAST
                .saturating_sub(self.since.get().elapsed())
                .saturating_sub(FADE_OUT),
        );
        let total = rise + hold + FADE_OUT;
        let _ = self.fade_out(peak, rise, hold, total);
        let current = self.generation.clone();
        let (layer, root, clock) = (self.layer.clone(), self.root.clone(), self.clock.clone());
        let (tab, since) = (self.tab, self.since.get());
        let detached =
            super::schedule_browser_timeout(total + Duration::from_millis(50), move || {
                if current.get() == generation {
                    let _ = clock.StopAnimation(&HSTRING::from("t"));
                    layer.detach(&root);
                    crate::page_reader::note(format_args!(
                        "page_glow removed tab={tab} reason=off shown_ms={}",
                        since.elapsed().as_millis()
                    ));
                }
            });
        *self.removal.borrow_mut() = detached;
        self.attached.set(false);
        self.travelling.set(false);
    }

    fn fade_out(&self, peak: f32, rise: Duration, hold: Duration, total: Duration) -> Option<()> {
        let compositor = super::page_layer::compositor()?;
        let animation = compositor.CreateScalarKeyFrameAnimation().ok()?;
        let at = |time: Duration| (time.as_secs_f32() / total.as_secs_f32()).clamp(0.0, 1.0);
        if !rise.is_zero() {
            animation.InsertKeyFrame(at(rise), peak).ok()?;
        }
        animation.InsertKeyFrame(at(rise + hold), peak).ok()?;
        animation.InsertKeyFrame(1.0, 0.0).ok()?;
        animation.SetDuration(TimeSpan::from(total)).ok()?;
        self.root
            .StartAnimation(&HSTRING::from("Opacity"), &animation)
            .ok()
    }
}

impl Drop for Glow {
    fn drop(&mut self) {
        if self.attached.get() {
            crate::page_reader::note(format_args!(
                "page_glow removed tab={} reason=closed shown_ms={}",
                self.tab,
                self.since.get().elapsed().as_millis()
            ));
        }
        let _ = self.clock.StopAnimation(&HSTRING::from("t"));
        self.layer.detach(&self.root);
    }
}

fn run(vertical: bool, reversed: bool, start: &'static str) -> Run {
    Run {
        vertical,
        reversed,
        start,
    }
}

fn v2(x: f32, y: f32) -> Vector2 {
    Vector2 { X: x, Y: y }
}

fn v3(x: f32, y: f32) -> Vector3 {
    Vector3 { X: x, Y: y, Z: 0.0 }
}

fn white(alpha: f32) -> Color {
    Color {
        A: (alpha.clamp(0.0, 1.0) * 255.0).round() as u8,
        R: 255,
        G: 255,
        B: 255,
    }
}
