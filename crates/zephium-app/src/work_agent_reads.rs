use super::*;
use zephium_core::work::collection::*;

const MAX_CONCURRENT_PAGE_READS: usize = 3;

struct PendingRead<F> {
    request: WorkAgentBrowseRequest,
    future: Pin<Box<F>>,
    prior: Option<WorkUsage>,
}

impl Driver {
    pub(super) async fn fetch_browses<B, F>(
        &mut self,
        attempt: &WorkNodeAttempt,
        browser: &mut B,
        browses: Vec<WorkStepKindV1>,
    ) -> Result<Option<WorkAttemptStatus>, WorkError>
    where
        B: FnMut(WorkAttemptProbe, WorkAgentBrowseRequest) -> F,
        F: Future<Output = Result<WorkBrowserOutcome, WorkError>>,
    {
        use crate::work_sites::SiteSession;
        let mut reads = Vec::new();
        // Page tasks whose entry question waits on their start page.
        let mut entries: Vec<String> = Vec::new();
        let mut viewing: Vec<String> = Vec::new();
        for kind in browses {
            if matches!(kind, WorkStepKindV1::Discover { .. }) {
                self.notice("Native discovery is not available in this run: provider search covers the web. Use search for facts and read for exact URLs listed in sources.");
                continue;
            }
            let mut kind = job_listing(kind);
            let WorkStepKindV1::Read { url, goal, .. } = &mut kind else {
                continue;
            };
            let Some(site) = crate::work_sites::site_of(url) else {
                continue;
            };
            let session = match goal.clone() {
                Some(goal) => {
                    let view = self.views.contains(url);
                    if let Some(entry) = view
                        .then(|| crate::work_sites::view_home(url))
                        .flatten()
                        .or_else(|| crate::work_sites::entry_url(url))
                    {
                        *url = entry.to_owned();
                    }
                    if view {
                        viewing.push(url.clone());
                    }
                    let enough = remaining_limits(self.limits, self.used).is_some_and(|left| {
                        left.operations >= PAGE_TASK_MIN.operations
                            && left.cost_micro_usd >= PAGE_TASK_MIN.cost_micro_usd
                    });
                    if !enough {
                        match self.keep_going().await? {
                            Ok(true) => {}
                            Ok(false) => {
                                self.notice(BUDGET_EXHAUSTED);
                                continue;
                            }
                            Err(status) => return Ok(Some(status)),
                        }
                    }
                    match self.page_task_session(&site, &goal).await? {
                        Ok(Some(session)) => session,
                        Ok(None) => {
                            entries.push(url.clone());
                            self.sites.tentative(&site)
                        }
                        Err(status) => return Ok(Some(status)),
                    }
                }
                None => self.sites.read_session(&site),
            };
            reads.push((kind, session));
        }
        let cap = usize::from(self.limits.max_workers).clamp(1, MAX_CONCURRENT_PAGE_READS);
        let mut offset = 0;
        while offset < reads.len() {
            let steps = usize::from(self.grant.max_steps).saturating_sub(self.steps as usize + 1);
            if steps == 0 {
                self.notice(STEPS_EXHAUSTED);
                break;
            }
            let Some(remaining) = remaining_limits(self.limits, self.used) else {
                self.notice(BUDGET_EXHAUSTED);
                break;
            };
            // A page task or a page in the person's session runs alone: the
            // engine groups only isolated reads.
            let alone = |(kind, session): &(WorkStepKindV1, SiteSession)| {
                *session != SiteSession::Private
                    || matches!(kind, WorkStepKindV1::Read { goal: Some(_), .. })
            };
            let run = if alone(&reads[offset]) {
                1
            } else {
                reads[offset..]
                    .iter()
                    .take_while(|read| !alone(read))
                    .count()
            };
            let count = cap
                .min(run)
                .min(steps)
                .min(remaining.model_tokens as usize)
                .min(remaining.cost_micro_usd as usize)
                .min(remaining.operations as usize);
            let Some(limits) = allocations(self.limits, self.used, count) else {
                self.notice(BUDGET_EXHAUSTED);
                break;
            };
            let batch = &reads[offset..offset + count];
            let mut pending = Vec::new();
            let mut retries: Vec<(
                WorkAgentBrowseRequest,
                Result<WorkBrowserOutcome, WorkError>,
            )> = Vec::new();
            let mut terminal = None;
            let mut failure = None;
            for ((kind, session), limits) in batch.iter().zip(limits) {
                if self.cancelled().await {
                    terminal = Some(WorkAttemptStatus::Cancelled);
                    break;
                }
                let WorkStepKindV1::Read { url, goal, .. } = kind else {
                    continue;
                };
                let task = goal.is_some();
                let limits = if task {
                    page_task_limits(limits)
                } else {
                    limits
                };
                let mut step = self.step(kind.clone(), WorkStepStatus::Running);
                if *session != SiteSession::Private {
                    step.account = crate::work_sites::host_of(url)
                        .map(|host| Box::new(WorkPageAccountV1 { host, badge: true }));
                }
                let id = match self.begin(step, vec![], None).await {
                    Ok(id) => id,
                    Err(error) => {
                        crate::work_trace::record(format_args!(
                            "work: phase=page event=begin_refused error={error:?}"
                        ));
                        failure = Some(error);
                        break;
                    }
                };
                if let Some(site) = crate::work_sites::site_of(url) {
                    self.page_sites.push((id, site));
                }
                if *session != SiteSession::Private {
                    self.session_steps.push(id);
                    self.report(WorkAgentDiagnostic::SessionPage {
                        task,
                        path: path_class(url),
                    });
                }
                self.probe.record_activity(WorkActivityV1::Reading);
                let board = matches!(kind, WorkStepKindV1::Read { ref url, .. } if job_board(url));
                let request = WorkAgentBrowseRequest {
                    // A board's listings render late: its page gets the longer
                    // loading window from the start, inside the read's budget.
                    construction_attempt: if board {
                        zephium_agentic::WorkBrowserConstructionAttempt::SlowPageRetry
                    } else {
                        Default::default()
                    },
                    id,
                    step: kind.clone(),
                    limits,
                    hops: self.grant.browse_hops,
                    objective: if board {
                        format!("{}\n\n{JOB_BOARD_READING}", self.objective)
                    } else {
                        self.objective.clone()
                    },
                    output: self.output.clone(),
                    session: session.clone(),
                    confirm: task.then(WorkConfirmPort::default),
                    entry: task && entries.contains(&url.clone()),
                    view: task && viewing.contains(url),
                    allow_edits: task
                        && crate::work_sites::site_of(url)
                            .is_some_and(|site| self.sites.edits_allowed(&site)),
                    hold_typing: task
                        && crate::work_sites::site_of(url)
                            .is_some_and(|site| self.typing_held.contains(&site)),
                };
                let future = Box::pin(browser(self.probe.clone(), request.clone()));
                pending.push(PendingRead {
                    request,
                    future,
                    prior: None,
                });
            }
            while !pending.is_empty() || !retries.is_empty() {
                let (request, outcome, prior) = if pending.is_empty() {
                    let (request, outcome) = retries.pop().expect("pending read or retry");
                    if terminal.is_none() && failure.is_none() && !self.cancelled().await {
                        let first = outcome
                            .as_ref()
                            .ok()
                            .and_then(|outcome: &WorkBrowserOutcome| outcome.usage)
                            .expect("retry requires known usage");
                        if let Some(limits) = retry_limits(request.limits, first) {
                            self.report(WorkAgentDiagnostic::ReadRetried);
                            let request = retry_request(request, &outcome, limits);
                            let future = Box::pin(browser(self.probe.clone(), request.clone()));
                            pending.push(PendingRead {
                                request,
                                future,
                                prior: Some(first),
                            });
                            continue;
                        }
                    }
                    (request, outcome, None)
                } else {
                    self.next_read_confirming(&mut pending).await?
                };
                let outcome = checked_outcome(outcome, request.limits);
                // A page task may have drafted something: it is never run
                // twice, unless it ended to start over (a sign-in finished in
                // a tab, or a saved cookie refusal replaced the page).
                let signed_in =
                    matches!(&outcome, Ok(outcome) if outcome.rerun && outcome.usage.is_some());
                let retry = prior.is_none()
                    && terminal.is_none()
                    && failure.is_none()
                    && (signed_in
                        || (!matches!(request.step, WorkStepKindV1::Read { goal: Some(_), .. })
                            && matches!(&outcome, Ok(WorkBrowserOutcome { status: WorkStepStatus::Failed, usage: Some(_), note: Some(note), helped, .. })
                                if (note == read_note::HUMAN_CHECK && *helped) || note == read_note::UNSETTLED || note == read_note::CONSTRUCTION_TIMEOUT)));
                if retry && !pending.is_empty() {
                    retries.push((request, outcome));
                    continue;
                }
                if retry && !self.cancelled().await {
                    let first = outcome
                        .as_ref()
                        .ok()
                        .and_then(|outcome| outcome.usage)
                        .expect("retry requires known usage");
                    if let Some(limits) = retry_limits(request.limits, first) {
                        self.report(WorkAgentDiagnostic::ReadRetried);
                        if signed_in {
                            if let WorkStepKindV1::Read { url, .. } = &request.step {
                                if let Some(site) = crate::work_sites::site_of(url) {
                                    self.sites
                                        .answer(&site, crate::work_sites::EntryAnswer::Allow);
                                }
                            }
                        }
                        let request = retry_request(request, &outcome, limits);
                        let future = Box::pin(browser(self.probe.clone(), request.clone()));
                        pending.push(PendingRead {
                            request,
                            future,
                            prior: Some(first),
                        });
                        continue;
                    }
                }
                match self
                    .settle_fetched(attempt, Fetched::Browse(request.id, outcome, prior))
                    .await
                {
                    Ok(Some(WorkAttemptStatus::OutcomeUnknown)) => {
                        terminal = Some(WorkAttemptStatus::OutcomeUnknown)
                    }
                    Ok(Some(status)) if terminal.is_none() => terminal = Some(status),
                    Err(error) if failure.is_none() => {
                        crate::work_trace::record(format_args!(
                            "work: phase=page event=settle_refused error={error:?}"
                        ));
                        failure = Some(error)
                    }
                    _ => {}
                }
            }
            if let Some(error) = failure {
                return Err(error);
            }
            if terminal.is_some() {
                return Ok(terminal);
            }
            offset += count;
        }
        Ok(None)
    }
}

