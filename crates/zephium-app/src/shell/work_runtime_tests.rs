use super::work_planning_tests::{drive, fixture};
use crate::{work_runtime::*, WorkIntent, WorkUserEdit};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
use zephium_core::work::{artifact::*, port::*, proposal::*, runtime::*, *};
use zephium_ipc::work::WorkCommandV1;

pub(super) struct SynthesisFixture(
    pub(super) u8,
    pub(super) Option<(crate::Handle, zephium_core::ids::ProfileId, WorkCommandV1)>,
);
impl zephium_core::work::synthesis::WorkSynthesisProvider for SynthesisFixture {
    fn produce<'a>(
        &'a self,
        input: &'a zephium_core::work::synthesis::WorkSynthesisDisclosure,
    ) -> zephium_core::work::synthesis::WorkSynthesisFuture<'a> {
        use zephium_core::work::synthesis::*;
        Box::pin(async move {
            assert!(input.context().sources.is_empty());
            assert_eq!(input.context().outputs[0].name, "checklist");
            if self.0 == 2 {
                return Err(WorkSynthesisError::OutcomeUnknown);
            }
            if self.0 == 3 {
                return Err(WorkSynthesisError::NotDispatched(WorkError::Capacity));
            }
            if let Some((handle, profile, command)) = &self.1 {
                // The model future stays pending after durable cancellation;
                // only the adapter's original cancellation loop can finish it.
                handle
                    .work_command(*profile, command.clone())
                    .unwrap()
                    .await
                    .unwrap();
                return std::future::pending().await;
            }
            Ok(WorkSynthesisResult {
                outputs: vec![WorkSynthesisOutput {
                    output: if self.0 == 1 { 1 } else { 0 },
                    title: "Suggested tasks".into(),
                    data: WorkArtifactDataV1::Checklist {
                        items: vec![WorkChecklistItem {
                            text: "Review changes".into(),
                            completed: false,
                        }],
                    },
                    evidence: vec![],
                }],
                usage: WorkUsage {
                    model_tokens: if self.0 == 4 { 9000 } else { 300 },
                    cost_micro_usd: 50,
                    operations: 1,
                    accounting: WorkUsageAccounting::ConservativeReservation,
                },
            })
        })
    }
}

#[tokio::test]
async fn owned_evidence_reads_reach_store_without_bypassing_profile_or_link_checks() {
    let store = Arc::new(zephium_store::SqliteStore::in_memory().unwrap());
    let (mut shell, queue, handle, profile) = fixture(store.clone());
    let create = handle
        .work_document(WorkIntent::Create {
            objective: "Review worker evidence".into(),
        })
        .unwrap();
    let work = create.work_id().unwrap();
    drive(&mut shell, &queue, create).await.unwrap();
    let evidence = WorkRequest::ReadEvidence {
        id: work,
        link: WorkEvidenceLink {
            extraction_id: WorkArtifactId::generate(),
            source_id: 1,
        },
    };
    let request = handle
        .submit_owned_work_runtime(evidence.clone(), profile)
        .expect("original runtime can request historical evidence from its pinned profile");
    assert!(
        matches!(
            drive(&mut shell, &queue, request).await,
            Err(WorkError::NotFound)
        ),
        "Store must still require an artifact-linked source in this exact Work"
    );

    // Pinned reads remain subject to current profile eligibility. Their owner
    // selection cannot turn a private profile into a model disclosure source.
    let mut private = shell.profiles.remove(profile).unwrap();
    private.kind = zephium_core::profiles::ProfileKind::Incognito;
    assert!(shell.profiles.insert(private));
    let request = handle.submit_owned_work_runtime(evidence, profile).unwrap();
    assert!(matches!(
        drive(&mut shell, &queue, request).await,
        Err(WorkError::ProfileUnavailable)
    ));
    assert!(
        matches!(
            handle.submit_owned_work_runtime(
                WorkRequest::RuntimeCommand {
                    id: work,
                    expected: WorkRevision::INITIAL,
                    command: WorkCommandId::generate(),
                    intent: WorkRuntimeIntent::Cancel {
                        execution: WorkExecutionId::generate(),
                        intervention: None,
                    },
                },
                profile
            ),
            Err(WorkError::Invalid)
        ),
        "historical reads do not admit user intents through the owned runtime lane"
    );
    assert_eq!(
        store.shutdown_until(Instant::now() + Duration::from_secs(5)),
        zephium_core::ports::store::StoreShutdownOutcome::Clean
    );
}

