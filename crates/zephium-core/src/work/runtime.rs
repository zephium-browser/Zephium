//! Durable execution facts and a closed compilation vocabulary. None of these
//! serializable values is a policy manifest, context lease or live worker token.
use super::{artifact::*, *};
use crate::ids::ItemId;
#[path = "local.rs"]
mod local;
pub use local::*;

pub const MAX_WORK_EXECUTIONS: usize = 16;
pub const MAX_WORK_ATTEMPTS: usize = 128;
pub const MAX_WORK_COMMANDS: usize = 256;
pub const MAX_WORK_STEPS: usize = 48;
/// A lead run's steps: its parts' pages and searches share one run.
pub const MAX_LEAD_STEPS: usize = 255;
/// A lead run's objects, their revisions and its pages' records.
pub const MAX_LEAD_ARTIFACTS: usize = 128;
pub const MAX_WORK_STEP_NOTE_BYTES: usize = 512;
/// The note of a step that was running when the app closed.
pub const WORK_STEP_INTERRUPTED: &str = "Zephium closed during this step";
pub const MAX_WORK_FOLLOWUPS: usize = 3;
pub const MAX_WORK_FOLLOWUP_BYTES: usize = 120;
/// A work's name: at most five words in one short line.
pub const MAX_WORK_TITLE_CHARS: usize = 48;
pub const MAX_WORK_TITLE_WORDS: usize = 5;

/// A work's name as a run gives it: one line of one to five words.
pub fn validate_work_title(value: &str) -> Result<(), WorkError> {
    validate_text(value, MAX_WORK_TITLE_CHARS * 4)?;
    let words = value.split_whitespace().count();
    if value.trim() != value
        || value.chars().count() > MAX_WORK_TITLE_CHARS
        || value.chars().any(char::is_control)
        || !(1..=MAX_WORK_TITLE_WORDS).contains(&words)
    {
        return Err(WorkError::Invalid);
    }
    Ok(())
}
pub const MAX_WORK_FOLDERS: usize = 8;
pub const MAX_WORK_FILE_PATH_BYTES: usize = 1024;
/// Text disclosed from one file step: a file excerpt, a listing, hits or a diff.
pub const MAX_WORK_FILE_TEXT_BYTES: usize = 16 * 1024;
/// Content the agent may propose for one file write or edit.
pub const MAX_WORK_FILE_CONTENT_BYTES: usize = 64 * 1024;
pub const MAX_WORK_FILE_QUERY_BYTES: usize = 256;

/// An absolute path without control bytes; the application resolves it.
pub fn validate_file_path(path: &str) -> Result<(), WorkError> {
    if path.is_empty()
        || path.len() > MAX_WORK_FILE_PATH_BYTES
        || !((path.starts_with('/') && !path.starts_with("//")) || windows_file_path(path))
        || path.chars().any(char::is_control)
    {
        return Err(WorkError::Invalid);
    }
    Ok(())
}

fn windows_file_path(path: &str) -> bool {
    let path = path.replace('/', r"\");
    let path = path.as_str();
    let extended = path.strip_prefix(r"\\?\");
    let path = extended.unwrap_or(path);
    let tail = if let Some(unc) = extended.and_then(|path| path.strip_prefix(r"UNC\")) {
        unc
    } else if let Some(unc) = path.strip_prefix(r"\\") {
        unc
    } else {
        let bytes = path.as_bytes();
        if bytes.len() < 3
            || !bytes[0].is_ascii_alphabetic()
            || bytes[1] != b':'
            || !matches!(bytes[2], b'/' | b'\\')
        {
            return false;
        }
        return windows_file_components(&path[3..]);
    };
    let mut parts = tail.split(['/', '\\']);
    let (Some(server), Some(share)) = (parts.next(), parts.next()) else {
        return false;
    };
    !server.is_empty() && !share.is_empty() && windows_file_components(tail)
}

#[cfg(test)]
mod file_path_tests {
    use super::*;

    #[test]
    fn native_paths_are_absolute_and_do_not_address_devices_or_streams() {
        for path in [
            "/Users/person/Documents/report.txt",
            r"C:\Users\person\Documents\report.txt",
            "C:/Users/person/Documents/report.txt",
            r"\\?\C:\Users\person\Documents\report.txt",
            r"\\server\share\Documents\report.txt",
            r"\\?\UNC\server\share\Documents\report.txt",
        ] {
            assert_eq!(validate_file_path(path), Ok(()), "{path}");
        }
        for path in [
            "relative/report.txt",
            r"C:Documents\report.txt",
            r"\Documents\report.txt",
            r"\\server",
            r"UNC\server\share\report.txt",
            r"\\.\PhysicalDrive0",
            r"\\?\GLOBALROOT\Device\HarddiskVolume1\report.txt",
            r"\\?\Volume{00000000-0000-0000-0000-000000000000}\report.txt",
            r"C:\Documents\report.txt:secret",
            r"C:\Documents\NUL.txt",
            r"C:\Documents\..\report.txt",
            r"C:\Documents\report.txt.",
            "C:\\Documents\\report.txt\n",
        ] {
            assert_eq!(validate_file_path(path), Err(WorkError::Invalid), "{path}");
        }
    }
}

fn windows_file_components(path: &str) -> bool {
    path.split(['/', '\\']).all(|part| {
        !part.contains([':', '*', '?', '"', '<', '>', '|'])
            && !matches!(part, "." | "..")
            && !part.ends_with(['.', ' '])
            && !matches!(
                part.split('.')
                    .next()
                    .unwrap_or("")
                    .trim_end_matches(' ')
                    .to_ascii_uppercase()
                    .as_str(),
                "CON"
                    | "PRN"
                    | "AUX"
                    | "NUL"
                    | "CONIN$"
                    | "CONOUT$"
                    | "COM1"
                    | "COM2"
                    | "COM3"
                    | "COM4"
                    | "COM5"
                    | "COM6"
                    | "COM7"
                    | "COM8"
                    | "COM9"
                    | "COM¹"
                    | "COM²"
                    | "COM³"
                    | "LPT1"
                    | "LPT2"
                    | "LPT3"
                    | "LPT4"
                    | "LPT5"
                    | "LPT6"
                    | "LPT7"
                    | "LPT8"
                    | "LPT9"
                    | "LPT¹"
                    | "LPT²"
                    | "LPT³"
            )
    })
}

#[cfg_attr(feature = "ipc-types", derive(specta::Type))]
#[derive(Clone, Copy, Debug, Serialize, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct WorkExecutionLimits {
    pub model_tokens: u32,
    pub cost_micro_usd: u32,
    pub operations: u32,
    pub timeout_seconds: u32,
    pub max_workers: u8,
}
impl WorkExecutionLimits {
    pub fn validate(&self) -> Result<(), WorkError> {
        if self.model_tokens == 0
            || self.model_tokens > 1_000_000
            || self.cost_micro_usd == 0
            || self.cost_micro_usd > 10_000_000
            || self.operations == 0
            || self.operations > 1024
            || self.timeout_seconds == 0
            || self.timeout_seconds > 3600
            || self.max_workers == 0
            || self.max_workers > 4
        {
            return Err(WorkError::Invalid);
        }
        Ok(())
    }
}

#[cfg_attr(feature = "ipc-types", derive(specta::Type))]
#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct WorkBrowseScope {
    pub start_url: String,
    /// Exact HTTPS origins and path prefixes, interpreted by the browser
    /// discovery compiler. Redirects do not implicitly extend this set.
    pub routes: Vec<WorkBrowseRoute>,
    pub max_hops: u8,
}
#[cfg_attr(feature = "ipc-types", derive(specta::Type))]
#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct WorkBrowseRoute {
    pub origin: String,
    pub path_prefix: String,
}

#[cfg_attr(feature = "ipc-types", derive(specta::Type))]
#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct WorkPublicDiscoveryScope {
    /// Exact initial public search disclosure reviewed before execution.
    pub search_query: String,
    pub max_hops: u8,
}
pub const WORK_PUBLIC_DISCOVERY_MAX_HOPS: u8 = 16;
pub const WORK_PUBLIC_DISCOVERY_MAX_QUERY_CHARS: usize = 512;
pub const WORK_PUBLIC_DISCOVERY_MAX_QUERY_BYTES: usize = 2048;
impl WorkPublicDiscoveryScope {
    pub fn validate(&self) -> Result<(), WorkError> {
        validate_text(&self.search_query, WORK_PUBLIC_DISCOVERY_MAX_QUERY_BYTES)?;
        if self.search_query.chars().count() > WORK_PUBLIC_DISCOVERY_MAX_QUERY_CHARS {
            return Err(WorkError::Invalid);
        }
        if self.max_hops == 0 || self.max_hops > 32 {
            return Err(WorkError::Invalid);
        }
        Ok(())
    }
    pub fn start_url(&self) -> Result<url::Url, WorkError> {
        self.validate()?;
        let mut url =
            url::Url::parse("https://www.bing.com/search").map_err(|_| WorkError::Invalid)?;
        url.query_pairs_mut().append_pair("q", &self.search_query);
        Ok(url)
    }
}

#[cfg_attr(feature = "ipc-types", derive(specta::Type))]
#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum WorkCapability {
    /// One public provider search, without browser state or attached context.
    PublicSearch {
        scope: super::search::WorkPublicSearchScope,
    },
    /// Direct children can search with this explicit model, browse anonymously,
    /// or synthesize. Each child's exact query remains separately approved.
    CoordinatePublicResearch {
        provider: super::search::WorkSearchProvider,
        model: String,
        max_hops: u8,
    },
    /// Anonymous read-only discovery in a fresh per-resource cookie store.
    PublicDiscovery {
        scope: WorkPublicDiscoveryScope,
    },
    /// Direct-child scheduling and compact synthesis within public discovery.
    CoordinatePublicDiscovery {
        max_hops: u8,
    },
    PublicBrowse {
        scope: WorkBrowseScope,
    },
    /// A primary agent may assign pre-approved children within this envelope
    /// and synthesize their results. This is not a direct browser/action port.
    Coordinate {
        scope: WorkBrowseScope,
    },
    /// Structured handoffs from completed plan dependencies only.
    Synthesize,
    /// Read one exact page with the profile's own signed-in session. The
    /// approving user attests the account; Zephium cannot verify it.
    AccountRead {
        scope: WorkAccountScope,
    },
    /// One approved field transition on that page, then its restoration,
    /// each verified from a fresh observation before the next step.
    AccountUpdate {
        scope: WorkAccountScope,
        update: WorkFieldUpdateV1,
    },
    /// Routine work under one grant: the agent chooses searches, reads,
    /// discoveries and published objects turn by turn. Read-only; reads are
    /// anonymous except inside origins the person approved in `accounts`, and
    /// effects need their own approval.
    Agent {
        grant: WorkAgentGrantV1,
    },
}

