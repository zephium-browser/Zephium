use super::*;

#[test]
fn empty_local_evidence_does_not_prevent_the_next_agent_turn() {
    use super::{agent::*, artifact::*, runtime::*};
    let mut preview = WorkEvidencePreviewV1 {
        version: 1,
        link: WorkEvidenceLink {
            extraction_id: 40.into(),
            source_id: 1,
        },
        origin: "file:///Users/person/project".into(),
        role: "file".into(),
        text: String::new(),
        truncated: false,
        source_bytes: "0".into(),
        link_destination: None,
        source: WorkEvidenceSourceV1::File {
            path: "/Users/person/project".into(),
            name: "No search matches".into(),
            file_kind: WorkFileKindV1::Search,
        },
    };
    let disclose = |preview: &WorkEvidencePreviewV1| {
        WorkAgentTurnDisclosure::try_new(
            "Inspect this folder",
            vec![],
            vec![],
            &[],
            std::slice::from_ref(preview),
            &[],
            WorkAgentBudget {
                turns_left: 3,
                steps_left: 8,
                browse_available: true,
            },
            WorkExecutionLimits {
                model_tokens: 8000,
                cost_micro_usd: 10000,
                operations: 8,
                timeout_seconds: 60,
                max_workers: 1,
            },
            vec![],
        )
    };
    assert!(disclose(&preview).is_ok());
    preview.text = "\0".into();
    assert!(disclose(&preview).is_err());
    preview.text = "x".repeat(MAX_WORK_FILE_TEXT_BYTES + 1);
    assert!(disclose(&preview).is_err());
    preview.text.clear();
    preview.source = WorkEvidenceSourceV1::NativeExtraction;
    assert!(disclose(&preview).is_err());
}

#[test]
fn native_link_handoff_admits_exact_observed_targets_without_promoting_prose() {
    use super::{agent::*, artifact::*, runtime::*};
    let target = "https://shop.example.test/products/42?variant=blue";
    let mut preview = WorkEvidencePreviewV1 {
        version: 1,
        link: WorkEvidenceLink {
            extraction_id: 40.into(),
            source_id: 1,
        },
        origin: "https://shop.example.test/".into(),
        role: "link".into(),
        text: target.into(),
        truncated: false,
        source_bytes: target.len().to_string(),
        link_destination: Some(target.into()),
        source: WorkEvidenceSourceV1::NativeExtraction,
    };
    let disclose = |preview: &WorkEvidencePreviewV1| {
        WorkAgentTurnDisclosure::try_new(
            "Inspect the selected product",
            vec![],
            vec![],
            &[],
            std::slice::from_ref(preview),
            &[],
            WorkAgentBudget {
                turns_left: 3,
                steps_left: 8,
                browse_available: true,
            },
            WorkExecutionLimits {
                model_tokens: 8000,
                cost_micro_usd: 10000,
                operations: 8,
                timeout_seconds: 60,
                max_workers: 1,
            },
            vec![],
        )
    };
    let read = |url: &str| WorkAgentTurnOutput {
        say: None,
        artifacts: vec![],
        ask: None,
        finish: false,
        followups: vec![],
        malformed: 0,
        fetch: vec![WorkAgentFetch::Read {
            url: url.into(),
            collection: None,
        }],
    };
    let disclosed = disclose(&preview).unwrap();
    assert_eq!(disclosed.context().sources[0].url, preview.origin);
    assert_eq!(
        disclosed.context().sources[0].link_destination.as_deref(),
        Some(target)
    );
    assert!(disclosed.resolve(read(target)).is_ok());
    // An unlisted page is dropped with a notice; the turn itself stands.
    let unlisted = disclosed
        .resolve(read("https://shop.example.test/products/42"))
        .unwrap();
    assert!(unlisted.fetch.is_empty() && unlisted.notices.len() == 1);
    assert!(disclosed
        .resolve(read(&format!("{target}/#details")))
        .is_ok_and(|turn| turn.fetch.len() == 1));
    preview.truncated = true;
    assert!(disclose(&preview).is_err());
    preview.truncated = false;
    preview.role = "image".into();
    assert!(disclose(&preview).is_err());
    preview.link_destination = None;
    preview.role = "paragraph".into();
    assert!(disclose(&preview)
        .unwrap()
        .resolve(read(target))
        .is_ok_and(|turn| turn.fetch.is_empty()));
    let legacy = serde_json::to_value(&preview).unwrap();
    assert!(legacy.get("link_destination").is_none());
    assert_eq!(
        serde_json::from_value::<WorkEvidencePreviewV1>(legacy).unwrap(),
        preview
    );
}

#[test]
fn coordination_rejects_combined_completion_deadlocks_and_scope_widening() {
    use super::runtime::*;
    let mut graph = draft();
    graph.nodes = (1..=4)
        .map(|id| {
            let mut node = graph.nodes[0].clone();
            node.id = id.into();
            node
        })
        .collect();
    let planned = work()
        .apply(
            WorkRevision::INITIAL,
            WorkEdit::ReplaceDraft { draft: graph },
            WorkAuthor::User,
        )
        .unwrap()
        .0;
    let mut plan = planned.plan.unwrap();
    let limits = WorkExecutionLimits {
        model_tokens: 1000,
        cost_micro_usd: 1000,
        operations: 2,
        timeout_seconds: 60,
        max_workers: 2,
    };
    let scope = WorkBrowseScope {
        start_url: "https://example.test/".into(),
        routes: vec![WorkBrowseRoute {
            origin: "https://example.test".into(),
            path_prefix: "/".into(),
        }],
        max_hops: 1,
    };
    let mut spec = WorkExecutionSpec {
        request: None,
        context: None,
        plan_revision: plan.revision,
        limits: WorkExecutionLimits {
            model_tokens: 4000,
            cost_micro_usd: 4000,
            operations: 8,
            ..limits
        },
        nodes: (1..=4)
            .map(|id| WorkNodeExecutionSpec {
                node: id.into(),
                parent: match id {
                    2 => Some(1.into()),
                    4 => Some(3.into()),
                    _ => None,
                },
                capability: WorkCapability::Coordinate {
                    scope: scope.clone(),
                },
                limits,
            })
            .collect(),
    };
    assert!(spec.validate(&plan).is_ok());
    plan.draft.nodes[1].dependencies = vec![3.into()];
    plan.draft.nodes[3].dependencies = vec![1.into()];
    assert!(plan.draft.validate().is_ok());
    assert_eq!(spec.validate(&plan), Err(WorkError::Invalid));
    plan.draft.nodes[1].dependencies.clear();
    plan.draft.nodes[3].dependencies.clear();
    spec.nodes[1].capability = WorkCapability::PublicBrowse {
        scope: WorkBrowseScope {
            max_hops: 2,
            ..scope
        },
    };
    assert_eq!(spec.validate(&plan), Err(WorkError::Invalid));
}

fn draft() -> WorkPlanDraft {
    WorkPlanDraft {
        id: 2.into(),
        nodes: vec![WorkPlanNode {
            id: 3.into(),
            objective: "Compare sources".into(),
            dependencies: vec![],
            outputs: vec![WorkExpectedOutput {
                name: "comparison".into(),
                description: "Source-bound comparison with uncertainties".into(),
                review: WorkOutputReview::SourceMappedNeedsReview,
            }],
        }],
    }
}
fn work() -> WorkSnapshot {
    WorkSnapshot::create(1.into(), 4.into(), "Understand the alternatives".into()).unwrap()
}

