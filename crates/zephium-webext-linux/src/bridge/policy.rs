//! Request-policy snapshots as sealed, read-only memory files.

use std::fs::File;
use std::io::Write;
use std::os::fd::{AsFd, BorrowedFd, OwnedFd};

use rustix::fs::{fcntl_add_seals, fcntl_get_seals, memfd_create, MemfdFlags, SealFlags};
use zephium_webkit_web_process_protocol::policy::MAX_SNAPSHOT_BYTES;

const REQUIRED_SEALS: SealFlags = SealFlags::SEAL
    .union(SealFlags::SHRINK)
    .union(SealFlags::GROW)
    .union(SealFlags::WRITE);

/// Why a snapshot could not be published.
#[derive(Debug)]
pub enum PolicyError {
    /// Empty or over the module's snapshot limit.
    InvalidSize,
    /// The memory file could not be created, written or sealed.
    Io(std::io::Error),
}

impl From<rustix::io::Errno> for PolicyError {
    fn from(errno: rustix::io::Errno) -> Self {
        Self::Io(errno.into())
    }
}

/// One snapshot: a memfd sealed against writes, growth, shrinking and
/// further sealing, so every web process maps exactly these bytes.
#[derive(Debug)]
pub struct SealedPolicy {
    fd: OwnedFd,
    length: u32,
}

impl SealedPolicy {
    /// Seals `bytes`. Their format is the caller's; the module validates it.
    pub fn new(bytes: &[u8]) -> Result<Self, PolicyError> {
        if bytes.is_empty() || bytes.len() > MAX_SNAPSHOT_BYTES {
            return Err(PolicyError::InvalidSize);
        }
        let length = u32::try_from(bytes.len()).map_err(|_| PolicyError::InvalidSize)?;
        let fd = memfd_create(
            "zephium-webext-policy",
            MemfdFlags::CLOEXEC | MemfdFlags::ALLOW_SEALING,
        )?;
        let mut file = File::from(fd);
        file.write_all(bytes).map_err(PolicyError::Io)?;
        fcntl_add_seals(&file, REQUIRED_SEALS)?;
        if !fcntl_get_seals(&file)?.contains(REQUIRED_SEALS) {
            return Err(PolicyError::Io(std::io::Error::other("memfd seals")));
        }
        Ok(Self {
            fd: file.into(),
            length,
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Seek, SeekFrom};

    #[test]
    fn snapshots_are_sealed_exactly_and_bounded() {
        let policy = SealedPolicy::new(b"ZWPS1-test").expect("sealed");
        assert_eq!(policy.len(), 10);
        let seals = fcntl_get_seals(policy.fd()).expect("seals");
        assert!(seals.contains(REQUIRED_SEALS));
        let mut file = File::from(policy.fd().try_clone_to_owned().expect("dup"));
        assert!(file.write_all(b"x").is_err(), "sealed against writes");
        assert!(file.set_len(1).is_err(), "sealed against shrinking");
        file.seek(SeekFrom::Start(0)).expect("seek");
        let mut contents = Vec::new();
        file.read_to_end(&mut contents).expect("read");
        assert_eq!(contents, b"ZWPS1-test");
        assert!(matches!(
            SealedPolicy::new(&[]),
            Err(PolicyError::InvalidSize)
        ));
        assert!(matches!(
            SealedPolicy::new(&vec![0; MAX_SNAPSHOT_BYTES + 1]),
            Err(PolicyError::InvalidSize)
        ));
    }
}
