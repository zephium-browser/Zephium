//! Stable application-shell protocol exposed to the desktop composition root.

use std::fmt;
use std::sync::mpsc::SyncSender;
use std::sync::{Arc, Mutex};

use zephium_core::blocker::{ContentPolicyGeneration, ProfileContentPolicyStatus};
use zephium_core::extensions::{
    ExtensionActionRevision, ExtensionPopupAnchor, ExtensionRuntimeInstance,
};
use zephium_core::geometry::{Rect, Size};
use zephium_core::ids::{ItemId, ProfileId};
use zephium_core::ports::blocker::ContentBlocker;
use zephium_core::ports::chrome::Chrome as GeometryChrome;
use zephium_core::ports::engine::{DiscardProbeId, Engine, EngineEvent, NavigationPresentationId};
use zephium_core::ports::store::Store;
use zephium_core::ports::store::{
    PagePermissionCatalogLoadOutcome, PagePermissionCatalogMutationOutcome,
};
use zephium_core::split::Axis;
use zephium_ipc::{BlockerStatusView, Projection, TabView};

use crate::store_reads::StoreReadResult;

#[cfg(feature = "agentic-browser")]
use zephium_agentic::AgentBrowserLifecycle;

pub type SharedEngine = Arc<dyn Engine + Send + Sync>;
pub type SharedStore = Arc<dyn Store + Send + Sync>;
pub type SharedBlocker = Arc<dyn ContentBlocker + Send + Sync>;
pub type SharedChrome = Arc<dyn PresentationChrome + Send + Sync>;
/// Unique application-owned lifecycle authority for the agent browser.
///
/// This exists only in the dormant agentic composition graph. It is consumed
/// before terminal Store and engine teardown and has no cloneable shutdown
/// surface.
#[cfg(feature = "agentic-browser")]
pub type AgentLifecycle = Box<dyn AgentBrowserLifecycle>;
/// Redacted terminal reason delivered to the desktop composition root when
/// the shell can no longer continue safely in the current process.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShellTerminalFailure {
    ProfileDeletionInvariant,
    ActorExitedUnexpectedly,
    /// The saved session or its deletion journal could not be opened. The
    /// store kept it untouched; the person is told so instead of facing an
    /// empty window that cannot do anything.
    SessionUnavailable,
    /// The saved session was written by a newer Zephium and is kept for it.
    SessionFromNewerVersion,
}

impl std::fmt::Display for ShellTerminalFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::ProfileDeletionInvariant => {
                "profile deletion violated a post-retirement invariant"
            }
            Self::ActorExitedUnexpectedly => "application shell actor exited unexpectedly",
            Self::SessionUnavailable => "the saved session could not be opened",
            Self::SessionFromNewerVersion => "the saved session belongs to a newer Zephium",
        })
    }
}

/// One-shot terminal shell handoff. Startup failures invoke it so the desktop
/// can enqueue Shell-owned orderly shutdown; unexpected actor exit invokes it
/// after bounded best-effort cleanup. Platform adapters must dispatch onto
/// their native event loop rather than performing teardown inline.
pub type ShellTerminalFailureCallback = Box<dyn FnOnce(ShellTerminalFailure) + Send>;
pub type EmitFn = Box<dyn Fn(Projection) + Send + Sync>;

/// Exact privileged-chrome work that must complete before one raw document
/// can become visible. The tab projection is carried in the same native eval
/// as the acknowledgement, avoiding an ordering assumption between generic
/// projection delivery and native content presentation.
#[derive(Clone, Debug, PartialEq)]
pub struct ChromePresentation {
    pub settings_visible: bool,
    pub id: ItemId,
    pub navigation: NavigationPresentationId,
    pub url: String,
    pub tab: TabView,
    pub active: Option<ItemId>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChromePresentationDispatch {
    /// The adapter applied and verified the projection synchronously. Used by
    /// deterministic embedders/tests; production native adapters are async.
    Applied,
    /// Callback ownership was accepted. It must report verification success
    /// or failure without blocking its native UI thread.
    Scheduled,
    /// No callback ownership transfer occurred.
    Rejected,
}

pub type ChromePresentationCallback = Box<dyn FnOnce(bool) + Send>;

/// Geometry plus the privileged DOM acknowledgement required by the raw-view
/// anti-spoof boundary.
pub trait PresentationChrome: GeometryChrome {
    fn restore_browser_chrome(
        &self,
        _revision: u64,
        _items: zephium_ipc::ItemsState,
        _done: ChromePresentationCallback,
    ) -> ChromePresentationDispatch {
        ChromePresentationDispatch::Rejected
    }

    fn apply_tab_for_presentation(
        &self,
        presentation: ChromePresentation,
        done: ChromePresentationCallback,
    ) -> ChromePresentationDispatch;
}

/// Terminal result of the ordered application shutdown protocol.
///
/// A retryable failure happens before the native engine is torn down and
/// leaves the actor live. `Unclean` is terminal: either the actor exited
/// without completing the barrier or one of agent lifecycle, Store, blocker,
/// or native cleanup was not proven, so the process must exit unsuccessfully.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShutdownOutcome {
    RetryableFailure,
    Clean,
    Unclean,
}

