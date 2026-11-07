//! The assistant's commands and its one event. Admission and gating live here; the
//! conversation, the page reads and the model loop belong to the page-assist
//! service in `zephium-work-composition`.
use super::*;
use zephium_ipc::page_assist::*;

/// One change to an assistant conversation.
#[derive(Clone, Debug, Serialize, Deserialize, specta::Type, tauri_specta::Event)]
#[tauri_specta(event_name = "zephium:page-assist-changed")]
pub(crate) struct PageAssistChanged(pub(crate) PageAssistChangedV1);

const VERSION: u16 = 2;
const EVENT: &str = "zephium:page-assist-changed";

fn allowed(caller: &WebviewWindow, command: &'static str) -> bool {
    authorize(caller, CallerPolicy::Main, command)
}

fn unavailable_status(space: String, tab: Option<String>) -> PageAssistStatusV1 {
    PageAssistStatusV1 {
        version: VERSION,
        space,
        tab,
        availability: PageAssistAvailabilityV1::Unavailable,
        page: None,
        model: None,
        suggestions: Vec::new(),
        selection: None,
        can_pick: false,
        styled: false,
        conversation: PageAssistConversationV1 {
            id: String::new(),
            title: None,
            messages: Vec::new(),
            seq: 0,
        },
    }
}

fn no_styles(tab: String) -> PageStylesV1 {
    PageStylesV1 {
        version: VERSION,
        tab,
        site: None,
        styles: Vec::new(),
    }
}

fn no_profile_styles(profile: String) -> PageStylesAllV1 {
    PageStylesAllV1 {
        version: VERSION,
        profile,
        sites: Vec::new(),
    }
}

fn valid_tab(tab: &str) -> bool {
    (1..=64).contains(&tab.len())
        && tab
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

fn valid_id(id: &str) -> bool {
    (1..=64).contains(&id.len())
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'.')
}

