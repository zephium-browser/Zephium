use super::*;
use zephium_core::ids::ProfileId;
fn task() -> ResourceDraft {
    ResourceDraft {
        title: "Research a topic".into(),
        pinned: false,
        content: ResourceContent::Task {
            details: Default::default(),
            description: "Keep the evidence".into(),
            completed: false,
            due_date: Some("2028-02-29".into()),
            due_time: None,
            status: TaskStatus::Open,
            assignee: TaskActor::User,
            origin: TaskActor::User,
            context: None,
            sort_key: None,
            work: None,
        },
        related: vec![],
    }
}
fn command(key: &str, intent: ResourceIntent) -> ResourceCommand {
    ResourceCommand {
        version: 1,
        request_id: format!("request-{key:0>16}"),
        intent,
    }
}
fn connection(path: &std::path::Path) -> Connection {
    let mut conn = Connection::open(path).unwrap();
    configure(&conn).unwrap();
    migrations::apply(&mut conn, migrations::PROFILE).unwrap();
    conn
}
fn applied(response: ResourceResponse) -> ResourceRecord {
    match response {
        ResourceResponse::Applied { record, .. } => record,
        other => panic!("unexpected {other:?}"),
    }
}
#[test]
fn durable_create_retry_conflict_trash_and_restore() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("resources.sqlite");
    let mut conn = connection(&path);
    let create = command("create", ResourceIntent::Create { draft: task() });
    let first = applied(mutate(&mut conn, ProfileId::from(1), create.clone()).unwrap());
    drop(conn);
    let mut conn = connection(&path);
    let replay = applied(mutate(&mut conn, ProfileId::from(1), create.clone()).unwrap());
    assert_eq!(first, replay);
    let mut draft = task();
    draft.title = "Changed".into();
    assert!(matches!(
        mutate(
            &mut conn,
            ProfileId::from(1),
            ResourceCommand {
                intent: ResourceIntent::Create {
                    draft: draft.clone()
                },
                ..create
            }
        )
        .unwrap(),
        ResourceResponse::Error {
            error: ResourceError::Conflict
        }
    ));
    let updated = applied(
        mutate(
            &mut conn,
            ProfileId::from(1),
            command(
                "edit",
                ResourceIntent::Replace {
                    id: first.id.clone(),
                    expected_revision: first.revision.clone(),
                    draft,
                },
            ),
        )
        .unwrap(),
    );
    assert_eq!(updated.revision, "2");
    assert!(matches!(
        mutate(
            &mut conn,
            ProfileId::from(1),
            command(
                "stale",
                ResourceIntent::Trash {
                    id: first.id.clone(),
                    expected_revision: first.revision
                }
            )
        )
        .unwrap(),
        ResourceResponse::Error {
            error: ResourceError::Conflict
        }
    ));
    let deleted = applied(
        mutate(
            &mut conn,
            ProfileId::from(1),
            command(
                "trash",
                ResourceIntent::Trash {
                    id: first.id,
                    expected_revision: updated.revision,
                },
            ),
        )
        .unwrap(),
    );
    assert!(deleted.trashed);
    let restored = applied(
        mutate(
            &mut conn,
            ProfileId::from(1),
            command(
                "restore",
                ResourceIntent::Restore {
                    id: deleted.id,
                    expected_revision: deleted.revision,
                },
            ),
        )
        .unwrap(),
    );
    assert!(!restored.trashed);
    assert_eq!(restored.draft.title, "Changed");
}
#[test]
fn unknown_profiles_and_cross_profile_links_fail_closed() {
    let mut hub = Hub::in_memory().unwrap();
    let a = ProfileId::from(1);
    let b = ProfileId::from(2);
    let call = ResourceCall::Mutate {
        command: Box::new(command("a", ResourceIntent::Create { draft: task() })),
    };
    assert!(matches!(
        hub.resource_call(a, call.clone()),
        ResourceResponse::Error {
            error: ResourceError::Unavailable
        }
    ));
    hub.registry.insert(a);
    hub.registry.insert(b);
    let record = applied(hub.resource_call(a, call));
    assert!(matches!(
        hub.resource_call(
            b,
            ResourceCall::Get {
                id: record.id.clone()
            }
        ),
        ResourceResponse::Error {
            error: ResourceError::NotFound
        }
    ));
    let mut draft = task();
    draft.related.push(record.id);
    assert!(matches!(
        hub.resource_call(
            b,
            ResourceCall::Mutate {
                command: Box::new(command("b", ResourceIntent::Create { draft }))
            }
        ),
        ResourceResponse::Error {
            error: ResourceError::NotFound
        }
    ));
}
#[test]
fn acknowledged_updates_still_cannot_reapply_an_old_revision() {
    let mut hub = Hub::in_memory().unwrap();
    let profile = ProfileId::from(1);
    hub.registry.insert(profile);
    let create = command("create", ResourceIntent::Create { draft: task() });
    let first = applied(hub.resource_call(
        profile,
        ResourceCall::Mutate {
            command: Box::new(create.clone()),
        },
    ));
    hub.resource_call(
        profile,
        ResourceCall::Acknowledge {
            request_id: create.request_id.clone(),
        },
    );
    assert_eq!(
        applied(hub.resource_call(
            profile,
            ResourceCall::Mutate {
                command: Box::new(create)
            }
        ))
        .id,
        first.id
    );
    let update = command(
        "update",
        ResourceIntent::Replace {
            id: first.id,
            expected_revision: first.revision,
            draft: task(),
        },
    );
    hub.resource_call(
        profile,
        ResourceCall::Mutate {
            command: Box::new(update.clone()),
        },
    );
    hub.resource_call(
        profile,
        ResourceCall::Acknowledge {
            request_id: update.request_id.clone(),
        },
    );
    assert!(matches!(
        hub.resource_call(
            profile,
            ResourceCall::Mutate {
                command: Box::new(update)
            }
        ),
        ResourceResponse::Error {
            error: ResourceError::Conflict
        }
    ));
}