#[cfg_attr(feature = "ipc-types", derive(specta::Type))]
#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct WorkAgentGrantV1 {
    pub provider: super::search::WorkSearchProvider,
    pub model: String,
    pub max_turns: u8,
    pub max_steps: u8,
    pub browse_hops: u8,
    /// Folders the person granted for this run, as absolute paths. Every file
    /// step must resolve inside one of them; the application enforces it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub folders: Vec<String>,
    /// Retired origin grants: accepted in stored runs, refused for new ones.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub accounts: Vec<WorkAccountGrantV1>,
    /// A private run: every page opens in the run's own empty storage and
    /// never in the person's sessions.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub private: bool,
    /// The lead agent's model: this run is a lead run with tools, parts and
    /// the current object kinds. Absent for the earlier runtime.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lead: Option<super::model::WorkModelRef>,
    /// The workflow the person started the run from: the lead begins with
    /// this skill loaded. A name it does not know is ignored.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skill: Option<String>,
}
impl WorkAgentGrantV1 {
    pub fn validate(&self) -> Result<(), WorkError> {
        let mut origins = BTreeSet::new();
        if self.accounts.len() > MAX_WORK_ACCOUNT_GRANTS
            || self
                .accounts
                .iter()
                .any(|grant| grant.validate().is_err() || !origins.insert(&grant.origin))
        {
            return Err(WorkError::Invalid);
        }
        if self.folders.len() > MAX_WORK_FOLDERS
            || self
                .folders
                .iter()
                .any(|folder| validate_file_path(folder).is_err())
        {
            return Err(WorkError::Invalid);
        }
        if let Some(skill) = &self.skill {
            if skill.is_empty()
                || skill.len() > 64
                || !skill
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
            {
                return Err(WorkError::Invalid);
            }
        }
        if let Some(lead) = &self.lead {
            validate_text(&lead.model, 128)?;
            if lead.model.chars().any(char::is_whitespace) {
                return Err(WorkError::Invalid);
            }
        }
        let max_turns = if self.lead.is_some() { u8::MAX } else { 16 };
        if !super::search::supported_public_search_model(&self.model)
            || self.max_turns == 0
            || self.max_turns > max_turns
            || self.max_steps == 0
            || usize::from(self.max_steps) > self.step_limit()
            || self.browse_hops == 0
            || self.browse_hops > 8
        {
            return Err(WorkError::Invalid);
        }
        Ok(())
    }
    pub fn step_limit(&self) -> usize {
        if self.lead.is_some() {
            MAX_LEAD_STEPS
        } else {
            MAX_WORK_STEPS
        }
    }
    pub fn artifact_limit(&self) -> usize {
        if self.lead.is_some() {
            MAX_LEAD_ARTIFACTS
        } else {
            MAX_WORK_ARTIFACTS
        }
    }
    /// The approved origin a page URL lies inside, if any.
    pub fn account_for(&self, url: &str) -> Option<&WorkAccountGrantV1> {
        self.accounts.iter().find(|grant| grant.admits(url))
    }
}

pub const MAX_WORK_ACCOUNT_GRANTS: usize = 4;
pub const MAX_WORK_ACCOUNT_PAGES: u8 = 12;
/// A page task's goal, as the agent states it for one site.
pub const MAX_WORK_PAGE_GOAL_BYTES: usize = 600;
/// One origin read with the profile's signed-in session for one request.
/// The person's approval attests the account; Zephium never infers it.
#[cfg_attr(feature = "ipc-types", derive(specta::Type))]
#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct WorkAccountGrantV1 {
    /// Exact HTTPS origin.
    pub origin: String,
    /// Opaque identity minted by Rust when the approval was drafted.
    pub account: String,
    /// The attached tab the person approved it from, when there was one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tab: Option<ItemId>,
    /// Signed-in pages the request may open on this origin.
    pub pages: u8,
}
impl WorkAccountGrantV1 {
    pub fn validate(&self) -> Result<(), WorkError> {
        let url = validate_public_url(&self.origin)?;
        if url.origin().ascii_serialization() != self.origin
            || !valid_account(&self.account)
            || self.pages == 0
            || self.pages > MAX_WORK_ACCOUNT_PAGES
        {
            return Err(WorkError::Invalid);
        }
        Ok(())
    }
    /// Whether `url` is an HTTPS page on exactly this origin.
    pub fn admits(&self, url: &str) -> bool {
        validate_public_url(url).is_ok_and(|url| url.origin().ascii_serialization() == self.origin)
    }
    pub fn host(&self) -> &str {
        self.origin.strip_prefix("https://").unwrap_or(&self.origin)
    }
}
fn valid_account(account: &str) -> bool {
    !account.is_empty() && account.len() <= 64 && account.bytes().all(|b| b.is_ascii_alphanumeric())
}

/// Closed shape of a page path, for logs that must not carry the path.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WorkPathClass {
    Root,
    Page,
    Query,
    /// Names a state change (sign out, delete, send, pay and the like).
    Action,
}
const ACTION_WORDS: [&str; 19] = [
    "logout",
    "log-out",
    "signout",
    "sign-out",
    "delete",
    "remove",
    "destroy",
    "unsubscribe",
    "send",
    "submit",
    "pay",
    "checkout",
    "purchase",
    "transfer",
    "approve",
    "confirm",
    "revoke",
    "disconnect",
    "deactivate",
];
pub fn path_class(url: &str) -> WorkPathClass {
    let Ok(url) = url::Url::parse(url) else {
        return WorkPathClass::Action;
    };
    let action = |word: &str| {
        let word = word.to_ascii_lowercase();
        ACTION_WORDS.iter().any(|action| {
            word == *action
                || word
                    .split(|c: char| !c.is_ascii_alphanumeric())
                    .any(|part| part == *action)
        })
    };
    if url
        .path_segments()
        .is_some_and(|mut parts| parts.any(action))
        || url
            .query_pairs()
            .any(|(key, value)| action(&key) || action(&value))
    {
        WorkPathClass::Action
    } else if url.query().is_some() {
        WorkPathClass::Query
    } else if url.path() == "/" {
        WorkPathClass::Root
    } else {
        WorkPathClass::Page
    }
}

/// A page item read under a grant: the canvas draws the account badge.
#[cfg_attr(feature = "ipc-types", derive(specta::Type))]
#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct WorkPageAccountV1 {
    pub host: String,
    pub badge: bool,
}
/// One granted origin as the request uses it: "Using your session on host".
#[cfg_attr(feature = "ipc-types", derive(specta::Type))]
#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct WorkAccountUseV1 {
    pub host: String,
    pub pages_used: u8,
    pub pages: u8,
}

/// A page the user chose from an attached tab. Execution opens it in a
/// Work-owned page sharing the profile's cookies; the tab itself is untouched.
#[cfg_attr(feature = "ipc-types", derive(specta::Type))]
#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct WorkAccountScope {
    pub tab: ItemId,
    pub url: String,
    pub origin: String,
    /// Opaque account identity minted by Rust at preparation; the approval
    /// binds it to this profile and origin.
    pub account: String,
}
pub const MAX_WORK_FIELD_VALUE_BYTES: usize = 512;
#[cfg_attr(feature = "ipc-types", derive(specta::Type))]
#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct WorkFieldUpdateV1 {
    /// Accessible field name when the page has several candidates.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub field: Option<String>,
    pub from: String,
    pub to: String,
}
impl WorkAccountScope {
    pub fn validate(&self) -> Result<(), WorkError> {
        let url = validate_public_url(&self.url)?;
        if url.origin().ascii_serialization() != self.origin || !valid_account(&self.account) {
            return Err(WorkError::Invalid);
        }
        Ok(())
    }
}
impl WorkFieldUpdateV1 {
    pub fn validate(&self) -> Result<(), WorkError> {
        for value in [&self.from, &self.to] {
            validate_text(value, MAX_WORK_FIELD_VALUE_BYTES)?;
            if value.trim().is_empty() || value.chars().any(char::is_control) {
                return Err(WorkError::Invalid);
            }
        }
        if let Some(field) = &self.field {
            validate_text(field, 128)?;
            if field.trim().is_empty() || field.chars().any(char::is_control) {
                return Err(WorkError::Invalid);
            }
        }
        if self.from == self.to {
            return Err(WorkError::Invalid);
        }
        Ok(())
    }
}

