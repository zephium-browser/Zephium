//! Browser-owned pages borrow chrome's full-window presentation, never a page bridge.
use super::*;

pub(super) struct PendingBrowserReturn {
    revision: u64,
    window: WindowId,
    items: ItemsState,
    splits: Option<Pane>,
}

fn restored_tab_matches_snapshot(old: &TabView, tab: &TabState) -> bool {
    let same_owner = matches!(
        (old.content, tab.content),
        (
            zephium_ipc::TabContentView::Web,
            zephium_core::item::TabContent::Web
        ) | (
            zephium_ipc::TabContentView::Settings,
            zephium_core::item::TabContent::BrowserOwned(
                zephium_core::item::BrowserOwnedTab::Settings
            )
        ) | (
            zephium_ipc::TabContentView::Extensions,
            zephium_core::item::TabContent::BrowserOwned(
                zephium_core::item::BrowserOwnedTab::Extensions
            )
        ) | (
            zephium_ipc::TabContentView::ExtensionOwned,
            zephium_core::item::TabContent::ExtensionOwned
        )
    );
    let same_url = tab.url.as_ref().map(url::Url::as_str) == old.url.as_deref();
    // A native extension document may publish its title while privileged
    // chrome is acknowledging the exact tab graph. Its identity, URL-less
    // owner, profile, active item and split topology still have to match.
    let guest_title_update = old.content == zephium_ipc::TabContentView::ExtensionOwned
        && old.url.is_none()
        && tab.url.is_none();
    same_owner && same_url && (tab.title == old.title || guest_title_update)
}

impl Shell {
    pub(super) fn active_browser_page(&self) -> Option<crate::BrowserPage> {
        let window = self.windows.focused()?.id;
        self.browser_page
            .filter(|(owner, _)| *owner == window)
            .map(|(_, page)| page)
            .or_else(|| {
                self.windows
                    .focused()
                    .and_then(|focused| focused.active)
                    .and_then(|id| self.items.tab(id))
                    .and_then(|tab| match tab.content {
                        zephium_core::item::TabContent::BrowserOwned(
                            zephium_core::item::BrowserOwnedTab::Settings,
                        ) => Some(crate::BrowserPage::Settings),
                        zephium_core::item::TabContent::BrowserOwned(
                            zephium_core::item::BrowserOwnedTab::Extensions,
                        ) => Some(crate::BrowserPage::Extensions),
                        zephium_core::item::TabContent::Web
                        | zephium_core::item::TabContent::ExtensionOwned => None,
                    })
            })
    }

    pub(super) fn project_browser_page(&mut self) {
        self.browser_page_projected = self
            .windows
            .focused()
            .map(|window| (window.id, self.active_browser_page()));
        let id = self
            .active_browser_page()
            .map_or("browser.return", crate::BrowserPage::command_id);
        (self.emit)(Projection::UiCommand(id.into()));
    }

    pub(super) fn project_browser_page_if_changed(&mut self) {
        let current = self
            .windows
            .focused()
            .map(|window| (window.id, self.active_browser_page()));
        if current != self.browser_page_projected {
            self.project_browser_page();
        }
    }

