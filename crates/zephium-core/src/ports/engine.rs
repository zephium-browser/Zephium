use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::blocker::{ContentPolicyGeneration, ContentRuleApplyFailure, ContentRules};
use crate::extensions::{
    ExtensionBrowserRequest, ExtensionBrowserRequestId, ExtensionBrowserRequestSettlement,
    ExtensionBrowserSurface,
};
use crate::geometry::Rect;
use crate::ids::{ExtensionInstallId, ItemId, ProfileId, ScriptId, UserscriptId, WindowId};
use crate::injection::MatchSet;
pub use crate::permissions::PagePermissionKind as PermissionKind;
use crate::permissions::{
    PagePermissionRequest, PagePermissionRequestId, PagePermissionRequestSettlement,
};
use crate::runtime_security::RuntimeSecurityAdvisories;
use crate::split::Pane;

/// OS memory pressure, independent of tab count or process RSS estimates.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MemoryPressure {
    #[default]
    Normal,
    Warning,
    Critical,
}

/// Native capture observations; page JavaScript cannot establish these facts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CaptureDeviceState {
    #[default]
    None,
    Active,
    Muted,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MediaCaptureState {
    pub camera: CaptureDeviceState,
    pub microphone: CaptureDeviceState,
}

impl MediaCaptureState {
    pub fn is_capturing(self) -> bool {
        self.camera != CaptureDeviceState::None || self.microphone != CaptureDeviceState::None
    }
}

/// A prepared extension package the user has consented to run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WebExtensionLoad {
    pub install: ExtensionInstallId,
    /// The Chrome Web Store ID, which is also the extension's origin.
    pub extension_id: String,
    pub root: std::path::PathBuf,
    pub permissions: Vec<String>,
    pub match_patterns: Vec<String>,
    /// Set when the package changed since WebKit last ran it, so its
    /// background runs once and WebKit relearns what it listens for.
    pub start_background: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WebExtensionLoaded {
    pub name: String,
    pub version: String,
}

/// Which engine data partition a view lives in. Every persistent profile gets
/// its own engine store; incognito is ephemeral and never intentionally
/// persists browsing data. Within one engine process a `ProfileId` is
/// permanently bound to either the durable class (`Default`/`Persistent`) or
/// `Ephemeral`; callers must mint a new id instead of changing that class.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Partition {
    Default(ProfileId),
    Persistent(ProfileId),
    Ephemeral(ProfileId),
}

