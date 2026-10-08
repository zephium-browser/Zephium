//! Typed contract between the Rust core and the frame. Pure data, no tauri;
//! the desktop crate maps `Projection` onto typed events and exports the TS
//! bindings. ULIDs cross the boundary as strings.

use serde::{Deserialize, Serialize};
use specta::Type;

mod time;
pub mod work;

pub use time::*;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Type)]
pub struct TabView {
    pub id: String,
    /// Process-local monotonically increasing projection revision, encoded as
    /// fixed-width hexadecimal so JavaScript can compare it without losing
    /// integer precision. Privileged chrome rejects an older per-tab delta
    /// after a newer presentation barrier has applied.
    pub projection_revision: String,
    pub title: String,
    pub url: Option<String>,
    /// Explicit content owner. Internal pages never carry a navigable URL.
    #[serde(default)]
    pub content: TabContentView,
    pub loading: bool,
    /// What the page asked for that waits on the person.
    #[serde(default)]
    #[specta(optional)]
    pub page_request: Option<PageRequestView>,
    /// Transient native residency state. It never replaces the committed URL
    /// or title, and an explicit retry remains a fresh navigation intent.
    #[serde(default)]
    #[specta(optional)]
    pub availability: Option<TabAvailability>,
    /// The last navigation the person asked for that did not load, until the
    /// next attempt or commit. Never carries native error text.
    #[serde(default)]
    #[specta(optional)]
    pub failure: Option<TabFailure>,
    pub can_go_back: bool,
    pub can_go_forward: bool,
    pub icon: Option<IconRef>,
    #[serde(default)]
    #[specta(optional)]
    pub capture: Option<MediaCaptureView>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PageRequestView {
    /// Open a link in another application. `app` is its name when the system
    /// knows one; `scheme` names the kind of link otherwise.
    ExternalApp {
        site: Option<String>,
        scheme: String,
        app: Option<String>,
    },
    /// A new tab the page tried to open; `host` is set when it can be opened.
    Popup { host: Option<String> },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum PageRequestAnswer {
    Allow,
    AlwaysAllow,
    Dismiss,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum CaptureDeviceStateView {
    None,
    Active,
    Muted,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct MediaCaptureView {
    pub navigation_id: String,
    pub camera: CaptureDeviceStateView,
    pub microphone: CaptureDeviceStateView,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct TabFailure {
    pub url: String,
    pub reason: TabFailureReason,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum TabFailureReason {
    Offline,
    HostNotFound,
    Unreachable,
    TimedOut,
    Insecure,
    Other,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum TabAvailability {
    Sleeping,
    WaitingForCapacity { url: String },
    BlockedByCapacity { url: String },
}

/// Browser chrome's bounded tab renderer choice. Future extension-owned
/// documents can add a separate variant without treating them as page URLs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum TabContentView {
    #[default]
    Web,
    Settings,
    Extensions,
    ExtensionOwned,
}

/// Names a cached site icon without carrying its pixels. Chrome keeps rasters
/// by origin and repaints only when `revision` changes, so a projection costs
/// a short string per tab instead of a five-kilobyte raster.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct IconRef {
    pub origin: String,
    pub revision: String,
}

/// One site icon: canonical base64 of exactly 32x32 RGBA bytes. Chrome never
/// decodes a page-controlled image format.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct FaviconEntry {
    pub origin: String,
    pub revision: String,
    pub rgba: String,
}

/// Which privileged webview a raster is destined for. Each keeps its own
/// cache, so delivery is tracked per surface rather than broadcast.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum IconSurface {
    Chrome,
    Panel,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct FaviconsView {
    pub surface: IconSurface,
    pub profile_id: String,
    pub entries: Vec<FaviconEntry>,
}

/// Non-authorizing identity for one live extension runtime. Privileged chrome
/// may echo this value only as part of an action gesture; the Shell rejoins it
/// to the focused profile, active tab, browser-surface generation, and current
/// action revision before any native work is admitted.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct ExtensionActionRuntimeView {
    pub install_id: String,
    /// Process-local nonzero generation, encoded as fixed-width hexadecimal
    /// so JavaScript never rounds a Rust `u64`.
    pub generation: String,
}

/// One effective toolbar action for the focused profile's active logical tab.
/// Labels and badges are bounded before this projection is constructed. The
/// optional icon is the canonical base64 encoding of exactly 32x32 RGBA bytes;
/// privileged chrome performs no extension-controlled image decoding.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct ExtensionActionView {
    pub runtime: ExtensionActionRuntimeView,
    pub revision: String,
    pub label: String,
    pub badge: String,
    pub icon_rgba_base64: Option<String>,
    pub enabled: bool,
    pub presents_popup: bool,
    pub unread_badge: bool,
}

/// Exact replacement action cohort. An empty `actions` collection removes all
/// previously visible actions for this profile/tab. The frame additionally
/// joins `profile_id` and `tab_id` to its current Items projection, so event
/// reordering cannot expose a stale action after focus changes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct ExtensionActionsView {
    pub projection_revision: String,
    pub profile_id: String,
    pub tab_id: Option<String>,
    pub actions: Vec<ExtensionActionView>,
}

/// Closed, sanitized reason why a trusted toolbar gesture could not complete.
/// Native strings, extension content, URLs, and runtime identities are never
/// forwarded through this transient user-notice channel.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum ExtensionActionFailure {
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

/// Actor-ordered, context-bound transient failure notice. The revision lets
/// privileged chrome discard a delayed eval and present each current failure
/// at most once; the profile/tab join prevents a late native refusal from
/// appearing after the user has switched context.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct ExtensionActionFailedView {
    pub projection_revision: String,
    pub profile_id: String,
    pub tab_id: String,
    pub reason: ExtensionActionFailure,
}

