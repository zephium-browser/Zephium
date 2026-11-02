//! Shell ownership of effective extension toolbar actions.

use std::collections::HashMap;

use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use zephium_core::extensions::{
    ExtensionActionRejection, ExtensionActionRequest, ExtensionActionRequestId,
    ExtensionActionRevision, ExtensionActionSettlement, ExtensionActionSnapshot,
    ExtensionActionSnapshotSettlement, ExtensionBrowserSurface, ExtensionBrowserSurfaceGeneration,
    ExtensionPopupAnchor, ExtensionRuntimeInstance,
};
use zephium_core::ids::{ItemId, ProfileId};
use zephium_core::ports::engine::NativeDispatch;
use zephium_ipc::{ExtensionActionRuntimeView, ExtensionActionView};

use super::{NativeWork, Shell};

const MAX_PENDING_EXTENSION_ACTIONS: usize = 8;

pub(super) struct ExtensionActionState {
    snapshots: HashMap<ProfileId, ExtensionActionSnapshot>,
    // Also acts as a bounded pending-settlement watchdog. A scheduled read
    // remains here until an exact applied result arrives, so the ordinary
    // maintenance tick repairs a dropped callback without a hot timer.
    retry_profiles: zephium_core::extensions::ExtensionActiveProfiles,
    pending: HashMap<ExtensionActionRequestId, ExtensionActionRequest>,
    next_request_id: Option<u64>,
}

impl Default for ExtensionActionState {
    fn default() -> Self {
        Self {
            snapshots: HashMap::new(),
            retry_profiles: zephium_core::extensions::ExtensionActiveProfiles::EMPTY,
            pending: HashMap::new(),
            next_request_id: Some(1),
        }
    }
}

impl ExtensionActionState {
    pub(super) fn retire_profile(&mut self, profile: ProfileId) {
        self.snapshots.remove(&profile);
        self.retry_profiles.remove(profile);
        self.pending
            .retain(|_, request| request.runtime().profile() != profile);
    }

    #[cfg(test)]
    pub(super) fn snapshot(&self, profile: ProfileId) -> Option<&ExtensionActionSnapshot> {
        self.snapshots.get(&profile)
    }

    pub(super) fn projected_actions(
        &self,
        profile: ProfileId,
        tab: ItemId,
        surface_generation: ExtensionBrowserSurfaceGeneration,
    ) -> Vec<ExtensionActionView> {
        // A newer surface generation (a title or load-state change) does not
        // change the tab's actions; keep them visible until the fresh
        // snapshot replaces them rather than blanking the toolbar.
        let _ = surface_generation;
        let Some(snapshot) = self
            .snapshots
            .get(&profile)
            .filter(|snapshot| snapshot.tab() == tab)
        else {
            return Vec::new();
        };
        snapshot
            .actions()
            .iter()
            .map(|action| {
                let runtime = action.runtime();
                ExtensionActionView {
                    runtime: ExtensionActionRuntimeView {
                        install_id: runtime.install_id().to_string(),
                        generation: format!("{:016x}", runtime.generation().get()),
                    },
                    revision: format!("{:016x}", action.revision().get()),
                    label: action.label().to_owned(),
                    badge: action.badge().to_owned(),
                    icon_rgba_base64: action.icon().map(|icon| STANDARD.encode(icon.rgba())),
                    enabled: action.is_enabled(),
                    presents_popup: action.presents_popup(),
                    unread_badge: action.has_unread_badge(),
                }
            })
            .collect()
    }

    /// Rejoins a native `_execute_action` command to the exact Shell-owned
    /// action snapshot. This mints no action request and accepts no geometry;
    /// privileged chrome must echo the current browser-owned button anchor
    /// through the ordinary invocation path before native work can begin.
    pub(super) fn shortcut_action_revision(
        &self,
        current_surface: Option<&ExtensionBrowserSurface>,
        runtime: ExtensionRuntimeInstance,
        tab: ItemId,
        surface_generation: ExtensionBrowserSurfaceGeneration,
    ) -> Result<ExtensionActionRevision, ExtensionActionRejection> {
        let surface = current_surface
            .filter(|surface| {
                surface.profile() == runtime.profile()
                    && surface.generation() == surface_generation
                    && surface
                        .windows()
                        .first()
                        .and_then(zephium_core::extensions::ExtensionBrowserWindow::active)
                        == Some(tab)
            })
            .ok_or(ExtensionActionRejection::TabUnavailable)?;
        let resident = surface
            .windows()
            .first()
            .and_then(|window| window.tabs().iter().find(|candidate| candidate.id() == tab))
            .is_some_and(zephium_core::extensions::ExtensionBrowserTab::resident);
        let action = self
            .snapshots
            .get(&runtime.profile())
            .filter(|snapshot| {
                snapshot.tab() == tab && snapshot.surface_generation() == surface_generation
            })
            .and_then(|snapshot| {
                snapshot
                    .actions()
                    .iter()
                    .find(|action| action.runtime() == runtime)
            })
            .ok_or(ExtensionActionRejection::ActionUnavailable)?;
        if !action.is_enabled() {
            return Err(ExtensionActionRejection::ActionDisabled);
        }
        if !resident && !action.presents_popup() {
            return Err(ExtensionActionRejection::TabDiscarded);
        }
        Ok(action.revision())
    }

