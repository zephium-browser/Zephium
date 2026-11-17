//! What the assistant's service needs from the shell, the engine, the store and the
//! model layer. Production adapters live beside the desktop shell; tests use
//! in-memory fakes.
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use zephium_app::work_models::WorkCallSource;
use zephium_core::ids::ProfileId;
use zephium_core::work::model::{
    WorkModelClient, WorkModelEntry, WorkModelError, WorkModelReasoning,
};
use zephium_ipc::page_assist::{PageAssistBringV1, PageAssistChangedV1, WorkConfirmCategoryV1};
use zephium_page_assist::actions::{Direction, Verb};
pub use zephium_page_assist::outline::{Outline, OutlineKind, OutlineNode, Rgb};

/// A boxed, sendable future.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// What the shell knows about a tab without reading it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TabFacts {
    /// The profile the tab belongs to.
    pub profile: ProfileId,
    /// The Space the tab is listed in.
    pub space: Option<String>,
    /// The committed URL; none for internal pages.
    pub url: Option<String>,
    /// The page's title (untrusted).
    pub title: Option<String>,
    /// The tab is in a private window.
    pub private: bool,
    /// The tab shows a browser viewer (PDF, image, source).
    pub viewer: bool,
    /// The first load has not finished.
    pub loading: bool,
    /// Changes on every committed navigation; reads and steps bind to it.
    pub epoch: u64,
}

/// A form field's kind, as the reader judged it from the live control.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum FieldKind {
    /// Ordinary text.
    Text,
    /// A search box.
    Search,
    /// An e-mail address.
    Email,
    /// A number.
    Number,
    /// A password: never read, never typed into.
    Password,
    /// A payment card field: never read, never typed into.
    Card,
    /// A one-time code: never read, never typed into.
    OneTimeCode,
}

impl FieldKind {
    /// Whether its value is a secret the panel never reads or writes.
    pub fn protected(self) -> bool {
        matches!(self, Self::Password | Self::Card | Self::OneTimeCode)
    }
}

/// What a passage is.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Role {
    /// A heading of the given level.
    Heading(u8),
    /// Running text.
    Text,
    /// A list item; `ordered` for numbered lists.
    ListItem {
        /// The list is numbered.
        ordered: bool,
    },
    /// A table cell.
    Cell,
    /// A link; `same_site` when it stays on the tab's site.
    Link {
        /// The destination is on the same registrable site.
        same_site: bool,
    },
    /// A button.
    Button,
    /// A text field.
    Field(FieldKind),
    /// A checkbox or switch.
    Checkbox {
        /// Its current state.
        checked: bool,
    },
    /// A drop-down or list box.
    Select,
    /// Anything else readable.
    Other,
}

/// One passage of a live read.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Passage {
    /// Its number within this read, as the model cites it (`rN`).
    pub id: u16,
    /// What it is.
    pub role: Role,
    /// The visible text or accessible name.
    pub text: String,
    /// `text` is exactly what the page shows, complete.
    pub verbatim: bool,
    /// A field's current value. Adapters never fill it for protected kinds.
    pub value: Option<String>,
    /// 0 the main content, 1 beside it, 2 the page's chrome.
    pub region: u8,
    /// Where a link leads: absolute `http(s)`, from the reader.
    pub href: Option<String>,
}

/// One fresh read of a tab.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Readout {
    /// The navigation the read belongs to.
    pub epoch: u64,
    /// This read's identity; steps planned on it carry it, so no other read
    /// can stand in for it.
    pub read: u64,
    /// The page's title (untrusted).
    pub title: Option<String>,
    /// Passages in reading order.
    pub passages: Vec<Passage>,
}

/// Why a read did not happen.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReadFailure {
    /// The page is not ready yet.
    NotReady,
    /// The tab navigated or closed.
    PageChanged,
    /// The page could not be read.
    Unavailable,
}

/// One step, as the service asks the engine to run it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StepRequest {
    /// What it does.
    pub verb: Verb,
    /// The read it was planned on; a step whose read is gone is refused as
    /// a changed page.
    pub read: u64,
    /// The passage of that read it acts on.
    pub target: Option<u16>,
    /// A page scroll with no passage.
    pub direction: Option<Direction>,
    /// What to type or choose.
    pub value: Option<String>,
}

/// What a step would do, judged from the live control and its page.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StepEffect {
    /// Reads or moves around.
    Read,
    /// Changes the page without committing anything.
    Draft,
    /// Sends, buys, deletes or saves; attended confirmation is decided separately.
    Commit(WorkConfirmCategoryV1),
}

/// A step the engine can run.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolvedStep {
    /// Names the exact control, such as `Click "Place order"`.
    pub label: String,
    /// What it would do.
    pub effect: StepEffect,
    /// The observed control and its region require an attended confirmation.
    pub confirm: bool,
}

