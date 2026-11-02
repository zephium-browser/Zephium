//! Stop reaches noncooperating adapters without waiting for their timeout.
use super::work_lead_tests::{command, model};
use super::work_planning_tests::{drive, fixture};
use crate::work_lead::{WorkLeadModels, WorkLeadService};
use crate::WorkIntent;
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc,
};
use std::time::{Duration, Instant};
use zephium_core::work::{model::*, port::*, runtime::*, search::*, *};
use zephium_ipc::work::WorkCommandV1;

struct StalledModel {
    started: tokio::sync::Notify,
    calls: AtomicUsize,
    dropped: AtomicBool,
    refusal: bool,
}
impl WorkModelClient for StalledModel {
    fn call<'a>(
        &'a self,
        _: WorkModelRequest,
        _: &'a (dyn Fn(WorkModelEvent) + Send + Sync),
    ) -> WorkModelFuture<'a> {
        Box::pin(async move {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.started.notify_one();
            if self.refusal {
                return Err(WorkModelError::RateLimited {
                    retry_after_ms: Some(30_000),
                });
            }
            struct Dropped<'a>(&'a AtomicBool);
            impl Drop for Dropped<'_> {
                fn drop(&mut self) {
                    self.0.store(true, Ordering::SeqCst);
                }
            }
            let _dropped = Dropped(&self.dropped);
            std::future::pending().await
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
        panic!("Stop never launches another tool")
    }
}

