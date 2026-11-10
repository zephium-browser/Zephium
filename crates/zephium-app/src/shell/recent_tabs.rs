use super::*;
use std::collections::HashMap;
use zephium_ipc::{TabSwitcherAction, TabSwitcherEntry, TabSwitcherState};

#[derive(Default)]
pub(super) struct RecentTabs {
    windows: HashMap<WindowId, Vec<ItemId>>,
    active: HashMap<WindowId, ItemId>,
    session: Option<Session>,
}

struct Session {
    id: u64,
    window: WindowId,
    profile: ProfileId,
    space: SpaceId,
    tabs: Vec<ItemId>,
    selected: usize,
    keyboard: bool,
    visible: bool,
}

impl Session {
    fn step(&mut self, reverse: bool) {
        if !self.tabs.is_empty() {
            self.selected =
                (self.selected + if reverse { self.tabs.len() - 1 } else { 1 }) % self.tabs.len();
        }
    }
    fn retain(&mut self, mut keep: impl FnMut(ItemId) -> bool) {
        let selected = self.tabs.get(self.selected).copied();
        self.tabs.retain(|id| keep(*id));
        self.selected = selected
            .and_then(|id| self.tabs.iter().position(|tab| *tab == id))
            .unwrap_or(self.selected.min(self.tabs.len().saturating_sub(1)));
    }
}

impl RecentTabs {
    pub(super) fn forget(&mut self, id: ItemId) {
        for order in self.windows.values_mut() {
            order.retain(|tab| *tab != id);
        }
        self.active.retain(|_, active| *active != id);
    }
}

impl Shell {
    pub(super) fn capture_departed_tab_preview(&mut self) {
        let Some(window) = self.windows.focused() else {
            return;
        };
        let Some(previous) = self.recent_tabs.active.get(&window.id).copied() else {
            return;
        };
        if window.active != Some(previous) {
            let (id, active) = (window.id, window.active);
            if tab_previews::CAPTURES_ONLY_ON_SCREEN {
                // The tab left is hidden by now; the one switched to is not.
                if let Some(active) = active {
                    self.queue_tab_preview(id, active, true);
                }
            } else {
                self.queue_tab_preview(id, previous, false);
            }
            self.capture_tab_preview();
        }
    }

    pub(super) fn reconcile_recent_tabs(&mut self) {
        self.recent_tabs
            .windows
            .retain(|window, _| self.windows.get(*window).is_some());
        self.recent_tabs
            .active
            .retain(|window, _| self.windows.get(*window).is_some());
        self.tab_previews
            .retain_windows(|window| self.windows.get(window).is_some());
        self.tab_previews
            .retain_tabs(|tab| self.items.tab(tab).is_some());
        for window in self.windows.iter() {
            let order = self.recent_tabs.windows.entry(window.id).or_default();
            order.retain(|id| self.items.tab(*id).is_some());
            if let Some(active) = window.active {
                if self.recent_tabs.active.get(&window.id) != Some(&active) {
                    self.recent_tabs.active.insert(window.id, active);
                    order.retain(|id| *id != active);
                    order.insert(0, active);
                }
            }
        }
        let Some(mut session) = self.recent_tabs.session.take() else {
            return;
        };
        if !self.windows.focused().is_some_and(|window| {
            window.id == session.window
                && window.profile == session.profile
                && window.space == session.space
        }) {
            (self.emit)(Projection::TabSwitcher {
                session: session.id,
                state: None,
            });
            return;
        }
        session.retain(|id| self.switchable_tab(id));
        if session.tabs.is_empty() && !session.visible {
            (self.emit)(Projection::TabSwitcher {
                session: session.id,
                state: None,
            });
        } else {
            self.recent_tabs.session = Some(session);
            self.project_tab_switcher();
        }
    }

    /// Every page of the focused space can be switched to: one asleep wakes
    /// as it is chosen, as it would from the sidebar, and a browser or
    /// extension page goes in without a preview. A blank web tab has no page
    /// to go back to.
    fn switchable_tab(&self, id: ItemId) -> bool {
        self.item_in_focused_scope(id)
            && self.items.tab(id).is_some_and(|tab| {
                tab.has_view()
                    || tab.url.is_some()
                    || tab.content != zephium_core::item::TabContent::Web
            })
    }

    /// Where a step starts: on the active tab when it heads the order, else
    /// just before the order, so the first step forward lands on its head.
    fn first_step(&self, tabs: &[ItemId], reverse: bool) -> Option<usize> {
        let current = self.windows.focused().and_then(|window| window.active);
        match (
            tabs.first().copied() == current && current.is_some(),
            reverse,
        ) {
            (_, _) if tabs.is_empty() => None,
            (true, _) if tabs.len() < 2 => None,
            (true, false) => Some(1),
            (_, true) => Some(tabs.len() - 1),
            (false, false) => Some(0),
        }
    }

    pub(super) fn recent_tab_order(&self) -> Vec<ItemId> {
        let Some(window) = self.windows.focused() else {
            return Vec::new();
        };
        let mut order: Vec<_> = self
            .recent_tabs
            .windows
            .get(&window.id)
            .into_iter()
            .flatten()
            .copied()
            .filter(|id| self.switchable_tab(*id))
            .collect();
        for id in self.keyboard_tabs(window.space) {
            if self.switchable_tab(id) && !order.contains(&id) {
                order.push(id);
            }
        }
        order
    }

    pub(super) fn operation_recent_tab(&mut self, reverse: bool) -> OperationDisposition {
        let tabs = self.recent_tab_order();
        let Some(index) = self.first_step(&tabs, reverse) else {
            return operation_result(OperationOutcome::NoOp, OperationReason::StateUnchanged);
        };
        self.operation_activate(tabs[index])
    }

