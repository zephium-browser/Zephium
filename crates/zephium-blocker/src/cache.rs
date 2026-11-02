//! Optional, fail-safe persistence for compiled blocker artifacts.

mod file_identity;

use std::borrow::Cow;
#[cfg(feature = "webkit")]
use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
#[cfg(feature = "webkit")]
use std::io::{Seek, SeekFrom};
use std::path::{Path, PathBuf};
#[cfg(feature = "webkit")]
use std::sync::{Arc, Mutex};

use sha2::{Digest, Sha256};
use thiserror::Error;
use zephium_core::blocker::{
    ContentRuleCoverage, ContentRuleDigest, ContentRules, ContentRulesPayload,
};
#[cfg(feature = "webkit")]
use zephium_core::blocker::{DeclarativeRuleFormat, DeclarativeRuleText};

#[cfg(feature = "runtime")]
use crate::compiler::prepare_and_validate_runtime_engine;
use crate::cosmetics::PreparedCosmetics;
#[cfg(feature = "runtime")]
use crate::rules::CachedRuntimeRules;
use crate::rules::CompiledRules;
#[cfg(feature = "webkit")]
use crate::webkit::ResourceType as WebKitResourceType;
use crate::{
    CompileLimits, CompileTarget, ADBLOCK_ENGINE_VERSION, POLICY_FORMAT_VERSION,
    WEBKIT_ARTIFACT_FORMAT_VERSION,
};

const CACHE_MAGIC: &[u8; 8] = b"ZPHBLK01";
const CACHE_FORMAT_VERSION: u32 = 3;
const CACHE_KEY_DOMAIN: &[u8] = b"zephium-compiled-blocker-cache-key";
#[cfg(feature = "webkit")]
const WEBKIT_DIGEST_DOMAIN: &[u8] = b"zephium-webkit-content-rules";
const LOCK_FILE: &str = "cache.lock";
const CURRENT_FILE: &str = "current.bin";
const PREVIOUS_FILE: &str = "previous.bin";
const STAGE_FILE: &str = "stage.bin";
const MAX_CACHE_DIRECTORY_ENTRIES: usize = 4;
const MAX_RUNTIME_PAYLOAD_BYTES: usize = 64 * 1024 * 1024;
const MAX_WEBKIT_PAYLOAD_BYTES: usize = 32 * 1024 * 1024;
const MAX_COSMETIC_PAYLOAD_BYTES: usize = 16 * 1024 * 1024;
const COVERAGE_FIELD_COUNT: usize = 9;
const RECORD_HEADER_BYTES: usize = 280;
const RECORD_CHECKSUM_BYTES: usize = 32;
const MAX_RECORD_BYTES: usize = RECORD_HEADER_BYTES
    + MAX_RUNTIME_PAYLOAD_BYTES
    + MAX_COSMETIC_PAYLOAD_BYTES
    + RECORD_CHECKSUM_BYTES;

/// Private directory used only for Zephium's compiled blocker cache.
///
/// Persistence is an optimization. If this root cannot be proven private,
/// locked, and structurally safe, the worker compiles from authenticated
/// source material and does not touch the cache.
#[derive(Clone, Debug)]
pub struct CompiledArtifactCacheConfig {
    root: PathBuf,
}

impl CompiledArtifactCacheConfig {
    /// Creates a cache configuration rooted at an absolute, dedicated path.
    ///
    /// The compiler worker may create this final leaf, but it never creates
    /// missing ancestors. Callers should place it below an existing,
    /// browser-owned per-user cache directory.
    pub fn new(root: impl Into<PathBuf>) -> Result<Self, CompiledArtifactCacheConfigError> {
        let root = root.into();
        if !root.is_absolute() || root.file_name().is_none() {
            return Err(CompiledArtifactCacheConfigError::UnsafeRoot);
        }
        Ok(Self { root })
    }

    pub(crate) fn root(&self) -> &Path {
        &self.root
    }
}

/// A compiled-cache root cannot establish a stable security boundary.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum CompiledArtifactCacheConfigError {
    /// The path was relative or did not name a dedicated leaf directory.
    #[error("compiled blocker cache root must be an absolute dedicated directory")]
    UnsafeRoot,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct CacheKey([u8; 32]);

impl CacheKey {
    pub(crate) fn for_authenticated_catalog(
        target: CompileTarget,
        limits: CompileLimits,
        manifest_sha256: [u8; 32],
    ) -> Self {
        let mut digest = cache_key_prefix(target, limits);
        digest.update(b"authenticated-manifest");
        digest.update(manifest_sha256);
        Self(digest.finalize().into())
    }

    pub(crate) fn for_catalog<'a>(
        target: CompileTarget,
        limits: CompileLimits,
        sources: impl IntoIterator<Item = (&'a str, u8, &'a str)>,
    ) -> Self {
        let mut sources: Vec<_> = sources.into_iter().collect();
        sources.sort_unstable_by(|left, right| left.0.cmp(right.0));
        let mut digest = cache_key_prefix(target, limits);
        digest.update(b"inline-sources");
        digest.update((sources.len() as u64).to_be_bytes());
        for (id, format, contents) in sources {
            digest.update((id.len() as u64).to_be_bytes());
            digest.update(id.as_bytes());
            digest.update([format]);
            digest.update((contents.len() as u64).to_be_bytes());
            digest.update(contents.as_bytes());
        }
        Self(digest.finalize().into())
    }
}

fn cache_key_prefix(target: CompileTarget, limits: CompileLimits) -> Sha256 {
    let mut digest = Sha256::new();
    digest.update(CACHE_KEY_DOMAIN);
    digest.update(CACHE_FORMAT_VERSION.to_be_bytes());
    digest.update(POLICY_FORMAT_VERSION.to_be_bytes());
    digest.update(WEBKIT_ARTIFACT_FORMAT_VERSION.to_be_bytes());
    digest.update(ADBLOCK_ENGINE_VERSION.as_bytes());
    digest.update([target_tag(target)]);
    digest.update((std::env::consts::OS.len() as u64).to_be_bytes());
    digest.update(std::env::consts::OS.as_bytes());
    digest.update((std::env::consts::ARCH.len() as u64).to_be_bytes());
    digest.update(std::env::consts::ARCH.as_bytes());
    digest.update([usize::BITS as u8]);
    digest.update([u8::from(cfg!(target_endian = "little"))]);
    for value in limit_values(limits) {
        digest.update((value as u64).to_be_bytes());
    }
    digest
}

pub(crate) enum LoadedArtifact {
    #[cfg(feature = "runtime")]
    Runtime {
        digest: ContentRuleDigest,
        coverage: ContentRuleCoverage,
        rules: Box<CachedRuntimeRules>,
        cosmetics: Option<PreparedCosmetics>,
    },
    #[cfg(feature = "webkit")]
    WebKit {
        digest: ContentRuleDigest,
        coverage: ContentRuleCoverage,
        artifact_digest: [u8; 32],
        encoded: Arc<dyn DeclarativeRuleText>,
        cosmetics: Option<PreparedCosmetics>,
    },
}

pub(crate) struct PersistentArtifactCache {
    root: PathBuf,
    _lock: File,
}

impl PersistentArtifactCache {
    pub(crate) fn open(config: &CompiledArtifactCacheConfig) -> Result<Self, CacheError> {
        prepare_private_root(config.root())?;
        let lock = open_and_lock(config.root())?;
        let cache = Self {
            root: config.root().to_path_buf(),
            _lock: lock,
        };
        cache.validate_inventory_and_clean_stage()?;
        Ok(cache)
    }

    pub(crate) fn load(
        &self,
        key: CacheKey,
        target: CompileTarget,
        limits: CompileLimits,
    ) -> Result<Option<LoadedArtifact>, CacheError> {
        let max_record_bytes = max_record_bytes(target);
        for name in [CURRENT_FILE, PREVIOUS_FILE] {
            #[cfg(feature = "webkit")]
            if target == CompileTarget::WebKit {
                match load_webkit_record(&self.root.join(name), key, limits) {
                    Ok(Some(artifact)) => return Ok(Some(artifact)),
                    Ok(None) | Err(CacheError::Invalid) => continue,
                    Err(error) => return Err(error),
                }
            }
            let bytes = match read_bounded_regular(&self.root.join(name), max_record_bytes) {
                Ok(Some(bytes)) => bytes,
                Ok(None) | Err(CacheError::Invalid) => continue,
                Err(error) => return Err(error),
            };
            let Some(record) = decode_record(&bytes, key, target) else {
                continue;
            };
            if let Some(artifact) = validate_payload(record, limits) {
                return Ok(Some(artifact));
            }
        }
        Ok(None)
    }

    pub(crate) fn store(
        &self,
        key: CacheKey,
        compiled: &CompiledRules,
        rules: &ContentRules,
    ) -> Result<(), CacheError> {
        let record = encode_compiled_record(key, compiled, rules).ok_or(CacheError::Invalid)?;
        self.persist_record(&record, PersistFault::None)
    }