impl Partition {
    pub fn profile(self) -> ProfileId {
        match self {
            Partition::Default(p) | Partition::Persistent(p) | Partition::Ephemeral(p) => p,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ContentScope {
    Global,
    Profile(ProfileId),
}

/// Native security principal for one isolated script world.
///
/// The variant is part of the identity: a userscript and extension can never
/// alias merely because their persistent ids happen to contain equal bytes.
/// `UserscriptId` is the durable catalog-row identity and
/// `ExtensionInstallId` is the durable per-profile installation identity;
/// source/package updates retain those ids, while delete-and-reinstall must
/// mint a new domain id. [`ScriptId`] remains owner-local registration state
/// and must never be promoted into a security principal.
/// Native adapters derive world and handler names from this value and must
/// never accept a principal supplied by page JavaScript.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ScriptPrincipal {
    Userscript(UserscriptId),
    Extension(ExtensionInstallId),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ScriptOwner {
    Builtin,
    Principal(ScriptPrincipal),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum World {
    Page,
    Isolated(ScriptPrincipal),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RunAt {
    DocumentStart,
    DocumentEnd,
    DocumentIdle,
}

/// Exact process-local identity for one desired user-content generation.
/// Values never wrap: exhausted owners must restart instead of risking a late
/// native callback being accepted as a newer replacement.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct UserContentGeneration(u64);

impl UserContentGeneration {
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
            Some(value) => Some(Self(value)),
            None => None,
        }
    }
}

/// What the engine can truthfully establish at its synchronous call boundary.
///
/// `Scheduled` is deliberately not a claim that WebKit/WebView2 applied the
/// action, navigated, or produced a renderer result. It proves that the exact,
/// lifecycle-checked task was admitted to the owning native UI event loop. A
/// task can still be invalidated by an ordered close before that queue runs.
///
/// Some platforms have a second bounded host queue because a native engine
/// can pump the event loop re-entrantly. Refusal at that later boundary is not
/// silently treated as success: the backend terminally revokes content
/// authority and invokes its mandatory fatal callback.
#[must_use]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NativeDispatch {
    Scheduled,
    Rejected,
    Unsupported,
}

/// Opaque identity for one native main-frame navigation presentation.
///
/// The browser shell may return this token only to [`Engine::present_navigation`].
/// URLs are deliberately not identities: redirects and overlapping loads can
/// otherwise reveal content before chrome has attributed the exact commit.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct NavigationPresentationId(u64);

impl NavigationPresentationId {
    /// Native adapters mint non-wrapping identities from their own exact
    /// navigation epochs. Callers must treat the value as opaque.
    pub const fn from_raw(value: u64) -> Self {
        Self(value)
    }

    pub const fn into_raw(self) -> u64 {
        self.0
    }
}

impl NativeDispatch {
    pub fn from_scheduled(scheduled: bool) -> Self {
        if scheduled {
            Self::Scheduled
        } else {
            Self::Rejected
        }
    }
}

/// Result of permanently retiring an engine profile and erasing the native
/// web engine data it owned.
///
/// `Verified` is deliberately narrow: the engine has released every native
/// view/context it knows for the profile and proved its engine-owned data
/// store absent (or empty for an ephemeral native store). It does not include
/// Zephium's SQLite/session data; callers must coordinate that separate store
/// transaction before removing a profile from the registry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProfileDataErasureOutcome {
    /// Native work settled and the scoped engine data was proven absent.
    Verified,
    /// The native attempt reached a terminal failure, including refusal to
    /// dispatch it. A retry may be admitted, but the synchronous retirement
    /// gate remains permanent and this outcome is never proof that existing
    /// native references closed or that their data was deleted. A dispatcher
    /// refusal is terminal for continued browsing and requires process exit
    /// unless a later attempt reaches `Verified`.
    Failed,
    /// The caller's bounded wait elapsed. This is not native cancellation or
    /// settlement; the in-process attempt remains active until a later native
    /// terminal callback, so an immediate retry must be rejected. Existing
    /// native references and data are unproven; continued browsing is unsafe
    /// and the coordinator must drive verified teardown or process exit.
    TimedOut,
}

/// One pipeline for everything injected into content: cosmetic CSS, Boosts,
/// userscripts, adblock cosmetics and a future extensions layer.
pub const MAX_USER_SCRIPTS_PER_SCOPE: usize = 256;
pub const MAX_USER_STYLES_PER_SCOPE: usize = 256;
pub const MAX_USER_SCRIPTS_PER_OWNER: usize = 64;
pub const MAX_USER_STYLES_PER_OWNER: usize = 64;
pub const MAX_USER_SCRIPT_BYTES: usize = 2 * 1024 * 1024;
// JSON escaping can expand one CSS byte to six source bytes before the
// document-start wrapper is installed. Keep the worst-case generated script
// below MAX_USER_SCRIPT_BYTES without needing an unbounded second pass.
pub const MAX_USER_STYLE_BYTES: usize = 256 * 1024;
pub const MAX_USER_CONTENT_BYTES_PER_SCOPE: usize = 16 * 1024 * 1024;
pub const MAX_USER_CONTENT_BYTES_PER_OWNER: usize = 4 * 1024 * 1024;
pub const MAX_USER_CONTENT_RETAINED_BYTES_PER_OWNER: usize = 16 * 1024 * 1024;
pub const MAX_USER_CONTENT_RETAINED_BYTES_PER_SCOPE: usize = 32 * 1024 * 1024;
pub const MAX_USER_CONTENT_RETAINED_BYTES_PROCESS: usize = 64 * 1024 * 1024;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct UserContent {
    pub scripts: Vec<UserScript>,
    pub styles: Vec<UserStyle>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UserScript {
    pub id: ScriptId,
    pub owner: ScriptOwner,
    pub source: Arc<str>,
    pub world: World,
    pub matches: MatchSet,
    pub run_at: RunAt,
    pub all_frames: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UserStyle {
    pub id: ScriptId,
    pub owner: ScriptOwner,
    pub css: Arc<str>,
    pub matches: MatchSet,
    pub all_frames: bool,
}

/// Stable registration identity. Script ids are owner-local, so native diff
/// maps must retain both fields when global/profile sets are composed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct UserScriptKey {
    pub owner: ScriptOwner,
    pub id: ScriptId,
}

impl UserScript {
    pub const fn key(&self) -> UserScriptKey {
        UserScriptKey {
            owner: self.owner,
            id: self.id,
        }
    }
}

impl UserStyle {
    pub const fn key(&self) -> UserScriptKey {
        UserScriptKey {
            owner: self.owner,
            id: self.id,
        }
    }
}

fn user_script_retained_budget_bytes(script: &UserScript) -> Option<usize> {
    script
        .source
        .len()
        .checked_add(script.matches.retained_budget_bytes())?
        .checked_add(256)
}

fn user_style_retained_budget_bytes(style: &UserStyle) -> Option<usize> {
    // JSON string escaping is at most six ASCII bytes per UTF-8 input byte
    // for the control characters that expand the most. Charge the cached
    // wrapper before it is materialized in the host.
    style
        .css
        .len()
        .checked_mul(6)?
        .checked_add(512)?
        .checked_add(style.matches.retained_budget_bytes())?
        .checked_add(256)
}

impl UserContent {
    /// Conservative process-memory charge used by both synchronous dispatch
    /// admission and the host's retained-registry budget.
    pub fn retained_budget_bytes(&self) -> Option<usize> {
        self.scripts
            .iter()
            .map(user_script_retained_budget_bytes)
            .chain(self.styles.iter().map(user_style_retained_budget_bytes))
            .try_fold(0_usize, |total, bytes| total.checked_add(bytes?))
    }

    /// Validates allocation and principal invariants before content reaches a
    /// native adapter. The returned refusal list is bounded by the already
    /// checked script/style count limits.
    pub fn validate(&self) -> Result<(), UserContentApplyFailure> {
        if self.scripts.len() > MAX_USER_SCRIPTS_PER_SCOPE {
            return Err(UserContentApplyFailure::TooManyScripts);
        }
        if self.styles.len() > MAX_USER_STYLES_PER_SCOPE {
            return Err(UserContentApplyFailure::TooManyStyles);
        }

        #[derive(Default)]
        struct OwnerUsage {
            scripts: usize,
            styles: usize,
            source_bytes: usize,
            retained_bytes: usize,
        }

        let mut total_bytes = 0_usize;
        let mut retained_bytes = 0_usize;
        let mut owner_usage = HashMap::<ScriptOwner, OwnerUsage>::new();
        let mut ids = HashSet::with_capacity(self.scripts.len() + self.styles.len());
        let mut refusals = Vec::new();
        for script in &self.scripts {
            total_bytes = total_bytes
                .checked_add(script.source.len())
                .ok_or(UserContentApplyFailure::TotalSourceTooLarge)?;
            let registration_bytes = user_script_retained_budget_bytes(script)
                .ok_or(UserContentApplyFailure::TotalRetainedTooLarge)?;
            retained_bytes = retained_bytes
                .checked_add(registration_bytes)
                .ok_or(UserContentApplyFailure::TotalRetainedTooLarge)?;
            let usage = owner_usage.entry(script.owner).or_default();
            usage.scripts = usage.scripts.saturating_add(1);
            usage.source_bytes = usage
                .source_bytes
                .checked_add(script.source.len())
                .ok_or(UserContentApplyFailure::OwnerBudgetExceeded)?;
            usage.retained_bytes = usage
                .retained_bytes
                .checked_add(registration_bytes)
                .ok_or(UserContentApplyFailure::OwnerBudgetExceeded)?;
            if script.source.is_empty() {
                refusals.push(UserScriptRefusal {
                    registration: script.key(),
                    reason: UserScriptRefusalReason::EmptySource,
                });
            } else if script.source.len() > MAX_USER_SCRIPT_BYTES {
                refusals.push(UserScriptRefusal {
                    registration: script.key(),
                    reason: UserScriptRefusalReason::SourceTooLarge,
                });
            } else if script.source.as_bytes().contains(&0) {
                // WebKitGTK consumes NUL-terminated UTF-8 and would otherwise
                // install only the caller-controlled prefix. Keep the core
                // contract identical on every platform instead of relying on
                // an adapter-specific conversion failure.
                refusals.push(UserScriptRefusal {
                    registration: script.key(),
                    reason: UserScriptRefusalReason::EmbeddedNul,
                });
            }
            let owner_matches_world = matches!(
                (script.owner, script.world),
                (ScriptOwner::Builtin, World::Page)
            ) || matches!(
                (script.owner, script.world),
                (ScriptOwner::Principal(owner), World::Isolated(world)) if owner == world
            );
            if !owner_matches_world {
                refusals.push(UserScriptRefusal {
                    registration: script.key(),
                    reason: UserScriptRefusalReason::OwnerWorldMismatch,
                });
            }
            if !ids.insert(script.key()) {
                refusals.push(UserScriptRefusal {
                    registration: script.key(),
                    reason: UserScriptRefusalReason::DuplicateId,
                });
            }
        }
        for style in &self.styles {
            total_bytes = total_bytes
                .checked_add(style.css.len())
                .ok_or(UserContentApplyFailure::TotalSourceTooLarge)?;
            let registration_bytes = user_style_retained_budget_bytes(style)
                .ok_or(UserContentApplyFailure::TotalRetainedTooLarge)?;
            retained_bytes = retained_bytes
                .checked_add(registration_bytes)
                .ok_or(UserContentApplyFailure::TotalRetainedTooLarge)?;
            let usage = owner_usage.entry(style.owner).or_default();
            usage.styles = usage.styles.saturating_add(1);
            usage.source_bytes = usage
                .source_bytes
                .checked_add(style.css.len())
                .ok_or(UserContentApplyFailure::OwnerBudgetExceeded)?;
            usage.retained_bytes = usage
                .retained_bytes
                .checked_add(registration_bytes)
                .ok_or(UserContentApplyFailure::OwnerBudgetExceeded)?;
            if style.css.is_empty() {
                refusals.push(UserScriptRefusal {
                    registration: style.key(),
                    reason: UserScriptRefusalReason::EmptySource,
                });
            } else if style.css.len() > MAX_USER_STYLE_BYTES {
                refusals.push(UserScriptRefusal {
                    registration: style.key(),
                    reason: UserScriptRefusalReason::SourceTooLarge,
                });
            } else if style.css.as_bytes().contains(&0) {
                refusals.push(UserScriptRefusal {
                    registration: style.key(),
                    reason: UserScriptRefusalReason::EmbeddedNul,
                });
            }
            if !ids.insert(style.key()) {
                refusals.push(UserScriptRefusal {
                    registration: style.key(),
                    reason: UserScriptRefusalReason::DuplicateId,
                });
            }
        }
        if total_bytes > MAX_USER_CONTENT_BYTES_PER_SCOPE {
            return Err(UserContentApplyFailure::TotalSourceTooLarge);
        }
        if retained_bytes > MAX_USER_CONTENT_RETAINED_BYTES_PER_SCOPE {
            return Err(UserContentApplyFailure::TotalRetainedTooLarge);
        }
        if owner_usage.values().any(|usage| {
            usage.scripts > MAX_USER_SCRIPTS_PER_OWNER
                || usage.styles > MAX_USER_STYLES_PER_OWNER
                || usage.source_bytes > MAX_USER_CONTENT_BYTES_PER_OWNER
                || usage.retained_bytes > MAX_USER_CONTENT_RETAINED_BYTES_PER_OWNER
        }) {
            return Err(UserContentApplyFailure::OwnerBudgetExceeded);
        }
        if refusals.is_empty() {
            Ok(())
        } else {
            Err(UserContentApplyFailure::Scripts(refusals))
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum UserScriptRefusalReason {
    EmptySource,
    SourceTooLarge,
    EmbeddedNul,
    OwnerWorldMismatch,
    DuplicateId,
    UnsupportedWorld,
    UnsupportedRunAt,
    UnsupportedFrameTarget,
    UnsupportedMatchSet,
    HostOnlyOwner,
    InvalidScope,
    ProtectedRegistration,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct UserScriptRefusal {
    pub registration: UserScriptKey,
    pub reason: UserScriptRefusalReason,
}

/// Stable, bounded failure classes for one atomic user-content replacement.
/// Native/parser strings and injected source are intentionally absent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UserContentApplyFailure {
    StaleGeneration,
    TooManyScripts,
    TooManyStyles,
    TooManyScopes,
    ReservedScope,
    TotalSourceTooLarge,
    TotalRetainedTooLarge,
    OwnerBudgetExceeded,
    ProcessBudgetExceeded,
    Scripts(Vec<UserScriptRefusal>),
    NativeInstallation,
    NativeCleanup,
    UnsupportedPlatform,
}

/// Terminal native state for one desired user-content generation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UserContentSettlement {
    Applied {
        generation: UserContentGeneration,
    },
    Retained {
        generation: UserContentGeneration,
        failure: UserContentApplyFailure,
    },
    Unavailable {
        failure: UserContentApplyFailure,
    },
}

/// Longest text one find may search for.
pub const MAX_FIND_QUERY_BYTES: usize = 1024;
/// Matches an engine counts before it stops.
pub const MAX_FIND_MATCHES: u32 = 1000;

/// A find in the page. Repeating the same `query` steps to the next match
/// (or the previous one when `forward` is false); a different query starts a
/// new search.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FindRequest {
    pub query: String,
    pub forward: bool,
}

/// A resolved keyboard shortcut for platforms where the engine must
/// intercept keys natively (WebView2 AcceleratorKeyPressed).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Shortcut {
    pub id: String,
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    pub key: u32,
}

/// How a content layout that follows a deliberate change of the window's
/// shape is carried out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StageMotion {
    /// The content keeps its size for the journey and slides to its new
    /// place, so a page is laid out at most once.
    Slide,
    /// The content was hidden behind a browser page and returns to view.
    Arrive,
}

/// Where page content goes when it enters element fullscreen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FullscreenPresentation {
    /// The engine moves the page into a fullscreen window of its own (WebKit
    /// on macOS). Browser layout stays as it is.
    OwnWindow,
    /// The page fills its own view only (WebView2). The browser makes its
    /// window fullscreen and lays that one view over the whole of it.
    FillHostWindow,
}

