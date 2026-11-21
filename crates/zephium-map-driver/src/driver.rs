use std::{
    collections::{BTreeMap, HashMap, VecDeque},
    sync::{
        atomic::{AtomicU32, Ordering},
        Arc, Mutex, MutexGuard, PoisonError,
    },
    time::Duration,
};

use tokio::{
    runtime::Handle,
    sync::{mpsc, watch},
    task::{self, JoinError, JoinHandle, JoinSet},
};
use url::Url;
use zephium_core::search::SearchEngine;
use zephium_map::{
    read_query,
    session::{Failure, Id, MAX_CONCURRENT, MAX_HOST_CONCURRENT},
    Config, Effect, Input, MapEvent, QueryKind, Session,
};

use crate::{
    io::{FetchRequest, Io},
    page,
    wire::{self, Change, StartRefusal, Target, Targets},
};

pub const TICK: Duration = Duration::from_millis(250);
pub const CHANGE_BUFFER: usize = 64;
const SLOTS: usize = 2;

pub struct MapRequest {
    pub query: String,
    pub engine: SearchEngine,
    /// Distinct for every live map; the session namespaces its effect ids with it.
    pub session_id: u64,
    /// Opaque; memos are kept apart per profile.
    pub profile: String,
    pub private: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    pub render_unavailable: u32,
    pub anonymous_unavailable: u32,
    pub truncated_results: u32,
    pub over_ceiling: u32,
}

#[derive(Default)]
struct Shared {
    targets: Mutex<[Option<Targets>; SLOTS]>,
    render_unavailable: AtomicU32,
    anonymous_unavailable: AtomicU32,
    truncated_results: AtomicU32,
    over_ceiling: AtomicU32,
}

impl Shared {
    fn targets(&self) -> MutexGuard<'_, [Option<Targets>; SLOTS]> {
        self.targets.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

fn bump(counter: &AtomicU32) {
    let _ = counter.fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| n.checked_add(1));
}

pub struct Driver {
    cancel: watch::Sender<bool>,
    task: JoinHandle<()>,
    shared: Arc<Shared>,
}

impl Driver {
    pub fn start(
        request: MapRequest,
        io: Io,
        runtime: &Handle,
    ) -> Result<(Self, mpsc::Receiver<Change>), StartRefusal> {
        Self::start_buffered(request, io, runtime, CHANGE_BUFFER)
    }

    fn start_buffered(
        request: MapRequest,
        io: Io,
        runtime: &Handle,
        buffer: usize,
    ) -> Result<(Self, mpsc::Receiver<Change>), StartRefusal> {
        let read = read_query(&request.query).map_err(|_| StartRefusal::Invalid)?;
        if !matches!(
            read.kind(),
            QueryKind::Place { .. } | QueryKind::Compare { .. }
        ) {
            return Err(StartRefusal::NotAMap);
        }
        if request.engine == SearchEngine::Custom {
            return Err(StartRefusal::UnsupportedEngine);
        }
        let config = Config {
            session_id: request.session_id,
            engine: request.engine,
            now: io.clock.unix(),
            ..Config::default()
        };
        let (session, effects) =
            Session::start(config, &request.query).map_err(|_| StartRefusal::Invalid)?;
        let (tx, rx) = mpsc::channel(buffer);
        let (cancel, cancelled) = watch::channel(false);
        let shared = Arc::new(Shared::default());
        let run = Run {
            started: io.clock.now(),
            session: Some(session),
            io,
            profile: request.profile,
            private: request.private,
            tx,
            shared: Arc::clone(&shared),
            reads: JoinSet::new(),
            tasks: HashMap::new(),
            hosts: BTreeMap::new(),
            immediate: VecDeque::new(),
            seq: 0,
            cancelled,
            cancelling: false,
            unsent: None,
        };
        let task = runtime.spawn(run.run(effects));
        Ok((
            Self {
                cancel,
                task,
                shared,
            },
            rx,
        ))
    }

