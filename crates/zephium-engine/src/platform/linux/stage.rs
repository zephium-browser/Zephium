//! Gtk mirror of the macOS ContentStage. Content webviews live in the
//! composition root's gtk::Fixed with the chrome webview beneath them, so
//! pane gaps show the chrome background. Divider and drop feedback are
//! input-transparent native children of the same composition root.
//! Corner rounding is deferred to the UI phase (needs the theme color
//! channel); gaps stay square on Linux until then.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use gtk::prelude::*;
use gtk::{cairo, glib::Propagation};
use wry::WebViewExtUnix;

use zephium_core::geometry::Rect;
use zephium_core::ids::ItemId;
use zephium_core::split::{self, Pane};

use crate::motion_curve::EMPHASIZED;
use crate::pane_geometry::rounded_native_size;

use super::guide::{Guide, GuideStyle};

const CARD_RADIUS: f64 = 12.0;
const CARD_AWAY: f64 = 24.0;
const SLIDE: Duration = Duration::from_millis(400);

#[derive(Clone, Copy, Debug)]
struct CardMotion {
    started: Instant,
    from: f64,
    to: f64,
}

impl CardMotion {
    fn at(self, now: Instant) -> (f64, bool) {
        let elapsed = now.saturating_duration_since(self.started);
        if elapsed >= SLIDE {
            return (self.to, true);
        }
        let progress = EMPHASIZED.at(elapsed.as_secs_f64() / SLIDE.as_secs_f64());
        (self.from + (self.to - self.from) * progress, false)
    }
}

#[derive(Clone)]
struct HostView {
    view: webkit2gtk::WebView,
    presentation_permit: Arc<AtomicBool>,
}

struct State {
    fixed: gtk::Fixed,
    gap: f64,
    origin: (f64, f64),
    size: (f64, f64),
    hidden: bool,
    tree: Option<Pane>,
    views: HashMap<ItemId, HostView>,
    ready: HashSet<ItemId>,
    visible: Vec<ItemId>,
    guide: Guide,
    chrome_overlay: Option<Rect>,
    card_away: f64,
    card_motion: Option<CardMotion>,
    card_tick_scheduled: bool,
    revision: u64,
}

#[derive(Clone)]
pub struct Stage {
    state: Rc<RefCell<State>>,
    sync_scheduled: Rc<Cell<bool>>,
}

impl Stage {
    pub fn new(fixed: gtk::Fixed, gap: f64) -> Self {
        let stage = Self {
            state: Rc::new(RefCell::new(State {
                fixed: fixed.clone(),
                gap,
                origin: (0.0, 0.0),
                size: (0.0, 0.0),
                hidden: true,
                tree: None,
                views: HashMap::new(),
                ready: HashSet::new(),
                visible: Vec::new(),
                guide: Guide::new(&fixed),
                chrome_overlay: None,
                card_away: 0.0,
                card_motion: None,
                card_tick_scheduled: false,
                revision: 0,
            })),
            sync_scheduled: Rc::new(Cell::new(false)),
        };
        let state = Rc::downgrade(&stage.state);
        let scheduled = Rc::downgrade(&stage.sync_scheduled);
        fixed.connect_size_allocate(move |_, _| {
            let (Some(state), Some(scheduled)) = (state.upgrade(), scheduled.upgrade()) else {
                return;
            };
            // A native shrink can precede the shell's updated content region.
            // Recheck containment before any stale allocation can paint.
            if let Ok(mut state) = state.try_borrow_mut() {
                state.revision = state.revision.wrapping_add(1).max(1);
            } else {
                schedule_sync(&state, &scheduled);
                return;
            }
            sync(&state, &scheduled);
        });
        stage
    }

    /// One native pass for frame, tree and visibility; `None` region hides
    /// the whole stage.
    pub fn apply(&self, region: Option<Rect>, tree: Option<Pane>, visible: &[ItemId]) -> bool {
        {
            let Ok(mut s) = self.state.try_borrow_mut() else {
                return false;
            };
            s.hidden = region.is_none();
            if let Some(r) = region {
                s.origin = (r.x, r.y);
                s.size = (r.width, r.height);
            }
            s.tree = tree;
            s.visible = visible.to_vec();
            s.revision = s.revision.wrapping_add(1).max(1);
        }
        sync(&self.state, &self.sync_scheduled);
        true
    }

    pub fn has_view(&self, id: ItemId) -> bool {
        self.state
            .try_borrow()
            .is_ok_and(|state| state.views.contains_key(&id))
    }

    /// A coalesced layout may run before a later-queued WebKit construction.
    /// Retaining the desired tree lets creation catch the native widget up to
    /// the latest geometry and visibility immediately.
    pub fn contains_item(&self, id: ItemId) -> bool {
        self.state
            .try_borrow()
            .is_ok_and(|state| state.tree.as_ref().is_some_and(|tree| tree.contains(id)))
    }

    pub fn insert_view(
        &self,
        id: ItemId,
        view: &wry::WebView,
        presentation_permit: Arc<AtomicBool>,
    ) -> bool {
        let widget = view.webview();
        widget.set_sensitive(false);
        widget.set_child_visible(false);
        widget.set_opacity(0.0);
        let Ok(mut state) = self.state.try_borrow_mut() else {
            return false;
        };
        state.ready.remove(&id);
        state.views.insert(
            id,
            HostView {
                view: widget.clone(),
                presentation_permit: presentation_permit.clone(),
            },
        );
        state.revision = state.revision.wrapping_add(1).max(1);
        drop(state);
        install_view_clip(&self.state, id, &widget, presentation_permit);
        // The retained layout may predate this widget. Apply it now so a
        // mapped WebKit view never remains at its construction-time bounds.
        sync(&self.state, &self.sync_scheduled);
        true
    }

    /// Guarded WebKitGTK construction remains unmapped until a Stage owns the
    /// first offscreen map. If the retained layout no longer expects a newly
    /// constructed widget, preserve that non-painting, non-input state;
    /// `insert_view`/`sync` performs the measured first-map path later.
    pub fn exclude_unstaged(view: &wry::WebView) {
        let widget = view.webview();
        // Input is revoked before paint or mapping. Opacity alone does not
        // remove a WebKitGTK native input surface, and an ancestor show_all
        // can restore the ordinary visible property after this function.
        widget.set_sensitive(false);
        widget.set_child_visible(false);
        widget.set_opacity(0.0);
        if widget.is_visible() {
            widget.hide();
        }
    }

    pub fn remove_view(&self, id: ItemId) {
        // wry owns the widget; dropping the webview removes it from the Fixed.
        if let Ok(mut state) = self.state.try_borrow_mut() {
            state.ready.remove(&id);
            if state.views.remove(&id).is_some() {
                state.revision = state.revision.wrapping_add(1).max(1);
            }
        }
    }

    /// Makes an offscreen-mapped WebKit widget presentable only after
    /// privileged chrome verified its attributed URL/revision. The pending
    /// path establishes its compositing surface without exposing paint or
    /// input at trusted-chrome geometry.
    pub fn set_ready(&self, id: ItemId) -> bool {
        let attached = match self.state.try_borrow_mut() {
            Ok(mut state) => {
                if !state.views.contains_key(&id) {
                    return false;
                }
                if state.ready.insert(id) {
                    state.revision = state.revision.wrapping_add(1).max(1);
                }
                true
            }
            Err(_) => false,
        };
        if !attached {
            return false;
        }
        // Re-drive an identical value too: an earlier GTK re-entry may have
        // interrupted the native opacity/visibility pass after `ready` was
        // retained. The revision need not change for this retry.
        sync(&self.state, &self.sync_scheduled);
        true
    }

