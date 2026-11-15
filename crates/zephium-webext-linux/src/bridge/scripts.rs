//! Validated immutable script tables published through sealed memory files.

use std::fs::File;
use std::io::Write;
use std::os::fd::{AsFd, BorrowedFd, OwnedFd};

use rustix::fs::{fcntl_add_seals, fcntl_get_seals, memfd_create, MemfdFlags, SealFlags};
use zephium_webkit_web_process_protocol::protocol::{
    ScriptTable, WireError, MAX_SCRIPT_TABLE_BYTES,
};

const REQUIRED_SEALS: SealFlags = SealFlags::SEAL
    .union(SealFlags::SHRINK)
    .union(SealFlags::GROW)
    .union(SealFlags::WRITE);

/// Why a snapshot could not be published.
#[derive(Debug)]
pub enum ScriptTableError {
    /// Empty or over the module's snapshot limit.
    InvalidSize,
    /// The script-table codec refused the table.
    Wire(WireError),
    /// Table generations must increase and live snapshots are bounded.
    Generation,
    /// A source URL or install belongs to another admitted extension.
    InvalidOwner,
    /// The memory file could not be created, written or sealed.
    Io(std::io::Error),
}

impl From<rustix::io::Errno> for ScriptTableError {
    fn from(errno: rustix::io::Errno) -> Self {
        Self::Io(errno.into())
    }
}

/// One snapshot: a memfd sealed against writes, growth, shrinking and
/// further sealing, so every web process maps exactly these bytes.
#[derive(Debug)]
pub struct SealedScripts {
    fd: OwnedFd,
    length: u32,
    pub(crate) table: ScriptTable,
}

impl SealedScripts {
    /// Validates and seals a host-built table before publication.
    pub fn new(table: ScriptTable) -> Result<Self, ScriptTableError> {
        let bytes = table.encode().map_err(ScriptTableError::Wire)?;
        let table = ScriptTable::decode(&bytes).map_err(ScriptTableError::Wire)?;
        if bytes.is_empty() || bytes.len() > MAX_SCRIPT_TABLE_BYTES {
            return Err(ScriptTableError::InvalidSize);
        }
        let length = u32::try_from(bytes.len()).map_err(|_| ScriptTableError::InvalidSize)?;
        let fd = memfd_create(
            "zephium-webext-scripts",
            MemfdFlags::CLOEXEC | MemfdFlags::ALLOW_SEALING,
        )?;
        let mut file = File::from(fd);
        file.write_all(&bytes).map_err(ScriptTableError::Io)?;
        fcntl_add_seals(&file, REQUIRED_SEALS)?;
        if !fcntl_get_seals(&file)?.contains(REQUIRED_SEALS) {
            return Err(ScriptTableError::Io(std::io::Error::other("memfd seals")));
        }
        Ok(Self {
            fd: file.into(),
            length,
            table,
        })
    }

    /// The snapshot length the module must find.
    pub fn len(&self) -> u32 {
        self.length
    }

    /// Never true: empty snapshots are refused.
    pub fn is_empty(&self) -> bool {
        self.length == 0
    }

    pub(crate) fn fd(&self) -> BorrowedFd<'_> {
        self.fd.as_fd()
    }
}

/// Immutable package source reused by programmatic injections of this admission.
#[derive(Debug)]
pub(crate) struct SealedSource {
    fd: OwnedFd,
    pub length: u32,
}

impl SealedSource {
    pub fn fd(&self) -> BorrowedFd<'_> {
        self.fd.as_fd()
    }
}

#[derive(Default)]
pub(crate) struct SourceCache {
    retired: Vec<std::rc::Weak<SealedSource>>,
    sources: std::collections::BTreeMap<
        (zephium_core::ids::ExtensionInstallId, String, [u8; 32]),
        std::rc::Rc<SealedSource>,
    >,
}

