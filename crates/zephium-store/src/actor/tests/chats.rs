use super::*;
use zephium_core::ports::store::{ChatSummary, MAX_CHATS, MAX_CHAT_BODY_BYTES, MAX_CHAT_ID_BYTES};

const PROFILE: u128 = 1;

fn profile() -> ProfileId {
    ProfileId::from(PROFILE)
}

// The first open of a profile database validates every schema version, which a
// loaded debug build can spend longer on than the store's reply budget.
pub(super) fn warm_schema_cache() {
    let mut conn = Connection::open_in_memory().unwrap();
    migrations::apply(&mut conn, migrations::PROFILE).unwrap();
}

#[test]
fn conversations_round_trip_through_the_store_and_survive_a_reopen() {
    warm_schema_cache();
    let dir = tempfile::tempdir().unwrap();
    {
        let store = SqliteStore::open(dir.path()).unwrap();
        // The profile is only queued, not yet durable: saving still works.
        store.save_session(sample());
        assert!(store.save_chat(
            profile(),
            "chat-1",
            "space-a",
            "First",
            10_000,
            "{\"a\":1}".into()
        ));
        assert!(store.save_chat(
            profile(),
            "chat-2",
            "space-b",
            "Second",
            20_000,
            "{}".into()
        ));
        assert!(store.flush());
    }

    let store = SqliteStore::open(dir.path()).unwrap();
    assert_eq!(
        store.chats(profile(), 10),
        vec![
            ChatSummary {
                id: "chat-2".into(),
                space: "space-b".into(),
                title: "Second".into(),
                updated_ms: 20_000,
            },
            ChatSummary {
                id: "chat-1".into(),
                space: "space-a".into(),
                title: "First".into(),
                updated_ms: 10_000,
            },
        ]
    );
    assert_eq!(
        store.chat(profile(), "chat-1").as_deref(),
        Some("{\"a\":1}")
    );
    assert_eq!(store.chats(profile(), 1).len(), 1);

    assert!(store.delete_chat(profile(), "chat-1"));
    assert!(!store.delete_chat(profile(), "chat-1"));
    assert_eq!(store.chat(profile(), "chat-1"), None);
    assert_eq!(store.clear_chats(profile(), Some(21)), 0);
    assert_eq!(store.clear_chats(profile(), Some(20)), 1);
    assert!(store.chats(profile(), 10).is_empty());
}

#[test]
fn clearing_history_and_clearing_conversations_are_separate() {
    warm_schema_cache();
    let store = SqliteStore::in_memory().unwrap();
    store.save_session(sample());
    assert!(store.flush());
    store.record_visit(profile(), "https://example.com/".into(), "Example".into());
    assert!(store.save_chat(profile(), "chat", "space", "Title", 1, "{}".into()));

    assert_eq!(store.clear_history_checked(profile(), None), Some(1));
    assert_eq!(store.chats(profile(), 10).len(), 1);
    assert_eq!(store.clear_chats(profile(), None), 1);
}

#[test]
fn an_unregistered_profile_stores_nothing_and_gets_no_database() {
    warm_schema_cache();
    let dir = tempfile::tempdir().unwrap();
    let store = SqliteStore::open(dir.path()).unwrap();
    store.save_session(sample());
    assert!(store.flush());
    let stranger = ProfileId::from(999);

    assert!(!store.save_chat(stranger, "chat", "space", "Title", 1, "{}".into()));
    assert!(store.chats(stranger, 10).is_empty());
    assert_eq!(store.chat(stranger, "chat"), None);
    assert!(!store.delete_chat(stranger, "chat"));
    assert_eq!(store.clear_chats(stranger, None), 0);
    assert!(!dir
        .path()
        .join(format!("profile-{stranger}.sqlite"))
        .exists());
}

#[test]
fn a_request_past_a_bound_never_reaches_the_actor() {
    let (tx, rx) = mpsc::sync_channel(8);
    let store = test_store_with_sender(tx);
    let id = "a".repeat(MAX_CHAT_ID_BYTES + 1);

    assert!(!store.save_chat(profile(), &id, "space", "Title", 1, "{}".into()));
    assert!(!store.save_chat(
        profile(),
        "chat",
        "space",
        "Title",
        1,
        "b".repeat(MAX_CHAT_BODY_BYTES + 1)
    ));
    assert!(!store.save_chat(profile(), "no spaces", "space", "Title", 1, "{}".into()));
    assert_eq!(store.chat(profile(), &id), None);
    assert!(!store.delete_chat(profile(), "../up"));
    assert!(store.chats(profile(), 0).is_empty());
    assert!(rx.try_recv().is_err());
}

#[test]
fn a_listing_is_never_asked_for_more_than_the_cap() {
    let (tx, rx) = mpsc::sync_channel(8);
    let store = test_store_with_sender(tx);

    assert!(store.chats(profile(), u32::MAX).is_empty());
    assert!(matches!(
        rx.try_recv(),
        Ok(Cmd::Chats(_, limit, _)) if limit == MAX_CHATS as u32
    ));
}

#[test]
fn deleting_a_profile_takes_its_conversations_with_it() {
    warm_schema_cache();
    let dir = tempfile::tempdir().unwrap();
    let store = SqliteStore::open(dir.path()).unwrap();
    store.save_session(sample());
    assert!(store.flush());
    assert!(store.save_chat(profile(), "chat", "space", "Title", 1, "{}".into()));
    let path = dir.path().join(format!("profile-{}.sqlite", profile()));
    assert!(path.exists());

    let deadline = Instant::now() + REPLY_TIMEOUT;
    assert_eq!(
        store.authorize_profile_deletion(profile(), SessionState::default(), deadline),
        ProfileDeletionAuthorizeOutcome::Authorized
    );
    assert!(store.chats(profile(), 10).is_empty());
    assert!(!store.save_chat(profile(), "late", "space", "Title", 2, "{}".into()));
    assert_eq!(
        store.finalize_profile_deletion(profile(), deadline),
        ProfileDeletionFinalizeOutcome::Completed
    );
    assert!(!path.exists());
}