#[test]
fn schema28_widens_resource_kinds_and_keeps_receipts_joined() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("resources.sqlite");
    let mut conn = Connection::open(&path).unwrap();
    configure(&conn).unwrap();
    migrations::apply(&mut conn, &migrations::PROFILE[..30]).unwrap();
    let create = command("create", ResourceIntent::Create { draft: task() });
    let first = applied(mutate(&mut conn, ProfileId::from(1), create.clone()).unwrap());
    let bytes_before: i64 = conn
        .query_row(
            "SELECT bytes FROM user_resource_usage WHERE id=1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    migrations::apply(&mut conn, migrations::PROFILE).unwrap();
    assert!(migrations::apply(&mut conn, &migrations::PROFILE[..30]).is_err());
    conn.execute(
        "INSERT INTO resource_titles_fts(resource_titles_fts) VALUES('integrity-check')",
        [],
    )
    .unwrap();
    let violations: i64 = conn
        .query_row(
            "SELECT count(*) FROM pragma_foreign_key_check('user_resource_receipts')",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(violations, 0);
    let bytes_after: i64 = conn
        .query_row(
            "SELECT bytes FROM user_resource_usage WHERE id=1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(bytes_before, bytes_after);
    let joined: i64 = conn
        .query_row(
            "SELECT count(*) FROM user_resource_receipts r JOIN user_resources u ON u.id=r.resource_id WHERE r.request_id=?1",
            [&create.request_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(joined, 1);
    let replay = applied(mutate(&mut conn, ProfileId::from(1), create).unwrap());
    assert_eq!(replay.id, first.id);
    let object = ResourceDraft {
        title: "Shortlist".into(),
        pinned: false,
        content: ResourceContent::Object {
            object: WorkObjectV1 {
                version: 1,
                data: zephium_core::work::artifact::WorkArtifactDataV1::Checklist {
                    items: vec![zephium_core::work::artifact::WorkChecklistItem {
                        text: "Compare pricing".into(),
                        completed: false,
                    }],
                },
                evidence: vec![],
                provenance: None,
            },
        },
        related: vec![],
    };
    let created = applied(
        mutate(
            &mut conn,
            ProfileId::from(1),
            command("object", ResourceIntent::Create { draft: object }),
        )
        .unwrap(),
    );
    assert_eq!(created.draft.kind(), ResourceKind::Object);
    let page = list(
        &conn,
        ResourceQuery {
            completed: None,
            kind: ResourceKind::Object,
            search: "pricing".into(),
            trashed: false,
            after: None,
            limit: 10,
        },
    )
    .unwrap();
    let ResourceResponse::Page { items, .. } = page else {
        panic!("expected page");
    };
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].title, "Shortlist");
}

#[test]
fn legacy_notes_are_read_out_once_and_never_written_again() {
    let mut hub = Hub::in_memory().unwrap();
    let profile = ProfileId::from(1);
    hub.registry.insert(profile);
    let conn = hub.profile_conn(profile).unwrap();
    conn.execute("INSERT INTO user_resources(id,kind,revision,title,pinned,trashed,created_at,updated_at,body,search_text) VALUES('01J9ZQ3V6Q4M8Y2K7T5R1N0B3A','note',1,'Plans',1,0,1,1,'{\"title\":\"Plans\",\"pinned\":true,\"content\":{\"kind\":\"note\",\"document\":{\"version\":1,\"document\":{\"type\":\"doc\",\"content\":[{\"type\":\"paragraph\"}]}}},\"related\":[]}','plans')", []).unwrap();
    conn.execute("INSERT INTO user_resource_receipts(request_id,digest,resource_id,revision,retained) VALUES('request-legacy-note-0001',zeroblob(32),'01J9ZQ3V6Q4M8Y2K7T5R1N0B3A',1,1)", []).unwrap();
    let notes = hub.legacy_notes(profile).unwrap();
    assert_eq!(notes.len(), 1);
    assert!(notes[0].draft.pinned);
    let note = notes[0].draft.clone();
    let replace = ResourceCall::Mutate {
        command: Box::new(command(
            "replace",
            ResourceIntent::Replace {
                id: notes[0].id.clone(),
                expected_revision: notes[0].revision.clone(),
                draft: note.clone(),
            },
        )),
    };
    assert!(matches!(
        hub.resource_call(profile, replace),
        ResourceResponse::Error {
            error: ResourceError::Invalid
        }
    ));
    let create = ResourceCall::Mutate {
        command: Box::new(command("create", ResourceIntent::Create { draft: note })),
    };
    assert!(matches!(
        hub.resource_call(profile, create),
        ResourceResponse::Error {
            error: ResourceError::Invalid
        }
    ));
    assert!(hub.retire_legacy_notes(profile, &[notes[0].id.clone()]));
    assert!(hub.legacy_notes(profile).unwrap().is_empty());
}

#[test]
fn tasks_written_before_the_lifecycle_load_and_adopt_the_state_they_implied() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("legacy.sqlite");
    let mut conn = Connection::open(&path).unwrap();
    configure(&conn).unwrap();
    migrations::apply(&mut conn, &migrations::PROFILE[..15]).unwrap();
    // Exactly the body shape shipped before status, assignee and origin existed.
    let legacy = r#"{"title":"Renew the certificate","pinned":false,"content":{"kind":"task","description":"","completed":true,"due_date":null},"related":[]}"#;
    conn.execute("INSERT INTO user_resources(id,kind,revision,title,pinned,trashed,created_at,updated_at,body,search_text,completed) VALUES('00000000000000000000000001','task',1,'Renew the certificate',0,0,1,1,?1,'renew the certificate',1)",[legacy]).unwrap();
    migrations::apply(&mut conn, migrations::PROFILE).unwrap();

    let record = get(&conn, "00000000000000000000000001").unwrap().unwrap();
    let ResourceContent::Task {
        status,
        assignee,
        origin,
        completed,
        ..
    } = record.draft.content
    else {
        panic!("task")
    };
    assert!(completed);
    assert_eq!(status, TaskStatus::Done);
    assert_eq!(assignee, TaskActor::User);
    assert_eq!(origin, TaskActor::User);

    let items = match list(
        &conn,
        ResourceQuery {
            completed: None,
            kind: ResourceKind::Task,
            search: String::new(),
            trashed: false,
            after: None,
            limit: 10,
        },
    )
    .unwrap()
    {
        ResourceResponse::Page { items, .. } => items,
        other => panic!("unexpected {other:?}"),
    };
    assert_eq!(items[0].status, Some(TaskStatus::Done));
    assert_eq!(items[0].assignee, Some(TaskActor::User));
    assert_eq!(items[0].origin, Some(TaskActor::User));
}

#[test]
fn a_listed_task_carries_everything_its_row_draws() {
    let dir = tempfile::tempdir().unwrap();
    let mut conn = connection(&dir.path().join("rows.sqlite"));
    let mut draft = task();
    draft.content = ResourceContent::Task {
        details: Default::default(),
        description: "Compare both vendors".into(),
        completed: false,
        due_date: Some("2028-02-29".into()),
        due_time: Some("09:30".into()),
        status: TaskStatus::Blocked,
        assignee: TaskActor::Agent,
        origin: TaskActor::Agent,
        context: Some(TaskContext {
            url: "https://example.com/pricing".into(),
            title: "Pricing".into(),
        }),
        sort_key: Some("m".into()),
        work: None,
    };
    applied(
        mutate(
            &mut conn,
            ProfileId::from(1),
            command("row", ResourceIntent::Create { draft }),
        )
        .unwrap(),
    );
    let items = match list(
        &conn,
        ResourceQuery {
            completed: None,
            kind: ResourceKind::Task,
            search: String::new(),
            trashed: false,
            after: None,
            limit: 10,
        },
    )
    .unwrap()
    {
        ResourceResponse::Page { items, .. } => items,
        other => panic!("unexpected {other:?}"),
    };
    let row = &items[0];
    assert_eq!(row.status, Some(TaskStatus::Blocked));
    assert_eq!(row.assignee, Some(TaskActor::Agent));
    assert_eq!(row.origin, Some(TaskActor::Agent));
    assert_eq!(row.due_date.as_deref(), Some("2028-02-29"));
    assert_eq!(row.due_time.as_deref(), Some("09:30"));
    assert_eq!(row.sort_key.as_deref(), Some("m"));
    assert_eq!(
        row.context.as_ref().map(|c| c.url.as_str()),
        Some("https://example.com/pricing")
    );
    // A note contributes none of it, rather than a default that reads as a task.
    assert!(row.work.is_none());
}

#[test]
fn task_views_filter_before_paging_and_counts_ignore_search() {
    let dir = tempfile::tempdir().unwrap();
    let mut conn = connection(&dir.path().join("tasks.sqlite"));
    let mut old = task();
    old.title = "Old overdue work".into();
    if let ResourceContent::Task { due_date, .. } = &mut old.content {
        *due_date = Some("2026-09-01".into());
    }
    let old = applied(
        mutate(
            &mut conn,
            ProfileId::from(1),
            command("old-task", ResourceIntent::Create { draft: old }),
        )
        .unwrap(),
    );
    for index in 0..105 {
        mutate(
            &mut conn,
            ProfileId::from(1),
            command(
                &format!("future-{index}"),
                ResourceIntent::Create { draft: task() },
            ),
        )
        .unwrap();
    }
    let query = TaskQuery {
        list: None,
        view: TaskView::Today,
        today: "2026-09-22".into(),
        search: String::new(),
        after: None,
        limit: 2,
    };
    let ResourceResponse::TaskPage {
        items,
        counts,
        next,
        ..
    } = task_query::list(&conn, query.clone()).unwrap()
    else {
        panic!()
    };
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].id, old.id);
    assert_eq!(counts.all, 106);
    assert_eq!(counts.overdue, 1);
    assert_eq!(counts.upcoming, 105);
    assert!(next.is_none());
    let ResourceResponse::TaskPage { items, counts, .. } = task_query::list(
        &conn,
        TaskQuery {
            list: None,
            search: "missing".into(),
            ..query
        },
    )
    .unwrap() else {
        panic!()
    };
    assert!(items.is_empty());
    assert_eq!(counts.all, 106);
}

