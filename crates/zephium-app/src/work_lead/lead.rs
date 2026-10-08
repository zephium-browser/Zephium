//! The lead's conversation: call the model, run the tool calls of a turn
//! together, return compact results, and stop on finish, a stop, the
//! budget or a turn that makes no progress. There is no fixed turn cap.
use std::collections::BTreeSet;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex, MutexGuard};

use serde_json::Value;
use zephium_core::work::{
    model::*,
    parts::{WorkInputFactV1, WorkInputKindV1, MAX_WORK_PARTS},
    runtime::*,
    search::WorkPublicSearchProvider,
    *,
};
use zephium_ipc::work::WorkActivityV1;

use super::call::{self, clip, CallFailure, WorkLeadModels};
use super::hands::{Hands, Request, SharedBrowser};
use super::objects::{self, ObjectRefusal};
use super::parts;
use super::prompt;
use super::run::LeadRun;
use super::skills::Skill;
use super::tools::{LeadHelper, LeadRunView, LeadScope, LeadToolContext, LeadToolSet};
use crate::work_agent::{WorkAgentBrowseRequest, WorkBrowserOutcome};
use crate::work_runtime::{WorkAttemptProbe, WorkNodeAttempt};

/// The lead stops making calls below this much budget and asks first.
/// Output one lead turn may produce: tool calls and a reply, never an essay.
const LEAD_TURN_OUTPUT: u32 = 8_000;
/// Said to the lead after a turn ran past its output limit.
const RAN_LONG: &str = "Your last turn ran past its length limit and was discarded. Act now: call the tools you need, with short arguments, and keep any reply brief.";
const FLOOR_COST: u32 = 40_000;
const FLOOR_TOKENS: u32 = 24_000;
/// Consecutive turns that call no tool before the run closes with what it
/// has.
const MAX_IDLE: u8 = 4;
/// Consecutive turns whose calls were all refused with feedback, or only
/// read the canvas or a skill, before the run closes with what it has.
const MAX_STUCK: u8 = 8;
const MAX_FAILED_CALLS: u8 = 3;
/// Conversation text past which older tool results are shortened.
const CONVERSATION_CHARS: usize = 240_000;
const KEEP_GOING: &str = "Keep going";
/// The lead's own searches and page reads in one request; wide research
/// goes to parts, which keep page text out of the lead's view.
const LEAD_SEARCHES: usize = 4;
const LEAD_READS: usize = 8;
/// A part's own object stays compact; the lead composes the result.
const PART_SHEET_ROWS: usize = 12;
const PART_SHEET_COLUMNS: usize = 6;

#[derive(Default)]
struct LeadState {
    parts_started: usize,
    parts_running: usize,
    reply: Option<WorkArtifactId>,
    finish_refusals: u8,
    title_refused: bool,
    /// Objects refused in this run, by what they are, and how often.
    refused: std::collections::BTreeMap<String, u8>,
    skills: Vec<String>,
    steers: BTreeSet<WorkStepId>,
    searches: usize,
    reads: usize,
}

pub(crate) struct Lead<'a, B> {
    pub run: &'a LeadRun,
    pub attempt: &'a WorkNodeAttempt,
    pub models: &'a WorkLeadModels,
    pub search: &'a dyn WorkPublicSearchProvider,
    pub browser: &'a SharedBrowser<B>,
    pub hands: Hands<'a, B>,
    pub skills: Vec<Skill>,
    pub extra: Vec<Arc<dyn LeadToolSet>>,
    pub helpers: Vec<Arc<dyn LeadHelper>>,
    pub objective: String,
    pub context: String,
    /// What the person's memory holds, as the prompt's stable tail.
    pub memory: Option<String>,
    pub base_limits: WorkExecutionLimits,
    /// No run of this work has named it yet: finish gives it a title.
    pub name_work: bool,
    pub slots: tokio::sync::Semaphore,
    state: Mutex<LeadState>,
}

type Answer = (usize, String, bool);
type Pending<'f> = Pin<Box<dyn Future<Output = Vec<Answer>> + Send + 'f>>;

