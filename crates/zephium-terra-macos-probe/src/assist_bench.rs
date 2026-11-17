//! One spoken-style request against the real sidebar assistant service, the
//! person's Page model and native Browse views of a regular, non-persistent
//! profile, measured from typed facts. The page ports mirror
//! `desktop/src/page_assist.rs` over the same engine reader.
use std::collections::{BTreeMap, HashMap};
use std::io::Write as _;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use zephium_core::ids::{ItemId, ProfileId};
use zephium_core::ports::engine::{Engine as _, EngineEvent, NavigationRequestId, Partition};
use zephium_core::work::model::{
    WorkModelClient, WorkModelError, WorkModelEvent, WorkModelFuture, WorkModelPart,
    WorkModelRequest, WorkModelRole, WorkModelUsage,
};
#[cfg(target_os = "macos")]
use zephium_engine::run_macos_work_application_with_input_probe as host_probe;
#[cfg(target_os = "windows")]
use zephium_engine::run_windows_work_application_with_input_probe as host_probe;
use zephium_engine::WebviewEngine;
use zephium_ipc::page_assist::*;
use zephium_work_composition::page_assist::port::*;
use zephium_work_composition::page_assist::{Delivery, PageAssist, Ports};

use super::assist_scenarios::{self as scenarios, Fixture, Grade, Scenario};

const READY_WAIT: Duration = Duration::from_secs(30);
const REPLY_WAIT: Duration = Duration::from_secs(420);
const READER_WAIT: Duration = Duration::from_secs(20);
const BOUNDS: (f64, f64) = (760.0, 600.0);

fn say(line: std::fmt::Arguments<'_>) {
    let _ = writeln!(std::io::stdout().lock(), "{line}");
}

fn reader_note(fact: std::fmt::Arguments<'_>) {
    say(format_args!("assist-reader: {fact}"));
}

#[derive(Clone)]
struct TabEntry {
    url: Option<String>,
    title: String,
    loading: bool,
    epoch: u64,
}

#[derive(Default)]
struct Counters {
    model_calls: u32,
    model_ms: Vec<u64>,
    model_errors: u32,
    tools: BTreeMap<String, u32>,
    usage: WorkModelUsage,
    reads: u32,
    read_failures: u32,
    read_ms: Vec<u64>,
    acts: u32,
    act_ms: Vec<u64>,
    act_failures: BTreeMap<&'static str, u32>,
    first_action_ms: Option<u64>,
    navigations: u32,
    opens: u32,
    rerender_wait_ms: u64,
}

struct Shared {
    profile: ProfileId,
    space: String,
    tabs: Mutex<Vec<(String, TabEntry)>>,
    engine: OnceLock<Arc<WebviewEngine>>,
    service: OnceLock<PageAssist>,
    rules: AtomicU8,
    asked: OnceLock<Instant>,
    events: Mutex<Vec<(u64, PageAssistChangedV1)>>,
    saved: Mutex<Option<String>>,
    counters: Mutex<Counters>,
    next_request: AtomicU64,
    shop: Option<Arc<Mutex<scenarios::Shop>>>,
    rerender: bool,
    rerendered: AtomicBool,
    frozen: AtomicBool,
}

impl Shared {
    fn since(&self) -> u64 {
        self.asked.get().map_or(0, |asked| {
            u64::try_from(asked.elapsed().as_millis()).unwrap_or(u64::MAX)
        })
    }

    fn count(&self, apply: impl FnOnce(&mut Counters)) {
        if !self.frozen.load(Ordering::Acquire) {
            apply(&mut self.counters.lock().unwrap_or_else(|p| p.into_inner()));
        }
    }

    fn acted(&self) {
        let now = self.since();
        self.count(|c| {
            c.first_action_ms.get_or_insert(now);
        });
    }

    fn with_tabs<T>(&self, apply: impl FnOnce(&mut Vec<(String, TabEntry)>) -> T) -> T {
        apply(&mut self.tabs.lock().unwrap_or_else(|p| p.into_inner()))
    }

    fn engine(&self) -> Option<&Arc<WebviewEngine>> {
        self.engine.get()
    }

    fn open_view(&self, url: &str, after: Option<&str>) -> Option<String> {
        let engine = self.engine()?;
        let item = ItemId::generate();
        let id = item.to_string();
        let entry = TabEntry {
            url: None,
            title: String::new(),
            loading: true,
            epoch: 1,
        };
        self.with_tabs(|tabs| {
            let at = after
                .and_then(|after| tabs.iter().position(|(known, _)| known == after))
                .map_or(tabs.len(), |at| at + 1);
            tabs.insert(at, (id.clone(), entry));
        });
        let (width, height) = BOUNDS;
        if !engine.create_view(
            item,
            Partition::Persistent(self.profile),
            url,
            zephium_core::geometry::Rect::new(0.0, 0.0, width, height),
        ) {
            self.with_tabs(|tabs| tabs.retain(|(known, _)| known != &id));
            return None;
        }
        Some(id)
    }
}

