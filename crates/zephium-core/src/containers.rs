//! Website-data containers are profile-owned identities that can be made
//! available to selected Spaces without merging their browsing sessions.

use std::collections::HashMap;

use crate::ids::{ContainerId, ProfileId, SpaceId};

pub const MAX_PROFILE_CONTAINERS: usize = 24;
pub const MAX_CONTAINER_NAME_CHARS: usize = 48;
pub const SETTINGS_KEY: &str = "browse.containers.v1";

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContainerKind {
    Browsing,
    Extensions,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", content = "spaces", rename_all = "snake_case")]
pub enum SpaceAccess {
    All,
    Selected(Vec<SpaceId>),
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Container {
    pub id: ContainerId,
    pub profile: ProfileId,
    pub name: String,
    pub kind: ContainerKind,
    pub look: crate::spaces::SpaceLook,
    pub spaces: SpaceAccess,
}

pub fn container_name(name: &str) -> Option<String> {
    let name: String = crate::session::bounded_name(name.trim(), "")
        .trim()
        .chars()
        .take(MAX_CONTAINER_NAME_CHARS)
        .collect();
    let name = name.trim();
    (!name.is_empty()).then(|| name.to_owned())
}

impl Container {
    pub fn available_in(&self, space: SpaceId) -> bool {
        match &self.spaces {
            SpaceAccess::All => true,
            SpaceAccess::Selected(spaces) => spaces.contains(&space),
        }
    }
}

#[derive(Clone, Default)]
pub struct Containers {
    order: Vec<ContainerId>,
    map: HashMap<ContainerId, Container>,
}

impl Containers {
    pub fn from_setting(value: Option<&str>) -> Self {
        let Some(value) = value else {
            return Self::default();
        };
        let Ok(containers) = serde_json::from_str::<Vec<Container>>(value) else {
            return Self::default();
        };
        let mut restored = Self::default();
        let mut per_profile = HashMap::<ProfileId, usize>::new();
        for mut container in containers {
            let count = per_profile.entry(container.profile).or_default();
            if *count >= MAX_PROFILE_CONTAINERS
                || container_name(&container.name).as_deref() != Some(container.name.as_str())
                || !container.look.valid()
                || matches!(&container.spaces, SpaceAccess::Selected(spaces) if spaces.len() > crate::session::MAX_SESSION_SPACES)
            {
                continue;
            }
            if let SpaceAccess::Selected(spaces) = &mut container.spaces {
                let mut seen = std::collections::HashSet::new();
                spaces.retain(|space| seen.insert(*space));
            }
            if restored.insert(container) {
                *count += 1;
            }
        }
        restored
    }

    pub fn setting(&self) -> Option<String> {
        serde_json::to_string(&self.iter().collect::<Vec<_>>()).ok()
    }

    pub fn insert(&mut self, container: Container) -> bool {
        let count = self
            .map
            .values()
            .filter(|candidate| candidate.profile == container.profile)
            .count();
        if count >= MAX_PROFILE_CONTAINERS || self.map.contains_key(&container.id) {
            return false;
        }
        self.order.push(container.id);
        self.map.insert(container.id, container);
        true
    }

    pub fn get(&self, id: ContainerId) -> Option<&Container> {
        self.map.get(&id)
    }

    pub fn replace(&mut self, container: Container) -> bool {
        let Some(previous) = self.map.get(&container.id) else {
            return false;
        };
        if previous.profile != container.profile
            || container_name(&container.name).as_deref() != Some(container.name.as_str())
            || !container.look.valid()
            || matches!(&container.spaces, SpaceAccess::Selected(spaces) if spaces.len() > crate::session::MAX_SESSION_SPACES)
        {
            return false;
        }
        self.map.insert(container.id, container);
        true
    }

    pub fn contains(&self, id: ContainerId) -> bool {
        self.map.contains_key(&id)
    }

    pub fn remove(&mut self, id: ContainerId, profile: ProfileId) -> Option<Container> {
        if self
            .map
            .get(&id)
            .is_none_or(|container| container.profile != profile)
        {
            return None;
        }
        self.order.retain(|candidate| *candidate != id);
        self.map.remove(&id)
    }

    pub fn iter(&self) -> impl Iterator<Item = &Container> {
        self.order.iter().filter_map(|id| self.map.get(id))
    }

    pub fn for_profile(&self, profile: ProfileId) -> impl Iterator<Item = &Container> {
        self.iter()
            .filter(move |container| container.profile == profile)
    }
}
