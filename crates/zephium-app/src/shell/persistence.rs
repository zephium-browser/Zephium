//! Bounded session persistence scheduling and snapshot durability.

use super::*;

pub(super) const PERSIST_DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(400);
pub(super) const PERSIST_MAX_AGE: std::time::Duration = std::time::Duration::from_secs(2);
// A committed address changes no structure, so it waits a little longer than
// one: long enough that a redirect chain or a script rewriting its URL costs
// one snapshot, short enough that a crash restores the page the person was on.
// The store adds its own debounce (at most 400 ms) before the write.
pub(super) const URL_CHECKPOINT_DELAY: std::time::Duration = std::time::Duration::from_secs(1);

#[derive(Default)]
pub(super) struct PersistenceState {
    /// Monotonic process-local identity for the session state represented by
    /// persistence scheduling. A u128 wrap would require more mutations than
    /// the process can physically execute; wrapping keeps this path infallible
    /// in release builds while preserving a fail-safe practical bound.
    pub(super) session_revision: u128,
    pub(super) persist_first_dirty: Option<std::time::Instant>,
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
        // A snapshot already waiting is taken when it fires, so it carries
        // this address too; only the first change since the last one arms it.
        if self.persistence.persist_first_dirty.is_some() {
            return;
        }
        let now = std::time::Instant::now();
        self.persistence.persist_first_dirty = Some(now);
        match self.self_queue.as_ref() {
            Some(queue) => queue.schedule_persist(now + URL_CHECKPOINT_DELAY),
            None => self.persist(),
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