    pub fn cancel(&self) {
        self.cancel.send_replace(true);
    }

    /// Resolves against the last place emitted for `slot`; only `https` addresses come back.
    pub fn target(&self, slot: u8, target: &Target) -> Option<Url> {
        self.shared
            .targets()
            .get(usize::from(slot))?
            .as_ref()?
            .resolve(target)
    }

    pub fn counts(&self) -> Counts {
        let load = |counter: &AtomicU32| counter.load(Ordering::Acquire);
        Counts {
            render_unavailable: load(&self.shared.render_unavailable),
            anonymous_unavailable: load(&self.shared.anonymous_unavailable),
            truncated_results: load(&self.shared.truncated_results),
            over_ceiling: load(&self.shared.over_ceiling),
        }
    }
}

impl Drop for Driver {
    fn drop(&mut self) {
        self.task.abort();
    }
}

#[derive(Clone, Copy)]
enum Kind {
    Results,
    Fetch,
}

enum Next {
    Ready,
    Tick,
    Cancel,
    Stop,
}

#[derive(PartialEq)]
enum Applied {
    Continue,
    Cancel,
    Stop,
}

struct Run {
    session: Option<Session>,
    io: Io,
    profile: String,
    private: bool,
    tx: mpsc::Sender<Change>,
    shared: Arc<Shared>,
    reads: JoinSet<Input>,
    tasks: HashMap<task::Id, (Id, Kind)>,
    hosts: BTreeMap<Id, String>,
    immediate: VecDeque<Input>,
    seq: u32,
    started: Duration,
    cancelled: watch::Receiver<bool>,
    cancelling: bool,
    unsent: Option<(usize, Targets)>,
}

fn is_end(effect: &Effect) -> bool {
    matches!(
        effect,
        Effect::Emit {
            event: MapEvent::End { .. },
            ..
        }
    )
}

impl Run {
    async fn run(mut self, mut effects: Vec<Effect>) {
        loop {
            let next = match self.apply(effects).await {
                Applied::Stop => return,
                Applied::Cancel => Next::Cancel,
                Applied::Continue if *self.cancelled.borrow() => Next::Cancel,
                Applied::Continue if self.immediate.is_empty() => self.wait().await,
                Applied::Continue => Next::Ready,
            };
            let input = match next {
                Next::Stop => return,
                Next::Cancel => {
                    self.reads.abort_all();
                    self.tasks.clear();
                    self.hosts.clear();
                    self.immediate.clear();
                    self.cancelling = true;
                    Some(Input::Cancel)
                }
                Next::Tick => None,
                Next::Ready => self.immediate.pop_front(),
            };
            match self.handle(input).await {
                Some(next) => effects = next,
                None => return,
            }
        }
    }

    async fn wait(&mut self) -> Next {
        let done = tokio::select! {
            changed = self.cancelled.changed() => {
                return match changed {
                    Ok(()) if *self.cancelled.borrow() => Next::Cancel,
                    Ok(()) => Next::Tick,
                    Err(_) => Next::Stop,
                };
            }
            () = self.tx.closed() => return Next::Stop,
            Some(done) = self.reads.join_next_with_id() => done,
            () = tokio::time::sleep(TICK) => return Next::Tick,
        };
        match self.completed(done) {
            Some(input) => {
                self.immediate.push_back(input);
                Next::Ready
            }
            None => Next::Tick,
        }
    }

    fn completed(&mut self, done: Result<(task::Id, Input), JoinError>) -> Option<Input> {
        let (task, input) = match done {
            Ok((task, input)) => (task, Some(input)),
            Err(error) => (error.id(), None),
        };
        let (id, kind) = self.tasks.remove(&task)?;
        self.hosts.remove(&id);
        Some(input.unwrap_or(match kind {
            Kind::Results => Input::Results {
                id,
                result: Err(Failure::Unavailable),
            },
            Kind::Fetch => Input::Fetch {
                id,
                result: Err(Failure::Unavailable),
            },
        }))
    }

