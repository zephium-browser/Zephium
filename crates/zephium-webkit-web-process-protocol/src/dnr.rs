//! Profile-wide DNR publication. Index bytes remain borrowed from the sealed map.
#![forbid(unsafe_code)]

use std::collections::BTreeSet;

pub use engine::HeaderOperation;
use url::Url;
use zephium_webext_dnr as engine;
use zephium_webext_matcher::{HostPattern, UrlGlob};

use crate::policy::{Decision, RequestInfo, RequestPolicy, ResourceType};

pub const MAX_DNR_SNAPSHOT_BYTES: usize = 256 * 1024 * 1024;
pub const MAX_DNR_EXTENSIONS: usize = 64;
pub const MAX_DNR_MATCHED_RULES: usize = 256;
pub const LINUX_DNR_LIMITATIONS: &[&str] = &[
    "WebSocket handshakes, service-worker-initiated fetches and usually the service-worker script fetch bypass the page request hook.",
    "Response headers cannot be modified; response-header rule conditions are not evaluated.",
    "Subframe allowAllRequests cannot establish a subtree exemption without request frame identity.",
    "Non-HTTPS public hosts get no fetch metadata: only documents, frames, objects, stylesheets and images are typed from WebKit's Accept header; scripts, fonts, fetches, workers and pings there are other.",
    "Media and prefetch requests were not issued by WebKitGTK in qualification and are unverified.",
    "Beacon and no-cors POST fetches share ping classification when loader metadata cannot distinguish them.",
    "Absent or opaque initiators remain unknown; host-requiring subresource actions are refused.",
    "WebTransport and WebBundle request attribution is unavailable.",
];
const MAX_PATTERNS: usize = 4096;
const MAGIC: &[u8; 8] = b"ZWDNR\x01\0\0";

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DnrResourceAccess {
    pub resources: Vec<String>,
    pub matches: Vec<String>,
    pub extension_ids: Vec<String>,
}

#[derive(Clone, Debug, Default)]
pub struct DnrExtensionPolicy {
    pub extension_id: String,
    pub install_order: u64,
    pub host_permissions: Vec<String>,
    /// True when only declarativeNetRequestWithHostAccess is granted.
    pub require_host_access: bool,
    pub web_accessible_resources: Vec<DnrResourceAccess>,
    pub rulesets: Vec<Vec<u8>>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DnrMatchedRule {
    pub extension_id: String,
    pub ruleset_id: String,
    pub rule_id: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DnrFrameAllow {
    pub extension_id: String,
    pub priority: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DnrPolicyMatch {
    pub decision: Decision,
    pub matched_rules: Vec<DnrMatchedRule>,
    pub frame_allow_updates: Vec<DnrFrameAllow>,
}

struct ResourceAccess {
    resources: Vec<UrlGlob>,
    matches: Vec<HostPattern>,
    extension_ids: Vec<String>,
}
struct Extension<'a> {
    id: &'a str,
    order: u64,
    hosts: Vec<HostPattern>,
    require_host_access: bool,
    resources: Vec<ResourceAccess>,
    rulesets: Vec<engine::CompiledRuleset<'a>>,
}

pub struct DnrPolicySnapshot<'a> {
    extensions: Vec<Extension<'a>>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DnrPolicyError;
impl std::fmt::Display for DnrPolicyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("invalid DNR policy snapshot")
    }
}
impl std::error::Error for DnrPolicyError {}

impl<'a> DnrPolicySnapshot<'a> {
    pub fn is_dnr(bytes: &[u8]) -> bool {
        bytes.starts_with(MAGIC)
    }

    pub fn encode(extensions: &[DnrExtensionPolicy]) -> Result<Vec<u8>, DnrPolicyError> {
        let mut out = MAGIC.to_vec();
        count(&mut out, extensions.len(), MAX_DNR_EXTENSIONS)?;
        for extension in extensions {
            text(&mut out, &extension.extension_id)?;
            out.extend_from_slice(&extension.install_order.to_le_bytes());
            out.push(u8::from(extension.require_host_access));
            strings(&mut out, &extension.host_permissions)?;
            count(
                &mut out,
                extension.web_accessible_resources.len(),
                MAX_PATTERNS,
            )?;
            for access in &extension.web_accessible_resources {
                strings(&mut out, &access.resources)?;
                strings(&mut out, &access.matches)?;
                strings(&mut out, &access.extension_ids)?;
            }
            count(
                &mut out,
                extension.rulesets.len(),
                engine::MAX_NUMBER_OF_STATIC_RULESETS + 2,
            )?;
            for ruleset in &extension.rulesets {
                count(&mut out, ruleset.len(), MAX_DNR_SNAPSHOT_BYTES)?;
                if out.len().saturating_add(ruleset.len()) > MAX_DNR_SNAPSHOT_BYTES {
                    return Err(DnrPolicyError);
                }
                out.extend_from_slice(ruleset);
            }
        }
        if out.len() > MAX_DNR_SNAPSHOT_BYTES {
            return Err(DnrPolicyError);
        }
        DnrPolicySnapshot::load(&out)?;
        Ok(out)
    }

