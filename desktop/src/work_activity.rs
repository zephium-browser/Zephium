//! Bounded observers of original attempts. No resource, task or execution owner.
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use zephium_app::work_runtime::{WorkAttemptObserver, WorkPageFrame, WorkPageFrames};
use zephium_core::ids::ProfileId;
use zephium_core::work::{runtime::*, WorkAttemptId, WorkExecutionId, WorkId, WorkStepId};
use zephium_ipc::work::{WorkChangedV1, WorkPageFrameV1, WorkPageV1, WorkSignalV1};
use zephium_store::{WorkFrameRecord, WorkFrameStore};

const MAX_PAGE_ATTEMPTS: usize = 8;
const MAX_STORED_WORKS: usize = 8;
const MAX_ATTEMPT_PAGES: usize = 16;
const MAX_FRAME_RETRIES: usize = MAX_PAGE_ATTEMPTS * MAX_ATTEMPT_PAGES;
const FRAME_RETRY_DELAY: Duration = Duration::from_secs(5);
type FrameKey = (WorkAttemptId, WorkStepId, u64);
type PageChanged = Arc<dyn Fn(WorkChangedV1) + Send + Sync>;

/// Data-only invalidation: neither the receiver nor its callback owns a run.
#[cfg(any(target_os = "macos", target_os = "windows"))]
struct PageProjectionWake {
    changed: PageChanged,
    scope: WorkChangedV1,
}
#[cfg(any(target_os = "macos", target_os = "windows"))]
impl std::task::Wake for PageProjectionWake {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }
    fn wake_by_ref(self: &Arc<Self>) {
        (self.changed)(self.scope.clone());
    }
}

