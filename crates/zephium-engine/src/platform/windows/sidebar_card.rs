//! A hidden sidebar's card over the page. The chrome and each page are
//! sibling child windows, and siblings do not blend: raising the chrome
//! would paint its whole window over the page. Over the pages it is cut to
//! the card's rounded rectangle instead, following the card as it slides,
//! and the card's shadow is drawn on the pages beneath, by composition above
//! their WebView2, so it falls on the page right up to the card's edge.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

use windows::core::HSTRING;
use windows::Foundation::TimeSpan;
use windows::Win32::Foundation::{HWND, POINT, RECT};
use windows::Win32::Graphics::Gdi::{
    CombineRgn, CreateRectRgn, CreateRoundRectRgn, DeleteObject, GetWindowRgn, MapWindowPoints,
    OffsetRgn, SetRectRgn, SetWindowRgn, RGN_DIFF, RGN_ERROR, RGN_OR,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetClassNameW, GetWindow, GetWindowRect, IsWindow, IsWindowVisible, SetWindowPos, GW_CHILD,
    GW_HWNDNEXT, HWND_BOTTOM, HWND_TOP, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE,
};
use windows::UI::Color;
use windows::UI::Composition::{ContainerVisual, DropShadow, SpriteVisual};
use windows_numerics::{Vector2, Vector3};

use super::page_layer::PageLayer;
use crate::card_geometry::PixelRect;

/// The card's drop shadow as the frame draws it: 0 12px 36px.
const SHADOW_OFFSET: f32 = 12.0;
const SHADOW_BLUR: f32 = 36.0;
const SHADOW_OPACITY_DARK: f32 = 0.3;
const SHADOW_OPACITY_LIGHT: f32 = 0.14;
const FADE: Duration = Duration::from_millis(200);
/// How far in from the card's edge the shadow's own fill stays: past the
/// curve of the card's corners, and far too little to show in the shadow.
const FILL_INSET: f32 = 4.0;
/// The class wry gives every WebView2 host window.
const HOST_CLASS: &str = "WRY_WEBVIEW";

/// The chrome's host window among `parent`'s children: of those that are not
/// one of `pages`, the lowest, which wry made first.
pub(super) fn find_chrome(parent: HWND, pages: &[HWND]) -> Option<HWND> {
    let mut candidates = Vec::new();
    // SAFETY: plain reads of a live window's children.
    let mut child = unsafe { GetWindow(parent, GW_CHILD) }.ok();
    while let Some(window) = child {
        if !pages.contains(&window) {
            candidates.push(window);
        }
        // SAFETY: as above.
        child = unsafe { GetWindow(window, GW_HWNDNEXT) }.ok();
    }
    candidates
        .iter()
        .rev()
        .copied()
        .find(|window| class_of(*window) == HOST_CLASS)
        .or_else(|| candidates.last().copied())
}

fn class_of(window: HWND) -> String {
    let mut name = [0_u16; 32];
    // SAFETY: the buffer is writable for its whole length.
    let length = unsafe { GetClassNameW(window, &mut name) }.max(0) as usize;
    String::from_utf16_lossy(&name[..length.min(name.len())])
}

pub(super) fn is_live(window: HWND) -> bool {
    // SAFETY: a plain query of any handle value.
    unsafe { IsWindow(Some(window)) }.as_bool()
}

/// Whether the chrome is already the topmost of its siblings.
pub(super) fn is_on_top(parent: HWND, chrome: HWND) -> bool {
    // SAFETY: a plain read of a live window's topmost child.
    unsafe { GetWindow(parent, GW_CHILD) }.ok() == Some(chrome)
}