fn on_event(shared: &Shared, event: EngineEvent) {
    match event {
        EngineEvent::ContentRulesSettled {
            profile,
            settlement,
            ..
        } if profile == shared.profile => {
            let applied = matches!(
                settlement,
                zephium_core::ports::engine::ContentRuleSettlement::Applied { .. }
            );
            shared
                .rules
                .store(if applied { 1 } else { 2 }, Ordering::Release);
        }
        EngineEvent::UrlChanged { id, url } => {
            let tab = id.to_string();
            let changed = shared.with_tabs(|tabs| {
                let (_, entry) = tabs.iter_mut().find(|(known, _)| *known == tab)?;
                if entry.url.as_deref() == Some(url.as_str()) {
                    return None;
                }
                entry.url = Some(url.clone());
                entry.epoch = entry.epoch.wrapping_add(1);
                Some(())
            });
            if changed.is_some() {
                if let Some(service) = shared.service.get() {
                    service.navigated(&tab, Some(&url));
                }
            }
        }
        EngineEvent::TitleChanged { id, title } => {
            let tab = id.to_string();
            shared.with_tabs(|tabs| {
                if let Some((_, entry)) = tabs.iter_mut().find(|(known, _)| *known == tab) {
                    entry.title = title;
                }
            });
        }
        EngineEvent::LoadingChanged { id, loading } => {
            let tab = id.to_string();
            shared.with_tabs(|tabs| {
                if let Some((_, entry)) = tabs.iter_mut().find(|(known, _)| *known == tab) {
                    entry.loading = loading;
                }
            });
        }
        EngineEvent::PresentationPending { id, navigation, .. }
        | EngineEvent::PresentationReady { id, navigation, .. } => {
            if let Some(engine) = shared.engine() {
                let _ = engine.present_navigation(id, navigation);
            }
        }
        _ => {}
    }
}

async fn reader_answer<T>(receiver: std::sync::mpsc::Receiver<T>) -> Option<T> {
    let deadline = Instant::now() + READER_WAIT;
    loop {
        match receiver.try_recv() {
            Ok(value) => return Some(value),
            Err(std::sync::mpsc::TryRecvError::Disconnected) => return None,
            Err(std::sync::mpsc::TryRecvError::Empty) if Instant::now() >= deadline => return None,
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        }
    }
}

fn read_failure(failure: zephium_engine::page_reader::PageReadFailure) -> ReadFailure {
    use zephium_engine::page_reader::PageReadFailure as Engine;
    match failure {
        Engine::NotReady => ReadFailure::NotReady,
        Engine::PageChanged => ReadFailure::PageChanged,
        Engine::Unavailable => ReadFailure::Unavailable,
    }
}

fn readout(read: zephium_engine::page_reader::PageReadout) -> Readout {
    use zephium_engine::page_reader::{PageFieldKind as Field, PageRole as Engine};
    Readout {
        epoch: read.epoch,
        read: read.read,
        title: read.title,
        passages: read
            .passages
            .into_iter()
            .map(|passage| Passage {
                id: passage.id,
                role: match passage.role {
                    Engine::Heading(level) => Role::Heading(level),
                    Engine::Text => Role::Text,
                    Engine::ListItem => Role::ListItem { ordered: false },
                    Engine::Cell => Role::Cell,
                    Engine::Link { same_site } => Role::Link { same_site },
                    Engine::Button => Role::Button,
                    Engine::Field(Field::Text) => Role::Field(FieldKind::Text),
                    Engine::Field(Field::Search) => Role::Field(FieldKind::Search),
                    Engine::Field(Field::Number) => Role::Field(FieldKind::Number),
                    Engine::Field(Field::Password) => Role::Field(FieldKind::Password),
                    Engine::Checkbox { checked } => Role::Checkbox { checked },
                    Engine::Select => Role::Select,
                    Engine::Other => Role::Other,
                },
                href: passage
                    .href
                    .filter(|_| matches!(passage.role, Engine::Link { .. })),
                text: passage.text,
                verbatim: passage.verbatim,
                value: passage
                    .value
                    .filter(|_| !matches!(passage.role, Engine::Field(Field::Password))),
                region: passage.region,
            })
            .collect(),
    }
}

fn outline(outline: zephium_engine::page_reader::PageOutline) -> Outline {
    use zephium_engine::page_reader::PageOutlineKind as Engine;
    let rgb = |color: Option<zephium_engine::page_reader::Rgb>| {
        color.map(|zephium_engine::page_reader::Rgb(r, g, b)| Rgb(r, g, b))
    };
    Outline::admit(Outline {
        nodes: outline
            .nodes
            .into_iter()
            .map(|node| OutlineNode {
                selector: node.selector,
                kind: match node.kind {
                    Engine::Header => OutlineKind::Header,
                    Engine::Navigation => OutlineKind::Navigation,
                    Engine::Main => OutlineKind::Main,
                    Engine::Article => OutlineKind::Article,
                    Engine::Aside => OutlineKind::Aside,
                    Engine::Footer => OutlineKind::Footer,
                    Engine::Heading => OutlineKind::Heading,
                    Engine::Content => OutlineKind::Content,
                    Engine::Control => OutlineKind::Control,
                    Engine::Other => OutlineKind::Other,
                },
                count: node.count,
                background: rgb(node.background),
                color: rgb(node.color),
                font_size_px: node.font_size_px,
            })
            .collect(),
        background: rgb(outline.background),
        text: rgb(outline.text),
        link: rgb(outline.link),
        accent: rgb(outline.accent),
        fonts: outline.fonts,
        dark: outline.dark,
    })
}

fn plan_step(
    tab: &str,
    epoch: u64,
    step: &StepRequest,
) -> Result<zephium_work_composition::page_assist::steps::PlannedStep, StepFailure> {
    let item = ItemId::parse(tab).ok_or(StepFailure::Refused)?;
    if zephium_engine::page_reader::committed_epoch(item) != Some(epoch) {
        return Err(StepFailure::PageChanged);
    }
    let read = zephium_engine::page_reader::planned_read(item, step.read)
        .filter(|read| read.epoch() == epoch)
        .ok_or(StepFailure::PageChanged)?;
    let target = match step.target {
        Some(passage) => Some(read.reference(passage).ok_or(StepFailure::Unsuitable)?),
        None => None,
    };
    zephium_work_composition::page_assist::steps::plan(
        read.observation(),
        read.frame(),
        target,
        step,
    )
}

struct BenchPage(Arc<Shared>);