#[test]
fn execution_proposal_preserves_plan_and_bounds_public_browser_responsibilities() {
    use super::{execution_proposal::*, runtime::*};
    let plan = work()
        .apply(
            WorkRevision::INITIAL,
            WorkEdit::ReplaceDraft { draft: draft() },
            WorkAuthor::User,
        )
        .unwrap()
        .0
        .plan
        .unwrap();
    let limits = WorkExecutionLimits {
        model_tokens: 256_000,
        cost_micro_usd: 1_000_000,
        operations: 256,
        timeout_seconds: 900,
        max_workers: 4,
    };
    let proposal = WorkExecutionProposal {
        nodes: vec![WorkResponsibilityProposal {
            key: 0,
            parent: None,
            capability: WorkCapabilityProposal::PublicDiscovery {
                search_query: "Svelte graph performance".into(),
            },
        }],
    };
    let spec = proposal.clone().compile(&plan, limits).unwrap();
    assert_eq!(spec.plan_revision, plan.revision);
    assert_eq!(spec.nodes[0].node, plan.draft.nodes[0].id);
    assert_eq!(spec.nodes[0].limits.timeout_seconds, 600);
    assert!(
        matches!(&spec.nodes[0].capability, WorkCapability::PublicDiscovery { scope } if scope.max_hops == WORK_PUBLIC_DISCOVERY_MAX_HOPS && scope.search_query == "Svelte graph performance")
    );
    for invalid in 0..3 {
        let mut changed = proposal.clone();
        match invalid {
            0 => changed.nodes[0].key = 1,
            1 => changed.nodes[0].parent = Some(0),
            _ => changed.nodes.push(changed.nodes[0].clone()),
        }
        let reason = changed
            .clone()
            .compile_diagnosed(&plan, limits)
            .err()
            .unwrap();
        assert_eq!(
            reason,
            match invalid {
                0 => WorkExecutionProposalRefusal::UnknownKey { key: 1 },
                1 => WorkExecutionProposalRefusal::DelegationTopology,
                _ => WorkExecutionProposalRefusal::NodeCount {
                    expected: 1,
                    actual: 2
                },
            }
        );
        assert!(changed.compile(&plan, limits).is_err());
    }
    let mut wrong_review = plan.clone();
    wrong_review.draft.nodes[0].outputs[0].review = WorkOutputReview::UserAcceptance;
    assert_eq!(
        proposal
            .clone()
            .compile_diagnosed(&wrong_review, limits)
            .err(),
        Some(WorkExecutionProposalRefusal::OutputReview { key: 0 })
    );
    let mut invalid_parent = proposal.clone();
    invalid_parent.nodes[0].parent = Some(4);
    assert_eq!(
        invalid_parent.compile_diagnosed(&plan, limits).err(),
        Some(WorkExecutionProposalRefusal::UnknownParent { key: 0, parent: 4 })
    );
    let mut invalid_query = proposal;
    invalid_query.nodes[0].capability = WorkCapabilityProposal::PublicDiscovery {
        search_query: "".into(),
    };
    assert_eq!(
        invalid_query.compile_diagnosed(&plan, limits).err(),
        Some(WorkExecutionProposalRefusal::Capability { key: 0 })
    );
}
#[test]
fn work_authoring_questions_invalidate_plans_and_stale_edits_leave_facts_unchanged() {
    let original = work();
    let (planned, _) = original
        .apply(
            original.revision,
            WorkEdit::ReplaceDraft { draft: draft() },
            WorkAuthor::User,
        )
        .unwrap();
    assert_eq!(planned.status, WorkAuthoringStatus::PlanReady);
    assert_eq!(
        planned.plan.as_ref().unwrap().basis_revision,
        original.revision
    );
    let (questioned, _) = planned
        .apply(
            planned.revision,
            WorkEdit::OpenQuestion {
                id: 9.into(),
                prompt: "Which constraints matter?".into(),
                options: vec![],
            },
            WorkAuthor::User,
        )
        .unwrap();
    assert!(questioned.plan.is_none());
    assert_eq!(questioned.status, WorkAuthoringStatus::NeedsInput);
    assert_eq!(
        questioned.apply(
            planned.revision,
            WorkEdit::ReplaceDraft { draft: draft() },
            WorkAuthor::User
        ),
        Err(WorkError::Conflict)
    );
    assert_eq!(
        questioned.apply(
            questioned.revision,
            WorkEdit::ReplaceDraft { draft: draft() },
            WorkAuthor::User
        ),
        Err(WorkError::Conflict)
    );
    let (answered, _) = questioned
        .apply(
            questioned.revision,
            WorkEdit::AnswerQuestion {
                id: 9.into(),
                answer: "Local processing".into(),
            },
            WorkAuthor::User,
        )
        .unwrap();
    assert_eq!(answered.status, WorkAuthoringStatus::Draft);
    assert!(answered.plan.is_none());
    assert!(questioned.questions[0].answer.is_none());
}
#[test]
fn work_authoring_graph_rejects_cycles_duplicates_dangling_edges_and_unbounded_outputs() {
    let first = draft().nodes.remove(0);
    let mut second = first.clone();
    second.id = 5.into();
    second.dependencies = vec![first.id];
    let mut graph = WorkPlanDraft {
        id: 2.into(),
        nodes: vec![second, first],
    };
    assert!(graph.validate().is_ok()); // view order is not execution order
    graph.nodes[1].dependencies = vec![5.into()];
    assert_eq!(graph.validate(), Err(WorkError::Invalid));
    graph.nodes[1].dependencies = vec![99.into()];
    assert_eq!(graph.validate(), Err(WorkError::Invalid));
    graph.nodes[1].dependencies.clear();
    graph.nodes[1].id = 5.into();
    assert_eq!(graph.validate(), Err(WorkError::Invalid));
    let mut oversized = draft();
    oversized.nodes[0].outputs = vec![oversized.nodes[0].outputs[0].clone(); 9];
    assert_eq!(oversized.validate(), Err(WorkError::Invalid));
}
#[test]
fn work_authoring_refuses_invalid_and_exhausted_values_without_wrapping() {
    assert!(WorkSnapshot::create(1.into(), 2.into(), " \n".into()).is_err());
    assert!(WorkSnapshot::create(1.into(), 2.into(), "x".repeat(MAX_WORK_TEXT_BYTES + 1)).is_err());
    assert_eq!(
        WorkRevision::new(i64::MAX as u64).unwrap().next(),
        Err(WorkError::Capacity)
    );
    assert!(WorkRevision::new(0).is_none());
    let mut invalid = work();
    invalid.schema_version = WORK_SCHEMA_VERSION + 1;
    assert_eq!(invalid.validate(), Err(WorkError::Invalid));
    let (mut invalid, _) = work()
        .apply(
            WorkRevision::INITIAL,
            WorkEdit::ReplaceDraft { draft: draft() },
            WorkAuthor::User,
        )
        .unwrap();
    invalid.context_revision = invalid.revision;
    assert_eq!(invalid.validate(), Err(WorkError::Invalid));
}
#[test]
fn work_identity_preserves_canonical_wire_and_debug_redaction() {
    let id = WorkId::from(0xabcd);
    assert_eq!(WorkId::parse(&id.to_string()), Some(id));
    assert!(WorkId::parse(&id.to_string().to_lowercase()).is_none());
    assert_eq!(id.bytes(), 0xabcd_u128.to_be_bytes());
    assert!(!format!("{:?}", work()).contains("alternatives"));
    assert!(!format!(
        "{:?}",
        port::WorkRequest::Create {
            author: WorkAuthor::User,
            id,
            objective: "private".into()
        }
    )
    .contains("private"));
}

#[test]
fn work_authoring_supersedes_context_and_preserves_attribution_without_reactivating_answers() {
    let first = work();
    let (asked, _) = first
        .apply(
            first.revision,
            WorkEdit::OpenQuestion {
                id: 90.into(),
                prompt: "Travel budget?".into(),
                options: vec![],
            },
            WorkAuthor::PrimaryAgent,
        )
        .unwrap();
    assert_eq!(asked.questions[0].basis_revision, Some(first.revision));
    let (answered, _) = asked
        .apply(
            asked.revision,
            WorkEdit::AnswerQuestion {
                id: 90.into(),
                answer: "2000".into(),
            },
            WorkAuthor::User,
        )
        .unwrap();
    let (changed, _) = answered
        .apply(
            answered.revision,
            WorkEdit::SetObjective {
                objective: "Research database indexes".into(),
            },
            WorkAuthor::User,
        )
        .unwrap();
    assert_eq!(changed.status, WorkAuthoringStatus::Draft);
    assert_eq!(changed.current_questions().count(), 0);
    assert_eq!(changed.questions[0].state, WorkQuestionState::Superseded);
    assert_eq!(changed.questions[0].answer.as_deref(), Some("2000"));
    assert_eq!(changed.questions[0].author, WorkAuthor::PrimaryAgent);
    assert_eq!(changed.questions[0].answer_author, Some(WorkAuthor::User));
    assert_eq!(
        changed.apply(
            changed.revision,
            WorkEdit::AnswerQuestion {
                id: 90.into(),
                answer: "Stale response".into(),
            },
            WorkAuthor::User
        ),
        Err(WorkError::Conflict)
    );
    let (fresh, _) = changed
        .apply(
            changed.revision,
            WorkEdit::OpenQuestion {
                id: 91.into(),
                prompt: "Which database?".into(),
                options: vec![],
            },
            WorkAuthor::PrimaryAgent,
        )
        .unwrap();
    let (dismissed, _) = fresh
        .apply(
            fresh.revision,
            WorkEdit::DismissQuestion { id: 91.into() },
            WorkAuthor::User,
        )
        .unwrap();
    let (planned, _) = dismissed
        .apply(
            dismissed.revision,
            WorkEdit::ReplaceDraft { draft: draft() },
            WorkAuthor::PrimaryAgent,
        )
        .unwrap();
    assert_eq!(
        planned.plan.as_ref().unwrap().author,
        WorkAuthor::PrimaryAgent
    );
    let (archived, _) = planned
        .apply(planned.revision, WorkEdit::Archive, WorkAuthor::User)
        .unwrap();
    assert_eq!(archived.plan, planned.plan);
    assert_eq!(
        archived.apply(
            archived.revision,
            WorkEdit::SetObjective {
                objective: "Cannot edit archive".into()
            },
            WorkAuthor::User
        ),
        Err(WorkError::Conflict)
    );
    let (compacted, _) = archived
        .apply(
            archived.revision,
            WorkEdit::CompactHistory,
            WorkAuthor::User,
        )
        .unwrap();
    assert!(compacted.questions.is_empty());
    assert_eq!(compacted.plan, planned.plan);
    assert!(compacted
        .apply(compacted.revision, WorkEdit::Restore, WorkAuthor::User)
        .is_ok());
}

#[test]
fn work_proposals_have_temporary_keys_and_cannot_smuggle_durable_identity_or_authority() {
    use proposal::*;
    let mut proposed = WorkPlanProposal {
        nodes: vec![WorkNodeProposal {
            key: 1,
            objective: "Read evidence".into(),
            dependencies: vec![],
            outputs: draft().nodes.remove(0).outputs,
        }],
    };
    let one = proposed.mint().unwrap();
    let two = proposed.mint().unwrap();
    assert_ne!(one.id, two.id);
    assert_ne!(one.nodes[0].id, two.nodes[0].id);
    proposed.nodes[0].dependencies = vec![2];
    assert_eq!(proposed.validate(), Err(WorkError::Invalid));
    proposed.nodes[0].dependencies = vec![1];
    assert_eq!(proposed.validate(), Err(WorkError::Invalid));
    proposed.nodes[0].dependencies.clear();
    let mut wire = serde_json::to_value(proposed).unwrap();
    wire["author"] = serde_json::json!("user");
    assert!(serde_json::from_value::<WorkPlanProposal>(wire).is_err());
}

#[test]
fn account_scoped_specs_bind_one_attested_page_and_a_single_step_plan() {
    use super::runtime::*;
    let plan = WorkPlanRevision {
        context: None,
        author: WorkAuthor::User,
        revision: WorkRevision::INITIAL,
        basis_revision: WorkRevision::INITIAL,
        draft: draft(),
    };
    let limits = WorkExecutionLimits {
        model_tokens: 128_000,
        cost_micro_usd: 500_000,
        operations: 64,
        timeout_seconds: 600,
        max_workers: 4,
    };
    let scope = WorkAccountScope {
        tab: crate::ids::ItemId::generate(),
        url: "https://app.notion.com/p/Sprint-42".into(),
        origin: "https://app.notion.com".into(),
        account: "01JACCOUNT0000000000000000".into(),
    };
    let read = WorkExecutionSpec::account_scoped(
        &plan,
        limits,
        WorkCapability::AccountRead {
            scope: scope.clone(),
        },
    )
    .unwrap();
    assert_eq!(read.nodes.len(), 1);
    assert_eq!(read.nodes[0].limits.max_workers, 1);
    let update = WorkFieldUpdateV1 {
        field: Some("Title".into()),
        from: "Sprint 42".into(),
        to: "Sprint 42 (probe)".into(),
    };
    WorkExecutionSpec::account_scoped(
        &plan,
        limits,
        WorkCapability::AccountUpdate {
            scope: scope.clone(),
            update: update.clone(),
        },
    )
    .unwrap();
    for refused in [
        WorkCapability::AccountRead {
            scope: WorkAccountScope {
                origin: "https://www.notion.so".into(),
                ..scope.clone()
            },
        },
        WorkCapability::AccountRead {
            scope: WorkAccountScope {
                url: "http://app.notion.com/p/Sprint-42".into(),
                origin: "http://app.notion.com".into(),
                ..scope.clone()
            },
        },
        WorkCapability::AccountRead {
            scope: WorkAccountScope {
                account: "not an id".into(),
                ..scope.clone()
            },
        },
        WorkCapability::AccountUpdate {
            scope: scope.clone(),
            update: WorkFieldUpdateV1 {
                to: "Sprint 42".into(),
                ..update.clone()
            },
        },
        WorkCapability::AccountUpdate {
            scope: scope.clone(),
            update: WorkFieldUpdateV1 {
                from: " ".into(),
                ..update.clone()
            },
        },
        WorkCapability::PublicBrowse {
            scope: WorkBrowseScope {
                start_url: "https://app.notion.com/p/Sprint-42".into(),
                routes: vec![WorkBrowseRoute {
                    origin: "https://app.notion.com".into(),
                    path_prefix: "/".into(),
                }],
                max_hops: 1,
            },
        },
    ] {
        assert!(WorkExecutionSpec::account_scoped(&plan, limits, refused).is_err());
    }
    let mut two_steps = plan.clone();
    let mut second = two_steps.draft.nodes[0].clone();
    second.id = WorkPlanNodeId::generate();
    two_steps.draft.nodes.push(second);
    assert!(WorkExecutionSpec::account_scoped(
        &two_steps,
        limits,
        WorkCapability::AccountRead { scope },
    )
    .is_err());
    assert!(WorkInterventionV1 {
        kind: WorkInterventionKindV1::SignIn,
        origin: Some("https://app.notion.com/p".into()),
    }
    .validate()
    .is_err());
}

