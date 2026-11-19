use super::*;
use zephium_core::work::collection::*;
use zephium_core::work::parts::{
    WorkPartFactV1, WorkPartNeedV1, WorkPartReasonV1, WorkPartStateV1,
};

const MAX_CONCURRENT_PAGE_READS: usize = 3;
/// New reads one public page task may become after a first-load redirect.
pub(crate) const MAX_REDIRECT_HOPS: usize = 2;
/// How often a page read looks at the steps its pages hold for the person.
const HELD_POLL: Duration = Duration::from_millis(500);

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
        let state = self.probe.runtime_projection().await?;
        let blockers = WorkAgentService::site_blockers(&state);
        let mut retrying = std::collections::BTreeSet::new();
        let mut retry_claims = Vec::new();
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
            if let Some(blocked) = blockers.iter().find(|blocked| blocked.site == site) {
                if !retrying.insert(site.clone()) {
                    self.notice(&blocked.notice());
                    continue;
                }
                let Some(claim) = SiteRetryClaim::acquire(
                    self.profile,
                    state.work.id,
                    self.probe.execution(),
                    &site,
                ) else {
                    self.notice(&blocked.notice());
                    continue;
                };
                retry_claims.push(claim);
                // Another part may have asked while this batch was being built.
                let fresh = self.probe.runtime_projection().await?;
                let decision = site_retry(&fresh, self.probe.execution(), blocked);
                // A request that names the site again ("Try Slack again") is
                // the person's retry: its page task opens, and a wall there
                // hands the page to them. Otherwise the recorded need stands
                // without a question; a research read never reopens it.
                let allowed = decision.unwrap_or_else(|| {
                    goal.is_some()
                        && attempt
                            .disclosure_objective()
                            .is_ok_and(|objective| names_site(&objective, &site))
                });
                if !allowed {
                    self.notice(&blocked.notice());
                    continue;
                }
            }
            let session = match goal.clone() {
                Some(goal) => {
                    let view = self.views.contains(url);
                    if let Some(entry) = view
                        .then(|| crate::work_sites::view_home(url))
                        .flatten()
                        .or_else(|| {
                            crate::work_sites::entry_url(url)
                                .filter(|_| view || !crate::work_sites::public_app(url))
                        })
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
        let mut capacity_races = 0;
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
            let mut consumed = 0;
            let mut at_capacity = false;
            // Each live read's step and the addresses its chain has read.
            let mut chains: Vec<(WorkStepId, Vec<String>)> = Vec::new();
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
                    Err(WorkError::Capacity) => {
                        at_capacity = true;
                        break;
                    }
                    // One page the Store will not record is that page's
                    // problem, not the run's: the others still read.
                    Err(WorkError::Invalid) => {
                        crate::work_trace::record(format_args!(
                            "work: phase=page event=begin_refused error=Invalid"
                        ));
                        self.notice(&format!(
                            "A page could not start and was skipped: {}",
                            crate::work_sites::host_of(url).unwrap_or_default()
                        ));
                        consumed += 1;
                        continue;
                    }
                    Err(error) => {
                        crate::work_trace::record(format_args!(
                            "work: phase=page event=begin_refused error={error:?}"
                        ));
                        failure = Some(error);
                        break;
                    }
                };
                consumed += 1;
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
                    image_entry_blocked: crate::work_sites::site_of(url)
                        .is_some_and(|site| self.sites.image_entry_blocked(&site)),
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
                    // Every page says when it waits on the person; only a
                    // task's page asks anything through its port.
                    confirm: Some(WorkConfirmPort::default()),
                    entry: task && entries.contains(&url.clone()),
                    view: task && viewing.contains(url),
                    allow_edits: task
                        && crate::work_sites::site_of(url)
                            .is_some_and(|site| self.sites.edits_allowed(&site)),
                    hold_typing: task
                        && crate::work_sites::site_of(url)
                            .is_some_and(|site| self.typing_held.contains(&site)),
                    results: None,
                    redirected_from: None,
                    private_run: self.grant.private,
                };
                chains.push((id, vec![url.clone()]));
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
                            .unwrap_or_default();
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
                // twice, unless it ended to start over (the person signed in
                // or passed a check on it, or a saved cookie refusal replaced
                // the page) or lost its page before acting.
                let signed_in = matches!(&outcome, Ok(outcome) if starts_over(outcome));
                let retry = prior.is_none()
                    && terminal.is_none()
                    && failure.is_none()
                    && (signed_in || retryable_read(&request.step, &outcome));
                if retry && !pending.is_empty() {
                    retries.push((request, outcome));
                    continue;
                }
                if retry && !self.cancelled().await {
                    let first = outcome
                        .as_ref()
                        .ok()
                        .and_then(|outcome| outcome.usage)
                        .unwrap_or_default();
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
                let redirect = outcome
                    .as_ref()
                    .ok()
                    .and_then(|outcome| outcome.redirect.clone().zip(outcome.usage));
                let id = request.id;
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
                let Some((destination, used)) = redirect else {
                    continue;
                };
                if terminal.is_some() || failure.is_some() || self.cancelled().await {
                    continue;
                }
                let read = chains
                    .iter()
                    .find(|(step, _)| *step == id)
                    .map(|(_, read)| read.clone())
                    .unwrap_or_default();
                match self
                    .redirect_read(&request, &destination, used, &read)
                    .await
                {
                    Ok(Some(next)) => {
                        let mut read = read;
                        read.push(destination);
                        chains.push((next.id, read));
                        let future = Box::pin(browser(self.probe.clone(), next.clone()));
                        pending.push(PendingRead {
                            request: next,
                            future,
                            prior: None,
                        });
                    }
                    Ok(None) => {}
                    Err(error) => failure = Some(error),
                }
            }
            if let Some(error) = failure {
                return Err(error);
            }
            if terminal.is_some() {
                return Ok(terminal);
            }
            offset += consumed;
            if consumed > 0 {
                capacity_races = 0;
            }
            if at_capacity
                && consumed == 0
                && !self.wait_for_step_capacity(&mut capacity_races).await?
            {
                return Ok(Some(WorkAttemptStatus::Cancelled));
            }
        }
        Ok(None)
    }
}

impl Driver {
    /// A new read of the address a public page task's first load was
    /// redirected to: its own step citing that address. A cross-site successor
    /// is a research read in fresh isolated storage, never an account/task replay,
    /// within what the first read left of its share, never an address the
    /// chain already read, and at most `MAX_REDIRECT_HOPS` of them. The
    /// host still admits it as it would a read asked for directly.
    /// `None` leaves the redirect with the failed read's step.
    async fn redirect_read(
        &mut self,
        request: &WorkAgentBrowseRequest,
        destination: &str,
        used: WorkUsage,
        read: &[String],
    ) -> Result<Option<WorkAgentBrowseRequest>, WorkError> {
        let WorkStepKindV1::Read { url, .. } = &request.step else {
            return Ok(None);
        };
        let kind = redirected_kind(&request.step, destination);
        let same_site = zephium_agentic::ContextNavigationTarget::parse(url)
            .ok()
            .zip(zephium_agentic::ContextNavigationTarget::parse(destination).ok())
            .is_some_and(|(from, to)| zephium_agentic::same_work_site(&from, &to));
        let followed = read.len().saturating_sub(1);
        let follows = kind.is_some()
            && request.results.is_none()
            && request.session == crate::work_sites::SiteSession::Private
            && followed < MAX_REDIRECT_HOPS
            && !read.iter().any(|known| known == destination)
            && usize::from(self.grant.max_steps) > self.steps as usize + 1;
        crate::work_trace::record(format_args!(
            "work: phase=page event=redirected same_site={same_site} hop={followed} followed={follows}"
        ));
        let Some(limits) = retry_limits(request.limits, used).filter(|_| follows) else {
            return Ok(None);
        };
        let Some(kind) = kind else {
            return Ok(None);
        };
        let id = self
            .begin(
                self.step(kind.clone(), WorkStepStatus::Running),
                vec![],
                None,
            )
            .await?;
        if let Some(site) = crate::work_sites::site_of(destination) {
            self.page_sites.push((id, site));
        }
        if let Some(redirect) = self
            .redirects
            .iter_mut()
            .rev()
            .find(|(step, _, _)| *step == request.id)
        {
            redirect.2 = Some(id);
        }
        self.probe.record_activity(WorkActivityV1::Reading);
        Ok(Some(WorkAgentBrowseRequest {
            image_entry_blocked: crate::work_sites::site_of(destination)
                .is_some_and(|site| self.sites.image_entry_blocked(&site)),
            construction_attempt: Default::default(),
            id,
            step: kind,
            limits,
            redirected_from: Some(url.clone()),
            // A different site never inherits Work's saved website data.
            private_run: request.private_run || !same_site,
            ..request.clone()
        }))
    }
}