/// Why a step did not run or did not take.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StepFailure {
    /// The tab navigated or closed.
    PageChanged,
    /// The passage is not a control this verb can use.
    Unsuitable,
    /// A protected field, another site, or a sensitive page.
    Refused,
    /// It ran without the expected effect.
    Failed,
}

/// The live tab: facts, reads, highlights, steps and page styles.
pub trait PagePort: Send + Sync {
    /// Whether this platform can read live tabs.
    fn supported(&self) -> bool;
    /// What the shell knows about `tab`.
    fn facts(&self, tab: &str) -> Option<TabFacts>;
    /// A fresh read of `tab`, bound to `epoch`.
    fn read<'a>(&'a self, tab: &'a str, epoch: u64) -> BoxFuture<'a, Result<Readout, ReadFailure>>;
    /// The page's structure and palette for a style turn, bound to `epoch`.
    fn outline<'a>(
        &'a self,
        tab: &'a str,
        epoch: u64,
    ) -> BoxFuture<'a, Result<Outline, ReadFailure>>;
    /// The selected text, when anything is selected.
    fn selection<'a>(&'a self, tab: &'a str, epoch: u64) -> BoxFuture<'a, Option<String>>;
    /// Scrolls to and marks `text` without the find bar.
    fn highlight(&self, tab: &str, epoch: u64, text: &str) -> bool;
    /// Judges a step against the latest read of `epoch`.
    fn resolve(
        &self,
        tab: &str,
        epoch: u64,
        step: &StepRequest,
    ) -> Result<ResolvedStep, StepFailure>;
    /// Runs a step under the tab's lease.
    fn act<'a>(
        &'a self,
        tab: &'a str,
        epoch: u64,
        step: &'a StepRequest,
    ) -> BoxFuture<'a, Result<(), StepFailure>>;
    /// Replaces the page-style slot of `site` in `profile`; empty removes it.
    fn apply_styles(&self, profile: &ProfileId, site: &str, css: &str);
    /// Replaces the tab's marks, resolved against read `read`; yields the
    /// numbers placed.
    fn marks<'a>(
        &'a self,
        tab: &'a str,
        epoch: u64,
        read: u64,
        marks: Vec<(u8, u16)>,
    ) -> BoxFuture<'a, Vec<u8>>;
    /// Emphasises one mark, or none; `scroll` brings it into view.
    fn mark_focus(&self, tab: &str, epoch: u64, n: Option<u8>, scroll: bool);
    /// Removes the tab's marks.
    fn marks_clear(&self, tab: &str);
    /// The person picks an element on the tab.
    fn pick<'a>(&'a self, tab: &'a str, epoch: u64) -> BoxFuture<'a, Result<Pick, PickFailure>>;
    /// A picked element as it is now; none when it is gone.
    fn pick_read<'a>(&'a self, tab: &'a str, epoch: u64, pick: u64) -> BoxFuture<'a, Option<Pick>>;
    /// Removes a picked element's outline.
    fn pick_clear(&self, tab: &str, pick: u64);
    /// Ends a pick in progress.
    fn pick_cancel(&self, tab: &str);
    /// Shows on the tab that the assistant is working there.
    fn glow(&self, tab: &str, glow: Glow);
}

/// The light along a tab's edges while the assistant works on it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Glow {
    /// Nothing.
    Off,
    /// Steps are running.
    Working,
    /// The person took over; the steps wait.
    Paused,
}

/// Closed facts about replies for the app's log: counts, names and closed
/// reasons only, never page or model text.
pub trait Diagnostics: Send + Sync {
    /// One fact.
    fn note(&self, fact: std::fmt::Arguments<'_>);
}

/// An element the person picked.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Pick {
    /// Its id on the tab.
    pub pick: u64,
    /// A readable name, such as "Story list".
    pub label: String,
    /// Its visible text (untrusted).
    pub text: String,
    /// A selector that styles it, when one is stable.
    pub selector: Option<String>,
}

/// Why picking ended without an element.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PickFailure {
    /// The person cancelled.
    Cancelled,
    /// The tab navigated or closed.
    PageChanged,
    /// Picking could not start.
    Unavailable,
}

/// One tab of a Space, as the model may see it listed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TabSummary {
    /// Its item id.
    pub id: String,
    /// Its title (untrusted).
    pub title: String,
    /// Its committed URL.
    pub url: Option<String>,
}

/// The Space's tabs.
pub trait Tabs: Send + Sync {
    /// The profile a Space belongs to.
    fn profile(&self, space: &str) -> Option<ProfileId>;
    /// The Space's tabs in order.
    fn list(&self, profile: &ProfileId, space: &str) -> Vec<TabSummary>;
    /// Opens `url` in a new tab of `space` after `after`, in front when
    /// `focus`; yields the new tab's id.
    fn open<'a>(
        &'a self,
        profile: &'a ProfileId,
        space: &'a str,
        url: &'a str,
        after: Option<&'a str>,
        focus: bool,
    ) -> BoxFuture<'a, Option<String>>;
    /// Loads `url` in `tab`, as the address field would; false when refused.
    fn navigate(&self, profile: &ProfileId, tab: &str, url: &str) -> bool;
    /// The person's own search engine's results address for `query`.
    fn search_url(&self, profile: &ProfileId, query: &str) -> Option<String>;
}

