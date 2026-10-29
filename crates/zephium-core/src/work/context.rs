//! Admitted Work context: what leaves the device for one model operation.
//! Rust resolves selected elements to bodies at operation begin; the persisted
//! manifest records identity, size, and digest, never the bodies themselves.
use super::*;
use sha2::{Digest, Sha256};

pub const MAX_CONTEXT_ITEMS: usize = 16;
pub const MAX_CONTEXT_ITEM_BYTES: usize = 6 * 1024;
pub const MAX_CONTEXT_TOTAL_BYTES: usize = 20 * 1024;
pub const MAX_CONTEXT_REVISION_BYTES: usize = 128;

/// The user's selection as displayed: element ids plus the identity token the
/// UI saw for each (resource revision, artifact id, tab URL, Work revision).
#[cfg_attr(feature = "ipc-types", derive(specta::Type))]
#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct WorkContextSelectionV1 {
    pub environment: WorkEnvironmentId,
    pub items: Vec<WorkContextSelectionItem>,
    /// The person's consent, for this request, to list their open tabs of
    /// the current window as context: title, host and path, never content.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub tabs: bool,
    /// This request may resolve a specifically referenced block on its canvas.
    /// No other block bodies or titles are disclosed implicitly.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub blocks: bool,
}
#[cfg_attr(feature = "ipc-types", derive(specta::Type))]
#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct WorkContextSelectionItem {
    pub element: WorkElementId,
    pub revision: String,
}
impl WorkContextSelectionV1 {
    pub fn validate(&self) -> Result<(), WorkError> {
        if (self.items.is_empty() && !self.tabs && !self.blocks)
            || self.items.len() > MAX_CONTEXT_ITEMS
        {
            return Err(WorkError::Invalid);
        }
        let mut seen = std::collections::HashSet::new();
        for item in &self.items {
            if item.revision.len() > MAX_CONTEXT_REVISION_BYTES
                || item.revision.chars().any(char::is_control)
                || !seen.insert(item.element)
            {
                return Err(WorkError::Invalid);
            }
        }
        Ok(())
    }
}

#[cfg_attr(feature = "ipc-types", derive(specta::Type))]
#[derive(Clone, Copy, Debug, Serialize, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum WorkContextItemKind {
    Note,
    Task,
    Object,
    Tab,
    Objective,
    Artifact,
    Subject,
    Finding,
    Source,
    /// The user's recorded choice about an element; added by Rust, not selected.
    Decision,
}
impl WorkContextItemKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Note => "note",
            Self::Task => "task",
            Self::Object => "object",
            Self::Tab => "tab",
            Self::Objective => "objective",
            Self::Artifact => "result",
            Self::Subject => "subject",
            Self::Finding => "finding",
            Self::Source => "source",
            Self::Decision => "decision",
        }
    }
}

/// Public items originate from public provider research and may accompany a
/// user-directed public read. Everything the user wrote or browsed is private.
#[cfg_attr(feature = "ipc-types", derive(specta::Type))]
#[derive(Clone, Copy, Debug, Serialize, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum WorkContextVisibility {
    Public,
    Private,
}

#[cfg_attr(feature = "ipc-types", derive(specta::Type))]
#[derive(Clone, Copy, Debug, Serialize, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum WorkContextPurpose {
    Planning,
    PublicRead,
    /// The routine agent loop: the model sees the bodies; Rust refuses search
    /// text that repeats private context.
    Agent,
}

#[cfg_attr(feature = "ipc-types", derive(specta::Type))]
#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct WorkContextItemV1 {
    pub element: WorkElementId,
    pub kind: WorkContextItemKind,
    pub title: String,
    pub revision: String,
    /// Hex SHA-256 of the admitted body after truncation.
    pub digest: String,
    pub bytes: u32,
    pub truncated: bool,
    pub visibility: WorkContextVisibility,
    /// Added by Rust from durable Work state (decisions), not by selection.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub implicit: bool,
}

