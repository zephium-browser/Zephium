use super::work_planning_tests::{drive, fixture};
use crate::work_context::WorkContextAdmission;
use crate::Command;
use std::time::Duration;
use zephium_core::ids::ResourceId;
use zephium_core::ports::engine::EngineEvent;
use zephium_core::resources::*;
use zephium_core::work::{context::*, environment::*, port::*, *};

async fn environment(
    shell: &mut crate::Shell,
    queue: &crate::actor::CommandQueue,
    handle: &crate::Handle,
    profile: zephium_core::ids::ProfileId,
    call: WorkEnvironmentCall,
) -> WorkEnvironmentSnapshot {
    let request = handle
        .submit_work_document(
            WorkRequest::Environment {
                call,
                space_available: true,
                browser_available: true,
                note_available: false,
            },
            Some(profile),
        )
        .unwrap();
    let projection = drive(shell, queue, request).await.unwrap();
    match projection.reply {
        WorkReply::Environment(WorkEnvironmentReply::Applied { snapshot, .. })
        | WorkReply::Environment(WorkEnvironmentReply::Snapshot { snapshot }) => *snapshot,
        _ => panic!("expected an environment snapshot"),
    }
}

struct OneNote {
    id: String,
    revision: String,
}

impl zephium_core::ports::notes::Notes for OneNote {
    fn call(
        &self,
        _: zephium_core::ids::ProfileId,
        call: zephium_core::notes::NoteCall,
        done: zephium_core::notes::NoteDone,
    ) {
        use zephium_core::notes::*;
        done(match call {
            NoteCall::Get { id } if id == self.id => NoteResponse::Record {
                record: NoteRecord {
                    summary: NoteSummary {
                        id,
                        revision: self.revision.clone(),
                        title: "Keyboard notes".into(),
                        preview: "Budget 150 EUR, tenkeyless".into(),
                        pinned: false,
                        trashed: false,
                        editable: true,
                        created_at: "0".into(),
                        modified_at: "0".into(),
                        path: "Keyboard notes.md".into(),
                    },
                    markdown: "# Keyboard notes\n\nBudget 150 EUR, tenkeyless\n".into(),
                },
            },
            _ => NoteResponse::Error {
                error: NoteError::NotFound,
            },
        });
    }

    fn release(&self, _: zephium_core::ids::ProfileId, done: Box<dyn FnOnce() + Send>) -> bool {
        done();
        true
    }
}

