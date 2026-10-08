//! Bounded session persistence scheduling and snapshot durability.

use super::*;

pub(super) const PERSIST_DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(400);
pub(super) const PERSIST_MAX_AGE: std::time::Duration = std::time::Duration::from_secs(2);
// URL-only native observations are recoverability checkpoints, not structural
// mutations. Structural changes retain the fast debounce; URL churn is
// globally coalesced to one full snapshot per five minutes.
pub(super) const URL_CHECKPOINT_INTERVAL: std::time::Duration =
    std::time::Duration::from_secs(5 * 60);
pub(super) const URL_CHECKPOINT_DEBOUNCE: std::time::Duration = std::time::Duration::from_secs(5);

pub(super) struct PersistenceState {
    /// Monotonic process-local identity for the session state represented by
    /// persistence scheduling. A u128 wrap would require more mutations than
    /// the process can physically execute; wrapping keeps this path infallible
    /// in release builds while preserving a fail-safe practical bound.
    pub(super) session_revision: u128,
    pub(super) persist_first_dirty: Option<std::time::Instant>,
    pub(super) url_checkpoint_dirty: std::collections::HashSet<ItemId>,
    pub(super) last_url_checkpoint: std::time::Instant,
}

impl Default for PersistenceState {
    fn default() -> Self {
        Self {
            session_revision: 0,
            persist_first_dirty: None,
            url_checkpoint_dirty: std::collections::HashSet::new(),
            last_url_checkpoint: std::time::Instant::now(),
        }
    }
}

impl Shell {
    pub(super) fn schedule_persist(&mut self) {
        if !self.bootstrapped {
            return;
        }
        self.persistence.session_revision = self.persistence.session_revision.wrapping_add(1);
        self.schedule_current_session_persist();
    }

    pub(super) fn schedule_url_checkpoint(&mut self, id: ItemId) {
        if !self.bootstrapped || self.items.tab(id).is_none() {
            return;
        }
        self.persistence.session_revision = self.persistence.session_revision.wrapping_add(1);
        let first_url_dirty = self.persistence.url_checkpoint_dirty.is_empty();
        self.persistence.url_checkpoint_dirty.insert(id);
        // A pending structural snapshot already includes the newest URL and
        // retains its much shorter durability deadline.
        if self.persistence.persist_first_dirty.is_some() || !first_url_dirty {
            return;
        }
        let now = std::time::Instant::now();
        let interval_floor = self
            .persistence
            .last_url_checkpoint
            .checked_add(URL_CHECKPOINT_INTERVAL)
            .unwrap_or(now + URL_CHECKPOINT_INTERVAL);
        let deadline = interval_floor.max(now + URL_CHECKPOINT_DEBOUNCE);
        if let Some(queue) = self.self_queue.as_ref() {
            queue.schedule_persist(deadline);
        } else {
            #[cfg(test)]
            {
                // Deterministic unit shells do not own the production timer.
                self.persist();
            }
            #[cfg(not(test))]
            {
                // Production construction always installs the timer before
                // the actor can receive a URL. If that invariant changes,
                // defer to the exact shutdown snapshot instead of restoring
                // hostile per-URL full rewrites.
                crate::diagnostic!("persistence: URL checkpoint timer is unavailable");
            }
        }
    }

    /// Schedules the current authoritative state without advancing its logical
    /// revision. Used after a durable deletion barrier when mutations already
    /// counted by `schedule_persist` need a new post-barrier debounce.
    pub(super) fn schedule_current_session_persist(&mut self) {
        if !self.bootstrapped {
            return;
        }
        let now = std::time::Instant::now();
        let first = *self.persistence.persist_first_dirty.get_or_insert(now);
        let deadline = (now + PERSIST_DEBOUNCE).min(first + PERSIST_MAX_AGE);
        if let Some(queue) = self.self_queue.as_ref() {
            queue.schedule_persist(deadline);
        } else {
            // Directly-constructed shells are used by deterministic unit
            // tests and embedders without the production timer thread.
            self.persist();
        }
    }

    pub(super) fn persist(&mut self) {
        self.persistence.persist_first_dirty = None;
        // The durable session is loaded by Bootstrap. Before that ordered
        // point, an empty in-memory shell is not authoritative: persisting it
        // during an immediate quit would erase a valid previous session.
        if !self.bootstrapped {
            return;
        }
        self.persistence.url_checkpoint_dirty.clear();
        self.persistence.last_url_checkpoint = std::time::Instant::now();
        let (space, active, splits) = self.persisted_scope();
        let state = session::snapshot_with_recently_closed(
            &self.profiles,
            &self.spaces,
            &self.items,
            space,
            active,
            splits,
            &self.recently_closed,
        );
        self.store.save_session(state);
    }
}
