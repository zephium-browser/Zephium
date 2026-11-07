//! A lead run end to end over a scripted model: parts in parallel, found
//! things placed per part, a plan pointing at them, a corrected fault, the
//! reply, and a follow-up that revises instead of adding.
use super::work_planning_tests::{drive, fixture};
use crate::work_agent::{WorkAgentBrowseRequest, WorkBrowserOutcome};
use crate::work_lead::{LeadModel, WorkLeadModels, WorkLeadService};
use crate::work_runtime::WorkArtifactDraft;
use crate::WorkIntent;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use zephium_core::work::{artifact::*, model::*, parts::*, runtime::*, search::*, *};
use zephium_ipc::work::WorkCommandV1;

struct Script {
    calls: Mutex<Vec<String>>,
}

fn call(id: &str, name: &str, arguments: Value) -> WorkModelPart {
    WorkModelPart::ToolCall(WorkModelToolCall {
        id: id.into(),
        name: name.into(),
        arguments,
    })
}
fn results(request: &WorkModelRequest) -> String {
    request
        .messages
        .iter()
        .filter_map(|m| match m {
            WorkModelMessage::ToolResults(results) => Some(
                results
                    .iter()
                    .map(|r| r.content.clone())
                    .collect::<Vec<_>>()
                    .join("\n"),
            ),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}
fn first_key(text: &str) -> String {
    let at = text.find("[s").expect("a source key");
    let end = text[at..].find(']').unwrap();
    text[at + 1..at + end].split(',').next().unwrap().to_owned()
}

impl Script {
    fn lead(&self, request: &WorkModelRequest, turn: usize) -> Vec<WorkModelPart> {
        let seen = results(request);
        let follow_up = request.messages.iter().any(|m| match m {
            WorkModelMessage::User(parts) => parts
                .iter()
                .any(|p| matches!(p, WorkModelPart::Text(t) if t.contains("Earlier requests"))),
            _ => false,
        });
        if follow_up {
            return match turn {
                0 => vec![call("r1", "read_canvas", json!({}))],
                1 => {
                    let plan = seen
                        .lines()
                        .find(|l| l.contains(" plan "))
                        .and_then(|l| l.split_whitespace().nth(1))
                        .unwrap()
                        .to_owned();
                    vec![
                        call(
                            "r2",
                            "revise",
                            json!({"id": plan, "data": {"steps": [
                                {"title": "Fly home early", "kind": "travel", "when": "Fri 10 Jan"}
                            ]}}),
                        ),
                        call(
                            "r3",
                            "create",
                            json!({"kind": "reply", "title": "Shorter trip", "data": {
                            "headline": "Home a day early", "text": "The plan now ends on Friday."}}),
                        ),
                    ]
                }
                // This update depends on r2's durable revision. A later model
                // turn observes that settlement instead of racing its read.
                2 => vec![call(
                    "r5",
                    "create",
                    json!({"kind": "plan", "title": "Your trip, shorter", "data": {"steps": [
                        {"title": "Fly home early", "kind": "travel"}]}}),
                )],
                3 => {
                    // A second plan on the same subject revises the first.
                    assert!(!seen.contains("already covers this subject"), "{seen}");
                    vec![call(
                        "r4",
                        "finish",
                        json!({"say": "The plan ends a day earlier."}),
                    )]
                }
                _ => vec![call(
                    "r4",
                    "finish",
                    json!({"say": "The plan ends a day earlier."}),
                )],
            };
        }
        match turn {
            0 => vec![
                WorkModelPart::Text("Starting the stay and entry parts.".into()),
                call("a", "load_skill", json!({"name": "trip-planning"})),
                call(
                    "b",
                    "start_part",
                    json!({"title": "Stay", "helper": "browser",
                    "goal": "Two homes near the YC office for 6–12 January",
                    "brief": "2 guests, under $300 a night", "service": "airbnb.com"}),
                ),
                call(
                    "c",
                    "start_part",
                    json!({"title": "Entry", "helper": "research",
                    "goal": "What a Polish citizen needs to enter the US"}),
                ),
            ],
            1 => {
                assert!(seen.contains("Part Stay"), "{seen}");
                assert!(seen.contains("Part Entry"), "{seen}");
                let picks = seen
                    .lines()
                    .find(|l| l.contains(" picks \""))
                    .and_then(|l| l.split_whitespace().nth(1))
                    .expect("the Stay part's picks")
                    .to_owned();
                vec![
                    call(
                        "d",
                        "create",
                        json!({"kind": "plan", "title": "Your trip", "data": {
                        "steps": [
                            {"title": "Stay in the Mission", "kind": "stay", "when": "6–12 Jan",
                             "pick": {"artifact": picks, "index": 0}},
                            {"title": "Apply for ESTA", "kind": "task", "when": "Before 3 Jan"}
                        ],
                        "total": {"label": "Estimated total", "value": "$1,740"}}}),
                    ),
                    call(
                        "e",
                        "create",
                        json!({"kind": "reply", "title": "Your trip", "data": {
                        "headline": "x".repeat(90), "text": "Six nights in the Mission."}}),
                    ),
                ]
            }
            2 => {
                // An overlong headline is clipped between words, not refused.
                assert!(
                    !seen.contains("headline is one line of 1 to 80 characters"),
                    "{seen}"
                );
                vec![call(
                    "f",
                    "create",
                    json!({"kind": "reply", "title": "Your trip", "data": {
                    "headline": "Six nights in the Mission, $1,740", "text": "Apply for **ESTA** first."}}),
                )]
            }
            _ => vec![call(
                "g",
                "finish",
                json!({"say": "Your trip is on the canvas.",
                "followups": ["Book the Mission loft"], "title": "YC trip from Warsaw."}),
            )],
        }
    }
    fn browser(&self, request: &WorkModelRequest, turn: usize) -> Vec<WorkModelPart> {
        match turn {
            0 => vec![call(
                "h1",
                "browse",
                json!({"start": "airbnb.com",
                "goal": "Homes near the YC office for 6–12 January, 2 guests",
                "records": {"title": "Homes", "max_items": 2, "columns": [
                    {"name": "price", "value": {"kind": "text"}, "required": false},
                    {"name": "photo", "value": {"kind": "image_url"}, "required": false}]}}),
            )],
            1 => {
                let seen = results(request);
                let key = first_key(&seen);
                vec![call(
                    "h2",
                    "create",
                    json!({"kind": "picks", "title": "Homes near YC", "data": {
                    "facet": "stay", "items": [
                        {"name": "Mission loft", "price": {"display": "$290 / night", "amount": 290, "currency": "USD"},
                         "image_candidates": ["https://a0.muscache.com/im/pictures/loft.jpg"],
                         "recommended": true, "source": key},
                        {"name": "SoMa studio", "price": {"display": "$240 / night"}, "source": key}]},
                    "sources": [key]}),
                )]
            }
            _ => vec![call(
                "h3",
                "finish",
                json!({"summary": "2 homes", "digest": "Mission loft $290/night; SoMa studio $240/night."}),
            )],
        }
    }
    fn research(&self, request: &WorkModelRequest, turn: usize) -> Vec<WorkModelPart> {
        match turn {
            0 => vec![call(
                "e1",
                "web_search",
                json!({"query": "US entry requirements Polish citizens ESTA"}),
            )],
            1 => {
                let key = first_key(&results(request));
                vec![call(
                    "e2",
                    "create",
                    json!({"kind": "list", "title": "Entry needs", "data": {
                    "style": "requirements", "items": [{"title": "ESTA approved before you fly", "source": key}]},
                    "sources": [key]}),
                )]
            }
            _ => {
                let seen = results(request);
                assert!(seen.contains("finish's digest"), "{seen}");
                let key = first_key(&seen);
                vec![call(
                    "e3",
                    "finish",
                    json!({"summary": "Entry needs", "digest": format!("ESTA required [{key}]")}),
                )]
            }
        }
    }
}

impl WorkModelClient for Script {
    fn call<'a>(
        &'a self,
        request: WorkModelRequest,
        _: &'a (dyn Fn(WorkModelEvent) + Send + Sync),
    ) -> WorkModelFuture<'a> {
        Box::pin(async move {
            let turn = request
                .messages
                .iter()
                .filter(|m| matches!(m, WorkModelMessage::Assistant(_)))
                .count();
            let system = request
                .system
                .iter()
                .map(|b| b.text.as_str())
                .collect::<String>();
            let (who, assistant) = if system.contains("You are the Work agent") {
                ("lead", self.lead(&request, turn))
            } else if system.contains("You work on web pages") {
                ("browser", self.browser(&request, turn))
            } else {
                ("research", self.research(&request, turn))
            };
            self.calls.lock().unwrap().push(format!("{who}:{turn}"));
            Ok(WorkModelOutcome {
                stop: WorkModelStop::ToolUse,
                usage: WorkModelUsage {
                    input_tokens: 2_000,
                    cached_input_tokens: 1_000,
                    cache_write_input_tokens: 0,
                    output_tokens: 200,
                    reasoning_tokens: 0,
                    cost_micros: None,
                },
                assistant,
            })
        })
    }
}

/// The chosen model's own search, as most runs now use it.
const SEARCH_MODEL: &str = "anthropic/claude-haiku-4-5-20251001";
struct Search;

struct BurstSearches(Script);
impl WorkModelClient for BurstSearches {
    fn call<'a>(
        &'a self,
        request: WorkModelRequest,
        event: &'a (dyn Fn(WorkModelEvent) + Send + Sync),
    ) -> WorkModelFuture<'a> {
        Box::pin(async move {
            let system: String = request
                .system
                .iter()
                .map(|block| block.text.as_str())
                .collect();
            let first = !request
                .messages
                .iter()
                .any(|message| matches!(message, WorkModelMessage::Assistant(_)));
            let research = !system.contains("You are the Work agent")
                && !system.contains("You work on web pages");
            let mut outcome = self.0.call(request, event).await?;
            if first && research {
                outcome.assistant = [
                    "ESTA eligibility nationality",
                    "ESTA processing times",
                    "ESTA application fees",
                ]
                .into_iter()
                .enumerate()
                .map(|(index, query)| {
                    call(
                        &format!("burst-{index}"),
                        "web_search",
                        json!({"query":query}),
                    )
                })
                .collect();
            }
            Ok(outcome)
        })
    }
}

struct SlowSearch(std::sync::atomic::AtomicUsize);

#[tokio::test]
async fn a_finished_reply_keeps_an_unsettled_approved_write_unknown() {
    let store = Arc::new(zephium_store::SqliteStore::in_memory().unwrap());
    let (mut shell, queue, handle, profile) = fixture(store);
    let create = handle
        .work_document(WorkIntent::Create {
            objective: "Plan my YC batch trip from Warsaw".into(),
        })
        .unwrap();
    let work = create.work_id().unwrap();
    drive(&mut shell, &queue, create).await.unwrap();
    let client = Arc::new(Script {
        calls: Mutex::new(Vec::new()),
    });
    let models = WorkLeadModels {
        lead: model(client.clone()),
        page: model(client.clone()),
        light: model(client),
    };
    let injected = std::sync::atomic::AtomicBool::new(false);
    let service = WorkLeadService::new(handle);
    let result = drive(
        &mut shell,
        &queue,
        service.run(
            profile,
            command(work, WorkRevision::INITIAL),
            None,
            models,
            &Search,
            |probe, request| {
                let first = !injected.swap(true, std::sync::atomic::Ordering::SeqCst);
                async move {
                    if first {
                        // A native write was approved but its settlement was lost.
                        // Only its durable fact is injected; no file is touched.
                        probe
                            .commit_step(WorkRuntimeUpdate::BeginStep {
                                execution: probe.execution(),
                                attempt: probe.attempt(),
                                step: WorkStepFact {
                                    model_fault: None,
                                    id: WorkStepId::generate(),
                                    turn: 1,
                                    kind: WorkStepKindV1::WriteFile {
                                        path: "/Users/person/notes.txt".into(),
                                        content: "Approved draft".into(),
                                        decision: Some(true),
                                    },
                                    status: WorkStepStatus::Running,
                                    usage: None,
                                    artifacts: vec![],
                                    evidence: None,
                                    note: None,
                                    measurements: None,
                                    local: None,
                                    account: None,
                                    part: None,
                                },
                                artifacts: vec![],
                                evidence: None,
                                file: None,
                            })
                            .await?;
                    }
                    page(request).await
                }
            },
            |_| {},
        ),
    )
    .await
    .unwrap();
    let execution = &result.executions[0];
    assert!(execution
        .artifacts
        .iter()
        .any(|artifact| matches!(artifact.data, WorkArtifactDataV1::Reply { .. })));
    assert!(execution
        .steps
        .iter()
        .any(|step| matches!(step.kind, WorkStepKindV1::Finish { .. })));
    assert_eq!(execution.status, WorkExecutionStatus::Interrupted);
    assert!(execution.attempts[0].usage.is_none());
    assert!(execution
        .steps
        .iter()
        .filter(|step| matches!(step.kind, WorkStepKindV1::WriteFile { .. }))
        .all(|step| step.status == WorkStepStatus::OutcomeUnknown));
}
impl WorkPublicSearchProvider for SlowSearch {
    fn grant(&self) -> Option<(WorkSearchProvider, String)> {
        Search.grant()
    }
    fn search<'a>(
        &'a self,
        scope: &'a WorkPublicSearchScope,
        context: &'a [zephium_core::work::context::WorkContextBody],
        limits: WorkExecutionLimits,
    ) -> WorkPublicSearchFuture<'a> {
        Box::pin(async move {
            self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            Search.search(scope, context, limits).await
        })
    }
}

#[tokio::test]
async fn concurrent_parts_drain_partial_search_batches_before_retrying_worker_capacity() {
    let store = Arc::new(zephium_store::SqliteStore::in_memory().unwrap());
    let (mut shell, queue, handle, profile) = fixture(store);
    let create = handle
        .work_document(WorkIntent::Create {
            objective: "Plan my YC batch trip from Warsaw".into(),
        })
        .unwrap();
    let work = create.work_id().unwrap();
    drive(&mut shell, &queue, create).await.unwrap();
    let client = Arc::new(BurstSearches(Script {
        calls: Mutex::new(Vec::new()),
    }));
    let models = WorkLeadModels {
        lead: model(client.clone()),
        page: model(client.clone()),
        light: model(client),
    };
    let search = SlowSearch(std::sync::atomic::AtomicUsize::new(0));
    let mut request = command(work, WorkRevision::INITIAL);
    let WorkRuntimeIntent::BeginAgent { limits, .. } = &mut request.intent else {
        panic!()
    };
    limits.max_workers = 2;
    let service = WorkLeadService::new(handle);
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        drive(
            &mut shell,
            &queue,
            service.run(
                profile,
                request,
                None,
                models,
                &search,
                |_, request| async {
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                    page(request).await
                },
                |_| {},
            ),
        ),
    )
    .await
    .expect("no admission deadlock")
    .unwrap();
    let execution = &result.executions[0];
    assert!(
        matches!(
            execution.status,
            WorkExecutionStatus::Completed | WorkExecutionStatus::NeedsReview
        ),
        "{:?}",
        execution.status
    );
    assert_eq!(search.0.load(std::sync::atomic::Ordering::SeqCst), 3);
    let searches: Vec<_> = execution
        .steps
        .iter()
        .filter(|step| matches!(step.kind, WorkStepKindV1::Search { .. }))
        .collect();
    assert_eq!(searches.len(), 3);
    assert!(searches
        .iter()
        .all(|step| step.status == WorkStepStatus::Succeeded));
    assert!(execution
        .steps
        .iter()
        .all(|step| step.status != WorkStepStatus::Running));
}
impl WorkPublicSearchProvider for Search {
    fn grant(&self) -> Option<(WorkSearchProvider, String)> {
        Some((WorkSearchProvider::Native, SEARCH_MODEL.into()))
    }