#[test]
fn task_cursor_survives_boundary_deletion_and_rejects_other_views() {
    let dir = tempfile::tempdir().unwrap();
    let mut conn = connection(&dir.path().join("tasks.sqlite"));
    for index in 0..4 {
        mutate(
            &mut conn,
            ProfileId::from(1),
            command(
                &format!("task-{index}"),
                ResourceIntent::Create { draft: task() },
            ),
        )
        .unwrap();
    }
    let query = TaskQuery {
        list: None,
        view: TaskView::All,
        today: "2026-09-22".into(),
        search: String::new(),
        after: None,
        limit: 2,
    };
    let ResourceResponse::TaskPage { items, next, .. } =
        task_query::list(&conn, query.clone()).unwrap()
    else {
        panic!()
    };
    assert_eq!(items.len(), 2);
    conn.execute(
        "UPDATE user_resources SET trashed=1 WHERE id=?1",
        [&items[1].id],
    )
    .unwrap();
    let ResourceResponse::TaskPage {
        items: rest,
        next: end,
        ..
    } = task_query::list(
        &conn,
        TaskQuery {
            list: None,
            after: next.clone(),
            ..query.clone()
        },
    )
    .unwrap()
    else {
        panic!()
    };
    assert_eq!(rest.len(), 2);
    assert!(rest
        .iter()
        .all(|row| items.iter().all(|first| first.id != row.id)));
    assert!(end.is_none());
    assert!(matches!(
        task_query::list(
            &conn,
            TaskQuery {
                list: None,
                view: TaskView::Today,
                after: next,
                ..query
            }
        )
        .unwrap(),
        ResourceResponse::Error {
            error: ResourceError::Invalid
        }
    ));
}

