use super::chats::warm_schema_cache;
use super::*;
use zephium_core::zoom::SavedLevel;

fn profile() -> ProfileId {
    ProfileId::from(1)
}

fn levels(store: &SqliteStore) -> Vec<(String, u16)> {
    store
        .zoom_levels(profile())
        .into_iter()
        .map(|SavedLevel { site, percent, .. }| (site, percent))
        .collect()
}

#[test]
fn levels_round_trip_through_the_store_and_survive_a_reopen() {
    warm_schema_cache();
    let dir = tempfile::tempdir().unwrap();
    {
        let store = SqliteStore::open(dir.path()).unwrap();
        // The profile is only queued, not yet durable: saving still works.
        store.save_session(sample());
        assert!(store.save_zoom_level(profile(), "github.com".into(), Some(80), 10));
        assert!(store.save_zoom_level(profile(), "example.org".into(), Some(150), 20));
        assert!(store.save_zoom_level(profile(), "github.com".into(), Some(90), 30));
        assert!(store.flush());
        assert_eq!(
            levels(&store),
            [("github.com".into(), 90), ("example.org".into(), 150)]
        );
        assert!(store.save_zoom_level(profile(), "example.org".into(), None, 40));
        assert!(store.flush());
    }
    let store = SqliteStore::open(dir.path()).unwrap();
    assert_eq!(levels(&store), [("github.com".into(), 90)]);
}

#[test]
fn unusable_levels_are_refused_before_they_reach_the_queue() {
    let store = SqliteStore::in_memory().unwrap();
    assert!(!store.save_zoom_level(profile(), String::new(), Some(100), 1));
    assert!(!store.save_zoom_level(profile(), "a".repeat(254), Some(100), 1));
    assert!(!store.save_zoom_level(profile(), "example.com".into(), Some(29), 1));
    assert!(!store.save_zoom_level(profile(), "example.com".into(), Some(301), 1));
}

#[test]
fn clearing_history_drops_the_levels_it_covers() {
    warm_schema_cache();
    let store = SqliteStore::in_memory().unwrap();
    store.save_session(sample());
    assert!(store.save_zoom_level(profile(), "old.example".into(), Some(80), 100));
    assert!(store.save_zoom_level(profile(), "new.example".into(), Some(125), 300));
    assert_eq!(store.clear_history_checked(profile(), Some(200)), Some(0));
    assert_eq!(levels(&store), [("old.example".into(), 80)]);
    assert_eq!(store.clear_history_checked(profile(), None), Some(0));
    assert!(levels(&store).is_empty());
}
