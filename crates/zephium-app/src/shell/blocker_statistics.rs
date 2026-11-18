use super::*;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use zephium_core::blocker::{BlockedLoadCounter, BlockerStatistics};
use zephium_ipc::BlockerStatsView;

const FLUSH_INTERVAL: std::time::Duration = std::time::Duration::from_secs(60);
type Reply = std::sync::mpsc::SyncSender<Option<BlockerStatsView>>;

struct State {
    value: Option<BlockerStatistics>,
    loading: bool,
    waiting: Option<Reply>,
    retired: bool,
    private: bool,
    dirty: bool,
    revision: u64,
    pending: Option<(u64, Arc<AtomicU8>)>,
    last_write: std::time::Instant,
}

/// Stored days count from 0001-01-01 as day 1, as chrono's
/// `num_days_from_ce` did when they were first written.
fn day_number(date: jiff::civil::Date) -> i32 {
    const BEFORE_CE: jiff::civil::Date = jiff::civil::Date::constant(0, 12, 31);
    date.since(BEFORE_CE).map_or(0, |span| span.get_days())
}

fn local_day() -> i32 {
    day_number(jiff::Zoned::now().date())
}

#[derive(Clone)]
pub(super) struct Statistics {
    counter: BlockedLoadCounter,
    state: Arc<Mutex<State>>,
}

impl Statistics {
    fn new(private: bool) -> Self {
        Self {
            counter: Arc::new((
                AtomicU64::new(0),
                AtomicBool::new(false),
                AtomicBool::new(true),
            )),
            state: Arc::new(Mutex::new(State {
                value: private.then(BlockerStatistics::default),
                loading: false,
                waiting: None,
                retired: false,
                private,
                dirty: false,
                revision: 0,
                pending: None,
                last_write: std::time::Instant::now(),
            })),
        }
    }

    fn load(&self, profile: ProfileId, store: &SharedStore) {
        {
            let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
            if state.value.is_some() || state.loading || state.retired {
                return;
            }
            state.loading = true;
        }
        let owned = self.clone();
        let storage = store.clone();
        if !store.load_blocker_statistics(
            profile,
            Box::new(move |value| owned.loaded(profile, &storage, value)),
        ) {
            self.loaded(profile, store, None);
        }
    }

    fn loaded(&self, profile: ProfileId, store: &SharedStore, value: Option<BlockerStatistics>) {
        let waiting = {
            let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
            state.loading = false;
            if !state.retired {
                state.value = value;
            }
            state.waiting.take()
        };
        if let Some(reply) = waiting {
            self.reply(profile, store, reply);
        }
    }

    fn reply(&self, profile: ProfileId, store: &SharedStore, reply: Reply) {
        {
            let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
            if !state.retired && state.value.is_none() && state.loading {
                if let Some(previous) = state.waiting.replace(reply) {
                    let _ = previous.try_send(None);
                }
                return;
            }
        }
        let _ = reply.try_send(self.collect(profile, store, false));
    }

    fn collect(
        &self,
        profile: ProfileId,
        store: &SharedStore,
        force: bool,
    ) -> Option<BlockerStatsView> {
        let day = local_day();
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        if state.retired || !self.counter.2.load(Ordering::Relaxed) {
            return None;
        }
        state.value.as_ref()?;
        let count = self.counter.0.swap(0, Ordering::Relaxed);
        if count != 0 {
            state.dirty = true;
            state.revision = state.revision.wrapping_add(1);
        }
        let value = state.value.as_mut()?;
        value.record(day, count);
        let view = BlockerStatsView {
            today: value.today(),
            last_seven_days: value.last_seven_days(),
            days: value.days,
        };
        Self::persist(&mut state, profile, store, force);
        Some(view)
    }

    fn persist(state: &mut State, profile: ProfileId, store: &SharedStore, force: bool) {
        if state.private {
            state.dirty = false;
            return;
        }
        if let Some((revision, result)) = &state.pending {
            match result.load(Ordering::Acquire) {
                0 if !force => return,
                1 if *revision == state.revision => state.dirty = false,
                _ => {}
            }
            state.pending = None;
        }
        if !state.dirty || (!force && state.last_write.elapsed() < FLUSH_INTERVAL) {
            return;
        }
        let Some(value) = state.value.clone() else {
            return;
        };
        let result = Arc::new(AtomicU8::new(0));
        let completed = result.clone();
        // Submission stays under the state lock so clear-data retirement orders
        // every older write before the store's clear operation. Completion only
        // touches an atomic, including stores that complete synchronously.
        if store.save_blocker_statistics(
            profile,
            value,
            Box::new(move |ok| completed.store(if ok { 1 } else { 2 }, Ordering::Release)),
        ) {
            state.pending = Some((state.revision, result));
        }
        state.last_write = std::time::Instant::now();
    }