#[test]
fn task_lists_preserve_tasks_and_replay_creation_exactly() {
    let dir = tempfile::tempdir().unwrap();
    let mut conn = connection(&dir.path().join("lists.sqlite"));
    let create = command(
        "list-create",
        ResourceIntent::CreateTaskList {
            title: "Zephium".into(),
        },
    );
    let ResourceResponse::TaskListApplied { list, .. } =
        mutate(&mut conn, ProfileId::from(1), create.clone()).unwrap()
    else {
        panic!()
    };
    let ResourceResponse::TaskListApplied { list: replay, .. } =
        mutate(&mut conn, ProfileId::from(1), create).unwrap()
    else {
        panic!()
    };
    assert_eq!(list.id, replay.id);
    let mut draft = task();
    if let ResourceContent::Task { details, .. } = &mut draft.content {
        details.list = Some(list.id.clone());
        details.steps = vec![TaskStep {
            id: "step-00000000000001".into(),
            title: "Check the keyboard flow".into(),
            completed: false,
        }];
        details.priority = TaskPriority::High;
    }
    let record = applied(
        mutate(
            &mut conn,
            ProfileId::from(1),
            command("list-task", ResourceIntent::Create { draft }),
        )
        .unwrap(),
    );
    let ResourceResponse::TaskPage {
        items,
        metadata,
        lists,
        ..
    } = task_query::list(
        &conn,
        TaskQuery {
            view: TaskView::All,
            list: Some(list.id.clone()),
            today: "2026-09-22".into(),
            search: String::new(),
            after: None,
            limit: 100,
        },
    )
    .unwrap()
    else {
        panic!()
    };
    assert_eq!(items.len(), 1);
    assert_eq!(metadata[0].steps, 1);
    assert_eq!(lists[0].count, 1);
    assert!(matches!(
        mutate(
            &mut conn,
            ProfileId::from(1),
            command(
                "remove-list",
                ResourceIntent::DeleteTaskList {
                    id: list.id,
                    expected_revision: list.revision
                }
            )
        )
        .unwrap(),
        ResourceResponse::TaskListApplied { .. }
    ));
    let kept = get(&conn, &record.id).unwrap().unwrap();
    assert_eq!(kept.draft.title, record.draft.title);
    assert_eq!(kept.revision, "2");
    let ResourceContent::Task { details, .. } = kept.draft.content else {
        panic!()
    };
    assert!(details.inbox);
    assert!(details.list.is_none());
    assert_eq!(details.steps.len(), 1);
    assert_eq!(details.priority, TaskPriority::High);
}