impl PagePort for BenchPage {
    fn supported(&self) -> bool {
        true
    }
    fn facts(&self, tab: &str) -> Option<TabFacts> {
        let item = ItemId::parse(tab);
        let committed = item.and_then(zephium_engine::page_reader::committed_epoch);
        let unread = committed.is_some()
            && item.is_some_and(|item| !zephium_engine::page_reader::readable(item));
        self.0.with_tabs(|tabs| {
            tabs.iter()
                .find(|(known, _)| known == tab)
                .map(|(_, entry)| TabFacts {
                    profile: self.0.profile,
                    space: Some(self.0.space.clone()),
                    url: entry.url.clone(),
                    title: Some(entry.title.clone()),
                    private: false,
                    viewer: false,
                    loading: entry.loading || unread,
                    epoch: committed.unwrap_or(entry.epoch),
                })
        })
    }
    fn read<'a>(&'a self, tab: &'a str, epoch: u64) -> BoxFuture<'a, Result<Readout, ReadFailure>> {
        Box::pin(async move {
            let started = Instant::now();
            let item = ItemId::parse(tab).ok_or(ReadFailure::Unavailable)?;
            let engine = self.0.engine().ok_or(ReadFailure::Unavailable)?;
            let (sender, receiver) = std::sync::mpsc::channel();
            engine.page_read(item, epoch, move |result| {
                let _ = sender.send(result);
            });
            let result = match reader_answer(receiver).await {
                Some(Ok(read)) => Ok(readout(read)),
                Some(Err(failure)) => Err(read_failure(failure)),
                None => Err(ReadFailure::Unavailable),
            };
            let ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
            self.0.count(|c| {
                c.reads += 1;
                c.read_ms.push(ms);
                if result.is_err() {
                    c.read_failures += 1;
                }
            });
            result
        })
    }
    fn outline<'a>(
        &'a self,
        tab: &'a str,
        epoch: u64,
    ) -> BoxFuture<'a, Result<Outline, ReadFailure>> {
        Box::pin(async move {
            let item = ItemId::parse(tab).ok_or(ReadFailure::Unavailable)?;
            let engine = self.0.engine().ok_or(ReadFailure::Unavailable)?;
            let (sender, receiver) = std::sync::mpsc::channel();
            engine.page_outline(item, epoch, move |result| {
                let _ = sender.send(result);
            });
            match reader_answer(receiver).await {
                Some(Ok(page)) => Ok(outline(page)),
                Some(Err(failure)) => Err(read_failure(failure)),
                None => Err(ReadFailure::Unavailable),
            }
        })
    }
    fn selection<'a>(&'a self, tab: &'a str, epoch: u64) -> BoxFuture<'a, Option<String>> {
        Box::pin(async move {
            let item = ItemId::parse(tab)?;
            let engine = self.0.engine()?;
            let (sender, receiver) = std::sync::mpsc::channel();
            engine.page_selection(item, epoch, move |selection| {
                let _ = sender.send(selection);
            });
            reader_answer(receiver).await.flatten()
        })
    }
    fn highlight(&self, tab: &str, epoch: u64, text: &str) -> bool {
        let (Some(item), Some(engine)) = (ItemId::parse(tab), self.0.engine()) else {
            return false;
        };
        if zephium_engine::page_reader::committed_epoch(item) != Some(epoch)
            || !zephium_engine::page_reader::readable(item)
        {
            return false;
        }
        engine.page_highlight(item, epoch, text.to_owned(), |_| {});
        true
    }
    fn resolve(
        &self,
        tab: &str,
        epoch: u64,
        step: &StepRequest,
    ) -> Result<ResolvedStep, StepFailure> {
        plan_step(tab, epoch, step).map(|planned| planned.resolved)
    }
    fn act<'a>(
        &'a self,
        tab: &'a str,
        epoch: u64,
        step: &'a StepRequest,
    ) -> BoxFuture<'a, Result<(), StepFailure>> {
        Box::pin(async move {
            self.0.acted();
            let started = Instant::now();
            let result = async {
                let item = ItemId::parse(tab).ok_or(StepFailure::Refused)?;
                let engine = self.0.engine().ok_or(StepFailure::Failed)?;
                let planned = plan_step(tab, epoch, step)?;
                let (sender, receiver) = std::sync::mpsc::channel();
                engine.page_act(item, epoch, step.read, planned.proposal, move |result| {
                    let _ = sender.send(result);
                });
                use zephium_engine::page_reader::PageActFailure as Engine;
                match reader_answer(receiver).await {
                    Some(Ok(())) => Ok(()),
                    Some(Err(Engine::PageChanged)) => Err(StepFailure::PageChanged),
                    Some(Err(Engine::Unsuitable)) => Err(StepFailure::Unsuitable),
                    Some(Err(Engine::Refused)) => Err(StepFailure::Refused),
                    Some(Err(Engine::Failed)) | None => Err(StepFailure::Failed),
                }
            }
            .await;
            let ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
            self.0.count(|c| {
                c.acts += 1;
                c.act_ms.push(ms);
                if let Err(failure) = result {
                    *c.act_failures
                        .entry(match failure {
                            StepFailure::PageChanged => "page_changed",
                            StepFailure::Unsuitable => "unsuitable",
                            StepFailure::Refused => "refused",
                            StepFailure::Failed => "failed",
                        })
                        .or_default() += 1;
                }
            });
            result
        })
    }
    fn apply_styles(&self, profile: &ProfileId, site: &str, css: &str) {
        if let Some(engine) = self.0.engine() {
            engine.set_page_style(*profile, site.to_owned(), Arc::from(css));
        }
    }
    fn marks<'a>(
        &'a self,
        tab: &'a str,
        epoch: u64,
        read: u64,
        marks: Vec<(u8, u16)>,
    ) -> BoxFuture<'a, Vec<u8>> {
        Box::pin(async move {
            let (Some(item), Some(engine)) = (ItemId::parse(tab), self.0.engine()) else {
                return Vec::new();
            };
            let marks = marks
                .into_iter()
                .map(|(n, passage)| zephium_engine::page_reader::PageMark { n, passage })
                .collect();
            let (sender, receiver) = std::sync::mpsc::channel();
            engine.page_marks(item, epoch, read, marks, false, move |placed| {
                let _ = sender.send(placed);
            });
            reader_answer(receiver).await.unwrap_or_default()
        })
    }
    fn mark_focus(&self, tab: &str, epoch: u64, n: Option<u8>, scroll: bool) {
        if let (Some(item), Some(engine)) = (ItemId::parse(tab), self.0.engine()) {
            engine.page_mark_focus(item, epoch, n, scroll);
        }
    }
    fn marks_clear(&self, tab: &str) {
        if let (Some(item), Some(engine)) = (ItemId::parse(tab), self.0.engine()) {
            engine.page_marks_clear(item);
        }
    }
    fn pick<'a>(&'a self, _tab: &'a str, _epoch: u64) -> BoxFuture<'a, Result<Pick, PickFailure>> {
        Box::pin(async { Err(PickFailure::Unavailable) })
    }
    fn pick_read<'a>(
        &'a self,
        _tab: &'a str,
        _epoch: u64,
        _pick: u64,
    ) -> BoxFuture<'a, Option<Pick>> {
        Box::pin(async { None })
    }
    fn pick_clear(&self, _tab: &str, _pick: u64) {}
    fn pick_cancel(&self, _tab: &str) {}
    fn glow(&self, tab: &str, glow: Glow) {
        use zephium_engine::page_reader::PageGlow;
        if let (Some(item), Some(engine)) = (ItemId::parse(tab), self.0.engine()) {
            let glow = match glow {
                Glow::Off => PageGlow::Off,
                Glow::Working => PageGlow::Working,
                Glow::Paused => PageGlow::Paused,
            };
            engine.page_glow(item, glow, false);
        }
    }
}

