use super::*;
use crate::work_agent::*;
use crate::{actor::CommandQueue, Shell};
use std::{
    collections::VecDeque,
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex,
    },
};
use zephium_core::ids::ProfileId;
use zephium_core::work::{agent::*, search::*, synthesis::*};

const GRANT: WorkAgentGrantV1 = WorkAgentGrantV1 {
    no_usage_charge: false,
    draft: None,
    lead: None,
    provider: WorkSearchProvider::OpenAi,
    model: String::new(),
    max_turns: 8,
    max_steps: 24,
    browse_hops: 1,
    folders: vec![],
    accounts: Vec::new(),
    private: false,
    skill: None,
    search_fallback: None,
};

fn begin(work: WorkId, expected: WorkRevision, timeout_seconds: u32) -> WorkCommandV1 {
    WorkCommandV1 {
        version: 1,
        work,
        expected_revision: expected,
        command: WorkCommandId::generate(),
        intent: WorkRuntimeIntent::BeginAgent {
            grant: WorkAgentGrantV1 {
                no_usage_charge: false,
                model: PUBLIC_SEARCH_MODEL.into(),
                ..GRANT
            },
            limits: WorkExecutionLimits {
                model_tokens: 200_000,
                cost_micro_usd: 200_000,
                operations: 64,
                timeout_seconds,
                max_workers: 3,
            },
        },
    }
}

fn output(fetch: Vec<WorkAgentFetch>) -> WorkAgentTurnOutput {
    WorkAgentTurnOutput {
        say: None,
        artifacts: vec![],
        fetch,
        ask: None,
        finish: false,
        followups: vec![],
        malformed: 0,
    }
}
fn read(url: &str) -> WorkAgentFetch {
    WorkAgentFetch::Read {
        url: url.into(),
        collection: None,
    }
}
fn search(query: &str) -> WorkAgentFetch {
    WorkAgentFetch::Search {
        query: query.into(),
    }
}
fn asking(prompt: &str) -> WorkAgentTurnOutput {
    WorkAgentTurnOutput {
        ask: Some(WorkAgentQuestion {
            prompt: prompt.into(),
            options: vec!["Yes".into(), "No".into()],
        }),
        ..output(vec![])
    }
}
/// Places a short summary citing the first listed source and finishes.
fn finishing() -> WorkAgentTurnOutput {
    WorkAgentTurnOutput {
        say: Some("Done".into()),
        artifacts: vec![WorkAgentArtifactOutput {
            title: "Summary".into(),
            data: WorkArtifactDataV1::Document {
                paragraphs: vec!["What the sources say".into()],
                formatted: None,
            },
            evidence: vec![1],
            general_knowledge: false,
        }],
        finish: true,
        ..output(vec![])
    }
}

/// What one turn was shown.
struct Seen {
    objective: String,
    thread: Vec<(String, Option<String>)>,
    decisions: Vec<planning::PlanningAnswer>,
    artifacts: usize,
    bytes: usize,
    notices: Vec<String>,
    sites: Vec<(String, &'static str)>,
}
/// Plays scripted turns in order and keeps what each turn saw.
#[derive(Default)]
struct Script {
    turns: Mutex<VecDeque<WorkAgentTurnOutput>>,
    seen: Mutex<Vec<Seen>>,
}
impl Script {
    fn play(&self, turns: impl IntoIterator<Item = WorkAgentTurnOutput>) {
        self.turns.lock().unwrap().extend(turns);
    }
}
impl WorkAgentTurnProvider for Script {
    fn turn<'a>(
        &'a self,
        input: &'a WorkAgentTurnDisclosure,
        _: WorkSynthesisTrace,
    ) -> WorkAgentTurnFuture<'a> {
        Box::pin(async move {
            let context = input.context();
            for source in &context.sources {
                if source.acquired_by == "command" {
                    assert!(source.command.is_some());
                }
            }
            self.seen.lock().unwrap().push(Seen {
                objective: context.objective.clone(),
                thread: context
                    .thread
                    .iter()
                    .map(|entry| (entry.request.clone(), entry.summary.clone()))
                    .collect(),
                decisions: context.decisions.clone(),
                artifacts: context.artifacts.len(),
                bytes: serde_json::to_vec(context).unwrap().len(),
                notices: context.notices.clone(),
                sites: context
                    .sites
                    .iter()
                    .map(|site| (site.site.clone(), site.session))
                    .collect(),
            });
            let output = self
                .turns
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or_else(finishing);
            Ok(WorkAgentTurnResult {
                output,
                usage: WorkUsage {
                    model_tokens: 900,
                    cost_micro_usd: 90,
                    operations: 1,
                    accounting: WorkUsageAccounting::Exact,
                },
            })
        })
    }
}