    fn search<'a>(
        &'a self,
        scope: &'a WorkPublicSearchScope,
        _: &'a [zephium_core::work::context::WorkContextBody],
        _: WorkExecutionLimits,
    ) -> WorkPublicSearchFuture<'a> {
        Box::pin(async move {
            assert!(scope.query.contains("ESTA"));
            assert_eq!(scope.provider, WorkSearchProvider::Native);
            assert_eq!(scope.model, SEARCH_MODEL);
            Ok(WorkPublicSearchResult {
                evidence: WorkProviderSearchEvidenceV1 {
                    version: 1,
                    provider: WorkSearchProvider::Native,
                    model: SEARCH_MODEL.into(),
                    response_model: SEARCH_MODEL.into(),
                    response_id: "zr_entry".into(),
                    search_call_id: "zs_entry".into(),
                    answer: "Polish citizens travel on ESTA [1].".into(),
                    citations: vec![WorkProviderSearchCitation {
                        url: "https://esta.cbp.dhs.gov/".into(),
                        title: "Official ESTA Application".into(),
                        start_index: 0,
                        end_index: 10,
                        snippet: None,
                    }],
                    actual_input_tokens: 200,
                    actual_output_tokens: 100,
                },
                usage: WorkUsage {
                    model_tokens: 300,
                    cost_micro_usd: 1_000,
                    operations: 1,
                    accounting: WorkUsageAccounting::Exact,
                },
            })
        })
    }
}

pub(super) fn model(client: Arc<dyn WorkModelClient>) -> LeadModel {
    LeadModel {
        entry: WorkModelEntry {
            id: "openai/gpt-6".into(),
            model: WorkModelRef {
                provider: WorkModelProvider::OpenAi,
                wire: WorkModelWire::OpenAiResponses,
                model: "gpt-6".into(),
            },
            display_name: "GPT-6".into(),
            roles: vec![WorkModelRole::Lead],
            recommended: true,
            context_window: 400_000,
            max_output: 32_000,
            supports: WorkModelSupports {
                tools: true,
                vision: true,
                prompt_cache: true,
                reasoning: true,
                native_search: true,
            },
            price: Some(WorkModelPrice {
                input: 2_000_000,
                cached_input: 200_000,
                output: 8_000_000,
            }),
        },
        client,
    }
}

pub(super) fn command(work: WorkId, expected: WorkRevision) -> WorkCommandV1 {
    WorkCommandV1 {
        version: 1,
        work,
        expected_revision: expected,
        command: WorkCommandId::generate(),
        intent: WorkRuntimeIntent::BeginAgent {
            grant: WorkAgentGrantV1 {
                no_usage_charge: false,
                draft: None,
                provider: WorkSearchProvider::OpenAi,
                model: PUBLIC_SEARCH_MODEL.into(),
                max_turns: 10,
                max_steps: 32,
                browse_hops: 4,
                folders: vec![],
                accounts: vec![],
                private: false,
                lead: None,
                skill: None,
                search_fallback: None,
            },
            limits: WorkExecutionLimits {
                model_tokens: 1_000_000,
                cost_micro_usd: 3_000_000,
                operations: 256,
                timeout_seconds: 120,
                max_workers: 4,
            },
        },
    }
}

async fn page(request: WorkAgentBrowseRequest) -> Result<WorkBrowserOutcome, WorkError> {
    let WorkStepKindV1::Read { goal, .. } = &request.step else {
        panic!("a page step");
    };
    assert!(goal.as_deref().is_some_and(|g| g.contains("YC office")));
    let subject = |name: &str, image: &str| WorkSubject {
        name: name.into(),
        descriptor: Some("Entire home".into()),
        homepage: Some(format!("https://www.airbnb.com/rooms/{}", name.len())),
        record_media: None,
        media_attempted: false,
        image_candidates: vec![image.into()],
    };
    Ok(WorkBrowserOutcome {
        status: WorkStepStatus::Succeeded,
        usage: Some(WorkUsage {
            model_tokens: 5_000,
            cost_micro_usd: 20_000,
            operations: 4,
            accounting: WorkUsageAccounting::Exact,
        }),
        artifacts: vec![WorkArtifactDraft {
            output: request.output,
            title: "Homes".into(),
            data: WorkArtifactDataV1::Findings {
                subjects: vec![
                    subject(
                        "Mission loft",
                        "https://a0.muscache.com/im/pictures/loft.jpg",
                    ),
                    subject(
                        "SoMa studio",
                        "https://a0.muscache.com/im/pictures/studio.jpg",
                    ),
                ],
                items: vec![WorkFinding {
                    claim: "Mission loft is $290 a night".into(),
                    subject: Some(0),
                    evidence: vec![0],
                    confidence: WorkConfidence::Supported,
                    detail: None,
                    general_knowledge: false,
                }],
            },
            evidence: vec![WorkEvidenceLink {
                extraction_id: WorkArtifactId::from(900),
                source_id: 1,
            }],
        }],
        intervention: None,
        note: None,
        measurements: None,
        helped: false,
        held_back: false,
        committed: Vec::new(),
        rerun: false,
        search: None,
        redirect: None,
    })
}

#[tokio::test]
async fn a_lead_run_splits_into_parts_builds_the_result_and_revises_on_follow_up() {
    let store = Arc::new(zephium_store::SqliteStore::in_memory().unwrap());
    let (mut shell, queue, handle, profile) = fixture(store);
    let create = handle
        .work_document(WorkIntent::Create {
            objective: "Plan my YC batch trip from Warsaw".into(),
        })
        .unwrap();
    let work = create.work_id().unwrap();
    drive(&mut shell, &queue, create).await.unwrap();
    let script = Arc::new(Script {
        calls: Mutex::new(Vec::new()),
    });
    let models = WorkLeadModels {
        lead: model(script.clone()),
        page: model(script.clone()),
        light: model(script.clone()),
    };
    let service = WorkLeadService::new(handle.clone());
    let first = drive(
        &mut shell,
        &queue,
        service.run(
            profile,
            command(work, WorkRevision::INITIAL),
            None,
            models.clone(),
            &Search,
            |_, request| page(request),
            |_| {},
        ),
    )
    .await
    .unwrap();
    let run = &first.executions[0];
    assert!(
        matches!(
            run.status,
            WorkExecutionStatus::NeedsReview | WorkExecutionStatus::Completed
        ),
        "{:?} {:?}",
        run.status,
        run.steps
            .iter()
            .map(|s| (s.status, s.note.clone()))
            .collect::<Vec<_>>()
    );
    assert!(run.agent_grant().unwrap().lead.is_some());
    let titles: Vec<(&str, WorkPartStateV1, Option<&str>)> = run
        .parts
        .iter()
        .map(|p| (p.title.as_str(), p.state, p.summary.as_deref()))
        .collect();
    assert!(
        titles.contains(&("Stay", WorkPartStateV1::Done, Some("2 homes"))),
        "{titles:?}"
    );
    assert!(
        titles.contains(&("Entry", WorkPartStateV1::Done, Some("Entry needs"))),
        "{titles:?}"
    );
    assert_eq!(run.inputs[0].kind, WorkInputKindV1::Skill);
    assert_eq!(run.inputs[0].label, "Trip planning");
    let stay = run.parts.iter().find(|p| p.title == "Stay").unwrap().id;
    let page_step = run
        .steps
        .iter()
        .find(|s| matches!(&s.kind, WorkStepKindV1::Read { goal: Some(_), .. }))
        .unwrap();
    assert_eq!(page_step.part, Some(stay));
    let object = |kind: &str| {
        run.artifacts
            .iter()
            .find(|a| a.data.kind_name() == kind)
            .unwrap_or_else(|| panic!("a {kind}"))
    };
    let picks = object("picks");
    assert_eq!(picks.part, Some(stay));
    assert!(!picks.evidence.is_empty());
    let WorkArtifactDataV1::Plan { steps, .. } = &object("plan").data else {
        panic!()
    };
    assert_eq!(steps[0].pick.as_ref().unwrap().artifact, picks.id);
    assert_eq!(object("plan").part, None);
    // The second reply revised the first instead of being refused.
    assert_eq!(
        run.artifacts
            .iter()
            .filter(|a| a.data.kind_name() == "reply" && a.revises.is_none())
            .count(),
        1
    );
    // A research part hands its facts to the lead; it places no list.
    let entry = run.parts.iter().find(|p| p.title == "Entry").unwrap().id;
    assert!(!run.artifacts.iter().any(|a| a.part == Some(entry)));
    let finish = run.steps.last().unwrap();
    assert!(
        matches!(&finish.kind, WorkStepKindV1::Finish { followups, .. } if followups.len() == 1)
    );
    assert_eq!(finish.note.as_deref(), Some("Your trip is on the canvas."));
    assert_eq!(run.title.as_deref(), Some("YC trip from Warsaw"));
    let calls = script.calls.lock().unwrap().clone();
    assert!(
        calls.contains(&"browser:2".into()) && calls.contains(&"research:2".into()),
        "{calls:?}"
    );

    let second = drive(
        &mut shell,
        &queue,
        service.run(
            profile,
            command(work, first.work.revision),
            None,
            models,
            &Search,
            |_, request| page(request),
            |_| {},
        ),
    )
    .await
    .unwrap();
    let follow_up = second.executions.last().unwrap();
    assert!(follow_up.parts.is_empty());
    let plans: Vec<_> = follow_up
        .artifacts
        .iter()
        .filter(|a| a.data.kind_name() == "plan")
        .collect();
    // Both settled updates remain immutable history; only the newest stands.
    assert_eq!(plans.len(), 2);
    let (revised, latest) = (plans[0], plans[1]);
    assert_eq!(revised.revises, Some(object("plan").id));
    assert_eq!(revised.title, "Your trip");
    assert_eq!(latest.revises, Some(revised.id));
    let canvas = crate::work_lead::objects::canvas(&second, follow_up.id);
    let current: Vec<_> = canvas
        .iter()
        .filter(|object| object.current && object.artifact.data.kind_name() == "plan")
        .collect();
    assert_eq!(current.len(), 1);
    assert_eq!(current[0].artifact.id, latest.id);
    assert_eq!(current[0].artifact.title, "Your trip");
    let WorkArtifactDataV1::Plan { steps, .. } = &current[0].artifact.data else {
        panic!("the current plan")
    };
    assert_eq!(steps.len(), 1);
    assert_eq!(steps[0].title, "Fly home early");
    assert_eq!(steps[0].when, None);
}

/// A part whose site fails says what it needs; the lead may not stand a
/// figure or a to-do in for what was not found.
struct Honest;
impl WorkModelClient for Honest {
    fn call<'a>(
        &'a self,
        request: WorkModelRequest,
        _: &'a (dyn Fn(WorkModelEvent) + Send + Sync),
    ) -> WorkModelFuture<'a> {
        Box::pin(async move {
            let turn = request
                .messages
                .iter()
                .filter(|m| matches!(m, WorkModelMessage::Assistant(_)))
                .count();
            let lead = request
                .system
                .iter()
                .any(|b| b.text.contains("You are the Work agent"));
            let seen = results(&request);
            let assistant = match (lead, turn) {
                (true, 0) => vec![call(
                    "p",
                    "start_part",
                    json!({"title": "Flights", "helper": "browser", "service": "kayak.com",
                    "goal": "Round trips WAW to SFO for 5 January 2027"}),
                )],
                (true, 1) => {
                    assert!(seen.contains("could not do its job"), "{seen}");
                    assert!(seen.contains("another try on kayak.com"), "{seen}");
                    vec![
                        call(
                            "r",
                            "create",
                            json!({"kind": "reply", "title": "Flights", "data": {
                            "headline": "Airfare recheck complete", "text": "No fare was found.",
                            "figures": [{"label": "Exact-date fare", "value": "Not verified"}]}}),
                        ),
                        call(
                            "l",
                            "create",
                            json!({"kind": "list", "title": "To do", "data": {"style": "todo",
                            "items": [{"title": "Retry the Kayak flights check"}]}}),
                        ),
                    ]
                }
                (true, 2) => {
                    assert!(seen.contains("figure 1 stands for something"), "{seen}");
                    assert!(seen.contains("list: item 1 is about work"), "{seen}");
                    vec![call(
                        "r2",
                        "create",
                        json!({"kind": "reply", "title": "Flights", "data": {
                        "headline": "No flights found yet",
                        "text": "Kayak did not load; try again from the Flights row."}}),
                    )]
                }
                (true, _) => vec![call("f", "finish", json!({"say": "No flights yet."}))],
                (false, _) => vec![call(
                    "h",
                    "finish",
                    json!({"summary": "Kayak did not load", "digest": "The results page failed.",
                    "found": false, "need": {"kind": "retry", "target": "https://www.kayak.com/flights"}}),
                )],
            };
            Ok(WorkModelOutcome {
                stop: WorkModelStop::ToolUse,
                usage: WorkModelUsage {
                    input_tokens: 1_000,
                    cached_input_tokens: 0,
                    cache_write_input_tokens: 0,
                    output_tokens: 100,
                    reasoning_tokens: 0,
                    cost_micros: None,
                },
                assistant,
            })
        })
    }
}

#[tokio::test]
async fn a_part_that_cannot_do_its_job_carries_its_fix_and_nothing_stands_for_it() {
    let store = Arc::new(zephium_store::SqliteStore::in_memory().unwrap());
    let (mut shell, queue, handle, profile) = fixture(store);
    let create = handle
        .work_document(WorkIntent::Create {
            objective: "Recheck flights WAW to SFO".into(),
        })
        .unwrap();
    let work = create.work_id().unwrap();
    drive(&mut shell, &queue, create).await.unwrap();
    let client = Arc::new(Honest);
    let lead = LeadModel {
        client,
        ..model(Arc::new(Script {
            calls: Mutex::new(Vec::new()),
        }))
    };
    let models = WorkLeadModels {
        lead: lead.clone(),
        page: lead.clone(),
        light: lead,
    };
    let done = drive(
        &mut shell,
        &queue,
        WorkLeadService::new(handle.clone()).run(
            profile,
            command(work, WorkRevision::INITIAL),
            None,
            models,
            &Search,
            |_, request| page(request),
            |_| {},
        ),
    )
    .await
    .unwrap();
    let run = &done.executions[0];
    let flights = &run.parts[0];
    assert_eq!(flights.state, WorkPartStateV1::Failed);
    assert_eq!(
        flights.need,
        Some(WorkPartNeedV1::Retry {
            host: Some("kayak.com".into()),
            reason: None,
        })
    );
    let kinds: Vec<&str> = run.artifacts.iter().map(|a| a.data.kind_name()).collect();
    assert_eq!(kinds, ["reply"]);
}