    fn reset(&mut self) {
        let mut old = self.state.lock().unwrap_or_else(|p| p.into_inner());
        old.retired = true;
        let fresh = Self::new(old.private);
        drop(old);
        {
            let mut state = fresh.state.lock().unwrap_or_else(|p| p.into_inner());
            state.value = Some(BlockerStatistics::default());
            state.dirty = true;
        }
        self.state = fresh.state;
        self.counter.0.store(0, Ordering::Relaxed);
    }

    fn active(&self) -> bool {
        self.counter.0.load(Ordering::Relaxed) != 0
            || self.counter.1.load(Ordering::Relaxed)
            || self.state.lock().unwrap_or_else(|p| p.into_inner()).dirty
    }
}

impl Shell {
    pub(super) fn ensure_blocker_statistics(&mut self, profile: ProfileId, fresh: bool) {
        let Some(profile_info) = self.profiles.get(profile) else {
            return;
        };
        let private = profile_info.kind == ProfileKind::Incognito;
        let statistics = match self.blocker_statistics.entry(profile) {
            std::collections::hash_map::Entry::Occupied(entry) => entry.into_mut(),
            std::collections::hash_map::Entry::Vacant(entry) => {
                let statistics = Statistics::new(private);
                if fresh {
                    statistics
                        .state
                        .lock()
                        .unwrap_or_else(|p| p.into_inner())
                        .value = Some(BlockerStatistics::default());
                }
                self.engine
                    .set_blocker_statistics(profile, statistics.counter.clone());
                entry.insert(statistics)
            }
        };
        statistics.load(profile, &self.store);
    }

    pub(super) fn query_blocker_statistics(&mut self, profile: ProfileId, reply: Reply) {
        if self.windows.focused().is_none_or(|w| w.profile != profile) {
            let _ = reply.try_send(None);
            return;
        }
        self.ensure_blocker_statistics(profile, false);
        let Some(statistics) = self.blocker_statistics.get(&profile).cloned() else {
            let _ = reply.try_send(None);
            return;
        };
        let store = self.store.clone();
        if statistics.counter.1.load(Ordering::Relaxed) {
            self.engine.collect_blocker_statistics(
                profile,
                false,
                Box::new(move || {
                    statistics.reply(profile, &store, reply);
                }),
            );
        } else {
            statistics.reply(profile, &store, reply);
        }
    }

    pub(super) fn maintain_blocker_statistics(&self) {
        for (&profile, statistics) in &self.blocker_statistics {
            if !statistics.active() {
                continue;
            }
            let statistics = statistics.clone();
            let store = self.store.clone();
            if statistics.counter.1.load(Ordering::Relaxed) {
                self.engine.collect_blocker_statistics(
                    profile,
                    false,
                    Box::new(move || {
                        statistics.collect(profile, &store, false);
                    }),
                );
            } else {
                statistics.collect(profile, &store, false);
            }
        }
    }

    pub(super) fn reset_blocker_statistics(&mut self, profile: ProfileId) {
        let Some(statistics) = self.blocker_statistics.get_mut(&profile) else {
            return;
        };
        statistics.reset();
        self.engine
            .collect_blocker_statistics(profile, true, Box::new(|| {}));
    }

    pub(super) fn retire_blocker_statistics(&mut self, profile: ProfileId) {
        if let Some(statistics) = self.blocker_statistics.remove(&profile) {
            statistics
                .state
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .retired = true;
        }
    }