impl<'a, B, Fut> Lead<'a, B>
where
    B: FnMut(WorkAttemptProbe, WorkAgentBrowseRequest) -> Fut + Send,
    Fut: Future<Output = Result<WorkBrowserOutcome, WorkError>> + Send,
{
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        run: &'a LeadRun,
        attempt: &'a WorkNodeAttempt,
        models: &'a WorkLeadModels,
        search: &'a dyn WorkPublicSearchProvider,
        browser: &'a SharedBrowser<B>,
        hands: Hands<'a, B>,
        skills: Vec<Skill>,
        objective: String,
        context: String,
        memory: Option<String>,
    ) -> Self {
        Self {
            run,
            attempt,
            models,
            search,
            browser,
            hands,
            skills,
            extra: super::registry::tool_sets(),
            helpers: super::registry::helpers(),
            objective,
            context,
            memory,
            base_limits: run.limits(),
            name_work: false,
            slots: tokio::sync::Semaphore::new(parts::PARALLEL_PARTS),
            state: Mutex::new(LeadState::default()),
        }
    }
    fn state(&self) -> MutexGuard<'_, LeadState> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
    pub(crate) fn claim_part(&self) -> bool {
        let mut state = self.state();
        if state.parts_started >= MAX_WORK_PARTS {
            return false;
        }
        state.parts_started += 1;
        state.parts_running += 1;
        true
    }
    /// Counts one of the lead's own searches or reads; false past the cap.
    fn spend(&self, search: bool) -> bool {
        let mut state = self.state();
        let (used, cap) = if search {
            (&mut state.searches, LEAD_SEARCHES)
        } else {
            (&mut state.reads, LEAD_READS)
        };
        *used += 1;
        *used <= cap
    }
    pub(crate) fn release_part(&self) {
        let mut state = self.state();
        state.parts_running = state.parts_running.saturating_sub(1);
    }
    pub(crate) fn parts_running(&self) -> usize {
        self.state().parts_running
    }

    fn system(&self) -> Vec<WorkModelSystemBlock> {
        let mut index = String::from("Skills (load the one that fits before you start):\n");
        for skill in &self.skills {
            index.push_str(&format!("- {}: {}\n", skill.name, skill.description));
        }
        if self.skills.is_empty() {
            index.push_str("- none\n");
        }
        if let Some(memory) = &self.memory {
            index.push_str(
                "\nWhat you know about the person (their memory; use it, never repeat it back):\n",
            );
            index.push_str(memory);
        }
        vec![
            WorkModelSystemBlock {
                text: prompt::CORE.to_owned(),
                cache: false,
            },
            WorkModelSystemBlock {
                text: index,
                cache: true,
            },
        ]
    }
    fn tools(&self) -> Vec<WorkModelTool> {
        let mut tools = prompt::lead_tools();
        let view = LeadRunView {
            run: self.run,
            service: None,
        };
        for set in &self.extra {
            for tool in set.tools(LeadScope::Lead, &view) {
                if !tools.iter().any(|t| t.name == tool.name) {
                    tools.push(tool);
                }
            }
        }
        tools
    }

    /// Runs the conversation to its end.
    pub(crate) async fn drive(&self) -> Result<WorkAttemptStatus, WorkError> {
        let system = self.system();
        let tools = self.tools();
        let mut context = self.context.clone();
        if let Some(name) = self
            .run
            .grant
            .skill
            .as_deref()
            .filter(|name| self.skills.iter().any(|skill| skill.name == *name))
        {
            let (body, refused) = self.load_skill(&serde_json::json!({ "name": name })).await;
            if !refused {
                context.push_str(&format!(
                    "\n\nThe person started this from the {} workflow, whose skill is loaded; follow it:\n{body}",
                    skill_label(name)
                ));
            }
        }
        let mut messages = vec![WorkModelMessage::User(vec![WorkModelPart::Text(context)])];
        let mut idle = 0u8;
        let mut stuck = 0u8;
        let mut failed_calls = 0u8;
        // A turn that ran out of output is asked again once, briefly.
        let mut ran_long = false;
        loop {
            if self.run.cancelled().await {
                return Ok(WorkAttemptStatus::Cancelled);
            }
            if let Some(status) = self.budget().await? {
                return Ok(status);
            }
            if self.run.next_turn() == u8::MAX {
                return self.close(Stall::Turns).await;
            }
            self.run.activity(WorkActivityV1::Planning);
            self.steers(&mut messages).await;
            compact(&mut messages);
            let sizes = Sections::of(&system, &tools, &messages);
            let model = &self.models.lead;
            let request = WorkModelRequest {
                model: model.entry.model.clone(),
                system: system.clone(),
                tools: tools.clone(),
                messages: messages.clone(),
                max_output_tokens: model.entry.max_output.clamp(4_096, LEAD_TURN_OUTPUT),
                reasoning: model.entry.supports.reasoning.then_some(if ran_long {
                    WorkModelReasoning::Low
                } else {
                    WorkModelReasoning::Medium
                }),
                native_search: false,
                parallel_tools: true,
            };
            let (outcome, usage) = match call::call(self.run, model, request).await {
                Ok(done) => done,
                Err(CallFailure::Stopped) => return Ok(WorkAttemptStatus::Cancelled),
                Err(CallFailure::Lost) => {
                    let mut step =
                        self.run
                            .step(WorkStepKindV1::Turn, WorkStepStatus::OutcomeUnknown, None);
                    step.note =
                        Some("The answer was lost on the way; it may have been charged".into());
                    let _ = self.run.begin(step, vec![]).await;
                    return Err(WorkError::OutcomeUnknown);
                }
                Err(CallFailure::Refused(error)) => {
                    failed_calls += 1;
                    self.record_turn(
                        None,
                        WorkStepStatus::Failed,
                        None,
                        CallFailure::Refused(error),
                    )
                    .await;
                    let fatal = matches!(
                        error,
                        WorkModelError::MissingKey
                            | WorkModelError::Unauthorized
                            | WorkModelError::OverBudget
                    );
                    if fatal {
                        return Ok(WorkAttemptStatus::Failed);
                    }
                    if failed_calls >= MAX_FAILED_CALLS {
                        return self.close(Stall::Model).await;
                    }
                    continue;
                }
            };
            failed_calls = 0;
            if outcome.stop == WorkModelStop::MaxTokens && !ran_long {
                ran_long = true;
                self.turn_step(None, usage, None).await;
                messages.push(WorkModelMessage::User(vec![WorkModelPart::Text(
                    RAN_LONG.into(),
                )]));
                continue;
            }
            let text = call::text(&outcome.assistant);
            let calls = call::tool_calls(&outcome.assistant);
            self.turn_step(None, usage, call::say_line(&text)).await;
            self.run.report(super::WorkLeadDiagnostic::Turn {
                turn: self.run.turn(),
                calls: calls.len(),
                tokens: usage.model_tokens,
                cost_micro_usd: usage.cost_micro_usd,
                input: u32::try_from(outcome.usage.input_tokens).unwrap_or(u32::MAX),
                cached: u32::try_from(outcome.usage.cached_input_tokens).unwrap_or(u32::MAX),
                output: u32::try_from(outcome.usage.output_tokens).unwrap_or(u32::MAX),
                prompt_chars: sizes.prompt,
                tools_chars: sizes.tools,
                context_chars: sizes.context,
                conversation_chars: sizes.conversation,
            });
            messages.push(WorkModelMessage::Assistant(outcome.assistant));
            if calls.is_empty() {
                idle += 1;
                if idle >= MAX_IDLE {
                    return self.close(Stall::NoProgress).await;
                }
                let nudge = if self.state().reply.is_some() {
                    "If the result stands, call finish; otherwise continue the work with your tools."
                } else {
                    "Do the work with your tools. The result needs its objects and a reply before finish."
                };
                messages.push(WorkModelMessage::User(vec![WorkModelPart::Text(
                    nudge.into(),
                )]));
                continue;
            }
            let (results, progress, finished) = match self.execute(&calls).await {
                Ok(done) => done,
                Err(status) => return Ok(status),
            };
            messages.push(WorkModelMessage::ToolResults(results));
            if finished {
                return Ok(WorkAttemptStatus::Succeeded);
            }
            idle = 0;
            if progress {
                stuck = 0;
            } else {
                stuck += 1;
                if stuck >= MAX_STUCK {
                    return self.close(Stall::NoProgress).await;
                }
            }
        }
    }

    /// The tool calls of one turn, run together; `finish` last.
    async fn execute(
        &self,
        calls: &[WorkModelToolCall],
    ) -> Result<(Vec<WorkModelToolResult>, bool, bool), WorkAttemptStatus> {
        let mut terminal = None;
        let mut answers: Vec<Option<(String, bool)>> = vec![None; calls.len()];
        let mut requests: Vec<(usize, Request)> = Vec::new();
        let mut pending: Vec<Pending<'_>> = Vec::new();
        let mut finish_at = None;
        let mut ran = vec![false; calls.len()];
        for (index, tool_call) in calls.iter().enumerate() {
            let args = &tool_call.arguments;
            match tool_call.name.as_str() {
                "web_search" => {
                    let mut args = args.clone();
                    if let (Some(query), Some(fresh)) = (
                        args.get("query").and_then(Value::as_str).map(str::to_owned),
                        args.get("freshness").and_then(Value::as_str),
                    ) {
                        let suffix = match fresh {
                            "day" => " (past day)",
                            "week" => " (past week)",
                            "month" => " (past month)",
                            _ => " (past year)",
                        };
                        args["query"] = Value::String(format!("{query}{suffix}"));
                    }
                    match parts::step_request("web_search", &args, self.run) {
                        Ok(_) if !self.spend(true) => {
                            answers[index] = Some((
                                "You have searched enough for this request: build the result from what you have, or start a research part for a new thread.".into(),
                                true,
                            ))
                        }
                        Ok(kind) => requests.push((
                            index,
                            Request {
                                call: tool_call.id.clone(),
                                kind,
                                mine: false,
                                view: false,
                            },
                        )),
                        Err(fault) => answers[index] = Some((fault, true)),
                    }
                }
                "web_fetch" => match parts::step_request("read", args, self.run) {
                    Ok(_) if !self.spend(false) => {
                        answers[index] = Some((
                            "You have read enough pages yourself for this request: build the result, or start a part for the pages that remain.".into(),
                            true,
                        ))
                    }
                    Ok(kind) => requests.push((
                        index,
                        Request {
                            call: tool_call.id.clone(),
                            kind,
                            mine: false,
                            view: false,
                        },
                    )),
                    Err(fault) => answers[index] = Some((fault, true)),
                },
                "start_part" => match parts::spec(args) {
                    Ok(spec) => {
                        ran[index] = true;
                        pending.push(Box::pin(async move {
                            let (content, error) = self.part(spec).await;
                            vec![(index, content, error)]
                        }))
                    }
                    Err(fault) => answers[index] = Some((fault, true)),
                },
                "create" => {
                    let args = args.clone();
                    pending.push(Box::pin(async move {
                        let part = args
                            .get("part")
                            .and_then(Value::as_str)
                            .and_then(|id| WorkPartId::parse(id.trim()));
                        let (content, error) = self.create(&args, part, false).await;
                        vec![(index, content, error)]
                    }))
                }
                "revise" => {
                    let args = args.clone();
                    pending.push(Box::pin(async move {
                        let (content, error) = self.revise(&args).await;
                        vec![(index, content, error)]
                    }))
                }
                "read_canvas" => answers[index] = Some(self.read_canvas(args).await),
                "ask" => {
                    let args = args.clone();
                    pending.push(Box::pin(async move {
                        let (content, error) = self.ask(&args).await;
                        vec![(index, content, error)]
                    }))
                }
                // Answered before the turn's other calls run, so a part
                // started beside it already has the folder.
                "ask_folder" => {
                    let text = |key: &str| args.get(key).and_then(Value::as_str).unwrap_or("");
                    answers[index] =
                        Some(super::folders::request(self.run, text("folder"), text("why")).await);
                }
                "load_skill" => answers[index] = Some(self.load_skill(args).await),
                "finish" => finish_at = Some(index),
                name => match self
                    .extra
                    .iter()
                    .find(|set| {
                        set.tools(
                            LeadScope::Lead,
                            &LeadRunView {
                                run: self.run,
                                service: None,
                            },
                        )
                            .iter()
                            .any(|tool| tool.name == name)
                    })
                    .cloned()
                {
                    Some(set) => {
                        let tool_call = tool_call.clone();
                        pending.push(Box::pin(async move {
                            let context = LeadToolContext {
                                run: self.run,
                                part: None,
                            };
                            let outcome = set.call(context, tool_call).await;
                            // Personal, project, computer and connection tools
                            // all answer with the person's own data.
                            self.run.mark_private();
                            vec![(index, outcome.content, outcome.is_error)]
                        }))
                    }
                    None => answers[index] = Some((format!("{name} is not a tool"), true)),
                },
            }
        }
        if !requests.is_empty() {
            let hands = &self.hands;
            let terminal = &mut terminal;
            let batch = requests;
            pending.push(Box::pin(async move {
                let order: Vec<(usize, String)> =
                    batch.iter().map(|(i, r)| (*i, r.call.clone())).collect();
                match hands.run(batch.into_iter().map(|(_, r)| r).collect()).await {
                    Ok(done) => done
                        .into_iter()
                        .filter_map(|(id, content, error)| {
                            order
                                .iter()
                                .find(|(_, call)| *call == id)
                                .map(|(i, _)| (*i, content, error))
                        })
                        .collect(),
                    Err(status) => {
                        *terminal = Some(status);
                        Vec::new()
                    }
                }
            }));
        }
        for (index, content, error) in join_all(pending).await.into_iter().flatten() {
            answers[index] = Some((content, error));
        }
        if let Some(status) = terminal {
            return Err(status);
        }
        // A part that ran moved the work even when it could not do its job;
        // reading the canvas or a skill alone does not.
        let progress = answers
            .iter()
            .zip(calls)
            .enumerate()
            .any(|(i, (answer, c))| {
                ran[i]
                    || (!matches!(c.name.as_str(), "read_canvas" | "load_skill" | "finish")
                        && answer.as_ref().is_some_and(|(_, error)| !error))
            });
        let mut finished = false;
        if let Some(index) = finish_at {
            let others_failed = answers
                .iter()
                .enumerate()
                .any(|(i, a)| i != index && a.as_ref().is_some_and(|(_, e)| *e));
            let (content, error) = if others_failed {
                (
                    "Not finished: a call in this turn failed; correct it first.".into(),
                    true,
                )
            } else {
                self.finish(&calls[index].arguments).await
            };
            finished = !error;
            answers[index] = Some((content, error));
        }
        let results = answers
            .into_iter()
            .zip(calls)
            .map(|(answer, c)| {
                let (content, is_error) = answer.unwrap_or_else(|| ("Not run".into(), true));
                WorkModelToolResult {
                    call: c.id.clone(),
                    content,
                    is_error,
                }
            })
            .collect();
        Ok((results, progress || finished, finished))
    }

    /// Places an object; a helper may place only its part's found things.
    /// A second try of an object that was refused stands without its
    /// offending optional parts; a second try of a copy or a second reply
    /// updates the object that is already there.
    pub(crate) async fn create(
        &self,
        args: &Value,
        part: Option<WorkPartId>,
        helper: bool,
    ) -> (String, bool) {
        let Some(kind) = args.get("kind").and_then(Value::as_str) else {
            return ("kind is required".into(), true);
        };
        if helper && !matches!(kind, "picks" | "sheet") {
            return (
                "A part places at most one compact object, picks or a small sheet, and hands every other fact to the lead in finish's digest".into(),
                true,
            );
        }
        if !helper && kind == "project" {
            return (
                "A project object comes from describe_project, which reads the folder exactly"
                    .into(),
                true,
            );
        }
        let key = format!(
            "create {kind} {}",
            part.map(|p| p.to_string()).unwrap_or_default()
        );
        let lenient = self.tried(&key);
        if kind == "reply" {
            let reply = self.state().reply;
            if let Some(reply) = reply {
                if lenient {
                    return self.revise(&with_id(args, reply)).await;
                }
                return self.refuse(
                    &key,
                    ObjectRefusal::Second,
                    format!("This request already has its reply ({reply}); revise it instead"),
                );
            }
        }
        let projection = match self.run.probe.runtime_projection().await {
            Ok(projection) => projection,
            Err(_) => return ("The canvas could not be read".into(), true),
        };
        let execution = self.run.probe.execution();
        if let Some(part) = part {
            let known = projection
                .executions
                .iter()
                .find(|e| e.id == execution)
                .is_some_and(|e| e.parts.iter().any(|p| p.id == part));
            if !known {
                return ("part names no part of this request".into(), true);
            }
        }
        let canvas = objects::canvas(&projection, execution);
        let title = args.get("title").and_then(Value::as_str).unwrap_or("");
        if helper {
            if let Some(placed) = canvas
                .iter()
                .find(|o| o.in_this_run && o.artifact.part.is_some() && o.artifact.part == part)
            {
                if lenient && placed.artifact.data.kind_name() == kind {
                    return self.revise(&with_id(args, placed.artifact.id)).await;
                }
                return self.refuse(
                    &key,
                    ObjectRefusal::Second,
                    format!(
                        "Your part already placed its object ({}); put every other fact in finish's digest",
                        placed.artifact.id
                    ),
                );
            }
        } else if let Some(held) = (kind == "picks")
            .then(|| {
                let names: Vec<String> = args
                    .pointer("/data/items")
                    .and_then(Value::as_array)
                    .map(|items| {
                        items
                            .iter()
                            .filter_map(|item| item.get("name").and_then(Value::as_str))
                            .map(str::to_owned)
                            .collect()
                    })
                    .unwrap_or_default();
                objects::part_holds(&canvas, &names)
            })
            .flatten()
        {
            if lenient {
                return self.revise(&with_id(args, held.artifact.id)).await;
            }
            return self.refuse(
                &key,
                ObjectRefusal::Duplicate,
                format!(
                    "picks {} from the part already holds these things and stands in the result as it is; add the reply around it, and revise it (id {}) only to change it",
                    held.artifact.id, held.artifact.id
                ),
            );
        } else if let Some(existing) = objects::duplicate(&canvas, kind, title, part) {
            if lenient {
                return self.revise(&with_id(args, existing.artifact.id)).await;
            }
            return self.refuse(
                &key,
                ObjectRefusal::Duplicate,
                format!(
                    "{kind} {} \"{}\" already covers this subject: revise it with revise (id {}) instead of placing a copy",
                    existing.artifact.id, existing.artifact.title, existing.artifact.id
                ),
            );
        }
        let data = args.get("data").cloned().unwrap_or(Value::Null);
        let sources = strings(args.get("sources"));
        let proposed =
            match objects::propose(self.run, &canvas, kind, title, data, &sources, &[], lenient) {
                Ok(proposed) => proposed,
                Err((fault, reason)) => return self.refuse(&key, reason, fault),
            };
        if helper {
            if let zephium_core::work::artifact::WorkArtifactDataV1::Sheet {
                columns, rows, ..
            } = &proposed.data
            {
                if rows.len() > PART_SHEET_ROWS || columns.len() > PART_SHEET_COLUMNS {
                    return self.refuse(
                        &key,
                        ObjectRefusal::Field(
                            zephium_core::work::artifact::WorkArtifactField::SheetRows,
                        ),
                        format!("A part's sheet is small: it has {} rows and {} columns; the limit is {PART_SHEET_ROWS} rows and {PART_SHEET_COLUMNS} columns, and the rest goes to the lead in finish's digest", rows.len(), columns.len()),
                    );
                }
            }
        }
        let mut proposed = proposed;
        let run_honesty = honesty(&projection, execution, &proposed);
        if let Err(fault) = objects::honest(&proposed.data, &run_honesty) {
            match objects::mend(&proposed.data, &run_honesty).filter(|_| lenient) {
                Some((data, words)) => {
                    proposed.data = data;
                    proposed.left_out.push(words);
                }
                None => return self.refuse(&key, ObjectRefusal::Honesty, fault),
            }
        }
        let left_out = proposed.left_out.clone();
        match objects::publish(self.run, proposed, part, None).await {
            Ok(id) => {
                self.placed(&key);
                if kind == "reply" {
                    self.state().reply = Some(id);
                }
                self.run.activity(WorkActivityV1::ProducingArtifact);
                (
                    placed_words(&format!("Placed {kind} {id}"), &left_out),
                    false,
                )
            }
            Err(error) => (publish_fault(error), true),
        }
    }

    async fn revise(&self, args: &Value) -> (String, bool) {
        let Some(id) = args
            .get("id")
            .and_then(Value::as_str)
            .and_then(|id| WorkArtifactId::parse(id.trim()))
        else {
            return ("id names an object on the canvas".into(), true);
        };
        let projection = match self.run.probe.runtime_projection().await {
            Ok(projection) => projection,
            Err(_) => return ("The canvas could not be read".into(), true),
        };
        let canvas = objects::canvas(&projection, self.run.probe.execution());
        let Some(target) = objects::newest(&canvas, id).cloned() else {
            return ("id names no object on this canvas".into(), true);
        };
        let kind = target.artifact.data.kind_name();
        if kind == "diff" && target.artifact.part.is_some() {
            return (
                "A diff a part placed is the exact change to the file and stays as it is".into(),
                true,
            );
        }
        if kind == "project" {
            return (
                "A project object is an exact reading of its folder: call describe_project again to update it".into(),
                true,
            );
        }
        if kind == "reply" && !target.in_this_run {
            return (
                "A reply belongs to its own request: make a new reply for this one with create"
                    .into(),
                true,
            );
        }
        let key = format!("revise {}", target.artifact.id);
        let lenient = self.tried(&key);
        // A revised object keeps its name unless its subject changed; what
        // changed shows as its update, never as a longer title.
        let title = args
            .get("title")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|t| {
                !t.is_empty()
                    && !t
                        .to_lowercase()
                        .starts_with(&target.artifact.title.to_lowercase())
            })
            .unwrap_or(&target.artifact.title)
            .to_owned();
        let data = args.get("data").cloned().unwrap_or(Value::Null);
        let sources = strings(args.get("sources"));
        let proposed = match objects::propose(
            self.run,
            &canvas,
            kind,
            &title,
            data,
            &sources,
            &target.artifact.evidence,
            lenient,
        ) {
            Ok(proposed) => proposed,
            Err((fault, reason)) => return self.refuse(&key, reason, fault),
        };
        if let Err(fault) = objects::honest(
            &proposed.data,
            &honesty(&projection, self.run.probe.execution(), &proposed),
        ) {
            return self.refuse(&key, ObjectRefusal::Honesty, fault);
        }
        let left_out = proposed.left_out.clone();
        let part = target.in_this_run.then_some(target.artifact.part).flatten();
        match objects::publish(self.run, proposed, part, Some(target.artifact.id)).await {
            Ok(new) => {
                self.placed(&key);
                if kind == "reply" {
                    self.state().reply = Some(new);
                }
                let forwarded = if target.artifact.id == id {
                    String::new()
                } else {
                    format!(" (its newest version was {})", target.artifact.id)
                };
                (
                    placed_words(
                        &format!("Updated {kind}: {new} now stands in place of {id}{forwarded}"),
                        &left_out,
                    ),
                    false,
                )
            }
            Err(error) => (publish_fault(error), true),
        }
    }

    /// Whether this object was refused before in this run: its next try is
    /// lenient.
    fn tried(&self, key: &str) -> bool {
        self.state().refused.contains_key(key)
    }
    fn placed(&self, key: &str) {
        self.state().refused.remove(key);
    }
    /// A refusal with its feedback for the model, logged by its closed reason.
    fn refuse(&self, key: &str, reason: ObjectRefusal, words: String) -> (String, bool) {
        *self.state().refused.entry(key.to_owned()).or_default() += 1;
        self.run.report(super::WorkLeadDiagnostic::ObjectRefused {
            reason: Some(reason),
        });
        let words = if matches!(
            reason,
            ObjectRefusal::Field(_) | ObjectRefusal::Shape | ObjectRefusal::Pick
        ) {
            format!("{words}\nCorrect it and call again; if the same object comes back once more, optional parts still outside their limits are left out.")
        } else {
            words
        };
        (words, true)
    }

    async fn read_canvas(&self, args: &Value) -> (String, bool) {
        let Ok(projection) = self.run.probe.runtime_projection().await else {
            return ("The canvas could not be read".into(), true);
        };
        let canvas = objects::canvas(&projection, self.run.probe.execution());
        let ids = strings(args.get("ids"));
        if ids.is_empty() {
            let view = objects::view(&canvas);
            (
                if view.is_empty() {
                    "The canvas has no objects yet.".into()
                } else {
                    view
                },
                false,
            )
        } else {
            (objects::read(self.run, &canvas, &ids), false)
        }
    }

    async fn ask(&self, args: &Value) -> (String, bool) {
        let Some(question) = args
            .get("question")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|q| !q.is_empty())
        else {
            return ("question is required".into(), true);
        };
        if question.chars().count() > 240 {
            return ("question is at most 240 characters".into(), true);
        }
        let mut options: Vec<String> = Vec::new();
        for option in strings(args.get("options")) {
            let option = clip(option.trim(), 80);
            if !option.is_empty() && !options.contains(&option) && options.len() < 4 {
                options.push(option);
            }
        }
        match self
            .run
            .ask(
                WorkAskPurposeV1::Question,
                question.to_owned(),
                options,
                None,
            )
            .await
        {
            Ok(Some(answer)) => (format!("The person answered: {answer}"), false),
            Ok(None) => ("No answer: the run stopped while waiting".into(), true),
            Err(_) => ("The question could not be recorded".into(), true),
        }
    }

    async fn load_skill(&self, args: &Value) -> (String, bool) {
        let name = args
            .get("name")
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or("");
        let Some(skill) = self.skills.iter().find(|s| s.name == name) else {
            let names: Vec<&str> = self.skills.iter().map(|s| s.name.as_str()).collect();
            return (
                format!("No skill named {name}; the skills are {}", names.join(", ")),
                true,
            );
        };
        let first = {
            let mut state = self.state();
            let first = !state.skills.contains(&skill.name);
            if first {
                state.skills.push(skill.name.clone());
            }
            first
        };
        if first {
            self.run
                .input(WorkInputFactV1 {
                    kind: WorkInputKindV1::Skill,
                    label: skill_label(&skill.name),
                    count: None,
                    reference: Some(skill.name.clone()),
                })
                .await;
            self.run.report(super::WorkLeadDiagnostic::SkillLoaded {
                builtin: skill.builtin,
            });
        }
        (skill.body.clone(), false)
    }

    async fn finish(&self, args: &Value) -> (String, bool) {
        if self.state().reply.is_none() {
            let refusals = {
                let mut state = self.state();
                state.finish_refusals += 1;
                state.finish_refusals
            };
            if refusals <= 2 {
                return (
                    "Not finished: make the reply first (kind reply: a headline and one to three sentences that answer the request), then finish.".into(),
                    true,
                );
            }
        }
        let say = args
            .get("say")
            .and_then(Value::as_str)
            .map(|s| {
                clip(
                    &super::call::plain(s.trim().lines().next().unwrap_or("")),
                    300,
                )
            })
            .filter(|s| !s.is_empty());
        let mut followups: Vec<String> = Vec::new();
        for followup in strings(args.get("followups")) {
            let followup = clip(
                followup.trim().lines().next().unwrap_or(""),
                MAX_WORK_FOLLOWUP_BYTES,
            );
            if !followup.is_empty()
                && !followups.contains(&followup)
                && followups.len() < MAX_WORK_FOLLOWUPS
            {
                followups.push(followup);
            }
        }
        let title = match self.title(args.get("title")) {
            Ok(title) => title,
            Err(fault) => return (fault, true),
        };
        self.run.activity(WorkActivityV1::Finishing);
        let mut step = self.run.step(
            WorkStepKindV1::Finish { followups, title },
            WorkStepStatus::Succeeded,
            None,
        );
        step.note = say;
        match self.run.begin(step, vec![]).await {
            Ok(_) => ("Finished".into(), false),
            Err(_) => ("The finish could not be recorded".into(), true),
        }
    }

    /// The work's name from finish's `title`, on a run that names it. A
    /// title outside the rule is refused once with the rule, then left out.
    fn title(&self, value: Option<&Value>) -> Result<Option<String>, String> {
        if !self.name_work {
            return Ok(None);
        }
        let title = value
            .and_then(Value::as_str)
            .map(|raw| {
                super::call::plain(raw.trim())
                    .trim_end_matches(['.', '!', '?'])
                    .trim()
                    .to_owned()
            })
            .unwrap_or_default();
        if validate_work_title(&title).is_ok() {
            return Ok(Some(title));
        }
        let mut state = self.state();
        if state.title_refused {
            return Ok(None);
        }
        state.title_refused = true;
        if title.is_empty() {
            return Err("Not finished: this is the work's first request, so finish names the work with title, a noun phrase of at most five words such as Compiler learning plan".into());
        }
        Err(format!(
            "Not finished: title has {} words and {} characters; the limit is {MAX_WORK_TITLE_WORDS} words and {MAX_WORK_TITLE_CHARS} characters, one line: a noun phrase such as Compiler learning plan",
            title.split_whitespace().count(),
            title.chars().count()
        ))
    }

    /// Messages the person sent while the run worked, once each.
    async fn steers(&self, messages: &mut Vec<WorkModelMessage>) {
        let Ok(execution) = self.run.execution().await else {
            return;
        };
        let mut said = Vec::new();
        {
            let mut state = self.state();
            for step in &execution.steps {
                if let WorkStepKindV1::Steer { text } = &step.kind {
                    if state.steers.insert(step.id) {
                        said.push(text.clone());
                    }
                }
            }
        }
        for text in said {
            self.run.allow_links_in(&text);
            self.run.trust_sites_in(&text);
            messages.push(WorkModelMessage::User(vec![WorkModelPart::Text(format!(
                "The person adds, while you work: {text}"
            ))]));
        }
    }

    /// Below the floor, the person decides whether the run keeps going with
    /// the same budget again. `Some` ends the run.
    async fn budget(&self) -> Result<Option<WorkAttemptStatus>, WorkError> {
        let left = self.run.remaining();
        if left.cost_micro_usd >= FLOOR_COST && left.model_tokens >= FLOOR_TOKENS {
            return Ok(None);
        }
        let limits = self.run.limits();
        let grown = WorkExecutionLimits {
            model_tokens: limits
                .model_tokens
                .saturating_add(self.base_limits.model_tokens)
                .min(1_000_000),
            cost_micro_usd: limits
                .cost_micro_usd
                .saturating_add(self.base_limits.cost_micro_usd)
                .min(10_000_000),
            operations: limits
                .operations
                .saturating_add(self.base_limits.operations)
                .min(1024),
            ..limits
        };
        let room = grown
            .cost_micro_usd
            .saturating_sub(self.run.used().cost_micro_usd)
            >= FLOOR_COST
            && grown
                .model_tokens
                .saturating_sub(self.run.used().model_tokens)
                >= FLOOR_TOKENS;
        if !room {
            self.run.report(super::WorkLeadDiagnostic::BudgetSpent);
            return self.close(Stall::Budget).await.map(Some);
        }
        let spent = f64::from(self.run.used().cost_micro_usd) / 1_000_000.0;
        let answer = self
            .run
            .ask(
                WorkAskPurposeV1::Budget,
                format!("Used ${spent:.2}. Keep going?"),
                vec![KEEP_GOING.into(), "Stop".into()],
                None,
            )
            .await?;
        let keep = answer
            .as_deref()
            .is_some_and(|a| a.trim().eq_ignore_ascii_case(KEEP_GOING));
        self.run
            .report(super::WorkLeadDiagnostic::KeepGoing { granted: keep });
        match answer {
            None => Ok(Some(WorkAttemptStatus::Cancelled)),
            Some(_) if !keep => self.close(Stall::Budget).await.map(Some),
            Some(_) => {
                self.run.probe.extend_limits(grown).await?;
                self.run.set_limits(grown);
                Ok(None)
            }
        }
    }

    /// A settled model turn, carrying what it said.
    pub(crate) async fn turn_step(
        &self,
        part: Option<WorkPartId>,
        usage: WorkUsage,
        note: Option<String>,
    ) {
        let mut step = self
            .run
            .step(WorkStepKindV1::Turn, WorkStepStatus::Succeeded, part);
        step.usage = Some(usage);
        step.note = note;
        let _ = self.run.begin(step, vec![]).await;
    }
    pub(crate) async fn record_turn(
        &self,
        part: Option<WorkPartId>,
        status: WorkStepStatus,
        usage: Option<WorkUsage>,
        failure: CallFailure,
    ) {
        let mut step = self.run.step(WorkStepKindV1::Turn, status, part);
        step.usage = Some(usage.unwrap_or_default());
        step.note = Some(
            match failure {
                CallFailure::Refused(WorkModelError::MissingKey) => {
                    "No key is set for the chosen model; add one in Settings → AI"
                }
                CallFailure::Refused(WorkModelError::Unauthorized) => {
                    "The model provider refused the key; check it in Settings → AI"
                }
                CallFailure::Refused(WorkModelError::RateLimited { .. }) => {
                    "The model provider is limiting requests right now"
                }
                CallFailure::Refused(WorkModelError::Overloaded) => {
                    "The model provider is overloaded"
                }
                CallFailure::Refused(WorkModelError::ContextTooLong) => {
                    "The work has grown too large for one turn"
                }
                CallFailure::Refused(WorkModelError::Network) => "The model could not be reached",
                CallFailure::Refused(WorkModelError::OverBudget) => {
                    "The model provider's account is out of credit; add credit there or choose another model in Settings → AI"
                }
                _ => "The model's turn could not be used",
            }
            .into(),
        );
        let _ = self.run.begin(step, vec![]).await;
    }
    /// Ends a run that could not reach its own finish as a result: what it
    /// placed stands, a reply says in plain words what is there, what each
    /// part that could not do its job needs and why the run stopped, and
    /// the run finishes. A run never dies with nothing to show.
    async fn close(&self, stall: Stall) -> Result<WorkAttemptStatus, WorkError> {
        self.run.report(super::WorkLeadDiagnostic::Closed { stall });
        let projection = self.run.probe.runtime_projection().await.ok();
        let execution = self.run.probe.execution();
        let placed: Vec<String> = projection
            .as_ref()
            .map(|projection| objects::canvas(projection, execution))
            .unwrap_or_default()
            .into_iter()
            .filter(|o| o.in_this_run && o.current && o.artifact.data.kind_name() != "reply")
            .map(|o| o.artifact.title)
            .collect();
        let parts = projection
            .iter()
            .flat_map(|p| p.executions.iter())
            .find(|e| e.id == execution)
            .map(|e| e.parts.clone())
            .unwrap_or_default();
        if self.state().reply.is_none() {
            let (headline, text) = closing(&placed, &parts, stall);
            let data = zephium_core::work::artifact::WorkArtifactDataV1::Reply {
                headline,
                text,
                figures: vec![],
                points: vec![],
            };
            if data.lead_fault(0).is_none() {
                let proposed = objects::Proposed {
                    title: "Where this stands".into(),
                    data,
                    evidence: vec![],
                    left_out: vec![],
                };
                if let Ok(id) = objects::publish(self.run, proposed, None, None).await {
                    self.state().reply = Some(id);
                }
            }
        }
        self.run.activity(WorkActivityV1::Finishing);
        let mut step = self.run.step(
            WorkStepKindV1::Finish {
                followups: vec![],
                title: None,
            },
            WorkStepStatus::Succeeded,
            None,
        );
        step.note = Some(
            if placed.is_empty() {
                "I couldn't finish this; the reply says why."
            } else {
                "I stopped here; what I found is on the canvas."
            }
            .into(),
        );
        self.run.begin(step, vec![]).await?;
        Ok(WorkAttemptStatus::Succeeded)
    }
}