    fn elapsed(&self) -> Duration {
        self.io.clock.now().saturating_sub(self.started)
    }

    async fn handle(&mut self, input: Option<Input>) -> Option<Vec<Effect>> {
        let mut session = self.session.take()?;
        let time = self.elapsed();
        let cancelling = self.cancelling;
        let done = task::spawn_blocking(move || {
            let mut effects = if cancelling {
                Vec::new()
            } else {
                session.handle(Input::Elapsed { time })
            };
            if let Some(input) = input {
                if !effects.iter().any(is_end) {
                    effects.extend(session.handle(input));
                }
            }
            (session, effects)
        })
        .await;
        match done {
            Ok((session, effects)) => {
                self.session = Some(session);
                Some(effects)
            }
            Err(_) => {
                let end = self.end(wire::Outcome::Failed {
                    failure: wire::Failure::Unavailable,
                });
                let _ = self.tx.send(end).await;
                None
            }
        }
    }

    async fn deliver(&mut self, change: Change) -> Applied {
        if self.cancelling {
            return match self.tx.send(change).await {
                Ok(()) => Applied::Continue,
                Err(_) => Applied::Stop,
            };
        }
        let seq = change.seq();
        tokio::select! {
            biased;
            cancelled = self.cancelled.wait_for(|cancelled| *cancelled) => {
                self.seq = seq;
                self.unsent = None;
                if cancelled.is_ok() { Applied::Cancel } else { Applied::Stop }
            }
            sent = self.tx.send(change) => {
                if sent.is_err() {
                    return Applied::Stop;
                }
                if let Some((index, targets)) = self.unsent.take() {
                    self.shared.targets()[index] = Some(targets);
                }
                Applied::Continue
            }
        }
    }

    async fn apply(&mut self, effects: Vec<Effect>) -> Applied {
        for effect in effects {
            if self.cancelling {
                if let Effect::Emit {
                    event: event @ MapEvent::End { .. },
                    ..
                } = effect
                {
                    if let Some(change) = self.project(event) {
                        self.deliver(change).await;
                    }
                    return Applied::Stop;
                }
                continue;
            }
            if *self.cancelled.borrow() {
                return Applied::Cancel;
            }
            match effect {
                Effect::LoadResults { id, url } => self.load(id, url),
                Effect::Fetch {
                    id,
                    url,
                    expect,
                    max_bytes,
                    timeout,
                } => self.fetch(
                    id,
                    FetchRequest {
                        url,
                        expect,
                        max_bytes,
                        timeout,
                    },
                ),
                Effect::Render { id, .. } => {
                    bump(&self.shared.render_unavailable);
                    self.immediate.push_back(Input::Render {
                        id,
                        result: Err(Failure::Unavailable),
                    });
                }
                Effect::ReadResultsAnonymously { id, .. } => {
                    bump(&self.shared.anonymous_unavailable);
                    self.immediate.push_back(Input::Results {
                        id,
                        result: Err(Failure::Unavailable),
                    });
                }
                Effect::LookupSite { id, site } => {
                    let memo = if self.private {
                        None
                    } else {
                        self.io.memo.lookup(&self.profile, &site)
                    };
                    self.immediate.push_back(Input::Cache { id, memo });
                }
                Effect::RememberSite { site, memo, .. } => {
                    if !self.private {
                        self.io.memo.remember(&self.profile, &site, memo);
                    }
                }
                Effect::Emit { event, .. } => {
                    let end = matches!(event, MapEvent::End { .. });
                    if let Some(change) = self.project(event) {
                        let applied = self.deliver(change).await;
                        if applied != Applied::Continue {
                            return applied;
                        }
                    }
                    if end {
                        return Applied::Stop;
                    }
                }
            }
        }
        if self.cancelling {
            let end = self.end(wire::Outcome::Cancelled);
            self.deliver(end).await;
            return Applied::Stop;
        }
        Applied::Continue
    }

