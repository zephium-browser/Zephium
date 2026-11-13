// Copyright 2020-2026 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

//! Bounded admission for native requests that outlive their initiating callback.

use std::{
  collections::HashSet,
  sync::{Arc, Mutex, TryLockError},
};

#[cfg(any(target_os = "macos", target_os = "ios", target_os = "windows", test))]
use std::collections::VecDeque;

/// Custom-protocol handlers are privileged and asynchronous. A modest ceiling
/// leaves ample room for normal asset/IPC concurrency while preventing a
/// compromised chrome renderer from retaining an unbounded number of native
/// tasks, deferrals, response channels, and request bodies.
pub(crate) const CUSTOM_PROTOCOL_IN_FLIGHT_LIMIT: usize = 32;
/// Requests past the ceiling wait their turn rather than fail: a refused
/// module import stays failed until its document reloads, and a lazy view's
/// static graph routinely arrives as one burst larger than the ceiling. The
/// wait list is bounded as well, so only a flood beyond both is refused.
#[cfg(any(target_os = "macos", target_os = "ios", target_os = "windows", test))]
pub(crate) const CUSTOM_PROTOCOL_WAITING_LIMIT: usize = 512;
pub(crate) const CUSTOM_PROTOCOL_OVERFLOW_STATUS: http::StatusCode =
  http::StatusCode::SERVICE_UNAVAILABLE;

#[derive(Debug)]
struct AdmissionState {
  inner: Mutex<AdmissionInner>,
  limit: usize,
}

#[derive(Debug)]
struct AdmissionInner {
  active: HashSet<u64>,
  next_token: u64,
  sealed: bool,
}

/// A cloneable admission scope. Clones share one exact active-request count.
#[derive(Clone, Debug)]
pub(crate) struct InFlightAdmission {
  state: Arc<AdmissionState>,
}

/// The unique ownership proof for one admitted request.
///
/// This type is deliberately neither `Clone` nor manually releasable. Moving
/// it into a native pending-request record makes every response, cancellation,
/// timeout, or teardown path release the slot through exactly one `Drop`.
#[derive(Debug)]
pub(crate) struct InFlightPermit {
  state: Option<Arc<AdmissionState>>,
  token: u64,
}

impl InFlightAdmission {
  pub(crate) fn new(limit: usize) -> Self {
    Self {
      state: Arc::new(AdmissionState {
        inner: Mutex::new(AdmissionInner {
          active: HashSet::new(),
          next_token: 1,
          sealed: false,
        }),
        limit,
      }),
    }
  }

  /// Acquires one slot without blocking. Once accounting becomes unverifiable,
  /// admission remains fail-closed for the lifetime of this scope.
  pub(crate) fn try_acquire(&self) -> Option<InFlightPermit> {
    let mut inner = match self.state.inner.try_lock() {
      Ok(inner) => inner,
      Err(TryLockError::WouldBlock) => return None,
      Err(TryLockError::Poisoned(error)) => {
        // A panic while mutating accounting makes its history unprovable.
        // Recover the guard only to seal and drain; never resume admission.
        let mut inner = error.into_inner();
        inner.sealed = true;
        inner.active.clear();
        return None;
      }
    };
    if inner.sealed || inner.active.len() >= self.state.limit {
      return None;
    }
    let token = inner.next_token;
    let Some(next_token) = token.checked_add(1) else {
      inner.sealed = true;
      inner.active.clear();
      return None;
    };
    inner.next_token = next_token;
    if token == 0 || !inner.active.insert(token) {
      inner.sealed = true;
      inner.active.clear();
      return None;
    }
    Some(InFlightPermit {
      state: Some(self.state.clone()),
      token,
    })
  }

  /// Revokes every outstanding slot and permanently rejects new work. Native
  /// teardown calls this after cancelling its task registry. Late permit drops
  /// are then recognized as teardown revocations rather than underflow.
  pub(crate) fn seal_and_drain(&self) {
    let mut inner = match self.state.inner.lock() {
      Ok(inner) => inner,
      Err(error) => error.into_inner(),
    };
    inner.sealed = true;
    inner.active.clear();
  }

  #[cfg(test)]
  fn active(&self) -> usize {
    self
      .state
      .inner
      .lock()
      .map(|inner| inner.active.len())
      .unwrap_or_default()
  }
}

/// Requests waiting for an in-flight slot, in arrival order within each pool.
/// A pool is one admission scope (one scheme of one webview); a full pool never
/// holds back another, and within a pool nothing overtakes an earlier arrival.
#[cfg(any(target_os = "macos", target_os = "ios", target_os = "windows", test))]
#[derive(Debug)]
pub(crate) struct WaitList<K, T> {
  entries: VecDeque<(K, T)>,
  limit: usize,
}

