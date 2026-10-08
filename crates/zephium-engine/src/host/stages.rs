use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

#[cfg(any(target_os = "macos", target_os = "windows"))]
use raw_window_handle::RawWindowHandle;
#[cfg(target_os = "macos")]
use wry::WebView;

use zephium_core::geometry::Rect;
use zephium_core::ids::{ItemId, WindowId};
use zephium_core::ports::engine::{EngineEvent, StageMotion};
use zephium_core::split::Pane;

#[cfg(any(target_os = "macos", target_os = "windows"))]
use super::dispatch::seal_ingress;
#[cfg(target_os = "macos")]
use super::dispatch::try_with_stage_failure;
#[cfg(target_os = "windows")]
use super::dispatch::with_renderer_exit;
use super::EngineHost;
#[cfg(target_os = "macos")]
use super::ParentHandle;

#[cfg(target_os = "macos")]
use {
    crate::platform::imp::ContentStage, objc2::rc::Retained, objc2_app_kit::NSView,
    objc2_foundation::MainThreadMarker,
};

#[cfg(target_os = "windows")]
use {crate::platform::imp::Stage, windows::Win32::Foundation::HWND};

const GAP: f64 = 8.0;

fn should_seed_stage_readiness(
    inserted: bool,
    presentable: bool,
    presentation_permitted: bool,
) -> bool {
    // Existing stage children retain readiness across geometry-only layouts.
    // Re-seeding them would synchronously rerun a full GTK/macOS stage pass
    // once per visible split leaf during every resize. A newly attached child
    // alone needs to inherit an already-presentable document's retained fact.
    inserted && presentable && presentation_permitted
}

impl EngineHost {
    /// A latest-value layout can occupy an earlier main-loop queue position
    /// than a native view construction that was accepted later. Reconcile the
    /// newly-owned view against every stage's retained authoritative tree so
    /// it cannot remain absent from the exact layout until an unrelated future
    /// resize. On WebKitGTK this handoff also owns the first offscreen map.
    pub(super) fn finish_new_view_insertion(&mut self, id: ItemId, event_token: &Arc<AtomicBool>) {
        // A hidden page gets a throttled grace before `set_dormant` suspends it.
        #[cfg(target_os = "macos")]
        if let Some(view) = self.views.get(&id) {
            crate::platform::imp::set_background_suspension(&view.view, false);
        }
        let reconciled = self.reconcile_new_view_with_stages(id);

        // Stage attachment enters native UI code and may pump callbacks. A
        // close accepted during that re-entry owns the logical item now; tear
        // down this just-inserted generation without publishing a failure for
        // its already-retired token.
        if !event_token.load(Ordering::Acquire) {
            self.close(id);
            return;
        }
        #[cfg(target_os = "macos")]
        let extension_surface_bound = self
            .partitions
            .get(&id)
            .copied()
            .map(crate::host::Partition::profile)
            .map(|profile| self.bind_extension_browser_surface_view(profile, id))
            .unwrap_or(false);
        #[cfg(not(target_os = "macos"))]
        let extension_surface_bound = true;

        if reconciled && extension_surface_bound {
            return;
        }

        eprintln!("engine: native view could not catch up to retained browser routing state");
        self.close(id);
        self.sink
            .emit_for(event_token.clone(), EngineEvent::ViewCreationFailed { id });
    }

    #[cfg(target_os = "macos")]
    fn reconcile_new_view_with_stages(&self, id: ItemId) -> bool {
        let expected = self
            .stages
            .values()
            .filter(|stage| stage.contains_item(id))
            .cloned()
            .collect::<Vec<_>>();
        if expected.is_empty() {
            // Raw WKWebViews start hidden, so an unstaged background view is
            // already fail-closed until a later authoritative layout owns it.
            return true;
        }
        let Some(view) = self.views.get(&id) else {
            return false;
        };
        view.download_surface_intent.store(
            expected
                .iter()
                .any(|stage| stage.allows_download_decision(id)),
            Ordering::Release,
        );
        let Some(native_view) = webview_nsview(view) else {
            return false;
        };
        let presentable = view.presentable && view.presentation_permit.load(Ordering::Acquire);
        let presentation_permit = view.presentation_permit.clone();
        let mut reconciled = true;
        for stage in expected {
            if !stage.has_view(id) {
                stage.insert_view(id, native_view.clone(), presentation_permit.clone());
            }
            if !stage.has_view(id) {
                reconciled = false;
                continue;
            }
            if presentable && !stage.set_ready(id) {
                reconciled = false;
            }
        }
        reconciled
    }