fn redirected_kind(step: &WorkStepKindV1, destination: &str) -> Option<WorkStepKindV1> {
    let WorkStepKindV1::Read {
        url,
        goal,
        verbatim,
        collection,
    } = step
    else {
        return None;
    };
    let from = zephium_agentic::ContextNavigationTarget::parse(url).ok()?;
    let to = zephium_agentic::ContextNavigationTarget::parse(destination).ok()?;
    let public_destination =
        zephium_agentic::AgentNavigationDiscovery::try_new_public_page(to.clone()).is_ok();
    let public_read = goal.is_none()
        && zephium_agentic::AgentNavigationDiscovery::try_new_public_page(from.clone()).is_ok();
    (public_destination && (zephium_agentic::same_work_site(&from, &to) || public_read)).then(
        || WorkStepKindV1::Read {
            verbatim: verbatim.clone(),
            url: destination.to_owned(),
            goal: goal.clone(),
            collection: collection.clone(),
        },
    )
}

// Short-lived coordination only: durable Ask/Read facts remain authoritative.
// Keep one opener per site while concurrent parts prepare their own batches.
type RetryKey = (ProfileId, WorkId, WorkExecutionId, String);
fn retry_claims() -> &'static std::sync::Mutex<Vec<RetryKey>> {
    static CLAIMS: std::sync::OnceLock<std::sync::Mutex<Vec<RetryKey>>> =
        std::sync::OnceLock::new();
    CLAIMS.get_or_init(Default::default)
}
struct SiteRetryClaim(RetryKey);
impl SiteRetryClaim {
    fn acquire(
        profile: ProfileId,
        work: WorkId,
        execution: WorkExecutionId,
        site: &str,
    ) -> Option<Self> {
        let key = (profile, work, execution, site.to_owned());
        let mut claims = retry_claims()
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if claims.len() >= 64 || claims.contains(&key) {
            return None;
        }
        claims.push(key.clone());
        Some(Self(key))
    }
}
impl Drop for SiteRetryClaim {
    fn drop(&mut self) {
        retry_claims()
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .retain(|key| *key != self.0);
    }
}

/// Whether the person's words name a site: its address, a known service's
/// name, or the site's own name (nvidia for nvidia.com).
fn names_site(text: &str, site: &str) -> bool {
    let lower = text.to_lowercase();
    let name = site.split('.').next().unwrap_or(site);
    lower.contains(site)
        || crate::work_sites::named_services(text)
            .iter()
            .any(|named| named == site)
        || lower
            .split(|c: char| !c.is_alphanumeric())
            .any(|word| word == name)
}

const OPEN_BLOCKED_SITE: &str = "Open the page";
const DEFER_BLOCKED_SITE: &str = "Not now";

fn retry_prompt(site: &str) -> String {
    format!("Work could not access {site}. Open its page to try again or resolve the sign-in or human check?")
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SiteBlockKind {
    SignIn,
    Check,
    Unreachable,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct WorkSiteBlocker {
    pub(crate) site: String,
    url: String,
    execution: WorkExecutionId,
    kind: SiteBlockKind,
    account: Option<Box<WorkPageAccountV1>>,
    precise: bool,
    settled_revision: Option<WorkRevision>,
}
impl WorkSiteBlocker {
    pub(crate) fn need(&self) -> WorkPartNeedV1 {
        match self.kind {
            SiteBlockKind::SignIn => WorkPartNeedV1::SignIn {
                host: self.site.clone(),
            },
            SiteBlockKind::Check => WorkPartNeedV1::Check {
                host: self.site.clone(),
            },
            SiteBlockKind::Unreachable => WorkPartNeedV1::Retry {
                host: Some(self.site.clone()),
                reason: Some(WorkPartReasonV1::SiteError),
            },
        }
    }
    fn reason(&self) -> &'static str {
        match self.kind {
            SiteBlockKind::SignIn => "needs sign-in",
            SiteBlockKind::Check => "needs a human check",
            SiteBlockKind::Unreachable => "was unavailable",
        }
    }
    pub(crate) fn notice(&self) -> String {
        let reason = self.reason();
        format!("{} {reason}, recorded earlier in this Work. Keep that need visible. Do not reopen its pages without the person's explicit retry; opening a page is not proof the problem is resolved. Public help/docs are not evidence of personal account content. You can explain this recorded failure without opening any page.", self.site)
    }
}

impl WorkAgentService {
    /// Fold bounded durable executions; no mutable process cache or new storage.
    pub(crate) fn site_blockers(state: &WorkRuntimeProjection) -> Vec<WorkSiteBlocker> {
        let mut executions: Vec<_> = state.executions.iter().collect();
        executions.sort_by_key(|execution| execution.approved_revision);
        fold_site_blockers(executions.into_iter().map(|execution| {
            (
                execution.id,
                execution.steps.as_slice(),
                execution.parts.as_slice(),
            )
        }))
    }

    pub(crate) fn site_blocker_context(state: &WorkRuntimeProjection) -> String {
        let blocked = Self::site_blockers(state);
        if blocked.is_empty() {
            return String::new();
        }
        let mut text = "\nUnresolved site access in this Work (durable observations):\nExplain these facts without opening pages. A site opens again only when the person asks for it again (names it, or Try again on its row); that is not resolution. Public help/docs do not stand for personal account content.\n".to_owned();
        for site in blocked.iter().take(16) {
            text.push_str(&format!("{}: {}", site.site, site.reason()));
            text.push('\n');
        }
        if blocked.len() > 16 {
            text.push_str("Additional sites remain blocked; each attempted read checks their durable state.\n");
        }
        text
    }
}

fn same_target(left: &str, right: &str) -> bool {
    let target = |value: &str| {
        url::Url::parse(value).ok().map(|mut url| {
            url.set_fragment(None);
            url
        })
    };
    target(left)
        .zip(target(right))
        .is_some_and(|(left, right)| left == right)
}

fn fold_site_blockers<'a>(
    executions: impl IntoIterator<Item = (WorkExecutionId, &'a [WorkStepFact], &'a [WorkPartFactV1])>,
) -> Vec<WorkSiteBlocker> {
    let mut blocked: Vec<WorkSiteBlocker> = Vec::new();
    for (execution, steps, parts) in executions {
        let mut ordered: Vec<_> = steps.iter().collect();
        ordered.sort_by_key(|step| read_settlement_order(step));
        for step in ordered {
            let WorkStepKindV1::Read { url, goal, .. } = &step.kind else {
                continue;
            };
            let Some(site) = crate::work_sites::site_of(url) else {
                continue;
            };
            let measurements = step.measurements;
            let settled_revision = measurements.and_then(|m| m.settled_revision);
            blocked.retain(|prior| {
                let later = prior.execution != execution
                    || prior
                        .settled_revision
                        .zip(settled_revision)
                        .is_some_and(|(before, after)| before < after);
                if !later
                    || prior.site != site
                    || !same_target(&prior.url, url)
                    || prior.account != step.account
                {
                    return true;
                }
                let receipt = measurements.and_then(|m| m.resolved_handoff).is_some_and(
                    |receipt| match prior.kind {
                        SiteBlockKind::SignIn => receipt.includes(WorkPageHandoffKindV1::SignIn),
                        SiteBlockKind::Check => receipt.includes(WorkPageHandoffKindV1::Check),
                        SiteBlockKind::Unreachable => false,
                    },
                );
                let read_target = step.status == WorkStepStatus::Succeeded
                    && measurements
                        .is_some_and(|m| m.outcome == Some(WorkPageOutcomeV1::Completed))
                    && !step.artifacts.is_empty()
                    && (prior.account.is_none() || goal.is_some());
                !(receipt || read_target)
            });
            let kind = match (step.status, measurements.and_then(|m| m.outcome)) {
                (
                    WorkStepStatus::Failed
                    | WorkStepStatus::OutcomeUnknown
                    | WorkStepStatus::Cancelled,
                    Some(WorkPageOutcomeV1::NeedsSignIn),
                ) => Some(SiteBlockKind::SignIn),
                (
                    WorkStepStatus::Failed
                    | WorkStepStatus::OutcomeUnknown
                    | WorkStepStatus::Cancelled,
                    Some(WorkPageOutcomeV1::NeedsCheck),
                ) => Some(SiteBlockKind::Check),
                (
                    WorkStepStatus::Failed | WorkStepStatus::OutcomeUnknown,
                    Some(WorkPageOutcomeV1::SiteError),
                ) => Some(SiteBlockKind::Unreachable),
                // Only old settled rows without a typed outcome use closed notes.
                (WorkStepStatus::Failed | WorkStepStatus::OutcomeUnknown, None) => {
                    match step.note.as_deref() {
                        Some("The page asked to sign in") => Some(SiteBlockKind::SignIn),
                        Some(note) if read_note::is_human_check(note) => Some(SiteBlockKind::Check),
                        Some(read_note::SLOW_SITE) => Some(SiteBlockKind::Unreachable),
                        _ => None,
                    }
                }
                _ => None,
            };
            let precise = kind.is_some();
            let kind = kind.or_else(|| waiting_part_wall(step, steps, parts, &site));
            if let Some(kind) = kind {
                // Coarse part state cannot relocate or replace a precise native wall.
                if !precise
                    && blocked
                        .iter()
                        .any(|prior| prior.site == site && prior.precise)
                {
                    for prior in blocked.iter_mut().filter(|prior| {
                        prior.site == site
                            && same_target(&prior.url, url)
                            && prior.account == step.account
                            && prior.kind == kind
                    }) {
                        prior.execution = execution;
                        prior.settled_revision = None;
                    }
                    continue;
                }
                if kind == SiteBlockKind::Unreachable
                    && blocked.iter().any(|prior| {
                        prior.site == site
                            && same_target(&prior.url, url)
                            && prior.account == step.account
                            && prior.kind != kind
                    })
                {
                    continue;
                }
                // A new ordered failure cannot erase uncertainty about another
                // unsequenced observation in this same execution.
                let unresolved_order = blocked.iter().any(|prior| {
                    prior.execution == execution
                        && prior.site == site
                        && same_target(&prior.url, url)
                        && prior.account == step.account
                        && prior.kind == kind
                        && prior.settled_revision.is_none()
                });
                blocked.retain(|prior| {
                    !(prior.site == site
                        && same_target(&prior.url, url)
                        && prior.account == step.account
                        && prior.kind == kind)
                });
                blocked.push(WorkSiteBlocker {
                    site,
                    url: url.clone(),
                    execution,
                    kind,
                    account: step.account.clone(),
                    precise,
                    settled_revision: if unresolved_order {
                        None
                    } else {
                        settled_revision
                    },
                });
            }
        }
    }
    blocked
}

