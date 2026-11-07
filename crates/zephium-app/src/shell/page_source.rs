//! View Source: the document a page is showing, opened read-only in a tab of
//! its own. The text comes from the engine's HTML extraction, which serializes
//! the live document, and is only ever shown as text: it is never loaded, so
//! nothing in it can run.

use super::*;

use crate::api::{PageSource, SourceCompletion};

/// Documents kept for source tabs. A tab that outlives its document shows
/// that it is gone.
const MAX_DOCUMENTS: usize = 24;
const MAX_DOCUMENT_BYTES: usize = 48 * 1024 * 1024;
/// Longest tab title taken from the page's address.
const MAX_TITLE_CHARS: usize = 80;

#[derive(Default)]
pub(super) struct SourceState {
    /// The tab whose source was asked for last; an answer for any other is
    /// stale. A newer request replaces an older one still waiting.
    awaiting: Option<ItemId>,
    documents: std::collections::VecDeque<(ItemId, Arc<PageSource>)>,
}

impl SourceState {
    fn keep(&mut self, id: ItemId, document: Arc<PageSource>, tabs: impl Fn(ItemId) -> bool) {
        self.documents.retain(|(held, _)| tabs(*held));
        self.documents.push_back((id, document));
        let mut bytes: usize = self.documents.iter().map(|(_, d)| d.text.len()).sum();
        while self.documents.len() > 1
            && (self.documents.len() > MAX_DOCUMENTS || bytes > MAX_DOCUMENT_BYTES)
        {
            if let Some((_, oldest)) = self.documents.pop_front() {
                bytes -= oldest.text.len();
            }
        }
    }

    fn get(&self, id: ItemId) -> Option<Arc<PageSource>> {
        self.documents
            .iter()
            .find(|(held, _)| *held == id)
            .map(|(_, document)| document.clone())
    }
}

/// The tab title for a page's source: its address in `view-source:` form,
/// which is the same in every language.
fn source_title(url: &url::Url) -> String {
    let address = url.as_str();
    let address = address.split_once("://").map_or(address, |(_, rest)| rest);
    let address = address.split(['?', '#']).next().unwrap_or(address);
    let shown: String = address.chars().take(MAX_TITLE_CHARS).collect();
    format!("view-source:{shown}")
}

impl Shell {
    /// Asks the page in front for its source. The tab opens when the engine
    /// answers.
    pub(super) fn operation_view_source(&mut self) -> OperationDisposition {
        let id = match self.loaded_page_in_front() {
            Ok(id) => id,
            Err(refused) => return refused,
        };
        self.sources.awaiting = Some(id);
        let mut native = NativeWork::default();
        native.record(self.engine.extract_html(id));
        if native.rejected {
            self.sources.awaiting = None;
        }
        mutation_result(native)
    }

    /// The page the person is looking at, when it is a loaded web page they
    /// read: the only thing that can be viewed as source or saved.
    fn loaded_page_in_front(&self) -> Result<ItemId, OperationDisposition> {
        let Some(id) = self
            .windows
            .focused()
            .and_then(|window| window.active)
            .filter(|_| self.active_browser_page().is_none())
        else {
            return Err(operation_result(
                OperationOutcome::NoOp,
                OperationReason::NoFocusedWindow,
            ));
        };
        let loaded = self.item_in_focused_scope(id)
            && self.items.tab(id).is_some_and(|tab| {
                tab.content == zephium_core::item::TabContent::Web
                    && tab.has_view()
                    && tab.url.as_ref().is_some_and(navigation::is_allowed)
            });
        if loaded {
            Ok(id)
        } else {
            Err(operation_result(
                OperationOutcome::NoOp,
                OperationReason::InvalidScope,
            ))
        }
    }

    /// Saves the page in front through the engine's own Save As.
    pub(super) fn operation_save_page(&mut self) -> OperationDisposition {
        let id = match self.loaded_page_in_front() {
            Ok(id) => id,
            Err(refused) => return refused,
        };
        let mut native = NativeWork::default();
        native.record(self.engine.save_page(id));
        mutation_result(native)
    }

    /// Opens the system's saved passwords at the field in focus on the page
    /// in front. Private windows too: the passwords are the system's, and
    /// nothing is kept by Zephium.
    pub(super) fn operation_autofill_password(&mut self) -> OperationDisposition {
        let id = match self.loaded_page_in_front() {
            Ok(id) => id,
            Err(refused) => return refused,
        };
        let mut native = NativeWork::default();
        native.record(self.engine.autofill_password(id));
        mutation_result(native)
    }

    pub(super) fn on_html_extracted(&mut self, id: ItemId, html: String, truncated: bool) {
        if self.sources.awaiting != Some(id) {
            return;
        }
        self.sources.awaiting = None;
        let Some(url) = self
            .items
            .tab(id)
            .filter(|tab| tab.content == zephium_core::item::TabContent::Web)
            .and_then(|tab| tab.url.clone())
            .filter(navigation::is_allowed)
        else {
            return;
        };
        if !self.item_in_focused_scope(id) {
            return;
        }
        let document = Arc::new(PageSource {
            url: url.to_string(),
            text: html,
            truncated,
        });
        let title = source_title(&url);
        let _ = self.open_source_tab(&title, document);
    }

    fn open_source_tab(&mut self, title: &str, document: Arc<PageSource>) -> OperationDisposition {
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
        let previous_active = self.windows.focused().and_then(|window| window.active);
        let previous_page = self.browser_page;
        let previous_recent = self.residency.recent.clone();
        let id = ItemId::generate();
        if !self.items.insert_source_tab(
            id,
            Placement::Space {
                space,
                section: SpaceSection::Today,
            },
            title,
        ) {
            return operation_result(
                OperationOutcome::Rejected,
                OperationReason::ItemLimitReached,
            );
        }
        let items = &self.items;
        self.sources
            .keep(id, document, |held| held == id || items.tab(held).is_some());
        self.browser_page = None;
        self.browser_return = None;
        self.browser_after_return = None;
        let effects = self.focus_tab(id);
        let native = self.commit(effects);
        if native.rejected {
            let _ = self.items.remove(id);
            self.sources.documents.retain(|(held, _)| *held != id);
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

    /// The document a source tab shows, if the tab is one of the focused
    /// window's and the document is still held.
    pub(super) fn page_source(&mut self, id: ItemId, done: SourceCompletion) {
        let is_source = self.item_in_focused_scope(id)
            && self.items.tab(id).is_some_and(|tab| {
                tab.content
                    == zephium_core::item::TabContent::BrowserOwned(
                        zephium_core::item::BrowserOwnedTab::Source,
                    )
            });
        done.finish(if is_source {
            self.sources.get(id)
        } else {
            None
        });
    }
}