#[cfg_attr(feature = "ipc-types", derive(specta::Type))]
#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct WorkNodeExecutionSpec {
    pub node: WorkPlanNodeId,
    /// A direct delegation edge, independent from data dependencies. Its
    /// authority must be contained in its parent's approved capability.
    pub parent: Option<WorkPlanNodeId>,
    pub capability: WorkCapability,
    pub limits: WorkExecutionLimits,
}
#[cfg_attr(feature = "ipc-types", derive(specta::Type))]
#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct WorkExecutionSpec {
    pub plan_revision: WorkRevision,
    pub limits: WorkExecutionLimits,
    pub nodes: Vec<WorkNodeExecutionSpec>,
    /// Admitted canvas context disclosed to this execution's provider calls.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<super::context::WorkContextDisclosureV1>,
    /// The person's request this execution serves: the work's objective when
    /// it began. Earlier executions of a work keep theirs, forming the thread.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request: Option<String>,
}
impl WorkExecutionSpec {
    /// First product adapter: explicit public browsing scope, optionally with
    /// one synthesizing primary. This only prepares an approval draft; it grants
    /// nothing and never increases the supplied aggregate budget.
    pub fn public_research(
        plan: &WorkPlanRevision,
        limits: WorkExecutionLimits,
        scope: WorkBrowseScope,
        primary: Option<WorkPlanNodeId>,
    ) -> Result<Self, WorkError> {
        plan.draft.validate()?;
        limits.validate()?;
        if primary.is_some_and(|id| !plan.draft.nodes.iter().any(|n| n.id == id))
            || (primary.is_some() && plan.draft.nodes.len() > 1 && limits.max_workers < 2)
            || plan
                .draft
                .nodes
                .iter()
                .flat_map(|n| &n.outputs)
                .any(|o| o.review == WorkOutputReview::Mechanical)
        {
            return Err(WorkError::Invalid);
        }
        let count = plan.draft.nodes.len() as u32;
        let spec = Self {
            context: None,
            request: None,
            plan_revision: plan.revision,
            limits,
            nodes: plan
                .draft
                .nodes
                .iter()
                .map(|node| WorkNodeExecutionSpec {
                    node: node.id,
                    parent: primary.filter(|id| *id != node.id),
                    capability: if primary == Some(node.id) {
                        WorkCapability::Coordinate {
                            scope: scope.clone(),
                        }
                    } else {
                        WorkCapability::PublicBrowse {
                            scope: scope.clone(),
                        }
                    },
                    limits: WorkExecutionLimits {
                        model_tokens: limits.model_tokens / count,
                        cost_micro_usd: limits.cost_micro_usd / count,
                        operations: limits.operations / count,
                        max_workers: if primary == Some(node.id) {
                            limits.max_workers
                        } else {
                            1
                        },
                        ..limits
                    },
                })
                .collect(),
        };
        spec.validate(plan)?;
        Ok(spec)
    }
    /// One signed-in page for a single-step plan. Approval of this draft is
    /// the user's attestation of the account; it grants only the named effect.
    pub fn account_scoped(
        plan: &WorkPlanRevision,
        limits: WorkExecutionLimits,
        capability: WorkCapability,
    ) -> Result<Self, WorkError> {
        plan.draft.validate()?;
        limits.validate()?;
        capability.validate()?;
        let [node] = plan.draft.nodes.as_slice() else {
            return Err(WorkError::Invalid);
        };
        if !matches!(
            capability,
            WorkCapability::AccountRead { .. } | WorkCapability::AccountUpdate { .. }
        ) || node
            .outputs
            .iter()
            .any(|o| o.review == WorkOutputReview::Mechanical)
        {
            return Err(WorkError::Invalid);
        }
        let spec = Self {
            context: None,
            request: None,
            plan_revision: plan.revision,
            limits,
            nodes: vec![WorkNodeExecutionSpec {
                node: node.id,
                parent: None,
                capability,
                limits: WorkExecutionLimits {
                    max_workers: 1,
                    ..limits
                },
            }],
        };
        spec.validate(plan)?;
        Ok(spec)
    }
    /// The routine envelope for a single-step plan. Sending the objective is
    /// the grant; nothing here reaches an account or a consequential effect.
    pub fn agent(
        plan: &WorkPlanRevision,
        limits: WorkExecutionLimits,
        grant: WorkAgentGrantV1,
    ) -> Result<Self, WorkError> {
        plan.draft.validate()?;
        limits.validate()?;
        grant.validate()?;
        let [node] = plan.draft.nodes.as_slice() else {
            return Err(WorkError::Invalid);
        };
        if node
            .outputs
            .iter()
            .any(|o| o.review == WorkOutputReview::Mechanical)
        {
            return Err(WorkError::Invalid);
        }
        let spec = Self {
            context: None,
            request: Some(node.objective.clone()),
            plan_revision: plan.revision,
            limits,
            nodes: vec![WorkNodeExecutionSpec {
                node: node.id,
                parent: None,
                capability: WorkCapability::Agent { grant },
                limits,
            }],
        };
        spec.validate(plan)?;
        Ok(spec)
    }
    pub fn validate_bounds(&self) -> Result<(), WorkError> {
        self.limits.validate()?;
        if self.nodes.is_empty() || self.nodes.len() > MAX_WORK_NODES {
            return Err(WorkError::Invalid);
        }
        for node in &self.nodes {
            node.limits.validate()?;
            node.capability.validate()?;
        }
        Ok(())
    }
    pub fn validate(&self, plan: &WorkPlanRevision) -> Result<(), WorkError> {
        plan.draft.validate()?;
        self.validate_bounds()?;
        self.limits.validate()?;
        if self.plan_revision != plan.revision || self.nodes.len() != plan.draft.nodes.len() {
            return Err(WorkError::Invalid);
        }
        let mut ids = BTreeSet::new();
        let mut tokens = 0_u64;
        let mut cost = 0_u64;
        let mut operations = 0_u64;
        for node in &self.nodes {
            node.limits.validate()?;
            node.capability.validate()?;
            if !ids.insert(node.node)
                || !plan.draft.nodes.iter().any(|n| n.id == node.node)
                || node.limits.timeout_seconds > self.limits.timeout_seconds
                || node.limits.max_workers > self.limits.max_workers
            {
                return Err(WorkError::Invalid);
            }
            tokens += u64::from(node.limits.model_tokens);
            cost += u64::from(node.limits.cost_micro_usd);
            operations += u64::from(node.limits.operations);
            if let Some(parent) = node.parent {
                let parent = self
                    .nodes
                    .iter()
                    .find(|n| n.node == parent)
                    .ok_or(WorkError::Invalid)?;
                if parent.node == node.node
                    || !parent.capability.is_coordinator()
                    || !node.capability.is_subset_of(&parent.capability)
                    || node.limits.model_tokens > parent.limits.model_tokens
                    || node.limits.cost_micro_usd > parent.limits.cost_micro_usd
                    || node.limits.operations > parent.limits.operations
                    || node.limits.timeout_seconds > parent.limits.timeout_seconds
                    || node.limits.max_workers > parent.limits.max_workers
                {
                    return Err(WorkError::Invalid);
                }
            }
        }
        if tokens > u64::from(self.limits.model_tokens)
            || cost > u64::from(self.limits.cost_micro_usd)
            || operations > u64::from(self.limits.operations)
        {
            return Err(WorkError::Capacity);
        }
        for node in &self.nodes {
            let mut parent = node.parent;
            for _ in 0..self.nodes.len() {
                match parent {
                    None => break,
                    Some(id) => {
                        parent = self
                            .nodes
                            .iter()
                            .find(|n| n.node == id)
                            .ok_or(WorkError::Invalid)?
                            .parent
                    }
                }
            }
            if parent.is_some() {
                return Err(WorkError::Invalid);
            }
        }
        // Completion waits for both data dependencies and delegated children.
        // Validate their combined graph, including cross-branch deadlocks that
        // neither independently acyclic graph would reveal.
        let mut complete = BTreeSet::new();
        loop {
            let before = complete.len();
            for node in &plan.draft.nodes {
                if node.dependencies.iter().all(|id| complete.contains(id))
                    && self
                        .nodes
                        .iter()
                        .filter(|entry| entry.parent == Some(node.id))
                        .all(|child| complete.contains(&child.node))
                {
                    complete.insert(node.id);
                }
            }
            if complete.len() == self.nodes.len() {
                return Ok(());
            }
            if before == complete.len() {
                return Err(WorkError::Invalid);
            }
        }
    }
}
impl WorkCapability {
    pub fn is_coordinator(&self) -> bool {
        matches!(
            self,
            Self::Coordinate { .. }
                | Self::CoordinatePublicDiscovery { .. }
                | Self::CoordinatePublicResearch { .. }
        )
    }
    pub fn validate(&self) -> Result<(), WorkError> {
        match self {
            Self::PublicSearch { scope } => scope.validate()?,
            Self::CoordinatePublicResearch {
                model, max_hops, ..
            } if !super::search::supported_public_search_model(model)
                || *max_hops == 0
                || *max_hops > 32 =>
            {
                return Err(WorkError::Invalid);
            }
            Self::PublicDiscovery { scope } => scope.validate()?,
            Self::CoordinatePublicDiscovery { max_hops } if *max_hops == 0 || *max_hops > 32 => {
                return Err(WorkError::Invalid);
            }
            Self::AccountRead { scope } => scope.validate()?,
            Self::AccountUpdate { scope, update } => {
                scope.validate()?;
                update.validate()?;
            }
            Self::Agent { grant } => grant.validate()?,
            _ => {}
        }
        if let Self::PublicBrowse { scope } | Self::Coordinate { scope } = self {
            let start = validate_public_url(&scope.start_url)?;
            if scope.routes.is_empty()
                || scope.routes.len() > 8
                || scope.max_hops == 0
                || scope.max_hops > 32
            {
                return Err(WorkError::Invalid);
            }
            let mut unique = BTreeSet::new();
            for route in &scope.routes {
                let origin = validate_public_url(&route.origin)?;
                if origin.origin().ascii_serialization() != route.origin
                    || !route.path_prefix.starts_with('/')
                    || !route.path_prefix.ends_with('/')
                    || route.path_prefix.len() > 1024
                    || route.path_prefix.contains(['?', '#', '\\', '%'])
                    || route.path_prefix.split('/').any(|p| p == ".." || p == ".")
                    || route.path_prefix.chars().any(char::is_control)
                    || !unique.insert((&route.origin, &route.path_prefix))
                {
                    return Err(WorkError::Invalid);
                }
            }
            if !scope.routes.iter().any(|route| {
                route.origin == start.origin().ascii_serialization()
                    && path_within(start.path(), &route.path_prefix)
            }) {
                return Err(WorkError::Invalid);
            }
        }
        Ok(())
    }
    pub fn is_subset_of(&self, parent: &Self) -> bool {
        match (self, parent) {
            (
                Self::Synthesize,
                Self::Synthesize
                | Self::Coordinate { .. }
                | Self::CoordinatePublicDiscovery { .. }
                | Self::CoordinatePublicResearch { .. },
            ) => true,
            (Self::PublicSearch { scope }, Self::PublicSearch { scope: parent }) => scope == parent,
            (
                Self::PublicSearch { scope },
                Self::CoordinatePublicResearch {
                    provider, model, ..
                },
            ) => scope.provider == *provider && scope.model == *model,
            (Self::PublicDiscovery { scope }, Self::CoordinatePublicResearch { max_hops, .. }) => {
                scope.max_hops <= *max_hops
            }
            (Self::PublicDiscovery { scope }, Self::CoordinatePublicDiscovery { max_hops }) => {
                scope.max_hops <= *max_hops
            }
            (Self::PublicDiscovery { scope }, Self::PublicDiscovery { scope: parent }) => {
                scope.max_hops <= parent.max_hops && scope.search_query == parent.search_query
            }
            (
                Self::PublicBrowse { scope: child },
                Self::PublicBrowse { scope: parent } | Self::Coordinate { scope: parent },
            )
            | (Self::Coordinate { scope: child }, Self::Coordinate { scope: parent }) => {
                child.max_hops <= parent.max_hops
                    && child.routes.iter().all(|route| {
                        parent.routes.iter().any(|p| {
                            p.origin == route.origin
                                && path_within(&route.path_prefix, &p.path_prefix)
                        })
                    })
            }
            _ => false,
        }
    }
}
fn path_within(path: &str, prefix: &str) -> bool {
    path == prefix
        || (path.starts_with(prefix)
            && (prefix.ends_with('/') || path.as_bytes().get(prefix.len()) == Some(&b'/')))
}
pub(crate) fn validate_public_url(value: &str) -> Result<url::Url, WorkError> {
    let url = validate_public_reference_url(value)?;
    if url.fragment().is_some() {
        return Err(WorkError::Invalid);
    }
    Ok(url)
}

/// Closed words for why a page read ended; the loop and the person read them.
pub mod read_note {
    pub const HUMAN_CHECK: &str = "The page asked for a human check";
    pub const UNSETTLED: &str = "The page did not settle while it was being read";
    pub const CONSTRUCTION_TIMEOUT: &str =
        "The page did not become ready within its loading window";
    pub const SLOW_SITE: &str =
        "The site is slowing us down; the page did not become ready after two attempts";
}

pub(crate) fn validate_public_reference_url(value: &str) -> Result<url::Url, WorkError> {
    validate_text(value, 4096)?;
    let url = url::Url::parse(value).map_err(|_| WorkError::Invalid)?;
    if url.scheme() != "https"
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(WorkError::Invalid);
    }
    Ok(url)
}

#[cfg_attr(feature = "ipc-types", derive(specta::Type))]
#[derive(Clone, Copy, Debug, Serialize, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum WorkExecutionStatus {
    Approved,
    Running,
    CancelRequested,
    Completed,
    NeedsReview,
    Cancelled,
    Failed,
    Interrupted,
}
impl WorkExecutionStatus {
    pub fn terminal(self) -> bool {
        matches!(
            self,
            Self::Completed
                | Self::NeedsReview
                | Self::Cancelled
                | Self::Failed
                | Self::Interrupted
        )
    }
}
#[cfg_attr(feature = "ipc-types", derive(specta::Type))]
#[derive(Clone, Copy, Debug, Serialize, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum WorkAttemptStatus {
    Running,
    Succeeded,
    Failed,
    Cancelled,
    OutcomeUnknown,
}
#[cfg_attr(feature = "ipc-types", derive(specta::Type))]
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct WorkUsage {
    pub model_tokens: u32,
    pub cost_micro_usd: u32,
    pub operations: u32,
    pub accounting: WorkUsageAccounting,
}
#[cfg_attr(feature = "ipc-types", derive(specta::Type))]
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum WorkUsageAccounting {
    #[default]
    Exact,
    ConservativeReservation,
}
/// How exactly a measured cost is known, weakest call first.
#[cfg_attr(feature = "ipc-types", derive(specta::Type))]
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, Eq, PartialEq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum WorkCostBasis {
    /// Every settled call reported exact provider cost.
    Exact,
    /// Exact tokens, and at least one cost priced from the trusted catalog.
    Priced,
    /// At least one call charged its reservation ceiling or was in flight.
    #[default]
    Reserved,
}

#[cfg_attr(feature = "ipc-types", derive(specta::Type))]
#[derive(Clone, Copy, Debug, Default, Serialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
/// Closed counts for one browser step: durations, call counts and exact
/// accounting beside the conservative ceiling in `usage`. Never page, model
/// or provider text.
pub struct WorkStepMeasurementsV1 {
    /// Wall time from the step's resource launch to its settled outcome.
    pub wall_millis: u32,
    /// Typed decision calls answered by the recommended backend.
    pub decision_calls: u16,
    /// Typed decision calls answered by the LLM emulation.
    pub emulation_calls: u16,
    /// Page-model calls, including focused generation and extraction.
    pub planner_calls: u16,
    /// Native actions the page agent started.
    pub native_actions: u16,
    /// Tokens charged by settled provider calls.
    pub model_tokens: u32,
    /// Micro-USD charged by settled provider calls.
    pub cost_micro_usd: u32,
    /// How exactly `cost_micro_usd` is known.
    pub cost_basis: WorkCostBasis,
}
/// Steps recorded before `cost_basis` carry `cost_exact`; a false one could
/// have been priced or reserved, so it reads as reserved.
impl<'de> Deserialize<'de> for WorkStepMeasurementsV1 {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            wall_millis: u32,
            decision_calls: u16,
            emulation_calls: u16,
            planner_calls: u16,
            native_actions: u16,
            model_tokens: u32,
            cost_micro_usd: u32,
            #[serde(default)]
            cost_basis: Option<WorkCostBasis>,
            #[serde(default)]
            cost_exact: Option<bool>,
        }
        let wire = Wire::deserialize(deserializer)?;
        Ok(Self {
            wall_millis: wire.wall_millis,
            decision_calls: wire.decision_calls,
            emulation_calls: wire.emulation_calls,
            planner_calls: wire.planner_calls,
            native_actions: wire.native_actions,
            model_tokens: wire.model_tokens,
            cost_micro_usd: wire.cost_micro_usd,
            cost_basis: wire.cost_basis.unwrap_or(if wire.cost_exact == Some(true) {
                WorkCostBasis::Exact
            } else {
                WorkCostBasis::Reserved
            }),
        })
    }
}