/// Exactly-once completion ownership for a per-profile native site snapshot.
/// Dropping a refused or shutdown task reports failure rather than stranding
/// an accepted caller. It carries no page data or native handles.
pub struct BlockerSiteCompletion(Option<Box<dyn FnOnce(bool) + Send>>);

impl BlockerSiteCompletion {
    pub fn new(done: impl FnOnce(bool) + Send + 'static) -> Self {
        Self(Some(Box::new(done)))
    }
    pub fn finish(mut self, applied: bool) {
        if let Some(done) = self.0.take() {
            done(applied);
        }
    }
}

impl Drop for BlockerSiteCompletion {
    fn drop(&mut self) {
        if let Some(done) = self.0.take() {
            done(false);
        }
    }
}

/// Native candidate admission does not install profile or view policy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ContentRuleValidationOutcome {
    Valid,
    /// Dispatch/queue/lifecycle refusal is retryable, not a bad list verdict.
    Unavailable,
    Rejected(crate::blocker::ContentRuleApplyFailure),
}

/// Exactly-once native preflight completion; dropped work remains unavailable.
pub struct ContentRuleValidationCompletion(
    Option<Box<dyn FnOnce(ContentRuleValidationOutcome) + Send>>,
);
impl ContentRuleValidationCompletion {
    pub fn new(done: impl FnOnce(ContentRuleValidationOutcome) + Send + 'static) -> Self {
        Self(Some(Box::new(done)))
    }
    pub fn finish(mut self, outcome: ContentRuleValidationOutcome) {
        if let Some(done) = self.0.take() {
            done(outcome);
        }
    }
}
impl Drop for ContentRuleValidationCompletion {
    fn drop(&mut self) {
        if let Some(done) = self.0.take() {
            done(ContentRuleValidationOutcome::Unavailable);
        }
    }
}

pub trait Engine {
    /// Shuts sites to top-level loads while a focus round runs; `None` opens
    /// everything again.
    fn set_focus_gate(&self, _gate: Option<crate::time::FocusGate>) {}
    /// Holds a view's audio and video still while it is covered for focus.
    fn set_media_suspended(&self, _id: ItemId, _suspended: bool) {}
    fn set_blocker_statistics(
        &self,
        _profile: ProfileId,
        _counter: crate::blocker::BlockedLoadCounter,
    ) {
    }
    fn collect_blocker_statistics(
        &self,
        _profile: ProfileId,
        _reset: bool,
        done: Box<dyn FnOnce() + Send>,
    ) {
        done();
    }

