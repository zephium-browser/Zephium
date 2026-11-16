//! Approved, recoverable removal. Generic file deletion remains unavailable.
mod journal;
mod move_file;
mod wait;
pub(crate) use move_file::PreparedMove;
pub(crate) use wait::{launch as launch_worker, observe as await_worker};
#[cfg(target_os = "macos")]
mod macos;
#[cfg(windows)]
mod windows;
#[cfg(target_os = "macos")]
use macos as native;
#[cfg(windows)]
use windows as native;
#[cfg(all(test, any(target_os = "macos", target_os = "linux")))]
mod tests;

use super::{WorkFileError, WorkFileGrant};
use sha2::{Digest, Sha256};
use std::{
    path::{Path, PathBuf},
    sync::OnceLock,
};
use zephium_core::work::runtime::{WorkLocalStepV1, WorkStepKindV1};
#[cfg(any(target_os = "macos", windows, test))]
use zephium_core::work::WorkArtifactId;
const MAX_BYTES: usize = zephium_core::work::runtime::MAX_WORK_FILE_CONTENT_BYTES;
static ROOT: OnceLock<PathBuf> = OnceLock::new();

/// Installs the trusted application-data root at desktop startup. Model input
/// cannot select recovery storage or switch it after initialization.
pub fn install_root(root: PathBuf) {
    let _ = ROOT.set(root);
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Failure {
    Unavailable,
    Input,
    Storage,
    Capacity,
    Changed,
    #[cfg(any(target_os = "macos", windows))]
    CrossVolume,
    RecoveryPending,
    Destination,
    Stopped,
    Busy,
    Deadline,
    File(WorkFileError),
}
impl Failure {
    pub fn step_status(self) -> zephium_core::work::runtime::WorkStepStatus {
        use zephium_core::work::runtime::WorkStepStatus;
        match self {
            Self::Stopped => WorkStepStatus::Cancelled,
            Self::RecoveryPending => WorkStepStatus::OutcomeUnknown,
            _ => WorkStepStatus::Failed,
        }
    }
    pub fn note(self) -> &'static str {
        match self {
            Self::Busy => "A recoverable file operation still owns native worker capacity; wait for its receipt before retrying",
            Self::Deadline => "The file operation exceeded its time limit and stopped before changing the file",
            Self::Destination => "The destination must be absent in the approved granted directory",
            Self::Unavailable => "Recoverable file changes are not available on this system",
            Self::Stopped => "Stopped before moving the file",
            Self::Input => "Recoverable file changes accept only ordinary text files up to 64 KiB",
            Self::Storage => "The private recovery journal could not be verified",
            Self::Capacity => "The private recovery journal is full; no file was removed",
            Self::Changed => "The file changed since it was read; no new change was admitted",
            #[cfg(any(target_os = "macos", windows))]
            Self::CrossVolume => "Trash across volumes is not supported; no file was removed",
            Self::RecoveryPending => {
                "The file change is not confirmed; its native worker or recovery state is still unresolved"
            }
            Self::File(error) => error.note(),
        }
    }
}
#[cfg(any(target_os = "macos", windows, test))]
fn recovery_id() -> String {
    format!(
        "{:032x}",
        u128::from_be_bytes(WorkArtifactId::generate().bytes())
    )
}
fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn available() -> Result<(), Failure> {
    // The macOS slice passed exact-ack, interrupted-recovery and queued-cancel
    // fixtures on the same build admitted by the qualified runner. Windows
    // requires its separate native private-fs and Recycle Bin qualification.
    if crate::work_commands::runner::native_recovery_qualified() {
        Ok(())
    } else {
        Err(Failure::Unavailable)
    }
}
fn root() -> Result<&'static Path, Failure> {
    ROOT.get().map(PathBuf::as_path).ok_or(Failure::Unavailable)
}