    pub(super) fn observe(
        &mut self,
        current_surface: Option<&ExtensionBrowserSurface>,
        profile: ProfileId,
        tab: ItemId,
        surface_generation: ExtensionBrowserSurfaceGeneration,
        settlement: ExtensionActionSnapshotSettlement,
    ) -> ExtensionActionObservation {
        let current = current_surface.is_some_and(|surface| {
            surface.profile() == profile
                && surface.generation() == surface_generation
                && surface
                    .windows()
                    .first()
                    .and_then(zephium_core::extensions::ExtensionBrowserWindow::active)
                    == Some(tab)
        });
        if !current {
            return ExtensionActionObservation::Stale;
        }
        let ExtensionActionSnapshotSettlement::Applied(snapshot) = settlement else {
            return if self.record_refresh(profile) {
                ExtensionActionObservation::Retained
            } else {
                ExtensionActionObservation::CapacityExceeded
            };
        };
        if snapshot.profile() != profile
            || snapshot.tab() != tab
            || snapshot.surface_generation() != surface_generation
        {
            let _ = self.record_refresh(profile);
            return ExtensionActionObservation::Contradictory;
        }
        self.retry_profiles.remove(profile);
        if self
            .snapshots
            .get(&profile)
            .is_some_and(|previous| previous == &snapshot)
        {
            return ExtensionActionObservation::Unchanged;
        }
        self.snapshots.insert(profile, snapshot);
        ExtensionActionObservation::Applied
    }

    fn record_refresh(&mut self, profile: ProfileId) -> bool {
        self.retry_profiles.try_insert(profile)
    }

    fn retry_profiles(&self) -> zephium_core::extensions::ExtensionActiveProfiles {
        self.retry_profiles
    }

    pub(super) fn clear_projection(&mut self, profile: ProfileId) -> bool {
        self.retry_profiles.remove(profile);
        self.snapshots.remove(&profile).is_some()
    }

    fn begin_invocation(
        &mut self,
        current_surface: Option<&ExtensionBrowserSurface>,
        runtime: ExtensionRuntimeInstance,
        revision: ExtensionActionRevision,
        anchor: ExtensionPopupAnchor,
        menu: bool,
    ) -> Result<ExtensionActionRequest, ExtensionActionRejection> {
        let profile = runtime.profile();
        let surface = current_surface
            .filter(|surface| surface.profile() == profile)
            .ok_or(ExtensionActionRejection::TabUnavailable)?;
        let tab = surface
            .windows()
            .first()
            .and_then(zephium_core::extensions::ExtensionBrowserWindow::active)
            .ok_or(ExtensionActionRejection::TabUnavailable)?;
        let snapshot = self
            .snapshots
            .get(&profile)
            .filter(|snapshot| {
                snapshot.tab() == tab && snapshot.surface_generation() == surface.generation()
            })
            .ok_or(ExtensionActionRejection::ActionUnavailable)?;
        let action = snapshot
            .actions()
            .iter()
            .find(|action| action.runtime() == runtime)
            .ok_or(ExtensionActionRejection::ActionUnavailable)?;
        if action.revision() != revision {
            return Err(ExtensionActionRejection::RuntimeSuperseded);
        }
        // A disabled action still has its menu, as in Chrome.
        if !action.is_enabled() && !menu {
            return Err(ExtensionActionRejection::ActionDisabled);
        }
        if self.pending.len() >= MAX_PENDING_EXTENSION_ACTIONS {
            return Err(ExtensionActionRejection::CapacityExceeded);
        }
        let id = self
            .next_request_id
            .and_then(ExtensionActionRequestId::new)
            .ok_or(ExtensionActionRejection::NativeAdmissionFailed)?;
        self.next_request_id = id.get().checked_add(1);
        let mut request =
            ExtensionActionRequest::new(id, runtime, tab, surface.generation(), revision, anchor);
        if menu {
            request = request.for_menu();
        }
        if self.pending.contains_key(&id) {
            return Err(ExtensionActionRejection::NativeAdmissionFailed);
        }
        self.pending.insert(id, request);
        Ok(request)
    }

    fn cancel_invocation(&mut self, request: ExtensionActionRequestId) {
        self.pending.remove(&request);
    }

