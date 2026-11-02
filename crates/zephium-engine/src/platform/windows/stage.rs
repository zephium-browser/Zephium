//! Win32 mirror of the macOS ContentStage. wry already wraps each child
//! webview in its own HWND; the stage positions and rounds those directly,
//! so corner cutouts and pane gaps show the chrome webview behind them.
//! Divider drags are DOM strips in the chrome (layered child windows drop
//! mouse input on Win8+, an MS-confirmed bug). The drop indicator is a
//! top-level per-pixel-alpha layered window in screen coordinates.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::{Rc, Weak};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::sync::OnceLock;

use webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2Controller;
use windows::core::{BOOL, PCWSTR};
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM};
use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_USE_IMMERSIVE_DARK_MODE};
use windows::Win32::Graphics::Gdi::{
    ClientToScreen, CombineRgn, CreateCompatibleDC, CreateDIBSection, CreateRoundRectRgn,
    CreateSolidBrush, DeleteDC, DeleteObject, FillRect, GetDC, ReleaseDC, SelectObject,
    SetWindowRgn, AC_SRC_ALPHA, AC_SRC_OVER, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, BLENDFUNCTION,
    DIB_RGB_COLORS, HDC, RGN_AND,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, GetAncestor, GetClientRect, GetWindowLongPtrW,
    KillTimer, LoadCursorW, RegisterClassW, SetTimer, SetWindowLongPtrW, SetWindowPos, ShowWindow,
    UpdateLayeredWindow, GA_ROOT, GWLP_USERDATA, HWND_TOP, IDC_ARROW, MA_NOACTIVATE,
    SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, SW_HIDE, SW_SHOWNA, ULW_ALPHA,
    USER_TIMER_MINIMUM, WM_ERASEBKGND, WM_MOUSEACTIVATE, WNDCLASSW, WS_CHILD, WS_CLIPSIBLINGS,
    WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TRANSPARENT, WS_POPUP,
};
use wry::WebViewExtWindows;

use zephium_core::geometry::Rect;
use zephium_core::ids::ItemId;
use zephium_core::ports::engine::{DropPreview, StageMotion};
use zephium_core::split::{self, Pane};

use crate::motion_curve::EMPHASIZED;
use std::time::{Duration, Instant};

use crate::pane_geometry::rounded_native_size;

const RADIUS: f64 = 8.0;
const INDICATOR_RADIUS: f64 = 10.0;
const INDICATOR_BORDER: f64 = 1.0;
// The wash over where a dropped tab will stand: the label colour, faint.
const INDICATOR_FILL: f64 = 0.06;
const INDICATOR_STROKE: f64 = 0.16;
/// How long the pointer rests on a drop target before the page makes room.
const DROP_DWELL_MS: u32 = 450;
// The resize guide is a neutral grey either way: it lies over a page, and a page
// may be white under a dark window or black under a light one.
const GUIDE_ON_DARK: (u8, u8, u8) = (0xa1, 0xa1, 0xa6);
const GUIDE_ON_LIGHT: (u8, u8, u8) = (0x8e, 0x8e, 0x93);
const MAX_NATIVE_RETRIES: u8 = 2;

type PhysicalRect = (i32, i32, i32, i32);

/// Native state that has actually been applied. Each field is independent so
/// a failed COM call is retried without replaying successful calls.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct AppliedPlacement {
    window_visible: Option<bool>,
    controller_visible: Option<bool>,
    container_rect: Option<PhysicalRect>,
    controller_size: Option<(i32, i32)>,
    rounded: Option<(i32, i32, i32)>,
    notified_screen_origin: Option<(i32, i32)>,
    /// Raised above every sibling for the fullscreen page it shows.
    raised: Option<bool>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct PlacementDelta {
    window_visibility: bool,
    controller_visibility: bool,
    container_rect: bool,
    controller_size: bool,
    rounded: bool,
    notify_parent_position: bool,
    raise: bool,
}

impl PlacementDelta {
    fn is_empty(self) -> bool {
        self == Self::default()
    }
}

fn placement_delta(
    applied: AppliedPlacement,
    visible: bool,
    rect: Option<PhysicalRect>,
    radius: i32,
    fullscreen: bool,
    parent_screen_origin: Option<(i32, i32)>,
) -> PlacementDelta {
    let mut delta = PlacementDelta {
        window_visibility: applied.window_visible != Some(visible),
        controller_visibility: applied.controller_visible != Some(visible),
        ..PlacementDelta::default()
    };
    if !visible {
        // Hidden views retain their old geometry. It is cheaper and safer to
        // apply the latest geometry once, immediately before they are shown.
        return delta;
    }
    let Some((x, y, width, height)) = rect else {
        return delta;
    };
    delta.container_rect = applied.container_rect != rect;
    delta.controller_size = applied.controller_size != Some((width, height));
    delta.rounded = applied.rounded != Some((width, height, radius));
    delta.raise = applied.raised != Some(fullscreen);
    delta.notify_parent_position = parent_screen_origin.is_some_and(|(parent_x, parent_y)| {
        applied.notified_screen_origin
            != Some((parent_x.saturating_add(x), parent_y.saturating_add(y)))
    });
    delta
}

struct HostView {
    container: HWND,
    controller: ICoreWebView2Controller,
    serial: u64,
    applied: Cell<AppliedPlacement>,
    consecutive_failures: Cell<u8>,
    terminal_failure_reported: Cell<bool>,
    generation: Arc<AtomicBool>,
    presentation_permit: Arc<AtomicBool>,
}

type PlacementFailureCallback = Rc<dyn Fn(ItemId, Arc<AtomicBool>)>;

struct State {
    fullscreen_transition: Option<super::fullscreen_transition::FullscreenTransition>,
    fullscreen_restore: Option<crate::fullscreen::ScreenRect>,
    parent: HWND,
    gap: f64,
    origin: (f64, f64),
    size: (f64, f64),
    hidden: bool,
    tree: Option<Pane>,
    views: HashMap<ItemId, HostView>,
    // Logical visibility alone must not expose a newly constructed
    // controller's blank backing store. Every identity-bearing commit removes
    // its leaf; exact URL attribution plus the shell's identity-bound
    // acknowledgement inserts it again.
    ready: HashSet<ItemId>,
    visible: HashSet<ItemId>,
    dirty: HashSet<ItemId>,
    // A stale native visibility call can pump and return after a newer
    // placement updated `applied`. Keep uncertainty separate instead of
    // overwriting that newer cache; the next pass forcibly reasserts both
    // HWND and controller visibility for the authoritative revision.
    visibility_uncertain: HashSet<ItemId>,
    revision: u64,
    next_serial: u64,
    layout_timer: Option<usize>,
    indicator: Option<HWND>,
    indicator_size: (i32, i32),
    indicator_resize: bool,
    /// A drop target the pointer has reached but not yet rested on, and the
    /// timer that shows it.
    armed: Option<DropPreview>,
    dwell_timer: Option<usize>,
    preview: Option<PreviewMove>,
    on_placement_failure: PlacementFailureCallback,
    /// Motion asked of the next `apply`, and the slide it started.
    pending_motion: Option<StageMotion>,
    slide: Option<Slide>,
    caption_offset: f64,
    caption_moving: bool,
    /// Paint covers by view, each with the token of the handoff that owns it.
    covers: HashMap<ItemId, (HWND, u64)>,
    /// The page shown fullscreen: square corners, above everything else.
    fullscreen: Option<ItemId>,
    /// The chrome's host window, once a hidden sidebar's card has needed it.
    chrome: Option<HWND>,
    /// Where the hidden sidebar's card stands over the page, while the
    /// chrome is up for it, and what has been cut to for it.
    card: Option<Rect>,
    card_clip: Option<(PhysicalRect, i32, Vec<PhysicalRect>)>,
    /// The card on its way in or out, and the timer that steps the cut.
    card_motion: Option<CardMotion>,
    card_tick: Option<super::ContentPolicyTimeout>,
}

/// A hidden sidebar's card sliding in or out, as the share of its slide that
/// lies between it and its place: 0 over the page, 1 out of sight beside the
/// window. The frame slides the card; the chrome's cut follows it there.
#[derive(Clone, Copy, Debug)]
struct CardMotion {
    started: Instant,
    from: f64,
    to: f64,
}

/// How far past its own width the card waits beside the window, as the
/// frame's `translate: calc(-100% - 24px)` puts it.
const CARD_AWAY: f64 = 24.0;

impl CardMotion {
    /// The share still to go, and whether the card has arrived.
    fn at(&self, now: Instant) -> (f64, bool) {
        let elapsed = now.saturating_duration_since(self.started);
        if elapsed >= SLIDE {
            return (self.to, true);
        }
        let progress = EMPHASIZED.at(elapsed.as_secs_f64() / SLIDE.as_secs_f64());
        (self.from + (self.to - self.from) * progress, false)
    }
}

/// The page travelling beside the sidebar. Child windows cannot be moved by
/// a compositor here, so the stage moves them itself, a little each display
/// frame, without ever resizing them mid-journey: each pane keeps whichever
/// of its two sizes is wider for the whole of it, so no page lays itself
/// out more than once and no edge opens a gap.
#[derive(Clone, Copy, Debug)]
struct Slide {
    started: Instant,
    /// The logical geometry the panes are laid out in for the journey.
    origin: (f64, f64),
    size: (f64, f64),
    /// Horizontal offset in logical pixels, at the start and at the end.
    from: f64,
    to: f64,
    /// It set off from under a hidden sidebar's card, which the page gives
    /// way to as it travels.
    beside: bool,
}

/// The chrome's --motion-page, so the page and the sidebar travel together.
const SLIDE: Duration = Duration::from_millis(400);

