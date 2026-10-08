//! The routine agent loop. Rust admits every turn's operations, commits each
//! step as it settles, and enforces budgets; the model only proposes.
#[path = "work_agent_local.rs"]
mod local;
use crate::work_runtime::*;
use std::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
    time::{Duration, Instant},
};
use zephium_core::{
    ids::ProfileId,
    work::{agent::*, artifact::*, port::*, runtime::*, search::*, synthesis::*, *},
};
use zephium_ipc::work::WorkActivityV1;

#[path = "work_agent_reads.rs"]
mod reads;

/// A page read or page task the loop admitted for one step. The host opens
/// it in the session the run decided for its site.
#[derive(Clone)]
pub struct WorkAgentBrowseRequest {
    pub construction_attempt: zephium_agentic::WorkBrowserConstructionAttempt,
    pub id: WorkStepId,
    pub step: WorkStepKindV1,
    pub hops: u8,
    pub objective: String,
    pub output: String,
    pub limits: WorkExecutionLimits,
    /// The person's session on the page's site, or the run's own storage.
    pub session: crate::work_sites::SiteSession,
    /// Where a page task asks the person about a step that commits something.
    pub confirm: Option<WorkConfirmPort>,
    /// The person allowed edits on this site for the run.
    pub allow_edits: bool,
    /// Typing on this site waits for the person: the run holds their data
    /// and they did not name the site.
    pub hold_typing: bool,
    /// The site's entry question waits on the start page: Rust checks it
    /// for a signed-out state before the question and before any model call.
    pub entry: bool,
    /// A page task that only reads what a daily app's view lists.
    pub view: bool,
}

/// A step a page task holds for the person, as the site policy read it
/// from the page.
#[derive(Clone, Debug)]
pub struct WorkSiteConfirmation {
    pub category: WorkConfirmCategoryV1,
    pub headline: String,
    pub action: String,
    pub text: Option<String>,
    pub facts: Vec<WorkConfirmFactV1>,
    pub run_option: bool,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WorkSiteDecision {
    Approve,
    AllowForRun,
    Decline,
}
/// How a held step ended once the person decided.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WorkSiteReceipt {
    Committed,
    /// It ran, but the page never showed it happened.
    Unverified,
    /// It never ran: the page changed first or the step was not taken.
    NotSent,
    Declined,
}

/// What the start page's check or the person decided about entering a site.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WorkSiteEntry {
    /// The start page is signed out: nothing to ask.
    SignedOut,
    Allow,
    Always,
    NotNow,
}

#[derive(Default)]
struct ConfirmMailbox {
    /// The page task's entry: asked, then answered or found signed out.
    entry_asked: bool,
    entry: Option<WorkSiteEntry>,
    next: u32,
    asks: std::collections::VecDeque<(u32, WorkSiteConfirmation)>,
    decisions: Vec<(u32, WorkSiteDecision)>,
    settled: std::collections::VecDeque<(u32, WorkSiteReceipt)>,
    /// The page began or stopped waiting on the person.
    needs_you: Option<Option<WorkPageWait>>,
    waiting_on_you: Option<WorkPageWait>,
}
/// What a page waits on the person for.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WorkPageWait {
    /// A bot check, a permission or a step the agent cannot take.
    Check,
    /// A sign-in on the page's site.
    SignIn,
}
/// One page task's line to the loop: held steps go out, decisions come
/// back, receipts go out. Polled on both sides; it carries no authority.
#[derive(Clone, Default)]
pub struct WorkConfirmPort(std::sync::Arc<std::sync::Mutex<ConfirmMailbox>>);
impl WorkConfirmPort {
    fn mailbox(&self) -> std::sync::MutexGuard<'_, ConfirmMailbox> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
    pub fn ask(&self, confirmation: WorkSiteConfirmation) -> u32 {
        let mut mailbox = self.mailbox();
        mailbox.next += 1;
        let id = mailbox.next;
        mailbox.asks.push_back((id, confirmation));
        id
    }
    pub fn decision(&self, id: u32) -> Option<WorkSiteDecision> {
        let mut mailbox = self.mailbox();
        let at = mailbox.decisions.iter().position(|(ask, _)| *ask == id)?;
        Some(mailbox.decisions.remove(at).1)
    }
    pub fn settle(&self, id: u32, receipt: WorkSiteReceipt) {
        self.mailbox().settled.push_back((id, receipt));
    }
    pub(crate) fn take_ask(&self) -> Option<(u32, WorkSiteConfirmation)> {
        self.mailbox().asks.pop_front()
    }
    pub(crate) fn decide(&self, id: u32, decision: WorkSiteDecision) {
        self.mailbox().decisions.push((id, decision));
    }
    pub(crate) fn take_settled(&self) -> Option<(u32, WorkSiteReceipt)> {
        self.mailbox().settled.pop_front()
    }
    /// The start page is signed in or unknown: put the entry question.
    pub fn ask_entry(&self) {
        self.mailbox().entry_asked = true;
    }
    /// The start page is signed out: the page goes on without a question.
    pub fn signed_out(&self) {
        self.mailbox().entry = Some(WorkSiteEntry::SignedOut);
    }
    pub fn entry(&self) -> Option<WorkSiteEntry> {
        self.mailbox().entry
    }
    pub(crate) fn take_entry_ask(&self) -> bool {
        std::mem::take(&mut self.mailbox().entry_asked)
    }
    pub(crate) fn answer_entry(&self, entry: WorkSiteEntry) {
        self.mailbox().entry = Some(entry);
    }
    /// The page waits on the person, or stopped waiting.
    pub fn needs_you(&self, wait: Option<WorkPageWait>) {
        let mut mailbox = self.mailbox();
        if mailbox.waiting_on_you != wait {
            mailbox.waiting_on_you = wait;
            mailbox.needs_you = Some(wait);
        }
    }
    pub(crate) fn take_needs_you(&self) -> Option<Option<WorkPageWait>> {
        self.mailbox().needs_you.take()
    }
}
pub struct WorkBrowserOutcome {
    pub status: WorkStepStatus,
    pub usage: Option<WorkUsage>,
    pub artifacts: Vec<WorkArtifactDraft>,
    pub intervention: Option<WorkInterventionV1>,
    /// Why the page gave nothing, in closed words the model and the person read.
    pub note: Option<String>,
    /// Closed wall time, provider call counts and exact accounting for this read.
    pub measurements: Option<WorkStepMeasurementsV1>,
    /// A person was shown this page and continued it.
    pub helped: bool,
    /// The page agent was stopped before a step that would commit something.
    pub held_back: bool,
    /// The page ended so that its task can start over once: the person
    /// finished a sign-in in a tab while it waited, or saving a cookie
    /// refusal replaced the page. The site counts as allowed for the rerun.
    pub rerun: bool,
}
pub struct WorkAgentProviders<'a> {
    pub turn: &'a dyn WorkAgentTurnProvider,
    pub search: &'a dyn WorkPublicSearchProvider,
}

/// Tokens a turn must still be able to spend before the loop stops itself.
const TURN_TOKEN_FLOOR: u32 = 12_000;
/// Consecutive refused, idle or dropped turns before the run gives up.
/// Cancellation polls the store may fail to answer in a row before a run stops.
const MAX_UNREADABLE_POLLS: u8 = 12;
const MAX_FAILED_TURNS: u8 = 3;
const STEPS_EXHAUSTED: &str = "The step budget is used up: no more searches or reads will run. Publish the result from the sources already collected and finish.";
const ASK_POLL: Duration = Duration::from_millis(500);
const MAX_PREVIEWS: usize = 96;
const INHERITED_PREVIEWS: usize = 64;
const INHERITED_ARTIFACTS: usize = 32;
/// Earlier executions whose objects a run inherits whole; older ones reach
/// the model through the thread, and by object only when the request names it.
const INHERITED_EXECUTIONS: usize = 3;
/// How long a run waits on the person's answer or decision. The wait never
/// spends the run's deadline; past it, the run stops on the question and the
/// next request answers it.
const WAIT_PATIENCE: Duration = Duration::from_secs(30 * 60);
const ASK_SUSPENDED: &str = "Waiting for your answer";
const DECISION_SUSPENDED: &str = "Waiting for your decision";
const BUDGET_SPENT: &str = "The run used its turns, steps or budget before it could finish";
const BUDGET_EXHAUSTED: &str = "The token or cost budget is used up: no more searches or reads will run. Publish the result from the sources already collected and finish.";
const CONTEXT_FULL: &str = "The work has grown too large for one turn";
const TURN_UNPREPARED: &str = "The turn could not be prepared";
const OUT_OF_TIME: &str = "The run ran out of time";
const STOPPED: &str = "Stopped by you";
const INTERRUPTED: &str = "The run was interrupted";
const ANSWER_LOST: &str = "The answer was lost on the way; it may have been charged";
const RUN_BROKEN: &str = "The run could not record its progress";
const NO_PROGRESS: &str = "The agent stopped making progress";
const PRIVATE_QUERY: &str = "A search would have repeated private context";
const UNPUBLISHED: &str = "The result could not be placed on the canvas";
const OVER_BUDGET: &str = "The model's turn cost more than the run had left";
const HELD_BACK: &str = "A browse stopped before a step that would commit something for the person (send, post, publish, pay, book, delete, share, save or submit). This version cannot ask them to confirm it: tell them in say what is ready and what is left for them to do.";
const KEEP_GOING: &str = "Keep going";
const STOP_HERE: &str = "Stop";

