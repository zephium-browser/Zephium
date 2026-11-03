//! Windows are runtime entities projecting the one authoritative state.
//! Active tab and split tree are per-window.

use std::collections::HashMap;

use crate::geometry::Size;
use crate::ids::{ItemId, ProfileId, SpaceId, WindowId};
use crate::layout::{Metrics, Mode, SidebarPosition};
use crate::split::Pane;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WindowKind {
    Main,
    App { origin: String },
    Little,
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
    pub active: Option<ItemId>,
    pub splits: Option<Pane>,
}

#[derive(Default)]
pub struct Windows {
    order: Vec<WindowId>,
    map: HashMap<WindowId, Window>,
    focused: Option<WindowId>,
    next: WindowId,
    sidebar_position: SidebarPosition,
    sidebar_caption: f64,
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
                metrics: Metrics {
                    sidebar_position: self.sidebar_position,
                    caption: self.sidebar_caption,
                    ..Metrics::default()
                },
                active: None,
                splits: None,
            },
        );
        self.order.push(id);
        self.focused = Some(id);
        id
    }

    /// Applies the app-wide preference to current and future windows.
    pub fn set_sidebar_position(&mut self, position: SidebarPosition, caption: f64) -> bool {
        if self.sidebar_position == position && self.sidebar_caption == caption {
            return false;
        }
        self.sidebar_position = position;
        self.sidebar_caption = caption;
        for window in self.map.values_mut() {
            window.metrics.sidebar_position = position;
            window.metrics.caption = caption;
        }
        true
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
}