#[tokio::test]
async fn synthesis_adapter_persists_review_artifacts_and_honest_failed_or_unknown_usage() {
    for mode in 0..6 {
        synthesis_case(mode, None).await;
    }
}

async fn synthesis_case(
    mode: u8,
    external: Option<&dyn zephium_core::work::synthesis::WorkSynthesisProvider>,
) -> WorkRuntimeProjection {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(zephium_store::SqliteStore::open(dir.path()).unwrap());
    let (mut shell, queue, handle, profile) = fixture(store.clone());
    let create = handle
        .work_document(WorkIntent::Create {
            objective: "Prepare a release checklist".into(),
        })
        .unwrap();
    let work = create.work_id().unwrap();
    drive(&mut shell, &queue, create).await.unwrap();
    let edit = handle
        .work_document(WorkIntent::Edit {
            id: work,
            expected: WorkRevision::INITIAL,
            edit: WorkUserEdit::ReplaceDraft {
                proposal: WorkPlanProposal {
                    nodes: vec![WorkNodeProposal {
                        key: 0,
                        objective: if mode == 6 { "Suggest at least three actionable software release checks. Produce a checklist with all items initially incomplete.".into() } else { "Suggest release checks".into() },
                        dependencies: vec![],
                        outputs: vec![WorkExpectedOutput {
                            name: "checklist".into(),
                            description: "Suggested checks for user review".into(),
                            review: WorkOutputReview::UserAcceptance,
                        }],
                    }],
                },
            },
        })
        .unwrap();
    let WorkReply::Snapshot(planned) = drive(&mut shell, &queue, edit).await.unwrap().reply else {
        panic!()
    };
    let plan = planned.plan.as_ref().unwrap();
    let node = plan.draft.nodes[0].id;
    let limits = WorkExecutionLimits {
        model_tokens: 8000,
        cost_micro_usd: 10000,
        operations: 2,
        timeout_seconds: 60,
        max_workers: 1,
    };
    let approve = handle
        .work_command(
            profile,
            WorkCommandV1 {
                version: 1,
                work,
                expected_revision: planned.revision,
                command: WorkCommandId::generate(),
                intent: WorkRuntimeIntent::Approve {
                    spec: WorkExecutionSpec {
                        request: None,
                        context: None,
                        plan_revision: plan.revision,
                        limits,
                        nodes: vec![WorkNodeExecutionSpec {
                            node,
                            parent: None,
                            capability: WorkCapability::Synthesize,
                            limits,
                        }],
                    },
                },
            },
        )
        .unwrap();
    let WorkReply::RuntimeCommand {
        projection,
        receipt,
    } = drive(&mut shell, &queue, approve).await.unwrap().reply
    else {
        panic!()
    };
    let runtime = WorkRuntimeService::new(handle.clone());
    let attempt = drive(
        &mut shell,
        &queue,
        runtime.begin_node(
            profile,
            work,
            projection.work.revision,
            receipt.execution,
            node,
        ),
    )
    .await
    .unwrap();
    let observer = attempt.observer();
    let model = SynthesisFixture(
        mode,
        (mode == 5).then(|| {
            (
                handle.clone(),
                profile,
                WorkCommandV1 {
                    version: 1,
                    work,
                    expected_revision: projection.work.revision.next().unwrap(),
                    command: WorkCommandId::generate(),
                    intent: WorkRuntimeIntent::Cancel {
                        execution: receipt.execution,
                        intervention: None,
                    },
                },
            )
        }),
    );
    let state = drive(
        &mut shell,
        &queue,
        attempt.synthesize_owned(external.unwrap_or(&model)),
    )
    .await
    .unwrap();
    assert_eq!(state.profile(), profile);
    assert_eq!(state.work(), work);
    assert_eq!(state.node(), node);
    assert_eq!(state.execution(), receipt.execution);
    let state = state.into_projection();
    if mode == 6 {
        let report = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/work-runtime-proof/synthesis.json");
        std::fs::create_dir_all(report.parent().unwrap()).unwrap();
        std::fs::write(report, serde_json::to_vec_pretty(&state).unwrap()).unwrap();
    }
    assert!(observer.latest().is_none());
    let execution = &state.executions[0];
    let attempt = &execution.attempts[0];
    match mode {
        0 | 6 => {
            assert_eq!(execution.status, WorkExecutionStatus::NeedsReview);
            assert_eq!(execution.artifacts.len(), 1);
            assert_eq!(
                execution.artifacts[0].review,
                WorkOutputReview::UserAcceptance
            );
            assert_eq!(attempt.status, WorkAttemptStatus::Succeeded);
            if mode == 0 {
                assert_eq!(attempt.usage.unwrap().model_tokens, 300);
            } else {
                assert!(attempt.usage.unwrap().model_tokens > 0);
                assert!(
                    matches!(&execution.artifacts[0].data, WorkArtifactDataV1::Checklist { items } if items.len() >= 3 && items.iter().all(|item| !item.completed)),
                    "Live artifact did not fulfill the objective: {}",
                    serde_json::to_string(&execution.artifacts[0].data).unwrap()
                );
            }
        }
        1 => {
            assert_eq!(execution.status, WorkExecutionStatus::Failed);
            assert!(execution.artifacts.is_empty());
            assert_eq!(attempt.usage.unwrap().model_tokens, 300);
        }
        2 | 4 | 5 => {
            assert_eq!(
                execution.status,
                if mode == 5 {
                    WorkExecutionStatus::Cancelled
                } else {
                    WorkExecutionStatus::Interrupted
                }
            );
            assert_eq!(attempt.status, WorkAttemptStatus::OutcomeUnknown);
            assert!(attempt.usage.is_none());
            assert!(execution.artifacts.is_empty());
        }
        3 => {
            assert_eq!(execution.status, WorkExecutionStatus::Failed);
            assert_eq!(attempt.usage, Some(WorkUsage::default()));
        }
        _ => unreachable!(),
    }
    let WorkReply::Runtime(read) = drive(
        &mut shell,
        &queue,
        handle.work_projection(profile, work).unwrap(),
    )
    .await
    .unwrap()
    .reply
    else {
        panic!()
    };
    assert_eq!(state, *read);
    assert!(store.flush());
    drop(model);
    drop(runtime);
    drop(handle);
    drop(shell);
    assert_eq!(
        store.shutdown_until(Instant::now() + Duration::from_secs(5)),
        zephium_core::ports::store::StoreShutdownOutcome::Clean
    );
    drop(store);
    let store = Arc::new(zephium_store::SqliteStore::open(dir.path()).unwrap());
    let (mut shell, queue, handle, _) = fixture(store.clone());
    let WorkReply::Runtime(reopened) = drive(
        &mut shell,
        &queue,
        handle.work_projection(profile, work).unwrap(),
    )
    .await
    .unwrap()
    .reply
    else {
        panic!()
    };
    assert_eq!(*reopened, state);
    drop(handle);
    drop(shell);
    assert_eq!(
        store.shutdown_until(Instant::now() + Duration::from_secs(5)),
        zephium_core::ports::store::StoreShutdownOutcome::Clean
    );
    state
}