/// The persisted manifest: bound to the plan revision or execution spec it
/// informed, renderable by chrome before dispatch from the same struct.
#[cfg_attr(feature = "ipc-types", derive(specta::Type))]
#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct WorkContextDisclosureV1 {
    pub version: u16,
    pub environment: WorkEnvironmentId,
    pub environment_revision: WorkRevision,
    pub purpose: WorkContextPurpose,
    pub items: Vec<WorkContextItemV1>,
    pub total_bytes: u32,
    /// Open tabs the person consented to list; the canvas shows them as page
    /// cards without a read.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tabs: Vec<WorkContextTabV1>,
    /// Request-bound canvas access for uniquely resolved block tools.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub blocks: bool,
}
pub const MAX_CONTEXT_TABS: usize = 60;
pub const MAX_CONTEXT_TAB_TITLE_BYTES: usize = 256;
pub const MAX_CONTEXT_TAB_PATH_BYTES: usize = 512;
/// One open tab: never its page content or query.
#[cfg_attr(feature = "ipc-types", derive(specta::Type))]
#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct WorkContextTabV1 {
    pub title: String,
    pub host: String,
    pub path: String,
    /// The profile holds a first-party cookie for this host: a closed fact,
    /// never an account or a name. It offers a grant; it never grants one.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub signed_in: bool,
}
impl WorkContextTabV1 {
    /// An HTTPS tab as title, host and path; query and fragment stay behind.
    pub fn from_page(title: &str, url: &str) -> Option<Self> {
        let url = url::Url::parse(url).ok()?;
        let host = url.host_str()?.to_owned();
        if url.scheme() != "https" || !url.username().is_empty() || url.password().is_some() {
            return None;
        }
        let path = url.path();
        let (path, _) = truncate(path, MAX_CONTEXT_TAB_PATH_BYTES);
        let title: String = title.chars().filter(|c| !c.is_control()).collect();
        let (title, _) = truncate(&title, MAX_CONTEXT_TAB_TITLE_BYTES);
        let tab = Self {
            title: if title.is_empty() {
                host.clone()
            } else {
                title
            },
            host,
            path,
            signed_in: false,
        };
        tab.validate().ok().map(|()| tab)
    }
    pub fn validate(&self) -> Result<(), WorkError> {
        if self.title.trim().is_empty()
            || self.title.len() > MAX_CONTEXT_TAB_TITLE_BYTES
            || self.title.chars().any(char::is_control)
            || self.host.is_empty()
            || self.host.len() > 253
            || !self
                .host
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b':' | b'[' | b']'))
            || !self.path.starts_with('/')
            || self.path.len() > MAX_CONTEXT_TAB_PATH_BYTES
            || self
                .path
                .chars()
                .any(|c| c.is_control() || matches!(c, '?' | '#'))
        {
            return Err(WorkError::Invalid);
        }
        Ok(())
    }
    pub fn url(&self) -> String {
        format!("https://{}{}", self.host, self.path)
    }
}
impl WorkContextDisclosureV1 {
    pub fn validate(&self) -> Result<(), WorkError> {
        if self.version != 1
            || (self.items.is_empty() && self.tabs.is_empty() && !self.blocks)
            || self.items.len() > MAX_CONTEXT_ITEMS
            || self.tabs.len() > MAX_CONTEXT_TABS
        {
            return Err(WorkError::Invalid);
        }
        for tab in &self.tabs {
            tab.validate()?;
        }
        let mut seen = std::collections::HashSet::new();
        let mut total = 0u64;
        for item in &self.items {
            if !seen.insert((item.element, item.implicit))
                || item.title.len() > MAX_WORK_TEXT_BYTES
                || item.revision.len() > MAX_CONTEXT_REVISION_BYTES
                || item.digest.len() != 64
                || !item.digest.bytes().all(|b| b.is_ascii_hexdigit())
                || item.bytes as usize > MAX_CONTEXT_ITEM_BYTES
            {
                return Err(WorkError::Invalid);
            }
            total += u64::from(item.bytes);
        }
        if total != u64::from(self.total_bytes) || total as usize > MAX_CONTEXT_TOTAL_BYTES {
            return Err(WorkError::Invalid);
        }
        Ok(())
    }
    pub fn requires_review(&self) -> bool {
        self.blocks
            || !self.tabs.is_empty()
            || self
                .items
                .iter()
                .any(|item| item.visibility == WorkContextVisibility::Private)
    }
}

