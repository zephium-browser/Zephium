//! Immutable sealed-memfd script tables and bounded programmatic evaluation.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use crate::protocol::{
    MatchRule, Reader, WireError, Writer, MAX_BOOTSTRAP_BYTES, MAX_URI_BYTES, MAX_WORLD_BYTES,
};

pub const MAX_INSTALL_ID_BYTES: usize = 32;
pub const MAX_SCRIPT_ID_BYTES: usize = 256;
pub const MAX_SCRIPT_TABLE_BYTES: usize = 32 * 1024 * 1024;
pub const MAX_SCRIPT_ENTRIES: usize = 4096;
/// Matches the runtime's locale record ceiling without a reverse dependency.
pub const MAX_CATALOG_MESSAGES: usize = 16_384;

fn catalog_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'@'))
}

pub const MAX_SCRIPT_FILES: usize = 256;
pub const MAX_SCRIPT_SOURCE_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_INLINE_SCRIPT_SOURCE_BYTES: usize = 2 * 1024 * 1024;
pub const MAX_FRAME_SCRIPT_BYTES: usize = 32 * 1024 * 1024;
pub const MAX_ARGS_JSON_BYTES: usize = 1024 * 1024;
pub const MAX_RESULT_JSON_BYTES: usize = 1024 * 1024;
pub const MAX_INJECT_REQUEST_BYTES: usize =
    MAX_INLINE_SCRIPT_SOURCE_BYTES + MAX_ARGS_JSON_BYTES + 16 * 1024;
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
pub struct ScriptFile<S = String> {
    pub source: S,
    pub url: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ScriptEntry<S = String> {
    pub install_id: String,
    pub script_id: String,
    /// An empty name selects MAIN, which receives no binding bootstrap.
    pub world: String,
    /// Exact grant required even when execution selects MAIN.
    pub admission_world: String,
    pub run_at: RunAt,
    pub rule: MatchRule,
    pub files: Vec<ScriptFile<S>>,
    pub css: Vec<ScriptFile<S>>,
    pub bootstrap: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ScriptBootstrap<S = String> {
    pub install_id: String,
    pub source: S,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ScriptTable<S = String> {
    pub generation: u64,
    /// JSON catalogs passed as data to content bootstraps, shared per install.
    /// They count against the existing table and per-document byte budgets.
    pub catalogs: BTreeMap<String, Arc<str>>,
    pub bootstraps: BTreeMap<String, ScriptBootstrap<S>>,
    pub entries: Vec<ScriptEntry<S>>,
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
    pub request_id: Option<u64>,
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
    ScriptAtIdle = 4,
    FunctionAtIdle = 5,
}

impl InjectionKind {
    fn decode(tag: u8) -> Result<Self, WireError> {
        match tag {
            0 => Ok(Self::Script),
            1 => Ok(Self::Function),
            2 => Ok(Self::InsertCss),
            3 => Ok(Self::RemoveCss),
            4 => Ok(Self::ScriptAtIdle),
            5 => Ok(Self::FunctionAtIdle),
            _ => Err(WireError::Invalid),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceFd {
    pub index: u32,
    pub length: u32,
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
    pub source: Arc<str>,
    pub source_fd: Option<SourceFd>,
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
        if self.source.len() > MAX_INLINE_SCRIPT_SOURCE_BYTES
            || self
                .source_fd
                .as_ref()
                .is_some_and(|fd| fd.length as usize > MAX_SCRIPT_SOURCE_BYTES)
        {
            return Err(WireError::Oversized);
        }
        if self.source_fd.as_ref().is_some_and(|fd| {
            fd.length == 0
                || !self.source.is_empty()
                || !matches!(
                    self.kind,
                    InjectionKind::Script | InjectionKind::ScriptAtIdle
                )
        }) {
            return Err(WireError::Invalid);
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

impl<S: AsRef<str>> ScriptTable<S> {
    pub fn validate(&self) -> Result<(), WireError> {
        if self.generation == 0 {
            return Err(WireError::Invalid);
        }
        if self.entries.len() > MAX_SCRIPT_ENTRIES {
            return Err(WireError::Oversized);
        }
        let mut ids = BTreeSet::new();
        let mut worlds = std::collections::BTreeMap::new();
        if self.catalogs.len() > MAX_SCRIPT_ENTRIES {
            return Err(WireError::Oversized);
        }
        let mut bytes = 0usize;
        if self.bootstraps.len() > MAX_SCRIPT_ENTRIES {
            return Err(WireError::Oversized);
        }
        for (world, bootstrap) in &self.bootstraps {
            if !valid_world_name(world)
                || world.is_empty()
                || !valid_install_id(&bootstrap.install_id)
                || bootstrap.source.as_ref().is_empty()
                || bootstrap.source.as_ref().contains('\0')
            {
                return Err(WireError::Invalid);
            }
            if bootstrap.source.as_ref().len() > MAX_BOOTSTRAP_BYTES {
                return Err(WireError::Oversized);
            }
            worlds.insert(
                world,
                (bootstrap.install_id.as_str(), bootstrap.source.as_ref()),
            );
            bytes = bytes.saturating_add(bootstrap.source.as_ref().len());
            if bytes > MAX_SCRIPT_TABLE_BYTES {
                return Err(WireError::Oversized);
            }
        }

        for (install, catalog) in &self.catalogs {
            if !valid_install_id(install)
                || !(self
                    .bootstraps
                    .values()
                    .any(|bootstrap| bootstrap.install_id == *install)
                    || self.entries.iter().any(|entry| {
                        entry.install_id == *install
                            && !entry.world.is_empty()
                            && !entry.bootstrap.is_empty()
                    }))
            {
                return Err(WireError::Invalid);
            }
            json(catalog, MAX_FRAME_SCRIPT_BYTES, false)?;
            let value: serde_json::Value =
                serde_json::from_str(catalog).map_err(|_| WireError::Invalid)?;
            let messages = value.as_object().ok_or(WireError::Invalid)?;
            if messages.len() > MAX_CATALOG_MESSAGES {
                return Err(WireError::Oversized);
            }
            for (name, value) in messages {
                if !catalog_name(name)
                    || name.starts_with("@@")
                    || (!value.is_string()
                        && value
                            .get("message")
                            .and_then(serde_json::Value::as_str)
                            .is_none())
                {
                    return Err(WireError::Invalid);
                }
                if let Some(placeholders) = value.get("placeholders") {
                    for (name, value) in placeholders.as_object().ok_or(WireError::Invalid)? {
                        if !catalog_name(name)
                            || value
                                .get("content")
                                .and_then(serde_json::Value::as_str)
                                .is_none()
                        {
                            return Err(WireError::Invalid);
                        }
                    }
                }
            }
            bytes = bytes
                .saturating_add(install.len())
                .saturating_add(catalog.len());
            if bytes > MAX_SCRIPT_TABLE_BYTES {
                return Err(WireError::Oversized);
            }
        }
        for entry in &self.entries {
            if !valid_install_id(&entry.install_id)
                || entry.script_id.is_empty()
                || entry.script_id.len() > MAX_SCRIPT_ID_BYTES
                || entry.script_id.chars().any(char::is_control)
                || (!entry.world.is_empty() && !valid_world_name(&entry.world))
                || entry.admission_world.is_empty()
                || !valid_world_name(&entry.admission_world)
                || (!entry.world.is_empty() && entry.world != entry.admission_world)
                || (entry.world.is_empty() && !entry.bootstrap.is_empty())
                || (entry.files.is_empty() && entry.css.is_empty())
                || entry.bootstrap.contains('\0')
                || !ids.insert((&entry.install_id, &entry.script_id))
            {
                return Err(WireError::Invalid);
            }
            if !entry.world.is_empty() {
                let owner = (entry.install_id.as_str(), entry.bootstrap.as_str());
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
                if !valid_source_url(&file.url) || file.source.as_ref().contains('\0') {
                    return Err(WireError::Invalid);
                }
                if file.source.as_ref().len() > MAX_SCRIPT_SOURCE_BYTES {
                    return Err(WireError::Oversized);
                }
                bytes = bytes
                    .saturating_add(file.source.as_ref().len())
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
        w.count(self.bootstraps.len(), MAX_SCRIPT_ENTRIES)?;
        for (world, bootstrap) in &self.bootstraps {
            w.string(world, MAX_WORLD_BYTES)?;
            w.string(&bootstrap.install_id, MAX_INSTALL_ID_BYTES)?;
            w.string(bootstrap.source.as_ref(), MAX_BOOTSTRAP_BYTES)?;
        }
        w.count(self.catalogs.len(), MAX_SCRIPT_ENTRIES)?;
        for (install, catalog) in &self.catalogs {
            w.string(install, MAX_INSTALL_ID_BYTES)?;
            w.string(catalog, MAX_FRAME_SCRIPT_BYTES)?;
        }
        w.count(self.entries.len(), MAX_SCRIPT_ENTRIES)?;
        for entry in &self.entries {
            w.string(&entry.install_id, MAX_INSTALL_ID_BYTES)?;
            w.string(&entry.script_id, MAX_SCRIPT_ID_BYTES)?;
            w.string(&entry.world, MAX_WORLD_BYTES)?;
            w.string(&entry.admission_world, MAX_WORLD_BYTES)?;
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

    fn decode_sources<'a>(
        bytes: &'a [u8],
        source: impl Fn(&'a str) -> S,
    ) -> Result<Self, WireError> {
        let mut r = Reader::new(bytes, MAX_SCRIPT_TABLE_BYTES)?;
        r.header(48)?;
        let generation = r.u64()?;
        let n = r.count(MAX_SCRIPT_ENTRIES)?;
        let mut bootstraps = BTreeMap::new();
        for _ in 0..n {
            let world = r.string(MAX_WORLD_BYTES)?;
            let bootstrap = ScriptBootstrap {
                install_id: r.string(MAX_INSTALL_ID_BYTES)?,
                source: source(r.borrowed_string(MAX_BOOTSTRAP_BYTES)?),
            };
            if bootstraps.insert(world, bootstrap).is_some() {
                return Err(WireError::Invalid);
            }
        }
        let count = r.count(MAX_SCRIPT_ENTRIES)?;
        let mut catalogs = BTreeMap::new();
        for _ in 0..count {
            let install = r.string(MAX_INSTALL_ID_BYTES)?;
            let catalog = Arc::from(r.string(MAX_FRAME_SCRIPT_BYTES)?);
            if catalogs.insert(install, catalog).is_some() {
                return Err(WireError::Invalid);
            }
        }
        let n = r.count(MAX_SCRIPT_ENTRIES)?;
        let mut entries = Vec::new();
        for _ in 0..n {
            entries.push(ScriptEntry {
                install_id: r.string(MAX_INSTALL_ID_BYTES)?,
                script_id: r.string(MAX_SCRIPT_ID_BYTES)?,
                world: r.string(MAX_WORLD_BYTES)?,
                admission_world: r.string(MAX_WORLD_BYTES)?,
                run_at: RunAt::decode(r.u8()?)?,
                rule: r.rule()?,
                files: r.files(&source)?,
                css: r.files(&source)?,
                bootstrap: r.string(MAX_BOOTSTRAP_BYTES)?,
            });
        }
        r.end()?;
        let table = Self {
            generation,
            catalogs,
            bootstraps,
            entries,
        };
        table.validate()?;
        Ok(table)
    }
}

impl ScriptTable {
    pub fn decode(bytes: &[u8]) -> Result<Self, WireError> {
        Self::decode_sources(bytes, str::to_owned)
    }
}

impl<'a> ScriptTable<&'a str> {
    /// Sources borrow the validated snapshot; callers must retain its backing.
    pub fn decode_borrowed(bytes: &'a [u8]) -> Result<Self, WireError> {
        Self::decode_sources(bytes, |source| source)
    }
}

pub fn encode_script_table(table: &ScriptTable) -> Result<Vec<u8>, WireError> {
    table.encode()
}
pub fn decode_script_table(bytes: &[u8]) -> Result<ScriptTable, WireError> {
    ScriptTable::decode(bytes)
}

impl Writer {
    fn files<S: AsRef<str>>(&mut self, files: &[ScriptFile<S>]) -> Result<(), WireError> {
        self.count(files.len(), MAX_SCRIPT_FILES)?;
        for file in files {
            self.string(file.source.as_ref(), MAX_SCRIPT_SOURCE_BYTES)?;
            self.string(&file.url, MAX_URI_BYTES)?;
        }
        Ok(())
    }

    pub(crate) fn evaluation(&mut self, value: &Evaluation) -> Result<(), WireError> {
        self.boolean(value.request_id.is_some());
        if let Some(id) = value.request_id {
            self.u64(id);
        }
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
        self.string(&value.source, MAX_INLINE_SCRIPT_SOURCE_BYTES)?;
        self.boolean(value.source_fd.is_some());
        if let Some(fd) = &value.source_fd {
            self.u32(fd.index);
            self.u32(fd.length);
        }
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

impl<'a> Reader<'a> {
    fn files<S>(
        &mut self,
        source: &impl Fn(&'a str) -> S,
    ) -> Result<Vec<ScriptFile<S>>, WireError> {
        let n = self.count(MAX_SCRIPT_FILES)?;
        let mut files = Vec::new();
        for _ in 0..n {
            files.push(ScriptFile {
                source: source(self.borrowed_string(MAX_SCRIPT_SOURCE_BYTES)?),
                url: self.string(MAX_URI_BYTES)?,
            });
        }
        Ok(files)
    }

    pub(crate) fn evaluation(&mut self) -> Result<Evaluation, WireError> {
        Ok(Evaluation {
            request_id: if self.boolean()? {
                Some(self.u64()?)
            } else {
                None
            },
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
            source: self.string(MAX_INLINE_SCRIPT_SOURCE_BYTES)?.into(),
            source_fd: if self.boolean()? {
                Some(SourceFd {
                    index: self.u32()?,
                    length: self.u32()?,
                })
            } else {
                None
            },
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
            bootstraps: Default::default(),
            catalogs: Default::default(),
            generation: 9,
            entries: vec![ScriptEntry {
                install_id: "a".repeat(32),
                script_id: "_manifest_0".into(),
                world: "zephium-webext-isolated".into(),
                admission_world: "zephium-webext-isolated".into(),
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
            source_fd: None,
            source_url: format!("chrome-extension://{}/execute.js", "a".repeat(32)),
            args_json: "[41]".into(),
            timeout_ms: 1000,
        }
    }

    #[test]
    fn catalogs_are_owned_data_with_unchanged_source_and_table_budgets() {
        let mut table = table();
        let catalog: Arc<str> =
            serde_json::json!({"large":{"message":"x".repeat(MAX_BOOTSTRAP_BYTES + 1)}})
                .to_string()
                .into();
        table.catalogs.insert("a".repeat(32), catalog);
        assert_eq!(
            ScriptTable::decode(&table.encode().unwrap()).unwrap(),
            table
        );
        table
            .catalogs
            .insert("a".repeat(32), r#"{"compact":"translated string"}"#.into());
        assert!(table.validate().is_ok());
        table.catalogs.insert("b".repeat(32), "{}".into());
        assert_eq!(table.validate(), Err(WireError::Invalid));
        table.catalogs.remove(&"b".repeat(32));
        table.catalogs.insert("a".repeat(32), "[]".into());
        assert_eq!(table.validate(), Err(WireError::Invalid));
        table.catalogs.insert(
            "a".repeat(32),
            " ".repeat(MAX_FRAME_SCRIPT_BYTES + 1).into(),
        );
        assert_eq!(table.validate(), Err(WireError::Oversized));
        table
            .catalogs
            .insert("a".repeat(32), r#"{"bad":{"message":17}}"#.into());
        assert_eq!(table.validate(), Err(WireError::Invalid));
        table.catalogs.insert(
            "a".repeat(32),
            r#"{"bad":{"message":"text","placeholders":{"x":{"content":17}}}}"#.into(),
        );
        assert_eq!(table.validate(), Err(WireError::Invalid));
        let too_many: serde_json::Map<String, serde_json::Value> = (0..=MAX_CATALOG_MESSAGES)
            .map(|i| (format!("m{i}"), serde_json::json!({"message":"x"})))
            .collect();
        table.catalogs.insert(
            "a".repeat(32),
            serde_json::Value::Object(too_many).to_string().into(),
        );
        assert_eq!(table.validate(), Err(WireError::Oversized));
        table.catalogs.clear();
        table.entries[0].bootstrap = "x".repeat(MAX_BOOTSTRAP_BYTES + 1);
        assert_eq!(table.validate(), Err(WireError::Oversized));
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
    fn borrowed_sources_point_into_the_snapshot_and_keep_strict_validation() {
        let mut input = table();
        input.entries[0].files[0].source = "/* shared source */".repeat(4096);
        let bytes = input.encode().unwrap();
        let decoded = ScriptTable::decode_borrowed(&bytes).unwrap();
        let source = decoded.entries[0].files[0].source;
        assert_eq!(source, input.entries[0].files[0].source);
        let start = bytes.as_ptr() as usize;
        let pointer = source.as_ptr() as usize;
        assert!(pointer >= start && pointer + source.len() <= start + bytes.len());
        assert_eq!(decoded.encode().unwrap(), bytes);
        for at in [0, 4, bytes.len() - 1] {
            assert!(ScriptTable::decode_borrowed(&bytes[..at]).is_err());
        }
        let offset = pointer - start;
        let mut invalid = bytes.clone();
        invalid[offset] = 0;
        assert_eq!(
            ScriptTable::decode_borrowed(&invalid),
            Err(WireError::Invalid)
        );
        invalid[offset] = 0xff;
        assert_eq!(
            ScriptTable::decode_borrowed(&invalid),
            Err(WireError::Invalid)
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
    fn user_union_and_exact_main_admission_survive_the_wire() {
        let mut value = table();
        let entry = &mut value.entries[0];
        entry.world.clear();
        entry.bootstrap.clear();
        entry.admission_world = "zephium-webext-dedicated-user-world".into();
        entry.rule.matcher = ContentScriptMatcher::user_script(
            &["https://a.test/*"],
            &["https://b.test/private/*"],
            &["https://b.test/*"],
            &[],
        )
        .unwrap();
        let bytes = value.encode().unwrap();
        assert_eq!(ScriptTable::decode(&bytes).unwrap(), value);
        let mut old = bytes;
        old[3] = b'3';
        assert_eq!(ScriptTable::decode(&old), Err(WireError::Version));
        value.entries[0].admission_world.clear();
        assert_eq!(value.encode(), Err(WireError::Invalid));
    }

    #[test]
    fn programmatic_bootstraps_are_borrowed_and_match_static_world_ownership() {
        let mut value = table();
        let entry = &value.entries[0];
        value.bootstraps.insert(
            entry.world.clone(),
            ScriptBootstrap {
                install_id: entry.install_id.clone(),
                source: entry.bootstrap.clone(),
            },
        );
        let encoded = value.encode().unwrap();
        let borrowed = ScriptTable::decode_borrowed(&encoded).unwrap();
        let source = borrowed.bootstraps.values().next().unwrap().source;
        assert!((encoded.as_ptr_range()).contains(&source.as_ptr()));
        value
            .bootstraps
            .values_mut()
            .next()
            .unwrap()
            .source
            .push('x');
        assert_eq!(value.validate(), Err(WireError::Invalid));
        let event = Event::ScriptsReleased {
            generation: 2,
            table_generation: 7,
        };
        assert_eq!(decode_event(&encode_event(&event).unwrap()), Ok(event));
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
                InjectionKind::ScriptAtIdle,
                InjectionKind::FunctionAtIdle,
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
        assert_eq!(PROTOCOL_VERSION, 7);
    }

    #[test]
    fn source_descriptors_are_exclusive_bounded_and_only_for_scripts() {
        let mut request = request();
        request.kind = InjectionKind::Script;
        request.source = "".into();
        request.source_fd = Some(SourceFd {
            index: 0,
            length: 128 * 1024,
        });
        let message = crate::protocol::UiMessage::Inject(request.clone());
        let encoded = crate::protocol::encode_ui(&message).unwrap();
        assert!(encoded.len() < 1024);
        assert_eq!(
            crate::protocol::decode_ui(crate::protocol::INJECT_MESSAGE, &encoded).unwrap(),
            message
        );
        request.kind = InjectionKind::Function;
        assert_eq!(request.validate(), Err(WireError::Invalid));
        request.kind = InjectionKind::Script;
        request.source = "code".into();
        assert_eq!(request.validate(), Err(WireError::Invalid));
        request.source = "".into();
        request.source_fd.as_mut().unwrap().length = MAX_SCRIPT_SOURCE_BYTES as u32 + 1;
        assert_eq!(request.validate(), Err(WireError::Oversized));
    }

    #[test]
    fn evaluation_external_claims_and_all_delivery_reasons_are_strict() {
        let events = Event::Frames {
            generation: 7,
            events: vec![
                FrameEvent::Evaluated(Evaluation {
                    request_id: None,
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
