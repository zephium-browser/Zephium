//! Hosts Home's maps: one driver for each search whose words read as a map,
//! its changes folded beside the search for the next snapshot and streamed to
//! the main window. The results page each map reads comes from the shell,
//! which loads it in a background tab; the pages a map offers are resolved
//! here once and never reach the frame as addresses.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use zephium_app::home::{
    HomeCommand, HomeMaps, HomeMapsAttachment, HomeRecords, MapKind, MapRefusal, MapStart, Reply,
    ResultsRefusal,
};
use zephium_core::home::{MapId, SearchId, SearchKind};
use zephium_core::ids::{ProfileId, WindowId};
use zephium_core::search::SearchEngine;
use zephium_map::query::Confidence;
use zephium_map::session::Failure;
use zephium_map::QueryKind;
use zephium_map_driver::wire::{self, MapChange, MapOpenRefusal, MapStartRefusal, MapTarget};
use zephium_map_driver::{
    awaits_results, Driver, Io, MapRequest, MemoryMemo, PublicIo, Reading, ResultsPage,
    ResultsSource, SystemClock,
};

use super::*;

mod dto;
#[cfg(test)]
mod tests;

pub(crate) use dto::*;

/// Maps a window keeps running at once; a further search loads its results
/// page as before.
pub(crate) const MAX_LIVE_MAPS_PER_WINDOW: usize = 4;
/// Longest a map waits for its results page, re-reads included.
pub(crate) const RESULTS_READ_TIMEOUT: Duration = Duration::from_secs(12);
/// Wait between reads of a results page that has not drawn its results yet.
pub(crate) const RESULTS_REREAD_INTERVAL: Duration = Duration::from_millis(750);
/// Reads after the first of a page that has not drawn its results yet.
pub(crate) const MAX_RESULTS_REREADS: u32 = 8;
/// Longest a snapshot waits for the shell's records.
const RECORDS_TIMEOUT: Duration = Duration::from_secs(4);
const SLOTS: u8 = 2;

pub(crate) const EVENT_HOME_MAP_CHANGED: &str = "zephium:home-map-changed";
pub(crate) const EVENT_HOME_CHANGED: &str = "zephium:home-changed";
pub(crate) const EVENT_HOME_SHOW: &str = "zephium:home-show";

/// The latest change of each kind a map sent, newest sequence last.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Folded {
    seq: Option<u32>,
    first: Option<MapChange>,
    slots: [Option<MapChange>; SLOTS as usize],
    compare: Option<MapChange>,
    end: Option<MapChange>,
}

impl Folded {
    /// Keeps `change` when it is newer than everything folded so far.
    pub(crate) fn fold(&mut self, change: &MapChange) -> bool {
        let seq = change.seq();
        if self.seq.is_some_and(|held| seq <= held) {
            return false;
        }
        self.seq = Some(seq);
        match change {
            MapChange::FirstLayer { .. } => self.first = Some(change.clone()),
            MapChange::Place { slot, .. } | MapChange::PlaceFailed { slot, .. } => {
                if let Some(held) = self.slots.get_mut(usize::from(*slot)) {
                    *held = Some(change.clone());
                }
            }
            MapChange::Compare { .. } => self.compare = Some(change.clone()),
            MapChange::End { .. } => self.end = Some(change.clone()),
            MapChange::Opened { .. } => {}
        }
        true
    }

    pub(crate) fn ended(&self) -> bool {
        self.end.is_some()
    }

    /// The folded changes in the order they were sent, to replay.
    pub(crate) fn changes(&self) -> Vec<MapChange> {
        let mut changes: Vec<MapChange> = self
            .first
            .iter()
            .chain(self.slots.iter().flatten())
            .chain(self.compare.iter())
            .chain(self.end.iter())
            .cloned()
            .collect();
        changes.sort_by_key(MapChange::seq);
        changes
    }

    pub(crate) fn seq(&self) -> Option<u32> {
        self.seq
    }
}