impl Slide {
    /// Where the journey has got to, or `None` once it has arrived.
    fn offset(&self, now: Instant) -> Option<f64> {
        let elapsed = now.saturating_duration_since(self.started);
        if elapsed >= SLIDE {
            return None;
        }
        let progress = EMPHASIZED.at(elapsed.as_secs_f64() / SLIDE.as_secs_f64());
        Some(self.from + (self.to - self.from) * progress)
    }
}

/// The page a drop preview slides aside: it keeps its size and only moves,
/// like a journey, and rests at `to` until the preview ends and it comes home.
#[derive(Clone, Copy, Debug)]
struct PreviewMove {
    id: ItemId,
    started: Instant,
    from: (f64, f64),
    to: (f64, f64),
    /// Coming home: dropped from the stage once it has arrived.
    release: bool,
}

impl PreviewMove {
    fn arrived(&self, now: Instant) -> bool {
        now.saturating_duration_since(self.started) >= SLIDE
    }

    fn at(&self, now: Instant) -> (f64, f64) {
        let elapsed = now.saturating_duration_since(self.started);
        let progress = if elapsed >= SLIDE {
            1.0
        } else {
            EMPHASIZED.at(elapsed.as_secs_f64() / SLIDE.as_secs_f64())
        };
        (
            self.from.0 + (self.to.0 - self.from.0) * progress,
            self.from.1 + (self.to.1 - self.from.1) * progress,
        )
    }
}

thread_local! {
    static CAPTION_STAGES: RefCell<Vec<(isize, Weak<RefCell<State>>)>> = const { RefCell::new(Vec::new()) };
    static CAPTION_OFFSETS: RefCell<HashMap<isize, (f64, bool)>> = RefCell::new(HashMap::new());
    /// `SetTimer` callbacks run on the window thread. A thread-local weak map
    /// keeps pending layout work non-owning and avoids making COM controllers
    /// or `Rc` state cross-thread.
    static LAYOUT_TIMERS: RefCell<HashMap<usize, Weak<RefCell<State>>>> = RefCell::new(HashMap::new());
    static DWELL_TIMERS: RefCell<HashMap<usize, Weak<RefCell<State>>>> = RefCell::new(HashMap::new());
}

/// Translate page containers on their owning UI thread, preserving controller
/// dimensions. The caption's single animation clock drives both page and controls.
/// A weak registry cannot extend a stage or a WebView's lifetime.
pub fn set_window_caption_offset(parent: isize, offset: f64, moving: bool) {
    if !offset.is_finite() || !(0.0..=64.0).contains(&offset) {
        return;
    }
    CAPTION_OFFSETS.with(|offsets| {
        let mut offsets = offsets.borrow_mut();
        if offset == 0.0 && !moving {
            offsets.remove(&parent);
        } else {
            offsets.insert(parent, (offset, moving));
        }
    });
    let stages = CAPTION_STAGES.with(|stages| {
        let mut stages = stages.borrow_mut();
        stages.retain(|(_, stage)| stage.strong_count() > 0);
        stages
            .iter()
            .filter(|(owner, _)| *owner == parent)
            .filter_map(|(_, stage)| stage.upgrade())
            .collect::<Vec<_>>()
    });
    for stage in stages {
        let changed = if let Ok(mut state) = stage.try_borrow_mut() {
            if state.parent.0 as isize != parent
                || (state.caption_offset == offset && state.caption_moving == moving)
            {
                false
            } else {
                state.caption_offset = offset;
                state.caption_moving = moving;
                state.revision = state.revision.wrapping_add(1);
                let visible = state.visible.clone();
                state.dirty.extend(visible);
                if offset == 0.0 {
                    for view in state.views.values() {
                        let mut applied = view.applied.get();
                        applied.rounded = None;
                        view.applied.set(applied);
                    }
                }
                true
            }
        } else {
            false
        };
        if changed {
            sync(&stage);
        }
    }
}

/// Retire window-local motion without touching child HWNDs during destruction.
pub fn forget_window_caption_offset(parent: isize) {
    CAPTION_OFFSETS.with(|offsets| offsets.borrow_mut().remove(&parent));
    CAPTION_STAGES.with(|stages| stages.borrow_mut().retain(|(owner, _)| *owner != parent));
}

impl Drop for State {
    fn drop(&mut self) {
        if let Some(timer) = self.layout_timer.take() {
            let _ = unsafe { KillTimer(None, timer) };
            let _ = LAYOUT_TIMERS.try_with(|timers| {
                timers.borrow_mut().remove(&timer);
            });
        }
        if let Some(indicator) = self.indicator.take() {
            let _ = unsafe { DestroyWindow(indicator) };
        }
        if let Some(timer) = self.dwell_timer.take() {
            let _ = unsafe { KillTimer(None, timer) };
            let _ = DWELL_TIMERS.try_with(|timers| {
                timers.borrow_mut().remove(&timer);
            });
        }
    }
}

#[derive(Clone)]
pub struct Stage {
    state: Rc<RefCell<State>>,
}

impl Stage {
    pub(crate) fn parent_window(&self) -> Option<HWND> {
        self.state.try_borrow().ok().map(|state| state.parent)
    }

    pub fn new(
        parent: HWND,
        gap: f64,
        on_placement_failure: impl Fn(ItemId, Arc<AtomicBool>) + 'static,
    ) -> Self {
        let (caption_offset, caption_moving) = CAPTION_OFFSETS.with(|offsets| {
            offsets
                .borrow()
                .get(&(parent.0 as isize))
                .copied()
                .unwrap_or_default()
        });
        let state = Rc::new(RefCell::new(State {
            fullscreen_transition: None,
            fullscreen_restore: None,
            parent,
            gap,
            origin: (0.0, 0.0),
            size: (0.0, 0.0),
            hidden: true,
            tree: None,
            views: HashMap::new(),
            ready: HashSet::new(),
            visible: HashSet::new(),
            dirty: HashSet::new(),
            visibility_uncertain: HashSet::new(),
            revision: 0,
            next_serial: 1,
            layout_timer: None,
            indicator: None,
            indicator_size: (0, 0),
            indicator_resize: false,
            armed: None,
            dwell_timer: None,
            preview: None,
            on_placement_failure: Rc::new(on_placement_failure),
            pending_motion: None,
            slide: None,
            caption_offset,
            caption_moving,
            covers: HashMap::new(),
            fullscreen: None,
            chrome: None,
            card: None,
            card_clip: None,
            card_motion: None,
            card_tick: None,
        }));
        CAPTION_STAGES.with(|stages| {
            stages
                .borrow_mut()
                .push((parent.0 as isize, Rc::downgrade(&state)))
        });
        Self { state }
    }

    /// One native pass for frame, tree and visibility; `None` region hides
    /// the whole stage.
    /// Asks the next `apply` to move rather than jump; consumed by it.
    pub fn hint_motion(&self, motion: StageMotion) {
        if let Ok(mut state) = self.state.try_borrow_mut() {
            state.pending_motion = Some(motion);
        }
    }

    pub fn apply(&self, region: Option<Rect>, tree: Option<Pane>, visible: &[ItemId]) -> bool {
        let hide_now = {
            let Ok(mut s) = self.state.try_borrow_mut() else {
                return false;
            };
            // A slide is started only by a deliberate change of shape of a
            // stage already on screen, where nothing but the horizontal
            // extent changes. Any other layout ends a journey in progress.
            // A layout is the drop, or a change that outdates the preview:
            // either way the pages are about to stand where they belong.
            if let Some(moving) = s.preview.take() {
                s.dirty.insert(moving.id);
                if let Some(view) = s.views.get(&moving.id) {
                    let mut applied = view.applied.get();
                    applied.rounded = None;
                    view.applied.set(applied);
                }
            }
            let motion = s.pending_motion.take().filter(|_| {
                !s.fullscreen_transition
                    .as_ref()
                    .is_some_and(|transition| transition.active())
            });
            s.slide = match (motion, region) {
                (Some(StageMotion::Slide), Some(r))
                    if !s.hidden
                        && !s.visible.is_empty()
                        && s.origin.1 == r.y
                        && s.size.1 == r.height
                        && s.origin.0 != r.x =>
                {
                    let started = Instant::now();
                    Some(if r.width >= s.size.0 {
                        // Wider: take the new size now, slide from where it was.
                        Slide {
                            started,
                            origin: (r.x, r.y),
                            size: (r.width, r.height),
                            from: s.origin.0 - r.x,
                            to: 0.0,
                            beside: s.card.is_some(),
                        }
                    } else {
                        // Narrower: keep the old size for the journey.
                        Slide {
                            started,
                            origin: s.origin,
                            size: s.size,
                            from: 0.0,
                            to: r.x - s.origin.0,
                            beside: s.card.is_some(),
                        }
                    })
                }
                _ => None,
            };
            let next_visible = if region.is_some() {
                visible.iter().copied().collect::<HashSet<_>>()
            } else {
                HashSet::new()
            };
            let hide_now = s
                .visible
                .difference(&next_visible)
                .copied()
                .collect::<Vec<_>>();
            let changed = s
                .visible
                .symmetric_difference(&next_visible)
                .copied()
                .collect::<Vec<_>>();
            s.dirty.extend(changed);
            // Geometry/DPI/parent-position changes only affect the currently
            // visible split leaves, never every tab accumulated in the host.
            s.dirty.extend(next_visible.iter().copied());
            s.hidden = region.is_none();
            if let Some(r) = region {
                s.origin = (r.x, r.y);
                s.size = (r.width, r.height);
            }
            s.tree = tree;
            s.visible = next_visible;
            s.revision = s.revision.wrapping_add(1);
            hide_now
        };
        // Preserve the old no-flash invariant: obsolete panes are hidden in
        // this call; positioning/showing the latest panes is coalesced.
        hide_views(&self.state, &hide_now);
        if let Some(transition) = self.state.borrow().fullscreen_transition.clone() {
            transition.layout_changed();
        }
        schedule_sync(&self.state);
        true
    }

