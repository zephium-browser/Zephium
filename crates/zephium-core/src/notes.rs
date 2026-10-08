//! Notes are Markdown files the person owns. The folder is the record; the
//! index, identities and everything below only describe it, and all of it can
//! be rebuilt from the files.
use serde::{Deserialize, Serialize};
use specta::Type;

use crate::resources::{valid_id, valid_request};

/// Largest note the editor accepts. A larger file stays in the folder and in
/// the list, but opens read-only rather than being truncated on save.
pub const MAX_NOTE_BYTES: usize = 1024 * 1024;
pub const MAX_NOTES: usize = 10_000;
pub const MAX_NOTE_PAGE: u16 = 200;
const MAX_SEARCH_BYTES: usize = 512;
const MAX_LINK_TARGETS: usize = 128;
const MAX_LINK_TARGET_BYTES: usize = 512;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct NoteSummary {
    pub id: String,
    /// Content hash of the bytes on disk. Stable across restarts and index
    /// rebuilds, so a writer can prove which version it edited.
    pub revision: String,
    pub title: String,
    /// Plain text after the title, collapsed to one line.
    pub preview: String,
    pub pinned: bool,
    pub trashed: bool,
    /// False when the file is too large or not UTF-8. It is listed and
    /// readable elsewhere but never rewritten from a partial view.
    pub editable: bool,
    /// Milliseconds since the Unix epoch, as decimal strings.
    pub created_at: String,
    pub modified_at: String,
    /// Folder-relative path with `/` separators, for display only.
    pub path: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct NoteRecord {
    pub summary: NoteSummary,
    pub markdown: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(deny_unknown_fields)]
pub struct NoteQuery {
    pub search: String,
    pub trashed: bool,
    pub after: Option<String>,
    pub limit: u16,
}

/// What a `[[target]]` in a note currently points at.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct NoteTarget {
    pub target: String,
    pub note: Option<NoteSummary>,
}

