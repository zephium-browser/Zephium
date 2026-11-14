//! In-memory adapter for namespace conformance and state-machine tests.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};
use url::Url;
use zephium_core::extensions::{
    ExtensionBrowserRequestAction as CoreAction, ExtensionBrowserSurfaceGeneration,
    ExtensionBrowserTab,
};
use zephium_core::ids::{ItemId, ProfileId};

use crate::api::{tabs, windows};
use crate::browser::*;
use crate::dispatch::{ApiTable, Dispatcher, Inbound};
use crate::envelope::{ApiName, CallId, Direction, Envelope};
use crate::permissions::InstallPermissions;
use crate::{ApiError, DocumentGeneration, Handled, Invocation, Sender};

pub struct FakeBrowser {
    pub snapshot: BrowserSnapshot,
    pub info: ExtensionInfo,
    pub requests: Vec<BrowserRequest>,
    pub events: Vec<(String, Value)>,
    pub reply: Result<Value, ApiError>,
    pub gesture: bool,
    pub resources: BTreeSet<String>,
    pub time_ms: u64,
    pub frames: Vec<crate::api::web_navigation::BrowserFrame>,
    pub action: crate::api::action::ActionStore,
    pub languages: BTreeMap<i32, String>,
    pub capture: String,
    pub history: Vec<(i32, String)>,
    navigation: BTreeMap<i32, (Vec<String>, usize)>,
    history_records: Vec<(i64, String, String, u64)>,
    next_visit: i64,
    next_tab: i32,
    next_window: i32,
    next_document: u64,
    next_call: u32,
    caller: Option<Sender>,
}

impl FakeBrowser {
    pub fn new(profile: ProfileId) -> Self {
        Self {
            snapshot: BrowserSnapshot {
                profile,
                generation: ExtensionBrowserSurfaceGeneration::INITIAL,
                tabs: Vec::new(),
                windows: Vec::new(),
                current_window: None,
                last_focused_window: None,
            },
            info: ExtensionInfo::default(),
            requests: Vec::new(),
            events: Vec::new(),
            reply: Ok(Value::Null),
            gesture: false,
            resources: BTreeSet::new(),
            time_ms: 1,
            frames: Vec::new(),
            action: Default::default(),
            languages: BTreeMap::new(),
            capture: "data:image/png;base64,AA==".into(),
            history: Vec::new(),
            navigation: BTreeMap::new(),
            history_records: Vec::new(),
            next_visit: 1,
            next_tab: 1,
            next_window: 1,
            next_document: 1,
            next_call: 1,
            caller: None,
        }
    }

    pub fn add_window(&mut self, incognito: bool) -> i32 {
        let id = self.next_window;
        self.next_window += 1;
        self.snapshot.windows.push(BrowserWindow {
            id,
            native_id: id as u64,
            incognito,
            focused: self.snapshot.windows.is_empty(),
            kind: "normal".into(),
            state: "normal".into(),
            bounds: [0, 0, 1024, 768],
            always_on_top: false,
        });
        if self.snapshot.current_window.is_none() {
            self.snapshot.current_window = Some(id);
            self.snapshot.last_focused_window = Some(id);
        }
        id
    }

    pub fn add_tab(&mut self, window_id: i32, url: &str, active: bool) -> i32 {
        let window = self.snapshot.window(window_id).expect("test window");
        let incognito = window.incognito;
        let id = self.next_tab;
        self.next_tab += 1;
        let document = DocumentGeneration::new(self.next_document).expect("document");
        self.next_document += 1;
        let url = Url::parse(url).expect("test URL");
        let tab = ExtensionBrowserTab::from_snapshot(
            None,
            ItemId::from(id as u128),
            true,
            url.host_str().unwrap_or("New Tab"),
            if zephium_core::navigation::is_browser_target(&url) {
                Some(&url)
            } else {
                None
            },
            false,
            false,
        )
        .expect("test tab");
        if active {
            for tab in &mut self.snapshot.tabs {
                if tab.window_id == window_id {
                    tab.active = false;
                    tab.highlighted = false;
                }
            }
        }
        let index = self
            .snapshot
            .tabs
            .iter()
            .filter(|tab| tab.window_id == window_id)
            .count();
        self.snapshot.tabs.push(BrowserTab {
            id,
            window_id,
            index,
            tab,
            active,
            highlighted: active,
            incognito,
            document,
            document_id: format!("document-{}", document.get()),
            native_url: (!zephium_core::navigation::is_browser_target(&url)).then(|| url.clone()),
            pending_url: None,
            fav_icon_url: None,
            zoom: 1.0,
            auto_discardable: true,
            opener_tab_id: None,
            last_accessed: self.time_ms as f64,
        });
        self.navigation.insert(id, (vec![url.into()], 0));
        id
    }

