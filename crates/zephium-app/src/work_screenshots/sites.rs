//! Standing screenshot choices and closed revocation state. No image grant is
//! issued by this settings path; native source proof is independently required.
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex, OnceLock},
};
use zephium_agentic::lead::ModelImageGate;
use zephium_core::{
    ids::ProfileId,
    work::{
        port::{WorkReply, WorkRequest},
        screenshots::{validate_origin, WorkScreenshotSiteV1, MAX_SCREENSHOT_SITES},
        WorkError,
    },
};

const MAX_ORIGINS: usize = 4096;
type OriginMap = BTreeMap<(ProfileId, String), Arc<Origin>>;
static ORIGINS: OnceLock<Mutex<OriginMap>> = OnceLock::new();
struct Origin {
    // Starts closed. A stored row is not itself a model-image fence.
    gate: ModelImageGate,
    changed: tokio::sync::Notify,
    state: Mutex<State>,
}
#[derive(Default)]
struct State {
    generation: u64,
    pending: bool,
    interrupted: bool,
    unconfirmed: bool,
}
impl Origin {
    fn new() -> Self {
        Self {
            gate: ModelImageGate::new(None),
            changed: tokio::sync::Notify::new(),
            state: Mutex::new(State::default()),
        }
    }
    fn begin(self: &Arc<Self>) -> Result<Change, WorkError> {
        self.begin_at(None)
    }
    fn begin_at(self: &Arc<Self>, expected: Option<u64>) -> Result<Change, WorkError> {
        let mut state = self.state.lock().map_err(|_| WorkError::Unavailable)?;
        if expected.is_some_and(|generation| state.generation != generation || state.unconfirmed) {
            return Err(WorkError::Conflict);
        }
        if state.pending {
            // Explicit settings changes interrupt an older in-flight choice.
            // Keep its queue ownership until it drains, but never confirm it.
            if expected.is_none() {
                state.interrupted = true;
            }
            return Err(WorkError::Conflict);
        }
        // Revocation precedes queue admission, including for an Allow change.
        self.gate.begin_change().ok_or(WorkError::Capacity)?;
        state.unconfirmed = true;
        state.generation = state.generation.checked_add(1).ok_or(WorkError::Capacity)?;
        state.pending = true;
        state.interrupted = false;
        let change = Change {
            origin: self.clone(),
            generation: state.generation,
        };
        drop(state);
        self.changed.notify_waiters();
        Ok(change)
    }
    fn unconfirmed(&self) -> bool {
        self.state.lock().map_or(true, |state| state.unconfirmed)
    }
}
struct Change {
    origin: Arc<Origin>,
    generation: u64,
}
impl Change {
    fn confirm(&self) -> Result<(), WorkError> {
        let mut state = self
            .origin
            .state
            .lock()
            .map_err(|_| WorkError::Unavailable)?;
        if !state.pending || state.interrupted || state.generation != self.generation {
            return Err(WorkError::Conflict);
        }
        state.pending = false;
        state.unconfirmed = false;
        // Deliberately do not open gate. Native source + consent admission owns
        // any future image capability, not this settings mutation callback.
        Ok(())
    }
}
impl Drop for Change {
    fn drop(&mut self) {
        if let Ok(mut state) = self.origin.state.lock() {
            if state.generation == self.generation {
                state.pending = false;
            }
        }
    }
}
fn origins() -> &'static Mutex<BTreeMap<(ProfileId, String), Arc<Origin>>> {
    ORIGINS.get_or_init(Default::default)
}
fn origin(profile: ProfileId, key: &str) -> Result<Arc<Origin>, WorkError> {
    validate_origin(key)?;
    let mut entries = origins().lock().map_err(|_| WorkError::Unavailable)?;
    if let Some(entry) = entries.get(&(profile, key.to_owned())) {
        return Ok(entry.clone());
    }
    if entries.len() >= MAX_ORIGINS {
        return Err(WorkError::Capacity);
    }
    let entry = Arc::new(Origin::new());
    entries.insert((profile, key.to_owned()), entry.clone());
    Ok(entry)
}
/// Closed origin generation, not a screenshot capability or a model fence.
#[derive(Clone)]
pub(super) struct OriginEpoch {
    origin: Arc<Origin>,
    generation: u64,
}
impl OriginEpoch {
    #[cfg(any(target_os = "macos", windows))]
    pub(super) fn same(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.origin, &other.origin) && self.generation == other.generation
    }
    pub(super) fn current(&self) -> bool {
        self.while_current(&mut || {})
    }
    pub(super) fn while_current(&self, operation: &mut dyn FnMut()) -> bool {
        let Ok(state) = self.origin.state.lock() else {
            return false;
        };
        if state.generation != self.generation || state.pending || state.unconfirmed {
            return false;
        }
        operation();
        true
    }
    #[cfg(any(target_os = "macos", windows))]
    pub(super) async fn changed(&self) {
        loop {
            let notified = self.origin.changed.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if !self.current() {
                return;
            }
            notified.await;
        }
    }
}
#[cfg(any(target_os = "macos", windows, test))]
pub(super) fn origin_epoch(profile: ProfileId, key: &str) -> Result<OriginEpoch, WorkError> {
    let origin = origin(profile, key)?;
    let state = origin.state.lock().map_err(|_| WorkError::Unavailable)?;
    if state.pending || state.unconfirmed {
        return Err(WorkError::Conflict);
    }
    let generation = state.generation;
    drop(state);
    Ok(OriginEpoch { origin, generation })
}
pub(super) async fn read_records(
    handle: &crate::Handle,
    profile: ProfileId,
) -> Result<Vec<WorkScreenshotSiteV1>, WorkError> {
    call(handle, profile, None).await
}
/// An explicit host question may remember only its original origin generation.
/// A concurrent settings change cannot be overwritten by a stale answer.
pub(super) async fn remember_for_question(
    handle: &crate::Handle,
    profile: ProfileId,
    key: String,
    expected: OriginEpoch,
) -> Result<OriginEpoch, WorkError> {
    let origin = origin(profile, &key)?;
    if !Arc::ptr_eq(&origin, &expected.origin) {
        return Err(WorkError::Invalid);
    }
    let change = origin.begin_at(Some(expected.generation))?;
    let records = call(handle, profile, Some((key.clone(), true))).await?;
    if !records.iter().any(|record| record.origin == key) {
        return Err(WorkError::Invalid);
    }
    change.confirm()?;
    let epoch = OriginEpoch {
        origin,
        generation: change.generation,
    };
    epoch.current().then_some(epoch).ok_or(WorkError::Conflict)
}

