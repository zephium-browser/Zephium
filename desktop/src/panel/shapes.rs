//! The launcher's material as two native shapes behind its web content: the
//! field capsule and the result sheet. A glass container merges shapes that
//! come close, so the sheet can grow out of the capsule and separate from it;
//! both animate on the compositor and the WebView never resizes for them.
//! The tab switcher borrows the field as its one plate meanwhile.
use std::cell::{Cell, RefCell};
use std::ptr::NonNull;

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::MainThreadMarker;
use objc2_app_kit::{
    NSAnimatablePropertyContainer, NSAnimationContext, NSAppKitVersionNumber,
    NSAutoresizingMaskOptions, NSColor, NSGlassEffectContainerView, NSGlassEffectView,
    NSGlassEffectViewStyle, NSView, NSVisualEffectBlendingMode, NSVisualEffectMaterial,
    NSVisualEffectState, NSVisualEffectView, NSWindow, NSWindowOrderingMode,
};
use objc2_foundation::{NSPoint, NSRect, NSSize};
use objc2_quartz_core::CAMediaTimingFunction;
use zephium_ipc::{PanelLayout, PanelRect};

use crate::material::Material;
use crate::overlay::geometry::SWITCHER_RADIUS;

/// Liquid Glass shipped with AppKit 26.
const GLASS_APPKIT: f64 = 2685.0;
/// Taller than any launcher window. The shapes live in a canvas pinned to the
/// window's top edge, so resizing the window never moves them.
const CANVAS: f64 = 2048.0;
/// Shapes closer than this merge. Below the resting gap between capsule and
/// sheet, so they read as two objects at rest and as one only in motion.
const MERGE: f64 = 8.0;
const SHEET_RADIUS: f64 = 24.0;
/// Matches the frame's `--launcher-settle` duration and curve, which moves the
/// web content over the same interval.
const SETTLE: f64 = 0.26;
const CURVE: [f32; 4] = [0.2, 0.8, 0.2, 1.0];
/// Arrival is longer than a settle and overshoots by a hair, like a spring
/// that is almost critically damped.
const ARRIVE: f64 = 0.42;
const ARRIVE_CURVE: [f32; 4] = [0.22, 1.08, 0.36, 1.0];

struct Shapes {
    canvas: Retained<NSView>,
    field: Retained<NSView>,
    sheet: Retained<NSView>,
    glass: bool,
}

thread_local! {
    static SHAPES: RefCell<Option<Shapes>> = const { RefCell::new(None) };
    static LAST: Cell<Option<PanelLayout>> = const { Cell::new(None) };
    /// Revision of the latest layout, so a collapse that finishes after the
    /// sheet was asked back does not hide it.
    static REVISION: Cell<u64> = const { Cell::new(0) };
    /// The switcher's plate while it stands in the launcher's place.
    static HELD: Cell<Option<PanelRect>> = const { Cell::new(None) };
}

fn tint(dark: bool, field: bool) -> Retained<NSColor> {
    // The field is a touch denser than the sheet: it is the object you act on,
    // the sheet is something you read through.
    let (base, alpha) = if dark {
        (22.0 / 255.0, if field { 0.34 } else { 0.22 })
    } else {
        (250.0 / 255.0, if field { 0.42 } else { 0.30 })
    };
    NSColor::colorWithRed_green_blue_alpha(base, base, base + 0.01, alpha)
}

fn glass_shape(mtm: MainThreadMarker, dark: bool, field: bool) -> Retained<NSView> {
    let view = NSGlassEffectView::initWithFrame(mtm.alloc(), NSRect::ZERO);
    view.setStyle(NSGlassEffectViewStyle::Regular);
    view.setTintColor(Some(&tint(dark, field)));
    view.setCornerRadius(SHEET_RADIUS);
    Retained::into_super(view)
}

fn vibrancy_shape(mtm: MainThreadMarker) -> Retained<NSView> {
    let view = NSVisualEffectView::initWithFrame(mtm.alloc(), NSRect::ZERO);
    view.setMaterial(NSVisualEffectMaterial::HUDWindow);
    view.setBlendingMode(NSVisualEffectBlendingMode::BehindWindow);
    view.setState(NSVisualEffectState::Active);
    view.setWantsLayer(true);
    if let Some(layer) = view.layer() {
        layer.setCornerRadius(SHEET_RADIUS);
        layer.setMasksToBounds(true);
    }
    Retained::into_super(view)
}