#[test]
fn agent_executions_commit_steps_incrementally_and_finish_explicitly() {
    use super::{agent::*, artifact::*, runtime::*, search::*};
    let plan = WorkPlanRevision {
        context: None,
        author: WorkAuthor::User,
        revision: WorkRevision::new(2).unwrap(),
        basis_revision: WorkRevision::INITIAL,
        draft: draft(),
    };
    let limits = WorkExecutionLimits {
        model_tokens: 600_000,
        cost_micro_usd: 1_000_000,
        operations: 64,
        timeout_seconds: 1200,
        max_workers: 2,
    };
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
    assert!(WorkAgentGrantV1 {
        no_usage_charge: false,
        max_turns: 0,
        ..grant.clone()
    }
    .validate()
    .is_err());
    // Only a browser search names a fallback, whose evidence then belongs too.
    let fallback = WorkSearchFallbackV1 {
        provider: WorkSearchProvider::Native,
        model: "anthropic/claude-haiku-4-5-20251001".into(),
    };
    let browser = WorkAgentGrantV1 {
        no_usage_charge: false,
        provider: WorkSearchProvider::Browser,
        model: "bing".into(),
        search_fallback: Some(fallback.clone()),
        ..grant.clone()
    };
    assert!(browser.validate().is_ok());
    assert!(browser.admits_search(WorkSearchProvider::Browser, "bing"));
    assert!(browser.admits_search(WorkSearchProvider::Native, &fallback.model));
    assert!(!browser.admits_search(WorkSearchProvider::Browser, "google"));
    assert!(!browser.admits_search(WorkSearchProvider::OpenAi, PUBLIC_SEARCH_MODEL));
    assert!(!grant.admits_search(WorkSearchProvider::Native, &fallback.model));
    assert!(WorkAgentGrantV1 {
        no_usage_charge: false,
        search_fallback: Some(fallback),
        ..grant.clone()
    }
    .validate()
    .is_err());
    let spec = WorkExecutionSpec::agent(&plan, limits, grant.clone()).unwrap();
    let node = plan.draft.nodes[0].id;
    let attempt = WorkAttemptId::from(500);
    let revision = WorkRevision::new(9).unwrap();
    let mut fact = WorkExecutionFact {
        parts: vec![],
        inputs: vec![],
        authorization: WorkExecutionAuthorization::UserDirectedAgent,
        id: WorkExecutionId::from(7),
        approved_revision: WorkRevision::new(3).unwrap(),
        spec,
        status: WorkExecutionStatus::Approved,
        attempts: vec![],
        artifacts: vec![],
        provider_evidence: vec![],
        file_evidence: vec![],
        command_evidence: vec![],
        folder_approvals: vec![],
        user_artifacts: vec![],
        intervention: None,
        steps: vec![],
        accounts: vec![],
        title: None,
    };
    fact.validate(&plan, revision).unwrap();
    let mut reviewed = fact.clone();
    reviewed.authorization = WorkExecutionAuthorization::ReviewedPlan;
    assert!(reviewed.validate(&plan, revision).is_err());
    fact.attempts.push(WorkAttemptFact {
        id: attempt,
        node,
        status: WorkAttemptStatus::Running,
        usage: None,
    });
    fact.status = WorkExecutionStatus::Running;
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
    let usage = WorkUsage {
        model_tokens: 1200,
        cost_micro_usd: 300,
        operations: 1,
        accounting: WorkUsageAccounting::Exact,
    };
    let mut turn = step(1, 1, WorkStepKindV1::Turn, WorkStepStatus::Succeeded);
    turn.usage = Some(usage);
    turn.note = Some("Looking for current options.".into());
    fact.steps.push(turn);
    fact.steps.push(step(
        2,
        1,
        WorkStepKindV1::Search {
            query: "best canvas libraries".into(),
        },
        WorkStepStatus::Running,
    ));
    fact.validate(&plan, revision).unwrap();
    // A settled search binds its provider record and usage.
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
    let sources = WorkArtifactV1 {
        revises: None,
        part: None,
        version: 1,
        id: WorkArtifactId::from(41),
        execution: fact.id,
        node,
        attempt,
        output: "comparison".into(),
        title: "Sources".into(),
        data: artifact::WorkArtifactDataV1::EvidenceCollection {
            summary: "Svelte Flow is a canvas library".into(),
            subjects: vec![],
            entries: vec![],
        },
        evidence: vec![WorkEvidenceLink {
            extraction_id: record.id,
            source_id: 1,
        }],
        review: WorkOutputReview::SourceMappedNeedsReview,
        presentation: artifact::WorkArtifactPresentationV1::Automatic,
        general_knowledge: false,
    };
    fact.steps[1].status = WorkStepStatus::Succeeded;
    fact.steps[1].usage = Some(usage);
    fact.steps[1].evidence = Some(record.id);
    fact.steps[1].artifacts = vec![sources.id];
    assert!(fact.validate(&plan, revision).is_err());
    fact.provider_evidence.push(record);
    fact.artifacts.push(sources);
    fact.validate(&plan, revision).unwrap();
    // Unclaimed artifacts and unknown questions never validate.
    let mut orphan = fact.clone();
    orphan.steps[1].artifacts.clear();
    assert!(orphan.validate(&plan, revision).is_err());
    let mut answered_running = fact.clone();
    answered_running.steps.push(step(
        3,
        2,
        WorkStepKindV1::Ask {
            prompt: "Which budget?".into(),
            options: vec!["Low".into(), "High".into()],
            answer: Some("Low".into()),
            purpose: Some(WorkAskPurposeV1::Question),
        },
        WorkStepStatus::Running,
    ));
    assert!(answered_running.validate(&plan, revision).is_err());
    // Success requires a final Finish step and no running steps.
    let mut early = fact.clone();
    early.attempts[0].status = WorkAttemptStatus::Succeeded;
    early.attempts[0].usage = Some(usage);
    early.status = WorkExecutionStatus::NeedsReview;
    assert!(early.validate(&plan, revision).is_err());
    fact.steps.push(step(
        4,
        2,
        WorkStepKindV1::Finish {
            followups: vec![],
            title: None,
        },
        WorkStepStatus::Succeeded,
    ));
    fact.attempts[0].status = WorkAttemptStatus::Succeeded;
    fact.attempts[0].usage = Some(usage);
    fact.status = WorkExecutionStatus::NeedsReview;
    fact.validate(&plan, revision).unwrap();
    // Finishing the answer must not make later cancellation or failed
    // settlement impossible; the published artifacts remain available.
    for (attempt_status, execution_status) in [
        (WorkAttemptStatus::Cancelled, WorkExecutionStatus::Cancelled),
        (WorkAttemptStatus::Failed, WorkExecutionStatus::Failed),
        (
            WorkAttemptStatus::OutcomeUnknown,
            WorkExecutionStatus::Interrupted,
        ),
    ] {
        let mut interrupted = fact.clone();
        interrupted.attempts[0].status = attempt_status;
        interrupted.attempts[0].usage =
            (attempt_status != WorkAttemptStatus::OutcomeUnknown).then_some(usage);
        interrupted.status = execution_status;
        assert!(
            interrupted.validate(&plan, revision).is_ok(),
            "terminal settlement after Finish: {attempt_status:?}"
        );
    }
    let mut after_finish = fact.clone();
    after_finish.attempts[0].status = WorkAttemptStatus::Running;
    after_finish.attempts[0].usage = None;
    after_finish.status = WorkExecutionStatus::Running;
    after_finish
        .steps
        .push(step(5, 3, WorkStepKindV1::Turn, WorkStepStatus::Failed));
    assert!(after_finish.validate(&plan, revision).is_err());

    // Turn resolution admits only shown sources and a complete vocabulary.
    let preview = WorkEvidencePreviewV1 {
        link_destination: None,
        version: 1,
        link: WorkEvidenceLink {
            extraction_id: WorkArtifactId::from(40),
            source_id: 1,
        },
        origin: "https://svelteflow.dev".into(),
        role: "provider_search".into(),
        text: "Svelte Flow is a canvas library".into(),
        truncated: false,
        source_bytes: "31".into(),
        source: WorkEvidenceSourceV1::ProviderSearch {
            provider: WorkSearchProvider::OpenAi,
            model: PUBLIC_SEARCH_MODEL.into(),
            url: "https://svelteflow.dev/docs".into(),
            title: "Svelte Flow".into(),
            response_id: "resp_1".into(),
            search_call_id: "ws_1".into(),
        },
    };
    let disclosure = WorkAgentTurnDisclosure::try_new(
        "Compare canvas libraries",
        vec![],
        vec![],
        &fact.steps,
        std::slice::from_ref(&preview),
        &fact.artifacts,
        WorkAgentBudget {
            turns_left: 6,
            steps_left: 20,
            browse_available: true,
        },
        limits,
        vec![],
    )
    .unwrap();
    assert_eq!(
        disclosure.context().sources[0].url,
        "https://svelteflow.dev/docs"
    );
    // Different artifacts can both cite local index zero after unrelated search results.
    let mut native = preview.clone();
    native.link.extraction_id = WorkArtifactId::from(90);
    native.source = WorkEvidenceSourceV1::NativeExtraction;
    let mut second_native = native.clone();
    second_native.link.source_id = 2;
    let mut findings = fact.artifacts[0].clone();
    findings.evidence = vec![native.link.clone(), second_native.link.clone()];
    let views = WorkAgentTurnDisclosure::try_new(
        "Compare canvas libraries",
        vec![],
        vec![],
        &[],
        &[preview, second_native.clone(), native.clone()],
        &[fact.artifacts[0].clone(), findings.clone()],
        WorkAgentBudget {
            turns_left: 6,
            steps_left: 20,
            browse_available: true,
        },
        limits,
        vec![],
    )
    .unwrap();
    assert_eq!(views.context().sources[0].acquired_by, "provider_search");
    assert_eq!(views.context().sources[1].acquired_by, "native_browser");
    assert_eq!(views.context().artifacts[0].evidence, vec![0]);
    assert_eq!(views.context().artifacts[1].evidence, vec![2, 1]);
    let missing = WorkAgentTurnDisclosure::try_new(
        "Compare canvas libraries",
        vec![],
        vec![],
        &[],
        &[second_native],
        &[findings],
        WorkAgentBudget {
            turns_left: 6,
            steps_left: 20,
            browse_available: true,
        },
        limits,
        vec![],
    )
    .unwrap();
    assert_eq!(missing.context().artifacts[0].evidence, vec![0]);
    assert_eq!(
        serde_json::to_value(missing.context()).unwrap()["artifacts"][0]["evidence"],
        serde_json::json!([0])
    );
    assert!(missing.context().artifacts[0].data.is_none());
    assert_eq!(views.context().citation_space, "source_keys");
    let output = |fetch, ask, finish| WorkAgentTurnOutput {
        say: Some("Comparing.".into()),
        artifacts: vec![WorkAgentArtifactOutput {
            title: "Svelte Flow".into(),
            data: artifact::WorkArtifactDataV1::Findings {
                subjects: vec![],
                items: vec![artifact::WorkFinding {
                    claim: "Svelte Flow is a canvas library".into(),
                    subject: None,
                    evidence: vec![0],
                    confidence: artifact::WorkConfidence::Supported,
                    detail: None,
                    general_knowledge: false,
                }],
            },
            evidence: vec![0],
            general_knowledge: false,
        }],
        fetch,
        ask,
        finish,
        followups: vec![],
        malformed: 0,
    };
    let turn = disclosure
        .resolve(output(
            vec![
                WorkAgentFetch::Read {
                    url: "https://svelteflow.dev/docs".into(),
                    collection: None,
                },
                WorkAgentFetch::Search {
                    query: "svelte flow bundle size".into(),
                },
            ],
            None,
            false,
        ))
        .unwrap();
    assert_eq!(turn.fetch.len(), 2);
    assert_eq!(turn.artifacts[0].evidence[0].source_id, 1);
    let mut mapped = output(vec![], None, true);
    mapped.artifacts[0].evidence = vec![views.context().artifacts[1].evidence[0]];
    if let artifact::WorkArtifactDataV1::Findings { items, .. } = &mut mapped.artifacts[0].data {
        items[0].evidence = vec![2];
    }
    assert_eq!(
        views.resolve(mapped).unwrap().artifacts[0].evidence,
        vec![native.link]
    );
    let collect: WorkAgentFetch = serde_json::from_value(serde_json::json!({
        "kind":"read", "url":"https://svelteflow.dev/docs",
        "records":{"title":"Libraries", "max_items":2, "columns":[
            {"name":"license", "required":false, "value":{"kind":"text"}}
        ]}
    }))
    .unwrap();
    let direct = WorkAgentTurnDisclosure::try_new(
        "Read [the documentation](https://svelteflow.dev/docs) in the browser",
        vec![],
        vec![],
        &[],
        &[],
        &[],
        WorkAgentBudget {
            turns_left: 6,
            steps_left: 20,
            browse_available: true,
        },
        limits,
        vec![],
    )
    .unwrap();
    assert!(direct.context().sources.is_empty());
    assert_eq!(
        direct.context().requested_pages,
        ["https://svelteflow.dev/docs"]
    );
    let direct_turn = direct
        .resolve(WorkAgentTurnOutput {
            say: None,
            artifacts: vec![],
            fetch: vec![collect.clone()],
            ask: None,
            finish: false,
            followups: vec![],
            malformed: 0,
        })
        .unwrap();
    assert_eq!(direct_turn.fetch.len(), 1);
    let admitted = disclosure
        .resolve(output(vec![collect.clone()], None, false))
        .unwrap();
    assert!(
        matches!(&admitted.fetch[0], WorkStepKindV1::Read { collection: Some(shape), .. } if shape.max_items == 2)
    );
    let encoded = serde_json::to_value(&admitted.fetch[0]).unwrap();
    assert!(serde_json::from_value::<WorkStepKindV1>(encoded).unwrap() == admitted.fetch[0]);
    let WorkAgentFetch::Read { collection, .. } = collect else {
        panic!()
    };
    let unseen = disclosure
        .resolve(output(
            vec![WorkAgentFetch::Read {
                url: "https://unseen.example/".into(),
                collection,
            }],
            None,
            false,
        ))
        .unwrap();
    assert!(unseen.fetch.is_empty() && unseen.artifacts.len() == 1 && unseen.notices.len() == 1);
    // Finish yields to a fetch in the same turn; the model is told.
    let fetching = disclosure
        .resolve(output(
            vec![WorkAgentFetch::Search {
                query: "more".into(),
            }],
            None,
            true,
        ))
        .unwrap();
    assert!(!fetching.finish && fetching.fetch.len() == 1 && fetching.notices.len() == 1);
    // An uncited object is dropped on its own; a turn made only of dropped
    // objects is admitted idle so its refusal reaches the model.
    let mut uncited = output(vec![], None, true);
    uncited.artifacts[0].evidence.clear();
    let finished = disclosure.resolve(uncited).unwrap();
    assert!(finished.artifacts.is_empty() && finished.finish && finished.dropped == 1);
    let mut only_uncited = output(vec![], None, false);
    only_uncited.artifacts[0].evidence.clear();
    let idle = disclosure.resolve(only_uncited).unwrap();
    assert!(idle.artifacts.is_empty() && idle.refusals.len() == 1 && idle.fetch.is_empty());
    // An object the transport could not decode is refused by name, and the
    // turn's fetch still runs.
    let mut undecodable = output(vec![], None, false);
    undecodable.artifacts.clear();
    undecodable.malformed = 2;
    undecodable.fetch = vec![WorkAgentFetch::Search {
        query: "svelte flow".into(),
    }];
    let admitted = disclosure.resolve(undecodable).unwrap();
    assert_eq!(admitted.dropped, 2);
    assert_eq!(
        admitted.refusals,
        vec![
            WorkAgentArtifactRefusal::Malformed(None),
            WorkAgentArtifactRefusal::Malformed(None)
        ]
    );
    assert_eq!(admitted.fetch.len(), 1);
    assert_eq!(
        disclosure
            .resolve(WorkAgentTurnOutput {
                say: Some("x".repeat(600)),
                artifacts: vec![],
                fetch: vec![],
                ask: None,
                finish: false,
                followups: vec![],
                malformed: 0,
            })
            .err(),
        Some(WorkAgentTurnRefusal::Empty)
    );
    let mut long_say = output(vec![], None, true);
    long_say.say = Some("x".repeat(600));
    let long = disclosure
        .resolve(long_say)
        .map(|turn| turn.say.unwrap())
        .unwrap();
    assert!(long.len() <= MAX_WORK_STEP_NOTE_BYTES);
    assert_eq!(clip_text("héllo wörld", 8), "héll\u{2026}");
}