    pub fn call(
        &mut self,
        dispatcher: &mut Dispatcher<Self>,
        sender: &Sender,
        permissions: &InstallPermissions,
        name: &str,
        args: Value,
    ) -> Result<Value, String> {
        let id = CallId::new(self.next_call).expect("call id");
        self.next_call += 1;
        let args = serde_json::to_vec(&args).expect("args");
        let wire = Envelope::Call {
            id,
            name: ApiName::parse(name).expect("name"),
            args: &args,
        }
        .encode(Direction::FromContext, u64::MAX)
        .expect("wire");
        let inbound = dispatcher.receive(self, permissions, sender, &wire, 0);
        let Inbound::Reply(reply) = inbound else {
            panic!("expected immediate fake reply: {inbound:?}")
        };
        let Envelope::Reply { result, .. } =
            Envelope::decode(&reply, Direction::ToContext, u64::MAX).expect("reply")
        else {
            panic!("expected reply frame")
        };
        result
            .map(|bytes| serde_json::from_slice(bytes).expect("value"))
            .map_err(str::to_owned)
    }

    pub fn dispatcher() -> Dispatcher<Self> {
        let mut table = ApiTable::new();
        tabs::register(&mut table).expect("tabs");
        windows::register(&mut table).expect("windows");
        Dispatcher::new(table)
    }

    fn project_tab(&self, tab: &BrowserTab, permissions: &InstallPermissions) -> Value {
        self.caller.as_ref().map_or_else(
            || tabs::project(tab, permissions),
            |sender| tabs::project_for(tab, sender, permissions),
        )
    }

    fn project_window(
        &self,
        snapshot: &BrowserSnapshot,
        window: &BrowserWindow,
        populate: bool,
        permissions: &InstallPermissions,
    ) -> Value {
        self.caller.as_ref().map_or_else(
            || windows::project(snapshot, window, populate, permissions),
            |sender| windows::project_for(snapshot, window, populate, sender, permissions),
        )
    }

    fn bump(&mut self) {
        self.snapshot.generation = self.snapshot.generation.next().expect("fake generation");
        self.time_ms += 1;
    }

    fn event(&mut self, name: &str, args: Value) {
        self.events.push((name.into(), args));
    }

    fn tab_index(&self, id: i32) -> Result<usize, ApiError> {
        self.snapshot
            .tabs
            .iter()
            .position(|tab| tab.id == id)
            .ok_or_else(|| ApiError::other(format!("No tab with id: {id}.")))
    }

    fn replace_record(
        &mut self,
        id: i32,
        url: Option<&str>,
        resident: Option<bool>,
        loading: Option<bool>,
        pinned: Option<bool>,
        muted: Option<bool>,
    ) -> Result<(), ApiError> {
        let index = self.tab_index(id)?;
        let tab = &self.snapshot.tabs[index];
        let url = url
            .map(Url::parse)
            .transpose()
            .map_err(|_| ApiError::other("Invalid URL."))?
            .or_else(|| tab.url().ok());
        let record = ExtensionBrowserTab::from_snapshot(
            Some(&tab.tab),
            tab.tab.id(),
            resident.unwrap_or(tab.tab.resident()),
            tab.tab.title(),
            url.as_ref()
                .filter(|url| zephium_core::navigation::is_browser_target(url)),
            loading.unwrap_or(tab.tab.loading()),
            pinned.unwrap_or(tab.tab.pinned()),
        )
        .map_err(|_| ApiError::other("Invalid tab state."))?
        .with_audio(muted.unwrap_or(tab.tab.muted()), tab.tab.audible());
        self.snapshot.tabs[index].tab = record;
        self.snapshot.tabs[index].native_url =
            url.filter(|url| !zephium_core::navigation::is_browser_target(url));
        Ok(())
    }

    fn updated(
        &mut self,
        old: BrowserTab,
        permissions: &InstallPermissions,
    ) -> Result<(), ApiError> {
        let new = self.snapshot.tab(old.id)?.clone();
        let changed = self.caller.as_ref().map_or_else(
            || tabs::change_info(&old, &new, permissions),
            |sender| tabs::change_info_for(&old, &new, sender, permissions),
        );
        if changed
            .as_object()
            .is_some_and(|changed| !changed.is_empty())
        {
            self.event(
                "tabs.onUpdated",
                json!([old.id, changed, self.project_tab(&new, permissions)]),
            );
        }
        Ok(())
    }

    pub fn navigate(
        &mut self,
        id: i32,
        url: &str,
        permissions: &InstallPermissions,
    ) -> Result<(), ApiError> {
        let old = self.snapshot.tab(id)?.clone();
        let index = self.tab_index(id)?;
        self.snapshot.tabs[index].pending_url = Some(url.into());
        self.replace_record(id, None, Some(true), Some(true), None, None)?;
        self.updated(old, permissions)?;
        let old = self.snapshot.tab(id)?.clone();
        self.replace_record(id, Some(url), None, None, None, None)?;
        let index = self.tab_index(id)?;
        self.snapshot.tabs[index].pending_url = None;
        self.snapshot.tabs[index].document =
            DocumentGeneration::new(self.next_document).expect("document");
        self.snapshot.tabs[index].document_id = format!("document-{}", self.next_document);
        self.next_document += 1;
        self.action.reset_tab(id)?;
        self.updated(old, permissions)?;
        let old = self.snapshot.tab(id)?.clone();
        self.replace_record(id, None, None, Some(false), None, None)?;
        self.updated(old, permissions)?;
        let navigation = self.navigation.entry(id).or_default();
        navigation.0.truncate(navigation.1 + 1);
        navigation.0.push(url.into());
        navigation.1 = navigation.0.len() - 1;
        if !self.snapshot.tab(id)?.incognito
            && Url::parse(url).is_ok_and(|url| matches!(url.scheme(), "http" | "https" | "file"))
        {
            self.history.push((id, url.into()));
            self.record_visit(url);
        }
        self.bump();
        Ok(())
    }

