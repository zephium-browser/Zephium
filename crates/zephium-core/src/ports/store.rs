use crate::blocker::{
    BlockerConfig, BlockerConfigRevision, BlockerSitePreferences, ProfileBlockerConfig,
};
use crate::ids::ProfileId;
use crate::permissions::{
    PagePermissionCatalog, PagePermissionCatalogRevision, PagePermissionPatch,
    PagePermissionPatchResults,
};
use crate::session::SessionState;
use crate::userscripts::{
    Userscript, UserscriptCatalog, UserscriptCatalogMutation, UserscriptCatalogRevision,
};
use std::time::Instant;

#[derive(Clone, Debug, PartialEq)]
pub struct HistoryHit {
    pub url: String,
    pub title: String,
    pub last_visit: i64,
}

/// A visit from another browser's history.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImportedVisit {
    pub url: String,
    pub title: String,
    /// Unix seconds.
    pub visited_at: i64,
}

/// Visits one import may carry; the history budget prunes the oldest after.
pub const MAX_IMPORTED_VISITS: usize = 50_000;

/// A failed read is distinct from a successfully empty history page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HistoryPageError {
    Invalid,
    Unavailable,
    NotAdmitted,
    TimedOut,
    Failed,
}

/// One recorded visit. Unlike `HistoryHit` these are not deduplicated by
/// address: a history list shows every time a page was opened.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HistoryVisit {
    pub id: i64,
    pub url: String,
    pub title: String,
    pub visited_at: i64,
}

/// A stored assistant conversation without its body, for a history list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChatSummary {
    pub id: String,
    pub space: String,
    pub title: String,
    pub updated_ms: i64,
}

/// Conversations kept per profile; saving a new one past this drops the oldest.
/// Sites one profile may let open new tabs without a click.
pub const MAX_POPUP_SITES: usize = 512;
pub const MAX_CHATS: usize = 200;
/// A conversation id is 1 to this many bytes of `[A-Za-z0-9-]`.
pub const MAX_CHAT_ID_BYTES: usize = 40;
pub const MAX_CHAT_SPACE_BYTES: usize = 128;
pub const MAX_CHAT_TITLE_BYTES: usize = 200;
pub const MAX_CHAT_BODY_BYTES: usize = 512 * 1024;

/// Maximum number of exact origins that browser chrome may hydrate in one
/// favicon-cache read. The returned raster for each origin is independently
/// fixed at `icon::RGBA32_BYTES`, bounding a batch to two MiB before small
/// collection overhead.
pub const MAX_FAVICON_BATCH_ORIGINS: usize = 512;

/// Durable cross-restart state for one profile deletion.
///
/// A row is created atomically with removal from the authoritative session
/// registry. `native_erasure_verified` becomes true only after the engine has
/// proved its platform-owned website data absent. The store must retain the
/// authorization until its own profile database has also been removed. A
/// platform adapter may retain an internal post-unlink tombstone beyond that
/// point for restart-time filesystem verification; such completed rows are
/// deliberately not returned as pending work to the current shell.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PendingProfileDeletion {
    pub profile: ProfileId,
    pub native_erasure_verified: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProfileDeletionLoad {
    Loaded(Vec<PendingProfileDeletion>),
    Failed,
}

/// Truthful result of the synchronous deletion-authorization barrier.
///
/// Native erasure may start only after `Authorized` or `AlreadyAuthorized`.
/// `OutcomeUnknown` means the bounded caller wait expired after the command
/// entered the storage actor; callers must reconcile through
/// `pending_profile_deletions` and must not assume either success or failure.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProfileDeletionAuthorizeOutcome {
    Authorized,
    AlreadyAuthorized,
    NotRegistered,
    SessionConflict,
    InvalidSession,
    NotAdmitted,
    OutcomeUnknown,
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProfileDeletionFinalizeOutcome {
    /// Native proof is durable and the exact local artifacts are absent for
    /// this process. The store may still retain a hidden completed tombstone
    /// until a fresh process verifies Windows filesystem recovery.
    Completed,
    NotAuthorized,
    NotAdmitted,
    OutcomeUnknown,
    Failed,
}

/// `SessionLoad::RecoveryRequired` for a session saved by a newer Zephium: it
/// opens again once that version is back, so it is never set aside.
pub const NEWER_SESSION_REASON: &str = "authoritative session is newer than supported";