/// Where each page a map offers leads, resolved once as it is shown.
#[derive(Default)]
pub(crate) struct Targets(Vec<(u8, MapTarget, tauri::Url)>);

impl Targets {
    pub(crate) fn resolve(
        &mut self,
        change: &MapChange,
        lookup: impl Fn(u8, &MapTarget) -> Option<tauri::Url>,
    ) {
        match change {
            MapChange::FirstLayer { also, .. } => {
                self.0
                    .retain(|(_, target, _)| !matches!(target, MapTarget::Also { .. }));
                for index in 0..also.len().min(wire::MAX_ALSO) {
                    let target = MapTarget::Also {
                        index: u8::try_from(index).unwrap_or(u8::MAX),
                    };
                    if let Some(url) = lookup(0, &target) {
                        self.0.push((0, target, url));
                    }
                }
            }
            MapChange::Place { slot, place, .. } => {
                let slot = *slot;
                self.clear_slot(slot);
                let mut targets = vec![MapTarget::Site];
                targets.extend(
                    place
                        .social
                        .iter()
                        .map(|network| MapTarget::Social { network: *network }),
                );
                let mut pending: Vec<&wire::MapEntry> = place.sections.iter().collect();
                while let Some(entry) = pending.pop() {
                    targets.push(MapTarget::Entry {
                        id: entry.id.clone(),
                    });
                    pending.extend(entry.children.iter());
                }
                for target in targets {
                    if let Some(url) = lookup(slot, &target) {
                        self.0.push((slot, target, url));
                    }
                }
            }
            MapChange::PlaceFailed { slot, .. } => self.clear_slot(*slot),
            MapChange::Compare { .. } | MapChange::End { .. } | MapChange::Opened { .. } => {}
        }
    }

    fn clear_slot(&mut self, slot: u8) {
        self.0
            .retain(|(held, target, _)| *held != slot || matches!(target, MapTarget::Also { .. }));
    }

    pub(crate) fn get(&self, slot: u8, target: &MapTarget) -> Option<tauri::Url> {
        self.0
            .iter()
            .find(|(held, candidate, _)| {
                candidate == target && (matches!(target, MapTarget::Also { .. }) || *held == slot)
            })
            .map(|(_, _, url)| url.clone())
    }
}

/// The row a tab opened from `target` is kept under.
pub(crate) fn entry_of(target: &MapTarget) -> String {
    match target {
        MapTarget::Entry { id } => id.clone(),
        MapTarget::Site => "site".into(),
        MapTarget::Social { network } => serde_json::to_value(network)
            .ok()
            .and_then(|value| value.as_str().map(|name| format!("social-{name}")))
            .unwrap_or_else(|| "social".into()),
        MapTarget::Also { index } => format!("also-{index}"),
    }
}

struct LiveMap {
    search: SearchId,
    window: WindowId,
    profile: ProfileId,
    driver: Option<Driver>,
    folded: Folded,
    targets: Targets,
}

pub(crate) type Dispatch = Arc<dyn Fn(Command) -> bool + Send + Sync>;
pub(crate) type Emit = Arc<dyn Fn(&str, &HomeMapChangedV1) + Send + Sync>;

/// The maps of every window, the memo of every profile and the reader their
/// pages are fetched through.
pub(crate) struct Host {
    dispatch: Dispatch,
    emit: Emit,
    runtime: tokio::runtime::Handle,
    reader: Option<zephium_public_fetch::PublicReader>,
    maps: Mutex<HashMap<MapId, LiveMap>>,
    memos: Mutex<HashMap<ProfileId, Arc<MemoryMemo>>>,
}

impl Host {
    pub(crate) fn new(
        dispatch: Dispatch,
        emit: Emit,
        runtime: tokio::runtime::Handle,
        reader: Option<zephium_public_fetch::PublicReader>,
    ) -> Arc<Self> {
        Arc::new(Self {
            dispatch,
            emit,
            runtime,
            reader,
            maps: Mutex::new(HashMap::new()),
            memos: Mutex::new(HashMap::new()),
        })
    }