/// One admitted body, provider-facing only. Never persisted.
#[derive(Clone, Serialize, Eq, PartialEq)]
pub struct WorkContextBody {
    pub kind: WorkContextItemKind,
    pub title: String,
    pub text: String,
}
impl std::fmt::Debug for WorkContextBody {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("WorkContextBody([redacted])")
    }
}

/// A resolved source for one selected element, before budgeting.
pub struct WorkContextSource {
    pub element: WorkElementId,
    pub kind: WorkContextItemKind,
    pub title: String,
    pub revision: String,
    pub visibility: WorkContextVisibility,
    pub text: String,
}

/// Admitted bodies with the exact manifest that describes them.
#[derive(Clone)]
pub struct WorkAdmittedContext {
    pub disclosure: WorkContextDisclosureV1,
    pub bodies: Vec<WorkContextBody>,
}
impl WorkAdmittedContext {
    /// Truncates bodies at character boundaries, digests the admitted text,
    /// and refuses when the selection's identity tokens no longer match.
    pub fn admit(
        environment: WorkEnvironmentId,
        environment_revision: WorkRevision,
        purpose: WorkContextPurpose,
        selection: &WorkContextSelectionV1,
        sources: Vec<WorkContextSource>,
        implicit: Vec<WorkContextSource>,
    ) -> Result<Self, WorkError> {
        Self::admit_with_tabs(
            environment,
            environment_revision,
            purpose,
            selection,
            sources,
            implicit,
            Vec::new(),
        )
    }
    /// As `admit`, with the open tabs listed under the selection's consent.
    pub fn admit_with_tabs(
        environment: WorkEnvironmentId,
        environment_revision: WorkRevision,
        purpose: WorkContextPurpose,
        selection: &WorkContextSelectionV1,
        sources: Vec<WorkContextSource>,
        implicit: Vec<WorkContextSource>,
        mut tabs: Vec<WorkContextTabV1>,
    ) -> Result<Self, WorkError> {
        selection.validate()?;
        if !selection.tabs && !tabs.is_empty() {
            return Err(WorkError::Invalid);
        }
        tabs.truncate(MAX_CONTEXT_TABS);
        if sources.len() != selection.items.len() {
            return Err(WorkError::NotFound);
        }
        if selection.items.len() + implicit.len() > MAX_CONTEXT_ITEMS {
            return Err(WorkError::Capacity);
        }
        let mut items = Vec::with_capacity(sources.len() + implicit.len());
        let mut bodies = Vec::with_capacity(sources.len() + implicit.len());
        let mut total = 0usize;
        let selected = sources
            .into_iter()
            .zip(&selection.items)
            .map(|(source, selected)| {
                if source.element != selected.element {
                    return Err(WorkError::Invalid);
                }
                if source.revision != selected.revision {
                    return Err(WorkError::Conflict);
                }
                Ok((source, false))
            });
        let implicit = implicit.into_iter().map(|source| Ok((source, true)));
        for entry in selected.chain(implicit) {
            let (source, implicit) = entry?;
            let (text, truncated) = truncate(&source.text, MAX_CONTEXT_ITEM_BYTES);
            total += text.len();
            if total > MAX_CONTEXT_TOTAL_BYTES {
                return Err(WorkError::Capacity);
            }
            let digest = hex(&Sha256::digest(text.as_bytes()));
            items.push(WorkContextItemV1 {
                element: source.element,
                kind: source.kind,
                title: source.title.clone(),
                revision: source.revision,
                digest,
                bytes: text.len() as u32,
                truncated,
                visibility: source.visibility,
                implicit,
            });
            bodies.push(WorkContextBody {
                kind: source.kind,
                title: source.title,
                text,
            });
        }
        let disclosure = WorkContextDisclosureV1 {
            version: 1,
            environment,
            environment_revision,
            purpose,
            items,
            total_bytes: total as u32,
            tabs,
            blocks: selection.blocks,
        };
        disclosure.validate()?;
        if purpose == WorkContextPurpose::PublicRead && disclosure.requires_review() {
            return Err(WorkError::ReviewRequired);
        }
        Ok(Self { disclosure, bodies })
    }
    pub fn bytes(&self) -> usize {
        self.disclosure.total_bytes as usize
    }
}
impl std::fmt::Debug for WorkAdmittedContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("WorkAdmittedContext([redacted])")
    }
}

