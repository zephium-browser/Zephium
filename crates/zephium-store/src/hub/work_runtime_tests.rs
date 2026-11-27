use super::super::tests::{create, draft, edit, session};
use super::*;

#[test]
fn coordination_requires_exact_live_parent_and_complete_children() {
    let mut hub = Hub::in_memory().unwrap();
    hub.save(&session()).unwrap();
    let initial = create(&mut hub);
    let mut graph = draft();
    graph.nodes[0].outputs[0].review = WorkOutputReview::UserAcceptance;
    let root = graph.nodes[0].id;
    let mut child = graph.nodes[0].clone();
    child.id = 13.into();
    graph.nodes[0].dependencies = vec![child.id];
    graph.nodes.push(child);
    let planned = edit(&mut hub, &initial, WorkEdit::ReplaceDraft { draft: graph }).unwrap();
    let mut execution_spec = spec(planned.plan.as_ref().unwrap());
    execution_spec.limits.max_workers = 2;
    execution_spec.limits.model_tokens *= 2;
    execution_spec.limits.cost_micro_usd *= 2;
    execution_spec.limits.operations *= 2;
    execution_spec.nodes[0].capability = WorkCapability::Coordinate {
        scope: WorkBrowseScope {
            start_url: "https://example.test/".into(),
            routes: vec![WorkBrowseRoute {
                origin: "https://example.test".into(),
                path_prefix: "/".into(),
            }],
            max_hops: 1,
        },
    };
    execution_spec.nodes[1].parent = Some(root);
    let WorkReply::RuntimeCommand { receipt, .. } = hub
        .work_document(
            planned.profile,
            WorkRequest::RuntimeCommand {
                id: planned.id,
                expected: planned.revision,
                command: 100.into(),
                intent: WorkRuntimeIntent::Approve {
                    spec: execution_spec,
                },
            },
        )
        .unwrap()
    else {
        panic!()
    };
    let execution = receipt.execution;
    let update = |hub: &mut Hub, update| {
        let state = read_runtime(hub, &planned);
        hub.work_document(
            planned.profile,
            WorkRequest::RuntimeUpdate {
                id: planned.id,
                expected: state.work.revision,
                update,
            },
        )
    };
    // Neither matching metadata nor a child-only Begin admits a child.
    assert!(matches!(
        update(
            &mut hub,
            WorkRuntimeUpdate::BeginChild {
                execution,
                attempt: 501.into(),
                node: 13.into(),
                parent: 500.into()
            }
        ),
        Err(WorkError::Unavailable)
    ));
    assert!(matches!(
        update(
            &mut hub,
            WorkRuntimeUpdate::Begin {
                execution,
                attempt: 501.into(),
                node: 13.into()
            }
        ),
        Err(WorkError::Unavailable)
    ));
    update(
        &mut hub,
        WorkRuntimeUpdate::Begin {
            execution,
            attempt: 500.into(),
            node: root,
        },
    )
    .unwrap();
    assert!(matches!(
        update(
            &mut hub,
            WorkRuntimeUpdate::BeginChild {
                execution,
                attempt: 501.into(),
                node: 13.into(),
                parent: 999.into()
            }
        ),
        Err(WorkError::Unavailable)
    ));
    let artifact = |node, attempt| artifact::WorkArtifactV1 {
        revises: None,
        part: None,
        version: 1,
        id: WorkArtifactId::generate(),
        execution,
        node,
        attempt,
        output: "sources".into(),
        title: "Proposed checks".into(),
        data: artifact::WorkArtifactDataV1::Document {
            paragraphs: vec!["Review release changes".into()],
            formatted: None,
        },
        evidence: vec![],
        review: WorkOutputReview::UserAcceptance,
        presentation: artifact::WorkArtifactPresentationV1::Automatic,
        general_knowledge: false,
    };
    assert!(matches!(
        update(
            &mut hub,
            WorkRuntimeUpdate::Settle {
                execution,
                intervention: None,
                attempt: 500.into(),
                status: WorkAttemptStatus::Succeeded,
                usage: Some(WorkUsage::default()),
                artifacts: vec![artifact(root, 500.into())]
            }
        ),
        Err(WorkError::Conflict)
    ));
    update(
        &mut hub,
        WorkRuntimeUpdate::BeginChild {
            execution,
            attempt: 501.into(),
            node: 13.into(),
            parent: 500.into(),
        },
    )
    .unwrap();
    update(
        &mut hub,
        WorkRuntimeUpdate::Settle {
            execution,
            intervention: None,
            attempt: 501.into(),
            status: WorkAttemptStatus::Succeeded,
            usage: Some(WorkUsage::default()),
            artifacts: vec![artifact(13.into(), 501.into())],
        },
    )
    .unwrap();
    update(
        &mut hub,
        WorkRuntimeUpdate::Settle {
            execution,
            intervention: None,
            attempt: 500.into(),
            status: WorkAttemptStatus::Succeeded,
            usage: Some(WorkUsage::default()),
            artifacts: vec![artifact(root, 500.into())],
        },
    )
    .unwrap();
    let state = read_runtime(&mut hub, &planned);
    assert_eq!(state.executions[0].status, WorkExecutionStatus::NeedsReview);
    // A canvas reference retains the exact historical result and does not
    // accept a valid artifact ID paired with another execution or profile.
    let original = &state.executions[0].artifacts[0];
    let conn = hub.profile_conn(planned.profile).unwrap();
    assert!(
        validate_artifact_reference(conn, planned.profile, planned.id, execution, original.id)
            .is_ok()
    );
    assert!(matches!(
        validate_artifact_reference(conn, planned.profile, planned.id, 999.into(), original.id),
        Err(WorkError::NotFound)
    ));
    assert!(matches!(
        validate_artifact_reference(conn, planned.profile, planned.id, execution, 999.into()),
        Err(WorkError::NotFound)
    ));
    // Profile authority selects the database at Hub admission; a caller cannot
    // choose a different profile label for an already-open connection.
    assert!(matches!(
        hub.work_document(999.into(), WorkRequest::RuntimeRead { id: planned.id }),
        Err(WorkError::ProfileUnavailable)
    ));
    // Durable decoding independently refuses an impossible parent success.
    let mut forged = state.executions[0].clone();
    forged.attempts.pop();
    forged.artifacts.retain(|a| a.node == root);
    assert!(forged
        .validate(planned.plan.as_ref().unwrap(), state.work.revision)
        .is_err());
}

