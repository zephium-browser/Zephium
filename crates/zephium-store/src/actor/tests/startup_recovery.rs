use super::*;
use zephium_core::work::{environment::*, port::*, WorkRevision};

fn composer(
    store: &SqliteStore,
    space: SpaceId,
    expected: Option<WorkRevision>,
) -> WorkComposerDraft {
    let (send, receive) = mpsc::channel();
    store
        .work_document(
            ProfileId::from(1),
            WorkRequest::Environment {
                call: expected.map_or(WorkEnvironmentCall::ComposerRead { space }, |expected| {
                    WorkEnvironmentCall::ComposerSave {
                        space,
                        expected,
                        text: "unsent words survive failed starts".into(),
                    }
                }),
                space_available: true,
                browser_available: false,
                note_available: false,
            },
            Box::new(move |result| {
                send.send(result).unwrap();
            }),
        )
        .unwrap();
    match receive.recv_timeout(REPLY_TIMEOUT).unwrap().unwrap() {
        WorkReply::Environment(WorkEnvironmentReply::Composer { draft }) => draft,
        _ => panic!("expected composer"),
    }
}

#[test]
fn explicit_startup_recovery_preserves_exact_session_and_drafts_without_restoring_tabs() {
    let dir = tempfile::tempdir().unwrap();
    let store = SqliteStore::open(dir.path()).unwrap();
    let state = sample();
    let space = state.spaces[0].id;
    store.save_session(state.clone());
    assert!(store.flush());
    let kept = composer(&store, space, Some(WorkRevision::INITIAL));
    let metadata = Connection::open(dir.path().join("meta.sqlite")).unwrap();
    let original: Vec<u8> = metadata
        .query_row(
            "SELECT CAST(data AS BLOB) FROM session_snapshot WHERE id=1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let (loaded, artifact) = store.set_aside_startup_session().unwrap();
    let SessionLoad::Loaded {
        state: recovered, ..
    } = loaded
    else {
        panic!("expected recovered state");
    };
    assert!(!state.items.is_empty());
    assert!(recovered.items.is_empty());
    assert_eq!(recovered.spaces, state.spaces);
    assert_eq!(std::fs::read(artifact.unwrap()).unwrap(), original);
    assert_eq!(composer(&store, space, None), kept);
    drop(store);
    let store = SqliteStore::open(dir.path()).unwrap();
    let SessionLoad::Loaded { state, .. } = store.load_session() else {
        panic!("expected reload");
    };
    assert!(state.items.is_empty());
    assert_eq!(composer(&store, space, None), kept);
}

#[test]
fn startup_recovery_refuses_newer_session_bytes_without_replacing_them() {
    let dir = tempfile::tempdir().unwrap();
    let store = SqliteStore::open(dir.path()).unwrap();
    store.save_session(sample());
    assert!(store.flush());
    drop(store);
    let metadata = Connection::open(dir.path().join("meta.sqlite")).unwrap();
    metadata
        .execute("UPDATE session_snapshot SET schema_version=9999", [])
        .unwrap();
    let original: Vec<u8> = metadata
        .query_row(
            "SELECT CAST(data AS BLOB) FROM session_snapshot WHERE id=1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let store = SqliteStore::open(dir.path()).unwrap();
    assert!(store.set_aside_startup_session().is_none());
    assert_eq!(
        metadata
            .query_row(
                "SELECT CAST(data AS BLOB) FROM session_snapshot WHERE id=1",
                [],
                |row| row.get::<_, Vec<u8>>(0)
            )
            .unwrap(),
        original
    );
    assert_eq!(
        metadata
            .query_row(
                "SELECT schema_version FROM session_snapshot WHERE id=1",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        9999
    );
}

#[test]
fn startup_recovery_cannot_enter_after_terminal_store_admission() {
    let dir = tempfile::tempdir().unwrap();
    let store = SqliteStore::open(dir.path()).unwrap();
    store.save_session(sample());
    assert!(store.flush());
    let metadata = Connection::open(dir.path().join("meta.sqlite")).unwrap();
    let original: Vec<u8> = metadata
        .query_row(
            "SELECT CAST(data AS BLOB) FROM session_snapshot WHERE id=1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        store.shutdown_until(Instant::now() + Duration::from_secs(5)),
        StoreShutdownOutcome::Clean
    );
    assert!(store.set_aside_startup_session().is_none());
    assert_eq!(
        metadata
            .query_row(
                "SELECT CAST(data AS BLOB) FROM session_snapshot WHERE id=1",
                [],
                |row| row.get::<_, Vec<u8>>(0)
            )
            .unwrap(),
        original
    );
    assert!(!std::fs::read_dir(dir.path())
        .unwrap()
        .filter_map(Result::ok)
        .any(|entry| entry
            .file_name()
            .to_string_lossy()
            .starts_with("recovered-session-")));
}