/// Cuts `chrome` to `card`, rounded by `diameter`, and to whatever of
/// `parent`'s `client` area its other windows leave bare: the chrome round
/// the pages stays in view, and the card comes over the page alone.
pub(super) fn clip_chrome(
    parent: HWND,
    chrome: HWND,
    client: (i32, i32),
    card: PixelRect,
    diameter: i32,
) -> bool {
    let (x, y, width, height) = card;
    // SAFETY: every region made here is handed to the window or deleted, and
    // the windows read are `parent`'s live children.
    unsafe {
        let region = CreateRectRgn(0, 0, client.0, client.1);
        if region.0.is_null() {
            return false;
        }
        let piece = CreateRectRgn(0, 0, 0, 0);
        let mut child = GetWindow(parent, GW_CHILD).ok();
        while let Some(window) = child {
            if window != chrome && IsWindowVisible(window).as_bool() && !piece.0.is_null() {
                let mut bounds = RECT::default();
                if GetWindowRect(window, &mut bounds).is_ok() {
                    let mut corners = [
                        POINT {
                            x: bounds.left,
                            y: bounds.top,
                        },
                        POINT {
                            x: bounds.right,
                            y: bounds.bottom,
                        },
                    ];
                    let _ = MapWindowPoints(None, Some(parent), &mut corners);
                    // A page is rounded by its own region, so the chrome shows
                    // in its corners as it does beneath it.
                    if GetWindowRgn(window, piece) == RGN_ERROR {
                        let _ = SetRectRgn(
                            piece,
                            0,
                            0,
                            corners[1].x - corners[0].x,
                            corners[1].y - corners[0].y,
                        );
                    }
                    let _ = OffsetRgn(piece, corners[0].x, corners[0].y);
                    let _ = CombineRgn(Some(region), Some(region), Some(piece), RGN_DIFF);
                }
            }
            child = GetWindow(window, GW_HWNDNEXT).ok();
        }
        if !piece.0.is_null() {
            let _ = DeleteObject(piece.into());
        }
        let rounded = CreateRoundRectRgn(
            x,
            y,
            x.saturating_add(width).saturating_add(1),
            y.saturating_add(height).saturating_add(1),
            diameter,
            diameter,
        );
        if !rounded.0.is_null() {
            let _ = CombineRgn(Some(region), Some(region), Some(rounded), RGN_OR);
            let _ = DeleteObject(rounded.into());
        }
        // The system owns the region only when SetWindowRgn succeeds.
        if SetWindowRgn(chrome, Some(region), true) == 0 {
            let _ = DeleteObject(region.into());
            return false;
        }
    }
    true
}

/// Puts `chrome` over its siblings, or back beneath them with its whole
/// window again.
pub(super) fn stack_chrome(chrome: HWND, over: bool) -> bool {
    // SAFETY: restacks one live child window without moving, sizing or
    // activating it.
    let stacked = unsafe {
        SetWindowPos(
            chrome,
            Some(if over { HWND_TOP } else { HWND_BOTTOM }),
            0,
            0,
            0,
            0,
            SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE,
        )
    }
    .is_ok();
    if !over {
        // SAFETY: dropping a window's region hands nothing to delete.
        return stacked && unsafe { SetWindowRgn(chrome, None, true) } != 0;
    }
    stacked
}

/// One page window's share of the card's shadow.
pub(super) struct Placement {
    pub container: HWND,
    pub rect: PixelRect,
}

struct Shadow {
    layer: Rc<PageLayer>,
    root: ContainerVisual,
    sprite: SpriteVisual,
    scale: f32,
    dark: bool,
    shown: Cell<bool>,
    generation: Rc<Cell<u64>>,
    removal: RefCell<Option<super::ContentPolicyTimeout>>,
}

thread_local! {
    static SHADOWS: RefCell<HashMap<isize, Shadow>> = RefCell::new(HashMap::new());
}

impl Shadow {
    fn new(container: HWND, scale: f32, dark: bool) -> Option<Self> {
        let compositor = super::page_layer::compositor()?;
        let layer = super::page_layer::layer(container)?;
        let root = compositor.CreateContainerVisual().ok()?;
        root.SetOpacity(0.0).ok()?;
        let sprite = compositor.CreateSpriteVisual().ok()?;
        let shadow: DropShadow = compositor.CreateDropShadow().ok()?;
        shadow.SetBlurRadius(SHADOW_BLUR * scale).ok()?;
        shadow
            .SetOffset(Vector3 {
                X: 0.0,
                Y: SHADOW_OFFSET * scale,
                Z: 0.0,
            })
            .ok()?;
        let (colour, opacity) = if dark {
            ((0, 0, 0), SHADOW_OPACITY_DARK)
        } else {
            ((24, 24, 27), SHADOW_OPACITY_LIGHT)
        };
        shadow
            .SetColor(Color {
                A: 255,
                R: colour.0,
                G: colour.1,
                B: colour.2,
            })
            .ok()?;
        shadow.SetOpacity(opacity).ok()?;
        // A shadow takes its shape from what the sprite paints, so it paints
        // the card's own rectangle. That stays beneath the raised chrome,
        // drawn in a little from the card's edge so its square corners never
        // show past the card's rounded ones.
        let fill = compositor
            .CreateColorBrushWithColor(Color {
                A: 255,
                R: colour.0,
                G: colour.1,
                B: colour.2,
            })
            .ok()?;
        sprite.SetBrush(&fill).ok()?;
        sprite.SetShadow(&shadow).ok()?;
        root.Children().ok()?.InsertAtTop(&sprite).ok()?;
        Some(Self {
            layer,
            root,
            sprite,
            scale,
            dark,
            shown: Cell::new(false),
            generation: Rc::new(Cell::new(0)),
            removal: RefCell::new(None),
        })
    }

