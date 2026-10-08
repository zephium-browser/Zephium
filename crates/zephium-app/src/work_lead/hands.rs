//! A part's or the lead's hands on searches, pages and files: the proven
//! page machinery (entry questions, sign-in waits, Confirm steps, frames)
//! runs each request as a durable step carrying the part, and what came back
//! returns to the model as a compact, cited digest, never a page.
use std::collections::BTreeSet;
use std::future::Future;
use std::sync::Mutex;

use serde_json::Value;
use zephium_core::work::{artifact::*, collection::WorkBrowseCollection, runtime::*, search::*, *};

use super::call::clip;
use super::run::LeadRun;
use crate::work_agent::{WorkAgentBrowseRequest, WorkBrowserOutcome, WorkPartDriver};
use crate::work_runtime::{WorkAttemptProbe, WorkNodeAttempt};

/// At most this many agent pages are live at once across a run's parts: the
/// page group's native worker seats.
pub(crate) const LIVE_PAGES: usize = 3;
/// Searches in flight at once across a run's parts. Each holds one of the
/// run's provider slots (four) while it searches or ranks; past that a
/// search waits instead of failing to be sent.
const LIVE_SEARCHES: usize = 3;
const SEARCH_ANSWER_CHARS: usize = 2_400;
const SEARCH_SOURCES: usize = 8;
const RECORD_LINES: usize = 32;
const FILE_TEXT_BYTES: usize = 6 * 1024;

/// The browser closure the host gives the run, shared by every part, with
/// its gates: at most three pages live, and at most one page task per site,
/// since one session means one actor per site.
pub(crate) struct SharedBrowser<B> {
    browser: Mutex<B>,
    pages: tokio::sync::Semaphore,
    searches: tokio::sync::Semaphore,
    sites: Mutex<Vec<(String, std::sync::Arc<tokio::sync::Mutex<()>>)>>,
    entries: EntryDesk,
}
impl<B> SharedBrowser<B> {
    pub(crate) fn new(browser: B) -> Self {
        Self {
            browser: Mutex::new(browser),
            pages: tokio::sync::Semaphore::new(LIVE_PAGES),
            searches: tokio::sync::Semaphore::new(LIVE_SEARCHES),
            sites: Mutex::new(Vec::new()),
            entries: EntryDesk::default(),
        }
    }
    fn site_lane(&self, site: &str) -> std::sync::Arc<tokio::sync::Mutex<()>> {
        let mut sites = self
            .sites
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some((_, lane)) = sites.iter().find(|(known, _)| known == site) {
            return lane.clone();
        }
        let lane = std::sync::Arc::new(tokio::sync::Mutex::new(()));
        sites.push((site.to_owned(), lane.clone()));
        lane
    }
}

/// How long the first part to reach the entry question waits for the
/// services of the parts started with it, so one question names them all.
const ENTRY_GATHER: std::time::Duration = std::time::Duration::from_millis(600);

/// The run's entry question: the parts that start together ask it once,
/// naming every service, and each applies the answer to its own sites.
#[derive(Default)]
pub(crate) struct EntryDesk {
    state: Mutex<Desk>,
    changed: tokio::sync::Notify,
}
#[derive(Default)]
struct Desk {
    /// Services waiting for a question: (site, name).
    waiting: Vec<(String, String)>,
    asking: bool,
    /// Siblings whose native presence/policy admission is still outstanding.
    preparing: usize,
    answers: Vec<(String, crate::work_sites::EntryAnswer)>,
    stopped: bool,
}
enum EntryTurn {
    /// Ask one question for these services, then publish the answer.
    Ask(Vec<(String, String)>),
    Decided(Vec<(String, crate::work_sites::EntryAnswer)>),
    Stopped,
}
/// Owns one presence admission until its candidates are published or canceled.
struct EntryPreparation<'a> {
    desk: &'a EntryDesk,
    active: bool,
}
impl EntryPreparation<'_> {
    fn finish(mut self, mine: &[(String, String)]) {
        if self.active {
            self.desk.finish_preparation(mine);
            self.active = false;
        }
    }
}
impl Drop for EntryPreparation<'_> {
    fn drop(&mut self) {
        if self.active {
            self.desk.finish_preparation(&[]);
        }
    }
}
impl EntryDesk {
    fn prepare(&self) -> EntryPreparation<'_> {
        let mut desk = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let active = match desk.preparing.checked_add(1) {
            Some(count) if !desk.stopped => {
                desk.preparing = count;
                true
            }
            _ => {
                desk.stopped = true;
                false
            }
        };
        EntryPreparation { desk: self, active }
    }
    fn finish_preparation(&self, mine: &[(String, String)]) {
        let mut desk = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match desk.preparing.checked_sub(1) {
            Some(count) => desk.preparing = count,
            None => desk.stopped = true,
        }
        for (site, name) in mine {
            if !desk.answers.iter().any(|(known, _)| known == site)
                && !desk.waiting.iter().any(|(known, _)| known == site)
            {
                desk.waiting.push((site.clone(), name.clone()));
            }
        }
        drop(desk);
        self.changed.notify_waiters();
    }
    async fn next(&self, mine: &[(String, String)]) -> EntryTurn {
        loop {
            let changed = self.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            if let Some(turn) = self.look(mine) {
                return turn;
            }
            if self.lead() {
                tokio::time::sleep(ENTRY_GATHER).await;
                loop {
                    // Register the wake before checking under the lock. A
                    // finishing native query cannot be lost between the check
                    // and this wait, or split one admitted sibling wave.
                    let changed = self.changed.notified();
                    tokio::pin!(changed);
                    changed.as_mut().enable();
                    let wave = {
                        let mut desk = self
                            .state
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner);
                        if desk.stopped {
                            return EntryTurn::Stopped;
                        }
                        (desk.preparing == 0).then(|| std::mem::take(&mut desk.waiting))
                    };
                    if let Some(wave) = wave {
                        return EntryTurn::Ask(wave);
                    }
                    changed.await;
                }
            }
            changed.await;
        }
    }
    /// The answer for all of `mine`, or the stop; otherwise `mine` waits.
    fn look(&self, mine: &[(String, String)]) -> Option<EntryTurn> {
        let mut desk = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if desk.stopped {
            return Some(EntryTurn::Stopped);
        }
        let decided: Vec<_> = desk
            .answers
            .iter()
            .filter(|(site, _)| mine.iter().any(|(own, _)| own == site))
            .cloned()
            .collect();
        if decided.len() == mine.len() {
            return Some(EntryTurn::Decided(decided));
        }
        for (site, name) in mine {
            if !desk.answers.iter().any(|(known, _)| known == site)
                && !desk.waiting.iter().any(|(known, _)| known == site)
            {
                desk.waiting.push((site.clone(), name.clone()));
            }
        }
        None
    }
    /// Takes the question when nobody is asking one.
    fn lead(&self) -> bool {
        let mut desk = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        !std::mem::replace(&mut desk.asking, true)
    }
    fn publish(&self, wave: &[(String, String)], answer: Option<crate::work_sites::EntryAnswer>) {
        let mut desk = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        desk.asking = false;
        match answer {
            Some(answer) => desk
                .answers
                .extend(wave.iter().map(|(site, _)| (site.clone(), answer))),
            None => desk.stopped = true,
        }
        drop(desk);
        self.changed.notify_waiters();
    }
}