fn spec(plan: &WorkPlanRevision) -> WorkExecutionSpec {
    let limits = WorkExecutionLimits {
        model_tokens: 8000,
        cost_micro_usd: 10000,
        operations: 32,
        timeout_seconds: 300,
        max_workers: 1,
    };
    WorkExecutionSpec {
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
                limits,
            })
            .collect(),
    }
}
fn approve(
    hub: &mut Hub,
    work: &WorkSnapshot,
    command: WorkCommandId,
) -> (WorkRuntimeProjection, WorkCommandReceipt) {
    let WorkReply::RuntimeCommand {
        projection,
        receipt,
    } = hub
        .work_document(
            work.profile,
            WorkRequest::RuntimeCommand {
                id: work.id,
                expected: work.revision,
                command,
                intent: WorkRuntimeIntent::Approve {
                    spec: spec(work.plan.as_ref().unwrap()),
                },
            },
        )
        .unwrap()
    else {
        panic!()
    };
    (*projection, receipt)
}
fn read_runtime(hub: &mut Hub, work: &WorkSnapshot) -> WorkRuntimeProjection {
    let WorkReply::Runtime(projection) = hub
        .work_document(work.profile, WorkRequest::RuntimeRead { id: work.id })
        .unwrap()
    else {
        panic!()
    };
    *projection
}
#[test]
fn approved_revision_is_idempotent_profile_bound_and_retained_after_compaction() {
    let mut hub = Hub::in_memory().unwrap();
    hub.save(&session()).unwrap();
    let initial = create(&mut hub);
    let planned = edit(
        &mut hub,
        &initial,
        WorkEdit::ReplaceDraft { draft: draft() },
    )
    .unwrap();
    let (approved, receipt) = approve(&mut hub, &planned, 100.into());
    let link = artifact::WorkEvidenceLink {
        extraction_id: 42_u128.into(),
        source_id: 1,
    };
    // An arbitrary archive identity cannot disclose content through an unrelated
    // Work, even to another Work in the same profile.
    assert!(matches!(
        hub.work_document(
            planned.profile,
            WorkRequest::ReadEvidence {
                id: planned.id,
                link: link.clone(),
            }
        ),
        Err(WorkError::NotFound)
    ));
    assert!(matches!(
        hub.work_document(
            2.into(),
            WorkRequest::ReadEvidence {
                id: planned.id,
                link,
            }
        ),
        Err(WorkError::NotFound)
    ));
    let (replayed, replay) = approve(&mut hub, &planned, 100.into());
    assert_eq!(receipt, replay);
    assert_eq!(approved.work.revision, replayed.work.revision);
    assert!(matches!(
        hub.work_document(2.into(), WorkRequest::RuntimeRead { id: planned.id }),
        Err(WorkError::NotFound)
    ));
    assert!(matches!(
        edit(
            &mut hub,
            &approved.work,
            WorkEdit::SetObjective {
                objective: "Change authority".into()
            }
        ),
        Err(WorkError::Conflict)
    ));
    assert!(matches!(
        hub.work_document(
            planned.profile,
            WorkRequest::RuntimeCommand {
                id: planned.id,
                expected: approved.work.revision,
                command: 100.into(),
                intent: WorkRuntimeIntent::Cancel {
                    execution: receipt.execution,
                    intervention: None,
                }
            }
        ),
        Err(WorkError::Conflict)
    ));
    hub.work_document(
        planned.profile,
        WorkRequest::RuntimeCommand {
            id: planned.id,
            expected: approved.work.revision,
            command: 101.into(),
            intent: WorkRuntimeIntent::Cancel {
                execution: receipt.execution,
                intervention: None,
            },
        },
    )
    .unwrap();
    let cancelled = read_runtime(&mut hub, &planned);
    let changed = edit(
        &mut hub,
        &cancelled.work,
        WorkEdit::ReplaceDraft { draft: draft() },
    )
    .unwrap();
    let compacted = edit(&mut hub, &changed, WorkEdit::CompactHistory).unwrap();
    assert!(matches!(
        hub.work_document(
            planned.profile,
            WorkRequest::ReadPlan {
                id: planned.id,
                revision: planned.plan.as_ref().unwrap().revision
            }
        ),
        Ok(WorkReply::Plan(_))
    ));
    assert_eq!(
        read_runtime(&mut hub, &compacted).executions[0].status,
        WorkExecutionStatus::Cancelled
    );
    hub.work_document(
        planned.profile,
        WorkRequest::Delete {
            id: planned.id,
            expected: compacted.revision,
        },
    )
    .unwrap();
}
#[test]
fn restart_never_recreates_authority_and_preserves_unknown_reservation() {
    let dir = tempfile::tempdir().unwrap();
    let mut hub = Hub::open(dir.path().into()).unwrap();
    hub.save(&session()).unwrap();
    let initial = create(&mut hub);
    let planned = edit(
        &mut hub,
        &initial,
        WorkEdit::ReplaceDraft { draft: draft() },
    )
    .unwrap();
    let (approved, receipt) = approve(&mut hub, &planned, 100.into());
    let attempt = WorkAttemptId::from(500);
    hub.work_document(
        planned.profile,
        WorkRequest::RuntimeUpdate {
            id: planned.id,
            expected: approved.work.revision,
            update: WorkRuntimeUpdate::Begin {
                execution: receipt.execution,
                attempt,
                node: planned.plan.as_ref().unwrap().draft.nodes[0].id,
            },
        },
    )
    .unwrap();
    drop(hub);
    let mut hub = Hub::open(dir.path().into()).unwrap();
    let interrupted = read_runtime(&mut hub, &planned);
    assert_eq!(interrupted.interrupted, vec![receipt.execution]);
    assert!(matches!(
        hub.work_document(
            planned.profile,
            WorkRequest::RuntimeUpdate {
                id: planned.id,
                expected: interrupted.work.revision,
                update: WorkRuntimeUpdate::Settle {
                    execution: receipt.execution,
                    intervention: None,
                    attempt,
                    status: WorkAttemptStatus::Failed,
                    usage: Some(WorkUsage::default()),
                    artifacts: vec![]
                }
            }
        ),
        Err(WorkError::Conflict)
    ));
    hub.work_document(
        planned.profile,
        WorkRequest::RuntimeCommand {
            id: planned.id,
            expected: interrupted.work.revision,
            command: 101.into(),
            intent: WorkRuntimeIntent::AcknowledgeInterruption {
                execution: receipt.execution,
            },
        },
    )
    .unwrap();
    let final_state = read_runtime(&mut hub, &planned);
    assert_eq!(
        final_state.executions[0].status,
        WorkExecutionStatus::Interrupted
    );
    assert_eq!(
        final_state.executions[0].attempts[0].status,
        WorkAttemptStatus::OutcomeUnknown
    );
    assert_eq!(final_state.executions[0].attempts[0].usage, None);
}
#[test]
fn lost_approval_ack_reconciles_same_receipt_without_renewing_budget() {
    let mut hub = Hub::in_memory().unwrap();
    hub.save(&session()).unwrap();
    let initial = create(&mut hub);
    let planned = edit(
        &mut hub,
        &initial,
        WorkEdit::ReplaceDraft { draft: draft() },
    )
    .unwrap();
    let request = WorkRequest::RuntimeCommand {
        id: planned.id,
        expected: planned.revision,
        command: 100.into(),
        intent: WorkRuntimeIntent::Approve {
            spec: spec(planned.plan.as_ref().unwrap()),
        },
    };
    super::super::tests::LOSE_COMMIT_ACK.with(|flag| flag.set(true));
    assert!(matches!(
        hub.work_document(planned.profile, request.clone()),
        Err(WorkError::OutcomeUnknown)
    ));
    let WorkReply::RuntimeCommand { projection, .. } =
        hub.work_document(planned.profile, request).unwrap()
    else {
        panic!()
    };
    assert_eq!(projection.executions.len(), 1);
    assert_eq!(projection.work.revision, planned.revision.next().unwrap());
}

#[test]
fn approved_deadline_uses_original_monotonic_store_incarnation() {
    let mut hub = Hub::in_memory().unwrap();
    hub.save(&session()).unwrap();
    let initial = create(&mut hub);
    let planned = edit(
        &mut hub,
        &initial,
        WorkEdit::ReplaceDraft { draft: draft() },
    )
    .unwrap();
    let (approved, receipt) = approve(&mut hub, &planned, 100.into());
    // Wall time has not elapsed. Only the original process's monotonic clock
    // decides admission; absolute timestamps remain descriptive history.
    let Some(epoch) = hub
        .work_runtime_epoch
        .checked_sub(std::time::Duration::from_secs(301))
    else {
        return;
    };
    hub.work_runtime_epoch = epoch;
    assert!(matches!(
        hub.work_document(
            planned.profile,
            WorkRequest::RuntimeUpdate {
                id: planned.id,
                expected: approved.work.revision,
                update: WorkRuntimeUpdate::Begin {
                    execution: receipt.execution,
                    attempt: 999.into(),
                    node: planned.plan.as_ref().unwrap().draft.nodes[0].id
                }
            }
        ),
        Err(WorkError::Conflict)
    ));
    assert!(read_runtime(&mut hub, &planned).executions[0]
        .attempts
        .is_empty());
}

fn direct_read_request(work: &WorkSnapshot, command: u128) -> WorkRequest {
    WorkRequest::RuntimeCommand {
        id: work.id,
        expected: work.revision,
        command: command.into(),
        intent: WorkRuntimeIntent::ReadPublic {
            scope: zephium_core::work::search::WorkPublicSearchScope {
                provider: zephium_core::work::search::WorkSearchProvider::OpenAi,
                model: zephium_core::work::search::PUBLIC_SEARCH_MODEL.into(),
                query: work.objective.clone(),
            },
            limits: WorkExecutionLimits {
                model_tokens: 147456,
                cost_micro_usd: 100000,
                operations: 1,
                timeout_seconds: 180,
                max_workers: 1,
            },
        },
    }
}
#[test]
fn direct_public_read_is_atomic_replayable_and_exactly_scoped() {
    let mut hub = Hub::in_memory().unwrap();
    hub.save(&session()).unwrap();
    let initial = create(&mut hub);
    // A failure after the internal plan write rolls the entire command back.
    hub.profile_conn(initial.profile).unwrap().execute_batch("CREATE TRIGGER fail_direct_execution BEFORE INSERT ON work_executions BEGIN SELECT RAISE(ABORT, 'test'); END;").unwrap();
    assert!(hub
        .work_document(initial.profile, direct_read_request(&initial, 700))
        .is_err());
    let unchanged = read_runtime(&mut hub, &initial);
    assert_eq!(unchanged.work.revision, initial.revision);
    assert!(unchanged.work.plan.is_none());
    assert!(unchanged.executions.is_empty());
    hub.profile_conn(initial.profile)
        .unwrap()
        .execute_batch("DROP TRIGGER fail_direct_execution;")
        .unwrap();
    assert!(hub
        .work_document(2.into(), direct_read_request(&initial, 700))
        .is_err());
    let WorkReply::PublicReadAdmitted {
        projection,
        receipt,
        replayed,
    } = hub
        .work_document(initial.profile, direct_read_request(&initial, 700))
        .unwrap()
    else {
        panic!()
    };
    assert!(!replayed);
    assert_eq!(projection.executions.len(), 1);
    assert!(projection.executions[0].attempts.is_empty());
    assert_eq!(
        projection.executions[0].authorization,
        WorkExecutionAuthorization::UserDirectedPublicRead
    );
    assert_eq!(
        projection.work.plan.as_ref().unwrap().draft.nodes[0].outputs[0].name,
        "Research findings"
    );
    let WorkReply::PublicReadAdmitted {
        receipt: again,
        replayed,
        ..
    } = hub
        .work_document(initial.profile, direct_read_request(&initial, 700))
        .unwrap()
    else {
        panic!()
    };
    assert!(replayed);
    assert_eq!(receipt, again);
    let mut changed = initial.clone();
    changed.objective.push('!');
    assert!(matches!(
        hub.work_document(initial.profile, direct_read_request(&changed, 700)),
        Err(WorkError::Conflict)
    ));
    assert!(matches!(
        hub.work_document(initial.profile, direct_read_request(&initial, 701)),
        Err(WorkError::Conflict)
    ));
    assert!(hub
        .work_document(initial.profile, direct_read_request(&projection.work, 701))
        .is_err());
    hub.work_document(
        initial.profile,
        WorkRequest::RuntimeCommand {
            id: initial.id,
            expected: projection.work.revision,
            command: 702.into(),
            intent: WorkRuntimeIntent::Cancel {
                execution: receipt.execution,
                intervention: None,
            },
        },
    )
    .unwrap();
    let WorkReply::PublicReadAdmitted {
        projection,
        replayed,
        ..
    } = hub
        .work_document(initial.profile, direct_read_request(&initial, 700))
        .unwrap()
    else {
        panic!()
    };
    assert!(replayed);
    assert_eq!(projection.executions.len(), 1);
    assert!(projection.executions[0].attempts.is_empty());
}