    fn traverse(
        &mut self,
        id: i32,
        forward: bool,
        permissions: &InstallPermissions,
    ) -> Result<(), ApiError> {
        let (entries, position) =
            self.navigation.get(&id).cloned().ok_or_else(|| {
                ApiError::other("Cannot navigate to the requested history entry.")
            })?;
        let next = if forward {
            position.checked_add(1).filter(|next| *next < entries.len())
        } else {
            position.checked_sub(1)
        }
        .ok_or_else(|| {
            ApiError::other(if forward {
                "Cannot navigate forward."
            } else {
                "Cannot navigate back."
            })
        })?;
        self.navigate(id, &entries[next], permissions)?;
        self.navigation.insert(id, (entries, next));
        Ok(())
    }

    fn activate(&mut self, id: i32) -> Result<(), ApiError> {
        let tab = self.snapshot.tab(id)?.clone();
        let previous = self
            .snapshot
            .tabs
            .iter()
            .find(|other| other.window_id == tab.window_id && other.active)
            .map(|tab| tab.id);
        for other in &mut self.snapshot.tabs {
            if other.window_id == tab.window_id {
                other.active = other.id == id;
                other.highlighted = other.id == id;
            }
        }
        let index = self.tab_index(id)?;
        self.snapshot.tabs[index].last_accessed = self.time_ms as f64;
        if previous != Some(id) {
            self.event(
                "tabs.onActivated",
                json!([{"tabId":id,"windowId":tab.window_id}]),
            );
            self.event(
                "tabs.onHighlighted",
                json!([{"tabIds":[id],"windowId":tab.window_id}]),
            );
        }
        Ok(())
    }

    fn normalize(&mut self, window: i32) {
        let mut indices: Vec<_> = self
            .snapshot
            .tabs
            .iter()
            .enumerate()
            .filter(|(_, tab)| tab.window_id == window)
            .map(|(index, tab)| (index, tab.index))
            .collect();
        indices.sort_by_key(|(_, position)| *position);
        for (position, (index, _)) in indices.into_iter().enumerate() {
            self.snapshot.tabs[index].index = position;
        }
    }

    fn position_tab(&mut self, id: i32, index: usize) -> Result<(), ApiError> {
        let tab = self.snapshot.tab(id)?.clone();
        let mut remaining: Vec<_> = self
            .snapshot
            .tabs
            .iter()
            .filter(|other| other.window_id == tab.window_id && other.id != id)
            .map(|other| (other.index, other.id, other.tab.pinned()))
            .collect();
        remaining.sort_by_key(|(index, _, _)| *index);
        let pinned = remaining.iter().filter(|(_, _, pinned)| *pinned).count();
        let index = if tab.tab.pinned() {
            index.min(pinned)
        } else {
            index.max(pinned).min(remaining.len())
        };
        let mut order: Vec<_> = remaining.into_iter().map(|(_, id, _)| id).collect();
        order.insert(index, id);
        for (index, id) in order.into_iter().enumerate() {
            let position = self.tab_index(id)?;
            self.snapshot.tabs[position].index = index;
        }
        Ok(())
    }

    fn remove_tab(&mut self, id: i32, closing_window: bool) -> Result<(), ApiError> {
        let index = self.tab_index(id)?;
        let removed = self.snapshot.tabs.remove(index);
        self.navigation.remove(&id);
        self.action.reset_tab(id)?;
        self.frames.retain(|frame| frame.tab != removed.tab.id());
        for tab in &mut self.snapshot.tabs {
            if tab.opener_tab_id == Some(id) {
                tab.opener_tab_id = None;
            }
        }
        self.normalize(removed.window_id);
        self.event(
            "tabs.onRemoved",
            json!([id,{"windowId":removed.window_id,"isWindowClosing":closing_window}]),
        );
        if removed.active && !closing_window {
            let next = self
                .snapshot
                .tabs
                .iter()
                .filter(|tab| tab.window_id == removed.window_id)
                .min_by_key(|tab| tab.index.abs_diff(removed.index))
                .map(|tab| tab.id);
            if let Some(next) = next {
                self.activate(next)?;
            }
        }
        if !closing_window
            && !self
                .snapshot
                .tabs
                .iter()
                .any(|tab| tab.window_id == removed.window_id)
        {
            self.remove_window(removed.window_id)?;
        }
        self.bump();
        Ok(())
    }