/// Result of reading the authoritative browser session.
///
/// `Failed` is deliberately distinct from `Absent`: callers may initialize a
/// new profile only when no snapshot exists. A corrupt snapshot or storage I/O
/// failure must not be interpreted as first run and overwritten.
#[derive(Clone, Debug, PartialEq)]
pub enum SessionLoad {
    Absent,
    Loaded {
        state: SessionState,
        /// Exact blocker configuration cohort for every profile registered in
        /// `state`. Missing or extra rows are storage corruption, never an
        /// invitation for the application to invent a default.
        blocker_configs: Vec<ProfileBlockerConfig>,
    },
    /// The authoritative session is exact and fully usable, but one or more
    /// registered per-profile ancillary databases could not be safely opened
    /// at their shipped schema. Their original files are preserved and every
    /// history/favicon operation for these profiles is disabled until an
    /// explicit repair, export, or deletion flow handles them.
    LoadedWithDegradedProfiles {
        state: SessionState,
        profiles: Vec<ProfileId>,
        blocker_configs: Vec<ProfileBlockerConfig>,
    },
    /// The authoritative bytes were preserved, but they do not describe an
    /// exact canonical session. The store is read-only until an explicit
    /// recovery flow exports, repairs, or discards the quarantined snapshot.
    RecoveryRequired {
        reason: String,
    },
    Failed,
}

/// Durable result of a compare-and-swap profile blocker preference update.
///
/// An admitted callback runs exactly once. `OutcomeUnknown` means the caller's
/// observation deadline elapsed after the command entered the storage actor;
/// it must reconcile through the next authoritative load instead of guessing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlockerConfigUpdateOutcome {
    Updated(ProfileBlockerConfig),
    Conflict(ProfileBlockerConfig),
    NotRegistered,
    NotAdmitted,
    OutcomeUnknown,
    Failed,
}