#[test]
fn task_completion_time_is_native_and_foreign_lists_are_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let mut conn = connection(&dir.path().join("complete.sqlite"));
    let mut draft = task();
    if let ResourceContent::Task { details, .. } = &mut draft.content {
        details.list = Some(ResourceId::generate().to_string());
    }
    assert!(matches!(
        mutate(
            &mut conn,
            ProfileId::from(1),
            command("foreign-list", ResourceIntent::Create { draft })
        )
        .unwrap(),
        ResourceResponse::Error {
            error: ResourceError::NotFound
        }
    ));
    let mut draft = task();
    if let ResourceContent::Task {
        details,
        status,
        completed,
        ..
    } = &mut draft.content
    {
        *status = TaskStatus::Done;
        *completed = true;
        details.completed_at = Some("1".into());
    }
    let record = applied(
        mutate(
            &mut conn,
            ProfileId::from(1),
            command("complete-time", ResourceIntent::Create { draft }),
        )
        .unwrap(),
    );
    let ResourceContent::Task { details, .. } = &record.draft.content else {
        panic!()
    };
    assert!(
        details
            .completed_at
            .as_ref()
            .unwrap()
            .parse::<i64>()
            .unwrap()
            > 1
    );
    let mut draft = record.draft.clone();
    if let ResourceContent::Task {
        status, completed, ..
    } = &mut draft.content
    {
        *status = TaskStatus::Open;
        *completed = false;
    }
    let reopened = applied(
        mutate(
            &mut conn,
            ProfileId::from(1),
            command(
                "reopen-time",
                ResourceIntent::Replace {
                    id: record.id,
                    expected_revision: record.revision,
                    draft,
                },
            ),
        )
        .unwrap(),
    );
    let ResourceContent::Task { details, .. } = reopened.draft.content else {
        panic!()
    };
    assert!(details.completed_at.is_none());
}