/// Browser-owned response vocabulary for one exact page capability request.
/// `Always*` is authority to mutate the durable per-profile catalog; `*Once`
/// never writes policy and holds for the asking document until its tab shows
/// another one. `Dismiss` refuses this request only.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PagePermissionPromptDecision {
    AllowOnce,
    AlwaysAllow,
    DenyOnce,
    AlwaysDeny,
    Dismiss,
}

/// Ordered result of a trusted profile content-policy status query.
#[must_use]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContentPolicyStatusQueryOutcome {
    Found(ProfileContentPolicyStatus),
    UnknownProfile,
    /// The query was not admitted or the actor exited before replying.
    Unavailable,
}

/// A bounded browser-owned destination rendered by the existing chrome view.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BrowserPage {
    Work,
    Settings,
    Extensions,
    History,
    Downloads,
    Tasks,
    Notes,
    Time,
    /// The source of the tab in front. Chrome never asks for it by name: a
    /// source tab being in front is what shows it.
    Source,
}

impl BrowserPage {
    pub fn command_id(self) -> &'static str {
        match self {
            Self::Work => "browser.work",
            Self::Settings => "browser.settings",
            Self::Extensions => "browser.extensions",
            Self::History => "browser.history",
            Self::Downloads => "browser.downloads",
            Self::Tasks => "browser.tasks",
            Self::Notes => "browser.notes",
            Self::Time => "browser.time",
            Self::Source => "browser.source",
        }
    }

    /// The feature switch the page belongs to, if any.
    pub fn feature(self) -> Option<&'static str> {
        match self {
            Self::Work => Some("work"),
            Self::Tasks => Some("tasks"),
            Self::Notes => Some("notes"),
            Self::Time => Some("time"),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TabMetadata {
    pub id: ItemId,
    pub title: String,
    pub url: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum WorkPaneTarget {
    Tab(ItemId),
    Url(String),
}

/// What the tab menu can do to a tab, beyond the commands it shares with the
/// rest of the browser.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TabAction {
    /// Opens the same page in a new tab right after it.
    Duplicate,
    Bookmark,
    /// Closes every open tab of the space but this one.
    CloseOthers,
    /// Closes the open tabs listed after this one.
    CloseBelow,
}

/// Space mutations from chrome, menus and keys. The shell checks each
/// against the focused profile.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SpaceCommand {
    Create {
        name: String,
        look: zephium_core::spaces::SpaceLook,
    },
    Update {
        id: zephium_core::ids::SpaceId,
        name: String,
        look: zephium_core::spaces::SpaceLook,
    },
    Delete(zephium_core::ids::SpaceId),
    Reorder {
        id: zephium_core::ids::SpaceId,
        before: Option<zephium_core::ids::SpaceId>,
    },
    Switch(zephium_core::ids::SpaceId),
    /// The next (1) or previous (-1) space, wrapping.
    Step(i8),
    MoveTab {
        tab: ItemId,
        space: zephium_core::ids::SpaceId,
    },
    /// Moves the tab in front, for the menu bar.
    MoveActive(zephium_core::ids::SpaceId),
}

/// What the person chose for a page's request to open another application
/// or a blocked new tab.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PageRequestDecision {
    Allow,
    /// Allow, and let this site open the same kind of link without asking
    /// again until Zephium quits.
    AlwaysAllow,
    Dismiss,
}

