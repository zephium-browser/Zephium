use crate::work_authoring::{WorkDocumentProjection, WorkDocumentSubmission};
use zephium_core::work::WorkError;
impl crate::Shell {
    pub(crate) fn work_document(&self, submission: WorkDocumentSubmission) {
        let Some(crate::work_authoring::Payload {
            mut request,
            expected_owner,
            pinned_owner,
            owner,
            reply,
            permit,
            notes_checked,
            record_media_delivery,
        }) = submission.take()
        else {
            return;
        };
        let selected = if pinned_owner {
            expected_owner
        } else {
            self.windows.focused().map(|w| w.profile)
        };
        let profile = selected
            .and_then(|profile| self.profiles.get(profile))
            .filter(|p| p.kind != zephium_core::profiles::ProfileKind::Incognito)
            .filter(|p| !self.profile_deletion_quarantines(p.id))
            .map(|p| p.id);
        let Some(profile) = profile else {
            reply.try_send(Err(WorkError::ProfileUnavailable));
            return;
        };
        if expected_owner.is_some_and(|expected| expected != profile) {
            reply.try_send(Err(WorkError::ProfileUnavailable));
            return;
        }
        let _ = owner.set(profile);
        if let Some(delivery) = record_media_delivery {
            let result = if matches!(
                request,
                zephium_core::work::port::WorkRequest::ReadRecordMedia { .. }
            ) && matches!(
                delivery,
                zephium_core::work::port::WorkReply::RecordMedia(_)
            ) && !pinned_owner
            {
                Ok(WorkDocumentProjection {
                    profile,
                    reply: delivery,
                })
            } else {
                Err(WorkError::Invalid)
            };
            reply.try_send(result);
            return;
        }
        // The attestation is made only here, after profile authentication.
        // A caller cannot submit the internal form and bypass live ownership.
        if matches!(
            request,
            zephium_core::work::port::WorkRequest::AcknowledgeOrphan { .. }
        ) {
            reply.try_send(Err(WorkError::Invalid));
            return;
        }
        #[cfg(feature = "work-runtime")]
        if let zephium_core::work::port::WorkRequest::RuntimeCommand {
            id,
            expected,
            command,
            intent:
                zephium_core::work::runtime::WorkRuntimeIntent::AcknowledgeInterruption { execution },
        } = &request
        {
            if !crate::work_runtime::has_execution_driver(profile, *id, *execution) {
                request = zephium_core::work::port::WorkRequest::AcknowledgeOrphan {
                    id: *id,
                    expected: *expected,
                    command: *command,
                    execution: *execution,
                };
            }
        }
        if let zephium_core::work::port::WorkRequest::Environment {
            call,
            space_available,
            browser_available,
            note_available,
        } = &mut request
        {
            // Only this actor's own notes lookup may attest a note.
            *note_available &= notes_checked;
            use zephium_core::work::environment::{
                WorkEnvironmentCall, WorkEnvironmentEdit, WorkEnvironmentIntent,
                WorkEnvironmentReference,
            };
            *space_available = call.space().is_none_or(|id| {
                self.spaces
                    .get(id)
                    .is_some_and(|space| space.profile == profile)
            });
            *browser_available = match call {
                WorkEnvironmentCall::Command {
                    intent:
                        WorkEnvironmentIntent::Edit {
                            edit:
                                WorkEnvironmentEdit::Add {
                                    reference: WorkEnvironmentReference::Browser { tab },
                                    ..
                                },
                            ..
                        },
                    ..
                } => self.items.get(*tab).is_some_and(|item| {
                    let (zephium_core::item::Placement::Favorites { space }
                    | zephium_core::item::Placement::Space { space, .. }) = item.placement;
                    item.tab().is_some()
                        && self
                            .spaces
                            .get(space)
                            .is_some_and(|space| space.profile == profile)
                }),
                _ => false,
            };
        }
        if let Some(note) = attested_note(&request).filter(|_| !notes_checked) {
            if let (Some(notes), Some(queue)) = (&self.notes, &self.self_queue) {
                let queue = queue.clone();
                let mut payload = crate::work_authoring::Payload {
                    owner,
                    request,
                    expected_owner: Some(profile),
                    pinned_owner: true,
                    reply,
                    permit,
                    notes_checked: true,
                    record_media_delivery: None,
                };
                notes.call(
                    profile,
                    zephium_core::notes::NoteCall::Get { id: note },
                    Box::new(move |response| {
                        let live = matches!(
                            &response,
                            zephium_core::notes::NoteResponse::Record { record }
                                if !record.summary.trashed
                        );
                        if let zephium_core::work::port::WorkRequest::Environment {
                            note_available,
                            ..
                        } = &mut payload.request
                        {
                            *note_available = live;
                        }
                        let command = crate::Command::WorkDocument(
                            crate::work_authoring::WorkDocumentSubmission::resume(payload),
                        );
                        if let Err(
                            crate::actor::TryPushError::Full(command)
                            | crate::actor::TryPushError::Sealed(command)
                            | crate::actor::TryPushError::Closed(command),
                        ) = queue.try_push(command)
                        {
                            if let crate::Command::WorkDocument(submission) = command {
                                if let Some(payload) = submission.take() {
                                    payload.reply.try_send(Err(WorkError::Unavailable));
                                }
                            }
                        }
                    }),
                );
                return;
            }
        }
        let refused = reply.clone();
        use zephium_core::work::port::{WorkReply, WorkRequest};
        let changes = !matches!(
            &request,
            WorkRequest::Read { .. }
                | WorkRequest::Environment {
                    call: zephium_core::work::environment::WorkEnvironmentCall::Read { .. }
                        | zephium_core::work::environment::WorkEnvironmentCall::List { .. },
                    ..
                }
                | WorkRequest::List { .. }
                | WorkRequest::ListPlans { .. }
                | WorkRequest::ReadPlan { .. }
                | WorkRequest::RuntimeRead { .. }
                | WorkRequest::ReadEvidence { .. }
                | WorkRequest::ReadMediaContext { .. }
                | WorkRequest::ReadRecordMedia { .. }
                | WorkRequest::PutRecordMedia { .. }
        );
        let queue = self.self_queue.clone();
        let media_delivery =
            matches!(&request, WorkRequest::ReadRecordMedia { .. }).then(|| request.clone());
        let result = self.store.work_document(
            profile,
            request,
            Box::new(move |result| {
                #[cfg(feature = "work-runtime")]
                let mut result = result;
                #[cfg(feature = "work-runtime")]
                if let Ok(
                    WorkReply::Runtime(projection)
                    | WorkReply::RuntimeCommand { projection, .. }
                    | WorkReply::RuntimeStarted { projection, .. }
                    | WorkReply::PublicReadAdmitted { projection, .. }
                    | WorkReply::AgentAdmitted { projection, .. },
                ) = &mut result
                {
                    crate::work_runtime::project_driver_liveness(profile, projection);
                    crate::work_runtime::revoke_published(profile, projection);
                }
                if changes {
                    if let Ok(WorkReply::Environment(
                        zephium_core::work::environment::WorkEnvironmentReply::Snapshot {
                            snapshot,
                        }
                        | zephium_core::work::environment::WorkEnvironmentReply::Applied {
                            snapshot,
                            ..
                        }
                        | zephium_core::work::environment::WorkEnvironmentReply::Checkpointed {
                            snapshot,
                            ..
                        },
                    )) = &result
                    {
                        if let Some(queue) = &queue {
                            let _ = queue.try_push(crate::Command::WorkEnvironmentChanged(
                                zephium_ipc::work::WorkEnvironmentChangedV1 {
                                    profile: profile.to_string(),
                                    environment: snapshot.id,
                                },
                            ));
                        }
                    }
                    let work = match &result {
                        Ok(WorkReply::Snapshot(snapshot)) => Some(snapshot.id),
                        Ok(
                            WorkReply::Runtime(state)
                            | WorkReply::RuntimeCommand {
                                projection: state, ..
                            }
                            | WorkReply::PublicReadAdmitted {
                                projection: state, ..
                            }
                            | WorkReply::RuntimeStarted {
                                projection: state, ..
                            },
                        ) => Some(state.work.id),
                        Ok(WorkReply::AuthoringCommand(receipt)) => Some(receipt.work),
                        Ok(WorkReply::Deleted { id }) => Some(*id),
                        _ => None,
                    };
                    #[cfg(feature = "work-runtime")]
                    {
                        let admitted = match &result {
                            Ok(WorkReply::AgentAdmitted { projection, .. }) => {
                                Some(projection.work.id)
                            }
                            _ => None,
                        };
                        if let Some(changed) = work.or(admitted) {
                            crate::work_changes::publish(changed);
                        }
                    }
                    if let (Some(queue), Some(work)) = (&queue, work) {
                        let _ = queue.try_push(crate::Command::WorkChanged(
                            zephium_ipc::work::WorkChangedV1 {
                                profile: profile.to_string(),
                                work,
                            },
                        ));
                    }
                }
                if let Some(request) = media_delivery {
                    if matches!(&result, Ok(WorkReply::RecordMedia(_))) {
                        // Bytes stay private until this completion passes through
                        // the actor again. A profile switch in the Store wait is
                        // checked by the same focused-profile admission above.
                        let completion =
                            WorkDocumentSubmission::resume(crate::work_authoring::Payload {
                                owner,
                                request,
                                expected_owner: Some(profile),
                                pinned_owner: false,
                                reply: reply.clone(),
                                permit,
                                notes_checked: true,
                                record_media_delivery: result.ok(),
                            });
                        if queue.as_ref().is_none_or(|queue| {
                            queue
                                .try_push(crate::Command::WorkDocument(completion.clone()))
                                .is_err()
                        }) {
                            reply.try_send(Err(WorkError::Unavailable));
                        }
                        return;
                    }
                }
                let _permit = permit;
                reply.try_send(result.map(|reply| WorkDocumentProjection { profile, reply }));
            }),
        );
        if let Err(error) = result {
            refused.try_send(Err(error));
        }
    }
}