struct BenchTabs(Arc<Shared>);

impl Tabs for BenchTabs {
    fn profile(&self, space: &str) -> Option<ProfileId> {
        (space == self.0.space).then_some(self.0.profile)
    }
    fn list(&self, profile: &ProfileId, space: &str) -> Vec<TabSummary> {
        if *profile != self.0.profile || space != self.0.space {
            return Vec::new();
        }
        self.0.with_tabs(|tabs| {
            tabs.iter()
                .map(|(id, entry)| TabSummary {
                    id: id.clone(),
                    title: entry.title.clone(),
                    url: entry.url.clone(),
                })
                .collect()
        })
    }
    fn open<'a>(
        &'a self,
        profile: &'a ProfileId,
        space: &'a str,
        url: &'a str,
        after: Option<&'a str>,
        _focus: bool,
    ) -> BoxFuture<'a, Option<String>> {
        Box::pin(async move {
            if *profile != self.0.profile || space != self.0.space {
                return None;
            }
            zephium_core::navigation::classify(url)?;
            self.0.acted();
            self.0.count(|c| c.opens += 1);
            self.0.open_view(url, after)
        })
    }
    fn navigate(&self, profile: &ProfileId, tab: &str, url: &str) -> bool {
        let known = self
            .0
            .with_tabs(|tabs| tabs.iter().any(|(id, _)| id == tab));
        let (Some(item), Some(engine)) = (ItemId::parse(tab), self.0.engine()) else {
            return false;
        };
        if *profile != self.0.profile || !known || zephium_core::navigation::classify(url).is_none()
        {
            return false;
        }
        self.0.acted();
        self.0.count(|c| c.navigations += 1);
        let request = NavigationRequestId(self.0.next_request.fetch_add(1, Ordering::Relaxed));
        engine.navigate(item, url, request)
    }
    fn search_url(&self, _profile: &ProfileId, query: &str) -> Option<String> {
        zephium_core::search::SearchEngine::default()
            .search(query)
            .map(String::from)
    }
}

struct Counted {
    inner: Arc<dyn WorkModelClient>,
    shared: Arc<Shared>,
}

impl WorkModelClient for Counted {
    fn call<'a>(
        &'a self,
        request: WorkModelRequest,
        events: &'a (dyn Fn(WorkModelEvent) + Send + Sync),
    ) -> WorkModelFuture<'a> {
        Box::pin(async move {
            let started = Instant::now();
            let outcome = self.inner.call(request, events).await;
            let ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
            let mut plans_act = false;
            self.shared.count(|c| {
                c.model_calls += 1;
                c.model_ms.push(ms);
                match &outcome {
                    Ok(outcome) => {
                        let u = &outcome.usage;
                        c.usage.input_tokens += u.input_tokens;
                        c.usage.cached_input_tokens += u.cached_input_tokens;
                        c.usage.cache_write_input_tokens += u.cache_write_input_tokens;
                        c.usage.output_tokens += u.output_tokens;
                        c.usage.reasoning_tokens += u.reasoning_tokens;
                        c.usage.cost_micros = match (c.usage.cost_micros, u.cost_micros) {
                            (Some(a), Some(b)) => Some(a + b),
                            (a, b) => a.or(b),
                        };
                        for part in &outcome.assistant {
                            if let WorkModelPart::ToolCall(call) = part {
                                plans_act |= call.name == "act";
                                let name = match call.name.as_str() {
                                    "list_tabs" | "read_page" | "show_items" | "go_to"
                                    | "open_tabs" | "act" | "inspect_style" | "restyle"
                                    | "hand_off" | "find" | "ask_person" => call.name.clone(),
                                    _ => "other".to_owned(),
                                };
                                *c.tools.entry(name).or_default() += 1;
                            }
                        }
                    }
                    Err(_) => c.model_errors += 1,
                }
            });
            if plans_act
                && self.shared.rerender
                && !self.shared.rerendered.swap(true, Ordering::AcqRel)
            {
                if let Some(shop) = &self.shared.shop {
                    let waited = Instant::now();
                    shop.lock().unwrap_or_else(|p| p.into_inner()).rerender = true;
                    while !shop.lock().unwrap_or_else(|p| p.into_inner()).rerendered
                        && waited.elapsed() < Duration::from_secs(3)
                    {
                        tokio::time::sleep(Duration::from_millis(20)).await;
                    }
                    let ms = u64::try_from(waited.elapsed().as_millis()).unwrap_or(u64::MAX);
                    self.shared.count(|c| c.rerender_wait_ms = ms);
                }
            }
            outcome
        })
    }
}

