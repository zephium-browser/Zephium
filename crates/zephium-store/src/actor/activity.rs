//! Retained, ordered activity writes. Admission accounts for both channel
//! entries and failed writes, so moving work into the retry FIFO never opens
//! an unbounded second mailbox.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use zephium_core::ids::ProfileId;
use zephium_core::time::{FocusRecord, HourTally, Place, MAX_PENDING_TALLIES, MAX_SITE_BYTES};

use super::{flush, Hub, PendingSession, WriteRetry};
use crate::hub::TimeBatchId;

pub(super) const MAX_PENDING_ACTIVITY_WRITES: usize = 64;
pub(super) const MAX_PENDING_ACTIVITY_WRITES_PER_SCOPE: usize = 16;
const MAX_PENDING_SITE_BYTES: usize = MAX_PENDING_TALLIES * MAX_SITE_BYTES;

pub(super) enum ActivityWrite {
    RecordTime(ProfileId, TimeBatchId, Vec<HourTally>, i64),
    ClearTime(ProfileId, Option<i64>),
    RecordFocus(FocusRecord, i64),
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum ActivityScope {
    Profile(ProfileId),
    Focus,
}

impl ActivityWrite {
    fn scope(&self) -> ActivityScope {
        self.profile()
            .map_or(ActivityScope::Focus, ActivityScope::Profile)
    }
    fn profile(&self) -> Option<ProfileId> {
        match self {
            Self::RecordTime(profile, ..) | Self::ClearTime(profile, ..) => Some(*profile),
            Self::RecordFocus(..) => None,
        }
    }

    fn allocation_cost(&self) -> Option<(usize, usize)> {
        let Self::RecordTime(_, _, tallies, _) = self else {
            return Some((0, 0));
        };
        // Charge retained allocation capacities, not only logical lengths.
        // An oversized in-process DTO cannot hide heap retention in spare
        // vector/string capacity while occupying one small command slot.
        if tallies.capacity() > MAX_PENDING_TALLIES {
            return None;
        }
        let mut site_bytes = 0usize;
        for tally in tallies {
            if let Place::Site(site) = &tally.place {
                if site.len() > MAX_SITE_BYTES {
                    return None;
                }
                site_bytes = site_bytes.checked_add(site.capacity())?;
            }
        }
        Some((tallies.capacity(), site_bytes))
    }

    fn commit(&self, hub: &mut Hub) -> rusqlite::Result<()> {
        match self {
            Self::RecordTime(profile, batch, tallies, keep_from_hour) => {
                hub.record_time_batch(*profile, *batch, tallies, *keep_from_hour)
            }
            Self::ClearTime(profile, since_hour) => hub.clear_time(*profile, *since_hour),
            Self::RecordFocus(record, day) => hub.record_focus(record, *day),
        }
    }
}

#[derive(Default)]
pub(super) struct ActivityAdmission {
    pub(super) commands: usize,
    tally_slots: usize,
    site_bytes: usize,
    scopes: HashMap<ActivityScope, usize>,
    quarantined: HashSet<ActivityScope>,
}

pub(super) struct ActivityWritePermit {
    admission: Arc<Mutex<ActivityAdmission>>,
    tally_slots: usize,
    site_bytes: usize,
    scope: ActivityScope,
}

impl ActivityWritePermit {
    pub(super) fn acquire(
        admission: &Arc<Mutex<ActivityAdmission>>,
        write: &ActivityWrite,
    ) -> Option<Self> {
        let (tally_slots, site_bytes) = write.allocation_cost()?;
        // This mutex protects only bounded counters, never SQLite or native
        // work. Wait for a brief competing admission/drop instead of refusing
        // a completed focus record merely because another producer won it.
        let mut state = admission.lock().unwrap_or_else(|p| p.into_inner());
        let scope = write.scope();
        let scope_count = state
            .scopes
            .get(&scope)
            .copied()
            .unwrap_or(0)
            .checked_add(1)?;
        // A clear replaces whatever is stuck before it, so it is never the
        // write a quarantine refuses.
        let clears = matches!(write, ActivityWrite::ClearTime(..));
        if (state.quarantined.contains(&scope) && !clears)
            || scope_count > MAX_PENDING_ACTIVITY_WRITES_PER_SCOPE
        {
            return None;
        }
        let commands = state.commands.checked_add(1)?;
        let next_slots = state.tally_slots.checked_add(tally_slots)?;
        let next_bytes = state.site_bytes.checked_add(site_bytes)?;
        if commands > MAX_PENDING_ACTIVITY_WRITES
            || next_slots > MAX_PENDING_TALLIES
            || next_bytes > MAX_PENDING_SITE_BYTES
        {
            return None;
        }
        state.commands = commands;
        state.tally_slots = next_slots;
        state.site_bytes = next_bytes;
        state.scopes.insert(scope, scope_count);
        Some(Self {
            admission: admission.clone(),
            tally_slots,
            site_bytes,
            scope,
        })
    }

