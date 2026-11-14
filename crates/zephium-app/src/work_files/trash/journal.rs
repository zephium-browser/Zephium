//! A bounded recovery journal in trusted application storage, never the project.
use super::{Failure, MAX_BYTES};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use zephium_private_fs::{
    ByteLimit, LockedPrivateNamespace, PrivateComponent, PrivateDirectory, PrivateFsError,
};

pub(super) const MAX_ACTIVE: usize = 64;
pub(super) const MAX_COMPLETED: usize = 256;
pub(super) const MAX_RECORDS: usize = MAX_ACTIVE + MAX_COMPLETED;
mod retention;
fn component(name: &str) -> Result<PrivateComponent, Failure> {
    PrivateComponent::new(name).map_err(|_| Failure::Storage)
}
fn limit(bytes: usize) -> ByteLimit {
    ByteLimit::new(bytes).expect("fixed bounded limit")
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub(super) struct Receipt {
    pub version: u8,
    pub id: String,
    pub path: PathBuf,
    pub digest: String,
    pub bytes: usize,
    pub step: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub destination: Option<PathBuf>,
}
impl Receipt {
    fn valid(&self) -> bool {
        self.version == 1
            && self.bytes <= MAX_BYTES
            && self.id.len() == 32
            && self.id.bytes().all(|c| c.is_ascii_hexdigit())
            && self.digest.len() == 64
            && self.digest.bytes().all(|c| c.is_ascii_hexdigit())
            && self.step.len() <= 64
            && self.path.is_absolute()
            && !super::super::protected_path(&self.path)
            && self.destination.as_ref().is_none_or(|path| {
                path.is_absolute() && path != &self.path && !super::super::protected_path(path)
            })
    }
}
pub(super) struct Journal {
    _namespace: LockedPrivateNamespace,
    directory: PrivateDirectory,
}
pub(super) struct Record {
    pub receipt: Receipt,
    directory: PrivateDirectory,
}
impl Journal {
    pub fn open(root: &Path, profile: &str, create: bool) -> Result<Option<Self>, Failure> {
        let path = root.join(".zephium-work-recovery");
        let namespace = match if create {
            LockedPrivateNamespace::open_or_create(path)
        } else {
            LockedPrivateNamespace::open_existing(path)
        } {
            Ok(value) => value,
            Err(PrivateFsError::NotFound) if !create => return Ok(None),
            Err(_) => return Err(Failure::Storage),
        };
        if profile.is_empty() || profile.len() > 128 {
            return Err(Failure::Storage);
        }
        let profile = component(&super::digest(profile.as_bytes()))?;
        let directory = match if create {
            namespace.directory().create_private_child(&profile)
        } else {
            namespace.directory().open_private_child(&profile)
        } {
            Ok(value) => value,
            Err(PrivateFsError::NotFound) if !create => return Ok(None),
            Err(_) => return Err(Failure::Storage),
        };
        Ok(Some(Self {
            _namespace: namespace,
            directory,
        }))
    }
    #[cfg(any(target_os = "macos", windows, test))]
    pub fn prepare(&self, receipt: Receipt, bytes: &[u8]) -> Result<Record, Failure> {
        if !receipt.valid()
            || bytes.len() != receipt.bytes
            || super::digest(bytes) != receipt.digest
        {
            return Err(Failure::Changed);
        }
        if self.maintain()? >= MAX_ACTIVE {
            return Err(Failure::Capacity);
        }
        let directory = self
            .directory
            .create_new_private_child(&component(&receipt.id)?)
            .map_err(|_| Failure::Storage)?;
        // The complete backup is durable before the prepared receipt, and both
        // are durable before the original can leave its granted directory.
        directory
            .write_new_synced(&component("saved")?, bytes, limit(MAX_BYTES))
            .map_err(|_| Failure::Storage)?;
        let encoded = serde_json::to_vec(&receipt).map_err(|_| Failure::Storage)?;
        directory
            .write_new_synced(&component("receipt")?, &encoded, limit(16 * 1024))
            .map_err(|_| Failure::Storage)?;
        Ok(Record { receipt, directory })
    }
    pub fn records(&self) -> Result<Vec<Record>, Failure> {
        let mut records = Vec::new();
        for name in self
            .directory
            .list_components(MAX_RECORDS)
            .map_err(|_| Failure::Capacity)?
        {
            if let Some(record) = self.read_record(&name)? {
                // Completed/expired IDs stay unavailable to restore. Keep only
                // the at-most64 active capabilities open, not256 history FDs.
                if !record.marked("expiring")? && !record.completed()? {
                    if records.len() == MAX_ACTIVE {
                        return Err(Failure::Capacity);
                    }
                    records.push(record);
                }
            }
        }
        Ok(records)
    }
    fn read_record(&self, name: &PrivateComponent) -> Result<Option<Record>, Failure> {
        let directory = self
            .directory
            .open_private_child(name)
            .map_err(|_| Failure::Storage)?;
        let Some(bytes) = directory
            .read_bounded_regular(&component("receipt")?, limit(16 * 1024))
            .map_err(|_| Failure::Storage)?
        else {
            return Ok(None);
        };
        let receipt: Receipt = serde_json::from_slice(&bytes).map_err(|_| Failure::Storage)?;
        if !receipt.valid() || receipt.id != name.as_str() {
            return Err(Failure::Storage);
        }
        Ok(Some(Record { receipt, directory }))
    }
}
impl Record {
    #[cfg(any(target_os = "macos", windows, test))]
    pub fn stage(&self) -> Result<PrivateDirectory, Failure> {
        self.directory
            .create_new_private_child(&component("stage")?)
            .map_err(|_| Failure::Storage)
    }
    #[cfg(any(target_os = "macos", windows))]
    pub fn backup(&self) -> Result<PrivateDirectory, Failure> {
        let directory = self
            .directory
            .create_new_private_child(&component("backup")?)
            .map_err(|_| Failure::Storage)?;
        directory
            .write_new_synced(
                &component("recovery.txt")?,
                self.saved()?.as_bytes(),
                limit(MAX_BYTES),
            )
            .map_err(|_| Failure::Storage)?;
        Ok(directory)
    }
    pub fn mark(&self, phase: &str) -> Result<(), Failure> {
        if !matches!(
            phase,
            "trashed" | "move_backup_trashed" | "moved" | "expiring"
        ) {
            return Err(Failure::Storage);
        }
        self.directory
            .write_new_synced(&component(phase)?, b"1\n", limit(2))
            .map(|_| ())
            .map_err(|_| Failure::Storage)
    }
    pub fn marked(&self, phase: &str) -> Result<bool, Failure> {
        let value = self
            .directory
            .read_bounded_regular(
                &component(phase)?,
                limit(if phase == "restored" { 35 } else { 2 }),
            )
            .map_err(|_| Failure::Storage)?;
        match value.as_deref() {
            None => Ok(false),
            Some(b"1\n") => Ok(true),
            Some(bytes) if phase == "restored" && restored_order(bytes).is_some() => Ok(true),
            _ => Err(Failure::Storage),
        }
    }
    pub fn saved(&self) -> Result<String, Failure> {
        let bytes = self
            .directory
            .read_bounded_regular(&component("saved")?, limit(MAX_BYTES))
            .map_err(|_| Failure::Storage)?
            .ok_or(Failure::Storage)?;
        if bytes.len() != self.receipt.bytes || super::digest(&bytes) != self.receipt.digest {
            return Err(Failure::Storage);
        }
        let text = String::from_utf8(bytes).map_err(|_| Failure::Storage)?;
        if super::super::binary_looking(&text) {
            return Err(Failure::Storage);
        }
        Ok(text)
    }
}

fn restored_order(bytes: &[u8]) -> Option<&str> {
    if bytes.len() != 35 || !bytes.starts_with(b"2:") || !bytes.ends_with(b"\n") {
        return None;
    }
    let order = std::str::from_utf8(&bytes[2..34]).ok()?;
    order
        .bytes()
        .all(|byte| byte.is_ascii_hexdigit())
        .then_some(order)
}