    fn open_extensions_tab(&mut self) -> OperationDisposition {
        let page = zephium_core::item::BrowserOwnedTab::Extensions;
        let Some((profile, space)) = self
            .windows
            .focused()
            .map(|window| (window.profile, window.space))
        else {
            return operation_result(OperationOutcome::Rejected, OperationReason::NoFocusedWindow);
        };
        if self.profile_deletion_quarantines(profile) {
            return operation_result(OperationOutcome::Rejected, OperationReason::InvalidScope);
        }
        let existing = [SpaceSection::Pinned, SpaceSection::Today]
            .into_iter()
            .flat_map(|section| {
                self.items
                    .roots(Placement::Space { space, section })
                    .iter()
                    .copied()
            })
            .find(|id| {
                self.items.tab(*id).is_some_and(|tab| {
                    tab.content == zephium_core::item::TabContent::BrowserOwned(page)
                })
            });
        let previous_active = self.windows.focused().and_then(|window| window.active);
        let previous_page = self.browser_page;
        let previous_recent = self.residency.recent.clone();
        let created = existing.is_none();
        let id = if let Some(id) = existing {
            id
        } else {
            let id = ItemId::generate();
            if !self.items.insert_browser_tab(
                id,
                Placement::Space {
                    space,
                    section: SpaceSection::Today,
                },
                page,
            ) {
                return operation_result(
                    OperationOutcome::Rejected,
                    OperationReason::ItemLimitReached,
                );
            }
            id
        };
        self.browser_page = None;
        self.browser_return = None;
        self.browser_after_return = None;
        let effects = self.focus_tab(id);
        let native = self.commit(effects);
        if native.rejected {
            if created {
                let _ = self.items.remove(id);
            } else {
                self.items.set_lifecycle(id, Lifecycle::Inactive);
            }
            if let Some(window) = self.windows.focused_mut() {
                window.active = previous_active;
            }
            if let Some(previous) = previous_active {
                self.items.set_lifecycle(previous, Lifecycle::Active);
            }
            self.residency.recent = previous_recent;
            self.browser_page = previous_page;
            self.schedule_persist();
            self.project_items();
            self.project_browser_page();
        }
        mutation_result(native)
    }

    pub(super) fn operation_show_browser_page(
        &mut self,
        page: Option<crate::BrowserPage>,
    ) -> OperationDisposition {
        if page == Some(crate::BrowserPage::Work)
            && self.store.app_setting("work.enabled").as_deref() == Some("false")
        {
            return operation_result(OperationOutcome::Rejected, OperationReason::InvalidScope);
        }
        if let Some(crate::BrowserPage::Extensions) = page {
            return self.open_extensions_tab();
        }
        let Some(window) = self.windows.focused().map(|window| window.id) else {
            return operation_result(OperationOutcome::Rejected, OperationReason::NoFocusedWindow);
        };
        if page.is_none() && self.active_browser_page().is_some() {
            if self.browser_page.is_none()
                && self
                    .windows
                    .focused()
                    .and_then(|window| window.active)
                    .and_then(|id| self.items.tab(id))
                    .is_some_and(|tab| {
                        matches!(tab.content, zephium_core::item::TabContent::BrowserOwned(_))
                    })
            {
                let fallback = self
                    .residency
                    .recent
                    .iter()
                    .rev()
                    .copied()
                    .find(|id| {
                        self.item_in_focused_scope(*id)
                            && self.items.tab(*id).is_some_and(|tab| {
                                !matches!(
                                    tab.content,
                                    zephium_core::item::TabContent::BrowserOwned(_)
                                )
                            })
                    })
                    .or_else(|| {
                        self.windows.focused().and_then(|window| {
                            [SpaceSection::Today, SpaceSection::Pinned]
                                .into_iter()
                                .flat_map(|section| {
                                    self.items
                                        .roots(Placement::Space {
                                            space: window.space,
                                            section,
                                        })
                                        .iter()
                                        .copied()
                                })
                                .find(|id| {
                                    self.items.tab(*id).is_some_and(|tab| {
                                        tab.content == zephium_core::item::TabContent::Web
                                    })
                                })
                        })
                    });
                if self.browser_after_return.is_none() {
                    self.browser_after_return =
                        Some(Box::new(fallback.map_or(Command::Open, Command::Activate)));
                }
            }
            return self.request_browser_return();
        }
        self.browser_return = None;
        self.browser_after_return = None;
        let previous = self.browser_page;
        self.browser_page = page.map(|page| (window, page));
        self.drop_divider();
        if page != Some(crate::BrowserPage::Work) {
            self.retire_work_pane();
        }
        if self.relayout() != NativeDispatch::Scheduled {
            self.browser_page = previous;
            let _ = self.relayout();
            return operation_result(
                OperationOutcome::NativeAdmissionFailed,
                OperationReason::NativeDispatchRejected,
            );
        }
        self.project_browser_page();
        // NativeDispatch is scheduling admission, not proof of completed native geometry.
        operation_result(
            OperationOutcome::Deferred,
            OperationReason::NativeWorkPending,
        )
    }
    pub(super) fn request_browser_return(&mut self) -> OperationDisposition {
        if self.browser_return.is_some() {
            return operation_result(
                OperationOutcome::Deferred,
                OperationReason::NativeWorkPending,
            );
        }

        let Some(items) = self.items_snapshot() else {
            return operation_result(OperationOutcome::Rejected, OperationReason::NoFocusedWindow);
        };
        let Some((window, splits)) = self.windows.focused().map(|w| (w.id, w.splits.clone()))
        else {
            return operation_result(OperationOutcome::Rejected, OperationReason::NoFocusedWindow);
        };
        let Some(revision) = self.browser_return_revision.checked_add(1) else {
            return operation_result(OperationOutcome::Rejected, OperationReason::InvalidScope);
        };
        self.browser_return_revision = revision;
        // The pane's page must leave the native stage before Browse chrome is
        // restored underneath it.
        if self.retire_work_pane() {
            let _ = self.relayout();
        }
        self.browser_return = Some(PendingBrowserReturn {
            revision,
            window,
            items: items.clone(),
            splits,
        });
        let callback = self.self_queue.as_ref().map(|queue| CallbackHandle {
            queue: Arc::downgrade(&queue.inner),
        });
        let dispatch = self.chrome.restore_browser_chrome(
            revision,
            items,
            Box::new(move |applied| {
                if let Some(callback) = callback {
                    let _ = callback.dispatch(Command::BrowserChromeRestored { revision, applied });
                }
            }),
        );
        match dispatch {
            ChromePresentationDispatch::Applied => self.browser_chrome_restored(revision, true),
            ChromePresentationDispatch::Rejected => self.browser_chrome_restored(revision, false),
            ChromePresentationDispatch::Scheduled => {}
        }
        operation_result(
            OperationOutcome::Deferred,
            OperationReason::NativeWorkPending,
        )
    }