#[cfg(target_os = "macos")]
#[tokio::test]
#[ignore = "explicit development OpenAI credential and at most $0.01 generation"]
async fn live_public_synthesis_publishes_review_artifact_and_reopens_store() {
    use zephium_agentic::{
        AgentProviderTransport, AgentProviderTransportConfig, OpenAiWorkSynthesizer,
        WorkPlanningConfig,
    };
    // Load the development key and networking libraries before the approved
    // attempt deadline starts. Nothing secret enters the Work or report.
    let credential = zephium_agentic::load_macos_development_openai_credential().unwrap();
    let transport =
        AgentProviderTransport::try_new(AgentProviderTransportConfig::STANDARD).unwrap();
    let model = OpenAiWorkSynthesizer::try_new(
        transport,
        credential,
        WorkPlanningConfig::try_new(
            zephium_agent_model_catalog::try_luna_provider_exact_call_config(2048).unwrap(),
            8192,
            10_000,
        )
        .unwrap(),
    )
    .unwrap();
    #[cfg(feature = "work-synthesis-probe")]
    let model = model.with_public_response_retention();
    let result = synthesis_case(6, Some(&model)).await;
    let report = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/work-runtime-proof/synthesis.json");
    std::fs::create_dir_all(report.parent().unwrap()).unwrap();
    std::fs::write(report, serde_json::to_vec_pretty(&result).unwrap()).unwrap();
}