/// A held step's Confirm step while its page task runs.
struct OpenConfirm {
    port: WorkConfirmPort,
    ask: u32,
    step: WorkStepId,
    site: String,
    /// It holds typing rather than a commit.
    typing: bool,
    decided: bool,
}

impl Driver {
    /// Waits for the next page to settle, meanwhile putting each held step
    /// to the person as a Confirm step and handing back their decision.
    async fn next_read_confirming<F>(
        &mut self,
        pending: &mut Vec<PendingRead<F>>,
    ) -> Result<
        (
            WorkAgentBrowseRequest,
            Result<WorkBrowserOutcome, WorkError>,
            Option<WorkUsage>,
        ),
        WorkError,
    >
    where
        F: Future<Output = Result<WorkBrowserOutcome, WorkError>>,
    {
        let mut open: Vec<OpenConfirm> = Vec::new();
        // An entry question the start page's check put to the person.
        let mut entry: Option<(WorkConfirmPort, WorkStepId, String, Vec<String>)> = None;
        let mut signed_out = false;
        loop {
            let settled = tokio::time::timeout(ASK_POLL, next_read(pending))
                .await
                .ok();
            let port_of = |request: &WorkAgentBrowseRequest| {
                let WorkStepKindV1::Read { url, .. } = &request.step else {
                    return None;
                };
                Some((
                    request.confirm.clone()?,
                    request.id,
                    crate::work_sites::site_of(url)?,
                    crate::work_sites::host_of(url).unwrap_or_default(),
                ))
            };
            let ports: Vec<(WorkConfirmPort, WorkStepId, String, String)> = pending
                .iter()
                .filter_map(|read| port_of(&read.request))
                .chain(
                    settled
                        .as_ref()
                        .and_then(|(request, _, _)| port_of(request)),
                )
                .collect();
            for (port, page, site, host) in ports {
                if port.take_entry_ask() && entry.is_none() {
                    let (prompt, options) =
                        crate::work_sites::entry_question_for(&[crate::work_sites::service_name(
                            &host,
                        )]);
                    self.probe.record_activity(WorkActivityV1::WaitingForHuman);
                    let step = self.step(
                        WorkStepKindV1::Ask {
                            prompt,
                            options: options.clone(),
                            answer: None,
                            purpose: Some(WorkAskPurposeV1::Entry),
                        },
                        WorkStepStatus::Running,
                    );
                    let step = self.begin(step, vec![], None).await?;
                    entry = Some((port.clone(), step, site.clone(), options));
                }
                if let Some(wait) = port.take_needs_you() {
                    self.part_needs_you(wait.map(|wait| (site.as_str(), wait)))
                        .await;
                }
                if port.entry() == Some(WorkSiteEntry::SignedOut) && !signed_out {
                    signed_out = true;
                    self.report(WorkAgentDiagnostic::SiteSignedOut);
                }
                while let Some((ask, confirmation)) = port.take_ask() {
                    let typing = confirmation.category == WorkConfirmCategoryV1::Type;
                    let step = self.confirm_step(page, &site, confirmation).await?;
                    open.push(OpenConfirm {
                        port: port.clone(),
                        ask,
                        step,
                        site: site.clone(),
                        typing,
                        decided: false,
                    });
                }
                while let Some((ask, receipt)) = port.take_settled() {
                    if let Some(at) = open.iter().position(|confirm| confirm.ask == ask) {
                        let confirm = open.swap_remove(at);
                        let (status, note) = match receipt {
                            WorkSiteReceipt::Committed => (WorkStepStatus::Succeeded, None),
                            WorkSiteReceipt::Unverified => (
                                WorkStepStatus::OutcomeUnknown,
                                Some("It ran, but the page did not show that it happened"),
                            ),
                            WorkSiteReceipt::NotSent => (
                                WorkStepStatus::Failed,
                                Some("The page changed before it ran; nothing was sent"),
                            ),
                            WorkSiteReceipt::Declined => (WorkStepStatus::Cancelled, None),
                        };
                        self.settle(
                            confirm.step,
                            status,
                            None,
                            vec![],
                            None,
                            note.map(str::to_owned),
                            None,
                        )
                        .await?;
                    }
                }
            }
            if let Some((port, step, site, options)) =
                entry.clone().filter(|(port, ..)| port.entry().is_none())
            {
                let state = self.probe.runtime_projection().await?;
                let answer = state
                    .executions
                    .iter()
                    .find(|execution| execution.id == self.probe.execution())
                    .and_then(|execution| execution.steps.iter().find(|s| s.id == step))
                    .and_then(|asked| match &asked.kind {
                        WorkStepKindV1::Ask {
                            answer: Some(answer),
                            ..
                        } => Some(answer.clone()),
                        _ => None,
                    });
                if let Some(answer) = answer {
                    let answer = crate::work_sites::entry_answer_to(&options, &answer);
                    self.answer_entry(&site, answer).await;
                    port.answer_entry(match answer {
                        crate::work_sites::EntryAnswer::Allow => WorkSiteEntry::Allow,
                        crate::work_sites::EntryAnswer::Always => WorkSiteEntry::Always,
                        crate::work_sites::EntryAnswer::NotNow => WorkSiteEntry::NotNow,
                    });
                }
            }
            if open.iter().any(|confirm| !confirm.decided) {
                let state = self.probe.runtime_projection().await?;
                let steps = state
                    .executions
                    .iter()
                    .find(|execution| execution.id == self.probe.execution())
                    .map(|execution| execution.steps.clone())
                    .unwrap_or_default();
                for confirm in open.iter_mut().filter(|confirm| !confirm.decided) {
                    let decision = steps.iter().find_map(|step| match &step.kind {
                        WorkStepKindV1::Confirm { confirm: held } if step.id == confirm.step => {
                            held.decision
                        }
                        _ => None,
                    });
                    let Some(decision) = decision else {
                        continue;
                    };
                    confirm.decided = true;
                    confirm.port.decide(
                        confirm.ask,
                        match decision {
                            WorkConfirmDecisionV1::Approved => WorkSiteDecision::Approve,
                            WorkConfirmDecisionV1::AllowedForRun if confirm.typing => {
                                self.typing_held.retain(|site| *site != confirm.site);
                                self.typing_allowed.push(confirm.site.clone());
                                WorkSiteDecision::AllowForRun
                            }
                            WorkConfirmDecisionV1::AllowedForRun => {
                                self.sites.allow_edits(&confirm.site);
                                WorkSiteDecision::AllowForRun
                            }
                            WorkConfirmDecisionV1::Declined => WorkSiteDecision::Decline,
                        },
                    );
                }
            }
            if let Some(read) = settled {
                if let Some((port, step, site, _)) = entry.take() {
                    if port.entry().is_none() {
                        self.settle(
                            step,
                            WorkStepStatus::Cancelled,
                            None,
                            vec![],
                            None,
                            Some("The page closed before you decided".to_owned()),
                            None,
                        )
                        .await?;
                        self.answer_entry(&site, crate::work_sites::EntryAnswer::NotNow)
                            .await;
                    }
                }
                for confirm in open {
                    self.settle(
                        confirm.step,
                        if confirm.decided {
                            WorkStepStatus::Failed
                        } else {
                            WorkStepStatus::Cancelled
                        },
                        None,
                        vec![],
                        None,
                        Some(
                            if confirm.decided {
                                "The page ended before the step ran"
                            } else {
                                "The page closed before you decided"
                            }
                            .to_owned(),
                        ),
                        None,
                    )
                    .await?;
                }
                return Ok(read);
            }
        }
    }

