//! UI-thread-owned pause signal shared by overlapping registration cohorts.

use std::cell::Cell;
use std::cell::RefCell;
use std::rc::Rc;
use std::rc::Weak;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

type ApplyPause = Rc<dyn Fn(bool)>;

#[derive(Clone, Default)]
pub(crate) struct ContentPause(Rc<Inner>);

#[derive(Default)]
struct Inner {
    paused: Arc<AtomicBool>,
    statistics: std::cell::OnceCell<zephium_core::blocker::BlockedLoadCounter>,
    callbacks: RefCell<Vec<ApplyPause>>,
}

pub(crate) struct PauseRegistration {
    owner: Weak<Inner>,
    callback: ApplyPause,
    alive: Rc<Cell<bool>>,
}

impl ContentPause {
    pub(crate) fn set_statistics(&self, counter: zephium_core::blocker::BlockedLoadCounter) {
        let _ = self.0.statistics.set(counter);
    }
    #[cfg_attr(not(any(target_os = "macos", target_os = "windows")), allow(dead_code))]
    pub(crate) fn statistics(&self) -> Option<&zephium_core::blocker::BlockedLoadCounter> {
        self.0.statistics.get()
    }

    #[cfg(any(target_os = "windows", test))]
    pub(crate) fn signal(&self) -> Arc<AtomicBool> {
        self.0.paused.clone()
    }
    pub(crate) fn paused(&self) -> bool {
        self.0.paused.load(Ordering::Relaxed)
    }

    pub(crate) fn set(&self, paused: bool) {
        if self.0.paused.swap(paused, Ordering::Relaxed) == paused {
            return;
        }
        // No RefCell borrow is held through a native mutation/re-entry.
        let callbacks = self.0.callbacks.borrow().clone();
        for callback in callbacks {
            callback(self.paused());
        }
    }

    pub(crate) fn register(&self, callback: impl Fn(bool) + 'static) -> Option<PauseRegistration> {
        let alive = Rc::new(Cell::new(true));
        let active = alive.clone();
        let callback: ApplyPause = Rc::new(move |paused| {
            if active.get() {
                callback(paused);
            }
        });
        {
            let mut callbacks = self.0.callbacks.borrow_mut();
            if callbacks.len() >= 4 {
                return None;
            }
            callbacks.push(callback.clone());
        }
        callback(self.paused());
        Some(PauseRegistration {
            owner: Rc::downgrade(&self.0),
            callback,
            alive,
        })
    }
}

impl Drop for PauseRegistration {
    fn drop(&mut self) {
        self.alive.set(false);
        if let Some(owner) = self.owner.upgrade() {
            owner
                .callbacks
                .borrow_mut()
                .retain(|entry| !Rc::ptr_eq(entry, &self.callback));
        }
    }
}

#[cfg(any(target_os = "windows", test))]
pub(crate) fn record_installed_block(
    counter: Option<&zephium_core::blocker::BlockedLoadCounter>,
    installed: bool,
) {
    if installed {
        if let Some(counter) = counter {
            counter.0.fetch_add(1, Ordering::Relaxed);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn windows_block_delivery_counts_only_success_in_the_owning_profile() {
        let first: zephium_core::blocker::BlockedLoadCounter = Arc::default();
        let second: zephium_core::blocker::BlockedLoadCounter = Arc::default();
        record_installed_block(Some(&first), false);
        record_installed_block(None, true);
        assert_eq!(first.0.load(Ordering::Relaxed), 0);
        record_installed_block(Some(&first), true);
        assert_eq!(first.0.load(Ordering::Relaxed), 1);
        assert_eq!(second.0.load(Ordering::Relaxed), 0);
    }
    #[test]
    fn overlapping_cohorts_share_pause_and_retired_callbacks_disappear() {
        let pause = ContentPause::default();
        let signal = pause.signal();
        let changes = Rc::new(RefCell::new(Vec::new()));
        let first_changes = changes.clone();
        let first = pause
            .register(move |paused| first_changes.borrow_mut().push((1, paused)))
            .unwrap();
        let second_changes = changes.clone();
        let second = pause
            .register(move |paused| second_changes.borrow_mut().push((2, paused)))
            .unwrap();
        let cosmetic_old = pause.register(|_| {}).unwrap();
        let cosmetic_new = pause.register(|_| {}).unwrap();
        assert!(pause.register(|_| {}).is_none());
        drop(cosmetic_old);
        drop(cosmetic_new);
        pause.set(true);
        assert!(signal.load(Ordering::Relaxed));
        drop(first);
        pause.set(false);
        assert!(!signal.load(Ordering::Relaxed));
        assert_eq!(
            *changes.borrow(),
            vec![(1, false), (2, false), (1, true), (2, true), (2, false)]
        );
        drop(second);
        assert!(pause.0.callbacks.borrow().is_empty());
    }
}
