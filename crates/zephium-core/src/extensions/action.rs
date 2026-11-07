//! Bounded browser-action state and trusted invocation contracts.
//!
//! Native extension runtimes own action behavior, while the application Shell
//! owns presentation. These values carry only the smallest non-authorizing
//! projection needed to join those domains. In particular, a runtime identity,
//! tab identity, or popup anchor is never sufficient to mint `activeTab`
//! authority or access a native extension context.

use std::sync::Arc;

use crate::geometry::{Rect, Size};
use crate::ids::ItemId;

use super::{ExtensionBrowserSurfaceGeneration, ExtensionRuntimeInstance};

/// One snapshot carries at most one action per installed extension.
pub const MAX_EXTENSION_INSTALLS_PER_PROFILE: usize = 64;
/// Each running extension on Windows keeps a hidden WebView2 page of its own.
pub const WINDOWS_RUNNING_EXTENSIONS: usize = 8;
/// Stable settlement reason used to retry Windows admission after a slot opens.
pub const WINDOWS_EXTENSION_CAPACITY_MESSAGE: &str =
    "This Windows build supports eight running extensions.";

/// Toolbar icons are rasterized once at the exact size consumed by privileged
/// chrome. Fixed-size RGBA avoids retaining native image graphs or accepting
/// attacker-controlled encoded image dimensions in Shell state.
pub const EXTENSION_ACTION_ICON_WIDTH: usize = 32;
pub const EXTENSION_ACTION_ICON_HEIGHT: usize = 32;
pub const EXTENSION_ACTION_ICON_RGBA_BYTES: usize =
    EXTENSION_ACTION_ICON_WIDTH * EXTENSION_ACTION_ICON_HEIGHT * 4;

/// Action labels and badges originate in extension code. Their independent
/// byte ceilings bound allocation even for multi-byte Unicode input.
pub const MAX_EXTENSION_ACTION_LABEL_BYTES: usize = 256;
pub const MAX_EXTENSION_ACTION_BADGE_BYTES: usize = 64;

/// A custom popup is constrained before native view admission. The minimum
/// keeps a malformed content-size report from creating an unusable zero-sized
/// surface; the maximum matches the product overlay contract.
pub const MIN_EXTENSION_POPUP_WIDTH: f64 = 64.0;
pub const MIN_EXTENSION_POPUP_HEIGHT: f64 = 48.0;
pub const MAX_EXTENSION_POPUP_WIDTH: f64 = 800.0;
pub const MAX_EXTENSION_POPUP_HEIGHT: f64 = 600.0;

/// Strictly positive, non-wrapping revision for one runtime's action state.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ExtensionActionRevision(u64);

impl ExtensionActionRevision {
    pub const INITIAL: Self = Self(1);

    pub const fn new(value: u64) -> Option<Self> {
        if value == 0 {
            None
        } else {
            Some(Self(value))
        }
    }

    pub const fn get(self) -> u64 {
        self.0
    }

    pub const fn next(self) -> Option<Self> {
        match self.0.checked_add(1) {
            Some(next) => Some(Self(next)),
            None => None,
        }
    }
}

/// Scope of a native action update.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ExtensionActionScope {
    /// Default state inherited by tabs without an override.
    Default,
    /// Effective state associated with one exact logical tab.
    Tab(ItemId),
}

/// Exact 32x32 RGBA toolbar icon.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExtensionActionIcon(Arc<[u8]>);

impl ExtensionActionIcon {
    pub fn from_rgba(rgba: Vec<u8>) -> Result<Self, ExtensionActionError> {
        if rgba.len() != EXTENSION_ACTION_ICON_RGBA_BYTES {
            return Err(ExtensionActionError::InvalidIcon);
        }
        Ok(Self(Arc::from(rgba)))
    }

    pub fn rgba(&self) -> &[u8] {
        &self.0
    }
}

/// Shell-safe snapshot of one native extension action.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExtensionActionState {
    runtime: ExtensionRuntimeInstance,
    scope: ExtensionActionScope,
    revision: ExtensionActionRevision,
    label: Arc<str>,
    badge: Arc<str>,
    icon: Option<ExtensionActionIcon>,
    enabled: bool,
    presents_popup: bool,
    unread_badge: bool,
}

