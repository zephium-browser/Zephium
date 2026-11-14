//! Bounded, little-endian protocol carried in UserMessage `ay` parameters.

#![forbid(unsafe_code)]

pub use crate::matching::*;
pub use crate::scripts::*;
use zephium_webext_matcher::injection::{MAX_MATCH_PATTERNS_PER_SET, MAX_MATCH_PATTERN_BYTES};
use zephium_webext_matcher::{MAX_GLOBS_PER_SCRIPT, MAX_GLOB_BYTES};

pub const PROTOCOL_VERSION: u32 = 2;
pub const ATTACH_MESSAGE: &str = "zephium-webext-attach";
pub const DELIVER_MESSAGE: &str = "zephium-webext-deliver";
pub const POLICY_MESSAGE: &str = "zephium-webext-policy";
pub const DETACH_MESSAGE: &str = "zephium-webext-detach";
pub const SCRIPTS_MESSAGE: &str = "zephium-webext-scripts";
pub const INJECT_MESSAGE: &str = "zephium-webext-inject";
pub const EVENT_MESSAGE: &str = "zephium-webext-event";
pub const POST_FUNCTION: &str = "__zephiumPost";
pub const REGISTER_FUNCTION: &str = "__zephiumRegisterReceiver";
pub const MAX_MESSAGE_BYTES: usize = 32 * 1024 * 1024;
pub const MAX_UI_BYTES: usize = MAX_MESSAGE_BYTES + 64 * 1024;
pub const MAX_ATTACH_BYTES: usize = 1024 * 1024;
pub const MAX_TOKEN_BYTES: usize = 128;
pub const MAX_URI_BYTES: usize = 8192;
pub const MAX_WORLD_BYTES: usize = 128;
pub const MAX_WORLDS: usize = 16;
pub const MAX_ORIGINS: usize = 64;
pub const MAX_BOOTSTRAP_BYTES: usize = 256 * 1024;
pub const MAX_BATCH_EVENTS: usize = 64;
pub const MAX_FORM_FIELDS: usize = 64;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum PageRole {
    #[default]
    OrdinaryTab,
    ExtensionOwned,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Limits {
    pub max_message_bytes: u32,
    pub frame_inflight_bytes: u32,
    pub page_inflight_bytes: u32,
    pub frame_outstanding: u16,
    pub page_outstanding: u16,
    pub frame_messages_per_second: u32,
    pub page_messages_per_second: u32,
    pub frame_burst: u16,
    pub page_burst: u16,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_message_bytes: 8 * 1024 * 1024,
            frame_inflight_bytes: 64 * 1024 * 1024,
            page_inflight_bytes: 128 * 1024 * 1024,
            frame_outstanding: 256,
            page_outstanding: 2048,
            frame_messages_per_second: 2000,
            page_messages_per_second: 16000,
            frame_burst: 500,
            page_burst: 4000,
        }
    }
}