/// Closed loop facts for development logs; never model, page or user text.
#[derive(Clone, Copy, Debug)]
pub enum WorkAgentDiagnostic {
    TurnAdmitted {
        turn: u8,
        artifacts: usize,
        dropped: usize,
        fetches: usize,
        asks: bool,
        finish: bool,
    },
    TurnRefused {
        turn: u8,
        /// None when the provider returned nothing to resolve.
        reason: Option<zephium_core::work::agent::WorkAgentTurnRefusal>,
    },
    /// A search answered but nothing in it could be admitted.
    SearchRefused { note: &'static str },
    /// The loop stopped because the durable state says so.
    Stopped {
        cause: crate::work_runtime::WorkCancelCause,
    },
    ArtifactRefused {
        turn: u8,
        reason: zephium_core::work::agent::WorkAgentArtifactRefusal,
    },
    /// A page whose check may have passed in the background is loaded once more.
    ReadRetried,
    /// One settled page read, measured: wall time, provider calls and exact cost.
    ReadMeasured(WorkStepMeasurementsV1),
    /// The loop ended on an error the run reports as interrupted.
    LoopFailed { error: WorkError },
    /// No turn could be disclosed, even after shedding older context.
    DisclosureRefused { error: WorkError },
    /// Turns, steps, tokens or cost left no room for another turn.
    BudgetSpent,
    /// A question or proposal stayed open past the wait; the run stops on it.
    AskSuspended,
    /// A step failed on an error the person reads as the step's note.
    StepFailed {
        kind: &'static str,
        error: WorkError,
    },
    /// Objects a step or turn produced that the canvas could not take.
    ArtifactsDropped { kind: &'static str, count: usize },
    CommitRefused {
        kind: &'static str,
        error: WorkError,
    },
    /// A page opened in the person's session: a task or a read, and its path shape.
    SessionPage { task: bool, path: WorkPathClass },
    /// A page task without the person's session, and why.
    PrivatePage {
        because: crate::work_sites::PrivateBecause,
    },
    /// The person answered a site's entry question.
    /// A page task's start page was signed out: no entry question.
    SiteSignedOut,
    SiteEntered {
        answer: crate::work_sites::EntryAnswer,
    },
    /// A page task stopped before a committing step.
    HeldBack,
    /// The run reached its budget and asked whether to keep going.
    KeepGoing { granted: bool },
    /// The person's open tabs listed as context, by count.
    TabsListed { count: u8 },
}

pub struct WorkAgentService {
    handle: crate::Handle,
    diagnostic: Option<fn(WorkAgentDiagnostic)>,
}
impl WorkAgentService {
    pub fn new(handle: crate::Handle) -> Self {
        Self {
            handle,
            diagnostic: None,
        }
    }
    pub fn with_diagnostic(mut self, diagnostic: fn(WorkAgentDiagnostic)) -> Self {
        self.diagnostic = Some(diagnostic);
        self
    }

    /// Admits the objective, begins the single attempt and runs turns until
    /// the agent finishes, a budget ends, the user stops it, or a provider
    /// outcome becomes unknown. Every step is durable before the next begins.
    pub async fn run<B, Fut, O>(
        &self,
        profile: ProfileId,
        command: zephium_ipc::work::WorkCommandV1,
        selection: Option<context::WorkContextSelectionV1>,
        providers: WorkAgentProviders<'_>,
        mut browser: B,
        mut observe: O,
    ) -> Result<WorkRuntimeProjection, WorkError>
    where
        B: FnMut(WorkAttemptProbe, WorkAgentBrowseRequest) -> Fut,
        Fut: Future<Output = Result<WorkBrowserOutcome, WorkError>>,
        O: FnMut(WorkAttemptObserver),
    {
        if command.version != 1 || !matches!(command.intent, WorkRuntimeIntent::BeginAgent { .. }) {
            return Err(WorkError::Invalid);
        }
        let work = command.work;
        let command_id = command.command;
        if let WorkRuntimeIntent::BeginAgent { grant, .. } = &command.intent {
            grant.validate()?;
            // Origin grants are retired: a run works in the person's session
            // per site, asking first.
            if !grant.accounts.is_empty() {
                return Err(WorkError::Invalid);
            }
        }
        let (request, bodies, private, tabs) = match selection {
            Some(selection) => {
                let admitted = crate::work_context::WorkContextAdmission::new(self.handle.clone())
                    .admit(profile, context::WorkContextPurpose::Agent, &selection)
                    .await?;
                let private: Vec<String> = admitted
                    .disclosure
                    .items
                    .iter()
                    .zip(&admitted.bodies)
                    .filter(|(item, _)| item.visibility == context::WorkContextVisibility::Private)
                    .map(|(_, body)| body.text.clone())
                    .collect();
                let tabs = admitted.disclosure.tabs.clone();
                let zephium_ipc::work::WorkCommandV1 {
                    work,
                    expected_revision,
                    command,
                    intent,
                    ..
                } = command;
                (
                    WorkRequest::RuntimeCommandDisclosed {
                        id: work,
                        expected: expected_revision,
                        command,
                        intent,
                        context: admitted.disclosure,
                    },
                    admitted.bodies,
                    private,
                    tabs,
                )
            }
            None => (command.into_request()?, Vec::new(), Vec::new(), Vec::new()),
        };
        request.validate()?;
        let response = tokio::time::timeout(
            Duration::from_secs(10),
            self.handle.submit_work_document(request, Some(profile))?,
        )
        .await
        .map_err(|_| WorkError::OutcomeUnknown)??;
        if response.profile != profile {
            return Err(WorkError::ProfileUnavailable);
        }
        let WorkReply::AgentAdmitted {
            projection,
            receipt,
            replayed,
        } = response.reply
        else {
            return Err(WorkError::Invalid);
        };
        if receipt.command != command_id || projection.work.id != work {
            return Err(WorkError::Invalid);
        }
        if replayed {
            return Ok(*projection);
        }
        let execution = projection
            .executions
            .iter()
            .find(|entry| entry.id == receipt.execution)
            .ok_or(WorkError::Invalid)?;
        let grant = execution.agent_grant().cloned().ok_or(WorkError::Invalid)?;
        if execution.authorization != WorkExecutionAuthorization::UserDirectedAgent
            || projection.work.revision != receipt.applied_revision
        {
            return Err(WorkError::Invalid);
        }
        let node = execution.spec.nodes[0].node;
        let standing = crate::work_sites::standing(&self.handle, profile)
            .await
            .unwrap_or_default();
        let sites = crate::work_sites::RunSites::new(grant.private, standing);
        let attempt = WorkRuntimeService::new(self.handle.clone())
            .begin_node(
                profile,
                work,
                receipt.applied_revision,
                receipt.execution,
                node,
            )
            .await?;
        observe(attempt.observer());
        let original = attempt.attempt();
        let mut driver = Driver {
            probe: attempt.probe(),
            grant,
            limits: attempt.specification().limits,
            output: attempt.node().outputs[0].name.clone(),
            objective: attempt.disclosure_objective()?,
            decisions: attempt.decisions().to_vec(),
            bodies,
            private,
            inherited: Vec::new(),
            kept: Vec::new(),
            thread: Vec::new(),
            unreadable_polls: std::sync::atomic::AtomicU8::new(0),
            stopped: std::sync::Mutex::new(None),
            waiting: false,
            files: None,
            previews: Vec::new(),
            used: WorkUsage::default(),
            steps: 0,
            turn: 0,
            failed_turns: 0,
            intervention: None,
            diagnostic: self.diagnostic,
            notices: Vec::new(),
            published: 0,
            finish_refusals: 0,
            pending_output_repair: false,
            sites,
            session_steps: Vec::new(),
            page_sites: Vec::new(),
            private_sites: Vec::new(),
            base_limits: attempt.specification().limits,
            handle: self.handle.clone(),
            profile,
            tabs,
            part: None,
            lead: false,
            extends: true,
            views: Vec::new(),
            typing_held: Vec::new(),
            typing_allowed: Vec::new(),
        };
        if !driver.tabs.is_empty() {
            driver.report(WorkAgentDiagnostic::TabsListed {
                count: u8::try_from(driver.tabs.len()).unwrap_or(u8::MAX),
            });
        }
        let (files, refused) = crate::work_files::WorkFileGrant::admit(&driver.grant.folders);
        for folder in refused {
            driver.notice(&format!(
                "Folder not granted: {folder}. It is outside the home folder, protected, or missing."
            ));
        }
        driver.files = (!files.is_empty()).then_some(files);
        driver.inherit(&projection, receipt.execution).await;
        let outcome = driver.drive(&attempt, &providers, &mut browser).await;
        let (status, usage) = match driver.conclude(outcome).await {
            Ok(status) => (status, driver.settled_usage(status)),
            Err(error) => return Err(error),
        };
        let settlement = attempt
            .settle_owned(WorkAdapterResult {
                status,
                usage,
                artifacts: vec![],
                intervention: driver
                    .intervention
                    .take()
                    .filter(|_| status != WorkAttemptStatus::Succeeded),
            })
            .await?;
        if settlement.profile() != profile
            || settlement.work() != work
            || settlement.execution() != receipt.execution
            || settlement.attempt() != original
        {
            return Err(WorkError::Invalid);
        }
        Ok(settlement.into_projection())
    }
}

struct Driver {
    probe: WorkAttemptProbe,
    grant: WorkAgentGrantV1,
    limits: WorkExecutionLimits,
    output: String,
    objective: String,
    decisions: Vec<planning::PlanningAnswer>,
    bodies: Vec<context::WorkContextBody>,
    /// Earlier executions of this work: their cards stay on the canvas and
    /// their sources stay citable.
    inherited: Vec<WorkArtifactV1>,
    /// Inherited objects the request names or the last run produced: never shed.
    kept: Vec<WorkArtifactId>,
    thread: Vec<WorkAgentThreadEntry>,
    /// Consecutive cancellation polls the store could not answer.
    unreadable_polls: std::sync::atomic::AtomicU8,
    /// Why the run was told to stop, once it was.
    stopped: std::sync::Mutex<Option<crate::work_runtime::WorkCancelCause>>,
    /// Waiting for the person: the deadline does not run meanwhile.
    waiting: bool,
    /// Folders the person granted, once admitted by policy.
    files: Option<crate::work_files::WorkFileGrant>,
    private: Vec<String>,
    previews: Vec<WorkEvidencePreviewV1>,
    used: WorkUsage,
    steps: u32,
    turn: u8,
    failed_turns: u8,
    intervention: Option<WorkInterventionV1>,
    diagnostic: Option<fn(WorkAgentDiagnostic)>,
    /// Closed feedback for the next turn: what was refused and why.
    notices: Vec<String>,
    published: usize,
    finish_refusals: u8,
    pending_output_repair: bool,
    /// Which session each site's pages open in, decided once per run.
    sites: crate::work_sites::RunSites,
    /// Steps worked in the person's session: their facts never ride a search.
    session_steps: Vec<WorkStepId>,
    /// Each page step's site.
    page_sites: Vec<(WorkStepId, String)>,
    /// Text from the person's own pages, by site, for a held step's provenance.
    private_sites: Vec<(String, String)>,
    /// The limits the run started with: one Keep going adds them again.
    base_limits: WorkExecutionLimits,
    handle: crate::Handle,
    profile: ProfileId,
    /// The person's open tabs, listed with their consent.
    tabs: Vec<context::WorkContextTabV1>,
    /// A lead run's part this driver works for: its steps and records carry it.
    part: Option<WorkPartId>,
    /// A lead run: searches keep their record and place no sources object.
    lead: bool,
    /// Whether this driver may ask the person to grow the run's budget.
    extends: bool,
    /// Page tasks that only read what a daily app's view lists, by start page.
    views: Vec<String>,
    /// Sites whose page tasks hold every field typed into for the person.
    typing_held: Vec<String>,
    /// Sites the person allowed typing on for the run, for the run to learn.
    typing_allowed: Vec<String>,
}

enum Fetched {
    Search(WorkStepId, WorkSearchOutcomeOwned),
    Browse(
        WorkStepId,
        Result<WorkBrowserOutcome, WorkError>,
        Option<WorkUsage>,
    ),
}
struct WorkSearchOutcomeOwned {
    status: WorkAttemptStatus,
    usage: Option<WorkUsage>,
    note: Option<&'static str>,
    record: Option<WorkProviderSearchRecordV1>,
    /// Why the search could not be run at all, for the diagnostic.
    error: Option<WorkError>,
}

impl Driver {
    fn remaining(&self) -> WorkExecutionLimits {
        WorkExecutionLimits {
            model_tokens: self
                .limits
                .model_tokens
                .saturating_sub(self.used.model_tokens)
                .max(1),
            cost_micro_usd: self
                .limits
                .cost_micro_usd
                .saturating_sub(self.used.cost_micro_usd)
                .max(1),
            operations: self
                .limits
                .operations
                .saturating_sub(self.used.operations)
                .max(1),
            ..self.limits
        }
    }
    fn charge(&mut self, usage: WorkUsage) {
        self.used.model_tokens = self.used.model_tokens.saturating_add(usage.model_tokens);
        self.used.cost_micro_usd = self
            .used
            .cost_micro_usd
            .saturating_add(usage.cost_micro_usd);
        self.used.operations = self.used.operations.saturating_add(usage.operations.max(1));
        if usage.accounting == WorkUsageAccounting::ConservativeReservation {
            self.used.accounting = WorkUsageAccounting::ConservativeReservation;
        }
    }
    fn settled_usage(&self, status: WorkAttemptStatus) -> Option<WorkUsage> {
        if status == WorkAttemptStatus::OutcomeUnknown {
            return None;
        }
        let mut usage = self.used;
        usage.operations = usage.operations.max(self.steps);
        if !usage.within(self.limits) {
            usage = WorkUsage {
                model_tokens: self.limits.model_tokens,
                cost_micro_usd: self.limits.cost_micro_usd,
                operations: self.limits.operations,
                accounting: WorkUsageAccounting::ConservativeReservation,
            };
        }
        Some(usage)
    }
    fn budget_left(&self) -> bool {
        self.turn < self.grant.max_turns
            && self.steps + 2 <= u32::from(self.grant.max_steps)
            && self.remaining().model_tokens >= TURN_TOKEN_FLOOR
            && self.remaining().cost_micro_usd > 1
    }
    fn step(&self, kind: WorkStepKindV1, status: WorkStepStatus) -> WorkStepFact {
        WorkStepFact {
            part: self.part,
            id: WorkStepId::generate(),
            turn: self.turn.max(1),
            kind,
            status,
            usage: None,
            artifacts: vec![],
            evidence: None,
            note: None,
            measurements: None,
            local: None,
            account: None,
        }
    }
    async fn begin(
        &mut self,
        step: WorkStepFact,
        artifacts: Vec<WorkArtifactV1>,
        evidence: Option<WorkProviderSearchRecordV1>,
    ) -> Result<WorkStepId, WorkError> {
        let id = step.id;
        let kind = step_kind_label(&step.kind);
        if let Err(error) = self
            .probe
            .commit_step(WorkRuntimeUpdate::BeginStep {
                execution: self.probe.execution(),
                attempt: self.probe.attempt(),
                step,
                artifacts,
                evidence: evidence.map(Box::new),
                file: None,
            })
            .await
        {
            self.report(WorkAgentDiagnostic::CommitRefused { kind, error });
            return Err(error);
        }
        self.steps += 1;
        Ok(id)
    }
    /// Notices reach the model next turn: at most eight, each within the
    /// disclosure's limit, never repeated.
    fn notice(&mut self, text: &str) {
        let text = zephium_core::work::agent::clip_text(text, 512);
        if self.notices.contains(&text) {
            return;
        }
        if self.notices.len() == 8 {
            self.notices.remove(0);
        }
        self.notices.push(text);
    }
    /// Settles a file step with what it disclosed, or with why it failed.
    async fn settle_file(
        &mut self,
        step: WorkStepId,
        status: WorkStepStatus,
        file: Option<WorkFileRecordV1>,
        note: Option<String>,
    ) -> Result<(), WorkError> {
        if let Err(error) = self
            .probe
            .commit_step(WorkRuntimeUpdate::SettleStep {
                execution: self.probe.execution(),
                attempt: self.probe.attempt(),
                step,
                status,
                usage: None,
                artifacts: vec![],
                evidence: None,
                file: file.map(Box::new),
                note,
                measurements: None,
            })
            .await
        {
            self.report(WorkAgentDiagnostic::CommitRefused {
                kind: "settle_file",
                error,
            });
            return Err(error);
        }
        Ok(())
    }
    fn report(&self, event: WorkAgentDiagnostic) {
        if let Some(diagnostic) = self.diagnostic {
            diagnostic(event);
        }
    }
    fn stop_cause(&self) -> Option<crate::work_runtime::WorkCancelCause> {
        self.stopped.lock().ok().and_then(|stopped| *stopped)
    }
    /// Why a step ended early, in the person's words, once the run was told to stop.
    fn stop_note(&self) -> Option<&'static str> {
        use crate::work_runtime::WorkCancelCause;
        self.stop_cause().map(|cause| match cause {
            WorkCancelCause::Deadline => OUT_OF_TIME,
            WorkCancelCause::Requested => STOPPED,
            _ => INTERRUPTED,
        })
    }
    /// Why a step that ended early did, asking the store once when the run
    /// has not yet heard that it was told to stop.
    async fn why_stopped(&self) -> Option<&'static str> {
        if self.stop_cause().is_none() {
            self.cancelled().await;
        }
        self.stop_note()
    }
    /// Every run that does not succeed says why, and leaves no step running:
    /// the store refuses to settle an attempt over a running step.
    async fn conclude(
        &mut self,
        outcome: Result<WorkAttemptStatus, WorkError>,
    ) -> Result<WorkAttemptStatus, WorkError> {
        let (mut status, broken) = match outcome {
            Ok(WorkAttemptStatus::Succeeded) => return outcome,
            Ok(status) => (status, None),
            Err(WorkError::OutcomeUnknown) => (WorkAttemptStatus::OutcomeUnknown, None),
            Err(error) => {
                self.report(WorkAgentDiagnostic::LoopFailed { error });
                (WorkAttemptStatus::Failed, Some(error))
            }
        };
        let out_of_time = self.stop_cause() == Some(crate::work_runtime::WorkCancelCause::Deadline);
        let note = self.stop_note().unwrap_or(match (broken, status) {
            (Some(WorkError::Capacity), _) => CONTEXT_FULL,
            (Some(_), _) => RUN_BROKEN,
            (None, WorkAttemptStatus::OutcomeUnknown) => ANSWER_LOST,
            _ => INTERRUPTED,
        });
        let running = match self.probe.runtime_projection().await {
            Ok(state) => state
                .executions
                .into_iter()
                .find(|execution| execution.id == self.probe.execution())
                .map(|execution| {
                    execution
                        .steps
                        .into_iter()
                        .filter(|step| step.status == WorkStepStatus::Running)
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default(),
            Err(error) => return Err(broken.unwrap_or(error)),
        };
        let swept = !running.is_empty();
        for step in running {
            // A fetch cut off mid-flight may have been charged: its outcome is unknown.
            let settled = match step.kind {
                WorkStepKindV1::Search { .. }
                | WorkStepKindV1::Read { .. }
                | WorkStepKindV1::Discover { .. } => WorkStepStatus::OutcomeUnknown,
                _ => WorkStepStatus::Cancelled,
            };
            if self
                .settle(
                    step.id,
                    settled,
                    None,
                    vec![],
                    None,
                    Some(note.into()),
                    None,
                )
                .await
                .is_err()
            {
                return Err(broken.unwrap_or(WorkError::Conflict));
            }
            if settled == WorkStepStatus::OutcomeUnknown && !out_of_time {
                status = WorkAttemptStatus::OutcomeUnknown;
            }
        }
        if out_of_time && status != WorkAttemptStatus::OutcomeUnknown {
            status = WorkAttemptStatus::Failed;
        }
        if !swept && (out_of_time || broken.is_some()) {
            self.fail_turn(note).await?;
        }
        Ok(status)
    }
    /// Ends the run on a failed turn step that says why, when the grant still
    /// has room for the step; the attempt fails either way.
    async fn fail_turn(&mut self, note: &str) -> Result<WorkAttemptStatus, WorkError> {
        if self.steps < u32::from(self.grant.max_steps) {
            let mut step = self.step(WorkStepKindV1::Turn, WorkStepStatus::Failed);
            step.usage = Some(WorkUsage::default());
            step.note = Some(note.to_owned());
            let _ = self.begin(step, vec![], None).await;
        }
        Ok(WorkAttemptStatus::Failed)
    }
    #[allow(clippy::too_many_arguments)]
    async fn settle(
        &mut self,
        step: WorkStepId,
        status: WorkStepStatus,
        usage: Option<WorkUsage>,
        artifacts: Vec<WorkArtifactV1>,
        evidence: Option<WorkProviderSearchRecordV1>,
        note: Option<String>,
        measurements: Option<WorkStepMeasurementsV1>,
    ) -> Result<(), WorkError> {
        if let Err(error) = self
            .probe
            .commit_step(WorkRuntimeUpdate::SettleStep {
                execution: self.probe.execution(),
                attempt: self.probe.attempt(),
                step,
                status,
                usage,
                artifacts,
                evidence: evidence.map(Box::new),
                file: None,
                note,
                measurements,
            })
            .await
        {
            self.report(WorkAgentDiagnostic::CommitRefused {
                kind: "settle",
                error,
            });
            return Err(error);
        }
        Ok(())
    }
    async fn cancelled(&self) -> bool {
        use std::sync::atomic::Ordering;
        // One poll the store could not answer is not a stop; a run only ends
        // as unreadable when the answer stays out of reach.
        let cause = match self.probe.cancellation_cause().await {
            Ok(Some(cause)) => Some(cause),
            Ok(None) if !self.waiting && Instant::now() >= self.probe.deadline() => {
                Some(crate::work_runtime::WorkCancelCause::Deadline)
            }
            Ok(None) => {
                self.unreadable_polls.store(0, Ordering::Relaxed);
                None
            }
            Err(_) => {
                let polls = self.unreadable_polls.fetch_add(1, Ordering::Relaxed) + 1;
                (polls >= MAX_UNREADABLE_POLLS)
                    .then_some(crate::work_runtime::WorkCancelCause::Unreadable)
            }
        };
        if let Some(cause) = cause {
            self.report(WorkAgentDiagnostic::Stopped { cause });
            if let Ok(mut stopped) = self.stopped.lock() {
                stopped.get_or_insert(cause);
            }
        }
        cause.is_some()
    }

    async fn drive<B, Fut>(
        &mut self,
        attempt: &WorkNodeAttempt,
        providers: &WorkAgentProviders<'_>,
        browser: &mut B,
    ) -> Result<WorkAttemptStatus, WorkError>
    where
        B: FnMut(WorkAttemptProbe, WorkAgentBrowseRequest) -> Fut,
        Fut: Future<Output = Result<WorkBrowserOutcome, WorkError>>,
    {
        loop {
            if self.cancelled().await {
                return Ok(WorkAttemptStatus::Cancelled);
            }
            if !self.budget_left() {
                if self.turn < self.grant.max_turns
                    && self.steps + 3 <= u32::from(self.grant.max_steps)
                {
                    match self.keep_going().await? {
                        Ok(true) => continue,
                        Ok(false) => {}
                        Err(status) => return Ok(status),
                    }
                }
                self.report(WorkAgentDiagnostic::BudgetSpent);
                return self.fail_turn(BUDGET_SPENT).await;
            }
            self.turn += 1;
            self.probe.record_activity(WorkActivityV1::Planning);
            let state = self.probe.runtime_projection().await?;
            let execution = state
                .executions
                .iter()
                .find(|e| e.id == self.probe.execution())
                .ok_or(WorkError::NotFound)?;
            // Steps the person appended (steers) count against the grant too.
            self.steps = self
                .steps
                .max(u32::try_from(execution.steps.len()).unwrap_or(u32::MAX));
            let budget = WorkAgentBudget {
                turns_left: self.grant.max_turns.saturating_sub(self.turn),
                steps_left: u8::try_from(
                    u32::from(self.grant.max_steps).saturating_sub(self.steps + 2),
                )
                .unwrap_or(u8::MAX),
                browse_available: true,
            };
            let notices = std::mem::take(&mut self.notices);
            let sites: Vec<WorkAgentSiteView> = self
                .sites
                .view()
                .into_iter()
                .rev()
                .take(MAX_AGENT_SITES)
                .map(|(site, session)| WorkAgentSiteView { site, session })
                .collect();
            let view = TurnView {
                objective: &self.objective,
                decisions: &self.decisions,
                bodies: &self.bodies,
                steps: &execution.steps,
                current: &execution.artifacts,
                budget,
                remaining: self.remaining(),
                notices: &notices,
                sites: &sites,
                tabs: &self.tabs,
            };
            let disclosed = disclose(
                &view,
                &mut self.previews,
                &mut self.inherited,
                &self.kept,
                &mut self.thread,
            );
            let disclosure = match disclosed {
                Ok(disclosure) => disclosure,
                Err(error) => {
                    self.report(WorkAgentDiagnostic::DisclosureRefused { error });
                    return self
                        .fail_turn(if error == WorkError::Capacity {
                            CONTEXT_FULL
                        } else {
                            TURN_UNPREPARED
                        })
                        .await;
                }
            };
            let trace = WorkSynthesisTrace {
                work: self.probe.work(),
                execution: self.probe.execution(),
                attempt: self.probe.attempt(),
            };
            let result = tokio::time::timeout_at(
                self.probe.deadline().into(),
                providers.turn.turn(&disclosure, trace),
            )
            .await;
            let mut reported = false;
            let mut failed_note = "The model did not answer";
            let (turn, usage) = match result {
                Ok(Ok(result)) => {
                    if !result.usage.within(self.remaining()) {
                        let mut step =
                            self.step(WorkStepKindV1::Turn, WorkStepStatus::OutcomeUnknown);
                        step.note = Some(OVER_BUDGET.into());
                        let _ = self.begin(step, vec![], None).await;
                        return Err(WorkError::OutcomeUnknown);
                    }
                    self.charge(result.usage);
                    let turn = match disclosure.resolve(result.output) {
                        Ok(turn) => Some(turn),
                        Err(refusal) => {
                            self.notice(refusal.notice());
                            self.report(WorkAgentDiagnostic::TurnRefused {
                                turn: self.turn,
                                reason: Some(refusal),
                            });
                            reported = true;
                            failed_note = match refusal {
                                WorkAgentTurnRefusal::Empty => "The agent did nothing this turn",
                                WorkAgentTurnRefusal::Oversized => {
                                    "The agent's turn was too large to use"
                                }
                            };
                            None
                        }
                    };
                    (turn, result.usage)
                }
                Ok(Err(WorkSynthesisError::NotDispatched(error))) => {
                    failed_note = match error {
                        WorkError::Capacity => "The request grew too large for the model",
                        _ => "The model could not be reached",
                    };
                    (None, WorkUsage::default())
                }
                Ok(Err(WorkSynthesisError::Rejected(usage))) => {
                    failed_note = "The model's turn could not be used";
                    self.charge(usage);
                    (None, usage)
                }
                Ok(Err(WorkSynthesisError::Stalled(usage))) => {
                    self.charge(usage);
                    (None, usage)
                }
                Ok(Err(WorkSynthesisError::OutcomeUnknown)) | Err(_) => {
                    let mut step = self.step(WorkStepKindV1::Turn, WorkStepStatus::OutcomeUnknown);
                    step.note = Some(
                        if result.is_err() {
                            OUT_OF_TIME
                        } else {
                            ANSWER_LOST
                        }
                        .into(),
                    );
                    let _ = self.begin(step, vec![], None).await;
                    return Err(WorkError::OutcomeUnknown);
                }
            };
            let mut record = self.step(
                WorkStepKindV1::Turn,
                if turn.is_some() {
                    WorkStepStatus::Succeeded
                } else {
                    WorkStepStatus::Failed
                },
            );
            record.usage = Some(usage);
            record.note = match &turn {
                Some(turn) if turn.finish => None,
                Some(turn) => turn.say.clone(),
                None => Some(failed_note.to_owned()),
            };
            self.begin(record, vec![], None).await?;
            match &turn {
                Some(turn) => self.report(WorkAgentDiagnostic::TurnAdmitted {
                    turn: self.turn,
                    artifacts: turn.artifacts.len(),
                    dropped: turn.dropped,
                    fetches: turn.fetch.len(),
                    asks: turn.ask.is_some(),
                    finish: turn.finish,
                }),
                None => {
                    if !reported {
                        self.report(WorkAgentDiagnostic::TurnRefused {
                            turn: self.turn,
                            reason: None,
                        });
                    }
                }
            }
            let Some(mut turn) = turn else {
                self.failed_turns += 1;
                if self.failed_turns >= MAX_FAILED_TURNS {
                    return Ok(WorkAttemptStatus::Failed);
                }
                continue;
            };
            for notice in std::mem::take(&mut turn.notices) {
                self.notice(&notice);
            }
            // A turn that only proposed refused objects is idle: the refusal
            // notices go back, but idle turns end the run like refused ones.
            let idle = !turn.finish
                && turn.ask.is_none()
                && turn.fetch.is_empty()
                && turn.artifacts.is_empty();
            if idle {
                self.failed_turns += 1;
            } else {
                self.failed_turns = 0;
            }
            let proposed_artifacts = turn.artifacts.len() + turn.dropped;
            let mut published_this_turn = 0;
            let mut repeated = false;
            turn.fetch.retain(|kind| {
                if execution
                    .steps
                    .iter()
                    .any(|step| reuses_completed_read(kind, step))
                {
                    repeated = true;
                    false
                } else {
                    true
                }
            });
            if repeated {
                self.notice("Repeated read skipped: this page and record schema already produced the cited canvas results shown in artifacts. Use those results, follow an observed source link_destination for missing details, or finish. No new browser work was dispatched.");
            }
            for refusal in &turn.refusals {
                self.report(WorkAgentDiagnostic::ArtifactRefused {
                    turn: self.turn,
                    reason: *refusal,
                });
                self.notice(&format!(
                    "A proposed object was refused last turn: it {}.",
                    refusal.notice()
                ));
            }
            if idle && self.failed_turns >= MAX_FAILED_TURNS {
                return self.fail_turn(NO_PROGRESS).await;
            }
            if !turn.artifacts.is_empty() {
                self.probe
                    .record_activity(WorkActivityV1::ProducingArtifact);
                let headroom = MAX_WORK_ARTIFACTS.saturating_sub(execution.artifacts.len());
                let proposed = turn.artifacts.len();
                let artifacts: Vec<WorkArtifactV1> = turn
                    .artifacts
                    .into_iter()
                    .take(headroom)
                    .filter_map(|artifact| {
                        attempt
                            .mint_marked_artifact(
                                WorkArtifactDraft {
                                    output: self.output.clone(),
                                    title: artifact.title,
                                    data: artifact.data,
                                    evidence: artifact.evidence,
                                },
                                artifact.general_knowledge,
                            )
                            .ok()
                    })
                    .collect();
                if artifacts.len() < proposed {
                    self.report(WorkAgentDiagnostic::ArtifactsDropped {
                        kind: "publish",
                        count: proposed - artifacts.len(),
                    });
                    self.notice(if proposed > headroom {
                        "The canvas of this run is full: objects beyond its limit were not placed. Finish with what is placed."
                    } else {
                        "A proposed object could not be placed: it failed validation. Repair it from the listed sources."
                    });
                }
                if !artifacts.is_empty() {
                    let mut step = self.step(WorkStepKindV1::Publish, WorkStepStatus::Succeeded);
                    step.artifacts = artifacts.iter().map(|a| a.id).collect();
                    step.note = Some(publish_note(&artifacts));
                    self.published += artifacts.len();
                    published_this_turn = artifacts.len();
                    self.begin(step, artifacts, None).await?;
                }
            }
            if !turn.fetch.is_empty() {
                let leaked = turn.fetch.iter().any(|kind| match kind {
                    WorkStepKindV1::Search { query } | WorkStepKindV1::Discover { query, .. } => {
                        query_discloses(query, &self.private)
                    }
                    _ => false,
                });
                if leaked {
                    self.failed_turns += 1;
                    self.notice("A search query repeated private context and was not sent. Rephrase the query without that text.");
                    if self.failed_turns >= MAX_FAILED_TURNS {
                        return self.fail_turn(PRIVATE_QUERY).await;
                    }
                    continue;
                }
                let status = self
                    .fetch(attempt, providers.search, browser, turn.fetch)
                    .await?;
                if let Some(status) = status {
                    return Ok(status);
                }
            }
            if let Some(question) = turn.ask {
                let status = self.ask(question).await?;
                if let Some(status) = status {
                    return Ok(status);
                }
            }
            if proposed_artifacts > 0 {
                self.pending_output_repair = published_this_turn < proposed_artifacts;
            }
            if turn.finish && (self.published == 0 || self.pending_output_repair) {
                if self.finish_refusals >= 2 {
                    return self.fail_turn(UNPUBLISHED).await;
                }
                self.finish_refusals += 1;
                self.notice("Finish was refused: the requested result has not been published successfully. Repair the refused object using the existing sources and publish it before finishing. Earlier partial results do not replace that object.");
                continue;
            }
            if turn.finish {
                self.probe.record_activity(WorkActivityV1::Finishing);
                let mut step = self.step(
                    WorkStepKindV1::Finish {
                        followups: turn.followups,
                        title: None,
                    },
                    WorkStepStatus::Succeeded,
                );
                step.note = turn.say;
                self.begin(step, vec![], None).await?;
                return Ok(WorkAttemptStatus::Succeeded);
            }
        }
    }

    /// Searches and reads each use bounded batches after their prerequisites.
    /// Returns a terminal attempt status when the run cannot continue.
    async fn fetch<B, Fut>(
        &mut self,
        attempt: &WorkNodeAttempt,
        search: &dyn WorkPublicSearchProvider,
        browser: &mut B,
        fetches: Vec<WorkStepKindV1>,
    ) -> Result<Option<WorkAttemptStatus>, WorkError>
    where
        B: FnMut(WorkAttemptProbe, WorkAgentBrowseRequest) -> Fut,
        Fut: Future<Output = Result<WorkBrowserOutcome, WorkError>>,
    {
        let cap = usize::from(self.limits.max_workers).max(1);
        let (searches, browses): (Vec<_>, Vec<_>) = fetches
            .into_iter()
            .partition(|kind| matches!(kind, WorkStepKindV1::Search { .. }));
        let mut offset = 0;
        while offset < searches.len() {
            let steps = usize::from(self.grant.max_steps).saturating_sub(self.steps as usize + 1);
            if steps == 0 {
                self.notice(STEPS_EXHAUSTED);
                break;
            }
            let Some(remaining) = reads::remaining_limits(self.limits, self.used) else {
                self.notice(BUDGET_EXHAUSTED);
                break;
            };
            let mut count = cap
                .min(searches.len() - offset)
                .min(steps)
                .min(remaining.model_tokens as usize)
                .min(remaining.cost_micro_usd as usize)
                .min(remaining.operations as usize);
            let shares =
                loop {
                    let Some(shares) = reads::budget_shares(self.limits, self.used, count) else {
                        break None;
                    };
                    let fits = searches[offset..offset + count].iter().zip(&shares).all(
                        |(kind, limits)| {
                            let WorkStepKindV1::Search { query } = kind else {
                                return false;
                            };
                            let scope = WorkPublicSearchScope {
                                provider: self.grant.provider,
                                model: self.grant.model.clone(),
                                query: query.clone(),
                            };
                            search
                                .minimum_reservation(&scope, &[])
                                .is_none_or(|usage| usage.within(*limits))
                        },
                    );
                    if fits || count == 1 {
                        break Some(shares);
                    }
                    count -= 1;
                };
            let Some(shares) = shares else {
                self.notice(BUDGET_EXHAUSTED);
                break;
            };
            let batch = &searches[offset..offset + count];
            let mut running = Vec::new();
            for (kind, limits) in batch.iter().zip(shares) {
                let WorkStepKindV1::Search { query } = kind else {
                    continue;
                };
                let step = self.step(kind.clone(), WorkStepStatus::Running);
                let id = self.begin(step, vec![], None).await?;
                running.push((
                    id,
                    WorkPublicSearchScope {
                        provider: self.grant.provider,
                        model: self.grant.model.clone(),
                        query: query.clone(),
                    },
                    limits,
                ));
            }
            let futures: Vec<Pin<Box<dyn Future<Output = Fetched> + Send + '_>>> = running
                .into_iter()
                .map(|(id, scope, limits)| {
                    let probe = self.probe.clone();
                    Box::pin(async move {
                        let outcome = probe.run_search(search, &scope, &[], limits).await;
                        Fetched::Search(
                            id,
                            match outcome {
                                Ok(outcome) => WorkSearchOutcomeOwned {
                                    status: outcome.status,
                                    usage: outcome.usage,
                                    note: outcome.note,
                                    record: outcome.record,
                                    error: None,
                                },
                                // Nothing was sent: a failed step, not a lost one.
                                Err(error) => {
                                    crate::work_trace::record(format_args!(
                                        "work: phase=search refused=not_run cause={error:?}"
                                    ));
                                    WorkSearchOutcomeOwned {
                                        status: WorkAttemptStatus::Failed,
                                        usage: Some(WorkUsage::default()),
                                        note: Some("The search could not be run"),
                                        record: None,
                                        error: Some(error),
                                    }
                                }
                            },
                        )
                    }) as Pin<Box<dyn Future<Output = Fetched> + Send + '_>>
                })
                .collect();
            let mut terminal = None;
            for fetched in join_all(futures).await {
                terminal = terminal.or(self.settle_fetched(attempt, fetched).await?);
            }
            if terminal.is_some() {
                return Ok(terminal);
            }
            offset += count;
        }
        let (file_steps, browses): (Vec<_>, Vec<_>) = browses
            .into_iter()
            .partition(|kind| kind.files() || matches!(kind, WorkStepKindV1::RunCommand { .. }));
        for kind in file_steps {
            if let Some(terminal) = self.file_step(kind).await? {
                return Ok(Some(terminal));
            }
        }
        self.fetch_browses(attempt, browser, browses).await
    }

    /// One step on the person's files. Reads settle at once with a record
    /// the agent can cite; a proposed change waits for the person's decision.
    async fn file_step(
        &mut self,
        kind: WorkStepKindV1,
    ) -> Result<Option<WorkAttemptStatus>, WorkError> {
        let Some(files) = self.files.clone() else {
            self.notice("No folder is granted in this run. Ask the person to add a folder to the canvas before reading or changing files.");
            return Ok(None);
        };
        if self.steps + 2 > u32::from(self.grant.max_steps) {
            self.notice(STEPS_EXHAUSTED);
            return Ok(None);
        }
        if matches!(kind, WorkStepKindV1::RunCommand { .. }) {
            return self.command_step(kind, &files).await;
        }
        let mut step = self.step(kind.clone(), WorkStepStatus::Running);
        let prepared = if kind.proposes_write() {
            let path = match &kind {
                WorkStepKindV1::MoveFile { from, .. } => from,
                WorkStepKindV1::WriteFile { path, .. }
                | WorkStepKindV1::EditFile { path, .. }
                | WorkStepKindV1::DeleteFile { path, .. } => path,
                _ => unreachable!(),
            };
            let resolved = files
                .resolve(path, matches!(kind, WorkStepKindV1::WriteFile { .. }))
                .ok();
            let state = self.probe.runtime_projection().await?;
            let known = state.executions.iter().find(|e| e.id == self.probe.execution()).and_then(|execution| {
                execution.file_evidence.iter().rev().find_map(|record| {
                    if resolved.as_ref().is_some_and(|p|p.to_string_lossy()==record.file.path) && !record.file.digest.is_empty() {
                        Some((record.file.kind != WorkFileKindV1::Deleted).then_some(record.file.digest.as_str()))
                    } else if execution.steps.iter().any(|step| step.evidence == Some(record.id) && matches!(&step.kind, WorkStepKindV1::MoveFile { from, .. } if from == path)) {
                        Some(None)
                    } else { None }
                }).flatten()
            });
            Some(files.prepare(&kind, known))
        } else {
            None
        };
        if let Some(Ok(change)) = &prepared {
            step.local = Some(Box::new(change.fact()));
        }
        let id = self.begin(step, vec![], None).await?;
        let outcome = match (&kind, prepared) {
            (_, Some(Ok(change))) => return self.await_decision(id, &files, change).await,
            (_, Some(Err(error))) => Err(error),
            (WorkStepKindV1::List { path, depth }, _) => {
                self.probe.record_activity(WorkActivityV1::Reading);
                files.list_at(path, depth.unwrap_or(1))
            }
            (
                WorkStepKindV1::ReadFile {
                    path,
                    offset,
                    limit,
                },
                _,
            ) => {
                self.probe.record_activity(WorkActivityV1::Reading);
                files.read_at(path, offset.unwrap_or(1), limit.unwrap_or(200))
            }
            (
                WorkStepKindV1::SearchFiles {
                    path,
                    query,
                    glob,
                    regex,
                },
                _,
            ) => {
                self.probe.record_activity(WorkActivityV1::Searching);
                files.search_with(path, query, glob.as_deref(), regex.unwrap_or(false))
            }
            _ => return Ok(None),
        };
        self.settle_file_outcome(id, outcome).await?;
        Ok(None)
    }
    async fn settle_file_outcome(
        &mut self,
        id: WorkStepId,
        outcome: Result<WorkFileEvidenceV1, crate::work_files::WorkFileError>,
    ) -> Result<(), WorkError> {
        match outcome {
            Ok(file) => {
                let record = WorkFileRecordV1 {
                    id: WorkArtifactId::generate(),
                    node: self.probe.node(),
                    attempt: self.probe.attempt(),
                    file,
                };
                self.keep_file_preview(&record);
                let note = Some(match record.file.kind {
                    WorkFileKindV1::Directory => format!("{} entries", record.file.bytes),
                    WorkFileKindV1::Search => format!("{} hits", record.file.bytes),
                    WorkFileKindV1::Written | WorkFileKindV1::Moved | WorkFileKindV1::Deleted => {
                        "Applied".to_owned()
                    }
                    WorkFileKindV1::Text | WorkFileKindV1::Binary => {
                        format!("{} bytes", record.file.bytes)
                    }
                });
                self.settle_file(id, WorkStepStatus::Succeeded, Some(record), note)
                    .await
            }
            Err(error) => {
                self.settle_file(
                    id,
                    WorkStepStatus::Failed,
                    None,
                    Some(error.note().to_owned()),
                )
                .await
            }
        }
    }
    /// Waits for the person's decision on a proposed change, then applies it.
    async fn await_decision(
        &mut self,
        id: WorkStepId,
        files: &crate::work_files::WorkFileGrant,
        change: crate::work_files::PreparedChange,
    ) -> Result<Option<WorkAttemptStatus>, WorkError> {
        self.probe.record_activity(WorkActivityV1::WaitingForHuman);
        let since = self.wait();
        loop {
            tokio::time::sleep(ASK_POLL).await;
            let state = self.probe.runtime_projection().await?;
            let execution = state
                .executions
                .iter()
                .find(|e| e.id == self.probe.execution())
                .ok_or(WorkError::NotFound)?;
            let proposed = execution
                .steps
                .iter()
                .find(|s| s.id == id)
                .ok_or(WorkError::NotFound)?;
            let decision = proposed.kind.file_decision();
            if decision.is_some() {
                self.resume(since);
            }
            match decision {
                Some(true) => {
                    let outcome = files.apply(change);
                    self.settle_file_outcome(id, outcome).await?;
                    return Ok(None);
                }
                Some(false) => {
                    self.settle_file(
                        id,
                        WorkStepStatus::Failed,
                        None,
                        Some("Declined by the person".into()),
                    )
                    .await?;
                    return Ok(None);
                }
                None => {}
            }
            if since.elapsed() >= WAIT_PATIENCE {
                self.suspend().await;
                self.settle_file(
                    id,
                    WorkStepStatus::Cancelled,
                    None,
                    Some(DECISION_SUSPENDED.into()),
                )
                .await?;
                return Ok(Some(WorkAttemptStatus::Cancelled));
            }
            if self.cancelled().await {
                let note = self.stop_note().unwrap_or(INTERRUPTED);
                self.settle_file(id, WorkStepStatus::Cancelled, None, Some(note.into()))
                    .await?;
                return Ok(Some(WorkAttemptStatus::Cancelled));
            }
        }
    }
    /// Starts waiting for the person; the deadline stands still until `resume`.
    fn wait(&mut self) -> Instant {
        self.waiting = true;
        Instant::now()
    }
    /// The person answered: the run gets back the time it waited.
    fn resume(&mut self, since: Instant) {
        self.waiting = false;
        self.probe.resume_after_wait(since.elapsed());
    }
    /// Nobody answered in time: the run stops on the open question through
    /// the same cancel request a person's Stop sends, so it reads as stopped.
    async fn suspend(&mut self) {
        self.report(WorkAgentDiagnostic::AskSuspended);
        if let Err(error) = self.probe.request_stop().await {
            self.report(WorkAgentDiagnostic::CommitRefused {
                kind: "suspend",
                error,
            });
        }
    }
    /// The disclosed excerpt is capped below the record so the turn stays
    /// within its preview bounds.
    fn keep_file_preview(&mut self, record: &WorkFileRecordV1) {
        const PREVIEW_BYTES: usize = 8192;
        let file = &record.file;
        let folder = std::path::Path::new(&file.path)
            .parent()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        let mut end = file.text.len().min(PREVIEW_BYTES);
        while !file.text.is_char_boundary(end) {
            end -= 1;
        }
        self.keep_preview(WorkEvidencePreviewV1 {
            link_destination: None,
            version: 1,
            link: WorkEvidenceLink {
                extraction_id: record.id,
                source_id: 1,
            },
            origin: format!("file://{folder}"),
            role: "file".into(),
            truncated: file.truncated || end < file.text.len(),
            text: file.text[..end].to_owned(),
            source_bytes: file.bytes.to_string(),
            source: WorkEvidenceSourceV1::File {
                path: file.path.clone(),
                name: file.name.clone(),
                file_kind: file.kind,
            },
        });
    }

    /// Commits one fetched outcome. Returns a terminal attempt status when the
    /// run cannot continue (unknown provider outcome, intervention).
    async fn settle_fetched(
        &mut self,
        attempt: &WorkNodeAttempt,
        fetched: Fetched,
    ) -> Result<Option<WorkAttemptStatus>, WorkError> {
        let mut terminal = None;
        if let Fetched::Browse(_, _, Some(prior)) = &fetched {
            self.charge(*prior);
        }
        {
            match fetched {
                Fetched::Search(id, outcome) => {
                    if let Some(usage) = outcome.usage {
                        self.charge(usage);
                    }
                    match (outcome.status, outcome.record) {
                        (WorkAttemptStatus::Succeeded, Some(record)) => {
                            let artifacts = if record.evidence.citations.is_empty() || self.lead {
                                vec![]
                            } else {
                                match attempt.mint_artifact(sources_draft(&self.output, &record)) {
                                    Ok(artifact) => vec![artifact],
                                    Err(_) => {
                                        self.report(WorkAgentDiagnostic::ArtifactsDropped {
                                            kind: "search",
                                            count: 1,
                                        });
                                        vec![]
                                    }
                                }
                            };
                            self.remember(&record);
                            let note = Some(sources_note(record.evidence.citations.len()));
                            self.settle(
                                id,
                                WorkStepStatus::Succeeded,
                                outcome.usage,
                                artifacts,
                                Some(record),
                                note,
                                None,
                            )
                            .await?;
                        }
                        (WorkAttemptStatus::OutcomeUnknown, _) => {
                            let note = self.why_stopped().await.unwrap_or(ANSWER_LOST);
                            self.settle(
                                id,
                                WorkStepStatus::OutcomeUnknown,
                                None,
                                vec![],
                                None,
                                Some(note.into()),
                                None,
                            )
                            .await?;
                            terminal = Some(WorkAttemptStatus::OutcomeUnknown);
                        }
                        (status, _) => {
                            if let Some(error) = outcome.error {
                                self.report(WorkAgentDiagnostic::StepFailed {
                                    kind: "search",
                                    error,
                                });
                            } else if let Some(note) = outcome.note {
                                self.report(WorkAgentDiagnostic::SearchRefused { note });
                            }
                            if let Some(note) = outcome.note {
                                self.notice(&format!(
                                    "A search failed: {note}. Try one differently worded search or read a listed page."
                                ));
                            }
                            let note = match outcome.note {
                                Some(note) => note,
                                None => self.why_stopped().await.unwrap_or(INTERRUPTED),
                            };
                            self.settle(
                                id,
                                step_status(status),
                                outcome.usage.or(Some(WorkUsage::default())),
                                vec![],
                                None,
                                Some(note.into()),
                                None,
                            )
                            .await?;
                        }
                    }
                }
                Fetched::Browse(id, outcome, prior) => match outcome {
                    Ok(outcome) => {
                        if let Some(usage) = outcome.usage {
                            self.charge(usage);
                        }
                        let signed_in = self.session_steps.contains(&id);
                        if outcome.held_back {
                            self.report(WorkAgentDiagnostic::HeldBack);
                            self.notice(HELD_BACK);
                        }
                        let measurements = outcome.measurements;
                        let usage = if outcome.status == WorkStepStatus::OutcomeUnknown {
                            None
                        } else {
                            reads::combined_usage(prior, outcome.usage)
                        };
                        let drafted = outcome.artifacts.len();
                        let artifacts: Vec<WorkArtifactV1> = outcome
                            .artifacts
                            .into_iter()
                            .filter_map(|draft| attempt.mint_artifact(draft).ok())
                            .map(|artifact| WorkArtifactV1 {
                                part: self.part,
                                ..artifact
                            })
                            .collect();
                        if artifacts.len() < drafted {
                            self.report(WorkAgentDiagnostic::ArtifactsDropped {
                                kind: "read",
                                count: drafted - artifacts.len(),
                            });
                        }
                        let (status, artifacts) = if outcome.status == WorkStepStatus::Succeeded {
                            (WorkStepStatus::Succeeded, artifacts)
                        } else {
                            (outcome.status, vec![])
                        };
                        let note = match (status, outcome.note.clone()) {
                            (WorkStepStatus::Succeeded, Some(note)) if !artifacts.is_empty() => {
                                format!("{} · {note}", read_note(&artifacts))
                            }
                            (WorkStepStatus::Succeeded, _) => read_note(&artifacts),
                            (_, Some(note)) => note,
                            (status, None) => self
                                .why_stopped()
                                .await
                                .unwrap_or(match status {
                                    WorkStepStatus::OutcomeUnknown => ANSWER_LOST,
                                    WorkStepStatus::Cancelled => INTERRUPTED,
                                    _ => "The page gave nothing",
                                })
                                .to_owned(),
                        };
                        if status == WorkStepStatus::Failed {
                            self.notice(&format!(
                                "A page read failed: {}. Use another listed source or finish with what the canvas has; do not reopen that page.",
                                outcome.note.as_deref().unwrap_or("the page gave nothing")
                            ));
                        }
                        // Facts from the person's own pages never ride a search query.
                        if signed_in {
                            self.private.extend(
                                artifacts.iter().map(|artifact| artifact.data.plain_text()),
                            );
                            if let Some((_, site)) =
                                self.page_sites.iter().find(|(step, _)| *step == id)
                            {
                                let site = site.clone();
                                self.private_sites.extend(
                                    artifacts
                                        .iter()
                                        .map(|artifact| (site.clone(), artifact.data.plain_text())),
                                );
                            }
                        }
                        let links = browser_preview_links(&artifacts);
                        let published = artifacts.len();
                        if let Some(measured) = measurements {
                            self.report(WorkAgentDiagnostic::ReadMeasured(measured));
                        }
                        self.settle(id, status, usage, artifacts, None, Some(note), measurements)
                            .await?;
                        self.published += published;
                        for link in links {
                            if self.cancelled().await {
                                break;
                            }
                            let cached = self.previews.iter().find(|p| p.link == link).cloned();
                            let preview = match cached {
                                Some(preview) => Ok(preview),
                                None => self.probe.read_evidence(link).await,
                            };
                            if let Ok(preview) = preview {
                                self.keep_preview(preview);
                            }
                        }
                        if outcome.status == WorkStepStatus::OutcomeUnknown {
                            terminal = Some(WorkAttemptStatus::OutcomeUnknown);
                        }
                        // An anonymous read that needs a person is one failed
                        // page, not the end of the run: its note says why.
                        let _ = outcome.intervention;
                    }
                    Err(WorkError::OutcomeUnknown) => {
                        let note = self.why_stopped().await.unwrap_or(ANSWER_LOST);
                        self.settle(
                            id,
                            WorkStepStatus::OutcomeUnknown,
                            None,
                            vec![],
                            None,
                            Some(note.into()),
                            None,
                        )
                        .await?;
                        terminal = Some(WorkAttemptStatus::OutcomeUnknown);
                    }
                    Err(error) => {
                        self.report(WorkAgentDiagnostic::StepFailed {
                            kind: "read",
                            error,
                        });
                        let note = browse_error_note(error);
                        self.notice(&format!(
                            "A page read failed: {note}. Use another listed source or finish with what the canvas has."
                        ));
                        self.settle(
                            id,
                            WorkStepStatus::Failed,
                            prior.or(Some(WorkUsage::default())),
                            vec![],
                            None,
                            Some(note.into()),
                            None,
                        )
                        .await?;
                    }
                },
            }
        }
        Ok(terminal)
    }

    async fn ask(
        &mut self,
        question: WorkAgentQuestion,
    ) -> Result<Option<WorkAttemptStatus>, WorkError> {
        let prompt = question.prompt.clone();
        match self
            .ask_person(
                question.prompt,
                question.options,
                WorkAskPurposeV1::Question,
            )
            .await?
        {
            Ok(answer) => {
                self.decisions.push(planning::PlanningAnswer {
                    question: prompt,
                    answer,
                });
                Ok(None)
            }
            Err(status) => Ok(Some(status)),
        }
    }

    /// Puts one question to the person and waits for the answer; an
    /// unanswered question past the wait, or a stop, ends the run.
    async fn ask_person(
        &mut self,
        prompt: String,
        options: Vec<String>,
        purpose: WorkAskPurposeV1,
    ) -> Result<Result<String, WorkAttemptStatus>, WorkError> {
        self.probe.record_activity(WorkActivityV1::WaitingForHuman);
        let step = self.step(
            WorkStepKindV1::Ask {
                prompt,
                options,
                answer: None,
                purpose: Some(purpose),
            },
            WorkStepStatus::Running,
        );
        let id = self.begin(step, vec![], None).await?;
        let since = self.wait();
        loop {
            tokio::time::sleep(ASK_POLL).await;
            let state = self.probe.runtime_projection().await?;
            let execution = state
                .executions
                .iter()
                .find(|e| e.id == self.probe.execution())
                .ok_or(WorkError::NotFound)?;
            let asked = execution
                .steps
                .iter()
                .find(|s| s.id == id)
                .ok_or(WorkError::NotFound)?;
            if asked.status == WorkStepStatus::Succeeded {
                self.resume(since);
                let WorkStepKindV1::Ask {
                    answer: Some(answer),
                    ..
                } = &asked.kind
                else {
                    return Ok(Ok(String::new()));
                };
                return Ok(Ok(answer.clone()));
            }
            // An open question never spends the run; unanswered past the
            // wait, the run stops on it and an answer or Continue resumes it.
            if since.elapsed() >= WAIT_PATIENCE {
                self.suspend().await;
                self.settle(
                    id,
                    WorkStepStatus::Cancelled,
                    None,
                    vec![],
                    None,
                    Some(ASK_SUSPENDED.into()),
                    None,
                )
                .await?;
                return Ok(Err(WorkAttemptStatus::Cancelled));
            }
            if self.cancelled().await {
                let note = self.stop_note().unwrap_or(INTERRUPTED);
                self.settle(
                    id,
                    WorkStepStatus::Cancelled,
                    None,
                    vec![],
                    None,
                    Some(note.into()),
                    None,
                )
                .await?;
                return Ok(Err(WorkAttemptStatus::Cancelled));
            }
        }
    }

    /// The run spent its budget: the person decides whether it keeps going
    /// with the same amount again. False when they stop it or it cannot grow.
    async fn keep_going(&mut self) -> Result<Result<bool, WorkAttemptStatus>, WorkError> {
        if !self.extends {
            return Ok(Ok(false));
        }
        let grown = WorkExecutionLimits {
            model_tokens: self
                .limits
                .model_tokens
                .saturating_add(self.base_limits.model_tokens)
                .min(1_000_000),
            cost_micro_usd: self
                .limits
                .cost_micro_usd
                .saturating_add(self.base_limits.cost_micro_usd)
                .min(10_000_000),
            operations: self
                .limits
                .operations
                .saturating_add(self.base_limits.operations)
                .min(1024),
            ..self.limits
        };
        if grown.cost_micro_usd == self.limits.cost_micro_usd
            && grown.operations == self.limits.operations
        {
            return Ok(Ok(false));
        }
        let spent = f64::from(self.used.cost_micro_usd) / 1_000_000.0;
        let answer = match self
            .ask_person(
                format!("Used ${spent:.2}. Keep going?"),
                vec![KEEP_GOING.into(), STOP_HERE.into()],
                WorkAskPurposeV1::Budget,
            )
            .await?
        {
            Ok(answer) => answer,
            Err(status) => return Ok(Err(status)),
        };
        let granted = answer.trim().eq_ignore_ascii_case(KEEP_GOING);
        self.report(WorkAgentDiagnostic::KeepGoing { granted });
        if !granted {
            return Ok(Ok(false));
        }
        self.probe.extend_limits(grown).await?;
        self.limits = grown;
        Ok(Ok(true))
    }

    /// The session a page task on `site` opens in, asking the person first
    /// when the profile holds a session there. A stop ends the run.
    /// A page task's session, or None when its entry question waits on the
    /// start page: the page task checks it for a signed-out state first.
    async fn page_task_session(
        &mut self,
        site: &str,
        _goal: &str,
    ) -> Result<Result<Option<crate::work_sites::SiteSession>, WorkAttemptStatus>, WorkError> {
        use crate::work_sites::*;
        let present = crate::work_context::sessions_present(self.profile, vec![site.to_owned()])
            .await
            .first()
            .copied()
            .flatten()
            .unwrap_or(true);
        let entry = self.sites.entry(site, present);
        Ok(Ok(match entry {
            Entry::Ask => None,
            entry => Some(self.entered(site, entry)),
        }))
    }

    /// Records the person's answer to a site's entry question.
    async fn answer_entry(&mut self, site: &str, answer: crate::work_sites::EntryAnswer) {
        use crate::work_sites::*;
        self.report(WorkAgentDiagnostic::SiteEntered { answer });
        if answer == EntryAnswer::Always {
            if let Err(error) = set_standing(
                &self.handle,
                self.profile,
                site.to_owned(),
                Some(zephium_core::work::sites::WorkSiteAccessV1::Always),
            )
            .await
            {
                self.report(WorkAgentDiagnostic::CommitRefused {
                    kind: "site",
                    error,
                });
            }
        }
        let entry = self.sites.answer(site, answer);
        self.entered(site, entry);
    }

    fn entered(
        &mut self,
        site: &str,
        entry: crate::work_sites::Entry,
    ) -> crate::work_sites::SiteSession {
        use crate::work_sites::*;
        match entry {
            Entry::Session(session) => session,
            Entry::Private(PrivateBecause::Public) => SiteSession::Private,
            Entry::Private(because) => {
                self.report(WorkAgentDiagnostic::PrivatePage { because });
                self.notice(&format!(
                    "{} is worked on without the person's session in this run: {}.",
                    site_name(site),
                    match because {
                        PrivateBecause::PrivateRun => "this is a private run",
                        PrivateBecause::Never => "the person never lets the agent use it",
                        PrivateBecause::Sensitive =>
                            "it is a sensitive site the person has not opened to the agent",
                        PrivateBecause::Declined => "the person said not now",
                        PrivateBecause::Public => "it is a public read",
                    }
                ));
                SiteSession::Private
            }
            Entry::Ask => SiteSession::Private,
        }
    }

    /// Seeds the thread from this work's earlier executions. Objects come
    /// whole from the last few runs, plus older ones the request names; a
    /// question the last run stopped on is answered by this request.
    async fn inherit(&mut self, projection: &WorkRuntimeProjection, current: WorkExecutionId) {
        let earlier: Vec<&WorkExecutionFact> = projection
            .executions
            .iter()
            .filter(|execution| execution.id != current)
            .collect();
        let produced: Vec<&[WorkArtifactV1]> = earlier
            .iter()
            .map(|execution| execution.artifacts.as_slice())
            .collect();
        let (artifacts, kept) = inheritable(&produced, &self.objective, &self.bodies);
        let recent = earlier.len().saturating_sub(INHERITED_EXECUTIONS);
        for execution in earlier[recent..].iter().rev() {
            for record in &execution.provider_evidence {
                if self.previews.len() >= INHERITED_PREVIEWS {
                    break;
                }
                self.remember(record);
            }
        }
        for artifact in artifacts.iter().rev() {
            for link in &artifact.evidence {
                if self.previews.len() >= INHERITED_PREVIEWS {
                    break;
                }
                if self.previews.iter().any(|preview| preview.link == *link) {
                    continue;
                }
                if let Ok(preview) = self.probe.read_evidence(link.clone()).await {
                    self.keep_preview(preview);
                }
            }
        }
        self.inherited = artifacts;
        self.kept = kept;
        if let Some((question, _)) = earlier.last().and_then(|last| pending_question(last)) {
            let answer = projection
                .executions
                .iter()
                .find(|execution| execution.id == current)
                .and_then(|execution| execution.spec.request.clone())
                .unwrap_or_else(|| self.objective.clone());
            self.decisions.push(planning::PlanningAnswer {
                question: question.to_owned(),
                answer,
            });
        }
        let mut thread: Vec<WorkAgentThreadEntry> = Vec::new();
        for execution in &earlier {
            let Some(request) = &execution.spec.request else {
                continue;
            };
            if thread.last().is_some_and(|entry| entry.request == *request) {
                continue;
            }
            thread.push(WorkAgentThreadEntry {
                request: request.clone(),
                ended: execution_ended(execution.status),
                summary: execution_summary(execution),
            });
        }
        if thread
            .last()
            .is_some_and(|entry| entry.request == self.objective)
        {
            thread.pop();
        }
        self.thread = thread.into_iter().rev().take(16).rev().collect();
    }
    fn remember(&mut self, record: &WorkProviderSearchRecordV1) {
        let preferred = record
            .ranking
            .as_ref()
            .map(|ranking| ranking.preferred.as_slice())
            .unwrap_or_default();
        let mut seen = std::collections::BTreeSet::new();
        let indices = preferred.iter().map(|id| usize::from(*id) - 1).chain(
            (0..record.evidence.citations.len())
                .filter(|index| !preferred.contains(&((*index + 1) as u16))),
        );
        for index in indices {
            let citation = &record.evidence.citations[index];
            if !seen.insert(source_key(&citation.url)) {
                continue;
            }
            let Ok(text) = record.evidence.citation_excerpt(index) else {
                continue;
            };
            let Ok(origin) = url::Url::parse(&citation.url) else {
                continue;
            };
            let source_bytes = record.evidence.answer.len();
            self.keep_preview(WorkEvidencePreviewV1 {
                link_destination: None,
                version: 1,
                link: WorkEvidenceLink {
                    extraction_id: record.id,
                    source_id: (index + 1) as u16,
                },
                origin: origin.origin().ascii_serialization(),
                role: "provider_search".into(),
                truncated: text.len() < source_bytes,
                text,
                source_bytes: source_bytes.to_string(),
                source: WorkEvidenceSourceV1::ProviderSearch {
                    provider: record.evidence.provider,
                    model: record.evidence.model.clone(),
                    url: citation.url.clone(),
                    title: citation.title.clone(),
                    response_id: record.evidence.response_id.clone(),
                    search_call_id: record.evidence.search_call_id.clone(),
                },
            });
        }
    }
    fn keep_preview(&mut self, preview: WorkEvidencePreviewV1) {
        if let Some(index) = self.previews.iter().position(|p| p.link == preview.link) {
            self.previews.remove(index);
        }
        if self.previews.len() >= MAX_PREVIEWS {
            self.previews.remove(0);
        }
        self.previews.push(preview);
    }
}

/// A lead part's hands on the page and file machinery: searches, page
/// reads and tasks with their entry questions, sign-in waits and Confirm
/// steps, and file and command steps, each carrying the part. It works
/// within its own budget share and never grows the run's budget.
pub(crate) struct WorkPartDriver(Driver);
impl WorkPartDriver {
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn new(
        handle: crate::Handle,
        profile: ProfileId,
        probe: WorkAttemptProbe,
        grant: WorkAgentGrantV1,
        limits: WorkExecutionLimits,
        output: String,
        objective: String,
        part: Option<WorkPartId>,
    ) -> Self {
        let standing = crate::work_sites::standing(&handle, profile)
            .await
            .unwrap_or_default();
        let sites = crate::work_sites::RunSites::new(grant.private, standing);
        let (files, _) = crate::work_files::WorkFileGrant::admit(&grant.folders);
        Self(Driver {
            probe,
            files: (!files.is_empty()).then_some(files),
            grant,
            limits,
            output,
            objective,
            decisions: Vec::new(),
            bodies: Vec::new(),
            private: Vec::new(),
            inherited: Vec::new(),
            kept: Vec::new(),
            thread: Vec::new(),
            unreadable_polls: std::sync::atomic::AtomicU8::new(0),
            stopped: std::sync::Mutex::new(None),
            waiting: false,
            previews: Vec::new(),
            used: WorkUsage::default(),
            steps: 0,
            turn: 1,
            failed_turns: 0,
            intervention: None,
            diagnostic: None,
            notices: Vec::new(),
            published: 0,
            finish_refusals: 0,
            pending_output_repair: false,
            sites,
            session_steps: Vec::new(),
            page_sites: Vec::new(),
            private_sites: Vec::new(),
            base_limits: limits,
            handle,
            profile,
            tabs: Vec::new(),
            part,
            lead: true,
            extends: false,
            views: Vec::new(),
            typing_held: Vec::new(),
            typing_allowed: Vec::new(),
        })
    }
    /// Runs searches, page reads and tasks, and file and command steps. A
    /// terminal status means the run cannot go on (stopped, lost outcome).
    pub(crate) async fn run<B, Fut>(
        &mut self,
        attempt: &WorkNodeAttempt,
        search: &dyn WorkPublicSearchProvider,
        browser: &mut B,
        turn: u8,
        steps: usize,
        kinds: Vec<WorkStepKindV1>,
    ) -> Result<Option<WorkAttemptStatus>, WorkError>
    where
        B: FnMut(WorkAttemptProbe, WorkAgentBrowseRequest) -> Fut,
        Fut: Future<Output = Result<WorkBrowserOutcome, WorkError>>,
    {
        self.0.turn = turn.max(1);
        self.0.steps = u32::try_from(steps).unwrap_or(u32::MAX);
        self.0.fetch(attempt, search, browser, kinds).await
    }
    /// The sites among this driver's page tasks whose entry the run has not
    /// decided and that would ask the person first; the rest are decided now
    /// exactly as a page task would decide them.
    pub(crate) async fn undecided_entries(&mut self, sites: Vec<String>) -> Vec<String> {
        let present = crate::work_context::sessions_present(self.0.profile, sites.clone()).await;
        sites
            .into_iter()
            .zip(present)
            .filter(|(site, present)| {
                self.0.sites.entry(site, present.unwrap_or(true)) == crate::work_sites::Entry::Ask
            })
            .map(|(site, _)| site)
            .collect()
    }
    /// Puts the run's one entry question naming each service; the answer
    /// read against its own options, or the terminal status that ended it.
    pub(crate) async fn ask_entry(
        &mut self,
        names: &[String],
    ) -> Result<Result<crate::work_sites::EntryAnswer, WorkAttemptStatus>, WorkError> {
        let (prompt, options) = crate::work_sites::entry_question_for(names);
        Ok(self
            .0
            .ask_person(prompt, options.clone(), WorkAskPurposeV1::Entry)
            .await?
            .map(|answer| crate::work_sites::entry_answer_to(&options, &answer)))
    }
    /// Page tasks on these sites read them publicly; the rest are worked on
    /// as the person, after the entry question where it applies.
    pub(crate) fn scope_sites(&mut self, public: &[String], personal: &[String]) {
        for site in public {
            self.0.sites.public(site);
        }
        for site in personal {
            self.0.sites.personal(site);
        }
    }
    /// Page tasks on these start pages only read what the app's view lists.
    pub(crate) fn view_pages(&mut self, pages: Vec<String>) {
        self.0.views = pages;
    }
    pub(crate) fn hold_typing(&mut self, sites: Vec<String>) {
        self.0.typing_held = sites;
    }
    /// Sites the person allowed typing on since this was last asked.
    pub(crate) fn take_typing_allowed(&mut self) -> Vec<String> {
        std::mem::take(&mut self.0.typing_allowed)
    }
    /// Records the run's entry answer for one of this driver's sites.
    pub(crate) async fn enter(&mut self, site: &str, answer: crate::work_sites::EntryAnswer) {
        self.0.answer_entry(site, answer).await;
    }
    /// What the driver refused or learned since it was last asked, in
    /// closed words.
    pub(crate) fn take_notices(&mut self) -> Vec<String> {
        std::mem::take(&mut self.0.notices)
    }
    pub(crate) fn used(&self) -> WorkUsage {
        self.0.used
    }
    /// Source text the driver kept for a link, clipped for a digest.
    pub(crate) fn preview(&self, link: &WorkEvidenceLink) -> Option<&WorkEvidencePreviewV1> {
        self.0.previews.iter().find(|preview| preview.link == *link)
    }
}

/// What one turn discloses besides the context it may shed.
struct TurnView<'a> {
    objective: &'a str,
    decisions: &'a [planning::PlanningAnswer],
    bodies: &'a [context::WorkContextBody],
    steps: &'a [WorkStepFact],
    current: &'a [WorkArtifactV1],
    budget: WorkAgentBudget,
    remaining: WorkExecutionLimits,
    notices: &'a [String],
    sites: &'a [WorkAgentSiteView],
    tabs: &'a [context::WorkContextTabV1],
}

/// Builds the turn, shedding until it fits: the disclosure itself first drops
/// older object bodies and shortens source text; then uncited sources go,
/// then inherited objects oldest first (never the kept ones), then thread
/// summaries oldest first, and only then the oldest thread entries. The
/// canvas keeps everything; only the model's view shrinks.
fn disclose(
    view: &TurnView<'_>,
    previews: &mut Vec<WorkEvidencePreviewV1>,
    inherited: &mut Vec<WorkArtifactV1>,
    kept: &[WorkArtifactId],
    thread: &mut Vec<WorkAgentThreadEntry>,
) -> Result<WorkAgentTurnDisclosure, WorkError> {
    loop {
        let room = MAX_WORK_ARTIFACTS.saturating_sub(view.current.len());
        let artifacts: Vec<WorkArtifactV1> = inherited
            .iter()
            .rev()
            .take(room)
            .rev()
            .chain(view.current)
            .cloned()
            .collect();
        let disclosed = WorkAgentTurnDisclosure::try_new(
            view.objective,
            view.decisions.to_vec(),
            view.bodies.to_vec(),
            view.steps,
            previews,
            &artifacts,
            view.budget,
            view.remaining,
            view.notices.to_vec(),
        )
        .and_then(|disclosure| disclosure.with_thread(thread.clone()))
        .and_then(|disclosure| disclosure.with_sites(view.sites.to_vec()))
        .and_then(|disclosure| disclosure.with_tabs(view.tabs));
        match disclosed {
            Err(WorkError::Capacity) => {}
            other => return other,
        }
        if evict_previews(previews, &artifacts) {
            continue;
        }
        let before = inherited.len();
        let mut shed = 0;
        inherited.retain(|artifact| {
            if shed < 4 && !kept.contains(&artifact.id) {
                shed += 1;
                false
            } else {
                true
            }
        });
        if inherited.len() < before {
            continue;
        }
        if let Some(entry) = thread.iter_mut().find(|entry| entry.summary.is_some()) {
            entry.summary = None;
            continue;
        }
        if thread.len() > 1 {
            thread.remove(0);
            continue;
        }
        return Err(WorkError::Capacity);
    }
}

/// Drops the oldest sources no current object cites; false when none can go.
fn evict_previews(previews: &mut Vec<WorkEvidencePreviewV1>, artifacts: &[WorkArtifactV1]) -> bool {
    let cited: Vec<&WorkEvidenceLink> = artifacts
        .iter()
        .flat_map(|artifact| artifact.evidence.iter())
        .collect();
    let before = previews.len();
    let mut dropped = 0;
    previews.retain(|preview| {
        if dropped < 16 && !cited.iter().any(|link| **link == preview.link) {
            dropped += 1;
            false
        } else {
            true
        }
    });
    previews.len() < before
}

/// Objects a run inherits, oldest first, from earlier executions' objects
/// (oldest execution first): everything from the last few runs plus older
/// subjects and comparisons the request names. Kept: those named and those
/// the latest producing run placed.
fn inheritable(
    executions: &[&[WorkArtifactV1]],
    objective: &str,
    bodies: &[context::WorkContextBody],
) -> (Vec<WorkArtifactV1>, Vec<WorkArtifactId>) {
    let recent = executions.len().saturating_sub(INHERITED_EXECUTIONS);
    let latest = executions
        .iter()
        .rposition(|artifacts| !artifacts.is_empty());
    let mut artifacts = Vec::new();
    let mut kept = Vec::new();
    for (index, produced) in executions.iter().enumerate().rev() {
        for artifact in produced.iter().rev() {
            if artifacts.len() >= INHERITED_ARTIFACTS {
                break;
            }
            let named = referenced(artifact, objective, bodies);
            let last = Some(index) == latest;
            if index < recent && !named && !last {
                continue;
            }
            if named || last {
                kept.push(artifact.id);
            }
            artifacts.push(artifact.clone());
        }
    }
    artifacts.reverse();
    (artifacts, kept)
}

/// A subject or comparison object the request names by its title or one of
/// its subjects, or selects as context.
fn referenced(
    artifact: &WorkArtifactV1,
    objective: &str,
    bodies: &[context::WorkContextBody],
) -> bool {
    let subjects: &[WorkSubject] = match &artifact.data {
        WorkArtifactDataV1::ComparisonMatrix { subjects, .. }
        | WorkArtifactDataV1::Findings { subjects, .. }
        | WorkArtifactDataV1::EvidenceCollection { subjects, .. } => subjects,
        WorkArtifactDataV1::Comparison { .. } => &[],
        _ => return false,
    };
    if subjects.is_empty() && !matches!(artifact.data, WorkArtifactDataV1::Comparison { .. }) {
        return false;
    }
    let objective = objective.to_lowercase();
    let named = |name: &str| {
        let name = name.trim().to_lowercase();
        name.chars().count() >= 3 && objective.contains(&name)
    };
    named(&artifact.title)
        || subjects.iter().any(|subject| named(&subject.name))
        || bodies.iter().any(|body| {
            matches!(
                body.kind,
                context::WorkContextItemKind::Artifact | context::WorkContextItemKind::Subject
            ) && (body.title == artifact.title
                || subjects.iter().any(|subject| subject.name == body.title))
        })
}

/// The question a settled execution stopped on, still unanswered.
fn pending_question(execution: &WorkExecutionFact) -> Option<(&str, &[String])> {
    if !execution.status.terminal() {
        return None;
    }
    execution
        .steps
        .iter()
        .rev()
        .find_map(|step| match &step.kind {
            WorkStepKindV1::Ask {
                prompt,
                options,
                answer: None,
                ..
            } if step.status != WorkStepStatus::Running => {
                Some((prompt.as_str(), options.as_slice()))
            }
            _ => None,
        })
}

fn reuses_completed_read(request: &WorkStepKindV1, step: &WorkStepFact) -> bool {
    if step.status != WorkStepStatus::Succeeded || step.artifacts.is_empty() {
        return false;
    }
    match (request, &step.kind) {
        (
            WorkStepKindV1::Read {
                url,
                collection,
                goal: None,
            },
            WorkStepKindV1::Read {
                url: previous_url,
                collection: previous,
                goal: None,
            },
        ) if url == previous_url => match (collection, previous) {
            (None, None) => true,
            (Some(current), Some(previous)) => {
                current.max_items == previous.max_items
                    && current.columns.len() == previous.columns.len()
                    && current.columns.iter().all(|column| {
                        previous.columns.iter().any(|old| {
                            old.name == column.name
                                && old.value == column.value
                                && old.required == column.required
                        })
                    })
            }
            _ => false,
        },
        _ => false,
    }
}

fn browser_preview_links(artifacts: &[WorkArtifactV1]) -> Vec<WorkEvidenceLink> {
    let mut links = Vec::new();
    for index in 0..MAX_ARTIFACT_EVIDENCE {
        for artifact in artifacts {
            if let Some(link) = artifact.evidence.get(index) {
                if !links.contains(link) {
                    links.push(link.clone());
                    if links.len() == MAX_PREVIEWS {
                        return links;
                    }
                }
            }
        }
    }
    links
}

fn step_kind_label(kind: &WorkStepKindV1) -> &'static str {
    match kind {
        WorkStepKindV1::Turn => "turn",
        WorkStepKindV1::Search { .. } => "search",
        WorkStepKindV1::Read { .. } => "read",
        WorkStepKindV1::Discover { .. } => "discover",
        WorkStepKindV1::Publish => "publish",
        WorkStepKindV1::Ask { .. } => "ask",
        WorkStepKindV1::Confirm { confirm: _ } => "confirm",
        WorkStepKindV1::Call { .. } => "call",
        WorkStepKindV1::Steer { .. } => "person",
        WorkStepKindV1::List { .. } => "list",
        WorkStepKindV1::ReadFile { .. } => "read_file",
        WorkStepKindV1::SearchFiles { .. } => "search_files",
        WorkStepKindV1::WriteFile { .. } => "write_file",
        WorkStepKindV1::EditFile { .. } => "edit_file",
        WorkStepKindV1::RunCommand { .. } => "run_command",
        WorkStepKindV1::MoveFile { .. } => "move_file",
        WorkStepKindV1::DeleteFile { .. } => "delete_file",
        WorkStepKindV1::Finish { .. } => "finish",
    }
}