#[tokio::test]
async fn runtime_application_approves_exact_plan_settles_artifact_and_recovers_dropped_attempt() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(zephium_store::SqliteStore::open(dir.path()).unwrap());
    let (mut shell, queue, handle, profile) = fixture(store.clone());
    let create = handle
        .work_document(WorkIntent::Create {
            objective: "Produce a local checklist".into(),
        })
        .unwrap();
    let work = create.work_id().unwrap();
    drive(&mut shell, &queue, create).await.unwrap();
    let edit = handle
        .work_document(WorkIntent::Edit {
            id: work,
            expected: WorkRevision::INITIAL,
            edit: WorkUserEdit::ReplaceDraft {
                proposal: WorkPlanProposal {
                    nodes: vec![
                        WorkNodeProposal {
                            key: 0,
                            objective: "Build the requested checklist".into(),
                            dependencies: vec![],
                            outputs: vec![WorkExpectedOutput {
                                name: "checklist".into(),
                                description: "The requested local task list".into(),
                                review: WorkOutputReview::Mechanical,
                            }],
                        },
                        WorkNodeProposal {
                            key: 1,
                            objective: "Consume the selected checklist result".into(),
                            dependencies: vec![0],
                            outputs: vec![WorkExpectedOutput {
                                name: "summary".into(),
                                description: "A derived local checklist".into(),
                                review: WorkOutputReview::Mechanical,
                            }],
                        },
                    ],
                },
            },
        })
        .unwrap();
    let WorkReply::Snapshot(planned) = drive(&mut shell, &queue, edit).await.unwrap().reply else {
        panic!()
    };
    let plan = planned.plan.as_ref().unwrap();
    let limits = WorkExecutionLimits {
        model_tokens: 8000,
        cost_micro_usd: 10000,
        operations: 32,
        timeout_seconds: 300,
        max_workers: 1,
    };
    let spec = WorkExecutionSpec {
        request: None,
        context: None,
        plan_revision: plan.revision,
        limits,
        nodes: plan
            .draft
            .nodes
            .iter()
            .map(|node| WorkNodeExecutionSpec {
                node: node.id,
                parent: None,
                capability: WorkCapability::Synthesize,
                limits: WorkExecutionLimits {
                    model_tokens: limits.model_tokens / 2,
                    cost_micro_usd: limits.cost_micro_usd / 2,
                    operations: limits.operations / 2,
                    ..limits
                },
            })
            .collect(),
    };
    let approve = handle
        .work_command(
            profile,
            WorkCommandV1 {
                version: 1,
                work,
                expected_revision: planned.revision,
                command: WorkCommandId::generate(),
                intent: WorkRuntimeIntent::Approve { spec: spec.clone() },
            },
        )
        .unwrap();
    let WorkReply::RuntimeCommand {
        projection: approved,
        receipt,
    } = drive(&mut shell, &queue, approve).await.unwrap().reply
    else {
        panic!()
    };
    let service = WorkRuntimeService::new(handle.clone());
    let attempt = drive(
        &mut shell,
        &queue,
        service.begin_node(
            profile,
            work,
            approved.work.revision,
            receipt.execution,
            plan.draft.nodes[0].id,
        ),
    )
    .await
    .unwrap();
    let browser_session = attempt.probe().browser_session().clone();
    assert_eq!(browser_session.id(), attempt.probe().browser_session().id());
    assert!(browser_session.admits(profile, work));
    assert_eq!(attempt.node().objective, plan.draft.nodes[0].objective);
    assert!(attempt.deadline() > Instant::now());
    assert!(attempt.dependency_artifacts().is_empty());
    let observer = attempt.observer();
    assert!(observer.latest().is_none());
    attempt.record_activity(zephium_ipc::work::WorkActivityV1::ProducingArtifact);
    let signal = observer.latest().unwrap();
    assert_eq!(signal.work, work);
    assert_eq!(signal.attempt, attempt.attempt());
    // A focused private surface cannot read the regular Work, but cannot
    // redirect or prevent settlement of that Work's already-owned attempt.
    let private = zephium_core::ids::ProfileId::generate();
    assert!(shell.profiles.insert(zephium_core::profiles::Profile {
        id: private,
        name: "Private fixture".into(),
        kind: zephium_core::profiles::ProfileKind::Incognito
    }));
    shell.windows.focused_mut().unwrap().profile = private;
    assert!(matches!(
        drive(
            &mut shell,
            &queue,
            handle.work_projection(profile, work).unwrap()
        )
        .await,
        Err(WorkError::ProfileUnavailable)
    ));
    let result = WorkAdapterResult {
        status: WorkAttemptStatus::Succeeded,
        intervention: None,
        usage: Some(WorkUsage::default()),
        artifacts: vec![WorkArtifactDraft {
            output: "checklist".into(),
            title: "Tasks".into(),
            data: WorkArtifactDataV1::Checklist {
                items: vec![WorkChecklistItem {
                    text: "Review release notes".into(),
                    completed: false,
                }],
            },
            evidence: vec![],
        }],
    };
    let completed = drive(&mut shell, &queue, attempt.settle(result))
        .await
        .unwrap();
    assert!(
        !browser_session.is_current(),
        "settlement revokes retained probe clones"
    );
    assert_eq!(completed.executions[0].status, WorkExecutionStatus::Running);
    assert_eq!(completed.executions[0].artifacts.len(), 1);
    let mut missing_output = completed.executions[0].clone();
    missing_output.artifacts.clear();
    assert_eq!(
        missing_output.validate(plan, completed.work.revision),
        Err(WorkError::Invalid)
    );
    assert_eq!(completed.work.profile, profile);
    assert!(observer.latest().is_none());
    shell.windows.focused_mut().unwrap().profile = profile;
    shell.profiles.remove(private);
    let successor = drive(
        &mut shell,
        &queue,
        service.begin_node(
            profile,
            work,
            completed.work.revision,
            receipt.execution,
            plan.draft.nodes[1].id,
        ),
    )
    .await
    .unwrap();
    assert_eq!(
        successor.dependency_artifacts(),
        completed.executions[0].artifacts
    );
    let completed = drive(
        &mut shell,
        &queue,
        successor.settle(WorkAdapterResult {
            status: WorkAttemptStatus::Succeeded,
            intervention: None,
            usage: Some(WorkUsage::default()),
            artifacts: vec![WorkArtifactDraft {
                output: "summary".into(),
                title: "Derived result".into(),
                data: WorkArtifactDataV1::Checklist {
                    items: vec![WorkChecklistItem {
                        text: "Review the selected predecessor result".into(),
                        completed: false,
                    }],
                },
                evidence: vec![],
            }],
        }),
    )
    .await
    .unwrap();
    assert_eq!(
        completed.executions[0].status,
        WorkExecutionStatus::Completed
    );
    assert_eq!(completed.executions[0].artifacts.len(), 2);
    let approve = handle
        .work_command(
            profile,
            WorkCommandV1 {
                version: 1,
                work,
                expected_revision: completed.work.revision,
                command: WorkCommandId::generate(),
                intent: WorkRuntimeIntent::Approve { spec },
            },
        )
        .unwrap();
    let WorkReply::RuntimeCommand {
        projection: approved,
        receipt,
    } = drive(&mut shell, &queue, approve).await.unwrap().reply
    else {
        panic!()
    };
    let attempt = drive(
        &mut shell,
        &queue,
        service.begin_node(
            profile,
            work,
            approved.work.revision,
            receipt.execution,
            plan.draft.nodes[0].id,
        ),
    )
    .await
    .unwrap();
    let abandoned_session = attempt.probe().browser_session().clone();
    assert_ne!(abandoned_session.id(), browser_session.id());
    let dropped = attempt.attempt();
    drop(attempt);
    assert!(!abandoned_session.is_current());
    let state = drive(
        &mut shell,
        &queue,
        handle.work_projection(profile, work).unwrap(),
    )
    .await
    .unwrap();
    let WorkReply::Runtime(state) = state.reply else {
        panic!()
    };
    let unknown = state
        .executions
        .iter()
        .find(|e| e.id == receipt.execution)
        .unwrap();
    assert_eq!(unknown.status, WorkExecutionStatus::Interrupted);
    assert_eq!(unknown.attempts[0].id, dropped);
    assert_eq!(unknown.attempts[0].usage, None);
    drop(service);
    drop(handle);
    drop(shell);
    assert_eq!(
        store.shutdown_until(Instant::now() + Duration::from_secs(5)),
        zephium_core::ports::store::StoreShutdownOutcome::Clean
    );
    drop(store);
    let store = Arc::new(zephium_store::SqliteStore::open(dir.path()).unwrap());
    let (mut shell, queue, handle, _) = fixture(store.clone());
    let WorkReply::Runtime(reopened) = drive(
        &mut shell,
        &queue,
        handle.work_projection(profile, work).unwrap(),
    )
    .await
    .unwrap()
    .reply
    else {
        panic!()
    };
    assert_eq!(reopened, state);
    drop(handle);
    drop(shell);
    assert_eq!(
        store.shutdown_until(Instant::now() + Duration::from_secs(5)),
        zephium_core::ports::store::StoreShutdownOutcome::Clean
    );
}

