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
    Little,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Glance {
    pub tab: ItemId,
    pub parent: ItemId,
    pub backdrop: Option<u64>,
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
    pub glance: Option<Glance>,
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
                glance: None,
            },
        );
        self.order.push(id);
        self.focused = Some(id);
        id
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