/// Answers with a few cited sources, or never answers while `hang` is set.
#[derive(Default)]
struct Sources {
    hang: AtomicBool,
}
impl WorkPublicSearchProvider for Sources {
    fn search<'a>(
        &'a self,
        scope: &'a WorkPublicSearchScope,
        _: &'a [zephium_core::work::context::WorkContextBody],
        _: WorkExecutionLimits,
    ) -> WorkPublicSearchFuture<'a> {
        Box::pin(async move {
            if self.hang.load(Ordering::SeqCst) {
                return std::future::pending().await;
            }
            let answer = format!("{} {}", scope.query, "Public detail. ".repeat(280));
            Ok(WorkPublicSearchResult {
                evidence: WorkProviderSearchEvidenceV1 {
                    version: 1,
                    provider: scope.provider,
                    model: scope.model.clone(),
                    response_model: PUBLIC_SEARCH_MODEL.into(),
                    response_id: "resp_lifecycle".into(),
                    search_call_id: "ws_lifecycle".into(),
                    citations: (0..6)
                        .map(|index| WorkProviderSearchCitation {
                            url: format!("https://example.test/{}/{index}", scope.query.len()),
                            title: format!("Source {index}"),
                            start_index: 10 + index * 200,
                            end_index: 40 + index * 200,
                            snippet: None,
                        })
                        .collect(),
                    answer,
                    actual_input_tokens: 400,
                    actual_output_tokens: 200,
                },
                usage: WorkUsage {
                    model_tokens: 600,
                    cost_micro_usd: 60,
                    operations: 1,
                    accounting: WorkUsageAccounting::Exact,
                },
            })
        })
    }
}

