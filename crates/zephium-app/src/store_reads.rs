//! Bounded asynchronous reads for presentation-only SQLite data.
//!
//! The shell actor never waits on these reads. Launcher input is latest-value,
//! favicon work is bounded per item, and shutdown first proves that the one
//! in-flight read has left the Store API before admitting its terminal barrier.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Instant;

use zephium_core::bookmarks::{BookmarkFailure, BookmarkNode, BookmarkReply, BookmarkRequest};
use zephium_core::ids::{ItemId, ProfileId, SpaceId};
use zephium_core::item::sanitize_page_title;
use zephium_core::ports::store::HistoryHit;
use zephium_core::{icon, navigation};

use crate::{CallbackHandle, Command, SharedStore};

const MAX_PENDING_FAVICON_READS: usize = 64;
const MAX_PENDING_FAVICON_PROBE_READS: usize = 8;
/// Pending history and bookmark surface calls, together.
const MAX_PENDING_SURFACE_CALLS: usize = 8;
const MAX_SEARCH_QUERY_BYTES: usize = 4 * 1024;
pub(crate) const FAVICON_CACHE_MAX_AGE_SECONDS: i64 = 7 * 24 * 3600;

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs() as i64)
}

#[derive(Clone, Debug)]
pub enum StoreReadResult {
    History {
        generation: u64,
        profile: ProfileId,
        query: String,
        hits: Vec<HistoryHit>,
    },
    Favicon {
        generation: u64,
        id: ItemId,
        profile: ProfileId,
        origin: String,
        rgba: Option<Vec<u8>>,
        /// The stored copy is older than the refresh window, or absent.
        stale: bool,
    },
    FaviconBatch {
        generation: u64,
        profile: ProfileId,
        space: SpaceId,
        origins: Vec<String>,
        rasters: Vec<(String, Vec<u8>)>,
    },
    /// Stored rasters with their age in seconds, for origins a probe asked about.
    FaviconProbe {
        generation: u64,
        profile: ProfileId,
        origins: Vec<String>,
        rasters: Vec<(String, Vec<u8>, i64)>,
    },
    HistorySurface {
        token: u64,
        profile: ProfileId,
        visits: Vec<zephium_core::ports::store::HistoryVisit>,
        next: Option<i64>,
        removed: Option<u32>,
    },
    HistorySurfaceFailed {
        token: u64,
        profile: ProfileId,
    },
    Bookmarks {
        token: u64,
        profile: ProfileId,
        reply: BookmarkSurfaceReply,
    },
    /// How many an import added, or None when the profile could not take it.
    Imported {
        token: u64,
        added: Option<u32>,
    },
}

/// Data read from another browser. Bookmarks and history are written in one
/// store transaction; Essentials join the sidebar, which the shell owns.
#[derive(Clone, Debug)]
pub enum ImportWork {
    Bookmarks {
        folder: String,
        nodes: Vec<zephium_core::bookmarks::ImportNode>,
    },
    History(Vec<zephium_core::ports::store::ImportedVisit>),
    Essentials(Vec<ImportedSite>),
    /// Fixed 32x32 rasters by HTTPS origin, from the source browser's own
    /// icon cache. Stored only; listings read them when they need them.
    Icons(Vec<(String, Vec<u8>)>),
}

/// A site another browser kept at the top of its sidebar.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImportedSite {
    pub url: String,
    pub title: String,
}

/// What the shell asks of bookmarks: a call from chrome, or adding the page
/// in front, whose address only the shell may name.
#[derive(Clone, Debug)]
pub enum BookmarkWork {
    Surface(zephium_ipc::BookmarkCall),
    AddPage { url: String, title: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BookmarkSurfaceReply {
    Listing {
        folder: Option<i64>,
        path: Vec<BookmarkNode>,
        nodes: Vec<BookmarkNode>,
    },
    Results(Vec<BookmarkNode>),
    Saved(Option<i64>),
    Failed(BookmarkFailure),
}

enum Request {
    History {
        generation: u64,
        profile: ProfileId,
        query: String,
    },
    Favicon {
        generation: u64,
        id: ItemId,
        profile: ProfileId,
        origin: String,
    },
    FaviconBatch {
        generation: u64,
        profile: ProfileId,
        space: SpaceId,
        origins: Vec<String>,
    },
    FaviconProbe {
        generation: u64,
        profile: ProfileId,
        origins: Vec<String>,
    },
    HistorySurface {
        token: u64,
        profile: ProfileId,
        call: zephium_ipc::HistoryCall,
    },
    Bookmarks {
        token: u64,
        profile: ProfileId,
        work: BookmarkWork,
    },
    Import {
        token: u64,
        profile: ProfileId,
        work: ImportWork,
    },
}

struct State {
    accepting: bool,
    stopped: bool,
    in_flight: bool,
    history: Option<Request>,
    surface_calls: VecDeque<Request>,
    favicon_batch: Option<Request>,
    favicons: HashMap<ItemId, Request>,
    favicon_order: VecDeque<ItemId>,
    favicon_probes: VecDeque<Request>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            accepting: true,
            stopped: false,
            in_flight: false,
            history: None,
            surface_calls: VecDeque::new(),
            favicon_batch: None,
            favicons: HashMap::new(),
            favicon_order: VecDeque::new(),
            favicon_probes: VecDeque::new(),
        }
    }
}

