//! Element fullscreen of ordinary tabs: native transitions come in through
//! the ledger, the shell hears one owner at a time, and every layout that
//! stops showing the owner asks it to leave.

use zephium_core::ids::{ItemId, WindowId};
use zephium_core::ports::engine::EngineEvent;

use super::permits::EventPermit;
use super::EngineHost;
use crate::fullscreen::FullscreenEffect;
#[cfg(target_os = "macos")]
use crate::fullscreen::NativeFullscreen;

/// How long a closed tab may wait for WebKit to bring its page home from the
/// fullscreen window before it is torn down regardless.
#[cfg(target_os = "macos")]
const RETIRE_DEADLINE: std::time::Duration = std::time::Duration::from_secs(1);

/// A closed tab whose page WebKit was still showing fullscreen. It is out of
/// every host map; only its native teardown waits.
#[cfg(target_os = "macos")]
pub(super) struct RetiringView {
    _view: super::ObservedView,
    _deadline: Option<crate::platform::imp::ContentPolicyTimeout>,
}

impl EngineHost {
    /// A native fullscreen callback for one exact view generation. The state
    /// is read again here, so a coalesced callback still lands the latest.
    pub(super) fn native_fullscreen_changed(&mut self, id: ItemId, permit: &EventPermit) {
        let Some(view) = self
            .views
            .get(&id)
            .filter(|view| view.event_permit.same_generation(permit))
        else {
            return;
        };
        let state = crate::platform::imp::fullscreen_state(&view.view);
        #[cfg(target_os = "macos")]
        if state == NativeFullscreen::Inactive {
            crate::platform::imp::set_background_suspension(&view.view, self.dormant.contains(&id));
            for stage in self.stages.values() {
                if stage.has_view(id) {
                    let _ = stage.adopt_after_fullscreen(id);
                }
            }
        }
        let effects = self.fullscreen.observe(id, state);
        #[cfg(target_os = "windows")]
        self.sync_stage_fullscreen();
        self.apply_fullscreen_effects(effects);
    }

    pub(crate) fn exit_fullscreen(&mut self, id: ItemId) {
        let effects = self.fullscreen.exit(id);
        self.apply_fullscreen_effects(effects);
    }

    /// The owner leaves fullscreen once `window`'s applied layout stops
    /// showing it: a tab switch, a browser page, a hidden window.
    pub(super) fn fullscreen_layout_applied(&mut self, window: WindowId, shown: &[ItemId]) {
        let Some(stage) = self.stages.get(&window) else {
            return;
        };
        let effects = self.fullscreen.layout(|id| stage.has_view(id), shown);
        self.apply_fullscreen_effects(effects);
    }

    pub(super) fn fullscreen_document_committed(&mut self, id: ItemId) {
        let effects = self.fullscreen.committed(id);
        self.apply_fullscreen_effects(effects);
    }

    pub(super) fn forget_fullscreen(&mut self, id: ItemId) {
        self.fullscreen.forget(id);
        #[cfg(target_os = "windows")]
        self.sync_stage_fullscreen();
    }

    fn apply_fullscreen_effects(&mut self, effects: Vec<FullscreenEffect>) {
        for effect in effects {
            match effect {
                FullscreenEffect::Report { id, active } => {
                    if let Some(view) = self.views.get(&id) {
                        view.event_permit
                            .emit(&self.sink, EngineEvent::FullscreenChanged { id, active });
                    }
                }
                FullscreenEffect::Exit(id) => {
                    if let Some(view) = self.views.get(&id) {
                        crate::platform::imp::exit_fullscreen(&view.view);
                    }
                }
            }
        }
    }

    #[cfg(target_os = "windows")]
    fn sync_stage_fullscreen(&self) {
        let owner = self.fullscreen.owner();
        for stage in self.stages.values() {
            stage.set_fullscreen(owner.filter(|id| stage.has_view(*id)));
        }
    }

    /// Lets WebKit bring a closed tab's page home from its fullscreen window
    /// before the view is destroyed, so the fullscreen Space closes with its
    /// usual animation instead of being torn out from under WebKit.
    #[cfg(target_os = "macos")]
    pub(super) fn retire_fullscreen_view(&mut self, view: super::ObservedView) {
        view.event_permit.revoke();
        view.navigation.revoke();
        self.next_fullscreen_retirement = self.next_fullscreen_retirement.wrapping_add(1);
        let token = self.next_fullscreen_retirement;
        let finish = move || {
            let _ = super::dispatch::try_with(move |host| {
                host.fullscreen_retiring.remove(&token);
            });
        };
        crate::platform::macos::fullscreen::close_presentations(&view.view, finish);
        let deadline =
            crate::platform::imp::schedule_presentation_timeout(RETIRE_DEADLINE, move || {
                let _ = super::dispatch::try_with(move |host| {
                    host.fullscreen_retiring.remove(&token);
                });
            });
        if deadline.is_none() {
            // Without a deadline the view could outlive its tab indefinitely.
            drop(view);
            return;
        }
        self.fullscreen_retiring.insert(
            token,
            RetiringView {
                _view: view,
                _deadline: deadline,
            },
        );
    }
}
