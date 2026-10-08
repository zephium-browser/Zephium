//! Parts: a helper does one purpose of the job with its own prompt, tools
//! and model, places what it found at the end of its row, and returns a
//! digest to the lead. Up to four run at once; their pages share the run's
//! three live agent pages.
use std::future::Future;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;
use zephium_core::work::{artifact::public_host, model::*, parts::*, runtime::*, *};
use zephium_ipc::work::WorkActivityV1;

use super::call::{self, clip, CallFailure};
use super::hands::{self, Hands, Request};
use super::lead::Lead;
use super::objects;
use super::prompt;
use super::tools::{LeadHelper, LeadScope, LeadToolContext, LeadToolSet};
use crate::work_agent::{WorkAgentBrowseRequest, WorkBrowserOutcome};
use crate::work_runtime::WorkAttemptProbe;

/// Parts that run at once; the rest wait for a slot.
pub(crate) const PARALLEL_PARTS: usize = 4;
/// One part's ceiling, inside the run's remaining budget.
const PART_MAX: WorkExecutionLimits = WorkExecutionLimits {
    model_tokens: 450_000,
    cost_micro_usd: 900_000,
    operations: 96,
    timeout_seconds: 1800,
    max_workers: 3,
};
const PART_TURNS: u8 = 10;
/// The least a part starts with when the run has it: one search's
/// reservation and one page task's cost.
const PART_MIN_TOKENS: u32 = zephium_core::work::search::PUBLIC_SEARCH_TOKEN_RESERVATION + 16_384;
const PART_MIN_COST: u32 = 150_000;
/// The note a page step ends with when its page asked to sign in.
const SIGN_IN_NOTE: &str = "The page asked to sign in";
/// Searches one part may run; past them it reports what it has.
const PART_SEARCHES: usize = 4;
/// Searches a part runs before each further one is weighed against its goal.
const PART_FREE_SEARCHES: usize = 2;
/// What of a part's search answers the goal check reads.
const FOUND_CHARS: usize = 6_000;

pub(crate) struct PartSpec {
    pub title: String,
    pub helper: WorkHelperV1,
    pub goal: String,
    pub brief: String,
    pub service: Option<WorkPartServiceV1>,
    pub records: Vec<String>,
    /// What a browser part searches, as typed fields for a site's results page.
    pub search: Option<super::recipes::SiteSearch>,
    /// The person's connection for the service, when they chose its website.
    pub declined: Option<super::route::Offer>,
}

fn now_ms() -> String {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
        .to_string()
}