/// One actor-ordered request for privileged chrome to invoke the exact
/// browser-owned action button already projected for the focused tab. The
/// frame contributes only that button's current geometry; every authorizing
/// identity is revalidated by Shell and the native host on the normal action
/// path.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct ExtensionActionShortcutView {
    pub projection_revision: String,
    pub profile_id: String,
    pub tab_id: String,
    pub runtime: ExtensionActionRuntimeView,
    pub action_revision: String,
}

/// Closed page capability names rendered by browser-owned chrome. Native
/// permission strings and page-controlled labels never cross this boundary.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum PagePermissionKindView {
    Camera,
    Microphone,
}

/// One exact, foreground page-permission request. Every identity is an opaque
/// stale fence: privileged chrome may only echo it back to the Shell.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct PagePermissionPromptEntryView {
    pub profile_id: String,
    pub item_id: String,
    pub request_id: String,
    pub origin: String,
    pub kinds: Vec<PagePermissionKindView>,
    /// False for ephemeral profiles; chrome must not offer durable policy.
    pub rememberable: bool,
    /// True while an exact durable remember-decision transaction is pending.
    pub processing: bool,
}

/// Exact replacement for the one process-wide page permission surface.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct PagePermissionPromptView {
    pub projection_revision: String,
    pub prompt: Option<PagePermissionPromptEntryView>,
}

/// An extension's run-time request for access, awaiting the user's answer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct WebExtensionAccessRequestView {
    pub profile_id: String,
    pub request: String,
    pub extension_id: String,
    pub warnings: Vec<String>,
    pub permissions: Vec<String>,
    pub patterns: Vec<String>,
}

/// The one retained split group owned by the focused window. Members are
/// normalized references into [`ItemsState::tabs`] in native pane traversal
/// order; geometry and mutable divider ratios remain native-only authority.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Type)]
pub struct SplitGroupView {
    pub members: Vec<String>,
}

/// Focused profile metadata. Profile isolation and lifecycle remain native
/// authority; this view is display-only.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct ProfileView {
    pub id: String,
    pub name: String,
    pub kind: ProfileKindView,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum ProfileKindView {
    Default,
    Named,
    Incognito,
}

/// One ordered space owned by the focused profile.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct SpaceView {
    pub id: String,
    pub name: String,
}

/// Stable display section for a sidebar node. Children inherit their
/// authoritative placement from the native item aggregate.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum SidebarSectionView {
    Favorites,
    Pinned,
    Today,
}

/// A folder carries bounded display metadata; a tab is a normalized reference
/// into [`ItemsState::tabs`]. The node id and tab id intentionally match, but
/// the explicit reference keeps consumers from inferring that invariant.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SidebarNodeKindView {
    Folder { name: String },
    Tab { tab_id: String },
}

/// One pre-order entry in the focused sidebar tree. Parents always precede
/// descendants, sibling order is native aggregate order, and `parent_id` is
/// `None` only for a section root.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct SidebarNodeView {
    pub id: String,
    pub parent_id: Option<String>,
    pub section: SidebarSectionView,
    pub kind: SidebarNodeKindView,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Type)]
pub struct ItemsState {
    pub projection_revision: String,
    /// `None` is reserved for the frame's cold pre-bootstrap state. Native
    /// snapshots are emitted only with an exact focused profile and space.
    pub profile: Option<ProfileView>,
    pub spaces: Vec<SpaceView>,
    pub active_space_id: Option<String>,
    pub nodes: Vec<SidebarNodeView>,
    /// Every tab referenced by `nodes`, in the same pre-order traversal.
    pub tabs: Vec<TabView>,
    pub active: Option<String>,
    pub split_group: Option<SplitGroupView>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Type)]
