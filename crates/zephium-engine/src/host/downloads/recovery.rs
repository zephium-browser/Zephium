//! Bounded profile recovery, independent of the Downloads UI and its pagination.
use super::*;

const RECOVERY_PAGE: u32 = 50;

#[derive(Default)]
pub(super) struct Recovery {
    initialized: HashSet<ProfileId>,
    queue: VecDeque<ProfileId>,
    queued: HashSet<ProfileId>,
    active: Option<ProfileId>,
    after: Option<DownloadId>,
    next: Option<DownloadId>,
    acknowledgements: usize,
    rescan: HashSet<ProfileId>,
    retiring: HashSet<ProfileId>,
    private_profiles: HashSet<ProfileId>,
    volatile: HashMap<DownloadId, (ProfileId, DownloadRecord)>,
    pub errors: HashMap<ProfileId, DownloadError>,
}

impl Recovery {
    fn remember_private(&mut self, profile: ProfileId, record: &DownloadRecord) {
        self.private_profiles.insert(profile);
        self.initialized.insert(profile);
        if record.staging.is_some() && record.staging_identity.is_some() {
            self.volatile.insert(record.id, (profile, record.clone()));
        } else if let Some((_, receipt)) = self.volatile.get_mut(&record.id) {
            // Publication clears the UI record's staging fields, never the
            // independent ownership receipt before a cleanup acknowledgement.
            receipt.state = record.state;
            receipt.writer_released = record.writer_released;
        }
    }
    fn private_page(&self, profile: ProfileId) -> Vec<DownloadRecord> {
        let mut records: Vec<_> = self
            .volatile
            .values()
            .filter(|(owner, record)| {
                *owner == profile
                    && record.state.terminal()
                    && self.after.is_none_or(|after| record.id > after)
            })
            .map(|(_, record)| record.clone())
            .collect();
        records.sort_by_key(|record| record.id);
        records.truncate(RECOVERY_PAGE as usize);
        records
    }
    fn release_private(
        &mut self,
        profile: ProfileId,
        id: DownloadId,
        expected: &FileIdentity,
    ) -> bool {
        let matches = self.volatile.get(&id).is_some_and(|(owner, record)| {
            *owner == profile && record.staging_identity.as_ref() == Some(expected)
        });
        if matches {
            self.volatile.remove(&id);
        }
        matches
    }
}

impl Downloads {
    fn can_recover(&self, profile: ProfileId) -> bool {
        let state = self.recovery.borrow();
        if self.stopping.get() {
            return state.private_profiles.contains(&profile) && state.retiring.contains(&profile);
        }
        !self.retired.borrow().contains(&profile) || state.retiring.contains(&profile)
    }

    pub(super) fn private_cleanup_capacity(&self, partition: Partition) -> bool {
        !matches!(partition, Partition::Ephemeral(_))
            || self.recovery.borrow().volatile.len()
                + self
                    .active
                    .borrow()
                    .values()
                    .filter(|transfer| {
                        matches!(transfer.partition, Partition::Ephemeral(_))
                            && transfer.record.staging.is_none()
                    })
                    .count()
                < RECENT_LIMIT
    }

    pub(super) fn remember_volatile_cleanup(&self, profile: ProfileId, record: &DownloadRecord) {
        self.recovery.borrow_mut().remember_private(profile, record);
    }

    pub(super) fn begin_shutdown_recovery(self: &Rc<Self>) {
        let profiles: Vec<_> = self
            .recovery
            .borrow()
            .private_profiles
            .iter()
            .copied()
            .collect();
        for profile in profiles {
            self.begin_retirement_recovery(profile);
        }
    }

    pub(super) fn begin_retirement_recovery(self: &Rc<Self>, profile: ProfileId) {
        // Private profiles never enter the durable registry. They only drain
        // their in-memory native transfers and filesystem work.
        if self.recovery.borrow().initialized.contains(&profile) {
            self.recovery.borrow_mut().retiring.insert(profile);
            self.schedule_recovery(profile);
        }
    }

