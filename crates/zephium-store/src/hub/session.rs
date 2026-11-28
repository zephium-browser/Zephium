 Space
/// identities. Scope preservation keeps unsent composer text reachable.
fn restarted_session(registry: &HashSet<ProfileId>, preserved: Option<&[u8]>) -> SessionState {
    let value = preserved.and_then(|bytes| serde_json::from_slice::<serde_json::Value>(bytes).ok());
    let named: Vec<(ProfileId, String, ProfileKind)> = value
        .as_ref()
        .and_then(|value| value.get("profiles")?.as_array().cloned())
        .unwrap_or_default()
        .iter()
        .filter_map(|profile| {
            let raw = profile.get("id")?.as_str()?;
            let id = ProfileId::parse(raw).filter(|id| id.to_string() == raw)?;
            let name = profile.get("name")?.as_str()?.trim();
            let kind = match profile.get("kind")?.as_str()? {
                "default" | "Default" => ProfileKind::Default,
                _ => ProfileKind::Named,
            };
            (registry.contains(&id) && !name.is_empty() && name.len() <= MAX_NAME_BYTES)
                .then(|| (id, name.to_owned(), kind))
        })
        .collect();
    let mut ids: Vec<ProfileId> = named.iter().map(|(id, ..)| *id).collect();
    let mut rest: Vec<ProfileId> = registry
        .iter()
        .filter(|id| !ids.contains(id))
        .copied()
        .collect();
    rest.sort_by_key(ToString::to_string);
    ids.dedup();
    ids.extend(rest);
    let mut defaulted = false;
    let profiles = ids
        .into_iter()
        .map(|id| {
            let (name, kind) = named
                .iter()
                .find(|(known, ..)| *known == id)
                .map(|(_, name, kind)| (name.clone(), *kind))
                .unwrap_or_else(|| ("Profile".to_owned(), ProfileKind::Named));
            let kind = if kind == ProfileKind::Default && !defaulted {
                defaulted = true;
                kind
            } else {
                ProfileKind::Named
            };
            PersistedProfile { id, name, kind }
        })
        .collect::<Vec<_>>();
    let mut profiles = profiles;
    if !defaulted {
        if let Some(first) = profiles.first_mut() {
            first.kind = ProfileKind::Default;
            if first.name == "Profile" {
                first.name = "Personal".into();
            }
        }
    }
    let spaces = value
        .as_ref()
        .and_then(|value| value.get("spaces")?.as_array())
        .into_iter()
        .flatten()
        .take(MAX_SESSION_SPACES)
        .filter_map(|value| {
            let space: BoundedSpace = serde_json::from_value(value.clone()).ok()?;
            (registry.contains(&space.profile)
                && space.name.len() <= MAX_NAME_BYTES
                && space_look_bounded(&space.look))
            .then_some(PersistedSpace {
                id: space.id,
                profile: space.profile,
                name: space.name,
                look: space.look,
            })
        })
        .collect();
    let active_space = value
        .as_ref()
        .and_then(|value| value.get("active_space")?.as_str())
        .and_then(SpaceId::parse);
    SessionState {
        profiles,
        spaces,
        active_space,
        ..Default::default()
    }
}

fn kind_to_str(kind: ProfileKind) -> Option<&'static str> {
    match kind {
        ProfileKind::Default => Some("default"),
        ProfileKind::Named => Some("named"),
        ProfileKind::Incognito => None,
    }
}

fn kind_from_str(s: &str) -> Option<ProfileKind> {
    match s {
        "default" => Some(ProfileKind::Default),
        "named" => Some(ProfileKind::Named),
        _ => None,
    }
}

#[cfg(test)]
mod bounded_snapshot_tests {
    use super::*;

    fn snapshot_with_profiles(profiles: &str) -> String {
        format!(
            r#"{{"profiles":[{profiles}],"spaces":[],"items":[],"active_space":null,"active_item":null,"splits":null}}"#
        )
    }

    #[test]
    fn authoritative_collection_limit_is_enforced_by_the_sequence_visitor() {
        let profile = format!(
            r#"{{"id":"{}","name":"P","kind":"Default"}}"#,
            ProfileId::from(1)
        );
        let profiles = vec![profile; MAX_SESSION_PROFILES + 1].join(",");
        assert!(decode_authoritative_snapshot(&snapshot_with_profiles(&profiles)).is_none());
    }

    #[test]
    fn authoritative_nested_unknown_fields_are_not_silently_projected_away() {
        let profile = format!(
            r#"{{"id":"{}","name":"P","kind":"Default","future":true}}"#,
            ProfileId::from(1)
        );
        assert!(decode_authoritative_snapshot(&snapshot_with_profiles(&profile)).is_none());
    }