    fn maps(&self) -> std::sync::MutexGuard<'_, HashMap<MapId, LiveMap>> {
        self.maps
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn memo(&self, profile: ProfileId) -> Arc<MemoryMemo> {
        self.memos
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .entry(profile)
            .or_default()
            .clone()
    }

    /// Room for one more running map in `window`: a finished map gives up its
    /// driver first, oldest first; its view and pages stay.
    fn make_room(maps: &mut HashMap<MapId, LiveMap>, window: WindowId) -> bool {
        let running = |maps: &HashMap<MapId, LiveMap>| {
            maps.values()
                .filter(|map| map.window == window && map.driver.is_some())
                .count()
        };
        while running(maps) >= MAX_LIVE_MAPS_PER_WINDOW {
            let Some(oldest) = maps
                .iter()
                .filter(|(_, map)| {
                    map.window == window && map.driver.is_some() && map.folded.ended()
                })
                .map(|(id, _)| *id)
                .min_by_key(|id| id.0)
            else {
                return false;
            };
            if let Some(map) = maps.get_mut(&oldest) {
                map.driver = None;
            }
        }
        true
    }

    fn fold(&self, map: MapId, change: MapChange) {
        let event = {
            let mut maps = self.maps();
            let Some(live) = maps.get_mut(&map) else {
                return;
            };
            if !live.folded.fold(&change) && !matches!(change, MapChange::Opened { .. }) {
                return;
            }
            if let Some(driver) = &live.driver {
                live.targets
                    .resolve(&change, |slot, target| driver.target(slot, target));
            }
            HomeMapChangedV1 {
                map: map.to_string(),
                search: live.search.to_string(),
                change,
            }
        };
        (self.emit)(EVENT_HOME_MAP_CHANGED, &event);
    }

    pub(crate) fn target(
        &self,
        search: SearchId,
        slot: u8,
        target: &MapTarget,
    ) -> Option<tauri::Url> {
        self.maps()
            .values()
            .find(|map| map.search == search)
            .and_then(|map| map.targets.get(slot, target))
    }

    pub(crate) fn open_inside(
        &self,
        search: SearchId,
        slot: u8,
        entry: &str,
    ) -> Result<(), HomeInsideRefusalV1> {
        let maps = self.maps();
        let map = maps
            .values()
            .find(|map| map.search == search)
            .ok_or(HomeInsideRefusalV1::MapGone)?;
        let driver = map.driver.as_ref().ok_or(HomeInsideRefusalV1::MapGone)?;
        driver.open(slot, entry).map_err(HomeInsideRefusalV1::from)
    }

    /// The map view beside a search for a snapshot.
    pub(crate) fn view(&self, map: MapId) -> Option<HomeMapV1> {
        let maps = self.maps();
        let live = maps.get(&map)?;
        Some(HomeMapV1 {
            id: map.to_string(),
            seq: live.folded.seq(),
            live: live.driver.is_some(),
            changes: live.folded.changes(),
        })
    }

    fn pump(self: &Arc<Self>, map: MapId, mut changes: tokio::sync::mpsc::Receiver<MapChange>) {
        let host = Arc::downgrade(self);
        self.runtime.spawn(async move {
            while let Some(change) = changes.recv().await {
                let Some(host) = host.upgrade() else {
                    return;
                };
                host.fold(map, change);
            }
        });
    }
}

/// Which searches stay on Home as a map: a place read at high confidence,
/// and every comparison.
pub(crate) fn map_kind(query: &str) -> Result<MapKind, MapRefusal> {
    let read = zephium_map::read_query(query).map_err(|_| MapRefusal::NotAMap)?;
    match read.kind() {
        QueryKind::Place { .. } if read.confidence() == Confidence::High => Ok(MapKind::Place),
        QueryKind::Compare { .. } => Ok(MapKind::Compare),
        _ => Err(MapRefusal::NotAMap),
    }
}