    pub(super) fn recovery_busy(&self, profile: Option<ProfileId>) -> bool {
        let state = self.recovery.borrow();
        if let Some(profile) = profile {
            state.active == Some(profile) || state.queued.contains(&profile)
        } else {
            state
                .active
                .is_some_and(|profile| state.private_profiles.contains(&profile))
                || state
                    .queue
                    .iter()
                    .any(|profile| state.private_profiles.contains(profile))
        }
    }

    pub(super) fn finish_retirement_recovery(&self, profile: Option<ProfileId>) -> bool {
        let mut state = self.recovery.borrow_mut();
        let Some(profile) = profile else {
            return state.volatile.is_empty();
        };
        state.retiring.remove(&profile);
        let clean = !state.errors.contains_key(&profile)
            && !state.volatile.values().any(|(owner, _)| *owner == profile);
        if clean {
            state.initialized.remove(&profile);
            state.private_profiles.remove(&profile);
        }
        clean
    }

    pub(super) fn cleanup_status(&self, profile: ProfileId) -> DownloadCleanup {
        DownloadCleanup {
            running: self.recovery_busy(Some(profile)),
            error: self.recovery.borrow().errors.get(&profile).copied().or(self
                .persistence_failed
                .get()
                .then_some(DownloadError::Storage)),
        }
    }

    pub(super) fn initialize_recovery(self: &Rc<Self>) {
        self.work.set(self.work.get() + 1);
        let sender = self.sender.clone();
        if !self
            .store
            .download_recovery_profiles(Box::new(move |reply| {
                let _ = sender.send(Message::RecoveryProfiles(reply));
            }))
        {
            let _ = self
                .sender
                .send(Message::RecoveryProfiles(DownloadStoreReply::Error(
                    DownloadError::Storage,
                )));
        }
        self.ensure_timer();
    }

    pub(super) fn ensure_recovery(self: &Rc<Self>, partition: Partition) {
        if matches!(partition, Partition::Ephemeral(_)) {
            let mut state = self.recovery.borrow_mut();
            state.initialized.insert(partition.profile());
            state.private_profiles.insert(partition.profile());
            return;
        }
        if self
            .recovery
            .borrow_mut()
            .initialized
            .insert(partition.profile())
        {
            if self.retired.borrow().contains(&partition.profile())
                && self
                    .waiters
                    .borrow()
                    .iter()
                    .any(|(profile, _)| *profile == Some(partition.profile()))
            {
                self.recovery
                    .borrow_mut()
                    .retiring
                    .insert(partition.profile());
            }
            self.schedule_recovery(partition.profile());
        }
    }

    pub(super) fn schedule_recovery(self: &Rc<Self>, profile: ProfileId) {
        if !self.can_recover(profile) {
            return;
        }
        {
            let mut state = self.recovery.borrow_mut();
            if state.active == Some(profile) {
                state.rescan.insert(profile);
                return;
            }
            if state.queued.insert(profile) {
                state.queue.push_back(profile);
            }
        }
        self.next_recovery_profile();
        self.ensure_timer();
    }

    fn next_recovery_profile(self: &Rc<Self>) {
        if self.recovery.borrow().active.is_some() {
            return;
        }
        let profile = {
            let mut state = self.recovery.borrow_mut();
            let mut profile = None;
            while let Some(candidate) = state.queue.pop_front() {
                state.queued.remove(&candidate);
                if (!self.stopping.get()
                    || (state.private_profiles.contains(&candidate)
                        && state.retiring.contains(&candidate)))
                    && (!self.retired.borrow().contains(&candidate)
                        || state.retiring.contains(&candidate))
                {
                    profile = Some(candidate);
                    break;
                }
            }
            state.active = profile;
            state.after = None;
            state.next = None;
            if let Some(profile) = profile {
                state.errors.remove(&profile);
            }
            profile
        };
        if let Some(profile) = profile {
            self.read_recovery_page(profile);
        }
    }