#[serde(tag = "type")]
pub enum SearchAction {
    OpenNote { id: String },
    ActivateTab { id: String },
    OpenUrl { url: String },
    RunCommand { id: String },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum ToolKind {
    Notes,
    Tasks,
    Ai,
    History,
    Downloads,
    Bookmarks,
    Time,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct SearchContext {
    pub window_id: String,
    pub session_id: String,
    pub request_id: String,
    pub profile_id: String,
    pub space_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PanelIntent {
    /// The hidden renderer has settled its asynchronous work.
    Idle {
        revision: String,
    },
    Search,
    Dismiss,
    /// Hands a destination to the browser, which already hosts its views, and
    /// puts the launcher away.
    Open {
        tool: ToolKind,
    },
}

/// A rectangle in the launcher's own coordinates: CSS pixels from the top-left
/// of its window, which are points on every platform.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, Type)]
pub struct PanelRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// What the launcher is showing, reported by its content. Native sizes the
/// window to `height` and, where it draws the material itself, places the
/// field and result shapes behind the content.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, Type)]
pub struct PanelLayout {
    pub height: f64,
    pub field: PanelRect,
    pub sheet: Option<PanelRect>,
}

impl PanelRect {
    pub fn bounded(&self) -> bool {
        [self.x, self.y, self.width, self.height]
            .iter()
            .all(|value| value.is_finite() && (0.0..=4096.0).contains(value))
    }
}

impl PanelLayout {
    pub fn bounded(&self) -> bool {
        self.height.is_finite()
            && (0.0..=4096.0).contains(&self.height)
            && self.field.bounded()
            && self.sheet.as_ref().is_none_or(PanelRect::bounded)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct PanelState {
    pub window_id: Option<String>,
    pub revision: String,
    pub session_id: String,
    pub visible: bool,
    pub profile_id: Option<String>,
    pub profile_name: Option<String>,
    pub space_id: Option<String>,
    pub error: bool,
    pub corner_radius: u16,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Type)]
pub struct SearchResult {
    pub kind: String,
    pub title: String,
    pub detail: String,
    pub icon: Option<IconRef>,
    pub action: SearchAction,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Type)]
pub struct SearchResults {
    pub pending: bool,
    pub context: Option<SearchContext>,
    pub query: String,
    /// Host the field may complete the typed text to. Native decides what is
    /// confident enough to offer; the field still refuses to apply one that
    /// does not extend exactly what the user has typed.
    pub completion: Option<String>,
    pub results: Vec<SearchResult>,
}

/// One request from a history surface. Reads and deletions share one bounded
/// entry point, as resource calls do.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum HistoryCall {
    /// `before` is the id of the last visit already seen. Row ids and unix
    /// seconds cross as decimal strings; JavaScript never parses a Rust i64.
    Page {
        query: String,
        /// How far back the list reaches. The same scope Clear operates on,
        /// so clearing removes exactly what the reader is looking at.
        range: HistoryRange,
        before: Option<String>,
        limit: u16,
    },
    Forget {
        urls: Vec<String>,
    },
    Clear {
        range: HistoryRange,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum HistoryRange {
    Hour,
    Day,
    Week,
    Everything,
}

impl HistoryRange {
    /// Seconds of history the range covers, or None for all of it.
    pub fn window_seconds(self) -> Option<i64> {
        match self {
            Self::Hour => Some(3600),
            Self::Day => Some(24 * 3600),
            Self::Week => Some(7 * 24 * 3600),
            Self::Everything => None,
        }
    }
}

pub const MAX_HISTORY_PAGE_LIMIT: u16 = 200;
pub const MAX_HISTORY_QUERY_BYTES: usize = 512;
pub const MAX_HISTORY_FORGET_URLS: usize = 100;

impl HistoryCall {
    pub fn validate(&self) -> bool {
        match self {
            Self::Page {
                query,
                before,
                limit,
                ..
            } => {
                query.len() <= MAX_HISTORY_QUERY_BYTES
                    && *limit > 0
                    && *limit <= MAX_HISTORY_PAGE_LIMIT
                    && before
                        .as_ref()
                        .is_none_or(|cursor| cursor.parse::<i64>().is_ok_and(|id| id > 0))
            }
            Self::Forget { urls } => {
                !urls.is_empty()
                    && urls.len() <= MAX_HISTORY_FORGET_URLS
                    && urls
                        .iter()
                        .all(|url| zephium_core::navigation::is_allowed_str(url))
            }
            Self::Clear { .. } => true,
        }
    }
}

/// One recorded visit. Visits are not deduplicated by address: a history list
/// shows every time a page was opened.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct HistoryVisitView {
    pub id: String,
    pub url: String,
    pub title: String,
    pub visited_at: String,
    pub icon: Option<IconRef>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum HistoryResponse {
    Page {
        visits: Vec<HistoryVisitView>,
        /// Cursor for the following page, absent once the list is exhausted.
        next: Option<String>,
    },
    Removed {
        count: u32,
    },
    Error {
        error: HistoryError,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum HistoryError {
    Invalid,
    Unavailable,
    Capacity,
}

/// Where a find in the page in front landed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct FindResultView {
    /// The search this answers; chrome ignores results for older text.
    pub query: String,
    pub matches: u32,
    /// One-based position of the current match, when the engine knows it.
    pub active: Option<u32>,
}

/// Bookmark ids are store row ids; they cross as decimal strings, like
/// history ids, so JavaScript never parses a Rust i64.
pub fn bookmark_id(id: &str) -> Option<i64> {
    id.parse::<i64>()
        .ok()
        .filter(|value| *value > 0 && value.to_string() == id)
}

/// Raw title input chrome may send; the store trims and bounds what it keeps.
pub const MAX_BOOKMARK_TITLE_INPUT_BYTES: usize = 2048;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum BookmarkCall {
    /// A folder's contents and the folders above it; no `folder` is the top
    /// level.
    List {
        folder: Option<String>,
    },
    /// The listing of whichever folder holds `id`, so it can be shown in place.
    Reveal {
        id: String,
    },
    Search {
        query: String,
    },
    AddFolder {
        parent: Option<String>,
        title: String,
    },
    /// A page added by hand; an empty `title` takes the page's site.
    AddLink {
        parent: Option<String>,
        title: String,
        url: String,
    },
    Rename {
        id: String,
        title: String,
    },
    /// Moves `id` into `parent` at `index` among its new siblings.
    Move {
        id: String,
        parent: Option<String>,
        index: u32,
    },
    /// Removes a bookmark, or a folder with everything in it.
    Remove {
        id: String,
    },
}

impl BookmarkCall {
    pub fn validate(&self) -> bool {
        let id = |id: &str| bookmark_id(id).is_some();
        let parent = |parent: &Option<String>| parent.as_deref().is_none_or(id);
        let title =
            |title: &str| !title.trim().is_empty() && title.len() <= MAX_BOOKMARK_TITLE_INPUT_BYTES;
        match self {
            Self::List { folder } => parent(folder),
            Self::Reveal { id: target } => id(target),
            Self::Search { query } => {
                !query.trim().is_empty() && query.len() <= zephium_core::bookmarks::MAX_QUERY_BYTES
            }
            Self::AddFolder {
                parent: at,
                title: name,
            } => parent(at) && title(name),
            Self::AddLink {
                parent: at,
                title: name,
                url,
            } => {
                parent(at)
                    && name.len() <= MAX_BOOKMARK_TITLE_INPUT_BYTES
                    && !url.trim().is_empty()
                    && url.len() <= zephium_core::bookmarks::MAX_URL_BYTES
            }
            Self::Rename {
                id: target,
                title: name,
            } => id(target) && title(name),
            Self::Move {
                id: target,
                parent: at,
                ..
            } => id(target) && parent(at),
            Self::Remove { id: target } => id(target),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct BookmarkView {
    pub id: String,
    pub title: String,
    /// Absent for a folder.
    pub url: Option<String>,
    pub icon: Option<IconRef>,
    /// Direct children, for a folder.
    pub children: u32,
    /// The folder it lives in; absent at the top level.
    pub parent: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct BookmarkCrumb {
    pub id: String,
    pub title: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BookmarkResponse {
    Listing {
        folder: Option<String>,
        /// Folders from the top level down to `folder`, `folder` last.
        path: Vec<BookmarkCrumb>,
        items: Vec<BookmarkView>,
    },
    Results {
        items: Vec<BookmarkView>,
    },
    /// A write applied; `id` names what an add created.
    Saved {
        id: Option<String>,
    },
    Error {
        error: BookmarkError,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum BookmarkError {
    Missing,
    /// The profile holds the maximum, or the folder is nested too deep.
    Full,
    /// A folder cannot move into itself.
    Cycle,
    Invalid,
    Unavailable,
    Capacity,
}

/// Split divider hit-strip in window logical coordinates; the chrome renders
/// these as drag targets on platforms without native stage dividers.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Type)]
pub struct DividerView {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    pub vertical: bool,
}

/// The Work browser pane's applied native hole in window logical coordinates.
/// `presented` is false while the pane's tab has no live view (crash, discard,
/// or a modal prompt that removes content from the stage).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Type)]
pub struct WorkPaneLayout {
    pub tab: String,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    pub presented: bool,
    pub generation: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, Type)]
pub struct WorkPaneRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Type)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WorkPaneTarget {
    Tab { id: String },
    Url { url: String },
}

/// Outcome of a native file import into the profile's media store.
#[derive(Clone, Debug, Serialize, Deserialize, Type)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MediaImportV1 {
    Imported {
        record: Box<zephium_core::resources::ResourceRecord>,
    },
    Cancelled,
    Refused {
        error: zephium_core::resources::ResourceError,
    },
}

/// Outcome of admitting one public image for a subject on the canvas.
#[derive(Clone, Debug, Serialize, Deserialize, Type)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MediaAdmitV1 {
    /// The media element now on the canvas, related to the subject.
    Admitted {
        element: zephium_core::work::WorkElementId,
    },
    Refused {
        error: zephium_core::resources::ResourceError,
    },
}

/// Outcome of admitting one folder the person dropped or named for a canvas.
#[derive(Clone, Debug, Serialize, Deserialize, Type)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WorkFolderAdmitV1 {
    /// The canonical path and display name to place as a Folder element.
    Admitted { path: String, name: String },
    /// Outside the home folder, protected, or missing (`not_a_folder` false),
    /// or an existing path that is not a folder.
    Refused { not_a_folder: bool },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Type)]
pub struct LayoutState {
    pub dividers: Vec<DividerView>,
    pub work_pane: Option<WorkPaneLayout>,
}

/// Immediate result returned by a privileged IPC command. `accepted` with an
/// `operation_id` means the mutation was successfully and non-evictably
/// admitted to the shell's process-local ordered FIFO; it does not claim that
/// later native/store work succeeded or survive a process restart. `accepted`
/// without an id is reserved for a fully applied, privileged-UI-only action
/// such as toggling the launcher.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct OperationAdmission {
    pub operation_id: Option<String>,
    pub accepted: bool,
}

/// The bounded terminal classification the actor can establish while
/// processing an admitted operation. `Deferred` means native work was queued,
/// an exact discard acknowledgement is still required, or a durable write
/// became indeterminate and entered explicit reconciliation; it never means a
/// page load, renderer callback, or unknown store transaction succeeded.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum OperationOutcome {
    Applied,
    NoOp,
    Rejected,
    NativeAdmissionFailed,
    Deferred,
}

/// Stable, non-page-derived detail for an operation outcome. Keeping this an
/// enum prevents native errors, URLs, or attacker-controlled strings from
/// becoming an unbounded privileged IPC/logging surface.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum OperationReason {
    MutationApplied,
    StateUnchanged,
    InvalidScope,
    NoFocusedWindow,
    ItemLimitReached,
    InvalidInput,
    HistoryUnavailable,
    LayoutUnavailable,
    UnsupportedCommand,
    NativeDispatchRejected,
    NativeWorkPending,
    DiscardCompletionPending,
    StoreWorkPending,
    StoreAdmissionRejected,
    StoreConflict,
    StoreOutcomeUnknown,
    StoreReconciliationFailed,
    ContentPolicyApplyFailed,
    ContentPolicySourceUnavailable,
    ContentPolicySourceRefreshPending,
    ContentPolicySourceRefreshFailed,
    ContentPolicySourcesRefreshed,
    ProfileDeletionPolicyRejected,
    ProfileDeletionInProgress,
    ProfileDeletionCompleted,
}

/// The shell has processed an admitted operation in actor order. Consumers
/// reconcile logical effects from authoritative projections. A deferred
/// native navigation still resolves independently through engine events.
/// Long-running profile deletion retains its id internally and emits this
/// disposition exactly once, only after definitive rejection or both durable
/// deletion phases complete; blocker preference mutations likewise retain
/// their id through CAS and exact native settlement. Retry/reconciliation
/// state is never mislabeled as successfully applied.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct OperationDisposition {
    pub operation_id: String,
    pub outcome: OperationOutcome,
    pub reason: OperationReason,
}

/// Process-local reconciliation state for an admitted mutation. Pending and
/// processed entries are retained in a bounded fail-closed desktop ledger;
/// processed entries remain queryable until privileged chrome acknowledges
/// them. `Unknown` means the id was never admitted in this process, was already
/// acknowledged, or belongs to a previous process lifetime.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum OperationStatus {
    Unknown,
    Pending,
    Processed { disposition: OperationDisposition },
}

/// Process-local browser-runtime status. Once `restart_required` becomes true
/// it remains true until the whole application exits; recoverable bounded
/// user-content degradation is projected independently in the same snapshot.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct RuntimeStatus {
    pub restart_required: bool,
    /// The saved session could not be restored this launch: its bytes were
    /// kept in a file and the browser started with fresh tabs.
    pub session_set_aside: bool,
    /// Bounded fail-closed aggregate of ownership scopes whose latest native
    /// user-content observation was not exactly applied. An impossible
    /// over-capacity observation contributes at most one sentinel. No script,
    /// extension, profile, or native failure identity crosses this privileged
    /// projection.
    pub user_content_degraded_scope_count: u16,
    /// Canonically ordered closed-vocabulary set. Rust emits at most five
    /// entries and privileged chrome must replace, never append, projections.
    pub security_advisories: Vec<RuntimeSecurityAdvisory>,
}