#[cfg(feature = "work-product")]
mod product {
    use super::*;
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex, OnceLock};
    use std::time::Instant;
    use zephium_core::ids::ProfileId;
    use zephium_core::ports::engine::Engine as _;
    use zephium_core::ports::store::Store as _;
    use zephium_core::work::model::{WorkModelError, WorkModelRole};
    use zephium_work_composition::page_assist::port::*;
    use zephium_work_composition::page_assist::{PageAssist, Ports};

    static SERVICE: OnceLock<PageAssist> = OnceLock::new();

    pub(super) fn service() -> Option<&'static PageAssist> {
        SERVICE.get()
    }

    #[derive(Clone)]
    struct TabEntry {
        profile: ProfileId,
        /// The Space the tab was last listed in: where "Run in Work" puts
        /// its Work, and which list can tell that the tab closed.
        space: Option<String>,
        private: bool,
        url: Option<String>,
        title: String,
        loading: bool,
        viewer: bool,
        epoch: u64,
    }

    /// Each Space's profile and its tabs in order, as last listed.
    type SpaceTabs = HashMap<String, (ProfileId, Vec<String>)>;
    static SPACES: Mutex<Option<SpaceTabs>> = Mutex::new(None);
    const MAX_SPACES: usize = 256;
    static REDUCE_MOTION: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

    fn with_spaces<T>(apply: impl FnOnce(&mut SpaceTabs) -> T) -> T {
        let mut spaces = SPACES.lock().unwrap_or_else(|p| p.into_inner());
        apply(spaces.get_or_insert_with(HashMap::new))
    }

    static TABS: Mutex<Option<HashMap<String, TabEntry>>> = Mutex::new(None);
    const MAX_TABS: usize = 4096;

    fn with_tabs<T>(apply: impl FnOnce(&mut HashMap<String, TabEntry>) -> T) -> T {
        let mut tabs = TABS.lock().unwrap_or_else(|p| p.into_inner());
        apply(tabs.get_or_insert_with(HashMap::new))
    }

    /// Follows one tab's committed address; a change is a navigation.
    fn follow(
        tabs: &mut HashMap<String, TabEntry>,
        tab: &zephium_ipc::TabView,
        profile: Option<(ProfileId, bool, Option<&str>)>,
    ) -> Option<(String, Option<String>)> {
        let viewer = tab.content != zephium_ipc::TabContentView::Web;
        match tabs.get_mut(&tab.id) {
            Some(entry) => {
                if let Some((profile, private, space)) = profile {
                    entry.profile = profile;
                    entry.private = private;
                    entry.space = space.map(str::to_owned);
                }
                entry.loading = tab.loading;
                entry.viewer = viewer;
                entry.title.clone_from(&tab.title);
                if entry.url != tab.url {
                    entry.url = tab.url.clone();
                    entry.epoch = entry.epoch.wrapping_add(1);
                    return Some((tab.id.clone(), tab.url.clone()));
                }
                None
            }
            None => {
                let (profile, private, space) = profile?;
                if tabs.len() >= MAX_TABS {
                    tabs.clear();
                }
                tabs.insert(
                    tab.id.clone(),
                    TabEntry {
                        profile,
                        space: space.map(str::to_owned),
                        private,
                        url: tab.url.clone(),
                        title: tab.title.clone(),
                        loading: tab.loading,
                        viewer,
                        epoch: 1,
                    },
                );
                None
            }
        }
    }

    fn navigated(changes: Vec<(String, Option<String>)>) {
        let Some(service) = service() else {
            return;
        };
        for (tab, url) in changes {
            service.navigated(&tab, url.as_deref());
        }
    }

    pub(crate) fn observe_items(state: &zephium_ipc::ItemsState) {
        let Some((profile, private)) = state.profile.as_ref().and_then(|profile| {
            ProfileId::parse(&profile.id)
                .map(|id| (id, profile.kind == zephium_ipc::ProfileKindView::Incognito))
        }) else {
            return;
        };
        let space = state.active_space_id.as_deref();
        if let Some(space) = space {
            with_spaces(|spaces| {
                if spaces.len() >= MAX_SPACES && !spaces.contains_key(space) {
                    spaces.clear();
                }
                spaces.insert(
                    space.to_owned(),
                    (
                        profile,
                        state.tabs.iter().map(|tab| tab.id.clone()).collect(),
                    ),
                );
            });
        }
        let (changes, closed) = with_tabs(|tabs| {
            let changes: Vec<_> = state
                .tabs
                .iter()
                .filter_map(|tab| follow(tabs, tab, Some((profile, private, space))))
                .collect();
            let closed = closed_tabs(tabs, state, profile, space);
            for tab in &closed {
                tabs.remove(tab);
            }
            (changes, closed)
        });
        navigated(changes);
        if let Some(service) = service() {
            for tab in closed {
                service.tab_closed(&tab);
            }
        }
    }

    /// Tabs this profile and Space listed before and no longer list: closed.
    /// A tab in another Space is never judged from this Space's list.
    fn closed_tabs(
        tabs: &HashMap<String, TabEntry>,
        state: &zephium_ipc::ItemsState,
        profile: ProfileId,
        space: Option<&str>,
    ) -> Vec<String> {
        let Some(space) = space else {
            return Vec::new();
        };
        tabs.iter()
            .filter(|(id, entry)| {
                entry.profile == profile
                    && entry.space.as_deref() == Some(space)
                    && !state.tabs.iter().any(|tab| &tab.id == *id)
            })
            .map(|(id, _)| id.clone())
            .collect()
    }

    pub(crate) fn observe_tab(tab: &zephium_ipc::TabView) {
        let change = with_tabs(|tabs| follow(tabs, tab, None));
        navigated(change.into_iter().collect());
    }

    pub(crate) fn preference_changed(key: &str, value: &str) {
        if key == "ui.reduce-motion" {
            REDUCE_MOTION.store(value == "true", std::sync::atomic::Ordering::Relaxed);
        }
        if key == "ai.enabled" {
            reader_switch(value != "false");
            if value == "false" {
                if let Some(service) = service() {
                    service.end_all();
                }
            }
        }
    }

    static ENGINE: OnceLock<Arc<zephium_engine::WebviewEngine>> = OnceLock::new();

    /// Turns the Browse reader on or off in every view. It never runs on a
    /// bank, payment, health or government site.
    fn reader_switch(on: bool) {
        let Some(engine) = ENGINE.get() else {
            return;
        };
        engine.set_page_reader(on.then(|| zephium_engine::page_reader::PageReaderPolicy {
            sensitive: Arc::new(|url: &str| {
                zephium_app::work_sites::site_of(url)
                    .is_none_or(|site| zephium_app::work_sites::is_sensitive(&site))
            }),
            human_input: Arc::new(|tab| {
                if let Some(service) = service() {
                    service.human_input(&tab.to_string());
                }
            }),
        }));
    }

    /// Longest wait for one answer from the page; the reader's own
    /// deadlines are shorter, so this only covers a lost engine.
    const READER_WAIT: std::time::Duration = std::time::Duration::from_secs(20);

    async fn reader_answer<T>(receiver: tokio::sync::oneshot::Receiver<T>) -> Option<T> {
        tokio::time::timeout(READER_WAIT, receiver).await.ok()?.ok()
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

    /// Judges a step against the exact read it was planned on, which must
    /// still be kept and belong to the tab's committed page.
    fn plan_step(
        tab: &str,
        epoch: u64,
        step: &StepRequest,
    ) -> Result<zephium_work_composition::page_assist::steps::PlannedStep, StepFailure> {
        let item = zephium_core::ids::ItemId::parse(tab).ok_or(StepFailure::Refused)?;
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

    /// The live tab, read through the engine's Browse reader.
    struct DesktopPage {
        engine: Arc<zephium_engine::WebviewEngine>,
    }

    impl PagePort for DesktopPage {
        fn supported(&self) -> bool {
            cfg!(any(target_os = "macos", target_os = "windows"))
        }
        fn facts(&self, tab: &str) -> Option<TabFacts> {
            let item = zephium_core::ids::ItemId::parse(tab);
            let committed = item.and_then(zephium_engine::page_reader::committed_epoch);
            // A document that predates the reader (it was switched on after
            // the page loaded) reads as loading until its next navigation.
            let unread = committed.is_some()
                && item.is_some_and(|item| !zephium_engine::page_reader::readable(item));
            with_tabs(|tabs| {
                tabs.get(tab).map(|entry| TabFacts {
                    profile: entry.profile,
                    space: entry.space.clone(),
                    url: entry.url.clone(),
                    title: Some(entry.title.clone()),
                    private: entry.private,
                    viewer: entry.viewer,
                    loading: entry.loading || unread,
                    epoch: committed.unwrap_or(entry.epoch),
                })
            })
        }
        fn read<'a>(
            &'a self,
            tab: &'a str,
            epoch: u64,
        ) -> BoxFuture<'a, Result<Readout, ReadFailure>> {
            Box::pin(async move {
                let item = zephium_core::ids::ItemId::parse(tab).ok_or(ReadFailure::Unavailable)?;
                let (sender, receiver) = tokio::sync::oneshot::channel();
                self.engine.page_read(item, epoch, move |result| {
                    let _ = sender.send(result);
                });
                match reader_answer(receiver).await {
                    Some(Ok(read)) => Ok(readout(read)),
                    Some(Err(failure)) => Err(read_failure(failure)),
                    None => Err(ReadFailure::Unavailable),
                }
            })
        }
        fn outline<'a>(
            &'a self,
            tab: &'a str,
            epoch: u64,
        ) -> BoxFuture<'a, Result<Outline, ReadFailure>> {
            Box::pin(async move {
                let item = zephium_core::ids::ItemId::parse(tab).ok_or(ReadFailure::Unavailable)?;
                let (sender, receiver) = tokio::sync::oneshot::channel();
                self.engine.page_outline(item, epoch, move |result| {
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
                let item = zephium_core::ids::ItemId::parse(tab)?;
                let (sender, receiver) = tokio::sync::oneshot::channel();
                self.engine.page_selection(item, epoch, move |selection| {
                    let _ = sender.send(selection);
                });
                reader_answer(receiver).await.flatten()
            })
        }
        fn highlight(&self, tab: &str, epoch: u64, text: &str) -> bool {
            let Some(item) = zephium_core::ids::ItemId::parse(tab) else {
                return false;
            };
            if zephium_engine::page_reader::committed_epoch(item) != Some(epoch)
                || !zephium_engine::page_reader::readable(item)
            {
                return false;
            }
            self.engine
                .page_highlight(item, epoch, text.to_owned(), |_| {});
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
                let item = zephium_core::ids::ItemId::parse(tab).ok_or(StepFailure::Refused)?;
                let planned = plan_step(tab, epoch, step)?;
                let (sender, receiver) = tokio::sync::oneshot::channel();
                self.engine
                    .page_act(item, epoch, step.read, planned.proposal, move |result| {
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
            })
        }
        fn apply_styles(&self, profile: &ProfileId, site: &str, css: &str) {
            self.engine
                .set_page_style(*profile, site.to_owned(), Arc::from(css));
        }
        fn marks<'a>(
            &'a self,
            tab: &'a str,
            epoch: u64,
            read: u64,
            marks: Vec<(u8, u16)>,
        ) -> BoxFuture<'a, Vec<u8>> {
            Box::pin(async move {
                let Some(item) = zephium_core::ids::ItemId::parse(tab) else {
                    return Vec::new();
                };
                let marks = marks
                    .into_iter()
                    .map(|(n, passage)| zephium_engine::page_reader::PageMark { n, passage })
                    .collect();
                let (sender, receiver) = tokio::sync::oneshot::channel();
                self.engine
                    .page_marks(item, epoch, read, marks, reduce_motion(), move |placed| {
                        let _ = sender.send(placed);
                    });
                reader_answer(receiver).await.unwrap_or_default()
            })
        }
        fn mark_focus(&self, tab: &str, epoch: u64, n: Option<u8>, scroll: bool) {
            if let Some(item) = zephium_core::ids::ItemId::parse(tab) {
                self.engine.page_mark_focus(item, epoch, n, scroll);
            }
        }
        fn marks_clear(&self, tab: &str) {
            if let Some(item) = zephium_core::ids::ItemId::parse(tab) {
                self.engine.page_marks_clear(item);
            }
        }
        fn pick<'a>(
            &'a self,
            tab: &'a str,
            epoch: u64,
        ) -> BoxFuture<'a, Result<Pick, PickFailure>> {
            Box::pin(async move {
                let item = zephium_core::ids::ItemId::parse(tab).ok_or(PickFailure::Unavailable)?;
                let (sender, receiver) = tokio::sync::oneshot::channel();
                self.engine
                    .page_pick(item, epoch, reduce_motion(), move |result| {
                        let _ = sender.send(result);
                    });
                use zephium_engine::page_reader::PagePickFailure as Engine;
                match tokio::time::timeout(PICK_WAIT, receiver).await {
                    Ok(Ok(Ok(pick))) => Ok(picked(pick)),
                    Ok(Ok(Err(Engine::Cancelled))) => Err(PickFailure::Cancelled),
                    Ok(Ok(Err(Engine::PageChanged))) => Err(PickFailure::PageChanged),
                    Ok(Ok(Err(Engine::Unavailable))) | Ok(Err(_)) => Err(PickFailure::Unavailable),
                    Err(_) => {
                        self.engine.page_pick_cancel(item);
                        Err(PickFailure::Cancelled)
                    }
                }
            })
        }
        fn pick_read<'a>(
            &'a self,
            tab: &'a str,
            epoch: u64,
            pick: u64,
        ) -> BoxFuture<'a, Option<Pick>> {
            Box::pin(async move {
                let item = zephium_core::ids::ItemId::parse(tab)?;
                let (sender, receiver) = tokio::sync::oneshot::channel();
                self.engine.page_pick_read(item, epoch, pick, move |found| {
                    let _ = sender.send(found);
                });
                reader_answer(receiver).await.flatten().map(picked)
            })
        }
        fn pick_clear(&self, tab: &str, pick: u64) {
            if let Some(item) = zephium_core::ids::ItemId::parse(tab) {
                self.engine.page_pick_clear(item, pick);
            }
        }
        fn pick_cancel(&self, tab: &str) {
            if let Some(item) = zephium_core::ids::ItemId::parse(tab) {
                self.engine.page_pick_cancel(item);
            }
        }
        fn glow(&self, tab: &str, glow: Glow) {
            use zephium_engine::page_reader::PageGlow;
            if let Some(item) = zephium_core::ids::ItemId::parse(tab) {
                let glow = match glow {
                    Glow::Off => PageGlow::Off,
                    Glow::Working => PageGlow::Working,
                    Glow::Paused => PageGlow::Paused,
                };
                self.engine.page_glow(item, glow, reduce_motion());
            }
        }
    }

    /// Closed facts about replies, into the app's diagnostic log.
    struct Log;
    impl Diagnostics for Log {
        fn note(&self, fact: std::fmt::Arguments<'_>) {
            super::super::work_provider::record_diagnostic(fact);
        }
    }

    /// Longest a pick may wait for the person; the page ends it sooner when idle.
    const PICK_WAIT: std::time::Duration = std::time::Duration::from_secs(150);

    fn reduce_motion() -> bool {
        REDUCE_MOTION.load(std::sync::atomic::Ordering::Relaxed)
    }

    fn picked(pick: zephium_engine::page_reader::PagePick) -> Pick {
        Pick {
            pick: pick.pick,
            label: pick.label,
            text: pick.text,
            selector: pick.selector,
        }
    }

    struct Models;

    impl ModelSource for Models {
        fn entry<'a>(&'a self, profile: &'a ProfileId) -> BoxFuture<'a, Option<ModelChoice>> {
            Box::pin(async move {
                let (entry, _) = zephium_app::work_models::resolve_entry_for(
                    *profile,
                    WorkModelRole::Page,
                    None,
                )
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
                    client,
                })
            })
        }
    }

    struct Events(tauri::AppHandle);
    impl EventSink for Events {
        fn emit(&self, event: PageAssistChangedV1) {
            emit_to_privileged(&self.0, MAIN_LABEL, EVENT, &event);
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

    struct Switches(tauri::AppHandle);
    impl Settings for Switches {
        fn ai_enabled(&self, _profile: &ProfileId) -> bool {
            self.0
                .try_state::<keymap::Keymap>()
                .is_some_and(|keymap| keymap.ai_enabled())
        }
    }

    /// "Run in Work": a new Work in the tab's Space with the question as its
    /// objective and the tab on its canvas, run through the ordinary Work
    /// operation owner.
    struct DesktopHandoff(tauri::AppHandle);
    /// Longest a shell lookup or a new tab is waited for.
    const LOOKUP_WAIT: std::time::Duration = std::time::Duration::from_secs(4);

    fn home_folder(folder: &str) -> Option<(std::path::PathBuf, String)> {
        let home = std::env::var_os("HOME").map(std::path::PathBuf::from)?;
        let relative = folder
            .strip_prefix("~/")
            .or_else(|| (folder == "~").then_some(""))?;
        if relative.split('/').any(|part| part == "..") {
            return None;
        }
        let path = home.join(relative);
        path.is_dir().then(|| {
            let shown = if relative.is_empty() {
                "~".to_owned()
            } else {
                format!("~/{}", relative.trim_end_matches('/'))
            };
            (path, shown)
        })
    }

    fn repository(name: &str) -> Option<String> {
        let name = name
            .trim()
            .trim_start_matches("https://github.com/")
            .trim_end_matches('/')
            .trim_end_matches(".git");
        let (owner, repo) = name.split_once('/')?;
        let part = |part: &str| {
            (1..=100).contains(&part.len())
                && part
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
        };
        (part(owner) && part(repo) && !repo.contains('/')).then(|| format!("{owner}/{repo}"))
    }

    impl WorkHandoff for DesktopHandoff {
        fn prepare<'a>(
            &'a self,
            profile: &'a ProfileId,
            _space: &'a str,
            request: BringRequest,
        ) -> BoxFuture<'a, Vec<PageAssistBringV1>> {
            Box::pin(async move {
                let mut brings = Vec::new();
                let handle = self.0.state::<zephium_app::Handle>();
                if let Some(words) = request.task.as_deref() {
                    if let Ok(Some(task)) =
                        tokio::time::timeout(LOOKUP_WAIT, handle.find_task(*profile, words)).await
                    {
                        brings.push(PageAssistBringV1 {
                            kind: PageAssistBringKindV1::Task,
                            label: task.title,
                            detail: task.due,
                            tab: None,
                        });
                    }
                }
                if let Some(words) = request.note.as_deref() {
                    if let Ok(Some(note)) =
                        tokio::time::timeout(LOOKUP_WAIT, handle.find_note(*profile, words)).await
                    {
                        brings.push(PageAssistBringV1 {
                            kind: PageAssistBringKindV1::Note,
                            label: note.title,
                            detail: None,
                            tab: None,
                        });
                    }
                }
                for tab in request.tabs.iter().take(6) {
                    let host = with_tabs(|tabs| {
                        tabs.get(tab)
                            .filter(|entry| entry.profile == *profile)
                            .and_then(|entry| entry.url.clone())
                    });
                    if let Some(url) = host {
                        let rest = url
                            .strip_prefix("https://")
                            .or_else(|| url.strip_prefix("http://"))
                            .unwrap_or(&url);
                        let host = rest
                            .split(['/', '?', '#'])
                            .next()
                            .unwrap_or_default()
                            .trim_start_matches("www.")
                            .to_owned();
                        brings.push(PageAssistBringV1 {
                            kind: PageAssistBringKindV1::Tab,
                            label: host,
                            detail: Some("Open tab".to_owned()),
                            tab: Some(tab.clone()),
                        });
                    }
                }
                if let Some(name) = request.repository.as_deref().and_then(repository) {
                    brings.push(PageAssistBringV1 {
                        kind: PageAssistBringKindV1::Repository,
                        label: name,
                        detail: Some("GitHub".to_owned()),
                        tab: None,
                    });
                }
                if let Some((_, shown)) = request.folder.as_deref().and_then(home_folder) {
                    brings.push(PageAssistBringV1 {
                        kind: PageAssistBringKindV1::Folder,
                        label: shown,
                        detail: Some("Local folder".to_owned()),
                        tab: None,
                    });
                }
                brings
            })
        }
        fn begin<'a>(
            &'a self,
            profile: &'a ProfileId,
            space: &'a str,
            start: WorkStart,
        ) -> BoxFuture<'a, Option<String>> {
            Box::pin(async move {
                let tabs: Vec<(String, String)> = start
                    .brings
                    .iter()
                    .filter_map(|bring| bring.tab.clone())
                    .filter_map(|tab| {
                        let url = with_tabs(|tabs| {
                            tabs.get(&tab)
                                .filter(|entry| entry.profile == *profile)
                                .and_then(|entry| entry.url.clone())
                        })?;
                        Some((tab, url))
                    })
                    .collect();
                super::handoff::start(&self.0, *profile, space, &start, &tabs)
                    .await
                    .ok()
            })
        }
    }

    /// The Space's tabs and new background tabs, through the shell.
    struct DesktopTabs(tauri::AppHandle, Arc<zephium_store::SqliteStore>);

    impl Tabs for DesktopTabs {
        fn navigate(&self, profile: &ProfileId, tab: &str, url: &str) -> bool {
            let owned = with_tabs(|tabs| {
                tabs.get(tab)
                    .is_some_and(|entry| entry.profile == *profile && !entry.private)
            });
            let Some(id) = zephium_core::ids::ItemId::parse(tab).filter(|_| owned) else {
                return false;
            };
            zephium_core::navigation::classify(url).is_some()
                && self
                    .0
                    .state::<zephium_app::Handle>()
                    .dispatch(zephium_app::Command::Navigate {
                        id,
                        input: url.to_owned(),
                    })
        }
        fn search_url(&self, _profile: &ProfileId, query: &str) -> Option<String> {
            let engine = self
                .1
                .app_setting("search.engine")
                .as_deref()
                .and_then(zephium_core::search::SearchEngine::from_id)
                .unwrap_or_default();
            let template = self.1.app_setting("search.custom-url").unwrap_or_default();
            engine
                .configured_search(query, &template)
                .or_else(|| zephium_core::search::SearchEngine::default().search(query))
                .map(String::from)
        }
        fn profile(&self, space: &str) -> Option<ProfileId> {
            with_spaces(|spaces| spaces.get(space).map(|(profile, _)| *profile))
        }
        fn list(&self, profile: &ProfileId, space: &str) -> Vec<TabSummary> {
            let order = with_spaces(|spaces| {
                spaces
                    .get(space)
                    .filter(|(owner, _)| owner == profile)
                    .map(|(_, tabs)| tabs.clone())
                    .unwrap_or_default()
            });
            with_tabs(|tabs| {
                order
                    .iter()
                    .filter_map(|id| {
                        let entry = tabs.get(id).filter(|entry| entry.profile == *profile)?;
                        Some(TabSummary {
                            id: id.clone(),
                            title: entry.title.clone(),
                            url: entry.url.clone(),
                        })
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
            focus: bool,
        ) -> BoxFuture<'a, Option<String>> {
            Box::pin(async move {
                let space = zephium_core::ids::SpaceId::parse(space)?;
                let after = after.and_then(zephium_core::ids::ItemId::parse);
                let handle = self.0.state::<zephium_app::Handle>();
                tokio::time::timeout(
                    LOOKUP_WAIT,
                    handle.open_tab(*profile, space, after, url.to_owned(), focus),
                )
                .await
                .ok()
                .flatten()
                .map(|tab| tab.to_string())
            })
        }
    }

    /// Conversations in the profile's store.
    struct Chats(Arc<zephium_store::SqliteStore>);

    impl ConversationStore for Chats {
        fn list(&self, profile: &ProfileId, limit: u32) -> Vec<StoredConversation> {
            self.0
                .chats(*profile, limit)
                .into_iter()
                .map(|chat| StoredConversation {
                    id: chat.id,
                    space: chat.space,
                    title: chat.title,
                    updated_ms: u64::try_from(chat.updated_ms).unwrap_or(0),
                })
                .collect()
        }
        fn load(&self, profile: &ProfileId, id: &str) -> Option<String> {
            self.0.chat(*profile, id)
        }
        fn save(&self, profile: &ProfileId, entry: &StoredConversation, body: String) -> bool {
            self.0.save_chat(
                *profile,
                &entry.id,
                &entry.space,
                &entry.title,
                i64::try_from(entry.updated_ms).unwrap_or(i64::MAX),
                body,
            )
        }
        fn delete(&self, profile: &ProfileId, id: &str) -> bool {
            self.0.delete_chat(*profile, id)
        }
    }

    /// Page styles in app settings, one key per profile and site, so no
    /// value outgrows the settings row.
    struct SettingStyles(Arc<zephium_store::SqliteStore>);
    /// The settings store takes values up to 64 KiB; stay clear of it.
    const MAX_SETTING_BYTES: usize = 60 * 1024;

    #[derive(serde::Serialize, serde::Deserialize)]
    struct Row {
        id: String,
        summary: String,
        css: String,
        enabled: bool,
    }

    impl SettingStyles {
        fn index_key(profile: &ProfileId) -> String {
            format!("page_assist.sites.{profile}")
        }
        fn site_key(profile: &ProfileId, site: &str) -> String {
            format!("page_assist.style.{profile}.{site}")
        }
    }

    impl StyleStore for SettingStyles {
        fn load(&self, profile: &ProfileId) -> Vec<StoredStyle> {
            let sites: Vec<String> = self
                .0
                .app_setting(&Self::index_key(profile))
                .and_then(|text| serde_json::from_str(&text).ok())
                .unwrap_or_default();
            let mut styles = Vec::new();
            for site in sites.into_iter().take(512) {
                let rows: Vec<Row> = self
                    .0
                    .app_setting(&Self::site_key(profile, &site))
                    .and_then(|text| serde_json::from_str(&text).ok())
                    .unwrap_or_default();
                for row in rows {
                    // Stored CSS passes the validator again before it is used.
                    let Ok(valid) =
                        zephium_work_composition::page_assist::core::css::validate(&row.css)
                    else {
                        continue;
                    };
                    styles.push(StoredStyle {
                        id: row.id,
                        site: site.clone(),
                        summary: row.summary,
                        css: valid.css,
                        enabled: row.enabled,
                    });
                }
            }
            styles
        }
        fn save(&self, profile: &ProfileId, styles: &[StoredStyle]) -> bool {
            let old: Vec<String> = self
                .0
                .app_setting(&Self::index_key(profile))
                .and_then(|text| serde_json::from_str(&text).ok())
                .unwrap_or_default();
            let mut sites: Vec<String> = styles.iter().map(|style| style.site.clone()).collect();
            sites.sort();
            sites.dedup();
            let mut rows = Vec::with_capacity(sites.len());
            for site in &sites {
                let site_rows: Vec<Row> = styles
                    .iter()
                    .filter(|style| &style.site == site)
                    .map(|style| Row {
                        id: style.id.clone(),
                        summary: style.summary.clone(),
                        css: style.css.clone(),
                        enabled: style.enabled,
                    })
                    .collect();
                let Ok(text) = serde_json::to_string(&site_rows) else {
                    return false;
                };
                // Refuse before writing anything: no row outgrows a value.
                if text.len() > MAX_SETTING_BYTES {
                    return false;
                }
                rows.push((site, text));
            }
            let Ok(index) = serde_json::to_string(&sites) else {
                return false;
            };
            if index.len() > MAX_SETTING_BYTES {
                return false;
            }
            let mut stored = true;
            for site in old.iter().filter(|site| !sites.contains(site)) {
                stored &= self
                    .0
                    .set_app_setting(Self::site_key(profile, site), "[]".to_owned());
            }
            for (site, text) in rows {
                stored &= self.0.set_app_setting(Self::site_key(profile, site), text);
            }
            stored & self.0.set_app_setting(Self::index_key(profile), index)
        }
    }

    pub(crate) fn install(
        app: &tauri::AppHandle,
        engine: Arc<zephium_engine::WebviewEngine>,
        store: Arc<zephium_store::SqliteStore>,
    ) {
        let _ = ENGINE.set(engine.clone());
        zephium_engine::page_reader::set_diagnostics(super::work_provider::record_diagnostic);
        reader_switch(
            app.try_state::<keymap::Keymap>()
                .is_some_and(|keymap| keymap.ai_enabled()),
        );
        REDUCE_MOTION.store(
            store.app_setting("ui.reduce-motion").as_deref() == Some("true"),
            std::sync::atomic::Ordering::Relaxed,
        );
        let service = PageAssist::new(Ports {
            page: Arc::new(DesktopPage { engine }),
            tabs: Arc::new(DesktopTabs(app.clone(), store.clone())),
            models: Arc::new(Models),
            events: Arc::new(Events(app.clone())),
            clock: Arc::new(Monotonic(Instant::now())),
            settings: Arc::new(Switches(app.clone())),
            work: Arc::new(DesktopHandoff(app.clone())),
            styles: Arc::new(SettingStyles(store.clone())),
            conversations: Arc::new(Chats(store)),
            diagnostics: Arc::new(Log),
        });
        let _ = SERVICE.set(service);
    }

    /// Hosted requests of assistant replies, until their charge is known.
    static HOSTED: Mutex<Vec<(String, String, u32)>> = Mutex::new(Vec::new());

    pub(crate) fn hosted_sourced(request_id: &str, conversation: &str, reply: u32) {
        let mut hosted = HOSTED.lock().unwrap_or_else(|p| p.into_inner());
        if hosted.len() >= 64 {
            hosted.remove(0);
        }
        hosted.push((request_id.to_owned(), conversation.to_owned(), reply));
    }

    /// A hosted request's charge is final: it joins its reply's cost.
    pub(crate) fn hosted_charged(
        request_id: &str,
        charged_units: Option<u64>,
        units_per_credit: Option<u64>,
    ) {
        let entry = {
            let mut hosted = HOSTED.lock().unwrap_or_else(|p| p.into_inner());
            let Some(index) = hosted.iter().position(|(id, ..)| id == request_id) else {
                return;
            };
            hosted.remove(index)
        };
        let (Some(charged), Some(per), Some(service)) =
            (charged_units, units_per_credit, service())
        else {
            return;
        };
        if let Some(credits) =
            zephium_work_composition::page_assist::core::cost::credits(charged, per)
        {
            service.settle_hosted(&entry.1, entry.2, credits);
        }
    }
}

#[cfg(feature = "work-product")]
pub(crate) use product::{
    hosted_charged, hosted_sourced, install, observe_items, observe_tab, preference_changed,
};

#[cfg(not(feature = "work-product"))]
pub(crate) fn observe_items(_state: &zephium_ipc::ItemsState) {}
#[cfg(not(feature = "work-product"))]
pub(crate) fn observe_tab(_tab: &zephium_ipc::TabView) {}
#[cfg(not(feature = "work-product"))]
pub(crate) fn preference_changed(_key: &str, _value: &str) {}

macro_rules! with_service {
    ($none:expr, |$service:ident| $body:expr) => {{
        #[cfg(feature = "work-product")]
        {
            match product::service() {
                Some($service) => $body,
                None => $none,
            }
        }
        #[cfg(not(feature = "work-product"))]
        {
            $none
        }
    }};
}

fn valid_space(space: &str) -> bool {
    valid_tab(space)
}

/// The assistant for a Space with `tab` in front: availability, the page chip and
/// the whole conversation.
#[tauri::command]
#[specta::specta]
pub(crate) async fn page_assist_status(
    caller: WebviewWindow,
    space: String,
    tab: Option<String>,
) -> PageAssistStatusV1 {
    if !allowed(&caller, "page_assist_status")
        || !valid_space(&space)
        || tab.as_deref().is_some_and(|tab| !valid_tab(tab))
    {
        return unavailable_status(space, tab);
    }
    with_service!(unavailable_status(space, tab), |service| service
        .status(&space, tab.as_deref())
        .await)
}

/// Sends a message. Admission only; the reply arrives as events.
#[tauri::command]
#[specta::specta]
pub(crate) async fn page_assist_ask(
    caller: WebviewWindow,
    request: PageAssistAskV1,
) -> PageAssistAskResponseV1 {
    let refused = |fault| PageAssistAskResponseV1 {
        reply: None,
        fault: Some(fault),
    };
    if !allowed(&caller, "page_assist_ask") {
        return refused(PageAssistFaultV1::Unavailable);
    }
    let tabs_valid = request
        .attachments
        .iter()
        .all(|attachment| match attachment {
            PageAssistAttachV1::Page { tab } | PageAssistAttachV1::Selection { tab } => {
                valid_tab(tab)
            }
            PageAssistAttachV1::Element { tab, pick } => valid_tab(tab) && valid_id(pick),
        });
    if !valid_space(&request.space)
        || !valid_id(&request.conversation)
        || !tabs_valid
        || request.text.len() > PAGE_ASSIST_MAX_QUESTION_BYTES
    {
        return refused(PageAssistFaultV1::Invalid);
    }
    with_service!(refused(PageAssistFaultV1::Unavailable), |service| {
        match service.ask(request).await {
            Ok((reply, task)) => {
                tauri::async_runtime::spawn(task);
                PageAssistAskResponseV1 {
                    reply: Some(reply),
                    fault: None,
                }
            }
            Err(fault) => refused(fault),
        }
    })
}

/// Stops the Space's running reply; nothing more is sent or applied.
#[tauri::command]
#[specta::specta]
pub(crate) async fn page_assist_stop(caller: WebviewWindow, space: String) -> bool {
    allowed(&caller, "page_assist_stop") && with_service!(false, |service| service.stop(&space))
}

/// Brings cite `n` of a reply into view on its page.
#[tauri::command]
#[specta::specta]
pub(crate) async fn page_assist_cite(
    caller: WebviewWindow,
    conversation: String,
    reply: u32,
    n: u8,
) -> bool {
    allowed(&caller, "page_assist_cite")
        && valid_id(&conversation)
        && with_service!(false, |service| service.cite(&conversation, reply, n))
}

/// Emphasises an item's mark on its page while its card is hovered; none clears it.
#[tauri::command]
#[specta::specta]
pub(crate) async fn page_assist_focus(
    caller: WebviewWindow,
    conversation: String,
    reply: u32,
    block: u16,
    n: Option<u8>,
) -> bool {
    allowed(&caller, "page_assist_focus")
        && valid_id(&conversation)
        && with_service!(false, |service| service.focus(
            &conversation,
            reply,
            block,
            n
        ))
}

/// Answers a waiting confirm, a proposed style or an offered handoff.
#[tauri::command]
#[specta::specta]
pub(crate) async fn page_assist_decide(
    caller: WebviewWindow,
    conversation: String,
    reply: u32,
    block: u16,
    decision: PageAssistDecisionV1,
) -> bool {
    allowed(&caller, "page_assist_decide")
        && valid_id(&conversation)
        && with_service!(false, |service| service.decide(
            &conversation,
            reply,
            block,
            decision
        ))
}

/// Continues a paused reply after the person used the page; it reads again.
#[tauri::command]
#[specta::specta]
pub(crate) async fn page_assist_resume(
    caller: WebviewWindow,
    conversation: String,
    reply: u32,
) -> bool {
    allowed(&caller, "page_assist_resume")
        && valid_id(&conversation)
        && with_service!(false, |service| service.resume(&conversation, reply))
}

/// Starts the Work a reply offered.
#[tauri::command]
#[specta::specta]
pub(crate) async fn page_assist_continue(
    caller: WebviewWindow,
    conversation: String,
    reply: u32,
    block: u16,
) -> PageAssistContinueV1 {
    let unavailable = PageAssistContinueV1 {
        work: None,
        fault: Some(PageAssistFaultV1::Unavailable),
    };
    if !allowed(&caller, "page_assist_continue") || !valid_id(&conversation) {
        return unavailable;
    }
    with_service!(unavailable, |service| service
        .continue_work(&conversation, reply, block)
        .await)
}

/// Starts a new conversation in the Space; the old one stays in history.
#[tauri::command]
#[specta::specta]
pub(crate) async fn page_assist_new(caller: WebviewWindow, space: String) -> bool {
    allowed(&caller, "page_assist_new")
        && valid_space(&space)
        && with_service!(false, |service| service.new_conversation(&space))
}

/// Past conversations of the Space's profile, newest first.
#[tauri::command]
#[specta::specta]
pub(crate) async fn page_assist_history(
    caller: WebviewWindow,
    space: String,
) -> PageAssistHistoryV1 {
    let empty = PageAssistHistoryV1 {
        version: VERSION,
        conversations: Vec::new(),
    };
    if !allowed(&caller, "page_assist_history") || !valid_space(&space) {
        return empty;
    }
    with_service!(empty, |service| service.history(&space))
}

/// Shows a past conversation in the Space.
#[tauri::command]
#[specta::specta]
pub(crate) async fn page_assist_open(
    caller: WebviewWindow,
    space: String,
    conversation: String,
) -> bool {
    allowed(&caller, "page_assist_open")
        && valid_space(&space)
        && valid_id(&conversation)
        && with_service!(false, |service| service.open(&space, &conversation))
}

/// Deletes a conversation.
#[tauri::command]
#[specta::specta]
pub(crate) async fn page_assist_delete(
    caller: WebviewWindow,
    space: String,
    conversation: String,
) -> bool {
    allowed(&caller, "page_assist_delete")
        && valid_space(&space)
        && valid_id(&conversation)
        && with_service!(false, |service| service.delete(&space, &conversation))
}

/// The person picks an element on the tab; resolves when they pick or cancel.
#[tauri::command]
#[specta::specta]
pub(crate) async fn page_assist_pick(
    caller: WebviewWindow,
    space: String,
    tab: String,
) -> PageAssistPickResponseV1 {
    let unavailable = PageAssistPickResponseV1 {
        pick: None,
        fault: Some(PageAssistFaultV1::Unavailable),
    };
    if !allowed(&caller, "page_assist_pick") || !valid_space(&space) || !valid_tab(&tab) {
        return unavailable;
    }
    with_service!(unavailable, |service| service.pick(&space, &tab).await)
}

/// Ends a pick in progress.
#[tauri::command]
#[specta::specta]
pub(crate) async fn page_assist_pick_cancel(caller: WebviewWindow, tab: String) -> bool {
    allowed(&caller, "page_assist_pick_cancel")
        && valid_tab(&tab)
        && with_service!(false, |service| {
            service.pick_cancel(&tab);
            true
        })
}

/// Detaches a picked element and removes its outline.
#[tauri::command]
#[specta::specta]
pub(crate) async fn page_assist_detach(caller: WebviewWindow, tab: String, pick: String) -> bool {
    allowed(&caller, "page_assist_detach")
        && valid_tab(&tab)
        && valid_id(&pick)
        && with_service!(false, |service| service.detach(&tab, &pick))
}

/// The panel closed: marks and picks leave the Space's pages.
#[tauri::command]
#[specta::specta]
pub(crate) async fn page_assist_close(caller: WebviewWindow, space: String) -> bool {
    allowed(&caller, "page_assist_close")
        && valid_space(&space)
        && with_service!(false, |service| {
            service.close(&space);
            true
        })
}

/// The page styles of the site in `tab`.
#[tauri::command]
#[specta::specta]
pub(crate) async fn page_styles(caller: WebviewWindow, tab: String) -> PageStylesV1 {
    if !allowed(&caller, "page_styles") {
        return no_styles(tab);
    }
    with_service!(no_styles(tab), |service| service.styles(&tab))
}

/// Turns one of the site's styles on or off.
#[tauri::command]
#[specta::specta]
pub(crate) async fn page_style_toggle(
    caller: WebviewWindow,
    tab: String,
    style: String,
    enabled: bool,
) -> PageStylesV1 {
    if !allowed(&caller, "page_style_toggle") || !valid_id(&style) {
        return no_styles(tab);
    }
    with_service!(no_styles(tab), |service| service
        .toggle_style(&tab, &style, enabled))
}

/// Deletes one of the site's styles.
#[tauri::command]
#[specta::specta]
pub(crate) async fn page_style_remove(
    caller: WebviewWindow,
    tab: String,
    style: String,
) -> PageStylesV1 {
    if !allowed(&caller, "page_style_remove") || !valid_id(&style) {
        return no_styles(tab);
    }
    with_service!(no_styles(tab), |service| service.remove_style(&tab, &style))
}

#[cfg(feature = "work-product")]
fn profile_of(profile: &str) -> Option<zephium_core::ids::ProfileId> {
    zephium_core::ids::ProfileId::parse(profile).filter(|id| id.to_string() == profile)
}

/// Every site's page styles in `profile`, for Settings.
#[tauri::command]
#[specta::specta]
pub(crate) async fn page_styles_all(caller: WebviewWindow, profile: String) -> PageStylesAllV1 {
    if !allowed(&caller, "page_styles_all") {
        return no_profile_styles(profile);
    }
    with_service!(no_profile_styles(profile), |service| {
        match profile_of(&profile) {
            Some(id) => service.all_styles(&id),
            None => no_profile_styles(profile),
        }
    })
}

/// Turns one of a site's styles on or off from Settings.
#[tauri::command]
#[specta::specta]
pub(crate) async fn page_style_toggle_for(
    caller: WebviewWindow,
    profile: String,
    site: String,
    style: String,
    enabled: bool,
) -> PageStylesAllV1 {
    if !allowed(&caller, "page_style_toggle_for") || !valid_id(&style) || site.len() > 253 {
        return no_profile_styles(profile);
    }
    with_service!(no_profile_styles(profile), |service| {
        match profile_of(&profile) {
            Some(id) => service.toggle_style_for(&id, &site, &style, enabled),
            None => no_profile_styles(profile),
        }
    })
}

/// Deletes one of a site's styles from Settings.
#[tauri::command]
#[specta::specta]
pub(crate) async fn page_style_remove_for(
    caller: WebviewWindow,
    profile: String,
    site: String,
    style: String,
) -> PageStylesAllV1 {
    if !allowed(&caller, "page_style_remove_for") || !valid_id(&style) || site.len() > 253 {
        return no_profile_styles(profile);
    }
    with_service!(no_profile_styles(profile), |service| {
        match profile_of(&profile) {
            Some(id) => service.remove_style_for(&id, &site, &style),
            None => no_profile_styles(profile),
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tab_and_style_ids_are_closed() {
        assert!(valid_tab("01HZX3J5Q4F0Y8N9W2E6R7T1M3"));
        for bad in ["", "a b", "../x", &"a".repeat(65)] {
            assert!(!valid_tab(bad), "{bad}");
        }
        assert!(valid_id("s123-4"));
        assert!(!valid_id("s1\"};"));
    }
}

/// "Run in Work" from the Ask panel: the same Work documents and operation
/// owner the panel's own Work flow uses, in order.
#[cfg(feature = "work-product")]
mod handoff {
    use super::*;
    use std::time::Duration;
    use zephium_core::ids::{ItemId, ProfileId, SpaceId};
    use zephium_core::work::authoring::WorkAuthoringIntent;
    use zephium_core::work::context::{
        url_revision, WorkContextSelectionItem, WorkContextSelectionV1,
    };
    use zephium_core::work::environment::{
        WorkEnvironmentCall, WorkEnvironmentEdit, WorkEnvironmentIntent, WorkEnvironmentReference,
        WorkEnvironmentReply, WorkEnvironmentSnapshot,
    };
    use zephium_core::work::runtime::WorkRuntimeIntent;
    use zephium_core::work::{WorkCommandId, WorkError};
    use zephium_ipc::work::{
        WorkAuthoringCommandV1, WorkCallV1, WorkCommandV1, WorkOperationStateV1, WorkOperationV1,
        WorkReplyV1,
    };

    const TIMEOUT: Duration = Duration::from_secs(8);
    const MAX_TITLE_CHARS: usize = 80;

    async fn call(
        app: &tauri::AppHandle,
        profile: ProfileId,
        call: WorkCallV1,
    ) -> Result<WorkReplyV1, WorkError> {
        let request = app
            .state::<zephium_app::Handle>()
            .work_call(profile, call)?;
        let response = tokio::time::timeout(TIMEOUT, request.response(profile))
            .await
            .map_err(|_| WorkError::OutcomeUnknown)?;
        match response.reply {
            WorkReplyV1::Error { .. } => Err(WorkError::Unavailable),
            reply => Ok(reply),
        }
    }

    fn environment(intent: WorkEnvironmentIntent) -> WorkCallV1 {
        WorkCallV1::Environment {
            version: 1,
            request: WorkEnvironmentCall::Command {
                command: WorkCommandId::generate(),
                intent,
            },
        }
    }

    fn added(
        snapshot: &WorkEnvironmentSnapshot,
        reference: WorkEnvironmentReference,
    ) -> WorkEnvironmentIntent {
        WorkEnvironmentIntent::Edit {
            id: snapshot.id,
            expected: snapshot.revision,
            edit: WorkEnvironmentEdit::Add {
                reference,
                area: None,
            },
        }
    }

    fn applied(reply: WorkReplyV1) -> Result<Box<WorkEnvironmentSnapshot>, WorkError> {
        match reply {
            WorkReplyV1::Environment {
                reply: WorkEnvironmentReply::Applied { snapshot, .. },
            } => Ok(snapshot),
            _ => Err(WorkError::Unavailable),
        }
    }

    /// The panel's own routine grant and limits (`AGENT_GRANT`, `AGENT_LIMITS`
    /// in `frame/src/domain/work/work-model.ts`).
    pub(super) fn begin_agent() -> Result<WorkRuntimeIntent, WorkError> {
        serde_json::from_value(serde_json::json!({
            "kind": "begin_agent",
            "grant": {
                "provider": "open_ai",
                "model": "gpt-6-luna",
                "max_turns": 10,
                "max_steps": 32,
                "browse_hops": 4
            },
            "limits": {
                "model_tokens": 1_000_000,
                "cost_micro_usd": 3_000_000,
                "operations": 256,
                "timeout_seconds": 1800,
                "max_workers": 4
            }
        }))
        .map_err(|_| WorkError::Invalid)
    }

    /// Creates a Work in the Space with the handoff's tabs and objective on
    /// its canvas, then begins a run with those tabs as context.
    pub(super) async fn start(
        app: &tauri::AppHandle,
        profile: ProfileId,
        space: &str,
        start: &zephium_work_composition::page_assist::port::WorkStart,
        tabs: &[(String, String)],
    ) -> Result<String, WorkError> {
        let space = SpaceId::parse(space).ok_or(WorkError::Invalid)?;
        let intent = begin_agent()?;
        let title = zephium_work_composition::page_assist::core::text::bounded(
            &zephium_work_composition::page_assist::core::text::clean(&start.title),
            MAX_TITLE_CHARS,
        );
        let mut snapshot = applied(
            call(
                app,
                profile,
                environment(WorkEnvironmentIntent::Create { space, title }),
            )
            .await?,
        )?;
        let mut items = Vec::new();
        for (tab, url) in tabs {
            let Some(tab) = ItemId::parse(tab) else {
                continue;
            };
            let browser = WorkEnvironmentReference::Browser { tab };
            snapshot =
                applied(call(app, profile, environment(added(&snapshot, browser.clone()))).await?)?;
            if let Some(element) = snapshot
                .elements
                .iter()
                .find(|element| element.reference == browser)
            {
                items.push(WorkContextSelectionItem {
                    element: element.id,
                    revision: url_revision(url),
                });
            }
        }
        let WorkReplyV1::AuthoringApplied { receipt } = call(
            app,
            profile,
            WorkCallV1::Author {
                command: WorkAuthoringCommandV1 {
                    version: 1,
                    command: WorkCommandId::generate(),
                    intent: WorkAuthoringIntent::Create {
                        objective: start.objective.clone(),
                    },
                },
            },
        )
        .await?
        else {
            return Err(WorkError::Unavailable);
        };
        // The objective joins the canvas beside its tabs; the run does not
        // depend on it, so a refusal here does not stop the run.
        let objective = WorkEnvironmentReference::Objective {
            objective: receipt.work,
        };
        let _ = call(app, profile, environment(added(&snapshot, objective))).await;
        let operation = WorkCommandId::generate();
        let input = WorkOperationV1::Run {
            command: WorkCommandV1 {
                version: 1,
                work: receipt.work,
                expected_revision: receipt.applied_revision,
                command: WorkCommandId::generate(),
                intent,
            },
            context: (!items.is_empty()).then_some(WorkContextSelectionV1 {
                environment: snapshot.id,
                items,
                tabs: false,
                blocks: false,
            }),
            signed_in: Default::default(),
        };
        match super::super::work_product::admit_run(app, profile, operation, input)? {
            WorkOperationStateV1::Refused { .. } => Err(WorkError::Unavailable),
            _ => Ok(receipt.work.to_string()),
        }
    }

    #[cfg(test)]
    mod tests {
        #[test]
        fn the_grant_matches_the_panels_own() {
            assert!(super::begin_agent().is_ok());
            let frame = include_str!("../../frame/src/domain/work/work-model.ts");
            for pinned in [
                "provider: \"open_ai\"",
                "model: \"gpt-6-luna\"",
                "max_turns: 10",
                "max_steps: 32",
                "browse_hops: 4",
                "model_tokens: 1_000_000",
                "cost_micro_usd: 3_000_000",
                "operations: 256",
                "timeout_seconds: 1800",
                "max_workers: 4",
            ] {
                assert!(
                    frame.contains(pinned),
                    "{pinned} drifted from the panel's grant"
                );
            }
        }
    }
}