/// Complete effective action cohort for one exact Shell browser-surface tab.
///
/// Replacement semantics are intentional: an empty snapshot removes every
/// previously projected action for the tab. This prevents disabled, retired,
/// or superseded runtimes from leaving stale toolbar buttons behind.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExtensionActionSnapshot {
    profile: crate::ids::ProfileId,
    tab: ItemId,
    surface_generation: ExtensionBrowserSurfaceGeneration,
    actions: Vec<ExtensionActionState>,
}

impl ExtensionActionSnapshot {
    pub fn new(
        profile: crate::ids::ProfileId,
        tab: ItemId,
        surface_generation: ExtensionBrowserSurfaceGeneration,
        actions: Vec<ExtensionActionState>,
    ) -> Result<Self, ExtensionActionError> {
        if actions.len() > MAX_EXTENSION_INSTALLS_PER_PROFILE {
            return Err(ExtensionActionError::TooManyActions);
        }
        let valid = actions.iter().enumerate().all(|(index, action)| {
            action.runtime().profile() == profile
                && action.scope() == ExtensionActionScope::Tab(tab)
                && actions[index + 1..]
                    .iter()
                    .all(|other| other.runtime() != action.runtime())
        });
        if !valid {
            return Err(ExtensionActionError::InvalidSnapshot);
        }
        Ok(Self {
            profile,
            tab,
            surface_generation,
            actions,
        })
    }

    pub const fn profile(&self) -> crate::ids::ProfileId {
        self.profile
    }

    pub const fn tab(&self) -> ItemId {
        self.tab
    }

    pub const fn surface_generation(&self) -> ExtensionBrowserSurfaceGeneration {
        self.surface_generation
    }

    pub fn actions(&self) -> &[ExtensionActionState] {
        &self.actions
    }
}

impl ExtensionActionState {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        runtime: ExtensionRuntimeInstance,
        scope: ExtensionActionScope,
        revision: ExtensionActionRevision,
        label: &str,
        badge: &str,
        icon: Option<ExtensionActionIcon>,
        enabled: bool,
        presents_popup: bool,
        unread_badge: bool,
    ) -> Result<Self, ExtensionActionError> {
        if !valid_text(label, MAX_EXTENSION_ACTION_LABEL_BYTES) {
            return Err(ExtensionActionError::InvalidLabel);
        }
        if !valid_text(badge, MAX_EXTENSION_ACTION_BADGE_BYTES) {
            return Err(ExtensionActionError::InvalidBadge);
        }
        if unread_badge && badge.is_empty() {
            return Err(ExtensionActionError::UnreadBadgeWithoutText);
        }
        Ok(Self {
            runtime,
            scope,
            revision,
            label: Arc::from(label),
            badge: Arc::from(badge),
            icon,
            enabled,
            presents_popup,
            unread_badge,
        })
    }

    pub const fn runtime(&self) -> ExtensionRuntimeInstance {
        self.runtime
    }

    pub const fn scope(&self) -> ExtensionActionScope {
        self.scope
    }

    pub const fn revision(&self) -> ExtensionActionRevision {
        self.revision
    }

    pub fn label(&self) -> &str {
        &self.label
    }

    pub fn badge(&self) -> &str {
        &self.badge
    }

    pub const fn icon(&self) -> Option<&ExtensionActionIcon> {
        self.icon.as_ref()
    }

    pub const fn is_enabled(&self) -> bool {
        self.enabled
    }

    pub const fn presents_popup(&self) -> bool {
        self.presents_popup
    }

    pub const fn has_unread_badge(&self) -> bool {
        self.unread_badge
    }

    /// Compares all effective presentation data while ignoring the monotonic
    /// revision. Native adapters use this to suppress unchanged snapshots.
    pub fn same_presentation(&self, other: &Self) -> bool {
        self.runtime == other.runtime
            && self.scope == other.scope
            && self.label == other.label
            && self.badge == other.badge
            && self.icon == other.icon
            && self.enabled == other.enabled
            && self.presents_popup == other.presents_popup
            && self.unread_badge == other.unread_badge
    }
}

/// Process-local correlation id for one trusted toolbar invocation.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ExtensionActionRequestId(u64);

impl ExtensionActionRequestId {
    pub const fn new(value: u64) -> Option<Self> {
        if value == 0 {
            None
        } else {
            Some(Self(value))
        }
    }

    pub const fn get(self) -> u64 {
        self.0
    }
}

/// Validated logical-pixel placement supplied by privileged browser chrome.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ExtensionPopupAnchor(Rect);

