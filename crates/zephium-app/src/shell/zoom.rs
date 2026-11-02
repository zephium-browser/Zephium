//! Page zoom: remembered per site and applied to every tab of that site, set
//! through generation-correlated native requests that settle exactly.

use super::*;
use zephium_core::zoom::{self, DEFAULT_PERCENT, MAX_SITE_LEVELS};
use zephium_ipc::{ZoomCall, ZoomError, ZoomLevelView, ZoomResponse};

/// Whose wish a native request carries out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ZoomOrigin {
    /// The person zoomed this tab, so its site remembers where it settles.
    Person,
    /// The tab is following what its site already remembers.
    Site,
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct PendingZoom {
    request: ZoomRequestId,
    pub(super) desired_scale: f64,
    origin: ZoomOrigin,
    /// The site a person's change belongs to, fixed when it was asked for so
    /// a navigation in between cannot move it to another site.
    owner: Option<(ProfileId, String)>,
}

#[derive(Clone, Copy)]
pub(super) struct Level {
    pub(super) percent: u16,
    set_at: i64,
}

pub(super) struct ZoomState {
    pub(super) next_request: u64,
    pub(super) pending: std::collections::HashMap<ItemId, PendingZoom>,
    pub(super) default_percent: u16,
    /// Levels by profile, read from the store on first use. A private
    /// profile's stay here alone and go with it.
    pub(super) saved:
        std::collections::HashMap<ProfileId, std::collections::HashMap<String, Level>>,
}

impl Default for ZoomState {
    fn default() -> Self {
        Self {
            next_request: 0,
            pending: std::collections::HashMap::new(),
            default_percent: DEFAULT_PERCENT,
            saved: std::collections::HashMap::new(),
        }
    }
}

impl ZoomState {
    pub(super) fn load(store: &dyn zephium_core::ports::store::Store) -> Self {
        let mut state = Self::default();
        if let Some(percent) = store
            .app_setting("zoom.default")
            .and_then(|value| default_percent(&value))
        {
            state.default_percent = percent;
        }
        state
    }

    /// Drops a profile's levels with the profile, as a private session's go
    /// when it ends.
    pub(super) fn forget_profile(&mut self, profile: ProfileId) {
        self.saved.remove(&profile);
    }
}

#[derive(Clone, Copy, Debug)]
pub(super) enum ZoomStep {
    In,
    Out,
    Reset,
}

fn default_percent(value: &str) -> Option<u16> {
    zephium_core::preferences::value_allowed("zoom.default", value)
        .then(|| value.parse().ok())
        .flatten()
}

/// Native scales agree to within what rounding to a whole percent loses.
fn same_scale(a: f64, b: f64) -> bool {
    (a - b).abs() < 0.005
}

impl Shell {
    fn site_levels(&mut self, profile: ProfileId) -> &mut std::collections::HashMap<String, Level> {
        if !self.zoom.saved.contains_key(&profile) {
            let loaded = if self.incognito_profile(profile) {
                Vec::new()
            } else {
                self.store.zoom_levels(profile)
            };
            let levels = loaded
                .into_iter()
                .filter(|level| zoom::valid_percent(level.percent))
                .map(|level| {
                    (
                        level.site,
                        Level {
                            percent: level.percent,
                            set_at: level.set_at,
                        },
                    )
                })
                .collect();
            self.zoom.saved.insert(profile, levels);
        }
        self.zoom.saved.entry(profile).or_default()
    }

    /// The percent a site shows at: its own level, else the default.
    pub(super) fn zoom_percent_for(&mut self, profile: ProfileId, site: Option<&str>) -> u16 {
        let default = self.zoom.default_percent;
        site.and_then(|site| {
            self.site_levels(profile)
                .get(site)
                .map(|level| level.percent)
        })
        .unwrap_or(default)
    }

    fn zoom_owner(&self, id: ItemId) -> Option<(ProfileId, String)> {
        let url = self.items.tab(id)?.url.as_ref()?;
        Some((self.profile_of_item(id)?, super::time::site_of(url)?))
    }