    pub fn has_view(&self, id: ItemId) -> bool {
        self.state.borrow().views.contains_key(&id)
    }

    /// Marks the page shown fullscreen, which the shell lays over the whole
    /// window: it loses its rounded corners and is raised above siblings.
    pub fn set_fullscreen(&self, id: Option<ItemId>) {
        let Ok(mut state) = self.state.try_borrow_mut() else {
            return;
        };
        if state.fullscreen == id {
            return;
        }
        let previous = std::mem::replace(&mut state.fullscreen, id);
        state.dirty.extend(previous.into_iter().chain(id));
        state.slide = None;
        state.pending_motion = None;
        let parent = state.parent;
        let controller = id
            .or(previous)
            .and_then(|owner| state.views.get(&owner).map(|view| view.controller.clone()));
        let restore = if id.is_none() {
            state.fullscreen_restore.take()
        } else {
            None
        };
        drop(state);
        if id.is_some() && previous.is_none() {
            self.state.borrow_mut().fullscreen_restore =
                super::fullscreen_transition::client_rect(parent);
        }
        let transition = super::fullscreen_transition::FullscreenTransition::new(
            parent,
            controller,
            id.is_some(),
            restore,
        );
        let previous_transition = std::mem::replace(
            &mut self.state.borrow_mut().fullscreen_transition,
            transition,
        );
        drop(previous_transition);
        schedule_sync(&self.state);
    }

    /// Desired visibility, including children awaiting their first paint. A
    /// re-entrant layout borrow is uncertainty: keep the view awake until the
    /// next settled layout rather than applying a background resource policy.
    pub fn wants_visible(&self, id: ItemId) -> bool {
        self.state
            .try_borrow()
            .map_or(true, |state| state.visible.contains(&id))
    }

    /// A coalesced layout may run before a later-queued controller creation.
    /// The stage retains that desired tree so creation can attach and schedule
    /// the missing child at the already-authoritative geometry.
    pub fn contains_item(&self, id: ItemId) -> bool {
        self.state
            .try_borrow()
            .is_ok_and(|state| state.tree.as_ref().is_some_and(|tree| tree.contains(id)))
    }

    pub fn insert_view(
        &self,
        id: ItemId,
        view: &wry::WebView,
        generation: Arc<AtomicBool>,
        presentation_permit: Arc<AtomicBool>,
    ) -> bool {
        let controller = view.controller();
        let mut container = HWND::default();
        if unsafe { controller.ParentWindow(&mut container) }.is_err() {
            return false;
        }
        let _ = unsafe { ShowWindow(container, SW_HIDE) };
        let controller_hidden = unsafe { controller.SetIsVisible(false) }.is_ok();
        let mut state = self.state.borrow_mut();
        state.ready.remove(&id);
        state.visibility_uncertain.remove(&id);
        let serial = state.next_serial;
        state.next_serial = state.next_serial.wrapping_add(1).max(1);
        state.views.insert(
            id,
            HostView {
                container,
                controller,
                serial,
                applied: Cell::new(AppliedPlacement {
                    window_visible: Some(false),
                    controller_visible: controller_hidden.then_some(false),
                    ..AppliedPlacement::default()
                }),
                consecutive_failures: Cell::new(u8::from(!controller_hidden)),
                terminal_failure_reported: Cell::new(false),
                generation,
                presentation_permit,
            },
        );
        state.dirty.insert(id);
        drop(state);
        // A new page window is made over its siblings, the card included.
        settle_card(&self.state);
        schedule_sync(&self.state);
        true
    }

    pub fn remove_view(&self, id: ItemId) {
        // wry owns the container window; dropping the webview destroys it.
        let mut state = self.state.borrow_mut();
        if state.preview.is_some_and(|moving| moving.id == id) {
            state.preview = None;
        }
        let container = state.views.remove(&id).map(|view| view.container);
        state.ready.remove(&id);
        state.dirty.remove(&id);
        state.visibility_uncertain.remove(&id);
        state.visible.remove(&id);
        let cover = state.covers.remove(&id);
        drop(state);
        if let Some(container) = container {
            super::sidebar_card::forget(container);
        }
        if let Some((cover, _)) = cover {
            let _ = unsafe { DestroyWindow(cover) };
        }
    }

    /// Brings the chrome over the pages inside the card of a hidden sidebar,
    /// or puts it back beneath them. With `slide`, the cut follows the card
    /// in, or out before the chrome goes back beneath; a card turned back
    /// part way goes from where it has got to.
    pub fn raise_chrome(&self, raised: bool, card: Rect, slide: bool) {
        let Ok(mut state) = self.state.try_borrow_mut() else {
            return;
        };
        let now = Instant::now();
        let away = match (state.card, state.card_motion) {
            (None, _) => 1.0,
            (Some(_), Some(motion)) => motion.at(now).0,
            (Some(_), None) => 0.0,
        };
        let to = if raised { 0.0 } else { 1.0 };
        if slide && away != to {
            if state.card_motion.is_none_or(|motion| motion.to != to) {
                state.card_motion = Some(CardMotion {
                    started: now,
                    from: away,
                    to,
                });
            }
            state.card = Some(card);
        } else {
            state.card_motion = None;
            state.card = raised.then_some(card);
        }
        drop(state);
        settle_card(&self.state);
    }

    /// Stacks an opaque child in the page ground over the WebView2 inside its
    /// container. Installed while the container is still hidden, so the reveal
    /// shows the ground instead of WebView2's white default; the container's
    /// region clips it, it never takes focus, and it absorbs pointer input
    /// until the page has painted. It grants no presentation authority.
    pub fn cover(&self, id: ItemId, token: u64) -> bool {
        // Only a page the window is about to show needs hiding until it
        // paints; a hidden tab's navigation would poll for nothing.
        let Some((container, parent)) = self.state.try_borrow().ok().and_then(|state| {
            (!state.hidden && state.visible.contains(&id))
                .then(|| Some((state.views.get(&id)?.container, state.parent)))
                .flatten()
        }) else {
            return false;
        };
        let module = unsafe { GetModuleHandleW(None) }.unwrap_or_default();
        let Ok(cover) = (unsafe {
            CreateWindowExW(
                WS_EX_NOACTIVATE,
                cover_class(),
                PCWSTR::null(),
                WS_CHILD | WS_CLIPSIBLINGS,
                0,
                0,
                COVER_EXTENT,
                COVER_EXTENT,
                Some(container),
                None,
                Some(module.into()),
                None,
            )
        }) else {
            return false;
        };
        unsafe { SetWindowLongPtrW(cover, GWLP_USERDATA, page_ground(parent).0 as isize) };
        let _ = unsafe {
            SetWindowPos(
                cover,
                Some(HWND_TOP),
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
            )
        };
        let _ = unsafe { ShowWindow(cover, SW_SHOWNA) };
        // Window creation can pump; the view may have been replaced meanwhile.
        let previous = match self.state.try_borrow_mut() {
            Ok(mut state)
                if state
                    .views
                    .get(&id)
                    .is_some_and(|view| view.container == container) =>
            {
                Ok(state.covers.insert(id, (cover, token)))
            }
            _ => Err(()),
        };
        match previous {
            Ok(previous) => {
                if let Some((previous, _)) = previous {
                    let _ = unsafe { DestroyWindow(previous) };
                }
                true
            }
            Err(()) => {
                let _ = unsafe { DestroyWindow(cover) };
                false
            }
        }
    }

    pub fn uncover(&self, id: ItemId, token: u64) {
        let cover =
            self.state
                .try_borrow_mut()
                .ok()
                .and_then(|mut state| match state.covers.get(&id) {
                    Some(&(_, owner)) if owner == token => state.covers.remove(&id),
                    _ => None,
                });
        if let Some((cover, _)) = cover {
            let _ = unsafe { DestroyWindow(cover) };
        }
    }

    /// Reveal one exact raw-view generation after privileged chrome verified
    /// its attributed URL/revision and the shell returned the same identity.
    pub fn set_ready(&self, id: ItemId) -> bool {
        {
            let Ok(mut state) = self.state.try_borrow_mut() else {
                return false;
            };
            if !state.views.contains_key(&id) {
                return false;
            }
            let newly_ready = state.ready.insert(id);
            state.dirty.insert(id);
            if newly_ready {
                state.revision = state.revision.wrapping_add(1).max(1);
            }
        }
        // Re-drive an identical value too: a previous bounded COM placement
        // retry may have stopped without exposing the controller.
        schedule_sync(&self.state);
        true
    }

    /// Re-arm the presentation barrier for a newly committed main-frame
    /// document. Hide in this call rather than waiting for the frame timer:
    /// ContentLoading is the last browser-process boundary before the new
    /// document can produce pixels, and an old readiness bit must not expose
    /// them under stale chrome.
    pub fn set_pending(&self, id: ItemId) -> bool {
        let attached = match self.state.try_borrow_mut() {
            Ok(mut state) => {
                if !state.views.contains_key(&id) {
                    return true;
                }
                if let Some(view) = state.views.get(&id) {
                    view.presentation_permit.store(false, Ordering::Release);
                }
                if state.ready.remove(&id) {
                    state.revision = state.revision.wrapping_add(1).max(1);
                }
                state.dirty.insert(id);
                true
            }
            Err(_) => false,
        };
        if !attached {
            return false;
        }
        hide_views(&self.state, &[id]);
        schedule_sync(&self.state);
        true
    }