fn radius(view: &NSView, glass: bool, value: f64) {
    if glass {
        if let Some(glass) = view.downcast_ref::<NSGlassEffectView>() {
            glass.setCornerRadius(value);
        }
    } else if let Some(layer) = view.layer() {
        layer.setCornerRadius(value);
    }
}

fn frame(rect: &PanelRect) -> NSRect {
    NSRect::new(
        NSPoint::new(rect.x, CANVAS - rect.y - rect.height),
        NSSize::new(rect.width, rect.height),
    )
}

/// Replaces any installed shapes for the current appearance and returns the
/// material now behind the launcher. `None` hands the whole surface to CSS,
/// which draws one opaque card with the window's own shadow.
pub fn install(window: &NSWindow, reduce_transparency: bool, dark: bool) -> Material {
    let Some(mtm) = MainThreadMarker::new() else {
        return Material::None;
    };
    remove();
    let Some(content) = window.contentView() else {
        return Material::None;
    };
    content.setWantsLayer(true);
    let material = if reduce_transparency {
        Material::None
    } else if unsafe { NSAppKitVersionNumber } >= GLASS_APPKIT {
        Material::LiquidGlass
    } else {
        Material::Vibrancy
    };
    if material == Material::None {
        if let Some(layer) = content.layer() {
            layer.setCornerRadius(crate::overlay::PANEL_RADIUS.into());
            layer.setMasksToBounds(true);
        }
        window.setHasShadow(true);
        window.invalidateShadow();
        return material;
    }
    // Each shape carries its own edge; a window shadow would outline the
    // transparent rectangle around them instead.
    if let Some(layer) = content.layer() {
        layer.setCornerRadius(0.0);
        layer.setMasksToBounds(false);
    }
    window.setHasShadow(false);

    let bounds = content.bounds();
    let canvas_frame = NSRect::new(
        NSPoint::new(0.0, bounds.size.height - CANVAS),
        NSSize::new(bounds.size.width, CANVAS),
    );
    let glass = material == Material::LiquidGlass;
    let (canvas, host, field, sheet) = if glass {
        let container = NSGlassEffectContainerView::initWithFrame(mtm.alloc(), canvas_frame);
        container.setSpacing(MERGE);
        let host =
            NSView::initWithFrame(mtm.alloc(), NSRect::new(NSPoint::ZERO, canvas_frame.size));
        container.setContentView(Some(&host));
        (
            Retained::into_super(container),
            host,
            glass_shape(mtm, dark, true),
            glass_shape(mtm, dark, false),
        )
    } else {
        let canvas = NSView::initWithFrame(mtm.alloc(), canvas_frame);
        let host = canvas.clone();
        (canvas, host, vibrancy_shape(mtm), vibrancy_shape(mtm))
    };
    canvas.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMinYMargin);
    sheet.setHidden(true);
    host.addSubview(&sheet);
    host.addSubview(&field);
    content.addSubview_positioned_relativeTo(&canvas, NSWindowOrderingMode::Below, None);
    SHAPES.with(|shapes| {
        *shapes.borrow_mut() = Some(Shapes {
            canvas,
            field,
            sheet,
            glass,
        });
    });
    if let Some(layout) = LAST.with(Cell::get) {
        apply(&layout, false);
    }
    material
}

fn remove() {
    if let Some(shapes) = SHAPES.with(|shapes| shapes.borrow_mut().take()) {
        shapes.canvas.removeFromSuperview();
    }
}

pub fn active() -> bool {
    SHAPES.with(|shapes| shapes.borrow().is_some())
}

/// Moves the shapes to what the content reports. The sheet grows out of the
/// capsule when it first appears and returns into it when it goes.
pub fn apply(layout: &PanelLayout, animate: bool) {
    HELD.with(|held| held.set(None));
    let revision = REVISION.with(|revision| {
        revision.set(revision.get().wrapping_add(1));
        revision.get()
    });
    let previous = LAST.with(|last| last.replace(Some(*layout)));
    SHAPES.with(|shapes| {
        let shapes = shapes.borrow();
        let Some(shapes) = shapes.as_ref() else {
            return;
        };
        let field = frame(&layout.field);
        radius(&shapes.field, shapes.glass, layout.field.height / 2.0);
        shapes.field.setFrame(field);
        let showing = !shapes.sheet.isHidden();
        match layout.sheet.as_ref().map(frame) {
            Some(target) => {
                if !showing || previous.is_none_or(|previous| previous.sheet.is_none()) {
                    shapes.sheet.setFrame(field);
                    radius(&shapes.sheet, shapes.glass, layout.field.height / 2.0);
                    shapes.sheet.setHidden(false);
                }
                let sheet = shapes.sheet.clone();
                let glass = shapes.glass;
                settle(
                    animate,
                    move || {
                        radius(&sheet, glass, SHEET_RADIUS);
                        sheet.animator().setFrame(target);
                    },
                    || {},
                );
            }
            None if showing => {
                let sheet = shapes.sheet.clone();
                let done = sheet.clone();
                settle(
                    animate,
                    move || sheet.animator().setFrame(field),
                    move || {
                        if REVISION.with(Cell::get) == revision {
                            done.setHidden(true);
                        }
                    },
                );
            }
            None => {}
        }
    });
}