    /// What a tab about to load `url` should be zoomed to.
    pub(super) fn new_view_zoom(&mut self, id: ItemId, url: &str) -> f64 {
        let site = url::Url::parse(url)
            .ok()
            .as_ref()
            .and_then(super::time::site_of);
        let percent = match self.profile_of_item(id) {
            Some(profile) => self.zoom_percent_for(profile, site.as_deref()),
            None => self.zoom.default_percent,
        };
        zoom::scale(percent)
    }

    pub(super) fn request_zoom(
        &mut self,
        id: ItemId,
        desired_scale: f64,
        origin: ZoomOrigin,
    ) -> NativeDispatch {
        let Some(next) = self.zoom.next_request.checked_add(1) else {
            // Reusing an identity could let a late result settle a newer
            // request. Saturation permanently rejects new zoom work.
            return NativeDispatch::Rejected;
        };
        self.zoom.next_request = next;
        let request = ZoomRequestId(next);
        let admission = self.engine.zoom(id, desired_scale, request);
        if admission == NativeDispatch::Scheduled {
            let owner = (origin == ZoomOrigin::Person)
                .then(|| self.zoom_owner(id))
                .flatten();
            self.zoom.pending.insert(
                id,
                PendingZoom {
                    request,
                    desired_scale,
                    origin,
                    owner,
                },
            );
            self.project_tab(id);
        }
        admission
    }

    /// Brings a tab to what its site is meant to show: after it commits
    /// another document, and whenever a level or the default changes.
    pub(super) fn reconcile_zoom(&mut self, id: ItemId) {
        let Some(tab) = self.items.tab(id) else {
            return;
        };
        if tab.content != zephium_core::item::TabContent::Web || !tab.has_view() {
            return;
        }
        let settled = tab.zoom;
        let site = tab.url.as_ref().and_then(super::time::site_of);
        let Some(profile) = self.profile_of_item(id) else {
            return;
        };
        let wanted = zoom::scale(self.zoom_percent_for(profile, site.as_deref()));
        match self.zoom.pending.get(&id) {
            // A person's change in flight is not fought; once it settles, the
            // site follows it and this tab is reconciled again.
            Some(pending) if pending.origin == ZoomOrigin::Person => return,
            Some(pending) if same_scale(pending.desired_scale, wanted) => return,
            None if same_scale(settled, wanted) => return,
            _ => {}
        }
        let _ = self.request_zoom(id, wanted, ZoomOrigin::Site);
    }

    fn reconcile_zoom_where(&mut self, mut matches: impl FnMut(&Self, ItemId) -> bool) {
        for id in self.items.view_ids() {
            if matches(self, id) {
                self.reconcile_zoom(id);
            }
        }
    }

    pub(super) fn reconcile_all_zoom(&mut self) {
        self.reconcile_zoom_where(|_, _| true);
    }

    fn reconcile_site_zoom(&mut self, profile: ProfileId, site: &str) {
        self.reconcile_zoom_where(|shell, id| {
            shell
                .zoom_owner(id)
                .is_some_and(|(owner, tab_site)| owner == profile && tab_site == site)
        });
    }

    /// Sets a site's level, or forgets it when it is the default, and brings
    /// the site's other tabs to it.
    fn remember_zoom(&mut self, profile: ProfileId, site: String, applied: f64) {
        let percent = zoom::percent(applied);
        let saved = (percent != self.zoom.default_percent).then_some(percent);
        let now = chrono::Utc::now().timestamp();
        let levels = self.site_levels(profile);
        if levels.get(&site).map(|level| level.percent) == saved {
            return;
        }
        match saved {
            Some(percent) => {
                levels.insert(
                    site.clone(),
                    Level {
                        percent,
                        set_at: now,
                    },
                );
                while levels.len() > MAX_SITE_LEVELS {
                    let Some(oldest) = levels
                        .iter()
                        .min_by(|a, b| (a.1.set_at, a.0).cmp(&(b.1.set_at, b.0)))
                        .map(|(site, _)| site.clone())
                    else {
                        break;
                    };
                    levels.remove(&oldest);
                }
            }
            None => {
                levels.remove(&site);
            }
        }
        if !self.incognito_profile(profile) {
            let _ = self
                .store
                .save_zoom_level(profile, site.clone(), saved, now);
        }
        self.reconcile_site_zoom(profile, &site);
    }