fn text_arg(args: &Value, key: &str) -> Option<String> {
    args.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

/// A `start_part` call's arguments, admitted or refused with the rule.
pub(crate) fn spec(args: &Value) -> Result<PartSpec, String> {
    let title = text_arg(args, "title").ok_or("title is required")?;
    if title.chars().count() > 24 || title.contains('\n') {
        return Err("title is one line of at most 24 characters: Stay, Flights, Entry".into());
    }
    let helper = match args.get("helper").and_then(Value::as_str) {
        Some("browser") => WorkHelperV1::Browser,
        Some("research") => WorkHelperV1::Research,
        Some("computer") => WorkHelperV1::Computer,
        Some("connection") => WorkHelperV1::Connection,
        _ => return Err("helper is one of browser, research, computer, connection".into()),
    };
    let goal = text_arg(args, "goal").ok_or("goal is required")?;
    if goal.chars().count() > 200 || goal.contains('\n') {
        return Err("goal is one line of at most 200 characters; put details in brief".into());
    }
    let brief = text_arg(args, "brief").unwrap_or_default();
    if brief.chars().count() > 1200 {
        return Err("brief is at most 1200 characters".into());
    }
    let service = text_arg(args, "service").and_then(|service| {
        let host = service
            .trim_start_matches("https://")
            .trim_start_matches("http://")
            .trim_start_matches("www.")
            .split('/')
            .next()
            .unwrap_or_default()
            .to_ascii_lowercase();
        if public_host(&host) {
            Some(WorkPartServiceV1 {
                host: Some(host),
                connection: None,
            })
        } else if service.chars().count() <= 40 && !service.contains('\n') {
            Some(WorkPartServiceV1 {
                host: None,
                connection: Some(service),
            })
        } else {
            None
        }
    });
    let records = args
        .get("records")
        .and_then(Value::as_array)
        .map(|fields| {
            fields
                .iter()
                .filter_map(Value::as_str)
                .map(|f| clip(f.trim(), 40))
                .take(12)
                .collect()
        })
        .unwrap_or_default();
    let search = super::recipes::parse(args.get("search"))?;
    Ok(PartSpec {
        title,
        helper,
        goal,
        brief,
        service,
        records,
        search,
        declined: None,
    })
}

/// How a part ended, for its fact and the lead.
pub(crate) struct PartReport {
    pub state: WorkPartStateV1,
    pub summary: Option<String>,
    pub digest: String,
    pub objects: Vec<WorkArtifactId>,
    /// What the person can do so the part can do its job.
    pub need: Option<WorkPartNeedV1>,
}
impl PartReport {
    fn ended(state: WorkPartStateV1, digest: &str, objects: Vec<WorkArtifactId>) -> Self {
        Self {
            state,
            summary: None,
            digest: digest.into(),
            objects,
            need: None,
        }
    }
}

enum Kit {
    Browser,
    Research,
    Files,
    Registered(Arc<dyn LeadHelper>),
}

impl<'a, B, Fut> Lead<'a, B>
where
    B: FnMut(WorkAttemptProbe, WorkAgentBrowseRequest) -> Fut + Send,
    Fut: Future<Output = Result<WorkBrowserOutcome, WorkError>> + Send,
{
    /// Runs one part to its end and returns what the lead reads.
    pub(crate) async fn part(&self, mut spec: PartSpec) -> (String, bool) {
        if !self.claim_part() {
            return (
                format!("A run holds at most {MAX_WORK_PARTS} parts; finish the work with the parts you have"),
                true,
            );
        }
        self.route(&mut spec).await;
        let mut fact = WorkPartFactV1 {
            id: WorkPartId::generate(),
            title: spec.title.clone(),
            helper: spec.helper,
            service: spec.service.clone(),
            goal: spec.goal.clone(),
            state: WorkPartStateV1::Planned,
            started_ms: None,
            ended_ms: None,
            summary: None,
            need: None,
        };
        if self.run.part(fact.clone()).await.is_err() {
            return ("The part could not be recorded".into(), true);
        }
        let _slot = self.slots.acquire().await.ok();
        fact.state = WorkPartStateV1::Running;
        fact.started_ms = Some(now_ms());
        let _ = self.run.part(fact.clone()).await;
        self.run.activity(WorkActivityV1::Delegating);
        let mut report = self.helper(fact.id, &spec).await;
        self.settle_need(fact.id, &spec, &mut report).await;
        if let Kit::Registered(helper) = self.kit(spec.helper) {
            let context = LeadToolContext {
                run: self.run,
                part: Some(fact.id),
            };
            for (title, data) in helper.objects(context, &report.digest).await {
                match context.publish(&title, data).await {
                    Ok(id) => report.objects.push(id),
                    Err(_) => self.run.report(super::WorkLeadDiagnostic::ObjectRefused {
                        reason: Some(objects::ObjectRefusal::Shape),
                    }),
                }
            }
        }
        self.release_part();
        fact.state = report.state;
        fact.need = report.need.clone();
        fact.ended_ms = Some(now_ms());
        fact.summary = report
            .summary
            .as_deref()
            .map(|s| clip(s.lines().next().unwrap_or(""), 78))
            .filter(|s| !s.trim().is_empty());
        let _ = self.run.part(fact.clone()).await;
        self.run.report(super::WorkLeadDiagnostic::PartEnded {
            helper: spec.helper,
            state: report.state,
            objects: report.objects.len(),
        });
        let mut out = format!(
            "Part {} ({}) {}",
            spec.title,
            fact.id,
            match report.state {
                WorkPartStateV1::Done => "done",
                WorkPartStateV1::Stopped => "stopped",
                _ => "could not do its job",
            }
        );
        if let Some(summary) = &fact.summary {
            out.push_str(&format!(": {summary}"));
        }
        out.push('\n');
        if let Some(need) = &fact.need {
            out.push_str(&format!(
                "It needs {}; its row shows the fix. Build the result only from what was found, make no object, figure, pick or task for what it could not do, and when nothing was found say so in the reply in one sentence.\n",
                need_words(need)
            ));
        } else if report.state != WorkPartStateV1::Done {
            out.push_str("Build the result only from what was found; make no object, figure, pick or task for what it could not do.\n");
        }
        if !report.objects.is_empty() {
            let projection = self.run.probe.runtime_projection().await.ok();
            let canvas = projection
                .as_ref()
                .map(|p| objects::canvas(p, self.run.probe.execution()))
                .unwrap_or_default();
            out.push_str("Objects it placed, which stand in the result as they are (never place another object of the same things):\n");
            out.push_str(&objects::view(
                &canvas
                    .into_iter()
                    .filter(|o| report.objects.contains(&o.artifact.id))
                    .collect::<Vec<_>>(),
            ));
        }
        out.push_str(&report.digest);
        (out, report.state != WorkPartStateV1::Done)
    }

    fn kit(&self, helper: WorkHelperV1) -> Kit {
        if let Some(registered) = self.helpers.iter().find(|h| h.kind() == helper) {
            return Kit::Registered(registered.clone());
        }
        match helper {
            WorkHelperV1::Research => Kit::Research,
            WorkHelperV1::Computer => Kit::Files,
            // Without an installed route, a connection works through its site.
            WorkHelperV1::Browser | WorkHelperV1::Connection => Kit::Browser,
        }
    }

    async fn helper(&self, part: WorkPartId, spec: &PartSpec) -> PartReport {
        let kit = self.kit(spec.helper);
        let (prompt_text, role, max_turns) = match &kit {
            Kit::Browser => (prompt::BROWSER.to_owned(), WorkModelRole::Page, PART_TURNS),
            Kit::Research => (
                prompt::RESEARCH.to_owned(),
                WorkModelRole::Light,
                PART_TURNS,
            ),
            Kit::Files => (
                prompt::COMPUTER.to_owned(),
                WorkModelRole::Page,
                PART_TURNS + 6,
            ),
            Kit::Registered(helper) => (
                helper.prompt().to_owned(),
                helper.role(),
                helper.max_turns(),
            ),
        };
        let model = self.models.for_role(role);
        let share = self.part_share();
        let objective = format!("{}\n\nThe person's request: {}", spec.goal, self.objective);
        let hands = Hands::new(
            self.run,
            self.attempt,
            self.search,
            self.browser,
            Some(part),
            share,
            objective,
        )
        .await
        .personal(
            spec.helper == WorkHelperV1::Connection
                || spec.declined.is_some()
                || super::route::brand(spec.service.as_ref(), &spec.title)
                    .is_some_and(|brand| super::route::site(&brand).is_some()),
        );
        let mut tools: Vec<WorkModelTool> = match &kit {
            Kit::Browser => vec![
                prompt::browse_tool(),
                prompt::read_tool(),
                prompt::search_tool(),
            ],
            Kit::Research => vec![prompt::search_tool(), prompt::read_tool()],
            Kit::Files => prompt::file_tools(),
            Kit::Registered(helper) => helper.tools().tools(
                LeadScope::Helper(spec.helper),
                &super::tools::LeadRunView {
                    run: self.run,
                    service: spec.service.as_ref(),
                },
            ),
        };
        // Tool sets beside the lead's own offer tools to helpers too.
        let view = super::tools::LeadRunView {
            run: self.run,
            service: spec.service.as_ref(),
        };
        let mut sets: Vec<Arc<dyn LeadToolSet>> = Vec::new();
        if let Kit::Registered(helper) = &kit {
            sets.push(helper.tools());
        }
        for set in &self.extra {
            let offered = set.tools(LeadScope::Helper(spec.helper), &view);
            if !offered.is_empty() {
                tools.extend(
                    offered
                        .into_iter()
                        .filter(|t| !tools.iter().any(|known| known.name == t.name))
                        .collect::<Vec<_>>(),
                );
                sets.push(set.clone());
            }
        }
        tools.extend(prompt::helper_tools());
        let system = vec![
            WorkModelSystemBlock {
                text: prompt::HELPER.to_owned(),
                cache: false,
            },
            WorkModelSystemBlock {
                text: prompt_text,
                cache: true,
            },
        ];
        let mut brief = format!(
            "Part: {} ({})\nGoal: {}\n",
            spec.title,
            match spec.helper {
                WorkHelperV1::Browser => "browser",
                WorkHelperV1::Research => "research",
                WorkHelperV1::Computer => "computer",
                WorkHelperV1::Connection => "connection",
            },
            spec.goal
        );
        if !spec.brief.is_empty() {
            brief.push_str(&format!("Details: {}\n", spec.brief));
        }
        if let Some(service) = &spec.service {
            if let Some(host) = &service.host {
                brief.push_str(&format!("Site: {host}\n"));
            }
            if let Some(connection) = &service.connection {
                brief.push_str(&format!("Service: {connection}\n"));
            }
        }
        if !spec.records.is_empty() {
            brief.push_str(&format!(
                "Return for each thing: {}\n",
                spec.records.join(", ")
            ));
        }
        let given = match kit {
            Kit::Browser => given_pages(&self.objective, spec),
            _ => Vec::new(),
        };
        if !given.is_empty() {
            let collection = records_collection(&spec.title, &spec.records);
            let requests = given
                .iter()
                .enumerate()
                .map(|(index, url)| Request {
                    call: format!("given-{index}"),
                    kind: WorkStepKindV1::Read {
                        url: url.clone(),
                        collection: collection.clone(),
                        goal: None,
                    },
                    mine: false,
                    view: false,
                })
                .collect();
            match hands.run(requests).await {
                Ok(done) => {
                    for ((_, content, error), url) in done.into_iter().zip(&given) {
                        brief.push_str(&format!(
                            "The page the person gave, already read as given ({url}){}:\n{}\n",
                            if error { ", which failed" } else { "" },
                            clip(&content, 6_000)
                        ));
                    }
                    brief.push_str("Build from what it gave. Read or browse other pages only for what it lacks, and never search the site for what it already shows.\n");
                }
                Err(_) => {
                    return PartReport::ended(
                        WorkPartStateV1::Stopped,
                        "Stopped while its pages ran.",
                        Vec::new(),
                    )
                }
            }
        }
        if let (Kit::Browser, Some(search), true) = (&kit, &spec.search, given.is_empty()) {
            let pages = super::recipes::pages(
                search,
                spec.service
                    .as_ref()
                    .and_then(|service| service.host.as_deref()),
            );
            if !pages.is_empty() {
                brief.push_str("Results pages with the search already in them, the part's own site first; browse the first as start, its goal to read the results shown as records (open an item only for a field the list lacks). If it will not load, use the next one before any form:\n");
                for (name, url) in pages {
                    // Built by Rust from a fixed template on a known store,
                    // not an address the model wrote.
                    self.run.allow_url(&url);
                    brief.push_str(&format!("- {name}: {url}\n"));
                }
            }
        }
        let folders = self.run.folders();
        if matches!(kit, Kit::Files | Kit::Registered(_)) {
            if !folders.current.is_empty() {
                brief.push_str(&format!("Folders: {}\n", folders.current.join(", ")));
            }
            if !folders.available.is_empty() {
                brief.push_str(&format!(
                    "Also readable, only if the goal is about them: {}\n",
                    folders.available.join(", ")
                ));
            }
        }
        brief.push_str(&format!(
            "The person's request: {}\nNow: {}\nYou have at most {max_turns} turns.",
            self.objective,
            prompt::now_line()
        ));
        let mut messages = vec![WorkModelMessage::User(vec![WorkModelPart::Text(brief)])];
        let mut placed: Vec<WorkArtifactId> = Vec::new();
        let mut last_text = String::new();
        let mut spent = WorkUsage::default();
        let mut warned = false;
        let mut idle = 0u8;
        let mut searches = 0usize;
        let mut found = String::new();
        for turn in 0..max_turns {
            let over = {
                let pages = hands.used().await;
                pages.cost_micro_usd.saturating_add(spent.cost_micro_usd) >= share.cost_micro_usd
                    || pages.model_tokens.saturating_add(spent.model_tokens) >= share.model_tokens
            };
            let last = turn + 1 == max_turns;
            if (over || last) && !warned {
                warned = true;
                messages.push(WorkModelMessage::User(vec![WorkModelPart::Text(
                    "This part's budget is spent: place what you found, then call finish now."
                        .into(),
                )]));
            } else if over {
                break;
            }
            let request = WorkModelRequest {
                model: model.entry.model.clone(),
                system: system.clone(),
                tools: tools.clone(),
                messages: messages.clone(),
                max_output_tokens: 8_000,
                reasoning: model
                    .entry
                    .supports
                    .reasoning
                    .then_some(WorkModelReasoning::Low),
                native_search: false,
                parallel_tools: true,
            };
            let (outcome, usage) = match call::call(self.run, model, request).await {
                Ok(done) => done,
                Err(CallFailure::Stopped) => {
                    return PartReport::ended(
                        WorkPartStateV1::Stopped,
                        "Stopped before it finished.",
                        placed,
                    )
                }
                Err(failure) => {
                    self.record_turn(Some(part), WorkStepStatus::Failed, None, failure)
                        .await;
                    let mut report = PartReport::ended(
                        WorkPartStateV1::Failed,
                        "The helper's model could not be reached.",
                        placed,
                    );
                    report.need = Some(WorkPartNeedV1::Retry {
                        host: None,
                        reason: None,
                    });
                    return report;
                }
            };
            spent = add(spent, usage);
            let text = call::text(&outcome.assistant);
            if !text.is_empty() {
                last_text = text.clone();
            }
            self.turn_step(Some(part), usage, call::say_line(&text))
                .await;
            let calls = call::tool_calls(&outcome.assistant);
            messages.push(WorkModelMessage::Assistant(outcome.assistant));
            if calls.is_empty() {
                idle += 1;
                if idle >= 2 {
                    break;
                }
                messages.push(WorkModelMessage::User(vec![WorkModelPart::Text(
                    "Use your tools to do the part, or call finish.".into(),
                )]));
                continue;
            }
            idle = 0;
            let mut results: Vec<Option<WorkModelToolResult>> = vec![None; calls.len()];
            let mut requests = Vec::new();
            let mut finished: Option<(String, String, bool, Option<WorkPartNeedV1>)> = None;
            let mut enough: Option<bool> = None;
            for (index, tool_call) in calls.iter().enumerate() {
                let answer = |content: String, is_error: bool| WorkModelToolResult {
                    call: tool_call.id.clone(),
                    content,
                    is_error,
                };
                match tool_call.name.as_str() {
                    "web_search" | "read" | "browse" | "list" | "read_file" | "search_files"
                    | "write_file" | "edit_file" | "run_command"
                        if !matches!(kit, Kit::Registered(_)) =>
                    {
                        match step_request(&tool_call.name, &tool_call.arguments, self.run) {
                            Ok(WorkStepKindV1::Search { .. }) if searches >= PART_SEARCHES => {
                                results[index] = Some(answer(
                                    "This part has used its searches: place what you found and finish, saying what is missing.".into(),
                                    true,
                                ))
                            }
                            Ok(WorkStepKindV1::Search { .. })
                                if searches >= PART_FREE_SEARCHES
                                    && *enough.get_or_insert(
                                        self.found_enough(&spec.goal, &found).await,
                                    ) =>
                            {
                                results[index] = Some(answer(
                                    "What your searches found already answers the goal: place what you found and finish.".into(),
                                    true,
                                ))
                            }
                            Ok(mut kind) => {
                                // Only a daily app has views: a store or a listings
                                // site read "as a view" would lose its page planner.
                                let view = reads_app(&kind)
                                    || (tool_call.arguments.get("view").and_then(Value::as_bool)
                                        == Some(true)
                                        && daily_app(&kind));
                                if let (true, WorkStepKindV1::Read { goal: Some(_), collection, .. }) = (view, &mut kind) {
                                    *collection = None;
                                }
                                searches += usize::from(matches!(kind, WorkStepKindV1::Search { .. }));
                                requests.push(Request {
                                    call: tool_call.id.clone(),
                                    kind,
                                    mine: tool_call
                                        .arguments
                                        .get("mine")
                                        .and_then(Value::as_bool)
                                        .unwrap_or(false),
                                    view,
                                })
                            }
                            Err(fault) => results[index] = Some(answer(fault, true)),
                        }
                    }
                    "create" => {
                        let (content, error) =
                            self.create(&tool_call.arguments, Some(part), true).await;
                        if !error {
                            if let Some(id) =
                                content.split_whitespace().find_map(WorkArtifactId::parse)
                            {
                                placed.push(id);
                            }
                        }
                        results[index] = Some(answer(content, error));
                    }
                    "finish" => {
                        let summary = text_arg(&tool_call.arguments, "summary").unwrap_or_default();
                        let digest = text_arg(&tool_call.arguments, "digest").unwrap_or_default();
                        let found = tool_call
                            .arguments
                            .get("found")
                            .and_then(Value::as_bool)
                            .unwrap_or(true);
                        let need = need_arg(tool_call.arguments.get("need"));
                        finished = Some((summary, digest, found, need));
                        results[index] = Some(answer("Reported to the lead.".into(), false));
                    }
                    name => match sets.iter().find(|set| {
                        set.tools(LeadScope::Helper(spec.helper), &view)
                            .iter()
                            .any(|tool| tool.name == name)
                    }) {
                        Some(set) => {
                            let context = LeadToolContext {
                                run: self.run,
                                part: Some(part),
                            };
                            let outcome = set.call(context, tool_call.clone()).await;
                            self.run.mark_private();
                            if !outcome.is_error {
                                if let Some(id) = outcome
                                    .content
                                    .split_whitespace()
                                    .find_map(WorkArtifactId::parse)
                                {
                                    if outcome.content.starts_with("Placed project") {
                                        placed.push(id);
                                    }
                                }
                            }
                            results[index] = Some(answer(outcome.content, outcome.is_error));
                        }
                        None => {
                            results[index] = Some(answer(
                                format!("{} is not a tool of this part", tool_call.name),
                                true,
                            ))
                        }
                    },
                }
            }
            if !requests.is_empty() {
                let asked: Vec<(String, bool)> = requests
                    .iter()
                    .map(|r| {
                        (
                            r.call.clone(),
                            matches!(r.kind, WorkStepKindV1::Search { .. }),
                        )
                    })
                    .collect();
                match hands.run(requests).await {
                    Ok(done) => {
                        for (id, content, error) in done {
                            let search = asked.iter().any(|(call, search)| *call == id && *search);
                            if search && !error && found.len() < FOUND_CHARS {
                                found.push_str(&clip(&content, 1_500));
                                found.push('\n');
                            }
                            if let Some(at) = calls.iter().position(|c| c.id == id) {
                                results[at] = Some(WorkModelToolResult {
                                    call: id,
                                    content,
                                    is_error: error,
                                });
                            }
                        }
                    }
                    Err(_) => {
                        return PartReport::ended(
                            WorkPartStateV1::Stopped,
                            "Stopped while its pages ran.",
                            placed,
                        )
                    }
                }
            }
            let results: Vec<WorkModelToolResult> = results
                .into_iter()
                .zip(&calls)
                .map(|(result, c)| {
                    result.unwrap_or_else(|| WorkModelToolResult {
                        call: c.id.clone(),
                        content: "Not run".into(),
                        is_error: true,
                    })
                })
                .collect();
            messages.push(WorkModelMessage::ToolResults(results));
            if let Some((summary, digest, found, need)) = finished {
                return PartReport {
                    state: if found && need.is_none() {
                        WorkPartStateV1::Done
                    } else {
                        WorkPartStateV1::Failed
                    },
                    summary: Some(summary),
                    digest: clip(&digest, 3_000),
                    objects: placed,
                    need,
                };
            }
        }
        PartReport {
            state: if placed.is_empty() {
                WorkPartStateV1::Failed
            } else {
                WorkPartStateV1::Done
            },
            summary: None,
            digest: if last_text.is_empty() {
                "It ran out of turns before it reported.".into()
            } else {
                clip(&last_text, 2_000)
            },
            objects: placed,
            need: None,
        }
    }

    /// Whether a part's searches already answer its goal, by the search
    /// provider's own check; false when it cannot tell.
    async fn found_enough(&self, goal: &str, found: &str) -> bool {
        if found.trim().is_empty() {
            return false;
        }
        let scope = zephium_core::work::search::WorkPublicSearchScope {
            provider: self.run.grant.provider,
            model: self.run.grant.model.clone(),
            query: clip(goal, 400),
        };
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        match self
            .search
            .enough(&scope, found, self.run.remaining(), deadline)
            .await
        {
            Ok(decided) => {
                if decided.usage.model_tokens > 0 || decided.usage.cost_micro_usd > 0 {
                    self.run.charge(decided.usage);
                }
                decided.answers
            }
            Err(_) => false,
        }
    }

    /// A part about a service the person uses goes through their own
    /// connection when they have one and accept it (asked once per work),
    /// else through the website in their session.
    async fn route(&self, spec: &mut PartSpec) {
        if self.run.grant.private
            || !matches!(
                spec.helper,
                WorkHelperV1::Browser | WorkHelperV1::Connection
            )
        {
            return;
        }
        let Some(brand) = super::route::brand(spec.service.as_ref(), &spec.title) else {
            return;
        };
        // Reading a service's public site (its product or pricing pages) is
        // not the person's workspace: their connection is not offered.
        if spec.helper == WorkHelperV1::Browser
            && !super::route::reaches_own_app(
                &brand,
                spec.service.as_ref().and_then(|s| s.host.as_deref()),
            )
        {
            return;
        }
        let profile = self.run.profile.to_string();
        let servers = crate::work_connections::store::shared()
            .and_then(|store| store.servers(&profile).ok())
            .unwrap_or_default();
        let offer = super::route::offer(
            &brand,
            crate::work_connections::helper::shared().gh_ready(),
            &servers,
        );
        let accepted = match &offer {
            Some(offer) => self.accepts(offer).await,
            None => false,
        };
        if !accepted && spec.helper == WorkHelperV1::Browser {
            spec.declined = offer.clone();
        }
        if let Some((helper, service)) =
            super::route::settle(spec.helper, &brand, offer.as_ref(), accepted)
        {
            spec.helper = helper;
            spec.service = Some(service);
        }
    }

    /// The person's answer to a connection offer: an earlier answer in this
    /// work stands, otherwise they are asked now.
    async fn accepts(&self, offer: &super::route::Offer) -> bool {
        let earlier = self
            .run
            .probe
            .runtime_projection()
            .await
            .ok()
            .and_then(|projection| {
                projection.executions.iter().rev().find_map(|execution| {
                    execution
                        .steps
                        .iter()
                        .rev()
                        .find_map(|step| match &step.kind {
                            WorkStepKindV1::Ask {
                                options,
                                answer: Some(answer),
                                purpose: Some(WorkAskPurposeV1::Connection),
                                ..
                            } if options.first() == Some(&offer.yes) => Some(answer.clone()),
                            _ => None,
                        })
                })
            });
        let objective = self.objective.to_lowercase();
        let asked_for = self.run.accepted_connection(&offer.yes)
            || objective.contains(&offer.yes.to_lowercase())
            || objective
                .split(|c: char| !c.is_alphanumeric())
                .any(|word| matches!(word, "mcp" | "connection" | "connector"));
        let answer = match earlier {
            _ if asked_for => Some(offer.yes.clone()),
            Some(answer) => Some(answer),
            None => self
                .run
                .ask(
                    WorkAskPurposeV1::Connection,
                    offer.prompt.clone(),
                    vec![
                        offer.yes.clone(),
                        crate::work_connections::gh::DECLINE.into(),
                    ],
                    None,
                )
                .await
                .ok()
                .flatten(),
        };
        let accepted = answer.as_deref() == Some(offer.yes.as_str());
        // The connection helper asks under the same words: it takes this answer.
        if accepted {
            self.run.accept_connection(&offer.yes);
        }
        accepted
    }

    /// The need a part that could not do its job shows on its row, with the
    /// reason its steps show: the one its helper named when it holds, else
    /// what its steps show. A web part for a service the person has a
    /// connection for offers the connection.
    async fn settle_need(&self, part: WorkPartId, spec: &PartSpec, report: &mut PartReport) {
        if report.state == WorkPartStateV1::Done {
            report.need = None;
            return;
        }
        let execution = self.run.execution().await.ok();
        let steps: Vec<&WorkStepFact> = execution
            .iter()
            .flat_map(|e| e.steps.iter())
            .filter(|s| s.part == Some(part))
            .collect();
        let service_host = spec.service.as_ref().and_then(|s| s.host.clone());
        let hosts: Vec<String> = service_host
            .iter()
            .cloned()
            .chain(steps.iter().filter_map(|s| match &s.kind {
                WorkStepKindV1::Read { url, .. } => crate::work_sites::site_of(url),
                _ => None,
            }))
            .collect();
        let helper_reason = match &report.need {
            Some(
                WorkPartNeedV1::Retry { reason, .. } | WorkPartNeedV1::UseConnection { reason, .. },
            ) => *reason,
            _ => None,
        };
        let reason = step_reason(&steps, report.objects.is_empty(), helper_reason);
        // A page that waited on a sign-in wall and ended there needs a
        // sign-in, whatever else went wrong around it.
        let sign_in = match execution
            .as_ref()
            .and_then(|e| e.parts.iter().find(|p| p.id == part))
            .and_then(|p| p.need.clone())
        {
            Some(need @ WorkPartNeedV1::SignIn { .. }) => Some(need),
            _ => steps.iter().find_map(|s| match &s.kind {
                WorkStepKindV1::Read { url, .. }
                    if s.status != WorkStepStatus::Succeeded
                        && s.note.as_deref() == Some(SIGN_IN_NOTE) =>
                {
                    crate::work_sites::site_of(url)
                        .or_else(|| service_host.clone())
                        .map(|host| WorkPartNeedV1::SignIn { host })
                }
                _ => None,
            }),
        }
        .filter(|need| need.validate().is_ok());
        let named = report.need.take().and_then(|need| {
            let need = match need {
                WorkPartNeedV1::AllowFolder { path } => {
                    let home = std::env::var_os("HOME").map(std::path::PathBuf::from)?;
                    let folder = super::folders::folder_for(std::path::Path::new(&path), &home)?;
                    let folder = folder.to_string_lossy().into_owned();
                    let (admitted, _) =
                        crate::work_files::WorkFileGrant::admit(std::slice::from_ref(&folder));
                    (!admitted.is_empty())
                        .then_some(WorkPartNeedV1::AllowFolder { path: folder })?
                }
                WorkPartNeedV1::Retry { host, .. } => WorkPartNeedV1::Retry {
                    host: host.or_else(|| hosts.first().cloned()),
                    reason,
                },
                WorkPartNeedV1::UseConnection { connection, .. } => {
                    if connected(self.run, &connection) {
                        WorkPartNeedV1::UseConnection { connection, reason }
                    } else if let Some(name) = super::route::brand(None, &connection)
                        .and_then(|brand| super::route::known_server(&brand))
                    {
                        WorkPartNeedV1::Connect {
                            connection: name.into(),
                        }
                    } else {
                        WorkPartNeedV1::Retry {
                            host: hosts.first().cloned().filter(|h| public_host(h)),
                            reason,
                        }
                    }
                }
                other => other,
            };
            need.validate().is_ok().then_some(need)
        });
        report.need = sign_in.or(named).or_else(|| {
            let declined_entry = steps.iter().any(|s| match &s.kind {
                WorkStepKindV1::Ask {
                    purpose: Some(WorkAskPurposeV1::Entry),
                    answer: Some(answer),
                    ..
                } => hosts.first().is_some_and(|site| {
                    crate::work_sites::entry_answer(site, answer)
                        == crate::work_sites::EntryAnswer::NotNow
                }),
                _ => false,
            });
            if declined_entry {
                return hosts
                    .first()
                    .map(|host| WorkPartNeedV1::AllowSite { host: host.clone() });
            }
            let reads: Vec<&&WorkStepFact> = steps
                .iter()
                .filter(|s| matches!(s.kind, WorkStepKindV1::Read { .. }))
                .collect();
            let pages_failed = !reads.is_empty()
                && reads
                    .iter()
                    .all(|s| s.status != WorkStepStatus::Succeeded || s.artifacts.is_empty());
            if (pages_failed || reason.is_some()) && report.objects.is_empty() {
                return Some(WorkPartNeedV1::Retry {
                    host: hosts.first().cloned().filter(|h| public_host(h)),
                    reason,
                });
            }
            None
        });
        if let (Some(offer), Some(need)) = (&spec.declined, &report.need) {
            if !matches!(need, WorkPartNeedV1::AllowFolder { .. }) {
                let reason = match need {
                    WorkPartNeedV1::SignIn { .. } => Some(WorkPartReasonV1::SignedOut),
                    WorkPartNeedV1::Retry { reason, .. } => *reason,
                    _ => reason,
                };
                report.need = Some(WorkPartNeedV1::UseConnection {
                    connection: offer.connection.clone(),
                    reason,
                })
                .filter(|need| need.validate().is_ok())
                .or(report.need.take());
            }
        }
        // The website did not do, and the service has a connection people
        // add that this person has not: connecting it is the fix. A request
        // that asked for the connection gets it whatever else went wrong.
        if let Some(name) = super::route::brand(spec.service.as_ref(), &spec.title)
            .filter(|brand| !connected(self.run, brand))
            .and_then(|brand| super::route::known_server(&brand))
        {
            let asked = self
                .objective
                .to_lowercase()
                .split(|c: char| !c.is_alphanumeric())
                .any(|word| matches!(word, "mcp" | "connection" | "connector"));
            if matches!(
                report.need,
                Some(WorkPartNeedV1::Retry { .. } | WorkPartNeedV1::UseConnection { .. })
            ) || (asked && report.need.is_none())
            {
                report.need = Some(WorkPartNeedV1::Connect {
                    connection: name.into(),
                });
            }
        }
        if report.state == WorkPartStateV1::Stopped
            && !matches!(report.need, Some(WorkPartNeedV1::SignIn { .. }))
        {
            report.need = None;
        }
    }

    /// A share of what the run has left, so parts that start together can
    /// all finish and the lead keeps room to build the result.
    fn part_share(&self) -> WorkExecutionLimits {
        let left = self.run.remaining();
        // Parts past the run's parallel slots wait: they do not share now.
        let ways = (self.parts_running().min(PARALLEL_PARTS) as u32 + 1).max(2);
        WorkExecutionLimits {
            // A search reserves its whole context: a part holds room for one.
            model_tokens: (left.model_tokens / ways)
                .max(PART_MIN_TOKENS)
                .clamp(1, PART_MAX.model_tokens)
                .min(left.model_tokens),
            cost_micro_usd: (left.cost_micro_usd / ways)
                .max(PART_MIN_COST)
                .clamp(1, PART_MAX.cost_micro_usd)
                .min(left.cost_micro_usd),
            operations: (left.operations / ways).clamp(1, PART_MAX.operations),
            timeout_seconds: left.timeout_seconds,
            max_workers: PART_MAX.max_workers.min(left.max_workers).max(1),
        }
    }
}

fn add(a: WorkUsage, b: WorkUsage) -> WorkUsage {
    WorkUsage {
        model_tokens: a.model_tokens.saturating_add(b.model_tokens),
        cost_micro_usd: a.cost_micro_usd.saturating_add(b.cost_micro_usd),
        operations: a.operations.saturating_add(b.operations),
        accounting: if b.accounting == WorkUsageAccounting::ConservativeReservation {
            b.accounting
        } else {
            a.accounting
        },
    }
}

/// A helper's page, search or file call as the step it becomes.
/// Whether the person has the connection a need names: gh, or an enabled
/// server of theirs by its name or id.
fn connected(run: &super::run::LeadRun, connection: &str) -> bool {
    let Some(brand) = super::route::brand(None, connection) else {
        return false;
    };
    let servers = crate::work_connections::store::shared()
        .and_then(|store| store.servers(&run.profile.to_string()).ok())
        .unwrap_or_default();
    super::route::offer(
        &brand,
        crate::work_connections::helper::shared().gh_ready(),
        &servers,
    )
    .is_some()
}

/// A page task that only reads one of the person's daily apps (what is new
/// in Slack, the Linear inbox): it is read from the app's own views first,
/// with no model call, whatever the helper set. A goal that asks to send,
/// post or change something is a task, not a read; a sentence that forbids
/// it ("do not change anything") asks nothing.
fn daily_app(kind: &WorkStepKindV1) -> bool {
    let WorkStepKindV1::Read { url, .. } = kind else {
        return false;
    };
    // Loopback replicas of the apps (.test sites) stand in for them in
    // qualification.
    url::Url::parse(url)
        .ok()
        .is_some_and(|url| match url.host() {
            Some(url::Host::Domain(host)) => {
                zephium_agentic::DailyApp::of(host).is_some() || host.ends_with(".test")
            }
            Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
            _ => false,
        })
}

fn reads_app(kind: &WorkStepKindV1) -> bool {
    const ACTS: [&str; 22] = [
        "send", "post", "reply", "write", "create", "draft", "comment", "assign", "archive",
        "delete", "remove", "invite", "schedule", "book", "update", "move", "mark", "react",
        "forward", "edit", "submit", "fill",
    ];
    const NOT: [&str; 5] = ["do not", "don't", "never", "without", "no "];
    let WorkStepKindV1::Read {
        goal: Some(goal), ..
    } = kind
    else {
        return false;
    };
    if !daily_app(kind) {
        return false;
    }
    let goal = goal.to_lowercase();
    !goal
        .split(['.', ';', '\n', '!'])
        .filter(|sentence| !NOT.iter().any(|not| sentence.contains(not)))
        .flat_map(|sentence| sentence.split(|c: char| !c.is_alphanumeric()))
        .any(|word| ACTS.contains(&word))
}

pub(crate) fn step_request(
    name: &str,
    args: &Value,
    run: &super::run::LeadRun,
) -> Result<WorkStepKindV1, String> {
    let text = |key: &str| text_arg(args, key);
    let kind = match name {
        "web_search" => {
            let query = text("query").ok_or("query is required")?;
            if query.contains(';') || query.contains(" | ") {
                return Err("query is one question: search each question in its own call".into());
            }
            WorkStepKindV1::Search { query }
        }
        "read" => {
            let url = text("url").ok_or("url is required")?;
            WorkStepKindV1::Read {
                url,
                collection: hands::collection(args.get("records"))?,
                goal: None,
            }
        }
        "browse" => {
            let start = text("start").ok_or("start is required")?;
            let goal = text("goal").ok_or("goal is required")?;
            if goal.len() > MAX_WORK_PAGE_GOAL_BYTES {
                return Err("goal is at most 600 bytes".into());
            }
            let url = if start.starts_with("https://") {
                start
            } else {
                let site = start
                    .trim_start_matches("http://")
                    .trim_start_matches("www.")
                    .trim_end_matches('/')
                    .to_ascii_lowercase();
                if !public_host(&site) {
                    return Err("start is an https page or a bare site such as airbnb.com".into());
                }
                format!("https://{site}/")
            };
            WorkStepKindV1::Read {
                url,
                collection: hands::collection(args.get("records"))?,
                goal: Some(goal),
            }
        }
        "list" => WorkStepKindV1::List {
            path: text("path").ok_or("path is required")?,
            depth: args
                .get("depth")
                .and_then(Value::as_u64)
                .map(|d| d.clamp(1, 3) as u8),
        },
        "read_file" => WorkStepKindV1::ReadFile {
            path: text("path").ok_or("path is required")?,
            offset: args
                .get("offset")
                .and_then(Value::as_u64)
                .map(|v| v.max(1) as u32),
            limit: args
                .get("limit")
                .and_then(Value::as_u64)
                .map(|v| v.clamp(1, 2000) as u32),
        },
        "search_files" => WorkStepKindV1::SearchFiles {
            path: text("path").ok_or("path is required")?,
            query: text("query").ok_or("query is required")?,
            glob: text("glob"),
            regex: args.get("regex").and_then(Value::as_bool),
        },
        "write_file" => WorkStepKindV1::WriteFile {
            path: text("path").ok_or("path is required")?,
            content: args
                .get("content")
                .and_then(Value::as_str)
                .ok_or("content is required")?
                .to_owned(),
            decision: None,
        },
        "edit_file" => WorkStepKindV1::EditFile {
            path: text("path").ok_or("path is required")?,
            old: args
                .get("old")
                .and_then(Value::as_str)
                .ok_or("old is required")?
                .to_owned(),
            new: args
                .get("new")
                .and_then(Value::as_str)
                .ok_or("new is required")?
                .to_owned(),
            replacements: vec![],
            decision: None,
        },
        "run_command" => WorkStepKindV1::RunCommand {
            cwd: text("cwd").ok_or("cwd is required")?,
            command: text("command").ok_or("command is required")?,
            timeout_secs: args
                .get("timeout_secs")
                .and_then(Value::as_u64)
                .map(|v| v.clamp(1, 600) as u32),
            decision: None,
        },
        _ => return Err(format!("{name} is not a tool here")),
    };
    if let WorkStepKindV1::Search { query } = &kind {
        zephium_core::work::search::validate_public_search_query(query)
            .map_err(|_| "query is one line of at most 512 characters".to_owned())?;
    }
    if let WorkStepKindV1::Read {
        url, goal: None, ..
    } = &kind
    {
        if !run.known_url(url) && !plain_page(url) {
            return Err("read takes a url you were given (a source, a link a page showed, one in the request) or a site's own page address with no query, such as https://www.hetzner.com/cloud; search or browse the site to find others".into());
        }
    }
    let probe = WorkStepFact {
        id: WorkStepId::from(1),
        turn: 1,
        kind: kind.clone(),
        status: WorkStepStatus::Running,
        usage: None,
        artifacts: vec![],
        evidence: None,
        note: None,
        measurements: None,
        local: None,
        account: None,
        part: None,
    };
    probe
        .validate()
        .map_err(|_| format!("{name}: the arguments are outside their limits"))?;
    Ok(kind)
}

/// A site's own page by its plain address: https, a public host, no query,
/// fragment or credentials, and a short path of plain words, such as a
/// pricing or docs page. Such an address carries nothing but its name.
fn plain_page(url: &str) -> bool {
    let Ok(parsed) = url::Url::parse(url) else {
        return false;
    };
    let segments: Vec<&str> = parsed.path().split('/').filter(|s| !s.is_empty()).collect();
    parsed.scheme() == "https"
        && parsed.query().is_none()
        && parsed.fragment().is_none()
        && parsed.username().is_empty()
        && parsed.password().is_none()
        && parsed.port().is_none()
        && parsed.host_str().is_some_and(public_host)
        && segments.len() <= 4
        && segments.iter().all(|s| {
            s.len() <= 40
                && s.chars().all(|c| {
                    c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '-' | '_' | '.')
                })
        })
}