/// One request the model made, as the step it becomes.
#[derive(Clone)]
pub(crate) struct Request {
    pub call: String,
    pub kind: WorkStepKindV1,
    /// A page task about the person's own account on its site.
    pub mine: bool,
    /// A page task that only reads what a daily app's view lists.
    pub view: bool,
}

pub(crate) struct Hands<'a, B> {
    run: &'a LeadRun,
    attempt: &'a WorkNodeAttempt,
    search: &'a dyn WorkPublicSearchProvider,
    browser: &'a SharedBrowser<B>,
    driver: tokio::sync::Mutex<WorkPartDriver>,
    part: Option<WorkPartId>,
    charged: Mutex<WorkUsage>,
    /// The part is about a service the person uses as themselves.
    personal: bool,
}

impl<'a, B, Fut> Hands<'a, B>
where
    B: FnMut(WorkAttemptProbe, WorkAgentBrowseRequest) -> Fut,
    Fut: Future<Output = Result<WorkBrowserOutcome, WorkError>>,
{
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn new(
        run: &'a LeadRun,
        attempt: &'a WorkNodeAttempt,
        search: &'a dyn WorkPublicSearchProvider,
        browser: &'a SharedBrowser<B>,
        part: Option<WorkPartId>,
        limits: WorkExecutionLimits,
        objective: String,
    ) -> Self {
        let driver = WorkPartDriver::new(
            run.handle.clone(),
            run.profile,
            run.probe.clone(),
            run.grant.clone(),
            limits,
            attempt.node().outputs[0].name.clone(),
            objective,
            part,
        )
        .await;
        Self {
            run,
            attempt,
            search,
            browser,
            driver: tokio::sync::Mutex::new(driver),
            part,
            charged: Mutex::new(WorkUsage::default()),
            personal: false,
        }
    }

    /// Every page task of this part works on the person's own account.
    pub(crate) fn personal(mut self, personal: bool) -> Self {
        self.personal = personal;
        self
    }

    /// What this driver has spent in all.
    pub(crate) async fn used(&self) -> WorkUsage {
        self.driver.lock().await.used()
    }

    /// Runs the requests as steps (searches in parallel, pages two at a
    /// time) and returns one digest per request, in order. `Err` carries a
    /// terminal status: the run must end.
    pub(crate) async fn run(
        &self,
        requests: Vec<Request>,
    ) -> Result<Vec<(String, String, bool)>, WorkAttemptStatus> {
        let mut refused = Vec::new();
        let mut admitted = Vec::with_capacity(requests.len());
        for request in requests {
            if let WorkStepKindV1::Read { url, .. } = &request.kind {
                if let Err(why) = self.run.admit_address(url, self.part).await {
                    refused.push((request.call, why, true));
                    continue;
                }
            }
            admitted.push(request);
        }
        let mut results = self.run_admitted(admitted).await?;
        results.extend(refused);
        Ok(results)
    }

    async fn run_admitted(
        &self,
        requests: Vec<Request>,
    ) -> Result<Vec<(String, String, bool)>, WorkAttemptStatus> {
        if requests.is_empty() {
            return Ok(Vec::new());
        }
        let before: BTreeSet<WorkStepId> = match self.run.execution().await {
            Ok(execution) => execution.steps.iter().map(|s| s.id).collect(),
            Err(_) => BTreeSet::new(),
        };
        let started = std::time::Instant::now();
        let mut driver = self.driver.lock().await;
        driver.view_pages(
            requests
                .iter()
                .filter(|request| request.view)
                .filter_map(|request| match &request.kind {
                    WorkStepKindV1::Read {
                        url, goal: Some(_), ..
                    } => Some(url.clone()),
                    _ => None,
                })
                .collect(),
        );
        if let Some(status) = self.enter_sites(&mut driver, &requests).await {
            return Err(status);
        }
        // Typing could carry what the run read to a site the person did not
        // name, so on those sites each field waits for them.
        let private = self.run.is_private();
        driver.hold_typing(
            requests
                .iter()
                .filter_map(|request| match &request.kind {
                    WorkStepKindV1::Read {
                        url, goal: Some(_), ..
                    } if private => crate::work_sites::site_of(url),
                    _ => None,
                })
                .filter(|site| !self.run.trusts(site))
                .collect(),
        );
        let mut gate = |probe: WorkAttemptProbe, request: WorkAgentBrowseRequest| {
            let lane = match &request.step {
                WorkStepKindV1::Read {
                    url, goal: Some(_), ..
                } => crate::work_sites::site_of(url).map(|site| self.browser.site_lane(&site)),
                _ => None,
            };
            let future = {
                let mut browser = self
                    .browser
                    .browser
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                (browser)(probe, request)
            };
            let pages = &self.browser.pages;
            async move {
                let _site = match &lane {
                    Some(lane) => Some(lane.lock().await),
                    None => None,
                };
                let _page = pages.acquire().await.ok();
                future.await
            }
        };
        let kinds: Vec<WorkStepKindV1> = requests.iter().map(|r| r.kind.clone()).collect();
        let search = SearchLane {
            inner: self.search,
            permits: &self.browser.searches,
        };
        let outcome = driver
            .run(
                self.attempt,
                &search,
                &mut gate,
                self.run.turn(),
                before.len(),
                kinds,
            )
            .await;
        for site in driver.take_typing_allowed() {
            self.run.trust_site(&site);
        }
        let used = driver.used();
        let notices = driver.take_notices();
        {
            let mut charged = self
                .charged
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            self.run.charge(WorkUsage {
                model_tokens: used.model_tokens.saturating_sub(charged.model_tokens),
                cost_micro_usd: used.cost_micro_usd.saturating_sub(charged.cost_micro_usd),
                operations: used.operations.saturating_sub(charged.operations),
                accounting: used.accounting,
            });
            *charged = used;
        }
        // A page whose outcome is unknown is one lost page, not the end of
        // the run: its step says so and the run's usage becomes a ceiling.
        if let Err(error) = &outcome {
            crate::work_trace::record(format_args!(
                "work: phase=lead event=hands_failed error={error:?}"
            ));
        }
        let terminal = match outcome {
            Ok(Some(WorkAttemptStatus::OutcomeUnknown)) | Err(WorkError::OutcomeUnknown) => {
                self.run.charge(WorkUsage {
                    model_tokens: 0,
                    cost_micro_usd: 0,
                    operations: 0,
                    accounting: WorkUsageAccounting::ConservativeReservation,
                });
                None
            }
            Ok(Some(status)) => Some(status),
            Ok(None) => None,
            // One request the machinery refused is that request's failure:
            // the pages that ran keep their results and the part goes on.
            Err(WorkError::Invalid | WorkError::Capacity | WorkError::Unavailable) => None,
            Err(_) => Some(WorkAttemptStatus::Failed),
        };
        let execution = self.run.execution().await.ok();
        let new: Vec<WorkStepFact> = execution
            .as_ref()
            .map(|execution| {
                execution
                    .steps
                    .iter()
                    .filter(|s| !before.contains(&s.id) && s.part == self.part)
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        if new.iter().any(|step| {
            step.account.is_some()
                || matches!(
                    step.kind,
                    WorkStepKindV1::List { .. }
                        | WorkStepKindV1::ReadFile { .. }
                        | WorkStepKindV1::SearchFiles { .. }
                        | WorkStepKindV1::RunCommand { .. }
                )
        }) {
            self.run.mark_private();
        }
        let mut used_steps = BTreeSet::new();
        let mut results = Vec::new();
        for request in &requests {
            let step = new.iter().find(|step| {
                !used_steps.contains(&step.id) && same_request(&request.kind, &step.kind)
            });
            let (content, error) = match (step, &execution) {
                (Some(step), Some(execution)) => {
                    used_steps.insert(step.id);
                    let asked = new
                        .iter()
                        .filter(|s| {
                            matches!(
                                s.kind,
                                WorkStepKindV1::Ask { .. } | WorkStepKindV1::Confirm { .. }
                            )
                        })
                        .collect::<Vec<_>>();
                    self.digest(execution, step, &asked, &driver)
                }
                _ => (
                    notices
                        .first()
                        .cloned()
                        .unwrap_or_else(|| "Not run: the part's budget or steps are spent".into()),
                    true,
                ),
            };
            results.push((request.call.clone(), content, error));
        }
        if let (Some(notice), Some((_, content, _))) = (notices.first(), results.last_mut()) {
            if !content.contains(notice.as_str()) {
                content.push_str("\nNote: ");
                content.push_str(notice);
            }
        }
        let count = |search: bool| {
            requests
                .iter()
                .filter(|r| matches!(r.kind, WorkStepKindV1::Search { .. }) == search)
                .count()
        };
        let pages = requests
            .iter()
            .filter(|r| matches!(r.kind, WorkStepKindV1::Read { .. }))
            .count();
        self.run.report(super::WorkLeadDiagnostic::Fetched {
            part: self.part.is_some(),
            searches: count(true),
            pages,
            others: count(false) - pages,
            failed: results.iter().filter(|(_, _, error)| *error).count(),
            elapsed_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
        });
        match terminal {
            Some(status) if status != WorkAttemptStatus::Succeeded => {
                crate::work_trace::record(format_args!(
                    "work: phase=lead event=hands_ended status={status:?} part={}",
                    self.part.is_some()
                ));
                Err(status)
            }
            _ => Ok(results),
        }
    }

    /// Decides, before its pages open, the person's entry for every site
    /// this call works on as them: one question for the run's parts that
    /// start together. A terminal status means the run ended on it.
    async fn enter_sites(
        &self,
        driver: &mut WorkPartDriver,
        requests: &[Request],
    ) -> Option<WorkAttemptStatus> {
        let mut hosts: Vec<(String, String)> = Vec::new();
        let mut public: Vec<String> = Vec::new();
        for request in requests {
            if let WorkStepKindV1::Read {
                url, goal: Some(_), ..
            } = &request.kind
            {
                let (Some(site), Some(host)) = (
                    crate::work_sites::site_of(url),
                    crate::work_sites::host_of(url),
                ) else {
                    continue;
                };
                if !(request.mine || self.personal || crate::work_sites::personal_page(url)) {
                    if !public.contains(&site) {
                        public.push(site);
                    }
                    continue;
                }
                if !hosts.iter().any(|(known, _)| *known == site) {
                    hosts.push((site, host));
                }
            }
        }
        public.retain(|site| !hosts.iter().any(|(known, _)| known == site));
        let personal: Vec<String> = hosts.iter().map(|(site, _)| site.clone()).collect();
        driver.scope_sites(&public, &personal);
        if hosts.is_empty() {
            return None;
        }
        let desk = &self.browser.entries;
        let preparing = desk.prepare();
        let undecided = driver
            .undecided_entries(hosts.iter().map(|(site, _)| site.clone()).collect())
            .await;
        let mine: Vec<(String, String)> = hosts
            .into_iter()
            .filter(|(site, _)| undecided.contains(site))
            .map(|(site, host)| (site, crate::work_sites::service_name(&host)))
            .collect();
        preparing.finish(&mine);
        if mine.is_empty() {
            return None;
        }
        loop {
            match desk.next(&mine).await {
                EntryTurn::Ask(wave) => {
                    let mut names: Vec<String> = Vec::new();
                    for (_, name) in &wave {
                        if !names.contains(name) {
                            names.push(name.clone());
                        }
                    }
                    crate::work_trace::record(format_args!(
                        "work: phase=entry state=asked services={} sites={}",
                        names.len(),
                        wave.len()
                    ));
                    match driver.ask_entry(&names).await {
                        Ok(Ok(answer)) => desk.publish(&wave, Some(answer)),
                        Ok(Err(status)) => {
                            desk.publish(&wave, None);
                            return Some(status);
                        }
                        Err(_) => {
                            desk.publish(&wave, None);
                            return Some(WorkAttemptStatus::Failed);
                        }
                    }
                }
                EntryTurn::Decided(answers) => {
                    for (site, answer) in answers {
                        driver.enter(&site, answer).await;
                    }
                    return None;
                }
                EntryTurn::Stopped => return Some(WorkAttemptStatus::Cancelled),
            }
        }
    }

    fn digest(
        &self,
        execution: &WorkExecutionFact,
        step: &WorkStepFact,
        asked: &[&WorkStepFact],
        driver: &WorkPartDriver,
    ) -> (String, bool) {
        let failed = step.status != WorkStepStatus::Succeeded;
        let mut out = String::new();
        if failed {
            out.push_str(match step.status {
                WorkStepStatus::Cancelled => "Stopped",
                WorkStepStatus::OutcomeUnknown => "Lost",
                _ => "Failed",
            });
            if let Some(note) = &step.note {
                out.push_str(": ");
                out.push_str(note);
            }
            out.push('\n');
        }
        for held in asked {
            match &held.kind {
                WorkStepKindV1::Confirm { confirm } => out.push_str(&format!(
                    "Held for the person on {}: {} — {}\n",
                    confirm.site,
                    confirm.headline,
                    match (confirm.decision, held.status) {
                        (Some(WorkConfirmDecisionV1::Declined), _) => "they declined",
                        (_, WorkStepStatus::Succeeded) => "done",
                        (Some(_), _) => "approved, but it did not complete",
                        (None, _) => "not decided",
                    }
                )),
                WorkStepKindV1::Ask {
                    answer: Some(answer),
                    ..
                } => out.push_str(&format!(
                    "The person answered the site question: {answer}\n"
                )),
                _ => {}
            }
        }
        match &step.kind {
            WorkStepKindV1::Search { .. } => {
                if let Some(record) = step
                    .evidence
                    .and_then(|id| execution.provider_evidence.iter().find(|r| r.id == id))
                {
                    let answer: String = record
                        .evidence
                        .answer
                        .chars()
                        .take(SEARCH_ANSWER_CHARS)
                        .collect();
                    out.push_str(answer.trim());
                    out.push_str("\nSources:\n");
                    let mut seen = BTreeSet::new();
                    for (index, citation) in record.evidence.citations.iter().enumerate() {
                        if seen.len() >= SEARCH_SOURCES || !seen.insert(citation.url.clone()) {
                            continue;
                        }
                        let key = self.run.cite(
                            WorkEvidenceLink {
                                extraction_id: record.id,
                                source_id: (index + 1) as u16,
                            },
                            &citation.title,
                            Some(&citation.url),
                        );
                        out.push_str(&format!(
                            "[{key}] {} — {}\n",
                            clip(&citation.title, 90),
                            citation.url
                        ));
                    }
                }
            }
            WorkStepKindV1::Read { url, goal, .. } => {
                let measured = step.measurements.as_ref();
                crate::work_trace::record(format_args!(
                    "work: phase=page site={} task={} status={:?} wall_ms={} calls={} actions={} tokens={} records={}",
                    crate::work_sites::site_of(url).unwrap_or_default(),
                    goal.is_some(),
                    step.status,
                    measured.map_or(0, |m| m.wall_millis),
                    measured.map_or(0, |m| m.planner_calls),
                    measured.map_or(0, |m| m.native_actions),
                    measured.map_or(0, |m| m.model_tokens),
                    step.artifacts.len(),
                ));
                let title = step
                    .local
                    .as_ref()
                    .and_then(|local| local.page_title.clone())
                    .unwrap_or_else(|| host(url));
                for id in &step.artifacts {
                    if let Some(artifact) = execution.artifacts.iter().find(|a| a.id == *id) {
                        out.push_str(&self.records(artifact, &title, url, driver));
                    }
                }
                if step.artifacts.is_empty() && !failed {
                    out.push_str("The page gave nothing usable.\n");
                }
                if let Some((_, said)) = step
                    .note
                    .as_deref()
                    .filter(|_| !failed)
                    .and_then(|note| note.split_once(" · "))
                {
                    out.push_str(said);
                    out.push('\n');
                }
            }
            kind if kind.files() || matches!(kind, WorkStepKindV1::RunCommand { .. }) => {
                if let Some(record) = step
                    .evidence
                    .and_then(|id| execution.file_evidence.iter().find(|r| r.id == id))
                {
                    let key = self.run.cite(
                        WorkEvidenceLink {
                            extraction_id: record.id,
                            source_id: 1,
                        },
                        &record.file.name,
                        None,
                    );
                    out.push_str(&format!("[{key}] {}\n", record.file.path));
                    out.push_str(&clip(&record.file.text, FILE_TEXT_BYTES));
                } else if let Some(record) = step
                    .evidence
                    .and_then(|id| execution.command_evidence.iter().find(|r| r.id == id))
                {
                    let key = self.run.cite(
                        WorkEvidenceLink {
                            extraction_id: record.id,
                            source_id: 1,
                        },
                        &record.command.command,
                        None,
                    );
                    out.push_str(&format!(
                        "[{key}] exit {}\n{}",
                        record
                            .command
                            .exit
                            .map_or("none".into(), |code| code.to_string()),
                        clip(&record.command.text, FILE_TEXT_BYTES)
                    ));
                } else if let Some(note) = &step.note {
                    if !failed {
                        out.push_str(note);
                    }
                }
            }
            _ => {}
        }
        if out.trim().is_empty() {
            out.push_str(if failed { "Failed" } else { "Done" });
        }
        (out, failed)
    }

    /// A page's records as lines the model can build objects from: names,
    /// facts, pictures and links, each with the key of its source.
    fn records(
        &self,
        artifact: &WorkArtifactV1,
        title: &str,
        url: &str,
        driver: &WorkPartDriver,
    ) -> String {
        let keys: Vec<String> = artifact
            .evidence
            .iter()
            .map(|link| {
                let page = driver
                    .preview(link)
                    .and_then(|p| p.link_destination.clone())
                    .unwrap_or_else(|| url.to_owned());
                self.run.allow_url(&page);
                self.run.cite(link.clone(), title, Some(&page))
            })
            .collect();
        let cite = |indices: &[u16]| -> String {
            let keys: Vec<&str> = indices
                .iter()
                .filter_map(|i| keys.get(usize::from(*i)).map(String::as_str))
                .collect();
            if keys.is_empty() {
                String::new()
            } else {
                format!(" [{}]", keys.join(", "))
            }
        };
        let all = if keys.is_empty() {
            String::new()
        } else {
            format!(" [{}]", keys[0])
        };
        // The page's own structured data names a picture for each item; a
        // record whose card showed none takes it from there, under the key
        // of the facts it came from.
        let facts: Vec<(String, String, String)> = artifact
            .evidence
            .iter()
            .zip(&keys)
            .filter_map(|(link, key)| Some((driver.preview(link)?.text.as_str(), key)))
            .flat_map(|(text, key)| {
                facts_pictures(text)
                    .into_iter()
                    .map(move |(page, image)| (page, image, key.clone()))
            })
            .collect();
        let subject = |subject: &WorkSubject| {
            let mut line = subject.name.clone();
            if let Some(descriptor) = &subject.descriptor {
                line.push_str(&format!(" — {descriptor}"));
            }
            if let Some(homepage) = &subject.homepage {
                self.run.allow_url(homepage);
                line.push_str(&format!(" · url {homepage}"));
            }
            for image in &subject.image_candidates {
                self.run.allow_url(image);
                line.push_str(&format!(" · photo {image}"));
            }
            if subject.image_candidates.is_empty() {
                let page = subject.homepage.as_deref().and_then(url_path);
                if let Some((_, image, key)) =
                    facts.iter().find(|(path, ..)| Some(path) == page.as_ref())
                {
                    self.run.allow_url(image);
                    line.push_str(&format!(" · photo {image} [{key}]"));
                }
            }
            line
        };
        let mut out = format!("{} ({title}){all}\n", artifact.title);
        match &artifact.data {
            WorkArtifactDataV1::ComparisonMatrix {
                subjects,
                criteria,
                cells,
                notes,
            } => {
                for (row, item) in subjects.iter().zip(cells).take(RECORD_LINES) {
                    let mut line = format!("- {}", subject(row));
                    for (criterion, cell) in criteria.iter().zip(item) {
                        let value = match &cell.value {
                            WorkCellValue::Text { text } => text.clone(),
                            WorkCellValue::Measurement { value } => value.clone(),
                            WorkCellValue::Money {
                                amount, currency, ..
                            } => format!("{amount} {currency}"),
                            WorkCellValue::Rating { value } => value.to_string(),
                            WorkCellValue::Presence { present } => {
                                if *present { "yes" } else { "no" }.into()
                            }
                            WorkCellValue::Unknown => continue,
                        };
                        line.push_str(&format!(" · {}: {value}", criterion.name));
                        line.push_str(&cite(&cell.evidence));
                    }
                    out.push_str(&clip(&line, 900));
                    out.push('\n');
                }
                for note in notes {
                    out.push_str(&format!("Note: {note}\n"));
                }
            }
            WorkArtifactDataV1::Findings { subjects, items } => {
                for item in subjects.iter().take(RECORD_LINES) {
                    out.push_str(&format!("- {}\n", subject(item)));
                }
                for finding in items.iter().take(RECORD_LINES) {
                    let mut line = format!("• {}", finding.claim);
                    if let Some(detail) = &finding.detail {
                        line.push_str(&format!(" ({detail})"));
                    }
                    line.push_str(&cite(&finding.evidence));
                    out.push_str(&clip(&line, 600));
                    out.push('\n');
                }
            }
            data => {
                out.push_str(&clip(&data.plain_text(), 3_000));
                out.push('\n');
            }
        }
        out
    }
}

/// The run's search provider behind its shared search permits.
struct SearchLane<'a> {
    inner: &'a dyn WorkPublicSearchProvider,
    permits: &'a tokio::sync::Semaphore,
}
impl WorkPublicSearchProvider for SearchLane<'_> {
    fn reuse<'a>(
        &'a self,
        scope: &'a WorkPublicSearchScope,
        earlier: &'a str,
        evidence: &'a WorkProviderSearchEvidenceV1,
        limits: WorkExecutionLimits,
        deadline: std::time::Instant,
    ) -> WorkPublicSearchReuseFuture<'a> {
        Box::pin(async move {
            let _permit = self.permits.acquire().await;
            self.inner
                .reuse(scope, earlier, evidence, limits, deadline)
                .await
        })
    }
    fn enough<'a>(
        &'a self,
        scope: &'a WorkPublicSearchScope,
        found: &'a str,
        limits: WorkExecutionLimits,
        deadline: std::time::Instant,
    ) -> WorkPublicSearchReuseFuture<'a> {
        Box::pin(async move {
            let _permit = self.permits.acquire().await;
            self.inner.enough(scope, found, limits, deadline).await
        })
    }
    fn minimum_reservation(
        &self,
        scope: &WorkPublicSearchScope,
        context: &[zephium_core::work::context::WorkContextBody],
    ) -> Option<WorkUsage> {
        self.inner.minimum_reservation(scope, context)
    }
    fn rerank<'a>(
        &'a self,
        scope: &'a WorkPublicSearchScope,
        evidence: &'a WorkProviderSearchEvidenceV1,
        limits: WorkExecutionLimits,
        deadline: std::time::Instant,
    ) -> WorkPublicSearchRankingFuture<'a> {
        Box::pin(async move {
            let _permit = self.permits.acquire().await;
            self.inner.rerank(scope, evidence, limits, deadline).await
        })
    }
    fn search<'a>(
        &'a self,
        scope: &'a WorkPublicSearchScope,
        context: &'a [zephium_core::work::context::WorkContextBody],
        limits: WorkExecutionLimits,
    ) -> WorkPublicSearchFuture<'a> {
        Box::pin(async move {
            let _permit = self.permits.acquire().await;
            self.inner.search(scope, context, limits).await
        })
    }
}