/// Why a run closed without its own finish, as a closed fact.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Stall {
    /// Its turns stopped moving the work.
    NoProgress,
    /// It reached its budget, or the person chose to stop there.
    Budget,
    /// It took every turn one run holds.
    Turns,
    /// The model stopped answering.
    Model,
}

/// The reply a closed run leaves: what it placed, what each part that could
/// not do its job needs, and why it stopped, in the person's words.
fn closing(
    placed: &[String],
    parts: &[zephium_core::work::parts::WorkPartFactV1],
    stall: Stall,
) -> (String, String) {
    let mut sentences: Vec<String> = Vec::new();
    if !placed.is_empty() {
        let names: Vec<&str> = placed.iter().take(3).map(String::as_str).collect();
        let listed = match names.as_slice() {
            [one] => (*one).to_owned(),
            [first @ .., last] => format!("{} and {last}", first.join(", ")),
            [] => String::new(),
        };
        sentences.push(format!("I placed {listed}."));
    }
    let blocked: Vec<String> = parts
        .iter()
        .filter(|p| {
            p.need.is_some()
                || matches!(
                    p.state,
                    zephium_core::work::parts::WorkPartStateV1::Failed
                        | zephium_core::work::parts::WorkPartStateV1::Stopped
                )
        })
        .take(3)
        .map(part_words)
        .collect();
    let any_blocked = !blocked.is_empty();
    for words in blocked {
        sentences.push(format!("{words}."));
    }
    sentences.push(
        match stall {
            Stall::Budget => "I reached this run's budget before I could finish.",
            Stall::Model => "The model stopped answering before I could finish.",
            Stall::NoProgress | Stall::Turns => "I stopped before I could put the rest together.",
        }
        .into(),
    );
    sentences.push(
        if any_blocked {
            "Each part's row has its fix."
        } else {
            "Ask me to continue and I'll pick up from here."
        }
        .into(),
    );
    let mut text = String::new();
    for sentence in sentences {
        let next = if text.is_empty() {
            sentence
        } else {
            format!("{text} {sentence}")
        };
        if next.chars().count() > zephium_core::work::objects::limit::REPLY_TEXT {
            break;
        }
        text = next;
    }
    let headline = if placed.is_empty() {
        "I couldn't finish this"
    } else {
        "Here is what I found so far"
    };
    (headline.into(), text)
}