/// Why a page could not be opened at all, in the person's words.
fn browse_error_note(error: WorkError) -> &'static str {
    match error {
        WorkError::Capacity => "Too many pages were already open",
        WorkError::Unavailable | WorkError::Shutdown => "The browser was not available",
        WorkError::ProfileUnavailable => "The browsing profile was not available",
        _ => "The page could not be opened",
    }
}

fn step_status(status: WorkAttemptStatus) -> WorkStepStatus {
    match status {
        WorkAttemptStatus::Running => WorkStepStatus::Running,
        WorkAttemptStatus::Succeeded => WorkStepStatus::Succeeded,
        WorkAttemptStatus::Failed => WorkStepStatus::Failed,
        WorkAttemptStatus::Cancelled => WorkStepStatus::Cancelled,
        WorkAttemptStatus::OutcomeUnknown => WorkStepStatus::OutcomeUnknown,
    }
}

/// Provider citations become one sources object: every citation is an entry
/// the canvas can show and later work can cite.
fn sources_draft(output: &str, record: &WorkProviderSearchRecordV1) -> WorkArtifactDraft {
    // One card per page: the provider often cites the same page twice.
    let mut seen = std::collections::BTreeSet::new();
    let entries = record
        .evidence
        .citations
        .iter()
        .take(64)
        .enumerate()
        .filter(|(_, citation)| seen.insert(source_key(&citation.url)))
        .map(|(index, citation)| WorkSourceEntry {
            evidence: index as u16,
            title: source_title(&citation.title, &citation.url),
            role: "source".into(),
            subject: None,
        })
        .collect();
    let evidence = (1..=record.evidence.citations.len().min(64))
        .map(|index| WorkEvidenceLink {
            extraction_id: record.id,
            source_id: index as u16,
        })
        .collect();
    let summary: String = record.evidence.answer.chars().take(4000).collect();
    WorkArtifactDraft {
        output: output.to_owned(),
        title: "Sources".into(),
        data: WorkArtifactDataV1::EvidenceCollection {
            summary: if summary.trim().is_empty() {
                "Sources found".into()
            } else {
                summary
            },
            subjects: vec![],
            entries,
        },
        evidence,
    }
}
/// Provider tracking parameters do not make a different page.
fn source_key(url: &str) -> String {
    match url::Url::parse(url) {
        Ok(mut parsed) => {
            let kept: Vec<(String, String)> = parsed
                .query_pairs()
                .filter(|(key, _)| key != "utm_source")
                .map(|(key, value)| (key.into_owned(), value.into_owned()))
                .collect();
            parsed.set_query(None);
            if !kept.is_empty() {
                parsed.query_pairs_mut().extend_pairs(kept);
            }
            parsed.set_fragment(None);
            parsed.to_string().trim_end_matches('/').to_lowercase()
        }
        Err(_) => url.to_lowercase(),
    }
}
fn source_title(title: &str, url: &str) -> String {
    let title: String = title.trim().chars().take(200).collect();
    if !title.is_empty() {
        return title;
    }
    url::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_owned))
        .unwrap_or_else(|| "Source".into())
}
fn sources_note(count: usize) -> String {
    match count {
        0 => "No sources found".into(),
        1 => "Found 1 source".into(),
        n => format!("Found {n} sources"),
    }
}
fn read_note(artifacts: &[WorkArtifactV1]) -> String {
    if let [artifact] = artifacts {
        match &artifact.data {
            WorkArtifactDataV1::ComparisonMatrix { subjects, .. } => {
                return format!("Placed {} cited records on the canvas", subjects.len());
            }
            WorkArtifactDataV1::Findings { items, .. } => {
                return format!("Placed {} cited findings on the canvas", items.len());
            }
            _ => {}
        }
    }
    match artifacts.len() {
        0 => "Read the page".into(),
        _ => "Read the page and kept notes".into(),
    }
}
fn publish_note(artifacts: &[WorkArtifactV1]) -> String {
    let mut kinds: Vec<&str> = artifacts.iter().map(|a| artifact_kind(&a.data)).collect();
    kinds.sort_unstable();
    kinds.dedup();
    format!(
        "Placed {} on the canvas",
        kinds.join(", ").replace('_', " ")
    )
}

