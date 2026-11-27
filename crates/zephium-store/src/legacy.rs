//! One-time import of the pre-split single-file store (`default.sqlite` with a
//! session JSON blob). Understands both blob generations: v1 flat tabs and v2
//! item-tree. Read-only; the hub writes the result into the new layout.

use std::collections::HashMap;
use std::path::Path;

use rusqlite::{Connection, OpenFlags, OptionalExtension};
use serde::Deserialize;

use zephium_core::ids::{ItemId, ProfileId, SpaceId};
use zephium_core::item::{sanitize_page_title, Placement, SpaceSection};
use zephium_core::navigation;
use zephium_core::profiles::ProfileKind;
use zephium_core::session::{
    self, PersistedItem, PersistedKind, PersistedProfile, PersistedSpace, SessionState,
    MAX_SESSION_ITEMS, MAX_SESSION_PROFILES, MAX_SESSION_SPACES,
};

use crate::bounded_json;
use crate::essentials;
use crate::hub::{MAX_NAME_BYTES, MAX_TITLE_BYTES, MAX_URL_BYTES};
use crate::pane::{self, StoredPane};

const MAX_LEGACY_SESSION_BYTES: i64 = 16 * 1024 * 1024;

pub struct LegacyData {
    pub session: SessionState,
    pub visits: Vec<(String, String, i64)>,
}

pub fn read(path: &Path) -> Option<LegacyData> {
    let parent = std::fs::canonicalize(path.parent()?).ok()?;
    let path = parent.join(path.file_name()?);
    if !std::fs::symlink_metadata(&path).ok()?.file_type().is_file() {
        return None;
    }
    let conn = Connection::open_with_flags(
        &path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )
    .ok()?;
    let json: Option<String> = conn
        .query_row(
            "SELECT CASE WHEN length(CAST(data AS BLOB)) <= ?1 THEN data END
             FROM session WHERE id = 1",
            [MAX_LEGACY_SESSION_BYTES],
            |r| r.get(0),
        )
        .optional()
        .ok()?;
    let session = json.and_then(|j| decode(&j))?;
    let visits = read_visits(&conn);
    Some(LegacyData { session, visits })
}

fn read_visits(conn: &Connection) -> Vec<(String, String, i64)> {
    read_visits_with_budget(conn, crate::hub::MAX_HISTORY_BYTES as usize)
}

fn read_visits_with_budget(conn: &Connection, maximum_bytes: usize) -> Vec<(String, String, i64)> {
    let Ok(mut stmt) = conn.prepare(
        "SELECT url, title, visited_at FROM history
         WHERE length(CAST(url AS BLOB)) <= 8192
           AND length(CAST(title AS BLOB)) <= 4096
         ORDER BY visited_at DESC, id DESC
         LIMIT 50000",
    ) else {
        return Vec::new();
    };
    let Ok(rows) = stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, i64>(2)?,
        ))
    }) else {
        return Vec::new();
    };
    let mut retained_bytes = 0_usize;
    let mut visits = Vec::new();
    for row in rows {
        let Ok((url, title, at)) = row else {
            continue;
        };
        if !navigation::is_allowed_str(&url) {
            continue;
        }
        let title = sanitize_page_title(&title);
        let row_bytes = url.len().saturating_add(title.len());
        if retained_bytes.saturating_add(row_bytes) > maximum_bytes {
            // Rows are newest-first. Keep a deterministic bounded recent
            // prefix instead of retaining up to ~600 MiB of legacy strings
            // before the normal history quota gets a chance to run.
            break;
        }
        retained_bytes += row_bytes;
        visits.push((url, title, at));
    }
    visits.reverse();
    visits
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredSession {
    v: u32,
    #[serde(deserialize_with = "deserialize_stored_profiles")]
    profiles: Vec<StoredProfile>,
    #[serde(deserialize_with = "deserialize_stored_spaces")]
    spaces: Vec<StoredSpace>,
    #[serde(deserialize_with = "deserialize_stored_items")]
    items: Vec<StoredItem>,
    active_space: Option<String>,
    active_item: Option<String>,
    splits: Option<StoredPane>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredProfile {
    id: String,
    name: String,
    kind: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredSpace {
    id: String,
    profile: String,
    name: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredItem {
    id: String,
    parent: Option<String>,
    profile: Option<String>,
    space: Option<String>,
    section: Option<String>,
    folder: Option<String>,
    url: Option<String>,
    title: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct V1Session {
    #[serde(deserialize_with = "deserialize_v1_tabs")]
    tabs: Vec<V1Tab>,
    active: usize,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct V1Tab {
    url: String,
    title: String,
}

fn decode(json: &str) -> Option<SessionState> {
    let json = bounded_json::preflight(json).ok()?;
    let state = if let Ok(stored) = json.deserialize::<StoredSession>() {
        decode_v2(stored)?
    } else {
        decode_v1(json.deserialize::<V1Session>().ok()?)?
    };
    // Import is the last point at which the exact source still exists. Never
    // delete it after silently normalizing invalid URLs, dangling references,
    // duplicate IDs, malformed focus, or hostile display data.
    (session::canonicalize(state.clone()) == state).then_some(state)
}

fn deserialize_stored_profiles<'de, D>(deserializer: D) -> Result<Vec<StoredProfile>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    bounded_json::deserialize_bounded_vec(deserializer, MAX_SESSION_PROFILES, |profile| {
        profile.id.len() <= 26
            && profile.name.len() <= MAX_NAME_BYTES
            && profile.kind.len() <= "default".len()
    })
}

fn deserialize_stored_spaces<'de, D>(deserializer: D) -> Result<Vec<StoredSpace>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    bounded_json::deserialize_bounded_vec(deserializer, MAX_SESSION_SPACES, |space| {
        space.id.len() <= 26 && space.profile.len() <= 26 && space.name.len() <= MAX_NAME_BYTES
    })
}

