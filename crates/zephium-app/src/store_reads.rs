//! Bounded asynchronous reads for presentation-only SQLite data.
//!
//! The shell actor never waits on these reads. Launcher input is latest-value,
//! favicon work is bounded per item, and shutdown first proves that the one
//! in-flight read has left the Store API before admitting its terminal barrier.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Instant;

use zephium_core::bookmarks::{
    BookmarkChange, BookmarkFailure, BookmarkNode, BookmarkReply, BookmarkRequest,
};
use zephium_core::export::{ExportKind, HISTORY_CSV_HEADER, MAX_EXPORT_VISITS};
use zephium_core::extension_data::{
    ExtensionDataChange, ExtensionDataInterest, ExtensionDataReply, HistoryReply, HistoryRequest,
};
use zephium_core::ids::{ItemId, ProfileId, SpaceId};
use zephium_core::item::sanitize_page_title;
use zephium_core::ports::store::HistoryHit;
use zephium_core::{icon, navigation};

use crate::{CallbackHandle, Command, SharedStore};

const MAX_PENDING_FAVICON_READS: usize = 64;
const MAX_PENDING_FAVICON_PROBE_READS: usize = 8;
/// Pending history and bookmark surface calls, together.
const MAX_PENDING_SURFACE_CALLS: usize = 8;
/// Pending extension calls and event lookups, apart from the surfaces' so
/// extensions can never crowd out the person's own requests.
pub(crate) const MAX_PENDING_EXTENSION_CALLS: usize = 16;
const MAX_SEARCH_QUERY_BYTES: usize = 4 * 1024;
pub(crate) const FAVICON_CACHE_MAX_AGE_SECONDS: i64 = 7 * 24 * 3600;

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs() as i64)
}

/// Reads the icon of an origin and, when named, of one page of it. Both come
/// with whether they are old enough to be asked of the renderer again.
pub(crate) fn read_favicon(
    store: &dyn zephium_core::ports::store::Store,
    generation: u64,
    id: ItemId,
    profile: ProfileId,
    origin: String,
    page: Option<String>,
) -> StoreReadResult {
    let read = |key: &str| {
        let stored = store
            .favicon_raster_with_age(profile, key)
            .filter(|(bytes, _)| icon::validated_rgba32(bytes).is_some());
        StoredIcon {
            stale: stored
                .as_ref()
                .is_none_or(|(_, age)| *age > FAVICON_CACHE_MAX_AGE_SECONDS),
            rgba: stored.map(|(bytes, _)| bytes),
        }
    };
    let icon = read(&origin);
    let page = page.map(|key| {
        let icon = read(&key);
        Box::new(PageFavicon { key, icon })
    });
    StoreReadResult::Favicon {
        generation,
        id,
        profile,
        origin,
        icon,
        page,
    }
}

/// A stored raster, and whether it is older than the refresh window or absent.
#[derive(Clone, Debug)]
pub struct StoredIcon {
    pub rgba: Option<Vec<u8>>,
    pub stale: bool,
}

/// What the store held for one page of an origin whose icon was asked for.
#[derive(Clone, Debug)]
pub struct PageFavicon {
    pub key: String,
    pub icon: StoredIcon,
}

/// What a launcher read looks through.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sources {
    pub history: bool,
    pub bookmarks: bool,
}

#[cfg(test)]
impl Sources {
    pub const HISTORY: Self = Self {
        history: true,
        bookmarks: false,
    };
}

/// A saved page that matched what was typed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BookmarkHit {
    pub title: String,
    pub url: String,
}