/// A part that could not do its job, and why, in the person's words.
fn part_words(part: &zephium_core::work::parts::WorkPartFactV1) -> String {
    use zephium_core::work::parts::{WorkPartNeedV1 as Need, WorkPartReasonV1 as Why};
    let title = &part.title;
    match &part.need {
        Some(Need::SignIn { host }) => format!("{title} needs you to sign in to {host}"),
        Some(Need::AllowSite { host }) => format!("{title} needs your OK to work on {host}"),
        Some(Need::AllowFolder { path }) => format!(
            "{title} needs your OK to read {}",
            path.rsplit('/').find(|n| !n.is_empty()).unwrap_or(path)
        ),
        Some(Need::UseConnection { connection, .. }) => {
            format!("{title} can go through your {connection} connection instead of the website")
        }
        Some(Need::Connect { connection }) => {
            format!("{title} can read {connection} directly once you connect it in Settings")
        }
        Some(Need::Retry { host, reason }) => {
            let site = host.as_deref().unwrap_or("the site");
            match reason {
                Some(Why::CouldntRead) => format!("{title} couldn't read {site}"),
                Some(Why::SignedOut) => format!("{site} showed {title} its signed-out view"),
                Some(Why::BlockedByCheck) => format!("{site} asked {title} for a human check"),
                Some(Why::NotFound) => format!("{title} found nothing that matched on {site}"),
                Some(Why::SiteError) => format!("{site} failed on its side for {title}"),
                Some(Why::NoAnswer) => format!("{site} didn't answer {title} in time"),
                None => format!("{title} didn't finish"),
            }
        }
        None => format!("{title} didn't finish"),
    }
}

