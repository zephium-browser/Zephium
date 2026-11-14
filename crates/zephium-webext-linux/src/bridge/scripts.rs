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