/// A Slack part when the person added a Slack MCP server: the run offers it
/// once, the part runs as a connection and calls the server's tools.
struct Connected;
impl WorkModelClient for Connected {
    fn call<'a>(
        &'a self,
        request: WorkModelRequest,
        _: &'a (dyn Fn(WorkModelEvent) + Send + Sync),
    ) -> WorkModelFuture<'a> {
        Box::pin(async move {
            let turn = request
                .messages
                .iter()
                .filter(|m| matches!(m, WorkModelMessage::Assistant(_)))
                .count();
            let system = request
                .system
                .iter()
                .map(|b| b.text.as_str())
                .collect::<String>();
            let seen = results(&request);
            let assistant = if system.contains("You are the Work agent") {
                match turn {
                    0 => vec![call(
                        "p",
                        "start_part",
                        json!({"title": "Slack", "helper": "browser", "service": "app.slack.com",
                        "goal": "Unread mentions since yesterday"}),
                    )],
                    1 => {
                        assert!(
                            seen.contains("Part Slack") && seen.contains("done"),
                            "{seen}"
                        );
                        vec![call(
                            "r",
                            "create",
                            json!({"kind": "reply", "title": "Today", "data": {
                            "headline": "One mention today", "text": "Ana asked about the deck."}}),
                        )]
                    }
                    _ => vec![call("f", "finish", json!({"say": "Slack is read."}))],
                }
            } else {
                assert!(system.contains("Connection helper"), "{system}");
                match turn {
                    0 => {
                        assert!(
                            request.tools.iter().any(|t| t.name == "slack__echo"),
                            "the server's tools are offered"
                        );
                        vec![call("e", "slack__echo", json!({"text": "mentions"}))]
                    }
                    _ => vec![call(
                        "h",
                        "finish",
                        json!({"summary": "1 mention", "digest": "Ana: the deck."}),
                    )],
                }
            };
            Ok(WorkModelOutcome {
                stop: WorkModelStop::ToolUse,
                usage: WorkModelUsage {
                    input_tokens: 1_000,
                    cached_input_tokens: 0,
                    cache_write_input_tokens: 0,
                    output_tokens: 100,
                    reasoning_tokens: 0,
                    cost_micros: None,
                },
                assistant,
            })
        })
    }
}

#[tokio::test]
#[ignore = "needs the zephium-mcp-fixture binary in ZEPHIUM_MCP_FIXTURE"]
async fn a_service_the_person_connected_is_offered_once_before_its_website() {
    use zephium_ipc::work::{WorkServerTransportV1, WorkServerV1};
    let program = std::env::var("ZEPHIUM_MCP_FIXTURE").expect("set ZEPHIUM_MCP_FIXTURE");
    let store = Arc::new(zephium_store::SqliteStore::in_memory().unwrap());
    let (mut shell, queue, handle, profile) = fixture(store);
    let data = std::env::temp_dir().join(format!("zephium-connected-{}", std::process::id()));
    crate::work_connections::store::install(&data);
    let server = WorkServerV1 {
        id: "slack".into(),
        name: "Slack".into(),
        transport: WorkServerTransportV1::Stdio {
            command: program,
            args: vec![],
            env: vec![],
        },
        enabled: true,
    };
    let connections = crate::work_connections::store::shared().unwrap();
    connections
        .put(&profile.to_string(), server.clone(), None)
        .unwrap();
    crate::work_connections::mcp::check(&profile.to_string(), &server).await;
    let create = handle
        .work_document(WorkIntent::Create {
            objective: "What do I need to do today in Slack?".into(),
        })
        .unwrap();
    let work = create.work_id().unwrap();
    drive(&mut shell, &queue, create).await.unwrap();
    let lead = LeadModel {
        client: Arc::new(Connected),
        ..model(Arc::new(Script {
            calls: Mutex::new(Vec::new()),
        }))
    };
    let models = WorkLeadModels {
        lead: lead.clone(),
        page: lead.clone(),
        light: lead,
    };
    let answering = {
        let handle = handle.clone();
        async move {
            let mut answered = 0;
            loop {
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                let Ok(request) = handle.work_projection(profile, work) else {
                    continue;
                };
                let zephium_ipc::work::WorkReplyV1::Projection { projection } =
                    request.response(profile).await.reply
                else {
                    continue;
                };
                let Some(execution) = projection.executions.last() else {
                    continue;
                };
                let intent = execution.steps.iter().find_map(|step| {
                    if step.status != WorkStepStatus::Running {
                        return None;
                    }
                    match &step.kind {
                        WorkStepKindV1::Ask {
                            options,
                            answer: None,
                            purpose,
                            ..
                        } => {
                            assert_eq!(*purpose, Some(WorkAskPurposeV1::Connection));
                            Some(WorkRuntimeIntent::AnswerStep {
                                execution: execution.id,
                                step: step.id,
                                answer: options[0].clone(),
                            })
                        }
                        WorkStepKindV1::Confirm { confirm } if confirm.decision.is_none() => {
                            assert_eq!(confirm.site, "slack");
                            assert_eq!(confirm.action, "run echo");
                            assert_eq!(confirm.headline, "Echo on Slack?");
                            assert!(!confirm.run_option);
                            assert_eq!(
                                serde_json::from_str::<Value>(confirm.text.as_deref().unwrap())
                                    .unwrap(),
                                json!({"text": "mentions"})
                            );
                            Some(WorkRuntimeIntent::ApproveStep {
                                execution: execution.id,
                                step: step.id,
                                approve: true,
                                for_run: false,
                            })
                        }
                        _ => None,
                    }
                });
                let Some(intent) = intent else {
                    continue;
                };
                answered += 1;
                let _ = handle
                    .work_command(
                        profile,
                        WorkCommandV1 {
                            version: 1,
                            work,
                            expected_revision: projection.work.revision,
                            command: WorkCommandId::generate(),
                            intent,
                        },
                    )
                    .unwrap()
                    .await;
                if answered > 3 {
                    return answered;
                }
            }
        }
    };
    let service = WorkLeadService::new(handle.clone());
    let done = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        drive(&mut shell, &queue, async {
            tokio::select! {
            result = service.run(
                profile,
                command(work, WorkRevision::INITIAL),
                None,
                models,
                &Search,
                |_, _| async { panic!("the Slack part never opens the website") },
                |_| {},
            ) => result,
            answered = answering => panic!("fixture responder stopped after {answered} answers"),
            }
        }),
    )
    .await
    .expect("the local fixture run must settle after its consent decisions")
    .unwrap();
    let run = &done.executions[0];
    let asks: Vec<&str> = run
        .steps
        .iter()
        .filter_map(|s| match &s.kind {
            WorkStepKindV1::Ask { prompt, .. } => Some(prompt.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(asks, ["Use your Slack connection, or work on the website?"]);
    let slack = &run.parts[0];
    assert_eq!(slack.helper, WorkHelperV1::Connection);
    assert_eq!(slack.state, WorkPartStateV1::Done);
    let confirmations: Vec<_> = run
        .steps
        .iter()
        .filter_map(|step| match &step.kind {
            WorkStepKindV1::Confirm { confirm } => Some(confirm),
            _ => None,
        })
        .collect();
    assert_eq!(confirmations.len(), 1);
    assert_eq!(
        confirmations[0].decision,
        Some(WorkConfirmDecisionV1::Approved)
    );
    let calls: Vec<_> = run
        .steps
        .iter()
        .filter_map(|step| match &step.kind {
            WorkStepKindV1::Call { call } => Some((step.status, call)),
            _ => None,
        })
        .collect();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].0, WorkStepStatus::Succeeded);
    assert_eq!(calls[0].1.service, "slack");
    assert_eq!(calls[0].1.tool, "slack__echo");
    let _ = std::fs::remove_dir_all(&data);
}

/// A lead that drafts an email, then a Gmail helper that browses with the
/// draft's id from its brief and nothing else of the words.
struct Drafting;
impl WorkModelClient for Drafting {
    fn call<'a>(
        &'a self,
        request: WorkModelRequest,
        _: &'a (dyn Fn(WorkModelEvent) + Send + Sync),
    ) -> WorkModelFuture<'a> {
        Box::pin(async move {
            let turn = request
                .messages
                .iter()
                .filter(|m| matches!(m, WorkModelMessage::Assistant(_)))
                .count();
            let system = request
                .system
                .iter()
                .map(|b| b.text.as_str())
                .collect::<String>();
            let id = |text: &str| {
                text.split(|c: char| !c.is_alphanumeric())
                    .find(|word| WorkArtifactId::parse(word).is_some())
                    .map(str::to_owned)
            };
            let assistant = if system.contains("You are the Work agent") {
                match turn {
                    0 => vec![call(
                        "d",
                        "create",
                        json!({"kind": "draft", "title": "Hello Hayk", "data": {
                            "destination": "email", "to": "hayk@example.com", "subject": "Hello",
                            "body": "Hi Hayk,\n\nThe deck is ready.\nBest, Ana"}}),
                    )],
                    1 => {
                        let draft = id(&results(&request)).expect("the draft's id");
                        vec![call(
                            "p",
                            "start_part",
                            json!({"title": "Gmail", "helper": "browser", "service": "mail.google.com",
                                "goal": "Send the draft", "draft": draft}),
                        )]
                    }
                    2 => vec![call(
                        "r",
                        "create",
                        json!({"kind": "reply", "title": "Sent", "data": {
                            "headline": "Email sent", "text": "The email went to Hayk."}}),
                    )],
                    _ => vec![call("f", "finish", json!({"say": "Sent."}))],
                }
            } else {
                match turn {
                    0 => {
                        let brief = request
                            .messages
                            .iter()
                            .find_map(|m| match m {
                                WorkModelMessage::User(parts) => {
                                    parts.iter().find_map(|p| match p {
                                        WorkModelPart::Text(t) => Some(t.clone()),
                                        _ => None,
                                    })
                                }
                                _ => None,
                            })
                            .unwrap();
                        let at = brief
                            .find("canvas object")
                            .expect("the brief names the draft");
                        let draft = id(&brief[at..]).expect("the draft's id");
                        vec![call(
                            "b",
                            "browse",
                            json!({"start": "mail.google.com", "goal": "Send the draft", "draft": draft}),
                        )]
                    }
                    _ => vec![call(
                        "f",
                        "finish",
                        json!({"summary": "Email", "digest": "Sent.", "found": true}),
                    )],
                }
            };
            Ok(WorkModelOutcome {
                stop: WorkModelStop::ToolUse,
                usage: WorkModelUsage {
                    input_tokens: 1_000,
                    cached_input_tokens: 0,
                    cache_write_input_tokens: 0,
                    output_tokens: 100,
                    reasoning_tokens: 0,
                    cost_micros: None,
                },
                assistant,
            })
        })
    }
}

#[tokio::test]
async fn a_draft_reaches_the_page_exactly_with_its_line_breaks_and_not_through_the_goal() {
    let store = Arc::new(zephium_store::SqliteStore::in_memory().unwrap());
    let (mut shell, queue, handle, profile) = fixture(store);
    let create = handle
        .work_document(WorkIntent::Create {
            objective: "Draft an email to Hayk and send it".into(),
        })
        .unwrap();
    let work = create.work_id().unwrap();
    drive(&mut shell, &queue, create).await.unwrap();
    let lead = LeadModel {
        client: Arc::new(Drafting),
        ..model(Arc::new(Script {
            calls: Mutex::new(Vec::new()),
        }))
    };
    let models = WorkLeadModels {
        lead: lead.clone(),
        page: lead.clone(),
        light: lead,
    };
    type SeenPair = (Option<String>, Option<String>);
    let seen: Arc<Mutex<Vec<SeenPair>>> = Arc::default();
    let record = seen.clone();
    let answering = answer_asks(&handle, profile, work, "Allow");
    drive(
        &mut shell,
        &queue,
        WorkLeadService::new(handle.clone()).run(
            profile,
            command(work, WorkRevision::INITIAL),
            None,
            models,
            &Search,
            move |_, request| {
                if let WorkStepKindV1::Read { goal, verbatim, .. } = &request.step {
                    record
                        .lock()
                        .unwrap()
                        .push((goal.clone(), verbatim.clone()));
                }
                async move { Ok(notes(request, "Sent")) }
            },
            |_| {},
        ),
    )
    .await
    .unwrap();
    answering.abort();
    let seen = seen.lock().unwrap();
    assert_eq!(
        *seen,
        [(
            Some("Send the draft".to_owned()),
            Some(
                "To: hayk@example.com\nSubject: Hello\nBody:\nHi Hayk,\n\nThe deck is ready.\nBest, Ana"
                    .to_owned()
            )
        )]
    );
}

/// A lead that sends one part to a service's own app, and a page helper that
/// browses it once.
struct Routing(&'static str);
impl WorkModelClient for Routing {
    fn call<'a>(
        &'a self,
        request: WorkModelRequest,
        _: &'a (dyn Fn(WorkModelEvent) + Send + Sync),
    ) -> WorkModelFuture<'a> {
        Box::pin(async move {
            let turn = request
                .messages
                .iter()
                .filter(|m| matches!(m, WorkModelMessage::Assistant(_)))
                .count();
            let system = request
                .system
                .iter()
                .map(|b| b.text.as_str())
                .collect::<String>();
            let assistant = if system.contains("You are the Work agent") {
                match turn {
                    0 => vec![
                        call(
                            "p",
                            "start_part",
                            json!({"title": "Issue", "helper": "browser", "service": self.0,
                                "goal": "Create an issue called Fix login"}),
                        ),
                        call(
                            "q",
                            "start_part",
                            json!({"title": "Labels", "helper": "browser", "service": self.0,
                                "goal": "Create a label called Bug"}),
                        ),
                    ],
                    1 => vec![call(
                        "r",
                        "create",
                        json!({"kind": "reply", "title": "Issue", "data": {
                            "headline": "Issue", "text": "The parts ran."}}),
                    )],
                    _ => vec![call("f", "finish", json!({"say": "Done."}))],
                }
            } else {
                match turn {
                    0 => vec![call(
                        "b",
                        "browse",
                        json!({"start": self.0, "goal": "Create an issue called Fix login", "mine": true}),
                    )],
                    _ => vec![call(
                        "f",
                        "finish",
                        json!({"summary": "Created", "digest": "Created.", "found": true}),
                    )],
                }
            };
            Ok(WorkModelOutcome {
                stop: WorkModelStop::ToolUse,
                usage: WorkModelUsage {
                    input_tokens: 1_000,
                    cached_input_tokens: 0,
                    cache_write_input_tokens: 0,
                    output_tokens: 100,
                    reasoning_tokens: 0,
                    cost_micros: None,
                },
                assistant,
            })
        })
    }
}