fn url_path(url: &str) -> Option<String> {
    url::Url::parse(url)
        .ok()
        .map(|url| url.path().trim_end_matches('/').to_ascii_lowercase())
        .filter(|path| !path.is_empty())
}

/// Each item's page and picture in a page's structured-data facts
/// ("product: name | ... | image: url | url: page ;; ...").
fn facts_pictures(text: &str) -> Vec<(String, String)> {
    text.split(" ;; ")
        .filter_map(|item| {
            let pairs: Vec<(&str, &str)> = item
                .split(" | ")
                .filter_map(|pair| pair.split_once(": "))
                .collect();
            let find = |key: &str| {
                pairs
                    .iter()
                    .find(|(k, _)| k.trim() == key)
                    .map(|(_, v)| v.trim())
                    .filter(|v| v.starts_with("https://"))
            };
            Some((url_path(find("url")?)?, find("image")?.to_owned()))
        })
        .collect()
}

fn host(url: &str) -> String {
    url::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_owned))
        .unwrap_or_else(|| "the page".into())
}

/// The step the driver made for a request: same query, url or path. A page
/// task may start from the site's entry page, so its goal decides.
fn same_request(request: &WorkStepKindV1, step: &WorkStepKindV1) -> bool {
    match (request, step) {
        (WorkStepKindV1::Search { query: a }, WorkStepKindV1::Search { query: b }) => a == b,
        (
            WorkStepKindV1::Read {
                url: a, goal: None, ..
            },
            WorkStepKindV1::Read {
                url: b, goal: None, ..
            },
        ) => a == b,
        (
            WorkStepKindV1::Read { goal: Some(a), .. },
            WorkStepKindV1::Read { goal: Some(b), .. },
        ) => a == b,
        (WorkStepKindV1::List { path: a, .. }, WorkStepKindV1::List { path: b, .. })
        | (WorkStepKindV1::ReadFile { path: a, .. }, WorkStepKindV1::ReadFile { path: b, .. })
        | (WorkStepKindV1::WriteFile { path: a, .. }, WorkStepKindV1::WriteFile { path: b, .. })
        | (WorkStepKindV1::EditFile { path: a, .. }, WorkStepKindV1::EditFile { path: b, .. })
        | (
            WorkStepKindV1::DeleteFile { path: a, .. },
            WorkStepKindV1::DeleteFile { path: b, .. },
        )
        | (WorkStepKindV1::MoveFile { from: a, .. }, WorkStepKindV1::MoveFile { from: b, .. })
        | (
            WorkStepKindV1::RunCommand { command: a, .. },
            WorkStepKindV1::RunCommand { command: b, .. },
        ) => a == b,
        (
            WorkStepKindV1::SearchFiles { query: a, .. },
            WorkStepKindV1::SearchFiles { query: b, .. },
        ) => a == b,
        _ => false,
    }
}