    #[test]
    fn near_snapshot_cap_oversized_string_is_rejected_by_lexical_preflight() {
        let name = "x".repeat(MAX_SESSION_SNAPSHOT_BYTES - 1024);
        let profile = format!(
            r#"{{"id":"{}","name":"{name}","kind":"Default"}}"#,
            ProfileId::from(1)
        );
        let snapshot = snapshot_with_profiles(&profile);
        assert!(snapshot.len() < MAX_SESSION_SNAPSHOT_BYTES);
        assert!(snapshot.len() > MAX_SESSION_SNAPSHOT_BYTES - 2048);
        assert!(decode_authoritative_snapshot(&snapshot).is_none());
    }
}

#[cfg(test)]
mod essentials_per_space_tests {
    use super::*;

    const PERSONAL: u128 = 1;
    const WORK_PROFILE: u128 = 2;
    const HOME: u128 = 11;
    const SECOND: u128 = 12;
    const WORK: u128 = 13;

    fn tab(id: u128, placement: Placement, url: &str) -> PersistedItem {
        PersistedItem {
            id: ItemId::from(id),
            parent: None,
            placement,
            kind: PersistedKind::Tab {
                url: url.into(),
                title: url.into(),
                zoom: 1.0,
                last_active_ms: None,
                custom_icon: None,
            },
        }
    }

    fn essentials(space: u128) -> Placement {
        Placement::Favorites {
            space: SpaceId::from(space),
        }
    }

    /// Two spaces in one profile, one in another, with the kept sites of the
    /// first profile in its second space and the other's in its only one.
    fn session(active: u128) -> SessionState {
        let space = |id, profile| PersistedSpace {
            id: SpaceId::from(id),
            profile: ProfileId::from(profile),
            name: "Space".into(),
            look: Default::default(),
        };
        let profile = |id, name: &str, kind| PersistedProfile {
            id: ProfileId::from(id),
            name: name.into(),
            kind,
        };
        SessionState {
            profiles: vec![
                profile(PERSONAL, "Personal", ProfileKind::Default),
                profile(WORK_PROFILE, "Work", ProfileKind::Named),
            ],
            spaces: vec![
                space(HOME, PERSONAL),
                space(SECOND, PERSONAL),
                space(WORK, WORK_PROFILE),
            ],
            items: vec![
                tab(
                    21,
                    Placement::Space {
                        space: SpaceId::from(HOME),
                        section: SpaceSection::Today,
                    },
                    "https://open.example/",
                ),
                tab(22, essentials(SECOND), "https://mail.example/"),
                tab(23, essentials(SECOND), "https://chat.example/"),
                tab(24, essentials(WORK), "https://tickets.example/"),
            ],
            active_space: Some(SpaceId::from(active)),
            active_item: Some(ItemId::from(22)),
            splits: None,
            recently_closed: Vec::new(),
        }
    }

    /// The same session as version 1 wrote it: each profile's kept sites are
    /// one set that names the profile.
    fn as_saved_by_version_one(state: &SessionState) -> String {
        let mut value = serde_json::to_value(state).unwrap();
        for item in value["items"].as_array_mut().unwrap() {
            let Some(favorites) = item["placement"].get_mut("Favorites") else {
                continue;
            };
            let space: SpaceId = serde_json::from_value(favorites["space"].clone()).unwrap();
            let owner = state.spaces.iter().find(|s| s.id == space).unwrap().profile;
            *favorites = serde_json::json!({ "profile": owner });
        }
        value.to_string()
    }

    fn rewind(hub: &mut Hub, state: &SessionState) {
        hub.meta
            .execute(
                "UPDATE session_snapshot SET schema_version = 1, data = ?1",
                [as_saved_by_version_one(state)],
            )
            .unwrap();
    }

    fn urls(state: &SessionState, placement: Placement) -> Vec<String> {
        state
            .items
            .iter()
            .filter(|item| item.placement == placement)
            .filter_map(|item| match &item.kind {
                PersistedKind::Tab { url, .. } => Some(url.clone()),
                _ => None,
            })
            .collect()
    }

    fn ids(state: &SessionState, placement: Placement) -> Vec<ItemId> {
        state
            .items
            .iter()
            .filter(|item| item.placement == placement)
            .map(|item| item.id)
            .collect()
    }