    pub fn load(bytes: &'a [u8]) -> Result<Self, DnrPolicyError> {
        if bytes.len() > MAX_DNR_SNAPSHOT_BYTES {
            return Err(DnrPolicyError);
        }
        let mut reader = Reader(bytes);
        if reader.take(MAGIC.len())? != MAGIC {
            return Err(DnrPolicyError);
        }
        let mut extensions = Vec::new();
        let mut ids = BTreeSet::new();
        let mut orders = BTreeSet::new();
        let mut pattern_count = 0usize;
        for _ in 0..reader.count(MAX_DNR_EXTENSIONS)? {
            let id = reader.text()?;
            if !valid_id(id) || !ids.insert(id) {
                return Err(DnrPolicyError);
            }
            let order = u64::from_le_bytes(reader.take(8)?.try_into().map_err(|_| DnrPolicyError)?);
            if !orders.insert(order) {
                return Err(DnrPolicyError);
            }
            let require_host_access = match reader.take(1)?[0] {
                0 => false,
                1 => true,
                _ => return Err(DnrPolicyError),
            };
            let mut hosts = Vec::new();
            for pattern in reader.strings(&mut pattern_count)? {
                hosts.push(HostPattern::parse(pattern).map_err(|_| DnrPolicyError)?);
            }
            let mut resources = Vec::new();
            for _ in 0..reader.count(MAX_PATTERNS)? {
                let mut patterns = Vec::new();
                for pattern in reader.strings(&mut pattern_count)? {
                    if pattern.is_empty()
                        || pattern.contains('\\')
                        || pattern.split('/').any(|p| p == "..")
                    {
                        return Err(DnrPolicyError);
                    }
                    patterns.push(
                        UrlGlob::parse(pattern.trim_start_matches('/'))
                            .map_err(|_| DnrPolicyError)?,
                    );
                }
                let mut matches = Vec::new();
                for pattern in reader.strings(&mut pattern_count)? {
                    matches.push(HostPattern::parse(pattern).map_err(|_| DnrPolicyError)?);
                }
                let extension_ids = reader.strings(&mut pattern_count)?;
                if extension_ids.iter().any(|id| *id != "*" && !valid_id(id)) {
                    return Err(DnrPolicyError);
                }
                resources.push(ResourceAccess {
                    resources: patterns,
                    matches,
                    extension_ids: extension_ids.into_iter().map(str::to_owned).collect(),
                });
            }
            let mut rulesets = Vec::new();
            for _ in 0..reader.count(engine::MAX_NUMBER_OF_STATIC_RULESETS + 2)? {
                let length = reader.count(MAX_DNR_SNAPSHOT_BYTES)?;
                rulesets.push(
                    engine::CompiledRuleset::load(reader.take(length)?)
                        .map_err(|_| DnrPolicyError)?,
                );
            }
            engine::validate_extension_rulesets(
                &rulesets.iter().collect::<Vec<_>>(),
                engine::MAX_INDEX_RULES,
            )
            .map_err(|_| DnrPolicyError)?;
            extensions.push(Extension {
                id,
                order,
                hosts,
                require_host_access,
                resources,
                rulesets,
            });
        }
        if !reader.0.is_empty() {
            return Err(DnrPolicyError);
        }
        Ok(Self { extensions })
    }