fn deserialize_stored_items<'de, D>(deserializer: D) -> Result<Vec<StoredItem>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    bounded_json::deserialize_bounded_vec(deserializer, MAX_SESSION_ITEMS, |item| {
        item.id.len() <= 26
            && item.parent.as_ref().is_none_or(|value| value.len() <= 26)
            && item.profile.as_ref().is_none_or(|value| value.len() <= 26)
            && item.space.as_ref().is_none_or(|value| value.len() <= 26)
            && item
                .section
                .as_ref()
                .is_none_or(|value| value.len() <= "pinned".len())
            && item
                .folder
                .as_ref()
                .is_none_or(|value| value.len() <= MAX_NAME_BYTES)
            && item
                .url
                .as_ref()
                .is_none_or(|value| value.len() <= MAX_URL_BYTES)
            && item
                .title
                .as_ref()
                .is_none_or(|value| value.len() <= MAX_TITLE_BYTES)
    })
}

fn deserialize_v1_tabs<'de, D>(deserializer: D) -> Result<Vec<V1Tab>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    bounded_json::deserialize_bounded_vec(deserializer, MAX_SESSION_ITEMS, |tab| {
        tab.url.len() <= MAX_URL_BYTES && tab.title.len() <= MAX_TITLE_BYTES
    })
}

fn decode_v2(stored: StoredSession) -> Option<SessionState> {
    if stored.v != 2
        || stored.profiles.len() > MAX_SESSION_PROFILES
        || stored.spaces.len() > MAX_SESSION_SPACES
        || stored.items.len() > MAX_SESSION_ITEMS
        || stored
            .active_space
            .as_ref()
            .is_some_and(|value| value.len() > 26)
        || stored
            .active_item
            .as_ref()
            .is_some_and(|value| value.len() > 26)
    {
        return None;
    }
    let profiles = stored
        .profiles
        .into_iter()
        .map(|p| {
            let id = ProfileId::parse(&p.id).filter(|id| id.to_string() == p.id)?;
            Some(PersistedProfile {
                id,
                name: p.name,
                kind: match p.kind.as_str() {
                    "default" => ProfileKind::Default,
                    "named" => ProfileKind::Named,
                    _ => return None,
                },
            })
        })
        .collect::<Option<Vec<_>>>()?;
    let spaces = stored
        .spaces
        .into_iter()
        .map(|s| {
            let id = SpaceId::parse(&s.id).filter(|id| id.to_string() == s.id)?;
            let profile = ProfileId::parse(&s.profile).filter(|id| id.to_string() == s.profile)?;
            Some(PersistedSpace {
                id,
                profile,
                name: s.name,
                look: Default::default(),
            })
        })
        .collect::<Option<Vec<_>>>()?;
    let active_space = match stored.active_space {
        Some(raw) => Some(SpaceId::parse(&raw).filter(|id| id.to_string() == raw)?),
        None => None,
    };
    let mut homes = HashMap::new();
    let items = stored
        .items
        .into_iter()
        .map(|item| decode_item(item, &spaces, active_space, &mut homes))
        .collect::<Option<Vec<_>>>()?;
    // Essentials were once a profile's; every space of it gets a copy.
    let items = if homes.is_empty() {
        items
    } else {
        essentials::spread(items, &spaces, &homes)
    };
    let active_item = match stored.active_item {
        Some(raw) => Some(ItemId::parse(&raw).filter(|id| id.to_string() == raw)?),
        None => None,
    };
    let splits = match stored.splits {
        Some(stored) => Some(pane::decode(&stored)?),
        None => None,
    };
    Some(SessionState {
        profiles,
        spaces,
        items,
        active_space,
        active_item,
        splits,
        recently_closed: Vec::new(),
    })
}