    fn move_tabs(&mut self, ids: &[i32], window: i32, index: i32) -> Result<(), ApiError> {
        let mut moved = Vec::new();
        for id in ids {
            let tab = self.snapshot.tab(*id)?.clone();
            if !moved.iter().any(|other: &BrowserTab| other.id == *id) {
                moved.push(tab);
            }
        }
        let mut remaining: Vec<_> = self
            .snapshot
            .tabs
            .iter()
            .filter(|tab| tab.window_id == window && !ids.contains(&tab.id))
            .map(|tab| (tab.index, tab.id))
            .collect();
        remaining.sort();
        let position = usize::try_from(index)
            .unwrap_or(remaining.len())
            .min(remaining.len());
        let mut order: Vec<_> = remaining.into_iter().map(|(_, id)| id).collect();
        order.splice(position..position, moved.iter().map(|tab| tab.id));
        for (new_index, id) in order.into_iter().enumerate() {
            let i = self.tab_index(id)?;
            self.snapshot.tabs[i].window_id = window;
            self.snapshot.tabs[i].index = new_index;
        }
        for old in &moved {
            let new = self.snapshot.tab(old.id)?.clone();
            if old.window_id != window {
                self.event(
                    "tabs.onDetached",
                    json!([old.id,{"oldWindowId":old.window_id,"oldPosition":old.index}]),
                );
                self.event(
                    "tabs.onAttached",
                    json!([old.id,{"newWindowId":window,"newPosition":new.index}]),
                );
                self.normalize(old.window_id);
            } else if old.index != new.index {
                self.event(
                    "tabs.onMoved",
                    json!([old.id,{"windowId":window,"fromIndex":old.index,"toIndex":new.index}]),
                );
            }
        }
        for old in &moved {
            if old.window_id != window {
                if old.active {
                    if let Some(next) = self
                        .snapshot
                        .tabs
                        .iter()
                        .find(|tab| tab.window_id == old.window_id)
                        .map(|tab| tab.id)
                    {
                        self.activate(next)?;
                    }
                }
                if !self
                    .snapshot
                    .tabs
                    .iter()
                    .any(|tab| tab.window_id == old.window_id)
                {
                    self.remove_window(old.window_id)?;
                }
            }
        }
        if !self
            .snapshot
            .tabs
            .iter()
            .any(|tab| tab.window_id == window && tab.active)
        {
            if let Some(first) = moved.first() {
                self.activate(first.id)?;
            }
        } else {
            let active = self
                .snapshot
                .tabs
                .iter()
                .find(|tab| tab.window_id == window && tab.active && !ids.contains(&tab.id))
                .map(|tab| tab.id);
            if let Some(active) = active {
                for tab in &mut self.snapshot.tabs {
                    if tab.window_id == window && tab.id != active {
                        tab.active = false;
                    }
                }
            }
        }
        self.bump();
        Ok(())
    }