    /// Shows what dropping a dragged tab would do, or takes it away. The
    /// pane's page slides aside without being resized; the drop is the one
    /// layout. A new target shows only once the pointer has rested on it.
    pub fn set_drop_indicator(&self, preview: Option<DropPreview>) {
        let Some(preview) = preview else {
            self.disarm();
            self.show_preview(None);
            return;
        };
        let Ok(mut s) = self.state.try_borrow_mut() else {
            return;
        };
        let shown = s
            .preview
            .is_some_and(|m| m.id == preview.tab && !m.release && m.to == preview.shift);
        let waiting = s
            .armed
            .is_some_and(|armed| armed.tab == preview.tab && armed.zone == preview.zone);
        if shown || waiting {
            return;
        }
        s.armed = Some(preview);
        if let Some(old) = s.dwell_timer.take() {
            let _ = unsafe { KillTimer(None, old) };
            DWELL_TIMERS.with(|timers| timers.borrow_mut().remove(&old));
        }
        let timer = unsafe { SetTimer(None, 0, DROP_DWELL_MS, Some(dwell_timer_proc)) };
        if timer != 0 {
            s.dwell_timer = Some(timer);
            DWELL_TIMERS.with(|timers| {
                timers
                    .borrow_mut()
                    .insert(timer, Rc::downgrade(&self.state));
            });
        }
        let release = s.preview.is_some_and(|m| !m.release);
        drop(s);
        if timer == 0 {
            self.disarm();
            self.show_preview(Some(preview));
        } else if release {
            self.show_preview(None);
        }
    }

    fn disarm(&self) {
        let Ok(mut s) = self.state.try_borrow_mut() else {
            return;
        };
        s.armed = None;
        if let Some(timer) = s.dwell_timer.take() {
            let _ = unsafe { KillTimer(None, timer) };
            DWELL_TIMERS.with(|timers| timers.borrow_mut().remove(&timer));
        }
    }

    fn show_preview(&self, preview: Option<DropPreview>) {
        self.set_indicator(preview.map(|preview| preview.zone), false);
        self.move_page(preview);
    }

    fn move_page(&self, preview: Option<DropPreview>) {
        let Ok(mut s) = self.state.try_borrow_mut() else {
            return;
        };
        let now = Instant::now();
        let current = s.preview;
        let next = match (preview, current) {
            (Some(p), Some(m)) if m.id == p.tab && !m.release && m.to == p.shift => return,
            (Some(p), current) => {
                let from = current
                    .filter(|m| m.id == p.tab)
                    .map_or((0.0, 0.0), |m| m.at(now));
                Some(PreviewMove {
                    id: p.tab,
                    started: now,
                    from,
                    to: p.shift,
                    release: false,
                })
            }
            (None, Some(m)) if !m.release => Some(PreviewMove {
                id: m.id,
                started: now,
                from: m.at(now),
                to: (0.0, 0.0),
                release: true,
            }),
            (None, _) => return,
        };
        // Another page was displaced: it goes home at once.
        if let Some(old) = current.filter(|m| next.is_none_or(|n| n.id != m.id)) {
            s.dirty.insert(old.id);
            if let Some(view) = s.views.get(&old.id) {
                let mut applied = view.applied.get();
                applied.rounded = None;
                view.applied.set(applied);
            }
        }
        if let Some(next) = next {
            s.dirty.insert(next.id);
        }
        s.preview = next;
        drop(s);
        schedule_sync(&self.state);
    }

    pub fn set_resize_guide(&self, zone: Option<Rect>) {
        self.set_indicator(zone, true);
    }

    fn set_indicator(&self, zone: Option<Rect>, resize: bool) {
        let Ok(mut s) = self.state.try_borrow_mut() else {
            return;
        };
        match zone {
            None => {
                if let Some(hwnd) = s.indicator.take() {
                    let _ = unsafe { DestroyWindow(hwnd) };
                }
                s.indicator_size = (0, 0);
            }
            Some(zone) => {
                let scale = scale_of(s.parent);
                let mut top_left = POINT {
                    x: ((if resize { zone.x } else { s.origin.0 + zone.x }) * scale).round() as i32,
                    y: ((if resize { zone.y } else { s.origin.1 + zone.y }) * scale).round() as i32,
                };
                let _ = unsafe { ClientToScreen(s.parent, &mut top_left) };
                let w = ((zone.width * scale).round() as i32).max(1);
                let h = ((zone.height * scale).round() as i32).max(1);
                let hwnd = match s.indicator {
                    Some(hwnd) => hwnd,
                    None => {
                        let Some(created) = create_indicator(s.parent) else {
                            return;
                        };
                        s.indicator = Some(created);
                        created
                    }
                };
                let resized = s.indicator_size != (w, h) || s.indicator_resize != resize;
                s.indicator_size = (w, h);
                s.indicator_resize = resize;
                let parent = s.parent;
                drop(s);
                unsafe {
                    if resized {
                        let guide = if dark_theme(parent) {
                            GUIDE_ON_DARK
                        } else {
                            GUIDE_ON_LIGHT
                        };
                        draw_indicator(hwnd, top_left.x, top_left.y, w, h, scale, resize, guide);
                    }
                    let _ = SetWindowPos(
                        hwnd,
                        Some(HWND_TOP),
                        top_left.x,
                        top_left.y,
                        w,
                        h,
                        SWP_NOACTIVATE,
                    );
                    let _ = ShowWindow(hwnd, SW_SHOWNA);
                }
            }
        }
    }
}

fn scale_of(hwnd: HWND) -> f64 {
    (unsafe { GetDpiForWindow(hwnd) } as f64 / 96.0).max(0.5)
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn indicator_class() -> PCWSTR {
    static NAME: OnceLock<Vec<u16>> = OnceLock::new();
    let name = NAME.get_or_init(|| {
        let name = wide("ZephiumIndicator");
        let class = WNDCLASSW {
            lpfnWndProc: Some(plain_proc),
            lpszClassName: PCWSTR(name.as_ptr()),
            hInstance: unsafe { GetModuleHandleW(None) }.unwrap_or_default().into(),
            ..Default::default()
        };
        unsafe { RegisterClassW(&class) };
        name
    });
    PCWSTR(name.as_ptr())
}

// Owned popup, not a child: layered child windows have broken input and
// spotty ULW support; a top-level layered window is the reliable primitive.
fn create_indicator(owner: HWND) -> Option<HWND> {
    let module = unsafe { GetModuleHandleW(None) }.unwrap_or_default();
    unsafe {
        CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW,
            indicator_class(),
            PCWSTR::null(),
            WS_POPUP,
            0,
            0,
            0,
            0,
            Some(owner),
            None,
            Some(module.into()),
            None,
        )
    }
    .map_err(|e| eprintln!("stage: indicator creation failed: {e}"))
    .ok()
}

unsafe extern "system" fn plain_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

/// Larger than any container; the container's client area and region clip it.
const COVER_EXTENT: i32 = 0x4000;

fn cover_class() -> PCWSTR {
    static NAME: OnceLock<Vec<u16>> = OnceLock::new();
    let name = NAME.get_or_init(|| {
        let name = wide("ZephiumPaintCover");
        let class = WNDCLASSW {
            lpfnWndProc: Some(cover_proc),
            lpszClassName: PCWSTR(name.as_ptr()),
            hInstance: unsafe { GetModuleHandleW(None) }.unwrap_or_default().into(),
            hCursor: unsafe { LoadCursorW(None, IDC_ARROW) }.unwrap_or_default(),
            ..Default::default()
        };
        unsafe { RegisterClassW(&class) };
        name
    });
    PCWSTR(name.as_ptr())
}

unsafe extern "system" fn cover_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_ERASEBKGND => {
            let color = COLORREF(unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as u32);
            let brush = unsafe { CreateSolidBrush(color) };
            let mut rect = RECT::default();
            if unsafe { GetClientRect(hwnd, &mut rect) }.is_ok() {
                unsafe { FillRect(HDC(wparam.0 as _), &rect, brush) };
            }
            let _ = unsafe { DeleteObject(brush.into()) };
            LRESULT(1)
        }
        WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

/// The frame's content ground for the app theme, read from the root window's
/// dark-mode attribute, which the app sets for forced and system themes alike.
/// The window follows the app theme through DWM; dark when unknown.
fn dark_theme(parent: HWND) -> bool {
    let root = unsafe { GetAncestor(parent, GA_ROOT) };
    let mut dark = BOOL(1);
    let read = unsafe {
        DwmGetWindowAttribute(
            root,
            DWMWA_USE_IMMERSIVE_DARK_MODE,
            (&mut dark as *mut BOOL).cast(),
            std::mem::size_of::<BOOL>() as u32,
        )
    };
    read.is_err() || dark.as_bool()
}

fn page_ground(parent: HWND) -> COLORREF {
    let (red, green, blue) = if dark_theme(parent) {
        crate::platform::PAGE_GROUND_DARK
    } else {
        crate::platform::PAGE_GROUND_LIGHT
    };
    COLORREF(u32::from(red) | (u32::from(green) << 8) | (u32::from(blue) << 16))
}

/// WebView2 may restack its own child when it becomes visible; keep the cover
/// above it.
fn raise_cover(state: &Rc<RefCell<State>>, id: ItemId) {
    let cover = state
        .try_borrow()
        .ok()
        .and_then(|state| state.covers.get(&id).map(|&(cover, _)| cover));
    if let Some(cover) = cover {
        let _ = unsafe {
            SetWindowPos(
                cover,
                Some(HWND_TOP),
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
            )
        };
    }
}

/// The pointer has rested on a drop target: the page makes room.
unsafe extern "system" fn dwell_timer_proc(_: HWND, _: u32, timer: usize, _: u32) {
    let _ = unsafe { KillTimer(None, timer) };
    let Some(state) = DWELL_TIMERS
        .with(|timers| timers.borrow_mut().remove(&timer))
        .and_then(|weak| weak.upgrade())
    else {
        return;
    };
    let armed = match state.try_borrow_mut() {
        Ok(mut s) if s.dwell_timer == Some(timer) => {
            s.dwell_timer = None;
            s.armed.take()
        }
        _ => None,
    };
    if let Some(preview) = armed {
        Stage { state }.show_preview(Some(preview));
    }
}