fn decode_item(
    item: StoredItem,
    spaces: &[PersistedSpace],
    active_space: Option<SpaceId>,
    homes: &mut HashMap<ProfileId, SpaceId>,
) -> Option<PersistedItem> {
    let placement = match (&item.profile, &item.space, &item.section) {
        (Some(profile), None, None) => {
            let profile = ProfileId::parse(profile).filter(|id| id.to_string() == *profile)?;
            let home = essentials::home_space(spaces, profile, active_space)?;
            homes.insert(profile, home);
            Placement::Favorites { space: home }
        }
        (None, Some(space), Some(section)) => Placement::Space {
            space: SpaceId::parse(space).filter(|id| id.to_string() == *space)?,
            section: match section.as_str() {
                "pinned" => SpaceSection::Pinned,
                "today" => SpaceSection::Today,
                _ => return None,
            },
        },
        _ => return None,
    };
    let kind = match (item.folder, item.url) {
        (Some(name), None) => PersistedKind::Folder { name },
        (None, Some(url)) => PersistedKind::Tab {
            url,
            title: item.title.unwrap_or_default(),
            zoom: 1.0,
            last_active_ms: None,
        },
        _ => return None,
    };
    let parent = match item.parent {
        Some(p) => Some(ItemId::parse(&p).filter(|id| id.to_string() == p)?),
        None => None,
    };
    let id = ItemId::parse(&item.id).filter(|id| id.to_string() == item.id)?;
    Some(PersistedItem {
        id,
        parent,
        placement,
        kind,
    })
}