#[tokio::test]
async fn context_admission_binds_digests_and_refuses_stale_or_private_public_reads() {
    let store = std::sync::Arc::new(zephium_store::SqliteStore::in_memory().unwrap());
    let (mut shell, queue, handle, profile) = fixture(store);
    let space = shell.windows.focused().unwrap().space;
    let tab = shell.windows.focused().unwrap().active.unwrap();
    shell.handle(Command::Navigate {
        id: tab,
        input: "https://docs.example/guide".into(),
    });
    shell.handle(Command::Engine(EngineEvent::UrlChanged {
        id: tab,
        url: "https://docs.example/guide".into(),
    }));
    shell.handle(Command::Engine(EngineEvent::TitleChanged {
        id: tab,
        title: "Guide".into(),
    }));

    // Notes are Markdown files served by the notes service, not store rows.
    let resource = ResourceId::generate();
    shell.self_queue = Some(queue.clone());
    let note_revision = "0123456789abcdef0123456789abcdef".to_owned();
    shell.handle(Command::AttachNotes(crate::api::NotesAttachment(
        std::sync::Arc::new(OneNote {
            id: resource.to_string(),
            revision: note_revision.clone(),
        }),
    )));

    let created = environment(
        &mut shell,
        &queue,
        &handle,
        profile,
        WorkEnvironmentCall::Command {
            command: WorkCommandId::generate(),
            intent: WorkEnvironmentIntent::Create {
                space,
                title: "Keyboards".into(),
            },
        },
    )
    .await;
    let with_note = environment(
        &mut shell,
        &queue,
        &handle,
        profile,
        WorkEnvironmentCall::Command {
            command: WorkCommandId::generate(),
            intent: WorkEnvironmentIntent::Edit {
                id: created.id,
                expected: created.revision,
                edit: WorkEnvironmentEdit::Add {
                    reference: WorkEnvironmentReference::Resource { resource },
                    area: None,
                },
            },
        },
    )
    .await;
    let snapshot = environment(
        &mut shell,
        &queue,
        &handle,
        profile,
        WorkEnvironmentCall::Command {
            command: WorkCommandId::generate(),
            intent: WorkEnvironmentIntent::Edit {
                id: created.id,
                expected: with_note.revision,
                edit: WorkEnvironmentEdit::Add {
                    reference: WorkEnvironmentReference::Browser { tab },
                    area: None,
                },
            },
        },
    )
    .await;
    let element = |kind: &str| {
        snapshot
            .elements
            .iter()
            .find(|element| {
                matches!(
                    (&element.reference, kind),
                    (WorkEnvironmentReference::Resource { .. }, "note")
                        | (WorkEnvironmentReference::Browser { .. }, "tab")
                )
            })
            .unwrap()
            .id
    };
    let selection = WorkContextSelectionV1 {
        environment: snapshot.id,
        items: vec![
            WorkContextSelectionItem {
                element: element("note"),
                revision: note_revision.clone(),
            },
            WorkContextSelectionItem {
                element: element("tab"),
                revision: "https://docs.example/guide".into(),
            },
        ],
        tabs: false,
        blocks: false,
    };

    let admission = WorkContextAdmission::new(handle.clone());
    let admitted = drive(
        &mut shell,
        &queue,
        admission.admit(profile, WorkContextPurpose::Planning, &selection),
    )
    .await
    .unwrap();
    let manifest = &admitted.disclosure;
    assert_eq!(manifest.environment, snapshot.id);
    assert_eq!(manifest.environment_revision, snapshot.revision);
    assert_eq!(manifest.items.len(), 2);
    assert_eq!(manifest.items[0].kind, WorkContextItemKind::Note);
    assert_eq!(manifest.items[0].title, "Keyboard notes");
    assert_eq!(manifest.items[0].visibility, WorkContextVisibility::Private);
    assert_eq!(manifest.items[0].digest.len(), 64);
    assert_eq!(manifest.items[1].kind, WorkContextItemKind::Tab);
    assert_eq!(manifest.items[1].title, "Guide");
    assert!(admitted.bodies[0].text.contains("Budget 150 EUR"));
    assert!(admitted.bodies[1]
        .text
        .contains("https://docs.example/guide"));
    assert_eq!(
        manifest.total_bytes as usize,
        admitted.bodies.iter().map(|b| b.text.len()).sum::<usize>()
    );
    assert!(manifest.requires_review());

    let public = drive(
        &mut shell,
        &queue,
        admission.admit(profile, WorkContextPurpose::PublicRead, &selection),
    )
    .await;
    assert!(matches!(public, Err(WorkError::ReviewRequired)));
    let preview = drive(
        &mut shell,
        &queue,
        admission.preview(profile, WorkContextPurpose::PublicRead, &selection),
    )
    .await
    .unwrap();
    assert_eq!(preview.purpose, WorkContextPurpose::PublicRead);
    assert_eq!(preview.items.len(), 2);

    let mut stale = selection.clone();
    stale.items[0].revision = "999999".into();
    let refused = drive(
        &mut shell,
        &queue,
        admission.admit(profile, WorkContextPurpose::Planning, &stale),
    )
    .await;
    assert!(matches!(refused, Err(WorkError::Conflict)));

    // A recorded decision rides reviewed work as an implicit private item and
    // never a public read.
    let decided = environment(
        &mut shell,
        &queue,
        &handle,
        profile,
        WorkEnvironmentCall::Command {
            command: WorkCommandId::generate(),
            intent: WorkEnvironmentIntent::Edit {
                id: created.id,
                expected: snapshot.revision,
                edit: WorkEnvironmentEdit::Decide {
                    element: element("tab"),
                    choice: "Follow this guide".into(),
                },
            },
        },
    )
    .await;
    assert_eq!(decided.decisions.len(), 1);
    // A concrete block reference grants lazy target resolution, not disclosure
    // of the rest of the canvas or its saved private decisions.
    let blocks_only = WorkContextSelectionV1 {
        environment: snapshot.id,
        items: vec![],
        tabs: false,
        blocks: true,
    };
    let block_context = drive(
        &mut shell,
        &queue,
        admission.admit(profile, WorkContextPurpose::Agent, &blocks_only),
    )
    .await
    .unwrap();
    assert!(block_context.bodies.is_empty());
    assert!(block_context.disclosure.items.is_empty());
    assert_eq!(block_context.disclosure.total_bytes, 0);
    assert!(block_context.disclosure.requires_review());

    let with_decision = drive(
        &mut shell,
        &queue,
        admission.admit(profile, WorkContextPurpose::Planning, &selection),
    )
    .await
    .unwrap();
    assert_eq!(with_decision.disclosure.items.len(), 3);
    let decision = &with_decision.disclosure.items[2];
    assert!(decision.implicit);
    assert_eq!(decision.kind, WorkContextItemKind::Decision);
    assert_eq!(decision.element, element("tab"));
    assert_eq!(decision.title, "Guide");
    assert_eq!(with_decision.bodies[2].text, "Follow this guide");
    assert_eq!(
        with_decision.disclosure.environment_revision,
        decided.revision
    );
    let public_only = WorkContextSelectionV1 {
        environment: snapshot.id,
        items: vec![],
        tabs: false,
        blocks: false,
    };
    assert!(public_only.validate().is_err());

    let missing = WorkContextSelectionV1 {
        environment: snapshot.id,
        items: vec![WorkContextSelectionItem {
            element: WorkElementId::generate(),
            revision: "x".into(),
        }],
        tabs: false,
        blocks: false,
    };
    let not_found = drive(
        &mut shell,
        &queue,
        admission.admit(profile, WorkContextPurpose::Planning, &missing),
    )
    .await;
    assert!(matches!(not_found, Err(WorkError::NotFound)));
}

