//! The lead's memory and context tools end to end over a scripted model: a
//! fact kept with its work, a secret refused, history asked for once with the
//! agent's reason and the answer kept, and each source shown as an input.
use super::work_planning_tests::{drive, fixture};
use crate::work_lead::{LeadModel, WorkLeadModels, WorkLeadService};
use crate::WorkIntent;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use zephium_core::ports::store::Store;
use zephium_core::work::{
    model::*, parts::*, personal::*, port::WorkReply, runtime::*, search::*, *,
};
use zephium_ipc::work::WorkCommandV1;

struct Script {
    seen: Mutex<Vec<String>>,
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
            let names: Vec<String> = request.tools.iter().map(|t| t.name.clone()).collect();
            for tool in [
                "remember",
                "recall",
                "search_history",
                "search_notes",
                "read_note",
                "list_tabs",
            ] {
                assert!(names.iter().any(|n| n == tool), "{tool} offered: {names:?}");
            }
            let seen = results(&request);
            self.seen.lock().unwrap().push(seen.clone());
            let assistant = match turn {
                0 => vec![
                    call(
                        "a",
                        "remember",
                        json!({"text": "Prefers aisle seats on long flights", "kind": "preference"}),
                    ),
                    call(
                        "b",
                        "remember",
                        json!({"text": "My password is hunter2", "kind": "fact"}),
                    ),
                    call(
                        "c",
                        "search_history",
                        json!({"query": "flights warsaw", "why": "Looking for the flight comparison you read last week."}),
                    ),
                ],
                1 => vec![
                    call("d", "recall", json!({"query": "aisle"})),
                    call("e", "search_history", json!({})),
                ],
                _ => vec![call("f", "finish", json!({"say": "Done."}))],
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

struct NoSearch;
impl WorkPublicSearchProvider for NoSearch {
    fn search<'a>(
        &'a self,
        _: &'a WorkPublicSearchScope,
        _: &'a [zephium_core::work::context::WorkContextBody],
        _: WorkExecutionLimits,
    ) -> WorkPublicSearchFuture<'a> {
        Box::pin(async { Err(WorkPublicSearchError::NotDispatched(WorkError::Unavailable)) })
    }
}

fn model(client: Arc<Script>) -> LeadModel {
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
            price: None,
        },
        client,
    }
}