    pub fn evaluate(&self, info: &RequestInfo, inherited: &[DnrFrameAllow]) -> DnrPolicyMatch {
        let no_match = || DnrPolicyMatch {
            decision: Decision::Allow,
            matched_rules: Vec::new(),
            frame_allow_updates: Vec::new(),
        };
        let Ok(url) = Url::parse(&info.url) else {
            return no_match();
        };
        if !matches!(url.scheme(), "http" | "https" | "ftp") {
            return no_match();
        }
        let initiator = info
            .initiator
            .as_deref()
            .and_then(|value| Url::parse(value).ok());
        let top = info
            .document_url
            .as_deref()
            .and_then(|value| Url::parse(value).ok());
        let frames: Vec<_> = inherited
            .iter()
            .map(|allow| engine::FrameAllow {
                extension_id: allow.extension_id.as_str(),
                priority: allow.priority,
            })
            .collect();
        let mut request = engine::Request::new(
            url.as_str(),
            url.host_str().unwrap_or_default(),
            resource_type(info.resource_type),
        );
        request.initiator_host = initiator.as_ref().and_then(Url::host_str);
        request.top_host = top.as_ref().and_then(Url::host_str);
        request.method = request_method(info.method.as_deref().unwrap_or_default());
        request.tab_id = info.tab_id;
        request.domain_type = domain_type(request.host, request.initiator_host);
        request.frame_allow = &frames;
        let decision = engine::evaluate_filtered(
            &request,
            self.extensions.iter().flat_map(|extension| {
                extension
                    .rulesets
                    .iter()
                    .map(move |set| (extension.id, extension.order, set))
            }),
            |id, rule| {
                let Some(extension) = self.extensions.iter().find(|extension| extension.id == id)
                else {
                    return false;
                };
                if rule.condition.has_response_header_condition() {
                    return false;
                }
                let requires_hosts = extension.require_host_access
                    || !matches!(
                        rule.action,
                        engine::Action::Block
                            | engine::Action::Allow
                            | engine::Action::AllowAllRequests
                            | engine::Action::UpgradeScheme
                    );
                !requires_hosts
                    || (extension.hosts.iter().any(|host| host.matches_url(&url))
                        && (matches!(
                            request.resource_type,
                            engine::ResourceType::MainFrame | engine::ResourceType::SubFrame
                        ) || initiator.as_ref().is_some_and(|origin| {
                            extension.hosts.iter().any(|host| host.matches_url(origin))
                        })))
            },
        );
        let mut action = match decision.action {
            engine::DecisionAction::Allow => Decision::Allow,
            engine::DecisionAction::Block => Decision::Cancel,
            engine::DecisionAction::Upgrade { url } => Decision::Redirect(url),
            engine::DecisionAction::Redirect { url: target } => {
                if self.redirect_allowed(
                    &target,
                    decision.matched_rules.first().map(|rule| rule.extension_id),
                    initiator.as_ref(),
                ) {
                    Decision::Redirect(target)
                } else {
                    Decision::Cancel
                }
            }
        };
        if matches!(action, Decision::Allow) && !decision.request_headers.is_empty() {
            action = Decision::ModifyRequestHeaders(
                decision
                    .request_headers
                    .into_iter()
                    .map(|edit| edit.modification)
                    .collect(),
            );
        }
        DnrPolicyMatch {
            decision: action,
            matched_rules: decision
                .matched_rules
                .into_iter()
                .take(MAX_DNR_MATCHED_RULES)
                .map(|rule| DnrMatchedRule {
                    extension_id: rule.extension_id.into(),
                    ruleset_id: rule.ruleset_id.into(),
                    rule_id: rule.rule_id,
                })
                .collect(),
            frame_allow_updates: decision
                .frame_allow_updates
                .into_iter()
                .map(|allow| DnrFrameAllow {
                    extension_id: allow.extension_id.into(),
                    priority: allow.priority,
                })
                .collect(),
        }
    }

    fn redirect_allowed(&self, target: &str, owner: Option<&str>, initiator: Option<&Url>) -> bool {
        let Ok(url) = Url::parse(target) else {
            return false;
        };
        if matches!(url.scheme(), "http" | "https" | "data") {
            return true;
        }
        if url.scheme() != "chrome-extension"
            || url.host_str() != owner
            || url.port().is_some()
            || !url.username().is_empty()
            || url.password().is_some()
        {
            return false;
        }
        let Some(extension) = self
            .extensions
            .iter()
            .find(|extension| Some(extension.id) == owner)
        else {
            return false;
        };
        if initiator.is_some_and(|origin| {
            origin.scheme() == "chrome-extension" && origin.host_str() == owner
        }) {
            return true;
        }
        extension.resources.iter().any(|access| {
            access
                .resources
                .iter()
                .any(|pattern| pattern.matches(url.path().trim_start_matches('/')))
                && initiator.is_some_and(|origin| {
                    access
                        .matches
                        .iter()
                        .any(|pattern| pattern.matches_url(origin))
                        || (origin.scheme() == "chrome-extension"
                            && access
                                .extension_ids
                                .iter()
                                .any(|id| id == "*" || Some(id.as_str()) == origin.host_str()))
                })
        })
    }
}