/// Immediate Stop can close the Read before its native wall outcome arrives.
/// Preserve only its own Waiting/Stopped part's typed handoff, at this Read's
/// position in history, so a later same-site resolution still wins.
fn waiting_part_wall(
    step: &WorkStepFact,
    steps: &[WorkStepFact],
    parts: &[WorkPartFactV1],
    site: &str,
) -> Option<SiteBlockKind> {
    if !matches!(
        step.status,
        WorkStepStatus::Running | WorkStepStatus::OutcomeUnknown | WorkStepStatus::Cancelled
    ) || step
        .measurements
        .is_some_and(|m| m.resolved_handoff.is_some())
    {
        return None;
    }
    let part = parts.iter().find(|part| {
        Some(part.id) == step.part
            && matches!(
                part.state,
                WorkPartStateV1::Waiting | WorkPartStateV1::Stopped
            )
    })?;
    // A part may have several pages. Its coarse need cannot identify which
    // uncertain page held the wall, so never guess a target in that case.
    if steps
        .iter()
        .filter(|candidate| {
            candidate.part == Some(part.id)
                && matches!(
                    candidate.status,
                    WorkStepStatus::Running
                        | WorkStepStatus::OutcomeUnknown
                        | WorkStepStatus::Cancelled
                )
                && candidate
                    .measurements
                    .is_none_or(|m| m.resolved_handoff.is_none())
                && matches!(&candidate.kind, WorkStepKindV1::Read { url, .. }
                if crate::work_sites::site_of(url).as_deref() == Some(site))
        })
        .count()
        != 1
    {
        return None;
    }
    let (host, kind) = match part.need.as_ref()? {
        WorkPartNeedV1::SignIn { host } => (host, SiteBlockKind::SignIn),
        WorkPartNeedV1::Check { host } => (host, SiteBlockKind::Check),
        _ => return None,
    };
    (crate::work_sites::site_of(&format!("https://{host}/")).as_deref() == Some(site))
        .then_some(kind)
}

/// One explicit page-opening decision per site and follow-up. It grants an
/// attempt, never clears a blocker; only resulting native/read evidence can.
fn site_retry(
    state: &WorkRuntimeProjection,
    current: WorkExecutionId,
    blocked: &WorkSiteBlocker,
) -> Option<bool> {
    let execution = state
        .executions
        .iter()
        .find(|execution| execution.id == current)?;
    site_retry_steps(current, blocked, &execution.steps)
}