/// A `records` argument as the page machinery's collection, if valid.
pub(crate) fn collection(value: Option<&Value>) -> Result<Option<WorkBrowseCollection>, String> {
    let Some(value) = value.filter(|v| !v.is_null()) else {
        return Ok(None);
    };
    let mut collection: WorkBrowseCollection = serde_json::from_value(value.clone())
        .map_err(|error| format!("records does not match its shape: {error}"))?;
    // A row missing a price or a picture is still a find the lead can use;
    // only a link is ever required. A price is kept as the page shows it:
    // most sites print "$" without a currency code, which a money value
    // never admits, and the lead reads records as text either way.
    for column in &mut collection.columns {
        use zephium_core::work::collection::{WorkBrowseExtraction, WorkBrowseValue};
        if matches!(column.value, WorkBrowseValue::Money { .. }) {
            column.value = WorkBrowseValue::Text;
            column.extraction = WorkBrowseExtraction::Generate;
        }
        if column.value != WorkBrowseValue::Url {
            column.required = false;
        }
    }
    collection.validate().map_err(|_| {
        "records needs a title, 1 to 32 max_items, 1 to 16 distinct ASCII column names other than name, at most three image_url columns, money only with generate, and (columns + 1) × max_items ≤ 256".to_owned()
    })?;
    Ok(Some(collection))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[derive(Default)]
    struct Counting {
        live: AtomicUsize,
        most: AtomicUsize,
    }
    impl WorkPublicSearchProvider for Counting {
        fn search<'a>(
            &'a self,
            _: &'a WorkPublicSearchScope,
            _: &'a [zephium_core::work::context::WorkContextBody],
            _: WorkExecutionLimits,
        ) -> WorkPublicSearchFuture<'a> {
            Box::pin(async move {
                let live = self.live.fetch_add(1, Ordering::SeqCst) + 1;
                self.most.fetch_max(live, Ordering::SeqCst);
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                self.live.fetch_sub(1, Ordering::SeqCst);
                Err(WorkPublicSearchError::NotDispatched(WorkError::Unavailable))
            })
        }
    }

    #[test]
    fn parts_that_start_together_ask_one_entry_question_for_all_their_services() {
        use crate::work_sites::EntryAnswer;
        let desk = std::sync::Arc::new(EntryDesk::default());
        let asked = std::sync::Arc::new(Mutex::new(Vec::new()));
        let part = |sites: &[(&str, &str)]| {
            let desk = desk.clone();
            let asked = asked.clone();
            let mine: Vec<(String, String)> = sites
                .iter()
                .map(|(site, name)| ((*site).to_owned(), (*name).to_owned()))
                .collect();
            async move {
                loop {
                    match desk.next(&mine).await {
                        EntryTurn::Ask(wave) => {
                            asked.lock().unwrap().push(wave.len());
                            desk.publish(&wave, Some(EntryAnswer::Allow));
                        }
                        EntryTurn::Decided(answers) => return answers.len(),
                        EntryTurn::Stopped => return 0,
                    }
                }
            }
        };
        let decided = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                let early = tokio::join!(
                    part(&[("slack.com", "Slack")]),
                    part(&[("google.com", "Gmail")]),
                    async {
                        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                        part(&[("google.com", "Calendar"), ("linear.app", "Linear")]).await
                    },
                );
                let late = part(&[("github.com", "GitHub")]).await;
                (early, late)
            });
        assert_eq!(decided, ((1, 1, 2), 1));
        assert_eq!(*asked.lock().unwrap(), [3, 1]);
    }

    #[test]
    fn entry_wave_waits_for_sibling_presence_beyond_the_gather_window() {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                let desk = EntryDesk::default();
                let first = vec![("slack.com".into(), "Slack".into())];
                let second = vec![("linear.app".into(), "Linear".into())];
                let third = vec![("notion.so".into(), "Notion".into())];
                let first_query = desk.prepare();
                let second_query = desk.prepare();
                let slow_query = desk.prepare();
                first_query.finish(&first);
                second_query.finish(&second);
                let question = desk.next(&first);
                tokio::pin!(question);
                assert!(tokio::time::timeout(
                    ENTRY_GATHER + std::time::Duration::from_millis(20),
                    &mut question,
                )
                .await
                .is_err());
                slow_query.finish(&third);
                let EntryTurn::Ask(wave) = question.await else {
                    panic!("combined question missing")
                };
                assert_eq!(
                    wave,
                    [first[0].clone(), second[0].clone(), third[0].clone()]
                );
                desk.publish(&wave, Some(crate::work_sites::EntryAnswer::Allow));
                for mine in [&first, &second, &third] {
                    let EntryTurn::Decided(answer) = desk.next(mine).await else {
                        panic!("answer missing")
                    };
                    assert_eq!(answer.len(), 1);
                }
            });
    }

    #[test]
    fn empty_and_canceled_presence_release_the_wave_without_inventing_sites() {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                let desk = EntryDesk::default();
                let mine = vec![("slack.com".into(), "Slack".into())];
                let admitted = desk.prepare();
                let empty = desk.prepare();
                let canceled = desk.prepare();
                admitted.finish(&mine);
                empty.finish(&[]);
                let question = desk.next(&mine);
                tokio::pin!(question);
                assert!(tokio::time::timeout(
                    ENTRY_GATHER + std::time::Duration::from_millis(20),
                    &mut question,
                )
                .await
                .is_err());
                drop(canceled);
                let EntryTurn::Ask(wave) = question.await else {
                    panic!("question stranded")
                };
                assert_eq!(wave, mine);
                assert_eq!(desk.state.lock().unwrap().preparing, 0);
            });
    }
    #[test]
    fn a_catalog_items_picture_comes_from_the_page_facts() {
        let facts = "product: London | price: 39.99 USD | image: https://www.lego.com/cdn/a/21034.jpg?width=800 | url: https://www.lego.com/en-us/product/london-21034 ;; product: Paris | url: https://www.lego.com/en-us/product/paris-21064";
        assert_eq!(
            facts_pictures(facts),
            [(
                "/en-us/product/london-21034".to_owned(),
                "https://www.lego.com/cdn/a/21034.jpg?width=800".to_owned()
            )]
        );
        assert!(facts_pictures("A page about London").is_empty());
    }

    #[test]
    fn a_price_column_keeps_the_price_as_the_page_shows_it() {
        use zephium_core::work::collection::{WorkBrowseExtraction, WorkBrowseValue};
        let records = serde_json::json!({
            "title": "Flights", "max_items": 8,
            "columns": [
                {"name": "price", "value": {"kind": "money", "permitted_currencies": ["USD"]},
                 "required": true, "extraction": "generate"},
                {"name": "url", "value": {"kind": "url"}, "required": true}
            ]
        });
        let collection = collection(Some(&records)).unwrap().unwrap();
        assert_eq!(collection.columns[0].value, WorkBrowseValue::Text);
        assert_eq!(
            collection.columns[0].extraction,
            WorkBrowseExtraction::Generate
        );
        assert!(!collection.columns[0].required && collection.columns[1].required);
    }

    #[test]
    fn a_runs_searches_wait_for_a_provider_slot_instead_of_crowding_it() {
        let provider = Counting::default();
        let permits = tokio::sync::Semaphore::new(LIVE_SEARCHES);
        let lane = SearchLane {
            inner: &provider,
            permits: &permits,
        };
        let scope = WorkPublicSearchScope {
            provider: WorkSearchProvider::OpenAi,
            model: PUBLIC_SEARCH_MODEL.into(),
            query: "public query".into(),
        };
        let limits = WorkExecutionLimits {
            model_tokens: 1,
            cost_micro_usd: 1,
            operations: 1,
            timeout_seconds: 1,
            max_workers: 1,
        };
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                let mut pending: Vec<_> =
                    (0..8).map(|_| lane.search(&scope, &[], limits)).collect();
                std::future::poll_fn(|cx| {
                    pending.retain_mut(|search| search.as_mut().poll(cx).is_pending());
                    if pending.is_empty() {
                        std::task::Poll::Ready(())
                    } else {
                        std::task::Poll::Pending
                    }
                })
                .await;
            });
        assert_eq!(provider.most.load(Ordering::SeqCst), LIVE_SEARCHES);
    }
}