    fn validate_inventory_and_clean_stage(&self) -> Result<(), CacheError> {
        let mut count = 0usize;
        for entry in fs::read_dir(&self.root).map_err(|_| CacheError::Io)? {
            let entry = entry.map_err(|_| CacheError::Io)?;
            count = count.checked_add(1).ok_or(CacheError::Unsafe)?;
            if count > MAX_CACHE_DIRECTORY_ENTRIES {
                return Err(CacheError::Unsafe);
            }
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                return Err(CacheError::Unsafe);
            };
            if !matches!(name, LOCK_FILE | CURRENT_FILE | PREVIOUS_FILE | STAGE_FILE) {
                return Err(CacheError::Unsafe);
            }
        }
        verify_optional_regular(&self.root.join(CURRENT_FILE))?;
        verify_optional_regular(&self.root.join(PREVIOUS_FILE))?;
        remove_verified_regular_if_present(&self.root.join(STAGE_FILE))?;
        sync_directory(&self.root)?;
        Ok(())
    }

    fn persist_record(&self, record: &[u8], fault: PersistFault) -> Result<(), CacheError> {
        if record.len() > MAX_RECORD_BYTES {
            return Err(CacheError::Invalid);
        }
        let stage = self.root.join(STAGE_FILE);
        let current = self.root.join(CURRENT_FILE);
        let previous = self.root.join(PREVIOUS_FILE);
        remove_verified_regular_if_present(&stage)?;
        write_new_private_file(&stage, record)?;
        if read_bounded_regular(&stage, MAX_RECORD_BYTES)?.as_deref() != Some(record) {
            return Err(CacheError::Io);
        }
        if fault == PersistFault::AfterStageSync {
            return Err(CacheError::Io);
        }

        remove_verified_regular_if_present(&previous)?;
        if fault == PersistFault::AfterPreviousRemoval {
            return Err(CacheError::Io);
        }
        if current.try_exists().map_err(|_| CacheError::Io)? {
            rename_verified(&current, &previous)?;
        }
        sync_directory(&self.root)?;
        if fault == PersistFault::AfterCurrentRotation {
            return Err(CacheError::Io);
        }

        rename_verified(&stage, &current)?;
        sync_directory(&self.root)?;
        if read_bounded_regular(&current, MAX_RECORD_BYTES)?.as_deref() != Some(record) {
            return Err(CacheError::Io);
        }
        if fault == PersistFault::AfterCurrentInstall {
            return Err(CacheError::Io);
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CacheError {
    Io,
    Unsafe,
    Invalid,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PersistFault {
    None,
    AfterStageSync,
    AfterPreviousRemoval,
    AfterCurrentRotation,
    AfterCurrentInstall,
}

// Only runtime records are decoded whole; WebKit ones stream through
// `load_webkit_record`.
#[cfg_attr(not(feature = "runtime"), allow(dead_code))]
struct DecodedRecord<'a> {
    target: CompileTarget,
    policy_digest: [u8; 32],
    native_digest: [u8; 32],
    coverage: ContentRuleCoverage,
    rule_count: usize,
    payload: &'a [u8],
    cosmetics: &'a [u8],
}

fn encode_compiled_record(
    key: CacheKey,
    compiled: &CompiledRules,
    rules: &ContentRules,
) -> Option<Vec<u8>> {
    if compiled.target() != target_for_payload(rules.payload())
        || compiled.digest().as_bytes() != rules.digest().as_bytes()
        || !rules.enabled()
        || !rules.coverage().is_consistent()
        || !rules.coverage().has_blocking_entries()
    {
        return None;
    }
    #[cfg(feature = "webkit")]
    let text;
    let (payload, native_digest, rule_count, payload_limit): (
        Cow<'_, [u8]>,
        [u8; 32],
        usize,
        usize,
    ) = match rules.payload() {
        #[cfg(feature = "runtime")]
        ContentRulesPayload::Runtime(_) => (
            Cow::Owned(compiled.serialize_runtime_engine()?),
            [0; 32],
            0usize,
            MAX_RUNTIME_PAYLOAD_BYTES,
        ),
        #[cfg(feature = "webkit")]
        ContentRulesPayload::Declarative {
            format: DeclarativeRuleFormat::WebKitContentBlockerV1,
            artifact_digest,
            encoded,
        } => (
            {
                text = encoded.load()?;
                Cow::Borrowed(text.as_bytes())
            },
            *artifact_digest.as_bytes(),
            compiled.webkit()?.rule_count(),
            MAX_WEBKIT_PAYLOAD_BYTES,
        ),
        ContentRulesPayload::AllowAll => return None,
        #[allow(unreachable_patterns)]
        _ => return None,
    };
    if payload.is_empty() || payload.len() > payload_limit {
        return None;
    }
    let cosmetics = compiled
        .cosmetics()
        .map(|value| value.policy.encode())
        .transpose()
        .ok()?
        .unwrap_or_default();
    encode_record_with_cosmetics(
        key,
        compiled.target(),
        *rules.digest().as_bytes(),
        native_digest,
        rules.coverage(),
        rule_count,
        payload.as_ref(),
        &cosmetics,
    )
}

#[cfg(test)]
fn encode_record(
    key: CacheKey,
    target: CompileTarget,
    policy_digest: [u8; 32],
    native_digest: [u8; 32],
    coverage: ContentRuleCoverage,
    rule_count: usize,
    payload: &[u8],
) -> Option<Vec<u8>> {
    encode_record_with_cosmetics(
        key,
        target,
        policy_digest,
        native_digest,
        coverage,
        rule_count,
        payload,
        &[],
    )
}

#[allow(clippy::too_many_arguments)]
fn encode_record_with_cosmetics(
    key: CacheKey,
    target: CompileTarget,
    policy_digest: [u8; 32],
    native_digest: [u8; 32],
    coverage: ContentRuleCoverage,
    rule_count: usize,
    payload: &[u8],
    cosmetics: &[u8],
) -> Option<Vec<u8>> {
    let payload_limit = match target {
        CompileTarget::Runtime => MAX_RUNTIME_PAYLOAD_BYTES,
        CompileTarget::WebKit => MAX_WEBKIT_PAYLOAD_BYTES,
    };
    if payload.is_empty()
        || payload.len() > payload_limit
        || cosmetics.len() > MAX_COSMETIC_PAYLOAD_BYTES
        || !coverage.is_consistent()
        || !coverage.has_blocking_entries()
    {
        return None;
    }
    let payload_digest: [u8; 32] = Sha256::digest(payload).into();
    let mut bytes = Vec::with_capacity(
        RECORD_HEADER_BYTES
            .checked_add(payload.len())?
            .checked_add(cosmetics.len())?
            .checked_add(RECORD_CHECKSUM_BYTES)?,
    );
    bytes.extend_from_slice(CACHE_MAGIC);
    bytes.extend_from_slice(&CACHE_FORMAT_VERSION.to_be_bytes());
    bytes.extend_from_slice(&POLICY_FORMAT_VERSION.to_be_bytes());
    bytes.extend_from_slice(&WEBKIT_ARTIFACT_FORMAT_VERSION.to_be_bytes());
    bytes.push(target_tag(target));
    bytes.extend_from_slice(&[0; 3]);
    bytes.extend_from_slice(&Sha256::digest(ADBLOCK_ENGINE_VERSION.as_bytes()));
    bytes.extend_from_slice(&key.0);
    bytes.extend_from_slice(&policy_digest);
    bytes.extend_from_slice(&native_digest);
    bytes.extend_from_slice(&payload_digest);
    for value in coverage_values(coverage) {
        bytes.extend_from_slice(&value.to_be_bytes());
    }
    bytes.extend_from_slice(&u64::try_from(rule_count).ok()?.to_be_bytes());
    bytes.extend_from_slice(&u64::try_from(payload.len()).ok()?.to_be_bytes());
    bytes.extend_from_slice(&u64::try_from(cosmetics.len()).ok()?.to_be_bytes());
    if bytes.len() != RECORD_HEADER_BYTES {
        return None;
    }
    bytes.extend_from_slice(payload);
    bytes.extend_from_slice(cosmetics);
    let checksum: [u8; 32] = Sha256::digest(&bytes).into();
    bytes.extend_from_slice(&checksum);
    Some(bytes)
}

struct RecordHeader {
    target: CompileTarget,
    policy_digest: [u8; 32],
    native_digest: [u8; 32],
    payload_digest: [u8; 32],
    coverage: ContentRuleCoverage,
    rule_count: usize,
    payload_len: usize,
    cosmetics_len: usize,
}

impl RecordHeader {
    fn record_len(&self) -> Option<usize> {
        RECORD_HEADER_BYTES
            .checked_add(self.payload_len)?
            .checked_add(self.cosmetics_len)?
            .checked_add(RECORD_CHECKSUM_BYTES)
    }
}

fn decode_header(
    header: &[u8],
    expected_key: CacheKey,
    expected_target: CompileTarget,
) -> Option<RecordHeader> {
    let mut reader = RecordReader::new(header.get(..RECORD_HEADER_BYTES)?);
    if reader.take::<8>()? != *CACHE_MAGIC
        || reader.u32()? != CACHE_FORMAT_VERSION
        || reader.u32()? != POLICY_FORMAT_VERSION
        || reader.u32()? != WEBKIT_ARTIFACT_FORMAT_VERSION
    {
        return None;
    }
    let target = target_from_tag(reader.u8()?)?;
    if target != expected_target || reader.take::<3>()? != [0; 3] {
        return None;
    }
    let adblock_digest: [u8; 32] = Sha256::digest(ADBLOCK_ENGINE_VERSION.as_bytes()).into();
    if reader.take::<32>()? != adblock_digest || reader.take::<32>()? != expected_key.0 {
        return None;
    }
    let policy_digest = reader.take::<32>()?;
    let native_digest = reader.take::<32>()?;
    let payload_digest = reader.take::<32>()?;
    let mut coverage_values = [0; COVERAGE_FIELD_COUNT];
    for value in &mut coverage_values {
        *value = reader.u64()?;
    }
    let coverage = coverage_from_values(coverage_values)?;
    let rule_count = usize::try_from(reader.u64()?).ok()?;
    let payload_len = usize::try_from(reader.u64()?).ok()?;
    let cosmetics_len = usize::try_from(reader.u64()?).ok()?;
    if cosmetics_len > MAX_COSMETIC_PAYLOAD_BYTES
        || reader.remaining() != 0
        || payload_len > payload_limit(target)
    {
        return None;
    }
    Some(RecordHeader {
        target,
        policy_digest,
        native_digest,
        payload_digest,
        coverage,
        rule_count,
        payload_len,
        cosmetics_len,
    })
}

fn decode_record(
    bytes: &[u8],
    expected_key: CacheKey,
    expected_target: CompileTarget,
) -> Option<DecodedRecord<'_>> {
    if bytes.len() < RECORD_HEADER_BYTES + RECORD_CHECKSUM_BYTES
        || bytes.len() > max_record_bytes(expected_target)
    {
        return None;
    }
    let (record, checksum) = bytes.split_at(bytes.len() - RECORD_CHECKSUM_BYTES);
    if Sha256::digest(record).as_slice() != checksum {
        return None;
    }
    let header = decode_header(record, expected_key, expected_target)?;
    if header.record_len()? != bytes.len() {
        return None;
    }
    let mut reader = RecordReader::new(record);
    reader.slice(RECORD_HEADER_BYTES)?;
    let payload = reader.slice(header.payload_len)?;
    let cosmetics = reader.slice(header.cosmetics_len)?;
    if reader.remaining() != 0 || Sha256::digest(payload).as_slice() != header.payload_digest {
        return None;
    }
    Some(DecodedRecord {
        target: header.target,
        policy_digest: header.policy_digest,
        native_digest: header.native_digest,
        coverage: header.coverage,
        rule_count: header.rule_count,
        payload,
        cosmetics,
    })
}

/// Checks a WebKit record without bringing its rule text into memory: one
/// streaming pass verifies the record checksum, which also covers the payload
/// and artifact digests stored in the header, and only the cosmetic section is
/// kept. The text itself is read again, re-hashed against the artifact digest
/// and validated only if the native store has lost the compiled list.
#[cfg(feature = "webkit")]
fn load_webkit_record(
    path: &Path,
    key: CacheKey,
    limits: CompileLimits,
) -> Result<Option<LoadedArtifact>, CacheError> {
    const CHUNK_BYTES: usize = 64 * 1024;

    let mut file = match file_identity::open_verified_regular(path) {
        Some(file) => file,
        None => match fs::symlink_metadata(path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            _ => return Err(CacheError::Unsafe),
        },
    };
    let length = file.metadata().map_err(|_| CacheError::Io)?.len();
    let length = usize::try_from(length).map_err(|_| CacheError::Unsafe)?;
    if length < RECORD_HEADER_BYTES + RECORD_CHECKSUM_BYTES
        || length > max_record_bytes(CompileTarget::WebKit)
    {
        return Err(CacheError::Invalid);
    }
    let mut header_bytes = [0; RECORD_HEADER_BYTES];
    file.read_exact(&mut header_bytes)
        .map_err(|_| CacheError::Io)?;
    let Some(header) = decode_header(&header_bytes, key, CompileTarget::WebKit) else {
        return Ok(None);
    };
    if header.record_len() != Some(length)
        || !coverage_within_limits(header.coverage, limits)
        || header.rule_count == 0
        || header.rule_count > limits.max_webkit_rules()
        || header.payload_len == 0
        || header.payload_len > limits.max_webkit_json_bytes()
        || header.coverage.platform_source_kind_approximated_rules != 0
    {
        return Ok(None);
    }

    let mut record_digest = Sha256::new();
    record_digest.update(header_bytes);
    let mut chunk = vec![0; CHUNK_BYTES];
    let mut remaining = header.payload_len;
    while remaining != 0 {
        let part = &mut chunk[..remaining.min(CHUNK_BYTES)];
        file.read_exact(part).map_err(|_| CacheError::Io)?;
        record_digest.update(&*part);
        remaining -= part.len();
    }
    let mut cosmetics = vec![0; header.cosmetics_len];
    file.read_exact(&mut cosmetics)
        .map_err(|_| CacheError::Io)?;
    record_digest.update(&cosmetics);
    let mut checksum = [0; RECORD_CHECKSUM_BYTES];
    file.read_exact(&mut checksum).map_err(|_| CacheError::Io)?;
    if record_digest.finalize().as_slice() != checksum {
        return Ok(None);
    }
    let artifact_digest = header.native_digest;
    let cosmetics = if cosmetics.is_empty() {
        None
    } else {
        let Some(prepared) = crate::CosmeticPolicy::decode(&cosmetics)
            .ok()
            .and_then(|policy| PreparedCosmetics::new(policy, CompileTarget::WebKit).ok())
        else {
            return Ok(None);
        };
        Some(prepared)
    };
    Ok(Some(LoadedArtifact::WebKit {
        digest: ContentRuleDigest::from_bytes(header.policy_digest),
        coverage: header.coverage,
        artifact_digest,
        encoded: Arc::new(CachedWebKitRules {
            file: Mutex::new(file),
            len: header.payload_len,
            rule_count: header.rule_count,
            blocking_entries: header.coverage.blocking_rule_entries,
            artifact_digest,
        }),
        cosmetics,
    }))
}

/// WebKit rule text left on disk in a checked cache record. The open file
/// keeps the exact record readable after the cache rotates its names, and
/// every read re-checks the text against the artifact digest.
#[cfg(feature = "webkit")]
struct CachedWebKitRules {
    file: Mutex<File>,
    len: usize,
    rule_count: usize,
    blocking_entries: u64,
    artifact_digest: [u8; 32],
}

#[cfg(feature = "webkit")]
impl DeclarativeRuleText for CachedWebKitRules {
    fn len(&self) -> usize {
        self.len
    }

    fn load(&self) -> Option<Arc<str>> {
        let mut payload = vec![0; self.len];
        {
            let mut file = self.file.lock().ok()?;
            file.seek(SeekFrom::Start(RECORD_HEADER_BYTES as u64))
                .ok()?;
            file.read_exact(&mut payload).ok()?;
        }
        let mut digest = webkit_artifact_digest_prefix();
        digest.update(&payload);
        if digest.finalize().as_slice() != self.artifact_digest {
            return None;
        }
        let (encoded, blocking_entries) =
            validate_canonical_webkit_payload(&payload, self.rule_count)?;
        (u64::try_from(blocking_entries).ok()? == self.blocking_entries).then_some(encoded)
    }
}

#[cfg(feature = "webkit")]
fn webkit_artifact_digest_prefix() -> Sha256 {
    let mut digest = Sha256::new();
    digest.update(WEBKIT_DIGEST_DOMAIN);
    digest.update(WEBKIT_ARTIFACT_FORMAT_VERSION.to_be_bytes());
    digest
}

fn validate_payload(record: DecodedRecord<'_>, limits: CompileLimits) -> Option<LoadedArtifact> {
    if !coverage_within_limits(record.coverage, limits) {
        return None;
    }
    let cosmetics = if record.cosmetics.is_empty() {
        None
    } else {
        Some(
            PreparedCosmetics::new(
                crate::CosmeticPolicy::decode(record.cosmetics).ok()?,
                record.target,
            )
            .ok()?,
        )
    };
    match record.target {
        CompileTarget::Runtime => validate_runtime_payload(record, limits, cosmetics),
        // WebKit records stream through `load_webkit_record` instead.
        CompileTarget::WebKit => None,
    }
}

#[cfg(feature = "runtime")]
fn validate_runtime_payload(
    record: DecodedRecord<'_>,
    limits: CompileLimits,
    cosmetics: Option<PreparedCosmetics>,
) -> Option<LoadedArtifact> {
    if record.rule_count != 0
        || record.native_digest != [0; 32]
        || record.coverage.platform_attribution_approximated_rules != 0
    {
        return None;
    }
    let mut engine = adblock::Engine::default();
    engine.deserialize(record.payload).ok()?;
    let (blocking_entries, _) = prepare_and_validate_runtime_engine(&engine, limits).ok()?;
    if u64::try_from(blocking_entries).ok()? != record.coverage.blocking_rule_entries {
        return None;
    }
    Some(LoadedArtifact::Runtime {
        digest: ContentRuleDigest::from_bytes(record.policy_digest),
        coverage: record.coverage,
        rules: Box::new(CachedRuntimeRules::new(engine, limits)),
        cosmetics,
    })
}

#[cfg(not(feature = "runtime"))]
fn validate_runtime_payload(
    _record: DecodedRecord<'_>,
    _limits: CompileLimits,
    _cosmetics: Option<PreparedCosmetics>,
) -> Option<LoadedArtifact> {
    None
}

fn coverage_within_limits(coverage: ContentRuleCoverage, limits: CompileLimits) -> bool {
    coverage.is_consistent()
        && coverage.has_blocking_entries()
        && coverage.source_rules <= limits.max_rules() as u64
        && coverage.accepted_rules <= limits.max_rules() as u64
        && coverage.rejected_rules <= limits.max_rules() as u64
        && coverage.blocking_rule_entries <= (limits.max_rules() as u64).saturating_mul(2)
}

fn coverage_values(coverage: ContentRuleCoverage) -> [u64; COVERAGE_FIELD_COUNT] {
    [
        coverage.source_rules,
        coverage.accepted_rules,
        coverage.rejected_rules,
        coverage.platform_omitted_rules,
        coverage.platform_approximated_rules,
        coverage.platform_resource_approximated_rules,
        coverage.platform_source_kind_approximated_rules,
        coverage.platform_attribution_approximated_rules,
        coverage.blocking_rule_entries,
    ]
}

fn coverage_from_values(values: [u64; COVERAGE_FIELD_COUNT]) -> Option<ContentRuleCoverage> {
    let coverage = ContentRuleCoverage {
        source_rules: values[0],
        accepted_rules: values[1],
        rejected_rules: values[2],
        platform_omitted_rules: values[3],
        platform_approximated_rules: values[4],
        platform_resource_approximated_rules: values[5],
        platform_source_kind_approximated_rules: values[6],
        platform_attribution_approximated_rules: values[7],
        blocking_rule_entries: values[8],
    };
    (coverage.is_consistent() && coverage.has_blocking_entries()).then_some(coverage)
}

fn limit_values(limits: CompileLimits) -> [usize; 10] {
    [
        limits.max_sources(),
        limits.max_source_bytes(),
        limits.max_total_source_bytes(),
        limits.max_line_bytes(),
        limits.max_rules(),
        limits.max_physical_lines(),
        limits.max_webkit_rules(),
        limits.max_webkit_json_bytes(),
        limits.max_request_url_bytes(),
        limits.max_source_url_bytes(),
    ]
}

const fn target_tag(target: CompileTarget) -> u8 {
    match target {
        CompileTarget::Runtime => 1,
        CompileTarget::WebKit => 2,
    }
}

const fn target_from_tag(tag: u8) -> Option<CompileTarget> {
    match tag {
        1 => Some(CompileTarget::Runtime),
        2 => Some(CompileTarget::WebKit),
        _ => None,
    }
}

const fn payload_limit(target: CompileTarget) -> usize {
    match target {
        CompileTarget::Runtime => MAX_RUNTIME_PAYLOAD_BYTES,
        CompileTarget::WebKit => MAX_WEBKIT_PAYLOAD_BYTES,
    }
}

const fn max_record_bytes(target: CompileTarget) -> usize {
    RECORD_HEADER_BYTES + payload_limit(target) + MAX_COSMETIC_PAYLOAD_BYTES + RECORD_CHECKSUM_BYTES
}

const fn target_for_payload(payload: &ContentRulesPayload) -> CompileTarget {
    match payload {
        ContentRulesPayload::Runtime(_) => CompileTarget::Runtime,
        ContentRulesPayload::Declarative { .. } | ContentRulesPayload::AllowAll => {
            CompileTarget::WebKit
        }
    }
}

struct RecordReader<'a> {
    bytes: &'a [u8],
    position: usize,
}