    fn load(&mut self, id: Id, url: Url) {
        let source = Arc::clone(&self.io.results);
        let shared = Arc::clone(&self.shared);
        let read = self.reads.spawn(async move {
            let result = match source.load(url).await {
                Ok(page) => {
                    if page.truncated || page.html.len() > page::MAX_RESULTS_SOURCE_BYTES {
                        bump(&shared.truncated_results);
                    }
                    task::spawn_blocking(move || page::parse(&page))
                        .await
                        .unwrap_or(Err(Failure::Unavailable))
                }
                Err(failure) => Err(failure),
            };
            Input::Results { id, result }
        });
        self.tasks.insert(read.id(), (id, Kind::Results));
    }

    fn fetch(&mut self, id: Id, request: FetchRequest) {
        let host = request.url.host_str().unwrap_or_default().to_owned();
        if self.hosts.len() >= MAX_CONCURRENT
            || self.hosts.values().filter(|h| **h == host).count() >= MAX_HOST_CONCURRENT
        {
            bump(&self.shared.over_ceiling);
            self.immediate.push_back(Input::Fetch {
                id,
                result: Err(Failure::Limit),
            });
            return;
        }
        let fetcher = Arc::clone(&self.io.fetch);
        let timeout = request.timeout;
        let read = self.reads.spawn(async move {
            let result = tokio::time::timeout(timeout, fetcher.fetch(request))
                .await
                .unwrap_or(Err(Failure::Timeout));
            Input::Fetch { id, result }
        });
        self.tasks.insert(read.id(), (id, Kind::Fetch));
        self.hosts.insert(id, host);
    }

    fn end(&mut self, outcome: wire::Outcome) -> Change {
        let seq = self.next_seq();
        Change::End {
            seq,
            outcome,
            elapsed_ms: u32::try_from(self.elapsed().as_millis()).unwrap_or(u32::MAX),
        }
    }

    fn next_seq(&mut self) -> u32 {
        let seq = self.seq;
        self.seq = self.seq.saturating_add(1);
        seq
    }

    fn project(&mut self, event: MapEvent) -> Option<Change> {
        Some(match event {
            MapEvent::FirstLayer { layer } => {
                let (official, social, organic) = wire::first_layer(&layer);
                Change::FirstLayer {
                    seq: self.next_seq(),
                    official,
                    social,
                    organic,
                }
            }
            MapEvent::Place {
                slot,
                place,
                stale,
                confidence,
                complete,
            } => {
                let index = usize::from(slot);
                if index >= SLOTS {
                    return None;
                }
                let (place, targets) = wire::place(&place);
                self.unsent = Some((index, targets));
                Change::Place {
                    seq: self.next_seq(),
                    slot,
                    place,
                    stale,
                    confidence: confidence.into(),
                    complete,
                }
            }
            MapEvent::PlaceFailed { slot, failure } => {
                if usize::from(slot) >= SLOTS {
                    return None;
                }
                Change::PlaceFailed {
                    seq: self.next_seq(),
                    slot,
                    failure: failure.into(),
                }
            }
            MapEvent::Compare { rows } => {
                let rows = {
                    let targets = self.shared.targets();
                    let none = Targets::default();
                    wire::rows(
                        &rows,
                        targets[0].as_ref().unwrap_or(&none),
                        targets[1].as_ref().unwrap_or(&none),
                    )
                };
                Change::Compare {
                    seq: self.next_seq(),
                    rows,
                }
            }
            MapEvent::End { outcome, .. } => self.end(outcome.into()),
            MapEvent::Kind { .. } | MapEvent::Passages { .. } => return None,
        })
    }
}

#[cfg(test)]
mod tests;
