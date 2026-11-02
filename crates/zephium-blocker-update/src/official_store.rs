//! Durable two-source snapshots and scheduling hints for official HTTPS lists.
use crate::storage_io::{
    atomic_write, create_private_directory, read_bounded_regular, remove_verified_regular,
    sync_directory, StoreError,
};
use crate::types::{ActivatedCatalog, CatalogIdentity};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use zephium_blocker::Sha256;
use zephium_blocker::{PolicyCatalog, PolicySource, SourceFormat, SourceId, StaticPolicyCatalog};
use zephium_update_transport::official_filters::FilterValidators;

const MAX_SOURCE: u64 = 16 * 1024 * 1024;
const MAX_STATE: u64 = 64 * 1024;
const MAX_OBJECTS: usize = 16;
const DAY: u64 = 24 * 60 * 60;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Source {
    pub sha256: [u8; 32],
    pub bytes: u64,
    pub rules: u64,
    pub version: String,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
}
impl Source {
    pub fn validators(&self) -> FilterValidators {
        FilterValidators {
            etag: self.etag.clone(),
            last_modified: self.last_modified.clone(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Package {
    pub identity: CatalogIdentity,
    pub sources: [Source; 2],
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Record {
    schema: u32,
    pub highest_revision: u64,
    pub jitter: u64,
    pub due: u64,
    pub attempted: Option<u64>,
    pub checked: Option<u64>,
    pub failure_streak: u8,
    pub verification: u64,
    pub current: Option<Package>,
    pub previous: Option<Package>,
    pub candidate: Option<Package>,
    pub rejected: Option<Package>,
}

pub(crate) struct OfficialStore {
    _lock: crate::cache_lock::CacheLock,
    root: PathBuf,
    pub record: Record,
    sealed: bool,
    needs_checkpoint: bool,
}

impl OfficialStore {
    pub fn open(root: PathBuf, seed_revision: u64, now: u64) -> Result<Self, StoreError> {
        create_private_directory(&root)?;
        let lock = crate::cache_lock::CacheLock::acquire(&root.join("lock-v1"))
            .ok_or(StoreError::LockUnavailable)?;
        let decode = |path: &Path| -> Result<Record, StoreError> {
            let bytes = read_bounded_regular(path, MAX_STATE)?;
            let record: Record =
                serde_json::from_slice(&bytes).map_err(|_| StoreError::StateCorrupt)?;
            validate_record(&record, now)?;
            Ok(record)
        };
        let primary = decode(&root.join("state.json"));
        let needs_checkpoint = primary.is_err();
        let record = match primary {
            Ok(record) => record,
            Err(StoreError::NotFound) if root.join("state.previous.json").exists() => {
                decode(&root.join("state.previous.json"))?
            }
            Err(StoreError::NotFound) => Record {
                schema: 1,
                highest_revision: seed_revision,
                jitter: jitter(),
                due: now,
                attempted: None,
                checked: None,
                failure_streak: 0,
                verification: 0,
                current: None,
                previous: None,
                candidate: None,
                rejected: None,
            },
            Err(StoreError::StateCorrupt) => decode(&root.join("state.previous.json"))?,
            Err(error) => return Err(error),
        };
        let store = Self {
            _lock: lock,
            root,
            record,
            sealed: false,
            needs_checkpoint,
        };
        store.collect()?;
        Ok(store)
    }

    pub fn catalog(&self, package: &Package) -> ActivatedCatalog {
        let root = self.root.clone();
        let package = package.clone();
        ActivatedCatalog {
            identity: package.identity.clone(),
            catalog: PolicyCatalog::deferred_reloadable(
                package.identity.manifest_sha256,
                move || {
                    let mut sources = Vec::with_capacity(2);
                    for (index, source) in package.sources.iter().enumerate() {
                        let bytes = read_source(&root, source).map_err(|_| {
                            zephium_blocker::BlockerCompileFailure::SourceUnavailable
                        })?;
                        let text = String::from_utf8(bytes)
                            .map_err(|_| zephium_blocker::BlockerCompileFailure::InvalidSource)?;
                        let (version, rules) = validate_source(index, &text)
                            .map_err(|_| zephium_blocker::BlockerCompileFailure::InvalidSource)?;
                        if version != source.version || rules != source.rules {
                            return Err(zephium_blocker::BlockerCompileFailure::SourceUnavailable);
                        }
                        sources.push(PolicySource::new(
                            SourceId::new(if index == 0 {
                                "easylist"
                            } else {
                                "easyprivacy"
                            })
                            .expect("fixed source id"),
                            SourceFormat::Standard,
                            Arc::from(text),
                        ));
                    }
                    StaticPolicyCatalog::new(sources)
                        .map_err(|_| zephium_blocker::BlockerCompileFailure::InvalidSource)
                },
            ),
        }
    }

    /// Check retained source identity with a small fixed buffer before choosing
    /// the startup current/LKG snapshot. This does not parse or inflate lists.
    pub fn intact(&self, package: &Package) -> bool {
        use std::io::Read;
        package.sources.iter().all(|source| {
            let Some(mut file) = crate::file_identity::open_verified_regular(
                &self.root.join(object_name(&source.sha256)),
            ) else {
                return false;
            };
            let Ok(metadata) = file.metadata() else {
                return false;
            };
            if metadata.len() != source.bytes {
                return false;
            }
            let mut digest = Sha256::new();
            let mut total = 0u64;
            let mut buffer = [0u8; 65536];
            loop {
                match file.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(bytes) => {
                        total = total.saturating_add(bytes as u64);
                        if total > source.bytes {
                            return false;
                        }
                        digest.update(&buffer[..bytes]);
                    }
                    Err(_) => return false,
                }
            }
            total == source.bytes && digest.finalize() == source.sha256
        })
    }

    pub fn cached_bytes(&self, source: &Source) -> Result<Vec<u8>, StoreError> {
        read_source(&self.root, source)
    }

    pub fn put_source(
        &self,
        index: usize,
        bytes: &[u8],
        validators: FilterValidators,
        previous: Option<&Source>,
    ) -> Result<Source, StoreError> {
        if self.sealed || bytes.len() as u64 > MAX_SOURCE {
            return Err(StoreError::ActivationSealed);
        }
        let text = std::str::from_utf8(bytes).map_err(|_| StoreError::InvalidPackage)?;
        let (version, rules) = validate_source(index, text)?;
        if previous.is_some_and(|old| rules < old.rules / 2 || (bytes.len() as u64) < old.bytes / 2)
        {
            return Err(StoreError::InvalidPackage);
        }
        let sha256 = Sha256::digest(bytes);
        let path = self.root.join(object_name(&sha256));
        let identical = read_bounded_regular(&path, MAX_SOURCE).is_ok_and(|stored| stored == bytes);
        if !identical {
            atomic_write(&path, bytes)?;
        }
        Ok(Source {
            sha256,
            bytes: bytes.len() as u64,
            rules,
            version,
            etag: validators.etag,
            last_modified: validators.last_modified,
        })
    }

    pub fn new_package(&self, sources: [Source; 2], now: u64) -> Result<Package, StoreError> {
        let revision = self
            .record
            .highest_revision
            .checked_add(1)
            .ok_or(StoreError::StateCorrupt)?;
        let manifest = serde_json::to_vec(&(
            1u32,
            revision,
            now,
            sources
                .iter()
                .map(|s| (s.sha256, s.bytes))
                .collect::<Vec<_>>(),
        ))
        .map_err(|_| StoreError::StateCorrupt)?;
        Ok(Package {
            identity: CatalogIdentity {
                revision,
                manifest_sha256: Sha256::digest(manifest),
                created_unix: now,
                expires_unix: now.saturating_add(7 * DAY),
                source_count: 2,
                source_bytes: sources.iter().map(|s| s.bytes).sum(),
            },
            sources,
        })
    }

    pub fn persist(&mut self, next: Record) -> Result<(), StoreError> {
        if self.sealed {
            return Err(StoreError::ActivationSealed);
        }
        validate_record(
            &next,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|t| t.as_secs())
                .unwrap_or(0),
        )?;
        if next.highest_revision < self.record.highest_revision {
            return Err(StoreError::Rollback);
        }
        if !self.needs_checkpoint && next == self.record {
            return Ok(());
        }
        let old = serde_json::to_vec(&self.record).map_err(|_| StoreError::StateCorrupt)?;
        let bytes = serde_json::to_vec(&next).map_err(|_| StoreError::StateCorrupt)?;
        if bytes.len() as u64 > MAX_STATE {
            return Err(StoreError::StateCorrupt);
        }
        let result = atomic_write(&self.root.join("state.previous.json"), &old)
            .and_then(|_| atomic_write(&self.root.join("state.json"), &bytes));
        if result.is_err() {
            self.sealed = true;
            return result;
        }
        self.record = next;
        self.needs_checkpoint = false;
        Ok(())
    }

    pub fn collect(&self) -> Result<(), StoreError> {
        let keep: HashSet<_> = [
            self.record.current.as_ref(),
            self.record.previous.as_ref(),
            self.record.candidate.as_ref(),
            self.record.rejected.as_ref(),
        ]
        .into_iter()
        .flatten()
        .flat_map(|p| p.sources.iter().map(|s| object_name(&s.sha256)))
        .collect();
        let mut count = 0;
        for entry in std::fs::read_dir(&self.root).map_err(|_| StoreError::Io)? {
            count += 1;
            if count > MAX_OBJECTS + 8 {
                return Err(StoreError::UnsafePath);
            }
            let entry = entry.map_err(|_| StoreError::Io)?;
            let name = entry
                .file_name()
                .into_string()
                .map_err(|_| StoreError::UnsafePath)?;
            if matches!(
                name.as_str(),
                "state.json" | "state.previous.json" | "lock-v1"
            ) {
                continue;
            }
            let hash_name = |text: &str| {
                text.len() == 64
                    && text
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            };
            if name.strip_suffix(".txt").is_some_and(hash_name) {
                if !keep.contains(&name) {
                    remove_verified_regular(&entry.path())?;
                }
            } else if name == ".state.json.stage"
                || name == ".state.previous.json.stage"
                || name
                    .strip_prefix('.')
                    .and_then(|n| n.strip_suffix(".txt.stage"))
                    .is_some_and(hash_name)
            {
                remove_verified_regular(&entry.path())?;
            } else {
                return Err(StoreError::UnsafePath);
            }
        }
        sync_directory(&self.root)
    }
}

fn object_name(hash: &[u8; 32]) -> String {
    use std::fmt::Write;
    let mut output = String::with_capacity(68);
    for byte in hash {
        let _ = write!(output, "{byte:02x}");
    }
    output.push_str(".txt");
    output
}
fn read_source(root: &Path, source: &Source) -> Result<Vec<u8>, StoreError> {
    let bytes = read_bounded_regular(&root.join(object_name(&source.sha256)), source.bytes)?;
    if bytes.len() as u64 != source.bytes || Sha256::digest(&bytes) != source.sha256 {
        return Err(StoreError::ObjectCorrupt);
    }
    Ok(bytes)
}
fn validate_record(record: &Record, now: u64) -> Result<(), StoreError> {
    if record.schema != 1
        || record.highest_revision == 0
        || record.jitter > 6 * 60 * 60
        || record.due > now.saturating_add(31 * DAY)
        || record.failure_streak > 8
        || record.verification == u64::MAX
        || record
            .checked
            .is_some_and(|t| t > now.saturating_add(31 * DAY))
        || record
            .attempted
            .is_some_and(|t| t > now.saturating_add(31 * DAY))
    {
        return Err(StoreError::StateCorrupt);
    }
    for package in [
        record.current.as_ref(),
        record.previous.as_ref(),
        record.candidate.as_ref(),
        record.rejected.as_ref(),
    ]
    .into_iter()
    .flatten()
    {
        if package.identity.revision == 0
            || package.identity.revision > record.highest_revision
            || package.identity.source_count != 2
            || package.identity.created_unix >= package.identity.expires_unix
            || package.identity.expires_unix
                != package.identity.created_unix.saturating_add(7 * DAY)
            || package.identity.source_bytes != package.sources.iter().map(|s| s.bytes).sum::<u64>()
        {
            return Err(StoreError::StateCorrupt);
        }
        let manifest = serde_json::to_vec(&(
            1u32,
            package.identity.revision,
            package.identity.created_unix,
            package
                .sources
                .iter()
                .map(|s| (s.sha256, s.bytes))
                .collect::<Vec<_>>(),
        ))
        .map_err(|_| StoreError::StateCorrupt)?;
        if package.identity.manifest_sha256 != Sha256::digest(manifest) {
            return Err(StoreError::StateCorrupt);
        }
        for source in &package.sources {
            if source.bytes == 0
                || source.bytes > MAX_SOURCE
                || source.rules < 1000
                || source.rules > 500000
                || source.version.len() != 12
                || !source.version.bytes().all(|b| b.is_ascii_digit())
                || [&source.etag, &source.last_modified]
                    .into_iter()
                    .flatten()
                    .any(|s| {
                        s.is_empty() || s.len() > 512 || !s.bytes().all(|b| (32..=126).contains(&b))
                    })
            {
                return Err(StoreError::StateCorrupt);
            }
        }
    }
    Ok(())
}
fn validate_source(index: usize, text: &str) -> Result<(String, u64), StoreError> {
    if text.len() < 256 * 1024
        || text.len() as u64 > MAX_SOURCE
        || !matches!(
            text.lines().next(),
            Some("[Adblock Plus 1.1]" | "[Adblock Plus 2.0]")
        )
        || text.contains('\0')
    {
        return Err(StoreError::InvalidPackage);
    }
    let head = text.lines().take(32).collect::<Vec<_>>();
    let title = if index == 0 {
        "! Title: EasyList"
    } else {
        "! Title: EasyPrivacy"
    };
    if !head.contains(&title) || !head.contains(&"! Homepage: https://easylist.to/") {
        return Err(StoreError::InvalidPackage);
    }
    let version = head
        .iter()
        .find_map(|line| line.strip_prefix("! Version: "))
        .filter(|s| s.len() == 12 && s.bytes().all(|b| b.is_ascii_digit()))
        .ok_or(StoreError::InvalidPackage)?
        .to_owned();
    let rules = text
        .lines()
        .filter(|l| !l.is_empty() && !l.starts_with('!') && !l.starts_with('['))
        .count() as u64;
    if !(1000..=500000).contains(&rules) {
        return Err(StoreError::InvalidPackage);
    }
    Ok((version, rules))
}
fn jitter() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|t| (t.as_nanos() as u64) ^ u64::from(std::process::id()))
        .unwrap_or(0)
        % (6 * 60 * 60 + 1)
}

#[cfg(test)]
impl OfficialStore {
    pub(super) fn root_for_test(&self) -> &Path {
        &self.root
    }
}
#[cfg(test)]
pub(super) fn object_name_for_test(hash: &[u8; 32]) -> String {
    object_name(hash)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn source(index: usize) -> Vec<u8> {
        let title = if index == 0 {
            "EasyList"
        } else {
            "EasyPrivacy"
        };
        let header = if index == 0 { "2.0" } else { "1.1" };
        let mut text=format!("[Adblock Plus {header}]\n! Version: 202609300001\n! Title: {title}\n! Homepage: https://easylist.to/\n");
        for n in 0..12000 {
            text.push_str(&format!("||ads{n}.example.invalid^\n"));
        }
        text.into_bytes()
    }
    fn root() -> tempfile::TempDir {
        tempfile::tempdir_in(std::env::current_dir().unwrap()).unwrap()
    }

    #[test]
    fn a_candidate_is_not_current_and_publication_keeps_previous() {
        let directory = root();
        let path = directory.path().join("official");
        let mut store = OfficialStore::open(path.clone(), 100, 1000).unwrap();
        let sources = [
            store
                .put_source(0, &source(0), FilterValidators::default(), None)
                .unwrap(),
            store
                .put_source(1, &source(1), FilterValidators::default(), None)
                .unwrap(),
        ];
        let package = store.new_package(sources, 1000).unwrap();
        let mut next = store.record.clone();
        next.highest_revision = package.identity.revision;
        next.candidate = Some(package.clone());
        next.due = 1000 + DAY;
        store.persist(next).unwrap();
        drop(store);
        let mut store = OfficialStore::open(path, 100, 1001).unwrap();
        assert!(store.record.current.is_none());
        assert_eq!(
            store.record.candidate.as_ref().unwrap().identity,
            package.identity
        );
        let mut next = store.record.clone();
        next.current = next.candidate.take();
        store.persist(next).unwrap();
        assert_eq!(
            store
                .catalog(store.record.current.as_ref().unwrap())
                .identity,
            package.identity
        );
        let mut changed = source(0);
        changed.extend_from_slice(b"||another.example.invalid^\n");
        let sources = [
            store
                .put_source(
                    0,
                    &changed,
                    FilterValidators::default(),
                    Some(&package.sources[0]),
                )
                .unwrap(),
            package.sources[1].clone(),
        ];
        let newer = store.new_package(sources, 1002).unwrap();
        let mut next = store.record.clone();
        next.highest_revision = newer.identity.revision;
        next.previous = next.current.take();
        next.current = Some(newer.clone());
        store.persist(next).unwrap();
        store.collect().unwrap();
        assert_eq!(
            store.record.previous.as_ref().unwrap().identity,
            package.identity
        );
        assert_ne!(
            newer.identity.manifest_sha256,
            package.identity.manifest_sha256
        );
        assert!(store.cached_bytes(&package.sources[0]).is_ok());
    }

    #[test]
    fn corrupt_cached_bytes_cannot_satisfy_a_conditional_hit() {
        let directory = root();
        let store = OfficialStore::open(directory.path().join("official"), 100, 1000).unwrap();
        let source = store
            .put_source(0, &source(0), FilterValidators::default(), None)
            .unwrap();
        let path = store.root.join(object_name(&source.sha256));
        atomic_write(&path, b"wrong source").unwrap();
        assert_eq!(store.cached_bytes(&source), Err(StoreError::ObjectCorrupt));
        // Untrusted multi-byte filenames cannot panic while parsing owned keys.
        atomic_write(&store.root.join(format!("{}.txt", "é".repeat(32))), b"x").unwrap();
        assert_eq!(store.collect(), Err(StoreError::UnsafePath));
    }

    #[test]
    fn manifest_identity_binds_sources_and_corrupt_state_uses_durable_backup() {
        let directory = root();
        let path = directory.path().join("official");
        let mut store = OfficialStore::open(path.clone(), 100, 1000).unwrap();
        let mut next = store.record.clone();
        next.due = 12345;
        store.persist(next).unwrap();
        atomic_write(&path.join("state.json"), b"not json").unwrap();
        drop(store);
        let recovered = OfficialStore::open(path, 100, 1001).unwrap();
        assert_eq!(recovered.record.due, 1000);
        let sources = [
            recovered
                .put_source(0, &source(0), FilterValidators::default(), None)
                .unwrap(),
            recovered
                .put_source(1, &source(1), FilterValidators::default(), None)
                .unwrap(),
        ];
        let mut package = recovered.new_package(sources, 1000).unwrap();
        package.sources[0].sha256[0] ^= 1;
        let mut record = recovered.record.clone();
        record.highest_revision = package.identity.revision;
        record.current = Some(package);
        assert_eq!(
            validate_record(&record, 1001),
            Err(StoreError::StateCorrupt)
        );
    }
}
