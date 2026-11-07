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
const RAN_LONG: &str = "Your previous turn hit the output limit. Do not repeat that response. Reuse the evidence and objects already available. You have 2048 output tokens for a compact recovery: at most three short reply sentences, fewer optional items, and details under 120 characters. Preserve required facts and explicitly requested counts by shortening details first. Keep any reply already published; never recreate it. If the work is complete, create the reply first if missing and call finish in this same turn; omit optional objects instead of delaying completion. If required work remains, call only its next necessary tools.";
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
    reply_input: Option<Value>,
    reply_sources: Vec<String>,
    finish_refusals: u8,
    final_records: Option<Value>,
    title_refused: bool,
    /// Objects refused in this run, by what they are, and how often.
    refused: std::collections::BTreeMap<String, u8>,
    skills: Vec<String>,
    steers: BTreeSet<WorkStepId>,
    searches: usize,
    reads: usize,
    /// The browser parts this run started, by the site each works on.
    sites: Vec<parts::SitePart>,
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

struct EarlyReply {
    call: WorkModelToolCall,
    answer: (String, bool),
}

fn early_reply_args(tool: &WorkModelToolCall) -> Option<Value> {
    if tool.name == "create"
        && tool.arguments.get("kind").and_then(Value::as_str) == Some("reply")
        && tool.arguments.get("part").is_none_or(Value::is_null)
    {
        Some(tool.arguments.clone())
    } else if tool.name == "finish" {
        finish_reply_args(&tool.arguments)
    } else {
        None
    }
}