    fn read_recovery_page(&self, profile: ProfileId) {
        if self.recovery.borrow().private_profiles.contains(&profile) {
            let records = self.recovery.borrow().private_page(profile);
            self.work.set(self.work.get() + 1);
            let _ = self.sender.send(Message::RecoveryPage(
                profile,
                DownloadStoreReply::Page(records),
            ));
            return;
        }

        let active = self
            .active
            .borrow()
            .iter()
            .filter(|(_, transfer)| transfer.partition.profile() == profile)
            .map(|(id, _)| *id)
            .collect();
        let after = self.recovery.borrow().after;
        self.work.set(self.work.get() + 1);
        let sender = self.sender.clone();
        if !self.store.download_call(
            profile,
            DownloadStoreCall::RecoveryPage {
                after,
                limit: RECOVERY_PAGE,
                session: self.session,
                active,
            },
            Box::new(move |reply| {
                let _ = sender.send(Message::RecoveryPage(profile, reply));
            }),
        ) {
            let _ = self.sender.send(Message::RecoveryPage(
                profile,
                DownloadStoreReply::Error(DownloadError::Storage),
            ));
        }
    }

    pub(super) fn recovery_profiles(self: &Rc<Self>, reply: DownloadStoreReply) {
        if let DownloadStoreReply::Profiles(profiles) = reply {
            for profile in profiles {
                self.ensure_recovery(Partition::Persistent(profile));
            }
        } else {
            self.persistence_failed.set(true);
        }
    }

    pub(super) fn recovery_page(self: &Rc<Self>, profile: ProfileId, reply: DownloadStoreReply) {
        if !self.can_recover(profile) {
            self.finish_recovery_page(profile, None);
            return;
        }
        let DownloadStoreReply::Page(records) = reply else {
            self.recovery
                .borrow_mut()
                .errors
                .insert(profile, DownloadError::Storage);
            self.finish_recovery_page(profile, None);
            return;
        };
        let next = (records.len() == RECOVERY_PAGE as usize)
            .then(|| records.last().map(|record| record.id))
            .flatten();
        if records.is_empty() {
            self.finish_recovery_page(profile, None);
            return;
        }
        self.work.set(self.work.get() + 1);
        let sender = self.sender.clone();
        if std::thread::Builder::new()
            .name("zephium-download-recovery".into())
            .spawn(move || {
                let mut cleaned = Vec::new();
                let mut failure = None;
                for record in records {
                    match super::super::download_files::recover_staging(&record) {
                        Ok(()) => {
                            if let Some(identity) = record.staging_identity {
                                cleaned.push((record.id, identity));
                            }
                        }
                        Err(error) => {
                            failure.get_or_insert(error);
                        }
                    }
                }
                let _ = sender.send(Message::Recovered(profile, cleaned, next, failure));
            })
            .is_err()
        {
            self.work.set(self.work.get() - 1);
            self.recovery
                .borrow_mut()
                .errors
                .insert(profile, DownloadError::Unavailable);
            self.finish_recovery_page(profile, None);
        }
    }

    pub(super) fn recovered(
        self: &Rc<Self>,
        profile: ProfileId,
        records: Vec<(DownloadId, FileIdentity)>,
        next: Option<DownloadId>,
        error: Option<DownloadError>,
    ) {
        {
            let mut state = self.recovery.borrow_mut();
            state.next = next;
            state.acknowledgements = records.len();
            if let Some(error) = error {
                state.errors.insert(profile, error);
            }
        }
        if records.is_empty() {
            self.finish_recovery_page(profile, next);
            return;
        }
        for (id, expected) in records {
            self.work.set(self.work.get() + 1);
            if self.recovery.borrow().private_profiles.contains(&profile) {
                let matches = self
                    .recovery
                    .borrow_mut()
                    .release_private(profile, id, &expected);
                let reply = if matches {
                    DownloadStoreReply::Saved
                } else {
                    DownloadStoreReply::Error(DownloadError::ChangedFile)
                };
                let _ = self
                    .sender
                    .send(Message::RecoverySaved(profile, id, expected, reply));
                continue;
            }

            let sender = self.sender.clone();
            let identity = expected.clone();
            let unavailable = expected.clone();
            if !self.store.download_call(
                profile,
                DownloadStoreCall::ClearStaging { id, expected },
                Box::new(move |reply| {
                    let _ = sender.send(Message::RecoverySaved(profile, id, identity, reply));
                }),
            ) {
                let _ = self.sender.send(Message::RecoverySaved(
                    profile,
                    id,
                    unavailable,
                    DownloadStoreReply::Error(DownloadError::Storage),
                ));
            }
        }
    }