#[tokio::test]
async fn stop_settles_a_noncooperating_model_within_one_second_without_refunding_it() {
    stopped_call(false, false, false).await;
}
#[tokio::test]
async fn stop_interrupts_provider_retry_backoff_without_retrying() {
    stopped_call(true, false, false).await;
}
#[tokio::test]
async fn stopping_a_fully_local_run_keeps_unknown_tokens_without_inventing_a_charge() {
    stopped_call(false, true, false).await;
}
struct FreeSearch;
impl WorkPublicSearchProvider for FreeSearch {
    fn grant(&self) -> Option<(WorkSearchProvider, String)> {
        Some((WorkSearchProvider::Browser, "duckduckgo".into()))
    }
    fn search<'a>(
        &'a self,
        _: &'a WorkPublicSearchScope,
        _: &'a [zephium_core::work::context::WorkContextBody],
        _: WorkExecutionLimits,
    ) -> WorkPublicSearchFuture<'a> {
        panic!("no search requested")
    }
}
#[tokio::test]
async fn stopping_a_local_call_in_a_mixed_run_preserves_paid_accounting() {
    stopped_call(false, true, true).await;
}
#[test]
fn caller_cannot_claim_an_execution_has_no_usage_charge() {
    let mut request = command(WorkId::generate(), WorkRevision::INITIAL);
    assert!(request.clone().into_request().is_ok());
    let WorkRuntimeIntent::BeginAgent { grant, .. } = &mut request.intent else {
        panic!()
    };
    grant.no_usage_charge = true;
    assert!(matches!(request.into_request(), Err(WorkError::Invalid)));
}
async fn stopped_call(refusal: bool, local: bool, paid_helper: bool) {
    let store = Arc::new(zephium_store::SqliteStore::in_memory().unwrap());
    let (mut shell, queue, handle, profile) = fixture(store);
    let create = handle
        .work_document(WorkIntent::Create {
            objective: "Compare two public pages".into(),
        })
        .unwrap();
    let work = create.work_id().unwrap();
    drive(&mut shell, &queue, create).await.unwrap();
    let client = Arc::new(StalledModel {
        started: tokio::sync::Notify::new(),
        calls: AtomicUsize::new(0),
        dropped: AtomicBool::new(false),
        refusal,
    });
    let mut lead = model(client.clone());
    if local {
        lead.entry.model.provider = WorkModelProvider::Compatible;
        lead.entry.price = Some(WorkModelPrice {
            input: 0,
            cached_input: 0,
            output: 0,
        });
    }
    let page = if paid_helper {
        model(client.clone())
    } else {
        lead.clone()
    };
    let models = WorkLeadModels {
        lead: lead.clone(),
        page,
        light: lead,
    };
    let service = WorkLeadService::new(handle.clone());
    let running = service.run(
        profile,
        command(work, WorkRevision::INITIAL),
        None,
        models,
        if local { &FreeSearch } else { &NoSearch },
        |_, _| async { panic!("No browser work was requested") },
        |_| {},
    );
    let stopping = async {
        client.started.notified().await;
        let response = handle
            .submit_work_document(WorkRequest::RuntimeRead { id: work }, Some(profile))
            .unwrap()
            .await
            .unwrap();
        let WorkReply::Runtime(state) = response.reply else {
            panic!("runtime projection");
        };
        assert!(
            state.interrupted.is_empty(),
            "pending admission and its live driver are not orphaned"
        );
        assert!(crate::work_runtime::has_execution_driver(
            profile,
            work,
            state.executions[0].id
        ));
        let forged = WorkRequest::AcknowledgeOrphan {
            id: work,
            expected: state.work.revision,
            command: WorkCommandId::generate(),
            execution: state.executions[0].id,
        };
        assert!(matches!(
            handle.submit_owned_work_runtime(forged.clone(), profile),
            Err(WorkError::Invalid)
        ));
        assert!(matches!(
            handle
                .submit_work_document(forged, Some(profile))
                .unwrap()
                .await,
            Err(WorkError::Invalid)
        ));
        assert!(
            matches!(
                handle
                    .work_command(
                        profile,
                        WorkCommandV1 {
                            version: 1,
                            work,
                            expected_revision: state.work.revision,
                            command: WorkCommandId::generate(),
                            intent: WorkRuntimeIntent::AcknowledgeInterruption {
                                execution: state.executions[0].id
                            },
                        }
                    )
                    .unwrap()
                    .await,
                Err(WorkError::Conflict)
            ),
            "a live driver cannot be acknowledged away"
        );
        let started = Instant::now();
        handle
            .work_command(
                profile,
                WorkCommandV1 {
                    version: 1,
                    work,
                    expected_revision: state.work.revision,
                    command: WorkCommandId::generate(),
                    intent: WorkRuntimeIntent::Cancel {
                        execution: state.executions[0].id,
                        intervention: None,
                    },
                },
            )
            .unwrap()
            .await
            .unwrap();
        started
    };
    let (done, started) = drive(
        &mut shell,
        &queue,
        tokio::time::timeout(Duration::from_secs(2), async {
            tokio::join!(running, stopping)
        }),
    )
    .await
    .expect("a hung model or retry backoff cannot trap Stop");
    let done = done.unwrap();
    assert!(started.elapsed() < Duration::from_secs(1));
    let execution = &done.executions[0];
    assert_eq!(execution.status, WorkExecutionStatus::Cancelled);
    assert_eq!(
        execution.attempts[0].status,
        WorkAttemptStatus::OutcomeUnknown
    );
    assert!(execution
        .steps
        .iter()
        .all(|step| step.status != WorkStepStatus::Running));
    // Logical cancellation preserves its admitted reservation until the
    // original owner/journal can establish what actually happened. A late
    // provider return cannot silently refund or replace this unknown usage.
    assert_eq!(
        execution.agent_grant().unwrap().no_usage_charge,
        local && !paid_helper
    );
    if local && !paid_helper {
        let usage = execution.attempts[0]
            .usage
            .expect("local unknown usage retains a zero-price reservation");
        assert_eq!(usage.cost_micro_usd, 0);
        assert!(usage.model_tokens > 0);
        assert_eq!(
            usage.accounting,
            WorkUsageAccounting::ConservativeReservation
        );
    } else {
        assert!(execution.attempts[0].usage.is_none());
    }
    assert!(execution.spec.limits.model_tokens > 0);
    assert!(execution.spec.limits.cost_micro_usd > 0);
    if !refusal {
        assert!(client.dropped.load(Ordering::SeqCst));
    }
    assert_eq!(client.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn a_dropped_driver_is_projected_interrupted_even_when_abandonment_cannot_save() {
    recover_orphan(false).await;
}

#[tokio::test]
async fn a_same_process_orphan_can_be_acknowledged_and_continue_without_replaying_its_run() {
    recover_orphan(true).await;
}

async fn recover_orphan(acknowledge: bool) {
    let store = Arc::new(zephium_store::SqliteStore::in_memory().unwrap());
    let (mut shell, queue, handle, profile) = fixture(store);
    let create = handle
        .work_document(WorkIntent::Create {
            objective: "Read a public page".into(),
        })
        .unwrap();
    let work = create.work_id().unwrap();
    drive(&mut shell, &queue, create).await.unwrap();
    let client = Arc::new(StalledModel {
        started: tokio::sync::Notify::new(),
        calls: AtomicUsize::new(0),
        dropped: AtomicBool::new(false),
        refusal: false,
    });
    let lead = model(client.clone());
    let service = WorkLeadService::new(handle.clone());
    {
        let running = service.run(
            profile,
            command(work, WorkRevision::INITIAL),
            None,
            WorkLeadModels {
                lead: lead.clone(),
                page: lead.clone(),
                light: lead,
            },
            &NoSearch,
            |_, _| async { panic!("No browser work") },
            |_| {},
        );
        tokio::pin!(running);
        drive(&mut shell, &queue, async {
            tokio::select! {
                _ = client.started.notified() => {},
                result = &mut running => panic!("model must remain pending: {result:?}"),
            }
        })
        .await;
    }
    assert!(client.dropped.load(Ordering::SeqCst));
    // Simulate rejected/undeliverable abandonment. The store still says Running;
    // only exact live ownership can justify displaying it that way.
    while queue.try_recv().is_some() {}
    let response = drive(
        &mut shell,
        &queue,
        handle
            .submit_work_document(WorkRequest::RuntimeRead { id: work }, Some(profile))
            .unwrap(),
    )
    .await
    .unwrap();
    let WorkReply::Runtime(state) = response.reply else {
        panic!("runtime projection")
    };
    let execution = &state.executions[0];
    assert_eq!(execution.status, WorkExecutionStatus::Running);
    assert_eq!(execution.attempts[0].status, WorkAttemptStatus::Running);
    assert_eq!(state.interrupted, vec![execution.id]);
    assert!(!crate::work_runtime::has_execution_driver(
        profile,
        work,
        execution.id
    ));
    let started = Instant::now();
    let recovery_command = WorkCommandV1 {
        version: 1,
        work,
        expected_revision: state.work.revision,
        command: WorkCommandId::generate(),
        intent: if acknowledge {
            WorkRuntimeIntent::AcknowledgeInterruption {
                execution: execution.id,
            }
        } else {
            WorkRuntimeIntent::Cancel {
                execution: execution.id,
                intervention: None,
            }
        },
    };
    let stopped = drive(
        &mut shell,
        &queue,
        handle
            .work_command(profile, recovery_command.clone())
            .unwrap(),
    )
    .await
    .unwrap();
    let WorkReply::RuntimeCommand { projection, .. } = stopped.reply else {
        panic!("cancel receipt")
    };
    assert!(started.elapsed() < Duration::from_secs(1));
    assert_eq!(
        projection.executions[0].status,
        if acknowledge {
            WorkExecutionStatus::Interrupted
        } else {
            WorkExecutionStatus::Cancelled
        }
    );
    assert!(projection.interrupted.is_empty());
    if acknowledge {
        let replay = drive(
            &mut shell,
            &queue,
            handle.work_command(profile, recovery_command).unwrap(),
        )
        .await
        .unwrap();
        let WorkReply::RuntimeCommand {
            projection: replay, ..
        } = replay.reply
        else {
            panic!("replayed acknowledgement")
        };
        assert_eq!(
            replay.work.revision, projection.work.revision,
            "original command digest and receipt replay unchanged"
        );
        let _starting =
            crate::work_runtime::WorkStartingAgent::acquire(&handle, profile, work).unwrap();
        let next = drive(
            &mut shell,
            &queue,
            handle
                .work_command(profile, command(work, projection.work.revision))
                .unwrap(),
        )
        .await
        .unwrap();
        let WorkReply::AgentAdmitted {
            projection: next, ..
        } = next.reply
        else {
            panic!("next run admitted")
        };
        assert_eq!(next.executions.len(), 2);
        assert_eq!(next.executions[1].status, WorkExecutionStatus::Approved);
        assert!(
            next.interrupted.is_empty(),
            "fresh approval has no missing driver"
        );
        assert_eq!(
            client.calls.load(Ordering::SeqCst),
            1,
            "recovery never replays the old model call"
        );
    }
}

struct FetchModel;
impl WorkModelClient for FetchModel {
    fn call<'a>(
        &'a self,
        _: WorkModelRequest,
        _: &'a (dyn Fn(WorkModelEvent) + Send + Sync),
    ) -> WorkModelFuture<'a> {
        Box::pin(async {
            Ok(WorkModelOutcome {
                stop: WorkModelStop::ToolUse,
                usage: WorkModelUsage {
                    input_tokens: 10,
                    cached_input_tokens: 0,
                    cache_write_input_tokens: 0,
                    output_tokens: 10,
                    reasoning_tokens: 0,
                    cost_micros: None,
                },
                assistant: vec![WorkModelPart::ToolCall(WorkModelToolCall {
                    id: "page".into(),
                    name: "web_fetch".into(),
                    arguments: serde_json::json!({"url": "https://example.com/"}),
                })],
            })
        })
    }
}