fn begin(work: WorkId, expected: WorkRevision) -> WorkCommandV1 {
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

async fn projection(
    handle: &crate::Handle,
    profile: zephium_core::ids::ProfileId,
    work: WorkId,
) -> WorkRuntimeProjection {
    let WorkReply::Runtime(state) = handle
        .work_projection(profile, work)
        .unwrap()
        .await
        .unwrap()
        .reply
    else {
        panic!()
    };
    *state
}

/// Answers every consent question with Allow, counting them, until the run ends.
async fn allow(
    handle: &crate::Handle,
    profile: zephium_core::ids::ProfileId,
    work: WorkId,
    asked: &Mutex<Vec<String>>,
) {
    answer(handle, profile, work, asked, "Allow").await
}

/// Answers every question with `reply`, counting them, until the run ends.
async fn answer(
    handle: &crate::Handle,
    profile: zephium_core::ids::ProfileId,
    work: WorkId,
    asked: &Mutex<Vec<String>>,
    reply: &str,
) {
    loop {
        tokio::time::sleep(Duration::from_millis(20)).await;
        let state = projection(handle, profile, work).await;
        let Some(execution) = state.executions.last() else {
            continue;
        };
        if !matches!(
            execution.status,
            WorkExecutionStatus::Running | WorkExecutionStatus::Approved
        ) && !execution.steps.is_empty()
        {
            return;
        }
        let open = execution.steps.iter().find_map(|step| match &step.kind {
            WorkStepKindV1::Ask {
                prompt,
                answer: None,
                ..
            } if step.status == WorkStepStatus::Running => Some((step.id, prompt.clone())),
            _ => None,
        });
        let Some((step, prompt)) = open else {
            continue;
        };
        if asked.lock().unwrap().contains(&prompt) {
            continue;
        }
        let submitted = handle
            .work_command(
                profile,
                WorkCommandV1 {
                    version: 1,
                    work,
                    expected_revision: state.work.revision,
                    command: WorkCommandId::generate(),
                    intent: WorkRuntimeIntent::AnswerStep {
                        execution: execution.id,
                        step,
                        answer: reply.into(),
                    },
                },
            )
            .unwrap()
            .await;
        if submitted.is_ok() {
            asked.lock().unwrap().push(prompt);
        }
    }
}

#[tokio::test]
async fn work_lead_remembers_asks_once_for_history_and_shows_what_it_read() {
    let store = Arc::new(zephium_store::SqliteStore::in_memory().unwrap());
    let (mut shell, queue, handle, profile) = fixture(store.clone());
    store.record_visit(
        profile,
        "https://www.google.com/travel/flights?q=WAW-SFO".into(),
        "Flights from Warsaw to San Francisco".into(),
    );
    store.record_visit(
        profile,
        "https://news.ycombinator.com/".into(),
        "Hacker News".into(),
    );
    assert!(store.flush());
    let create = handle
        .work_document(WorkIntent::Create {
            objective: "Plan my YC batch trip from Warsaw".into(),
        })
        .unwrap();
    let work = create.work_id().unwrap();
    drive(&mut shell, &queue, create).await.unwrap();
    drive(
        &mut shell,
        &queue,
        crate::work_personal::consent(
            &handle,
            profile,
            work,
            WorkContextSourceV1::Tabs,
            Some(false),
        ),
    )
    .await
    .unwrap();
    let script = Arc::new(Script {
        seen: Mutex::new(Vec::new()),
    });
    let models = WorkLeadModels {
        lead: model(script.clone()),
        page: model(script.clone()),
        light: model(script.clone()),
    };
    let service = WorkLeadService::new(handle.clone());
    let asked = Mutex::new(Vec::new());
    let (state, ()) = drive(&mut shell, &queue, async {
        tokio::join!(
            service.run(
                profile,
                begin(work, WorkRevision::INITIAL),
                None,
                models,
                &NoSearch,
                |_, _| async { Err(WorkError::Unavailable) },
                |_| {},
            ),
            allow(&handle, profile, work, &asked),
        )
    })
    .await;
    let run = &state.unwrap().executions[0];

    let seen = script.seen.lock().unwrap().join("\n");
    assert!(seen.contains("Remembered."), "{seen}");
    assert!(seen.contains("never remember secrets"), "{seen}");
    assert!(
        seen.contains("Flights from Warsaw to San Francisco") && seen.contains("google.com"),
        "{seen}"
    );
    assert!(
        seen.contains("Prefers aisle seats on long flights"),
        "{seen}"
    );
    assert!(seen.contains("Hacker News"), "{seen}");
    assert!(
        !seen.contains("\"open_tabs\""),
        "history disclosed separately refused tabs: {seen}"
    );

    assert_eq!(
        *asked.lock().unwrap(),
        ["Use your history? Looking for the flight comparison you read last week."]
    );
    let inputs: Vec<(WorkInputKindV1, &str)> = run
        .inputs
        .iter()
        .map(|input| (input.kind, input.label.as_str()))
        .collect();
    assert!(
        inputs.contains(&(WorkInputKindV1::History, "Your history")),
        "{inputs:?}"
    );
    assert!(
        inputs.contains(&(WorkInputKindV1::Memory, "Your memory")),
        "{inputs:?}"
    );

    let kept = drive(
        &mut shell,
        &queue,
        crate::work_personal::memories(&handle, profile, None, Some(work), 10),
    )
    .await
    .unwrap();
    assert_eq!(kept.len(), 1);
    assert_eq!(kept[0].text, "Prefers aisle seats on long flights");
    assert_eq!(kept[0].execution, Some(run.id));
    assert_eq!(kept[0].kind, WorkMemoryKindV1::Preference);
    let consent = drive(
        &mut shell,
        &queue,
        crate::work_personal::consent(&handle, profile, work, WorkContextSourceV1::History, None),
    )
    .await
    .unwrap();
    assert_eq!(consent, Some(true));
}

/// Plans a day: asks for its sources once, reads them, remembers them.
struct Day;
impl WorkModelClient for Day {
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
            let assistant = match turn {
                0 => vec![call("d", "day_sources", json!({}))],
                1 => {
                    let seen = results(&request);
                    assert!(
                        seen.contains("\"service\":\"mail.google.com\"")
                            && seen.contains("list_tasks"),
                        "{seen}"
                    );
                    vec![call("t", "list_tasks", json!({"view": "today"}))]
                }
                2 => {
                    let seen = results(&request);
                    assert!(seen.contains("No open task is due by"), "{seen}");
                    vec![call(
                        "f",
                        "finish",
                        json!({"say": "Your day is on the canvas.", "title": "Tuesday plan"}),
                    )]
                }
                _ => vec![],
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
async fn a_day_plan_asks_for_its_sources_once_and_remembers_them() {
    let store = Arc::new(zephium_store::SqliteStore::in_memory().unwrap());
    let (mut shell, queue, handle, profile) = fixture(store.clone());
    store.record_visit(
        profile,
        "https://mail.google.com/mail/u/0/#inbox".into(),
        "Inbox - ana@example.com - Gmail".into(),
    );
    assert!(store.flush());
    let day = Arc::new(Day);
    let lead = LeadModel {
        client: day,
        ..model(Arc::new(Script {
            seen: Mutex::new(Vec::new()),
        }))
    };
    let models = || WorkLeadModels {
        lead: lead.clone(),
        page: lead.clone(),
        light: lead.clone(),
    };
    let service = WorkLeadService::new(handle.clone());
    let mut asked_each = Vec::new();
    for _ in 0..2 {
        let create = handle
            .work_document(WorkIntent::Create {
                objective: "Plan my day".into(),
            })
            .unwrap();
        let work = create.work_id().unwrap();
        drive(&mut shell, &queue, create).await.unwrap();
        let asked = Mutex::new(Vec::new());
        let (state, ()) = drive(&mut shell, &queue, async {
            tokio::join!(
                service.run(
                    profile,
                    begin(work, WorkRevision::INITIAL),
                    None,
                    models(),
                    &NoSearch,
                    |_, _| async { Err(WorkError::Unavailable) },
                    |_| {},
                ),
                answer(&handle, profile, work, &asked, "Use these"),
            )
        })
        .await;
        let run = &state.unwrap().executions[0];
        assert_eq!(run.status, WorkExecutionStatus::NeedsReview);
        asked_each.push(asked.into_inner().unwrap());
    }
    // gh on this Mac may add GitHub; the history found Gmail.
    assert_eq!(asked_each[1], Vec::<String>::new());
    let [asked] = asked_each[0].as_slice() else {
        panic!("{asked_each:?}")
    };
    assert!(
        asked.starts_with("Plan your day from Gmail")
            && asked.ends_with(" and Zephium tasks and notes?"),
        "{asked}"
    );
    let kept = drive(
        &mut shell,
        &queue,
        crate::work_personal::memories(&handle, profile, None, None, 10),
    )
    .await
    .unwrap();
    assert!(
        kept.iter()
            .any(|m| m.text.starts_with("Plans the day from Gmail")
                && m.text.ends_with(" and Zephium tasks and notes")),
        "{:?}",
        kept.iter().map(|m| m.text.clone()).collect::<Vec<_>>()
    );
}