/// Search text may not repeat a private context body: a 24-character window
/// of the query found inside any private body refuses the whole turn.
pub fn query_discloses(query: &str, private: &[String]) -> bool {
    const WINDOW: usize = 24;
    let query: Vec<char> = query.to_lowercase().chars().collect();
    if query.len() < WINDOW || private.is_empty() {
        return false;
    }
    let bodies: Vec<String> = private.iter().map(|body| body.to_lowercase()).collect();
    query.windows(WINDOW).any(|window| {
        let window: String = window.iter().collect();
        bodies.iter().any(|body| body.contains(&window))
    })
}

/// Bounded fan-out without a futures dependency: polls every pending future
/// each wake and returns once all have settled.
async fn join_all<T>(mut futures: Vec<Pin<Box<dyn Future<Output = T> + Send + '_>>>) -> Vec<T> {
    let mut results: Vec<Option<T>> = futures.iter().map(|_| None).collect();
    std::future::poll_fn(|cx: &mut Context<'_>| {
        let mut pending = false;
        for (index, future) in futures.iter_mut().enumerate() {
            if results[index].is_some() {
                continue;
            }
            match future.as_mut().poll(cx) {
                Poll::Ready(value) => results[index] = Some(value),
                Poll::Pending => pending = true,
            }
        }
        if pending {
            Poll::Pending
        } else {
            Poll::Ready(())
        }
    })
    .await;
    results.into_iter().flatten().collect()
}

