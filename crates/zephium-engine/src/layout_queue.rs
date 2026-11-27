//! Bounded latest-value admission for replaceable native layout facts.
//!
//! Window resize and divider drag events can arrive faster than a platform
//! event loop can perform native placement. Keeping every intermediate frame
//! only increases latency; one latest value per live window is sufficient.

use std::collections::BTreeMap;
use std::sync::{Mutex, MutexGuard};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Submit {
    /// The caller owns scheduling the one native event-loop task.
    Schedule,
    /// An existing task will observe this replacement.
    Coalesced,
    /// A new window key exceeded the independently bounded registry.
    Full,
}

struct State<T> {
    pending: BTreeMap<u64, T>,
    scheduled: bool,
}

pub(crate) struct LatestLayouts<T> {
    state: Mutex<State<T>>,
    max_windows: usize,
}

impl<T> LatestLayouts<T> {
    pub(crate) fn new(max_windows: usize) -> Self {
        Self {
            state: Mutex::new(State {
                pending: BTreeMap::new(),
                scheduled: false,
            }),
            max_windows,
        }
    }

    #[cfg(test)]
    pub(crate) fn submit(&self, window: u64, value: T) -> Submit {
        self.submit_with(window, value, |_, _| {})
    }

    /// Like `submit`, but a value replacing one still pending first takes
    /// what `carry` keeps of it, under the same lock as the replacement.
    pub(crate) fn submit_with(
        &self,
        window: u64,
        mut value: T,
        carry: impl FnOnce(&T, &mut T),
    ) -> Submit {
        let mut state = self.lock();
        if !state.pending.contains_key(&window) && state.pending.len() >= self.max_windows {
            return Submit::Full;
        }
        if let Some(replaced) = state.pending.get(&window) {
            carry(replaced, &mut value);
        }
        state.pending.insert(window, value);
        if state.scheduled {
            Submit::Coalesced
        } else {
            state.scheduled = true;
            Submit::Schedule
        }
    }

    /// Take the latest frame for every pending window. `scheduled` remains
    /// true while the caller performs native work, so concurrent submissions
    /// are retained for a later event-loop turn.
    pub(crate) fn take_batch(&self) -> Vec<T> {
        let mut state = self.lock();
        std::mem::take(&mut state.pending).into_values().collect()
    }

    /// Finish one event-loop turn. `true` means another bounded task must be
    /// scheduled because a value arrived while the batch was executing.
    pub(crate) fn finish_batch(&self) -> bool {
        let mut state = self.lock();
        if state.pending.is_empty() {
            state.scheduled = false;
            false
        } else {
            true
        }
    }

    /// A dispatcher rejection means no queued task owns these values. Clear
    /// the replaceable facts and reopen admission; the engine separately
    /// reports the failed native transition.
    pub(crate) fn reject_scheduled(&self) {
        let mut state = self.lock();
        state.pending.clear();
        state.scheduled = false;
    }

    /// Drops the pending value of a window that is going away.
    pub(crate) fn forget(&self, window: u64) {
        self.lock().pending.remove(&window);
    }

    /// Amends the first pending value `amend` accepts. `false` when none
    /// does: the value is already being applied, or was never submitted.
    pub(crate) fn amend(&self, amend: impl FnMut(&mut T) -> bool) -> bool {
        self.lock().pending.values_mut().any(amend)
    }

    fn lock(&self) -> MutexGuard<'_, State<T>> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repeated_window_updates_keep_only_the_latest_value() {
        let queue = LatestLayouts::new(2);
        assert_eq!(queue.submit(7, "first"), Submit::Schedule);
        assert_eq!(queue.submit(7, "latest"), Submit::Coalesced);
        assert_eq!(queue.take_batch(), vec!["latest"]);
        assert!(!queue.finish_batch());
    }

    #[test]
    fn updates_arriving_during_a_batch_require_one_more_turn() {
        let queue = LatestLayouts::new(2);
        assert_eq!(queue.submit(7, 1), Submit::Schedule);
        assert_eq!(queue.take_batch(), vec![1]);
        assert_eq!(queue.submit(7, 2), Submit::Coalesced);
        assert!(queue.finish_batch());
        assert_eq!(queue.take_batch(), vec![2]);
        assert!(!queue.finish_batch());
    }

    #[test]
    fn distinct_windows_are_bounded_without_blocking_replacement() {
        let queue = LatestLayouts::new(2);
        assert_eq!(queue.submit(1, "one"), Submit::Schedule);
        assert_eq!(queue.submit(2, "two"), Submit::Coalesced);
        assert_eq!(queue.submit(3, "three"), Submit::Full);
        assert_eq!(queue.submit(1, "new one"), Submit::Coalesced);
        assert_eq!(queue.take_batch(), vec!["new one", "two"]);
        assert!(!queue.finish_batch());
    }

    #[test]
    fn a_forgotten_window_drops_only_its_own_pending_value() {
        let queue = LatestLayouts::new(1);
        assert_eq!(queue.submit(1, "one"), Submit::Schedule);
        queue.forget(2);
        queue.forget(1);
        assert!(queue.take_batch().is_empty());
        assert!(!queue.finish_batch());
        assert_eq!(queue.submit(2, "two"), Submit::Schedule);
    }

    #[test]
    fn a_replacement_carries_only_from_the_value_it_replaces() {
        let queue = LatestLayouts::new(2);
        let keep = |old: &(u8, bool), new: &mut (u8, bool)| new.1 |= old.1;
        assert_eq!(queue.submit_with(1, (1, true), keep), Submit::Schedule);
        assert_eq!(queue.submit_with(2, (2, false), keep), Submit::Coalesced);
        assert_eq!(queue.submit_with(1, (3, false), keep), Submit::Coalesced);
        assert_eq!(queue.take_batch(), vec![(3, true), (2, false)]);
        assert!(!queue.finish_batch());
        assert_eq!(queue.submit_with(1, (4, false), keep), Submit::Schedule);
        assert_eq!(queue.take_batch(), vec![(4, false)]);
    }

    #[test]
    fn only_a_value_still_pending_can_be_amended() {
        let queue = LatestLayouts::new(2);
        assert_eq!(queue.submit(1, 1), Submit::Schedule);
        assert!(!queue.amend(|value| *value == 2));
        assert!(queue.amend(|value| {
            *value += 10;
            true
        }));
        assert_eq!(queue.take_batch(), vec![11]);
        assert!(!queue.amend(|_| true));
    }

    #[test]
    fn rejected_dispatch_drops_unowned_facts_and_reopens_admission() {
        let queue = LatestLayouts::new(1);
        assert_eq!(queue.submit(1, 1), Submit::Schedule);
        queue.reject_scheduled();
        assert!(queue.take_batch().is_empty());
        assert_eq!(queue.submit(2, 2), Submit::Schedule);
    }
}