#[test]
fn direct_public_read_refuses_unanswered_clarification_and_changed_disclosure() {
    let mut hub = Hub::in_memory().unwrap();
    hub.save(&session()).unwrap();
    let initial = create(&mut hub);
    let mut changed = initial.clone();
    changed.objective.push('!');
    assert!(matches!(
        hub.work_document(initial.profile, direct_read_request(&changed, 800)),
        Err(WorkError::Invalid)
    ));
    let asked = edit(
        &mut hub,
        &initial,
        WorkEdit::OpenQuestion {
            id: 99.into(),
            prompt: "Which public release?".into(),
            options: vec![],
        },
    )
    .unwrap();
    assert!(matches!(
        hub.work_document(initial.profile, direct_read_request(&asked, 801)),
        Err(WorkError::Invalid)
    ));
    let state = read_runtime(&mut hub, &asked);
    assert!(state.executions.is_empty());
    assert_eq!(state.work, asked);
}
#[test]
fn takeover_persists_its_reason_and_success_cannot_carry_one() {
    let mut hub = Hub::in_memory().unwrap();
    hub.save(&session()).unwrap();
    let initial = create(&mut hub);
    let planned = edit(
        &mut hub,
        &initial,
        WorkEdit::ReplaceDraft { draft: draft() },
    )
    .unwrap();
    let (approved, receipt) = approve(&mut hub, &planned, 100.into());
    let attempt = WorkAttemptId::from(500);
    hub.work_document(
        planned.profile,
        WorkRequest::RuntimeUpdate {
            id: planned.id,
            expected: approved.work.revision,
            update: WorkRuntimeUpdate::Begin {
                execution: receipt.execution,
                attempt,
                node: planned.plan.as_ref().unwrap().draft.nodes[0].id,
            },
        },
    )
    .unwrap();
    let running = read_runtime(&mut hub, &planned);
    let cancel = |hub: &mut Hub, command: u128, intervention| {
        hub.work_document(
            planned.profile,
            WorkRequest::RuntimeCommand {
                id: planned.id,
                expected: running.work.revision,
                command: command.into(),
                intent: WorkRuntimeIntent::Cancel {
                    execution: receipt.execution,
                    intervention,
                },
            },
        )
    };
    assert!(matches!(
        cancel(
            &mut hub,
            101,
            Some(WorkInterventionV1 {
                kind: WorkInterventionKindV1::HumanTakeover,
                origin: Some("https://example.test/page".into()),
            })
        ),
        Err(WorkError::Invalid)
    ));
    cancel(
        &mut hub,
        102,
        Some(WorkInterventionV1 {
            kind: WorkInterventionKindV1::HumanTakeover,
            origin: Some("https://example.test".into()),
        }),
    )
    .unwrap();
    let taken = read_runtime(&mut hub, &planned);
    assert_eq!(taken.executions[0].status, WorkExecutionStatus::Cancelled);
    assert_eq!(
        taken.executions[0].intervention,
        Some(WorkInterventionV1 {
            kind: WorkInterventionKindV1::HumanTakeover,
            origin: Some("https://example.test".into()),
        })
    );
    let settle = |hub: &mut Hub, status, intervention| {
        let state = read_runtime(hub, &planned);
        hub.work_document(
            planned.profile,
            WorkRequest::RuntimeUpdate {
                id: planned.id,
                expected: state.work.revision,
                update: WorkRuntimeUpdate::Settle {
                    execution: receipt.execution,
                    attempt,
                    status,
                    usage: Some(WorkUsage::default()),
                    artifacts: vec![],
                    intervention,
                },
            },
        )
    };
    assert!(matches!(
        settle(
            &mut hub,
            WorkAttemptStatus::Succeeded,
            Some(WorkInterventionV1 {
                kind: WorkInterventionKindV1::SignIn,
                origin: None,
            })
        ),
        Err(WorkError::Conflict)
    ));
    assert!(matches!(
        settle(&mut hub, WorkAttemptStatus::Cancelled, None),
        Err(WorkError::Conflict)
    ));
    let settled = read_runtime(&mut hub, &planned);
    assert_eq!(
        settled.executions[0].intervention.as_ref().map(|i| i.kind),
        Some(WorkInterventionKindV1::HumanTakeover)
    );
}