    #[cfg(target_os = "windows")]
    fn reconcile_new_view_with_stages(&self, id: ItemId) -> bool {
        let expected = self
            .stages
            .values()
            .filter(|stage| stage.contains_item(id))
            .cloned()
            .collect::<Vec<_>>();
        if expected.is_empty() {
            // The controller and its child HWND were constructed hidden.
            return true;
        }
        let Some(view) = self.views.get(&id) else {
            return false;
        };
        view.download_surface_intent
            .store(!expected.is_empty(), Ordering::Release);
        let Some(generation) = view.event_permit.active_token() else {
            return false;
        };
        let presentable = view.presentable && view.presentation_permit.load(Ordering::Acquire);
        let presentation_permit = view.presentation_permit.clone();
        let mut reconciled = true;
        for stage in expected {
            if !stage.has_view(id) {
                stage.insert_view(id, view, generation.clone(), presentation_permit.clone());
            }
            if !stage.has_view(id) {
                reconciled = false;
                continue;
            }
            if presentable && !stage.set_ready(id) {
                reconciled = false;
            }
        }
        reconciled
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    fn reconcile_new_view_with_stages(&self, id: ItemId) -> bool {
        let expected = self
            .stages
            .values()
            .filter(|stage| stage.contains_item(id))
            .cloned()
            .collect::<Vec<_>>();
        let Some(view) = self.views.get(&id) else {
            return false;
        };
        if expected.is_empty() {
            // Guarded WebKitGTK construction stays unmapped. Preserve that
            // fail-closed state when the retained layout has already moved on;
            // a later stage insertion performs the first offscreen map.
            crate::platform::imp::Stage::exclude_unstaged(view);
            return true;
        }
        let presentable = view.presentable && view.presentation_permit.load(Ordering::Acquire);
        let presentation_permit = view.presentation_permit.clone();
        let mut reconciled = true;
        for stage in expected {
            if !stage.has_view(id) {
                stage.insert_view(id, view, presentation_permit.clone());
            }
            if !stage.has_view(id) {
                reconciled = false;
                continue;
            }
            if presentable && !stage.set_ready(id) {
                reconciled = false;
            }
        }
        if !reconciled {
            crate::platform::imp::Stage::exclude_unstaged(view);
        }
        reconciled
    }

    pub(crate) fn set_content(
        &mut self,
        window: WindowId,
        tree: Option<Pane>,
        region: Option<Rect>,
        motion: Option<StageMotion>,
    ) -> bool {
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        let shown = match region {
            Some(_) => tree.as_ref().map(Pane::tabs).unwrap_or_default(),
            None => Vec::new(),
        };
        let applied = self.apply_content(window, tree, region, motion);
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        self.fullscreen_layout_applied(window, &shown);
        applied
    }

    #[cfg(target_os = "macos")]
    fn apply_content(
        &mut self,
        window: WindowId,
        tree: Option<Pane>,
        region: Option<Rect>,
        motion: Option<StageMotion>,
    ) -> bool {
        let Some(stage) = self.ensure_stage(window) else {
            return false;
        };
        // Reserve this layout before the first AppKit call. Any nested newer
        // layout invalidates `update_epoch`, so this outer stack frame can no
        // longer re-show an obsolete stage container when it resumes.
        let Some(update_epoch) = stage.begin_content_update(region.is_some()) else {
            return false;
        };
        let next_tabs = tree.as_ref().map(Pane::tabs).unwrap_or_default();
        // Revoke every old source before cancelling a panel can reenter AppKit.
        for (id, view) in &self.views {
            if stage.has_view(*id) || next_tabs.contains(id) {
                view.download_surface_intent.store(
                    region.is_some() && next_tabs.contains(id),
                    Ordering::Release,
                );
            }
        }
        for (id, view) in &self.views {
            if stage.has_view(*id) && (region.is_none() || !next_tabs.contains(id)) {
                view.file_uploads.cancel();
            }
        }
        if !stage.content_update_is_current(update_epoch) {
            return !stage.has_terminal_failure();
        }
        let Some(r) = region else {
            let applied = stage.finish_content_update(update_epoch);
            if applied {
                self.refresh_visible_generic_styles();
            }
            return applied;
        };
        if !stage_set_frame(&stage, &self.parent, r, motion) {
            stage.abort_content_update(update_epoch);
            return false;
        }
        if !stage.content_update_is_current(update_epoch) {
            return !stage.has_terminal_failure();
        }
        let tabs = tree.as_ref().map(Pane::tabs).unwrap_or_default();
        let mut invalid_views = Vec::new();
        for id in &tabs {
            let Some(view) = self.views.get(id) else {
                // The lifecycle gate already proved this id has a live
                // reservation. A coalesced layout may overtake its later
                // native construction task, so retain the tree with this
                // leaf concealed. `finish_new_view_insertion` attaches the
                // exact generation and catches it up to this layout.
                continue;
            };
            let mut inserted = false;
            if !stage.has_view(*id) {
                if let Some(native_view) = webview_nsview(view) {
                    if !stage.insert_view(*id, native_view, view.presentation_permit.clone()) {
                        stage.abort_content_update(update_epoch);
                        return false;
                    }
                    inserted = true;
                } else {
                    invalid_views.push(*id);
                }
            }
            if !stage.content_update_is_current(update_epoch) {
                return !stage.has_terminal_failure();
            }
            if should_seed_stage_readiness(
                inserted,
                view.presentable,
                view.presentation_permit.load(Ordering::Acquire),
            ) && !stage.set_ready(*id)
            {
                stage.abort_content_update(update_epoch);
                return false;
            }
            if !stage.content_update_is_current(update_epoch) {
                return !stage.has_terminal_failure();
            }
        }
        if !invalid_views.is_empty() {
            stage.abort_content_update(update_epoch);
            for id in invalid_views {
                let token = self
                    .views
                    .get(&id)
                    .and_then(|view| view.event_permit.active_token());
                self.close(id);
                if let Some(token) = token {
                    self.sink
                        .emit_for(token, EngineEvent::ViewCreationFailed { id });
                }
            }
            return !stage.has_terminal_failure();
        }
        if !stage.set_tree(tree) {
            stage.abort_content_update(update_epoch);
            return false;
        }
        if !stage.content_update_is_current(update_epoch) {
            return !stage.has_terminal_failure();
        }
        if !stage.set_visible(&tabs) {
            stage.abort_content_update(update_epoch);
            return false;
        }
        if !stage.content_update_is_current(update_epoch) {
            return !stage.has_terminal_failure();
        }
        let applied = stage.finish_content_update(update_epoch);
        if applied {
            self.refresh_visible_generic_styles();
        }
        applied
    }

    #[cfg(target_os = "macos")]
    pub(crate) fn set_drop_indicator(&mut self, window: WindowId, zone: Option<Rect>) {
        if let Some(stage) = self.ensure_stage(window) {
            stage.set_drop_indicator(zone);
        }
    }

    #[cfg(not(target_os = "macos"))]
    fn apply_content(
        &mut self,
        window: WindowId,
        tree: Option<Pane>,
        region: Option<Rect>,
        _motion: Option<StageMotion>,
    ) -> bool {
        let Some(stage) = self.ensure_stage(window) else {
            return false;
        };
        let tabs = match region {
            Some(_) => tree.as_ref().map(Pane::tabs).unwrap_or_default(),
            None => Vec::new(),
        };
        #[cfg(target_os = "windows")]
        for (id, view) in &self.views {
            if stage.has_view(*id) || tabs.contains(id) {
                view.download_surface_intent
                    .store(region.is_some() && tabs.contains(id), Ordering::Release);
            }
        }
        for id in &tabs {
            let Some(view) = self.views.get(id) else {
                // The outer lifecycle gate distinguishes an inactive item
                // from a live reservation whose native construction has not
                // run yet. Retain the authoritative tree; insertion will
                // attach and synchronize that exact generation.
                continue;
            };
            let mut inserted = false;
            if !stage.has_view(*id) {
                #[cfg(target_os = "windows")]
                {
                    let Some(generation) = view.event_permit.active_token() else {
                        return false;
                    };
                    if !stage.insert_view(*id, view, generation, view.presentation_permit.clone()) {
                        return false;
                    }
                    inserted = true;
                }
                #[cfg(all(unix, not(target_os = "macos")))]
                if !stage.insert_view(*id, view, view.presentation_permit.clone()) {
                    return false;
                } else {
                    inserted = true;
                }
            }
            if should_seed_stage_readiness(
                inserted,
                view.presentable,
                view.presentation_permit.load(Ordering::Acquire),
            ) && !stage.set_ready(*id)
            {
                return false;
            }
        }
        // one native pass: frame, tree and visibility land atomically, so a
        // switch can never flash the previous pane
        #[cfg(target_os = "windows")]
        if let Some(motion) = _motion {
            stage.hint_motion(motion);
        }
        if !stage.apply(region, tree, &tabs) {
            return false;
        }
        // Off-screen views drop to the low-memory hint (reversible, nothing
        // freezes); actual suspension waits for the shell's idle verdict.
        // Becoming visible resumes a suspended view natively.
        #[cfg(target_os = "windows")]
        {
            use wry::{MemoryUsageLevel, WebViewExtWindows};
            let mut woken = Vec::new();
            for (id, view) in &self.views {
                // A layout belongs to one window; resource policy belongs to
                // the whole host. Updating window A must not mark a visible
                // view in window B hidden or make it eligible for suspension.
                let off = !self.stages.values().any(|stage| stage.wants_visible(*id));
                if off == self.hidden.contains(id) {
                    continue;
                }
                if off {
                    self.hidden.insert(*id);
                    let _ = view.set_memory_usage_level(MemoryUsageLevel::Low);
                } else {
                    self.hidden.remove(id);
                    self.dormant.remove(id);
                    self.desired_dormant.remove(id);
                    // Keep the in-flight slot until its exact callback settles.
                    // A rapid show/hide must not issue overlapping TrySuspend
                    // operations against the same native view.
                    self.suspend_failed.remove(id);
                    let _ = view.set_memory_usage_level(MemoryUsageLevel::Normal);
                    woken.push(*id);
                }
            }
            for id in woken {
                self.cancel_suspend_guard(id);
                self.refresh_missed_styles(id);
            }
        }
        self.refresh_visible_generic_styles();
        true
    }

    #[cfg(not(target_os = "macos"))]
    pub(crate) fn set_drop_indicator(&mut self, window: WindowId, zone: Option<Rect>) {
        if let Some(stage) = self.ensure_stage(window) {
            stage.set_drop_indicator(zone);
        }
    }

    #[cfg(target_os = "windows")]
    pub(crate) fn set_resize_guide(&mut self, window: WindowId, zone: Option<Rect>) {
        if let Some(stage) = self.ensure_stage(window) {
            stage.set_resize_guide(zone);
        }
    }

    #[cfg(target_os = "windows")]
    fn ensure_stage(&mut self, window: WindowId) -> Option<Stage> {
        if let Some(stage) = self.stages.get(&window) {
            return Some(stage.clone());
        }
        let RawWindowHandle::Win32(h) = self.parent.0 else {
            return None;
        };
        let parent = HWND(h.hwnd.get() as *mut _);
        let native_terminal_failure = self.native_terminal_failure.clone();
        let stage = Stage::new(parent, GAP, move |id, generation| {
            let admitted = with_renderer_exit(id, move |host| {
                host.on_stage_placement_failure(id, &generation)
            });
            if !admitted {
                seal_ingress();
                native_terminal_failure(
                    "terminal Windows stage failure was not admitted by the engine host",
                );
            }
        });
        self.stages.insert(window, stage.clone());
        Some(stage)
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    fn ensure_stage(&mut self, window: WindowId) -> Option<crate::platform::imp::Stage> {
        if let Some(stage) = self.stages.get(&window) {
            return Some(stage.clone());
        }
        let fixed = crate::platform::imp::container()?;
        let stage = crate::platform::imp::Stage::new(fixed, GAP);
        self.stages.insert(window, stage.clone());
        Some(stage)
    }

    #[cfg(target_os = "macos")]
    fn on_macos_stage_failure(&mut self, window: WindowId, failed_identity: usize) {
        let Some(stage) = self.stages.get(&window) else {
            return;
        };
        if Retained::as_ptr(stage) as usize != failed_identity {
            return;
        }
        // Prove the full attached-id set while the exact failed stage remains
        // mapped. A temporary RefCell conflict must never be canonicalized to
        // an empty set and then retire native ownership without its views.
        let Some(ids) = stage.attached_items() else {
            seal_ingress();
            (self.native_terminal_failure)(
                "terminal macOS stage could not prove its attached native views",
            );
            return;
        };
        let Some(stage) = self.stages.remove(&window) else {
            return;
        };
        if Retained::as_ptr(&stage) as usize != failed_identity {
            // No native call occurs between identity proof and removal, but
            // keep this invariant fail-closed if that ever changes.
            self.stages.insert(window, stage);
            seal_ingress();
            (self.native_terminal_failure)(
                "terminal macOS stage identity changed during retirement",
            );
            return;
        }
        stage.retire();
        for id in ids {
            let token = self
                .views
                .get(&id)
                .and_then(|view| view.event_permit.active_token());
            self.close(id);
            if let Some(token) = token {
                self.sink
                    .emit_for(token, EngineEvent::ViewCreationFailed { id });
            }
        }
    }

    #[cfg(target_os = "macos")]
    pub(super) fn ensure_stage(&mut self, window: WindowId) -> Option<Retained<ContentStage>> {
        if let Some(stage) = self.stages.get(&window) {
            return Some(stage.clone());
        }
        let mtm = MainThreadMarker::new()?;
        let content = content_view(&self.parent)?;
        let native_terminal_failure = self.native_terminal_failure.clone();
        let stage = ContentStage::new(
            mtm,
            GAP,
            Box::new(move |failed_identity| {
                let admitted = try_with_stage_failure(move |host| {
                    host.on_macos_stage_failure(window, failed_identity)
                });
                if !admitted {
                    seal_ingress();
                    native_terminal_failure(
                        "terminal macOS stage failure was not admitted by the engine host",
                    );
                }
            }),
        );
        use objc2_app_kit::NSAutoresizingMaskOptions as Mask;
        stage.setAutoresizingMask(Mask::ViewWidthSizable | Mask::ViewHeightSizable);
        let sink = self.sink.clone();
        stage.set_on_ratio(Box::new(move |tree| {
            sink.emit(EngineEvent::SplitChanged { window, tree })
        }));
        content.addSubview(&stage);
        self.stages.insert(window, stage.clone());
        Some(stage)
    }
}

#[cfg(target_os = "macos")]
fn content_view(parent: &ParentHandle) -> Option<Retained<NSView>> {
    if let RawWindowHandle::AppKit(h) = parent.0 {
        return unsafe { Retained::retain(h.ns_view.as_ptr() as *mut NSView) };
    }
    None
}

#[cfg(target_os = "macos")]
fn webview_nsview(view: &WebView) -> Option<Retained<NSView>> {
    use wry::WebViewExtMacOS;
    let wk = view.webview();
    unsafe { Retained::retain(Retained::as_ptr(&wk) as *mut NSView) }
}

#[cfg(target_os = "macos")]
fn stage_set_frame(
    stage: &ContentStage,
    parent: &ParentHandle,
    r: Rect,
    motion: Option<StageMotion>,
) -> bool {
    use objc2_foundation::{NSPoint, NSRect, NSSize};

    let Some(content) = content_view(parent) else {
        return false;
    };
    let h = content.bounds().size.height;
    let frame = NSRect::new(
        NSPoint::new(r.x, h - r.y - r.height),
        NSSize::new(r.width, r.height),
    );
    if motion == Some(StageMotion::Slide) && stage.can_slide_to(frame) {
        stage.slide_to(frame);
        return true;
    }
    let same = |a: NSRect| {
        a.origin.x == frame.origin.x
            && a.origin.y == frame.origin.y
            && a.size.width == frame.size.width
            && a.size.height == frame.size.height
    };
    // A layout that only restates where a slide is already going lets it
    // finish; anything else ends it where it is and takes over.
    if !same(stage.motion_target()) {
        stage.settle_motion();
        if !same(stage.frame()) {
            stage.setFrame(frame);
        }
    }
    if motion == Some(StageMotion::Arrive) {
        stage.arrive();
    }
    true
}

#[cfg(test)]
mod tests;