#[derive(Clone, Debug)]
pub enum Command {
    WorkDocument(crate::WorkDocumentSubmission),
    WorkChanged(zephium_ipc::work::WorkChangedV1),
    WorkEnvironmentChanged(zephium_ipc::work::WorkEnvironmentChangedV1),
    #[cfg(feature = "work-execution")]
    AttachRetainedWork(crate::work_resources::product::RetainedWorkAttachment),
    ResourceCall {
        expected_profile: ProfileId,
        call: Arc<zephium_core::resources::ResourceCall>,
        done: ResourceCompletion,
    },
    /// Bytes the desktop read on the user's behalf; the store sniffs, bounds,
    /// and mints the Media resource. Never constructed from IPC.
    ImportMedia {
        expected_profile: ProfileId,
        import: Box<zephium_core::resources::MediaImport>,
        done: ResourceCompletion,
    },
    DownloadCall {
        expected_profile: ProfileId,
        call: Box<zephium_core::downloads::DownloadCall>,
        done: zephium_core::downloads::DownloadCompletion,
    },
    HistoryCall {
        expected_profile: ProfileId,
        call: Box<zephium_ipc::HistoryCall>,
        done: HistoryCompletion,
    },
    BrowsingDataCall {
        expected_profile: ProfileId,
        call: Box<zephium_ipc::BrowsingDataCall>,
        done: BrowsingDataCompletion,
    },
    BookmarkCall {
        expected_profile: ProfileId,
        call: Box<zephium_ipc::BookmarkCall>,
        done: BookmarkCompletion,
    },
    /// The document a source tab shows, for the page that renders it.
    PageSource {
        id: ItemId,
        done: SourceCompletion,
    },
    /// What the Back and Forward buttons of a tab can reach, for their menu.
    HistoryList {
        id: ItemId,
        done: HistoryListCompletion,
    },
    GoToHistoryEntry {
        id: ItemId,
        offset: i32,
        url: String,
    },
    /// The focused profile's bookmarks or history as the text of a file.
    Export {
        kind: zephium_core::export::ExportKind,
        done: ExportCompletion,
    },
    TimeCall {
        expected_profile: ProfileId,
        call: Box<zephium_ipc::TimeCall>,
        done: TimeCompletion,
    },
    ZoomCall {
        expected_profile: ProfileId,
        call: Box<zephium_ipc::ZoomCall>,
        done: ZoomCompletion,
    },
    /// A time report the store read, coming back for icons.
    TimeReportRead {
        profile: ProfileId,
        report: Option<zephium_core::time::TimeReport>,
        done: TimeCompletion,
    },
    /// Whether a Zephium window is the one the person is using.
    SetAppActive(bool),
    /// Whether the machine is awake and unlocked with its display on.
    SetSystemAwake(bool),
    /// Native pressure notifications only; pages cannot choose resource policy.
    SetMemoryPressure(zephium_core::ports::engine::MemoryPressure),
    Focus(zephium_ipc::FocusControl),
    /// The running focus session reached its next change.
    FocusWake,
    /// A quiet tab's speaker is due to go.
    AudioWake,
    /// Writes what was read from another browser into the focused profile,
    /// restoring the session first when it has not been yet (onboarding).
    Import {
        work: Box<crate::store_reads::ImportWork>,
        done: ImportCompletion,
    },
    /// Hands the shell its notes service, once, after startup.
    AttachNotes(NotesAttachment),
    NoteCall {
        expected_profile: ProfileId,
        call: Arc<zephium_core::notes::NoteCall>,
        done: NoteCompletion,
    },
    #[cfg(feature = "work-execution")]
    AttachWork(crate::work::WorkAttachment),
    #[cfg(feature = "work-execution")]
    AdmitWork(crate::work::WorkSubmission),
    #[cfg(feature = "work-execution")]
    WorkControl(Box<crate::work::WorkCommand>),
    #[cfg(feature = "work-execution")]
    WorkWake,
    /// A privileged user mutation with an externally visible admission and
    /// actor-order disposition identity. Engine callbacks and replaceable UI
    /// facts never use this wrapper.
    Operation {
        operation_id: String,
        command: Box<Command>,
    },
    Bootstrap,
    Open,
    Activate(ItemId),
    Close(ItemId),
    SetTabEssential {
        id: ItemId,
        essential: bool,
        before: Option<ItemId>,
    },
    /// Keeps one of onboarding's catalog sites, named by its catalog id.
    KeepSite(String),
    /// Lets go of a catalog site onboarding kept, named by its catalog id.
    ReleaseSite(String),
    RenameFocusedProfile(String),
    /// A named profile with its own space, engine store and content policy.
    CreateProfile {
        name: String,
        avatar: zephium_core::profiles::ProfileAvatar,
        photo: Option<Arc<Vec<u8>>>,
        /// Shows the new profile in the window once it exists.
        switch: bool,
    },
    /// Shows a persistent profile in the window.
    SwitchProfile(ProfileId),
    RenameProfile {
        profile: ProfileId,
        name: String,
    },
    /// A photo style carries its already downscaled bytes.
    SetProfileAvatar {
        profile: ProfileId,
        avatar: zephium_core::profiles::ProfileAvatar,
        photo: Option<Arc<Vec<u8>>>,
    },
    ProfileAvatarPhoto {
        profile: ProfileId,
        reply: SyncSender<Option<Vec<u8>>>,
    },
    Profiles {
        reply: SyncSender<Option<zephium_ipc::ProfilesView>>,
    },
    Navigate {
        id: ItemId,
        input: String,
    },
    Reload(ItemId),
    /// The person's answer to what the tab's page asked for.
    AnswerPageRequest {
        id: ItemId,
        decision: PageRequestDecision,
    },
    /// The person's answer to the tab's sign-in prompt: credentials, or
    /// None to cancel. Credentials are wiped when the command drops.
    AnswerHttpAuth {
        id: ItemId,
        request: u32,
        credentials: Option<zephium_core::http_auth::HttpCredentials>,
    },
    /// The person chose to continue past the certificate the tab refused,
    /// on a host on their own network, and to load it again.
    ContinuePastCertificate(ItemId),
    GoBack(ItemId),
    GoForward(ItemId),
    SplitWith {
        other: ItemId,
        axis: Axis,
    },
    Unsplit,
    /// Takes one tab out of the focused split, leaving the rest paired.
    LeaveSplit(ItemId),
    /// Something the tab menu does to the one tab it was opened on.
    TabAction {
        id: ItemId,
        action: TabAction,
    },
    /// Something done to the focused profile's spaces.
    Space(Box<SpaceCommand>),
    SetWindowSize(Size),
    /// Whether the OS can currently present the main window. Minimized
    /// windows hide native content views so the engine can lower their memory
    /// priority and, after the normal idle grace, suspend them.
    SetWindowVisible(bool),
    /// OS focus is separate from visibility: background pages may keep playing
    /// audio, but they may not initiate or retain browser-owned device consent.
    SetWindowFocused(bool),
    StopMediaCapture {
        item: ItemId,
        navigation: zephium_core::ports::engine::NavigationPresentationId,
    },
    /// Mutes or unmutes one tab for as long as it is open.
    SetTabMuted {
        id: ItemId,
        muted: bool,
    },
    /// Acts on the media of the tab the sidebar's player shows.
    MediaPlayer {
        id: ItemId,
        action: zephium_ipc::MediaPlayerAction,
    },
    /// The sidebar's width, and whether it changed by a deliberate change of
    /// shape — a toggle, a snap, a tool opening — that the content should
    /// travel with, rather than by a drag that it should simply follow.
    SetSidebarWidth(f64, bool),
    /// The sidebar hides and the page takes its place, or comes back. The
    /// column floats over the page on demand; that raise is native only.
    SetSidebarHidden(bool),
    /// Transient Windows resize feedback only; no width or layout mutation.
    SidebarResizeGuide(Option<f64>),
    /// Whether the expanded sidebar is on screen to show a page-permission
    /// prompt under its address field.
    SetPagePermissionCardHost(bool),
    ShowBrowserPage(Option<BrowserPage>),
    /// Shows the transient Work browser pane over an existing Space tab or a
    /// fresh tab navigated to `Url`. `rect` is window-local and clamped.
    WorkPaneShow {
        target: WorkPaneTarget,
        rect: Rect,
    },
    /// Replaceable geometry fact carrying the pane generation it measured.
    WorkPaneSetRect {
        rect: Rect,
        generation: u32,
    },
    /// Hides the pane; its tab keeps every page state.
    WorkPaneHide,
    BrowserChromeRestored {
        revision: u64,
        applied: bool,
    },
    DragOver {
        point: Option<(f64, f64)>,
    },
    DropTab {
        id: ItemId,
        x: f64,
        y: f64,
    },
    DividerGrab {
        x: f64,
        y: f64,
    },
    DividerDrag {
        x: f64,
        y: f64,
    },
    DividerRelease {
        /// Final pointer position, folded into the same ordered mutation as
        /// release so separate IPC deliveries cannot persist a stale ratio.
        x: Option<f64>,
        y: Option<f64>,
    },
    Run(String),
    /// Trusted browser-chrome toolbar intent. The Shell derives the active tab
    /// and exact browser-surface generation; callers can only echo one action
    /// runtime/revision from the latest privileged projection.
    InvokeExtensionAction {
        runtime: ExtensionRuntimeInstance,
        revision: ExtensionActionRevision,
        anchor: ExtensionPopupAnchor,
    },
    /// Browser-owned response to the exact currently projected foreground
    /// page request. Public composition wraps this in `Operation`.
    RespondToPagePermissionPrompt {
        profile: ProfileId,
        item: ItemId,
        request: zephium_core::permissions::PagePermissionRequestId,
        decision: PagePermissionPromptDecision,
    },
    /// Internal callback from one admitted on-demand durable catalog read.
    PagePermissionCatalogLoaded {
        profile: ProfileId,
        item: ItemId,
        request: zephium_core::permissions::PagePermissionRequestId,
        outcome: Box<PagePermissionCatalogLoadOutcome>,
    },
    /// Internal callback from one admitted remembered-decision mutation.
    PagePermissionCatalogMutated {
        profile: ProfileId,
        item: ItemId,
        request: zephium_core::permissions::PagePermissionRequestId,
        outcome: Box<PagePermissionCatalogMutationOutcome>,
    },
    /// Hard Shell-side bound shorter than the native completion watchdog.
    PagePermissionTimeout {
        profile: ProfileId,
        item: ItemId,
        request: zephium_core::permissions::PagePermissionRequestId,
    },
    /// Replaces the extensions a profile runs.
    SetWebExtensions {
        profile: ProfileId,
        extensions: Vec<zephium_core::ports::engine::WebExtensionLoad>,
    },
    /// Opens an extension's options page in a tab.
    OpenWebExtensionOptions {
        profile: ProfileId,
        extension_id: String,
    },
    /// The user's answer to an extension's run-time access request.
    AnswerWebExtensionAccess {
        profile: ProfileId,
        request: u64,
        allowed: bool,
    },
    /// Uninstalls one extension, erasing what it stored.
    RemoveWebExtension {
        profile: ProfileId,
        extension: Box<zephium_core::ports::engine::WebExtensionLoad>,
    },
    /// The profile an installation from `tab` would go to.
    ResolveWebExtensionTarget {
        tab: Option<ItemId>,
        reply: SyncSender<Option<crate::shell::WebExtensionTarget>>,
    },
    WebExtensionStatus {
        profile: ProfileId,
        reply: SyncSender<
            Vec<(
                zephium_core::ids::ExtensionInstallId,
                crate::shell::WebExtensionStatus,
            )>,
        >,
    },
    SearchSupplementaryFinished {
        context: Box<zephium_ipc::SearchContext>,
        query: String,
    },
    SearchAdditional {
        context: Box<zephium_ipc::SearchContext>,
        query: String,
        results: Vec<zephium_ipc::SearchResult>,
    },
    Search(String),
    SearchScoped {
        query: String,
        context: Box<zephium_ipc::SearchContext>,
    },
    CancelSearch {
        session_id: String,
    },
    RunSearchAction {
        context: Box<zephium_ipc::SearchContext>,
        action: zephium_ipc::SearchAction,
        /// Open an address in a new tab behind the current one and keep the
        /// search alive. Ignored by every other action.
        background: bool,
        /// The tab whose address the launcher was opened to edit: an address
        /// chosen there replaces that tab's page while it is still in front.
        replace: Option<String>,
    },
    OpenUrl {
        input: String,
        new_tab: bool,
    },
    /// Finds text in the page in front; `None` ends the search.
    Find(Option<zephium_core::ports::engine::FindRequest>),
    /// Addresses handed over by another application, already admitted by
    /// `navigation::external_target`. Each opens in a new tab; any that
    /// arrive before the session is restored wait for it.
    OpenExternal(Vec<String>),
    SetAppSetting {
        key: String,
        value: String,
    },
    /// Already-revoked global image preference waiting for exact persistence.
    #[cfg(feature = "work-planning")]
    SetWorkScreenshots(crate::work_screenshots::PreferenceChange),
    /// Permanently removes one inactive named profile through the durable
    /// cross-store deletion coordinator. The profile id comes only from
    /// privileged chrome and is revalidated against authoritative state.
    DeleteProfile(ProfileId),
    /// Explicitly retries one exact failed content-policy generation.
    ///
    /// The failed generation comes from a trusted status query. Requiring it
    /// prevents a duplicated or delayed command from retrying a newer failure
    /// after state has already advanced.
    RetryContentPolicy {
        profile: ProfileId,
        failed_generation: ContentPolicyGeneration,
    },
    /// Changes only the actor-selected focused profile. The operation remains
    /// pending until both the exact durable CAS and native policy generation
    /// settle.
    SetFocusedContentBlockerEnabled(bool),
    ElementPicker {
        context: Box<zephium_ipc::BlockerSiteContext>,
        action: zephium_ipc::BlockerPickerAction,
        reply: SyncSender<Option<zephium_ipc::BlockerPickerView>>,
    },
    ChangeBlockerSite {
        context: Box<zephium_ipc::BlockerSiteContext>,
        action: zephium_ipc::BlockerSiteAction,
    },
    /// Retries the focused profile's exact failed generation without exposing
    /// a profile selector to privileged IPC.
    RetryFocusedContentPolicy {
        failed_generation: ContentPolicyGeneration,
    },
    /// Requests one authenticated source-package refresh. This is a global
    /// browser maintenance operation, not a profile or page capability.
    RefreshContentBlockerSources,
    /// The focused profile's saved site decisions, of one origin when given.
    SitePermissions {
        origin: Option<zephium_core::permissions::PageOrigin>,
        reply: SyncSender<Option<zephium_ipc::SitePermissionsView>>,
    },
    /// Changes or, with None, forgets one decision of the focused profile.
    /// Accepted only as an operation.
    SetSitePermission {
        origin: zephium_core::permissions::PageOrigin,
        kind: zephium_core::permissions::SitePermissionKind,
        decision: Option<zephium_core::permissions::RememberedPagePermission>,
    },
    SitePermissionCatalogLoaded {
        operation_id: String,
        profile: ProfileId,
        origin: zephium_core::permissions::PageOrigin,
        kind: zephium_core::permissions::PagePermissionKind,
        decision: Option<zephium_core::permissions::RememberedPagePermission>,
        outcome: Box<zephium_core::ports::store::PagePermissionCatalogLoadOutcome>,
    },
    SitePermissionSettled {
        operation_id: String,
        applied: bool,
    },
    /// Trusted, bounded actor query. Raw page content has no command bridge
    /// and the desktop layer does not expose this variant over IPC.
    ContentPolicyStatus {
        profile: ProfileId,
        reply: SyncSender<ContentPolicyStatusQueryOutcome>,
    },
    /// Read-only privileged-chrome reconciliation query. The actor chooses the
    /// focused profile and assigns the projection revision; IPC callers cannot
    /// enumerate or select another profile.
    BlockerStatistics {
        profile: ProfileId,
        reply: SyncSender<Option<zephium_ipc::BlockerStatsView>>,
    },
    FocusedContentPolicyStatus {
        reply: SyncSender<BlockerStatusView>,
    },
    /// Trusted Rust-only Work binding query. No caller-selected profile/session.
    #[cfg(feature = "work-execution")]
    WorkProfileBinding {
        reply: SyncSender<crate::AgentWorkProfileReadiness>,
    },
    /// Title and URL of Space tabs owned by `profile`, for context admission.
    /// Never page content; a page read is a separately authorized capability.
    TabMetadata {
        profile: ProfileId,
        ids: Vec<ItemId>,
        reply: SyncSender<Vec<TabMetadata>>,
    },
    /// Title and URL of the open tabs in the focused window of `profile`,
    /// for context the person consented to. Never page content.
    WindowTabs {
        profile: ProfileId,
        reply: SyncSender<Vec<TabMetadata>>,
    },
    /// A page opened behind the person's work in `space`: right after
    /// `after` when that tab is in Today, never focused. Answers the new tab,
    /// or `None` when the profile, space, address or tab limit refuses.
    OpenBackgroundTab {
        profile: ProfileId,
        space: zephium_core::ids::SpaceId,
        after: Option<ItemId>,
        url: String,
        /// Show it at once instead of behind the person's work.
        focus: bool,
        done: TabCompletion,
    },
    /// Hands the shell the anonymous origin prober, once, after startup.
    AttachFaviconProber(FaviconProberAttachment),
    /// Icons for origins shown outside a tab: cached or stored rasters are
    /// delivered to chrome, the rest are queued for the origin probe.
    ProbeFavicons {
        profile: ProfileId,
        origins: Vec<String>,
    },
    /// The prober's single answer for one requested origin: an exact 32x32
    /// RGBA raster, or `None` when the origin gave no usable icon.
    FaviconProbed {
        profile: ProfileId,
        origin: String,
        rgba: Option<Vec<u8>>,
    },
    /// The prober's single answer for one page's icon: the raster, the
    /// address it came from, and the addresses that gave nothing.
    FaviconFetched {
        ticket: u64,
        rgba: Option<Vec<u8>>,
        source: Option<String>,
        failed: Vec<String>,
    },
    /// One-shot deadline for a tab's icon work: attempt 0 is a lost store
    /// read, the maximum is a deferred page check, and the one below it ends
    /// the wait that keeps the previous icon on show.
    FaviconPoll {
        id: ItemId,
        attempt: u8,
    },
    /// Bounded admission retry for one exact committed navigation. Normal
    /// presentation is requested immediately after its URL reaches chrome;
    /// stale identities can never reveal overlapping content.
    PresentationFallback {
        id: ItemId,
        navigation: NavigationPresentationId,
        /// Absolute dispatch-admission bound. Retries and overlapping
        /// navigations cannot move it later.
        hard_deadline: std::time::Instant,
    },
    /// Result of one privileged eval-with-callback presentation barrier. The
    /// callback is untrusted lifecycle timing: the actor revalidates every
    /// field against its current exact pending obligation.
    ChromePresentationApplied {
        id: ItemId,
        navigation: NavigationPresentationId,
        url: String,
        active: Option<ItemId>,
        projection_revision: String,
        applied: bool,
    },
    /// Fail-closed deadline for one exact renderer discard-safety probe.
    DiscardProbeTimeout {
        id: ItemId,
        probe: DiscardProbeId,
    },
    /// Deadline wake for one bounded foreground residency request.
    ViewCapacityRetry(ItemId),
    /// Exact-generation wakeup for a native profile-erasure callback. The
    /// outcome itself stays in a bounded inbox so queue overload cannot lose
    /// the security-critical proof.
    ProfileDeletionReady(ProfileId),
    /// Small wake for one exact compiler result retained in the bounded
    /// profile inbox. The immutable compiled artifact never enters the actor
    /// command queue.
    BlockerReady(ProfileId),
    /// Wake for one token-tagged durable preference mutation or
    /// reconciliation result retained in the bounded blocker inbox.
    BlockerStoreReady(ProfileId),
    /// Exact bounded-backoff retry for an indeterminate durable preference
    /// reconciliation.
    BlockerPreferenceRetry {
        profile: ProfileId,
        token: u64,
    },
    /// Bounded follow-up for an exact user-requested refresh or the reserved
    /// internal activation token. Ordinary refresh scheduling remains on the
    /// low-frequency maintenance heartbeat.
    BlockerCatalogPoll {
        operation: u64,
        attempt: u8,
    },
    /// One bounded-backoff retry for journal reconciliation, native erasure,
    /// or local SQLite finalization.
    ProfileDeletionRetry {
        profile: ProfileId,
        generation: u64,
    },
    /// Completion from the bounded storage-read worker. Every result carries
    /// the exact request generation and is revalidated against current shell
    /// state before it can affect privileged projections.
    StoreRead(StoreReadResult),
    /// Internal one-shot debounce fired by the queue's single timer thread.
    Persist,
    /// Periodic maintenance heartbeat; idle tabs suspend or hibernate even
    /// when no user command arrives.
    Tick,
    Engine(EngineEvent),
    /// Ordered process-boundary barrier. The actor snapshots after every
    /// command already queued ahead of this one, then flushes the store.
    Shutdown {
        deadline: std::time::Instant,
        ack: SyncSender<ShutdownOutcome>,
    },
}