#[test]
fn agent_admission_mints_the_plan_and_steps_commit_while_the_attempt_runs() {
    use zephium_core::work::search::*;
    let mut hub = Hub::in_memory().unwrap();
    hub.save(&session()).unwrap();
    let initial = create(&mut hub);
    let grant = WorkAgentGrantV1 {
        no_usage_charge: false,
        draft: None,
        lead: None,
        provider: WorkSearchProvider::OpenAi,
        model: PUBLIC_SEARCH_MODEL.into(),
        max_turns: 8,
        max_steps: 24,
        browse_hops: 4,
        folders: vec![],
        accounts: Vec::new(),
        private: false,
        skill: None,
        search_fallback: None,
    };
    let limits = WorkExecutionLimits {
        model_tokens: 600_000,
        cost_micro_usd: 1_000_000,
        operations: 64,
        timeout_seconds: 1200,
        max_workers: 2,
    };
    let begin = |hub: &mut Hub, command: u128, expected| {
        hub.work_document(
            initial.profile,
            WorkRequest::RuntimeCommand {
                id: initial.id,
                expected,
                command: command.into(),
                intent: WorkRuntimeIntent::BeginAgent {
                    grant: grant.clone(),
                    limits,
                },
            },
        )
    };
    let WorkReply::AgentAdmitted {
        projection,
        receipt,
        replayed: false,
    } = begin(&mut hub, 100, initial.revision).unwrap()
    else {
        panic!()
    };
    let execution = receipt.execution;
    let fact = &projection.executions[0];
    assert_eq!(
        fact.authorization,
        WorkExecutionAuthorization::UserDirectedAgent
    );
    assert!(fact.is_agent());
    assert_eq!(projection.work.plan.as_ref().unwrap().draft.nodes.len(), 1);
    assert!(matches!(
        begin(&mut hub, 100, initial.revision).unwrap(),
        WorkReply::AgentAdmitted { replayed: true, .. }
    ));
    assert!(matches!(
        begin(&mut hub, 101, projection.work.revision),
        Err(WorkError::Conflict)
    ));
    let node = projection.work.plan.as_ref().unwrap().draft.nodes[0].id;
    let attempt = WorkAttemptId::from(500);
    let update = |hub: &mut Hub, update| {
        let state = read_runtime(hub, &initial);
        hub.work_document(
            initial.profile,
            WorkRequest::RuntimeUpdate {
                id: initial.id,
                expected: state.work.revision,
                update,
            },
        )
    };
    let step = |id: u128, turn, kind, status| WorkStepFact {
        model_fault: None,
        part: None,
        id: id.into(),
        turn,
        kind,
        status,
        usage: None,
        artifacts: vec![],
        evidence: None,
        note: None,
        measurements: None,
        local: None,
        account: None,
    };
    // Steps need a live attempt.
    assert!(update(
        &mut hub,
        WorkRuntimeUpdate::BeginStep {
            execution,
            attempt,
            step: step(1, 1, WorkStepKindV1::Turn, WorkStepStatus::Succeeded),
            artifacts: vec![],
            evidence: None,
            file: None,
        }
    )
    .is_err());
    update(
        &mut hub,
        WorkRuntimeUpdate::Begin {
            execution,
            attempt,
            node,
        },
    )
    .unwrap();
    // Keep going grows the budget; it never shrinks it or changes its shape.
    let grown = WorkExecutionLimits {
        cost_micro_usd: limits.cost_micro_usd * 2,
        operations: limits.operations * 2,
        ..limits
    };
    for refused in [
        WorkExecutionLimits {
            operations: limits.operations - 1,
            ..grown
        },
        WorkExecutionLimits {
            timeout_seconds: limits.timeout_seconds + 1,
            ..grown
        },
        WorkExecutionLimits {
            cost_micro_usd: 20_000_000,
            ..grown
        },
    ] {
        assert!(update(
            &mut hub,
            WorkRuntimeUpdate::ExtendLimits {
                execution,
                attempt,
                limits: refused,
            }
        )
        .is_err());
    }
    update(
        &mut hub,
        WorkRuntimeUpdate::ExtendLimits {
            execution,
            attempt,
            limits: grown,
        },
    )
    .unwrap();
    let extended = read_runtime(&mut hub, &initial);
    assert_eq!(extended.executions[0].spec.limits, grown);
    assert_eq!(extended.executions[0].spec.nodes[0].limits.operations, 128);
    let usage = WorkUsage {
        model_tokens: 1200,
        cost_micro_usd: 300,
        operations: 1,
        accounting: WorkUsageAccounting::Exact,
    };
    let mut turn = step(1, 1, WorkStepKindV1::Turn, WorkStepStatus::Succeeded);
    turn.usage = Some(usage);
    turn.note = Some("Searching for options.".into());
    update(
        &mut hub,
        WorkRuntimeUpdate::BeginStep {
            execution,
            attempt,
            step: turn,
            artifacts: vec![],
            evidence: None,
            file: None,
        },
    )
    .unwrap();
    update(
        &mut hub,
        WorkRuntimeUpdate::BeginStep {
            execution,
            attempt,
            step: step(
                2,
                1,
                WorkStepKindV1::Search {
                    query: "canvas libraries".into(),
                },
                WorkStepStatus::Running,
            ),
            artifacts: vec![],
            evidence: None,
            file: None,
        },
    )
    .unwrap();
    let record = WorkProviderSearchRecordV1 {
        ranking: None,
        id: WorkArtifactId::from(40),
        node,
        attempt,
        evidence: WorkProviderSearchEvidenceV1 {
            version: 1,
            provider: WorkSearchProvider::OpenAi,
            model: PUBLIC_SEARCH_MODEL.into(),
            response_model: PUBLIC_SEARCH_MODEL.into(),
            response_id: "resp_1".into(),
            search_call_id: "ws_1".into(),
            answer: "Svelte Flow is a canvas library".into(),
            citations: vec![WorkProviderSearchCitation {
                url: "https://svelteflow.dev".into(),
                title: "Svelte Flow".into(),
                start_index: 0,
                end_index: 11,
                snippet: None,
            }],
            actual_input_tokens: 1000,
            actual_output_tokens: 200,
        },
    };
    let sources = artifact::WorkArtifactV1 {
        revises: None,
        part: None,
        version: 1,
        id: WorkArtifactId::from(41),
        execution,
        node,
        attempt,
        output: "Result".into(),
        title: "Sources".into(),
        data: artifact::WorkArtifactDataV1::EvidenceCollection {
            summary: "Svelte Flow is a canvas library".into(),
            subjects: vec![],
            entries: vec![],
        },
        evidence: vec![artifact::WorkEvidenceLink {
            extraction_id: record.id,
            source_id: 1,
        }],
        review: WorkOutputReview::SourceMappedNeedsReview,
        presentation: artifact::WorkArtifactPresentationV1::Automatic,
        general_knowledge: false,
    };
    // A settled search publishes its sources and record atomically.
    assert!(update(
        &mut hub,
        WorkRuntimeUpdate::SettleStep {
            execution,
            attempt,
            step: 2.into(),
            status: WorkStepStatus::Succeeded,
            usage: Some(usage),
            artifacts: vec![sources.clone()],
            evidence: None,
            file: None,
            note: None,
            measurements: None,
        }
    )
    .is_err());
    update(
        &mut hub,
        WorkRuntimeUpdate::SettleStep {
            execution,
            attempt,
            step: 2.into(),
            status: WorkStepStatus::Succeeded,
            usage: Some(usage),
            artifacts: vec![sources],
            evidence: Some(Box::new(record)),
            file: None,
            note: Some("Found 1 source".into()),
            measurements: None,
        },
    )
    .unwrap();
    let state = read_runtime(&mut hub, &initial);
    assert_eq!(state.executions[0].artifacts.len(), 1);
    assert_eq!(state.executions[0].steps[1].artifacts.len(), 1);
    assert_eq!(state.executions[0].status, WorkExecutionStatus::Running);
    // Questions are answered while the attempt runs, exactly once.
    update(
        &mut hub,
        WorkRuntimeUpdate::BeginStep {
            execution,
            attempt,
            step: step(
                3,
                2,
                WorkStepKindV1::Ask {
                    prompt: "Which budget?".into(),
                    options: vec!["Low".into(), "High".into()],
                    answer: None,
                    purpose: None,
                },
                WorkStepStatus::Running,
            ),
            artifacts: vec![],
            evidence: None,
            file: None,
        },
    )
    .unwrap();
    let answer = |hub: &mut Hub, command: u128| {
        let state = read_runtime(hub, &initial);
        hub.work_document(
            initial.profile,
            WorkRequest::RuntimeCommand {
                id: initial.id,
                expected: state.work.revision,
                command: command.into(),
                intent: WorkRuntimeIntent::AnswerStep {
                    execution,
                    step: 3.into(),
                    answer: "Low".into(),
                },
            },
        )
    };
    answer(&mut hub, 200).unwrap();
    assert!(matches!(answer(&mut hub, 201), Err(WorkError::Conflict)));
    let state = read_runtime(&mut hub, &initial);
    assert!(matches!(
        &state.executions[0].steps[2].kind,
        WorkStepKindV1::Ask { answer: Some(answer), .. } if answer == "Low"
    ));
    // A steer is a settled person step appended while the attempt runs.
    let steer = |hub: &mut Hub, command: u128| {
        let state = read_runtime(hub, &initial);
        hub.work_document(
            initial.profile,
            WorkRequest::RuntimeCommand {
                id: initial.id,
                expected: state.work.revision,
                command: command.into(),
                intent: WorkRuntimeIntent::Steer {
                    execution,
                    text: "Only official pricing pages".into(),
                },
            },
        )
    };
    steer(&mut hub, 210).unwrap();
    let state = read_runtime(&mut hub, &initial);
    assert!(matches!(
        &state.executions[0].steps[3],
        WorkStepFact { kind: WorkStepKindV1::Steer { text }, status: WorkStepStatus::Succeeded, usage: None, turn: 2, .. }
            if text == "Only official pricing pages"
    ));
    // A proposed change waits undecided, takes exactly one decision, and
    // settles with the file record it produced.
    update(
        &mut hub,
        WorkRuntimeUpdate::BeginStep {
            execution,
            attempt,
            step: step(
                7,
                2,
                WorkStepKindV1::WriteFile {
                    path: "/Users/me/Documents/project/notes.txt".into(),
                    content: "one\n".into(),
                    decision: None,
                },
                WorkStepStatus::Running,
            ),
            artifacts: vec![],
            evidence: None,
            file: None,
        },
    )
    .unwrap();
    let approve = |hub: &mut Hub, command: u128| {
        let state = read_runtime(hub, &initial);
        hub.work_document(
            initial.profile,
            WorkRequest::RuntimeCommand {
                id: initial.id,
                expected: state.work.revision,
                command: command.into(),
                intent: WorkRuntimeIntent::ApproveStep {
                    execution,
                    step: 7.into(),
                    approve: true,
                    for_run: false,
                },
            },
        )
    };
    approve(&mut hub, 220).unwrap();
    assert!(matches!(approve(&mut hub, 221), Err(WorkError::Conflict)));
    // A held site step takes one decision; only an offered allowance may be
    // accepted for the run.
    let confirm = |category, run_option| WorkStepKindV1::Confirm {
        confirm: Box::new(WorkConfirmV1 {
            site: "notion.so".into(),
            category,
            headline: "Save changes on notion.so?".into(),
            action: "type into Notes".into(),
            text: Some("Agenda".into()),
            facts: vec![],
            page: None,
            provenance: vec![],
            run_option,
            decision: None,
        }),
    };
    for (id, kind) in [
        (8, confirm(WorkConfirmCategoryV1::Edit, true)),
        (9, confirm(WorkConfirmCategoryV1::Communication, false)),
    ] {
        update(
            &mut hub,
            WorkRuntimeUpdate::BeginStep {
                execution,
                attempt,
                step: step(id, 2, kind, WorkStepStatus::Running),
                artifacts: vec![],
                evidence: None,
                file: None,
            },
        )
        .unwrap();
    }
    let decide = |hub: &mut Hub, command: u128, step: u128, approve: bool, for_run: bool| {
        let state = read_runtime(hub, &initial);
        hub.work_document(
            initial.profile,
            WorkRequest::RuntimeCommand {
                id: initial.id,
                expected: state.work.revision,
                command: command.into(),
                intent: WorkRuntimeIntent::ApproveStep {
                    execution,
                    step: step.into(),
                    approve,
                    for_run,
                },
            },
        )
    };
    assert!(matches!(
        decide(&mut hub, 222, 9, true, true),
        Err(WorkError::Invalid)
    ));
    assert!(matches!(
        decide(&mut hub, 223, 8, false, true),
        Err(WorkError::Invalid)
    ));
    decide(&mut hub, 224, 8, true, true).unwrap();
    assert!(matches!(
        decide(&mut hub, 225, 8, true, false),
        Err(WorkError::Conflict)
    ));
    decide(&mut hub, 226, 9, false, false).unwrap();
    let state = read_runtime(&mut hub, &initial);
    let decisions = state.executions[0]
        .steps
        .iter()
        .filter_map(|step| match &step.kind {
            WorkStepKindV1::Confirm { confirm } => confirm.decision,
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        decisions,
        [
            WorkConfirmDecisionV1::AllowedForRun,
            WorkConfirmDecisionV1::Declined
        ]
    );
    for (id, status) in [
        (8, WorkStepStatus::Succeeded),
        (9, WorkStepStatus::Cancelled),
    ] {
        update(
            &mut hub,
            WorkRuntimeUpdate::SettleStep {
                execution,
                attempt,
                step: id.into(),
                status,
                usage: None,
                artifacts: vec![],
                evidence: None,
                note: None,
                measurements: None,
                file: None,
            },
        )
        .unwrap();
    }
    let written = WorkFileRecordV1 {
        id: 71.into(),
        node,
        attempt,
        file: WorkFileEvidenceV1 {
            path: "/Users/me/Documents/project/notes.txt".into(),
            name: "notes.txt".into(),
            kind: WorkFileKindV1::Written,
            bytes: 4,
            digest: "a".repeat(64),
            text: "@@ -1,0 +1,1 @@\n+ one\n".into(),
            truncated: false,
            before_digest: None,
            after_digest: None,
            lines: None,
        },
    };
    update(
        &mut hub,
        WorkRuntimeUpdate::SettleStep {
            execution,
            attempt,
            step: 7.into(),
            status: WorkStepStatus::Succeeded,
            usage: None,
            artifacts: vec![],
            evidence: None,
            file: Some(Box::new(written.clone())),
            note: Some("Applied".into()),
            measurements: None,
        },
    )
    .unwrap();
    let state = read_runtime(&mut hub, &initial);
    assert_eq!(state.executions[0].file_evidence, vec![written]);
    assert!(matches!(
        &state.executions[0].steps[4],
        WorkStepFact { kind: WorkStepKindV1::WriteFile { decision: Some(true), .. }, status: WorkStepStatus::Succeeded, evidence: Some(id), .. } if *id == 71.into()
    ));
    // A public page title is projected only under a live read step.
    update(
        &mut hub,
        WorkRuntimeUpdate::BeginStep {
            execution,
            attempt,
            step: step(
                88,
                2,
                WorkStepKindV1::Read {
                    verbatim: None,
                    url: "https://example.com/page".into(),
                    collection: None,
                    goal: None,
                },
                WorkStepStatus::Running,
            ),
            artifacts: vec![],
            evidence: None,
            file: None,
        },
    )
    .unwrap();
    update(
        &mut hub,
        WorkRuntimeUpdate::PageTitle {
            execution,
            attempt,
            step: 88.into(),
            title: "Observed page title".into(),
        },
    )
    .unwrap();
    let state = read_runtime(&mut hub, &initial);
    assert_eq!(
        state.executions[0]
            .steps
            .iter()
            .find(|s| s.id == 88.into())
            .unwrap()
            .local
            .as_ref()
            .unwrap()
            .page_title
            .as_deref(),
        Some("Observed page title")
    );
    assert!(update(
        &mut hub,
        WorkRuntimeUpdate::PageTitle {
            execution,
            attempt,
            step: 1.into(),
            title: "Not a read".into(),
        }
    )
    .is_err());
    update(
        &mut hub,
        WorkRuntimeUpdate::SettleStep {
            execution,
            attempt,
            step: 88.into(),
            status: WorkStepStatus::Succeeded,
            usage: Some(WorkUsage {
                model_tokens: 0,
                cost_micro_usd: 0,
                operations: 0,
                accounting: WorkUsageAccounting::Exact,
            }),
            artifacts: vec![],
            evidence: None,
            file: None,
            note: None,
            measurements: None,
        },
    )
    .unwrap();
    assert!(update(
        &mut hub,
        WorkRuntimeUpdate::PageTitle {
            execution,
            attempt,
            step: 88.into(),
            title: "Too late".into(),
        }
    )
    .is_err());
    // Success needs the Finish step first.
    let settle = |hub: &mut Hub, status| {
        let state = read_runtime(hub, &initial);
        hub.work_document(
            initial.profile,
            WorkRequest::RuntimeUpdate {
                id: initial.id,
                expected: state.work.revision,
                update: WorkRuntimeUpdate::Settle {
                    execution,
                    attempt,
                    status,
                    usage: Some(usage),
                    artifacts: vec![],
                    intervention: None,
                },
            },
        )
    };
    assert!(settle(&mut hub, WorkAttemptStatus::Succeeded).is_err());
    update(
        &mut hub,
        WorkRuntimeUpdate::BeginStep {
            execution,
            attempt,
            step: step(
                4,
                3,
                WorkStepKindV1::Finish {
                    followups: vec![],
                    title: None,
                },
                WorkStepStatus::Succeeded,
            ),
            artifacts: vec![],
            evidence: None,
            file: None,
        },
    )
    .unwrap();
    settle(&mut hub, WorkAttemptStatus::Succeeded).unwrap();
    let state = read_runtime(&mut hub, &initial);
    assert_eq!(state.executions[0].status, WorkExecutionStatus::NeedsReview);
    assert_eq!(state.executions[0].steps.len(), 9);
    assert!(matches!(steer(&mut hub, 211), Err(WorkError::Conflict)));
}

#[test]
fn screenshot_records_never_inherit_or_modify_account_access_and_bind_exact_origins() {
    use zephium_core::work::sites::WorkSiteAccessV1;
    let mut hub = Hub::in_memory().unwrap();
    hub.save(&session()).unwrap();
    let initial = create(&mut hub);
    hub.work_document(
        initial.profile,
        WorkRequest::SiteAccess {
            set: Some(("slack.com".into(), Some(WorkSiteAccessV1::Always))),
        },
    )
    .unwrap();
    let WorkReply::ScreenshotAccess(empty) = hub
        .work_document(initial.profile, WorkRequest::ScreenshotAccess { set: None })
        .unwrap()
    else {
        panic!()
    };
    assert!(
        empty.is_empty(),
        "legacy Always never becomes screenshot consent"
    );
    for origin in ["https://app.slack.com", "https://slack.com"] {
        hub.work_document(
            initial.profile,
            WorkRequest::ScreenshotAccess {
                set: Some((origin.into(), true)),
            },
        )
        .unwrap();
    }
    let WorkReply::SiteAccess(entries) = hub
        .work_document(initial.profile, WorkRequest::SiteAccess { set: None })
        .unwrap()
    else {
        panic!()
    };
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].access, WorkSiteAccessV1::Always);
    hub.work_document(
        initial.profile,
        WorkRequest::SiteAccess {
            set: Some(("slack.com".into(), None)),
        },
    )
    .unwrap();
    let WorkReply::ScreenshotAccess(entries) = hub
        .work_document(
            initial.profile,
            WorkRequest::ScreenshotAccess {
                set: Some(("https://app.slack.com".into(), false)),
            },
        )
        .unwrap()
    else {
        panic!()
    };
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].origin, "https://slack.com");
    assert!(hub
        .work_document(
            initial.profile,
            WorkRequest::ScreenshotAccess {
                set: Some(("https://slack.com/client?token=x".into(), true))
            }
        )
        .is_err());
    assert!(hub
        .work_document(
            ProfileId::from(999_u128),
            WorkRequest::ScreenshotAccess { set: None }
        )
        .is_err());
}

