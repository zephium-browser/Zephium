idates every
    /// field against its current exact pending obligation.
    ChromePresentationApplied {
        id: ItemId,
        navigation: NavigationPresentationId,
        url: String,
        active: Option<ItemId>,
        projection_revision: String,
        applied: bool,
    },
    /// Fail-closed deadline for one exact renderer discard-safety probe.
    DiscardProbeTimeout {
        id: ItemId,
        probe: DiscardProbeId,
    },
    /// Deadline wake for one bounded foreground residency request.
    ViewCapacityRetry(ItemId),
    /// Exact-generation wakeup for a native profile-erasure callback. The
    /// outcome itself stays in a bounded inbox so queue overload cannot lose
    /// the security-critical proof.
    ProfileDeletionReady(ProfileId),
    /// Small wake for one exact compiler result retained in the bounded
    /// profile inbox. The immutable compiled artifact never enters the actor
    /// command queue.
    BlockerReady(ProfileId),
    /// Wake for one token-tagged durable preference mutation or
    /// reconciliation result retained in the bounded blocker inbox.
    BlockerStoreReady(ProfileId),
    /// Exact bounded-backoff retry for an indeterminate durable preference
    /// reconciliation.
    BlockerPreferenceRetry {
        profile: ProfileId,
        token: u64,
    },
    /// The startup protection deadline for one profile: views held for its
    /// enabled policy open without it from here on.
    BlockerStartupDeadline {
        profile: ProfileId,
        token: u64,
    },
    /// Bounded follow-up for an exact user-requested refresh or the reserved
    /// internal activation token. Ordinary refresh scheduling remains on the
    /// low-frequency maintenance heartbeat.
    BlockerCatalogPoll {
        operation: u64,
        attempt: u8,
    },
    /// The catalog's update worker published a new status, such as a finished
    /// download, so the catalog is maintained now rather than at the next tick.
    BlockerCatalogChanged,
    /// One bounded-backoff retry for journal reconciliation, native erasure,
    /// or local SQLite finalization.
    ProfileDeletionRetry {
        profile: ProfileId,
        generation: u64,
    },
    /// Completion from the bounded storage-read worker. Every result carries
    /// the exact request generation and is revalidated against current shell
    /// state before it can affect privileged projections.
    StoreRead(StoreReadResult),
    /// Internal one-shot debounce fired by the queue's single timer thread.
    Persist,
    /// Periodic maintenance heartbeat; idle tabs suspend or hibernate even
    /// when no user command arrives.
    Tick,
    Engine(EngineEvent),
    /// Ordered process-boundary barrier. The actor snapshots after every
    /// command already queued ahead of this one, then flushes the store.
    Shutdown {
        deadline: std::time::Instant,
        ack: SyncSender<ShutdownOutcome>,
    },
}

type Completion<T> = Arc<Mutex<Option<Box<dyn FnOnce(T) + Send>>>>;

#[derive(Clone)]
pub struct ResourceCompletion(Completion<zephium_core::resources::ResourceReply>);
impl ResourceCompletion {
    pub fn new(done: impl FnOnce(zephium_core::resources::ResourceReply) + Send + 'static) -> Self {
        Self(Arc::new(Mutex::new(Some(Box::new(done)))))
    }
    pub fn finish(self, reply: zephium_core::resources::ResourceReply) {
        let done = self
            .0
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take();
        if let Some(done) = done {
            done(reply);
        }
    }
}
impl fmt::Debug for ResourceCompletion {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ResourceCompletion")
    }
}

/// Where an icon comes from, for a request the prober answers with
/// [`Command::FaviconFetched`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FaviconPageSource {
    /// Icon addresses the page declared, best first. Each is fetched without
    /// cookies, credentials or referrer; the first that decodes wins.
    Addresses(Vec<String>),
    /// A PNG the web engine already chose for the page.
    Encoded(Vec<u8>),
}

/// One request to the prober.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FaviconProbe {
    /// An origin's icon, answered exactly once with [`Command::FaviconProbed`]
    /// for the same profile and origin.
    Origin { profile: ProfileId, origin: String },
    /// A page's icon, answered exactly once with [`Command::FaviconFetched`]
    /// for the same ticket.
    Page {
        ticket: u64,
        profile: ProfileId,
        origin: String,
        source: FaviconPageSource,
    },
}