#[derive(Clone, Debug)]
pub enum StoreReadResult {
    History {
        generation: u64,
        profile: ProfileId,
        query: String,
        hits: Vec<HistoryHit>,
        bookmarks: Vec<BookmarkHit>,
    },
    Favicon {
        generation: u64,
        id: ItemId,
        profile: ProfileId,
        origin: String,
        icon: StoredIcon,
        /// The page's own icon, when the request named a page.
        page: Option<Box<PageFavicon>>,
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
    /// The file an export produced, or None when the store could not read it.
    Exported {
        token: u64,
        text: Option<String>,
    },
    ExtensionData {
        token: u64,
        reply: ExtensionDataReply,
    },
    /// What a write changed, for the extensions listening to it.
    ExtensionDataChanged {
        profile: ProfileId,
        change: Box<ExtensionDataChange>,
    },
}

/// What an extension asks of the store, or a lookup an event needs.
#[derive(Clone, Debug)]
pub enum ExtensionDataWork {
    History(HistoryRequest),
    Bookmarks(BookmarkRequest),
    /// The address just visited, for `history.onVisited`.
    Visited(String),
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
        sources: Sources,
    },
    Favicon {
        generation: u64,
        id: ItemId,
        profile: ProfileId,
        origin: String,
        page: Option<String>,
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
    Export {
        token: u64,
        profile: ProfileId,
        kind: ExportKind,
    },
    ExtensionData {
        token: u64,
        profile: ProfileId,
        work: ExtensionDataWork,
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
    extension_calls: VecDeque<Request>,
    interest: HashMap<ProfileId, ExtensionDataInterest>,
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
            extension_calls: VecDeque::new(),
            interest: HashMap::new(),
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
        sources: Sources,
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
            sources,
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

    pub(crate) fn request_export(&self, token: u64, profile: ProfileId, kind: ExportKind) -> bool {
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
        state.surface_calls.push_back(Request::Export {
            token,
            profile,
            kind,
        });
        self.inner.ready.notify_one();
        true
    }

    pub(crate) fn request_extension_data(
        &self,
        token: u64,
        profile: ProfileId,
        work: ExtensionDataWork,
    ) -> bool {
        let mut state = self
            .inner
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if state.stopped
            || !state.accepting
            || state.extension_calls.len() >= MAX_PENDING_EXTENSION_CALLS
        {
            return false;
        }
        state.extension_calls.push_back(Request::ExtensionData {
            token,
            profile,
            work,
        });
        self.inner.ready.notify_one();
        true
    }

    /// Which changes the worker describes for a profile's extensions.
    pub(crate) fn set_interest(&self, profile: ProfileId, interest: ExtensionDataInterest) {
        let mut state = self
            .inner
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if interest.any() {
            state.interest.insert(profile, interest);
        } else {
            state.interest.remove(&profile);
        }
    }

    fn interest(&self, profile: ProfileId) -> ExtensionDataInterest {
        self.inner
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .interest
            .get(&profile)
            .copied()
            .unwrap_or_default()
    }

    pub(crate) fn request_favicon(
        &self,
        generation: u64,
        id: ItemId,
        profile: ProfileId,
        origin: String,
        page: Option<String>,
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
                    page,
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
                    page,
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
        state.extension_calls.clear();
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
        state.extension_calls.clear();
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
        .or_else(|| state.extension_calls.pop_front())
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
const BOOKMARK_READ_LIMIT: usize = 10;

/// Visited pages and saved pages for the launcher, vetted before they leave
/// the worker: only allowed addresses, titles with their controls stripped.
pub(crate) fn read_suggestions(
    store: &dyn zephium_core::ports::store::Store,
    profile: ProfileId,
    query: &str,
    sources: Sources,
) -> (Vec<HistoryHit>, Vec<BookmarkHit>) {
    let hits = if sources.history {
        store
            .search_history(profile, query, HISTORY_READ_LIMIT)
            .into_iter()
            .take(HISTORY_READ_LIMIT as usize)
            .filter(|hit| navigation::is_allowed_str(&hit.url))
            .map(|mut hit| {
                hit.title = sanitize_page_title(&hit.title);
                hit
            })
            .collect()
    } else {
        Vec::new()
    };
    let bookmarks = if sources.bookmarks {
        match store.bookmarks(
            profile,
            BookmarkRequest::Search {
                query: query.to_owned(),
            },
        ) {
            BookmarkReply::Nodes(nodes) => nodes
                .into_iter()
                .filter_map(|node| {
                    let url = node.url?;
                    navigation::is_allowed_str(&url).then(|| BookmarkHit {
                        title: sanitize_page_title(&node.title),
                        url,
                    })
                })
                .take(BOOKMARK_READ_LIMIT)
                .collect(),
            _ => Vec::new(),
        }
    } else {
        Vec::new()
    };
    (hits, bookmarks)
}

fn bookmark_work(
    store: &dyn zephium_core::ports::store::Store,
    profile: ProfileId,
    work: BookmarkWork,
    observe: bool,
) -> (BookmarkSurfaceReply, Option<BookmarkChange>) {
    use zephium_ipc::{bookmark_id, BookmarkCall};
    let made = std::cell::Cell::new(None);
    let write = |request: BookmarkRequest| {
        if observe {
            let (reply, change) = store.bookmarks_observed(profile, request);
            made.set(change);
            reply
        } else {
            store.bookmarks(profile, request)
        }
    };
    let nodes = |reply: BookmarkReply| match reply {
        BookmarkReply::Nodes(nodes) => Ok(nodes),
        BookmarkReply::Failed(failure) => Err(failure),
        BookmarkReply::Added(_)
        | BookmarkReply::Done
        | BookmarkReply::Imported(_)
        | BookmarkReply::Entries(_) => Err(BookmarkFailure::Unavailable),
    };
    let saved = |reply: BookmarkReply| match reply {
        BookmarkReply::Added(id) => BookmarkSurfaceReply::Saved(Some(id)),
        BookmarkReply::Done => BookmarkSurfaceReply::Saved(None),
        BookmarkReply::Failed(failure) => BookmarkSurfaceReply::Failed(failure),
        BookmarkReply::Nodes(_) | BookmarkReply::Imported(_) | BookmarkReply::Entries(_) => {
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
            let reply = saved(write(BookmarkRequest::AddLink {
                parent: None,
                title,
                url,
                if_absent: true,
            }));
            return (reply, made.take());
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
        BookmarkCall::AddFolder { parent: at, title } => {
            parent(at).map(|parent| saved(write(BookmarkRequest::AddFolder { parent, title })))
        }
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
            Ok(saved(write(BookmarkRequest::AddLink {
                parent,
                title,
                url: url.to_string(),
                if_absent: false,
            })))
        }),
        BookmarkCall::Rename { id: target, title } => {
            id(target).map(|id| saved(write(BookmarkRequest::Rename { id, title })))
        }
        BookmarkCall::Move {
            id: target,
            parent: at,
            index,
        } => id(target).and_then(|id| {
            parent(at).map(|parent| saved(write(BookmarkRequest::Move { id, parent, index })))
        }),
        BookmarkCall::Remove { id: target } => {
            id(target).map(|id| saved(write(BookmarkRequest::Remove { id })))
        }
    };
    (
        result.unwrap_or_else(BookmarkSurfaceReply::Failed),
        made.take(),
    )
}

/// Visits fetched per store call, the most a history page returns.
const EXPORT_PAGE: u32 = 200;

/// The file for an export, or None when the store could not be read.
fn export_text(
    store: &dyn zephium_core::ports::store::Store,
    profile: ProfileId,
    kind: ExportKind,
) -> Option<String> {
    match kind {
        ExportKind::Bookmarks => match store.bookmarks(profile, BookmarkRequest::All) {
            BookmarkReply::Nodes(nodes) => Some(zephium_core::export::bookmarks_html(&nodes)),
            _ => None,
        },
        ExportKind::History => history_export(|before| {
            store.history_page_checked(profile, "", None, before, EXPORT_PAGE)
        }),
    }
}

fn history_export(
    mut page: impl FnMut(
        Option<i64>,
    ) -> Result<
        Vec<zephium_core::ports::store::HistoryVisit>,
        zephium_core::ports::store::HistoryPageError,
    >,
) -> Option<String> {
    let mut text = String::from(HISTORY_CSV_HEADER);
    let (mut before, mut exported) = (None, 0);
    while exported < MAX_EXPORT_VISITS {
        let visits = match page(before) {
            Err(
                zephium_core::ports::store::HistoryPageError::TimedOut
                | zephium_core::ports::store::HistoryPageError::NotAdmitted,
            ) => page(before).ok()?,
            result => result.ok()?,
        };
        zephium_core::export::push_history_rows(&visits, &mut text);
        exported += visits.len();
        match visits.last() {
            Some(last) if visits.len() == EXPORT_PAGE as usize => before = Some(last.id),
            _ => return Some(text),
        }
    }
    Some(text)
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
        let mut change = None;
        let result = match request {
            Request::History {
                generation,
                profile,
                query,
                sources,
            } => {
                let (hits, bookmarks) = read_suggestions(&*store, profile, &query, sources);
                StoreReadResult::History {
                    generation,
                    profile,
                    query,
                    hits,
                    bookmarks,
                }
            }
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
                zephium_ipc::HistoryCall::Forget { urls } => {
                    match store.forget_history_urls_checked(profile, &urls) {
                        Some(removed) => {
                            if queue.interest(profile).history {
                                change = Some((
                                    profile,
                                    ExtensionDataChange::VisitsRemoved { urls, all: false },
                                ));
                            }
                            StoreReadResult::HistorySurface {
                                token,
                                profile,
                                visits: Vec::new(),
                                next: None,
                                removed: Some(removed),
                            }
                        }
                        None => StoreReadResult::HistorySurfaceFailed { token, profile },
                    }
                }
                call @ (zephium_ipc::HistoryCall::Clear { .. }
                | zephium_ipc::HistoryCall::ClearSince { .. }) => {
                    let since = match call {
                        zephium_ipc::HistoryCall::ClearSince { since } => Some(since),
                        zephium_ipc::HistoryCall::Clear { range } => {
                            range.window_seconds().map(|window| now_secs() - window)
                        }
                        _ => unreachable!(),
                    };
                    store.clear_chats(profile, since);
                    // Listeners hear which addresses went: the visits since
                    // `since` are taken first, naming them, and the clear
                    // takes the rest of what the range holds.
                    let mut taken = 0;
                    if queue.interest(profile).history {
                        let request = match since {
                            Some(since) => HistoryRequest::RemoveRange {
                                start: since,
                                end: i64::MAX,
                            },
                            None => HistoryRequest::RemoveAll,
                        };
                        if let HistoryReply::Removed { visits, urls, all } =
                            store.extension_history(profile, request)
                        {
                            taken = visits;
                            change =
                                Some((profile, ExtensionDataChange::VisitsRemoved { urls, all }));
                        }
                    }
                    match store.clear_history_checked(profile, since) {
                        Some(removed) => StoreReadResult::HistorySurface {
                            token,
                            profile,
                            visits: Vec::new(),
                            next: None,
                            removed: Some(removed.saturating_add(taken)),
                        },
                        None => StoreReadResult::HistorySurfaceFailed { token, profile },
                    }
                }
                // The shell answers these itself; the store holds nothing of them.
                zephium_ipc::HistoryCall::ClosedTabs
                | zephium_ipc::HistoryCall::ReopenClosed { .. } => {
                    StoreReadResult::HistorySurfaceFailed { token, profile }
                }
            },
            Request::Bookmarks {
                token,
                profile,
                work,
            } => {
                let observe = queue.interest(profile).bookmarks;
                let (reply, made) = bookmark_work(&*store, profile, work, observe);
                change = made.map(|made| (profile, ExtensionDataChange::Bookmark(made)));
                StoreReadResult::Bookmarks {
                    token,
                    profile,
                    reply,
                }
            }
            Request::Import {
                token,
                profile,
                work,
            } => StoreReadResult::Imported {
                token,
                added: match work {
                    ImportWork::Bookmarks { folder, nodes } => {
                        let request = BookmarkRequest::Import { folder, nodes };
                        let (reply, made) = if queue.interest(profile).bookmarks {
                            store.bookmarks_observed(profile, request)
                        } else {
                            (store.bookmarks(profile, request), None)
                        };
                        change = made.map(|made| (profile, ExtensionDataChange::Bookmark(made)));
                        match reply {
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
            Request::Export {
                token,
                profile,
                kind,
            } => StoreReadResult::Exported {
                token,
                text: export_text(&*store, profile, kind),
            },
            Request::ExtensionData {
                token,
                profile,
                work,
            } => {
                let interest = queue.interest(profile);
                let (reply, made) = extension_work(&*store, profile, work, interest);
                change = made.map(|made| (profile, made));
                StoreReadResult::ExtensionData { token, reply }
            }
            Request::Favicon {
                generation,
                id,
                profile,
                origin,
                page,
            } => read_favicon(&*store, generation, id, profile, origin, page),
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
        if let Some((profile, change)) = change {
            let _ = deliver(StoreReadResult::ExtensionDataChanged {
                profile,
                change: Box::new(change),
            });
        }
    }
}

/// An extension's request, with what it changed when someone listens.
fn extension_work(
    store: &dyn zephium_core::ports::store::Store,
    profile: ProfileId,
    work: ExtensionDataWork,
    interest: ExtensionDataInterest,
) -> (ExtensionDataReply, Option<ExtensionDataChange>) {
    match work {
        ExtensionDataWork::History(request) => {
            let reply = store.extension_history(profile, request);
            let change = match &reply {
                HistoryReply::Removed { visits, urls, all }
                    if interest.history && (*visits > 0 || *all) =>
                {
                    Some(ExtensionDataChange::VisitsRemoved {
                        urls: urls.clone(),
                        all: *all,
                    })
                }
                _ => None,
            };
            (ExtensionDataReply::History(reply), change)
        }
        ExtensionDataWork::Bookmarks(request) => {
            let (reply, change) = if interest.bookmarks {
                store.bookmarks_observed(profile, request)
            } else {
                (store.bookmarks(profile, request), None)
            };
            (
                ExtensionDataReply::Bookmarks(reply),
                change.map(ExtensionDataChange::Bookmark),
            )
        }
        ExtensionDataWork::Visited(url) => {
            let change = match store.extension_history(profile, HistoryRequest::Address { url }) {
                HistoryReply::Entries(entries) if interest.history => {
                    entries.into_iter().next().map(ExtensionDataChange::Visited)
                }
                _ => None,
            };
            (ExtensionDataReply::Unavailable, change)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_export_refuses_first_or_middle_page_errors_and_accepts_real_eof() {
        use zephium_core::ports::store::{HistoryPageError, HistoryVisit};
        for error in [
            HistoryPageError::TimedOut,
            HistoryPageError::NotAdmitted,
            HistoryPageError::Failed,
            HistoryPageError::Invalid,
        ] {
            for fail_after in [0, 1] {
                let mut calls = 0;
                assert!(history_export(|_| {
                    calls += 1;
                    if calls > fail_after {
                        return Err(error);
                    }
                    Ok((1..=EXPORT_PAGE)
                        .rev()
                        .map(|id| HistoryVisit {
                            id: i64::from(id),
                            url: format!("https://site{id}.example/"),
                            title: "Kept".into(),
                            visited_at: 1,
                        })
                        .collect())
                })
                .is_none());
                assert_eq!(
                    calls,
                    fail_after
                        + if matches!(
                            error,
                            HistoryPageError::TimedOut | HistoryPageError::NotAdmitted
                        ) {
                            2
                        } else {
                            1
                        }
                );
            }
        }
        assert_eq!(
            history_export(|_| Ok(Vec::new())),
            Some(HISTORY_CSV_HEADER.into())
        );
    }

    #[test]
    fn history_export_retries_only_the_failed_page_once_without_losing_or_duplicating_rows() {
        use zephium_core::ports::store::{HistoryPageError, HistoryVisit};
        for error in [HistoryPageError::TimedOut, HistoryPageError::NotAdmitted] {
            for failed_page in [None, Some(2)] {
                let mut retried = false;
                let mut cursors = Vec::new();
                let text = history_export(|before| {
                    cursors.push(before);
                    if before == failed_page && !retried {
                        retried = true;
                        return Err(error);
                    }
                    Ok(match before {
                        None => (2..=EXPORT_PAGE + 1)
                            .rev()
                            .map(|id| HistoryVisit {
                                id: i64::from(id),
                                url: format!("https://site{id}.example/"),
                                title: "Kept".into(),
                                visited_at: 1,
                            })
                            .collect(),
                        Some(2) => vec![HistoryVisit {
                            id: 1,
                            url: "https://last.example/".into(),
                            title: "Last".into(),
                            visited_at: 1,
                        }],
                        _ => panic!("unexpected cursor"),
                    })
                })
                .expect("one transient page failure is retried");
                assert_eq!(text.lines().count(), EXPORT_PAGE as usize + 2);
                assert_eq!(text.matches("https://last.example/").count(), 1);
                assert_eq!(
                    cursors
                        .iter()
                        .filter(|cursor| **cursor == failed_page)
                        .count(),
                    2
                );
            }
        }
    }

    #[test]
    fn a_failed_checked_history_forget_delivers_failure_instead_of_removed_zero() {
        let store: SharedStore = Arc::new(zephium_store::SqliteStore::in_memory().unwrap());
        let profile = ProfileId::from(777);
        let queue = StoreReadQueue::new();
        assert!(queue.request_history_call(
            43,
            profile,
            zephium_ipc::HistoryCall::Forget {
                urls: vec!["https://example.test/".into()]
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
            matches!(result, Some(StoreReadResult::HistorySurfaceFailed { token: 43, profile: owner }) if owner == profile)
        );
    }

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
    fn clearing_history_also_clears_conversations_from_the_same_instant() {
        let store = Arc::new(crate::shell::tests::FakeStore::default());
        let profile = ProfileId::from(1);
        for (call, window) in [
            (
                zephium_ipc::HistoryCall::Clear {
                    range: zephium_ipc::HistoryRange::Hour,
                },
                Some(3600),
            ),
            (
                zephium_ipc::HistoryCall::Clear {
                    range: zephium_ipc::HistoryRange::Everything,
                },
                None,
            ),
            (
                zephium_ipc::HistoryCall::ClearSince {
                    since: now_secs() - 28 * 24 * 3600,
                },
                Some(28 * 24 * 3600),
            ),
        ] {
            let queue = StoreReadQueue::new();
            assert!(queue.request_history_call(7, profile, call));
            let before = now_secs();
            let mut result = None;
            let stop = queue.clone();
            run_with(store.clone(), queue, |reply| {
                result = Some(reply);
                stop.stop();
                false
            });
            let after = now_secs();
            assert!(matches!(
                result,
                Some(StoreReadResult::HistorySurface {
                    removed: Some(_),
                    ..
                })
            ));

            let clears = std::mem::take(&mut *store.chat_clears.lock().unwrap());
            let [(cleared, since)] = clears[..] else {
                panic!("conversations were cleared {} times", clears.len());
            };
            assert_eq!(cleared, profile);
            match window {
                Some(window) => {
                    assert!((before - window..=after - window).contains(&since.unwrap()));
                }
                None => assert_eq!(since, None),
            }
        }
    }

    #[test]
    fn latest_search_replaces_pending_without_growing_a_fifo() {
        let queue = StoreReadQueue::new();
        let profile = ProfileId::from(1);
        assert!(queue.request_history(1, profile, "old".into(), Sources::HISTORY));
        assert!(queue.request_history(2, profile, "new".into(), Sources::HISTORY));

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
                format!("https://{raw}.example"),
                None
            ));
        }
        assert!(queue.request_favicon(
            999,
            ItemId::from(1),
            profile,
            "https://replacement.example".into(),
            Some("https://replacement.example/page".into())
        ));
        assert!(!queue.request_favicon(
            1000,
            ItemId::from(999),
            profile,
            "https://overflow.example".into(),
            None
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
                format!("https://{generation}.example"),
                None
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
        assert!(queue.request_history(1, ProfileId::from(1), "pending".into(), Sources::HISTORY));
        assert!(queue.quiesce_until(Instant::now()));
        assert!(!queue.request_history(2, ProfileId::from(1), "rejected".into(), Sources::HISTORY));
        queue.resume();
        assert!(queue.request_history(3, ProfileId::from(1), "accepted".into(), Sources::HISTORY));
    }

    #[test]
    fn quiescence_deadline_cannot_claim_an_in_flight_read_is_finished() {
        let queue = StoreReadQueue::new();
        assert!(queue.request_history(1, ProfileId::from(1), "active".into(), Sources::HISTORY));
        let lease = queue.recv().unwrap();
        assert!(!queue.quiesce_until(Instant::now() + std::time::Duration::from_millis(1)));
        drop(lease);
        assert!(queue.quiesce_until(Instant::now()));
    }
}