    /// Re-arm the presentation barrier for a newly committed main-frame
    /// document. Wry unmaps synchronously at commit; the next Stage sync may
    /// remap it only at the parked offscreen allocation, preventing a newer
    /// document from borrowing the prior document's readiness acknowledgement.
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
                true
            }
            Err(_) => false,
        };
        if attached {
            sync(&self.state, &self.sync_scheduled);
        }
        attached
    }

    pub fn set_drop_indicator(&self, preview: Option<zephium_core::ports::engine::DropPreview>) {
        let Some((guide, origin)) = self
            .state
            .try_borrow()
            .ok()
            .map(|state| (state.guide.clone(), state.origin))
        else {
            return;
        };
        if let Some(preview) = preview {
            let zone = preview.zone;
            guide.show(
                Rect::new(
                    origin.0 + zone.x,
                    origin.1 + zone.y,
                    zone.width,
                    zone.height,
                ),
                GuideStyle::Drop,
            );
        } else {
            guide.hide();
        }
    }

    pub fn set_resize_guide(&self, zone: Option<Rect>) {
        let Some(guide) = self
            .state
            .try_borrow()
            .ok()
            .map(|state| state.guide.clone())
        else {
            return;
        };
        if let Some(zone) = zone {
            guide.show(zone, GuideStyle::Resize);
        } else {
            guide.hide();
        }
    }

    pub fn raise_chrome(&self, raised: bool, card: Rect, slide: bool) -> bool {
        if raised
            && (![card.x, card.y, card.width, card.height]
                .into_iter()
                .all(f64::is_finite)
                || card.x < 0.0
                || card.y < 0.0
                || card.width <= 0.0
                || card.height <= 0.0)
        {
            return false;
        }
        let Ok(mut state) = self.state.try_borrow_mut() else {
            return false;
        };
        let now = Instant::now();
        let away = match (state.chrome_overlay, state.card_motion) {
            (None, _) => 1.0,
            (_, Some(motion)) => motion.at(now).0,
            (_, None) => state.card_away,
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
            state.chrome_overlay = Some(card);
            state.card_away = away;
        } else {
            state.chrome_overlay = raised.then_some(card);
            state.card_away = to;
            state.card_motion = None;
        }
        state.revision = state.revision.wrapping_add(1).max(1);
        drop(state);
        sync(&self.state, &self.sync_scheduled);
        schedule_card_tick(&self.state, &self.sync_scheduled);
        true
    }
}

