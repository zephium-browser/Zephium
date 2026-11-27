use super::*;
use zephium_core::work::{
    environment::{WorkComposerDraft, WorkEnvironmentCall, WorkEnvironmentReply},
    port::{WorkReply, WorkRequest},
    WorkError, WorkRevision,
};

fn composer(
    store: &SqliteStore,
    space: SpaceId,
    expected: Option<WorkRevision>,
    text: &str,
) -> Result<WorkComposerDraft, WorkError> {
    let (done, reply) = mpsc::channel();
    let call = match expected {
        Some(expected) => WorkEnvironmentCall::ComposerSave {
            space,
            expected,
            text: text.into(),
        },
        None => WorkEnvironmentCall::ComposerRead { space },
    };
    store.work_document(
        ProfileId::from(1),
        WorkRequest::Environment {
            call,
            space_available: true,
            browser_available: false,
            note_available: false,
        },
        Box::new(move |result| {
            done.send(result).unwrap();
        }),
    )?;
    match reply.recv_timeout(REPLY_TIMEOUT).unwrap()? {
        WorkReply::Environment(WorkEnvironmentReply::Composer { draft }) => Ok(draft),
        _ => panic!("composer response"),
    }
}

#[test]
fn composer_barrier_persists_new_space_before_ack_and_rejects_delayed_save_after_deletion() {
    let dir = tempfile::tempdir().unwrap();
    let store = SqliteStore::open(dir.path()).unwrap();
    let mut state = sample();
    let space = SpaceId::from(900);
    state.spaces.push(PersistedSpace {
        id: space,
        profile: ProfileId::from(1),
        name: "Just created".into(),
        look: Default::default(),
    });
    store.save_session(state);
    let kept = composer(
        &store,
        space,
        Some(WorkRevision::INITIAL),
        "new space durable words",
    )
    .unwrap();
    drop(store);
    let store = SqliteStore::open(dir.path()).unwrap();
    assert_eq!(composer(&store, space, None, ""), Ok(kept.clone()));
    store.save_session(sample());
    assert_eq!(
        composer(
            &store,
            space,
            Some(WorkRevision::INITIAL),
            "late initial writer"
        ),
        Err(WorkError::NotFound)
    );
    assert_eq!(
        composer(&store, space, Some(kept.revision), "stale attested save"),
        Err(WorkError::NotFound)
    );
    drop(store);
    let store = SqliteStore::open(dir.path()).unwrap();
    assert_eq!(composer(&store, space, None, ""), Err(WorkError::NotFound));
}

#[test]
fn failed_pending_scope_commit_refuses_composer_write_and_preserves_accepted_words() {
    let dir = tempfile::tempdir().unwrap();
    let store = SqliteStore::open(dir.path()).unwrap();
    let state = sample();
    let space = state.spaces[0].id;
    store.save_session(state.clone());
    assert!(store.flush());
    let kept = composer(
        &store,
        space,
        Some(WorkRevision::INITIAL),
        "last accepted words",
    )
    .unwrap();
    let metadata = Connection::open(dir.path().join("meta.sqlite")).unwrap();
    metadata.execute_batch("CREATE TRIGGER fail_composer_scope_commit BEFORE UPDATE ON session_snapshot BEGIN SELECT RAISE(ABORT,'injected scope commit failure'); END;").unwrap();
    let mut changed = state;
    changed.spaces[0].name = "Changed scope".into();
    store.save_session(changed);
    assert_eq!(
        composer(&store, space, Some(kept.revision), "must not acknowledge"),
        Err(WorkError::Unavailable)
    );
    metadata
        .execute_batch("DROP TRIGGER fail_composer_scope_commit;")
        .unwrap();
    assert!(store.flush());
    assert_eq!(composer(&store, space, None, ""), Ok(kept));
}