    fn validate_content_rules(
        &self,
        _rules: Arc<ContentRules>,
        completion: ContentRuleValidationCompletion,
    ) {
        completion.finish(ContentRuleValidationOutcome::Unavailable);
    }

    fn element_picker(
        &self,
        _profile: ProfileId,
        _id: ItemId,
        _site: crate::blocker::BlockerSite,
        _request: crate::blocker::ElementPickerRequest,
        completion: crate::blocker::ElementPickerCompletion,
    ) {
        completion.finish(None);
    }

    fn set_blocker_site_preferences(
        &self,
        _profile: ProfileId,
        _preferences: Arc<crate::blocker::PreparedBlockerSites>,
        completion: BlockerSiteCompletion,
    ) {
        completion.finish(false);
    }
    /// Browser-owned file actions. Only trusted Shell admission supplies the
    /// profile partition; the caller supplies IDs, never filesystem paths.
    fn download_call(
        &self,
        _partition: Partition,
        _call: crate::downloads::DownloadCall,
        done: crate::downloads::DownloadCompletion,
    ) -> bool {
        done.finish(crate::downloads::DownloadResponse::Error {
            error: crate::downloads::DownloadError::Unsupported,
        });
        true
    }

    /// Schedules creation on the native UI thread. `false` means the request
    /// was not admitted at all, so the shell must roll back its live-view bit.
    fn create_view(&self, id: ItemId, partition: Partition, url: &str, bounds: Rect) -> bool;
    /// Schedules a navigation on the native UI thread. `false` means the
    /// request was not admitted. An admitted request can still fail before
    /// its identity-bearing native commit, synchronously or asynchronously;
    /// that arrives as `EngineEvent::NavigationFailed` with the same request
    /// id.
    fn navigate(&self, id: ItemId, url: &str, request: NavigationRequestId) -> bool;
    /// Reveals an initially hidden native view only if `navigation` is still
    /// the exact committed main-frame epoch whose URL was delivered to the
    /// shell. The shell calls this only after privileged chrome has applied
    /// and verified the matching revision-bearing URL projection; native
    /// completion may re-drive the same idempotent acknowledgement.
    fn present_navigation(
        &self,
        _id: ItemId,
        _navigation: NavigationPresentationId,
    ) -> NativeDispatch {
        NativeDispatch::Unsupported
    }
    fn reload(&self, id: ItemId) -> NativeDispatch;
    fn stop(&self, id: ItemId) -> NativeDispatch;
    fn go_back(&self, id: ItemId) -> NativeDispatch;
    fn go_forward(&self, id: ItemId) -> NativeDispatch;
    fn close(&self, id: ItemId) -> NativeDispatch;
    /// Lay the split `tree` into `region` of `window`, or hide that window's
    /// content when `region` is `None`. The engine owns pane geometry so
    /// resize stays in the native pass.
    fn set_content(
        &self,
        window: WindowId,
        tree: Option<Pane>,
        region: Option<Rect>,
    ) -> NativeDispatch;
    fn set_drop_indicator(&self, window: WindowId, zone: Option<Rect>) -> NativeDispatch;
    /// A transient guide in window coordinates; never changes pane geometry.
    fn set_resize_guide(&self, _window: WindowId, _zone: Option<Rect>) -> NativeDispatch {
        NativeDispatch::Unsupported
    }
    /// Asks the next content layout applied to `window` to move rather than
    /// jump, because it follows a deliberate change of the window's shape
    /// and not a resize. Consumed by that layout whether or not it moved
    /// anything; an engine without native motion ignores it.
    fn hint_stage_motion(&self, _window: WindowId, _motion: StageMotion) -> NativeDispatch {
        NativeDispatch::Unsupported
    }
    /// Requests an exact page zoom for the current native-view generation.
    /// Queue admission is not application: the authoritative native scale
    /// arrives as [`EngineEvent::ZoomSettled`] carrying the same `request`.
    fn zoom(&self, id: ItemId, scale: f64, request: ZoomRequestId) -> NativeDispatch;
    fn set_muted(&self, id: ItemId, muted: bool) -> NativeDispatch;
    /// One step of finding text in the page; `None` ends the session and
    /// clears its highlights. Results arrive as `EngineEvent::FindResult`.
    fn find(&self, id: ItemId, request: Option<FindRequest>) -> NativeDispatch;
    /// Result arrives as `EngineEvent::Captured`.
    fn capture(&self, id: ItemId) -> NativeDispatch;
    /// Result arrives as `EngineEvent::HtmlExtracted`.
    fn extract_html(&self, id: ItemId) -> NativeDispatch;
    /// Asks the sandboxed page renderer to fetch and decode its best icon;
    /// an exact 32x32 RGBA result arrives as `EngineEvent::FaviconPixels`.
    /// Repeated calls poll the renderer-owned asynchronous decode state.
    fn discover_favicon(&self, id: ItemId) -> NativeDispatch;
    /// Asynchronously asks the exact live native-view generation and its
    /// current committed navigation whether it is safe to discard. A missing
    /// or malformed response is deliberately not a positive result. Callers
    /// must correlate `probe` with `EngineEvent::DiscardSafety` and recheck
    /// visibility/loading state before closing the view.
    fn probe_discard_safety(&self, id: ItemId, probe: DiscardProbeId) -> bool;
    /// Refreshes safety and prepares restoration before retiring a probed view.
    /// Refusal keeps the view live; successful completion arrives only after
    /// native destruction. False means admission failed without retirement.
    fn discard_view(&self, id: ItemId, probe: DiscardProbeId) -> bool;
    /// Synchronously cancels this exact pending discard before retirement.
    /// Its eventual terminal event still settles the caller's closing state.
    fn cancel_discard(&self, _id: ItemId, _probe: DiscardProbeId) {}
    /// Erases volatile restoration data on logical close or browsing-data deletion.
    /// None clears the profile; this never closes an active page.
    fn forget_discarded_state(&self, _profile: ProfileId, _item: Option<ItemId>) {}
    fn print(&self, id: ItemId) -> NativeDispatch;
    /// Opens the page's own inspector. Browser chrome is never inspectable.
    fn open_devtools(&self, _id: ItemId) -> NativeDispatch {
        NativeDispatch::Rejected
    }
    /// Hands an application link the person allowed to the system.
    fn open_external_app(&self, _url: &str) -> NativeDispatch {
        NativeDispatch::Rejected
    }
    /// Atomically replaces one ownership scope's desired injected content.
    /// Queue admission is not native application; the terminal outcome is
    /// reported as [`EngineEvent::UserContentSettled`]. `Rejected` is a
    /// synchronous terminal result for malformed/over-budget candidates,
    /// reserved host scope, lifecycle retirement, or bounded in-flight
    /// backpressure; no settlement follows a rejected dispatch.
    /// Product composition must not call this before the authoritative app
    /// shell has completed bootstrap: pre-bootstrap native facts are rejected
    /// and there is deliberately no inferred user-content snapshot to replay.
    fn set_user_content(
        &self,
        scope: ContentScope,
        generation: UserContentGeneration,
        content: UserContent,
    ) -> NativeDispatch;
    fn set_shortcuts(&self, shortcuts: Vec<Shortcut>);
    /// Prebuild a hidden webview for `partition` so the next open adopts it
    /// instead of paying the renderer spawn. Safe moment: after a page load.
    fn warm_spare(&self, _partition: Partition) {}
    /// Hidden views the shell's idle policy wants suspended. The engine
    /// suspends where it has a primitive (WebView2, WebKit scheduling policy)
    /// and resumes implicitly when a view becomes visible again.
    fn set_dormant(&self, _ids: Vec<ItemId>) {}
    /// Whether `set_dormant` actually suspends hidden renderers here.
    fn suspends_hidden_views(&self) -> bool {
        false
    }
    /// Drops speculative resources during OS pressure; never evicts user work.
    fn set_memory_pressure(&self, _pressure: MemoryPressure) {}
    /// Requests `EngineEvent::PageMemory` for these live pages where the
    /// platform can attribute a renderer process to a page.
    fn sample_page_memory(&self, _ids: Vec<ItemId>) {}
    /// Replaces one profile's Shell-owned logical window/tab routing facts.
    ///
    /// This projection is deliberately incapable of creating a native view.
    /// Platform adapters may bind only tabs for which the engine already owns
    /// the exact live view generation; discarded tabs remain logical entries.
    /// Newer generations supersede older ones, and a rejected dispatch has no
    /// later settlement.
    fn set_extension_browser_surface(&self, _surface: ExtensionBrowserSurface) -> NativeDispatch {
        NativeDispatch::Unsupported
    }
    /// Loads a prepared extension package into the profile's runtime, or
    /// replaces the loaded one for the same install. The outcome arrives as
    /// [`EngineEvent::WebExtensionSettled`].
    fn load_web_extension(&self, _profile: ProfileId, _load: WebExtensionLoad) -> NativeDispatch {
        NativeDispatch::Unsupported
    }
    /// Stops one installed extension; it keeps its stored data.
    fn unload_web_extension(
        &self,
        _profile: ProfileId,
        _install: ExtensionInstallId,
    ) -> NativeDispatch {
        NativeDispatch::Unsupported
    }
    /// Opens an extension's options page in a tab.
    fn open_web_extension_options(
        &self,
        _profile: ProfileId,
        _extension_id: String,
    ) -> NativeDispatch {
        NativeDispatch::Unsupported
    }
    /// Settles a [`EngineEvent::WebExtensionAccessRequested`].
    fn answer_web_extension_access(
        &self,
        _profile: ProfileId,
        _request: u64,
        _allowed: bool,
    ) -> NativeDispatch {
        NativeDispatch::Unsupported
    }
    /// Stops an extension being uninstalled and erases what it stored.
    fn remove_web_extension(&self, _profile: ProfileId, _load: WebExtensionLoad) -> NativeDispatch {
        NativeDispatch::Unsupported
    }
    /// Requests one complete effective toolbar-action cohort for the exact
    /// published logical tab generation. The terminal result arrives as
    /// [`EngineEvent::ExtensionActionsSnapshotSettled`]. This query may read
    /// native action metadata but must never create a tab or popup webview.
    fn request_extension_actions(
        &self,
        _profile: ProfileId,
        _tab: ItemId,
        _surface_generation: crate::extensions::ExtensionBrowserSurfaceGeneration,
    ) -> NativeDispatch {
        NativeDispatch::Unsupported
    }
    /// Invokes one Shell-authored, exact-version toolbar action. Page and
    /// extension content must have no route to this port. The terminal result
    /// arrives as [`EngineEvent::ExtensionActionSettled`].
    fn invoke_extension_action(
        &self,
        _request: crate::extensions::ExtensionActionRequest,
    ) -> NativeDispatch {
        NativeDispatch::Unsupported
    }
    /// Settles one exact native WebExtension browser mutation. The native
    /// adapter retains the platform completion handler behind the
    /// `(profile, request)` correlation pair and invokes it exactly once.
    fn settle_extension_browser_request(
        &self,
        _profile: ProfileId,
        _request: ExtensionBrowserRequestId,
        _settlement: ExtensionBrowserRequestSettlement,
        _first_url_after_reply: Option<(Arc<str>, NavigationRequestId)>,
    ) -> NativeDispatch {
        NativeDispatch::Unsupported
    }
    /// Settles one exact, origin-labelled native page-permission completion.
    ///
    /// The adapter must bind all four coordinates to the same live view and
    /// retained native request. Unknown, stale, navigated, retired, or already
    /// settled identities are rejected and can never target a replacement
    /// view reusing the same logical item id.
    fn settle_page_permission_request(
        &self,
        _profile: ProfileId,
        _item: ItemId,
        _request: PagePermissionRequestId,
        _settlement: PagePermissionRequestSettlement,
    ) -> NativeDispatch {
        NativeDispatch::Unsupported
    }
    /// How a page's element fullscreen is presented on this engine.
    fn fullscreen_presentation(&self) -> FullscreenPresentation {
        FullscreenPresentation::OwnWindow
    }
    /// Asks the page to leave element fullscreen. The result arrives as
    /// [`EngineEvent::FullscreenChanged`]; picture in picture is unaffected.
    fn exit_fullscreen(&self, _id: ItemId) -> NativeDispatch {
        NativeDispatch::Unsupported
    }
    /// Stop capture in the exact committed document. Unsupported engines
    /// expose no live indicator or pretend to stop through page JavaScript.
    fn stop_media_capture(
        &self,
        _item: ItemId,
        _navigation: NavigationPresentationId,
    ) -> NativeDispatch {
        NativeDispatch::Unsupported
    }
    /// Installs one exact, immutable profile-scoped content policy.
    ///
    /// Queue admission is not native application. The terminal result arrives
    /// as [`EngineEvent::ContentRulesSettled`]. A profile has no implicit
    /// allow-all state: callers must install an explicit `AllowAll` generation
    /// before its first view can be created.
    fn install_content_rules(
        &self,
        _profile: ProfileId,
        _generation: ContentPolicyGeneration,
        _rules: Arc<ContentRules>,
    ) -> NativeDispatch {
        NativeDispatch::Unsupported
    }
    /// Sticky process-local signal that the native browser runtime reported a
    /// newer version. `true` never means the running environments adopted the
    /// update: the composition root must use its ordinary ordered shutdown
    /// path and perform a whole-application restart, including privileged
    /// chrome, before clearing this state.
    fn runtime_restart_required(&self) -> bool {
        false
    }
    /// Non-fatal result of the process-start native runtime assessment.
    ///
    /// This is immutable for the current native process generation. It is
    /// computed locally before WebView construction and performs no network,
    /// filesystem, actor, or page-derived work.
    fn runtime_security_advisories(&self) -> RuntimeSecurityAdvisories {
        RuntimeSecurityAdvisories::new()
    }
    /// Permanently tombstones `profile` at the synchronous call boundary,
    /// rejects all future native access to it except cleanup, and schedules
    /// closure of every existing view/context plus asynchronous erasure of
    /// the profile's engine-owned data. The completion is invoked exactly
    /// once.
    ///
    /// Retirement does not depend on native-thread dispatch: even `Failed`
    /// leaves the profile inaccessible for the rest of this engine process.
    /// When dispatch itself fails, the backend must also reject continued
    /// content use globally because existing native pages cannot be proven
    /// closed; callers must terminate unless a retry verifies erasure.
    /// `Failed` may otherwise be retried because the native attempt is
    /// terminal, but it must never be interpreted as successful teardown.
    /// `TimedOut` is only a one-shot report to the caller: retry remains denied
    /// while the old native work might still be running, and a late terminal
    /// callback releases admission without invoking `done` again.
    fn erase_profile_data(
        &self,
        _profile: ProfileId,
        done: Box<dyn FnOnce(ProfileDataErasureOutcome) + Send>,
    );
    /// Close every native view/context on its owning thread. Completion runs
    /// only after native references have been released; composition roots use
    /// this as the final process-lifecycle barrier.
    fn shutdown(&self, done: Box<dyn FnOnce(bool) + Send>) {
        done(true);
    }
}