#[cfg(any(target_os = "macos", target_os = "ios", target_os = "windows", test))]
impl<K: PartialEq, T> WaitList<K, T> {
  pub(crate) const fn new(limit: usize) -> Self {
    Self {
      entries: VecDeque::new(),
      limit,
    }
  }

  pub(crate) fn is_empty(&self) -> bool {
    self.entries.is_empty()
  }

  /// Whether a new arrival in `pool` must queue behind one already waiting.
  pub(crate) fn has_waiting(&self, pool: &K) -> bool {
    self.entries.iter().any(|(key, _)| key == pool)
  }

  /// Queues an arrival; a full list hands it back to be refused.
  pub(crate) fn push(&mut self, pool: K, item: T) -> Result<(), T> {
    if self.entries.len() >= self.limit {
      return Err(item);
    }
    self.entries.push_back((pool, item));
    Ok(())
  }

  /// Removes the earliest waiter whose pool admits it now. Only the head of
  /// each pool is offered a slot, so a pool's order holds.
  pub(crate) fn take_ready<P>(&mut self, mut admit: impl FnMut(&K) -> Option<P>) -> Option<(T, P)> {
    let mut tried: Vec<usize> = Vec::new();
    for index in 0..self.entries.len() {
      let pool = &self.entries[index].0;
      if tried
        .iter()
        .any(|earlier| &self.entries[*earlier].0 == pool)
      {
        continue;
      }
      match admit(pool) {
        Some(permit) => {
          let (_, item) = self.entries.remove(index)?;
          return Some((item, permit));
        }
        None => tried.push(index),
      }
    }
    None
  }

  #[cfg(any(target_os = "macos", target_os = "ios", test))]
  pub(crate) fn retain(&mut self, mut keep: impl FnMut(&K, &T) -> bool) {
    self.entries.retain(|(pool, item)| keep(pool, item));
  }

  /// Takes out every waiter whose pool matches, in arrival order, so the
  /// caller can complete them after releasing its borrow of the list.
  #[cfg(any(target_os = "windows", test))]
  pub(crate) fn take_where(&mut self, mut matches: impl FnMut(&K) -> bool) -> Vec<T> {
    let mut taken = Vec::new();
    let mut kept = VecDeque::with_capacity(self.entries.len());
    for (pool, item) in self.entries.drain(..) {
      if matches(&pool) {
        taken.push(item);
      } else {
        kept.push_back((pool, item));
      }
    }
    self.entries = kept;
    taken
  }
}