/// Result of one actor-owned, asynchronous read of an exact durable blocker
/// preference.
///
/// This narrow reconciliation path exists for indeterminate CAS outcomes.
/// It must not be implemented by calling the synchronous whole-session load
/// from the application actor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlockerConfigLoadOutcome {
    Loaded(ProfileBlockerConfig),
    NotRegistered,
    NotAdmitted,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BlockerSiteLoadOutcome {
    Loaded(std::sync::Arc<BlockerSitePreferences>),
    NotRegistered,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BlockerSiteUpdateOutcome {
    Updated(std::sync::Arc<BlockerSitePreferences>),
    Conflict(std::sync::Arc<BlockerSitePreferences>),
    NotRegistered,
    /// A commit was attempted and its durable outcome must be reconciled.
    OutcomeUnknown,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UserscriptCatalogLoadOutcome {
    Loaded(UserscriptCatalog),
    NotRegistered,
    /// The exact per-profile database was preserved but could not be safely
    /// opened at the shipped schema. No subset of its scripts is returned.
    DegradedProfile,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UserscriptCatalogMutationApplied {
    pub catalog_revision: UserscriptCatalogRevision,
    /// The exact durable row after install/update/toggle. Deletion returns
    /// `None`; callers retain the mutation's id for reconciliation.
    pub script: Option<Box<Userscript>>,
}

/// Durable result of one profile-catalog compare-and-swap mutation.
///
/// Commit errors are `OutcomeUnknown`, never `Failed`, because a caller must
/// reconcile the exact catalog before deciding whether another mutation is
/// legal. `Invalid` means the adapter proved no durable write or commit was
/// attempted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UserscriptCatalogMutationOutcome {
    Applied(UserscriptCatalogMutationApplied),
    Conflict { current: UserscriptCatalogRevision },
    NotRegistered,
    DegradedProfile,
    Invalid,
    RevisionExhausted,
    OutcomeUnknown,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PagePermissionCatalogLoadOutcome {
    Loaded(PagePermissionCatalog),
    NotRegistered,
    /// The exact per-profile database was preserved but could not be safely
    /// opened at the shipped schema. No subset of its grants is returned.
    DegradedProfile,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PagePermissionCatalogMutationApplied {
    pub catalog_revision: PagePermissionCatalogRevision,
    /// One bounded result for every change in the submitted patch, in patch
    /// order. Create/update carries the exact durable row; delete carries
    /// `None`.
    pub results: PagePermissionPatchResults,
}

/// Durable result of one atomic profile page-permission patch.
///
/// `OutcomeUnknown` means SQLite settlement could not be observed after the
/// transaction entered commit; callers must exact-load before attempting a
/// new mutation. `Invalid` and `LimitReached` prove no durable write or commit
/// was attempted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PagePermissionCatalogMutationOutcome {
    Applied(PagePermissionCatalogMutationApplied),
    Conflict {
        current: PagePermissionCatalogRevision,
    },
    NotRegistered,
    DegradedProfile,
    Invalid,
    LimitReached,
    RevisionExhausted,
    OutcomeUnknown,
    Failed,
}

/// Result of the store's terminal process-boundary protocol.
///
/// `RetryableFailure` proves the terminal command was not entered (normally
/// because the durability barrier failed), so the live actor may accept a
/// later retry. `Unclean` means terminal ownership may have transferred but
/// actor exit/resource release was not proved before the caller's deadline;
/// continued in-process use is unsafe and the process must exit non-zero.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StoreShutdownOutcome {
    RetryableFailure,
    Clean,
    Unclean,
}

pub type LegacyNotesDone = Box<dyn FnOnce(Option<Vec<crate::resources::ResourceRecord>>) + Send>;

/// Exact application-setting readback. Missing is distinct from an unavailable
/// database/actor so permission defaults never turn read failures into consent.
#[derive(Clone, Eq, PartialEq)]
pub enum AppSettingReadback {
    Value(String),
    Missing,
    Unavailable,
}

pub trait Store {
    /// Dormant product Work authoring lane. The application selects the profile;
    /// Store independently rejects unregistered/private/deleting profiles.
    fn work_document(
        &self,
        _profile: ProfileId,
        _request: crate::work::port::WorkRequest,
        _completion: crate::work::port::WorkCompletion,
    ) -> Result<(), crate::work::WorkError> {
        Err(crate::work::WorkError::Unavailable)
    }

    /// Admits bytes the desktop read on the user's behalf into the profile's
    /// media store and mints the Media resource that describes them.
    fn import_media(
        &self,
        _profile: ProfileId,
        _import: crate::resources::MediaImport,
        done: crate::resources::ResourceDone,
    ) {
        done(crate::resources::ResourceResponse::Error {
            error: crate::resources::ResourceError::Unavailable,
        });
    }

    /// Bounded asynchronous access to durable Notes/Tasks. Caller owns authorization.
    fn resource_call(
        &self,
        _profile: ProfileId,
        _call: crate::resources::ResourceCall,
        done: crate::resources::ResourceDone,
    ) {
        done(crate::resources::ResourceResponse::Error {
            error: crate::resources::ResourceError::Unavailable,
        });
    }
    /// Every saved profile avatar, read once at startup. Profiles without a
    /// row are drawn with [`crate::profiles::ProfileAvatar::initial`].
    fn load_profile_avatars(&self) -> Vec<(ProfileId, crate::profiles::ProfileAvatar)> {
        Vec::new()
    }
    /// Replaces a profile's avatar. A photo style carries its downscaled
    /// bytes; any other style clears the stored photo.
    fn save_profile_avatar(
        &self,
        _profile: ProfileId,
        _avatar: crate::profiles::ProfileAvatar,
        _photo: Option<std::sync::Arc<Vec<u8>>>,
        done: Box<dyn FnOnce(bool) + Send>,
    ) {
        done(false);
    }
    fn profile_avatar_photo(
        &self,
        _profile: ProfileId,
        done: Box<dyn FnOnce(Option<Vec<u8>>) + Send>,
    ) {
        done(None);
    }
    /// Notes kept in the profile database before notes became Markdown
    /// files, for their one-time move into the notes folder. `None` when they
    /// cannot be read now.
    fn legacy_notes(&self, _profile: ProfileId, done: LegacyNotesDone) {
        done(None);
    }
    /// Deletes notes that now live in the notes folder.
    fn retire_legacy_notes(
        &self,
        _profile: ProfileId,
        _ids: Vec<String>,
        done: Box<dyn FnOnce(bool) + Send>,
    ) {
        done(false);
    }
    fn save_session(&self, session: SessionState);
    /// Ordered session-durability barrier for shutdown and other process
    /// boundaries. Returns only after the latest session snapshot queued
    /// before this call has committed (`true`) or the adapter reports failure.
    fn flush(&self) -> bool;
    /// Deadline-aware form of the durability barrier. Adapters must not keep
    /// the caller blocked after `deadline`; they may continue an already
    /// admitted OS write on their private worker after returning `false`.
    fn flush_until(&self, _deadline: Instant) -> bool {
        self.flush()
    }
    /// Flushes every mutation ordered before this call and, for actor-backed
    /// stores, terminates and joins the actor while releasing its database
    /// handles. The implementation must use the caller's existing deadline;
    /// it must not start a fresh timeout after durability completes.
    fn shutdown_until(&self, deadline: Instant) -> StoreShutdownOutcome {
        if self.flush_until(deadline) {
            StoreShutdownOutcome::Clean
        } else {
            StoreShutdownOutcome::RetryableFailure
        }
    }
    fn load_session(&self) -> SessionLoad;
    /// After `RecoveryRequired`: keeps the unrestorable session's bytes in a
    /// file and restarts the session from the profile registry with no tabs.
    /// Returns the restarted load and the file, or `None` when it could not.
    fn set_aside_session(&self) -> Option<(SessionLoad, Option<std::path::PathBuf>)> {
        None
    }
    /// Durably replaces a profile's blocker preference only when `expected`
    /// is still authoritative. The storage adapter allocates the next checked
    /// revision and invokes `done` after transaction settlement.
    ///
    /// `false` proves the bounded adapter did not admit the command and the
    /// callback will not run. This rare control-plane mutation is never
    /// coalesced with browsing-history or session-snapshot traffic.
    fn update_profile_blocker_config(
        &self,
        _profile: ProfileId,
        _expected: BlockerConfigRevision,
        _next: BlockerConfig,
        _done: Box<dyn FnOnce(BlockerConfigUpdateOutcome) + Send>,
    ) -> bool {
        false
    }
    /// Reads one profile's current authoritative blocker preference without
    /// blocking the application actor. `true` transfers exactly-once callback
    /// ownership; `false` proves the request was not admitted.
    fn load_profile_blocker_config(
        &self,
        _profile: ProfileId,
        _done: Box<dyn FnOnce(BlockerConfigLoadOutcome) + Send>,
    ) -> bool {
        false
    }
    /// Private-session preferences must never be passed to these durable ports.
    fn load_profile_blocker_sites(
        &self,
        _profile: ProfileId,
        _done: Box<dyn FnOnce(BlockerSiteLoadOutcome) + Send>,
    ) -> bool {
        false
    }

    fn update_profile_blocker_sites(
        &self,
        _profile: ProfileId,
        _expected_revision: u64,
        _next: std::sync::Arc<BlockerSitePreferences>,
        _done: Box<dyn FnOnce(BlockerSiteUpdateOutcome) + Send>,
    ) -> bool {
        false
    }
    /// Loads one complete bounded profile catalog. `true` transfers
    /// exactly-once callback ownership; `false` proves the request was not
    /// admitted. Implementations must never return a filtered valid subset of
    /// a malformed catalog.
    fn load_userscript_catalog(
        &self,
        _profile: ProfileId,
        _done: Box<dyn FnOnce(UserscriptCatalogLoadOutcome) + Send>,
    ) -> bool {
        false
    }
    /// Applies one exact durable catalog mutation after comparing the
    /// collection revision. Source-carrying requests are independently byte-
    /// and count-bounded before entering an actor mailbox.
    fn mutate_userscript_catalog(
        &self,
        _profile: ProfileId,
        _expected: UserscriptCatalogRevision,
        _mutation: UserscriptCatalogMutation,
        _done: Box<dyn FnOnce(UserscriptCatalogMutationOutcome) + Send>,
    ) -> bool {
        false
    }
    /// Loads one complete bounded page-permission authority catalog. `true`
    /// transfers exactly-once callback ownership; `false` proves the request
    /// was not admitted. Malformed durable state fails as a whole.
    fn load_page_permission_catalog(
        &self,
        _profile: ProfileId,
        _done: Box<dyn FnOnce(PagePermissionCatalogLoadOutcome) + Send>,
    ) -> bool {
        false
    }
    /// Atomically applies at most four page-permission changes after comparing
    /// the collection revision. The patch is already structurally bounded by
    /// its core constructor before it can enter an adapter mailbox.
    fn mutate_page_permission_catalog(
        &self,
        _profile: ProfileId,
        _expected: PagePermissionCatalogRevision,
        _patch: PagePermissionPatch,
        _done: Box<dyn FnOnce(PagePermissionCatalogMutationOutcome) + Send>,
    ) -> bool {
        false
    }
    /// Enumerates only registered durable profiles for startup download recovery.
    /// False means the callback was not retained.
    fn download_recovery_profiles(
        &self,
        _done: Box<dyn FnOnce(crate::downloads::DownloadStoreReply) + Send>,
    ) -> bool {
        false
    }
    /// Bounded asynchronous download persistence on the existing Store actor.
    /// Private profiles must never call this port. False means no callback is retained.
    fn download_call(
        &self,
        _profile: ProfileId,
        _call: crate::downloads::DownloadStoreCall,
        _done: Box<dyn FnOnce(crate::downloads::DownloadStoreReply) + Send>,
    ) -> bool {
        false
    }
    /// History is per-profile; the adapter must ignore profiles it does not
    /// persist (incognito never reaches disk).
    fn record_visit(&self, profile: ProfileId, url: String, title: String);
    /// App-level settings (keymap, launcher prefs) live outside profiles.
    fn app_setting(&self, key: &str) -> Option<String>;
    /// Reads an authoritative value, including a confirmed absent setting.
    /// Adapters without this proof fail closed for permission consumers.
    fn app_setting_readback(&self, _key: &str) -> AppSettingReadback {
        AppSettingReadback::Unavailable
    }

    /// Enqueues an ordered application-setting mutation. `true` means the
    /// bounded adapter accepted the command; durability is established by a
    /// later `flush`/`flush_until` barrier. `false` is a definite rejection.
    fn set_app_setting(&self, key: String, value: String) -> bool;

    /// Records an explicit submitted search, never a partial keystroke.
    fn record_search(&self, _profile: ProfileId, _query: String, _url: String) -> bool {
        false
    }
    /// Prefix search over the profile's history FTS index, deduped by url,
    /// most recent first.
    fn search_history(&self, profile: ProfileId, query: &str, limit: u32) -> Vec<HistoryHit>;
    /// One page of visits, newest first, optionally narrowed by a query and by
    /// `since`. `before` is the id of the last visit already seen.
    fn history_page(
        &self,
        profile: ProfileId,
        query: &str,
        since: Option<i64>,
        before: Option<i64>,
        limit: u32,
    ) -> Vec<HistoryVisit>;
    /// Completeness-sensitive readers must use this method. Adapters without
    /// a typed read fail closed rather than treating failures as EOF.
    fn history_page_checked(
        &self,
        _profile: ProfileId,
        _query: &str,
        _since: Option<i64>,
        _before: Option<i64>,
        _limit: u32,
    ) -> Result<Vec<HistoryVisit>, HistoryPageError> {
        Err(HistoryPageError::Unavailable)
    }
    /// Removes every visit to each address; returns how many rows went.
    fn forget_history_urls(&self, profile: ProfileId, urls: &[String]) -> u32;
    /// `None` means the deletion or recovery-copy disposal failed. Zero is
    /// a successful deletion with no matching visits.
    fn forget_history_urls_checked(&self, profile: ProfileId, urls: &[String]) -> Option<u32> {
        Some(self.forget_history_urls(profile, urls))
    }
    /// Visits from another browser, kept with their own times. A visit
    /// already present at the same time is skipped, so importing again adds
    /// only what is new. Returns how many were added, or None when the
    /// profile cannot take them.
    fn import_history(&self, _profile: ProfileId, _visits: Vec<ImportedVisit>) -> Option<u32> {
        None
    }
    /// Site icons another browser held, as fixed 32x32 rasters by origin. An
    /// origin that already has one keeps it. Returns how many were added.
    fn import_favicons(&self, _profile: ProfileId, _icons: Vec<(String, Vec<u8>)>) -> Option<u32> {
        None
    }
    /// One bookmark read or write. Blocking; callers run it off the shell.
    fn bookmarks(
        &self,
        _profile: ProfileId,
        _request: crate::bookmarks::BookmarkRequest,
    ) -> crate::bookmarks::BookmarkReply {
        crate::bookmarks::BookmarkReply::Failed(crate::bookmarks::BookmarkFailure::Unavailable)
    }
    /// [`Self::bookmarks`], also describing what a write changed for the
    /// extensions listening. Blocking; callers run it off the shell.
    fn bookmarks_observed(
        &self,
        profile: ProfileId,
        request: crate::bookmarks::BookmarkRequest,
    ) -> (
        crate::bookmarks::BookmarkReply,
        Option<crate::bookmarks::BookmarkChange>,
    ) {
        (self.bookmarks(profile, request), None)
    }
    /// History as extensions read and change it. Blocking; callers run it
    /// off the shell.
    fn extension_history(
        &self,
        _profile: ProfileId,
        _request: crate::extension_data::HistoryRequest,
    ) -> crate::extension_data::HistoryReply {
        crate::extension_data::HistoryReply::Failed
    }
    /// Removes visits at or after `since`, or all of them when it is absent.
    fn load_blocker_statistics(
        &self,
        _profile: ProfileId,
        done: Box<dyn FnOnce(Option<crate::blocker::BlockerStatistics>) + Send>,
    ) -> bool {
        done(Some(crate::blocker::BlockerStatistics::default()));
        true
    }
    fn save_blocker_statistics(
        &self,
        _profile: ProfileId,
        _statistics: crate::blocker::BlockerStatistics,
        done: Box<dyn FnOnce(bool) + Send>,
    ) -> bool {
        done(false);
        true
    }
    /// Adds hour tallies to a profile's time and drops hours before
    /// `keep_from_hour`. False when not admitted; the caller keeps them.
    fn record_time(
        &self,
        _profile: ProfileId,
        _tallies: Vec<crate::time::HourTally>,
        _keep_from_hour: i64,
    ) -> bool {
        false
    }
    /// Ordered after every admitted `record_time`, so it sees them.
    fn time_report(
        &self,
        _profile: ProfileId,
        _query: crate::time::TimeQuery,
        _done: Box<dyn FnOnce(Option<crate::time::TimeReport>) + Send>,
    ) -> bool {
        false
    }
    /// Removes a profile's time from `since_hour` on, or all of it.
    fn clear_time(&self, _profile: ProfileId, _since_hour: Option<i64>) -> bool {
        false
    }
    fn record_focus(&self, _record: crate::time::FocusRecord, _day: i64) -> bool {
        false
    }
    fn focus_days(
        &self,
        _from_day: i64,
        _days: u32,
        _done: Box<dyn FnOnce(Option<Vec<crate::time::FocusDay>>) + Send>,
    ) -> bool {
        false
    }
    fn clear_history(&self, profile: ProfileId, since: Option<i64>) -> u32;
    /// Clears history only after earlier activity writes settle. `None`
    /// means the durability barrier or deletion failed; zero rows is a
    /// successful, distinct result. Legacy stores retain their old contract.
    fn clear_history_checked(&self, profile: ProfileId, since: Option<i64>) -> Option<u32> {
        Some(self.clear_history(profile, since))
    }
    /// Disposes recovery copies that may contain this profile's browsing
    /// data. False means queue refusal, with no completion; admitted calls
    /// report whether disposal committed. Call for site-data erasure too.
    fn discard_session_backup(
        &self,
        _profile: ProfileId,
        _done: Box<dyn FnOnce(bool) + Send>,
    ) -> bool {
        false
    }
    /// Replaces the placeholder title on the newest recent visit to an address.
    fn amend_visit_title(&self, profile: ProfileId, url: String, title: String) -> bool;
    /// Age in seconds of the cached icon for a favicon key, None when absent.
    /// A key is a page origin, or one page of it (`icon::page_key`) whose own
    /// icon differs from its origin's.
    fn favicon_age(&self, profile: ProfileId, origin: &str) -> Option<i64>;
    fn save_favicon(
        &self,
        profile: ProfileId,
        origin: String,
        content_type: Option<String>,
        bytes: Vec<u8>,
    );
    fn favicon_bytes(&self, profile: ProfileId, origin: &str) -> Option<(Option<String>, Vec<u8>)>;
    /// Drops the icon kept for one page, as when it now matches its origin's.
    fn forget_page_favicon(&self, _profile: ProfileId, _page: String) {}

    /// Loads one already-decoded favicon with the age of the stored copy.
    /// Age decides whether to refresh, never whether to display: an old
    /// raster is still the right thing to draw while a newer one is fetched.
    fn favicon_raster_with_age(&self, profile: ProfileId, origin: &str) -> Option<(Vec<u8>, i64)> {
        let age = self.favicon_age(profile, origin)?;
        self.favicon_bytes(profile, origin)
            .map(|(_, bytes)| (bytes, age))
    }

    /// Loads already-decoded favicon rasters for a bounded authoritative set
    /// of origins. Actor-backed stores should override this to perform one
    /// mailbox round trip; the default is suitable for simple test adapters.
    fn favicon_rasters(&self, profile: ProfileId, origins: &[String]) -> Vec<(String, Vec<u8>)> {
        if origins.len() > MAX_FAVICON_BATCH_ORIGINS {
            return Vec::new();
        }
        let mut seen = std::collections::HashSet::with_capacity(origins.len());
        origins
            .iter()
            .filter(|origin| seen.insert((*origin).clone()))
            .filter_map(|origin| {
                self.favicon_bytes(profile, origin)
                    .map(|(_, bytes)| (origin.clone(), bytes))
            })
            .collect()
    }

    /// The page zoom a profile saved per site, newest first.
    fn zoom_levels(&self, _profile: ProfileId) -> Vec<crate::zoom::SavedLevel> {
        Vec::new()
    }
    /// Saves a site's page zoom, or forgets it when `percent` is none. The
    /// oldest sites beyond `zoom::MAX_SITE_LEVELS` are dropped. False when
    /// not admitted. Clearing history clears the levels it covers.
    fn save_zoom_level(
        &self,
        _profile: ProfileId,
        _site: String,
        _percent: Option<u16>,
        _set_at: i64,
    ) -> bool {
        false
    }

    /// Assistant conversations, newest first, at most `limit` (never more than
    /// `MAX_CHATS`).
    fn chats(&self, _profile: ProfileId, _limit: u32) -> Vec<ChatSummary> {
        Vec::new()
    }
    /// The body stored with a conversation, exactly as it was saved.
    fn chat(&self, _profile: ProfileId, _id: &str) -> Option<String> {
        None
    }
    /// Inserts or replaces a conversation, dropping the oldest past
    /// `MAX_CHATS`. False when a bound is exceeded, the profile is not
    /// persisted, or the conversation is older than all the ones kept. `body`
    /// is opaque to the store.
    fn save_chat(
        &self,
        _profile: ProfileId,
        _id: &str,
        _space: &str,
        _title: &str,
        _updated_ms: i64,
        _body: String,
    ) -> bool {
        false
    }
    /// True when a conversation was removed.
    fn delete_chat(&self, _profile: ProfileId, _id: &str) -> bool {
        false
    }
    /// Deletes conversations last updated at or after `since` (Unix seconds,
    /// as for `clear_history`, which leaves conversations alone), or all of
    /// them when it is absent. Returns how many went.
    fn clear_chats(&self, _profile: ProfileId, _since: Option<i64>) -> u32 {
        0
    }
    /// The sites `profile` lets open new tabs without a click, or None when
    /// they cannot be read.
    fn popup_sites(&self, _profile: ProfileId) -> Option<Vec<crate::permissions::PageOrigin>> {
        None
    }
    /// Adds or removes one site from that list. False when nothing was
    /// written, including when the list is full.
    fn set_popup_site(
        &self,
        _profile: ProfileId,
        _origin: crate::permissions::PageOrigin,
        _allowed: bool,
    ) -> bool {
        false
    }

    /// Returns the bounded, durable deletion work that survived the previous
    /// process. An empty default keeps non-persistent test adapters inert.
    fn pending_profile_deletions(&self) -> ProfileDeletionLoad {
        ProfileDeletionLoad::Loaded(Vec::new())
    }

    /// Atomically commits the exact canonical post-removal session and a
    /// durable deletion authorization. This is the ordering barrier between
    /// aggregate removal and native website-data erasure.
    ///
    /// The store validates persistence invariants only; policy such as whether
    /// a default/last profile may be deleted belongs to the application.
    fn authorize_profile_deletion(
        &self,
        _profile: ProfileId,
        _filtered_session: SessionState,
        _deadline: Instant,
    ) -> ProfileDeletionAuthorizeOutcome {
        ProfileDeletionAuthorizeOutcome::NotRegistered
    }

    /// Records the engine's authoritative native-erasure proof and removes
    /// the exact journal-authorized SQLite profile. Implementations must write
    /// the proof before touching the file and clear the journal only after
    /// deletion succeeds, so every crash point remains idempotently resumable.
    fn finalize_profile_deletion(
        &self,
        _profile: ProfileId,
        _deadline: Instant,
    ) -> ProfileDeletionFinalizeOutcome {
        ProfileDeletionFinalizeOutcome::NotAuthorized
    }
}