/// Correlates an explicit shell navigation with a native failure before its
/// identity-bearing main-frame commit.
///
/// Successful commits intentionally do not carry this token: page-initiated
/// navigations, redirects and same-document history changes have no shell
/// request and all use the same authoritative `UrlChanged` path.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct NavigationRequestId(pub u64);

/// Process-local correlation identity for one native page-zoom request.
///
/// Zoom is persisted only after the exact live native generation reports its
/// applied scale. Keeping this distinct from navigation identity prevents a
/// late result from a replaced view from mutating the replacement tab.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ZoomRequestId(pub u64);

/// Process-local correlation identity for one renderer-state discard probe.
/// It is never persisted or accepted from page content.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DiscardProbeId(pub u64);

/// A native browser action whose synchronous platform invocation failed.
///
/// This does not describe page-load completion. Reload/history success still
/// settles through the ordinary navigation callbacks; this enum exists so an
/// HRESULT/native refusal is never silently discarded.
/// Why a navigation the person asked for did not load, as a category safe
/// to show them. Never carries native error text.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum NavigationFailureReason {
    Offline,
    HostNotFound,
    Unreachable,
    TimedOut,
    Insecure,
    Other,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum NativeAction {
    Reload,
    GoBack,
    GoForward,
}

/// Terminal native state for one requested content-policy generation.
///
/// The shape deliberately cannot express contradictory states such as a
/// successful application with a failure, or a failed replacement without
/// saying whether a prior generation remains active.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContentRuleSettlement {
    Applied {
        generation: ContentPolicyGeneration,
    },
    Retained {
        generation: ContentPolicyGeneration,
        failure: ContentRuleApplyFailure,
    },
    Unavailable {
        failure: ContentRuleApplyFailure,
    },
}