unsafe extern "system" fn layout_timer_proc(_: HWND, _: u32, timer: usize, _: u32) {
    let weak = LAYOUT_TIMERS.with(|timers| timers.borrow().get(&timer).cloned());
    let Some(weak) = weak else {
        let _ = unsafe { KillTimer(None, timer) };
        return;
    };
    let Some(state) = weak.upgrade() else {
        let _ = unsafe { KillTimer(None, timer) };
        LAYOUT_TIMERS.with(|timers| {
            timers.borrow_mut().remove(&timer);
        });
        return;
    };
    {
        let Ok(mut state) = state.try_borrow_mut() else {
            // A window-less SetTimer repeats. If native reentrancy happens to
            // hold this borrow, leave it armed and retry on a later turn.
            return;
        };
        if state.layout_timer != Some(timer) {
            let _ = unsafe { KillTimer(None, timer) };
            LAYOUT_TIMERS.with(|timers| {
                timers.borrow_mut().remove(&timer);
            });
            return;
        }
        let _ = unsafe { KillTimer(None, timer) };
        LAYOUT_TIMERS.with(|timers| {
            timers.borrow_mut().remove(&timer);
        });
        state.layout_timer = None;
    }
    sync(&state);
}

fn schedule_sync(state: &Rc<RefCell<State>>) {
    {
        let state = state.borrow();
        if state.layout_timer.is_some() || state.dirty.is_empty() {
            return;
        }
    }
    // A window-less timer is delivered on this UI thread after the current
    // message-loop turn. Its minimum cadence is close to one display frame,
    // so a responsive resize/divider burst collapses into one native pass.
    let timer = unsafe { SetTimer(None, 0, USER_TIMER_MINIMUM, Some(layout_timer_proc)) };
    if timer == 0 {
        // Queue exhaustion must not strand a pane indefinitely.
        sync(state);
        return;
    }
    state.borrow_mut().layout_timer = Some(timer);
    LAYOUT_TIMERS.with(|timers| {
        timers.borrow_mut().insert(timer, Rc::downgrade(state));
    });
}

struct NativePlacement {
    id: ItemId,
    serial: u64,
    revision: u64,
    container: HWND,
    controller: ICoreWebView2Controller,
    rect: Option<PhysicalRect>,
    show: bool,
    radius: i32,
    fullscreen: bool,
    screen_origin: Option<(i32, i32)>,
    applied: AppliedPlacement,
    delta: PlacementDelta,
    presentation_permit: Arc<AtomicBool>,
}

fn hide_views(state: &Rc<RefCell<State>>, ids: &[ItemId]) {
    struct Hide {
        id: ItemId,
        serial: u64,
        revision: u64,
        container: HWND,
        controller: ICoreWebView2Controller,
        applied: AppliedPlacement,
    }

    let hides = {
        let state = state.borrow();
        ids.iter()
            .filter_map(|id| {
                let view = state.views.get(id)?;
                let applied = view.applied.get();
                ((applied.window_visible != Some(false))
                    || (applied.controller_visible != Some(false)))
                .then(|| Hide {
                    id: *id,
                    serial: view.serial,
                    revision: state.revision,
                    container: view.container,
                    controller: view.controller.clone(),
                    applied,
                })
            })
            .collect::<Vec<_>>()
    };

    for hide in hides {
        let mut applied = hide.applied;
        if applied.window_visible != Some(false) {
            let _ = unsafe { ShowWindow(hide.container, SW_HIDE) };
            applied.window_visible = Some(false);
            if !hide_is_current(state, hide.id, hide.serial, hide.revision) {
                redrive_uncertain_visibility(state, hide.id, hide.serial);
                continue;
            }
        }
        let controller_failed = if applied.controller_visible != Some(false) {
            let hidden = unsafe { hide.controller.SetIsVisible(false) }.is_ok();
            if !hide_is_current(state, hide.id, hide.serial, hide.revision) {
                redrive_uncertain_visibility(state, hide.id, hide.serial);
                continue;
            }
            if hidden {
                applied.controller_visible = Some(false);
                false
            } else {
                true
            }
        } else {
            false
        };
        let mut current = state.borrow_mut();
        if current.revision != hide.revision {
            drop(current);
            redrive_uncertain_visibility(state, hide.id, hide.serial);
            continue;
        }
        let Some(view) = current
            .views
            .get(&hide.id)
            .filter(|view| view.serial == hide.serial)
        else {
            drop(current);
            redrive_uncertain_visibility(state, hide.id, hide.serial);
            continue;
        };
        view.applied.set(applied);
        if controller_failed {
            current.dirty.insert(hide.id);
        }
    }
}

fn hide_is_current(state: &Rc<RefCell<State>>, id: ItemId, serial: u64, revision: u64) -> bool {
    state.try_borrow().is_ok_and(|state| {
        state.revision == revision
            && state
                .views
                .get(&id)
                .is_some_and(|view| view.serial == serial)
    })
}

fn redrive_uncertain_visibility(state: &Rc<RefCell<State>>, id: ItemId, serial: u64) {
    let marked = state.try_borrow_mut().is_ok_and(|mut state| {
        if state
            .views
            .get(&id)
            .is_none_or(|view| view.serial != serial)
        {
            return false;
        }
        state.visibility_uncertain.insert(id);
        state.dirty.insert(id);
        true
    });
    if marked {
        schedule_sync(state);
    }
}

fn placement_is_current(state: &Rc<RefCell<State>>, placement: &NativePlacement) -> bool {
    state.try_borrow().is_ok_and(|state| {
        state.revision == placement.revision
            && state
                .views
                .get(&placement.id)
                .is_some_and(|view| view.serial == placement.serial)
    })
}

fn placement_may_reveal(state: &Rc<RefCell<State>>, placement: &NativePlacement) -> bool {
    placement.show
        && placement.presentation_permit.load(Ordering::Acquire)
        && state.try_borrow().is_ok_and(|state| {
            state.revision == placement.revision
                && !state.hidden
                && state.visible.contains(&placement.id)
                && state.ready.contains(&placement.id)
                && state
                    .tree
                    .as_ref()
                    .is_some_and(|tree| tree.contains(placement.id))
                && state.views.get(&placement.id).is_some_and(|view| {
                    view.serial == placement.serial
                        && Arc::ptr_eq(&view.presentation_permit, &placement.presentation_permit)
                })
        })
        && placement.presentation_permit.load(Ordering::Acquire)
}