/// The shell's port onto the host.
pub(crate) struct HostPort(pub(crate) Arc<Host>);

impl HomeMaps for HostPort {
    fn start(&self, start: MapStart) -> Result<MapKind, MapRefusal> {
        let kind = map_kind(&start.query)?;
        let Some(reader) = &self.0.reader else {
            return Err(MapRefusal::Unavailable);
        };
        let mut maps = self.0.maps();
        if !Host::make_room(&mut maps, start.window) {
            return Err(MapRefusal::Busy);
        }
        let io = Io {
            results: Arc::new(ShellResults {
                dispatch: self.0.dispatch.clone(),
                map: start.map,
                engine: start.engine,
                query: start.query.clone(),
                timeout: RESULTS_READ_TIMEOUT,
                interval: RESULTS_REREAD_INTERVAL,
            }),
            fetch: Arc::new(PublicIo::new(reader)),
            memo: self.0.memo(start.profile),
            clock: Arc::new(SystemClock::new()),
        };
        let request = map_request(&start);
        let (driver, changes) =
            Driver::start(request, io, &self.0.runtime).map_err(|refusal| match refusal {
                MapStartRefusal::NotAMap => MapRefusal::NotAMap,
                MapStartRefusal::UnsupportedEngine | MapStartRefusal::Invalid => {
                    MapRefusal::Unavailable
                }
            })?;
        maps.insert(
            start.map,
            LiveMap {
                search: start.search,
                window: start.window,
                profile: start.profile,
                driver: Some(driver),
                folded: Folded::default(),
                targets: Targets::default(),
            },
        );
        drop(maps);
        self.0.pump(start.map, changes);
        Ok(kind)
    }

    fn forget(&self, map: MapId) {
        self.0.maps().remove(&map);
    }

    fn forget_profile(&self, profile: ProfileId) {
        self.0.maps().retain(|_, map| map.profile != profile);
        self.0
            .memos
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(&profile);
    }
}

/// What the driver is asked: a private search keeps its private flag, so
/// its map neither reads nor writes the memo.
pub(crate) fn map_request(start: &MapStart) -> MapRequest {
    MapRequest {
        query: start.query.clone(),
        engine: start.engine,
        session_id: start.map.0,
        profile: start.profile.to_string(),
        private: start.private,
    }
}

/// A map's results page, read by the shell from a background tab.
struct ShellResults {
    dispatch: Dispatch,
    map: MapId,
    engine: SearchEngine,
    query: String,
    timeout: Duration,
    interval: Duration,
}

/// Tells the shell to stop waiting when the driver stops first.
struct Abandon {
    dispatch: Dispatch,
    map: MapId,
}

impl Drop for Abandon {
    fn drop(&mut self) {
        let _ = (self.dispatch)(Command::Home(Box::new(HomeCommand::ReadAbandoned {
            map: self.map,
        })));
    }
}

async fn ask(
    dispatch: &Dispatch,
    command: impl FnOnce(zephium_app::home::ResultsReply) -> HomeCommand,
) -> Result<ResultsPage, Failure> {
    let (send, receive) = tokio::sync::oneshot::channel();
    let reply = Reply::new(move |answer| {
        let _ = send.send(answer);
    });
    if !dispatch(Command::Home(Box::new(command(reply)))) {
        return Err(Failure::Unavailable);
    }
    match receive.await {
        Ok(Ok(document)) => Ok(ResultsPage {
            html: document.html.0,
            url: document.url,
            truncated: document.truncated,
        }),
        Ok(Err(ResultsRefusal::Redirected)) => Err(Failure::Consent),
        Ok(Err(ResultsRefusal::Unknown | ResultsRefusal::Unavailable)) | Err(_) => {
            Err(Failure::Unavailable)
        }
    }
}