/// Non-fatal, process-local classification produced before native WebView
/// construction. Hard admission failures never reach privileged chrome.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeSecurityAdvisoryKind {
    ReviewOverdue,
    UpdateRecommended,
    UnreviewedRuntime,
}

/// Fixed destination of the recommended maintenance action. No page or
/// network response can select this value.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeSecurityUpdateTarget {
    Zephium,
    OperatingSystem,
    BrowserRuntime,
}

/// Sanitized advisory delivered only to privileged main chrome.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct RuntimeSecurityAdvisory {
    pub kind: RuntimeSecurityAdvisoryKind,
    pub update_target: RuntimeSecurityUpdateTarget,
}

/// Effective protection for the focused profile's exact native policy.
/// This is derived in Rust from both desired and retained state. Privileged
/// chrome must not infer protection from a pending preference and accidentally
/// present a retained allow-all generation as active blocking.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum BlockerProtection {
    Disabled,
    Pending,
    Active,
    Degraded,
    Unavailable,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum BlockerPhase {
    Unavailable,
    Uninitialized,
    Compiling,
    Installing,
    Ready,
    Failed,
    Retired,
}

/// Authority of the focused profile's durable blocker preference.
/// `Reconciling` and `Unavailable` are intentionally distinct from native
/// policy state: the browser may still know which generation is installed
/// while refusing to guess what durable preference should replace it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum BlockerPreferenceState {
    Authoritative,
    Updating,
    Reconciling,
    Unavailable,
}