    pub(super) fn begin_tab_switcher(&mut self, id: u64, reverse: bool, keyboard: bool) {
        let Some(window) = self.windows.focused() else {
            (self.emit)(Projection::TabSwitcher {
                session: id,
                state: None,
            });
            return;
        };
        let tabs = self.recent_tab_order();
        let first = self.first_step(&tabs, reverse);
        let mut session = Session {
            id,
            window: window.id,
            profile: window.profile,
            space: window.space,
            tabs,
            selected: 0,
            keyboard,
            visible: !keyboard,
        };
        if keyboard {
            session.selected = first.unwrap_or(0);
        }
        // Opened by hand it shows even with nothing to switch to, for its new
        // tab; a keyboard gesture with nothing to go to does nothing.
        if session.tabs.is_empty() && keyboard {
            (self.emit)(Projection::TabSwitcher {
                session: id,
                state: None,
            });
            return;
        }
        self.recent_tabs.session = Some(session);
        if !keyboard {
            self.reveal_tab_switcher(id);
        }
    }

    pub(super) fn reveal_tab_switcher(&mut self, id: u64) {
        let Some(session) = self.recent_tabs.session.as_mut().filter(|s| s.id == id) else {
            return;
        };
        session.visible = true;
        let window = session.window;
        if let Some(active) = self.windows.get(window).and_then(|window| window.active) {
            // Where only a tab on screen captures, the one in front is the
            // only one that can be brought up to date.
            self.queue_tab_preview(window, active, !tab_previews::CAPTURES_ONLY_ON_SCREEN);
        }
        self.project_tab_switcher();
    }

    pub(super) fn tab_switcher_action(&mut self, id: u64, action: TabSwitcherAction) {
        if self.recent_tabs.session.as_ref().is_none_or(|s| s.id != id) {
            return;
        }
        let Some(mut session) = self.recent_tabs.session.take() else {
            return;
        };
        if !self.windows.focused().is_some_and(|w| {
            w.id == session.window && w.profile == session.profile && w.space == session.space
        }) {
            (self.emit)(Projection::TabSwitcher {
                session: id,
                state: None,
            });
            return;
        }
        session.retain(|tab| self.switchable_tab(tab));
        // A tab named by its id is the one meant, whatever the selection has
        // become while the intent was on its way; one no longer listed is
        // ignored rather than read as another.
        let named = |tab_id: &str| ItemId::parse(tab_id).filter(|tab| session.tabs.contains(tab));
        match action {
            TabSwitcherAction::Commit
            | TabSwitcherAction::Cancel
            | TabSwitcherAction::NewTab
            | TabSwitcherAction::Activate { .. } => {
                let target = match &action {
                    TabSwitcherAction::Commit => session.tabs.get(session.selected).copied(),
                    TabSwitcherAction::Activate { tab_id } => named(tab_id),
                    _ => None,
                };
                if let Some(tab) = target {
                    let _ = self.operation_activate(tab);
                }
                (self.emit)(Projection::TabSwitcher {
                    session: id,
                    state: None,
                });
                if action == TabSwitcherAction::NewTab {
                    let _ = self.operation_open();
                }
                return;
            }
            TabSwitcherAction::CloseTab { tab_id } => {
                if let Some(tab) = named(&tab_id) {
                    let _ = self.operation_close(tab);
                }
                session.retain(|tab| self.switchable_tab(tab));
            }
            TabSwitcherAction::Step { reverse } => session.step(reverse),
            TabSwitcherAction::Select { index } => {
                if (index as usize) < session.tabs.len() {
                    session.selected = index as usize;
                }
            }
            TabSwitcherAction::Close => {
                if let Some(tab) = session.tabs.get(session.selected).copied() {
                    let _ = self.operation_close(tab);
                }
                session.retain(|tab| self.switchable_tab(tab));
            }
        }
        if session.tabs.is_empty() && !session.visible {
            (self.emit)(Projection::TabSwitcher {
                session: id,
                state: None,
            });
        } else {
            self.recent_tabs.session = Some(session);
            self.project_tab_switcher();
        }
    }

    pub(super) fn project_tab_switcher(&self) {
        let Some(session) = self.recent_tabs.session.as_ref().filter(|s| s.visible) else {
            return;
        };
        let active = self.windows.get(session.window).and_then(|w| w.active);
        self.select_icon_profile(zephium_ipc::IconSurface::Panel, session.profile);
        let entries = session
            .tabs
            .iter()
            .filter_map(|id| {
                let tab = self.items.tab(*id)?;
                Some(TabSwitcherEntry {
                    tab_id: id.to_string(),
                    title: tab.title.clone(),
                    url_host: tab
                        .url
                        .as_ref()
                        .and_then(|url| url.host_str())
                        .unwrap_or_default()
                        .into(),
                    favicon: self.icon_ref(
                        zephium_ipc::IconSurface::Panel,
                        tab,
                        Some(session.profile),
                    ),
                    preview_url: self.tab_previews.url(
                        session.window,
                        *id,
                        tab.url.as_ref().map(|url| url.as_str()),
                    ),
                    active: active == Some(*id),
                })
            })
            .collect();
        self.publish_icons();
        (self.emit)(Projection::TabSwitcher {
            session: session.id,
            state: Some(TabSwitcherState {
                session_id: session.id.to_string(),
                window_id: session.window.to_string(),
                entries,
                selected: session.selected as u32,
                keyboard: session.keyboard,
            }),
        });
    }
}