/// Fetches icons anonymously and decodes them into the fixed raster.
pub type FaviconProber = Arc<dyn Fn(FaviconProbe) + Send + Sync>;

#[derive(Clone)]
pub struct FaviconProberAttachment {
    pub probe: FaviconProber,
    /// Cancels active network requests, which still send their ordinary replies.
    pub cancel_network: Arc<dyn Fn() + Send + Sync>,
}
impl fmt::Debug for FaviconProberAttachment {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("FaviconProberAttachment")
    }
}

/// Told the proxy chosen in Zephium, or `None` for the system's: when
/// attached, then on every change.
pub type AppProxyReport = Arc<dyn Fn(Option<zephium_core::proxy::ProxyServer>) + Send + Sync>;

#[derive(Clone)]
pub struct AppProxyAttachment(pub AppProxyReport);
impl fmt::Debug for AppProxyAttachment {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("AppProxyAttachment")
    }
}

#[derive(Clone)]
pub struct NotesAttachment(pub zephium_core::ports::notes::SharedNotes);
impl fmt::Debug for NotesAttachment {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("NotesAttachment")
    }
}

#[derive(Clone)]
pub struct NoteCompletion(Completion<zephium_core::notes::NoteReply>);
impl NoteCompletion {
    pub fn new(done: impl FnOnce(zephium_core::notes::NoteReply) + Send + 'static) -> Self {
        Self(Arc::new(Mutex::new(Some(Box::new(done)))))
    }
    pub fn finish(self, reply: zephium_core::notes::NoteReply) {
        let done = self
            .0
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take();
        if let Some(done) = done {
            done(reply);
        }
    }
}
impl fmt::Debug for NoteCompletion {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("NoteCompletion")
    }
}

#[derive(Clone)]
pub struct TabCompletion(Completion<Option<ItemId>>);
impl TabCompletion {
    pub fn new(done: impl FnOnce(Option<ItemId>) + Send + 'static) -> Self {
        Self(Arc::new(Mutex::new(Some(Box::new(done)))))
    }
    pub fn finish(self, tab: Option<ItemId>) {
        let done = self
            .0
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take();
        if let Some(done) = done {
            done(tab);
        }
    }
}
impl fmt::Debug for TabCompletion {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("TabCompletion")
    }
}

#[derive(Clone)]
pub struct HistoryCompletion(Completion<zephium_ipc::HistoryResponse>);
impl HistoryCompletion {
    pub fn new(done: impl FnOnce(zephium_ipc::HistoryResponse) + Send + 'static) -> Self {
        Self(Arc::new(Mutex::new(Some(Box::new(done)))))
    }
    pub fn finish(self, response: zephium_ipc::HistoryResponse) {
        let done = self
            .0
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take();
        if let Some(done) = done {
            done(response);
        }
    }
}
#[derive(Clone)]
pub struct BrowsingDataCompletion(Completion<zephium_ipc::BrowsingDataResponse>);
impl BrowsingDataCompletion {
    pub fn new(done: impl FnOnce(zephium_ipc::BrowsingDataResponse) + Send + 'static) -> Self {
        Self(Arc::new(Mutex::new(Some(Box::new(done)))))
    }
    pub fn finish(self, response: zephium_ipc::BrowsingDataResponse) {
        let done = self
            .0
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take();
        if let Some(done) = done {
            done(response);
        }
    }
}
#[derive(Clone)]
pub struct TimeCompletion(Completion<zephium_ipc::TimeResponse>);
impl TimeCompletion {
    pub fn new(done: impl FnOnce(zephium_ipc::TimeResponse) + Send + 'static) -> Self {
        Self(Arc::new(Mutex::new(Some(Box::new(done)))))
    }
    pub fn finish(self, response: zephium_ipc::TimeResponse) {
        let done = self
            .0
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take();
        if let Some(done) = done {
            done(response);
        }
    }
}
impl fmt::Debug for TimeCompletion {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("TimeCompletion")
    }
}
#[derive(Clone)]
pub struct ZoomCompletion(Completion<zephium_ipc::ZoomResponse>);
impl ZoomCompletion {
    pub fn new(done: impl FnOnce(zephium_ipc::ZoomResponse) + Send + 'static) -> Self {
        Self(Arc::new(Mutex::new(Some(Box::new(done)))))
    }
    pub fn finish(self, response: zephium_ipc::ZoomResponse) {
        let done = self
            .0
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take();
        if let Some(done) = done {
            done(response);
        }
    }
}
impl fmt::Debug for ZoomCompletion {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ZoomCompletion")
    }
}
#[derive(Clone)]
pub struct BookmarkCompletion(Completion<zephium_ipc::BookmarkResponse>);
impl BookmarkCompletion {
    pub fn new(done: impl FnOnce(zephium_ipc::BookmarkResponse) + Send + 'static) -> Self {
        Self(Arc::new(Mutex::new(Some(Box::new(done)))))
    }
    pub fn finish(self, response: zephium_ipc::BookmarkResponse) {
        let done = self
            .0
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take();
        if let Some(done) = done {
            done(response);
        }
    }
}
/// The serialized document of a page, kept for the tab that shows its source.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PageSource {
    pub url: String,
    pub text: String,
    /// The page was longer than the engine reads.
    pub truncated: bool,
}