/// What a proposed object is checked against: its sources, and the parts
/// of this request that could not do their job.
fn honesty(
    projection: &WorkRuntimeProjection,
    execution: WorkExecutionId,
    proposed: &objects::Proposed,
) -> objects::Honesty {
    let mut failed = Vec::new();
    if let Some(run) = projection.executions.iter().find(|e| e.id == execution) {
        for part in run.parts.iter().filter(|p| {
            p.need.is_some()
                || matches!(
                    p.state,
                    zephium_core::work::parts::WorkPartStateV1::Failed
                        | zephium_core::work::parts::WorkPartStateV1::Stopped
                )
        }) {
            failed.push(part.title.to_lowercase());
            if let Some(service) = &part.service {
                if let Some(host) = &service.host {
                    let host = host.trim_start_matches("www.").trim_start_matches("app.");
                    if let Some(name) = host.split('.').next() {
                        failed.push(name.to_lowercase());
                    }
                }
                if let Some(connection) = &service.connection {
                    failed.push(connection.to_lowercase());
                }
            }
        }
    }
    failed.sort();
    failed.dedup();
    objects::Honesty {
        sourced: !proposed.evidence.is_empty(),
        failed,
    }
}

/// The same call's arguments aimed at an object already on the canvas.
fn with_id(args: &Value, id: WorkArtifactId) -> Value {
    let mut args = args.clone();
    if let Value::Object(map) = &mut args {
        map.insert("id".into(), Value::String(id.to_string()));
    }
    args
}