    /// Records a held step for the person; the text's provenance is the
    /// other sites whose pages it quotes.
    /// A part whose page waits on the person says so on its row at once,
    /// and goes back to running when the person is done or the wait ends.
    async fn part_needs_you(&self, site: Option<(&str, crate::work_agent::WorkPageWait)>) {
        let Some(part) = self.part else {
            return;
        };
        let Ok(state) = self.probe.runtime_projection().await else {
            return;
        };
        let Some(mut fact) = state
            .executions
            .iter()
            .find(|execution| execution.id == self.probe.execution())
            .and_then(|execution| execution.parts.iter().find(|fact| fact.id == part))
            .cloned()
            .filter(|fact| match site {
                Some(_) => fact.state == zephium_core::work::parts::WorkPartStateV1::Running,
                None => fact.state == zephium_core::work::parts::WorkPartStateV1::Waiting,
            })
        else {
            return;
        };
        match site {
            Some((site, wait)) => {
                fact.state = zephium_core::work::parts::WorkPartStateV1::Waiting;
                fact.summary = Some(format!("Needs you on {site}"));
                // A sign-in says so on the row, with its fix.
                fact.need = (wait == crate::work_agent::WorkPageWait::SignIn)
                    .then(|| zephium_core::work::parts::WorkPartNeedV1::SignIn {
                        host: site.to_owned(),
                    })
                    .filter(|need| need.validate().is_ok());
            }
            None => {
                fact.state = zephium_core::work::parts::WorkPartStateV1::Running;
                fact.summary = None;
                fact.need = None;
            }
        }
        let _ = self
            .probe
            .commit_step(WorkRuntimeUpdate::Part {
                execution: self.probe.execution(),
                attempt: self.probe.attempt(),
                part: fact,
            })
            .await;
    }

