//! Immutable sealed-memfd script tables and bounded programmatic evaluation.

use std::collections::BTreeSet;

use crate::protocol::{
    MatchRule, Reader, WireError, Writer, MAX_BOOTSTRAP_BYTES, MAX_URI_BYTES, MAX_WORLD_BYTES,
};

pub const MAX_INSTALL_ID_BYTES: usize = 32;
pub const MAX_SCRIPT_ID_BYTES: usize = 256;
pub const MAX_SCRIPT_TABLE_BYTES: usize = 32 * 1024 * 1024;
pub const MAX_SCRIPT_ENTRIES: usize = 4096;
pub const MAX_SCRIPT_FILES: usize = 256;
pub const MAX_SCRIPT_SOURCE_BYTES: usize = 2 * 1024 * 1024;
pub const MAX_FRAME_SCRIPT_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_ARGS_JSON_BYTES: usize = 1024 * 1024;
pub const MAX_RESULT_JSON_BYTES: usize = 1024 * 1024;
pub const MAX_INJECT_REQUEST_BYTES: usize =
    MAX_SCRIPT_SOURCE_BYTES + MAX_ARGS_JSON_BYTES + 16 * 1024;
pub const MAX_INJECT_TIMEOUT_MS: u32 = 30_000;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[repr(u8)]
pub enum RunAt {
    DocumentStart = 0,
    DocumentEnd = 1,
    #[default]
    DocumentIdle = 2,
}