/// A placed object's answer, with what a second try left out.
fn placed_words(head: &str, left_out: &[String]) -> String {
    if left_out.is_empty() {
        head.to_owned()
    } else {
        format!("{head}. Left out to place it: {}", left_out.join("; "))
    }
}

fn publish_fault(error: WorkError) -> String {
    match error {
        WorkError::Capacity => {
            "The work has no room left for more objects; revise existing ones".into()
        }
        WorkError::Conflict => {
            "That object was already revised; read the canvas and revise its newest version".into()
        }
        _ => "The object could not be placed".into(),
    }
}

fn strings(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .take(64)
                .collect()
        })
        .unwrap_or_default()
}

/// "trip-planning" → "Trip planning".
pub(crate) fn skill_label(name: &str) -> String {
    let words = name.replace('-', " ");
    let mut chars = words.chars();
    match chars.next() {
        Some(first) => clip(
            &(first.to_uppercase().collect::<String>() + chars.as_str()),
            40,
        ),
        None => String::new(),
    }
}

/// What a turn sends, by section, in characters: the stable prompt, the tool
/// definitions, the request's context and the conversation since.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Sections {
    pub prompt: u32,
    pub tools: u32,
    pub context: u32,
    pub conversation: u32,
}
impl Sections {
    pub(crate) fn of(
        system: &[WorkModelSystemBlock],
        tools: &[WorkModelTool],
        messages: &[WorkModelMessage],
    ) -> Self {
        let chars = |n: usize| u32::try_from(n).unwrap_or(u32::MAX);
        Self {
            prompt: chars(system.iter().map(|b| b.text.len()).sum()),
            tools: chars(
                tools
                    .iter()
                    .map(|t| t.name.len() + t.description.len() + t.schema.to_string().len())
                    .sum(),
            ),
            context: chars(messages.first().map(size).unwrap_or(0)),
            conversation: chars(messages.iter().skip(1).map(size).sum()),
        }
    }
}