pub(crate) struct PreparedTrash {
    pub id: String,
    pub path: PathBuf,
    #[cfg(any(target_os = "macos", windows))]
    native: native::Prepared,
}
impl PreparedTrash {
    pub fn prepare(grant: &WorkFileGrant, path: &str, known: &str) -> Result<Self, Failure> {
        available()?;
        root()?;
        Self::prepare_checked(grant, path, known)
    }
    fn prepare_checked(grant: &WorkFileGrant, path: &str, known: &str) -> Result<Self, Failure> {
        let resolved = grant.resolve(path, false).map_err(Failure::File)?;
        // Refuse even an in-grant symlink spelling: approval names the actual
        // regular file, never an alias whose target would be moved instead.
        let _checked = super::open_checked_file(Path::new(path), false).map_err(Failure::File)?;
        let path = resolved;
        #[cfg(any(target_os = "macos", windows))]
        {
            let native = native::Prepared::new(path.clone(), known)?;
            Ok(Self {
                id: recovery_id(),
                path,
                native,
            })
        }
        #[cfg(not(any(target_os = "macos", windows)))]
        {
            let _ = (path, known);
            Err(Failure::Unavailable)
        }
    }
    pub fn bytes(&self) -> u32 {
        #[cfg(any(target_os = "macos", windows))]
        {
            self.native.text.len() as u32
        }
        #[cfg(not(any(target_os = "macos", windows)))]
        {
            0
        }
    }
    pub fn fact(&self) -> WorkLocalStepV1 {
        #[cfg(any(target_os = "macos", windows))]
        {
            WorkLocalStepV1 {
                proposal: Some(format!(
                    "Move {} to Trash; keep a private recovery copy ({} bytes).",
                    self.path.display(),
                    self.native.text.len()
                )),
                before_digest: Some(self.native.digest.clone()),
                ..Default::default()
            }
        }
        #[cfg(not(any(target_os = "macos", windows)))]
        {
            WorkLocalStepV1::default()
        }
    }
    pub fn apply(
        self,
        profile: &str,
        step: &str,
        cancelled: &std::sync::atomic::AtomicBool,
    ) -> Result<(), Failure> {
        self.apply_with_cancel(root()?, profile, step, cancelled)
            .map(|_| ())
    }
    #[cfg(all(test, target_os = "macos"))]
    fn apply_at(self, root: &Path, profile: &str, step: &str) -> Result<PathBuf, Failure> {
        self.apply_with_cancel(
            root,
            profile,
            step,
            &std::sync::atomic::AtomicBool::new(false),
        )
    }
    fn apply_with_cancel(
        self,
        root: &Path,
        profile: &str,
        step: &str,
        cancelled: &std::sync::atomic::AtomicBool,
    ) -> Result<PathBuf, Failure> {
        if cancelled.load(std::sync::atomic::Ordering::Acquire) {
            return Err(Failure::Stopped);
        }
        #[cfg(any(target_os = "macos", windows))]
        {
            let journal = journal::Journal::open(root, profile, true)?.ok_or(Failure::Storage)?;
            let receipt = journal::Receipt {
                version: 1,
                id: self.id,
                path: self.path,
                digest: self.native.digest.clone(),
                bytes: self.native.text.len(),
                step: step.into(),
                destination: None,
            };
            let record = journal.prepare(receipt, self.native.text.as_bytes())?;
            let stage = record.stage()?;
            let returned = self.native.apply(&stage, &record, cancelled)?;
            record
                .mark("trashed")
                .map_err(|_| Failure::RecoveryPending)?;
            Ok(returned)
        }
        #[cfg(not(any(target_os = "macos", windows)))]
        {
            let _ = (root, profile, step);
            Err(Failure::Unavailable)
        }
    }
}
pub(crate) struct Recovery {
    pub id: String,
    pub path: PathBuf,
    pub confirmed: bool,
    pub destination: Option<PathBuf>,
    pub moved: bool,
    pub backup_confirmed: bool,
}
pub(crate) fn recoveries(grant: &WorkFileGrant, profile: &str) -> Result<Vec<Recovery>, Failure> {
    available()?;
    recoveries_at(root()?, grant, profile)
}
fn recoveries_at(
    root: &Path,
    grant: &WorkFileGrant,
    profile: &str,
) -> Result<Vec<Recovery>, Failure> {
    let Some(journal) = journal::Journal::open(root, profile, false)? else {
        return Ok(Vec::new());
    };
    let mut found = Vec::new();
    for record in journal.records()? {
        if record.marked("restored")? {
            continue;
        }
        if grant
            .resolve(record.receipt.path.to_str().ok_or(Failure::Storage)?, true)
            .ok()
            .as_ref()
            != Some(&record.receipt.path)
        {
            continue;
        }
        let confirmed = record.marked("trashed")?;
        let moved = record.marked("moved")?;
        let backup_confirmed = record.marked("move_backup_trashed")?;
        let destination = record
            .receipt
            .destination
            .as_ref()
            .filter(|path| {
                path.to_str()
                    .and_then(|path| grant.resolve(path, true).ok())
                    .as_ref()
                    == Some(*path)
            })
            .cloned();
        found.push(Recovery {
            id: record.receipt.id,
            path: record.receipt.path,
            confirmed,
            destination,
            moved,
            backup_confirmed,
        });
    }
    Ok(found)
}
pub(crate) fn restore_proposal(
    grant: &WorkFileGrant,
    profile: &str,
    id: &str,
) -> Result<(WorkStepKindV1, super::PreparedChange, PathBuf, String), Failure> {
    available()?;
    restore_proposal_at(root()?, grant, profile, id)
}
fn restore_proposal_at(
    root: &Path,
    grant: &WorkFileGrant,
    profile: &str,
    id: &str,
) -> Result<(WorkStepKindV1, super::PreparedChange, PathBuf, String), Failure> {
    let journal = journal::Journal::open(root, profile, false)?.ok_or(Failure::Storage)?;
    let record = journal
        .records()?
        .into_iter()
        .find(|record| record.receipt.id == id)
        .ok_or(Failure::Storage)?;
    if record.marked("restored")? {
        return Err(Failure::Changed);
    }
    let path = record.receipt.path.clone();
    if grant
        .resolve(path.to_str().ok_or(Failure::Storage)?, true)
        .map_err(Failure::File)?
        != path
    {
        return Err(Failure::Changed);
    }
    let saved = record.saved()?;
    let kind = WorkStepKindV1::WriteFile {
        path: path.to_string_lossy().into_owned(),
        content: saved.clone(),
        decision: None,
    };
    let change = grant.prepare(&kind, None).map_err(Failure::File)?;
    // Never turn restoration into an overwrite, even if a file appears between
    // discovery and proposal. Atomic exclusive publication checks again later.
    if change.fact().before_digest.is_some() {
        return Err(Failure::File(WorkFileError::Exists));
    }
    Ok((kind, change, path, saved))
}
pub(crate) fn restored(profile: &str, id: &str) -> Result<(), Failure> {
    let journal = journal::Journal::open(root()?, profile, false)?.ok_or(Failure::Storage)?;
    let record = journal
        .records()?
        .into_iter()
        .find(|record| record.receipt.id == id)
        .ok_or(Failure::Storage)?;
    journal.mark_restored(&record)?;
    journal.maintain().map(|_| ())
}

#[cfg(all(test, target_os = "macos"))]
pub(crate) fn seed_recovery_fixture(
    root: &Path,
    profile: &str,
    path: PathBuf,
    text: &str,
) -> String {
    let id = recovery_id();
    let journal = journal::Journal::open(root, profile, true)
        .unwrap()
        .unwrap();
    journal
        .prepare(
            journal::Receipt {
                version: 1,
                id: id.clone(),
                path,
                digest: digest(text.as_bytes()),
                bytes: text.len(),
                step: "fixture".into(),
                destination: None,
            },
            text.as_bytes(),
        )
        .unwrap();
    id
}