    fn quarantine(&self, quarantined: bool) {
        let mut state = self.admission.lock().unwrap_or_else(|p| p.into_inner());
        if quarantined {
            state.quarantined.insert(self.scope);
        } else {
            state.quarantined.remove(&self.scope);
        }
    }
}

impl Drop for ActivityWritePermit {
    fn drop(&mut self) {
        let mut state = self.admission.lock().unwrap_or_else(|p| p.into_inner());
        let remaining = state.commands.checked_sub(1).zip(
            state
                .tally_slots
                .checked_sub(self.tally_slots)
                .zip(state.site_bytes.checked_sub(self.site_bytes)),
        );
        if let Some((commands, (tally_slots, site_bytes))) = remaining {
            state.commands = commands;
            state.tally_slots = tally_slots;
            state.site_bytes = site_bytes;
            let scope_count = state
                .scopes
                .get(&self.scope)
                .copied()
                .and_then(|count| count.checked_sub(1));
            match scope_count {
                Some(0) => {
                    state.scopes.remove(&self.scope);
                }
                Some(count) => {
                    state.scopes.insert(self.scope, count);
                }
                None => {
                    state.commands = usize::MAX;
                }
            }
        } else {
            // Accounting failure closes admission instead of reopening an
            // effectively unbounded mailbox.
            state.commands = usize::MAX;
            state.tally_slots = usize::MAX;
            state.site_bytes = usize::MAX;
        }
    }
}

#[derive(Default)]
struct ActivityLane {
    queue: VecDeque<(ActivityWrite, ActivityWritePermit)>,
    retry: WriteRetry,
    permanent_failure: bool,
}

fn transient_activity_error(error: &rusqlite::Error) -> bool {
    use rusqlite::ErrorCode::*;
    matches!(error, rusqlite::Error::SqliteFailure(code, _) if matches!(code.code,
        DatabaseBusy | DatabaseLocked | OutOfMemory | OperationInterrupted |
        SystemIoFailure | DiskFull | CannotOpen | FileLockingProtocolFailed | SchemaChanged
    ))
}

#[derive(Default)]
pub(super) struct PendingActivityWrites {
    lanes: HashMap<ActivityScope, ActivityLane>,
    uncertain_profiles: HashSet<ProfileId>,
    authorization_limit_uncertain: bool,
}

impl PendingActivityWrites {
    pub(super) fn deadline(&self) -> Option<Instant> {
        self.lanes
            .values()
            .filter_map(|lane| lane.retry.retry_at)
            .min()
    }

    pub(super) fn push(&mut self, write: ActivityWrite, permit: ActivityWritePermit) {
        let lane = self.lanes.entry(write.scope()).or_default();
        if matches!(write, ActivityWrite::ClearTime(_, None)) {
            // Clearing all time erases every tally still waiting to be
            // written, including one that keeps failing.
            lane.queue
                .retain(|(queued, _)| !matches!(queued, ActivityWrite::RecordTime(..)));
            lane.permanent_failure = false;
            lane.retry.clear();
            permit.quarantine(false);
        }
        lane.queue.push_back((write, permit));
    }

    pub(super) fn forget_profile(&mut self, profile: ProfileId) {
        if let Some(lane) = self.lanes.remove(&ActivityScope::Profile(profile)) {
            if let Some((_, permit)) = lane.queue.front() {
                permit.quarantine(false);
            }
        }
        self.uncertain_profiles.remove(&profile);
    }

    pub(super) fn quarantine_profile(&mut self, profile: ProfileId) {
        if self.uncertain_profiles.len() < zephium_core::session::MAX_SESSION_PROFILES {
            self.uncertain_profiles.insert(profile);
        } else {
            self.authorization_limit_uncertain = true;
        }
    }