impl WorkUsage {
    pub fn within(self, limits: WorkExecutionLimits) -> bool {
        self.model_tokens <= limits.model_tokens
            && self.cost_micro_usd <= limits.cost_micro_usd
            && self.operations <= limits.operations
    }
}

#[cfg_attr(feature = "ipc-types", derive(specta::Type))]
#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct WorkAttemptFact {
    pub id: WorkAttemptId,
    pub node: WorkPlanNodeId,
    pub status: WorkAttemptStatus,
    /// Reservation is retained for unknown outcomes. Settled accounting names
    /// exact usage or a conservative ceiling; missing means unknown, never zero.
    pub usage: Option<WorkUsage>,
}
#[cfg_attr(feature = "ipc-types", derive(specta::Type))]
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum WorkExecutionAuthorization {
    #[default]
    ReviewedPlan,
    UserDirectedPublicRead,
    /// The user sent an objective; the routine public envelope is the grant.
    UserDirectedAgent,
}
#[cfg_attr(feature = "ipc-types", derive(specta::Type))]
#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct WorkExecutionFact {
    #[serde(default)]
    pub authorization: WorkExecutionAuthorization,
    pub id: WorkExecutionId,
    pub approved_revision: WorkRevision,
    pub spec: WorkExecutionSpec,
    pub status: WorkExecutionStatus,
    pub attempts: Vec<WorkAttemptFact>,
    pub artifacts: Vec<WorkArtifactV1>,
    #[serde(default)]
    pub provider_evidence: Vec<WorkProviderSearchRecordV1>,
    /// What file steps disclosed: listings, excerpts, hits and applied diffs.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub file_evidence: Vec<WorkFileRecordV1>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub command_evidence: Vec<WorkCommandRecordV1>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub folder_approvals: Vec<WorkFolderApprovalV1>,
    /// User edits and decisions never overwrite the original agent output.
    #[serde(default)]
    pub user_artifacts: Vec<WorkArtifactUserState>,
    /// Why automation stopped for a person. Continuation is a fresh approval.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub intervention: Option<WorkInterventionV1>,
    /// Admitted agent operations in order, committed as each one settles.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub steps: Vec<WorkStepFact>,
    /// Granted signed-in origins and their page budgets, derived from the
    /// grant and the steps; see `refresh_accounts`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub accounts: Vec<WorkAccountUseV1>,
    /// The parts a lead run split into, in the order they were started.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub parts: Vec<super::parts::WorkPartFactV1>,
    /// What a lead run pulled in: skills, notes, tabs, files.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub inputs: Vec<super::parts::WorkInputFactV1>,
    /// The name its finish gave the work, derived from the finish step.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}

#[cfg_attr(feature = "ipc-types", derive(specta::Type))]
#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum WorkStepKindV1 {
    /// One model turn; `note` on the step is what the agent said.
    Turn,
    Search {
        query: String,
    },
    Read {
        url: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        collection: Option<super::collection::WorkBrowseCollection>,
        /// A page task: the agent works toward this goal on the url's site.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        goal: Option<String>,
    },
    Discover {
        query: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        collection: Option<super::collection::WorkBrowseCollection>,
    },
    /// Objects the agent placed on the canvas from this turn.
    Publish,
    Ask {
        prompt: String,
        options: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        answer: Option<String>,
        /// Why it asks, so the question shows where it belongs.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        purpose: Option<WorkAskPurposeV1>,
    },
    /// A message the person sent while the agent ran; the agent reads it at
    /// its next turn.
    Steer {
        text: String,
    },
    /// Directory listing inside a granted folder.
    List {
        path: String,
        #[serde(default)]
        depth: Option<u8>,
    },
    ReadFile {
        path: String,
        #[serde(default)]
        offset: Option<u32>,
        #[serde(default)]
        limit: Option<u32>,
    },
    SearchFiles {
        path: String,
        query: String,
        #[serde(default)]
        glob: Option<String>,
        #[serde(default)]
        regex: Option<bool>,
    },
    /// A proposed whole-file write; `decision` is the person's answer.
    WriteFile {
        path: String,
        content: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        decision: Option<bool>,
    },
    /// A proposed replacement of one exact passage.
    EditFile {
        path: String,
        old: String,
        new: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        replacements: Vec<WorkFileReplacementV1>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        decision: Option<bool>,
    },
    MoveFile {
        from: String,
        to: String,
        #[serde(default)]
        decision: Option<bool>,
    },
    DeleteFile {
        path: String,
        #[serde(default)]
        decision: Option<bool>,
    },
    RunCommand {
        cwd: String,
        command: String,
        #[serde(default)]
        timeout_secs: Option<u32>,
        #[serde(default)]
        decision: Option<bool>,
    },
    Finish {
        /// Up to three short next requests the person may choose.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        followups: Vec<String>,
        /// The work's name, given by its first run: a short noun phrase.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
    },
    /// A step on a site that commits something for the person, held until
    /// they decide. Every field is Rust's reading of the page, never the
    /// model's words; the status is the receipt once it settles.
    Confirm {
        confirm: Box<WorkConfirmV1>,
    },
    /// A call to an installed tool or connected service, settled as it is
    /// recorded; the note is its row for people.
    Call {
        call: Box<WorkConnectionCallV1>,
    },
}

/// What one connection call touched, in closed fields the frame writes its
/// row from.
#[cfg_attr(feature = "ipc-types", derive(specta::Type))]
#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct WorkConnectionCallV1 {
    /// "github", or an MCP server's id.
    pub service: String,
    /// The namespaced tool: `github_issue`, `notion__search`.
    pub tool: String,
    /// What kind of call it was: `issue`, `issues`, `pr`, `comment`.
    pub verb: String,
    /// What it touched: "#123", "octo/app".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    /// A title it read, such as the issue's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Items it listed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub count: Option<u32>,
    /// The page the call is about, when it has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}
impl WorkConnectionCallV1 {
    fn validate(&self) -> Result<(), WorkError> {
        let word = |value: &str, max: usize| {
            validate_text(value, max)?;
            if value.contains('\n') {
                return Err(WorkError::Invalid);
            }
            Ok(())
        };
        word(&self.service, 64)?;
        word(&self.tool, 128)?;
        word(&self.verb, 32)?;
        if let Some(target) = &self.target {
            word(target, 256)?;
        }
        if let Some(title) = &self.title {
            word(title, 512)?;
        }
        if let Some(url) = &self.url {
            validate_public_reference_url(url)?;
        }
        Ok(())
    }
}

/// Why a step asks the person.
#[cfg_attr(feature = "ipc-types", derive(specta::Type))]
#[derive(Clone, Copy, Debug, Serialize, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum WorkAskPurposeV1 {
    /// The agent's own question about the request.
    Question,
    /// Whether the agent may work in the person's session on a site.
    Entry,
    /// The run spent its budget: keep going or stop.
    Budget,
    /// Whether the agent may read the person's history, notes or tabs.
    Context,
    /// Whether the agent may use an installed tool or connected service.
    Connection,
    /// Any other yes or no before the agent acts.
    Confirm,
    /// Whether the agent may read a folder on this Mac the request named;
    /// the step's local fact carries the folder.
    Folder,
    /// Whether to open a page address the agent wrote itself after reading
    /// the person's own information; the prompt is the address.
    Address,
}

/// What a held step would commit.
#[cfg_attr(feature = "ipc-types", derive(specta::Type))]
#[derive(Clone, Copy, Debug, Serialize, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum WorkConfirmCategoryV1 {
    /// Send, post, reply, share or invite.
    Communication,
    /// Pay, buy, book or order.
    Purchase,
    /// Delete, remove or cancel.
    Destructive,
    /// Save or submit a change.
    Save,
    /// Type into a document that saves as it is typed.
    Edit,
    /// Type into a site the person did not name, in a run holding their data.
    Type,
}

#[cfg_attr(feature = "ipc-types", derive(specta::Type))]
#[derive(Clone, Copy, Debug, Serialize, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum WorkConfirmDecisionV1 {
    Approved,
    /// Approved, and later edits on this site in this run need no question.
    AllowedForRun,
    Declined,
}

#[cfg_attr(feature = "ipc-types", derive(specta::Type))]
#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct WorkConfirmFactV1 {
    pub label: String,
    pub value: String,
}

#[cfg_attr(feature = "ipc-types", derive(specta::Type))]
#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct WorkConfirmV1 {
    pub site: String,
    pub category: WorkConfirmCategoryV1,
    /// "Send to #design as you?", from a fixed template and page text.
    pub headline: String,
    /// "press Send".
    pub action: String,
    /// The exact text the step would send or save, as the page holds it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub facts: Vec<WorkConfirmFactV1>,
    /// The page step whose captured frame shows the page as it stands.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page: Option<WorkStepId>,
    /// Other sites whose page text appears in `text`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub provenance: Vec<String>,
    /// "Allow edits on <site> for this run" is offered.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub run_option: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision: Option<WorkConfirmDecisionV1>,
}

pub const MAX_WORK_CONFIRM_FACTS: usize = 12;
pub const MAX_WORK_CONFIRM_LINE_BYTES: usize = 300;
pub const MAX_WORK_CONFIRM_TEXT_BYTES: usize = 4096;
const MAX_WORK_CONFIRM_SITES: usize = 8;