/// Pages the person gave in their request (or the lead put in the part's
/// goal) on the part's own site: a browser part reads them as given first.
/// Pages in the person's own apps go through the part's page tasks instead.
fn given_pages(objective: &str, spec: &PartSpec) -> Vec<String> {
    let site = spec
        .service
        .as_ref()
        .and_then(|service| service.host.as_deref())
        .and_then(|host| crate::work_sites::site_of(&format!("https://{host}/")));
    let mut pages: Vec<String> = Vec::new();
    for text in [objective, spec.goal.as_str(), spec.brief.as_str()] {
        for word in
            text.split(|c: char| c.is_whitespace() || matches!(c, '<' | '>' | '"' | '(' | ')'))
        {
            let word = word.trim_end_matches(['.', ',', ';', ':', '!', '?', '\'']);
            let Ok(url) = url::Url::parse(word) else {
                continue;
            };
            let url = url.to_string();
            if !url.starts_with("https://")
                || url.len() > 1024
                || crate::work_sites::personal_page(&url)
                || site
                    .as_ref()
                    .is_some_and(|site| crate::work_sites::site_of(&url).as_ref() != Some(site))
                || pages.contains(&url)
            {
                continue;
            }
            pages.push(url);
        }
    }
    pages.truncate(2);
    pages
}