    async fn confirm_step(
        &mut self,
        page: WorkStepId,
        site: &str,
        confirmation: WorkSiteConfirmation,
    ) -> Result<WorkStepId, WorkError> {
        let mut provenance: Vec<String> = Vec::new();
        if let Some(text) = &confirmation.text {
            for (source, body) in &self.private_sites {
                if source != site
                    && !provenance.contains(source)
                    && provenance.len() < 8
                    && query_discloses(text, std::slice::from_ref(body))
                {
                    provenance.push(source.clone());
                }
            }
        }
        self.probe.record_activity(WorkActivityV1::WaitingForHuman);
        let step = self.step(
            WorkStepKindV1::Confirm {
                confirm: Box::new(WorkConfirmV1 {
                    site: site.to_owned(),
                    category: confirmation.category,
                    headline: confirmation.headline,
                    action: confirmation.action,
                    text: confirmation.text,
                    facts: confirmation.facts,
                    page: Some(page),
                    provenance,
                    run_option: confirmation.run_option,
                    decision: None,
                }),
            },
            WorkStepStatus::Running,
        );
        self.begin(step, vec![], None).await
    }
}

/// A page task's own ceiling inside the run's remaining budget: room for
/// sixty actions and forty model calls, and the least it starts with.
const PAGE_TASK_MAX: WorkExecutionLimits = WorkExecutionLimits {
    model_tokens: 600_000,
    cost_micro_usd: 600_000,
    operations: 160,
    timeout_seconds: 540,
    max_workers: 1,
};
const PAGE_TASK_MIN: WorkExecutionLimits = WorkExecutionLimits {
    model_tokens: 40_000,
    cost_micro_usd: 100_000,
    operations: 40,
    timeout_seconds: 60,
    max_workers: 1,
};
fn page_task_limits(share: WorkExecutionLimits) -> WorkExecutionLimits {
    WorkExecutionLimits {
        model_tokens: share.model_tokens.min(PAGE_TASK_MAX.model_tokens),
        cost_micro_usd: share.cost_micro_usd.min(PAGE_TASK_MAX.cost_micro_usd),
        operations: share.operations.min(PAGE_TASK_MAX.operations),
        ..share
    }
}