type Completion<T> = Arc<Mutex<Option<Box<dyn FnOnce(T) + Send>>>>;

#[derive(Clone)]
pub struct ResourceCompletion(Completion<zephium_core::resources::ResourceReply>);
impl ResourceCompletion {
    pub fn new(done: impl FnOnce(zephium_core::resources::ResourceReply) + Send + 'static) -> Self {
        Self(Arc::new(Mutex::new(Some(Box::new(done)))))
    }
    pub fn finish(self, reply: zephium_core::resources::ResourceReply) {
        let done = self
            .0
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take();
        if let Some(done) = done {
            done(reply);
        }
    }
}
impl fmt::Debug for ResourceCompletion {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ResourceCompletion")
    }
}

/// Where an icon comes from, for a request the prober answers with
/// [`Command::FaviconFetched`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FaviconPageSource {
    /// Icon addresses the page declared, best first. Each is fetched without
    /// cookies, credentials or referrer; the first that decodes wins.
    Addresses(Vec<String>),
    /// A PNG the web engine already chose for the page.
    Encoded(Vec<u8>),
}

/// One request to the prober.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FaviconProbe {
    /// An origin's icon, answered exactly once with [`Command::FaviconProbed`]
    /// for the same profile and origin.
    Origin { profile: ProfileId, origin: String },
    /// A page's icon, answered exactly once with [`Command::FaviconFetched`]
    /// for the same ticket.
    Page {
        ticket: u64,
        profile: ProfileId,
        origin: String,
        source: FaviconPageSource,
    },
}