struct Inner {
    state: Mutex<State>,
    ready: Condvar,
}

#[derive(Clone)]
pub(crate) struct StoreReadQueue {
    inner: Arc<Inner>,
}

impl StoreReadQueue {
    #[cfg(test)]
    pub(crate) fn pending_surface_calls_for_test(&self) -> usize {
        self.inner.state.lock().unwrap().surface_calls.len()
    }

    pub(crate) fn new() -> Self {
        Self {
            inner: Arc::new(Inner {
                state: Mutex::new(State::default()),
                ready: Condvar::new(),
            }),
        }
    }

    pub(crate) fn request_history(
        &self,
        generation: u64,
        profile: ProfileId,
        query: String,
    ) -> bool {
        if query.len() > MAX_SEARCH_QUERY_BYTES {
            return false;
        }
        let mut state = self
            .inner
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if state.stopped || !state.accepting {
            return false;
        }
        state.history = Some(Request::History {
            generation,
            profile,
            query,
        });
        self.inner.ready.notify_one();
        true
    }

    /// Queues one history-surface request. Unlike launcher input these are not
    /// latest-value: each carries a completion the caller is waiting on.
    pub(crate) fn request_history_call(
        &self,
        token: u64,
        profile: ProfileId,
        call: zephium_ipc::HistoryCall,
    ) -> bool {
        if !call.validate() {
            return false;
        }
        let mut state = self
            .inner
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if state.stopped
            || !state.accepting
            || state.surface_calls.len() >= MAX_PENDING_SURFACE_CALLS
        {
            return false;
        }
        state.surface_calls.push_back(Request::HistorySurface {
            token,
            profile,
            call,
        });
        self.inner.ready.notify_one();
        true
    }

    /// Queues one bookmark request; like history calls, each carries a
    /// completion and shares their bound.
    pub(crate) fn request_bookmarks(
        &self,
        token: u64,
        profile: ProfileId,
        work: BookmarkWork,
    ) -> bool {
        if matches!(&work, BookmarkWork::Surface(call) if !call.validate()) {
            return false;
        }
        let mut state = self
            .inner
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if state.stopped
            || !state.accepting
            || state.surface_calls.len() >= MAX_PENDING_SURFACE_CALLS
        {
            return false;
        }
        state.surface_calls.push_back(Request::Bookmarks {
            token,
            profile,
            work,
        });
        self.inner.ready.notify_one();
        true
    }

    pub(crate) fn request_import(&self, token: u64, profile: ProfileId, work: ImportWork) -> bool {
        let mut state = self
            .inner
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if state.stopped
            || !state.accepting
            || state.surface_calls.len() >= MAX_PENDING_SURFACE_CALLS
        {
            return false;
        }
        state.surface_calls.push_back(Request::Import {
            token,
            profile,
            work,
        });
        self.inner.ready.notify_one();
        true
    }

    pub(crate) fn request_favicon(
        &self,
        generation: u64,
        id: ItemId,
        profile: ProfileId,
        origin: String,
    ) -> bool {
        let mut state = self
            .inner
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if state.stopped || !state.accepting {
            return false;
        }
        if state.favicons.contains_key(&id) {
            state.favicons.insert(
                id,
                Request::Favicon {
                    generation,
                    id,
                    profile,
                    origin,
                },
            );
        } else {
            if state.favicons.len() >= MAX_PENDING_FAVICON_READS {
                return false;
            }
            state.favicons.insert(
                id,
                Request::Favicon {
                    generation,
                    id,
                    profile,
                    origin,
                },
            );
            state.favicon_order.push_back(id);
        }
        self.inner.ready.notify_one();
        true
    }