fn size(message: &WorkModelMessage) -> usize {
    match message {
        WorkModelMessage::User(parts) | WorkModelMessage::Assistant(parts) => parts
            .iter()
            .map(|p| match p {
                WorkModelPart::Text(t) => t.len(),
                WorkModelPart::ToolCall(c) => c.arguments.to_string().len(),
                _ => 256,
            })
            .sum::<usize>(),
        WorkModelMessage::ToolResults(results) => {
            results.iter().map(|r| r.content.len()).sum::<usize>()
        }
    }
}

/// A tool result the model has used and moved past: large ones give way to
/// a stub that keeps their source keys.
const USED_RESULT_CHARS: usize = 1_200;
/// Used results are dropped together once this much has gathered, so the
/// cached prefix breaks once per batch rather than every turn.
const USED_BATCH_CHARS: usize = 12_000;
/// Tool results the model still works from: the newest ones stay whole.
const FRESH_RESULTS: usize = 2;
const STUB: &str = "[Used; dropped to keep the conversation small.";

/// Keeps the conversation small without moving the cached prefix every
/// turn: large tool results the model has moved past are replaced together
/// by a stub with their source keys, and past the hard budget the older
/// results are shortened too. The canvas keeps everything.
fn compact(messages: &mut [WorkModelMessage]) {
    let results: Vec<usize> = messages
        .iter()
        .enumerate()
        .filter(|(_, m)| matches!(m, WorkModelMessage::ToolResults(_)))
        .map(|(i, _)| i)
        .collect();
    let used = &results[..results.len().saturating_sub(FRESH_RESULTS)];
    let droppable = |result: &WorkModelToolResult| {
        result.content.len() > USED_RESULT_CHARS && !result.content.starts_with(STUB)
    };
    let waiting: usize = used
        .iter()
        .filter_map(|i| match &messages[*i] {
            WorkModelMessage::ToolResults(results) => Some(results),
            _ => None,
        })
        .flatten()
        .filter(|r| droppable(r))
        .map(|r| r.content.len())
        .sum();
    if waiting >= USED_BATCH_CHARS {
        for index in used {
            if let WorkModelMessage::ToolResults(results) = &mut messages[*index] {
                for result in results.iter_mut().filter(|r| droppable(r)) {
                    result.content = stub(&result.content);
                }
            }
        }
    }
    let mut total: usize = messages.iter().map(size).sum();
    let keep = messages.len().saturating_sub(6);
    for message in messages[..keep].iter_mut() {
        if total <= CONVERSATION_CHARS {
            break;
        }
        if let WorkModelMessage::ToolResults(results) = message {
            for result in results.iter_mut() {
                if result.content.len() > 400 {
                    total -= result.content.len() - 400;
                    result.content = format!(
                        "{} [shortened; read_canvas or search again if needed]",
                        clip(&result.content, 380)
                    );
                }
            }
        }
    }
}

