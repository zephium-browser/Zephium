//! Engine-neutral declarative network request rule data.

pub const GUARANTEED_MINIMUM_STATIC_RULES: usize = 30_000;
pub const MAX_NUMBER_OF_DYNAMIC_RULES: usize = 30_000;
pub const MAX_NUMBER_OF_UNSAFE_DYNAMIC_RULES: usize = 5_000;
pub const MAX_NUMBER_OF_SESSION_RULES: usize = 5_000;
pub const MAX_NUMBER_OF_UNSAFE_SESSION_RULES: usize = 5_000;
pub const MAX_NUMBER_OF_REGEX_RULES: usize = 1_000;
pub const MAX_NUMBER_OF_STATIC_RULESETS: usize = 100;
pub const MAX_NUMBER_OF_ENABLED_STATIC_RULESETS: usize = 50;
pub const MAX_NUMBER_OF_DISABLED_STATIC_RULES: usize = 5_000;
pub const DYNAMIC_RULESET_ID: &str = "_dynamic";
pub const SESSION_RULESET_ID: &str = "_session";

/// Rust implementation bounds; RE2's program-memory accounting is unrelated.
pub const MAX_REGEX_SOURCE_BYTES: usize = 4 * 1024;
pub const MAX_REGEX_NFA_BYTES: usize = 256 * 1024;
pub const MAX_REGEX_DFA_CACHE_BYTES: usize = 512 * 1024;
pub const MAX_REGEX_NFA_BYTES_PER_RULESET: usize = 16 * 1024 * 1024;
pub const MAX_REGEX_NFA_BYTES_PER_EXTENSION: usize = 64 * 1024 * 1024;
/// Other implementation resource bounds, separate from Chrome's quotas.
pub const MAX_RULESET_JSON_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_RULES_PER_RULESET: usize = 1_000_000;
pub const MAX_HEADER_PATTERNS_PER_RULE: usize = 64;
pub const MAX_HEADER_PATTERNS_PER_RULESET: usize = 4_096;
pub const MAX_HEADER_PATTERN_MEMORY_BYTES: usize = 8 * 1024;
pub const MAX_HEADER_PATTERN_BYTES: usize = 8 * 1024;
pub const MAX_DECODED_ACTION_BYTES_PER_RULESET: usize = 64 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "compile", derive(serde::Serialize, serde::Deserialize))]
pub enum RulesetKind {
    Static,
    Dynamic,
    Session,
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "compile", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "compile", serde(rename_all = "camelCase"))]
pub struct Rule {
    pub id: u32,
    #[cfg_attr(feature = "compile", serde(default = "default_priority"))]
    pub priority: u32,
    pub action: Action,
    pub condition: Condition,
}

#[cfg(feature = "compile")]
fn default_priority() -> u32 {
    1
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "compile", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "compile", serde(tag = "type", rename_all = "camelCase"))]
pub enum Action {
    Block,
    Allow,
    AllowAllRequests,
    UpgradeScheme,
    Redirect {
        redirect: Box<Redirect>,
    },
    ModifyHeaders {
        #[cfg_attr(
            feature = "compile",
            serde(
                default,
                rename = "requestHeaders",
                skip_serializing_if = "Vec::is_empty"
            )
        )]
        request_headers: Vec<HeaderModification>,
        #[cfg_attr(
            feature = "compile",
            serde(
                default,
                rename = "responseHeaders",
                skip_serializing_if = "Vec::is_empty"
            )
        )]
        response_headers: Vec<HeaderModification>,
    },
}

impl Action {
    pub fn is_safe(&self) -> bool {
        matches!(
            self,
            Self::Block | Self::Allow | Self::AllowAllRequests | Self::UpgradeScheme
        )
    }

    /// Linux's request hook cannot apply response-header modifications.
    pub fn has_unsupported_response_header_modifications(&self) -> bool {
        matches!(self, Self::ModifyHeaders { response_headers, .. } if !response_headers.is_empty())
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "compile", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "compile", serde(default, rename_all = "camelCase"))]
pub struct Condition {
    #[cfg_attr(feature = "compile", serde(skip_serializing_if = "Option::is_none"))]
    pub url_filter: Option<String>,
    #[cfg_attr(feature = "compile", serde(skip_serializing_if = "Option::is_none"))]
    pub regex_filter: Option<String>,
    pub is_url_filter_case_sensitive: bool,
    #[cfg_attr(feature = "compile", serde(skip_serializing_if = "Option::is_none"))]
    pub initiator_domains: Option<Vec<String>>,
    #[cfg_attr(feature = "compile", serde(skip_serializing_if = "Option::is_none"))]
    pub excluded_initiator_domains: Option<Vec<String>>,
    #[cfg_attr(feature = "compile", serde(skip_serializing_if = "Option::is_none"))]
    pub request_domains: Option<Vec<String>>,
    #[cfg_attr(feature = "compile", serde(skip_serializing_if = "Option::is_none"))]
    pub excluded_request_domains: Option<Vec<String>>,
    #[cfg_attr(feature = "compile", serde(skip_serializing_if = "Option::is_none"))]
    pub top_domains: Option<Vec<String>>,
    #[cfg_attr(feature = "compile", serde(skip_serializing_if = "Option::is_none"))]
    pub excluded_top_domains: Option<Vec<String>>,
    #[cfg_attr(feature = "compile", serde(skip_serializing_if = "Option::is_none"))]
    pub domains: Option<Vec<String>>,
    #[cfg_attr(feature = "compile", serde(skip_serializing_if = "Option::is_none"))]
    pub excluded_domains: Option<Vec<String>>,
    #[cfg_attr(feature = "compile", serde(skip_serializing_if = "Option::is_none"))]
    pub resource_types: Option<Vec<ResourceType>>,
    #[cfg_attr(feature = "compile", serde(skip_serializing_if = "Option::is_none"))]
    pub excluded_resource_types: Option<Vec<ResourceType>>,
    #[cfg_attr(feature = "compile", serde(skip_serializing_if = "Option::is_none"))]
    pub request_methods: Option<Vec<RequestMethod>>,
    #[cfg_attr(feature = "compile", serde(skip_serializing_if = "Option::is_none"))]
    pub excluded_request_methods: Option<Vec<RequestMethod>>,
    #[cfg_attr(feature = "compile", serde(skip_serializing_if = "Option::is_none"))]
    pub domain_type: Option<DomainType>,
    #[cfg_attr(feature = "compile", serde(skip_serializing_if = "Option::is_none"))]
    pub tab_ids: Option<Vec<i32>>,
    #[cfg_attr(feature = "compile", serde(skip_serializing_if = "Option::is_none"))]
    pub excluded_tab_ids: Option<Vec<i32>>,
    #[cfg_attr(feature = "compile", serde(skip_serializing_if = "Option::is_none"))]
    pub response_headers: Option<Vec<HeaderCondition>>,
    #[cfg_attr(feature = "compile", serde(skip_serializing_if = "Option::is_none"))]
    pub excluded_response_headers: Option<Vec<HeaderCondition>>,
}