impl RequestPolicy for DnrPolicySnapshot<'_> {
    fn decide(&self, request: &RequestInfo) -> Decision {
        self.evaluate(request, &[]).decision
    }
}

pub use engine::domain_type;

pub fn request_method(method: &str) -> engine::RequestMethod {
    match method {
        "CONNECT" => engine::RequestMethod::Connect,
        "DELETE" => engine::RequestMethod::Delete,
        "GET" => engine::RequestMethod::Get,
        "HEAD" => engine::RequestMethod::Head,
        "OPTIONS" => engine::RequestMethod::Options,
        "PATCH" => engine::RequestMethod::Patch,
        "POST" => engine::RequestMethod::Post,
        "PUT" => engine::RequestMethod::Put,
        _ => engine::RequestMethod::Other,
    }
}

pub fn resource_type(kind: ResourceType) -> engine::ResourceType {
    match kind {
        ResourceType::MainFrame => engine::ResourceType::MainFrame,
        ResourceType::SubFrame => engine::ResourceType::SubFrame,
        ResourceType::Script | ResourceType::Worker => engine::ResourceType::Script,
        ResourceType::Stylesheet => engine::ResourceType::Stylesheet,
        ResourceType::Image => engine::ResourceType::Image,
        ResourceType::Font => engine::ResourceType::Font,
        ResourceType::Media => engine::ResourceType::Media,
        ResourceType::Fetch => engine::ResourceType::Xmlhttprequest,
        ResourceType::Object => engine::ResourceType::Object,
        ResourceType::Ping => engine::ResourceType::Ping,
        ResourceType::CspReport => engine::ResourceType::CspReport,
        ResourceType::Other => engine::ResourceType::Other,
    }
}
fn valid_id(id: &str) -> bool {
    id.len() == 32 && id.bytes().all(|byte| (b'a'..=b'p').contains(&byte))
}
fn count(out: &mut Vec<u8>, count: usize, maximum: usize) -> Result<(), DnrPolicyError> {
    if count > maximum || out.len().saturating_add(4) > MAX_DNR_SNAPSHOT_BYTES {
        return Err(DnrPolicyError);
    }
    out.extend_from_slice(&(count as u32).to_le_bytes());
    Ok(())
}
fn text(out: &mut Vec<u8>, value: &str) -> Result<(), DnrPolicyError> {
    if value.len() > 8192
        || value.contains('\0')
        || out.len().saturating_add(value.len()).saturating_add(4) > MAX_DNR_SNAPSHOT_BYTES
    {
        return Err(DnrPolicyError);
    }
    count(out, value.len(), 8192)?;
    out.extend_from_slice(value.as_bytes());
    Ok(())
}
fn strings(out: &mut Vec<u8>, values: &[String]) -> Result<(), DnrPolicyError> {
    count(out, values.len(), MAX_PATTERNS)?;
    for value in values {
        text(out, value)?;
    }
    Ok(())
}
struct Reader<'a>(&'a [u8]);
impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], DnrPolicyError> {
        let (head, tail) = self.0.split_at_checked(n).ok_or(DnrPolicyError)?;
        self.0 = tail;
        Ok(head)
    }
    fn count(&mut self, max: usize) -> Result<usize, DnrPolicyError> {
        let value =
            u32::from_le_bytes(self.take(4)?.try_into().map_err(|_| DnrPolicyError)?) as usize;
        if value > max {
            return Err(DnrPolicyError);
        }
        Ok(value)
    }
    fn text(&mut self) -> Result<&'a str, DnrPolicyError> {
        let n = self.count(8192)?;
        let value = std::str::from_utf8(self.take(n)?).map_err(|_| DnrPolicyError)?;
        if value.contains('\0') {
            return Err(DnrPolicyError);
        }
        Ok(value)
    }
    fn strings(&mut self, total: &mut usize) -> Result<Vec<&'a str>, DnrPolicyError> {
        let n = self.count(MAX_PATTERNS)?;
        *total = total.checked_add(n).ok_or(DnrPolicyError)?;
        if *total > MAX_PATTERNS {
            return Err(DnrPolicyError);
        }
        (0..n).map(|_| self.text()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine::{Action, Condition, HeaderModification, Redirect, Rule, RulesetKind};

    const OWNER: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const OTHER: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    fn rule(id: u32, priority: u32, action: Action) -> Rule {
        Rule {
            id,
            priority,
            action,
            condition: Condition::default(),
        }
    }
    fn policy(id: &str, order: u64, rules: Vec<Rule>) -> DnrExtensionPolicy {
        DnrExtensionPolicy {
            extension_id: id.into(),
            install_order: order,
            host_permissions: vec!["<all_urls>".into()],
            rulesets: vec![
                engine::CompiledRuleset::compile("static", RulesetKind::Static, rules)
                    .unwrap()
                    .to_bytes(),
            ],
            ..DnrExtensionPolicy::default()
        }
    }
    fn request() -> RequestInfo {
        RequestInfo {
            url: "https://sub.example.co.uk/track.js?remove=1&keep=2".into(),
            tab_id: 7,
            method: Some("GET".into()),
            resource_type: ResourceType::Script,
            frame_id: None,
            document_url: Some("https://other.example.co.uk/index.html".into()),
            initiator: Some("https://other.example.co.uk".into()),
            is_main_frame: false,
        }
    }
    fn run(
        extensions: &[DnrExtensionPolicy],
        info: &RequestInfo,
        inherited: &[DnrFrameAllow],
    ) -> DnrPolicyMatch {
        let bytes = DnrPolicySnapshot::encode(extensions).unwrap();
        DnrPolicySnapshot::load(&bytes)
            .unwrap()
            .evaluate(info, inherited)
    }

    #[test]
    fn arbitration_spans_rulesets_but_allow_never_exempts_another_extension() {
        let mut extension = policy(OWNER, 1, vec![rule(1, 1, Action::Block)]);
        extension.rulesets.push(
            engine::CompiledRuleset::compile(
                "_dynamic",
                RulesetKind::Dynamic,
                vec![rule(2, 2, Action::Allow)],
            )
            .unwrap()
            .to_bytes(),
        );
        assert_eq!(
            run(std::slice::from_ref(&extension), &request(), &[]).decision,
            Decision::Allow
        );
        let blocked = run(
            &[extension, policy(OTHER, 2, vec![rule(3, 1, Action::Block)])],
            &request(),
            &[],
        );
        assert_eq!(blocked.decision, Decision::Cancel);
        assert_eq!(blocked.matched_rules[0].extension_id, OTHER);
    }

    #[test]
    fn newest_extension_wins_redirect_ties_without_changing_block_precedence() {
        let redirect = |url: &str| Action::Redirect {
            redirect: Box::new(Redirect {
                url: Some(url.into()),
                ..Redirect::default()
            }),
        };
        let result = run(
            &[
                policy(OWNER, 1, vec![rule(1, 999, redirect("https://old.test/"))]),
                policy(OTHER, 2, vec![rule(2, 1, redirect("https://new.test/"))]),
            ],
            &request(),
            &[],
        );
        assert_eq!(
            result.decision,
            Decision::Redirect("https://new.test/".into())
        );
        assert_eq!(result.matched_rules[0].rule_id, 2);
    }

    #[test]
    fn request_and_non_navigation_initiator_need_granted_hosts() {
        let mut extension = policy(
            OWNER,
            1,
            vec![rule(
                1,
                1,
                Action::Redirect {
                    redirect: Box::new(Redirect {
                        url: Some("https://redirect.test/".into()),
                        ..Redirect::default()
                    }),
                },
            )],
        );
        extension.host_permissions = vec!["https://sub.example.co.uk/*".into()];
        assert_eq!(
            run(std::slice::from_ref(&extension), &request(), &[]).decision,
            Decision::Allow
        );
        let mut navigation = request();
        navigation.resource_type = ResourceType::SubFrame;
        assert_eq!(
            run(std::slice::from_ref(&extension), &navigation, &[]).decision,
            Decision::Redirect("https://redirect.test/".into())
        );
        extension
            .host_permissions
            .push("https://other.example.co.uk/*".into());
        assert_eq!(run(&[extension], &request(), &[]).matched_rules.len(), 1);
    }

    #[test]
    fn with_host_access_gates_even_block_and_plain_dnr_safe_actions_work_without_hosts() {
        let mut extension = policy(OWNER, 1, vec![rule(1, 1, Action::Block)]);
        extension.host_permissions.clear();
        assert_eq!(
            run(std::slice::from_ref(&extension), &request(), &[]).decision,
            Decision::Cancel
        );
        extension.require_host_access = true;
        assert_eq!(run(&[extension], &request(), &[]).decision, Decision::Allow);
        let mut upgrade = policy(OWNER, 1, vec![rule(1, 1, Action::UpgradeScheme)]);
        upgrade.host_permissions.clear();
        let mut info = request();
        info.url = "http://example.test/a".into();
        assert_eq!(
            run(&[upgrade], &info, &[]).decision,
            Decision::Redirect("https://example.test/a".into())
        );
    }

    #[test]
    fn extension_redirects_require_own_web_accessible_path_and_initiator() {
        let mut extension = policy(
            OWNER,
            1,
            vec![rule(
                1,
                1,
                Action::Redirect {
                    redirect: Box::new(Redirect {
                        extension_path: Some("/public/noop.js".into()),
                        ..Redirect::default()
                    }),
                },
            )],
        );
        assert_eq!(
            run(std::slice::from_ref(&extension), &request(), &[]).decision,
            Decision::Cancel
        );
        extension.web_accessible_resources.push(DnrResourceAccess {
            resources: vec!["public/*.js".into()],
            matches: vec!["https://*.example.co.uk/*".into()],
            extension_ids: Vec::new(),
        });
        assert_eq!(
            run(std::slice::from_ref(&extension), &request(), &[]).decision,
            Decision::Redirect(format!("chrome-extension://{OWNER}/public/noop.js"))
        );
        let mut info = request();
        info.initiator = None;
        assert_eq!(run(&[extension], &info, &[]).decision, Decision::Allow);
    }

    #[test]
    fn regex_substitution_and_query_transform_use_engine_semantics() {
        let mut regex = rule(
            1,
            1,
            Action::Redirect {
                redirect: Box::new(Redirect {
                    regex_substitution: Some("https://destination.test/\\1".into()),
                    ..Redirect::default()
                }),
            },
        );
        regex.condition.regex_filter = Some("^https://[^/]+/(.*)$".into());
        assert_eq!(
            run(&[policy(OWNER, 1, vec![regex])], &request(), &[]).decision,
            Decision::Redirect("https://destination.test/track.js?remove=1&keep=2".into())
        );
        let transform = rule(
            2,
            1,
            Action::Redirect {
                redirect: Box::new(Redirect {
                    transform: Some(engine::UrlTransform {
                        host: Some("destination.test".into()),
                        query_transform: Some(engine::QueryTransform {
                            remove_params: vec!["remove".into()],
                            add_or_replace_params: vec![engine::QueryKeyValue {
                                key: "keep".into(),
                                value: "updated".into(),
                                replace_only: true,
                            }],
                        }),
                        ..engine::UrlTransform::default()
                    }),
                    ..Redirect::default()
                }),
            },
        );
        assert_eq!(
            run(&[policy(OWNER, 1, vec![transform])], &request(), &[]).decision,
            Decision::Redirect("https://destination.test/track.js?keep=updated".into())
        );
    }

    #[test]
    fn request_header_set_append_remove_preserve_cross_extension_arbitration() {
        let headers = |id, name: &str, operation, value: Option<&str>| {
            rule(
                id,
                1,
                Action::ModifyHeaders {
                    request_headers: vec![HeaderModification {
                        header: name.into(),
                        operation,
                        value: value.map(str::to_owned),
                    }],
                    response_headers: Vec::new(),
                },
            )
        };
        let result = run(
            &[
                policy(
                    OWNER,
                    1,
                    vec![headers(1, "x-test", HeaderOperation::Set, Some("older"))],
                ),
                policy(
                    OTHER,
                    2,
                    vec![headers(2, "x-test", HeaderOperation::Remove, None)],
                ),
            ],
            &request(),
            &[],
        );
        let Decision::ModifyRequestHeaders(edits) = result.decision else {
            panic!("headers")
        };
        assert_eq!(edits.len(), 1);
        assert_eq!(edits[0].operation, HeaderOperation::Remove);
        assert_eq!(result.matched_rules[0].extension_id, OTHER);
    }

    #[test]
    fn allow_all_emits_document_state_and_inherited_priority_is_extension_scoped() {
        let mut allow = rule(1, 5, Action::AllowAllRequests);
        allow.condition.resource_types = Some(vec![engine::ResourceType::MainFrame]);
        let extension = policy(OWNER, 1, vec![allow, rule(2, 1, Action::Block)]);
        let mut main = request();
        main.resource_type = ResourceType::MainFrame;
        main.is_main_frame = true;
        let result = run(std::slice::from_ref(&extension), &main, &[]);
        assert_eq!(
            result.frame_allow_updates,
            vec![DnrFrameAllow {
                extension_id: OWNER.into(),
                priority: 5
            }]
        );
        assert_eq!(
            run(
                std::slice::from_ref(&extension),
                &request(),
                &result.frame_allow_updates
            )
            .decision,
            Decision::Allow
        );
        assert_eq!(
            run(
                &[extension, policy(OTHER, 2, vec![rule(3, 1, Action::Block)])],
                &request(),
                &result.frame_allow_updates
            )
            .decision,
            Decision::Cancel
        );
    }

    #[test]
    fn response_conditions_never_match_the_request_stage() {
        let mut response = rule(1, 100, Action::Block);
        response.condition.response_headers = Some(vec![engine::HeaderCondition {
            header: "x-test".into(),
            values: None,
            excluded_values: None,
        }]);
        assert_eq!(
            run(&[policy(OWNER, 1, vec![response])], &request(), &[]).matched_rules,
            Vec::new()
        );
    }

    #[test]
    fn session_tab_and_party_facts_are_not_replaced_by_top_url() {
        let mut candidate = rule(1, 1, Action::Block);
        candidate.condition.tab_ids = Some(vec![7]);
        candidate.condition.domain_type = Some(engine::DomainType::ThirdParty);
        let mut extension = policy(OWNER, 1, Vec::new());
        extension.rulesets.push(
            engine::CompiledRuleset::compile("_session", RulesetKind::Session, vec![candidate])
                .unwrap()
                .to_bytes(),
        );
        let mut info = request();
        info.initiator = Some("https://unrelated.test".into());
        assert_eq!(
            run(std::slice::from_ref(&extension), &info, &[]).decision,
            Decision::Cancel
        );
        info.tab_id = 8;
        assert_eq!(
            run(std::slice::from_ref(&extension), &info, &[]).decision,
            Decision::Allow
        );
        info.tab_id = 7;
        info.initiator = Some("http://other.example.co.uk".into());
        assert_eq!(run(&[extension], &info, &[]).decision, Decision::Allow);
    }

    #[test]
    fn snapshots_reject_truncation_corruption_duplicate_extensions_and_rulesets() {
        let extension = policy(OWNER, 1, vec![rule(1, 1, Action::Block)]);
        let bytes = DnrPolicySnapshot::encode(std::slice::from_ref(&extension)).unwrap();
        for end in 0..bytes.len() {
            assert!(DnrPolicySnapshot::load(&bytes[..end]).is_err(), "{end}");
        }
        let mut trailing = bytes.clone();
        trailing.push(0);
        assert!(DnrPolicySnapshot::load(&trailing).is_err());
        assert!(DnrPolicySnapshot::encode(&[extension.clone(), extension.clone()]).is_err());
        let mut duplicate = extension;
        duplicate.rulesets.push(duplicate.rulesets[0].clone());
        assert!(DnrPolicySnapshot::encode(&[duplicate]).is_err());
        let mut corrupt = bytes;
        *corrupt.last_mut().unwrap() ^= 1;
        assert!(DnrPolicySnapshot::load(&corrupt).is_err());
    }

    #[test]
    fn snapshots_refuse_bad_hosts_ids_and_resource_traversal() {
        for change in 0..4 {
            let mut extension = policy(OWNER, 1, Vec::new());
            match change {
                0 => extension.extension_id = "forged".into(),
                1 => extension.host_permissions.push("https://[bad".into()),
                2 => extension.web_accessible_resources.push(DnrResourceAccess {
                    resources: vec!["../private".into()],
                    ..DnrResourceAccess::default()
                }),
                _ => extension.web_accessible_resources.push(DnrResourceAccess {
                    extension_ids: vec!["invalid".into()],
                    ..DnrResourceAccess::default()
                }),
            }
            assert!(DnrPolicySnapshot::encode(&[extension]).is_err(), "{change}");
        }
    }
}