/// v1 blobs predate profiles: mint a default profile/space and turn the flat
/// tabs into Today items.
fn decode_v1(v1: V1Session) -> Option<SessionState> {
    if v1.tabs.len() > MAX_SESSION_ITEMS
        || (v1.tabs.is_empty() && v1.active != 0)
        || (!v1.tabs.is_empty() && v1.active >= v1.tabs.len())
    {
        return None;
    }
    let profile = ProfileId::generate();
    let space = SpaceId::generate();
    let items: Vec<PersistedItem> = v1
        .tabs
        .into_iter()
        .map(|t| PersistedItem {
            id: ItemId::generate(),
            parent: None,
            placement: Placement::Space {
                space,
                section: SpaceSection::Today,
            },
            kind: PersistedKind::Tab {
                url: t.url,
                title: t.title,
                zoom: 1.0,
                last_active_ms: None,
            },
        })
        .collect();
    let active_item = items.get(v1.active).or(items.first()).map(|i| i.id);
    Some(SessionState {
        profiles: vec![PersistedProfile {
            id: profile,
            name: "Personal".into(),
            kind: ProfileKind::Default,
        }],
        spaces: vec![PersistedSpace {
            id: space,
            profile,
            name: "Space".into(),
            look: Default::default(),
        }],
        items,
        active_space: Some(space),
        active_item,
        splits: None,
        recently_closed: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn v1_blob_becomes_item_tree() {
        let json = r#"{"tabs":[{"url":"https://example.com/","title":"Example"},
                       {"url":"https://github.com/","title":"GitHub"}],"active":1}"#;
        let session = decode(json).unwrap();
        assert_eq!(session.profiles.len(), 1);
        assert_eq!(session.spaces.len(), 1);
        assert_eq!(session.items.len(), 2);
        assert_eq!(session.active_item, Some(session.items[1].id));
    }

    #[test]
    fn v2_blob_essentials_reach_every_space_of_their_profile() {
        let (profile, first, second) = (ProfileId::from(1), SpaceId::from(2), SpaceId::from(3));
        let json = format!(
            r#"{{"v":2,
                "profiles":[{{"id":"{profile}","name":"P","kind":"default"}}],
                "spaces":[{{"id":"{first}","profile":"{profile}","name":"A"}},
                          {{"id":"{second}","profile":"{profile}","name":"B"}}],
                "items":[{{"id":"{item}","parent":null,"profile":"{profile}","space":null,
                           "section":null,"folder":null,"url":"https://mail.example/","title":"Mail"}}],
                "active_space":"{second}","active_item":"{item}","splits":null}}"#,
            item = ItemId::from(4),
        );

        let session = decode(&json).unwrap();

        let kept = |space| {
            session
                .items
                .iter()
                .filter(|item| item.placement == Placement::Favorites { space })
                .count()
        };
        assert_eq!((kept(first), kept(second)), (1, 1));
        assert_eq!(session.active_item, Some(ItemId::from(4)));
        assert!(session.items.iter().any(|item| item.id == ItemId::from(4)
            && item.placement == Placement::Favorites { space: second }));
    }

    #[test]
    fn v2_blob_corrupt_rows_reject_the_entire_import() {
        let json = r#"{"v":2,
            "profiles":[{"id":"not-a-ulid","name":"X","kind":"default"}],
            "spaces":[],"items":[],"active_space":null,"active_item":null,"splits":null}"#;
        assert!(decode(json).is_none());
    }

    #[test]
    fn oversized_legacy_snapshot_is_rejected_before_materialization() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("default.sqlite");
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE session (id INTEGER PRIMARY KEY, data TEXT NOT NULL);
             CREATE TABLE history (
                 id INTEGER PRIMARY KEY, url TEXT, title TEXT, visited_at INTEGER
             );",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO session(id, data)
             VALUES (1, CAST(zeroblob(?1) AS TEXT))",
            [MAX_LEGACY_SESSION_BYTES + 1],
        )
        .unwrap();
        drop(conn);

        assert!(read(&path).is_none());
    }

    #[test]
    fn legacy_history_filters_urls_and_sanitizes_titles() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE history (
                 id INTEGER PRIMARY KEY, url TEXT, title TEXT, visited_at INTEGER
             );
             INSERT INTO history VALUES
                 (1, 'file:///etc/passwd', 'Local', 1),
                 (2, 'https://example.com/', char(8238) || 'Example' || char(10), 2);",
        )
        .unwrap();

        let visits = read_visits(&conn);
        assert_eq!(
            visits,
            vec![("https://example.com/".into(), "Example".into(), 2)]
        );
    }

    #[test]
    fn legacy_history_retention_has_an_aggregate_byte_budget() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE history (
                 id INTEGER PRIMARY KEY, url TEXT, title TEXT, visited_at INTEGER
             );
             INSERT INTO history VALUES
                 (1, 'https://example.com/older', 'Old', 1),
                 (2, 'https://example.com/newest', 'New', 2);",
        )
        .unwrap();
        let budget = "https://example.com/newest".len() + "New".len();
        assert_eq!(
            read_visits_with_budget(&conn, budget),
            vec![("https://example.com/newest".into(), "New".into(), 2)]
        );
    }

    #[test]
    fn v1_item_and_focus_limits_are_fail_closed() {
        let too_many = V1Session {
            tabs: (0..=MAX_SESSION_ITEMS)
                .map(|index| V1Tab {
                    url: format!("https://example.com/{index}"),
                    title: "Tab".into(),
                })
                .collect(),
            active: 0,
        };
        assert!(decode_v1(too_many).is_none());
        assert!(decode_v1(V1Session {
            tabs: vec![V1Tab {
                url: "https://example.com/".into(),
                title: "Tab".into(),
            }],
            active: 1,
        })
        .is_none());
    }

    #[test]
    fn legacy_collection_limit_is_enforced_during_deserialization() {
        let tab = r#"{"url":"https://example.com/","title":"Tab"}"#;
        let tabs = vec![tab; MAX_SESSION_ITEMS + 1].join(",");
        let json = format!(r#"{{"tabs":[{tabs}],"active":0}}"#);
        assert!(decode(&json).is_none());
    }

    #[test]
    fn near_legacy_cap_oversized_string_is_rejected_by_lexical_preflight() {
        let title = "x".repeat(MAX_LEGACY_SESSION_BYTES as usize - 1024);
        let json = format!(
            r#"{{"tabs":[{{"url":"https://example.com/","title":"{title}"}}],"active":0}}"#
        );
        assert!(json.len() < MAX_LEGACY_SESSION_BYTES as usize);
        assert!(json.len() > MAX_LEGACY_SESSION_BYTES as usize - 2048);
        assert!(decode(&json).is_none());
    }
}
