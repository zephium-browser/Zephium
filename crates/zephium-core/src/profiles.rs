//! Profiles are isolation roots: each owns a store partition and a webview
//! data partition. Incognito is ephemeral and never persisted.

use std::collections::HashMap;

use crate::ids::ProfileId;

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ProfileKind {
    Default,
    Named,
    Incognito,
}

#[derive(Clone, Debug)]
pub struct Profile {
    pub id: ProfileId,
    pub name: String,
    pub kind: ProfileKind,
}

/// Colours and symbols are indices into the frame's curated sets, so a
/// stored avatar never names a colour or glyph the interface cannot draw.
pub const PROFILE_AVATAR_COLORS: u8 = 10;
pub const PROFILE_AVATAR_SYMBOLS: u8 = 16;
/// A photo is stored downscaled; nothing larger is ever written or served.
pub const MAX_PROFILE_AVATAR_PHOTO_BYTES: usize = 64 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProfileAvatarStyle {
    Initials,
    Symbol,
    Photo,
}

/// How a profile is drawn. `revision` advances on every change so a photo's
/// URL changes with it; revision 0 is the unsaved default.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProfileAvatar {
    pub style: ProfileAvatarStyle,
    pub color: u8,
    pub symbol: u8,
    pub revision: u32,
}

impl ProfileAvatar {
    /// Initials on a colour picked from the id, stable across launches.
    pub fn initial(profile: ProfileId) -> Self {
        let seed = profile.bytes().iter().fold(0_u32, |sum, byte| {
            sum.wrapping_mul(31).wrapping_add(u32::from(*byte))
        });
        Self {
            style: ProfileAvatarStyle::Initials,
            color: (seed % u32::from(PROFILE_AVATAR_COLORS)) as u8,
            symbol: 0,
            revision: 0,
        }
    }

    pub fn valid(&self) -> bool {
        self.color < PROFILE_AVATAR_COLORS && self.symbol < PROFILE_AVATAR_SYMBOLS
    }
}

/// A profile name as it is kept: bounded, stripped of controls, never empty.
pub fn profile_name(name: &str) -> Option<String> {
    let name = crate::session::bounded_name(name.trim(), "");
    let name = name.trim();
    (!name.is_empty()).then(|| name.to_owned())
}

#[derive(Default)]
pub struct Profiles {
    order: Vec<ProfileId>,
    map: HashMap<ProfileId, Profile>,
    bookmark_folders: HashMap<ProfileId, i64>,
}

impl Profiles {
    pub fn insert(&mut self, profile: Profile) -> bool {
        if self.map.len() >= crate::session::MAX_SESSION_PROFILES
            || self.map.contains_key(&profile.id)
        {
            return false;
        }
        self.order.push(profile.id);
        self.map.insert(profile.id, profile);
        true
    }

    pub fn bookmark_folder(&self, id: ProfileId) -> Option<i64> {
        self.bookmark_folders.get(&id).copied()
    }

    pub fn set_bookmark_folder(&mut self, id: ProfileId, folder: Option<i64>) -> bool {
        if !self.map.contains_key(&id) || folder.is_some_and(|folder| folder <= 0) {
            return false;
        }
        if let Some(folder) = folder {
            self.bookmark_folders.insert(id, folder);
        } else {
            self.bookmark_folders.remove(&id);
        }
        true
    }

    pub fn get(&self, id: ProfileId) -> Option<&Profile> {
        self.map.get(&id)
    }

    /// Removes exactly one isolation root. Selection/default-profile policy is
    /// deliberately left to the application layer.
    pub fn remove(&mut self, id: ProfileId) -> Option<Profile> {
        let profile = self.map.remove(&id)?;
        self.bookmark_folders.remove(&id);
        self.order.retain(|candidate| *candidate != id);
        Some(profile)
    }