    pub(super) fn on_zoom_settled(
        &mut self,
        id: ItemId,
        request: ZoomRequestId,
        applied_scale: f64,
        succeeded: bool,
    ) {
        let Some(pending) = self.zoom.pending.get(&id).cloned() else {
            return;
        };
        if pending.request != request {
            // A newest-per-item native settlement contains the cumulative
            // applied scale. Older results cannot settle a newer desired
            // value and are deliberately ignored.
            return;
        }
        // This exact terminal result owns the obligation even when its native
        // payload is malformed. Retire it before validation so a corrupt or
        // incompatible engine response cannot leave all future zoom input
        // based on a value that will never settle.
        self.zoom.pending.remove(&id);
        if !applied_scale.is_finite() || !(0.3..=3.0).contains(&applied_scale) {
            crate::diagnostic!("engine: rejected malformed native zoom settlement");
            self.project_tab(id);
            return;
        }
        if !succeeded {
            crate::diagnostic!("engine: native zoom request was not applied");
        } else if (applied_scale - pending.desired_scale).abs() > f64::EPSILON {
            crate::diagnostic!("engine: native zoom settled at an unexpected scale");
        }
        let Some(previous) = self.items.tab(id).map(|tab| tab.zoom) else {
            return;
        };
        if (previous - applied_scale).abs() > f64::EPSILON {
            self.items.set_zoom(id, applied_scale);
            self.schedule_persist();
        }
        self.project_tab(id);
        // A request that was not applied as asked changes nothing else, and
        // is not retried here: that could repeat without end.
        if !succeeded || !same_scale(applied_scale, pending.desired_scale) {
            return;
        }
        if let (ZoomOrigin::Person, Some((profile, site))) = (pending.origin, pending.owner) {
            self.remember_zoom(profile, site, applied_scale);
        }
        // Whatever changed while this was in flight, such as a navigation to
        // another site, is caught up with now.
        self.reconcile_zoom(id);
    }

    /// A zoom the person made in the page itself, such as with Ctrl and the
    /// mouse wheel, which the view already shows. The site remembers it like
    /// one made with the keys.
    pub(super) fn on_zoom_gesture(&mut self, id: ItemId, applied_scale: f64) {
        if !applied_scale.is_finite() || !(0.3..=3.0).contains(&applied_scale) {
            crate::diagnostic!("engine: rejected malformed native zoom gesture");
            return;
        }
        if self.items.tab(id).is_none_or(|tab| !tab.has_view()) {
            return;
        }
        // What the person did supersedes anything still being asked of the view.
        self.zoom.pending.remove(&id);
        if self
            .items
            .tab(id)
            .is_some_and(|tab| tab.zoom != applied_scale)
        {
            self.items.set_zoom(id, applied_scale);
            self.schedule_persist();
        }
        self.project_tab(id);
        if let Some((profile, site)) = self.zoom_owner(id) {
            self.remember_zoom(profile, site, applied_scale);
        }
        // A level outside the range the page may hold is brought inside it.
        self.reconcile_zoom(id);
    }

    /// The percent chrome shows for a tab with a page: the level just asked
    /// for while a request is in flight, so the glass answers the key.
    pub(super) fn zoom_view(&self, id: ItemId, tab: &TabState) -> Option<u16> {
        (tab.content == zephium_core::item::TabContent::Web && tab.has_view()).then(|| {
            zoom::percent(
                self.zoom
                    .pending
                    .get(&id)
                    .map_or(tab.zoom, |pending| pending.desired_scale),
            )
        })
    }