#[derive(Clone)]
pub struct SourceCompletion(Completion<Option<Arc<PageSource>>>);
impl SourceCompletion {
    pub fn new(done: impl FnOnce(Option<Arc<PageSource>>) + Send + 'static) -> Self {
        Self(Arc::new(Mutex::new(Some(Box::new(done)))))
    }
    pub fn finish(self, document: Option<Arc<PageSource>>) {
        let done = self
            .0
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take();
        if let Some(done) = done {
            done(document);
        }
    }
}
impl fmt::Debug for SourceCompletion {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SourceCompletion")
    }
}
#[derive(Clone)]
pub struct HistoryListCompletion(Completion<zephium_core::ports::engine::HistoryEntries>);
impl HistoryListCompletion {
    pub fn new(
        done: impl FnOnce(zephium_core::ports::engine::HistoryEntries) + Send + 'static,
    ) -> Self {
        Self(Arc::new(Mutex::new(Some(Box::new(done)))))
    }
    pub fn finish(self, entries: zephium_core::ports::engine::HistoryEntries) {
        let done = self
            .0
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take();
        if let Some(done) = done {
            done(entries);
        }
    }
}
impl fmt::Debug for HistoryListCompletion {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("HistoryListCompletion")
    }
}
#[derive(Clone)]
pub struct ExportCompletion(Completion<Result<String, zephium_core::export::ExportRefusal>>);
impl ExportCompletion {
    pub fn new(
        done: impl FnOnce(Result<String, zephium_core::export::ExportRefusal>) + Send + 'static,
    ) -> Self {
        Self(Arc::new(Mutex::new(Some(Box::new(done)))))
    }
    pub fn finish(self, reply: Result<String, zephium_core::export::ExportRefusal>) {
        let done = self
            .0
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take();
        if let Some(done) = done {
            done(reply);
        }
    }
}
impl fmt::Debug for ExportCompletion {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ExportCompletion")
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ImportDestination {
    pub profile: ProfileId,
    pub space: SpaceId,
}

#[derive(Clone)]
pub struct ImportDestinationCompletion(Completion<Option<ImportDestination>>);
impl ImportDestinationCompletion {
    pub fn new(done: impl FnOnce(Option<ImportDestination>) + Send + 'static) -> Self {
        Self(Arc::new(Mutex::new(Some(Box::new(done)))))
    }
    pub fn finish(self, profile: Option<ImportDestination>) {
        let done = self
            .0
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take();
        if let Some(done) = done {
            done(profile);
        }
    }
}
impl fmt::Debug for ImportDestinationCompletion {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ImportDestinationCompletion")
    }
}
#[derive(Clone)]
pub struct ImportCompletion(Completion<Option<(u32, bool)>>);
impl ImportCompletion {
    pub fn new(done: impl FnOnce(Option<(u32, bool)>) + Send + 'static) -> Self {
        Self(Arc::new(Mutex::new(Some(Box::new(done)))))
    }
    pub fn finish(self, added: Option<(u32, bool)>) {
        let done = self
            .0
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take();
        if let Some(done) = done {
            done(added);
        }
    }
}
impl fmt::Debug for ImportCompletion {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ImportCompletion")
    }
}
impl fmt::Debug for BookmarkCompletion {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("BookmarkCompletion")
    }
}
impl fmt::Debug for BrowsingDataCompletion {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("BrowsingDataCompletion")
    }
}
impl fmt::Debug for HistoryCompletion {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("HistoryCompletion")
    }
}