/// A page read that honours Stop like the real adapter, or returns notes.
async fn page(
    probe: WorkAttemptProbe,
    request: WorkAgentBrowseRequest,
    hang: bool,
) -> Result<WorkBrowserOutcome, WorkError> {
    let session = probe.browser_session();
    assert!(session.is_current() && probe.deadline() > Instant::now());
    let _admission = probe.admit_read_page(request.id).await?;
    loop {
        if probe.cancellation_requested().await? {
            return Ok(WorkBrowserOutcome {
                status: WorkStepStatus::Cancelled,
                usage: Some(WorkUsage::default()),
                artifacts: vec![],
                intervention: None,
                note: None,
                measurements: None,
                helped: false,
                held_back: false,
                committed: Vec::new(),
                rerun: false,
                search: None,
                redirect: None,
            });
        }
        if !hang {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    Ok(WorkBrowserOutcome {
        status: WorkStepStatus::Succeeded,
        usage: Some(WorkUsage {
            model_tokens: 300,
            cost_micro_usd: 30,
            operations: 1,
            accounting: WorkUsageAccounting::Exact,
        }),
        artifacts: vec![WorkArtifactDraft {
            output: request.output,
            title: "Notes from example.test".into(),
            evidence: vec![WorkEvidenceLink {
                extraction_id: WorkArtifactId::generate(),
                source_id: 1,
            }],
            data: WorkArtifactDataV1::Document {
                paragraphs: (0..8).map(|_| "Page detail. ".repeat(260)).collect(),
                formatted: None,
            },
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
async fn lifecycle_projection_waits_for_a_bounded_admission_slot() {
    let store = Arc::new(zephium_store::SqliteStore::in_memory().unwrap());
    let (mut shell, queue, handle, profile) = fixture(store);
    let work = new_work(&mut shell, &queue, &handle, "Read a public page").await;
    let mut pending = Vec::new();
    loop {
        match handle.work_projection(profile, work) {
            Ok(read) => {
                pending.push(read);
                assert!(pending.len() <= 8);
            }
            Err(WorkError::Capacity) => break,
            Err(error) => panic!("{error:?}"),
        }
    }
    assert!(!pending.is_empty());
    let read = projection(&handle, profile, work);
    tokio::pin!(read);
    std::future::poll_fn(|cx| {
        assert!(std::future::Future::poll(read.as_mut(), cx).is_pending());
        std::task::Poll::Ready(())
    })
    .await;
    let state = drive(&mut shell, &queue, read).await;
    assert_eq!(state.work.id, work);
    drop(pending);
}

async fn admit<T>(submit: impl FnMut() -> Result<T, WorkError>) -> T {
    tokio::time::timeout(
        Duration::from_secs(1),
        crate::work_runtime::admitted(submit),
    )
    .await
    .expect("lifecycle admission remained full for a second")
    .expect("lifecycle submission was refused")
}

async fn projection(
    handle: &crate::Handle,
    profile: ProfileId,
    work: WorkId,
) -> WorkRuntimeProjection {
    let WorkReply::Runtime(state) = admit(|| handle.work_projection(profile, work))
        .await
        .await
        .unwrap()
        .reply
    else {
        panic!()
    };
    *state
}

/// Submits one runtime intent against the current revision, retrying the
/// conflicts the running loop's own commits cause.
async fn command(
    handle: &crate::Handle,
    profile: ProfileId,
    work: WorkId,
    intent: WorkRuntimeIntent,
) {
    for _ in 0..40 {
        let state = projection(handle, profile, work).await;
        let submitted = admit(|| {
            handle.work_command(
                profile,
                WorkCommandV1 {
                    version: 1,
                    work,
                    expected_revision: state.work.revision,
                    command: WorkCommandId::generate(),
                    intent: intent.clone(),
                },
            )
        })
        .await
        .await;
        match submitted {
            Ok(_) => return,
            Err(WorkError::Conflict) => tokio::time::sleep(Duration::from_millis(20)).await,
            Err(error) => panic!("{error:?}"),
        }
    }
    panic!("the intent never applied");
}

/// Waits until the latest execution has a running step the predicate accepts.
async fn running_step(
    handle: &crate::Handle,
    profile: ProfileId,
    work: WorkId,
    accept: impl Fn(&WorkStepKindV1) -> bool,
) -> (WorkExecutionId, WorkStepId) {
    loop {
        let state = projection(handle, profile, work).await;
        if let Some(execution) = state.executions.last() {
            if let Some(step) = execution
                .steps
                .iter()
                .find(|step| step.status == WorkStepStatus::Running && accept(&step.kind))
            {
                return (execution.id, step.id);
            }
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

async fn new_work(
    shell: &mut Shell,
    queue: &CommandQueue,
    handle: &crate::Handle,
    objective: &str,
) -> WorkId {
    let create = handle
        .work_document(WorkIntent::Create {
            objective: objective.into(),
        })
        .unwrap();
    let work = create.work_id().unwrap();
    drive(shell, queue, create).await.unwrap();
    work
}

/// The person's next message: it becomes the objective and starts a run.
async fn request(
    shell: &mut Shell,
    queue: &CommandQueue,
    handle: &crate::Handle,
    work: WorkId,
    text: &str,
) -> WorkRevision {
    let profile = shell_profile(shell);
    let state = drive(shell, queue, projection(handle, profile, work)).await;
    let edit = handle
        .work_document(WorkIntent::Edit {
            id: work,
            expected: state.work.revision,
            edit: WorkUserEdit::SetObjective {
                objective: text.into(),
            },
        })
        .unwrap();
    drive(shell, queue, edit).await.unwrap();
    state.work.revision.next().unwrap()
}
fn shell_profile(shell: &Shell) -> ProfileId {
    shell.windows.focused().unwrap().profile
}

fn settled_with_notes(execution: &WorkExecutionFact) {
    for step in &execution.steps {
        assert_ne!(step.status, WorkStepStatus::Running, "{:?}", step.kind);
        if !matches!(step.status, WorkStepStatus::Succeeded) {
            assert!(
                step.note.as_deref().is_some_and(|note| !note.is_empty()),
                "{:?} {:?} settled without a note",
                step.kind,
                step.status
            );
        }
    }
}

#[tokio::test]
async fn work_waiting_on_a_question_does_not_spend_the_run() {
    let store = Arc::new(zephium_store::SqliteStore::in_memory().unwrap());
    let (mut shell, queue, handle, profile) = fixture(store);
    let work = new_work(&mut shell, &queue, &handle, "Check the visa rules").await;
    let script = Script::default();
    script.play([
        asking("Which passport do you hold?"),
        output(vec![search("visa rules")]),
    ]);
    let sources = Sources::default();
    let service = WorkAgentService::new(handle.clone());
    let answer = async {
        let (execution, step) = running_step(&handle, profile, work, |kind| {
            matches!(kind, WorkStepKindV1::Ask { .. })
        })
        .await;
        tokio::time::sleep(Duration::from_secs(20)).await;
        command(
            &handle,
            profile,
            work,
            WorkRuntimeIntent::AnswerStep {
                execution,
                step,
                answer: "Portuguese".into(),
            },
        )
        .await;
    };
    let (result, ()) = drive(&mut shell, &queue, async {
        tokio::join!(
            service.run(
                profile,
                begin(work, WorkRevision::INITIAL, 10),
                None,
                WorkAgentProviders {
                    turn: &script,
                    search: &sources,
                },
                |probe, request| page(probe, request, false),
                |_| {},
            ),
            answer
        )
    })
    .await;
    let execution = &result.unwrap().executions[0];
    settled_with_notes(execution);
    assert_eq!(execution.status, WorkExecutionStatus::NeedsReview);
    assert!(execution.steps.iter().any(|step| matches!(
        &step.kind,
        WorkStepKindV1::Ask { answer: Some(answer), .. } if answer == "Portuguese"
    )));
    assert!(execution
        .steps
        .iter()
        .any(|step| matches!(step.kind, WorkStepKindV1::Search { .. })
            && step.status == WorkStepStatus::Succeeded));
    assert!(matches!(
        execution.steps.last().unwrap().kind,
        WorkStepKindV1::Finish { .. }
    ));
}

/// Where a run is when the person stops it.
#[derive(Clone, Copy, Debug)]
enum StopAt {
    Search,
    Read,
    Reads,
    Ask,
    Takeover,
}

#[tokio::test]
async fn work_stop_anywhere_settles_every_step_and_the_next_request_runs() {
    for at in [
        StopAt::Search,
        StopAt::Read,
        StopAt::Reads,
        StopAt::Ask,
        StopAt::Takeover,
    ] {
        let store = Arc::new(zephium_store::SqliteStore::in_memory().unwrap());
        let (mut shell, queue, handle, profile) = fixture(store);
        let work = new_work(
            &mut shell,
            &queue,
            &handle,
            "Read https://example.test/a https://example.test/b https://example.test/c",
        )
        .await;
        let script = Script::default();
        script.play([match at {
            StopAt::Search => output(vec![search("visa rules")]),
            StopAt::Read | StopAt::Takeover => output(vec![read("https://example.test/a")]),
            StopAt::Reads => output(
                ["a", "b", "c"]
                    .iter()
                    .map(|page| read(&format!("https://example.test/{page}")))
                    .collect(),
            ),
            StopAt::Ask => asking("Which dates?"),
        }]);
        let sources = Sources::default();
        sources.hang.store(true, Ordering::SeqCst);
        let service = WorkAgentService::new(handle.clone());
        let sessions = Mutex::new(Vec::new());
        let stop = async {
            let (execution, _) = running_step(&handle, profile, work, |kind| match at {
                StopAt::Search => matches!(kind, WorkStepKindV1::Search { .. }),
                StopAt::Ask => matches!(kind, WorkStepKindV1::Ask { .. }),
                _ => matches!(kind, WorkStepKindV1::Read { .. }),
            })
            .await;
            if matches!(at, StopAt::Reads) {
                while projection(&handle, profile, work).await.executions[0]
                    .steps
                    .iter()
                    .filter(|step| step.status == WorkStepStatus::Running)
                    .count()
                    < 3
                {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
            }
            command(
                &handle,
                profile,
                work,
                WorkRuntimeIntent::Cancel {
                    execution,
                    intervention: matches!(at, StopAt::Takeover).then(|| WorkInterventionV1 {
                        kind: WorkInterventionKindV1::HumanTakeover,
                        origin: Some("https://example.test".into()),
                    }),
                },
            )
            .await;
        };
        let (result, ()) = drive(&mut shell, &queue, async {
            tokio::join!(
                service.run(
                    profile,
                    begin(work, WorkRevision::INITIAL, 60),
                    None,
                    WorkAgentProviders {
                        turn: &script,
                        search: &sources,
                    },
                    |probe, request| {
                        sessions.lock().unwrap().push(probe.browser_session());
                        page(probe, request, true)
                    },
                    |_| {},
                ),
                stop
            )
        })
        .await;
        let state = result.unwrap_or_else(|error| panic!("{at:?}: {error:?}"));
        let execution = &state.executions[0];
        settled_with_notes(execution);
        assert!(
            execution.status.terminal(),
            "{at:?}: {:?}",
            execution.status
        );
        assert!(
            matches!(
                execution.status,
                WorkExecutionStatus::Cancelled | WorkExecutionStatus::Interrupted
            ),
            "{at:?}: {:?}",
            execution.status
        );
        assert!(execution
            .steps
            .iter()
            .filter(|step| step.status != WorkStepStatus::Succeeded)
            .all(|step| step.note.as_deref()
                == Some("Stopped by you; any in-flight outcome remains unconfirmed")));
        if matches!(at, StopAt::Takeover) {
            assert_eq!(
                execution.intervention.as_ref().map(|i| i.kind),
                Some(WorkInterventionKindV1::HumanTakeover)
            );
        }
        assert!(sessions
            .lock()
            .unwrap()
            .iter()
            .all(|session| !session.is_current()));

        // The next request runs on the same work.
        let expected = request(&mut shell, &queue, &handle, work, "Continue").await;
        sources.hang.store(false, Ordering::SeqCst);
        script.play([output(vec![search("visa rules")])]);
        let next = drive(
            &mut shell,
            &queue,
            service.run(
                profile,
                begin(work, expected, 60),
                None,
                WorkAgentProviders {
                    turn: &script,
                    search: &sources,
                },
                |probe, request| page(probe, request, false),
                |_| {},
            ),
        )
        .await
        .unwrap_or_else(|error| panic!("{at:?} next: {error:?}"));
        assert_eq!(
            next.executions[1].status,
            WorkExecutionStatus::NeedsReview,
            "{at:?}"
        );
    }
}

#[tokio::test]
async fn work_a_question_stopped_on_is_answered_by_the_next_request() {
    const PROMPT: &str = "Airbnb shows no monthly totals without dates. What should I do next?";
    let store = Arc::new(zephium_store::SqliteStore::in_memory().unwrap());
    let (mut shell, queue, handle, profile) = fixture(store);
    let work = new_work(
        &mut shell,
        &queue,
        &handle,
        "Compare the Airbnb shortlist by monthly total",
    )
    .await;
    let script = Script::default();
    script.play([WorkAgentTurnOutput {
        ask: Some(WorkAgentQuestion {
            prompt: PROMPT.into(),
            options: vec!["Use sample dates".into(), "Skip totals".into()],
        }),
        ..output(vec![])
    }]);
    let sources = Sources::default();
    let service = WorkAgentService::new(handle.clone());
    let stop = async {
        let (execution, _) = running_step(&handle, profile, work, |kind| {
            matches!(kind, WorkStepKindV1::Ask { .. })
        })
        .await;
        command(
            &handle,
            profile,
            work,
            WorkRuntimeIntent::Cancel {
                execution,
                intervention: None,
            },
        )
        .await;
    };
    let providers = WorkAgentProviders {
        turn: &script,
        search: &sources,
    };
    let (stopped, ()) = drive(&mut shell, &queue, async {
        tokio::join!(
            service.run(
                profile,
                begin(work, WorkRevision::INITIAL, 60),
                None,
                WorkAgentProviders { ..providers },
                |probe, request| page(probe, request, false),
                |_| {},
            ),
            stop
        )
    })
    .await;
    let stopped = stopped.unwrap();
    let ask = stopped.executions[0].steps.last().unwrap();
    assert!(matches!(
        &ask.kind,
        WorkStepKindV1::Ask { answer: None, .. }
    ));
    assert_eq!(ask.status, WorkStepStatus::Cancelled);

    let expected = request(&mut shell, &queue, &handle, work, "Use sample dates").await;
    script.play([output(vec![search("airbnb monthly totals")])]);
    drive(
        &mut shell,
        &queue,
        service.run(
            profile,
            begin(work, expected, 60),
            None,
            providers,
            |probe, request| page(probe, request, false),
            |_| {},
        ),
    )
    .await
    .unwrap();
    let seen = script.seen.lock().unwrap();
    let last = seen.last().unwrap();
    assert!(last
        .decisions
        .iter()
        .any(|decision| decision.question == PROMPT && decision.answer == "Use sample dates"));
    let summary = last.thread[0].1.as_deref().unwrap();
    assert!(summary.contains(PROMPT) && summary.contains("Use sample dates; Skip totals"));
}

#[tokio::test]
async fn work_a_run_the_app_quit_during_reads_as_interrupted_and_the_next_request_runs() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(zephium_store::SqliteStore::open(dir.path()).unwrap());
    let (mut shell, queue, handle, profile) = fixture(store.clone());
    let work = new_work(&mut shell, &queue, &handle, "Read https://example.test/a").await;
    let script = Script::default();
    script.play([output(vec![read("https://example.test/a")])]);
    let sources = Sources::default();
    let service = WorkAgentService::new(handle.clone());
    // The process ends mid-read: nothing the dropped run queues is ever applied.
    drive(&mut shell, &queue, async {
        tokio::select! {
            _ = service.run(
                profile,
                begin(work, WorkRevision::INITIAL, 60),
                None,
                WorkAgentProviders { turn: &script, search: &sources },
                |probe, request| page(probe, request, true),
                |_| {},
            ) => panic!("the read never settles"),
            _ = running_step(&handle, profile, work, |kind| matches!(kind, WorkStepKindV1::Read { .. })) => {}
        }
    })
    .await;
    drop(service);
    drop(handle);
    drop(queue);
    drop(shell);
    assert_eq!(
        store.shutdown_until(Instant::now() + Duration::from_secs(5)),
        zephium_core::ports::store::StoreShutdownOutcome::Clean
    );
    drop(store);

    let store = Arc::new(zephium_store::SqliteStore::open(dir.path()).unwrap());
    let (mut shell, queue, handle, _) = fixture(store.clone());
    let state = drive(&mut shell, &queue, projection(&handle, profile, work)).await;
    let execution = state.executions[0].id;
    assert_eq!(state.interrupted, vec![execution]);
    // Sending the next request first acknowledges what the old launch left.
    drive(
        &mut shell,
        &queue,
        command(
            &handle,
            profile,
            work,
            WorkRuntimeIntent::AcknowledgeInterruption { execution },
        ),
    )
    .await;
    let state = drive(&mut shell, &queue, projection(&handle, profile, work)).await;
    let interrupted = &state.executions[0];
    assert_eq!(interrupted.status, WorkExecutionStatus::Interrupted);
    settled_with_notes(interrupted);
    let plan = state.work.plan.as_ref().unwrap();
    interrupted.validate(plan, state.work.revision).unwrap();

    let expected = request(&mut shell, &queue, &handle, work, "Continue").await;
    script.play([output(vec![search("example pages")])]);
    let service = WorkAgentService::new(handle.clone());
    let next = drive(
        &mut shell,
        &queue,
        service.run(
            profile,
            begin(work, expected, 60),
            None,
            WorkAgentProviders {
                turn: &script,
                search: &sources,
            },
            |probe, request| page(probe, request, false),
            |_| {},
        ),
    )
    .await
    .unwrap();
    assert_eq!(next.executions[1].status, WorkExecutionStatus::NeedsReview);
    let seen = script.seen.lock().unwrap();
    assert_eq!(
        seen.last().unwrap().thread[0].0,
        "Read https://example.test/a"
    );
}

#[tokio::test]
async fn work_sixteen_runs_keep_working_within_their_limits() {
    let store = Arc::new(zephium_store::SqliteStore::in_memory().unwrap());
    let (mut shell, queue, handle, profile) = fixture(store);
    let first = "Plan a month in Lisbon: flights, an Airbnb shortlist and the visa rules";
    let work = new_work(&mut shell, &queue, &handle, first).await;
    let script = Script::default();
    let sources = Sources::default();
    let service = WorkAgentService::new(handle.clone());
    let mut expected = WorkRevision::INITIAL;
    for run in 1..=MAX_WORK_EXECUTIONS {
        let text = match run {
            1 => first.to_owned(),
            15 => "check the visa stuff as well".to_owned(),
            16 => "Continue".to_owned(),
            run => format!("Compare option {run} of the shortlist by monthly total"),
        };
        if run > 1 {
            expected = request(&mut shell, &queue, &handle, work, &text).await;
        }
        let query = format!("lisbon {run}");
        let listed = |index| format!("https://example.test/{}/{index}", query.len());
        script.play([
            output(vec![search(&query)]),
            WorkAgentTurnOutput {
                ask: Some(WorkAgentQuestion {
                    prompt: format!("Which dates for option {run}?"),
                    options: vec!["June".into(), "July".into()],
                }),
                ..output(vec![read(&listed(0)), read(&listed(1))])
            },
        ]);
        let answered = AtomicBool::new(false);
        let answer = async {
            let (execution, step) = running_step(&handle, profile, work, |kind| {
                matches!(kind, WorkStepKindV1::Ask { .. })
            })
            .await;
            command(
                &handle,
                profile,
                work,
                WorkRuntimeIntent::AnswerStep {
                    execution,
                    step,
                    answer: "June".into(),
                },
            )
            .await;
            answered.store(true, Ordering::SeqCst);
        };
        let (result, ()) = drive(&mut shell, &queue, async {
            tokio::join!(
                service.run(
                    profile,
                    begin(work, expected, 600),
                    None,
                    WorkAgentProviders {
                        turn: &script,
                        search: &sources,
                    },
                    |probe, request| page(probe, request, false),
                    |_| {},
                ),
                answer
            )
        })
        .await;
        let state = result.unwrap_or_else(|error| panic!("run {run}: {error:?}"));
        let execution = state.executions.last().unwrap();
        assert!(answered.load(Ordering::SeqCst));
        settled_with_notes(execution);
        assert_eq!(
            execution.status,
            WorkExecutionStatus::NeedsReview,
            "run {run}: {:?}",
            execution.steps.last().map(|step| (&step.kind, &step.note))
        );
        assert_eq!(state.executions.len(), run);
    }
    {
        let seen = script.seen.lock().unwrap();
        assert!(seen
            .iter()
            .all(|turn| turn.bytes <= MAX_AGENT_CONTEXT_BYTES));
        for (objective, earlier) in [("check the visa stuff as well", 14), ("Continue", 15)] {
            let turn = seen
                .iter()
                .find(|turn| turn.objective == objective)
                .unwrap();
            assert_eq!(turn.thread.len(), earlier, "{objective}");
            assert_eq!(turn.thread[0].0, first);
            assert!(turn.artifacts > 0, "{objective} still sees the canvas");
        }
    }
    let state = drive(&mut shell, &queue, projection(&handle, profile, work)).await;
    let bodies: Vec<usize> = state
        .executions
        .iter()
        .map(|execution| serde_json::to_vec(execution).unwrap().len())
        .collect();
    let total: usize = bodies.iter().sum();
    eprintln!(
        "sixteen runs: execution bodies {bodies:?} bytes, total {total} bytes, projection {} bytes, largest turn context {} bytes",
        serde_json::to_vec(&state).unwrap().len(),
        script.seen.lock().unwrap().iter().map(|turn| turn.bytes).max().unwrap()
    );
    assert!(bodies.iter().all(|bytes| *bytes <= 512 * 1024));
    assert!(total <= 2 * 1024 * 1024);

    // A seventeenth request is refused plainly; the work stays readable.
    let expected = request(&mut shell, &queue, &handle, work, "One more").await;
    let refused = drive(
        &mut shell,
        &queue,
        service.run(
            profile,
            begin(work, expected, 600),
            None,
            WorkAgentProviders {
                turn: &script,
                search: &sources,
            },
            |probe, request| page(probe, request, false),
            |_| {},
        ),
    )
    .await;
    assert!(refused.is_err(), "a work holds at most sixteen runs");
    let state = drive(&mut shell, &queue, projection(&handle, profile, work)).await;
    assert_eq!(state.executions.len(), MAX_WORK_EXECUTIONS);
}

#[tokio::test]
async fn work_a_human_check_is_read_again_only_after_a_person_continued_it() {
    for helped in [false, true] {
        let store = Arc::new(zephium_store::SqliteStore::in_memory().unwrap());
        let (mut shell, queue, handle, profile) = fixture(store);
        let work = new_work(&mut shell, &queue, &handle, "Read https://example.test/a").await;
        let script = Script::default();
        script.play([
            output(vec![read("https://example.test/a")]),
            output(vec![search("example rules")]),
        ]);
        let sources = Sources::default();
        let calls = std::sync::atomic::AtomicUsize::new(0);
        let state = drive(
            &mut shell,
            &queue,
            WorkAgentService::new(handle.clone()).run(
                profile,
                begin(work, WorkRevision::INITIAL, 60),
                None,
                WorkAgentProviders {
                    turn: &script,
                    search: &sources,
                },
                |_, _| {
                    calls.fetch_add(1, Ordering::SeqCst);
                    async move {
                        Ok(WorkBrowserOutcome {
                            status: WorkStepStatus::Failed,
                            usage: Some(WorkUsage {
                                model_tokens: 10,
                                cost_micro_usd: 1,
                                operations: 1,
                                accounting: WorkUsageAccounting::Exact,
                            }),
                            artifacts: vec![],
                            intervention: None,
                            note: Some(read_note::HUMAN_CHECK.into()),
                            measurements: None,
                            helped,
                            held_back: false,
                            committed: Vec::new(),
                            rerun: false,
                            search: None,
                            redirect: None,
                        })
                    }
                },
                |_| {},
            ),
        )
        .await
        .unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), if helped { 2 } else { 1 });
        let execution = &state.executions[0];
        let read = execution
            .steps
            .iter()
            .find(|step| matches!(step.kind, WorkStepKindV1::Read { .. }))
            .unwrap();
        assert_eq!(read.status, WorkStepStatus::Failed);
        assert_eq!(read.note.as_deref(), Some(read_note::HUMAN_CHECK));
        assert_eq!(execution.status, WorkExecutionStatus::NeedsReview);
    }
}

#[tokio::test]
async fn work_a_person_on_a_page_does_not_spend_the_run() {
    let store = Arc::new(zephium_store::SqliteStore::in_memory().unwrap());
    let (mut shell, queue, handle, profile) = fixture(store);
    let work = new_work(&mut shell, &queue, &handle, "Read https://example.test/a").await;
    let script = Script::default();
    script.play([output(vec![
        search("example rules"),
        read("https://example.test/a"),
    ])]);
    let sources = Sources::default();
    let state = drive(
        &mut shell,
        &queue,
        WorkAgentService::new(handle.clone()).run(
            profile,
            begin(work, WorkRevision::INITIAL, 3),
            None,
            WorkAgentProviders {
                turn: &script,
                search: &sources,
            },
            |probe, request| async move {
                // The page is presented past the run's deadline, then continued.
                let hold = probe.hold_for_person();
                tokio::time::sleep(Duration::from_secs(5)).await;
                assert!(probe.deadline() > Instant::now());
                drop(hold);
                page(probe, request, false).await
            },
            |_| {},
        ),
    )
    .await
    .unwrap();
    let execution = &state.executions[0];
    settled_with_notes(execution);
    assert_eq!(execution.status, WorkExecutionStatus::NeedsReview);
    assert!(execution
        .steps
        .iter()
        .any(|step| matches!(step.kind, WorkStepKindV1::Read { .. })
            && step.status == WorkStepStatus::Succeeded));
}

#[path = "work_local_tests.rs"]
mod local_tests;

#[path = "work_site_session_tests.rs"]
mod site_session_tests;