/// Clips a context body at a UTF-8 boundary and reports omitted text.
pub fn truncate(text: &str, max: usize) -> (String, bool) {
    let text = text.trim();
    if text.len() <= max {
        return (text.to_owned(), false);
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    (text[..end].to_owned(), true)
}

/// A tab or link's identity token: its URL routinely outgrows `MAX_CONTEXT_REVISION_BYTES`.
pub fn url_revision(url: &str) -> String {
    hex(&Sha256::digest(url.as_bytes()))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// One subject's row of a matrix or its findings as compact text.
pub fn subject_body(data: &artifact::WorkArtifactDataV1, index: u16) -> Option<(String, String)> {
    use artifact::{WorkArtifactDataV1, WorkCellValue};
    let index = usize::from(index);
    match data {
        WorkArtifactDataV1::ComparisonMatrix {
            subjects,
            criteria,
            cells,
            ..
        } => {
            let subject = subjects.get(index)?;
            let mut text = String::new();
            if let Some(descriptor) = &subject.descriptor {
                text.push_str(descriptor);
            }
            for (column, criterion) in criteria.iter().enumerate() {
                let Some(cell) = cells.get(index).and_then(|row| row.get(column)) else {
                    continue;
                };
                let value = match &cell.value {
                    WorkCellValue::Text { text } => text.clone(),
                    WorkCellValue::Measurement { value } => value.clone(),
                    WorkCellValue::Money {
                        amount, currency, ..
                    } => format!("{amount} {currency}"),
                    WorkCellValue::Rating { value } => value.to_string(),
                    WorkCellValue::Presence { present } => {
                        if *present { "yes" } else { "no" }.to_owned()
                    }
                    WorkCellValue::Unknown => continue,
                };
                text.push('\n');
                text.push_str(&criterion.name);
                text.push_str(": ");
                text.push_str(&value);
                if let Some(note) = &cell.note {
                    text.push_str(" (");
                    text.push_str(note);
                    text.push(')');
                }
            }
            Some((subject.name.clone(), text))
        }
        WorkArtifactDataV1::Findings { subjects, items } => {
            let subject = subjects.get(index)?;
            let mut text = subject.descriptor.clone().unwrap_or_default();
            for item in items
                .iter()
                .filter(|item| item.subject == Some(index as u16))
            {
                text.push('\n');
                text.push_str(&item.claim);
            }
            Some((subject.name.clone(), text))
        }
        WorkArtifactDataV1::EvidenceCollection { subjects, .. } => {
            let subject = subjects.get(index)?;
            Some((
                subject.name.clone(),
                subject.descriptor.clone().unwrap_or_default(),
            ))
        }
        _ => None,
    }
}

/// One cited source entry: its title and role. The URL stays with the
/// provider record; a card shows it, context does not need it.
pub fn source_body(data: &artifact::WorkArtifactDataV1, index: u16) -> Option<(String, String)> {
    let artifact::WorkArtifactDataV1::EvidenceCollection { entries, .. } = data else {
        return None;
    };
    let entry = entries.get(usize::from(index))?;
    Some((entry.title.clone(), entry.role.clone()))
}

pub fn finding_body(data: &artifact::WorkArtifactDataV1, index: u16) -> Option<(String, String)> {
    let artifact::WorkArtifactDataV1::Findings { subjects, items } = data else {
        return None;
    };
    let item = items.get(usize::from(index))?;
    let mut text = item.claim.clone();
    if let Some(detail) = &item.detail {
        text.push('\n');
        text.push_str(detail);
    }
    let title = item
        .subject
        .and_then(|subject| subjects.get(usize::from(subject)))
        .map(|subject| format!("{}: {}", subject.name, item.claim))
        .unwrap_or_else(|| item.claim.clone());
    Some((title, text))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(
        element: u128,
        revision: &str,
        text: &str,
        visibility: WorkContextVisibility,
    ) -> WorkContextSource {
        WorkContextSource {
            element: WorkElementId::from(element),
            kind: WorkContextItemKind::Note,
            title: "Note".into(),
            revision: revision.into(),
            visibility,
            text: text.into(),
        }
    }
    fn selection(items: &[(u128, &str)]) -> WorkContextSelectionV1 {
        WorkContextSelectionV1 {
            environment: WorkEnvironmentId::from(9),
            items: items
                .iter()
                .map(|(element, revision)| WorkContextSelectionItem {
                    element: WorkElementId::from(*element),
                    revision: (*revision).into(),
                })
                .collect(),
            tabs: false,
            blocks: false,
        }
    }

    #[test]
    fn admission_digests_truncated_bodies_and_binds_the_manifest() {
        let long = "x".repeat(MAX_CONTEXT_ITEM_BYTES + 10);
        let admitted = WorkAdmittedContext::admit(
            WorkEnvironmentId::from(9),
            WorkRevision::INITIAL,
            WorkContextPurpose::Planning,
            &selection(&[(1, "r1"), (2, "r2")]),
            vec![
                source(1, "r1", "  hello  ", WorkContextVisibility::Private),
                source(2, "r2", &long, WorkContextVisibility::Public),
            ],
            vec![],
        )
        .unwrap();
        assert_eq!(admitted.bodies[0].text, "hello");
        assert_eq!(admitted.disclosure.items[0].bytes, 5);
        assert_eq!(
            admitted.disclosure.items[0].digest,
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
        assert!(admitted.disclosure.items[1].truncated);
        assert_eq!(
            admitted.disclosure.items[1].bytes as usize,
            MAX_CONTEXT_ITEM_BYTES
        );
        assert_eq!(
            admitted.disclosure.total_bytes as usize,
            MAX_CONTEXT_ITEM_BYTES + 5
        );
        assert!(admitted.disclosure.requires_review());
        assert_eq!(admitted.disclosure.validate(), Ok(()));
    }

    #[test]
    fn admission_refuses_stale_private_and_oversized_selections() {
        let stale = WorkAdmittedContext::admit(
            WorkEnvironmentId::from(9),
            WorkRevision::INITIAL,
            WorkContextPurpose::Planning,
            &selection(&[(1, "seen")]),
            vec![source(1, "now", "body", WorkContextVisibility::Private)],
            vec![],
        );
        assert!(matches!(stale, Err(WorkError::Conflict)));
        let private = WorkAdmittedContext::admit(
            WorkEnvironmentId::from(9),
            WorkRevision::INITIAL,
            WorkContextPurpose::PublicRead,
            &selection(&[(1, "r")]),
            vec![source(1, "r", "body", WorkContextVisibility::Private)],
            vec![],
        );
        assert!(matches!(private, Err(WorkError::ReviewRequired)));
        let public = WorkAdmittedContext::admit(
            WorkEnvironmentId::from(9),
            WorkRevision::INITIAL,
            WorkContextPurpose::PublicRead,
            &selection(&[(1, "r")]),
            vec![source(1, "r", "body", WorkContextVisibility::Public)],
            vec![],
        );
        assert!(public.is_ok());
        let items: Vec<(u128, &str)> = (1..=4).map(|i| (i, "r")).collect();
        let sources = (1..=4)
            .map(|i| {
                source(
                    i,
                    "r",
                    &"y".repeat(MAX_CONTEXT_ITEM_BYTES),
                    WorkContextVisibility::Public,
                )
            })
            .collect();
        let oversized = WorkAdmittedContext::admit(
            WorkEnvironmentId::from(9),
            WorkRevision::INITIAL,
            WorkContextPurpose::Planning,
            &selection(&items),
            sources,
            vec![],
        );
        assert!(matches!(oversized, Err(WorkError::Capacity)));
        assert!(selection(&[]).validate().is_err());
        assert!(selection(&[(1, "a"), (1, "b")]).validate().is_err());
    }

    #[test]
    fn a_long_url_is_a_bounded_selection_token() {
        let url = format!("https://outlook.live.com/mail/0/inbox/id/{}", "A".repeat(300));
        assert!(selection(&[(1, url.as_str())]).validate().is_err());
        let revision = url_revision(&url);
        assert_eq!(revision.len(), 64);
        assert!(revision.bytes().all(|b| b.is_ascii_hexdigit()));
        assert_eq!(selection(&[(1, revision.as_str())]).validate(), Ok(()));
        assert_ne!(url_revision(&format!("{url}B")), revision);
        // frame/src/domain/work-environment/tests/context.svelte.test.ts pins the same digest.
        assert_eq!(
            url_revision("https://outlook.live.com/mail/0/inbox/id/AQQkADAwATM0MDAAMS1hYmQ5"),
            "0694dabf6871fb107ac842abba5f084b2a54d906ca6e854fcb80af0aa5702ea0"
        );
    }
}

#[cfg(test)]
mod implicit_tests {
    use super::*;

    #[test]
    fn decisions_join_the_manifest_as_implicit_items_and_keep_selected_identity() {
        let element = WorkElementId::from(5);
        let selection = WorkContextSelectionV1 {
            environment: WorkEnvironmentId::from(1),
            items: vec![WorkContextSelectionItem {
                element,
                revision: "art-1".into(),
            }],
            tabs: false,
            blocks: false,
        };
        let source = |kind, revision: &str, text: &str| WorkContextSource {
            element,
            kind,
            title: "Keychron K2".into(),
            revision: revision.into(),
            visibility: WorkContextVisibility::Public,
            text: text.into(),
        };
        let admitted = WorkAdmittedContext::admit(
            WorkEnvironmentId::from(1),
            WorkRevision::INITIAL,
            WorkContextPurpose::Planning,
            &selection,
            vec![source(WorkContextItemKind::Subject, "art-1", "75% layout")],
            vec![WorkContextSource {
                visibility: WorkContextVisibility::Private,
                ..source(WorkContextItemKind::Decision, "", "Buy this one")
            }],
        )
        .unwrap();
        assert_eq!(admitted.disclosure.items.len(), 2);
        assert!(!admitted.disclosure.items[0].implicit);
        assert!(admitted.disclosure.items[1].implicit);
        assert_eq!(
            admitted.disclosure.items[1].kind,
            WorkContextItemKind::Decision
        );
        assert_eq!(admitted.bodies[1].text, "Buy this one");
        assert!(admitted.disclosure.requires_review());
        let json = serde_json::to_string(&admitted.disclosure).unwrap();
        assert_eq!(json.matches("\"implicit\":true").count(), 1);
        let restored: WorkContextDisclosureV1 = serde_json::from_str(&json).unwrap();
        assert_eq!(restored, admitted.disclosure);
    }
}