/// Fetches icons anonymously and decodes them into the fixed raster.
pub type FaviconProber = Arc<dyn Fn(FaviconProbe) + Send + Sync>;

#[derive(Clone)]
pub struct FaviconProberAttachment(pub FaviconProber);
impl fmt::Debug for FaviconProberAttachment {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("FaviconProberAttachment")
    }
}

#[derive(Clone)]
pub struct NotesAttachment(pub zephium_core::ports::notes::SharedNotes);
impl fmt::Debug for NotesAttachment {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("NotesAttachment")
    }
}

#[derive(Clone)]
pub struct NoteCompletion(Completion<zephium_core::notes::NoteReply>);
impl NoteCompletion {
    pub fn new(done: impl FnOnce(zephium_core::notes::NoteReply) + Send + 'static) -> Self {
        Self(Arc::new(Mutex::new(Some(Box::new(done)))))
    }
    pub fn finish(self, reply: zephium_core::notes::NoteReply) {
        let done = self
            .0
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take();
        if let Some(done) = done {
            done(reply);
        }
    }
}
impl fmt::Debug for NoteCompletion {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("NoteCompletion")
    }
}

#[derive(Clone)]
pub struct TabCompletion(Completion<Option<ItemId>>);
impl TabCompletion {
    pub fn new(done: impl FnOnce(Option<ItemId>) + Send + 'static) -> Self {
        Self(Arc::new(Mutex::new(Some(Box::new(done)))))
    }
    pub fn finish(self, tab: Option<ItemId>) {
        let done = self
            .0
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take();
        if let Some(done) = done {
            done(tab);
        }
    }
}
impl fmt::Debug for TabCompletion {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("TabCompletion")
    }
}