impl Limits {
    pub fn validate(&self, role: PageRole) -> bool {
        let max_message = match role {
            PageRole::OrdinaryTab => 8 * 1024 * 1024,
            PageRole::ExtensionOwned => MAX_MESSAGE_BYTES as u32,
        };
        self.max_message_bytes > 0
            && self.max_message_bytes <= max_message
            && self.frame_inflight_bytes >= self.max_message_bytes
            && self.frame_inflight_bytes <= 64 * 1024 * 1024
            && self.page_inflight_bytes >= self.frame_inflight_bytes
            && self.page_inflight_bytes <= 128 * 1024 * 1024
            && (1..=256).contains(&self.frame_outstanding)
            && (self.frame_outstanding..=2048).contains(&self.page_outstanding)
            && (1..=2000).contains(&self.frame_messages_per_second)
            && (1..=16000).contains(&self.page_messages_per_second)
            && (1..=500).contains(&self.frame_burst)
            && (1..=4000).contains(&self.page_burst)
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct WorldRule {
    pub install_id: String,
    pub name: String,
    pub matches: Vec<MatchRule>,
    pub bootstrap: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Attach {
    pub protocol_version: u32,
    pub token: String,
    pub generation: u64,
    pub role: PageRole,
    pub limits: Limits,
    pub worlds: Vec<WorldRule>,
    pub external: Vec<ExternalRule>,
    pub extension_origins: Vec<String>,
    pub extension_bootstrap: String,
    pub observe_requests: bool,
    pub observe_forms: bool,
}

impl Default for Attach {
    fn default() -> Self {
        Self {
            protocol_version: PROTOCOL_VERSION,
            token: String::new(),
            generation: 0,
            role: PageRole::default(),
            limits: Limits::default(),
            worlds: Vec::new(),
            external: Vec::new(),
            extension_origins: Vec::new(),
            extension_bootstrap: String::new(),
            observe_requests: false,
            observe_forms: false,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Deliver {
    pub external_install: Option<String>,
    pub token: String,
    pub generation: u64,
    pub frame_id: u64,
    pub document_seq: u64,
    pub world: String,
    pub bytes: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Detach {
    pub token: String,
    pub generation: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PolicyUpdate {
    pub token: String,
    pub generation: u64,
    pub policy_generation: u64,
    pub fd_index: u32,
    pub length: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum UiMessage {
    Attach(Attach),
    Deliver(Deliver),
    Detach(Detach),
    Policy(PolicyUpdate),
    Scripts(ScriptUpdate),
    Inject(InjectRequest),
}

impl UiMessage {
    pub fn name(&self) -> &'static str {
        match self {
            Self::Attach(_) => ATTACH_MESSAGE,
            Self::Deliver(_) => DELIVER_MESSAGE,
            Self::Detach(_) => DETACH_MESSAGE,
            Self::Policy(_) => POLICY_MESSAGE,
            Self::Scripts(_) => SCRIPTS_MESSAGE,
            Self::Inject(_) => INJECT_MESSAGE,
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct FrameInfo {
    pub frame_id: u64,
    pub document_seq: u64,
    pub uri: String,
    pub is_main: bool,
    pub parent_frame_id: Option<u64>,
    pub creator_uri: Option<String>,
    pub opaque: bool,
    pub table_generation: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FrameEvent {
    Created(FrameInfo),
    Committed(FrameInfo),
    Destroyed {
        frame_id: u64,
        document_seq: u64,
    },
    Reset,
    Evaluated(Evaluation),
    ExternalPage {
        install_id: String,
        frame_id: u64,
        document_seq: u64,
    },
    /// The document's trusted `DOMContentLoaded` reached its window.
    ContentLoaded {
        frame_id: u64,
        document_seq: u64,
    },
    /// The document's trusted window `load` event fired.
    Completed {
        frame_id: u64,
        document_seq: u64,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Post {
    pub external_install: Option<String>,
    pub generation: u64,
    pub post_id: u64,
    pub frame_id: u64,
    pub document_seq: u64,
    pub world: String,
    pub uri: String,
    pub bytes: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RequestEvent {
    pub url: String,
    pub method: String,
    pub resource_type: String,
    pub frame_id: u64,
    pub document_url: String,
    pub is_main_frame: bool,
    pub decision: String,
    pub matched: bool,
    pub matched_rule_id: Option<u32>,
    pub initiator: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FormEvent {
    pub generation: u64,
    pub frame_id: u64,
    pub document_seq: u64,
    pub fields: Vec<(String, String)>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Event {
    Attached {
        generation: u64,
    },
    Frames {
        generation: u64,
        events: Vec<FrameEvent>,
    },
    Post(Post),
    Requests {
        generation: u64,
        observations: Vec<RequestEvent>,
    },
    Form(FormEvent),
    Injection(InjectionResult),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WireError {
    Oversized,
    Truncated,
    Invalid,
    Version,
    Trailing,
}

impl std::fmt::Display for WireError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "invalid web-process wire message: {self:?}")
    }
}

impl std::error::Error for WireError {}

pub(crate) struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    pub(crate) fn new(bytes: &'a [u8], max: usize) -> Result<Self, WireError> {
        if bytes.len() > max {
            return Err(WireError::Oversized);
        }
        Ok(Self { bytes, at: 0 })
    }

    pub(crate) fn take(&mut self, len: usize) -> Result<&'a [u8], WireError> {
        let end = self.at.checked_add(len).ok_or(WireError::Oversized)?;
        let result = self.bytes.get(self.at..end).ok_or(WireError::Truncated)?;
        self.at = end;
        Ok(result)
    }

    pub(crate) fn u8(&mut self) -> Result<u8, WireError> {
        Ok(self.take(1)?[0])
    }
    pub(crate) fn u16(&mut self) -> Result<u16, WireError> {
        Ok(u16::from_le_bytes(
            self.take(2)?.try_into().map_err(|_| WireError::Truncated)?,
        ))
    }
    pub(crate) fn u32(&mut self) -> Result<u32, WireError> {
        Ok(u32::from_le_bytes(
            self.take(4)?.try_into().map_err(|_| WireError::Truncated)?,
        ))
    }
    pub(crate) fn u64(&mut self) -> Result<u64, WireError> {
        Ok(u64::from_le_bytes(
            self.take(8)?.try_into().map_err(|_| WireError::Truncated)?,
        ))
    }
    pub(crate) fn boolean(&mut self) -> Result<bool, WireError> {
        match self.u8()? {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(WireError::Invalid),
        }
    }
    pub(crate) fn count(&mut self, max: usize) -> Result<usize, WireError> {
        let n = self.u32()? as usize;
        if n > max {
            Err(WireError::Oversized)
        } else {
            Ok(n)
        }
    }
    pub(crate) fn blob(&mut self, max: usize) -> Result<&'a [u8], WireError> {
        let n = self.count(max)?;
        self.take(n)
    }
    pub(crate) fn string(&mut self, max: usize) -> Result<String, WireError> {
        let value = std::str::from_utf8(self.blob(max)?).map_err(|_| WireError::Invalid)?;
        if value.contains('\0') {
            return Err(WireError::Invalid);
        }
        Ok(value.to_owned())
    }
    pub(crate) fn end(self) -> Result<(), WireError> {
        if self.at == self.bytes.len() {
            Ok(())
        } else {
            Err(WireError::Trailing)
        }
    }
    pub(crate) fn header(&mut self, kind: u8) -> Result<(), WireError> {
        if self.take(4)? != b"ZWP2" {
            return Err(WireError::Version);
        }
        if self.u8()? != kind {
            return Err(WireError::Invalid);
        }
        Ok(())
    }
    pub(crate) fn limits(&mut self) -> Result<Limits, WireError> {
        Ok(Limits {
            max_message_bytes: self.u32()?,
            frame_inflight_bytes: self.u32()?,
            page_inflight_bytes: self.u32()?,
            frame_outstanding: self.u16()?,
            page_outstanding: self.u16()?,
            frame_messages_per_second: self.u32()?,
            page_messages_per_second: self.u32()?,
            frame_burst: self.u16()?,
            page_burst: self.u16()?,
        })
    }
    pub(crate) fn strings(&mut self, max: usize, length: usize) -> Result<Vec<String>, WireError> {
        let count = self.count(max)?;
        let mut items = Vec::new();
        for _ in 0..count {
            items.push(self.string(length)?);
        }
        Ok(items)
    }
    pub(crate) fn frame(&mut self) -> Result<FrameInfo, WireError> {
        Ok(FrameInfo {
            frame_id: self.u64()?,
            document_seq: self.u64()?,
            uri: self.string(MAX_URI_BYTES)?,
            is_main: self.boolean()?,
            parent_frame_id: if self.boolean()? {
                Some(self.u64()?)
            } else {
                None
            },
            creator_uri: if self.boolean()? {
                Some(self.string(MAX_URI_BYTES)?)
            } else {
                None
            },
            opaque: self.boolean()?,
            table_generation: self.u64()?,
        })
    }
}

pub(crate) struct Writer(pub(crate) Vec<u8>);

impl Writer {
    pub(crate) fn new(kind: u8) -> Self {
        let mut bytes = b"ZWP2".to_vec();
        bytes.push(kind);
        Self(bytes)
    }
    pub(crate) fn u8(&mut self, v: u8) {
        self.0.push(v);
    }
    pub(crate) fn u16(&mut self, v: u16) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    pub(crate) fn u32(&mut self, v: u32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    pub(crate) fn u64(&mut self, v: u64) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    pub(crate) fn boolean(&mut self, v: bool) {
        self.u8(u8::from(v));
    }
    pub(crate) fn count(&mut self, n: usize, max: usize) -> Result<(), WireError> {
        if n > max {
            return Err(WireError::Oversized);
        }
        self.u32(u32::try_from(n).map_err(|_| WireError::Oversized)?);
        Ok(())
    }
    pub(crate) fn blob(&mut self, v: &[u8], max: usize) -> Result<(), WireError> {
        self.count(v.len(), max)?;
        if self.0.len().saturating_add(v.len()) > MAX_UI_BYTES {
            return Err(WireError::Oversized);
        }
        self.0.extend_from_slice(v);
        Ok(())
    }
    pub(crate) fn string(&mut self, v: &str, max: usize) -> Result<(), WireError> {
        if v.contains('\0') {
            return Err(WireError::Invalid);
        }
        self.blob(v.as_bytes(), max)
    }
    pub(crate) fn strings(
        &mut self,
        v: &[String],
        max: usize,
        length: usize,
    ) -> Result<(), WireError> {
        self.count(v.len(), max)?;
        for item in v {
            self.string(item, length)?;
        }
        Ok(())
    }
    pub(crate) fn limits(&mut self, v: &Limits) {
        self.u32(v.max_message_bytes);
        self.u32(v.frame_inflight_bytes);
        self.u32(v.page_inflight_bytes);
        self.u16(v.frame_outstanding);
        self.u16(v.page_outstanding);
        self.u32(v.frame_messages_per_second);
        self.u32(v.page_messages_per_second);
        self.u16(v.frame_burst);
        self.u16(v.page_burst);
    }
    pub(crate) fn frame(&mut self, v: &FrameInfo) -> Result<(), WireError> {
        self.u64(v.frame_id);
        self.u64(v.document_seq);
        self.string(&v.uri, MAX_URI_BYTES)?;
        self.boolean(v.is_main);
        self.boolean(v.parent_frame_id.is_some());
        if let Some(parent) = v.parent_frame_id {
            self.u64(parent);
        }
        self.boolean(v.creator_uri.is_some());
        if let Some(creator) = &v.creator_uri {
            self.string(creator, MAX_URI_BYTES)?;
        }
        self.boolean(v.opaque);
        self.u64(v.table_generation);
        Ok(())
    }
}

impl Reader<'_> {
    pub(crate) fn rule(&mut self) -> Result<MatchRule, WireError> {
        let matches = self.strings(MAX_MATCH_PATTERNS_PER_SET, MAX_MATCH_PATTERN_BYTES)?;
        let excluded = self.strings(MAX_MATCH_PATTERNS_PER_SET, MAX_MATCH_PATTERN_BYTES)?;
        if matches.len().saturating_add(excluded.len()) > MAX_MATCH_PATTERNS_PER_SET {
            return Err(WireError::Oversized);
        }
        let included_globs = self.strings(MAX_GLOBS_PER_SCRIPT, MAX_GLOB_BYTES)?;
        let excluded_globs = self.strings(MAX_GLOBS_PER_SCRIPT, MAX_GLOB_BYTES)?;
        if included_globs.len().saturating_add(excluded_globs.len()) > MAX_GLOBS_PER_SCRIPT {
            return Err(WireError::Oversized);
        }
        let options = MatchOptions {
            match_about_blank: self.boolean()?,
            match_origin_as_fallback: self.boolean()?,
        };
        let all_frames = self.boolean()?;
        let file_access = self.boolean()?;
        let matcher = ContentScriptMatcher::new(
            &matches,
            &excluded,
            &included_globs,
            &excluded_globs,
            options,
        )
        .map_err(|_| WireError::Invalid)?;
        Ok(MatchRule {
            matcher,
            all_frames,
            file_access,
        })
    }
}

impl Writer {
    pub(crate) fn rule(&mut self, rule: &MatchRule) -> Result<(), WireError> {
        let set = rule.matcher.match_set();
        if set.includes().len().saturating_add(set.excludes().len()) > MAX_MATCH_PATTERNS_PER_SET
            || rule
                .matcher
                .include_globs()
                .len()
                .saturating_add(rule.matcher.exclude_globs().len())
                > MAX_GLOBS_PER_SCRIPT
        {
            return Err(WireError::Oversized);
        }
        self.count(set.includes().len(), MAX_MATCH_PATTERNS_PER_SET)?;
        for item in set.includes() {
            self.string(item.as_str(), MAX_MATCH_PATTERN_BYTES)?;
        }
        self.count(set.excludes().len(), MAX_MATCH_PATTERNS_PER_SET)?;
        for item in set.excludes() {
            self.string(item.as_str(), MAX_MATCH_PATTERN_BYTES)?;
        }
        self.count(rule.matcher.include_globs().len(), MAX_GLOBS_PER_SCRIPT)?;
        for item in rule.matcher.include_globs() {
            self.string(item.as_str(), MAX_GLOB_BYTES)?;
        }
        self.count(rule.matcher.exclude_globs().len(), MAX_GLOBS_PER_SCRIPT)?;
        for item in rule.matcher.exclude_globs() {
            self.string(item.as_str(), MAX_GLOB_BYTES)?;
        }
        self.boolean(set.options().match_about_blank);
        self.boolean(set.options().match_origin_as_fallback);
        self.boolean(rule.all_frames);
        self.boolean(rule.file_access);
        Ok(())
    }
}

pub fn encode_ui(message: &UiMessage) -> Result<Vec<u8>, WireError> {
    let mut w = Writer::new(match message {
        UiMessage::Attach(_) => 1,
        UiMessage::Deliver(_) => 2,
        UiMessage::Policy(_) => 3,
        UiMessage::Detach(_) => 4,
        UiMessage::Scripts(_) => 5,
        UiMessage::Inject(_) => 6,
    });
    match message {
        UiMessage::Attach(v) => {
            if v.protocol_version != PROTOCOL_VERSION {
                return Err(WireError::Version);
            }
            w.u32(v.protocol_version);
            w.string(&v.token, MAX_TOKEN_BYTES)?;
            w.u64(v.generation);
            w.u8(match v.role {
                PageRole::OrdinaryTab => 0,
                PageRole::ExtensionOwned => 1,
            });
            w.limits(&v.limits);
            w.count(v.worlds.len(), MAX_WORLDS)?;
            for world in &v.worlds {
                w.string(&world.install_id, MAX_INSTALL_ID_BYTES)?;
                w.string(&world.name, MAX_WORLD_BYTES)?;
                w.count(world.matches.len(), MAX_ORIGINS)?;
                for rule in &world.matches {
                    w.rule(rule)?;
                }
                w.string(&world.bootstrap, MAX_BOOTSTRAP_BYTES)?;
            }
            w.count(v.external.len(), MAX_ORIGINS)?;
            for rule in &v.external {
                w.string(&rule.install_id, MAX_INSTALL_ID_BYTES)?;
                w.count(rule.matches.len(), MAX_ORIGINS)?;
                for pattern in &rule.matches {
                    w.rule(pattern)?;
                }
                w.string(&rule.bootstrap, MAX_BOOTSTRAP_BYTES)?;
            }
            w.strings(&v.extension_origins, MAX_ORIGINS, MAX_URI_BYTES)?;
            w.string(&v.extension_bootstrap, MAX_BOOTSTRAP_BYTES)?;
            w.boolean(v.observe_requests);
            w.boolean(v.observe_forms);
            if w.0.len() > MAX_ATTACH_BYTES {
                return Err(WireError::Oversized);
            }
        }
        UiMessage::Deliver(v) => {
            w.string(&v.token, MAX_TOKEN_BYTES)?;
            w.u64(v.generation);
            w.u64(v.frame_id);
            w.u64(v.document_seq);
            w.string(&v.world, MAX_WORLD_BYTES)?;
            w.boolean(v.external_install.is_some());
            if let Some(install) = &v.external_install {
                w.string(install, MAX_INSTALL_ID_BYTES)?;
            }
            w.blob(&v.bytes, MAX_MESSAGE_BYTES)?;
        }
        UiMessage::Policy(v) => {
            w.string(&v.token, MAX_TOKEN_BYTES)?;
            w.u64(v.generation);
            w.u64(v.policy_generation);
            w.u32(v.fd_index);
            w.u32(v.length);
        }
        UiMessage::Detach(v) => {
            w.string(&v.token, MAX_TOKEN_BYTES)?;
            w.u64(v.generation);
        }
        UiMessage::Scripts(v) => {
            w.string(&v.token, MAX_TOKEN_BYTES)?;
            w.u64(v.generation);
            w.u64(v.table_generation);
            w.u32(v.fd_index);
            if v.length as usize > MAX_SCRIPT_TABLE_BYTES {
                return Err(WireError::Oversized);
            }
            w.u32(v.length);
        }
        UiMessage::Inject(v) => w.inject(v)?,
    }
    Ok(w.0)
}

pub fn decode_ui(name: &str, bytes: &[u8]) -> Result<UiMessage, WireError> {
    let (tag, max) = match name {
        ATTACH_MESSAGE => (1, MAX_ATTACH_BYTES),
        DELIVER_MESSAGE => (2, MAX_UI_BYTES),
        POLICY_MESSAGE => (3, 1024),
        DETACH_MESSAGE => (4, 1024),
        SCRIPTS_MESSAGE => (5, 1024),
        INJECT_MESSAGE => (6, MAX_INJECT_REQUEST_BYTES),
        _ => return Err(WireError::Invalid),
    };
    let mut r = Reader::new(bytes, max)?;
    r.header(tag)?;
    let message = match tag {
        1 => {
            let protocol_version = r.u32()?;
            if protocol_version != PROTOCOL_VERSION {
                return Err(WireError::Version);
            }
            let token = r.string(MAX_TOKEN_BYTES)?;
            let generation = r.u64()?;
            let role = match r.u8()? {
                0 => PageRole::OrdinaryTab,
                1 => PageRole::ExtensionOwned,
                _ => return Err(WireError::Invalid),
            };
            let limits = r.limits()?;
            let count = r.count(MAX_WORLDS)?;
            let mut worlds = Vec::new();
            for _ in 0..count {
                let install_id = r.string(MAX_INSTALL_ID_BYTES)?;
                let name = r.string(MAX_WORLD_BYTES)?;
                let n = r.count(MAX_ORIGINS)?;
                let mut matches = Vec::new();
                for _ in 0..n {
                    matches.push(r.rule()?);
                }
                worlds.push(WorldRule {
                    install_id,
                    name,
                    matches,
                    bootstrap: r.string(MAX_BOOTSTRAP_BYTES)?,
                });
            }
            let n = r.count(MAX_ORIGINS)?;
            let mut external = Vec::new();
            for _ in 0..n {
                let install_id = r.string(MAX_INSTALL_ID_BYTES)?;
                let n = r.count(MAX_ORIGINS)?;
                let mut matches = Vec::new();
                for _ in 0..n {
                    matches.push(r.rule()?);
                }
                external.push(ExternalRule {
                    install_id,
                    matches,
                    bootstrap: r.string(MAX_BOOTSTRAP_BYTES)?,
                });
            }
            UiMessage::Attach(Attach {
                protocol_version,
                token,
                generation,
                role,
                limits,
                worlds,
                external,
                extension_origins: r.strings(MAX_ORIGINS, MAX_URI_BYTES)?,
                extension_bootstrap: r.string(MAX_BOOTSTRAP_BYTES)?,
                observe_requests: r.boolean()?,
                observe_forms: r.boolean()?,
            })
        }
        2 => UiMessage::Deliver(Deliver {
            token: r.string(MAX_TOKEN_BYTES)?,
            generation: r.u64()?,
            frame_id: r.u64()?,
            document_seq: r.u64()?,
            world: r.string(MAX_WORLD_BYTES)?,
            external_install: if r.boolean()? {
                Some(r.string(MAX_INSTALL_ID_BYTES)?)
            } else {
                None
            },
            bytes: r.blob(MAX_MESSAGE_BYTES)?.to_vec(),
        }),
        3 => UiMessage::Policy(PolicyUpdate {
            token: r.string(MAX_TOKEN_BYTES)?,
            generation: r.u64()?,
            policy_generation: r.u64()?,
            fd_index: r.u32()?,
            length: r.u32()?,
        }),
        4 => UiMessage::Detach(Detach {
            token: r.string(MAX_TOKEN_BYTES)?,
            generation: r.u64()?,
        }),
        5 => {
            let v = ScriptUpdate {
                token: r.string(MAX_TOKEN_BYTES)?,
                generation: r.u64()?,
                table_generation: r.u64()?,
                fd_index: r.u32()?,
                length: r.u32()?,
            };
            if v.length as usize > MAX_SCRIPT_TABLE_BYTES {
                return Err(WireError::Oversized);
            }
            UiMessage::Scripts(v)
        }
        6 => UiMessage::Inject(r.inject()?),
        _ => return Err(WireError::Invalid),
    };
    r.end()?;
    Ok(message)
}

pub fn encode_event(event: &Event) -> Result<Vec<u8>, WireError> {
    let mut w = Writer::new(match event {
        Event::Attached { .. } => 16,
        Event::Frames { .. } => 17,
        Event::Post(_) => 18,
        Event::Requests { .. } => 19,
        Event::Form(_) => 20,
        Event::Injection(_) => 21,
    });
    match event {
        Event::Attached { generation } => w.u64(*generation),
        Event::Frames { generation, events } => {
            w.u64(*generation);
            w.count(events.len(), MAX_BATCH_EVENTS)?;
            for event in events {
                match event {
                    FrameEvent::Created(v) => {
                        w.u8(1);
                        w.frame(v)?;
                    }
                    FrameEvent::Committed(v) => {
                        w.u8(2);
                        w.frame(v)?;
                    }
                    FrameEvent::Destroyed {
                        frame_id,
                        document_seq,
                    } => {
                        w.u8(3);
                        w.u64(*frame_id);
                        w.u64(*document_seq);
                    }
                    FrameEvent::Reset => w.u8(4),
                    FrameEvent::Evaluated(v) => {
                        w.u8(5);
                        w.evaluation(v)?;
                    }
                    FrameEvent::ExternalPage {
                        install_id,
                        frame_id,
                        document_seq,
                    } => {
                        w.u8(6);
                        w.string(install_id, MAX_INSTALL_ID_BYTES)?;
                        w.u64(*frame_id);
                        w.u64(*document_seq);
                    }
                    FrameEvent::ContentLoaded {
                        frame_id,
                        document_seq,
                    } => {
                        w.u8(7);
                        w.u64(*frame_id);
                        w.u64(*document_seq);
                    }
                    FrameEvent::Completed {
                        frame_id,
                        document_seq,
                    } => {
                        w.u8(8);
                        w.u64(*frame_id);
                        w.u64(*document_seq);
                    }
                }
            }
        }
        Event::Post(v) => {
            w.u64(v.generation);
            w.u64(v.post_id);
            w.u64(v.frame_id);
            w.u64(v.document_seq);
            w.string(&v.world, MAX_WORLD_BYTES)?;
            w.boolean(v.external_install.is_some());
            if let Some(install) = &v.external_install {
                w.string(install, MAX_INSTALL_ID_BYTES)?;
            }
            w.string(&v.uri, MAX_URI_BYTES)?;
            w.blob(&v.bytes, MAX_MESSAGE_BYTES)?;
        }
        Event::Requests {
            generation,
            observations,
        } => {
            w.u64(*generation);
            w.count(observations.len(), MAX_BATCH_EVENTS)?;
            for v in observations {
                w.string(&v.url, MAX_URI_BYTES)?;
                w.string(&v.method, 32)?;
                w.string(&v.resource_type, 64)?;
                w.u64(v.frame_id);
                w.string(&v.document_url, MAX_URI_BYTES)?;
                w.boolean(v.is_main_frame);
                w.string(&v.decision, 32)?;
                w.boolean(v.matched);
                w.boolean(v.matched_rule_id.is_some());
                if let Some(id) = v.matched_rule_id {
                    w.u32(id);
                }
                w.boolean(v.initiator.is_some());
                if let Some(initiator) = &v.initiator {
                    w.string(initiator, MAX_URI_BYTES)?;
                }
            }
        }
        Event::Form(v) => {
            w.u64(v.generation);
            w.u64(v.frame_id);
            w.u64(v.document_seq);
            w.count(v.fields.len(), MAX_FORM_FIELDS)?;
            for (name, value) in &v.fields {
                w.string(name, 1024)?;
                w.string(value, 8192)?;
            }
        }
        Event::Injection(v) => w.injection_result(v)?,
    }
    Ok(w.0)
}

pub fn encode_post_bytes(
    generation: u64,
    post_id: u64,
    frame: &FrameInfo,
    world: &str,
    payload: &[u8],
) -> Result<Vec<u8>, WireError> {
    encode_post_bytes_for_install(generation, post_id, frame, world, payload, None)
}

pub fn encode_post_bytes_for_install(
    generation: u64,
    post_id: u64,
    frame: &FrameInfo,
    world: &str,
    payload: &[u8],
    external_install: Option<&str>,
) -> Result<Vec<u8>, WireError> {
    let mut w = Writer::new(18);
    w.u64(generation);
    w.u64(post_id);
    w.u64(frame.frame_id);
    w.u64(frame.document_seq);
    w.string(world, MAX_WORLD_BYTES)?;
    w.boolean(external_install.is_some());
    if let Some(install) = external_install {
        w.string(install, MAX_INSTALL_ID_BYTES)?;
    }
    w.string(&frame.uri, MAX_URI_BYTES)?;
    w.blob(payload, MAX_MESSAGE_BYTES)?;
    Ok(w.0)
}

pub fn decode_event(bytes: &[u8]) -> Result<Event, WireError> {
    let mut r = Reader::new(bytes, MAX_UI_BYTES)?;
    if r.take(4)? != b"ZWP2" {
        return Err(WireError::Version);
    }
    let tag = r.u8()?;
    let generation = r.u64()?;
    let event = match tag {
        16 => Event::Attached { generation },
        17 => {
            let n = r.count(MAX_BATCH_EVENTS)?;
            let mut events = Vec::new();
            for _ in 0..n {
                events.push(match r.u8()? {
                    1 => FrameEvent::Created(r.frame()?),
                    2 => FrameEvent::Committed(r.frame()?),
                    3 => FrameEvent::Destroyed {
                        frame_id: r.u64()?,
                        document_seq: r.u64()?,
                    },
                    4 => FrameEvent::Reset,
                    5 => FrameEvent::Evaluated(r.evaluation()?),
                    6 => FrameEvent::ExternalPage {
                        install_id: r.string(MAX_INSTALL_ID_BYTES)?,
                        frame_id: r.u64()?,
                        document_seq: r.u64()?,
                    },
                    7 => FrameEvent::ContentLoaded {
                        frame_id: r.u64()?,
                        document_seq: r.u64()?,
                    },
                    8 => FrameEvent::Completed {
                        frame_id: r.u64()?,
                        document_seq: r.u64()?,
                    },
                    _ => return Err(WireError::Invalid),
                });
            }
            Event::Frames { generation, events }
        }
        18 => Event::Post(Post {
            generation,
            post_id: r.u64()?,
            frame_id: r.u64()?,
            document_seq: r.u64()?,
            world: r.string(MAX_WORLD_BYTES)?,
            external_install: if r.boolean()? {
                Some(r.string(MAX_INSTALL_ID_BYTES)?)
            } else {
                None
            },
            uri: r.string(MAX_URI_BYTES)?,
            bytes: r.blob(MAX_MESSAGE_BYTES)?.to_vec(),
        }),
        19 => {
            let n = r.count(MAX_BATCH_EVENTS)?;
            let mut observations = Vec::new();
            for _ in 0..n {
                observations.push(RequestEvent {
                    url: r.string(MAX_URI_BYTES)?,
                    method: r.string(32)?,
                    resource_type: r.string(64)?,
                    frame_id: r.u64()?,
                    document_url: r.string(MAX_URI_BYTES)?,
                    is_main_frame: r.boolean()?,
                    decision: r.string(32)?,
                    matched: r.boolean()?,
                    matched_rule_id: if r.boolean()? { Some(r.u32()?) } else { None },
                    initiator: if r.boolean()? {
                        Some(r.string(MAX_URI_BYTES)?)
                    } else {
                        None
                    },
                });
            }
            Event::Requests {
                generation,
                observations,
            }
        }
        20 => {
            let frame_id = r.u64()?;
            let document_seq = r.u64()?;
            let n = r.count(MAX_FORM_FIELDS)?;
            let mut fields = Vec::new();
            for _ in 0..n {
                fields.push((r.string(1024)?, r.string(8192)?));
            }
            Event::Form(FormEvent {
                generation,
                frame_id,
                document_seq,
                fields,
            })
        }
        21 => Event::Injection(r.injection_result(generation)?),
        _ => return Err(WireError::Invalid),
    };
    r.end()?;
    Ok(event)
}

/// The exact origin string attachment grants are matched against: the
/// ASCII serialization for HTTP(S), `chrome-extension://<id>` for a
/// canonical 32-letter extension id, and nothing for other documents.
pub fn document_origin(uri: &str) -> Option<String> {
    if uri.len() > MAX_URI_BYTES || uri.chars().any(|ch| ch.is_control()) {
        return None;
    }
    let url = url::Url::parse(uri).ok()?;
    match url.scheme() {
        "http" | "https" => Some(url.origin().ascii_serialization()),
        "chrome-extension" => {
            let host = url.host_str()?;
            if url.port().is_some()
                || !url.username().is_empty()
                || url.password().is_some()
                || host.len() != 32
                || !host.bytes().all(|byte| (b'a'..=b'p').contains(&byte))
            {
                return None;
            }
            Some(format!("chrome-extension://{host}"))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attach_and_delivery_round_trip() {
        let messages = [
            UiMessage::Attach(Attach {
                token: "process-token".into(),
                generation: 1,
                worlds: vec![WorldRule {
                    install_id: "a".repeat(32),
                    name: "isolated".into(),
                    matches: vec![MatchRule::for_origin("https://example.com").expect("rule")],
                    bootstrap: "void 0".into(),
                }],
                ..Attach::default()
            }),
            UiMessage::Deliver(Deliver {
                external_install: None,
                token: "process-token".into(),
                generation: 1,
                frame_id: 42,
                document_seq: 7,
                world: "isolated".into(),
                bytes: vec![0, 1, 255],
            }),
        ];
        for message in messages {
            let bytes = encode_ui(&message).expect("encode");
            assert_eq!(decode_ui(message.name(), &bytes), Ok(message));
        }
        let message = UiMessage::Policy(PolicyUpdate {
            token: "process-token".into(),
            generation: 1,
            policy_generation: 2,
            fd_index: 0,
            length: 4096,
        });
        assert_eq!(
            decode_ui(POLICY_MESSAGE, &encode_ui(&message).expect("encode")),
            Ok(message)
        );
    }

    #[test]
    fn every_truncation_and_trailing_bytes_are_rejected() {
        let msg = UiMessage::Detach(Detach {
            token: "process-token".into(),
            generation: 3,
        });
        let bytes = encode_ui(&msg).expect("encode");
        for at in 0..bytes.len() {
            assert!(decode_ui(DETACH_MESSAGE, &bytes[..at]).is_err());
        }
        let mut bytes = bytes;
        bytes.push(0);
        assert_eq!(decode_ui(DETACH_MESSAGE, &bytes), Err(WireError::Trailing));
    }

    #[test]
    fn hostile_lengths_types_and_utf8_are_refused() {
        let mut bytes = b"ZWP2\x04".to_vec();
        bytes.extend_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(decode_ui(DETACH_MESSAGE, &bytes), Err(WireError::Oversized));
        let mut bytes = b"ZWP2\x04".to_vec();
        bytes.extend_from_slice(&1u32.to_le_bytes());
        bytes.push(255);
        assert_eq!(decode_ui(DETACH_MESSAGE, &bytes), Err(WireError::Invalid));
        assert_eq!(decode_ui("unknown", &[]), Err(WireError::Invalid));
        let bytes = encode_ui(&UiMessage::Detach(Detach {
            token: "x".into(),
            generation: 1,
        }))
        .expect("encode");
        assert_eq!(decode_ui(POLICY_MESSAGE, &bytes), Err(WireError::Invalid));
    }

    #[test]
    fn events_round_trip_and_batches_are_bounded() {
        let event = Event::Frames {
            generation: 1,
            events: vec![
                FrameEvent::Created(FrameInfo {
                    frame_id: 9,
                    document_seq: 2,
                    uri: "https://example.com".into(),
                    is_main: true,
                    ..FrameInfo::default()
                }),
                FrameEvent::Destroyed {
                    frame_id: 9,
                    document_seq: 1,
                },
                FrameEvent::ContentLoaded {
                    frame_id: 9,
                    document_seq: 2,
                },
                FrameEvent::Completed {
                    frame_id: 9,
                    document_seq: 2,
                },
                FrameEvent::Reset,
            ],
        };
        assert_eq!(
            decode_event(&encode_event(&event).expect("encode")),
            Ok(event)
        );
        let event = Event::Frames {
            generation: 1,
            events: vec![FrameEvent::Reset; MAX_BATCH_EVENTS + 1],
        };
        assert_eq!(encode_event(&event), Err(WireError::Oversized));
    }

    #[test]
    fn strict_boolean_and_role_tags_are_refused() {
        let message = UiMessage::Attach(Attach {
            token: "process-token".into(),
            generation: 1,
            ..Attach::default()
        });
        let mut bytes = encode_ui(&message).expect("encode");
        let end = bytes.len() - 1;
        bytes[end] = 2;
        assert_eq!(decode_ui(ATTACH_MESSAGE, &bytes), Err(WireError::Invalid));
        let mut bytes = encode_ui(&message).expect("encode");
        bytes[21 + "process-token".len()] = 2;
        assert_eq!(decode_ui(ATTACH_MESSAGE, &bytes), Err(WireError::Invalid));
    }

    #[test]
    fn borrowed_post_encoder_matches_owned_event_and_telemetry_round_trips() {
        let frame = FrameInfo {
            frame_id: 9,
            document_seq: 2,
            uri: "https://example.com".into(),
            is_main: true,
            ..FrameInfo::default()
        };
        let post = Post {
            external_install: None,
            generation: 1,
            post_id: 3,
            frame_id: frame.frame_id,
            document_seq: frame.document_seq,
            world: "zephium-webext-isolated".into(),
            uri: frame.uri.clone(),
            bytes: vec![0, 128, 255],
        };
        assert_eq!(
            encode_post_bytes(1, 3, &frame, &post.world, &post.bytes),
            encode_event(&Event::Post(post.clone()))
        );
        let events = [
            Event::Post(post),
            Event::Attached { generation: 1 },
            Event::Requests {
                generation: 1,
                observations: vec![RequestEvent {
                    url: "https://example.com/x".into(),
                    method: "GET".into(),
                    resource_type: "fetch".into(),
                    frame_id: 0,
                    document_url: String::new(),
                    is_main_frame: false,
                    decision: "cancel".into(),
                    matched: true,
                    matched_rule_id: Some(7),
                    initiator: None,
                }],
            },
            Event::Form(FormEvent {
                generation: 1,
                frame_id: 9,
                document_seq: 2,
                fields: vec![("field".into(), "value".into())],
            }),
        ];
        for event in events {
            assert_eq!(
                decode_event(&encode_event(&event).expect("encode")),
                Ok(event)
            );
        }
    }

    #[test]
    fn caps_cannot_exceed_the_module_floor() {
        let mut limits = Limits::default();
        assert!(limits.validate(PageRole::OrdinaryTab));
        limits.max_message_bytes = MAX_MESSAGE_BYTES as u32;
        assert!(!limits.validate(PageRole::OrdinaryTab));
        assert!(limits.validate(PageRole::ExtensionOwned));
        limits.frame_burst = 0;
        assert!(!limits.validate(PageRole::ExtensionOwned));
    }
    #[test]
    fn version_one_headers_and_embedded_versions_are_refused() {
        let message = UiMessage::Attach(Attach {
            token: "token".into(),
            generation: 1,
            ..Attach::default()
        });
        let bytes = encode_ui(&message).expect("message");
        let mut old_header = bytes.clone();
        old_header[3] = b'1';
        assert_eq!(
            decode_ui(ATTACH_MESSAGE, &old_header),
            Err(WireError::Version)
        );
        let mut old_field = bytes;
        old_field[5..9].copy_from_slice(&1u32.to_le_bytes());
        assert_eq!(
            decode_ui(ATTACH_MESSAGE, &old_field),
            Err(WireError::Version)
        );
        let old = Attach {
            protocol_version: 1,
            ..Attach::default()
        };
        assert_eq!(encode_ui(&UiMessage::Attach(old)), Err(WireError::Version));
    }
    #[test]
    fn complete_frame_facts_and_external_install_routing_round_trip() {
        let frame = FrameInfo {
            frame_id: 42,
            document_seq: 7,
            uri: "about:srcdoc".into(),
            is_main: false,
            parent_frame_id: Some(9),
            creator_uri: Some("https://a.test/path".into()),
            opaque: true,
            table_generation: 12,
        };
        let facts = Event::Frames {
            generation: 3,
            events: vec![
                FrameEvent::Created(frame.clone()),
                FrameEvent::Committed(frame.clone()),
            ],
        };
        assert_eq!(
            decode_event(&encode_event(&facts).expect("facts")),
            Ok(facts)
        );
        let post = Post {
            external_install: Some("a".repeat(32)),
            generation: 3,
            post_id: 4,
            frame_id: frame.frame_id,
            document_seq: frame.document_seq,
            world: String::new(),
            uri: frame.uri.clone(),
            bytes: vec![0, 1, 2],
        };
        let bytes = encode_post_bytes_for_install(
            3,
            4,
            &frame,
            "",
            &post.bytes,
            post.external_install.as_deref(),
        )
        .expect("post");
        assert_eq!(decode_event(&bytes), Ok(Event::Post(post.clone())));
        assert_eq!(encode_event(&Event::Post(post)), Ok(bytes));
        let deliver = UiMessage::Deliver(Deliver {
            external_install: Some("a".repeat(32)),
            token: "token".into(),
            generation: 3,
            frame_id: 42,
            document_seq: 7,
            world: String::new(),
            bytes: vec![0, 1, 2],
        });
        assert_eq!(
            decode_ui(DELIVER_MESSAGE, &encode_ui(&deliver).expect("delivery")),
            Ok(deliver)
        );
    }
}