#[tokio::test]
async fn stop_revokes_a_hung_browser_and_closes_logical_work_before_its_native_drain() {
    let store = Arc::new(zephium_store::SqliteStore::in_memory().unwrap());
    let (mut shell, queue, handle, profile) = fixture(store);
    let create = handle
        .work_document(WorkIntent::Create {
            objective: "Read https://example.com/".into(),
        })
        .unwrap();
    let work = create.work_id().unwrap();
    drive(&mut shell, &queue, create).await.unwrap();
    let lead = model(Arc::new(FetchModel));
    let service = WorkLeadService::new(handle.clone());
    let session = Arc::new(std::sync::Mutex::new(None));
    let started = Arc::new(tokio::sync::Notify::new());
    let drained = Arc::new(tokio::sync::Notify::new());
    let browser_done = Arc::new(AtomicBool::new(false));
    let browser = {
        let session = session.clone();
        let started = started.clone();
        let drained = drained.clone();
        let browser_done = browser_done.clone();
        move |probe: crate::work_runtime::WorkAttemptProbe, _| {
            let session = session.clone();
            let started = started.clone();
            let drained = drained.clone();
            let browser_done = browser_done.clone();
            async move {
                *session.lock().unwrap() = Some(probe.browser_session());
                started.notify_one();
                // This adapter deliberately ignores cancellation. The original
                // native owner still owns cleanup until the drain releases it.
                drained.notified().await;
                browser_done.store(true, Ordering::SeqCst);
                Err(WorkError::OutcomeUnknown)
            }
        }
    };
    let running = service.run(
        profile,
        command(work, WorkRevision::INITIAL),
        None,
        WorkLeadModels {
            lead: lead.clone(),
            page: lead.clone(),
            light: lead,
        },
        &NoSearch,
        browser,
        |_| {},
    );
    let stopping = async {
        started.notified().await;
        let response = handle
            .submit_work_document(WorkRequest::RuntimeRead { id: work }, Some(profile))
            .unwrap()
            .await
            .unwrap();
        let WorkReply::Runtime(state) = response.reply else {
            panic!("runtime projection")
        };
        let begun = Instant::now();
        let cancelled = handle
            .work_command(
                profile,
                WorkCommandV1 {
                    version: 1,
                    work,
                    expected_revision: state.work.revision,
                    command: WorkCommandId::generate(),
                    intent: WorkRuntimeIntent::Cancel {
                        execution: state.executions[0].id,
                        intervention: None,
                    },
                },
            )
            .unwrap()
            .await
            .unwrap();
        let WorkReply::RuntimeCommand { projection, .. } = cancelled.reply else {
            panic!("cancel receipt")
        };
        assert!(begun.elapsed() < Duration::from_secs(1));
        assert_eq!(
            projection.executions[0].status,
            WorkExecutionStatus::Cancelled
        );
        assert!(projection.executions[0]
            .steps
            .iter()
            .all(|step| step.status != WorkStepStatus::Running));
        assert!(!session.lock().unwrap().as_ref().unwrap().is_current());
        assert!(
            !browser_done.load(Ordering::SeqCst),
            "logical Stop must not claim native cleanup finished"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(!browser_done.load(Ordering::SeqCst));
        drained.notify_one();
    };
    let (done, ()) = drive(
        &mut shell,
        &queue,
        tokio::time::timeout(Duration::from_secs(3), async {
            tokio::join!(running, stopping)
        }),
    )
    .await
    .expect("Stop and eventual native drain both finish");
    assert!(browser_done.load(Ordering::SeqCst));
    assert_eq!(
        done.unwrap().executions[0].status,
        WorkExecutionStatus::Cancelled
    );
}

#[tokio::test]
async fn an_approved_fifth_run_refused_by_capacity_does_not_remain_live_or_disrupt_existing_runs() {
    let store = Arc::new(zephium_store::SqliteStore::in_memory().unwrap());
    let (mut shell, queue, handle, profile) = fixture(store);
    let runtime = crate::work_runtime::WorkRuntimeService::new(handle.clone());
    let mut active = Vec::new();
    for index in 0..6 {
        let create = handle
            .work_document(WorkIntent::Create {
                objective: format!("Capacity fixture {index}"),
            })
            .unwrap();
        let work = create.work_id().unwrap();
        drive(&mut shell, &queue, create).await.unwrap();
        let starting =
            crate::work_runtime::WorkStartingAgent::acquire(&handle, profile, work).unwrap();
        let admitted = drive(
            &mut shell,
            &queue,
            handle
                .work_command(profile, command(work, WorkRevision::INITIAL))
                .unwrap(),
        )
        .await
        .unwrap();
        let WorkReply::AgentAdmitted {
            projection,
            receipt,
            ..
        } = admitted.reply
        else {
            panic!("agent admission");
        };
        assert!(
            projection.interrupted.is_empty(),
            "new approval alone is not an orphan"
        );
        let node = projection.executions[0].spec.nodes[0].node;
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
        .await;
        drop(starting);
        if index == 5 {
            // Lose the terminal cleanup request as well. Guard absence, not a
            // growing failure cache, must still suppress this unstarted ghost.
            while queue.try_recv().is_some() {}
        }
        if index < 4 {
            active.push(attempt.expect("four worker permits are available"));
        } else {
            assert!(matches!(attempt, Err(WorkError::Capacity)));
            let state = drive(
                &mut shell,
                &queue,
                handle.work_projection(profile, work).unwrap(),
            )
            .await
            .unwrap();
            let WorkReply::Runtime(state) = state.reply else {
                panic!("runtime projection")
            };
            assert_eq!(
                state.executions[0].status,
                if index == 5 {
                    WorkExecutionStatus::Approved
                } else {
                    WorkExecutionStatus::Interrupted
                }
            );
            if index == 5 {
                assert_eq!(state.interrupted, vec![receipt.execution]);
                let recovered = drive(
                    &mut shell,
                    &queue,
                    handle
                        .work_command(
                            profile,
                            WorkCommandV1 {
                                version: 1,
                                work,
                                expected_revision: state.work.revision,
                                command: WorkCommandId::generate(),
                                intent: WorkRuntimeIntent::AcknowledgeInterruption {
                                    execution: receipt.execution,
                                },
                            },
                        )
                        .unwrap(),
                )
                .await
                .unwrap();
                let WorkReply::RuntimeCommand {
                    projection: recovered,
                    ..
                } = recovered.reply
                else {
                    panic!("orphan acknowledgement")
                };
                assert_eq!(
                    recovered.executions[0].status,
                    WorkExecutionStatus::Interrupted
                );
                assert!(recovered.interrupted.is_empty());
            }
            assert!(
                state.executions[0].attempts.is_empty(),
                "a refused Begin never invents an attempt"
            );
            assert!(!crate::work_runtime::has_execution_driver(
                profile,
                work,
                receipt.execution
            ));
        }
    }
    let original = &active[0];
    let state = drive(&mut shell, &queue, original.probe().runtime_projection())
        .await
        .unwrap();
    let duplicate = drive(
        &mut shell,
        &queue,
        runtime.begin_node(
            profile,
            original.work(),
            state.work.revision,
            original.execution(),
            original.node().id,
        ),
    )
    .await;
    assert!(matches!(duplicate, Err(WorkError::Capacity)));
    let state = drive(&mut shell, &queue, original.probe().runtime_projection())
        .await
        .unwrap();
    assert_eq!(
        state.executions[0].status,
        WorkExecutionStatus::Running,
        "an unknown duplicate attempt cannot abandon its already-running original"
    );
    assert_eq!(state.executions[0].attempts.len(), 1);
    assert_eq!(state.executions[0].attempts[0].id, original.attempt());
    assert!(state.interrupted.is_empty());
    assert!(original.probe().browser_session().is_current());
}

#[tokio::test]
async fn work_owned_terminal_refusal_recovers_a_published_reply_without_hiding_mutation_debt() {
    use crate::work_runtime::{
        WorkAdapterResult, WorkArtifactDraft, WorkRuntimeService, WorkStartingAgent,
    };
    use zephium_core::work::artifact::WorkArtifactDataV1;
    for uncertain_write in [false, true] {
        let store = Arc::new(zephium_store::SqliteStore::in_memory().unwrap());
        let (mut shell, queue, handle, profile) = fixture(store);
        let create = handle
            .work_document(WorkIntent::Create {
                objective: "Prepare an exam study plan".into(),
            })
            .unwrap();
        let work = create.work_id().unwrap();
        drive(&mut shell, &queue, create).await.unwrap();
        let starting = WorkStartingAgent::acquire(&handle, profile, work).unwrap();
        let admitted = drive(
            &mut shell,
            &queue,
            handle
                .work_command(profile, command(work, WorkRevision::INITIAL))
                .unwrap(),
        )
        .await
        .unwrap();
        let WorkReply::AgentAdmitted {
            projection,
            receipt,
            ..
        } = admitted.reply
        else {
            panic!("agent admission");
        };
        let node = projection.executions[0].spec.nodes[0].node;
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
        drop(starting);
        let original = attempt.attempt();
        let probe = attempt.probe();
        let step = |kind, status, artifacts| WorkStepFact {
            model_fault: None,
            id: WorkStepId::generate(),
            turn: 1,
            kind,
            status,
            usage: None,
            artifacts,
            evidence: None,
            note: None,
            measurements: None,
            local: None,
            account: None,
            part: None,
        };
        if uncertain_write {
            drive(
                &mut shell,
                &queue,
                probe.commit_step(WorkRuntimeUpdate::BeginStep {
                    execution: receipt.execution,
                    attempt: original,
                    step: step(
                        WorkStepKindV1::WriteFile {
                            path: "/fixture/study-plan.txt".into(),
                            content: "Approved plan".into(),
                            decision: Some(true),
                        },
                        WorkStepStatus::Running,
                        vec![],
                    ),
                    artifacts: vec![],
                    evidence: None,
                    file: None,
                }),
            )
            .await
            .unwrap();
        }
        let artifact = attempt
            .mint_marked_artifact(
                WorkArtifactDraft {
                    output: attempt.node().outputs[0].name.clone(),
                    title: "Exam plan".into(),
                    data: WorkArtifactDataV1::Reply {
                        headline: "Your study plan".into(),
                        text: "Review the collected course material.".into(),
                        figures: vec![],
                        points: vec![],
                    },
                    evidence: vec![],
                },
                true,
            )
            .unwrap();
        drive(
            &mut shell,
            &queue,
            probe.commit_step(WorkRuntimeUpdate::BeginStep {
                execution: receipt.execution,
                attempt: original,
                step: step(
                    WorkStepKindV1::Publish,
                    WorkStepStatus::Succeeded,
                    vec![artifact.id],
                ),
                artifacts: vec![artifact.clone()],
                evidence: None,
                file: None,
            }),
        )
        .await
        .unwrap();
        drive(
            &mut shell,
            &queue,
            probe.commit_step(WorkRuntimeUpdate::BeginStep {
                execution: receipt.execution,
                attempt: original,
                step: step(
                    WorkStepKindV1::Finish {
                        followups: vec![],
                        title: None,
                    },
                    WorkStepStatus::Succeeded,
                    vec![],
                ),
                artifacts: vec![],
                evidence: None,
                file: None,
            }),
        )
        .await
        .unwrap();
        // A successful attempt cannot carry an intervention: Store definitively
        // refuses this terminal write. Only terminal bookkeeping is recovered.
        let settlement = drive(
            &mut shell,
            &queue,
            attempt.settle_owned(WorkAdapterResult {
                status: WorkAttemptStatus::Succeeded,
                usage: Some(WorkUsage::default()),
                artifacts: vec![],
                intervention: Some(WorkInterventionV1 {
                    kind: WorkInterventionKindV1::SignIn,
                    origin: None,
                }),
            }),
        )
        .await
        .expect("the original owner returns its recovered terminal receipt");
        assert_eq!(settlement.profile(), profile);
        assert_eq!(settlement.work(), work);
        assert_eq!(settlement.execution(), receipt.execution);
        assert_eq!(settlement.attempt(), original);
        assert_eq!(settlement.node(), node);
        let execution = &settlement.projection().executions[0];
        assert_eq!(execution.artifacts, vec![artifact]);
        assert!(execution
            .steps
            .iter()
            .all(|step| step.status != WorkStepStatus::Running));
        if uncertain_write {
            assert_eq!(execution.status, WorkExecutionStatus::Interrupted);
            assert_eq!(
                execution.attempts[0].status,
                WorkAttemptStatus::OutcomeUnknown
            );
            assert!(execution.attempts[0].usage.is_none());
            assert_eq!(execution.steps[0].status, WorkStepStatus::OutcomeUnknown);
        } else {
            assert_eq!(execution.status, WorkExecutionStatus::NeedsReview);
            assert_eq!(execution.attempts[0].status, WorkAttemptStatus::Succeeded);
            let usage = execution.attempts[0].usage.unwrap();
            assert_eq!(
                usage.accounting,
                WorkUsageAccounting::ConservativeReservation
            );
            assert_eq!(usage.model_tokens, execution.spec.limits.model_tokens);
            assert_eq!(usage.cost_micro_usd, execution.spec.limits.cost_micro_usd);
        }
    }
}