/// UI-only record; pending/unconfirmed choices are visibly closed.
pub struct SiteRow {
    pub origin: String,
    pub unconfirmed: bool,
}
fn project(
    profile: ProfileId,
    records: Vec<WorkScreenshotSiteV1>,
) -> Result<Vec<SiteRow>, WorkError> {
    let entries = origins().lock().map_err(|_| WorkError::Unavailable)?;
    let mut rows = records
        .into_iter()
        .map(|record| {
            let unconfirmed = entries
                .get(&(profile, record.origin.clone()))
                .is_some_and(|entry| entry.unconfirmed());
            (record.origin, unconfirmed)
        })
        .collect::<BTreeMap<_, _>>();
    // A failed Allow may have no persisted row yet. Keep it visible for retry.
    for ((owner, key), entry) in entries.iter().filter(|((owner, _), _)| *owner == profile) {
        let _ = owner;
        if entry.unconfirmed() {
            rows.insert(key.clone(), true);
        }
    }
    Ok(rows
        .into_iter()
        .map(|(origin, unconfirmed)| SiteRow {
            origin,
            unconfirmed,
        })
        .collect())
}
async fn call(
    handle: &crate::Handle,
    profile: ProfileId,
    set: Option<(String, bool)>,
) -> Result<Vec<WorkScreenshotSiteV1>, WorkError> {
    let submitted = crate::work_runtime::admitted(|| {
        handle.submit_work_document(
            WorkRequest::ScreenshotAccess { set: set.clone() },
            Some(profile),
        )
    })
    .await?;
    let response = tokio::time::timeout(std::time::Duration::from_secs(10), submitted)
        .await
        .map_err(|_| WorkError::Unavailable)??;
    let WorkReply::ScreenshotAccess(records) = response.reply else {
        return Err(WorkError::Invalid);
    };
    if response.profile != profile || records.len() > MAX_SCREENSHOT_SITES {
        return Err(WorkError::Invalid);
    }
    let mut previous: Option<&str> = None;
    for record in &records {
        validate_origin(&record.origin)?;
        if previous.is_some_and(|old| old >= record.origin.as_str()) {
            return Err(WorkError::Invalid);
        }
        previous = Some(&record.origin);
    }
    Ok(records)
}
/// Reads only separately remembered screenshot choices, never account access.
pub async fn standing(
    handle: &crate::Handle,
    profile: ProfileId,
) -> Result<Vec<SiteRow>, WorkError> {
    project(profile, call(handle, profile, None).await?)
}
/// Revokes before queueing; missing or stale Store replies remain closed.
pub async fn set_standing(
    handle: &crate::Handle,
    profile: ProfileId,
    key: String,
    allow: bool,
) -> Result<Vec<SiteRow>, WorkError> {
    let change = origin(profile, &key)?.begin()?;
    let records = call(handle, profile, Some((key.clone(), allow))).await?;
    if records.iter().any(|record| record.origin == key) != allow {
        return Err(WorkError::Invalid);
    }
    change.confirm()?;
    project(profile, records)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pending_always_cannot_confirm_after_explicit_revoke_or_stale_generation() {
        let origin = Arc::new(Origin::new());
        let pending = origin.begin_at(Some(0)).unwrap();
        assert!(origin.begin().is_err());
        assert!(pending.confirm().is_err());
        assert!(origin.unconfirmed());
        drop(pending);
        assert!(origin.begin_at(Some(0)).is_err());
        let retry = origin.begin().unwrap();
        retry.confirm().unwrap();
        assert!(origin.begin_at(Some(0)).is_err());
    }
    #[test]
    fn origin_lookup_and_confirmed_settings_never_mint_an_image_grant() {
        let origin = Arc::new(Origin::new());
        assert!(origin.gate.fence().is_none());
        let change = origin.begin().unwrap();
        assert!(origin.unconfirmed());
        assert!(origin.gate.fence().is_none());
        change.confirm().unwrap();
        assert!(!origin.unconfirmed());
        assert!(origin.gate.fence().is_none());
    }
    #[test]
    fn cancelled_settings_future_stays_unconfirmed_and_stale_reply_cannot_reopen() {
        let origin = Arc::new(Origin::new());
        let first = origin.begin().unwrap();
        assert!(matches!(origin.begin(), Err(WorkError::Conflict)));
        drop(first);
        assert!(origin.unconfirmed());
        let next = origin.begin().unwrap();
        let stale = Change {
            origin: origin.clone(),
            generation: next.generation - 1,
        };
        assert!(stale.confirm().is_err());
        drop(stale);
        assert!(matches!(origin.begin(), Err(WorkError::Conflict)));
        assert!(next.confirm().is_err());
        assert!(origin.unconfirmed());
        drop(next);
        let retry = origin.begin().unwrap();
        retry.confirm().unwrap();
        assert!(!origin.unconfirmed());
        assert!(origin.gate.fence().is_none());
    }
    #[test]
    fn screenshot_origin_revocation_is_exact_and_profile_scoped() {
        let first = origin(ProfileId::from(9981), "https://app.example.test").unwrap();
        let sibling = origin(ProfileId::from(9981), "https://example.test").unwrap();
        let other = origin(ProfileId::from(9982), "https://app.example.test").unwrap();
        drop(first.begin().unwrap());
        assert!(first.unconfirmed());
        assert!(!sibling.unconfirmed());
        assert!(!other.unconfirmed());
        assert!(origin(ProfileId::from(9981), "https://app.example.test/path").is_err());
    }
}