fn sync(state: &Rc<RefCell<State>>) {
    let (
        parent,
        gap,
        origin,
        size,
        offset,
        hidden,
        tree,
        ready,
        visible,
        dirty,
        revision,
        fullscreen,
        preview,
        caption_offset,
        caption_moving,
        cut,
    ) = {
        let Ok(mut state) = state.try_borrow_mut() else {
            return;
        };
        // A page displaced for a drop preview keeps moving until it rests,
        // and is let go once it is home.
        let now = Instant::now();
        let preview = state.preview.and_then(|moving| {
            if moving.release && moving.arrived(now) {
                state.preview = None;
                state.dirty.insert(moving.id);
                if let Some(view) = state.views.get(&moving.id) {
                    let mut applied = view.applied.get();
                    applied.rounded = None;
                    view.applied.set(applied);
                }
                None
            } else {
                if !moving.arrived(now) {
                    state.dirty.insert(moving.id);
                }
                Some((moving.id, moving.at(now), !moving.arrived(now)))
            }
        });
        // Mid-journey, the panes are laid out in the journey's geometry and
        // shifted by how far it has got; on arrival, the real geometry.
        let travelling = state
            .slide
            .and_then(|slide| slide.offset(Instant::now()).map(|offset| (slide, offset)));
        if travelling.is_none() && state.slide.take().is_some() {
            let visible = state.visible.clone();
            // Home, a page cut on its way takes its own whole shape again.
            for id in &visible {
                if let Some(view) = state.views.get(id) {
                    let mut applied = view.applied.get();
                    applied.rounded = None;
                    view.applied.set(applied);
                }
            }
            state.dirty.extend(visible);
        }
        // Travelling right from under a hidden sidebar's card, the stage gives
        // way to the column the card becomes: its pages are cut at its new
        // edge, which stays put while they slide out past it, so no page
        // shows where the column now stands.
        let cut = travelling
            .filter(|(slide, _)| slide.beside && slide.to > slide.from)
            .map(|(slide, _)| {
                (
                    slide.origin.0 + slide.to,
                    slide.origin.1,
                    slide.size.0 - slide.to,
                    slide.size.1,
                )
            });
        let dirty = std::mem::take(&mut state.dirty);
        if dirty.is_empty() {
            return;
        }
        let (origin, size, offset) = travelling
            .map_or((state.origin, state.size, 0.0), |(slide, offset)| {
                (slide.origin, slide.size, offset)
            });
        (
            state.parent,
            state.gap,
            origin,
            size,
            offset,
            state.hidden,
            state.tree.clone(),
            state.ready.clone(),
            state.visible.clone(),
            dirty,
            state.revision,
            state.fullscreen,
            preview,
            if state.fullscreen.is_some() {
                0.0
            } else {
                state.caption_offset
            },
            state.caption_moving && state.fullscreen.is_none(),
            cut,
        )
    };
    let sliding = caption_moving || offset != 0.0 || preview.is_some_and(|(_, _, moving)| moving);

    let scale = scale_of(parent);
    let local = Rect::new(0.0, 0.0, size.0, size.1);
    let radius = ((RADIUS * scale * 2.0).round() as i32).max(1);
    let pane_rects = tree
        .as_ref()
        .map(|tree| split::layout(tree, local, gap))
        .unwrap_or_default()
        .into_iter()
        .filter_map(|(id, rect)| {
            let (width, height) = rounded_native_size(rect.width, rect.height, scale)?;
            let shift = preview
                .filter(|(shifted, _, _)| *shifted == id)
                .map_or((0.0, 0.0), |(_, shift, _)| shift);
            let x = (origin.0 + rect.x + offset + shift.0) * scale;
            let y = (origin.1 + rect.y + shift.1 + caption_offset) * scale;
            (x.is_finite() && y.is_finite())
                .then_some((id, (x.round() as i32, y.round() as i32, width, height)))
        })
        .collect::<HashMap<_, _>>();
    let mut parent_screen = POINT { x: 0, y: 0 };
    let parent_screen_origin = unsafe { ClientToScreen(parent, &mut parent_screen) }
        .as_bool()
        .then_some((parent_screen.x, parent_screen.y));

    let placements = {
        let state = state.borrow();
        dirty
            .into_iter()
            .filter_map(|id| {
                let view = state.views.get(&id)?;
                let rect = pane_rects.get(&id).copied();
                let show = !hidden
                    && ready.contains(&id)
                    && visible.contains(&id)
                    && rect.is_some()
                    && view.presentation_permit.load(Ordering::Acquire);
                let applied = view.applied.get();
                let fullscreen = fullscreen == Some(id);
                let radius = if fullscreen { 0 } else { radius };
                let mut delta = placement_delta(
                    applied,
                    show,
                    rect,
                    radius,
                    fullscreen,
                    parent_screen_origin,
                );
                // WebView2 is told where it sits once the journey is over,
                // not on every step of it.
                if sliding {
                    delta.notify_parent_position = false;
                }
                if state.visibility_uncertain.contains(&id) {
                    delta.window_visibility = true;
                    delta.controller_visibility = true;
                }
                (!delta.is_empty()).then(|| NativePlacement {
                    id,
                    serial: view.serial,
                    revision,
                    container: view.container,
                    controller: view.controller.clone(),
                    rect,
                    show,
                    radius,
                    fullscreen,
                    screen_origin: parent_screen_origin,
                    applied,
                    delta,
                    presentation_permit: view.presentation_permit.clone(),
                })
            })
            .collect::<Vec<_>>()
    };

    // Hide first so a replacement pane can never reveal the old tab's last
    // frame. `apply` normally did this synchronously; this also retries any
    // failed controller-visibility transition.
    for placement in placements.iter().filter(|placement| !placement.show) {
        apply_native_placement(state, placement);
    }
    for placement in placements.iter().filter(|placement| placement.show) {
        apply_native_placement(state, placement);
    }
    // A displaced page is clipped to the stage, so what slides out of it
    // never covers the chrome around.
    if state.borrow().revision != revision {
        return;
    }
    if preview.is_some() || caption_offset > 0.0 || cut.is_some() {
        let (x, y, width, height) = cut.unwrap_or((origin.0, origin.1, size.0, size.1));
        let stage = (
            (x * scale).round() as i32,
            (y * scale).round() as i32,
            (width * scale).round() as i32,
            (height * scale).round() as i32,
        );
        for (id, rect) in &pane_rects {
            if caption_offset > 0.0
                || cut.is_some()
                || preview.is_some_and(|(shifted, _, _)| shifted == *id)
            {
                clip_to_stage(state, *id, *rect, stage, radius);
            }
        }
    }
    settle_card(state);
    let transition = state.borrow().fullscreen_transition.clone();
    if let Some(transition) = transition {
        transition.raise();
    }
    // The next step of the journey is the next pass: every visible pane is
    // due to move again, on the stage's own frame-paced timer.
    let travelling = state.try_borrow().is_ok_and(|state| {
        state.slide.is_some()
            || state
                .preview
                .is_some_and(|moving| !moving.arrived(Instant::now()))
    });
    if travelling {
        if let Ok(mut current) = state.try_borrow_mut() {
            let visible = current.visible.clone();
            current.dirty.extend(visible);
        }
        schedule_sync(state);
    }
}

/// Keeps the chrome over the pages, cut to the hidden sidebar's card, with
/// the card's shadow on the pages, for as long as the card is out; and puts
/// it back as it was when it is not. A fullscreen page stays above all.
fn settle_card(state: &Rc<RefCell<State>>) {
    let (parent, card, chrome, clip, pages, away, moving) = {
        let Ok(mut state) = state.try_borrow_mut() else {
            return;
        };
        let (away, moving) = match state.card_motion {
            Some(motion) => {
                let (away, arrived) = motion.at(Instant::now());
                if arrived {
                    state.card_motion = None;
                    if motion.to >= 1.0 {
                        state.card = None;
                    }
                }
                (away, !arrived)
            }
            None => (0.0, false),
        };
        let pages: Vec<(HWND, Option<PhysicalRect>)> = state
            .views
            .values()
            .map(|view| {
                let applied = view.applied.get();
                let shown = applied.window_visible == Some(true);
                (view.container, applied.container_rect.filter(|_| shown))
            })
            .collect();
        (
            state.parent,
            state.card.filter(|_| state.fullscreen.is_none()),
            state.chrome,
            state.card_clip.clone(),
            pages,
            away,
            moving,
        )
    };
    if moving {
        step_card(state);
    }
    let Some(card) = card else {
        if clip.is_some() {
            let lowered = chrome.is_none_or(|chrome| {
                super::sidebar_card::is_live(chrome)
                    && super::sidebar_card::stack_chrome(chrome, false)
            });
            if lowered {
                if let Ok(mut state) = state.try_borrow_mut() {
                    state.card_clip = None;
                }
            }
        }
        super::sidebar_card::lift();
        return;
    };
    let scale = scale_of(parent);
    let mut client = RECT::default();
    if unsafe { GetClientRect(parent, &mut client) }.is_err() {
        return;
    }
    let Some(placed) =
        crate::card_geometry::card_pixels(card, (client.right, client.bottom), scale)
    else {
        return;
    };
    let travel = ((card.width + CARD_AWAY) * scale * away).round() as i32;
    let rect = (placed.0 - travel, placed.1, placed.2, placed.3);
    let chrome = match chrome.filter(|chrome| super::sidebar_card::is_live(*chrome)) {
        Some(chrome) => chrome,
        None => {
            let containers: Vec<HWND> = pages.iter().map(|(container, _)| *container).collect();
            let Some(found) = super::sidebar_card::find_chrome(parent, &containers) else {
                return;
            };
            if let Ok(mut state) = state.try_borrow_mut() {
                state.chrome = Some(found);
            }
            found
        }
    };
    let diameter = ((RADIUS * scale * 2.0).round() as i32).max(1);
    let shown: Vec<PhysicalRect> = pages.iter().filter_map(|(_, rect)| *rect).collect();
    let key = (rect, diameter, shown);
    if clip.as_ref() != Some(&key)
        && super::sidebar_card::clip_chrome(
            parent,
            chrome,
            (client.right, client.bottom),
            rect,
            diameter,
        )
    {
        if let Ok(mut state) = state.try_borrow_mut() {
            state.card_clip = Some(key);
        }
    }
    if !super::sidebar_card::is_on_top(parent, chrome) {
        super::sidebar_card::stack_chrome(chrome, true);
    }
    let placements: Vec<super::sidebar_card::Placement> = pages
        .into_iter()
        .filter_map(|(container, rect)| {
            Some(super::sidebar_card::Placement {
                container,
                rect: rect?,
            })
        })
        .collect();
    super::sidebar_card::cast(&placements, rect, scale, dark_theme(parent));
}

/// Moves the card's cut on by a display frame's worth, on the stage's own
/// frame-paced timer, until the card arrives.
fn step_card(state: &Rc<RefCell<State>>) {
    if state
        .try_borrow()
        .map_or(true, |state| state.card_tick.is_some())
    {
        return;
    }
    let weak = Rc::downgrade(state);
    let tick = super::schedule_browser_timeout(Duration::from_millis(1), move || {
        let Some(state) = weak.upgrade() else {
            return;
        };
        if let Ok(mut current) = state.try_borrow_mut() {
            current.card_tick = None;
        }
        settle_card(&state);
    });
    if tick.is_none() {
        // With no timer to step it, the card's cut goes straight to where the
        // card is going rather than staying part way.
        if let Ok(mut current) = state.try_borrow_mut() {
            if current
                .card_motion
                .take()
                .is_some_and(|motion| motion.to >= 1.0)
            {
                current.card = None;
            }
        }
        settle_card(state);
        return;
    }
    if let Ok(mut current) = state.try_borrow_mut() {
        current.card_tick = tick;
    }
}

/// Narrows a page's rounded region to the part of it inside the stage, which
/// is rounded too, so a page cut by the stage's edge keeps its corners there.
fn clip_to_stage(
    state: &Rc<RefCell<State>>,
    id: ItemId,
    rect: PhysicalRect,
    stage: PhysicalRect,
    radius: i32,
) {
    let Ok(state) = state.try_borrow() else {
        return;
    };
    let Some(view) = state.views.get(&id) else {
        return;
    };
    let (x, y, width, height) = rect;
    let left = (stage.0 - x).max(0);
    let top = (stage.1 - y).max(0);
    let right = (stage.0 + stage.2 - x).min(width);
    let bottom = (stage.1 + stage.3 - y).min(height);
    if right <= left || bottom <= top {
        return;
    }
    // SAFETY: GDI regions created and consumed on this thread; the system owns
    // the final region only when SetWindowRgn succeeds.
    unsafe {
        let rounded = CreateRoundRectRgn(
            0,
            0,
            width.saturating_add(1),
            height.saturating_add(1),
            radius,
            radius,
        );
        let inside = CreateRoundRectRgn(
            left,
            top,
            right.saturating_add(1),
            bottom.saturating_add(1),
            radius,
            radius,
        );
        if !rounded.0.is_null() && !inside.0.is_null() {
            let _ = CombineRgn(Some(rounded), Some(rounded), Some(inside), RGN_AND);
            if SetWindowRgn(view.container, Some(rounded), true) == 0 {
                let _ = DeleteObject(rounded.into());
            }
        } else if !rounded.0.is_null() {
            let _ = DeleteObject(rounded.into());
        }
        if !inside.0.is_null() {
            let _ = DeleteObject(inside.into());
        }
    }
}

