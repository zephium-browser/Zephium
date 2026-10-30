//! Finite completed history. This never opens a source/destination or OS Trash.
use super::{component, limit, Failure, Journal, Record, MAX_COMPLETED, MAX_RECORDS};
use zephium_private_fs::{PrivateComponent, PrivateDirectory, PrivateFsError};
const EXPIRING: &str = "expiring";
const CONTENTS: &[&str] = &[
    "receipt",
    "saved",
    "restored",
    "trashed",
    "move_backup_trashed",
    "moved",
    "stage",
    "backup",
    EXPIRING,
];
fn identifier(name: &str) -> bool {
    name.len() == 32 && name.bytes().all(|byte| byte.is_ascii_hexdigit())
}
fn empty_child(directory: &PrivateDirectory, name: &str) -> Result<bool, Failure> {
    let child = match directory.open_private_child(&component(name)?) {
        Ok(child) => child,
        Err(PrivateFsError::NotFound) => return Ok(true),
        Err(_) => return Err(Failure::Storage),
    };
    match child.list_entry_names(1) {
        Ok(names) => Ok(names.is_empty()),
        Err(PrivateFsError::BoundExceeded) => Ok(false),
        Err(_) => Err(Failure::Storage),
    }
}
impl Journal {
    /// The namespace lock serializes completion. Derive a strictly increasing
    /// counter from bounded retained history instead of a random ULID suffix or
    /// wall clock, so same-tick completion and clock rollback preserve order.
    pub(in crate::work_files::trash) fn mark_restored(
        &self,
        record: &Record,
    ) -> Result<(), Failure> {
        let mut latest = 0u128;
        for name in self
            .directory
            .list_components(MAX_RECORDS)
            .map_err(|_| Failure::Capacity)?
        {
            if let Some(existing) = self.read_record(&name)? {
                if existing.marked("restored")? {
                    latest = latest.max(
                        u128::from_str_radix(&existing.completion_order()?, 16)
                            .map_err(|_| Failure::Storage)?,
                    );
                }
            }
        }
        let next = latest.checked_add(1).ok_or(Failure::Capacity)?;
        record
            .directory
            .write_new_synced(
                &component("restored")?,
                format!("2:{next:032x}\n").as_bytes(),
                limit(35),
            )
            .map(|_| ())
            .map_err(|_| Failure::Storage)
    }
    /// Returns the number of active/incomplete rows. A committed expiration is
    /// resumed only inside the locked, verified private namespace.
    pub(in crate::work_files::trash) fn maintain(&self) -> Result<usize, Failure> {
        let names = self
            .directory
            .list_components(MAX_RECORDS)
            .map_err(|_| Failure::Capacity)?;
        let mut active = 0usize;
        let mut completed = Vec::new();
        for name in names {
            if !identifier(name.as_str()) {
                return Err(Failure::Storage);
            }
            let directory = self
                .directory
                .open_private_child(&name)
                .map_err(|_| Failure::Storage)?;
            let Some(record) = self.read_record(&name)? else {
                // Empty preparation residue has no payload. An expiring-only
                // row is the last durable phase after its receipt was removed.
                let contents = directory
                    .list_components(10)
                    .map_err(|_| Failure::Storage)?;
                if contents.is_empty() {
                    self.directory
                        .remove_empty_private_child(&name)
                        .map_err(|_| Failure::Storage)?;
                } else if contents.len() == 1 && contents[0].as_str() == EXPIRING {
                    if directory
                        .read_bounded_regular(&component(EXPIRING)?, limit(2))
                        .map_err(|_| Failure::Storage)?
                        .as_deref()
                        != Some(b"1\n")
                    {
                        return Err(Failure::Storage);
                    }
                    directory
                        .remove_verified_regular(&component(EXPIRING)?)
                        .map_err(|_| Failure::Storage)?;
                    self.directory
                        .remove_empty_private_child(&name)
                        .map_err(|_| Failure::Storage)?;
                } else {
                    active = active.checked_add(1).ok_or(Failure::Capacity)?;
                }
                continue;
            };
            if record.marked(EXPIRING)? {
                self.finish_expiry(&name, record)?;
            } else if record.completed()? {
                completed.push((record.completion_order()?, name));
            } else {
                active = active.checked_add(1).ok_or(Failure::Capacity)?;
            }
        }
        completed.sort_by(|left, right| {
            left.0
                .cmp(&right.0)
                .then_with(|| left.1.as_str().cmp(right.1.as_str()))
        });
        let expire = completed.len().saturating_sub(MAX_COMPLETED);
        for (_, name) in completed.into_iter().take(expire) {
            let record = self.read_record(&name)?.ok_or(Failure::Storage)?;
            if !record.completed()? {
                return Err(Failure::Changed);
            }
            // Verify the bounded saved copy before committing expiration. It is
            // no longer an active recovery: restoration and native completion
            // were durably acknowledged and no original remains staged.
            record.saved()?;
            record.known_completed_contents()?;
            record.mark(EXPIRING)?;
            self.finish_expiry(&name, record)?;
        }
        Ok(active)
    }
    fn finish_expiry(&self, name: &PrivateComponent, record: Record) -> Result<(), Failure> {
        record.known_completed_contents()?;
        if let Some(bytes) = record
            .directory
            .read_bounded_regular(&component("saved")?, limit(super::MAX_BYTES))
            .map_err(|_| Failure::Storage)?
        {
            if bytes.len() != record.receipt.bytes
                || super::super::digest(&bytes) != record.receipt.digest
            {
                return Err(Failure::Storage);
            }
        }
        for child in ["stage", "backup"] {
            record
                .directory
                .remove_empty_private_child(&component(child)?)
                .map_err(|_| Failure::Storage)?;
        }
        // Only exact admitted regular protocol files are removed. The durable
        // expiring marker remains until the receipt is gone; missing IDs always
        // fail closed and can never recreate an operation.
        for file in [
            "saved",
            "trashed",
            "move_backup_trashed",
            "moved",
            "restored",
            "receipt",
            EXPIRING,
        ] {
            record
                .directory
                .remove_verified_regular(&component(file)?)
                .map_err(|_| Failure::Storage)?;
        }
        drop(record);
        self.directory
            .remove_empty_private_child(name)
            .map_err(|_| Failure::Storage)?;
        Ok(())
    }
}
impl Record {
    pub(super) fn completed(&self) -> Result<bool, Failure> {
        if !self.marked("restored")? {
            return Ok(false);
        }
        let acknowledged = if self.receipt.destination.is_some() {
            self.marked("moved")? && self.marked("move_backup_trashed")?
        } else {
            self.marked("trashed")?
        };
        Ok(acknowledged
            && empty_child(&self.directory, "stage")?
            && empty_child(&self.directory, "backup")?)
    }
    fn completion_order(&self) -> Result<String, Failure> {
        let bytes = self
            .directory
            .read_bounded_regular(&component("restored")?, limit(35))
            .map_err(|_| Failure::Storage)?
            .ok_or(Failure::Storage)?;
        if bytes == b"1\n" {
            return Ok(self.receipt.id.clone());
        } // Legacy completion ordering.
        super::restored_order(&bytes)
            .map(str::to_owned)
            .ok_or(Failure::Storage)
    }
    fn known_completed_contents(&self) -> Result<(), Failure> {
        if !empty_child(&self.directory, "stage")? || !empty_child(&self.directory, "backup")? {
            return Err(Failure::RecoveryPending);
        }
        if self
            .directory
            .list_components(CONTENTS.len())
            .map_err(|_| Failure::Storage)?
            .iter()
            .any(|name| !CONTENTS.contains(&name.as_str()))
        {
            return Err(Failure::Storage);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    #[cfg(not(windows))]
    use super::super::Receipt;
    use super::*;
    fn root() -> tempfile::TempDir {
        tempfile::Builder::new()
            .prefix("zephium-retention-inert-")
            .tempdir()
            .unwrap()
    }
    #[cfg(not(windows))]
    fn receipt(root: &std::path::Path, id: usize) -> Receipt {
        Receipt {
            version: 1,
            id: format!("{id:032x}"),
            path: root.join(format!("original-{id}.txt")),
            digest: super::super::super::digest(b"saved"),
            bytes: 5,
            step: "inert".into(),
            destination: None,
        }
    }
    #[cfg(not(windows))]
    fn completed(journal: &Journal, root: &std::path::Path, id: usize) -> Record {
        let record = journal.prepare(receipt(root, id), b"saved").unwrap();
        record.mark("trashed").unwrap();
        journal.mark_restored(&record).unwrap();
        record
    }
    #[cfg(not(windows))]
    #[test]
    fn only_oldest_completed_rows_beyond_256_expire_and_missing_ids_stay_missing() {
        let root = root();
        let root_path = root.path().canonicalize().unwrap();
        let journal = Journal::open(&root_path, "profile", true).unwrap().unwrap();
        let mut orders = Vec::new();
        for id in 0..=MAX_COMPLETED {
            let record = completed(&journal, &root_path, MAX_COMPLETED - id);
            orders.push((
                record.completion_order().unwrap(),
                record.receipt.id.clone(),
            ));
        }
        // Assert completion order itself; sorting random IDs would conceal a
        // same-millisecond ordering bug. IDs deliberately run backwards.
        assert!(orders.windows(2).all(|pair| pair[0].0 < pair[1].0));
        let oldest = orders[0].1.clone();
        assert_eq!(journal.maintain().unwrap(), 0);
        assert_eq!(
            journal
                .directory
                .list_components(MAX_RECORDS)
                .unwrap()
                .len(),
            MAX_COMPLETED
        );
        assert!(matches!(
            journal
                .directory
                .open_private_child(&component(&oldest).unwrap()),
            Err(PrivateFsError::NotFound)
        ));
        assert!(journal.records().unwrap().is_empty());
        assert_eq!(journal.maintain().unwrap(), 0); // Expired IDs are never re-created.
        let active = journal
            .prepare(receipt(&root_path, MAX_COMPLETED + 1), b"saved")
            .unwrap();
        assert_eq!(active.saved().unwrap(), "saved");
        assert_eq!(journal.records().unwrap().len(), 1);
        drop(active);
        drop(journal);
        let grant = crate::work_files::WorkFileGrant {
            roots: vec![root_path.clone()],
            written: vec![root_path.clone()],
        };
        // Exercise the same restore-proposal entry used by the public recovery
        // tool, with only its application-storage root injected for isolation.
        assert!(matches!(
            super::super::super::restore_proposal_at(&root_path, &grant, "profile", &oldest),
            Err(Failure::Storage)
        ));
        assert!(!root_path
            .join(format!("original-{MAX_COMPLETED}.txt"))
            .exists());
    }
    #[cfg(not(windows))]
    #[test]
    fn restored_or_completed_markers_never_retire_an_active_original_or_unacknowledged_move() {
        let root = root();
        let root_path = root.path().canonicalize().unwrap();
        let journal = Journal::open(&root_path, "profile", true).unwrap().unwrap();
        let record = completed(&journal, &root_path, 1);
        let stage = record.stage().unwrap();
        stage
            .write_new_synced(&component("original.txt").unwrap(), b"active", limit(16))
            .unwrap();
        let mut movement = receipt(&root_path, 2);
        movement.destination = Some(root_path.join("destination.txt"));
        let pending = journal.prepare(movement, b"saved").unwrap();
        journal.mark_restored(&pending).unwrap();
        pending.mark("move_backup_trashed").unwrap();
        assert_eq!(journal.maintain().unwrap(), 2);
        assert_eq!(record.saved().unwrap(), "saved");
        assert_eq!(pending.saved().unwrap(), "saved");
        assert_eq!(
            stage
                .read_bounded_regular(&component("original.txt").unwrap(), limit(16))
                .unwrap()
                .as_deref(),
            Some(b"active".as_slice())
        );
        assert!(!record.marked(EXPIRING).unwrap());
        assert!(!pending.marked(EXPIRING).unwrap());
    }
    #[cfg(not(windows))]
    #[test]
    fn committed_expiry_resumes_after_payload_receipt_or_marker_removal_without_file_replay() {
        for removed in [
            vec!["saved"],
            vec!["saved", "trashed", "restored", "receipt"],
            vec!["saved", "trashed", "restored", "receipt", EXPIRING],
        ] {
            let root = root();
            let root_path = root.path().canonicalize().unwrap();
            {
                let journal = Journal::open(&root_path, "profile", true).unwrap().unwrap();
                let record = completed(&journal, &root_path, 1);
                record.mark(EXPIRING).unwrap();
                for file in &removed {
                    record
                        .directory
                        .remove_verified_regular(&component(file).unwrap())
                        .unwrap();
                }
            }
            let journal = Journal::open(&root_path, "profile", false)
                .unwrap()
                .unwrap();
            assert_eq!(journal.maintain().unwrap(), 0);
            assert!(journal
                .directory
                .list_components(MAX_RECORDS)
                .unwrap()
                .is_empty());
            assert!(!root_path.join("original-1.txt").exists());
        }
    }
    #[cfg(not(windows))]
    #[test]
    fn unknown_content_and_incomplete_payloads_are_preserved() {
        let root = root();
        let root_path = root.path().canonicalize().unwrap();
        let journal = Journal::open(&root_path, "profile", true).unwrap().unwrap();
        let record = completed(&journal, &root_path, 1);
        record.mark(EXPIRING).unwrap();
        record
            .directory
            .write_new_synced(&component("unexpected").unwrap(), b"keep", limit(16))
            .unwrap();
        assert!(journal.maintain().is_err());
        assert_eq!(record.saved().unwrap(), "saved");
        assert_eq!(
            record
                .directory
                .read_bounded_regular(&component("unexpected").unwrap(), limit(16))
                .unwrap()
                .as_deref(),
            Some(b"keep".as_slice())
        );
        record
            .directory
            .remove_verified_regular(&component("unexpected").unwrap())
            .unwrap();
        journal.maintain().unwrap();
        let orphan = journal
            .directory
            .create_new_private_child(&component(&format!("{:032x}", 2)).unwrap())
            .unwrap();
        orphan
            .write_new_synced(&component("saved").unwrap(), b"unclassified", limit(16))
            .unwrap();
        assert_eq!(journal.maintain().unwrap(), 1);
        assert_eq!(
            orphan
                .read_bounded_regular(&component("saved").unwrap(), limit(16))
                .unwrap()
                .as_deref(),
            Some(b"unclassified".as_slice())
        );
    }

    #[cfg(windows)]
    #[test]
    fn journal_fails_closed_without_windows_namespace_validation() {
        let root = root();
        let root_path = root.path().canonicalize().unwrap();

        assert!(matches!(
            Journal::open(&root_path, "profile", true),
            Err(Failure::Storage)
        ));
        assert!(!root_path.join(".zephium-work-recovery").exists());
    }
}
