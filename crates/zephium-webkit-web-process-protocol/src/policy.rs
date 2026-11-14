//! A bounded snapshot adapter; DNR can implement `RequestPolicy` independently.
#![forbid(unsafe_code)]

use std::collections::BTreeSet;

pub const MAX_SNAPSHOT_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_RULES: usize = 4096;
pub const MAX_URL_BYTES: usize = 8192;
pub const MAX_HEADERS: usize = 32;
const MAGIC: &[u8; 8] = b"ZWPS\x01\0\0\0";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum ResourceType {
    MainFrame = 1,
    SubFrame,
    Script,
    Stylesheet,
    Image,
    Font,
    Media,
    Fetch,
    Worker,
    Other,
    Object,
    Ping,
    CspReport,
}

impl ResourceType {
    fn decode(value: u8) -> Result<Self, PolicyError> {
        match value {
            1 => Ok(Self::MainFrame),
            2 => Ok(Self::SubFrame),
            3 => Ok(Self::Script),
            4 => Ok(Self::Stylesheet),
            5 => Ok(Self::Image),
            6 => Ok(Self::Font),
            7 => Ok(Self::Media),
            8 => Ok(Self::Fetch),
            9 => Ok(Self::Worker),
            10 => Ok(Self::Other),
            11 => Ok(Self::Object),
            12 => Ok(Self::Ping),
            13 => Ok(Self::CspReport),
            _ => Err(PolicyError),
        }
    }

    /// Fetch metadata is advisory; absent metadata must remain unknown.
    pub fn infer(destination: Option<&str>) -> Self {
        match destination {
            Some("document") => Self::MainFrame,
            Some("iframe" | "frame") => Self::SubFrame,
            Some("script") => Self::Script,
            Some("object" | "embed") => Self::Object,
            Some("report") => Self::CspReport,
            Some("style") => Self::Stylesheet,
            Some("image") => Self::Image,
            Some("font") => Self::Font,
            Some("audio" | "video" | "track") => Self::Media,
            Some("worker" | "sharedworker" | "serviceworker") => Self::Worker,
            Some("empty") => Self::Fetch,
            _ => Self::Other,
        }
    }