    /// Names a profile. The name is bounded and stripped of controls exactly
    /// as a restored one is, and a name with nothing left is refused rather
    /// than replaced, so a cleared field never renames a profile to a default.
    pub fn rename(&mut self, id: ProfileId, name: &str) -> bool {
        let Some(name) = profile_name(name) else {
            return false;
        };
        match self.map.get_mut(&id) {
            Some(profile) if profile.kind != ProfileKind::Incognito => {
                profile.name = name;
                true
            }
            _ => false,
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = &Profile> {
        self.order.iter().filter_map(|id| self.map.get(id))
    }

    pub fn is_empty(&self) -> bool {
        self.order.is_empty()
    }

    pub fn default_profile(&self) -> Option<ProfileId> {
        self.iter()
            .find(|p| p.kind == ProfileKind::Default)
            .map(|p| p.id)
            .or(self.order.first().copied())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bookmark_folder_selection_requires_a_profile_and_positive_id_and_clears_on_remove() {
        let mut profiles = Profiles::default();
        let first = ProfileId::from(1);
        assert!(!profiles.set_bookmark_folder(first, Some(1)));
        assert!(profiles.insert(Profile {
            id: first,
            name: "Personal".into(),
            kind: ProfileKind::Default,
        }));
        assert!(!profiles.set_bookmark_folder(first, Some(0)));
        assert!(!profiles.set_bookmark_folder(first, Some(-1)));
        assert!(profiles.set_bookmark_folder(first, Some(42)));
        assert_eq!(profiles.bookmark_folder(first), Some(42));
        assert!(profiles.remove(first).is_some());
        assert_eq!(profiles.bookmark_folder(first), None);
    }

    #[test]
    fn profile_count_is_bounded_at_the_aggregate_boundary() {
        let mut profiles = Profiles::default();
        for value in 0..crate::session::MAX_SESSION_PROFILES {
            assert!(profiles.insert(Profile {
                id: ProfileId::from(value as u128 + 1),
                name: "Profile".into(),
                kind: ProfileKind::Named,
            }));
        }
        assert!(!profiles.insert(Profile {
            id: ProfileId::from(10_000),
            name: "Overflow".into(),
            kind: ProfileKind::Named,
        }));
    }

    #[test]
    fn a_rename_is_bounded_and_never_empties_a_name() {
        let mut profiles = Profiles::default();
        for (value, kind) in [(1, ProfileKind::Default), (2, ProfileKind::Incognito)] {
            assert!(profiles.insert(Profile {
                id: ProfileId::from(value),
                name: "Personal".into(),
                kind,
            }));
        }
        let personal = ProfileId::from(1);
        assert!(profiles.rename(personal, "  Alex\u{202e} Rivera \n"));
        assert_eq!(profiles.get(personal).unwrap().name, "Alex Rivera");
        assert!(!profiles.rename(personal, " \u{200f} "));
        assert_eq!(profiles.get(personal).unwrap().name, "Alex Rivera");
        assert!(!profiles.rename(ProfileId::from(2), "Private"));
        assert!(!profiles.rename(ProfileId::from(3), "Nobody"));
    }

    #[test]
    fn a_default_avatar_is_stable_initials_within_the_curated_set() {
        let first = ProfileAvatar::initial(ProfileId::from(42));
        assert_eq!(first, ProfileAvatar::initial(ProfileId::from(42)));
        assert_eq!(first.style, ProfileAvatarStyle::Initials);
        assert_eq!(first.revision, 0);
        assert!(first.valid());
        assert!(!ProfileAvatar {
            color: PROFILE_AVATAR_COLORS,
            ..first
        }
        .valid());
        assert!(!ProfileAvatar {
            symbol: PROFILE_AVATAR_SYMBOLS,
            ..first
        }
        .valid());
        assert_eq!(profile_name("  Work \n").as_deref(), Some("Work"));
        assert_eq!(profile_name(" \u{200f} "), None);
    }

    #[test]
    fn removal_is_exact_order_preserving_and_idempotent() {
        let mut profiles = Profiles::default();
        for value in 1..=3 {
            assert!(profiles.insert(Profile {
                id: ProfileId::from(value),
                name: format!("Profile {value}"),
                kind: ProfileKind::Named,
            }));
        }

        let removed = profiles.remove(ProfileId::from(2)).unwrap();
        assert_eq!(removed.id, ProfileId::from(2));
        assert_eq!(
            profiles
                .iter()
                .map(|profile| profile.id)
                .collect::<Vec<_>>(),
            vec![ProfileId::from(1), ProfileId::from(3)]
        );
        assert!(profiles.remove(ProfileId::from(2)).is_none());
    }
}