impl<'a> RecordReader<'a> {
    const fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, position: 0 }
    }

    fn take<const N: usize>(&mut self) -> Option<[u8; N]> {
        let end = self.position.checked_add(N)?;
        let value = self.bytes.get(self.position..end)?.try_into().ok()?;
        self.position = end;
        Some(value)
    }

    fn slice(&mut self, len: usize) -> Option<&'a [u8]> {
        let end = self.position.checked_add(len)?;
        let value = self.bytes.get(self.position..end)?;
        self.position = end;
        Some(value)
    }

    fn u8(&mut self) -> Option<u8> {
        Some(self.take::<1>()?[0])
    }

    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_be_bytes(self.take()?))
    }

    fn u64(&mut self) -> Option<u64> {
        Some(u64::from_be_bytes(self.take()?))
    }

    const fn remaining(&self) -> usize {
        self.bytes.len().saturating_sub(self.position)
    }
}

fn prepare_private_root(root: &Path) -> Result<(), CacheError> {
    if !root.is_absolute() || root.file_name().is_none() {
        return Err(CacheError::Unsafe);
    }
    match fs::symlink_metadata(root) {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let parent = root.parent().ok_or(CacheError::Unsafe)?;
            validate_directory_chain(parent, false)?;
            #[allow(unused_mut)]
            let mut builder = fs::DirBuilder::new();
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                builder.mode(0o700);
            }
            builder.create(root).map_err(|_| CacheError::Io)?;
        }
        Err(_) => return Err(CacheError::Io),
    }
    validate_directory_chain(root, true)
}