/// Sanitized state of the authenticated filter-package supply chain.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum BlockerSourcePhase {
    NotConfigured,
    DurableActivationUnsupported,
    StorageUnavailable,
    ClockUnsafe,
    Idle,
    Fresh,
    Stale,
    Refreshing,
    Failed,
    Shutdown,
}

/// Authority which admitted the displayed filter package.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum BlockerSourceProvenance {
    ReleaseBundle,
    TufRepository,
    OfficialHttps,
}

/// Stable package-refresh failure category. Endpoint, parser, and native
/// strings are intentionally never forwarded to privileged JavaScript.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum BlockerSourceFailure {
    Transport,
    Metadata,
    Clock,
    Manifest,
    Target,
    License,
    Rollback,
    Storage,
    Catalog,
    Internal,
}

/// Stable diagnostics classification. Native/parser text and filter content
/// never cross the privileged IPC boundary.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum BlockerFailure {
    SitePreferencesUnavailable,
    GenerationExhausted,
    CompilerDispatchRejected,
    CompilerUnavailable,
    CompileSourceUnavailable,
    CompileInvalidSource,
    CompileResourceLimit,
    CompileInternal,
    CompiledArtifactMismatch,
    NativeDispatchRejected,
    NativeUnsupported,
    NativeUnsupportedArtifact,
    NativeInvalidArtifact,
    NativeCompilation,
    NativeInstallation,
    NativeCleanup,
    NativeSuperseded,
    ContradictoryNativeSettlement,
}