impl RunAt {
    fn decode(tag: u8) -> Result<Self, WireError> {
        match tag {
            0 => Ok(Self::DocumentStart),
            1 => Ok(Self::DocumentEnd),
            2 => Ok(Self::DocumentIdle),
            _ => Err(WireError::Invalid),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ScriptFile {
    pub source: String,
    pub url: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ScriptEntry {
    pub install_id: String,
    pub script_id: String,
    /// An empty name selects MAIN, which receives no binding bootstrap.
    pub world: String,
    pub run_at: RunAt,
    pub rule: MatchRule,
    pub files: Vec<ScriptFile>,
    pub css: Vec<ScriptFile>,
    pub bootstrap: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ScriptTable {
    pub generation: u64,
    pub entries: Vec<ScriptEntry>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ScriptUpdate {
    pub token: String,
    pub generation: u64,
    pub table_generation: u64,
    pub fd_index: u32,
    pub length: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Evaluation {
    pub install_id: String,
    pub script_id: String,
    pub world: String,
    pub frame_id: u64,
    pub document_seq: u64,
    pub table_generation: u64,
    /// `u16::MAX` identifies the once-per-document/world binding bootstrap.
    pub file_index: u16,
    pub success: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InjectionTarget {
    Frame { frame_id: u64, document_seq: u64 },
    AllFrames,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum InjectionKind {
    Script = 0,
    Function = 1,
    InsertCss = 2,
    RemoveCss = 3,
}

impl InjectionKind {
    fn decode(tag: u8) -> Result<Self, WireError> {
        match tag {
            0 => Ok(Self::Script),
            1 => Ok(Self::Function),
            2 => Ok(Self::InsertCss),
            3 => Ok(Self::RemoveCss),
            _ => Err(WireError::Invalid),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InjectRequest {
    pub install_id: String,
    pub token: String,
    pub generation: u64,
    pub request_id: u64,
    pub target: InjectionTarget,
    pub world: String,
    pub kind: InjectionKind,
    pub source: String,
    pub source_url: String,
    pub args_json: String,
    pub timeout_ms: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum InjectionStatus {
    Success = 0,
    StaleTarget = 1,
    Denied = 2,
    InvalidArguments = 3,
    EvaluationFailed = 4,
    TimedOut = 5,
    Oversized = 6,
    Unsupported = 7,
}

impl InjectionStatus {
    fn decode(tag: u8) -> Result<Self, WireError> {
        match tag {
            0 => Ok(Self::Success),
            1 => Ok(Self::StaleTarget),
            2 => Ok(Self::Denied),
            3 => Ok(Self::InvalidArguments),
            4 => Ok(Self::EvaluationFailed),
            5 => Ok(Self::TimedOut),
            6 => Ok(Self::Oversized),
            7 => Ok(Self::Unsupported),
            _ => Err(WireError::Invalid),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InjectionResult {
    pub generation: u64,
    pub request_id: u64,
    pub frame_id: u64,
    pub document_seq: u64,
    pub world: String,
    pub status: InjectionStatus,
    /// Valid JSON; failures use `null` and a reason code.
    pub result_json: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum DeliveryStatus {
    Delivered = 0,
    StaleTarget = 1,
    Denied = 2,
    NoReceiver = 3,
    EvaluationFailed = 4,
    RateLimited = 5,
}

impl DeliveryStatus {
    fn decode(tag: u8) -> Result<Self, WireError> {
        match tag {
            0 => Ok(Self::Delivered),
            1 => Ok(Self::StaleTarget),
            2 => Ok(Self::Denied),
            3 => Ok(Self::NoReceiver),
            4 => Ok(Self::EvaluationFailed),
            5 => Ok(Self::RateLimited),
            _ => Err(WireError::Invalid),
        }
    }
}

pub fn encode_delivery_result(status: DeliveryStatus) -> Result<Vec<u8>, WireError> {
    let mut w = Writer::new(32);
    w.u8(status as u8);
    Ok(w.0)
}

pub fn decode_delivery_result(bytes: &[u8]) -> Result<DeliveryStatus, WireError> {
    let mut r = Reader::new(bytes, 6)?;
    r.header(32)?;
    let status = DeliveryStatus::decode(r.u8()?)?;
    r.end()?;
    Ok(status)
}

pub fn valid_install_id(id: &str) -> bool {
    id.len() == MAX_INSTALL_ID_BYTES
        && id
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

pub fn valid_world_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_WORLD_BYTES
        && (name.starts_with("zephium-principal-extension-") || name.starts_with("zephium-webext-"))
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

fn valid_source_url(source: &str) -> bool {
    if source.is_empty() || source.len() > MAX_URI_BYTES || source.chars().any(char::is_control) {
        return false;
    }
    let Ok(url) = url::Url::parse(source) else {
        return false;
    };
    let Some(host) = url.host_str() else {
        return false;
    };
    url.scheme() == "chrome-extension"
        && url.port().is_none()
        && url.username().is_empty()
        && url.password().is_none()
        && url.fragment().is_none()
        && host.len() == 32
        && host.bytes().all(|byte| (b'a'..=b'p').contains(&byte))
}

fn json(value: &str, max: usize, array: bool) -> Result<(), WireError> {
    if value.len() > max {
        return Err(WireError::Oversized);
    }
    let parsed: serde_json::Value = serde_json::from_str(value).map_err(|_| WireError::Invalid)?;
    if array && !parsed.is_array() {
        return Err(WireError::Invalid);
    }
    let mut pending = vec![(&parsed, 0usize)];
    while let Some((value, depth)) = pending.pop() {
        if depth > 64 {
            return Err(WireError::Oversized);
        }
        match value {
            serde_json::Value::Array(items) => pending.extend(items.iter().map(|v| (v, depth + 1))),
            serde_json::Value::Object(items) => {
                pending.extend(items.values().map(|v| (v, depth + 1)))
            }
            _ => {}
        }
    }
    Ok(())
}

impl InjectRequest {
    pub fn validate(&self) -> Result<(), WireError> {
        if self.source.len() > MAX_SCRIPT_SOURCE_BYTES {
            return Err(WireError::Oversized);
        }
        if !valid_install_id(&self.install_id)
            || !(1..=MAX_INJECT_TIMEOUT_MS).contains(&self.timeout_ms)
            || (!self.world.is_empty() && !valid_world_name(&self.world))
            || !valid_source_url(&self.source_url)
            || self.source.contains('\0')
        {
            return Err(WireError::Invalid);
        }
        json(&self.args_json, MAX_ARGS_JSON_BYTES, true)
    }
}

impl ScriptTable {
    pub fn validate(&self) -> Result<(), WireError> {
        if self.generation == 0 {
            return Err(WireError::Invalid);
        }
        if self.entries.len() > MAX_SCRIPT_ENTRIES {
            return Err(WireError::Oversized);
        }
        let mut ids = BTreeSet::new();
        let mut worlds = std::collections::BTreeMap::new();
        let mut bytes = 0usize;
        for entry in &self.entries {
            if !valid_install_id(&entry.install_id)
                || entry.script_id.is_empty()
                || entry.script_id.len() > MAX_SCRIPT_ID_BYTES
                || entry.script_id.chars().any(char::is_control)
                || (!entry.world.is_empty() && !valid_world_name(&entry.world))
                || (entry.world.is_empty() && !entry.bootstrap.is_empty())
                || (entry.files.is_empty() && entry.css.is_empty())
                || entry.bootstrap.contains('\0')
                || !ids.insert((&entry.install_id, &entry.script_id))
            {
                return Err(WireError::Invalid);
            }
            if !entry.world.is_empty() {
                let owner = (&entry.install_id, &entry.bootstrap);
                if worlds
                    .insert(&entry.world, owner)
                    .is_some_and(|previous| previous != owner)
                {
                    return Err(WireError::Invalid);
                }
            }
            if entry.bootstrap.len() > MAX_BOOTSTRAP_BYTES
                || entry.files.len().saturating_add(entry.css.len()) > MAX_SCRIPT_FILES
            {
                return Err(WireError::Oversized);
            }
            bytes = bytes.saturating_add(entry.bootstrap.len());
            for file in entry.files.iter().chain(&entry.css) {
                if !valid_source_url(&file.url) || file.source.contains('\0') {
                    return Err(WireError::Invalid);
                }
                if file.source.len() > MAX_SCRIPT_SOURCE_BYTES {
                    return Err(WireError::Oversized);
                }
                bytes = bytes
                    .saturating_add(file.source.len())
                    .saturating_add(file.url.len());
            }
            if bytes > MAX_SCRIPT_TABLE_BYTES {
                return Err(WireError::Oversized);
            }
        }
        Ok(())
    }

    pub fn encode(&self) -> Result<Vec<u8>, WireError> {
        self.validate()?;
        let mut w = Writer::new(48);
        w.u64(self.generation);
        w.count(self.entries.len(), MAX_SCRIPT_ENTRIES)?;
        for entry in &self.entries {
            w.string(&entry.install_id, MAX_INSTALL_ID_BYTES)?;
            w.string(&entry.script_id, MAX_SCRIPT_ID_BYTES)?;
            w.string(&entry.world, MAX_WORLD_BYTES)?;
            w.u8(entry.run_at as u8);
            w.rule(&entry.rule)?;
            w.files(&entry.files)?;
            w.files(&entry.css)?;
            w.string(&entry.bootstrap, MAX_BOOTSTRAP_BYTES)?;
        }
        if w.0.len() > MAX_SCRIPT_TABLE_BYTES {
            return Err(WireError::Oversized);
        }
        Ok(w.0)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, WireError> {
        let mut r = Reader::new(bytes, MAX_SCRIPT_TABLE_BYTES)?;
        r.header(48)?;
        let generation = r.u64()?;
        let n = r.count(MAX_SCRIPT_ENTRIES)?;
        let mut entries = Vec::new();
        for _ in 0..n {
            entries.push(ScriptEntry {
                install_id: r.string(MAX_INSTALL_ID_BYTES)?,
                script_id: r.string(MAX_SCRIPT_ID_BYTES)?,
                world: r.string(MAX_WORLD_BYTES)?,
                run_at: RunAt::decode(r.u8()?)?,
                rule: r.rule()?,
                files: r.files()?,
                css: r.files()?,
                bootstrap: r.string(MAX_BOOTSTRAP_BYTES)?,
            });
        }
        r.end()?;
        let table = Self {
            generation,
            entries,
        };
        table.validate()?;
        Ok(table)
    }
}

pub fn encode_script_table(table: &ScriptTable) -> Result<Vec<u8>, WireError> {
    table.encode()
}
pub fn decode_script_table(bytes: &[u8]) -> Result<ScriptTable, WireError> {
    ScriptTable::decode(bytes)
}

impl Writer {
    fn files(&mut self, files: &[ScriptFile]) -> Result<(), WireError> {
        self.count(files.len(), MAX_SCRIPT_FILES)?;
        for file in files {
            self.string(&file.source, MAX_SCRIPT_SOURCE_BYTES)?;
            self.string(&file.url, MAX_URI_BYTES)?;
        }
        Ok(())
    }

    pub(crate) fn evaluation(&mut self, value: &Evaluation) -> Result<(), WireError> {
        self.string(&value.install_id, MAX_INSTALL_ID_BYTES)?;
        self.string(&value.script_id, MAX_SCRIPT_ID_BYTES)?;
        self.string(&value.world, MAX_WORLD_BYTES)?;
        self.u64(value.frame_id);
        self.u64(value.document_seq);
        self.u64(value.table_generation);
        self.u16(value.file_index);
        self.boolean(value.success);
        Ok(())
    }

    pub(crate) fn inject(&mut self, value: &InjectRequest) -> Result<(), WireError> {
        value.validate()?;
        self.string(&value.install_id, MAX_INSTALL_ID_BYTES)?;
        self.string(&value.token, crate::protocol::MAX_TOKEN_BYTES)?;
        self.u64(value.generation);
        self.u64(value.request_id);
        match value.target {
            InjectionTarget::Frame {
                frame_id,
                document_seq,
            } => {
                self.u8(0);
                self.u64(frame_id);
                self.u64(document_seq);
            }
            InjectionTarget::AllFrames => self.u8(1),
        }
        self.string(&value.world, MAX_WORLD_BYTES)?;
        self.u8(value.kind as u8);
        self.string(&value.source, MAX_SCRIPT_SOURCE_BYTES)?;
        self.string(&value.source_url, MAX_URI_BYTES)?;
        self.string(&value.args_json, MAX_ARGS_JSON_BYTES)?;
        self.u32(value.timeout_ms);
        Ok(())
    }

    pub(crate) fn injection_result(&mut self, value: &InjectionResult) -> Result<(), WireError> {
        json(&value.result_json, MAX_RESULT_JSON_BYTES, false)?;
        self.u64(value.generation);
        self.u64(value.request_id);
        self.u64(value.frame_id);
        self.u64(value.document_seq);
        self.string(&value.world, MAX_WORLD_BYTES)?;
        self.u8(value.status as u8);
        self.string(&value.result_json, MAX_RESULT_JSON_BYTES)?;
        Ok(())
    }
}

impl Reader<'_> {
    fn files(&mut self) -> Result<Vec<ScriptFile>, WireError> {
        let n = self.count(MAX_SCRIPT_FILES)?;
        let mut files = Vec::new();
        for _ in 0..n {
            files.push(ScriptFile {
                source: self.string(MAX_SCRIPT_SOURCE_BYTES)?,
                url: self.string(MAX_URI_BYTES)?,
            });
        }
        Ok(files)
    }

    pub(crate) fn evaluation(&mut self) -> Result<Evaluation, WireError> {
        Ok(Evaluation {
            install_id: self.string(MAX_INSTALL_ID_BYTES)?,
            script_id: self.string(MAX_SCRIPT_ID_BYTES)?,
            world: self.string(MAX_WORLD_BYTES)?,
            frame_id: self.u64()?,
            document_seq: self.u64()?,
            table_generation: self.u64()?,
            file_index: self.u16()?,
            success: self.boolean()?,
        })
    }

    pub(crate) fn inject(&mut self) -> Result<InjectRequest, WireError> {
        let install_id = self.string(MAX_INSTALL_ID_BYTES)?;
        let token = self.string(crate::protocol::MAX_TOKEN_BYTES)?;
        let generation = self.u64()?;
        let request_id = self.u64()?;
        let target = match self.u8()? {
            0 => InjectionTarget::Frame {
                frame_id: self.u64()?,
                document_seq: self.u64()?,
            },
            1 => InjectionTarget::AllFrames,
            _ => return Err(WireError::Invalid),
        };
        let value = InjectRequest {
            install_id,
            token,
            generation,
            request_id,
            target,
            world: self.string(MAX_WORLD_BYTES)?,
            kind: InjectionKind::decode(self.u8()?)?,
            source: self.string(MAX_SCRIPT_SOURCE_BYTES)?,
            source_url: self.string(MAX_URI_BYTES)?,
            args_json: self.string(MAX_ARGS_JSON_BYTES)?,
            timeout_ms: self.u32()?,
        };
        value.validate()?;
        Ok(value)
    }

    pub(crate) fn injection_result(
        &mut self,
        generation: u64,
    ) -> Result<InjectionResult, WireError> {
        let value = InjectionResult {
            generation,
            request_id: self.u64()?,
            frame_id: self.u64()?,
            document_seq: self.u64()?,
            world: self.string(MAX_WORLD_BYTES)?,
            status: InjectionStatus::decode(self.u8()?)?,
            result_json: self.string(MAX_RESULT_JSON_BYTES)?,
        };
        json(&value.result_json, MAX_RESULT_JSON_BYTES, false)?;
        Ok(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{
        decode_event, decode_ui, encode_event, encode_ui, ContentScriptMatcher, Event, FrameEvent,
        MatchOptions, UiMessage, INJECT_MESSAGE, PROTOCOL_VERSION, SCRIPTS_MESSAGE,
    };

    fn table() -> ScriptTable {
        ScriptTable {
            generation: 9,
            entries: vec![ScriptEntry {
                install_id: "a".repeat(32),
                script_id: "_manifest_0".into(),
                world: "zephium-webext-isolated".into(),
                run_at: RunAt::DocumentStart,
                rule: MatchRule {
                    matcher: ContentScriptMatcher::new(
                        &["<all_urls>"],
                        &["https://private.test/*"],
                        &["*/news/*"],
                        &["*draft*"],
                        MatchOptions {
                            match_about_blank: true,
                            match_origin_as_fallback: false,
                        },
                    )
                    .expect("matcher"),
                    all_frames: true,
                    file_access: false,
                },
                files: vec![ScriptFile {
                    source: "\"use strict\"; let shared = 1;".into(),
                    url: format!("chrome-extension://{}/first.js", "a".repeat(32)),
                }],
                css: vec![ScriptFile {
                    source: "body {color: red}".into(),
                    url: format!("chrome-extension://{}/first.css", "a".repeat(32)),
                }],
                bootstrap: "void 0".into(),
            }],
        }
    }

    fn request() -> InjectRequest {
        InjectRequest {
            install_id: "a".repeat(32),
            token: "token".into(),
            generation: 7,
            request_id: 8,
            target: InjectionTarget::Frame {
                frame_id: 42,
                document_seq: 3,
            },
            world: "zephium-webext-isolated".into(),
            kind: InjectionKind::Function,
            source: "async value => value + 1".into(),
            source_url: format!("chrome-extension://{}/execute.js", "a".repeat(32)),
            args_json: "[41]".into(),
            timeout_ms: 1000,
        }
    }

    #[test]
    fn script_table_roundtrip_preserves_files_options_and_matcher_behavior() {
        let table = table();
        let bytes = table.encode().expect("table");
        let decoded = ScriptTable::decode(&bytes).expect("decode");
        assert_eq!(decoded, table);
        let frame = crate::protocol::FrameInfo {
            uri: "https://new-origin.test/news/1".into(),
            is_main: true,
            ..Default::default()
        };
        assert!(decoded.entries[0].rule.matches(&frame));
        assert_eq!(
            decoded.entries[0].files[0].source,
            "\"use strict\"; let shared = 1;"
        );
    }

    #[test]
    fn table_decoder_rejects_every_truncation_trailing_bytes_and_old_version() {
        let bytes = table().encode().expect("table");
        for at in 0..bytes.len() {
            assert!(ScriptTable::decode(&bytes[..at]).is_err(), "{at}");
        }
        let mut trailing = bytes.clone();
        trailing.push(0);
        assert_eq!(ScriptTable::decode(&trailing), Err(WireError::Trailing));
        let mut old = bytes.clone();
        old[3] = b'1';
        assert_eq!(ScriptTable::decode(&old), Err(WireError::Version));
        let mut bad_count = bytes.clone();
        bad_count[13..17].copy_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(ScriptTable::decode(&bad_count), Err(WireError::Oversized));
        // Mutating any single byte must remain a safe decoding operation.
        for at in 0..bytes.len() {
            let mut changed = bytes.clone();
            changed[at] ^= 0xff;
            let _ = ScriptTable::decode(&changed);
        }
    }

    #[test]
    fn table_refuses_unbounded_sources_duplicates_and_ambiguous_world_bootstraps() {
        let mut oversized = table();
        oversized.entries[0].files[0].source = "x".repeat(MAX_SCRIPT_SOURCE_BYTES + 1);
        assert_eq!(oversized.encode(), Err(WireError::Oversized));
        let mut duplicate = table();
        duplicate.entries.push(duplicate.entries[0].clone());
        assert_eq!(duplicate.encode(), Err(WireError::Invalid));
        let mut world = table();
        let mut second = world.entries[0].clone();
        second.script_id = "second".into();
        second.bootstrap = "different".into();
        world.entries.push(second);
        assert_eq!(world.encode(), Err(WireError::Invalid));
        let mut files = table();
        let file = files.entries[0].files[0].clone();
        files.entries[0].files = vec![file; MAX_SCRIPT_FILES];
        assert_eq!(files.encode(), Err(WireError::Oversized));
        let mut main = table();
        main.entries[0].world.clear();
        assert_eq!(main.encode(), Err(WireError::Invalid));
        main.entries[0].bootstrap.clear();
        assert!(main.encode().is_ok());
        let mut source = table();
        source.entries[0].files[0].url = "https://remote.test/code.js".into();
        assert_eq!(source.encode(), Err(WireError::Invalid));
        let mut zero_generation = table();
        zero_generation.generation = 0;
        assert_eq!(zero_generation.encode(), Err(WireError::Invalid));
    }

    #[test]
    fn script_publication_programmatic_targets_and_bounded_json_round_trip() {
        let script_message = UiMessage::Scripts(ScriptUpdate {
            token: "token".into(),
            generation: 1,
            table_generation: 9,
            fd_index: 0,
            length: table().encode().expect("table").len() as u32,
        });
        assert_eq!(
            decode_ui(
                SCRIPTS_MESSAGE,
                &encode_ui(&script_message).expect("message")
            ),
            Ok(script_message)
        );
        for target in [
            InjectionTarget::AllFrames,
            InjectionTarget::Frame {
                frame_id: 42,
                document_seq: 3,
            },
        ] {
            for kind in [
                InjectionKind::Script,
                InjectionKind::Function,
                InjectionKind::InsertCss,
                InjectionKind::RemoveCss,
            ] {
                let mut value = request();
                value.target = target;
                value.kind = kind;
                let message = UiMessage::Inject(value);
                let bytes = encode_ui(&message).expect("encode");
                assert_eq!(decode_ui(INJECT_MESSAGE, &bytes), Ok(message));
                for at in 0..bytes.len() {
                    assert!(decode_ui(INJECT_MESSAGE, &bytes[..at]).is_err());
                }
            }
        }
        let mut value = request();
        value.args_json = "{}".into();
        assert_eq!(
            encode_ui(&UiMessage::Inject(value)),
            Err(WireError::Invalid)
        );
        let mut value = request();
        value.timeout_ms = MAX_INJECT_TIMEOUT_MS + 1;
        assert_eq!(
            encode_ui(&UiMessage::Inject(value)),
            Err(WireError::Invalid)
        );
        let mut value = request();
        value.args_json = format!("{}0{}", "[".repeat(66), "]".repeat(66));
        assert_eq!(
            encode_ui(&UiMessage::Inject(value)),
            Err(WireError::Oversized)
        );
        let mut value = request();
        value.args_json = format!("[\"{}\"]", "a".repeat(MAX_ARGS_JSON_BYTES));
        assert_eq!(
            encode_ui(&UiMessage::Inject(value)),
            Err(WireError::Oversized)
        );
        assert_eq!(PROTOCOL_VERSION, 3);
    }

    #[test]
    fn evaluation_external_claims_and_all_delivery_reasons_are_strict() {
        let events = Event::Frames {
            generation: 7,
            events: vec![
                FrameEvent::Evaluated(Evaluation {
                    install_id: "a".repeat(32),
                    script_id: "_manifest_0".into(),
                    world: "zephium-webext-isolated".into(),
                    frame_id: 42,
                    document_seq: 3,
                    table_generation: 9,
                    file_index: 0,
                    success: true,
                }),
                FrameEvent::ExternalPage {
                    install_id: "a".repeat(32),
                    frame_id: 42,
                    document_seq: 3,
                },
            ],
        };
        assert_eq!(
            decode_event(&encode_event(&events).expect("events")),
            Ok(events)
        );
        for status in [
            DeliveryStatus::Delivered,
            DeliveryStatus::StaleTarget,
            DeliveryStatus::Denied,
            DeliveryStatus::NoReceiver,
            DeliveryStatus::EvaluationFailed,
            DeliveryStatus::RateLimited,
        ] {
            let bytes = encode_delivery_result(status).expect("status");
            assert_eq!(decode_delivery_result(&bytes), Ok(status));
        }
        let mut invalid = encode_delivery_result(DeliveryStatus::Delivered).expect("status");
        invalid[5] = 255;
        assert_eq!(decode_delivery_result(&invalid), Err(WireError::Invalid));
        for status in [
            InjectionStatus::Success,
            InjectionStatus::StaleTarget,
            InjectionStatus::Denied,
            InjectionStatus::InvalidArguments,
            InjectionStatus::EvaluationFailed,
            InjectionStatus::TimedOut,
            InjectionStatus::Oversized,
            InjectionStatus::Unsupported,
        ] {
            let event = Event::Injection(InjectionResult {
                generation: 7,
                request_id: 8,
                frame_id: 42,
                document_seq: 3,
                world: "zephium-webext-isolated".into(),
                status,
                result_json: if status == InjectionStatus::Success {
                    "{\"answer\":42}".into()
                } else {
                    "null".into()
                },
            });
            assert_eq!(
                decode_event(&encode_event(&event).expect("result")),
                Ok(event)
            );
        }
        let bad = Event::Injection(InjectionResult {
            generation: 7,
            request_id: 8,
            frame_id: 42,
            document_seq: 3,
            world: String::new(),
            status: InjectionStatus::Success,
            result_json: "undefined".into(),
        });
        assert_eq!(encode_event(&bad), Err(WireError::Invalid));
    }
}