impl Condition {
    pub fn has_response_header_condition(&self) -> bool {
        self.response_headers.is_some() || self.excluded_response_headers.is_some()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "compile", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "compile", serde(rename_all = "snake_case"))]
pub enum ResourceType {
    MainFrame,
    SubFrame,
    Stylesheet,
    Script,
    Image,
    Font,
    Object,
    Xmlhttprequest,
    Ping,
    CspReport,
    Media,
    Websocket,
    Webtransport,
    Webbundle,
    Other,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "compile", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "compile", serde(rename_all = "lowercase"))]
pub enum RequestMethod {
    Connect,
    Delete,
    Get,
    Head,
    Options,
    Patch,
    Post,
    Put,
    Other,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "compile", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "compile", serde(rename_all = "camelCase"))]
pub enum DomainType {
    FirstParty,
    ThirdParty,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "compile", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "compile", serde(rename_all = "lowercase"))]
pub enum HeaderOperation {
    Append,
    Set,
    Remove,
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "compile", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "compile", serde(rename_all = "camelCase"))]
pub struct HeaderModification {
    pub header: String,
    pub operation: HeaderOperation,
    #[cfg_attr(feature = "compile", serde(skip_serializing_if = "Option::is_none"))]
    pub value: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "compile", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "compile", serde(rename_all = "camelCase"))]
pub struct HeaderCondition {
    pub header: String,
    #[cfg_attr(feature = "compile", serde(skip_serializing_if = "Option::is_none"))]
    pub values: Option<Vec<String>>,
    #[cfg_attr(feature = "compile", serde(skip_serializing_if = "Option::is_none"))]
    pub excluded_values: Option<Vec<String>>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "compile", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "compile", serde(default, rename_all = "camelCase"))]
pub struct Redirect {
    #[cfg_attr(feature = "compile", serde(skip_serializing_if = "Option::is_none"))]
    pub url: Option<String>,
    #[cfg_attr(feature = "compile", serde(skip_serializing_if = "Option::is_none"))]
    pub extension_path: Option<String>,
    #[cfg_attr(feature = "compile", serde(skip_serializing_if = "Option::is_none"))]
    pub regex_substitution: Option<String>,
    #[cfg_attr(feature = "compile", serde(skip_serializing_if = "Option::is_none"))]
    pub transform: Option<UrlTransform>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "compile", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "compile", serde(default, rename_all = "camelCase"))]
pub struct UrlTransform {
    #[cfg_attr(feature = "compile", serde(skip_serializing_if = "Option::is_none"))]
    pub scheme: Option<String>,
    #[cfg_attr(feature = "compile", serde(skip_serializing_if = "Option::is_none"))]
    pub host: Option<String>,
    #[cfg_attr(feature = "compile", serde(skip_serializing_if = "Option::is_none"))]
    pub port: Option<String>,
    #[cfg_attr(feature = "compile", serde(skip_serializing_if = "Option::is_none"))]
    pub path: Option<String>,
    #[cfg_attr(feature = "compile", serde(skip_serializing_if = "Option::is_none"))]
    pub query: Option<String>,
    #[cfg_attr(feature = "compile", serde(skip_serializing_if = "Option::is_none"))]
    pub query_transform: Option<QueryTransform>,
    #[cfg_attr(feature = "compile", serde(skip_serializing_if = "Option::is_none"))]
    pub fragment: Option<String>,
    #[cfg_attr(feature = "compile", serde(skip_serializing_if = "Option::is_none"))]
    pub username: Option<String>,
    #[cfg_attr(feature = "compile", serde(skip_serializing_if = "Option::is_none"))]
    pub password: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "compile", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "compile", serde(default, rename_all = "camelCase"))]
pub struct QueryTransform {
    pub remove_params: Vec<String>,
    pub add_or_replace_params: Vec<QueryKeyValue>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "compile", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "compile", serde(rename_all = "camelCase"))]
pub struct QueryKeyValue {
    pub key: String,
    pub value: String,
    #[cfg_attr(feature = "compile", serde(default))]
    pub replace_only: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuleRejection {
    pub index: usize,
    pub id: Option<u32>,
    pub reason: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ParsedRuleset {
    pub rules: Vec<Rule>,
    pub rejected: Vec<RuleRejection>,
}