#[test]
fn site_access_is_a_bounded_sorted_profile_list() {
    use zephium_core::work::sites::*;
    let mut hub = Hub::in_memory().unwrap();
    hub.save(&session()).unwrap();
    let initial = create(&mut hub);
    let mut call = |set| match hub
        .work_document(initial.profile, WorkRequest::SiteAccess { set })
        .unwrap()
    {
        WorkReply::SiteAccess(entries) => entries,
        _ => panic!(),
    };
    assert!(call(None).is_empty());
    call(Some(("slack.com".into(), Some(WorkSiteAccessV1::Always))));
    let listed = call(Some(("chase.com".into(), Some(WorkSiteAccessV1::Never))));
    assert_eq!(
        listed.iter().map(|e| e.site.as_str()).collect::<Vec<_>>(),
        ["chase.com", "slack.com"]
    );
    let changed = call(Some(("slack.com".into(), Some(WorkSiteAccessV1::Never))));
    assert!(changed.iter().all(|e| e.access == WorkSiteAccessV1::Never));
    let cleared = call(Some(("slack.com".into(), None)));
    assert_eq!(cleared.len(), 1);
    assert_eq!(call(None), cleared);
    assert!(hub
        .work_document(
            initial.profile,
            WorkRequest::SiteAccess {
                set: Some(("https://slack.com".into(), None))
            }
        )
        .is_err());
}

struct AgentSettlementFixture {
    hub: Hub,
    work: WorkSnapshot,
    execution: WorkExecutionId,
    attempt: WorkAttemptId,
    node: WorkPlanNodeId,
}
impl AgentSettlementFixture {
    fn new() -> Self {
        Self::with_pricing(Hub::in_memory().unwrap(), false)
    }
    fn with_pricing(mut hub: Hub, no_usage_charge: bool) -> Self {
        use zephium_core::work::search::*;
        hub.save(&session()).unwrap();
        let work = create(&mut hub);
        let WorkReply::AgentAdmitted {
            projection,
            receipt,
            ..
        } = hub
            .work_document(
                work.profile,
                WorkRequest::RuntimeCommand {
                    id: work.id,
                    expected: work.revision,
                    command: 100.into(),
                    intent: WorkRuntimeIntent::BeginAgent {
                        grant: WorkAgentGrantV1 {
                            no_usage_charge,
                            draft: None,
                            lead: no_usage_charge.then(|| {
                                zephium_core::work::model::WorkModelRef {
                                    provider:
                                        zephium_core::work::model::WorkModelProvider::Compatible,
                                    wire: zephium_core::work::model::WorkModelWire::ChatCompletions,
                                    model: "local-fixture".into(),
                                }
                            }),
                            provider: if no_usage_charge {
                                WorkSearchProvider::Browser
                            } else {
                                WorkSearchProvider::OpenAi
                            },
                            model: if no_usage_charge {
                                "duckduckgo".into()
                            } else {
                                PUBLIC_SEARCH_MODEL.into()
                            },
                            max_turns: 8,
                            max_steps: 24,
                            browse_hops: 4,
                            folders: vec![],
                            accounts: vec![],
                            private: false,
                            skill: None,
                            search_fallback: None,
                        },
                        limits: WorkExecutionLimits {
                            model_tokens: 100_000,
                            cost_micro_usd: 100_000,
                            operations: 32,
                            timeout_seconds: 120,
                            max_workers: 2,
                        },
                    },
                },
            )
            .unwrap()
        else {
            panic!("admitted agent")
        };
        let mut fixture = Self {
            hub,
            work,
            execution: receipt.execution,
            attempt: 500.into(),
            node: projection.work.plan.as_ref().unwrap().draft.nodes[0].id,
        };
        fixture
            .update(WorkRuntimeUpdate::Begin {
                execution: fixture.execution,
                attempt: fixture.attempt,
                node: fixture.node,
            })
            .unwrap();
        fixture
    }
    fn update(&mut self, update: WorkRuntimeUpdate) -> Result<WorkReply, WorkError> {
        let state = read_runtime(&mut self.hub, &self.work);
        self.hub.work_document(
            self.work.profile,
            WorkRequest::RuntimeUpdate {
                id: self.work.id,
                expected: state.work.revision,
                update,
            },
        )
    }
    fn step(
        &mut self,
        id: u128,
        kind: WorkStepKindV1,
        status: WorkStepStatus,
        artifacts: Vec<artifact::WorkArtifactV1>,
    ) -> Result<WorkReply, WorkError> {
        self.update(WorkRuntimeUpdate::BeginStep {
            execution: self.execution,
            attempt: self.attempt,
            step: WorkStepFact {
                model_fault: None,
                id: id.into(),
                turn: 1,
                kind,
                status,
                usage: None,
                artifacts: artifacts.iter().map(|a| a.id).collect(),
                evidence: None,
                note: None,
                measurements: None,
                local: None,
                account: None,
                part: None,
            },
            artifacts,
            evidence: None,
            file: None,
        })
    }
    fn publish(&mut self) {
        let reply = artifact::WorkArtifactV1 {
            version: 1,
            id: 900.into(),
            execution: self.execution,
            attempt: self.attempt,
            node: self.node,
            output: "Result".into(),
            title: "Exam plan".into(),
            data: artifact::WorkArtifactDataV1::Reply {
                headline: "Your study plan".into(),
                text: "Review the collected course material.".into(),
                figures: vec![],
                points: vec![],
            },
            review: WorkOutputReview::SourceMappedNeedsReview,
            presentation: artifact::WorkArtifactPresentationV1::Automatic,
            evidence: vec![],
            general_knowledge: true,
            part: None,
            revises: None,
        };
        self.step(
            10,
            WorkStepKindV1::Publish,
            WorkStepStatus::Succeeded,
            vec![reply],
        )
        .unwrap();
    }
    fn finish(&mut self) {
        self.step(
            11,
            WorkStepKindV1::Finish {
                followups: vec![],
                title: None,
            },
            WorkStepStatus::Succeeded,
            vec![],
        )
        .unwrap();
    }
    fn abandon(&mut self) {
        self.hub
            .work_document(
                self.work.profile,
                WorkRequest::RuntimeAbandon {
                    id: self.work.id,
                    execution: self.execution,
                    attempt: self.attempt,
                },
            )
            .unwrap();
    }
    fn fact(&mut self) -> WorkExecutionFact {
        read_runtime(&mut self.hub, &self.work).executions.remove(0)
    }
}

