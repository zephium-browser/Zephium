use zephium_ipc::{PanelRoute, PanelState, SearchContext, ToolKind};
pub const RADIUS: u16 = 20;
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Owner {
    pub private: bool,
    pub window: String,
    pub profile: String,
    pub name: String,
    pub space: String,
}
#[derive(Debug)]
pub struct Model {
    revision: u64,
    publication: u64,
    pub route: PanelRoute,
    pub presented: bool,
    pub suppressed: bool,
    pub ready: bool,
    pub owner: Option<Owner>,
    pub error: bool,
    pub pending_action: Option<String>,
}
impl Default for Model {
    fn default() -> Self {
        Self {
            revision: 0,
            publication: 0,
            route: PanelRoute::Search,
            presented: false,
            suppressed: false,
            ready: false,
            owner: None,
            error: false,
            pending_action: None,
        }
    }
}
impl Model {
    pub fn session(&self) -> String {
        format!("{:016x}", self.revision)
    }
    pub fn snapshot(&self) -> PanelState {
        PanelState {
            window_id: self.owner.as_ref().map(|owner| owner.window.clone()),
            revision: format!("{:016x}", self.publication),
            session_id: self.session(),
            visible: self.presented && !self.suppressed,
            route: self.route.clone(),
            profile_id: self.owner.as_ref().map(|o| o.profile.clone()),
            profile_name: self.owner.as_ref().map(|o| o.name.clone()),
            space_id: self.owner.as_ref().map(|o| o.space.clone()),
            error: self.error,
            corner_radius: RADIUS,
            position_restorable: true,
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
        self.pending_action = None;
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
        self.route = PanelRoute::Search;
        self.presented = true;
        self.suppressed = false;
        self.error = false;
    }
    pub fn tool(&mut self, tool: ToolKind) {
        if !self.advance() {
            return;
        }
        self.route = PanelRoute::Tool { tool };
        self.presented = true;
        self.suppressed = false;
        self.error = false;
    }
    pub fn hide(&mut self) {
        if !self.advance() {
            return;
        }
        self.presented = false;
        self.suppressed = false;
        self.error = false;
    }
    pub fn toggle(&mut self) {
        if self.presented && !self.suppressed && matches!(self.route, PanelRoute::Search) {
            self.hide();
        } else {
            self.search();
        }
    }
    pub fn focus(&mut self, panel_focused: bool, main_focused: bool, app_active: bool) {
        if !self.presented {
            return;
        }
        if matches!(self.route, PanelRoute::Search) {
            if !panel_focused && (main_focused || !app_active) {
                self.hide();
            }
        } else {
            let suppressed = !app_active && !panel_focused;
            if suppressed != self.suppressed {
                if !self.advance() {
                    return;
                }
                self.suppressed = suppressed;
            }
        }
    }
    pub fn set_owner(&mut self, owner: Option<Owner>) {
        if self.owner == owner {
            return;
        }
        let keep_tool = self.presented
            && matches!(self.route, PanelRoute::Tool { .. })
            && self
                .owner
                .as_ref()
                .zip(owner.as_ref())
                .is_some_and(|(old, new)| old.profile == new.profile);
        if !self.advance() {
            return;
        }
        if !keep_tool {
            self.presented = false;
            self.suppressed = false;
        }
        self.owner = owner;
    }
    pub fn context(&self, request_id: &str) -> Option<SearchContext> {
        if !self.presented
            || self.suppressed
            || !matches!(self.route, PanelRoute::Search)
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
    #[test]
    fn requests_before_document_ready_preserve_latest_intent_and_dismissal() {
        let mut model = Model::default();
        model.search();
        model.tool(ToolKind::Notes);
        assert!(!model.ready);
        model.ready = true;
        assert!(model.snapshot().visible);
        assert!(matches!(
            model.snapshot().route,
            PanelRoute::Tool {
                tool: ToolKind::Notes
            }
        ));

        let mut dismissed = Model::default();
        dismissed.search();
        dismissed.hide();
        dismissed.ready = true;
        assert!(!dismissed.snapshot().visible);
    }

    #[test]
    fn search_and_tools_have_distinct_focus_policy() {
        let mut m = Model::default();
        m.search();
        m.focus(false, true, true);
        assert!(!m.presented);
        m.tool(ToolKind::Notes);
        m.focus(false, true, true);
        assert!(m.snapshot().visible);
        m.focus(false, false, false);
        assert!(!m.snapshot().visible);
        assert!(m.presented);
        m.focus(false, true, true);
        assert!(m.snapshot().visible);
        m.hide();
        m.focus(false, true, true);
        assert!(!m.snapshot().visible);
    }
    #[test]
    fn shortcut_and_context_revisions_do_not_replay() {
        let mut m = Model::default();
        m.set_owner(Some(Owner {
            private: false,
            window: "window".into(),
            profile: "p".into(),
            name: "P".into(),
            space: "s".into(),
        }));
        m.toggle();
        let first = m.context("1").unwrap();
        m.tool(ToolKind::Tasks);
        assert!(!m.admits(&first));
        m.toggle();
        assert!(matches!(m.route, PanelRoute::Search));
        assert!(!m.admits(&first));
        m.toggle();
        assert!(!m.presented);
    }

    #[test]
    fn owned_native_surface_does_not_dismiss_search() {
        let mut m = Model::default();
        m.search();
        // A menu/dialog owns focus, but Zephium is still the active application.
        m.focus(false, false, true);
        assert!(m.snapshot().visible);
        m.focus(false, false, false);
        assert!(!m.snapshot().visible);
    }

    #[test]
    fn owner_changes_invalidate_search_and_profile_changes_hide_tools() {
        let mut m = Model::default();
        let mut owner = Owner {
            private: false,
            window: "one".into(),
            profile: "p".into(),
            name: "Personal".into(),
            space: "s".into(),
        };
        m.set_owner(Some(owner.clone()));
        m.search();
        let request = m.context("request").unwrap();
        owner.window = "two".into();
        m.set_owner(Some(owner.clone()));
        assert!(!m.admits(&request));
        assert!(!m.snapshot().visible);
        m.tool(ToolKind::Notes);
        owner.space = "another".into();
        m.set_owner(Some(owner.clone()));
        assert!(m.snapshot().visible);
        owner.profile = "other-profile".into();
        m.set_owner(Some(owner));
        assert!(!m.snapshot().visible);
    }
}
