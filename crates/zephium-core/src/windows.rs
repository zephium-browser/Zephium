//! Windows are runtime entities projecting the one authoritative state.
//! Active tab and split tree are per-window.

use std::collections::HashMap;

use crate::geometry::Size;
use crate::ids::{ItemId, ProfileId, SpaceId, WindowId};
use crate::layout::{Metrics, Mode};
use crate::split::Pane;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WindowKind {
    Main,
    App { origin: String },
    Mini,
}

#[derive(Debug)]
pub struct Window {
    pub id: WindowId,
    pub kind: WindowKind,
    pub profile: ProfileId,
    pub space: SpaceId,
    pub mode: Mode,
    pub size: Size,
    pub metrics: Metrics,
    /// An admitted request to keep the hidden card raised, not native completion.
    pub sidebar_peek: bool,
    pub active: Option<ItemId>,
    pub splits: Option<Pane>,
}

#[derive(Default)]
pub struct Windows {
    order: Vec<WindowId>,
    map: HashMap<WindowId, Window>,
    focused: Option<WindowId>,
    next: WindowId,
}

impl Windows {
    pub fn create(
        &mut self,
        kind: WindowKind,
        profile: ProfileId,
        space: SpaceId,
        size: Size,
    ) -> WindowId {
        self.next += 1;
        let id = self.next;
        self.map.insert(
            id,
            Window {
                id,
                kind,
                profile,
                space,
                mode: Mode::Sidebar,
                size,
                metrics: Metrics::default(),
                sidebar_peek: false,
                active: None,
                splits: None,
            },
        );
        self.order.push(id);
        self.focused = Some(id);
        id
    }

    /// A window that leaves focus where it was, as a mini window does: the
    /// main window stays the one commands and layout follow.
    pub fn create_unfocused(
        &mut self,
        kind: WindowKind,
        profile: ProfileId,
        space: SpaceId,
        size: Size,
    ) -> WindowId {
        let focused = self.focused;
        let id = self.create(kind, profile, space, size);
        self.focused = focused;
        id
    }

    /// Removes a window that is not focused. The focused window stays.
    pub fn remove(&mut self, id: WindowId) -> Option<Window> {
        if self.focused == Some(id) {
            return None;
        }
        self.order.retain(|window| *window != id);
        self.map.remove(&id)
    }

    pub fn get(&self, id: WindowId) -> Option<&Window> {
        self.map.get(&id)
    }

    pub fn get_mut(&mut self, id: WindowId) -> Option<&mut Window> {
        self.map.get_mut(&id)
    }

    pub fn focused(&self) -> Option<&Window> {
        self.focused.and_then(|id| self.map.get(&id))
    }

    pub fn focused_mut(&mut self) -> Option<&mut Window> {
        self.focused.and_then(|id| self.map.get_mut(&id))
    }

    /// Iterates logical windows in stable creation order.
    pub fn iter(&self) -> impl Iterator<Item = &Window> {
        self.order.iter().filter_map(|id| self.map.get(id))
    }

    pub fn iter_mut(&mut self) -> impl Iterator<Item = &mut Window> {
        self.map.values_mut()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::{ProfileId, SpaceId};

    #[test]
    fn an_unfocused_window_leaves_focus_and_is_removed_alone() {
        let mut windows = Windows::default();
        let profile = ProfileId::generate();
        let space = SpaceId::generate();
        let main = windows.create(WindowKind::Main, profile, space, Size::new(800.0, 600.0));
        let mini =
            windows.create_unfocused(WindowKind::Mini, profile, space, Size::new(400.0, 300.0));
        assert_ne!(main, mini);
        assert_eq!(windows.focused().map(|window| window.id), Some(main));
        assert!(windows.remove(main).is_none());
        assert_eq!(
            windows.remove(mini).map(|window| window.kind),
            Some(WindowKind::Mini)
        );
        assert_eq!(windows.iter().count(), 1);
        assert_eq!(windows.focused().map(|window| window.id), Some(main));
    }
}