/// Exact coverage of the generation which native code proved applied.
/// Counts are bounded far below JavaScript's exact-integer ceiling by the
/// blocker compiler's hard rule limits.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct BlockerRuleCoverage {
    pub source_rules: u32,
    pub accepted_rules: u32,
    pub rejected_rules: u32,
    pub platform_omitted_rules: u32,
    pub platform_approximated_rules: u32,
    pub platform_resource_approximated_rules: u32,
    pub platform_source_kind_approximated_rules: u32,
    pub platform_attribution_approximated_rules: u32,
    pub blocking_rule_entries: u32,
}

/// Volatile, process-local health counters for the exact applied runtime
/// matcher. Decimal strings preserve the full saturating `u64` range in
/// JavaScript. These counters are never persisted and contain no request,
/// origin, URL, profile, or rule identity.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct BlockerRuntimeDiagnostics {
    pub total_decisions: String,
    pub candidate_budget_exhausted: String,
    pub matcher_unavailable: String,
    pub matcher_unprepared: String,
    pub attribution_unavailable: String,
    pub evaluation_errors: String,
}

/// Exact authenticated identities for source-package transition diagnostics.
/// This is boxed in [`BlockerStatusView`] so infrequent debug strings do not
/// inflate every application projection on the shell actor's hot path.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct BlockerSourceIdentities {
    pub package_manifest_sha256: Option<String>,
    pub candidate_revision: Option<String>,
    pub candidate_manifest_sha256: Option<String>,
    pub installed_manifest_sha256: Option<String>,
}

/// Host-initiated picker controls, available only to privileged main chrome.
#[derive(Clone, Debug, Serialize, Deserialize, Type)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum BlockerPickerAction {
    Start,
    Read { session: String },
    Preview { session: String, enabled: bool },
    Stop { session: String },
}