fn validate_directory_chain(path: &Path, private_leaf: bool) -> Result<(), CacheError> {
    for current in path.ancestors().collect::<Vec<_>>().into_iter().rev() {
        if current.as_os_str().is_empty() {
            continue;
        }
        let metadata = fs::symlink_metadata(current).map_err(|_| CacheError::Unsafe)?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err(CacheError::Unsafe);
        }
        validate_directory_security(current, &metadata, private_leaf && current == path)?;
    }
    Ok(())
}

#[cfg(unix)]
fn validate_directory_security(
    _path: &Path,
    metadata: &fs::Metadata,
    private_leaf: bool,
) -> Result<(), CacheError> {
    use std::os::unix::fs::MetadataExt;

    let owner = metadata.uid();
    let current_user = rustix::process::geteuid().as_raw();
    let mode = metadata.mode();
    if owner != 0 && owner != current_user {
        return Err(CacheError::Unsafe);
    }
    if private_leaf {
        if owner != current_user || mode & 0o077 != 0 {
            return Err(CacheError::Unsafe);
        }
    } else if mode & 0o022 != 0 && (owner != 0 || mode & 0o1000 == 0) {
        return Err(CacheError::Unsafe);
    }
    Ok(())
}

#[cfg(target_os = "windows")]
fn validate_directory_security(
    path: &Path,
    metadata: &fs::Metadata,
    private_leaf: bool,
) -> Result<(), CacheError> {
    use std::os::windows::fs::MetadataExt;

    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
    if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(CacheError::Unsafe);
    }
    if private_leaf && !windows_private_directory_acl(path) {
        return Err(CacheError::Unsafe);
    }
    Ok(())
}

#[cfg(target_os = "windows")]
#[allow(unsafe_code)]
fn windows_private_directory_acl(path: &Path) -> bool {
    use std::ffi::c_void;
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use windows::core::{BOOL, PCWSTR};
    use windows::Win32::Foundation::{GENERIC_ALL, GENERIC_WRITE, HANDLE};
    use windows::Win32::Security::{
        GetAce, GetFileSecurityW, GetLengthSid, GetSecurityDescriptorDacl,
        GetSecurityDescriptorOwner, GetTokenInformation, IsValidSid, TokenUser, ACCESS_ALLOWED_ACE,
        ACE_HEADER, ACL, DACL_SECURITY_INFORMATION, OWNER_SECURITY_INFORMATION,
        PSECURITY_DESCRIPTOR, PSID, TOKEN_QUERY, TOKEN_USER,
    };
    use windows::Win32::Storage::FileSystem::{
        DELETE, FILE_APPEND_DATA, FILE_DELETE_CHILD, FILE_WRITE_ATTRIBUTES, FILE_WRITE_DATA,
        FILE_WRITE_EA, WRITE_DAC, WRITE_OWNER,
    };
    use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    const MAX_SECURITY_DESCRIPTOR_BYTES: u32 = 128 * 1024;
    const MAX_TOKEN_INFORMATION_BYTES: u32 = 64 * 1024;
    const ACCESS_ALLOWED_ACE_TYPE: u8 = 0;
    const ACCESS_DENIED_ACE_TYPE: u8 = 1;

    let wide_path: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let requested = OWNER_SECURITY_INFORMATION.0 | DACL_SECURITY_INFORMATION.0;
    let mut descriptor_bytes = 0u32;
    // SAFETY: the NUL-terminated path is live for both synchronous calls. A
    // null first buffer is the documented size-query form.
    unsafe {
        let _ = GetFileSecurityW(
            PCWSTR(wide_path.as_ptr()),
            requested,
            None,
            0,
            &raw mut descriptor_bytes,
        );
    }
    if descriptor_bytes == 0 || descriptor_bytes > MAX_SECURITY_DESCRIPTOR_BYTES {
        return false;
    }
    let descriptor_words = usize::try_from(descriptor_bytes)
        .ok()
        .and_then(|bytes| bytes.checked_add(std::mem::size_of::<usize>() - 1))
        .map(|bytes| bytes / std::mem::size_of::<usize>());
    let Some(descriptor_words) = descriptor_words else {
        return false;
    };
    let mut descriptor = vec![0usize; descriptor_words];
    let descriptor_pointer = PSECURITY_DESCRIPTOR(descriptor.as_mut_ptr().cast());
    // SAFETY: the aligned buffer has at least `descriptor_bytes` writable
    // bytes and remains live while every pointer derived from it is used.
    if !unsafe {
        GetFileSecurityW(
            PCWSTR(wide_path.as_ptr()),
            requested,
            Some(descriptor_pointer),
            descriptor_bytes,
            &raw mut descriptor_bytes,
        )
        .as_bool()
    } {
        return false;
    }

    let mut token_handle = HANDLE::default();
    // SAFETY: the pseudo-process handle is valid and the output points to a
    // live HANDLE slot.
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &raw mut token_handle) }.is_err()
        || token_handle.is_invalid()
    {
        return false;
    }
    // SAFETY: OpenProcessToken returned exclusive ownership of this handle.
    let token = unsafe { OwnedHandle::from_raw_handle(token_handle.0) };
    let token_handle = HANDLE(token.as_raw_handle());
    let mut token_bytes = 0u32;
    // SAFETY: the null-buffer call is the documented size query.
    let _ = unsafe { GetTokenInformation(token_handle, TokenUser, None, 0, &raw mut token_bytes) };
    if token_bytes < u32::try_from(std::mem::size_of::<TOKEN_USER>()).unwrap_or(u32::MAX)
        || token_bytes > MAX_TOKEN_INFORMATION_BYTES
    {
        return false;
    }
    let token_words = usize::try_from(token_bytes)
        .ok()
        .and_then(|bytes| bytes.checked_add(std::mem::size_of::<usize>() - 1))
        .map(|bytes| bytes / std::mem::size_of::<usize>());
    let Some(token_words) = token_words else {
        return false;
    };
    let mut token_information = vec![0usize; token_words];
    // SAFETY: the aligned token buffer has the exact capacity reported by
    // GetTokenInformation and remains live while its SID pointer is used.
    if unsafe {
        GetTokenInformation(
            token_handle,
            TokenUser,
            Some(token_information.as_mut_ptr().cast()),
            token_bytes,
            &raw mut token_bytes,
        )
    }
    .is_err()
    {
        return false;
    }
    // SAFETY: the successful TokenUser query returned at least TOKEN_USER.
    let current_user = unsafe { &*token_information.as_ptr().cast::<TOKEN_USER>() }
        .User
        .Sid;
    if !unsafe { IsValidSid(current_user).as_bool() } {
        return false;
    }

    let mut owner = PSID::default();
    let mut owner_defaulted = BOOL::default();
    // SAFETY: the descriptor buffer is valid and all output slots live for
    // the call. `owner` remains within that live descriptor afterwards.
    if unsafe {
        GetSecurityDescriptorOwner(descriptor_pointer, &raw mut owner, &raw mut owner_defaulted)
    }
    .is_err()
        || owner.is_invalid()
        || !unsafe { IsValidSid(owner).as_bool() }
        || !trusted_windows_cache_owner(owner, current_user, token_handle)
    {
        return false;
    }

    let mut dacl_present = BOOL::default();
    let mut dacl_defaulted = BOOL::default();
    let mut dacl: *mut ACL = std::ptr::null_mut();
    // A missing or null DACL grants full access and therefore cannot back a
    // private cache boundary.
    if unsafe {
        GetSecurityDescriptorDacl(
            descriptor_pointer,
            &raw mut dacl_present,
            &raw mut dacl,
            &raw mut dacl_defaulted,
        )
    }
    .is_err()
        || !dacl_present.as_bool()
        || dacl.is_null()
    {
        return false;
    }

    let mutation_rights = FILE_WRITE_DATA.0
        | FILE_APPEND_DATA.0
        | FILE_WRITE_EA.0
        | FILE_WRITE_ATTRIBUTES.0
        | FILE_DELETE_CHILD.0
        | DELETE.0
        | WRITE_DAC.0
        | WRITE_OWNER.0
        | GENERIC_WRITE.0
        | GENERIC_ALL.0;
    // SAFETY: `dacl` points into the live, kernel-validated descriptor.
    let ace_count = unsafe { (*dacl).AceCount };
    for index in 0..u32::from(ace_count) {
        let mut ace: *mut c_void = std::ptr::null_mut();
        // SAFETY: the index is within the ACL's declared ACE count and the
        // output pointer remains inside the live descriptor.
        if unsafe { GetAce(dacl, index, &raw mut ace) }.is_err() || ace.is_null() {
            return false;
        }
        // SAFETY: GetAce returned a valid ACE with at least an ACE_HEADER.
        let header = unsafe { &*ace.cast::<ACE_HEADER>() };
        if header.AceType == ACCESS_DENIED_ACE_TYPE {
            continue;
        }
        // Reject uncommon/callback/object allow forms instead of attempting
        // to reinterpret their variable layouts.
        if header.AceType != ACCESS_ALLOWED_ACE_TYPE
            || usize::from(header.AceSize) < std::mem::size_of::<ACCESS_ALLOWED_ACE>()
        {
            return false;
        }
        // SAFETY: the size check above proves the fixed ACCESS_ALLOWED_ACE
        // prefix, whose trailing SidStart begins the variable-length SID.
        let allowed = unsafe { &*ace.cast::<ACCESS_ALLOWED_ACE>() };
        if allowed.Mask & mutation_rights == 0 {
            continue;
        }
        let sid = PSID(std::ptr::addr_of!(allowed.SidStart).cast_mut().cast());
        if !unsafe { IsValidSid(sid).as_bool() } {
            return false;
        }
        // SAFETY: IsValidSid established that GetLengthSid may inspect `sid`.
        let sid_bytes = unsafe { GetLengthSid(sid) } as usize;
        let sid_offset = std::mem::offset_of!(ACCESS_ALLOWED_ACE, SidStart);
        if sid_offset
            .checked_add(sid_bytes)
            .is_none_or(|bytes| bytes > usize::from(header.AceSize))
            || !trusted_windows_cache_writer(sid, current_user)
        {
            return false;
        }
    }
    true
}