/// Answers every question the run asks with `answer`, until aborted.
fn answer_asks(
    handle: &crate::Handle,
    profile: zephium_core::ids::ProfileId,
    work: WorkId,
    answer: &'static str,
) -> tokio::task::JoinHandle<()> {
    let handle = handle.clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            let Ok(request) = handle.work_projection(profile, work) else {
                continue;
            };
            let zephium_ipc::work::WorkReplyV1::Projection { projection } =
                request.response(profile).await.reply
            else {
                continue;
            };
            let Some(execution) = projection.executions.last() else {
                continue;
            };
            let open = execution.steps.iter().find_map(|step| match &step.kind {
                WorkStepKindV1::Ask { answer: None, .. }
                    if step.status == WorkStepStatus::Running =>
                {
                    Some(step.id)
                }
                _ => None,
            });
            let Some(step) = open else {
                continue;
            };
            let _ = handle
                .work_command(
                    profile,
                    WorkCommandV1 {
                        version: 1,
                        work,
                        expected_revision: projection.work.revision,
                        command: WorkCommandId::generate(),
                        intent: WorkRuntimeIntent::AnswerStep {
                            execution: execution.id,
                            step,
                            answer: answer.into(),
                        },
                    },
                )
                .unwrap()
                .await;
        }
    })
}

/// Runs the Routing lead on `service`, answering every question with
/// `answer`; returns the run and the pages that opened.
async fn routed(service: &'static str, answer: &'static str) -> (WorkExecutionFact, usize) {
    let store = Arc::new(zephium_store::SqliteStore::in_memory().unwrap());
    let (mut shell, queue, handle, profile) = fixture(store);
    let create = handle
        .work_document(WorkIntent::Create {
            objective: "Create an issue called Fix login".into(),
        })
        .unwrap();
    let work = create.work_id().unwrap();
    drive(&mut shell, &queue, create).await.unwrap();
    let lead = LeadModel {
        client: Arc::new(Routing(service)),
        ..model(Arc::new(Script {
            calls: Mutex::new(Vec::new()),
        }))
    };
    let models = WorkLeadModels {
        lead: lead.clone(),
        page: lead.clone(),
        light: lead,
    };
    let answering = answer_asks(&handle, profile, work, answer);
    let opened = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counted = opened.clone();
    let done = drive(
        &mut shell,
        &queue,
        WorkLeadService::new(handle.clone()).run(
            profile,
            command(work, WorkRevision::INITIAL),
            None,
            models,
            &Search,
            move |_, request| {
                counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                async move { Ok(notes(request, "Created")) }
            },
            |_| {},
        ),
    )
    .await
    .unwrap();
    answering.abort();
    (
        done.executions[0].clone(),
        opened.load(std::sync::atomic::Ordering::SeqCst),
    )
}

fn way_asks(run: &WorkExecutionFact) -> Vec<(String, Vec<String>)> {
    run.steps
        .iter()
        .filter_map(|s| match &s.kind {
            WorkStepKindV1::Ask {
                prompt, options, ..
            } => Some((prompt.clone(), options.clone())),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn a_service_with_an_official_connection_asks_one_way_question_for_all_its_parts() {
    let (run, opened) = routed("linear.app", "Use the website").await;
    assert_eq!(
        way_asks(&run),
        [(
            "Work on the Linear website, or connect Linear first?".to_owned(),
            vec![
                "Use the website".to_owned(),
                "Connect Linear".to_owned(),
                "Always the website".to_owned(),
                "Not now".to_owned()
            ]
        )]
    );
    assert!(opened >= 1);
    assert!(run.parts.iter().all(|p| p.state == WorkPartStateV1::Done));
}

#[tokio::test]
async fn connecting_first_ends_the_part_with_the_connect_need() {
    let (run, opened) = routed("linear.app", "Connect Linear").await;
    assert_eq!(opened, 0);
    assert_eq!(way_asks(&run).len(), 1);
    for part in &run.parts {
        assert_eq!(part.state, WorkPartStateV1::Failed);
        assert_eq!(
            part.need,
            Some(WorkPartNeedV1::Connect {
                connection: "Linear".into()
            })
        );
    }
}

#[tokio::test]
async fn not_now_keeps_the_service_untouched_and_asks_to_allow_it() {
    let (run, opened) = routed("app.todoist.com", "Not now").await;
    assert_eq!(opened, 0);
    assert_eq!(way_asks(&run).len(), 1);
    assert!(run
        .parts
        .iter()
        .all(|p| matches!(p.need, Some(WorkPartNeedV1::AllowSite { .. }))));
}

#[tokio::test]
async fn a_service_without_an_official_connection_keeps_its_entry_question_alone() {
    let (run, _) = routed("airbnb.com", "Allow").await;
    assert!(way_asks(&run)
        .iter()
        .all(|(prompt, _)| !prompt.contains("connect")));
}

/// A lead scripted turn by turn: the turn's index and every tool result so
/// far in, the calls out.
struct Turns(fn(usize, &str) -> Vec<WorkModelPart>);
impl WorkModelClient for Turns {
    fn call<'a>(
        &'a self,
        request: WorkModelRequest,
        _: &'a (dyn Fn(WorkModelEvent) + Send + Sync),
    ) -> WorkModelFuture<'a> {
        Box::pin(async move {
            let turn = request
                .messages
                .iter()
                .filter(|m| matches!(m, WorkModelMessage::Assistant(_)))
                .count();
            Ok(WorkModelOutcome {
                stop: WorkModelStop::ToolUse,
                usage: WorkModelUsage {
                    input_tokens: 1_000,
                    cached_input_tokens: 0,
                    cache_write_input_tokens: 0,
                    output_tokens: 100,
                    reasoning_tokens: 0,
                    cost_micros: None,
                },
                assistant: (self.0)(turn, &results(&request)),
            })
        })
    }
}

async fn scripted(
    objective: &str,
    turns: fn(usize, &str) -> Vec<WorkModelPart>,
) -> WorkExecutionFact {
    let store = Arc::new(zephium_store::SqliteStore::in_memory().unwrap());
    let (mut shell, queue, handle, profile) = fixture(store);
    let create = handle
        .work_document(WorkIntent::Create {
            objective: objective.into(),
        })
        .unwrap();
    let work = create.work_id().unwrap();
    drive(&mut shell, &queue, create).await.unwrap();
    let lead = LeadModel {
        client: Arc::new(Turns(turns)),
        ..model(Arc::new(Script {
            calls: Mutex::new(Vec::new()),
        }))
    };
    let models = WorkLeadModels {
        lead: lead.clone(),
        page: lead.clone(),
        light: lead,
    };
    let done = drive(
        &mut shell,
        &queue,
        WorkLeadService::new(handle.clone()).run(
            profile,
            command(work, WorkRevision::INITIAL),
            None,
            models,
            &Search,
            |_, request| page(request),
            |_| {},
        ),
    )
    .await
    .unwrap();
    done.executions[0].clone()
}

#[tokio::test]
async fn an_object_stands_on_its_first_try_with_long_text_clipped_and_strays_left_out() {
    fn turns(turn: usize, seen: &str) -> Vec<WorkModelPart> {
        match turn {
            0 => vec![call(
                "p",
                "create",
                json!({"kind": "plan", "title": "Your trip", "data": {"steps": [
                    {"title": "Fly to SFO", "kind": "travel", "detail": "word ".repeat(70)},
                    {"title": "Check in", "kind": "stay", "day": "5 Jan"}]}}),
            )],
            1 => {
                assert!(
                    seen.contains("Left out to place it: steps[1].day (not a field of plan)"),
                    "{seen}"
                );
                vec![
                    call(
                        "r",
                        "create",
                        json!({"kind": "reply", "title": "Your trip", "data": {
                        "headline": "Two steps to San Francisco", "text": "Fly, then check in."}}),
                    ),
                    call(
                        "f",
                        "finish",
                        json!({"say": "Your trip is on the canvas.", "title": "Trip to San Francisco"}),
                    ),
                ]
            }
            _ => panic!("the run finished at turn 1"),
        }
    }
    let run = scripted("Plan my trip", turns).await;
    assert_eq!(run.status, WorkExecutionStatus::NeedsReview);
    let plan = run
        .artifacts
        .iter()
        .find_map(|a| match &a.data {
            WorkArtifactDataV1::Plan { steps, .. } => Some(steps.clone()),
            _ => None,
        })
        .unwrap();
    let detail = plan[0].detail.clone().unwrap();
    assert!(
        detail.starts_with("word word") && detail.chars().count() <= 280,
        "{detail}"
    );
    assert_eq!(plan[1].title, "Check in");
    assert_eq!(run.title.as_deref(), Some("Trip to San Francisco"));
}

#[tokio::test]
async fn a_run_that_stops_moving_closes_with_what_it_has() {
    fn turns(_: usize, _: &str) -> Vec<WorkModelPart> {
        vec![WorkModelPart::Text("Thinking about it.".into())]
    }
    let run = scripted("Plan my day", turns).await;
    assert_eq!(run.status, WorkExecutionStatus::NeedsReview);
    assert!(run.steps.iter().all(|s| s.status != WorkStepStatus::Failed));
    let reply = run
        .artifacts
        .iter()
        .find_map(|a| match &a.data {
            WorkArtifactDataV1::Reply { headline, text, .. } => {
                Some((headline.clone(), text.clone()))
            }
            _ => None,
        })
        .unwrap();
    assert_eq!(reply.0, "I couldn't finish this");
    assert!(reply
        .1
        .contains("I stopped before I could put the rest together."));
    assert!(matches!(
        run.steps.last().unwrap().kind,
        WorkStepKindV1::Finish { .. }
    ));
}

/// A lead run over the person's engine: `browse` answers every page the
/// run opens, results pages included.
async fn scripted_search(
    objective: &str,
    turns: fn(usize, &str) -> Vec<WorkModelPart>,
    search: &dyn WorkPublicSearchProvider,
    browse: fn(WorkAgentBrowseRequest) -> WorkBrowserOutcome,
) -> WorkExecutionFact {
    let store = Arc::new(zephium_store::SqliteStore::in_memory().unwrap());
    let (mut shell, queue, handle, profile) = fixture(store);
    let create = handle
        .work_document(WorkIntent::Create {
            objective: objective.into(),
        })
        .unwrap();
    let work = create.work_id().unwrap();
    drive(&mut shell, &queue, create).await.unwrap();
    let lead = LeadModel {
        client: Arc::new(Turns(turns)),
        ..model(Arc::new(Script {
            calls: Mutex::new(Vec::new()),
        }))
    };
    let models = WorkLeadModels {
        lead: lead.clone(),
        page: lead.clone(),
        light: lead,
    };
    let done = drive(
        &mut shell,
        &queue,
        WorkLeadService::new(handle.clone()).run(
            profile,
            command(work, WorkRevision::INITIAL),
            None,
            models,
            search,
            |_, request| async move { Ok(browse(request)) },
            |_| {},
        ),
    )
    .await
    .unwrap();
    done.executions[0].clone()
}

fn outcome(
    search: Option<WorkSearchPage>,
    artifacts: Vec<WorkArtifactDraft>,
) -> WorkBrowserOutcome {
    WorkBrowserOutcome {
        status: WorkStepStatus::Succeeded,
        usage: Some(WorkUsage {
            model_tokens: 0,
            cost_micro_usd: 0,
            operations: 1,
            accounting: WorkUsageAccounting::Exact,
        }),
        artifacts,
        intervention: None,
        note: None,
        measurements: None,
        helped: false,
        held_back: false,
        committed: Vec::new(),
        rerun: false,
        search,
        redirect: None,
    }
}

/// The search a browser search falls back to, counting what it is asked.
struct Fallback(Arc<std::sync::atomic::AtomicUsize>);
impl WorkPublicSearchProvider for Fallback {
    fn search<'a>(
        &'a self,
        scope: &'a WorkPublicSearchScope,
        _: &'a [zephium_core::work::context::WorkContextBody],
        limits: WorkExecutionLimits,
    ) -> WorkPublicSearchFuture<'a> {
        Box::pin(async move {
            self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            assert_eq!(scope.provider, WorkSearchProvider::Native);
            let asked = WorkPublicSearchScope {
                query: format!("{} ESTA", scope.query),
                ..scope.clone()
            };
            Search.search(&asked, &[], limits).await
        })
    }
}

fn browser_search(asked: &Arc<std::sync::atomic::AtomicUsize>) -> zephium_agentic::BrowserSearch {
    zephium_agentic::BrowserSearch::try_new(zephium_core::search::SearchEngine::DuckDuckGo, "")
        .unwrap()
        .with_fallback(
            Box::new(Fallback(asked.clone())),
            WorkSearchFallbackV1 {
                provider: WorkSearchProvider::Native,
                model: SEARCH_MODEL.into(),
            },
        )
        .unwrap()
}

#[tokio::test]
async fn a_lead_search_reads_the_engines_results_then_the_page_it_picks() {
    fn turns(turn: usize, seen: &str) -> Vec<WorkModelPart> {
        match turn {
            0 => vec![call(
                "s",
                "web_search",
                json!({"query": "rust async runtime", "freshness": "week"}),
            )],
            1 => {
                assert!(seen.contains("Results from DuckDuckGo"), "{seen}");
                assert!(
                    seen.contains(
                        "1. [s1] Tokio - An asynchronous Rust runtime — https://tokio.rs/"
                    ),
                    "{seen}"
                );
                assert!(
                    seen.contains("   Tokio is an event-driven platform."),
                    "{seen}"
                );
                assert!(
                    seen.contains("2. [s2] async-std — https://async.rs/"),
                    "{seen}"
                );
                vec![call("f", "web_fetch", json!({"url": "https://tokio.rs/"}))]
            }
            2 => {
                assert!(
                    seen.contains("Tokio schedules tasks on a work-stealing pool"),
                    "{seen}"
                );
                vec![
                    call(
                        "r",
                        "create",
                        json!({"kind": "reply", "title": "Tokio", "data": {
                        "headline": "Use Tokio", "text": "It is the common async runtime."}}),
                    ),
                    call(
                        "d",
                        "finish",
                        json!({"say": "Done.", "title": "Rust async runtime"}),
                    ),
                ]
            }
            _ => panic!("the run finished at turn 2"),
        }
    }
    fn browse(request: WorkAgentBrowseRequest) -> WorkBrowserOutcome {
        if let Some(page) = &request.results {
            assert!(
                matches!(&request.step, WorkStepKindV1::Search { query } if query == "rust async runtime (past week)")
            );
            assert!(request.session == crate::work_sites::SiteSession::Private);
            assert_eq!(
                page.url.as_str(),
                "https://duckduckgo.com/?q=rust+async+runtime&df=w"
            );
            return outcome(
                Some(WorkSearchPage::Results(vec![
                    WorkSearchHit {
                        title: "Tokio - An asynchronous Rust runtime".into(),
                        url: "https://tokio.rs/".into(),
                        snippet: Some("Tokio is an event-driven platform.".into()),
                    },
                    WorkSearchHit {
                        title: "async-std".into(),
                        url: "https://async.rs/".into(),
                        snippet: None,
                    },
                ])),
                vec![],
            );
        }
        let WorkStepKindV1::Read {
            url, goal: None, ..
        } = &request.step
        else {
            panic!("a page read");
        };
        assert_eq!(url, "https://tokio.rs/");
        outcome(
            None,
            vec![WorkArtifactDraft {
                output: request.output,
                title: "Notes from tokio.rs".into(),
                data: WorkArtifactDataV1::Findings {
                    subjects: vec![],
                    items: vec![WorkFinding {
                        claim: "Tokio schedules tasks on a work-stealing pool".into(),
                        subject: None,
                        evidence: vec![0],
                        confidence: WorkConfidence::Supported,
                        detail: None,
                        general_knowledge: false,
                    }],
                },
                evidence: vec![WorkEvidenceLink {
                    extraction_id: WorkArtifactId::from(901),
                    source_id: 1,
                }],
            }],
        )
    }
    let asked = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let run = scripted_search(
        "Which async runtime should I use in Rust?",
        turns,
        &browser_search(&asked),
        browse,
    )
    .await;
    assert_eq!(run.status, WorkExecutionStatus::NeedsReview);
    let evidence = &run.provider_evidence[0].evidence;
    assert_eq!(evidence.provider, WorkSearchProvider::Browser);
    assert_eq!(evidence.model, "duckduckgo");
    assert!(evidence.answer.is_empty());
    assert_eq!(evidence.citations.len(), 2);
    let search = run
        .steps
        .iter()
        .find(|step| matches!(step.kind, WorkStepKindV1::Search { .. }))
        .unwrap();
    assert_eq!(search.status, WorkStepStatus::Succeeded);
    let usage = search.usage.unwrap();
    assert_eq!((usage.model_tokens, usage.cost_micro_usd), (0, 0));
    assert!(run.steps.iter().any(|step| matches!(
        &step.kind,
        WorkStepKindV1::Read { url, .. } if url == "https://tokio.rs/"
    )));
    assert_eq!(asked.load(std::sync::atomic::Ordering::SeqCst), 0);
}