impl WorkConfirmV1 {
    fn validate(&self) -> Result<(), WorkError> {
        super::sites::validate_site(&self.site)?;
        for line in [&self.headline, &self.action] {
            validate_text(line, MAX_WORK_CONFIRM_LINE_BYTES)?;
            if line.trim().is_empty() {
                return Err(WorkError::Invalid);
            }
        }
        if let Some(text) = &self.text {
            validate_text(text, MAX_WORK_CONFIRM_TEXT_BYTES)?;
        }
        if self.facts.len() > MAX_WORK_CONFIRM_FACTS
            || self.provenance.len() > MAX_WORK_CONFIRM_SITES
            || (self.run_option
                && !matches!(
                    self.category,
                    WorkConfirmCategoryV1::Edit | WorkConfirmCategoryV1::Type
                ))
            || (self.decision == Some(WorkConfirmDecisionV1::AllowedForRun) && !self.run_option)
        {
            return Err(WorkError::Invalid);
        }
        for fact in &self.facts {
            validate_text(&fact.label, MAX_WORK_CONFIRM_LINE_BYTES)?;
            validate_text(&fact.value, MAX_WORK_CONFIRM_LINE_BYTES)?;
        }
        for site in &self.provenance {
            super::sites::validate_site(site)?;
        }
        Ok(())
    }
}
#[cfg_attr(feature = "ipc-types", derive(specta::Type))]
#[derive(Clone, Copy, Debug, Serialize, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum WorkStepStatus {
    Running,
    Succeeded,
    Failed,
    Cancelled,
    OutcomeUnknown,
}
#[cfg_attr(feature = "ipc-types", derive(specta::Type))]
#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct WorkStepFact {
    pub id: WorkStepId,
    pub turn: u8,
    pub kind: WorkStepKindV1,
    pub status: WorkStepStatus,
    /// Present once a model or browser step settled; turn-local steps carry none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<WorkUsage>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub artifacts: Vec<WorkArtifactId>,
    /// Provider search record produced by this step.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence: Option<WorkArtifactId>,
    /// A short line for people: what the agent said or what this step found.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// Closed measurements of a settled browser step; absent for other kinds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub measurements: Option<WorkStepMeasurementsV1>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local: Option<Box<WorkLocalStepV1>>,
    /// A page opened in the person's own session on its site.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<Box<WorkPageAccountV1>>,
    /// The part of a lead run this step works for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub part: Option<WorkPartId>,
}
impl WorkStepKindV1 {
    fn validate(&self) -> Result<(), WorkError> {
        match self {
            Self::Search { query } => super::search::validate_public_search_query(query),
            Self::Read {
                url,
                collection,
                goal,
            } => {
                validate_public_url(url)?;
                if let Some(goal) = goal {
                    validate_text(goal, MAX_WORK_PAGE_GOAL_BYTES)?;
                    if goal.trim().is_empty() || goal.chars().any(char::is_control) {
                        return Err(WorkError::Invalid);
                    }
                }
                collection.as_ref().map_or(Ok(()), |shape| shape.validate())
            }
            Self::Discover { query, collection } => {
                WorkPublicDiscoveryScope {
                    search_query: query.clone(),
                    max_hops: 1,
                }
                .validate()?;
                collection.as_ref().map_or(Ok(()), |shape| shape.validate())
            }
            Self::Ask {
                prompt,
                options,
                answer,
                ..
            } => {
                validate_text(prompt, MAX_WORK_TEXT_BYTES)?;
                if options.len() > 8 {
                    return Err(WorkError::Invalid);
                }
                let mut unique = BTreeSet::new();
                for option in options {
                    validate_text(option, 512)?;
                    if !unique.insert(option) {
                        return Err(WorkError::Invalid);
                    }
                }
                if let Some(answer) = answer {
                    validate_text(answer, MAX_WORK_TEXT_BYTES)?;
                }
                Ok(())
            }
            Self::Steer { text } => validate_text(text, MAX_WORK_TEXT_BYTES),
            Self::List { path, depth } => {
                validate_file_path(path)?;
                if depth.is_some_and(|v| !(1..=3).contains(&v)) {
                    return Err(WorkError::Invalid);
                }
                Ok(())
            }
            Self::ReadFile {
                path,
                offset,
                limit,
            } => {
                validate_file_path(path)?;
                if offset == &Some(0) || limit.is_some_and(|v| !(1..=2000).contains(&v)) {
                    return Err(WorkError::Invalid);
                }
                Ok(())
            }
            Self::SearchFiles {
                path, query, glob, ..
            } => {
                if let Some(glob) = glob {
                    validate_text(glob, MAX_WORK_FILE_QUERY_BYTES)?;
                }
                validate_file_path(path)?;
                validate_text(query, MAX_WORK_FILE_QUERY_BYTES)
            }
            Self::WriteFile { path, content, .. } => {
                validate_file_path(path)?;
                if content.len() > MAX_WORK_FILE_CONTENT_BYTES || content.contains('\0') {
                    return Err(WorkError::Invalid);
                }
                Ok(())
            }
            Self::EditFile {
                path,
                old,
                new,
                replacements,
                ..
            } => {
                validate_file_path(path)?;
                validate_replacements(old, new, replacements)
            }
            Self::MoveFile { from, to, .. } => {
                validate_file_path(from)?;
                validate_file_path(to)
            }
            Self::DeleteFile { path, .. } => validate_file_path(path),
            Self::RunCommand {
                cwd,
                command,
                timeout_secs,
                ..
            } => {
                validate_file_path(cwd)?;
                validate_command(command)?;
                if timeout_secs.is_some_and(|v| !(1..=600).contains(&v)) {
                    return Err(WorkError::Invalid);
                }
                Ok(())
            }
            Self::Finish { followups, title } => {
                if followups.len() > MAX_WORK_FOLLOWUPS
                    || title
                        .as_deref()
                        .is_some_and(|t| validate_work_title(t).is_err())
                {
                    return Err(WorkError::Invalid);
                }
                let mut unique = BTreeSet::new();
                for followup in followups {
                    validate_text(followup, MAX_WORK_FOLLOWUP_BYTES)?;
                    if !unique.insert(followup) {
                        return Err(WorkError::Invalid);
                    }
                }
                Ok(())
            }
            Self::Confirm { confirm } => confirm.validate(),
            Self::Call { call } => call.validate(),
            Self::Turn | Self::Publish => Ok(()),
        }
    }
    fn fetches(&self) -> bool {
        matches!(
            self,
            Self::Search { .. } | Self::Read { .. } | Self::Discover { .. }
        )
    }
    /// A step on the person's files; it settles with a file record.
    pub fn files(&self) -> bool {
        matches!(
            self,
            Self::List { .. }
                | Self::ReadFile { .. }
                | Self::SearchFiles { .. }
                | Self::WriteFile { .. }
                | Self::EditFile { .. }
                | Self::MoveFile { .. }
                | Self::DeleteFile { .. }
        )
    }
    /// A proposed change that waits for the person's decision.
    pub fn proposes_write(&self) -> bool {
        matches!(
            self,
            Self::WriteFile { .. }
                | Self::EditFile { .. }
                | Self::MoveFile { .. }
                | Self::DeleteFile { .. }
                | Self::RunCommand { .. }
        )
    }
    pub fn file_decision(&self) -> Option<bool> {
        match self {
            Self::WriteFile { decision, .. }
            | Self::EditFile { decision, .. }
            | Self::MoveFile { decision, .. }
            | Self::DeleteFile { decision, .. }
            | Self::RunCommand { decision, .. } => *decision,
            _ => None,
        }
    }
    fn keeps_record(&self) -> bool {
        matches!(self, Self::Search { .. } | Self::RunCommand { .. }) || self.files()
    }
}
impl WorkStepFact {
    pub fn validate(&self) -> Result<(), WorkError> {
        self.kind.validate()?;
        if let Some(local) = &self.local {
            local.validate(&self.kind)?;
        }
        if let WorkStepKindV1::Ask {
            purpose: Some(WorkAskPurposeV1::Folder),
            ..
        } = &self.kind
        {
            if self
                .local
                .as_ref()
                .is_none_or(|local| local.folder.is_none())
            {
                return Err(WorkError::Invalid);
            }
        }
        if let Some(note) = &self.note {
            validate_text(note, MAX_WORK_STEP_NOTE_BYTES)?;
        }
        if let Some(account) = &self.account {
            let WorkStepKindV1::Read { url, .. } = &self.kind else {
                return Err(WorkError::Invalid);
            };
            if !account.badge || validate_public_url(url)?.host_str() != Some(account.host.as_str())
            {
                return Err(WorkError::Invalid);
            }
        }
        if self.turn == 0 || self.artifacts.len() > MAX_WORK_ARTIFACTS {
            return Err(WorkError::Invalid);
        }
        let mut unique = BTreeSet::new();
        if !self.artifacts.iter().all(|id| unique.insert(*id)) {
            return Err(WorkError::Invalid);
        }
        let running = self.status == WorkStepStatus::Running;
        let succeeded = self.status == WorkStepStatus::Succeeded;
        let accounted = matches!(self.kind, WorkStepKindV1::Turn) || self.kind.fetches();
        let usage_expected = accounted
            && !matches!(
                self.status,
                WorkStepStatus::Running | WorkStepStatus::OutcomeUnknown
            );
        if self.usage.is_some() != usage_expected
            || (!self.artifacts.is_empty()
                && (!succeeded
                    || !(matches!(self.kind, WorkStepKindV1::Publish) || self.kind.fetches())))
            || if matches!(self.kind, WorkStepKindV1::RunCommand { .. }) {
                (running && self.evidence.is_some()) || (succeeded && self.evidence.is_none())
            } else {
                self.evidence.is_some() != (succeeded && self.kind.keeps_record())
            }
        {
            return Err(WorkError::Invalid);
        }
        let consistent = match &self.kind {
            WorkStepKindV1::Ask { answer, .. } => answer.is_some() == succeeded,
            // Committed only once approved; undecided only while it runs.
            WorkStepKindV1::Confirm { confirm } => match confirm.decision {
                None => running || self.status == WorkStepStatus::Cancelled,
                Some(WorkConfirmDecisionV1::Declined) => !succeeded,
                Some(_) => true,
            },
            // A proposal may run undecided or decided (being applied); once
            // settled, the decision is on record.
            WorkStepKindV1::WriteFile { decision, .. }
            | WorkStepKindV1::EditFile { decision, .. }
            | WorkStepKindV1::MoveFile { decision, .. }
            | WorkStepKindV1::DeleteFile { decision, .. } => !succeeded || *decision == Some(true),
            WorkStepKindV1::Publish
            | WorkStepKindV1::Finish { .. }
            | WorkStepKindV1::Steer { .. } => succeeded,
            WorkStepKindV1::Call { .. } => !running,
            WorkStepKindV1::Turn => !running,
            _ => true,
        };
        if !consistent {
            return Err(WorkError::Invalid);
        }
        Ok(())
    }
}

#[cfg_attr(feature = "ipc-types", derive(specta::Type))]
#[derive(Clone, Copy, Debug, Serialize, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum WorkInterventionKindV1 {
    /// Authentication or account selection needs a person.
    SignIn,
    /// A CAPTCHA or equivalent human challenge is present.
    Challenge,
    /// A permission or operating-system boundary needs a person.
    Permission,
    /// The page needs an interaction Zephium cannot automate safely.
    UnsupportedInteraction,
    /// The effect or its verification needs the user's review.
    Review,
    /// The user took the page over; automation was revoked and drained.
    HumanTakeover,
}
#[cfg_attr(feature = "ipc-types", derive(specta::Type))]
#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct WorkInterventionV1 {
    pub kind: WorkInterventionKindV1,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<String>,
}
impl WorkInterventionV1 {
    pub fn validate(&self) -> Result<(), WorkError> {
        if let Some(origin) = &self.origin {
            let url = validate_public_url(origin)?;
            if url.origin().ascii_serialization() != *origin {
                return Err(WorkError::Invalid);
            }
        }
        Ok(())
    }
}

/// Original provider attribution committed with its attempt's outputs. This
/// carries no native browser reference or authority to open its source URLs.
#[cfg_attr(feature = "ipc-types", derive(specta::Type))]
#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct WorkProviderSearchRecordV1 {
    pub id: WorkArtifactId,
    pub node: WorkPlanNodeId,
    pub attempt: WorkAttemptId,
    pub evidence: super::search::WorkProviderSearchEvidenceV1,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ranking: Option<super::search::WorkPublicSearchRanking>,
}
impl WorkProviderSearchRecordV1 {
    fn admits_usage(&self, usage: WorkUsage) -> bool {
        let original = self
            .evidence
            .actual_input_tokens
            .checked_add(self.evidence.actual_output_tokens);
        let Some(ranking) = &self.ranking else {
            return original == Some(usage.model_tokens);
        };
        let extra = ranking.usage;
        ranking.validate(&self.evidence).is_ok()
            && extra.operations > 0
            && original.and_then(|tokens| tokens.checked_add(extra.model_tokens))
                == Some(usage.model_tokens)
            && extra.cost_micro_usd <= usage.cost_micro_usd
            && extra
                .operations
                .checked_add(1)
                .is_some_and(|operations| operations <= usage.operations)
            && (extra.accounting != WorkUsageAccounting::ConservativeReservation
                || usage.accounting == WorkUsageAccounting::ConservativeReservation)
    }
}