/// Job boards whose listings a script renders after the page loads. Closed.
const JOB_BOARDS: &[&str] = &[
    "ashbyhq.com",
    "greenhouse.io",
    "lever.co",
    "myworkdayjobs.com",
    "myworkdaysite.com",
    "dropbox.jobs",
];
const JOB_BOARD_READING: &str = "This page is a job board whose listings appear after it loads: wait until its list of open positions is shown, scrolling once if it is not, before extracting, then return one record per listed position with its title as the name, its location and its link.";
const JOB_LISTINGS: u8 = 32;

fn job_board(url: &str) -> bool {
    url::Url::parse(url)
        .ok()
        .filter(|url| url.scheme() == "https")
        .and_then(|url| url.host_str().map(str::to_ascii_lowercase))
        .is_some_and(|host| {
            JOB_BOARDS.iter().any(|board| {
                host == *board
                    || host
                        .strip_suffix(board)
                        .is_some_and(|rest| rest.ends_with('.'))
            })
        })
}

/// A board read the model left without records extracts each listing as
/// one: title, location and link. A collection the model chose stands.
fn job_listing(kind: WorkStepKindV1) -> WorkStepKindV1 {
    match kind {
        WorkStepKindV1::Read {
            url,
            collection: None,
            goal: None,
        } if job_board(&url) => WorkStepKindV1::Read {
            url,
            goal: None,
            collection: Some(WorkBrowseCollection {
                title: "Open positions".into(),
                columns: vec![
                    WorkBrowseColumn {
                        name: "location".into(),
                        value: WorkBrowseValue::Text,
                        required: false,
                        extraction: WorkBrowseExtraction::Verbatim,
                    },
                    WorkBrowseColumn {
                        name: "url".into(),
                        value: WorkBrowseValue::Url,
                        required: false,
                        extraction: WorkBrowseExtraction::Verbatim,
                    },
                ],
                max_items: JOB_LISTINGS,
            }),
        },
        kind => kind,
    }
}