#[test]
fn page_settlement_revisions_are_store_owned_and_follow_commit_order() {
    let mut f = AgentSettlementFixture::new();
    for id in [1, 2] {
        f.step(
            id,
            WorkStepKindV1::Read {
                verbatim: None,
                url: "https://example.com/inbox".into(),
                goal: None,
                collection: None,
            },
            WorkStepStatus::Running,
            vec![],
        )
        .unwrap();
    }
    let before = read_runtime(&mut f.hub, &f.work);
    let mut forged = before.executions[0].steps[0].clone();
    forged.id = 3.into();
    forged.measurements = Some(WorkStepMeasurementsV1 {
        settled_revision: WorkRevision::new(999_999),
        ..Default::default()
    });
    assert!(
        matches!(
            f.update(WorkRuntimeUpdate::BeginStep {
                execution: f.execution,
                attempt: f.attempt,
                step: forged,
                artifacts: vec![],
                evidence: None,
                file: None,
            }),
            Err(WorkError::Invalid)
        ),
        "Begin cannot invent a settlement receipt"
    );
    let mut assigned = Vec::new();
    for id in [2u128, 1] {
        let expected = read_runtime(&mut f.hub, &f.work)
            .work
            .revision
            .next()
            .unwrap();
        f.update(WorkRuntimeUpdate::SettleStep {
            execution: f.execution,
            attempt: f.attempt,
            step: id.into(),
            status: WorkStepStatus::Failed,
            usage: Some(WorkUsage::default()),
            artifacts: vec![],
            evidence: None,
            file: None,
            note: None,
            measurements: Some(WorkStepMeasurementsV1 {
                outcome: Some(WorkPageOutcomeV1::NeedsCheck),
                settled_revision: WorkRevision::new(999_999),
                ..Default::default()
            }),
        })
        .unwrap();
        let measured = f
            .fact()
            .steps
            .into_iter()
            .find(|step| step.id == id.into())
            .unwrap()
            .measurements
            .unwrap();
        assert_eq!(measured.settled_revision, Some(expected));
        assigned.push(expected);
    }
    assert!(assigned[0] < assigned[1]);
    let snapshot = read_runtime(&mut f.hub, &f.work);
    let fact = &snapshot.executions[0];
    assert_eq!(fact.steps[0].id, 1.into(), "Begin order remains unchanged");
    assert!(
        fact.steps[0].measurements.unwrap().settled_revision
            > fact.steps[1].measurements.unwrap().settled_revision
    );
    for invalid in [
        fact.approved_revision,
        snapshot.work.revision.next().unwrap(),
    ] {
        let mut corrupted = fact.clone();
        corrupted.steps[0]
            .measurements
            .as_mut()
            .unwrap()
            .settled_revision = Some(invalid);
        assert_eq!(
            corrupted.validate(snapshot.work.plan.as_ref().unwrap(), snapshot.work.revision),
            Err(WorkError::Invalid)
        );
    }
}

#[test]
fn worker_contention_is_capacity_and_does_not_leave_an_unsent_step() {
    let mut f = AgentSettlementFixture::new();
    for id in [1, 2] {
        f.step(
            id,
            WorkStepKindV1::Search {
                query: format!("CS exam topic {id}"),
            },
            WorkStepStatus::Running,
            vec![],
        )
        .unwrap();
    }
    assert!(matches!(
        f.step(
            3,
            WorkStepKindV1::Search {
                query: "CS exam topic 3".into()
            },
            WorkStepStatus::Running,
            vec![]
        ),
        Err(WorkError::Capacity)
    ));
    assert_eq!(f.fact().steps.len(), 2);
    f.step(
        4,
        WorkStepKindV1::Ask {
            prompt: "Which exam?".into(),
            options: vec![],
            answer: None,
            purpose: Some(WorkAskPurposeV1::Question),
        },
        WorkStepStatus::Running,
        vec![],
    )
    .unwrap();
    f.abandon();
    let fact = f.fact();
    assert_eq!(fact.status, WorkExecutionStatus::Interrupted);
    assert!(fact
        .steps
        .iter()
        .all(|step| step.status != WorkStepStatus::Running));
}

#[test]
fn every_persisted_lead_stage_recovers_without_losing_the_published_answer() {
    for stage in 0..5 {
        let mut f = AgentSettlementFixture::new();
        if stage >= 1 {
            f.step(
                1,
                WorkStepKindV1::Search {
                    query: "CS exam topics".into(),
                },
                WorkStepStatus::Running,
                vec![],
            )
            .unwrap();
        }
        if stage >= 2 {
            f.publish();
        }
        if stage >= 3 {
            f.finish();
        }
        if stage == 4 {
            // A malformed final accounting write must not poison recovery.
            assert!(f
                .update(WorkRuntimeUpdate::Settle {
                    execution: f.execution,
                    attempt: f.attempt,
                    status: WorkAttemptStatus::Succeeded,
                    usage: None,
                    artifacts: vec![],
                    intervention: Some(WorkInterventionV1 {
                        kind: WorkInterventionKindV1::SignIn,
                        origin: None
                    })
                })
                .is_err());
        }
        f.abandon();
        let fact = f.fact();
        assert!(fact.status.terminal(), "stage={stage}");
        assert!(fact
            .steps
            .iter()
            .all(|step| step.status != WorkStepStatus::Running));
        assert_eq!(fact.artifacts.len(), usize::from(stage >= 2));
        if stage >= 3 {
            assert_eq!(fact.status, WorkExecutionStatus::NeedsReview);
            assert_eq!(
                fact.attempts[0].usage.unwrap().accounting,
                WorkUsageAccounting::ConservativeReservation
            );
            assert!(fact.steps.last().unwrap().note.is_some());
        } else {
            assert_eq!(fact.status, WorkExecutionStatus::Interrupted);
        }
    }
}

#[test]
fn a_published_reply_does_not_hide_an_unconfirmed_page_task() {
    for abandon in [false, true] {
        let mut f = AgentSettlementFixture::new();
        f.step(
            1,
            WorkStepKindV1::Read {
                verbatim: None,
                url: "https://example.com/".into(),
                goal: Some("Send the approved message".into()),
                collection: None,
            },
            WorkStepStatus::Running,
            vec![],
        )
        .unwrap();
        f.publish();
        f.finish();
        if abandon {
            f.abandon();
        } else {
            f.update(WorkRuntimeUpdate::Settle {
                execution: f.execution,
                attempt: f.attempt,
                status: WorkAttemptStatus::Succeeded,
                usage: Some(WorkUsage::default()),
                artifacts: vec![],
                intervention: None,
            })
            .unwrap();
        }
        let fact = f.fact();
        assert_eq!(fact.status, WorkExecutionStatus::Interrupted);
        assert_eq!(fact.steps[0].status, WorkStepStatus::OutcomeUnknown);
        assert!(fact.attempts[0].usage.is_none());
        assert_eq!(fact.artifacts.len(), 1);
    }
}