fn sync(state: &Rc<RefCell<State>>, sync_scheduled: &Rc<Cell<bool>>) {
    // GTK geometry/visibility calls may synchronously pump callbacks. Clone
    // GObject handles and immutable model data first, then retry from the
    // latest revision if re-entry changed ownership while this pass ran.
    'attempt: for _ in 0..4 {
        let Some((
            revision,
            fixed,
            gap,
            origin,
            size,
            hidden,
            tree,
            views,
            visible,
            ready,
            overlay,
            away,
        )) = state.try_borrow().ok().map(|state| {
            (
                state.revision,
                state.fixed.clone(),
                state.gap,
                state.origin,
                state.size,
                state.hidden,
                state.tree.clone(),
                state
                    .views
                    .iter()
                    .map(|(id, view)| (*id, view.clone()))
                    .collect::<Vec<_>>(),
                state.visible.clone(),
                state.ready.clone(),
                state.chrome_overlay,
                state.card_away,
            )
        })
        else {
            schedule_sync(state, sync_scheduled);
            return;
        };
        let local = Rect::new(0.0, 0.0, size.0, size.1);
        let panes = tree
            .as_ref()
            .map_or_else(Vec::new, |tree| split::layout(tree, local, gap));
        let root = fixed.allocation();
        let placements = views
            .into_iter()
            .map(|(id, view)| {
                let pane = panes.iter().find(|(pid, _)| *pid == id).and_then(|(_, r)| {
                    rounded_native_size(r.width, r.height, 1.0)?;
                    native_pane(*r, origin, size, (root.width(), root.height()))
                        .map(|r| (r, (r.width(), r.height())))
                });
                let mapped = !hidden && pane.is_some() && visible.contains(&id);
                (id, view, pane, mapped)
            })
            .collect::<Vec<_>>();

        // Fail closed before any geometry or paint work. Opacity is not an
        // input or mapping barrier: revoke sensitivity first and child-visible
        // second. Only then may opacity, position or size change. The mapping
        // barrier both prevents compositor black layers at the old bounds and
        // survives Tao/GTK ancestor show_all calls.
        for (id, view, _, mapped) in &placements {
            let snapshot_ready =
                ready.contains(id) && view.presentation_permit.load(Ordering::Acquire);
            if !mapped || !snapshot_ready {
                if view.view.is_sensitive() {
                    view.view.set_sensitive(false);
                    if !revision_is_current(state, revision) {
                        revoke_and_unmap_for_retry(state, revision, &view.view);
                        continue 'attempt;
                    }
                }
                if view.view.is_child_visible() {
                    view.view.set_child_visible(false);
                    if !revision_is_current(state, revision) {
                        continue 'attempt;
                    }
                }
                if view.view.opacity() != 0.0 {
                    view.view.set_opacity(0.0);
                    if !revision_is_current(state, revision) {
                        continue 'attempt;
                    }
                }
            }
            if !mapped && view.view.is_visible() {
                view.view.hide();
                if !revision_is_current(state, revision) {
                    continue 'attempt;
                }
            }
        }

        // Keep provisional current-layout widgets mapped so WebKitGTK owns a
        // live compositing surface, but park their correctly-sized allocation
        // fully to the left of the composition root. A transparent native child can
        // otherwise paint a black hardware layer and intercept clicks meant
        // for the privileged New Tab surface beneath it.
        for (id, view, pane, mapped) in &placements {
            let snapshot_ready =
                ready.contains(id) && view.presentation_permit.load(Ordering::Acquire);
            if !mapped || snapshot_ready {
                continue;
            }
            if let Some((_, (width, height))) = pane {
                let Some(parked_x) = parked_x(*width) else {
                    // No representable offscreen allocation exists. Keep the
                    // child behind the mapping barrier; a later sane layout
                    // can restore its compositing surface.
                    continue;
                };
                // The child-visible barrier above is still active here, so
                // neither call can expose the old or new native allocation.
                fixed.move_(&view.view, parked_x, 0);
                if !revision_is_current(state, revision) {
                    continue 'attempt;
                }
                view.view.set_size_request(*width, *height);
                if !revision_is_current(state, revision) {
                    continue 'attempt;
                }
                if !view.view.is_visible() {
                    // Show behind the mapping barrier so GTK accepts the
                    // allocation before creating the compositing surface.
                    view.view.show_all();
                    if !revision_is_current(state, revision) {
                        continue 'attempt;
                    }
                }
                let Some(allocation) = child_allocation(&fixed, parked_x, 0, *width, *height)
                else {
                    continue;
                };
                view.view.size_allocate(&allocation);
                if !revision_is_current(state, revision) {
                    continue 'attempt;
                }
            }
            if !view.view.is_child_visible() {
                view.view.set_child_visible(true);
                if !revision_is_current(state, revision) {
                    continue 'attempt;
                }
            }
            // child-visible was synchronously cleared in the first pass even
            // when an ancestor show_all left the ordinary visible property
            // true. Always rebuild the just-remapped offscreen surface.
            view.view.queue_resize();
            if !revision_is_current(state, revision) {
                continue 'attempt;
            }
            view.view.queue_draw();
            if !revision_is_current(state, revision) {
                continue 'attempt;
            }
        }

        for (id, view, pane, mapped) in &placements {
            if !mapped || !ready.contains(id) {
                continue;
            }
            // A native call above may have synchronously installed a newer
            // tab/split layout. Prove both that exact layout and the exact
            // document-generation permit immediately around the reveal.
            if !view_may_reveal(state, revision, *id, view) {
                if !revision_is_current(state, revision) {
                    continue 'attempt;
                }
                continue;
            }
            let Some((r, (width, height))) = pane else {
                continue;
            };
            if view.view.size_request() != (*width, *height) {
                view.view.set_size_request(*width, *height);
            }
            if !view_may_reveal(state, revision, *id, view) {
                revoke_and_unmap_for_retry(state, revision, &view.view);
                continue 'attempt;
            }
            if fixed.child_property::<i32>(&view.view, "x") != r.x()
                || fixed.child_property::<i32>(&view.view, "y") != r.y()
            {
                fixed.move_(&view.view, r.x(), r.y());
            }
            if !view_may_reveal(state, revision, *id, view) {
                revoke_and_unmap_for_retry(state, revision, &view.view);
                continue 'attempt;
            }
            if !view.view.is_visible() {
                view.view.set_child_visible(false);
                if !view_may_reveal(state, revision, *id, view) {
                    revoke_and_unmap_for_retry(state, revision, &view.view);
                    continue 'attempt;
                }
                view.view.show_all();
                if !view_may_reveal(state, revision, *id, view) {
                    revoke_and_unmap_for_retry(state, revision, &view.view);
                    continue 'attempt;
                }
            }
            let Some(allocation) = child_allocation(&fixed, r.x(), r.y(), *width, *height) else {
                revoke_and_unmap_for_retry(state, revision, &view.view);
                continue;
            };
            // Requests and Fixed coordinates take effect on a later GTK pass.
            // Commit the native move/resize before revealing, retaining focus
            // for an already-presented page throughout sidebar/split resize.
            if view.view.allocation() != allocation {
                view.view.size_allocate(&allocation);
            }
            if !view_may_reveal(state, revision, *id, view) {
                revoke_and_unmap_for_retry(state, revision, &view.view);
                continue 'attempt;
            }
            if !apply_input_clip(&fixed, &view.view, overlay, away, || {
                view_may_reveal(state, revision, *id, view)
            }) {
                revoke_and_unmap_for_retry(state, revision, &view.view);
                continue 'attempt;
            }
            view.view.queue_draw();
            if !view_may_reveal(state, revision, *id, view) {
                revoke_and_unmap_for_retry(state, revision, &view.view);
                continue 'attempt;
            }
            if view.view.opacity() != 1.0 {
                view.view.set_opacity(1.0);
            }
            if !view_may_reveal(state, revision, *id, view) {
                // Native calls may pump a newer layout or WebKit commit.
                // Revoke input before paint and park synchronously before
                // retrying, so stale content can neither render nor receive a
                // gesture intended for the newer trusted-chrome state.
                revoke_and_unmap_for_retry(state, revision, &view.view);
                continue 'attempt;
            }
            let remapped = !view.view.is_child_visible();
            if remapped {
                view.view.set_child_visible(true);
                if !view_may_reveal(state, revision, *id, view) {
                    revoke_and_unmap_for_retry(state, revision, &view.view);
                    continue 'attempt;
                }
            }
            if remapped {
                view.view.queue_resize();
                if !view_may_reveal(state, revision, *id, view) {
                    revoke_and_unmap_for_retry(state, revision, &view.view);
                    continue 'attempt;
                }
                view.view.queue_draw();
                if !view_may_reveal(state, revision, *id, view) {
                    revoke_and_unmap_for_retry(state, revision, &view.view);
                    continue 'attempt;
                }
            }
            // Input is the final surface enabled. All mapping, geometry,
            // paint and redraw calls have settled under exact checks before
            // this point; revalidate immediately around the setter too.
            if !view_may_reveal(state, revision, *id, view) {
                revoke_and_unmap_for_retry(state, revision, &view.view);
                continue 'attempt;
            }
            if !view.view.is_sensitive() {
                view.view.set_sensitive(true);
            }
            if !view_may_reveal(state, revision, *id, view) {
                revoke_and_unmap_for_retry(state, revision, &view.view);
                continue 'attempt;
            }
        }
        if state
            .try_borrow()
            .is_ok_and(|state| state.revision == revision)
        {
            let guide = state.try_borrow().ok().map(|state| state.guide.clone());
            if let Some(guide) = guide {
                guide.raise();
                if revision_is_current(state, revision) {
                    return;
                }
            }
        }
    }
    schedule_sync(state, sync_scheduled);
    conceal_views_until_retry(state);
}

fn revision_is_current(state: &Rc<RefCell<State>>, revision: u64) -> bool {
    state
        .try_borrow()
        .is_ok_and(|state| state.revision == revision)
}

fn schedule_card_tick(state: &Rc<RefCell<State>>, scheduled: &Rc<Cell<bool>>) {
    let fixed = {
        let Ok(mut state) = state.try_borrow_mut() else {
            return;
        };
        if state.card_tick_scheduled || state.card_motion.is_none() {
            return;
        }
        state.card_tick_scheduled = true;
        state.fixed.clone()
    };
    let weak = Rc::downgrade(state);
    let weak_scheduled = Rc::downgrade(scheduled);
    fixed.add_tick_callback(move |_, _| {
        let (Some(state), Some(scheduled)) = (weak.upgrade(), weak_scheduled.upgrade()) else {
            return gtk::glib::ControlFlow::Break;
        };
        {
            let Ok(mut state) = state.try_borrow_mut() else {
                return gtk::glib::ControlFlow::Continue;
            };
            let Some(motion) = state.card_motion else {
                state.card_tick_scheduled = false;
                return gtk::glib::ControlFlow::Break;
            };
            let (away, finished) = motion.at(Instant::now());
            state.card_away = away;
            if finished {
                state.card_motion = None;
                if motion.to >= 1.0 {
                    state.chrome_overlay = None;
                }
            }
            state.revision = state.revision.wrapping_add(1).max(1);
        }
        sync_card(&state, &scheduled);
        let Ok(mut state) = state.try_borrow_mut() else {
            return gtk::glib::ControlFlow::Continue;
        };
        if state.card_motion.is_some() {
            gtk::glib::ControlFlow::Continue
        } else {
            state.card_tick_scheduled = false;
            gtk::glib::ControlFlow::Break
        }
    });
}