fn retry_request(
    request: WorkAgentBrowseRequest,
    outcome: &Result<WorkBrowserOutcome, WorkError>,
    limits: WorkExecutionLimits,
) -> WorkAgentBrowseRequest {
    let construction_attempt = if request.session == crate::work_sites::SiteSession::Private
        && matches!(outcome, Ok(WorkBrowserOutcome { note: Some(note), .. }) if note == read_note::CONSTRUCTION_TIMEOUT)
    {
        zephium_agentic::WorkBrowserConstructionAttempt::SlowPageRetry
    } else {
        request.construction_attempt
    };
    // A sign-in the person finished counts as their yes for the site.
    let entry = request.entry && !matches!(outcome, Ok(outcome) if outcome.rerun);
    WorkAgentBrowseRequest {
        limits,
        construction_attempt,
        entry,
        ..request
    }
}

async fn next_read<F>(
    pending: &mut Vec<PendingRead<F>>,
) -> (
    WorkAgentBrowseRequest,
    Result<WorkBrowserOutcome, WorkError>,
    Option<WorkUsage>,
)
where
    F: Future<Output = Result<WorkBrowserOutcome, WorkError>>,
{
    std::future::poll_fn(|cx| {
        for index in 0..pending.len() {
            if let Poll::Ready(outcome) = pending[index].future.as_mut().poll(cx) {
                let read = pending.swap_remove(index);
                return Poll::Ready((read.request, outcome, read.prior));
            }
        }
        Poll::Pending
    })
    .await
}

