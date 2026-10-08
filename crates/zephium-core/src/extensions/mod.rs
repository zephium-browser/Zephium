//! Bounded extension contracts shared by the shell and the native engine.
//!
//! Every value here is routing or presentation data. None of it grants a
//! runtime, package, or page authority.

mod action;
mod active_profiles;
mod browser_surface;
mod runtime;

pub use action::{
    ExtensionActionError, ExtensionActionIcon, ExtensionActionRejection, ExtensionActionRequest,
    ExtensionActionRequestId, ExtensionActionRevision, ExtensionActionScope,
    ExtensionActionSettlement, ExtensionActionSnapshot, ExtensionActionSnapshotSettlement,
    ExtensionActionState, ExtensionPopupAnchor, EXTENSION_ACTION_ICON_HEIGHT,
    EXTENSION_ACTION_ICON_RGBA_BYTES, EXTENSION_ACTION_ICON_WIDTH,
    MAX_EXTENSION_ACTION_BADGE_BYTES, MAX_EXTENSION_ACTION_LABEL_BYTES,
    MAX_EXTENSION_INSTALLS_PER_PROFILE, MAX_EXTENSION_POPUP_HEIGHT, MAX_EXTENSION_POPUP_WIDTH,
    MIN_EXTENSION_POPUP_HEIGHT, MIN_EXTENSION_POPUP_WIDTH, WINDOWS_EXTENSION_CAPACITY_MESSAGE,
};
pub use active_profiles::{ExtensionActiveProfiles, MAX_EXTENSION_ACTIVE_PROFILES};
pub use browser_surface::{
    AuthTabCleanupPermit, ExtensionBrowserRequest, ExtensionBrowserRequestAction,
    ExtensionBrowserRequestError, ExtensionBrowserRequestId, ExtensionBrowserRequestRejection,
    ExtensionBrowserRequestResult, ExtensionBrowserRequestSettlement, ExtensionBrowserSurface,
    ExtensionBrowserSurfaceError, ExtensionBrowserSurfaceGeneration, ExtensionBrowserTab,
    ExtensionBrowserWindow, MAX_EXTENSION_BROWSER_REQUEST_URL_BYTES, MAX_EXTENSION_BROWSER_TABS,
    MAX_EXTENSION_BROWSER_WINDOWS, MAX_PENDING_EXTENSION_BROWSER_REQUESTS,
    MAX_PENDING_EXTENSION_BROWSER_REQUESTS_PER_PROFILE,
};
pub use runtime::{ExtensionRuntimeGeneration, ExtensionRuntimeInstance};