/// The fields a part returns for each thing, as the rows a page read
/// collects: a link, up to three pictures, the rest as text.
fn records_collection(
    title: &str,
    records: &[String],
) -> Option<zephium_core::work::collection::WorkBrowseCollection> {
    let mut columns: Vec<Value> = Vec::new();
    let mut names: Vec<String> = Vec::new();
    let (mut link, mut pictures) = (false, 0);
    for field in records {
        let name: String = field
            .trim()
            .to_ascii_lowercase()
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
            .collect();
        let name = name.trim_matches('_').to_owned();
        if name.is_empty() || matches!(name.as_str(), "name" | "title") || names.contains(&name) {
            continue;
        }
        let kind = if (name.contains("url") || name.contains("link")) && !link {
            link = true;
            "url"
        } else if ["photo", "image", "picture", "img"]
            .iter()
            .any(|w| name.contains(w))
        {
            if pictures == 3 {
                continue;
            }
            pictures += 1;
            "image_url"
        } else {
            "text"
        };
        names.push(name.clone());
        columns.push(serde_json::json!({"name": name, "value": {"kind": kind}, "required": kind == "url", "extraction": "generate"}));
    }
    if columns.is_empty() {
        return None;
    }
    let max_items = (256 / (columns.len() + 1)).min(12);
    let value =
        serde_json::json!({"title": clip(title, 60), "max_items": max_items, "columns": columns});
    hands::collection(Some(&value)).ok().flatten()
}