async fn still_drawing(
    page: ResultsPage,
    engine: SearchEngine,
    query: String,
) -> Result<(ResultsPage, bool), Failure> {
    tokio::task::spawn_blocking(move || {
        let waits = awaits_results(&page, engine, &query);
        (page, waits)
    })
    .await
    .map_err(|_| Failure::Unavailable)
}

impl ShellResults {
    async fn read(
        dispatch: Dispatch,
        map: MapId,
        url: tauri::Url,
        engine: SearchEngine,
        query: String,
        interval: Duration,
    ) -> Result<ResultsPage, Failure> {
        let mut page = ask(&dispatch, |done| HomeCommand::ReadResults {
            map,
            url,
            done,
        })
        .await?;
        for _ in 0..MAX_RESULTS_REREADS {
            let (read, waits) = still_drawing(page, engine, query.clone()).await?;
            if !waits {
                return Ok(read);
            }
            tokio::time::sleep(interval).await;
            page = ask(&dispatch, |done| HomeCommand::ReadAgain { map, done }).await?;
        }
        Ok(page)
    }
}

impl ResultsSource for ShellResults {
    fn load(&self, url: tauri::Url) -> Reading<Result<ResultsPage, Failure>> {
        let (dispatch, map, engine, query) = (
            self.dispatch.clone(),
            self.map,
            self.engine,
            self.query.clone(),
        );
        let (timeout, interval) = (self.timeout, self.interval);
        Box::pin(async move {
            let abandon = Abandon {
                dispatch: dispatch.clone(),
                map,
            };
            let read = Self::read(dispatch, map, url, engine, query, interval);
            let answer = tokio::time::timeout(timeout, read)
                .await
                .unwrap_or(Err(Failure::Timeout));
            drop(abandon);
            answer
        })
    }
}

/// The snapshot of Home for the focused profile and space.
pub(crate) fn snapshot(host: &Host, records: HomeRecords) -> HomeSnapshotV1 {
    let searches = records
        .searches
        .iter()
        .zip(records.pages.iter())
        .map(|(search, pages)| HomeSearchV1 {
            id: search.id.to_string(),
            text: wire::text(&search.text),
            at_ms: search.at_ms,
            kind: match search.kind {
                SearchKind::Place => HomeSearchKindV1::Place,
                SearchKind::Compare => HomeSearchKindV1::Compare,
                SearchKind::Other => HomeSearchKindV1::Other,
                SearchKind::Address => HomeSearchKindV1::Address,
            },
            results_open: search.results.is_some(),
            map: search.map.and_then(|map| host.view(map)),
            pages: pages.iter().map(page_view).collect(),
        })
        .collect();
    HomeSnapshotV1 {
        profile: records.profile.to_string(),
        space: records.space.to_string(),
        searches,
    }
}

fn page_view(page: &zephium_app::home::HomePage) -> HomePageV1 {
    HomePageV1 {
        tab: page.tab.map(|tab| tab.to_string()),
        slot: page.row.as_ref().map(|row| row.slot),
        entry: page.row.as_ref().map(|row| row.entry.clone()),
        open: page.open,
        title: wire::shown(&page.title).unwrap_or_default(),
        host: shown_host(&page.host),
    }
}

/// A host as Home names it: its labels only, without `www.`.
pub(crate) fn shown_host(host: &str) -> String {
    let host = host.trim().trim_end_matches('.').to_ascii_lowercase();
    let host = host.strip_prefix("www.").unwrap_or(&host);
    if host.is_empty()
        || host.len() > zephium_core::home::MAX_PAGE_HOST_BYTES
        || !host
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-'))
    {
        return String::new();
    }
    host.to_owned()
}

/// Only the main window's chrome asks for Home.
const HOME_POLICY: CallerPolicy = CallerPolicy::Main;