    pub(crate) fn cancel_favicon(&self, id: ItemId) {
        let mut state = self
            .inner
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.favicons.remove(&id);
        state.favicon_order.retain(|candidate| *candidate != id);
    }

    pub(crate) fn request_favicon_batch(
        &self,
        generation: u64,
        profile: ProfileId,
        space: SpaceId,
        origins: Vec<String>,
    ) -> bool {
        if origins.len() > zephium_core::ports::store::MAX_FAVICON_BATCH_ORIGINS {
            return false;
        }
        let mut state = self
            .inner
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if state.stopped || !state.accepting {
            return false;
        }
        state.favicon_batch = Some(Request::FaviconBatch {
            generation,
            profile,
            space,
            origins,
        });
        self.inner.ready.notify_one();
        true
    }

    pub(crate) fn request_favicon_probe(
        &self,
        generation: u64,
        profile: ProfileId,
        origins: Vec<String>,
    ) -> bool {
        if origins.len() > zephium_core::ports::store::MAX_FAVICON_BATCH_ORIGINS {
            return false;
        }
        let mut state = self
            .inner
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if state.stopped
            || !state.accepting
            || state.favicon_probes.len() >= MAX_PENDING_FAVICON_PROBE_READS
        {
            return false;
        }
        state.favicon_probes.push_back(Request::FaviconProbe {
            generation,
            profile,
            origins,
        });
        self.inner.ready.notify_one();
        true
    }

    fn recv(&self) -> Option<Lease> {
        let mut state = self
            .inner
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        loop {
            if state.stopped {
                return None;
            }
            if state.accepting && !state.in_flight {
                if let Some(request) = pop_browser_read(&mut state) {
                    state.in_flight = true;
                    return Some(Lease {
                        queue: self.clone(),
                        request: Some(request),
                    });
                }
            }
            state = self
                .inner
                .ready
                .wait(state)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
        }
    }

    fn finish_request(&self) {
        let mut state = self
            .inner
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.in_flight = false;
        self.inner.ready.notify_all();
    }

    /// Stops admission, discards presentation-only pending reads, and proves
    /// no Store RPC is executing before a terminal storage command begins.
    pub(crate) fn quiesce_until(&self, deadline: Instant) -> bool {
        let mut state = self
            .inner
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.accepting = false;
        state.history = None;
        state.surface_calls.clear();
        state.favicon_batch = None;
        state.favicons.clear();
        state.favicon_order.clear();
        state.favicon_probes.clear();
        while state.in_flight {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return false;
            }
            let (next, timeout) = self
                .inner
                .ready
                .wait_timeout(state, remaining)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            state = next;
            if timeout.timed_out() && state.in_flight {
                return false;
            }
        }
        true
    }

    pub(crate) fn resume(&self) {
        let mut state = self
            .inner
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if !state.stopped {
            state.accepting = true;
            self.inner.ready.notify_one();
        }
    }

    pub(crate) fn stop(&self) {
        let mut state = self
            .inner
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.stopped = true;
        state.accepting = false;
        state.history = None;
        state.surface_calls.clear();
        state.favicon_batch = None;
        state.favicons.clear();
        state.favicon_order.clear();
        state.favicon_probes.clear();
        self.inner.ready.notify_all();
    }
}

fn pop_browser_read(state: &mut State) -> Option<Request> {
    state
        .history
        .take()
        .or_else(|| state.surface_calls.pop_front())
        .or_else(|| state.favicon_batch.take())
        .or_else(|| {
            while let Some(id) = state.favicon_order.pop_front() {
                if let Some(request) = state.favicons.remove(&id) {
                    return Some(request);
                }
            }
            None
        })
        .or_else(|| state.favicon_probes.pop_front())
}

struct Lease {
    queue: StoreReadQueue,
    request: Option<Request>,
}

impl Lease {
    fn take(&mut self) -> Option<Request> {
        self.request.take()
    }
}

impl Drop for Lease {
    fn drop(&mut self) {
        self.queue.finish_request();
    }
}

pub(crate) struct StoreReaderStopGuard(StoreReadQueue);

impl StoreReaderStopGuard {
    pub(crate) fn new(queue: StoreReadQueue) -> Self {
        Self(queue)
    }
}

impl Drop for StoreReaderStopGuard {
    fn drop(&mut self) {
        self.0.stop();
    }
}