fn sync_card(state: &Rc<RefCell<State>>, scheduled: &Rc<Cell<bool>>) {
    'attempt: for _ in 0..4 {
        let Some((revision, fixed, overlay, away, views)) = state.try_borrow().ok().map(|state| {
            (
                state.revision,
                state.fixed.clone(),
                state.chrome_overlay,
                state.card_away,
                state
                    .views
                    .iter()
                    .map(|(id, view)| (*id, view.clone()))
                    .collect::<Vec<_>>(),
            )
        }) else {
            schedule_sync(state, scheduled);
            return;
        };
        for (id, view) in views {
            if !view.view.is_child_visible() || !view.view.is_sensitive() {
                continue;
            }
            if !apply_input_clip(&fixed, &view.view, overlay, away, || {
                view_may_reveal(state, revision, id, &view)
            }) {
                revoke_and_unmap_for_retry(state, revision, &view.view);
                if !revision_is_current(state, revision) {
                    continue 'attempt;
                }
                continue;
            }
            view.view.queue_draw();
            if !view_may_reveal(state, revision, id, &view) {
                revoke_and_unmap_for_retry(state, revision, &view.view);
                continue 'attempt;
            }
        }
        if revision_is_current(state, revision) {
            return;
        }
    }
    schedule_sync(state, scheduled);
    conceal_views_until_retry(state);
}

fn install_view_clip(
    state: &Rc<RefCell<State>>,
    id: ItemId,
    view: &webkit2gtk::WebView,
    permit: Arc<AtomicBool>,
) {
    let weak = Rc::downgrade(state);
    let draw_permit = permit.clone();
    view.connect_draw(move |view, cr| {
        let Some(state) = weak.upgrade() else {
            return Propagation::Proceed;
        };
        let Ok(state) = state.try_borrow() else {
            return Propagation::Stop;
        };
        if !state.views.get(&id).is_some_and(|owned| {
            owned.view == *view && Arc::ptr_eq(&owned.presentation_permit, &draw_permit)
        }) {
            return Propagation::Proceed;
        }
        let (fixed, overlay, away) = (state.fixed.clone(), state.chrome_overlay, state.card_away);
        drop(state);
        if let Some(region) = card_clip_region(&fixed, view, overlay, away) {
            clip_cairo(cr, &region);
            draw_webkit(view, cr);
            return Propagation::Stop;
        }
        Propagation::Proceed
    });
    let weak = Rc::downgrade(state);
    view.connect_realize(move |view| {
        let Some(state) = weak.upgrade() else {
            return;
        };
        let Some((revision, fixed, overlay, away)) = state.try_borrow().ok().and_then(|state| {
            state
                .views
                .get(&id)
                .filter(|owned| {
                    owned.view == *view && Arc::ptr_eq(&owned.presentation_permit, &permit)
                })
                .map(|_| {
                    (
                        state.revision,
                        state.fixed.clone(),
                        state.chrome_overlay,
                        state.card_away,
                    )
                })
        }) else {
            return;
        };
        if !apply_input_clip(&fixed, view, overlay, away, || {
            revision_is_current(&state, revision)
        }) {
            revoke_and_unmap_for_retry(&state, revision, view);
        }
    });
}

fn draw_webkit(view: &webkit2gtk::WebView, cr: &cairo::Context) {
    let class = view.class();
    let class = (class.as_ref() as *const webkit2gtk::ffi::WebKitWebViewClass)
        .cast::<gtk::ffi::GtkWidgetClass>();
    // SAFETY: WebKitWebView derives from GtkWidget. This live widget's class
    // and Cairo context belong to this GTK draw callback. Calling its draw
    // vfunc directly preserves the clip; GTK restores Cairo state between
    // signal handlers, so proceeding to the default signal would discard it.
    unsafe {
        if let Some(draw) = (*class).draw {
            draw(view.as_ptr().cast(), cr.to_raw_none());
        }
    }
}

fn clip_cairo(cr: &cairo::Context, region: &cairo::Region) {
    cr.new_path();
    for i in 0..region.num_rectangles() {
        let r = region.rectangle(i);
        cr.rectangle(
            f64::from(r.x()),
            f64::from(r.y()),
            f64::from(r.width()),
            f64::from(r.height()),
        );
    }
    cr.clip();
}

fn card_clip_region(
    fixed: &gtk::Fixed,
    view: &webkit2gtk::WebView,
    card: Option<Rect>,
    away: f64,
) -> Option<cairo::Region> {
    let card = card?;
    let root = fixed.allocation();
    if card.x.round() >= f64::from(root.width()) || card.y.round() >= f64::from(root.height() / 2) {
        return None;
    }
    let (x, y, width, height) =
        crate::card_geometry::card_pixels(card, (root.width(), root.height()), 1.0)?;
    let travel = ((card.width + CARD_AWAY) * away.clamp(0.0, 1.0)).round() as i32;
    let x = x.checked_sub(travel)?;
    let page = view.allocation();
    let (px, py) = if fixed.has_window() {
        (page.x(), page.y())
    } else {
        (
            page.x().checked_sub(root.x())?,
            page.y().checked_sub(root.y())?,
        )
    };
    rounded_card_exclusion(
        (page.width(), page.height()),
        (x.checked_sub(px)?, y.checked_sub(py)?, width, height),
    )
}

fn rounded_card_exclusion(size: (i32, i32), card: (i32, i32, i32, i32)) -> Option<cairo::Region> {
    let (x, y, width, height) = card;
    if size.0 <= 0 || size.1 <= 0 || width <= 0 || height <= 0 {
        return None;
    }
    let radius = (CARD_RADIUS as i32).min(width / 2).min(height / 2);
    let region = cairo::Region::create_rectangle(&cairo::RectangleInt::new(0, 0, size.0, size.1));
    region
        .subtract_rectangle(&cairo::RectangleInt::new(
            x,
            y.checked_add(radius)?,
            width,
            height.checked_sub(2 * radius)?,
        ))
        .ok()?;
    for row in 0..radius {
        let dy = f64::from(radius - row) - 0.5;
        let inset =
            (f64::from(radius) - (f64::from(radius * radius) - dy * dy).sqrt()).floor() as i32;
        let left = x.checked_add(inset)?;
        let span = width.checked_sub(2 * inset)?;
        for top in [
            y.checked_add(row)?,
            y.checked_add(height)?.checked_sub(row + 1)?,
        ] {
            region
                .subtract_rectangle(&cairo::RectangleInt::new(left, top, span, 1))
                .ok()?;
        }
    }
    Some(region)
}

fn apply_input_clip(
    fixed: &gtk::Fixed,
    view: &webkit2gtk::WebView,
    card: Option<Rect>,
    away: f64,
    current: impl Fn() -> bool,
) -> bool {
    if !current() {
        return false;
    }
    if let Some(window) = view.window() {
        // WebKit's accelerated Wayland paint ignores GDK's visual shape.
        // The draw callback clips Cairo too; X11 needs the native shape to
        // uncover the chrome beneath a child window's old opaque pixels.
        let region = card_clip_region(fixed, view, card, away);
        window.shape_combine_region(region.as_ref(), 0, 0);
        if !current() {
            return false;
        }
        let region = region.unwrap_or_else(|| {
            cairo::Region::create_rectangle(&cairo::RectangleInt::new(
                0,
                0,
                view.allocated_width(),
                view.allocated_height(),
            ))
        });
        window.input_shape_combine_region(&region, 0, 0);
    }
    current()
}