    pub(super) fn operation_adjust_zoom(&mut self, step: ZoomStep) -> OperationDisposition {
        let Some(active) = self.windows.focused().and_then(|window| window.active) else {
            return operation_result(OperationOutcome::NoOp, OperationReason::NoFocusedWindow);
        };
        let Some(settled) = self.items.tab(active).map(|tab| tab.zoom) else {
            return operation_result(OperationOutcome::Rejected, OperationReason::InvalidScope);
        };
        let current = self
            .zoom
            .pending
            .get(&active)
            .map_or(settled, |pending| pending.desired_scale);
        let percent = match step {
            ZoomStep::In => zoom::step(zoom::percent(current), true),
            ZoomStep::Out => zoom::step(zoom::percent(current), false),
            ZoomStep::Reset => self.zoom.default_percent,
        };
        let target = zoom::scale(percent);
        if same_scale(target, current) {
            return operation_result(OperationOutcome::NoOp, OperationReason::StateUnchanged);
        }
        // `Items.zoom` is durable authoritative state, and a site's level
        // follows what a view proved it applied. Keep the responsive desired
        // value in a separate bounded map until the exact native view
        // generation reports what it actually applied; otherwise any
        // unrelated session save could persist an optimistic lie.
        let admission = self.request_zoom(active, target, ZoomOrigin::Person);
        let mut native = NativeWork::default();
        native.record(admission);
        mutation_result(native)
    }

    pub(super) fn apply_zoom_setting(&mut self, key: &str, value: &str) {
        if key != "zoom.default" {
            return;
        }
        let Some(percent) = default_percent(value) else {
            return;
        };
        self.zoom.default_percent = percent;
        self.reconcile_all_zoom();
    }

    /// Settings' view of the focused profile's levels, by site.
    pub(super) fn zoom_call(
        &mut self,
        expected_profile: ProfileId,
        call: ZoomCall,
        done: crate::api::ZoomCompletion,
    ) {
        let authorized = self
            .windows
            .focused()
            .is_some_and(|window| window.profile == expected_profile);
        if !authorized || !call.validate() {
            done.finish(ZoomResponse::Error {
                error: ZoomError::Invalid,
            });
            return;
        }
        match call {
            ZoomCall::Levels => {}
            ZoomCall::Forget { site } => {
                self.forget_zoom_level(expected_profile, &site);
            }
            ZoomCall::ForgetAll => {
                let sites: Vec<String> =
                    self.site_levels(expected_profile).keys().cloned().collect();
                for site in sites {
                    self.forget_zoom_level(expected_profile, &site);
                }
            }
        }
        let mut levels: Vec<ZoomLevelView> = self
            .site_levels(expected_profile)
            .iter()
            .map(|(site, level)| ZoomLevelView {
                site: site.clone(),
                percent: level.percent,
            })
            .collect();
        levels.sort_by(|a, b| a.site.cmp(&b.site));
        done.finish(ZoomResponse::Levels { levels });
    }

    /// Forgets a site's saved level, so its tabs show the default.
    pub(super) fn forget_zoom_level(&mut self, profile: ProfileId, site: &str) -> bool {
        if self.site_levels(profile).remove(site).is_none() {
            return false;
        }
        if !self.incognito_profile(profile) {
            let _ = self
                .store
                .save_zoom_level(profile, site.to_owned(), None, 0);
        }
        self.reconcile_site_zoom(profile, site);
        true
    }

    /// Drops the levels set from `since` (Unix seconds) on, or all of them.
    /// The store has dropped the same rows with the history they belong to.
    pub(super) fn clear_zoom_levels(&mut self, profile: ProfileId, since: Option<i64>) {
        match since {
            None => {
                self.zoom
                    .saved
                    .insert(profile, std::collections::HashMap::new());
            }
            Some(since) => self
                .site_levels(profile)
                .retain(|_, level| level.set_at < since),
        }
        self.reconcile_zoom_where(|shell, id| shell.profile_of_item(id) == Some(profile));
    }
}