/// The worker loop with its delivery injected, so a test can drive the real
/// queue and the real request handling without an actor behind it.
#[cfg(test)]
pub(crate) fn run_for_test(
    store: SharedStore,
    queue: StoreReadQueue,
    sink: std::sync::mpsc::Sender<StoreReadResult>,
) {
    run_with(store, queue, move |result| sink.send(result).is_ok());
}

/// Candidates handed to ranking. Larger than what is shown, so recorded
/// searches and already-open tabs can be filtered out without leaving the
/// history section short.
const HISTORY_READ_LIMIT: u32 = 10;

fn bookmark_work(
    store: &dyn zephium_core::ports::store::Store,
    profile: ProfileId,
    work: BookmarkWork,
) -> BookmarkSurfaceReply {
    use zephium_ipc::{bookmark_id, BookmarkCall};
    let nodes = |reply: BookmarkReply| match reply {
        BookmarkReply::Nodes(nodes) => Ok(nodes),
        BookmarkReply::Failed(failure) => Err(failure),
        BookmarkReply::Added(_) | BookmarkReply::Done | BookmarkReply::Imported(_) => {
            Err(BookmarkFailure::Unavailable)
        }
    };
    let saved = |reply: BookmarkReply| match reply {
        BookmarkReply::Added(id) => BookmarkSurfaceReply::Saved(Some(id)),
        BookmarkReply::Done => BookmarkSurfaceReply::Saved(None),
        BookmarkReply::Failed(failure) => BookmarkSurfaceReply::Failed(failure),
        BookmarkReply::Nodes(_) | BookmarkReply::Imported(_) => {
            BookmarkSurfaceReply::Failed(BookmarkFailure::Unavailable)
        }
    };
    // Ids were validated at admission; a parse that fails here is a bug, and
    // reads as a missing bookmark rather than the top level.
    let parent = |parent: Option<String>| match parent {
        None => Ok(None),
        Some(id) => bookmark_id(&id).map(Some).ok_or(BookmarkFailure::Missing),
    };
    let id = |id: String| bookmark_id(&id).ok_or(BookmarkFailure::Missing);
    let call = match work {
        BookmarkWork::AddPage { url, title } => {
            return saved(store.bookmarks(
                profile,
                BookmarkRequest::AddLink {
                    parent: None,
                    title,
                    url,
                    if_absent: true,
                },
            ))
        }
        BookmarkWork::Surface(call) => call,
    };
    let result = match call {
        BookmarkCall::List { folder } => parent(folder).and_then(|folder| {
            let listed =
                nodes(store.bookmarks(profile, BookmarkRequest::Children { parent: folder }))?;
            let path = match folder {
                Some(id) => nodes(store.bookmarks(profile, BookmarkRequest::Path { id }))?,
                None => Vec::new(),
            };
            Ok(BookmarkSurfaceReply::Listing {
                folder,
                path,
                nodes: listed,
            })
        }),
        BookmarkCall::Reveal { id: target } => id(target).and_then(|id| {
            let mut path = nodes(store.bookmarks(profile, BookmarkRequest::Path { id }))?;
            // The path ends at `id` itself; what holds it is the one above.
            path.pop();
            let folder = path.last().map(|holder| holder.id);
            let listed =
                nodes(store.bookmarks(profile, BookmarkRequest::Children { parent: folder }))?;
            Ok(BookmarkSurfaceReply::Listing {
                folder,
                path,
                nodes: listed,
            })
        }),
        BookmarkCall::Search { query } => {
            nodes(store.bookmarks(profile, BookmarkRequest::Search { query }))
                .map(BookmarkSurfaceReply::Results)
        }
        BookmarkCall::AddFolder { parent: at, title } => parent(at).map(|parent| {
            saved(store.bookmarks(profile, BookmarkRequest::AddFolder { parent, title }))
        }),
        BookmarkCall::AddLink {
            parent: at,
            title,
            url,
        } => parent(at).and_then(|parent| {
            let url =
                zephium_core::navigation::web_address(&url).ok_or(BookmarkFailure::Invalid)?;
            let title = if title.trim().is_empty() {
                url.host_str().unwrap_or_default().to_owned()
            } else {
                title
            };
            Ok(saved(store.bookmarks(
                profile,
                BookmarkRequest::AddLink {
                    parent,
                    title,
                    url: url.to_string(),
                    if_absent: false,
                },
            )))
        }),
        BookmarkCall::Rename { id: target, title } => id(target)
            .map(|id| saved(store.bookmarks(profile, BookmarkRequest::Rename { id, title }))),
        BookmarkCall::Move {
            id: target,
            parent: at,
            index,
        } => id(target).and_then(|id| {
            parent(at).map(|parent| {
                saved(store.bookmarks(profile, BookmarkRequest::Move { id, parent, index }))
            })
        }),
        BookmarkCall::Remove { id: target } => {
            id(target).map(|id| saved(store.bookmarks(profile, BookmarkRequest::Remove { id })))
        }
    };
    result.unwrap_or_else(BookmarkSurfaceReply::Failed)
}