    pub(super) fn recovery_saved(
        self: &Rc<Self>,
        profile: ProfileId,
        id: DownloadId,
        expected: FileIdentity,
        reply: DownloadStoreReply,
    ) {
        if matches!(reply, DownloadStoreReply::Saved) {
            for (_, record) in self
                .recent
                .borrow_mut()
                .iter_mut()
                .filter(|(owner, record)| owner.profile() == profile && record.id == id)
            {
                if record.staging_identity.as_ref() == Some(&expected) {
                    record.staging = None;
                    record.staging_identity = None;
                }
            }
        } else {
            self.recovery
                .borrow_mut()
                .errors
                .insert(profile, DownloadError::Storage);
        }
        let next = {
            let mut state = self.recovery.borrow_mut();
            state.acknowledgements -= 1;
            (state.acknowledgements == 0).then_some(state.next)
        };
        if let Some(next) = next {
            self.finish_recovery_page(profile, next);
        }
    }

    fn finish_recovery_page(self: &Rc<Self>, profile: ProfileId, next: Option<DownloadId>) {
        if next.is_some() && self.can_recover(profile) {
            self.recovery.borrow_mut().after = next;
            self.read_recovery_page(profile);
            return;
        }
        let allowed = self.can_recover(profile);
        {
            let mut state = self.recovery.borrow_mut();
            state.active = None;
            state.after = None;
            if state.rescan.remove(&profile) && allowed && state.queued.insert(profile) {
                state.queue.push_back(profile);
            }
        }
        self.changed(profile);
        self.next_recovery_profile();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn private_publication_retains_cleanup_until_exact_profile_receipt_is_acknowledged() {
        let profile = ProfileId::from(1);
        let mut state = Recovery::default();
        let mut record = DownloadRecord {
            id: DownloadId::generate(),
            session: DownloadId::generate(),
            revision: 1,
            created_at: 1,
            filename: "private.txt".into(),
            source: "https://example.com".into(),
            source_is_context: false,
            state: DownloadState::Receiving,
            received: 0,
            total: None,
            error: None,
            destination: None,
            staging: Some(std::env::temp_dir().join("owned-private-stage")),
            staging_identity: Some(FileIdentity {
                volume: 1,
                file: 2,
                file_high: 3,
                bytes: 0,
                modified: None,
            }),
            identity: None,
            writer: None,
            writer_released: false,
        };
        let identity = record.staging_identity.clone().unwrap();
        state.remember_private(profile, &record);
        assert!(
            state.private_page(profile).is_empty(),
            "a live writer cannot be recovered"
        );
        record.state = DownloadState::Completed;
        record.writer_released = true;
        record.staging = None;
        record.staging_identity = None;
        state.remember_private(profile, &record);
        let page = state.private_page(profile);
        assert_eq!(page.len(), 1);
        assert!(
            page[0].staging.is_some(),
            "publication does not abandon cleanup"
        );
        assert!(page[0].writer_released);
        assert!(state.private_page(ProfileId::from(2)).is_empty());
        assert!(!state.release_private(ProfileId::from(2), record.id, &identity));
        let mut wrong = identity.clone();
        wrong.file_high ^= 1;
        assert!(!state.release_private(profile, record.id, &wrong));
        assert!(state.release_private(profile, record.id, &identity));
        assert!(state.volatile.is_empty());
    }
}