#[tokio::test]
async fn work_media_context_keeps_revision_privacy_and_existing_budget() {
    let dir = tempfile::tempdir().unwrap();
    let store = std::sync::Arc::new(zephium_store::SqliteStore::open(dir.path()).unwrap());
    let (mut shell, queue, handle, profile) = fixture(store);
    let space = shell.windows.focused().unwrap().space;
    let receiver = handle.import_media(
        profile,
        MediaImport {
            request_id: "work-media-context-fixture-0001".into(),
            name: "notes.txt".into(),
            origin: MediaOrigin::Imported,
            bytes: std::sync::Arc::new("context text\n".repeat(2000).into_bytes()),
        },
    );
    let reply = drive(&mut shell, &queue, async {
        loop {
            if let Ok(reply) = receiver.try_recv() {
                break reply;
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await;
    let ResourceResponse::Applied { record, .. } = reply.response else {
        panic!()
    };
    let created = environment(
        &mut shell,
        &queue,
        &handle,
        profile,
        WorkEnvironmentCall::Command {
            command: WorkCommandId::generate(),
            intent: WorkEnvironmentIntent::Create {
                space,
                title: "Media context".into(),
            },
        },
    )
    .await;
    let snapshot = environment(
        &mut shell,
        &queue,
        &handle,
        profile,
        WorkEnvironmentCall::Command {
            command: WorkCommandId::generate(),
            intent: WorkEnvironmentIntent::Edit {
                id: created.id,
                expected: created.revision,
                edit: WorkEnvironmentEdit::Add {
                    reference: WorkEnvironmentReference::Resource {
                        resource: ResourceId::parse(&record.id).unwrap(),
                    },
                    area: None,
                },
            },
        },
    )
    .await;
    let selection = WorkContextSelectionV1 {
        environment: snapshot.id,
        items: vec![WorkContextSelectionItem {
            element: snapshot.elements[0].id,
            revision: record.revision.clone(),
        }],
        tabs: false,
        blocks: false,
    };
    let admission = WorkContextAdmission::new(handle);
    let admitted = drive(
        &mut shell,
        &queue,
        admission.admit(profile, WorkContextPurpose::Agent, &selection),
    )
    .await
    .unwrap();
    assert_eq!(admitted.disclosure.items[0].revision, record.revision);
    assert_eq!(
        admitted.disclosure.items[0].kind,
        WorkContextItemKind::Object
    );
    assert_eq!(
        admitted.disclosure.items[0].visibility,
        WorkContextVisibility::Private
    );
    assert!(admitted.disclosure.items[0].truncated);
    assert!(admitted.bodies[0].text.contains("1 pages)\ncontext text"));
    assert!(admitted.bodies[0].text.len() <= MAX_CONTEXT_ITEM_BYTES);
    assert!(matches!(
        drive(
            &mut shell,
            &queue,
            admission.admit(profile, WorkContextPurpose::PublicRead, &selection)
        )
        .await,
        Err(WorkError::ReviewRequired)
    ));
}

#[tokio::test]
async fn open_tabs_join_context_only_with_consent_and_without_their_query() {
    let store = std::sync::Arc::new(zephium_store::SqliteStore::in_memory().unwrap());
    let (mut shell, queue, handle, profile) = fixture(store);
    let space = shell.windows.focused().unwrap().space;
    let tab = shell.windows.focused().unwrap().active.unwrap();
    shell.handle(Command::Navigate {
        id: tab,
        input: "https://mail.example.com/u/0/?search=inbox".into(),
    });
    shell.handle(Command::Engine(EngineEvent::UrlChanged {
        id: tab,
        url: "https://mail.example.com/u/0/?search=inbox".into(),
    }));
    shell.handle(Command::Engine(EngineEvent::TitleChanged {
        id: tab,
        title: "Inbox (3)".into(),
    }));
    let created = environment(
        &mut shell,
        &queue,
        &handle,
        profile,
        WorkEnvironmentCall::Command {
            command: WorkCommandId::generate(),
            intent: WorkEnvironmentIntent::Create {
                space,
                title: "Research".into(),
            },
        },
    )
    .await;
    let admission = WorkContextAdmission::new(handle.clone());
    let consent = WorkContextSelectionV1 {
        environment: created.id,
        items: vec![],
        tabs: true,
        blocks: false,
    };
    let admitted = drive(
        &mut shell,
        &queue,
        admission.admit(profile, WorkContextPurpose::Agent, &consent),
    )
    .await
    .unwrap();
    assert_eq!(
        admitted.disclosure.tabs,
        [WorkContextTabV1 {
            title: "Inbox (3)".into(),
            host: "mail.example.com".into(),
            path: "/u/0/".into(),
            signed_in: false,
        }]
    );
    assert!(admitted.bodies.is_empty());
    assert!(admitted.disclosure.requires_review());
    // Tabs are the person's browsing: never public-read context.
    let public = drive(
        &mut shell,
        &queue,
        admission.admit(profile, WorkContextPurpose::PublicRead, &consent),
    )
    .await;
    assert!(matches!(public, Err(WorkError::ReviewRequired)));
    // Without consent no tab is listed.
    assert!(WorkContextSelectionV1 {
        tabs: false,
        blocks: false,
        ..consent
    }
    .validate()
    .is_err());
}
