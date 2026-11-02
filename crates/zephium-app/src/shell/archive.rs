//! Today tabs left unused for the person's chosen time close into History,
//! as closing them by hand does. Checked at launch and on the existing
//! maintenance tick; there is no timer of its own.

use super::*;

impl Shell {
    pub(super) fn archive_idle_tabs(&mut self) {
        let Some(after) = self.tab_preferences.archive_after.millis() else {
            return;
        };
        let Some(now) = tabs::unix_millis() else {
            return;
        };
        let in_front: std::collections::HashSet<ItemId> = self
            .windows
            .iter()
            .flat_map(|window| {
                window
                    .active
                    .into_iter()
                    .chain(window.splits.iter().flat_map(|tree| tree.tabs()))
            })
            .chain(self.work_pane.as_ref().map(|pane| pane.tab))
            .collect();
        let spaces: Vec<SpaceId> = self
            .spaces
            .iter()
            .filter(|space| {
                self.profiles
                    .get(space.profile)
                    .is_some_and(|profile| profile.kind != ProfileKind::Incognito)
            })
            .map(|space| space.id)
            .collect();
        let mut idle = Vec::new();
        for space in spaces {
            // Roots only: a tab inside a folder was put there to keep.
            for id in self.today_tabs(space) {
                let since = self.items.tab(id).and_then(|tab| tab.last_active_ms);
                match since {
                    // Clocks restart from first sight, and in front is in use.
                    None => self.items.set_last_active(id, now),
                    Some(_) if in_front.contains(&id) => self.items.set_last_active(id, now),
                    Some(since) if now.saturating_sub(since) >= after && self.archivable(id) => {
                        idle.push(id)
                    }
                    Some(_) => {}
                }
            }
        }
        for id in idle {
            self.archive_tab(id, now);
        }
    }

    /// Never a page that is busy, asking for something, capturing, or one
    /// the engine refused to discard because a reload would lose its state.
    fn archivable(&self, id: ItemId) -> bool {
        self.items.tab(id).is_some_and(|tab| {
            tab.content == zephium_core::item::TabContent::Web
                && tab.url.is_some()
                && !tab.loading
                && tab.page_request.is_none()
                && tab.capture.is_none()
        }) && !self.residency.discard_protected_until.contains_key(&id)
            && !self.residency.discard_probes.contains_key(&id)
    }

    fn archive_tab(&mut self, id: ItemId, now: u64) {
        // Closing in the focused space records the tab itself.
        let record = (!self.item_in_focused_scope(id))
            .then(|| self.closed_record(id, Some(now)))
            .flatten();
        self.close_in_any_space(id);
        if let Some(record) = record {
            self.remember_closed(record);
        }
    }
}