    #[test]
    fn upgrading_keeps_the_original_bytes_and_schema_once() {
        let mut hub = Hub::in_memory().unwrap();
        let state = session(SECOND);
        hub.save(&state).unwrap();
        rewind(&mut hub, &state);
        let source = as_saved_by_version_one(&state).into_bytes();
        let restored = hub.load().unwrap().unwrap();
        hub.save(&restored).unwrap();
        let backup: (i64, Vec<u8>) = hub
            .meta
            .query_row(
                "SELECT schema_version, data FROM session_snapshot_backup WHERE id = 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(backup, (1, source));
        assert_eq!(hub.load().unwrap().unwrap(), restored);
        hub.save(&restored).unwrap();
        let count: i64 = hub
            .meta
            .query_row("SELECT count(*) FROM session_snapshot_backup", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn clearing_before_upgrade_cannot_recreate_the_old_source_backup() {
        let mut hub = Hub::in_memory().unwrap();
        let state = session(SECOND);
        hub.save(&state).unwrap();
        rewind(&mut hub, &state);
        let restored = hub.load().unwrap().unwrap();
        assert!(hub.pending_session_backup.is_some());
        hub.discard_session_backup(ProfileId::from(PERSONAL))
            .unwrap();
        assert!(hub.pending_session_backup.is_none());
        assert_eq!(hub.load().unwrap().unwrap(), restored);
        assert!(hub.pending_session_backup.is_none());
        hub.save(&restored).unwrap();
        let count: i64 = hub
            .meta
            .query_row("SELECT count(*) FROM session_snapshot_backup", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn each_space_keeps_the_essentials_it_was_given() {
        let mut hub = Hub::in_memory().unwrap();
        let state = session(SECOND);
        hub.save(&state).unwrap();

        let loaded = hub.load().unwrap().unwrap();

        assert_eq!(loaded, state);
        assert!(urls(&loaded, essentials(HOME)).is_empty());
        assert_eq!(urls(&loaded, essentials(SECOND)).len(), 2);
    }

    #[test]
    fn a_session_saved_with_one_set_per_profile_gets_a_copy_in_every_space() {
        let mut hub = Hub::in_memory().unwrap();
        let state = session(SECOND);
        hub.save(&state).unwrap();
        rewind(&mut hub, &state);

        let loaded = hub.load().unwrap().unwrap();

        for space in [HOME, SECOND] {
            assert_eq!(
                urls(&loaded, essentials(space)),
                ["https://mail.example/", "https://chat.example/"],
                "space {space} lost or reordered a kept site"
            );
        }
        assert_eq!(
            urls(&loaded, essentials(WORK)),
            ["https://tickets.example/"],
            "a profile's kept sites reach only its own spaces"
        );
        // The space the person was in keeps the originals, so what was in
        // front stays in front; the copies are new items.
        assert_eq!(
            ids(&loaded, essentials(SECOND)),
            [ItemId::from(22), ItemId::from(23)]
        );
        assert_eq!(loaded.active_item, Some(ItemId::from(22)));
        let mut every: Vec<ItemId> = loaded.items.iter().map(|item| item.id).collect();
        let count = every.len();
        every.sort();
        every.dedup();
        assert_eq!(every.len(), count, "an item is owned by two spaces");
        assert_eq!(count, 6);
    }

    #[test]
    fn the_originals_go_to_the_profiles_first_space_when_the_person_was_in_another_profile() {
        let mut hub = Hub::in_memory().unwrap();
        let mut state = session(WORK);
        state.active_item = Some(ItemId::from(24));
        hub.save(&state).unwrap();
        rewind(&mut hub, &state);

        let loaded = hub.load().unwrap().unwrap();

        assert_eq!(
            ids(&loaded, essentials(HOME)),
            [ItemId::from(22), ItemId::from(23)]
        );
        assert_eq!(loaded.active_item, Some(ItemId::from(24)));
    }

    #[test]
    fn an_upgraded_session_is_saved_in_the_new_schema_and_not_copied_again() {
        let mut hub = Hub::in_memory().unwrap();
        let state = session(SECOND);
        hub.save(&state).unwrap();
        rewind(&mut hub, &state);

        let upgraded = hub.load().unwrap().unwrap();
        hub.save(&upgraded).unwrap();

        let version: i64 = hub
            .meta
            .query_row("SELECT schema_version FROM session_snapshot", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(version, SESSION_SCHEMA_VERSION);
        assert_eq!(hub.load().unwrap().unwrap(), upgraded);
    }

    #[test]
    fn a_schema_is_read_only_in_the_form_it_wrote() {
        let state = session(SECOND);
        let version_one = as_saved_by_version_one(&state);
        let version_two = serde_json::to_string(&state).unwrap();

        assert!(decode_snapshot(&version_one, 1).is_some());
        assert!(decode_snapshot(&version_two, 2).is_some());
        assert!(decode_snapshot(&version_one, 2).is_none());
        assert!(decode_snapshot(&version_two, 1).is_none());
    }

    #[test]
    fn essentials_naming_both_a_profile_and_a_space_are_not_a_session() {
        let state = session(SECOND);
        let mut value = serde_json::to_value(&state).unwrap();
        value["items"][1]["placement"]["Favorites"]["profile"] =
            serde_json::json!(ProfileId::from(PERSONAL));
        assert!(decode_snapshot(&value.to_string(), 2).is_none());
        assert!(decode_snapshot(&value.to_string(), 1).is_none());
    }
}