fn allocations(
    limits: WorkExecutionLimits,
    used: WorkUsage,
    count: usize,
) -> Option<Vec<WorkExecutionLimits>> {
    if count > MAX_CONCURRENT_PAGE_READS {
        return None;
    }
    budget_shares(limits, used, count)
}

pub(super) fn budget_shares(
    limits: WorkExecutionLimits,
    used: WorkUsage,
    count: usize,
) -> Option<Vec<WorkExecutionLimits>> {
    if count == 0 || count > usize::from(limits.max_workers) {
        return None;
    }
    let remaining = remaining_limits(limits, used)?;
    let count = count as u32;
    if remaining.model_tokens < count
        || remaining.cost_micro_usd < count
        || remaining.operations < count
    {
        return None;
    }
    let share = |value: u32, index| value / count + u32::from(index < value % count);
    Some(
        (0..count)
            .map(|index| WorkExecutionLimits {
                model_tokens: share(remaining.model_tokens, index),
                cost_micro_usd: share(remaining.cost_micro_usd, index),
                operations: share(remaining.operations, index),
                max_workers: 1,
                ..remaining
            })
            .collect(),
    )
}

fn retry_limits(limits: WorkExecutionLimits, used: WorkUsage) -> Option<WorkExecutionLimits> {
    remaining_limits(
        limits,
        WorkUsage {
            operations: used.operations.max(1),
            ..used
        },
    )
}

pub(super) fn remaining_limits(
    limits: WorkExecutionLimits,
    used: WorkUsage,
) -> Option<WorkExecutionLimits> {
    let limits = WorkExecutionLimits {
        model_tokens: limits.model_tokens.checked_sub(used.model_tokens)?,
        cost_micro_usd: limits.cost_micro_usd.checked_sub(used.cost_micro_usd)?,
        operations: limits.operations.checked_sub(used.operations)?,
        max_workers: 1,
        ..limits
    };
    limits.validate().ok().map(|()| limits)
}

fn checked_outcome(
    outcome: Result<WorkBrowserOutcome, WorkError>,
    limits: WorkExecutionLimits,
) -> Result<WorkBrowserOutcome, WorkError> {
    match outcome {
        Ok(outcome) if outcome.usage.is_none_or(|usage| !usage.within(limits)) => {
            Err(WorkError::OutcomeUnknown)
        }
        outcome => outcome,
    }
}