#[tokio::test]
async fn a_challenged_results_page_falls_back_to_the_models_own_search() {
    fn turns(turn: usize, seen: &str) -> Vec<WorkModelPart> {
        match turn {
            0 => vec![call("s", "web_search", json!({"query": "us entry rules"}))],
            1 => {
                assert!(seen.contains("Polish citizens travel on ESTA"), "{seen}");
                vec![
                    call(
                        "r",
                        "create",
                        json!({"kind": "reply", "title": "Entry", "data": {
                        "headline": "ESTA", "text": "Apply for an ESTA."}}),
                    ),
                    call(
                        "d",
                        "finish",
                        json!({"say": "Done.", "title": "Rust async runtime"}),
                    ),
                ]
            }
            _ => panic!("the run finished at turn 1"),
        }
    }
    fn browse(request: WorkAgentBrowseRequest) -> WorkBrowserOutcome {
        assert!(request.results.is_some(), "only the results page opens");
        outcome(
            Some(WorkSearchPage::Wall(WorkSearchWall::Challenge)),
            vec![],
        )
    }
    let asked = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let run = scripted_search(
        "What does a Pole need to enter the US?",
        turns,
        &browser_search(&asked),
        browse,
    )
    .await;
    assert_eq!(run.status, WorkExecutionStatus::NeedsReview);
    // The fallback's evidence belongs to the run under its browser grant.
    let evidence = &run.provider_evidence[0].evidence;
    assert_eq!(evidence.provider, WorkSearchProvider::Native);
    assert_eq!(evidence.model, SEARCH_MODEL);
    assert_eq!(asked.load(std::sync::atomic::Ordering::SeqCst), 1);
}

/// A read whose first load the page's server redirected: the browser read
/// failed naming where, as the composition reports it.
fn redirected(to: &str) -> WorkBrowserOutcome {
    WorkBrowserOutcome {
        status: WorkStepStatus::Failed,
        usage: Some(WorkUsage {
            model_tokens: 0,
            cost_micro_usd: 0,
            operations: 1,
            accounting: WorkUsageAccounting::Exact,
        }),
        artifacts: vec![],
        intervention: None,
        note: Some(format!("The page redirects to {to}")),
        measurements: None,
        helped: false,
        held_back: false,
        committed: Vec::new(),
        rerun: false,
        search: None,
        redirect: Some(to.into()),
    }
}

fn notes(request: WorkAgentBrowseRequest, claim: &str) -> WorkBrowserOutcome {
    outcome(
        None,
        vec![WorkArtifactDraft {
            output: request.output,
            title: "Notes".into(),
            data: WorkArtifactDataV1::Findings {
                subjects: vec![],
                items: vec![WorkFinding {
                    claim: claim.into(),
                    subject: None,
                    evidence: vec![0],
                    confidence: WorkConfidence::Supported,
                    detail: None,
                    general_knowledge: false,
                }],
            },
            evidence: vec![WorkEvidenceLink {
                extraction_id: WorkArtifactId::from(902),
                source_id: 1,
            }],
        }],
    )
}

fn finish() -> Vec<WorkModelPart> {
    vec![
        call(
            "r",
            "create",
            json!({"kind": "reply", "title": "Plans", "data": {
            "headline": "Plans", "text": "The plans are listed."}}),
        ),
        call("d", "finish", json!({"say": "Done.", "title": "Plans"})),
    ]
}