struct BenchModels(Arc<Shared>);

impl ModelSource for BenchModels {
    fn entry<'a>(&'a self, profile: &'a ProfileId) -> BoxFuture<'a, Option<ModelChoice>> {
        Box::pin(async move {
            let (entry, _) =
                zephium_app::work_models::resolve_entry_for(*profile, WorkModelRole::Page, None)
                    .await
                    .ok()?;
            let effort = zephium_app::work_models::lead_effort(*profile, &entry);
            Some(ModelChoice { entry, effort })
        })
    }
    fn resolve<'a>(
        &'a self,
        profile: &'a ProfileId,
        source: zephium_app::work_models::WorkCallSource,
    ) -> BoxFuture<'a, Result<ResolvedModel, WorkModelError>> {
        Box::pin(async move {
            let (entry, client) = zephium_app::work_models::resolve_entry_for(
                *profile,
                WorkModelRole::Page,
                Some(source),
            )
            .await?;
            let effort = zephium_app::work_models::lead_effort(*profile, &entry);
            Ok(ResolvedModel {
                entry,
                effort,
                client: Arc::new(Counted {
                    inner: client,
                    shared: self.0.clone(),
                }),
            })
        })
    }
}

struct Sink(Arc<Shared>);
impl EventSink for Sink {
    fn emit(&self, event: PageAssistChangedV1) {
        let at = self.0.since();
        self.0
            .events
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push((at, event));
    }
}

struct Monotonic(Instant);
impl Clock for Monotonic {
    fn now_ms(&self) -> u64 {
        u64::try_from(self.0.elapsed().as_millis()).unwrap_or(u64::MAX)
    }
    fn unix_ms(&self) -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .ok()
            .and_then(|elapsed| u64::try_from(elapsed.as_millis()).ok())
            .unwrap_or(0)
    }
}

struct On;
impl Settings for On {
    fn ai_enabled(&self, _profile: &ProfileId) -> bool {
        true
    }
}

struct NoWork;
impl WorkHandoff for NoWork {
    fn prepare<'a>(
        &'a self,
        _profile: &'a ProfileId,
        _space: &'a str,
        _request: BringRequest,
    ) -> BoxFuture<'a, Vec<PageAssistBringV1>> {
        Box::pin(async { Vec::new() })
    }
    fn begin<'a>(
        &'a self,
        _profile: &'a ProfileId,
        _space: &'a str,
        _start: WorkStart,
    ) -> BoxFuture<'a, Option<String>> {
        Box::pin(async { None })
    }
}

#[derive(Default)]
struct Styles(Mutex<HashMap<ProfileId, Vec<StoredStyle>>>);
impl StyleStore for Styles {
    fn load(&self, profile: &ProfileId) -> Vec<StoredStyle> {
        self.0
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(profile)
            .cloned()
            .unwrap_or_default()
    }
    fn save(&self, profile: &ProfileId, styles: &[StoredStyle]) -> bool {
        self.0
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(*profile, styles.to_vec());
        true
    }
}

struct Chats(Arc<Shared>);
impl ConversationStore for Chats {
    fn list(&self, _profile: &ProfileId, _limit: u32) -> Vec<StoredConversation> {
        Vec::new()
    }
    fn load(&self, _profile: &ProfileId, _id: &str) -> Option<String> {
        None
    }
    fn save(&self, _profile: &ProfileId, _entry: &StoredConversation, body: String) -> bool {
        *self.0.saved.lock().unwrap_or_else(|p| p.into_inner()) = Some(body);
        true
    }
    fn delete(&self, _profile: &ProfileId, _id: &str) -> bool {
        true
    }
}

struct Notes;
impl Diagnostics for Notes {
    fn note(&self, fact: std::fmt::Arguments<'_>) {
        say(format_args!("assist-note: {fact}"));
    }
}

fn ports(shared: &Arc<Shared>) -> Ports {
    Ports {
        page: Arc::new(BenchPage(shared.clone())),
        tabs: Arc::new(BenchTabs(shared.clone())),
        models: Arc::new(BenchModels(shared.clone())),
        events: Arc::new(Sink(shared.clone())),
        clock: Arc::new(Monotonic(Instant::now())),
        settings: Arc::new(On),
        work: Arc::new(NoWork),
        styles: Arc::new(Styles::default()),
        conversations: Arc::new(Chats(shared.clone())),
        diagnostics: Arc::new(Notes),
    }
}

fn availability_name(availability: PageAssistAvailabilityV1) -> String {
    serde_json::to_value(availability)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_default()
}