/// A helper's `need` argument: `{kind, target}`.
fn need_arg(value: Option<&Value>) -> Option<WorkPartNeedV1> {
    let value = value?;
    let target = value
        .get("target")
        .and_then(Value::as_str)
        .map(|t| t.trim().to_owned())
        .filter(|t| !t.is_empty());
    let host = |t: Option<String>| {
        t.map(|t| {
            t.trim_start_matches("https://")
                .trim_start_matches("http://")
                .trim_start_matches("www.")
                .split('/')
                .next()
                .unwrap_or("")
                .to_ascii_lowercase()
        })
    };
    let reason = match value.get("reason").and_then(Value::as_str) {
        Some("couldnt_read") => Some(WorkPartReasonV1::CouldntRead),
        Some("blocked_by_check") => Some(WorkPartReasonV1::BlockedByCheck),
        Some("not_found") => Some(WorkPartReasonV1::NotFound),
        Some("site_error") => Some(WorkPartReasonV1::SiteError),
        Some("no_answer") => Some(WorkPartReasonV1::NoAnswer),
        _ => None,
    };
    Some(match value.get("kind").and_then(Value::as_str)? {
        "sign_in" => WorkPartNeedV1::SignIn {
            host: host(target)?,
        },
        "allow_site" => WorkPartNeedV1::AllowSite {
            host: host(target)?,
        },
        "allow_folder" => WorkPartNeedV1::AllowFolder { path: target? },
        "use_connection" => WorkPartNeedV1::UseConnection {
            connection: target?,
            reason,
        },
        "retry" => WorkPartNeedV1::Retry {
            host: host(target).filter(|h| public_host(h)),
            reason,
        },
        _ => return None,
    })
}