#[test]
fn field_updates_merge_independent_edits_and_refuse_stale_expectations() {
    let dir = tempfile::tempdir().unwrap();
    let mut conn = connection(&dir.path().join("tasks.sqlite"));
    let created = applied(
        mutate(
            &mut conn,
            ProfileId::from(1),
            command("field-create", ResourceIntent::Create { draft: task() }),
        )
        .unwrap(),
    );
    let update = |key: &str, set: Vec<TaskField>, expect: Vec<TaskField>| {
        command(
            key,
            ResourceIntent::UpdateTask {
                id: created.id.clone(),
                set,
                expect,
            },
        )
    };
    // Written against revision 1 by one actor while another already moved on.
    applied(
        mutate(
            &mut conn,
            ProfileId::from(1),
            update(
                "field-priority",
                vec![TaskField::Priority {
                    value: TaskPriority::High,
                }],
                vec![],
            ),
        )
        .unwrap(),
    );
    let record = applied(
        mutate(
            &mut conn,
            ProfileId::from(1),
            update(
                "field-title",
                vec![
                    TaskField::Title {
                        value: "Research the topic".into(),
                    },
                    TaskField::Deadline {
                        date: Some("2028-03-02".into()),
                    },
                    TaskField::Duration { minutes: Some(45) },
                    TaskField::Status {
                        value: TaskStatus::Done,
                    },
                ],
                vec![TaskField::Title {
                    value: "Research a topic".into(),
                }],
            ),
        )
        .unwrap(),
    );
    assert_eq!(record.revision, "3");
    assert_eq!(record.draft.title, "Research the topic");
    let ResourceContent::Task {
        details, completed, ..
    } = &record.draft.content
    else {
        panic!()
    };
    assert_eq!(details.priority, TaskPriority::High);
    assert_eq!(details.deadline.as_deref(), Some("2028-03-02"));
    assert_eq!(details.duration, Some(45));
    assert!(*completed);
    assert!(details.completed_at.is_some());

    let stale = mutate(
        &mut conn,
        ProfileId::from(1),
        update(
            "field-stale",
            vec![TaskField::Title {
                value: "Overwrite".into(),
            }],
            vec![TaskField::Title {
                value: "Research a topic".into(),
            }],
        ),
    )
    .unwrap();
    assert!(matches!(
        stale,
        ResourceResponse::Error {
            error: ResourceError::Conflict
        }
    ));
    let invalid = mutate(
        &mut conn,
        ProfileId::from(1),
        update(
            "field-invalid",
            vec![TaskField::Schedule {
                date: None,
                time: Some("09:00".into()),
            }],
            vec![],
        ),
    )
    .unwrap();
    assert!(matches!(
        invalid,
        ResourceResponse::Error {
            error: ResourceError::Invalid
        }
    ));
    let duplicate = ResourceCall::Mutate {
        command: Box::new(update(
            "field-twice",
            vec![
                TaskField::Pinned { value: true },
                TaskField::Pinned { value: false },
            ],
            vec![],
        )),
    };
    assert!(!duplicate.validate());
}