    fn tabs(
        &mut self,
        request: tabs::Request,
        permissions: &InstallPermissions,
    ) -> Result<Value, ApiError> {
        use tabs::Request;
        match request {
            Request::Create { properties, .. } => {
                let window = properties
                    .window_id
                    .ok_or_else(ApiError::malformed_arguments)?;
                let id = self.add_tab(
                    window,
                    properties.url.as_deref().unwrap_or("about:blank"),
                    false,
                );
                self.replace_record(id, None, None, None, properties.pinned, None)?;
                let i = self.tab_index(id)?;
                self.snapshot.tabs[i].opener_tab_id = properties.opener_tab_id;
                let index = properties.index.unwrap_or(self.snapshot.tab(id)?.index);
                self.position_tab(id, index)?;
                let event_start = self.events.len();
                if properties.active.or(properties.selected).unwrap_or(true) {
                    self.activate(id)?;
                }
                let activation = self.events.split_off(event_start);
                self.event(
                    "tabs.onCreated",
                    json!([self.project_tab(self.snapshot.tab(id)?, permissions)]),
                );
                self.events.extend(activation);
                self.bump();
                Ok(self.project_tab(self.snapshot.tab(id)?, permissions))
            }
            Request::Update {
                tab_id, properties, ..
            } => {
                if let Some(url) = &properties.url {
                    self.navigate(tab_id, url, permissions)?;
                }
                let old = self.snapshot.tab(tab_id)?.clone();
                self.replace_record(
                    tab_id,
                    None,
                    None,
                    None,
                    properties.pinned,
                    properties.muted,
                )?;
                if properties
                    .pinned
                    .is_some_and(|pinned| pinned != old.tab.pinned())
                {
                    self.position_tab(tab_id, 0)?;
                }
                let index = self.tab_index(tab_id)?;
                if let Some(value) = properties.auto_discardable {
                    self.snapshot.tabs[index].auto_discardable = value;
                }
                if let Some(value) = properties.opener_tab_id {
                    self.snapshot.tabs[index].opener_tab_id = Some(value);
                }
                if properties.active == Some(true) || properties.selected == Some(true) {
                    self.activate(tab_id)?;
                }
                if let Some(value) = properties.highlighted.or(properties.selected) {
                    let index = self.tab_index(tab_id)?;
                    self.snapshot.tabs[index].highlighted = value;
                    let window = self.snapshot.tabs[index].window_id;
                    let highlighted: Vec<_> = self
                        .snapshot
                        .tabs
                        .iter()
                        .filter(|tab| tab.window_id == window && tab.highlighted)
                        .map(|tab| tab.id)
                        .collect();
                    if highlighted.is_empty() {
                        self.snapshot.tabs[index].highlighted = true;
                        return Err(ApiError::other("Cannot unhighlight all tabs."));
                    }
                    self.event(
                        "tabs.onHighlighted",
                        json!([{"windowId":window,"tabIds":highlighted}]),
                    );
                }
                self.updated(old, permissions)?;
                self.bump();
                Ok(self.project_tab(self.snapshot.tab(tab_id)?, permissions))
            }
            Request::Remove { tab_ids, .. } => {
                for id in tab_ids {
                    if self.snapshot.tab(id).is_ok() {
                        self.remove_tab(id, false)?;
                    }
                }
                Ok(Value::Null)
            }
            Request::Reload { tab_id, .. } => {
                let url = self
                    .snapshot
                    .tab(tab_id)?
                    .committed_url()
                    .unwrap_or("about:blank")
                    .to_owned();
                let navigation = self.navigation.get(&tab_id).cloned();
                self.navigate(tab_id, &url, permissions)?;
                if let Some(navigation) = navigation {
                    self.navigation.insert(tab_id, navigation);
                }
                Ok(Value::Null)
            }
            Request::Duplicate { tab_id, .. } => {
                let source = self.snapshot.tab(tab_id)?.clone();
                let id = self.add_tab(
                    source.window_id,
                    source.committed_url().unwrap_or("about:blank"),
                    false,
                );
                let index = self.tab_index(id)?;
                self.snapshot.tabs[index].tab = ExtensionBrowserTab::from_snapshot(
                    None,
                    self.snapshot.tabs[index].tab.id(),
                    true,
                    source.tab.title(),
                    source
                        .url()
                        .ok()
                        .as_ref()
                        .filter(|url| zephium_core::navigation::is_browser_target(url)),
                    false,
                    source.tab.pinned(),
                )
                .expect("duplicate")
                .with_audio(source.tab.muted(), false);
                self.snapshot.tabs[index].native_url = source.native_url.clone();
                self.snapshot.tabs[index].zoom = source.zoom;
                if let Some(navigation) = self.navigation.get(&tab_id).cloned() {
                    self.navigation.insert(id, navigation);
                }
                self.position_tab(id, source.index + 1)?;
                let event_start = self.events.len();
                self.activate(id)?;
                let activation = self.events.split_off(event_start);
                self.event(
                    "tabs.onCreated",
                    json!([self.project_tab(self.snapshot.tab(id)?, permissions)]),
                );
                self.events.extend(activation);
                Ok(self.project_tab(self.snapshot.tab(id)?, permissions))
            }
            Request::Move {
                tab_ids,
                window_id,
                index,
                single,
                ..
            } => {
                self.move_tabs(&tab_ids, window_id, index)?;
                let values = tab_ids
                    .iter()
                    .map(|id| {
                        self.snapshot
                            .tab(*id)
                            .map(|tab| self.project_tab(tab, permissions))
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(if single {
                    values.into_iter().next().unwrap_or(Value::Null)
                } else {
                    Value::Array(values)
                })
            }
            Request::Highlight {
                window_id, tab_ids, ..
            } => {
                self.activate(tab_ids[0])?;
                for tab in &mut self.snapshot.tabs {
                    if tab.window_id == window_id {
                        tab.highlighted = tab_ids.contains(&tab.id);
                    }
                }
                self.event(
                    "tabs.onHighlighted",
                    json!([{"windowId":window_id,"tabIds":tab_ids}]),
                );
                self.bump();
                Ok(self.project_window(
                    &self.snapshot,
                    self.snapshot.window(window_id)?,
                    true,
                    permissions,
                ))
            }
            Request::Navigate {
                tab_id, forward, ..
            } => {
                self.traverse(tab_id, forward, permissions)?;
                Ok(Value::Null)
            }
            Request::Capture { .. } => Ok(json!(self.capture)),
            Request::DetectLanguage { tab_id, .. } => Ok(json!(self
                .languages
                .get(&tab_id)
                .map_or("und", String::as_str))),
            Request::SetZoom { tab_id, factor, .. } => {
                let index = self.tab_index(tab_id)?;
                let old = self.snapshot.tabs[index].zoom;
                let factor = if factor == 0.0 { 1.0 } else { factor };
                self.snapshot.tabs[index].zoom = factor;
                if old != factor {
                    self.event("tabs.onZoomChange",json!([{"tabId":tab_id,"oldZoomFactor":old,"newZoomFactor":factor,"zoomSettings":{"mode":"automatic","scope":"per-tab"}}]));
                }
                self.bump();
                Ok(Value::Null)
            }
            Request::Discard { tab_id } => {
                let old = self.snapshot.tab(tab_id)?.clone();
                self.replace_record(tab_id, None, Some(false), None, None, None)?;
                self.updated(old, permissions)?;
                self.bump();
                Ok(self.project_tab(self.snapshot.tab(tab_id)?, permissions))
            }
        }
    }

    fn focus(&mut self, id: Option<i32>) {
        let previous = self
            .snapshot
            .windows
            .iter()
            .find(|window| window.focused)
            .map(|window| window.id);
        for window in &mut self.snapshot.windows {
            window.focused = Some(window.id) == id;
        }
        self.snapshot.current_window = id.or(self.snapshot.last_focused_window);
        if let Some(id) = id {
            self.snapshot.last_focused_window = Some(id);
        }
        if previous != id {
            self.event("windows.onFocusChanged", json!([id.unwrap_or(-1)]));
        }
    }

    fn remove_window(&mut self, id: i32) -> Result<(), ApiError> {
        self.snapshot.window(id)?;
        let tabs: Vec<_> = self
            .snapshot
            .tabs
            .iter()
            .filter(|tab| tab.window_id == id)
            .map(|tab| tab.id)
            .collect();
        for tab in tabs {
            self.remove_tab(tab, true)?;
        }
        let focused = self
            .snapshot
            .windows
            .iter()
            .any(|window| window.id == id && window.focused);
        self.snapshot.windows.retain(|window| window.id != id);
        self.event("windows.onRemoved", json!([id]));
        if self.snapshot.last_focused_window == Some(id) {
            self.snapshot.last_focused_window =
                self.snapshot.windows.first().map(|window| window.id);
        }
        if focused {
            let next = self.snapshot.windows.first().map(|window| window.id);
            if next.is_none() {
                self.event("windows.onFocusChanged", json!([-1]));
            }
            self.focus(next);
        }
        if self.snapshot.current_window == Some(id) {
            self.snapshot.current_window = self.snapshot.last_focused_window;
        }
        self.bump();
        Ok(())
    }

    fn windows(
        &mut self,
        request: windows::Request,
        permissions: &InstallPermissions,
    ) -> Result<Value, ApiError> {
        match request {
            windows::Request::Create { properties } => {
                let id = self.add_window(properties.incognito.unwrap_or_else(|| {
                    self.snapshot
                        .windows
                        .first()
                        .is_some_and(|window| window.incognito)
                }));
                self.apply_window(id, &properties, permissions)?;
                if let Some(tab) = properties.tab_id {
                    self.move_tabs(&[tab], id, -1)?;
                }
                let urls = match properties.url {
                    Some(Value::String(url)) => vec![url],
                    Some(Value::Array(urls)) => urls
                        .into_iter()
                        .filter_map(|url| url.as_str().map(str::to_owned))
                        .collect(),
                    _ => Vec::new(),
                };
                if urls.is_empty() && properties.tab_id.is_none() {
                    self.add_tab(id, "about:blank", true);
                } else {
                    for url in urls {
                        self.add_tab(id, &url, true);
                    }
                }
                if properties.focused.unwrap_or(true) {
                    self.focus(Some(id));
                }
                self.event(
                    "windows.onCreated",
                    json!([self.project_window(
                        &self.snapshot,
                        self.snapshot.window(id)?,
                        false,
                        permissions
                    )]),
                );
                self.bump();
                Ok(self.project_window(
                    &self.snapshot,
                    self.snapshot.window(id)?,
                    true,
                    permissions,
                ))
            }
            windows::Request::Update {
                window_id,
                properties,
                ..
            } => {
                self.apply_window(window_id, &properties, permissions)?;
                if properties.state.as_deref() == Some("minimized")
                    && self.snapshot.window(window_id)?.focused
                {
                    let next = self
                        .snapshot
                        .windows
                        .iter()
                        .find(|window| window.id != window_id && window.state != "minimized")
                        .map(|window| window.id);
                    self.focus(next);
                }
                if let Some(focused) = properties.focused {
                    if focused {
                        self.focus(Some(window_id));
                    } else if self.snapshot.window(window_id)?.focused {
                        self.focus(
                            self.snapshot
                                .windows
                                .iter()
                                .find(|window| window.id != window_id)
                                .map(|window| window.id),
                        );
                    }
                }
                self.bump();
                Ok(self.project_window(
                    &self.snapshot,
                    self.snapshot.window(window_id)?,
                    false,
                    permissions,
                ))
            }
            windows::Request::Remove { window_id, .. } => {
                self.remove_window(window_id)?;
                Ok(Value::Null)
            }
        }
    }

    fn apply_window(
        &mut self,
        id: i32,
        properties: &windows::Properties,
        permissions: &InstallPermissions,
    ) -> Result<(), ApiError> {
        let window = self
            .snapshot
            .windows
            .iter_mut()
            .find(|window| window.id == id)
            .ok_or_else(|| ApiError::other("No current window."))?;
        let old = window.bounds;
        for (index, value) in [
            properties.left,
            properties.top,
            properties.width,
            properties.height,
        ]
        .into_iter()
        .enumerate()
        {
            if let Some(value) = value {
                window.bounds[index] = value;
            }
        }
        if let Some(kind) = &properties.kind {
            window.kind = kind.clone();
        }
        if let Some(state) = &properties.state {
            window.state = state.clone();
        }
        if old != window.bounds {
            let window = window.clone();
            self.event(
                "windows.onBoundsChanged",
                json!([self.project_window(&self.snapshot, &window, false, permissions)]),
            );
        }
        Ok(())
    }

    fn record_visit(&mut self, url: &str) {
        let title = self
            .snapshot
            .tabs
            .iter()
            .find(|tab| tab.committed_url() == Some(url))
            .map(|tab| tab.tab.title().to_owned())
            .unwrap_or_default();
        self.history_records
            .push((self.next_visit, url.into(), title, self.time_ms));
        self.next_visit += 1;
    }

    fn history_request(
        &mut self,
        request: zephium_core::extension_data::HistoryRequest,
        method: &str,
    ) -> Result<Value, ApiError> {
        use zephium_core::extension_data::{
            ExtensionDataReply, HistoryEntry, HistoryReply, HistoryRequest, TopSite, VisitEntry,
        };
        let entries = |records: &[(i64, String, String, u64)]| {
            let mut by_url: BTreeMap<String, HistoryEntry> = BTreeMap::new();
            for (id, url, title, time) in records {
                let entry = by_url.entry(url.clone()).or_insert_with(|| HistoryEntry {
                    id: *id,
                    url: url.clone(),
                    title: title.clone(),
                    last_visit: (time / 1000) as i64,
                    visits: 0,
                });
                entry.visits += 1;
                entry.last_visit = entry.last_visit.max((time / 1000) as i64);
                entry.title = title.clone();
            }
            let mut entries: Vec<_> = by_url.into_values().collect();
            entries.sort_by_key(|entry| std::cmp::Reverse(entry.last_visit));
            entries
        };
        let reply = match request {
            HistoryRequest::Add { url } => {
                self.record_visit(&url);
                HistoryReply::Entries(
                    entries(&self.history_records)
                        .into_iter()
                        .filter(|entry| entry.url == url)
                        .collect(),
                )
            }
            HistoryRequest::Search {
                text,
                start,
                end,
                limit,
            } => {
                let words: Vec<_> = text
                    .to_lowercase()
                    .split_whitespace()
                    .map(str::to_owned)
                    .collect();
                let matching: Vec<_> = self
                    .history_records
                    .iter()
                    .filter(|(_, url, title, time)| {
                        let time = (time / 1000) as i64;
                        let text = format!("{url} {title}").to_lowercase();
                        time >= start && time < end && words.iter().all(|word| text.contains(word))
                    })
                    .cloned()
                    .collect();
                HistoryReply::Entries(
                    entries(&matching)
                        .into_iter()
                        .take(limit as usize)
                        .collect(),
                )
            }
            HistoryRequest::Visits { url } => {
                let entry = self
                    .history_records
                    .iter()
                    .find(|(_, address, _, _)| address == &url)
                    .map(|(id, _, _, _)| *id)
                    .unwrap_or(0);
                HistoryReply::Visits(
                    self.history_records
                        .iter()
                        .filter(|(_, address, _, _)| address == &url)
                        .map(|(id, _, _, time)| VisitEntry {
                            id: *id,
                            entry,
                            visited_at: (time / 1000) as i64,
                        })
                        .collect(),
                )
            }
            HistoryRequest::Address { url } => HistoryReply::Entries(
                entries(&self.history_records)
                    .into_iter()
                    .filter(|entry| entry.url == url)
                    .collect(),
            ),
            HistoryRequest::MostVisited { limit } => {
                let mut entries = entries(&self.history_records);
                entries.sort_by_key(|entry| std::cmp::Reverse(entry.visits));
                HistoryReply::Sites(
                    entries
                        .into_iter()
                        .take(limit as usize)
                        .map(|entry| TopSite {
                            url: entry.url,
                            title: entry.title,
                        })
                        .collect(),
                )
            }
            request @ (HistoryRequest::RemoveUrl { .. }
            | HistoryRequest::RemoveRange { .. }
            | HistoryRequest::RemoveAll) => {
                let before = self.history_records.len();
                let old: BTreeSet<_> = self
                    .history_records
                    .iter()
                    .map(|(_, url, _, _)| url.clone())
                    .collect();
                self.history_records
                    .retain(|(_, url, _, time)| match &request {
                        HistoryRequest::RemoveUrl { url: remove } => url != remove,
                        HistoryRequest::RemoveRange { start, end } => {
                            let time = (time / 1000) as i64;
                            time < *start || time >= *end
                        }
                        _ => false,
                    });
                let kept: BTreeSet<_> = self
                    .history_records
                    .iter()
                    .map(|(_, url, _, _)| url.clone())
                    .collect();
                let removed: Vec<_> = old.difference(&kept).cloned().collect();
                let all = matches!(request, HistoryRequest::RemoveAll);
                self.event(
                    "history.onVisitRemoved",
                    crate::api::history::removed(&removed, all),
                );
                HistoryReply::Removed {
                    visits: (before - self.history_records.len()) as u32,
                    urls: removed,
                    all,
                }
            }
        };
        let reply = ExtensionDataReply::History(reply);
        if method == "topSites.get" {
            crate::api::top_sites::reply(&reply)
        } else {
            crate::api::history::reply(method, &reply)
        }
    }

    fn core(
        &mut self,
        action: CoreAction,
        permissions: &InstallPermissions,
    ) -> Result<Value, ApiError> {
        let id = |this: &Self, native: ItemId| {
            this.snapshot
                .tabs
                .iter()
                .find(|tab| tab.tab.id() == native)
                .map(|tab| tab.id)
                .ok_or_else(ApiError::context_invalidated)
        };
        match action {
            action @ CoreAction::CreateTab { .. } => {
                let CoreAction::CreateTab {
                    window,
                    url,
                    active,
                } = &action
                else {
                    unreachable!()
                };
                let window_id = window
                    .and_then(|native| {
                        self.snapshot
                            .windows
                            .iter()
                            .find(|window| window.native_id == native)
                            .map(|window| window.id)
                    })
                    .or(self.snapshot.current_window)
                    .ok_or_else(|| ApiError::other("No current window."))?;
                self.tabs(
                    tabs::Request::Create {
                        properties: tabs::CreateProperties {
                            window_id: Some(window_id),
                            url: url.as_deref().map(str::to_owned),
                            active: Some(*active),
                            ..Default::default()
                        },
                        action,
                    },
                    permissions,
                )
            }
            action @ CoreAction::DuplicateTab { tab } => {
                let tab_id = id(self, tab)?;
                self.tabs(tabs::Request::Duplicate { tab_id, action }, permissions)
            }
            action @ CoreAction::ReloadTab { tab } => {
                let tab_id = id(self, tab)?;
                self.tabs(
                    tabs::Request::Reload {
                        tab_id,
                        bypass_cache: false,
                        action,
                    },
                    permissions,
                )
            }
            action @ CoreAction::SetTabMuted { tab, muted } => {
                let tab_id = id(self, tab)?;
                self.tabs(
                    tabs::Request::Update {
                        tab_id,
                        properties: tabs::UpdateProperties {
                            muted: Some(muted),
                            ..Default::default()
                        },
                        actions: vec![action],
                    },
                    permissions,
                )
            }
            action @ CoreAction::SetTabZoom { tab, percent } => {
                let tab_id = id(self, tab)?;
                self.tabs(
                    tabs::Request::SetZoom {
                        tab_id,
                        factor: f64::from(percent.unwrap_or(100)) / 100.0,
                        action,
                    },
                    permissions,
                )
            }
            CoreAction::MoveTabs { tabs, before } => {
                let ids = tabs
                    .iter()
                    .map(|tab| id(self, *tab))
                    .collect::<Result<Vec<_>, _>>()?;
                let first = *ids.first().ok_or_else(ApiError::malformed_arguments)?;
                let window = self.snapshot.tab(first)?.window_id;
                let index = before
                    .and_then(|before| {
                        self.snapshot
                            .tabs
                            .iter()
                            .find(|tab| tab.tab.id() == before)
                            .map(|tab| tab.index as i32)
                    })
                    .unwrap_or(-1);
                self.move_tabs(&ids, window, index)?;
                Ok(Value::Null)
            }
            CoreAction::CloseTabIfUnchanged {
                tab, url, cleanup, ..
            } => {
                let tab_id = id(self, tab)?;
                if cleanup.is_owned()
                    && self.snapshot.tab(tab_id)?.committed_url() == Some(url.as_ref())
                {
                    self.remove_tab(tab_id, false)?;
                }
                Ok(Value::Null)
            }
            CoreAction::ActivateTab { tab } => {
                self.activate(id(self, tab)?)?;
                Ok(Value::Null)
            }
            CoreAction::CloseTab { tab } | CoreAction::CloseTabIfPristine { tab } => {
                self.remove_tab(id(self, tab)?, false)?;
                Ok(Value::Null)
            }
            CoreAction::LoadTabUrl { tab, url } => {
                self.navigate(id(self, tab)?, &url, permissions)?;
                Ok(Value::Null)
            }
            CoreAction::GoBack { tab } => {
                self.traverse(id(self, tab)?, false, permissions)?;
                Ok(Value::Null)
            }
            CoreAction::GoForward { tab } => {
                self.traverse(id(self, tab)?, true, permissions)?;
                Ok(Value::Null)
            }
            _ => Err(unsupported()),
        }
    }
}

impl BrowserPort for FakeBrowser {
    fn snapshot(&self, sender: &Sender) -> Result<BrowserSnapshot, ApiError> {
        if sender.profile() != self.snapshot.profile {
            return Err(ApiError::cannot_access_page());
        }
        Ok(self.snapshot.clone())
    }
    fn extension_info(&self, _: &Sender) -> Result<ExtensionInfo, ApiError> {
        Ok(self.info.clone())
    }
    fn request(&mut self, request: BrowserRequest, invocation: Invocation<'_>) -> Handled {
        self.requests.push(request.clone());
        self.caller = Some(invocation.sender().clone());
        let result = match request {
            BrowserRequest::Tabs(request) => self.tabs(request, invocation.permissions()),
            BrowserRequest::Windows(request) => self.windows(request, invocation.permissions()),
            BrowserRequest::Core(action) => self.core(action, invocation.permissions()),
            BrowserRequest::Action(request) => self.action.handle(&request),
            BrowserRequest::Data(zephium_core::extension_data::ExtensionDataCall::History(
                request,
            )) if self.reply == Ok(Value::Null) => self.history_request(request, invocation.name()),
            _ => self.reply.clone(),
        };
        self.caller = None;
        invocation.reply(result)
    }
    fn frames(
        &self,
        _: &Sender,
        tab: ItemId,
    ) -> Result<Vec<crate::api::web_navigation::BrowserFrame>, ApiError> {
        Ok(self
            .frames
            .iter()
            .filter(|frame| frame.tab == tab)
            .cloned()
            .collect())
    }
    fn user_gesture(&self, _: &Sender) -> bool {
        self.gesture
    }
    fn validate_resource(&self, _: &Sender, path: &str) -> Result<(), ApiError> {
        if self.resources.contains(path) {
            Ok(())
        } else {
            Err(ApiError::other("Could not load file."))
        }
    }
    fn now_ms(&self) -> u64 {
        self.time_ms
    }
}