#[cfg_attr(feature = "ipc-types", derive(specta::Type))]
#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct WorkFileRecordV1 {
    pub id: WorkArtifactId,
    pub node: WorkPlanNodeId,
    pub attempt: WorkAttemptId,
    pub file: WorkFileEvidenceV1,
}
#[cfg_attr(feature = "ipc-types", derive(specta::Type))]
#[derive(Clone, Copy, Debug, Serialize, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum WorkFileKindV1 {
    Directory,
    Text,
    Binary,
    Search,
    Written,
    Moved,
    Deleted,
}
/// What one file step disclosed, bounded and never the whole file system.
#[cfg_attr(feature = "ipc-types", derive(specta::Type))]
#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct WorkFileEvidenceV1 {
    pub path: String,
    pub name: String,
    pub kind: WorkFileKindV1,
    pub bytes: u32,
    /// Hex SHA-256 of the file bytes; empty for directories and searches.
    pub digest: String,
    /// The excerpt, listing, hits or applied diff shown to the agent.
    pub text: String,
    pub truncated: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before_digest: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after_digest: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lines: Option<WorkFileLinesV1>,
}
impl WorkFileEvidenceV1 {
    pub fn validate(&self) -> Result<(), WorkError> {
        validate_file_path(&self.path)?;
        for value in [&self.before_digest, &self.after_digest]
            .into_iter()
            .flatten()
        {
            validate_digest(value)?;
        }
        if self.name.is_empty()
            || self.name.len() > 255
            || self.name.chars().any(char::is_control)
            || self.text.len() > MAX_WORK_FILE_TEXT_BYTES
            || self
                .text
                .chars()
                .any(|c| c.is_control() && c != '\n' && c != '\t')
            || !(self.digest.is_empty()
                || (self.digest.len() == 64 && self.digest.bytes().all(|b| b.is_ascii_hexdigit())))
        {
            return Err(WorkError::Invalid);
        }
        Ok(())
    }
}

#[cfg_attr(feature = "ipc-types", derive(specta::Type))]
#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct WorkArtifactUserState {
    pub artifact: WorkArtifactId,
    pub revision: WorkRevision,
    pub decision: Option<WorkArtifactDecision>,
    pub edited_data: Option<WorkArtifactDataV1>,
    /// Citations for edited content. Original citations remain on the artifact.
    pub evidence: Vec<WorkEvidenceLink>,
}

#[cfg_attr(feature = "ipc-types", derive(specta::Type))]
#[derive(Clone, Copy, Debug, Serialize, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum WorkArtifactDecision {
    Accepted,
    Rejected,
}