pub(super) fn combined_usage(
    prior: Option<WorkUsage>,
    latest: Option<WorkUsage>,
) -> Option<WorkUsage> {
    match (prior, latest) {
        (None, latest) => latest,
        (Some(prior), Some(latest)) => Some(WorkUsage {
            model_tokens: prior.model_tokens.checked_add(latest.model_tokens)?,
            cost_micro_usd: prior.cost_micro_usd.checked_add(latest.cost_micro_usd)?,
            operations: prior
                .operations
                .max(1)
                .checked_add(latest.operations.max(1))?,
            accounting: if prior.accounting == WorkUsageAccounting::ConservativeReservation
                || latest.accounting == WorkUsageAccounting::ConservativeReservation
            {
                WorkUsageAccounting::ConservativeReservation
            } else {
                WorkUsageAccounting::Exact
            },
        }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn work_job_boards_read_their_listings_as_records() {
        // The three boards of the person's run, and the other closed hosts.
        for url in [
            "https://jobs.ashbyhq.com/dropbox",
            "https://boards.greenhouse.io/dropbox",
            "https://job-boards.greenhouse.io/figma/jobs/123",
            "https://jobs.lever.co/acme",
            "https://acme.wd5.myworkdayjobs.com/en-US/careers",
            "https://www.dropbox.jobs/en/jobs",
            "https://dropbox.jobs/",
        ] {
            assert!(job_board(url), "{url}");
            let WorkStepKindV1::Read {
                collection: Some(collection),
                ..
            } = job_listing(WorkStepKindV1::Read {
                url: url.into(),
                collection: None,
                goal: None,
            })
            else {
                panic!("a board read carries its listings");
            };
            collection.validate().unwrap();
            let columns: Vec<_> = collection.columns.iter().map(|c| c.name.as_str()).collect();
            assert_eq!(columns, ["location", "url"]);
            assert_eq!(collection.max_items, JOB_LISTINGS);
        }
        for url in [
            "https://greenhouse.io.evil.test/jobs",
            "https://notlever.co/jobs",
            "http://jobs.lever.co/acme",
            "https://www.lego.com/en-us/themes/star-wars",
        ] {
            assert!(!job_board(url), "{url}");
            let kind = WorkStepKindV1::Read {
                url: url.into(),
                collection: None,
                goal: None,
            };
            assert_eq!(job_listing(kind.clone()), kind);
        }
        // A collection the model chose is its own.
        let chosen = WorkStepKindV1::Read {
            url: "https://jobs.lever.co/acme".into(),
            collection: Some(WorkBrowseCollection {
                title: "Roles".into(),
                columns: vec![WorkBrowseColumn {
                    name: "team".into(),
                    value: WorkBrowseValue::Text,
                    required: false,
                    extraction: WorkBrowseExtraction::Generate,
                }],
                max_items: 5,
            }),
            goal: None,
        };
        assert_eq!(job_listing(chosen.clone()), chosen);
    }

    #[test]
    fn work_read_budget_shares_preserve_the_original_reservation() {
        let limits = WorkExecutionLimits {
            model_tokens: 10,
            cost_micro_usd: 14,
            operations: 8,
            timeout_seconds: 30,
            max_workers: 4,
        };
        let shares = allocations(limits, WorkUsage::default(), 3).unwrap();
        assert_eq!(
            shares.iter().map(|share| share.model_tokens).sum::<u32>(),
            10
        );
        assert_eq!(
            shares.iter().map(|share| share.cost_micro_usd).sum::<u32>(),
            14
        );
        assert_eq!(shares.iter().map(|share| share.operations).sum::<u32>(), 8);
        assert!(shares
            .iter()
            .all(|share| share.max_workers == 1 && share.timeout_seconds == 30));
        assert!(allocations(limits, WorkUsage::default(), 4).is_none());
        assert!(allocations(
            limits,
            WorkUsage {
                operations: 8,
                ..WorkUsage::default()
            },
            1
        )
        .is_none());
        assert_eq!(
            retry_limits(shares[0], WorkUsage::default())
                .unwrap()
                .operations
                + 1,
            shares[0].operations
        );
        for usage in [
            None,
            Some(WorkUsage {
                model_tokens: 11,
                ..WorkUsage::default()
            }),
            Some(WorkUsage {
                cost_micro_usd: 15,
                ..WorkUsage::default()
            }),
            Some(WorkUsage {
                operations: 9,
                ..WorkUsage::default()
            }),
        ] {
            let outcome = WorkBrowserOutcome {
                status: WorkStepStatus::Succeeded,
                usage,
                artifacts: vec![],
                intervention: None,
                note: None,
                measurements: None,
                helped: false,
                held_back: false,
                rerun: false,
            };
            assert!(matches!(
                checked_outcome(Ok(outcome), limits),
                Err(WorkError::OutcomeUnknown)
            ));
        }
        assert!(combined_usage(Some(WorkUsage::default()), None).is_none());
    }
}
