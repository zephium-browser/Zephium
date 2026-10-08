//! One lead run's shared state: the attempt, its budget, its sources and its
//! waits. Parts run concurrently over it; no lock is held across an await.
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

use zephium_core::ids::ProfileId;
use zephium_core::work::{artifact::*, parts::*, runtime::*, *};
use zephium_ipc::work::WorkActivityV1;

use crate::work_runtime::{WorkAttemptProbe, WorkCancelCause};

/// How long a question or a proposed change waits for the person before the
/// run stops on it; the next request answers it.
pub(crate) const WAIT_PATIENCE: Duration = Duration::from_secs(30 * 60);
const POLL: Duration = Duration::from_millis(500);
const MAX_UNREADABLE_POLLS: u8 = 12;
pub(crate) const MAX_SOURCES: usize = 160;
/// The address question's option words; the frame sends them back as given.
const ADDRESS_OPEN: &str = "Open";
const ADDRESS_DECLINE: &str = "Don\u{2019}t open";

/// A page, search result, file or command output an object may cite by key.
#[derive(Clone)]
pub(crate) struct LeadSource {
    pub key: String,
    pub link: WorkEvidenceLink,
    pub url: Option<String>,
}

#[derive(Clone, Default)]
pub(crate) struct Folders {
    /// Attached with this request or allowed for it, in grant order.
    pub current: Vec<String>,
    /// On the canvas from earlier requests: usable when the request is about them.
    pub available: Vec<String>,
    pub grant: Option<crate::work_files::WorkFileGrant>,
}
impl Folders {
    fn admit(&mut self) {
        let all: Vec<String> = self
            .current
            .iter()
            .chain(&self.available)
            .cloned()
            .collect();
        let (files, _) = crate::work_files::WorkFileGrant::admit(&all);
        self.grant = (!files.is_empty()).then_some(files);
    }
}

struct State {
    turn: u8,
    limits: WorkExecutionLimits,
    used: WorkUsage,
    sources: Vec<LeadSource>,
    next_source: u32,
    /// Links the run was given outside its sources: the request, context,
    /// and addresses pages showed.
    allowed: Vec<String>,
    /// The run has read something of the person's own: their tabs, history,
    /// notes, memory, files, connected services or a signed-in page.
    private: bool,
    /// Sites the person named in the request or while it ran, and sites they
    /// let the run open addresses on.
    trusted_sites: Vec<String>,
    waits: usize,
    waiting_since: Option<Instant>,
    stopped: Option<WorkCancelCause>,
    unreadable: u8,
}

pub(crate) struct LeadRun {
    pub probe: WorkAttemptProbe,
    pub handle: crate::Handle,
    pub profile: ProfileId,
    pub grant: WorkAgentGrantV1,
    /// Folders this run may read, the request's own first; a folder the
    /// person allows mid-run joins them.
    folders: Mutex<Folders>,
    /// The run's one output: every object it places is minted under it.
    pub output: WorkExpectedOutput,
    state: Mutex<State>,
    /// Connections the person chose for this run where they were asked for
    /// something broader (their day's sources): no second question.
    accepted: Mutex<Vec<String>>,
    diagnostic: Option<fn(super::WorkLeadDiagnostic)>,
}

/// Stands still the run's deadline while it lives: the person is deciding.
pub(crate) struct Waiting<'a>(&'a LeadRun);
impl Drop for Waiting<'_> {
    fn drop(&mut self) {
        let mut state = self.0.state();
        state.waits = state.waits.saturating_sub(1);
        if state.waits == 0 {
            if let Some(since) = state.waiting_since.take() {
                self.0.probe.resume_after_wait(since.elapsed());
            }
        }
    }
}