type NativeTabDecision = Box<dyn FnOnce(bool) + Send>;
struct NativeTabAdoptionInner(std::sync::Mutex<Option<NativeTabDecision>>);
#[derive(Clone)]
pub struct NativeTabAdoption(Arc<NativeTabAdoptionInner>);
impl NativeTabAdoption {
    pub fn new(done: impl FnOnce(bool) + Send + 'static) -> Self {
        Self(Arc::new(NativeTabAdoptionInner(std::sync::Mutex::new(
            Some(Box::new(done)),
        ))))
    }
    pub fn finish(&self, accepted: bool) {
        let done = self
            .0
             .0
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take();
        if let Some(done) = done {
            done(accepted);
        }
    }
}
impl Drop for NativeTabAdoptionInner {
    fn drop(&mut self) {
        if let Some(done) = self
            .0
            .get_mut()
            .unwrap_or_else(|error| error.into_inner())
            .take()
        {
            done(false);
        }
    }
}
impl std::fmt::Debug for NativeTabAdoption {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("NativeTabAdoption")
    }
}
impl PartialEq for NativeTabAdoption {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WebExtensionAccessRequest {
    pub profile: ProfileId,
    pub request: u64,
    pub extension_id: String,
    pub warnings: Vec<String>,
    pub permissions: Vec<String>,
    pub patterns: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum EngineEvent {
    /// A native browser environment reported that a newer runtime is
    /// available. This is a process-global, sticky notification: recycling a
    /// content profile alone cannot update privileged chrome and must not be
    /// presented as successful adoption.
    RuntimeRestartRequired,
    /// One exact native content-policy installation attempt settled.
    ///
    /// Replacement failure reports `Retained` with the prior known-good
    /// generation. `Unavailable` means no explicit policy is active, so the
    /// engine continues to reject view creation for this profile.
    ///
    /// On WebKit, native rule-list changes govern future resource loads and
    /// navigations; settlement is not a claim that resources already loaded
    /// by the current document were retroactively filtered.
    ContentRulesSettled {
        profile: ProfileId,
        requested: ContentPolicyGeneration,
        settlement: ContentRuleSettlement,
    },
    /// One exact user-content replacement settled. A retained result means
    /// the prior generation remains the only authoritative native set. The
    /// shell's bounded mailbox may retain only the newest settlement per
    /// scope, so owners reconcile by monotonic `requested` generation rather
    /// than waiting independently on every superseded intermediate event.
    UserContentSettled {
        scope: ContentScope,
        requested: UserContentGeneration,
        settlement: UserContentSettlement,
    },
    /// A native WebExtension context requested a mutation of Shell-owned
    /// browser state. The engine retains and times out the exact platform
    /// completion; the Shell must answer through
    /// [`Engine::settle_extension_browser_request`].
    ExtensionBrowserRequested {
        request: ExtensionBrowserRequest,
    },
    /// The exact native tabs.create completion has returned to its extension.
    /// Shell may now dispatch this previously admitted first URL for the
    /// still-owned logical tab. It grants no committed document or URL.
    ExtensionCreatedTabReplied {
        profile: ProfileId,
        request: ExtensionBrowserRequestId,
        tab: ItemId,
        url: Arc<str>,
        intent: NavigationRequestId,
    },
    /// A native extension document's browser-owned tab guest was closed or
    /// failed admission. The Shell must rejoin both fields to its typed tab
    /// marker before removing it; the event carries no page URL authority.
    ExtensionPageClosed {
        profile: ProfileId,
        id: ItemId,
    },
    /// Trusted native page metadata for an already broker-bound extension
    /// tab. The Shell must join profile and typed marker before projection;
    /// extension URLs are deliberately absent from this browser-owned event.
    ExtensionPageChanged {
        profile: ProfileId,
        id: ItemId,
        title: String,
        loading: bool,
        can_go_back: bool,
        can_go_forward: bool,
    },
    /// Terminal response to one exact effective action-cohort query.
    ExtensionActionsSnapshotSettled {
        profile: ProfileId,
        tab: ItemId,
        surface_generation: crate::extensions::ExtensionBrowserSurfaceGeneration,
        settlement: crate::extensions::ExtensionActionSnapshotSettlement,
    },
    /// Terminal response to one exact trusted toolbar invocation.
    ExtensionActionSettled {
        profile: ProfileId,
        request: crate::extensions::ExtensionActionRequestId,
        settlement: crate::extensions::ExtensionActionSettlement,
    },
    /// Coalescible native notification that one or more effective actions for
    /// this profile changed. It carries no native or extension identity; the
    /// Shell responds by requesting a fresh exact replacement cohort.
    ExtensionActionsInvalidated {
        profile: ProfileId,
    },
    WebExtensionSettled {
        profile: ProfileId,
        install: ExtensionInstallId,
        result: Result<WebExtensionLoaded, String>,
    },
    /// An extension asked at run time for access the user must approve;
    /// answered through [`EnginePort::answer_web_extension_access`].
    WebExtensionAccessRequested(Box<WebExtensionAccessRequest>),
    /// A native command matched the reserved browser-action shortcut for one
    /// exact published runtime and resident tab. The event carries no popup
    /// geometry: Shell must rejoin it to the current action snapshot and ask
    /// privileged chrome for the already-rendered browser-owned button anchor.
    ExtensionActionShortcutRequested {
        runtime: crate::extensions::ExtensionRuntimeInstance,
        tab: ItemId,
        surface_generation: crate::extensions::ExtensionBrowserSurfaceGeneration,
    },
    TitleChanged {
        id: ItemId,
        title: String,
    },
    UrlChanged {
        id: ItemId,
        url: String,
    },
    /// The exact committed URL has already been delivered ahead of this
    /// event. After applying that URL to privileged chrome, the shell returns
    /// this opaque identity immediately to authorize the first presentation.
    PresentationPending {
        id: ItemId,
        navigation: NavigationPresentationId,
        /// Exact canonical URL emitted through `UrlChanged` immediately
        /// before this token. The shell must match both facts before it can
        /// acknowledge presentation; callback/queue order alone is not an
        /// authorization boundary.
        url: String,
    },
    /// Native completion re-drives the same exact presentation fact. This is
    /// an idempotent recovery path when a bounded queue coalesced `Pending`;
    /// it is not a prerequisite or an intentional first-paint delay.
    PresentationReady {
        id: ItemId,
        navigation: NavigationPresentationId,
        /// Same committed URL bound to `navigation`. This is repeated because
        /// a bounded lifecycle queue may coalesce `Pending` into `Ready`.
        url: String,
    },
    NavigationFailed {
        id: ItemId,
        request: NavigationRequestId,
    },
    /// Why the view's current main-frame navigation failed, reported before
    /// its failure settles, only where the engine can tell (macOS; WebView2
    /// shows its own error pages).
    NavigationFailureReported {
        id: ItemId,
        reason: NavigationFailureReason,
    },
    /// The exact native zoom invocation settled. `applied_scale` is the
    /// adapter's last successfully applied scale for this view generation, so
    /// the newest event remains authoritative even when intermediate results
    /// are coalesced under pressure.
    ZoomSettled {
        id: ItemId,
        request: ZoomRequestId,
        applied_scale: f64,
        succeeded: bool,
    },
    /// A reload/history platform call returned an error. An `Ok` call is only
    /// native invocation success, never a claim that navigation completed.
    NativeActionFailed {
        id: ItemId,
        action: NativeAction,
    },
    LoadingChanged {
        id: ItemId,
        loading: bool,
    },
    MediaCaptureChanged {
        id: ItemId,
        navigation: NavigationPresentationId,
        state: MediaCaptureState,
    },
    /// The page entered or left element fullscreen. At most one view per
    /// engine is reported active at a time.
    FullscreenChanged {
        id: ItemId,
        active: bool,
    },
    FaviconPixels {
        id: ItemId,
        page_url: String,
        rgba: Vec<u8>,
    },
    /// The same renderer-side discovery as [`EngineEvent::FaviconPixels`],
    /// run on a Work page the agent read. It names no item: the icon is
    /// cached by the page's origin under the profile the page ran in.
    WorkPageFavicon {
        profile: ProfileId,
        page_url: String,
        rgba: Vec<u8>,
    },
    /// Positive results have already passed exact native-view generation,
    /// navigation-epoch, fixed-schema DOM-state, and (where available)
    /// native audio-state checks. The shell still owns the final visibility
    /// and idle/budget revalidation.
    DiscardSafety {
        id: ItemId,
        probe: DiscardProbeId,
        can_discard: bool,
    },
    ViewDiscarded {
        id: ItemId,
        profile: ProfileId,
        probe: DiscardProbeId,
    },
    /// Final native safety/restoration checks vetoed a requested discard.
    /// The exact resident view remains alive; this is not a close acknowledgement.
    ViewDiscardRefused {
        id: ItemId,
        profile: ProfileId,
        probe: DiscardProbeId,
    },
    /// Physical footprint of the renderer process serving one live page.
    PageMemory {
        id: ItemId,
        profile: ProfileId,
        bytes: u64,
    },
    NavState {
        id: ItemId,
        can_go_back: bool,
        can_go_forward: bool,
    },
    /// The engine already owns this fully configured native child. Shell must
    /// adopt the exact id or reject the lease; it must never replay the URL.
    NativeTabOpened {
        id: ItemId,
        child: ItemId,
        foreground: bool,
        adoption: NativeTabAdoption,
    },
    NativeTabCloseRequested {
        id: ItemId,
    },
    PageOpenBlocked {
        id: ItemId,
        /// The refused page, only when it is an ordinary web address.
        url: Option<String>,
    },
    /// The page asked to open a link in another application. Nothing is
    /// opened until the person allows it.
    ExternalAppRequested {
        id: ItemId,
        url: String,
        app: Option<String>,
    },
    /// A top-level load was shut because a focus round is running.
    FocusBlocked {
        id: ItemId,
        url: String,
    },
    LinkedDownloadStarted {
        id: ItemId,
    },
    NewWindowRequested {
        id: ItemId,
        url: String,
    },
    PermissionRequested {
        id: ItemId,
        profile: ProfileId,
        request: PagePermissionRequest,
    },
    DownloadRequested {
        id: ItemId,
        url: String,
    },
    ViewCreationFailed {
        id: ItemId,
    },
    /// The engine has already revoked and physically removed every listed
    /// native controller from the exited browser-process generation. The
    /// shell may recreate an id and must not issue a second id-only close.
    ProfileProcessExited {
        profile: ProfileId,
        ids: Vec<ItemId>,
    },
    /// The exact native-view generation was revoked and physically removed
    /// before this event was emitted. The shell may recreate `id` and must not
    /// send a cleanup close that could race the replacement generation.
    Crashed {
        id: ItemId,
    },
    Captured {
        id: ItemId,
        png: Vec<u8>,
    },
    HtmlExtracted {
        id: ItemId,
        html: String,
        truncated: bool,
    },
    /// Where a find in `id` landed. `query` names the search it answers, so a
    /// result for text since replaced can be told apart.
    FindResult {
        id: ItemId,
        query: String,
        matches: u32,
        /// One-based position of the current match, when the engine knows it.
        active: Option<u32>,
    },
    SplitChanged {
        window: WindowId,
        tree: Pane,
    },
    ShortcutPressed {
        item: ItemId,
        command: String,
    },
}

#[cfg(test)]
mod user_content_tests {
    use super::*;