    pub(super) fn settle_invocation(
        &mut self,
        profile: ProfileId,
        request: ExtensionActionRequestId,
        settlement: ExtensionActionSettlement,
    ) -> ExtensionActionInvocationObservation {
        let Some(pending) = self.pending.remove(&request) else {
            return ExtensionActionInvocationObservation::Stale;
        };
        if pending.runtime().profile() != profile {
            return ExtensionActionInvocationObservation::Contradictory;
        }
        // A menu that could not open shows nothing to report.
        if pending.is_menu() {
            return ExtensionActionInvocationObservation::Dispatched;
        }
        match settlement {
            ExtensionActionSettlement::Dispatched => {
                ExtensionActionInvocationObservation::Dispatched
            }
            ExtensionActionSettlement::PopupPresented(_) => {
                ExtensionActionInvocationObservation::PopupPresented
            }
            ExtensionActionSettlement::PopupDismissed => {
                ExtensionActionInvocationObservation::PopupDismissed
            }
            ExtensionActionSettlement::Rejected(reason) => {
                ExtensionActionInvocationObservation::Rejected {
                    tab: pending.tab(),
                    reason,
                }
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ExtensionActionObservation {
    Applied,
    Unchanged,
    Retained,
    Stale,
    Contradictory,
    CapacityExceeded,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ExtensionActionInvocationObservation {
    Dispatched,
    PopupPresented,
    PopupDismissed,
    Rejected {
        tab: ItemId,
        reason: ExtensionActionRejection,
    },
    Stale,
    Contradictory,
}

impl Shell {
    /// Schedules one bounded read for the exact currently-published active
    /// tab. The request is independent of browser-surface publication: action
    /// backpressure must not turn a successfully-routed content view into a
    /// failed browser mutation.
    pub(super) fn refresh_extension_actions(&mut self, profile: ProfileId) -> NativeWork {
        let target = self
            .extension_browser_surfaces
            .published_surface(profile)
            .and_then(|surface| {
                surface
                    .windows()
                    .first()
                    .and_then(zephium_core::extensions::ExtensionBrowserWindow::active)
                    .map(|tab| (tab, surface.generation()))
            });
        let Some((tab, generation)) = target else {
            if self.extension_actions.clear_projection(profile) {
                self.project_extension_actions(profile);
            }
            return NativeWork::default();
        };
        let mut native = NativeWork::default();
        if !self.extension_actions.record_refresh(profile) {
            native.rejected = true;
            return native;
        }
        let admission = self
            .engine
            .request_extension_actions(profile, tab, generation);
        native.record(admission);
        if admission == NativeDispatch::Unsupported
            && self.extension_actions.clear_projection(profile)
        {
            self.project_extension_actions(profile);
        }
        native
    }

    /// Retries only reads that were synchronously refused, terminally
    /// rejected, or lost after native scheduling. Native action changes
    /// arrive as invalidations, so with no pending work this is a fixed-size
    /// empty iteration and performs no allocation or dispatch.
    pub(super) fn retry_extension_actions(&mut self) -> NativeWork {
        let profiles = self.extension_actions.retry_profiles();
        let mut native = NativeWork::default();
        for profile in profiles.iter() {
            native.merge(self.refresh_extension_actions(profile));
        }
        native
    }

    /// Admits a toolbar intent only against the exact Shell-visible action.
    /// The caller cannot select a tab, surface generation, or newer revision;
    /// those are derived from the current authoritative projection here.
    #[cfg(test)]
    pub(super) fn invoke_extension_action(
        &mut self,
        runtime: ExtensionRuntimeInstance,
        revision: ExtensionActionRevision,
        anchor: ExtensionPopupAnchor,
    ) -> Result<ExtensionActionRequestId, ExtensionActionRejection> {
        self.dispatch_extension_action(runtime, revision, anchor, false)
    }

    /// [`Self::invoke_extension_action`], or the action's context menu.
    pub(super) fn dispatch_extension_action(
        &mut self,
        runtime: ExtensionRuntimeInstance,
        revision: ExtensionActionRevision,
        anchor: ExtensionPopupAnchor,
        menu: bool,
    ) -> Result<ExtensionActionRequestId, ExtensionActionRejection> {
        let request = self.extension_actions.begin_invocation(
            self.extension_browser_surfaces
                .published_surface(runtime.profile()),
            runtime,
            revision,
            anchor,
            menu,
        )?;
        match self.engine.invoke_extension_action(request) {
            NativeDispatch::Scheduled => Ok(request.id()),
            NativeDispatch::Rejected => {
                self.extension_actions.cancel_invocation(request.id());
                Err(ExtensionActionRejection::NativeAdmissionFailed)
            }
            NativeDispatch::Unsupported => {
                self.extension_actions.cancel_invocation(request.id());
                Err(ExtensionActionRejection::UnsupportedPlatform)
            }
        }
    }
}
