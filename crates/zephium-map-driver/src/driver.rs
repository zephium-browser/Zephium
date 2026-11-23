use std::{
    collections::{BTreeMap, HashMap, VecDeque},
    sync::{
        atomic::{AtomicU32, AtomicU64, Ordering},
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
    query::Confidence,
    read_query,
    session::{Failure, HeldSite, Id, ReadState, Sections, MAX_CONCURRENT, MAX_HOST_CONCURRENT},
    Config, Effect, Input, MapEvent, QueryKind, Session,
};

use crate::{
    io::{FetchRequest, Io},
    page,
    wire::{self, Change, OpenRefusal, OpenResult, StartRefusal, Target, Targets},
};

pub const TICK: Duration = Duration::from_millis(250);
pub const CHANGE_BUFFER: usize = 64;
pub const MAX_WAITING_OPENS: usize = 4;
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
    pub fetches: u32,
    pub bytes: u64,
    pub render_unavailable: u32,
    pub anonymous_unavailable: u32,
    pub truncated_results: u32,
    pub over_ceiling: u32,
}

#[derive(Default)]
struct Shared {
    published: Mutex<Published>,
    fetches: AtomicU32,
    bytes: AtomicU64,
    render_unavailable: AtomicU32,
    anonymous_unavailable: AtomicU32,
    truncated_results: AtomicU32,
    over_ceiling: AtomicU32,
}

impl Shared {
    fn published(&self) -> MutexGuard<'_, Published> {
        self.published
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }
}

fn bump(counter: &AtomicU32) {
    let _ = counter.fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| n.checked_add(1));
}

#[derive(Default)]
struct Published {
    slots: [Option<PublishedPlace>; SLOTS],
    also: Vec<Url>,
}

struct PublishedPlace {
    targets: Targets,
    held: Option<Arc<HeldSite>>,
    confidence: Confidence,
    stale: bool,
}

struct OpenRequest {
    slot: u8,
    id: String,
}

enum Publication {
    Place(usize, Box<PublishedPlace>),
    PlaceFailed(usize),
    Also(Vec<Url>),
}

pub struct Driver {
    cancel: watch::Sender<bool>,
    task: JoinHandle<()>,
    shared: Arc<Shared>,
    opens: mpsc::Sender<OpenRequest>,
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
            sections: Sections::Asked,
            ..Config::default()
        };
        let (session, effects) =
            Session::start(config, &request.query).map_err(|_| StartRefusal::Invalid)?;
        let (tx, rx) = mpsc::channel(buffer);
        let (cancel, cancelled) = watch::channel(false);
        let (opens, open_requests) = mpsc::channel(MAX_WAITING_OPENS);
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
            open_requests,
            opening: None,
            session_id: request.session_id,
        };
        let task = runtime.spawn(run.run(effects));
        Ok((
            Self {
                cancel,
                task,
                shared,
                opens,
            },
            rx,
        ))
    }

    pub fn cancel(&self) {
        let _published = self.shared.published();
        self.cancel.send_replace(true);
    }

    pub fn open(&self, slot: u8, id: &str) -> Result<(), OpenRefusal> {
        let published = self.shared.published();
        if *self.cancel.borrow() || self.opens.is_closed() {
            return Err(OpenRefusal::Cancelled);
        }
        let place = published
            .slots
            .get(usize::from(slot))
            .and_then(Option::as_ref)
            .ok_or(OpenRefusal::NoPlace)?;
        if !place.targets.has(id) {
            return Err(OpenRefusal::UnknownEntry);
        }
        place
            .held
            .as_ref()
            .ok_or(OpenRefusal::NotOpenable)?
            .check_open(id)?;
        self.opens
            .try_send(OpenRequest {
                slot,
                id: id.to_owned(),
            })
            .map_err(|error| match error {
                mpsc::error::TrySendError::Full(_) => OpenRefusal::TooManyWaiting,
                mpsc::error::TrySendError::Closed(_) => OpenRefusal::Cancelled,
            })
    }

    pub fn target(&self, slot: u8, target: &Target) -> Option<Url> {
        let published = self.shared.published();
        if let Target::Also { index } = target {
            return wire::also_target(&published.also, *index);
        }
        published
            .slots
            .get(usize::from(slot))?
            .as_ref()?
            .targets
            .resolve(target)
    }

    pub fn counts(&self) -> Counts {
        let load = |counter: &AtomicU32| counter.load(Ordering::Acquire);
        Counts {
            fetches: load(&self.shared.fetches),
            bytes: self.shared.bytes.load(Ordering::Acquire),
            render_unavailable: load(&self.shared.render_unavailable),
            anonymous_unavailable: load(&self.shared.anonymous_unavailable),
            truncated_results: load(&self.shared.truncated_results),
            over_ceiling: load(&self.shared.over_ceiling),
        }
    }
}