impl SourceCache {
    pub fn bytes(&self) -> usize {
        self.sources
            .values()
            .map(|source| source.length as usize)
            .chain(
                self.retired
                    .iter()
                    .filter_map(std::rc::Weak::upgrade)
                    .map(|source| source.length as usize),
            )
            .sum()
    }
    pub fn retire(&mut self, install: zephium_core::ids::ExtensionInstallId) {
        for ((owner, _, _), source) in &self.sources {
            if *owner == install {
                self.retired.push(std::rc::Rc::downgrade(source));
            }
        }
        self.sources.retain(|(owner, _, _), _| *owner != install);
        self.retired.retain(|source| source.strong_count() != 0);
    }

    pub fn source(
        &mut self,
        install: zephium_core::ids::ExtensionInstallId,
        url: &str,
        source: &str,
        available: usize,
    ) -> Result<std::rc::Rc<SealedSource>, ScriptTableError> {
        use sha2::{Digest, Sha256};
        if source.is_empty()
            || source.len() > zephium_webkit_web_process_protocol::protocol::MAX_SCRIPT_SOURCE_BYTES
            || source.contains('\0')
        {
            return Err(ScriptTableError::InvalidSize);
        }
        let key = (
            install,
            url.to_owned(),
            Sha256::digest(source.as_bytes()).into(),
        );
        if let Some(source) = self.sources.get(&key) {
            return Ok(source.clone());
        }
        self.retired.retain(|source| source.strong_count() != 0);
        let bytes = self.bytes();
        if self.sources.len().saturating_add(self.retired.len()) >= 1024
            || bytes.saturating_add(source.len()) > available.min(64 * 1024 * 1024)
        {
            return Err(ScriptTableError::InvalidSize);
        }
        let fd = memfd_create(
            "zephium-webext-source",
            MemfdFlags::CLOEXEC | MemfdFlags::ALLOW_SEALING,
        )?;
        let mut file = File::from(fd);
        file.write_all(source.as_bytes())
            .map_err(ScriptTableError::Io)?;
        fcntl_add_seals(&file, REQUIRED_SEALS)?;
        if !fcntl_get_seals(&file)?.contains(REQUIRED_SEALS) {
            return Err(ScriptTableError::InvalidSize);
        }
        let source = std::rc::Rc::new(SealedSource {
            fd: file.into(),
            length: source.len() as u32,
        });
        self.sources.insert(key, source.clone());
        Ok(source)
    }
}

#[cfg(test)]
mod source_tests {
    use super::*;
    use zephium_core::ids::ExtensionInstallId;

    #[test]
    fn source_cache_reuses_seals_and_retires_install_authority() {
        let mut cache = SourceCache::default();
        let install = ExtensionInstallId::from(1);
        let source = "/* source */".repeat(8192);
        let first = cache
            .source(install, "file.js", &source, 64 * 1024 * 1024)
            .unwrap();
        let second = cache
            .source(install, "file.js", &source, 64 * 1024 * 1024)
            .unwrap();
        assert!(std::rc::Rc::ptr_eq(&first, &second));
        assert!(fcntl_get_seals(first.fd())
            .unwrap()
            .contains(REQUIRED_SEALS));
        let changed = cache
            .source(install, "file.js", "/* changed */", 64 * 1024 * 1024)
            .unwrap();
        assert!(!std::rc::Rc::ptr_eq(&first, &changed));
        let other = cache
            .source(
                ExtensionInstallId::from(2),
                "file.js",
                &source,
                64 * 1024 * 1024,
            )
            .unwrap();
        assert!(!std::rc::Rc::ptr_eq(&first, &other));
        cache.retire(install);
        let next = cache
            .source(install, "file.js", &source, 64 * 1024 * 1024)
            .unwrap();
        assert!(!std::rc::Rc::ptr_eq(&first, &next));
        assert!(cache
            .source(install, "file.js", "bad\0source", 64 * 1024 * 1024)
            .is_err());
        assert!(cache
            .source(
                install,
                "file.js",
                &"x".repeat(
                    zephium_webkit_web_process_protocol::protocol::MAX_SCRIPT_SOURCE_BYTES + 1
                ),
                64 * 1024 * 1024,
            )
            .is_err());
    }
}