pub(crate) struct WorkActivity {
    observers: Mutex<BTreeMap<WorkAttemptId, WorkAttemptObserver>>,
    /// At most eight data-only mailboxes preserve final pictures between UI
    /// reads, including when an attempt ends before the next read arrives.
    page_observers: Mutex<BTreeMap<WorkAttemptId, (WorkExecutionId, WorkPageFrames)>>,
    /// Pages of recent attempts, kept after the attempt ends so their last
    /// frame outlives the run for this app session.
    pages: Mutex<BTreeMap<WorkAttemptId, (WorkExecutionId, Vec<WorkPageFrame>)>>,
    frames: Option<Arc<WorkFrameStore>>,
    /// Frames already written this session, by attempt, step and generation.
    persisted: Mutex<BTreeSet<FrameKey>>,
    /// Failed writes retry on a later activity read, without a background timer.
    retry_after: Mutex<BTreeMap<FrameKey, Instant>>,
    /// Stored frames of recently read works, listed once per session.
    stored: Mutex<BTreeMap<WorkId, (ProfileId, Vec<WorkFrameRecord>)>>,
    page_changed: Mutex<Option<PageChanged>>,
}
impl WorkActivity {
    pub(crate) fn new(frames: Option<Arc<WorkFrameStore>>) -> Self {
        Self {
            observers: Mutex::default(),
            page_observers: Mutex::default(),
            pages: Mutex::default(),
            frames,
            persisted: Mutex::default(),
            retry_after: Mutex::default(),
            stored: Mutex::default(),
            page_changed: Mutex::default(),
        }
    }
    pub(crate) fn set_page_observer(&self, changed: PageChanged) {
        if let Ok(mut observer) = self.page_changed.lock() {
            *observer = Some(changed);
        }
    }
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    pub(crate) fn track(&self, observer: WorkAttemptObserver) {
        let mut evicted = None;
        let frames = observer.retain_pages();
        if let Some(frames) = &frames {
            let changed = self
                .page_changed
                .lock()
                .ok()
                .and_then(|value| value.clone());
            if let Some(changed) = changed {
                let (profile, work) = frames.scope();
                frames.register_projection_waker(std::task::Waker::from(Arc::new(
                    PageProjectionWake {
                        changed,
                        scope: WorkChangedV1 {
                            profile: profile.to_string(),
                            work,
                        },
                    },
                )));
            }
        }
        if let (Some(frames), Ok(mut pages)) = (frames, self.page_observers.lock()) {
            if pages.len() >= MAX_PAGE_ATTEMPTS && !pages.contains_key(&observer.attempt()) {
                let oldest = pages
                    .iter()
                    .find(|(_, (_, frames))| !frames.is_alive())
                    .map(|(id, _)| *id)
                    .or_else(|| pages.keys().next().copied());
                if let Some(oldest) = oldest {
                    evicted = pages.remove(&oldest).map(|entry| (oldest, entry));
                }
            }
            pages.insert(observer.attempt(), (observer.execution(), frames));
        }
        if let Some((attempt, (execution, mailbox))) = evicted {
            // The UI can stay hidden across several completed attempts. Save
            // an evicted mailbox's newest pixels before releasing its bound.
            let (profile, work) = mailbox.scope();
            self.persist_evicted(profile, work, attempt, execution, mailbox.pages());
        }
        let Ok(mut observers) = self.observers.lock() else {
            return;
        };
        observers.retain(|_, observer| observer.is_alive());
        if observers.len() < MAX_WORK_ATTEMPTS {
            observers.entry(observer.attempt()).or_insert(observer);
        }
    }
    #[cfg(any(test, target_os = "macos", target_os = "windows"))]
    fn persist_evicted(
        &self,
        profile: ProfileId,
        work: WorkId,
        attempt: WorkAttemptId,
        execution: WorkExecutionId,
        pages: Vec<WorkPageFrame>,
    ) {
        let Some(frames) = &self.frames else {
            return;
        };
        for page in pages {
            let Some(frame) = page.frame else {
                continue;
            };
            let record = WorkFrameRecord {
                work,
                execution,
                attempt,
                step: page.step,
                url: page.url,
                generation: u32::try_from(frame.generation).unwrap_or(u32::MAX),
                width: frame.width,
                height: frame.height,
            };
            if frames.put(profile, &record, &frame.png).is_ok() {
                if let Ok(mut stored) = self.stored.lock() {
                    if let Some((_, records)) = stored.get_mut(&work) {
                        records.retain(|r| !(r.attempt == attempt && r.step == record.step));
                        records.push(record);
                    }
                }
            }
        }
    }
    pub(crate) fn read(&self, state: &WorkRuntimeProjection) -> Vec<WorkSignalV1> {
        let Ok(mut observers) = self.observers.lock() else {
            return Vec::new();
        };
        observers.retain(|_, observer| observer.is_alive());
        observers
            .values()
            .filter_map(WorkAttemptObserver::latest)
            .filter(|signal| current(signal, state))
            .collect()
    }
    pub(crate) fn read_pages(&self, state: &WorkRuntimeProjection) -> Vec<WorkPageV1> {
        let (Ok(mut observers), Ok(mut pages)) = (self.page_observers.lock(), self.pages.lock())
        else {
            return Vec::new();
        };
        for (attempt, (execution, observer)) in observers.iter() {
            if observer.scope() != (state.work.profile, state.work.id) {
                continue;
            }
            let mut opened = observer.pages();
            if opened.is_empty() {
                continue;
            }
            // A page a long run let go keeps its last frame until it is stored.
            if let (Some((_, known)), Ok(persisted)) = (pages.get(attempt), self.persisted.lock()) {
                let dropped: Vec<WorkPageFrame> = known
                    .iter()
                    .filter(|page| {
                        !opened.iter().any(|open| open.step == page.step)
                            && page.frame.as_ref().is_some_and(|frame| {
                                !persisted.contains(&(*attempt, page.step, frame.generation))
                            })
                    })
                    .take(MAX_ATTEMPT_PAGES.saturating_sub(opened.len()))
                    .cloned()
                    .map(|page| WorkPageFrame {
                        live: false,
                        ..page
                    })
                    .collect();
                opened.extend(dropped);
            }
            if pages.len() >= MAX_PAGE_ATTEMPTS && !pages.contains_key(attempt) {
                let oldest = pages.keys().next().copied();
                if let Some(oldest) = oldest {
                    pages.remove(&oldest);
                }
            }
            pages.insert(*attempt, (*execution, opened));
        }
        // Consume the final update before releasing a completed mailbox.
        observers.retain(|_, (_, observer)| {
            observer.is_alive() || observer.scope() != (state.work.profile, state.work.id)
        });
        let executions: Vec<_> = state.executions.iter().map(|e| e.id).collect();
        pages.retain(|_, (execution, _)| executions.contains(execution));
        let profile = state.work.profile;
        let work = state.work.id;
        let mut live: Vec<WorkPageV1> = pages
            .iter()
            .flat_map(|(attempt, (execution, opened))| {
                opened.iter().map(|page| WorkPageV1 {
                    execution: *execution,
                    attempt: *attempt,
                    step: page.step,
                    url: page.url.clone(),
                    live: page.live,
                    frame: page.frame.as_ref().map(|frame| WorkPageFrameV1 {
                        generation: u32::try_from(frame.generation).unwrap_or(u32::MAX),
                        width: frame.width,
                        height: frame.height,
                    }),
                    part: None,
                    title: None,
                })
            })
            .collect();
        self.persist(profile, work, &pages);
        let mut stored = self.stored_records(profile, work);
        stored.retain(|record| {
            executions.contains(&record.execution)
                && !live
                    .iter()
                    .any(|page| page.attempt == record.attempt && page.step == record.step)
        });
        live.extend(stored.into_iter().map(|record| WorkPageV1 {
            execution: record.execution,
            attempt: record.attempt,
            step: record.step,
            url: record.url,
            live: false,
            frame: Some(WorkPageFrameV1 {
                generation: record.generation,
                width: record.width,
                height: record.height,
            }),
            part: None,
            title: None,
        }));
        owned(state, &mut live);
        live
    }
    /// Writes the last frame of every settled page once, then remembers it.
    fn persist(
        &self,
        profile: ProfileId,
        work: WorkId,
        pages: &BTreeMap<WorkAttemptId, (WorkExecutionId, Vec<WorkPageFrame>)>,
    ) {
        self.persist_at(profile, work, pages, Instant::now());
    }
    fn persist_at(
        &self,
        profile: ProfileId,
        work: WorkId,
        pages: &BTreeMap<WorkAttemptId, (WorkExecutionId, Vec<WorkPageFrame>)>,
        now: Instant,
    ) {
        let Some(frames) = &self.frames else {
            return;
        };
        let Ok(mut retry_after) = self.retry_after.lock() else {
            return;
        };
        retry_after.retain(|(attempt, step, generation), _| {
            pages.get(attempt).is_some_and(|(_, opened)| {
                opened.iter().any(|page| {
                    page.step == *step
                        && page
                            .frame
                            .as_ref()
                            .is_some_and(|frame| frame.generation == *generation)
                })
            })
        });
        let Ok(mut persisted) = self.persisted.lock() else {
            return;
        };
        persisted.retain(|(attempt, step, generation)| {
            pages.get(attempt).is_some_and(|(_, opened)| {
                opened.iter().any(|page| {
                    page.step == *step
                        && page
                            .frame
                            .as_ref()
                            .is_some_and(|frame| frame.generation == *generation)
                })
            })
        });
        for (attempt, (execution, opened)) in pages {
            for page in opened.iter().filter(|page| !page.live) {
                let Some(frame) = &page.frame else {
                    continue;
                };
                let key = (*attempt, page.step, frame.generation);
                if persisted.contains(&key) {
                    continue;
                }
                if retry_after.get(&key).is_some_and(|retry| now < *retry) {
                    continue;
                }
                if !retry_after.contains_key(&key) && retry_after.len() >= MAX_FRAME_RETRIES {
                    continue;
                }
                let record = WorkFrameRecord {
                    work,
                    execution: *execution,
                    attempt: *attempt,
                    step: page.step,
                    url: page.url.clone(),
                    generation: u32::try_from(frame.generation).unwrap_or(u32::MAX),
                    width: frame.width,
                    height: frame.height,
                };
                if frames.put(profile, &record, &frame.png).is_ok() {
                    persisted.insert(key);
                    retry_after.remove(&key);
                    if let Ok(mut stored) = self.stored.lock() {
                        if let Some((_, records)) = stored.get_mut(&work) {
                            records.retain(|r| {
                                !(r.attempt == record.attempt && r.step == record.step)
                            });
                            records.push(record);
                        }
                    }
                } else {
                    retry_after.insert(key, now + FRAME_RETRY_DELAY);
                }
            }
        }
    }
    fn stored_records(&self, profile: ProfileId, work: WorkId) -> Vec<WorkFrameRecord> {
        let Some(frames) = &self.frames else {
            return Vec::new();
        };
        let Ok(mut stored) = self.stored.lock() else {
            return Vec::new();
        };
        if let Some((_, records)) = stored.get(&work) {
            return records.clone();
        }
        if stored.len() >= MAX_STORED_WORKS {
            let oldest = stored.keys().next().copied();
            if let Some(oldest) = oldest {
                stored.remove(&oldest);
            }
        }
        let records = frames.list(profile, work);
        stored.insert(work, (profile, records.clone()));
        records
    }
    pub(crate) fn frame(&self, attempt: WorkAttemptId, step: WorkStepId) -> Option<Arc<Vec<u8>>> {
        if let Some(png) = self.pages.lock().ok().and_then(|pages| {
            pages
                .get(&attempt)?
                .1
                .iter()
                .find(|page| page.step == step)?
                .frame
                .as_ref()
                .map(|frame| frame.png.clone())
        }) {
            return Some(png);
        }
        let frames = self.frames.as_ref()?;
        let stored = self.stored.lock().ok()?;
        let (profile, _) = stored.values().find(|(_, records)| {
            records
                .iter()
                .any(|record| record.attempt == attempt && record.step == step)
        })?;
        frames.read(*profile, attempt, step).map(Arc::new)
    }
}