    fn script(id: u128) -> UserScript {
        let principal = ScriptPrincipal::Userscript(UserscriptId::from(id + 1_000));
        UserScript {
            id: ScriptId::from(id),
            owner: ScriptOwner::Principal(principal),
            source: "document.documentElement.dataset.zephium = '1'".into(),
            world: World::Isolated(principal),
            matches: MatchSet::all_urls(),
            run_at: RunAt::DocumentStart,
            all_frames: false,
        }
    }

    #[test]
    fn user_content_rejects_cross_principal_worlds() {
        let mut candidate = script(1);
        candidate.world = World::Isolated(ScriptPrincipal::Userscript(UserscriptId::from(9_999)));
        let failure = UserContent {
            scripts: vec![candidate],
            styles: Vec::new(),
        }
        .validate()
        .unwrap_err();
        assert!(matches!(
            failure,
            UserContentApplyFailure::Scripts(refusals)
                if refusals == vec![UserScriptRefusal {
                    registration: script(1).key(),
                    reason: UserScriptRefusalReason::OwnerWorldMismatch,
                }]
        ));
    }

    #[test]
    fn user_content_rejects_duplicate_ids_across_scripts_and_styles() {
        let script = script(2);
        let failure = UserContent {
            scripts: vec![script.clone()],
            styles: vec![UserStyle {
                id: ScriptId::from(2),
                owner: script.owner,
                css: "html { color: black; }".into(),
                matches: MatchSet::all_urls(),
                all_frames: false,
            }],
        }
        .validate()
        .unwrap_err();
        assert!(matches!(
            failure,
            UserContentApplyFailure::Scripts(refusals)
                if refusals.iter().any(|refusal| refusal.registration == script.key()
                    && refusal.reason == UserScriptRefusalReason::DuplicateId)
        ));
    }