    pub(super) fn flush_blocker_statistics_until(&self, deadline: std::time::Instant) -> bool {
        for (&profile, statistics) in &self.blocker_statistics {
            if !statistics.active() {
                continue;
            }
            // Persist already collected totals before relying on a final UI
            // dispatch. A new-tab query may have drained every native counter.
            statistics.collect(profile, &self.store, true);
            if !statistics.counter.1.load(Ordering::Relaxed) {
                continue;
            }
            let statistics = statistics.clone();
            let store = self.store.clone();
            let (send, receive) = std::sync::mpsc::sync_channel(1);
            self.engine.collect_blocker_statistics(
                profile,
                false,
                Box::new(move || {
                    statistics.collect(profile, &store, true);
                    let _ = send.try_send(());
                }),
            );
            if receive
                .recv_timeout(deadline.saturating_duration_since(std::time::Instant::now()))
                .is_err()
            {
                return false;
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::FakeStore;
    use super::*;
    #[test]
    fn initial_query_waits_for_loaded_totals_without_polling() {
        let store: SharedStore = Arc::new(FakeStore::default());
        let statistics = Statistics::new(false);
        statistics.state.lock().unwrap().loading = true;
        let (reply, receive) = std::sync::mpsc::sync_channel(1);
        statistics.reply(ProfileId::from(4), &store, reply);
        assert!(receive.try_recv().is_err());
        let mut saved = BlockerStatistics::default();
        saved.record(local_day(), 12);
        statistics.loaded(ProfileId::from(4), &store, Some(saved));
        assert_eq!(receive.try_recv().unwrap().unwrap().today, 12);
    }

    #[test]
    fn stored_days_keep_their_numbers() {
        use jiff::civil::date;
        assert_eq!(day_number(date(1, 1, 1)), 1);
        assert_eq!(day_number(date(1970, 1, 1)), 719_163);
        assert_eq!(day_number(date(2024, 2, 29)), 738_945);
        assert_eq!(day_number(date(2026, 11, 17)), 739_937);
    }

    #[test]
    fn flush_is_dirty_only_rate_limited_and_forced_at_shutdown() {
        let fake = Arc::new(FakeStore::default());
        let store: SharedStore = fake.clone();
        let profile = ProfileId::from(1);
        let statistics = Statistics::new(false);
        statistics.load(profile, &store);
        statistics.counter.0.store(4, Ordering::Relaxed);
        assert_eq!(statistics.collect(profile, &store, false).unwrap().today, 4);
        assert_eq!(fake.statistics_writes.load(Ordering::Relaxed), 0);
        statistics.state.lock().unwrap().last_write -= FLUSH_INTERVAL;
        statistics.collect(profile, &store, false);
        statistics.collect(profile, &store, false);
        assert_eq!(fake.statistics_writes.load(Ordering::Relaxed), 1);
        statistics.counter.0.store(3, Ordering::Relaxed);
        statistics.collect(profile, &store, false);
        assert_eq!(fake.statistics_writes.load(Ordering::Relaxed), 1);
        statistics.collect(profile, &store, true);
        assert_eq!(fake.statistics_writes.load(Ordering::Relaxed), 2);
        assert_eq!(fake.statistics.lock().unwrap()[&profile].today(), 7);
    }
    #[test]
    fn clear_resets_pending_counts_and_late_callbacks_cannot_resurrect_them() {
        let fake = Arc::new(FakeStore::default());
        let store: SharedStore = fake.clone();
        let profile = ProfileId::from(3);
        let mut statistics = Statistics::new(false);
        statistics.load(profile, &store);
        statistics.counter.0.store(9, Ordering::Relaxed);
        let late = statistics.clone();
        statistics.reset();
        assert_eq!(statistics.collect(profile, &store, false).unwrap().today, 0);
        statistics.counter.0.store(4, Ordering::Relaxed);
        assert!(late.collect(profile, &store, true).is_none());
        assert_eq!(statistics.collect(profile, &store, true).unwrap().today, 4);
        assert_eq!(fake.statistics.lock().unwrap()[&profile].today(), 4);
    }

    #[test]
    fn shutdown_queues_latest_totals_after_an_unacknowledged_older_write() {
        let fake = Arc::new(FakeStore::default());
        let store: SharedStore = fake.clone();
        let statistics = Statistics::new(false);
        statistics.load(ProfileId::from(5), &store);
        statistics.state.lock().unwrap().pending = Some((0, Arc::new(AtomicU8::new(0))));
        statistics.counter.0.store(8, Ordering::Relaxed);
        statistics.collect(ProfileId::from(5), &store, true);
        assert_eq!(
            fake.statistics.lock().unwrap()[&ProfileId::from(5)].today(),
            8
        );
    }

    #[test]
    fn private_counts_never_reach_store_and_retired_callbacks_are_inert() {
        let fake = Arc::new(FakeStore::default());
        let store: SharedStore = fake.clone();
        let profile = ProfileId::from(2);
        let statistics = Statistics::new(true);
        statistics.counter.0.store(9, Ordering::Relaxed);
        assert_eq!(statistics.collect(profile, &store, true).unwrap().today, 9);
        assert_eq!(fake.statistics_writes.load(Ordering::Relaxed), 0);
        assert!(!statistics.active());
        statistics.state.lock().unwrap().retired = true;
        statistics.counter.0.store(3, Ordering::Relaxed);
        assert!(statistics.collect(profile, &store, true).is_none());
        assert_eq!(statistics.counter.0.load(Ordering::Relaxed), 3);
    }
}