#[derive(Clone, Debug, Serialize, Deserialize, Type)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum NoteCall {
    List {
        query: NoteQuery,
    },
    Get {
        id: String,
    },
    Create {
        request_id: String,
        markdown: String,
    },
    /// Replaces the file only if it still holds `base_revision`. Replaying the
    /// same write after an unknown outcome succeeds without a second change.
    Write {
        request_id: String,
        id: String,
        base_revision: String,
        markdown: String,
        /// The person has stopped retitling the note, or left it. Only then
        /// does a file named after its title take the new one, so typing a
        /// heading does not rename the file on every pause.
        #[serde(default)]
        settle: bool,
    },
    SetPinned {
        id: String,
        pinned: bool,
    },
    Trash {
        id: String,
    },
    Restore {
        id: String,
    },
    /// Permanent. Only a note already in the trash can be deleted.
    Delete {
        id: String,
    },
    Resolve {
        targets: Vec<String>,
    },
    Backlinks {
        id: String,
    },
    /// Shows the note, or the folder when `id` is absent, in the system file manager.
    Reveal {
        id: Option<String>,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize, Type)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum NoteResponse {
    Done,
    Page {
        items: Vec<NoteSummary>,
        next: Option<String>,
    },
    Record {
        record: NoteRecord,
    },
    Applied {
        request_id: String,
        summary: NoteSummary,
    },
    /// The file changed since `base_revision`; nothing was written.
    Conflict {
        current: NoteRecord,
    },
    Targets {
        items: Vec<NoteTarget>,
    },
    Error {
        error: NoteError,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum NoteError {
    Invalid,
    NotFound,
    Capacity,
    TooLarge,
    ReadOnly,
    Unavailable,
    OutcomeUnknown,
}

#[derive(Clone, Debug, Serialize, Deserialize, Type)]
pub struct NoteReply {
    pub profile: Option<String>,
    pub response: NoteResponse,
}

pub type NoteDone = Box<dyn FnOnce(NoteResponse) + Send>;

/// A note that changed, and the revision it now has on disk, or `None` when
/// it no longer exists.
#[derive(Clone, Debug, Serialize, Deserialize, Type)]
pub struct ChangedNote {
    pub id: String,
    pub revision: Option<String>,
}

/// Notes that changed on disk, by this browser or anything else. `reset`
/// means the listing itself may have changed beyond the named notes.
#[derive(Clone, Debug, Serialize, Deserialize, Type)]
pub struct NoteChanges {
    pub profile: String,
    pub notes: Vec<ChangedNote>,
    pub reset: bool,
    /// Link keys that may now lead to a different note, because a note's
    /// title or file name changed from or to them. A retitled note is news
    /// for its links, not for the listing.
    pub links: Vec<String>,
}

pub fn valid_revision(value: &str) -> bool {
    value.len() == 32
        && value
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

fn valid_markdown(value: &str) -> bool {
    value.len() <= MAX_NOTE_BYTES && !value.contains('\0')
}

impl NoteCall {
    pub fn validate(&self) -> bool {
        match self {
            Self::List { query } => {
                query.search.len() <= MAX_SEARCH_BYTES
                    && (1..=MAX_NOTE_PAGE).contains(&query.limit)
                    && query.after.as_ref().is_none_or(|cursor| cursor.len() <= 64)
            }
            Self::Get { id }
            | Self::SetPinned { id, .. }
            | Self::Trash { id }
            | Self::Restore { id }
            | Self::Delete { id }
            | Self::Backlinks { id } => valid_id(id),
            Self::Reveal { id } => id.as_deref().is_none_or(valid_id),
            Self::Create {
                request_id,
                markdown,
            } => valid_request(request_id) && valid_markdown(markdown),
            Self::Write {
                request_id,
                id,
                base_revision,
                markdown,
                ..
            } => {
                valid_request(request_id)
                    && valid_id(id)
                    && valid_revision(base_revision)
                    && valid_markdown(markdown)
            }
            Self::Resolve { targets } => {
                targets.len() <= MAX_LINK_TARGETS
                    && targets
                        .iter()
                        .all(|target| !target.is_empty() && target.len() <= MAX_LINK_TARGET_BYTES)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "01J9ZQ3V6Q4M8Y2K7T5R1N0B3C";

    #[test]
    fn writes_carry_a_content_revision() {
        let write = |base: &str| NoteCall::Write {
            request_id: "request-0000000001".into(),
            id: ID.into(),
            base_revision: base.into(),
            markdown: "# A".into(),
            settle: false,
        };
        assert!(write(&"a".repeat(32)).validate());
        assert!(!write(&"A".repeat(32)).validate());
        assert!(!write("7").validate());
    }

    #[test]
    fn a_write_that_does_not_say_settles_nothing() {
        let call: NoteCall = serde_json::from_value(serde_json::json!({
            "kind": "write",
            "request_id": "request-0000000001",
            "id": ID,
            "base_revision": "a".repeat(32),
            "markdown": "# A",
        }))
        .unwrap();
        assert!(matches!(call, NoteCall::Write { settle: false, .. }));
    }

    #[test]
    fn bodies_are_bounded_and_text() {
        let create = |markdown: String| NoteCall::Create {
            request_id: "request-0000000001".into(),
            markdown,
        };
        assert!(create("x".repeat(MAX_NOTE_BYTES)).validate());
        assert!(!create("x".repeat(MAX_NOTE_BYTES + 1)).validate());
        assert!(!create("a\0b".into()).validate());
    }

    #[test]
    fn pages_are_bounded() {
        let list = |limit| NoteCall::List {
            query: NoteQuery {
                search: String::new(),
                trashed: false,
                after: None,
                limit,
            },
        };
        assert!(list(1).validate());
        assert!(!list(0).validate());
        assert!(!list(MAX_NOTE_PAGE + 1).validate());
    }
}