#[cfg(target_os = "windows")]
#[allow(unsafe_code)]
fn trusted_windows_cache_owner(
    owner: windows::Win32::Security::PSID,
    current_user: windows::Win32::Security::PSID,
    token: windows::Win32::Foundation::HANDLE,
) -> bool {
    use windows::Win32::Security::{
        EqualSid, GetTokenInformation, IsWellKnownSid, TokenElevation, WinBuiltinAdministratorsSid,
        TOKEN_ELEVATION,
    };

    // SAFETY: both SIDs were validated by the caller and remain live.
    if unsafe { EqualSid(owner, current_user) }.is_ok() {
        return true;
    }
    // Elevated tokens can create Administrators-owned directories by default.
    // The DACL still admits only the same trusted writers as user-owned roots.
    // SAFETY: owner remains a valid SID and token is a live query handle.
    if !unsafe { IsWellKnownSid(owner, WinBuiltinAdministratorsSid).as_bool() } {
        return false;
    }
    let mut elevation = TOKEN_ELEVATION::default();
    let mut bytes = 0;
    // SAFETY: the aligned output is sized for TokenElevation and lives for the call.
    unsafe {
        GetTokenInformation(
            token,
            TokenElevation,
            Some((&raw mut elevation).cast()),
            std::mem::size_of::<TOKEN_ELEVATION>() as u32,
            &raw mut bytes,
        )
        .is_ok()
            && elevation.TokenIsElevated != 0
    }
}

#[cfg(target_os = "windows")]
#[allow(unsafe_code)]
fn trusted_windows_cache_writer(
    candidate: windows::Win32::Security::PSID,
    current_user: windows::Win32::Security::PSID,
) -> bool {
    use windows::Win32::Security::{
        CreateWellKnownSid, EqualSid, WinBuiltinAdministratorsSid, WinCreatorOwnerRightsSid,
        WinCreatorOwnerSid, WinLocalSystemSid, PSID, WELL_KNOWN_SID_TYPE,
    };

    // SAFETY: both SIDs were validated by the caller and remain live.
    if unsafe { EqualSid(candidate, current_user) }.is_ok() {
        return true;
    }
    [
        WinLocalSystemSid,
        WinBuiltinAdministratorsSid,
        WinCreatorOwnerSid,
        WinCreatorOwnerRightsSid,
    ]
    .into_iter()
    .any(|kind: WELL_KNOWN_SID_TYPE| {
        let mut storage = [0usize; 16];
        let mut bytes = u32::try_from(std::mem::size_of_val(&storage)).unwrap_or(u32::MAX);
        let trusted = PSID(storage.as_mut_ptr().cast());
        // SAFETY: the aligned storage is larger than SECURITY_MAX_SID_SIZE and
        // both SIDs remain live for the immediate comparison.
        unsafe {
            CreateWellKnownSid(kind, None, Some(trusted), &raw mut bytes).is_ok()
                && EqualSid(candidate, trusted).is_ok()
        }
    })
}

#[cfg(not(any(unix, target_os = "windows")))]
fn validate_directory_security(
    _path: &Path,
    _metadata: &fs::Metadata,
    _private_leaf: bool,
) -> Result<(), CacheError> {
    Err(CacheError::Unsafe)
}

fn open_and_lock(root: &Path) -> Result<File, CacheError> {
    let path = root.join(LOCK_FILE);
    if !path.try_exists().map_err(|_| CacheError::Io)? {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options.open(&path).map_err(|error| {
            if error.kind() == std::io::ErrorKind::AlreadyExists {
                CacheError::Unsafe
            } else {
                CacheError::Io
            }
        })?;
        file.sync_all().map_err(|_| CacheError::Io)?;
        drop(file);
        sync_directory(root)?;
    }
    let file = file_identity::open_verified_regular(&path).ok_or(CacheError::Unsafe)?;
    if !file_identity::try_lock_exclusive(&file) {
        return Err(CacheError::Unsafe);
    }
    validate_directory_chain(root, true)?;
    Ok(file)
}

fn read_bounded_regular(path: &Path, max_bytes: usize) -> Result<Option<Vec<u8>>, CacheError> {
    let mut file = match file_identity::open_verified_regular(path) {
        Some(file) => file,
        None => match fs::symlink_metadata(path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            _ => return Err(CacheError::Unsafe),
        },
    };
    let metadata = file.metadata().map_err(|_| CacheError::Io)?;
    let length = usize::try_from(metadata.len()).map_err(|_| CacheError::Unsafe)?;
    if length == 0 || length > max_bytes {
        return Err(CacheError::Invalid);
    }
    let mut bytes = Vec::with_capacity(length);
    Read::by_ref(&mut file)
        .take((max_bytes as u64).saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|_| CacheError::Io)?;
    if bytes.len() != length || bytes.len() > max_bytes {
        return Err(CacheError::Invalid);
    }
    Ok(Some(bytes))
}

fn verify_optional_regular(path: &Path) -> Result<(), CacheError> {
    match file_identity::open_verified_regular(path) {
        Some(file) => {
            drop(file);
            Ok(())
        }
        None => match fs::symlink_metadata(path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            _ => Err(CacheError::Unsafe),
        },
    }
}

fn write_new_private_file(path: &Path, bytes: &[u8]) -> Result<(), CacheError> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path).map_err(|_| CacheError::Io)?;
    file.write_all(bytes).map_err(|_| CacheError::Io)?;
    file.sync_all().map_err(|_| CacheError::Io)?;
    drop(file);
    file_identity::open_verified_regular(path).ok_or(CacheError::Unsafe)?;
    Ok(())
}

fn remove_verified_regular_if_present(path: &Path) -> Result<(), CacheError> {
    let Some(file) = (match file_identity::open_verified_regular(path) {
        Some(file) => Some(file),
        None => match fs::symlink_metadata(path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            _ => return Err(CacheError::Unsafe),
        },
    }) else {
        return Ok(());
    };
    fs::remove_file(path).map_err(|_| CacheError::Io)?;
    drop(file);
    Ok(())
}

fn rename_verified(source: &Path, destination: &Path) -> Result<(), CacheError> {
    let source_file = file_identity::open_verified_regular(source).ok_or(CacheError::Unsafe)?;
    if destination.try_exists().map_err(|_| CacheError::Io)? {
        remove_verified_regular_if_present(destination)?;
    }
    atomic_rename(source, destination)?;
    let destination_file =
        file_identity::open_verified_regular(destination).ok_or(CacheError::Unsafe)?;
    if !file_identity::same_open_file_identity(&source_file, &destination_file) {
        return Err(CacheError::Unsafe);
    }
    Ok(())
}

#[cfg(unix)]
fn atomic_rename(source: &Path, destination: &Path) -> Result<(), CacheError> {
    fs::rename(source, destination).map_err(|_| CacheError::Io)
}

#[cfg(target_os = "windows")]
#[allow(unsafe_code)]
fn atomic_rename(source: &Path, destination: &Path) -> Result<(), CacheError> {
    use std::os::windows::ffi::OsStrExt;
    use windows::core::PCWSTR;
    use windows::Win32::Storage::FileSystem::{
        MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
    };

    let source: Vec<u16> = source.as_os_str().encode_wide().chain(Some(0)).collect();
    let destination: Vec<u16> = destination
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect();
    // SAFETY: both UTF-16 buffers are NUL-terminated and live for the complete
    // synchronous call.
    unsafe {
        MoveFileExW(
            PCWSTR(source.as_ptr()),
            PCWSTR(destination.as_ptr()),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
        .map_err(|_| CacheError::Io)
    }
}

#[cfg(not(any(unix, target_os = "windows")))]
fn atomic_rename(_source: &Path, _destination: &Path) -> Result<(), CacheError> {
    Err(CacheError::Unsafe)
}

#[cfg(unix)]
fn sync_directory(path: &Path) -> Result<(), CacheError> {
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|_| CacheError::Io)
}

#[cfg(target_os = "windows")]
fn sync_directory(_path: &Path) -> Result<(), CacheError> {
    // Every rename uses MOVEFILE_WRITE_THROUGH and every payload is synced
    // before publication. Windows does not expose a portable directory fsync.
    Ok(())
}

#[cfg(not(any(unix, target_os = "windows")))]
fn sync_directory(_path: &Path) -> Result<(), CacheError> {
    Err(CacheError::Unsafe)
}

#[cfg(feature = "webkit")]
#[derive(serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
struct CachedWebKitRule {
    action: CachedWebKitAction,
    trigger: CachedWebKitTrigger,
}

#[cfg(feature = "webkit")]
#[derive(serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
struct CachedWebKitAction {
    #[serde(rename = "type")]
    typ: CachedWebKitActionType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    selector: Option<String>,
}

#[cfg(feature = "webkit")]
#[derive(Clone, Copy, Eq, Hash, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
enum CachedWebKitActionType {
    Block,
    IgnorePreviousRules,
}