impl Drop for Driver {
    fn drop(&mut self) {
        let _published = self.shared.published();
        self.task.abort();
        self.cancel.send_replace(true);
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
    unsent: Option<Publication>,
    open_requests: mpsc::Receiver<OpenRequest>,
    opening: Option<OpenRequest>,
    session_id: u64,
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
                Applied::Continue if self.session.is_none() => {
                    let Some(next) = self.next_open().await else {
                        return;
                    };
                    effects = next;
                    continue;
                }
                Applied::Cancel => Next::Cancel,
                Applied::Continue if *self.cancelled.borrow() => Next::Cancel,
                Applied::Continue if self.immediate.is_empty() => self.wait().await,
                Applied::Continue => Next::Ready,
            };
            let input = match next {
                Next::Stop => return,
                Next::Cancel => {
                    self.clear_reads();
                    if self.opening.is_some() {
                        return;
                    }
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

    fn clear_reads(&mut self) {
        self.reads = JoinSet::new();
        self.tasks.clear();
        self.hosts.clear();
        self.immediate.clear();
    }

    async fn next_open(&mut self) -> Option<Vec<Effect>> {
        loop {
            let request = tokio::select! {
                biased;
                _ = self.cancelled.wait_for(|cancelled| *cancelled) => return None,
                () = self.tx.closed() => return None,
                request = self.open_requests.recv() => request?,
            };
            let slot = request.slot;
            let place = {
                let published = self.shared.published();
                published.slots[usize::from(slot)]
                    .as_ref()
                    .and_then(|item| {
                        Some((item.held.as_ref()?.clone(), item.confidence, item.stale))
                    })
            };
            if let Some((held, confidence, stale)) = place {
                let cfg = Config {
                    session_id: self.session_id,
                    now: self.io.clock.unix(),
                    ..Config::default()
                };
                if let Ok((session, effects)) =
                    Session::open(cfg, (*held).clone(), slot, &request.id, confidence, stale)
                {
                    self.session = Some(session);
                    self.opening = Some(request);
                    self.started = self.io.clock.now();
                    return Some(effects);
                }
                let (place, _) = wire::place(held.place(), Some(held.robots()));
                let change = Change::Place {
                    seq: self.next_seq(),
                    slot,
                    place,
                    stale,
                    confidence: confidence.into(),
                    complete: true,
                };
                if self.deliver(change).await != Applied::Continue {
                    return None;
                }
            }
            let result = self.open_result(&request);
            let change = Change::Opened {
                seq: self.next_seq(),
                slot,
                id: request.id,
                result,
            };
            if self.deliver(change).await != Applied::Continue {
                return None;
            }
        }
    }

    fn open_result(&self, request: &OpenRequest) -> OpenResult {
        let published = self.shared.published();
        let entry = published.slots[usize::from(request.slot)]
            .as_ref()
            .and_then(|slot| slot.held.as_ref())
            .and_then(|held| {
                held.place()
                    .sections
                    .iter()
                    .find(|entry| entry.id.as_str() == request.id)
            });
        match entry.and_then(|entry| entry.read) {
            Some(ReadState::Read) => OpenResult::Read,
            Some(ReadState::Refused { failure }) => OpenResult::Refused {
                failure: failure.into(),
            },
            _ => OpenResult::Refused {
                failure: if entry.is_some() {
                    wire::Failure::Blocked
                } else {
                    wire::Failure::Missing
                },
            },
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
        let shared = Arc::clone(&self.shared);
        let done = task::spawn_blocking(move || {
            let mut effects = if cancelling {
                Vec::new()
            } else {
                session.handle(Input::Elapsed { time })
            };
            if let Some(input) = input {
                if !effects.iter().any(is_end) {
                    if let Input::Fetch {
                        result: Ok(answer), ..
                    } = &input
                    {
                        let size = answer.bytes.len() as u64;
                        let _ =
                            shared
                                .bytes
                                .fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| {
                                    Some(n.saturating_add(size))
                                });
                    }
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
                if self.opening.is_none() {
                    let end = self.end(wire::Outcome::Failed {
                        failure: wire::Failure::Unavailable,
                    });
                    self.deliver(end).await;
                }
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
        if seq == u32::MAX {
            return Applied::Stop;
        }
        tokio::select! {
            biased;
            cancelled = async { self.cancelled.wait_for(|cancelled| *cancelled).await.is_ok() } => {
                self.seq = seq;
                self.unsent = None;
                if cancelled { Applied::Cancel } else { Applied::Stop }
            }
            permit = self.tx.reserve() => {
                let Ok(permit) = permit else { return Applied::Stop };
                let mut published = self.shared.published();
                if *self.cancelled.borrow() {
                    self.seq = seq;
                    self.unsent = None;
                    return Applied::Cancel;
                }
                match self.unsent.take() {
                    Some(Publication::Place(index, place)) => published.slots[index] = Some(*place),
                    Some(Publication::PlaceFailed(index)) => published.slots[index] = None,
                    Some(Publication::Also(targets)) => published.also = targets,
                    None => {}
                }
                permit.send(change);
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
                    if matches!(event, MapEvent::End { .. }) {
                        if let Some(request) = self.opening.take() {
                            let result = self.open_result(&request);
                            let change = Change::Opened {
                                seq: self.next_seq(),
                                slot: request.slot,
                                id: request.id,
                                result,
                            };
                            if self.deliver(change).await != Applied::Continue {
                                return Applied::Stop;
                            }
                        } else if let Some(change) = self.project(event) {
                            let applied = self.deliver(change).await;
                            if applied != Applied::Continue {
                                return applied;
                            }
                        }
                        self.clear_reads();
                        self.session = None;
                        return Applied::Continue;
                    }
                    if let Some(change) = self.project(event) {
                        let applied = self.deliver(change).await;
                        if applied != Applied::Continue {
                            return applied;
                        }
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
        bump(&self.shared.fetches);
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
                let places = self
                    .session
                    .as_ref()
                    .map(|session| session.result_places().collect::<Vec<_>>())
                    .unwrap_or_default();
                let wire::Layer {
                    official,
                    social,
                    organic,
                    also,
                    targets,
                } = wire::first_layer(&layer, &places);
                self.unsent = Some(Publication::Also(targets));
                Change::FirstLayer {
                    seq: self.next_seq(),
                    official,
                    social,
                    organic,
                    also,
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
                let held = self
                    .session
                    .as_ref()
                    .and_then(|session| session.hold_place(slot, place.clone()));
                let (place, targets) = wire::place(&place, held.as_ref().map(HeldSite::robots));
                self.unsent = Some(Publication::Place(
                    index,
                    Box::new(PublishedPlace {
                        targets,
                        held: held.map(Arc::new),
                        confidence,
                        stale,
                    }),
                ));
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
                self.unsent = Some(Publication::PlaceFailed(usize::from(slot)));
                Change::PlaceFailed {
                    seq: self.next_seq(),
                    slot,
                    failure: failure.into(),
                }
            }
            MapEvent::Compare { rows } => {
                let rows = {
                    let published = self.shared.published();
                    let none = Targets::default();
                    wire::rows(
                        &rows,
                        published.slots[0]
                            .as_ref()
                            .map_or(&none, |slot| &slot.targets),
                        published.slots[1]
                            .as_ref()
                            .map_or(&none, |slot| &slot.targets),
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