fn search_id(raw: &str) -> Option<SearchId> {
    (raw.len() == zephium_core::home::ID_HEX_BYTES)
        .then(|| SearchId::parse(raw))
        .flatten()
}

fn entry_id(raw: &str) -> bool {
    raw.len() == wire::ID_BYTES && raw.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// The search `homeOpenTarget` names, when the request is well formed.
pub(crate) fn target_request(search: &str, slot: u8, target: &MapTarget) -> Option<SearchId> {
    let entry_ok = match target {
        MapTarget::Entry { id } => entry_id(id),
        MapTarget::Also { index } => usize::from(*index) < wire::MAX_ALSO,
        MapTarget::Site | MapTarget::Social { .. } => true,
    };
    (slot < SLOTS && entry_ok)
        .then(|| search_id(search))
        .flatten()
}

/// The search `homeOpenInside` names, or why the request is refused.
pub(crate) fn inside_request(
    search: &str,
    slot: u8,
    entry: &str,
) -> Result<SearchId, HomeInsideRefusalV1> {
    let search = search_id(search).ok_or(HomeInsideRefusalV1::SearchUnknown)?;
    if slot >= SLOTS || !entry_id(entry) {
        return Err(HomeInsideRefusalV1::UnknownEntry);
    }
    Ok(search)
}

/// Starts the host and hands the shell its port. Without a public reader no
/// map starts and every search loads its results page.
pub(crate) fn install(
    app: &tauri::AppHandle,
    shell: &Handle,
    routes: zephium_public_fetch::RouteSource,
) -> bool {
    let reader = Some(zephium_public_fetch::PublicReader::new(
        zephium_public_fetch::ReaderLimits::default(),
        routes,
    ));
    let dispatch_shell = shell.clone();
    let emit_app = app.clone();
    let host = Host::new(
        Arc::new(move |command| dispatch_shell.dispatch(command)),
        Arc::new(move |event, payload| emit_to_privileged(&emit_app, MAIN_LABEL, event, payload)),
        tauri::async_runtime::handle().inner().clone(),
        reader,
    );
    if !app.manage(HomeHost(host.clone())) {
        return false;
    }
    shell.dispatch(Command::Home(Box::new(HomeCommand::Attach(
        HomeMapsAttachment(Arc::new(HostPort(host))),
    ))))
}

pub(crate) struct HomeHost(Arc<Host>);

pub(crate) fn emit_home_changed(app: &tauri::AppHandle, profile: String, space: String) {
    emit_to_privileged(
        app,
        MAIN_LABEL,
        EVENT_HOME_CHANGED,
        &HomeChangedV1 { profile, space },
    );
}

pub(crate) fn emit_home_show(app: &tauri::AppHandle, search: String) {
    emit_to_privileged(app, MAIN_LABEL, EVENT_HOME_SHOW, &HomeShowV1 { search });
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn home_snapshot(
    caller: WebviewWindow,
    app: tauri::AppHandle,
) -> Option<HomeSnapshotV1> {
    if !authorize(&caller, HOME_POLICY, "home_snapshot") || shutdown_started(&app) {
        return None;
    }
    let host = app.try_state::<HomeHost>()?.0.clone();
    let shell = app.state::<Handle>().inner().clone();
    let (send, receive) = tokio::sync::oneshot::channel();
    if !shell.dispatch(Command::Home(Box::new(HomeCommand::Records {
        done: Reply::new(move |records| {
            let _ = send.send(records);
        }),
    }))) {
        return None;
    }
    let records = tokio::time::timeout(RECORDS_TIMEOUT, receive)
        .await
        .ok()?
        .ok()??;
    Some(snapshot(&host, records))
}

#[tauri::command]
#[specta::specta]
pub(crate) fn home_open_target(
    caller: WebviewWindow,
    app: tauri::AppHandle,
    shell: State<'_, Handle>,
    search: String,
    slot: u8,
    target: MapTarget,
) -> zephium_ipc::OperationAdmission {
    if !authorize(&caller, HOME_POLICY, "home_open_target") {
        return rejected_operation();
    }
    let (Some(search), Some(host)) = (
        target_request(&search, slot, &target),
        app.try_state::<HomeHost>(),
    ) else {
        return rejected_operation();
    };
    let url = host.0.target(search, slot, &target);
    dispatch_operation(
        &app,
        &shell,
        Command::Home(Box::new(HomeCommand::OpenPage {
            search,
            slot,
            entry: entry_of(&target),
            url,
        })),
    )
}

#[tauri::command]
#[specta::specta]
pub(crate) fn home_open_inside(
    caller: WebviewWindow,
    app: tauri::AppHandle,
    search: String,
    slot: u8,
    entry: String,
) -> HomeInsideAdmissionV1 {
    if !authorize(&caller, HOME_POLICY, "home_open_inside") {
        return HomeInsideAdmissionV1::refused(HomeInsideRefusalV1::SearchUnknown);
    }
    let search = match inside_request(&search, slot, &entry) {
        Ok(search) => search,
        Err(refusal) => return HomeInsideAdmissionV1::refused(refusal),
    };
    let Some(host) = app.try_state::<HomeHost>() else {
        return HomeInsideAdmissionV1::refused(HomeInsideRefusalV1::MapGone);
    };
    match host.0.open_inside(search, slot, &entry) {
        Ok(()) => HomeInsideAdmissionV1 {
            accepted: true,
            refusal: None,
        },
        Err(refusal) => HomeInsideAdmissionV1::refused(refusal),
    }
}

fn home_intent(
    caller: &WebviewWindow,
    app: &tauri::AppHandle,
    shell: &Handle,
    name: &str,
    search: &str,
    intent: impl FnOnce(SearchId) -> HomeCommand,
) -> zephium_ipc::OperationAdmission {
    if !authorize(caller, HOME_POLICY, name) {
        return rejected_operation();
    }
    let Some(search) = search_id(search) else {
        return rejected_operation();
    };
    dispatch_operation(app, shell, Command::Home(Box::new(intent(search))))
}

#[tauri::command]
#[specta::specta]
pub(crate) fn home_open_results(
    caller: WebviewWindow,
    app: tauri::AppHandle,
    shell: State<'_, Handle>,
    search: String,
) -> zephium_ipc::OperationAdmission {
    home_intent(
        &caller,
        &app,
        &shell,
        "home_open_results",
        &search,
        |search| HomeCommand::OpenResults { search },
    )
}

#[tauri::command]
#[specta::specta]
pub(crate) fn home_focus_search(
    caller: WebviewWindow,
    app: tauri::AppHandle,
    shell: State<'_, Handle>,
    search: String,
) -> zephium_ipc::OperationAdmission {
    home_intent(
        &caller,
        &app,
        &shell,
        "home_focus_search",
        &search,
        |search| HomeCommand::FocusSearch { search },
    )
}

#[tauri::command]
#[specta::specta]
pub(crate) fn home_forget_search(
    caller: WebviewWindow,
    app: tauri::AppHandle,
    shell: State<'_, Handle>,
    search: String,
) -> zephium_ipc::OperationAdmission {
    home_intent(
        &caller,
        &app,
        &shell,
        "home_forget_search",
        &search,
        |search| HomeCommand::ForgetSearch { search },
    )
}

impl From<MapOpenRefusal> for HomeInsideRefusalV1 {
    fn from(refusal: MapOpenRefusal) -> Self {
        match refusal {
            MapOpenRefusal::NoPlace => Self::NoPlace,
            MapOpenRefusal::UnknownEntry => Self::UnknownEntry,
            MapOpenRefusal::NotOpenable => Self::NotOpenable,
            MapOpenRefusal::TooManyWaiting => Self::TooManyWaiting,
            MapOpenRefusal::Cancelled => Self::MapGone,
        }
    }
}
