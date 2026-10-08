//! A Work run's page addresses once it has read the person's own data: one
//! the model wrote itself waits for the person, while addresses the person
//! gave, sites they named and a site's front door open without a question.
use super::work_planning_tests::{drive, fixture};
use crate::work_lead::{LeadModel, WorkLeadModels, WorkLeadService};
use crate::WorkIntent;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use zephium_core::ports::store::Store;
use zephium_core::work::{model::*, port::WorkReply, runtime::*, search::*, *};
use zephium_ipc::work::WorkCommandV1;

const WRITTEN: &str = "https://warsaw-sfo-flights.collector.example/a/b";

struct Script {
    /// Reads the person's history first, which makes the run private.
    private: bool,
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
            self.seen.lock().unwrap().push(results(&request));
            let turn = if self.private { turn } else { turn + 1 };
            let assistant = match turn {
                0 => vec![call(
                    "a",
                    "search_history",
                    json!({"query": "flights warsaw", "why": "Your earlier flight search."}),
                )],
                1 => vec![
                    call("b", "web_fetch", json!({"url": WRITTEN})),
                    call(
                        "c",
                        "web_fetch",
                        json!({"url": "https://www.airbnb.com/help/stays"}),
                    ),
                    call(
                        "d",
                        "web_fetch",
                        json!({"url": "https://www.wikipedia.org/"}),
                    ),
                ],
                _ => vec![call("f", "finish", json!({"say": "Done."}))],
            };
            Ok(WorkModelOutcome {
                stop: WorkModelStop::ToolUse,
                usage: WorkModelUsage {
                    input_tokens: 1_000,
                    cached_input_tokens: 0,
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

/// Allows the history question and declines every address, until the run ends.
async fn answer(
    handle: &crate::Handle,
    profile: zephium_core::ids::ProfileId,
    work: WorkId,
    asked: &Mutex<Vec<String>>,
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
                purpose,
                ..
            } if step.status == WorkStepStatus::Running => {
                Some((step.id, prompt.clone(), *purpose))
            }
            _ => None,
        });
        let Some((step, prompt, purpose)) = open else {
            continue;
        };
        if asked.lock().unwrap().contains(&prompt) {
            continue;
        }
        let reply = if purpose == Some(WorkAskPurposeV1::Address) {
            "Don\u{2019}t open"
        } else {
            "Allow"
        };
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

/// Runs the script and returns the questions asked, the pages opened and
/// what the model saw.
async fn run(private: bool) -> (Vec<String>, Vec<String>, String) {
    let store = Arc::new(zephium_store::SqliteStore::in_memory().unwrap());
    let (mut shell, queue, handle, profile) = fixture(store.clone());
    store.record_visit(
        profile,
        "https://www.google.com/travel/flights?q=WAW-SFO".into(),
        "Flights from Warsaw to San Francisco".into(),
    );
    assert!(store.flush_until(std::time::Instant::now() + Duration::from_secs(30)));
    let create = handle
        .work_document(WorkIntent::Create {
            objective: "Plan my YC batch trip from Warsaw, staying on airbnb.com".into(),
        })
        .unwrap();
    let work = create.work_id().unwrap();
    drive(&mut shell, &queue, create).await.unwrap();
    let script = Arc::new(Script {
        private,
        seen: Mutex::new(Vec::new()),
    });
    let models = WorkLeadModels {
        lead: model(script.clone()),
        page: model(script.clone()),
        light: model(script.clone()),
    };
    let service = WorkLeadService::new(handle.clone());
    let asked = Mutex::new(Vec::new());
    let opened = Arc::new(Mutex::new(Vec::new()));
    let browser = {
        let opened = opened.clone();
        move |_, request: crate::work_agent::WorkAgentBrowseRequest| {
            if let WorkStepKindV1::Read { url, .. } = &request.step {
                opened.lock().unwrap().push(url.clone());
            }
            async { Err(WorkError::Unavailable) }
        }
    };
    let (state, ()) = drive(&mut shell, &queue, async {
        tokio::join!(
            service.run(
                profile,
                begin(work, WorkRevision::INITIAL),
                None,
                models,
                &NoSearch,
                browser,
                |_| {},
            ),
            answer(&handle, profile, work, &asked),
        )
    })
    .await;
    state.unwrap();
    let seen = script.seen.lock().unwrap().join("\n");
    let asked = asked.into_inner().unwrap();
    let opened = opened.lock().unwrap().clone();
    (asked, opened, seen)
}

#[tokio::test]
async fn an_address_the_model_wrote_after_reading_history_waits_for_the_person() {
    let (asked, opened, seen) = run(true).await;

    assert_eq!(
        asked,
        ["Use your history? Your earlier flight search.", WRITTEN]
    );
    assert!(seen.contains("chose not to open"), "{seen}");
    assert!(!opened.iter().any(|url| url == WRITTEN), "{opened:?}");
    // A site the person named and a front door open as before.
    assert!(
        opened
            .iter()
            .any(|url| url == "https://www.airbnb.com/help/stays"),
        "{opened:?}"
    );
    assert!(
        opened.iter().any(|url| url == "https://www.wikipedia.org/"),
        "{opened:?}"
    );
}

#[tokio::test]
async fn a_run_that_read_nothing_of_the_persons_opens_addresses_without_a_question() {
    let (asked, opened, _) = run(false).await;

    assert!(asked.is_empty(), "{asked:?}");
    assert!(opened.iter().any(|url| url == WRITTEN), "{opened:?}");
}