#[derive(Clone, Debug, Serialize, Deserialize, Type)]
pub struct BlockerPickerView {
    pub session: String,
    pub active: bool,
    pub selection: Option<BlockerSelectionView>,
}
#[derive(Clone, Debug, Serialize, Deserialize, Type)]
pub struct BlockerSelectionView {
    pub identity: String,
    pub label: String,
    pub count: u32,
    pub positional: bool,
}

/// Exact privileged site-control context; never accepted from ordinary page IPC.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(deny_unknown_fields)]
pub struct BlockerSiteContext {
    pub profile: String,
    pub tab: String,
    pub site: String,
    pub revision: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct PersonalHideView {
    pub id: String,
    pub label: String,
    pub enabled: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct BlockerSiteView {
    pub context: BlockerSiteContext,
    pub paused: bool,
    pub private_session: bool,
    pub ready: bool,
    pub busy: bool,
    pub hides: Vec<PersonalHideView>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum BlockerSiteAction {
    SaveSelection { session: String, selection: String },
    Retry,
    Pause { paused: bool },
    SetHideEnabled { id: String, enabled: bool },
    RemoveHide { id: String },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct BlockerStatusView {
    pub site: Option<Box<BlockerSiteView>>,
    pub projection_revision: String,
    pub protection: BlockerProtection,
    pub phase: BlockerPhase,
    pub preference: BlockerPreferenceState,
    pub config_revision: Option<String>,
    pub desired_enabled: Option<bool>,
    pub applied_enabled: Option<bool>,
    pub desired_generation: Option<String>,
    pub retained_generation: Option<String>,
    pub failure: Option<BlockerFailure>,
    pub retryable: bool,
    pub retries_remaining: u8,
    /// Boxed with the other diagnostic-only payloads so ordinary projection
    /// queue entries do not carry the full coverage report inline.
    pub applied_coverage: Option<Box<BlockerRuleCoverage>>,
    /// Boxed because the six decimal counters are diagnostic-only and should
    /// not inflate every projection enum value on the actor/UI hot path.
    pub runtime_diagnostics: Option<Box<BlockerRuntimeDiagnostics>>,
    pub source_phase: BlockerSourcePhase,
    pub source_failure: Option<BlockerSourceFailure>,
    pub source_package_revision: Option<String>,
    pub source_installed_revision: Option<String>,
    pub source_package_provenance: Option<BlockerSourceProvenance>,
    pub source_installed_provenance: Option<BlockerSourceProvenance>,
    pub source_identities: Option<Box<BlockerSourceIdentities>>,
    pub source_package_created_unix: Option<String>,
    pub source_package_expires_unix: Option<String>,
    pub source_package_stale: Option<bool>,
    /// Advisory source update cadence. This never downgrades a healthy
    /// release-bundled policy.
    pub source_refresh_due: bool,
    pub source_count: Option<u32>,
    pub source_bytes: Option<u32>,
    pub source_activation_pending: bool,
    pub source_material_repair_pending: bool,
    pub source_material_repair_retry_pending: bool,
    pub source_repair_retry_pending: bool,
    pub source_last_refresh_attempt_unix: Option<String>,
    pub source_refresh_operation: Option<String>,
    /// Authoritative source-policy capability for the focused profile.
    pub can_enable: bool,
    /// Authoritative refresh admission capability for the active supply mode.
    pub can_refresh_sources: bool,
}

impl BlockerStatusView {
    /// Static reconciliation result used only when no actor-owned revision can
    /// be obtained. Revision zero cannot overwrite a real actor projection.
    pub fn unavailable() -> Self {
        Self {
            site: None,
            projection_revision: "00000000000000000000000000000000".into(),
            protection: BlockerProtection::Unavailable,
            phase: BlockerPhase::Unavailable,
            preference: BlockerPreferenceState::Unavailable,
            config_revision: None,
            desired_enabled: None,
            applied_enabled: None,
            desired_generation: None,
            retained_generation: None,
            failure: None,
            retryable: false,
            retries_remaining: 0,
            applied_coverage: None,
            runtime_diagnostics: None,
            source_phase: BlockerSourcePhase::NotConfigured,
            source_failure: None,
            source_package_revision: None,
            source_installed_revision: None,
            source_package_provenance: None,
            source_installed_provenance: None,
            source_identities: None,
            source_package_created_unix: None,
            source_package_expires_unix: None,
            source_package_stale: None,
            source_refresh_due: false,
            source_count: None,
            source_bytes: None,
            source_activation_pending: false,
            source_material_repair_pending: false,
            source_material_repair_retry_pending: false,
            source_repair_retry_pending: false,
            source_last_refresh_attempt_unix: None,
            source_refresh_operation: None,
            can_enable: false,
            can_refresh_sources: false,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct PanelOwner {
    pub private: bool,
    pub window_id: String,
    pub profile_id: String,
    pub profile_name: String,
    pub space_id: String,
}

/// Snapshots for structural changes, single-row deltas for per-tab churn.
#[derive(Clone, Debug)]
pub enum Projection {
    WorkEnvironmentChanged(work::WorkEnvironmentChangedV1),
    WorkChanged(work::WorkChangedV1),
    PanelOwner(PanelOwner),
    Items(ItemsState),
    Tab(TabView),
    Favicons(FaviconsView),
    ExtensionActions(ExtensionActionsView),
    ExtensionActionFailed(ExtensionActionFailedView),
    ExtensionActionShortcut(ExtensionActionShortcutView),
    PagePermissionPrompt(PagePermissionPromptView),
    WebExtensionAccessRequest(WebExtensionAccessRequestView),
    UiCommand(String),
    FindResult(FindResultView),
    Search(SearchResults),
    OpenNote {
        profile: String,
        id: String,
    },
    Layout(LayoutState),
    /// The page on screen is fullscreen and fills the browser window, which
    /// should itself be fullscreen until this turns false. Only engines that
    /// present fullscreen inside the browser window send it.
    HostFullscreen(bool),
    RuntimeStatus(RuntimeStatus),
    BlockerStatus(BlockerStatusView),
    Focus(FocusStatus),
    OperationProcessed(OperationDisposition),
}

/// Shared Rust-owned Notes/Tasks wire model.
pub use zephium_core::resources::{ResourceCall, ResourceReply, ResourceResponse};

#[cfg(test)]
mod blocker_status_tests {
    use super::*;

    #[test]
    fn unavailable_status_is_bounded_and_cannot_supersede_actor_state() {
        let status = BlockerStatusView::unavailable();
        assert_eq!(
            status.projection_revision,
            "00000000000000000000000000000000"
        );
        assert_eq!(status.protection, BlockerProtection::Unavailable);
        assert_eq!(status.phase, BlockerPhase::Unavailable);
        assert_eq!(status.preference, BlockerPreferenceState::Unavailable);
        assert!(status.config_revision.is_none());
        assert!(status.desired_generation.is_none());
        assert!(status.retained_generation.is_none());
        assert!(status.failure.is_none());
        assert!(status.applied_coverage.is_none());
        assert_eq!(status.source_phase, BlockerSourcePhase::NotConfigured);
        assert!(status.source_failure.is_none());
        assert!(status.source_package_revision.is_none());
        assert!(status.source_installed_revision.is_none());
        assert!(status.source_identities.is_none());
        assert!(!status.source_activation_pending);
        assert!(!status.source_material_repair_pending);
        assert!(!status.source_material_repair_retry_pending);
        assert!(!status.source_repair_retry_pending);
        assert!(!status.retryable);
        assert_eq!(status.retries_remaining, 0);
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct BlockerStatsView {
    #[specta(type = f64)]
    pub today: u64,
    #[serde(rename = "last7Days")]
    #[specta(type = f64)]
    pub last_seven_days: u64,
    #[specta(type = Vec<f64>)]
    pub days: [u64; 7],
}

#[cfg(test)]
mod bookmark_tests {
    use super::*;

    #[test]
    fn bookmark_calls_admit_only_canonical_ids_and_bounded_text() {
        assert_eq!(bookmark_id("42"), Some(42));
        for id in ["0", "-1", "042", "4.2", "", "x"] {
            assert_eq!(bookmark_id(id), None, "{id}");
        }
        let valid = [
            BookmarkCall::List { folder: None },
            BookmarkCall::Reveal { id: "3".into() },
            BookmarkCall::Search {
                query: "docs".into(),
            },
            BookmarkCall::AddFolder {
                parent: Some("3".into()),
                title: "Reading".into(),
            },
            BookmarkCall::Move {
                id: "4".into(),
                parent: None,
                index: 9,
            },
            BookmarkCall::AddLink {
                parent: None,
                title: String::new(),
                url: "example.com".into(),
            },
        ];
        assert!(valid.iter().all(BookmarkCall::validate));
        let invalid = [
            BookmarkCall::List {
                folder: Some("top".into()),
            },
            BookmarkCall::Search { query: "  ".into() },
            BookmarkCall::Rename {
                id: "3".into(),
                title: "a".repeat(MAX_BOOKMARK_TITLE_INPUT_BYTES + 1),
            },
            BookmarkCall::AddFolder {
                parent: None,
                title: " ".into(),
            },
            BookmarkCall::Remove { id: "0".into() },
            BookmarkCall::AddLink {
                parent: None,
                title: "Docs".into(),
                url: " ".into(),
            },
        ];
        assert!(!invalid.iter().any(BookmarkCall::validate));
    }
}