#[cfg(feature = "webkit")]
#[derive(serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
struct CachedWebKitTrigger {
    url_filter: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    url_filter_is_case_sensitive: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    if_domain: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    unless_domain: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    resource_type: Option<Vec<WebKitResourceType>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    load_type: Vec<CachedWebKitLoadType>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    if_top_url: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    unless_top_url: Option<Vec<String>>,
}

#[cfg(feature = "webkit")]
#[derive(Clone, Copy, Eq, Hash, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
enum CachedWebKitLoadType {
    FirstParty,
    ThirdParty,
}

#[cfg(feature = "webkit")]
fn validate_canonical_webkit_payload(
    payload: &[u8],
    expected_rules: usize,
) -> Option<(Arc<str>, usize)> {
    use serde::de::{Error, SeqAccess, Visitor};
    use std::io::Write;

    // Validate and compare one rule at a time. Rebuilding the whole rule tree
    // and another full JSON buffer caused large allocation peaks on warm hits.
    struct Rules<'a> {
        payload: &'a [u8],
        expected: usize,
    }
    impl<'de> Visitor<'de> for Rules<'_> {
        type Value = usize;
        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("canonical WebKit network rules")
        }
        fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<usize, A::Error> {
            let mut writer = CanonicalComparison {
                bytes: self.payload,
                offset: 0,
            };
            writer.write_all(b"[").map_err(A::Error::custom)?;
            let mut count = 0usize;
            let mut blocking = 0usize;
            let mut exceptions = false;
            while let Some(rule) = seq.next_element::<CachedWebKitRule>()? {
                if count >= self.expected || !valid_cached_webkit_rule(&rule) {
                    return Err(A::Error::custom("invalid cached rule"));
                }
                match rule.action.typ {
                    CachedWebKitActionType::Block if !exceptions => blocking += 1,
                    CachedWebKitActionType::Block => {
                        return Err(A::Error::custom("block after exception"))
                    }
                    CachedWebKitActionType::IgnorePreviousRules => exceptions = true,
                }
                if count != 0 {
                    writer.write_all(b",").map_err(A::Error::custom)?;
                }
                serde_json::to_writer(&mut writer, &rule).map_err(A::Error::custom)?;
                count += 1;
            }
            writer.write_all(b"]").map_err(A::Error::custom)?;
            if count != self.expected || blocking == 0 || writer.offset != self.payload.len() {
                return Err(A::Error::custom("invalid cached rule count or encoding"));
            }
            Ok(blocking)
        }
    }
    let mut decoder = serde_json::Deserializer::from_slice(payload);
    let blocking = serde::Deserializer::deserialize_seq(
        &mut decoder,
        Rules {
            payload,
            expected: expected_rules,
        },
    )
    .ok()?;
    decoder.end().ok()?;
    Some((Arc::from(std::str::from_utf8(payload).ok()?), blocking))
}

#[cfg(feature = "webkit")]
struct CanonicalComparison<'a> {
    bytes: &'a [u8],
    offset: usize,
}
#[cfg(feature = "webkit")]
impl std::io::Write for CanonicalComparison<'_> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let end = self
            .offset
            .checked_add(bytes.len())
            .ok_or_else(|| std::io::Error::other("encoding overflow"))?;
        if self.bytes.get(self.offset..end) != Some(bytes) {
            return Err(std::io::Error::other("noncanonical cached encoding"));
        }
        self.offset = end;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[cfg(feature = "webkit")]
fn valid_cached_webkit_rule(rule: &CachedWebKitRule) -> bool {
    const MAX_URL_FILTER_BYTES: usize = 8 * 1024;
    const MAX_PREDICATES_PER_FIELD: usize = 8 * 1024;
    if rule.action.selector.is_some()
        || rule.trigger.url_filter.is_empty()
        || rule.trigger.url_filter.len() > MAX_URL_FILTER_BYTES
        || !rule.trigger.url_filter.is_ascii()
        || (rule.trigger.if_domain.is_some() && rule.trigger.unless_domain.is_some())
        || (rule.trigger.if_top_url.is_some() && rule.trigger.unless_top_url.is_some())
        || !valid_string_predicates(
            rule.trigger.if_domain.as_deref(),
            MAX_PREDICATES_PER_FIELD,
            255,
        )
        || !valid_string_predicates(
            rule.trigger.unless_domain.as_deref(),
            MAX_PREDICATES_PER_FIELD,
            255,
        )
        || !valid_string_predicates(
            rule.trigger.if_top_url.as_deref(),
            MAX_PREDICATES_PER_FIELD,
            MAX_URL_FILTER_BYTES,
        )
        || !valid_string_predicates(
            rule.trigger.unless_top_url.as_deref(),
            MAX_PREDICATES_PER_FIELD,
            MAX_URL_FILTER_BYTES,
        )
        || !unique_bounded(
            rule.trigger.resource_type.as_deref(),
            WebKitResourceType::COUNT,
        )
        || (!rule.trigger.load_type.is_empty() && !unique_bounded(Some(&rule.trigger.load_type), 2))
    {
        return false;
    }
    true
}

#[cfg(feature = "webkit")]
fn valid_string_predicates(values: Option<&[String]>, max_count: usize, max_bytes: usize) -> bool {
    let Some(values) = values else {
        return true;
    };
    if values.is_empty() || values.len() > max_count {
        return false;
    }
    let mut unique = HashSet::with_capacity(values.len());
    values.iter().all(|value| {
        !value.is_empty()
            && value.len() <= max_bytes
            && value.is_ascii()
            && unique.insert(value.as_str())
    })
}

#[cfg(feature = "webkit")]
fn unique_bounded<T: Eq + std::hash::Hash>(values: Option<&[T]>, max_count: usize) -> bool {
    let Some(values) = values else {
        return true;
    };
    if values.is_empty() || values.len() > max_count {
        return false;
    }
    let mut unique = HashSet::with_capacity(values.len());
    values.iter().all(|value| unique.insert(value))
}