impl WorkExecutionFact {
    pub fn needs_review(&self) -> bool {
        self.artifacts.iter().any(|artifact| {
            let edit = self
                .user_artifacts
                .iter()
                .find(|u| u.artifact == artifact.id);
            match edit.and_then(|u| u.decision) {
                Some(WorkArtifactDecision::Accepted) => false,
                Some(WorkArtifactDecision::Rejected) => true,
                None => {
                    artifact.review != WorkOutputReview::Mechanical
                        || edit.is_some_and(|u| u.edited_data.is_some())
                }
            }
        })
    }
    pub fn validate(
        &self,
        plan: &WorkPlanRevision,
        revision: WorkRevision,
    ) -> Result<(), WorkError> {
        self.spec.validate(plan)?;
        if self.authorization == WorkExecutionAuthorization::UserDirectedPublicRead {
            let [node] = self.spec.nodes.as_slice() else {
                return Err(WorkError::Invalid);
            };
            let [planned] = plan.draft.nodes.as_slice() else {
                return Err(WorkError::Invalid);
            };
            let WorkCapability::PublicSearch { scope } = &node.capability else {
                return Err(WorkError::Invalid);
            };
            super::search::validate_direct_public_read(scope, self.spec.limits)?;
            if node.parent.is_some()
                || !planned.dependencies.is_empty()
                || scope.query != planned.objective
                || node.limits != self.spec.limits
            {
                return Err(WorkError::Invalid);
            }
        }

        if self.user_artifacts.len() > self.artifacts.len() {
            return Err(WorkError::Invalid);
        }
        let mut reviewed = BTreeSet::new();
        for edit in &self.user_artifacts {
            if !reviewed.insert(edit.artifact)
                || !self.artifacts.iter().any(|a| a.id == edit.artifact)
                || edit.revision <= self.approved_revision
                || edit.revision > revision
                || edit.evidence.len() > 64
                || (edit.edited_data.is_none() && !edit.evidence.is_empty())
                || edit.evidence.iter().any(|link| {
                    !self
                        .artifacts
                        .iter()
                        .flat_map(|a| &a.evidence)
                        .any(|original| original == link)
                })
                || !matches!(
                    self.status,
                    WorkExecutionStatus::Completed | WorkExecutionStatus::NeedsReview
                )
            {
                return Err(WorkError::Invalid);
            }
            if let Some(data) = &edit.edited_data {
                data.validate(edit.evidence.len())?;
            }
        }
        if self.approved_revision <= plan.revision
            || self.approved_revision > revision
            || self.attempts.len() > MAX_WORK_ATTEMPTS
            || self.artifacts.len()
                > self
                    .agent_grant()
                    .map_or(MAX_WORK_ARTIFACTS, WorkAgentGrantV1::artifact_limit)
        {
            return Err(WorkError::Invalid);
        }
        let mut attempts = BTreeSet::new();
        let mut nodes = BTreeSet::new();
        if self
            .attempts
            .iter()
            .filter(|attempt| attempt.status == WorkAttemptStatus::Running)
            .count()
            > usize::from(self.spec.limits.max_workers)
        {
            return Err(WorkError::Invalid);
        }
        if self.is_agent() {
            return self.validate_agent(plan);
        }
        if self.authorization == WorkExecutionAuthorization::UserDirectedAgent
            || !self.steps.is_empty()
            || !self.accounts.is_empty()
            || !self.parts.is_empty()
            || !self.inputs.is_empty()
        {
            return Err(WorkError::Invalid);
        }
        for attempt in &self.attempts {
            let node = self
                .spec
                .nodes
                .iter()
                .find(|n| n.node == attempt.node)
                .ok_or(WorkError::Invalid)?;
            // One budget reservation per node. Retry requires a newly approved
            // execution; it never silently renews the approved node allocation.
            if !attempts.insert(attempt.id)
                || !nodes.insert(attempt.node)
                || node
                    .parent
                    .is_some_and(|parent| !self.attempts.iter().any(|a| a.node == parent))
                || attempt.usage.is_some_and(|u| !u.within(node.limits))
                || matches!(
                    attempt.status,
                    WorkAttemptStatus::Running | WorkAttemptStatus::OutcomeUnknown
                ) != attempt.usage.is_none()
            {
                return Err(WorkError::Invalid);
            }
            if attempt.status == WorkAttemptStatus::Succeeded {
                let node_plan = plan
                    .draft
                    .nodes
                    .iter()
                    .find(|node| node.id == attempt.node)
                    .ok_or(WorkError::Invalid)?;
                let succeeded = |id| {
                    self.attempts
                        .iter()
                        .any(|a| a.node == id && a.status == WorkAttemptStatus::Succeeded)
                };
                if !node_plan.dependencies.iter().all(|id| succeeded(*id))
                    || self
                        .spec
                        .nodes
                        .iter()
                        .any(|entry| entry.parent == Some(attempt.node) && !succeeded(entry.node))
                    || !node_plan.outputs.iter().all(|output| {
                        self.artifacts.iter().any(|artifact| {
                            artifact.attempt == attempt.id && artifact.output == output.name
                        })
                    })
                {
                    return Err(WorkError::Invalid);
                }
            }
        }
        let mut artifacts = BTreeSet::new();
        let mut outputs = BTreeSet::new();
        for artifact in &self.artifacts {
            artifact.validate()?;
            let attempt = self
                .attempts
                .iter()
                .find(|a| a.id == artifact.attempt)
                .ok_or(WorkError::Invalid)?;
            let output = plan
                .draft
                .nodes
                .iter()
                .find(|n| n.id == artifact.node)
                .and_then(|n| n.outputs.iter().find(|o| o.name == artifact.output))
                .ok_or(WorkError::Invalid)?;
            if artifact.execution != self.id
                || artifact.node != attempt.node
                || attempt.status != WorkAttemptStatus::Succeeded
                || artifact.review != output.review
                || !artifacts.insert(artifact.id)
                || !outputs.insert((artifact.node, &artifact.output))
            {
                return Err(WorkError::Invalid);
            }
        }
        if self.provider_evidence.len() > MAX_WORK_ARTIFACTS {
            return Err(WorkError::Capacity);
        }
        let mut source_ids = BTreeSet::new();
        let mut source_attempts = BTreeSet::new();
        for source in &self.provider_evidence {
            source.evidence.validate()?;
            let attempt = self
                .attempts
                .iter()
                .find(|a| a.id == source.attempt)
                .ok_or(WorkError::Invalid)?;
            let spec = self
                .spec
                .nodes
                .iter()
                .find(|n| n.node == source.node)
                .ok_or(WorkError::Invalid)?;
            let WorkCapability::PublicSearch { scope } = &spec.capability else {
                return Err(WorkError::Invalid);
            };
            if attempt.node != source.node
                || attempt.status != WorkAttemptStatus::Succeeded
                || scope.provider != source.evidence.provider
                || scope.model != source.evidence.model
                || !source_ids.insert(source.id)
                || !source_attempts.insert(source.attempt)
                || artifacts.contains(&source.id)
                || attempt
                    .usage
                    .is_none_or(|usage| !source.admits_usage(usage))
                || !self.artifacts.iter().any(|a| {
                    a.attempt == source.attempt
                        && a.evidence
                            .iter()
                            .any(|link| link.extraction_id == source.id)
                })
            {
                return Err(WorkError::Invalid);
            }
        }
        for attempt in &self.attempts {
            if attempt.status == WorkAttemptStatus::Succeeded
                && self.spec.nodes.iter().any(|n| {
                    n.node == attempt.node
                        && matches!(n.capability, WorkCapability::PublicSearch { .. })
                })
                && !source_attempts.contains(&attempt.id)
            {
                return Err(WorkError::Invalid);
            }
        }
        for artifact in &self.artifacts {
            let is_search = self.spec.nodes.iter().any(|n| {
                n.node == artifact.node
                    && matches!(n.capability, WorkCapability::PublicSearch { .. })
            });
            for link in &artifact.evidence {
                let source = self
                    .provider_evidence
                    .iter()
                    .find(|s| s.id == link.extraction_id);
                if source.is_some_and(|source| {
                    usize::from(link.source_id) > source.evidence.citations.len()
                }) || (is_search
                    && source.is_none_or(|source| source.attempt != artifact.attempt))
                {
                    return Err(WorkError::Invalid);
                }
            }
        }
        let running = self
            .attempts
            .iter()
            .any(|a| a.status == WorkAttemptStatus::Running);
        let complete = self.attempts.len() == plan.draft.nodes.len()
            && self
                .attempts
                .iter()
                .all(|a| a.status == WorkAttemptStatus::Succeeded)
            && plan.draft.nodes.iter().all(|node| {
                node.outputs.iter().all(|output| {
                    self.artifacts
                        .iter()
                        .any(|a| a.node == node.id && a.output == output.name)
                })
            });
        self.validate_status(complete, running)
    }
    fn validate_status(&self, complete: bool, running: bool) -> Result<(), WorkError> {
        let review = self.needs_review();
        let valid_status = match self.status {
            WorkExecutionStatus::Approved => self.attempts.is_empty() && self.artifacts.is_empty(),
            WorkExecutionStatus::Running => !self.attempts.is_empty() && !complete,
            WorkExecutionStatus::CancelRequested => running,
            WorkExecutionStatus::Completed => complete && !review,
            WorkExecutionStatus::NeedsReview => complete && review,
            WorkExecutionStatus::Cancelled | WorkExecutionStatus::Interrupted => !running,
            WorkExecutionStatus::Failed => {
                !running
                    && self.attempts.iter().any(|a| {
                        matches!(
                            a.status,
                            WorkAttemptStatus::Failed | WorkAttemptStatus::Cancelled
                        )
                    })
            }
        };
        if !valid_status {
            return Err(WorkError::Invalid);
        }
        Ok(())
    }
    /// Settles what an owner the app lost left behind: running attempts have
    /// unknown outcomes, and every running step says the app closed during it.
    pub fn interrupt(&mut self) {
        self.status = WorkExecutionStatus::Interrupted;
        for attempt in &mut self.attempts {
            if attempt.status == WorkAttemptStatus::Running {
                attempt.status = WorkAttemptStatus::OutcomeUnknown;
            }
        }
        for step in &mut self.steps {
            if step.status != WorkStepStatus::Running {
                continue;
            }
            // A fetch cut off mid-flight may have been charged.
            step.status = match step.kind {
                WorkStepKindV1::Search { .. }
                | WorkStepKindV1::Read { .. }
                | WorkStepKindV1::Discover { .. } => WorkStepStatus::OutcomeUnknown,
                WorkStepKindV1::RunCommand { .. } => WorkStepStatus::Failed,
                _ => WorkStepStatus::Cancelled,
            };
            step.note = Some(
                if matches!(step.kind, WorkStepKindV1::RunCommand { .. }) {
                    "Stopped when the app quit"
                } else {
                    WORK_STEP_INTERRUPTED
                }
                .into(),
            );
        }
    }
    /// Each granted origin with the signed-in pages its steps opened.
    pub fn account_use(&self) -> Vec<WorkAccountUseV1> {
        let Some(grant) = self.agent_grant() else {
            return Vec::new();
        };
        grant
            .accounts
            .iter()
            .map(|account| WorkAccountUseV1 {
                host: account.host().to_owned(),
                pages_used: u8::try_from(
                    self.steps
                        .iter()
                        .filter(|step| {
                            step.account.is_some()
                                && matches!(&step.kind, WorkStepKindV1::Read { url, .. } if account.admits(url))
                        })
                        .count(),
                )
                .unwrap_or(u8::MAX),
                pages: account.pages,
            })
            .collect()
    }
    /// Recomputes `accounts`; the Store calls it whenever steps change.
    /// Recomputes what the steps imply: the signed-in origins used and the
    /// work's name its finish gave.
    pub fn refresh_accounts(&mut self) {
        self.accounts = self.account_use();
        self.title = self.finish_title().map(str::to_owned);
    }
    fn finish_title(&self) -> Option<&str> {
        self.steps.iter().find_map(|step| match &step.kind {
            WorkStepKindV1::Finish {
                title: Some(title), ..
            } => Some(title.as_str()),
            _ => None,
        })
    }
    pub fn is_agent(&self) -> bool {
        matches!(self.spec.nodes.as_slice(), [node] if matches!(node.capability, WorkCapability::Agent { .. }))
    }
    pub fn agent_grant(&self) -> Option<&WorkAgentGrantV1> {
        match self.spec.nodes.as_slice() {
            [WorkNodeExecutionSpec {
                capability: WorkCapability::Agent { grant },
                ..
            }] => Some(grant),
            _ => None,
        }
    }
    /// Parts, inputs, and the part and revision marks of objects belong to
    /// lead runs; a revision names an earlier object, at most once per run.
    fn validate_parts(&self, grant: &WorkAgentGrantV1) -> Result<(), WorkError> {
        use super::parts::{MAX_WORK_INPUTS, MAX_WORK_PARTS};
        let lead = grant.lead.is_some();
        if (!lead
            && (!self.parts.is_empty()
                || !self.inputs.is_empty()
                || self.steps.iter().any(|s| s.part.is_some())
                || self
                    .artifacts
                    .iter()
                    .any(|a| a.part.is_some() || a.revises.is_some())))
            || self.parts.len() > MAX_WORK_PARTS
            || self.inputs.len() > MAX_WORK_INPUTS
        {
            return Err(WorkError::Invalid);
        }
        let mut parts = BTreeSet::new();
        for part in &self.parts {
            part.validate()?;
            if !parts.insert(part.id) {
                return Err(WorkError::Invalid);
            }
        }
        for input in &self.inputs {
            input.validate()?;
        }
        let mut revised = BTreeSet::new();
        for (index, artifact) in self.artifacts.iter().enumerate() {
            if artifact.part.is_some_and(|part| !parts.contains(&part)) {
                return Err(WorkError::Invalid);
            }
            if let Some(earlier) = artifact.revises {
                if !revised.insert(earlier)
                    || self.artifacts[index..].iter().any(|a| a.id == earlier)
                {
                    return Err(WorkError::Invalid);
                }
            }
        }
        Ok(())
    }
    /// One attempt, incremental steps. Artifacts and provider evidence belong
    /// to exactly one settled step; success ends with a Finish step.
    fn validate_agent(&self, plan: &WorkPlanRevision) -> Result<(), WorkError> {
        let [node] = self.spec.nodes.as_slice() else {
            return Err(WorkError::Invalid);
        };
        let WorkCapability::Agent { grant } = &node.capability else {
            return Err(WorkError::Invalid);
        };
        let [planned] = plan.draft.nodes.as_slice() else {
            return Err(WorkError::Invalid);
        };
        if self.authorization != WorkExecutionAuthorization::UserDirectedAgent
            || node.parent.is_some()
            || node.node != planned.id
            || !planned.dependencies.is_empty()
            || self.steps.len() > grant.step_limit()
            || self.steps.len() > usize::from(grant.max_steps)
            || self.attempts.len() > 1
        {
            return Err(WorkError::Invalid);
        }
        let attempt = self.attempts.first();
        if let Some(fact) = attempt {
            if fact.node != node.node
                || fact.usage.is_some_and(|u| !u.within(node.limits))
                || matches!(
                    fact.status,
                    WorkAttemptStatus::Running | WorkAttemptStatus::OutcomeUnknown
                ) != fact.usage.is_none()
            {
                return Err(WorkError::Invalid);
            }
        } else if !self.steps.is_empty()
            || !self.artifacts.is_empty()
            || !self.provider_evidence.is_empty()
        {
            return Err(WorkError::Invalid);
        }
        self.validate_parts(grant)?;
        // A signed-in read lies inside a granted origin, within its budget.
        let accounts = self.account_use();
        if self.accounts != accounts || accounts.iter().any(|use_| use_.pages_used > use_.pages) {
            return Err(WorkError::Invalid);
        }
        if self.title.as_deref() != self.finish_title() {
            return Err(WorkError::Invalid);
        }
        let running = attempt.is_some_and(|a| a.status == WorkAttemptStatus::Running);
        let complete = attempt.is_some_and(|a| a.status == WorkAttemptStatus::Succeeded);
        let mut ids = BTreeSet::new();
        let mut claimed_artifacts = BTreeSet::new();
        let mut claimed_evidence = BTreeSet::new();
        let mut running_commands = 0usize;
        let mut pending_file_change = false;
        let mut running_steps = 0usize;
        let mut finished = false;
        for step in &self.steps {
            step.validate()?;
            if !ids.insert(step.id)
                || step.turn > grant.max_turns
                || finished
                || step
                    .part
                    .is_some_and(|part| !self.parts.iter().any(|p| p.id == part))
            {
                return Err(WorkError::Invalid);
            }
            if matches!(step.kind, WorkStepKindV1::RunCommand { .. }) {
                let policy = step
                    .local
                    .as_ref()
                    .and_then(|local| local.policy.as_ref())
                    .ok_or(WorkError::Invalid)?;
                if policy.class == WorkCommandClassV1::Write
                    && policy.scope == WorkCommandApprovalScopeV1::None
                    && !self
                        .folder_approvals
                        .iter()
                        .any(|approval| approval.root == policy.root)
                {
                    return Err(WorkError::Invalid);
                }
                if (step.status == WorkStepStatus::Succeeded || step.evidence.is_some())
                    && policy.scope != WorkCommandApprovalScopeV1::None
                    && step.kind.file_decision() != Some(true)
                {
                    return Err(WorkError::Invalid);
                }
            }
            if step.status == WorkStepStatus::Running {
                if !running {
                    return Err(WorkError::Invalid);
                }
                // A held site step waits beside its page; it is no worker.
                if !matches!(step.kind, WorkStepKindV1::Confirm { confirm: _ }) {
                    running_steps += 1;
                }
                if matches!(step.kind, WorkStepKindV1::RunCommand { .. })
                    && (step.kind.file_decision() == Some(true)
                        || step
                            .local
                            .as_ref()
                            .and_then(|l| l.policy.as_ref())
                            .is_some_and(|p| p.scope == WorkCommandApprovalScopeV1::None))
                {
                    running_commands += 1;
                }
                pending_file_change |= step.kind.files()
                    && step.kind.proposes_write()
                    && step.kind.file_decision().is_none();
            }
            for artifact in &step.artifacts {
                if !claimed_artifacts.insert(*artifact)
                    || !self.artifacts.iter().any(|a| a.id == *artifact)
                {
                    return Err(WorkError::Invalid);
                }
            }
            if let Some(evidence) = step.evidence {
                if !claimed_evidence.insert(evidence) {
                    return Err(WorkError::Invalid);
                }
                if step.kind.files() {
                    if !self
                        .file_evidence
                        .iter()
                        .any(|record| record.id == evidence)
                    {
                        return Err(WorkError::Invalid);
                    }
                } else if matches!(step.kind, WorkStepKindV1::RunCommand { .. }) {
                    if !self.command_evidence.iter().any(|r| r.id == evidence) {
                        return Err(WorkError::Invalid);
                    }
                } else {
                    let record = self
                        .provider_evidence
                        .iter()
                        .find(|record| record.id == evidence)
                        .ok_or(WorkError::Invalid)?;
                    let usage = step.usage.ok_or(WorkError::Invalid)?;
                    if !record.admits_usage(usage) {
                        return Err(WorkError::Invalid);
                    }
                }
            }
            if matches!(step.kind, WorkStepKindV1::Finish { .. }) {
                finished = true;
            }
        }
        if running_commands > MAX_WORK_RUNNING_COMMANDS
            || (running_commands > 0 && pending_file_change)
            || running_steps > usize::from(self.spec.limits.max_workers)
            || (complete && (running_steps > 0 || !finished))
            || (finished && !(complete || running))
            || claimed_artifacts.len() != self.artifacts.len()
            || claimed_evidence.len()
                != self.provider_evidence.len()
                    + self.file_evidence.len()
                    + self.command_evidence.len()
        {
            return Err(WorkError::Invalid);
        }
        let mut artifacts = BTreeSet::new();
        for artifact in &self.artifacts {
            artifact.validate()?;
            let fact = attempt.ok_or(WorkError::Invalid)?;
            let output = planned
                .outputs
                .iter()
                .find(|o| o.name == artifact.output)
                .ok_or(WorkError::Invalid)?;
            if artifact.execution != self.id
                || artifact.node != node.node
                || artifact.attempt != fact.id
                || artifact.review != output.review
                || !artifacts.insert(artifact.id)
            {
                return Err(WorkError::Invalid);
            }
            for link in &artifact.evidence {
                if self
                    .provider_evidence
                    .iter()
                    .find(|s| s.id == link.extraction_id)
                    .is_some_and(|source| {
                        usize::from(link.source_id) > source.evidence.citations.len()
                    })
                {
                    return Err(WorkError::Invalid);
                }
            }
        }
        let mut sources = BTreeSet::new();
        for source in &self.provider_evidence {
            source.evidence.validate()?;
            let fact = attempt.ok_or(WorkError::Invalid)?;
            if source.attempt != fact.id
                || source.node != node.node
                || source.evidence.provider != grant.provider
                || source.evidence.model != grant.model
                || !sources.insert(source.id)
                || artifacts.contains(&source.id)
            {
                return Err(WorkError::Invalid);
            }
        }
        for record in &self.file_evidence {
            record.file.validate()?;
            let fact = attempt.ok_or(WorkError::Invalid)?;
            if record.attempt != fact.id
                || record.node != node.node
                || !sources.insert(record.id)
                || artifacts.contains(&record.id)
            {
                return Err(WorkError::Invalid);
            }
        }
        for record in &self.command_evidence {
            record.command.validate()?;
            let fact = attempt.ok_or(WorkError::Invalid)?;
            if record.attempt != fact.id
                || record.node != node.node
                || !sources.insert(record.id)
                || artifacts.contains(&record.id)
            {
                return Err(WorkError::Invalid);
            }
        }
        if self.folder_approvals.len() > MAX_WORK_FOLDERS {
            return Err(WorkError::Invalid);
        }
        let mut approved = BTreeSet::new();
        for approval in &self.folder_approvals {
            validate_file_path(&approval.root)?;
            if approval.at.parse::<u64>().is_err()
                || !approved.insert(&approval.root)
                || !self.steps.iter().any(|s| {
                    s.kind.file_decision() == Some(true)
                        && s.local
                            .as_ref()
                            .and_then(|l| l.policy.as_ref())
                            .is_some_and(|p| {
                                p.scope == WorkCommandApprovalScopeV1::Folder
                                    && p.root == approval.root
                            })
                })
            {
                return Err(WorkError::Invalid);
            }
        }
        self.validate_status(complete, running)
    }
}