    #[test]
    fn user_content_rejects_embedded_nul_before_native_conversion() {
        let mut script = script(3);
        script.source = "prefix\0suffix".into();
        let style = UserStyle {
            id: ScriptId::from(4),
            owner: script.owner,
            css: "html { color: red; }\0html { color: green; }".into(),
            matches: MatchSet::all_urls(),
            all_frames: false,
        };
        let expected = vec![
            UserScriptRefusal {
                registration: script.key(),
                reason: UserScriptRefusalReason::EmbeddedNul,
            },
            UserScriptRefusal {
                registration: style.key(),
                reason: UserScriptRefusalReason::EmbeddedNul,
            },
        ];

        let failure = UserContent {
            scripts: vec![script],
            styles: vec![style],
        }
        .validate()
        .unwrap_err();

        assert_eq!(failure, UserContentApplyFailure::Scripts(expected));
    }

    #[test]
    fn user_content_checks_count_before_walking_untrusted_entries() {
        let content = UserContent {
            scripts: (0..=MAX_USER_SCRIPTS_PER_SCOPE)
                .map(|index| script(index as u128 + 10))
                .collect(),
            styles: Vec::new(),
        };
        assert_eq!(
            content.validate(),
            Err(UserContentApplyFailure::TooManyScripts)
        );
    }

    #[test]
    fn script_identity_is_owner_qualified() {
        let first = script(50);
        let mut second = script(50);
        let second_principal = ScriptPrincipal::Extension(ExtensionInstallId::from(99_999));
        second.owner = ScriptOwner::Principal(second_principal);
        second.world = World::Isolated(second_principal);
        assert_ne!(first.key(), second.key());
        assert!(UserContent {
            scripts: vec![first, second],
            styles: Vec::new(),
        }
        .validate()
        .is_ok());
    }

    #[test]
    fn durable_principal_survives_edits_but_not_delete_and_reinstall() {
        let original = script(60);
        let mut edited = original.clone();
        edited.source = "globalThis.edited = true".into();
        assert_eq!(original.key(), edited.key());

        let reinstalled_principal = ScriptPrincipal::Userscript(UserscriptId::from(61_001));
        let mut reinstalled = edited;
        reinstalled.owner = ScriptOwner::Principal(reinstalled_principal);
        reinstalled.world = World::Isolated(reinstalled_principal);

        assert_eq!(original.id, reinstalled.id);
        assert_ne!(original.key(), reinstalled.key());
    }

    #[test]
    fn per_owner_script_count_is_bounded_below_scope_count() {
        let principal = ScriptPrincipal::Userscript(UserscriptId::from(50_000));
        let content = UserContent {
            scripts: (0..=MAX_USER_SCRIPTS_PER_OWNER)
                .map(|index| {
                    let mut script = script(index as u128 + 1_000);
                    script.owner = ScriptOwner::Principal(principal);
                    script.world = World::Isolated(principal);
                    script
                })
                .collect(),
            styles: Vec::new(),
        };
        assert_eq!(
            content.validate(),
            Err(UserContentApplyFailure::OwnerBudgetExceeded)
        );
    }

    #[test]
    fn retained_budget_counts_compiled_patterns_and_css_expansion() {
        let principal = ScriptPrincipal::Userscript(UserscriptId::from(123));
        let styles = (0..12)
            .map(|index| UserStyle {
                id: ScriptId::from(index + 10_000),
                owner: ScriptOwner::Principal(principal),
                css: std::iter::repeat_n('\u{1f}', MAX_USER_STYLE_BYTES)
                    .collect::<String>()
                    .into(),
                matches: MatchSet::all_urls(),
                all_frames: false,
            })
            .collect();
        let content = UserContent {
            scripts: Vec::new(),
            styles,
        };
        assert_eq!(
            content.validate(),
            Err(UserContentApplyFailure::OwnerBudgetExceeded)
        );
    }
}

#[cfg(test)]
mod native_tab_adoption_tests {
    use super::*;
    #[test]
    fn abandoned_native_tabs_are_rejected_once_after_the_last_clone() {
        let (send, receive) = std::sync::mpsc::channel();
        let lease = NativeTabAdoption::new(move |accepted| send.send(accepted).unwrap());
        let retained = lease.clone();
        drop(lease);
        assert!(receive.try_recv().is_err());
        drop(retained);
        assert!(!receive.recv().unwrap());
        assert!(receive.try_recv().is_err());
    }
    #[test]
    fn a_settled_adoption_cannot_be_rejected_by_a_late_clone() {
        let (send, receive) = std::sync::mpsc::channel();
        let lease = NativeTabAdoption::new(move |accepted| send.send(accepted).unwrap());
        let retained = lease.clone();
        lease.finish(true);
        retained.finish(false);
        drop(lease);
        drop(retained);
        assert!(receive.recv().unwrap());
        assert!(receive.try_recv().is_err());
    }
}