/// The model a reply would use.
#[derive(Clone)]
pub struct ModelChoice {
    /// The catalog entry: name, provider, price.
    pub entry: WorkModelEntry,
    /// The person's effort, as the model takes it.
    pub effort: Option<WorkModelReasoning>,
}

/// A model and its client for one reply.
#[derive(Clone)]
pub struct ResolvedModel {
    /// The catalog entry: name, provider, price.
    pub entry: WorkModelEntry,
    /// The person's effort, as the model takes it.
    pub effort: Option<WorkModelReasoning>,
    /// The client; dropping a call cancels it.
    pub client: Arc<dyn WorkModelClient>,
}

/// The person's Page model.
pub trait ModelSource: Send + Sync {
    /// The model a reply in `profile` would use, without a client.
    fn entry<'a>(&'a self, profile: &'a ProfileId) -> BoxFuture<'a, Option<ModelChoice>>;
    /// The model and a client for one reply's calls.
    fn resolve<'a>(
        &'a self,
        profile: &'a ProfileId,
        source: WorkCallSource,
    ) -> BoxFuture<'a, Result<ResolvedModel, WorkModelError>>;
}

/// Where the panel's events go.
pub trait EventSink: Send + Sync {
    /// Delivers one event.
    fn emit(&self, event: PageAssistChangedV1);
}

/// Time.
pub trait Clock: Send + Sync {
    /// Milliseconds on a monotonic clock.
    fn now_ms(&self) -> u64;
    /// Milliseconds since the Unix epoch.
    fn unix_ms(&self) -> u64;
}

/// The shell's switches.
pub trait Settings: Send + Sync {
    /// AI is switched on for `profile`.
    fn ai_enabled(&self, profile: &ProfileId) -> bool;
    /// The person asked for less motion.
    fn reduce_motion(&self) -> bool {
        false
    }
}

/// What a handoff asks to bring along, in the model's words.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct BringRequest {
    /// Tabs of the Space.
    pub tabs: Vec<String>,
    /// Words naming a task to find.
    pub task: Option<String>,
    /// Words naming a note to find.
    pub note: Option<String>,
    /// A GitHub repository, `owner/name`.
    pub repository: Option<String>,
    /// A local folder, such as `~/Dev/zephium-infra`.
    pub folder: Option<String>,
}

/// A Work run to begin.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkStart {
    /// Its title.
    pub title: String,
    /// The objective: the summary, the person's words and the conversation.
    pub objective: String,
    /// What it brings along, as resolved.
    pub brings: Vec<PageAssistBringV1>,
}

/// Starts Work.
pub trait WorkHandoff: Send + Sync {
    /// Finds what a handoff would bring along; only what exists.
    fn prepare<'a>(
        &'a self,
        profile: &'a ProfileId,
        space: &'a str,
        request: BringRequest,
    ) -> BoxFuture<'a, Vec<PageAssistBringV1>>;
    /// Begins Work in `space`; yields the new Work's id.
    fn begin<'a>(
        &'a self,
        profile: &'a ProfileId,
        space: &'a str,
        start: WorkStart,
    ) -> BoxFuture<'a, Option<String>>;
}

/// One stored conversation, as the history list shows it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StoredConversation {
    /// Its id.
    pub id: String,
    /// The Space it was held in.
    pub space: String,
    /// Its title.
    pub title: String,
    /// Milliseconds since the Unix epoch.
    pub updated_ms: u64,
}

/// Where conversations persist per profile.
pub trait ConversationStore: Send + Sync {
    /// Newest first.
    fn list(&self, profile: &ProfileId, limit: u32) -> Vec<StoredConversation>;
    /// A stored body.
    fn load(&self, profile: &ProfileId, id: &str) -> Option<String>;
    /// Stores a body; false when it was not stored.
    fn save(&self, profile: &ProfileId, entry: &StoredConversation, body: String) -> bool;
    /// Deletes one.
    fn delete(&self, profile: &ProfileId, id: &str) -> bool;
}

/// One stored page style.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StoredStyle {
    /// Stable id.
    pub id: String,
    /// Registrable site.
    pub site: String,
    /// What it does, in a few words.
    pub summary: String,
    /// Canonical validated CSS.
    pub css: String,
    /// Applied to the site's pages.
    pub enabled: bool,
}

/// Where page styles persist per profile.
pub trait StyleStore: Send + Sync {
    /// Every stored style of `profile`.
    fn load(&self, profile: &ProfileId) -> Vec<StoredStyle>;
    /// Replaces every stored style of `profile`; false when it could not be
    /// stored, and then nothing changed.
    fn save(&self, profile: &ProfileId, styles: &[StoredStyle]) -> bool;
}