#[test]
fn a_deadline_places_a_task_by_whichever_day_comes_first() {
    let dir = tempfile::tempdir().unwrap();
    let mut conn = connection(&dir.path().join("tasks.sqlite"));
    let mut create = |key: &str, due: Option<&str>, deadline: Option<&str>| {
        let mut draft = task();
        if let ResourceContent::Task {
            due_date, details, ..
        } = &mut draft.content
        {
            *due_date = due.map(Into::into);
            details.deadline = deadline.map(Into::into);
        }
        applied(
            mutate(
                &mut conn,
                ProfileId::from(1),
                command(key, ResourceIntent::Create { draft }),
            )
            .unwrap(),
        )
        .id
    };
    let missed = create("deadline-missed", Some("2026-09-30"), Some("2026-09-21"));
    let owed = create("deadline-today", None, Some("2026-09-22"));
    let planned = create("deadline-later", Some("2026-09-22"), Some("2026-10-01"));
    create("deadline-future", Some("2026-09-25"), Some("2026-09-28"));
    create("deadline-none", None, None);
    let query = TaskQuery {
        list: None,
        view: TaskView::Today,
        today: "2026-09-22".into(),
        search: String::new(),
        after: None,
        limit: 10,
    };
    let ResourceResponse::TaskPage {
        items,
        counts,
        metadata,
        ..
    } = task_query::list(&conn, query).unwrap()
    else {
        panic!()
    };
    let ids: Vec<_> = items.iter().map(|item| item.id.clone()).collect();
    assert_eq!(ids[0], missed);
    assert_eq!(ids.len(), 3);
    assert!(ids.contains(&owed) && ids.contains(&planned));
    assert_eq!(metadata[0].deadline.as_deref(), Some("2026-09-21"));
    assert_eq!((counts.today, counts.overdue, counts.upcoming), (3, 1, 1));
    let ResourceResponse::TaskOverview {
        counts: overview, ..
    } = task_query::overview(&conn, "2026-09-22").unwrap()
    else {
        panic!()
    };
    assert_eq!((overview.today, overview.all), (3, 5));
}

#[test]
fn a_new_run_keeps_only_the_newest_receipts() {
    let dir = tempfile::tempdir().unwrap();
    let profile = ProfileId::from(43);
    let open = || {
        let mut hub = Hub::open(dir.path().to_path_buf()).unwrap();
        hub.meta
            .execute(
                "INSERT OR IGNORE INTO profiles(id, name, kind, position) VALUES (?1, 'Fixture', 'named', 0)",
                [profile.to_string()],
            )
            .unwrap();
        hub.load_registry().unwrap();
        hub
    };
    let mutate = |hub: &mut Hub, command: ResourceCommand| {
        applied(hub.resource_call(
            profile,
            ResourceCall::Mutate {
                command: Box::new(command),
            },
        ))
    };
    let mut hub = open();
    let first = mutate(
        &mut hub,
        command("first", ResourceIntent::Create { draft: task() }),
    );
    let conn = hub.profile_conn(profile).unwrap();
    for n in 0..(KEPT_RECEIPTS + 4) {
        conn.execute(
            "INSERT INTO user_resource_receipts(request_id,digest,resource_id,revision,retained) VALUES(?1,zeroblob(32),?2,1,?3)",
            params![format!("filler-{n:0>12}"), first.id, n % 2],
        )
        .unwrap();
    }
    let last = command("last", ResourceIntent::Create { draft: task() });
    let made = mutate(&mut hub, last.clone());
    drop(hub);

    let mut hub = open();
    let count = |hub: &mut Hub| -> i64 {
        hub.profile_conn(profile)
            .unwrap()
            .query_row("SELECT count(*) FROM user_resource_receipts", [], |r| {
                r.get(0)
            })
            .unwrap()
    };
    assert_eq!(count(&mut hub), KEPT_RECEIPTS);
    // The newest write is still answered from its receipt, not applied twice.
    assert_eq!(mutate(&mut hub, last).id, made.id);
    // The oldest is gone, so replaying it would make a second task.
    assert_ne!(
        mutate(
            &mut hub,
            command("first", ResourceIntent::Create { draft: task() })
        )
        .id,
        first.id
    );
}