impl ExtensionPopupAnchor {
    pub fn new(rect: Rect) -> Result<Self, ExtensionActionError> {
        let values = [rect.x, rect.y, rect.width, rect.height];
        if values.iter().any(|value| !value.is_finite()) || rect.width <= 0.0 || rect.height <= 0.0
        {
            return Err(ExtensionActionError::InvalidPopupAnchor);
        }
        Ok(Self(rect))
    }

    pub const fn rect(self) -> Rect {
        self.0
    }
}

/// One browser-chrome-authored request to invoke a native action.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ExtensionActionRequest {
    id: ExtensionActionRequestId,
    runtime: ExtensionRuntimeInstance,
    tab: ItemId,
    surface_generation: ExtensionBrowserSurfaceGeneration,
    action_revision: ExtensionActionRevision,
    anchor: ExtensionPopupAnchor,
    menu: bool,
}

impl ExtensionActionRequest {
    pub const fn new(
        id: ExtensionActionRequestId,
        runtime: ExtensionRuntimeInstance,
        tab: ItemId,
        surface_generation: ExtensionBrowserSurfaceGeneration,
        action_revision: ExtensionActionRevision,
        anchor: ExtensionPopupAnchor,
    ) -> Self {
        Self {
            id,
            runtime,
            tab,
            surface_generation,
            action_revision,
            anchor,
            menu: false,
        }
    }

    /// Shows the action's context menu (its `contexts: ["action"]` items)
    /// at the anchor instead of clicking it.
    #[must_use]
    pub const fn for_menu(mut self) -> Self {
        self.menu = true;
        self
    }

    pub const fn is_menu(self) -> bool {
        self.menu
    }

    pub const fn id(self) -> ExtensionActionRequestId {
        self.id
    }

    pub const fn runtime(self) -> ExtensionRuntimeInstance {
        self.runtime
    }

    pub const fn tab(self) -> ItemId {
        self.tab
    }

    pub const fn surface_generation(self) -> ExtensionBrowserSurfaceGeneration {
        self.surface_generation
    }

    pub const fn action_revision(self) -> ExtensionActionRevision {
        self.action_revision
    }

    pub const fn anchor(self) -> ExtensionPopupAnchor {
        self.anchor
    }
}

/// Terminal result for a trusted action invocation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ExtensionActionSettlement {
    /// The action dispatched its click event and owns no popup view.
    Dispatched,
    /// A custom popup is visible at the returned bounded logical size.
    PopupPresented(Size),
    /// A second invocation dismissed the already-visible popup for the exact
    /// same runtime and tab without allocating another native surface.
    PopupDismissed,
    Rejected(ExtensionActionRejection),
}

/// Terminal response to one action-cohort refresh. A rejected refresh must
/// not erase the Shell's last known-good cohort; an applied empty snapshot
/// does.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ExtensionActionSnapshotSettlement {
    Applied(ExtensionActionSnapshot),
    Rejected(ExtensionActionRejection),
}

/// Closed, user-visible rejection taxonomy for toolbar actions.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExtensionActionRejection {
    InvalidRequest,
    RuntimeUnavailable,
    RuntimeSuperseded,
    TabUnavailable,
    TabDiscarded,
    ActionUnavailable,
    ActionDisabled,
    CapacityExceeded,
    PopupUnavailable,
    PopupCapacityExceeded,
    NativeAdmissionFailed,
    ShuttingDown,
    UnsupportedPlatform,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExtensionActionError {
    InvalidLabel,
    InvalidBadge,
    InvalidIcon,
    UnreadBadgeWithoutText,
    InvalidPopupAnchor,
    TooManyActions,
    InvalidSnapshot,
}