fn apply_native_placement(state: &Rc<RefCell<State>>, placement: &NativePlacement) {
    if !placement_is_current(state, placement) {
        return;
    }

    let mut applied = placement.applied;
    let mut failed = false;
    let presentation_revoked = !placement.presentation_permit.load(Ordering::Acquire);
    if !placement.show || presentation_revoked {
        if presentation_revoked || placement.delta.window_visibility {
            let _ = unsafe { ShowWindow(placement.container, SW_HIDE) };
            applied.window_visible = Some(false);
            if !placement_is_current(state, placement) {
                finish_native_placement(state, placement, applied, false);
                return;
            }
        }
        if presentation_revoked || placement.delta.controller_visibility {
            if unsafe { placement.controller.SetIsVisible(false) }.is_ok() {
                applied.controller_visible = Some(false);
            } else {
                failed = true;
            }
            if !placement_is_current(state, placement) {
                finish_native_placement(state, placement, applied, false);
                return;
            }
        }
        finish_native_placement(state, placement, applied, failed);
        return;
    }

    let Some((x, y, width, height)) = placement.rect else {
        return;
    };
    if placement.delta.container_rect {
        if unsafe {
            SetWindowPos(
                placement.container,
                None,
                x,
                y,
                width,
                height,
                SWP_NOACTIVATE | SWP_NOZORDER,
            )
        }
        .is_ok()
        {
            applied.container_rect = placement.rect;
        } else {
            failed = true;
        }
        if !placement_is_current(state, placement) {
            conceal_superseded_placement(state, placement, applied);
            return;
        }
    }
    if placement.delta.rounded && placement.radius == 0 {
        // A fullscreen page is square: drop the region rather than round by zero.
        if unsafe { SetWindowRgn(placement.container, None, true) } == 0 {
            failed = true;
        } else {
            applied.rounded = Some((width, height, 0));
        }
        if !placement_is_current(state, placement) {
            conceal_superseded_placement(state, placement, applied);
            return;
        }
    } else if placement.delta.rounded {
        let region = unsafe {
            CreateRoundRectRgn(
                0,
                0,
                width.saturating_add(1),
                height.saturating_add(1),
                placement.radius,
                placement.radius,
            )
        };
        if region.0.is_null() {
            failed = true;
        } else if unsafe { SetWindowRgn(placement.container, Some(region), true) } == 0 {
            // The system owns the region only when SetWindowRgn succeeds.
            let _ = unsafe { DeleteObject(region.into()) };
            failed = true;
        } else {
            applied.rounded = Some((width, height, placement.radius));
        }
        if !placement_is_current(state, placement) {
            conceal_superseded_placement(state, placement, applied);
            return;
        }
    }
    if placement.delta.raise {
        if !placement.fullscreen {
            applied.raised = Some(false);
        } else if unsafe {
            SetWindowPos(
                placement.container,
                Some(HWND_TOP),
                0,
                0,
                0,
                0,
                SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE,
            )
        }
        .is_ok()
        {
            applied.raised = Some(true);
        } else {
            failed = true;
        }
        if !placement_is_current(state, placement) {
            conceal_superseded_placement(state, placement, applied);
            return;
        }
    }
    if placement.delta.controller_size {
        if unsafe {
            placement.controller.SetBounds(RECT {
                left: 0,
                top: 0,
                right: width,
                bottom: height,
            })
        }
        .is_ok()
        {
            applied.controller_size = Some((width, height));
        } else {
            failed = true;
        }
        if !placement_is_current(state, placement) {
            conceal_superseded_placement(state, placement, applied);
            return;
        }
    }
    if placement.delta.notify_parent_position {
        if unsafe { placement.controller.NotifyParentWindowPositionChanged() }.is_ok() {
            applied.notified_screen_origin = placement.screen_origin.map(|(parent_x, parent_y)| {
                (parent_x.saturating_add(x), parent_y.saturating_add(y))
            });
        } else {
            failed = true;
        }
        if !placement_is_current(state, placement) {
            conceal_superseded_placement(state, placement, applied);
            return;
        }
    }

    // COM can re-enter the message pump. Revalidate the stage revision before
    // making content visible so a superseded placement cannot win the race.
    let geometry_ready = applied.container_rect == placement.rect
        && applied.controller_size == Some((width, height));
    if geometry_ready && placement_may_reveal(state, placement) {
        if placement.delta.controller_visibility {
            if unsafe { placement.controller.SetIsVisible(true) }.is_ok() {
                applied.controller_visible = Some(true);
            } else {
                failed = true;
            }
        }
        if !placement_may_reveal(state, placement) {
            // SetIsVisible is a COM boundary and may pump a newer tab/split
            // update whose old leaf still has a valid navigation permit.
            // Revision and logical visibility therefore dominate the permit.
            let _ = unsafe { placement.controller.SetIsVisible(false) };
            applied.controller_visible = Some(false);
            let _ = unsafe { ShowWindow(placement.container, SW_HIDE) };
            applied.window_visible = Some(false);
        } else if applied.controller_visible == Some(true) && placement.delta.window_visibility {
            let _ = unsafe { ShowWindow(placement.container, SW_SHOWNA) };
            applied.window_visible = Some(true);
            raise_cover(state, placement.id);
        }
        if !placement_may_reveal(state, placement) {
            // Both COM visibility and ShowWindow can pump a nested native
            // commit or layout. A superseding revision observed after either
            // call wins before the outer placement reaches the compositor.
            let _ = unsafe { ShowWindow(placement.container, SW_HIDE) };
            applied.window_visible = Some(false);
            if unsafe { placement.controller.SetIsVisible(false) }.is_ok() {
                applied.controller_visible = Some(false);
            }
        }
    } else {
        let _ = unsafe { ShowWindow(placement.container, SW_HIDE) };
        applied.window_visible = Some(false);
        if unsafe { placement.controller.SetIsVisible(false) }.is_ok() {
            applied.controller_visible = Some(false);
        }
        failed |= !geometry_ready;
    }
    finish_native_placement(state, placement, applied, failed);
}

fn conceal_superseded_placement(
    state: &Rc<RefCell<State>>,
    placement: &NativePlacement,
    mut applied: AppliedPlacement,
) {
    // A geometry API can pump a nested message loop which applies a newer
    // tab/split revision, then return to this stale stack frame. Conceal both
    // native surfaces before returning; finish_native_placement recognizes
    // supersession, preserves the latest dirty obligation, and does not spend
    // the bounded native-failure budget.
    let _ = unsafe { ShowWindow(placement.container, SW_HIDE) };
    applied.window_visible = Some(false);
    if unsafe { placement.controller.SetIsVisible(false) }.is_ok() {
        applied.controller_visible = Some(false);
    }
    finish_native_placement(state, placement, applied, false);
}

fn finish_native_placement(
    state: &Rc<RefCell<State>>,
    placement: &NativePlacement,
    applied: AppliedPlacement,
    failed: bool,
) {
    let mut retry = false;
    let mut terminal = None;
    {
        let mut state = state.borrow_mut();
        let superseded = state.revision != placement.revision;
        let report_generation = {
            let Some(view) = state
                .views
                .get(&placement.id)
                .filter(|view| view.serial == placement.serial)
            else {
                return;
            };
            if superseded {
                // A WebView2 COM call above may have pumped a nested message
                // loop in which a newer resize/split revision was applied.
                // Do not overwrite the newer pass's applied cache. The stale
                // native call can still have landed after that cache update,
                // so force both visibility surfaces on the next revision.
                retry = true;
                None
            } else if failed {
                view.applied.set(applied);
                let failures = view.consecutive_failures.get().saturating_add(1);
                view.consecutive_failures.set(failures);
                retry = failures <= MAX_NATIVE_RETRIES;
                (!retry && !view.terminal_failure_reported.replace(true))
                    .then(|| view.generation.clone())
            } else {
                view.applied.set(applied);
                view.consecutive_failures.set(0);
                None
            }
        };
        if superseded {
            state.visibility_uncertain.insert(placement.id);
        } else if !failed
            && applied.window_visible == Some(placement.show)
            && applied.controller_visible == Some(placement.show)
        {
            state.visibility_uncertain.remove(&placement.id);
        }
        if superseded || failed {
            state.dirty.insert(placement.id);
        }
        if let Some(generation) = report_generation {
            terminal = Some((state.on_placement_failure.clone(), generation));
            state.dirty.remove(&placement.id);
        }
    }
    if retry {
        schedule_sync(state);
    } else if let Some((callback, generation)) = terminal {
        // Invoke only after the RefCell borrow is released. The callback may
        // enter the host queue and retire this exact generation.
        callback(placement.id, generation);
    }
}