#[test]
fn lead_runs_keep_parts_inputs_and_linear_revisions_across_runs() {
    use zephium_core::work::{model::*, objects::*, parts::*, search::*};
    let mut hub = Hub::in_memory().unwrap();
    hub.save(&session()).unwrap();
    let initial = create(&mut hub);
    let grant = WorkAgentGrantV1 {
        no_usage_charge: false,
        draft: None,
        provider: WorkSearchProvider::OpenAi,
        model: PUBLIC_SEARCH_MODEL.into(),
        max_turns: 200,
        max_steps: 255,
        browse_hops: 4,
        folders: vec![],
        accounts: vec![],
        private: false,
        lead: Some(WorkModelRef {
            provider: WorkModelProvider::OpenAi,
            wire: WorkModelWire::OpenAiResponses,
            model: "gpt-6".into(),
        }),
        skill: None,
        search_fallback: None,
    };
    let limits = WorkExecutionLimits {
        model_tokens: 1_000_000,
        cost_micro_usd: 3_000_000,
        operations: 256,
        timeout_seconds: 1800,
        max_workers: 4,
    };
    let usage = WorkUsage {
        model_tokens: 1200,
        cost_micro_usd: 300,
        operations: 1,
        accounting: WorkUsageAccounting::Exact,
    };
    let update = |hub: &mut Hub, update| {
        let state = read_runtime(hub, &initial);
        hub.work_document(
            initial.profile,
            WorkRequest::RuntimeUpdate {
                id: initial.id,
                expected: state.work.revision,
                update,
            },
        )
    };
    let step = |id: u128, kind, status, part: Option<WorkPartId>| WorkStepFact {
        model_fault: None,
        id: id.into(),
        turn: 1,
        kind,
        status,
        usage: None,
        artifacts: vec![],
        evidence: None,
        note: None,
        measurements: None,
        local: None,
        account: None,
        part,
    };
    let reply = |headline: &str| artifact::WorkArtifactDataV1::Reply {
        headline: headline.into(),
        text: "Six nights near the YC office.".into(),
        figures: vec![],
        points: vec![],
    };
    let run = |hub: &mut Hub, command: u128, attempt: u128| {
        let state = read_runtime(hub, &initial);
        let WorkReply::AgentAdmitted {
            receipt,
            projection,
            ..
        } = hub
            .work_document(
                initial.profile,
                WorkRequest::RuntimeCommand {
                    id: initial.id,
                    expected: state.work.revision,
                    command: command.into(),
                    intent: WorkRuntimeIntent::BeginAgent {
                        grant: grant.clone(),
                        limits,
                    },
                },
            )
            .unwrap()
        else {
            panic!()
        };
        let node = projection
            .executions
            .iter()
            .find(|e| e.id == receipt.execution)
            .unwrap()
            .spec
            .nodes[0]
            .node;
        (receipt.execution, node, WorkAttemptId::from(attempt))
    };
    let (first, node, attempt) = run(&mut hub, 100, 500);
    update(
        &mut hub,
        WorkRuntimeUpdate::Begin {
            execution: first,
            attempt,
            node,
        },
    )
    .unwrap();
    let stay = WorkPartFactV1 {
        id: 60.into(),
        title: "Stay".into(),
        helper: WorkHelperV1::Browser,
        service: Some(WorkPartServiceV1 {
            host: Some("airbnb.com".into()),
            connection: None,
        }),
        goal: "Three homes near the YC office".into(),
        state: WorkPartStateV1::Running,
        started_ms: Some("1790000000000".into()),
        ended_ms: None,
        summary: None,
        need: None,
    };
    update(
        &mut hub,
        WorkRuntimeUpdate::Part {
            execution: first,
            attempt,
            part: stay.clone(),
        },
    )
    .unwrap();
    let skill = WorkInputFactV1 {
        kind: WorkInputKindV1::Skill,
        label: "Trip planning".into(),
        count: None,
        reference: Some("trip-planning".into()),
    };
    for _ in 0..2 {
        update(
            &mut hub,
            WorkRuntimeUpdate::Input {
                execution: first,
                attempt,
                input: skill.clone(),
            },
        )
        .unwrap();
    }
    let object = |id: u128, execution, (node, attempt), data, revises: Option<u128>, part| {
        artifact::WorkArtifactV1 {
            version: 1,
            id: id.into(),
            execution,
            node,
            attempt,
            output: "Result".into(),
            title: "Your trip".into(),
            data,
            evidence: vec![],
            review: WorkOutputReview::SourceMappedNeedsReview,
            presentation: artifact::WorkArtifactPresentationV1::Automatic,
            general_knowledge: true,
            revises: revises.map(Into::into),
            part,
        }
    };
    let mut publish = step(
        1,
        WorkStepKindV1::Publish,
        WorkStepStatus::Succeeded,
        Some(stay.id),
    );
    publish.artifacts = vec![10.into()];
    update(
        &mut hub,
        WorkRuntimeUpdate::BeginStep {
            execution: first,
            attempt,
            step: publish,
            artifacts: vec![object(
                10,
                first,
                (node, attempt),
                reply("Your trip"),
                None,
                Some(stay.id),
            )],
            evidence: None,
            file: None,
        },
    )
    .unwrap();
    let done = WorkPartFactV1 {
        state: WorkPartStateV1::Done,
        ended_ms: Some("1790000100000".into()),
        summary: Some("3 homes".into()),
        ..stay.clone()
    };
    update(
        &mut hub,
        WorkRuntimeUpdate::Part {
            execution: first,
            attempt,
            part: done.clone(),
        },
    )
    .unwrap();
    // A part that ended stays as it ended.
    assert!(matches!(
        update(
            &mut hub,
            WorkRuntimeUpdate::Part {
                execution: first,
                attempt,
                part: stay.clone()
            }
        ),
        Err(WorkError::Conflict)
    ));
    let mut finish = step(
        2,
        WorkStepKindV1::Finish {
            followups: vec![],
            title: None,
        },
        WorkStepStatus::Succeeded,
        None,
    );
    finish.note = Some("Your trip is ready".into());
    update(
        &mut hub,
        WorkRuntimeUpdate::BeginStep {
            execution: first,
            attempt,
            step: finish,
            artifacts: vec![],
            evidence: None,
            file: None,
        },
    )
    .unwrap();
    update(
        &mut hub,
        WorkRuntimeUpdate::Settle {
            execution: first,
            attempt,
            status: WorkAttemptStatus::Succeeded,
            usage: Some(usage),
            artifacts: vec![],
            intervention: None,
        },
    )
    .unwrap();
    let state = read_runtime(&mut hub, &initial);
    let settled = &state.executions[0];
    assert_eq!(settled.parts, vec![done]);
    assert_eq!(settled.inputs, vec![skill]);
    assert_eq!(settled.artifacts[0].part, Some(stay.id));

    // A follow-up revises the earlier object once, in its own kind.
    let (second, node, attempt) = run(&mut hub, 101, 501);
    update(
        &mut hub,
        WorkRuntimeUpdate::Begin {
            execution: second,
            attempt,
            node,
        },
    )
    .unwrap();
    let revise = |hub: &mut Hub, step_id: u128, id: u128, data, revises| {
        let mut publish = step(
            step_id,
            WorkStepKindV1::Publish,
            WorkStepStatus::Succeeded,
            None,
        );
        publish.artifacts = vec![id.into()];
        update(
            hub,
            WorkRuntimeUpdate::BeginStep {
                execution: second,
                attempt,
                step: publish,
                artifacts: vec![object(
                    id,
                    second,
                    (node, attempt),
                    data,
                    Some(revises),
                    None,
                )],
                evidence: None,
                file: None,
            },
        )
    };
    let plan = artifact::WorkArtifactDataV1::Plan {
        steps: vec![WorkPlanStepV1 {
            when: None,
            date: None,
            title: "Fly".into(),
            detail: None,
            kind: WorkPlanStepKindV1::Travel,
            cost: None,
            place: None,
            pick: None,
            source: None,
        }],
        total: None,
        checkable: false,
        calendar: false,
    };
    assert!(matches!(
        revise(&mut hub, 3, 20, plan, 10),
        Err(WorkError::Conflict)
    ));
    assert!(matches!(
        revise(&mut hub, 3, 20, reply("Cheaper"), 99),
        Err(WorkError::NotFound)
    ));
    revise(&mut hub, 3, 20, reply("Cheaper trip"), 10).unwrap();
    assert!(matches!(
        revise(&mut hub, 4, 21, reply("Cheapest trip"), 10),
        Err(WorkError::Conflict)
    ));
    revise(&mut hub, 4, 21, reply("Cheapest trip"), 20).unwrap();
    let state = read_runtime(&mut hub, &initial);
    let run = state.executions.iter().find(|e| e.id == second).unwrap();
    assert_eq!(
        run.artifacts.iter().map(|a| a.revises).collect::<Vec<_>>(),
        [Some(10.into()), Some(20.into())]
    );
}

#[test]
fn live_draft_edit_preserves_running_agent_and_exact_user_copy() {
    use zephium_core::resources::*;
    let mut f = AgentSettlementFixture::new();
    let draft_data = |body: &str| artifact::WorkArtifactDataV1::Draft {
        destination: zephium_core::work::objects::WorkDraftDestinationV1::Email,
        to: Some("Example colleague".into()),
        subject: Some("Synthetic draft".into()),
        body: body.into(),
        target_url: None,
    };
    let original = artifact::WorkArtifactV1 {
        version: 1,
        id: 901.into(),
        execution: f.execution,
        attempt: f.attempt,
        node: f.node,
        output: "Result".into(),
        title: "Synthetic draft".into(),
        data: draft_data("Original agent body"),
        review: WorkOutputReview::SourceMappedNeedsReview,
        presentation: artifact::WorkArtifactPresentationV1::Automatic,
        evidence: vec![],
        general_knowledge: true,
        part: None,
        revises: None,
    };
    f.step(
        10,
        WorkStepKindV1::Publish,
        WorkStepStatus::Succeeded,
        vec![original.clone()],
    )
    .unwrap();
    let before = read_runtime(&mut f.hub, &f.work);
    let edited = f
        .hub
        .work_document(
            f.work.profile,
            WorkRequest::RuntimeCommand {
                id: f.work.id,
                expected: before.work.revision,
                command: 700.into(),
                intent: WorkRuntimeIntent::EditArtifact {
                    execution: f.execution,
                    artifact: original.id,
                    data: draft_data("User-edited body"),
                    evidence: vec![],
                },
            },
        )
        .unwrap();
    let WorkReply::RuntimeCommand { projection, .. } = edited else {
        panic!("edited draft")
    };
    assert_eq!(
        projection.executions[0].status,
        WorkExecutionStatus::Running
    );
    assert_eq!(projection.executions[0].artifacts[0], original);
    assert_eq!(
        projection.executions[0].attempts,
        before.executions[0].attempts
    );
    let user = &projection.executions[0].user_artifacts[0];
    assert_eq!(user.edited_data, Some(draft_data("User-edited body")));
    let revision = user.revision;
    let copy = f.hub.resource_call(
        f.work.profile,
        ResourceCall::Mutate {
            command: Box::new(ResourceCommand {
                version: 1,
                request_id: "copy-live-draft-0001".into(),
                intent: ResourceIntent::PreserveArtifact {
                    objective: f.work.id,
                    execution: f.execution,
                    artifact: original.id,
                    basis: WorkObjectBasis::UserRevision { revision },
                    title: None,
                },
            }),
        },
    );
    let ResourceResponse::Applied { record, .. } = copy else {
        panic!("preserved user revision: {copy:?}")
    };
    let ResourceContent::Object { object } = record.draft.content else {
        panic!()
    };
    assert_eq!(object.data, draft_data("User-edited body"));
    assert_eq!(
        object.provenance.unwrap().basis,
        WorkObjectBasis::UserRevision { revision }
    );
    // A later stop remains legal and does not erase the user's durable draft.
    let current = read_runtime(&mut f.hub, &f.work);
    f.hub
        .work_document(
            f.work.profile,
            WorkRequest::RuntimeCommand {
                id: f.work.id,
                expected: current.work.revision,
                command: 701.into(),
                intent: WorkRuntimeIntent::Cancel {
                    execution: f.execution,
                    intervention: None,
                },
            },
        )
        .unwrap();
    let stopped = read_runtime(&mut f.hub, &f.work);
    assert_eq!(
        stopped.executions[0].user_artifacts[0].edited_data,
        Some(draft_data("User-edited body"))
    );
    assert_eq!(stopped.executions[0].artifacts[0], original);
}