#[derive(Clone)]
pub struct HistoryCompletion(Completion<zephium_ipc::HistoryResponse>);
impl HistoryCompletion {
    pub fn new(done: impl FnOnce(zephium_ipc::HistoryResponse) + Send + 'static) -> Self {
        Self(Arc::new(Mutex::new(Some(Box::new(done)))))
    }
    pub fn finish(self, response: zephium_ipc::HistoryResponse) {
        let done = self
            .0
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take();
        if let Some(done) = done {
            done(response);
        }
    }
}
#[derive(Clone)]
pub struct BrowsingDataCompletion(Completion<zephium_ipc::BrowsingDataResponse>);
impl BrowsingDataCompletion {
    pub fn new(done: impl FnOnce(zephium_ipc::BrowsingDataResponse) + Send + 'static) -> Self {
        Self(Arc::new(Mutex::new(Some(Box::new(done)))))
    }
    pub fn finish(self, response: zephium_ipc::BrowsingDataResponse) {
        let done = self
            .0
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take();
        if let Some(done) = done {
            done(response);
        }
    }
}
#[derive(Clone)]
pub struct TimeCompletion(Completion<zephium_ipc::TimeResponse>);
impl TimeCompletion {
    pub fn new(done: impl FnOnce(zephium_ipc::TimeResponse) + Send + 'static) -> Self {
        Self(Arc::new(Mutex::new(Some(Box::new(done)))))
    }
    pub fn finish(self, response: zephium_ipc::TimeResponse) {
        let done = self
            .0
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take();
        if let Some(done) = done {
            done(response);
        }
    }
}
impl fmt::Debug for TimeCompletion {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("TimeCompletion")
    }
}
#[derive(Clone)]
pub struct ZoomCompletion(Completion<zephium_ipc::ZoomResponse>);
impl ZoomCompletion {
    pub fn new(done: impl FnOnce(zephium_ipc::ZoomResponse) + Send + 'static) -> Self {
        Self(Arc::new(Mutex::new(Some(Box::new(done)))))
    }
    pub fn finish(self, response: zephium_ipc::ZoomResponse) {
        let done = self
            .0
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take();
        if let Some(done) = done {
            done(response);
        }
    }
}
impl fmt::Debug for ZoomCompletion {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ZoomCompletion")
    }
}
#[derive(Clone)]
pub struct BookmarkCompletion(Completion<zephium_ipc::BookmarkResponse>);
impl BookmarkCompletion {
    pub fn new(done: impl FnOnce(zephium_ipc::BookmarkResponse) + Send + 'static) -> Self {
        Self(Arc::new(Mutex::new(Some(Box::new(done)))))
    }
    pub fn finish(self, response: zephium_ipc::BookmarkResponse) {
        let done = self
            .0
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take();
        if let Some(done) = done {
            done(response);
        }
    }
}
/// The serialized document of a page, kept for the tab that shows its source.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PageSource {
    pub url: String,
    pub text: String,
    /// The page was longer than the engine reads.
    pub truncated: bool,
}