#[test]
fn agent_citations_round_trip_source_keys_without_model_renumbering() {
    use super::{agent::*, artifact::*, runtime::*};
    use serde_json::json;
    let previews: Vec<_> = (0..80)
        .map(|key| WorkEvidencePreviewV1 {
            version: 1,
            link: WorkEvidenceLink {
                extraction_id: 40.into(),
                source_id: key + 1,
            },
            origin: "https://shop.example.test/".into(),
            role: "paragraph".into(),
            text: format!("Observed detail {key}"),
            source_bytes: format!("Observed detail {key}").len().to_string(),
            truncated: false,
            link_destination: None,
            source: WorkEvidenceSourceV1::NativeExtraction,
        })
        .collect();
    let disclose = |previews: &[WorkEvidencePreviewV1], artifacts: &[WorkArtifactV1]| {
        WorkAgentTurnDisclosure::try_new(
            "Compare the observed products",
            vec![],
            vec![],
            &[],
            previews,
            artifacts,
            WorkAgentBudget {
                turns_left: 3,
                steps_left: 8,
                browse_available: true,
            },
            WorkExecutionLimits {
                model_tokens: 8000,
                cost_micro_usd: 10000,
                operations: 8,
                timeout_seconds: 60,
                max_workers: 1,
            },
            vec![],
        )
        .unwrap()
    };
    let disclosure = disclose(&previews, &[]);
    let subject = json!({"name":"Product"});
    for data in [
        json!({"kind":"comparison_matrix","subjects":[subject],"criteria":[{"name":"Details","kind":{"kind":"text"}},{"name":"Unavailable","kind":{"kind":"text"}}],"cells":[[{"value":{"kind":"text","text":"Observed"},"evidence":[70,20]},{"value":{"kind":"unknown"},"evidence":[]}]],"notes":[]}),
        json!({"kind":"findings","items":[{"claim":"Observed","evidence":[70,20],"confidence":"supported"}]}),
        json!({"kind":"chart","x_label":"Product","y_label":"Count","series":[{"name":"Count","points":[{"label":"Product","value":"12","evidence":[70,20]}]}]}),
        json!({"kind":"evidence_collection","summary":"Sources","entries":[{"evidence":70,"title":"First","role":"source"},{"evidence":20,"title":"Second","role":"source"}]}),
    ] {
        let output = || WorkAgentTurnOutput {
            say: None,
            fetch: vec![],
            ask: None,
            finish: true,
            followups: vec![],
            malformed: 0,
            artifacts: vec![WorkAgentArtifactOutput {
                title: "Comparison".into(),
                data: serde_json::from_value(data.clone()).unwrap(),
                evidence: vec![1],
                general_knowledge: false,
            }],
        };
        let resolved = disclosure.resolve(output()).unwrap();
        assert_eq!(resolved.dropped, 0);
        let proposed = &resolved.artifacts[0];
        assert!(proposed.data.validate(3).is_ok());
        assert_eq!(
            proposed.evidence,
            vec![
                previews[1].link.clone(),
                previews[70].link.clone(),
                previews[20].link.clone()
            ]
        );
        let stored = WorkArtifactV1 {
            revises: None,
            part: None,
            version: 1,
            id: 1.into(),
            execution: 2.into(),
            node: 3.into(),
            attempt: 4.into(),
            output: "Result".into(),
            title: proposed.title.clone(),
            data: proposed.data.clone(),
            evidence: proposed.evidence.clone(),
            review: WorkOutputReview::SourceMappedNeedsReview,
            presentation: WorkArtifactPresentationV1::Automatic,
            general_knowledge: false,
        };
        assert!(stored.validate().is_ok());
        let shown = disclose(&previews, std::slice::from_ref(&stored));
        assert_eq!(
            serde_json::to_value(shown.context().artifacts[0].data.as_ref().unwrap()).unwrap(),
            serde_json::to_value(
                serde_json::from_value::<WorkArtifactDataV1>(data.clone()).unwrap()
            )
            .unwrap()
        );
        let reversed: Vec<_> = previews.iter().rev().cloned().collect();
        let shown = disclose(&reversed, &[stored]);
        let mut replay = output();
        replay.artifacts[0].data = shown.context().artifacts[0].data.clone().unwrap();
        replay.artifacts[0].evidence = shown.context().artifacts[0].evidence.clone();
        let replayed = shown.resolve(replay).unwrap();
        assert_eq!(replayed.artifacts[0].evidence, proposed.evidence);
        assert!(replayed.artifacts[0].data == proposed.data);
        // A citation of a source that does not exist is dropped on its own;
        // the object stands on the real citations it still has.
        let mut unknown = output();
        unknown.artifacts[0].evidence = vec![80, 1];
        let kept = disclosure.resolve(unknown).unwrap();
        assert!(kept.refusals.is_empty());
        assert_eq!(kept.artifacts[0].evidence, proposed.evidence);
        assert!(kept
            .notices
            .iter()
            .any(|notice| notice.starts_with("1 citations pointed at sources that do not exist")));
        // With no real citation left, the object is refused.
        let mut none = output();
        none.artifacts[0].evidence = vec![80];
        none.artifacts[0].data =
            serde_json::from_str(&data.to_string().replace("70", "81").replace("20", "82"))
                .unwrap();
        let refused = disclosure.resolve(none).unwrap();
        assert!(refused.artifacts.is_empty());
        assert_eq!(
            refused.refusals,
            vec![WorkAgentArtifactRefusal::UnknownEvidenceKey]
        );
        assert!(refused
            .notices
            .iter()
            .any(|notice| notice.starts_with("3 citations pointed at sources that do not exist")));
    }
}