fn reads(run: &WorkExecutionFact) -> Vec<(String, WorkStepStatus, Option<String>, usize)> {
    run.steps
        .iter()
        .filter_map(|step| match &step.kind {
            WorkStepKindV1::Read { url, .. } => Some((
                url.clone(),
                step.status,
                step.note.clone(),
                step.artifacts.len(),
            )),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn a_same_site_redirect_is_read_again_as_one_fresh_read_of_its_destination() {
    fn turns(turn: usize, seen: &str) -> Vec<WorkModelPart> {
        match turn {
            0 => vec![call(
                "f",
                "web_fetch",
                json!({"url": "https://hetzner.com/cloud"}),
            )],
            1 => {
                assert!(
                    seen.contains("https://hetzner.com/cloud redirects to https://www.hetzner.com/cloud; this is a read of https://www.hetzner.com/cloud."),
                    "{seen}"
                );
                assert!(seen.contains("CX22 costs 3.79 EUR a month"), "{seen}");
                assert!(!seen.contains("Failed"), "{seen}");
                finish()
            }
            _ => panic!("the run finished at turn 1"),
        }
    }
    fn browse(request: WorkAgentBrowseRequest) -> WorkBrowserOutcome {
        let WorkStepKindV1::Read { url, .. } = &request.step else {
            panic!("a page read");
        };
        match url.as_str() {
            "https://hetzner.com/cloud" => {
                assert_eq!(request.redirected_from, None);
                redirected("https://www.hetzner.com/cloud")
            }
            "https://www.hetzner.com/cloud" => {
                assert_eq!(
                    request.redirected_from.as_deref(),
                    Some("https://hetzner.com/cloud")
                );
                notes(request, "CX22 costs 3.79 EUR a month")
            }
            other => panic!("unexpected read of {other}"),
        }
    }
    let asked = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let run = scripted_search(
        "Hetzner cloud prices",
        turns,
        &browser_search(&asked),
        browse,
    )
    .await;
    assert_eq!(run.status, WorkExecutionStatus::NeedsReview);
    assert_eq!(
        reads(&run),
        [
            (
                "https://hetzner.com/cloud".into(),
                WorkStepStatus::Failed,
                Some("The page redirects to https://www.hetzner.com/cloud".into()),
                0,
            ),
            (
                "https://www.hetzner.com/cloud".into(),
                WorkStepStatus::Succeeded,
                reads(&run)[1].2.clone(),
                1,
            ),
        ]
    );
}

#[tokio::test]
async fn a_public_cross_site_redirect_is_isolated_and_chains_stop_at_their_bound_or_loop() {
    fn turns(turn: usize, seen: &str) -> Vec<WorkModelPart> {
        match turn {
            0 => vec![
                call(
                    "a",
                    "web_fetch",
                    json!({"url": "https://plans.example.com/a"}),
                ),
                call(
                    "b",
                    "web_fetch",
                    json!({"url": "https://loop.example.org/a"}),
                ),
                call(
                    "c",
                    "web_fetch",
                    json!({"url": "https://round.example.org/a"}),
                ),
                // Work never loads plain http: this reads the https page.
                call("e", "web_fetch", json!({"url": "http://example.com/"})),
            ],
            1 => {
                assert!(
                    seen.contains("https://plans.example.com/a redirects to https://other.example.net/plans; this is a read of https://other.example.net/plans.") && seen.contains("The destination lists its plans"),
                    "{seen}"
                );
                assert!(
                    seen.contains("https://loop.example.org/a redirects to https://loop.example.org/c; this is a read of https://loop.example.org/c.\nFailed: The page redirects to https://loop.example.org/d\nRead https://loop.example.org/d if it is what you need"),
                    "{seen}"
                );
                assert!(
                    seen.contains("https://round.example.org/a redirects to https://round.example.org/b; this is a read of https://round.example.org/b.\nFailed: The page redirects to https://round.example.org/a\n"),
                    "{seen}"
                );
                finish()
            }
            _ => panic!("the run finished at turn 1"),
        }
    }
    fn browse(request: WorkAgentBrowseRequest) -> WorkBrowserOutcome {
        let WorkStepKindV1::Read { url, .. } = &request.step else {
            panic!("a page read");
        };
        match url.as_str() {
            "https://example.com/" => notes(request, "Example Domain is for examples"),
            "https://round.example.org/a" => redirected("https://round.example.org/b"),
            "https://round.example.org/b" => redirected("https://round.example.org/a"),
            "https://plans.example.com/a" => redirected("https://other.example.net/plans"),
            "https://other.example.net/plans" => {
                assert!(request.private_run);
                assert_eq!(request.session, crate::work_sites::SiteSession::Private);
                assert!(!request.allow_edits);
                assert!(matches!(
                    request.step,
                    WorkStepKindV1::Read { goal: None, .. }
                ));
                notes(request, "The destination lists its plans")
            }
            "https://loop.example.org/a" => redirected("https://loop.example.org/b"),
            "https://loop.example.org/b" => redirected("https://loop.example.org/c"),
            "https://loop.example.org/c" => redirected("https://loop.example.org/d"),
            other => panic!("unexpected read of {other}"),
        }
    }
    let asked = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let run = scripted_search("Compare plans", turns, &browser_search(&asked), browse).await;
    assert_eq!(run.status, WorkExecutionStatus::NeedsReview);
    let urls: Vec<String> = reads(&run).into_iter().map(|(url, ..)| url).collect();
    // Each address is read once: the bound ends one chain, the loop another.
    assert_eq!(urls.len(), 8, "{urls:?}");
    assert!(urls.iter().any(|url| url == "https://example.com/"));
    assert_eq!(
        urls.iter()
            .filter(|url| url.contains("other.example.net"))
            .count(),
        1
    );
    assert!(!urls.iter().any(|url| url.ends_with("/d")));
}

struct FolderHandoffModel {
    root: String,
    helper_calls: std::sync::atomic::AtomicUsize,
}
impl WorkModelClient for FolderHandoffModel {
    fn call<'a>(
        &'a self,
        request: WorkModelRequest,
        _: &'a (dyn Fn(WorkModelEvent) + Send + Sync),
    ) -> WorkModelFuture<'a> {
        Box::pin(async move {
            let lead = request
                .tools
                .iter()
                .any(|tool| matches!(tool.name.as_str(), "start_part" | "continue_work"));
            let turn = request
                .messages
                .iter()
                .filter(|message| matches!(message, WorkModelMessage::Assistant(_)))
                .count();
            let assistant = if lead {
                match turn {
                    0 => ["First", "Second"].into_iter().enumerate().map(|(i,title)| call(&format!("part-{i}"), "start_part", json!({"title":title,"helper":"computer","goal":"Use the chosen project folder for this synthetic handoff fixture"}))).collect(),
                    1 => vec![call("part-again","start_part",json!({"title":"Again","helper":"computer","goal":"Use the same folder for this synthetic handoff fixture"}))],
                    _ => vec![call("reply","create",json!({"kind":"reply","title":"Folder handoff","data":{"headline":"Folder handoff recorded","text":"This synthetic fixture performed no file changes."}})),call("finish","finish",json!({"say":"The fixture is complete."}))],
                }
            } else {
                let context = request
                    .messages
                    .iter()
                    .filter_map(|message| match message {
                        WorkModelMessage::User(parts) => Some(
                            parts
                                .iter()
                                .filter_map(|part| match part {
                                    WorkModelPart::Text(text) => Some(text.as_str()),
                                    _ => None,
                                })
                                .collect::<String>(),
                        ),
                        _ => None,
                    })
                    .collect::<String>();
                assert!(
                    context.contains(&self.root),
                    "the helper receives only the chosen root"
                );
                self.helper_calls
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                vec![call(
                    "helper-finish",
                    "finish",
                    json!({"summary":"Scoped","digest":"Synthetic permission routing only; no file reads or changes.","found":true}),
                )]
            };
            Ok(WorkModelOutcome {
                stop: WorkModelStop::ToolUse,
                usage: WorkModelUsage {
                    input_tokens: 100,
                    cached_input_tokens: 0,
                    cache_write_input_tokens: 0,
                    output_tokens: 20,
                    reasoning_tokens: 0,
                    cost_micros: Some(1),
                },
                assistant,
            })
        })
    }
}

#[test]
fn zero_folder_computer_parts_share_picker_and_only_accept_real_choice() {
    let _serial = crate::WORK_RUNTIME_TEST_SERIAL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(zero_folder_computer_parts_body());
}

async fn zero_folder_computer_parts_body() {
    let folder = tempfile::Builder::new()
        .prefix("zephium-picker-home-")
        .tempdir()
        .unwrap();
    let _home = crate::work_files::TestGrantHome::new(folder.path().canonicalize().unwrap());
    let project = folder.path().join("Documents/project");
    std::fs::create_dir_all(&project).unwrap();
    let root = project
        .canonicalize()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    for mode in ["picked", "cancelled", "bare-label", "stopped"] {
        let store = Arc::new(zephium_store::SqliteStore::in_memory().unwrap());
        let (mut shell, queue, handle, profile) = fixture(store);
        let create = handle
            .work_document(WorkIntent::Create {
                objective: "Inspect my project with the Computer helper".into(),
            })
            .unwrap();
        let work = create.work_id().unwrap();
        drive(&mut shell, &queue, create).await.unwrap();
        let client = Arc::new(FolderHandoffModel {
            root: root.clone(),
            helper_calls: std::sync::atomic::AtomicUsize::new(0),
        });
        let lead = model(client.clone());
        let models = WorkLeadModels {
            lead: lead.clone(),
            page: lead.clone(),
            light: lead,
        };
        let answered = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let blocked_at_stop = Arc::new(Mutex::new(Vec::new()));
        let answering = {
            let handle = handle.clone();
            let answered = answered.clone();
            let blocked_at_stop = blocked_at_stop.clone();
            let root = root.clone();
            tokio::spawn(async move {
                loop {
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                    let Ok(read) = handle.work_projection(profile, work) else {
                        continue;
                    };
                    let zephium_ipc::work::WorkReplyV1::Projection { projection } =
                        read.response(profile).await.reply
                    else {
                        continue;
                    };
                    let Some(execution) = projection.executions.last() else {
                        continue;
                    };
                    let Some(step) = execution.steps.iter().find(|step| {
                        step.status == WorkStepStatus::Running
                            && matches!(
                                step.kind,
                                WorkStepKindV1::Ask {
                                    purpose: Some(WorkAskPurposeV1::Folder),
                                    ..
                                }
                            )
                    }) else {
                        continue;
                    };
                    assert!(step
                        .local
                        .as_ref()
                        .is_some_and(|local| local.folder.is_none()));
                    assert!(execution
                        .parts
                        .iter()
                        .any(|part| part.state == WorkPartStateV1::Waiting
                            && part.need == Some(WorkPartNeedV1::ChooseFolder)));
                    assert_eq!(
                        answered.load(std::sync::atomic::Ordering::SeqCst),
                        0,
                        "a declined or granted shared picker is not asked again"
                    );
                    let intent = if mode == "stopped" {
                        *blocked_at_stop.lock().unwrap() = execution
                            .parts
                            .iter()
                            .filter(|part| {
                                part.state == WorkPartStateV1::Waiting
                                    && part.need == Some(WorkPartNeedV1::ChooseFolder)
                            })
                            .map(|part| part.id)
                            .collect::<Vec<_>>();
                        WorkRuntimeIntent::Cancel {
                            execution: execution.id,
                            intervention: None,
                        }
                    } else {
                        WorkRuntimeIntent::AnswerStep {
                            execution: execution.id,
                            step: step.id,
                            answer: match mode {
                                "picked" => root.clone(),
                                "bare-label" => "Choose folder…".into(),
                                _ => "Not now".into(),
                            },
                        }
                    };
                    let result = handle
                        .work_command(
                            profile,
                            WorkCommandV1 {
                                version: 1,
                                work,
                                expected_revision: projection.work.revision,
                                command: WorkCommandId::generate(),
                                intent,
                            },
                        )
                        .unwrap()
                        .await;
                    if result.is_ok() {
                        answered.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    }
                }
            })
        };
        let service = WorkLeadService::new(handle.clone());
        let result = drive(
            &mut shell,
            &queue,
            tokio::time::timeout(
                std::time::Duration::from_secs(15),
                service.run(
                    profile,
                    command(work, WorkRevision::INITIAL),
                    None,
                    models,
                    &Search,
                    |_, _| async { panic!("no web browsing in folder fixture") },
                    |_| {},
                ),
            ),
        )
        .await;
        answering.abort();
        let projection = result.expect("bounded handoff").expect("settled handoff");
        assert_eq!(answered.load(std::sync::atomic::Ordering::SeqCst), 1);
        let execution = &projection.executions[0];
        assert!(execution.parts.iter().all(|part| part.state.terminal()));
        assert!(!execution
            .steps
            .iter()
            .any(|step| step.status == WorkStepStatus::Running));
        if mode == "picked" {
            assert!(
                client
                    .helper_calls
                    .load(std::sync::atomic::Ordering::SeqCst)
                    >= 2
            );
            assert!(execution
                .parts
                .iter()
                .all(|part| part.need != Some(WorkPartNeedV1::ChooseFolder)));
        } else {
            assert_eq!(
                client
                    .helper_calls
                    .load(std::sync::atomic::Ordering::SeqCst),
                0
            );
            if mode == "stopped" {
                let blocked = blocked_at_stop.lock().unwrap();
                assert!(!blocked.is_empty());
                assert!(execution
                    .parts
                    .iter()
                    .all(|part| part.state == WorkPartStateV1::Stopped));
                assert!(execution
                    .parts
                    .iter()
                    .filter(|part| blocked.contains(&part.id))
                    .all(|part| part.need == Some(WorkPartNeedV1::ChooseFolder)));
                assert!(execution
                    .parts
                    .iter()
                    .all(|part| part.need.is_none()
                        || part.need == Some(WorkPartNeedV1::ChooseFolder)));
            } else {
                assert!(
                    execution
                        .parts
                        .iter()
                        .all(|part| part.need == Some(WorkPartNeedV1::ChooseFolder)),
                    "{mode}"
                );
            }
        }
        assert!(!execution.steps.iter().any(|step| step.kind.files()));
    }
}

#[tokio::test]
async fn list_record_source_survives_real_propose_and_read_canvas_without_private_pixels() {
    const PRIVATE: &str = "Confidential synthetic invoice total is 47219 euros";
    fn turns(turn: usize, seen: &str) -> Vec<WorkModelPart> {
        match turn {
            0 => vec![call(
                "read",
                "web_fetch",
                json!({"url":"https://records.example.com/one"}),
            )],
            1 => {
                assert!(
                    seen.contains("[s"),
                    "synthetic fetch returned no source key: {seen}"
                );
                let key = first_key(seen);
                assert!(!seen.contains("native_crop"));
                vec![call(
                    "make",
                    "create",
                    json!({"kind":"list","title":"Reading records","data":{"style":"reading","items":[
                        {"title":"Keep exact row","detail":"Kept detail","source":key,"record_source":key,
                         "record_media":{"origin":"native_crop","digest":"MODEL_FORGED_REFERENCE"},"media_attempted":true},
                        {"title":"Keep unknown row text","detail":"Still available","source":key,"record_source":"missing-source-key"}
                    ]}}),
                )]
            }
            2 => {
                assert!(seen.contains("Placed list"), "{seen}");
                assert!(
                    seen.contains("record_source"),
                    "optional unknown source should be reported"
                );
                let id = seen
                    .split("Placed list ")
                    .nth(1)
                    .unwrap()
                    .split_whitespace()
                    .next()
                    .unwrap()
                    .trim_end_matches('.');
                assert!(
                    WorkArtifactId::parse(id).is_some(),
                    "valid object id from create result"
                );
                vec![call("canvas", "read_canvas", json!({"ids":[id]}))]
            }
            3 => {
                let line = seen
                    .lines()
                    .find(|line| line.starts_with("[{") && line.contains("Reading records"))
                    .unwrap_or_else(|| panic!("synthetic canvas returned: {seen}"));
                let read: Value = serde_json::from_str(line).unwrap();
                let data = &read[0]["data"];
                assert!(data["items"][0]["record_source"]
                    .as_str()
                    .is_some_and(|key| key.starts_with('s')));
                assert!(data["items"][1].get("record_source").is_none());
                assert_eq!(data["items"][1]["detail"], "Still available");
                assert!(!line.contains("record_media"));
                assert!(!line.contains("media_attempted"));
                assert!(!line.contains("native_crop"));
                assert!(!line.contains("MODEL_FORGED_REFERENCE"));
                vec![call(
                    "private-search",
                    "web_search",
                    json!({"query":PRIVATE}),
                )]
            }
            4 => {
                assert!(
                    seen.contains("private") || seen.contains("person's"),
                    "private disclosure must be refused before public search: {seen}"
                );
                vec![
                    call(
                        "reply",
                        "create",
                        json!({"kind":"reply","title":"Records ready","data":{"headline":"Records ready","text":"Both list texts stay available."}}),
                    ),
                    call(
                        "finish",
                        "finish",
                        json!({"say":"The text remains available.","title":"Reading records"}),
                    ),
                ]
            }
            _ => panic!("unexpected extra turn {turn}"),
        }
    }
    let store = Arc::new(zephium_store::SqliteStore::in_memory().unwrap());
    let (mut shell, queue, handle, profile) = fixture(store);
    let create = handle
        .work_document(WorkIntent::Create {
            objective: "Read the record at https://records.example.com/one and keep a list".into(),
        })
        .unwrap();
    let work = create.work_id().unwrap();
    drive(&mut shell, &queue, create).await.unwrap();
    let lead = LeadModel {
        client: Arc::new(Turns(turns)),
        ..model(Arc::new(Script {
            calls: Mutex::new(Vec::new()),
        }))
    };
    let models = WorkLeadModels {
        lead: lead.clone(),
        page: lead.clone(),
        light: lead,
    };
    let done=drive(&mut shell,&queue,WorkLeadService::new(handle).run(profile,command(work,WorkRevision::INITIAL),None,models,&Search,|owner,request|async move{
        assert!(request.results.is_none(),"private query must never reach a results page");
        let source=WorkEvidenceLink{extraction_id:WorkArtifactId::from(910),source_id: 1};
        let media=zephium_core::work::media::WorkRecordMediaRefV1{profile:owner.profile(),work:owner.work(),execution:owner.execution(),attempt:owner.attempt(),step:request.id,extraction:source.extraction_id,record:0,source:"a".repeat(64),digest:"b".repeat(64),width:128,height:96,origin:zephium_core::work::media::WorkRecordMediaOriginV1::NativeCrop};
        let mut result=outcome(None,vec![WorkArtifactDraft{output:request.output,title:"Native records".into(),evidence:vec![source],data:serde_json::from_value(json!({"kind":"comparison_matrix","subjects":[{"name":PRIVATE,"record_media":media,"media_attempted":true}],"criteria":[{"name":"Title","kind":{"kind":"text"}}],"cells":[[{"value":{"kind":"text","text":PRIVATE},"evidence":[0]}]]})).unwrap()}]);
        // Metadata-only fixture: no PNG or Store image asset is created.
        let draft=&result.artifacts[0];
        WorkArtifactV1{version:1,id:WorkArtifactId::generate(),execution:owner.execution(),node:owner.node(),attempt:owner.attempt(),output:draft.output.clone(),title:draft.title.clone(),data:draft.data.clone(),evidence:draft.evidence.clone(),review:WorkOutputReview::SourceMappedNeedsReview,presentation:WorkArtifactPresentationV1::Automatic,general_knowledge:false,revises:None,part:None}.validate().expect("valid synthetic source before browser result");
        result.measurements=Some(WorkStepMeasurementsV1{resolved_handoff:Some(WorkPageHandoffKindV1::SignIn),..Default::default()});Ok(result)
    },|_|{})).await.unwrap();
    let execution = &done.executions[0];
    let list = execution
        .artifacts
        .iter()
        .find(|artifact| matches!(artifact.data, WorkArtifactDataV1::List { .. }))
        .unwrap();
    let WorkArtifactDataV1::List { items, .. } = &list.data else {
        panic!()
    };
    assert_eq!(items.len(), 2);
    assert!(items[0].record_source.is_some());
    assert!(items[0].record_media.is_some());
    assert!(items[1].record_source.is_none());
    assert!(items[1].record_media.is_none());
    assert_eq!(items[1].title, "Keep unknown row text");
    assert!(crate::work_agent::private_artifact_texts([(
        &execution.steps[..],
        &execution.artifacts[..]
    )])
    .iter()
    .any(|text| text.contains("Keep exact row")));
    assert!(execution
        .steps
        .iter()
        .filter(|step| matches!(step.kind, WorkStepKindV1::Search { .. }))
        .all(|step| step.status != WorkStepStatus::Succeeded));
}

struct FailedSiteAdvice {
    account: bool,
}
impl WorkModelClient for FailedSiteAdvice {
    fn call<'a>(
        &'a self,
        request: WorkModelRequest,
        _: &'a (dyn Fn(WorkModelEvent) + Send + Sync),
    ) -> WorkModelFuture<'a> {
        Box::pin(async move {
            let turn = request
                .messages
                .iter()
                .filter(|message| matches!(message, WorkModelMessage::Assistant(_)))
                .count();
            let lead = request
                .system
                .iter()
                .any(|block| block.text.contains("You are the Work agent"));
            let seen = results(&request);
            let assistant = match (lead, turn) {
                (true, 0) => vec![call(
                    "part",
                    "start_part",
                    json!({"title":"Account","helper":"browser","service":"work.example.test","goal":if self.account {"Read my account inbox"}else{"Read the public documentation"}}),
                )],
                (true, 1) => {
                    assert!(seen.contains("could not do its job"), "{seen}");
                    assert_eq!(
                        seen.contains("search the web once"),
                        !self.account,
                        "account={} {seen}",
                        self.account
                    );
                    vec![call(
                        "reply",
                        "create",
                        json!({"kind":"reply","title":"Site unavailable","data":{"headline":"The requested content could not be read","text":"The browser helper could not read the requested site."}}),
                    )]
                }
                (true, _) => vec![call(
                    "finish",
                    "finish",
                    json!({"say":"The requested content could not be read."}),
                )],
                (false, _) => vec![call(
                    "helper-finish",
                    "finish",
                    json!({"summary":"Could not read the site","digest":"The browser did not obtain the requested content.","found":false,"need":{"kind":"retry","target":"https://work.example.test/"}}),
                )],
            };
            Ok(WorkModelOutcome {
                stop: WorkModelStop::ToolUse,
                assistant,
                usage: WorkModelUsage {
                    input_tokens: 100,
                    cached_input_tokens: 0,
                    cache_write_input_tokens: 0,
                    output_tokens: 10,
                    reasoning_tokens: 0,
                    cost_micros: None,
                },
            })
        })
    }
}
#[tokio::test]
async fn failed_account_parts_never_suggest_public_source_substitution() {
    for account in [true, false] {
        let store = Arc::new(zephium_store::SqliteStore::in_memory().unwrap());
        let (mut shell, queue, handle, profile) = fixture(store);
        let create = handle
            .work_document(WorkIntent::Create {
                objective: if account {
                    "Read my account inbox".into()
                } else {
                    "Read the public documentation".into()
                },
            })
            .unwrap();
        let work = create.work_id().unwrap();
        drive(&mut shell, &queue, create).await.unwrap();
        let lead = LeadModel {
            client: Arc::new(FailedSiteAdvice { account }),
            ..model(Arc::new(Script {
                calls: Mutex::new(Vec::new()),
            }))
        };
        let models = WorkLeadModels {
            lead: lead.clone(),
            page: lead.clone(),
            light: lead,
        };
        let projection = drive(
            &mut shell,
            &queue,
            WorkLeadService::new(handle).run(
                profile,
                command(work, WorkRevision::INITIAL),
                None,
                models,
                &Search,
                |_, _| async { panic!("scripted failure requires no browsing") },
                |_| {},
            ),
        )
        .await
        .unwrap();
        let execution = &projection.executions[0];
        assert_eq!(execution.parts.len(), 1);
        assert_eq!(execution.parts[0].state, WorkPartStateV1::Failed);
        assert!(!execution
            .steps
            .iter()
            .any(|step| step.status == WorkStepStatus::Running));
    }
}

static MODEL_TIMING_EVENTS: Mutex<Vec<crate::work_lead::WorkLeadDiagnostic>> =
    Mutex::new(Vec::new());