fn valid_text(value: &str, max_bytes: usize) -> bool {
    value.len() <= max_bytes && !value.chars().any(char::is_control)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extensions::ExtensionRuntimeGeneration;
    use crate::ids::{ExtensionInstallId, ProfileId};

    fn runtime() -> ExtensionRuntimeInstance {
        ExtensionRuntimeInstance::new(
            ProfileId::from(1),
            ExtensionInstallId::from(2),
            ExtensionRuntimeGeneration::INITIAL,
        )
    }

    #[test]
    fn action_projection_is_bounded_and_self_consistent() {
        let icon = ExtensionActionIcon::from_rgba(vec![7; EXTENSION_ACTION_ICON_RGBA_BYTES])
            .expect("exact RGBA icon");
        let state = ExtensionActionState::new(
            runtime(),
            ExtensionActionScope::Tab(ItemId::from(3)),
            ExtensionActionRevision::INITIAL,
            "Bitwarden",
            "1",
            Some(icon),
            true,
            true,
            true,
        )
        .expect("bounded action");
        assert_eq!(state.label(), "Bitwarden");
        assert_eq!(state.badge(), "1");
        assert_eq!(
            state.icon().unwrap().rgba().len(),
            EXTENSION_ACTION_ICON_RGBA_BYTES
        );
        assert!(state.is_enabled());
        assert!(state.presents_popup());
        assert!(state.has_unread_badge());

        assert_eq!(
            ExtensionActionState::new(
                runtime(),
                ExtensionActionScope::Default,
                ExtensionActionRevision::INITIAL,
                "ok\nnot-ok",
                "",
                None,
                true,
                false,
                false,
            ),
            Err(ExtensionActionError::InvalidLabel)
        );
        assert_eq!(
            ExtensionActionState::new(
                runtime(),
                ExtensionActionScope::Default,
                ExtensionActionRevision::INITIAL,
                "ok",
                "",
                None,
                true,
                false,
                true,
            ),
            Err(ExtensionActionError::UnreadBadgeWithoutText)
        );
    }

    #[test]
    fn popup_anchor_rejects_non_finite_and_empty_geometry() {
        assert!(ExtensionPopupAnchor::new(Rect::new(1.0, 2.0, 20.0, 20.0)).is_ok());
        assert_eq!(
            ExtensionPopupAnchor::new(Rect {
                x: f64::NAN,
                y: 0.0,
                width: 1.0,
                height: 1.0,
            }),
            Err(ExtensionActionError::InvalidPopupAnchor)
        );
        assert_eq!(
            ExtensionPopupAnchor::new(Rect::default()),
            Err(ExtensionActionError::InvalidPopupAnchor)
        );
    }

    #[test]
    fn invocation_carries_the_exact_shell_versions() {
        let id = ExtensionActionRequestId::new(9).unwrap();
        let surface = ExtensionBrowserSurfaceGeneration::new(7).unwrap();
        let revision = ExtensionActionRevision::new(11).unwrap();
        let anchor = ExtensionPopupAnchor::new(Rect::new(10.0, 20.0, 24.0, 24.0)).unwrap();
        let request =
            ExtensionActionRequest::new(id, runtime(), ItemId::from(3), surface, revision, anchor);
        assert_eq!(request.id(), id);
        assert_eq!(request.runtime(), runtime());
        assert_eq!(request.tab(), ItemId::from(3));
        assert_eq!(request.surface_generation(), surface);
        assert_eq!(request.action_revision(), revision);
        assert_eq!(request.anchor(), anchor);
    }

    #[test]
    fn action_revision_never_wraps_or_accepts_zero() {
        assert_eq!(ExtensionActionRevision::new(0), None);
        assert_eq!(
            ExtensionActionRevision::INITIAL.next(),
            ExtensionActionRevision::new(2)
        );
        assert_eq!(ExtensionActionRevision::new(u64::MAX).unwrap().next(), None);
    }

    #[test]
    fn snapshot_is_exact_replaceable_and_rejects_mixed_scope() {
        let state = ExtensionActionState::new(
            runtime(),
            ExtensionActionScope::Tab(ItemId::from(3)),
            ExtensionActionRevision::INITIAL,
            "Bitwarden",
            "",
            None,
            true,
            true,
            false,
        )
        .unwrap();
        let snapshot = ExtensionActionSnapshot::new(
            ProfileId::from(1),
            ItemId::from(3),
            ExtensionBrowserSurfaceGeneration::INITIAL,
            vec![state.clone()],
        )
        .unwrap();
        assert_eq!(snapshot.actions(), std::slice::from_ref(&state));
        assert_eq!(
            ExtensionActionSnapshot::new(
                ProfileId::from(1),
                ItemId::from(4),
                ExtensionBrowserSurfaceGeneration::INITIAL,
                vec![state],
            ),
            Err(ExtensionActionError::InvalidSnapshot)
        );
        assert!(ExtensionActionSnapshot::new(
            ProfileId::from(1),
            ItemId::from(3),
            ExtensionBrowserSurfaceGeneration::INITIAL,
            Vec::new(),
        )
        .unwrap()
        .actions()
        .is_empty());
    }
}