    /// Lies the shadow where the card does, and brings it in if it is not.
    fn show(&self, offset: (i32, i32), size: (i32, i32)) {
        let inset = FILL_INSET * self.scale;
        let _ = self.sprite.SetOffset(Vector3 {
            X: offset.0 as f32 + inset,
            Y: offset.1 as f32 + inset,
            Z: 0.0,
        });
        let _ = self.sprite.SetSize(Vector2 {
            X: (size.0 as f32 - 2.0 * inset).max(0.0),
            Y: (size.1 as f32 - 2.0 * inset).max(0.0),
        });
        self.generation.set(self.generation.get().wrapping_add(1));
        self.removal.borrow_mut().take();
        if !self.shown.replace(true) {
            let _ = self.layer.attach(&self.root);
            let _ = self.fade(1.0);
        }
    }

    /// Fades the shadow out, then takes it off the page.
    fn hide(&self) {
        if !self.shown.replace(false) {
            return;
        }
        let _ = self.fade(0.0);
        let generation = self.generation.get().wrapping_add(1);
        self.generation.set(generation);
        let current = self.generation.clone();
        let (layer, root) = (self.layer.clone(), self.root.clone());
        *self.removal.borrow_mut() =
            super::schedule_browser_timeout(FADE + Duration::from_millis(50), move || {
                if current.get() == generation {
                    layer.detach(&root);
                }
            });
    }

    fn fade(&self, to: f32) -> Option<()> {
        let compositor = super::page_layer::compositor()?;
        let animation = compositor.CreateScalarKeyFrameAnimation().ok()?;
        animation.InsertKeyFrame(1.0, to).ok()?;
        animation.SetDuration(TimeSpan::from(FADE)).ok()?;
        self.root
            .StartAnimation(&HSTRING::from("Opacity"), &animation)
            .ok()
    }
}

impl Drop for Shadow {
    fn drop(&mut self) {
        self.layer.detach(&self.root);
    }
}

/// Casts the card's shadow on each of `pages` and takes it off every other
/// page window.
pub(super) fn cast(pages: &[Placement], card: PixelRect, scale: f64, dark: bool) {
    let scale = scale as f32;
    SHADOWS.with(|shadows| {
        let mut shadows = shadows.borrow_mut();
        let present: Vec<isize> = pages.iter().map(|page| page.container.0 as isize).collect();
        for (key, shadow) in shadows.iter() {
            if !present.contains(key) {
                shadow.hide();
            }
        }
        for page in pages {
            let key = page.container.0 as isize;
            if shadows
                .get(&key)
                .is_some_and(|shadow| shadow.scale != scale || shadow.dark != dark)
            {
                shadows.remove(&key);
            }
            if let std::collections::hash_map::Entry::Vacant(slot) = shadows.entry(key) {
                if let Some(shadow) = Shadow::new(page.container, scale, dark) {
                    slot.insert(shadow);
                }
            }
            if let Some(shadow) = shadows.get(&key) {
                let (offset, size) = crate::card_geometry::in_container(card, page.rect);
                shadow.show(offset, size);
            }
        }
    });
}

/// Fades the card's shadow from every page.
pub(super) fn lift() {
    SHADOWS.with(|shadows| shadows.borrow().values().for_each(Shadow::hide));
}

/// Drops the shadow on a page window that is going away.
pub(super) fn forget(container: HWND) {
    let removed = SHADOWS.with(|shadows| shadows.borrow_mut().remove(&(container.0 as isize)));
    drop(removed);
}