#[path = "work_search_tests.rs"]
mod search_tests;

#[path = "work_parallel_read_tests.rs"]
mod parallel_read_tests;

#[tokio::test]
async fn rejected_final_output_cannot_finish_on_earlier_partial_artifacts() {
    use crate::work_agent::*;
    use zephium_core::work::{agent::*, search::*, synthesis::*};
    struct Turns(std::sync::atomic::AtomicUsize);
    impl WorkAgentTurnProvider for Turns {
        fn turn<'a>(
            &'a self,
            input: &'a WorkAgentTurnDisclosure,
            _: WorkSynthesisTrace,
        ) -> WorkAgentTurnFuture<'a> {
            Box::pin(async move {
                let turn = self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                assert!(turn < 4, "bounded repair attempts");
                if turn > 1 {
                    assert!(input
                        .context()
                        .notices
                        .iter()
                        .any(|notice| notice.starts_with("Finish was refused:")));
                }
                Ok(WorkAgentTurnResult {
                    output: WorkAgentTurnOutput {
                        say: Some("Completed the final comparison".into()),
                        artifacts: if turn == 1 {
                            vec![WorkAgentArtifactOutput {
                                title: "Refused final output".into(),
                                data: WorkArtifactDataV1::Document {
                                    paragraphs: vec!["Unsupported result".into()],
                                    formatted: None,
                                },
                                evidence: vec![],
                                general_knowledge: false,
                            }]
                        } else {
                            vec![]
                        },
                        fetch: if turn == 0 {
                            vec![WorkAgentFetch::Read {
                                url: "https://example.test/catalog".into(),
                                collection: None,
                            }]
                        } else {
                            vec![]
                        },
                        ask: None,
                        finish: turn > 0,
                        followups: vec![],
                        malformed: 0,
                    },
                    usage: WorkUsage::default(),
                })
            })
        }
    }
    struct NoSearch;
    impl WorkPublicSearchProvider for NoSearch {
        fn search<'a>(
            &'a self,
            _: &'a WorkPublicSearchScope,
            _: &'a [zephium_core::work::context::WorkContextBody],
            _: WorkExecutionLimits,
        ) -> WorkPublicSearchFuture<'a> {
            panic!("repair must not dispatch a search");
        }
    }
    let store = Arc::new(zephium_store::SqliteStore::in_memory().unwrap());
    let (mut shell, queue, handle, profile) = fixture(store);
    let create = handle
        .work_document(WorkIntent::Create {
            objective: "Read https://example.test/catalog and compare the results".into(),
        })
        .unwrap();
    let work = create.work_id().unwrap();
    drive(&mut shell, &queue, create).await.unwrap();
    let provider = Turns(std::sync::atomic::AtomicUsize::new(0));
    let service = WorkAgentService::new(handle);
    let result = drive(
        &mut shell,
        &queue,
        service.run(
            profile,
            WorkCommandV1 {
                version: 1,
                work,
                expected_revision: WorkRevision::INITIAL,
                command: WorkCommandId::generate(),
                intent: WorkRuntimeIntent::BeginAgent {
                    grant: WorkAgentGrantV1 {
                        no_usage_charge: false,
                        draft: None,
                        lead: None,
                        provider: WorkSearchProvider::OpenAi,
                        model: PUBLIC_SEARCH_MODEL.into(),
                        max_turns: 8,
                        max_steps: 24,
                        browse_hops: 1,
                        folders: vec![],
                        accounts: Vec::new(),
                        private: false,
                        skill: None,
                        search_fallback: None,
                    },
                    limits: WorkExecutionLimits {
                        model_tokens: 100_000,
                        cost_micro_usd: 100_000,
                        operations: 32,
                        timeout_seconds: 30,
                        max_workers: 1,
                    },
                },
            },
            None,
            WorkAgentProviders {
                turn: &provider,
                search: &NoSearch,
            },
            |_, request| async move {
                Ok(WorkBrowserOutcome {
                    status: WorkStepStatus::Succeeded,
                    usage: Some(WorkUsage::default()),
                    intervention: None,
                    note: None,
                    measurements: None,
                    helped: false,
                    artifacts: vec![WorkArtifactDraft {
                        output: request.output,
                        title: "Earlier partial result".into(),
                        evidence: vec![WorkEvidenceLink {
                            extraction_id: WorkArtifactId::from(100),
                            source_id: 1,
                        }],
                        data: WorkArtifactDataV1::Document {
                            paragraphs: vec!["Partial page data".into()],
                            formatted: None,
                        },
                    }],
                    held_back: false,
                    committed: Vec::new(),
                    rerun: false,
                    search: None,
                    redirect: None,
                })
            },
            |_| {},
        ),
    )
    .await
    .unwrap();
    let execution = &result.executions[0];
    assert_eq!(execution.artifacts.len(), 1);
    assert_eq!(execution.status, WorkExecutionStatus::Failed);
    assert!(!execution
        .steps
        .iter()
        .any(|step| matches!(step.kind, WorkStepKindV1::Finish { .. })));
    assert!(execution
        .steps
        .iter()
        .filter(|step| step.turn > 1)
        .all(|step| step.note.as_deref() != Some("Completed the final comparison")));
    assert_eq!(provider.0.load(std::sync::atomic::Ordering::SeqCst), 4);
}