fn native_pane(
    pane: Rect,
    origin: (f64, f64),
    size: (f64, f64),
    root_size: (i32, i32),
) -> Option<gtk::Allocation> {
    let (ox, oy) = origin;
    let (width, height) = size;
    if [
        ox,
        oy,
        width,
        height,
        pane.x,
        pane.y,
        pane.width,
        pane.height,
    ]
    .into_iter()
    .any(|value| !value.is_finite())
        || ox < 0.0
        || oy < 0.0
        || width <= 0.0
        || height <= 0.0
        || pane.x < 0.0
        || pane.y < 0.0
        || pane.width <= 0.0
        || pane.height <= 0.0
    {
        return None;
    }
    // Round shared split edges together, but stay inward at chrome edges.
    let left = (ox + pane.x).round().max(ox.ceil());
    let top = (oy + pane.y).round().max(oy.ceil());
    let right = (ox + pane.x + pane.width).round().min((ox + width).floor());
    let bottom = (oy + pane.y + pane.height)
        .round()
        .min((oy + height).floor());
    if [left, top, right, bottom]
        .into_iter()
        .any(|value| !value.is_finite())
        || left < 0.0
        || top < 0.0
        || right > f64::from(root_size.0)
        || bottom > f64::from(root_size.1)
    {
        return None;
    }
    let (width, height) = rounded_native_size(right - left, bottom - top, 1.0)?;
    Some(gtk::Allocation::new(left as i32, top as i32, width, height))
}

fn child_allocation(
    fixed: &gtk::Fixed,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
) -> Option<gtk::Allocation> {
    let (x, y) = if fixed.has_window() {
        (x, y)
    } else {
        let parent = fixed.allocation();
        (parent.x().checked_add(x)?, parent.y().checked_add(y)?)
    };
    Some(gtk::Allocation::new(x, y, width, height))
}

fn parked_x(child_width: i32) -> Option<i32> {
    if child_width <= 0 {
        return None;
    }
    // Park entirely to the left. A positive offscreen coordinate contributes
    // to GtkFixed's preferred width and can grow the composition root; the
    // right edge at -1 is clipped without changing the root requisition.
    child_width.checked_neg()?.checked_sub(1)
}

fn revoke_and_unmap_for_retry(
    state: &Rc<RefCell<State>>,
    revision: u64,
    view: &webkit2gtk::WebView,
) -> bool {
    // Do not return early when native re-entry supersedes `revision`: this is
    // a terminal fail-closed transition for the stale snapshot. Re-read after
    // every GTK call, and finish with a second mapping barrier so a nested
    // newer sync cannot leave the outer stale surface mapped. The caller then
    // retries from the authoritative revision immediately.
    view.set_sensitive(false);
    let after_sensitivity = revision_is_current(state, revision);
    view.set_child_visible(false);
    let after_unmap = revision_is_current(state, revision);
    view.set_opacity(0.0);
    let after_opacity = revision_is_current(state, revision);
    view.set_child_visible(false);
    let after_final_unmap = revision_is_current(state, revision);
    after_sensitivity && after_unmap && after_opacity && after_final_unmap
}

fn view_may_reveal(state: &Rc<RefCell<State>>, revision: u64, id: ItemId, view: &HostView) -> bool {
    view.presentation_permit.load(Ordering::Acquire)
        && state.try_borrow().is_ok_and(|state| {
            state.revision == revision
                && !state.hidden
                && state.visible.contains(&id)
                && state.ready.contains(&id)
                && state.tree.as_ref().is_some_and(|tree| tree.contains(id))
                && state.views.get(&id).is_some_and(|current| {
                    Arc::ptr_eq(&current.presentation_permit, &view.presentation_permit)
                })
        })
        && view.presentation_permit.load(Ordering::Acquire)
}

fn schedule_sync(state: &Rc<RefCell<State>>, sync_scheduled: &Rc<Cell<bool>>) {
    // At most one idle turn may own the retry obligation. GTK callbacks can
    // synchronously re-enter layout more often than the bounded immediate
    // loop permits; retaining this bit guarantees the latest revision gets a
    // fresh main-loop pass without recursion or an unbounded callback queue.
    if sync_scheduled.replace(true) {
        return;
    }
    let weak_state = Rc::downgrade(state);
    let weak_scheduled = Rc::downgrade(sync_scheduled);
    gtk::glib::idle_add_local_once(move || {
        let (Some(state), Some(sync_scheduled)) = (weak_state.upgrade(), weak_scheduled.upgrade())
        else {
            return;
        };
        sync_scheduled.set(false);
        sync(&state, &sync_scheduled);
    });
}