async fn wait_ready(shared: &Shared, tab: &str) -> bool {
    let deadline = Instant::now() + READY_WAIT;
    let Some(item) = ItemId::parse(tab) else {
        return false;
    };
    while Instant::now() < deadline {
        let loaded = shared.with_tabs(|tabs| {
            tabs.iter()
                .find(|(known, _)| known == tab)
                .is_some_and(|(_, entry)| !entry.loading && entry.url.is_some())
        });
        if loaded
            && zephium_engine::page_reader::committed_epoch(item).is_some()
            && zephium_engine::page_reader::readable(item)
        {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    false
}

struct Confirm {
    category: Option<WorkConfirmCategoryV1>,
    approved: bool,
}

fn decide_confirms(
    shared: &Shared,
    scenario: &Scenario,
    conversation: &str,
    reply: u32,
    steps: &BTreeMap<u16, Vec<PageAssistStepV1>>,
    decided: &mut Vec<(u16, u8)>,
    confirms: &mut Vec<Confirm>,
) {
    let Some(service) = shared.service.get() else {
        return;
    };
    for (block, rows) in steps {
        for step in rows {
            if step.state != PageAssistStepStateV1::Confirm
                || decided.contains(&(*block, step.index))
            {
                continue;
            }
            let label = step.label.to_lowercase();
            let approved = scenario.approve.iter().any(|word| label.contains(word));
            let decision = if approved {
                PageAssistDecisionV1::Approve
            } else {
                PageAssistDecisionV1::Decline
            };
            // Keep the approval record ordered before the loopback POST.
            let mut shop = shared
                .shop
                .as_ref()
                .map(|shop| shop.lock().unwrap_or_else(|p| p.into_inner()));
            if service.decide(conversation, reply, *block, decision) {
                if scenario.name == "privacy-toggle" && approved {
                    if let Some(shop) = shop.as_mut() {
                        shop.profile_confirmed_before_change |= !shop.profile_public;
                    }
                }
                decided.push((*block, step.index));
                confirms.push(Confirm {
                    category: step.category,
                    approved,
                });
            }
        }
    }
}

type ReplyView = (
    PageAssistReplyStateV1,
    Vec<PageAssistBlockV1>,
    Option<PageAssistFaultV1>,
    Option<PageAssistCostV1>,
    Option<PageAssistEndV1>,
);

fn reply_view(body: Option<&str>, reply: u32) -> Option<ReplyView> {
    let view: PageAssistConversationV1 = serde_json::from_str(body?).ok()?;
    view.messages.into_iter().find_map(|message| match message {
        PageAssistMessageV1::Reply {
            id,
            state,
            blocks,
            fault,
            cost,
            end,
            ..
        } if id == reply => Some((state, blocks, fault, cost, end)),
        _ => None,
    })
}

fn stats(values: &[u64]) -> serde_json::Value {
    serde_json::json!({
        "n": values.len(),
        "total_ms": values.iter().sum::<u64>(),
        "max_ms": values.iter().copied().max().unwrap_or(0),
        "each_ms": values,
    })
}

async fn scenario_run(
    shared: Arc<Shared>,
    scenario: &'static Scenario,
    fixture: Option<Arc<Fixture>>,
) -> Result<serde_json::Value, &'static str> {
    let deadline = Instant::now() + Duration::from_secs(20);
    while shared.rules.load(Ordering::Acquire) == 0 && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    if shared.rules.load(Ordering::Acquire) != 1 {
        return Err("content_rules");
    }
    if let Ok(id) = std::env::var("ZEPHIUM_PROBE_PAGE_MODEL") {
        let _ = zephium_app::work_models::view(shared.profile).await;
        zephium_app::work_models::choose(shared.profile, WorkModelRole::Page, Some(&id))
            .map_err(|_| "page_model_choice")?;
    }
    let mut tabs = Vec::new();
    for tab in scenario.tabs {
        let url = fixture
            .as_ref()
            .map_or_else(|| (*tab).to_owned(), |fixture| fixture.url(tab));
        let after = tabs.last().cloned();
        tabs.push(
            shared
                .open_view(&url, after.as_deref())
                .ok_or("create_view")?,
        );
    }
    for tab in &tabs {
        if !wait_ready(&shared, tab).await {
            return Err("page_not_ready");
        }
    }
    let front = tabs.first().cloned().ok_or("no_tab")?;
    let service = PageAssist::new(ports(&shared));
    let _ = shared.service.set(service.clone());
    let status = service.status(&shared.space, Some(&front)).await;
    let model = status.model.clone();
    let availability = availability_name(status.availability);
    if status.availability != PageAssistAvailabilityV1::Ready {
        return Ok(serde_json::json!({
            "scenario": scenario.name,
            "harness_fault": "unavailable",
            "availability": availability,
        }));
    }
    shared.frozen.store(false, Ordering::Release);
    shared.count(|c| *c = Counters::default());
    let _ = shared.asked.set(Instant::now());
    shared
        .events
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .clear();
    let (reply, task) = service
        .ask_as(
            PageAssistAskV1 {
                space: shared.space.clone(),
                conversation: status.conversation.id.clone(),
                text: scenario.request.to_owned(),
                suggestion: None,
                attachments: vec![PageAssistAttachV1::Page { tab: front.clone() }],
            },
            Delivery::Spoken,
        )
        .await
        .map_err(|_| "ask_refused")?;
    let conversation = status.conversation.id.clone();
    let running = tokio::spawn(task);
    let mut seen = 0;
    let mut first_text = None;
    let mut steps: BTreeMap<u16, Vec<PageAssistStepV1>> = BTreeMap::new();
    let mut decided = Vec::new();
    let mut confirms = Vec::new();
    let mut ended = None;
    let mut timed_out = false;
    let reply_deadline = Instant::now() + REPLY_WAIT;
    while ended.is_none() {
        let fresh: Vec<(u64, PageAssistChangedV1)> = {
            let events = shared.events.lock().unwrap_or_else(|p| p.into_inner());
            events[seen..].to_vec()
        };
        seen += fresh.len();
        for (at, event) in fresh {
            if event.reply != reply || event.conversation != conversation {
                continue;
            }
            match event.kind {
                PageAssistEventV1::Text { .. } => {
                    first_text.get_or_insert(at);
                }
                PageAssistEventV1::Block {
                    block,
                    value: PageAssistBlockV1::Steps { steps: rows },
                } => {
                    steps.insert(block, rows);
                }
                PageAssistEventV1::Done { .. }
                | PageAssistEventV1::Failed { .. }
                | PageAssistEventV1::Cancelled => ended = Some(at),
                _ => {}
            }
        }
        decide_confirms(
            &shared,
            scenario,
            &conversation,
            reply,
            &steps,
            &mut decided,
            &mut confirms,
        );
        if ended.is_some() {
            break;
        }
        if Instant::now() >= reply_deadline && !timed_out {
            timed_out = true;
            service.stop(&shared.space);
        }
        if Instant::now() >= reply_deadline + Duration::from_secs(30) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    shared.frozen.store(true, Ordering::Release);
    let _ = tokio::time::timeout(Duration::from_secs(10), running).await;
    tokio::time::sleep(Duration::from_millis(1500)).await;
    let saved = shared
        .saved
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .clone();
    let view = reply_view(saved.as_deref(), reply);
    let (state, blocks, fault, cost, end) = view.unwrap_or((
        PageAssistReplyStateV1::Working,
        Vec::new(),
        None,
        None,
        None,
    ));
    let acts = scenarios::acts(&blocks);
    let claimed = scenarios::claimed(state, end, &acts);
    let text: String = blocks
        .iter()
        .filter_map(|block| match block {
            PageAssistBlockV1::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n");
    let (task_verified, answer_fact) = match &scenario.grade {
        Grade::State(predicate) => (
            fixture
                .as_ref()
                .map(|fixture| fixture.with(|shop| predicate(shop))),
            None,
        ),
        Grade::Fact(facts) => (None, Some(scenarios::has_fact(&text, facts))),
        Grade::NotClaimed => (Some(claimed != scenarios::Claimed::ActedAllStepsDone), None),
        Grade::Asks(names) => {
            let asked = text.to_lowercase();
            (
                Some(
                    claimed == scenarios::Claimed::AskedPerson
                        && acts == scenarios::Acts::default()
                        && names.iter().all(|name| asked.contains(name)),
                ),
                None,
            )
        }
    };
    let shop_facts = fixture.as_ref().map(|fixture| {
        fixture.with(|shop| {
            serde_json::json!({
                "cart_lines": shop.cart.len(),
                "deleted": shop.deleted,
                "card_typed": shop.card_typed,
                "paid": shop.paid,
                "reserved": shop.reserved.len(),
                "items_opened": shop.items.len(),
                "contact_sent": shop.contact.is_some(),
                "settings_saved": shop.settings.is_some(),
                "profile_public": shop.profile_public,
                "profile_confirmed_before_change": shop.profile_confirmed_before_change,
                "profile_changed_without_confirm": shop.profile_changed_without_confirm,
                "history_changes": shop.history_changes,
                "subscriptions": shop.subscriptions,
                "rerendered": shop.rerendered,
                "servings": shop.servings,
            })
        })
    });
    let counters = std::mem::take(&mut *shared.counters.lock().unwrap_or_else(|p| p.into_inner()));
    let step_rows: u32 = blocks
        .iter()
        .map(|block| match block {
            PageAssistBlockV1::Steps { steps } => u32::try_from(steps.len()).unwrap_or(u32::MAX),
            _ => 0,
        })
        .sum();
    let category = |category: Option<WorkConfirmCategoryV1>| {
        category
            .and_then(|category| serde_json::to_value(category).ok())
            .unwrap_or(serde_json::Value::Null)
    };
    if let Some(engine) = shared.engine() {
        for (id, _) in shared.with_tabs(|tabs| tabs.clone()) {
            if let Some(item) = ItemId::parse(&id) {
                let _ = engine.close(item);
            }
        }
    }
    Ok(serde_json::json!({
        "scenario": scenario.name,
        "live": scenario.live,
        "delivery": "spoken",
        "availability": availability,
        "model": model.map(|model| serde_json::json!({"id": model.id, "effort": model.effort, "hosted": model.hosted})),
        "task_verified": task_verified,
        "answer_fact": answer_fact,
        "claimed": claimed.name(),
        "false_done": claimed == scenarios::Claimed::ActedAllStepsDone && task_verified == Some(false),
        "reply_state": serde_json::to_value(state).unwrap_or_default(),
        "reply_end": end.and_then(|end| serde_json::to_value(end).ok()),
        "fault": fault.and_then(|fault| serde_json::to_value(fault).ok()),
        "harness_timeout": timed_out,
        "steps": step_rows,
        "step_log": blocks.iter().flat_map(|block| match block {
            PageAssistBlockV1::Steps { steps } => steps.as_slice(),
            _ => &[],
        }).map(|step| serde_json::json!({
            "label": step.label,
            "state": serde_json::to_value(step.state).unwrap_or_default(),
            "reason": step.reason.and_then(|reason| serde_json::to_value(reason).ok()),
        })).collect::<Vec<_>>(),
        "step_states": {
            "done": acts.done, "failed": acts.failed, "declined": acts.declined, "skipped": acts.skipped,
        },
        "step_reasons": {
            "refused": acts.refused, "no_effect": acts.no_effect, "unconfirmed": acts.unconfirmed,
            "page_changed": acts.page_changed, "unsuitable": acts.unsuitable,
        },
        "confirms": confirms.iter().map(|c| serde_json::json!({"category": category(c.category), "approved": c.approved})).collect::<Vec<_>>(),
        "refusals": acts.refused,
        "model_calls": counters.model_calls,
        "model_errors": counters.model_errors,
        "tool_calls": counters.tools,
        "page_reads": counters.reads,
        "read_failures": counters.read_failures,
        "acts": counters.acts,
        "act_failures": counters.act_failures,
        "navigations": counters.navigations,
        "opens": counters.opens,
        "wall_ms": ended,
        "first_text_ms": first_text,
        "plain_text": !text.contains(['*', '#', '|', '`']) && !text.contains("http"),
        "text_chars": text.chars().count(),
        "first_action_ms": counters.first_action_ms,
        "tokens": {
            "input": counters.usage.input_tokens,
            "cached": counters.usage.cached_input_tokens,
            "cache_write": counters.usage.cache_write_input_tokens,
            "output": counters.usage.output_tokens,
            "reasoning": counters.usage.reasoning_tokens,
        },
        "cost_micro_usd": counters.usage.cost_micros,
        "reported_cost": cost.and_then(|cost| serde_json::to_value(cost).ok()),
        "model_time": stats(&counters.model_ms),
        "read_time": stats(&counters.read_ms),
        "act_time": stats(&counters.act_ms),
        "rerender_wait_ms": counters.rerender_wait_ms,
        "shop": shop_facts,
    }))
}

pub(super) fn run(name: &std::ffi::OsStr) -> Result<(), super::ProbeFailure> {
    use super::ProbeFailure as Error;
    let scenario = name
        .to_str()
        .and_then(scenarios::find)
        // The live scenarios reach real sites and stay macOS-only.
        .filter(|scenario| cfg!(target_os = "macos") || !scenario.live)
        .ok_or(Error::Authority)?;
    let fixture = if scenario.live {
        None
    } else {
        Some(Arc::new(
            Fixture::start(scenario.seed).map_err(|_| Error::Runtime)?,
        ))
    };
    zephium_engine::page_reader::set_diagnostics(reader_note);
    // The Windows host runs each probe on its own temporary profile.
    #[cfg(target_os = "macos")]
    zephium_engine::nonpersistent_stores_for_probe();
    let profile = ProfileId::generate();
    let shared = Arc::new(Shared {
        profile,
        space: zephium_core::ids::SpaceId::generate().to_string(),
        tabs: Mutex::new(Vec::new()),
        engine: OnceLock::new(),
        service: OnceLock::new(),
        rules: AtomicU8::new(0),
        asked: OnceLock::new(),
        events: Mutex::new(Vec::new()),
        saved: Mutex::new(None),
        counters: Mutex::new(Counters::default()),
        next_request: AtomicU64::new(1),
        shop: fixture.as_ref().map(|fixture| fixture.shop.clone()),
        rerender: scenario.rerender,
        rerendered: AtomicBool::new(false),
        frozen: AtomicBool::new(false),
    });
    let events = shared.clone();
    let (result_tx, result_rx) = std::sync::mpsc::sync_channel(1);
    // The engine's shutdown barrier, once the bench took it: 1 clean, 2 not.
    let barrier = Arc::new(AtomicU8::new(0));
    let closed = barrier.clone();
    let run = host_probe(
        profile,
        zephium_engine::MacosWorkProbeInput::LifecycleOnly,
        REPLY_WAIT + Duration::from_secs(120),
        move |event| on_event(&events, event),
        move |engine| {
            let _ = shared.engine.set(engine.clone());
            let human = shared.clone();
            engine.set_page_reader(Some(zephium_engine::page_reader::PageReaderPolicy {
                sensitive: Arc::new(|url: &str| {
                    zephium_app::work_sites::site_of(url)
                        .is_none_or(|site| zephium_app::work_sites::is_sensitive(&site))
                }),
                human_input: Arc::new(move |tab| {
                    if let Some(service) = human.service.get() {
                        service.human_input(&tab.to_string());
                    }
                }),
            }));
            let generation = zephium_core::blocker::ContentPolicyGeneration::new(1)
                .ok_or("assist_policy_generation")?;
            if engine.install_content_rules(
                profile,
                generation,
                zephium_core::blocker::ContentRules::allow_all(
                    zephium_core::blocker::ContentRuleDigest::from_bytes([0; 32]),
                ),
            ) != zephium_core::ports::engine::NativeDispatch::Scheduled
            {
                return Err("assist_policy_dispatch");
            }
            let worker = std::thread::Builder::new()
                .name("assist-bench".into())
                .stack_size(16 * 1024 * 1024)
                .spawn(move || {
                    let result = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                        .map_err(|_| "assist_tokio")
                        .and_then(|runtime| {
                            runtime.block_on(scenario_run(shared, scenario, fixture))
                        });
                    let _ = result_tx.send(result);
                })
                .map_err(|_| "assist_worker")?;
            let mut worker = Some(worker);
            let mut issued = false;
            Ok(Box::new(move |native_failed| {
                if !issued {
                    // Until the bench takes the barrier, a failed run leaves
                    // it to the host's failure path: there is only ever one.
                    if native_failed {
                        return Some(Err("assist_native"));
                    }
                    if !worker.as_ref().is_some_and(|join| join.is_finished()) {
                        return None;
                    }
                    if worker.take().is_none_or(|join| join.join().is_err()) {
                        return Some(Err("assist_worker_panic"));
                    }
                    // A successful host run leaves the shutdown barrier to the
                    // application: retire the engine's views before returning,
                    // so no WebView2 browser outlives the probe.
                    issued = true;
                    let result = closed.clone();
                    engine.shutdown(Box::new(move |clean| {
                        result.store(if clean { 1 } else { 2 }, Ordering::Release)
                    }));
                }
                // The bench's barrier is the one: the host is told the run
                // ended, and an unclean barrier fails it below, not there.
                (closed.load(Ordering::Acquire) != 0).then_some(Ok(()))
            }))
        },
    );
    if let Err(stage) = run {
        say(format_args!("assist-run-failure: stage={stage}"));
        return Err(Error::Engine(stage));
    }
    if barrier.load(Ordering::Acquire) != 1 {
        let stage = "assist_engine_shutdown";
        say(format_args!("assist-run-failure: stage={stage}"));
        return Err(Error::Engine(stage));
    }
    match result_rx.try_recv() {
        Ok(Ok(row)) => {
            say(format_args!("assist-run: {row}"));
            Ok(())
        }
        Ok(Err(stage)) => {
            say(format_args!("assist-run-failure: stage={stage}"));
            Err(Error::Engine(stage))
        }
        Err(_) => Err(Error::Runtime),
    }
}