pub(crate) fn run(store: SharedStore, queue: StoreReadQueue, callback: CallbackHandle) {
    run_with(store, queue, move |result| {
        callback.dispatch(Command::StoreRead(result))
    });
}

fn run_with(
    store: SharedStore,
    queue: StoreReadQueue,
    mut deliver: impl FnMut(StoreReadResult) -> bool,
) {
    while let Some(mut lease) = queue.recv() {
        let Some(request) = lease.take() else {
            continue;
        };
        let result = match request {
            Request::History {
                generation,
                profile,
                query,
            } => StoreReadResult::History {
                generation,
                profile,
                hits: store
                    .search_history(profile, &query, HISTORY_READ_LIMIT)
                    .into_iter()
                    .take(HISTORY_READ_LIMIT as usize)
                    .filter(|hit| navigation::is_allowed_str(&hit.url))
                    .map(|mut hit| {
                        hit.title = sanitize_page_title(&hit.title);
                        hit
                    })
                    .collect(),
                query,
            },
            Request::HistorySurface {
                token,
                profile,
                call,
            } => match call {
                zephium_ipc::HistoryCall::Page {
                    query,
                    range,
                    before,
                    limit,
                } => {
                    let before = before
                        .as_deref()
                        .and_then(|cursor| cursor.parse::<i64>().ok());
                    let since = range.window_seconds().map(|window| now_secs() - window);
                    let visits =
                        store.history_page(profile, &query, since, before, u32::from(limit));
                    // A full page implies there may be more; a short one is the end.
                    let next = (visits.len() == usize::from(limit))
                        .then(|| visits.last().map(|visit| visit.id))
                        .flatten();
                    StoreReadResult::HistorySurface {
                        token,
                        profile,
                        visits,
                        next,
                        removed: None,
                    }
                }
                zephium_ipc::HistoryCall::Forget { urls } => StoreReadResult::HistorySurface {
                    token,
                    profile,
                    visits: Vec::new(),
                    next: None,
                    removed: Some(store.forget_history_urls(profile, &urls)),
                },
                zephium_ipc::HistoryCall::Clear { range } => {
                    let since = range.window_seconds().map(|window| now_secs() - window);
                    match store.clear_history_checked(profile, since) {
                        Some(removed) => StoreReadResult::HistorySurface {
                            token,
                            profile,
                            visits: Vec::new(),
                            next: None,
                            removed: Some(removed),
                        },
                        None => StoreReadResult::HistorySurfaceFailed { token, profile },
                    }
                }
            },
            Request::Bookmarks {
                token,
                profile,
                work,
            } => StoreReadResult::Bookmarks {
                token,
                profile,
                reply: bookmark_work(&*store, profile, work),
            },
            Request::Import {
                token,
                profile,
                work,
            } => StoreReadResult::Imported {
                token,
                added: match work {
                    ImportWork::Bookmarks { folder, nodes } => {
                        match store.bookmarks(profile, BookmarkRequest::Import { folder, nodes }) {
                            BookmarkReply::Imported(added) => Some(added),
                            _ => None,
                        }
                    }
                    ImportWork::History(visits) => store.import_history(profile, visits),
                    ImportWork::Icons(icons) => store.import_favicons(profile, icons),
                    // The shell keeps these itself and never queues them.
                    ImportWork::Essentials(_) => None,
                },
            },
            Request::Favicon {
                generation,
                id,
                profile,
                origin,
            } => {
                let stored = store
                    .favicon_raster_with_age(profile, &origin)
                    .filter(|(bytes, _)| icon::validated_rgba32(bytes).is_some());
                let stale = stored
                    .as_ref()
                    .is_none_or(|(_, age)| *age > FAVICON_CACHE_MAX_AGE_SECONDS);
                StoreReadResult::Favicon {
                    generation,
                    id,
                    profile,
                    origin,
                    rgba: stored.map(|(bytes, _)| bytes),
                    stale,
                }
            }
            Request::FaviconBatch {
                generation,
                profile,
                space,
                origins,
            } => {
                let requested: std::collections::HashSet<&str> =
                    origins.iter().map(String::as_str).collect();
                let rasters = store
                    .favicon_rasters(profile, &origins)
                    .into_iter()
                    .take(zephium_core::ports::store::MAX_FAVICON_BATCH_ORIGINS)
                    .filter(|(origin, bytes)| {
                        requested.contains(origin.as_str())
                            && icon::validated_rgba32(bytes).is_some()
                    })
                    .collect();
                StoreReadResult::FaviconBatch {
                    generation,
                    profile,
                    space,
                    origins,
                    rasters,
                }
            }
            Request::FaviconProbe {
                generation,
                profile,
                origins,
            } => {
                let rasters = origins
                    .iter()
                    .filter_map(|origin| {
                        let (bytes, age) = store.favicon_raster_with_age(profile, origin)?;
                        icon::validated_rgba32(&bytes)?;
                        Some((origin.clone(), bytes, age))
                    })
                    .collect();
                StoreReadResult::FaviconProbe {
                    generation,
                    profile,
                    origins,
                    rasters,
                }
            }
        };
        let _ = deliver(result);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_failed_checked_history_clear_delivers_failure_instead_of_removed_zero() {
        let store: SharedStore = Arc::new(zephium_store::SqliteStore::in_memory().unwrap());
        let profile = ProfileId::from(777); // No authoritative profile exists.
        let queue = StoreReadQueue::new();
        assert!(queue.request_history_call(
            42,
            profile,
            zephium_ipc::HistoryCall::Clear {
                range: zephium_ipc::HistoryRange::Everything,
            }
        ));
        let mut result = None;
        let stop = queue.clone();
        run_with(store, queue, |reply| {
            result = Some(reply);
            stop.stop();
            false
        });
        assert!(
            matches!(result, Some(StoreReadResult::HistorySurfaceFailed { token: 42, profile: owner }) if owner == profile)
        );
    }

    #[test]
    fn latest_search_replaces_pending_without_growing_a_fifo() {
        let queue = StoreReadQueue::new();
        let profile = ProfileId::from(1);
        assert!(queue.request_history(1, profile, "old".into()));
        assert!(queue.request_history(2, profile, "new".into()));

        let mut lease = queue.recv().unwrap();
        assert!(matches!(
            lease.take(),
            Some(Request::History {
                generation: 2,
                query,
                ..
            }) if query == "new"
        ));
    }

    #[test]
    fn favicon_admission_is_bounded_and_replacement_is_per_item() {
        let queue = StoreReadQueue::new();
        let profile = ProfileId::from(1);
        for raw in 1..=MAX_PENDING_FAVICON_READS as u128 {
            assert!(queue.request_favicon(
                raw as u64,
                ItemId::from(raw),
                profile,
                format!("https://{raw}.example")
            ));
        }
        assert!(queue.request_favicon(
            999,
            ItemId::from(1),
            profile,
            "https://replacement.example".into()
        ));
        assert!(!queue.request_favicon(
            1000,
            ItemId::from(999),
            profile,
            "https://overflow.example".into()
        ));
    }

    #[test]
    fn repeated_favicon_cancel_and_replace_keeps_order_metadata_bounded() {
        let queue = StoreReadQueue::new();
        let profile = ProfileId::from(1);
        let id = ItemId::from(1);
        for generation in 1..=10_000 {
            assert!(queue.request_favicon(
                generation,
                id,
                profile,
                format!("https://{generation}.example")
            ));
            queue.cancel_favicon(id);
        }
        let state = queue.inner.state.lock().unwrap();
        assert!(state.favicons.is_empty());
        assert!(state.favicon_order.is_empty());
    }

    #[test]
    fn quiescence_clears_pending_and_can_resume_after_retryable_shutdown() {
        let queue = StoreReadQueue::new();
        assert!(queue.request_history(1, ProfileId::from(1), "pending".into()));
        assert!(queue.quiesce_until(Instant::now()));
        assert!(!queue.request_history(2, ProfileId::from(1), "rejected".into()));
        queue.resume();
        assert!(queue.request_history(3, ProfileId::from(1), "accepted".into()));
    }

    #[test]
    fn quiescence_deadline_cannot_claim_an_in_flight_read_is_finished() {
        let queue = StoreReadQueue::new();
        assert!(queue.request_history(1, ProfileId::from(1), "active".into()));
        let lease = queue.recv().unwrap();
        assert!(!queue.quiesce_until(Instant::now() + std::time::Duration::from_millis(1)));
        drop(lease);
        assert!(queue.quiesce_until(Instant::now()));
    }
}