#[tokio::test]
async fn a_run_that_runs_out_of_time_says_so() {
    use crate::work_agent::*;
    use zephium_core::work::{agent::*, search::*, synthesis::*};
    struct Reads;
    impl WorkAgentTurnProvider for Reads {
        fn turn<'a>(
            &'a self,
            _: &'a WorkAgentTurnDisclosure,
            _: WorkSynthesisTrace,
        ) -> WorkAgentTurnFuture<'a> {
            Box::pin(async move {
                Ok(WorkAgentTurnResult {
                    output: WorkAgentTurnOutput {
                        say: None,
                        artifacts: vec![],
                        fetch: vec![WorkAgentFetch::Read {
                            url: "https://example.test/catalog".into(),
                            collection: None,
                        }],
                        ask: None,
                        finish: false,
                        followups: vec![],
                        malformed: 0,
                    },
                    usage: WorkUsage::default(),
                })
            })
        }
    }
    struct NoSearch;
    impl WorkPublicSearchProvider for NoSearch {
        fn search<'a>(
            &'a self,
            _: &'a WorkPublicSearchScope,
            _: &'a [zephium_core::work::context::WorkContextBody],
            _: WorkExecutionLimits,
        ) -> WorkPublicSearchFuture<'a> {
            panic!("no search");
        }
    }
    let store = Arc::new(zephium_store::SqliteStore::in_memory().unwrap());
    let (mut shell, queue, handle, profile) = fixture(store);
    let create = handle
        .work_document(WorkIntent::Create {
            objective: "Read https://example.test/catalog".into(),
        })
        .unwrap();
    let work = create.work_id().unwrap();
    drive(&mut shell, &queue, create).await.unwrap();
    let service = WorkAgentService::new(handle);
    let result = drive(
        &mut shell,
        &queue,
        service.run(
            profile,
            WorkCommandV1 {
                version: 1,
                work,
                expected_revision: WorkRevision::INITIAL,
                command: WorkCommandId::generate(),
                intent: WorkRuntimeIntent::BeginAgent {
                    grant: WorkAgentGrantV1 {
                        no_usage_charge: false,
                        draft: None,
                        lead: None,
                        provider: WorkSearchProvider::OpenAi,
                        model: PUBLIC_SEARCH_MODEL.into(),
                        max_turns: 8,
                        max_steps: 24,
                        browse_hops: 1,
                        folders: vec![],
                        accounts: Vec::new(),
                        private: false,
                        skill: None,
                        search_fallback: None,
                    },
                    limits: WorkExecutionLimits {
                        model_tokens: 100_000,
                        cost_micro_usd: 100_000,
                        operations: 32,
                        timeout_seconds: 1,
                        max_workers: 1,
                    },
                },
            },
            None,
            WorkAgentProviders {
                turn: &Reads,
                search: &NoSearch,
            },
            |_, _| async {
                tokio::time::sleep(Duration::from_millis(1300)).await;
                Ok(WorkBrowserOutcome {
                    status: WorkStepStatus::Failed,
                    usage: Some(WorkUsage::default()),
                    intervention: None,
                    note: None,
                    measurements: None,
                    helped: false,
                    artifacts: vec![],
                    held_back: false,
                    committed: Vec::new(),
                    rerun: false,
                    search: None,
                    redirect: None,
                })
            },
            |_| {},
        ),
    )
    .await
    .unwrap();
    let execution = &result.executions[0];
    assert_eq!(execution.status, WorkExecutionStatus::Failed);
    let last = execution.steps.last().unwrap();
    assert!(matches!(last.kind, WorkStepKindV1::Turn));
    assert_eq!(last.status, WorkStepStatus::Failed);
    assert_eq!(last.note.as_deref(), Some("The run ran out of time"));
}

#[path = "work_lifecycle_tests.rs"]
mod lifecycle_tests;