impl Drop for InFlightPermit {
  fn drop(&mut self) {
    let Some(state) = self.state.take() else {
      return;
    };
    let mut inner = match state.inner.lock() {
      Ok(inner) => inner,
      Err(error) => {
        let mut inner = error.into_inner();
        inner.sealed = true;
        inner.active.clear();
        return;
      }
    };
    if !inner.active.remove(&self.token) && !inner.sealed {
      // The safe API cannot release a token twice. Treat any unexplained
      // absence as corrupt accounting and fail closed without panicking.
      inner.sealed = true;
      inner.active.clear();
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn exact_cap_plus_one_and_reuse_after_completion() {
    assert_eq!(
      CUSTOM_PROTOCOL_OVERFLOW_STATUS,
      http::StatusCode::SERVICE_UNAVAILABLE
    );
    let admission = InFlightAdmission::new(CUSTOM_PROTOCOL_IN_FLIGHT_LIMIT);
    let mut permits = Vec::new();
    for _ in 0..CUSTOM_PROTOCOL_IN_FLIGHT_LIMIT {
      permits.push(admission.try_acquire().expect("slot within exact cap"));
    }
    assert_eq!(admission.active(), CUSTOM_PROTOCOL_IN_FLIGHT_LIMIT);
    assert!(admission.try_acquire().is_none());

    drop(permits.pop());
    assert_eq!(admission.active(), CUSTOM_PROTOCOL_IN_FLIGHT_LIMIT - 1);
    let reused = admission.try_acquire().expect("released slot is reusable");
    assert_eq!(admission.active(), CUSTOM_PROTOCOL_IN_FLIGHT_LIMIT);

    drop((permits, reused));
    assert_eq!(admission.active(), 0);
  }

  #[test]
  fn cloned_scopes_and_cross_thread_drop_share_exact_accounting() {
    let admission = InFlightAdmission::new(1);
    let cloned = admission.clone();
    let permit = cloned.try_acquire().expect("shared slot");
    assert!(admission.try_acquire().is_none());

    std::thread::spawn(move || drop(permit))
      .join()
      .expect("permit drop thread");
    assert_eq!(admission.active(), 0);
    assert!(admission.try_acquire().is_some());
  }

  #[test]
  fn a_burst_past_the_ceiling_waits_and_completes_in_order() {
    let admission = InFlightAdmission::new(CUSTOM_PROTOCOL_IN_FLIGHT_LIMIT);
    let mut waiting = WaitList::new(CUSTOM_PROTOCOL_WAITING_LIMIT);
    let mut running = VecDeque::new();
    let mut peak = 0;
    for request in 0..68 {
      if waiting.has_waiting(&"assets") {
        waiting.push("assets", request).expect("room to wait");
      } else if let Some(permit) = admission.try_acquire() {
        running.push_back((request, permit));
      } else {
        waiting.push("assets", request).expect("room to wait");
      }
      peak = peak.max(admission.active());
    }
    assert_eq!(running.len(), CUSTOM_PROTOCOL_IN_FLIGHT_LIMIT);
    let mut finished = Vec::new();
    while let Some((request, permit)) = running.pop_front() {
      drop(permit);
      finished.push(request);
      while let Some((next, permit)) = waiting.take_ready(|_| admission.try_acquire()) {
        running.push_back((next, permit));
        peak = peak.max(admission.active());
      }
    }
    assert_eq!(finished, (0..68).collect::<Vec<_>>());
    assert_eq!(peak, CUSTOM_PROTOCOL_IN_FLIGHT_LIMIT);
    assert!(waiting.is_empty());
    assert_eq!(admission.active(), 0);
  }

  #[test]
  fn a_full_pool_never_holds_back_another_and_order_holds_within_one() {
    let assets = InFlightAdmission::new(1);
    let ipc = InFlightAdmission::new(1);
    let held = assets.try_acquire().expect("assets slot");
    let mut waiting = WaitList::new(8);
    waiting.push("assets", 1).unwrap();
    waiting.push("assets", 2).unwrap();
    waiting.push("ipc", 3).unwrap();
    let admit = |pool: &&str| match *pool {
      "assets" => assets.try_acquire(),
      _ => ipc.try_acquire(),
    };
    let (item, _ipc_permit) = waiting.take_ready(admit).expect("ipc is free");
    assert_eq!(item, 3);
    assert!(waiting.take_ready(admit).is_none());
    drop(held);
    let (item, _permit) = waiting.take_ready(admit).expect("assets freed");
    assert_eq!(item, 1);
  }

  #[test]
  fn the_wait_list_is_bounded_and_forgets_cancelled_waiters() {
    let mut waiting = WaitList::new(2);
    waiting.push("assets", 1).unwrap();
    waiting.push("assets", 2).unwrap();
    assert_eq!(waiting.push("assets", 3), Err(3));
    waiting.retain(|_, item| *item != 1);
    assert!(waiting.push("assets", 3).is_ok());
    let admission = InFlightAdmission::new(4);
    let (first, _permit) = waiting.take_ready(|_| admission.try_acquire()).unwrap();
    assert_eq!(first, 2);
  }

  #[test]
  fn a_closing_webview_takes_out_only_its_own_waiters_in_order() {
    let mut waiting = WaitList::new(8);
    waiting.push(("one", "assets"), 1).unwrap();
    waiting.push(("two", "assets"), 2).unwrap();
    waiting.push(("one", "ipc"), 3).unwrap();
    waiting.push(("two", "ipc"), 4).unwrap();
    assert_eq!(waiting.take_where(|(webview, _)| *webview == "one"), [1, 3]);
    assert!(!waiting.has_waiting(&("one", "assets")));
    let admission = InFlightAdmission::new(4);
    let (next, _permit) = waiting.take_ready(|_| admission.try_acquire()).unwrap();
    assert_eq!(next, 2);
  }

  #[test]
  fn teardown_drain_revokes_late_permits_without_underflow_or_reopening() {
    let admission = InFlightAdmission::new(2);
    let first = admission.try_acquire().expect("first slot");
    let second = admission.try_acquire().expect("second slot");
    admission.seal_and_drain();
    assert_eq!(admission.active(), 0);
    assert!(admission.try_acquire().is_none());

    drop((first, second));
    assert_eq!(admission.active(), 0);
    assert!(admission.try_acquire().is_none());
  }
}