#[test]
fn person_steps_settle_at_once_and_followups_ride_only_on_finish() {
    use super::{agent::*, runtime::*};
    let step = |kind, status| WorkStepFact {
        model_fault: None,
        part: None,
        id: 9.into(),
        turn: 2,
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
    let steer = |status| {
        step(
            WorkStepKindV1::Steer {
                text: "Only official pages".into(),
            },
            status,
        )
    };
    assert!(steer(WorkStepStatus::Succeeded).validate().is_ok());
    assert!(steer(WorkStepStatus::Running).validate().is_err());
    let finish = |followups: Vec<&str>| {
        step(
            WorkStepKindV1::Finish {
                followups: followups.into_iter().map(String::from).collect(),
                title: None,
            },
            WorkStepStatus::Succeeded,
        )
        .validate()
    };
    assert!(finish(vec!["Pick the Airbnb for Jan 4–23", "Book the LOT fare"]).is_ok());
    assert!(finish(vec!["Add Tower Bridge to cart", "Watch the price"]).is_ok());
    assert!(finish(vec!["a", "b", "c", "d"]).is_err());
    assert!(finish(vec!["a", "a"]).is_err());
    assert!(finish(vec![&"x".repeat(121)]).is_err());
    let disclosure = WorkAgentTurnDisclosure::try_new(
        "Compare sets",
        vec![],
        vec![],
        &[],
        &[],
        &[],
        WorkAgentBudget {
            turns_left: 3,
            steps_left: 9,
            browse_available: true,
        },
        WorkExecutionLimits {
            model_tokens: 100_000,
            cost_micro_usd: 100_000,
            operations: 10,
            timeout_seconds: 60,
            max_workers: 1,
        },
        vec![],
    )
    .unwrap();
    let output = |finish| WorkAgentTurnOutput {
        say: None,
        artifacts: vec![],
        fetch: vec![WorkAgentFetch::Search {
            query: "lego architecture sets".into(),
        }],
        ask: None,
        finish,
        malformed: 0,
        followups: vec![
            " Add to cart ".into(),
            "".into(),
            "Add to cart".into(),
            "x".repeat(121),
            "Watch the price".into(),
            "Compare two".into(),
            "Fourth".into(),
        ],
    };
    let file = |kind, status| WorkStepFact {
        model_fault: None,
        part: None,
        id: 10.into(),
        turn: 2,
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
    let proposal = |decision| WorkStepKindV1::WriteFile {
        path: "/Users/me/Documents/project/notes.txt".into(),
        content: "one\n".into(),
        decision,
    };
    assert!(file(proposal(None), WorkStepStatus::Running)
        .validate()
        .is_ok());
    assert!(file(proposal(Some(true)), WorkStepStatus::Running)
        .validate()
        .is_ok());
    assert!(file(proposal(None), WorkStepStatus::Failed)
        .validate()
        .is_ok());
    assert!(file(proposal(Some(false)), WorkStepStatus::Failed)
        .validate()
        .is_ok());
    assert!(file(
        WorkStepKindV1::ReadFile {
            path: "relative/notes.txt".into(),
            offset: None,
            limit: None
        },
        WorkStepStatus::Running
    )
    .validate()
    .is_err());
    let mut succeeded_read = file(
        WorkStepKindV1::ReadFile {
            path: "/Users/me/Documents/project/notes.txt".into(),
            offset: None,
            limit: None,
        },
        WorkStepStatus::Succeeded,
    );
    assert!(succeeded_read.validate().is_err());
    succeeded_read.evidence = Some(11.into());
    assert!(succeeded_read.validate().is_ok());
    let grant = |folders: Vec<&str>| WorkAgentGrantV1 {
        no_usage_charge: false,
        draft: None,
        lead: None,
        provider: super::search::WorkSearchProvider::OpenAi,
        model: "gpt-5.6-luna".into(),
        max_turns: 4,
        max_steps: 12,
        browse_hops: 2,
        folders: folders.into_iter().map(String::from).collect(),
        accounts: Vec::new(),
        private: false,
        skill: None,
        search_fallback: None,
    };
    assert!(grant(vec!["/Users/me/Documents/project"])
        .validate()
        .is_ok());
    assert!(grant(vec!["Documents/project"]).validate().is_err());
    assert!(grant(vec!["/a"; 9]).validate().is_err());
    for (skill, valid) in [
        ("trip-planning", true),
        ("Trip planning", false),
        ("", false),
    ] {
        let started = WorkAgentGrantV1 {
            no_usage_charge: false,
            skill: Some(skill.into()),
            ..grant(vec![])
        };
        assert_eq!(started.validate().is_ok(), valid, "{skill}");
    }
    assert!(disclosure
        .resolve(output(false))
        .unwrap()
        .followups
        .is_empty());
    let mut finishing = output(true);
    finishing.fetch.clear();
    assert_eq!(
        disclosure.resolve(finishing).unwrap().followups,
        vec!["Add to cart", "Watch the price", "Compare two"]
    );
}

#[test]
fn step_measurements_name_their_cost_basis_and_read_older_records() {
    use super::runtime::{WorkCostBasis, WorkStepMeasurementsV1};
    let fields = r#""wall_millis":1,"decision_calls":0,"emulation_calls":0,"planner_calls":1,"native_actions":0,"model_tokens":10,"cost_micro_usd":2"#;
    let read = |extra: &str| -> WorkStepMeasurementsV1 {
        serde_json::from_str(&format!("{{{fields}{extra}}}")).unwrap()
    };
    assert_eq!(
        read(r#","cost_exact":true"#).cost_basis,
        WorkCostBasis::Exact
    );
    assert_eq!(
        read(r#","cost_exact":false"#).cost_basis,
        WorkCostBasis::Reserved
    );
    assert_eq!(read("").settled_revision, None);
    let sequenced = read(r#","settled_revision":"42""#);
    assert_eq!(sequenced.settled_revision, WorkRevision::new(42));
    assert_eq!(
        serde_json::from_str::<WorkStepMeasurementsV1>(&serde_json::to_string(&sequenced).unwrap())
            .unwrap(),
        sequenced
    );
    let priced = read(r#","cost_basis":"priced""#);
    assert_eq!(priced.cost_basis, WorkCostBasis::Priced);
    assert_eq!(priced.outcome, None);
    assert_eq!(priced.resolved_handoff, None);
    for (wire, kind) in [
        ("sign_in", runtime::WorkPageHandoffKindV1::SignIn),
        ("check", runtime::WorkPageHandoffKindV1::Check),
        (
            "sign_in_and_check",
            runtime::WorkPageHandoffKindV1::SignInAndCheck,
        ),
    ] {
        let resolved = read(&format!(",\"resolved_handoff\":\"{wire}\""));
        assert_eq!(resolved.resolved_handoff, Some(kind));
        assert_eq!(
            serde_json::from_str::<WorkStepMeasurementsV1>(
                &serde_json::to_string(&resolved).unwrap()
            )
            .unwrap(),
            resolved
        );
    }
    assert!(runtime::WorkPageHandoffKindV1::SignInAndCheck
        .includes(runtime::WorkPageHandoffKindV1::SignIn));
    assert!(runtime::WorkPageHandoffKindV1::SignInAndCheck
        .includes(runtime::WorkPageHandoffKindV1::Check));
    assert!(!runtime::WorkPageHandoffKindV1::Check.includes(runtime::WorkPageHandoffKindV1::SignIn));
    let checked = read(",\"outcome\":{\"kind\":\"needs_check\"}");
    assert_eq!(
        checked.outcome,
        Some(runtime::WorkPageOutcomeV1::NeedsCheck)
    );
    assert_eq!(
        serde_json::from_str::<WorkStepMeasurementsV1>(&serde_json::to_string(&checked).unwrap())
            .unwrap(),
        checked
    );
    let written = serde_json::to_string(&priced).unwrap();
    assert!(written.contains(r#""cost_basis":"priced""#) && !written.contains("cost_exact"));
    assert_eq!(
        serde_json::from_str::<WorkStepMeasurementsV1>(&written).unwrap(),
        priced
    );
    assert!(serde_json::from_str::<WorkStepMeasurementsV1>(&format!(
        "{{{fields},\"cost_basis\":\"priced\",\"other\":1}}"
    ))
    .is_err());
}

#[test]
fn a_knowledge_object_stands_without_evidence_but_never_claims_a_source() {
    use super::{agent::*, artifact::*, runtime::*};
    let disclosure = WorkAgentTurnDisclosure::try_new(
        "Design an AI SaaS architecture",
        vec![],
        vec![],
        &[],
        &[],
        &[],
        WorkAgentBudget {
            turns_left: 6,
            steps_left: 20,
            browse_available: true,
        },
        WorkExecutionLimits {
            model_tokens: 1000,
            cost_micro_usd: 1000,
            operations: 2,
            timeout_seconds: 60,
            max_workers: 2,
        },
        vec![],
    )
    .unwrap();
    let findings = |homepage: Option<&str>| WorkArtifactDataV1::Findings {
        subjects: vec![WorkSubject {
            name: "Postgres".into(),
            descriptor: None,
            homepage: homepage.map(Into::into),
            record_media: None,
            media_attempted: false,
            image_candidates: vec![],
        }],
        items: vec![WorkFinding {
            claim: "Row-level security isolates tenants".into(),
            subject: Some(0),
            evidence: vec![],
            confidence: WorkConfidence::Supported,
            detail: None,
            general_knowledge: false,
        }],
    };
    let turn = |data: WorkArtifactDataV1, general_knowledge: bool| WorkAgentTurnOutput {
        say: None,
        artifacts: vec![WorkAgentArtifactOutput {
            title: "Practices".into(),
            data,
            evidence: vec![],
            general_knowledge,
        }],
        fetch: vec![],
        ask: None,
        finish: false,
        followups: vec![],
        malformed: 0,
    };
    // Findings are cited research only: without a source they are refused,
    // knowledge or not, and the notice sends the prose to the answer.
    for known in [false, true] {
        let refused = disclosure.resolve(turn(findings(None), known)).unwrap();
        assert!(refused.artifacts.is_empty());
        assert_eq!(
            refused.refusals,
            [WorkAgentArtifactRefusal::FindingsUncited]
        );
    }
    let notice = WorkAgentArtifactRefusal::FindingsUncited.notice();
    assert!(
        notice.contains("findings are cited facts")
            && notice.contains("the answer carries the prose")
    );
    let answer = |markdown: &str| WorkArtifactDataV1::Answer {
        markdown: markdown.into(),
    };
    let lead = "Row-level security isolates tenants in one `Postgres` database.";
    let cited = disclosure.resolve(turn(answer(lead), false)).unwrap();
    assert_eq!(cited.refusals, [WorkAgentArtifactRefusal::Uncited]);
    let known = disclosure.resolve(turn(answer(lead), true)).unwrap();
    let [artifact] = known.artifacts.as_slice() else {
        panic!("knowledge object refused: {:?}", known.refusals);
    };
    assert!(artifact.general_knowledge && artifact.evidence.is_empty());
    // One answer per request: a second in the same turn is refused, and so is
    // any answer once one of this request's publish steps placed one.
    let two = disclosure
        .resolve(WorkAgentTurnOutput {
            artifacts: [lead, "A second reply."]
                .into_iter()
                .map(|markdown| WorkAgentArtifactOutput {
                    title: "Tenancy".into(),
                    data: answer(markdown),
                    evidence: vec![],
                    general_knowledge: true,
                })
                .collect(),
            ..turn(answer(lead), true)
        })
        .unwrap();
    assert_eq!(two.artifacts.len(), 1);
    assert_eq!(two.refusals, [WorkAgentArtifactRefusal::AnswerRepeated]);
    assert!(WorkAgentArtifactRefusal::AnswerRepeated
        .notice()
        .contains("the first answer stands"));
    let placed = WorkArtifactV1 {
        revises: None,
        part: None,
        version: 1,
        id: 7.into(),
        execution: 1.into(),
        node: 1.into(),
        attempt: 1.into(),
        output: String::new(),
        title: "Tenancy".into(),
        data: answer(lead),
        evidence: vec![],
        review: WorkOutputReview::UserAcceptance,
        presentation: WorkArtifactPresentationV1::Automatic,
        general_knowledge: true,
    };
    let publish = WorkStepFact {
        model_fault: None,
        part: None,
        id: 3.into(),
        turn: 1,
        kind: WorkStepKindV1::Publish,
        status: WorkStepStatus::Succeeded,
        usage: None,
        artifacts: vec![placed.id],
        evidence: None,
        note: None,
        measurements: None,
        local: None,
        account: None,
    };
    let later = WorkAgentTurnDisclosure::try_new(
        "Design an AI SaaS architecture",
        vec![],
        vec![],
        std::slice::from_ref(&publish),
        &[],
        std::slice::from_ref(&placed),
        WorkAgentBudget {
            turns_left: 5,
            steps_left: 19,
            browse_available: true,
        },
        WorkExecutionLimits {
            model_tokens: 1000,
            cost_micro_usd: 1000,
            operations: 2,
            timeout_seconds: 60,
            max_workers: 2,
        },
        vec![],
    )
    .unwrap();
    let again = later
        .resolve(turn(answer("A revised reply."), true))
        .unwrap();
    assert_eq!(again.refusals, [WorkAgentArtifactRefusal::AnswerRepeated]);
    // An answer of an earlier request on the canvas does not count.
    let inherited = WorkAgentTurnDisclosure::try_new(
        "Design an AI SaaS architecture",
        vec![],
        vec![],
        &[],
        &[],
        std::slice::from_ref(&placed),
        WorkAgentBudget {
            turns_left: 5,
            steps_left: 19,
            browse_available: true,
        },
        WorkExecutionLimits {
            model_tokens: 1000,
            cost_micro_usd: 1000,
            operations: 2,
            timeout_seconds: 60,
            max_workers: 2,
        },
        vec![],
    )
    .unwrap();
    assert!(inherited
        .resolve(turn(answer("A new request's reply."), true))
        .unwrap()
        .refusals
        .is_empty());
    let linked = disclosure
        .resolve(turn(
            answer("Read https://www.postgresql.org/docs/ first."),
            true,
        ))
        .unwrap();
    assert!(linked.artifacts.is_empty());
    assert_eq!(linked.refusals, [WorkAgentArtifactRefusal::KnowledgeLink]);
    // A URL inside code is code, and a fence written in capitals or CRLF stands.
    let fenced = disclosure
        .resolve(turn(
            answer("Fetch it:\r\n\r\n```Bash\r\ncurl https://example.com\r\n```\r\n"),
            true,
        ))
        .unwrap();
    let WorkArtifactDataV1::Answer { markdown } = &fenced.artifacts[0].data else {
        panic!("answer refused: {:?}", fenced.refusals);
    };
    assert_eq!(
        markdown,
        "Fetch it:\n\n```bash\ncurl https://example.com\n```\n"
    );
    let matrix = WorkArtifactDataV1::ComparisonMatrix {
        subjects: vec![WorkSubject {
            name: "Postgres".into(),
            descriptor: None,
            homepage: Some("https://www.postgresql.org/".into()),
            record_media: None,
            media_attempted: false,
            image_candidates: vec![],
        }],
        criteria: vec![WorkCriterion {
            name: "Isolation".into(),
            kind: WorkCriterionKind::Text,
        }],
        cells: vec![vec![WorkCell {
            value: WorkCellValue::Text {
                text: "Row-level security".into(),
            },
            evidence: vec![],
            note: None,
            general_knowledge: true,
        }]],
        notes: vec![],
    };
    let linked = disclosure.resolve(turn(matrix, true)).unwrap();
    assert_eq!(linked.refusals, [WorkAgentArtifactRefusal::KnowledgeLink]);
    let diagram = WorkArtifactDataV1::Diagram {
        nodes: vec![
            WorkDiagramNode {
                id: "web".into(),
                name: "Web app".into(),
                kind: WorkDiagramNodeKind::Client,
                vendor: Some(" https://Vercel.com/ ".into()),
                note: None,
                layer: None,
            },
            WorkDiagramNode {
                id: "db".into(),
                name: "Postgres".into(),
                kind: WorkDiagramNodeKind::Store,
                vendor: Some("not a host".into()),
                note: None,
                layer: None,
            },
        ],
        edges: vec![WorkDiagramEdge {
            from: "web".into(),
            to: "db".into(),
            label: Some("SQL".into()),
        }],
        layers: vec![],
    };
    let drawn = disclosure.resolve(turn(diagram, true)).unwrap();
    let WorkArtifactDataV1::Diagram { nodes, .. } = &drawn.artifacts[0].data else {
        panic!("diagram refused: {:?}", drawn.refusals);
    };
    assert_eq!(nodes[0].vendor.as_deref(), Some("vercel.com"));
    assert_eq!(nodes[1].vendor, None);
    let matrix = WorkArtifactDataV1::ComparisonMatrix {
        subjects: vec![WorkSubject {
            name: "Starter".into(),
            descriptor: None,
            homepage: None,
            record_media: None,
            media_attempted: false,
            image_candidates: vec![],
        }],
        criteria: vec![WorkCriterion {
            name: "Monthly".into(),
            kind: WorkCriterionKind::Text,
        }],
        cells: vec![vec![WorkCell {
            value: WorkCellValue::Money {
                amount: "300".into(),
                currency: "USD".into(),
                observed_at: Some("2026-09-01".into()),
            },
            evidence: vec![],
            note: None,
            general_knowledge: false,
        }]],
        notes: vec![],
    };
    let priced = disclosure.resolve(turn(matrix, true)).unwrap();
    let WorkArtifactDataV1::ComparisonMatrix { cells, .. } = &priced.artifacts[0].data else {
        panic!("matrix refused: {:?}", priced.refusals);
    };
    assert!(cells[0][0].general_knowledge);
    assert!(matches!(
        &cells[0][0].value,
        WorkCellValue::Money {
            observed_at: None,
            ..
        }
    ));
}

#[test]
fn charts_of_nothing_second_findings_and_malformed_objects_are_refused_by_cause() {
    use super::{agent::*, artifact::*, runtime::*};
    let disclosure = WorkAgentTurnDisclosure::try_new(
        "Compare SQLite, DuckDB and RocksDB",
        vec![],
        vec![],
        &[],
        &[],
        &[],
        WorkAgentBudget {
            turns_left: 6,
            steps_left: 20,
            browse_available: true,
        },
        WorkExecutionLimits {
            model_tokens: 1000,
            cost_micro_usd: 1000,
            operations: 2,
            timeout_seconds: 60,
            max_workers: 2,
        },
        vec![],
    )
    .unwrap();
    let object = |data: WorkArtifactDataV1| WorkAgentArtifactOutput {
        title: "Object".into(),
        data,
        evidence: vec![],
        general_knowledge: true,
    };
    let turn = |data: Vec<WorkArtifactDataV1>| WorkAgentTurnOutput {
        say: None,
        artifacts: data.into_iter().map(object).collect(),
        fetch: vec![],
        ask: None,
        finish: false,
        followups: vec![],
        malformed: 0,
    };
    let chart = |values: &[&str]| WorkArtifactDataV1::Chart {
        x_label: "Engine".into(),
        y_label: "Queries per second".into(),
        series: vec![WorkChartSeries {
            name: "Throughput".into(),
            points: values
                .iter()
                .zip(["SQLite", "DuckDB", "RocksDB"])
                .map(|(value, label)| WorkChartPoint {
                    label: label.into(),
                    value: (*value).into(),
                    evidence: vec![],
                })
                .collect(),
        }],
        basis: Some(WorkMeasurementBasis {
            method: "Typical published figures, not measurements".into(),
            conditions: None,
            versions: None,
            observed_at: None,
        }),
        general_knowledge: true,
    };
    for values in [
        &["0", "0.0", "0"][..],
        &["unknown", "n/a", "—"],
        &["0", "unknown", "0"],
    ] {
        let refused = disclosure.resolve(turn(vec![chart(values)])).unwrap();
        assert!(refused.artifacts.is_empty(), "{values:?}");
        assert_eq!(refused.refusals, [WorkAgentArtifactRefusal::EmptyChart]);
    }
    assert!(WorkAgentArtifactRefusal::EmptyChart
        .notice()
        .contains("say so in the answer"));
    let drawn = disclosure
        .resolve(turn(vec![chart(&["0", "120000", "80000"])]))
        .unwrap();
    assert_eq!(drawn.artifacts.len(), 1);
    let priced = disclosure
        .resolve(turn(vec![chart(&["20", "$50–500", "30"])]))
        .unwrap();
    assert_eq!(
        priced.refusals,
        [WorkAgentArtifactRefusal::Malformed(Some(
            WorkArtifactFault {
                kind: "chart",
                field: WorkArtifactField::ChartPointValue,
            }
        ))]
    );
    assert_eq!(
        priced.refusals[0].notice(),
        "is a chart with invalid content: chart point value must be a plain number, such as 250 or 0.5, with units in the axis label"
    );
    let node = |id: &str| WorkDiagramNode {
        id: id.into(),
        name: id.into(),
        kind: WorkDiagramNodeKind::Service,
        vendor: None,
        note: None,
        layer: None,
    };
    let dangling = WorkArtifactDataV1::Diagram {
        nodes: vec![node("api"), node("db")],
        edges: vec![WorkDiagramEdge {
            from: "api".into(),
            to: "cache".into(),
            label: None,
        }],
        layers: vec![],
    };
    let refused = disclosure.resolve(turn(vec![dangling])).unwrap();
    assert_eq!(
        refused.refusals[0].notice(),
        "is a diagram with invalid content: diagram edge refers to an unknown node"
    );
    let code = |language: &str, from: u32, to: u32| WorkArtifactDataV1::Code {
        language: language.into(),
        text: "fn main() {\r\n    run();\r\n}\r\n".into(),
        notes: vec![WorkCodeNote {
            from,
            to,
            text: "Calls the loop".into(),
        }],
    };
    let excerpt = disclosure.resolve(turn(vec![code(" Rust", 2, 3)])).unwrap();
    let WorkArtifactDataV1::Code { language, text, .. } = &excerpt.artifacts[0].data else {
        panic!("code refused: {:?}", excerpt.refusals);
    };
    assert_eq!((language.as_str(), text.lines().count()), ("rust", 3));
    assert!(!text.contains('\r'));
    for (data, notice) in [
        (
            code("brainfuck", 1, 1),
            "is a code with invalid content: code language must be one of rust,",
        ),
        (
            code("rust", 2, 4),
            "is a code with invalid content: code note lines need 1 <= from <= to",
        ),
    ] {
        let refused = disclosure.resolve(turn(vec![data])).unwrap();
        assert!(refused.artifacts.is_empty());
        assert!(refused.refusals[0].notice().starts_with(notice));
    }
    let findings = |subjects: &[&str], claim: &str| WorkArtifactDataV1::Findings {
        subjects: subjects
            .iter()
            .map(|name| WorkSubject {
                name: (*name).into(),
                descriptor: None,
                homepage: None,
                record_media: None,
                media_attempted: false,
                image_candidates: vec![],
            })
            .collect(),
        items: vec![WorkFinding {
            claim: claim.into(),
            subject: None,
            evidence: vec![],
            confidence: WorkConfidence::Inferred,
            detail: None,
            general_knowledge: false,
        }],
    };
    let text = "SQLite FTS5 ships a full-text index.";
    let preview = WorkEvidencePreviewV1 {
        version: 1,
        link: WorkEvidenceLink {
            extraction_id: 40.into(),
            source_id: 1,
        },
        origin: "https://sqlite.org/".into(),
        role: "paragraph".into(),
        text: text.into(),
        truncated: false,
        source_bytes: text.len().to_string(),
        link_destination: None,
        source: WorkEvidenceSourceV1::NativeExtraction,
    };
    let sourced = WorkAgentTurnDisclosure::try_new(
        "Compare SQLite, DuckDB and RocksDB",
        vec![],
        vec![],
        &[],
        std::slice::from_ref(&preview),
        &[],
        WorkAgentBudget {
            turns_left: 6,
            steps_left: 20,
            browse_available: true,
        },
        WorkExecutionLimits {
            model_tokens: 1000,
            cost_micro_usd: 1000,
            operations: 2,
            timeout_seconds: 60,
            max_workers: 2,
        },
        vec![],
    )
    .unwrap();
    let cite = |data: WorkArtifactDataV1| WorkAgentArtifactOutput {
        title: "Findings".into(),
        data,
        evidence: vec![0],
        general_knowledge: false,
    };
    let repeated = sourced
        .resolve(WorkAgentTurnOutput {
            artifacts: [
                findings(
                    &["SQLite", "DuckDB"],
                    "SQLite FTS5 has a built-in full-text index",
                ),
                findings(&["duckdb "], "Run a controlled benchmark before choosing"),
                findings(&[], "No measured winner"),
                findings(&["RocksDB"], "RocksDB is a key-value store"),
            ]
            .into_iter()
            .map(cite)
            .collect(),
            ..turn(vec![])
        })
        .unwrap();
    assert_eq!(repeated.artifacts.len(), 2);
    assert_eq!(
        repeated.refusals,
        [
            WorkAgentArtifactRefusal::DuplicateFindings,
            WorkAgentArtifactRefusal::DuplicateFindings
        ]
    );
    assert!(repeated.refusals[0]
        .notice()
        .starts_with("is a second findings object this turn"));
}

#[test]
fn stored_origin_grants_still_load_and_session_pages_carry_their_badge() {
    use super::{agent::*, context::*, runtime::*, search::*};
    let account = |origin: &str, pages| WorkAccountGrantV1 {
        origin: origin.into(),
        account: "01J8ACCOUNT".into(),
        tab: None,
        pages,
    };
    assert!(account("https://mail.example.com", 12).validate().is_ok());
    for invalid in [
        account("http://mail.example.com", 3),
        account("https://mail.example.com/inbox", 3),
        account("https://mail.example.com", 0),
        account("https://mail.example.com", 13),
        WorkAccountGrantV1 {
            account: "not-alnum".into(),
            ..account("https://mail.example.com", 3)
        },
    ] {
        assert_eq!(invalid.validate(), Err(WorkError::Invalid));
    }
    let mail = account("https://mail.example.com", 2);
    assert!(mail.admits("https://mail.example.com/u/0/inbox?x=1"));
    assert!(!mail.admits("https://docs.example.com/"));
    assert!(!mail.admits("http://mail.example.com/"));
    assert!(!mail.admits("https://mail.example.com:8443/"));
    assert_eq!(mail.host(), "mail.example.com");

    let plan = WorkPlanRevision {
        context: None,
        author: WorkAuthor::User,
        revision: WorkRevision::new(2).unwrap(),
        basis_revision: WorkRevision::INITIAL,
        draft: draft(),
    };
    let limits = WorkExecutionLimits {
        model_tokens: 600_000,
        cost_micro_usd: 1_000_000,
        operations: 64,
        timeout_seconds: 1200,
        max_workers: 2,
    };
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
        accounts: vec![mail.clone()],
        private: false,
        skill: None,
        search_fallback: None,
    };
    grant.validate().unwrap();
    for accounts in [
        vec![mail.clone(), mail.clone()],
        vec![account("https://a.example.com", 1); MAX_WORK_ACCOUNT_GRANTS + 1],
    ] {
        assert!(WorkAgentGrantV1 {
            no_usage_charge: false,
            accounts,
            ..grant.clone()
        }
        .validate()
        .is_err());
    }
    let legacy = serde_json::to_value(WorkAgentGrantV1 {
        no_usage_charge: false,
        accounts: vec![],
        ..grant.clone()
    })
    .unwrap();
    assert!(legacy.get("accounts").is_none());

    let spec = WorkExecutionSpec::agent(&plan, limits, grant).unwrap();
    let revision = WorkRevision::new(9).unwrap();
    let mut fact = WorkExecutionFact {
        parts: vec![],
        inputs: vec![],
        authorization: WorkExecutionAuthorization::UserDirectedAgent,
        id: WorkExecutionId::from(7),
        approved_revision: WorkRevision::new(3).unwrap(),
        spec,
        status: WorkExecutionStatus::Running,
        attempts: vec![WorkAttemptFact {
            id: 500.into(),
            node: plan.draft.nodes[0].id,
            status: WorkAttemptStatus::Running,
            usage: None,
        }],
        artifacts: vec![],
        provider_evidence: vec![],
        file_evidence: vec![],
        command_evidence: vec![],
        folder_approvals: vec![],
        user_artifacts: vec![],
        intervention: None,
        steps: vec![],
        accounts: vec![],
        title: None,
    };
    // The request's accounts are derived: an unrefreshed fact is refused.
    assert!(fact.validate(&plan, revision).is_err());
    fact.refresh_accounts();
    assert_eq!(
        fact.accounts,
        [WorkAccountUseV1 {
            host: "mail.example.com".into(),
            pages_used: 0,
            pages: 2,
        }]
    );
    fact.validate(&plan, revision).unwrap();
    let read = |id: u128, url: &str, host: Option<&str>| WorkStepFact {
        model_fault: None,
        part: None,
        id: id.into(),
        turn: 1,
        kind: WorkStepKindV1::Read {
            verbatim: None,
            url: url.into(),
            collection: None,
            goal: None,
        },
        status: WorkStepStatus::Running,
        usage: None,
        artifacts: vec![],
        evidence: None,
        note: None,
        measurements: None,
        local: None,
        account: host.map(|host| {
            Box::new(WorkPageAccountV1 {
                host: host.into(),
                badge: true,
            })
        }),
    };
    fact.steps.push(read(
        1,
        "https://mail.example.com/inbox",
        Some("mail.example.com"),
    ));
    fact.steps
        .push(read(2, "https://public.example.org/", None));
    fact.refresh_accounts();
    assert_eq!(fact.accounts[0].pages_used, 1);
    fact.validate(&plan, revision).unwrap();
    // A page worked in the person's session carries its own host's badge.
    let mut session = fact.clone();
    session.steps.pop();
    session.steps.push(read(
        3,
        "https://docs.example.com/",
        Some("docs.example.com"),
    ));
    session.refresh_accounts();
    session.validate(&plan, revision).unwrap();
    let mut other = fact.clone();
    other.steps.pop();
    other.steps.push(read(
        3,
        "https://mail.example.com/a",
        Some("other.example.com"),
    ));
    other.refresh_accounts();
    assert!(other.validate(&plan, revision).is_err());
    let mut over = fact.clone();
    over.steps.push(read(
        3,
        "https://mail.example.com/b",
        Some("mail.example.com"),
    ));
    over.steps.push(read(
        4,
        "https://mail.example.com/c",
        Some("mail.example.com"),
    ));
    over.refresh_accounts();
    assert_eq!(over.accounts[0].pages_used, 3);
    assert!(over.validate(&plan, revision).is_err());
    let mut unbadged = read(5, "https://mail.example.com/d", Some("mail.example.com"));
    unbadged.account.as_mut().unwrap().badge = false;
    assert!(unbadged.validate().is_err());
    let mut search = read(5, "https://mail.example.com/d", Some("mail.example.com"));
    search.kind = WorkStepKindV1::Search {
        query: "signed in".into(),
    };
    assert!(search.validate().is_err());

    for (url, class) in [
        ("https://mail.example.com/", WorkPathClass::Root),
        ("https://mail.example.com/u/0/inbox", WorkPathClass::Page),
        (
            "https://mail.example.com/search?q=report",
            WorkPathClass::Query,
        ),
        (
            "https://mail.example.com/account/delete",
            WorkPathClass::Action,
        ),
        ("https://mail.example.com/sign-out", WorkPathClass::Action),
        (
            "https://mail.example.com/settings?action=unsubscribe",
            WorkPathClass::Action,
        ),
        (
            "https://mail.example.com/confirmation-notes",
            WorkPathClass::Page,
        ),
    ] {
        assert_eq!(path_class(url), class, "{url}");
    }

    // Tabs: host and path only, readable by the agent, and private context.
    let tab =
        WorkContextTabV1::from_page("Inbox (3)", "https://mail.example.com/u/0/?tab=rm#inbox")
            .unwrap();
    assert_eq!(
        (tab.host.as_str(), tab.path.as_str()),
        ("mail.example.com", "/u/0/")
    );
    assert!(WorkContextTabV1::from_page("Local", "http://intranet.example/").is_none());
    // A session is a closed fact beside the tab, absent from the wire when false.
    assert!(!tab.signed_in);
    assert!(!serde_json::to_string(&tab).unwrap().contains("signed_in"));
    let signed = WorkContextTabV1 {
        signed_in: true,
        ..tab.clone()
    };
    let wire = serde_json::to_string(&signed).unwrap();
    assert!(wire.contains("\"signed_in\":true"));
    assert_eq!(
        serde_json::from_str::<WorkContextTabV1>(&wire).unwrap(),
        signed
    );
    assert!(WorkContextSelectionV1 {
        environment: 1.into(),
        items: vec![],
        tabs: true,
        blocks: false,
    }
    .validate()
    .is_ok());
    let disclosure = WorkContextDisclosureV1 {
        version: 1,
        environment: 1.into(),
        environment_revision: WorkRevision::INITIAL,
        purpose: WorkContextPurpose::Agent,
        items: vec![],
        total_bytes: 0,
        tabs: vec![tab.clone(); MAX_CONTEXT_TABS],
        blocks: false,
    };
    disclosure.validate().unwrap();
    assert!(disclosure.requires_review());
    assert!(WorkContextDisclosureV1 {
        tabs: vec![tab.clone(); MAX_CONTEXT_TABS + 1],
        ..disclosure.clone()
    }
    .validate()
    .is_err());

    let turn = WorkAgentTurnDisclosure::try_new(
        "Summarize my inbox",
        vec![],
        vec![],
        &[],
        &[],
        &[],
        WorkAgentBudget {
            turns_left: 3,
            steps_left: 8,
            browse_available: true,
        },
        limits,
        vec![],
    )
    .unwrap()
    .with_sites(vec![
        WorkAgentSiteView {
            site: "example.com".into(),
            session: "yours",
        },
        WorkAgentSiteView {
            site: "example.org".into(),
            session: "private",
        },
    ])
    .unwrap()
    .with_tabs(&[WorkContextTabV1::from_page("Docs", "https://docs.example.com/guide").unwrap()])
    .unwrap();
    let read_fetch = |url: &str| WorkAgentTurnOutput {
        say: None,
        artifacts: vec![],
        fetch: vec![WorkAgentFetch::Read {
            url: url.into(),
            collection: None,
        }],
        ask: None,
        finish: false,
        followups: vec![],
        malformed: 0,
    };
    for readable in [
        "https://mail.example.com/u/0/inbox",
        "https://docs.example.com/guide",
    ] {
        let admitted = turn.resolve(read_fetch(readable)).ok().unwrap();
        assert_eq!(admitted.fetch.len(), 1, "{readable}");
    }
    let refused = turn
        .resolve(read_fetch("https://public.example.org/other"))
        .ok()
        .unwrap();
    assert!(refused.fetch.is_empty());
    let browse = |start: &str| WorkAgentTurnOutput {
        say: None,
        artifacts: vec![],
        fetch: vec![WorkAgentFetch::Browse {
            start: start.into(),
            goal: " Read #design since Monday ".into(),
            collection: None,
        }],
        ask: None,
        finish: false,
        followups: vec![],
        malformed: 0,
    };
    let started = turn.resolve(browse("slack.com")).ok().unwrap();
    assert_eq!(
        started.fetch,
        [WorkStepKindV1::Read {
            verbatim: None,
            url: "https://slack.com/".into(),
            collection: None,
            goal: Some("Read #design since Monday".into()),
        }]
    );
    let page = turn
        .resolve(browse("https://app.slack.com/client"))
        .ok()
        .unwrap();
    assert_eq!(page.fetch.len(), 1);
    for refused in ["http://slack.com/", "ftp://slack.com", "not a site"] {
        assert!(
            turn.resolve(browse(refused)).ok().unwrap().fetch.is_empty(),
            "{refused}"
        );
    }
    let mut long = read(9, "https://slack.com/", None);
    long.kind = WorkStepKindV1::Read {
        verbatim: None,
        url: "https://slack.com/".into(),
        collection: None,
        goal: Some("x".repeat(MAX_WORK_PAGE_GOAL_BYTES + 1)),
    };
    assert!(long.validate().is_err());
    let context = serde_json::to_value(turn.context()).unwrap();
    assert_eq!(context["sites"][0]["session"], "yours");
    assert_eq!(context["tabs"][0]["url"], "https://docs.example.com/guide");
}

#[test]
fn a_held_site_step_round_trips_and_its_decision_matches_its_status() {
    use super::runtime::*;
    let confirm = WorkConfirmV1 {
        site: "slack.com".into(),
        category: WorkConfirmCategoryV1::Communication,
        headline: "Send to #design as you?".into(),
        action: "press Send".into(),
        text: Some("On my way".into()),
        facts: vec![WorkConfirmFactV1 {
            label: "Channel".into(),
            value: "#design".into(),
        }],
        page: Some(7u128.into()),
        provenance: vec!["notion.so".into()],
        run_option: false,
        decision: None,
    };
    let mut step = WorkStepFact {
        model_fault: None,
        part: None,
        id: 8u128.into(),
        turn: 1,
        kind: WorkStepKindV1::Confirm {
            confirm: Box::new(confirm.clone()),
        },
        status: WorkStepStatus::Running,
        usage: None,
        artifacts: vec![],
        evidence: None,
        note: None,
        measurements: None,
        local: None,
        account: None,
    };
    step.validate().unwrap();
    let json = serde_json::to_value(&step.kind).unwrap();
    assert_eq!(json["kind"], "confirm");
    assert_eq!(json["confirm"]["headline"], "Send to #design as you?");
    assert_eq!(
        serde_json::from_value::<WorkStepKindV1>(json).unwrap(),
        step.kind
    );
    step.status = WorkStepStatus::Succeeded;
    assert!(step.validate().is_err());
    for (decision, status, valid) in [
        (
            WorkConfirmDecisionV1::Approved,
            WorkStepStatus::Succeeded,
            true,
        ),
        (
            WorkConfirmDecisionV1::Approved,
            WorkStepStatus::Failed,
            true,
        ),
        (
            WorkConfirmDecisionV1::Declined,
            WorkStepStatus::Cancelled,
            true,
        ),
        (
            WorkConfirmDecisionV1::Declined,
            WorkStepStatus::Succeeded,
            false,
        ),
        (
            WorkConfirmDecisionV1::AllowedForRun,
            WorkStepStatus::Succeeded,
            false,
        ),
    ] {
        let mut decided = confirm.clone();
        decided.decision = Some(decision);
        step.kind = WorkStepKindV1::Confirm {
            confirm: Box::new(decided),
        };
        step.status = status;
        assert_eq!(step.validate().is_ok(), valid, "{decision:?} {status:?}");
    }
    let mut edit = confirm;
    edit.category = WorkConfirmCategoryV1::Edit;
    edit.run_option = true;
    edit.decision = Some(WorkConfirmDecisionV1::AllowedForRun);
    step.kind = WorkStepKindV1::Confirm {
        confirm: Box::new(edit),
    };
    step.validate().unwrap();
    let approve = WorkRuntimeIntent::ApproveStep {
        execution: WorkExecutionId::generate(),
        step: 9u128.into(),
        approve: true,
        for_run: false,
    };
    let json = serde_json::to_value(&approve).unwrap();
    assert!(json.get("for_run").is_none());
    let approve: WorkRuntimeIntent = serde_json::from_value(json).unwrap();
    assert!(matches!(
        approve,
        WorkRuntimeIntent::ApproveStep { for_run: false, .. }
    ));
}

#[test]
fn token_or_operation_overflow_does_not_invent_model_charges() {
    use super::runtime::*;
    let limits = WorkExecutionLimits {
        model_tokens: 100,
        cost_micro_usd: 1000,
        operations: 2,
        timeout_seconds: 60,
        max_workers: 1,
    };
    for cost in [0, 125] {
        let bounded = WorkUsage {
            model_tokens: 101,
            cost_micro_usd: cost,
            operations: 3,
            accounting: WorkUsageAccounting::Exact,
        }
        .bounded_by(limits);
        assert_eq!(bounded.model_tokens, 100);
        assert_eq!(bounded.cost_micro_usd, cost);
        assert_eq!(bounded.operations, 2);
        assert_eq!(
            bounded.accounting,
            WorkUsageAccounting::ConservativeReservation
        );
    }
}