/// The resource an environment edit adds, which may be a note file.
fn attested_note(request: &zephium_core::work::port::WorkRequest) -> Option<String> {
    use zephium_core::work::environment::{
        WorkEnvironmentCall, WorkEnvironmentEdit, WorkEnvironmentIntent, WorkEnvironmentReference,
    };
    match request {
        zephium_core::work::port::WorkRequest::Environment {
            call:
                WorkEnvironmentCall::Command {
                    intent:
                        WorkEnvironmentIntent::Edit {
                            edit:
                                WorkEnvironmentEdit::Add {
                                    reference: WorkEnvironmentReference::Resource { resource },
                                    ..
                                },
                            ..
                        },
                    ..
                },
            note_available: false,
            ..
        } => Some(resource.to_string()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Command, Shell, WorkIntent, WorkUserEdit};
    use std::{
        sync::Arc,
        time::{Duration, Instant},
    };
    use zephium_core::work::{port::*, *};

    #[test]
    fn queued_record_media_revalidates_active_profile_before_bytes_leave_actor() {
        use zephium_core::work::media::{WorkRecordMediaOriginV1, WorkRecordMediaRefV1};
        let store = Arc::new(zephium_store::SqliteStore::in_memory().unwrap());
        let mut shell = Shell::new(
            Arc::new(crate::shell::tests::FakeEngine::default()),
            store.clone(),
            Arc::new(crate::shell::tests::FakeChrome),
            Box::new(|_| {}),
        );
        shell.handle(Command::Bootstrap);
        assert!(store.flush());
        let profile = shell.windows.focused().unwrap().profile;
        let other = zephium_core::ids::ProfileId::generate();
        assert!(shell.profiles.insert(zephium_core::profiles::Profile {
            id: other,
            name: "Other regular profile".into(),
            kind: zephium_core::profiles::ProfileKind::Named,
        }));
        for switched in [false, true] {
            shell.windows.focused_mut().unwrap().profile = profile;
            let pending = crate::work_authoring::Pending::default();
            let media = WorkRecordMediaRefV1 {
                profile,
                work: 1.into(),
                execution: 2.into(),
                attempt: 3.into(),
                step: 4.into(),
                extraction: 5.into(),
                record: 0,
                source: "a".repeat(64),
                digest: "b".repeat(64),
                width: 1,
                height: 1,
                origin: WorkRecordMediaOriginV1::NativeCrop,
            };
            let (submission, receipt) = WorkDocumentSubmission::prepare_bound(
                &pending,
                WorkRequest::ReadRecordMedia { media },
                Some(profile),
            )
            .unwrap();
            let mut payload = submission.take().unwrap();
            // The Store callback has finished, but the queued actor completion
            // still owns both private bytes and the original pending permit.
            payload.record_media_delivery =
                Some(WorkReply::RecordMedia(WorkMediaContext(vec![1, 2, 3])));
            let command = Command::WorkDocument(WorkDocumentSubmission::resume(payload));
            assert!(receipt.try_recv().is_none());
            if switched {
                shell.windows.focused_mut().unwrap().profile = other;
            }
            shell.handle(command.clone());
            shell.handle(command);
            let result = receipt.try_recv().expect("one actor completion");
            if switched {
                assert!(matches!(result, Err(WorkError::ProfileUnavailable)));
            } else {
                assert!(matches!(
                    result,
                    Ok(WorkDocumentProjection {
                        reply: WorkReply::RecordMedia(_),
                        ..
                    })
                ));
            }
            assert!(receipt.try_recv().is_none());
            assert_eq!(pending.load(std::sync::atomic::Ordering::Acquire), 0);
        }
        assert_eq!(
            store.shutdown_until(Instant::now() + Duration::from_secs(5)),
            zephium_core::ports::store::StoreShutdownOutcome::Clean
        );
    }

    #[test]
    fn work_document_product_intent_uses_selected_profile_and_real_store_without_execution() {
        let store = Arc::new(zephium_store::SqliteStore::in_memory().unwrap());
        let mut shell = Shell::new(
            Arc::new(crate::shell::tests::FakeEngine::default()),
            store.clone(),
            Arc::new(crate::shell::tests::FakeChrome),
            Box::new(|_| {}),
        );
        shell.handle(Command::Bootstrap);
        assert!(store.flush());
        let profile = shell.windows.focused().unwrap().profile;
        let queue = crate::actor::CommandQueue::new();
        let handle = crate::actor::Handle::new(queue.clone());
        let request = handle
            .work_document(WorkIntent::Create {
                objective: "Compare local model deployment options".into(),
            })
            .unwrap();
        let minted_id = request.work_id().unwrap();
        let command = queue.try_recv().unwrap();
        let duplicate = command.clone();
        shell.handle(command);
        shell.handle(duplicate); // cloned Command owns only the same one-shot payload
        let projection = wait(&request).unwrap();
        assert!(
            request.try_recv().is_none(),
            "a known outcome cannot become uncertain later"
        );
        assert_eq!(projection.profile, profile);
        assert_eq!(request.profile(), Some(profile));
        let WorkReply::Snapshot(work) = projection.reply else {
            panic!()
        };
        let id = work.id;
        assert_eq!(id, minted_id);
        assert_eq!(work.objective_author, WorkAuthor::User);
        assert_eq!(work.status, WorkAuthoringStatus::Draft);
        assert!(work.plan.is_none());
        let request = handle
            .work_document(WorkIntent::Edit {
                id,
                expected: work.revision,
                edit: WorkUserEdit::OpenQuestion {
                    prompt: "Which hardware?".into(),
                    options: vec![],
                },
            })
            .unwrap();
        shell.handle(queue.try_recv().unwrap());
        let WorkReply::Snapshot(questioned) = wait(&request).unwrap().reply else {
            panic!()
        };
        assert_eq!(questioned.status, WorkAuthoringStatus::NeedsInput);
        // Private selection is refused before the Store adapter sees content.
        let mut private = shell.profiles.remove(profile).unwrap();
        private.kind = zephium_core::profiles::ProfileKind::Incognito;
        assert!(shell.profiles.insert(private));
        let request = handle.work_document(WorkIntent::Read { id }).unwrap();
        shell.handle(queue.try_recv().unwrap());
        assert!(matches!(wait(&request), Err(WorkError::ProfileUnavailable)));
        assert_eq!(
            store.shutdown_until(Instant::now() + Duration::from_secs(5)),
            zephium_core::ports::store::StoreShutdownOutcome::Clean
        );
    }
    #[test]
    fn environment_attachment_uses_native_ownership_and_replays_after_tab_retirement() {
        use zephium_core::work::environment::*;
        let store = Arc::new(zephium_store::SqliteStore::in_memory().unwrap());
        let mut shell = Shell::new(
            Arc::new(crate::shell::tests::FakeEngine::default()),
            store.clone(),
            Arc::new(crate::shell::tests::FakeChrome),
            Box::new(|_| {}),
        );
        shell.handle(Command::Bootstrap);
        assert!(store.flush());
        let window = shell.windows.focused().unwrap();
        let profile = window.profile;
        let space = window.space;
        let tab = window.active.unwrap();
        let submit = |shell: &Shell, call| {
            // Deliberately forged flags: the actor must replace them from its
            // actual selected profile, Space and tab ownership.
            let (submission, request) = WorkDocumentSubmission::prepare_bound(
                &crate::work_authoring::Pending::default(),
                WorkRequest::Environment {
                    call,
                    space_available: true,
                    browser_available: true,
                    note_available: true,
                },
                Some(profile),
            )
            .unwrap();
            shell.work_document(submission);
            wait(&request).map(|value| value.reply)
        };
        assert!(matches!(
            submit(
                &shell,
                WorkEnvironmentCall::Command {
                    command: WorkCommandId::generate(),
                    intent: WorkEnvironmentIntent::Create {
                        space: 999.into(),
                        title: "Wrong Space".into()
                    },
                }
            ),
            Err(WorkError::NotFound)
        ));
        let WorkReply::Environment(WorkEnvironmentReply::Applied { snapshot, .. }) = submit(
            &shell,
            WorkEnvironmentCall::Command {
                command: WorkCommandId::generate(),
                intent: WorkEnvironmentIntent::Create {
                    space,
                    title: "Native Work".into(),
                },
            },
        )
        .unwrap() else {
            panic!("environment")
        };
        let foreign = zephium_core::ids::ItemId::from(998_u128);
        assert!(shell.items.insert_tab(
            foreign,
            zephium_core::item::Placement::Favorites { space: 999.into() }
        ));
        let attach = |tab, command| WorkEnvironmentCall::Command {
            command,
            intent: WorkEnvironmentIntent::Edit {
                id: snapshot.id,
                expected: snapshot.revision,
                edit: WorkEnvironmentEdit::Add {
                    reference: WorkEnvironmentReference::Browser { tab },
                    area: None,
                },
            },
        };
        assert!(matches!(
            submit(&shell, attach(foreign, WorkCommandId::generate())),
            Err(WorkError::NotFound)
        ));
        let original = attach(tab, WorkCommandId::generate());
        assert!(matches!(
            submit(&shell, original.clone()).unwrap(),
            WorkReply::Environment(WorkEnvironmentReply::Applied {
                replayed: false,
                ..
            })
        ));
        let _ = shell.items.remove(tab);
        // The original durable receipt survives retirement. It neither creates
        // a replacement tab nor requires fresh resource authority.
        assert!(matches!(
            submit(&shell, original).unwrap(),
            WorkReply::Environment(WorkEnvironmentReply::Applied { replayed: true, .. })
        ));
        assert!(shell.items.get(tab).is_none());
        assert_eq!(
            store.shutdown_until(Instant::now() + Duration::from_secs(5)),
            zephium_core::ports::store::StoreShutdownOutcome::Clean
        );
    }
    fn wait(request: &crate::WorkDocumentRequest) -> Result<WorkDocumentProjection, WorkError> {
        // A hang guard, not a speed bound: a busy parallel suite starves the store thread.
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            if let Some(reply) = request.try_recv() {
                return reply;
            }
            assert!(Instant::now() < deadline, "Work Store callback deadline");
            std::thread::sleep(Duration::from_millis(1));
        }
    }
}