/// The need in the lead's words.
fn need_words(need: &WorkPartNeedV1) -> String {
    match need {
        WorkPartNeedV1::SignIn { host } => format!("the person to sign in on {host}"),
        WorkPartNeedV1::AllowSite { host } => format!("the person to allow work on {host}"),
        WorkPartNeedV1::AllowFolder { path } => format!("the person to allow reading {path}"),
        WorkPartNeedV1::UseConnection { connection, .. } => {
            format!("the person to choose their {connection} connection instead of its website")
        }
        WorkPartNeedV1::Connect { connection } => format!(
            "the person to connect {connection} in Settings: there is no {connection} connection yet"
        ),
        WorkPartNeedV1::Retry { host, reason } => {
            let site = host.as_deref().unwrap_or("the site");
            match reason {
                Some(WorkPartReasonV1::CouldntRead) => {
                    format!("another try: {site} loaded but could not be read")
                }
                Some(WorkPartReasonV1::SignedOut) => {
                    format!("another try: {site} showed its signed-out view")
                }
                Some(WorkPartReasonV1::BlockedByCheck) => {
                    format!("another try: {site} asked for a human check")
                }
                Some(WorkPartReasonV1::NotFound) => {
                    format!("another try: {site} showed nothing that matched")
                }
                Some(WorkPartReasonV1::SiteError) => {
                    format!("another try: {site} failed on its side")
                }
                Some(WorkPartReasonV1::NoAnswer) => {
                    format!("another try: {site} did not answer in time")
                }
                None if host.is_some() => format!("another try on {site}"),
                None => "another try".into(),
            }
        }
    }
}