#[derive(Clone)]
pub struct SourceCompletion(Completion<Option<Arc<PageSource>>>);
impl SourceCompletion {
    pub fn new(done: impl FnOnce(Option<Arc<PageSource>>) + Send + 'static) -> Self {
        Self(Arc::new(Mutex::new(Some(Box::new(done)))))
    }
    pub fn finish(self, document: Option<Arc<PageSource>>) {
        let done = self
            .0
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take();
        if let Some(done) = done {
            done(document);
        }
    }
}
impl fmt::Debug for SourceCompletion {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SourceCompletion")
    }
}
#[derive(Clone)]
pub struct HistoryListCompletion(Completion<zephium_core::ports::engine::HistoryEntries>);
impl HistoryListCompletion {
    pub fn new(
        done: impl FnOnce(zephium_core::ports::engine::HistoryEntries) + Send + 'static,
    ) -> Self {
        Self(Arc::new(Mutex::new(Some(Box::new(done)))))
    }
    pub fn finish(self, entries: zephium_core::ports::engine::HistoryEntries) {
        let done = self
            .0
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take();
        if let Some(done) = done {
            done(entries);
        }
    }
}
impl fmt::Debug for HistoryListCompletion {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("HistoryListCompletion")
    }
}
#[derive(Clone)]
pub struct ExportCompletion(Completion<Result<String, zephium_core::export::ExportRefusal>>);
impl ExportCompletion {
    pub fn new(
        done: impl FnOnce(Result<String, zephium_core::export::ExportRefusal>) + Send + 'static,
    ) -> Self {
        Self(Arc::new(Mutex::new(Some(Box::new(done)))))
    }
    pub fn finish(self, reply: Result<String, zephium_core::export::ExportRefusal>) {
        let done = self
            .0
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take();
        if let Some(done) = done {
            done(reply);
        }
    }
}
impl fmt::Debug for ExportCompletion {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ExportCompletion")
    }
}
#[derive(Clone)]
pub struct ImportCompletion(Completion<Option<u32>>);
impl ImportCompletion {
    pub fn new(done: impl FnOnce(Option<u32>) + Send + 'static) -> Self {
        Self(Arc::new(Mutex::new(Some(Box::new(done)))))
    }
    pub fn finish(self, added: Option<u32>) {
        let done = self
            .0
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take();
        if let Some(done) = done {
            done(added);
        }
    }
}
impl fmt::Debug for ImportCompletion {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ImportCompletion")
    }
}
impl fmt::Debug for BookmarkCompletion {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("BookmarkCompletion")
    }
}
impl fmt::Debug for BrowsingDataCompletion {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("BrowsingDataCompletion")
    }
}
impl fmt::Debug for HistoryCompletion {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("HistoryCompletion")
    }
}