struct TruncatedLeadCall(std::sync::atomic::AtomicUsize);
impl WorkModelClient for TruncatedLeadCall {
    fn call<'a>(
        &'a self,
        request: WorkModelRequest,
        events: &'a (dyn Fn(WorkModelEvent) + Send + Sync),
    ) -> WorkModelFuture<'a> {
        Box::pin(async move {
            let turn = self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if turn == 0 {
                assert_eq!(request.reasoning, Some(WorkModelReasoning::Medium));
                events(WorkModelEvent::Text(
                    "private fixture content must not enter timing diagnostics".into(),
                ));
                tokio::time::sleep(std::time::Duration::from_millis(15)).await;
                return Ok(WorkModelOutcome {
                    stop: WorkModelStop::MaxTokens,
                    assistant: vec![call(
                        "discarded",
                        "create",
                        json!({"kind":"reply","title":"Must never run","data":{"headline":"Discarded generation","text":"This tool call must not execute."}}),
                    )],
                    usage: WorkModelUsage {
                        input_tokens: 100,
                        output_tokens: 8_000,
                        reasoning_tokens: 7_900,
                        ..WorkModelUsage::default()
                    },
                });
            }
            assert_eq!(request.reasoning, Some(WorkModelReasoning::Low));
            assert!(!results(&request).contains("Must never run"));
            assert_eq!(request.max_output_tokens, 2_048);
            assert_eq!(turn, 1, "compact recovery gets only one further call");
            assert!(request.messages.iter().any(|message| matches!(message,
                WorkModelMessage::User(parts) if parts.iter().any(|part| matches!(part,
                    WorkModelPart::Text(text) if text.contains("hit the output limit")
                        && text.contains("fewer optional items") && text.contains("same turn"))))));
            Ok(WorkModelOutcome {
                stop: WorkModelStop::ToolUse,
                assistant: vec![
                    call(
                        "reply",
                        "create",
                        json!({"kind":"reply","title":"Recovered answer","data":{"headline":"Recovered","text":"The bounded retry finished."}}),
                    ),
                    call(
                        "finish",
                        "finish",
                        json!({"say":"The bounded retry finished.","title":"Model recovery"}),
                    ),
                ],
                usage: WorkModelUsage {
                    input_tokens: 100,
                    output_tokens: 30,
                    ..WorkModelUsage::default()
                },
            })
        })
    }
}
#[tokio::test]
async fn an_output_limited_lead_call_is_measured_without_replaying_its_partial_tools() {
    MODEL_TIMING_EVENTS.lock().unwrap().clear();
    let store = Arc::new(zephium_store::SqliteStore::in_memory().unwrap());
    let (mut shell, queue, handle, profile) = fixture(store);
    let create = handle
        .work_document(WorkIntent::Create {
            objective: "Return a short confirmation".into(),
        })
        .unwrap();
    let work = create.work_id().unwrap();
    drive(&mut shell, &queue, create).await.unwrap();
    let client = Arc::new(TruncatedLeadCall(std::sync::atomic::AtomicUsize::new(0)));
    let mut lead = model(client.clone());
    lead.entry.supports.reasoning = true;
    let models = WorkLeadModels {
        lead: lead.clone(),
        page: lead.clone(),
        light: lead,
    };
    let state = drive(
        &mut shell,
        &queue,
        WorkLeadService::new(handle)
            .with_diagnostic(|event| MODEL_TIMING_EVENTS.lock().unwrap().push(event))
            .run(
                profile,
                command(work, WorkRevision::INITIAL),
                None,
                models,
                &Search,
                |_, _| async { panic!("model recovery must not browse") },
                |_| {},
            ),
    )
    .await
    .unwrap();
    assert_eq!(client.0.load(std::sync::atomic::Ordering::SeqCst), 2);
    let execution = &state.executions[0];
    assert_eq!(execution.artifacts.len(), 1);
    assert_eq!(execution.artifacts[0].title, "Recovered answer");
    let events = MODEL_TIMING_EVENTS.lock().unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(
                event,
                crate::work_lead::WorkLeadDiagnostic::ModelSettled {
                    role: WorkModelRole::Lead,
                    ..
                }
            ))
            .count(),
        2
    );
    assert!(events.iter().any(|event|matches!(event,crate::work_lead::WorkLeadDiagnostic::ModelSettled {
        role:WorkModelRole::Lead,stop:WorkModelStop::MaxTokens,elapsed_ms,first_event_ms:Some(first),output_tokens:8_000,reasoning_tokens:7_900
    } if *elapsed_ms>=10 && first<=elapsed_ms)));
    assert!(!format!("{events:?}").contains("private fixture content"));
}

struct EarlyReplyModel {
    handle: crate::Handle,
    profile: zephium_core::ids::ProfileId,
    work: WorkId,
    truncations: usize,
    combined: bool,
    calls: std::sync::atomic::AtomicUsize,
    observed: std::sync::atomic::AtomicBool,
}
impl WorkModelClient for EarlyReplyModel {
    fn call<'a>(
        &'a self,
        request: WorkModelRequest,
        events: &'a (dyn Fn(WorkModelEvent) + Send + Sync),
    ) -> WorkModelFuture<'a> {
        Box::pin(async move {
            let turn = self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let reply = if self.combined {
                call(
                    "early-reply",
                    "finish",
                    json!({"title":"First answer","say":"The result is ready.","reply":{"headline":"A short result","text":"The result is available; supporting details may follow."}}),
                )
            } else {
                call(
                    "early-reply",
                    "create",
                    json!({"kind":"reply","title":"First answer","data":{"headline":"A short result","text":"The result is available; supporting details may follow."}}),
                )
            };
            let finish = call(
                "finish",
                "finish",
                json!({"say":"The result is ready.","title":"Early reply"}),
            );
            if turn == 0 {
                let WorkModelPart::ToolCall(tool) = &reply else {
                    unreachable!()
                };
                events(WorkModelEvent::ToolCall(tool.clone()));
                if self.truncations == 6 {
                    return Ok(WorkModelOutcome {
                        stop: WorkModelStop::MaxTokens,
                        assistant: vec![reply],
                        usage: WorkModelUsage::default(),
                    });
                }
                tokio::time::timeout(std::time::Duration::from_secs(2), async {
                    loop {
                        let port::WorkReply::Runtime(state) = self
                            .handle
                            .work_projection(self.profile, self.work)
                            .unwrap()
                            .await
                            .unwrap()
                            .reply
                        else {
                            panic!()
                        };
                        if let Some(execution) = state.executions.last() {
                            if execution
                                .artifacts
                                .iter()
                                .any(|artifact| artifact.title == "First answer")
                            {
                                assert_eq!(
                                    execution.artifacts.len(),
                                    1,
                                    "supporting object has not been generated yet"
                                );
                                self.observed
                                    .store(true, std::sync::atomic::Ordering::SeqCst);
                                break;
                            }
                        }
                        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
                    }
                })
                .await
                .expect("reply must be durable before generation completes");
            } else {
                assert_eq!(turn, 1, "one compact recovery only");
                assert_eq!(request.max_output_tokens, 2_048);
                assert!(results(&request).contains("Placed reply"));
                if self.truncations == 6 {
                    self.observed
                        .store(true, std::sync::atomic::Ordering::SeqCst);
                }
            }
            if self.truncations == 3 {
                return Err(WorkModelError::Network);
            }
            if matches!(self.truncations, 4 | 5 | 7 | 8) {
                let mut assistant = vec![
                    call(
                        "must-not-run",
                        "create",
                        json!({"kind":"list","title":"Forbidden later object","data":{"style":"todo","items":[{"title":"Unexpected"}]}}),
                    ),
                    finish,
                ];
                if self.truncations == 5 {
                    let WorkModelPart::ToolCall(mut changed) = reply.clone() else {
                        unreachable!()
                    };
                    changed.id = "changed-reply-id".into();
                    assistant.insert(0, WorkModelPart::ToolCall(changed));
                }
                if self.truncations == 7 {
                    let WorkModelPart::ToolCall(mut changed) = reply.clone() else {
                        unreachable!()
                    };
                    changed.arguments[if self.combined { "reply" } else { "data" }]["headline"] =
                        json!("Changed after publication");
                    assistant.insert(0, WorkModelPart::ToolCall(changed));
                }
                if self.truncations == 8 {
                    assistant.insert(0, reply.clone());
                    assistant.insert(0, reply);
                }
                return Ok(WorkModelOutcome {
                    stop: WorkModelStop::ToolUse,
                    assistant,
                    usage: WorkModelUsage::default(),
                });
            }
            let truncated = self.truncations != 6 && turn < self.truncations;
            let assistant = if truncated {
                vec![reply]
            } else if turn == 0 {
                let mut tools = vec![
                    reply,
                    call(
                        "details",
                        "create",
                        json!({"kind":"list","title":"Optional details","data":{"style":"todo","items":[{"title":"A supporting detail"}]}}),
                    ),
                ];
                if !self.combined {
                    tools.push(finish);
                }
                tools
            } else {
                vec![finish]
            };
            Ok(WorkModelOutcome {
                stop: if truncated {
                    WorkModelStop::MaxTokens
                } else {
                    WorkModelStop::ToolUse
                },
                assistant,
                usage: WorkModelUsage {
                    input_tokens: 100,
                    output_tokens: if truncated { 2_048 } else { 100 },
                    ..WorkModelUsage::default()
                },
            })
        })
    }
}
#[tokio::test]
async fn a_completed_reply_is_visible_before_generation_finishes_and_survives_compact_recovery() {
    for (combined, truncations) in [false, true]
        .into_iter()
        .flat_map(|combined| (0..=8).map(move |n| (combined, n)))
    {
        let store = Arc::new(zephium_store::SqliteStore::in_memory().unwrap());
        let (mut shell, queue, handle, profile) = fixture(store);
        let create = handle
            .work_document(WorkIntent::Create {
                objective: "Return a short result with optional supporting details".into(),
            })
            .unwrap();
        let work = create.work_id().unwrap();
        drive(&mut shell, &queue, create).await.unwrap();
        let client = Arc::new(EarlyReplyModel {
            handle: handle.clone(),
            profile,
            work,
            truncations,
            combined,
            calls: std::sync::atomic::AtomicUsize::new(0),
            observed: std::sync::atomic::AtomicBool::new(false),
        });
        let lead = model(client.clone());
        let models = WorkLeadModels {
            lead: lead.clone(),
            page: lead.clone(),
            light: lead,
        };
        let state = drive(
            &mut shell,
            &queue,
            WorkLeadService::new(handle).run(
                profile,
                command(work, WorkRevision::INITIAL),
                None,
                models,
                &Search,
                |_, _| async { panic!("reply publication must not browse") },
                |_| {},
            ),
        )
        .await
        .unwrap();
        assert!(client.observed.load(std::sync::atomic::Ordering::SeqCst));
        assert_eq!(
            client.calls.load(std::sync::atomic::Ordering::SeqCst),
            if truncations == 1 || truncations == 2 || truncations == 6 {
                2
            } else {
                1
            }
        );
        let execution = &state.executions[0];
        assert_eq!(
            execution
                .artifacts
                .iter()
                .filter(|artifact| artifact.title == "First answer")
                .count(),
            1
        );
        assert_eq!(
            execution.artifacts.len(),
            if truncations == 0 { 2 } else { 1 }
        );
        if truncations <= 2 || truncations == 6 {
            assert!(execution.steps.iter().any(|step| matches!(
                step.kind,
                WorkStepKindV1::Finish { .. }
            ) && step.status
                == WorkStepStatus::Succeeded));
        } else {
            assert!(execution
                .steps
                .iter()
                .any(|step| matches!(step.kind, WorkStepKindV1::Turn)
                    && step.status == WorkStepStatus::OutcomeUnknown));
            assert!(!execution
                .artifacts
                .iter()
                .any(|artifact| artifact.title == "Forbidden later object"));
        }
        assert!(execution
            .steps
            .iter()
            .all(|step| step.status != WorkStepStatus::Running));
    }
}