fn execution_ended(status: WorkExecutionStatus) -> &'static str {
    match status {
        WorkExecutionStatus::Completed | WorkExecutionStatus::NeedsReview => "completed",
        WorkExecutionStatus::Cancelled | WorkExecutionStatus::CancelRequested => "stopped",
        WorkExecutionStatus::Failed => "failed",
        WorkExecutionStatus::Interrupted => "interrupted",
        WorkExecutionStatus::Approved | WorkExecutionStatus::Running => "running",
    }
}

/// The run's last line for the person, or the note of the step that ended
/// it, plus what it did; the model reads it, so it is clipped.
fn execution_summary(execution: &WorkExecutionFact) -> Option<String> {
    let last = execution
        .steps
        .iter()
        .rev()
        .find_map(|step| step.note.as_deref().filter(|note| !note.trim().is_empty()));
    let searches = execution
        .steps
        .iter()
        .filter(|step| matches!(step.kind, WorkStepKindV1::Search { .. }))
        .count();
    let reads = execution
        .steps
        .iter()
        .filter(|step| {
            matches!(
                step.kind,
                WorkStepKindV1::Read { .. } | WorkStepKindV1::Discover { .. }
            ) && step.status == WorkStepStatus::Succeeded
        })
        .count();
    // A question the run stopped on leads, so clipping never loses it.
    let mut summary = match pending_question(execution) {
        Some((prompt, [])) => format!("Stopped waiting for the person's answer to: {prompt}"),
        Some((prompt, options)) => format!(
            "Stopped waiting for the person's answer to: {prompt} Options: {}.",
            options.join("; ")
        ),
        None => last.map(str::to_owned).unwrap_or_default(),
    };
    if !summary.is_empty() {
        summary.push(' ');
    }
    summary.push_str(&format!(
        "({searches} searches, {reads} pages read, {} objects placed)",
        execution.artifacts.len()
    ));
    Some(zephium_core::work::agent::clip_text(
        &summary,
        MAX_WORK_STEP_NOTE_BYTES,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn repeated_read_reuses_results_but_failed_or_changed_requests_can_run() {
        let collection = serde_json::from_value(serde_json::json!({
            "title":"Products", "max_items":3, "columns":[
                {"name":"price","required":false,"value":{"kind":"text"}}
            ]
        }))
        .unwrap();
        let request = WorkStepKindV1::Read {
            url: "https://shop.example.test/catalog?q=sets".into(),
            collection: Some(collection),
            goal: None,
        };
        let mut step = WorkStepFact {
            part: None,
            id: 1.into(),
            turn: 1,
            kind: request.clone(),
            status: WorkStepStatus::Succeeded,
            usage: Some(WorkUsage::default()),
            artifacts: vec![1.into()],
            evidence: None,
            note: None,
            measurements: None,
            local: None,
            account: None,
        };
        assert!(reuses_completed_read(&request, &step));
        let mut renamed = request.clone();
        if let WorkStepKindV1::Read {
            collection: Some(schema),
            ..
        } = &mut renamed
        {
            schema.title = "Compared products".into();
        }
        assert!(reuses_completed_read(&renamed, &step));
        let mut changed_requirement = request.clone();
        if let WorkStepKindV1::Read {
            collection: Some(schema),
            ..
        } = &mut changed_requirement
        {
            schema.columns[0].required = true;
        }
        assert!(!reuses_completed_read(&changed_requirement, &step));
        let mut stricter_previous = step.clone();
        stricter_previous.kind = changed_requirement;
        assert!(!reuses_completed_read(&request, &stricter_previous));
        if let WorkStepKindV1::Read {
            collection: Some(schema),
            ..
        } = &mut renamed
        {
            schema.max_items = 5;
        }
        assert!(!reuses_completed_read(&renamed, &step));
        let mut changed_url = request.clone();
        if let WorkStepKindV1::Read { url, .. } = &mut changed_url {
            *url = "https://shop.example.test/catalog?q=other".into();
        }
        assert!(!reuses_completed_read(&changed_url, &step));
        step.status = WorkStepStatus::Failed;
        assert!(!reuses_completed_read(&request, &step));
        step.status = WorkStepStatus::Succeeded;
        step.artifacts.clear();
        assert!(!reuses_completed_read(&request, &step));
    }

    #[test]
    fn browser_previews_cover_later_artifacts_and_deduplicate_within_the_budget() {
        let artifact = |id: u128| WorkArtifactV1 {
            revises: None,
            part: None,
            version: 1,
            id: id.into(),
            execution: 1.into(),
            node: 1.into(),
            attempt: 1.into(),
            output: "results".into(),
            title: "Results".into(),
            data: WorkArtifactDataV1::Findings {
                subjects: vec![],
                items: vec![],
            },
            evidence: (1..=64)
                .map(|source_id| WorkEvidenceLink {
                    extraction_id: id.into(),
                    source_id,
                })
                .collect(),
            review: WorkOutputReview::SourceMappedNeedsReview,
            presentation: WorkArtifactPresentationV1::Automatic,
            general_knowledge: false,
        };
        let first = artifact(10);
        let second = artifact(20);
        let links = browser_preview_links(&[first.clone(), first, second]);
        assert_eq!(links.len(), MAX_PREVIEWS);
        assert_eq!(
            links
                .iter()
                .filter(|link| link.extraction_id == 10.into())
                .count(),
            48
        );
        assert_eq!(
            links
                .iter()
                .filter(|link| link.extraction_id == 20.into())
                .count(),
            48
        );
        assert!(links.iter().any(|link| link.source_id == 48));
    }

    #[test]
    fn a_long_work_still_discloses_a_turn_after_shedding() {
        let artifact = |execution: u128, index: u128, data: WorkArtifactDataV1| WorkArtifactV1 {
            revises: None,
            part: None,
            version: 1,
            id: (execution * 100 + index).into(),
            execution: execution.into(),
            node: 1.into(),
            attempt: execution.into(),
            output: "results".into(),
            title: format!("Result {execution}.{index}"),
            data,
            evidence: (1..=2)
                .map(|source_id| WorkEvidenceLink {
                    extraction_id: (execution * 100 + index).into(),
                    source_id,
                })
                .collect(),
            review: WorkOutputReview::SourceMappedNeedsReview,
            presentation: WorkArtifactPresentationV1::Automatic,
            general_knowledge: false,
        };
        let document = || WorkArtifactDataV1::Document {
            paragraphs: vec!["x".repeat(3000)],
            formatted: None,
        };
        let executions: Vec<Vec<WorkArtifactV1>> = (1..=16)
            .map(|execution| {
                (1..=4)
                    .map(|index| {
                        let data = if execution == 2 && index == 1 {
                            WorkArtifactDataV1::Findings {
                                subjects: vec![WorkSubject {
                                    name: "Lisbon shortlist".into(),
                                    descriptor: None,
                                    homepage: None,
                                    image_candidates: vec![],
                                }],
                                items: vec![],
                            }
                        } else {
                            document()
                        };
                        artifact(execution, index, data)
                    })
                    .collect()
            })
            .collect();
        let slices: Vec<&[WorkArtifactV1]> = executions.iter().map(Vec::as_slice).collect();
        let objective = "Check the visa rules for the Lisbon shortlist as well";
        let (mut inherited, kept) = inheritable(&slices, objective, &[]);
        assert_eq!(inherited.len(), 3 * 4 + 1);
        assert!(inherited.iter().any(|a| a.title == "Result 2.1"));
        assert!(kept.contains(&WorkArtifactId::from(201)) && kept.contains(&1601.into()));
        let current: Vec<WorkArtifactV1> = (1..=4)
            .map(|index| artifact(17, index, document()))
            .collect();
        let mut previews: Vec<WorkEvidencePreviewV1> = inherited
            .iter()
            .chain(&current)
            .flat_map(|a| a.evidence.clone())
            .chain((1..=40).map(|source_id| WorkEvidenceLink {
                extraction_id: 9999.into(),
                source_id,
            }))
            .map(|link| WorkEvidencePreviewV1 {
                version: 1,
                link,
                origin: "https://example.test".into(),
                role: "page".into(),
                text: "y".repeat(8000),
                truncated: false,
                source_bytes: "8000".into(),
                link_destination: None,
                source: WorkEvidenceSourceV1::NativeExtraction,
            })
            .collect();
        let mut thread: Vec<WorkAgentThreadEntry> = (0..16)
            .map(|_| WorkAgentThreadEntry {
                request: "r".repeat(1500),
                ended: "completed",
                summary: Some("s".repeat(500)),
            })
            .collect();
        let view = TurnView {
            objective,
            decisions: &[],
            bodies: &[],
            steps: &[],
            current: &current,
            budget: WorkAgentBudget {
                turns_left: 4,
                steps_left: 8,
                browse_available: true,
            },
            remaining: WorkExecutionLimits {
                model_tokens: 100_000,
                cost_micro_usd: 100_000,
                operations: 32,
                timeout_seconds: 600,
                max_workers: 1,
            },
            notices: &[],
            sites: &[],
            tabs: &[],
        };
        let disclosure = disclose(&view, &mut previews, &mut inherited, &kept, &mut thread)
            .expect("a long work still gets its turn");
        let context = disclosure.context();
        assert!(previews.len() < 74, "uncited sources went first");
        assert_eq!(context.thread.len(), 16);
        for title in ["Result 2.1", "Result 16.1", "Result 17.4"] {
            assert!(context.artifacts.iter().any(|a| a.title == title));
        }
    }

    #[test]
    fn private_context_never_rides_a_search_query() {
        let private = vec!["Our Q3 revenue target is 4.2M with a hiring freeze".to_owned()];
        assert!(query_discloses(
            "revenue target is 4.2M with a hiring freeze",
            &private
        ));
        assert!(!query_discloses("best canvas libraries 2026", &private));
        assert!(!query_discloses("hiring freeze", &private));
    }
}