/// Each page's reader and title, from its step: the part it works for, or
/// none for the lead's own read.
fn owned(state: &WorkRuntimeProjection, pages: &mut [WorkPageV1]) {
    for page in pages {
        let Some(step) = state
            .executions
            .iter()
            .find(|execution| execution.id == page.execution)
            .and_then(|execution| execution.steps.iter().find(|step| step.id == page.step))
        else {
            continue;
        };
        page.part = step.part;
        page.title = step
            .local
            .as_ref()
            .and_then(|local| local.page_title.clone())
            .filter(|_| step.account.is_none());
    }
}

fn current(signal: &WorkSignalV1, state: &WorkRuntimeProjection) -> bool {
    signal.version == 1
        && signal.profile == state.work.profile.to_string()
        && signal.work == state.work.id
        && signal.basis_revision == state.work.revision
        && !state.interrupted.contains(&signal.execution)
        && state
            .owners
            .iter()
            .any(|entry| entry.execution == signal.execution && entry.owner == signal.owner)
        && state.executions.iter().any(|execution| {
            execution.id == signal.execution
                && matches!(
                    execution.status,
                    WorkExecutionStatus::Running | WorkExecutionStatus::CancelRequested
                )
                && execution.attempts.iter().any(|attempt| {
                    attempt.id == signal.attempt
                        && attempt.node == signal.node
                        && attempt.status == WorkAttemptStatus::Running
                })
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn evicted_mailbox_preserves_newest_frame_in_its_original_scope() {
        let temp = tempfile::tempdir().unwrap();
        let frames = Arc::new(WorkFrameStore::new(temp.path().join("frames")));
        let activity = WorkActivity::new(Some(frames.clone()));
        let profile = ProfileId::generate();
        let work = WorkId::generate();
        let attempt = WorkAttemptId::generate();
        let execution = WorkExecutionId::generate();
        let step = WorkStepId::generate();
        assert!(activity.stored_records(profile, work).is_empty());
        for generation in [1, 2] {
            activity.persist_evicted(
                profile,
                work,
                attempt,
                execution,
                vec![WorkPageFrame {
                    step,
                    url: "https://fixture.invalid/final".into(),
                    live: false,
                    frame: Some(Arc::new(zephium_agentic::WorkBrowserFrame {
                        generation,
                        width: 4,
                        height: 2,
                        png: Arc::new(vec![generation as u8]),
                    })),
                }],
            );
        }
        let stored = activity.stored_records(profile, work);
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].generation, 2);
        assert_eq!(stored[0].execution, execution);
        assert_eq!(frames.read(profile, attempt, step), Some(vec![2]));
        assert!(frames.read(ProfileId::generate(), attempt, step).is_none());
        assert!(frames.list(profile, WorkId::generate()).is_empty());
        assert_eq!(activity.frame(attempt, step).unwrap().as_slice(), &[2]);
    }

    #[test]
    fn failed_frame_write_keeps_live_pixels_and_retries_only_after_backoff() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("frames");
        std::fs::write(&root, b"temporary unavailable directory").unwrap();
        let frames = Arc::new(WorkFrameStore::new(root.clone()));
        let activity = WorkActivity::new(Some(frames.clone()));
        let profile = ProfileId::generate();
        let work = WorkId::generate();
        let attempt = WorkAttemptId::generate();
        let step = WorkStepId::generate();
        let pages = BTreeMap::from([(
            attempt,
            (
                WorkExecutionId::generate(),
                vec![WorkPageFrame {
                    step,
                    url: "https://fixture.invalid/".into(),
                    live: false,
                    frame: Some(Arc::new(zephium_agentic::WorkBrowserFrame {
                        generation: 1,
                        width: 4,
                        height: 2,
                        png: Arc::new(vec![1, 2, 3]),
                    })),
                }],
            ),
        )]);
        *activity.pages.lock().unwrap() = pages.clone();
        let now = Instant::now();
        activity.persist_at(profile, work, &pages, now);
        assert!(activity.persisted.lock().unwrap().is_empty());
        assert_eq!(
            activity.frame(attempt, step).unwrap().as_slice(),
            &[1, 2, 3]
        );
        std::fs::remove_file(&root).unwrap();
        activity.persist_at(profile, work, &pages, now + Duration::from_secs(1));
        assert!(frames.read(profile, attempt, step).is_none());
        activity.persist_at(profile, work, &pages, now + FRAME_RETRY_DELAY);
        assert_eq!(frames.read(profile, attempt, step), Some(vec![1, 2, 3]));
        assert!(activity
            .persisted
            .lock()
            .unwrap()
            .contains(&(attempt, step, 1)));
        assert!(activity.retry_after.lock().unwrap().is_empty());
        std::fs::remove_dir_all(root).unwrap();
        activity.persist_at(profile, work, &pages, now + FRAME_RETRY_DELAY * 2);
        assert!(frames.read(profile, attempt, step).is_none());
    }
}
