//! User knowledge and unfinished work. These records are resources, not system
//! memory, execution attempts, browser leases or agent capabilities.
use serde::{Deserialize, Serialize};

pub const MAX_DOCUMENT_BYTES: usize = 256 * 1024;
pub const MAX_DOCUMENT_NODES: usize = 4096;
pub const MAX_RESOURCES: usize = 10_000;
const MAX_TASK_CONTEXT_URL_BYTES: usize = 8 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ipc-types", derive(specta::Type))]
#[serde(deny_unknown_fields)]
pub struct NoteDocument {
    pub version: u8,
    pub document: DocumentNode,
}

/// Constrained ProseMirror JSON. Allowed node/mark/attribute combinations are
/// checked before persistence, independently from editor-side validation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ipc-types", derive(specta::Type))]
#[serde(deny_unknown_fields)]
pub struct DocumentNode {
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub content: Vec<DocumentNode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attrs: Option<DocumentAttrs>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub marks: Vec<DocumentMark>,
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ipc-types", derive(specta::Type))]
#[serde(deny_unknown_fields)]
pub struct DocumentAttrs {
    #[serde(default)]
    pub start: Option<i32>,
    #[serde(default)]
    pub level: Option<u8>,
    #[serde(default)]
    pub resource: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ipc-types", derive(specta::Type))]
#[serde(deny_unknown_fields)]
pub struct DocumentMark {
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attrs: Option<DocumentMarkAttrs>,
}
/// A link is descriptive: chrome opens it through a native intent, never as
/// an anchor navigation inside privileged chrome.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ipc-types", derive(specta::Type))]
#[serde(deny_unknown_fields)]
pub struct DocumentMarkAttrs {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub href: Option<String>,
}
/// Who holds the next move on a task. People and agents share one list; this
/// records which of them is expected to act, not which of them is permitted to.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ipc-types", derive(specta::Type))]
#[serde(rename_all = "snake_case")]
pub enum TaskActor {
    #[default]
    User,
    Agent,
}

/// A task's own lifecycle. `Blocked` is where a delegated task lands when it
/// cannot proceed without a person, so a stalled delegation stays visible
/// instead of sitting in `Active` forever.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ipc-types", derive(specta::Type))]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    #[default]
    Open,
    Active,
    Blocked,
    Done,
}

/// The page a task came from, so it can reopen its own context. The URL passes
/// the same commit gate as any navigation; a stored task can never become a
/// route to a scheme the browser would refuse to open.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ipc-types", derive(specta::Type))]
#[serde(deny_unknown_fields)]
pub struct TaskContext {
    pub url: String,
    pub title: String,
}

impl TaskContext {
    pub fn validate(&self) -> bool {
        self.url.len() <= MAX_TASK_CONTEXT_URL_BYTES
            && url::Url::parse(&self.url).is_ok_and(|target| {
                target.as_str().len() <= MAX_TASK_CONTEXT_URL_BYTES
                    && crate::navigation::is_allowed(&target)
            })
            && self.title.len() <= 1024
            && !self.title.contains('\0')
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ipc-types", derive(specta::Type))]
#[serde(rename_all = "snake_case")]
pub enum TaskPriority {
    #[default]
    None,
    Low,
    Medium,
    High,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ipc-types", derive(specta::Type))]
#[serde(deny_unknown_fields)]
pub struct TaskStep {
    pub id: String,
    pub title: String,
    pub completed: bool,
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ipc-types", derive(specta::Type))]
#[serde(default, deny_unknown_fields)]
pub struct TaskDetails {
    pub list: Option<String>,
    pub inbox: bool,
    pub priority: TaskPriority,
    pub steps: Vec<TaskStep>,
    /// Native completion time; callers cannot forge or preserve it across reopening.
    pub completed_at: Option<String>,
    /// The day the outcome is owed, independent of the day it is planned for:
    /// `due_date` says when to work on it, `deadline` when it must be finished.
    pub deadline: Option<String>,
    /// Estimated effort in minutes.
    pub duration: Option<u32>,
}
/// Longest estimate a task may carry: a week of wall-clock minutes.
pub const MAX_TASK_DURATION: u32 = 7 * 24 * 60;
#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "ipc-types", derive(specta::Type))]
pub struct TaskList {
    pub id: String,
    pub title: String,
    pub revision: String,
    pub count: u32,
    pub deleted: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "ipc-types", derive(specta::Type))]
pub struct TaskMetadata {
    pub id: String,
    pub list: Option<String>,
    pub inbox: bool,
    pub priority: TaskPriority,
    pub steps: u32,
    pub steps_done: u32,
    pub completed_at: Option<String>,
    pub deadline: Option<String>,
    pub duration: Option<u32>,
}
/// One task property, as a value to write or as a precondition to hold.
///
/// Fields are written independently so two actors editing different properties
/// of one task never conflict; a caller that must not overwrite a concurrent
/// change to the same property states its expected value in `expect`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ipc-types", derive(specta::Type))]
#[serde(tag = "field", rename_all = "snake_case", deny_unknown_fields)]
pub enum TaskField {
    Title {
        value: String,
    },
    Description {
        value: String,
    },
    Status {
        value: TaskStatus,
    },
    Schedule {
        date: Option<String>,
        time: Option<String>,
    },
    Deadline {
        date: Option<String>,
    },
    Duration {
        minutes: Option<u32>,
    },
    Organization {
        list: Option<String>,
        inbox: bool,
    },
    Priority {
        value: TaskPriority,
    },
    Steps {
        value: Vec<TaskStep>,
    },
    Pinned {
        value: bool,
    },
    Position {
        sort_key: Option<String>,
    },
}
const MAX_TASK_FIELDS: usize = 16;
impl TaskField {
    fn slot(&self) -> u8 {
        match self {
            Self::Title { .. } => 0,
            Self::Description { .. } => 1,
            Self::Status { .. } => 2,
            Self::Schedule { .. } => 3,
            Self::Deadline { .. } => 4,
            Self::Duration { .. } => 5,
            Self::Organization { .. } => 6,
            Self::Priority { .. } => 7,
            Self::Steps { .. } => 8,
            Self::Pinned { .. } => 9,
            Self::Position { .. } => 10,
        }
    }
    fn distinct(fields: &[Self]) -> bool {
        let mut seen = 0u16;
        fields.len() <= MAX_TASK_FIELDS
            && fields.iter().all(|field| {
                let bit = 1 << field.slot();
                let fresh = seen & bit == 0;
                seen |= bit;
                fresh
            })
    }
    /// Whether `draft` currently holds this value. Always false for a note.
    pub fn holds(&self, draft: &ResourceDraft) -> bool {
        let ResourceContent::Task {
            details,
            description: body,
            due_date,
            due_time,
            status: state,
            sort_key: key,
            ..
        } = &draft.content
        else {
            return false;
        };
        match self {
            Self::Title { value } => draft.title == *value,
            Self::Description { value } => body == value,
            Self::Status { value } => state == value,
            Self::Schedule { date, time } => due_date == date && due_time == time,
            Self::Deadline { date } => details.deadline == *date,
            Self::Duration { minutes } => details.duration == *minutes,
            Self::Organization { list, inbox } => details.list == *list && details.inbox == *inbox,
            Self::Priority { value } => details.priority == *value,
            Self::Steps { value } => details.steps == *value,
            Self::Pinned { value } => draft.pinned == *value,
            Self::Position { sort_key } => key == sort_key,
        }
    }
    fn apply(&self, draft: &mut ResourceDraft) {
        let ResourceContent::Task {
            details,
            description,
            completed,
            due_date,
            due_time,
            status,
            sort_key,
            ..
        } = &mut draft.content
        else {
            return;
        };
        match self.clone() {
            Self::Title { value } => draft.title = value,
            Self::Description { value } => *description = value,
            Self::Status { value } => {
                *status = value;
                *completed = value == TaskStatus::Done;
            }
            Self::Schedule { date, time } => {
                *due_date = date;
                *due_time = time;
            }
            Self::Deadline { date } => details.deadline = date,
            Self::Duration { minutes } => details.duration = minutes,
            Self::Organization { list, inbox } => {
                details.list = list;
                details.inbox = inbox;
            }
            Self::Priority { value } => details.priority = value,
            Self::Steps { value } => details.steps = value,
            Self::Pinned { value } => draft.pinned = value,
            Self::Position { sort_key: key } => *sort_key = key,
        }
    }
}
/// Applies a field update to a task draft. The result still has to pass
/// `ResourceDraft::validate`; this only decides conflict and shape.
pub fn update_task(
    draft: &ResourceDraft,
    set: &[TaskField],
    expect: &[TaskField],
) -> Result<ResourceDraft, ResourceError> {
    if draft.kind() != ResourceKind::Task {
        return Err(ResourceError::Invalid);
    }
    if !expect.iter().all(|field| field.holds(draft)) {
        return Err(ResourceError::Conflict);
    }
    let mut next = draft.clone();
    for field in set {
        field.apply(&mut next);
    }
    Ok(next)
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ipc-types", derive(specta::Type))]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ResourceContent {
    Note {
        document: NoteDocument,
    },
    /// Fields added after the first release carry `#[serde(default)]` so bodies
    /// written before they existed still load. A later track adding its own
    /// binding here follows the same rule.
    Task {
        #[serde(default)]
        details: TaskDetails,
        description: String,
        /// The persisted projection of `status` that the listing column and
        /// query filter are built from. `validate` keeps the two in step.
        completed: bool,
        due_date: Option<String>,
        /// `HH:MM`, and only alongside a day: a time with no date is not a
        /// moment, and nothing could sort or show it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        due_time: Option<String>,
        #[serde(default)]
        status: TaskStatus,
        #[serde(default)]
        assignee: TaskActor,
        #[serde(default)]
        origin: TaskActor,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        context: Option<TaskContext>,
        /// Manual position within a section, honoured by the list's ordering.
        /// Opaque here: only the ordering of two keys matters, never their
        /// contents. No gesture writes one yet.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sort_key: Option<String>,
        /// Reserved for the Work runtime track, which owns Work identity and
        /// the rules binding a task to one. This crate assigns it no meaning
        /// beyond being a same-profile ULID and enforces no reference.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        work: Option<String>,
    },
    /// A user-owned semantic object: a table, checklist, comparison, chart,
    /// document, or findings, editable like a note.
    Object {
        object: WorkObjectV1,
    },
    /// An imported or admitted file. Bytes live in the profile's
    /// content-addressed media store; this row is its provenance and shape.
    /// Only Rust mints it, after bounded sniffing and decoding.
    Media {
        asset: MediaAssetV1,
    },
}

pub const MAX_MEDIA_IMAGE_BYTES: u32 = 8 * 1024 * 1024;
pub const MAX_MEDIA_FETCHED_IMAGE_BYTES: u32 = 2 * 1024 * 1024;
pub const MAX_MEDIA_FILE_BYTES: u32 = 32 * 1024 * 1024;
pub const MAX_MEDIA_DIMENSION: u32 = 8192;
pub const MAX_MEDIA_NAME_BYTES: usize = 255;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ipc-types", derive(specta::Type))]
#[serde(rename_all = "snake_case")]
pub enum MediaKind {
    Image,
    Pdf,
    File,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ipc-types", derive(specta::Type))]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum MediaOrigin {
    /// A snapshot of a file the user picked; the source path is not retained.
    Imported,
    /// Fetched by Rust from a public HTTPS URL without cookies.
    Fetched { url: String, observed_at: String },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ipc-types", derive(specta::Type))]
#[serde(deny_unknown_fields)]
pub struct MediaAssetV1 {
    pub version: u16,
    pub kind: MediaKind,
    /// Sniffed from bytes, never taken from a file name or a server.
    pub mime: String,
    pub bytes: u32,
    /// Hex SHA-256 of the stored bytes; also the blob's address.
    pub digest: String,
    pub name: String,
    pub origin: MediaOrigin,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
}
impl MediaAssetV1 {
    pub fn validate(&self) -> bool {
        let mime_ok = match self.kind {
            MediaKind::Image => matches!(
                self.mime.as_str(),
                "image/png" | "image/jpeg" | "image/webp" | "image/gif"
            ),
            MediaKind::Pdf => self.mime == "application/pdf",
            MediaKind::File => self.mime == "application/octet-stream",
        };
        let size_ok = self.bytes > 0
            && match (self.kind, &self.origin) {
                (MediaKind::Image, MediaOrigin::Fetched { .. }) => {
                    self.bytes <= MAX_MEDIA_FETCHED_IMAGE_BYTES
                }
                (MediaKind::Image, MediaOrigin::Imported) => self.bytes <= MAX_MEDIA_IMAGE_BYTES,
                (_, MediaOrigin::Fetched { .. }) => false,
                _ => self.bytes <= MAX_MEDIA_FILE_BYTES,
            };
        let dimensions_ok = match self.kind {
            MediaKind::Image => {
                matches!((self.width, self.height), (Some(w), Some(h)) if (1..=MAX_MEDIA_DIMENSION).contains(&w) && (1..=MAX_MEDIA_DIMENSION).contains(&h))
            }
            _ => self.width.is_none() && self.height.is_none(),
        };
        let origin_ok = match &self.origin {
            MediaOrigin::Imported => true,
            MediaOrigin::Fetched { url, observed_at } => {
                url.len() <= 4096
                    && url.starts_with("https://")
                    && !url.chars().any(char::is_control)
                    && valid_date(observed_at)
            }
        };
        self.version == 1
            && mime_ok
            && size_ok
            && dimensions_ok
            && origin_ok
            && self.digest.len() == 64
            && self.digest.bytes().all(|b| b.is_ascii_hexdigit())
            && !self.name.is_empty()
            && self.name.len() <= MAX_MEDIA_NAME_BYTES
            && !self
                .name
                .chars()
                .any(|c| c.is_control() || c == '/' || c == '\\')
    }
}

/// A Rust-owned import request: the bytes never cross the IPC boundary.
#[derive(Clone)]
pub struct MediaImport {
    pub request_id: String,
    pub name: String,
    pub origin: MediaOrigin,
    pub bytes: std::sync::Arc<Vec<u8>>,
}
impl std::fmt::Debug for MediaImport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("MediaImport([redacted])")
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ipc-types", derive(specta::Type))]
#[serde(deny_unknown_fields)]
pub struct WorkObjectV1 {
    pub version: u16,
    pub data: crate::work::artifact::WorkArtifactDataV1,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence: Vec<crate::work::artifact::WorkEvidenceLink>,
    /// Set only by Rust when preserving an artifact; never accepted from callers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provenance: Option<WorkObjectProvenance>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ipc-types", derive(specta::Type))]
#[serde(deny_unknown_fields)]
pub struct WorkObjectProvenance {
    pub objective: crate::work::WorkId,
    pub execution: crate::work::WorkExecutionId,
    pub artifact: crate::work::WorkArtifactId,
    pub basis: WorkObjectBasis,
    pub review: crate::work::WorkOutputReview,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ipc-types", derive(specta::Type))]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum WorkObjectBasis {
    Original,
    UserRevision { revision: crate::work::WorkRevision },
}
impl WorkObjectV1 {
    pub fn validate(&self) -> bool {
        use crate::work::artifact::WorkArtifactDataV1;
        if self.version != 1 || self.evidence.len() > 64 {
            return false;
        }
        let mut unique = std::collections::BTreeSet::new();
        if !self
            .evidence
            .iter()
            .all(|link| link.source_id != 0 && unique.insert((link.extraction_id, link.source_id)))
        {
            return false;
        }
        !matches!(self.data, WorkArtifactDataV1::BrowserResourcePreview { .. })
            && self.data.validate_owned(self.evidence.len()).is_ok()
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ipc-types", derive(specta::Type))]
#[serde(deny_unknown_fields)]
pub struct ResourceDraft {
    pub title: String,
    pub pinned: bool,
    pub content: ResourceContent,
    /// Same-profile resources, never permission grants or copied entities.
    pub related: Vec<String>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ipc-types", derive(specta::Type))]
#[serde(deny_unknown_fields)]
pub struct ResourceRecord {
    pub id: String,
    /// Decimal string; never rounded through JavaScript's number type.
    pub revision: String,
    pub created_at: String,
    pub updated_at: String,
    pub trashed: bool,
    pub draft: ResourceDraft,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ipc-types", derive(specta::Type))]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ResourceIntent {
    CreateTaskList {
        title: String,
    },
    RenameTaskList {
        id: String,
        expected_revision: String,
        title: String,
    },
    DeleteTaskList {
        id: String,
        expected_revision: String,
    },
    Create {
        draft: ResourceDraft,
    },
    /// Copy an artifact (original or the user's revision) into an Object
    /// resource. Rust resolves data, evidence, review, and provenance.
    PreserveArtifact {
        objective: crate::work::WorkId,
        execution: crate::work::WorkExecutionId,
        artifact: crate::work::WorkArtifactId,
        basis: WorkObjectBasis,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
    },
    Replace {
        id: String,
        expected_revision: String,
        draft: ResourceDraft,
    },
    Trash {
        id: String,
        expected_revision: String,
    },
    Restore {
        id: String,
        expected_revision: String,
    },
    /// Writes individual task properties onto the current revision, whatever
    /// it is, after checking the `expect` preconditions against it.
    UpdateTask {
        id: String,
        set: Vec<TaskField>,
        #[serde(default)]
        expect: Vec<TaskField>,
    },
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "ipc-types", derive(specta::Type))]
#[serde(deny_unknown_fields)]
pub struct ResourceCommand {
    pub version: u8,
    pub request_id: String,
    pub intent: ResourceIntent,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "ipc-types", derive(specta::Type))]
#[serde(deny_unknown_fields)]
pub struct ResourceQuery {
    #[serde(default)]
    pub completed: Option<bool>,
    pub kind: ResourceKind,
    pub search: String,
    pub trashed: bool,
    pub after: Option<String>,
    pub limit: u16,
}
/// Task views are filtered and ordered before pagination, independently of the
/// generic resource browser. `today` is the caller's local calendar date.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ipc-types", derive(specta::Type))]
#[serde(rename_all = "snake_case")]
pub enum TaskView {
    Inbox,
    Today,
    Upcoming,
    All,
    Completed,
    Trash,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "ipc-types", derive(specta::Type))]
#[serde(deny_unknown_fields)]
pub struct TaskQuery {
    #[serde(default)]
    pub list: Option<String>,
    pub view: TaskView,
    pub today: String,
    pub search: String,
    pub after: Option<String>,
    pub limit: u16,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "ipc-types", derive(specta::Type))]
pub struct TaskCounts {
    pub inbox: u32,
    pub today: u32,
    pub overdue: u32,
    pub upcoming: u32,
    pub all: u32,
    pub completed: u32,
    pub trash: u32,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ipc-types", derive(specta::Type))]
#[serde(rename_all = "snake_case")]
pub enum ResourceKind {
    Note,
    Task,
    Object,
    Media,
}
/// Everything a list row draws, so a populated list costs one query rather than
/// one query and a fetch per row. Task-only fields are `None` for a note.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "ipc-types", derive(specta::Type))]
pub struct ResourceSummary {
    pub id: String,
    pub revision: String,
    pub title: String,
    pub pinned: bool,
    pub updated_at: String,
    pub completed: Option<bool>,
    pub due_date: Option<String>,
    pub due_time: Option<String>,
    pub status: Option<TaskStatus>,
    pub assignee: Option<TaskActor>,
    pub origin: Option<TaskActor>,
    pub context: Option<TaskContext>,
    pub sort_key: Option<String>,
    pub work: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "ipc-types", derive(specta::Type))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ResourceResponse {
    Acknowledged,
    Record {
        record: ResourceRecord,
    },
    Page {
        items: Vec<ResourceSummary>,
        next: Option<String>,
    },
    TaskListApplied {
        request_id: String,
        list: TaskList,
    },
    TaskPage {
        lists: Vec<TaskList>,
        metadata: Vec<TaskMetadata>,
        items: Vec<ResourceSummary>,
        next: Option<String>,
        counts: TaskCounts,
    },
    TaskOverview {
        lists: Vec<TaskList>,
        counts: TaskCounts,
    },
    Applied {
        request_id: String,
        applied_revision: String,
        record: ResourceRecord,
    },
    Error {
        error: ResourceError,
    },
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ipc-types", derive(specta::Type))]
#[serde(rename_all = "snake_case")]
pub enum ResourceError {
    Invalid,
    NotFound,
    Conflict,
    Capacity,
    Unavailable,
    OutcomeUnknown,
}

impl ResourceDraft {
    /// Callers never supply provenance or media; only Rust admission mints them.
    pub fn caller_owned(&self) -> bool {
        match &self.content {
            ResourceContent::Object { object } => object.provenance.is_none(),
            ResourceContent::Media { .. } => false,
            _ => true,
        }
    }
    pub fn kind(&self) -> ResourceKind {
        match self.content {
            ResourceContent::Note { .. } => ResourceKind::Note,
            ResourceContent::Task { .. } => ResourceKind::Task,
            ResourceContent::Object { .. } => ResourceKind::Object,
            ResourceContent::Media { .. } => ResourceKind::Media,
        }
    }
    pub fn validate(&self) -> bool {
        if self.title.trim().is_empty()
            || self.title.len() > 1024
            || self.title.contains('\0')
            || self.related.len() > 64
        {
            return false;
        }
        let mut seen = std::collections::HashSet::new();
        if !self
            .related
            .iter()
            .all(|id| valid_id(id) && seen.insert(id))
        {
            return false;
        }
        match &self.content {
            ResourceContent::Note { document } => document.validate(),
            ResourceContent::Task {
                details,
                description,
                completed,
                due_date,
                due_time,
                status,
                context,
                sort_key,
                work,
                ..
            } => {
                details.list.as_deref().is_none_or(valid_id)
                    && !(details.inbox && details.list.is_some())
                    && details.steps.len() <= 100
                    && {
                        let mut ids = std::collections::HashSet::new();
                        details.steps.iter().all(|step| {
                            valid_request(&step.id)
                                && ids.insert(&step.id)
                                && !step.title.trim().is_empty()
                                && step.title.len() <= 1024
                                && !step.title.contains('\0')
                        })
                    }
                    && description.len() <= 16_384
                    && !description.contains('\0')
                    && details.deadline.as_deref().is_none_or(valid_date)
                    && details
                        .duration
                        .is_none_or(|minutes| (1..=MAX_TASK_DURATION).contains(&minutes))
                    && due_date.as_deref().is_none_or(valid_date)
                    && due_time.as_deref().is_none_or(valid_time)
                    && (due_time.is_none() || due_date.is_some())
                    && (*status == TaskStatus::Done) == *completed
                    && context.as_ref().is_none_or(TaskContext::validate)
                    && sort_key.as_deref().is_none_or(valid_sort_key)
                    && work.as_deref().is_none_or(valid_id)
            }
            ResourceContent::Object { object } => object.validate(),
            ResourceContent::Media { asset } => asset.validate(),
        }
    }
}
/// Manual ordering keys are compared, never parsed. Bounding them to printable
/// ASCII keeps that comparison identical everywhere the list is drawn.
fn valid_sort_key(value: &str) -> bool {
    (1..=64).contains(&value.len()) && value.bytes().all(|b| (0x21..=0x7e).contains(&b))
}
/// `HH:MM` on a 24-hour clock. Seconds and zones are deliberately absent: a due
/// time is a wall-clock moment on a calendar day, not an instant.
fn valid_time(value: &str) -> bool {
    let bytes = value.as_bytes();
    if value.len() != 5
        || bytes[2] != b':'
        || !bytes
            .iter()
            .enumerate()
            .all(|(i, b)| i == 2 || b.is_ascii_digit())
    {
        return false;
    }
    let (Ok(hour), Ok(minute)) = (value[..2].parse::<u32>(), value[3..].parse::<u32>()) else {
        return false;
    };
    hour < 24 && minute < 60
}
pub fn valid_id(value: &str) -> bool {
    value.len() == 26 && ulid::Ulid::from_string(value).is_ok_and(|id| id.to_string() == value)
}
pub fn revision(value: &str) -> Option<i64> {
    value
        .parse::<i64>()
        .ok()
        .filter(|n| *n > 0 && n.to_string() == value)
}
pub fn valid_request(value: &str) -> bool {
    (16..=128).contains(&value.len())
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
}
fn valid_date(value: &str) -> bool {
    if value.len() != 10
        || value.as_bytes()[4] != b'-'
        || value.as_bytes()[7] != b'-'
        || !value
            .bytes()
            .enumerate()
            .all(|(i, b)| i == 4 || i == 7 || b.is_ascii_digit())
    {
        return false;
    }
    let Ok(year) = value[..4].parse::<u32>() else {
        return false;
    };
    let Ok(month) = value[5..7].parse::<usize>() else {
        return false;
    };
    let Ok(day) = value[8..].parse::<u32>() else {
        return false;
    };
    let leap = year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400));
    let days = [
        31,
        if leap { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    year >= 1 && (1..=12).contains(&month) && (1..=days[month - 1]).contains(&day)
}
impl NoteDocument {
    /// Text content in document order; headings and paragraphs separated by newlines.
    pub fn plain_text(&self) -> String {
        let mut text = String::new();
        let mut pending = vec![&self.document];
        while let Some(node) = pending.pop() {
            if let Some(value) = &node.text {
                text.push_str(value);
            } else if matches!(
                node.kind.as_str(),
                "paragraph" | "heading" | "listItem" | "codeBlock" | "blockquote"
            ) && !text.is_empty()
                && !text.ends_with('\n')
            {
                text.push('\n');
            }
            pending.extend(node.content.iter().rev());
        }
        text
    }
    pub fn validate(&self) -> bool {
        let mut count = 0;
        let mut bytes = 0;
        self.version == 1
            && self.document.kind == "doc"
            && valid_node(&self.document, "", 0, &mut count, &mut bytes)
            && self
                .references()
                .into_iter()
                .collect::<std::collections::HashSet<_>>()
                .len()
                <= 64
    }
    pub fn references(&self) -> Vec<&str> {
        let mut refs = Vec::new();
        let mut pending = vec![&self.document];
        while let Some(node) = pending.pop() {
            if let Some(id) = node.attrs.as_ref().and_then(|a| a.resource.as_deref()) {
                refs.push(id);
            }
            pending.extend(&node.content);
        }
        refs
    }
}
fn valid_node(
    node: &DocumentNode,
    parent: &str,
    depth: usize,
    count: &mut usize,
    bytes: &mut usize,
) -> bool {
    *count += 1;
    *bytes += node.text.as_ref().map_or(0, String::len);
    if depth > 16 || *count > MAX_DOCUMENT_NODES || *bytes > MAX_DOCUMENT_BYTES {
        return false;
    }
    let inline = matches!(node.kind.as_str(), "text" | "hardBreak" | "noteReference");
    let allowed = match parent {
        "" => node.kind == "doc",
        "doc" | "blockquote" | "listItem" => matches!(
            node.kind.as_str(),
            "paragraph" | "heading" | "bulletList" | "orderedList" | "blockquote" | "codeBlock"
        ),
        "paragraph" | "heading" => inline,
        "codeBlock" => node.kind == "text" && node.marks.is_empty(),
        "bulletList" | "orderedList" => node.kind == "listItem",
        _ => false,
    };
    if !allowed {
        return false;
    }
    if node.kind == "text" {
        if node
            .text
            .as_ref()
            .is_none_or(|t| t.is_empty() || t.contains('\0'))
            || !node.content.is_empty()
        {
            return false;
        }
    } else if node.text.is_some() || !node.marks.is_empty() {
        return false;
    }
    if inline && !node.content.is_empty() {
        return false;
    }
    if matches!(
        node.kind.as_str(),
        "doc" | "listItem" | "bulletList" | "orderedList" | "blockquote"
    ) && node.content.is_empty()
    {
        return false;
    }
    if node.kind == "listItem"
        && node
            .content
            .first()
            .is_none_or(|child| child.kind != "paragraph")
    {
        return false;
    }
    let attrs_ok = match (&*node.kind, &node.attrs) {
        ("heading", Some(attrs)) => {
            attrs.level.is_some_and(|n| (1..=3).contains(&n))
                && attrs.resource.is_none()
                && attrs.start.is_none()
        }
        ("noteReference", Some(attrs)) => {
            attrs.level.is_none()
                && attrs.start.is_none()
                && attrs.resource.as_deref().is_some_and(valid_id)
        }
        ("orderedList", Some(attrs)) => {
            attrs.level.is_none()
                && attrs.resource.is_none()
                && attrs
                    .start
                    .is_some_and(|start| (-1_000_000..=1_000_000).contains(&start))
        }
        ("heading" | "noteReference", None) => false,
        (_, None) => true,
        _ => false,
    };
    let mut marks = std::collections::HashSet::new();
    attrs_ok
        && node.marks.len() <= 4
        && node.marks.iter().all(|mark| {
            let shape = match mark.kind.as_str() {
                "bold" | "italic" | "code" => mark.attrs.is_none(),
                "link" => mark.attrs.as_ref().is_some_and(|attrs| {
                    attrs.href.as_deref().is_some_and(|href| {
                        href.len() <= 2048 && crate::navigation::is_allowed_str(href)
                    })
                }),
                _ => false,
            };
            shape && marks.insert(&mark.kind)
        })
        && node
            .content
            .iter()
            .all(|child| valid_node(child, &node.kind, depth + 1, count, bytes))
}

#[cfg(test)]
#[path = "resources/tests.rs"]
mod tests;

#[derive(Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "ipc-types", derive(specta::Type))]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ResourceCall {
    Acknowledge {
        request_id: String,
    },
    List {
        query: ResourceQuery,
    },
    ListTasks {
        query: TaskQuery,
    },
    /// Navigation counts and lists without a page of rows, for refreshing
    /// totals after a write whose record the caller already holds.
    TaskOverview {
        today: String,
    },
    Get {
        id: String,
    },
    // Boxed: a command is several times the size of every other call.
    Mutate {
        command: Box<ResourceCommand>,
    },
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "ipc-types", derive(specta::Type))]
pub struct ResourceReply {
    pub profile: Option<String>,
    pub response: ResourceResponse,
}
pub type ResourceDone = Box<dyn FnOnce(ResourceResponse) + Send>;
impl ResourceCall {
    pub fn validate(&self) -> bool {
        match self {
            Self::Acknowledge { request_id } => valid_request(request_id),
            Self::Get { id } => valid_id(id),
            Self::TaskOverview { today } => valid_date(today),
            Self::ListTasks { query } => {
                query.list.as_deref().is_none_or(valid_id)
                    && valid_date(&query.today)
                    && query.search.len() <= 512
                    && (1..=100).contains(&query.limit)
                    && query.after.as_ref().is_none_or(|v| v.len() <= 2048)
            }
            Self::List { query } => {
                query.search.len() <= 512
                    && query.limit > 0
                    && query.limit <= 100
                    && query.after.as_ref().is_none_or(|v| v.len() <= 64)
            }
            Self::Mutate { command } => {
                command.version == 1
                    && valid_request(&command.request_id)
                    && match &command.intent {
                        ResourceIntent::CreateTaskList { title } => {
                            !title.trim().is_empty() && title.len() <= 256 && !title.contains('\0')
                        }
                        ResourceIntent::RenameTaskList {
                            id,
                            expected_revision,
                            title,
                        } => {
                            valid_id(id)
                                && revision(expected_revision).is_some()
                                && !title.trim().is_empty()
                                && title.len() <= 256
                                && !title.contains('\0')
                        }
                        ResourceIntent::DeleteTaskList {
                            id,
                            expected_revision,
                        } => valid_id(id) && revision(expected_revision).is_some(),
                        ResourceIntent::Create { draft } => {
                            draft.validate() && draft.caller_owned()
                        }
                        ResourceIntent::PreserveArtifact { title, .. } => title
                            .as_deref()
                            .is_none_or(|t| !t.trim().is_empty() && t.len() <= 1024),
                        ResourceIntent::Replace {
                            id,
                            expected_revision,
                            draft,
                        } => {
                            valid_id(id)
                                && revision(expected_revision).is_some()
                                && draft.validate()
                                && draft.caller_owned()
                        }
                        ResourceIntent::Trash {
                            id,
                            expected_revision,
                        }
                        | ResourceIntent::Restore {
                            id,
                            expected_revision,
                        } => valid_id(id) && revision(expected_revision).is_some(),
                        ResourceIntent::UpdateTask { id, set, expect } => {
                            valid_id(id)
                                && !set.is_empty()
                                && TaskField::distinct(set)
                                && TaskField::distinct(expect)
                        }
                    }
            }
        }
    }
}

impl std::fmt::Debug for ResourceCall {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ResourceCall (content redacted)")
    }
}

#[cfg(test)]
mod task_context_bounds_tests {
    use super::*;

    #[test]
    fn task_context_keeps_its_eight_kib_raw_and_serialized_url_budget() {
        let prefix = "https://example.test/";
        let at_bound = format!(
            "{prefix}{}",
            "x".repeat(MAX_TASK_CONTEXT_URL_BYTES - prefix.len())
        );
        let context = |url| TaskContext {
            url,
            title: "Source page".into(),
        };
        assert!(context(at_bound.clone()).validate());
        assert!(!context(format!("{at_bound}x")).validate());
        assert!(!context(format!("{at_bound}\n")).validate());
        let expanded = format!(
            "{prefix}{}",
            "é".repeat((MAX_TASK_CONTEXT_URL_BYTES - prefix.len()) / 2)
        );
        assert!(expanded.len() <= MAX_TASK_CONTEXT_URL_BYTES);
        assert!(crate::navigation::is_allowed_str(&expanded));
        assert!(!context(expanded).validate());
    }
}