fn finish_reply_args(args: &Value) -> Option<Value> {
    args.get("reply").filter(|reply| reply.is_object()).map(|reply| serde_json::json!({
        "kind":"reply", "title":args.get("title").and_then(Value::as_str).unwrap_or("Answer"),
        "data":reply, "sources":args.get("reply_sources").cloned().unwrap_or_else(|| serde_json::json!([]))
    }))
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
    /// Whether a browser part may start on `site` now, given the parts this
    /// run already ran there; a part that may is counted as running there.
    pub(crate) fn claim_site(&self, site: &str, title: &str) -> parts::SiteTurn {
        let mut state = self.state();
        let turn = parts::site_turn(&state.sites, site);
        if !matches!(turn, parts::SiteTurn::Refuse(_)) {
            state.sites.push(parts::SitePart {
                site: site.to_owned(),
                title: title.to_owned(),
                ended: None,
            });
        }
        turn
    }
    /// How the part that claimed `site` under `title` ended.
    pub(crate) fn end_site(&self, site: &str, title: &str, ended: parts::SiteEnd) {
        let mut state = self.state();
        if let Some(part) = state
            .sites
            .iter_mut()
            .rev()
            .find(|p| p.site == site && p.title == title && p.ended.is_none())
        {
            part.ended = Some(ended);
        }
    }
    pub(crate) fn release_part(&self) {
        let mut state = self.state();
        state.parts_running = state.parts_running.saturating_sub(1);
    }
    pub(crate) fn parts_running(&self) -> usize {
        self.state().parts_running
    }

    fn system(&self) -> Vec<WorkModelSystemBlock> {
        let mut index =
            String::from("Skills for workflows (plain fact reads use web_fetch directly):\n");
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
        let effort = crate::work_models::lead_effort(self.run.profile, &self.models.lead.entry);
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
            compact(&mut messages, conversation_budget(&self.models.lead.entry));
            let (call_system, call_tools, call_messages) = (&system, &tools, &messages);
            let sizes = Sections::of(call_system, call_tools, call_messages);
            let model = &self.models.lead;
            let request = WorkModelRequest {
                image_fence: None,
                model: model.entry.model.clone(),
                system: call_system.clone(),
                tools: call_tools.clone(),
                messages: call_messages.clone(),
                max_output_tokens: if ran_long {
                    model.entry.max_output.min(2_048)
                } else {
                    model.entry.max_output.clamp(4_096, LEAD_TURN_OUTPUT)
                },
                reasoning: effort.map(|effort| {
                    if ran_long {
                        WorkModelReasoning::Low
                    } else {
                        effort
                    }
                }),
                native_search: false,
                parallel_tools: true,
                tool_choice: WorkModelToolChoice::Auto,
                volatile_tail: 0,
                cache_key: Some(work_model_cache_key(call_system, call_tools)),
            };
            let (called, early_reply) = self.call_with_early_reply(model, request).await;
            let (outcome, usage) = match called {
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
                        self.models.lead.entry.model.provider,
                    )
                    .await;
                    let fatal = matches!(
                        error,
                        WorkModelError::MissingKey
                            | WorkModelError::Unauthorized
                            | WorkModelError::OverBudget
                            | WorkModelError::PlanLimit
                            | WorkModelError::Hosted { .. }
                    );
                    if fatal {
                        return Ok(WorkAttemptStatus::Failed);
                    }
                    if failed_calls >= MAX_FAILED_CALLS {
                        return self.close(Stall::Model(error)).await;
                    }
                    continue;
                }
            };
            failed_calls = 0;
            if outcome.stop == WorkModelStop::MaxTokens {
                self.turn_step(None, usage, None).await;
                if ran_long {
                    return self.close(Stall::OutputLimit).await;
                }
                ran_long = true;
                if let Some(reply) = early_reply {
                    messages.push(WorkModelMessage::Assistant(vec![WorkModelPart::ToolCall(
                        reply.call.clone(),
                    )]));
                    messages.push(WorkModelMessage::ToolResults(vec![WorkModelToolResult {
                        call: reply.call.id,
                        content: reply.answer.0,
                        is_error: reply.answer.1,
                    }]));
                }
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
                tool_kinds: Box::new(std::array::from_fn(|index| {
                    calls.get(index).map(|call| match call.name.as_str() {
                        "web_fetch" => "web_fetch",
                        "web_search" => "web_search",
                        "start_part" => "start_part",
                        "create" => "create",
                        "revise" => "revise",
                        "read_canvas" => "read_canvas",
                        "ask" => "ask",
                        "ask_folder" => "ask_folder",
                        "load_skill" => "load_skill",
                        "finish" => "finish",
                        "continue_work" => "continue_work",
                        "revise_block" => "revise_block",
                        _ => "other",
                    })
                })),
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
                if ran_long {
                    return self.close(Stall::OutputLimit).await;
                }
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
            let (results, progress, finished) =
                match self.execute(&calls, early_reply.as_ref()).await {
                    Ok(done) => done,
                    Err(status) => return Ok(status),
                };
            messages.push(WorkModelMessage::ToolResults(results));
            if finished {
                return Ok(WorkAttemptStatus::Succeeded);
            }
            if ran_long
                && calls.iter().all(|call| {
                    matches!(
                        call.name.as_str(),
                        "create" | "revise" | "read_canvas" | "finish"
                    )
                })
            {
                return self.close(Stall::OutputLimit).await;
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

    async fn call_with_early_reply(
        &self,
        model: &call::LeadModel,
        request: WorkModelRequest,
    ) -> (
        Result<(WorkModelOutcome, WorkUsage), CallFailure>,
        Option<EarlyReply>,
    ) {
        let (send, mut receive) = tokio::sync::mpsc::channel(1);
        let offered = std::sync::atomic::AtomicBool::new(false);
        let completed = |tool: WorkModelToolCall| {
            if early_reply_args(&tool).is_some()
                && !offered.swap(true, std::sync::atomic::Ordering::Relaxed)
            {
                let _ = send.try_send(tool);
            }
        };
        let generation = call::call(
            self.run,
            model,
            request,
            WorkModelRole::Lead,
            Some(&completed),
        );
        tokio::pin!(generation);
        let mut reply = None;
        let mut publication: Option<Pin<Box<dyn Future<Output = EarlyReply> + Send + '_>>> = None;
        let mut outcome = None;
        loop {
            tokio::select! {
                result = &mut generation, if outcome.is_none() => { outcome = Some(result); }
                Some(tool) = receive.recv(), if reply.is_none() && publication.is_none() && outcome.is_none() => {
                    publication = Some(Box::pin(async move {
                        // Only a complete reply uses this path, through the same
                        // schema, source, honesty and durable publication checks.
                        let answer = self.publish_early_reply(&tool).await;
                        EarlyReply { call: tool, answer }
                    }));
                }
                published = async { publication.as_mut().expect("guarded publication").await }, if publication.is_some() => {
                    if !published.answer.1 {
                        self.run.report(super::WorkLeadDiagnostic::ReplyPublished { generation_pending: outcome.is_none() });
                    }
                    reply = Some(published);
                    publication = None;
                }
                _ = self.run.probe.wait_cancelled(), if outcome.is_some() => {
                    self.run.cancelled().await;
                    return (Err(CallFailure::Stopped), reply);
                }
            }
            if matches!(outcome, Some(Err(CallFailure::Stopped))) {
                return (Err(CallFailure::Stopped), reply);
            }
            if publication.is_none()
                && reply.is_none()
                && matches!(&outcome, Some(Ok((result, _))) if result.stop == WorkModelStop::MaxTokens)
            {
                if let Ok(tool) = receive.try_recv() {
                    publication = Some(Box::pin(async move {
                        let answer = self.publish_early_reply(&tool).await;
                        EarlyReply { call: tool, answer }
                    }));
                    continue;
                }
            }
            if publication.is_none() {
                if let Some(mut result) = outcome.take() {
                    if let (Some(reply), Ok((finished, _))) = (&reply, &result) {
                        if finished.stop != WorkModelStop::MaxTokens {
                            let calls = call::tool_calls(&finished.assistant);
                            if calls.iter().filter(|call| call.id == reply.call.id).count() != 1
                                || !calls.iter().any(|call| {
                                    call.id == reply.call.id
                                        && call.name == reply.call.name
                                        && call.arguments == reply.call.arguments
                                })
                            {
                                result = Err(CallFailure::Lost);
                            }
                        }
                    }
                    return (result, reply);
                }
            }
        }
    }

    fn check_finish_records(&self, args: &Value) -> Result<(), (String, ObjectRefusal)> {
        let Some(fields) = args.as_object() else {
            return Err((
                "finish arguments must be an object".into(),
                ObjectRefusal::Shape,
            ));
        };
        if fields.keys().any(|key| {
            !matches!(
                key.as_str(),
                "reply"
                    | "reply_sources"
                    | "records"
                    | "layout"
                    | "records_title"
                    | "say"
                    | "followups"
                    | "title"
            )
        }) {
            return Err(("finish accepts only reply, reply_sources, records, layout, records_title, say, followups and title; nothing was published. Select native records by r key, or create other objects separately.".into(), ObjectRefusal::Shape));
        }
        if let Some(keys) = args.get("reply_sources") {
            if args.get("reply").is_none()
                || !keys.as_array().is_some_and(|keys| {
                    keys.len() <= 16
                        && keys
                            .iter()
                            .all(|key| key.as_str().is_some_and(|key| !key.is_empty()))
                })
            {
                return Err(("reply_sources requires a reply and at most 16 source keys; nothing was published".into(), ObjectRefusal::Source));
            }
            let request = finish_reply_args(args)
                .ok_or_else(|| ("reply must be an object".into(), ObjectRefusal::Shape))?;
            objects::propose(
                self.run,
                &[],
                "reply",
                request["title"].as_str().unwrap_or("Answer"),
                request["data"].clone(),
                &strings(Some(keys)),
                &[],
                false,
            )?;
        }
        if args.get("records").is_none()
            && (args.get("layout").is_some() || args.get("records_title").is_some())
        {
            return Err((
                "layout and records_title require records; nothing was published".into(),
                ObjectRefusal::Shape,
            ));
        }
        if let Some(records) = args.get("records") {
            let kind = args
                .get("layout")
                .and_then(Value::as_str)
                .unwrap_or("picks");
            if !matches!(kind, "picks" | "sheet") {
                return Err(("layout must be picks or sheet".into(), ObjectRefusal::Shape));
            }
            let title = args
                .get("records_title")
                .and_then(Value::as_str)
                .unwrap_or("Results");
            objects::propose(
                self.run,
                &[],
                kind,
                title,
                serde_json::json!({"records": records}),
                &[],
                &[],
                false,
            )?;
        }
        Ok(())
    }

    async fn publish_early_reply(&self, tool: &WorkModelToolCall) -> (String, bool) {
        if tool.name == "finish" {
            if let Err((message, reason)) = self.check_finish_records(&tool.arguments) {
                return self.refuse("finish records", reason, message);
            }
            let state = self.state();
            if state.reply.is_some() {
                return if state.reply_input.as_ref() == tool.arguments.get("reply")
                    && state.reply_sources == strings(tool.arguments.get("reply_sources"))
                {
                    ("Reply already published".into(), false)
                } else {
                    ("A different reply is already published; revise it explicitly, then finish without reply.".into(), true)
                };
            }
        }
        let Some(args) = early_reply_args(tool) else {
            return ("no complete reply".into(), true);
        };
        self.create(&args, None, false).await
    }

    /// The tool calls of one turn, run together; `finish` last.
    async fn execute(
        &self,
        calls: &[WorkModelToolCall],
        early_reply: Option<&EarlyReply>,
    ) -> Result<(Vec<WorkModelToolResult>, bool, bool), WorkAttemptStatus> {
        let mut terminal = None;
        let mut answers: Vec<Option<(String, bool)>> = vec![None; calls.len()];
        let mut requests: Vec<(usize, Request)> = Vec::new();
        let mut pending: Vec<Pending<'_>> = Vec::new();
        let mut finish_at = None;
        let mut ran = vec![false; calls.len()];
        for (index, tool_call) in calls.iter().enumerate() {
            if let Some(reply) = early_reply.filter(|reply| reply.call.id == tool_call.id) {
                if tool_call.name == "finish"
                    && reply.call.name == tool_call.name
                    && reply.call.arguments == tool_call.arguments
                    && !reply.answer.1
                {
                    finish_at = Some(index);
                    continue;
                }
                answers[index] = Some(
                    if reply.call.name == tool_call.name
                        && reply.call.arguments == tool_call.arguments
                    {
                        reply.answer.clone()
                    } else {
                        ("The completed reply changed in the final response. Keep the published reply and revise it explicitly if needed.".into(), true)
                    },
                );
                continue;
            }
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
                                "You have searched enough for this request: read the best pages your searches already listed with web_fetch and build the result from them; start a research part only for a subject you have not searched.".into(),
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
                    let args = unquoted_data(args);
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
                    let args = unquoted_data(args);
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
                            let private = super::run::tool_reads_private(&tool_call.name);
                            let outcome = set.call(context, tool_call).await;
                            // Personal, project, computer and connection tools
                            // answer with the person's own data.
                            if private {
                                self.run.mark_private();
                            }
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
        // The reply answers the whole request on top of the result, never
        // from the end of one part's row.
        let part = part.filter(|_| kind != "reply" || helper);
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
            // A second reply is the model updating its answer: revise it.
            if let Some(reply) = reply {
                return self.revise(&with_id(args, reply)).await;
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
                if placed.artifact.data.kind_name() == kind {
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
        } else if let Some(placed) = part.and_then(|part| {
            canvas.iter().find(|o| {
                o.in_this_run
                    && o.artifact.part == Some(part)
                    && o.artifact.data.kind_name() == kind
            })
        }) {
            // The part's row holds one object of a kind: the lead's version
            // of it replaces the part's.
            return self.revise(&with_id(args, placed.artifact.id)).await;
        } else if let Some(held) = (kind == "picks")
            .then(|| {
                let names = self.pick_names(args);
                objects::part_holds(&canvas, &names)
                    .or_else(|| objects::lead_holds(&canvas, &names))
            })
            .flatten()
        {
            // The same things again are an update of the part's picks.
            return self.revise(&with_id(args, held.artifact.id)).await;
        } else if let Some(existing) = objects::duplicate(&canvas, kind, title, part) {
            return self.revise(&with_id(args, existing.artifact.id)).await;
        }
        let mut data = args.get("data").cloned().unwrap_or(Value::Null);
        let sources = strings(args.get("sources"));
        if let Some(records) = self.as_records(kind, &data) {
            data = records;
        }
        let mut proposed =
            match objects::propose(self.run, &canvas, kind, title, data, &sources, &[], lenient) {
                Ok(proposed) => proposed,
                Err((fault, reason)) => return self.refuse(&key, reason, fault),
            };
        if args.pointer("/data/records").is_none() {
            objects::hydrate_record_media(
                &mut proposed,
                &projection.executions,
                self.run.profile,
                self.run.probe.work(),
            );
        }
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
                    self.state().reply_input = args.get("data").cloned();
                    self.state().reply_sources = strings(args.get("sources"));
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

    /// A hand-written pick or row that restates a record this run read: its
    /// values must come from the record, so it is selected by its r key. A
    /// pick that draws a route (a flight) is the lead's own shape.
    /// The names a picks call is about: its items' names, or the names of
    /// the records it selects by key.
    fn pick_names(&self, args: &Value) -> Vec<String> {
        if let Some(records) = args.pointer("/data/records").and_then(Value::as_array) {
            let read = self.run.records.lock().unwrap_or_else(|e| e.into_inner());
            return records
                .iter()
                .filter_map(super::records::Selection::loose)
                .filter_map(|record| read.name_of(&record.source).map(str::to_owned))
                .collect();
        }
        args.pointer("/data/items")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| item.get("name").and_then(Value::as_str))
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Hand-written picks or rows that name only records this run read stand
    /// as those records, so their prices, links and pictures are the page's.
    /// The lead's own words about each one are kept as its note.
    fn as_records(&self, kind: &str, data: &Value) -> Option<Value> {
        if data.get("records").is_some() {
            return None;
        }
        let records = self.run.records.lock().unwrap_or_else(|e| e.into_inner());
        let items = match kind {
            "picks" => data["items"].as_array()?,
            "sheet" => data["rows"].as_array()?,
            _ => return None,
        };
        let chosen = items
            .iter()
            .map(|item| {
                if item.get("route").is_some() {
                    return None;
                }
                let name = if kind == "picks" {
                    item["name"].as_str()
                } else {
                    item["cells"][0].as_str()
                }?;
                let mut record = serde_json::json!({ "source": records.key_of(name)? });
                for (from, to) in [
                    ("why", "annotation"),
                    ("subtitle", "subtitle"),
                    ("facts", "facts"),
                    ("recommended", "recommended"),
                ] {
                    if let Some(value) = item.get(from).filter(|_| record.get(to).is_none()) {
                        record[to] = value.clone();
                    }
                }
                Some(record)
            })
            .collect::<Option<Vec<_>>>()?;
        (!chosen.is_empty()).then(|| serde_json::json!({ "records": chosen }))
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
        let mut proposed = match objects::propose(
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
        if args.pointer("/data/records").is_none() {
            objects::hydrate_record_media(
                &mut proposed,
                &projection.executions,
                self.run.profile,
                self.run.probe.work(),
            );
        }
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
                    self.state().reply_input = args.get("data").cloned();
                    self.state().reply_sources = strings(args.get("sources"));
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
            at: schema_path(&words),
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
        if let Err((message, reason)) = self.check_finish_records(args) {
            return self.refuse("finish records", reason, message);
        }
        if let Some(reply) = args.get("reply") {
            let state = self.state();
            if state.reply.is_some()
                && (state.reply_input.as_ref() != Some(reply)
                    || state.reply_sources != strings(args.get("reply_sources")))
            {
                return ("A different reply is already published; revise it explicitly, then finish without reply.".into(), true);
            }
        }
        let checked_title = if args.get("reply").is_some() || args.get("records").is_some() {
            match self.title(args.get("title")) {
                Ok(title) => Some(title),
                Err(fault) => return (fault, true),
            }
        } else {
            None
        };
        if self.state().reply.is_none() {
            if let Some(request) = finish_reply_args(args) {
                let result = self.create(&request, None, false).await;
                if result.1 {
                    return result;
                }
            }
        }
        if let Some(records) = args.get("records") {
            let layout = args
                .get("layout")
                .and_then(Value::as_str)
                .unwrap_or("picks");
            if !matches!(layout, "picks" | "sheet") {
                return ("layout must be picks or sheet".into(), true);
            }
            let request = serde_json::json!({"kind":layout, "title":args.get("records_title").and_then(Value::as_str).unwrap_or("Results"), "data":{"records":records}});
            let prior = self.state().final_records.clone();
            match prior {
                Some(prior) if prior == request => {},
                Some(_) => return ("The final records were already published. Use revise to change them, then finish without records.".into(), true),
                None => {
                    let result = self.create(&request, None, false).await;
                    if result.1 { return result; }
                    self.state().final_records = Some(request);
                }
            }
        }
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
        let title = match checked_title {
            Some(title) => title,
            None => match self.title(args.get("title")) {
                Ok(title) => title,
                Err(fault) => return (fault, true),
            },
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
        if (self.run.grant.no_usage_charge || left.cost_micro_usd >= FLOOR_COST)
            && left.model_tokens >= FLOOR_TOKENS
        {
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
        let room = (self.run.grant.no_usage_charge
            || grown
                .cost_micro_usd
                .saturating_sub(self.run.used().cost_micro_usd)
                >= FLOOR_COST)
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
                if self.run.grant.no_usage_charge {
                    "This run reached its token limit. Keep going?".into()
                } else {
                    format!("Used ${spent:.2}. Keep going?")
                },
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
        provider: WorkModelProvider,
    ) {
        let mut step = self.run.step(WorkStepKindV1::Turn, status, part);
        step.usage = Some(usage.unwrap_or_default());
        let hosted = provider == WorkModelProvider::Cloud;
        step.model_fault = match failure {
            CallFailure::Refused(error) => model_fault(error, hosted),
            _ => None,
        };
        step.note = Some(
            match failure {
                CallFailure::Refused(WorkModelError::OverBudget) if hosted => {
                    "Zephium AI's included usage is spent; upgrade, buy extra usage or use your own key"
                }
                CallFailure::Refused(WorkModelError::Hosted { refusal, .. }) => match refusal {
                    HostedRefusal::PlanRequired => {
                        "The chosen Zephium AI model needs another plan; see plans in Settings → Account"
                    }
                    HostedRefusal::BillingReview => {
                        "The Zephium account is under billing review; open Settings → Account"
                    }
                    HostedRefusal::OutOfCredits => {
                        "Zephium AI's included usage is spent; upgrade, buy extra usage or use your own key"
                    }
                    HostedRefusal::DailyLimit => {
                        "The daily Zephium AI limit is reached; it resets at 00:00 UTC"
                    }
                    HostedRefusal::FreeUsagePaused => {
                        "Free Zephium AI usage is paused for now; credits and your own key still work"
                    }
                    HostedRefusal::ConfirmationLimited => {
                        "Zephium AI is still confirming earlier requests; wait a moment"
                    }
                    HostedRefusal::Paused => {
                        "Zephium AI is paused as a spending protection; try later or use your own key"
                    }
                    _ => "The model's turn could not be used",
                },
                CallFailure::Refused(WorkModelError::Unauthorized) if hosted => {
                    "The Zephium account's session ended; sign in again in Settings → Account"
                }
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
                CallFailure::Refused(WorkModelError::PlanLimit) => {
                    "One request is larger than the model provider's plan allows per minute; upgrade the plan there or choose another model in Settings → AI"
                }
                CallFailure::Refused(WorkModelError::BadRequest) => {
                    "The model provider refused the request"
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
        let execution = self.run.probe.execution();
        let mut projection = self.run.probe.runtime_projection().await.ok();
        let found_nothing = projection.as_ref().is_some_and(|projection| {
            !objects::canvas(projection, execution)
                .iter()
                .any(|o| o.in_this_run && o.artifact.data.kind_name() != "reply")
        });
        if found_nothing {
            let proposed = {
                let records = self.run.records.lock().unwrap_or_else(|e| e.into_inner());
                records
                    .read_so_far()
                    .and_then(|request| records.present(&request).ok())
            };
            if let Some(proposed) = proposed {
                if objects::publish(self.run, proposed, None, None)
                    .await
                    .is_ok()
                {
                    projection = self.run.probe.runtime_projection().await.ok();
                }
            }
        }
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
        let reply_stood = self.state().reply.is_some();
        if !reply_stood {
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
            if stall == Stall::OutputLimit && reply_stood {
                "The reply is ready; further details could not be completed."
            } else if placed.is_empty() {
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

/// Call arguments whose `data` came as JSON text, as models sometimes send
/// it, with that text read as the object it spells.
fn unquoted_data(args: &Value) -> Value {
    let mut args = args.clone();
    let parsed = args
        .get("data")
        .and_then(Value::as_str)
        .and_then(|text| serde_json::from_str::<Value>(text).ok())
        .filter(Value::is_object);
    if let Some(data) = parsed {
        args["data"] = data;
    }
    // The kind's fields given beside `data` rather than in it are its data.
    if !args.get("data").is_some_and(Value::is_object) {
        if let Some(map) = args.as_object_mut() {
            let fields: serde_json::Map<String, Value> = [
                "items", "records", "steps", "rows", "columns", "headline", "text", "points",
                "figures", "facet", "nodes", "edges", "series", "language", "code", "calendar",
            ]
            .iter()
            .filter_map(|key| map.remove(*key).map(|value| ((*key).to_owned(), value)))
            .collect();
            if !fields.is_empty() {
                map.insert("data".into(), Value::Object(fields));
            }
        }
    }
    args
}

/// The schema path a refusal's words lead with, after an optional `kind: `.
fn schema_path(words: &str) -> Option<super::SchemaPath> {
    let words = words.split_once(": ").map_or(words, |(_, rest)| rest);
    let path = words.split_whitespace().next()?;
    (path.contains(|c: char| c.is_ascii_lowercase())
        && path
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || "_.[]".contains(c)))
    .then(|| super::SchemaPath::new(path))
    .flatten()
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
    /// The model stopped answering, with its last refusal.
    Model(WorkModelError),
    /// The compact recovery could not finish within one further call.
    OutputLimit,
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
            Stall::Model(error) => model_stop(error),
            Stall::OutputLimit => {
                "I could not finish the remaining details within the result's size limit."
            }
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
        Some(Need::Check { host }) => {
            format!("{title} needs you to pass the human check on {host}")
        }
        Some(Need::AllowSite { host }) => format!("{title} needs your OK to work on {host}"),
        Some(Need::ChooseFolder) => format!("{title} needs you to choose a folder"),
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
/// Why the model stopped, as the closing reply says it.
fn model_stop(error: WorkModelError) -> &'static str {
    match error {
        WorkModelError::RateLimited { .. } => {
            "The model provider kept limiting requests, so I stopped."
        }
        WorkModelError::Overloaded => "The model provider was overloaded and did not answer.",
        WorkModelError::Network => "The model could not be reached.",
        WorkModelError::ContextTooLong => {
            "The work grew too large for the model to take in one turn."
        }
        WorkModelError::BadRequest => "The model provider refused the requests I sent it.",
        WorkModelError::PlanLimit => {
            "Each request is larger than the model provider's plan allows per minute."
        }
        _ => "The model stopped answering before I could finish.",
    }
}

/// A refused model call in the words the person acts on.
fn model_fault(error: WorkModelError, hosted: bool) -> Option<WorkModelFaultV1> {
    Some(match error {
        WorkModelError::MissingKey if hosted => WorkModelFaultV1::SignedOut,
        WorkModelError::MissingKey => WorkModelFaultV1::MissingKey,
        WorkModelError::Unauthorized if hosted => WorkModelFaultV1::SignedOut,
        WorkModelError::Unauthorized => WorkModelFaultV1::KeyRefused,
        WorkModelError::OverBudget if hosted => WorkModelFaultV1::HostedUsage,
        WorkModelError::OverBudget => WorkModelFaultV1::ProviderCredit,
        WorkModelError::PlanLimit => WorkModelFaultV1::PlanLimit,
        WorkModelError::Hosted { refusal, .. } => match refusal {
            HostedRefusal::PlanRequired => WorkModelFaultV1::PlanRequired,
            HostedRefusal::BillingReview => WorkModelFaultV1::BillingReview,
            HostedRefusal::OutOfCredits => WorkModelFaultV1::HostedUsage,
            HostedRefusal::DailyLimit => WorkModelFaultV1::DailyLimit,
            HostedRefusal::FreeUsagePaused => WorkModelFaultV1::FreeUsagePaused,
            HostedRefusal::ConfirmationLimited => WorkModelFaultV1::HostedBusy,
            HostedRefusal::Paused => WorkModelFaultV1::HostedPaused,
            HostedRefusal::AlreadySent
            | HostedRefusal::InvalidAudio
            | HostedRefusal::UnsupportedAudio
            | HostedRefusal::AudioTooLong => return None,
        },
        WorkModelError::RateLimited { .. } => WorkModelFaultV1::RateLimited,
        WorkModelError::Overloaded => WorkModelFaultV1::ProviderDown,
        WorkModelError::Network => WorkModelFaultV1::Unreachable,
        WorkModelError::ContextTooLong => WorkModelFaultV1::TooLarge,
        _ => return None,
    })
}

/// How much conversation the model's context can carry beside the prompt,
/// tools and answer: two characters a token keeps it under the window for
/// dense text, and large windows keep the usual ceiling.
pub(crate) fn conversation_budget(entry: &WorkModelEntry) -> usize {
    CONVERSATION_CHARS.min(entry.context_window as usize * 2)
}

pub(crate) fn compact(messages: &mut [WorkModelMessage], budget: usize) {
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
        if total <= budget {
            break;
        }
        if let WorkModelMessage::ToolResults(results) = message {
            for result in results.iter_mut() {
                let shortened = format!(
                    "{} [shortened; read_canvas or search again if needed]",
                    clip(&result.content, 380)
                );
                if shortened.len() < result.content.len() {
                    total = total.saturating_sub(result.content.len()) + shortened.len();
                    result.content = shortened;
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

#[cfg(test)]
mod compact_tests {
    use super::*;
    use zephium_core::work::model::{
        WorkModelProvider, WorkModelRef, WorkModelSupports, WorkModelWire,
    };

    #[test]
    fn data_sent_as_json_text_is_read_as_its_object_and_a_refusal_names_its_path() {
        let args = serde_json::json!({"kind":"picks","data":"{\"records\":[\"r1\"]}"});
        assert_eq!(unquoted_data(&args)["data"]["records"][0], "r1");
        let plain = serde_json::json!({"data":"not json"});
        assert_eq!(unquoted_data(&plain), plain);
        let beside = serde_json::json!({"kind":"picks","title":"Stays","records":["r1"]});
        assert_eq!(unquoted_data(&beside)["data"]["records"][0], "r1");
        assert_eq!(
            format!(
                "{:?}",
                schema_path("plan: steps[3].detail is 300 characters").unwrap()
            ),
            "steps[3].detail"
        );
        assert!(schema_path("The canvas could not be read").is_none());
    }

    fn entry(context_window: u32) -> WorkModelEntry {
        WorkModelEntry {
            id: "compatible/local".into(),
            model: WorkModelRef {
                provider: WorkModelProvider::Compatible,
                wire: WorkModelWire::ChatCompletions,
                model: "local".into(),
            },
            display_name: "Local".into(),
            roles: vec![],
            recommended: false,
            context_window,
            max_output: 4096,
            supports: WorkModelSupports {
                tools: true,
                vision: false,
                prompt_cache: false,
                reasoning: false,
                native_search: false,
            },
            price: None,
        }
    }

    fn turn(index: usize, size: usize) -> [WorkModelMessage; 2] {
        [
            WorkModelMessage::Assistant(vec![WorkModelPart::Text(format!("turn {index}"))]),
            WorkModelMessage::ToolResults(vec![WorkModelToolResult {
                call: format!("call_{index}"),
                content: "x".repeat(size),
                is_error: false,
            }]),
        ]
    }

    #[test]
    fn a_model_stall_says_why_the_provider_refused() {
        let (_, text) = closing(&[], &[], Stall::Model(WorkModelError::ContextTooLong));
        assert!(text.contains("too large"), "{text}");
        let (_, text) = closing(&[], &[], Stall::Model(WorkModelError::BadRequest));
        assert!(text.contains("refused"), "{text}");
        assert_eq!(
            model_fault(WorkModelError::PlanLimit, false),
            Some(WorkModelFaultV1::PlanLimit)
        );
    }

    #[test]
    fn hosted_refusals_name_the_account_not_a_provider_key() {
        let fault = |error, hosted| model_fault(error, hosted);
        assert_eq!(
            fault(WorkModelError::OverBudget, true),
            Some(WorkModelFaultV1::HostedUsage)
        );
        assert_eq!(
            fault(WorkModelError::OverBudget, false),
            Some(WorkModelFaultV1::ProviderCredit)
        );
        assert_eq!(
            fault(WorkModelError::Unauthorized, true),
            Some(WorkModelFaultV1::SignedOut)
        );
        assert_eq!(
            fault(WorkModelError::Unauthorized, false),
            Some(WorkModelFaultV1::KeyRefused)
        );
        assert_eq!(fault(WorkModelError::Cancelled, false), None);
    }

    #[test]
    fn each_hosted_refusal_has_its_own_fault_and_the_rest_have_none() {
        let hosted = |refusal| WorkModelError::Hosted {
            refusal,
            retry_after_ms: None,
        };
        for (refusal, expected) in [
            (
                HostedRefusal::PlanRequired,
                Some(WorkModelFaultV1::PlanRequired),
            ),
            (
                HostedRefusal::BillingReview,
                Some(WorkModelFaultV1::BillingReview),
            ),
            (
                HostedRefusal::OutOfCredits,
                Some(WorkModelFaultV1::HostedUsage),
            ),
            (
                HostedRefusal::DailyLimit,
                Some(WorkModelFaultV1::DailyLimit),
            ),
            (
                HostedRefusal::FreeUsagePaused,
                Some(WorkModelFaultV1::FreeUsagePaused),
            ),
            (
                HostedRefusal::ConfirmationLimited,
                Some(WorkModelFaultV1::HostedBusy),
            ),
            (HostedRefusal::Paused, Some(WorkModelFaultV1::HostedPaused)),
            (HostedRefusal::AlreadySent, None),
            (HostedRefusal::AudioTooLong, None),
        ] {
            assert_eq!(model_fault(hosted(refusal), true), expected, "{refusal:?}");
        }
    }

    #[test]
    fn a_small_context_window_shortens_older_results_sooner() {
        assert_eq!(conversation_budget(&entry(1_000_000)), CONVERSATION_CHARS);
        assert_eq!(conversation_budget(&entry(32_768)), 65_536);
        // Results small enough to stay whole until the conversation outgrows its budget.
        let conversation: Vec<WorkModelMessage> = (0..80).flat_map(|i| turn(i, 1_100)).collect();
        let total = |messages: &[WorkModelMessage]| messages.iter().map(size).sum::<usize>();

        let mut large = conversation.clone();
        compact(&mut large, conversation_budget(&entry(1_000_000)));
        let mut small = conversation;
        compact(&mut small, conversation_budget(&entry(32_768)));
        assert_eq!(
            total(&large),
            total(&(0..80).flat_map(|i| turn(i, 1_100)).collect::<Vec<_>>())
        );
        assert!(total(&small) <= 65_536);
        assert!(total(&small) < total(&large));
        // The newest turns stay whole for the model to act on.
        let Some(WorkModelMessage::ToolResults(last)) = small.last() else {
            panic!("results last");
        };
        assert_eq!(last[0].content.len(), 1_100);
    }
}