    pub(super) fn reconciled_authorizations(&mut self) {
        self.uncertain_profiles.clear();
        self.authorization_limit_uncertain = false;
        for lane in self.lanes.values_mut() {
            if !lane.permanent_failure && !lane.queue.is_empty() {
                lane.retry.retry_at = Some(Instant::now());
            }
        }
    }

    pub(super) fn flush_profile(
        &mut self,
        profile: ProfileId,
        hub: &mut Hub,
        session: &mut Option<PendingSession>,
        explicit_retry: bool,
    ) -> bool {
        self.flush_scope(
            ActivityScope::Profile(profile),
            hub,
            session,
            explicit_retry,
        )
    }

    pub(super) fn flush_focus(
        &mut self,
        hub: &mut Hub,
        session: &mut Option<PendingSession>,
    ) -> bool {
        self.flush_scope(ActivityScope::Focus, hub, session, false)
    }

    /// True when everything still queued is recorded time or focus that
    /// failed permanently. Shutdown does not wait on those: the process ends
    /// either way, and holding the exit unclean would only postpone updates.
    /// A pending clear always counts.
    pub(super) fn only_unrecoverable_records(&self) -> bool {
        self.lanes.values().all(|lane| {
            lane.queue.is_empty()
                || (lane.permanent_failure
                    && lane.queue.iter().all(|(write, _)| {
                        matches!(
                            write,
                            ActivityWrite::RecordTime(..) | ActivityWrite::RecordFocus(..)
                        )
                    }))
        })
    }

    pub(super) fn flush(
        &mut self,
        hub: &mut Hub,
        session: &mut Option<PendingSession>,
        explicit_retry: bool,
    ) -> bool {
        let scopes: Vec<_> = self.lanes.keys().copied().collect();
        let mut durable = true;
        for scope in scopes {
            // Always visit every scope: an unrelated failure cannot prevent
            // another profile's or the person's focus data from committing.
            durable &= self.flush_scope(scope, hub, session, explicit_retry);
        }
        durable
    }

    fn flush_scope(
        &mut self,
        scope: ActivityScope,
        hub: &mut Hub,
        session: &mut Option<PendingSession>,
        explicit_retry: bool,
    ) -> bool {
        let blocked = matches!(scope, ActivityScope::Profile(profile)
            if self.authorization_limit_uncertain || self.uncertain_profiles.contains(&profile));
        let Some(lane) = self.lanes.get_mut(&scope) else {
            return true;
        };
        if blocked {
            lane.retry.retry_at = None;
            return false;
        }
        if !explicit_retry
            && (lane.permanent_failure || lane.retry.retry_at.is_some_and(|at| Instant::now() < at))
        {
            return false;
        }
        while let Some((write, permit)) = lane.queue.front() {
            if let Some(profile) = write.profile() {
                if !hub.knows(profile) && hub.recovery_reason().is_none() {
                    let registration = session.as_ref().is_some_and(|save| {
                        save.state.profiles.iter().any(|owner| owner.id == profile)
                    });
                    if registration {
                        if !explicit_retry {
                            if let Some(at) = session
                                .as_ref()
                                .and_then(|save| save.retry_at)
                                .filter(|at| Instant::now() < *at)
                            {
                                lane.retry.retry_at = Some(at);
                                return false;
                            }
                        }
                        if !flush(hub, session) {
                            lane.retry.failed(Instant::now());
                            return false;
                        }
                    }
                    if !hub.knows(profile) {
                        // Never create a database from private/retired/unknown
                        // activity. Authoritative registration owns that edge.
                        permit.quarantine(false);
                        lane.queue.pop_front();
                        lane.retry.clear();
                        continue;
                    }
                }
            }
            match write.commit(hub) {
                Ok(()) => {
                    permit.quarantine(false);
                    lane.permanent_failure = false;
                    lane.queue.pop_front();
                    lane.retry.clear();
                }
                Err(error) if transient_activity_error(&error) => {
                    permit.quarantine(false);
                    lane.permanent_failure = false;
                    lane.retry.failed(Instant::now());
                    return false;
                }
                Err(_) => {
                    eprintln!("store: an activity write failed and is held until the next flush");
                    permit.quarantine(true);
                    lane.permanent_failure = true;
                    lane.retry.clear();
                    return false;
                }
            }
        }
        self.lanes.remove(&scope);
        true
    }
}