#[cfg(all(
    test,
    any(
        all(target_os = "windows", feature = "runtime"),
        all(not(target_os = "windows"), feature = "webkit")
    )
))]
pub(crate) fn private_test_root() -> tempfile::TempDir {
    tests::private_root()
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(super) fn private_root() -> tempfile::TempDir {
        // macOS exposes `/var` through an OS-managed symlink; cache admission
        // intentionally rejects every symlink in the path chain. Keeping test
        // roots below the checked-out workspace exercises the real invariant.
        let root = tempfile::tempdir_in(std::env::current_dir().unwrap()).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
        }
        #[cfg(windows)]
        set_windows_root_security(root.path(), false, false);
        root
    }

    #[cfg(windows)]
    #[allow(unsafe_code)]
    fn set_windows_root_security(path: &Path, administrator_owner: bool, public_writer: bool) {
        use std::os::windows::ffi::OsStrExt;
        use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
        use windows::core::PCWSTR;
        use windows::Win32::Foundation::HANDLE;
        use windows::Win32::Security::{
            AddAccessAllowedAceEx, CreateWellKnownSid, GetTokenInformation, InitializeAcl,
            InitializeSecurityDescriptor, SetFileSecurityW, SetSecurityDescriptorDacl,
            SetSecurityDescriptorOwner, TokenUser, WinBuiltinAdministratorsSid, WinLocalSystemSid,
            WinWorldSid, ACL, ACL_REVISION, CONTAINER_INHERIT_ACE, DACL_SECURITY_INFORMATION,
            OBJECT_INHERIT_ACE, OWNER_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION,
            PSECURITY_DESCRIPTOR, PSID, SECURITY_DESCRIPTOR, TOKEN_QUERY, TOKEN_USER,
        };
        use windows::Win32::Storage::FileSystem::FILE_ALL_ACCESS;
        use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

        let mut token = HANDLE::default();
        // SAFETY: all API outputs below use aligned storage of the declared size;
        // SIDs, ACL and descriptor stay alive until SetFileSecurityW returns.
        unsafe {
            OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &raw mut token).unwrap();
            let token = OwnedHandle::from_raw_handle(token.0);
            let mut user = [0usize; 128];
            let mut bytes = 0;
            GetTokenInformation(
                HANDLE(token.as_raw_handle()),
                TokenUser,
                Some(user.as_mut_ptr().cast()),
                std::mem::size_of_val(&user) as u32,
                &raw mut bytes,
            )
            .unwrap();
            let user_sid = (*user.as_ptr().cast::<TOKEN_USER>()).User.Sid;
            let mut known = [[0usize; 16]; 3];
            for (storage, kind) in
                known
                    .iter_mut()
                    .zip([WinBuiltinAdministratorsSid, WinLocalSystemSid, WinWorldSid])
            {
                let mut bytes = std::mem::size_of_val(storage) as u32;
                CreateWellKnownSid(
                    kind,
                    None,
                    Some(PSID(storage.as_mut_ptr().cast())),
                    &raw mut bytes,
                )
                .unwrap();
            }
            let mut acl = [0usize; 128];
            let acl_ptr = acl.as_mut_ptr().cast::<ACL>();
            InitializeAcl(acl_ptr, std::mem::size_of_val(&acl) as u32, ACL_REVISION).unwrap();
            for sid in [
                user_sid,
                PSID(known[0].as_mut_ptr().cast()),
                PSID(known[1].as_mut_ptr().cast()),
            ]
            .into_iter()
            .chain(public_writer.then_some(PSID(known[2].as_mut_ptr().cast())))
            {
                AddAccessAllowedAceEx(
                    acl_ptr,
                    ACL_REVISION,
                    CONTAINER_INHERIT_ACE | OBJECT_INHERIT_ACE,
                    FILE_ALL_ACCESS.0,
                    sid,
                )
                .unwrap();
            }
            let mut descriptor = SECURITY_DESCRIPTOR::default();
            let descriptor = PSECURITY_DESCRIPTOR((&raw mut descriptor).cast());
            InitializeSecurityDescriptor(descriptor, 1).unwrap();
            SetSecurityDescriptorOwner(
                descriptor,
                Some(if administrator_owner {
                    PSID(known[0].as_mut_ptr().cast())
                } else {
                    user_sid
                }),
                false,
            )
            .unwrap();
            SetSecurityDescriptorDacl(descriptor, true, Some(acl_ptr), false).unwrap();
            let path: Vec<_> = path.as_os_str().encode_wide().chain(Some(0)).collect();
            assert!(SetFileSecurityW(
                PCWSTR(path.as_ptr()),
                OWNER_SECURITY_INFORMATION
                    | DACL_SECURITY_INFORMATION
                    | PROTECTED_DACL_SECURITY_INFORMATION,
                descriptor,
            )
            .as_bool());
        }
    }

    #[cfg(windows)]
    #[test]
    fn windows_cache_rejects_public_writers() {
        let root = private_root();
        let config = CompiledArtifactCacheConfig::new(root.path()).unwrap();
        assert!(PersistentArtifactCache::open(&config).is_ok());
        set_windows_root_security(root.path(), false, true);
        assert!(matches!(
            PersistentArtifactCache::open(&config),
            Err(CacheError::Unsafe)
        ));
    }

    #[cfg(windows)]
    #[test]
    #[allow(unsafe_code)]
    fn windows_cache_accepts_administrator_owner_only_when_elevated() {
        use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
        use windows::Win32::Foundation::HANDLE;
        use windows::Win32::Security::{
            CreateWellKnownSid, GetTokenInformation, TokenElevation, WinBuiltinAdministratorsSid,
            WinWorldSid, PSID, TOKEN_ELEVATION, TOKEN_QUERY,
        };
        use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

        let root = private_root();
        let mut token = HANDLE::default();
        let mut admin = [0usize; 16];
        let mut world = [0usize; 16];
        // SAFETY: every output uses aligned, correctly sized live storage.
        let elevated = unsafe {
            OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &raw mut token).unwrap();
            let token = OwnedHandle::from_raw_handle(token.0);
            let token = HANDLE(token.as_raw_handle());
            for (storage, kind) in [
                (&mut admin, WinBuiltinAdministratorsSid),
                (&mut world, WinWorldSid),
            ] {
                let mut bytes = std::mem::size_of_val(storage) as u32;
                CreateWellKnownSid(
                    kind,
                    None,
                    Some(PSID(storage.as_mut_ptr().cast())),
                    &raw mut bytes,
                )
                .unwrap();
            }
            let mut elevation = TOKEN_ELEVATION::default();
            let mut bytes = 0;
            GetTokenInformation(
                token,
                TokenElevation,
                Some((&raw mut elevation).cast()),
                std::mem::size_of_val(&elevation) as u32,
                &raw mut bytes,
            )
            .unwrap();
            let admin = PSID(admin.as_mut_ptr().cast());
            let world = PSID(world.as_mut_ptr().cast());
            assert_eq!(
                trusted_windows_cache_owner(admin, world, token),
                elevation.TokenIsElevated != 0
            );
            assert!(!trusted_windows_cache_owner(world, admin, token));
            assert!(!trusted_windows_cache_owner(
                admin,
                world,
                HANDLE::default()
            ));
            elevation.TokenIsElevated != 0
        };
        if elevated {
            set_windows_root_security(root.path(), true, false);
            let config = CompiledArtifactCacheConfig::new(root.path()).unwrap();
            assert!(PersistentArtifactCache::open(&config).is_ok());
            set_windows_root_security(root.path(), true, true);
            assert!(matches!(
                PersistentArtifactCache::open(&config),
                Err(CacheError::Unsafe)
            ));
        }
    }

    #[cfg(feature = "webkit")]
    fn key(byte: u8) -> CacheKey {
        CacheKey([byte; 32])
    }

    #[cfg(feature = "webkit")]
    fn coverage() -> ContentRuleCoverage {
        ContentRuleCoverage {
            source_rules: 2,
            accepted_rules: 2,
            rejected_rules: 0,
            platform_omitted_rules: 0,
            platform_approximated_rules: 0,
            platform_resource_approximated_rules: 0,
            platform_source_kind_approximated_rules: 0,
            platform_attribution_approximated_rules: 0,
            blocking_rule_entries: 1,
        }
    }

    #[cfg(feature = "webkit")]
    #[cfg(feature = "webkit")]
    fn load_webkit_bytes(
        record: &[u8],
        cache_key: CacheKey,
    ) -> (tempfile::TempDir, Option<LoadedArtifact>) {
        let root = private_root();
        let path = root.path().join(CURRENT_FILE);
        write_new_private_file(&path, record).unwrap();
        let loaded = load_webkit_record(&path, cache_key, CompileLimits::default()).unwrap();
        (root, loaded)
    }

    #[cfg(feature = "webkit")]
    #[test]
    fn webkit_text_is_validated_only_when_read() {
        let payload = br#"[{"trigger":{"url-filter":".*"},"action":{"type":"block"}}]"#;
        let mut artifact = Sha256::new();
        artifact.update(WEBKIT_DIGEST_DOMAIN);
        artifact.update(WEBKIT_ARTIFACT_FORMAT_VERSION.to_be_bytes());
        artifact.update(payload);
        let record = encode_record(
            key(5),
            CompileTarget::WebKit,
            [5; 32],
            artifact.finalize().into(),
            coverage(),
            1,
            payload,
        )
        .unwrap();
        let (_root, loaded) = load_webkit_bytes(&record, key(5));
        let Some(LoadedArtifact::WebKit { encoded, .. }) = loaded else {
            panic!("an intact record must load without reading its text");
        };
        assert_eq!(encoded.len(), payload.len());
        assert!(encoded.load().is_none());
    }

    #[cfg(feature = "webkit")]
    #[test]
    fn webkit_text_changed_on_disk_after_loading_is_refused() {
        let record = webkit_record(key(6), 6);
        let (root, loaded) = load_webkit_bytes(&record, key(6));
        let Some(LoadedArtifact::WebKit { encoded, .. }) = loaded else {
            panic!("WebKit record did not load");
        };
        let mut file = OpenOptions::new()
            .write(true)
            .open(root.path().join(CURRENT_FILE))
            .unwrap();
        file.seek(SeekFrom::Start(RECORD_HEADER_BYTES as u64 + 40))
            .unwrap();
        file.write_all(b"X").unwrap();
        drop(file);
        assert!(encoded.load().is_none());
    }

    #[cfg(feature = "webkit")]
    #[test]
    fn webkit_record_with_a_corrupt_checksum_is_skipped() {
        let mut record = webkit_record(key(7), 7);
        let last = record.len() - 1;
        record[last] ^= 1;
        let (_root, loaded) = load_webkit_bytes(&record, key(7));
        assert!(loaded.is_none());
    }

    fn webkit_record(cache_key: CacheKey, policy: u8) -> Vec<u8> {
        let payload = br#"[{"action":{"type":"block"},"trigger":{"url-filter":"ads\\.invalid"}}]"#;
        let mut artifact = Sha256::new();
        artifact.update(WEBKIT_DIGEST_DOMAIN);
        artifact.update(WEBKIT_ARTIFACT_FORMAT_VERSION.to_be_bytes());
        artifact.update(payload);
        encode_record(
            cache_key,
            CompileTarget::WebKit,
            [policy; 32],
            artifact.finalize().into(),
            coverage(),
            1,
            payload,
        )
        .unwrap()
    }

    #[cfg(feature = "runtime")]
    fn runtime_record(cache_key: CacheKey) -> (Vec<u8>, ContentRuleCoverage) {
        let compiled = crate::Compiler::default()
            .compile(
                CompileTarget::Runtime,
                vec![crate::FilterSource::new(
                    crate::SourceId::new("cache-runtime").unwrap(),
                    crate::SourceFormat::Standard,
                    concat!("||ads.invalid^\n", "@@||allowed.invalid^\n").to_owned(),
                )],
            )
            .unwrap();
        let report = compiled.report();
        let coverage = ContentRuleCoverage {
            source_rules: report.candidate_rules() as u64,
            accepted_rules: report.accepted_rules() as u64,
            rejected_rules: report.rejected_rules() as u64,
            platform_omitted_rules: report.runtime_omitted_rules() as u64,
            platform_approximated_rules: report.runtime_approximated_rules() as u64,
            platform_resource_approximated_rules: report.runtime_resource_approximated_rules()
                as u64,
            platform_source_kind_approximated_rules: report.runtime_source_kind_approximated_rules()
                as u64,
            platform_attribution_approximated_rules: 0,
            blocking_rule_entries: report.native_blocking_rule_entries() as u64,
        };
        let payload = compiled.serialize_runtime_engine().unwrap();
        (
            encode_record(
                cache_key,
                CompileTarget::Runtime,
                *compiled.digest().as_bytes(),
                [0; 32],
                coverage,
                0,
                &payload,
            )
            .unwrap(),
            coverage,
        )
    }

    #[cfg(all(feature = "runtime", feature = "webkit"))]
    fn compiled_artifact(
        target: CompileTarget,
        source_id: &str,
    ) -> (CompiledRules, Arc<ContentRules>) {
        let compiled = crate::Compiler::default()
            .compile(
                target,
                vec![crate::FilterSource::new(
                    crate::SourceId::new(source_id).unwrap(),
                    crate::SourceFormat::Standard,
                    "||ads.zephium.invalid^$script".to_owned(),
                )],
            )
            .unwrap();
        let rules = crate::worker::adapt_rules(compiled.clone()).unwrap();
        (compiled, rules)
    }

    #[test]
    fn key_is_canonical_by_source_id_and_bound_to_every_input() {
        let limits = CompileLimits::default();
        let first = CacheKey::for_catalog(
            CompileTarget::WebKit,
            limits,
            [("z", 0, "one"), ("a", 1, "two")],
        );
        let reordered = CacheKey::for_catalog(
            CompileTarget::WebKit,
            limits,
            [("a", 1, "two"), ("z", 0, "one")],
        );
        let changed = CacheKey::for_catalog(
            CompileTarget::WebKit,
            limits,
            [("a", 1, "two"), ("z", 0, "three")],
        );
        assert_eq!(first, reordered);
        assert_ne!(first, changed);
        assert_ne!(
            first,
            CacheKey::for_catalog(
                CompileTarget::Runtime,
                limits,
                [("a", 1, "two"), ("z", 0, "one")],
            )
        );
    }

    #[cfg(all(feature = "runtime", feature = "webkit"))]
    #[test]
    fn encoded_records_have_one_exact_version_header_and_round_trip() {
        for (target, source_id, cache_key) in [
            (CompileTarget::Runtime, "cache-runtime-round-trip", key(1)),
            (CompileTarget::WebKit, "cache-webkit-round-trip", key(2)),
        ] {
            let (compiled, rules) = compiled_artifact(target, source_id);
            let record = encode_compiled_record(cache_key, &compiled, &rules).unwrap();

            assert_eq!(&record[..8], CACHE_MAGIC);
            assert_eq!(
                u32::from_be_bytes(record[8..12].try_into().unwrap()),
                CACHE_FORMAT_VERSION
            );
            assert_eq!(
                u32::from_be_bytes(record[12..16].try_into().unwrap()),
                POLICY_FORMAT_VERSION
            );
            assert_eq!(
                u32::from_be_bytes(record[16..20].try_into().unwrap()),
                WEBKIT_ARTIFACT_FORMAT_VERSION
            );
            assert_eq!(record[20], target_tag(target));

            let decoded = decode_record(&record, cache_key, target).unwrap();
            assert_eq!(decoded.target, target);
            assert_eq!(decoded.policy_digest, *compiled.digest().as_bytes());
            match target {
                CompileTarget::Runtime => {
                    assert!(validate_payload(decoded, CompileLimits::default()).is_some());
                }
                CompileTarget::WebKit => {
                    let (_root, loaded) = load_webkit_bytes(&record, cache_key);
                    let Some(LoadedArtifact::WebKit { encoded, .. }) = loaded else {
                        panic!("WebKit record did not load");
                    };
                    assert!(encoded.load().is_some());
                }
            }
        }
    }

    #[cfg(all(feature = "runtime", feature = "webkit"))]
    #[test]
    fn persistent_reopen_warm_hits_runtime_and_webkit_artifacts() {
        let root = private_root();
        let config = CompiledArtifactCacheConfig::new(root.path()).unwrap();
        let runtime = compiled_artifact(CompileTarget::Runtime, "cache-runtime-reopen");
        let webkit = compiled_artifact(CompileTarget::WebKit, "cache-webkit-reopen");
        let cache = PersistentArtifactCache::open(&config).unwrap();
        cache.store(key(3), &runtime.0, &runtime.1).unwrap();
        cache.store(key(4), &webkit.0, &webkit.1).unwrap();
        drop(cache);

        let reopened = PersistentArtifactCache::open(&config).unwrap();
        assert!(matches!(
            reopened
                .load(key(3), CompileTarget::Runtime, CompileLimits::default())
                .unwrap(),
            Some(LoadedArtifact::Runtime { .. })
        ));
        assert!(matches!(
            reopened
                .load(key(4), CompileTarget::WebKit, CompileLimits::default())
                .unwrap(),
            Some(LoadedArtifact::WebKit { .. })
        ));
    }

    #[cfg(feature = "webkit")]
    #[test]
    fn record_rejects_corruption_truncation_versions_and_cross_target_replay() {
        let record = webkit_record(key(1), 7);
        assert!(decode_record(&record, key(1), CompileTarget::WebKit).is_some());
        assert!(decode_record(&record, key(1), CompileTarget::Runtime).is_none());
        assert!(decode_record(&record, key(2), CompileTarget::WebKit).is_none());
        for length in [0, 1, RECORD_HEADER_BYTES, record.len() - 1] {
            assert!(decode_record(&record[..length], key(1), CompileTarget::WebKit).is_none());
        }
        let mut corrupt = record.clone();
        corrupt[RECORD_HEADER_BYTES] ^= 1;
        assert!(decode_record(&corrupt, key(1), CompileTarget::WebKit).is_none());
        let mut wrong_version = record;
        wrong_version[8..12].copy_from_slice(&(CACHE_FORMAT_VERSION + 1).to_be_bytes());
        let checksum: [u8; 32] = Sha256::digest(&wrong_version[..wrong_version.len() - 32]).into();
        let length = wrong_version.len();
        wrong_version[length - 32..].copy_from_slice(&checksum);
        assert!(decode_record(&wrong_version, key(1), CompileTarget::WebKit).is_none());
    }

    #[cfg(feature = "webkit")]
    #[test]
    fn webkit_loader_requires_exact_canonical_typed_structure() {
        let valid = webkit_record(key(1), 7);
        let (_root, loaded) = load_webkit_bytes(&valid, key(1));
        let Some(LoadedArtifact::WebKit { encoded, .. }) = loaded else {
            panic!("WebKit record did not load");
        };
        assert!(encoded.load().is_some());

        let forbidden =
            br#"[{"action":{"type":"css-display-none","selector":"body"},"trigger":{"url-filter":".*"}}]"#;
        assert!(validate_canonical_webkit_payload(forbidden, 1).is_none());
        let reordered = br#"[{"trigger":{"url-filter":".*"},"action":{"type":"block"}}]"#;
        assert!(validate_canonical_webkit_payload(reordered, 1).is_none());
    }

    #[cfg(feature = "runtime")]
    #[test]
    fn runtime_loader_revalidates_preparation_and_structural_block_count() {
        let cache_key = CacheKey([4; 32]);
        let (record, coverage) = runtime_record(cache_key);
        let decoded = decode_record(&record, cache_key, CompileTarget::Runtime).unwrap();
        let rules = match validate_payload(decoded, CompileLimits::default()).unwrap() {
            LoadedArtifact::Runtime { rules, .. } => rules,
            #[cfg(feature = "webkit")]
            LoadedArtifact::WebKit { .. } => panic!("runtime cache returned a WebKit artifact"),
        };
        let decision = rules
            .evaluate_source_independent(crate::NetworkRequest::source_independent(
                "https://ads.invalid/banner.js",
                crate::ResourceType::Script,
                crate::RequestMethod::Get,
            ))
            .unwrap();
        assert_eq!(decision.action(), crate::NetworkAction::Block);

        let payload = decode_record(&record, cache_key, CompileTarget::Runtime)
            .unwrap()
            .payload;
        let mut inconsistent = coverage;
        inconsistent.blocking_rule_entries += 1;
        let forged = encode_record(
            cache_key,
            CompileTarget::Runtime,
            [9; 32],
            [0; 32],
            inconsistent,
            0,
            payload,
        )
        .unwrap();
        let decoded = decode_record(&forged, cache_key, CompileTarget::Runtime).unwrap();
        assert!(validate_payload(decoded, CompileLimits::default()).is_none());
    }

    #[test]
    fn exclusive_lock_disables_a_second_process_owner() {
        let root = private_root();
        let config = CompiledArtifactCacheConfig::new(root.path()).unwrap();
        let first = PersistentArtifactCache::open(&config).unwrap();
        assert!(PersistentArtifactCache::open(&config).is_err());
        drop(first);
        assert!(PersistentArtifactCache::open(&config).is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn unsafe_root_permissions_and_symlinks_disable_cache() {
        use std::os::unix::fs::{symlink, PermissionsExt};

        let root = private_root();
        fs::set_permissions(root.path(), fs::Permissions::from_mode(0o755)).unwrap();
        let config = CompiledArtifactCacheConfig::new(root.path()).unwrap();
        assert!(PersistentArtifactCache::open(&config).is_err());

        let root = private_root();
        symlink(root.path().join("missing"), root.path().join(STAGE_FILE)).unwrap();
        let config = CompiledArtifactCacheConfig::new(root.path()).unwrap();
        assert!(PersistentArtifactCache::open(&config).is_err());
    }

    #[test]
    fn hardlinks_and_unknown_entries_disable_cache() {
        let root = private_root();
        fs::write(root.path().join(CURRENT_FILE), b"cache").unwrap();
        let alias_root = private_root();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(
                root.path().join(CURRENT_FILE),
                fs::Permissions::from_mode(0o600),
            )
            .unwrap();
        }
        fs::hard_link(
            root.path().join(CURRENT_FILE),
            alias_root.path().join("alias.bin"),
        )
        .unwrap();
        let config = CompiledArtifactCacheConfig::new(root.path()).unwrap();
        assert!(PersistentArtifactCache::open(&config).is_err());

        let root = private_root();
        fs::write(root.path().join("foreign"), b"x").unwrap();
        let config = CompiledArtifactCacheConfig::new(root.path()).unwrap();
        assert!(PersistentArtifactCache::open(&config).is_err());
    }

    #[cfg(feature = "webkit")]
    #[test]
    fn current_previous_and_stale_stage_recover_every_rotation_boundary() {
        for fault in [
            PersistFault::AfterStageSync,
            PersistFault::AfterPreviousRemoval,
            PersistFault::AfterCurrentRotation,
            PersistFault::AfterCurrentInstall,
        ] {
            let root = private_root();
            let config = CompiledArtifactCacheConfig::new(root.path()).unwrap();
            let cache = PersistentArtifactCache::open(&config).unwrap();
            let old = webkit_record(key(1), 1);
            cache.persist_record(&old, PersistFault::None).unwrap();
            let new = webkit_record(key(2), 2);
            assert!(cache.persist_record(&new, fault).is_err());
            drop(cache);

            let recovered = PersistentArtifactCache::open(&config).unwrap();
            let old_hit = recovered
                .load(key(1), CompileTarget::WebKit, CompileLimits::default())
                .unwrap()
                .is_some();
            let new_hit = recovered
                .load(key(2), CompileTarget::WebKit, CompileLimits::default())
                .unwrap()
                .is_some();
            assert!(old_hit || new_hit);
        }
    }

    #[cfg(feature = "webkit")]
    #[test]
    fn corrupt_current_falls_back_to_exact_previous() {
        let root = private_root();
        let config = CompiledArtifactCacheConfig::new(root.path()).unwrap();
        let cache = PersistentArtifactCache::open(&config).unwrap();
        cache
            .persist_record(&webkit_record(key(1), 1), PersistFault::None)
            .unwrap();
        cache
            .persist_record(&webkit_record(key(2), 2), PersistFault::None)
            .unwrap();
        drop(cache);
        fs::write(root.path().join(CURRENT_FILE), b"truncated").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(
                root.path().join(CURRENT_FILE),
                fs::Permissions::from_mode(0o600),
            )
            .unwrap();
        }

        let cache = PersistentArtifactCache::open(&config).unwrap();
        assert!(cache
            .load(key(1), CompileTarget::WebKit, CompileLimits::default())
            .unwrap()
            .is_some());

        let oversized = File::create(root.path().join(CURRENT_FILE)).unwrap();
        oversized
            .set_len((max_record_bytes(CompileTarget::WebKit) as u64).saturating_add(1))
            .unwrap();
        oversized.sync_all().unwrap();
        drop(oversized);
        assert!(cache
            .load(key(1), CompileTarget::WebKit, CompileLimits::default())
            .unwrap()
            .is_some());
    }
}