fn site_retry_steps(
    current: WorkExecutionId,
    blocked: &WorkSiteBlocker,
    steps: &[WorkStepFact],
) -> Option<bool> {
    if blocked.execution == current || steps.iter().any(|step| {
        matches!(&step.kind, WorkStepKindV1::Read { url, .. } if crate::work_sites::site_of(url).as_deref() == Some(blocked.site.as_str()))
    }) { return Some(false); }
    steps.iter().rev().find_map(|step| match &step.kind {
        WorkStepKindV1::Ask {
            prompt,
            options,
            answer,
            purpose: Some(WorkAskPurposeV1::Confirm),
        } if *prompt == retry_prompt(&blocked.site)
            && options == &[OPEN_BLOCKED_SITE.to_owned(), DEFER_BLOCKED_SITE.to_owned()] =>
        {
            Some(
                step.status == WorkStepStatus::Succeeded
                    && answer.as_deref() == Some(OPEN_BLOCKED_SITE),
            )
        }
        _ => None,
    })
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
        // The person's answers are durable: the run's projection is read
        // when the work changed, not on every look at the pages.
        let mut changes = self.probe.changes();
        let mut looked = Instant::now();
        loop {
            let settled = tokio::time::timeout(HELD_POLL, next_read(pending))
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
            // These tickets originate only from the native screenshot fallback,
            // not from a helper-authored Ask or the site's account-entry choice.
            let screenshot_pages: Vec<_> = pending.iter().map(|read| read.request.id).collect();
            while let Some(ticket) = self
                .probe
                .screenshot_choices()
                .take_check(&screenshot_pages)
            {
                ticket.check(&self.handle).await;
            }
            while let Some(ticket) = self
                .probe
                .screenshot_choices()
                .take_question(&screenshot_pages)
            {
                self.probe.record_activity(WorkActivityV1::WaitingForHuman);
                let fact = self.step(
                    WorkStepKindV1::Ask {
                        prompt: ticket.origin().to_owned(),
                        options: vec!["Allow".into(), "Always allow".into(), "Not now".into()],
                        answer: None,
                        purpose: Some(WorkAskPurposeV1::Screenshot),
                    },
                    WorkStepStatus::Running,
                );
                let step = match self.begin(fact, vec![], None).await {
                    Ok(step) => step,
                    Err(error) => {
                        ticket.deny();
                        return Err(error);
                    }
                };
                if !ticket.bind(step) {
                    ticket.deny();
                    self.settle(
                        step,
                        WorkStepStatus::Cancelled,
                        None,
                        vec![],
                        None,
                        Some("The page closed before you decided".into()),
                        None,
                    )
                    .await?;
                }
            }
            let look = changes.take() || looked.elapsed() >= crate::work_changes::FALLBACK;
            if look {
                looked = Instant::now();
            }
            if let Some((port, step, site, options)) = entry
                .clone()
                .filter(|(port, ..)| look && port.entry().is_none())
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
            if look {
                let questions = self.probe.screenshot_choices().questions(&screenshot_pages);
                if !questions.is_empty() {
                    let projection = self.probe.runtime_projection().await?;
                    let steps = projection
                        .executions
                        .iter()
                        .find(|execution| execution.id == self.probe.execution())
                        .map(|execution| execution.steps.as_slice())
                        .unwrap_or_default();
                    for ticket in questions {
                        let Some(step) = ticket.step() else {
                            continue;
                        };
                        let Some(fact) = steps.iter().find(|fact| fact.id == step) else {
                            ticket.deny();
                            continue;
                        };
                        let answer = match &fact.kind {
                            WorkStepKindV1::Ask {
                                prompt,
                                options,
                                answer: Some(answer),
                                purpose: Some(WorkAskPurposeV1::Screenshot),
                            } if fact.status == WorkStepStatus::Succeeded
                                && prompt == ticket.origin()
                                && options
                                    == &[
                                        "Allow".to_owned(),
                                        "Always allow".to_owned(),
                                        "Not now".to_owned(),
                                    ] =>
                            {
                                use crate::work_screenshots::consent::ScreenshotAnswer;
                                Some(match answer.as_str() {
                                    "Allow" => ScreenshotAnswer::Allow,
                                    "Always allow" => ScreenshotAnswer::Always,
                                    _ => ScreenshotAnswer::NotNow,
                                })
                            }
                            _ => None,
                        };
                        if let Some(answer) = answer {
                            ticket.answer(&self.handle, step, answer).await;
                            ticket.question_closed();
                        } else if fact.status != WorkStepStatus::Running {
                            ticket.deny();
                            ticket.question_closed();
                        } else if ticket.current()
                            == crate::work_screenshots::consent::ScreenshotChoice::Denied
                        {
                            self.cancel_screenshot_question(&ticket).await?;
                        }
                    }
                }
            }
            if look && open.iter().any(|confirm| !confirm.decided) {
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
                let screenshot_questions = self.probe.screenshot_choices().questions(&[read.0.id]);
                self.probe.screenshot_choices().cancel_page(read.0.id);
                for ticket in screenshot_questions {
                    self.cancel_screenshot_question(&ticket).await?;
                }
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

    async fn cancel_screenshot_question(
        &mut self,
        ticket: &crate::work_screenshots::consent::QuestionTicket,
    ) -> Result<(), WorkError> {
        ticket.deny();
        let Some(step) = ticket.step() else {
            return Ok(());
        };
        let result = self
            .probe
            .commit_step(WorkRuntimeUpdate::SettleStep {
                execution: self.probe.execution(),
                attempt: self.probe.attempt(),
                step,
                status: WorkStepStatus::Cancelled,
                usage: None,
                artifacts: vec![],
                evidence: None,
                file: None,
                note: Some("The screenshot question is no longer current".into()),
                measurements: None,
            })
            .await;
        if let Err(error) = result {
            // Answer/Stop can close the question concurrently. Its terminal
            // stays authoritative; never overwrite it or leave a waiting Ask.
            let closed = self
                .probe
                .runtime_projection()
                .await?
                .executions
                .iter()
                .find(|execution| execution.id == self.probe.execution())
                .and_then(|execution| execution.steps.iter().find(|fact| fact.id == step))
                .is_some_and(|fact| fact.status != WorkStepStatus::Running);
            if !closed {
                return Err(error);
            }
        }
        ticket.question_closed();
        Ok(())
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
                use zephium_core::work::parts::WorkPartNeedV1;
                fact.need = Some(match wait {
                    crate::work_agent::WorkPageWait::SignIn => WorkPartNeedV1::SignIn {
                        host: site.to_owned(),
                    },
                    crate::work_agent::WorkPageWait::Check => WorkPartNeedV1::Check {
                        host: site.to_owned(),
                    },
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
/// sixty actions and forty model calls, and the least it starts with. The
/// browser composition narrows its money to the page model's own prices.
const PAGE_TASK_MAX: WorkExecutionLimits = WorkExecutionLimits {
    model_tokens: 600_000,
    cost_micro_usd: 5_000_000,
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
            ..
        } if job_board(&url) => WorkStepKindV1::Read {
            verbatim: None,
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

/// A read that ended so it can start over on a fresh page in the same
/// session: a saved cookie refusal replaced its page, or the person signed in
/// or passed a check on it and the page then failed (a sign-in that moves on
/// to the site's app loses the page it began on) before it sent or saved
/// anything.
fn starts_over(outcome: &WorkBrowserOutcome) -> bool {
    (outcome.usage.is_some() || untouched(outcome))
        && (outcome.rerun
            || (outcome.helped
                && outcome.status == WorkStepStatus::Failed
                && outcome.committed.is_empty()
                && !outcome.held_back
                && outcome.note.as_deref() != Some(read_note::UNREACHABLE)))
}

/// A page lost before it called a model or acted: its usage is unknown only
/// because nothing was spent.
fn untouched(outcome: &WorkBrowserOutcome) -> bool {
    outcome.measurements.as_ref().is_some_and(|m| {
        m.planner_calls == 0
            && m.decision_calls == 0
            && m.native_actions == 0
            && m.model_tokens == 0
    })
}

fn retryable_read(step: &WorkStepKindV1, outcome: &Result<WorkBrowserOutcome, WorkError>) -> bool {
    let Ok(outcome) = outcome else {
        return false;
    };
    if outcome.status != WorkStepStatus::Failed || outcome.usage.is_none() {
        return false;
    }
    if matches!(step, WorkStepKindV1::Read { goal: Some(_), .. }) {
        // A page task that lost its page before it acted on anything has
        // drafted nothing, so it starts over once on a fresh page.
        return outcome.note.as_deref() == Some(read_note::UNSETTLED)
            && outcome
                .measurements
                .as_ref()
                .is_some_and(|m| m.native_actions == 0);
    }
    if !matches!(step, WorkStepKindV1::Read { goal: None, .. }) {
        return false;
    }
    let steps = outcome
        .measurements
        .as_ref()
        .and_then(|m| m.outcome.as_ref())
        == Some(&WorkPageOutcomeV1::TooManySteps);
    steps
        || outcome.note.as_deref().is_some_and(|note| {
            (note == read_note::HUMAN_CHECK && outcome.helped)
                || note == read_note::UNSETTLED
                || note == read_note::CONSTRUCTION_TIMEOUT
        })
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
    let entry = request.entry && !matches!(outcome, Ok(outcome) if starts_over(outcome));
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
    fn typed_turn_limit_can_retry_only_a_failed_read_with_known_usage() {
        for goal in [None, Some("Send the message".into())] {
            let step = WorkStepKindV1::Read {
                verbatim: None,
                url: "https://example.com/article".into(),
                goal: goal.clone(),
                collection: None,
            };
            for status in [
                WorkStepStatus::Failed,
                WorkStepStatus::Succeeded,
                WorkStepStatus::Cancelled,
                WorkStepStatus::OutcomeUnknown,
            ] {
                for usage in [None, Some(WorkUsage::default())] {
                    let outcome = WorkBrowserOutcome {
                        status,
                        usage,
                        artifacts: vec![],
                        intervention: None,
                        note: None,
                        measurements: Some(WorkStepMeasurementsV1 {
                            outcome: Some(WorkPageOutcomeV1::TooManySteps),
                            ..Default::default()
                        }),
                        helped: false,
                        held_back: false,
                        committed: Vec::new(),
                        rerun: false,
                        search: None,
                        redirect: None,
                    };
                    assert_eq!(
                        retryable_read(&step, &Ok(outcome)),
                        goal.is_none() && status == WorkStepStatus::Failed && usage.is_some()
                    );
                }
            }
            assert!(!retryable_read(&step, &Err(WorkError::OutcomeUnknown)));
        }
    }

    #[test]
    fn a_page_task_starts_over_once_only_when_it_lost_its_page_before_acting() {
        let step = WorkStepKindV1::Read {
            verbatim: None,
            url: "https://www.ebay.com/".into(),
            goal: Some("Find the listed GPUs".into()),
            collection: None,
        };
        let outcome = |note: &str, native_actions: u16| WorkBrowserOutcome {
            status: WorkStepStatus::Failed,
            usage: Some(WorkUsage::default()),
            artifacts: vec![],
            intervention: None,
            note: Some(note.into()),
            measurements: Some(WorkStepMeasurementsV1 {
                native_actions,
                ..Default::default()
            }),
            helped: false,
            held_back: false,
            committed: Vec::new(),
            rerun: false,
            search: None,
            redirect: None,
        };
        assert!(retryable_read(&step, &Ok(outcome(read_note::UNSETTLED, 0))));
        let mut helped = outcome("The page asked to sign in", 3);
        assert!(!starts_over(&helped));
        helped.helped = true;
        assert!(
            starts_over(&helped),
            "the person signed in, then the page moved on"
        );
        let mut lost = outcome("The page asked to sign in", 0);
        lost.helped = true;
        lost.usage = None;
        lost.measurements = Some(WorkStepMeasurementsV1::default());
        assert!(starts_over(&lost), "lost before spending anything");
        lost.measurements = Some(WorkStepMeasurementsV1 {
            planner_calls: 1,
            ..Default::default()
        });
        assert!(!starts_over(&lost), "unknown usage after a model call");
        helped.committed = vec!["Sent on the page: hi".into()];
        assert!(!starts_over(&helped), "never twice once it sent something");
        let mut unreachable = outcome(read_note::UNREACHABLE, 0);
        unreachable.helped = true;
        assert!(
            !starts_over(&unreachable),
            "a site that could not be reached is not read again after a sign-in"
        );
        assert!(!retryable_read(
            &step,
            &Ok(outcome(read_note::UNSETTLED, 1))
        ));
        assert!(!retryable_read(
            &step,
            &Ok(outcome(read_note::CONSTRUCTION_TIMEOUT, 0))
        ));
        assert!(!retryable_read(
            &step,
            &Ok(outcome(read_note::UNREACHABLE, 0))
        ));
        let read = WorkStepKindV1::Read {
            verbatim: None,
            url: "https://www.ebay.com/".into(),
            goal: None,
            collection: None,
        };
        assert!(
            !retryable_read(&read, &Ok(outcome(read_note::UNREACHABLE, 0))),
            "a site that could not be reached is not read again"
        );
    }

    #[test]
    fn first_load_redirect_preserves_reads_but_never_moves_tasks_to_another_site() {
        for goal in [
            None,
            Some("Compare the listed products without buying".into()),
        ] {
            let collection = Some(WorkBrowseCollection {
                title: "Products".into(),
                columns: vec![WorkBrowseColumn {
                    name: "price".into(),
                    value: WorkBrowseValue::Text,
                    required: false,
                    extraction: WorkBrowseExtraction::Generate,
                }],
                max_items: 5,
            });
            let step = WorkStepKindV1::Read {
                verbatim: None,
                url: "https://example.com/products".into(),
                goal: goal.clone(),
                collection: collection.clone(),
            };
            assert_eq!(
                redirected_kind(&step, "https://www.example.com/products/"),
                Some(WorkStepKindV1::Read {
                    verbatim: None,
                    url: "https://www.example.com/products/".into(),
                    goal: goal.clone(),
                    collection,
                })
            );
            assert_eq!(
                redirected_kind(&step, "https://other.example.net/products").is_some(),
                goal.is_none()
            );
            for destination in [
                "http://example.com/products",
                "https://127.0.0.1/products",
                "https://intranet.local/products",
                "https://example.com/products?access_token=private-value",
                "https://other.example.net/products?access_token=private-value",
                "file:///etc/passwd",
            ] {
                assert!(redirected_kind(&step, destination).is_none());
            }
        }
        assert!(redirected_kind(
            &WorkStepKindV1::Search {
                query: "products".into()
            },
            "https://example.com/products"
        )
        .is_none());
    }

    fn blocker_read(
        url: &str,
        status: WorkStepStatus,
        outcome: Option<WorkPageOutcomeV1>,
    ) -> WorkStepFact {
        WorkStepFact {
            id: WorkStepId::generate(),
            turn: 1,
            kind: WorkStepKindV1::Read {
                verbatim: None,
                url: url.into(),
                collection: None,
                goal: None,
            },
            status,
            usage: None,
            artifacts: vec![],
            evidence: None,
            note: None,
            measurements: outcome.map(|outcome| WorkStepMeasurementsV1 {
                outcome: Some(outcome),
                ..Default::default()
            }),
            local: None,
            account: None,
            part: None,
            model_fault: None,
        }
    }
    fn blocked_then(earlier: &[WorkStepFact], later: &[WorkStepFact]) -> Vec<WorkSiteBlocker> {
        fold_site_blockers([
            (WorkExecutionId::generate(), earlier, &[][..]),
            (WorkExecutionId::generate(), later, &[][..]),
        ])
    }

    #[test]
    fn site_blockers_persist_across_followups_and_ignore_internal_failures() {
        let first = [
            blocker_read(
                "https://mail.example.com/inbox",
                WorkStepStatus::Failed,
                Some(WorkPageOutcomeV1::NeedsSignIn),
            ),
            blocker_read(
                "https://airbnb.com/search",
                WorkStepStatus::Failed,
                Some(WorkPageOutcomeV1::NeedsCheck),
            ),
            blocker_read(
                "https://example.org/listings",
                WorkStepStatus::Failed,
                Some(WorkPageOutcomeV1::SiteError),
            ),
            blocker_read(
                "https://internal.test/",
                WorkStepStatus::Failed,
                Some(WorkPageOutcomeV1::Refused {
                    stage: WorkPageFailureStageV1::Provider,
                }),
            ),
        ];
        let blockers = blocked_then(&first, &[]);
        assert_eq!(blockers.len(), 3);
        assert!(blockers.iter().any(|site| site.need()
            == WorkPartNeedV1::Check {
                host: "airbnb.com".into()
            }));
        assert!(blockers.iter().any(|site| site.need()
            == WorkPartNeedV1::SignIn {
                host: "example.com".into()
            }));
        assert!(blockers.iter().all(|site| site.site != "internal.test"));
    }

    #[test]
    fn help_pages_empty_results_and_model_success_do_not_clear_an_account_wall() {
        let mut first = [blocker_read(
            "https://example.com/inbox",
            WorkStepStatus::Failed,
            Some(WorkPageOutcomeV1::NeedsSignIn),
        )];
        first[0].account = Some(Box::new(WorkPageAccountV1 {
            host: "example.com".into(),
            badge: true,
        }));
        let mut help = blocker_read(
            "https://example.com/help/login",
            WorkStepStatus::Succeeded,
            Some(WorkPageOutcomeV1::Completed),
        );
        help.artifacts.push(WorkArtifactId::generate());
        let empty = blocker_read(
            "https://example.com/inbox",
            WorkStepStatus::Succeeded,
            Some(WorkPageOutcomeV1::Completed),
        );
        let untyped = blocker_read("https://example.com/inbox", WorkStepStatus::Succeeded, None);
        assert_eq!(blocked_then(&first, &[help, empty, untyped]).len(), 1);
        let mut public = blocker_read(
            "https://example.com/inbox",
            WorkStepStatus::Succeeded,
            Some(WorkPageOutcomeV1::Completed),
        );
        public.artifacts.push(WorkArtifactId::generate());
        assert_eq!(blocked_then(&first, &[public.clone()]).len(), 1);
        public.account = Some(Box::new(WorkPageAccountV1 {
            host: "example.com".into(),
            badge: true,
        }));
        if let WorkStepKindV1::Read { goal, .. } = &mut public.kind {
            *goal = Some("Read my inbox".into());
        }
        assert!(blocked_then(&first, &[public]).is_empty());
    }

    #[test]
    fn verified_native_resolution_clears_only_the_matching_site_and_handoff() {
        let first = [blocker_read(
            "https://example.com/inbox",
            WorkStepStatus::Failed,
            Some(WorkPageOutcomeV1::NeedsSignIn),
        )];
        let mut resumed = blocker_read(
            "https://example.com/inbox",
            WorkStepStatus::Failed,
            Some(WorkPageOutcomeV1::Refused {
                stage: WorkPageFailureStageV1::Extraction,
            }),
        );
        resumed.measurements.as_mut().unwrap().resolved_handoff =
            Some(WorkPageHandoffKindV1::Check);
        assert_eq!(blocked_then(&first, &[resumed.clone()]).len(), 1);
        resumed.measurements.as_mut().unwrap().resolved_handoff =
            Some(WorkPageHandoffKindV1::SignIn);
        assert!(blocked_then(&first, &[resumed.clone()]).is_empty());
        if let WorkStepKindV1::Read { url, .. } = &mut resumed.kind {
            *url = "https://different.test/".into();
        }
        assert_eq!(blocked_then(&first, &[resumed]).len(), 1);
    }

    #[test]
    fn reachability_needs_real_same_target_success_and_does_not_replace_a_check() {
        let first = [blocker_read(
            "https://example.com/search",
            WorkStepStatus::Failed,
            Some(WorkPageOutcomeV1::SiteError),
        )];
        let mut completed = blocker_read(
            "https://example.com/search#results",
            WorkStepStatus::Succeeded,
            Some(WorkPageOutcomeV1::Completed),
        );
        completed.artifacts.push(WorkArtifactId::generate());
        assert!(blocked_then(&first, &[completed]).is_empty());
        let first = [blocker_read(
            "https://example.com/search",
            WorkStepStatus::Failed,
            Some(WorkPageOutcomeV1::NeedsCheck),
        )];
        let unavailable = blocker_read(
            "https://example.com/search",
            WorkStepStatus::Failed,
            Some(WorkPageOutcomeV1::SiteError),
        );
        assert_eq!(
            blocked_then(&first, &[unavailable])[0].kind,
            SiteBlockKind::Check
        );
    }

    #[test]
    fn legacy_notes_migrate_but_never_override_a_typed_outcome() {
        let mut old = blocker_read("https://example.com/", WorkStepStatus::Failed, None);
        old.note = Some(read_note::HUMAN_CHECK.into());
        assert_eq!(
            blocked_then(&[old.clone()], &[])[0].kind,
            SiteBlockKind::Check
        );
        old.measurements = Some(WorkStepMeasurementsV1 {
            outcome: Some(WorkPageOutcomeV1::Refused {
                stage: WorkPageFailureStageV1::Runtime,
            }),
            ..Default::default()
        });
        assert!(blocked_then(&[old], &[]).is_empty());
    }

    #[test]
    fn opening_decision_is_one_retry_not_a_resolution_or_repeated_probe() {
        let previous = WorkExecutionId::generate();
        let current = WorkExecutionId::generate();
        let failed = blocker_read(
            "https://example.com/",
            WorkStepStatus::Failed,
            Some(WorkPageOutcomeV1::NeedsCheck),
        );
        let blocker = fold_site_blockers([(previous, &[failed][..], &[][..])]).remove(0);
        assert_eq!(site_retry_steps(current, &blocker, &[]), None);
        let mut answer = blocker_read("https://example.com/", WorkStepStatus::Succeeded, None);
        answer.kind = WorkStepKindV1::Ask {
            purpose: Some(WorkAskPurposeV1::Confirm),
            prompt: retry_prompt("example.com"),
            options: vec![OPEN_BLOCKED_SITE.into(), DEFER_BLOCKED_SITE.into()],
            answer: Some(OPEN_BLOCKED_SITE.into()),
        };
        assert_eq!(
            site_retry_steps(current, &blocker, &[answer.clone()]),
            Some(true)
        );
        assert_eq!(
            site_retry_steps(previous, &blocker, &[answer.clone()]),
            Some(false)
        );
        let attempted = blocker_read(
            "https://example.com/",
            WorkStepStatus::Failed,
            Some(WorkPageOutcomeV1::Refused {
                stage: WorkPageFailureStageV1::Provider,
            }),
        );
        assert_eq!(
            site_retry_steps(current, &blocker, &[answer.clone(), attempted]),
            Some(false)
        );
        if let WorkStepKindV1::Ask { answer, .. } = &mut answer.kind {
            *answer = Some(DEFER_BLOCKED_SITE.into());
        }
        assert_eq!(site_retry_steps(current, &blocker, &[answer]), Some(false));
    }

    #[test]
    fn stopped_native_wait_persists_across_followup_and_later_resolution_wins() {
        use zephium_core::work::parts::WorkHelperV1;
        for kind in [WorkPageHandoffKindV1::SignIn, WorkPageHandoffKindV1::Check] {
            let mut part = WorkPartFactV1 {
                id: zephium_core::work::WorkPartId::generate(),
                title: "Inbox".into(),
                helper: WorkHelperV1::Browser,
                service: None,
                goal: "Read my inbox".into(),
                state: WorkPartStateV1::Stopped,
                started_ms: Some("1".into()),
                ended_ms: Some("2".into()),
                summary: None,
                need: Some(match kind {
                    WorkPageHandoffKindV1::SignIn => WorkPartNeedV1::SignIn {
                        host: "example.com".into(),
                    },
                    WorkPageHandoffKindV1::Check | WorkPageHandoffKindV1::SignInAndCheck => {
                        WorkPartNeedV1::Check {
                            host: "example.com".into(),
                        }
                    }
                }),
            };
            let mut stopped = blocker_read(
                "https://example.com/inbox",
                WorkStepStatus::OutcomeUnknown,
                Some(WorkPageOutcomeV1::OutcomeUnknown),
            );
            stopped.part = Some(part.id);
            stopped.measurements.as_mut().unwrap().settled_revision = WorkRevision::new(10);
            let first = WorkExecutionId::generate();
            let second = WorkExecutionId::generate();
            for state in [WorkPartStateV1::Waiting, WorkPartStateV1::Stopped] {
                part.state = state;
                let blocked = fold_site_blockers([
                    (first, &[stopped.clone()][..], &[part.clone()][..]),
                    (second, &[][..], &[][..]),
                ]);
                assert_eq!(blocked.len(), 1);
                assert_eq!(blocked[0].need(), part.need.clone().unwrap());
            }
            // An arbitrary helper's failed need is not a native waiting receipt.
            part.state = WorkPartStateV1::Failed;
            assert!(
                fold_site_blockers([(first, &[stopped.clone()][..], &[part.clone()][..])])
                    .is_empty()
            );
            part.state = WorkPartStateV1::Stopped;
            let mut resolved = blocker_read(
                "https://example.com/inbox",
                WorkStepStatus::Failed,
                Some(WorkPageOutcomeV1::Refused {
                    stage: WorkPageFailureStageV1::Extraction,
                }),
            );
            resolved.part = Some(part.id);
            resolved.measurements.as_mut().unwrap().settled_revision = WorkRevision::new(20);
            resolved.measurements.as_mut().unwrap().resolved_handoff = Some(kind);
            // A stale stopped-part need cannot overwrite a later verified resume,
            // whether that receipt is in this execution or the next follow-up.
            assert!(fold_site_blockers([(
                first,
                &[stopped.clone(), resolved.clone()][..],
                &[part.clone()][..]
            )])
            .is_empty());
            assert!(fold_site_blockers([
                (first, &[stopped.clone()][..], &[part.clone()][..]),
                (second, &[resolved.clone()][..], &[][..])
            ])
            .is_empty());
            stopped.measurements.as_mut().unwrap().resolved_handoff = Some(kind);
            assert!(
                fold_site_blockers([(first, &[stopped.clone()][..], &[part.clone()][..])])
                    .is_empty()
            );
            stopped.measurements.as_mut().unwrap().resolved_handoff = None;
            part.need = Some(WorkPartNeedV1::Check {
                host: "different.test".into(),
            });
            assert!(fold_site_blockers([(first, &[stopped][..], &[part][..])]).is_empty());
        }
    }

    #[test]
    fn manual_page_dismissal_keeps_a_typed_wall_but_not_an_untyped_cancel() {
        for outcome in [
            WorkPageOutcomeV1::NeedsSignIn,
            WorkPageOutcomeV1::NeedsCheck,
        ] {
            let dismissed = blocker_read(
                "https://example.com/",
                WorkStepStatus::Cancelled,
                Some(outcome),
            );
            assert_eq!(blocked_then(&[dismissed], &[]).len(), 1);
        }
        let dismissed = blocker_read(
            "https://example.com/",
            WorkStepStatus::Cancelled,
            Some(WorkPageOutcomeV1::Cancelled),
        );
        assert!(blocked_then(&[dismissed], &[]).is_empty());
    }

    #[test]
    fn handoff_receipts_require_original_target_and_account_provenance() {
        let mut wall = blocker_read(
            "https://mail.example.com/inbox",
            WorkStepStatus::Failed,
            Some(WorkPageOutcomeV1::NeedsSignIn),
        );
        wall.measurements.as_mut().unwrap().settled_revision = WorkRevision::new(10);
        wall.account = Some(Box::new(WorkPageAccountV1 {
            host: "mail.example.com".into(),
            badge: true,
        }));
        let mut resumed = blocker_read(
            "https://docs.example.com/login",
            WorkStepStatus::Failed,
            Some(WorkPageOutcomeV1::Refused {
                stage: WorkPageFailureStageV1::Extraction,
            }),
        );
        resumed.measurements.as_mut().unwrap().settled_revision = WorkRevision::new(20);
        resumed.account = wall.account.clone();
        resumed.measurements.as_mut().unwrap().resolved_handoff =
            Some(WorkPageHandoffKindV1::SignInAndCheck);
        assert_eq!(blocked_then(&[wall.clone()], &[resumed.clone()]).len(), 1);
        assert!(!read_handoff_resolves(
            &wall,
            &resumed,
            WorkPageHandoffKindV1::SignIn
        ));
        if let WorkStepKindV1::Read { url, .. } = &mut resumed.kind {
            *url = "https://mail.example.com/inbox".into();
        }
        resumed.account = None;
        assert_eq!(blocked_then(&[wall.clone()], &[resumed.clone()]).len(), 1);
        assert!(!read_handoff_resolves(
            &wall,
            &resumed,
            WorkPageHandoffKindV1::SignIn
        ));
        resumed.account = wall.account.clone();
        assert!(blocked_then(&[wall.clone()], &[resumed.clone()]).is_empty());
        assert!(read_handoff_resolves(
            &wall,
            &resumed,
            WorkPageHandoffKindV1::SignIn
        ));
        // An anonymous page can genuinely sign in during its own native handoff.
        wall.account = None;
        resumed.account = None;
        assert!(blocked_then(&[wall.clone()], &[resumed.clone()]).is_empty());
        wall.measurements.as_mut().unwrap().outcome = Some(WorkPageOutcomeV1::NeedsCheck);
        assert!(blocked_then(&[wall], &[resumed]).is_empty());
    }

    #[test]
    fn check_resolution_does_not_erase_an_unresolved_sign_in_on_the_same_target() {
        let sign_in = blocker_read(
            "https://mail.example.com/inbox",
            WorkStepStatus::Failed,
            Some(WorkPageOutcomeV1::NeedsSignIn),
        );
        let check = blocker_read(
            "https://mail.example.com/inbox",
            WorkStepStatus::Failed,
            Some(WorkPageOutcomeV1::NeedsCheck),
        );
        let mut resumed = blocker_read(
            "https://mail.example.com/inbox",
            WorkStepStatus::Failed,
            Some(WorkPageOutcomeV1::Refused {
                stage: WorkPageFailureStageV1::Extraction,
            }),
        );
        resumed.measurements.as_mut().unwrap().resolved_handoff =
            Some(WorkPageHandoffKindV1::Check);
        let blocked = blocked_then(&[sign_in.clone(), check.clone()], &[resumed.clone()]);
        assert_eq!(blocked.len(), 1);
        assert_eq!(blocked[0].kind, SiteBlockKind::SignIn);
        resumed.measurements.as_mut().unwrap().resolved_handoff =
            Some(WorkPageHandoffKindV1::SignInAndCheck);
        assert!(blocked_then(&[sign_in, check], &[resumed]).is_empty());
    }

    #[test]
    fn different_origin_walls_on_one_site_remain_independent() {
        let inbox = blocker_read(
            "https://mail.example.com/inbox",
            WorkStepStatus::Failed,
            Some(WorkPageOutcomeV1::NeedsSignIn),
        );
        let docs = blocker_read(
            "https://docs.example.com/login",
            WorkStepStatus::Failed,
            Some(WorkPageOutcomeV1::NeedsSignIn),
        );
        let mut resumed = blocker_read(
            "https://docs.example.com/login",
            WorkStepStatus::Succeeded,
            Some(WorkPageOutcomeV1::Completed),
        );
        resumed.measurements.as_mut().unwrap().resolved_handoff =
            Some(WorkPageHandoffKindV1::SignIn);
        let blocked = blocked_then(&[inbox, docs], &[resumed]);
        assert_eq!(blocked.len(), 1);
        assert_eq!(blocked[0].url, "https://mail.example.com/inbox");
    }

    #[test]
    fn coarse_part_need_never_relocates_native_wall_or_guesses_between_pages() {
        let part = WorkPartFactV1 {
            id: zephium_core::work::WorkPartId::generate(),
            title: "Inbox".into(),
            helper: zephium_core::work::parts::WorkHelperV1::Browser,
            service: None,
            goal: "Read my inbox".into(),
            state: WorkPartStateV1::Stopped,
            started_ms: Some("1".into()),
            ended_ms: Some("2".into()),
            summary: None,
            need: Some(WorkPartNeedV1::SignIn {
                host: "example.com".into(),
            }),
        };
        let mut inbox = blocker_read(
            "https://mail.example.com/inbox",
            WorkStepStatus::Failed,
            Some(WorkPageOutcomeV1::NeedsSignIn),
        );
        inbox.part = Some(part.id);
        inbox.measurements.as_mut().unwrap().settled_revision = WorkRevision::new(10);
        let mut docs = blocker_read(
            "https://docs.example.com/help",
            WorkStepStatus::Cancelled,
            Some(WorkPageOutcomeV1::Cancelled),
        );
        docs.part = Some(part.id);
        let mut success = blocker_read(
            "https://docs.example.com/help",
            WorkStepStatus::Succeeded,
            Some(WorkPageOutcomeV1::Completed),
        );
        success.artifacts.push(WorkArtifactId::generate());
        success.measurements.as_mut().unwrap().settled_revision = WorkRevision::new(20);
        let blocked = fold_site_blockers([(
            WorkExecutionId::generate(),
            &[inbox.clone(), docs.clone(), success][..],
            &[part.clone()][..],
        )]);
        assert_eq!(blocked.len(), 1);
        assert_eq!(blocked[0].url, "https://mail.example.com/inbox");
        inbox.status = WorkStepStatus::OutcomeUnknown;
        inbox.measurements.as_mut().unwrap().settled_revision = None;
        inbox.measurements.as_mut().unwrap().outcome = Some(WorkPageOutcomeV1::OutcomeUnknown);
        assert!(
            fold_site_blockers([(WorkExecutionId::generate(), &[inbox, docs][..], &[part][..])])
                .is_empty(),
            "ambiguous coarse needs must not invent a page target"
        );
    }

    #[test]
    fn anonymous_sign_in_then_check_keeps_source_and_followup_objects_private() {
        let mut read = blocker_read(
            "https://mail.example.com/inbox",
            WorkStepStatus::Succeeded,
            Some(WorkPageOutcomeV1::Completed),
        );
        let artifact = |id: u128, text: &str| WorkArtifactV1 {
            revises: None,
            part: None,
            version: 1,
            id: id.into(),
            execution: 1.into(),
            node: 1.into(),
            attempt: 1.into(),
            output: "results".into(),
            title: "Notes".into(),
            data: WorkArtifactDataV1::Document {
                paragraphs: vec![text.into()],
                formatted: None,
            },
            evidence: vec![],
            review: WorkOutputReview::SourceMappedNeedsReview,
            presentation: WorkArtifactPresentationV1::Automatic,
            general_knowledge: false,
        };
        let source = [artifact(
            1,
            "Confidential customer invoice total is 47219 euros",
        )];
        let summary = [artifact(
            2,
            "My private account renewal budget is 99317 euros",
        )];
        read.measurements.as_mut().unwrap().resolved_handoff = Some(WorkPageHandoffKindV1::Check);
        assert!(!private_page_step(&read));
        read.measurements.as_mut().unwrap().resolved_handoff =
            Some(WorkPageHandoffKindV1::SignInAndCheck);
        assert!(private_page_step(&read));
        assert!(measurements_resolved_sign_in(read.measurements));
        let private =
            private_artifact_texts([(&[read.clone()][..], &source[..]), (&[][..], &summary[..])]);
        assert_eq!(private.len(), 2);
        assert!(query_discloses(
            "Confidential customer invoice total is 47219 euros",
            &private
        ));
        assert!(query_discloses(
            "My private account renewal budget is 99317 euros",
            &private
        ));
        assert!(!query_discloses("weather in Warsaw", &private));
        // Screenshot consent is independent of sign-in/account-entry authority.
        read.measurements.as_mut().unwrap().resolved_handoff = None;
        read.measurements.as_mut().unwrap().image_disclosure =
            Some(WorkPageImageDisclosureV1::PublicIsolated);
        assert!(!private_page_step(&read));
        read.measurements.as_mut().unwrap().image_disclosure =
            Some(WorkPageImageDisclosureV1::SiteConsent);
        assert!(private_page_step(&read));
        assert!(measurements_private_page(read.measurements));
        assert!(!measurements_resolved_sign_in(read.measurements));
        let private =
            private_artifact_texts([(&[read.clone()][..], &source[..]), (&[][..], &summary[..])]);
        assert_eq!(private.len(), 2);
        assert!(query_discloses(
            "My private account renewal budget is 99317 euros",
            &private
        ));
        assert_eq!(
            WorkPageImageDisclosureV1::SiteConsent.merge(WorkPageImageDisclosureV1::PublicIsolated),
            WorkPageImageDisclosureV1::SiteConsent
        );
        assert!(
            read.account.is_none(),
            "privacy classification must not invent account permission"
        );
    }

    #[test]
    fn blocker_history_uses_settlement_revision_in_both_reverse_orders() {
        let execution = WorkExecutionId::generate();
        let mut failed = blocker_read(
            "https://example.com/inbox",
            WorkStepStatus::Failed,
            Some(WorkPageOutcomeV1::NeedsCheck),
        );
        let mut success = blocker_read(
            "https://example.com/inbox",
            WorkStepStatus::Succeeded,
            Some(WorkPageOutcomeV1::Completed),
        );
        success.artifacts.push(WorkArtifactId::generate());
        // Begin(failed), Begin(success), Settle(success), Settle(failed).
        failed.measurements.as_mut().unwrap().settled_revision = WorkRevision::new(30);
        success.measurements.as_mut().unwrap().settled_revision = WorkRevision::new(20);
        assert_eq!(
            fold_site_blockers([(execution, &[failed.clone(), success.clone()][..], &[][..])])
                .len(),
            1
        );
        // Begin(resolved), Begin(failed), Settle(failed), Settle(resolved).
        failed.measurements.as_mut().unwrap().settled_revision = WorkRevision::new(20);
        success.measurements.as_mut().unwrap().settled_revision = WorkRevision::new(30);
        success.measurements.as_mut().unwrap().resolved_handoff =
            Some(WorkPageHandoffKindV1::Check);
        assert!(
            fold_site_blockers([(execution, &[success.clone(), failed.clone()][..], &[][..])])
                .is_empty()
        );
        if let WorkStepKindV1::Read { url, .. } = &mut success.kind {
            *url = "https://docs.example.com/inbox".into();
        }
        assert_eq!(
            fold_site_blockers([(execution, &[success, failed][..], &[][..])]).len(),
            1
        );
    }

    #[test]
    fn legacy_or_unordered_observations_cannot_clear_a_current_execution_wall() {
        let execution = WorkExecutionId::generate();
        let mut failed = blocker_read(
            "https://example.com/inbox",
            WorkStepStatus::Failed,
            Some(WorkPageOutcomeV1::NeedsCheck),
        );
        let mut resolved = blocker_read(
            "https://example.com/inbox",
            WorkStepStatus::Succeeded,
            Some(WorkPageOutcomeV1::Completed),
        );
        resolved.artifacts.push(WorkArtifactId::generate());
        resolved.measurements.as_mut().unwrap().resolved_handoff =
            Some(WorkPageHandoffKindV1::Check);
        resolved.measurements.as_mut().unwrap().settled_revision = WorkRevision::new(30);
        assert_eq!(
            fold_site_blockers([(execution, &[failed.clone(), resolved.clone()][..], &[][..])])
                .len(),
            1
        );
        failed.measurements.as_mut().unwrap().settled_revision = WorkRevision::new(20);
        resolved.measurements.as_mut().unwrap().settled_revision = None;
        assert_eq!(
            fold_site_blockers([(execution, &[failed.clone(), resolved.clone()][..], &[][..])])
                .len(),
            1
        );
        // A new execution is itself a trustworthy boundary after the old one ended.
        assert!(blocked_then(&[failed], &[resolved]).is_empty());
    }

    #[test]
    fn concurrent_site_retry_claims_are_single_and_release_on_drop() {
        let profile = ProfileId::generate();
        let work = WorkId::generate();
        let execution = WorkExecutionId::generate();
        let first = SiteRetryClaim::acquire(profile, work, execution, "example.com").unwrap();
        assert!(SiteRetryClaim::acquire(profile, work, execution, "example.com").is_none());
        assert!(
            SiteRetryClaim::acquire(profile, WorkId::generate(), execution, "example.com")
                .is_some()
        );
        drop(first);
        assert!(SiteRetryClaim::acquire(profile, work, execution, "example.com").is_some());
    }

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
                verbatim: None,
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
                verbatim: None,
                url: url.into(),
                collection: None,
                goal: None,
            };
            assert_eq!(job_listing(kind.clone()), kind);
        }
        // A collection the model chose is its own.
        let chosen = WorkStepKindV1::Read {
            verbatim: None,
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
                committed: Vec::new(),
                rerun: false,
                search: None,
                redirect: None,
            };
            assert!(matches!(
                checked_outcome(Ok(outcome), limits),
                Err(WorkError::OutcomeUnknown)
            ));
        }
        assert!(combined_usage(Some(WorkUsage::default()), None).is_none());
    }
}