/// Internal Store grammar. User commands and host-only attempt facts have
/// separate variants at the application edge; IPC never accepts settlements.
#[cfg_attr(feature = "ipc-types", derive(specta::Type))]
#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum WorkRuntimeIntent {
    ReadPublic {
        scope: super::search::WorkPublicSearchScope,
        limits: WorkExecutionLimits,
    },
    ReviewArtifact {
        execution: WorkExecutionId,
        artifact: WorkArtifactId,
        decision: WorkArtifactDecision,
    },
    EditArtifact {
        execution: WorkExecutionId,
        artifact: WorkArtifactId,
        data: WorkArtifactDataV1,
        evidence: Vec<WorkEvidenceLink>,
    },
    Approve {
        spec: WorkExecutionSpec,
    },
    Cancel {
        execution: WorkExecutionId,
        /// Present when a person takes the page over rather than abandoning
        /// the work; persisted with the execution.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        intervention: Option<WorkInterventionV1>,
    },
    /// Explicitly acknowledge that an old owner is gone. Cannot restart it.
    AcknowledgeInterruption {
        execution: WorkExecutionId,
    },
    /// Start the routine agent loop on the objective. Mints the single-step
    /// plan and the execution in one transaction; the grant is the approval.
    BeginAgent {
        grant: WorkAgentGrantV1,
        limits: WorkExecutionLimits,
    },
    /// Answer a question the running agent asked; allowed while it runs.
    AnswerStep {
        execution: WorkExecutionId,
        step: WorkStepId,
        answer: String,
    },
    /// Hand the running agent a message; it reads it at its next turn.
    Steer {
        execution: WorkExecutionId,
        text: String,
    },
    /// Decide a proposed file change or held site step the running agent
    /// is waiting on. `for_run` accepts a site's offered run-wide allowance.
    ApproveStep {
        execution: WorkExecutionId,
        step: WorkStepId,
        approve: bool,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        for_run: bool,
    },
}

#[derive(Clone, Debug)]
pub enum WorkRuntimeUpdate {
    PageTitle {
        execution: WorkExecutionId,
        attempt: WorkAttemptId,
        step: WorkStepId,
        title: String,
    },
    SettleProviderSearch {
        execution: WorkExecutionId,
        attempt: WorkAttemptId,
        status: WorkAttemptStatus,
        usage: Option<WorkUsage>,
        artifacts: Vec<WorkArtifactV1>,
        evidence: Box<WorkProviderSearchRecordV1>,
    },
    Begin {
        execution: WorkExecutionId,
        attempt: WorkAttemptId,
        node: WorkPlanNodeId,
    },
    /// Host-only join to an original live parent attempt. The application also
    /// requires its move-only supervisor turn; IPC cannot construct this intent.
    BeginChild {
        execution: WorkExecutionId,
        attempt: WorkAttemptId,
        node: WorkPlanNodeId,
        parent: WorkAttemptId,
    },
    Settle {
        execution: WorkExecutionId,
        attempt: WorkAttemptId,
        status: WorkAttemptStatus,
        usage: Option<WorkUsage>,
        artifacts: Vec<WorkArtifactV1>,
        intervention: Option<WorkInterventionV1>,
    },
    FinishCancellation {
        execution: WorkExecutionId,
    },
    /// Admit one agent step; a turn-local step may already be settled and
    /// carry its artifacts.
    BeginStep {
        execution: WorkExecutionId,
        attempt: WorkAttemptId,
        step: WorkStepFact,
        artifacts: Vec<WorkArtifactV1>,
        evidence: Option<Box<WorkProviderSearchRecordV1>>,
        file: Option<Box<WorkFileRecordV1>>,
    },
    CommandProgress {
        execution: WorkExecutionId,
        attempt: WorkAttemptId,
        step: WorkStepId,
        output: WorkCommandOutputV1,
    },
    SettleCommand {
        execution: WorkExecutionId,
        attempt: WorkAttemptId,
        step: WorkStepId,
        status: WorkStepStatus,
        record: Box<WorkCommandRecordV1>,
        note: String,
    },
    SettleStep {
        execution: WorkExecutionId,
        attempt: WorkAttemptId,
        step: WorkStepId,
        status: WorkStepStatus,
        usage: Option<WorkUsage>,
        artifacts: Vec<WorkArtifactV1>,
        evidence: Option<Box<WorkProviderSearchRecordV1>>,
        file: Option<Box<WorkFileRecordV1>>,
        note: Option<String>,
        measurements: Option<WorkStepMeasurementsV1>,
    },
    /// The person let a running agent keep going past a spent budget: its
    /// token, cost and operation limits grow; nothing else changes.
    ExtendLimits {
        execution: WorkExecutionId,
        attempt: WorkAttemptId,
        limits: WorkExecutionLimits,
    },
    /// A lead run's part as it stands now: added once, then replaced by id.
    Part {
        execution: WorkExecutionId,
        attempt: WorkAttemptId,
        part: super::parts::WorkPartFactV1,
    },
    /// Something a lead run pulled in.
    Input {
        execution: WorkExecutionId,
        attempt: WorkAttemptId,
        input: super::parts::WorkInputFactV1,
    },
}

#[cfg_attr(feature = "ipc-types", derive(specta::Type))]
#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct WorkExecutionOwnership {
    pub execution: WorkExecutionId,
    /// Observation identity only. Never a worker token or restart authority.
    pub owner: WorkRuntimeSessionId,
}

#[cfg_attr(feature = "ipc-types", derive(specta::Type))]
#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct WorkRuntimeProjection {
    pub version: u16,
    pub work: WorkSnapshot,
    pub executions: Vec<WorkExecutionFact>,
    /// Old incarnation has no live authority. Facts and reservations remain.
    pub interrupted: Vec<WorkExecutionId>,
    /// Exact original execution owners. Older projections without this field
    /// remain readable, but cannot admit transient activity.
    #[serde(default)]
    pub owners: Vec<WorkExecutionOwnership>,
}
#[cfg_attr(feature = "ipc-types", derive(specta::Type))]
#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct WorkCommandReceipt {
    pub command: WorkCommandId,
    pub applied_revision: WorkRevision,
    pub execution: WorkExecutionId,
}

#[cfg(test)]
mod discovery_query_tests {
    use super::*;

    #[test]
    fn unicode_discovery_query_matches_schema_and_preserves_bounded_navigation() {
        for character in ['a', '—', '界', '😀', '&', '%'] {
            let mut scope = WorkPublicDiscoveryScope {
                search_query: character
                    .to_string()
                    .repeat(WORK_PUBLIC_DISCOVERY_MAX_QUERY_CHARS),
                max_hops: WORK_PUBLIC_DISCOVERY_MAX_HOPS,
            };
            assert!(scope.validate().is_ok());
            assert!(scope.search_query.len() <= WORK_PUBLIC_DISCOVERY_MAX_QUERY_BYTES);
            let url = scope.start_url().unwrap();
            assert!(
                url.as_str().len() < 8192,
                "must fit the existing native navigation URL ceiling"
            );
            assert_eq!(
                url.query_pairs().find(|(key, _)| key == "q").unwrap().1,
                scope.search_query
            );
            assert_eq!(url.origin().ascii_serialization(), "https://www.bing.com");
            scope.search_query.push(character);
            assert_eq!(scope.start_url(), Err(WorkError::Invalid));
        }
        let invalid = WorkPublicDiscoveryScope {
            search_query: "public\0query".into(),
            max_hops: WORK_PUBLIC_DISCOVERY_MAX_HOPS,
        };
        assert_eq!(invalid.validate(), Err(WorkError::Invalid));
    }
}

#[cfg(test)]
mod search_ranking_tests {
    use super::*;
    use crate::work::search::*;

    #[test]
    fn ranking_usage_is_additive_and_cannot_replace_original_search_attribution() {
        let mut record = WorkProviderSearchRecordV1 {
            id: 1.into(),
            node: 2.into(),
            attempt: 3.into(),
            ranking: None,
            evidence: WorkProviderSearchEvidenceV1 {
                version: 1,
                provider: WorkSearchProvider::OpenAi,
                model: PUBLIC_SEARCH_MODEL.into(),
                response_model: PUBLIC_SEARCH_MODEL.into(),
                response_id: "resp_fixture".into(),
                search_call_id: "ws_fixture".into(),
                answer: "Public source".into(),
                citations: vec![WorkProviderSearchCitation {
                    url: "https://example.test/".into(),
                    title: "Source".into(),
                    start_index: 0,
                    end_index: 6,
                }],
                actual_input_tokens: 100,
                actual_output_tokens: 10,
            },
        };
        let original = WorkUsage {
            model_tokens: 110,
            cost_micro_usd: 10,
            operations: 1,
            accounting: WorkUsageAccounting::Exact,
        };
        assert!(record.admits_usage(original));
        assert!(record.admits_usage(WorkUsage {
            operations: 0,
            ..original
        }));
        let legacy = serde_json::to_value(&record).unwrap();
        assert!(legacy.get("ranking").is_none());
        assert!(serde_json::from_value::<WorkProviderSearchRecordV1>(legacy)
            .unwrap()
            .ranking
            .is_none());
        record.ranking = Some(WorkPublicSearchRanking {
            preferred: vec![1],
            usage: WorkUsage {
                model_tokens: 20,
                cost_micro_usd: 5,
                operations: 1,
                accounting: WorkUsageAccounting::ConservativeReservation,
            },
        });
        let total = WorkUsage {
            model_tokens: 130,
            cost_micro_usd: 15,
            operations: 2,
            accounting: WorkUsageAccounting::ConservativeReservation,
        };
        assert!(record.admits_usage(total));
        assert!(!record.admits_usage(original));
        for invalid in [
            WorkUsage {
                model_tokens: 131,
                ..total
            },
            WorkUsage {
                operations: 1,
                ..total
            },
            WorkUsage {
                cost_micro_usd: 4,
                ..total
            },
            WorkUsage {
                accounting: WorkUsageAccounting::Exact,
                ..total
            },
        ] {
            assert!(!record.admits_usage(invalid));
        }
        let mut foreign = record.clone();
        foreign.ranking.as_mut().unwrap().preferred = vec![2];
        assert!(!foreign.admits_usage(total));
        let mut duplicate = record.clone();
        duplicate.ranking.as_mut().unwrap().preferred = vec![1, 1];
        assert!(!duplicate.admits_usage(total));
        assert_eq!(record.evidence.actual_input_tokens, 100);
        assert_eq!(record.evidence.actual_output_tokens, 10);
    }
}