fn conceal_views_until_retry(state: &Rc<RefCell<State>>) {
    // A GTK frame can be painted before the idle retry. Once immediate
    // convergence is exhausted, no stale tab may remain opaque beneath newer
    // browser chrome; the privileged chrome surface is the fail-closed backing.
    let Some((revision, views)) = state.try_borrow().ok().map(|state| {
        (
            state.revision,
            state.views.values().cloned().collect::<Vec<_>>(),
        )
    }) else {
        return;
    };
    for view in views {
        // The already-scheduled authoritative retry remaps only expected
        // panes. Until then, prefer a short compositing-surface interruption
        // over one frame of stale paint or page-owned input.
        let _ = revoke_and_unmap_for_retry(state, revision, &view.view);
        if view.view.is_visible() {
            view.view.hide();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gtk::prelude::ContainerExtManual;
    use webkit2gtk::WebViewExt as _;

    #[test]
    fn parking_coordinate_places_the_complete_child_left_of_the_root() {
        assert_eq!(parked_x(720), Some(-721));
        assert_eq!(parked_x(1), Some(-2));
        assert_eq!(parked_x(i32::MAX), Some(i32::MIN));
        assert_eq!(parked_x(0), None);
        assert_eq!(parked_x(-1), None);
    }

    #[test]
    fn card_motion_uses_chrome_timing_and_reverses_from_its_current_position() {
        let start = Instant::now();
        let opening = CardMotion {
            started: start,
            from: 1.0,
            to: 0.0,
        };
        assert_eq!(opening.at(start), (1.0, false));
        let middle = start + Duration::from_millis(100);
        let (away, finished) = opening.at(middle);
        assert!(!finished);
        assert!(away > 0.0 && away < 0.4);
        assert_eq!(opening.at(start + SLIDE), (0.0, true));
        let closing = CardMotion {
            started: middle,
            from: away,
            to: 1.0,
        };
        assert_eq!(closing.at(middle), (away, false));
        assert!(closing.at(middle + Duration::from_millis(100)).0 > away);
        assert_eq!(closing.at(middle + SLIDE), (1.0, true));
    }

    #[test]
    fn fractional_pane_edges_stay_inside_chrome_and_share_split_rounding() {
        let first = ItemId::from(1);
        let second = ItemId::from(2);
        let tree = Pane::Branch {
            axis: split::Axis::Row,
            ratio: 0.37,
            a: Box::new(Pane::Leaf(first)),
            b: Box::new(Pane::Leaf(second)),
        };
        for width in [359.75, 659.75, 719.75] {
            let panes = split::layout(&tree, Rect::new(0.0, 0.0, width, 703.75), 8.0);
            let allocations = panes
                .into_iter()
                .map(|(_, pane)| native_pane(pane, (240.25, 16.25), (width, 703.75), (960, 720)))
                .collect::<Option<Vec<_>>>()
                .expect("contained split");
            let (a, b) = (&allocations[0], &allocations[1]);
            assert_eq!(a.x(), 241);
            assert_eq!(a.y(), 17);
            assert_eq!(a.y() + a.height(), 720);
            assert_eq!(b.x() - a.x() - a.width(), 8);
            assert_eq!(b.x() + b.width(), (240.25 + width).floor() as i32);
        }
    }

    #[test]
    fn malformed_geometry_is_concealed_and_root_resize_can_admit_it_again() {
        let pane = Rect::new(0.0, 0.0, 720.0, 720.0);
        for origin in [
            (f64::NAN, 0.0),
            (0.0, f64::INFINITY),
            (-1.0, 0.0),
            (0.0, -0.1),
            (f64::from(i32::MAX) + 1.0, 0.0),
        ] {
            assert!(native_pane(pane, origin, (720.0, 720.0), (960, 720)).is_none());
        }
        assert!(native_pane(pane, (240.0, 0.0), (f64::INFINITY, 720.0), (960, 720)).is_none());
        assert!(native_pane(pane, (240.0, 0.0), (720.0, 720.0), (800, 600)).is_none());
        assert_eq!(
            native_pane(pane, (240.0, 0.0), (720.0, 720.0), (960, 720)),
            Some(gtk::Allocation::new(240, 0, 720, 720))
        );
        let smaller = Rect::new(0.0, 0.0, 560.0, 600.0);
        assert_eq!(
            native_pane(smaller, (240.0, 0.0), (560.0, 600.0), (800, 600)),
            Some(gtk::Allocation::new(240, 0, 560, 600))
        );
    }

    #[test]
    fn rounded_card_uses_the_same_exclusion_for_paint_and_pointer_input() {
        let region = rounded_card_exclusion((640, 480), (8, 8, 256, 464)).expect("card clip");
        assert!(!region.contains_point(100, 100));
        assert!(region.contains_point(400, 100));
        assert!(region.contains_point(8, 8));
        assert!(region.contains_point(0, 100));
        assert!(!region.contains_point(20, 8));
        let mut surface =
            cairo::ImageSurface::create(cairo::Format::ARgb32, 640, 480).expect("paint surface");
        let cr = cairo::Context::new(&surface).expect("paint context");
        cr.set_source_rgb(0.0, 1.0, 0.0);
        cr.paint().expect("chrome backing");
        clip_cairo(&cr, &region);
        cr.set_source_rgb(1.0, 0.0, 0.0);
        cr.paint().expect("content paint");
        drop(cr);
        let stride = surface.stride() as usize;
        let data = surface.data().expect("paint pixels");
        let pixel = |x: usize, y: usize| {
            let offset = y * stride + x * 4;
            u32::from_ne_bytes(data[offset..offset + 4].try_into().expect("ARGB pixel"))
        };
        assert_eq!(pixel(100, 100), 0xff00_ff00);
        assert_eq!(pixel(400, 100), 0xffff_0000);
        assert_eq!(pixel(8, 8), 0xffff_0000);
        assert!(rounded_card_exclusion((640, 480), (0, i32::MAX, 256, 464)).is_none());
        assert!(rounded_card_exclusion((640, 480), (0, 0, 0, 464)).is_none());
    }

    #[test]
    fn concealment_revokes_input_before_paint_and_mapping() {
        let source = include_str!("stage.rs");
        let exclude = source
            .split("pub fn exclude_unstaged")
            .nth(1)
            .and_then(|source| source.split("pub fn remove_view").next())
            .expect("unstaged concealment body");
        let sensitivity = exclude
            .find("widget.set_sensitive(false)")
            .expect("input revocation");
        let opacity = exclude
            .find("widget.set_opacity(0.0)")
            .expect("paint concealment");
        let child_visibility = exclude
            .find("widget.set_child_visible(false)")
            .expect("mapping concealment");
        let hide = exclude.find("widget.hide()").expect("ordinary hide");
        assert!(sensitivity < child_visibility);
        assert!(child_visibility < opacity);
        assert!(opacity < hide);

        let sync_conceal = source
            .split("// Fail closed before any geometry or paint work")
            .nth(1)
            .and_then(|source| {
                source
                    .split("// Keep provisional current-layout widgets mapped")
                    .next()
            })
            .expect("synchronous stage concealment pass");
        let sensitivity = sync_conceal
            .find("view.view.set_sensitive(false)")
            .expect("stage input revocation");
        let mapping = sync_conceal
            .find("view.view.set_child_visible(false)")
            .expect("stage mapping barrier");
        assert!(sensitivity < mapping);
        let sensitivity_reentry = &sync_conceal[sensitivity..mapping];
        assert!(sensitivity_reentry.contains("revoke_and_unmap_for_retry"));

        let revoke = source
            .split("fn revoke_and_unmap_for_retry")
            .nth(1)
            .and_then(|source| source.split("fn view_may_reveal").next())
            .expect("fail-closed unmapping helper");
        assert!(
            revoke.find("view.set_sensitive(false)").unwrap()
                < revoke.find("view.set_child_visible(false)").unwrap()
        );
        assert!(
            revoke.find("view.set_child_visible(false)").unwrap()
                < revoke.find("view.set_opacity(0.0)").unwrap()
        );
        assert_eq!(revoke.matches("view.set_child_visible(false)").count(), 2);

        let pending = source
            .split("for (id, view, pane, mapped) in &placements")
            .nth(1)
            .and_then(|source| {
                source
                    .split("for (id, view, pane, mapped) in &placements")
                    .next()
            })
            .expect("pending mapped-surface pass");
        assert!(
            pending
                .find("fixed.move_(&view.view, parked_x, 0)")
                .unwrap()
                < pending.find("view.view.set_size_request").unwrap()
        );
        assert!(
            pending.find("view.view.set_size_request").unwrap()
                < pending.find("view.view.set_child_visible(true)").unwrap()
        );

        let reveal = source
            .split("for (id, view, pane, mapped) in &placements")
            .nth(2)
            .and_then(|source| source.split("if state").next())
            .expect("exact presentation reveal pass");
        let input = reveal
            .rfind("view.view.set_sensitive(true)")
            .expect("final input enablement");
        assert!(reveal.rfind("view.view.queue_draw()").unwrap() < input);
        assert!(reveal[..input].rfind("view_may_reveal").is_some());
        assert!(reveal[input..].find("view_may_reveal").is_some());
    }

    fn load_red_frame(view: &webkit2gtk::WebView) {
        let loop_ = gtk::glib::MainLoop::new(None, false);
        let snapshot = Rc::new(RefCell::new(None));
        let loaded = snapshot.clone();
        let finished = loop_.clone();
        let handler = view.connect_load_changed(move |view, event| {
            if event != webkit2gtk::LoadEvent::Finished {
                return;
            }
            let loaded = loaded.clone();
            let finished = finished.clone();
            view.snapshot(
                webkit2gtk::SnapshotRegion::Visible,
                webkit2gtk::SnapshotOptions::NONE,
                None::<&gtk::gio::Cancellable>,
                move |result| {
                    *loaded.borrow_mut() = Some(result);
                    finished.quit();
                },
            );
        });
        let timed_out = Rc::new(Cell::new(false));
        let timeout_flag = timed_out.clone();
        let timeout_loop = loop_.clone();
        let timeout =
            gtk::glib::timeout_add_local_once(std::time::Duration::from_secs(5), move || {
                timeout_flag.set(true);
                timeout_loop.quit();
            });
        view.load_html(
            "<!doctype html><html style='background:#f00'><body style='margin:0;background:#f00;height:100vh'></body></html>",
            None,
        );
        loop_.run();
        view.disconnect(handler);
        assert!(!timed_out.get(), "local WebKit frame did not become ready");
        timeout.remove();
        let surface = snapshot
            .borrow_mut()
            .take()
            .expect("completed local frame")
            .expect("local WebKit snapshot");
        let image = cairo::ImageSurface::try_from(surface).expect("local snapshot pixels");
        let offset = (image.height() as usize / 2) * image.stride() as usize
            + (image.width() as usize / 2) * 4;
        image
            .with_data(|data| {
                assert_eq!(
                    u32::from_ne_bytes(
                        data[offset..offset + 4].try_into().expect("snapshot pixel")
                    ),
                    0xffff_0000,
                    "local page must paint red before native clipping is tested"
                );
            })
            .expect("local snapshot bytes");
    }

    fn expect_native_pixels(window: &gtk::ApplicationWindow, checks: &[(i32, i32, [u8; 3])]) {
        let loop_ = gtk::glib::MainLoop::new(None, false);
        let finished = loop_.clone();
        let checks = checks.to_vec();
        let root = window.window().expect("native test window");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let observed = Rc::new(RefCell::new(Vec::new()));
        let actual = observed.clone();
        let passed = Rc::new(Cell::new(false));
        let success = passed.clone();
        gtk::glib::timeout_add_local(std::time::Duration::from_millis(16), move || {
            root.display().sync();
            let pixels = checks
                .iter()
                .map(|(x, y, _)| {
                    let pixbuf = root.pixbuf(*x, *y, 1, 1)?;
                    pixbuf.read_pixel_bytes().as_ref().get(..3)?.try_into().ok()
                })
                .collect::<Vec<Option<[u8; 3]>>>();
            let matches = pixels
                .iter()
                .zip(&checks)
                .all(|(actual, (_, _, expected))| *actual == Some(*expected));
            *actual.borrow_mut() = pixels;
            if matches || std::time::Instant::now() >= deadline {
                success.set(matches);
                finished.quit();
                gtk::glib::ControlFlow::Break
            } else {
                gtk::glib::ControlFlow::Continue
            }
        });
        window.queue_draw();
        loop_.run();
        assert!(
            passed.get(),
            "native pixels did not settle: {:?}",
            observed.borrow()
        );
    }

    fn wait_for_card_motion(stage: &Stage) {
        let context = gtk::glib::MainContext::default();
        let deadline = Instant::now() + Duration::from_secs(5);
        context.block_on(async {
            while stage.state.borrow().card_motion.is_some() {
                assert!(
                    Instant::now() < deadline,
                    "native card motion did not settle"
                );
                gtk::glib::timeout_future(Duration::from_millis(16)).await;
            }
        });
    }

    #[test]
    #[ignore = "requires a native GTK display; Linux CI runs native GTK tests under Xvfb"]
    fn stage_parks_pending_views_and_hidden_children_survive_ancestor_show_all() {
        gtk::init().expect("GTK display");
        let application = gtk::Application::new(
            Some("dev.zephium.native-stage-test"),
            gtk::gio::ApplicationFlags::NON_UNIQUE,
        );
        application
            .register(None::<&gtk::gio::Cancellable>)
            .expect("register GTK test application");

        let window = gtk::ApplicationWindow::new(&application);
        window.set_default_size(960, 720);
        let fixed = gtk::Fixed::new();
        let chrome = gtk::DrawingArea::new();
        chrome.connect_draw(|_, cr| {
            cr.set_source_rgb(0.0, 1.0, 0.0);
            cr.paint().expect("native chrome backing");
            Propagation::Stop
        });
        chrome.set_can_focus(true);
        chrome.set_size_request(960, 720);
        fixed.put(&chrome, 0, 0);
        let context = webkit2gtk::WebContext::new_ephemeral();
        let content = webkit2gtk::WebView::with_context(&context);
        content.set_background_color(&gtk::gdk::RGBA::new(1.0, 0.0, 0.0, 1.0));
        fixed.put(&content, 240, 0);
        let sibling = webkit2gtk::WebView::with_context(&context);
        sibling.set_background_color(&gtk::gdk::RGBA::new(1.0, 0.0, 0.0, 1.0));
        fixed.put(&sibling, 240, 360);
        window.add(&fixed);
        window.show_all();
        while gtk::events_pending() {
            gtk::main_iteration_do(false);
        }

        let id = ItemId::from(7);
        let sibling_id = ItemId::from(8);
        let permit = Arc::new(AtomicBool::new(false));
        let sibling_permit = Arc::new(AtomicBool::new(false));
        let stage = Stage::new(fixed.clone(), 8.0);
        {
            let mut state = stage.state.borrow_mut();
            state.views.insert(
                id,
                HostView {
                    view: content.clone(),
                    presentation_permit: permit.clone(),
                },
            );
            state.views.insert(
                sibling_id,
                HostView {
                    view: sibling.clone(),
                    presentation_permit: sibling_permit.clone(),
                },
            );
        }

        install_view_clip(&stage.state, id, &content, permit.clone());
        install_view_clip(&stage.state, sibling_id, &sibling, sibling_permit.clone());
        let restored_split = Pane::Branch {
            axis: zephium_core::split::Axis::Col,
            ratio: 0.5,
            a: Box::new(Pane::Leaf(id)),
            b: Box::new(Pane::Leaf(sibling_id)),
        };
        assert!(stage.apply(
            Some(Rect::new(240.0, 0.0, 720.0, 720.0)),
            Some(restored_split),
            &[id, sibling_id],
        ));
        while gtk::events_pending() {
            gtk::main_iteration_do(false);
        }
        let parked: i32 = fixed.child_property(&content, "x");
        let sibling_parked: i32 = fixed.child_property(&sibling, "x");
        assert_eq!(parked, -721);
        assert_eq!(sibling_parked, parked);
        assert!(content.is_visible());
        assert!(content.is_child_visible());
        assert!(!content.is_sensitive());
        assert_eq!(content.opacity(), 0.0);
        assert!(sibling.is_visible());
        assert!(sibling.is_child_visible());
        assert!(!sibling.is_sensitive());
        assert_eq!(sibling.opacity(), 0.0);

        permit.store(true, Ordering::Release);
        sibling_permit.store(true, Ordering::Release);
        assert!(stage.set_ready(id));
        assert!(stage.set_ready(sibling_id));
        assert_eq!(content.allocation(), gtk::Allocation::new(240, 0, 720, 356));
        assert_eq!(
            sibling.allocation(),
            gtk::Allocation::new(240, 364, 720, 356)
        );
        while gtk::events_pending() {
            gtk::main_iteration_do(false);
        }
        let presented_x: i32 = fixed.child_property(&content, "x");
        assert_eq!(presented_x, 240);
        assert!(content.is_sensitive());
        assert_eq!(content.opacity(), 1.0);
        let sibling_presented_x: i32 = fixed.child_property(&sibling, "x");
        let sibling_presented_y: i32 = fixed.child_property(&sibling, "y");
        assert_eq!(sibling_presented_x, 240);
        assert!(sibling_presented_y > 0);
        assert!(sibling.is_sensitive());
        assert_eq!(sibling.opacity(), 1.0);

        window.set_focus(Some(&content));
        let page_focus = window.focused_widget().expect("focused page");
        assert_eq!(page_focus, content.clone().upcast::<gtk::Widget>());
        let column = stage.state.borrow().tree.clone();
        assert!(stage.apply(
            Some(Rect::new(300.25, 16.25, 659.75, 703.75)),
            column,
            &[id, sibling_id],
        ));
        assert_eq!(
            content.allocation(),
            gtk::Allocation::new(301, 17, 659, 347)
        );
        assert_eq!(
            sibling.allocation(),
            gtk::Allocation::new(301, 372, 659, 348)
        );
        assert_eq!(window.focused_widget(), Some(page_focus));
        assert!(content.is_mapped());

        window.set_focus(Some(&chrome));
        let chrome_focus = window.focused_widget().expect("focused chrome");
        let row = Pane::Branch {
            axis: split::Axis::Row,
            ratio: 0.37,
            a: Box::new(Pane::Leaf(id)),
            b: Box::new(Pane::Leaf(sibling_id)),
        };
        assert!(stage.apply(
            Some(Rect::new(240.0, 0.0, 720.0, 720.0)),
            Some(row),
            &[id, sibling_id],
        ));
        assert_eq!(content.allocation(), gtk::Allocation::new(240, 0, 263, 720));
        assert_eq!(sibling.allocation(), gtk::Allocation::new(511, 0, 449, 720));
        assert_eq!(window.focused_widget(), Some(chrome_focus.clone()));

        // A tab switch conceals its old sibling before the new leaf is shown.
        assert!(stage.apply(
            Some(Rect::new(240.0, 0.0, 720.0, 720.0)),
            Some(Pane::Leaf(sibling_id)),
            &[sibling_id],
        ));
        assert!(!content.is_child_visible());
        assert!(!content.is_sensitive());
        assert_eq!(sibling.allocation(), gtk::Allocation::new(240, 0, 720, 720));
        window.show_all();
        assert!(!content.is_mapped());
        assert_eq!(window.focused_widget(), Some(chrome_focus));

        load_red_frame(&sibling);
        let native_page = sibling.allocation();
        expect_native_pixels(
            &window,
            &[
                (native_page.x() + 100, native_page.y() + 100, [255, 0, 0]),
                (native_page.x() + 400, native_page.y() + 100, [255, 0, 0]),
            ],
        );
        let card = Rect::new(248.0, 8.0, 256.0, 704.0);
        let page_before_peek = sibling.allocation();
        assert!(stage.raise_chrome(true, card, false));
        assert_eq!(sibling.allocation(), page_before_peek);
        assert!(sibling.is_mapped());
        assert!(sibling.is_sensitive());
        let clip = card_clip_region(&fixed, &sibling, Some(card), 0.0).expect("native card clip");
        assert!(!clip.contains_point(100, 100));
        assert!(clip.contains_point(400, 100));
        assert!(clip.contains_point(8, 8));

        expect_native_pixels(
            &window,
            &[
                (native_page.x() + 100, native_page.y() + 100, [0, 255, 0]),
                (native_page.x() + 400, native_page.y() + 100, [255, 0, 0]),
            ],
        );
        let root = window.window().expect("native input root");
        let pointer = root
            .display()
            .default_seat()
            .and_then(|seat| seat.pointer())
            .expect("private X11 pointer");
        let (_, root_x, root_y) = root.origin();
        for (x, target) in [(100, chrome.window()), (400, sibling.window())] {
            pointer.warp(
                &root.screen(),
                root_x + native_page.x() + x,
                root_y + native_page.y() + 100,
            );
            root.display().sync();
            assert_eq!(
                pointer.window_at_position().0,
                target,
                "the native pointer must pass through the card hole only"
            );
        }
        assert!(stage.apply(
            Some(Rect::new(300.0, 0.0, 660.0, 720.0)),
            Some(Pane::Leaf(sibling_id)),
            &[sibling_id],
        ));
        let moved = card_clip_region(&fixed, &sibling, Some(card), 0.0).expect("moved card clip");
        assert!(!moved.contains_point(0, 100));
        assert!(moved.contains_point(300, 100));
        assert!(stage.raise_chrome(false, card, false));
        let moved_page = sibling.allocation();
        expect_native_pixels(
            &window,
            &[
                (moved_page.x() + 100, moved_page.y() + 100, [255, 0, 0]),
                (moved_page.x() + 400, moved_page.y() + 100, [255, 0, 0]),
            ],
        );
        assert!(stage.state.borrow().chrome_overlay.is_none());
        assert!(card_clip_region(&fixed, &sibling, None, 0.0).is_none());
        assert!(!stage.raise_chrome(true, Rect::new(f64::NAN, 8.0, 256.0, 704.0), false));
        let focus_before_motion = window.focused_widget();
        let allocation_before_motion = sibling.allocation();
        assert!(stage.raise_chrome(true, card, true));
        let started = stage
            .state
            .borrow()
            .card_motion
            .expect("opening motion")
            .started;
        assert!(stage.raise_chrome(true, card, true));
        assert_eq!(
            stage
                .state
                .borrow()
                .card_motion
                .expect("continued motion")
                .started,
            started
        );
        wait_for_card_motion(&stage);
        expect_native_pixels(
            &window,
            &[
                (moved_page.x() + 100, moved_page.y() + 100, [0, 255, 0]),
                (moved_page.x() + 400, moved_page.y() + 100, [255, 0, 0]),
            ],
        );
        assert_eq!(sibling.allocation(), allocation_before_motion);
        assert_eq!(window.focused_widget(), focus_before_motion);
        assert_eq!(stage.state.borrow().card_away, 0.0);
        assert!(stage.raise_chrome(false, card, true));
        wait_for_card_motion(&stage);
        expect_native_pixels(
            &window,
            &[
                (moved_page.x() + 100, moved_page.y() + 100, [255, 0, 0]),
                (moved_page.x() + 400, moved_page.y() + 100, [255, 0, 0]),
            ],
        );
        assert!(stage.state.borrow().chrome_overlay.is_none());
        assert!(stage.raise_chrome(true, card, true));
        assert!(stage.raise_chrome(false, card, false));
        assert!(stage.state.borrow().card_motion.is_none());
        assert!(stage.state.borrow().chrome_overlay.is_none());
        assert!(stage.apply(
            Some(Rect::new(240.0, 0.0, 720.0, 720.0)),
            Some(Pane::Leaf(sibling_id)),
            &[sibling_id],
        ));

        // The root can shrink before the shell publishes its smaller region.
        fixed.size_allocate(&gtk::Allocation::new(0, 0, 800, 600));
        assert!(!sibling.is_child_visible());
        assert!(!sibling.is_mapped());
        assert!(!sibling.is_sensitive());
        assert!(stage.apply(
            Some(Rect::new(240.0, 0.0, 560.0, 600.0)),
            Some(Pane::Leaf(sibling_id)),
            &[sibling_id],
        ));
        assert_eq!(sibling.allocation(), gtk::Allocation::new(240, 0, 560, 600));
        assert!(sibling.is_child_visible());
        assert!(sibling.is_mapped());
        assert!(sibling.is_sensitive());
        assert_eq!(sibling.opacity(), 1.0);

        assert!(stage.apply(
            Some(Rect::new(f64::NAN, 0.0, 560.0, 600.0)),
            Some(Pane::Leaf(sibling_id)),
            &[sibling_id],
        ));
        assert!(!sibling.is_mapped());
        assert!(!sibling.is_sensitive());

        assert!(stage.apply(None, None, &[]));
        window.show_all();
        while gtk::events_pending() {
            gtk::main_iteration_do(false);
        }
        assert!(!content.is_child_visible());
        assert!(!content.is_mapped());
        assert!(!content.is_sensitive());
        assert_eq!(content.opacity(), 0.0);
        assert!(!sibling.is_child_visible());
        assert!(!sibling.is_mapped());
        assert!(!sibling.is_sensitive());
        assert_eq!(sibling.opacity(), 0.0);
    }
}