struct StalledAfterReply {
    dropped: std::sync::atomic::AtomicBool,
    calls: std::sync::atomic::AtomicUsize,
}
impl WorkModelClient for StalledAfterReply {
    fn call<'a>(
        &'a self,
        _: WorkModelRequest,
        events: &'a (dyn Fn(WorkModelEvent) + Send + Sync),
    ) -> WorkModelFuture<'a> {
        Box::pin(async move {
            self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            struct Dropped<'a>(&'a std::sync::atomic::AtomicBool);
            impl Drop for Dropped<'_> {
                fn drop(&mut self) {
                    self.0.store(true, std::sync::atomic::Ordering::SeqCst);
                }
            }
            let _guard = Dropped(&self.dropped);
            let WorkModelPart::ToolCall(tool) = call(
                "early",
                "create",
                json!({"kind":"reply","title":"Held reply","data":{"headline":"Held","text":"This publication has not committed yet."}}),
            ) else {
                unreachable!()
            };
            events(WorkModelEvent::ToolCall(tool));
            std::future::pending().await
        })
    }
}
#[tokio::test]
async fn stop_during_early_reply_publication_drops_generation_and_refuses_late_commit() {
    let store = Arc::new(zephium_store::SqliteStore::in_memory().unwrap());
    let (mut shell, queue, handle, profile) = fixture(store);
    let create = handle
        .work_document(WorkIntent::Create {
            objective: "Return a short result".into(),
        })
        .unwrap();
    let work = create.work_id().unwrap();
    drive(&mut shell, &queue, create).await.unwrap();
    let client = Arc::new(StalledAfterReply {
        dropped: std::sync::atomic::AtomicBool::new(false),
        calls: std::sync::atomic::AtomicUsize::new(0),
    });
    let lead = model(client.clone());
    let models = WorkLeadModels {
        lead: lead.clone(),
        page: lead.clone(),
        light: lead,
    };
    let service = WorkLeadService::new(handle.clone());
    let held = Arc::new(tokio::sync::Notify::new());
    let stopping = async {
        held.notified().await;
        let port::WorkReply::Runtime(state) = handle
            .work_projection(profile, work)
            .unwrap()
            .await
            .unwrap()
            .reply
        else {
            panic!()
        };
        let started = std::time::Instant::now();
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
    let running = service.run(
        profile,
        command(work, WorkRevision::INITIAL),
        None,
        models,
        &Search,
        |_, _| async { panic!("no browser") },
        |_| {},
    );
    let all = async { tokio::join!(running, stopping) };
    tokio::pin!(all);
    let mut delayed = None;
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(3);
    let (result, stopped_at) = loop {
        while let Some(command) = queue.try_recv() {
            let command = match command {
                crate::Command::WorkDocument(submission) => {
                    let payload = submission.take().unwrap();
                    let publication = matches!(&payload.request,port::WorkRequest::RuntimeUpdate {update:WorkRuntimeUpdate::BeginStep {step,..},..} if matches!(step.kind,WorkStepKindV1::Publish));
                    let resumed = crate::WorkDocumentSubmission::resume(payload);
                    if publication && delayed.is_none() {
                        delayed = Some(resumed);
                        held.notify_one();
                        continue;
                    }
                    crate::Command::WorkDocument(resumed)
                }
                command => command,
            };
            shell.handle(command);
        }
        tokio::select! {result=&mut all=>break result,_=tokio::time::sleep_until(deadline)=>panic!("Stop waited for publication"),_=tokio::time::sleep(std::time::Duration::from_millis(1))=>{}}
    };
    result.unwrap();
    assert!(stopped_at.elapsed() < std::time::Duration::from_secs(1));
    assert!(client.dropped.load(std::sync::atomic::Ordering::SeqCst));
    assert_eq!(client.calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    shell.handle(crate::Command::WorkDocument(
        delayed.expect("publication was held"),
    ));
    let result = drive(
        &mut shell,
        &queue,
        handle.work_projection(profile, work).unwrap(),
    )
    .await
    .unwrap();
    let port::WorkReply::Runtime(state) = result.reply else {
        panic!()
    };
    assert!(
        state.executions[0].artifacts.is_empty(),
        "cancelled publication cannot arrive late"
    );
    assert_ne!(state.executions[0].status, WorkExecutionStatus::Running);
}

struct NativeRecordFinish(u8);
impl WorkModelClient for NativeRecordFinish {
    fn call<'a>(
        &'a self,
        request: WorkModelRequest,
        events: &'a (dyn Fn(WorkModelEvent) + Send + Sync),
    ) -> WorkModelFuture<'a> {
        Box::pin(async move {
            let turn = request
                .messages
                .iter()
                .filter(|m| matches!(m, WorkModelMessage::Assistant(_)))
                .count();
            let seen = results(&request);
            if request
                .tools
                .iter()
                .any(|tool| tool.name == "continue_work")
            {
                assert_eq!(
                    request
                        .tools
                        .iter()
                        .map(|tool| tool.name.as_str())
                        .collect::<Vec<_>>(),
                    ["finish", "continue_work"]
                );
                assert!(turn > 0);
                assert!(request.max_output_tokens <= 2048);
                assert_eq!(request.reasoning, Some(WorkModelReasoning::Low));
                assert!(matches!(request.tool_choice, WorkModelToolChoice::Required));
            } else {
                let fetch = request
                    .tools
                    .iter()
                    .find(|t| t.name == "web_fetch")
                    .unwrap();
                assert!(
                    fetch
                        .schema
                        .pointer("/properties/records/properties/columns")
                        .is_some(),
                    "the lead must be able to request native records on its first public read"
                );
            }
            let final_call = || {
                let mut output = call(
                    "finish",
                    "finish",
                    json!({
                        "reply":{"headline":"Two offers found","text":"The result carries the original prices and availability."},
                        "records":[{"source":"r2"},{"source":"r1"}],"layout":"picks","records_title":"Offers",
                        "say":"Both original records are available.","title":"Original offers"
                    }),
                );
                if self.0 >= 3 {
                    let WorkModelPart::ToolCall(tool) = &mut output else {
                        unreachable!()
                    };
                    tool.arguments["reply_sources"] = json!(["s1"]);
                }
                output
            };
            let output = match turn {
                0 => call(
                    "read",
                    "web_fetch",
                    json!({"url":"https://records.example.com/one","records":{"title":"Offers","max_items":2,"columns":[
                        {"name":"price","value":{"kind":"text"},"required":false},
                        {"name":"stock","value":{"kind":"text"},"required":false},
                        {"name":"url","value":{"kind":"url"},"required":true}
                    ]}}),
                ),
                1 => {
                    assert!(seen.contains("record r1:"), "{seen}");
                    match self.0 {
                        0 | 3 => final_call(),
                        1 => call(
                            "rewrite",
                            "create",
                            json!({"kind":"sheet","title":"Offers",
                            "data":{"columns":[{"kind":"text","label":"Name"},{"kind":"text","label":"Price"}],
                            "rows":[{"cells":["First offer","FREE"]}]}}),
                        ),
                        2 => call(
                            "unknown",
                            "finish",
                            json!({"reply":{"headline":"Lost table","text":"The table is complete."},
                            "sheet":{"rows":["silently lost"]},"say":"Done","title":"Wrong result"}),
                        ),
                        5 => call(
                            "long-point",
                            "finish",
                            json!({"reply":{"headline":"Lost table","text":"Both offers were read.","points":["x".repeat(141)]},
                                "records":[{"source":"r2"},{"source":"r1"}],"layout":"picks","records_title":"Offers",
                                "reply_sources":["s1"],"say":"Done","title":"Offers"}),
                        ),
                        4 => call(
                            "unknown-source",
                            "finish",
                            json!({
                                "reply":{"headline":"Lost table","text":"Unsupported evidence."},
                                "reply_sources":["s999"],"say":"Done","title":"Wrong result"
                            }),
                        ),
                        _ => unreachable!(),
                    }
                }
                2 if self.0 != 0 => {
                    assert!(
                        seen.contains(if self.0 == 5 {
                            "140"
                        } else if self.0 == 1 {
                            "Placed sheet"
                        } else if self.0 == 4 {
                            "is not a source of this run"
                        } else {
                            "finish accepts only"
                        }),
                        "{seen}"
                    );
                    final_call()
                }
                _ => panic!("native records must settle within one repair call: {turn}: {seen}"),
            };
            if let WorkModelPart::ToolCall(tool) = &output {
                events(WorkModelEvent::ToolCall(tool.clone()));
            }
            Ok(WorkModelOutcome {
                stop: WorkModelStop::ToolUse,
                usage: WorkModelUsage {
                    input_tokens: 1000,
                    cached_input_tokens: 0,
                    cache_write_input_tokens: 0,
                    output_tokens: 100,
                    reasoning_tokens: 0,
                    cost_micros: None,
                },
                assistant: vec![output],
            })
        })
    }
}

#[tokio::test]
async fn native_records_publish_with_reply_and_finish_in_one_final_model_turn() {
    native_record_finish_fixture(0).await;
}
#[tokio::test]
async fn native_records_refuse_rewritten_values_and_repair_in_one_call() {
    native_record_finish_fixture(1).await;
}
#[tokio::test]
async fn native_records_reject_lost_finish_objects_before_early_reply_and_repair_once() {
    native_record_finish_fixture(2).await;
}
#[tokio::test]
async fn cited_reply_and_records_finish_in_one_call_with_original_evidence() {
    native_record_finish_fixture(3).await;
}
#[tokio::test]
async fn unknown_finish_reply_source_refuses_before_early_publication() {
    native_record_finish_fixture(4).await;
}
#[tokio::test]
async fn overlong_final_reply_point_is_clipped_without_a_repair_call() {
    native_record_finish_fixture(5).await;
}
async fn native_record_finish_fixture(mode: u8) {
    let store = Arc::new(zephium_store::SqliteStore::in_memory().unwrap());
    let (mut shell, queue, handle, profile) = fixture(store);
    let create = handle
        .work_document(WorkIntent::Create {
            objective: "Read https://records.example.com/one and compare the two offers".into(),
        })
        .unwrap();
    let work = create.work_id().unwrap();
    drive(&mut shell, &queue, create).await.unwrap();
    let lead = LeadModel {
        client: Arc::new(NativeRecordFinish(mode)),
        ..model(Arc::new(Script {
            calls: Mutex::new(vec![]),
        }))
    };
    let models = WorkLeadModels {
        lead: lead.clone(),
        page: lead.clone(),
        light: lead,
    };
    let done = drive(&mut shell,&queue,WorkLeadService::new(handle).run(profile,command(work,WorkRevision::INITIAL),None,models,&Search,|_,request|async move {
        let WorkStepKindV1::Read { collection: Some(collection), .. } = &request.step else { panic!("native records must be requested on the first read") };
        assert_eq!(collection.max_items, 2);
        assert_eq!(collection.columns.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["price", "stock", "url"]);
        let source = WorkEvidenceLink { extraction_id:WorkArtifactId::from(940),source_id:1 };
        Ok(outcome(None,vec![WorkArtifactDraft {
            output:request.output,title:"Original records".into(), evidence:vec![source],
            data:serde_json::from_value(json!({"kind":"comparison_matrix",
                "subjects":[{"name":"First offer","homepage":"https://records.example.com/item?id=1&variant=blue"},{"name":"Second offer","homepage":"https://records.example.com/item?id=2&variant=red"}],
                "criteria":[{"name":"price","kind":{"kind":"text"}},{"name":"stock","kind":{"kind":"text"}},{"name":"url","kind":{"kind":"text"}}],
                "cells":[[{"value":{"kind":"text","text":"129,99 zł"},"evidence":[0]},{"value":{"kind":"text","text":"In stock"},"evidence":[0]},{"value":{"kind":"text","text":"https://records.example.com/item?id=1&variant=blue"},"evidence":[0]}],
                         [{"value":{"kind":"text","text":"€32.50 / item"},"evidence":[0]},{"value":{"kind":"text","text":"Out of stock"},"evidence":[0]},{"value":{"kind":"text","text":"https://records.example.com/item?id=2&variant=red"},"evidence":[0]}]]
            })).unwrap()
        }]))
    },|_|{})).await.unwrap();
    let execution = &done.executions[0];
    assert!(matches!(
        execution.status,
        WorkExecutionStatus::NeedsReview | WorkExecutionStatus::Completed
    ));
    let picks: Vec<_> = execution
        .artifacts
        .iter()
        .filter_map(|artifact| match &artifact.data {
            WorkArtifactDataV1::Picks { items, .. } => Some(items),
            _ => None,
        })
        .collect();
    // Rows naming records stand as those records: the page's price, never
    // the model's.
    let stored = serde_json::to_string(&execution.artifacts).unwrap();
    assert!(
        !stored.contains("FREE"),
        "a model rewrite must never reach storage"
    );
    assert_eq!(
        stored.contains("\"sheet\""),
        mode == 1,
        "only a sheet of records may stand"
    );
    if mode == 5 {
        // An overlong point is clipped between words; the finish needs no repair.
        assert!(execution
            .artifacts
            .iter()
            .any(|a| matches!(&a.data, WorkArtifactDataV1::Reply {points,..}
            if points.first().is_some_and(|p| p.chars().count() == 140 && p.ends_with('…')))));
    } else {
        assert!(!execution.artifacts.iter().any(|a| matches!(&a.data, WorkArtifactDataV1::Reply {headline,..} if headline == "Lost table")),
            "invalid finish fields must refuse before early publication");
    }
    assert_eq!(picks.len(), 1);
    assert_eq!(picks[0][0].name, "Second offer");
    assert_eq!(picks[0][0].price.as_ref().unwrap().display, "€32.50 / item");
    assert_eq!(
        picks[0][1].url.as_deref(),
        Some("https://records.example.com/item?id=1&variant=blue")
    );
    assert_eq!(picks[0][1].facts[0].value, "In stock");
    assert_eq!(
        execution
            .artifacts
            .iter()
            .filter(|a| matches!(a.data, WorkArtifactDataV1::Reply { .. }))
            .count(),
        1
    );
    if mode >= 3 {
        let reply = execution
            .artifacts
            .iter()
            .find(|a| matches!(a.data, WorkArtifactDataV1::Reply { .. }))
            .unwrap();
        assert_eq!(
            reply.evidence,
            vec![WorkEvidenceLink {
                extraction_id: WorkArtifactId::from(940),
                source_id: 1
            }]
        );
        assert!(!reply.general_knowledge);
    }
}

struct FailedCatalogThenDetail;
impl WorkModelClient for FailedCatalogThenDetail {
    fn call<'a>(
        &'a self,
        request: WorkModelRequest,
        _: &'a (dyn Fn(WorkModelEvent) + Send + Sync),
    ) -> WorkModelFuture<'a> {
        Box::pin(async move {
            let lead = request
                .tools
                .iter()
                .any(|tool| matches!(tool.name.as_str(), "start_part" | "continue_work"));
            let turn = request
                .messages
                .iter()
                .filter(|m| matches!(m, WorkModelMessage::Assistant(_)))
                .count();
            let assistant = if !lead {
                vec![call(
                    "part-end",
                    "finish",
                    json!({"summary":"Catalog unavailable","digest":"No catalog rows were returned.","found":false}),
                )]
            } else {
                match turn {
                    0 => vec![call(
                        "catalog",
                        "start_part",
                        json!({"helper":"browser","title":"Books","goal":"Find books and return their prices and availability","service":"shop.example.com","records":["price","availability","url"]}),
                    )],
                    1 => vec![call(
                        "detail",
                        "web_fetch",
                        json!({"url":"https://shop.example.com/book"}),
                    )],
                    2 => {
                        assert!(results(&request).contains("record r1:"));
                        vec![
                            call(
                                "reply",
                                "create",
                                json!({"kind":"reply","title":"Book found","data":{"headline":"Book found","text":"The detail page supplied price and availability."}}),
                            ),
                            call(
                                "pick",
                                "create",
                                json!({"kind":"picks","title":"Book","data":{"records":[{"source":"r1"}]}}),
                            ),
                            call(
                                "done",
                                "finish",
                                json!({"say":"The original record is preserved.","title":"Book availability"}),
                            ),
                        ]
                    }
                    _ => panic!(
                        "unexpected additional lead turn {turn}: {}",
                        results(&request)
                    ),
                }
            };
            Ok(WorkModelOutcome {
                stop: WorkModelStop::ToolUse,
                assistant,
                usage: WorkModelUsage {
                    input_tokens: 100,
                    cached_input_tokens: 0,
                    cache_write_input_tokens: 0,
                    output_tokens: 100,
                    reasoning_tokens: 0,
                    cost_micros: None,
                },
            })
        })
    }
}

#[tokio::test]
async fn a_failed_browser_catalog_keeps_stock_in_the_leads_fallback_detail_read() {
    let store = Arc::new(zephium_store::SqliteStore::in_memory().unwrap());
    let (mut shell, queue, handle, profile) = fixture(store);
    let create=handle.work_document(WorkIntent::Create{objective:"Find an available book at https://shop.example.com/catalog with its price and availability".into()}).unwrap();
    let work = create.work_id().unwrap();
    drive(&mut shell, &queue, create).await.unwrap();
    let lead = LeadModel {
        client: Arc::new(FailedCatalogThenDetail),
        ..model(Arc::new(Script {
            calls: Mutex::new(vec![]),
        }))
    };
    let models = WorkLeadModels {
        lead: lead.clone(),
        page: lead.clone(),
        light: lead,
    };
    let done=drive(&mut shell,&queue,WorkLeadService::new(handle).run(profile,command(work,WorkRevision::INITIAL),None,models,&Search,|_,request|async move{
        let WorkStepKindV1::Read{url,collection,..}=&request.step else {panic!()};
        if url.ends_with("/catalog") {return Err(WorkError::Unavailable);}
        assert!(url.ends_with("/book"));
        let collection=collection.as_ref().expect("fallback must retain requested native records");
        assert!(collection.columns.iter().any(|column|column.name=="availability"));
        let source=WorkEvidenceLink{extraction_id:WorkArtifactId::from(950),source_id:1};
        Ok(outcome(None,vec![WorkArtifactDraft{output:request.output,title:"Book metadata".into(),evidence:vec![source],data:serde_json::from_value(json!({
            "kind":"comparison_matrix","subjects":[{"name":"Book Alpha","homepage":"https://shop.example.com/book"}],
            "criteria":[{"name":"price","kind":{"kind":"text"}},{"name":"availability","kind":{"kind":"text"}}],
            "cells":[[{"value":{"kind":"text","text":"14.99 PLN"},"evidence":[0]},{"value":{"kind":"text","text":"InStock"},"evidence":[0]}]]
        })).unwrap()}]))
    },|_|{})).await.unwrap();
    let execution = &done.executions[0];
    assert!(matches!(
        execution.status,
        WorkExecutionStatus::Completed | WorkExecutionStatus::NeedsReview
    ));
    let pick = execution
        .artifacts
        .iter()
        .find_map(|artifact| match &artifact.data {
            WorkArtifactDataV1::Picks { items, .. } => items.first(),
            _ => None,
        })
        .unwrap();
    assert_eq!(pick.price.as_ref().unwrap().display, "14.99 PLN");
    assert_eq!(pick.facts[0].value, "InStock");
}