    pub(super) fn browser_chrome_restored(&mut self, revision: u64, applied: bool) {
        let Some(PendingBrowserReturn {
            revision: expected,
            window,
            items,
            splits,
        }) = self.browser_return.as_ref()
        else {
            return;
        };
        if *expected != revision {
            return;
        }
        let exact = self.windows.focused().is_some_and(|current| {
            current.id == *window
                && current.active.map(|id| id.to_string()) == items.active
                && Some(current.space.to_string()) == items.active_space_id
                && items
                    .profile
                    .as_ref()
                    .is_some_and(|profile| profile.id == current.profile.to_string())
                && items.tabs.iter().all(|old| {
                    ItemId::parse(&old.id)
                        .and_then(|id| self.items.tab(id))
                        .is_some_and(|tab| restored_tab_matches_snapshot(old, tab))
                })
                // Compare native state with its native snapshot. The public
                // projection intentionally omits retained single-leaf trees.
                && &current.splits == splits
        });
        self.browser_return = None;
        if !applied || !exact {
            self.browser_after_return = None;
            self.project_browser_page();
            (self.emit)(Projection::UiCommand("browser.return-failed".into()));
            return;
        }
        let previous = self.browser_page.take();
        // The page the browser page covered comes back into view rather
        // than reappearing in a single frame.
        if let Some(window) = self.windows.focused().map(|window| window.id) {
            let _ = self.engine.hint_stage_motion(window, StageMotion::Arrive);
        }
        if self.relayout() != NativeDispatch::Scheduled {
            self.browser_page = previous;
            let _ = self.relayout();
            self.project_browser_page();
            (self.emit)(Projection::UiCommand("browser.return-failed".into()));
        } else {
            if let Some(next) = self.browser_after_return.take() {
                self.browser_return_ready = true;
                self.handle(*next);
                self.browser_return_ready = false;
                self.project_browser_page_if_changed();
            } else {
                self.project_browser_page();
            }
        }
    }
}
