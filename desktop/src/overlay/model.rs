use zephium_ipc::{PanelState, SearchContext};
/// The launcher's corner. Windows draws its own window corners and rim at
/// the system radius, and the card has to agree with them.
pub const RADIUS: u16 = if cfg!(target_os = "windows") { 8 } else { 20 };
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Owner {
    pub private: bool,
    pub window: String,
    pub profile: String,
    pub name: String,
    pub space: String,
}
/// The presentation and native focus event a deferred reconciliation belongs to.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FocusChange {
    session: u64,
    revision: u64,
}
#[derive(Debug, Default)]
pub struct Model {
    revision: u64,
    publication: u64,
    focus_change: FocusChange,
    pub presented: bool,
    pub ready: bool,
    pub owner: Option<Owner>,
    pub error: bool,
    /// The tab whose address this presentation edits, and that address.
    editing: Option<(String, String)>,
    pub switcher: Option<zephium_ipc::TabSwitcherState>,
}
impl Model {
    pub fn session(&self) -> String {
        format!("{:016x}", self.revision)
    }
    /// The tab whose address this presentation edits.
    pub fn editing_tab(&self) -> Option<String> {
        self.editing.as_ref().map(|(tab, _)| tab.clone())
    }
    pub fn snapshot(&self) -> PanelState {
        PanelState {
            switcher: self.switcher.clone(),
            window_id: self.owner.as_ref().map(|owner| owner.window.clone()),
            revision: format!("{:016x}", self.publication),
            session_id: self.session(),
            visible: self.presented,
            profile_id: self.owner.as_ref().map(|o| o.profile.clone()),
            profile_name: self.owner.as_ref().map(|o| o.name.clone()),
            space_id: self.owner.as_ref().map(|o| o.space.clone()),
            error: self.error,
            corner_radius: RADIUS,
            seed: self.editing.as_ref().map(|(_, url)| url.clone()),
            seed_tab: self.editing_tab(),
        }
    }
    pub fn reject(&mut self) {
        self.error = true;
        if let Some(next) = self.publication.checked_add(1) {
            self.publication = next;
        } else {
            self.presented = false;
        }
    }
    pub fn clear_error(&mut self) {
        if self.error {
            self.error = false;
            if let Some(next) = self.publication.checked_add(1) {
                self.publication = next;
            } else {
                self.presented = false;
            }
        }
    }
    fn advance(&mut self) -> bool {
        match (
            self.revision.checked_add(1),
            self.publication.checked_add(1),
        ) {
            (Some(session), Some(publication)) => {
                self.revision = session;
                self.publication = publication;
                true
            }
            _ => {
                self.presented = false;
                self.error = true;
                false
            }
        }
    }
    pub fn search(&mut self) {
        if !self.advance() {
            return;
        }
        self.presented = true;
        self.error = false;
        self.editing = None;
        self.switcher = None;
    }
    /// Opens on a tab's address, so a chosen result replaces that tab's page.
    pub fn edit(&mut self, tab: String, url: String) {
        self.search();
        if self.presented {
            self.editing = Some((tab, url));
        }
    }
    pub fn show_switcher(&mut self, switcher: zephium_ipc::TabSwitcherState) {
        if self.switcher.as_ref().map(|s| &s.session_id) != Some(&switcher.session_id) {
            self.search();
        }
        self.publication = self.publication.saturating_add(1);
        self.switcher = Some(switcher);
    }
    pub fn hide(&mut self) {
        if !self.advance() {
            return;
        }
        self.presented = false;
        self.error = false;
        self.editing = None;
        self.switcher = None;
    }
    pub fn record_focus_change(&mut self) -> FocusChange {
        self.focus_change = FocusChange {
            session: self.revision,
            revision: self.focus_change.revision.saturating_add(1),
        };
        self.focus_change
    }
    pub fn latest_focus_change(&self) -> FocusChange {
        self.focus_change
    }
    /// A launcher is transient: it goes away when focus moves to the browser
    /// or to another application. An owned native surface, such as a menu or
    /// dialog, keeps Zephium active and does not count as leaving.
    pub fn focus(
        &mut self,
        observed: FocusChange,
        panel_focused: bool,
        main_focused: bool,
        app_active: bool,
    ) {
        // Recheck at mutation, after native focus queries and the UI queue.
        // A focus event from before this presentation cannot dismiss it.
        if observed != self.focus_change || observed.session != self.revision {
            return;
        }
        if self.presented && !panel_focused && (main_focused || !app_active) {
            self.hide();
        }
    }
    pub fn set_owner(&mut self, owner: Option<Owner>) {
        if self.owner == owner {
            return;
        }
        if !self.advance() {
            return;
        }
        self.presented = false;
        self.owner = owner;
        self.switcher = None;
    }
    pub fn context(&self, request_id: &str) -> Option<SearchContext> {
        if self.switcher.is_some()
            || !self.presented
            || request_id.is_empty()
            || request_id.len() > 64
            || !request_id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        {
            return None;
        }
        let owner = self.owner.as_ref()?;
        Some(SearchContext {
            window_id: owner.window.clone(),
            session_id: self.session(),
            request_id: request_id.into(),
            profile_id: owner.profile.clone(),
            space_id: owner.space.clone(),
        })
    }
    pub fn admits(&self, context: &SearchContext) -> bool {
        self.context(&context.request_id).as_ref() == Some(context)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn owner(window: &str, profile: &str, space: &str) -> Owner {
        Owner {
            private: false,
            window: window.into(),
            profile: profile.into(),
            name: "Personal".into(),
            space: space.into(),
        }
    }
    #[test]
    fn switcher_and_search_replace_each_other_and_hidden_state_releases_entries() {
        let mut model = Model::default();
        model.set_owner(Some(owner("one", "p", "s")));
        model.search();
        let context = model.context("1").unwrap();
        model.show_switcher(zephium_ipc::TabSwitcherState {
            session_id: "1".into(),
            window_id: "one".into(),
            entries: Vec::new(),
            selected: 0,
            keyboard: true,
        });
        assert!(model.presented);
        assert!(!model.admits(&context));
        assert!(model.snapshot().switcher.is_some());
        model.search();
        assert!(model.snapshot().switcher.is_none());
        model.hide();
        assert!(!model.snapshot().visible);
        assert!(model.snapshot().switcher.is_none());
    }

    #[test]
    fn requests_before_document_ready_preserve_latest_intent_and_dismissal() {
        let mut model = Model::default();
        model.search();
        assert!(!model.ready);
        model.ready = true;
        assert!(model.snapshot().visible);

        let mut dismissed = Model::default();
        dismissed.search();
        dismissed.hide();
        dismissed.ready = true;
        assert!(!dismissed.snapshot().visible);
    }

    #[test]
    fn focus_leaving_for_the_browser_or_another_app_dismisses() {
        let mut m = Model::default();
        m.search();
        let observed = m.record_focus_change();
        m.focus(observed, false, true, true);
        assert!(!m.presented);
        m.search();
        let observed = m.record_focus_change();
        m.focus(observed, false, false, false);
        assert!(!m.snapshot().visible);
    }
    #[test]
    fn focus_recorded_while_hidden_cannot_dismiss_a_new_presentation() {
        let mut model = Model::default();
        let observed = model.record_focus_change();
        model.search();
        let opened = model.snapshot();
        model.focus(observed, false, true, true);
        assert_eq!(model.snapshot(), opened);
    }

    #[test]
    fn queued_focus_reconciliation_cannot_outlive_a_newer_focus_event() {
        let mut model = Model::default();
        model.search();
        let queued = model.record_focus_change();
        // The debounce settled, but a new native event arrived before its
        // queued UI reconciliation could run.
        let latest = model.record_focus_change();
        let opened = model.snapshot();
        model.focus(queued, false, false, false);
        assert_eq!(model.snapshot(), opened);
        model.focus(latest, false, false, false);
        assert!(!model.presented);
    }

    #[test]
    fn coalesced_focus_event_in_a_new_session_still_dismisses_that_session() {
        let mut model = Model::default();
        model.search();
        let previous = model.record_focus_change();
        model.hide();
        model.search();
        model.record_focus_change();
        // An existing debounce worker follows the latest event, including
        // the session recorded when that event arrived.
        let observed = model.latest_focus_change();
        assert_ne!(observed, previous);
        let opened = model.snapshot();
        model.focus(previous, false, false, false);
        assert_eq!(model.snapshot(), opened);
        model.focus(observed, false, true, true);
        assert!(!model.presented);
    }
    #[test]
    fn shortcut_and_context_revisions_do_not_replay() {
        let mut m = Model::default();
        m.set_owner(Some(owner("window", "p", "s")));
        m.search();
        let first = m.context("1").unwrap();
        m.hide();
        assert!(!m.presented);
        m.search();
        assert!(m.presented);
        assert!(!m.admits(&first));
    }

    #[test]
    fn owned_native_surface_does_not_dismiss_search() {
        let mut m = Model::default();
        m.search();
        // A menu/dialog owns focus, but Zephium is still the active application.
        let observed = m.record_focus_change();
        m.focus(observed, false, false, true);
        assert!(m.snapshot().visible);
        let observed = m.record_focus_change();
        m.focus(observed, false, false, false);
        assert!(!m.snapshot().visible);
    }

    #[test]
    fn owner_changes_invalidate_search() {
        let mut m = Model::default();
        m.set_owner(Some(owner("one", "p", "s")));
        m.search();
        let request = m.context("request").unwrap();
        m.set_owner(Some(owner("two", "p", "s")));
        assert!(!m.admits(&request));
        assert!(!m.snapshot().visible);
    }

    #[test]
    fn editing_seeds_the_address_and_keeps_the_panel_session() {
        let mut m = Model::default();
        m.set_owner(Some(owner("one", "p", "s")));
        m.edit("tab".into(), "https://example.com/".into());
        let snapshot = m.snapshot();
        assert_eq!(snapshot.seed.as_deref(), Some("https://example.com/"));
        // Search answers the panel's own session, or the panel drops every result.
        let request = m.context("request").unwrap();
        assert_eq!(request.session_id, snapshot.session_id);
        assert!(m.admits(&request));
        assert_eq!(m.editing_tab().as_deref(), Some("tab"));
        m.search();
        assert_eq!(m.snapshot().seed, None);
        assert_eq!(m.editing_tab(), None);
    }
}