    /// WebKit omits fetch metadata for requests to non-trustworthy URLs. Its
    /// loader then still sets a type-specific `Accept`; pages can spoof that
    /// value only for their own fetches, and `*/*` stays unknown.
    pub fn infer_without_metadata(accept: Option<&str>, page_navigation: bool) -> Self {
        let accept = accept.unwrap_or_default();
        if accept.starts_with("text/html") {
            if page_navigation {
                Self::MainFrame
            } else {
                Self::SubFrame
            }
        } else if accept.starts_with("text/css") {
            Self::Stylesheet
        } else if accept.starts_with("image/") {
            Self::Image
        } else {
            Self::Other
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RequestInfo {
    pub url: String,
    pub tab_id: i32,
    pub method: Option<String>,
    pub resource_type: ResourceType,
    /// WebKit's page-level hook does not expose the originating frame.
    pub frame_id: Option<u64>,
    pub document_url: Option<String>,
    pub initiator: Option<String>,
    pub is_main_frame: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Header {
    pub name: String,
    pub value: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Decision {
    Allow,
    Cancel,
    Redirect(String),
    SetRequestHeaders(Vec<Header>),
    ModifyRequestHeaders(Vec<zephium_webext_dnr::HeaderModification>),
}

pub trait RequestPolicy {
    fn decide(&self, request: &RequestInfo) -> Decision;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rule {
    pub id: u32,
    pub url: String,
    pub method: Option<String>,
    pub resource_type: Option<ResourceType>,
    pub decision: Decision,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PolicyMatch {
    pub rule_id: Option<u32>,
    pub decision: Decision,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PolicySnapshot {
    rules: Vec<Rule>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PolicyError;

impl std::fmt::Display for PolicyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("invalid request policy snapshot")
    }
}

impl std::error::Error for PolicyError {}

impl PolicySnapshot {
    pub fn new(rules: Vec<Rule>) -> Result<Self, PolicyError> {
        if rules.len() > MAX_RULES {
            return Err(PolicyError);
        }
        let mut ids = BTreeSet::new();
        for rule in &rules {
            if rule.id == 0 || !ids.insert(rule.id) || !valid_url(&rule.url) {
                return Err(PolicyError);
            }
            if rule
                .method
                .as_ref()
                .is_some_and(|m| m.is_empty() || m.len() > 32 || !m.bytes().all(is_token))
            {
                return Err(PolicyError);
            }
            match &rule.decision {
                Decision::ModifyRequestHeaders(_) => return Err(PolicyError),
                Decision::Redirect(url) if !valid_url(url) => return Err(PolicyError),
                Decision::SetRequestHeaders(headers) => {
                    if headers.is_empty() || headers.len() > MAX_HEADERS {
                        return Err(PolicyError);
                    }
                    let mut names = BTreeSet::new();
                    for header in headers {
                        if header.name.is_empty()
                            || header.name.len() > 256
                            || !header.name.bytes().all(is_token)
                            || !names.insert(header.name.to_ascii_lowercase())
                            || header.value.len() > 8192
                            || header
                                .value
                                .bytes()
                                .any(|b| b < 0x20 && b != b'\t' || b == 0x7f)
                        {
                            return Err(PolicyError);
                        }
                    }
                }
                _ => {}
            }
        }
        let snapshot = Self { rules };
        snapshot.encode()?;
        Ok(snapshot)
    }

    pub fn evaluate(&self, request: &RequestInfo) -> PolicyMatch {
        for rule in &self.rules {
            if rule.url == request.url
                && rule
                    .method
                    .as_ref()
                    .is_none_or(|method| request.method.as_ref() == Some(method))
                && rule
                    .resource_type
                    .is_none_or(|kind| kind == request.resource_type)
            {
                return PolicyMatch {
                    rule_id: Some(rule.id),
                    decision: rule.decision.clone(),
                };
            }
        }
        PolicyMatch {
            rule_id: None,
            decision: Decision::Allow,
        }
    }

    /// ZWPS/version, u32 rule count, then rules; all numbers little-endian.
    /// Strings have u32 byte lengths. Empty method/type zero means unrestricted.
    pub fn encode(&self) -> Result<Vec<u8>, PolicyError> {
        let mut out = MAGIC.to_vec();
        out.extend_from_slice(&(self.rules.len() as u32).to_le_bytes());
        for rule in &self.rules {
            out.extend_from_slice(&rule.id.to_le_bytes());
            write_text(&mut out, &rule.url)?;
            write_text(&mut out, rule.method.as_deref().unwrap_or_default())?;
            out.push(rule.resource_type.map_or(0, |kind| kind as u8));
            match &rule.decision {
                Decision::ModifyRequestHeaders(_) => return Err(PolicyError),
                Decision::Allow => out.push(0),
                Decision::Cancel => out.push(1),
                Decision::Redirect(url) => {
                    out.push(2);
                    write_text(&mut out, url)?;
                }
                Decision::SetRequestHeaders(headers) => {
                    out.push(3);
                    out.push(headers.len() as u8);
                    for header in headers {
                        write_text(&mut out, &header.name)?;
                        write_text(&mut out, &header.value)?;
                    }
                }
            }
            if out.len() > MAX_SNAPSHOT_BYTES {
                return Err(PolicyError);
            }
        }
        Ok(out)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, PolicyError> {
        if bytes.len() > MAX_SNAPSHOT_BYTES {
            return Err(PolicyError);
        }
        let mut input = Reader(bytes);
        if input.take(MAGIC.len())? != MAGIC {
            return Err(PolicyError);
        }
        let count = input.u32()? as usize;
        if count > MAX_RULES {
            return Err(PolicyError);
        }
        let mut rules = Vec::new();
        for _ in 0..count {
            let id = input.u32()?;
            let url = input.text(MAX_URL_BYTES)?;
            let method = input.text(32)?;
            let resource = input.byte()?;
            let resource_type = if resource == 0 {
                None
            } else {
                Some(ResourceType::decode(resource)?)
            };
            let decision = match input.byte()? {
                0 => Decision::Allow,
                1 => Decision::Cancel,
                2 => Decision::Redirect(input.text(MAX_URL_BYTES)?),
                3 => {
                    let count = input.byte()? as usize;
                    if count > MAX_HEADERS {
                        return Err(PolicyError);
                    }
                    let mut headers = Vec::new();
                    for _ in 0..count {
                        headers.push(Header {
                            name: input.text(256)?,
                            value: input.text(8192)?,
                        });
                    }
                    Decision::SetRequestHeaders(headers)
                }
                _ => return Err(PolicyError),
            };
            rules.push(Rule {
                id,
                url,
                method: (!method.is_empty()).then_some(method),
                resource_type,
                decision,
            });
        }
        if !input.0.is_empty() {
            return Err(PolicyError);
        }
        Self::new(rules)
    }
}

impl RequestPolicy for PolicySnapshot {
    fn decide(&self, request: &RequestInfo) -> Decision {
        self.evaluate(request).decision
    }
}

fn valid_url(text: &str) -> bool {
    if text.len() > MAX_URL_BYTES || text.bytes().any(|b| b <= 0x20 || b == 0x7f) {
        return false;
    }
    url::Url::parse(text).is_ok_and(|url| {
        matches!(url.scheme(), "http" | "https" | "chrome-extension")
            && url.host_str().is_some()
            && url.username().is_empty()
            && url.password().is_none()
            && (url.scheme() != "chrome-extension"
                || (url.port().is_none()
                    && url.host_str().is_some_and(|host| {
                        host.len() == 32 && host.bytes().all(|byte| (b'a'..=b'p').contains(&byte))
                    })))
    })
}

fn is_token(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&byte)
}

fn write_text(out: &mut Vec<u8>, text: &str) -> Result<(), PolicyError> {
    if out.len().saturating_add(4).saturating_add(text.len()) > MAX_SNAPSHOT_BYTES {
        return Err(PolicyError);
    }
    out.extend_from_slice(&(text.len() as u32).to_le_bytes());
    out.extend_from_slice(text.as_bytes());
    Ok(())
}

struct Reader<'a>(&'a [u8]);
impl<'a> Reader<'a> {
    fn take(&mut self, size: usize) -> Result<&'a [u8], PolicyError> {
        let (head, tail) = self.0.split_at_checked(size).ok_or(PolicyError)?;
        self.0 = tail;
        Ok(head)
    }
    fn byte(&mut self) -> Result<u8, PolicyError> {
        Ok(self.take(1)?[0])
    }
    fn u32(&mut self) -> Result<u32, PolicyError> {
        Ok(u32::from_le_bytes(
            self.take(4)?.try_into().map_err(|_| PolicyError)?,
        ))
    }
    fn text(&mut self, cap: usize) -> Result<String, PolicyError> {
        let len = self.u32()? as usize;
        if len > cap {
            return Err(PolicyError);
        }
        Ok(std::str::from_utf8(self.take(len)?)
            .map_err(|_| PolicyError)?
            .to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn rule(decision: Decision) -> Rule {
        Rule {
            id: 1,
            url: "https://fixture.test/a".into(),
            method: None,
            resource_type: None,
            decision,
        }
    }
    fn request() -> RequestInfo {
        RequestInfo {
            url: "https://fixture.test/a".into(),
            tab_id: -1,
            method: Some("GET".into()),
            resource_type: ResourceType::Fetch,
            frame_id: None,
            document_url: None,
            initiator: None,
            is_main_frame: false,
        }
    }
    #[test]
    fn decisions_roundtrip_and_do_not_match_other_urls() {
        for decision in [
            Decision::Allow,
            Decision::Cancel,
            Decision::Redirect(
                "chrome-extension://aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa/noop.js".into(),
            ),
            Decision::SetRequestHeaders(vec![Header {
                name: "X-Test".into(),
                value: "yes".into(),
            }]),
        ] {
            let policy = PolicySnapshot::new(vec![rule(decision.clone())]).unwrap();
            let decoded = PolicySnapshot::decode(&policy.encode().unwrap()).unwrap();
            assert_eq!(decoded, policy);
            assert_eq!(decoded.decide(&request()), decision);
            let mut other = request();
            other.url.push('b');
            assert_eq!(decoded.evaluate(&other).rule_id, None);
        }
    }
    #[test]
    fn first_match_respects_method_and_resource_constraints() {
        let mut first = rule(Decision::Cancel);
        first.method = Some("POST".into());
        first.resource_type = Some(ResourceType::Worker);
        let mut second = rule(Decision::Allow);
        second.id = 2;
        let policy = PolicySnapshot::new(vec![first, second]).unwrap();
        let mut req = request();
        assert_eq!(policy.evaluate(&req).rule_id, Some(2));
        req.method = Some("POST".into());
        assert_eq!(policy.decide(&req), Decision::Allow);
        req.resource_type = ResourceType::Worker;
        assert_eq!(policy.decide(&req), Decision::Cancel);
        req.method = None;
        assert_eq!(policy.decide(&req), Decision::Allow);
    }
    #[test]
    fn malformed_snapshots_and_header_injection_are_refused() {
        let bytes = PolicySnapshot::new(vec![rule(Decision::Cancel)])
            .unwrap()
            .encode()
            .unwrap();
        for end in 0..bytes.len() {
            assert!(PolicySnapshot::decode(&bytes[..end]).is_err());
        }
        let mut trailing = bytes.clone();
        trailing.push(0);
        assert!(PolicySnapshot::decode(&trailing).is_err());
        let mut version = bytes;
        version[4] = 2;
        assert!(PolicySnapshot::decode(&version).is_err());
        for name in ["X Bad", "X\r\nInjected", ""] {
            assert!(
                PolicySnapshot::new(vec![rule(Decision::SetRequestHeaders(vec![Header {
                    name: name.into(),
                    value: "ok".into()
                }]))])
                .is_err()
            );
        }
        for value in ["ok\r\nInjected: true", "nul\0", "control\u{7f}"] {
            assert!(
                PolicySnapshot::new(vec![rule(Decision::SetRequestHeaders(vec![Header {
                    name: "X-Test".into(),
                    value: value.into()
                }]))])
                .is_err()
            );
        }
        for url in [
            "javascript:alert(1)",
            "file:///etc/passwd",
            "https://u:p@fixture.test/",
            "https://fixture.test/\n",
        ] {
            assert!(PolicySnapshot::new(vec![rule(Decision::Redirect(url.into()))]).is_err());
        }
    }
    #[test]
    fn bounded_counts_and_duplicate_ids_fail_closed() {
        assert!(PolicySnapshot::new(vec![rule(Decision::Allow); MAX_RULES + 1]).is_err());
        assert!(PolicySnapshot::new(vec![rule(Decision::Allow); 2]).is_err());
        assert!(PolicySnapshot::decode(&vec![0; MAX_SNAPSHOT_BYTES + 1]).is_err());
        assert_eq!(ResourceType::infer(None), ResourceType::Other);
        let document = Some("text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8");
        assert_eq!(
            ResourceType::infer_without_metadata(document, true),
            ResourceType::MainFrame
        );
        assert_eq!(
            ResourceType::infer_without_metadata(document, false),
            ResourceType::SubFrame
        );
        assert_eq!(
            ResourceType::infer_without_metadata(Some("text/css,*/*;q=0.1"), false),
            ResourceType::Stylesheet
        );
        assert_eq!(
            ResourceType::infer_without_metadata(Some("image/webp,image/avif,*/*;q=0.5"), false),
            ResourceType::Image
        );
        for accept in [None, Some("*/*"), Some("application/json")] {
            assert_eq!(
                ResourceType::infer_without_metadata(accept, true),
                ResourceType::Other
            );
        }
        assert_eq!(ResourceType::infer(Some("worker")), ResourceType::Worker);
    }

    #[test]
    fn invalid_rule_fields_and_forged_lengths_are_refused() {
        for method in ["", "GET\r\nX: yes", "GET /", "méthod"] {
            let mut candidate = rule(Decision::Allow);
            candidate.method = Some(method.into());
            assert!(PolicySnapshot::new(vec![candidate]).is_err());
        }
        let mut zero = rule(Decision::Allow);
        zero.id = 0;
        assert!(PolicySnapshot::new(vec![zero]).is_err());
        let headers = vec![
            Header {
                name: "X-Test".into(),
                value: "a".into(),
            },
            Header {
                name: "x-test".into(),
                value: "b".into(),
            },
        ];
        assert!(PolicySnapshot::new(vec![rule(Decision::SetRequestHeaders(headers))]).is_err());
        for url in [
            "chrome-extension://invalid/noop.js",
            "chrome-extension://aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa:123/noop.js",
        ] {
            assert!(PolicySnapshot::new(vec![rule(Decision::Redirect(url.into()))]).is_err());
        }
        let bytes = PolicySnapshot::new(vec![rule(Decision::Allow)])
            .unwrap()
            .encode()
            .unwrap();
        for offset in [8, 16] {
            let mut forged = bytes.clone();
            forged[offset..offset + 4].copy_from_slice(&u32::MAX.to_le_bytes());
            assert!(PolicySnapshot::decode(&forged).is_err());
        }
        for offset in [bytes.len() - 2, bytes.len() - 1] {
            let mut forged = bytes.clone();
            forged[offset] = 255;
            assert!(PolicySnapshot::decode(&forged).is_err());
        }
    }
}