/// Why a part's steps show it could not do its job: the last failed page or
/// search in closed words, else what the helper named, else nothing found
/// where its pages read cleanly.
fn step_reason(
    steps: &[&WorkStepFact],
    nothing_placed: bool,
    named: Option<WorkPartReasonV1>,
) -> Option<WorkPartReasonV1> {
    let failed = steps.iter().rev().find(|s| {
        matches!(
            s.kind,
            WorkStepKindV1::Read { .. } | WorkStepKindV1::Search { .. }
        ) && matches!(
            s.status,
            WorkStepStatus::Failed | WorkStepStatus::OutcomeUnknown
        )
    });
    if let Some(reason) = failed.and_then(|step| {
        note_reason(
            step.note.as_deref(),
            matches!(step.kind, WorkStepKindV1::Search { .. }),
        )
    }) {
        return Some(reason);
    }
    if named.is_some() {
        return named;
    }
    let read_cleanly = failed.is_none()
        && steps.iter().any(|s| {
            matches!(
                s.kind,
                WorkStepKindV1::Read { .. } | WorkStepKindV1::Search { .. }
            )
        });
    (read_cleanly && nothing_placed).then_some(WorkPartReasonV1::NotFound)
}

/// A failed step's closed note as the reason the canvas phrases.
fn note_reason(note: Option<&str>, search: bool) -> Option<WorkPartReasonV1> {
    use zephium_core::work::runtime::read_note;
    use WorkPartReasonV1 as R;
    let note = note.unwrap_or("");
    Some(match note {
        SIGN_IN_NOTE => R::SignedOut,
        read_note::HUMAN_CHECK => R::BlockedByCheck,
        read_note::CONSTRUCTION_TIMEOUT
        | read_note::SLOW_SITE
        | "The page took too long"
        | "The page could not be opened"
        | "The search could not be sent" => R::NoAnswer,
        "The search gave no usable sources" => R::NotFound,
        // This Mac's browser or the run's own search, not the site: no
        // reason to give.
        "The browser was not available"
        | "The browser was not ready for this page"
        | "The browsing profile was not available"
        | "Too many pages were already open" => return None,
        // "state.gov asks for a human check; its page was skipped", and a
        // page left waiting on the person: "Airbnb needs you to continue".
        _ if note.contains("human check") || note.ends_with("needs you to continue") => {
            R::BlockedByCheck
        }
        _ if search => return None,
        _ => R::CouldntRead,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_page_the_person_gave_is_read_as_given_by_the_part_for_its_site() {
        let spec = PartSpec {
            title: "Sets".into(),
            helper: WorkHelperV1::Browser,
            goal: "Pick three sets".into(),
            brief: String::new(),
            service: Some(WorkPartServiceV1 {
                host: Some("lego.com".into()),
                connection: None,
            }),
            records: vec![
                "price".into(),
                "pieces".into(),
                "url".into(),
                "photo".into(),
            ],
            search: None,
            declined: None,
        };
        assert_eq!(
            given_pages(
                "Open https://www.lego.com/en-us/themes/architecture, pick three sets; see https://example.com/x.",
                &spec
            ),
            ["https://www.lego.com/en-us/themes/architecture"]
        );
        let slack = PartSpec {
            service: None,
            ..spec
        };
        assert!(given_pages("Summarise https://app.slack.com/client/T1/C2", &slack).is_empty());
        let collection = records_collection("Sets", &slack.records).unwrap();
        assert_eq!(collection.columns.len(), 4);
        assert!(collection
            .columns
            .iter()
            .all(|c| c.required == (c.name == "url")));
    }

    #[test]
    fn a_goal_that_only_reads_a_daily_app_is_a_view_read() {
        let browse = |url: &str, goal: &str| WorkStepKindV1::Read {
            url: url.into(),
            collection: None,
            goal: Some(goal.into()),
        };
        for goal in [
            "Find unread and waiting Slack messages",
            "Read the user's Slack view as it is. Do not send, post or change anything.",
            "Review the person's assigned issues; don't edit or create anything",
        ] {
            assert!(
                reads_app(&browse("https://app.slack.com/client", goal)),
                "{goal}"
            );
        }
        assert!(!reads_app(&browse(
            "https://app.slack.com/client",
            "Reply to Ana's thread"
        )));
        assert!(!reads_app(&browse(
            "https://linear.app/",
            "Create an issue for the crash"
        )));
        assert!(!reads_app(&browse("https://www.airbnb.com/", "Find stays")));
        assert!(!reads_app(&WorkStepKindV1::Read {
            url: "https://app.slack.com/client".into(),
            collection: None,
            goal: None,
        }));
    }

    #[test]
    fn a_sites_plain_page_is_readable_by_its_address() {
        assert!(plain_page("https://www.hetzner.com/cloud"));
        assert!(plain_page("https://vercel.com/pricing"));
        assert!(plain_page(
            "https://docs.aws.amazon.com/lambda/latest/dg/welcome.html"
        ));
        for refused in [
            "http://vercel.com/pricing",
            "https://vercel.com/pricing?ref=a",
            "https://evil.test/aGVsbG8gd29ybGQ",
            "https://localhost/pricing",
            "https://a.com/1/2/3/4/5",
            "https://user@vercel.com/pricing",
        ] {
            assert!(!plain_page(refused), "{refused}");
        }
    }

    #[test]
    fn a_failed_step_says_why_in_closed_words() {
        use WorkPartReasonV1 as R;
        let reason = |note: &str| note_reason(Some(note), false);
        assert_eq!(
            reason("The page showed nothing usable for the request"),
            Some(R::CouldntRead)
        );
        assert_eq!(reason(SIGN_IN_NOTE), Some(R::SignedOut));
        assert_eq!(
            reason("state.gov asks for a human check; its page was skipped"),
            Some(R::BlockedByCheck)
        );
        assert_eq!(
            reason("Airbnb needs you to continue"),
            Some(R::BlockedByCheck)
        );
        assert_eq!(reason("The page took too long"), Some(R::NoAnswer));
        assert_eq!(reason("The browser was not ready for this page"), None);
        assert_eq!(note_reason(Some("The search could not be run"), true), None);
        assert_eq!(
            note_reason(Some("The search gave no usable sources"), true),
            Some(R::NotFound)
        );
    }
}