#[test]
fn an_interrupted_block_edit_remains_unknown_effect_debt() {
    let mut f = AgentSettlementFixture::new();
    f.step(
        40,
        WorkStepKindV1::EditBlock {
            resource: "01M4K000000000000000000001".into(),
            expected_revision: "1".into(),
        },
        WorkStepStatus::Running,
        vec![],
    )
    .unwrap();
    let current = read_runtime(&mut f.hub, &f.work);
    f.hub
        .work_document(
            f.work.profile,
            WorkRequest::RuntimeCommand {
                id: f.work.id,
                expected: current.work.revision,
                command: 800.into(),
                intent: WorkRuntimeIntent::Cancel {
                    execution: f.execution,
                    intervention: None,
                },
            },
        )
        .unwrap();
    let stopped = read_runtime(&mut f.hub, &f.work);
    let step = stopped.executions[0]
        .steps
        .iter()
        .find(|s| s.id == WorkStepId::from(40))
        .unwrap();
    assert_eq!(step.status, WorkStepStatus::OutcomeUnknown);
    assert!(step.has_unconfirmed_effect());
}

fn image_disclosure_read(f: &mut AgentSettlementFixture) {
    f.step(
        1,
        WorkStepKindV1::Read {
            verbatim: None,
            url: "https://example.test/view".into(),
            goal: None,
            collection: None,
        },
        WorkStepStatus::Running,
        vec![],
    )
    .unwrap();
}
fn stop_image_fixture(f: &mut AgentSettlementFixture) {
    let current = read_runtime(&mut f.hub, &f.work);
    f.hub
        .work_document(
            f.work.profile,
            WorkRequest::RuntimeCommand {
                id: f.work.id,
                expected: current.work.revision,
                command: WorkCommandId::generate(),
                intent: WorkRuntimeIntent::Cancel {
                    execution: f.execution,
                    intervention: None,
                },
            },
        )
        .unwrap();
}
#[test]
fn image_disclosure_mark_is_durable_before_dispatch_and_survives_stop() {
    for mark_first in [false, true] {
        let mut f = AgentSettlementFixture::new();
        image_disclosure_read(&mut f);
        let mark = WorkRuntimeUpdate::MarkImageDisclosure {
            execution: f.execution,
            attempt: f.attempt,
            step: 1.into(),
            disclosure: WorkPageImageDisclosureV1::SiteConsent,
        };
        if mark_first {
            f.update(mark.clone()).unwrap();
        }
        stop_image_fixture(&mut f);
        assert!(matches!(f.update(mark), Err(WorkError::Conflict)));
        let fact = f.fact();
        let read = &fact.steps[0];
        assert_ne!(read.status, WorkStepStatus::Running);
        assert_eq!(
            read.measurements.and_then(|m| m.image_disclosure),
            mark_first.then_some(WorkPageImageDisclosureV1::SiteConsent)
        );
        assert!(
            read.measurements
                .is_none_or(|m| m.settled_revision.is_none()),
            "privacy marking is not a settlement receipt"
        );
    }
}
#[test]
fn image_disclosure_private_provenance_cannot_be_downgraded_by_public_or_missing_settlement() {
    for measurement in [
        None,
        Some(WorkStepMeasurementsV1 {
            image_disclosure: Some(WorkPageImageDisclosureV1::PublicIsolated),
            model_tokens: 123,
            ..Default::default()
        }),
    ] {
        let mut f = AgentSettlementFixture::new();
        image_disclosure_read(&mut f);
        for disclosure in [
            WorkPageImageDisclosureV1::SiteConsent,
            WorkPageImageDisclosureV1::PublicIsolated,
        ] {
            f.update(WorkRuntimeUpdate::MarkImageDisclosure {
                execution: f.execution,
                attempt: f.attempt,
                step: 1.into(),
                disclosure,
            })
            .unwrap();
        }
        f.update(WorkRuntimeUpdate::SettleStep {
            execution: f.execution,
            attempt: f.attempt,
            step: 1.into(),
            status: WorkStepStatus::Succeeded,
            usage: Some(WorkUsage::default()),
            artifacts: vec![],
            evidence: None,
            file: None,
            note: None,
            measurements: measurement,
        })
        .unwrap();
        let fact = f.fact();
        let measured = fact.steps[0].measurements.unwrap();
        assert_eq!(
            measured.image_disclosure,
            Some(WorkPageImageDisclosureV1::SiteConsent)
        );
        assert_eq!(
            measured.model_tokens,
            measurement.map_or(0, |m| m.model_tokens)
        );
        assert!(measured.settled_revision.is_some());
        assert!(matches!(
            f.update(WorkRuntimeUpdate::MarkImageDisclosure {
                execution: f.execution,
                attempt: f.attempt,
                step: 1.into(),
                disclosure: WorkPageImageDisclosureV1::PublicIsolated
            }),
            Err(WorkError::Conflict)
        ));
    }
}
#[test]
fn image_disclosure_mark_refuses_wrong_attempt_execution_profile_and_non_read() {
    let mut f = AgentSettlementFixture::new();
    image_disclosure_read(&mut f);
    let mark = |execution, attempt, step| WorkRuntimeUpdate::MarkImageDisclosure {
        execution,
        attempt,
        step,
        disclosure: WorkPageImageDisclosureV1::SiteConsent,
    };
    assert!(f.update(mark(f.execution, 999.into(), 1.into())).is_err());
    assert!(f.update(mark(999.into(), f.attempt, 1.into())).is_err());
    let current = read_runtime(&mut f.hub, &f.work);
    assert!(f
        .hub
        .work_document(
            999.into(),
            WorkRequest::RuntimeUpdate {
                id: f.work.id,
                expected: current.work.revision,
                update: mark(f.execution, f.attempt, 1.into())
            }
        )
        .is_err());
    f.step(
        2,
        WorkStepKindV1::Search {
            query: "public fact".into(),
        },
        WorkStepStatus::Running,
        vec![],
    )
    .unwrap();
    assert!(matches!(
        f.update(mark(f.execution, f.attempt, 2.into())),
        Err(WorkError::Invalid)
    ));
    assert!(f
        .fact()
        .steps
        .iter()
        .all(|step| step.measurements.is_none()));
}

#[test]
fn local_pricing_fact_survives_stop_missing_usage_abandonment_and_restart() {
    for local in [false, true] {
        for finish in ["cancel", "missing", "abandon", "restart"] {
            let directory = tempfile::tempdir().unwrap();
            let mut f = AgentSettlementFixture::with_pricing(
                Hub::open(directory.path().into()).unwrap(),
                local,
            );
            f.step(
                1,
                WorkStepKindV1::Search {
                    query: "fixture".into(),
                },
                WorkStepStatus::Running,
                vec![],
            )
            .unwrap();
            match finish {
                "abandon" => f.abandon(),
                "missing" => {
                    f.update(WorkRuntimeUpdate::Settle {
                        execution: f.execution,
                        attempt: f.attempt,
                        status: WorkAttemptStatus::Failed,
                        usage: None,
                        artifacts: vec![],
                        intervention: None,
                    })
                    .unwrap();
                }
                operation => {
                    if operation == "restart" {
                        drop(std::mem::replace(&mut f.hub, Hub::in_memory().unwrap()));
                        f.hub = Hub::open(directory.path().into()).unwrap();
                    }
                    let state = read_runtime(&mut f.hub, &f.work);
                    f.hub
                        .work_document(
                            f.work.profile,
                            WorkRequest::RuntimeCommand {
                                id: f.work.id,
                                expected: state.work.revision,
                                command: WorkCommandId::generate(),
                                intent: if operation == "restart" {
                                    WorkRuntimeIntent::AcknowledgeInterruption {
                                        execution: f.execution,
                                    }
                                } else {
                                    WorkRuntimeIntent::Cancel {
                                        execution: f.execution,
                                        intervention: None,
                                    }
                                },
                            },
                        )
                        .unwrap();
                }
            }
            let fact = f.fact();
            let decoded: WorkExecutionFact =
                serde_json::from_str(&serde_json::to_string(&fact).unwrap()).unwrap();
            assert_eq!(
                decoded.agent_grant().unwrap().no_usage_charge,
                local,
                "{finish}"
            );
            assert!(fact.status.terminal(), "{finish}");
            if local {
                let usage = fact.attempts[0].usage.expect(finish);
                assert_eq!(usage.cost_micro_usd, 0, "{finish}");
                assert_eq!(
                    usage.model_tokens, fact.spec.limits.model_tokens,
                    "{finish}"
                );
                assert_eq!(usage.operations, fact.spec.limits.operations, "{finish}");
                assert_eq!(
                    usage.accounting,
                    WorkUsageAccounting::ConservativeReservation
                );
            } else if finish == "missing" {
                assert_eq!(
                    fact.attempts[0].usage.unwrap().cost_micro_usd,
                    fact.spec.limits.cost_micro_usd
                );
            } else {
                assert!(
                    fact.attempts[0].usage.is_none(),
                    "paid unknown reservations remain unknown: {finish}"
                );
            }
        }
    }
}