impl LeadRun {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        probe: WorkAttemptProbe,
        handle: crate::Handle,
        profile: ProfileId,
        grant: WorkAgentGrantV1,
        limits: WorkExecutionLimits,
        folders: (Vec<String>, Vec<String>),
        output: WorkExpectedOutput,
        diagnostic: Option<fn(super::WorkLeadDiagnostic)>,
    ) -> Self {
        Self {
            probe,
            handle,
            profile,
            grant,
            folders: Mutex::new({
                let mut folders = Folders {
                    current: folders.0,
                    available: folders.1,
                    grant: None,
                };
                folders.admit();
                folders
            }),
            output,
            state: Mutex::new(State {
                turn: 0,
                limits,
                used: WorkUsage::default(),
                sources: Vec::new(),
                next_source: 0,
                allowed: Vec::new(),
                private: false,
                trusted_sites: Vec::new(),
                waits: 0,
                waiting_since: None,
                stopped: None,
                unreadable: 0,
            }),
            accepted: Mutex::new(Vec::new()),
            diagnostic,
        }
    }
    /// The person chose this connection for the run, by the answer that
    /// uses it ("Use Slack").
    pub(crate) fn accept_connection(&self, connection: &str) {
        let mut accepted = self
            .accepted
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !accepted.iter().any(|known| known == connection) {
            accepted.push(connection.to_owned());
        }
    }
    pub(crate) fn accepted_connection(&self, connection: &str) -> bool {
        self.accepted
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .any(|known| known == connection)
    }
    fn state(&self) -> MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
    pub(crate) fn folders(&self) -> Folders {
        self.folders
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
    /// The granted folders as the file tools resolve them.
    pub(crate) fn files(&self) -> Option<crate::work_files::WorkFileGrant> {
        self.folders().grant
    }
    /// Adds a folder the person allowed while the run works; it leads.
    pub(crate) fn grant_folder(&self, folder: &str) {
        let mut folders = self
            .folders
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        folders.available.retain(|known| known != folder);
        if !folders.current.iter().any(|known| known == folder) {
            folders.current.insert(0, folder.to_owned());
        }
        folders.admit();
    }
    pub(crate) fn report(&self, event: super::WorkLeadDiagnostic) {
        if let Some(diagnostic) = self.diagnostic {
            diagnostic(event);
        }
    }

    pub(crate) fn next_turn(&self) -> u8 {
        let mut state = self.state();
        state.turn = state.turn.saturating_add(1);
        state.turn
    }
    pub(crate) fn turn(&self) -> u8 {
        self.state().turn.max(1)
    }
    pub(crate) fn limits(&self) -> WorkExecutionLimits {
        self.state().limits
    }
    pub(crate) fn set_limits(&self, limits: WorkExecutionLimits) {
        self.state().limits = limits;
    }
    pub(crate) fn used(&self) -> WorkUsage {
        self.state().used
    }
    pub(crate) fn charge(&self, usage: WorkUsage) {
        let mut state = self.state();
        state.used.model_tokens = state.used.model_tokens.saturating_add(usage.model_tokens);
        state.used.cost_micro_usd = state
            .used
            .cost_micro_usd
            .saturating_add(usage.cost_micro_usd);
        state.used.operations = state
            .used
            .operations
            .saturating_add(usage.operations.max(1));
        if usage.accounting == WorkUsageAccounting::ConservativeReservation {
            state.used.accounting = WorkUsageAccounting::ConservativeReservation;
        }
    }
    /// What is left of each limit, never below one so shares stay valid.
    pub(crate) fn remaining(&self) -> WorkExecutionLimits {
        let state = self.state();
        WorkExecutionLimits {
            model_tokens: state
                .limits
                .model_tokens
                .saturating_sub(state.used.model_tokens)
                .max(1),
            cost_micro_usd: state
                .limits
                .cost_micro_usd
                .saturating_sub(state.used.cost_micro_usd)
                .max(1),
            operations: state
                .limits
                .operations
                .saturating_sub(state.used.operations)
                .max(1),
            ..state.limits
        }
    }

    pub(crate) fn activity(&self, activity: WorkActivityV1) {
        self.probe.record_activity(activity);
    }

    /// Why the run was told to stop, once it was.
    pub(crate) fn stop_cause(&self) -> Option<WorkCancelCause> {
        self.state().stopped
    }
    /// Asks the store whether the run must stop; the deadline counts only
    /// while nobody is being waited on.
    pub(crate) async fn cancelled(&self) -> bool {
        if self.state().stopped.is_some() {
            return true;
        }
        let waiting = self.state().waits > 0;
        let cause = match self.probe.cancellation_cause().await {
            Ok(Some(cause)) => Some(cause),
            Ok(None) if !waiting && Instant::now() >= self.probe.deadline() => {
                Some(WorkCancelCause::Deadline)
            }
            Ok(None) => {
                self.state().unreadable = 0;
                None
            }
            Err(_) => {
                let mut state = self.state();
                state.unreadable = state.unreadable.saturating_add(1);
                (state.unreadable >= MAX_UNREADABLE_POLLS).then_some(WorkCancelCause::Unreadable)
            }
        };
        if let Some(cause) = cause {
            self.state().stopped.get_or_insert(cause);
            self.report(super::WorkLeadDiagnostic::Stopped { cause });
        }
        cause.is_some()
    }
    pub(crate) fn wait(&self) -> Waiting<'_> {
        let mut state = self.state();
        if state.waits == 0 {
            state.waiting_since = Some(Instant::now());
        }
        state.waits += 1;
        Waiting(self)
    }
    /// Nobody answered in time: the run stops the way a person's Stop does.
    pub(crate) async fn suspend(&self) {
        self.report(super::WorkLeadDiagnostic::Suspended);
        let _ = self.probe.request_stop().await;
    }

    pub(crate) fn step(
        &self,
        kind: WorkStepKindV1,
        status: WorkStepStatus,
        part: Option<WorkPartId>,
    ) -> WorkStepFact {
        WorkStepFact {
            id: WorkStepId::generate(),
            turn: self.turn(),
            kind,
            status,
            usage: None,
            artifacts: vec![],
            evidence: None,
            note: None,
            measurements: None,
            local: None,
            account: None,
            part,
        }
    }
    pub(crate) async fn begin(
        &self,
        step: WorkStepFact,
        artifacts: Vec<WorkArtifactV1>,
    ) -> Result<WorkStepId, WorkError> {
        let id = step.id;
        self.probe
            .commit_step(WorkRuntimeUpdate::BeginStep {
                execution: self.probe.execution(),
                attempt: self.probe.attempt(),
                step,
                artifacts,
                evidence: None,
                file: None,
            })
            .await
            .inspect_err(|error| {
                self.report(super::WorkLeadDiagnostic::CommitRefused {
                    kind: "begin",
                    error: *error,
                })
            })?;
        Ok(id)
    }
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn settle(
        &self,
        step: WorkStepId,
        status: WorkStepStatus,
        usage: Option<WorkUsage>,
        note: Option<String>,
        file: Option<WorkFileRecordV1>,
    ) -> Result<(), WorkError> {
        self.probe
            .commit_step(WorkRuntimeUpdate::SettleStep {
                execution: self.probe.execution(),
                attempt: self.probe.attempt(),
                step,
                status,
                usage,
                artifacts: vec![],
                evidence: None,
                file: file.map(Box::new),
                note,
                measurements: None,
            })
            .await
            .map(|_| ())
            .inspect_err(|error| {
                self.report(super::WorkLeadDiagnostic::CommitRefused {
                    kind: "settle",
                    error: *error,
                })
            })
    }
    pub(crate) async fn part(&self, part: WorkPartFactV1) -> Result<(), WorkError> {
        self.probe
            .commit_step(WorkRuntimeUpdate::Part {
                execution: self.probe.execution(),
                attempt: self.probe.attempt(),
                part,
            })
            .await
            .map(|_| ())
    }
    pub(crate) async fn input(&self, input: WorkInputFactV1) {
        if input.kind != WorkInputKindV1::Skill {
            self.mark_private();
        }
        if let Err(error) = self
            .probe
            .commit_step(WorkRuntimeUpdate::Input {
                execution: self.probe.execution(),
                attempt: self.probe.attempt(),
                input,
            })
            .await
        {
            self.report(super::WorkLeadDiagnostic::CommitRefused {
                kind: "input",
                error,
            });
        }
    }
    pub(crate) async fn execution(&self) -> Result<WorkExecutionFact, WorkError> {
        self.probe
            .runtime_projection()
            .await?
            .executions
            .into_iter()
            .find(|execution| execution.id == self.probe.execution())
            .ok_or(WorkError::NotFound)
    }

    /// Puts one question to the person and waits for the answer. `Ok(None)`
    /// means the run stopped first or nobody answered within the patience.
    pub(crate) async fn ask(
        &self,
        purpose: WorkAskPurposeV1,
        prompt: String,
        options: Vec<String>,
        part: Option<WorkPartId>,
    ) -> Result<Option<String>, WorkError> {
        self.ask_with(purpose, prompt, options, part, None).await
    }
    /// A question whose step carries a local fact: the folder it asks for.
    pub(crate) async fn ask_with(
        &self,
        purpose: WorkAskPurposeV1,
        prompt: String,
        options: Vec<String>,
        part: Option<WorkPartId>,
        local: Option<WorkLocalStepV1>,
    ) -> Result<Option<String>, WorkError> {
        self.activity(WorkActivityV1::WaitingForHuman);
        let mut step = self.step(
            WorkStepKindV1::Ask {
                prompt,
                options,
                answer: None,
                purpose: Some(purpose),
            },
            WorkStepStatus::Running,
            part,
        );
        step.local = local.map(Box::new);
        let id = self.begin(step, vec![]).await?;
        let waiting = self.wait();
        let since = Instant::now();
        loop {
            tokio::time::sleep(POLL).await;
            let execution = self.execution().await?;
            let asked = execution
                .steps
                .iter()
                .find(|s| s.id == id)
                .ok_or(WorkError::NotFound)?;
            if asked.status == WorkStepStatus::Succeeded {
                drop(waiting);
                return Ok(match &asked.kind {
                    WorkStepKindV1::Ask { answer, .. } => answer.clone(),
                    _ => None,
                });
            }
            let expired = since.elapsed() >= WAIT_PATIENCE;
            if expired {
                self.suspend().await;
            }
            if expired || self.cancelled().await {
                drop(waiting);
                let note = if expired {
                    "Waiting for your answer"
                } else {
                    self.stop_note()
                };
                self.settle(id, WorkStepStatus::Cancelled, None, Some(note.into()), None)
                    .await?;
                return Ok(None);
            }
        }
    }
    /// Holds a committing step for the person; see `LeadToolContext::confirm`.
    pub(crate) async fn confirm(
        &self,
        confirm: WorkConfirmV1,
        part: Option<WorkPartId>,
    ) -> Result<Option<bool>, WorkError> {
        self.activity(WorkActivityV1::WaitingForHuman);
        let step = self.step(
            WorkStepKindV1::Confirm {
                confirm: Box::new(confirm),
            },
            WorkStepStatus::Running,
            part,
        );
        let id = self.begin(step, vec![]).await?;
        let waiting = self.wait();
        let since = Instant::now();
        loop {
            tokio::time::sleep(POLL).await;
            let execution = self.execution().await?;
            let decision =
                execution
                    .steps
                    .iter()
                    .find(|s| s.id == id)
                    .and_then(|s| match &s.kind {
                        WorkStepKindV1::Confirm { confirm } => confirm.decision,
                        _ => None,
                    });
            let settled = match decision {
                Some(WorkConfirmDecisionV1::Declined) => {
                    Some((false, WorkStepStatus::Cancelled, Some("Declined")))
                }
                Some(_) => Some((true, WorkStepStatus::Succeeded, None)),
                None => None,
            };
            if let Some((approved, status, note)) = settled {
                drop(waiting);
                self.settle(id, status, None, note.map(str::to_owned), None)
                    .await?;
                return Ok(Some(approved));
            }
            let expired = since.elapsed() >= WAIT_PATIENCE;
            if expired {
                self.suspend().await;
            }
            if expired || self.cancelled().await {
                drop(waiting);
                let note = if expired {
                    "Waiting for your decision"
                } else {
                    self.stop_note()
                };
                self.settle(id, WorkStepStatus::Cancelled, None, Some(note.into()), None)
                    .await?;
                return Ok(None);
            }
        }
    }
    /// Waits for the person's decision on a proposed step: a file change or
    /// a command. `Ok(None)` when the run stopped first.
    pub(crate) async fn decision(&self, step: WorkStepId) -> Result<Option<bool>, WorkError> {
        self.activity(WorkActivityV1::WaitingForHuman);
        let waiting = self.wait();
        let since = Instant::now();
        loop {
            let execution = self.execution().await?;
            let decision = execution
                .steps
                .iter()
                .find(|s| s.id == step)
                .ok_or(WorkError::NotFound)?
                .kind
                .file_decision();
            if decision.is_some() {
                drop(waiting);
                return Ok(decision);
            }
            if since.elapsed() >= WAIT_PATIENCE {
                self.suspend().await;
                return Ok(None);
            }
            if self.cancelled().await {
                return Ok(None);
            }
            tokio::time::sleep(POLL).await;
        }
    }
    /// Why the run stopped, in the person's words.
    pub(crate) fn stop_note(&self) -> &'static str {
        match self.stop_cause() {
            Some(WorkCancelCause::Deadline) => "The run ran out of time",
            Some(WorkCancelCause::Requested) => "Stopped by you",
            _ => "The run was interrupted",
        }
    }

    /// Registers a citable source and returns its key; a source already
    /// registered keeps its key.
    pub(crate) fn cite(&self, link: WorkEvidenceLink, _title: &str, url: Option<&str>) -> String {
        let mut state = self.state();
        if let Some(known) = state.sources.iter().find(|s| s.link == link) {
            return known.key.clone();
        }
        state.next_source += 1;
        let key = format!("s{}", state.next_source);
        if state.sources.len() >= MAX_SOURCES {
            state.sources.remove(0);
        }
        state.sources.push(LeadSource {
            key: key.clone(),
            link,
            url: url.map(str::to_owned),
        });
        key
    }
    pub(crate) fn source(&self, key: &str) -> Option<LeadSource> {
        self.state()
            .sources
            .iter()
            .find(|s| s.key == key.trim())
            .cloned()
    }
    /// A registered source whose page has this URL, for `web_fetch`.
    pub(crate) fn source_by_url(&self, url: &str) -> Option<LeadSource> {
        let wanted = url.trim_end_matches('/');
        self.state()
            .sources
            .iter()
            .find(|s| {
                s.url
                    .as_deref()
                    .is_some_and(|u| u.trim_end_matches('/') == wanted)
            })
            .cloned()
    }
    pub(crate) fn known_url(&self, url: &str) -> bool {
        let wanted = url.trim_end_matches('/');
        self.source_by_url(url).is_some()
            || self
                .state()
                .allowed
                .iter()
                .any(|allowed| allowed.trim_end_matches('/') == wanted)
    }
    /// Admits a link the run was given for a later read.
    pub(crate) fn allow_url(&self, url: &str) {
        if url::Url::parse(url).is_ok_and(|u| u.scheme() == "https") {
            let mut state = self.state();
            if !state.allowed.iter().any(|known| known == url) {
                if state.allowed.len() >= MAX_SOURCES * 4 {
                    state.allowed.remove(0);
                }
                state.allowed.push(url.to_owned());
            }
        }
    }
    /// Admits every https link in a text the person gave.
    pub(crate) fn allow_links_in(&self, text: &str) {
        for word in
            text.split(|c: char| c.is_whitespace() || matches!(c, '<' | '>' | '"' | '(' | ')'))
        {
            let word = word.trim_end_matches(['.', ',', ';', ':', '!', '?']);
            if word.starts_with("https://") {
                self.allow_url(word);
            }
        }
    }

    /// Trusts the sites the person named in their own words: the request and
    /// what they add while it runs, never attached text, which can quote
    /// anyone's links.
    pub(crate) fn trust_sites_in(&self, text: &str) {
        for word in
            text.split(|c: char| c.is_whitespace() || matches!(c, '<' | '>' | '"' | '(' | ')'))
        {
            let word = word.trim_end_matches(['.', ',', ';', ':', '!', '?']);
            if let Some(site) = named_site(word) {
                let mut state = self.state();
                if !state.trusted_sites.contains(&site) {
                    state.trusted_sites.push(site);
                }
            }
        }
    }

    /// Whether the person named this site or let the run work on it.
    pub(crate) fn trusts(&self, site: &str) -> bool {
        self.state().trusted_sites.iter().any(|known| known == site)
    }
    pub(crate) fn trust_site(&self, site: &str) {
        let mut state = self.state();
        if !state.trusted_sites.iter().any(|known| known == site) {
            state.trusted_sites.push(site.to_owned());
        }
    }

    /// From here on an address the model writes itself may carry what it
    /// read, so `admit_address` holds the ones nobody gave it.
    pub(crate) fn mark_private(&self) {
        self.state().private = true;
    }
    pub(crate) fn is_private(&self) -> bool {
        let private = self.state().private;
        private || self.folders().grant.is_some()
    }

    /// Lets a page address through, or asks the person first when the model
    /// wrote it itself after reading something of theirs: a prompt injection
    /// could otherwise have it carry that into a URL. Addresses the person
    /// gave, links pages showed, sites the person named and a site's bare
    /// home page need no question. `Err` is the model's answer.
    pub(crate) async fn admit_address(
        &self,
        url: &str,
        part: Option<WorkPartId>,
    ) -> Result<(), String> {
        if !self.is_private() || self.known_url(url) {
            return Ok(());
        }
        let Some(site) = crate::work_sites::site_of(url) else {
            return Err("that address is not a public web page".into());
        };
        if self.state().trusted_sites.contains(&site) || home_page(url, &site) {
            return Ok(());
        }
        let open = ADDRESS_OPEN.to_owned();
        let for_run = format!("Allow {site} for this request");
        let answer = self
            .ask(
                WorkAskPurposeV1::Address,
                url.to_owned(),
                vec![open.clone(), for_run.clone(), ADDRESS_DECLINE.into()],
                part,
            )
            .await
            .map_err(|_| "the person could not be asked".to_owned())?;
        match answer {
            Some(answer) if answer == open => {
                self.allow_url(url);
                Ok(())
            }
            Some(answer) if answer == for_run => {
                let mut state = self.state();
                if !state.trusted_sites.contains(&site) {
                    state.trusted_sites.push(site);
                }
                Ok(())
            }
            Some(_) => Err(format!(
                "The person chose not to open {url}. Continue with what you have, or use a link a page showed."
            )),
            None => Err(self.stop_note().to_owned()),
        }
    }
}

/// The registrable site a word of the person's names: a link, or a bare
/// domain such as "airbnb.com" or "www.lego.com".
fn named_site(word: &str) -> Option<String> {
    let word = word.trim_matches(|c: char| matches!(c, '\'' | '`' | '*'));
    if word.starts_with("https://") || word.starts_with("http://") {
        return crate::work_sites::site_of(word);
    }
    if !word.contains('.') || word.contains('@') || word.contains('/') {
        return None;
    }
    crate::work_sites::site_of(&format!("https://{}/", word.to_ascii_lowercase()))
}

/// A site's front door: no path, query or credentials, on the registrable
/// domain itself or its www host. Nothing in it can carry what a run read.
fn home_page(url: &str, site: &str) -> bool {
    let Ok(parsed) = url::Url::parse(url) else {
        return false;
    };
    let host = parsed.host_str().unwrap_or_default();
    parsed.scheme() == "https"
        && matches!(parsed.path(), "" | "/")
        && parsed.query().is_none()
        && parsed.fragment().is_none()
        && parsed.username().is_empty()
        && parsed.password().is_none()
        && parsed.port().is_none()
        && (host == site || host.strip_prefix("www.") == Some(site))
}