/// A used result's stub: its first line and the keys of its sources.
fn stub(content: &str) -> String {
    let first = clip(content.lines().next().unwrap_or(""), 160);
    let keys: Vec<String> = content
        .lines()
        .filter(|line| line.starts_with("[s"))
        .filter_map(|line| line.split(']').next().map(|key| format!("{key}]")))
        .take(12)
        .collect();
    let mut out = format!("{STUB} It began: {first}");
    if !keys.is_empty() {
        out.push_str(&format!(" Its sources: {}.", keys.join(" ")));
    }
    out.push_str(" Search, read or read_canvas again if you need it.]");
    out
}

/// Polls every future each wake and returns once all have settled.
async fn join_all<T>(mut futures: Vec<Pin<Box<dyn Future<Output = T> + Send + '_>>>) -> Vec<T> {
    let mut results: Vec<Option<T>> = futures.iter().map(|_| None).collect();
    std::future::poll_fn(|cx| {
        let mut pending = false;
        for (index, future) in futures.iter_mut().enumerate() {
            if results[index].is_some() {
                continue;
            }
            match future.as_mut().poll(cx) {
                std::task::Poll::Ready(value) => results[index] = Some(value),
                std::task::Poll::Pending => pending = true,
            }
        }
        if pending {
            std::task::Poll::Pending
        } else {
            std::task::Poll::Ready(())
        }
    })
    .await;
    results.into_iter().flatten().collect()
}