/// Premultiplied BGRA rounded rect pushed through UpdateLayeredWindow:
/// a faint wash in the label colour with a finer edge, signed-distance antialiasing.
#[allow(clippy::too_many_arguments)]
fn draw_indicator(
    hwnd: HWND,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    scale: f64,
    resize: bool,
    guide: (u8, u8, u8),
) {
    let header = BITMAPINFOHEADER {
        biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
        biWidth: w,
        biHeight: -h,
        biPlanes: 1,
        biBitCount: 32,
        biCompression: BI_RGB.0,
        ..Default::default()
    };
    let info = BITMAPINFO {
        bmiHeader: header,
        ..Default::default()
    };
    let mut bits: *mut std::ffi::c_void = std::ptr::null_mut();
    unsafe {
        let screen = GetDC(None);
        let Ok(bitmap) = CreateDIBSection(Some(screen), &info, DIB_RGB_COLORS, &mut bits, None, 0)
        else {
            ReleaseDC(None, screen);
            return;
        };
        let pixels = std::slice::from_raw_parts_mut(bits as *mut u32, (w as usize) * (h as usize));
        let radius = (INDICATOR_RADIUS * scale).min(w.min(h) as f64 / 2.0);
        let border = INDICATOR_BORDER * scale;
        let (half_w, half_h) = (w as f64 / 2.0, h as f64 / 2.0);
        for row in 0..h as usize {
            for col in 0..w as usize {
                let dx = (col as f64 + 0.5 - half_w).abs() - half_w + radius;
                let dy = (row as f64 + 0.5 - half_h).abs() - half_h + radius;
                let outside = (dx.max(0.0)).hypot(dy.max(0.0));
                let d = dx.max(dy).min(0.0) + outside - radius;
                let outer = (0.5 - d).clamp(0.0, 1.0);
                let inner = (0.5 - (d + border)).clamp(0.0, 1.0);
                let alpha = if resize {
                    0.9 * outer
                } else {
                    INDICATOR_STROKE * (outer - inner) + INDICATOR_FILL * inner
                };
                let v = (alpha * 255.0).round() as u32;
                let red = (f64::from(guide.0) * alpha).round() as u32;
                let green = (f64::from(guide.1) * alpha).round() as u32;
                let blue = (f64::from(guide.2) * alpha).round() as u32;
                pixels[row * w as usize + col] = (v << 24) | (red << 16) | (green << 8) | blue;
            }
        }
        let memory = CreateCompatibleDC(Some(screen));
        let previous = SelectObject(memory, bitmap.into());
        let blend = BLENDFUNCTION {
            BlendOp: AC_SRC_OVER as u8,
            BlendFlags: 0,
            SourceConstantAlpha: 255,
            AlphaFormat: AC_SRC_ALPHA as u8,
        };
        if let Err(e) = UpdateLayeredWindow(
            hwnd,
            None,
            Some(&POINT { x, y }),
            Some(&SIZE { cx: w, cy: h }),
            Some(memory),
            Some(&POINT { x: 0, y: 0 }),
            COLORREF(0),
            Some(&blend),
            ULW_ALPHA,
        ) {
            eprintln!("stage: indicator ULW failed: {e}");
        }
        SelectObject(memory, previous);
        let _ = DeleteDC(memory);
        let _ = DeleteObject(bitmap.into());
        ReleaseDC(None, screen);
    }
}

#[cfg(test)]
mod tests {
    use super::{placement_delta, AppliedPlacement, PlacementDelta};

    #[test]
    fn resource_visibility_keeps_pending_paint_and_uncertain_layouts_awake() {
        let stage = super::Stage::new(super::HWND::default(), 8.0, |_, _| {});
        let visible = super::ItemId::generate();
        let background = super::ItemId::generate();
        // Layout has selected the item but no controller has attached or
        // announced first paint yet. Resource policy must not suspend it.
        stage.state.borrow_mut().visible.insert(visible);
        assert!(stage.wants_visible(visible));
        assert!(!stage.wants_visible(background));
        let layout = stage.state.borrow_mut();
        assert!(stage.wants_visible(visible));
        assert!(stage.wants_visible(background));
        drop(layout);
        stage.state.borrow_mut().visible.clear();
        assert!(!stage.wants_visible(visible));
    }

    #[test]
    fn identical_placement_emits_no_native_calls() {
        let applied = AppliedPlacement {
            window_visible: Some(true),
            controller_visible: Some(true),
            container_rect: Some((10, 20, 800, 600)),
            controller_size: Some((800, 600)),
            rounded: Some((800, 600, 16)),
            notified_screen_origin: Some((110, 220)),
            raised: Some(false),
        };

        assert_eq!(
            placement_delta(
                applied,
                true,
                Some((10, 20, 800, 600)),
                16,
                false,
                Some((100, 200)),
            ),
            PlacementDelta::default()
        );
    }

    #[test]
    fn hiding_skips_all_geometry_work() {
        let delta = placement_delta(
            AppliedPlacement {
                window_visible: Some(true),
                controller_visible: Some(true),
                container_rect: Some((0, 0, 640, 480)),
                controller_size: Some((640, 480)),
                rounded: Some((640, 480, 16)),
                notified_screen_origin: Some((0, 0)),
                raised: Some(false),
            },
            false,
            Some((500, 500, 1, 1)),
            48,
            false,
            Some((900, 900)),
        );

        assert!(delta.window_visibility);
        assert!(delta.controller_visibility);
        assert!(!delta.container_rect);
        assert!(!delta.controller_size);
        assert!(!delta.rounded);
        assert!(!delta.notify_parent_position);
    }

    #[test]
    fn caption_translation_preserves_the_webview_viewport_at_every_scale() {
        for scale in [1.0, 1.25, 1.5, 2.0] {
            let width = (800.0 * scale) as i32;
            let height = (600.0 * scale) as i32;
            let radius = (16.0 * scale) as i32;
            let applied = AppliedPlacement {
                window_visible: Some(true),
                controller_visible: Some(true),
                container_rect: Some((10, 20, width, height)),
                controller_size: Some((width, height)),
                rounded: Some((width, height, radius)),
                notified_screen_origin: Some((110, 220)),
                raised: Some(false),
            };
            for offset in [1.0, 20.0, 40.0] {
                let delta = placement_delta(
                    applied,
                    true,
                    Some((10, 20 + (offset * scale) as i32, width, height)),
                    radius,
                    false,
                    Some((100, 200)),
                );
                assert!(delta.container_rect);
                assert!(delta.notify_parent_position);
                assert!(!delta.controller_size);
                assert!(!delta.window_visibility);
                assert!(!delta.controller_visibility);
                assert!(!delta.rounded);
                assert!(!delta.raise);
            }
        }
    }

    #[test]
    fn caption_motion_is_window_local_and_retirement_does_not_reuse_it() {
        use super::{forget_window_caption_offset, set_window_caption_offset, Stage};
        use windows::Win32::Foundation::HWND;
        let parent = 101_isize;
        let other = Stage::new(HWND(102_isize as *mut _), 8.0, |_, _| {});
        set_window_caption_offset(parent, 20.0, true);
        let stage = Stage::new(HWND(parent as *mut _), 8.0, |_, _| {});
        assert_eq!(stage.state.borrow().caption_offset, 20.0);
        assert!(stage.state.borrow().caption_moving);
        assert_eq!(other.state.borrow().caption_offset, 0.0);
        set_window_caption_offset(parent, 0.0, false);
        assert_eq!(stage.state.borrow().caption_offset, 0.0);
        assert!(!stage.state.borrow().caption_moving);
        set_window_caption_offset(parent, 40.0, false);
        forget_window_caption_offset(parent);
        let replacement = Stage::new(HWND(parent as *mut _), 8.0, |_, _| {});
        assert_eq!(replacement.state.borrow().caption_offset, 0.0);
        set_window_caption_offset(parent, 20.0, false);
        assert_eq!(replacement.state.borrow().caption_offset, 20.0);
        assert_eq!(stage.state.borrow().caption_offset, 40.0);
        forget_window_caption_offset(parent);
    }

    #[test]
    fn size_change_only_updates_size_dependent_primitives() {
        let delta = placement_delta(
            AppliedPlacement {
                window_visible: Some(true),
                controller_visible: Some(true),
                container_rect: Some((10, 20, 800, 600)),
                controller_size: Some((800, 600)),
                rounded: Some((800, 600, 16)),
                notified_screen_origin: Some((110, 220)),
                raised: Some(false),
            },
            true,
            Some((10, 20, 900, 700)),
            16,
            false,
            Some((100, 200)),
        );

        assert!(!delta.window_visibility);
        assert!(!delta.controller_visibility);
        assert!(delta.container_rect);
        assert!(delta.controller_size);
        assert!(delta.rounded);
        assert!(!delta.notify_parent_position);
    }

    #[test]
    fn fullscreen_squares_and_raises_the_page_once() {
        let windowed = AppliedPlacement {
            window_visible: Some(true),
            controller_visible: Some(true),
            container_rect: Some((0, 0, 1920, 1080)),
            controller_size: Some((1920, 1080)),
            rounded: Some((1920, 1080, 16)),
            notified_screen_origin: Some((0, 0)),
            raised: Some(false),
        };
        let entering = placement_delta(
            windowed,
            true,
            Some((0, 0, 1920, 1080)),
            0,
            true,
            Some((0, 0)),
        );
        assert_eq!(
            entering,
            PlacementDelta {
                rounded: true,
                raise: true,
                ..PlacementDelta::default()
            }
        );
        let fullscreen = AppliedPlacement {
            rounded: Some((1920, 1080, 0)),
            raised: Some(true),
            ..windowed
        };
        assert!(placement_delta(
            fullscreen,
            true,
            Some((0, 0, 1920, 1080)),
            0,
            true,
            Some((0, 0)),
        )
        .is_empty());
        // Leaving puts the corners back and forgets the raise without
        // reordering siblings that never overlap it.
        let leaving = placement_delta(
            fullscreen,
            true,
            Some((256, 8, 1656, 1064)),
            16,
            false,
            Some((0, 0)),
        );
        assert!(leaving.rounded && leaving.raise && leaving.container_rect);
    }

    #[test]
    fn parent_move_only_notifies_webview2() {
        let delta = placement_delta(
            AppliedPlacement {
                window_visible: Some(true),
                controller_visible: Some(true),
                container_rect: Some((10, 20, 800, 600)),
                controller_size: Some((800, 600)),
                rounded: Some((800, 600, 16)),
                notified_screen_origin: Some((110, 220)),
                raised: Some(false),
            },
            true,
            Some((10, 20, 800, 600)),
            16,
            false,
            Some((200, 300)),
        );

        assert_eq!(
            delta,
            PlacementDelta {
                notify_parent_position: true,
                ..PlacementDelta::default()
            }
        );
    }
}