/// The switcher's plate: the field alone, at the plate's corner. The
/// launcher's own layout is kept for when it returns.
pub fn hold(plate: Option<PanelRect>) {
    HELD.with(|held| held.set(plate));
    let Some(plate) = plate else {
        return;
    };
    REVISION.with(|revision| revision.set(revision.get().wrapping_add(1)));
    SHAPES.with(|shapes| {
        let shapes = shapes.borrow();
        let Some(shapes) = shapes.as_ref() else {
            return;
        };
        radius(&shapes.field, shapes.glass, SWITCHER_RADIUS);
        shapes.field.setFrame(frame(&plate));
        shapes.sheet.setHidden(true);
    });
}

/// The launcher arriving: the capsule grows in from slightly smaller and the
/// sheet flows out of it, on a curve with a touch of overshoot so it lands
/// rather than stops. With motion reduced the shapes are simply in place.
/// The switcher's plate arrives the same way, alone.
pub fn present(calm: bool) {
    let held = HELD.with(Cell::get);
    let Some(layout) = held
        .map(|plate| PanelLayout {
            height: plate.y + plate.height,
            field: plate,
            sheet: None,
        })
        .or_else(|| LAST.with(Cell::get))
    else {
        return;
    };
    let corner = if held.is_some() {
        SWITCHER_RADIUS
    } else {
        layout.field.height / 2.0
    };
    REVISION.with(|revision| revision.set(revision.get().wrapping_add(1)));
    SHAPES.with(|shapes| {
        let shapes = shapes.borrow();
        let Some(shapes) = shapes.as_ref() else {
            return;
        };
        let field = frame(&layout.field);
        let sheet = layout.sheet.as_ref().map(frame);
        radius(&shapes.field, shapes.glass, corner);
        if sheet.is_none() {
            shapes.sheet.setHidden(true);
        }
        if calm {
            shapes.field.setFrame(field);
            if let Some(sheet) = sheet {
                radius(&shapes.sheet, shapes.glass, SHEET_RADIUS);
                shapes.sheet.setFrame(sheet);
                shapes.sheet.setHidden(false);
            }
            return;
        }
        let (dx, dy) = (field.size.width * 0.02, field.size.height * 0.04);
        shapes.field.setFrame(NSRect::new(
            NSPoint::new(field.origin.x + dx, field.origin.y + dy),
            NSSize::new(field.size.width - 2.0 * dx, field.size.height - 2.0 * dy),
        ));
        if sheet.is_some() {
            shapes.sheet.setFrame(field);
            radius(&shapes.sheet, shapes.glass, SHEET_RADIUS);
            shapes.sheet.setHidden(false);
        }
        let capsule = shapes.field.clone();
        let flowing = shapes.sheet.clone();
        run(
            ARRIVE,
            ARRIVE_CURVE,
            move || {
                capsule.animator().setFrame(field);
                if let Some(sheet) = sheet {
                    flowing.animator().setFrame(sheet);
                }
            },
            || {},
        );
    });
}

fn settle(animate: bool, changes: impl Fn() + 'static, done: impl Fn() + 'static) {
    if animate {
        run(SETTLE, CURVE, changes, done);
    } else {
        changes();
        done();
    }
}

fn run(duration: f64, curve: [f32; 4], changes: impl Fn() + 'static, done: impl Fn() + 'static) {
    let changes = RcBlock::new(move |context: NonNull<NSAnimationContext>| {
        // SAFETY: AppKit passes the live context for the duration of the block.
        let context = unsafe { context.as_ref() };
        context.setDuration(duration);
        context.setAllowsImplicitAnimation(true);
        let [a, b, c, d] = curve;
        context.setTimingFunction(Some(&CAMediaTimingFunction::functionWithControlPoints(
            a, b, c, d,
        )));
        changes();
    });
    let done = RcBlock::new(done);
    NSAnimationContext::runAnimationGroup_completionHandler(&changes, Some(&done));
}
